#![allow(clippy::unwrap_used)]
use nuncio_engine::store::{stage_restore, Store};
use zeroize::Zeroizing;
fn phrase() -> Zeroizing<String> {
    Zeroizing::new("synthetic restore path race recovery phrase".into())
}
#[tokio::test]
async fn replaced_restore_paths_are_neither_activated_nor_removed_during_cleanup() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(&temp.path().join("source"), Zeroizing::new(vec![0x58; 32]))
        .await
        .unwrap();
    let backup = store.create_backup(phrase(), 1000).await.unwrap();
    let original = std::fs::read(backup.path()).unwrap();
    for activate in [true, false] {
        for scenario in ["parent", "stage", "parent_symlink"] {
            let parent = temp.path().join(format!("{scenario}-{activate}"));
            std::fs::create_dir(&parent).unwrap();
            let staged = stage_restore(
                backup.path(),
                phrase(),
                Zeroizing::new(vec![0x59; 32]),
                &parent,
                1001,
            )
            .unwrap();
            let stage = staged.directory().to_owned();
            let name = stage.file_name().unwrap().to_owned();
            let cipher = std::fs::read(stage.join("store.db")).unwrap();
            let relocated = temp.path().join(format!("relocated-{scenario}-{activate}"));
            let actual = if scenario == "stage" {
                std::fs::rename(&stage, &relocated).unwrap();
                relocated
            } else {
                std::fs::rename(&parent, &relocated).unwrap();
                if scenario == "parent_symlink" {
                    let substitute = temp.path().join(format!("substitute-{activate}"));
                    std::fs::create_dir(&substitute).unwrap();
                    std::os::unix::fs::symlink(&substitute, &parent).unwrap();
                } else {
                    std::fs::create_dir(&parent).unwrap();
                }
                relocated.join(name)
            };
            std::fs::create_dir(&stage).unwrap();
            std::fs::write(stage.join("unrelated"), b"unrelated concurrent directory").unwrap();
            let target = parent.join("restored");
            if activate {
                assert!(
                    staged.activate(&target).is_err(),
                    "activated replaced {scenario}"
                );
            } else {
                drop(staged);
            }
            assert!(!target.exists(), "published substituted {scenario}");
            assert_eq!(
                std::fs::read(stage.join("unrelated")).unwrap(),
                b"unrelated concurrent directory",
                "cleanup removed unrelated {scenario}"
            );
            assert_eq!(std::fs::read_dir(&stage).unwrap().count(), 1);
            assert_eq!(std::fs::read(actual.join("store.db")).unwrap(), cipher);
            assert_eq!(std::fs::read(backup.path()).unwrap(), original);
        }
    }
    store.close().await.unwrap();
}
