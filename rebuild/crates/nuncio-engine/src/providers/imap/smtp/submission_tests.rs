#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
use super::submission::{validate_data, DataResult, SubmissionError};
use super::*;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
};

#[tokio::test]
async fn data_is_dot_stuffed_once_and_only_complete_final_replies_classify_delivery() {
    for (reply, expected) in [
        (
            Some(b"250 accepted\r\n".as_slice()),
            DataResult::Accepted(250),
        ),
        (
            Some(b"450 not accepted\r\n".as_slice()),
            DataResult::Rejected(450),
        ),
        (
            Some(b"550 rejected\r\n".as_slice()),
            DataResult::Rejected(550),
        ),
        (None, DataResult::Unknown),
        (Some(b"350 incomplete\r\n".as_slice()), DataResult::Unknown),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let socket = TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(socket);
            for (expected, reply) in [
                (
                    b"MAIL FROM:<alpha@example.test>\r\n".as_slice(),
                    b"250 OK\r\n".as_slice(),
                ),
                (b"RCPT TO:<beta@example.test>\r\n", b"250 OK\r\n"),
                (b"DATA\r\n", b"354 go\r\n"),
            ] {
                let mut line = Vec::new();
                reader.read_until(b'\n', &mut line).await.unwrap();
                assert_eq!(line, expected);
                reader.get_mut().write_all(reply).await.unwrap();
            }
            let mut data = Vec::new();
            loop {
                let mut line = Vec::new();
                assert!(reader.read_until(b'\n', &mut line).await.unwrap() > 0);
                if line == b".\r\n" {
                    break;
                }
                data.extend(line);
            }
            if let Some(reply) = reply {
                reader.get_mut().write_all(reply).await.unwrap();
            }
            data
        });
        let session = Session {
            wire: Wire::Plain(socket),
            caps: BTreeSet::new(),
        };
        let prepared = session
            .prepare(
                "alpha@example.test",
                &["beta@example.test".into()],
                b"From: alpha@example.test\r\n\r\n.one\r\n..two\r\n.\r\n",
            )
            .await
            .unwrap();
        assert_eq!(prepared.transmit().await, expected);
        assert_eq!(
            server.await.unwrap(),
            b"From: alpha@example.test\r\n\r\n..one\r\n...two\r\n..\r\n"
        );
    }
}

#[tokio::test]
async fn partial_recipient_rejection_never_sends_data() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let socket = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(socket);
        let mut commands = Vec::new();
        for reply in [
            b"250 OK\r\n".as_slice(),
            b"250 OK\r\n",
            b"550 recipient rejected\r\n",
        ] {
            let mut line = Vec::new();
            reader.read_until(b'\n', &mut line).await.unwrap();
            commands.push(line);
            reader.get_mut().write_all(reply).await.unwrap();
        }
        let mut line = Vec::new();
        assert_eq!(reader.read_until(b'\n', &mut line).await.unwrap(), 0);
        commands
    });
    let session = Session {
        wire: Wire::Plain(socket),
        caps: BTreeSet::new(),
    };
    let result = session
        .prepare(
            "alpha@example.test",
            &["first@example.test".into(), "rejected@example.test".into()],
            b"From: alpha@example.test\r\n\r\nbody\r\n",
        )
        .await;
    assert!(matches!(result, Err(SubmissionError::Rejected(550))));
    let commands = server.await.unwrap();
    assert_eq!(commands.len(), 3);
    assert!(commands.iter().all(|c| !c.starts_with(b"DATA")));
}

#[test]
fn smtp_payload_validation_preserves_canonical_lines_and_requires_extensions() {
    for raw in [
        b"From: a@example.test\n\nbody\n".as_slice(),
        b"From: a@example.test\r\n\r\nbody",
        b"From: a@example.test\r\n\r\nx\0y\r\n",
    ] {
        assert!(validate_data(raw, &BTreeSet::new(), false).is_err());
    }
    assert!(validate_data(
        "From: café@example.test\r\n\r\nbody\r\n".as_bytes(),
        &BTreeSet::new(),
        false
    )
    .is_err());
    assert!(validate_data(
        "From: a@example.test\r\n\r\ncafé\r\n".as_bytes(),
        &BTreeSet::new(),
        false
    )
    .is_err());
    assert!(validate_data(
        b"From: a@example.test\r\n\r\nbody\r\n",
        &BTreeSet::new(),
        true
    )
    .is_err());
    let caps = BTreeSet::from(["SMTPUTF8".into(), "8BITMIME".into()]);
    assert_eq!(
        validate_data(
            "From: café@example.test\r\n\r\ncafé\r\n".as_bytes(),
            &caps,
            true
        )
        .unwrap(),
        (true, true)
    );
}
