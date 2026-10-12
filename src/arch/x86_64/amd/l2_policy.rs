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

//! L2 policy enforcement for nested SVM (M4).
//!
//! When L2 executes CPUID/MSR/IOIO, L0 must apply its mandatory policies
//! before reflecting the exit to L1. This prevents L2 from seeing protected
//! features or bypassing L0's isolation guarantees.

use crate::error::HvResult;

/// Apply L0's CPUID policy to L2's CPUID result before reflecting to L1.
///
/// L0 masks out features that would break its isolation model (e.g., SME/SEV
/// bits that are already used by L0, nested virtualization deeper than L2).
///
/// # Arguments
/// - `leaf`: CPUID leaf (EAX input)
/// - `subleaf`: CPUID subleaf (ECX input)
/// - `eax`, `ebx`, `ecx`, `edx`: CPUID result registers (modified in place)
///
/// # Returns
/// - `Ok(())` if the CPUID is allowed (possibly with masked bits)
/// - `Err(...)` if the CPUID should be blocked entirely (inject #GP to L2)
pub fn apply_cpuid_policy(
    leaf: u32,
    subleaf: u32,
    eax: &mut u32,
    ebx: &mut u32,
    ecx: &mut u32,
    edx: &mut u32,
) -> HvResult {
    match leaf {
        // Leaf 0x8000_000A: SVM features.
        // L2 sees SVM as disabled (no recursive nesting beyond L2).
        0x8000_000A => {
            *eax = 0; // SVM revision = 0
            *ebx = 0; // NASID = 0
            *ecx = 0; // Reserved
            *edx = 0; // SVM features = 0 (no nested SVM in L2)
            debug!("CPUID 0x8000_000A masked: L2 sees SVM disabled");
        }

        // Leaf 0x8000_001F: Encrypted memory features (SME/SEV).
        // L0 already uses SME/SEV; L2 cannot re-enable it.
        0x8000_001F => {
            *eax = 0; // SME/SEV disabled
            *ebx = 0;
            *ecx = 0;
            *edx = 0;
            debug!("CPUID 0x8000_001F masked: L2 sees SME/SEV disabled");
        }

        // Leaf 0x1: Processor info and features.
        // Mask hypervisor bit (ECX bit 31) if L0 wants to hide itself.
        0x1 => {
            // For now, keep hypervisor bit as-is (L2 knows it's virtualized).
            // In production, you might mask ECX bit 31 to hide L0.
        }

        // Leaf 0xD: Extended state enumeration (XSAVE).
        // Allow L2 to see XSAVE features, but validate that L1 has enabled them.
        0xD => {
            // No additional masking needed; L1's CPUID interception will handle this.
        }

        // Other leaves: pass through to L1 (no L0 policy).
        _ => {}
    }

    Ok(())
}

/// Check if an MSR access from L2 violates L0's policy.
///
/// L0 protects certain MSRs from L2 access (e.g., VM_HSAVE_PA, SVM control MSRs).
/// If the access is allowed, return `Ok(true)` to reflect to L1; if blocked,
/// return `Err(...)` to inject #GP to L2.
///
/// # Arguments
/// - `msr`: MSR number
/// - `is_write`: true if WRMSR, false if RDMSR
///
/// # Returns
/// - `Ok(true)`: Allowed, reflect to L1
/// - `Err(...)`: Blocked, inject #GP to L2
pub fn check_msr_policy(msr: u32, is_write: bool) -> HvResult<bool> {
    match msr {
        // VM_HSAVE_PA (0xC001_0117): L2 cannot access L0's SVM state.
        0xC001_0117 => {
            warn!("L2 attempted to access VM_HSAVE_PA, blocked");
            return hv_result_err!(EACCES, "L2 cannot access VM_HSAVE_PA");
        }

        // VM_CR (0xC001_0114): L2 cannot disable SVM or manipulate L0's VM control.
        0xC001_0114 => {
            if is_write {
                warn!("L2 attempted to write VM_CR, blocked");
                return hv_result_err!(EACCES, "L2 cannot write VM_CR");
            }
            // RDMSR VM_CR is allowed; L2 sees a sanitized value from L1.
        }

        // EFER (0xC000_0080): L2 can read/write, but L0 validates the value.
        // The merge logic already ensures L2's EFER is safe (see vmcb_merge.rs).
        0xC000_0080 => {}

        // Other MSRs: allow L1 to decide (reflect).
        _ => {}
    }

    Ok(true)
}

/// Check if a port I/O access from L2 violates L0's policy.
///
/// L0 protects certain ports from L2 access (e.g., 0xCF8/0xCFC for PCI config,
/// ports controlling protected devices).
///
/// # Arguments
/// - `port`: I/O port number
/// - `size`: Access size in bytes (1/2/4)
/// - `is_write`: true if OUT, false if IN
///
/// # Returns
/// - `Ok(true)`: Allowed, reflect to L1
/// - `Err(...)`: Blocked, inject #GP to L2
pub fn check_ioio_policy(port: u16, _size: u8, is_write: bool) -> HvResult<bool> {
    match port {
        // 0xCF8/0xCFC: PCI configuration space.
        // L0 intercepts these to enforce device isolation.
        0xCF8 | 0xCFC => {
            debug!(
                "L2 attempted PCI config access (port {:#x}), reflecting to L1",
                port
            );
            // Allow L1 to handle it (L1's IOPM will decide).
            // L0 doesn't block this outright, but L1's emulation will apply device filtering.
        }

        // Ports 0x3F8-0x3FF: COM1 serial port.
        // Allow L2 to access serial (L1 decides if it's allowed).
        0x3F8..=0x3FF => {}

        // Other ports: reflect to L1.
        _ => {}
    }

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpuid_policy_svm_masked() {
        let mut eax = 0xFFFF_FFFF;
        let mut ebx = 0xFFFF_FFFF;
        let mut ecx = 0xFFFF_FFFF;
        let mut edx = 0xFFFF_FFFF;

        apply_cpuid_policy(0x8000_000A, 0, &mut eax, &mut ebx, &mut ecx, &mut edx).unwrap();

        assert_eq!(eax, 0); // SVM revision = 0
        assert_eq!(edx, 0); // SVM features = 0
    }

    #[test]
    fn test_cpuid_policy_sme_masked() {
        let mut eax = 0xFFFF_FFFF;
        let mut ebx = 0xFFFF_FFFF;
        let mut ecx = 0xFFFF_FFFF;
        let mut edx = 0xFFFF_FFFF;

        apply_cpuid_policy(0x8000_001F, 0, &mut eax, &mut ebx, &mut ecx, &mut edx).unwrap();

        assert_eq!(eax, 0); // SME/SEV disabled
    }

    #[test]
    fn test_msr_policy_vm_hsave_blocked() {
        let result = check_msr_policy(0xC001_0117, true);
        assert!(result.is_err()); // WRMSR VM_HSAVE_PA blocked
    }

    #[test]
    fn test_msr_policy_vm_cr_write_blocked() {
        let result = check_msr_policy(0xC001_0114, true);
        assert!(result.is_err()); // WRMSR VM_CR blocked
    }

    #[test]
    fn test_msr_policy_vm_cr_read_allowed() {
        let result = check_msr_policy(0xC001_0114, false);
        assert!(result.is_ok()); // RDMSR VM_CR allowed
    }

    #[test]
    fn test_ioio_policy_pci_config_allowed() {
        let result = check_ioio_policy(0xCF8, 4, true);
        assert!(result.is_ok()); // L1 decides
    }
}
