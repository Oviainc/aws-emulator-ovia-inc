// SPDX-License-Identifier: CC0-1.0

//! Whether a schedule must SPECIFY a retention at all, end to end over HTTP.
//!
//! The rule under test: a schedule that specifies a rule acting on the snapshots
//! it retains — fast restore, sharing, cross-Region copy — must specify a
//! retention of one or more, so such a schedule stating NO retention rule is
//! refused with `InvalidRequestException` (HTTP 400, `Code` `InvalidParameter`)
//! exactly as one stating a retention of nothing is, before anything is stored
//! or changed. A schedule carrying none of those rules may state no retention
//! rule, and is accepted and read back as it was stated.
//!
//! `RetainRule` states the requirement for both retention modes
//! (`smithy_query(query="doc", shape="RetainRule")`):
//!
//! * count-based — "You must specify Count. ... If you specify a
//!   FastRestoreRule, ShareRule, or a CrossRegionCopyRule, then you must specify
//!   a retention count of `1` or more."
//! * age-based — "You must specify Interval and IntervalUnit. ... If you specify
//!   a FastRestoreRule, ShareRule, or a CrossRegionCopyRule, then you must
//!   specify a retention interval of `1` day or more."
//!
//! A schedule that omits the `RetainRule` member specifies neither a retention
//! count nor a retention interval, so it satisfies neither sentence. The member
//! is not `@required` in the model — `Schedule$RetainRule` is an optional
//! structure — so the omission reaches the service and the requirement is
//! conditional on the other rules the same schedule carries.
//!
//! The tables span the class rather than the one request the issue cited: each of
//! the three rules the sentence names, each form a rule can take, all three at
//! once, the empty lists and the `ArchiveRule` that name none of them, every
//! position of `Schedules`, both the create and the update, and — beside the
//! omitted member — every other way a schedule can fail or meet the requirement
//! (a retention of zero in either mode, and a retention at the floor in either
//! mode).

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

/// `IntervalUnitValues` offers `HOURS` alone
/// (`smithy_query(query="shape", shape="IntervalUnitValues")`).
const CREATE_INTERVAL_UNIT: &str = "HOURS";

/// A creation rule that creates one snapshot a month, the frequency a schedule
/// carrying an `ArchiveRule` must run at least as rarely as (see
/// [`create_rule_for`]).
const MONTHLY_CRON_EXPRESSION: &str = "cron(0 0 1 * ? *)";

/// The documented floor on a retention stated beside one of those rules, and the
/// retention that falls short of it.
const RETENTION_FLOOR: i64 = 1;
const NO_RETENTION: i64 = 0;

/// `Schedules` is `@length(min: 1, max: 4)`
/// (`smithy_query(query="shape", shape="PolicyDetails")`).
const MAX_SCHEDULES: usize = 4;

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
/// service MATERIALIZES on top of what a request stated are left out of the
/// comparison, since this file is about whether a retention is required; a list
/// a request stated EMPTY specifies nothing, so a read reporting nothing for it
/// carries it.
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
/// creation rule as a monthly cron expression instead. This file is about when a
/// retention is mandatory; that creation floor is
/// `tests/all/archive_creation_frequency.rs`'s.
fn create_rule_for(members: &Value) -> Value {
    if members.get("ArchiveRule").is_some() {
        json!({"CronExpression": MONTHLY_CRON_EXPRESSION})
    } else {
        json!({"Interval": 24, "IntervalUnit": CREATE_INTERVAL_UNIT})
    }
}

/// A schedule named `name` carrying the members of `rule`, plus `retain_rule`
/// when there is one to state.
fn schedule_with(name: &str, rule: &Value, retain_rule: Option<&Value>) -> Value {
    let mut schedule = json!({
        "Name": name,
        "CreateRule": create_rule_for(rule),
    });
    if let Some(retain_rule) = retain_rule {
        schedule["RetainRule"] = retain_rule.clone();
    }
    for (member, value) in rule.as_object().expect("a rule is a JSON object") {
        schedule[member] = value.clone();
    }
    schedule
}

/// One set of schedule members that may make a retention mandatory.
struct DependentRule {
    case: &'static str,
    /// The `Schedule` members that carry the rule
    /// (`smithy_query(query="shape", shape="Schedule")`: `FastRestoreRule`,
    /// `ShareRules`, `CrossRegionCopyRules`).
    members: Value,
    /// The rule shapes a refusal must name, empty when these members specify
    /// none of the three rules and so require no retention.
    names: &'static [&'static str],
}

impl DependentRule {
    /// Whether a schedule carrying these members must specify a retention.
    fn requires_retention(&self) -> bool {
        !self.names.is_empty()
    }
}

/// A `ShareRule` as a request states it, carried by `Schedule$ShareRules`
/// (`@length(min: 0, max: 1)`).
fn share_rules() -> Value {
    json!([{
        "TargetAccounts": ["111122223333"],
        "UnshareInterval": 1,
        "UnshareIntervalUnit": "DAYS",
    }])
}

/// One `CrossRegionCopyRule`, carried by `Schedule$CrossRegionCopyRules`
/// (`@length(min: 0, max: 3)`).
fn copy_rules() -> Value {
    json!([{"Target": COPY_TARGET_REGION, "Encrypted": false}])
}

/// Every way a schedule can specify a rule acting on the snapshots it retains,
/// one row per rule the documented sentence names and per form that rule can
/// take, all three at once, and the configurations that name none of them.
fn dependent_rules() -> Vec<DependentRule> {
    vec![
        DependentRule {
            case: "no rule acting on retained snapshots",
            members: json!({}),
            names: &[],
        },
        DependentRule {
            case: "an ArchiveRule, which the sentence does not name",
            // The sentence that admits retaining nothing: "If you specify an
            // ArchiveRule for the schedule, then you can specify a retention
            // count of 0 to archive snapshots immediately after creation."
            members: json!({
                "ArchiveRule": {
                    "RetainRule": {"RetentionArchiveTier": {"Count": 90}},
                },
            }),
            names: &[],
        },
        DependentRule {
            case: "empty ShareRules and CrossRegionCopyRules lists",
            // Both members are `@length(min: 0, ...)`: an empty list specifies
            // no rule, so it requires no retention.
            members: json!({"ShareRules": [], "CrossRegionCopyRules": []}),
            names: &[],
        },
        DependentRule {
            case: "a count-based FastRestoreRule",
            members: json!({
                "FastRestoreRule": {"Count": 1, "AvailabilityZones": ["us-west-2a"]},
            }),
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
            names: &["FastRestoreRule"],
        },
        DependentRule {
            case: "a FastRestoreRule beside lists specifying no other rule",
            members: json!({
                "FastRestoreRule": {"Count": 1, "AvailabilityZones": ["us-west-2a"]},
                "ShareRules": [],
                "CrossRegionCopyRules": [],
            }),
            names: &["FastRestoreRule"],
        },
        DependentRule {
            case: "a ShareRule",
            members: json!({"ShareRules": [{
                "TargetAccounts": ["111122223333"],
                "UnshareInterval": 1,
                "UnshareIntervalUnit": "DAYS",
            }]}),
            names: &["ShareRule"],
        },
        DependentRule {
            case: "a ShareRule stating target accounts alone",
            members: json!({"ShareRules": [{"TargetAccounts": ["111122223333"]}]}),
            names: &["ShareRule"],
        },
        DependentRule {
            case: "a CrossRegionCopyRule",
            // The request the issue reproduced, with the RetainRule deleted.
            members: json!({"CrossRegionCopyRules": [{
                "Target": COPY_TARGET_REGION,
                "Encrypted": false,
            }]}),
            names: &["CrossRegionCopyRule"],
        },
        DependentRule {
            case: "three CrossRegionCopyRules, the most the list may carry",
            members: json!({
                "CrossRegionCopyRules": [
                    {"Target": COPY_TARGET_REGION, "Encrypted": false},
                    {"Target": "eu-west-1", "Encrypted": true},
                    {"Target": "ap-south-1", "Encrypted": false},
                ],
            }),
            names: &["CrossRegionCopyRule"],
        },
        DependentRule {
            case: "a ShareRule beside a CrossRegionCopyRule",
            members: json!({
                "ShareRules": share_rules(),
                "CrossRegionCopyRules": copy_rules(),
            }),
            names: &["ShareRule", "CrossRegionCopyRule"],
        },
        DependentRule {
            case: "all three rules at once",
            members: json!({
                "FastRestoreRule": {"Count": 1, "AvailabilityZones": ["us-west-2b"]},
                "ShareRules": share_rules(),
                "CrossRegionCopyRules": copy_rules(),
            }),
            names: &["FastRestoreRule", "ShareRule", "CrossRegionCopyRule"],
        },
    ]
}

/// Assert `answer` is this service's modelled refusal of a schedule that
/// specifies no retention: the status and `Code` of `InvalidRequestException`, a
/// message naming the omitted member, the schedule, the floor and every rule
/// that requires a retention, the omitted member reported in
/// `RequiredParameters`, and no `MutuallyExclusiveParameters` (that list names
/// request parameters stated together, which is a different class of refusal).
fn assert_retention_required(answer: &Answer, case: &str, position: usize, names: &[&str]) {
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

    let omitted = format!("PolicyDetails.Schedules[{position}].RetainRule");
    assert_eq!(
        at(&answer.body, "/RequiredParameters"),
        json!([omitted]),
        "case {case:?}: RequiredParameters must name {omitted:?}, got {:?}",
        at(&answer.body, "/RequiredParameters")
    );
    assert_eq!(
        at(&answer.body, "/MutuallyExclusiveParameters"),
        Value::Null,
        "case {case:?}: MutuallyExclusiveParameters must be absent, got {:?}",
        at(&answer.body, "/MutuallyExclusiveParameters")
    );

    let message = answer.message();
    for expected in [
        omitted,
        format!("{{{SUBJECT_SCHEDULE}}}"),
        format!("{RETENTION_FLOOR} or more"),
    ] {
        assert!(
            message.contains(&expected),
            "case {case:?}: the refusal must name {expected:?}, got {message:?}"
        );
    }
    for name in names {
        assert!(
            message.contains(name),
            "case {case:?}: the refusal must name the {name} that requires a retention, got \
             {message:?}"
        );
    }
}

/// RULE — a schedule specifying a rule that acts on the snapshots it retains
/// must specify a retention; any other schedule may specify none.
///
/// Every rule row is driven through the create with the `RetainRule` member left
/// out entirely: the configuration is refused exactly when one of the three
/// documented rules is specified, and an accepted one is read back carrying the
/// rule it stated and no retention rule.
#[tokio::test]
async fn a_rule_acting_on_retained_snapshots_requires_a_stated_retention() {
    let router = emulator();
    for rule in dependent_rules() {
        let case = format!("no RetainRule beside {}", rule.case);
        let details = snapshot_details(vec![schedule_with(
            SUBJECT_SCHEDULE,
            &rule.members,
            None,
        )]);
        let before = policy_count(&router).await;
        let answer = create(&router, create_request(&case, details)).await;

        if rule.requires_retention() {
            assert_retention_required(&answer, &case, 1, rule.names);
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
        assert!(
            carries(&stored, &rule.members),
            "case {case:?}: the stored schedule must carry the rule it stated ({:?}), got \
             {stored:?}",
            rule.members
        );
        assert_eq!(
            at(&stored, "/RetainRule"),
            Value::Null,
            "case {case:?}: a schedule stating no retention rule must be read back without one, \
             got {:?}",
            at(&stored, "/RetainRule")
        );
    }
}

/// The requirement is judged on EVERY element of `Schedules`, not just the
/// first, and the refusal names the schedule and the member path that carries
/// the fault.
#[tokio::test]
async fn the_requirement_is_judged_on_every_schedule() {
    let router = emulator();
    let offenders: Vec<(&str, Value, &[&str])> = vec![
        (
            "a CrossRegionCopyRule",
            json!({"CrossRegionCopyRules": copy_rules()}),
            &["CrossRegionCopyRule"],
        ),
        (
            "a ShareRule",
            json!({"ShareRules": share_rules()}),
            &["ShareRule"],
        ),
        (
            "a FastRestoreRule",
            json!({"FastRestoreRule": {"Count": 1, "AvailabilityZones": ["us-west-2a"]}}),
            &["FastRestoreRule"],
        ),
    ];

    for (what, members, names) in offenders {
        for position in 1..=MAX_SCHEDULES {
            let case = format!("no RetainRule beside {what} at position {position}");
            let mut schedules: Vec<Value> = (1..position)
                .map(|index| good_schedule(&format!("leading-{index}")))
                .collect();
            schedules.push(schedule_with(SUBJECT_SCHEDULE, &members, None));

            let before = policy_count(&router).await;
            let answer = create(&router, create_request(&case, snapshot_details(schedules))).await;
            assert_retention_required(&answer, &case, position, names);
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

/// The update path applies the same requirement, so a valid policy cannot be
/// patched into a schedule that copies, shares or fast-restores snapshots it
/// specifies no retention for; a refused update leaves the stored configuration
/// exactly as it was.
#[tokio::test]
async fn an_update_cannot_drop_a_required_retention() {
    let router = emulator();
    let created = create(
        &router,
        create_request(
            "retention requirement subject",
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

    for rule in dependent_rules() {
        let case = format!("no RetainRule beside {} (update)", rule.case);
        let details = snapshot_details(vec![schedule_with(
            SUBJECT_SCHEDULE,
            &rule.members,
            None,
        )]);
        let answer = update(&router, &id, json!({"PolicyDetails": details})).await;

        if rule.requires_retention() {
            assert_retention_required(&answer, &case, 1, rule.names);
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
        // Restore the accepted configuration, so the next refused row compares
        // against a known state.
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

/// Every way of specifying no retention is refused beside such a rule, and every
/// way of specifying one of `1` or more is accepted: the omitted member is
/// refused exactly like a retention of zero, so the requirement cannot be evaded
/// by deleting the member the documentation requires.
#[tokio::test]
async fn omitting_the_retention_is_refused_like_a_retention_of_zero() {
    let router = emulator();
    let retentions: Vec<(String, Option<Value>, bool)> = vec![
        ("no RetainRule at all".to_string(), None, false),
        (
            format!("a retention count of {NO_RETENTION}"),
            Some(json!({"Count": NO_RETENTION})),
            false,
        ),
        (
            format!("a retention interval of {NO_RETENTION} DAYS"),
            Some(json!({"Interval": NO_RETENTION, "IntervalUnit": "DAYS"})),
            false,
        ),
        (
            format!("a retention count of {RETENTION_FLOOR}"),
            Some(json!({"Count": RETENTION_FLOOR})),
            true,
        ),
        (
            format!("a retention interval of {RETENTION_FLOOR} DAYS"),
            Some(json!({"Interval": RETENTION_FLOOR, "IntervalUnit": "DAYS"})),
            true,
        ),
    ];

    for (what, retain_rule, accepted) in retentions {
        let case = format!("{what} beside a CrossRegionCopyRule");
        let details = snapshot_details(vec![schedule_with(
            SUBJECT_SCHEDULE,
            &json!({"CrossRegionCopyRules": copy_rules()}),
            retain_rule.as_ref(),
        )]);
        let before = policy_count(&router).await;
        let answer = create(&router, create_request(&case, details)).await;

        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200, got {} with {:?}",
                answer.status, answer.body
            );
            let stated = retain_rule
                .as_ref()
                .expect("an accepted row states a retention");
            let stored = at(
                get(&router, &answer.policy_id()).await.policy(),
                "/PolicyDetails/Schedules/0/RetainRule",
            );
            assert!(
                carries(&stored, stated),
                "case {case:?}: the stored schedule must carry the retention it stated \
                 ({stated:?}), got {stored:?}"
            );
            continue;
        }

        assert_eq!(
            answer.status, INVALID_REQUEST_STATUS,
            "case {case:?}: expected HTTP {INVALID_REQUEST_STATUS}, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.message().contains("CrossRegionCopyRule")
                && answer.message().contains(&format!("{{{SUBJECT_SCHEDULE}}}")),
            "case {case:?}: the refusal must name the schedule and the rule that requires a \
             retention, got {:?}",
            answer.message()
        );
        assert_eq!(
            policy_count(&router).await,
            before,
            "case {case:?}: a refused create must store no policy"
        );
    }
}
