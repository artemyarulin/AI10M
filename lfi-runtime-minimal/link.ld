/* One RX host page, including ELF/program headers. Slots start at 0x9000. */
ENTRY(_start)
PHDRS { code PT_LOAD FILEHDR PHDRS FLAGS(5); }
SECTIONS {
    . = 0x8000 + SIZEOF_HEADERS;
    .text : { *(.text.entry) *(.text .text.*) } :code
    .rodata : { *(.rodata .rodata.*) } :code
    /DISCARD/ : { *(.eh_frame*) *(.comment) *(.note*) }
    ASSERT(. <= 0x9000, "host must fit one 4 KiB page")
}
