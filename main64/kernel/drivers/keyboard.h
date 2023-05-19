#ifndef KEYBOARD_H
#define KEYBOARD_H

// Video output memory address
#define KEYBOARD_BUFFER 0xFFFF8000001FFFFF

// Onboard Keyboard Controller Status Register
#define KYBRD_CTRL_STATS_REG    0x64

// Onboard Keyboard Controller Send Command
#define KYBRD_CTRL_CMD_REG      0x64

// Keyboard Status Mask
#define KYBRD_CTRL_STATS_MASK_OUT_BUF   1       // 00000001
#define KYBRD_CTRL_STATS_MASK_IN_BUF    2       // 00000010
#define KYBRD_CTRL_STATS_MASK_SYSTEM    4       // 00000100
#define KYBRD_CTRL_STATS_MASK_CMD_DATA  8       // 00001000
#define KYBRD_CTRL_STATS_MASK_LOCKED    0x10    // 00010000
#define KYBRD_CTRL_STATS_MASK_AUX_BUF   0x20    // 00100000
#define KYBRD_CTRL_STATS_MASK_TIMEOUT   0x40    // 01000000
#define KYBRD_CTRL_STATS_MASK_PARITY    0x80    // 10000000

// Keyboard Encoder
#define KYBRD_ENC_INPUT_BUF     0x60
#define KYBRD_ENC_CMD_REG       0x60

enum KEYCODE
{
    // Numeric keys
    KEY_0                 = '0',
    KEY_1                 = '1',
    KEY_2                 = '2',
    KEY_3                 = '3',
    KEY_4                 = '4',
    KEY_5                 = '5',
    KEY_6                 = '6',
    KEY_7                 = '7',
    KEY_8                 = '8',
    KEY_9                 = '9',
    
    // Alphanumeric lower case keys
    KEY_A_LC              = 'a',
    KEY_B_LC              = 'b',
    KEY_C_LC              = 'c',
    KEY_D_LC              = 'd',
    KEY_E_LC              = 'e',
    KEY_F_LC              = 'f',
    KEY_G_LC              = 'g',
    KEY_H_LC              = 'h',
    KEY_I_LC              = 'i',
    KEY_J_LC              = 'j',
    KEY_K_LC              = 'k',
    KEY_L_LC              = 'l',
    KEY_M_LC              = 'm',
    KEY_N_LC              = 'n',
    KEY_O_LC              = 'o',
    KEY_P_LC              = 'p',
    KEY_Q_LC              = 'q',
    KEY_R_LC              = 'r',
    KEY_S_LC              = 's',
    KEY_T_LC              = 't',
    KEY_U_LC              = 'u',
    KEY_V_LC              = 'v',
    KEY_W_LC              = 'w',
    KEY_X_LC              = 'x',
    KEY_Y_LC              = 'y',
    KEY_Z_LC              = 'z',
    KEY_SS_LC             = 's',
    KEY_OE_LC             = 'o',
    KEY_AE_LC             = 'a',

    // Alphanumeric upper case keys
    KEY_A_UC              = 'A',
    KEY_B_UC              = 'B',
    KEY_C_UC              = 'C',
    KEY_D_UC              = 'D',
    KEY_E_UC              = 'E',
    KEY_F_UC              = 'F',
    KEY_G_UC              = 'G',
    KEY_H_UC              = 'H',
    KEY_I_UC              = 'I',
    KEY_J_UC              = 'J',
    KEY_K_UC              = 'K',
    KEY_L_UC              = 'L',
    KEY_M_UC              = 'M',
    KEY_N_UC              = 'N',
    KEY_O_UC              = 'O',
    KEY_P_UC              = 'P',
    KEY_Q_UC              = 'Q',
    KEY_R_UC              = 'R',
    KEY_S_UC              = 'S',
    KEY_T_UC              = 'T',
    KEY_U_UC              = 'U',
    KEY_V_UC              = 'V',
    KEY_W_UC              = 'W',
    KEY_X_UC              = 'X',
    KEY_Y_UC              = 'Y',
    KEY_Z_UC              = 'Z',
    KEY_OE_UC             = 'O',
    KEY_AE_UC             = 'A',

    // Special character keys
    KEY_DOT               = '.',
    KEY_COMMA             = ',',
    KEY_COLON             = ':',
    KEY_SEMICOLON         = ';',
    KEY_SLASH             = '/',
    KEY_BACKSLASH         = '\\',
    KEY_PLUS              = '+',
    KEY_MINUS             = '-',
    KEY_ASTERISK          = '*',
    KEY_EXCLAMATION       = '!',
    KEY_QUESTION          = '?',
    KEY_QUOTEDOUBLE       = '\"',
    KEY_QUOTE             = '\'',
    KEY_EQUAL             = '=',
    KEY_HASH              = '#',
    KEY_PERCENT           = '%',
    KEY_AMPERSAND         = '&',
    KEY_UNDERSCORE        = '_',
    KEY_LEFTPARENTHESIS   = '(',
    KEY_RIGHTPARENTHESIS  = ')',
    KEY_LEFTBRACKET       = '[',
    KEY_RIGHTBRACKET      = ']',
    KEY_LEFTCURL          = '{',
    KEY_RIGHTCURL         = '}',
    KEY_DOLLAR            = '$',
    KEY_POUND             = ' ',
    KEY_EURO              = '$',
    KEY_LESS              = '<',
    KEY_GREATER           = '>',
    KEY_BAR               = '|',
    KEY_GRAVE             = '`',
    KEY_TILDE             = '~',
    KEY_AT                = '@',
    KEY_CARRET            = '^',
    KEY_PARAGRAPH         = '$',

    // Control keys
    KEY_SPACE             = ' ',
    KEY_RETURN            = '\r',
    KEY_ESCAPE            = 0x1001,
    KEY_BACKSPACE         = '\b',
    KEY_TAB               = 0x4000,
    KEY_CAPSLOCK          = 0x4001,
    KEY_LSHIFT            = 0x4002,
    KEY_LCTRL             = 0x4003,
    KEY_LALT              = 0x4004,
    KEY_LWIN              = 0x4005,
    KEY_RSHIFT            = 0x4006,
    KEY_RCTRL             = 0x4007,
    KEY_RALT              = 0x4008,
    KEY_RWIN              = 0x4009,
    KEY_INSERT            = 0x400a,
    KEY_DELETE            = 0x400b,
    KEY_HOME              = 0x400c,
    KEY_END               = 0x400d,
    KEY_PAGEUP            = 0x400e,
    KEY_PAGEDOWN          = 0x400f,
    KEY_SCROLLLOCK        = 0x4010,
    KEY_PAUSE             = 0x4011,
    KEY_UNKNOWN          =  0x5000,
    
    // Arrow keys
    KEY_UP                = 0x1100,
    KEY_DOWN              = 0x1101,
    KEY_LEFT              = 0x1102,
    KEY_RIGHT             = 0x1103,
    
    // Function keys
    KEY_F1                = 0x1201,
    KEY_F2                = 0x1202,
    KEY_F3                = 0x1203,
    KEY_F4                = 0x1204,
    KEY_F5                = 0x1205,
    KEY_F6                = 0x1206,
    KEY_F7                = 0x1207,
    KEY_F8                = 0x1208,
    KEY_F9                = 0x1209,
    KEY_F10               = 0x120a,
    KEY_F11               = 0x120b,
    KEY_F12               = 0x120b,
    KEY_F13               = 0x120c,
    KEY_F14               = 0x120d,
    KEY_F15               = 0x120e,
    
    // Numeric keypad keys
    KEY_KP_0              = '0',
    KEY_KP_1              = '1',
    KEY_KP_2              = '2',
    KEY_KP_3              = '3',
    KEY_KP_4              = '4',
    KEY_KP_5              = '5',
    KEY_KP_6              = '6',
    KEY_KP_7              = '7',
    KEY_KP_8              = '8',
    KEY_KP_9              = '9',
    KEY_KP_PLUS           = '+',
    KEY_KP_MINUS          = '-',
    KEY_KP_DECIMAL        = '.',
    KEY_KP_DIVIDE         = '/',
    KEY_KP_ASTERISK       = '*',
    KEY_KP_NUMLOCK        = 0x300f,
    KEY_KP_ENTER          = 0x3010
};

// XT Scan Code Set
// The Array Index is the Make Code received from the keyboard.
static int ScanCodes_LowerCase_QWERTZ [] =
{
    // Key                  Scan Code
    KEY_UNKNOWN,            // 0
    KEY_ESCAPE,             // 1
    KEY_1,                  // 2
    KEY_2,                  // 3
    KEY_3,                  // 4
    KEY_4,                  // 5
    KEY_5,                  // 6
    KEY_6,                  // 7
    KEY_7,                  // 8
    KEY_8,                  // 9
    KEY_9,                  // 0xa
    KEY_0,                  // 0xb
    KEY_SS_LC,              // 0xc
    KEY_EQUAL,              // 0xd
    KEY_BACKSPACE,          // 0xe
    KEY_TAB,                // 0xf
    KEY_Q_LC,               // 0x10
    KEY_W_LC,               // 0x11
    KEY_E_LC,               // 0x12
    KEY_R_LC,               // 0x13
    KEY_T_LC,               // 0x14
    KEY_Z_LC,               // 0x15
    KEY_U_LC,               // 0x16
    KEY_I_LC,               // 0x17
    KEY_O_LC,               // 0x18
    KEY_P_LC,               // 0x19
    KEY_LEFTBRACKET,        // 0x1a
    KEY_PLUS,               // 0x1b
    KEY_RETURN,             // 0x1c
    KEY_LCTRL,              // 0x1d
    KEY_A_LC,               // 0x1e
    KEY_S_LC,               // 0x1f
    KEY_D_LC,               // 0x20
    KEY_F_LC,               // 0x21
    KEY_G_LC,               // 0x22
    KEY_H_LC,               // 0x23
    KEY_J_LC,               // 0x24
    KEY_K_LC,               // 0x25
    KEY_L_LC,               // 0x26
    KEY_LEFTCURL,           // 0x27
    KEY_TILDE,              // 0x28
    KEY_LESS,               // 0x29
    KEY_LSHIFT,             // 0x2a
    KEY_HASH,               // 0x2b
    KEY_Y_LC,               // 0x2c
    KEY_X_LC,               // 0x2d
    KEY_C_LC,               // 0x2e
    KEY_V_LC,               // 0x2f
    KEY_B_LC,               // 0x30
    KEY_N_LC,               // 0x31
    KEY_M_LC,               // 0x32
    KEY_COMMA,              // 0x33
    KEY_DOT,                // 0x34
    KEY_MINUS,              // 0x35
    KEY_RSHIFT,             // 0x36
    KEY_KP_ASTERISK,        // 0x37
    KEY_RALT,               // 0x38
    KEY_SPACE,              // 0x39
    KEY_CAPSLOCK,           // 0x3a
    KEY_F1,                 // 0x3b
    KEY_F2,                 // 0x3c
    KEY_F3,                 // 0x3d
    KEY_F4,                 // 0x3e
    KEY_F5,                 // 0x3f
    KEY_F6,                 // 0x40
    KEY_F7,                 // 0x41
    KEY_F8,                 // 0x42
    KEY_F9,                 // 0x43
    KEY_F10,                // 0x44
    KEY_UNKNOWN,            // 0x45
    KEY_UNKNOWN,            // 0x46
    KEY_UNKNOWN,            // 0x47
    KEY_UP,                 // 0x48
    KEY_UNKNOWN,            // 0x49
    KEY_UNKNOWN,            // 0x4a,
    KEY_LEFT,               // 0x4b,
    KEY_UNKNOWN,            // 0x4c,
    KEY_RIGHT,              // 0x4d,
    KEY_UNKNOWN,            // 0x4e,
    KEY_UNKNOWN,            // 0x4f,
    KEY_DOWN,               // 0x50
    KEY_UNKNOWN,            // 0x51
    KEY_UNKNOWN,            // 0x52
    KEY_UNKNOWN,            // 0x53
    KEY_UNKNOWN,            // 0x54
    KEY_UNKNOWN,            // 0x55
    KEY_UNKNOWN,            // 0x56
    KEY_F11,                // 0x57
    KEY_F12                 // 0x58
};

// XT Scan Code Set
// The Array Index is the Make Code received from the keyboard.
static int ScanCodes_UpperCase_QWERTZ [] =
{
    // Key                  Scan Code
    KEY_UNKNOWN,            // 0
    KEY_ESCAPE,             // 1
    KEY_EXCLAMATION,        // 2
    KEY_QUOTEDOUBLE,        // 3
    KEY_PARAGRAPH,          // 4
    KEY_DOLLAR,             // 5
    KEY_PERCENT,            // 6
    KEY_AMPERSAND,          // 7
    KEY_SLASH,              // 8
    KEY_LEFTPARENTHESIS,    // 9
    KEY_RIGHTPARENTHESIS,   // 0xa
    KEY_EQUAL,              // 0xb
    KEY_QUESTION,           // 0xc
    KEY_GRAVE,              // 0xd
    KEY_BACKSPACE,          // 0xe
    KEY_TAB,                // 0xf
    KEY_Q_UC,               // 0x10
    KEY_W_UC,               // 0x11
    KEY_E_UC,               // 0x12
    KEY_R_UC,               // 0x13
    KEY_T_UC,               // 0x14
    KEY_Z_UC,               // 0x15
    KEY_U_UC,               // 0x16
    KEY_I_UC,               // 0x17
    KEY_O_UC,               // 0x18
    KEY_P_UC,               // 0x19
    KEY_RIGHTBRACKET,       // 0x1a
    KEY_ASTERISK,           // 0x1b
    KEY_RETURN,             // 0x1c
    KEY_LCTRL,              // 0x1d
    KEY_A_UC,               // 0x1e
    KEY_S_UC,               // 0x1f
    KEY_D_UC,               // 0x20
    KEY_F_UC,               // 0x21
    KEY_G_UC,               // 0x22
    KEY_H_UC,               // 0x23
    KEY_J_UC,               // 0x24
    KEY_K_UC,               // 0x25
    KEY_L_UC,               // 0x26
    KEY_RIGHTCURL,          // 0x27
    KEY_AT,                 // 0x28
    KEY_GREATER,            // 0x29
    KEY_LSHIFT,             // 0x2a
    KEY_BACKSLASH,          // 0x2b
    KEY_Y_UC,               // 0x2c
    KEY_X_UC,               // 0x2d
    KEY_C_UC,               // 0x2e
    KEY_V_UC,               // 0x2f
    KEY_B_UC,               // 0x30
    KEY_N_UC,               // 0x31
    KEY_M_UC,               // 0x32
    KEY_SEMICOLON,          // 0x33
    KEY_COLON,              // 0x34
    KEY_UNDERSCORE,         // 0x35
    KEY_RSHIFT,             // 0x36
    KEY_KP_ASTERISK,        // 0x37
    KEY_RALT,               // 0x38
    KEY_SPACE,              // 0x39
    KEY_CAPSLOCK,           // 0x3a
    KEY_F1,                 // 0x3b
    KEY_F2,                 // 0x3c
    KEY_F3,                 // 0x3d
    KEY_F4,                 // 0x3e
    KEY_F5,                 // 0x3f
    KEY_F6,                 // 0x40
    KEY_F7,                 // 0x41
    KEY_F8,                 // 0x42
    KEY_F9,                 // 0x43
    KEY_F10,                // 0x44
    KEY_UNKNOWN,            // 0x45
    KEY_UNKNOWN,            // 0x46
    KEY_UNKNOWN,            // 0x47
    KEY_UP,                 // 0x48
    KEY_UNKNOWN,            // 0x49
    KEY_UNKNOWN,            // 0x4a,
    KEY_LEFT,               // 0x4b,
    KEY_UNKNOWN,            // 0x4c,
    KEY_RIGHT,              // 0x4d,
    KEY_UNKNOWN,            // 0x4e,
    KEY_UNKNOWN,            // 0x4f,
    KEY_DOWN,               // 0x50
    KEY_UNKNOWN,            // 0x51
    KEY_UNKNOWN,            // 0x52
    KEY_UNKNOWN,            // 0x53
    KEY_UNKNOWN,            // 0x54
    KEY_UNKNOWN,            // 0x55
    KEY_UNKNOWN,            // 0x56
    KEY_F11,                // 0x57
    KEY_F12                 // 0x58
};

// Initializes the keyboard
void InitKeyboard();

// This function runs continuosly in the Kernel, processes a key press, 
// and stores the entered character in the memory.
void KeyboardHandlerTask();

// Reads data from the keyboard
void scanf(char *buffer, int buffer_size);

// Waits for a key stroke, and returns it
char getchar();

// Discards the last key press
static void DiscardLastKey();

// Keyboard callback function
static void KeyboardCallback(int Number);

// Reads the keyboard status
static unsigned char ReadStatus();

// Reads the keyboard encoder buffer
static unsigned char ReadBuffer();

#endif