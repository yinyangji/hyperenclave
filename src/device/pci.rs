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

//! PCI configuration-space decoding, independent of how config space is
//! reached.
//!
//! The monitor needs its own enumerator because it has no ACPI parser and
//! cannot ask the host: by the time Linux has enumerated the bus it would
//! already have bound a driver to the accelerator. Config access itself is
//! abstracted behind [`ConfigAccess`] so an ARM backend can supply an ECAM
//! window from a device tree without touching the decoding below.

use alloc::vec::Vec;

use super::bdf::Bdf;
use super::ident::DeviceIdent;
use crate::memory::HostPhysAddr;

/// Configuration-header offsets (PCI Local Bus Spec 3.0 §6.1).
pub mod offset {
    pub const VENDOR_ID: u16 = 0x00;
    pub const DEVICE_ID: u16 = 0x02;
    pub const CLASS_REVISION: u16 = 0x08;
    pub const HEADER_TYPE: u16 = 0x0e;
    /// First of the six 32-bit base address registers of a type-0 header.
    pub const BAR0: u16 = 0x10;
    /// Secondary bus number of a type-1 (bridge) header.
    pub const SECONDARY_BUS: u16 = 0x19;
}

/// Number of BARs in a type-0 (endpoint) configuration header.
const NUM_BARS: usize = 6;

/// Value configuration reads return for a function that does not exist.
pub const INVALID_VENDOR_ID: u16 = 0xffff;

/// Header type field, masked to drop the multi-function bit 7.
pub mod header_type {
    pub const ENDPOINT: u8 = 0x00;
    pub const PCI_BRIDGE: u8 = 0x01;
}

/// Read access to PCI configuration space. Implemented by the vendor layer
/// (ECAM MMIO on both x86 vendors; the legacy 0xCF8/0xCFC port pair as a
/// fallback).
///
/// Read-only by design: the monitor enumerates and then denies, and never
/// reprograms a device on the host's behalf.
pub trait ConfigAccess {
    /// Read a 32-bit aligned configuration dword. Returns `!0` for a
    /// non-existent function, matching how hardware answers an unclaimed
    /// configuration cycle.
    fn read32(&self, bdf: Bdf, offset: u16) -> u32;

    fn read16(&self, bdf: Bdf, offset: u16) -> u16 {
        let dword = self.read32(bdf, offset & !0x3);
        (dword >> ((offset & 0x2) * 8)) as u16
    }

    fn read8(&self, bdf: Bdf, offset: u16) -> u8 {
        let dword = self.read32(bdf, offset & !0x3);
        (dword >> ((offset & 0x3) * 8)) as u8
    }
}

/// One decoded memory BAR aperture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BarRegion {
    pub start: HostPhysAddr,
    pub size: u64,
}

/// A memory BAR's low bits are attribute flags, not address (PCI spec §6.2.5.1).
const BAR_IO_SPACE: u32 = 1 << 0;
const BAR_TYPE_MASK: u32 = 0b110;
const BAR_TYPE_64BIT: u32 = 0b100;
const BAR_MEM_ADDR_MASK: u32 = !0xf;

/// Decode the memory BARs of a type-0 header.
///
/// Only the *programmed* base addresses are read; the monitor deliberately
/// does not size BARs by writing all-ones, because that is destructive on a
/// live device and firmware has already assigned every aperture by the time
/// the monitor runs. Sizes are therefore inferred from the gap to the next
/// aperture, see [`decode_bars_with_size`].
pub fn decode_bar_bases(cfg: &dyn ConfigAccess, bdf: Bdf) -> Vec<(usize, u64, bool)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < NUM_BARS {
        let offset = offset::BAR0 + (i as u16) * 4;
        let low = cfg.read32(bdf, offset);
        if low == 0 || low & BAR_IO_SPACE != 0 {
            // Unimplemented, or an I/O-space BAR (not memory, nothing to
            // unmap from the guest's memory view).
            i += 1;
            continue;
        }
        let is_64bit = low & BAR_TYPE_MASK == BAR_TYPE_64BIT;
        let base = if is_64bit {
            if i + 1 >= NUM_BARS {
                // Malformed header: a 64-bit BAR needs a following dword.
                break;
            }
            let high = cfg.read32(bdf, offset + 4) as u64;
            ((low & BAR_MEM_ADDR_MASK) as u64) | (high << 32)
        } else {
            (low & BAR_MEM_ADDR_MASK) as u64
        };
        if base != 0 {
            out.push((i, base, is_64bit));
        }
        i += if is_64bit { 2 } else { 1 };
    }
    out
}

/// Probe the size of one memory BAR.
///
/// Sizing a BAR requires writing all-ones and reading back the mask, which
/// momentarily removes the aperture from the address space. That is safe here
/// only because it happens before the host's own PCI enumeration (the monitor
/// runs with Linux still parked in `primary_init_late`) and the original value
/// is restored immediately. Needs write access, so it takes a separate trait.
pub trait ConfigWriteAccess: ConfigAccess {
    fn write32(&self, bdf: Bdf, offset: u16, value: u32);
}

/// Size the memory BARs of a type-0 header by the write-all-ones method.
///
/// Returns the decoded apertures. The caller must guarantee the host is not
/// concurrently touching this device's config space.
pub fn decode_bars_with_size(cfg: &dyn ConfigWriteAccess, bdf: Bdf) -> Vec<BarRegion> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < NUM_BARS {
        let offset = offset::BAR0 + (i as u16) * 4;
        let low = cfg.read32(bdf, offset);
        if low & BAR_IO_SPACE != 0 {
            i += 1;
            continue;
        }
        let is_64bit = low & BAR_TYPE_MASK == BAR_TYPE_64BIT;
        let stride = if is_64bit { 2 } else { 1 };
        if is_64bit && i + 1 >= NUM_BARS {
            break;
        }
        let high = if is_64bit {
            cfg.read32(bdf, offset + 4)
        } else {
            0
        };

        // Write all-ones, read back the writable (address) bits, restore.
        cfg.write32(bdf, offset, !0);
        let mask_low = cfg.read32(bdf, offset);
        let mask_high = if is_64bit {
            cfg.write32(bdf, offset + 4, !0);
            let m = cfg.read32(bdf, offset + 4);
            cfg.write32(bdf, offset + 4, high);
            m
        } else {
            !0
        };
        cfg.write32(bdf, offset, low);

        let size = bar_size_from_mask(mask_low, mask_high, is_64bit);
        let base = ((low & BAR_MEM_ADDR_MASK) as u64) | ((high as u64) << 32);
        if size != 0 && base != 0 {
            out.push(BarRegion {
                start: base as HostPhysAddr,
                size,
            });
        }
        i += stride;
    }
    out
}

/// Turn a BAR size mask into a byte count.
///
/// After writing all-ones a memory BAR reads back zeros in the bits it does
/// not decode, so the aperture size is `!(mask & addr_bits) + 1`.
fn bar_size_from_mask(mask_low: u32, mask_high: u32, is_64bit: bool) -> u64 {
    let mask = if is_64bit {
        ((mask_high as u64) << 32) | ((mask_low & BAR_MEM_ADDR_MASK) as u64)
    } else {
        // Only 32 address bits are decodable; set the upper half so the
        // complement below does not produce a 4GB-plus size.
        0xffff_ffff_0000_0000 | ((mask_low & BAR_MEM_ADDR_MASK) as u64)
    };
    if mask == 0 || mask == 0xffff_ffff_ffff_ffff {
        return 0;
    }
    (!mask).wrapping_add(1)
}

/// Read the identity of one function, returning `None` if nothing answers.
pub fn probe_ident(
    cfg: &dyn ConfigAccess,
    bdf: Bdf,
    largest_bar_size: u64,
) -> Option<DeviceIdent> {
    let vendor_id = cfg.read16(bdf, offset::VENDOR_ID);
    if vendor_id == INVALID_VENDOR_ID || vendor_id == 0 {
        return None;
    }
    Some(DeviceIdent {
        bdf,
        vendor_id,
        device_id: cfg.read16(bdf, offset::DEVICE_ID),
        // Class code occupies bits 31:16 of the class/revision dword; the
        // monitor compares base class + subclass, so drop the prog-if byte.
        class_code: (cfg.read32(bdf, offset::CLASS_REVISION) >> 16) as u16,
        largest_bar_size,
    })
}

/// Whether this function is a multi-function device (header type bit 7).
pub fn is_multi_function(cfg: &dyn ConfigAccess, bdf: Bdf) -> bool {
    cfg.read8(bdf, offset::HEADER_TYPE) & 0x80 != 0
}

/// Header type with the multi-function bit masked off.
pub fn header_kind(cfg: &dyn ConfigAccess, bdf: Bdf) -> u8 {
    cfg.read8(bdf, offset::HEADER_TYPE) & 0x7f
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;

    /// A fake config space: a map from (bdf, dword offset) to value.
    struct FakeConfig {
        regs: BTreeMap<(u16, u16), u32>,
    }

    impl FakeConfig {
        fn new() -> Self {
            Self {
                regs: BTreeMap::new(),
            }
        }
        fn set(&mut self, bdf: Bdf, offset: u16, value: u32) {
            self.regs.insert((bdf.raw(), offset & !0x3), value);
        }
    }

    impl ConfigAccess for FakeConfig {
        fn read32(&self, bdf: Bdf, offset: u16) -> u32 {
            *self
                .regs
                .get(&(bdf.raw(), offset & !0x3))
                .unwrap_or(&0xffff_ffff)
        }
    }

    const GB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn test_read_sub_dword() {
        let bdf = Bdf::new(1, 0, 0);
        let mut cfg = FakeConfig::new();
        cfg.set(bdf, 0x00, 0x2335_10de);
        assert_eq!(cfg.read16(bdf, offset::VENDOR_ID), 0x10de);
        assert_eq!(cfg.read16(bdf, offset::DEVICE_ID), 0x2335);
        assert_eq!(cfg.read8(bdf, 0x00), 0xde);
        assert_eq!(cfg.read8(bdf, 0x01), 0x10);
        assert_eq!(cfg.read8(bdf, 0x02), 0x35);
        assert_eq!(cfg.read8(bdf, 0x03), 0x23);
    }

    #[test]
    fn test_probe_ident_absent_device() {
        let cfg = FakeConfig::new();
        // Nothing configured: reads return all-ones -> no device.
        assert!(probe_ident(&cfg, Bdf::new(0, 0, 0), 0).is_none());
    }

    #[test]
    fn test_probe_ident_reads_class_without_progif() {
        let bdf = Bdf::new(0x65, 0, 0);
        let mut cfg = FakeConfig::new();
        cfg.set(bdf, 0x00, 0x2335_10de);
        // class 0x03, subclass 0x02, prog-if 0x00, revision 0xa1
        cfg.set(bdf, offset::CLASS_REVISION, 0x0302_00a1);
        let ident = probe_ident(&cfg, bdf, 128 * GB).unwrap();
        assert_eq!(ident.vendor_id, 0x10de);
        assert_eq!(ident.device_id, 0x2335);
        assert_eq!(ident.class_code, 0x0302);
        assert!(ident.is_protected());
    }

    #[test]
    fn test_decode_bar_bases_64bit() {
        let bdf = Bdf::new(0x65, 0, 0);
        let mut cfg = FakeConfig::new();
        // BAR0/1: 64-bit memory BAR at 0x3800_0000_0000 (above 4GB).
        cfg.set(bdf, offset::BAR0, 0x0000_000c);
        cfg.set(bdf, offset::BAR0 + 4, 0x0000_3800);
        // BAR2: 32-bit memory BAR at 0xf000_0000.
        cfg.set(bdf, offset::BAR0 + 8, 0xf000_0000);
        // BAR3: an I/O BAR, must be skipped.
        cfg.set(bdf, offset::BAR0 + 12, 0x0000_3001);
        // BAR4/5: unimplemented.
        cfg.set(bdf, offset::BAR0 + 16, 0);
        cfg.set(bdf, offset::BAR0 + 20, 0);

        let bars = decode_bar_bases(&cfg, bdf);
        assert_eq!(bars.len(), 2);
        // 64-bit BAR: high dword combined, attribute bits stripped.
        assert_eq!(bars[0], (0, 0x3800_0000_0000, true));
        // 32-bit BAR follows at index 2 (index 1 consumed by the 64-bit pair).
        assert_eq!(bars[1], (2, 0xf000_0000, false));
    }

    /// Fake config space that also supports the write-all-ones size probe.
    struct SizableConfig {
        inner: FakeConfig,
        /// Per-dword writable address mask, i.e. what reads back after !0.
        masks: BTreeMap<(u16, u16), u32>,
        written: core::cell::RefCell<BTreeMap<(u16, u16), u32>>,
    }

    impl ConfigAccess for SizableConfig {
        fn read32(&self, bdf: Bdf, offset: u16) -> u32 {
            if let Some(v) = self.written.borrow().get(&(bdf.raw(), offset & !0x3)) {
                // A pending all-ones write reads back as the size mask.
                if *v == !0 {
                    if let Some(m) = self.masks.get(&(bdf.raw(), offset & !0x3)) {
                        return *m;
                    }
                }
                return *v;
            }
            self.inner.read32(bdf, offset)
        }
    }

    impl ConfigWriteAccess for SizableConfig {
        fn write32(&self, bdf: Bdf, offset: u16, value: u32) {
            self.written
                .borrow_mut()
                .insert((bdf.raw(), offset & !0x3), value);
        }
    }

    #[test]
    fn test_decode_bars_with_size_restores_original() {
        let bdf = Bdf::new(0x65, 0, 0);
        let mut inner = FakeConfig::new();
        // 64-bit BAR at 0x3800_0000_0000, 128GB aperture.
        inner.set(bdf, offset::BAR0, 0x0000_000c);
        inner.set(bdf, offset::BAR0 + 4, 0x0000_3800);
        // Remaining BARs unimplemented.
        for i in 2..6u16 {
            inner.set(bdf, offset::BAR0 + i * 4, 0);
        }
        let mut masks = BTreeMap::new();
        // 128GB = 0x20_0000_0000: mask clears the low 37 bits.
        masks.insert((bdf.raw(), offset::BAR0), 0x0000_000c);
        masks.insert((bdf.raw(), offset::BAR0 + 4), 0xffff_ffe0);

        let cfg = SizableConfig {
            inner,
            masks,
            written: core::cell::RefCell::new(BTreeMap::new()),
        };
        let bars = decode_bars_with_size(&cfg, bdf);
        assert_eq!(bars.len(), 1);
        assert_eq!(bars[0].start, 0x3800_0000_0000);
        assert_eq!(bars[0].size, 128 * GB);

        // The original BAR values must be back in place afterwards.
        assert_eq!(cfg.read32(bdf, offset::BAR0), 0x0000_000c);
        assert_eq!(cfg.read32(bdf, offset::BAR0 + 4), 0x0000_3800);
    }

    #[test]
    fn test_bar_size_from_mask() {
        // 32-bit BAR, 16MB aperture: mask clears the low 24 bits.
        assert_eq!(
            bar_size_from_mask(0xff00_0000, !0, false),
            16 * 1024 * 1024
        );
        // 32-bit BAR, 256MB.
        assert_eq!(
            bar_size_from_mask(0xf000_0000, !0, false),
            256 * 1024 * 1024
        );
        // 64-bit BAR, 128GB.
        assert_eq!(bar_size_from_mask(0x0000_0000, 0xffff_ffe0, true), 128 * GB);
        // 64-bit BAR, 8GB (Ascend-class aperture).
        assert_eq!(bar_size_from_mask(0x0000_0000, 0xffff_fffe, true), 8 * GB);
        // An unimplemented BAR reads back zero -> no aperture.
        assert_eq!(bar_size_from_mask(0, 0, true), 0);
    }
}
