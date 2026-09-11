use std::io::Write;

pub struct AppError {
    pub recovery: Option<serde_json::Value>,
    pub operation: Option<Box<nuncio_proto::v2::Operation>>,
    pub sync_run: Option<Box<nuncio_proto::v2::SyncRun>>,
    pub code: &'static str,
    pub message: &'static str,
    pub exit: u8,
}

impl AppError {
    pub fn with_operation(mut self, operation: &nuncio_proto::v2::Operation) -> Self {
        self.operation = Some(Box::new(operation.clone()));
        self
    }
    pub fn with_sync(mut self, run: &nuncio_proto::v2::SyncRun) -> Self {
        self.sync_run = Some(Box::new(run.clone()));
        self
    }
    pub fn invalid() -> Self {
        Self {
            code: "invalid_input",
            message: "Invalid command or argument",
            sync_run: None,
            operation: None,
            recovery: None,
            exit: 2,
        }
    }
    pub fn auth() -> Self {
        Self {
            code: "authorization_required",
            message: "Profile authorization required",
            sync_run: None,
            operation: None,
            recovery: None,
            exit: 3,
        }
    }
    pub fn unavailable() -> Self {
        Self {
            code: "unavailable",
            message: "Daemon is unavailable",
            sync_run: None,
            operation: None,
            recovery: None,
            exit: 4,
        }
    }
}

pub fn emit(result: Result<serde_json::Value, AppError>, json: bool) -> std::process::ExitCode {
    let (value, exit) = match result {
        Ok(value) => (serde_json::json!({"schema_version":1, "result":value}), 0),
        Err(error) => {
            let _ = writeln!(std::io::stderr().lock(), "{}", error.message);
            let mut detail = serde_json::json!({"code":error.code, "message":error.message, "retryable":error.exit == 4});
            if let Some(run) = error.sync_run {
                detail["sync_run"] = serde_json::json!(run);
            }
            if let Some(operation) = error.operation {
                detail["operation"] = serde_json::json!(operation);
            }
            if let Some(recovery) = error.recovery {
                detail["recovery"] = recovery;
            }
            (
                serde_json::json!({"schema_version":1, "error":detail}),
                error.exit,
            )
        }
    };
    let rendered = render(&value, json);
    match rendered {
        Ok(rendered) if writeln!(std::io::stdout().lock(), "{rendered}").is_ok() => {
            std::process::ExitCode::from(exit)
        }
        _ => std::process::ExitCode::from(1),
    }
}
pub(crate) fn render(value: &serde_json::Value, json: bool) -> Result<String, serde_json::Error> {
    let rendered = if json {
        serde_json::to_string(&value)
    } else {
        serde_json::to_string_pretty(&value)
    }?;
    // JSON already quotes C0 controls. Also quote C1 and direction controls so
    // terminal output is inert while a JSON consumer can recover original text.
    let mut safe = String::with_capacity(rendered.len());
    for ch in rendered.chars() {
        if matches!(ch,'\u{7f}'..='\u{9f}'|'\u{202a}'..='\u{202e}'|'\u{2066}'..='\u{2069}') {
            use std::fmt::Write;
            let _ = write!(safe, "\\u{:04x}", ch as u32);
        } else {
            safe.push(ch)
        }
    }
    Ok(safe)
}
#[cfg(test)]
mod tests {
    #[test]
    #[allow(clippy::unwrap_used)]
    fn terminal_controls_are_escaped_without_changing_json_content() {
        let value = serde_json::json!({"subject":"a\u{1b}[2J\u{9b}2J\u{202e}hidden"});
        for json in [true, false] {
            let output = super::render(&value, json).unwrap();
            assert!(!output.contains(['\u{1b}', '\u{9b}', '\u{202e}']));
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&output).unwrap(),
                value
            );
        }
    }
}
