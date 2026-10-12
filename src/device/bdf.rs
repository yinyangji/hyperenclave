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

//! Bus/Device/Function device identity.
//!
//! Bus-topology addressing is not x86-specific: PCIe enumerates the same way
//! behind an ARM SMMUv3 root complex, where the SMMU StreamID for a PCIe
//! device is derived from exactly these 16 bits. Keeping `Bdf` in the
//! architecture-neutral layer lets the device policy and the IOMMU domain
//! bookkeeping be shared by every backend.

use core::fmt;

/// A 16-bit routing ID: bus[15:8], device[7:3], function[2:0].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Bdf(u16);

impl Bdf {
    /// Number of configuration-space bytes per function.
    pub const CONFIG_SIZE: usize = 4096;

    pub const fn new(bus: u8, device: u8, function: u8) -> Self {
        Self(((bus as u16) << 8) | (((device & 0x1f) as u16) << 3) | ((function & 0x7) as u16))
    }

    /// Builds a `Bdf` from a raw 16-bit routing ID.
    pub const fn from_raw(raw: u16) -> Self {
        Self(raw)
    }

    /// The raw 16-bit routing ID, as used for IOMMU device-table indexing
    /// (AMD-Vi) and as the PCIe StreamID base (ARM SMMUv3).
    pub const fn raw(self) -> u16 {
        self.0
    }

    pub const fn bus(self) -> u8 {
        (self.0 >> 8) as u8
    }

    pub const fn device(self) -> u8 {
        ((self.0 >> 3) & 0x1f) as u8
    }

    pub const fn function(self) -> u8 {
        (self.0 & 0x7) as u8
    }

    /// Byte offset of this function's configuration space inside an ECAM
    /// window: bus/device/function are packed as bits 27:12 of the offset
    /// (PCIe Base Spec, enhanced configuration access mechanism).
    pub const fn ecam_offset(self) -> usize {
        (self.0 as usize) << 12
    }
}

impl fmt::Display for Bdf {
    /// Renders the conventional `0000:bb:dd.f` form so log lines can be
    /// matched against `lspci` output directly.
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "0000:{:02x}:{:02x}.{}",
            self.bus(),
            self.device(),
            self.function()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

    #[test]
    fn test_bdf_roundtrip() {
        let bdf = Bdf::new(0x65, 0x00, 0x0);
        assert_eq!(bdf.bus(), 0x65);
        assert_eq!(bdf.device(), 0x00);
        assert_eq!(bdf.function(), 0x0);
        assert_eq!(Bdf::from_raw(bdf.raw()), bdf);

        // Every field at its maximum, to catch mask/shift mistakes.
        let bdf = Bdf::new(0xff, 0x1f, 0x7);
        assert_eq!((bdf.bus(), bdf.device(), bdf.function()), (0xff, 0x1f, 0x7));
        assert_eq!(bdf.raw(), 0xffff);
    }

    #[test]
    fn test_bdf_field_overflow_is_masked() {
        // Device and function are narrower than a byte; excess bits must not
        // bleed into the neighbouring field.
        let bdf = Bdf::new(0x01, 0xff, 0xff);
        assert_eq!((bdf.bus(), bdf.device(), bdf.function()), (0x01, 0x1f, 0x7));
    }

    #[test]
    fn test_ecam_offset() {
        // 0000:00:00.0 sits at the start of the window.
        assert_eq!(Bdf::new(0, 0, 0).ecam_offset(), 0);
        // Each function is one 4KB page apart.
        assert_eq!(Bdf::new(0, 0, 1).ecam_offset(), 0x1000);
        // Each device is eight functions apart.
        assert_eq!(Bdf::new(0, 1, 0).ecam_offset(), 0x8000);
        // Each bus is 256 functions (1MB) apart.
        assert_eq!(Bdf::new(1, 0, 0).ecam_offset(), 0x10_0000);
        // The window spans 256MB for 256 buses.
        assert_eq!(
            Bdf::new(0xff, 0x1f, 0x7).ecam_offset() + Bdf::CONFIG_SIZE,
            0x1000_0000
        );
    }

    #[test]
    fn test_display_matches_lspci() {
        assert_eq!(format!("{}", Bdf::new(0x65, 0x00, 0x0)), "0000:65:00.0");
        assert_eq!(format!("{}", Bdf::new(0x0d, 0x1f, 0x7)), "0000:0d:1f.7");
    }
}
