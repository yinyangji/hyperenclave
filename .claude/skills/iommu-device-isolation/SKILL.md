---
name: iommu-device-isolation
description: How to change HyperEnclave accelerator isolation and IOMMU code (src/device/ classification and domains, VT-d context entries, AMD-Vi device table and command buffer, legacy 0xCF8/0xCFC config emulation, device hypercalls). Use when protecting/hiding a GPU/NPU/DCU from the host, changing which devices are classified as accelerators, touching per-device DMA domains, or editing intel/vtd.rs, amd/iommu.rs, pio.rs or pci_cfg.rs.
---

# IOMMU and device isolation

Design: `docs/device-isolation-design.md`. Status and open gaps:
`docs/hypergpu-gap-analysis.md`. **Scope for now is x86 only** (Intel and
AMD); do not start an ARM/SMMUv3 backend until both x86 vendors are validated
on hardware, but keep `src/device/` architecture-neutral.

## Two planes, both required

- **Control plane** (host cannot drive the device): config page + BARs map to
  the all-ones sinkhole in the GPM; legacy `0xCF8`/`0xCFC` cycles to a protected
  BDF read all-ones and drop writes (`arch/x86_64/pio.rs`). ECAM and PIO must
  agree: both consult `device::is_protected_device`.
- **Data plane** (device cannot DMA freely): each protected function has its
  own IOMMU domain, starting with an **empty** table (fail-closed).

## Classification rules (`src/device/ident.rs`)

- Protected only if class, vendor whitelist **and** BAR ≥ 256MB all agree.
- Integrated graphics must never be hidden (blacks out the console). Intel
  `0x8086` stays off the vendor list; add tests for any new vendor.
- Overrides: `HE_PROTECT_BDF` / `HE_EXCLUDE_BDF` at build time; exclude wins.

## IOMMU invariants

- Every protected device gets a **distinct hardware domain id** from
  `FIRST_PRIVATE_DOMAIN_ID` (16) up; never the default domain (1). The IOTLB is
  tagged by domain id, so sharing one leaks cached translations.
- After changing a live entry or page table, invalidate before returning:
  - VT-d: clflush the context entry, context-cache + IOTLB invalidation.
  - AMD-Vi: `INVALIDATE_DEVTAB_ENTRY`, `INVALIDATE_IOMMU_PAGES`, then
    `COMPLETION_WAIT`. The IOMMU only processes commands while enabled, so skip
    them before `set_enabled(true)` and never spin without a timeout.
  - Under SME, any address handed to the IOMMU carries the C-bit
    (`phys_encrypted`).
- `DeviceManager` re-syncs the IOMMU inside the domain lock (`sync_iommu`), so
  no caller sees a mapped-but-unflushed state. Keep it that way.
- DMA grants (`DeviceMapDma`) must lie wholly in normal-world RAM and outside
  the CMR (`Cell::is_normal_world_dma_range`); batches are all-or-nothing.

## Pairing

A change in `intel/vtd.rs` almost always needs the matching change in
`amd/iommu.rs`, and vice versa. Put encodings in pure functions with unit
tests (see `cmd_inv_*` in `amd/iommu.rs`).

## Verify

Run the `build-verify` skill. Real isolation (host `lspci` no longer shows the
card, DMA faults while unassigned, iGPU display still works) can only be shown
on hardware: see `debug-on-hardware`.
