//! High-precision Time Driver using CMOS/BIB bootstrapping and CPU TSC.

pub mod calibration;
pub mod manager;
pub mod types;

#[allow(unused_imports)]
pub use calibration::rdtsc;
#[allow(unused_imports)]
pub use manager::{get_time, init};
#[allow(unused_imports)]
pub use types::DateTime;
