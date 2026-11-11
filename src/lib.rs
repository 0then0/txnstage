//! TxnStage: a local laboratory for SQLite transactions and concurrency.
//!
//! The binary and the integration tests share this library. See
//! `docs/architecture.md` for how the modules work together.
pub mod engine;
pub mod model;
pub mod scenarios;
pub mod server;
pub mod worker;
