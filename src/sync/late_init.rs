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

//! A one-shot runtime initializer for statics that cannot be built in `const`
//! context (e.g. types containing locks).
//!
//! This is the single audited `unsafe` convergence point that replaces the old
//! `static mut` + `transmute` pattern: the value is still placed in BSS, but
//! every access is guarded by an initialization flag.

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, Ordering};

/// A static that is uninitialized in BSS and initialized once at runtime,
/// before first use.
///
/// The type keeps the same BSS footprint as the zero-buffer hack it replaces:
/// `size_of::<T>()` bytes of storage (zeroed at boot) plus an init flag.
pub struct LateInit<T> {
    inner: UnsafeCell<MaybeUninit<T>>,
    inited: AtomicBool,
}

// SAFETY: `get()` hands out `&T` only after `init()` stored the value, and
// `init()` runs exactly once (asserted). Writing through `&T` is impossible,
// so concurrent `get()` calls are safe as long as `T: Sync`.
unsafe impl<T: Sync> Sync for LateInit<T> {}

impl<T> LateInit<T> {
    /// Creates an uninitialized `LateInit` suitable for `static` declaration.
    pub const fn new() -> Self {
        Self {
            inner: UnsafeCell::new(MaybeUninit::uninit()),
            inited: AtomicBool::new(false),
        }
    }

    /// Initializes the value. Must be called exactly once, before any `get()`.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that no other CPU can observe the value
    /// before `init` returns. In practice this means calling it on the primary
    /// CPU while secondary CPUs are still waiting on the early-init barrier.
    pub unsafe fn init(&self, value: T) {
        assert!(
            !self.inited.load(Ordering::Acquire),
            "LateInit: initialized twice"
        );
        (*self.inner.get()).write(value);
        self.inited.store(true, Ordering::Release);
    }

    /// Returns a shared reference to the value.
    ///
    /// # Panics
    ///
    /// Panics if called before `init()`; that is a programming error and must
    /// fail fast rather than return a zeroed value.
    pub fn get(&self) -> &T {
        if !self.inited.load(Ordering::Acquire) {
            panic!("LateInit: accessed before initialization");
        }
        // SAFETY: `inited` is set after the value was fully written and is
        // never cleared, so the reference stays valid for the lifetime of
        // this `LateInit` (i.e. `'static` for statics).
        unsafe { (*self.inner.get()).assume_init_ref() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_late_init_get() {
        static L: LateInit<u64> = LateInit::new();
        unsafe { L.init(0xdead_beef) };
        assert_eq!(*L.get(), 0xdead_beef);
    }

    #[test]
    fn test_late_init_struct() {
        struct S(u64, [u8; 4]);
        static L: LateInit<S> = LateInit::new();
        unsafe { L.init(S(0x1234_5678, [1, 2, 3, 4])) };
        assert_eq!(L.get().0, 0x1234_5678);
        assert_eq!(L.get().1, [1, 2, 3, 4]);
    }

    #[test]
    #[should_panic]
    fn test_late_init_access_before_init() {
        static L: LateInit<u64> = LateInit::new();
        let _ = L.get();
    }
}
