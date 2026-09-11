#![allow(clippy::unwrap_used, clippy::panic)]
#[test]
fn assertion_failure_closes_and_joins_the_storage_worker() {
    if std::env::var_os("NUNCIO_STORE_OWNER_FAILURE_CHILD").is_some() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let temp = tempfile::tempdir().unwrap();
            let store = nuncio_engine::store::Store::open(
                temp.path(),
                zeroize::Zeroizing::new(vec![0x57; 32]),
            )
            .await
            .unwrap();
            let _reader = store.watch_changes(0).await.unwrap();
            panic!("intentional assertion failure exercises storage-owner unwinding");
        });
        return;
    }
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "assertion_failure_closes_and_joins_the_storage_worker",
            "--nocapture",
        ])
        .env("NUNCIO_STORE_OWNER_FAILURE_CHILD", "1")
        .output()
        .unwrap();
    assert_eq!(
        result.status.code(),
        Some(101),
        "A failed assertion must exit normally, without a teardown signal"
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("FAILED"));
}
