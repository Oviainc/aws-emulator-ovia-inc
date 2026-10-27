// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListScheduledQueries`, the operation that enumerates the scheduled
//! queries an account holds.
//!
//! The rule under test: `ListScheduledQueries` reports EVERY scheduled query the account and
//! Region holds — the ones `CreateScheduledQuery` stored and the one CloudWatch manages for
//! each log alarm alike — each rendered from the record that owns it (so a listing and
//! `GetScheduledQuery` cannot disagree about one), in ascending name order whatever order the
//! queries were created in; `state` and `scheduleType` each narrow the enumeration to the
//! queries carrying that value and nothing else, and a request naming neither gets all of
//! them; a page holds at most `maxResults` queries and carries a `nextToken` exactly while
//! queries remain beyond it, resuming the SAME enumeration after the last query reported. A
//! token this operation did not mint, or one minted for different filters, is refused and
//! reports nothing, and reading changes nothing a later caller sees.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a response
//! that is not computed from the shared store cannot pass.
//!
//! The corpus here spans what the rule generalises over rather than one request: the created
//! queries differ in name case (an upper-case initial sorts before every lower-case one) and in
//! whether one name is a prefix of another, in query language (each value the model's
//! `QueryLanguage` enumerates), in the state named explicitly and left to the documented
//! default, in which optional members the create named (none, some, all) and in which
//! destination the results go to; the managed queries differ in whether the owning alarm names
//! an ARN of its own, and one alarm carries no scheduled query at all; the filter table spans
//! each value of both filter members, the two combined, and a combination nothing matches; the
//! page table spans every page size from one query to more than the corpus holds, plus the
//! member's published bounds and one past each, and the unset default is read off an account
//! holding one query more than that default.

use serde_json::{json, Map, Value};

use service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;
use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    scheduled_query_arn, DEFAULT_LIST_LIMIT,
};

use crate::harness::{Logs, REGION};

/// The operation under test, the one that puts a query in the store, and the single-query read
/// a listing must agree with.
const LIST: &str = "ListScheduledQueries";
const CREATE: &str = "CreateScheduledQuery";
const GET: &str = "GetScheduledQuery";

/// The code every scheduled-query operation answers for a parameter it will not accept ("One of
/// the parameters for the request is not valid").
const VALIDATION: &str = "ValidationException";

/// The two values the model's `ScheduledQueryState` enumerates; `ENABLED` is also the state a
/// create that names none starts a query in ("Default is ENABLED").
const ENABLED: &str = "ENABLED";
const DISABLED: &str = "DISABLED";

/// The two values the model's `ScheduleType` enumerates: the schedule of a query a caller
/// created is the caller's, and the schedule of the query CloudWatch manages for a log alarm is
/// the service's.
const CUSTOMER_MANAGED: &str = "CUSTOMER_MANAGED";
const AWS_MANAGED: &str = "AWS_MANAGED";

/// The published bounds of `maxResults`: `ListScheduledQueriesMaxResults` carries
/// `@range(min: 1, max: 1000)`, which the member's own documentation states as well ("Valid
/// range is 1 to 1000").
const MAX_RESULTS_FLOOR: i64 = 1;
const MAX_RESULTS_CEILING: i64 = 1_000;

/// The IAM role every created query executes under.
const ROLE: &str = "arn:aws:iam::000000000000:role/scheduled-query-execution";

/// The role a destination delivers results under, which the model gives its own member.
const DELIVERY_ROLE: &str = "arn:aws:iam::000000000000:role/scheduled-query-delivery";

/// Where a created query delivers its results — the destinations the model's
/// `DestinationConfiguration` carries, which `CreateScheduledQuery$destinationConfiguration`
/// allows one or both of ("You can configure one or both destination types").
#[derive(Clone, Copy, Debug)]
enum Destination {
    None,
    S3,
    LookupTable,
    Both,
}

impl Destination {
    /// This destination as the request states it, and therefore as every reader reports it.
    fn request(self) -> Option<Value> {
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
            Self::None => None,
            Self::S3 => Some(json!({"s3Configuration": s3})),
            Self::LookupTable => Some(json!({"lookupTableConfiguration": lookup_table})),
            Self::Both => Some(
                json!({"s3Configuration": s3, "lookupTableConfiguration": lookup_table}),
            ),
        }
    }
}

/// One created scheduled query: what the create names, and therefore what every reader of it
/// must report back.
struct Seed {
    /// What this row varies, for the assertion messages.
    case: &'static str,
    name: &'static str,
    /// One value of the model's `QueryLanguage` enum (`CWLI` | `SQL` | `PPL`).
    query_language: &'static str,
    /// The state the create names, or `None` to leave it at the documented default.
    state: Option<&'static str>,
    timezone: Option<&'static str>,
    destination: Destination,
    description: Option<&'static str>,
}

impl Seed {
    /// The state a reader reports for this query: the one the create named, or the documented
    /// `ENABLED` default when it named none.
    fn reported_state(&self) -> &'static str {
        self.state.unwrap_or(ENABLED)
    }
}

/// The created corpus, in the scrambled order the queries are CREATED in: the order a listing
/// reports is the rule under test, so it must not be the order they went in.
fn seeds() -> Vec<Seed> {
    vec![
        Seed {
            case: "every optional member named, SQL, DISABLED, both destinations",
            name: "orders-sql",
            query_language: "SQL",
            state: Some(DISABLED),
            timezone: Some("Europe/London"),
            destination: Destination::Both,
            description: Some("the nightly order roll-up"),
        },
        Seed {
            case: "no optional member named, CWLI, the default state",
            name: "zeta-defaults",
            query_language: "CWLI",
            state: None,
            timezone: None,
            destination: Destination::None,
            description: None,
        },
        Seed {
            case: "an upper-case initial, PPL, ENABLED named explicitly, Amazon S3",
            name: "Alpha-ppl",
            query_language: "PPL",
            state: Some(ENABLED),
            timezone: Some("UTC"),
            destination: Destination::S3,
            description: None,
        },
        Seed {
            case: "a name another query's name is a prefix of",
            name: "orders-sql-nightly",
            query_language: "CWLI",
            state: Some(ENABLED),
            timezone: None,
            destination: Destination::LookupTable,
            description: Some("the same roll-up, hourly"),
        },
        Seed {
            case: "a lookup-table destination, DISABLED",
            name: "metrics-lookup",
            query_language: "CWLI",
            state: Some(DISABLED),
            timezone: Some("America/New_York"),
            destination: Destination::LookupTable,
            description: None,
        },
    ]
}

/// One log alarm whose configuration CloudWatch manages a scheduled query for. The alarm is the
/// one home of that query's configuration, so the query is derived from the alarm rather than
/// created by any call — which is why a suite seeds the ALARM and reads the QUERY back over the
/// wire.
struct Managed {
    /// What this row varies, for the assertion messages.
    case: &'static str,
    /// The alarm's name, which is the managed query's name.
    name: &'static str,
    /// The ARN the alarm's configuration names its managed query by, when it names one.
    query_arn: Option<&'static str>,
    /// Whether the alarm carries a scheduled query configuration at all: an alarm with none
    /// owns no managed query, so no listing reports one for it.
    configured: bool,
}

/// The seeded alarms: one naming an ARN of its own, one leaving the query named by nothing but
/// the alarm, and one carrying no scheduled query at all.
fn managed() -> Vec<Managed> {
    vec![
        Managed {
            case: "an alarm naming its managed query's ARN",
            name: "beta-log-alarm",
            query_arn: Some(
                "arn:aws:logs:eu-west-2:000000000000:scheduled-query:beta-log-alarm-managed",
            ),
            configured: true,
        },
        Managed {
            case: "an alarm whose configuration names no ARN",
            name: "gamma-log-alarm",
            query_arn: None,
            configured: true,
        },
        Managed {
            case: "an alarm with no scheduled query configuration",
            name: "delta-plain-alarm",
            query_arn: None,
            configured: false,
        },
    ]
}

/// The schedule expression every managed query runs on, and the lookback its alarm states.
const MANAGED_SCHEDULE: &str = "rate(5 minutes)";
const MANAGED_START_OFFSET: i64 = 900;

/// Put a log alarm in the CloudWatch store, so the account holds the scheduled query CloudWatch
/// manages for it.
///
/// `PutLogAlarm` belongs to the CloudWatch service, which this target does not mount, so the
/// alarm is seeded on the shared engine the way `harness::report_anomaly` seeds a detected
/// anomaly; everything the suite then asserts goes over the wire like any client's call.
fn seed_log_alarm(logs: &Logs, alarm: &Managed, updated_ms: i64) {
    let state = logs
        .engine()
        .state::<cloudwatch::state::ComAmazonawsCloudwatchGraniteserviceversion20100801State>();
    let mut store = state.lock();
    store.log_alarms.insert(
        alarm.name.to_string(),
        cloudwatch::state::StoredLogAlarm {
            alarm: cloudwatch::ops::types::LogAlarm {
                alarm_name: Some(alarm.name.to_string()),
                alarm_configuration_updated_timestamp: Some(
                    ::aws_smithy_types::DateTime::from_millis(updated_ms),
                ),
                scheduled_query_configuration: alarm.configured.then(|| {
                    cloudwatch::ops::types::ScheduledQueryConfiguration {
                        query_string: "fields @message | stats count() by bin(5m)".to_string(),
                        log_group_identifiers: Some(vec!["/aws/lambda/orders".to_string()]),
                        query_arn: alarm.query_arn.map(str::to_string),
                        scheduled_query_role_arn: ROLE.to_string(),
                        schedule_configuration: cloudwatch::ops::types::ScheduleConfiguration {
                            schedule_expression: MANAGED_SCHEDULE.to_string(),
                            start_time_offset: MANAGED_START_OFFSET,
                            end_time_offset: None,
                        },
                        ..cloudwatch::ops::types::ScheduledQueryConfiguration::default()
                    }
                }),
                ..cloudwatch::ops::types::LogAlarm::default()
            },
            tags: Vec::new(),
        },
    );
}

/// The whole corpus in the store: every created query, through the wire call that creates one,
/// and every seeded log alarm.
async fn seed(logs: &Logs) {
    for seed in seeds() {
        logs.ok(CREATE, &create_request(&seed)).await;
    }
    for (index, alarm) in managed().iter().enumerate() {
        // Each alarm's configuration was last written at its own instant, so a reader that
        // reports one alarm's instant for another's query cannot pass.
        seed_log_alarm(logs, alarm, 1_700_000_000_000 + i64::try_from(index).unwrap() * 1_000);
    }
}

/// The `CreateScheduledQuery` request one seed states.
fn create_request(seed: &Seed) -> Value {
    let mut request = Map::new();
    request.insert("name".to_string(), json!(seed.name));
    request.insert("queryLanguage".to_string(), json!(seed.query_language));
    request.insert("queryString".to_string(), json!("fields @message"));
    request.insert("scheduleExpression".to_string(), json!("cron(0 * * * ? *)"));
    request.insert("executionRoleArn".to_string(), json!(ROLE));
    if let Some(state) = seed.state {
        request.insert("state".to_string(), json!(state));
    }
    if let Some(timezone) = seed.timezone {
        request.insert("timezone".to_string(), json!(timezone));
    }
    if let Some(description) = seed.description {
        request.insert("description".to_string(), json!(description));
    }
    if let Some(destination) = seed.destination.request() {
        request.insert("destinationConfiguration".to_string(), destination);
    }
    Value::Object(request)
}

/// Every name the account holds a scheduled query under, in the ascending order a listing
/// reports them in.
fn ascending_names() -> Vec<String> {
    let mut names: Vec<String> = seeds().iter().map(|seed| seed.name.to_string()).collect();
    names.extend(
        managed()
            .iter()
            .filter(|alarm| alarm.configured)
            .map(|alarm| alarm.name.to_string()),
    );
    names.sort();
    names
}

/// One `ListScheduledQueries` request, from the members a case names.
fn request(
    state: Option<&str>,
    schedule_type: Option<&str>,
    max_results: Option<i64>,
    token: Option<&str>,
) -> Value {
    let mut request = Map::new();
    if let Some(state) = state {
        request.insert("state".to_string(), json!(state));
    }
    if let Some(schedule_type) = schedule_type {
        request.insert("scheduleType".to_string(), json!(schedule_type));
    }
    if let Some(max_results) = max_results {
        request.insert("maxResults".to_string(), json!(max_results));
    }
    if let Some(token) = token {
        request.insert("nextToken".to_string(), json!(token));
    }
    Value::Object(request)
}

/// The summaries one page reports.
fn queries_of(page: &Value, case: &str) -> Vec<Value> {
    page.get("scheduledQueries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: a page reports scheduledQueries, got {page}"))
        .clone()
}

/// The names one page reports, in the order it reports them.
fn names_of(page: &Value, case: &str) -> Vec<String> {
    queries_of(page, case)
        .iter()
        .map(|query| {
            query
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every reported query carries name, got {query}")
                })
                .to_string()
        })
        .collect()
}

/// Every query one enumeration reports, read page by page through the tokens it mints: a page
/// holds at most `maxResults` queries, a token arrives only with a page that reported
/// something, and the walk terminates.
async fn walk(
    logs: &Logs,
    case: &str,
    state: Option<&str>,
    schedule_type: Option<&str>,
    max_results: Option<i64>,
) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let page = logs
            .ok(
                LIST,
                &request(state, schedule_type, max_results, token.as_deref()),
            )
            .await;
        let reported = names_of(&page, case);
        if let Some(max_results) = max_results {
            let held = i64::try_from(reported.len()).expect("a page's size fits in an i64");
            assert!(
                held <= max_results,
                "case {case:?}: a page holds at most maxResults={max_results} queries, got \
                 {reported:?}"
            );
        }
        names.extend(reported.clone());
        token = page
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_some() {
            assert!(
                !reported.is_empty(),
                "case {case:?}: a page that reported nothing cannot carry a nextToken, got \
                 {token:?}"
            );
        }
        pages += 1;
        assert!(
            pages <= 256,
            "case {case:?}: the walk terminates, got {pages} pages and nextToken {token:?}"
        );
        if token.is_none() {
            break;
        }
    }
    names
}

/// Every held query — created or service-managed — is reported in ascending name order, with
/// every summary member read off the record that owns it, which is what the single-query read
/// reports too.
#[tokio::test]
async fn every_held_query_is_reported_in_ascending_name_order() {
    let logs = Logs::new();
    seed(&logs).await;

    let page = logs.ok(LIST, &request(None, None, None, None)).await;
    let reported = queries_of(&page, "the whole account");
    assert_eq!(
        names_of(&page, "the whole account"),
        ascending_names(),
        "case \"the whole account\": every held query is reported in ascending name order, got \
         {reported:?}"
    );
    assert!(
        page.get("nextToken").is_none(),
        "case \"the whole account\": one page holds every query, so no nextToken follows, got \
         {page}"
    );

    let summary_of = |name: &str, case: &str| -> Value {
        reported
            .iter()
            .find(|query| query.get("name").and_then(Value::as_str) == Some(name))
            .unwrap_or_else(|| panic!("case {case:?}: the listing reports {name}, got {reported:?}"))
            .clone()
    };

    for seed in seeds() {
        let case = seed.case;
        let query = summary_of(seed.name, case);

        let expected_arn = scheduled_query_arn(REGION, seed.name);
        assert_eq!(
            query.get("scheduledQueryArn").and_then(Value::as_str),
            Some(expected_arn.as_str()),
            "case {case:?}: expected scheduledQueryArn {expected_arn}, got {query}"
        );
        assert_eq!(
            query.get("state").and_then(Value::as_str),
            Some(seed.reported_state()),
            "case {case:?}: expected state {}, got {query}",
            seed.reported_state()
        );
        assert_eq!(
            query.get("scheduleType").and_then(Value::as_str),
            Some(CUSTOMER_MANAGED),
            "case {case:?}: a query a caller created carries the caller's schedule, got {query}"
        );
        assert_eq!(
            query.get("scheduleExpression").and_then(Value::as_str),
            Some("cron(0 * * * ? *)"),
            "case {case:?}: the schedule expression the create named is reported, got {query}"
        );
        assert_eq!(
            query.get("timezone").and_then(Value::as_str),
            seed.timezone,
            "case {case:?}: the timezone is reported only when the create named one, got {query}"
        );
        assert_eq!(
            query.get("destinationConfiguration").cloned(),
            seed.destination.request(),
            "case {case:?}: expected the destination the create named ({:?}), got {query}",
            seed.destination
        );
        let creation = query.get("creationTime").and_then(Value::as_i64);
        assert!(
            creation.is_some(),
            "case {case:?}: a created query reports the instant it was created, got {query}"
        );
        assert_eq!(
            query.get("lastUpdatedTime").and_then(Value::as_i64),
            creation,
            "case {case:?}: nothing has updated this query, so it was last updated when it was \
             created, got {query}"
        );
        for absent in ["lastTriggeredTime", "lastExecutionStatus"] {
            assert!(
                query.get(absent).is_none(),
                "case {case:?}: {absent} reports an execution that happened, and none has, got \
                 {query}"
            );
        }
        // A SUMMARY carries "basic configuration and execution status", so the members only the
        // single-query read models are not in it.
        for absent in ["queryString", "queryLanguage", "logGroupIdentifiers", "executionRoleArn"] {
            assert!(
                query.get(absent).is_none(),
                "case {case:?}: a summary carries no {absent}, got {query}"
            );
        }

        // The listing and the single-query read are two readers of ONE record, so what they
        // report about a query cannot differ.
        let read = logs.ok(GET, &json!({"identifier": seed.name})).await;
        for member in ["scheduledQueryArn", "name", "state", "scheduleType", "scheduleExpression", "timezone"] {
            assert_eq!(
                query.get(member).and_then(Value::as_str),
                read.get(member).and_then(Value::as_str),
                "case {case:?}: the listing and the read agree about {member}, got {query} and \
                 {read}"
            );
        }
        for member in ["creationTime", "lastUpdatedTime"] {
            assert_eq!(
                query.get(member).and_then(Value::as_i64),
                read.get(member).and_then(Value::as_i64),
                "case {case:?}: the listing and the read agree about {member}, got {query} and \
                 {read}"
            );
        }
        assert_eq!(
            query.get("destinationConfiguration"),
            read.get("destinationConfiguration"),
            "case {case:?}: the listing and the read agree about destinationConfiguration, got \
             {query} and {read}"
        );
    }

    for alarm in managed() {
        if !alarm.configured {
            assert!(
                !names_of(&page, alarm.case).iter().any(|name| name == alarm.name),
                "case {:?}: an alarm with no scheduled query owns none to report, got {reported:?}",
                alarm.case
            );
            continue;
        }

        let case = alarm.case;
        let query = summary_of(alarm.name, case);
        let expected_arn = alarm
            .query_arn
            .map(str::to_string)
            .unwrap_or_else(|| scheduled_query_arn(REGION, alarm.name));
        assert_eq!(
            query.get("scheduledQueryArn").and_then(Value::as_str),
            Some(expected_arn.as_str()),
            "case {case:?}: expected scheduledQueryArn {expected_arn}, got {query}"
        );
        assert_eq!(
            query.get("scheduleType").and_then(Value::as_str),
            Some(AWS_MANAGED),
            "case {case:?}: the schedule of a query the service manages is the service's, got \
             {query}"
        );
        assert_eq!(
            query.get("scheduleExpression").and_then(Value::as_str),
            Some(MANAGED_SCHEDULE),
            "case {case:?}: the alarm's schedule is the managed query's schedule, got {query}"
        );
        assert!(
            query.get("creationTime").is_none(),
            "case {case:?}: the alarm records no instant of its creation, so the query reports \
             none, got {query}"
        );
        assert!(
            query.get("lastUpdatedTime").and_then(Value::as_i64).is_some(),
            "case {case:?}: the alarm's configuration instant is the managed query's, got {query}"
        );

        // The listing and the single-query read derive the same query from the same alarm.
        let read = logs.ok(GET, &json!({"identifier": alarm.name})).await;
        for member in ["scheduledQueryArn", "name", "state", "scheduleType", "scheduleExpression"] {
            assert_eq!(
                query.get(member).and_then(Value::as_str),
                read.get(member).and_then(Value::as_str),
                "case {case:?}: the listing and the read agree about {member}, got {query} and \
                 {read}"
            );
        }
        assert_eq!(
            query.get("lastUpdatedTime").and_then(Value::as_i64),
            read.get("lastUpdatedTime").and_then(Value::as_i64),
            "case {case:?}: the listing and the read agree about lastUpdatedTime, got {query} \
             and {read}"
        );
    }
}

/// `state` and `scheduleType` each select exactly the queries carrying that value, and a
/// request naming neither gets every query the account holds.
#[tokio::test]
async fn the_filters_select_exactly_the_queries_carrying_their_value() {
    let logs = Logs::new();
    seed(&logs).await;

    // Every query the corpus holds, with the state and the schedule type a reader reports for
    // it, so the expected set of a filter is DERIVED from the corpus rather than typed out.
    let mut held: Vec<(String, &str, &str)> = seeds()
        .iter()
        .map(|seed| {
            (
                seed.name.to_string(),
                seed.reported_state(),
                CUSTOMER_MANAGED,
            )
        })
        .collect();
    held.extend(
        managed()
            .iter()
            .filter(|alarm| alarm.configured)
            // A managed query is in the state a scheduled query starts in; nothing on the alarm
            // disables its query.
            .map(|alarm| (alarm.name.to_string(), ENABLED, AWS_MANAGED)),
    );
    held.sort_by(|left, right| left.0.cmp(&right.0));

    let selected = |state: Option<&str>, schedule_type: Option<&str>| -> Vec<String> {
        held.iter()
            .filter(|(_, held_state, _)| state.is_none_or(|wanted| *held_state == wanted))
            .filter(|(_, _, held_type)| schedule_type.is_none_or(|wanted| *held_type == wanted))
            .map(|(name, _, _)| name.clone())
            .collect()
    };

    // (case, the state filter, the scheduleType filter)
    let cases: Vec<(&str, Option<&str>, Option<&str>)> = vec![
        ("neither filter", None, None),
        ("state ENABLED", Some(ENABLED), None),
        ("state DISABLED", Some(DISABLED), None),
        ("scheduleType CUSTOMER_MANAGED", None, Some(CUSTOMER_MANAGED)),
        ("scheduleType AWS_MANAGED", None, Some(AWS_MANAGED)),
        (
            "both filters, matching several queries",
            Some(ENABLED),
            Some(CUSTOMER_MANAGED),
        ),
        (
            "both filters, matching exactly one query",
            Some(ENABLED),
            Some(AWS_MANAGED),
        ),
        (
            "both filters, matching nothing",
            Some(DISABLED),
            Some(AWS_MANAGED),
        ),
    ];

    for (case, state, schedule_type) in cases {
        let expected = selected(state, schedule_type);
        let page = logs
            .ok(LIST, &request(state, schedule_type, None, None))
            .await;
        let names = names_of(&page, case);
        assert_eq!(
            names, expected,
            "case {case:?}: state={state:?} scheduleType={schedule_type:?} selects {expected:?}, \
             got {names:?}"
        );
        assert!(
            page.get("nextToken").is_none(),
            "case {case:?}: one page holds every selected query, so no nextToken follows, got \
             {page}"
        );

        // The same selection, walked one query per page: a filtered enumeration pages the same
        // way the whole one does.
        let walked = walk(&logs, case, state, schedule_type, Some(1)).await;
        assert_eq!(
            walked, expected,
            "case {case:?}: walking the filtered enumeration one query at a time reports \
             {expected:?}, got {walked:?}"
        );
    }
}

/// A page holds at most `maxResults` queries and the token resumes the same enumeration after
/// the last one reported, so walking the pages of any page size reports the enumeration once
/// and whole.
#[tokio::test]
async fn walking_the_pages_reports_the_enumeration_once_and_whole() {
    let logs = Logs::new();
    seed(&logs).await;

    let whole = ascending_names();
    for max_results in [
        MAX_RESULTS_FLOOR,
        2,
        3,
        4,
        6,
        7,
        8,
        MAX_RESULTS_CEILING,
    ] {
        let case = format!("maxResults {max_results} over the whole account");
        let walked = walk(&logs, &case, None, None, Some(max_results)).await;
        assert_eq!(
            walked, whole,
            "case {case:?}: the pages concatenate to the whole enumeration {whole:?}, got \
             {walked:?}"
        );
    }

    // A first page of one names the first query and a token; the token's page resumes AFTER it
    // rather than repeating it.
    let first = logs.ok(LIST, &request(None, None, Some(1), None)).await;
    assert_eq!(
        names_of(&first, "the first page of one"),
        vec![whole[0].clone()],
        "case \"the first page of one\": the first query of the enumeration is reported, got \
         {first}"
    );
    let token = first
        .get("nextToken")
        .and_then(Value::as_str)
        .expect("a page with queries beyond it carries a nextToken")
        .to_string();
    let second = logs
        .ok(LIST, &request(None, None, Some(1), Some(&token)))
        .await;
    assert_eq!(
        names_of(&second, "the page after the first"),
        vec![whole[1].clone()],
        "case \"the page after the first\": the walk resumes after the last query reported, got \
         {second}"
    );

    // The token is a position, not a consumption: replaying it answers the same page.
    let again = logs
        .ok(LIST, &request(None, None, Some(1), Some(&token)))
        .await;
    assert_eq!(
        second, again,
        "case \"a replayed token\": a token names a position in the enumeration, got {second} \
         then {again}"
    );
}

/// A request that names no `maxResults` gets this service's own list page size, not the whole
/// store and not an empty page.
#[tokio::test]
async fn an_unset_max_results_pages_at_the_services_list_size() {
    let logs = Logs::new();

    let default = usize::try_from(DEFAULT_LIST_LIMIT).expect("the page size is not negative");
    // One query more than that page size, so the default is observable: the names are
    // zero-padded, so their ascending order is the order they are numbered in.
    let held = default + 1;
    for index in 0..held {
        logs.ok(
            CREATE,
            &json!({
                "name": format!("page-{index:03}"),
                "queryLanguage": "CWLI",
                "queryString": "fields @message",
                "scheduleExpression": "cron(0 * * * ? *)",
                "executionRoleArn": ROLE,
            }),
        )
        .await;
    }

    let page = logs.ok(LIST, &request(None, None, None, None)).await;
    let names = names_of(&page, "an unset maxResults");
    assert_eq!(
        names.len(),
        default,
        "case \"an unset maxResults\": expected a page of {default} queries, got {}",
        names.len()
    );
    assert_eq!(
        names[0], "page-000",
        "case \"an unset maxResults\": the page starts at the first query of the enumeration, \
         got {}",
        names[0]
    );

    let walked = walk(&logs, "an unset maxResults, walked", None, None, None).await;
    assert_eq!(
        walked.len(),
        held,
        "case \"an unset maxResults, walked\": the pages concatenate to every held query \
         ({held}), got {}",
        walked.len()
    );
}

/// An account holding no scheduled query reports an empty enumeration, not a missing resource,
/// and reading changes nothing a later caller sees.
#[tokio::test]
async fn an_account_holding_no_query_reports_an_empty_enumeration() {
    let logs = Logs::new();

    for case in ["the first read", "the second read"] {
        let page = logs.ok(LIST, &request(None, None, None, None)).await;
        let names = names_of(&page, case);
        assert!(
            names.is_empty(),
            "case {case:?}: an account holding no query reports no query, got {names:?}"
        );
        assert!(
            page.get("nextToken").is_none(),
            "case {case:?}: an empty enumeration has no next set, got {page}"
        );
    }

    // And a read of a populated account is repeatable too: a listing reports, it does not
    // consume.
    seed(&logs).await;
    let first = logs.ok(LIST, &request(None, None, None, None)).await;
    let second = logs.ok(LIST, &request(None, None, None, None)).await;
    assert_eq!(
        first, second,
        "case \"two reads of one account\": a listing reports the store rather than changing it, \
         got {first} then {second}"
    );
}

/// A `nextToken` this operation did not mint for THIS enumeration is refused, and a
/// `maxResults` outside the member's published range is refused for its bound.
#[tokio::test]
async fn a_token_or_a_bound_the_operation_did_not_issue_is_refused() {
    let logs = Logs::new();
    seed(&logs).await;

    // A token minted under one filter names a position in THAT enumeration, so continuing a
    // different one with it is refused.
    let page = logs
        .ok(LIST, &request(Some(ENABLED), None, Some(1), None))
        .await;
    let foreign = page
        .get("nextToken")
        .and_then(Value::as_str)
        .expect("a first page of one over several enabled queries carries a nextToken")
        .to_string();

    // (case, the request it is made with)
    let cases: Vec<(&str, Value)> = vec![
        (
            "a token that is not one of this operation's",
            request(None, None, None, Some("not-a-token")),
        ),
        (
            "a token minted for another state filter",
            request(Some(DISABLED), None, Some(1), Some(&foreign)),
        ),
        (
            "a token minted under a filter, continued without one",
            request(None, None, Some(1), Some(&foreign)),
        ),
        (
            "a token minted without a scheduleType, continued with one",
            request(Some(ENABLED), Some(CUSTOMER_MANAGED), Some(1), Some(&foreign)),
        ),
        (
            "a maxResults one below the published minimum",
            request(None, None, Some(MAX_RESULTS_FLOOR - 1), None),
        ),
        (
            "a maxResults one past the published maximum",
            request(None, None, Some(MAX_RESULTS_CEILING + 1), None),
        ),
        (
            "a state outside the member's enum",
            request(Some("PAUSED"), None, None, None),
        ),
        (
            "a scheduleType outside the member's enum",
            request(None, Some("SELF_MANAGED"), None, None),
        ),
    ];

    for (case, body) in cases {
        let (code, message) = logs.refusal(LIST, &body).await;
        assert_eq!(
            code, VALIDATION,
            "case {case:?}: expected {VALIDATION}, got {code} {message:?}"
        );
    }

    // Each published bound itself is accepted: a bound is the last admitted value.
    for max_results in [MAX_RESULTS_FLOOR, MAX_RESULTS_CEILING] {
        let case = format!("a maxResults at the published bound {max_results}");
        let walked = walk(&logs, &case, None, None, Some(max_results)).await;
        assert_eq!(
            walked,
            ascending_names(),
            "case {case:?}: the bound is admitted and the enumeration is whole, got {walked:?}"
        );
    }

    // Every refusal above reported nothing, and the store is as it was.
    let walked = walk(&logs, "after the refusals", None, None, Some(2)).await;
    assert_eq!(
        walked,
        ascending_names(),
        "case \"after the refusals\": a refused listing changes nothing, got {walked:?}"
    );
}
