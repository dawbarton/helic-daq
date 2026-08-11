//! SRAM-resident compiler memory helpers used implicitly by the hot loop.

/// Copy arbitrarily aligned storage without fetching compiler code from flash.
///
/// LLVM began emitting the generic helper when the command type became
/// non-`Copy`. Most firmware calls are word-aligned, so copy a short byte
/// prefix only when both pointers can reach word alignment together.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".data.ram_func")]
pub unsafe extern "aapcs" fn __aeabi_memcpy(
    mut destination: *mut u8,
    mut source: *const u8,
    mut length: usize,
) {
    while length != 0
        && ((destination as usize) & (align_of::<u32>() - 1)
            != (source as usize) & (align_of::<u32>() - 1))
    {
        // SAFETY: byte access has no alignment requirement and remains inside
        // the caller-provided non-overlapping regions.
        unsafe {
            destination.write_volatile(source.read_volatile());
            source = source.add(1);
            destination = destination.add(1);
        }
        length -= 1;
    }
    while length != 0 && (destination as usize) & (align_of::<u32>() - 1) != 0 {
        // SAFETY: as above; this prefix brings both equally aligned pointers
        // to a four-byte boundary.
        unsafe {
            destination.write_volatile(source.read_volatile());
            source = source.add(1);
            destination = destination.add(1);
        }
        length -= 1;
    }
    while length >= size_of::<u32>() {
        // SAFETY: the prefixes establish four-byte alignment for both
        // pointers. Volatile access prevents recursive memcpy recognition.
        unsafe {
            let value = source.cast::<u32>().read_volatile();
            destination.cast::<u32>().write_volatile(value);
            source = source.add(size_of::<u32>());
            destination = destination.add(size_of::<u32>());
        }
        length -= size_of::<u32>();
    }
    while length != 0 {
        // SAFETY: the tail remains within the same valid regions.
        unsafe {
            destination.write_volatile(source.read_volatile());
            source = source.add(1);
            destination = destination.add(1);
        }
        length -= 1;
    }
}

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

/// Eight-byte-aligned EABI copy entry point, using the aligned SRAM routine.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".data.ram_func")]
pub unsafe extern "aapcs" fn __aeabi_memcpy8(
    destination: *mut u8,
    source: *const u8,
    length: usize,
) {
    // SAFETY: eight-byte alignment satisfies `__aeabi_memcpy4`'s weaker
    // four-byte-alignment contract.
    unsafe { __aeabi_memcpy4(destination, source, length) }
}

/// Clear arbitrarily aligned storage without fetching compiler code from flash.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".data.ram_func")]
pub unsafe extern "aapcs" fn __aeabi_memclr(mut destination: *mut u8, mut length: usize) {
    while length != 0 && (destination as usize) & (align_of::<u32>() - 1) != 0 {
        // SAFETY: byte access has no alignment requirement and remains inside
        // the caller-provided writable region.
        unsafe {
            destination.write_volatile(0);
            destination = destination.add(1);
        }
        length -= 1;
    }
    while length >= size_of::<u32>() {
        // SAFETY: the prefix establishes four-byte alignment. Volatile access
        // prevents recursive compiler-helper recognition.
        unsafe {
            destination.cast::<u32>().write_volatile(0);
            destination = destination.add(size_of::<u32>());
        }
        length -= size_of::<u32>();
    }
    while length != 0 {
        // SAFETY: the tail remains within the same writable region.
        unsafe {
            destination.write_volatile(0);
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

/// Eight-byte-aligned EABI clear entry point, using the aligned SRAM routine.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".data.ram_func")]
pub unsafe extern "aapcs" fn __aeabi_memclr8(destination: *mut u8, length: usize) {
    // SAFETY: eight-byte alignment satisfies `__aeabi_memclr4`'s weaker
    // four-byte-alignment contract.
    unsafe { __aeabi_memclr4(destination, length) }
}
