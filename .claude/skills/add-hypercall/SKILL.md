---
name: add-hypercall
description: Step-by-step procedure for adding or changing a HyperEnclave hypercall (code allocation, privilege level, CPU-state validation, dispatch, guest-pointer handling, error returns, ABI layout tests). Use when adding a new VMCALL/VMMCALL interface, changing hypercall arguments or return values, or touching src/hypercall/.
---

# Add a hypercall

Reference implementations: `get_policy_stats` (simple export) and
`src/hypercall/device.rs` (struct-based ABI with errno returns).

## 1. Allocate the code — `src/hypercall/mod.rs`

Add a variant to `HyperCallCode` inside `numeric_enum!`:

- **Supervisor** (driver / host kernel only): `< 0x4000_0000`.
- **User** (enclave / user mode): `>= 0x8000_0000`.

Bits 31:30 decide the privilege level and the dispatcher rejects a mismatch,
so the range is a security property, not a convention. Group related calls
(e.g. devices use `0x50..`) and do not reuse a retired code.

## 2. Register valid CPU states — `validate_state()`

Add the variant to the match arm for the states it may run in, usually
`CpuState::HvEnabled`. The match is exhaustive on purpose: forgetting this step
is a compile error, not a silent allow.

## 3. Dispatch — `HyperCall::hypercall()`

```rust
HyperCallCode::FooBar => self.foo_bar(arg0.as_guest_ptr_ns(&self.gpt, guest_privilege_level)),
```

Put the handler in a submodule (`impl HyperCall<'_>` with `pub(super) fn`).

## 4. Guest memory: always through `GuestPtr`

Never turn a guest-supplied address into a host pointer yourself.
`as_guest_ptr_ns` + `read()` / `write()` walks the guest page table, checks
permissions, and refuses EPC or non-normal-world GPAs. Only fixed-size
`#[repr(C)]` types; variable-length data uses a bounded array plus a count
field, with the bound checked before use.

Raw physical addresses passed as *values* (e.g. a DMA target) are untrusted
too: validate the whole range, not only its first page (see
`Cell::is_normal_world_dma_range`).

## 5. Errors

- Expected, caller-correctable failures (busy, not found, bad argument) for a
  Supervisor call: return a negative errno in the return register
  (`Ok(err.num().code() as isize as usize)`), as `device.rs` does. Returning
  `Err(HvError)` makes the dispatcher inject #GP into the host kernel.
- Monitor-internal failures: `hypercall_hv_err_result!(ERRNO, msg)`.
- No `unwrap()` / `expect()` / `panic!()` (AGENTS.md §2.3).

## 6. ABI tests

Structs shared with the out-of-tree `hyperenclave-driver` get a unit test
pinning `size_of` (and offsets if relevant). Tell the user the driver must be
updated to the same layout; it is not in this repo.

## 7. Verify

Run the `build-verify` skill. Update the hypercall table in the relevant
design doc under `docs/`.
