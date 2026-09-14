/* CanLab Phase 4 spike: bare-metal STM32F103 bxCAN TX/RX firmware.
 *
 * One source, two roles (`-DROLE_TX` / `-DROLE_RX`), zero dependencies:
 * no HAL, no libopencm3, no libc (compiled `-nostdlib -nostartfiles`,
 * entry `Reset_Handler` in startup.s). All state is stack-local.
 *
 * Clock assumption (documented, not hidden): after reset the F103 runs on
 * the internal 8 MHz HSI with no PLL, so PCLK1 = 8 MHz. USART2 BRR and the
 * bxCAN bit timing below are computed for that clock:
 *
 * - USART2 115200 baud: DIV = 8e6 / 115200 = 69.44 -> MANT=69, FRAC=7,
 *   BRR = (69 << 4) | 7 = 0x451.
 * - bxCAN 500 kbit/s: CAN clock = PCLK1 = 8 MHz, BRP = 0 (TQ = 125 ns),
 *   16 TQ/bit = SYNC(1) + TS1(13) + TS2(2) -> TS1 field 12, TS2 field 1,
 *   BTR = 0x001C0000.
 *
 * Deliberately polling-only (no NVIC/CAN interrupts in the spike) and with
 * bounded waits: every poll loop has a timeout that prints FAIL instead of
 * hanging, so a Renode model gap shows up as a log line, not a deadlock.
 * RCC ready-flags are never waited on: enable bits are set and firmware
 * proceeds (real silicon tolerates this for these enables; a model that
 * needs flag polling is a finding, not a hang).
 */

#include <stdint.h>

/* ---------- registers ---------- */

#define RCC_BASE   0x40021000u
#define RCC_APB2ENR (*(volatile uint32_t *)(RCC_BASE + 0x18u))
#define RCC_APB1ENR (*(volatile uint32_t *)(RCC_BASE + 0x1Cu))

#define GPIOA_BASE 0x40010800u
#define GPIOA_CRH  (*(volatile uint32_t *)(GPIOA_BASE + 0x04u))
#define GPIOA_CRL  (*(volatile uint32_t *)(GPIOA_BASE + 0x00u))
#define GPIOA_ODR  (*(volatile uint32_t *)(GPIOA_BASE + 0x0Cu))

#define USART2_BASE 0x40004400u
#define USART2_SR  (*(volatile uint32_t *)(USART2_BASE + 0x00u))
#define USART2_DR  (*(volatile uint32_t *)(USART2_BASE + 0x04u))
#define USART2_BRR (*(volatile uint32_t *)(USART2_BASE + 0x08u))
#define USART2_CR1 (*(volatile uint32_t *)(USART2_BASE + 0x0Cu))

#define CAN1_BASE  0x40006400u
#define CAN_MCR    (*(volatile uint32_t *)(CAN1_BASE + 0x000u))
#define CAN_MSR    (*(volatile uint32_t *)(CAN1_BASE + 0x004u))
#define CAN_TSR    (*(volatile uint32_t *)(CAN1_BASE + 0x008u))
#define CAN_RF0R   (*(volatile uint32_t *)(CAN1_BASE + 0x00Cu))
#define CAN_BTR    (*(volatile uint32_t *)(CAN1_BASE + 0x01Cu))
#define CAN_TI0R   (*(volatile uint32_t *)(CAN1_BASE + 0x180u))
#define CAN_TDT0R  (*(volatile uint32_t *)(CAN1_BASE + 0x184u))
#define CAN_TDL0R  (*(volatile uint32_t *)(CAN1_BASE + 0x188u))
#define CAN_TDH0R  (*(volatile uint32_t *)(CAN1_BASE + 0x18Cu))
#define CAN_RI0R   (*(volatile uint32_t *)(CAN1_BASE + 0x1B0u))
#define CAN_RDT0R  (*(volatile uint32_t *)(CAN1_BASE + 0x1B4u))
#define CAN_RDL0R  (*(volatile uint32_t *)(CAN1_BASE + 0x1B8u))
#define CAN_RDH0R  (*(volatile uint32_t *)(CAN1_BASE + 0x1BCu))
#define CAN_FMR    (*(volatile uint32_t *)(CAN1_BASE + 0x200u))
#define CAN_FM1R   (*(volatile uint32_t *)(CAN1_BASE + 0x204u))
#define CAN_FS1R   (*(volatile uint32_t *)(CAN1_BASE + 0x20Cu))
#define CAN_FFA1R  (*(volatile uint32_t *)(CAN1_BASE + 0x214u))
#define CAN_FA1R   (*(volatile uint32_t *)(CAN1_BASE + 0x21Cu))
#define CAN_F0R1   (*(volatile uint32_t *)(CAN1_BASE + 0x240u))
#define CAN_F0R2   (*(volatile uint32_t *)(CAN1_BASE + 0x244u))

/* ---------- tiny utils ---------- */

static void spin(volatile uint32_t n) {
    while (n--) {
        __asm__ volatile("nop");
    }
}

static void uart_putc(char c) {
    while (!(USART2_SR & (1u << 7))) { /* TXE */
    }
    USART2_DR = (uint32_t)c;
}

static void uart_puts(const char *s) {
    while (*s) {
        if (*s == '\n') {
            uart_putc('\r');
        }
        uart_putc(*s++);
    }
}

static void uart_puthex_nib(uint8_t v) {
    uart_putc((char)(v < 10 ? '0' + v : 'A' + (v - 10)));
}

/* ---------- USART2 on PA2(TX)/PA3(RX), 115200 8N1 @ PCLK1=8MHz ---------- */

static void uart_init(void) {
    RCC_APB2ENR |= (1u << 2) | (1u << 0); /* IOPAEN | AFIOEN */
    RCC_APB1ENR |= (1u << 17);            /* USART2EN */
    /* PA2: AF push-pull 50 MHz (0xB); PA3: input floating (0x4). */
    GPIOA_CRL = (GPIOA_CRL & ~(0xFu << 8)) | (0xBu << 8);
    GPIOA_CRL = (GPIOA_CRL & ~(0xFu << 12)) | (0x4u << 12);
    USART2_BRR = 0x451u;
    USART2_CR1 = (1u << 13) | (1u << 3) | (1u << 2); /* UE | TE | RE */
}

/* ---------- bxCAN ---------- */

/* Returns 0 on success, negative step on timeout (reported, never hung). */
static int can_init(void) {
    /* PA11 (RX): input pull-up; PA12 (TX): AF push-pull 50 MHz. */
    RCC_APB2ENR |= (1u << 2) | (1u << 0); /* IOPAEN | AFIOEN (idempotent) */
    RCC_APB1ENR |= (1u << 25);            /* CAN1EN */
    GPIOA_CRH = (GPIOA_CRH & ~(0xFu << 12)) | (0x8u << 12);
    GPIOA_ODR |= (1u << 11);
    GPIOA_CRH = (GPIOA_CRH & ~(0xFu << 16)) | (0xBu << 16);

    CAN_MCR |= (1u << 0); /* INRQ: request init mode */
    CAN_MCR &= ~(1u << 1); /* SLEEP: make sure we are not sleeping */
    {
        uint32_t t = 200000;
        while (!(CAN_MSR & (1u << 0))) { /* INAK */
            if (--t == 0) {
                return -1;
            }
        }
    }

    CAN_BTR = 0x001C0000u; /* 500 kbit/s @ 8 MHz: BRP=0, TS1=13, TS2=2 */

    /* Accept-all filter, bank 0 -> FIFO0: mask mode, 32-bit, zeros. */
    CAN_FMR |= (1u << 0);  /* FINIT */
    CAN_FM1R &= ~(1u << 0); /* mask mode */
    CAN_FS1R |= (1u << 0);  /* 32-bit scale */
    CAN_F0R1 = 0;
    CAN_F0R2 = 0;
    CAN_FFA1R &= ~(1u << 0); /* FIFO0 */
    CAN_FA1R |= (1u << 0);   /* activate bank 0 */
    CAN_FMR &= ~(1u << 0);   /* leave filter init */

    CAN_MCR &= ~(1u << 0); /* leave init: INRQ=0 */
    {
        uint32_t t = 200000;
        while (CAN_MSR & (1u << 0)) { /* wait INAK==0 */
            if (--t == 0) {
                return -2;
            }
        }
    }
    return 0;
}

#if defined(ROLE_TX)

static void uart_puthex32(uint32_t v) {
    for (int i = 7; i >= 0; i--) {
        uart_puthex_nib((uint8_t)((v >> (i * 4)) & 0xFu));
    }
}

static int can_tx_std(uint16_t id, const uint8_t *data, uint8_t dlc) {
    uint32_t t = 200000;
    while (!(CAN_TSR & (1u << 26))) { /* TME0 */
        if (--t == 0) {
            return -1;
        }
    }
    CAN_TDT0R = (uint32_t)(dlc & 0xFu);
    CAN_TDL0R = (uint32_t)data[0] | ((uint32_t)data[1] << 8) |
                ((uint32_t)data[2] << 16) | ((uint32_t)data[3] << 24);
    CAN_TDH0R = (uint32_t)data[4] | ((uint32_t)data[5] << 8) |
                ((uint32_t)data[6] << 16) | ((uint32_t)data[7] << 24);
    CAN_TI0R = ((uint32_t)id << 21) | (1u << 0); /* STID + TXRQ */

    t = 2000000;
    while (!(CAN_TSR & (1u << 0))) { /* RQCP0 */
        if (--t == 0) {
            return -2;
        }
    }
    {
        int ok = (CAN_TSR & (1u << 1)) != 0; /* TXOK0 */
        CAN_TSR |= (1u << 0);                /* rc_w1: clear RQCP0 */
        return ok ? 0 : -3;
    }
}

#endif /* ROLE_TX */

#if defined(ROLE_RX)

static void uart_puthex8(uint8_t v) {
    uart_puthex_nib((uint8_t)(v >> 4));
    uart_puthex_nib((uint8_t)(v & 0xFu));
}

/* Poll FIFO0 once. Returns 1 + fills out on frame, 0 on timeout, -1 if the
   pending entry is an extended frame (spike only sends standard). */
static int can_rx_std_once(uint16_t *id, uint8_t *data, uint8_t *dlc,
                           uint32_t timeout) {
    while ((CAN_RF0R & 0x3u) == 0) { /* FMP0 */
        if (--timeout == 0) {
            return 0;
        }
    }
    {
        uint32_t rir = CAN_RI0R;
        uint32_t rdt = CAN_RDT0R;
        uint32_t rdl = CAN_RDL0R;
        uint32_t rdh = CAN_RDH0R;
        CAN_RF0R |= (1u << 5); /* RFOM0: release FIFO entry */
        if (rir & (1u << 2)) { /* IDE: extended — not our spike traffic */
            return -1;
        }
        *id = (uint16_t)((rir >> 21) & 0x7FFu);
        *dlc = (uint8_t)(rdt & 0xFu);
        if (*dlc > 8) {
            *dlc = 8;
        }
        data[0] = (uint8_t)(rdl & 0xFFu);
        data[1] = (uint8_t)((rdl >> 8) & 0xFFu);
        data[2] = (uint8_t)((rdl >> 16) & 0xFFu);
        data[3] = (uint8_t)((rdl >> 24) & 0xFFu);
        data[4] = (uint8_t)(rdh & 0xFFu);
        data[5] = (uint8_t)((rdh >> 8) & 0xFFu);
        data[6] = (uint8_t)((rdh >> 16) & 0xFFu);
        data[7] = (uint8_t)((rdh >> 24) & 0xFFu);
        return 1;
    }
}

static void print_frame(const char *tag, uint16_t id, const uint8_t *data,
                        uint8_t dlc) {
    uart_puts(tag);
    uart_puts(" 0x");
    uart_puthex_nib((uint8_t)((id >> 8) & 0xFu));
    uart_puthex_nib((uint8_t)((id >> 4) & 0xFu));
    uart_puthex_nib((uint8_t)(id & 0xFu));
    uart_puts(" [");
    for (uint8_t i = 0; i < dlc; i++) {
        if (i) {
            uart_putc(' ');
        }
        uart_puthex8(data[i]);
    }
    uart_puts("]\n");
}

#endif /* ROLE_RX */

int main(void) {
    uart_init();

#if defined(ROLE_TX)
    uart_puts("CANLAB TX BOOT\n");
    {
        int rc = can_init();
        if (rc != 0) {
            uart_puts("CAN INIT FAIL\n");
            while (1) {
            }
        }
        uart_puts("CAN INIT OK\n");
    }
    /* Let the RX node boot and join the hub first. */
    spin(8000000);

    {
        static const uint8_t payload[8] = {1, 2, 3, 4, 5, 6, 7, 8};
        for (int i = 0; i < 5; i++) {
            int rc = can_tx_std(0x123, payload, 8);
            if (rc == 0) {
                uart_puts("CAN TX OK 0x123 [01 02 03 04 05 06 07 08]\n");
            } else {
                uart_puts("CAN TX FAIL TSR=0x");
                uart_puthex32(CAN_TSR);
                uart_puts("\n");
            }
            spin(2000000);
        }
        uart_puts("CAN TX DONE\n");
    }
    while (1) {
    }

#elif defined(ROLE_RX)
    uart_puts("CANLAB RX BOOT\n");
    {
        int rc = can_init();
        if (rc != 0) {
            uart_puts("CAN INIT FAIL\n");
            while (1) {
            }
        }
        uart_puts("CAN INIT OK\n");
    }
    /* Let UART settle before the long RX poll. */
    spin(1000000);
    uart_puts("CAN RX WAIT\n");
    {
        int got = 0;
        for (int i = 0; i < 5; i++) {
            uint16_t id = 0;
            uint8_t data[8] = {0};
            uint8_t dlc = 0;
            int rc = can_rx_std_once(&id, data, &dlc, 60000000);
            if (rc == 1) {
                print_frame("CAN RX", id, data, dlc);
                got++;
            } else if (rc == -1) {
                uart_puts("CAN RX EXTENDED (ignored)\n");
            } else {
                uart_puts("CAN RX TIMEOUT\n");
                break;
            }
        }
        uart_puts("CAN RX DONE got=");
        uart_putc((char)('0' + got));
        uart_puts("\n");
    }
    while (1) {
    }

#else
#error "Define ROLE_TX or ROLE_RX"
#endif

    return 0;
}
