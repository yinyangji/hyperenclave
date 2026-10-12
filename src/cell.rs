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

use alloc::vec::Vec;

use crate::arch::{vmm::IoPageTable, HostPageTable, NestedPageTable};
use crate::config::HvSystemConfig;
use crate::consts::{HV_BASE, PER_CPU_SIZE};
use crate::device;
use crate::error::HvResult;
use crate::header::HvHeader;
use crate::hypercall::tc::TpmMode;
use crate::intervaltree::IntervalTree;
use crate::memory::addr::{phys_to_virt, GuestPhysAddr, HostPhysAddr, HostVirtAddr};
use crate::memory::cmr::NR_INIT_EPC_RANGES;
use crate::memory::{MemFlags, MemoryRegion, MemorySet};

#[derive(Debug)]
pub struct Cell {
    /// Guest physical memory set.
    pub gpm: MemorySet<NestedPageTable>,
    /// Host virtual memory set.
    pub hvm: MemorySet<HostPageTable>,
    /// DMA memory set.
    pub dma_regions: MemorySet<IoPageTable>,
    /// Normal world region which can be accessed by hypervisor.
    normal_world_mem_region: IntervalTree,
}

impl Cell {
    fn new_root() -> HvResult<Self> {
        let header = HvHeader::get();
        let sys_config = HvSystemConfig::get();

        let hv_phys_start = sys_config.hypervisor_memory.phys_start as usize;
        let hv_phys_size = sys_config.hypervisor_memory.size as usize;
        let mut gpm = MemorySet::new();
        let mut hvm = MemorySet::new();
        let mut dma_regions = MemorySet::new();
        let mut normal_world_mem_region = IntervalTree::new();

        // Init guest physical memory set, create hypervisor page table.
        //
        // map hypervisor memory to empty in gpm
        // It returns the plaintext view of the empty page
        // if untrusted world dump trusted memory
        gpm.insert(MemoryRegion::new_with_empty_mapper(
            hv_phys_start,
            hv_phys_size,
            MemFlags::READ | MemFlags::ENCRYPTED,
        ))?;
        // preventing guest vm read out the encrypted view of EPC
        // from high addr with c-bit = 1
        // expected behavior: return plaintext view of the empty page
        #[cfg(feature = "sme")]
        gpm.insert(MemoryRegion::new_with_empty_mapper(
            crate::memory::addr::phys_encrypted(hv_phys_start),
            hv_phys_size,
            MemFlags::READ | MemFlags::ENCRYPTED,
        ))?;
        // map epc memory to empty page in gpm
        for epc_range in &header.init_epc_ranges[..*NR_INIT_EPC_RANGES] {
            let epc_start_hpa = epc_range.start as HostPhysAddr;
            let epc_size = epc_range.size;
            info!(
                "map epc to empty page in gpm, epc_range: [{:#x}, {:#x}], {:#x}",
                epc_start_hpa,
                epc_start_hpa + epc_size - 1,
                epc_size
            );
            gpm.insert(MemoryRegion::new_with_empty_mapper(
                epc_start_hpa,
                epc_size,
                MemFlags::READ | MemFlags::ENCRYPTED,
            ))?;
            #[cfg(feature = "sme")]
            gpm.insert(MemoryRegion::new_with_empty_mapper(
                crate::memory::addr::phys_encrypted(epc_start_hpa),
                epc_size,
                MemFlags::READ | MemFlags::ENCRYPTED,
            ))?;
        }

        // all physical memory regions
        for region in sys_config.mem_regions() {
            let phys_start = region.phys_start;
            let virt_start = region.virt_start;
            let size = region.size;
            let flags = region.flags;
            let r = MemoryRegion::new_with_offset_mapper(
                virt_start as GuestPhysAddr,
                phys_start as HostPhysAddr,
                size as usize,
                flags - MemFlags::ENCRYPTED, // guest should not read decrypted data
            );
            if flags.contains(MemFlags::DMA) {
                dma_regions.insert(r.clone())?;
            } else {
                for rmrr_range in sys_config.rmrr_ranges() {
                    //if region contains rmrr_range
                    if phys_start <= rmrr_range.base && rmrr_range.limit <= phys_start + size {
                        dma_regions.insert(r.clone())?;
                        break;
                    }
                }
            }
            // A protected accelerator's registers must not appear in the
            // host's view. The driver hands us one region per memory range,
            // so the device's apertures usually arrive inside a much larger
            // MMIO region: insert the region in pieces, skipping the parts
            // the device owns, then map those to the all-ones page.
            Self::insert_gpm_region_with_device_holes(&mut gpm, r)?;
        }
        Self::insert_device_sinkholes(&mut gpm)?;

        // Init host virtual memory set, create host page table.
        let core_and_percpu_size =
            header.core_size as usize + header.max_cpus as usize * PER_CPU_SIZE;
        // hypervisor core
        hvm.insert(MemoryRegion::new_with_offset_mapper(
            HV_BASE,
            hv_phys_start,
            header.core_size,
            MemFlags::READ | MemFlags::WRITE | MemFlags::EXECUTE | MemFlags::ENCRYPTED,
        ))?;
        // configurations & hypervisor free memory
        hvm.insert(MemoryRegion::new_with_offset_mapper(
            HV_BASE + core_and_percpu_size,
            hv_phys_start + core_and_percpu_size,
            hv_phys_size - core_and_percpu_size,
            MemFlags::READ | MemFlags::WRITE | MemFlags::ENCRYPTED,
        ))?;

        if header.tpm_type != TpmMode::Fake as u32 {
            hvm.insert(MemoryRegion::new_with_offset_mapper(
                header.tpm_mmio_pa,
                header.tpm_mmio_pa,
                header.tpm_mmio_size as usize,
                MemFlags::READ | MemFlags::WRITE,
            ))?;
            println!("Not Fake TPM, mmio is mapped va={:#x}", header.tpm_mmio_pa);
        }

        // guest RAM
        for region in sys_config.mem_regions() {
            let phys_start = region.phys_start;
            let virt_start = region.virt_start;
            let size = region.size;
            let flags = region.flags;
            if flags.contains(MemFlags::DMA) {
                let hv_virt_start = phys_to_virt(virt_start as GuestPhysAddr);
                if hv_virt_start < virt_start as GuestPhysAddr {
                    return hv_result_err!(
                        EINVAL,
                        format!("Guest physical address {:#x} is too large", virt_start)
                    );
                }
                hvm.insert(MemoryRegion::new_with_offset_mapper(
                    hv_virt_start,
                    phys_start as HostPhysAddr,
                    size as usize,
                    MemFlags::READ | MemFlags::WRITE,
                ))?;
                // Support hardware encrypt when swap out EPC page to guest RAM
                #[cfg(feature = "sme")]
                hvm.insert(MemoryRegion::new_with_offset_mapper(
                    region.virt_start as HostVirtAddr,
                    region.phys_start as HostPhysAddr,
                    region.size as usize,
                    MemFlags::READ | MemFlags::WRITE | MemFlags::ENCRYPTED,
                ))?;
                normal_world_mem_region.insert(
                    (region.phys_start as usize)..(region.phys_start + region.size) as usize,
                )?;
            }
        }
        // EPC memory
        {
            debug!("NR_INIT_EPC_RANGES: {}", *NR_INIT_EPC_RANGES);
            for epc_range in &header.init_epc_ranges[..*NR_INIT_EPC_RANGES] {
                let epc_start_hpa = epc_range.start as HostPhysAddr;
                let epc_start_hva = phys_to_virt(epc_start_hpa) as HostVirtAddr;
                let epc_size = epc_range.size;
                debug!(
                    "epc_range: [{:#x}, {:#x}], {:#x}",
                    epc_start_hpa,
                    epc_start_hpa + epc_size - 1,
                    epc_size
                );
                hvm.insert(MemoryRegion::new_with_offset_mapper(
                    epc_start_hva,
                    epc_start_hpa,
                    epc_size,
                    MemFlags::READ | MemFlags::WRITE | MemFlags::ENCRYPTED,
                ))?;
            }
        }
        // IOMMU
        for iommu in sys_config.iommu_units() {
            let paddr = iommu.base as HostPhysAddr;
            hvm.insert(MemoryRegion::new_with_offset_mapper(
                phys_to_virt(paddr),
                paddr,
                iommu.size as usize,
                MemFlags::READ | MemFlags::WRITE,
            ))?;
        }

        Ok(Self {
            gpm,
            hvm,
            dma_regions,
            normal_world_mem_region,
        })
    }

    /// Insert `region` into the guest's page table, leaving out any part a
    /// protected accelerator owns.
    ///
    /// The driver describes system memory in large ranges, so a device's
    /// 4KB configuration page or multi-GB BAR usually sits in the middle of
    /// one of them. Splitting here (rather than asking the driver for finer
    /// regions) keeps the kernel ABI untouched.
    fn insert_gpm_region_with_device_holes(
        gpm: &mut MemorySet<NestedPageTable>,
        region: MemoryRegion<GuestPhysAddr>,
    ) -> HvResult {
        let holes = device::protected_mmio_ranges();
        if holes.is_empty() {
            return gpm.insert(region);
        }

        let region_start = region.start;
        let region_end = region_start + region.size;
        // Physical base of this region, recovered through the mapper so the
        // split pieces keep the original GPA -> HPA relationship.
        let phys_base = region.target_paddr();

        // Collect the holes overlapping this region, in address order.
        let mut overlaps: Vec<(GuestPhysAddr, GuestPhysAddr)> = holes
            .iter()
            .map(|&(start, size)| (start, start + size))
            .filter(|&(start, end)| start < region_end && end > region_start)
            .map(|(start, end)| (start.max(region_start), end.min(region_end)))
            .collect();
        if overlaps.is_empty() {
            return gpm.insert(region);
        }
        overlaps.sort_unstable();

        let mut cursor = region_start;
        for (hole_start, hole_end) in overlaps {
            // The hole list may contain adjacent or overlapping entries
            // (a device with several BARs in one region).
            if hole_start > cursor {
                gpm.insert(MemoryRegion::new_with_offset_mapper(
                    cursor,
                    phys_base + (cursor - region_start),
                    hole_start - cursor,
                    region.flags,
                ))?;
            }
            cursor = cursor.max(hole_end);
        }
        if cursor < region_end {
            gpm.insert(MemoryRegion::new_with_offset_mapper(
                cursor,
                phys_base + (cursor - region_start),
                region_end - cursor,
                region.flags,
            ))?;
        }
        Ok(())
    }

    /// Map every protected accelerator's configuration space and BAR
    /// apertures to the all-ones page.
    ///
    /// This runs after the system's own regions so the holes punched above
    /// are filled by a mapping that reads as absent hardware. Regions the
    /// driver never described are covered too: a BAR outside every reported
    /// range would otherwise be unmapped, and an unmapped access faults the
    /// host instead of quietly reading 0xFF.
    fn insert_device_sinkholes(gpm: &mut MemorySet<NestedPageTable>) -> HvResult {
        for (start, size) in device::protected_mmio_ranges() {
            gpm.insert(MemoryRegion::new_with_ff_mapper(
                start as GuestPhysAddr,
                size,
                MemFlags::READ | MemFlags::WRITE,
            ))?;
            info!(
                "device MMIO [{:#x}, {:#x}) hidden from host (reads 0xFF)",
                start,
                start + size
            );
        }
        Ok(())
    }

    /// Whether all of [`start`, `start` + `size`) is ordinary host RAM a
    /// device may be granted DMA to: inside one driver-reported normal-world
    /// region and clear of the Convertible Memory Range (EPC, current or
    /// future). Ranges spanning two adjacent regions are refused; callers
    /// split them.
    pub fn is_normal_world_dma_range(&self, start: GuestPhysAddr, size: usize) -> bool {
        let Some(end) = start.checked_add(size) else {
            return false;
        };
        self.normal_world_mem_region.contains_range(start..end)
            && !crate::memory::cmr::ConvMemManager::overlaps_conv_mem(start, size)
    }

    /// Whether [gpaddr, gpaddr + 4kB) is accessible by hypervisor.
    pub fn is_valid_normal_world_gpaddr(&self, gpaddr: GuestPhysAddr) -> bool {
        self.normal_world_mem_region.contains(&gpaddr)
    }
}

lazy_static! {
    pub static ref ROOT_CELL: Cell = Cell::new_root().unwrap();
}

pub fn init() -> HvResult {
    crate::arch::vmm::check_hypervisor_feature()?;

    // Enumerate accelerators before the root cell is built: `Cell::new_root()`
    // needs the device list to decide what to withhold from the guest's
    // memory view, and this must happen before the host enumerates the bus
    // itself.
    device::init()?;

    lazy_static::initialize(&ROOT_CELL);

    info!("Root cell init end.");
    debug!("{:#x?}", &*ROOT_CELL);
    Ok(())
}
