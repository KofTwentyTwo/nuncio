use clap::error::{ContextKind, ErrorKind};
use clap::CommandFactory;
use std::io::Write;

/// Use parser-owned argument names and usage, never rejected user values.
pub fn emit(error: &clap::Error) -> std::process::ExitCode {
    let reason = match error.kind() {
        ErrorKind::MissingRequiredArgument => "Missing required arguments",
        ErrorKind::MissingSubcommand => "Choose a command from the help below",
        ErrorKind::UnknownArgument => "Unrecognized option or extra argument",
        ErrorKind::InvalidSubcommand => "Unrecognized command",
        ErrorKind::InvalidValue | ErrorKind::ValueValidation => "An option has an invalid value",
        ErrorKind::ArgumentConflict => "These options cannot be used together",
        ErrorKind::TooManyValues | ErrorKind::TooFewValues | ErrorKind::WrongNumberOfValues => {
            "An option has the wrong number of values"
        }
        _ => "Invalid command or argument",
    };
    let mut text = format!("Error: {reason}.\n");
    // For these kinds InvalidArg describes a declared option. For unknown
    // arguments/subcommands it contains user input and must not be printed.
    if matches!(
        error.kind(),
        ErrorKind::MissingRequiredArgument
            | ErrorKind::InvalidValue
            | ErrorKind::ValueValidation
            | ErrorKind::ArgumentConflict
            | ErrorKind::TooFewValues
            | ErrorKind::TooManyValues
            | ErrorKind::WrongNumberOfValues
    ) {
        if let Some(arg) = error.get(ContextKind::InvalidArg) {
            text.push_str(&format!("  {arg}\n"));
        }
    }
    let (mut command, path) = selected_command();
    if let Some(clap::error::ContextValue::String(suggestion)) =
        error.get(ContextKind::SuggestedArg)
    {
        if let Some(name) = command
            .get_arguments()
            .filter_map(|argument| argument.get_long())
            .find(|name| suggestion == &format!("--{name}"))
        {
            text.push_str(&format!("Did you mean '--{name}'?\n"));
        }
    }

    if matches!(
        error.kind(),
        ErrorKind::InvalidValue | ErrorKind::ValueValidation
    ) {
        if let Some(clap::error::ContextValue::String(argument)) =
            error.get(ContextKind::InvalidArg)
        {
            let name = argument
                .split_whitespace()
                .next()
                .and_then(|name| name.strip_prefix("--"));
            if let Some(help) = command
                .get_arguments()
                .find(|arg| arg.get_long() == name)
                .and_then(|arg| arg.get_help())
            {
                text.push_str(&format!("Expected: {help}\n"));
            }
        }
    }
    text.push_str(&format!("\n{}\n", command.render_usage()));
    if text.contains("--account") {
        text.push_str("\nFind the local account ID with 'nuncio-cli account list'.\nUse the same --profile as your daemon; --account selects one account inside it.\nExample: nuncio-cli --profile laptop-qa mail list --account ACCOUNT_ID\n");
    }
    text.push_str(&format!(
        "\nRun '{path} --help' for its options and examples.\n"
    ));
    match std::io::stderr().lock().write_all(text.as_bytes()) {
        Ok(()) => std::process::ExitCode::from(2),
        Err(_) => std::process::ExitCode::from(1),
    }
}

fn selected_command() -> (clap::Command, String) {
    let mut command = crate::args::Args::command();
    command.build();
    let mut path = String::from("nuncio-cli");
    let mut arguments = std::env::args_os().skip(1);
    while let Some(argument) = arguments.next() {
        let Some(argument) = argument.to_str() else {
            break;
        };
        if argument == "--" {
            break;
        }
        if let Some(option) = argument.strip_prefix("--") {
            let (name, inline) = option
                .split_once('=')
                .map_or((option, false), |(name, _)| (name, true));
            let Some(option) = command.get_arguments().find(|a| a.get_long() == Some(name)) else {
                break;
            };
            if option.get_action().takes_values() && !inline {
                arguments.next();
            }
            continue;
        }
        let Some(next) = command
            .get_subcommands()
            .find(|child| {
                child.get_name() == argument
                    || child.get_all_aliases().any(|alias| alias == argument)
            })
            .cloned()
        else {
            break;
        };
        path.push(' ');
        path.push_str(next.get_name());
        command = next;
    }
    (command.bin_name(path.clone()), path)
}
