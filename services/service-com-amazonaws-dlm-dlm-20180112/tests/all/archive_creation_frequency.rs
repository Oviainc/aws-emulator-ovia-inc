// SPDX-License-Identifier: CC0-1.0

//! The creation frequency a schedule that archives its snapshots must run at,
//! end to end over HTTP.
//!
//! The rule under test: a schedule carrying an `ArchiveRule` creates snapshots
//! no more often than once every 28 days, and a create or an update stating a
//! shorter creation frequency beside such a rule is refused with
//! `InvalidRequestException` (HTTP 400, `Code` `InvalidParameter`) before
//! anything is stored or changed. A schedule carrying no `ArchiveRule` is held
//! to no such floor, and a frequency of 28 days or more is accepted and read
//! back.
//!
//! The sentence is the SECOND bullet of `CreateRule`'s note
//! (`smithy_query(query="doc", shape="CreateRule")`): "If you need to specify an
//! ArchiveRule for the schedule, then you must specify a creation frequency of
//! at least 28 days." Nothing in the model expresses it — the frequency is
//! stated by `Interval` and `IntervalUnit` or by a `CronExpression`, so the floor
//! is a relation between those members and the `ArchiveRule` beside them.
//!
//! A creation rule states its frequency in one of two forms, and the table spans
//! both:
//!
//! * the interval form, whose unit is `HOURS` alone
//!   (`smithy_query(query="shape", shape="IntervalUnitValues")`) and whose
//!   supported intervals are "1, 2, 3, 4, 6, 8, 12, and 24"
//!   (`smithy_query(query="doc", shape="CreateRule", member="Interval")`): EVERY
//!   one of those is driven, and the largest of them — one day — still falls
//!   short, so no interval-form frequency can be stated beside an `ArchiveRule`;
//! * the cron form, whose "schedule interval must be between 1 hour and 1 year"
//!   (`smithy_query(query="shape", shape="CreateRule")`, member
//!   `CronExpression`): hourly, daily, weekly and fortnightly expressions fall
//!   short, a monthly one sits exactly ON the floor (February is 28 days long)
//!   and quarterly and yearly ones clear it.
//!
//! Each row is driven beside an `ArchiveRule` and, for the forms that state a
//! frequency below the floor, without one as well — the floor is the
//! `ArchiveRule`'s, not the creation rule's — through both write paths (the
//! create and the update, which share one rule engine), as the only schedule and
//! as a later element of `Schedules`.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for.
const REGION: &str = "us-west-2";

/// The modelled client error this service answers for a refused parameter, its
/// `@httpError` status and its `Code`.
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";
const INVALID_REQUEST_STATUS: u16 = 400;
const INVALID_PARAMETER_CODE: &str = "InvalidParameter";

/// The documented floor, in days, on the creation frequency of a schedule
/// carrying an `ArchiveRule`: "you must specify a creation frequency of at least
/// 28 days" (`smithy_query(query="doc", shape="CreateRule")`).
const ARCHIVE_CREATION_FREQUENCY_MIN_DAYS: i64 = 28;

/// `IntervalUnitValues` offers `HOURS` alone
/// (`smithy_query(query="shape", shape="IntervalUnitValues")`).
const CREATE_INTERVAL_UNIT: &str = "HOURS";

/// Every value `CreateRule$Interval` supports: "The supported values are 1, 2,
/// 3, 4, 6, 8, 12, and 24" (`smithy_query(query="doc", shape="CreateRule",
/// member="Interval")`). The issue cited only 24.
const SUPPORTED_CREATE_RULE_INTERVALS: [i64; 8] = [1, 2, 3, 4, 6, 8, 12, 24];

/// The name the schedule under test carries, so a refusal can be seen to blame
/// it and not another schedule of the same policy.
const SUBJECT_SCHEDULE: &str = "under-test";

/// An `ExecutionRoleArn` satisfying `^arn:aws(-[a-z]{1,3}){0,2}:iam::\d+:role/`
/// in the emulator's own account, so `iam:PassRole` is granted.
fn role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/AWSDataLifecycleManagerDefaultRole",
        engine::emulator_account_id()
    )
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    fn policy(&self) -> &Value {
        &self.body["Policy"]
    }

    fn policy_id(&self) -> String {
        self.body["PolicyId"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    fn message(&self) -> &str {
        self.body["Message"]
            .as_str()
            .or_else(|| self.body["message"].as_str())
            .unwrap_or_default()
    }
}

/// A SigV4 credential scope naming this service — what routes the request to dlm.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/dlm/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("dlm.{REGION}.amazonaws.com"))
        .header("authorization", signed_for(REGION));
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn create(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/policies", Some(request)).await
}

async fn get(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("/policies/{id}"), None).await
}

async fn update(router: &MultiServiceRouter, id: &str, request: Value) -> Answer {
    call(router, "PATCH", &format!("/policies/{id}"), Some(request)).await
}

/// How many policies the store holds, read through `GetLifecyclePolicies`.
async fn policy_count(router: &MultiServiceRouter) -> usize {
    call(router, "GET", "/policies", None).await.body["Policies"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// True when `stored` carries everything `stated` states: every member of an
/// object and every element of a list, compared the same way. Members the
/// service MATERIALIZES on top of what a request stated (the zero interval of a
/// cron-driven creation rule, a copy rule's `CopyTags`) are left out of the
/// comparison, since this file is about the creation frequency and not about
/// what is stamped.
fn carries(stored: &Value, stated: &Value) -> bool {
    match (stored, stated) {
        (Value::Object(stored), Value::Object(stated)) => stated
            .iter()
            .all(|(member, value)| stored.get(member).is_some_and(|held| carries(held, value))),
        (Value::Array(stored), Value::Array(stated)) => {
            stored.len() == stated.len()
                && stored
                    .iter()
                    .zip(stated)
                    .all(|(held, value)| carries(held, value))
        }
        (held, value) => held == value,
    }
}

/// A create request carrying `configuration` on top of the three members the
/// model marks `@required`. The description is spelled in the characters
/// `PolicyDescription`'s `@pattern(^[0-9A-Za-z _-]+$)` admits and clipped to its
/// `@length(max: 500)`, so a case name is never what is refused.
fn create_request(description: &str, configuration: Value) -> Value {
    let spelled: String = description
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, ' ' | '_' | '-') {
                character
            } else {
                '-'
            }
        })
        .take(500)
        .collect();
    json!({
        "ExecutionRoleArn": role_arn(),
        "Description": spelled,
        "State": "ENABLED",
        "PolicyDetails": configuration,
    })
}

/// A complete custom EBS snapshot policy configuration carrying `schedules`.
/// `ArchiveRule` is `[Custom snapshot policies only]`
/// (`smithy_query(query="shape", shape="Schedule")`), so every row configures a
/// snapshot policy.
fn snapshot_details(schedules: Vec<Value>) -> Value {
    json!({
        "PolicyType": "EBS_SNAPSHOT_MANAGEMENT",
        "ResourceTypes": ["VOLUME"],
        "TargetTags": [{"Key": "backup", "Value": "daily"}],
        "Schedules": schedules,
    })
}

/// A schedule this service accepts: a daily creation rule, count-based retention
/// and no archiving at all.
fn good_schedule(name: &str) -> Value {
    json!({
        "Name": name,
        "CreateRule": {"Interval": 24, "IntervalUnit": CREATE_INTERVAL_UNIT},
        "RetainRule": {"Count": 2},
    })
}

/// The archiving rule a row states when it archives: the count-based tier a
/// count-based schedule retains archived snapshots by ("For count-based
/// schedules, you must specify Count", `smithy_query(query="doc",
/// shape="RetentionArchiveTier")`).
fn archive_rule() -> Value {
    json!({"RetainRule": {"RetentionArchiveTier": {"Count": 90}}})
}

/// What the service owes one request.
#[derive(Debug, Clone)]
enum Owed {
    /// The creation frequency is one the schedule may archive at, so the request
    /// is applied.
    Accepted,
    /// Refused, naming this `CreateRule` member and the value it carried.
    Refused {
        member: &'static str,
        stated: String,
    },
}

/// One creation rule under test: the form it states its frequency in, whether
/// the schedule carrying it archives its snapshots, and what the service owes.
struct FrequencyCase {
    case: String,
    create_rule: Value,
    archived: bool,
    owed: Owed,
}

/// Assert `answer` satisfies `owed`.
fn assert_owed(case: &str, answer: &Answer, owed: &Owed) {
    match owed {
        Owed::Accepted => assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        ),
        Owed::Refused { member, stated } => {
            assert_eq!(
                answer.status, INVALID_REQUEST_STATUS,
                "case {case:?}: expected HTTP {INVALID_REQUEST_STATUS}, got {} with {:?}",
                answer.status, answer.body
            );
            assert!(
                answer.error_code().ends_with(INVALID_REQUEST_EXCEPTION),
                "case {case:?}: expected the modelled {INVALID_REQUEST_EXCEPTION}, got {:?}",
                answer.error_code()
            );
            assert_eq!(
                at(&answer.body, "/Code"),
                json!(INVALID_PARAMETER_CODE),
                "case {case:?}: expected Code {INVALID_PARAMETER_CODE:?}, got {:?}",
                at(&answer.body, "/Code")
            );
            let message = answer.message();
            for named in [
                format!("CreateRule.{member}"),
                format!("{{{SUBJECT_SCHEDULE}}}"),
                stated.clone(),
                format!("{ARCHIVE_CREATION_FREQUENCY_MIN_DAYS} days"),
            ] {
                assert!(
                    message.contains(&named),
                    "case {case:?}: the refusal must name {named:?}, got {message:?}"
                );
            }
            // The refusal is about the frequency a nested structure's member
            // states, not about a pair of the request's own parameters, so
            // neither structured parameter list is filled in.
            for list in ["RequiredParameters", "MutuallyExclusiveParameters"] {
                let reported = at(&answer.body, &format!("/{list}"));
                assert_eq!(
                    reported,
                    Value::Null,
                    "case {case:?}: {list} must be absent, got {reported:?}"
                );
            }
        }
    }
}

/// The schedule a row drives: its creation rule, count-based retention and, when
/// the row archives, the archiving rule the floor is owed to.
fn schedule_for(row: &FrequencyCase) -> Value {
    let mut schedule = json!({
        "Name": SUBJECT_SCHEDULE,
        "CreateRule": row.create_rule.clone(),
        "RetainRule": {"Count": 2},
    });
    if row.archived {
        schedule["ArchiveRule"] = archive_rule();
    }
    schedule
}

/// A policy holding a configuration the service accepts, for the update rows to
/// patch.
async fn subject(router: &MultiServiceRouter) -> String {
    let created = create(
        router,
        create_request(
            "archive creation frequency subject",
            snapshot_details(vec![good_schedule("daily")]),
        ),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the subject policy must be created, got {} with {:?}",
        created.status, created.body
    );
    created.policy_id()
}

/// Run every row against the create (as the only schedule and as the second of
/// two) and against the update, asserting a refused request stores and changes
/// nothing and an accepted one is read back carrying the rules it stated.
async fn assert_frequency_cases(rows: Vec<FrequencyCase>) {
    let router = emulator();
    let subject_id = subject(&router).await;

    for row in rows {
        for position in [1usize, 2] {
            let mut schedules = Vec::new();
            if position == 2 {
                schedules.push(good_schedule("leading"));
            }
            schedules.push(schedule_for(&row));
            let details = snapshot_details(schedules);
            let stated = schedule_for(&row);

            let case = format!("{} (as schedule {position} of a create)", row.case);
            let before = policy_count(&router).await;
            let answer = create(&router, create_request(&case, details.clone())).await;
            assert_owed(&case, &answer, &row.owed);
            match row.owed {
                Owed::Accepted => {
                    assert_eq!(
                        policy_count(&router).await,
                        before + 1,
                        "case {case:?}: an accepted create must store its policy"
                    );
                    let stored = at(
                        get(&router, &answer.policy_id()).await.policy(),
                        &format!("/PolicyDetails/Schedules/{}", position - 1),
                    );
                    assert!(
                        carries(&stored, &stated),
                        "case {case:?}: the stored schedule must carry the rules it stated \
                         ({stated:?}), got {stored:?}"
                    );
                }
                Owed::Refused { .. } => assert_eq!(
                    policy_count(&router).await,
                    before,
                    "case {case:?}: a refused create must store no policy"
                ),
            }

            // The update runs the same walk, so it owes the same answer, and a
            // refused one leaves the stored configuration exactly as it was.
            let stored_before = at(get(&router, &subject_id).await.policy(), "/PolicyDetails");
            let case = format!("{} (as schedule {position} of an update)", row.case);
            let answer = update(&router, &subject_id, json!({"PolicyDetails": details})).await;
            assert_owed(&case, &answer, &row.owed);
            let read = at(get(&router, &subject_id).await.policy(), "/PolicyDetails");
            match row.owed {
                Owed::Accepted => {
                    let stored = at(&read, &format!("/Schedules/{}", position - 1));
                    assert!(
                        carries(&stored, &stated),
                        "case {case:?}: an accepted update must store the rules it stated \
                         ({stated:?}), got {stored:?}"
                    );
                    // Leave the subject holding the accepted configuration
                    // again, so the next row compares against a known state.
                    let restored = update(
                        &router,
                        &subject_id,
                        json!({"PolicyDetails": snapshot_details(vec![good_schedule("daily")])}),
                    )
                    .await;
                    assert_eq!(
                        restored.status, 200,
                        "case {case:?}: the subject policy must be restored, got {} with {:?}",
                        restored.status, restored.body
                    );
                }
                Owed::Refused { .. } => assert_eq!(
                    read, stored_before,
                    "case {case:?}: a refused update must leave the stored configuration alone"
                ),
            }
        }
    }
}

/// RULE, for the INTERVAL form: its unit is `HOURS` alone and its largest
/// supported interval is 24, so one day is the longest creation frequency it can
/// state and EVERY supported interval falls short of the 28 days a schedule that
/// archives must run at least as rarely as. The same intervals are accepted
/// without an `ArchiveRule`, which is what makes the floor the archiving rule's.
#[tokio::test]
async fn an_interval_form_creation_rule_cannot_archive() {
    let mut rows = Vec::new();
    for interval in SUPPORTED_CREATE_RULE_INTERVALS {
        let create_rule = json!({"Interval": interval, "IntervalUnit": CREATE_INTERVAL_UNIT});
        rows.push(FrequencyCase {
            case: format!("an ArchiveRule beside a creation rule of {interval} HOURS"),
            create_rule: create_rule.clone(),
            archived: true,
            owed: Owed::Refused {
                member: "Interval",
                stated: interval.to_string(),
            },
        });
        rows.push(FrequencyCase {
            case: format!("a creation rule of {interval} HOURS and no ArchiveRule"),
            create_rule,
            archived: false,
            owed: Owed::Accepted,
        });
    }
    assert_frequency_cases(rows).await;
}

/// An interval the member does not support states no frequency this sentence can
/// be asked about: it is refused on its own terms, by the domain
/// `CreateRule$Interval`'s documentation publishes ("The supported values are 1,
/// 2, 3, 4, 6, 8, 12, and 24"), and the same answer is owed whether or not the
/// schedule carries an `ArchiveRule`.
///
/// The rows span an interval between two of the supported values, one above its
/// ceiling, and one that would be as long as the archiving floor if it were
/// supported at all.
#[tokio::test]
async fn an_unsupported_interval_is_refused_on_its_own_terms() {
    let router = emulator();
    let subject_id = subject(&router).await;

    // 24 hours a day, so the last of these states exactly the archiving floor —
    // and is still not a value the member supports.
    for interval in [5, 36, 24 * ARCHIVE_CREATION_FREQUENCY_MIN_DAYS] {
        for archived in [true, false] {
            let mut schedule = json!({
                "Name": SUBJECT_SCHEDULE,
                "CreateRule": {"Interval": interval, "IntervalUnit": CREATE_INTERVAL_UNIT},
                "RetainRule": {"Count": 2},
            });
            if archived {
                schedule["ArchiveRule"] = archive_rule();
            }
            let details = snapshot_details(vec![schedule]);
            let case = format!(
                "a creation interval of {interval} HOURS{}",
                if archived {
                    " beside an ArchiveRule"
                } else {
                    ""
                }
            );

            for (path, answer) in [
                ("create", create(&router, create_request(&case, details.clone())).await),
                (
                    "update",
                    update(&router, &subject_id, json!({"PolicyDetails": details.clone()})).await,
                ),
            ] {
                let case = format!("{case} (through the {path})");
                assert_eq!(
                    answer.status, INVALID_REQUEST_STATUS,
                    "case {case:?}: expected HTTP {INVALID_REQUEST_STATUS}, got {} with {:?}",
                    answer.status, answer.body
                );
                let message = answer.message();
                assert!(
                    message.contains("The supported values are"),
                    "case {case:?}: the member's own domain must be what is named, got \
                     {message:?}"
                );
                assert!(
                    !message.contains(&format!("{ARCHIVE_CREATION_FREQUENCY_MIN_DAYS} days")),
                    "case {case:?}: the archiving floor is not what refuses an unsupported \
                     interval, got {message:?}"
                );
            }
        }
    }
}

/// RULE, for the CRON form: the frequency is the shortest span between two of
/// the instants the expression names, and a schedule carrying an `ArchiveRule`
/// is refused for every expression that creates snapshots more often than once
/// every 28 days.
///
/// The rows walk the calendar frequencies the grammar states, from an hour
/// (`CronExpression`'s own floor, "the schedule interval must be between 1 hour
/// and 1 year") up to a year (its ceiling): hourly, daily, weekly and twice a
/// month fall short, a monthly expression sits exactly ON the floor (February is
/// 28 days long) and quarterly and yearly ones clear it. Two spellings of the
/// dialect this service computes no instants from pin the deliberate permissive
/// end: an expression whose frequency cannot be decided is not refused. The
/// expressions that fall short are driven without an `ArchiveRule` too, where
/// they are accepted.
#[tokio::test]
async fn a_cron_creation_rule_archives_only_at_or_above_the_floor() {
    // Each expression with the frequency it states, as AWS cron expressions are
    // written: minute, hour, day-of-month, month, day-of-week, year, and whether
    // a schedule carrying an `ArchiveRule` may create on it.
    let expressions: [(&str, &str, bool); 9] = [
        ("every hour", "cron(0 * * * ? *)", false),
        ("every day", "cron(0 12 * * ? *)", false),
        ("every week", "cron(0 12 ? * MON *)", false),
        ("twice a month", "cron(0 12 1,15 * ? *)", false),
        // A month is the shortest frequency at or above the floor: the February
        // of a common year is exactly 28 days long.
        ("every month", "cron(0 0 1 * ? *)", true),
        ("every quarter", "cron(0 0 1 1,4,7,10 ? *)", true),
        ("every year", "cron(0 0 1 1 ? *)", true),
        // Two spellings this service computes no instants from — the
        // last-day-of-the-month `L` and the nth-weekday `#` of the Amazon Web
        // Services cron dialect. An expression whose frequency cannot be
        // decided states none this sentence can be asked about, so it is NOT
        // refused: the floor is enforced on what a request decidably asked for,
        // and refusing an expression the service cannot read would refuse
        // schedules AWS accepts.
        ("an hourly last-day-of-the-month spelling", "cron(0 * L * ? *)", true),
        ("an hourly nth-weekday spelling", "cron(0 * ? * 6#3 *)", true),
    ];

    let mut rows = Vec::new();
    for (frequency, expression, may_archive) in expressions {
        let create_rule = json!({"CronExpression": expression});
        rows.push(FrequencyCase {
            case: format!("an ArchiveRule beside a creation rule of {frequency}"),
            create_rule: create_rule.clone(),
            archived: true,
            owed: if may_archive {
                Owed::Accepted
            } else {
                Owed::Refused {
                    member: "CronExpression",
                    stated: expression.to_string(),
                }
            },
        });
        if !may_archive {
            rows.push(FrequencyCase {
                case: format!("a creation rule of {frequency} and no ArchiveRule"),
                create_rule,
                archived: false,
                owed: Owed::Accepted,
            });
        }
    }
    assert_frequency_cases(rows).await;
}
