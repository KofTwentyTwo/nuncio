//! NSQL Email Filter Rule Parsing, AST, Validation, Webhook Dispatching, and SQL Code Generation.

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod ast;
pub mod codegen;
pub mod engine;
pub mod outbox;
pub mod parser;
pub mod validator;
pub mod webhook;

pub use ast::*;
pub use codegen::*;
pub use engine::*;
pub use outbox::*;
pub use parser::*;
pub use validator::*;
pub use webhook::*;
