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
//!
//! The legacy PCI configuration ports 0xCF8-0xCFF trap too, so that config
//! cycles aimed at a protected accelerator can be answered as if the device
//! were absent (all-ones reads, dropped writes) while cycles to ordinary
//! devices are executed on the guest's behalf. This costs one VM exit per
//! guest config access; acceptable because config cycles are rare after
//! boot enumeration.

use x86_64::instructions::port::Port;

use super::pci_cfg::decode_config_address;
use crate::arch::vmm::VcpuAccessGuestState;
use crate::error::HvResult;
use crate::percpu::PerCpu;

/// Legacy PCI configuration mechanism ports (PCI Local Bus Spec §3.2.2.3.2).
/// CONFIG_ADDRESS is 0xCF8-0xCFB, the CONFIG_DATA window is 0xCFC-0xCFF.
const CONFIG_ADDRESS_PORT: u16 = 0xcf8;
const CONFIG_DATA_PORT: u16 = 0xcfc;

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
    /// Ports 0xCF8-0xCFF, the legacy PCI configuration mechanism. The
    /// action is decided per access in `handle_pio`, because it depends on
    /// which device the guest's last CONFIG_ADDRESS targets.
    ConfigSpace,
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
    // Legacy PCI config mechanism 0xCF8-0xCFF: trapped so config cycles
    // targeting a protected accelerator can be hidden from the host. Every
    // guest PCI config access now costs a VM exit; acceptable because
    // config cycles are rare after boot enumeration.
    PioEntry {
        port: 0xcf8,
        mask: !0x7,
        action: PioAction::ConfigSpace,
        name: "PCI config",
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

/// Handle a trapped PIO access. `size` is the access width in bytes
/// (1, 2 or 4); `instr_len` is the length of the I/O instruction, needed
/// only when the action completes it (RIP advance).
pub fn handle_pio(cpu: &mut PerCpu, port: u16, is_read: bool, size: u8, instr_len: u8) -> HvResult {
    let entry = pio_lookup(port);
    let action = entry.map_or(PioAction::DenyInjectGP, |e| e.action);
    let name = entry.map_or("unknown port", |e| e.name);

    match action {
        PioAction::ConfigSpace => handle_config_pio(cpu, port, is_read, size, instr_len),
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
            crate::arch::POLICY_STATS.record_pio_deny();
            cpu.vcpu.inject_fault()?;
            // #GP is a fault: RIP stays on the I/O instruction.
            Ok(())
        }
    }
}

/// How a CONFIG_DATA cycle must be served, decided from the stored
/// CONFIG_ADDRESS value alone. Kept pure so the host-side tests can
/// exercise the decision without hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigCycle {
    /// Enable bit clear, or the cycle targets a protected accelerator:
    /// reads return all-ones, writes are dropped.
    Protected,
    /// The cycle targets an unprotected device: perform the real port
    /// access on the guest's behalf.
    Passthrough,
}

fn config_cycle(config_addr: u32) -> ConfigCycle {
    match decode_config_address(config_addr) {
        Some((bdf, _)) if !crate::device::is_protected_device(bdf) => ConfigCycle::Passthrough,
        _ => ConfigCycle::Protected,
    }
}

/// Mask of the low `size` bytes of a value.
const fn size_mask(size: u8) -> u64 {
    match size {
        1 => 0xff,
        2 => 0xffff,
        _ => 0xffff_ffff,
    }
}

/// Replace the low `size` bytes of RAX, preserving the rest: a guest
/// 8/16-bit IN leaves the upper bytes of RAX untouched.
fn merge_rax(rax: u64, value: u64, size: u8) -> u64 {
    (rax & !size_mask(size)) | (value & size_mask(size))
}

/// Merge `size` bytes of `value` into a u32 register image at byte
/// `offset` (sub-dword accesses to 0xCF9-0xCFB).
fn merge_u32(old: u32, value: u64, offset: u64, size: u8) -> u32 {
    let shift = offset * 8;
    let mask = size_mask(size) << shift;
    (((old as u64) & !mask) | ((value << shift) & mask)) as u32
}

/// Emulate one access to the legacy PCI configuration ports. Always
/// completes the instruction (RIP advance); it never faults.
fn handle_config_pio(
    cpu: &mut PerCpu,
    port: u16,
    is_read: bool,
    size: u8,
    instr_len: u8,
) -> HvResult {
    if port < CONFIG_DATA_PORT {
        // 0xCF8-0xCFB: the CONFIG_ADDRESS register. Sub-dword accesses
        // merge into the stored value, as the real register would.
        let offset = (port - CONFIG_ADDRESS_PORT) as u64;
        if is_read {
            let value = ((cpu.pci_cfg_addr as u64) >> (offset * 8)) & size_mask(size);
            let rax = cpu.vcpu.regs().rax;
            cpu.vcpu.regs_mut().rax = merge_rax(rax, value, size);
        } else {
            let value = (cpu.vcpu.regs().rax >> (offset * 8)) & size_mask(size);
            cpu.pci_cfg_addr = merge_u32(cpu.pci_cfg_addr, value, offset, size);
        }
        return cpu.vcpu.advance_rip(instr_len);
    }

    // 0xCFC-0xCFF: the CONFIG_DATA window. The effective config register
    // of the cycle is the decoded offset plus (port - 0xCFC); the protect
    // decision needs only the target BDF.
    match config_cycle(cpu.pci_cfg_addr) {
        ConfigCycle::Protected => {
            // A hidden accelerator answers as absent: all-ones reads,
            // dropped writes.
            if is_read {
                let rax = cpu.vcpu.regs().rax;
                cpu.vcpu.regs_mut().rax = merge_rax(rax, size_mask(size), size);
            }
        }
        ConfigCycle::Passthrough => {
            // SAFETY: the monitor owns these ports (every guest access to
            // them traps here), and this performs exactly the access the
            // guest asked for, against a device the policy leaves to the
            // host. The hardware resolves the register from the real
            // CONFIG_ADDRESS plus the port offset.
            unsafe {
                Port::<u32>::new(CONFIG_ADDRESS_PORT).write(cpu.pci_cfg_addr);
                match size {
                    1 => {
                        let mut p = Port::<u8>::new(port);
                        if is_read {
                            let rax = cpu.vcpu.regs().rax;
                            cpu.vcpu.regs_mut().rax = merge_rax(rax, p.read() as u64, size);
                        } else {
                            p.write(cpu.vcpu.regs().rax as u8);
                        }
                    }
                    2 => {
                        let mut p = Port::<u16>::new(port);
                        if is_read {
                            let rax = cpu.vcpu.regs().rax;
                            cpu.vcpu.regs_mut().rax = merge_rax(rax, p.read() as u64, size);
                        } else {
                            p.write(cpu.vcpu.regs().rax as u16);
                        }
                    }
                    _ => {
                        let mut p = Port::<u32>::new(port);
                        if is_read {
                            let rax = cpu.vcpu.regs().rax;
                            cpu.vcpu.regs_mut().rax = merge_rax(rax, p.read() as u64, size);
                        } else {
                            p.write(cpu.vcpu.regs().rax as u32);
                        }
                    }
                }
            }
        }
    }
    cpu.vcpu.advance_rip(instr_len)
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
        // The legacy PCI config ports 0xCF8-0xCFF trap for per-access
        // handling.
        for port in 0xcf8..=0xcffu16 {
            assert_eq!(pio_lookup(port).unwrap().action, PioAction::ConfigSpace);
        }
        // Neighbours and common kernel ports stay unlisted (passthrough).
        for port in [0x603u16, 0x606, 0xaf, 0xb4, 0x3f8, 0xcf7, 0xd00] {
            assert!(pio_lookup(port).is_none(), "port {:#x} must not trap", port);
        }
    }

    #[test]
    fn test_bitmap_generation_set() {
        let mut ports = Vec::new();
        foreach_trapped_port(|port| ports.push(port));
        // Exactly the policy ranges, nothing else.
        let mut expected: Vec<u16> = alloc::vec![0x604, 0x605, 0xb0, 0xb1, 0xb2, 0xb3];
        expected.extend(0xcf8..=0xcff);
        assert_eq!(ports, expected);
        // Every generated port resolves back to a denying entry.
        for port in ports {
            let entry = pio_lookup(port).unwrap();
            assert_ne!(entry.action, PioAction::Allow);
        }
    }

    #[test]
    fn test_config_cycle_decision() {
        // Enable bit clear: the cycle targets nothing, so it is answered
        // as protected (all-ones / dropped).
        assert_eq!(config_cycle(0x0065_0000), ConfigCycle::Protected);
        assert_eq!(config_cycle(0), ConfigCycle::Protected);
        // Enable bit set: on the host-test path DEVICE_MANAGER is
        // uninitialized, so no device is protected and the verdict is
        // passthrough.
        assert_eq!(config_cycle(0x8065_0000), ConfigCycle::Passthrough);
        assert_eq!(config_cycle(0x8000_fb13), ConfigCycle::Passthrough);
    }

    #[test]
    fn test_merge_helpers() {
        // IN merges only the accessed bytes; the rest of RAX survives.
        assert_eq!(
            merge_rax(0xdead_beef_1234_5678, 0xaa, 1),
            0xdead_beef_1234_56aa
        );
        assert_eq!(
            merge_rax(0xdead_beef_1234_5678, 0xbbcc, 2),
            0xdead_beef_1234_bbcc
        );
        assert_eq!(
            merge_rax(0xdead_beef_1234_5678, 0x0102_0304, 4),
            0xdead_beef_0102_0304
        );
        // Sub-dword CONFIG_ADDRESS writes merge at their byte offset.
        assert_eq!(merge_u32(0x1122_3344, 0xaa, 1, 1), 0x1122_aa44);
        assert_eq!(merge_u32(0x1122_3344, 0xbbcc, 2, 2), 0xbbcc_3344);
        assert_eq!(merge_u32(0x1122_3344, 0xddee_ff00, 0, 4), 0xddee_ff00);
    }
}
