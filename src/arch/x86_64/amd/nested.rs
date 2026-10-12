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

//! Nested SVM (AMD), milestone M1: capability exposure.
//!
//! See `docs/nested-svm-design.md`. This module is compiled only with the
//! `nested` feature; with it off the monitor hides SVM from the downgraded
//! host exactly as before, and nothing here is built.
//!
//! M1 exposes just enough for the host's KVM-amd to conclude that SVM is
//! available and usable: the CPUID SVM feature bit (handled in `cpuid.rs`),
//! a masked SVM feature-identification leaf, and the two capability MSRs
//! (`VM_CR`, `VM_HSAVE_PA`) plus the `EFER.SVME` intent bit. The VMRUN
//! emulation that actually runs an L2 lands in later milestones; until then
//! a guest VMRUN still traps to the monitor and is refused.

/// `VM_CR` (0xC001_0114, AMD APM Vol.2 §15.30.1). KVM reads it to decide
/// whether SVM is usable.
pub const MSR_VM_CR: u32 = 0xc001_0114;
/// `VM_HSAVE_PA` (0xC001_0117): host save-area physical address the guest's
/// own SVM sets up. The monitor does not consume it (it saves/loads VMCB01
/// itself), but stores it so the guest's VMSAVE/VMLOAD path stays self-consistent.
pub const MSR_VM_HSAVE_PA: u32 = 0xc001_0117;

/// `VM_CR` bits (APM Vol.2 §15.30.1).
mod vm_cr {
    /// DPD — disable debug-port decode. Not virtualized; reads 0.
    pub const DPD: u64 = 1 << 0;
    /// R_INIT — intercept INIT as #SX. Not virtualized; reads 0.
    pub const R_INIT: u64 = 1 << 1;
    /// DIS_A20M — disable A20 masking. Not virtualized; reads 0.
    pub const DIS_A20M: u64 = 1 << 2;
    /// LOCK — SVM lock bit. We report it set (configuration locked) with
    /// SVMDIS clear, i.e. "SVM enabled and locked", which is what a BIOS
    /// that permits SVM leaves behind.
    pub const LOCK: u64 = 1 << 3;
    /// SVMDIS — SVM disabled. Must read 0 or KVM refuses to use SVM.
    pub const SVMDIS: u64 = 1 << 4;
}

/// The `VM_CR` value the monitor reports to the guest: SVM available
/// (`SVMDIS` clear) and locked (`LOCK` set). Writes are ignored.
pub const fn vm_cr_value() -> u64 {
    vm_cr::LOCK
}

/// `EFER.SVME` (bit 12). The monitor's real EFER always has it set; the
/// guest sees its own intent bit instead (see `rdmsr_virt`/`wrmsr_virt`).
pub const EFER_SVME: u64 = 1 << 12;

/// CPUID leaf 0x8000_000A EDX feature bits (APM Vol.3, "SVM Features").
mod svm_feature {
    /// Nested paging. Must be kept: KVM-amd requires it to build an L2.
    pub const NP: u32 = 1 << 0;
    /// LBR virtualization. Not merged by the monitor; hidden.
    pub const LBR_VIRT: u32 = 1 << 1;
    /// SVM lock. Harmless to report; kept.
    pub const SVML: u32 = 1 << 2;
    /// NRIP save on #VMEXIT. Kept: simplifies L2 exit handling.
    pub const NRIPS: u32 = 1 << 3;
    /// TSC rate control. Not virtualized; hidden.
    pub const TSC_RATE_MSR: u32 = 1 << 4;
    /// VMCB clean bits. L0-internal; hidden from the guest's L2 control.
    pub const VMCB_CLEAN: u32 = 1 << 5;
    /// Flush-by-ASID. Kept: used by the nNPT invalidation path.
    pub const FLUSH_BY_ASID: u32 = 1 << 6;
    /// Decode assists. Not merged yet; hidden.
    pub const DECODE_ASSISTS: u32 = 1 << 7;
    /// Pause filter. Not virtualized; hidden.
    pub const PAUSE_FILTER: u32 = 1 << 10;
    /// Pause filter threshold. Not virtualized; hidden.
    pub const PAUSE_FILTER_THRESH: u32 = 1 << 12;
    /// AVIC. Not virtualized; hidden.
    pub const AVIC: u32 = 1 << 13;
    /// Virtualized VMSAVE/VMLOAD. Not virtualized; hidden.
    pub const VMSAVE_VIRT: u32 = 1 << 15;
}

/// Features the monitor is willing to let the guest's SVM use for its L2.
/// Everything outside this mask is cleared from CPUID leaf 0x8000_000A EDX:
/// a capability the monitor cannot faithfully merge must never be reported,
/// because KVM would then program it into its VMCB12 and expect it to work.
pub const SVM_FEATURE_EDX_MASK: u32 =
    svm_feature::NP | svm_feature::SVML | svm_feature::NRIPS | svm_feature::FLUSH_BY_ASID;

/// Apply the feature mask to a raw leaf 0x8000_000A EDX value.
pub const fn mask_svm_features(edx: u32) -> u32 {
    edx & SVM_FEATURE_EDX_MASK
}

/// Per-vCPU nested-SVM state. Absent (never constructed) until the guest
/// sets `EFER.SVME`, so a host that never loads KVM behaves exactly as the
/// non-nested monitor does.
#[derive(Debug, Default)]
pub struct NestedSvmState {
    /// The guest's intended `EFER.SVME`. The real EFER keeps SVME set
    /// regardless; this is what the guest reads back.
    pub efer_svme: bool,
    /// Guest-physical address the guest wrote to `VM_HSAVE_PA`.
    pub hsave_pa: u64,
    /// Whether an L2 is currently running on this vCPU (VMCB02 active).
    /// Later milestones set this across a VMRUN; M1 never runs an L2.
    pub in_l2: bool,
}

impl NestedSvmState {
    pub const fn new() -> Self {
        Self {
            efer_svme: false,
            hsave_pa: 0,
            in_l2: false,
        }
    }

    /// Serve a read of a nested-SVM capability MSR, or `None` if `msr` is
    /// not one of them (the caller then falls through to its normal path).
    pub fn rdmsr(&self, msr: u32) -> Option<u64> {
        match msr {
            MSR_VM_CR => Some(vm_cr_value()),
            MSR_VM_HSAVE_PA => Some(self.hsave_pa),
            _ => None,
        }
    }

    /// Serve a write of a nested-SVM capability MSR. Returns `true` if `msr`
    /// was consumed here. `VM_CR` is read-only to the guest (writes ignored
    /// but consumed, matching how a locked VM_CR behaves).
    pub fn wrmsr(&mut self, msr: u32, value: u64) -> bool {
        match msr {
            MSR_VM_CR => true,
            MSR_VM_HSAVE_PA => {
                self.hsave_pa = value;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vm_cr_reports_svm_usable() {
        let v = vm_cr_value();
        // SVMDIS must be clear and LOCK set: "SVM enabled, configuration locked".
        assert_eq!(v & vm_cr::SVMDIS, 0);
        assert_ne!(v & vm_cr::LOCK, 0);
        // No other behavioural bits leak in.
        assert_eq!(v & (vm_cr::DPD | vm_cr::R_INIT | vm_cr::DIS_A20M), 0);
    }

    #[test]
    fn test_svm_feature_mask_keeps_np_hides_avic() {
        // Nested paging survives (KVM needs it); AVIC and friends are gone.
        let raw = 0xffff_ffff;
        let masked = mask_svm_features(raw);
        assert_ne!(masked & svm_feature::NP, 0);
        assert_ne!(masked & svm_feature::NRIPS, 0);
        assert_eq!(masked & svm_feature::AVIC, 0);
        assert_eq!(masked & svm_feature::LBR_VIRT, 0);
        assert_eq!(masked & svm_feature::VMSAVE_VIRT, 0);
        assert_eq!(masked & svm_feature::PAUSE_FILTER, 0);
    }

    #[test]
    fn test_hsave_pa_roundtrips() {
        let mut s = NestedSvmState::new();
        assert_eq!(s.rdmsr(MSR_VM_HSAVE_PA), Some(0));
        assert!(s.wrmsr(MSR_VM_HSAVE_PA, 0x1234_5000));
        assert_eq!(s.rdmsr(MSR_VM_HSAVE_PA), Some(0x1234_5000));
        // VM_CR write is consumed but has no effect.
        assert!(s.wrmsr(MSR_VM_CR, !0));
        assert_eq!(s.rdmsr(MSR_VM_CR), Some(vm_cr_value()));
        // Unrelated MSRs are not consumed.
        assert_eq!(s.rdmsr(0x10), None);
        assert!(!s.wrmsr(0x10, 1));
    }

    #[test]
    fn test_state_defaults_closed() {
        let s = NestedSvmState::new();
        assert!(!s.efer_svme);
        assert!(!s.in_l2);
    }
}
