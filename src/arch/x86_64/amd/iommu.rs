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

use core::convert::TryFrom;
use core::fmt;

use bitflags::bitflags;
use spin::Mutex;

use crate::device::bdf::Bdf;
use crate::error::HvResult;
use crate::iommu::{GenericIommu, IommuInfo};
use crate::memory::addr::{phys_encrypted, phys_to_virt, GuestPhysAddr, HostPhysAddr};
use crate::memory::PagingResult;
use crate::memory::{EmptyPagingInstr, GenericPTE, GenericPageTableImmut, Level4PageTable};
use crate::memory::{Frame, MemFlags, Mmio, PageTableLevel, PAGE_SIZE};

const DEV_TABLE_SIZE: usize = 2 * 1024 * 1024; // 2M bytes
const DEV_TABLE_ENTRY_COUNT: usize = DEV_TABLE_SIZE / core::mem::size_of::<DevTableEntry>();

/// IOMMU MMIO Registers.
///
/// Reference: Sec 3.4, AMD I/O Virtualization Technology (IOMMU) Specification.
#[repr(C)]
struct IommuMmioRegion {
    /// ( 0h) Device Table Base Address Register.
    dev_table_base: Mmio<u64>,
    /// ( 8h) Command Buffer Base Address Register.
    cmd_buf_base: Mmio<u64>,
    /// (10h) Event Log Base Address Register.
    event_log_base: Mmio<u64>,
    /// (18h) IOMMU Control Register.
    control: Mmio<u64>,
    /// (20h) IOMMU Exclusion Base Register / Completion Store Base Register.
    excl_base: Mmio<u64>,
    /// (28h) IOMMU Exclusion Range Limit Register / Completion Store Limit Register.
    excl_range_limit: Mmio<u64>,
    /// (30h) IOMMU Extended Feature Register.
    ext_feature: Mmio<u64>,
    /// (38h) PPR Log Base Address Register.
    ppr_log_base: Mmio<u64>,
    /// (40h) IOMMU Hardware Event Upper Register.
    hw_event_upper: Mmio<u64>,
    /// (48h) IOMMU Hardware Event Lower Register.
    hw_event_lower: Mmio<u64>,
    /// (50h) IOMMU Hardware Event Status Register.
    hw_event_status: Mmio<u64>,
    /// (60h-D8h) IOMMU SMI Filter Register.
    smi_filter: [Mmio<u64>; 16],
    /// (E0h-1FF8h) Unused.
    _reserved: [u64; 996],
    /// (2000h) Command Buffer Head Pointer Register.
    cmd_buf_head: Mmio<u64>,
    /// (2008h) Command Buffer Tail Pointer Register.
    cmd_buf_tail: Mmio<u64>,
    /// (2010h) Event Log Head Pointer Register.
    event_log_head: Mmio<u64>,
    /// (2018h) Event Log Tail Pointer Register.
    event_log_tail: Mmio<u64>,
    /// (2020h) IOMMU Status Register.
    status: Mmio<u64>,
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct IommuControlFlags: u64 {
        /// IOMMU enable.
        const IOMMU_EN =        1 << 0;
        /// HyperTransport tunnel translation enable.
        const HT_TUN_EN =       1 << 1;
        /// Event log enable.
        const EVENT_LOG_EN =    1 << 2;
        /// Event log interrupt enable.
        const EVENT_INT_EN =    1 << 3;
        /// Command buffer enable.
        const CMD_BUF_EN =      1 << 12;
        /// Peripheral page request log enable.
        const PPR_LOG_EN =      1 << 13;
        /// Peripheral page request interrupt enable
        const PPR_INT_EN =      1 << 14;
        /// Peripheral page request processing enable.
        const PPR_EN =          1 << 15;
        /// Guest translation enable.
        const GT_EN =           1 << 16;
    }
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct IoPTFlags: u64 {
        /// Valid.
        const V =       1 << 0;
        /// Translate information valid. (Device table only)
        const TV =      1 << 1;
        /// Sets Access bits in the host page table corresponding to peripheral
        /// requests. (Device table only)
        const HA =      1 << 7;
        /// Sets Dirty bits in the host page table corresponding to peripheral
        /// requests. (Device table only)
        const HD =      1 << 8;
        /// PPR enable. (Device table only)
        const PPR =     1 << 52;
        /// Guest PPR response with PASID. (Device table only)
        const GPPR =    1 << 53;
        /// Guest I/O protection valid. (Device table only)
        const GIOV =    1 << 54;
        /// Guest translation valid. (Device table only)
        const GV =      1 << 55;
        /// I/O read permission.
        const IR =      1 << 61;
        /// I/O write permission.
        const IW =      1 << 62;
    }
}

impl From<MemFlags> for IoPTFlags {
    fn from(f: MemFlags) -> Self {
        let mut ret = Self::empty();
        if !f.contains(MemFlags::NO_PRESENT) {
            ret |= Self::V;
        }
        if f.contains(MemFlags::READ) {
            ret |= Self::IR;
        }
        if f.contains(MemFlags::WRITE) {
            ret |= Self::IW;
        }
        ret
    }
}

impl From<IoPTFlags> for MemFlags {
    fn from(f: IoPTFlags) -> Self {
        let mut ret = Self::empty();
        if !f.contains(IoPTFlags::V) {
            ret |= Self::NO_PRESENT;
        }
        if f.contains(IoPTFlags::IR) {
            ret |= Self::READ;
        }
        if f.contains(IoPTFlags::IW) {
            ret |= Self::WRITE;
        }
        ret
    }
}

#[derive(Clone)]
pub struct IoPTEntry(u64);

impl IoPTEntry {
    const NEXT_LEVEL_MASK: u64 = 0b111 << 9; // 9..11
    const ADDR_MASK: u64 = 0x000f_ffff_ffff_f000; // 12..52

    fn next_level(&self) -> Result<PageTableLevel, u8> {
        let l = ((self.0 & Self::NEXT_LEVEL_MASK) >> 9) as u8;
        PageTableLevel::try_from(l)
    }

    fn set_next_level(&mut self, next_level: PageTableLevel) {
        self.0 = (self.0 & !Self::NEXT_LEVEL_MASK) | (next_level as u64) << 9;
    }
}

impl GenericPTE for IoPTEntry {
    fn addr(&self) -> HostPhysAddr {
        (self.0 & Self::ADDR_MASK) as _
    }
    fn flags(&self) -> MemFlags {
        IoPTFlags::from_bits_truncate(self.0).into()
    }
    fn is_unused(&self) -> bool {
        self.0 == 0
    }
    fn is_present(&self) -> bool {
        (self.0 & IoPTFlags::V.bits()) != 0
    }
    fn is_leaf(&self) -> bool {
        self.next_level() == Ok(PageTableLevel::L0)
    }
    fn is_young(&self) -> bool {
        (self.0 & IoPTFlags::HA.bits()) != 0
    }
    fn set_old(&mut self) {
        let mut flags = IoPTFlags::from_bits_truncate(self.0);
        flags -= IoPTFlags::HA;
        self.0 = self.addr() as u64 | flags.bits();
    }
    fn set_addr(&mut self, paddr: HostPhysAddr) {
        self.0 = (self.0 & !Self::ADDR_MASK) | (paddr as u64 & Self::ADDR_MASK);
    }
    fn set_flags(&mut self, flags: MemFlags, _is_huge: bool) -> PagingResult {
        let flags = IoPTFlags::from(flags);
        self.0 = self.addr() as u64 | flags.bits();
        Ok(())
    }
    fn set_table(
        &mut self,
        paddr: HostPhysAddr,
        next_level: PageTableLevel,
        is_present: bool,
    ) -> PagingResult {
        let mut flags = IoPTFlags::IR | IoPTFlags::IW;
        if is_present {
            flags |= IoPTFlags::V;
        }
        self.0 = (paddr as u64 & Self::ADDR_MASK) | flags.bits();
        self.set_next_level(next_level);
        Ok(())
    }
    fn set_present(&mut self) -> PagingResult {
        self.0 |= IoPTFlags::V.bits();
        Ok(())
    }
    fn set_notpresent(&mut self) -> PagingResult {
        let mut flags = IoPTFlags::from_bits_truncate(self.0);
        flags -= IoPTFlags::V;
        self.0 = self.addr() as u64 | flags.bits();
        Ok(())
    }
    fn clear(&mut self) {
        self.0 = 0;
    }
}

impl fmt::Debug for IoPTEntry {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("IoPTEntry")
            .field("raw", &self.0)
            .field("addr", &self.addr())
            .field("flags", &self.flags())
            .field("next_level", &self.next_level())
            .finish()
    }
}

#[repr(C)]
struct DevTableEntry {
    raw64_0: IoPTEntry,
    raw64_1: u64,
    raw64_2: u64,
    raw64_3: u64,
}

impl DevTableEntry {
    fn flags(&self) -> IoPTFlags {
        IoPTFlags::from_bits_truncate(self.raw64_0.0)
    }

    fn mode(&self) -> Result<PageTableLevel, u8> {
        self.raw64_0.next_level()
    }

    fn table_root(&self) -> HostPhysAddr {
        self.raw64_0.addr()
    }

    fn set_table_root(&mut self, root_paddr: HostPhysAddr) {
        self.raw64_0.0 = (IoPTFlags::V | IoPTFlags::TV | IoPTFlags::IR | IoPTFlags::IW).bits();
        self.raw64_0.set_addr(root_paddr);
        self.raw64_0.set_next_level(PageTableLevel::L4); // 4 Level Page Table (provides a 48-bit GPA space)
    }

    /// DTE bits 79:64. The IOTLB is tagged by DomainID, so two devices with
    /// different page-table roots must never share one, or cached
    /// translations of one leak to the other.
    fn set_domain_id(&mut self, domain_id: u16) {
        self.raw64_1 = (self.raw64_1 & !0xffff) | domain_id as u64;
    }
}

impl fmt::Debug for DevTableEntry {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("DevTableEntry")
            .field(
                "raw",
                &[self.raw64_0.0, self.raw64_1, self.raw64_2, self.raw64_3],
            )
            .field("host_page_table_root", &self.table_root())
            .field("flags", &self.flags())
            .field("mode", &self.mode())
            .finish()
    }
}

/// Command buffer: one 4KB page = 256 entries of 16 bytes.
const CMD_BUF_SIZE: usize = PAGE_SIZE;
const CMD_ENTRY_SIZE: usize = 16;
/// Command Buffer Base Register ComLen (bits 59:56) = log2(entries).
/// 256 entries -> 8; values below 8 are reserved.
const CMD_BUF_COMLEN_256: u64 = 8 << 56;
/// Head/tail pointer registers hold a byte offset in bits 18:4.
const CMD_PTR_MASK: u64 = 0x0007_fff0;
/// Physical address bits 51:12 of a base register.
const BASE_ADDR_MASK: u64 = 0x000f_ffff_ffff_f000;

/// Command opcodes, placed in bits 63:60 of the first qword
/// (AMD IOMMU spec §2.4).
const CMD_COMPLETION_WAIT: u64 = 0x01;
const CMD_INVALIDATE_DEVTAB_ENTRY: u64 = 0x02;
const CMD_INVALIDATE_IOMMU_PAGES: u64 = 0x03;
/// COMPLETION_WAIT flag: store `StoreData` to `StoreAddr` when done.
const COMPLETION_WAIT_S: u64 = 1 << 0;
/// INVALIDATE_IOMMU_PAGES flags and the "whole address space" address
/// (S=1 with every address bit below 63 set).
const INV_PAGES_S: u64 = 1 << 0;
const INV_PAGES_PDE: u64 = 1 << 1;
const INV_PAGES_ALL_ADDR: u64 = 0x7fff_ffff_ffff_f000;

/// Hardware DomainID of the shared default domain (matches VT-d).
const DEFAULT_DOMAIN_ID: u16 = 1;
/// Bound on completion polling. A healthy IOMMU retires a handful of
/// commands in microseconds; failing beats hanging the whole machine.
const CMD_TIMEOUT_SPINS: usize = 100_000_000;

/// Build the 128-bit encodings as two qwords, kept pure for unit tests.
fn cmd_inv_devtab_entry(bdf: Bdf) -> [u64; 2] {
    [(CMD_INVALIDATE_DEVTAB_ENTRY << 60) | bdf.raw() as u64, 0]
}

fn cmd_inv_iommu_pages(domain_id: u16) -> [u64; 2] {
    [
        (CMD_INVALIDATE_IOMMU_PAGES << 60) | ((domain_id as u64) << 32),
        INV_PAGES_ALL_ADDR | INV_PAGES_PDE | INV_PAGES_S,
    ]
}

fn cmd_completion_wait(store_paddr: HostPhysAddr, data: u64) -> [u64; 2] {
    // StoreAddr[51:3] occupies the same bit positions in the first qword.
    let addr = store_paddr as u64 & 0x000f_ffff_ffff_fff8;
    [(CMD_COMPLETION_WAIT << 60) | addr | COMPLETION_WAIT_S, data]
}

struct IommuInner {
    regs: &'static mut IommuMmioRegion,
    dev_table_frame: Frame,
    cmd_buf_frame: Frame,
    /// Target of COMPLETION_WAIT stores; separate from the ring so the
    /// IOMMU's write can never land on a command slot.
    sem_frame: Frame,
    /// Value the next COMPLETION_WAIT stores; monotonic so a stale store
    /// can never satisfy a later wait.
    sem_seq: u64,
}

pub struct Iommu {
    inner: Mutex<IommuInner>,
}

impl IommuInner {
    fn dev_table_entries(&mut self) -> &mut [DevTableEntry] {
        let ptr = self.dev_table_frame.as_mut_ptr() as _;
        // SAFETY: the 2MB device table frame is owned by this IOMMU and holds
        // exactly DEV_TABLE_ENTRY_COUNT entries; `&mut self` (under the
        // mutex) makes the borrow exclusive.
        unsafe { core::slice::from_raw_parts_mut(ptr, DEV_TABLE_ENTRY_COUNT) }
    }

    fn is_enabled(&self) -> bool {
        self.regs.control.read() & IommuControlFlags::IOMMU_EN.bits() != 0
    }

    /// Append one command at the tail and hand it to the hardware. Callers
    /// always finish a batch with `completion_wait`, which drains the ring,
    /// so a batch of a few commands can never overrun the head.
    fn push_cmd(&mut self, cmd: [u64; 2]) {
        let tail = (self.regs.cmd_buf_tail.read() & CMD_PTR_MASK) as usize;
        let slot = self.cmd_buf_frame.as_mut_ptr() as *mut u64;
        // SAFETY: `tail` comes from the hardware's 18:4 field, so it is
        // 16-byte aligned; the ring is CMD_BUF_SIZE long and the hardware
        // wraps the pointer within it, so both qwords stay in the frame.
        unsafe {
            slot.add(tail / 8).write_volatile(cmd[0]);
            slot.add(tail / 8 + 1).write_volatile(cmd[1]);
        }
        let next = (tail + CMD_ENTRY_SIZE) % CMD_BUF_SIZE;
        self.regs.cmd_buf_tail.write(next as u64);
    }

    /// Wait until every previously pushed command has completed.
    fn completion_wait(&mut self) -> HvResult {
        self.sem_seq = self.sem_seq.wrapping_add(1);
        let seq = self.sem_seq;
        let sem = self.sem_frame.as_mut_ptr() as *mut u64;
        // SAFETY: `sem_frame` is a whole frame owned by this IOMMU; the
        // IOMMU is the only other writer, and only via this command.
        unsafe { sem.write_volatile(0) };
        let store = phys_encrypted(self.sem_frame.start_paddr());
        self.push_cmd(cmd_completion_wait(store, seq));
        for _ in 0..CMD_TIMEOUT_SPINS {
            // SAFETY: as above.
            if unsafe { sem.read_volatile() } == seq {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        hv_result_err!(EIO, "AMD IOMMU COMPLETION_WAIT timed out")
    }
}

impl Iommu {
    pub fn new(info: &IommuInfo) -> HvResult<Self> {
        let iommu_base = phys_to_virt(info.base as HostPhysAddr);
        let regs: &mut IommuMmioRegion = unsafe { Mmio::<u64>::from_base_as(iommu_base) };

        let mut dev_table_frame = Frame::new_contiguous(DEV_TABLE_SIZE / PAGE_SIZE, 12)?;
        dev_table_frame.zero();
        let dev_table_base = phys_encrypted(dev_table_frame.start_paddr()) | 0x1FF;
        regs.dev_table_base.write(dev_table_base as u64);

        // The IOMMU fetches commands with the C-bit the base register
        // carries, so it must match how the monitor writes them under SME.
        let cmd_buf_frame = Frame::new_zero()?;
        let cmd_base = phys_encrypted(cmd_buf_frame.start_paddr()) as u64 & BASE_ADDR_MASK;
        regs.cmd_buf_base.write(cmd_base | CMD_BUF_COMLEN_256);
        regs.cmd_buf_head.write(0);
        regs.cmd_buf_tail.write(0);
        let sem_frame = Frame::new_zero()?;

        Ok(Self {
            inner: Mutex::new(IommuInner {
                regs,
                dev_table_frame,
                cmd_buf_frame,
                sem_frame,
                sem_seq: 0,
            }),
        })
    }
}

impl GenericIommu for Iommu {
    fn set_io_page_table(&self, pt: &IoPageTable) -> HvResult {
        let mut inner = self.inner.lock();
        for entry in inner.dev_table_entries() {
            entry.set_table_root(pt.root_paddr());
            entry.set_domain_id(DEFAULT_DOMAIN_ID);
        }
        Ok(())
    }

    fn set_device_page_table(&self, bdf: Bdf, pt: &IoPageTable, domain_id: u16) -> HvResult {
        if domain_id == DEFAULT_DOMAIN_ID {
            return hv_result_err!(EINVAL, "private domain id collides with the default domain");
        }
        let mut inner = self.inner.lock();
        let entry = &mut inner.dev_table_entries()[bdf.raw() as usize];
        entry.set_table_root(pt.root_paddr());
        entry.set_domain_id(domain_id);

        // The IOMMU processes commands only while enabled. At boot this
        // runs before `set_enabled(true)`, so nothing is cached yet and
        // there is nothing to flush; at runtime the cached DTE and the
        // domain's IOTLB entries must go before the new root is trusted.
        if inner.is_enabled() {
            inner.push_cmd(cmd_inv_devtab_entry(bdf));
            inner.push_cmd(cmd_inv_iommu_pages(domain_id));
            inner.completion_wait()?;
        }
        Ok(())
    }

    fn set_enabled(&self, enabled: bool) -> HvResult {
        // Written whole, not read-modified: stale firmware bits (e.g. an event
        // log enable with no log programmed) must not survive.
        let flags = if enabled {
            IommuControlFlags::IOMMU_EN | IommuControlFlags::CMD_BUF_EN
        } else {
            IommuControlFlags::empty()
        };
        self.inner.lock().regs.control.write(flags.bits());
        Ok(())
    }
}

pub type IoPageTable = Level4PageTable<GuestPhysAddr, IoPTEntry, EmptyPagingInstr>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inv_devtab_entry_encoding() {
        let cmd = cmd_inv_devtab_entry(Bdf::new(0x65, 0, 0));
        // Opcode 02h in bits 63:60, DeviceID in bits 15:0.
        assert_eq!(cmd, [0x2000_0000_0000_6500, 0]);
    }

    #[test]
    fn test_inv_iommu_pages_encoding() {
        let cmd = cmd_inv_iommu_pages(16);
        // Opcode 03h, DomainID in bits 47:32, PASID 0.
        assert_eq!(cmd[0], 0x3000_0010_0000_0000);
        // Whole address space: S=1, PDE=1, address 7FFF_FFFF_FFFF_F000.
        assert_eq!(cmd[1], 0x7fff_ffff_ffff_f003);
    }

    #[test]
    fn test_completion_wait_encoding() {
        let cmd = cmd_completion_wait(0x0000_8012_3456_7008, 42);
        // Opcode 01h, StoreAddr[51:3] in place, S=1.
        assert_eq!(cmd[0], 0x1000_8012_3456_7009);
        assert_eq!(cmd[1], 42);
        // Low address bits never leak into the flag field.
        assert_eq!(cmd_completion_wait(0x1007, 0)[0] & 0x7, 1);
    }

    #[test]
    fn test_comlen_encoding() {
        // 256 entries of 16 bytes fill exactly one page.
        assert_eq!(
            CMD_BUF_SIZE / CMD_ENTRY_SIZE,
            1 << (CMD_BUF_COMLEN_256 >> 56)
        );
    }
}
