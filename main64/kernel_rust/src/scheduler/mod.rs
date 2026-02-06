//! Scheduler module facade.
//!
//! Public scheduler API is implemented in `roundrobin.rs` and re-exported here
//! so `crate::scheduler::*` call sites stay clean.

mod roundrobin;
mod demotasks;

pub use roundrobin::*;
pub use demotasks::*;
