// SPDX-License-Identifier: CC0-1.0

//! What the control plane does with a configuration member the model documents
//! `[Default policies only]`, end to end over HTTP.
//!
//! The rule under test: such a member is accepted only by a request creating or
//! updating a DEFAULT policy, wherever the request states it — in the request body
//! or inside the `PolicyDetails` structure — and a request that states one for a
//! policy that is not a default policy is refused with `InvalidRequestException`
//! (HTTP 400) and stores nothing. A default policy, conversely, reports every one
//! of them back through `GetLifecyclePolicy`'s `PolicyDetails`.
//!
//! Every case drives the router, so each one exercises routing, deserialization,
//! the handler, the persisted record and serialization — the path a client takes.
//! The rows span the whole member set and differ along what the rule generalises
//! over: which member, the value's type and truthiness (a stated `false` is still
//! stated), a list's element count including the empty list and the model's
//! maximum, each published interval bound a member stated on its own can reach,
//! where the member was stated, and whether the request is a create or an update.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for.
const REGION: &str = "eu-west-1";

/// A destination Region for the cross-Region copy rows, matching
/// `TargetRegion`'s `^([a-z]+-){2,3}\d$`.
const OTHER_REGION: &str = "us-west-2";

/// An `ExecutionRoleArn` satisfying `^arn:aws(-[a-z]{1,3}){0,2}:iam::\d+:role/`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AWSDataLifecycleManagerDefaultRole";

/// `PolicyDetails$PolicyLanguage`: "`SIMPLIFIED` To create a default policy."
const DEFAULT_POLICY_LANGUAGE: &str = "SIMPLIFIED";

/// `PolicyDetails$PolicyLanguage`: "`STANDARD` To create a custom policy."
const CUSTOM_POLICY_LANGUAGE: &str = "STANDARD";

/// The `Code` an `InvalidRequestException` carries for a parameter this service
/// refuses, as the rest of this crate's error paths report it.
const INVALID_PARAMETER_CODE: &str = "InvalidParameter";

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

/// A custom EBS snapshot policy's `PolicyDetails`, as a client sends it: no
/// `[Default policies only]` member anywhere in it.
fn custom_snapshot_details() -> Value {
    json!({
        "PolicyType": "EBS_SNAPSHOT_MANAGEMENT",
        "ResourceTypes": ["VOLUME"],
        "TargetTags": [{"Key": "backup", "Value": "daily"}],
        "Schedules": [{
            "Name": "daily",
            "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS"},
            "RetainRule": {"Count": 7},
        }],
    })
}

/// One `[Default policies only]` configuration member, with a value for it.
struct Parameter {
    /// The case name: the member plus what is particular about this value.
    case: &'static str,
    /// The member's wire name.
    member: &'static str,
    /// The value a request states for it.
    value: Value,
    /// True when `CreateLifecyclePolicyRequest` and `UpdateLifecyclePolicyRequest`
    /// declare this member in the request BODY as well as in `PolicyDetails`.
    in_request_body: bool,
    /// True when a read of a default policy reports this very value at
    /// `/PolicyDetails/<member>`.
    reported_verbatim: bool,
}

/// Every member the model documents `[Default policies only]`, each with values
/// spanning what its target shape admits.
///
/// Source: `smithy_query(query="shape", shape="PolicyDetails")` — `ResourceType`,
/// `CreateInterval`, `RetainInterval`, `CopyTags`, `CrossRegionCopyTargets`,
/// `ExtendDeletion` and `Exclusions` each open with `[Default policies only]` —
/// read against `smithy_query(query="operation", operation="CreateLifecyclePolicy")`,
/// whose request body declares all of them but `ResourceType` (the body states
/// the type of default policy as `DefaultPolicy` instead). The bounds come from
/// the member docs: a creation frequency "can range from 1 to 7 days", a
/// retention period "from 2 to 14 days", `CrossRegionCopyTargetList` is
/// `@length(min: 0, max: 3)`.
fn default_policy_only_parameters() -> Vec<Parameter> {
    vec![
        Parameter {
            case: "ResourceType VOLUME",
            member: "ResourceType",
            value: json!("VOLUME"),
            in_request_body: false,
            reported_verbatim: true,
        },
        Parameter {
            // The other `ResourceTypeValues` member, which the issue did not cite.
            case: "ResourceType INSTANCE",
            member: "ResourceType",
            value: json!("INSTANCE"),
            in_request_body: false,
            reported_verbatim: true,
        },
        Parameter {
            case: "CreateInterval at the documented minimum of 1 day",
            member: "CreateInterval",
            value: json!(1),
            in_request_body: true,
            reported_verbatim: true,
        },
        Parameter {
            // Every row here states its member ALONE, so this policy's retention
            // period is the documented default of 7 days and the documented
            // ordering ("it must be greater than the creation frequency") admits
            // a creation frequency of at most 6. The member's own ceiling of 7
            // days, which only a request also stating a longer retention period
            // can reach, belongs to the cadence pair's rule — `interval_validation.rs`
            // and `effective_cadence_ordering.rs`.
            case: "CreateInterval at 6 days, the most the retention default leaves room for",
            member: "CreateInterval",
            value: json!(6),
            in_request_body: true,
            reported_verbatim: true,
        },
        Parameter {
            case: "RetainInterval at the documented minimum of 2 days",
            member: "RetainInterval",
            value: json!(2),
            in_request_body: true,
            reported_verbatim: true,
        },
        Parameter {
            case: "RetainInterval at the documented maximum of 14 days",
            member: "RetainInterval",
            value: json!(14),
            in_request_body: true,
            reported_verbatim: true,
        },
        Parameter {
            case: "CopyTags true",
            member: "CopyTags",
            value: json!(true),
            in_request_body: true,
            reported_verbatim: true,
        },
        Parameter {
            // A stated `false` is still stated: the rule is about whether the
            // request carries the member, not about the value it carries.
            case: "CopyTags false",
            member: "CopyTags",
            value: json!(false),
            in_request_body: true,
            reported_verbatim: true,
        },
        Parameter {
            case: "ExtendDeletion true",
            member: "ExtendDeletion",
            value: json!(true),
            in_request_body: true,
            reported_verbatim: true,
        },
        Parameter {
            case: "ExtendDeletion false",
            member: "ExtendDeletion",
            value: json!(false),
            in_request_body: true,
            reported_verbatim: true,
        },
        Parameter {
            case: "CrossRegionCopyTargets with one destination Region",
            member: "CrossRegionCopyTargets",
            value: json!([{"TargetRegion": OTHER_REGION}]),
            in_request_body: true,
            reported_verbatim: true,
        },
        Parameter {
            case: "CrossRegionCopyTargets with the maximum of three destination Regions",
            member: "CrossRegionCopyTargets",
            value: json!([
                {"TargetRegion": OTHER_REGION},
                {"TargetRegion": "eu-central-1"},
                {"TargetRegion": "ap-south-1"},
            ]),
            in_request_body: true,
            reported_verbatim: true,
        },
        Parameter {
            // `@length(min: 0)`, so the empty list is a value the member admits:
            // stating it states the member. A default policy that copies nowhere
            // reports no destination list at all, so only the refusal side of the
            // rule can assert on this row.
            case: "CrossRegionCopyTargets stated as the empty list",
            member: "CrossRegionCopyTargets",
            value: json!([]),
            in_request_body: true,
            reported_verbatim: false,
        },
        Parameter {
            case: "Exclusions excluding boot volumes",
            member: "Exclusions",
            value: json!({"ExcludeBootVolumes": true}),
            in_request_body: true,
            reported_verbatim: true,
        },
        Parameter {
            case: "Exclusions excluding volume types",
            member: "Exclusions",
            value: json!({"ExcludeVolumeTypes": ["gp2", "io1"]}),
            in_request_body: true,
            reported_verbatim: true,
        },
    ]
}

/// The members of `request` plus `member` at the top level.
fn with_body_parameter(mut request: Value, member: &str, value: &Value) -> Value {
    request[member] = value.clone();
    request
}

/// `details` with `member` stated inside it.
fn with_details_parameter(mut details: Value, member: &str, value: &Value) -> Value {
    details[member] = value.clone();
    details
}

/// A create request for a policy that is NOT a default policy, in each shape a
/// client can send one.
fn custom_create_requests(parameter: &Parameter) -> Vec<(String, Value)> {
    let base = json!({
        "ExecutionRoleArn": ROLE_ARN,
        "Description": "not a default policy",
        "State": "ENABLED",
    });
    let mut requests = Vec::new();
    if parameter.in_request_body {
        // The request the issue reproduced: body parameters and no `DefaultPolicy`.
        requests.push((
            format!("{}: in the request body with no PolicyDetails", parameter.case),
            with_body_parameter(base.clone(), parameter.member, &parameter.value),
        ));
        let mut beside_custom = base.clone();
        beside_custom["PolicyDetails"] = custom_snapshot_details();
        requests.push((
            format!(
                "{}: in the request body beside a custom PolicyDetails",
                parameter.case
            ),
            with_body_parameter(beside_custom, parameter.member, &parameter.value),
        ));
    }
    // Inside a `PolicyDetails` that is not a default policy's: once with the
    // custom policy language stated, once with none (the documented default is
    // the custom language).
    for (where_stated, details) in [
        ("inside a PolicyDetails stating no PolicyLanguage", {
            custom_snapshot_details()
        }),
        ("inside a STANDARD PolicyDetails", {
            let mut details = custom_snapshot_details();
            details["PolicyLanguage"] = json!(CUSTOM_POLICY_LANGUAGE);
            details
        }),
    ] {
        let mut request = base.clone();
        request["PolicyDetails"] =
            with_details_parameter(details, parameter.member, &parameter.value);
        requests.push((format!("{}: {where_stated}", parameter.case), request));
    }
    requests
}

/// Assert `answer` is the modelled refusal of `member`.
fn assert_refused(case: &str, member: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.error_code().ends_with("InvalidRequestException"),
        "case {case:?}: expected InvalidRequestException, got {:?}",
        answer.error_code()
    );
    assert_eq!(
        at(&answer.body, "/Code"),
        json!(INVALID_PARAMETER_CODE),
        "case {case:?}: expected the {INVALID_PARAMETER_CODE:?} code, got {:?}",
        at(&answer.body, "/Code")
    );
    assert!(
        answer.message().contains(member),
        "case {case:?}: the refusal must name {member:?}, got {:?}",
        answer.message()
    );
}

/// A `[Default policies only]` member stated by a create that is not creating a
/// default policy is refused, and no policy is minted for it.
#[tokio::test]
async fn a_create_refuses_default_only_parameters_for_a_custom_policy() {
    let router = emulator();
    for parameter in default_policy_only_parameters() {
        for (case, request) in custom_create_requests(&parameter) {
            let before = policy_count(&router).await;
            let answer = create(&router, request).await;
            assert_refused(&case, parameter.member, &answer);
            assert_eq!(
                policy_count(&router).await,
                before,
                "case {case:?}: a refused create must leave no policy behind"
            );
        }
    }
}

/// The other side of the rule: a DEFAULT policy accepts every one of those
/// members and reports it back, whether it was stated in the request body or in
/// the `PolicyDetails` structure.
#[tokio::test]
async fn a_default_policy_reports_every_default_only_parameter_it_states() {
    let router = emulator();
    for parameter in default_policy_only_parameters() {
        let mut requests: Vec<(String, Value)> = Vec::new();
        if parameter.in_request_body {
            let request = json!({
                "ExecutionRoleArn": ROLE_ARN,
                "Description": "a default policy from the body",
                "State": "ENABLED",
                "DefaultPolicy": "VOLUME",
            });
            requests.push((
                format!("{}: stated in the request body", parameter.case),
                with_body_parameter(request, parameter.member, &parameter.value),
            ));
        }
        // A default policy's configuration states which KIND of default policy
        // to create — `PolicyDetails$ResourceType` here — because that member
        // documents no value the service fills in for a request omitting it; a
        // row stating the member itself replaces this one.
        let details = with_details_parameter(
            json!({"PolicyLanguage": DEFAULT_POLICY_LANGUAGE, "ResourceType": "VOLUME"}),
            parameter.member,
            &parameter.value,
        );
        requests.push((
            format!("{}: stated in a SIMPLIFIED PolicyDetails", parameter.case),
            json!({
                "ExecutionRoleArn": ROLE_ARN,
                "Description": "a default policy from PolicyDetails",
                "State": "ENABLED",
                "PolicyDetails": details,
            }),
        ));

        for (case, request) in requests {
            let answer = create(&router, request).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected HTTP 200, got {} with {:?}",
                answer.status, answer.body
            );
            let read = get(&router, &answer.policy_id()).await;
            assert_eq!(
                at(read.policy(), "/DefaultPolicy"),
                json!(true),
                "case {case:?}: the policy must be reported as a default policy, got {:?}",
                at(read.policy(), "/DefaultPolicy")
            );
            if !parameter.reported_verbatim {
                continue;
            }
            let pointer = format!("/PolicyDetails/{}", parameter.member);
            assert_eq!(
                at(read.policy(), &pointer),
                parameter.value,
                "case {case:?}: expected {} to report {:?}, got {:?}",
                pointer,
                parameter.value,
                at(read.policy(), &pointer)
            );
        }
    }
}

/// `UpdateLifecyclePolicy` follows the same rule: it refuses a `[Default
/// policies only]` member for a policy that is not a default policy, leaving the
/// stored configuration untouched, and applies it to a default policy.
#[tokio::test]
async fn an_update_refuses_default_only_parameters_for_a_custom_policy() {
    let router = emulator();
    let custom = create(
        &router,
        json!({
            "ExecutionRoleArn": ROLE_ARN,
            "Description": "a custom policy to patch",
            "State": "ENABLED",
            "PolicyDetails": custom_snapshot_details(),
        }),
    )
    .await
    .policy_id();
    let stored = at(get(&router, &custom).await.policy(), "/PolicyDetails");

    for parameter in default_policy_only_parameters() {
        let mut patches: Vec<(String, Value)> = Vec::new();
        if parameter.in_request_body {
            patches.push((
                format!("{}: patched in the request body", parameter.case),
                with_body_parameter(json!({}), parameter.member, &parameter.value),
            ));
        }
        patches.push((
            format!("{}: patched inside PolicyDetails", parameter.case),
            json!({
                "PolicyDetails": with_details_parameter(
                    custom_snapshot_details(),
                    parameter.member,
                    &parameter.value,
                ),
            }),
        ));

        for (case, patch) in patches {
            let answer = update(&router, &custom, patch).await;
            assert_refused(&case, parameter.member, &answer);
            assert_eq!(
                at(get(&router, &custom).await.policy(), "/PolicyDetails"),
                stored,
                "case {case:?}: a refused update must leave the stored configuration alone"
            );
        }

        // The same patch against a DEFAULT policy is applied and reported. Only
        // the request-body form is exercised here: patching `PolicyDetails`
        // replaces the configuration wholesale, which is its own rule.
        if !parameter.in_request_body || !parameter.reported_verbatim {
            continue;
        }
        // A FRESH default policy per row. These patches state ONE member at a
        // time, and the creation frequency and the retention period are a pair:
        // patching them onto a policy still carrying the previous row's value
        // would answer about that pair instead of about this row's member (the
        // pair's own rule is `interval_validation.rs`).
        let default_policy = create(
            &router,
            json!({
                "ExecutionRoleArn": ROLE_ARN,
                "Description": "a default policy to patch",
                "State": "ENABLED",
                "DefaultPolicy": "VOLUME",
            }),
        )
        .await
        .policy_id();
        let case = format!("{}: patched on a default policy", parameter.case);
        let answer = update(
            &router,
            &default_policy,
            with_body_parameter(json!({}), parameter.member, &parameter.value),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        let pointer = format!("/PolicyDetails/{}", parameter.member);
        let read = get(&router, &default_policy).await;
        assert_eq!(
            at(read.policy(), &pointer),
            parameter.value,
            "case {case:?}: expected {} to report {:?}, got {:?}",
            pointer,
            parameter.value,
            at(read.policy(), &pointer)
        );
    }
}
