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

use crate::header::HvHeader;
use crate::percpu::PerCpu;

#[cfg(not(test))]
extern "C" {
    fn __header_start();
    fn __core_end();
}

#[cfg(not(test))]
pub const PER_CPU_ARRAY_PTR: *mut PerCpu = __core_end as _;
#[cfg(not(test))]
pub const HEADER_PTR: *const HvHeader = __header_start as _;

// Host test builds have no linker script providing the image layout
// symbols. Tests never dereference these pointers; they exist so that
// code retained by the test harness (e.g. drop glue) still links.
#[cfg(test)]
pub const PER_CPU_ARRAY_PTR: *mut PerCpu = core::ptr::null_mut();
#[cfg(test)]
pub const HEADER_PTR: *const HvHeader = core::ptr::null();
