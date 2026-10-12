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

//! Protected-device hypercalls (DeviceList / DeviceAssign / DeviceMapDma /
//! DeviceRelease).
//!
//! The ABI follows VFIO's shape: claim a device, then grant its DMA domain
//! memory in batches, because a VM's RAM is rarely physically contiguous.
//! None of these calls ever makes a protected device visible to the host
//! again; they only decide what memory the device itself can reach.
//!
//! Expected failures (unknown device, busy, bad range) are returned as a
//! negative errno in the return register rather than as `HvError`, which the
//! dispatcher would turn into a #GP inside the host kernel. A malformed guest
//! pointer still faults through `GuestPtr`, as for every other hypercall.

use core::convert::TryFrom;

use super::error::HyperCallResult;
use super::HyperCall;
use crate::cell::ROOT_CELL;
use crate::device::bdf::Bdf;
use crate::device::domain::{DmaMapping, DomainOwner};
use crate::device::DEVICE_MANAGER;
use crate::error::HvResult;
use crate::memory::gaccess::GuestPtr;

/// Entries returned by one `DeviceList` call. `total` reports the real
/// count, so a caller can tell when the list was truncated.
pub const DEVICE_LIST_MAX: usize = 32;
/// Mappings accepted by one `DeviceMapDma` call.
pub const DMA_MAP_BATCH_MAX: usize = 64;

pub const DEVICE_FLAG_ASSIGNED: u16 = 1 << 0;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DeviceInfo {
    pub bdf: u16,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_code: u16,
    pub domain_id: u16,
    pub flags: u16,
    pub nr_bars: u32,
    /// Owner handle when `DEVICE_FLAG_ASSIGNED` is set, else 0.
    pub owner: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DeviceListBuf {
    /// Entries filled in `devices`.
    pub count: u32,
    /// Protected devices in the system.
    pub total: u32,
    pub devices: [DeviceInfo; DEVICE_LIST_MAX],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct DeviceDmaMapReq {
    pub bdf: u16,
    pub _reserved: [u16; 3],
    pub handle: u64,
    pub nr_maps: u32,
    pub _reserved2: u32,
    pub maps: [DmaMapping; DMA_MAP_BATCH_MAX],
}

/// Fold an expected failure into the return register.
fn errno_ret(result: HvResult) -> usize {
    match result {
        Ok(()) => 0,
        Err(e) => {
            warn!("device hypercall refused: {:?}", e);
            e.num().code() as isize as usize
        }
    }
}

fn bdf_arg(arg: u64) -> HvResult<Bdf> {
    u16::try_from(arg)
        .map(Bdf::from_raw)
        .map_err(|_| hv_err!(EINVAL, format!("{:#x} is not a BDF", arg)))
}

impl HyperCall<'_> {
    pub(super) fn device_list(
        &self,
        mut buf_ptr: GuestPtr<DeviceListBuf>,
    ) -> HyperCallResult<usize> {
        let empty = DeviceInfo {
            bdf: 0,
            vendor_id: 0,
            device_id: 0,
            class_code: 0,
            domain_id: 0,
            flags: 0,
            nr_bars: 0,
            owner: 0,
        };
        let mut buf = DeviceListBuf {
            count: 0,
            total: 0,
            devices: [empty; DEVICE_LIST_MAX],
        };
        let snapshot = DEVICE_MANAGER
            .try_get()
            .map(|m| m.snapshot())
            .unwrap_or_default();
        buf.total = snapshot.len() as u32;
        for (slot, (dev, owner)) in buf.devices.iter_mut().zip(&snapshot) {
            let (flags, owner) = match *owner {
                DomainOwner::Assigned { handle } => (DEVICE_FLAG_ASSIGNED, handle),
                DomainOwner::Unassigned => (0, 0),
            };
            *slot = DeviceInfo {
                bdf: dev.bdf().raw(),
                vendor_id: dev.ident.vendor_id,
                device_id: dev.ident.device_id,
                class_code: dev.ident.class_code,
                domain_id: dev.domain_id,
                flags,
                nr_bars: dev.bars.len() as u32,
                owner,
            };
            buf.count += 1;
        }
        buf_ptr.write(buf)?;
        Ok(0)
    }

    pub(super) fn device_assign(&self, bdf: u64, handle: u64) -> HyperCallResult<usize> {
        Ok(errno_ret(bdf_arg(bdf).and_then(|bdf| {
            DEVICE_MANAGER
                .try_get()
                .ok_or_else(|| hv_err!(ENODEV))?
                .assign(bdf, handle)
        })))
    }

    pub(super) fn device_map_dma(
        &self,
        req_ptr: GuestPtr<DeviceDmaMapReq>,
    ) -> HyperCallResult<usize> {
        let req = req_ptr.read()?;
        let result = (|| {
            let nr = req.nr_maps as usize;
            if nr > DMA_MAP_BATCH_MAX {
                return hv_result_err!(E2BIG, format!("{} mappings in one batch", nr));
            }
            let batch = &req.maps[..nr];
            // The device must never reach the monitor, EPC, or another
            // device's MMIO: only memory the host itself legitimately owns.
            for m in batch {
                if !ROOT_CELL.is_normal_world_dma_range(m.paddr as usize, m.size as usize) {
                    return hv_result_err!(
                        EPERM,
                        format!(
                            "paddr {:#x} size {:#x} is not normal-world RAM",
                            m.paddr, m.size
                        )
                    );
                }
            }
            DEVICE_MANAGER
                .try_get()
                .ok_or_else(|| hv_err!(ENODEV))?
                .map_dma(
                    Bdf::from_raw(req.bdf),
                    req.handle,
                    batch,
                    crate::iommu::active_iommus(),
                )
        })();
        Ok(errno_ret(result))
    }

    pub(super) fn device_release(&self, bdf: u64) -> HyperCallResult<usize> {
        Ok(errno_ret(bdf_arg(bdf).and_then(|bdf| {
            DEVICE_MANAGER
                .try_get()
                .ok_or_else(|| hv_err!(ENODEV))?
                .release(bdf, crate::iommu::active_iommus())
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::size_of;

    /// The layouts are shared with the out-of-tree driver; pin them.
    #[test]
    fn test_abi_layout() {
        assert_eq!(size_of::<DeviceInfo>(), 24);
        assert_eq!(size_of::<DeviceListBuf>(), 8 + 24 * DEVICE_LIST_MAX);
        assert_eq!(size_of::<DmaMapping>(), 24);
        assert_eq!(size_of::<DeviceDmaMapReq>(), 24 + 24 * DMA_MAP_BATCH_MAX);
    }

    #[test]
    fn test_bdf_arg() {
        assert_eq!(bdf_arg(0x6500).unwrap(), Bdf::new(0x65, 0, 0));
        assert!(bdf_arg(0x1_0000).is_err());
    }
}
