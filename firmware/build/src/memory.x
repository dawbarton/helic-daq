/* RP2350 memory layout shared by every 2 MiB-flash HELIC-DAQ board
   (W5500-EVB-Pico2, W6100-EVB-Pico2 and Pico 2 W). `emit_memory_x` writes this
   file into the linker search path; the special sections are required by the
   boot ROM and picotool and are platform boilerplate, not experiment storage.
   The layout follows the embassy rp235x examples. A rig on a board with a
   different flash size keeps its own `memory.x` instead of calling that helper.
   Changing this file relinks every rig, so treat it as a platform change. */
MEMORY {
    FLASH : ORIGIN = 0x10000000, LENGTH = 2048K
    RAM   : ORIGIN = 0x20000000, LENGTH = 512K
    SRAM4 : ORIGIN = 0x20080000, LENGTH = 4K
    SRAM5 : ORIGIN = 0x20081000, LENGTH = 4K
}

SECTIONS {
    /* Keep the boot ROM image definition within the first 4 KiB of flash,
       where the boot ROM and picotool look for it. */
    .start_block : ALIGN(4)
    {
        __start_block_addr = .;
        KEEP(*(.start_block));
        KEEP(*(.boot_info));
    } > FLASH

} INSERT AFTER .vector_table;

/* Place executable code after the boot metadata, keeping 8-byte alignment. */
_stext = (ADDR(.start_block) + SIZEOF(.start_block) + 7) & ~7;

SECTIONS {
    /* Picotool follows pointers in the header into this block to find the
       binary information entries. */
    .bi_entries : ALIGN(4)
    {
        __bi_entries_start = .;
        KEEP(*(.bi_entries));
        . = ALIGN(4);
        __bi_entries_end = .;
    } > FLASH
} INSERT AFTER .text;

SECTIONS {
    /* Trailing boot information, after everything else so that it can hold a
       signature. */
    .end_block : ALIGN(4)
    {
        __end_block_addr = .;
        KEEP(*(.end_block));
    } > FLASH

} INSERT AFTER .uninit;

PROVIDE(start_to_end = __end_block_addr - __start_block_addr);
PROVIDE(end_to_start = __start_block_addr + 256M - __end_block_addr);
