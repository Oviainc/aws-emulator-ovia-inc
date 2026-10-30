// SPDX-License-Identifier: CC0-1.0

//! What an `UpdateLifecyclePolicy` may not change about a stored policy, end to
//! end over HTTP.
//!
//! The rule under test: `UpdateLifecyclePolicyRequest$PolicyDetails` is "the
//! configuration of the lifecycle policy. You cannot update the policy type or
//! the resource type"
//! (`smithy_query(query="doc", shape="UpdateLifecyclePolicyRequest",
//! member="PolicyDetails")`), so the members of a policy's identity are fixed
//! when it is created: a replacement configuration naming a different value for
//! one of them is refused with the modelled client error and writes nothing,
//! while one that restates the value the policy holds — or states none — is
//! applied and the policy keeps that value in both read projections and under
//! the `resourceTypes` filter.
//!
//! Which members state it, from `smithy_query(query="shape",
//! shape="PolicyDetails")`: `PolicyType` ("The type of policy"), `ResourceTypes`
//! ("[Custom snapshot policies only] The target resource type for snapshot and
//! AMI lifecycle policies") and `ResourceType` ("[Default policies only] ... the
//! type of default policy"). `policy_language_immutability.rs` owns the same rule
//! for `PolicyLanguage`, the member that names which KIND of policy a
//! configuration describes.
//!
//! The rows span what the rule generalises over: every kind of policy this
//! service stores (a custom snapshot policy under either target, a custom AMI
//! policy, a custom event-based policy and a default policy of either
//! `DefaultPolicyTypeValues` member), every member of `PolicyTypeValues` and of
//! `ResourceTypeValues` as the value asked for, and the two accepted forms — the
//! policy's own value restated, and no value stated at all.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for.
const REGION: &str = "eu-central-1";

/// A destination Region for the event-based policy's copy action, matching
/// `TargetRegion`'s `^([a-z]+-){2,3}\d$`.
const OTHER_REGION: &str = "us-west-2";

/// The modelled client error this service answers for a refused parameter, its
/// `@httpError` status and its `Code`.
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";
const INVALID_REQUEST_STATUS: u16 = 400;
const INVALID_PARAMETER_CODE: &str = "InvalidParameter";

/// Every `PolicyTypeValues` member, from
/// `smithy_query(query="shape", shape="PolicyTypeValues")`.
const POLICY_TYPE_VALUES: &[&str] = &[
    "EBS_SNAPSHOT_MANAGEMENT",
    "IMAGE_MANAGEMENT",
    "EVENT_BASED_POLICY",
];

/// Every `ResourceTypeValues` member, from
/// `smithy_query(query="shape", shape="ResourceTypeValues")` — the same two
/// members `DefaultPolicyTypeValues` carries.
const RESOURCE_TYPE_VALUES: &[&str] = &["VOLUME", "INSTANCE"];

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
        self.body["Message"].as_str().unwrap_or_default()
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

async fn create(router: &MultiServiceRouter, case: &str, request: Value) -> String {
    let answer = call(router, "POST", "/policies", Some(request)).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from CreateLifecyclePolicy, got {} with {:?}",
        answer.status, answer.body
    );
    answer.policy_id()
}

async fn get(router: &MultiServiceRouter, case: &str, id: &str) -> Answer {
    let answer = call(router, "GET", &format!("/policies/{id}"), None).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from GetLifecyclePolicy, got {} with {:?}",
        answer.status, answer.body
    );
    answer
}

async fn update(router: &MultiServiceRouter, id: &str, request: Value) -> Answer {
    call(router, "PATCH", &format!("/policies/{id}"), Some(request)).await
}

/// The `LifecyclePolicySummary` `GetLifecyclePolicies` reports for `id` under
/// `query`, or `Value::Null` when the filter does not select it.
async fn summary(router: &MultiServiceRouter, case: &str, query: &str, id: &str) -> Value {
    let answer = call(router, "GET", &format!("/policies{query}"), None).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from GetLifecyclePolicies, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["Policies"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .find(|policy| policy["PolicyId"] == json!(id))
        .cloned()
        .unwrap_or(Value::Null)
}

fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// One stored policy, with the identity it keeps and a replacement configuration
/// of its own kind.
struct Subject {
    /// What is particular about this row.
    case: &'static str,
    /// The create request that makes the policy.
    create: Value,
    /// The `PolicyTypeValues` member both reads report for it.
    policy_type: &'static str,
    /// The `ResourceTypeValues` member the `resourceTypes` filter selects it
    /// under, or `None` for a policy that targets no resource (an event-based
    /// one).
    target: Option<&'static str>,
    /// True when it is a DEFAULT policy, which states its target as
    /// `PolicyDetails$ResourceType` rather than as `ResourceTypes`.
    default_policy: bool,
    /// A complete replacement configuration for this policy, stating its own
    /// type and target: an update's structure replaces the configuration
    /// wholesale, so it has to be as complete as a create's.
    replacement: Value,
}

/// A custom snapshot policy's configuration targeting `target`. The creation
/// interval stays inside the supported set (`interval_validation.rs` owns that
/// rule).
fn custom_snapshot_details(target: &str, interval: i64) -> Value {
    json!({
        "PolicyType": "EBS_SNAPSHOT_MANAGEMENT",
        // `ResourceTypeValuesList` is `@length(min: 1, max: 1)`, so a custom
        // policy targets exactly one resource type.
        "ResourceTypes": [target],
        "TargetTags": [{"Key": "backup", "Value": "daily"}],
        "Schedules": [{
            "Name": "daily",
            "CreateRule": {"Interval": interval, "IntervalUnit": "HOURS"},
            "RetainRule": {"Count": 7},
        }],
    })
}

/// A custom AMI policy's configuration, which "manages the lifecycle of
/// EBS-backed AMIs" and so targets instances.
fn custom_image_details(interval: i64) -> Value {
    let mut details = custom_snapshot_details("INSTANCE", interval);
    details["PolicyType"] = json!("IMAGE_MANAGEMENT");
    details
}

/// An event-based policy's configuration, which targets no resource and
/// configures no schedule.
fn event_based_details(description_regex: &str) -> Value {
    json!({
        "PolicyType": "EVENT_BASED_POLICY",
        "EventSource": {
            "Type": "MANAGED_CWE",
            "Parameters": {
                "EventType": "shareSnapshot",
                "SnapshotOwner": ["000000000000"],
                "DescriptionRegex": description_regex,
            },
        },
        "Actions": [{
            "Name": "copy",
            "CrossRegionCopy": [{
                "Target": OTHER_REGION,
                "EncryptionConfiguration": {"Encrypted": false},
                "RetainRule": {"Interval": 1, "IntervalUnit": "DAYS"},
            }],
        }],
    })
}

/// A default policy's replacement configuration: its own kind's language, its
/// own target, and a cadence inside the documented ranges (a creation frequency
/// of 1 to 7 days, a retention period of 2 to 14 days greater than it).
fn default_policy_details(target: &str, policy_type: &str) -> Value {
    json!({
        "PolicyLanguage": "SIMPLIFIED",
        "PolicyType": policy_type,
        "ResourceType": target,
        "CreateInterval": 3,
        "RetainInterval": 14,
    })
}

fn custom_policy(description: &str, details: Value) -> Value {
    json!({
        "ExecutionRoleArn": role_arn(),
        "Description": description,
        "State": "ENABLED",
        "PolicyDetails": details,
    })
}

/// The table: every kind of policy this service stores.
fn subjects() -> Vec<Subject> {
    vec![
        Subject {
            case: "a custom snapshot policy targeting VOLUME",
            create: custom_policy("volume snapshots", custom_snapshot_details("VOLUME", 24)),
            policy_type: "EBS_SNAPSHOT_MANAGEMENT",
            target: Some("VOLUME"),
            default_policy: false,
            replacement: custom_snapshot_details("VOLUME", 12),
        },
        Subject {
            case: "a custom multi-volume snapshot policy targeting INSTANCE",
            create: custom_policy(
                "multi volume snapshots",
                custom_snapshot_details("INSTANCE", 24),
            ),
            policy_type: "EBS_SNAPSHOT_MANAGEMENT",
            target: Some("INSTANCE"),
            default_policy: false,
            replacement: custom_snapshot_details("INSTANCE", 8),
        },
        Subject {
            case: "a custom AMI policy",
            create: custom_policy("ami management", custom_image_details(24)),
            policy_type: "IMAGE_MANAGEMENT",
            target: Some("INSTANCE"),
            default_policy: false,
            replacement: custom_image_details(6),
        },
        Subject {
            case: "a custom event-based policy",
            create: custom_policy("event based copies", event_based_details("^.*$")),
            policy_type: "EVENT_BASED_POLICY",
            target: None,
            default_policy: false,
            replacement: event_based_details("^share-.*$"),
        },
        Subject {
            case: "a default VOLUME policy created from the request body",
            create: json!({
                "ExecutionRoleArn": role_arn(),
                "Description": "default volume policy",
                "State": "ENABLED",
                "DefaultPolicy": "VOLUME",
            }),
            // A default policy configures no type of its own: `VOLUME` "creates
            // snapshots of all volumes in the Region".
            policy_type: "EBS_SNAPSHOT_MANAGEMENT",
            target: Some("VOLUME"),
            default_policy: true,
            replacement: default_policy_details("VOLUME", "EBS_SNAPSHOT_MANAGEMENT"),
        },
        Subject {
            case: "a default INSTANCE policy created from a SIMPLIFIED structure",
            create: json!({
                "ExecutionRoleArn": role_arn(),
                "Description": "simplified instance policy",
                "State": "ENABLED",
                "PolicyDetails": {
                    "PolicyLanguage": "SIMPLIFIED",
                    "ResourceType": "INSTANCE",
                },
            }),
            // `INSTANCE` "creates EBS-backed AMIs from all instances in the
            // Region".
            policy_type: "IMAGE_MANAGEMENT",
            target: Some("INSTANCE"),
            default_policy: true,
            replacement: default_policy_details("INSTANCE", "IMAGE_MANAGEMENT"),
        },
    ]
}

/// `details` with `member` set to `value`.
fn stating(details: &Value, member: &str, value: Value) -> Value {
    let mut details = details.clone();
    details[member] = value;
    details
}

/// `details` with `member` left out entirely.
fn without(details: &Value, member: &str) -> Value {
    let mut details = details.clone();
    details
        .as_object_mut()
        .expect("a configuration is a JSON object")
        .remove(member);
    details
}

/// One refused update: the member it states, the value it asks for, the value the
/// policy holds, and the replacement configuration carrying it.
struct Refusal {
    member: &'static str,
    stated: String,
    stored: String,
    details: Value,
}

/// Every refusal the rule produces for `subject`: each member of
/// `PolicyTypeValues` other than the policy's own type, and each member of
/// `ResourceTypeValues` other than its own target, in whichever member states
/// the target for this kind of policy.
fn refusals(subject: &Subject) -> Vec<Refusal> {
    let mut refusals: Vec<Refusal> = Vec::new();

    for stated in POLICY_TYPE_VALUES
        .iter()
        .filter(|stated| **stated != subject.policy_type)
    {
        refusals.push(Refusal {
            member: "PolicyType",
            stated: (*stated).to_string(),
            stored: subject.policy_type.to_string(),
            details: stating(&subject.replacement, "PolicyType", json!(stated)),
        });
    }

    let Some(target) = subject.target else {
        return refusals;
    };
    for stated in RESOURCE_TYPE_VALUES
        .iter()
        .filter(|stated| **stated != target)
    {
        let (member, path, value) = if subject.default_policy {
            ("ResourceType", "ResourceType", json!(stated))
        } else {
            (
                "ResourceTypes",
                "ResourceTypes",
                json!([stated]),
            )
        };
        refusals.push(Refusal {
            member: path,
            stated: (*stated).to_string(),
            stored: target.to_string(),
            details: stating(&subject.replacement, member, value),
        });
    }

    refusals
}

/// An update whose replacement configuration names another policy type or
/// another resource type is refused, and writes nothing.
#[tokio::test]
async fn an_update_cannot_change_a_policys_type_or_resource_type() {
    let router = emulator();
    let mut refused_types: Vec<String> = Vec::new();
    let mut refused_targets: Vec<String> = Vec::new();

    for subject in subjects() {
        let id = create(&router, subject.case, subject.create.clone()).await;
        let before = get(&router, subject.case, &id).await;
        let before_summary = summary(&router, subject.case, "", &id).await;

        for Refusal {
            member,
            stated,
            stored,
            details,
        } in refusals(&subject)
        {
            let case = format!(
                "{} updated with a configuration stating {member} {stated:?}",
                subject.case
            );
            let case = case.as_str();

            let answer = update(&router, &id, json!({"PolicyDetails": details})).await;
            assert_eq!(
                answer.status, INVALID_REQUEST_STATUS,
                "case {case:?}: the policy's value is {stored:?}, so the update must answer \
                 HTTP {INVALID_REQUEST_STATUS}, got {} with {:?}",
                answer.status, answer.body
            );
            assert!(
                answer.error_code().ends_with(INVALID_REQUEST_EXCEPTION),
                "case {case:?}: expected {INVALID_REQUEST_EXCEPTION}, got {:?}",
                answer.error_code()
            );
            assert_eq!(
                at(&answer.body, "/Code"),
                json!(INVALID_PARAMETER_CODE),
                "case {case:?}: expected the {INVALID_PARAMETER_CODE:?} code, got {:?}",
                at(&answer.body, "/Code")
            );
            // The refusal names the parameter the update may not change, and
            // nothing of the values the request stated or the policy holds: the
            // wording is the same for every policy, account and region.
            let expected = format!("The following parameter(s) cannot be updated: {member}");
            assert_eq!(
                answer.message(),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                answer.message()
            );
            for value in [stored.as_str(), stated.as_str()] {
                assert!(
                    !answer.message().contains(value),
                    "case {case:?}: the refusal must not carry the value {value:?}, got {:?}",
                    answer.message()
                );
            }

            // Refused before the first write: the whole projection, and the
            // summary the list reports, are what they were.
            let after = get(&router, case, &id).await;
            assert_eq!(
                after.policy(),
                before.policy(),
                "case {case:?}: a refused update must leave the stored policy exactly as it was"
            );
            assert_eq!(
                summary(&router, case, "", &id).await,
                before_summary,
                "case {case:?}: a refused update must leave the policy's summary as it was"
            );

            if member.ends_with("PolicyType") {
                refused_types.push(stated.clone());
            } else {
                refused_targets.push(stated.clone());
                // The `resourceTypes` filter selects on what a policy targets,
                // so a refused update cannot have moved the policy between
                // filters.
                assert_ne!(
                    summary(&router, case, &format!("?resourceTypes={stored}"), &id).await,
                    Value::Null,
                    "case {case:?}: the policy must still be selected by resourceTypes={stored}"
                );
                assert_eq!(
                    summary(&router, case, &format!("?resourceTypes={stated}"), &id).await,
                    Value::Null,
                    "case {case:?}: the policy must not be selected by resourceTypes={stated}"
                );
            }
        }
    }

    // Every member of both catalogs is refused by some row, so no value is
    // accepted by a rule no case exercises.
    for policy_type in POLICY_TYPE_VALUES {
        assert!(
            refused_types.iter().any(|stated| stated == policy_type),
            "the table must refuse PolicyType {policy_type:?} on some policy, refused \
             {refused_types:?}"
        );
    }
    for resource_type in RESOURCE_TYPE_VALUES {
        assert!(
            refused_targets.iter().any(|stated| stated == resource_type),
            "the table must refuse a resource type of {resource_type:?} on some policy, refused \
             {refused_targets:?}"
        );
    }
}

/// An update whose replacement configuration restates the policy's own type, or
/// states none at all, is applied — and the policy keeps the type and the target
/// it was created with, in both read projections and under the `resourceTypes`
/// filter.
#[tokio::test]
async fn an_accepted_update_keeps_the_identity_the_policy_was_created_with() {
    let router = emulator();
    let mut covered: Vec<&str> = Vec::new();

    for subject in subjects() {
        for restates_type in [true, false] {
            let case = format!(
                "{} updated with a configuration {}",
                subject.case,
                if restates_type {
                    "restating its own PolicyType"
                } else {
                    "stating no PolicyType"
                }
            );
            let case = case.as_str();
            let id = create(&router, case, subject.create.clone()).await;

            let details = if restates_type {
                subject.replacement.clone()
            } else {
                without(&subject.replacement, "PolicyType")
            };
            let answer = update(&router, &id, json!({"PolicyDetails": details})).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: the update must be accepted, got {} with {:?}",
                answer.status, answer.body
            );

            // The documented default (`PolicyDetails$PolicyType`: "The default is
            // EBS_SNAPSHOT_MANAGEMENT") is a create-time default, so it is never
            // stamped over the type a policy already has.
            let read = get(&router, case, &id).await;
            assert_eq!(
                at(read.policy(), "/PolicyDetails/PolicyType"),
                json!(subject.policy_type),
                "case {case:?}: expected PolicyType {:?}, got {:?}",
                subject.policy_type,
                at(read.policy(), "/PolicyDetails/PolicyType")
            );
            let listed = summary(&router, case, "", &id).await;
            assert_eq!(
                at(&listed, "/PolicyType"),
                json!(subject.policy_type),
                "case {case:?}: the summary must report PolicyType {:?}, got {:?}",
                subject.policy_type,
                at(&listed, "/PolicyType")
            );

            // And the target, in the member the kind of policy states it in.
            if let Some(target) = subject.target {
                let pointer = if subject.default_policy {
                    "/PolicyDetails/ResourceType"
                } else {
                    "/PolicyDetails/ResourceTypes"
                };
                let expected = if subject.default_policy {
                    json!(target)
                } else {
                    json!([target])
                };
                assert_eq!(
                    at(read.policy(), pointer),
                    expected,
                    "case {case:?}: expected {pointer} to report {expected:?}, got {:?}",
                    at(read.policy(), pointer)
                );
                assert_ne!(
                    summary(&router, case, &format!("?resourceTypes={target}"), &id).await,
                    Value::Null,
                    "case {case:?}: the policy must still be selected by resourceTypes={target}"
                );
            }

            covered.push(subject.policy_type);
        }
    }

    // Every member of the catalog is the stored type of some row.
    for policy_type in POLICY_TYPE_VALUES {
        assert!(
            covered.contains(policy_type),
            "the table must store a policy whose type is {policy_type:?}, covered {covered:?}"
        );
    }
}
