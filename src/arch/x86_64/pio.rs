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

//! PIO policy engine (G6, vendor-independent core).
//!
//! Both vendor exit paths (Intel `IO_INSTRUCTION`, AMD `IOIO`) funnel into
//! `handle_pio`, which dispatches on the shared policy table. The
//! interception bitmaps (Intel I/O bitmaps A/B, AMD IOPM) are generated
//! from the very same table via `foreach_trapped_port`: only the listed
//! ports trap, everything else stays at bare-metal speed — the kernel's
//! PIO is far too frequent for unconditional interception
//! (`UNCOND_IO_EXITING` / a fully-set IOPM would make the system
//! unusable). A port that traps without a denying table entry is refused
//! with #GP — fail-closed defense in depth, same as the MSR path.

use crate::arch::vmm::VcpuAccessGuestState;
use crate::error::HvResult;
use crate::percpu::PerCpu;

/// What the monitor does with a trapped port access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PioAction {
    /// Let the access through. Never traps (the bitmaps only encode
    /// denying entries); kept so the table can express intent.
    Allow,
    /// Inject #GP: the guest sees a fault on the I/O instruction itself
    /// (fault semantics, RIP stays on the instruction).
    DenyInjectGP,
    /// Reads return all-ones (as if no device answered), writes are
    /// dropped; RIP advances past the instruction.
    DenyReturnFF,
}

/// One port range. `mask` clears the low bits of `port`, so an entry
/// covers every port with the same masked value — e.g. `0x604 / !0x1`
/// covers both bytes of the 16-bit PM1a_CNT at 0x604-0x605.
#[derive(Debug, Clone, Copy)]
pub struct PioEntry {
    pub port: u16,
    pub mask: u16,
    pub action: PioAction,
    pub name: &'static str,
}

/// Shared PIO policy table (implementation design §5.1). Ports not
/// listed here pass through untouched.
pub static PIO_POLICY_TABLE: &[PioEntry] = &[
    // ACPI PM1a_CNT (S3/S4 entry): writing SLP_EN suspends the machine
    // behind the monitor's back, and the resume path runs firmware the
    // monitor never measured. 0x604 is the common QEMU/FADT value; the
    // real base should eventually be parsed from the FADT at boot.
    PioEntry {
        port: 0x604,
        mask: !0x1,
        action: PioAction::DenyInjectGP,
        name: "PM1a_CNT",
    },
    // APMC block 0xB0-0xB3: a write to 0xB2 raises an SMI — an
    // out-of-band privilege domain above the monitor, invisible to it.
    // Sealed unconditionally (design §5.3: SMM is never emulated).
    PioEntry {
        port: 0xb2,
        mask: !0x3,
        action: PioAction::DenyInjectGP,
        name: "APMC",
    },
];

/// Look up the table entry covering `port`, if any.
pub fn pio_lookup(port: u16) -> Option<&'static PioEntry> {
    PIO_POLICY_TABLE
        .iter()
        .find(|entry| port & entry.mask == entry.port & entry.mask)
}

/// Invoke `f` once for every port the policy table traps. This is the
/// single source both vendors' bitmaps are generated from, so Intel and
/// AMD intercept exactly the same port set.
pub fn foreach_trapped_port(mut f: impl FnMut(u16)) {
    for entry in PIO_POLICY_TABLE {
        if entry.action == PioAction::Allow {
            continue;
        }
        let base = entry.port & entry.mask;
        for port in base..=(base | !entry.mask) {
            f(port);
        }
    }
}

/// Handle a trapped PIO access. `instr_len` is the length of the I/O
/// instruction, needed only when the action completes it (RIP advance).
pub fn handle_pio(cpu: &mut PerCpu, port: u16, is_read: bool, instr_len: u8) -> HvResult {
    let entry = pio_lookup(port);
    let action = entry.map_or(PioAction::DenyInjectGP, |e| e.action);
    let name = entry.map_or("unknown port", |e| e.name);

    match action {
        PioAction::DenyReturnFF => {
            warn!(
                "PIO {:#06x} ({}) {} denied, reads return all-ones",
                port,
                name,
                if is_read { "read" } else { "write" }
            );
            if is_read {
                // As if no device answered.
                cpu.vcpu.regs_mut().rax = !0;
            }
            // Writes are dropped; either way the instruction completes.
            cpu.vcpu.advance_rip(instr_len)
        }
        // DenyInjectGP, plus the defensive cases that must never happen
        // (Allow / unlisted ports are not trapped by the bitmaps): refuse
        // with #GP rather than perform I/O on the guest's behalf.
        _ => {
            error!(
                "PIO {:#06x} ({}) {} denied by policy",
                port,
                name,
                if is_read { "read" } else { "write" }
            );
            cpu.vcpu.inject_fault()?;
            // #GP is a fault: RIP stays on the I/O instruction.
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn test_lookup() {
        // PM1a_CNT: both bytes of the 16-bit register are covered.
        for port in [0x604u16, 0x605] {
            assert_eq!(pio_lookup(port).unwrap().action, PioAction::DenyInjectGP);
        }
        // APMC block 0xB0-0xB3 (a write to 0xB2 raises an SMI).
        for port in 0xb0..=0xb3u16 {
            assert_eq!(pio_lookup(port).unwrap().action, PioAction::DenyInjectGP);
        }
        // Neighbours and common kernel ports stay unlisted (passthrough).
        for port in [0x603u16, 0x606, 0xaf, 0xb4, 0x3f8, 0xcf8, 0xcfc] {
            assert!(pio_lookup(port).is_none(), "port {:#x} must not trap", port);
        }
    }

    #[test]
    fn test_bitmap_generation_set() {
        let mut ports = Vec::new();
        foreach_trapped_port(|port| ports.push(port));
        // Exactly the two policy ranges, nothing else.
        assert_eq!(ports, [0x604, 0x605, 0xb0, 0xb1, 0xb2, 0xb3]);
        // Every generated port resolves back to a denying entry.
        for port in ports {
            let entry = pio_lookup(port).unwrap();
            assert_ne!(entry.action, PioAction::Allow);
        }
    }
}
