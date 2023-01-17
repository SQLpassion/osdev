#ifndef DATE_H
#define DATE_H

// Increments the system data by 1 second.
void IncrementSystemDate();

// Sets the system date.
void SetDate(int Year, int Month, int Day);

// Sets the system time.
void SetTime(int Hour, int Minute, int Second);

#endif