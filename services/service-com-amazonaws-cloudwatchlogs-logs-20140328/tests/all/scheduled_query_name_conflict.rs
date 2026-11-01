// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the refusal `CreateScheduledQuery` answers a name the account already
//! holds a scheduled query under.
//!
//! The rule under test: a refusal about a scheduled query states this service's identifier
//! phrasing for the resource — the resource kind, then the identifier the request carried
//! verbatim, then the condition — so a create under a stored name is refused as
//! `ConflictException` (HTTP 400, "This operation attempted to create a resource that already
//! exists") saying "Scheduled query with identifier <identifier> already exists", the same
//! phrasing the not-found refusal of the same resource uses. Nothing the account generated (an
//! ARN, the account id, an instant) appears in the message, and the refused create leaves the
//! stored query exactly as it was.
//!
//! The table spans what the rule generalises over — the identifier VALUE, since that is its
//! only variable part: the shortest and the longest name `ScheduledQueryName` admits (@length
//! 1..300), names carrying the separators, spaces and non-ASCII characters the shape does not
//! constrain, and a name that is itself shaped like an ARN. Each row also varies what the
//! second create differs in (query language, query string, schedule, optional members), since
//! the refusal is about the name alone.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a response
//! that is not computed from the shared store cannot pass.

use serde_json::{json, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    scheduled_query_already_exists_message, scheduled_query_arn,
    scheduled_query_not_found_message,
};

use crate::harness::{Logs, REGION};

/// The operation under test, and the one each row's stored query is read back through.
const CREATE: &str = "CreateScheduledQuery";
const GET: &str = "GetScheduledQuery";

/// The code a create under a name the account already holds is refused with.
const CONFLICT: &str = "ConflictException";

/// The IAM role every row executes under: `CreateScheduledQuery$executionRoleArn`.
const ROLE: &str = "arn:aws:iam::000000000000:role/scheduled-query-execution";

/// One row: the name both creates name, and the members the SECOND create differs in — the
/// refusal is about the name, so what else the second create says must not change it.
struct Row {
    case: &'static str,
    name: String,
    /// One value of the model's `QueryLanguage` enum (`CWLI` | `SQL` | `PPL`).
    second_query_language: &'static str,
    second_query_string: &'static str,
    second_schedule_expression: &'static str,
    /// Extra members the second create names and the first did not.
    second_extras: Value,
}

/// The rows, spanning the identifier values `ScheduledQueryName` admits.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the shortest name the shape admits (@length min 1)",
            name: "q".to_string(),
            second_query_language: "SQL",
            second_query_string: "SELECT 1",
            second_schedule_expression: "cron(0 1 * * ? *)",
            second_extras: json!({}),
        },
        Row {
            case: "the longest name the shape admits (@length max 300)",
            name: "n".repeat(300),
            second_query_language: "PPL",
            second_query_string: "source=logs | head 1",
            second_schedule_expression: "cron(5 * * * ? *)",
            second_extras: json!({"description": "the second create"}),
        },
        Row {
            case: "a name carrying the usual resource separators",
            name: "orders.errors_daily-2026/q".to_string(),
            second_query_language: "CWLI",
            second_query_string: "fields @timestamp | limit 1",
            second_schedule_expression: "cron(0 12 * * ? *)",
            second_extras: json!({"startTimeOffset": 3600, "timezone": "UTC"}),
        },
        Row {
            case: "a name carrying spaces, which the shape does not constrain",
            name: "nightly orders report".to_string(),
            second_query_language: "CWLI",
            second_query_string: "fields @message",
            second_schedule_expression: "cron(0 2 * * ? *)",
            second_extras: json!({"endTimeOffset": 60}),
        },
        Row {
            case: "a non-ASCII name, which the shape does not constrain",
            name: "métriques-journalières-日本".to_string(),
            second_query_language: "SQL",
            second_query_string: "SELECT count(*)",
            second_schedule_expression: "cron(30 3 * * ? *)",
            second_extras: json!({"tags": {"team": "platform"}}),
        },
        Row {
            case: "a name shaped like an ARN, which is still just a name",
            name: scheduled_query_arn(REGION, "looks-like-an-arn"),
            second_query_language: "CWLI",
            second_query_string: "fields @timestamp, @message | limit 5",
            second_schedule_expression: "cron(0 4 * * ? *)",
            second_extras: json!({"description": "named by an ARN-shaped string"}),
        },
    ]
}

/// The create that stores each row's query: the members a create must name, and one optional
/// member whose value the refused create must not be able to replace.
fn first_create(name: &str) -> Value {
    json!({
        "name": name,
        "queryLanguage": "CWLI",
        "queryString": "fields @timestamp, @message | limit 10",
        "scheduleExpression": "cron(0 12 * * ? *)",
        "executionRoleArn": ROLE,
        "startTimeOffset": 3600,
        "description": "the first create",
    })
}

/// A create under a name the account already holds is refused as a conflict stating this
/// service's identifier phrasing, and the stored query is left exactly as it was.
#[tokio::test]
async fn a_duplicate_name_create_is_refused_in_the_services_identifier_phrasing() {
    let logs = Logs::new();

    for row in rows() {
        let Row {
            case,
            name,
            second_query_language,
            second_query_string,
            second_schedule_expression,
            second_extras,
        } = row;

        logs.ok(CREATE, &first_create(&name)).await;
        let before = logs.ok(GET, &json!({"identifier": &name})).await;

        let mut second = json!({
            "name": &name,
            "queryLanguage": second_query_language,
            "queryString": second_query_string,
            "scheduleExpression": second_schedule_expression,
            "executionRoleArn": ROLE,
        });
        for (key, value) in second_extras
            .as_object()
            .expect("a row's extra members are an object")
        {
            second[key] = value.clone();
        }

        // `refusal` asserts the HTTP 400 the class is modelled with and hands back the wire
        // code and message.
        let (code, message) = logs.refusal(CREATE, &second).await;
        assert_eq!(
            code, CONFLICT,
            "case {case:?}: expected {CONFLICT}, got {code} {message:?}"
        );

        let expected = scheduled_query_already_exists_message(&name);
        assert_eq!(
            message, expected,
            "case {case:?}: expected the refusal {expected:?}, got {message:?}"
        );

        // The identifier is the message's ONLY variable part: between the fixed opening and
        // the fixed closing of the class there is exactly the requested name, so nothing the
        // account generated (an ARN it minted, the account id, an instant) is in it.
        let middle = message
            .strip_prefix("Scheduled query with identifier ")
            .and_then(|rest| rest.strip_suffix(" already exists"));
        assert_eq!(
            middle,
            Some(name.as_str()),
            "case {case:?}: the fixed text of the class carries only the requested identifier, \
             got {message:?}"
        );

        let after = logs.ok(GET, &json!({"identifier": &name})).await;
        assert_eq!(
            after, before,
            "case {case:?}: the refused create replaces nothing, expected {before}, got {after}"
        );
    }
}

/// The conflict and the not-found refusals of a scheduled query are ONE phrasing family: for
/// any identifier, the two sentences differ only in the condition they end with — so a client
/// matching on the phrasing places both.
#[tokio::test]
async fn the_conflict_and_not_found_refusals_share_one_phrasing() {
    let logs = Logs::new();

    for row in rows() {
        let case = row.case;
        let name = row.name;

        // The missing-resource refusal of the same identifier, over the wire.
        let (_, not_found) = logs.refusal(GET, &json!({"identifier": &name})).await;
        assert_eq!(
            not_found,
            scheduled_query_not_found_message(&name),
            "case {case:?}: expected this service's not-found phrasing, got {not_found:?}"
        );

        logs.ok(CREATE, &first_create(&name)).await;
        let (code, conflict) = logs.refusal(CREATE, &first_create(&name)).await;
        assert_eq!(
            code, CONFLICT,
            "case {case:?}: expected {CONFLICT}, got {code} {conflict:?}"
        );

        let prefix = format!("Scheduled query with identifier {name} ");
        assert_eq!(
            not_found.strip_prefix(&prefix),
            Some("not found"),
            "case {case:?}: expected {prefix:?} then the condition, got {not_found:?}"
        );
        assert_eq!(
            conflict.strip_prefix(&prefix),
            Some("already exists"),
            "case {case:?}: expected {prefix:?} then the condition, got {conflict:?}"
        );
    }
}

/// The refusal is about the NAME, not about the account holding queries: a create under a name
/// the account holds nothing under still succeeds after one was refused, and the refusal of the
/// next duplicate names that second query rather than the first.
#[tokio::test]
async fn the_conflict_names_the_requested_identifier_not_a_previous_one() {
    let logs = Logs::new();
    const FIRST: &str = "orders-conflict-first";
    const SECOND: &str = "orders-conflict-second";

    logs.ok(CREATE, &first_create(FIRST)).await;
    let (_, first_refusal) = logs.refusal(CREATE, &first_create(FIRST)).await;
    assert_eq!(
        first_refusal,
        scheduled_query_already_exists_message(FIRST),
        "case {FIRST:?}: expected the refusal to name the requested identifier, got \
         {first_refusal:?}"
    );

    // A different name is free, so the create under it is accepted.
    logs.ok(CREATE, &first_create(SECOND)).await;
    let (code, second_refusal) = logs.refusal(CREATE, &first_create(SECOND)).await;
    assert_eq!(
        code, CONFLICT,
        "case {SECOND:?}: expected {CONFLICT}, got {code} {second_refusal:?}"
    );
    assert_eq!(
        second_refusal,
        scheduled_query_already_exists_message(SECOND),
        "case {SECOND:?}: expected the refusal to name the requested identifier, got \
         {second_refusal:?}"
    );
}
