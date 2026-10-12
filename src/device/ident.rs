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

//! Discrete accelerator identification.
//!
//! The monitor protects *discrete* compute accelerators (datacentre GPUs,
//! NPUs, DCUs). Integrated display controllers must never be caught: hiding
//! the iGPU that owns the console would black out the machine with no way to
//! read the failure. Classification therefore requires three independent
//! signals to agree (class, vendor, aperture size), and a build-time exclude
//! list overrides all of them.

use super::bdf::Bdf;

include!(concat!(env!("OUT_DIR"), "/device_policy.rs"));

/// PCI class codes (base class << 8 | subclass) that can front an
/// accelerator.
mod class {
    /// Display controller / VGA compatible.
    pub const VGA: u16 = 0x0300;
    /// Display controller / XGA.
    pub const XGA: u16 = 0x0301;
    /// Display controller / 3D controller. Datacentre GPUs with no display
    /// output report this (NVIDIA H100/H200, AMD Instinct).
    pub const DISPLAY_3D: u16 = 0x0302;
    /// Processing accelerator / processing accelerator. Ascend NPUs and most
    /// domestic AI accelerators report this.
    pub const PROCESSING_ACCEL: u16 = 0x1200;
    /// Processing accelerator / AI inference accelerator.
    pub const AI_INFERENCE: u16 = 0x1201;
}

/// PCI vendor IDs whose discrete accelerators the monitor protects.
mod vendor {
    /// NVIDIA (H100, H200, A100, ...).
    pub const NVIDIA: u16 = 0x10de;
    /// AMD (Instinct MI series).
    pub const AMD: u16 = 0x1002;
    /// Huawei (Ascend 910/310 NPUs).
    pub const HUAWEI: u16 = 0x19e5;
    /// Hygon (DCU, e.g. BW1000).
    pub const HYGON: u16 = 0x1d94;
    /// Chengdu Haiguang IC (DCU).
    pub const HAIGUANG: u16 = 0x1db7;
    /// Cambricon (MLU).
    pub const CAMBRICON: u16 = 0xcabc;
    /// Iluvatar CoreX.
    pub const ILUVATAR: u16 = 0x1e3e;
    /// Moore Threads.
    pub const MOORE_THREADS: u16 = 0x1ed5;
    /// Biren Technology.
    pub const BIREN: u16 = 0x1ee9;
    /// Enflame (CloudBlazer).
    pub const ENFLAME: u16 = 0x1e36;

    /// Intel is deliberately absent: its PCI-visible display controllers are
    /// integrated graphics driving the console. Intel discrete accelerators
    /// (Gaudi, Max) must be opted in by BDF via `HE_PROTECT_BDF`.
    pub const INTEL_EXCLUDED: u16 = 0x8086;

    pub const PROTECTED: &[u16] = &[
        NVIDIA,
        AMD,
        HUAWEI,
        HYGON,
        HAIGUANG,
        CAMBRICON,
        ILUVATAR,
        MOORE_THREADS,
        BIREN,
        ENFLAME,
    ];
}

/// Minimum size of the largest memory BAR for a device to count as a discrete
/// accelerator.
///
/// Discrete compute cards expose their HBM/GDDR aperture as a large BAR:
/// H200 has a 128GB BAR1 (with resizable-BAR enabled), Ascend 910B and DCU
/// BW1000 expose multi-GB apertures. Integrated graphics and small display
/// adapters stay far below this. The threshold is a lower bound chosen to
/// clear even a conservatively-sized aperture while excluding the 16-256MB
/// BARs typical of display-only devices.
pub const MIN_ACCELERATOR_BAR_SIZE: u64 = 256 * 1024 * 1024;

/// Why a device was or was not classified as a protected accelerator. Kept
/// for the boot log: on real hardware this is the only way to see why a card
/// was missed or why an unexpected device was caught.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Classification {
    /// Forced in by the `HE_PROTECT_BDF` build-time list.
    ForcedProtected,
    /// Forced out by the `HE_EXCLUDE_BDF` build-time list.
    ForcedExcluded,
    /// Class, vendor and aperture size all agree.
    Accelerator,
    /// Class code is not a display/accelerator class.
    NotAcceleratorClass,
    /// Vendor is not a known discrete-accelerator vendor (integrated
    /// graphics lands here).
    VendorNotProtected,
    /// Looks right but has no large aperture: a display-only adapter.
    ApertureTooSmall,
}

impl Classification {
    pub fn is_protected(self) -> bool {
        matches!(self, Self::ForcedProtected | Self::Accelerator)
    }
}

/// The identity of one PCI function, as read from its configuration header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceIdent {
    pub bdf: Bdf,
    pub vendor_id: u16,
    pub device_id: u16,
    /// Base class << 8 | subclass.
    pub class_code: u16,
    /// Size in bytes of the largest memory BAR.
    pub largest_bar_size: u64,
}

impl DeviceIdent {
    /// Classify this function. The build-time lists are consulted first so a
    /// real deployment can always correct a wrong automatic verdict without
    /// touching this logic.
    pub fn classify(&self) -> Classification {
        // Exclude wins over protect: it is the escape hatch that keeps a
        // machine bootable, so nothing may override it.
        if EXCLUDE_BDF.contains(&self.bdf.raw()) {
            return Classification::ForcedExcluded;
        }
        if PROTECT_BDF.contains(&self.bdf.raw()) {
            return Classification::ForcedProtected;
        }

        let class_ok = matches!(
            self.class_code,
            class::VGA
                | class::XGA
                | class::DISPLAY_3D
                | class::PROCESSING_ACCEL
                | class::AI_INFERENCE
        );
        if !class_ok {
            return Classification::NotAcceleratorClass;
        }
        if !vendor::PROTECTED.contains(&self.vendor_id) {
            return Classification::VendorNotProtected;
        }
        if self.largest_bar_size < MIN_ACCELERATOR_BAR_SIZE {
            return Classification::ApertureTooSmall;
        }
        Classification::Accelerator
    }

    pub fn is_protected(&self) -> bool {
        self.classify().is_protected()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ident(vendor_id: u16, device_id: u16, class_code: u16, bar: u64) -> DeviceIdent {
        DeviceIdent {
            bdf: Bdf::new(0x65, 0, 0),
            vendor_id,
            device_id,
            class_code,
            largest_bar_size: bar,
        }
    }

    const GB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn test_datacentre_accelerators_are_protected() {
        // NVIDIA H200 SXM: 3D controller, 128GB BAR1.
        let h200 = ident(vendor::NVIDIA, 0x2335, class::DISPLAY_3D, 128 * GB);
        assert_eq!(h200.classify(), Classification::Accelerator);
        // Huawei Ascend 910B: processing accelerator.
        let ascend = ident(vendor::HUAWEI, 0xd802, class::PROCESSING_ACCEL, 8 * GB);
        assert_eq!(ascend.classify(), Classification::Accelerator);
        // Hygon DCU BW1000: display controller class with a large aperture.
        let dcu = ident(vendor::HYGON, 0x6210, class::DISPLAY_3D, 16 * GB);
        assert_eq!(dcu.classify(), Classification::Accelerator);
        // Cambricon MLU.
        let mlu = ident(vendor::CAMBRICON, 0x0590, class::AI_INFERENCE, 4 * GB);
        assert!(mlu.is_protected());
    }

    /// The console-blackout regression: the integrated GPU of the host must
    /// never be hidden.
    #[test]
    fn test_integrated_graphics_is_never_protected() {
        // Intel Core Ultra 7 155H (Meteor Lake-P) integrated graphics.
        let igpu = ident(
            vendor::INTEL_EXCLUDED,
            0x7d55,
            class::VGA,
            256 * 1024 * 1024,
        );
        assert_eq!(igpu.classify(), Classification::VendorNotProtected);
        assert!(!igpu.is_protected());
        // Even with a huge reported aperture it stays out, because the
        // vendor check runs before the aperture check.
        let igpu_big = ident(vendor::INTEL_EXCLUDED, 0x7d55, class::DISPLAY_3D, 64 * GB);
        assert!(!igpu_big.is_protected());
    }

    #[test]
    fn test_class_match_alone_is_not_enough() {
        // A display-class device from an unknown vendor: not protected.
        let unknown = ident(0x1234, 0x5678, class::VGA, 64 * GB);
        assert_eq!(unknown.classify(), Classification::VendorNotProtected);
        // A protected vendor on a non-accelerator class: an audio function
        // of a GPU card lands here and must stay visible.
        let hdmi_audio = ident(vendor::NVIDIA, 0x22e8, 0x0403, 0);
        assert_eq!(hdmi_audio.classify(), Classification::NotAcceleratorClass);
        // A protected vendor, right class, but a display-sized aperture.
        let small = ident(vendor::AMD, 0x15d8, class::VGA, 16 * 1024 * 1024);
        assert_eq!(small.classify(), Classification::ApertureTooSmall);
    }

    #[test]
    fn test_aperture_threshold_boundary() {
        let at = ident(
            vendor::NVIDIA,
            0x2335,
            class::DISPLAY_3D,
            MIN_ACCELERATOR_BAR_SIZE,
        );
        assert_eq!(at.classify(), Classification::Accelerator);
        let below = ident(
            vendor::NVIDIA,
            0x2335,
            class::DISPLAY_3D,
            MIN_ACCELERATOR_BAR_SIZE - 1,
        );
        assert_eq!(below.classify(), Classification::ApertureTooSmall);
    }

    #[test]
    fn test_build_time_lists_are_empty_by_default() {
        // The default build must not hardcode any machine-specific BDF;
        // otherwise a stray entry would silently hide a device.
        assert!(PROTECT_BDF.is_empty());
        assert!(EXCLUDE_BDF.is_empty());
    }
}
