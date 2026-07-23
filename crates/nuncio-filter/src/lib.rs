//! NSQL Email Filter Rule Parsing, AST, Validation, and SQL Code Generation.

#![forbid(unsafe_code)]

pub mod ast;
pub mod codegen;
pub mod engine;
pub mod outbox;
pub mod parser;
pub mod validator;

pub use ast::*;
pub use codegen::*;
pub use engine::*;
pub use outbox::*;
pub use parser::*;
pub use validator::*;
