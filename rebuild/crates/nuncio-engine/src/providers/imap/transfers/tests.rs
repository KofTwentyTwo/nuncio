#![allow(clippy::unwrap_used)]
use super::*;
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
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
};

#[tokio::test]
async fn copyuid_evidence_requires_exact_identity_and_survives_only_valid_lost_acknowledgements() {
    for (response,acknowledged,uid) in [
        ("{tag} OK [COPYUID 9010 11 3] copied\r\n",true,Some(3)),
        ("* OK [COPYUID 9010 11:11 3:3] copied\r\n* 1 EXPUNGE\r\n",false,Some(3)),
        ("* OK [COPYUID 9010 11 3] copied\r\n{tag} NO partial MOVE\r\n",false,Some(3)),
        ("{tag} OK [COPYUID 9010 12 3] wrong source\r\n",false,None),
        ("{tag} OK [COPYUID 9011 11 3] wrong epoch\r\n",false,None),
        ("{tag} OK [COPYUID 9010 1:4294967295 3] oversized set\r\n",false,None),
        ("* OK [COPYUID 9010 11 3] copied\r\nWRONG OK completed\r\n",false,None),
        ("{tag} OK no mapping\r\n",true,None),
        ("* OK [COPYUID 9010 11 3] copied\r\n{tag} OK [COPYUID 9010 11 4] conflicting mapping\r\n",false,None),
    ] {
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
        let tcp=TcpStream::connect(listener.local_addr().unwrap()).await.unwrap();
        let peer=tokio::spawn(async move {
            let (tcp,_)=listener.accept().await.unwrap();let mut peer=BufReader::new(tcp);let mut line=String::new();
            peer.read_line(&mut line).await.unwrap();let tag=line.split_once(' ').unwrap().0.to_string();
            peer.get_mut().write_all(b"+\r\n").await.unwrap();line.clear();peer.read_line(&mut line).await.unwrap();
            peer.get_mut().write_all(format!("{tag} OK authenticated\r\n").as_bytes()).await.unwrap();line.clear();peer.read_line(&mut line).await.unwrap();
            let (tag,command)=line.trim_end().split_once(' ').unwrap();assert_eq!(command,"UID MOVE 11 \"Archive\"");
            peer.get_mut().write_all(response.replace("{tag}",tag).as_bytes()).await.unwrap();
        });
        let wire=ImapWire::new(Wire::test(tcp),std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),262144,0).unwrap();
        let session=async_imap::Client::new(wire).authenticate("PLAIN",Plain{bytes:zeroize::Zeroizing::new(b"\0synthetic\0synthetic".to_vec()),used:false}).await.map_err(|(e,_)|e).unwrap();
        let mut connection=Connection{session,capabilities:Default::default()};
        let source=ImapPlacement::new(AccountId::generate(),ImapMailboxId::generate(),9001,11).unwrap();
        let payload=ImapTransferPayload{restore_origin:None,source,source_mailbox:MailboxName::from_unicode("INBOX").unwrap(),destination:ImapMailboxId::generate(),destination_mailbox:MailboxName::from_unicode("Archive").unwrap(),destination_uid_validity:std::num::NonZeroU32::new(9010).unwrap(),mode:ImapTransferMode::Move};
        let result=copy_or_move(&mut connection,&payload,ImapTransferMode::Move).await;
        peer.await.unwrap();assert_eq!(result.acknowledged,acknowledged,"{response}");assert_eq!(result.proof.map(|p|p.destination.uid.get()),uid,"{response}");
    }
}
