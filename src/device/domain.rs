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

//! Per-device DMA domains.
//!
//! The pre-existing IOMMU setup is single-domain: every device in the system
//! shares one I/O page table (`ROOT_CELL.dma_regions`), so DMA isolation was
//! all-or-nothing against the monitor's own memory. A protected accelerator
//! needs its own domain, because the point is to let *one* owner reach it
//! while the host cannot.
//!
//! A domain starts with an **empty** page table. That is the fail-closed
//! datapath: until something is explicitly assigned, every DMA the device
//! issues misses the page table and faults in the IOMMU. Hiding the device's
//! registers from the host (the control plane) and starving its DMA (the data
//! plane) are independent, and both are on by default.

use alloc::vec::Vec;

use super::bdf::Bdf;
use crate::arch::vmm::IoPageTable;
use crate::error::HvResult;
use crate::memory::{MemFlags, MemoryRegion, MemorySet, PAGE_SIZE};

/// Domain id 0 is the shared default domain that every unprotected device
/// stays in (the `domain_id = 1` the VT-d code already writes is that
/// domain's hardware id; see `vtd.rs`). Protected devices get ids from
/// `FIRST_PRIVATE_DOMAIN_ID` upwards so they can never collide with it.
pub const FIRST_PRIVATE_DOMAIN_ID: u16 = 16;

/// Who a protected device currently belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainOwner {
    /// Nobody. The device is hidden from the host and its DMA is starved.
    /// This is the state every protected device boots into.
    Unassigned,
    /// Assigned to a protected domain, identified by the opaque handle the
    /// assigning hypercall supplied. The handle is how a future VM-based
    /// consumer names itself; the monitor does not interpret it.
    Assigned { handle: u64 },
}

/// One I/O-virtual to physical grant in a device's DMA domain.
///
/// `iova` is the address the device emits (for a VM-owned device, the VM's
/// guest-physical address); `paddr` is the host-physical frame it reaches.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DmaMapping {
    pub iova: u64,
    pub paddr: u64,
    pub size: u64,
}

impl DmaMapping {
    fn iova_end(&self) -> u64 {
        self.iova + self.size
    }

    /// Structural checks only: page alignment, non-empty, no wrap-around.
    /// Whether `paddr` is memory the device may reach is the caller's call.
    fn validate(&self) -> HvResult {
        let page = PAGE_SIZE as u64;
        let aligned = self.iova % page == 0 && self.paddr % page == 0 && self.size % page == 0;
        let wraps = self.iova.checked_add(self.size).is_none()
            || self.paddr.checked_add(self.size).is_none();
        if self.size == 0 || !aligned || wraps {
            return hv_result_err!(
                EINVAL,
                format!(
                    "invalid DMA mapping iova {:#x} -> paddr {:#x} size {:#x}",
                    self.iova, self.paddr, self.size
                )
            );
        }
        Ok(())
    }
}

/// A DMA domain owning exactly one device.
///
/// One device per domain, deliberately: sharing a domain between accelerators
/// would let a compromised one DMA into the other's memory, which is the very
/// boundary this is meant to draw. Multi-function cards get one domain per
/// function for the same reason.
pub struct DeviceDomain {
    /// Hardware domain id programmed into the IOMMU (VT-d DID / AMD-Vi
    /// domain, ARM SMMU VMID).
    pub domain_id: u16,
    pub bdf: Bdf,
    pub owner: DomainOwner,
    /// This domain's I/O page table. Empty while unassigned. Created on
    /// first use rather than in `new()`: allocating the root frame up front
    /// would make host-side unit tests depend on the monitor's frame
    /// allocator, which does not exist there.
    page_table: Option<MemorySet<IoPageTable>>,
    /// Grants currently mapped into `page_table`.
    mapped: Vec<DmaMapping>,
}

impl DeviceDomain {
    pub fn new(domain_id: u16, bdf: Bdf) -> Self {
        Self {
            domain_id,
            bdf,
            owner: DomainOwner::Unassigned,
            page_table: None,
            mapped: Vec::new(),
        }
    }

    /// The domain's page table, allocated on first use.
    fn ensure_page_table(&mut self) -> &mut MemorySet<IoPageTable> {
        self.page_table.get_or_insert_with(MemorySet::new)
    }

    pub fn page_table(&mut self) -> &IoPageTable {
        self.ensure_page_table().page_table()
    }

    pub fn is_assigned(&self) -> bool {
        matches!(self.owner, DomainOwner::Assigned { .. })
    }

    fn overlaps_existing(&self, m: &DmaMapping) -> bool {
        self.mapped
            .iter()
            .any(|e| m.iova < e.iova_end() && e.iova < m.iova_end())
    }

    /// Add a batch of grants, all or nothing.
    ///
    /// Every mapping is checked (structure, overlap with existing grants and
    /// with the rest of the batch) before the page table is touched, so a bad
    /// batch is refused without side effects; a page-table failure half way
    /// rolls back what this batch already inserted.
    pub fn map_batch(&mut self, batch: &[DmaMapping]) -> HvResult {
        let committed = self.mapped.len();
        for m in batch {
            let checked = m.validate().and_then(|()| {
                if self.overlaps_existing(m) {
                    hv_result_err!(
                        EEXIST,
                        format!("iova {:#x} size {:#x} already mapped", m.iova, m.size)
                    )
                } else {
                    Ok(())
                }
            });
            if let Err(e) = checked {
                self.mapped.truncate(committed);
                return Err(e);
            }
            // Staged in `mapped` so later batch entries see it for overlap.
            self.mapped.push(*m);
        }
        for i in committed..self.mapped.len() {
            let m = self.mapped[i];
            let region = MemoryRegion::new_with_offset_mapper(
                m.iova as usize,
                m.paddr as usize,
                m.size as usize,
                MemFlags::READ | MemFlags::WRITE,
            );
            if let Err(e) = self.ensure_page_table().insert(region) {
                for done in &self.mapped[committed..i] {
                    if let Some(pt) = &mut self.page_table {
                        // Best effort: the region was inserted by this very
                        // batch, so deleting it can only fail on corruption.
                        let _ = pt.delete(done.iova as usize);
                    }
                }
                self.mapped.truncate(committed);
                return Err(e);
            }
        }
        Ok(())
    }

    /// Revoke every grant, returning the domain to its fail-closed state.
    /// The page-table root survives, so the IOMMU entry stays valid.
    pub fn clear(&mut self) {
        if let Some(pt) = &mut self.page_table {
            pt.clear();
        }
        self.mapped.clear();
    }

    /// Grants this device currently holds.
    pub fn mappings(&self) -> &[DmaMapping] {
        &self.mapped
    }

    /// Whether the device could DMA to `iova` right now. `mapped` is the
    /// ground truth for grants, so this needs no page-table walk (and works
    /// in host unit tests).
    pub fn can_access(&self, iova: u64) -> bool {
        self.mapped
            .iter()
            .any(|m| iova.wrapping_sub(m.iova) < m.size)
    }
}

impl core::fmt::Debug for DeviceDomain {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        f.debug_struct("DeviceDomain")
            .field("domain_id", &self.domain_id)
            .field("bdf", &format_args!("{}", self.bdf))
            .field("owner", &self.owner)
            .field("mappings", &self.mapped)
            .finish()
    }
}
