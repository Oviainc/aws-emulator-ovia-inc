// SPDX-License-Identifier: CC0-1.0

//! What this service answers when the ARN a request addresses names no stored resource.
//!
//! The rule under test, over every operation whose handler resolves an ARN against this
//! service's stored resources: such a request is refused with **HTTP 400**, the wire error
//! code `ResourceNotFoundException`, and a `resourceId` member echoing the ARN the request
//! carried — and it changes nothing a later caller can see. The status is the recorded
//! service's, not the model's: the Smithy model declares that shape with `@httpError 404`
//! (`smithy_query(query="operation",operation="GetDashboard")`), while every recorded
//! not-found answer in the wire corpus is 400 and the recorded `GetDashboard` body carries
//! `resourceId`, which the modelled shape does not even declare.
//!
//! The rows below are a TABLE spanning that class rather than the one recorded request:
//! they differ in the OPERATION refused (a dashboard read, a dashboard update, a policy
//! read, a scheduled-report read and all three tagging operations — including the two that
//! would have written), in the RESOURCE TYPE the ARN names (`dashboard` and `scheduled-report`,
//! the two `ResourceArn` admits), in the ARN's FORM (partition, account id, the wildcard
//! resource id the pattern admits, a one-character id, a 32-hex id of the form this
//! emulator mints) and in the resource's HISTORY (never created, versus created and then
//! deleted). The store is populated throughout, so a miss is a miss beside live records.
//!
//! Messages are NOT asserted beyond being present: a status and an error code are wire
//! requirements, a message only where the rule producing it is known, and the per-operation
//! not-found templates are owned by the separate behavioral issue that records them.
//!
//! Every call goes through the real router, so what these tests assert is what an SDK
//! client sees.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::dashboard_lifecycle::{call, error_code, ok_call, router};
use super::scheduled_report_lifecycle::EXECUTION_ROLE_ARN;

/// The wire error code every not-found answer of this service carries, on every operation
/// — the same code the modelled `com.amazonaws.bcmdashboards#ResourceNotFoundException`
/// puts on the wire.
const NOT_FOUND_CODE: &str = "ResourceNotFoundException";

/// The HTTP status the recorded service answers a not-found with (the model says 404; the
/// wire record says 400, and the wire record is what a client sees).
const NOT_FOUND_STATUS: u16 = 400;

/// One operation that resolves an ARN against the stored resources, and how a request to
/// it is built.
struct Reader {
    operation: &'static str,
    /// The input member carrying the ARN.
    member: &'static str,
    /// The resource types that member's modelled `@pattern` admits — `dashboard` for
    /// `DashboardArn`, `scheduled-report` for `ScheduledReportArn`, both for `ResourceArn`
    /// (source: `smithy_query(query="operation",operation="<Op>")`).
    kinds: &'static [&'static str],
    /// The other required members of a well-formed request, so the refusal under test is
    /// the not-found and not a boundary refusal.
    others: &'static [(&'static str, &'static str)],
}

/// Every operation of this service whose handler resolves an ARN against stored state.
/// `DeleteDashboard` is absent because it models no not-found error at all and answers
/// its modelled `ValidationException` instead (pinned by `dashboard_lifecycle`).
const READERS: &[Reader] = &[
    Reader {
        operation: "GetDashboard",
        member: "arn",
        kinds: &["dashboard"],
        others: &[],
    },
    Reader {
        operation: "UpdateDashboard",
        member: "arn",
        kinds: &["dashboard"],
        others: &[("name", "Back From The Dead")],
    },
    Reader {
        operation: "GetResourcePolicy",
        member: "resourceArn",
        kinds: &["dashboard"],
        others: &[],
    },
    Reader {
        operation: "GetScheduledReport",
        member: "arn",
        kinds: &["scheduled-report"],
        others: &[],
    },
    Reader {
        operation: "ListTagsForResource",
        member: "resourceArn",
        kinds: &["dashboard", "scheduled-report"],
        others: &[],
    },
    Reader {
        operation: "UntagResource",
        member: "resourceArn",
        kinds: &["dashboard", "scheduled-report"],
        others: &[],
    },
    Reader {
        operation: "TagResource",
        member: "resourceArn",
        kinds: &["dashboard", "scheduled-report"],
        others: &[],
    },
];

/// The request `reader` sends for `arn`.
fn request(reader: &Reader, arn: &str) -> Value {
    let mut body = serde_json::Map::new();
    body.insert(reader.member.to_string(), Value::String(arn.to_string()));
    for (member, value) in reader.others {
        body.insert((*member).to_string(), Value::String((*value).to_string()));
    }
    // `UntagResource`'s and `TagResource`'s second required member is a list, so each is
    // added here rather than in the string-valued `others` table.
    if reader.operation == "UntagResource" {
        body.insert("resourceTagKeys".to_string(), json!(["env"]));
    }
    if reader.operation == "TagResource" {
        body.insert(
            "resourceTags".to_string(),
            json!([{"key": "env", "value": "prod"}]),
        );
    }
    Value::Object(body)
}

/// Well-formed ARNs of `kind` that no store this suite builds holds a resource under,
/// each row differing in the part of the ARN it varies. Every one satisfies the modelled
/// pattern `^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:(dashboard|scheduled-report)/(\*|[-a-z0-9]+)$`
/// and its 20..2048 length (source:
/// `smithy_query(query="operation",operation="ListTagsForResource")`), so each reaches the
/// handler rather than the boundary validator.
fn absent_arns(kind: &str) -> Vec<(String, String)> {
    [
        ("an id never created", "aws", "000000000000", "never-created"),
        (
            "the wildcard resource id the pattern admits",
            "aws",
            "000000000000",
            "*",
        ),
        ("a partition other than aws", "aws-cn", "000000000000", "elsewhere"),
        (
            "an account id nothing was minted under",
            "aws",
            "999999999999",
            "never-created",
        ),
        (
            "a 32-hex id of the form this emulator mints, never minted",
            "aws",
            "000000000000",
            "ffffffffffffffffffffffffffffffff",
        ),
        ("a one-character id", "aws", "000000000000", "a"),
    ]
    .iter()
    .map(|(label, partition, account, id)| {
        (
            (*label).to_string(),
            format!("arn:{partition}:bcm-dashboards::{account}:{kind}/{id}"),
        )
    })
    .collect()
}

/// Assert the one rule on one answer: HTTP 400, the not-found code, and `resourceId`
/// echoing the ARN the request addressed.
fn assert_not_found(label: &str, arn: &str, status: u16, body: &Value) {
    assert_eq!(
        status, NOT_FOUND_STATUS,
        "case {label}: expected HTTP {NOT_FOUND_STATUS}, got {status}: {body}"
    );
    let code = error_code(body);
    assert!(
        code.ends_with(NOT_FOUND_CODE),
        "case {label}: expected error code {NOT_FOUND_CODE}, got {code:?}: {body}"
    );
    let reported = body["resourceId"].as_str();
    assert_eq!(
        reported,
        Some(arn),
        "case {label}: expected resourceId {arn:?}, got {reported:?}: {body}"
    );
    let message = body["message"].as_str();
    assert!(
        message.is_some_and(|text| !text.is_empty()),
        "case {label}: expected a non-empty message, got {message:?}: {body}"
    );
}

/// A store holding one dashboard (with a tag) and one scheduled report, plus the ARN of a
/// dashboard that was created and then deleted. Returned as
/// `(live dashboard, live report, deleted dashboard)`.
async fn populated_store(router: &MultiServiceRouter) -> (String, String, String) {
    let live = ok_call(
        router,
        "CreateDashboard",
        &json!({
            "name": "Live Dashboard",
            "widgets": [],
            "resourceTags": [{"key": "env", "value": "prod"}]
        }),
    )
    .await["arn"]
        .as_str()
        .expect("CreateDashboard answers an arn")
        .to_string();

    let report = ok_call(
        router,
        "CreateScheduledReport",
        &json!({
            "scheduledReport": {
                "name": "Live Report",
                "dashboardArn": live,
                "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"}
            }
        }),
    )
    .await["arn"]
        .as_str()
        .expect("CreateScheduledReport answers an arn")
        .to_string();

    let doomed = ok_call(
        router,
        "CreateDashboard",
        &json!({"name": "Doomed Dashboard", "widgets": []}),
    )
    .await["arn"]
        .as_str()
        .expect("CreateDashboard answers an arn")
        .to_string();
    ok_call(router, "DeleteDashboard", &json!({"arn": doomed})).await;

    (live, report, doomed)
}

/// Every operation that resolves an ARN answers an ARN it cannot resolve with HTTP 400,
/// the `ResourceNotFoundException` code and the requested ARN in `resourceId` — for every
/// ARN form, both resource types, and a resource that was deleted as well as one that
/// never existed.
#[tokio::test]
async fn an_unresolvable_arn_is_refused_with_400_and_the_requested_resource_id() {
    let router = router();
    let (live, live_report, deleted) = populated_store(&router).await;

    for reader in READERS {
        let mut cases: Vec<(String, String)> = reader
            .kinds
            .iter()
            .flat_map(|kind| absent_arns(kind))
            .collect();
        // A resource that DID exist and was removed: the same refusal as one that never
        // existed, which is the whole point of resolving against stored state.
        if reader.kinds.contains(&"dashboard") {
            cases.push(("a dashboard created and then deleted".to_string(), deleted.clone()));
        }

        for (form, arn) in &cases {
            let label = format!("{} / {form}", reader.operation);
            let (status, body) = call(&router, reader.operation, &request(reader, arn)).await;
            assert_not_found(&label, arn, status, &body);
        }
    }

    // None of those refusals touched the store: the live records still read back, with the
    // tag the create stored still on the dashboard.
    let fetched = ok_call(&router, "GetDashboard", &json!({"arn": live})).await;
    assert_eq!(
        fetched["name"].as_str(),
        Some("Live Dashboard"),
        "case the live dashboard: the refusals must leave it alone, got {fetched}"
    );
    let tags = ok_call(&router, "ListTagsForResource", &json!({"resourceArn": live})).await;
    assert_eq!(
        tags["resourceTags"],
        json!([{"key": "env", "value": "prod"}]),
        "case the live dashboard: a refused tagging write must change no tag, got {tags}"
    );
    let report = ok_call(
        &router,
        "GetScheduledReport",
        &json!({"arn": live_report}),
    )
    .await;
    assert_eq!(
        report["scheduledReport"]["name"].as_str(),
        Some("Live Report"),
        "case the live report: the refusals must leave it alone, got {report}"
    );
    let listed = ok_call(&router, "ListDashboards", &json!({})).await;
    assert_eq!(
        listed["dashboards"].as_array().map(Vec::len),
        Some(1),
        "the refusals must neither create nor remove a dashboard: {listed}"
    );
}

/// A not-found and a malformed ARN now share the status line, so the WIRE CODE is what
/// tells them apart — and only the not-found carries `resourceId`.
#[tokio::test]
async fn a_not_found_and_a_malformed_arn_are_told_apart_by_their_code() {
    let router = router();
    let (live, _report, _deleted) = populated_store(&router).await;

    // Well-formed per `DashboardArn` but stored under nothing.
    let (status, body) = call(
        &router,
        "GetDashboard",
        &json!({"arn": "arn:aws:bcm-dashboards::000000000000:dashboard/never-created"}),
    )
    .await;
    assert_not_found(
        "a well-formed arn naming nothing",
        "arn:aws:bcm-dashboards::000000000000:dashboard/never-created",
        status,
        &body,
    );

    // Outside the modelled pattern: the boundary validator refuses it, with the same
    // status but the other code and no `resourceId`.
    let malformed = "arn:aws:bcm-dashboards::000000000000:dashboard/NOT-LOWERCASE";
    let (status, body) = call(&router, "GetDashboard", &json!({"arn": malformed})).await;
    assert_eq!(
        status, 400,
        "case a malformed arn: expected HTTP 400, got {status}: {body}"
    );
    assert!(
        error_code(&body).ends_with("ValidationException"),
        "case a malformed arn: expected ValidationException, got {}: {body}",
        error_code(&body)
    );
    assert_eq!(
        body.get("resourceId"),
        None,
        "case a malformed arn: a boundary refusal names no resource, got {body}"
    );

    // And the resource that IS stored still resolves, so neither refusal is a blanket
    // answer.
    let fetched = ok_call(&router, "GetDashboard", &json!({"arn": live})).await;
    assert_eq!(
        fetched["arn"].as_str(),
        Some(live.as_str()),
        "case the live dashboard: it must still resolve, got {fetched}"
    );
}
