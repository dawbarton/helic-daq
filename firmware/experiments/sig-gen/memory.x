/* RP2350 memory map for a 2 MiB flash board. build.rs copies this file into
   Cargo's linker search path. The special sections are required by the boot
   ROM and picotool; they are platform boilerplate, not experiment storage. */
MEMORY {
    FLASH : ORIGIN = 0x10000000, LENGTH = 2048K
    RAM   : ORIGIN = 0x20000000, LENGTH = 512K
    SRAM4 : ORIGIN = 0x20080000, LENGTH = 4K
    SRAM5 : ORIGIN = 0x20081000, LENGTH = 4K
}

SECTIONS {
    /* Keep the firmware image definition within the first 4 KiB of flash. */
    .start_block : ALIGN(4)
    {
        __start_block_addr = .;
        KEEP(*(.start_block));
        KEEP(*(.boot_info));
    } > FLASH
} INSERT AFTER .vector_table;

/* Place executable code after boot metadata with eight-byte alignment. */
_stext = (ADDR(.start_block) + SIZEOF(.start_block) + 7) & ~7;

SECTIONS {
    /* Preserve metadata entries inspected by picotool. */
    .bi_entries : ALIGN(4)
    {
        __bi_entries_start = .;
        KEEP(*(.bi_entries));
        . = ALIGN(4);
        __bi_entries_end = .;
    } > FLASH
} INSERT AFTER .text;

SECTIONS {
    /* Reserve the trailing boot-information/signature section. */
    .end_block : ALIGN(4)
    {
        __end_block_addr = .;
        KEEP(*(.end_block));
    } > FLASH
} INSERT AFTER .uninit;

PROVIDE(start_to_end = __end_block_addr - __start_block_addr);
PROVIDE(end_to_start = __start_block_addr + 256M - __end_block_addr);
