// SPDX-License-Identifier: CC0-1.0

//! The retention a schedule must keep when it also carries a rule that acts on
//! the snapshots it keeps, end to end over HTTP.
//!
//! The rule under test: a schedule may retain NOTHING only while every other
//! rule it carries acts on creation alone. A schedule that specifies a rule
//! acting on the snapshots it RETAINS — fast restore, sharing, cross-Region copy
//! — must retain at least one of them, and a create or an update stating a
//! standard-tier retention of `0` beside such a rule is refused with
//! `InvalidRequestException` (HTTP 400, `Code` `InvalidParameter`) before
//! anything is stored or changed. Any other retention is accepted and read back.
//!
//! `RetainRule` states the floor for both retention modes
//! (`smithy_query(query="doc", shape="RetainRule")`):
//!
//! * count-based — "You must specify Count. If you specify an ArchiveRule for the
//!   schedule, then you can specify a retention count of `0` to archive snapshots
//!   immediately after creation. If you specify a FastRestoreRule, ShareRule, or
//!   a CrossRegionCopyRule, then you must specify a retention count of `1` or
//!   more."
//! * age-based — "You must specify Interval and IntervalUnit. If you specify an
//!   ArchiveRule for the schedule, then you can specify a retention interval of
//!   `0` days to archive snapshots immediately after creation. If you specify a
//!   FastRestoreRule, ShareRule, or a CrossRegionCopyRule, then you must specify
//!   a retention interval of `1` day or more."
//!
//! Neither floor is expressible in the model: `StandardTierRetainRuleCount` is
//! `@range(min: 0, max: 1000)` and `StandardTierRetainRuleInterval` is
//! `@range(min: 0)`, so `0` reaches the service.
//!
//! The tables span the class rather than the one request the issue cited: each of
//! the three rules the sentence names (and all three at once, and the empty list
//! that specifies none of them), both retention modes, every member of
//! `RetentionIntervalUnitValues`, the floor and the value one below it, the
//! ceiling of the count's range, both the first and a later element of
//! `Schedules`, and both the create and the update.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for.
const REGION: &str = "us-west-2";

/// The Region a cross-Region copy rule targets, which is not the one the request
/// is made in.
const COPY_TARGET_REGION: &str = "us-east-1";

/// The modelled client error this service answers for a refused parameter, its
/// `@httpError` status and its `Code`.
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";
const INVALID_REQUEST_STATUS: u16 = 400;
const INVALID_PARAMETER_CODE: &str = "InvalidParameter";

/// `RetentionIntervalUnitValues`, verbatim from the model
/// (`smithy_query(query="shape", shape="RetentionIntervalUnitValues")`). Every
/// entry is exercised at the floor and one below it; the issue cited only `DAYS`.
const RETENTION_INTERVAL_UNITS: [&str; 4] = ["DAYS", "WEEKS", "MONTHS", "YEARS"];

/// `IntervalUnitValues` offers `HOURS` alone
/// (`smithy_query(query="shape", shape="IntervalUnitValues")`).
const CREATE_INTERVAL_UNIT: &str = "HOURS";

/// A creation rule that creates one snapshot a month, the frequency a schedule
/// carrying an `ArchiveRule` must run at least as rarely as (see
/// [`create_rule_for`]).
const MONTHLY_CRON_EXPRESSION: &str = "cron(0 0 1 * ? *)";

/// The documented floor, and the value one below it — the only value of either
/// member that falls short of a floor stated in days, since every retention unit
/// is at least a day long.
const RETENTION_FLOOR: i64 = 1;
const NO_RETENTION: i64 = 0;

/// `StandardTierRetainRuleCount` is `@range(min: 0, max: 1000)`.
const RETAIN_COUNT_CEILING: i64 = 1000;

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
/// service MATERIALIZES on top of what a request stated (a copy rule's
/// `CopyTags`, a rule interval's zero) are left out of the comparison, since this
/// file is about the retention floor and not about what is stamped; a list a
/// request stated EMPTY specifies nothing, so a read reporting nothing for it
/// carries it.
fn carries(stored: &Value, stated: &Value) -> bool {
    match (stored, stated) {
        (Value::Object(stored), Value::Object(stated)) => stated.iter().all(|(member, value)| {
            stored
                .get(member)
                .is_some_and(|held| carries(held, value))
        }),
        (Value::Array(stored), Value::Array(stated)) => {
            stored.len() == stated.len()
                && stored
                    .iter()
                    .zip(stated)
                    .all(|(held, value)| carries(held, value))
        }
        (Value::Null, Value::Array(stated)) => stated.is_empty(),
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

/// A schedule the service accepts: the interval form of the creation rule and
/// count-based retention above the floor.
fn good_schedule(name: &str) -> Value {
    json!({
        "Name": name,
        "CreateRule": {"Interval": 24, "IntervalUnit": CREATE_INTERVAL_UNIT},
        "RetainRule": {"Count": 2},
    })
}

/// A complete custom EBS snapshot policy configuration carrying `schedules`.
fn snapshot_details(schedules: Vec<Value>) -> Value {
    json!({
        "PolicyType": "EBS_SNAPSHOT_MANAGEMENT",
        "ResourceTypes": ["VOLUME"],
        "TargetTags": [{"Key": "backup", "Value": "daily"}],
        "Schedules": schedules,
    })
}

/// The creation rule a schedule carrying `members` states.
///
/// The second bullet of `CreateRule`'s note — "If you need to specify an
/// ArchiveRule for the schedule, then you must specify a creation frequency of
/// at least 28 days" (`smithy_query(query="doc", shape="CreateRule")`) — cannot
/// be met by the interval form, whose only unit is `HOURS` and whose largest
/// supported interval is 24, so a schedule carrying an `ArchiveRule` states its
/// creation rule as a monthly cron expression instead. This file is about the
/// standard-tier retention floor; that creation floor is
/// `tests/all/archive_creation_frequency.rs`'s.
fn create_rule_for(members: &Value) -> Value {
    if members.get("ArchiveRule").is_some() {
        json!({"CronExpression": MONTHLY_CRON_EXPRESSION})
    } else {
        json!({"Interval": 24, "IntervalUnit": CREATE_INTERVAL_UNIT})
    }
}

/// A schedule carrying `retain_rule` and the members of `rule` that specify a
/// rule acting on the snapshots it retains.
fn schedule_with(name: &str, rule: &Value, retain_rule: &Value) -> Value {
    let mut schedule = json!({
        "Name": name,
        "CreateRule": create_rule_for(rule),
        "RetainRule": retain_rule.clone(),
    });
    for (member, value) in rule.as_object().expect("a rule is a JSON object") {
        schedule[member] = value.clone();
    }
    schedule
}

/// One set of schedule members that may raise the retention floor.
struct DependentRule {
    case: &'static str,
    /// The `Schedule` members that carry the rule
    /// (`smithy_query(query="shape", shape="Schedule")`: `FastRestoreRule`,
    /// `ShareRules`, `CrossRegionCopyRules`).
    members: Value,
    /// Whether these members SPECIFY such a rule at all.
    raises_floor: bool,
    /// The rule shapes a refusal must name.
    names: &'static [&'static str],
}

impl DependentRule {
    /// These members as a schedule retaining by `retain_rule` states them.
    ///
    /// An `ArchiveRule`'s `RetentionArchiveTier` retains in the mode the
    /// schedule's own retention rule states ("For count-based schedules, you
    /// must specify Count. For age-based schedules, you must specify Interval
    /// and IntervalUnit", `smithy_query(query="doc",
    /// shape="RetentionArchiveTier")`), so beside an age-based retention the
    /// archive tier states its own age-based form: this file is about the
    /// standard-tier floor, and the form each rule is stated in is
    /// `tests/all/schedule_rule_forms.rs`'s.
    fn members_beside(&self, retain_rule: &Value) -> Value {
        let mut members = self.members.clone();
        let age_based = retain_rule.get("IntervalUnit").is_some();
        if members.get("ArchiveRule").is_some() && age_based {
            members["ArchiveRule"] = json!({
                "RetainRule": {
                    "RetentionArchiveTier": {"Interval": 90, "IntervalUnit": "DAYS"},
                },
            });
        }
        members
    }
}

/// Every way a schedule can specify a rule acting on the snapshots it retains,
/// one row per rule the documented sentence names, all three at once, the empty
/// lists that specify none of them, and a schedule carrying none.
fn dependent_rules() -> Vec<DependentRule> {
    let share_rules = json!([{
        "TargetAccounts": ["111122223333"],
        "UnshareInterval": 1,
        "UnshareIntervalUnit": "DAYS",
    }]);
    let copy_rules = json!([{"Target": COPY_TARGET_REGION, "Encrypted": false}]);
    vec![
        DependentRule {
            case: "no rule acting on retained snapshots",
            members: json!({}),
            raises_floor: false,
            names: &[],
        },
        DependentRule {
            case: "an ArchiveRule, which acts on retained snapshots of its own",
            // The sentence that admits a retention of 0: "If you specify an
            // ArchiveRule for the schedule, then you can specify a retention
            // count of 0 to archive snapshots immediately after creation."
            members: json!({
                "ArchiveRule": {
                    "RetainRule": {"RetentionArchiveTier": {"Count": 90}},
                },
            }),
            raises_floor: false,
            names: &[],
        },
        DependentRule {
            case: "empty ShareRules and CrossRegionCopyRules lists",
            // Both members are `@length(min: 0, ...)`: an empty list specifies no
            // rule, so it raises no floor.
            members: json!({"ShareRules": [], "CrossRegionCopyRules": []}),
            raises_floor: false,
            names: &[],
        },
        DependentRule {
            case: "a count-based FastRestoreRule",
            members: json!({
                "FastRestoreRule": {"Count": 1, "AvailabilityZones": ["us-west-2a"]},
            }),
            raises_floor: true,
            names: &["FastRestoreRule"],
        },
        DependentRule {
            case: "an age-based FastRestoreRule",
            members: json!({
                "FastRestoreRule": {
                    "Interval": 3,
                    "IntervalUnit": "DAYS",
                    "AvailabilityZoneIds": ["usw2-az1"],
                },
            }),
            raises_floor: true,
            names: &["FastRestoreRule"],
        },
        DependentRule {
            case: "a ShareRule",
            members: json!({"ShareRules": share_rules.clone()}),
            raises_floor: true,
            names: &["ShareRule"],
        },
        DependentRule {
            case: "a CrossRegionCopyRule",
            // The request the issue reproduced.
            members: json!({"CrossRegionCopyRules": copy_rules.clone()}),
            raises_floor: true,
            names: &["CrossRegionCopyRule"],
        },
        DependentRule {
            case: "two CrossRegionCopyRules",
            members: json!({
                "CrossRegionCopyRules": [
                    {"Target": COPY_TARGET_REGION, "Encrypted": false},
                    {"Target": "eu-west-1", "Encrypted": true},
                ],
            }),
            raises_floor: true,
            names: &["CrossRegionCopyRule"],
        },
        DependentRule {
            case: "all three rules at once",
            members: json!({
                "FastRestoreRule": {"Count": 1, "AvailabilityZones": ["us-west-2b"]},
                "ShareRules": share_rules,
                "CrossRegionCopyRules": copy_rules,
            }),
            raises_floor: true,
            names: &["FastRestoreRule", "ShareRule", "CrossRegionCopyRule"],
        },
    ]
}

/// One standard-tier retention a schedule may state.
struct Retention {
    case: String,
    /// The `RetainRule` as a request states it.
    rule: Value,
    /// The member the floor is judged on, as a refusal names it.
    member: &'static str,
    /// How much it retains: `0` retains nothing.
    retained: i64,
    /// The JSON pointer, relative to the stored schedule, a read reports the
    /// retention at.
    stored_at: String,
}

/// Every retention the two documented modes can state around the floor: the
/// count at the value below the floor, at the floor, above it and at the ceiling
/// of its range, and the interval below and at the floor in every retention unit.
fn retentions() -> Vec<Retention> {
    let mut rows = vec![
        Retention {
            case: format!("a retention count of {NO_RETENTION}"),
            rule: json!({"Count": NO_RETENTION}),
            member: "Count",
            retained: NO_RETENTION,
            stored_at: "/RetainRule/Count".to_string(),
        },
        Retention {
            case: format!("a retention count of {RETENTION_FLOOR}, at the floor"),
            rule: json!({"Count": RETENTION_FLOOR}),
            member: "Count",
            retained: RETENTION_FLOOR,
            stored_at: "/RetainRule/Count".to_string(),
        },
        Retention {
            case: "a retention count of 7".to_string(),
            rule: json!({"Count": 7}),
            member: "Count",
            retained: 7,
            stored_at: "/RetainRule/Count".to_string(),
        },
        Retention {
            case: format!("a retention count of {RETAIN_COUNT_CEILING}, at the ceiling"),
            rule: json!({"Count": RETAIN_COUNT_CEILING}),
            member: "Count",
            retained: RETAIN_COUNT_CEILING,
            stored_at: "/RetainRule/Count".to_string(),
        },
    ];
    for unit in RETENTION_INTERVAL_UNITS {
        for retained in [NO_RETENTION, RETENTION_FLOOR] {
            rows.push(Retention {
                case: format!("a retention interval of {retained} {unit}"),
                rule: json!({"Interval": retained, "IntervalUnit": unit}),
                member: "Interval",
                retained,
                stored_at: "/RetainRule/Interval".to_string(),
            });
        }
    }
    rows
}

/// Assert `answer` is this service's modelled refusal of a retention below the
/// floor: the status and `Code` of `InvalidRequestException`, a message naming
/// the schedule, the member, the value it stated and the rules that raised the
/// floor, and neither structured parameter list (each names the request's own
/// parameters, and this rule is about a structure nested inside one of them).
fn assert_floor_refusal(
    case: &str,
    answer: &Answer,
    schedule: &str,
    member: &str,
    retained: i64,
    names: &[&str],
) {
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
    for expected in [
        format!("RetainRule.{member}"),
        format!("{{{schedule}}}"),
        retained.to_string(),
        format!("{RETENTION_FLOOR} "),
    ] {
        assert!(
            message.contains(&expected),
            "case {case:?}: the refusal must name {expected:?}, got {message:?}"
        );
    }
    for name in names {
        assert!(
            message.contains(name),
            "case {case:?}: the refusal must name the {name} that raised the floor, got \
             {message:?}"
        );
    }
    for list in ["RequiredParameters", "MutuallyExclusiveParameters"] {
        let reported = at(&answer.body, &format!("/{list}"));
        assert_eq!(
            reported,
            Value::Null,
            "case {case:?}: {list} must be absent, got {reported:?}"
        );
    }
}

/// RULE — a schedule specifying a rule that acts on the snapshots it retains
/// must retain at least one of them; any other schedule may retain none.
///
/// Every rule row is crossed with every retention row, over the create: the
/// combination is refused exactly when a rule acting on retained snapshots is
/// specified beside a retention of nothing, and an accepted configuration is read
/// back carrying the retention and the rule it stated.
#[tokio::test]
async fn a_rule_acting_on_retained_snapshots_requires_a_retention_of_one_or_more() {
    let router = emulator();
    for rule in dependent_rules() {
        for retention in retentions() {
            let case = format!("{} beside {}", retention.case, rule.case);
            let schedule_name = "under-test";
            let members = rule.members_beside(&retention.rule);
            let details = snapshot_details(vec![schedule_with(
                schedule_name,
                &members,
                &retention.rule,
            )]);
            let refused = rule.raises_floor && retention.retained < RETENTION_FLOOR;
            let before = policy_count(&router).await;
            let answer = create(&router, create_request(&case, details)).await;

            if refused {
                assert_floor_refusal(
                    &case,
                    &answer,
                    schedule_name,
                    retention.member,
                    retention.retained,
                    rule.names,
                );
                assert_eq!(
                    policy_count(&router).await,
                    before,
                    "case {case:?}: a refused create must store no policy"
                );
                continue;
            }

            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200, got {} with {:?}",
                answer.status, answer.body
            );
            let stored = at(
                get(&router, &answer.policy_id()).await.policy(),
                "/PolicyDetails/Schedules/0",
            );
            assert_eq!(
                at(&stored, &retention.stored_at),
                json!(retention.retained),
                "case {case:?}: the stored retention must be the one that was stated, got {:?}",
                at(&stored, &retention.stored_at)
            );
            assert!(
                carries(&stored, &members),
                "case {case:?}: the stored schedule must carry the rule it stated ({:?}), got \
                 {stored:?}",
                members
            );
        }
    }
}

/// The floor is judged on EVERY element of `Schedules`, not just the first, and
/// the refusal names the schedule that carries the fault.
#[tokio::test]
async fn the_floor_is_judged_on_every_schedule() {
    let router = emulator();
    let offenders: Vec<(&str, Value)> = vec![
        (
            "a retention count of 0 beside a CrossRegionCopyRule",
            json!({
                "RetainRule": {"Count": NO_RETENTION},
                "CrossRegionCopyRules": [{"Target": COPY_TARGET_REGION, "Encrypted": false}],
            }),
        ),
        (
            "a retention interval of 0 DAYS beside a ShareRule",
            json!({
                "RetainRule": {"Interval": NO_RETENTION, "IntervalUnit": "DAYS"},
                "ShareRules": [{"TargetAccounts": ["111122223333"]}],
            }),
        ),
    ];

    for (what, members) in offenders {
        // `Schedules` is `@length(min: 1, max: 4)`, so the offending schedule is
        // placed at each position a policy can hold.
        for position in 1..=4usize {
            let case = format!("{what} at position {position} of {position}");
            let mut schedules: Vec<Value> = (1..position)
                .map(|index| good_schedule(&format!("leading-{index}")))
                .collect();
            let mut offender = json!({
                "Name": "under-test",
                "CreateRule": {"Interval": 24, "IntervalUnit": CREATE_INTERVAL_UNIT},
            });
            for (member, value) in members.as_object().expect("a schedule is a JSON object") {
                offender[member] = value.clone();
            }
            schedules.push(offender);

            let before = policy_count(&router).await;
            let answer = create(
                &router,
                create_request(&case, snapshot_details(schedules)),
            )
            .await;
            assert_eq!(
                answer.status, INVALID_REQUEST_STATUS,
                "case {case:?}: expected HTTP {INVALID_REQUEST_STATUS}, got {} with {:?}",
                answer.status, answer.body
            );
            assert!(
                answer.message().contains("{under-test}"),
                "case {case:?}: the refusal must name the offending schedule, got {:?}",
                answer.message()
            );
            assert!(
                !answer.message().contains("leading-"),
                "case {case:?}: the refusal must not blame a schedule that retains, got {:?}",
                answer.message()
            );
            assert_eq!(
                policy_count(&router).await,
                before,
                "case {case:?}: a refused create must store no policy"
            );
        }
    }
}

/// The update path applies the same floor, and a refused update leaves the
/// stored configuration exactly as it was.
#[tokio::test]
async fn an_update_cannot_patch_a_policy_below_the_floor() {
    let router = emulator();
    let created = create(
        &router,
        create_request(
            "retention floor subject",
            snapshot_details(vec![good_schedule("daily")]),
        ),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "the subject policy must be created, got {} with {:?}",
        created.status, created.body
    );
    let id = created.policy_id();
    let stored = at(get(&router, &id).await.policy(), "/PolicyDetails");

    let rules = dependent_rules();
    for rule in &rules {
        for retention in retentions() {
            let case = format!("{} beside {} (update)", retention.case, rule.case);
            let details = snapshot_details(vec![schedule_with(
                "under-test",
                &rule.members_beside(&retention.rule),
                &retention.rule,
            )]);
            let refused = rule.raises_floor && retention.retained < RETENTION_FLOOR;
            let answer = update(&router, &id, json!({"PolicyDetails": details})).await;

            if refused {
                assert_floor_refusal(
                    &case,
                    &answer,
                    "under-test",
                    retention.member,
                    retention.retained,
                    rule.names,
                );
                assert_eq!(
                    at(get(&router, &id).await.policy(), "/PolicyDetails"),
                    stored,
                    "case {case:?}: a refused update must leave the stored configuration alone"
                );
                continue;
            }

            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200, got {} with {:?}",
                answer.status, answer.body
            );
            let read = at(
                get(&router, &id).await.policy(),
                "/PolicyDetails/Schedules/0",
            );
            assert_eq!(
                at(&read, &retention.stored_at),
                json!(retention.retained),
                "case {case:?}: an accepted update must store its retention, got {:?}",
                at(&read, &retention.stored_at)
            );
            // Restore the accepted configuration, so the next refused row
            // compares against a known state.
            let restored = update(
                &router,
                &id,
                json!({"PolicyDetails": snapshot_details(vec![good_schedule("daily")])}),
            )
            .await;
            assert_eq!(
                restored.status, 200,
                "case {case:?}: the subject policy must be restored, got {} with {:?}",
                restored.status, restored.body
            );
        }
    }
}

/// The configuration this service REPORTS for a schedule carrying one of these
/// rules is one it accepts back: the retention it materializes never falls below
/// the floor the rule raises.
#[tokio::test]
async fn a_reported_configuration_is_accepted_again() {
    let router = emulator();
    let retentions: Vec<(&str, Value)> = vec![
        ("count-based retention at the floor", json!({"Count": 1})),
        (
            "age-based retention at the floor",
            json!({"Interval": 1, "IntervalUnit": "DAYS"}),
        ),
    ];

    for (case, retain_rule) in retentions {
        let details = snapshot_details(vec![schedule_with(
            "round-trip",
            &json!({"CrossRegionCopyRules": [{"Target": COPY_TARGET_REGION, "Encrypted": false}]}),
            &retain_rule,
        )]);
        let created = create(&router, create_request(case, details)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            created.status, created.body
        );
        let id = created.policy_id();
        let reported = at(get(&router, &id).await.policy(), "/PolicyDetails");

        let answer = update(&router, &id, json!({"PolicyDetails": reported.clone()})).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the configuration the service reports must be one it accepts, got \
             {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            at(get(&router, &id).await.policy(), "/PolicyDetails"),
            reported,
            "case {case:?}: re-sending the reported configuration must leave it unchanged"
        );
    }
}
