use crate::{args::Args, output::AppError};
use std::{fs::File, io::Read, path::PathBuf};
use zeroize::Zeroizing;

pub fn load(args: &Args) -> Result<Zeroizing<String>, AppError> {
    if args.profile.is_empty()
        || args.profile.len() > 64
        || !args
            .profile
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(AppError::invalid());
    }
    let directory = match &args.data_dir {
        Some(path) => path.clone(),
        None => PathBuf::from(std::env::var_os("HOME").ok_or_else(AppError::auth)?)
            .join(".nuncio-rebuild")
            .join(&args.profile),
    };
    let mut bytes = Vec::new();
    File::open(directory.join("profile.json"))
        .map_err(|_| AppError::auth())?
        .take(4097)
        .read_to_end(&mut bytes)
        .map_err(|_| AppError::auth())?;
    if bytes.len() > 4096 {
        return Err(AppError::auth());
    }
    let manifest: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| AppError::auth())?;
    if manifest["version"] != 1 {
        return Err(AppError::auth());
    }
    let id = uuid::Uuid::parse_str(manifest["id"].as_str().ok_or_else(AppError::auth)?)
        .map_err(|_| AppError::auth())?;
    let secret = read_secret(args, &format!("{id}/profile/api"))?;
    if secret.len() != 64 || !secret.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AppError::auth());
    }
    Ok(Zeroizing::new(format!("Bearer {}", secret.as_str())))
}

fn read_secret(_args: &Args, name: &str) -> Result<Zeroizing<String>, AppError> {
    #[cfg(feature = "test-harness")]
    if let Some(path) = &_args.test_secrets_file {
        let mut bytes = Zeroizing::new(Vec::new());
        File::open(path)
            .map_err(|_| AppError::auth())?
            .take(1_048_577)
            .read_to_end(&mut bytes)
            .map_err(|_| AppError::auth())?;
        if bytes.len() > 1_048_576 {
            return Err(AppError::auth());
        }
        let mut values: std::collections::BTreeMap<String, String> =
            serde_json::from_slice(&bytes).map_err(|_| AppError::auth())?;
        return values
            .remove(name)
            .map(Zeroizing::new)
            .ok_or_else(AppError::auth);
    }
    let entry = keyring::Entry::new("mx.nuncio.rebuild", name).map_err(|_| AppError::auth())?;
    entry
        .get_password()
        .map(Zeroizing::new)
        .map_err(|_| AppError::auth())
}
