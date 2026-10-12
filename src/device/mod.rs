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

//! Accelerator device isolation.
//!
//! Goal: a discrete compute accelerator (datacentre GPU, NPU, DCU) must be
//! unusable by the downgraded host Linux, and reachable only by an owner the
//! monitor has explicitly assigned it to. Everything else about the machine
//! behaves exactly as it did before the monitor was loaded.
//!
//! Two orthogonal planes, both arbitrated here (HyperGPU design, see
//! `docs/arch-images/io.png`):
//!
//! - **Control plane** — the host must not be able to *drive* the device.
//!   Its configuration space and BAR apertures are replaced in the host's
//!   second-level page table (GPM) by a page that reads as all-ones, and
//!   legacy 0xCF8/0xCFC configuration cycles aimed at it are answered the
//!   same way. Linux then sees vendor id 0xFFFF, concludes no device is
//!   present, and never binds a driver. No instruction decoder is needed.
//!
//! - **Data plane** — the device must not be able to DMA where it pleases.
//!   Each protected device is pulled out of the shared IOMMU domain into its
//!   own, which starts empty, so its DMA faults until something is assigned.
//!
//! This module is deliberately architecture-neutral: `Bdf`, the policy table
//! and the domain bookkeeping contain nothing x86-specific. An ARM backend
//! supplies `ConfigAccess` plus `GenericIommu::set_device_page_table` and
//! reuses the rest.

pub mod bdf;
pub mod domain;
pub mod ident;
pub mod pci;
pub mod policy;

use alloc::vec::Vec;
use spin::Mutex;

use self::bdf::Bdf;
use self::domain::{DeviceDomain, DomainOwner, FIRST_PRIVATE_DOMAIN_ID};
use self::ident::{Classification, DeviceIdent};
use self::pci::{ConfigAccess, ConfigWriteAccess};
use self::policy::{DeviceAction, ProtectedDevice};
use crate::error::HvResult;
use crate::memory::PAGE_SIZE;
use crate::sync::LateInit;

/// The protected-device registry, built once during `cell::init()`.
pub static DEVICE_MANAGER: LateInit<DeviceManager> = LateInit::new();

/// Maximum PCI bus number scanned. A full 0..=255 sweep of all 8 functions is
/// 65536 configuration reads; buses are only followed when a bridge reports
/// them, so in practice far fewer are touched.
const MAX_BUS: u8 = 0xff;

pub struct DeviceManager {
    /// Protected devices, in enumeration order. Fixed after `init()`.
    devices: Vec<ProtectedDevice>,
    /// Per-device DMA domains, index-aligned with `devices`.
    domains: Mutex<Vec<DeviceDomain>>,
}

impl DeviceManager {
    /// Enumerate the bus and build the protected-device set.
    ///
    /// Runs before the host's own PCI enumeration, which is what makes
    /// hiding possible at all: by the time Linux probes the bus the apertures
    /// are already gone from its memory view.
    fn enumerate(cfg: &dyn ConfigWriteAccess) -> Self {
        let mut devices = Vec::new();
        let mut next_domain_id = FIRST_PRIVATE_DOMAIN_ID;

        for bus in 0..=MAX_BUS {
            for device in 0..32u8 {
                // Function 0 must exist for any device to be present.
                let f0 = Bdf::new(bus, device, 0);
                if cfg.read16(f0, pci::offset::VENDOR_ID) == pci::INVALID_VENDOR_ID {
                    continue;
                }
                let num_functions = if pci::is_multi_function(cfg, f0) { 8 } else { 1 };
                for function in 0..num_functions {
                    let bdf = Bdf::new(bus, device, function);
                    let bars = pci::decode_bars_with_size(cfg, bdf);
                    let largest = bars.iter().map(|b| b.size).max().unwrap_or(0);
                    let ident = match pci::probe_ident(cfg, bdf, largest) {
                        Some(ident) => ident,
                        None => continue,
                    };

                    let class = ident.classify();
                    if policy::classify(&ident) != DeviceAction::Protect {
                        debug!(
                            "device {} [{:04x}:{:04x}] class {:#06x}: {:?}, host keeps it",
                            bdf, ident.vendor_id, ident.device_id, ident.class_code, class
                        );
                        continue;
                    }

                    let domain_id = next_domain_id;
                    next_domain_id = next_domain_id.saturating_add(1);
                    let dev = ProtectedDevice {
                        ident,
                        // Populated by the vendor layer when an ECAM window
                        // is located; the PIO path denies access regardless.
                        config_page: None,
                        bars: bars.iter().map(|b| (b.start, b.size)).collect(),
                        domain_id,
                    };
                    println!(
                        "HyperEnclave: protecting accelerator {} [{:04x}:{:04x}] \
                         class {:#06x} ({:?}), domain {}, {} BAR(s)",
                        bdf,
                        ident.vendor_id,
                        ident.device_id,
                        ident.class_code,
                        class,
                        domain_id,
                        dev.bars.len()
                    );
                    for &(start, size) in &dev.bars {
                        info!(
                            "  {} BAR [{:#x}, {:#x}) size {:#x}",
                            bdf,
                            start,
                            start as u64 + size,
                            size
                        );
                    }
                    devices.push(dev);
                }
            }
        }

        let domains = devices
            .iter()
            .map(|d| DeviceDomain::new(d.domain_id, d.bdf()))
            .collect();
        Self {
            devices,
            domains: Mutex::new(domains),
        }
    }

    /// Record the ECAM configuration page of each protected device, so the
    /// GPM construction can replace it.
    fn set_config_pages(&mut self, ecam_base: usize) {
        for dev in &mut self.devices {
            dev.config_page = Some(ecam_base + dev.bdf().ecam_offset());
        }
    }

    pub fn devices(&self) -> &[ProtectedDevice] {
        &self.devices
    }

    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }

    /// Whether this routing ID belongs to a protected accelerator.
    pub fn is_protected(&self, bdf: Bdf) -> bool {
        self.devices.iter().any(|d| d.bdf() == bdf)
    }

    pub fn find(&self, bdf: Bdf) -> Option<&ProtectedDevice> {
        self.devices.iter().find(|d| d.bdf() == bdf)
    }

    /// Whether `paddr` lies in a protected device's configuration page or in
    /// one of its BAR apertures — i.e. whether this physical page must be
    /// withheld from the host's memory view.
    pub fn is_protected_mmio(&self, paddr: usize) -> bool {
        self.devices.iter().any(|d| {
            d.bar_contains(paddr)
                || d.config_page
                    .is_some_and(|c| paddr >= c && paddr < c + Bdf::CONFIG_SIZE)
        })
    }

    /// Physical page ranges that must be withheld from the host.
    pub fn protected_mmio_ranges(&self) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for dev in &self.devices {
            if let Some(config) = dev.config_page {
                out.push((config, Bdf::CONFIG_SIZE));
            }
            for &(start, size) in &dev.bars {
                // An aperture smaller than a page still costs a full page in
                // the second-level page table; round up so no tail of the
                // aperture stays mapped.
                let size = (size as usize).next_multiple_of(PAGE_SIZE);
                out.push((start, size));
            }
        }
        out
    }

    /// Assign a device to an owner and grant it DMA to `ranges`.
    ///
    /// Caller must have validated that every range is normal-world memory.
    pub fn assign(&self, bdf: Bdf, handle: u64, ranges: &[(usize, usize)]) -> HvResult {
        let mut domains = self.domains.lock();
        let domain = domains
            .iter_mut()
            .find(|d| d.bdf == bdf)
            .ok_or_else(|| hv_err!(ENODEV, format!("{} is not a protected device", bdf)))?;
        if domain.is_assigned() {
            return hv_result_err!(
                EBUSY,
                format!("{} is already assigned to {:?}", bdf, domain.owner)
            );
        }
        // Build the full mapping before publishing the owner, so a failure
        // half way leaves the device fail-closed rather than partly open.
        for &(start, size) in ranges {
            if let Err(e) = domain.map_range(start, size) {
                domain.clear();
                return Err(e);
            }
        }
        domain.owner = DomainOwner::Assigned { handle };
        info!(
            "assigned {} to handle {:#x}, domain {}, {} range(s)",
            bdf,
            handle,
            domain.domain_id,
            ranges.len()
        );
        Ok(())
    }

    /// Revoke an assignment, returning the device to fail-closed.
    pub fn release(&self, bdf: Bdf) -> HvResult {
        let mut domains = self.domains.lock();
        let domain = domains
            .iter_mut()
            .find(|d| d.bdf == bdf)
            .ok_or_else(|| hv_err!(ENODEV, format!("{} is not a protected device", bdf)))?;
        domain.clear();
        domain.owner = DomainOwner::Unassigned;
        info!("released {}, DMA starved again", bdf);
        Ok(())
    }

    /// Run `f` against the domain of `bdf`.
    pub fn with_domain<R>(&self, bdf: Bdf, f: impl FnOnce(&DeviceDomain) -> R) -> Option<R> {
        let domains = self.domains.lock();
        domains.iter().find(|d| d.bdf == bdf).map(f)
    }

    /// Program every protected device's private domain into the IOMMU,
    /// taking it out of the shared default domain.
    pub fn program_iommu(&self, iommus: &[crate::arch::vmm::Iommu]) -> HvResult {
        use crate::iommu::GenericIommu;
        let domains = self.domains.lock();
        for domain in domains.iter() {
            for iommu in iommus {
                iommu.set_device_page_table(
                    domain.bdf,
                    domain.page_table(),
                    domain.domain_id,
                )?;
            }
            info!(
                "{} moved to private IOMMU domain {} (root {:#x}), DMA starved until assigned",
                domain.bdf,
                domain.domain_id,
                domain.root_paddr()
            );
        }
        Ok(())
    }
}

/// Enumerate the bus and record which accelerators to protect.
///
/// Must run before `Cell::new_root()` builds the GPM, because that is where
/// the devices' apertures are withheld from the host.
pub fn init() -> HvResult {
    let cfg = crate::arch::PciConfig;
    let mut manager = DeviceManager::enumerate(&cfg);

    match crate::arch::ecam_base() {
        Some(base) => {
            info!("ECAM window at {:#x}", base);
            manager.set_config_pages(base);
        }
        // Without an ECAM window the host can only reach configuration space
        // through the legacy ports, which the PIO policy already denies for
        // protected devices. The BAR apertures are hidden either way, so the
        // device stays unusable; only the MMCFG belt-and-braces is missing.
        None => warn!(
            "no ECAM window located; protected devices' config space is denied \
             via legacy PIO only"
        ),
    }

    if manager.is_empty() {
        println!("HyperEnclave: no discrete accelerator found, device isolation idle");
    } else {
        println!(
            "HyperEnclave: device isolation active, {} accelerator(s) withheld from host",
            manager.devices().len()
        );
    }

    // SAFETY: called from `cell::init()` on the primary CPU while every
    // secondary CPU is still parked on the INIT_EARLY_OK barrier, so no other
    // CPU can observe the value before this returns.
    unsafe { DEVICE_MANAGER.init(manager) };
    Ok(())
}

/// Whether a protected accelerator owns this physical page.
///
/// Safe to call before `init()`: returns `false`, which is the pre-isolation
/// behaviour (host sees everything).
pub fn is_protected_mmio(paddr: usize) -> bool {
    DEVICE_MANAGER
        .try_get()
        .is_some_and(|m| m.is_protected_mmio(paddr))
}

/// Physical ranges to withhold from the host's memory view. Empty before
/// `init()` runs, or when no accelerator is present.
pub fn protected_mmio_ranges() -> Vec<(usize, usize)> {
    DEVICE_MANAGER
        .try_get()
        .map(|m| m.protected_mmio_ranges())
        .unwrap_or_default()
}

/// Whether this routing ID is a protected accelerator.
pub fn is_protected_device(bdf: Bdf) -> bool {
    DEVICE_MANAGER
        .try_get()
        .is_some_and(|m| m.is_protected(bdf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;
    use core::cell::RefCell;

    /// A synthetic bus: one iGPU at 00:02.0 and one H200 at 65:00.0.
    struct FakeBus {
        regs: BTreeMap<(u16, u16), u32>,
        masks: BTreeMap<(u16, u16), u32>,
        written: RefCell<BTreeMap<(u16, u16), u32>>,
    }

    impl FakeBus {
        fn new() -> Self {
            let mut regs = BTreeMap::new();
            let mut masks = BTreeMap::new();

            // 00:02.0 Intel iGPU, class 0300, 256MB BAR -> must stay visible.
            let igpu = Bdf::new(0, 2, 0).raw();
            regs.insert((igpu, 0x00), 0x7d55_8086);
            regs.insert((igpu, 0x08), 0x0300_0001);
            regs.insert((igpu, 0x0c), 0x0000_0000);
            regs.insert((igpu, 0x10), 0x0000_000c);
            regs.insert((igpu, 0x14), 0x0000_0060);
            masks.insert((igpu, 0x10), 0x0000_000c);
            masks.insert((igpu, 0x14), 0xffff_ffff);

            // 65:00.0 NVIDIA H200, class 0302, 128GB BAR -> must be hidden.
            let gpu = Bdf::new(0x65, 0, 0).raw();
            regs.insert((gpu, 0x00), 0x2335_10de);
            regs.insert((gpu, 0x08), 0x0302_00a1);
            regs.insert((gpu, 0x0c), 0x0000_0000);
            regs.insert((gpu, 0x10), 0x0000_000c);
            regs.insert((gpu, 0x14), 0x0000_3800);
            masks.insert((gpu, 0x10), 0x0000_000c);
            masks.insert((gpu, 0x14), 0xffff_ffe0);

            Self {
                regs,
                masks,
                written: RefCell::new(BTreeMap::new()),
            }
        }
    }

    impl ConfigAccess for FakeBus {
        fn read32(&self, bdf: Bdf, offset: u16) -> u32 {
            let key = (bdf.raw(), offset & !0x3);
            if let Some(v) = self.written.borrow().get(&key) {
                if *v == !0 {
                    return *self.masks.get(&key).unwrap_or(&0);
                }
                return *v;
            }
            *self.regs.get(&key).unwrap_or(&0xffff_ffff)
        }
    }

    impl ConfigWriteAccess for FakeBus {
        fn write32(&self, bdf: Bdf, offset: u16, value: u32) {
            self.written
                .borrow_mut()
                .insert((bdf.raw(), offset & !0x3), value);
        }
    }

    const GB: u64 = 1024 * 1024 * 1024;

    fn manager() -> DeviceManager {
        DeviceManager::enumerate(&FakeBus::new())
    }

    #[test]
    fn test_enumeration_protects_only_the_accelerator() {
        let m = manager();
        assert_eq!(m.devices().len(), 1);
        let dev = &m.devices()[0];
        assert_eq!(dev.bdf(), Bdf::new(0x65, 0, 0));
        assert_eq!(dev.ident.vendor_id, 0x10de);
        assert_eq!(dev.bars, alloc::vec![(0x3800_0000_0000, 128 * GB)]);
        // The console iGPU is untouched.
        assert!(!m.is_protected(Bdf::new(0, 2, 0)));
        assert!(m.is_protected(Bdf::new(0x65, 0, 0)));
    }

    #[test]
    fn test_protected_mmio_covers_bars_and_config() {
        let mut m = manager();
        m.set_config_pages(0xe000_0000);
        // Inside the H200's 128GB aperture.
        assert!(m.is_protected_mmio(0x3800_0000_0000));
        assert!(m.is_protected_mmio(0x381f_ffff_f000));
        assert!(!m.is_protected_mmio(0x3820_0000_0000));
        // The H200's ECAM config page.
        let cfg = 0xe000_0000 + Bdf::new(0x65, 0, 0).ecam_offset();
        assert!(m.is_protected_mmio(cfg));
        assert!(m.is_protected_mmio(cfg + 0xfff));
        assert!(!m.is_protected_mmio(cfg + 0x1000));
        // The iGPU's aperture and config page stay visible to the host.
        assert!(!m.is_protected_mmio(0x6000_0000));
        assert!(!m.is_protected_mmio(0xe000_0000 + Bdf::new(0, 2, 0).ecam_offset()));
    }

    #[test]
    fn test_domain_starts_fail_closed() {
        let m = manager();
        let bdf = Bdf::new(0x65, 0, 0);
        // Unassigned, and no address is reachable.
        let assigned = m.with_domain(bdf, |d| d.is_assigned()).unwrap();
        assert!(!assigned);
        let reachable = m.with_domain(bdf, |d| d.can_access(0x1000)).unwrap();
        assert!(!reachable, "an unassigned device must not reach any memory");
    }

    #[test]
    fn test_assign_rejects_unknown_and_double_assignment() {
        let m = manager();
        let bdf = Bdf::new(0x65, 0, 0);
        // The iGPU is not a protected device: it has no domain to assign.
        assert!(m.assign(Bdf::new(0, 2, 0), 1, &[]).is_err());
        // First assignment succeeds, the second is refused.
        assert!(m.assign(bdf, 0xcafe, &[]).is_ok());
        assert!(m.assign(bdf, 0xbeef, &[]).is_err());
        // After release it can be assigned again.
        assert!(m.release(bdf).is_ok());
        assert!(m.assign(bdf, 0xbeef, &[]).is_ok());
    }

    #[test]
    fn test_assign_rejects_misaligned_range() {
        let m = manager();
        let bdf = Bdf::new(0x65, 0, 0);
        // Unaligned start, and a size that is not a page multiple.
        assert!(m.assign(bdf, 1, &[(0x1001, PAGE_SIZE)]).is_err());
        assert!(m.assign(bdf, 1, &[(0x1000, 0x1)]).is_err());
        // A rejected assignment must leave the device unassigned, not
        // half-mapped.
        assert!(!m.with_domain(bdf, |d| d.is_assigned()).unwrap());
    }

    #[test]
    fn test_release_unknown_device_errors() {
        let m = manager();
        assert!(m.release(Bdf::new(0x01, 0, 0)).is_err());
    }
}
