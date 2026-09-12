#![allow(clippy::unwrap_used)]
use super::super::{
    wire::{ImapWire, Wire},
    Plain,
};
use super::*;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
};
#[tokio::test]
async fn sparse_high_uids_are_bounded_by_message_count_not_uid_number_space() {
    for (reply, valid) in [
        ("* SEARCH 4000000000\r\n", true),
        ("* 1 EXPUNGE\r\n* SEARCH 4000000000\r\n", false),
        ("* SEARCH 4000000000 4000000000\r\n", false),
        ("* SEARCH 4294967295\r\n", false),
        ("* SEARCH 0\r\n", false),
        ("* SEARCH\r\n", false),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let tcp = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let peer = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut peer = BufReader::new(tcp);
            let mut line = String::new();
            peer.read_line(&mut line).await.unwrap();
            let tag = line.split_once(' ').unwrap().0.to_string();
            peer.get_mut().write_all(b"+\r\n").await.unwrap();
            line.clear();
            peer.read_line(&mut line).await.unwrap();
            peer.get_mut()
                .write_all(format!("{tag} OK authenticated\r\n").as_bytes())
                .await
                .unwrap();
            line.clear();
            peer.read_line(&mut line).await.unwrap();
            let (tag, query) = line.trim_end().split_once(' ').unwrap();
            assert_eq!(
                query, "UID SEARCH 1:1 UID 1:4294967294",
                "one extant message requires one bounded sequence search"
            );
            peer.get_mut()
                .write_all(format!("{reply}{tag} OK complete\r\n").as_bytes())
                .await
                .unwrap();
        });
        let wire = ImapWire::new(
            Wire::test(tcp),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            262144,
            0,
        )
        .unwrap();
        let session = async_imap::Client::new(wire)
            .authenticate(
                "PLAIN",
                Plain {
                    bytes: zeroize::Zeroizing::new(b"\0synthetic\0synthetic".to_vec()),
                    used: false,
                },
            )
            .await
            .map_err(|(error, _)| error)
            .unwrap();
        let mut connection = Connection {
            session,
            capabilities: Default::default(),
        };
        let found = inventory(&mut connection, u32::MAX, 1).await;
        peer.await.unwrap();
        if valid {
            assert_eq!(found.unwrap(), [4_000_000_000]);
        } else {
            assert!(found.is_err());
        }
    }
}
