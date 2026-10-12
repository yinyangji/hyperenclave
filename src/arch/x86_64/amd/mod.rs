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
mod iommu;
mod mem_encrypt;
#[cfg(feature = "nested")]
pub mod nested;
#[cfg(feature = "nested")]
mod vmcb_merge;
mod npt;
mod vcpu;
mod vmexit;

use libvmm::svm::flags::{VmCr, VmCrFlags};

use crate::arch::cpu::check_cpuid;
use crate::error::HvResult;

pub use iommu::{IoPTEntry, IoPageTable, Iommu};
pub use mem_encrypt::{EncHW, HmacSWEncHW};
pub use npt::{EnclaveNestedPageTableUnlocked, NPTEntry, NestedPageTable};
pub use vcpu::Vcpu;

/// Locate the PCIe enhanced configuration window (MMCFG).
///
/// AMD reports it in `MSR_MMIO_CFG_BASE` (0xC001_0058): bit 0 enables the
/// window, bits 5:2 encode the bus range as a power of two, and bits 47:20
/// hold the base (AMD APM Vol.2 §3.2.14).
pub fn ecam_base() -> Option<usize> {
    const MSR_MMIO_CFG_BASE: u32 = 0xc001_0058;

    // SAFETY: reading an AMD-defined MSR on an AMD CPU. The monitor runs at
    // the highest privilege level, and `check_hypervisor_feature` has already
    // confirmed this is an SVM-capable AMD part.
    let raw = unsafe { x86::msr::rdmsr(MSR_MMIO_CFG_BASE) };
    // Bit 0 is MmioCfgEnable.
    if raw & 1 == 0 {
        return None;
    }
    let base = (raw & 0x0000_ffff_fff0_0000) as usize;
    if base == 0 {
        return None;
    }
    Some(base)
}

pub fn check_hypervisor_feature() -> HvResult {
    check_cpuid()?;
    if VmCr::read().contains(VmCrFlags::SVMDIS) {
        return hv_result_err!(ENODEV, "SVM disabled by BIOS!");
    }
    Ok(())
}
