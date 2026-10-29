// SPDX-License-Identifier: CC0-1.0

//! The bound every error message of this service stays inside, over the wire.
//!
//! The rule under test: a message is a MODELLED member, so whatever a refusal quotes from
//! the request is quoted within the `@length` maximum that member's target shape declares
//! — 1024 characters, from `GenericString`, which every error shape of this service
//! declares its `message` against (source:
//! `smithy_query(query="shape",shape="GenericString,ValidationException,ResourceNotFoundException")`).
//! The members a refusal quotes are bounded by their OWN models and not by that one:
//! `resourceTags` and `resourceTagKeys` each carry up to 200 tag keys of up to 128
//! characters (source: `smithy_query(query="shape",shape="ResourceTagList,ResourceTagKeyList,ResourceTagKey")`)
//! and every ARN member up to 2048 (source:
//! `smithy_query(query="shape",shape="ResourceArn,DashboardArn,ScheduledReportArn")`), so
//! a message that quoted them whole would run far past the maximum a client was told could
//! occur. A refusal therefore quotes WHOLE values — never half of one — and accounts for
//! the values it had no room for.
//!
//! The rows span that class rather than one request. For the reserved-tag-key refusal they
//! differ in how many keys the request carries (one, a few, half the modelled list, the
//! modelled maximum of 200), in how long each key is (short, 64 characters, the modelled
//! maximum of 128) and in the operation refused — both tagging writes and both create
//! paths, so both members that carry keys and both resource types. For the ARN templates
//! they differ in the ARN's length (the shortest well-formed form, 120 characters, exactly
//! the message bound, one past it, the modelled maximum of 2048), in the resource type and
//! in the operation that resolves it.
//!
//! Every call goes through the real router, so what these tests assert is what an SDK
//! client sees.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::state::{
    MAX_MESSAGE_LENGTH, RESERVED_TAG_KEY_PREFIX,
};

use super::dashboard_lifecycle::{call, error_code, ok_call, router};
use super::resource_not_found::assert_not_found;
use super::scheduled_report_lifecycle::EXECUTION_ROLE_ARN;
use super::untag_resource::{create_dashboard_with_tags, stored_tags, tags_json};

/// The modelled maximum length of a tag key (`ResourceTagKey`, `@length 1..128`, source:
/// `smithy_query(query="shape",shape="ResourceTagKey")`).
const MAX_TAG_KEY_LENGTH: usize = 128;

/// The modelled maximum number of entries one request's tag list carries
/// (`ResourceTagList` and `ResourceTagKeyList`, both `@length 0..200`, source:
/// `smithy_query(query="shape",shape="ResourceTagList,ResourceTagKeyList")`).
const MAX_TAG_LIST_LENGTH: usize = 200;

/// The modelled maximum length of an ARN member of this service (`@length 20..2048` on
/// `ResourceArn`, `DashboardArn` and `ScheduledReportArn`, source:
/// `smithy_query(query="shape",shape="ResourceArn,DashboardArn,ScheduledReportArn")`).
const MAX_ARN_LENGTH: usize = 2048;

/// `count` distinct keys inside the reserved namespace, each exactly `length` characters
/// long.
///
/// Every key of one row has the same length and a distinct index field, so no key is a
/// substring of another and a test can count the ones a message quotes. The prefix comes
/// from the handler-side constant rather than being re-spelled, and the modelled key
/// pattern is `^[\S\s]*$`, so each row reaches the handler rather than the boundary.
fn reserved_keys(count: usize, length: usize) -> Vec<String> {
    (0..count)
        .map(|index| {
            let head = format!("{RESERVED_TAG_KEY_PREFIX}{index:04}-");
            let padding = length.saturating_sub(head.chars().count());
            format!("{head}{}", "k".repeat(padding))
        })
        .collect()
}

/// Everything an ARN naming `kind` carries before its resource id.
fn arn_prefix(kind: &str) -> String {
    format!("arn:aws:bcm-dashboards::123456789012:{kind}/")
}

/// A well-formed ARN naming `kind`, exactly `length` characters long.
///
/// Satisfies the modelled pattern
/// `^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:(dashboard|scheduled-report)/(\*|[-a-z0-9]+)$`
/// and the 20..2048 length, so every row reaches the handler instead of being refused by
/// the boundary validator.
fn arn_of_length(kind: &str, length: usize) -> String {
    let prefix = arn_prefix(kind);
    let id_length = length
        .checked_sub(prefix.chars().count())
        .filter(|id_length| *id_length >= 1)
        .unwrap_or_else(|| {
            panic!(
                "an ARN naming a {kind} cannot be {length} characters long: its fixed fields \
                 alone take {}",
                prefix.chars().count()
            )
        });
    format!("{prefix}{}", "a".repeat(id_length))
}

/// The ARN lengths every operation that resolves one is driven over: the shortest
/// well-formed ARN of that resource type, a mid-sized one, one exactly as long as the
/// message bound, one past it, and one at the modelled maximum.
fn arn_lengths(kind: &str) -> Vec<(String, usize)> {
    // The fixed fields plus a one-character resource id, which the pattern admits.
    let shortest = arn_prefix(kind).chars().count() + 1;
    vec![
        (
            format!("the shortest well-formed {kind} ARN ({shortest} characters)"),
            shortest,
        ),
        ("a 120-character ARN".to_string(), 120),
        (
            format!("an ARN as long as the message bound ({MAX_MESSAGE_LENGTH} characters)"),
            MAX_MESSAGE_LENGTH,
        ),
        (
            "an ARN one character past the message bound".to_string(),
            MAX_MESSAGE_LENGTH + 1,
        ),
        (
            format!("an ARN at the modelled maximum length ({MAX_ARN_LENGTH} characters)"),
            MAX_ARN_LENGTH,
        ),
    ]
}

/// The message of an error body: present, and within the maximum the member carrying it
/// declares. Returned so a caller can state more about it.
fn message_within_bound(case: &str, body: &Value) -> String {
    let message = body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the error body carries no message: {body}"));
    let length = message.chars().count();
    assert!(
        length <= MAX_MESSAGE_LENGTH,
        "case {case:?}: a message member holds at most {MAX_MESSAGE_LENGTH} characters, got \
         {length}: {message:?}"
    );
    message.to_string()
}

/// The refusal of a request carrying `keys` in `member`: HTTP 400 `ValidationException`,
/// a message inside the bound that names the member, quotes whole offending keys, and
/// accounts by count for the ones it had no room for.
async fn assert_reserved_key_refusal(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    case: &str,
    member: &str,
    keys: &[String],
) {
    let (status, response) = call(router, operation, body).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400 from {operation}, got {status} with {response}"
    );
    let code = error_code(&response);
    assert!(
        code.ends_with("ValidationException"),
        "case {case:?}: expected ValidationException from {operation}, got {code:?} with \
         {response}"
    );
    let message = message_within_bound(case, &response);
    assert!(
        message.contains(member),
        "case {case:?}: the refusal from {operation} must name the member {member:?}, got \
         {message:?}"
    );
    let quoted = keys
        .iter()
        .filter(|key| message.contains(key.as_str()))
        .count();
    assert!(
        quoted >= 1,
        "case {case:?}: the refusal from {operation} must quote at least one whole offending \
         key, got {message:?}"
    );
    let omitted = keys.len() - quoted;
    assert!(
        omitted == 0 || message.contains(&format!("{omitted} more")),
        "case {case:?}: the refusal from {operation} quoted {quoted} of {} keys, so it must \
         account for the other {omitted}, got {message:?}",
        keys.len()
    );
}

/// The rows of the reserved-key table: key counts from one to the modelled maximum, each
/// at three key lengths up to the modelled maximum.
fn key_rows() -> Vec<(String, Vec<String>)> {
    let counts = [
        ("one key".to_string(), 1),
        ("two keys".to_string(), 2),
        ("eight keys".to_string(), 8),
        (
            format!("{} keys", MAX_TAG_LIST_LENGTH / 2),
            MAX_TAG_LIST_LENGTH / 2,
        ),
        (
            format!("the modelled maximum of {MAX_TAG_LIST_LENGTH} keys"),
            MAX_TAG_LIST_LENGTH,
        ),
    ];
    let lengths = [
        ("12 characters each".to_string(), 12),
        ("64 characters each".to_string(), 64),
        (
            format!("the modelled maximum of {MAX_TAG_KEY_LENGTH} characters each"),
            MAX_TAG_KEY_LENGTH,
        ),
    ];
    let mut rows = Vec::new();
    for (count_label, count) in &counts {
        for (length_label, length) in &lengths {
            rows.push((
                format!("{count_label}, {length_label}"),
                reserved_keys(*count, *length),
            ));
        }
    }
    rows
}

/// Every path that takes caller-supplied tag keys refuses a reserved one with a message
/// inside the modelled maximum, however many keys the request names and however long they
/// are — and the refused tagging still stores nothing.
#[tokio::test]
async fn the_reserved_key_refusal_stays_inside_the_message_bound() {
    let router = router();
    let dashboard = create_dashboard_with_tags(&router, "Message Bound Board", &[]).await;
    let report = ok_call(
        &router,
        "CreateScheduledReport",
        &json!({
            "scheduledReport": {
                "name": "Message Bound Report",
                "dashboardArn": dashboard,
                "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"},
            },
        }),
    )
    .await["arn"]
        .as_str()
        .expect("CreateScheduledReport answers an arn")
        .to_string();

    for (label, keys) in key_rows() {
        let tags: Vec<(String, String)> = keys
            .iter()
            .map(|key| (key.clone(), "x".to_string()))
            .collect();

        // The two tagging writes, on both resource types a `ResourceArn` names.
        for (kind, arn) in [("dashboard", &dashboard), ("scheduled-report", &report)] {
            assert_reserved_key_refusal(
                &router,
                "TagResource",
                &json!({"resourceArn": arn, "resourceTags": tags_json(&tags)}),
                &format!("TagResource on a {kind}: {label}"),
                "resourceTags",
                &keys,
            )
            .await;
            assert_reserved_key_refusal(
                &router,
                "UntagResource",
                &json!({"resourceArn": arn, "resourceTagKeys": &keys}),
                &format!("UntagResource on a {kind}: {label}"),
                "resourceTagKeys",
                &keys,
            )
            .await;
        }

        // And the two paths that take tag keys at create time.
        assert_reserved_key_refusal(
            &router,
            "CreateDashboard",
            &json!({
                "name": "Tagged At Create",
                "widgets": [],
                "resourceTags": tags_json(&tags),
            }),
            &format!("CreateDashboard: {label}"),
            "resourceTags",
            &keys,
        )
        .await;
        assert_reserved_key_refusal(
            &router,
            "CreateScheduledReport",
            &json!({
                "scheduledReport": {
                    "name": "Tagged At Create",
                    "dashboardArn": dashboard,
                    "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                    "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"},
                },
                "resourceTags": tags_json(&tags),
            }),
            &format!("CreateScheduledReport: {label}"),
            "resourceTags",
            &keys,
        )
        .await;
    }

    // A bounded refusal is still a refusal: none of those requests stored a tag.
    for (kind, arn) in [("dashboard", &dashboard), ("scheduled-report", &report)] {
        let stored = stored_tags(&router, arn, kind).await;
        assert!(
            stored.is_empty(),
            "case the refused tagging of a {kind}: expected no stored tags, got {stored:?}"
        );
    }
}

/// Every operation that resolves an ARN. `UpdateDashboard` carries a second required
/// member, and the tagging writes a tag list, so each request is completed below.
const ADDRESSED: &[(&str, &str, &str)] = &[
    ("GetDashboard", "arn", "dashboard"),
    ("UpdateDashboard", "arn", "dashboard"),
    ("GetResourcePolicy", "resourceArn", "dashboard"),
    ("GetScheduledReport", "arn", "scheduled-report"),
    ("DeleteScheduledReport", "arn", "scheduled-report"),
    ("ListTagsForResource", "resourceArn", "dashboard"),
    ("ListTagsForResource", "resourceArn", "scheduled-report"),
    ("TagResource", "resourceArn", "scheduled-report"),
    ("UntagResource", "resourceArn", "dashboard"),
];

/// The request `operation` sends for `arn`, carrying whatever else the model requires of
/// it.
fn addressed_request(operation: &str, member: &str, arn: &str) -> Value {
    let mut body = serde_json::Map::new();
    body.insert(member.to_string(), Value::String(arn.to_string()));
    match operation {
        "UpdateDashboard" => {
            body.insert("name".to_string(), json!("Back From The Dead"));
        }
        "TagResource" => {
            body.insert(
                "resourceTags".to_string(),
                json!([{"key": "env", "value": "prod"}]),
            );
        }
        "UntagResource" => {
            body.insert("resourceTagKeys".to_string(), json!(["env"]));
        }
        _ => {}
    }
    Value::Object(body)
}

/// A not-found answer quotes the ARN the request addressed within the message bound: the
/// whole ARN while the template has room for it, and never a message past the maximum
/// however long the ARN is. The `resourceId` member keeps the identifier whole throughout,
/// so nothing a client needs is lost to the bound.
#[tokio::test]
async fn an_unresolvable_arn_is_quoted_within_the_message_bound() {
    let router = router();
    // A populated store, so a miss is a miss beside live records.
    let live = create_dashboard_with_tags(&router, "Addressed Board", &[]).await;
    ok_call(
        &router,
        "CreateScheduledReport",
        &json!({
            "scheduledReport": {
                "name": "Addressed Report",
                "dashboardArn": live,
                "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"},
            },
        }),
    )
    .await;

    for &(operation, member, kind) in ADDRESSED {
        // The fixed part of this operation's template, measured on an ARN short enough
        // that the whole of it fits: what is left is the room an ARN has before the bound
        // takes over. An operation whose refusal quotes no ARN at all (the tagging
        // not-found is one constant sentence) is driven over the same rows, and only the
        // bound itself is asserted for it.
        let mut fixed_length: Option<usize> = None;
        for (label, length) in arn_lengths(kind) {
            let arn = arn_of_length(kind, length);
            let case = format!("{operation} addressing {label}");
            let (status, body) = call(&router, operation, &addressed_request(operation, member, &arn)).await;
            assert_not_found(&case, &arn, status, &body);
            let message = message_within_bound(&case, &body);

            let quotes_whole_arn = message.contains(arn.as_str());
            let fixed = *fixed_length.get_or_insert_with(|| {
                if quotes_whole_arn {
                    message.chars().count() - arn.chars().count()
                } else {
                    // The template quotes no ARN, so it leaves no room for one either.
                    MAX_MESSAGE_LENGTH + 1
                }
            });
            if fixed + arn.chars().count() <= MAX_MESSAGE_LENGTH {
                assert!(
                    quotes_whole_arn,
                    "case {case:?}: the template has room for the whole ARN, so the message \
                     must quote it, got {message:?}"
                );
            }
        }
    }
}

/// The two operations that resolve a `dashboardArn` beside the resource they address
/// obey the same bound: `CreateScheduledReport` refuses an unknown dashboard with its
/// modelled `ValidationException`, `UpdateScheduledReport` with the not-found answer, and
/// neither message runs past the maximum however long the ARN is.
#[tokio::test]
async fn a_quoted_dashboard_arn_is_bounded_on_the_report_write_paths() {
    let router = router();
    let dashboard = create_dashboard_with_tags(&router, "Report Source Board", &[]).await;
    let report = ok_call(
        &router,
        "CreateScheduledReport",
        &json!({
            "scheduledReport": {
                "name": "Repointed Report",
                "dashboardArn": dashboard,
                "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"},
            },
        }),
    )
    .await["arn"]
        .as_str()
        .expect("CreateScheduledReport answers an arn")
        .to_string();

    for (label, length) in arn_lengths("dashboard") {
        let absent = arn_of_length("dashboard", length);

        let case = format!("CreateScheduledReport naming {label}");
        let (status, body) = call(
            &router,
            "CreateScheduledReport",
            &json!({
                "scheduledReport": {
                    "name": format!("Bound Probe {length}"),
                    "dashboardArn": absent,
                    "scheduledReportExecutionRoleArn": EXECUTION_ROLE_ARN,
                    "scheduleConfig": {"scheduleExpression": "cron(0 8 1 * ? *)"},
                },
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        let code = error_code(&body);
        assert!(
            code.ends_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {code:?} with {body}"
        );
        message_within_bound(&case, &body);

        let case = format!("UpdateScheduledReport repointed at {label}");
        let (status, body) = call(
            &router,
            "UpdateScheduledReport",
            &json!({"arn": report, "dashboardArn": absent}),
        )
        .await;
        assert_not_found(&case, &absent, status, &body);
        message_within_bound(&case, &body);
    }
}
