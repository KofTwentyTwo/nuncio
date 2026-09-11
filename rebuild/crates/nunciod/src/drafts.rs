use nuncio_engine::{domain::drafts, store};
use nuncio_proto::v2;

pub(crate) async fn upload(
    engine: &nuncio_engine::engine::Engine,
    mut stream: tonic::Streaming<v2::DraftUploadChunk>,
) -> Result<v2::Draft, tonic::Status> {
    use crate::mail::storage_error;
    use v2::draft_upload_chunk::Part;
    let Some(v2::DraftUploadChunk {
        part: Some(Part::Header(h)),
    }) = stream.message().await?
    else {
        return Err(tonic::Status::invalid_argument(
            "Upload header must come first",
        ));
    };
    let upload = engine
        .begin_draft_upload(store::DraftUploadInput {
            account_id: h.account_id,
            draft_id: h.draft_id,
            expected_version: h.expected_version,
            filename: h.filename,
            mime_type: h.mime_type,
            parameters: h.parameters.into_iter().collect(),
            content_id: h.content_id,
            disposition: h.disposition,
            byte_length: h.byte_length,
            sha256: h.sha256,
        })
        .await
        .map_err(storage_error)?;
    while let Some(chunk) = stream.message().await? {
        let Some(Part::Data(data)) = chunk.part else {
            return Err(tonic::Status::invalid_argument(
                "Only data may follow the upload header",
            ));
        };
        engine
            .append_draft_upload(&upload, data.offset, data.data)
            .await
            .map_err(storage_error)?;
    }
    Ok(draft(
        engine
            .finish_draft_upload(upload)
            .await
            .map_err(storage_error)?,
    ))
}

pub(crate) fn content(c: v2::DraftContent) -> drafts::DraftContent {
    let recipients = |items: Vec<v2::DraftRecipient>| {
        items
            .into_iter()
            .map(|r| drafts::Recipient {
                address: r.address,
                name: r.name,
            })
            .collect()
    };
    drafts::DraftContent {
        to: recipients(c.to),
        cc: recipients(c.cc),
        bcc: recipients(c.bcc),
        subject: c.subject,
        text: c.text,
        html: c.html,
    }
}
pub(crate) fn draft(d: store::Draft) -> v2::Draft {
    let recipients = |items: Vec<drafts::Recipient>| {
        items
            .into_iter()
            .map(|r| v2::DraftRecipient {
                address: r.address,
                name: r.name,
            })
            .collect()
    };
    v2::Draft {
        context: d.context.map(|c| v2::DraftContext {
            source_message_id: c.source_message_id,
            kind: c.kind,
            original_subject: c.original_subject,
            thread_id: c.thread_id,
            in_reply_to: c.in_reply_to,
            references: c.references,
        }),
        id: d.id,
        account_id: d.account_id,
        version: d.version,
        created_at_ms: d.created_at_ms,
        updated_at_ms: d.updated_at_ms,
        content: Some(v2::DraftContent {
            to: recipients(d.content.to),
            cc: recipients(d.content.cc),
            bcc: recipients(d.content.bcc),
            subject: d.content.subject,
            text: d.content.text,
            html: d.content.html,
        }),
        attachments: d
            .attachments
            .into_iter()
            .map(|a| v2::DraftAttachment {
                id: a.id,
                filename: a.filename,
                mime_type: a.mime_type,
                parameters: a.parameters.into_iter().collect(),
                content_id: a.content_id,
                disposition: a.disposition,
                byte_length: a.byte_length,
                sha256: a.sha256,
            })
            .collect(),
    }
}
