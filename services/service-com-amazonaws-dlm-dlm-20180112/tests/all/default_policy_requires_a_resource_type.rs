// SPDX-License-Identifier: CC0-1.0

//! A DEFAULT policy's own resource type, required of the request that creates
//! one, end to end over HTTP.
//!
//! The rule under test: a conditionally required configuration member whose
//! documentation publishes no value the service fills in has to be stated by the
//! request that creates the kind of policy built from it. `PolicyDetails`
//! declares the `[Default policies only]` members, and all but one of them
//! document what a request omitting them asks for ("If you do not specify a
//! value, the default is ..." on `CreateInterval`, `RetainInterval`, `CopyTags`
//! and `ExtendDeletion`); the resource type documents only "Specify the type of
//! default policy to create", so a create that states it nowhere — not as the
//! request body's `DefaultPolicy`, not as `PolicyDetails$ResourceType`, and not
//! as the `PolicyType` one kind of default policy alone manages — is refused
//! with the modelled client error and stores nothing. Every policy that IS
//! created therefore holds one resource type, and the three places it is read —
//! the `PolicyType` both read projections report, the `resourceTypes` filter and
//! the `defaultPolicyType` filter — agree on it; an update cannot change it.
//!
//! Every case drives the router, so each one exercises routing,
//! deserialization, the handler, the persisted record, serialization and the
//! list filters — the only path on which a stored type one read reports and
//! another cannot place is visible at all.
//!
//! The rows span what the rule generalises over: every `[Default policies only]`
//! member that documents a fill-in value, stated alone beside a configuration
//! that names no kind of default policy (each refused); every
//! `DefaultPolicyTypeValues` member, stated in each of the three ways a create
//! may state the kind (each accepted, with every read and filter agreeing); and,
//! for each of those, an update restating the kind, omitting it, and naming the
//! other one.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for.
const REGION: &str = "eu-west-2";

/// `PolicyDetails$PolicyLanguage`: "`SIMPLIFIED` To create a default policy."
const DEFAULT_POLICY_LANGUAGE: &str = "SIMPLIFIED";

/// The wire name of the member that states which kind of default policy to
/// create inside the configuration structure.
const RESOURCE_TYPE_MEMBER: &str = "ResourceType";

/// The modelled client error this service answers for a refused parameter, its
/// `@httpError` status and its `Code`.
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";
const INVALID_REQUEST_STATUS: u16 = 400;
const INVALID_PARAMETER_CODE: &str = "InvalidParameter";

/// The `defaultPolicyType` value that selects a default policy of any kind
/// (`smithy_query(query="operation", operation="GetLifecyclePolicies")`:
/// `DefaultPoliciesTypeValues` carries `VOLUME`, `INSTANCE` and `ALL`).
const ANY_DEFAULT_POLICY_TYPE: &str = "ALL";

/// Every `DefaultPolicyTypeValues` member, read against the `PolicyTypeValues` a
/// default policy of that kind manages.
///
/// Source: `smithy_query(query="shape", shape="DefaultPolicyTypeValues")` —
/// `VOLUME` and `INSTANCE` — read against `PolicyDetails$ResourceType` ("To
/// create a default policy for EBS snapshots ... specify `VOLUME`", "for
/// EBS-backed AMIs ... specify `INSTANCE`") and `PolicyDetails$PolicyType`
/// ("`EBS_SNAPSHOT_MANAGEMENT` ... manages the lifecycle of Amazon EBS
/// snapshots", "`IMAGE_MANAGEMENT` ... manages the lifecycle of EBS-backed
/// AMIs").
const DEFAULT_POLICY_KINDS: [(&str, &str); 2] = [
    ("VOLUME", "EBS_SNAPSHOT_MANAGEMENT"),
    ("INSTANCE", "IMAGE_MANAGEMENT"),
];

/// Every `[Default policies only]` member whose documentation publishes the
/// value the service fills in for a request that omits it, with a value inside
/// the documented domain of each.
///
/// Source: `smithy_query(query="shape", shape="PolicyDetails")` —
/// `CreateInterval` "If you do not specify a value, the default is 1" (and "can
/// range from 1 to 7 days"), `RetainInterval` 7 ("from 2 to 14 days, but it must
/// be greater than the creation frequency"), `CopyTags` "the default is false",
/// `ExtendDeletion` "If you do not specify a value, the default is false". The
/// two members that publish no fill-in value but are not the resource type
/// either, `CrossRegionCopyTargets` ("If you do not want to create cross-Region
/// copies, omit this parameter", `@length(min: 0, max: 3)`) and `Exclusions`,
/// are listed last: a configuration that carries one of them still states no
/// kind of default policy.
fn filled_in_default_policy_members() -> Vec<(&'static str, Value)> {
    vec![
        ("CreateInterval", json!(3)),
        ("RetainInterval", json!(14)),
        ("CopyTags", json!(true)),
        ("ExtendDeletion", json!(true)),
        (
            "CrossRegionCopyTargets",
            json!([{"TargetRegion": "us-west-2"}]),
        ),
        ("Exclusions", json!({"ExcludeBootVolumes": true})),
    ]
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

    fn message(&self) -> String {
        self.body["Message"].as_str().unwrap_or_default().to_string()
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
    /// answer does not list the policy at all.
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

/// A create request carrying `details` as its configuration structure.
fn create_with_details(description: &str, details: Value) -> Value {
    json!({
        "ExecutionRoleArn": role_arn(),
        "Description": description,
        "State": "ENABLED",
        "PolicyDetails": details,
    })
}

/// The create requests that ask for a default policy and state its kind nowhere:
/// a `SIMPLIFIED` configuration alone, and one carrying each `[Default policies
/// only]` member that documents a fill-in value.
fn creates_stating_no_kind() -> Vec<(String, Value)> {
    let mut rows = vec![(
        "a SIMPLIFIED configuration and nothing else".to_string(),
        create_with_details(
            "default policy with no kind",
            json!({"PolicyLanguage": DEFAULT_POLICY_LANGUAGE}),
        ),
    )];
    for (member, value) in filled_in_default_policy_members() {
        let mut details = json!({"PolicyLanguage": DEFAULT_POLICY_LANGUAGE});
        details[member] = value;
        rows.push((
            format!("a SIMPLIFIED configuration stating {member} and no kind"),
            create_with_details(&format!("default policy stating {member}"), details),
        ));
    }
    rows
}

/// The three ways a create may state which kind of default policy it asks for.
#[derive(Debug, Clone, Copy)]
enum Stated {
    /// `DefaultPolicy`, in the request body.
    Body,
    /// `PolicyDetails$ResourceType`, inside the configuration structure.
    Details,
    /// `PolicyDetails$PolicyType` — the type one kind of default policy alone
    /// manages, which is the same fact stated from the other end.
    ManagedType,
}

impl Stated {
    fn label(self) -> &'static str {
        match self {
            Stated::Body => "DefaultPolicy in the request body",
            Stated::Details => "PolicyDetails.ResourceType",
            Stated::ManagedType => "PolicyDetails.PolicyType",
        }
    }
}

/// A create of a default policy of `kind`, stating that kind the way `stated`
/// names.
fn create_stating_kind(stated: Stated, kind: &str, managed: &str) -> Value {
    let mut request = json!({
        "ExecutionRoleArn": role_arn(),
        "Description": format!("default {kind} policy"),
        "State": "ENABLED",
    });
    let mut details = json!({"PolicyLanguage": DEFAULT_POLICY_LANGUAGE});
    match stated {
        Stated::Body => {
            request["DefaultPolicy"] = json!(kind);
            return request;
        }
        Stated::Details => details[RESOURCE_TYPE_MEMBER] = json!(kind),
        Stated::ManagedType => details["PolicyType"] = json!(managed),
    }
    request["PolicyDetails"] = details;
    request
}

/// The other `DefaultPolicyTypeValues` member — the kind a change would move to.
fn other_kind(kind: &str) -> &'static str {
    DEFAULT_POLICY_KINDS
        .iter()
        .map(|(candidate, _)| *candidate)
        .find(|candidate| *candidate != kind)
        .expect("DefaultPolicyTypeValues has more than one member")
}

/// The modelled refusal, its status, its `Code`, and a message naming `member`.
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
        "case {case:?}: the message must name {member:?}, got {:?}",
        answer.message()
    );
}

/// Everything the service reports about one default policy's kind: the resource
/// type the read states, the type both projections derive from it, and which
/// resource-type and default-policy-type filters select the policy.
///
/// Asserted together because the defect is exactly a disagreement between them:
/// a policy one read calls a snapshot default policy while no resource-type
/// filter and no `defaultPolicyType` filter can place it.
async fn assert_every_read_agrees(router: &MultiServiceRouter, case: &str, id: &str, kind: &str) {
    let managed = DEFAULT_POLICY_KINDS
        .iter()
        .find(|(candidate, _)| *candidate == kind)
        .map(|(_, policy_type)| *policy_type)
        .expect("the row names a DefaultPolicyTypeValues member");

    let read = get(router, id).await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: the policy must be readable, got {} with {:?}",
        read.status, read.body
    );
    for (pointer, expected) in [
        ("/DefaultPolicy", json!(true)),
        ("/PolicyDetails/PolicyLanguage", json!(DEFAULT_POLICY_LANGUAGE)),
        ("/PolicyDetails/ResourceType", json!(kind)),
        ("/PolicyDetails/PolicyType", json!(managed)),
    ] {
        let actual = at(read.policy(), pointer);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {pointer} {expected:?}, got {actual:?}"
        );
    }

    let summary_type = list(router, "").await.listed_policy_type(id);
    assert_eq!(
        summary_type,
        json!(managed),
        "case {case:?}: the summary must report PolicyType {managed:?}, got {summary_type:?}"
    );

    // Each filter value against the one kind the policy holds: it is selected by
    // its own kind, by the value that selects any default policy, and by nothing
    // else.
    for (candidate, _) in DEFAULT_POLICY_KINDS {
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
                candidate == kind,
                "case {case:?}: ?{filter}={candidate} must {} a policy of kind {kind:?}, got {:?}",
                if candidate == kind { "select" } else { "skip" },
                answer.listed_ids()
            );
        }
    }
    let any = list(router, &format!("?defaultPolicyType={ANY_DEFAULT_POLICY_TYPE}")).await;
    assert!(
        any.listed_ids().contains(&id.to_string()),
        "case {case:?}: ?defaultPolicyType={ANY_DEFAULT_POLICY_TYPE} must select every default \
         policy, got {:?}",
        any.listed_ids()
    );
}

/// A create that asks for a default policy and states its kind nowhere is
/// refused with the service's missing-required-parameter answer, and stores
/// nothing.
#[tokio::test]
async fn a_create_that_states_no_kind_of_default_policy_is_refused() {
    let router = emulator();
    for (case, request) in creates_stating_no_kind() {
        let before = policy_count(&router).await;
        let answer = create(&router, request).await;
        assert_refused(&case, RESOURCE_TYPE_MEMBER, &answer);
        assert_eq!(
            answer.message(),
            format!("The following required parameter(s) are missing: {{{RESOURCE_TYPE_MEMBER}}}"),
            "case {case:?}: expected the missing-required-parameter wording, got {:?}",
            answer.message()
        );
        assert_eq!(
            at(&answer.body, "/RequiredParameters"),
            json!([RESOURCE_TYPE_MEMBER]),
            "case {case:?}: RequiredParameters must name the omitted member, got {:?}",
            at(&answer.body, "/RequiredParameters")
        );
        assert_eq!(
            policy_count(&router).await,
            before,
            "case {case:?}: a refused create must leave no policy behind"
        );
    }
}

/// Whichever of the three ways a create states the kind of default policy it
/// asks for, the policy holds that one kind and every read and filter agrees on
/// it.
#[tokio::test]
async fn an_accepted_default_policy_holds_one_kind_every_read_agrees_on() {
    let router = emulator();
    for (kind, managed) in DEFAULT_POLICY_KINDS {
        for stated in [Stated::Body, Stated::Details, Stated::ManagedType] {
            let case = format!("a default {kind} policy stated as {}", stated.label());
            let created = create(&router, create_stating_kind(stated, kind, managed)).await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: the create must be accepted, got {} with {:?}",
                created.status, created.body
            );
            assert_every_read_agrees(&router, &case, &created.policy_id(), kind).await;
        }
    }
}

/// The kind a create stated is the policy's for good: an update restating it or
/// omitting it leaves it in place, and one naming the other kind is refused as a
/// parameter that cannot be updated ("You cannot update the policy type or the
/// resource type").
#[tokio::test]
async fn an_update_cannot_change_the_kind_a_create_stated() {
    let router = emulator();
    for (kind, managed) in DEFAULT_POLICY_KINDS {
        for stated in [Stated::Body, Stated::Details, Stated::ManagedType] {
            let case = format!("a default {kind} policy stated as {}", stated.label());
            let id = create(&router, create_stating_kind(stated, kind, managed))
                .await
                .policy_id();

            // A replacement configuration that OMITS the member asks for no
            // change of kind, so it is accepted and the kind stands.
            let omitted = update(
                &router,
                &id,
                json!({
                    "PolicyDetails": {
                        "PolicyLanguage": DEFAULT_POLICY_LANGUAGE,
                        "CreateInterval": 2,
                    },
                }),
            )
            .await;
            assert_eq!(
                omitted.status, 200,
                "case {case:?}: an update omitting the resource type must be accepted, got {} \
                 with {:?}",
                omitted.status, omitted.body
            );
            assert_every_read_agrees(&router, &format!("{case}, kind omitted by an update"), &id, kind)
                .await;

            // One RESTATING it states the kind the policy holds.
            let restated = update(
                &router,
                &id,
                json!({
                    "PolicyDetails": {
                        "PolicyLanguage": DEFAULT_POLICY_LANGUAGE,
                        RESOURCE_TYPE_MEMBER: kind,
                    },
                }),
            )
            .await;
            assert_eq!(
                restated.status, 200,
                "case {case:?}: an update restating the resource type must be accepted, got {} \
                 with {:?}",
                restated.status, restated.body
            );
            assert_every_read_agrees(
                &router,
                &format!("{case}, kind restated by an update"),
                &id,
                kind,
            )
            .await;

            // And one naming the OTHER kind is refused, leaving the policy as it
            // was.
            let wanted = other_kind(kind);
            let refused = update(
                &router,
                &id,
                json!({
                    "PolicyDetails": {
                        "PolicyLanguage": DEFAULT_POLICY_LANGUAGE,
                        RESOURCE_TYPE_MEMBER: wanted,
                    },
                }),
            )
            .await;
            let refused_case = format!("{case}, asked to become {wanted}");
            assert_refused(&refused_case, RESOURCE_TYPE_MEMBER, &refused);
            assert_eq!(
                refused.message(),
                format!("The following parameter(s) cannot be updated: {RESOURCE_TYPE_MEMBER}"),
                "case {refused_case:?}: expected the non-updatable-parameter wording, got {:?}",
                refused.message()
            );
            assert_every_read_agrees(&router, &refused_case, &id, kind).await;
        }
    }
}

/// The tables cover the catalog: every `DefaultPolicyTypeValues` member appears
/// as a kind a create states, and every `[Default policies only]` member that
/// documents a fill-in value appears in a create that states no kind.
#[tokio::test]
async fn the_tables_cover_every_modelled_value() {
    for (kind, managed) in DEFAULT_POLICY_KINDS {
        for stated in [Stated::Body, Stated::Details, Stated::ManagedType] {
            let request = create_stating_kind(stated, kind, managed);
            assert!(
                request.to_string().contains(kind) || request.to_string().contains(managed),
                "case {:?}: the request must state the kind {kind:?} or the type it manages \
                 {managed:?}, got {request:?}",
                stated.label()
            );
        }
    }
    let rows = creates_stating_no_kind();
    for (member, _) in filled_in_default_policy_members() {
        assert!(
            rows.iter().any(|(case, _)| case.contains(member)),
            "the refused table must carry a row stating {member:?}, got {:?}",
            rows.iter().map(|(case, _)| case).collect::<Vec<_>>()
        );
    }
}
