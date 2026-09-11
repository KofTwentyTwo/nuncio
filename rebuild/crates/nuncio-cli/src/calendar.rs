mod action;
mod free_busy;
use crate::{
    args::CalendarCommand,
    mail::{json, wait_run},
    output::AppError,
    rpc_error,
};
use nuncio_proto::{
    client::TokenInjector,
    v2::{self, calendar_client::CalendarClient, system_client::SystemClient},
};
use serde_json::{json as value, Value};
use tonic::transport::Channel;

pub async fn run(
    command: CalendarCommand,
    channel: Channel,
    token: TokenInjector,
) -> Result<Value, AppError> {
    let mut client = CalendarClient::with_interceptor(channel.clone(), token.clone())
        .max_decoding_message_size(16 * 1024 * 1024);
    match command {
        CalendarCommand::FreeBusy { account, file } => {
            let mut request = free_busy::read(&file)?;
            request.account_id = account;
            json(
                client
                    .query_free_busy(request)
                    .await
                    .map_err(rpc_error)?
                    .into_inner(),
            )
        }

        CalendarCommand::Change {
            account,
            calendar,
            request_id,
            file,
            wait,
        } => {
            let mut request = action::read(&file)?;
            request.account_id = account;
            request.calendar_id = calendar;
            request.request_id = request_id;
            let op = client
                .change_event(request)
                .await
                .map_err(rpc_error)?
                .into_inner();
            if wait {
                crate::operations::wait_for(op, channel, token).await
            } else {
                json(op)
            }
        }
        CalendarCommand::List {
            account,
            page_size,
            page_token,
        } => json(
            client
                .list_calendars(v2::ListCalendarsRequest {
                    account_id: account,
                    page_size,
                    page_token,
                })
                .await
                .map_err(rpc_error)?
                .into_inner(),
        ),
        CalendarCommand::Get {
            account,
            calendar,
            event: id,
        } => {
            let response = client
                .get_event(v2::CalendarEventRequest {
                    account_id: account,
                    calendar_id: calendar,
                    event_id: id,
                })
                .await
                .map_err(rpc_error)?
                .into_inner();
            Ok(
                value!({"revision":response.revision,"event":event(response.event.ok_or_else(invalid_response)?)?,"coverage":response.coverage}),
            )
        }
        CalendarCommand::Agenda {
            account,
            from,
            to,
            page_size,
            page_token,
        } => {
            let response = client
                .list_agenda(v2::ListAgendaRequest {
                    account_id: account,
                    window: Some(v2::AgendaWindow { from, to }),
                    page_size,
                    page_token,
                })
                .await
                .map_err(rpc_error)?
                .into_inner();
            let events = response
                .items
                .into_iter()
                .map(event)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(
                value!({"revision":response.revision,"items":events,"coverage":response.coverage,"next_page_token":response.next_page_token}),
            )
        }
        CalendarCommand::Refresh {
            account,
            from,
            to,
            wait,
        } => {
            let window = match (from, to) {
                (Some(from), Some(to)) => Some(v2::AgendaWindow { from, to }),
                (None, None) => None,
                _ => return Err(AppError::invalid()),
            };
            let run = client
                .refresh_agenda(v2::RefreshAgendaRequest {
                    account_id: account,
                    window,
                })
                .await
                .map_err(rpc_error)?
                .into_inner();
            wait_run(
                &mut SystemClient::with_interceptor(channel, token),
                run,
                wait,
            )
            .await
        }
    }
}
fn event(e: v2::CalendarEvent) -> Result<Value, AppError> {
    Ok(
        value!({"id":e.id,"account_id":e.account_id,"calendar_id":e.calendar_id,"provider_id":e.provider_id,"summary":e.summary,"etag":e.etag,"status":e.status,
        "start":e.start.map(time).transpose()?,"end":e.end.map(time).transpose()?,"original_start":e.original_start.map(time).transpose()?,"recurring_provider_id":e.recurring_provider_id,"recurring_event_id":e.recurring_event_id,"provider_json":e.provider_json}),
    )
}
fn time(t: v2::EventTime) -> Result<Value, AppError> {
    Ok(match t.value.ok_or_else(invalid_response)? {
        v2::event_time::Value::Date(date) => value!({"date":date}),
        v2::event_time::Value::DateTime(t) => {
            value!({"date_time":{"rfc3339":t.rfc3339,"time_zone":t.time_zone}})
        }
    })
}
fn invalid_response() -> AppError {
    AppError {
        code: "invalid_response",
        message: "Engine returned an invalid Calendar response",
        sync_run: None,
        operation: None,
        recovery: None,
        exit: 1,
    }
}
