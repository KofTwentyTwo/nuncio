use nuncio_core::NuncioEngine;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine = NuncioEngine::new();
    println!("Starting {} GUI desktop shell ({})", engine.name, engine.domain);
    Ok(())
}
