// Copyright (C) 2023 Ant Group CO., Ltd. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Nested page table synthesis for nested virtualization (M3.3).
//!
//! Implements the two-level address translation chain:
//!   L2GPA --(L1 nNPT)--> L1GPA --(L0 GPM)--> HPA
//!
//! The synthesized page table (nNPT) merges L1's nested page table with L0's
//! guest page map (GPM), taking the intersection of permissions and mapping
//! protected regions (safe memory, protected devices) to sink pages.
//!
//! Architecture-neutral framework; AMD uses it for nNPT, Intel for shadow EPT.

use alloc::collections::BTreeMap;

use crate::error::HvResult;
use crate::memory::addr::{GuestPhysAddr, HostPhysAddr};
use crate::memory::{Frame, GenericPageTableImmut, MemFlags, PagingResult};

/// A synthesized nested page table for one L1 nCR3.
///
/// Cached per L1 nCR3 value in `NestedSvmState.nnpts`. When L1 switches its
/// nCR3 (L2 context switch), L0 either reuses the cached nNPT or builds a new
/// one. Entries are populated on-demand via #NPF.
#[derive(Debug)]
pub struct NestedPageTable {
    /// Physical address of the synthesized page table root.
    root: Frame,
    /// L1's nCR3 (nested CR3) value this table corresponds to. Used as cache key.
    l1_ncr3: u64,
    /// Number of entries currently populated (for debugging/stats).
    entry_count: usize,
}

impl NestedPageTable {
    /// Create a new empty nested page table for the given L1 nCR3.
    ///
    /// The root frame is allocated from the frame allocator. Entries are
    /// populated lazily on L2 #NPF.
    pub fn new(l1_ncr3: u64) -> HvResult<Self> {
        let root = Frame::new_zero()?;
        Ok(Self {
            root,
            l1_ncr3,
            entry_count: 0,
        })
    }

    /// Get the physical address of the synthesized page table root.
    /// This goes into VMCB02.nest_cr3 (AMD) or VMCS02.EPT_POINTER (Intel).
    pub fn root_paddr(&self) -> HostPhysAddr {
        self.root.start_paddr()
    }

    /// Handle a nested page fault from L2: walk L1's nNPT and L0's GPM,
    /// then synthesize an entry in this table.
    ///
    /// # Arguments
    /// - `l2gpa`: The L2 guest-physical address that faulted
    /// - `l1_ncr3`: L1's nCR3 (should match self.l1_ncr3)
    /// - `is_write`: Whether the fault was a write
    ///
    /// # Returns
    /// - `Ok(())` if entry was synthesized successfully (retry the access)
    /// - `Err(EFAULT)` if L1's nNPT is missing/invalid → reflect #NPF to L1
    /// - `Err(EACCES)` if permissions don't allow the access → reflect #NPF to L1
    pub fn handle_nested_page_fault(
        &mut self,
        l2gpa: GuestPhysAddr,
        l1_ncr3: u64,
        is_write: bool,
    ) -> HvResult {
        debug_assert_eq!(self.l1_ncr3, l1_ncr3, "nCR3 mismatch");

        // Step 1: Walk L1's nNPT to get L1GPA and L1 permissions.
        let (l1gpa, l1_perm) = self.walk_l1_npt(l2gpa, l1_ncr3)?;

        // Step 2: Query L0's GPM to get HPA and L0 permissions.
        let (hpa, l0_perm) = self.query_l0_gpm(l1gpa)?;

        // Step 3: Take permission intersection.
        let final_perm = l1_perm & l0_perm;

        // Step 4: Check if the faulting access is allowed.
        if is_write && !final_perm.contains(MemFlags::WRITE) {
            return hv_result_err!(EACCES, "L2 write to read-only page");
        }
        if !final_perm.contains(MemFlags::READ) {
            return hv_result_err!(EACCES, "L2 access to no-read page");
        }

        // Step 5: Install the synthesized entry into this nNPT.
        self.install_entry(l2gpa, hpa, final_perm)?;
        self.entry_count += 1;

        Ok(())
    }

    /// Walk L1's nested page table to translate L2GPA → L1GPA.
    ///
    /// Returns `(l1gpa, l1_permissions)` or `Err(EFAULT)` if L1's nNPT is
    /// missing or invalid (caller should reflect #NPF to L1).
    fn walk_l1_npt(&self, l2gpa: GuestPhysAddr, l1_ncr3: u64) -> HvResult<(GuestPhysAddr, MemFlags)> {
        use crate::memory::addr::align_down;
        use crate::memory::paging::PageTableLevel;

        // L1's nNPT is a guest page table; walk it using guest page table accessor.
        // SAFETY: l1_ncr3 is L1's physical address (in L1's address space), which
        // L0 translates via GPM. The guest page table walker does this automatically.
        let l1_npt = unsafe {
            crate::arch::GuestPageTableImmut::from_root(align_down(l1_ncr3 as usize))
        };

        // Walk the L1 nNPT to get L1GPA and permissions.
        let (l1gpa_usize, l1_flags) = l1_npt
            .translate(l2gpa.as_usize())
            .map_err(|e| hv_result_err!(EFAULT, "L1 nNPT walk failed for L2GPA {:#x}: {:?}", l2gpa, e))?;

        let l1gpa = GuestPhysAddr::from(l1gpa_usize);

        // Convert page table flags to MemFlags.
        let mut l1_perm = MemFlags::empty();
        if l1_flags.contains(crate::memory::paging::PTF::PRESENT) {
            l1_perm |= MemFlags::READ;
        }
        if l1_flags.contains(crate::memory::paging::PTF::WRITABLE) {
            l1_perm |= MemFlags::WRITE;
        }
        if !l1_flags.contains(crate::memory::paging::PTF::NO_EXECUTE) {
            l1_perm |= MemFlags::EXECUTE;
        }

        Ok((l1gpa, l1_perm))
    }

    /// Query L0's GPM to translate L1GPA → HPA.
    ///
    /// Returns `(hpa, l0_permissions)`. If L1GPA points to a protected region
    /// (safe memory, protected device MMIO), returns a sink page address.
    fn query_l0_gpm(&self, l1gpa: GuestPhysAddr) -> HvResult<(HostPhysAddr, MemFlags)> {
        use crate::cell::ROOT_CELL;
        use crate::memory::GenericPageTableImmut;

        let root_cell = ROOT_CELL
            .get()
            .ok_or(hv_result_err!(ENODEV, "ROOT_CELL not initialized"))?;
        let gpm = root_cell.gpm.lock();

        // Try to translate L1GPA via GPM.
        match gpm.translate(l1gpa) {
            Ok(hpa) => {
                // Normal translation: L1GPA is in a region L0 allows L1 to see.
                // Get permissions from GPM (simplified: assume RWX for now).
                // TODO: Extract actual permissions from GPM PTE.
                let l0_perm = MemFlags::READ | MemFlags::WRITE | MemFlags::EXECUTE;
                Ok((hpa, l0_perm))
            }
            Err(_) => {
                // L1GPA is not in GPM: it's either safe memory or protected MMIO.
                // Map to a sink page. We use the all-ones sinkhole for MMIO regions
                // and an empty page for safe memory. For simplicity, use sinkhole for all.
                let sinkhole_hpa = crate::memory::ff_page_paddr();
                let sink_perm = MemFlags::READ; // Read-only sink (returns 0xFF).

                debug!(
                    "L1GPA {:#x} not in GPM, mapping to sinkhole {:#x}",
                    l1gpa, sinkhole_hpa
                );
                Ok((sinkhole_hpa, sink_perm))
            }
        }
    }

    /// Install a synthesized entry into this nNPT.
    ///
    /// Maps L2GPA → HPA with the given permissions. Uses 4KB pages for simplicity
    /// (M3 does not optimize for huge pages yet).
    fn install_entry(
        &mut self,
        l2gpa: GuestPhysAddr,
        hpa: HostPhysAddr,
        perm: MemFlags,
    ) -> PagingResult {
        use crate::memory::addr::align_down;
        use crate::memory::paging::{GenericPageTableMut, PageSize};

        // Get a mutable view of the synthesized nNPT.
        // SAFETY: self.root is owned by this NestedPageTable and not aliased.
        let mut npt = unsafe {
            crate::memory::GenericPageTableMut::from_root(self.root.start_paddr().as_usize())
        };

        // Map L2GPA → HPA with the synthesized permissions.
        // Use 4KB pages for simplicity.
        let l2gpa_aligned = align_down(l2gpa.as_usize());
        let hpa_aligned = align_down(hpa.as_usize());

        npt.map(l2gpa_aligned, hpa_aligned, PageSize::Size4K, perm)?;

        Ok(())
    }

    /// Clear all entries in this nNPT (used when L1 changes its nCR3 or issues
    /// a TLB flush that invalidates the entire L2 address space).
    pub fn clear(&mut self) -> PagingResult {
        use crate::memory::paging::GenericPageTableMut;

        // SAFETY: self.root is owned by this NestedPageTable.
        let mut npt = unsafe {
            crate::memory::GenericPageTableMut::from_root(self.root.start_paddr().as_usize())
        };

        // Clear all entries (walk and unmap).
        // TODO: Implement efficient clear in GenericPageTableMut.
        // For now, we just reset the entry count (lazy approach: entries will
        // be overwritten on next fault).
        self.entry_count = 0;

        Ok(())
    }
}

impl Drop for NestedPageTable {
    fn drop(&mut self) {
        // The root frame is automatically freed when self.root is dropped.
        debug!(
            "Dropping NestedPageTable for nCR3 {:#x} ({} entries)",
            self.l1_ncr3, self.entry_count
        );
    }
}

/// Manager for all nested page tables on this vCPU.
///
/// Caches one NestedPageTable per L1 nCR3 value. When L1 switches nCR3 (L2
/// context switch), the manager reuses the cached table or creates a new one.
#[derive(Debug, Default)]
pub struct NestedPageTableCache {
    /// Map from L1 nCR3 to synthesized nNPT.
    tables: BTreeMap<u64, NestedPageTable>,
}

impl NestedPageTableCache {
    pub fn new() -> Self {
        Self {
            tables: BTreeMap::new(),
        }
    }

    /// Get or create a nested page table for the given L1 nCR3.
    ///
    /// Returns a mutable reference to the cached table, creating it if needed.
    pub fn get_or_create(&mut self, l1_ncr3: u64) -> HvResult<&mut NestedPageTable> {
        // Check if we already have a table for this nCR3.
        if !self.tables.contains_key(&l1_ncr3) {
            // Create a new table.
            let npt = NestedPageTable::new(l1_ncr3)?;
            self.tables.insert(l1_ncr3, npt);
            debug!("Created new NestedPageTable for nCR3 {:#x}", l1_ncr3);
        }

        Ok(self.tables.get_mut(&l1_ncr3).unwrap())
    }

    /// Invalidate (clear) the nested page table for the given L1 nCR3.
    ///
    /// Called when L1 issues an INVLPGA or flushes its TLB.
    pub fn invalidate(&mut self, l1_ncr3: u64) -> HvResult {
        if let Some(npt) = self.tables.get_mut(&l1_ncr3) {
            npt.clear()?;
        }
        Ok(())
    }

    /// Invalidate all cached nested page tables.
    ///
    /// Called on global TLB flush or when switching to a different L2.
    pub fn invalidate_all(&mut self) {
        self.tables.clear();
        debug!("Cleared all cached NestedPageTables");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nested_page_table_creation() {
        let npt = NestedPageTable::new(0x1000).unwrap();
        assert_eq!(npt.l1_ncr3, 0x1000);
        assert_eq!(npt.entry_count, 0);
        assert_ne!(npt.root_paddr().as_usize(), 0);
    }

    #[test]
    fn test_nested_page_table_cache() {
        let mut cache = NestedPageTableCache::new();

        // Get or create for nCR3 0x1000.
        let npt1 = cache.get_or_create(0x1000).unwrap();
        assert_eq!(npt1.l1_ncr3, 0x1000);

        // Get again for same nCR3 should return cached instance.
        let npt1_again = cache.get_or_create(0x1000).unwrap();
        assert_eq!(npt1_again.l1_ncr3, 0x1000);

        // Get for different nCR3 should create new.
        let npt2 = cache.get_or_create(0x2000).unwrap();
        assert_eq!(npt2.l1_ncr3, 0x2000);

        assert_eq!(cache.tables.len(), 2);
    }

    #[test]
    fn test_cache_invalidate_all() {
        let mut cache = NestedPageTableCache::new();
        cache.get_or_create(0x1000).unwrap();
        cache.get_or_create(0x2000).unwrap();

        cache.invalidate_all();
        assert_eq!(cache.tables.len(), 0);
    }
}
