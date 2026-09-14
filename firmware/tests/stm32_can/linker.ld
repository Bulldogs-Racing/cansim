/* STM32F103C8 linker script: 64K flash, 20K RAM. */
ENTRY(Reset_Handler)

MEMORY
{
    FLASH (rx)  : ORIGIN = 0x08000000, LENGTH = 64K
    RAM   (rwx) : ORIGIN = 0x20000000, LENGTH = 20K
}

SECTIONS
{
    .vectors :
    {
        KEEP(*(.vectors))
    } >FLASH

    .text :
    {
        *(.text*)
        *(.rodata*)
        . = ALIGN(4);
    } >FLASH

    /* No .data needed (no initialised statics), but keep the stanza so
       future edits that add globals keep working. */
    .data :
    {
        . = ALIGN(4);
        *(.data*)
        . = ALIGN(4);
    } >RAM AT>FLASH

    .bss :
    {
        . = ALIGN(4);
        *(.bss*)
        *(COMMON)
        . = ALIGN(4);
    } >RAM
}
