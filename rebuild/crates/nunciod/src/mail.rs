use nuncio_engine::{accounts::AccountError, engine::Engine, mail::MailError, store};
use nuncio_proto::v2::{self, mail_server::Mail};
use std::{pin::Pin, sync::Arc};
use tonic::{Request, Response, Status};
pub(crate) struct MailService(pub Arc<Engine>, pub tokio::sync::watch::Receiver<bool>);
#[tonic::async_trait]
impl Mail for MailService {
    async fn get_capabilities(
        &self,
        request: Request<v2::MailCapabilitiesRequest>,
    ) -> Result<Response<v2::MailCapabilities>, Status> {
        let c = self
            .0
            .mail_capabilities(request.into_inner().account_id)
            .await
            .map_err(storage_error)?;
        Ok(Response::new(v2::MailCapabilities {
            account_id: c.account_id,
            provider: c.provider,
            placement_model: c.placement_model,
            read_unread: c.read_unread,
            star_unstar: c.star_unstar,
            archive: c.archive,
            trash_restore: c.trash_restore,
            existing_labels: c.existing_labels,
            move_copy: c.move_copy,
        }))
    }
    async fn change_message(
        &self,
        request: Request<v2::ChangeMessageRequest>,
    ) -> Result<Response<v2::Operation>, Status> {
        use nuncio_engine::domain::mail_change::MailAction;
        use v2::change_message_request::Action;
        let q = request.into_inner();
        let action = match q
            .action
            .ok_or_else(|| Status::invalid_argument("Mail action is required"))?
        {
            Action::Read(v) => MailAction::Read { read: v.read },
            Action::Star(v) => MailAction::Star { starred: v.starred },
            Action::Archive(_) => MailAction::Archive {},
            Action::Move(v) => MailAction::Move {
                destination_collection_id: v.destination_collection_id,
            },
            Action::Copy(v) => MailAction::Copy {
                destination_collection_id: v.destination_collection_id,
            },
            Action::Trash(v) => MailAction::Trash { trashed: v.trashed },
            Action::Label(v) => MailAction::Label {
                collection_id: v.collection_id,
                present: v.present,
            },
        };
        let mut stopped = self.1.clone();
        if *stopped.borrow() {
            return Err(Status::unavailable("Daemon is stopping"));
        }
        tokio::select! {
            result=self.0.change_message(store::EnqueueMailChange{account_id:q.account_id,message_id:q.message_id,request_id:q.request_id,action})=>result.map(crate::operations::operation).map(Response::new).map_err(storage_error),
            _=stopped.changed()=>Err(Status::unavailable("Daemon is stopping; retry the same request identity")),
            _=tokio::time::sleep(std::time::Duration::from_secs(30))=>Err(Status::deadline_exceeded("Enqueue exceeded 30 seconds; retry the same request identity")),
        }
    }
    async fn send_draft(
        &self,
        request: Request<v2::SendDraftRequest>,
    ) -> Result<Response<v2::Operation>, Status> {
        let q = request.into_inner();
        let mut stopped = self.1.clone();
        if *stopped.borrow() {
            return Err(Status::unavailable("Daemon is stopping"));
        }
        let work = async {
            Ok(Response::new(crate::operations::operation(
                self.0
                    .send_draft(q.account_id, q.draft_id, q.request_id, q.expected_version)
                    .await
                    .map_err(storage_error)?,
            )))
        };
        tokio::select! {
            result=work=>result,
            _=stopped.changed()=>Err(Status::unavailable("Daemon is stopping; inspect the request identity after restart")),
            _=tokio::time::sleep(std::time::Duration::from_secs(30))=>Err(Status::deadline_exceeded("Enqueue exceeded 30 seconds; retry the same request identity")),
        }
    }
    async fn prepare_draft(
        &self,
        request: Request<v2::PrepareDraftRequest>,
    ) -> Result<Response<v2::Draft>, Status> {
        let q = request.into_inner();
        use nuncio_engine::domain::prepare::PrepareKind;
        let kind = match q.kind.as_str() {
            "reply" => PrepareKind::Reply,
            "reply_all" => PrepareKind::ReplyAll,
            "forward" => PrepareKind::Forward,
            _ => return Err(Status::invalid_argument("Unknown draft preparation kind")),
        };
        let to =
            q.to.into_iter()
                .map(|r| nuncio_engine::domain::drafts::Recipient {
                    address: r.address,
                    name: r.name,
                })
                .collect();
        Ok(Response::new(crate::drafts::draft(
            self.0
                .prepare_draft(q.account_id, q.message_id, kind, q.body, to)
                .await
                .map_err(storage_error)?,
        )))
    }
    async fn upload_draft_attachment(
        &self,
        request: Request<tonic::Streaming<v2::DraftUploadChunk>>,
    ) -> Result<Response<v2::Draft>, Status> {
        let mut stopped = self.1.clone();
        if *stopped.borrow() {
            return Err(Status::unavailable("Daemon is stopping"));
        }
        tokio::select! {
            result = crate::drafts::upload(&self.0, request.into_inner()) => result.map(Response::new),
            _ = stopped.changed() => Err(Status::unavailable("Daemon is stopping")),
            _ = tokio::time::sleep(std::time::Duration::from_secs(30)) => Err(Status::deadline_exceeded("Draft upload exceeded 30 seconds")),
        }
    }
    async fn save_draft(
        &self,
        request: Request<v2::SaveDraftRequest>,
    ) -> Result<Response<v2::Draft>, Status> {
        let q = request.into_inner();
        let content = q
            .content
            .ok_or_else(|| Status::invalid_argument("Draft content is required"))?;
        let draft = self
            .0
            .save_draft(store::SaveDraft {
                account_id: q.account_id,
                id: q.draft_id,
                expected_version: q.expected_version,
                content: crate::drafts::content(content),
            })
            .await
            .map_err(storage_error)?;
        Ok(Response::new(crate::drafts::draft(draft)))
    }
    async fn get_draft(
        &self,
        request: Request<v2::DraftRequest>,
    ) -> Result<Response<v2::Draft>, Status> {
        let q = request.into_inner();
        Ok(Response::new(crate::drafts::draft(
            self.0
                .get_draft(q.account_id, q.draft_id)
                .await
                .map_err(storage_error)?,
        )))
    }
    async fn list_drafts(
        &self,
        request: Request<v2::ListDraftsRequest>,
    ) -> Result<Response<v2::ListDraftsResponse>, Status> {
        let q = request.into_inner();
        let page = self
            .0
            .list_drafts(
                q.account_id,
                if q.page_size == 0 { 100 } else { q.page_size },
                q.page_token,
            )
            .await
            .map_err(storage_error)?;
        Ok(Response::new(v2::ListDraftsResponse {
            revision: page.revision,
            next_page_token: page.next_page_token,
            items: page
                .items
                .into_iter()
                .map(|d| v2::DraftSummary {
                    id: d.id,
                    account_id: d.account_id,
                    version: d.version,
                    subject: d.subject,
                    created_at_ms: d.created_at_ms,
                    updated_at_ms: d.updated_at_ms,
                    attachment_count: d.attachment_count,
                })
                .collect(),
        }))
    }
    async fn delete_draft(
        &self,
        request: Request<v2::DeleteDraftRequest>,
    ) -> Result<Response<v2::DeleteDraftResponse>, Status> {
        let q = request.into_inner();
        self.0
            .delete_draft(q.account_id, q.draft_id, q.expected_version)
            .await
            .map_err(storage_error)?;
        Ok(Response::new(v2::DeleteDraftResponse {}))
    }
    async fn list_messages(
        &self,
        request: Request<v2::ListMailRequest>,
    ) -> Result<Response<v2::ListMailResponse>, Status> {
        let q = request.into_inner();
        let page = self
            .0
            .list_mail(store::MailQuery {
                account_id: q.account_id,
                collection_id: q.collection_id,
                query: q.query,
                page_size: if q.page_size == 0 { 100 } else { q.page_size },
                page_token: q.page_token,
            })
            .await
            .map_err(storage_error)?;
        Ok(Response::new(v2::ListMailResponse {
            revision: page.revision,
            items: page.items.into_iter().map(summary).collect(),
            next_page_token: page.next_page_token,
            coverage: Some(v2::MailCoverage {
                state: page.coverage.state,
                synchronized_at_ms: page.coverage.synchronized_at_ms,
                cursor: page.coverage.cursor,
            }),
        }))
    }
    async fn get_message(
        &self,
        request: Request<v2::MessageRequest>,
    ) -> Result<Response<v2::GetMailResponse>, Status> {
        let q = request.into_inner();
        let detail = self
            .0
            .get_mail(q.account_id, q.message_id)
            .await
            .map_err(storage_error)?;
        Ok(Response::new(v2::GetMailResponse {
            revision: detail.revision,
            message: Some(summary(detail.message)),
            headers: detail
                .headers
                .into_iter()
                .map(|h| v2::MailHeader {
                    name: h.name,
                    value: h.value,
                })
                .collect(),
            attachments: detail
                .attachments
                .into_iter()
                .map(|a| v2::MailAttachment {
                    id: a.id,
                    part_index: a.part_index,
                    filename: a.filename,
                    mime_type: a.mime_type,
                    content_id: a.content_id,
                    byte_length: a.byte_length,
                    sha256: a.sha256,
                })
                .collect(),
            provider_json: detail.provider_json,
        }))
    }
    async fn list_collections(
        &self,
        request: Request<v2::ListCollectionsRequest>,
    ) -> Result<Response<v2::ListCollectionsResponse>, Status> {
        let (revision, items) = self
            .0
            .mail_collections(request.into_inner().account_id)
            .await
            .map_err(storage_error)?;
        Ok(Response::new(v2::ListCollectionsResponse {
            revision,
            items: items.into_iter().map(collection).collect(),
        }))
    }
    type DownloadStream =
        Pin<Box<dyn futures_util::Stream<Item = Result<v2::PayloadChunk, Status>> + Send>>;
    async fn download(
        &self,
        request: Request<v2::DownloadMailRequest>,
    ) -> Result<Response<Self::DownloadStream>, Status> {
        let q = request.into_inner();
        let blob = self
            .0
            .mail_blob(q.account_id.clone(), q.message_id, q.kind, q.attachment_id)
            .await
            .map_err(storage_error)?;
        let engine = self.0.clone();
        let stream = futures_util::stream::unfold(
            (engine, q.account_id, blob, 0_u64, false),
            |(engine, account, blob, ordinal, done)| async move {
                if done {
                    return None;
                }
                let offset = ordinal * 262144;
                let bytes = if blob.byte_length == 0 {
                    Ok(vec![])
                } else {
                    engine
                        .blob_chunk(account.clone(), blob.id.clone(), ordinal)
                        .await
                        .map_err(storage_error)
                };
                let (result, done) = match bytes {
                    Ok(data) => {
                        let eof = offset + data.len() as u64 == blob.byte_length;
                        (
                            Ok(v2::PayloadChunk {
                                offset,
                                data,
                                total_bytes: blob.byte_length,
                                sha256: blob.sha256.clone(),
                                eof,
                            }),
                            eof,
                        )
                    }
                    Err(error) => (Err(error), true),
                };
                Some((result, (engine, account, blob, ordinal + 1, done)))
            },
        );
        Ok(Response::new(Box::pin(stream)))
    }
}
fn collection(c: store::MailCollection) -> v2::MailCollection {
    v2::MailCollection {
        id: c.id,
        provider_id: c.provider_id,
        name: c.name,
        kind: c.kind,
        retired: c.retired,
    }
}
fn summary(m: store::MailSummary) -> v2::MailSummary {
    v2::MailSummary {
        id: m.id,
        account_id: m.account_id,
        provider_id: m.provider_id,
        thread_id: m.thread_id,
        history_id: m.history_id,
        internal_date_ms: m.internal_date_ms,
        subject: m.subject,
        body_availability: m.body_availability,
        collections: m.collections.into_iter().map(collection).collect(),
    }
}
pub(crate) fn sync_run(r: store::SyncRun) -> v2::SyncRun {
    v2::SyncRun {
        id: r.id,
        account_id: r.account_id,
        scope: r.scope,
        mode: r.mode,
        state: r.state,
        started_at_ms: r.started_at_ms,
        finished_at_ms: r.finished_at_ms,
        processed: r.processed,
        error_code: r.error_code,
    }
}
pub(crate) fn storage_error(e: store::StoreError) -> Status {
    let message = e.to_string();
    match e {
        store::StoreError::Busy | store::StoreError::VersionConflict => {
            Status::failed_precondition(message)
        }
        store::StoreError::ChangeHistoryExpired => Status::out_of_range(message),
        store::StoreError::ResultTooLarge => Status::resource_exhausted(message),
        store::StoreError::NotFound => Status::not_found(message),
        store::StoreError::InvalidInput => Status::invalid_argument(message),
        store::StoreError::AccountLifecycle => Status::failed_precondition(message),
        store::StoreError::RefreshRequired => Status::failed_precondition(message),
        store::StoreError::Unavailable => Status::unavailable(message),
        _ => Status::internal(message),
    }
}
pub(crate) fn error(e: MailError) -> Status {
    let message = e.to_string();
    match e {
        MailError::Storage(e) => storage_error(e),
        MailError::Account(AccountError::Authorization | AccountError::ScopeDenied) => {
            Status::unauthenticated(message)
        }
        MailError::Account(AccountError::Invalid) => Status::invalid_argument(message),
        MailError::Account(AccountError::NotFound) | MailError::NotFound => {
            Status::not_found(message)
        }
        MailError::Unavailable
        | MailError::RetryAfter(_)
        | MailError::Account(AccountError::Unavailable | AccountError::RetryAfter(_)) => {
            Status::unavailable(message)
        }
        MailError::TooLarge => Status::resource_exhausted(message),
        MailError::Cancelled => Status::cancelled(message),
        _ => Status::internal(message),
    }
}
