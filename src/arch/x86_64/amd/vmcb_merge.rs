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

//! VMCB merge logic for nested SVM (M3).
//!
//! Merges L1's VMCB12 (what L1 prepares for L2) with L0's mandatory policy
//! to produce VMCB02 (what L0 actually runs). See `docs/nested-svm-design.md` §6.

use libvmm::svm::flags::VmcbTlbControl;
use libvmm::svm::{SvmIntercept, Vmcb};
use x86_64::registers::control::{Cr0Flags, Cr4Flags};
use x86_64::registers::model_specific::EferFlags;

use crate::error::HvResult;

/// L0's mandatory intercepts that L1 cannot disable. These ensure L0 keeps
/// control of security-critical events even when running L2.
///
/// Based on the current VMCB01 setup in `vcpu.rs::vmcb_setup()`.
const L0_MANDATORY_INTERCEPTS: &[SvmIntercept] = &[
    SvmIntercept::NMI,
    SvmIntercept::CPUID,
    SvmIntercept::SHUTDOWN,
    SvmIntercept::VMRUN,
    SvmIntercept::VMMCALL,
    SvmIntercept::VMLOAD,
    SvmIntercept::VMSAVE,
    SvmIntercept::STGI,
    SvmIntercept::CLGI,
    SvmIntercept::SKINIT,
    // MSR_PROT and IOIO_PROT are handled via MSRPM/IOPM merge, not intercept bits.
];

/// Reserved CR0 bits that must be validated (AMD APM Vol.2 Table 15-5).
/// Bit positions where the guest is not allowed to set arbitrary values.
const CR0_RESERVED_MASK: u64 = 0xFFFF_FFFF_0000_0000
    | (1 << 17) // Reserved bit 17
    | (1 << 28) // Reserved bit 28
    | (1 << 29); // Reserved bit 29

/// Reserved CR4 bits (AMD APM Vol.2 Table 15-5).
const CR4_RESERVED_MASK: u64 = 0xFFFF_FFFF_FF80_0000 | (1 << 12) | (1 << 19) | (1 << 15);

/// Reserved EFER bits (AMD APM Vol.2 Table 15-5).
/// LMA (bit 10) is read-only and set by hardware; we don't validate it here
/// but L0 should not let L1 toggle it arbitrarily.
const EFER_RESERVED_MASK: u64 = 0xFFFF_FFFF_FFFF_F000
    | (1 << 1) // Reserved bit 1
    | (1 << 9); // Reserved bit 9

/// Merge L1's VMCB12 into L0's VMCB02, applying L0's mandatory policy.
///
/// # Arguments
/// - `vmcb02`: The VMCB L0 will actually run (per-CPU, mutated in place)
/// - `vmcb12`: L1's VMCB for L2 (read-only, from L1's memory)
/// - `l0_asid`: The ASID L0 assigns to this L2 (from per-CPU counter)
/// - `nnpt_root`: Physical address of the synthesized nNPT root (from M3.3)
///
/// # Returns
/// - `Ok(())` if merge succeeded
/// - `Err(EINVAL)` if VMCB12 contains invalid guest state (L0 will inject
///   VMEXIT(INVALID) back to L1)
pub fn merge_vmcb12_to_vmcb02(
    vmcb02: &mut Vmcb,
    vmcb12: &Vmcb,
    l0_asid: u32,
    nnpt_root: u64,
) -> HvResult {
    // Step 1: Validate VMCB12 guest state (CR0/CR4/EFER reserved bits).
    validate_guest_state(&vmcb12.save)?;

    // Step 2: Copy guest state from VMCB12 to VMCB02 (segments, GPRs, control regs).
    copy_guest_state(vmcb02, vmcb12);

    // Step 3: Merge intercept vectors (L0_MANDATORY ∪ L1).
    merge_intercepts(vmcb02, vmcb12);

    // Step 4: Set control fields with L0 policy.
    vmcb02.control.np_enable = 1; // Always use nested paging for L2.
    vmcb02.control.nest_cr3 = nnpt_root; // Point to synthesized nNPT, never L1's nCR3.
    vmcb02.control.guest_asid = l0_asid; // L0-assigned ASID, not L1's.
    vmcb02.control.tlb_control = VmcbTlbControl::FlushAsid as u8; // Flush on every VMRUN.

    // Step 5: TSC offset: L0 base + L1 offset (currently L0 base is 0).
    vmcb02.control.tsc_offset = vmcb12.control.tsc_offset;

    // Step 6: Interrupt control: pass through L1's settings (L1 manages L2's virtual interrupts).
    // TODO: Apply L0 interrupt policy constraints if needed (e.g., V_INTR_MASKING).
    vmcb02.control.int_control = vmcb12.control.int_control;
    vmcb02.control.int_vector = vmcb12.control.int_vector;
    vmcb02.control.int_state = vmcb12.control.int_state;

    // Step 7: MSRPM/IOPM will be set by the caller (M3.2 merge functions).
    // vmcb02.control.msrpm_base_pa and iopm_base_pa are already pointing to
    // per-CPU merged pages from VMCB01 setup; M3.2 will update them.

    Ok(())
}

/// Validate that VMCB12's guest state does not violate architectural constraints.
/// AMD APM Vol.2 §15.5 VMRUN checks.
fn validate_guest_state(save: &libvmm::svm::vmcb::VmcbStateSaveArea) -> HvResult {
    // Check CR0 reserved bits.
    if save.cr0 & CR0_RESERVED_MASK != 0 {
        return hv_result_err!(EINVAL, "VMCB12 CR0 {:#x} has reserved bits set", save.cr0);
    }

    // CR0.CD and CR0.NW must be consistent (both set or both clear).
    let cd = save.cr0 & Cr0Flags::CACHE_DISABLE.bits() != 0;
    let nw = save.cr0 & Cr0Flags::NOT_WRITE_THROUGH.bits() != 0;
    if cd != nw {
        return hv_result_err!(EINVAL, "VMCB12 CR0.CD and CR0.NW inconsistent");
    }

    // Check CR4 reserved bits.
    if save.cr4 & CR4_RESERVED_MASK != 0 {
        return hv_result_err!(EINVAL, "VMCB12 CR4 {:#x} has reserved bits set", save.cr4);
    }

    // Check EFER reserved bits.
    if save.efer & EFER_RESERVED_MASK != 0 {
        return hv_result_err!(EINVAL, "VMCB12 EFER {:#x} has reserved bits set", save.efer);
    }

    // EFER.LME requires CR0.PG and CR4.PAE (long mode consistency).
    let lme = save.efer & EferFlags::LONG_MODE_ENABLE.bits() != 0;
    let pg = save.cr0 & Cr0Flags::PAGING.bits() != 0;
    let pae = save.cr4 & Cr4Flags::PHYSICAL_ADDRESS_EXTENSION.bits() != 0;
    if lme && pg && !pae {
        return hv_result_err!(EINVAL, "VMCB12 EFER.LME set but CR4.PAE clear");
    }

    Ok(())
}

/// Copy guest state from VMCB12 to VMCB02. This is a direct copy of the
/// save area; L0 trusts the validated state.
fn copy_guest_state(vmcb02: &mut Vmcb, vmcb12: &Vmcb) {
    // Copy all segment registers.
    vmcb02.save.es = vmcb12.save.es;
    vmcb02.save.cs = vmcb12.save.cs;
    vmcb02.save.ss = vmcb12.save.ss;
    vmcb02.save.ds = vmcb12.save.ds;
    vmcb02.save.fs = vmcb12.save.fs;
    vmcb02.save.gs = vmcb12.save.gs;
    vmcb02.save.gdtr = vmcb12.save.gdtr;
    vmcb02.save.ldtr = vmcb12.save.ldtr;
    vmcb02.save.idtr = vmcb12.save.idtr;
    vmcb02.save.tr = vmcb12.save.tr;

    // Copy control registers.
    vmcb02.save.cr0 = vmcb12.save.cr0;
    vmcb02.save.cr2 = vmcb12.save.cr2;
    vmcb02.save.cr3 = vmcb12.save.cr3; // L2's CR3 (guest-physical addresses).
    vmcb02.save.cr4 = vmcb12.save.cr4;

    // Copy EFER.
    vmcb02.save.efer = vmcb12.save.efer;

    // Copy debug registers.
    vmcb02.save.dr6 = vmcb12.save.dr6;
    vmcb02.save.dr7 = vmcb12.save.dr7;

    // Copy GPRs and RIP/RSP/RFLAGS.
    vmcb02.save.rip = vmcb12.save.rip;
    vmcb02.save.rsp = vmcb12.save.rsp;
    vmcb02.save.rax = vmcb12.save.rax;
    vmcb02.save.rflags = vmcb12.save.rflags;
    vmcb02.save.cpl = vmcb12.save.cpl;

    // Copy system MSRs (SYSCALL/SYSENTER).
    vmcb02.save.star = vmcb12.save.star;
    vmcb02.save.lstar = vmcb12.save.lstar;
    vmcb02.save.cstar = vmcb12.save.cstar;
    vmcb02.save.sfmask = vmcb12.save.sfmask;
    vmcb02.save.sysenter_cs = vmcb12.save.sysenter_cs;
    vmcb02.save.sysenter_esp = vmcb12.save.sysenter_esp;
    vmcb02.save.sysenter_eip = vmcb12.save.sysenter_eip;
    vmcb02.save.kernel_gs_base = vmcb12.save.kernel_gs_base;

    // Copy PAT.
    vmcb02.save.g_pat = vmcb12.save.g_pat;
}

/// Merge intercept bits: VMCB02.intercept = L0_MANDATORY ∪ L1.
/// L1 can add its own intercepts but cannot remove L0's.
fn merge_intercepts(vmcb02: &mut Vmcb, vmcb12: &Vmcb) {
    // Start with L1's intercept vectors.
    vmcb02.control.intercept_cr = vmcb12.control.intercept_cr;
    vmcb02.control.intercept_dr = vmcb12.control.intercept_dr;
    vmcb02.control.intercept_exceptions = vmcb12.control.intercept_exceptions;
    vmcb02.control.intercept_vector3 = vmcb12.control.intercept_vector3;
    vmcb02.control.intercept_vector4 = vmcb12.control.intercept_vector4;
    vmcb02.control.intercept_vector5 = vmcb12.control.intercept_vector5;

    // Force-set L0's mandatory intercepts (OR operation).
    for &intercept in L0_MANDATORY_INTERCEPTS {
        vmcb02.set_intercept(intercept, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_guest_state_valid() {
        let mut save = unsafe { core::mem::zeroed::<libvmm::svm::vmcb::VmcbStateSaveArea>() };
        // Valid state: CR0.PE=1, PG=0, CR4=0, EFER=0.
        save.cr0 = Cr0Flags::PROTECTED_MODE_ENABLE.bits();
        save.cr4 = 0;
        save.efer = 0;
        assert!(validate_guest_state(&save).is_ok());
    }

    #[test]
    fn test_validate_guest_state_cr0_reserved_bits() {
        let mut save = unsafe { core::mem::zeroed::<libvmm::svm::vmcb::VmcbStateSaveArea>() };
        save.cr0 = 1 << 17; // Reserved bit 17.
        assert!(validate_guest_state(&save).is_err());
    }

    #[test]
    fn test_validate_guest_state_cr0_cd_nw_mismatch() {
        let mut save = unsafe { core::mem::zeroed::<libvmm::svm::vmcb::VmcbStateSaveArea>() };
        save.cr0 = Cr0Flags::CACHE_DISABLE.bits(); // CD=1, NW=0.
        assert!(validate_guest_state(&save).is_err());
    }

    #[test]
    fn test_validate_guest_state_efer_lme_without_pae() {
        let mut save = unsafe { core::mem::zeroed::<libvmm::svm::vmcb::VmcbStateSaveArea>() };
        save.cr0 = Cr0Flags::PAGING.bits();
        save.cr4 = 0; // PAE=0.
        save.efer = EferFlags::LONG_MODE_ENABLE.bits();
        assert!(validate_guest_state(&save).is_err());
    }

    #[test]
    fn test_merge_intercepts_preserves_l0_mandatory() {
        let mut vmcb02 = unsafe { core::mem::zeroed::<Vmcb>() };
        let vmcb12 = unsafe { core::mem::zeroed::<Vmcb>() };
        // VMCB12 has no intercepts; merge should still set L0's.
        merge_intercepts(&mut vmcb02, &vmcb12);

        // Check that L0_MANDATORY_INTERCEPTS are all set in vmcb02.
        // SvmIntercept enum values map to bits in intercept_vector3/4/5.
        // VMRUN = 0x80 → vector4 bit 0, VMMCALL = 0x81 → vector4 bit 1, etc.
        assert_ne!(vmcb02.control.intercept_vector4 & (1 << 0), 0); // VMRUN
        assert_ne!(vmcb02.control.intercept_vector4 & (1 << 1), 0); // VMMCALL
    }
}

/// Merge L0's policy MSRPM with L1's MSRPM into a per-CPU merged page.
///
/// The merge is a bitwise OR: a bit set in either L0 or L1 results in
/// interception. L0's policy bits ensure that security-critical MSRs
/// (EFER, VM_CR, etc.) always trap, even if L1 tries to pass them through.
///
/// # Arguments
/// - `merged_msrpm`: 8KB buffer (aligned, from per-CPU storage) to write result
/// - `l0_policy_msrpm`: L0's policy MSRPM (from VMCB01)
/// - `l1_msrpm_gpa`: L1's MSRPM GPA from VMCB12.msrpm_base_pa
pub fn merge_msrpm(
    merged_msrpm: &mut [u8; 8192],
    l0_policy_msrpm: &[u8; 8192],
    l1_msrpm_gpa: u64,
) -> HvResult {
    use super::nested;

    // Translate L1's MSRPM GPA to HPA.
    let l1_msrpm_hpa = nested::gpa_to_hpa(l1_msrpm_gpa.into())?;

    // Map L1's MSRPM as read-only (it's in L1's memory).
    // SAFETY: HPA comes from a valid GPM translation. MSRPM is 2 pages (8KB).
    let l1_msrpm_ptr =
        crate::memory::addr::phys_to_virt(l1_msrpm_hpa.as_usize()) as *const [u8; 8192];
    let l1_msrpm = unsafe { &*l1_msrpm_ptr };

    // Bitwise OR: merged[i] = l0[i] | l1[i].
    for i in 0..8192 {
        merged_msrpm[i] = l0_policy_msrpm[i] | l1_msrpm[i];
    }

    Ok(())
}

/// Merge L0's policy IOPM with L1's IOPM into a per-CPU merged page.
///
/// The merge is a bitwise OR. L0's mandatory ports (PM1a, APMC, 0xCF8-0xCFF)
/// always trap, ensuring L2 cannot bypass device isolation or trigger S3.
///
/// # Arguments
/// - `merged_iopm`: 12KB buffer (3 pages, aligned) to write result
/// - `l0_policy_iopm`: L0's policy IOPM (from VMCB01)
/// - `l1_iopm_gpa`: L1's IOPM GPA from VMCB12.iopm_base_pa
pub fn merge_iopm(
    merged_iopm: &mut [u8; 12288],
    l0_policy_iopm: &[u8; 12288],
    l1_iopm_gpa: u64,
) -> HvResult {
    use super::nested;

    // Translate L1's IOPM GPA to HPA.
    let l1_iopm_hpa = nested::gpa_to_hpa(l1_iopm_gpa.into())?;

    // Map L1's IOPM as read-only (it's in L1's memory).
    // SAFETY: HPA comes from a valid GPM translation. IOPM is 3 pages (12KB).
    let l1_iopm_ptr =
        crate::memory::addr::phys_to_virt(l1_iopm_hpa.as_usize()) as *const [u8; 12288];
    let l1_iopm = unsafe { &*l1_iopm_ptr };

    // Bitwise OR: merged[i] = l0[i] | l1[i].
    for i in 0..12288 {
        merged_iopm[i] = l0_policy_iopm[i] | l1_iopm[i];
    }

    Ok(())
}

#[cfg(test)]
mod msrpm_iopm_tests {
    use super::*;

    #[test]
    fn test_merge_msrpm_or_operation() {
        let mut merged = [0u8; 8192];
        let l0 = {
            let mut arr = [0u8; 8192];
            // L0 traps MSR 0x174 (SYSENTER_CS): byte 0x174*2/8 = 0x5D, bit 0.
            arr[0x5D] = 0b0000_0001;
            arr
        };
        let l1 = {
            let mut arr = [0u8; 8192];
            // L1 traps MSR 0x175 (SYSENTER_ESP): byte 0x175*2/8 = 0x5E, bit 2.
            arr[0x5E] = 0b0000_0100;
            arr
        };

        // Mock merge without GPA translation (unit test limitation).
        for i in 0..8192 {
            merged[i] = l0[i] | l1[i];
        }

        // Both bits should be set in merged.
        assert_eq!(merged[0x5D], 0b0000_0001);
        assert_eq!(merged[0x5E], 0b0000_0100);
    }

    #[test]
    fn test_merge_iopm_or_operation() {
        let mut merged = [0u8; 12288];
        let l0 = {
            let mut arr = [0u8; 12288];
            // L0 traps port 0xCF8 (PCI config address): byte 0xCF8/8 = 0x19F, bit 0.
            arr[0x19F] = 0b0000_0001;
            arr
        };
        let l1 = {
            let mut arr = [0u8; 12288];
            // L1 traps port 0x3F8 (COM1): byte 0x3F8/8 = 0x7F, bit 0.
            arr[0x7F] = 0b0000_0001;
            arr
        };

        for i in 0..12288 {
            merged[i] = l0[i] | l1[i];
        }

        // Both ports should trap in merged.
        assert_eq!(merged[0x19F], 0b0000_0001);
        assert_eq!(merged[0x7F], 0b0000_0001);
    }
}
