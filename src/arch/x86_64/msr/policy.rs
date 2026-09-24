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

//! MSR virtualization policy table (vendor-independent).
//!
//! The single policy table below drives both hardware interception maps
//! (Intel MSR bitmap / AMD MSRPM) and the common RDMSR/WRMSR handlers.
//! The maps encode exactly the table: `Passthrough` never traps, every
//! other action sets its interception bit. MSRs outside the table keep
//! their bare-metal semantics in the maps (the hardware raises #GP itself
//! for registers that do not exist); should one ever reach the handlers
//! anyway it is denied — fail-closed defense in depth.
//!
//! Actions:
//! - `Passthrough`: no interception bit; the guest talks to the hardware MSR.
//!   Used for MSRs the guest owns as the "original owner" (LAPIC, perf,
//!   microcode, TSC) whose native semantics must not be broken.
//! - `Proxy`: intercepted, the monitor executes the real rdmsr/wrmsr on
//!   behalf of the guest and audits the access.
//! - `Emulate`: intercepted, the value lives in a per-vCPU software mirror
//!   (`VcpuMsrState`); nothing is written to the hardware MSR.
//! - `AreaSwap`: intercepted, the value lives in a hardware-managed guest
//!   structure (Intel: VMCS guest field or MSR load/store area; AMD: VMCB
//!   save area) that the CPU loads on VM entry.
//! - `Deny`: intercepted, #GP is injected (fault-closed default).

/// Per-MSR action decided by the policy table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsrAction {
    /// No interception; direct hardware access.
    Passthrough,
    /// Interception + monitor proxies the real MSR access (audited).
    Proxy,
    /// Interception + access is served from the per-vCPU software mirror.
    Emulate,
    /// Interception + access is served from a hardware guest-state
    /// structure that VM entry loads (VMCS field, MSR area, or VMCB).
    AreaSwap,
    /// Interception + #GP injection (fail-closed).
    Deny,
}

/// One policy table entry, covering the inclusive range `first..=last`.
#[derive(Debug)]
pub struct MsrEntry {
    pub first: u32,
    pub last: u32,
    pub read: MsrAction,
    pub write: MsrAction,
    pub name: &'static str,
}

/// Sorted, non-overlapping static policy table (binary-searched).
///
/// Entries must stay in strictly ascending order; the unit tests below
/// enforce this. MSRs outside both vendor bitmap coverage windows are not
/// listed: they cannot be trapped anyway and the hardware raises #GP for
/// non-existent MSRs, which matches bare-metal semantics.
pub static MSR_POLICY_TABLE: &[MsrEntry] = &[
    // ---- low MSRs (0x0000_0000..=0x0000_1FFF), covered by both vendors ----
    MsrEntry { first: 0x10, last: 0x10, read: MsrAction::Passthrough, write: MsrAction::Passthrough, name: "IA32_TSC" },
    MsrEntry { first: 0x1b, last: 0x1b, read: MsrAction::Passthrough, write: MsrAction::Passthrough, name: "IA32_APIC_BASE" },
    MsrEntry { first: 0x48, last: 0x49, read: MsrAction::Passthrough, write: MsrAction::Passthrough, name: "IA32_SPEC_CTRL/PRED_CMD" },
    MsrEntry { first: 0x79, last: 0x79, read: MsrAction::Passthrough, write: MsrAction::Passthrough, name: "IA32_BIOS_UPDT_TRIG" },
    MsrEntry { first: 0x8b, last: 0x8b, read: MsrAction::Passthrough, write: MsrAction::Deny, name: "IA32_BIOS_SIGN" },
    MsrEntry { first: 0xfe, last: 0xfe, read: MsrAction::Passthrough, write: MsrAction::Deny, name: "IA32_MTRRCAP" },
    MsrEntry { first: 0x174, last: 0x176, read: MsrAction::AreaSwap, write: MsrAction::AreaSwap, name: "IA32_SYSENTER_CS/ESP/EIP" },
    MsrEntry { first: 0x179, last: 0x179, read: MsrAction::Passthrough, write: MsrAction::Deny, name: "IA32_MCG_CAP" },
    MsrEntry { first: 0x17a, last: 0x17b, read: MsrAction::Passthrough, write: MsrAction::Passthrough, name: "IA32_MCG_STATUS/CTL" },
    MsrEntry { first: 0x186, last: 0x18d, read: MsrAction::Passthrough, write: MsrAction::Passthrough, name: "IA32_PERFEVTSEL0-7" },
    MsrEntry { first: 0x197, last: 0x1b0, read: MsrAction::Passthrough, write: MsrAction::Passthrough, name: "IA32_THERM/MISC/MCA zone" },
    MsrEntry { first: 0x1d9, last: 0x1d9, read: MsrAction::Emulate, write: MsrAction::Emulate, name: "IA32_DEBUGCTL" },
    MsrEntry { first: 0x200, last: 0x26f, read: MsrAction::Emulate, write: MsrAction::Emulate, name: "IA32_MTRRphys/FIX" },
    MsrEntry { first: 0x277, last: 0x277, read: MsrAction::AreaSwap, write: MsrAction::AreaSwap, name: "IA32_PAT" },
    MsrEntry { first: 0x2ff, last: 0x2ff, read: MsrAction::Emulate, write: MsrAction::Emulate, name: "IA32_MTRR_DEF_TYPE" },
    MsrEntry { first: 0x309, last: 0x30b, read: MsrAction::Passthrough, write: MsrAction::Passthrough, name: "IA32_FIXED_CTR0-2" },
    MsrEntry { first: 0x345, last: 0x345, read: MsrAction::Passthrough, write: MsrAction::Deny, name: "IA32_PERF_CAPABILITIES" },
    MsrEntry { first: 0x38d, last: 0x38d, read: MsrAction::Passthrough, write: MsrAction::Passthrough, name: "IA32_FIXED_CTR_CTRL" },
    MsrEntry { first: 0x38f, last: 0x390, read: MsrAction::Passthrough, write: MsrAction::Passthrough, name: "IA32_PERF_GLOBAL_CTRL/OVF_CTRL" },
    MsrEntry { first: 0x400, last: 0x47f, read: MsrAction::Passthrough, write: MsrAction::Passthrough, name: "IA32_MCi_CTL/STATUS/ADDR/MISC" },
    MsrEntry { first: 0x480, last: 0x4bf, read: MsrAction::Emulate, write: MsrAction::Deny, name: "IA32_VMX capability zone" },
    MsrEntry { first: 0x4e0, last: 0x4e0, read: MsrAction::Emulate, write: MsrAction::Deny, name: "IA32_SMM_FEATURE_CONTROL" },
    MsrEntry { first: 0x570, last: 0x58f, read: MsrAction::Emulate, write: MsrAction::Deny, name: "IA32_RTIT/PEBS zone" },
    MsrEntry { first: 0x6e0, last: 0x6e0, read: MsrAction::Passthrough, write: MsrAction::Passthrough, name: "IA32_TSC_DEADLINE" },
    MsrEntry { first: 0x802, last: 0x83f, read: MsrAction::Passthrough, write: MsrAction::Passthrough, name: "IA32_X2APIC zone" },
    // ---- high MSRs (0xC000_0000..=0xC000_1FFF), covered by both vendors ----
    MsrEntry { first: 0xc000_0080, last: 0xc000_0080, read: MsrAction::AreaSwap, write: MsrAction::AreaSwap, name: "IA32_EFER" },
    MsrEntry { first: 0xc000_0081, last: 0xc000_0084, read: MsrAction::AreaSwap, write: MsrAction::AreaSwap, name: "IA32_STAR/LSTAR/CSTAR/SFMASK" },
    MsrEntry { first: 0xc000_0100, last: 0xc000_0102, read: MsrAction::AreaSwap, write: MsrAction::AreaSwap, name: "IA32_FS_BASE/GS_BASE/KERNEL_GS_BASE" },
    MsrEntry { first: 0xc000_0103, last: 0xc000_0103, read: MsrAction::Emulate, write: MsrAction::Emulate, name: "IA32_TSC_AUX" },
    // ---- AMD extended MSRs (0xC001_0000..=0xC001_1FFF), MSRPM range 3 only ----
    MsrEntry { first: 0xc001_0015, last: 0xc001_0015, read: MsrAction::Passthrough, write: MsrAction::Passthrough, name: "AMD_HWCR" },
];

/// Look up the policy entry covering `msr`; `None` means "unknown MSR",
/// which the handlers must treat as `Deny` (fail-closed defense in depth).
pub fn policy_lookup(msr: u32) -> Option<&'static MsrEntry> {
    MSR_POLICY_TABLE
        .binary_search_by(|e| {
            use core::cmp::Ordering;
            if msr < e.first {
                Ordering::Greater
            } else if msr > e.last {
                Ordering::Less
            } else {
                Ordering::Equal
            }
        })
        .ok()
        .map(|i| &MSR_POLICY_TABLE[i])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_table_sorted_and_disjoint() {
        for pair in MSR_POLICY_TABLE.windows(2) {
            assert!(pair[0].first <= pair[0].last, "empty range in table");
            assert!(
                pair[0].last < pair[1].first,
                "overlapping or unsorted entries: {:#x?} vs {:#x?}",
                (pair[0].first, pair[0].last),
                (pair[1].first, pair[1].last),
            );
        }
    }

    #[test]
    fn test_lookup_boundaries() {
        // Exact and in-range hits.
        assert_eq!(policy_lookup(0x10).unwrap().name, "IA32_TSC");
        assert_eq!(policy_lookup(0x174).unwrap().name, "IA32_SYSENTER_CS/ESP/EIP");
        assert_eq!(policy_lookup(0x176).unwrap().name, "IA32_SYSENTER_CS/ESP/EIP");
        assert_eq!(policy_lookup(0xc000_0102).unwrap().name, "IA32_FS_BASE/GS_BASE/KERNEL_GS_BASE");
        assert_eq!(policy_lookup(0x830).unwrap().name, "IA32_X2APIC zone");
        // Unknown MSRs.
        assert!(policy_lookup(0x1).is_none());
        assert!(policy_lookup(0x9).is_none());
        assert!(policy_lookup(0x4e1).is_none());
        assert!(policy_lookup(0xdead_beef).is_none());
        assert!(policy_lookup(0x4000_0000).is_none());
        assert!(policy_lookup(0xc001_0200).is_none());
    }

    #[test]
    fn test_action_semantics() {
        // Passthrough: no interception bit, no exit.
        assert_eq!(policy_lookup(0x6e0).unwrap().read, MsrAction::Passthrough);
        assert_eq!(policy_lookup(0x6e0).unwrap().write, MsrAction::Passthrough);
        assert_eq!(policy_lookup(0x808).unwrap().read, MsrAction::Passthrough);
        assert_eq!(policy_lookup(0x80f).unwrap().write, MsrAction::Passthrough);
        assert_eq!(policy_lookup(0xc000_0102).unwrap().write, MsrAction::AreaSwap); // KERNEL_GS_BASE
        assert_eq!(policy_lookup(0xc001_0015).unwrap().write, MsrAction::Passthrough); // AMD HWCR
        // Emulate / AreaSwap: intercepted in both directions.
        assert_eq!(policy_lookup(0x200).unwrap().read, MsrAction::Emulate);
        assert_eq!(policy_lookup(0x200).unwrap().write, MsrAction::Emulate);
        assert_eq!(policy_lookup(0x1d9).unwrap().read, MsrAction::Emulate);
        assert_eq!(policy_lookup(0x277).unwrap().read, MsrAction::AreaSwap);
        assert_eq!(policy_lookup(0x277).unwrap().write, MsrAction::AreaSwap);
        assert_eq!(policy_lookup(0xc000_0080).unwrap().read, MsrAction::AreaSwap);
        assert_eq!(policy_lookup(0xc000_0100).unwrap().write, MsrAction::AreaSwap);
        assert_eq!(policy_lookup(0xc000_0101).unwrap().write, MsrAction::AreaSwap);
        // Hidden capability zones: read emulated as zero, write denied.
        assert_eq!(policy_lookup(0x480).unwrap().read, MsrAction::Emulate);
        assert_eq!(policy_lookup(0x480).unwrap().write, MsrAction::Deny);
        // Read-only platform MSRs: read passthrough, write denied.
        assert_eq!(policy_lookup(0xfe).unwrap().read, MsrAction::Passthrough);
        assert_eq!(policy_lookup(0xfe).unwrap().write, MsrAction::Deny);
        // Unknown MSRs: the handlers treat them as Deny (fail-closed defense
        // in depth); the hardware maps keep bare-metal semantics (no bit).
        assert!(policy_lookup(0x9999).is_none());
        assert!(policy_lookup(0x4e1).is_none());
    }

    #[test]
    fn test_linux_hot_path_msrs_are_covered() {
        // MSRs the 5.x x86_64 kernel touches after activation; each must
        // have an explicit entry so it never falls into the deny hole.
        for msr in [
            0x10, 0x1b, 0x48, 0x79, 0x8b, 0xfe, 0x174, 0x1a0, 0x1d9, 0x200,
            0x277, 0x2ff, 0x400, 0x6e0, 0x802, 0x830, 0xc000_0080,
            0xc000_0081, 0xc000_0100, 0xc000_0101, 0xc000_0102, 0xc000_0103,
        ] {
            assert!(policy_lookup(msr).is_some(), "MSR {:#x} missing from policy table", msr);
        }
    }
}
