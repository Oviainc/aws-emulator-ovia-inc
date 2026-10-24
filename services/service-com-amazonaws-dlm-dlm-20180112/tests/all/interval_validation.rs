// SPDX-License-Identifier: CC0-1.0

//! The documented numeric domain of a lifecycle policy's interval parameters,
//! end to end over HTTP.
//!
//! The rule under test: Amazon Data Lifecycle Manager refuses a create or an
//! update whose interval parameters fall outside the domain their documentation
//! publishes — each member's bounds, the create rule's supported value set, and
//! the documented ordering between the creation frequency and the retention
//! period — with `InvalidRequestException` (HTTP 400, `Code`
//! `InvalidParameter`), wherever the request states them (the request body or
//! the `PolicyDetails` structure) and before any policy is stored or changed. A
//! value INSIDE the domain is accepted and read back.
//!
//! The published domains, from the model's own documentation:
//!
//! * `CreateInterval` — "The creation frequency can range from 1 to 7 days"
//!   (`smithy_query(query="doc", shape="CreateLifecyclePolicyRequest",
//!   member="CreateInterval")`).
//! * `RetainInterval` — "The retention period can range from 2 to 14 days, but
//!   it must be greater than the creation frequency to ensure that the policy
//!   retains at least 1 snapshot or AMI at any given time"
//!   (`smithy_query(query="doc", shape="CreateLifecyclePolicyRequest",
//!   member="RetainInterval")`).
//! * `CreateRule$Interval` — "The supported values are 1, 2, 3, 4, 6, 8, 12, and
//!   24" (`smithy_query(query="doc", shape="CreateRule", member="Interval")`).
//!
//! Each table spans the class rather than the one request the issue cited: every
//! published bound and a value one past it, each member's floor and ceiling, the
//! ordering at equality and below it, every entry of the supported interval set
//! and values the set omits, both places a request can state the members, and
//! both the create and the update.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for.
const REGION: &str = "us-east-2";

/// An `ExecutionRoleArn` satisfying `^arn:aws(-[a-z]{1,3}){0,2}:iam::\d+:role/`
/// in the emulator's own account, so `iam:PassRole` is granted.
fn role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/AWSDataLifecycleManagerDefaultRole",
        engine::emulator_account_id()
    )
}

/// `PolicyDetails$PolicyLanguage`: "`SIMPLIFIED` To create a default policy."
const DEFAULT_POLICY_LANGUAGE: &str = "SIMPLIFIED";

/// The modelled client error this service answers for a refused parameter, its
/// `@httpError` status and its `Code`.
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";
const INVALID_REQUEST_STATUS: u16 = 400;
const INVALID_PARAMETER_CODE: &str = "InvalidParameter";

/// The published bounds of the two cadence members, and the documented default
/// each takes when a request states none ("If you do not specify a value, the
/// default is 1" / "the default is 7").
const CREATE_INTERVAL_MIN: i64 = 1;
const CREATE_INTERVAL_MAX: i64 = 7;
const RETAIN_INTERVAL_MIN: i64 = 2;
const RETAIN_INTERVAL_MAX: i64 = 14;
const CREATE_INTERVAL_DEFAULT: i64 = 1;
const RETAIN_INTERVAL_DEFAULT: i64 = 7;

/// `CreateRule$Interval`'s supported values, verbatim from its documentation.
const SUPPORTED_CREATE_RULE_INTERVALS: [i64; 8] = [1, 2, 3, 4, 6, 8, 12, 24];

/// Values the supported set omits: the gaps inside its span, values past its
/// largest entry, and the largest integer the member's target shape admits.
const UNSUPPORTED_CREATE_RULE_INTERVALS: [i64; 10] =
    [5, 7, 9, 10, 11, 13, 23, 25, 168, 2_147_483_647];

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
    let answer = call(router, "GET", "/policies", None).await;
    answer.body["Policies"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// Assert `answer` is this service's modelled refusal, naming `member`.
fn assert_refused(case: &str, member: &str, answer: &Answer) {
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
        answer.body
    );
    assert!(
        answer.message().contains(member),
        "case {case:?}: the refusal must name {member:?}, got {:?}",
        answer.message()
    );
}

/// One creation/retention cadence a request can state, and what the service owes
/// for it.
struct CadenceCase {
    /// What is particular about this cadence.
    case: &'static str,
    create_interval: Option<i64>,
    retain_interval: Option<i64>,
    /// The member a refusal must name, or `None` when the cadence is accepted.
    refused: Option<&'static str>,
}

/// Every cadence row: the published bounds, one value past each of them, the
/// ordering rule at equality and below it, and the omitted-member defaults.
fn cadence_cases() -> Vec<CadenceCase> {
    vec![
        CadenceCase {
            case: "neither member stated, so both take their documented default",
            create_interval: None,
            retain_interval: None,
            refused: None,
        },
        CadenceCase {
            case: "both members at their documented minimum",
            create_interval: Some(CREATE_INTERVAL_MIN),
            retain_interval: Some(RETAIN_INTERVAL_MIN),
            refused: None,
        },
        CadenceCase {
            case: "both members at their documented maximum",
            create_interval: Some(CREATE_INTERVAL_MAX),
            retain_interval: Some(RETAIN_INTERVAL_MAX),
            refused: None,
        },
        CadenceCase {
            case: "a retention period one day longer than the creation frequency",
            create_interval: Some(4),
            retain_interval: Some(5),
            refused: None,
        },
        CadenceCase {
            case: "only the creation frequency stated, inside its range",
            create_interval: Some(3),
            retain_interval: None,
            refused: None,
        },
        CadenceCase {
            case: "only the retention period stated, at its maximum",
            create_interval: None,
            retain_interval: Some(RETAIN_INTERVAL_MAX),
            refused: None,
        },
        CadenceCase {
            case: "a creation frequency one day past its documented maximum",
            create_interval: Some(CREATE_INTERVAL_MAX + 1),
            retain_interval: Some(RETAIN_INTERVAL_MAX),
            refused: Some("CreateInterval"),
        },
        CadenceCase {
            // The request the issue recorded.
            case: "a creation frequency of 30 days beside a 1-day retention period",
            create_interval: Some(30),
            retain_interval: Some(1),
            refused: Some("CreateInterval"),
        },
        CadenceCase {
            case: "a creation frequency of the largest integer the member admits",
            create_interval: Some(2_147_483_647),
            retain_interval: None,
            refused: Some("CreateInterval"),
        },
        CadenceCase {
            case: "a retention period one day below its documented minimum",
            create_interval: None,
            retain_interval: Some(RETAIN_INTERVAL_MIN - 1),
            refused: Some("RetainInterval"),
        },
        CadenceCase {
            case: "a retention period one day past its documented maximum",
            create_interval: Some(CREATE_INTERVAL_MIN),
            retain_interval: Some(RETAIN_INTERVAL_MAX + 1),
            refused: Some("RetainInterval"),
        },
        CadenceCase {
            case: "a retention period of 100 days",
            create_interval: Some(2),
            retain_interval: Some(100),
            refused: Some("RetainInterval"),
        },
        CadenceCase {
            case: "a retention period equal to the creation frequency",
            create_interval: Some(3),
            retain_interval: Some(3),
            refused: Some("RetainInterval"),
        },
        CadenceCase {
            case: "a retention period shorter than the creation frequency, both in range",
            create_interval: Some(5),
            retain_interval: Some(4),
            refused: Some("RetainInterval"),
        },
    ]
}

/// The cadence row stated in the request BODY of a default-policy create.
fn body_create_request(row: &CadenceCase) -> Value {
    let mut request = json!({
        "ExecutionRoleArn": role_arn(),
        "Description": "cadence in the request body",
        "State": "ENABLED",
        "DefaultPolicy": "VOLUME",
    });
    if let Some(create_interval) = row.create_interval {
        request["CreateInterval"] = json!(create_interval);
    }
    if let Some(retain_interval) = row.retain_interval {
        request["RetainInterval"] = json!(retain_interval);
    }
    request
}

/// The same row stated inside the `PolicyDetails` of a default-policy create.
fn details_create_request(row: &CadenceCase) -> Value {
    let mut details = json!({"PolicyLanguage": DEFAULT_POLICY_LANGUAGE, "ResourceType": "VOLUME"});
    if let Some(create_interval) = row.create_interval {
        details["CreateInterval"] = json!(create_interval);
    }
    if let Some(retain_interval) = row.retain_interval {
        details["RetainInterval"] = json!(retain_interval);
    }
    json!({
        "ExecutionRoleArn": role_arn(),
        "Description": "cadence in PolicyDetails",
        "State": "ENABLED",
        "PolicyDetails": details,
    })
}

/// A create states its default policy's cadence either in the request body or in
/// `PolicyDetails`; both places answer the same way, and an accepted cadence is
/// read back from the stored policy.
#[tokio::test]
async fn a_create_enforces_the_documented_cadence_wherever_it_is_stated() {
    let router = emulator();
    for row in cadence_cases() {
        for (where_stated, request) in [
            ("in the request body", body_create_request(&row)),
            ("in PolicyDetails", details_create_request(&row)),
        ] {
            let case = format!("{} ({where_stated})", row.case);
            let before = policy_count(&router).await;
            let answer = create(&router, request).await;

            if let Some(member) = row.refused {
                assert_refused(&case, member, &answer);
                assert_eq!(
                    policy_count(&router).await,
                    before,
                    "case {case:?}: a refused create must leave no policy behind"
                );
                continue;
            }

            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200, got {} with {:?}",
                answer.status, answer.body
            );
            // Round trip: the accepted cadence is what a read reports, with each
            // omitted member carrying its documented default.
            let read = get(&router, &answer.policy_id()).await;
            let expected_create = row.create_interval.unwrap_or(CREATE_INTERVAL_DEFAULT);
            let expected_retain = row.retain_interval.unwrap_or(RETAIN_INTERVAL_DEFAULT);
            assert_eq!(
                at(read.policy(), "/PolicyDetails/CreateInterval"),
                json!(expected_create),
                "case {case:?}: expected CreateInterval {expected_create}, got {:?}",
                at(read.policy(), "/PolicyDetails/CreateInterval")
            );
            assert_eq!(
                at(read.policy(), "/PolicyDetails/RetainInterval"),
                json!(expected_retain),
                "case {case:?}: expected RetainInterval {expected_retain}, got {:?}",
                at(read.policy(), "/PolicyDetails/RetainInterval")
            );
        }
    }
}

/// One patch of a default policy's cadence, against a policy holding a known one.
struct PatchCase {
    case: &'static str,
    patch: Value,
    /// The member a refusal must name, or `None` when the patch is applied.
    refused: Option<&'static str>,
    /// The cadence the policy holds once the patch has been answered.
    expected: (i64, i64),
}

/// `UpdateLifecyclePolicy` enforces the same domain, including against the value
/// the policy ALREADY holds for the member a partial patch leaves out — and a
/// refused patch changes nothing.
#[tokio::test]
async fn an_update_enforces_the_documented_cadence_against_the_stored_policy() {
    let router = emulator();
    let policy = create(
        &router,
        json!({
            "ExecutionRoleArn": role_arn(),
            "Description": "a default policy to patch",
            "State": "ENABLED",
            "DefaultPolicy": "VOLUME",
            "CreateInterval": 3,
            "RetainInterval": 10,
        }),
    )
    .await;
    assert_eq!(
        policy.status, 200,
        "the subject policy must be created, got {} with {:?}",
        policy.status, policy.body
    );
    let id = policy.policy_id();

    // Applied in order against the one policy, so each row also pins what the
    // row before it left stored.
    let patches = vec![
        PatchCase {
            case: "a retention period below the stored creation frequency",
            patch: json!({"RetainInterval": 2}),
            refused: Some("RetainInterval"),
            expected: (3, 10),
        },
        PatchCase {
            case: "a creation frequency past its documented maximum",
            patch: json!({"CreateInterval": 10}),
            refused: Some("CreateInterval"),
            expected: (3, 10),
        },
        PatchCase {
            case: "a retention period past its documented maximum",
            patch: json!({"RetainInterval": RETAIN_INTERVAL_MAX + 1}),
            refused: Some("RetainInterval"),
            expected: (3, 10),
        },
        PatchCase {
            case: "a creation frequency the stored retention period still exceeds",
            patch: json!({"CreateInterval": 5}),
            refused: None,
            expected: (5, 10),
        },
        PatchCase {
            case: "a retention period above the newly stored creation frequency",
            patch: json!({"RetainInterval": 6}),
            refused: None,
            expected: (5, 6),
        },
        PatchCase {
            case: "a creation frequency that would equal the stored retention period",
            patch: json!({"CreateInterval": 6}),
            refused: Some("RetainInterval"),
            expected: (5, 6),
        },
        PatchCase {
            case: "both members at once, at their documented maximums",
            patch: json!({
                "CreateInterval": CREATE_INTERVAL_MAX,
                "RetainInterval": RETAIN_INTERVAL_MAX,
            }),
            refused: None,
            expected: (CREATE_INTERVAL_MAX, RETAIN_INTERVAL_MAX),
        },
        PatchCase {
            case: "a retention period stated inside PolicyDetails, below the creation frequency",
            patch: json!({
                "PolicyDetails": {
                    "PolicyLanguage": DEFAULT_POLICY_LANGUAGE,
                    "ResourceType": "VOLUME",
                    "CreateInterval": 4,
                    "RetainInterval": 4,
                },
            }),
            refused: Some("RetainInterval"),
            expected: (CREATE_INTERVAL_MAX, RETAIN_INTERVAL_MAX),
        },
        PatchCase {
            case: "a creation frequency stated inside PolicyDetails, past its maximum",
            patch: json!({
                "PolicyDetails": {
                    "PolicyLanguage": DEFAULT_POLICY_LANGUAGE,
                    "ResourceType": "VOLUME",
                    "CreateInterval": 9,
                },
            }),
            refused: Some("CreateInterval"),
            expected: (CREATE_INTERVAL_MAX, RETAIN_INTERVAL_MAX),
        },
    ];

    for row in patches {
        let case = row.case;
        let answer = update(&router, &id, row.patch).await;
        match row.refused {
            Some(member) => assert_refused(case, member, &answer),
            None => assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200, got {} with {:?}",
                answer.status, answer.body
            ),
        }
        let read = get(&router, &id).await;
        let (expected_create, expected_retain) = row.expected;
        assert_eq!(
            at(read.policy(), "/PolicyDetails/CreateInterval"),
            json!(expected_create),
            "case {case:?}: expected the policy to hold CreateInterval {expected_create}, got {:?}",
            at(read.policy(), "/PolicyDetails/CreateInterval")
        );
        assert_eq!(
            at(read.policy(), "/PolicyDetails/RetainInterval"),
            json!(expected_retain),
            "case {case:?}: expected the policy to hold RetainInterval {expected_retain}, got {:?}",
            at(read.policy(), "/PolicyDetails/RetainInterval")
        );
    }
}

/// A custom snapshot policy whose schedules are `schedules`.
fn custom_details(schedules: Value) -> Value {
    json!({
        "PolicyType": "EBS_SNAPSHOT_MANAGEMENT",
        "ResourceTypes": ["VOLUME"],
        "TargetTags": [{"Key": "backup", "Value": "daily"}],
        "Schedules": schedules,
    })
}

/// A create of a custom policy whose single schedule creates every `interval`
/// hours.
fn create_rule_request(interval: i64) -> Value {
    json!({
        "ExecutionRoleArn": role_arn(),
        "Description": "a schedule interval",
        "State": "ENABLED",
        "PolicyDetails": custom_details(json!([{
            "Name": "only",
            "CreateRule": {"Interval": interval, "IntervalUnit": "HOURS"},
            "RetainRule": {"Count": 1},
        }])),
    })
}

/// A create whose SECOND schedule carries `interval`, and which names neither
/// schedule: the rule holds for every element of `Schedules`, and a refusal names
/// an unnamed schedule by its position.
fn second_schedule_request(interval: i64) -> Value {
    json!({
        "ExecutionRoleArn": role_arn(),
        "Description": "an interval on the second schedule",
        "State": "ENABLED",
        "PolicyDetails": custom_details(json!([
            {
                "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS"},
                "RetainRule": {"Count": 1},
            },
            {
                "CreateRule": {"Interval": interval, "IntervalUnit": "HOURS"},
                "RetainRule": {"Count": 2},
            },
        ])),
    })
}

/// A custom schedule's creation interval is one of the supported values, on every
/// operation that accepts a `CreateRule` and for every schedule in the policy.
#[tokio::test]
async fn a_create_rule_interval_outside_the_supported_set_is_refused() {
    let router = emulator();

    // The subject of the update rows: a policy whose stored schedule is one the
    // service accepts.
    let subject = create(&router, create_rule_request(24)).await;
    assert_eq!(
        subject.status, 200,
        "the subject policy must be created, got {} with {:?}",
        subject.status, subject.body
    );
    let subject_id = subject.policy_id();
    let stored_schedules = at(
        get(&router, &subject_id).await.policy(),
        "/PolicyDetails/Schedules",
    );

    for interval in SUPPORTED_CREATE_RULE_INTERVALS {
        let case = format!("the supported interval {interval}");
        let created = create(&router, create_rule_request(interval)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            created.status, created.body
        );
        // Round trip: the accepted interval is what the stored policy reports.
        let read = get(&router, &created.policy_id()).await;
        assert_eq!(
            at(
                read.policy(),
                "/PolicyDetails/Schedules/0/CreateRule/Interval"
            ),
            json!(interval),
            "case {case:?}: expected the schedule to report its interval, got {:?}",
            at(
                read.policy(),
                "/PolicyDetails/Schedules/0/CreateRule/Interval"
            )
        );

        let patched = update(
            &router,
            &subject_id,
            json!({"PolicyDetails": custom_details(json!([{
                "Name": "only",
                "CreateRule": {"Interval": interval, "IntervalUnit": "HOURS"},
                "RetainRule": {"Count": 1},
            }]))}),
        )
        .await;
        assert_eq!(
            patched.status, 200,
            "case {case:?}: expected HTTP 200 from the update, got {} with {:?}",
            patched.status, patched.body
        );
    }

    // Leave the subject holding a supported interval again, so the refusal rows
    // below can assert the stored configuration was untouched.
    let restored = update(
        &router,
        &subject_id,
        json!({"PolicyDetails": custom_details(json!([{
            "Name": "only",
            "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS"},
            "RetainRule": {"Count": 1},
        }]))}),
    )
    .await;
    assert_eq!(
        restored.status, 200,
        "the subject policy must be restored, got {} with {:?}",
        restored.status, restored.body
    );

    for interval in UNSUPPORTED_CREATE_RULE_INTERVALS {
        for (where_stated, request, named) in [
            (
                "on a create's only schedule",
                create_rule_request(interval),
                "only",
            ),
            (
                "on a create's second, unnamed schedule",
                second_schedule_request(interval),
                "Schedules[2]",
            ),
        ] {
            let case = format!("the unsupported interval {interval} {where_stated}");
            let before = policy_count(&router).await;
            let answer = create(&router, request).await;
            assert_refused(&case, "CreateRule.Interval", &answer);
            assert!(
                answer.message().contains(named),
                "case {case:?}: the refusal must name the schedule {named:?}, got {:?}",
                answer.message()
            );
            assert!(
                answer.message().contains(&interval.to_string()),
                "case {case:?}: the refusal must name the interval it refused, got {:?}",
                answer.message()
            );
            assert_eq!(
                policy_count(&router).await,
                before,
                "case {case:?}: a refused create must leave no policy behind"
            );
        }

        let case = format!("the unsupported interval {interval} on an update");
        let answer = update(
            &router,
            &subject_id,
            json!({"PolicyDetails": custom_details(json!([{
                "Name": "only",
                "CreateRule": {"Interval": interval, "IntervalUnit": "HOURS"},
                "RetainRule": {"Count": 1},
            }]))}),
        )
        .await;
        assert_refused(&case, "CreateRule.Interval", &answer);
        assert_eq!(
            at(
                get(&router, &subject_id).await.policy(),
                "/PolicyDetails/Schedules"
            ),
            stored_schedules,
            "case {case:?}: a refused update must leave the stored schedules alone"
        );
    }
}
