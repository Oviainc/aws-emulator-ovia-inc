// SPDX-License-Identifier: CC0-1.0

//! Where the `iam:PassRole` authorization of an `ExecutionRoleArn` sits relative
//! to the refusal of the request's shape, end to end over HTTP.
//!
//! The rule under test:
//!
//! > `iam:PassRole` for the `ExecutionRoleArn` a request names is authorized
//! > BEFORE the request's shape is refused, so an ARN the caller may not pass is
//! > answered `AccessDeniedException` (HTTP 403) for every body that names it —
//! > whether that body also omits a required member (at any depth), states a
//! > value outside a modelled constraint, or is complete — and the denial changes
//! > no state. Only a request naming a role that CAN be passed reaches the shape
//! > walk, which answers this service's modelled `InvalidRequestException`
//! > (HTTP 400), and a request naming no role at all has nothing to authorize.
//!
//! So the class of the answer is a function of the ARN alone, never of an
//! unrelated member: two bodies carrying the same non-passable ARN cannot be
//! answered with two different error classes because one of them left a member
//! out.
//!
//! The table spans the class rather than the one request the issue cited: the
//! defect is a top-level required member omitted, several members omitted at
//! once, a required member of a list element omitted, one of a list element
//! nested under another list element, a scalar value outside its `@pattern`, a
//! collection outside its `@length`, and a body with no defect at all — crossed
//! with each class of non-passable role, over both operations that take an
//! `ExecutionRoleArn` (`CreateLifecyclePolicy`, `UpdateLifecyclePolicy`).
//!
//! The member names, their constraints and which of them are `@required` are the
//! model's (`smithy_query(query="operation", operation="CreateLifecyclePolicy")`,
//! `operation="UpdateLifecyclePolicy"`, `smithy_query(query="shape",
//! shape="PolicyDetails")`, `shape="Schedule"`, `shape="Tag"`,
//! `shape="CrossRegionCopyRule"`, `shape="ShareRule"`, `shape="TagMap"`).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for.
const REGION: &str = "eu-central-1";

/// The error code an AWS authorization failure carries, and its status.
const ACCESS_DENIED: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 403;

/// The modelled client error this service refuses a shape with, its `@httpError`
/// status and its `Code`.
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";
const INVALID_REQUEST_STATUS: u16 = 400;
const INVALID_PARAMETER_CODE: &str = "InvalidParameter";

/// `IntervalUnitValues` offers `HOURS` alone, and `CreateRule$Interval`'s
/// documentation publishes 24 among its supported values
/// (`smithy_query(query="shape", shape="IntervalUnitValues")`,
/// `smithy_query(query="doc", shape="CreateRule", member="Interval")`).
const CREATE_INTERVAL: i32 = 24;
const CREATE_INTERVAL_UNIT: &str = "HOURS";

/// What the service must answer for one request body, once the role it names may
/// be passed.
#[derive(Debug, Clone, Copy)]
enum Walk {
    /// Nothing for the shape walk to refuse: the operation runs.
    Accepted,
    /// The body omitted required parameters: the refusal names them in
    /// `RequiredParameters`. A body missing several reports at least one of
    /// them and nothing else.
    Omitted(&'static [&'static str]),
    /// The body stated a parameter whose value is outside what the model admits:
    /// the message states it and `RequiredParameters` stays unset.
    Invalid(&'static str),
}

/// One request body, named by the defect it carries, with the answer the shape
/// walk owes it. The `ExecutionRoleArn` is added by the caller, so one body is
/// sent once per role class.
struct Shape {
    case: &'static str,
    body: Value,
    walk: Walk,
}

/// Which roles the caller may pass, and why not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pass {
    /// `iam:PassRole` is granted.
    Granted,
    /// Denied: the ARN names another account's principal.
    DeniedCrossAccount,
    /// Denied: the ARN is in this account but names no role.
    DeniedNotARole,
}

/// One `ExecutionRoleArn` and the authorization answer it is owed.
fn roles() -> Vec<(&'static str, String, Pass)> {
    let own = engine::emulator_account_id();
    vec![
        (
            "a role in another account",
            "arn:aws:iam::111122223333:role/CrossAccountDlmRole".to_string(),
            Pass::DeniedCrossAccount,
        ),
        (
            "a role in another account in another partition",
            "arn:aws-us-gov:iam::111122223333:role/CrossAccountDlmRole".to_string(),
            Pass::DeniedCrossAccount,
        ),
        (
            "an IAM user in this account",
            format!("arn:aws:iam::{own}:user/not-a-role"),
            Pass::DeniedNotARole,
        ),
        (
            "a role in this account",
            format!("arn:aws:iam::{own}:role/AWSDataLifecycleManagerDefaultRole"),
            Pass::Granted,
        ),
    ]
}

/// The message a denial must carry for `role`.
fn denial_message(pass: Pass, role: &str) -> String {
    match pass {
        Pass::DeniedCrossAccount => "Cross-account pass role is not allowed.".to_string(),
        Pass::DeniedNotARole => {
            format!("The role {role} cannot be passed: it does not identify an IAM role.")
        }
        Pass::Granted => String::new(),
    }
}

/// A schedule the service accepts, as the subject of every row that is not about
/// a schedule's own defect.
fn good_schedule() -> Value {
    json!({
        "Name": "daily",
        "CreateRule": {"Interval": CREATE_INTERVAL, "IntervalUnit": CREATE_INTERVAL_UNIT},
        "RetainRule": {"Count": 1},
    })
}

/// A custom EBS snapshot configuration the service accepts.
fn good_details() -> Value {
    json!({
        "PolicyType": "EBS_SNAPSHOT_MANAGEMENT",
        "ResourceTypes": ["VOLUME"],
        "TargetTags": [{"Key": "backup", "Value": "daily"}],
        "Schedules": [good_schedule()],
    })
}

/// `good_details`, with its single schedule replaced by `schedule`.
fn details_with(schedule: Value) -> Value {
    let mut details = good_details();
    details["Schedules"] = json!([schedule]);
    details
}

/// Every body `CreateLifecyclePolicy` is sent, one per defect class.
fn create_shapes() -> Vec<Shape> {
    vec![
        Shape {
            case: "a complete body",
            body: json!({
                "Description": "pass role before the shape walk",
                "State": "ENABLED",
                "PolicyDetails": good_details(),
            }),
            walk: Walk::Accepted,
        },
        Shape {
            case: "a body omitting the required State",
            body: json!({
                "Description": "pass role before the shape walk",
                "PolicyDetails": good_details(),
            }),
            walk: Walk::Omitted(&["State"]),
        },
        Shape {
            case: "a body omitting the required Description",
            body: json!({
                "State": "ENABLED",
                "PolicyDetails": good_details(),
            }),
            walk: Walk::Omitted(&["Description"]),
        },
        Shape {
            case: "a body carrying nothing but the role",
            body: json!({}),
            walk: Walk::Omitted(&["Description", "State"]),
        },
        Shape {
            case: "a body whose TargetTags element omits the required Key",
            body: json!({
                "Description": "pass role before the shape walk",
                "State": "ENABLED",
                "PolicyDetails": {
                    "PolicyType": "EBS_SNAPSHOT_MANAGEMENT",
                    "ResourceTypes": ["VOLUME"],
                    "TargetTags": [{"Key": "backup", "Value": "daily"}, {"Value": "keyless"}],
                    "Schedules": [good_schedule()],
                },
            }),
            walk: Walk::Omitted(&["PolicyDetails.TargetTags[2].Key"]),
        },
        Shape {
            case: "a body whose nested CrossRegionCopyRules element omits the required Encrypted",
            body: json!({
                "Description": "pass role before the shape walk",
                "State": "ENABLED",
                "PolicyDetails": details_with(json!({
                    "Name": "daily",
                    "CreateRule": {
                        "Interval": CREATE_INTERVAL,
                        "IntervalUnit": CREATE_INTERVAL_UNIT,
                    },
                    "RetainRule": {"Count": 1},
                    "CrossRegionCopyRules": [{"Target": "eu-west-1"}],
                })),
            }),
            walk: Walk::Omitted(&["PolicyDetails.Schedules[1].CrossRegionCopyRules[1].Encrypted"]),
        },
        Shape {
            case: "a body whose nested ShareRules element omits the required TargetAccounts",
            body: json!({
                "Description": "pass role before the shape walk",
                "State": "ENABLED",
                "PolicyDetails": details_with(json!({
                    "Name": "daily",
                    "CreateRule": {
                        "Interval": CREATE_INTERVAL,
                        "IntervalUnit": CREATE_INTERVAL_UNIT,
                    },
                    "RetainRule": {"Count": 1},
                    "ShareRules": [{"UnshareInterval": 1, "UnshareIntervalUnit": "DAYS"}],
                })),
            }),
            walk: Walk::Omitted(&["PolicyDetails.Schedules[1].ShareRules[1].TargetAccounts"]),
        },
        Shape {
            case: "a body whose Description is outside its pattern",
            body: json!({
                "Description": "not a legal description!",
                "State": "ENABLED",
                "PolicyDetails": good_details(),
            }),
            walk: Walk::Invalid("Description"),
        },
        Shape {
            case: "a body whose nested Schedule Name is outside its pattern",
            body: json!({
                "Description": "pass role before the shape walk",
                "State": "ENABLED",
                "PolicyDetails": details_with(json!({
                    "Name": "not a legal name!",
                    "CreateRule": {
                        "Interval": CREATE_INTERVAL,
                        "IntervalUnit": CREATE_INTERVAL_UNIT,
                    },
                    "RetainRule": {"Count": 1},
                })),
            }),
            walk: Walk::Invalid("PolicyDetails.Schedules[1].Name"),
        },
        Shape {
            // `TagMap` is `@length(min: 1, max: 200)`, so an empty map is
            // outside the collection's own bound.
            case: "a body whose Tags map is emptier than its length bound admits",
            body: json!({
                "Description": "pass role before the shape walk",
                "State": "ENABLED",
                "PolicyDetails": good_details(),
                "Tags": {},
            }),
            walk: Walk::Invalid("Tags"),
        },
    ]
}

/// Every body `UpdateLifecyclePolicy` is sent, one per defect class.
fn update_shapes() -> Vec<Shape> {
    vec![
        Shape {
            case: "a body carrying nothing but the role",
            body: json!({}),
            walk: Walk::Accepted,
        },
        Shape {
            case: "a body that also updates the State",
            body: json!({"State": "DISABLED"}),
            walk: Walk::Accepted,
        },
        Shape {
            case: "a body whose nested CrossRegionCopyRules element omits the required Encrypted",
            body: json!({
                "PolicyDetails": details_with(json!({
                    "Name": "daily",
                    "CreateRule": {
                        "Interval": CREATE_INTERVAL,
                        "IntervalUnit": CREATE_INTERVAL_UNIT,
                    },
                    "RetainRule": {"Count": 1},
                    "CrossRegionCopyRules": [{"Target": "eu-west-1"}],
                })),
            }),
            walk: Walk::Omitted(&["PolicyDetails.Schedules[1].CrossRegionCopyRules[1].Encrypted"]),
        },
        Shape {
            case: "a body whose TargetTags element omits the required Key",
            body: json!({
                "PolicyDetails": {
                    "PolicyType": "EBS_SNAPSHOT_MANAGEMENT",
                    "ResourceTypes": ["VOLUME"],
                    "TargetTags": [{"Value": "keyless"}],
                    "Schedules": [good_schedule()],
                },
            }),
            walk: Walk::Omitted(&["PolicyDetails.TargetTags[1].Key"]),
        },
        Shape {
            case: "a body whose Description is outside its pattern",
            body: json!({"Description": "not a legal description!"}),
            walk: Walk::Invalid("Description"),
        },
    ]
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
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The error message, however the envelope spells the member.
    fn message(&self) -> &str {
        self.body["Message"]
            .as_str()
            .or_else(|| self.body["message"].as_str())
            .unwrap_or_default()
    }

    /// The names `RequiredParameters` reports, empty when the member is unset.
    fn required_parameters(&self) -> Vec<String> {
        self.body["RequiredParameters"]
            .as_array()
            .map(|names| {
                names
                    .iter()
                    .map(|name| name.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The `PolicyId` `CreateLifecyclePolicy` returned.
    fn policy_id(&self) -> String {
        self.body["PolicyId"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

/// A SigV4 credential scope naming this service in `REGION`; it is what routes a
/// request to dlm.
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
            panic!("{method} {uri} answered a body that is not JSON ({error}): {bytes:?}")
        })
    };
    Answer { status, body }
}

/// `body` with the `ExecutionRoleArn` the row names added to it.
fn naming(body: &Value, role: &str) -> Value {
    let mut named = body.clone();
    named["ExecutionRoleArn"] = json!(role);
    named
}

/// Every policy id the store holds, as `GetLifecyclePolicies` reports them.
async fn stored_ids(router: &MultiServiceRouter) -> Vec<String> {
    let answer = call(router, "GET", "/policies", None).await;
    answer.body["Policies"]
        .as_array()
        .unwrap_or_else(|| panic!("GetLifecyclePolicies must return a list: {:?}", answer.body))
        .iter()
        .map(|summary| summary["PolicyId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The policy `id` as `GetLifecyclePolicy` reports it.
async fn stored_policy(router: &MultiServiceRouter, id: &str) -> Value {
    let answer = call(router, "GET", &format!("/policies/{id}"), None).await;
    answer.body["Policy"].clone()
}

/// The answer a denied `iam:PassRole` owes, whatever else the body held.
fn assert_denied(case: &str, role: &str, pass: Pass, answer: &Answer) {
    assert_eq!(
        answer.status, ACCESS_DENIED_STATUS,
        "case {case:?} ({role}): expected HTTP {ACCESS_DENIED_STATUS}, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        ACCESS_DENIED,
        "case {case:?} ({role}): expected error code {ACCESS_DENIED}, got {:?}",
        answer.error_code()
    );
    assert_eq!(
        answer.message(),
        denial_message(pass, role),
        "case {case:?} ({role}): expected the denial to say why, got {:?}",
        answer.message()
    );
}

/// The answer the shape walk owes a body whose role may be passed.
fn assert_walked(case: &str, role: &str, walk: Walk, answer: &Answer) {
    match walk {
        Walk::Accepted => assert_eq!(
            answer.status, 200,
            "case {case:?} ({role}): expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        ),
        Walk::Omitted(names) => {
            assert_refused(case, role, answer);
            let reported = answer.required_parameters();
            assert!(
                reported.iter().any(|name| names.contains(&name.as_str())),
                "case {case:?} ({role}): RequiredParameters must name one of {names:?}, got \
                 {reported:?}"
            );
            assert!(
                reported.iter().all(|name| names.contains(&name.as_str())),
                "case {case:?} ({role}): RequiredParameters must name only omitted parameters of \
                 {names:?}, got {reported:?}"
            );
        }
        Walk::Invalid(name) => {
            assert_refused(case, role, answer);
            assert!(
                answer.message().contains(name),
                "case {case:?} ({role}): the refusal must state {name:?}, got {:?}",
                answer.message()
            );
            let reported = answer.required_parameters();
            assert!(
                reported.is_empty(),
                "case {case:?} ({role}): an invalid value is not an omitted parameter, got \
                 RequiredParameters {reported:?}"
            );
        }
    }
}

/// The envelope every shape refusal of this service carries.
fn assert_refused(case: &str, role: &str, answer: &Answer) {
    assert_eq!(
        answer.status, INVALID_REQUEST_STATUS,
        "case {case:?} ({role}): expected HTTP {INVALID_REQUEST_STATUS}, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        INVALID_REQUEST_EXCEPTION,
        "case {case:?} ({role}): expected error code {INVALID_REQUEST_EXCEPTION}, got {:?}",
        answer.error_code()
    );
    assert_eq!(
        answer.body["Code"],
        json!(INVALID_PARAMETER_CODE),
        "case {case:?} ({role}): the refusal must carry the modelled Code, got {:?}",
        answer.body
    );
}

/// `CreateLifecyclePolicy` answers a role it may not pass with the denial for
/// every body that names it, and stores nothing; only a passable role reaches
/// the shape walk.
#[tokio::test]
async fn create_authorizes_the_role_ahead_of_every_shape_refusal() {
    let router = emulator();

    for shape in create_shapes() {
        for (role_case, role, pass) in roles() {
            let case = format!("{} with {role_case}", shape.case);
            let before = stored_ids(&router).await;
            let answer = call(
                &router,
                "POST",
                "/policies",
                Some(naming(&shape.body, &role)),
            )
            .await;
            let after = stored_ids(&router).await;

            if pass == Pass::Granted {
                assert_walked(&case, &role, shape.walk, &answer);
                if answer.status == 200 {
                    let id = answer.policy_id();
                    assert!(
                        after.contains(&id),
                        "case {case:?}: expected the created policy {id} in the store, got \
                         {after:?}"
                    );
                    let stored = stored_policy(&router, &id).await;
                    assert_eq!(
                        stored["ExecutionRoleArn"],
                        json!(role),
                        "case {case:?}: expected the stored ExecutionRoleArn to be the created \
                         one, got {:?}",
                        stored["ExecutionRoleArn"]
                    );
                }
                continue;
            }

            assert_denied(&case, &role, pass, &answer);
            assert_eq!(
                after, before,
                "case {case:?}: a denied create must leave no policy behind, got {after:?}"
            );
        }
    }
}

/// `UpdateLifecyclePolicy` answers a role it may not pass with the denial for
/// every body that names it, and leaves the stored policy as it was.
#[tokio::test]
async fn update_authorizes_the_role_ahead_of_every_shape_refusal() {
    let router = emulator();
    let original = format!(
        "arn:aws:iam::{}:role/AWSDataLifecycleManagerDefaultRole",
        engine::emulator_account_id()
    );

    for shape in update_shapes() {
        for (role_case, role, pass) in roles() {
            let case = format!("{} with {role_case}", shape.case);
            // One fresh subject per row, so what the row changes is only ever
            // compared against the policy it was sent to.
            let created = call(
                &router,
                "POST",
                "/policies",
                Some(naming(
                    &json!({
                        "Description": "pass role update subject",
                        "State": "ENABLED",
                        "PolicyDetails": good_details(),
                    }),
                    &original,
                )),
            )
            .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: the subject policy must be created, got {} with {:?}",
                created.status, created.body
            );
            let id = created.policy_id();
            let before = stored_policy(&router, &id).await;

            let answer = call(
                &router,
                "PATCH",
                &format!("/policies/{id}"),
                Some(naming(&shape.body, &role)),
            )
            .await;

            if pass == Pass::Granted {
                assert_walked(&case, &role, shape.walk, &answer);
                continue;
            }

            assert_denied(&case, &role, pass, &answer);
            assert_eq!(
                stored_policy(&router, &id).await,
                before,
                "case {case:?}: a denied update must leave the stored policy as it was, got {:?}",
                stored_policy(&router, &id).await
            );
        }
    }
}

/// A request naming NO `ExecutionRoleArn` has no role to authorize, so the shape
/// walk answers it — including the refusal of that very member when the
/// operation requires it.
#[tokio::test]
async fn a_request_naming_no_role_is_left_to_the_shape_walk() {
    let router = emulator();

    let answer = call(
        &router,
        "POST",
        "/policies",
        Some(json!({
            "Description": "no role named",
            "State": "ENABLED",
            "PolicyDetails": good_details(),
        })),
    )
    .await;
    assert_walked(
        "a create naming no ExecutionRoleArn",
        "none",
        Walk::Omitted(&["ExecutionRoleArn"]),
        &answer,
    );

    let created = call(
        &router,
        "POST",
        "/policies",
        Some(naming(
            &json!({
                "Description": "no role named update subject",
                "State": "ENABLED",
                "PolicyDetails": good_details(),
            }),
            &format!(
                "arn:aws:iam::{}:role/AWSDataLifecycleManagerDefaultRole",
                engine::emulator_account_id()
            ),
        )),
    )
    .await;
    let id = created.policy_id();

    // The update declares no required `ExecutionRoleArn`, so a body omitting it
    // is refused only for the defect it does carry.
    let answer = call(
        &router,
        "PATCH",
        &format!("/policies/{id}"),
        Some(json!({"Description": "not a legal description!"})),
    )
    .await;
    assert_walked(
        "an update naming no ExecutionRoleArn",
        "none",
        Walk::Invalid("Description"),
        &answer,
    );
}
