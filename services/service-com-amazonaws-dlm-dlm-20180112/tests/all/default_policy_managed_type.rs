// SPDX-License-Identifier: CC0-1.0

//! What a DEFAULT policy's `PolicyDetails$PolicyType` states, end to end over
//! HTTP.
//!
//! The rule under test: for a default policy the type it MANAGES and the kind of
//! default policy it is state one fact — `DefaultPolicy` / `ResourceType`
//! `INSTANCE` is "a default policy for EBS-backed AMIs" and `PolicyType`
//! `IMAGE_MANAGEMENT` is the policy that "manages the lifecycle of EBS-backed
//! AMIs", while `VOLUME`'s snapshots of volumes are
//! `EBS_SNAPSHOT_MANAGEMENT`'s — so a create may state either member, a pair
//! that disagrees and a type no kind of default policy manages are refused with
//! the modelled client error before anything is stored, and the type the
//! accepted request asked for is what the record holds and every read reports.
//!
//! Every case drives the router, so each one exercises routing,
//! deserialization, the handler, the persisted record and serialization — the
//! only path on which the type a create was accepted with can be seen to be the
//! type the policy holds.
//!
//! The rows span what the rule generalises over: every member of
//! `DefaultPolicyTypeValues` as the kind asked for, both places a create may
//! state that kind in (the body's `DefaultPolicy` and the structure's
//! `ResourceType`), the type stated and left unstated beside it, each type a
//! default policy manages stated as the ONLY statement of the kind, and every
//! `PolicyTypeValues` member that contradicts the kind asked for — including
//! `EVENT_BASED_POLICY`, which no kind of default policy manages.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for.
const REGION: &str = "eu-central-1";

/// The language a DEFAULT policy's configuration names
/// (`PolicyDetails$PolicyLanguage`: "`SIMPLIFIED` To create a default policy").
const DEFAULT_POLICY_LANGUAGE: &str = "SIMPLIFIED";

/// Every `DefaultPolicyTypeValues` member, from `smithy_query(query="shape",
/// shape="DefaultPolicyTypeValues")`.
const DEFAULT_POLICY_TYPE_VALUES: &[&str] = &["VOLUME", "INSTANCE"];

/// Every `PolicyTypeValues` member, from `smithy_query(query="shape",
/// shape="PolicyTypeValues")`.
const POLICY_TYPE_VALUES: &[&str] = &[
    "EBS_SNAPSHOT_MANAGEMENT",
    "IMAGE_MANAGEMENT",
    "EVENT_BASED_POLICY",
];

/// The `PolicyTypeValues` member that "manages the lifecycle of Amazon EBS
/// snapshots", which is also the documented default ("The default is
/// `EBS_SNAPSHOT_MANAGEMENT`").
const SNAPSHOT_POLICY_TYPE: &str = "EBS_SNAPSHOT_MANAGEMENT";

/// The `PolicyTypeValues` member that "manages the lifecycle of EBS-backed
/// AMIs".
const IMAGE_POLICY_TYPE: &str = "IMAGE_MANAGEMENT";

/// The `PolicyTypeValues` member of a policy that "performs specific actions
/// when a defined event occurs", which is neither kind of default policy
/// `DefaultPolicy` offers.
const EVENT_BASED_POLICY_TYPE: &str = "EVENT_BASED_POLICY";

/// The wire name a refusal of the configuration's policy type names.
const POLICY_TYPE_PARAMETER: &str = "PolicyDetails.PolicyType";

/// The modelled client error this service answers for a refused parameter, its
/// `@httpError` status and its `Code`.
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";
const INVALID_REQUEST_STATUS: u16 = 400;
const INVALID_PARAMETER_CODE: &str = "InvalidParameter";

/// What a default policy of the kind `resource_type` manages, from the two
/// members' own documentation: `DefaultPolicy` "To create a default policy for
/// EBS-backed AMIs ... specify `INSTANCE`", against `VOLUME`, which "creates
/// snapshots of all volumes in the Region"; `PolicyDetails$PolicyType`
/// "`IMAGE_MANAGEMENT` ... manages the lifecycle of EBS-backed AMIs" against
/// "`EBS_SNAPSHOT_MANAGEMENT` ... manages the lifecycle of Amazon EBS
/// snapshots".
fn managed_type(resource_type: &str) -> &'static str {
    match resource_type {
        "INSTANCE" => IMAGE_POLICY_TYPE,
        _ => SNAPSHOT_POLICY_TYPE,
    }
}

/// The same correspondence read the other way: the kind of default policy that
/// manages `policy_type`, or `None` when none does.
fn kind_managing(policy_type: &str) -> Option<&'static str> {
    DEFAULT_POLICY_TYPE_VALUES
        .iter()
        .find(|kind| managed_type(kind) == policy_type)
        .copied()
}

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

    /// The policy ids a `GetLifecyclePolicies` answer lists.
    fn listed_ids(&self) -> Vec<String> {
        self.body["Policies"]
            .as_array()
            .map(|policies| {
                policies
                    .iter()
                    .map(|policy| policy["PolicyId"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The `PolicyType` the summary for `id` reports, or `Value::Null` when the
    /// answer does not list it.
    fn listed_policy_type(&self, id: &str) -> Value {
        self.body["Policies"]
            .as_array()
            .and_then(|policies| {
                policies
                    .iter()
                    .find(|policy| policy["PolicyId"].as_str() == Some(id))
            })
            .map(|policy| at(policy, "/PolicyType"))
            .unwrap_or(Value::Null)
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

async fn list(router: &MultiServiceRouter) -> Answer {
    call(router, "GET", "/policies", None).await
}

fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// How many policies the store holds, so a refused create can be shown to have
/// added none.
async fn policy_count(router: &MultiServiceRouter) -> usize {
    list(router).await.listed_ids().len()
}

/// The two places a create may state the kind of default policy it asks for.
#[derive(Debug, Clone, Copy)]
enum Place {
    /// `DefaultPolicy`, in the request body.
    Body,
    /// `PolicyDetails$ResourceType`, inside the configuration structure.
    Details,
}

impl Place {
    fn label(self) -> &'static str {
        match self {
            Place::Body => "DefaultPolicy",
            Place::Details => "PolicyDetails.ResourceType",
        }
    }
}

/// One create request: the kind of default policy it asks for (stated in
/// `place`, or not stated at all) beside the `PolicyType` its configuration
/// states.
fn create_request(
    description: &str,
    kind: Option<(Place, &str)>,
    policy_type: Option<&str>,
) -> Value {
    let mut details = json!({"PolicyLanguage": DEFAULT_POLICY_LANGUAGE});
    if let Some(policy_type) = policy_type {
        details["PolicyType"] = json!(policy_type);
    }
    let mut request = json!({
        "ExecutionRoleArn": role_arn(),
        "Description": description,
        "State": "ENABLED",
    });
    match kind {
        Some((Place::Body, resource_type)) => request["DefaultPolicy"] = json!(resource_type),
        Some((Place::Details, resource_type)) => details["ResourceType"] = json!(resource_type),
        None => {}
    }
    request["PolicyDetails"] = details;
    request
}

/// One accepted row: the request, and the kind and type every read of the
/// policy it creates must report.
struct Accepted {
    case: String,
    request: Value,
    policy_type: &'static str,
    resource_type: &'static str,
}

/// The accepted table: for each kind of default policy, both places it may be
/// stated in, with and without the type that kind manages stated beside it; and
/// each type a default policy manages as the ONLY statement of the kind.
fn accepted() -> Vec<Accepted> {
    let mut rows = Vec::new();
    for kind in DEFAULT_POLICY_TYPE_VALUES {
        let managed = managed_type(kind);
        for place in [Place::Body, Place::Details] {
            for stated in [None, Some(managed)] {
                let case = format!(
                    "a default {kind} policy stated as {} with PolicyType {stated:?}",
                    place.label()
                );
                rows.push(Accepted {
                    request: create_request(
                        &format!("default {kind} {}", stated.unwrap_or("unstated")),
                        Some((place, kind)),
                        stated,
                    ),
                    case,
                    policy_type: managed,
                    resource_type: kind,
                });
            }
        }
    }
    for policy_type in POLICY_TYPE_VALUES {
        let Some(kind) = kind_managing(policy_type) else {
            continue;
        };
        rows.push(Accepted {
            case: format!("a default policy asked for by PolicyType {policy_type} alone"),
            request: create_request(
                &format!("default by type {policy_type}"),
                None,
                Some(policy_type),
            ),
            policy_type,
            resource_type: kind,
        });
    }
    rows
}

/// One refused row: the request, and the `PolicyType` it stated.
struct Refused {
    case: String,
    request: Value,
    stated: &'static str,
}

/// The refused table: every `PolicyTypeValues` member other than the one the
/// kind asked for manages, in both places the kind may be stated, plus the type
/// no kind of default policy manages stated with no kind beside it.
fn refused() -> Vec<Refused> {
    let mut rows = Vec::new();
    for kind in DEFAULT_POLICY_TYPE_VALUES {
        for stated in POLICY_TYPE_VALUES {
            if *stated == managed_type(kind) {
                continue;
            }
            for place in [Place::Body, Place::Details] {
                rows.push(Refused {
                    case: format!(
                        "a default {kind} policy stated as {} with PolicyType {stated}",
                        place.label()
                    ),
                    request: create_request(
                        &format!("contradiction {kind} {stated}"),
                        Some((place, kind)),
                        Some(stated),
                    ),
                    stated,
                });
            }
        }
    }
    rows.push(Refused {
        case: format!("a default policy whose only statement of its kind is {EVENT_BASED_POLICY_TYPE}"),
        request: create_request(
            "event based default",
            None,
            Some(EVENT_BASED_POLICY_TYPE),
        ),
        stated: EVENT_BASED_POLICY_TYPE,
    });
    rows
}

/// A refusal of a parameter: the modelled error, its status, its `Code`, and a
/// message naming the member refused and the value the request stated.
fn assert_refused(case: &str, stated: &str, answer: &Answer) {
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
    let message = at(&answer.body, "/Message");
    for named in [POLICY_TYPE_PARAMETER, stated] {
        assert!(
            message.as_str().is_some_and(|text| text.contains(named)),
            "case {case:?}: the message must name {named:?}, got {message:?}"
        );
    }
}

/// An accepted default-policy create stores the kind and the type the request
/// asked for, and both reads report them.
#[tokio::test]
async fn a_default_policy_reports_the_type_the_request_asked_for() {
    let router = emulator();

    for row in accepted() {
        let case = row.case.as_str();
        let created = create(&router, row.request.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the create must be accepted, got {} with {:?}",
            created.status, created.body
        );
        let id = created.policy_id();

        let read = get(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetLifecyclePolicy, got {} with {:?}",
            read.status, read.body
        );
        for (pointer, expected) in [
            ("/DefaultPolicy", json!(true)),
            ("/PolicyDetails/PolicyLanguage", json!(DEFAULT_POLICY_LANGUAGE)),
            ("/PolicyDetails/PolicyType", json!(row.policy_type)),
            ("/PolicyDetails/ResourceType", json!(row.resource_type)),
        ] {
            let actual = at(read.policy(), pointer);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {pointer} {expected:?}, got {actual:?}"
            );
        }

        // The two read projections of one policy agree about the type it manages.
        let listed = list(&router).await.listed_policy_type(&id);
        assert_eq!(
            listed,
            json!(row.policy_type),
            "case {case:?}: the summary must report PolicyType {:?}, got {listed:?}",
            row.policy_type
        );
    }
}

/// The configuration an accepted create stored is one the UPDATE accepts back:
/// the record holds no type its own reads contradict, so resending the
/// configuration a read reports asks for no change of type and leaves the policy
/// as it was.
#[tokio::test]
async fn the_stored_configuration_is_accepted_back_by_an_update() {
    let router = emulator();

    for row in accepted() {
        let case = row.case.as_str();
        let id = create(&router, row.request.clone()).await.policy_id();
        let reported = at(get(&router, &id).await.policy(), "/PolicyDetails");

        let resent = update(&router, &id, json!({"PolicyDetails": reported.clone()})).await;
        assert_eq!(
            resent.status, 200,
            "case {case:?}: the update must accept the stored configuration {reported:?} back, \
             got {} with {:?}",
            resent.status, resent.body
        );

        let after = at(get(&router, &id).await.policy(), "/PolicyDetails");
        assert_eq!(
            after, reported,
            "case {case:?}: resending the stored configuration must leave it alone, \
             got {after:?}"
        );
    }
}

/// A create whose configuration states a type the kind of default policy it
/// asks for does not manage is refused, and stores nothing.
#[tokio::test]
async fn a_policy_type_no_default_policy_of_that_kind_manages_is_refused() {
    let router = emulator();

    for row in refused() {
        let case = row.case.as_str();
        let before = policy_count(&router).await;
        let answer = create(&router, row.request.clone()).await;
        assert_refused(case, row.stated, &answer);

        let after = policy_count(&router).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused create must store no policy, the store went from \
             {before} to {after}"
        );
    }
}

/// The tables cover the whole catalog: every `PolicyTypeValues` member is either
/// a type some kind of default policy manages (accepted as the statement of that
/// kind) or one none manages (refused), and every `DefaultPolicyTypeValues`
/// member appears as a kind asked for.
#[tokio::test]
async fn the_tables_cover_every_modelled_value() {
    for policy_type in POLICY_TYPE_VALUES {
        let accepted_for = accepted()
            .into_iter()
            .any(|row| row.policy_type == *policy_type);
        let refused_for = refused().into_iter().any(|row| row.stated == *policy_type);
        assert_eq!(
            accepted_for,
            kind_managing(policy_type).is_some(),
            "PolicyTypeValues {policy_type:?} must be accepted exactly when a kind of default \
             policy manages it"
        );
        assert!(
            refused_for || kind_managing(policy_type).is_some(),
            "PolicyTypeValues {policy_type:?} must appear in one of the tables"
        );
    }
    for kind in DEFAULT_POLICY_TYPE_VALUES {
        assert!(
            accepted().into_iter().any(|row| row.resource_type == *kind),
            "DefaultPolicyTypeValues {kind:?} must appear in the accepted table"
        );
    }
}
