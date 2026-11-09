// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the AWS IoT POLICY ATTACHMENT relation: `AttachPolicy`,
//! `DetachPolicy` and the two listings that read the relation from either end,
//! `ListAttachedPolicies` (one target's policies) and `ListTargetsForPolicy` (one
//! policy's targets).
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `iot` signing name — so what is asserted is what an
//! AWS SDK client would see, not an internal function's return value.
//!
//! The rule under test: attaching a policy RECORDS the pair (policy, target) in one
//! persisted relation and answers 200 with an empty body; both listings report exactly
//! the recorded pairs; detaching removes the pair; the attach is idempotent; and a pair
//! is only ever recorded for resources that exist, so a policy name the registry does
//! not hold and a well-formed target naming a resource this account does not hold are
//! refused `ResourceNotFoundException` (404), while a target of no supported form is
//! refused `InvalidRequestException` (400) — never a 200 that records an attachment to
//! nothing.
//!
//! The tables span the class the rule generalises over rather than one recorded
//! request: each of the three target forms the modelled member documents (a certificate
//! ARN, a thing group ARN and a Cognito identity), a certificate that is ACTIVE and one
//! that is INACTIVE, policy names of several forms, the ancestors a `recursive` listing
//! adds, every page size on both listings and the ceiling the relation refuses past.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region the harness signs for.
const REGION: &str = "us-east-1";
/// An IoT policy document every policy in this suite is created with: the document is
/// not what this suite is about, so one valid document serves every case.
const POLICY_DOCUMENT: &str =
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iot:*","Resource":"*"}]}"#;

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name
/// and the region: the scope is how the router picks the claiming service, and how the
/// service picks the region its ARNs are scoped to. The signature itself is never
/// checked here — the harness turns request authentication off, exactly as an
/// unauthenticated emulator runs.
fn signed_for_iot() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{REGION}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// A string member of the body, or `None` when it is absent or not a string.
    fn text(&self, name: &str) -> Option<String> {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The `message` of an error body.
    fn message(&self) -> String {
        self.text("message").unwrap_or_default()
    }

    /// The `policyName` of every entry of a `ListAttachedPolicies` answer, in the order
    /// the listing reported them.
    fn policy_names(&self) -> Vec<String> {
        self.body
            .get("policies")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| {
                        entry
                            .get("policyName")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The `policyArn` of every entry of a `ListAttachedPolicies` answer.
    fn policy_arns(&self) -> Vec<String> {
        self.body
            .get("policies")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| {
                        entry
                            .get("policyArn")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The `targets` of a `ListTargetsForPolicy` answer.
    fn targets(&self) -> Vec<String> {
        self.body
            .get("targets")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| entry.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether the body carries no member at all — what an operation whose output shape
    /// is `Unit` answers with.
    fn is_empty_body(&self) -> bool {
        match &self.body {
            Value::Null => true,
            Value::Object(members) => members.is_empty(),
            _ => false,
        }
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it,
/// reached the way the production router reaches it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{REGION}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot())
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    /// `CreatePolicy`: `POST /policies/{policyName}`.
    async fn create_policy(&self, policy_name: &str) -> Reply {
        self.call(
            "POST",
            &format!("/policies/{policy_name}"),
            Some(&json!({ "policyDocument": POLICY_DOCUMENT })),
        )
        .await
    }

    /// `CreateKeysAndCertificate`: `POST /keys-and-certificate`, which issues the
    /// certificate a policy is attached to.
    async fn create_certificate(&self, set_as_active: bool) -> Reply {
        self.call(
            "POST",
            &format!("/keys-and-certificate?setAsActive={set_as_active}"),
            None,
        )
        .await
    }

    /// `CreateThingGroup`: `POST /thing-groups/{thingGroupName}`.
    async fn create_thing_group(&self, name: &str, request: &Value) -> Reply {
        self.call("POST", &format!("/thing-groups/{name}"), Some(request))
            .await
    }

    /// `AttachPolicy`: `PUT /target-policies/{policyName}` carrying the target.
    async fn attach_policy(&self, policy_name: &str, target: &str) -> Reply {
        self.call(
            "PUT",
            &format!("/target-policies/{policy_name}"),
            Some(&json!({ "target": target })),
        )
        .await
    }

    /// `DetachPolicy`: `POST /target-policies/{policyName}` carrying the target.
    async fn detach_policy(&self, policy_name: &str, target: &str) -> Reply {
        self.call(
            "POST",
            &format!("/target-policies/{policy_name}"),
            Some(&json!({ "target": target })),
        )
        .await
    }

    /// `ListAttachedPolicies`: `POST /attached-policies/{target}`, whose target is ONE
    /// path segment ([`as_path_segment`]).
    async fn list_attached_policies(
        &self,
        target: &str,
        recursive: bool,
        page_size: Option<i32>,
        marker: Option<&str>,
    ) -> Reply {
        let mut query: Vec<String> = Vec::new();
        if recursive {
            query.push("recursive=true".to_string());
        }
        if let Some(page_size) = page_size {
            query.push(format!("pageSize={page_size}"));
        }
        if let Some(marker) = marker {
            query.push(format!("marker={marker}"));
        }
        let suffix = if query.is_empty() {
            String::new()
        } else {
            format!("?{}", query.join("&"))
        };
        self.call(
            "POST",
            &format!("/attached-policies/{}{suffix}", as_path_segment(target)),
            None,
        )
        .await
    }

    /// `ListTargetsForPolicy`: `POST /policy-targets/{policyName}`.
    async fn list_targets_for_policy(
        &self,
        policy_name: &str,
        page_size: Option<i32>,
        marker: Option<&str>,
    ) -> Reply {
        let mut query: Vec<String> = Vec::new();
        if let Some(page_size) = page_size {
            query.push(format!("pageSize={page_size}"));
        }
        if let Some(marker) = marker {
            query.push(format!("marker={marker}"));
        }
        let suffix = if query.is_empty() {
            String::new()
        } else {
            format!("?{}", query.join("&"))
        };
        self.call("POST", &format!("/policy-targets/{policy_name}{suffix}"), None)
            .await
    }

    /// Every policy name `target` holds, walked page by page at `page_size`: the pages a
    /// client would follow through `nextMarker`, flattened in the order they arrived.
    async fn walk_attached_policies(
        &self,
        target: &str,
        recursive: bool,
        page_size: Option<i32>,
    ) -> Vec<String> {
        let mut walked: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        // One more page than any case in this suite can produce, so a listing that kept
        // issuing markers fails the assertion below instead of spinning.
        for _ in 0..32 {
            let page = self
                .list_attached_policies(target, recursive, page_size, marker.as_deref())
                .await;
            assert_eq!(
                page.status, 200,
                "case {target:?}/pageSize={page_size:?}: a page must answer 200, got \
                 {} {:?}",
                page.status, page.body
            );
            walked.extend(page.policy_names());
            match page.text("nextMarker") {
                Some(next) => marker = Some(next),
                None => return walked,
            }
        }
        panic!("case {target:?}: the listing never stopped issuing a nextMarker");
    }

    /// Every target `policy_name` is attached to, walked page by page at `page_size`.
    async fn walk_policy_targets(&self, policy_name: &str, page_size: Option<i32>) -> Vec<String> {
        let mut walked: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        for _ in 0..32 {
            let page = self
                .list_targets_for_policy(policy_name, page_size, marker.as_deref())
                .await;
            assert_eq!(
                page.status, 200,
                "case {policy_name:?}/pageSize={page_size:?}: a page must answer 200, \
                 got {} {:?}",
                page.status, page.body
            );
            walked.extend(page.targets());
            match page.text("nextMarker") {
                Some(next) => marker = Some(next),
                None => return walked,
            }
        }
        panic!("case {policy_name:?}: the listing never stopped issuing a nextMarker");
    }
}

/// A target as ONE URI path segment: the `:` and `/` of an ARN travel as `%3A` and
/// `%2F`, or the request addresses a different route.
fn as_path_segment(target: &str) -> String {
    target.replace('%', "%25").replace(':', "%3A").replace('/', "%2F")
}

/// The ARN a policy created in this suite's region carries.
fn policy_arn(policy_name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:policy/{policy_name}")
}

/// The ARN of a thing group created in this suite's region.
fn thing_group_arn(name: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:thinggroup/{name}")
}

/// The ARN of a certificate in this suite's region, for a certificate id this account
/// does not hold.
fn certificate_arn(certificate_id: &str) -> String {
    format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:cert/{certificate_id}")
}

/// Create `policy_name`, asserting the create succeeded: every attachment case starts
/// from a policy that exists.
async fn given_policy(emulator: &Emulator, policy_name: &str) {
    let created = emulator.create_policy(policy_name).await;
    assert_eq!(
        created.status, 200,
        "case {policy_name:?}: CreatePolicy must answer 200, got {} {:?}",
        created.status, created.body
    );
}

/// Issue a certificate, answering its `certificateArn`.
async fn given_certificate(emulator: &Emulator, set_as_active: bool) -> String {
    let issued = emulator.create_certificate(set_as_active).await;
    assert_eq!(
        issued.status, 200,
        "case setAsActive={set_as_active}: CreateKeysAndCertificate must answer 200, \
         got {} {:?}",
        issued.status, issued.body
    );
    issued
        .text("certificateArn")
        .unwrap_or_else(|| panic!("case setAsActive={set_as_active}: no certificateArn"))
}

/// Create a thing group, answering its `thingGroupArn`.
async fn given_thing_group(emulator: &Emulator, name: &str, request: &Value) -> String {
    let created = emulator.create_thing_group(name, request).await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: CreateThingGroup must answer 200, got {} {:?}",
        created.status, created.body
    );
    created
        .text("thingGroupArn")
        .unwrap_or_else(|| panic!("case {name:?}: no thingGroupArn"))
}

/// One row of the round-trip table: a target form, and how it is reached.
struct TargetCase {
    /// What the case is called in an assertion.
    label: &'static str,
    /// The policy attached in this case.
    policy_name: &'static str,
    /// The target the policy is attached to.
    target: String,
    /// The target as both listings must report it: the ARN this account holds for the
    /// resource, or the Cognito identity itself.
    reported: String,
}

/// An attach RECORDS the pair, both listings report it, a repeat attach changes
/// nothing, and a detach removes it — for every target form the modelled member
/// documents: a certificate ARN (ACTIVE and INACTIVE), a thing group ARN and a Cognito
/// identity.
#[tokio::test]
async fn attaching_a_policy_records_the_pair_both_listings_report() {
    let emulator = Emulator::new();

    let active_certificate = given_certificate(&emulator, true).await;
    let inactive_certificate = given_certificate(&emulator, false).await;
    let group = given_thing_group(&emulator, "plant-floor", &json!({})).await;

    // The target forms `ListAttachedPolicies$target` documents: "Valid principals are
    // CertificateArn (arn:aws:iot:region:accountId:cert/certificateId), thingGroupArn
    // (arn:aws:iot:region:accountId:thinggroup/groupName) and CognitoId (region:id)".
    let cases = vec![
        TargetCase {
            label: "an ACTIVE certificate ARN",
            policy_name: "device-policy",
            target: active_certificate.clone(),
            reported: active_certificate.clone(),
        },
        TargetCase {
            label: "an INACTIVE certificate ARN",
            policy_name: "Second_Policy-1",
            target: inactive_certificate.clone(),
            reported: inactive_certificate.clone(),
        },
        TargetCase {
            label: "a thing group ARN",
            policy_name: "fleet.policy",
            target: group.clone(),
            reported: thing_group_arn("plant-floor"),
        },
        TargetCase {
            label: "a Cognito identity",
            policy_name: "app+policy=1",
            target: format!("{REGION}:0bb1c2d3-4e5f-6789-abcd-ef0123456789"),
            reported: format!("{REGION}:0bb1c2d3-4e5f-6789-abcd-ef0123456789"),
        },
    ];

    for case in &cases {
        let label = case.label;
        given_policy(&emulator, case.policy_name).await;

        // Nothing is attached before the attach: the listing answers an EMPTY list, not
        // a refusal and not an absent member.
        let before = emulator
            .list_attached_policies(&case.target, false, None, None)
            .await;
        assert_eq!(
            before.status, 200,
            "case {label}: ListAttachedPolicies on an unattached target must answer \
             200, got {} {:?}",
            before.status, before.body
        );
        assert!(
            before.policy_names().is_empty(),
            "case {label}: an unattached target must hold no policy, got {:?}",
            before.policy_names()
        );

        let attached = emulator.attach_policy(case.policy_name, &case.target).await;
        assert_eq!(
            attached.status, 200,
            "case {label}: AttachPolicy must answer 200, got {} {:?}",
            attached.status, attached.body
        );
        assert!(
            attached.is_empty_body(),
            "case {label}: AttachPolicy must answer an empty body, got {:?}",
            attached.body
        );

        // The target's end of the relation: the policy, with the ARN the registry holds
        // for it.
        let listed = emulator
            .list_attached_policies(&case.target, false, None, None)
            .await;
        assert_eq!(
            listed.policy_names(),
            vec![case.policy_name.to_string()],
            "case {label}: ListAttachedPolicies must name the attached policy, got {:?}",
            listed.body
        );
        assert_eq!(
            listed.policy_arns(),
            vec![policy_arn(case.policy_name)],
            "case {label}: ListAttachedPolicies must carry the policy's ARN, got {:?}",
            listed.body
        );

        // The policy's end of the SAME relation.
        let targets = emulator
            .list_targets_for_policy(case.policy_name, None, None)
            .await;
        assert_eq!(
            targets.targets(),
            vec![case.reported.clone()],
            "case {label}: ListTargetsForPolicy must name the target, got {:?}",
            targets.body
        );

        // The attach is idempotent: the pair is recorded once, so a repeat changes
        // nothing either end reports.
        let again = emulator.attach_policy(case.policy_name, &case.target).await;
        assert_eq!(
            again.status, 200,
            "case {label}: a repeated AttachPolicy must answer 200, got {} {:?}",
            again.status, again.body
        );
        let after_repeat = emulator
            .list_attached_policies(&case.target, false, None, None)
            .await;
        assert_eq!(
            after_repeat.policy_names(),
            vec![case.policy_name.to_string()],
            "case {label}: a repeated attach must record the pair once, got {:?}",
            after_repeat.body
        );

        // A detach removes the pair, and both ends stop reporting it.
        let detached = emulator.detach_policy(case.policy_name, &case.target).await;
        assert_eq!(
            detached.status, 200,
            "case {label}: DetachPolicy must answer 200, got {} {:?}",
            detached.status, detached.body
        );
        assert!(
            detached.is_empty_body(),
            "case {label}: DetachPolicy must answer an empty body, got {:?}",
            detached.body
        );
        let after_detach = emulator
            .list_attached_policies(&case.target, false, None, None)
            .await;
        assert!(
            after_detach.policy_names().is_empty(),
            "case {label}: a detached target must hold no policy, got {:?}",
            after_detach.body
        );
        let targets_after = emulator
            .list_targets_for_policy(case.policy_name, None, None)
            .await;
        assert!(
            targets_after.targets().is_empty(),
            "case {label}: a detached policy must name no target, got {:?}",
            targets_after.body
        );

        // A detach of a pair the relation does not hold is still a 200: `DetachPolicy`
        // models no `ResourceNotFoundException`, and the state the caller asked for
        // already holds.
        let repeat_detach = emulator.detach_policy(case.policy_name, &case.target).await;
        assert_eq!(
            repeat_detach.status, 200,
            "case {label}: a repeated DetachPolicy must answer 200, got {} {:?}",
            repeat_detach.status, repeat_detach.body
        );
    }
}

/// One target may hold several policies and one policy several targets, and each pair is
/// independent: detaching one leaves the others recorded.
#[tokio::test]
async fn the_relation_is_many_to_many_and_each_pair_is_independent() {
    let emulator = Emulator::new();

    let certificate = given_certificate(&emulator, true).await;
    let group = given_thing_group(&emulator, "line-7", &json!({})).await;
    let cognito = format!("{REGION}:11112222-3333-4444-5555-666677778888");
    let targets = [certificate.clone(), group.clone(), cognito.clone()];
    let policies = ["connect", "publish", "subscribe"];

    for policy_name in policies {
        given_policy(&emulator, policy_name).await;
        for target in &targets {
            let attached = emulator.attach_policy(policy_name, target).await;
            assert_eq!(
                attached.status, 200,
                "case {policy_name:?}->{target:?}: AttachPolicy must answer 200, got {} {:?}",
                attached.status, attached.body
            );
        }
    }

    for target in &targets {
        let listed: BTreeSet<String> = emulator
            .walk_attached_policies(target, false, None)
            .await
            .into_iter()
            .collect();
        let expected: BTreeSet<String> = policies.iter().map(|name| name.to_string()).collect();
        assert_eq!(
            listed, expected,
            "case {target:?}: every attached policy must be reported, got {listed:?}"
        );
    }

    for policy_name in policies {
        let listed: BTreeSet<String> = emulator
            .walk_policy_targets(policy_name, None)
            .await
            .into_iter()
            .collect();
        let expected: BTreeSet<String> = targets.iter().cloned().collect();
        assert_eq!(
            listed, expected,
            "case {policy_name:?}: every target must be reported, got {listed:?}"
        );
    }

    // One pair removed leaves every other pair of both its policy and its target.
    let detached = emulator.detach_policy("publish", &group).await;
    assert_eq!(
        detached.status, 200,
        "case publish->line-7: DetachPolicy must answer 200, got {} {:?}",
        detached.status, detached.body
    );
    let group_policies: BTreeSet<String> = emulator
        .walk_attached_policies(&group, false, None)
        .await
        .into_iter()
        .collect();
    assert_eq!(
        group_policies,
        BTreeSet::from(["connect".to_string(), "subscribe".to_string()]),
        "case publish->line-7: only the detached pair may go, got {group_policies:?}"
    );
    let publish_targets: BTreeSet<String> = emulator
        .walk_policy_targets("publish", None)
        .await
        .into_iter()
        .collect();
    assert_eq!(
        publish_targets,
        BTreeSet::from([certificate.clone(), cognito.clone()]),
        "case publish->line-7: only the detached pair may go, got {publish_targets:?}"
    );
}

/// A `recursive` listing of a thing group reports the policies attached to the group AND
/// to every group above it; a non-recursive one reports only the group's own.
#[tokio::test]
async fn a_recursive_listing_adds_the_policies_of_the_groups_above() {
    let emulator = Emulator::new();

    // A three-level hierarchy: root -> middle -> leaf, one policy attached at each
    // level, so the recursive answer differs at every level of the walk.
    let root = given_thing_group(&emulator, "root", &json!({})).await;
    let middle =
        given_thing_group(&emulator, "middle", &json!({ "parentGroupName": "root" })).await;
    let leaf = given_thing_group(&emulator, "leaf", &json!({ "parentGroupName": "middle" })).await;

    for (policy_name, target) in [
        ("root-policy", &root),
        ("middle-policy", &middle),
        ("leaf-policy", &leaf),
    ] {
        given_policy(&emulator, policy_name).await;
        let attached = emulator.attach_policy(policy_name, target).await;
        assert_eq!(
            attached.status, 200,
            "case {policy_name:?}: AttachPolicy must answer 200, got {} {:?}",
            attached.status, attached.body
        );
    }

    let cases: Vec<(&str, &String, Vec<&str>, Vec<&str>)> = vec![
        ("the root group", &root, vec!["root-policy"], vec!["root-policy"]),
        (
            "the middle group",
            &middle,
            vec!["middle-policy"],
            vec!["middle-policy", "root-policy"],
        ),
        (
            "the leaf group",
            &leaf,
            vec!["leaf-policy"],
            vec!["leaf-policy", "middle-policy", "root-policy"],
        ),
    ];

    for (label, target, direct, recursive) in cases {
        let listed: BTreeSet<String> = emulator
            .walk_attached_policies(target, false, None)
            .await
            .into_iter()
            .collect();
        let expected: BTreeSet<String> = direct.iter().map(|name| name.to_string()).collect();
        assert_eq!(
            listed, expected,
            "case {label}: a non-recursive listing must report only its own policies, \
             got {listed:?}"
        );

        let walked: BTreeSet<String> = emulator
            .walk_attached_policies(target, true, None)
            .await
            .into_iter()
            .collect();
        let expected_recursive: BTreeSet<String> =
            recursive.iter().map(|name| name.to_string()).collect();
        assert_eq!(
            walked, expected_recursive,
            "case {label}: a recursive listing must add the groups above, got {walked:?}"
        );
    }

    // A marker belongs to the listing that issued it: the recursive and the
    // non-recursive listing of one target walk different rows, so neither accepts the
    // other's marker.
    let recursive_page = emulator
        .list_attached_policies(&leaf, true, Some(1), None)
        .await;
    let recursive_marker = recursive_page
        .text("nextMarker")
        .expect("case the marker scope: a truncated recursive page must carry a marker");
    let crossed = emulator
        .list_attached_policies(&leaf, false, Some(1), Some(&recursive_marker))
        .await;
    assert_eq!(
        crossed.status, 400,
        "case the marker scope: a recursive listing's marker must not resume the \
         non-recursive one, got {} {:?}",
        crossed.status, crossed.body
    );
    assert_eq!(
        crossed.error_code.as_deref(),
        Some("InvalidRequestException"),
        "case the marker scope: expected InvalidRequestException, got {:?}",
        crossed.error_code
    );

    // A policy attached BOTH to a group and to an ancestor is one policy, reported once.
    let shared = emulator.attach_policy("root-policy", &leaf).await;
    assert_eq!(
        shared.status, 200,
        "case shared policy: AttachPolicy must answer 200, got {} {:?}",
        shared.status, shared.body
    );
    let walked = emulator.walk_attached_policies(&leaf, true, None).await;
    let mut deduped = walked.clone();
    deduped.sort();
    deduped.dedup();
    assert_eq!(
        walked.len(),
        deduped.len(),
        "case shared policy: a recursive listing must report a policy once, got {walked:?}"
    );
    assert!(
        walked.contains(&"root-policy".to_string()),
        "case shared policy: the shared policy must be reported, got {walked:?}"
    );
}

/// Every page size pages the SAME set: each listing walks its rows through `nextMarker`
/// without repeating or dropping one, and the last page carries no marker.
#[tokio::test]
async fn every_page_size_pages_the_same_set() {
    let emulator = Emulator::new();

    // Cognito identities need no resource of their own, so a policy can be attached to
    // as many targets as the page walk needs cheaply.
    let identities: Vec<String> = (0..7)
        .map(|index| format!("{REGION}:aaaa0000-0000-0000-0000-00000000000{index}"))
        .collect();
    given_policy(&emulator, "paged-policy").await;
    for identity in &identities {
        let attached = emulator.attach_policy("paged-policy", identity).await;
        assert_eq!(
            attached.status, 200,
            "case {identity:?}: AttachPolicy must answer 200, got {} {:?}",
            attached.status, attached.body
        );
    }

    let all_targets: BTreeSet<String> = identities.iter().cloned().collect();
    // Every page size from one row per page to more rows than exist, plus the modelled
    // maximum (`pageSize` is `range 1..250`) and no page size at all.
    for page_size in [None, Some(1), Some(2), Some(3), Some(7), Some(8), Some(250)] {
        let walked = emulator.walk_policy_targets("paged-policy", page_size).await;
        let mut sorted = walked.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            walked.len(),
            "case pageSize={page_size:?}: no target may be reported twice, got {walked:?}"
        );
        assert_eq!(
            walked.into_iter().collect::<BTreeSet<String>>(),
            all_targets,
            "case pageSize={page_size:?}: the walk must report every target"
        );
    }

    // The other listing, paged the same way: several policies on one target.
    let policies: Vec<String> = (0..5).map(|index| format!("paged-{index}")).collect();
    let target = identities[0].clone();
    for policy_name in &policies {
        given_policy(&emulator, policy_name).await;
        let attached = emulator.attach_policy(policy_name, &target).await;
        assert_eq!(
            attached.status, 200,
            "case {policy_name:?}: AttachPolicy must answer 200, got {} {:?}",
            attached.status, attached.body
        );
    }
    let mut expected: BTreeSet<String> = policies.iter().cloned().collect();
    expected.insert("paged-policy".to_string());
    for page_size in [None, Some(1), Some(2), Some(5), Some(6), Some(250)] {
        let walked = emulator.walk_attached_policies(&target, false, page_size).await;
        let mut sorted = walked.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            walked.len(),
            "case pageSize={page_size:?}: no policy may be reported twice, got {walked:?}"
        );
        assert_eq!(
            walked.into_iter().collect::<BTreeSet<String>>(),
            expected,
            "case pageSize={page_size:?}: the walk must report every attached policy"
        );
    }

    // A page that is not truncated carries NO marker, whatever page size asked for it.
    let whole = emulator
        .list_targets_for_policy("paged-policy", Some(250), None)
        .await;
    assert_eq!(
        whole.text("nextMarker"),
        None,
        "case the whole listing: an untruncated page must carry no nextMarker, got {:?}",
        whole.body
    );

    // A marker neither listing issued is the modelled 400, not a page of something else.
    // Each row satisfies the modelled `Marker` pattern `^[A-Za-z0-9+/]+={0,2}$` — a
    // marker outside it is refused by the framework's own constraint check before any
    // handler runs — and is not a token either listing ever issued.
    for marker in ["QUJD", "0000", "7a7a7a", "ABCD=="] {
        let attached_page = emulator
            .list_attached_policies(&target, false, None, Some(marker))
            .await;
        assert_eq!(
            attached_page.status, 400,
            "case marker={marker:?}: ListAttachedPolicies must refuse it 400, got {} {:?}",
            attached_page.status, attached_page.body
        );
        assert_eq!(
            attached_page.error_code.as_deref(),
            Some("InvalidRequestException"),
            "case marker={marker:?}: expected InvalidRequestException, got {:?}",
            attached_page.error_code
        );
        let targets_page = emulator
            .list_targets_for_policy("paged-policy", None, Some(marker))
            .await;
        assert_eq!(
            targets_page.status, 400,
            "case marker={marker:?}: ListTargetsForPolicy must refuse it 400, got {} {:?}",
            targets_page.status, targets_page.body
        );
        assert_eq!(
            targets_page.error_code.as_deref(),
            Some("InvalidRequestException"),
            "case marker={marker:?}: expected InvalidRequestException, got {:?}",
            targets_page.error_code
        );
    }
}

/// A pair is only ever recorded for resources that EXIST: an unknown policy and a
/// well-formed target naming a resource this account does not hold are refused 404, and
/// a target of no supported form 400 — and nothing is recorded either way.
#[tokio::test]
async fn an_attachment_to_a_resource_that_does_not_exist_is_refused() {
    let emulator = Emulator::new();

    let certificate = given_certificate(&emulator, true).await;
    given_policy(&emulator, "known-policy").await;
    given_thing_group(&emulator, "known-group", &json!({})).await;

    // A certificate id this account holds no certificate under: 64 hex characters, the
    // modelled `CertificateId` form (`length 64..64, pattern ^(0x)?[a-fA-F0-9]+$`).
    let unknown_certificate = certificate_arn(&"ab".repeat(32));

    // The NOT-FOUND rows: a well-formed target, or a policy name the registry does not
    // hold, with the message AWS IoT words each refusal with.
    let not_found: Vec<(&str, &str, String, String)> = vec![
        (
            "an unknown policy name",
            "no-such-policy",
            certificate.clone(),
            "Policy no-such-policy does not exist".to_string(),
        ),
        (
            "an unknown certificate target",
            "known-policy",
            unknown_certificate.clone(),
            "The certificate given in the principal does not exist.".to_string(),
        ),
        (
            "an unknown thing group target",
            "known-policy",
            thing_group_arn("no-such-group"),
            // The thing-group family states a miss as `ThingGroup <accountId>:<name>
            // not found.`, which is the sentence every reader of a group in this crate
            // refuses with (`state/not_found.rs`).
            format!("ThingGroup {ACCOUNT_ID}:no-such-group not found."),
        ),
    ];

    for (label, policy_name, target, message) in &not_found {
        let refused = emulator.attach_policy(policy_name, target).await;
        assert_eq!(
            refused.status, 404,
            "case {label}: AttachPolicy must refuse it 404, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {label}: expected ResourceNotFoundException, got {:?}",
            refused.error_code
        );
        assert_eq!(
            &refused.message(),
            message,
            "case {label}: expected the message {message:?}, got {:?}",
            refused.message()
        );
    }

    // The INVALID rows: strings that name no policy target at all. Each is refused 400,
    // the class the model has for a request that is not valid.
    let invalid = vec![
        ("an empty target", String::new()),
        ("a bare name", "plant-floor".to_string()),
        ("a path with no ARN prefix", "cert/abc".to_string()),
        (
            "an ARN of another service",
            format!("arn:aws:iam::{ACCOUNT_ID}:role/device"),
        ),
        (
            "an IoT ARN of a resource no policy attaches to",
            format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:thing/sensor-1"),
        ),
        (
            "a certificate ARN with no id",
            format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:cert/"),
        ),
        (
            "an IoT ARN with no resource type",
            format!("arn:aws:iot:{REGION}:{ACCOUNT_ID}:cert"),
        ),
        ("a Cognito identity with no id", format!("{REGION}:")),
        ("a Cognito identity with no region", ":abcdef".to_string()),
    ];

    for (label, target) in &invalid {
        let refused = emulator.attach_policy("known-policy", target).await;
        assert_eq!(
            refused.status, 400,
            "case {label}: AttachPolicy must refuse it 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("InvalidRequestException"),
            "case {label}: expected InvalidRequestException, got {:?}",
            refused.error_code
        );
        assert!(
            refused.message().contains("policy target"),
            "case {label}: the refusal must name what a policy target is, got {:?}",
            refused.message()
        );
    }

    // Nothing was recorded by any refusal: the policy that exists holds no target, and
    // the certificate that exists holds no policy.
    let targets = emulator
        .list_targets_for_policy("known-policy", None, None)
        .await;
    assert!(
        targets.targets().is_empty(),
        "case after the refusals: no target may have been recorded, got {:?}",
        targets.body
    );
    let policies = emulator
        .list_attached_policies(&certificate, false, None, None)
        .await;
    assert!(
        policies.policy_names().is_empty(),
        "case after the refusals: no policy may have been recorded, got {:?}",
        policies.body
    );

    // The LISTINGS resolve their resource the same way the attach does.
    let unknown_target_listing = emulator
        .list_attached_policies(&unknown_certificate, false, None, None)
        .await;
    assert_eq!(
        unknown_target_listing.status, 404,
        "case listing an unknown certificate: expected 404, got {} {:?}",
        unknown_target_listing.status, unknown_target_listing.body
    );
    assert_eq!(
        unknown_target_listing.message(),
        "The certificate given in the principal does not exist.",
        "case listing an unknown certificate: unexpected message {:?}",
        unknown_target_listing.message()
    );
    let invalid_target_listing = emulator
        .list_attached_policies("plant-floor", false, None, None)
        .await;
    assert_eq!(
        invalid_target_listing.status, 400,
        "case listing a target of no form: expected 400, got {} {:?}",
        invalid_target_listing.status, invalid_target_listing.body
    );
    let unknown_policy_listing = emulator
        .list_targets_for_policy("no-such-policy", None, None)
        .await;
    assert_eq!(
        unknown_policy_listing.status, 404,
        "case listing an unknown policy's targets: expected 404, got {} {:?}",
        unknown_policy_listing.status, unknown_policy_listing.body
    );
    assert_eq!(
        unknown_policy_listing.message(),
        "Policy no-such-policy does not exist",
        "case listing an unknown policy's targets: unexpected message {:?}",
        unknown_policy_listing.message()
    );

    // A detach names no resource that has to exist — the operation models no 404 — so an
    // unknown policy and an unknown target are both a 200 that records nothing, while a
    // target of no form is still the modelled 400.
    for (label, policy_name, target) in [
        ("an unknown policy", "no-such-policy", certificate.clone()),
        (
            "an unknown certificate",
            "known-policy",
            unknown_certificate.clone(),
        ),
    ] {
        let detached = emulator.detach_policy(policy_name, &target).await;
        assert_eq!(
            detached.status, 200,
            "case detaching {label}: expected 200, got {} {:?}",
            detached.status, detached.body
        );
    }
    let malformed_detach = emulator.detach_policy("known-policy", "plant-floor").await;
    assert_eq!(
        malformed_detach.status, 400,
        "case detaching a target of no form: expected 400, got {} {:?}",
        malformed_detach.status, malformed_detach.body
    );
}

/// The relation a caller grows is BOUNDED: past the ceiling of policies one target may
/// hold, the attach answers the modelled `LimitExceededException` (410) and records
/// nothing.
#[tokio::test]
async fn a_target_holds_at_most_the_ceiling_of_policies() {
    let emulator = Emulator::new();

    // The ceiling is a constant of the state module; the test reads it the way a client
    // does — by filling the target until the attach is refused — so it asserts the
    // ceiling is ENFORCED rather than restating its value.
    let target = format!("{REGION}:99998888-7777-6666-5555-444433332222");
    let mut attached_names: Vec<String> = Vec::new();
    let mut refusal: Option<Reply> = None;
    for index in 0..24 {
        let policy_name = format!("ceiling-{index:02}");
        given_policy(&emulator, &policy_name).await;
        let attached = emulator.attach_policy(&policy_name, &target).await;
        if attached.status == 200 {
            attached_names.push(policy_name);
            continue;
        }
        refusal = Some(attached);
        break;
    }

    let refused = refusal.expect("case the ceiling: the relation must refuse past a ceiling");
    assert_eq!(
        refused.status, 410,
        "case the ceiling: expected 410, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code.as_deref(),
        Some("LimitExceededException"),
        "case the ceiling: expected LimitExceededException, got {:?}",
        refused.error_code
    );
    assert!(
        !attached_names.is_empty(),
        "case the ceiling: the first attach must have succeeded"
    );

    // The refusal recorded nothing: the target still holds exactly what was attached
    // before it.
    let listed: BTreeSet<String> = emulator
        .walk_attached_policies(&target, false, None)
        .await
        .into_iter()
        .collect();
    assert_eq!(
        listed,
        attached_names.iter().cloned().collect::<BTreeSet<String>>(),
        "case the ceiling: the refusal must have recorded nothing, got {listed:?}"
    );

    // A policy already attached is still attachable past the ceiling: the repeat records
    // nothing, so it is no nearer the limit.
    let repeat = emulator
        .attach_policy(&attached_names[0], &target)
        .await;
    assert_eq!(
        repeat.status, 200,
        "case the ceiling: a repeat of a recorded pair must answer 200, got {} {:?}",
        repeat.status, repeat.body
    );
}
