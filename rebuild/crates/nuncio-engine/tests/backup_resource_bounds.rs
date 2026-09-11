#![allow(clippy::unwrap_used)]
use nuncio_engine::store::{inspect_backup, stage_restore, StoreError};
use zeroize::Zeroizing;

#[test]
fn empty_and_oversized_sparse_backups_are_rejected_without_copying_or_replacing_files() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("source.nuncio");
    let target = temporary.path().join("restore-parent");
    std::fs::create_dir(&target).unwrap();
    let original = target.join("keep-original");
    std::fs::write(&original, b"original sentinel").unwrap();
    for length in [0, (1u64 << 40) + 1] {
        std::fs::File::create(&source)
            .unwrap()
            .set_len(length)
            .unwrap();
        let phrase = || Zeroizing::new("synthetic resource limit passphrase".into());
        let inspection = inspect_backup(&source, phrase());
        let staged = stage_restore(
            &source,
            phrase(),
            Zeroizing::new(vec![0x41; 32]),
            &target,
            1000,
        );
        if length == 0 {
            assert!(matches!(inspection, Err(StoreError::InvalidInput)));
            assert!(matches!(staged, Err(StoreError::InvalidInput)));
        } else {
            assert!(matches!(inspection, Err(StoreError::ResultTooLarge)));
            assert!(matches!(staged, Err(StoreError::ResultTooLarge)));
        }
        assert_eq!(std::fs::metadata(&source).unwrap().len(), length);
        assert_eq!(std::fs::read(&original).unwrap(), b"original sentinel");
        assert_eq!(
            std::fs::read_dir(&target).unwrap().count(),
            1,
            "failed staging must remove its owned files"
        );
    }
}
