// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteScheduledQuery`, the operation that removes a scheduled query
//! the account holds.
//!
//! The rule under test: a delete removes the ONE scheduled query the identifier names — "The
//! ARN or name of the scheduled query to delete", so either form reaches the same stored query
//! — together with everything that query held ("This operation also removes any configured
//! actions and associated resources"), so afterwards no reader reports it, the name it occupied
//! is free for a later create, and no other query of the account is touched. An identifier the
//! account holds no scheduled query under — including one a delete has already removed — is
//! refused as a missing resource in the sentence this service refuses one with, and removes
//! nothing; the query CloudWatch manages for a log alarm, which exists because that alarm's
//! configuration states it, is refused rather than removed.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a response
//! that is not computed from the shared store cannot pass, and every removal is read back
//! through `GetScheduledQuery` and `ListScheduledQueries`.
//!
//! The table spans what the rule generalises over rather than one request: the identifier form
//! the delete addresses the query by (its name and its ARN), each query language the model's
//! `QueryLanguage` enumerates (`CWLI`, `SQL`, `PPL`), log-group lists of no, one and many
//! identifiers in both the name and the ARN form, each destination the model's
//! `DestinationConfiguration` carries (Amazon S3, a lookup table, both at once) and none at
//! all, the state named explicitly and left to the documented default, tags present and absent,
//! a schedule window and offsets present and absent, and the removed query's position among the
//! queries the account keeps.

use serde_json::{json, Map, Value};

use service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;
use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    log_group_arn, scheduled_query_arn, ComAmazonawsCloudwatchlogsLogs20140328State,
};

use crate::harness::{Logs, REGION};

/// The operation under test, the create that puts a query in the store, and the readers every
/// removal is proved through.
const DELETE: &str = "DeleteScheduledQuery";
const CREATE: &str = "CreateScheduledQuery";
const GET: &str = "GetScheduledQuery";
const LIST: &str = "ListScheduledQueries";

/// The code answered when the resource a request names is not there.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The code answered for a parameter this operation will not accept ("One of the parameters for
/// the request is not valid").
const VALIDATION: &str = "ValidationException";

/// The refusal sentence of each class, pinned LITERALLY here rather than built with the helper
/// the handler itself calls, so a wording that is wrong but consistent with the implementation
/// cannot pass. The missing-query refusal names the `ScheduledQueryIdentifier` the request asked
/// for, whichever form it named the query by; the managed-query refusal names the query the
/// request asked for.
fn not_found_message(identifier: &str) -> String {
    format!("Scheduled query with identifier {identifier} not found")
}

fn managed_refusal_message(name: &str) -> String {
    format!("The scheduled query {name} is managed by CloudWatch and cannot be deleted.")
}

/// The IAM role every query executes under: "The ARN of the IAM role used to execute the query
/// and deliver results."
const ROLE: &str = "arn:aws:iam::000000000000:role/scheduled-query-execution";

/// The role a destination delivers results under, which the model gives its own member.
const DELIVERY_ROLE: &str = "arn:aws:iam::000000000000:role/scheduled-query-delivery";

/// The two queries the account keeps in every case, named so that the query under test sorts
/// before, between and after them as the rows vary its name.
const KEPT: [&str; 2] = ["aaa-kept-query", "mmm-kept-query"];

/// Every log group a row names, in either identifier form. A scheduled-query write resolves each
/// `logGroupIdentifiers` entry against the log groups the ACCOUNT holds, so these are created
/// before the queries that read them.
const GROUPS: [&str; 3] = [
    "/aws/lambda/orders",
    "/aws/lambda/payments",
    "/aws/apigateway/welcome",
];

/// Those log groups, created over the wire like any client's call.
async fn seed_log_groups(logs: &Logs) {
    for group in GROUPS {
        logs.ok("CreateLogGroup", &json!({"logGroupName": group})).await;
    }
}

/// The end of a scheduled query's lookback window when the create names none: the offset in
/// seconds a query that names no `endTimeOffset` resolves to.
const DEFAULT_END_TIME_OFFSET: i64 = 0;

/// Where a row delivers its results — the destinations the model's `DestinationConfiguration`
/// carries: `s3Configuration` ("Configuration for delivering query results to Amazon S3"),
/// `lookupTableConfiguration` ("Configuration for delivering query results to a lookup table")
/// or both at once ("You can configure one or both destination types").
#[derive(Clone, Copy, Debug)]
enum Destination {
    S3,
    LookupTable,
    Both,
}

impl Destination {
    /// This destination as a request states it.
    fn request(self) -> Value {
        let s3 = json!({
            "destinationIdentifier": "s3://scheduled-query-results/daily",
            "roleArn": DELIVERY_ROLE,
            "ownerAccountId": "000000000000",
        });
        let lookup_table = json!({
            "tableName": "scheduled_query_results",
            "roleArn": DELIVERY_ROLE,
            "description": "the rows this query refreshes",
        });
        match self {
            Self::S3 => json!({"s3Configuration": s3}),
            Self::LookupTable => json!({"lookupTableConfiguration": lookup_table}),
            Self::Both => json!({"s3Configuration": s3, "lookupTableConfiguration": lookup_table}),
        }
    }
}

/// One removal: the query that is created, the form of `ScheduledQueryIdentifier` the delete
/// addresses it by, and the configuration it held when it was removed — everything the delete
/// must take with it.
#[derive(Debug)]
struct Row {
    /// What this row varies, for the assertion messages.
    case: &'static str,
    name: &'static str,
    /// Whether the delete names the query by its ARN rather than by its name; both forms of
    /// `ScheduledQueryIdentifier` reach the one query.
    by_arn: bool,
    /// One value of the model's `QueryLanguage` enum (`CWLI` | `SQL` | `PPL`).
    query_language: &'static str,
    query_string: &'static str,
    /// "An array of log group names or ARNs to query", of no, one or many identifiers, in either
    /// form; `None` names no log group at all.
    log_groups: Option<&'static [&'static str]>,
    /// Whether the log groups are named by their ARNs rather than their names.
    groups_by_arn: bool,
    destination: Option<Destination>,
    /// The state the create names, or `None` to leave it at the documented `ENABLED` default.
    state: Option<&'static str>,
    /// Whether the create names the optional window, offsets, timezone and description.
    window: bool,
    /// Whether the create assigns tags.
    tags: bool,
}

impl Row {
    /// The create that puts this row's query in the store.
    fn create_request(&self) -> Value {
        let mut body = Map::new();
        body.insert("name".to_string(), json!(self.name));
        body.insert("queryLanguage".to_string(), json!(self.query_language));
        body.insert("queryString".to_string(), json!(self.query_string));
        body.insert(
            "scheduleExpression".to_string(),
            json!("cron(0 12 * * ? *)"),
        );
        body.insert("executionRoleArn".to_string(), json!(ROLE));
        if let Some(groups) = self.log_groups {
            let identifiers: Vec<String> = groups
                .iter()
                .map(|group| {
                    if self.groups_by_arn {
                        log_group_arn(REGION, group, false)
                    } else {
                        (*group).to_string()
                    }
                })
                .collect();
            body.insert("logGroupIdentifiers".to_string(), json!(identifiers));
        }
        if let Some(destination) = self.destination {
            body.insert(
                "destinationConfiguration".to_string(),
                destination.request(),
            );
        }
        if let Some(state) = self.state {
            body.insert("state".to_string(), json!(state));
        }
        if self.window {
            body.insert(
                "description".to_string(),
                json!("the hourly error roll-up"),
            );
            body.insert("timezone".to_string(), json!("Europe/London"));
            body.insert("startTimeOffset".to_string(), json!(3_600));
            body.insert("endTimeOffset".to_string(), json!(60));
            body.insert("scheduleStartTime".to_string(), json!(1_700_000_000_000i64));
            body.insert("scheduleEndTime".to_string(), json!(1_800_000_000_000i64));
        }
        if self.tags {
            body.insert("tags".to_string(), json!({"team": "orders"}));
        }
        Value::Object(body)
    }

    /// The identifier the delete names this query by.
    fn identifier(&self) -> String {
        if self.by_arn {
            scheduled_query_arn(REGION, self.name)
        } else {
            self.name.to_string()
        }
    }
}

/// The rows: one per shape of removal the rule covers.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "by name, every optional member named, both destinations",
            name: "zzz-orders-errors-hourly",
            by_arn: false,
            query_language: "CWLI",
            query_string: "fields @timestamp, @message | filter @message like /ERROR/",
            log_groups: Some(&["/aws/lambda/orders", "/aws/lambda/payments"]),
            groups_by_arn: false,
            destination: Some(Destination::Both),
            state: Some("DISABLED"),
            window: true,
            tags: true,
        },
        Row {
            case: "by ARN, no optional member named at all",
            name: "bbb-bare-query",
            by_arn: true,
            query_language: "SQL",
            query_string: "SELECT count(*) FROM logs",
            log_groups: None,
            groups_by_arn: false,
            destination: None,
            state: None,
            window: false,
            tags: false,
        },
        Row {
            case: "by ARN, one log group in its ARN form, S3 destination",
            name: "ddd-ppl-rollup",
            by_arn: true,
            query_language: "PPL",
            query_string: "source=logs | stats count() by level",
            log_groups: Some(&["/aws/lambda/orders"]),
            groups_by_arn: true,
            destination: Some(Destination::S3),
            state: Some("ENABLED"),
            window: false,
            tags: true,
        },
        Row {
            case: "by name, many log groups in their ARN form, lookup-table destination",
            name: "nnn-lookup-refresh",
            by_arn: false,
            query_language: "CWLI",
            query_string: "fields @message | stats count() by bin(5m)",
            log_groups: Some(&[
                "/aws/lambda/orders",
                "/aws/lambda/payments",
                "/aws/apigateway/welcome",
            ]),
            groups_by_arn: true,
            destination: Some(Destination::LookupTable),
            state: None,
            window: true,
            tags: false,
        },
        Row {
            case: "by name, an empty query string and no destination",
            name: "aab-empty-query-string",
            by_arn: false,
            query_language: "CWLI",
            query_string: "",
            log_groups: Some(&["/aws/lambda/orders"]),
            groups_by_arn: false,
            destination: None,
            state: Some("DISABLED"),
            window: false,
            tags: true,
        },
    ]
}

/// The names `ListScheduledQueries` reports, in the order it reports them.
async fn listed(logs: &Logs) -> Vec<String> {
    let page = logs.ok(LIST, &json!({})).await;
    page.get("scheduledQueries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{LIST} must return scheduledQueries, got {page}"))
        .iter()
        .map(|summary| {
            summary
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("every summary carries a name, got {summary}"))
                .to_string()
        })
        .collect()
}

/// The names the store itself holds, which is what a later create's uniqueness check reads.
fn held(logs: &Logs) -> Vec<String> {
    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let names: Vec<String> = state.lock().scheduled_queries.keys().cloned().collect();
    names
}

/// A delete removes exactly the query its identifier names — under either form of that
/// identifier and whatever the query held — leaving every other query of the account as it was,
/// and frees the name for a later create.
#[tokio::test]
async fn a_delete_removes_exactly_the_query_it_names() {
    for row in rows() {
        let case = row.case;
        let logs = Logs::new();
        seed_log_groups(&logs).await;

        for kept in KEPT {
            logs.ok(
                CREATE,
                &json!({
                    "name": kept,
                    "queryLanguage": "CWLI",
                    "queryString": "fields @message",
                    "scheduleExpression": "rate(5 minutes)",
                    "executionRoleArn": ROLE,
                }),
            )
            .await;
        }
        logs.ok(CREATE, &row.create_request()).await;

        // What the queries the account keeps report BEFORE the removal, so the removal can be
        // shown to have left them untouched.
        let mut before = Vec::new();
        for kept in KEPT {
            before.push(logs.ok(GET, &json!({"identifier": kept})).await);
        }

        let identifier = row.identifier();
        let (status, body) = logs
            .call(DELETE, &json!({"identifier": identifier}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a delete of {identifier:?} is accepted, expected HTTP 200, got \
             {status} {body}"
        );
        assert!(
            body.as_object().is_none_or(Map::is_empty),
            "case {case:?}: the response of {DELETE} carries no members, got {body}"
        );

        // The removed query is reported by no reader any more, under EITHER form of its
        // identifier.
        for form in [row.name.to_string(), scheduled_query_arn(REGION, row.name)] {
            let (code, message) = logs.refusal(GET, &json!({"identifier": form})).await;
            assert_eq!(
                code, NOT_FOUND,
                "case {case:?}: a read of the removed query under {form:?} expected {NOT_FOUND}, \
                 got {code} {message:?}"
            );
            let expected = not_found_message(&form);
            assert_eq!(
                message, expected,
                "case {case:?}: expected the refusal {expected:?}, got {message:?}"
            );
        }

        let names = listed(&logs).await;
        let expected: Vec<String> = KEPT.iter().map(|kept| (*kept).to_string()).collect();
        assert_eq!(
            names, expected,
            "case {case:?}: the account keeps every other query, expected {expected:?}, got \
             {names:?}"
        );
        let stored = held(&logs);
        assert_eq!(
            stored, expected,
            "case {case:?}: the store holds every other query and nothing of the removed one, \
             expected {expected:?}, got {stored:?}"
        );

        for (kept, was) in KEPT.iter().zip(&before) {
            let now = logs.ok(GET, &json!({"identifier": kept})).await;
            assert_eq!(
                &now, was,
                "case {case:?}: the delete leaves {kept} exactly as it was, expected {was}, got \
                 {now}"
            );
        }

        // The name the removed query occupied is free again, and the query created under it
        // carries only what THAT create states: nothing of the removed configuration survives.
        let recreated = logs
            .ok(
                CREATE,
                &json!({
                    "name": row.name,
                    "queryLanguage": "CWLI",
                    "queryString": "fields @message",
                    "scheduleExpression": "rate(1 hour)",
                    "executionRoleArn": ROLE,
                }),
            )
            .await;
        let minted = recreated.get("scheduledQueryArn").and_then(Value::as_str);
        let arn = scheduled_query_arn(REGION, row.name);
        assert_eq!(
            minted,
            Some(arn.as_str()),
            "case {case:?}: a create under the freed name mints its ARN again, expected \
             {arn:?}, got {minted:?}"
        );
        let fresh = logs.ok(GET, &json!({"identifier": row.name})).await;
        for (member, expected) in [
            ("queryString", json!("fields @message")),
            ("scheduleExpression", json!("rate(1 hour)")),
            ("state", json!("ENABLED")),
        ] {
            let got = fresh.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                got, expected,
                "case {case:?}: the recreated query reports its own {member}, expected \
                 {expected}, got {got}"
            );
        }
        for member in [
            "description",
            "timezone",
            "startTimeOffset",
            "scheduleStartTime",
            "scheduleEndTime",
            "destinationConfiguration",
            "logGroupIdentifiers",
        ] {
            let got = fresh.get(member);
            assert!(
                got.is_none(),
                "case {case:?}: the recreated query carries nothing of the removed one's \
                 {member}, got {got:?}"
            );
        }
        // `endTimeOffset` has an effective value whether or not a create names one, so the
        // recreated query reports the offset it resolves to rather than the removed one's.
        let end_offset = fresh.get("endTimeOffset").and_then(Value::as_i64);
        assert_eq!(
            end_offset,
            Some(DEFAULT_END_TIME_OFFSET),
            "case {case:?}: the recreated query carries its own end offset, expected \
             {DEFAULT_END_TIME_OFFSET}, got {end_offset:?}"
        );
    }
}

/// An identifier the account holds no scheduled query under is refused as a missing resource —
/// including the identifier of a query an earlier delete already removed — and removes nothing.
#[tokio::test]
async fn a_delete_of_an_unknown_identifier_is_refused_and_removes_nothing() {
    let logs = Logs::new();

    let gone = "orders-errors-hourly";
    for name in KEPT.iter().chain([&gone]) {
        logs.ok(
            CREATE,
            &json!({
                "name": name,
                "queryLanguage": "CWLI",
                "queryString": "fields @message",
                "scheduleExpression": "rate(5 minutes)",
                "executionRoleArn": ROLE,
            }),
        )
        .await;
    }
    logs.ok(DELETE, &json!({"identifier": gone})).await;

    // A name no query is stored under, a name differing only in case, the ARN form of such a
    // name, the ARN of another resource kind entirely, a value that is neither, and both forms
    // of the query a delete has already removed.
    let cases = [
        "orders-missing".to_string(),
        KEPT[0].to_uppercase(),
        scheduled_query_arn(REGION, "orders-missing"),
        log_group_arn(REGION, "/aws/lambda/orders", false),
        "not-an-arn-and-not-a-name".to_string(),
        gone.to_string(),
        scheduled_query_arn(REGION, gone),
    ];

    for identifier in cases {
        let (code, message) = logs.refusal(DELETE, &json!({"identifier": identifier})).await;
        assert_eq!(
            code, NOT_FOUND,
            "case {identifier:?}: expected {NOT_FOUND}, got {code} {message:?}"
        );
        let expected_message = not_found_message(&identifier);
        assert_eq!(
            message, expected_message,
            "case {identifier:?}: expected the refusal {expected_message:?}, got {message:?}"
        );

        let stored = held(&logs);
        let expected: Vec<String> = KEPT.iter().map(|kept| (*kept).to_string()).collect();
        assert_eq!(
            stored, expected,
            "case {identifier:?}: a refused delete removes nothing, expected {expected:?}, got \
             {stored:?}"
        );
    }
}

/// Put a log alarm in the CloudWatch store, so the account holds the scheduled query CloudWatch
/// manages for it.
///
/// `PutLogAlarm` belongs to the CloudWatch service, which this target does not mount, so the
/// alarm is seeded on the shared engine the way `harness::report_anomaly` seeds a detected
/// anomaly; everything the suite then asserts goes over the wire like any client's call.
fn seed_log_alarm(logs: &Logs, alarm: &str, query_arn: Option<&str>) {
    let state = logs
        .engine()
        .state::<cloudwatch::state::ComAmazonawsCloudwatchGraniteserviceversion20100801State>();
    let mut store = state.lock();
    store.log_alarms.insert(
        alarm.to_string(),
        cloudwatch::state::StoredLogAlarm {
            alarm: cloudwatch::ops::types::LogAlarm {
                alarm_name: Some(alarm.to_string()),
                alarm_configuration_updated_timestamp: Some(
                    ::aws_smithy_types::DateTime::from_millis(1_700_000_000_000),
                ),
                scheduled_query_configuration: Some(
                    cloudwatch::ops::types::ScheduledQueryConfiguration {
                        query_string: "fields @message | stats count() by bin(5m)".to_string(),
                        log_group_identifiers: Some(vec!["/aws/lambda/orders".to_string()]),
                        query_arn: query_arn.map(str::to_string),
                        scheduled_query_role_arn: ROLE.to_string(),
                        schedule_configuration: cloudwatch::ops::types::ScheduleConfiguration {
                            schedule_expression: "rate(5 minutes)".to_string(),
                            start_time_offset: 900,
                            end_time_offset: None,
                        },
                        ..cloudwatch::ops::types::ScheduledQueryConfiguration::default()
                    },
                ),
                ..cloudwatch::ops::types::LogAlarm::default()
            },
            tags: Vec::new(),
        },
    );
}

/// The scheduled query CloudWatch manages for a log alarm exists because that alarm's
/// configuration states it, so a delete naming it is refused as a parameter this operation
/// cannot act on — under either form of the identifier — and the query is still there.
#[tokio::test]
async fn a_delete_of_a_service_managed_query_is_refused() {
    // One alarm naming its managed query's ARN, one leaving the query named by nothing but the
    // alarm: both own a managed query, whichever ARN names it.
    let cases = [
        (
            "an alarm naming its managed query's ARN",
            "beta-log-alarm",
            Some("arn:aws:logs:eu-west-2:000000000000:scheduled-query:beta-log-alarm-managed"),
        ),
        (
            "an alarm whose configuration names no ARN",
            "gamma-log-alarm",
            None,
        ),
    ];

    for (case, alarm, query_arn) in cases {
        let logs = Logs::new();
        seed_log_alarm(&logs, alarm, query_arn);

        let before = logs.ok(GET, &json!({"identifier": alarm})).await;
        let expected = managed_refusal_message(alarm);

        for identifier in [alarm.to_string(), scheduled_query_arn(REGION, alarm)] {
            let (code, message) = logs.refusal(DELETE, &json!({"identifier": identifier})).await;
            assert_eq!(
                code, VALIDATION,
                "case {case:?} under {identifier:?}: expected {VALIDATION}, got {code} {message:?}"
            );
            assert_eq!(
                message, expected,
                "case {case:?} under {identifier:?}: expected the refusal {expected:?}, got \
                 {message:?}"
            );
        }

        let after = logs.ok(GET, &json!({"identifier": alarm})).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused delete leaves the managed query as the alarm states it, \
             expected {before}, got {after}"
        );
        let stored = held(&logs);
        assert!(
            stored.is_empty(),
            "case {case:?}: a refused delete writes nothing to the log store, got {stored:?}"
        );
    }
}
