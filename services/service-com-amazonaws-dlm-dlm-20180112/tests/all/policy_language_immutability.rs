// SPDX-License-Identifier: CC0-1.0

//! Which KIND of policy a stored lifecycle policy is, across an update, end to
//! end over HTTP.
//!
//! The rule under test: the kind of policy a record is — a DEFAULT policy or a
//! CUSTOM one, which `PolicyDetails$PolicyLanguage` names ("`SIMPLIFIED` To
//! create a default policy", "`STANDARD` To create a custom policy") — is decided
//! when the policy is created and an update cannot change it
//! (`UpdateLifecyclePolicyRequest$PolicyDetails`: "You cannot update the policy
//! type or the resource type"). So an update whose configuration states the other
//! kind's language is refused with the modelled client error and writes nothing,
//! and an accepted update stores and reports a configuration materialized for the
//! kind the policy IS, whether or not the structure restated the language.
//!
//! Every case drives the router, so each one exercises routing, deserialization,
//! the handler, the persisted record and serialization — the only path on which
//! an update answered 200 can be seen to have taken effect, and the path on which
//! the two read projections of one policy can be seen to agree.
//!
//! The rows span what the rule generalises over: both members of the policy
//! language catalog as the STORED language and as the STATED one, each kind of
//! policy the service stores (a default policy of either
//! `DefaultPolicyTypeValues` member, stated in either of the two places a create
//! may state it, and a custom snapshot, AMI and event-based policy), and all
//! three ways an update's configuration can state the language: the policy's own,
//! the other kind's, and none at all.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for.
const REGION: &str = "us-west-2";

/// A destination Region for the event-based policy's copy action, matching
/// `TargetRegion`'s `^([a-z]+-){2,3}\d$`.
const OTHER_REGION: &str = "eu-west-1";

/// Every `PolicyLanguageValues` member, from
/// `smithy_query(query="shape", shape="PolicyLanguageValues")`: the kind of
/// policy each one names is `PolicyDetails$PolicyLanguage`'s documentation —
/// `SIMPLIFIED` "To create a default policy", `STANDARD` "To create a custom
/// policy".
const POLICY_LANGUAGE_VALUES: &[&str] = &["SIMPLIFIED", "STANDARD"];

/// The language a DEFAULT policy's configuration names.
const DEFAULT_POLICY_LANGUAGE: &str = "SIMPLIFIED";

/// The language a CUSTOM policy's configuration names.
const CUSTOM_POLICY_LANGUAGE: &str = "STANDARD";

/// The modelled client error this service answers for a refused parameter, its
/// `@httpError` status and its `Code`.
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";
const INVALID_REQUEST_STATUS: u16 = 400;
const INVALID_PARAMETER_CODE: &str = "InvalidParameter";

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

/// The `LifecyclePolicySummary` `GetLifecyclePolicies` reports for `id`.
async fn summary(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let answer = call(router, "GET", "/policies", None).await;
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
        .unwrap_or_else(|| panic!("case {case:?}: GetLifecyclePolicies must report {id}"))
}

fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// The catalog member that is not `language` — the other kind's language.
fn other_language(language: &str) -> &'static str {
    POLICY_LANGUAGE_VALUES
        .iter()
        .copied()
        .find(|candidate| *candidate != language)
        .expect("PolicyLanguageValues has more than one member")
}

/// One stored policy, and a replacement configuration an update may state for it.
struct Subject {
    /// What is particular about this row.
    case: &'static str,
    /// The create request that makes the policy.
    create: Value,
    /// The language the kind of policy it is names.
    language: &'static str,
    /// True when it is a DEFAULT policy, as both read projections report.
    default_policy: bool,
    /// A `PolicyDetails` an update may state for it, stating no `PolicyLanguage`:
    /// the test states each of the three languages on top of it. It is a complete
    /// configuration for the kind of policy it replaces, since an update's
    /// structure replaces the configuration wholesale.
    replacement: Value,
    /// A member of `replacement` the read must report after an accepted update,
    /// with the value it must report — what shows the write took effect.
    applied: (&'static str, Value),
}

/// A custom snapshot policy's configuration: `Schedules`' creation interval stays
/// inside the supported set (`interval_validation.rs` owns that rule).
fn custom_snapshot_details(resource_type: &str, interval: i64) -> Value {
    json!({
        "ResourceTypes": [resource_type],
        "TargetTags": [{"Key": "backup", "Value": "daily"}],
        "Schedules": [{
            "Name": "daily",
            "CreateRule": {"Interval": interval, "IntervalUnit": "HOURS"},
            "RetainRule": {"Count": 7},
        }],
    })
}

/// A custom AMI policy's configuration.
fn custom_image_details(interval: i64) -> Value {
    let mut details = custom_snapshot_details("INSTANCE", interval);
    details["PolicyType"] = json!("IMAGE_MANAGEMENT");
    details
}

/// An event-based policy's configuration, which configures no target and no
/// schedule.
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

/// A create of a default policy whose type is named by the `DefaultPolicy`
/// request-body parameter.
fn default_policy_from_body(resource_type: &str) -> Value {
    json!({
        "ExecutionRoleArn": role_arn(),
        "Description": format!("default {resource_type} policy"),
        "State": "ENABLED",
        "DefaultPolicy": resource_type,
    })
}

/// A create of a default policy whose type is named inside a `SIMPLIFIED`
/// configuration structure.
fn default_policy_from_details(resource_type: &str) -> Value {
    json!({
        "ExecutionRoleArn": role_arn(),
        "Description": format!("simplified {resource_type} policy"),
        "State": "ENABLED",
        "PolicyDetails": {
            "PolicyLanguage": DEFAULT_POLICY_LANGUAGE,
            "ResourceType": resource_type,
        },
    })
}

/// A create of a custom policy from the configuration `details`.
fn custom_policy(description: &str, details: Value) -> Value {
    json!({
        "ExecutionRoleArn": role_arn(),
        "Description": description,
        "State": "ENABLED",
        "PolicyDetails": details,
    })
}

/// The table: every kind of policy this service stores, each with a replacement
/// configuration of its own kind.
///
/// The `[Default policies only]` members a default policy's replacement states
/// stay inside their documented ranges (a creation frequency of 1 to 7 days, a
/// retention period of 2 to 14 days greater than the frequency).
fn subjects() -> Vec<Subject> {
    vec![
        Subject {
            case: "a default VOLUME policy created from the request body",
            create: default_policy_from_body("VOLUME"),
            language: DEFAULT_POLICY_LANGUAGE,
            default_policy: true,
            replacement: json!({"CreateInterval": 3, "RetainInterval": 14, "CopyTags": true}),
            applied: ("CreateInterval", json!(3)),
        },
        Subject {
            case: "a default INSTANCE policy created from a SIMPLIFIED structure",
            create: default_policy_from_details("INSTANCE"),
            language: DEFAULT_POLICY_LANGUAGE,
            default_policy: true,
            replacement: json!({"RetainInterval": 9, "ExtendDeletion": true}),
            applied: ("RetainInterval", json!(9)),
        },
        Subject {
            case: "a custom snapshot policy stating no language",
            create: custom_policy(
                "custom snapshot policy",
                custom_snapshot_details("VOLUME", 24),
            ),
            language: CUSTOM_POLICY_LANGUAGE,
            default_policy: false,
            replacement: custom_snapshot_details("VOLUME", 12),
            applied: ("Schedules", {
                let mut schedules = custom_snapshot_details("VOLUME", 12)["Schedules"].clone();
                // The members the service materializes onto every schedule of a
                // stored custom policy, as `policy_read_projection.rs` pins them.
                schedules[0]["CopyTags"] = json!(false);
                schedules[0]["CreateRule"]["Location"] = json!("CLOUD");
                schedules[0]["RetainRule"]["Interval"] = json!(0);
                schedules
            }),
        },
        Subject {
            case: "a custom AMI policy stating the custom language",
            create: custom_policy("custom ami policy", {
                let mut details = custom_image_details(24);
                details["PolicyLanguage"] = json!(CUSTOM_POLICY_LANGUAGE);
                details
            }),
            language: CUSTOM_POLICY_LANGUAGE,
            default_policy: false,
            replacement: custom_image_details(6),
            applied: ("ResourceTypes", json!(["INSTANCE"])),
        },
        Subject {
            case: "a custom event-based policy",
            create: custom_policy("event based copies", event_based_details("^.*$")),
            language: CUSTOM_POLICY_LANGUAGE,
            default_policy: false,
            replacement: event_based_details("^share-.*$"),
            applied: (
                "EventSource",
                event_based_details("^share-.*$")["EventSource"].clone(),
            ),
        },
    ]
}

/// `details` with `PolicyLanguage` stated as `language`, or stating none when it
/// is `None`.
fn with_language(details: &Value, language: Option<&str>) -> Value {
    let mut details = details.clone();
    if let Some(language) = language {
        details["PolicyLanguage"] = json!(language);
    }
    details
}

/// An update whose configuration states the language of the OTHER kind of policy
/// is refused, and writes nothing.
#[tokio::test]
async fn an_update_cannot_change_the_kind_of_policy_a_record_is() {
    let router = emulator();
    let mut refused_languages: Vec<&str> = Vec::new();

    for subject in subjects() {
        let case = subject.case;
        let id = create(&router, case, subject.create).await;
        let before = get(&router, case, &id).await;
        let before_summary = summary(&router, case, &id).await;

        let stated = other_language(subject.language);
        let answer = update(
            &router,
            &id,
            json!({"PolicyDetails": with_language(&subject.replacement, Some(stated))}),
        )
        .await;

        assert_eq!(
            answer.status, INVALID_REQUEST_STATUS,
            "case {case:?}: stating PolicyLanguage {stated:?} on a policy whose language is \
             {:?} must answer HTTP {INVALID_REQUEST_STATUS}, got {} with {:?}",
            subject.language, answer.status, answer.body
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
        // The refusal is built from the request and the stored policy: the member
        // refused, the language the policy holds and the one that was asked for.
        for named in ["PolicyDetails.PolicyLanguage", subject.language, stated] {
            assert!(
                answer.message().contains(named),
                "case {case:?}: the refusal must name {named:?}, got {:?}",
                answer.message()
            );
        }

        // Refused before the first write: the whole projection is what it was,
        // the kind of policy included.
        let after = get(&router, case, &id).await;
        assert_eq!(
            after.policy(),
            before.policy(),
            "case {case:?}: a refused update must leave the stored policy exactly as it was"
        );
        assert_eq!(
            summary(&router, case, &id).await,
            before_summary,
            "case {case:?}: a refused update must leave the policy's summary as it was"
        );

        refused_languages.push(stated);
    }

    // Both members of the catalog are refused by some row, so neither direction
    // of the conversion is accepted by a rule no case exercises.
    for language in POLICY_LANGUAGE_VALUES {
        assert!(
            refused_languages.contains(language),
            "the table must refuse PolicyLanguage {language:?} on some policy, refused \
             {refused_languages:?}"
        );
    }
}

/// An update whose configuration restates the policy's own language, or states
/// none, is applied — and the policy reads back as the kind it has always been,
/// with the language that kind names, in both read projections.
#[tokio::test]
async fn an_accepted_update_reports_the_kind_the_policy_is() {
    let router = emulator();
    let mut covered: Vec<&str> = Vec::new();

    for subject in subjects() {
        for stated in [Some(subject.language), None] {
            let case = format!(
                "{} updated with a configuration stating PolicyLanguage {stated:?}",
                subject.case
            );
            let case = case.as_str();
            let id = create(&router, case, subject.create.clone()).await;

            let answer = update(
                &router,
                &id,
                json!({"PolicyDetails": with_language(&subject.replacement, stated)}),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: the update must be accepted, got {} with {:?}",
                answer.status, answer.body
            );

            let read = get(&router, case, &id).await;
            assert_eq!(
                at(read.policy(), "/PolicyDetails/PolicyLanguage"),
                json!(subject.language),
                "case {case:?}: expected PolicyLanguage {:?}, got {:?}",
                subject.language,
                at(read.policy(), "/PolicyDetails/PolicyLanguage")
            );
            assert_eq!(
                at(read.policy(), "/DefaultPolicy"),
                json!(subject.default_policy),
                "case {case:?}: expected DefaultPolicy {}, got {:?}",
                subject.default_policy,
                at(read.policy(), "/DefaultPolicy")
            );

            // The write the language accompanied took effect.
            let (member, expected) = &subject.applied;
            assert_eq!(
                at(read.policy(), &format!("/PolicyDetails/{member}")),
                *expected,
                "case {case:?}: expected PolicyDetails.{member} to report {expected:?}, got {:?}",
                at(read.policy(), &format!("/PolicyDetails/{member}"))
            );

            // A DEFAULT policy's configuration carries none of the members only a
            // custom snapshot or AMI policy has: `ResourceLocations` is "[Custom
            // snapshot and AMI policies only]"
            // (`smithy_query(query="doc", shape="PolicyDetails",
            // member="ResourceLocations")`).
            if subject.default_policy {
                assert_eq!(
                    at(read.policy(), "/PolicyDetails/ResourceLocations"),
                    Value::Null,
                    "case {case:?}: a default policy has no ResourceLocations, got {:?}",
                    at(read.policy(), "/PolicyDetails/ResourceLocations")
                );
            }

            // The two read projections of one policy agree about it.
            let listed = summary(&router, case, &id).await;
            assert_eq!(
                at(&listed, "/DefaultPolicy"),
                json!(subject.default_policy),
                "case {case:?}: the summary must report DefaultPolicy {}, got {:?}",
                subject.default_policy,
                at(&listed, "/DefaultPolicy")
            );
            assert_eq!(
                at(&listed, "/PolicyType"),
                at(read.policy(), "/PolicyDetails/PolicyType"),
                "case {case:?}: the two projections must report one PolicyType, got {:?} and {:?}",
                at(&listed, "/PolicyType"),
                at(read.policy(), "/PolicyDetails/PolicyType")
            );

            covered.push(subject.language);
        }
    }

    // Both members of the catalog are the stored language of some row.
    for language in POLICY_LANGUAGE_VALUES {
        assert!(
            covered.contains(language),
            "the table must store a policy whose language is {language:?}, covered {covered:?}"
        );
    }
}
