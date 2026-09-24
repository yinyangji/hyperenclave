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

//! MSR virtualization subsystem (vendor-independent core).
//!
//! Both vendor exit paths (Intel `MSR_READ`/`MSR_WRITE`, AMD `MSR` with
//! `exit_info_1` 0/1) funnel into `handle_rdmsr`/`handle_wrmsr`, which
//! dispatch on the shared policy table. The interception maps are generated
//! from the very same table, so a `Passthrough` MSR never traps and a
//! `Deny`/unknown MSR is always rejected with #GP (fail-closed).

pub mod emul;
pub mod policy;

pub use emul::VcpuMsrState;
pub use policy::{MsrAction, MsrEntry};

use x86::msr::{rdmsr, wrmsr};

use crate::arch::vmm::VcpuAccessGuestState;
use crate::error::HvResult;
use crate::percpu::PerCpu;

/// RDMSR and WRMSR are both 2-byte instructions.
const VM_EXIT_LEN_MSR: u8 = 2;

/// Handle an RDMSR exit. On success the result is written to
/// RAX/EDX:EAX-style (low 32 bits in RAX, high 32 bits in RDX) and RIP is
/// advanced. A denied access injects #GP and leaves RIP untouched so the
/// guest sees the fault on the RDMSR itself.
pub fn handle_rdmsr(cpu: &mut PerCpu) -> HvResult {
    let id = cpu.vcpu.regs().rcx as u32;
    let (action, name) = match policy::policy_lookup(id) {
        Some(entry) => (entry.read, entry.name),
        None => (MsrAction::Deny, "unknown MSR"),
    };

    match action {
        // Bitmaps never trap passthrough MSRs; if one still arrives
        // (e.g. a future out-of-coverage entry), behave like a proxy.
        MsrAction::Passthrough | MsrAction::Proxy => {
            let value = unsafe { rdmsr(id) };
            if action == MsrAction::Proxy {
                debug!("RDMSR({:#x} {}) proxied: {:#x}", id, name, value);
            }
            write_msr_result(cpu, value);
        }
        MsrAction::Emulate => {
            let value = cpu.msr_state.read(id);
            write_msr_result(cpu, value);
        }
        MsrAction::AreaSwap => {
            let value = cpu.vcpu.rdmsr_virt(id).ok_or_else(|| {
                hv_err!(
                    EINVAL,
                    format!("no virtualized backing for MSR {:#x} ({})", id, name)
                )
            })?;
            write_msr_result(cpu, value);
        }
        MsrAction::Deny => {
            error!("RDMSR({:#x} {}) denied by policy", id, name);
            crate::arch::POLICY_STATS.record_msr_deny();
            cpu.vcpu.inject_fault()?;
            return Ok(());
        }
    }

    cpu.vcpu.advance_rip(VM_EXIT_LEN_MSR)
}

/// Handle a WRMSR exit. Semantics mirror `handle_rdmsr`.
pub fn handle_wrmsr(cpu: &mut PerCpu) -> HvResult {
    let (id, value) = {
        let regs = cpu.vcpu.regs();
        let id = regs.rcx as u32;
        let value = (regs.rax as u32 as u64) | ((regs.rdx as u32 as u64) << 32);
        (id, value)
    };
    let (action, name) = match policy::policy_lookup(id) {
        Some(entry) => (entry.write, entry.name),
        None => (MsrAction::Deny, "unknown MSR"),
    };

    match action {
        MsrAction::Passthrough => unsafe { wrmsr(id, value) },
        MsrAction::Proxy => {
            info!("WRMSR({:#x} {}) proxied <- {:#x}", id, name, value);
            unsafe { wrmsr(id, value) };
        }
        MsrAction::Emulate => cpu.msr_state.write(id, value)?,
        MsrAction::AreaSwap => cpu.vcpu.wrmsr_virt(id, value)?,
        MsrAction::Deny => {
            error!("WRMSR({:#x} {}) denied by policy", id, name);
            crate::arch::POLICY_STATS.record_msr_deny();
            cpu.vcpu.inject_fault()?;
            return Ok(());
        }
    }

    cpu.vcpu.advance_rip(VM_EXIT_LEN_MSR)
}

/// Write a 64-bit RDMSR result into the guest RAX/RDX (32 bits each).
fn write_msr_result(cpu: &mut PerCpu, value: u64) {
    let regs = cpu.vcpu.regs_mut();
    regs.rax = value as u32 as u64;
    regs.rdx = (value >> 32) as u32 as u64;
}
