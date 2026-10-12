---
name: build-verify
description: Runs the HyperEnclave quality gates (rustfmt, clippy, host unit tests, ELF build) for both Intel and AMD configurations and interprets the results. Use after any code change in this repo, before committing, or when the user asks to build, check, lint or test the hypervisor.
---

# Build and verify

Every code change must pass all gates for **both** vendors. A change that only
builds for one vendor is not done (see AGENTS.md §2.5).

## Steps

```bash
make format                               # rewrite formatting first
make format-check                         # must print nothing
make clippy VENDOR=intel SME=off
make clippy VENDOR=amd   SME=on           # SME=on is only legal with VENDOR=amd
make test                                 # host-side cargo test
make elf VENDOR=intel SME=off LOG=warn    # there is no `build` target
make elf VENDOR=amd   SME=on  LOG=warn
```

Run the two `make elf` builds even if clippy passed: feature-gated code
(`#[cfg(feature = "intel")]` / `"amd"` / `"sme"`) is only type-checked by the
configuration that enables it.

## Pass criteria

- Zero errors in every step.
- No **new** clippy warnings in files you touched. The tree has pre-existing
  warnings (e.g. `crates/yogcrypt`, unused `hypercall_hv_err` imports); do not
  fix unrelated ones in the same change, and do not count them against it.
- New unit tests actually ran: check `cargo test -- --list` if a module's tests
  are missing from the count (vendor modules appear as
  `arch::vmm::vendor::<module>::tests`).

## When a gate cannot run

- `could not parse override file: .../rust-toolchain`: the toolchain file is
  malformed. Do **not** edit it (AGENTS.md §7). Tell the user, and run the
  gates with `RUSTUP_TOOLCHAIN=nightly-2025-11-05` set for the command only.
- Missing `rust-src`: `rustup component add rust-src --toolchain nightly-2025-11-05`.
- Never pass `--no-verify` to get past the pre-commit hook; fix the format
  error instead.

## What host tests cannot show

`make test` exercises pure logic only (encodings, decoders, policy tables).
IOMMU programming, PCI enumeration, VM exits and anything touching real MMIO
need a physical Intel or AMD machine (see the `debug-on-hardware` skill). Say
so explicitly when reporting results.
