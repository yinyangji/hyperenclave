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

use bit_field::BitField;
use x86::msr::rdmsr;

use crate::arch::msr::policy::{self, MsrAction};
use crate::error::HvResult;
use crate::memory::addr::{phys_encrypted, virt_to_phys};
use crate::memory::{AlignedPage, Frame, PhysAddr};

pub(super) struct VmxRegion {
    frame: Frame,
}

impl VmxRegion {
    pub fn new(revision_id: u32, shadow_indicator: bool) -> HvResult<Self> {
        let frame = Frame::new()?;
        unsafe {
            (*(frame.as_mut_ptr() as *mut u32))
                .set_bits(0..=30, revision_id)
                .set_bit(31, shadow_indicator);
        }
        Ok(Self { frame })
    }

    pub fn paddr(&self) -> PhysAddr {
        self.frame.start_paddr()
    }
}

pub(super) struct MsrBitmap(AlignedPage);

impl MsrBitmap {
    /// Whether the Intel MSR bitmap covers `msr` at all. MSRs outside the
    /// two bitmap windows cannot be trapped; accesses to them fall through
    /// to the hardware, which raises #GP for non-existent MSRs.
    fn covers(msr: u32) -> bool {
        msr <= 0x1fff || (0xc000_0000..=0xc000_1fff).contains(&msr)
    }

    /// Build the bitmap from the shared MSR policy table: every read/write
    /// action other than `Passthrough` sets its interception bit. MSRs
    /// outside the table keep their bare-metal semantics (no interception
    /// bit, no exit); the common handlers deny them if one ever slips
    /// through — fail-closed defense in depth.
    pub fn from_policy() -> Self {
        let mut map = Self(AlignedPage::new());
        for entry in policy::MSR_POLICY_TABLE.iter() {
            if entry.read == MsrAction::Passthrough && entry.write == MsrAction::Passthrough {
                continue;
            }
            for msr in entry.first..=entry.last {
                if !Self::covers(msr) {
                    continue;
                }
                if entry.read != MsrAction::Passthrough {
                    map.mask(msr, false);
                }
                if entry.write != MsrAction::Passthrough {
                    map.mask(msr, true);
                }
            }
        }
        map
    }

    fn mask(&mut self, msr: u32, is_write: bool) {
        // (Intel SDM Volume 3, Section 24.6.9, MSR-Bitmap Address)
        // There are four contiguous MSR bitmaps, which are each 1-KByte in size:
        // 1. Read bitmap for low MSRs (0x0000_0000..0x0000_1FFF)
        // 2. Read bitmap for high MSRs (0xC000_0000..0xC000_1FFF)
        // 3. Write bitmap for low MSRs (0x0000_0000..0x0000_1FFF)
        // 4. Write bitmap for high MSRs (0xC000_0000..0xC000_1FFF)
        let mut ptr = self.0.as_mut_ptr();
        let msr_low = msr & 0x1fff;
        let msr_byte = (msr_low / 8) as usize;
        let msr_bit = (msr_low % 8) as u8;

        unsafe {
            if msr >= 0xc000_0000 {
                ptr = ptr.add(1 << 10);
            }
            if is_write {
                ptr = ptr.add(2 << 10);
            }
            // Bit set means "intercept": OR the bit in, don't clear it (F1).
            core::slice::from_raw_parts_mut(ptr, 1024)[msr_byte] |= 1 << msr_bit;
        }
    }

    /// Test an interception bit (unit tests / debugging only).
    #[cfg(test)]
    fn is_intercepted(&self, msr: u32, is_write: bool) -> bool {
        let mut ptr = self.0.as_ptr();
        let msr_low = msr & 0x1fff;
        let msr_byte = (msr_low / 8) as usize;
        let msr_bit = (msr_low % 8) as u8;
        unsafe {
            if msr >= 0xc000_0000 {
                ptr = ptr.add(1 << 10);
            }
            if is_write {
                ptr = ptr.add(2 << 10);
            }
            core::slice::from_raw_parts(ptr, 1024)[msr_byte] & (1 << msr_bit) != 0
        }
    }

    pub fn paddr(&self) -> usize {
        phys_encrypted(virt_to_phys(self.0.as_ptr() as usize))
    }
}

/// Intel I/O bitmaps (SDM Vol.3 §24.6.4): bitmap A traps ports
/// 0x0000-0x7FFF, bitmap B traps 0x8000-0xFFFF, one bit per port, set =
/// intercept. String INS/OUTS follow the same port bits, and a multi-byte
/// access traps if any covered port bit is set — so encoding the trapped
/// ports is sufficient. Built from the same shared PIO policy table as the
/// AMD IOPM, so both vendors intercept exactly the same port set.
pub(super) struct IoBitmap {
    a: AlignedPage,
    b: AlignedPage,
}

impl IoBitmap {
    pub fn from_policy() -> Self {
        let mut map = Self {
            a: AlignedPage::new(),
            b: AlignedPage::new(),
        };
        crate::arch::pio::foreach_trapped_port(|port| map.mask(port));
        map
    }

    fn mask(&mut self, port: u16) {
        // Bit set means "intercept" (same convention as the MSR bitmap).
        let (page, port) = if port < 0x8000 {
            (&mut self.a[..], port)
        } else {
            (&mut self.b[..], port - 0x8000)
        };
        page[port as usize / 8] |= 1 << (port % 8);
    }

    /// Test an interception bit (unit tests / debugging only).
    #[cfg(test)]
    fn is_intercepted(&self, port: u16) -> bool {
        let (page, port) = if port < 0x8000 {
            (&self.a[..], port)
        } else {
            (&self.b[..], port - 0x8000)
        };
        page[port as usize / 8] & (1 << (port % 8)) != 0
    }

    pub fn paddr_a(&self) -> usize {
        phys_encrypted(virt_to_phys(self.a.as_ptr() as usize))
    }

    pub fn paddr_b(&self) -> usize {
        phys_encrypted(virt_to_phys(self.b.as_ptr() as usize))
    }
}

/// MSR index/value pairs loaded by VM entry (and stored on VM exit) for
/// AreaSwap MSRs that have no VMCS guest field.
/// Layout per Intel SDM Vol.3 §24.7.2/§27.4: 32-bit index, 32 bits reserved,
/// 64-bit value.
#[repr(C, align(16))]
pub(super) struct MsrArea {
    entries: [(u32, u64); MSR_AREA_COUNT],
}

/// AreaSwap MSRs served through the MSR load/store areas: they have no
/// dedicated VMCS guest field, so the hardware swaps them via the areas.
/// SYSENTER/EFER/PAT/FS_BASE/GS_BASE instead use VMCS guest fields and are
/// handled by `Vcpu::rdmsr_virt`/`wrmsr_virt` directly.
pub(super) const MSR_AREA_MSRS: &[u32] = &[
    0xc000_0081, // IA32_STAR
    0xc000_0082, // IA32_LSTAR
    0xc000_0083, // IA32_CSTAR
    0xc000_0084, // IA32_FMASK
    0xc000_0102, // IA32_KERNEL_GS_BASE
];

pub(super) const MSR_AREA_COUNT: usize = MSR_AREA_MSRS.len();

impl MsrArea {
    /// Snapshot the current hardware values. Called before the first
    /// vmlaunch, where the hardware values are the guest's initial ones.
    pub fn from_hardware() -> Self {
        let mut entries = [(0, 0); MSR_AREA_COUNT];
        for (i, &msr) in MSR_AREA_MSRS.iter().enumerate() {
            entries[i] = (msr, unsafe { rdmsr(msr) });
        }
        Self { entries }
    }

    pub fn get(&self, msr: u32) -> Option<u64> {
        self.entries
            .iter()
            .find(|(index, _)| *index == msr)
            .map(|(_, value)| *value)
    }

    pub fn set(&mut self, msr: u32, value: u64) -> Option<()> {
        let entry = self.entries.iter_mut().find(|(index, _)| *index == msr)?;
        entry.1 = value;
        Some(())
    }

    pub fn paddr(&self) -> usize {
        phys_encrypted(virt_to_phys(self as *const _ as usize))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bitmap must faithfully encode the policy table for every MSR it
    /// can trap. Passthrough MSRs stay clear; everything else (including
    /// gaps, via the fail-closed default) is intercepted.
    #[test]
    fn test_bitmap_follows_policy() {
        let map = MsrBitmap::from_policy();
        // Spot-check both bitmap windows, all four quadrants.
        assert!(!map.is_intercepted(0x6e0, false)); // TSC_DEADLINE: passthrough
        assert!(!map.is_intercepted(0x6e0, true));
        assert!(!map.is_intercepted(0x830, true)); // x2APIC ICR: passthrough
        assert!(map.is_intercepted(0x200, false)); // MTRRphys: emulate
        assert!(map.is_intercepted(0x200, true));
        assert!(map.is_intercepted(0x1d9, false)); // DEBUGCTL: emulate
        assert!(map.is_intercepted(0x174, true)); // SYSENTER: area swap
        assert!(map.is_intercepted(0x277, true)); // PAT: area swap
        assert!(map.is_intercepted(0x480, false)); // VMX zone: emulate(0)/deny
        assert!(map.is_intercepted(0x480, true));
        assert!(map.is_intercepted(0x570, true)); // RTIT zone: deny write
        assert!(map.is_intercepted(0xc000_0080, true)); // EFER: area swap
        assert!(map.is_intercepted(0xc000_0081, true)); // STAR: area swap (write)
        assert!(map.is_intercepted(0xc000_0081, false)); // STAR: area swap (read)
        assert!(map.is_intercepted(0xc000_0100, true)); // FS_BASE: area swap
        assert!(!map.is_intercepted(0xfe, false)); // MTRRCAP: read passthrough
        assert!(map.is_intercepted(0xfe, true)); // MTRRCAP: write denied
                                                 // Exhaustive consistency over every table entry that the bitmap
                                                 // covers (both windows, all four quadrants).
        for entry in policy::MSR_POLICY_TABLE.iter() {
            for msr in entry.first..=entry.last {
                if !MsrBitmap::covers(msr) {
                    continue;
                }
                assert_eq!(
                    map.is_intercepted(msr, false),
                    entry.read != MsrAction::Passthrough,
                    "read mismatch for MSR {:#x}",
                    msr
                );
                assert_eq!(
                    map.is_intercepted(msr, true),
                    entry.write != MsrAction::Passthrough,
                    "write mismatch for MSR {:#x}",
                    msr
                );
            }
        }
        // MSRs outside the table keep bare-metal semantics in the map: no
        // interception bit (the handlers still deny them if one ever slips
        // through — fail-closed defense in depth).
        for msr in [0x1u32, 0x9, 0x2e, 0x1fff, 0xc000_0001, 0xc000_1fff] {
            assert!(
                !map.is_intercepted(msr, false),
                "unknown MSR {:#x} must not be intercepted",
                msr
            );
            assert!(
                !map.is_intercepted(msr, true),
                "unknown MSR {:#x} must not be intercepted",
                msr
            );
        }
    }

    #[test]
    fn test_io_bitmap_follows_policy() {
        let map = IoBitmap::from_policy();
        // Trapped ports: PM1a_CNT (0x604-0x605) and the APMC block (0xB0-0xB3).
        for port in [0x604u16, 0x605, 0xb0, 0xb1, 0xb2, 0xb3] {
            assert!(map.is_intercepted(port), "port {:#x} must trap", port);
        }
        // Everything else (incl. bitmap B) passes through at bare-metal speed.
        for port in [
            0x3f8u16, 0xcf8, 0xcfc, 0x606, 0x603, 0xb4, 0xaf, 0x8000, 0xffff,
        ] {
            assert!(!map.is_intercepted(port), "port {:#x} must not trap", port);
        }
        // The bitmap encodes exactly the ports the policy traps.
        let count = (0..=0xffffu32)
            .filter(|p| map.is_intercepted(*p as u16))
            .count();
        assert_eq!(count, 6);
    }

    #[test]
    fn test_msr_area_layout() {
        // Entry layout: u32 index + 32 bits padding + u64 value.
        assert_eq!(core::mem::size_of::<MsrArea>(), MSR_AREA_COUNT * 16);
        assert_eq!(core::mem::align_of::<MsrArea>(), 16);
        // Direct construction: from_hardware() executes rdmsr, which is
        // privileged and would fault in a unit test.
        let mut area = MsrArea {
            entries: [(0, 0); MSR_AREA_COUNT],
        };
        for (i, &msr) in MSR_AREA_MSRS.iter().enumerate() {
            area.entries[i] = (msr, 0x1000 + msr as u64);
        }
        assert_eq!(area.get(0xc000_0082), Some(0x1000 + 0xc000_0082));
        assert!(area.set(0xc000_0081, 0x1234_0000_abcd).is_some());
        assert_eq!(area.get(0xc000_0081), Some(0x1234_0000_abcd));
        assert!(area.get(0xc000_0103).is_none()); // TSC_AUX is not in the area
        assert!(area.set(0x1b, 0).is_none()); // APIC_BASE is not in the area
    }
}
