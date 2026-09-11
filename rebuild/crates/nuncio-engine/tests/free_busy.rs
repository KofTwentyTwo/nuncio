#![allow(clippy::unwrap_used)]
use nuncio_engine::domain::free_busy::{FreeBusyRequest, FreeBusyResult};
use serde_json::json;
fn request() -> FreeBusyRequest {
    FreeBusyRequest {
        account_id: "account".into(),
        from: "2026-10-02T14:30:00Z".into(),
        to: "2026-10-02T15:30:00Z".into(),
        time_zone: "UTC".into(),
        calendar_ids: vec!["primary".into(), "missing@example.test".into()],
    }
}
#[test]
fn validates_freebusy_bounds_and_never_interprets_missing_coverage_as_free() {
    let q = request();
    q.validate().unwrap();
    let good = json!({"kind":"calendar#freeBusy","timeMin":q.from,"timeMax":q.to,"calendars":{"primary":{"busy":[{"start":"2026-10-02T09:30:00-05:00","end":"2026-10-02T10:30:00-05:00"}]},"missing@example.test":{"errors":[{"domain":"global","reason":"futureProviderError"}]}}});
    let result = FreeBusyResult::from_google(&q, good.clone(), 123).unwrap();
    assert!(!result.complete);
    assert!(result.calendars[0].complete);
    assert!(!result.calendars[1].complete);
    assert_eq!(result.calendars[1].errors[0].reason, "futureProviderError");
    assert_eq!(result.checked_at_ms, 123);
    for bad in [
        {
            let mut v = good.clone();
            v["calendars"].as_object_mut().unwrap().remove("primary");
            v
        },
        {
            let mut v = good.clone();
            v["calendars"]["primary"] = json!({});
            v
        },
        {
            let mut v = good.clone();
            v["timeMax"] = json!("2026-10-02T16:30:00Z");
            v
        },
        {
            let mut v = good.clone();
            v["calendars"]["primary"]["busy"][0]["end"] = json!("2026-10-02T09:00:00-05:00");
            v
        },
        {
            let mut v = good.clone();
            v["calendars"]["primary"]["busy"][0]["start"] = json!("2026-10-02T08:00:00-05:00");
            v["calendars"]["primary"]["busy"][0]["end"] = json!("2026-10-02T09:00:00-05:00");
            v
        },
        {
            let mut v = good.clone();
            v["calendars"]["primary"]["errors"] = json!([{}]);
            v
        },
    ] {
        assert!(FreeBusyResult::from_google(&q, bad, 123).is_err());
    }
    for bad in [
        {
            let mut v = q.clone();
            v.calendar_ids.clear();
            v
        },
        {
            let mut v = q.clone();
            v.calendar_ids = vec!["primary".into(); 51];
            v
        },
        {
            let mut v = q.clone();
            v.calendar_ids = vec!["primary".into(); 2];
            v
        },
        {
            let mut v = q.clone();
            v.from = v.to.clone();
            v
        },
        {
            let mut v = q.clone();
            v.from = "2026-10-02T14:30:00".into();
            v
        },
        {
            let mut v = q.clone();
            v.to = "2028-10-02T15:30:00Z".into();
            v
        },
        {
            let mut v = q.clone();
            v.time_zone = "Not/A_Zone".into();
            v
        },
        {
            let mut v = q.clone();
            v.calendar_ids = vec!["id\r\n".into()];
            v
        },
    ] {
        assert!(bad.validate().is_err());
    }
}
