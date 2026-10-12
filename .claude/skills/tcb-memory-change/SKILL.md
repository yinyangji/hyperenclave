---
name: tcb-memory-change
description: Safety procedure for changing HyperEnclave memory-isolation code (page tables, GPM/EPT/NPT, HVM, memory regions, frame allocation, sinkhole/empty pages, cell.rs). Use before editing src/memory/, src/cell.rs, src/arch/x86_64/intel/ept.rs or src/arch/x86_64/amd/npt.rs, or anything that changes what the host or an enclave can map.
---

# Change TCB memory code

The page-table code is formally verified (ASPLOS'24). A change here can break a
proved invariant without failing any test. Work through every step.

## 1. Name the invariants first

Before editing, write down which of these the change can affect:

- The host GPM never maps monitor memory or EPC to real frames (they map to the
  empty/sinkhole page instead).
- EPC pages are mapped only into their owning enclave.
- A protected device's config page and BARs map to the all-ones sinkhole in
  the GPM (`cell.rs` `insert_device_sinkholes`).
- GPA→HPA is unique per address space; regions in a `MemorySet` never overlap.
- The GPM, HVM and DMA (IOMMU) views of a region stay consistent.

## 2. Both vendors

EPT (`intel/ept.rs`) and NPT (`amd/npt.rs`) share `GenericPTE`
(`src/memory/paging.rs`). A change to one is a change to both; check flag
translations (`MemFlags` ↔ hardware bits) on each side.

## 3. Coherency

- After changing live mappings, flush: the page table's TLB flush, and on SVM
  clear the relevant VMCB clean bits.
- Changes made before the host is downgraded (inside `Cell::new_root()`) need
  no flush; say which case you are in.

## 4. Allocation

- No static arrays larger than a page (a 2MB static lands in `.data`); take
  frames from the frame allocator at init.
- Allocation failure returns `HvResult`, never panics.

## 5. Prove it

Before finishing, answer in the summary: *can an untrusted party (host kernel,
enclave, device DMA) reach secure memory through the new code?* If you cannot
argue no, do not ship. Consider asking the `tcb-security-reviewer` agent for an
independent check.

## 6. Verify

Run the `build-verify` skill (both vendors).
