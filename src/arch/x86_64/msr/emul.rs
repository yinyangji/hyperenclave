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

//! Per-vCPU software mirror for `Emulate`-class MSRs.
//!
//! The monitor is activated from a running Linux kernel, so the "guest" is
//! the original owner of the machine. `VcpuMsrState::new()` therefore
//! snapshots the real hardware values at activation time and the mirror
//! keeps serving them until the guest writes something else.
//!
//! Hidden capability zones (VMX capability MSRs, RTIT/PEBS, SMM control)
//! always read as zero, which stays consistent with the CPUID hiding done
//! by the CPUID policy engine.

use alloc::collections::BTreeMap;

use x86::msr::{rdmsr, wrmsr};

use crate::arch::cpuid::CpuFeatures;
use crate::error::HvResult;

/// IA32_MTRRCAP (0xFE), read-only.
const MSR_MTRRCAP: u32 = 0xfe;
/// IA32_MTRR_DEF_TYPE (0x2FF).
const MSR_MTRR_DEF_TYPE: u32 = 0x2ff;
/// IA32_MTRRphysBase0 (0x200); mask registers directly follow each base.
const MSR_MTRRPHYS_BASE0: u32 = 0x200;
/// Architectural upper bound of IA32_MTRRcap.VCNT.
pub const MTRR_VCNT_MAX: usize = 32;
/// IA32_DEBUGCTL (0x1D9).
const MSR_DEBUGCTL: u32 = 0x1d9;
/// IA32_TSC_AUX (0xC0000103).
const MSR_TSC_AUX: u32 = 0xc000_0103;

/// Vendor-independent mirror of the `Emulate`-class guest MSRs.
#[derive(Debug)]
pub struct VcpuMsrState {
    /// IA32_TSC_AUX mirror; writes are also forwarded to the hardware so
    /// that untrapped RDTSCP keeps observing the guest's value.
    tsc_aux: u64,
    /// IA32_DEBUGCTL mirror. Never forwarded: LBR/BTS stay off, which
    /// closes an enclave side channel.
    debugctl: u64,
    /// IA32_MTRR_DEF_TYPE mirror (activation snapshot).
    mtrr_def_type: u64,
    /// MTRRphys (base, mask) pairs, indices 0..VCNT hold the hardware
    /// snapshot taken at activation.
    mtrr_phys: [(u64, u64); MTRR_VCNT_MAX],
    /// Write-through fallback for the remaining Emulate-class MSRs
    /// (MTRRfix range, future additions).
    backup: BTreeMap<u32, u64>,
}

impl VcpuMsrState {
    /// Snapshot the real hardware state. Runs on the CPU that is about to
    /// activate the hypervisor, before the first VM entry.
    pub fn new() -> Self {
        // Read MTRRphys pairs strictly up to the enumerated VCNT: reading
        // a non-existent MTRRphys register raises #GP.
        let vcnt = ((unsafe { rdmsr(MSR_MTRRCAP) } & 0xff) as usize).min(MTRR_VCNT_MAX);
        let mut mtrr_phys = [(0, 0); MTRR_VCNT_MAX];
        for i in 0..vcnt {
            mtrr_phys[i] = (
                unsafe { rdmsr(MSR_MTRRPHYS_BASE0 + 2 * i as u32) },
                unsafe { rdmsr(MSR_MTRRPHYS_BASE0 + 2 * i as u32 + 1) },
            );
        }

        Self {
            tsc_aux: if CpuFeatures::new().has_rdtscp() {
                unsafe { rdmsr(MSR_TSC_AUX) }
            } else {
                0
            },
            debugctl: unsafe { rdmsr(MSR_DEBUGCTL) },
            mtrr_def_type: unsafe { rdmsr(MSR_MTRR_DEF_TYPE) },
            mtrr_phys,
            backup: BTreeMap::new(),
        }
    }

    /// Serve an emulated RDMSR. MSRs without a named field fall back to the
    /// write-through backup map; hidden capability zones always read zero.
    pub fn read(&self, msr: u32) -> u64 {
        match msr {
            MSR_TSC_AUX => self.tsc_aux,
            MSR_DEBUGCTL => self.debugctl,
            MSR_MTRR_DEF_TYPE => self.mtrr_def_type,
            MSR_MTRRPHYS_BASE0..=0x23f => {
                let idx = ((msr - MSR_MTRRPHYS_BASE0) / 2) as usize;
                let (base, mask) = self.mtrr_phys[idx];
                if (msr - MSR_MTRRPHYS_BASE0) % 2 == 0 {
                    base
                } else {
                    mask
                }
            }
            // Hidden capability zones: consistent with CPUID hiding.
            0x480..=0x4bf | 0x4e0 | 0x570..=0x58f => 0,
            _ => self.backup.get(&msr).copied().unwrap_or(0),
        }
    }

    /// Serve an emulated WRMSR. Returns an error only for programming
    /// errors (an entry marked Deny reaching this path).
    pub fn write(&mut self, msr: u32, value: u64) -> HvResult {
        match msr {
            MSR_TSC_AUX => {
                self.tsc_aux = value;
                // Keep the hardware in sync so RDTSCP (not intercepted)
                // returns the guest's value.
                unsafe { wrmsr(MSR_TSC_AUX, value) };
            }
            MSR_DEBUGCTL => self.debugctl = value,
            MSR_MTRR_DEF_TYPE => self.mtrr_def_type = value,
            MSR_MTRRPHYS_BASE0..=0x23f => {
                let idx = ((msr - MSR_MTRRPHYS_BASE0) / 2) as usize;
                if (msr - MSR_MTRRPHYS_BASE0) % 2 == 0 {
                    self.mtrr_phys[idx].0 = value;
                } else {
                    self.mtrr_phys[idx].1 = value;
                }
            }
            // MTRRfix range and any future Emulate additions: write-through.
            0x250..=0x26f => {
                self.backup.insert(msr, value);
            }
            // These zones are write-denied by the policy table; reaching
            // this arm means the table and this mirror are out of sync.
            0x480..=0x4bf | 0x4e0 | 0x570..=0x58f => {
                return hv_result_err!(EPERM, format!("MSR {:#x} is write-denied", msr));
            }
            _ => {
                self.backup.insert(msr, value);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mirror must compile and behave without touching hardware for
    /// read/write of mirrored values (construction needs a real CPU and is
    /// therefore not exercised here).
    #[test]
    fn test_emulated_msr_rw() {
        let mut state = VcpuMsrState {
            tsc_aux: 0,
            debugctl: 0,
            mtrr_def_type: 0xc00,
            mtrr_phys: [(0x6, 0x7f); MTRR_VCNT_MAX],
            backup: BTreeMap::new(),
        };

        // MTRRphys base/mask mirror.
        assert_eq!(state.read(0x200), 0x6);
        assert_eq!(state.read(0x201), 0x7f);
        state.write(0x200, 0x1234).unwrap();
        assert_eq!(state.read(0x200), 0x1234);
        assert_eq!(state.read(0x201), 0x7f);
        // DEF_TYPE mirror.
        assert_eq!(state.read(0x2ff), 0xc00);
        // Hidden capability zones read zero.
        assert_eq!(state.read(0x480), 0);
        assert_eq!(state.read(0x570), 0);
        assert_eq!(state.read(0x4e0), 0);
        // Write-denied zones are rejected here as well (defense in depth).
        assert!(state.write(0x480, 1).is_err());
        assert!(state.write(0x4e0, 1).is_err());
        assert!(state.write(0x570, 1).is_err());
        // MTRRfix fallback: write-through and read-back.
        state.write(0x250, 0xabcd).unwrap();
        assert_eq!(state.read(0x250), 0xabcd);
    }
}
