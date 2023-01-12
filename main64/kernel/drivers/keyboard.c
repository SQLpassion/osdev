//
//  keyboard.c
//  KAOS
//
//  Created by Klaus Aschenbrenner on 23.02.2014.
//  Copyright (c) 2014 Klaus Aschenbrenner. All rights reserved.
//

#include "../common.h"
#include "../isr/irq.h"
#include "screen.h"
#include "keyboard.h"

// Stores the last received Scan Code from the keyboard.
// The value "0" means, we have entered a non-printable character (like "shift")
static char lastReceivedScanCode;

// Stores if the shift key is pressed, or not
int shiftKey;

// Stores if the caps lock key is pressed, or not
int capsLock;

int leftCtrl;

// Used to indicate the last scan code is not to be reused
const char INVALID_SCANCODE = (char)0;

// Initializes the keyboard
void InitKeyboard()
{
    // Registers the IRQ callback function for the hardware timer
    RegisterIrqHandler(33, &KeyboardCallback);
    
    // Set the internal key buffer to KEY_UNKNOWN
    DiscardLastKey();
    
    capsLock = 0;
}

// Reads data from the keyboard
void scanf(char *buffer, int buffer_size)
{
    int processKey = 1;
    int i = 0;
    
    while (i < buffer_size)
    {
        char key = getchar();
        processKey = 1;
        
        // When we have hit the ENTER key, we have finished entering our input data
        if (key == KEY_RETURN)
        {
            print_char('\n');
            break;
        }
        
        if (key == KEY_BACKSPACE)
        {
            processKey = 0;
        
            // We only process the backspace key, if we have data already in the input buffer
            if (i > 0)
            {
                int col;
                int row;
            
                // Move the cursor position one character back
                GetCursorPosition(&row, &col);
                col -= 1;
                SetCursorPosition(row, col);
            
                // Clear out the last printed key
                // This also moves the cursor one character forward, so we have to go back
                // again with the cursor in the next step
                print_char(' ');
                
                // Move the cursor position one character back again
                GetCursorPosition(&row, &col);
                col -= 1;
                SetCursorPosition(row, col);
            
                // Delete the last entered character from the input buffer
                i--;
            }
        }
        
        if (processKey == 1)
        {
            // Print out the current entered key stroke
            // If we have pressed a non-printable key, the character is not printed out
            if (key != 0)
            {
                print_char(key);
            }
        
            // Write the entered character into the provided buffer
            buffer[i] = key;
            i++;
        }
    }
    
    // Null-terminate the input string
    buffer[i] = '\0';
}

// Waits for a key press, and returns it
char getchar()
{
    char key = INVALID_SCANCODE;
    
    // Wait until we get a key stroke
    while (key == INVALID_SCANCODE)
    {
        if (lastReceivedScanCode > INVALID_SCANCODE)
        {
            if (shiftKey || capsLock)
                key = ScanCodes_UpperCase_QWERTZ[lastReceivedScanCode];
            else
                key = ScanCodes_LowerCase_QWERTZ[lastReceivedScanCode];
        }
        else
        {
            key = INVALID_SCANCODE;
        }
    }

    DiscardLastKey();

    // Return the received character from the keyboard
    return key;
}

// Discards the last key press
static void DiscardLastKey()
{
    lastReceivedScanCode = INVALID_SCANCODE;
}

// Keyboard callback function
static void KeyboardCallback(int Number)
{
    // Check if the keyboard controller output buffer is full
	if (ReadStatus() & KYBRD_CTRL_STATS_MASK_OUT_BUF)
    {
        // Read the scan code
        int code = ReadBuffer();
        
        // Check if the current scan code is a break code
        if (code & 0x80)
        {
            // A break code is received from the keyboard when the key is released.
            // In a break code the 8th bit is set, therefore we test above against 0x80 (10000000b)
            
            // Convert the break code into the make code by clearing the 8th bit
            code -= 0x80;
            
            // Get the key from the scan code table
            int key = ScanCodes_LowerCase_QWERTZ[code];
            
            switch (key)
            {
                case KEY_LCTRL:
                {
                    leftCtrl = 0;
                    lastReceivedScanCode = 0;
                    break;
                }
                case KEY_LSHIFT:
                {
                    // The left shift key is released
                    shiftKey = 0;
                    lastReceivedScanCode = 0;
                    break;
                }
                case KEY_RSHIFT:
                {
                    // The right shift key is released
                    lastReceivedScanCode = 0;
                    shiftKey = 0;
                    break;
                }
            }
        }
        else
        {
            // Get the key from the scan code table
            int key = ScanCodes_LowerCase_QWERTZ[code];
            
            switch (key)
            {
                case KEY_LCTRL:
                {
                    leftCtrl = 1;
                    lastReceivedScanCode = 0;
                    break;
                }
                case KEY_CAPSLOCK:
                {
                    // The caps lock key is pressed
                    // We just toggle the flag
                    if (capsLock == 1)
                        capsLock = 0;
                    else
                        capsLock = 1;
                    
                    break;
                }
                case KEY_LSHIFT:
                {
                    // The left shift key is pressed
                    shiftKey = 1;
                    lastReceivedScanCode = 0;
                    break;
                }
                case KEY_RSHIFT:
                {
                    // The right shift key is pressed
                    lastReceivedScanCode = 0;
                    shiftKey = 1;
                    break;
                }
                default:
                {
                    // We only buffer the Scan Code from the keyboard, if it is a printable character
                    lastReceivedScanCode = code;
                    break;
                }
            }
        }
    }
}

// Reads the keyboard status
static unsigned char ReadStatus()
{
    return inb(KYBRD_CTRL_STATS_REG);
}

// Reads the keyboard encoder buffer
static unsigned char ReadBuffer()
{
    return inb(KYBRD_ENC_INPUT_BUF);
}