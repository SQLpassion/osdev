#include "common.h"
#include "date.h"

// The number of days for each month
int NumberOfDaysPerMonth[12] =
{
    31, // January
    28, // February
    31, // March
    30, // April
    31, // Mai
    30, // June
    31, // July
    31, // August
    30, // September
    31, // October
    30, // November
    31  // December
};

// Increments the system data by 1 second.
void IncrementSystemDate()
{
    // Getting a reference to the BIOS Information Block
    BiosInformationBlock *bib = (BiosInformationBlock *)BIB_OFFSET;

    // Increment the system date by 1 second
    bib->Second++;

    // Roll over to the next minute
    if (bib->Second > 59)
    {
        bib->Second = 0;
        bib->Minute++;
    }

    // Roll over to the next hour
    if (bib->Minute > 59)
    {
        bib->Minute = 0;
        bib->Hour++;
    }

    // Roll over to the next day
    if (bib->Hour > 23)
    {
        bib->Hour = 0;
        bib->Day++;
    }

    // Roll over to the next month
    // We don't check here for the leap year!!!
    if (bib->Day > NumberOfDaysPerMonth[bib->Month - 1])
    {
        bib->Day = 1;
        bib->Month++;
    }
}

// Sets the system date.
void SetDate(int Year, int Month, int Day)
{
    // Getting a reference to the BIOS Information Block
    BiosInformationBlock *bib = (BiosInformationBlock *)BIB_OFFSET;

    // Set the date
    bib->Year = Year;
    bib->Month = Month;
    bib->Day = Day;
}

// Sets the system time.
void SetTime(int Hour, int Minute, int Second)
{
    // Getting a reference to the BIOS Information Block
    BiosInformationBlock *bib = (BiosInformationBlock *)BIB_OFFSET;

    // Set the time
    bib->Hour = Hour;
    bib->Minute = Minute;
    bib->Second = Second;
}