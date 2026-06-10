//! Allocation fast paths (port of `alloc.c`).

use core::ptr::{self, NonNull};

use crate::constants::*;
use crate::heap::{Heap, heap_get_free_small_page};
use crate::page::Page;
use crate::page_ops::malloc_generic;

/// `_mi_page_malloc_zero`: pop a block from the page free list, or fall
/// into the generic path. ~7 instructions in the fast case.
#[inline(always)]
pub fn page_malloc_zero(heap: &Heap, page: &Page, size: usize, zero: bool) -> Option<NonNull<u8>> {
    debug_assert!(page.block_size.get() == 0 || page.block_size() >= size);
    let block = page.free.get();
    let Some(block_ref) = (
        // SAFETY: free-list nodes are free blocks in this page.
        unsafe { block.as_ref() }
    ) else {
        return malloc_generic(heap, size, zero, 0);
    };
    page.free.set(block_ref.next.get());
    page.used.set(page.used.get() + 1);
    debug_assert!(
        page.block_size() < MAX_ALIGN_SIZE || block.addr().is_multiple_of(MAX_ALIGN_SIZE)
    );

    if zero {
        debug_assert!(page.block_size.get() != 0);
        if page.free_is_zero.get() {
            block_ref.next.set(ptr::null_mut());
        } else {
            // SAFETY: the block is block_size bytes, exclusively ours now.
            unsafe { ptr::write_bytes(block.cast::<u8>(), 0, page.block_size()) };
        }
    }
    NonNull::new(block.cast())
}

/// `mi_heap_malloc_small_zero`
#[inline(always)]
pub fn heap_malloc_small_zero(heap: &Heap, size: usize, zero: bool) -> Option<NonNull<u8>> {
    debug_assert!(size <= SMALL_SIZE_MAX);
    // SAFETY: pages_free_direct entries are live pages or the static empty page.
    let page = unsafe { &*heap_get_free_small_page(heap, size + PADDING_SIZE) };
    page_malloc_zero(heap, page, size + PADDING_SIZE, zero)
}

/// `_mi_heap_malloc_zero_ex`: the main entry point.
#[inline(always)]
pub fn heap_malloc_zero(heap: &Heap, size: usize, zero: bool) -> Option<NonNull<u8>> {
    if size <= SMALL_SIZE_MAX {
        heap_malloc_small_zero(heap, size, zero)
    } else {
        malloc_generic(heap, size + PADDING_SIZE, zero, 0)
    }
}
