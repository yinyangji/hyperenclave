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

use libvmm::svm::flags::{InterruptType, VmcbCleanBits, VmcbIntInfo};
use libvmm::svm::{NptViolationInfo, SvmExitCode, VmExitInfo};

use crate::arch::vmm::{VcpuAccessGuestState, VmExit};
use crate::arch::{EnclaveExceptionInfo, ExceptionType};
use crate::enclave::{AexException, EnclaveStatsId};
use crate::error::HvResult;
use crate::percpu::CpuState;
use crate::stats::Instant;

impl VmExit<'_> {
    fn handle_nmi(&mut self) -> HvResult {
        unsafe { core::arch::asm!("cli; stgi; clgi; sti") };
        Ok(())
    }

    fn handle_exception(&mut self, vec: u8, exit_info: &VmExitInfo) -> HvResult {
        info!(
            "#VMEXIT(EXCP {}) @ RIP({:#x}): {:#x?}",
            vec, exit_info.guest_rip, exit_info
        );

        let error_code = if ExceptionType::has_error_code(vec) {
            Some(exit_info.exit_info_1 as u32)
        } else {
            None
        };
        let fault_gvaddr = if vec == ExceptionType::PageFault {
            Some(exit_info.exit_info_2 as usize)
        } else {
            None
        };

        let enclave = self.cpu_data.get_current_enclave()?;
        if let Some(exception_info) = enclave.fixup_exception(vec, error_code, fault_gvaddr)? {
            self.inject_exception(exception_info)
        } else {
            Ok(())
        }
    }

    pub fn inject_exception(&mut self, enclave_exception: EnclaveExceptionInfo) -> HvResult {
        let now = Instant::now();

        // Write the exception information to VMCB.
        {
            let linux_info = enclave_exception.linux_info;
            let error_code = if ExceptionType::has_error_code(linux_info.exception_type) {
                match linux_info.error_code {
                    Some(error_code) => error_code,
                    None => {
                        error!(
                            "VmExit::inject_exception({:#x?}): error_code is None",
                            enclave_exception
                        );
                        return hv_result_err!(EINVAL);
                    }
                }
            } else {
                // Set to default value
                0
            };

            if linux_info.exception_type == ExceptionType::PageFault {
                let cr2 = match linux_info.cr2 {
                    Some(cr2) => cr2,
                    None => {
                        error!(
                            "VmExit::inject_exception({:#x?}): CR2 is None for #PF",
                            enclave_exception
                        );
                        return hv_result_err!(EINVAL);
                    }
                };
                self.cpu_data.vcpu.vmcb.save.cr2 = cr2;
                self.cpu_data.vcpu.vmcb.control.clean_bits -= VmcbCleanBits::CR2;
            }

            self.cpu_data.vcpu.vmcb.inject_event(
                VmcbIntInfo::from(InterruptType::Exception, linux_info.exception_type),
                error_code,
            );
        }

        let aex_excep = if let Some(aex_excep) = enclave_exception.aex_excep {
            aex_excep
        } else {
            // In non-enclave mode
            return Ok(());
        };

        // In enclave mode
        if self.cpu_data.state == CpuState::EnclaveRunning {
            match self.cpu_data.enclave_aex(aex_excep) {
                Ok(enclave) => enclave.atomic_add_stats(EnclaveStatsId::Aex, now.elapsed()),
                Err(e) => {
                    warn!("Enclave AEX failed!: {:x?}", e);
                    self.cpu_data.fault()?;
                }
            }
        } else {
            error!(
                "handle_exception cpu state {:?} is wrong",
                self.cpu_data.state
            );
            return hv_result_err!(EINVAL);
        }

        Ok(())
    }

    fn handle_interrupt(&mut self, exit_info: &VmExitInfo) -> HvResult {
        let now = Instant::now();

        debug!(
            "#VMEXIT(INTR) @ RIP({:#x}): {:#x?}",
            exit_info.guest_rip, exit_info,
        );

        if self.cpu_data.state == CpuState::EnclaveRunning {
            match self.cpu_data.enclave_aex(AexException {
                vec: ExceptionType::IrqStart,
                misc: None,
            }) {
                Ok(enclave) => enclave.atomic_add_stats(EnclaveStatsId::Aex, now.elapsed()),
                Err(e) => {
                    warn!("Enclave AEX failed!: {:x?}", e);
                    self.cpu_data.fault()?;
                }
            }
        } else {
            error!(
                "handle_interrupt cpu state {:?} is wrong",
                self.cpu_data.state
            );
            return hv_result_err!(EINVAL);
        }
        Ok(())
    }

    fn handle_nested_page_fault(&mut self, exit_info: &VmExitInfo) -> HvResult {
        let npt_vio_info = NptViolationInfo::from_exit_info(exit_info);

        // Check if we're running L2 (nested mode).
        #[cfg(feature = "nested")]
        if self.cpu_data.vcpu.nested.in_l2 {
            return self.handle_l2_nested_page_fault(npt_vio_info);
        }

        // L1 nested page fault (normal enclave path).
        if self.cpu_data.state == CpuState::EnclaveRunning {
            let enclave = self.cpu_data.get_current_enclave()?;
            enclave
                .handle_npt_violation(npt_vio_info.guest_paddr, npt_vio_info.final_translation)?;
            return Ok(());
        }
        warn!(
            "#VMEXIT(NPF) @ {:#x?} RIP({:#x?})",
            npt_vio_info, exit_info.guest_rip,
        );
        hv_result_err!(ENOSYS)
    }

    /// Handle L2 nested page fault: synthesize an entry in the nNPT.
    #[cfg(feature = "nested")]
    fn handle_l2_nested_page_fault(&mut self, npt_vio_info: NptViolationInfo) -> HvResult {
        use crate::memory::addr::GuestPhysAddr;

        let l2gpa = GuestPhysAddr::from(npt_vio_info.guest_paddr);
        let is_write = npt_vio_info.final_translation & 0x2 != 0; // NPT violation bit 1 = write

        // Get L1's nCR3 from the current VMCB02.
        let vmcb12_gpa = self.cpu_data.vcpu.nested.vmcb12_gpa;

        // Access VMCB12 to get L1's nCR3.
        let l1_ncr3 =
            super::nested::with_vmcb12(vmcb12_gpa, |vmcb12| Ok(vmcb12.control.nest_cr3))??;

        // Get or create the nNPT for this L1 nCR3.
        let nnpt = self
            .cpu_data
            .vcpu
            .nested
            .nnpt_cache
            .get_or_create(l1_ncr3)?;

        // Handle the page fault: walk L1 nNPT + L0 GPM, synthesize entry.
        match nnpt.handle_nested_page_fault(l2gpa, l1_ncr3, is_write) {
            Ok(()) => {
                debug!(
                    "L2 #NPF resolved: L2GPA={:#x} L1 nCR3={:#x}",
                    l2gpa, l1_ncr3
                );
                Ok(())
            }
            Err(e) if e.code() == EFAULT || e.code() == EACCES => {
                // L1's nNPT is missing or permissions deny the access.
                // Reflect the #NPF back to L1 by triggering normal exit reflection.
                warn!(
                    "L2 #NPF cannot be resolved (L1 nNPT issue), reflecting to L1: {:?}",
                    e
                );

                // The exit reflection will happen in handle_exit() after we return this error.
                // Set a flag so handle_exit knows this NPF needs reflection.
                Err(e)
            }
            Err(e) => Err(e),
        }
    }

    pub fn handle_exit(&mut self) -> HvResult {
        let vcpu = &mut self.cpu_data.vcpu;
        vcpu.regs_mut().rax = vcpu.vmcb.save.rax;

        // All guest state is marked unmodified; individual handlers must clear
        // the bits as needed.
        vcpu.vmcb.control.clean_bits = VmcbCleanBits::UNMODIFIED;

        let exit_info = VmExitInfo::new(&vcpu.vmcb);
        let exit_code = match exit_info.exit_code {
            Ok(code) => code,
            Err(code) => {
                error!("Unknown #VMEXIT exit code: {:#x}", code);
                return hv_result_err!(EIO);
            }
        };

        // Check if we're running L2 (nested mode).
        #[cfg(feature = "nested")]
        let in_l2 = self.cpu_data.vcpu.nested.in_l2;
        #[cfg(not(feature = "nested"))]
        let in_l2 = false;

        // If in L2, apply L0 policy before deciding whether to reflect.
        #[cfg(feature = "nested")]
        if in_l2 {
            // For certain exits, apply L0 policy modifications before reflection.
            match exit_code {
                SvmExitCode::CPUID => {
                    // Apply L0's CPUID policy (mask SVM/SME bits, etc.)
                    self.apply_l2_cpuid_policy(&exit_info)?;
                }
                SvmExitCode::MSR => {
                    // Check L0's MSR policy (block VM_HSAVE_PA, VM_CR writes, etc.)
                    if let Err(e) = self.check_l2_msr_policy(&exit_info) {
                        warn!("L2 MSR access blocked by L0 policy: {:?}", e);
                        // Inject #GP(0) to L2.
                        self.cpu_data.vcpu.vmcb.inject_event(
                            libvmm::svm::VmcbIntInfo::from(
                                libvmm::svm::InterruptType::Exception,
                                13, // #GP
                            ),
                            Some(0),
                        );
                        return Ok(());
                    }
                }
                SvmExitCode::IOIO => {
                    // Check L0's I/O policy.
                    if let Err(e) = self.check_l2_ioio_policy(&exit_info) {
                        warn!("L2 I/O access blocked by L0 policy: {:?}", e);
                        // Inject #GP(0) to L2.
                        self.cpu_data.vcpu.vmcb.inject_event(
                            libvmm::svm::VmcbIntInfo::from(
                                libvmm::svm::InterruptType::Exception,
                                13,
                            ),
                            Some(0),
                        );
                        return Ok(());
                    }
                }
                _ => {}
            }
        }

        // If in L2, check if L0 should handle this exit or reflect to L1.
        #[cfg(feature = "nested")]
        if in_l2 && !super::exit_reflection::should_l0_handle(exit_code) {
            // Reflect this exit to L1.
            let vmcb12_gpa = self.cpu_data.vcpu.nested.vmcb12_gpa;
            let vmcb02 = &self.cpu_data.vcpu.vmcb;

            // Get the backed-up VMCB01 (L1's state before VMRUN).
            let vmcb01 = self
                .cpu_data
                .vcpu
                .nested
                .vmcb01_backup
                .as_ref()
                .ok_or(hv_result_err!(EIO, "VMCB01 backup missing"))?;

            super::exit_reflection::reflect_exit_to_l1(vmcb02, vmcb12_gpa, vmcb01)?;

            // Restore VMCB01 to continue running L1.
            self.cpu_data.vcpu.vmcb = *vmcb01.clone();

            // Mark that we're no longer in L2.
            self.cpu_data.vcpu.nested.in_l2 = false;

            debug!(
                "Reflected L2 exit to L1: exit_code={:#x}",
                exit_info.exit_code
            );
            return Ok(());
        }

        let res = match exit_code {
            SvmExitCode::INVALID => panic!("VM entry failed: {:#x?}\n{:#x?}", exit_info, vcpu.vmcb),
            SvmExitCode::EXCP(vec) => self.handle_exception(vec, &exit_info),
            SvmExitCode::INTR => self.handle_interrupt(&exit_info),
            SvmExitCode::NMI => self.handle_nmi(),
            SvmExitCode::CPUID => self.handle_cpuid(),
            SvmExitCode::VMMCALL => self.handle_hypercall(),
            SvmExitCode::NPF => self.handle_nested_page_fault(&exit_info),
            SvmExitCode::MSR => match exit_info.exit_info_1 {
                0 => self.handle_msr_read(),
                1 => self.handle_msr_write(),
                _ => hv_result_err!(EIO),
            },
            #[cfg(feature = "nested")]
            SvmExitCode::VMRUN => {
                let res = self.cpu_data.vcpu.handle_vmrun();
                if res.is_ok() {
                    self.advance_rip()?;
                }
                res
            }
            #[cfg(feature = "nested")]
            SvmExitCode::VMLOAD => {
                let res = self.cpu_data.vcpu.handle_vmload();
                if res.is_ok() {
                    self.advance_rip()?;
                }
                res
            }
            #[cfg(feature = "nested")]
            SvmExitCode::VMSAVE => {
                let res = self.cpu_data.vcpu.handle_vmsave();
                if res.is_ok() {
                    self.advance_rip()?;
                }
                res
            }
            #[cfg(feature = "nested")]
            SvmExitCode::STGI => {
                let res = self.cpu_data.vcpu.handle_stgi();
                if res.is_ok() {
                    self.advance_rip()?;
                }
                res
            }
            #[cfg(feature = "nested")]
            SvmExitCode::CLGI => {
                let res = self.cpu_data.vcpu.handle_clgi();
                if res.is_ok() {
                    self.advance_rip()?;
                }
                res
            }
            SvmExitCode::IOIO => {
                // EXITINFO1 (SVM Ref. Manual §2): bits 31:16 = port number,
                // bit 0 = TYPE (1 = IN, 0 = OUT), bit 4 = SZ8, bit 5 = SZ16,
                // bit 6 = SZ32 (exactly one is set). EXITINFO2 is the RIP
                // that follows the instruction, so the length is the
                // difference.
                let port = (exit_info.exit_info_1 >> 16) as u16;
                let is_read = exit_info.exit_info_1 & 1 != 0;
                let size = if exit_info.exit_info_1 & (1 << 4) != 0 {
                    1
                } else if exit_info.exit_info_1 & (1 << 5) != 0 {
                    2
                } else {
                    4
                };
                let instr_len = (exit_info.exit_info_2 - exit_info.guest_rip) as u8;
                self.handle_pio(port, is_read, size, instr_len)
            }
            SvmExitCode::SHUTDOWN => {
                error!("#VMEXIT(SHUTDOWN): {:#x?}", exit_info);
                self.cpu_data.vcpu.inject_fault()?;
                Ok(())
            }
            _ => hv_result_err!(ENOSYS),
        };

        let vcpu = &mut self.cpu_data.vcpu;
        if res.is_err() {
            warn!(
                "#VMEXIT handler returned {:?}:\n\
                {:#x?}\n\n\
                Guest State Dump:\n\
                {:#x?}",
                res, exit_info, vcpu,
            );
        }
        vcpu.vmcb.save.rax = vcpu.regs().rax;
        res
    }
}

/// Apply L0's CPUID policy to L2's CPUID result before reflecting to L1.
#[cfg(feature = "nested")]
fn apply_l2_cpuid_policy(&mut self, _exit_info: &VmExitInfo) -> HvResult {
    let vcpu = &mut self.cpu_data.vcpu;

    let leaf = vcpu.regs().rax as u32;
    let subleaf = vcpu.regs().rcx as u32;

    // Execute CPUID to get the raw result.
    let cpuid = unsafe { core::arch::x86_64::__cpuid_count(leaf, subleaf) };
    let mut eax = cpuid.eax;
    let mut ebx = cpuid.ebx;
    let mut ecx = cpuid.ecx;
    let mut edx = cpuid.edx;

    // Apply L0's policy (mask SVM/SME bits, etc.)
    super::l2_policy::apply_cpuid_policy(leaf, subleaf, &mut eax, &mut ebx, &mut ecx, &mut edx)?;

    // Write the masked result back to VMCB (L1 will see this).
    vcpu.vmcb.save.rax = eax as u64;
    vcpu.regs_mut().rbx = ebx as u64;
    vcpu.regs_mut().rcx = ecx as u64;
    vcpu.regs_mut().rdx = edx as u64;

    Ok(())
}

/// Check L0's MSR policy for L2's MSR access before reflecting to L1.
#[cfg(feature = "nested")]
fn check_l2_msr_policy(&self, exit_info: &VmExitInfo) -> HvResult {
    let is_write = exit_info.exit_info_1 == 1;
    let msr = self.cpu_data.vcpu.regs().rcx as u32;

    super::l2_policy::check_msr_policy(msr, is_write)?;
    Ok(())
}

/// Check L0's I/O policy for L2's port access before reflecting to L1.
#[cfg(feature = "nested")]
fn check_l2_ioio_policy(&self, exit_info: &VmExitInfo) -> HvResult {
    let info = exit_info.exit_info_1;
    let port = ((info >> 16) & 0xFFFF) as u16;
    let size = ((info & 0x70) >> 4) as u8;
    let is_write = (info & 0x1) != 0;

    super::l2_policy::check_ioio_policy(port, size, is_write)?;
    Ok(())
}
