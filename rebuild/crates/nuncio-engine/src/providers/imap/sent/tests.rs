#![allow(clippy::unwrap_used)]
use super::*;
use crate::domain::imap_account::{MailEndpoint, MailTls, SentPolicy};
use crate::{
    domain::{
        identity::{AccountId, ImapMailboxId},
        imap::MailboxName,
    },
    providers::imap::{
        wire::{ImapWire, Wire},
        Plain,
    },
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
};

#[tokio::test]
async fn appenduid_requires_exact_epoch_single_uid_and_valid_tag() {
    for (response, acknowledged, uid) in [
        ("{tag} OK [APPENDUID 9010 3] copied\r\n", true, Some(3)),
        ("* OK [APPENDUID 9010 3:3] copied\r\n", false, Some(3)),
        ("{tag} OK [APPENDUID 9011 3] wrong epoch\r\n", false, None),
        ("{tag} OK [APPENDUID 9010 0] zero\r\n", false, None),
        (
            "{tag} OK [APPENDUID 9010 1:4294967295] oversized set\r\n",
            false,
            None,
        ),
        (
            "* OK [APPENDUID 9010 3] copied\r\nWRONG OK completed\r\n",
            false,
            None,
        ),
        ("{tag} OK no mapping\r\n", true, None),
        (
            "* OK [APPENDUID 9010 3] copied\r\n{tag} OK [APPENDUID 9010 4] conflicting mapping\r\n",
            false,
            None,
        ),
        ("{tag} NO rejected\r\n", false, None),
        (
            "* OK [APPENDUID 9010 3] copied\r\n{tag} NO conflicting completion\r\n",
            false,
            Some(3),
        ),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let tcp = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let raw = b"Subject: frozen\r\n\r\nBody\r\n";
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
            let (tag, command) = line.trim_end().split_once(' ').unwrap();
            assert_eq!(
                command,
                format!("APPEND \"Sent\" (\\Seen) {{{}}}", raw.len())
            );
            let tag = tag.to_string();
            peer.get_mut()
                .write_all(b"+ ready for literal\r\n")
                .await
                .unwrap();
            let mut literal = vec![0; raw.len() + 2];
            peer.read_exact(&mut literal).await.unwrap();
            assert_eq!(&literal[..raw.len()], raw);
            assert_eq!(&literal[raw.len()..], b"\r\n");
            peer.get_mut()
                .write_all(response.replace("{tag}", &tag).as_bytes())
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
            .map_err(|(e, _)| e)
            .unwrap();
        let mut connection = Connection {
            session,
            capabilities: Default::default(),
        };
        let intent = SmtpIntent {
            sent_fingerprint: None,
            account_id: AccountId::generate(),
            endpoint: MailEndpoint {
                host: "localhost".into(),
                port: 587,
                tls: MailTls::StartTls,
                username: "synthetic".into(),
            },
            sent_policy: SentPolicy::ClientAppend,
            mailbox_id: ImapMailboxId::generate(),
            mailbox: MailboxName::from_unicode("Sent").unwrap(),
            uid_validity: std::num::NonZeroU32::new(9010).unwrap(),
            uid_next: std::num::NonZeroU32::new(1).unwrap(),
        };
        let result = append(&mut connection, &intent, raw).await;
        peer.await.unwrap();
        assert_eq!(result.acknowledged, acknowledged, "{response}");
        assert_eq!(
            result.rejected,
            response == "{tag} NO rejected\r\n",
            "{response}"
        );
        assert_eq!(result.placement.map(|p| p.uid.get()), uid, "{response}");
    }
}
