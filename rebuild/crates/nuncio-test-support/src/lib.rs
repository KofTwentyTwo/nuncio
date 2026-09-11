pub mod google;
pub mod imap;
pub mod process;

pub type TestError = Box<dyn std::error::Error + Send + Sync>;
