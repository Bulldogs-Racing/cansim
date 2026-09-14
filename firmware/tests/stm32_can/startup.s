    .syntax unified
    .cpu cortex-m3
    .thumb

    .section .vectors,"a",%progbits
    .type vectors, %object
vectors:
    .word 0x20005000          /* initial SP: top of 20K RAM (F103C8) */
    .word Reset_Handler       /* Reset */
    .word NMI_Handler         /* NMI */
    .word HardFault_Handler   /* HardFault */
    .word 0                   /* MemManage (M3 has it, unused here) */
    .word 0                   /* BusFault */
    .word 0                   /* UsageFault */
    .word 0, 0, 0, 0          /* reserved */
    .word 0                   /* SVCall */
    .word 0, 0                /* reserved */
    .word 0                   /* PendSV */
    .word 0                   /* SysTick */
    .size vectors, .-vectors

    .section .text,"ax",%progbits
    .global Reset_Handler
    .thumb_func
    .type Reset_Handler, %function
Reset_Handler:
    /* No .data/.bss init needed: firmware uses no initialised statics.
       (All state is stack-local; keeping the startup dependency-free.) */
    bl main
hang:
    b hang
    .size Reset_Handler, .-Reset_Handler

    .thumb_func
    .type NMI_Handler, %function
NMI_Handler:
    b .
    .size NMI_Handler, .-NMI_Handler

    .thumb_func
    .type HardFault_Handler, %function
HardFault_Handler:
    b .
    .size HardFault_Handler, .-HardFault_Handler
