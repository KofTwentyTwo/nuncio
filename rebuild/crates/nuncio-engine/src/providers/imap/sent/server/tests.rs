#![allow(clippy::unwrap_used, clippy::panic)]
use super::*;
use crate::{
    domain::{
        identity::{AccountId, ImapMailboxId},
        imap::MailboxName,
        imap_account::{MailEndpoint, MailTls, SentPolicy},
    },
    providers::imap::{
        wire::{ImapWire, Wire},
        Plain,
    },
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
};

#[tokio::test]
async fn sent_search_is_bounded_by_sequence_windows_and_rejects_stale_or_ambiguous_uids() {
    for (count, next, replies, expected) in [
        (
            4097,
            9000,
            vec![
                "* SEARCH\r\n{tag} OK done\r\n",
                "* SEARCH 8500\r\n{tag} OK done\r\n",
            ],
            "one",
        ),
        (
            4097,
            9000,
            vec![
                "* SEARCH 8001\r\n{tag} OK done\r\n",
                "* SEARCH 8500\r\n{tag} OK done\r\n",
            ],
            "ambiguous",
        ),
        (
            1,
            9000,
            vec!["* SEARCH 7999\r\n{tag} OK done\r\n"],
            "protocol",
        ),
        (
            1,
            9000,
            vec!["* SEARCH 9000\r\n{tag} OK done\r\n"],
            "protocol",
        ),
        (
            1,
            9000,
            vec!["* SEARCH 8500\r\n* SEARCH 8500\r\n{tag} OK done\r\n"],
            "protocol",
        ),
        (
            1,
            9000,
            vec!["* SEARCH 8500\r\nWRONG OK done\r\n"],
            "protocol",
        ),
        (1, 9000, vec!["* SEARCH 8500\r\n"], "unavailable"),
        (
            1,
            9000,
            vec!["* SEARCH 8500\r\n{tag} NO failed\r\n"],
            "unavailable",
        ),
        (1_000_001, 9000, vec![], "protocol"),
        (4097, 8000, vec![], "empty"),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let tcp = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let peer = tokio::spawn(async move {
            tokio::time::timeout(std::time::Duration::from_secs(5), async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut peer = BufReader::new(tcp);
            let mut line = String::new();
            peer.read_line(&mut line).await.unwrap();
            let tag = line.split_once(' ').unwrap().0.to_owned();
            peer.get_mut().write_all(b"+\r\n").await.unwrap();
            line.clear();
            peer.read_line(&mut line).await.unwrap();
            peer.get_mut()
                .write_all(format!("{tag} OK authenticated\r\n").as_bytes())
                .await
                .unwrap();
            line.clear();
            peer.read_line(&mut line).await.unwrap();
            let (tag, command) = line.trim_end().split_once(' ').unwrap();
            assert_eq!(command, "LIST \"\" \"*\"");
            peer.get_mut()
                .write_all(
                    format!("* LIST () \"/\" \"INBOX\"\r\n* LIST (\\Sent) \"/\" \"Sent\"\r\n{tag} OK listed\r\n").as_bytes(),
                )
                .await
                .unwrap();
            line.clear();
            peer.read_line(&mut line).await.unwrap();
            let (tag, command) = line.trim_end().split_once(' ').unwrap();
            assert_eq!(command, "EXAMINE \"Sent\"");
            peer.get_mut().write_all(format!("* {count} EXISTS\r\n* OK [UIDVALIDITY 9010] epoch\r\n* OK [UIDNEXT {next}] next\r\n{tag} OK [READ-ONLY] selected\r\n").as_bytes()).await.unwrap();
            for (index, response) in replies.into_iter().enumerate() {
                line.clear();
                peer.read_line(&mut line).await.unwrap();
                let (tag, command) = line.trim_end().split_once(' ').unwrap();
                let low = index * 4096 + 1;
                let high = ((index + 1) * 4096).min(count as usize);
                assert_eq!(command,format!("UID SEARCH {low}:{high} UID 8000:{} HEADER Message-ID \"<one@nuncio.invalid>\"",next-1));
                peer.get_mut()
                    .write_all(response.replace("{tag}", tag).as_bytes())
                    .await
                    .unwrap();
            }
            }).await.unwrap();
        });
        let wire = ImapWire::new(
            Wire::Plain(tcp),
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
            .map_err(|(e, _)| e)
            .unwrap();
        let mut connection = Connection {
            session,
            capabilities: Default::default(),
        };
        let intent = SmtpIntent {
            account_id: AccountId::generate(),
            endpoint: MailEndpoint {
                host: "localhost".into(),
                port: 587,
                tls: MailTls::StartTls,
                username: "synthetic".into(),
            },
            sent_policy: SentPolicy::Server,
            mailbox_id: ImapMailboxId::generate(),
            mailbox: MailboxName::from_unicode("Sent").unwrap(),
            uid_validity: NonZeroU32::new(9010).unwrap(),
            uid_next: NonZeroU32::new(1).unwrap(),
            sent_fingerprint: None,
        };
        let result = search(
            &mut connection,
            &intent,
            NonZeroU32::new(8000).unwrap(),
            "one@nuncio.invalid",
        )
        .await;
        drop(connection);
        peer.await.unwrap();
        match expected {
            "one" => assert_eq!(result.unwrap(), vec![8500]),
            "empty" => assert!(result.unwrap().is_empty()),
            "ambiguous" => assert!(matches!(
                result,
                Err(TransferError::Conflict("smtp_server_sent_ambiguous"))
            )),
            "protocol" => assert!(
                matches!(result, Err(TransferError::Wire(MailError::Protocol))),
                "{result:?}"
            ),
            "unavailable" => assert!(matches!(
                result,
                Err(TransferError::Wire(MailError::Unavailable))
            )),
            _ => panic!("invalid test case"),
        }
    }
}
