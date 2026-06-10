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
mod bins;
mod constants;
mod free;
mod heap;
mod heap_ops;
mod init;
mod options;
mod os;
mod page;
mod page_ops;
mod segment;
mod visit;

pub use api::*;
pub use bins::good_size as mi_good_size;
pub use heap::Heap;
pub use visit::{HeapArea, heap_visit_blocks};

/// `_mi_error_message`: write to stderr without allocating.
pub(crate) fn error(msg: &str) {
    // SAFETY: plain write(2) to stderr.
    unsafe {
        libc::write(2, "rimalloc: error: ".as_ptr().cast(), 17);
        libc::write(2, msg.as_ptr().cast(), msg.len());
        libc::write(2, "\n".as_ptr().cast(), 1);
    }
}
