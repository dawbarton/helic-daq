//! SRAM-resident compiler memory helpers used implicitly by the hot loop.

/// Copy four-byte-aligned storage without calling back into a compiler helper.
///
/// Rust lowers fixed-array moves in the generic tick loop to this ARM EABI
/// symbol. The compiler-builtins implementation lives in XIP flash, so merely
/// placing the Rust caller in `.data.ram_func` is insufficient for isolation.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".data.ram_func")]
pub unsafe extern "aapcs" fn __aeabi_memcpy4(
    mut destination: *mut u8,
    mut source: *const u8,
    mut length: usize,
) {
    while length >= size_of::<u32>() {
        // SAFETY: the EABI `memcpy4` contract guarantees four-byte alignment,
        // and the loop remains within the caller-provided non-overlapping
        // regions. Volatile accesses prevent LLVM recognising this loop as a
        // memcpy operation and recursively lowering it to this same symbol.
        unsafe {
            let value = source.cast::<u32>().read_volatile();
            destination.cast::<u32>().write_volatile(value);
            source = source.add(size_of::<u32>());
            destination = destination.add(size_of::<u32>());
        }
        length -= size_of::<u32>();
    }
    while length != 0 {
        // SAFETY: any non-word tail is still inside the same valid regions.
        unsafe {
            destination.write_volatile(source.read_volatile());
            source = source.add(1);
            destination = destination.add(1);
        }
        length -= 1;
    }
}

/// Clear four-byte-aligned storage without fetching compiler code from flash.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".data.ram_func")]
pub unsafe extern "aapcs" fn __aeabi_memclr4(mut destination: *mut u8, mut length: usize) {
    while length >= size_of::<u32>() {
        // SAFETY: the EABI `memclr4` contract guarantees four-byte alignment
        // and a writable region of `length` bytes. Volatile writes also stop
        // LLVM replacing this implementation with a recursive helper call.
        unsafe {
            destination.cast::<u32>().write_volatile(0);
            destination = destination.add(size_of::<u32>());
        }
        length -= size_of::<u32>();
    }
    while length != 0 {
        // SAFETY: any non-word tail remains within the provided region.
        unsafe {
            destination.write_volatile(0);
            destination = destination.add(1);
        }
        length -= 1;
    }
}
