#ifndef GDT_H
#define GDT_H

// Virtual address where the GDT and TSS tables are stored
#define GDT_START_OFFSET    0xFFFF800000061000
#define TSS_START_OFFSET    0xFFFF800000062000

// The number of entries in the GDT
#define GDT_ENTRIES	6

// The various GDT flags
#define GDT_FLAG_DATASEG            0x02
#define GDT_FLAG_CODESEG            0x0a
#define GDT_FLAG_TSS                0x09
#define GDT_FLAG_TSS_BUSY           0x02
#define GDT_FLAG_SEGMENT            0x10
#define GDT_FLAG_RING0              0x00
#define GDT_FLAG_RING1              0x20
#define GDT_FLAG_RING2              0x40
#define GDT_FLAG_RING3              0x60
#define GDT_FLAG_PRESENT            0x80
#define GDT_FLAG_ACCESSED           0x01
#define GDT_FLAG_4K_GRAN            0x80
#define GDT_FLAG_16_BIT             0x00
#define GDT_FLAG_32_BIT             0x40
#define GDT_FLAG_64_BIT             0x20

// The various Segment Selectors for the GDT
#define GDT_KERNEL_CODE_SEGMENT     0x8
#define GDT_KERNEL_DATA_SEGMENT     0x10
#define GDT_USER_CODE_SEGMENT       0x18
#define GDT_USER_DATA_SEGMENT       0x20

// The various used RPL levels
#define RPL_RING0                   0x0
#define RPL_RING3                   0x3

// This structure describes a GDT entry
typedef struct
{
    unsigned short LimitLow;            // 16 Bits
    unsigned short BaseLow;             // 16 Bits
    unsigned char BaseMiddle;           // 8 Bits
    unsigned char Access;               // 8 Bits
    unsigned char Granularity;          // 8 Bits
    unsigned char BaseHigh;             // 8 Bits
} __attribute__ ((packed)) GdtEntry;

// This structure describes the GDT pointer
typedef struct
{
    unsigned short Limit;
    unsigned long Base;
} __attribute__ ((packed)) GdtPointer;

typedef struct
{
    int reserved1;
    long rsp0;
    long rsp1;
    long rsp2;
    long reserved2;
    long ist1;
    long ist2;
    long ist3;
    long ist4;
    long ist5;
    long ist6;
    long ist7;
    long reserved3;
    int reserved4;
} __attribute__ ((packed)) TssEntry;

// Initializes the GDT
void InitGdt();

// Returns the TSS Entry
TssEntry *GetTss();

// Sets the GDT Entry
void GdtSetGate(unsigned char Num, unsigned long Base, unsigned long Limit, unsigned char Access, unsigned char Granularity);

// Loads the GDT table into the processor register (implemented in Assembler)
extern void GdtFlush(unsigned long);

#endif