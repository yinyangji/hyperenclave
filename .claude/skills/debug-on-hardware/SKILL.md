---
name: debug-on-hardware
description: Diagnoses HyperEnclave on real Intel/AMD machines (boot/activation failures, GRUB memmap, dmesg logs, version mismatch with libtpm, disassembly, checking that accelerator isolation actually works). Use when the user reports a crash, hang or unexpected behaviour on hardware, or asks how to validate a change on a physical machine.
---

# Debug on hardware

Nothing here can be done from the dev machine alone. Give the user exact
commands to run and ask for the output; do not claim hardware results you have
not seen.

## Logs

- Monitor logs reach the kernel ring buffer through the driver: `dmesg`.
- Log level is compile-time: rebuild with `LOG=debug` (or `info`) to see more.
- Activation: every CPU should print `Activating hypervisor on CPU N...`. A
  missing CPU points at per-CPU init or the `wait_for_other_completed` barrier.

## Common boot failures

- **GRUB**: needs `memmap=<N>G\$0x100000000 iommu=off intremap=off no5lvl`
  (see README.md). Missing `iommu=off` lets Linux fight the monitor for the
  IOMMU.
- **Version mismatch**: `lib/libtpm.a` must match `VERSION`
  (`primary_init_late` checks it). Never edit `VERSION` to make it pass; the
  user needs a matching libtpm.
- **Hang on IOMMU init (AMD)**: a `COMPLETION_WAIT timed out` error means the
  command buffer is misconfigured or the IOMMU is disabled.

## Inspect the binary

`make disasm` dumps the last build (Intel syntax). Useful for checking that a
feature-gated path was compiled in.

## Validating accelerator isolation

Ask the user for, on each of an Intel and an AMD machine:

1. `dmesg | grep HyperEnclave`: the expected accelerators are listed as
   protected, with their BDF and domain id, and the iGPU is not.
2. `lspci -nn` on the host: protected cards are absent (or read `ffff`).
3. The display (iGPU) still works.
4. Denied access is visible when the host or the device tries to reach the
   card. This needs gaps G5 (IOMMU fault-log reading) and G6 (device counters
   in `GetPolicyStats`) from `docs/hypergpu-gap-analysis.md`; until they land,
   only steps 1–3 can be checked.

Record results per vendor in `docs/hypergpu-gap-analysis.md`.
