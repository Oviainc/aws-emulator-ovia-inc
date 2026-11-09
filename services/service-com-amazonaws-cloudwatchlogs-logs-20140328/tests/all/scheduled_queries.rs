// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetScheduledQuery` and the create it is proved through,
//! `CreateScheduledQuery`.
//!
//! The rule under test: a scheduled query is an account-level resource identified by the name
//! it was created under and by the ARN derived from that name, so `GetScheduledQuery` reports,
//! under EITHER form of `ScheduledQueryIdentifier` ("The ARN or name of the scheduled query"),
//! exactly the configuration the create persisted — every member the create named echoed back,
//! every member it did not named by nothing, the state it asked for or the documented `ENABLED`
//! default, the `CUSTOMER_MANAGED` schedule type of a schedule the caller states, and the
//! instant it was created as both its creation and its last-updated instant — while an
//! identifier the account holds no query under is refused with `ResourceNotFoundException` and
//! a create under a name the account already holds is refused as a conflict rather than
//! replacing that query. Reporting is a read, so two reads of one query answer the same thing.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a response
//! that is not computed from the shared store cannot pass.
//!
//! The table spans what the rule generalises over rather than one request: each query language
//! the model enumerates (`CWLI`, `SQL`, `PPL`), the state named explicitly and left to the
//! default, log-group lists of no, one and many identifiers in both the name and the ARN form,
//! each destination the model's `DestinationConfiguration` carries (Amazon S3, a lookup table,
//! both at once), offsets and a schedule window present and absent, and the optional members
//! absent, present one at a time and all together.

use serde_json::{json, Map, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    log_group_arn, scheduled_query_arn, scheduled_query_not_found_message,
    ComAmazonawsCloudwatchlogsLogs20140328State, ScheduledQueryRecord, MAX_SCHEDULED_QUERIES,
};

use crate::harness::{now_ms, Logs, REGION};

/// The operation that creates a scheduled query, and the one that reports it back.
const CREATE: &str = "CreateScheduledQuery";
const GET: &str = "GetScheduledQuery";

/// The code answered when the resource a request names is not there.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The code answered when a create names a scheduled-query name the account already holds
/// ("This operation attempted to create a resource that already exists").
const CONFLICT: &str = "ConflictException";

/// The code answered when a create would grow the account's store past its ceiling.
const QUOTA_EXCEEDED: &str = "ServiceQuotaExceededException";

/// The schedule type of every query a caller creates, since the schedule is the caller's.
const CUSTOMER_MANAGED: &str = "CUSTOMER_MANAGED";

/// The state a create that names none starts a query in, from `CreateScheduledQuery$state`:
/// "Default is ENABLED".
const ENABLED: &str = "ENABLED";

/// The other state the model's `ScheduledQueryState` enumerates.
const DISABLED: &str = "DISABLED";

/// The IAM role every row executes under: "The ARN of the IAM role used to execute the query
/// and deliver results."
const ROLE: &str = "arn:aws:iam::000000000000:role/scheduled-query-execution";

/// Every log group a row names, in either identifier form. A scheduled-query write resolves each
/// `logGroupIdentifiers` entry against the log groups the ACCOUNT holds, so these are created
/// before the queries that read them.
const GROUPS: [&str; 3] = [
    "/aws/lambda/orders",
    "/aws/lambda/payments",
    "/aws/lambda/search",
];

/// Those log groups, created over the wire like any client's call.
async fn seed_log_groups(logs: &Logs) {
    for group in GROUPS {
        logs.ok("CreateLogGroup", &json!({"logGroupName": group})).await;
    }
}

/// The end of a scheduled query's lookback window when the create states none: the offset in
/// seconds a query that names no `endTimeOffset` resolves to, which every reader reports.
const DEFAULT_END_TIME_OFFSET: i64 = 0;

/// The role a destination delivers results under, which the model gives its own member.
const DELIVERY_ROLE: &str = "arn:aws:iam::000000000000:role/scheduled-query-delivery";

/// Where a row delivers its results — the destinations the model's `DestinationConfiguration`
/// carries: `s3Configuration` ("Configuration for delivering query results to Amazon S3"),
/// `lookupTableConfiguration` ("Configuration for delivering query results to a lookup table")
/// or, as `CreateScheduledQuery$destinationConfiguration` allows, both at once ("You can
/// configure one or both destination types").
#[derive(Clone, Copy)]
enum Destination {
    S3,
    LookupTable,
    Both,
}

impl Destination {
    /// This destination as the request states it.
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
            Self::Both => {
                json!({"s3Configuration": s3, "lookupTableConfiguration": lookup_table})
            }
        }
    }
}

/// One accepted create: the members it names, each of which the read must report back exactly
/// when the create named it — and report not at all when it did not.
struct Row {
    /// What this row varies, for the assertion messages.
    case: &'static str,
    name: &'static str,
    /// One value of the model's `QueryLanguage` enum (`CWLI` | `SQL` | `PPL`).
    query_language: &'static str,
    query_string: &'static str,
    schedule_expression: &'static str,
    description: Option<&'static str>,
    /// The log groups the query reads, by name or by ARN.
    log_group_identifiers: Option<&'static [&'static str]>,
    timezone: Option<&'static str>,
    start_time_offset: Option<i64>,
    end_time_offset: Option<i64>,
    destination: Option<Destination>,
    schedule_start_time: Option<i64>,
    schedule_end_time: Option<i64>,
    /// One value of the model's `ScheduledQueryState` enum, or `None` to leave the create to
    /// the documented default.
    state: Option<&'static str>,
    tags: Option<&'static [(&'static str, &'static str)]>,
}

impl Row {
    /// A row naming nothing but the members a create must name.
    const fn bare(
        case: &'static str,
        name: &'static str,
        query_language: &'static str,
        query_string: &'static str,
        schedule_expression: &'static str,
    ) -> Self {
        Self {
            case,
            name,
            query_language,
            query_string,
            schedule_expression,
            description: None,
            log_group_identifiers: None,
            timezone: None,
            start_time_offset: None,
            end_time_offset: None,
            destination: None,
            schedule_start_time: None,
            schedule_end_time: None,
            state: None,
            tags: None,
        }
    }

    /// The state this row's query must be in once created: the one it named, or the documented
    /// default when it named none.
    fn expected_state(&self) -> &'static str {
        self.state.unwrap_or(ENABLED)
    }
}

/// The accepted corpus. The query languages are the three the model's `QueryLanguage` enum
/// lists; the rest of each row differs along what the rule generalises over.
fn rows() -> Vec<Row> {
    vec![
        Row::bare(
            "the required members only",
            "orders-errors-hourly",
            "CWLI",
            "fields @timestamp, @message | filter @message like /ERROR/",
            "cron(0 * * * ? *)",
        ),
        Row {
            log_group_identifiers: Some(&["/aws/lambda/orders"]),
            ..Row::bare(
                "one log group, by name",
                "orders-one-group",
                "CWLI",
                "stats count(*) by bin(5m)",
                "cron(*/5 * * * ? *)",
            )
        },
        Row {
            log_group_identifiers: Some(&[
                "/aws/lambda/orders",
                "/aws/lambda/payments",
                "/aws/lambda/search",
            ]),
            ..Row::bare(
                "many log groups, by name",
                "orders-many-groups",
                "SQL",
                "SELECT count(*) FROM `/aws/lambda/orders`",
                "cron(0 0 * * ? *)",
            )
        },
        Row {
            // A log group may be named "by name or full ARN", so one row reads them by ARN.
            log_group_identifiers: Some(&["arn:aws:logs:eu-west-2:000000000000:log-group:/aws/lambda/orders"]),
            ..Row::bare(
                "a log group by ARN",
                "orders-group-by-arn",
                "PPL",
                "source=/aws/lambda/orders | stats count() by level",
                "cron(0 6 * * ? *)",
            )
        },
        Row {
            destination: Some(Destination::S3),
            ..Row::bare(
                "an Amazon S3 destination",
                "orders-to-s3",
                "CWLI",
                "fields @message",
                "cron(0 1 * * ? *)",
            )
        },
        Row {
            destination: Some(Destination::LookupTable),
            ..Row::bare(
                "a lookup table destination",
                "orders-to-lookup-table",
                "CWLI",
                "stats count(*) by level",
                "cron(0 2 * * ? *)",
            )
        },
        Row {
            destination: Some(Destination::Both),
            ..Row::bare(
                "both destinations at once",
                "orders-to-both",
                "SQL",
                "SELECT level, count(*) FROM `/aws/lambda/orders` GROUP BY level",
                "cron(0 3 * * ? *)",
            )
        },
        Row {
            start_time_offset: Some(3_600),
            end_time_offset: Some(0),
            ..Row::bare(
                "a lookback window stated by both offsets",
                "orders-offsets",
                "CWLI",
                "fields @timestamp",
                "cron(15 * * * ? *)",
            )
        },
        Row {
            schedule_start_time: Some(1_767_225_600_000),
            schedule_end_time: Some(1_798_761_600_000),
            ..Row::bare(
                "a schedule window",
                "orders-schedule-window",
                "CWLI",
                "fields @message",
                "cron(30 4 * * ? *)",
            )
        },
        Row {
            state: Some(DISABLED),
            ..Row::bare(
                "the state named explicitly",
                "orders-disabled",
                "CWLI",
                "fields @message",
                "cron(0 5 * * ? *)",
            )
        },
        Row {
            state: Some(ENABLED),
            ..Row::bare(
                "the default state named explicitly",
                "orders-enabled",
                "CWLI",
                "fields @message",
                "cron(0 7 * * ? *)",
            )
        },
        Row {
            timezone: Some("Europe/London"),
            ..Row::bare(
                "a timezone",
                "orders-timezone",
                "CWLI",
                "fields @message",
                "cron(0 8 * * ? *)",
            )
        },
        Row {
            description: Some("hourly error roll-up for the orders service"),
            ..Row::bare(
                "a description",
                "orders-described",
                "CWLI",
                "fields @message",
                "cron(0 9 * * ? *)",
            )
        },
        Row {
            tags: Some(&[("team", "platform"), ("env", "test")]),
            ..Row::bare(
                "tags",
                "orders-tagged",
                "CWLI",
                "fields @message",
                "cron(0 10 * * ? *)",
            )
        },
        Row {
            description: Some("every optional member at once"),
            log_group_identifiers: Some(&["/aws/lambda/orders", "/aws/lambda/payments"]),
            timezone: Some("America/New_York"),
            start_time_offset: Some(86_400),
            end_time_offset: Some(60),
            destination: Some(Destination::Both),
            schedule_start_time: Some(1_767_225_600_000),
            schedule_end_time: Some(1_798_761_600_000),
            state: Some(DISABLED),
            tags: Some(&[("team", "platform")]),
            ..Row::bare(
                "every optional member at once",
                "orders-everything",
                "PPL",
                "source=/aws/lambda/orders | head 10",
                "cron(0 11 * * ? *)",
            )
        },
        Row {
            // `queryString` is `@length(min: 0)`, so the empty string is a value a create may
            // name and the read must report back as it stands.
            ..Row::bare(
                "an empty query string, which the modelled length admits",
                "orders-empty-query",
                "CWLI",
                "",
                "cron(0 12 * * ? *)",
            )
        },
    ]
}

/// The request body one row sends.
fn create_request(row: &Row) -> Value {
    let mut request = Map::new();
    request.insert("name".to_string(), json!(row.name));
    request.insert("queryLanguage".to_string(), json!(row.query_language));
    request.insert("queryString".to_string(), json!(row.query_string));
    request.insert(
        "scheduleExpression".to_string(),
        json!(row.schedule_expression),
    );
    request.insert("executionRoleArn".to_string(), json!(ROLE));
    if let Some(description) = row.description {
        request.insert("description".to_string(), json!(description));
    }
    if let Some(identifiers) = row.log_group_identifiers {
        request.insert("logGroupIdentifiers".to_string(), json!(identifiers));
    }
    if let Some(timezone) = row.timezone {
        request.insert("timezone".to_string(), json!(timezone));
    }
    if let Some(offset) = row.start_time_offset {
        request.insert("startTimeOffset".to_string(), json!(offset));
    }
    if let Some(offset) = row.end_time_offset {
        request.insert("endTimeOffset".to_string(), json!(offset));
    }
    if let Some(destination) = row.destination {
        request.insert("destinationConfiguration".to_string(), destination.request());
    }
    if let Some(start) = row.schedule_start_time {
        request.insert("scheduleStartTime".to_string(), json!(start));
    }
    if let Some(end) = row.schedule_end_time {
        request.insert("scheduleEndTime".to_string(), json!(end));
    }
    if let Some(state) = row.state {
        request.insert("state".to_string(), json!(state));
    }
    if let Some(tags) = row.tags {
        let mut map = Map::new();
        for (key, value) in tags {
            map.insert((*key).to_string(), json!(value));
        }
        request.insert("tags".to_string(), Value::Object(map));
    }
    Value::Object(request)
}

/// One string member of a response, or `None` when the response omits it.
fn member(body: &Value, key: &str) -> Option<String> {
    body.get(key).and_then(Value::as_str).map(str::to_string)
}

/// One integer member of a response, or `None` when the response omits it.
fn number(body: &Value, key: &str) -> Option<i64> {
    body.get(key).and_then(Value::as_i64)
}

/// Every member of a read, asserted against the row the create was built from.
fn assert_reports_row(read: &Value, row: &Row, arn: &str, created_at: i64, under: &str) {
    let case = row.case;
    assert_eq!(
        member(read, "scheduledQueryArn").as_deref(),
        Some(arn),
        "case {case:?} read by {under}: the read reports the ARN the create minted, got {read}"
    );
    assert_eq!(
        member(read, "name").as_deref(),
        Some(row.name),
        "case {case:?} read by {under}: the read reports the name the create was given, got \
         {read}"
    );
    assert_eq!(
        member(read, "queryLanguage").as_deref(),
        Some(row.query_language),
        "case {case:?} read by {under}: the read reports the query language the create named, \
         got {read}"
    );
    assert_eq!(
        member(read, "queryString").as_deref(),
        Some(row.query_string),
        "case {case:?} read by {under}: the read reports the query string verbatim, got {read}"
    );
    assert_eq!(
        member(read, "scheduleExpression").as_deref(),
        Some(row.schedule_expression),
        "case {case:?} read by {under}: the read reports the schedule expression the create \
         named, got {read}"
    );
    assert_eq!(
        member(read, "executionRoleArn").as_deref(),
        Some(ROLE),
        "case {case:?} read by {under}: the read reports the execution role the create named, \
         got {read}"
    );
    assert_eq!(
        member(read, "description").as_deref(),
        row.description,
        "case {case:?} read by {under}: the description is reported only when the create named \
         one, got {read}"
    );
    assert_eq!(
        member(read, "timezone").as_deref(),
        row.timezone,
        "case {case:?} read by {under}: the timezone is reported only when the create named \
         one, got {read}"
    );
    assert_eq!(
        number(read, "startTimeOffset"),
        row.start_time_offset,
        "case {case:?} read by {under}: the start offset is reported only when the create named \
         one, got {read}"
    );
    // `endTimeOffset` has an effective value whether or not the create named one, so a row
    // naming none reports the offset it resolves to rather than no offset.
    let expected_end_offset = row.end_time_offset.unwrap_or(DEFAULT_END_TIME_OFFSET);
    assert_eq!(
        number(read, "endTimeOffset"),
        Some(expected_end_offset),
        "case {case:?} read by {under}: the end offset is the create's, or \
         {expected_end_offset} where it named none, got {read}"
    );
    assert_eq!(
        number(read, "scheduleStartTime"),
        row.schedule_start_time,
        "case {case:?} read by {under}: the schedule start is reported only when the create \
         named one, got {read}"
    );
    assert_eq!(
        number(read, "scheduleEndTime"),
        row.schedule_end_time,
        "case {case:?} read by {under}: the schedule end is reported only when the create named \
         one, got {read}"
    );

    // `logGroupIdentifiers` is `@length(min: 1)`, so a query naming no log group reports no
    // member at all rather than an empty list.
    let expected_groups = row.log_group_identifiers.map(|identifiers| json!(identifiers));
    assert_eq!(
        read.get("logGroupIdentifiers").cloned(),
        expected_groups,
        "case {case:?} read by {under}: the log groups are reported as the create named them, \
         expected {expected_groups:?}, got {read}"
    );

    let expected_destination = row.destination.map(Destination::request);
    assert_eq!(
        read.get("destinationConfiguration").cloned(),
        expected_destination,
        "case {case:?} read by {under}: the destination configuration round-trips, expected \
         {expected_destination:?}, got {read}"
    );

    assert_eq!(
        member(read, "state").as_deref(),
        Some(row.expected_state()),
        "case {case:?} read by {under}: expected state {}, got {read}",
        row.expected_state()
    );
    assert_eq!(
        member(read, "scheduleType").as_deref(),
        Some(CUSTOMER_MANAGED),
        "case {case:?} read by {under}: a schedule the caller states is {CUSTOMER_MANAGED}, got \
         {read}"
    );
    assert_eq!(
        number(read, "creationTime"),
        Some(created_at),
        "case {case:?} read by {under}: the creation instant is the one the create recorded \
         ({created_at}), got {read}"
    );
    assert_eq!(
        number(read, "lastUpdatedTime"),
        Some(created_at),
        "case {case:?} read by {under}: a query nothing has updated was last updated when it \
         was created ({created_at}), got {read}"
    );

    // Both members report an execution that happened; nothing has executed this query, so
    // neither is reported rather than invented.
    assert_eq!(
        read.get("lastTriggeredTime"),
        None,
        "case {case:?} read by {under}: a query no execution was recorded for reports no last \
         triggered time, got {read}"
    );
    assert_eq!(
        read.get("lastExecutionStatus"),
        None,
        "case {case:?} read by {under}: a query no execution was recorded for reports no last \
         execution status, got {read}"
    );
}

/// Every accepted row: the create persists the query and answers the ARN that identifies it
/// plus the state it is in, and the read reports back — under the name and under the ARN
/// alike — exactly what the create stored.
#[tokio::test]
async fn a_created_scheduled_query_is_reported_back_exactly() {
    let logs = Logs::new();
    seed_log_groups(&logs).await;

    for row in rows() {
        let before = now_ms();
        let created = logs.ok(CREATE, &create_request(&row)).await;
        let after = now_ms();

        let expected_arn = scheduled_query_arn(REGION, row.name);
        let arn = member(&created, "scheduledQueryArn").unwrap_or_else(|| {
            panic!("case {:?}: a create answers scheduledQueryArn, got {created}", row.case)
        });
        assert_eq!(
            arn, expected_arn,
            "case {:?}: the create answers the ARN built for the name it was given, expected \
             {expected_arn}, got {arn}",
            row.case
        );
        assert_eq!(
            member(&created, "state").as_deref(),
            Some(row.expected_state()),
            "case {:?}: the create answers the state the query is in, expected {}, got {created}",
            row.case,
            row.expected_state()
        );

        // The creation instant is the instant of the call, which the read then reports.
        let by_name = logs.ok(GET, &json!({"identifier": row.name})).await;
        let created_at = number(&by_name, "creationTime").unwrap_or_else(|| {
            panic!("case {:?}: the read reports creationTime, got {by_name}", row.case)
        });
        assert!(
            (before..=after).contains(&created_at),
            "case {:?}: creationTime is the instant of the create, expected it in \
             {before}..={after}, got {created_at}",
            row.case
        );

        assert_reports_row(&by_name, &row, &arn, created_at, "name");

        // "The ARN or name of the scheduled query": both identify the same stored query, so
        // both reads answer the same thing.
        let by_arn = logs.ok(GET, &json!({"identifier": arn})).await;
        assert_reports_row(&by_arn, &row, &arn, created_at, "ARN");
        assert_eq!(
            by_arn, by_name,
            "case {:?}: the name and the ARN identify one query, so the two reads agree, got \
             {by_arn} and {by_name}",
            row.case
        );

        // Reporting is a read: a second read answers what the first did.
        let again = logs.ok(GET, &json!({"identifier": row.name})).await;
        assert_eq!(
            again, by_name,
            "case {:?}: a read changes nothing, so reading twice answers the same, got {again} \
             and {by_name}",
            row.case
        );
    }
}

/// The ARN segments are pinned LITERALLY here, once, so the assertions above — which build the
/// expected ARN with the same helper the handler uses — cannot pass on an ARN of the wrong
/// SHAPE. The form is the one every named CloudWatch Logs resource of this service carries:
/// `arn:aws:logs:<region>:<account>:<resource-type>:<name>`, with the region taken from the
/// request's credential scope.
#[tokio::test]
async fn the_created_arn_carries_the_documented_segments() {
    let logs = Logs::new();
    const NAME: &str = "orders-arn-form";

    let created = logs
        .ok(
            CREATE,
            &json!({
                "name": NAME,
                "queryLanguage": "CWLI",
                "queryString": "fields @message",
                "scheduleExpression": "cron(0 * * * ? *)",
                "executionRoleArn": ROLE,
            }),
        )
        .await;
    let arn = member(&created, "scheduledQueryArn")
        .unwrap_or_else(|| panic!("a create answers scheduledQueryArn, got {created}"));
    let segments: Vec<&str> = arn.splitn(6, ':').collect();
    assert_eq!(
        segments.len(),
        6,
        "case \"the ARN's segments\": an ARN has six segments, got {arn}"
    );
    assert_eq!(
        (segments[0], segments[1], segments[2], segments[3]),
        ("arn", "aws", "logs", REGION),
        "case \"the ARN's segments\": expected the logs ARN prefix in {REGION}, got {arn}"
    );
    assert_eq!(
        (segments[4].len(), segments[5]),
        (12, format!("scheduled-query:{NAME}").as_str()),
        "case \"the ARN's segments\": expected a 12-digit account and the scheduled-query \
         resource segment, got {arn}"
    );
}

/// The ARN a create mints names the Region the request was signed for, and a read of that ARN
/// reports the query whatever Region the read is signed for — the ARN identifies one query in
/// one account.
#[tokio::test]
async fn the_arn_carries_the_requesting_region() {
    let logs = Logs::new();
    const OTHER_REGION: &str = "ap-south-1";
    const NAME: &str = "orders-other-region";

    let created = logs
        .ok_in_region(
            OTHER_REGION,
            CREATE,
            &json!({
                "name": NAME,
                "queryLanguage": "CWLI",
                "queryString": "fields @message",
                "scheduleExpression": "cron(0 * * * ? *)",
                "executionRoleArn": ROLE,
            }),
        )
        .await;
    let arn = member(&created, "scheduledQueryArn")
        .unwrap_or_else(|| panic!("a create answers scheduledQueryArn, got {created}"));
    let expected = scheduled_query_arn(OTHER_REGION, NAME);
    assert_eq!(
        arn, expected,
        "case {OTHER_REGION:?}: the ARN names the Region the create was signed for, expected \
         {expected}, got {arn}"
    );

    let read = logs.ok(GET, &json!({"identifier": arn})).await;
    assert_eq!(
        member(&read, "name").as_deref(),
        Some(NAME),
        "case {OTHER_REGION:?}: the ARN identifies the query it was minted for, got {read}"
    );

    // An ARN names the query its resource segment does, so one account's query is reported
    // under the ARN form of its name whatever Region that ARN carries — the same way a
    // `logGroupIdentifier` names a log group.
    let cross_region = scheduled_query_arn(REGION, NAME);
    let read = logs.ok(GET, &json!({"identifier": cross_region})).await;
    assert_eq!(
        member(&read, "name").as_deref(),
        Some(NAME),
        "case {REGION:?}: an ARN names the query its resource segment does, got {read}"
    );
}

/// An identifier the account holds no scheduled query under is refused as a missing resource,
/// in the sentence this service refuses one with — whichever form of identifier it is.
#[tokio::test]
async fn a_read_of_an_unknown_identifier_is_refused() {
    let logs = Logs::new();
    const STORED: &str = "orders-stored";

    logs.ok(
        CREATE,
        &json!({
            "name": STORED,
            "queryLanguage": "CWLI",
            "queryString": "fields @message",
            "scheduleExpression": "cron(0 * * * ? *)",
            "executionRoleArn": ROLE,
        }),
    )
    .await;

    // A name no query is stored under, a name differing only in case, the ARN form of such a
    // name, the ARN of another resource kind entirely, and a value that is neither: none of
    // them identifies a stored scheduled query.
    let cases = [
        "orders-missing",
        "ORDERS-STORED",
        scheduled_query_arn(REGION, "orders-missing").as_str(),
        log_group_arn(REGION, "/aws/lambda/orders", false).as_str(),
        "not-an-arn-and-not-a-name",
    ]
    .map(str::to_string);

    for identifier in cases {
        let (code, message) = logs.refusal(GET, &json!({"identifier": identifier})).await;
        assert_eq!(
            code, NOT_FOUND,
            "case {identifier:?}: expected {NOT_FOUND}, got {code} {message:?}"
        );
        let expected = scheduled_query_not_found_message(&identifier);
        assert_eq!(
            message, expected,
            "case {identifier:?}: expected the refusal {expected:?}, got {message:?}"
        );
    }
}

/// "The name must be unique within your account and region": a second create under a stored
/// name is refused as a conflict, and the query the account holds is left exactly as it was.
#[tokio::test]
async fn a_create_under_a_stored_name_is_refused_and_changes_nothing() {
    let logs = Logs::new();
    const NAME: &str = "orders-unique";
    let first = json!({
        "name": NAME,
        "queryLanguage": "CWLI",
        "queryString": "fields @message",
        "scheduleExpression": "cron(0 * * * ? *)",
        "executionRoleArn": ROLE,
        "description": "the first create",
    });

    logs.ok(CREATE, &first).await;
    let before = logs.ok(GET, &json!({"identifier": NAME})).await;

    let (code, message) = logs
        .refusal(
            CREATE,
            &json!({
                "name": NAME,
                "queryLanguage": "SQL",
                "queryString": "SELECT 1",
                "scheduleExpression": "cron(0 1 * * ? *)",
                "executionRoleArn": ROLE,
                "description": "the second create",
            }),
        )
        .await;
    assert_eq!(
        code, CONFLICT,
        "case \"a second create under a stored name\": expected {CONFLICT}, got {code} \
         {message:?}"
    );
    assert!(
        message.contains(NAME),
        "case \"a second create under a stored name\": the refusal names the query \
         ({NAME}), got {message:?}"
    );

    let after = logs.ok(GET, &json!({"identifier": NAME})).await;
    assert_eq!(
        after, before,
        "case \"a second create under a stored name\": the refused create replaces nothing, \
         expected {before}, got {after}"
    );
}

/// A create that would grow the account's scheduled-query store past the emulator's own
/// ceiling is refused with the `ServiceQuotaExceededException` the operation models, and
/// stores nothing.
#[tokio::test]
async fn a_create_past_the_account_ceiling_is_refused() {
    let logs = Logs::new();

    // The store filled to exactly its ceiling, through the same record constructor the create
    // uses: the rule is about the COUNT the store holds, so how each query got there does not
    // matter.
    {
        let state = logs
            .engine()
            .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
        let mut store = state.lock();
        for index in 0..MAX_SCHEDULED_QUERIES {
            let name = format!("ceiling-{index}");
            let record = ScheduledQueryRecord::new(
                &name,
                &scheduled_query_arn(REGION, &name),
                "CWLI",
                "fields @message",
                "cron(0 * * * ? *)",
                ROLE,
            );
            store.scheduled_queries.insert(name, record);
        }
    }

    let (code, message) = logs
        .refusal(
            CREATE,
            &json!({
                "name": "one-past-the-ceiling",
                "queryLanguage": "CWLI",
                "queryString": "fields @message",
                "scheduleExpression": "cron(0 * * * ? *)",
                "executionRoleArn": ROLE,
            }),
        )
        .await;
    assert_eq!(
        code, QUOTA_EXCEEDED,
        "case \"one create past the ceiling\": expected {QUOTA_EXCEEDED}, got {code} {message:?}"
    );
    assert!(
        message.contains(&MAX_SCHEDULED_QUERIES.to_string()),
        "case \"one create past the ceiling\": the refusal states the ceiling \
         ({MAX_SCHEDULED_QUERIES}), got {message:?}"
    );

    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let stored = state.lock().scheduled_queries.len();
    assert_eq!(
        stored, MAX_SCHEDULED_QUERIES,
        "a refused create persists nothing, expected {MAX_SCHEDULED_QUERIES} queries, got \
         {stored}"
    );
}

/// The tags a create assigns are stored on the query it creates, which is where the tagging
/// operations read them from once those reach this resource. No member of a scheduled query's
/// response reports them, so they are read off the record the create stored.
#[tokio::test]
async fn a_create_stores_the_tags_it_was_given() {
    let logs = Logs::new();
    const NAME: &str = "orders-tag-store";

    logs.ok(
        CREATE,
        &json!({
            "name": NAME,
            "queryLanguage": "CWLI",
            "queryString": "fields @message",
            "scheduleExpression": "cron(0 * * * ? *)",
            "executionRoleArn": ROLE,
            "tags": {"team": "platform", "env": "test"},
        }),
    )
    .await;

    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let store = state.lock();
    let stored = store
        .scheduled_query(NAME)
        .unwrap_or_else(|| panic!("case {NAME:?}: the create persists the query"));
    let got: Vec<(String, String)> = stored
        .tags
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let expected = vec![
        ("env".to_string(), "test".to_string()),
        ("team".to_string(), "platform".to_string()),
    ];
    assert_eq!(
        got, expected,
        "case {NAME:?}: the create stores the tags it was given, expected {expected:?}, got \
         {got:?}"
    );
}
