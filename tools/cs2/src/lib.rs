#![forbid(unsafe_code)]
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

//! Revision-bound donor inspection. Encoding evidence and command semantics
//! are independent: a lossless instruction is not automatically portable.

pub mod archive;
mod bridge;
pub mod corpus;
pub mod database;
pub mod enums;
pub mod flow;
pub mod frames;
pub mod import910;
pub mod lower910;
pub mod profile;
pub mod semantic;
pub mod variable_bindings;
pub mod variables;
pub mod wire;
