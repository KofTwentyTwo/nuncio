use serde_json::Value;
use std::fmt::Write;

pub fn render(value: &Value) -> String {
    let mut output = String::new();
    fields(&mut output, value, 0);
    output.trim_end().to_owned()
}

fn fields(output: &mut String, value: &Value, depth: usize) {
    let indent = "  ".repeat(depth);
    match value {
        Value::Object(values) if !values.is_empty() => {
            for (key, value) in values {
                if key.ends_with("_at_ms") || key == "internal_date_ms" {
                    if let Some(time) = value
                        .as_i64()
                        .and_then(chrono::DateTime::from_timestamp_millis)
                    {
                        let _ = writeln!(
                            output,
                            "{indent}{}: {}",
                            label(key.trim_end_matches("_ms")),
                            time.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                        );
                        continue;
                    }
                }
                let label = label(key);
                match value {
                    Value::String(text) if text.contains('\n') => {
                        let _ = writeln!(output, "{indent}{label}:");
                        for line in text.lines() {
                            let _ = writeln!(output, "{indent}  | {}", safe(line));
                        }
                    }
                    Value::Array(items) if items.is_empty() => {
                        let _ = writeln!(output, "{indent}{label}: No items");
                    }
                    Value::Array(_) | Value::Object(_) => {
                        let _ = writeln!(output, "{indent}{label}:");
                        fields(output, value, depth + 1);
                    }
                    _ => {
                        let _ = writeln!(output, "{indent}{label}: {}", scalar(value));
                    }
                }
            }
        }
        Value::Array(items) if !items.is_empty() => {
            for (index, item) in items.iter().enumerate() {
                if matches!(item, Value::Object(_) | Value::Array(_)) {
                    let _ = writeln!(output, "{indent}{}.", index + 1);
                    fields(output, item, depth + 1);
                } else {
                    let _ = writeln!(output, "{indent}- {}", scalar(item));
                }
            }
        }
        Value::Array(_) => {
            let _ = writeln!(output, "{indent}No items");
        }
        Value::Object(_) => {
            let _ = writeln!(output, "{indent}No details");
        }
        _ => {
            let _ = writeln!(output, "{indent}{}", scalar(value));
        }
    }
}

fn label(key: &str) -> String {
    let mut words: Vec<_> = key.split('_').map(str::to_owned).collect();
    for (index, word) in words.iter_mut().enumerate() {
        if matches!(
            word.as_str(),
            "id" | "api" | "url" | "tls" | "uid" | "smtp" | "imap"
        ) {
            *word = word.to_uppercase();
        } else if index == 0 {
            let mut letters = word.chars();
            if let Some(first) = letters.next() {
                *word = first.to_uppercase().collect::<String>() + letters.as_str();
            }
        }
    }
    safe(&words.join(" "))
}

fn scalar(value: &Value) -> String {
    match value {
        Value::Null => "Not set".into(),
        Value::Bool(true) => "yes".into(),
        Value::Bool(false) => "no".into(),
        Value::String(text) if text.is_empty() => "(empty)".into(),
        Value::String(text) => safe(text),
        _ => value.to_string(),
    }
}

fn safe(text: &str) -> String {
    let mut output = String::new();
    for character in text.chars() {
        if character.is_control()
            || matches!(character, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        {
            let _ = write!(output, "\\u{:04x}", character as u32);
        } else {
            output.push(character);
        }
    }
    output
}
