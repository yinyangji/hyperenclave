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

//! x86 PCI configuration access via the legacy 0xCF8/0xCFC port pair.
//!
//! The monitor enumerates the bus during `cell::init()`, *before* its own
//! page tables exist and while it still runs on Linux's CR3 — so an ECAM MMIO
//! window cannot be assumed to be mapped. The legacy configuration mechanism
//! needs no mapping at all and reaches every register the classifier reads
//! (vendor/device/class at 0x00-0x0f and the BARs at 0x10-0x27), which all sit
//! inside the 256-byte window it can address. ECAM is only required for
//! extended capabilities, which the monitor does not interpret.
//!
//! This is the privileged side of the same mechanism whose guest accesses
//! trap into `pio.rs`: the monitor owns these ports once the guest's
//! accesses start trapping, and re-performs only the ones targeting
//! unprotected devices.

use x86_64::instructions::port::Port;

use crate::device::bdf::Bdf;
use crate::device::pci::{ConfigAccess, ConfigWriteAccess};

const CONFIG_ADDRESS: u16 = 0xcf8;
const CONFIG_DATA: u16 = 0xcfc;

/// Build a CONFIG_ADDRESS value: enable bit 31, bus 23:16, device 15:11,
/// function 10:8, dword-aligned register 7:2 (PCI Local Bus Spec §3.2.2.3.2).
pub fn config_address(bdf: Bdf, offset: u16) -> u32 {
    0x8000_0000
        | ((bdf.bus() as u32) << 16)
        | ((bdf.device() as u32) << 11)
        | ((bdf.function() as u32) << 8)
        | ((offset as u32) & 0xfc)
}

/// Decode a CONFIG_ADDRESS value back into the function it selects and the
/// register offset within it. Returns `None` if the enable bit is clear.
///
/// Used by the PIO emulation path to decide whether a guest 0xCFC access
/// targets a protected device.
pub fn decode_config_address(value: u32) -> Option<(Bdf, u16)> {
    if value & 0x8000_0000 == 0 {
        return None;
    }
    let bus = ((value >> 16) & 0xff) as u8;
    let device = ((value >> 11) & 0x1f) as u8;
    let function = ((value >> 8) & 0x7) as u8;
    Some((Bdf::new(bus, device, function), (value & 0xfc) as u16))
}

/// Legacy port-pair configuration access, used by the monitor itself.
pub struct LegacyPciConfig;

impl ConfigAccess for LegacyPciConfig {
    fn read32(&self, bdf: Bdf, offset: u16) -> u32 {
        // The legacy mechanism only addresses the first 256 bytes.
        if offset >= 0x100 {
            return !0;
        }
        let mut addr_port = Port::<u32>::new(CONFIG_ADDRESS);
        let mut data_port = Port::<u32>::new(CONFIG_DATA);
        // SAFETY: the monitor owns these ports while it enumerates (all other
        // CPUs are parked on the init barrier and Linux is not yet running as
        // a guest). Reading configuration space has no side effects.
        unsafe {
            addr_port.write(config_address(bdf, offset));
            data_port.read()
        }
    }
}

impl ConfigWriteAccess for LegacyPciConfig {
    fn write32(&self, bdf: Bdf, offset: u16, value: u32) {
        if offset >= 0x100 {
            return;
        }
        let mut addr_port = Port::<u32>::new(CONFIG_ADDRESS);
        let mut data_port = Port::<u32>::new(CONFIG_DATA);
        // SAFETY: same ownership argument as the read path. The only writes
        // the monitor issues are the BAR size probe, which restores the
        // original value before returning (see `pci::decode_bars_with_size`).
        unsafe {
            addr_port.write(config_address(bdf, offset));
            data_port.write(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_address_encoding() {
        // 0000:65:00.0 register 0x00.
        let addr = config_address(Bdf::new(0x65, 0, 0), 0x00);
        assert_eq!(addr, 0x8065_0000);
        // Register offsets are forced dword-aligned.
        assert_eq!(config_address(Bdf::new(0x65, 0, 0), 0x02), 0x8065_0000);
        assert_eq!(config_address(Bdf::new(0x65, 0, 0), 0x10), 0x8065_0010);
        // Device and function land in bits 15:11 and 10:8.
        assert_eq!(config_address(Bdf::new(0x00, 0x1f, 0x3), 0x00), 0x8000_fb00);
    }

    #[test]
    fn test_decode_config_address_roundtrip() {
        for bdf in [
            Bdf::new(0x65, 0x00, 0x0),
            Bdf::new(0x00, 0x1f, 0x7),
            Bdf::new(0xff, 0x00, 0x1),
        ] {
            for offset in [0x00u16, 0x10, 0x24, 0xfc] {
                let addr = config_address(bdf, offset);
                assert_eq!(decode_config_address(addr), Some((bdf, offset)));
            }
        }
    }

    #[test]
    fn test_decode_config_address_requires_enable_bit() {
        // Bit 31 clear: the cycle targets nothing.
        assert_eq!(decode_config_address(0x0065_0000), None);
    }

    #[test]
    fn test_decode_ignores_low_two_bits() {
        // Hardware ignores bits 1:0 of CONFIG_ADDRESS; a guest may leave
        // them set and must still be resolved to the same register.
        let (bdf, offset) = decode_config_address(0x8065_0013).unwrap();
        assert_eq!(bdf, Bdf::new(0x65, 0, 0));
        assert_eq!(offset, 0x10);
    }
}
