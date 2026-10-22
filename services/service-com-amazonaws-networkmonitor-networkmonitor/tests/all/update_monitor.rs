// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage for `UpdateMonitor`.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `networkmonitor` SDK client sees. The rules under test:
//!
//! * an update of a stored monitor replaces its `aggregationPeriod` with the
//!   requested value and reports the STORED record, so a later `GetMonitor`
//!   reads back the same period, ARN, name, state and tags;
//! * the requested period must be one of the values the model documents for
//!   `aggregationPeriod` ("Valid values are either `30` or `60`"); any other
//!   value is refused — with 400 `ValidationException` when it is inside the
//!   shape's own `@range(min: 30)`, and with the front door's 400
//!   `BadRequestException` when the BODY value is outside that range — and the
//!   stored period is left as it was;
//! * a `monitorName` no monitor is stored under is refused with 404
//!   `ResourceNotFoundException`, and the update creates nothing;
//! * the operation is `@idempotent`: repeating the same update answers the same
//!   record.

use std::sync::Arc;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` naming this service's signing name
/// (`networkmonitor`) — how the router picks the service — and the region in its
/// credential scope, which is the region every ARN the service mints carries.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTONLY/20260101/us-west-2/networkmonitor/aws4_request, \
     SignedHeaders=host, Signature=00";

/// The region in [`AUTHORIZATION`]'s credential scope.
const REGION: &str = "us-west-2";

/// The one account id a generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The `aggregationPeriod` values the model documents for this service
/// (`CreateMonitorInput$aggregationPeriod`: "Valid values are either `30` or
/// `60`"), which is the catalog `UpdateMonitor` admits.
const DOCUMENTED_PERIODS: &[i64] = &[30, 60];

fn router() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_networkmonitor_networkmonitor::EmulatedService::new(engine),
    );
    router
}

/// Send one request and return its status and parsed JSON body (`Value::Null`
/// for an empty body).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (u16, Value) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://localhost{path}"))
        .header("authorization", AUTHORIZATION);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let payload = body.map_or_else(bytes::Bytes::new, |b| {
        bytes::Bytes::copy_from_slice(b.as_bytes())
    });
    let request = builder.body(payload).expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code of an error response (`__type`), or `"<none>"`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<none>")
        .to_string()
}

fn string_at(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn period_of(body: &Value) -> Option<i64> {
    body.get("aggregationPeriod").and_then(Value::as_i64)
}

async fn create_monitor(router: &MultiServiceRouter, body: &str) -> (u16, Value) {
    call(router, "POST", "/monitors", Some(body)).await
}

async fn get_monitor(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    call(router, "GET", &format!("/monitors/{name}"), None).await
}

async fn update_monitor(router: &MultiServiceRouter, name: &str, body: &str) -> (u16, Value) {
    call(router, "PATCH", &format!("/monitors/{name}"), Some(body)).await
}

/// An update replaces the stored `aggregationPeriod` and every later read
/// reports the new value.
///
/// The table spans the whole class the rule covers: each documented period as
/// the stored value (including the documented default, left unnamed on the
/// create), crossed with each documented period as the requested one — so the
/// no-change rows (30 -> 30, 60 -> 60) are covered beside the changing ones.
/// The recorded case (period 30 updated to 60) is one row. Tags vary too, since
/// the update reports them and must not disturb them.
#[tokio::test]
async fn an_update_replaces_the_stored_aggregation_period() {
    // (case, create body fragment, period before, requested period, tags)
    let mut cases: Vec<(String, String, i64, i64, Value)> = Vec::new();
    for &before in DOCUMENTED_PERIODS {
        for &requested in DOCUMENTED_PERIODS {
            cases.push((
                format!("stored {before} updated to {requested}"),
                format!(r#","aggregationPeriod":{before},"tags":{{"team":"net"}}"#),
                before,
                requested,
                serde_json::json!({"team": "net"}),
            ));
        }
    }
    // The documented default, never named on the create, then updated to each
    // documented value — and with no tags at all.
    for &requested in DOCUMENTED_PERIODS {
        cases.push((
            format!("the default period updated to {requested}"),
            String::new(),
            60,
            requested,
            serde_json::json!({}),
        ));
    }

    let router = router();
    for (index, (case, fragment, before, requested, expected_tags)) in cases.iter().enumerate() {
        let name = format!("update-case-{index}");
        let (status, created) =
            create_monitor(&router, &format!(r#"{{"monitorName":"{name}"{fragment}}}"#)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup CreateMonitor expected 200, got {status} {created:?}"
        );
        assert_eq!(
            period_of(&created),
            Some(*before),
            "case {case:?}: expected the stored period {before} before the update, got {created:?}"
        );

        let (status, updated) =
            update_monitor(&router, &name, &format!(r#"{{"aggregationPeriod":{requested}}}"#))
                .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateMonitor 200, got {status} {updated:?}"
        );
        assert_eq!(
            period_of(&updated),
            Some(*requested),
            "case {case:?}: expected aggregationPeriod {requested}, got {updated:?}"
        );
        let expected_arn = format!("arn:aws:networkmonitor:{REGION}:{ACCOUNT}:monitor/{name}");
        assert_eq!(
            string_at(&updated, "monitorArn"),
            expected_arn,
            "case {case:?}: expected monitorArn {expected_arn:?}, got {updated:?}"
        );
        assert_eq!(
            string_at(&updated, "monitorName"),
            name,
            "case {case:?}: expected monitorName {name:?}, got {updated:?}"
        );
        assert_eq!(
            string_at(&updated, "state"),
            "INACTIVE",
            "case {case:?}: a monitor with no probes stays INACTIVE, got {updated:?}"
        );
        assert_eq!(
            updated.get("tags"),
            Some(expected_tags),
            "case {case:?}: expected tags {expected_tags:?}, got {updated:?}"
        );

        // What was STORED is what a later read reports — the whole point of the
        // update.
        let (status, got) = get_monitor(&router, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetMonitor 200, got {status} {got:?}"
        );
        assert_eq!(
            period_of(&got),
            Some(*requested),
            "case {case:?}: GetMonitor must report the updated period {requested}, got {got:?}"
        );
        assert_eq!(
            string_at(&got, "monitorArn"),
            expected_arn,
            "case {case:?}: GetMonitor must report the same ARN, got {got:?}"
        );
        assert_eq!(
            got.get("tags"),
            Some(expected_tags),
            "case {case:?}: an update must not disturb the tags, got {got:?}"
        );

        // `@idempotent`: the same update again answers the same record.
        let (status, again) =
            update_monitor(&router, &name, &format!(r#"{{"aggregationPeriod":{requested}}}"#))
                .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a repeated UpdateMonitor 200, got {status} {again:?}"
        );
        assert_eq!(
            again, updated,
            "case {case:?}: a repeated update must answer the same record, got {again:?}"
        );
    }
}

/// `UpdateMonitor` admits only the `aggregationPeriod` values the model
/// documents, and a refused update leaves the stored period as it was.
///
/// The table carries both published values, each published bound of the shape's
/// own `@range(min: 30)` and one past it (`30` and `29`), and values between and
/// above the documented pair. It also pins WHICH refusal each row gets: a body
/// value outside the shape's own `@range` is refused by the service's front door
/// with `BadRequestException`, while one inside the range but outside the
/// documented value list reaches the handler and is refused with the modelled
/// `ValidationException`.
#[tokio::test]
async fn an_update_accepts_only_the_documented_aggregation_periods() {
    // (case, requested period, expected status, expected error code)
    let cases: &[(&str, i64, u16, &str)] = &[
        ("the documented floor, which is the range's bound", 30, 200, ""),
        ("the documented default", 60, 200, ""),
        ("one below the modelled range bound", 29, 400, "BadRequestException"),
        ("between the two documented values", 45, 400, "ValidationException"),
        ("one past the upper documented value", 61, 400, "ValidationException"),
        ("above both documented values", 90, 400, "ValidationException"),
        ("far above", 3600, 400, "ValidationException"),
        ("zero", 0, 400, "BadRequestException"),
        ("negative", -30, 400, "BadRequestException"),
    ];

    let router = router();
    for (index, (case, requested, expected_status, expected_code)) in cases.iter().enumerate() {
        let name = format!("period-rule-{index}");
        // Every row starts from the SAME stored period, so a refused update is
        // visible as "nothing changed".
        let (status, created) = create_monitor(
            &router,
            &format!(r#"{{"monitorName":"{name}","aggregationPeriod":30}}"#),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup CreateMonitor expected 200, got {status} {created:?}"
        );

        let (status, answered) =
            update_monitor(&router, &name, &format!(r#"{{"aggregationPeriod":{requested}}}"#))
                .await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {answered:?}"
        );
        let expected_stored = if *expected_status == 200 { *requested } else { 30 };
        if *expected_status != 200 {
            assert_eq!(
                error_code(&answered),
                *expected_code,
                "case {case:?}: expected {expected_code}, got {answered:?}"
            );
        }
        let (_, got) = get_monitor(&router, &name).await;
        assert_eq!(
            period_of(&got),
            Some(expected_stored),
            "case {case:?}: expected the stored period {expected_stored}, got {got:?}"
        );
    }
}

/// An update of a `monitorName` no monitor is stored under is 404
/// `ResourceNotFoundException`, and stores nothing under that name.
///
/// The table spans names that never existed, a name whose monitor was deleted,
/// and names at the published bounds of the `ResourceName` length constraint
/// (`@length(1..200)`).
#[tokio::test]
async fn an_update_of_an_unknown_monitor_is_not_found() {
    let router = router();

    let (status, created) = create_monitor(&router, r#"{"monitorName":"deleted-later"}"#).await;
    assert_eq!(status, 200, "setup: CreateMonitor -> {status} {created:?}");
    let (status, deleted) = call(&router, "DELETE", "/monitors/deleted-later", None).await;
    assert_eq!(status, 200, "setup: DeleteMonitor -> {status} {deleted:?}");

    // (case, monitorName)
    let cases: Vec<(&str, String)> = vec![
        ("a name nothing ever created", "absent-monitor".to_string()),
        ("the name of a deleted monitor", "deleted-later".to_string()),
        ("the shortest name the pattern admits", "a".to_string()),
        (
            "the longest name the pattern admits",
            "z".repeat(200),
        ),
        ("a name of digits only", "1234567890".to_string()),
    ];

    for (case, name) in cases {
        let (status, refused) =
            update_monitor(&router, &name, r#"{"aggregationPeriod":60}"#).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {refused:?}"
        );
        // A refused update creates nothing.
        let (status, got) = get_monitor(&router, &name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused update must store nothing, got {status} {got:?}"
        );
    }
}
