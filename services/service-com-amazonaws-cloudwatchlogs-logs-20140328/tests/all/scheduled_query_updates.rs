// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateScheduledQuery`, the operation that writes the configuration of
//! a scheduled query the account already holds.
//!
//! The rule under test: the operation "uses PUT semantics", so the stored configuration becomes
//! EXACTLY what the request states — every member it names takes the value it names, every
//! optional member it omits goes back to naming nothing, and the state is the one it names or
//! the documented `ENABLED` default — while what a request cannot state is carried over: the
//! query's name, the ARN derived from that name, the instant it was created, its
//! `CUSTOMER_MANAGED` schedule type, its tags and its execution history. The instant of the call
//! becomes the query's last-updated instant, the write is addressed by either form of
//! `ScheduledQueryIdentifier` ("The ARN or name of the scheduled query to update"), it touches
//! no other query, an identifier the account holds no query under is refused as a missing
//! resource, and the query CloudWatch manages for a log alarm — whose configuration lives on
//! that alarm — is refused rather than written.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a response
//! that is not computed from the shared store cannot pass, and every write is read back through
//! `GetScheduledQuery`.
//!
//! The table spans what the rule generalises over rather than one request: it starts from a
//! query created with EVERY optional member named, so each row that omits one must clear it;
//! the rows differ in query language (each value the model's `QueryLanguage` enumerates), in the
//! state named explicitly, named as the default and left unnamed, in log-group lists of one and
//! many identifiers in both the name and the ARN form, in each destination the model's
//! `DestinationConfiguration` carries (Amazon S3, a lookup table, both at once), in offsets and
//! a schedule window present and absent, in the execution role and the schedule expression, in
//! an empty query string, and in whether the query is addressed by its name or by its ARN.

use serde_json::{json, Map, Value};

use service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;
use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    log_group_arn, scheduled_query_arn, ComAmazonawsCloudwatchlogsLogs20140328State,
};

use crate::harness::{now_ms, Logs, REGION};

/// The operation under test, the create that puts a query in the store, and the read every
/// write is proved through.
const UPDATE: &str = "UpdateScheduledQuery";
const CREATE: &str = "CreateScheduledQuery";
const GET: &str = "GetScheduledQuery";

/// The code answered when the resource a request names is not there.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The code answered for a parameter this operation will not accept ("One of the parameters for
/// the request is not valid").
const VALIDATION: &str = "ValidationException";

/// The refusal sentence of each class, pinned LITERALLY here rather than built with the helper
/// the handler itself calls, so a wording that is wrong but consistent with the implementation
/// cannot pass. The missing-query refusal names the identifier the request asked for; the
/// managed-query refusal names the query the request asked for.
fn not_found_message(identifier: &str) -> String {
    format!("Scheduled query with identifier {identifier} not found")
}

fn managed_refusal_message(name: &str) -> String {
    format!("The scheduled query {name} is managed by CloudWatch and cannot be updated.")
}

/// The two values the model's `ScheduledQueryState` enumerates. `ENABLED` is also the state a
/// scheduled query naming none is in ("Default is ENABLED"), which under PUT semantics is the
/// state an update naming none leaves the query in.
const ENABLED: &str = "ENABLED";
const DISABLED: &str = "DISABLED";

/// The schedule type of every query a caller creates, which no update member states and which
/// therefore stands across one.
const CUSTOMER_MANAGED: &str = "CUSTOMER_MANAGED";

/// The IAM role the created query executes under, and the other one an update may move it to.
const ROLE: &str = "arn:aws:iam::000000000000:role/scheduled-query-execution";
const OTHER_ROLE: &str = "arn:aws:iam::000000000000:role/scheduled-query-execution-v2";

/// The role a destination delivers results under, which the model gives its own member.
const DELIVERY_ROLE: &str = "arn:aws:iam::000000000000:role/scheduled-query-delivery";

/// The name the query under test is created under in every case.
const NAME: &str = "orders-errors-hourly";

/// Every log group the create and the rows name, in either identifier form. A scheduled-query
/// write resolves each `logGroupIdentifiers` entry against the log groups the ACCOUNT holds, so
/// each case creates these before the query that reads them.
const GROUPS: [&str; 4] = [
    "/aws/lambda/orders",
    "/aws/lambda/payments",
    "/aws/lambda/cart",
    "/aws/lambda/search",
];

/// Those log groups, created over the wire like any client's call.
async fn seed_log_groups(logs: &Logs) {
    for group in GROUPS {
        logs.ok("CreateLogGroup", &json!({"logGroupName": group})).await;
    }
}

/// The end of a scheduled query's lookback window when the write states none: the offset in
/// seconds a query that states no `endTimeOffset` resolves to, which every reader of the query
/// reports.
const DEFAULT_END_TIME_OFFSET: i64 = 0;

/// Where a row delivers its results — the destinations the model's `DestinationConfiguration`
/// carries: `s3Configuration` ("Configuration for delivering query results to Amazon S3"),
/// `lookupTableConfiguration` ("Configuration for delivering query results to a lookup table")
/// or both at once ("You can configure one or both destination types").
#[derive(Clone, Copy)]
enum Destination {
    S3,
    LookupTable,
    Both,
}

impl Destination {
    /// This destination as a request states it, and as a read reports it back.
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

/// One update: the members it states, which are exactly the members the query must carry
/// afterwards — the optional ones it leaves `None` must be reported by nothing once it is
/// applied, whatever the create put there.
struct Update {
    /// What this row varies, for the assertion messages.
    case: &'static str,
    /// One value of the model's `QueryLanguage` enum (`CWLI` | `SQL` | `PPL`).
    query_language: &'static str,
    query_string: &'static str,
    schedule_expression: &'static str,
    execution_role_arn: &'static str,
    description: Option<&'static str>,
    /// The log groups the query reads, by name or by ARN.
    log_group_identifiers: Option<&'static [&'static str]>,
    timezone: Option<&'static str>,
    start_time_offset: Option<i64>,
    end_time_offset: Option<i64>,
    destination: Option<Destination>,
    schedule_start_time: Option<i64>,
    schedule_end_time: Option<i64>,
    /// One value of the model's `ScheduledQueryState` enum, or `None` to state none.
    state: Option<&'static str>,
    /// Whether this row addresses the query by the ARN form of its identifier rather than by
    /// its name; both forms name the one query.
    by_arn: bool,
}

impl Update {
    /// A row stating nothing but the members an update must state, so applying it must clear
    /// every optional member the create had named.
    const fn bare(
        case: &'static str,
        query_language: &'static str,
        query_string: &'static str,
        schedule_expression: &'static str,
    ) -> Self {
        Self {
            case,
            query_language,
            query_string,
            schedule_expression,
            execution_role_arn: ROLE,
            description: None,
            log_group_identifiers: None,
            timezone: None,
            start_time_offset: None,
            end_time_offset: None,
            destination: None,
            schedule_start_time: None,
            schedule_end_time: None,
            state: None,
            by_arn: false,
        }
    }

    /// The state the query is in once this row is applied: the one the row states, or the
    /// documented default when it states none.
    fn expected_state(&self) -> &'static str {
        self.state.unwrap_or(ENABLED)
    }
}

/// The corpus of updates. Each row differs along what the rule generalises over; every one of
/// them is applied to the SAME fully populated created query, so a row omitting a member proves
/// the omission clears it.
fn rows() -> Vec<Update> {
    vec![
        Update::bare(
            "the required members only, clearing every optional member",
            "CWLI",
            "fields @timestamp, @message",
            "cron(0 * * * ? *)",
        ),
        Update {
            description: Some("the description an update stated"),
            ..Update::bare("a description", "CWLI", "fields @message", "cron(5 * * * ? *)")
        },
        Update {
            log_group_identifiers: Some(&["/aws/lambda/orders"]),
            ..Update::bare(
                "one log group, by name",
                "CWLI",
                "stats count(*) by bin(5m)",
                "cron(*/5 * * * ? *)",
            )
        },
        Update {
            log_group_identifiers: Some(&[
                "/aws/lambda/orders",
                "/aws/lambda/payments",
                "/aws/lambda/search",
            ]),
            ..Update::bare(
                "many log groups, by name",
                "SQL",
                "SELECT count(*) FROM `/aws/lambda/orders`",
                "cron(0 0 * * ? *)",
            )
        },
        Update {
            // A log group may be named "by name or full ARN", so one row states them by ARN.
            log_group_identifiers: Some(&[
                "arn:aws:logs:eu-west-2:000000000000:log-group:/aws/lambda/orders",
            ]),
            ..Update::bare(
                "a log group by ARN",
                "PPL",
                "source=/aws/lambda/orders | stats count() by level",
                "cron(0 6 * * ? *)",
            )
        },
        Update {
            destination: Some(Destination::S3),
            ..Update::bare(
                "an Amazon S3 destination",
                "CWLI",
                "fields @message",
                "cron(0 1 * * ? *)",
            )
        },
        Update {
            destination: Some(Destination::LookupTable),
            ..Update::bare(
                "a lookup table destination",
                "CWLI",
                "stats count(*) by level",
                "cron(0 2 * * ? *)",
            )
        },
        Update {
            destination: Some(Destination::Both),
            ..Update::bare(
                "both destinations at once",
                "SQL",
                "SELECT level, count(*) FROM `/aws/lambda/orders` GROUP BY level",
                "cron(0 3 * * ? *)",
            )
        },
        Update {
            start_time_offset: Some(7_200),
            end_time_offset: Some(0),
            ..Update::bare(
                "a lookback window stated by both offsets",
                "CWLI",
                "fields @timestamp",
                "cron(15 * * * ? *)",
            )
        },
        Update {
            schedule_start_time: Some(1_767_312_000_000),
            schedule_end_time: Some(1_798_848_000_000),
            ..Update::bare(
                "a schedule window",
                "CWLI",
                "fields @message",
                "cron(30 4 * * ? *)",
            )
        },
        Update {
            state: Some(DISABLED),
            ..Update::bare(
                "the state stated as DISABLED",
                "CWLI",
                "fields @message",
                "cron(0 5 * * ? *)",
            )
        },
        Update {
            state: Some(ENABLED),
            ..Update::bare(
                "the state stated as the default",
                "CWLI",
                "fields @message",
                "cron(0 7 * * ? *)",
            )
        },
        Update {
            timezone: Some("America/New_York"),
            ..Update::bare("a timezone", "CWLI", "fields @message", "cron(0 8 * * ? *)")
        },
        Update {
            execution_role_arn: OTHER_ROLE,
            ..Update::bare(
                "another execution role",
                "CWLI",
                "fields @message",
                "cron(0 9 * * ? *)",
            )
        },
        Update {
            // `queryString` is `@length(min: 0)`, so the empty string is a value an update may
            // state and the read must report back as it stands.
            ..Update::bare(
                "an empty query string, which the modelled length admits",
                "CWLI",
                "",
                "cron(0 10 * * ? *)",
            )
        },
        Update {
            description: Some("every optional member at once"),
            log_group_identifiers: Some(&["/aws/lambda/orders", "/aws/lambda/payments"]),
            timezone: Some("Asia/Tokyo"),
            start_time_offset: Some(86_400),
            end_time_offset: Some(120),
            destination: Some(Destination::Both),
            schedule_start_time: Some(1_767_312_000_000),
            schedule_end_time: Some(1_798_848_000_000),
            state: Some(DISABLED),
            by_arn: true,
            ..Update::bare(
                "every optional member at once, addressed by ARN",
                "PPL",
                "source=/aws/lambda/orders | head 10",
                "cron(0 11 * * ? *)",
            )
        },
        Update {
            by_arn: true,
            ..Update::bare(
                "the required members only, addressed by ARN",
                "SQL",
                "SELECT 1",
                "cron(0 12 * * ? *)",
            )
        },
    ]
}

/// The create every case starts from: a query carrying EVERY optional member, in the state that
/// is NOT the default, so each row's omissions have something to clear and the state the row
/// states (or omits) is observable.
fn create_request(name: &str) -> Value {
    json!({
        "name": name,
        "queryLanguage": "CWLI",
        "queryString": "fields @timestamp, @message | filter @message like /ERROR/",
        "scheduleExpression": "cron(0 * * * ? *)",
        "executionRoleArn": ROLE,
        "description": "the configuration the create stored",
        "logGroupIdentifiers": ["/aws/lambda/orders", "/aws/lambda/payments", "/aws/lambda/cart"],
        "timezone": "Europe/London",
        "startTimeOffset": 3_600,
        "endTimeOffset": 60,
        "destinationConfiguration": Destination::S3.request(),
        "scheduleStartTime": 1_767_225_600_000_i64,
        "scheduleEndTime": 1_798_761_600_000_i64,
        "state": DISABLED,
        "tags": {"team": "platform", "env": "test"},
    })
}

/// The `UpdateScheduledQuery` request one row sends, addressed by the identifier form it names.
fn update_request(row: &Update, name: &str, arn: &str) -> Value {
    let mut request = Map::new();
    let identifier = if row.by_arn { arn } else { name };
    request.insert("identifier".to_string(), json!(identifier));
    request.insert("queryLanguage".to_string(), json!(row.query_language));
    request.insert("queryString".to_string(), json!(row.query_string));
    request.insert(
        "scheduleExpression".to_string(),
        json!(row.schedule_expression),
    );
    request.insert(
        "executionRoleArn".to_string(),
        json!(row.execution_role_arn),
    );
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

/// Every member of a report of the query, asserted against the row that was applied to it: the
/// members the row stated, the members it did not (reported by nothing), and the members no
/// update member states, which stand as the create left them.
fn assert_reports_update(
    report: &Value,
    row: &Update,
    arn: &str,
    created_at: i64,
    window: (i64, i64),
    under: &str,
) {
    let case = row.case;
    let (before, after) = window;

    assert_eq!(
        member(report, "scheduledQueryArn").as_deref(),
        Some(arn),
        "case {case:?} in {under}: the ARN is derived from the name, which an update does not \
         change, got {report}"
    );
    assert_eq!(
        member(report, "name").as_deref(),
        Some(NAME),
        "case {case:?} in {under}: an update identifies the query, it does not rename it, got \
         {report}"
    );
    assert_eq!(
        member(report, "scheduleType").as_deref(),
        Some(CUSTOMER_MANAGED),
        "case {case:?} in {under}: no update member states the schedule type, so it stands, got \
         {report}"
    );
    assert_eq!(
        number(report, "creationTime"),
        Some(created_at),
        "case {case:?} in {under}: the creation instant is the one the create recorded \
         ({created_at}), got {report}"
    );
    let last_updated = number(report, "lastUpdatedTime");
    assert!(
        last_updated.is_some_and(|stamp| (before..=after).contains(&stamp)),
        "case {case:?} in {under}: the last-updated instant is the instant of the update, \
         expected it in {before}..={after}, got {report}"
    );

    assert_eq!(
        member(report, "queryLanguage").as_deref(),
        Some(row.query_language),
        "case {case:?} in {under}: the query language is the one the update stated, got {report}"
    );
    assert_eq!(
        member(report, "queryString").as_deref(),
        Some(row.query_string),
        "case {case:?} in {under}: the query string is the one the update stated, verbatim, got \
         {report}"
    );
    assert_eq!(
        member(report, "scheduleExpression").as_deref(),
        Some(row.schedule_expression),
        "case {case:?} in {under}: the schedule expression is the one the update stated, got \
         {report}"
    );
    assert_eq!(
        member(report, "executionRoleArn").as_deref(),
        Some(row.execution_role_arn),
        "case {case:?} in {under}: the execution role is the one the update stated, got {report}"
    );
    assert_eq!(
        member(report, "state").as_deref(),
        Some(row.expected_state()),
        "case {case:?} in {under}: expected state {}, got {report}",
        row.expected_state()
    );

    // Each optional member: the value the update stated, and NOTHING where it stated none —
    // whatever the create had put there.
    assert_eq!(
        member(report, "description").as_deref(),
        row.description,
        "case {case:?} in {under}: the description is the update's, cleared where it stated \
         none, got {report}"
    );
    assert_eq!(
        member(report, "timezone").as_deref(),
        row.timezone,
        "case {case:?} in {under}: the timezone is the update's, cleared where it stated none, \
         got {report}"
    );
    assert_eq!(
        number(report, "startTimeOffset"),
        row.start_time_offset,
        "case {case:?} in {under}: the start offset is the update's, cleared where it stated \
         none, got {report}"
    );
    // `endTimeOffset` has an effective value whether or not the update stated one, so a row
    // stating none leaves the query at the offset it resolves to rather than at no offset.
    let expected_end_offset = row.end_time_offset.unwrap_or(DEFAULT_END_TIME_OFFSET);
    assert_eq!(
        number(report, "endTimeOffset"),
        Some(expected_end_offset),
        "case {case:?} in {under}: the end offset is the update's, or {expected_end_offset} where \
         it stated none, got {report}"
    );
    assert_eq!(
        number(report, "scheduleStartTime"),
        row.schedule_start_time,
        "case {case:?} in {under}: the schedule start is the update's, cleared where it stated \
         none, got {report}"
    );
    assert_eq!(
        number(report, "scheduleEndTime"),
        row.schedule_end_time,
        "case {case:?} in {under}: the schedule end is the update's, cleared where it stated \
         none, got {report}"
    );

    // `logGroupIdentifiers` is `@length(min: 1)`, so a query naming no log group reports no
    // member at all rather than an empty list.
    let expected_groups = row.log_group_identifiers.map(|identifiers| json!(identifiers));
    assert_eq!(
        report.get("logGroupIdentifiers").cloned(),
        expected_groups,
        "case {case:?} in {under}: the log groups are the update's, expected \
         {expected_groups:?}, got {report}"
    );

    let expected_destination = row.destination.map(Destination::request);
    assert_eq!(
        report.get("destinationConfiguration").cloned(),
        expected_destination,
        "case {case:?} in {under}: the destination configuration is the update's, expected \
         {expected_destination:?}, got {report}"
    );

    // An update configures the query, it does not run it, so no execution is reported for one
    // nothing has executed.
    assert_eq!(
        report.get("lastTriggeredTime"),
        None,
        "case {case:?} in {under}: an update records no execution, got {report}"
    );
    assert_eq!(
        report.get("lastExecutionStatus"),
        None,
        "case {case:?} in {under}: an update records no execution status, got {report}"
    );
}

/// Every row: the update replaces the stored configuration with exactly what it states, answers
/// the query as it then stands, and a read of that query answers the same thing.
#[tokio::test]
async fn an_update_stores_exactly_what_it_states() {
    for row in rows() {
        // One account per row, so each row starts from the same fully populated query.
        let logs = Logs::new();
        seed_log_groups(&logs).await;
        logs.ok(CREATE, &create_request(NAME)).await;
        let arn = scheduled_query_arn(REGION, NAME);

        let created = logs.ok(GET, &json!({"identifier": NAME})).await;
        let created_at = number(&created, "creationTime").unwrap_or_else(|| {
            panic!(
                "case {:?}: the create records a creation instant, got {created}",
                row.case
            )
        });

        let before = now_ms();
        let updated = logs.ok(UPDATE, &update_request(&row, NAME, &arn)).await;
        let after = now_ms();

        assert_reports_update(
            &updated,
            &row,
            &arn,
            created_at,
            (before, after),
            "the update's own response",
        );

        // The write is observable through the reader: the query the account holds is the one the
        // update stated, down to every member.
        let read = logs.ok(GET, &json!({"identifier": NAME})).await;
        assert_reports_update(&read, &row, &arn, created_at, (before, after), "a later read");
        assert_eq!(
            read, updated,
            "case {:?}: the update reports the query it stored, so a read answers the same, got \
             {read} and {updated}",
            row.case
        );

        // Either form of the identifier names the one query, so the ARN form reads back what the
        // write — by whichever form — stored.
        let by_arn = logs.ok(GET, &json!({"identifier": &arn})).await;
        assert_eq!(
            by_arn, read,
            "case {:?}: the name and the ARN identify one query, got {by_arn} and {read}",
            row.case
        );
    }
}

/// An update writes the query its identifier names and no other, and leaves the account holding
/// exactly the queries it held.
#[tokio::test]
async fn an_update_touches_no_other_query() {
    let logs = Logs::new();
    const OTHER: &str = "payments-errors-hourly";

    seed_log_groups(&logs).await;
    logs.ok(CREATE, &create_request(NAME)).await;
    logs.ok(CREATE, &create_request(OTHER)).await;
    let before = logs.ok(GET, &json!({"identifier": OTHER})).await;

    logs.ok(
        UPDATE,
        &json!({
            "identifier": NAME,
            "queryLanguage": "SQL",
            "queryString": "SELECT 1",
            "scheduleExpression": "cron(0 13 * * ? *)",
            "executionRoleArn": OTHER_ROLE,
        }),
    )
    .await;

    let after = logs.ok(GET, &json!({"identifier": OTHER})).await;
    assert_eq!(
        after, before,
        "case {OTHER:?}: an update writes the query it names and no other, expected {before}, \
         got {after}"
    );

    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let held: Vec<String> = state.lock().scheduled_queries.keys().cloned().collect();
    let expected = vec![NAME.to_string(), OTHER.to_string()];
    assert_eq!(
        held, expected,
        "case \"the account's queries\": an update creates none and removes none, expected \
         {expected:?}, got {held:?}"
    );
}

/// No update member states the tags a query carries, so an update leaves them as they were.
#[tokio::test]
async fn an_update_keeps_the_tags_the_create_assigned() {
    let logs = Logs::new();

    seed_log_groups(&logs).await;
    logs.ok(CREATE, &create_request(NAME)).await;
    logs.ok(
        UPDATE,
        &json!({
            "identifier": NAME,
            "queryLanguage": "CWLI",
            "queryString": "fields @message",
            "scheduleExpression": "cron(0 14 * * ? *)",
            "executionRoleArn": ROLE,
        }),
    )
    .await;

    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let store = state.lock();
    let stored = store
        .scheduled_query(NAME)
        .unwrap_or_else(|| panic!("case {NAME:?}: the updated query is stored"));
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
        "case {NAME:?}: an update states no tags, so the create's stand, expected {expected:?}, \
         got {got:?}"
    );
}

/// An identifier the account holds no scheduled query under is refused as a missing resource, in
/// the sentence this service refuses one with — whichever form of identifier it is — and the
/// query the account does hold is left exactly as it was.
#[tokio::test]
async fn an_update_of_an_unknown_identifier_is_refused_and_writes_nothing() {
    let logs = Logs::new();

    seed_log_groups(&logs).await;
    logs.ok(CREATE, &create_request(NAME)).await;
    let before = logs.ok(GET, &json!({"identifier": NAME})).await;

    // A name no query is stored under, a name differing only in case, the ARN form of such a
    // name, the ARN of another resource kind entirely, and a value that is neither.
    let cases = [
        "orders-missing".to_string(),
        NAME.to_uppercase(),
        scheduled_query_arn(REGION, "orders-missing"),
        log_group_arn(REGION, "/aws/lambda/orders", false),
        "not-an-arn-and-not-a-name".to_string(),
    ];

    for identifier in cases {
        let (code, message) = logs
            .refusal(
                UPDATE,
                &json!({
                    "identifier": identifier,
                    "queryLanguage": "CWLI",
                    "queryString": "fields @message",
                    "scheduleExpression": "cron(0 15 * * ? *)",
                    "executionRoleArn": ROLE,
                }),
            )
            .await;
        assert_eq!(
            code, NOT_FOUND,
            "case {identifier:?}: expected {NOT_FOUND}, got {code} {message:?}"
        );
        let expected = not_found_message(&identifier);
        assert_eq!(
            message, expected,
            "case {identifier:?}: expected the refusal {expected:?}, got {message:?}"
        );
    }

    let after = logs.ok(GET, &json!({"identifier": NAME})).await;
    assert_eq!(
        after, before,
        "case \"a refused update\": a refused update writes nothing, expected {before}, got \
         {after}"
    );
    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let held = state.lock().scheduled_queries.len();
    assert_eq!(
        held, 1,
        "case \"a refused update\": a refused update creates no query, expected 1, got {held}"
    );
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

/// The scheduled query CloudWatch manages for a log alarm is held by that alarm's own
/// configuration, so an update naming it is refused as a parameter this operation cannot act on
/// — under either form of the identifier — and nothing of that query changes.
#[tokio::test]
async fn an_update_of_a_service_managed_query_is_refused() {
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
            let (code, message) = logs
                .refusal(
                    UPDATE,
                    &json!({
                        "identifier": identifier,
                        "queryLanguage": "CWLI",
                        "queryString": "fields @message",
                        "scheduleExpression": "cron(0 16 * * ? *)",
                        "executionRoleArn": OTHER_ROLE,
                    }),
                )
                .await;
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
            "case {case:?}: a refused update leaves the managed query as the alarm states it, \
             expected {before}, got {after}"
        );
        let state = logs
            .engine()
            .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
        let held = state.lock().scheduled_queries.len();
        assert_eq!(
            held, 0,
            "case {case:?}: a refused update stores no copy of the managed query, expected 0, \
             got {held}"
        );
    }
}
