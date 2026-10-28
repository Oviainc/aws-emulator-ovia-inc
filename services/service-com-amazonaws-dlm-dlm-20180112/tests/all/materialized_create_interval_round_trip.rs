// SPDX-License-Identifier: CC0-1.0

//! The creation interval this service MATERIALIZES onto a stored schedule, and
//! what the two schedule checks make of it, end to end over HTTP.
//!
//! The rule under test: the interval this service itself fills in for a creation
//! rule that stated none is not a value the request asked for, so no check reads
//! it as one. A read of a policy whose schedule is cron-driven reports
//! `CreateRule.Interval` as `0`, the value the service materializes, so that `0`
//! is a value this API's own responses carry: it neither states the interval form
//! of the either/or creation rule nor is judged against the supported interval
//! set, and the configuration a read REPORTS is one `CreateLifecyclePolicy` and
//! `UpdateLifecyclePolicy` accept back unchanged — the ordinary read-modify-write
//! flow — for a cron-driven schedule as much as an interval-driven one.
//!
//! What a request DID state is judged by the domain the member's documentation
//! publishes rather than by the weaker modelled bound: an interval beside a cron
//! expression states both documented forms and is refused, a stated interval
//! outside the supported set is refused whichever side of the set it falls, and
//! the interval form missing its interval is refused.
//!
//! The documented forms, from `smithy_query(query="doc", shape="CreateRule")`:
//! "You must specify either CronExpression, or Interval, IntervalUnit, and
//! Times", and `CreateRule$Interval` is "the interval between snapshots. The
//! supported values are 1, 2, 3, 4, 6, 8, 12, and 24".
//!
//! The tables span the class rather than the one request the issue cited: both
//! creation forms, several cron expressions (including one at the floor of
//! `CronExpression`'s `@length`), both custom policy types (an AMI policy's
//! creation rule carries no materialized `Location`), both retention modes, a
//! schedule carrying no retention rule at all, a schedule carrying a rule whose
//! own members are materialized too, the first and a later element of
//! `Schedules`, every value of the supported interval set, values inside, above
//! and below it, the materialized filler stated alone and beside an interval
//! unit, and all three of the create, the read-back update and a brand-new policy
//! cloned from the reported configuration.

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

/// The interval a read reports for a creation rule that stated none: the value
/// the service materializes, which `Interval`'s `@range(min: 1)` keeps a request
/// from ever stating.
const MATERIALIZED_INTERVAL: i64 = 0;

/// The values a creation interval supports: `CreateRule$Interval` is "the
/// interval between snapshots. The supported values are 1, 2, 3, 4, 6, 8, 12,
/// and 24" (`smithy_query(query="doc", shape="CreateRule", member="Interval")`).
const SUPPORTED_CREATE_INTERVALS: [i64; 8] = [1, 2, 3, 4, 6, 8, 12, 24];

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

fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// A create request carrying `configuration` on top of the three members the
/// model marks `@required`. `description` is spelled in the characters
/// `PolicyDescription`'s `@pattern(^[0-9A-Za-z _-]+$)` admits, so a case name is
/// never what gets refused.
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
        "Description": if spelled.is_empty() { "a policy".to_string() } else { spelled },
        "State": "ENABLED",
        "PolicyDetails": configuration,
    })
}

/// A custom policy configuration of `policy_type`, targeting `resource_type`, and
/// carrying `schedules`.
fn custom_details(policy_type: &str, resource_type: &str, schedules: Value) -> Value {
    json!({
        "PolicyType": policy_type,
        "ResourceTypes": [resource_type],
        "TargetTags": [{"Key": "backup", "Value": "nightly"}],
        "Schedules": schedules,
    })
}

/// One configuration whose reported form must be acceptable back.
struct RoundTrip {
    case: &'static str,
    details: Value,
}

/// Assert `answer` is a success.
fn assert_accepted(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {} with {:?}",
        answer.status, answer.body
    );
}

/// The interval a read must report for the creation rule `sent_rule` states:
/// the one it stated, or the materialized filler when it stated none.
fn expected_reported_interval(sent_rule: &Value) -> Value {
    match at(sent_rule, "/Interval") {
        Value::Null => json!(MATERIALIZED_INTERVAL),
        stated => stated,
    }
}

/// RULE — the configuration a read REPORTS is one the create and the update
/// accept back unchanged, whichever documented form each of its creation rules
/// states: the interval the service materializes for a rule that stated none is
/// not a value the request asked for.
#[tokio::test]
async fn a_reported_configuration_is_accepted_back_whatever_form_its_create_rule_states() {
    let router = emulator();
    let cases = vec![
        RoundTrip {
            // The configuration the issue reproduced.
            case: "a cron-driven snapshot schedule, count-based retention",
            details: custom_details(
                "EBS_SNAPSHOT_MANAGEMENT",
                "VOLUME",
                json!([{
                    "Name": "round-trip",
                    "CreateRule": {"CronExpression": "cron(0 12 * * ? *)"},
                    "RetainRule": {"Count": 7},
                }]),
            ),
        },
        RoundTrip {
            // An AMI policy's creation rule carries no materialized `Location`:
            // the member is `[Custom snapshot policies only]`.
            case: "a cron-driven AMI schedule, age-based retention",
            details: custom_details(
                "IMAGE_MANAGEMENT",
                "INSTANCE",
                json!([{
                    "Name": "ami-round-trip",
                    "CreateRule": {"CronExpression": "cron(30 2 ? * MON *)"},
                    "RetainRule": {"Interval": 2, "IntervalUnit": "WEEKS"},
                }]),
            ),
        },
        RoundTrip {
            // `CronExpression` is `@length(min: 17, max: 106)`, so this is the
            // shortest expression the model admits.
            case: "a cron expression at the floor of its length",
            details: custom_details(
                "EBS_SNAPSHOT_MANAGEMENT",
                "VOLUME",
                json!([{
                    "Name": "shortest-cron",
                    "CreateRule": {"CronExpression": "cron(0 0 1 * ? *)"},
                    "RetainRule": {"Count": 1},
                }]),
            ),
        },
        RoundTrip {
            case: "a cron-driven schedule carrying no retention rule at all",
            details: custom_details(
                "EBS_SNAPSHOT_MANAGEMENT",
                "VOLUME",
                json!([{
                    "Name": "no-retention",
                    "CreateRule": {"CronExpression": "cron(0 3 * * ? *)"},
                }]),
            ),
        },
        RoundTrip {
            // A cross-Region copy rule has materialized members of its own, and
            // raises the floor on how much the schedule may retain.
            case: "a cron-driven schedule with a cross-Region copy rule",
            details: custom_details(
                "EBS_SNAPSHOT_MANAGEMENT",
                "VOLUME",
                json!([{
                    "Name": "copied",
                    "CreateRule": {"CronExpression": "cron(15 4 * * ? *)"},
                    "RetainRule": {"Count": 3},
                    "CrossRegionCopyRules": [{"Target": COPY_TARGET_REGION, "Encrypted": false}],
                }]),
            ),
        },
        RoundTrip {
            // A LATER element of `Schedules` (`@length(min: 1, max: 4)`), and
            // both creation forms inside one configuration.
            case: "an interval-driven schedule beside a later cron-driven one",
            details: custom_details(
                "EBS_SNAPSHOT_MANAGEMENT",
                "VOLUME",
                json!([
                    {
                        "Name": "first-interval",
                        "CreateRule": {"Interval": 12, "IntervalUnit": CREATE_INTERVAL_UNIT},
                        "RetainRule": {"Count": 2},
                    },
                    {
                        "Name": "second-cron",
                        "CreateRule": {"CronExpression": "cron(0 6 * * ? *)"},
                        "RetainRule": {"Interval": 1, "IntervalUnit": "DAYS"},
                    },
                ]),
            ),
        },
        RoundTrip {
            // The form the crate's existing row covers, kept as the baseline the
            // cron rows are judged beside.
            case: "an interval-driven schedule stating its own interval",
            details: custom_details(
                "EBS_SNAPSHOT_MANAGEMENT",
                "VOLUME",
                json!([{
                    "Name": "interval-round-trip",
                    "CreateRule": {
                        "Interval": 24,
                        "IntervalUnit": CREATE_INTERVAL_UNIT,
                        "Times": ["03:00"],
                    },
                    "RetainRule": {"Count": 7},
                }]),
            ),
        },
    ];

    for RoundTrip { case, details } in cases {
        let created = create(&router, create_request(case, details.clone())).await;
        assert_accepted(case, &created);
        let id = created.policy_id();
        let reported = at(get(&router, &id).await.policy(), "/PolicyDetails");

        // What the read reports for each creation rule: the stated interval, or
        // the materialized filler for a rule that stated none.
        let sent_schedules = at(&details, "/Schedules");
        let sent_schedules = sent_schedules.as_array().expect("the rows state schedules");
        for (index, sent_schedule) in sent_schedules.iter().enumerate() {
            let sent_rule = at(sent_schedule, "/CreateRule");
            let reported_interval =
                at(&reported, &format!("/Schedules/{index}/CreateRule/Interval"));
            assert_eq!(
                reported_interval,
                expected_reported_interval(&sent_rule),
                "case {case:?}: Schedules[{index}].CreateRule.Interval must be reported as the \
                 stated interval or the materialized one, got {reported_interval:?}"
            );
        }

        // The read-modify-write flow: the very structure the read reported, sent
        // back as this policy's configuration.
        let answer = update(&router, &id, json!({"PolicyDetails": reported.clone()})).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the configuration the service reports must be one the update \
             accepts, got {} with {:?}",
            answer.status, answer.body
        );
        let after_update = at(get(&router, &id).await.policy(), "/PolicyDetails");
        assert_eq!(
            after_update, reported,
            "case {case:?}: re-sending the reported configuration must leave it unchanged, got \
             {after_update:?}"
        );

        // And the same structure as a brand-new policy's configuration: a caller
        // may clone a reported configuration.
        let clone = create(&router, create_request(case, reported.clone())).await;
        assert_eq!(
            clone.status, 200,
            "case {case:?}: the configuration the service reports must be one the create \
             accepts, got {} with {:?}",
            clone.status, clone.body
        );
        let cloned = at(get(&router, &clone.policy_id()).await.policy(), "/PolicyDetails");
        assert_eq!(
            cloned, reported,
            "case {case:?}: the cloned policy must hold the configuration it was created from, \
             got {cloned:?}"
        );
    }
}

/// What a request that STATES something about the creation interval is owed.
#[derive(Debug, Clone, Copy)]
enum Owed {
    /// Both documented forms stated at once.
    BothForms,
    /// An interval the service does not support.
    UnsupportedInterval(i64),
    /// A member the form its rule states requires is missing: the wire path the
    /// refusal must report in `RequiredParameters`.
    Required(&'static str),
}

struct Refusal {
    case: &'static str,
    create_rule: Value,
    owed: Owed,
}

/// Assert `answer` is this service's modelled refusal, and says why.
fn assert_refused(case: &str, answer: &Answer, owed: Owed) {
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
    match owed {
        Owed::BothForms => assert!(
            answer.message().contains(
                "Interval based CreateRule and Cron based CreateRule cannot both be present"
            ),
            "case {case:?}: the refusal must say both forms were stated, got {:?}",
            answer.message()
        ),
        Owed::UnsupportedInterval(interval) => {
            let supported: Vec<String> = SUPPORTED_CREATE_INTERVALS
                .iter()
                .map(i64::to_string)
                .collect();
            let expected = format!(
                "The parameter CreateRule.Interval in schedule {{refused}} was {interval}. The \
                 supported values are {}.",
                supported.join(", ")
            );
            assert_eq!(
                answer.message(),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                answer.message()
            );
        }
        Owed::Required(parameter) => {
            let reported = at(&answer.body, "/RequiredParameters");
            assert!(
                reported
                    .as_array()
                    .is_some_and(|names| names.contains(&json!(parameter))),
                "case {case:?}: RequiredParameters must name {parameter:?}, got {reported:?}"
            );
            assert!(
                answer.message().contains(parameter),
                "case {case:?}: the refusal must name {parameter:?}, got {:?}",
                answer.message()
            );
        }
    }
}

/// RULE, the other half — what a request STATES about the creation interval is
/// judged by the domain the member's documentation publishes: beside a cron
/// expression an interval states both documented forms, outside the supported
/// set it is refused, and the materialized filler states no interval at all, on
/// the create and on the update alike.
///
/// The rows span: every way the interval form can be stated beside the cron form,
/// a stated interval below, between and above the entries of the supported set,
/// and the filler stated alone and beside an interval unit.
#[tokio::test]
async fn an_interval_the_request_stated_is_still_judged() {
    let router = emulator();
    // A policy to update, whose stored configuration a refused update must not
    // change.
    let subject = create(
        &router,
        create_request(
            "refusal subject",
            custom_details(
                "EBS_SNAPSHOT_MANAGEMENT",
                "VOLUME",
                json!([{
                    "Name": "subject",
                    "CreateRule": {"Interval": 12, "IntervalUnit": CREATE_INTERVAL_UNIT},
                    "RetainRule": {"Count": 1},
                }]),
            ),
        ),
    )
    .await;
    assert_accepted("refusal subject", &subject);
    let subject_id = subject.policy_id();
    let stored = at(get(&router, &subject_id).await.policy(), "/PolicyDetails");

    let cron = "cron(0 12 * * ? *)";
    let mut rows = vec![
        Refusal {
            case: "the cron form beside an interval the request stated",
            create_rule: json!({"CronExpression": cron, "Interval": 12}),
            owed: Owed::BothForms,
        },
        Refusal {
            case: "the cron form beside a complete interval form",
            create_rule: json!({
                "CronExpression": cron,
                "Interval": 24,
                "IntervalUnit": CREATE_INTERVAL_UNIT,
            }),
            owed: Owed::BothForms,
        },
        Refusal {
            case: "the cron form beside an IntervalUnit",
            create_rule: json!({"CronExpression": cron, "IntervalUnit": CREATE_INTERVAL_UNIT}),
            owed: Owed::BothForms,
        },
        Refusal {
            case: "the cron form beside the Times the interval form may state",
            create_rule: json!({"CronExpression": cron, "Times": ["03:00"]}),
            owed: Owed::BothForms,
        },
        Refusal {
            // The filler states no interval, so a rule carrying it beside an
            // IntervalUnit states the interval form without its interval.
            case: "an IntervalUnit beside the materialized interval",
            create_rule: json!({
                "Interval": MATERIALIZED_INTERVAL,
                "IntervalUnit": CREATE_INTERVAL_UNIT,
            }),
            owed: Owed::Required("PolicyDetails.Schedules[1].CreateRule.Interval"),
        },
        Refusal {
            // And alone it states neither form at all.
            case: "the materialized interval alone",
            create_rule: json!({"Interval": MATERIALIZED_INTERVAL}),
            owed: Owed::Required("PolicyDetails.Schedules[1].CreateRule"),
        },
    ];
    // Every interval outside the supported set that a request can state, up to
    // one past its largest entry: inside the set's span (5, 7), between its
    // entries (9, 11), above it (25) and below the floor the handler now owns
    // (-1), since the boundary no longer refuses this member's value for it.
    for interval in [5i64, 7, 9, 11, 25, -1] {
        rows.push(Refusal {
            case: "an interval the service does not support",
            create_rule: json!({"Interval": interval, "IntervalUnit": CREATE_INTERVAL_UNIT}),
            owed: Owed::UnsupportedInterval(interval),
        });
    }

    for Refusal {
        case,
        create_rule,
        owed,
    } in rows
    {
        let details = custom_details(
            "EBS_SNAPSHOT_MANAGEMENT",
            "VOLUME",
            json!([{
                "Name": "refused",
                "CreateRule": create_rule,
                "RetainRule": {"Count": 1},
            }]),
        );
        let created = create(&router, create_request(case, details.clone())).await;
        assert_refused(case, &created, owed);

        let updated = update(&router, &subject_id, json!({"PolicyDetails": details})).await;
        assert_refused(case, &updated, owed);
        let after = at(get(&router, &subject_id).await.policy(), "/PolicyDetails");
        assert_eq!(
            after, stored,
            "case {case:?}: a refused update must leave the stored configuration as it was, got \
             {after:?}"
        );
    }
}

/// RULE, applied to the whole supported set — every interval
/// `CreateRule$Interval` offers is accepted, stored and reported back, and the
/// configuration reported for it is accepted again.
#[tokio::test]
async fn every_supported_create_interval_round_trips() {
    let router = emulator();
    for interval in SUPPORTED_CREATE_INTERVALS {
        let case = format!("a creation interval of {interval}");
        let details = custom_details(
            "EBS_SNAPSHOT_MANAGEMENT",
            "VOLUME",
            json!([{
                "Name": "supported",
                "CreateRule": {"Interval": interval, "IntervalUnit": CREATE_INTERVAL_UNIT},
                "RetainRule": {"Count": 1},
            }]),
        );
        let created = create(&router, create_request(&case, details)).await;
        assert_accepted(&case, &created);
        let id = created.policy_id();
        let reported = at(get(&router, &id).await.policy(), "/PolicyDetails");
        assert_eq!(
            at(&reported, "/Schedules/0/CreateRule/Interval"),
            json!(interval),
            "case {case:?}: the stored interval must be the one that was stated, got {:?}",
            at(&reported, "/Schedules/0/CreateRule/Interval")
        );
        let answer = update(&router, &id, json!({"PolicyDetails": reported.clone()})).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the reported configuration must be accepted back, got {} with {:?}",
            answer.status, answer.body
        );
    }
}
