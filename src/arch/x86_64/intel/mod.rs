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

mod enclave;
mod ept;
mod structs;
mod vcpu;
mod vmexit;
mod vtd;

use libvmm::msr::Msr;
use libvmm::vmx::Vmcs;
use x86::vmx::VmFail;

use crate::arch::cpu::check_cpuid;
use crate::arch::cpuid::CpuFeatures;
use crate::error::{HvError, HvResult};

pub use ept::EPTEntry as NPTEntry;
pub use ept::EnclaveExtendedPageTableUnlocked as EnclaveNestedPageTableUnlocked;
pub use ept::ExtendedPageTable as NestedPageTable;
pub use vcpu::Vcpu;
pub use vtd::{IoPTEntry, IoPageTable, Iommu};

use libvmm::vmx::flags::VmExitControls as ExitCtrl;

const VMEXIT_CTRL_MIN: u32 = ExitCtrl::HOST_ADDR_SPACE_SIZE.bits()
    | ExitCtrl::SAVE_IA32_PAT.bits()
    | ExitCtrl::LOAD_IA32_PAT.bits()
    | ExitCtrl::SAVE_IA32_EFER.bits()
    | ExitCtrl::LOAD_IA32_EFER.bits();

impl From<VmFail> for HvError {
    fn from(err: VmFail) -> Self {
        match err {
            VmFail::VmFailValid => hv_err!(
                EIO,
                format!("{:?}: {:x?}", err, Vmcs::instruction_error().unwrap())
            ),
            _ => hv_err!(EIO, format!("{:?}", err)),
        }
    }
}

/// Locate the PCIe enhanced configuration window (MMCFG).
///
/// On Intel the DMI/host bridge at 0000:00:00.0 holds PCIEXBAR at config
/// offset 0x60: bit 0 enables the window, bits 2:1 select its length
/// (256/128/64 MB) and the base occupies the upper bits. Read through the
/// legacy port pair, which needs no mapping.
pub fn ecam_base() -> Option<usize> {
    use crate::device::bdf::Bdf;
    use crate::device::pci::ConfigAccess;

    const PCIEXBAR: u16 = 0x60;
    let cfg = crate::arch::PciConfig;
    let host_bridge = Bdf::new(0, 0, 0);

    let low = cfg.read32(host_bridge, PCIEXBAR);
    let high = cfg.read32(host_bridge, PCIEXBAR + 4);
    // Bit 0 is PCIEXBAREN.
    if low & 1 == 0 {
        return None;
    }
    let raw = ((high as u64) << 32) | (low as u64);
    // Length field (bits 2:1): 00 = 256MB, 01 = 128MB, 10 = 64MB. The base
    // is aligned to the window size, so mask accordingly.
    let mask: u64 = match (low >> 1) & 0b11 {
        0b00 => !0x0fff_ffff,
        0b01 => !0x07ff_ffff,
        0b10 => !0x03ff_ffff,
        _ => return None,
    };
    let base = (raw & mask) as usize;
    if base == 0 {
        return None;
    }
    Some(base)
}

pub fn check_hypervisor_feature() -> HvResult {
    // Check cpuid
    check_cpuid()?;

    if !CpuFeatures::new().has_vmx() {
        warn!("Feature VMX not supported!");
        return hv_result_err!(ENODEV, "VMX feature checks failed!");
    }

    let vmexit_ctrl = (Msr::IA32_VMX_EXIT_CTLS.read() >> 32) as u32;
    if (!vmexit_ctrl) & VMEXIT_CTRL_MIN != 0 {
        return hv_result_err!(ENODEV, "required VmExitControls flags checks failed!");
    }

    Ok(())
}
