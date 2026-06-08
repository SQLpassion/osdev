//! High-precision Time Driver using CMOS/BIB bootstrapping and CPU TSC.

pub mod types;
pub mod calibration;
pub mod manager;

#[allow(unused_imports)]
pub use types::DateTime;
#[allow(unused_imports)]
pub use calibration::rdtsc;
#[allow(unused_imports)]
pub use manager::{init, get_time};

