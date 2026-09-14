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
    pub fn input_context(mut self, message: &'static str) -> Self {
        if self.code == "invalid_input" {
            self.message = message;
        }
        self
    }
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
            message: "Input was rejected; check the required options, IDs and file format with this command's --help",
            sync_run: None,
            operation: None,
            recovery: None,
            exit: 2,
        }
    }
    pub fn auth() -> Self {
        Self {
            code: "authorization_required",
            message: "Profile authorization is unavailable; start the daemon with the same --profile and unlock your OS credential store",
            sync_run: None,
            operation: None,
            recovery: None,
            exit: 3,
        }
    }
    pub fn unavailable() -> Self {
        Self {
            code: "unavailable",
            message: "Cannot reach Nuncio; start 'nunciod --profile PROFILE' and use the same --profile and --endpoint here",
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
            if !json {
                return emit_human_error(error);
            }
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
fn emit_human_error(error: AppError) -> std::process::ExitCode {
    let mut text = format!("Error: {}\n", error.message);
    if let Some(operation) = error.operation {
        let detail = serde_json::json!({"operation_id":operation.id,"account_id":operation.account_id,
            "state":operation.state,"error_code":operation.error_code,"needs_reconciliation":operation.needs_reconciliation});
        text.push_str(&super::human::render(&detail));
        text.push_str("\nInspect with: nuncio-cli operation show --account ACCOUNT_ID --operation OPERATION_ID\n");
    }
    if let Some(run) = error.sync_run {
        let detail = serde_json::json!({"sync_run_id":run.id,"account_id":run.account_id,
            "state":run.state,"error_code":run.error_code});
        text.push_str(&super::human::render(&detail));
        text.push_str("\nInspect with: nuncio-cli system sync-status --account ACCOUNT_ID --run SYNC_RUN_ID\n");
    }
    if let Some(recovery) = error.recovery {
        text.push_str("Recovery details:\n");
        text.push_str(&super::human::render(&recovery));
        text.push('\n');
    }
    match std::io::stderr().lock().write_all(text.as_bytes()) {
        Ok(()) => std::process::ExitCode::from(error.exit),
        Err(_) => std::process::ExitCode::from(1),
    }
}

pub fn emit_setup(result: Result<serde_json::Value, AppError>) -> std::process::ExitCode {
    match result {
        Ok(value) => {
            let address = render(&value["account"]["address"], true);
            let Ok(address) = address else {
                return std::process::ExitCode::from(1);
            };
            let text = format!("Account connected: {address}\nBackground sync is enabled. Use account list to view your accounts.");
            if writeln!(std::io::stdout().lock(), "{text}").is_err() {
                return std::process::ExitCode::from(1);
            }
            if value["credential_cleanup_pending"] == true {
                let _ = writeln!(
                    std::io::stderr().lock(),
                    "An older credential still needs cleanup. Run account check for details."
                );
            }
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            let _ = writeln!(std::io::stderr().lock(), "{}", error.message);
            std::process::ExitCode::from(error.exit)
        }
    }
}

pub(crate) fn render(value: &serde_json::Value, json: bool) -> Result<String, serde_json::Error> {
    if !json {
        return Ok(super::human::render(value.get("result").unwrap_or(value)));
    }
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
    fn human_dates_are_utc_and_durations_stay_numeric() {
        let value = serde_json::json!({"created_at_ms":0,"elapsed_ms":1500});
        let output = super::render(&value, false).unwrap();
        assert!(
            output.contains("Created at: 1970-01-01T00:00:00.000Z"),
            "{output}"
        );
        assert!(output.contains("Elapsed ms: 1500"), "{output}");
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn human_multiline_content_stays_visibly_inside_its_field() {
        let value = serde_json::json!({"text":"Hello\nState: applied\n\u{1b}[2J"});
        let output = super::render(&value, false).unwrap();
        assert!(
            output.contains("Text:\n  | Hello\n  | State: applied"),
            "{output}"
        );
        assert!(!output.contains('\u{1b}'));
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn human_output_keeps_ids_pagination_and_empty_results_readable() {
        let value = serde_json::json!({"schema_version":1,"result":{
            "items":[{"id":"message-17","subject":"Meeting notes","read":false}],
            "next_page_token":"page-2","revision":7}});
        let output = super::render(&value, false).unwrap();
        assert!(output.contains("Subject: Meeting notes"), "{output}");
        assert!(output.contains("ID: message-17"), "{output}");
        assert!(output.contains("Read: no"), "{output}");
        assert!(output.contains("Next page token: page-2"), "{output}");
        assert!(!output.contains("schema_version"));
        assert!(!output.trim_start().starts_with('{'));
        let empty = super::render(&serde_json::json!({"items":[]}), false).unwrap();
        assert!(empty.contains("No items"), "{empty}");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&super::render(&value, true).unwrap())
                .unwrap(),
            value
        );
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn terminal_controls_are_escaped_without_changing_json_content() {
        let value = serde_json::json!({"subject":"a\u{1b}[2J\u{9b}2J\u{202e}hidden"});
        for json in [true, false] {
            let output = super::render(&value, json).unwrap();
            assert!(!output.contains(['\u{1b}', '\u{9b}', '\u{202e}']));
            if json {
                assert_eq!(
                    serde_json::from_str::<serde_json::Value>(&output).unwrap(),
                    value
                );
            } else {
                assert!(output.contains("hidden"));
            }
        }
    }
}
