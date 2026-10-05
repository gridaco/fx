//! One module per verb. Each returns `Ok(exit status)` or an error (exit 2).

pub mod doctor;
pub mod later;
pub mod lock;
pub mod nodes;
pub mod planning;
pub mod schema;
