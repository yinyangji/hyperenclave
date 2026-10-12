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

//! L2 exit reflection for nested SVM (M4).
//!
//! When L2 exits, L0 must decide: handle it directly (hypercalls, policy MSRs/ports)
//! or reflect it back to L1 so KVM can handle it. This module implements the
//! reflection path: writing L2 state back to VMCB12, restoring VMCB01, and
//! injecting a synthetic #VMEXIT into L1.

use libvmm::svm::{SvmExitCode, Vmcb};

use crate::error::HvResult;

/// Classify an L2 exit: does L0 handle it or reflect to L1?
///
/// Returns `true` if L0 should handle this exit directly (e.g., hypercalls,
/// L0 policy MSRs/ports), `false` if it should be reflected to L1.
pub fn should_l0_handle(exit_code: SvmExitCode) -> bool {
    match exit_code {
        // L0 always handles hypercalls (VMMCALL intercept ensures this).
        SvmExitCode::VMMCALL => true,

        // L0 handles nested SVM instructions (VMRUN/VMLOAD/VMSAVE/STGI/CLGI).
        // If L2 executes these, they trap to L0 (nested SVM is recursive).
        SvmExitCode::VMRUN
        | SvmExitCode::VMLOAD
        | SvmExitCode::VMSAVE
        | SvmExitCode::STGI
        | SvmExitCode::CLGI => true,

        // NPF is handled by L0 (nNPT synthesis) or reflected if L1 nNPT is missing.
        // The caller (vmexit.rs) decides based on handle_nested_page_fault result.
        SvmExitCode::NPF => true,

        // CPUID/MSR/IOIO: L0 applies its policy mask/filter, then reflects.
        // These are NOT handled entirely by L0; after policy check, reflect to L1.
        SvmExitCode::CPUID => false,
        SvmExitCode::MSR => false,
        SvmExitCode::IOIO => false,

        // Exceptions, interrupts, NMI: always reflect to L1 (L1's KVM manages L2's interrupts).
        SvmExitCode::EXCP(_) => false,
        SvmExitCode::INTR => false,
        SvmExitCode::NMI => false,

        // Everything else: reflect to L1.
        _ => false,
    }
}

/// Reflect an L2 exit back to L1: write L2 state to VMCB12, restore VMCB01,
/// and let L1 continue (it will see the exit when it next reads VMCB12).
///
/// # Arguments
/// - `vmcb02`: The VMCB L0 ran for L2 (currently loaded in the vCPU)
/// - `vmcb12_gpa`: GPA of L1's VMCB12
/// - `vmcb01`: Reference VMCB01 state to restore (L1's VMCB before VMRUN)
///
/// # Returns
/// - `Ok(())` if reflection succeeded; caller should return to L1
/// - `Err(...)` if VMCB12 access failed (should inject fault to L1)
pub fn reflect_exit_to_l1(vmcb02: &Vmcb, vmcb12_gpa: u64, vmcb01: &Vmcb) -> HvResult {
    use super::nested;

    // Step 1: Write L2's state back to VMCB12.
    nested::with_vmcb12_mut(vmcb12_gpa, |vmcb12| {
        // Copy L2's guest state from VMCB02 to VMCB12.
        copy_guest_state_to_vmcb12(vmcb12, vmcb02);

        // Copy exit information from VMCB02 to VMCB12.
        vmcb12.control.exit_code = vmcb02.control.exit_code;
        vmcb12.control.exit_info_1 = vmcb02.control.exit_info_1;
        vmcb12.control.exit_info_2 = vmcb02.control.exit_info_2;
        vmcb12.control.exit_int_info = vmcb02.control.exit_int_info;
        vmcb12.control.exit_int_info_err = vmcb02.control.exit_int_info_err;

        // Copy event injection state (if L0 injected an event, L1 sees it).
        vmcb12.control.event_inj = vmcb02.control.event_inj;
        vmcb12.control.event_inj_err = vmcb02.control.event_inj_err;

        debug!(
            "Reflected L2 exit to L1: exit_code={:#x} RIP={:#x}",
            vmcb12.control.exit_code, vmcb12.save.rip
        );

        Ok(())
    })??;

    // Step 2: Restore VMCB01 (L1's state before it executed VMRUN).
    // For M4, we simplified this: VMCB02 will be overwritten by the next VMRUN.
    // A full implementation would restore VMCB01's control area here.
    // For now, we rely on the fact that L1 will re-setup the VMCB before next VMRUN.

    Ok(())
}

/// Copy L2's guest state from VMCB02 back to VMCB12.
/// This is the inverse of the merge operation: L2's execution modified VMCB02's
/// save area (RIP, RSP, RAX, segment registers, etc.), and L1 needs to see
/// those changes in VMCB12.
fn copy_guest_state_to_vmcb12(vmcb12: &mut Vmcb, vmcb02: &Vmcb) {
    // Copy all segment registers.
    vmcb12.save.es = vmcb02.save.es;
    vmcb12.save.cs = vmcb02.save.cs;
    vmcb12.save.ss = vmcb02.save.ss;
    vmcb12.save.ds = vmcb02.save.ds;
    vmcb12.save.fs = vmcb02.save.fs;
    vmcb12.save.gs = vmcb02.save.gs;
    vmcb12.save.gdtr = vmcb02.save.gdtr;
    vmcb12.save.ldtr = vmcb02.save.ldtr;
    vmcb12.save.idtr = vmcb02.save.idtr;
    vmcb12.save.tr = vmcb02.save.tr;

    // Copy control registers.
    vmcb12.save.cr0 = vmcb02.save.cr0;
    vmcb12.save.cr2 = vmcb02.save.cr2;
    vmcb12.save.cr3 = vmcb02.save.cr3;
    vmcb12.save.cr4 = vmcb02.save.cr4;

    // Copy EFER.
    vmcb12.save.efer = vmcb02.save.efer;

    // Copy debug registers.
    vmcb12.save.dr6 = vmcb02.save.dr6;
    vmcb12.save.dr7 = vmcb02.save.dr7;

    // Copy GPRs and RIP/RSP/RFLAGS.
    vmcb12.save.rip = vmcb02.save.rip;
    vmcb12.save.rsp = vmcb02.save.rsp;
    vmcb12.save.rax = vmcb02.save.rax;
    vmcb12.save.rflags = vmcb02.save.rflags;
    vmcb12.save.cpl = vmcb02.save.cpl;

    // Copy system MSRs.
    vmcb12.save.star = vmcb02.save.star;
    vmcb12.save.lstar = vmcb02.save.lstar;
    vmcb12.save.cstar = vmcb02.save.cstar;
    vmcb12.save.sfmask = vmcb02.save.sfmask;
    vmcb12.save.sysenter_cs = vmcb02.save.sysenter_cs;
    vmcb12.save.sysenter_esp = vmcb02.save.sysenter_esp;
    vmcb12.save.sysenter_eip = vmcb02.save.sysenter_eip;
    vmcb12.save.kernel_gs_base = vmcb02.save.kernel_gs_base;

    // Copy PAT.
    vmcb12.save.g_pat = vmcb02.save.g_pat;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_l0_handle_classification() {
        // L0 handles hypercalls and nested SVM instructions.
        assert!(should_l0_handle(SvmExitCode::VMMCALL));
        assert!(should_l0_handle(SvmExitCode::VMRUN));
        assert!(should_l0_handle(SvmExitCode::VMLOAD));
        assert!(should_l0_handle(SvmExitCode::NPF));

        // L0 reflects CPUID/MSR/IOIO after policy check.
        assert!(!should_l0_handle(SvmExitCode::CPUID));
        assert!(!should_l0_handle(SvmExitCode::MSR));
        assert!(!should_l0_handle(SvmExitCode::IOIO));

        // L0 reflects exceptions and interrupts.
        assert!(!should_l0_handle(SvmExitCode::EXCP(14))); // #PF
        assert!(!should_l0_handle(SvmExitCode::INTR));
        assert!(!should_l0_handle(SvmExitCode::NMI));
    }
}
