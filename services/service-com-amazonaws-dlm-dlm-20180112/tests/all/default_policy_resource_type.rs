// SPDX-License-Identifier: CC0-1.0

//! A DEFAULT policy's resource type, end to end over HTTP: where a request may
//! state it, what every read reports for it, and which policies the
//! `resourceTypes` and `defaultPolicyType` filters select.
//!
//! The rule under test: the resource type of a default policy is ONE value, so
//! the two places a request can state it — `DefaultPolicy` in the request body
//! and `PolicyDetails$ResourceType` inside the configuration structure, which
//! the model documents with the same sentence — are alternatives a single
//! request may not use both of, every read and every resource-type filter report
//! that one value, and an update that asks to change it is refused ("You cannot
//! update the policy type or the resource type").
//!
//! Every case drives the router, so each one exercises routing,
//! deserialization, the handler, the persisted record, serialization and the
//! list filters — the only path on which "the read reports one type while the
//! filter matches another" can be seen at all.
//!
//! The rows span what the rule generalises over: both `DefaultPolicyTypeValues`
//! members, both places a create can state the type in, both directions of a
//! change, an update that agrees with the stored type and one that differs, a
//! create stating the two places in agreement and in disagreement, and a create
//! that states the type in neither place (which `ResourceType`'s own rule
//! refuses — `default_policy_requires_a_resource_type.rs` spans that rule).

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for.
const REGION: &str = "ap-southeast-2";

/// `PolicyDetails$PolicyLanguage`: "`SIMPLIFIED` To create a default policy."
const DEFAULT_POLICY_LANGUAGE: &str = "SIMPLIFIED";

/// The modelled client error this service answers for a refused parameter, its
/// `@httpError` status and its `Code`.
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";
const INVALID_REQUEST_STATUS: u16 = 400;
const INVALID_PARAMETER_CODE: &str = "InvalidParameter";

/// Every `DefaultPolicyTypeValues` member, read against the `PolicyTypeValues` a
/// default policy of that type reports.
///
/// Source: `smithy_query(query="shape", shape="DefaultPolicyTypeValues")` —
/// `VOLUME` and `INSTANCE` — read against
/// `CreateLifecyclePolicyRequest$DefaultPolicy` / `PolicyDetails$ResourceType`
/// ("To create a default policy for EBS snapshots ... specify `VOLUME`", "for
/// EBS-backed AMIs ... specify `INSTANCE`") and `PolicyTypeValues`
/// (`EBS_SNAPSHOT_MANAGEMENT`, `IMAGE_MANAGEMENT`).
const DEFAULT_POLICY_TYPES: [(&str, &str); 2] = [
    ("VOLUME", "EBS_SNAPSHOT_MANAGEMENT"),
    ("INSTANCE", "IMAGE_MANAGEMENT"),
];

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

async fn list(router: &MultiServiceRouter, query: &str) -> Answer {
    call(router, "GET", &format!("/policies{query}"), None).await
}

fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// How many policies the store holds, so a refused create can be shown to have
/// added none.
async fn policy_count(router: &MultiServiceRouter) -> usize {
    list(router, "").await.listed_ids().len()
}

/// The two places a request can state a default policy's resource type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    /// `DefaultPolicy`, in the request body of the create.
    Body,
    /// `PolicyDetails$ResourceType`, inside the configuration structure.
    Details,
}

impl Place {
    fn label(self) -> &'static str {
        match self {
            Place::Body => "DefaultPolicy in the request body",
            Place::Details => "PolicyDetails.ResourceType",
        }
    }
}

/// A create of a default policy of `resource_type`, stating it in `place`.
fn create_request(place: Place, resource_type: &str) -> Value {
    let mut request = json!({
        "ExecutionRoleArn": role_arn(),
        "Description": "a default policy",
        "State": "ENABLED",
    });
    match place {
        Place::Body => request["DefaultPolicy"] = json!(resource_type),
        Place::Details => {
            request["PolicyDetails"] = json!({
                "PolicyLanguage": DEFAULT_POLICY_LANGUAGE,
                "ResourceType": resource_type,
            })
        }
    }
    request
}

/// The other `DefaultPolicyTypeValues` member — the type a change would move to.
fn other_type(resource_type: &str) -> &'static str {
    DEFAULT_POLICY_TYPES
        .iter()
        .map(|(candidate, _)| *candidate)
        .find(|candidate| *candidate != resource_type)
        .expect("DefaultPolicyTypeValues has more than one member")
}

/// Everything the service reports about one policy's resource type: what the
/// read states, the type both projections derive from it, and which
/// resource-type filters select the policy.
///
/// Asserted together because the defect is exactly a disagreement between them:
/// a value only the filter can see is a value stored where no read reports it.
async fn assert_reports_resource_type(
    router: &MultiServiceRouter,
    case: &str,
    id: &str,
    resource_type: &str,
) {
    let expected_type = DEFAULT_POLICY_TYPES
        .iter()
        .find(|(candidate, _)| *candidate == resource_type)
        .map(|(_, policy_type)| *policy_type)
        .expect("the row names a DefaultPolicyTypeValues member");

    let read = get(router, id).await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: the policy must be readable, got {} with {:?}",
        read.status, read.body
    );
    let reported = at(read.policy(), "/PolicyDetails/ResourceType");
    assert_eq!(
        reported,
        json!(resource_type),
        "case {case:?}: expected PolicyDetails.ResourceType {resource_type:?}, got {reported:?}"
    );
    let read_policy_type = at(read.policy(), "/PolicyDetails/PolicyType");
    assert_eq!(
        read_policy_type,
        json!(expected_type),
        "case {case:?}: expected PolicyType {expected_type:?}, got {read_policy_type:?}"
    );

    let listed = list(router, "").await;
    assert_eq!(
        listed.listed_policy_type(id),
        json!(expected_type),
        "case {case:?}: the summary must report PolicyType {expected_type:?}, got {:?}",
        listed.listed_policy_type(id)
    );

    // Every resource type the filter can name, against the one the policy
    // reports: the policy is selected by that type and by no other.
    for (candidate, _) in DEFAULT_POLICY_TYPES {
        for filter in ["resourceTypes", "defaultPolicyType"] {
            let answer = list(router, &format!("?{filter}={candidate}")).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: ?{filter}={candidate} must be answered, got {} with {:?}",
                answer.status, answer.body
            );
            let selected = answer.listed_ids().contains(&id.to_string());
            assert_eq!(
                selected,
                resource_type == candidate,
                "case {case:?}: ?{filter}={candidate} must {} a policy reporting \
                 {resource_type:?}, got {:?}",
                if resource_type == candidate {
                    "select"
                } else {
                    "skip"
                },
                answer.listed_ids()
            );
        }
    }
}

/// A refusal of a parameter: the modelled error, its status, its `Code` and a
/// message naming the member refused.
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
    let message = at(&answer.body, "/Message");
    assert!(
        message.as_str().is_some_and(|text| text.contains(member)),
        "case {case:?}: the message must name {member:?}, got {message:?}"
    );
}

/// Whichever of the two places a create states the resource type in, every read
/// reports that one type and only its own resource-type filters select the
/// policy.
#[tokio::test]
async fn a_default_policys_resource_type_is_one_value_every_read_and_filter_agrees_on() {
    let router = emulator();
    for (resource_type, _) in DEFAULT_POLICY_TYPES {
        for place in [Place::Body, Place::Details] {
            let case = format!("a default {resource_type} policy stated as {}", place.label());
            let created = create(&router, create_request(place, resource_type)).await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: the create must be accepted, got {} with {:?}",
                created.status, created.body
            );
            assert_reports_resource_type(&router, &case, &created.policy_id(), resource_type)
                .await;
        }
    }
}

/// An update may not change the resource type: the configuration structure is
/// "the configuration of the lifecycle policy. You cannot update the policy type
/// or the resource type", so one naming another type is refused and the stored
/// policy is left exactly as it was — in the read AND in the filters.
#[tokio::test]
async fn an_update_may_not_change_a_default_policys_resource_type() {
    let router = emulator();
    for (resource_type, _) in DEFAULT_POLICY_TYPES {
        for place in [Place::Body, Place::Details] {
            let wanted = other_type(resource_type);
            let case = format!(
                "a default {resource_type} policy (stated as {}) updated to {wanted}",
                place.label()
            );
            let id = create(&router, create_request(place, resource_type))
                .await
                .policy_id();

            let refused = update(
                &router,
                &id,
                json!({
                    "PolicyDetails": {
                        "PolicyLanguage": DEFAULT_POLICY_LANGUAGE,
                        "ResourceType": wanted,
                        "CreateInterval": 5,
                    },
                }),
            )
            .await;
            assert_refused(&case, "ResourceType", &refused);

            // Nothing of the refused request took effect: not the type it asked
            // for, and not the configuration it carried beside it.
            assert_reports_resource_type(&router, &case, &id, resource_type).await;
            let read = get(&router, &id).await;
            let create_interval = at(read.policy(), "/PolicyDetails/CreateInterval");
            assert_eq!(
                create_interval,
                json!(1),
                "case {case:?}: a refused update must leave the stored configuration alone, \
                 got CreateInterval {create_interval:?}"
            );
        }
    }
}

/// A configuration that RESTATES the type the policy holds states the policy's
/// own resource type, so it is accepted, applies the rest of the configuration
/// it carries, and leaves the type — and everything derived from it — as it was.
#[tokio::test]
async fn an_update_restating_the_stored_resource_type_is_accepted() {
    let router = emulator();
    for (resource_type, _) in DEFAULT_POLICY_TYPES {
        for place in [Place::Body, Place::Details] {
            let case = format!(
                "a default {resource_type} policy (stated as {}) restating its type",
                place.label()
            );
            let id = create(&router, create_request(place, resource_type))
                .await
                .policy_id();

            let accepted = update(
                &router,
                &id,
                json!({
                    "PolicyDetails": {
                        "PolicyLanguage": DEFAULT_POLICY_LANGUAGE,
                        "ResourceType": resource_type,
                        "CreateInterval": 5,
                        "RetainInterval": 10,
                    },
                }),
            )
            .await;
            assert_eq!(
                accepted.status, 200,
                "case {case:?}: the update must be accepted, got {} with {:?}",
                accepted.status, accepted.body
            );

            assert_reports_resource_type(&router, &case, &id, resource_type).await;
            let read = get(&router, &id).await;
            for (member, expected) in [("CreateInterval", json!(5)), ("RetainInterval", json!(10))] {
                let actual = at(read.policy(), &format!("/PolicyDetails/{member}"));
                assert_eq!(
                    actual, expected,
                    "case {case:?}: the accepted update's {member} must be reported as \
                     {expected:?}, got {actual:?}"
                );
            }
        }
    }
}

/// A create that states the type in NEITHER place describes no kind of default
/// policy, so it is refused and leaves no policy behind: the member documents no
/// value the service fills in, unlike the other `[Default policies only]`
/// members. The rule's own rows live in
/// `default_policy_requires_a_resource_type.rs`.
#[tokio::test]
async fn a_create_stating_the_resource_type_in_neither_place_is_refused() {
    let router = emulator();
    let case = "a SIMPLIFIED configuration stating no resource type";
    let before = policy_count(&router).await;
    let answer = create(
        &router,
        json!({
            "ExecutionRoleArn": role_arn(),
            "Description": "a default policy stating no resource type",
            "State": "ENABLED",
            "PolicyDetails": {"PolicyLanguage": DEFAULT_POLICY_LANGUAGE},
        }),
    )
    .await;
    assert_refused(case, "ResourceType", &answer);
    assert_eq!(
        policy_count(&router).await,
        before,
        "case {case:?}: a refused create must leave no policy behind"
    );
}

/// One request may not state the resource type in both places: `DefaultPolicy`
/// and `PolicyDetails$ResourceType` are the body form and the structure form of
/// one member, which `CreateLifecyclePolicy` documents as alternatives, "but not
/// both" — whether the two agree or not.
#[tokio::test]
async fn a_create_may_not_state_the_resource_type_in_both_places() {
    let router = emulator();
    for (body_type, _) in DEFAULT_POLICY_TYPES {
        for (details_type, _) in DEFAULT_POLICY_TYPES {
            let case = format!("DefaultPolicy {body_type} beside ResourceType {details_type}");
            let before = policy_count(&router).await;
            let answer = create(
                &router,
                json!({
                    "ExecutionRoleArn": role_arn(),
                    "Description": "a default policy stated twice",
                    "State": "ENABLED",
                    "DefaultPolicy": body_type,
                    "PolicyDetails": {
                        "PolicyLanguage": DEFAULT_POLICY_LANGUAGE,
                        "ResourceType": details_type,
                    },
                }),
            )
            .await;
            assert_refused(&case, "DefaultPolicy", &answer);
            let conflicting = at(&answer.body, "/MutuallyExclusiveParameters");
            for parameter in ["DefaultPolicy", "PolicyDetails"] {
                assert!(
                    conflicting
                        .as_array()
                        .is_some_and(|names| names.contains(&json!(parameter))),
                    "case {case:?}: MutuallyExclusiveParameters must name {parameter:?}, got \
                     {conflicting:?}"
                );
            }
            assert_eq!(
                policy_count(&router).await,
                before,
                "case {case:?}: a refused create must leave no policy behind"
            );
        }
    }
}

/// Two policies holding the same resource type report the same thing about it,
/// however each of them was written: the place a request stated the type in is
/// not part of what is stored.
#[tokio::test]
async fn identically_typed_policies_report_the_same_type_however_they_were_stated() {
    let router = emulator();
    for (resource_type, expected_type) in DEFAULT_POLICY_TYPES {
        let mut reported: Vec<(Place, Value, Value)> = Vec::new();
        for place in [Place::Body, Place::Details] {
            let id = create(&router, create_request(place, resource_type))
                .await
                .policy_id();
            let read = get(&router, &id).await;
            reported.push((
                place,
                at(read.policy(), "/PolicyDetails/ResourceType"),
                at(read.policy(), "/PolicyDetails/PolicyType"),
            ));
        }
        let [first, second] = reported.as_slice() else {
            panic!("one reading per place");
        };
        let (first_place, first_type, first_policy_type) = first;
        let (second_place, second_type, second_policy_type) = second;
        let case = format!("a default {resource_type} policy");
        assert_eq!(
            first_type, second_type,
            "case {case:?}: {} and {} must report the same ResourceType, got {first_type:?} and \
             {second_type:?}",
            first_place.label(),
            second_place.label()
        );
        assert_eq!(
            first_policy_type,
            &json!(expected_type),
            "case {case:?}: {} must report PolicyType {expected_type:?}, got \
             {first_policy_type:?}",
            first_place.label()
        );
        assert_eq!(
            second_policy_type,
            &json!(expected_type),
            "case {case:?}: {} must report PolicyType {expected_type:?}, got \
             {second_policy_type:?}",
            second_place.label()
        );
    }
}
