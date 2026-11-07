//! Linux builds allocate Rust's heap through mimalloc (`src/lib.rs`), and
//! so does every test binary that links the library.
//!
//! The question goes to mimalloc itself: whether a block Rust just
//! allocated lies in mimalloc's heap. A build where the global allocator is
//! missing, or its cfg names no target this runs on, answers no.
#![cfg(all(target_os = "linux", not(feature = "memory-census")))]

// The global allocator lives in the library, which a test that names none
// of its items would not link. mimalloc is named too, so its C stays linked
// when the library's cfg drops the allocator: that build then fails the
// assertion below, not the link of every test bundled with this one.
extern crate mimalloc;
extern crate roundhouse;

unsafe extern "C" {
    // mimalloc.h: "Is a pointer part of our heap?"
    fn mi_is_in_heap_region(p: *const std::ffi::c_void) -> bool;
}

/// Asks mimalloc about one block of each kind a transpile allocates: a small
/// box, a large vector, a string, and a vector grown by reallocation.
#[test]
fn rust_allocations_come_from_mimalloc() {
    let small = Box::new(7u64);
    let large = vec![0u8; 1 << 20];
    let text = String::from("a transpile allocates IR nodes, types and strings");
    let mut grown = Vec::with_capacity(1);
    grown.extend(0..4096u32);
    for p in [
        std::ptr::from_ref(&*small).cast::<std::ffi::c_void>(),
        large.as_ptr().cast(),
        text.as_ptr().cast(),
        grown.as_ptr().cast(),
    ] {
        assert!(
            unsafe { mi_is_in_heap_region(p) },
            "{p:p} was not allocated by mimalloc"
        );
    }
}
