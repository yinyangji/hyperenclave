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

//! Fail-closed policy diagnostics (implementation design §10, GetPolicyStats).
//!
//! Every MSR/PIO access the monitor refuses under its fail-closed policy
//! bumps a counter here. The `GetPolicyStats` hypercall exports them so the
//! driver can surface "something the guest attempted that the monitor
//! denied" without the monitor keeping an unbounded log. Counters are
//! process-wide atomics: they are a coarse diagnostic, not a per-vCPU
//! accounting, and never gate a security decision (the deny already
//! happened at the handler).

use core::sync::atomic::{AtomicU64, Ordering};

/// Counters of policy-denied accesses, one per fail-closed path.
pub struct PolicyStats {
    /// MSR reads/writes refused with #GP (`msr::handle_rdmsr/wrmsr`).
    pub msr_denies: AtomicU64,
    /// Port I/O refused with #GP (`pio::handle_pio`).
    pub pio_denies: AtomicU64,
}

/// Global instance. No `LateInit` needed: atomics initialize to zero and are
/// valid straight from BSS, before any VM exit can reach a handler.
pub static POLICY_STATS: PolicyStats = PolicyStats {
    msr_denies: AtomicU64::new(0),
    pio_denies: AtomicU64::new(0),
};

/// Snapshot exported across the guest boundary by `GetPolicyStats`. Plain
/// `u64`s in a `#[repr(C)]` struct so the layout is stable for the driver.
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct PolicyStatsSnapshot {
    pub msr_denies: u64,
    pub pio_denies: u64,
}

impl PolicyStats {
    /// Record one denied MSR access.
    pub fn record_msr_deny(&self) {
        self.msr_denies.fetch_add(1, Ordering::Relaxed);
    }

    /// Record one denied port I/O access.
    pub fn record_pio_deny(&self) {
        self.pio_denies.fetch_add(1, Ordering::Relaxed);
    }

    /// Read a consistent-enough snapshot (each field loaded independently;
    /// the counters are diagnostic, so a torn read across fields is fine).
    pub fn snapshot(&self) -> PolicyStatsSnapshot {
        PolicyStatsSnapshot {
            msr_denies: self.msr_denies.load(Ordering::Relaxed),
            pio_denies: self.pio_denies.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The counters must advance monotonically. Assertions are relative to a
    /// baseline snapshot because `POLICY_STATS` is a process-wide global and
    /// other tests (or a prior run in the same binary) may have bumped it.
    #[test]
    fn test_policy_stats_counters() {
        let base = POLICY_STATS.snapshot();
        POLICY_STATS.record_msr_deny();
        POLICY_STATS.record_msr_deny();
        POLICY_STATS.record_pio_deny();
        let snap = POLICY_STATS.snapshot();
        assert_eq!(snap.msr_denies, base.msr_denies + 2);
        assert_eq!(snap.pio_denies, base.pio_denies + 1);
    }

    /// The exported snapshot layout must stay a plain two-word C struct so
    /// the driver's view matches byte for byte.
    #[test]
    fn test_snapshot_layout() {
        assert_eq!(core::mem::size_of::<PolicyStatsSnapshot>(), 16);
        assert_eq!(core::mem::align_of::<PolicyStatsSnapshot>(), 8);
    }
}
