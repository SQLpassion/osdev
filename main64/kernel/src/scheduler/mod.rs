//! Scheduler module facade.
//!
//! Public scheduler API is implemented in `roundrobin.rs` and re-exported here
//! so `crate::scheduler::*` call sites stay clean.

mod roundrobin;

// Re-exported as scheduler facade API for library consumers/tests.
// The binary target may not reference every symbol directly.
#[allow(unused_imports)]
pub use roundrobin::*;
