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
use crate::memory::{GenericPageTableImmut, MemFlags, MemoryRegion, MemorySet, PAGE_SIZE};

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
    /// This domain's I/O page table. Empty while unassigned.
    page_table: MemorySet<IoPageTable>,
    /// Guest-physical ranges currently mapped into `page_table`.
    mapped: Vec<(usize, usize)>,
}

impl DeviceDomain {
    pub fn new(domain_id: u16, bdf: Bdf) -> Self {
        Self {
            domain_id,
            bdf,
            owner: DomainOwner::Unassigned,
            page_table: MemorySet::new(),
            mapped: Vec::new(),
        }
    }

    pub fn page_table(&self) -> &IoPageTable {
        self.page_table.page_table()
    }

    pub fn is_assigned(&self) -> bool {
        matches!(self.owner, DomainOwner::Assigned { .. })
    }

    /// Grant the device DMA access to `[start, start + size)`.
    ///
    /// The caller must have already validated that the range is normal-world
    /// memory; this function enforces only the structural invariants
    /// (alignment, non-empty, no overlap with an existing grant).
    pub fn map_range(&mut self, start: usize, size: usize) -> HvResult {
        if size == 0 || start % PAGE_SIZE != 0 || size % PAGE_SIZE != 0 {
            return hv_result_err!(
                EINVAL,
                format!("DMA range [{:#x}, {:#x}) is not page aligned", start, start + size)
            );
        }
        self.page_table
            .insert(MemoryRegion::new_with_offset_mapper(
                start,
                start,
                size,
                MemFlags::READ | MemFlags::WRITE,
            ))?;
        self.mapped.push((start, size));
        Ok(())
    }

    /// Revoke every grant, returning the domain to its fail-closed state.
    pub fn clear(&mut self) {
        self.page_table.clear();
        self.mapped.clear();
    }

    /// Guest-physical ranges this device can currently reach.
    pub fn mapped_ranges(&self) -> &[(usize, usize)] {
        &self.mapped
    }

    /// Whether this domain can translate `gpaddr`, i.e. whether the device
    /// could DMA to it right now.
    pub fn can_access(&self, gpaddr: usize) -> bool {
        self.page_table().query(gpaddr).is_ok()
    }

    pub fn root_paddr(&self) -> usize {
        self.page_table().root_paddr()
    }
}

impl core::fmt::Debug for DeviceDomain {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        f.debug_struct("DeviceDomain")
            .field("domain_id", &self.domain_id)
            .field("bdf", &format_args!("{}", self.bdf))
            .field("owner", &self.owner)
            .field("mapped_ranges", &self.mapped)
            .finish()
    }
}
