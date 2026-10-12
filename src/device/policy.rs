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

//! What the monitor does with an access to a given device.
//!
//! The table is consulted from two places that must agree, or the host would
//! see an inconsistent machine: the GPM construction in `cell.rs` (which
//! decides what the host's memory view contains) and the PIO emulation in
//! `arch/x86_64/pio.rs` (which answers legacy configuration cycles). Keeping
//! the decision in one architecture-neutral function is what makes those two
//! paths consistent by construction.

use super::bdf::Bdf;
use super::ident::DeviceIdent;

/// The monitor's disposition towards one device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceAction {
    /// Not an accelerator: the host owns it exactly as before. This is the
    /// overwhelming majority of the system (NICs, storage, USB, the iGPU
    /// driving the console).
    Passthrough,
    /// A protected accelerator. Its configuration space and BARs read as
    /// all-ones from the host and its DMA is confined to its own domain,
    /// whether or not the device is currently assigned to an owner.
    ///
    /// Assignment changes who can *reach* the device through its IOMMU
    /// domain; it never makes the device visible to the host again. That is
    /// the whole point: the host is the untrusted party in this threat model,
    /// so "assigned" must not be a way back in.
    Protect,
}

impl DeviceAction {
    pub fn is_protected(self) -> bool {
        matches!(self, Self::Protect)
    }
}

/// Decide the action for a freshly enumerated device.
pub fn classify(ident: &DeviceIdent) -> DeviceAction {
    if ident.is_protected() {
        DeviceAction::Protect
    } else {
        DeviceAction::Passthrough
    }
}

/// One protected device as recorded by the manager.
#[derive(Debug, Clone)]
pub struct ProtectedDevice {
    pub ident: DeviceIdent,
    /// Configuration-space physical page, when an ECAM window was located.
    /// `None` means only legacy PIO config cycles reach this device, which
    /// the PIO path still denies.
    pub config_page: Option<usize>,
    /// Memory apertures (BARs) to remove from the host's view.
    pub bars: alloc::vec::Vec<(usize, u64)>,
    /// Hardware domain id assigned to this device.
    pub domain_id: u16,
}

impl ProtectedDevice {
    pub fn bdf(&self) -> Bdf {
        self.ident.bdf
    }

    /// Whether `paddr` falls inside one of this device's BAR apertures.
    pub fn bar_contains(&self, paddr: usize) -> bool {
        self.bars
            .iter()
            .any(|&(start, size)| paddr >= start && (paddr as u64) < start as u64 + size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn h200() -> DeviceIdent {
        DeviceIdent {
            bdf: Bdf::new(0x65, 0, 0),
            vendor_id: 0x10de,
            device_id: 0x2335,
            class_code: 0x0302,
            largest_bar_size: 128 * 1024 * 1024 * 1024,
        }
    }

    fn igpu() -> DeviceIdent {
        DeviceIdent {
            bdf: Bdf::new(0x00, 0x02, 0),
            vendor_id: 0x8086,
            device_id: 0x7d55,
            class_code: 0x0300,
            largest_bar_size: 256 * 1024 * 1024,
        }
    }

    #[test]
    fn test_classify_accelerator_vs_igpu() {
        assert_eq!(classify(&h200()), DeviceAction::Protect);
        assert!(classify(&h200()).is_protected());
        // The console GPU must stay passthrough.
        assert_eq!(classify(&igpu()), DeviceAction::Passthrough);
        assert!(!classify(&igpu()).is_protected());
    }

    #[test]
    fn test_bar_contains() {
        let dev = ProtectedDevice {
            ident: h200(),
            config_page: Some(0xe065_0000),
            bars: vec![
                (0x3800_0000_0000, 128 * 1024 * 1024 * 1024),
                (0xf000_0000, 0x100_0000),
            ],
            domain_id: 16,
        };
        // Inside the first (above-4GB) aperture.
        assert!(dev.bar_contains(0x3800_0000_0000));
        assert!(dev.bar_contains(0x3800_0000_1000));
        assert!(dev.bar_contains(0x381f_ffff_ffff));
        // Just past its end.
        assert!(!dev.bar_contains(0x3820_0000_0000));
        // Inside the second aperture, and just before its start.
        assert!(dev.bar_contains(0xf000_0000));
        assert!(!dev.bar_contains(0xefff_ffff));
        // Unrelated RAM address.
        assert!(!dev.bar_contains(0x1000_0000));
    }
}
