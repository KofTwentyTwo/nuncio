use crate::mail::{error, storage_error, sync_run};
use nuncio_engine::{domain::calendar as domain, engine::Engine, store};
use nuncio_proto::v2::{self, calendar_server::Calendar};
use std::sync::Arc;
use tonic::{Request, Response, Status};

mod change;
pub(crate) struct CalendarService(pub Arc<Engine>, pub tokio::sync::watch::Receiver<bool>);
#[tonic::async_trait]
impl Calendar for CalendarService {
    async fn query_free_busy(
        &self,
        request: Request<v2::FreeBusyRequest>,
    ) -> Result<Response<v2::FreeBusyResult>, Status> {
        let q = request.into_inner();
        let input = nuncio_engine::domain::free_busy::FreeBusyRequest {
            account_id: q.account_id,
            from: q.from,
            to: q.to,
            time_zone: q.time_zone,
            calendar_ids: q.provider_calendar_ids,
        };
        let mut stopped = self.1.clone();
        if *stopped.borrow() {
            return Err(Status::unavailable("Daemon is stopping"));
        }
        let result = tokio::select! {
            result=self.0.query_free_busy(input)=>result.map_err(error)?,
            _=stopped.changed()=>return Err(Status::unavailable("Daemon is stopping")),
            _=tokio::time::sleep(std::time::Duration::from_secs(30))=>return Err(Status::deadline_exceeded("Free/busy query exceeded 30 seconds")),
        };
        Ok(Response::new(v2::FreeBusyResult {
            from: result.from,
            to: result.to,
            time_zone: result.time_zone,
            checked_at_ms: result.checked_at_ms,
            complete: result.complete,
            calendars: result
                .calendars
                .into_iter()
                .map(|c| v2::CalendarAvailability {
                    provider_calendar_id: c.provider_calendar_id,
                    complete: c.complete,
                    busy: c
                        .busy
                        .into_iter()
                        .map(|b| v2::BusyPeriod {
                            from: b.from,
                            to: b.to,
                        })
                        .collect(),
                    errors: c
                        .errors
                        .into_iter()
                        .map(|e| v2::AvailabilityError {
                            domain: e.domain,
                            reason: e.reason,
                        })
                        .collect(),
                })
                .collect(),
        }))
    }

    async fn change_event(
        &self,
        request: Request<v2::ChangeEventRequest>,
    ) -> Result<Response<v2::Operation>, Status> {
        let input = change::input(request.into_inner())?;
        let mut stopped = self.1.clone();
        if *stopped.borrow() {
            return Err(Status::unavailable("Daemon is stopping"));
        }
        tokio::select! {
            result=self.0.change_event(input)=>result.map(|op|Response::new(crate::operations::operation(op))).map_err(storage_error),
            _=stopped.changed()=>Err(Status::unavailable("Daemon is stopping; inspect the operation after restart")),
            _=tokio::time::sleep(std::time::Duration::from_secs(30))=>Err(Status::deadline_exceeded("Calendar enqueue exceeded 30 seconds; retry the identical request")),
        }
    }

    async fn list_calendars(
        &self,
        request: Request<v2::ListCalendarsRequest>,
    ) -> Result<Response<v2::ListCalendarsResponse>, Status> {
        let q = request.into_inner();
        let page = self
            .0
            .calendar_list(q.account_id, q.page_size, q.page_token)
            .await
            .map_err(storage_error)?;
        Ok(Response::new(v2::ListCalendarsResponse {
            revision: page.revision,
            items: page
                .items
                .into_iter()
                .map(|c| v2::CalendarSummary {
                    id: c.id,
                    account_id: c.account_id,
                    provider_id: c.provider_id,
                    summary: c.summary,
                    time_zone: c.time_zone,
                    access_role: c.access_role,
                    is_primary: c.is_primary,
                    retired: c.retired,
                    canonical_revision: c.canonical_revision,
                })
                .collect(),
            next_page_token: page.next_page_token,
        }))
    }
    async fn get_event(
        &self,
        request: Request<v2::CalendarEventRequest>,
    ) -> Result<Response<v2::GetCalendarEventResponse>, Status> {
        let q = request.into_inner();
        let result = self
            .0
            .calendar_event(q.account_id, q.calendar_id, q.event_id)
            .await
            .map_err(storage_error)?;
        Ok(Response::new(v2::GetCalendarEventResponse {
            revision: result.revision,
            event: Some(event(result.event)),
            coverage: Some(coverage(result.coverage)),
        }))
    }
    async fn list_agenda(
        &self,
        request: Request<v2::ListAgendaRequest>,
    ) -> Result<Response<v2::ListAgendaResponse>, Status> {
        let q = request.into_inner();
        let window = window(
            q.window
                .ok_or_else(|| Status::invalid_argument("Agenda window required"))?,
        )?;
        let page = self
            .0
            .calendar_agenda(q.account_id, window, q.page_size, q.page_token)
            .await
            .map_err(storage_error)?;
        Ok(Response::new(v2::ListAgendaResponse {
            revision: page.revision,
            items: page.items.into_iter().map(event).collect(),
            next_page_token: page.next_page_token,
            coverage: page.coverage.into_iter().map(coverage).collect(),
        }))
    }
    async fn refresh_agenda(
        &self,
        request: Request<v2::RefreshAgendaRequest>,
    ) -> Result<Response<v2::SyncRun>, Status> {
        let q = request.into_inner();
        let window = q.window.map(window).transpose()?;
        Ok(Response::new(sync_run(
            self.0
                .refresh_calendar(q.account_id, window)
                .await
                .map_err(error)?,
        )))
    }
}
fn window(w: v2::AgendaWindow) -> Result<domain::AgendaWindow, Status> {
    domain::AgendaWindow::new(&w.from, &w.to).map_err(|_| {
        Status::invalid_argument(
            "Agenda window must contain 1 through 366 days; upper date is exclusive",
        )
    })
}
fn event(e: store::StoredEvent) -> v2::CalendarEvent {
    v2::CalendarEvent {
        id: e.id,
        account_id: e.account_id,
        calendar_id: e.calendar_id,
        recurring_event_id: e.recurring_event_id,
        provider_id: e.object.provider_id,
        summary: e.object.summary,
        etag: e.object.etag,
        status: e.object.status,
        start: e.object.start.map(time),
        end: e.object.end.map(time),
        original_start: e.object.original_start.map(time),
        recurring_provider_id: e.object.recurring_provider_id,
        provider_json: e.object.provider_json,
    }
}
fn coverage(c: store::CalendarCoverage) -> v2::AgendaCoverage {
    v2::AgendaCoverage {
        calendar_id: c.calendar_id,
        state: c.state,
        from: c.from,
        to: c.to,
        canonical_revision: c.canonical_revision,
        cached_revision: c.cached_revision,
        refreshed_at_ms: c.refreshed_at_ms,
    }
}
fn time(t: domain::EventTime) -> v2::EventTime {
    v2::EventTime {
        value: Some(match t {
            domain::EventTime::Date { date } => v2::event_time::Value::Date(date),
            domain::EventTime::DateTime { date_time } => {
                v2::event_time::Value::DateTime(v2::ZonedDateTime {
                    rfc3339: date_time.rfc3339,
                    time_zone: date_time.time_zone,
                })
            }
        }),
    }
}
