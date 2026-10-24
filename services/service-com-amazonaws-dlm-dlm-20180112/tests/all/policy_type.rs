// SPDX-License-Identifier: CC0-1.0

//! Which `PolicyTypeValues` a lifecycle policy is reported as, end to end over
//! HTTP.
//!
//! The rule under test: a policy's type is the one its configuration STATES in
//! `PolicyDetails$PolicyType`, and a configuration that states none carries that
//! member's documented default (`EBS_SNAPSHOT_MANAGEMENT`) — the type is never
//! derived from the resource the policy TARGETS, because a target says nothing
//! about what the policy manages (`PolicyDetails$ResourceTypes` documents
//! `INSTANCE` as "create multi-volume snapshots from the volumes for an
//! instance"). The ONE policy whose target IS the statement of what it manages is
//! a DEFAULT policy, which configures no type of its own: `DefaultPolicy`
//! `INSTANCE` "creates EBS-backed AMIs" and `VOLUME` creates snapshots.
//!
//! Both reads must agree, since they project ONE stored record:
//! `GetLifecyclePolicy`'s `Policy.PolicyDetails.PolicyType` and
//! `GetLifecyclePolicies`' `Policies[*].PolicyType`.
//!
//! The rows span what the rule generalises over: whether the configuration states
//! a type at all, which type it states, which resource type it targets, custom
//! against default policy, which way a default policy's type was stated, the
//! event-based configuration that targets nothing, and the create path against the
//! update path.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for.
const REGION: &str = "eu-west-1";

/// An `ExecutionRoleArn` satisfying `^arn:aws(-[a-z]{1,3}){0,2}:iam::\d+:role/`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AWSDataLifecycleManagerDefaultRole";

/// `PolicyDetails$PolicyType`: "The default is `EBS_SNAPSHOT_MANAGEMENT`."
const SNAPSHOT_POLICY_TYPE: &str = "EBS_SNAPSHOT_MANAGEMENT";

/// The `PolicyTypeValues` member that "manages the lifecycle of EBS-backed AMIs".
const IMAGE_POLICY_TYPE: &str = "IMAGE_MANAGEMENT";

/// The `PolicyTypeValues` member of a policy that "performs specific actions when
/// a defined event occurs".
const EVENT_BASED_POLICY_TYPE: &str = "EVENT_BASED_POLICY";

/// Every `PolicyTypeValues` member, from
/// `smithy_query(query="shape", shape="PolicyTypeValues")`; a row below states
/// each one, so no member of the catalog is left untested.
const POLICY_TYPE_VALUES: &[&str] = &[
    SNAPSHOT_POLICY_TYPE,
    IMAGE_POLICY_TYPE,
    EVENT_BASED_POLICY_TYPE,
];

/// Every `DefaultPolicyTypeValues` member read against the `PolicyTypeValues` a
/// default policy of that type manages.
///
/// Source: `CreateLifecyclePolicyRequest$DefaultPolicy` / `PolicyDetails$ResourceType`
/// ("To create a default policy for EBS snapshots ... specify `VOLUME`", "To
/// create a default policy for EBS-backed AMIs, that creates EBS-backed AMIs from
/// all instances in the Region ... specify `INSTANCE`").
const DEFAULT_POLICY_TYPES: &[(&str, &str)] = &[
    ("VOLUME", SNAPSHOT_POLICY_TYPE),
    ("INSTANCE", IMAGE_POLICY_TYPE),
];

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

/// Create the policy `request` describes and answer its id.
async fn create(router: &MultiServiceRouter, case: &str, request: Value) -> String {
    let answer = call(router, "POST", "/policies", Some(request)).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from CreateLifecyclePolicy, got {} with {:?}",
        answer.status, answer.body
    );
    answer
        .body
        .pointer("/PolicyId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: the create must report a PolicyId"))
        .to_string()
}

/// The `PolicyType` `GetLifecyclePolicy` reports inside the policy's stored
/// configuration.
async fn read_policy_type(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let answer = call(router, "GET", &format!("/policies/{id}"), None).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from GetLifecyclePolicy, got {} with {:?}",
        answer.status, answer.body
    );
    answer
        .body
        .pointer("/Policy/PolicyDetails/PolicyType")
        .cloned()
        .unwrap_or(Value::Null)
}

/// The `PolicyType` the summary for `id` reports, from a `GetLifecyclePolicies`
/// call made with `query`.
async fn listed_policy_type(
    router: &MultiServiceRouter,
    case: &str,
    query: &str,
    id: &str,
) -> Value {
    let answer = call(router, "GET", &format!("/policies{query}"), None).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from GetLifecyclePolicies, got {} with {:?}",
        answer.status, answer.body
    );
    let summaries = answer.body["Policies"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: GetLifecyclePolicies must return a Policies list, got {:?}",
                answer.body
            )
        })
        .clone();
    let summary = summaries
        .iter()
        .find(|summary| summary["PolicyId"] == json!(id))
        .unwrap_or_else(|| panic!("case {case:?}: {id} is missing from {summaries:?}"));
    summary
        .pointer("/PolicyType")
        .cloned()
        .unwrap_or(Value::Null)
}

/// A custom snapshot/AMI configuration targeting `resource_type`, stating
/// `policy_type` only when one is given.
fn custom_details(policy_type: Option<&str>, resource_type: &str) -> Value {
    let mut details = json!({
        // `ResourceTypeValuesList` is `@length(min: 1, max: 1)`, so a custom
        // policy targets exactly one resource type.
        "ResourceTypes": [resource_type],
        "TargetTags": [{"Key": "backup", "Value": "daily"}],
        "Schedules": [{
            "Name": "daily",
            "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS"},
            "RetainRule": {"Count": 7},
        }],
    });
    if let Some(policy_type) = policy_type {
        details["PolicyType"] = json!(policy_type);
    }
    details
}

/// An event-based configuration, which targets no resource type at all.
fn event_based_details() -> Value {
    json!({
        "PolicyType": EVENT_BASED_POLICY_TYPE,
        "EventSource": {
            "Type": "MANAGED_CWE",
            "Parameters": {
                "EventType": "shareSnapshot",
                "SnapshotOwner": ["000000000000"],
                "DescriptionRegex": "^.*$",
            },
        },
        "Actions": [{
            "Name": "copy",
            "CrossRegionCopy": [{
                "Target": "us-west-2",
                "EncryptionConfiguration": {"Encrypted": false},
                "RetainRule": {"Interval": 1, "IntervalUnit": "DAYS"},
            }],
        }],
    })
}

/// One row: the create request, and the `PolicyTypeValues` both reads must report
/// for the policy it makes.
struct Case {
    case: String,
    request: Value,
    expected: &'static str,
}

/// The table: every way a policy's type can be stated or left unstated.
fn cases() -> Vec<Case> {
    let mut cases: Vec<Case> = Vec::new();

    // A CUSTOM policy: the stated type when it states one, the documented
    // default when it does not — for either resource type it may target. The
    // `(stated type, target)` pairs are every combination the documentation
    // admits: `IMAGE_MANAGEMENT` "manages the lifecycle of EBS-backed AMIs", so
    // an AMI policy targets instances.
    let custom: &[(Option<&str>, &str, &str)] = &[
        (None, "VOLUME", SNAPSHOT_POLICY_TYPE),
        (None, "INSTANCE", SNAPSHOT_POLICY_TYPE),
        (Some(SNAPSHOT_POLICY_TYPE), "VOLUME", SNAPSHOT_POLICY_TYPE),
        (Some(SNAPSHOT_POLICY_TYPE), "INSTANCE", SNAPSHOT_POLICY_TYPE),
        (Some(IMAGE_POLICY_TYPE), "INSTANCE", IMAGE_POLICY_TYPE),
    ];
    for (stated, resource_type, expected) in custom {
        cases.push(Case {
            case: format!(
                "a custom policy targeting {resource_type} stating PolicyType {stated:?}"
            ),
            request: json!({
                "ExecutionRoleArn": ROLE_ARN,
                "Description": format!("custom {resource_type} {}", stated.unwrap_or("default")),
                "State": "ENABLED",
                "PolicyDetails": custom_details(*stated, resource_type),
            }),
            expected,
        });
    }

    // The same, with the custom policy language stated explicitly: naming the
    // language is not naming a type.
    let mut with_language = custom_details(None, "INSTANCE");
    with_language["PolicyLanguage"] = json!("STANDARD");
    cases.push(Case {
        case: "a STANDARD policy targeting INSTANCE stating no PolicyType".to_string(),
        request: json!({
            "ExecutionRoleArn": ROLE_ARN,
            "Description": "standard language no type",
            "State": "ENABLED",
            "PolicyDetails": with_language,
        }),
        expected: SNAPSHOT_POLICY_TYPE,
    });

    // An event-based policy, which configures no target and states its own type.
    cases.push(Case {
        case: "an event-based policy".to_string(),
        request: json!({
            "ExecutionRoleArn": ROLE_ARN,
            "Description": "event based copies",
            "State": "ENABLED",
            "PolicyDetails": event_based_details(),
        }),
        expected: EVENT_BASED_POLICY_TYPE,
    });

    // A DEFAULT policy, whose type follows the kind of default policy it is —
    // stated either as the `DefaultPolicy` body parameter or as the
    // `[Default policies only]` `PolicyDetails$ResourceType`.
    for (default_type, expected) in DEFAULT_POLICY_TYPES {
        cases.push(Case {
            case: format!("a default {default_type} policy stated as DefaultPolicy"),
            request: json!({
                "ExecutionRoleArn": ROLE_ARN,
                "Description": format!("default {default_type} policy"),
                "State": "ENABLED",
                "DefaultPolicy": default_type,
            }),
            expected,
        });
        cases.push(Case {
            case: format!("a default {default_type} policy stated as PolicyDetails"),
            request: json!({
                "ExecutionRoleArn": ROLE_ARN,
                "Description": format!("simplified {default_type} policy"),
                "State": "ENABLED",
                "PolicyDetails": {
                    "PolicyLanguage": "SIMPLIFIED",
                    "ResourceType": default_type,
                },
            }),
            expected,
        });
        // The type of default policy named in the request body, beside a
        // configuration structure that names the default-policy language and
        // nothing else: the kind of default policy it is still decides the type.
        cases.push(Case {
            case: format!("a default {default_type} policy with a SIMPLIFIED structure"),
            request: json!({
                "ExecutionRoleArn": ROLE_ARN,
                "Description": format!("default {default_type} policy and language"),
                "State": "ENABLED",
                "DefaultPolicy": default_type,
                "PolicyDetails": {"PolicyLanguage": "SIMPLIFIED"},
            }),
            expected,
        });
    }

    cases
}

/// Both reads report the type the policy's configuration states, or the
/// documented default when it states none.
#[tokio::test]
async fn a_policys_reported_type_is_the_one_its_configuration_states() {
    let router = emulator();
    let mut covered: Vec<&str> = Vec::new();

    for Case {
        case,
        request,
        expected,
    } in cases()
    {
        let case = case.as_str();
        let id = create(&router, case, request).await;

        let read = read_policy_type(&router, case, &id).await;
        assert_eq!(
            read,
            json!(expected),
            "case {case:?}: GetLifecyclePolicy must report PolicyType {expected:?}, got {read:?}"
        );

        let listed = listed_policy_type(&router, case, "", &id).await;
        assert_eq!(
            listed,
            json!(expected),
            "case {case:?}: GetLifecyclePolicies must report PolicyType {expected:?}, \
             got {listed:?}"
        );

        covered.push(expected);
    }

    // The table states every member of the catalog, so none of them is reported
    // by a rule no case exercises.
    for policy_type in POLICY_TYPE_VALUES {
        assert!(
            covered.contains(policy_type),
            "the table must cover PolicyTypeValues {policy_type:?}, covered {covered:?}"
        );
    }
}

/// A snapshot policy that targets instances is distinguishable from an AMI
/// policy: both are found by the `resourceTypes` filter — which is about what a
/// policy TARGETS — while their reported types differ.
#[tokio::test]
async fn a_multi_volume_snapshot_policy_is_not_reported_as_an_ami_policy() {
    let router = emulator();

    let snapshot_case = "a custom policy targeting INSTANCE with no PolicyType";
    let snapshot = create(
        &router,
        snapshot_case,
        json!({
            "ExecutionRoleArn": ROLE_ARN,
            "Description": "multi volume snapshots",
            "State": "ENABLED",
            "PolicyDetails": custom_details(None, "INSTANCE"),
        }),
    )
    .await;

    let image_case = "a custom policy targeting INSTANCE stating IMAGE_MANAGEMENT";
    let image = create(
        &router,
        image_case,
        json!({
            "ExecutionRoleArn": ROLE_ARN,
            "Description": "ami management",
            "State": "ENABLED",
            "PolicyDetails": custom_details(Some(IMAGE_POLICY_TYPE), "INSTANCE"),
        }),
    )
    .await;

    // The filter selects on the target, so it reports both policies...
    let snapshot_listed =
        listed_policy_type(&router, snapshot_case, "?resourceTypes=INSTANCE", &snapshot).await;
    let image_listed =
        listed_policy_type(&router, image_case, "?resourceTypes=INSTANCE", &image).await;
    // ...and the two summaries report different types.
    assert_eq!(
        snapshot_listed,
        json!(SNAPSHOT_POLICY_TYPE),
        "case {snapshot_case:?}: expected PolicyType {SNAPSHOT_POLICY_TYPE:?}, \
         got {snapshot_listed:?}"
    );
    assert_eq!(
        image_listed,
        json!(IMAGE_POLICY_TYPE),
        "case {image_case:?}: expected PolicyType {IMAGE_POLICY_TYPE:?}, got {image_listed:?}"
    );
    assert_ne!(
        snapshot_listed, image_listed,
        "the two policies must be distinguishable by PolicyType, both reported \
         {snapshot_listed:?}"
    );
}

/// The rule holds on the UPDATE path too: a replacement configuration is reported
/// with the type IT states, or the documented default when it states none.
#[tokio::test]
async fn a_replaced_configuration_reports_the_type_it_states() {
    let router = emulator();

    let cases: &[(&str, Value, Value, &str)] = &[
        (
            "an AMI policy replaced by a configuration stating no type",
            json!(custom_details(Some(IMAGE_POLICY_TYPE), "INSTANCE")),
            json!(custom_details(None, "INSTANCE")),
            SNAPSHOT_POLICY_TYPE,
        ),
        (
            "a snapshot policy replaced by a configuration stating IMAGE_MANAGEMENT",
            json!(custom_details(None, "VOLUME")),
            json!(custom_details(Some(IMAGE_POLICY_TYPE), "INSTANCE")),
            IMAGE_POLICY_TYPE,
        ),
        (
            "a snapshot policy replaced by another stating no type",
            json!(custom_details(None, "INSTANCE")),
            json!(custom_details(None, "VOLUME")),
            SNAPSHOT_POLICY_TYPE,
        ),
    ];

    for (case, created, replacement, expected) in cases {
        let id = create(
            &router,
            case,
            json!({
                "ExecutionRoleArn": ROLE_ARN,
                "Description": "replaced configuration",
                "State": "ENABLED",
                "PolicyDetails": created,
            }),
        )
        .await;

        let patched = call(
            &router,
            "PATCH",
            &format!("/policies/{id}"),
            Some(json!({"PolicyDetails": replacement})),
        )
        .await;
        assert_eq!(
            patched.status, 200,
            "case {case:?}: expected HTTP 200 from UpdateLifecyclePolicy, got {} with {:?}",
            patched.status, patched.body
        );

        let read = read_policy_type(&router, case, &id).await;
        assert_eq!(
            read,
            json!(expected),
            "case {case:?}: GetLifecyclePolicy must report PolicyType {expected:?} after the \
             update, got {read:?}"
        );
        let listed = listed_policy_type(&router, case, "", &id).await;
        assert_eq!(
            listed,
            json!(expected),
            "case {case:?}: GetLifecyclePolicies must report PolicyType {expected:?} after the \
             update, got {listed:?}"
        );
    }
}
