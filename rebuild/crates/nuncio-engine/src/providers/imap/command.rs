use super::{wire::ImapWire, MailError};
use async_imap::{
    imap_proto::{Response, Status},
    Session,
};

pub(super) async fn complete<T>(
    session: &mut Session<ImapWire>,
    command: &str,
    bytes: usize,
    literal: usize,
    mut visit: impl FnMut(&Response<'_>) -> Result<Option<T>, MailError>,
) -> Result<Vec<T>, MailError> {
    complete_with_raw(session, command, bytes, literal, |response, _| {
        visit(response)
    })
    .await
}
pub(super) async fn complete_with_raw<T>(
    session: &mut Session<ImapWire>,
    command: &str,
    bytes: usize,
    literal: usize,
    visit: impl FnMut(&Response<'_>, &[u8]) -> Result<Option<T>, MailError>,
) -> Result<Vec<T>, MailError> {
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        complete_inner(session, command, bytes, literal, visit),
    )
    .await
    .map_err(|_| MailError::Unavailable)?
}
async fn complete_inner<T>(
    session: &mut Session<ImapWire>,
    command: &str,
    bytes: usize,
    literal: usize,
    mut visit: impl FnMut(&Response<'_>, &[u8]) -> Result<Option<T>, MailError>,
) -> Result<Vec<T>, MailError> {
    if command.is_empty() || command.len() > 65536 || command.contains(['\r', '\n', '\0']) {
        return Err(MailError::Invalid);
    }
    session
        .get_mut()
        .set_limits(bytes, literal)
        .map_err(|_| MailError::Protocol)?;
    let tag = session
        .run_command(command)
        .await
        .map_err(|_| MailError::Unavailable)?;
    let mut responses = Vec::new();
    for _ in 0..4096 {
        let response = session
            .read_response()
            .await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::UnexpectedEof {
                    MailError::Unavailable
                } else {
                    MailError::Protocol
                }
            })?
            .ok_or(MailError::Unavailable)?;
        let done = match response.parsed() {
            Response::Done {
                tag: observed,
                status,
                ..
            } => {
                if observed != &tag {
                    return Err(MailError::Protocol);
                }
                match status {
                    Status::Ok => true,
                    Status::No => return Err(MailError::Unavailable),
                    Status::Bad => return Err(MailError::Unsupported),
                    _ => return Err(MailError::Protocol),
                }
            }
            Response::Data {
                status: Status::Bye,
                ..
            } => return Err(MailError::Unavailable),
            Response::Continue { .. } => return Err(MailError::Protocol),
            _ => false,
        };
        if let Some(value) = visit(response.parsed(), response.borrow_owner())? {
            responses.push(value);
        }
        if done {
            return Ok(responses);
        }
    }
    Err(MailError::Protocol)
}
