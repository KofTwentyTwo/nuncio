use thiserror::Error;

#[derive(Error, Debug)]
pub enum CoreError {
    #[error("Initialization error: {0}")]
    Initialization(String),
}

pub struct NuncioEngine {
    pub name: &'static str,
    pub domain: &'static str,
}

impl NuncioEngine {
    pub fn new() -> Self {
        Self {
            name: "Nuncio",
            domain: "nuncio.mx",
        }
    }
}

impl Default for NuncioEngine {
    fn default() -> Self {
        Self::new()
    }
}
