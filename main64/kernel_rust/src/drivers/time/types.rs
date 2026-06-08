//! Type definitions for the time driver.

/// Calendar Date and Time representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl DateTime {
    /// Increments the DateTime by a given number of seconds.
    pub fn add_seconds(&mut self, secs: u64) {
        // Step 1: Add seconds and calculate minute/hour/day increments.
        let sec_sum = self.second as u64 + secs;
        self.second = (sec_sum % 60) as u8;
        let mins = sec_sum / 60;

        if mins > 0 {
            let min_sum = self.minute as u64 + mins;
            self.minute = (min_sum % 60) as u8;
            let hours = min_sum / 60;

            if hours > 0 {
                let hour_sum = self.hour as u64 + hours;
                self.hour = (hour_sum % 24) as u8;
                let mut days = hour_sum / 24;

                // Step 2: Propagate days across months and years.
                while days > 0 {
                    let days_in_mo = days_in_month(self.year, self.month) as u64;
                    let day_sum = self.day as u64 + days;
                    if day_sum <= days_in_mo {
                        self.day = day_sum as u8;
                        break;
                    } else {
                        days = day_sum - days_in_mo - 1;
                        self.day = 1;
                        self.month += 1;
                        if self.month > 12 {
                            self.month = 1;
                            self.year += 1;
                        }
                    }
                }
            }
        }
    }
}

/// Helper function to return the number of days in a given month.
fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

/// Helper function to check if a year is a leap year.
fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}
