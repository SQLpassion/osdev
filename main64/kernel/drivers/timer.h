#ifndef TIMER_H
#define TIMER_H

// Initializes the hardware timer
void InitTimer(int Hertz);

// IRQ callback function
static void TimerCallback(int Number);

// Refreshs the status line
void RefreshStatusLine();

#endif