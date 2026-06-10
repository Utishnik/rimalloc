//! # rimalloc
//!
//! A pure-Rust port of [mimalloc](https://github.com/microsoft/mimalloc)
//! (v2.3.2): a compact general-purpose allocator with per-thread heaps,
//! sharded free lists, and lock-free cross-thread frees.
//!
//! ```
//! use rimalloc::Rimalloc;
//!
//! // #[global_allocator]
//! // static ALLOC: Rimalloc = Rimalloc;
//! let p = rimalloc::malloc(42);
//! assert!(rimalloc::usable_size(p) >= 42);
//! rimalloc::free(p);
//! ```

#![feature(thread_local)]
#![feature(ptr_mask)]
#![allow(clippy::missing_safety_doc)]

mod alloc;
mod api;
mod arena;
mod bins;
mod constants;
mod free;
mod heap;
mod heap_ops;
pub mod hooks;
mod init;
pub mod options;
mod os;
mod page;
mod page_ops;
mod segment;
mod stats;
mod visit;

pub use api::*;
pub use bins::good_size as mi_good_size;
pub use heap::Heap;
pub use visit::{HeapArea, heap_visit_blocks};

/// `_mi_error_message`: route to the registered output/error hooks
/// (stderr by default) without allocating.
pub(crate) fn error(msg: &str) {
    error_code(msg, libc::EFAULT)
}

pub(crate) fn error_code(msg: &str, code: i32) {
    hooks::output("rimalloc: error: ");
    hooks::output(msg);
    hooks::output("\n");
    hooks::error(code);
}
