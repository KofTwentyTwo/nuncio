fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = tonic_prost_build::Config::new();
    config.protoc_executable(protoc_bin_vendored::protoc_bin_path()?);
    config.skip_source_info();
    for name in [
        "ResourceStatus",
        "BackupInspection",
        "RepairProjectionResponse",
        "ProjectionCounts",
        "PreservedStateCounts",
        "AgendaWindow",
        "RestoreBackupResponse",
        "MailCapabilities",
        "ImapCapabilities",
        "Draft",
        "Operation",
        "OperationSummary",
        "OperationAttempt",
        "OperationReceipt",
        "OperationResolution",
        "OperationReconciliation",
        "ListOperationsResponse",
        "ListOperationAttemptsResponse",
        "DraftContext",
        "DraftSummary",
        "DraftContent",
        "DraftRecipient",
        "DraftAttachment",
        "ListDraftsResponse",
        "DeleteDraftResponse",
        "SyncRun",
        "SyncScopeStatus",
        "ChangeEvent",
        "MailCollection",
        "MailSummary",
        "MailAttachment",
        "MailHeader",
        "MailCoverage",
        "ListMailResponse",
        "GetMailResponse",
        "ListCollectionsResponse",
        "CalendarSummary",
        "ListCalendarsResponse",
        "AgendaCoverage",
        "FreeBusyResult",
        "CalendarAvailability",
        "BusyPeriod",
        "AvailabilityError",
    ] {
        config.type_attribute(format!(".nuncio.v2.{name}"), "#[derive(serde::Serialize)]");
    }
    for name in ["DraftContent", "DraftRecipient"] {
        config.type_attribute(
            format!(".nuncio.v2.{name}"),
            "#[derive(serde::Deserialize)] #[serde(default,deny_unknown_fields)]",
        );
    }
    config.type_attribute(
        ".nuncio.v2.OperationConfirmation",
        "#[derive(serde::Deserialize)] #[serde(deny_unknown_fields)]",
    );
    config.skip_debug([
        ".nuncio.v2.RecoverySecret",
        ".nuncio.v2.BeginGoogleAuthRequest",
        ".nuncio.v2.ImapCredentials",
        ".nuncio.v2.ConnectImapRequest",
    ]);
    config.type_attribute(
        ".nuncio.v2.RecoverySecret",
        "#[derive(zeroize::Zeroize, zeroize::ZeroizeOnDrop)]",
    );
    config.type_attribute(
        ".nuncio.v2.ImapCredentials",
        "#[derive(zeroize::Zeroize, zeroize::ZeroizeOnDrop)]",
    );
    config.type_attribute(
        ".nuncio.v2.BeginGoogleAuthRequest",
        "#[derive(zeroize::Zeroize, zeroize::ZeroizeOnDrop)]",
    );
    config.file_descriptor_set_path(
        std::path::PathBuf::from(std::env::var("OUT_DIR")?).join("nuncio_v2.bin"),
    );
    let files = [
        "proto/nuncio/v2/system.proto",
        "proto/nuncio/v2/accounts.proto",
        "proto/nuncio/v2/mail.proto",
        "proto/nuncio/v2/calendar.proto",
        "proto/nuncio/v2/operations.proto",
        "proto/nuncio/v2/maintenance.proto",
    ];
    for file in files {
        println!("cargo:rerun-if-changed={file}");
    }
    tonic_prost_build::configure().compile_with_config(config, &files, &["proto"])?;
    Ok(())
}
