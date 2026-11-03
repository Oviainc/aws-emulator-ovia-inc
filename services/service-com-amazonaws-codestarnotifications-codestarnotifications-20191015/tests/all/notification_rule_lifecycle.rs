// SPDX-License-Identifier: CC0-1.0

//! The notification-rule lifecycle over the wire: create, describe, list (with filters
//! and pagination), subscribe, update and delete, driven through the same
//! `MultiServiceRouter` dispatch the emulator serves production requests with.
//!
//! Every suite here asserts a RULE over a TABLE of inputs — the rows differ in the
//! dimension the rule generalises over (detail type, status, event type count, target
//! count and type, tag count, region, page size) — rather than pinning one recorded
//! request.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_codestarnotifications_codestarnotifications_20191015 as codestar;
use smithy_server_runtime::MultiServiceRouter;

/// The emulated service behind the router that mounts it, exactly as the production
/// front door assembles it: one shared engine, one registered service.
///
/// `pub(crate)` so the sibling suites in this test target drive the emulator through
/// this ONE harness rather than each building their own.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signature verification is the emulator's, not this service's: switch it off so
        // the suite exercises the handlers rather than SigV4.
        router.set_request_authentication(false);
        router.register_service(codestar::EmulatedService::new(engine));
        Self { router }
    }

    /// POST one restJson1 request, signed for `region`, and return its status and body.
    pub(crate) async fn post_in(&self, region: &str, path: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTFIXTURE/20260101/{region}/codestar-notifications/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=0123456789abcdef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://codestar-notifications.{region}.amazonaws.com{path}"
            ))
            .header("authorization", authorization)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let mut parsed: Value = if response.body().is_empty() {
            Value::Object(serde_json::Map::new())
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        // Carry the wire error code (restJson1 puts it in `x-amzn-errortype`) into the
        // body a case can assert on.
        if let (Some(code), Some(object)) = (error_code, parsed.as_object_mut()) {
            object.insert("__errorType".to_string(), Value::String(code));
        }
        (status, parsed)
    }

    pub(crate) async fn post(&self, path: &str, body: &Value) -> (u16, Value) {
        self.post_in("us-east-1", path, body).await
    }

    /// POST a request that must succeed, and return its body.
    pub(crate) async fn ok(&self, case: &str, path: &str, body: &Value) -> Value {
        let (status, parsed) = self.post(path, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from {path}, got {status} with {parsed}"
        );
        parsed
    }
}

/// The wire error code of a failed call.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__errorType")
        .and_then(Value::as_str)
        .or_else(|| body.get("__type").and_then(Value::as_str))
        .unwrap_or("<none>")
        .to_string()
}

pub(crate) fn string(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `Message` member of an error body. restJson1 renders it under the modelled member
/// name; a client that lowercases it is served by the second lookup.
pub(crate) fn message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn number(body: &Value, member: &str) -> f64 {
    body.get(member).and_then(Value::as_f64).unwrap_or(f64::NAN)
}

/// One row of the create/describe table.
struct RuleCase {
    label: &'static str,
    name: &'static str,
    resource: &'static str,
    detail_type: &'static str,
    status: Option<&'static str>,
    event_type_ids: &'static [&'static str],
    targets: &'static [(&'static str, &'static str)],
    tags: &'static [(&'static str, &'static str)],
}

impl RuleCase {
    fn request(&self) -> Value {
        let mut body = json!({
            "Name": self.name,
            "Resource": self.resource,
            "DetailType": self.detail_type,
            "EventTypeIds": self.event_type_ids,
            "Targets": self.targets.iter().map(target_json).collect::<Vec<_>>(),
        });
        if let Some(status) = self.status {
            body["Status"] = json!(status);
        }
        if !self.tags.is_empty() {
            body["Tags"] = Value::Object(
                self.tags
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), json!(v)))
                    .collect(),
            );
        }
        body
    }

    /// The status the rule must report: the one asked for, else `ENABLED` — the default
    /// `CreateNotificationRule$Status` documents.
    fn expected_status(&self) -> &str {
        self.status.unwrap_or("ENABLED")
    }
}

fn target_json((target_type, address): &(&str, &str)) -> Value {
    json!({ "TargetType": target_type, "TargetAddress": address })
}

/// The table every create/describe assertion runs over. The rows differ in detail type,
/// status (absent / both values), event-type count and owning service, target count
/// (including the modelled minimum of zero) and type, and tag count — the dimensions the
/// create-then-read rule generalises over.
const RULE_CASES: &[RuleCase] = &[
    RuleCase {
        label: "codecommit, FULL, default status, one SNS target, one tag",
        name: "demo-rule",
        resource: "arn:aws:codecommit:us-east-1:123456789012:MyRepo",
        detail_type: "FULL",
        status: None,
        event_type_ids: &["codecommit-repository-comments-on-commits"],
        targets: &[("SNS", "arn:aws:sns:us-east-1:123456789012:demo-topic")],
        tags: &[("Team", "dev")],
    },
    RuleCase {
        label: "codebuild, BASIC, DISABLED, three event types, two targets, no tags",
        name: "build_alerts",
        resource: "arn:aws:codebuild:us-east-1:123456789012:project/widget",
        detail_type: "BASIC",
        status: Some("DISABLED"),
        event_type_ids: &[
            "codebuild-project-build-state-failed",
            "codebuild-project-build-state-succeeded",
            "codebuild-project-build-phase-failure",
        ],
        targets: &[
            ("SNS", "arn:aws:sns:us-east-1:123456789012:builds"),
            ("AWSChatbotSlack", "arn:aws:chatbot::123456789012:chat-configuration/slack-channel/eng"),
        ],
        tags: &[],
    },
    RuleCase {
        label: "codepipeline, FULL, ENABLED, zero targets (the modelled minimum), two tags",
        name: "Pipeline Watch",
        resource: "arn:aws:codepipeline:us-east-1:123456789012:release",
        detail_type: "FULL",
        status: Some("ENABLED"),
        event_type_ids: &["codepipeline-pipeline-pipeline-execution-failed"],
        targets: &[],
        tags: &[("Team", "platform"), ("env", "prod")],
    },
    RuleCase {
        label: "codedeploy, BASIC, default status, one SNS target, no tags",
        name: "deploy-events",
        resource: "arn:aws:codedeploy:us-east-1:123456789012:application:app",
        detail_type: "BASIC",
        status: None,
        event_type_ids: &["codedeploy-application-deployment-succeeded"],
        targets: &[("SNS", "arn:aws:sns:us-east-1:123456789012:deploys")],
        tags: &[],
    },
];

/// A rule that was created is readable, member for member, from the store — and every
/// event type id it carries resolves to the catalog's own service and resource type.
#[tokio::test]
async fn a_created_rule_describes_exactly_what_was_created() {
    let emulator = Emulator::new();

    for case in RULE_CASES {
        let created = emulator
            .ok(case.label, "/createNotificationRule", &case.request())
            .await;
        let arn = string(&created, "Arn");
        assert_arn(case.label, &arn, "us-east-1");

        let described = emulator
            .ok(
                case.label,
                "/describeNotificationRule",
                &json!({ "Arn": arn }),
            )
            .await;

        assert_eq!(
            string(&described, "Arn"),
            arn,
            "case {:?}: describe must report the ARN it was asked about, got {described}",
            case.label
        );
        assert_eq!(
            string(&described, "Name"),
            case.name,
            "case {:?}: expected Name {:?}, got {described}",
            case.label,
            case.name
        );
        assert_eq!(
            string(&described, "Resource"),
            case.resource,
            "case {:?}: expected Resource {:?}, got {described}",
            case.label,
            case.resource
        );
        assert_eq!(
            string(&described, "DetailType"),
            case.detail_type,
            "case {:?}: expected DetailType {:?}, got {described}",
            case.label,
            case.detail_type
        );
        assert_eq!(
            string(&described, "Status"),
            case.expected_status(),
            "case {:?}: expected Status {:?}, got {described}",
            case.label,
            case.expected_status()
        );
        assert!(
            !string(&described, "CreatedBy").is_empty(),
            "case {:?}: CreatedBy is len 1.., got {described}",
            case.label
        );
        let created_at = number(&described, "CreatedTimestamp");
        let modified_at = number(&described, "LastModifiedTimestamp");
        assert!(
            created_at > 0.0 && modified_at >= created_at,
            "case {:?}: expected 0 < CreatedTimestamp <= LastModifiedTimestamp, got {created_at} and {modified_at}",
            case.label
        );

        // Tags: reported exactly as stored (an untagged rule reports an empty map; see
        // the Tags suite in `describe_notification_rule_fidelity.rs`).
        let described_tags = tags_of(&described);
        let expected_tags: BTreeMap<String, String> = case
            .tags
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        assert_eq!(
            described_tags, expected_tags,
            "case {:?}: expected Tags {expected_tags:?}, got {described}",
            case.label
        );

        // Targets: one summary per stored address, carrying the type it was created
        // with and the target's own status.
        let described_targets = described
            .get("Targets")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            described_targets.len(),
            case.targets.len(),
            "case {:?}: expected {} targets, got {described}",
            case.label,
            case.targets.len()
        );
        for (target_type, address) in case.targets {
            let found = described_targets
                .iter()
                .find(|summary| string(summary, "TargetAddress") == *address)
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: target {address} is missing from {described}",
                        case.label
                    )
                });
            assert_eq!(
                string(found, "TargetType"),
                *target_type,
                "case {:?}: expected TargetType {target_type:?} for {address}, got {found}",
                case.label
            );
            assert_eq!(
                string(found, "TargetStatus"),
                "ACTIVE",
                "case {:?}: a subscribed target is ACTIVE, got {found}",
                case.label
            );
        }

        // EventTypes: one summary per stored id, in the order the rule stores them.
        let event_types = described
            .get("EventTypes")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            event_types.len(),
            case.event_type_ids.len(),
            "case {:?}: expected {} event types, got {described}",
            case.label,
            case.event_type_ids.len()
        );
        for (index, id) in case.event_type_ids.iter().enumerate() {
            let summary = &event_types[index];
            assert_eq!(
                string(summary, "EventTypeId"),
                *id,
                "case {:?}: expected EventTypeId {id:?} at {index}, got {summary}",
                case.label
            );
            let catalogued = codestar::state::EVENT_TYPE_CATALOG
                .iter()
                .any(|entry| entry.event_type_id == *id);
            assert!(
                catalogued,
                "case {:?}: a stored event type id is one the service publishes, got {id:?}",
                case.label
            );
            assert!(
                !string(summary, "ServiceName").is_empty()
                    && !string(summary, "ResourceType").is_empty()
                    && !string(summary, "EventTypeName").is_empty(),
                "case {:?}: a catalogued event type resolves every member, got {summary}",
                case.label
            );
        }
    }
}

/// Every entry of the seeded catalog resolves, including ones no case above cites.
///
/// The table at the top pins rows to LITERAL expected values — the catalog data itself,
/// not just the plumbing — and the loop then checks that no entry is left unresolved.
#[tokio::test]
async fn every_catalogued_event_type_resolves_to_its_service_and_resource_type() {
    let emulator = Emulator::new();

    // Literal rows, taken from the "Notification concepts" list in the Developer Tools
    // Console User Guide that `EventTypeSummary$EventTypeId`'s documentation links to.
    // One per service in the catalog, none of them cited by the create/describe table.
    let pinned: &[(&str, &str, &str, &str)] = &[
        (
            "codecommit-repository-branches-and-tags-created",
            "CodeCommit",
            "Repository",
            "Branches and tags: Created",
        ),
        (
            "codebuild-project-build-phase-success",
            "CodeBuild",
            "Project",
            "Build phase: Success",
        ),
        (
            "codedeploy-application-deployment-started",
            "CodeDeploy",
            "Application",
            "Deployment: Started",
        ),
        (
            "codepipeline-pipeline-manual-approval-needed",
            "CodePipeline",
            "Pipeline",
            "Manual approval: Needed",
        ),
    ];

    for (index, (id, service_name, resource_type, event_type_name)) in
        pinned.iter().enumerate()
    {
        let created = emulator
            .ok(
                id,
                "/createNotificationRule",
                &json!({
                    "Name": format!("pinned-{index}"),
                    "Resource": "arn:aws:codecommit:us-east-1:123456789012:MyRepo",
                    "DetailType": "BASIC",
                    "EventTypeIds": [id],
                    "Targets": [],
                }),
            )
            .await;
        let described = emulator
            .ok(
                id,
                "/describeNotificationRule",
                &json!({ "Arn": string(&created, "Arn") }),
            )
            .await;
        let summary = described
            .get("EventTypes")
            .and_then(Value::as_array)
            .and_then(|list| list.first())
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            (
                string(&summary, "ServiceName"),
                string(&summary, "ResourceType"),
                string(&summary, "EventTypeName"),
            ),
            (
                (*service_name).to_string(),
                (*resource_type).to_string(),
                (*event_type_name).to_string(),
            ),
            "case {id:?}: unexpected resolved event type {summary}"
        );
    }

    for (index, entry) in codestar::state::EVENT_TYPE_CATALOG.iter().enumerate() {
        let name = format!("catalog-rule-{index}");
        let created = emulator
            .ok(
                entry.event_type_id,
                "/createNotificationRule",
                &json!({
                    "Name": name,
                    "Resource": "arn:aws:codecommit:us-east-1:123456789012:MyRepo",
                    "DetailType": "BASIC",
                    "EventTypeIds": [entry.event_type_id],
                    "Targets": [],
                }),
            )
            .await;
        let described = emulator
            .ok(
                entry.event_type_id,
                "/describeNotificationRule",
                &json!({ "Arn": string(&created, "Arn") }),
            )
            .await;
        let summary = described
            .get("EventTypes")
            .and_then(Value::as_array)
            .and_then(|list| list.first())
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            string(&summary, "ServiceName"),
            entry.service_name,
            "case {:?}: expected ServiceName {:?}, got {summary}",
            entry.event_type_id,
            entry.service_name
        );
        assert_eq!(
            string(&summary, "ResourceType"),
            entry.resource_type,
            "case {:?}: expected ResourceType {:?}, got {summary}",
            entry.event_type_id,
            entry.resource_type
        );
        assert_eq!(
            string(&summary, "EventTypeName"),
            entry.event_type_name,
            "case {:?}: expected EventTypeName {:?}, got {summary}",
            entry.event_type_id,
            entry.event_type_name
        );
    }
}

/// The ARN a rule is minted with carries the region the request was signed for, the
/// canonical emulator account, and an id inside `NotificationRuleId`'s 1..40 length.
#[tokio::test]
async fn the_rule_arn_is_built_from_the_requests_own_region() {
    let emulator = Emulator::new();

    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let body = json!({
            "Name": format!("region-{region}"),
            "Resource": "arn:aws:codecommit:us-east-1:123456789012:MyRepo",
            "DetailType": "BASIC",
            "EventTypeIds": ["codecommit-repository-pull-request-created"],
            "Targets": [{ "TargetType": "SNS", "TargetAddress": "arn:aws:sns:us-east-1:123456789012:t" }],
        });
        let (status, created) = emulator.post_in(region, "/createNotificationRule", &body).await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected 200, got {status} with {created}"
        );
        assert_arn(region, &string(&created, "Arn"), region);
    }
}

fn assert_arn(case: &str, arn: &str, region: &str) {
    let segments: Vec<&str> = arn.splitn(6, ':').collect();
    assert_eq!(
        segments.len(),
        6,
        "case {case:?}: a notification rule ARN has six segments, got {arn:?}"
    );
    assert_eq!(
        (segments[0], segments[1], segments[2]),
        ("arn", "aws", "codestar-notifications"),
        "case {case:?}: unexpected ARN prefix in {arn:?}"
    );
    assert_eq!(
        segments[3], region,
        "case {case:?}: expected the request's region in {arn:?}"
    );
    assert_eq!(
        segments[4].len(),
        12,
        "case {case:?}: the account segment is 12 digits, got {arn:?}"
    );
    assert!(
        segments[4].chars().all(char::is_numeric),
        "case {case:?}: the account segment is 12 digits, got {arn:?}"
    );
    let id = segments[5]
        .strip_prefix("notificationrule/")
        .unwrap_or_else(|| panic!("case {case:?}: expected a notificationrule/ resource in {arn:?}"));
    assert!(
        (1..=40).contains(&id.len()),
        "case {case:?}: NotificationRuleId is len 1..40, got {id:?}"
    );
}

pub(crate) fn tags_of(body: &Value) -> BTreeMap<String, String> {
    body.get("Tags")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        v.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A rule name is unique per account: re-using one is a 409, whatever else the second
/// request changes.
#[tokio::test]
async fn a_second_rule_with_a_taken_name_is_a_conflict() {
    let emulator = Emulator::new();

    for case in RULE_CASES {
        emulator
            .ok(case.label, "/createNotificationRule", &case.request())
            .await;

        // Same name, everything else different.
        let mut second = case.request();
        second["Resource"] = json!("arn:aws:codecommit:us-east-1:123456789012:OtherRepo");
        second["DetailType"] = json!("FULL");
        second["Targets"] = json!([]);
        let (status, body) = emulator.post("/createNotificationRule", &second).await;
        assert_eq!(
            status, 409,
            "case {:?}: a taken rule name is a 409, got {status} with {body}",
            case.label
        );
        assert_eq!(
            error_code(&body),
            "ResourceAlreadyExistsException",
            "case {:?}: got {body}",
            case.label
        );
    }
}

/// `Tags` key names cannot start with "aws" (`CreateNotificationRule$Tags`).
#[tokio::test]
async fn a_reserved_tag_key_is_refused_and_an_allowed_one_is_kept() {
    let emulator = Emulator::new();

    for (index, (key, allowed)) in [
        ("Team", true),
        ("env", true),
        ("awsome", false),
        ("aws:created-by", false),
        ("AWS", false),
    ]
    .iter()
    .enumerate()
    {
        let body = json!({
            "Name": format!("tag-case-{index}"),
            "Resource": "arn:aws:codecommit:us-east-1:123456789012:MyRepo",
            "DetailType": "BASIC",
            "EventTypeIds": ["codecommit-repository-pull-request-created"],
            "Targets": [],
            "Tags": { *key: "value" },
        });
        let (status, parsed) = emulator.post("/createNotificationRule", &body).await;
        if *allowed {
            assert_eq!(
                status, 200,
                "case {key:?}: expected the tag to be accepted, got {status} with {parsed}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {key:?}: a key starting with \"aws\" is refused, got {status} with {parsed}"
            );
            assert_eq!(
                error_code(&parsed),
                "ValidationException",
                "case {key:?}: got {parsed}"
            );
        }
    }
}

/// A `ClientRequestToken` replayed with the same parameters returns the rule it first
/// created; replayed with a changed parameter it is refused.
#[tokio::test]
async fn an_idempotency_token_replays_the_first_rule() {
    let emulator = Emulator::new();

    for (index, token) in ["token-1", "client/token:2", "A-very_long-token-3"]
        .iter()
        .enumerate()
    {
        let mut body = RULE_CASES[0].request();
        body["Name"] = json!(format!("idempotent-{index}"));
        body["ClientRequestToken"] = json!(token);

        let first = emulator.ok(token, "/createNotificationRule", &body).await;
        let replay = emulator.ok(token, "/createNotificationRule", &body).await;
        assert_eq!(
            string(&first, "Arn"),
            string(&replay, "Arn"),
            "case {token:?}: a replayed token must report the original rule, got {replay}"
        );

        // The name index must not have been consumed twice either: exactly one rule.
        let listed = emulator.ok(token, "/listNotificationRules", &json!({})).await;
        let matching = listed
            .get("NotificationRules")
            .and_then(Value::as_array)
            .map(|rules| {
                rules
                    .iter()
                    .filter(|summary| string(summary, "Arn") == string(&first, "Arn"))
                    .count()
            })
            .unwrap_or_default();
        assert_eq!(
            matching, 1,
            "case {token:?}: the replay must not create a second rule, got {listed}"
        );

        let mut changed = body.clone();
        changed["Name"] = json!(format!("idempotent-changed-{index}"));
        let (status, parsed) = emulator.post("/createNotificationRule", &changed).await;
        assert_eq!(
            status, 400,
            "case {token:?}: a token reused with a changed parameter is refused, got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {token:?}: got {parsed}"
        );
    }
}

/// Describing an ARN the store does not hold is a 404.
#[tokio::test]
async fn describing_an_unknown_rule_is_a_404() {
    let emulator = Emulator::new();

    for arn in [
        "arn:aws:codestar-notifications:us-east-1:000000000000:notificationrule/11111111-1111-4111-8111-111111111111",
        "arn:aws:codestar-notifications:eu-west-1:000000000000:notificationrule/does-not-exist",
    ] {
        let (status, parsed) = emulator
            .post("/describeNotificationRule", &json!({ "Arn": arn }))
            .await;
        assert_eq!(
            status, 404,
            "case {arn:?}: expected 404, got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ResourceNotFoundException",
            "case {arn:?}: got {parsed}"
        );
    }
}

/// Create a fixed set of rules and return their ARNs in creation order.
async fn seed_rules(emulator: &Emulator, count: usize) -> Vec<String> {
    let mut arns = Vec::with_capacity(count);
    for index in 0..count {
        let created = emulator
            .ok(
                "seed",
                "/createNotificationRule",
                &json!({
                    "Name": format!("seeded-{index}"),
                    "Resource": format!("arn:aws:codecommit:us-east-1:123456789012:Repo{}", index % 2),
                    "DetailType": "BASIC",
                    "EventTypeIds": ["codecommit-repository-pull-request-created"],
                    "Targets": [{
                        "TargetType": "SNS",
                        "TargetAddress": format!("arn:aws:sns:us-east-1:123456789012:topic-{index}"),
                    }],
                }),
            )
            .await;
        arns.push(string(&created, "Arn"));
    }
    arns
}

/// Whatever page size a client asks for, following `NextToken` walks every rule exactly
/// once, in the account's creation order.
#[tokio::test]
async fn paging_with_any_max_results_walks_every_rule_once() {
    let emulator = Emulator::new();
    let expected = seed_rules(&emulator, 7).await;

    // Row 1 is the modelled minimum page size, row 5 is the whole collection in one
    // page, row 100 is the modelled maximum, and `None` is a request with no
    // `MaxResults` at all.
    for max_results in [Some(1), Some(2), Some(3), Some(7), Some(100), None] {
        let case = format!("MaxResults={max_results:?}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({});
            if let Some(max) = max_results {
                body["MaxResults"] = json!(max);
            }
            if let Some(value) = &token {
                body["NextToken"] = json!(value);
            }
            let listed = emulator.ok(&case, "/listNotificationRules", &body).await;
            let page = listed
                .get("NotificationRules")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if let Some(max) = max_results {
                assert!(
                    page.len() <= max as usize,
                    "case {case:?}: a page must not exceed MaxResults, got {listed}"
                );
            }
            for summary in &page {
                let id = string(summary, "Id");
                assert!(
                    (1..=40).contains(&id.len()),
                    "case {case:?}: every summary carries the rule's own Id, got {summary}"
                );
                seen.push(string(summary, "Arn"));
            }
            pages += 1;
            assert!(
                pages <= 20,
                "case {case:?}: pagination did not terminate after {pages} pages"
            );
            match listed.get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case {case:?}: paging must yield every rule once, in creation order"
        );
    }
}

/// A `NextToken` this service did not mint does not decode.
#[tokio::test]
async fn an_undecodable_next_token_is_refused() {
    let emulator = Emulator::new();
    seed_rules(&emulator, 2).await;

    // Each row satisfies `NextToken`'s `^[\w/+=]+$` pattern, so it reaches the handler.
    for token in ["notarealtoken", "deadbeef", "AAAA====", "7"] {
        let (status, parsed) = emulator
            .post("/listNotificationRules", &json!({ "NextToken": token }))
            .await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected 400, got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "InvalidNextTokenException",
            "case {token:?}: got {parsed}"
        );
    }
}

/// `MaxResults` outside its modelled 1..100 range is refused.
#[tokio::test]
async fn max_results_outside_the_modelled_range_is_refused() {
    let emulator = Emulator::new();
    seed_rules(&emulator, 2).await;

    for (max_results, accepted) in [(0, false), (1, true), (100, true), (101, false)] {
        let (status, parsed) = emulator
            .post(
                "/listNotificationRules",
                &json!({ "MaxResults": max_results }),
            )
            .await;
        let expected = if accepted { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case MaxResults={max_results}: expected {expected}, got {status} with {parsed}"
        );
    }
}

/// Filters are ORed within one filter name and ANDed across names, as `Filters`
/// documents.
#[tokio::test]
async fn filters_or_within_a_name_and_and_across_names() {
    let emulator = Emulator::new();
    let arns = seed_rules(&emulator, 4).await;
    // `seed_rules` alternates the resource and gives each rule its own topic.
    let repo0 = "arn:aws:codecommit:us-east-1:123456789012:Repo0";
    let repo1 = "arn:aws:codecommit:us-east-1:123456789012:Repo1";

    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "one RESOURCE filter",
            json!([{ "Name": "RESOURCE", "Value": repo0 }]),
            vec![arns[0].clone(), arns[2].clone()],
        ),
        (
            "two RESOURCE filters are an OR",
            json!([
                { "Name": "RESOURCE", "Value": repo0 },
                { "Name": "RESOURCE", "Value": repo1 },
            ]),
            arns.clone(),
        ),
        (
            "RESOURCE and TARGET_ADDRESS are an AND",
            json!([
                { "Name": "RESOURCE", "Value": repo0 },
                { "Name": "TARGET_ADDRESS", "Value": "arn:aws:sns:us-east-1:123456789012:topic-2" },
            ]),
            vec![arns[2].clone()],
        ),
        (
            "an AND with no overlap matches nothing",
            json!([
                { "Name": "RESOURCE", "Value": repo1 },
                { "Name": "TARGET_ADDRESS", "Value": "arn:aws:sns:us-east-1:123456789012:topic-2" },
            ]),
            vec![],
        ),
        (
            "EVENT_TYPE_ID matches every seeded rule",
            json!([{ "Name": "EVENT_TYPE_ID", "Value": "codecommit-repository-pull-request-created" }]),
            arns.clone(),
        ),
        (
            "an event type nothing was created with matches nothing",
            json!([{ "Name": "EVENT_TYPE_ID", "Value": "codebuild-project-build-state-failed" }]),
            vec![],
        ),
    ];

    for (case, filters, expected) in cases {
        let listed = emulator
            .ok(case, "/listNotificationRules", &json!({ "Filters": filters }))
            .await;
        let got: Vec<String> = listed
            .get("NotificationRules")
            .and_then(Value::as_array)
            .map(|rules| rules.iter().map(|r| string(r, "Arn")).collect())
            .unwrap_or_default();
        assert_eq!(
            got, expected,
            "case {case:?}: unexpected filtered listing {listed}"
        );
    }
}

/// A notification rule is a REGIONAL resource: the same name is free in another region,
/// a listing names only the region it was asked in, and an ARN from another region is
/// not found through this region's endpoint.
#[tokio::test]
async fn rules_are_scoped_to_the_region_they_were_created_in() {
    let emulator = Emulator::new();
    let request = json!({
        "Name": "shared-name",
        "Resource": "arn:aws:codecommit:us-east-1:123456789012:MyRepo",
        "DetailType": "BASIC",
        "EventTypeIds": ["codecommit-repository-pull-request-created"],
        "Targets": [],
    });

    let mut arns: BTreeMap<&str, String> = BTreeMap::new();
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let (status, created) = emulator
            .post_in(region, "/createNotificationRule", &request)
            .await;
        assert_eq!(
            status, 200,
            "case {region:?}: the same name is free in each region, got {status} with {created}"
        );
        arns.insert(region, string(&created, "Arn"));
    }

    // The name IS taken within one region.
    let (status, parsed) = emulator
        .post_in("eu-west-1", "/createNotificationRule", &request)
        .await;
    assert_eq!(
        status, 409,
        "case \"the same name twice in one region\": expected 409, got {status} with {parsed}"
    );

    for (region, arn) in &arns {
        // Each region lists exactly its own rule.
        let (status, listed) = emulator
            .post_in(region, "/listNotificationRules", &json!({}))
            .await;
        assert_eq!(status, 200, "case {region:?}: got {status} with {listed}");
        let listed_arns: Vec<String> = listed
            .get("NotificationRules")
            .and_then(Value::as_array)
            .map(|rules| rules.iter().map(|r| string(r, "Arn")).collect())
            .unwrap_or_default();
        assert_eq!(
            listed_arns,
            vec![arn.clone()],
            "case {region:?}: a regional listing names only that region's rules, got {listed}"
        );

        // And the other regions' ARNs are not found here.
        for (other_region, other_arn) in &arns {
            let (status, parsed) = emulator
                .post_in(region, "/describeNotificationRule", &json!({ "Arn": other_arn }))
                .await;
            let expected = if other_region == region { 200 } else { 404 };
            assert_eq!(
                status, expected,
                "case {region:?} describing {other_region:?}: expected {expected}, got {status} with {parsed}"
            );
        }
    }
}

/// The filter list a request may carry is bounded.
#[tokio::test]
async fn an_oversized_filter_list_is_refused() {
    let emulator = Emulator::new();
    seed_rules(&emulator, 1).await;

    for (count, accepted) in [(1, true), (32, true), (33, false), (500, false)] {
        let filters: Vec<Value> = (0..count)
            .map(|index| json!({ "Name": "RESOURCE", "Value": format!("arn:aws:codecommit:us-east-1:123456789012:Repo{index}") }))
            .collect();
        let (status, parsed) = emulator
            .post("/listNotificationRules", &json!({ "Filters": filters }))
            .await;
        let expected = if accepted { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case \"{count} filters\": expected {expected}, got {status} with {parsed}"
        );
        if !accepted {
            assert_eq!(
                error_code(&parsed),
                "ValidationException",
                "case \"{count} filters\": got {parsed}"
            );
        }
    }
}

/// `Subscribe` upserts one target per address on the stored rule, and the next describe
/// sees it.
#[tokio::test]
async fn subscribe_upserts_a_target_the_next_describe_reports() {
    let emulator = Emulator::new();
    let created = emulator
        .ok("subscribe", "/createNotificationRule", &RULE_CASES[0].request())
        .await;
    let arn = string(&created, "Arn");

    // Each row is (target type, address, how many targets the rule must then hold).
    let rows = [
        ("SNS", "arn:aws:sns:us-east-1:123456789012:second-topic", 2),
        ("SNS", "arn:aws:sns:us-east-1:123456789012:third-topic", 3),
        // The same address again is one subscription, not a second one...
        ("SNS", "arn:aws:sns:us-east-1:123456789012:third-topic", 3),
        // ...and re-stating it with another type updates the type in place.
        ("AWSChatbotSlack", "arn:aws:sns:us-east-1:123456789012:third-topic", 3),
    ];

    for (target_type, address, expected_len) in rows {
        let case = format!("{target_type} {address}");
        let subscribed = emulator
            .ok(
                &case,
                "/subscribe",
                &json!({
                    "Arn": arn,
                    "Target": { "TargetType": target_type, "TargetAddress": address },
                }),
            )
            .await;
        assert_eq!(
            string(&subscribed, "Arn"),
            arn,
            "case {case:?}: subscribe reports the rule it subscribed to, got {subscribed}"
        );

        let described = emulator
            .ok(&case, "/describeNotificationRule", &json!({ "Arn": arn }))
            .await;
        let targets = described
            .get("Targets")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            targets.len(),
            expected_len,
            "case {case:?}: expected {expected_len} targets, got {described}"
        );
        let found = targets
            .iter()
            .find(|summary| string(summary, "TargetAddress") == address)
            .unwrap_or_else(|| panic!("case {case:?}: {address} is missing from {described}"));
        assert_eq!(
            string(found, "TargetType"),
            target_type,
            "case {case:?}: the last type given for an address wins, got {found}"
        );
    }
}

/// Subscribing on an ARN the store does not hold is a 404; an incomplete target is a
/// 400 `ConfigurationException`; and a rule cannot hold more targets than the model
/// allows.
#[tokio::test]
async fn subscribe_refuses_an_unknown_rule_an_incomplete_target_and_an_overfull_rule() {
    let emulator = Emulator::new();
    let created = emulator
        .ok("overfull", "/createNotificationRule", &RULE_CASES[2].request())
        .await;
    let arn = string(&created, "Arn");
    let unknown =
        "arn:aws:codestar-notifications:us-east-1:000000000000:notificationrule/22222222-2222-4222-8222-222222222222";

    let (status, parsed) = emulator
        .post(
            "/subscribe",
            &json!({
                "Arn": unknown,
                "Target": { "TargetType": "SNS", "TargetAddress": "arn:aws:sns:us-east-1:123456789012:t" },
            }),
        )
        .await;
    assert_eq!(
        status, 404,
        "case \"unknown rule\": expected 404, got {status} with {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ResourceNotFoundException",
        "case \"unknown rule\": got {parsed}"
    );

    for (case, target) in [
        ("no address", json!({ "TargetType": "SNS" })),
        (
            "no type",
            json!({ "TargetAddress": "arn:aws:sns:us-east-1:123456789012:t" }),
        ),
    ] {
        let (status, parsed) = emulator
            .post("/subscribe", &json!({ "Arn": arn, "Target": target }))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ConfigurationException",
            "case {case:?}: got {parsed}"
        );
    }

    // `Targets` is `@length(min: 0, max: 10)`: the tenth address fits, the eleventh does
    // not.
    for index in 0..11 {
        let address = format!("arn:aws:sns:us-east-1:123456789012:fill-{index}");
        let (status, parsed) = emulator
            .post(
                "/subscribe",
                &json!({
                    "Arn": arn,
                    "Target": { "TargetType": "SNS", "TargetAddress": address },
                }),
            )
            .await;
        let expected = if index < 10 { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case \"target {index}\": expected {expected}, got {status} with {parsed}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&parsed),
                "ConfigurationException",
                "case \"target {index}\": got {parsed}"
            );
        }
    }
}

/// `UpdateNotificationRule` changes the members it is given and nothing else, and the
/// change is what the next describe reports.
#[tokio::test]
async fn update_mutates_only_the_members_it_is_given() {
    let emulator = Emulator::new();

    // Each row is (label, the patch, the members it must change).
    let rows: Vec<(&str, Value)> = vec![
        ("status only", json!({ "Status": "DISABLED" })),
        ("detail type only", json!({ "DetailType": "BASIC" })),
        ("name only", json!({ "Name": "renamed-rule" })),
        (
            "event type ids only",
            json!({ "EventTypeIds": ["codebuild-project-build-state-failed"] }),
        ),
        (
            "targets only",
            json!({ "Targets": [{ "TargetType": "SNS", "TargetAddress": "arn:aws:sns:us-east-1:123456789012:replacement" }] }),
        ),
        (
            "several members at once",
            json!({ "Name": "multi-updated", "Status": "DISABLED", "DetailType": "BASIC" }),
        ),
    ];

    for (index, (case, patch)) in rows.iter().enumerate() {
        let mut create = RULE_CASES[0].request();
        create["Name"] = json!(format!("update-case-{index}"));
        let created = emulator.ok(case, "/createNotificationRule", &create).await;
        let arn = string(&created, "Arn");
        let before = emulator
            .ok(case, "/describeNotificationRule", &json!({ "Arn": arn }))
            .await;

        // A measurable gap, so the bumped LastModifiedTimestamp is observable.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;

        let mut body = patch.clone();
        body["Arn"] = json!(arn);
        let updated = emulator.ok(case, "/updateNotificationRule", &body).await;
        assert_eq!(
            updated,
            json!({}),
            "case {case:?}: UpdateNotificationRule has no output members, got {updated}"
        );

        let after = emulator
            .ok(case, "/describeNotificationRule", &json!({ "Arn": arn }))
            .await;
        assert!(
            number(&after, "LastModifiedTimestamp") > number(&before, "LastModifiedTimestamp"),
            "case {case:?}: LastModifiedTimestamp must be bumped, got {} then {}",
            number(&before, "LastModifiedTimestamp"),
            number(&after, "LastModifiedTimestamp")
        );
        assert_eq!(
            number(&after, "CreatedTimestamp"),
            number(&before, "CreatedTimestamp"),
            "case {case:?}: CreatedTimestamp never changes, got {after}"
        );

        for member in ["Name", "Status", "DetailType"] {
            let expected = patch
                .get(member)
                .and_then(Value::as_str)
                .map_or_else(|| string(&before, member), str::to_string);
            assert_eq!(
                string(&after, member),
                expected,
                "case {case:?}: expected {member} {expected:?}, got {after}"
            );
        }

        let expected_event_type_ids = patch
            .get("EventTypeIds")
            .and_then(Value::as_array)
            .map_or_else(
                || event_type_ids_of(&before),
                |ids| {
                    ids.iter()
                        .map(|id| id.as_str().unwrap_or_default().to_string())
                        .collect()
                },
            );
        assert_eq!(
            event_type_ids_of(&after),
            expected_event_type_ids,
            "case {case:?}: expected EventTypes {expected_event_type_ids:?}, got {after}"
        );

        let expected_addresses = patch.get("Targets").and_then(Value::as_array).map_or_else(
            || target_addresses_of(&before),
            |targets| {
                targets
                    .iter()
                    .map(|target| string(target, "TargetAddress"))
                    .collect()
            },
        );
        assert_eq!(
            target_addresses_of(&after),
            expected_addresses,
            "case {case:?}: expected Targets {expected_addresses:?}, got {after}"
        );

        // Tags are untouched by this operation ("To add or remove tags for a
        // notification rule, you must use TagResource and UntagResource").
        assert_eq!(
            tags_of(&after),
            tags_of(&before),
            "case {case:?}: Tags must be untouched, got {after}"
        );
    }
}

fn event_type_ids_of(body: &Value) -> Vec<String> {
    body.get("EventTypes")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|summary| string(summary, "EventTypeId"))
                .collect()
        })
        .unwrap_or_default()
}

pub(crate) fn target_addresses_of(body: &Value) -> Vec<String> {
    body.get("Targets")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|summary| string(summary, "TargetAddress"))
                .collect()
        })
        .unwrap_or_default()
}

/// Updating an ARN the store does not hold is a 404, and a rename onto a name another
/// rule already holds is refused.
#[tokio::test]
async fn update_refuses_an_unknown_rule_and_a_name_already_taken() {
    let emulator = Emulator::new();
    let first = emulator
        .ok("taken", "/createNotificationRule", &RULE_CASES[0].request())
        .await;
    let mut second_request = RULE_CASES[1].request();
    second_request["Name"] = json!("the-other-rule");
    let second = emulator
        .ok("taken", "/createNotificationRule", &second_request)
        .await;

    let (status, parsed) = emulator
        .post(
            "/updateNotificationRule",
            &json!({
                "Arn": "arn:aws:codestar-notifications:us-east-1:000000000000:notificationrule/33333333-3333-4333-8333-333333333333",
                "Status": "DISABLED",
            }),
        )
        .await;
    assert_eq!(
        status, 404,
        "case \"unknown rule\": expected 404, got {status} with {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ResourceNotFoundException",
        "case \"unknown rule\": got {parsed}"
    );

    let (status, parsed) = emulator
        .post(
            "/updateNotificationRule",
            &json!({ "Arn": string(&second, "Arn"), "Name": RULE_CASES[0].name }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"name already taken\": expected 400, got {status} with {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "ValidationException",
        "case \"name already taken\": got {parsed}"
    );

    // Renaming a rule to its OWN name is not a conflict.
    let (status, parsed) = emulator
        .post(
            "/updateNotificationRule",
            &json!({ "Arn": string(&first, "Arn"), "Name": RULE_CASES[0].name }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"rename to its own name\": expected 200, got {status} with {parsed}"
    );
}

/// A renamed rule frees its old name and holds the new one.
#[tokio::test]
async fn renaming_a_rule_moves_the_name_index() {
    let emulator = Emulator::new();
    let created = emulator
        .ok("rename", "/createNotificationRule", &RULE_CASES[0].request())
        .await;
    let arn = string(&created, "Arn");

    emulator
        .ok(
            "rename",
            "/updateNotificationRule",
            &json!({ "Arn": arn, "Name": "renamed" }),
        )
        .await;

    // The old name is free again...
    let mut reuse = RULE_CASES[0].request();
    reuse["Targets"] = json!([]);
    emulator
        .ok("rename", "/createNotificationRule", &reuse)
        .await;

    // ...and the new one is taken.
    let mut taken = RULE_CASES[0].request();
    taken["Name"] = json!("renamed");
    let (status, parsed) = emulator.post("/createNotificationRule", &taken).await;
    assert_eq!(
        status, 409,
        "case \"the new name is taken\": expected 409, got {status} with {parsed}"
    );
}

/// Delete removes the rule from the store: describe answers 404, the listing no longer
/// holds it, its name is reusable, and a second delete is a success that deletes
/// nothing (the operation models no not-found error).
#[tokio::test]
async fn delete_removes_the_rule_from_the_store() {
    let emulator = Emulator::new();
    let arns = seed_rules(&emulator, 3).await;

    for (index, arn) in arns.iter().enumerate() {
        let case = format!("delete {index}");
        let deleted = emulator
            .ok(&case, "/deleteNotificationRule", &json!({ "Arn": arn }))
            .await;
        assert_eq!(
            string(&deleted, "Arn"),
            *arn,
            "case {case:?}: delete reports the ARN of the rule it deleted, got {deleted}"
        );

        let (status, parsed) = emulator
            .post("/describeNotificationRule", &json!({ "Arn": arn }))
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: a deleted rule is gone, got {status} with {parsed}"
        );

        let listed = emulator
            .ok(&case, "/listNotificationRules", &json!({}))
            .await;
        let remaining: Vec<String> = listed
            .get("NotificationRules")
            .and_then(Value::as_array)
            .map(|rules| rules.iter().map(|r| string(r, "Arn")).collect())
            .unwrap_or_default();
        assert_eq!(
            remaining,
            arns[index + 1..].to_vec(),
            "case {case:?}: the listing must hold exactly the rules that are left, got {listed}"
        );

        // Deleting it again changes nothing and is not an error.
        let (status, parsed) = emulator
            .post("/deleteNotificationRule", &json!({ "Arn": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: delete models no not-found error, got {status} with {parsed}"
        );

        // The name the rule held is free again. Create it anew, then delete it so the
        // next row's listing assertion still sees only the seeded rules.
        let recreated = emulator
            .ok(
                &case,
                "/createNotificationRule",
                &json!({
                    "Name": format!("seeded-{index}"),
                    "Resource": "arn:aws:codecommit:us-east-1:123456789012:MyRepo",
                    "DetailType": "BASIC",
                    "EventTypeIds": ["codecommit-repository-pull-request-created"],
                    "Targets": [],
                }),
            )
            .await;
        emulator
            .ok(
                &case,
                "/deleteNotificationRule",
                &json!({ "Arn": string(&recreated, "Arn") }),
            )
            .await;
    }
}

/// A target the service subscribes is `ACTIVE` as soon as it exists: the association
/// is usable when the call that created it returns, so there is no transient status for
/// a later read to settle.
#[tokio::test]
async fn a_subscribed_target_is_active_from_the_first_read() {
    let emulator = Emulator::new();
    let created = emulator
        .ok("activation", "/createNotificationRule", &RULE_CASES[0].request())
        .await;
    let arn = string(&created, "Arn");

    let described = emulator
        .ok("activation", "/describeNotificationRule", &json!({ "Arn": arn }))
        .await;
    assert_eq!(
        target_statuses_of(&described),
        vec!["ACTIVE".to_string()],
        "case \"immediately after create\": expected ACTIVE, got {described}"
    );

    // Reading it again changes nothing: the status is terminal, not settled by a read.
    let described = emulator
        .ok("activation", "/describeNotificationRule", &json!({ "Arn": arn }))
        .await;
    assert_eq!(
        target_statuses_of(&described),
        vec!["ACTIVE".to_string()],
        "case \"a second read\": expected ACTIVE, got {described}"
    );

    // A target subscribed later is ACTIVE straight away too.
    emulator
        .ok(
            "activation",
            "/subscribe",
            &json!({
                "Arn": arn,
                "Target": { "TargetType": "SNS", "TargetAddress": "arn:aws:sns:us-east-1:123456789012:late" },
            }),
        )
        .await;
    let described = emulator
        .ok("activation", "/describeNotificationRule", &json!({ "Arn": arn }))
        .await;
    assert_eq!(
        target_statuses_of(&described),
        vec!["ACTIVE".to_string(), "ACTIVE".to_string()],
        "case \"a newly subscribed target\": expected both ACTIVE, got {described}"
    );
}

pub(crate) fn target_statuses_of(body: &Value) -> Vec<String> {
    body.get("Targets")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|summary| string(summary, "TargetStatus"))
                .collect()
        })
        .unwrap_or_default()
}

/// One `ClientRequestToken` used on two different operations is two records, not one:
/// neither operation hands back the other's resource.
#[tokio::test]
async fn one_token_used_on_create_and_subscribe_does_not_collide() {
    let emulator = Emulator::new();
    let token = "shared-token";

    let mut create = RULE_CASES[0].request();
    create["ClientRequestToken"] = json!(token);
    let created = emulator.ok(token, "/createNotificationRule", &create).await;
    let arn = string(&created, "Arn");

    let subscribed = emulator
        .ok(
            token,
            "/subscribe",
            &json!({
                "Arn": arn,
                "Target": { "TargetType": "SNS", "TargetAddress": "arn:aws:sns:us-east-1:123456789012:shared" },
                "ClientRequestToken": token,
            }),
        )
        .await;
    assert_eq!(
        string(&subscribed, "Arn"),
        arn,
        "case {token:?}: subscribe must answer for the rule it was given, got {subscribed}"
    );

    // Both replays still answer from their own record.
    let create_replay = emulator.ok(token, "/createNotificationRule", &create).await;
    assert_eq!(
        string(&create_replay, "Arn"),
        arn,
        "case {token:?}: the create replay must report the original rule, got {create_replay}"
    );
    let described = emulator
        .ok(token, "/describeNotificationRule", &json!({ "Arn": arn }))
        .await;
    assert_eq!(
        target_addresses_of(&described).len(),
        2,
        "case {token:?}: the subscribe must have landed, got {described}"
    );
}

/// The account holds a bounded number of rules: the request past the cap is refused
/// with the modelled `LimitExceededException` rather than growing the store further.
#[tokio::test]
async fn the_rule_store_is_bounded_per_account() {
    let emulator = Emulator::new();
    let cap = codestar::state::MAX_RULES_PER_REGION;

    for index in 0..cap {
        let (status, parsed) = emulator
            .post(
                "/createNotificationRule",
                &json!({
                    "Name": format!("bounded-{index}"),
                    "Resource": "arn:aws:codecommit:us-east-1:123456789012:MyRepo",
                    "DetailType": "BASIC",
                    "EventTypeIds": ["codecommit-repository-pull-request-created"],
                    "Targets": [],
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case \"rule {index} of {cap}\": expected 200, got {status} with {parsed}"
        );
    }

    let (status, parsed) = emulator
        .post(
            "/createNotificationRule",
            &json!({
                "Name": "one-too-many",
                "Resource": "arn:aws:codecommit:us-east-1:123456789012:MyRepo",
                "DetailType": "BASIC",
                "EventTypeIds": ["codecommit-repository-pull-request-created"],
                "Targets": [],
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"one past the cap\": expected 400, got {status} with {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "LimitExceededException",
        "case \"one past the cap\": got {parsed}"
    );
}

/// The store is bounded by a figure that does NOT depend on a request-controlled key:
/// the region a rule lands in comes from the caller's own credential scope, so a client
/// that varies it must not be able to open a fresh per-region budget forever. Once the
/// whole store is at `MAX_RULES_TOTAL`, a create in a region that holds NO rules at all
/// is still refused with the modelled `LimitExceededException`.
#[tokio::test]
async fn the_rule_store_is_bounded_across_every_region() {
    let emulator = Emulator::new();
    let per_region = codestar::state::MAX_RULES_PER_REGION;
    let total = codestar::state::MAX_RULES_TOTAL;
    // Each region takes the per-region cap, so filling the global cap takes this many
    // of them — and the next region after that holds nothing.
    let regions = total / per_region;

    for region_index in 0..regions {
        let region = format!("eu-region-{region_index}");
        for index in 0..per_region {
            let (status, parsed) = emulator
                .post_in(
                    &region,
                    "/createNotificationRule",
                    &json!({
                        "Name": format!("spread-{index}"),
                        "Resource": "arn:aws:codecommit:us-east-1:123456789012:MyRepo",
                        "DetailType": "BASIC",
                        "EventTypeIds": ["codecommit-repository-pull-request-created"],
                        "Targets": [],
                    }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case \"rule {index} in {region}\": expected 200, got {status} with {parsed}"
            );
        }
    }

    let (status, parsed) = emulator
        .post_in(
            "eu-region-empty",
            "/createNotificationRule",
            &json!({
                "Name": "one-region-too-many",
                "Resource": "arn:aws:codecommit:us-east-1:123456789012:MyRepo",
                "DetailType": "BASIC",
                "EventTypeIds": ["codecommit-repository-pull-request-created"],
                "Targets": [],
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"a fresh region past the global cap of {total}\": expected 400, got {status} with {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        "LimitExceededException",
        "case \"a fresh region past the global cap of {total}\": got {parsed}"
    );
}

/// An address no rule subscribes to any more is released, so a later subscription to it
/// is a NEW subscription — reported, with its own status, by the rule that made it.
#[tokio::test]
async fn a_target_is_released_when_its_last_rule_goes_away() {
    let emulator = Emulator::new();
    let address = "arn:aws:sns:us-east-1:123456789012:recycled";

    let request = json!({
        "Name": "recycler",
        "Resource": "arn:aws:codecommit:us-east-1:123456789012:MyRepo",
        "DetailType": "BASIC",
        "EventTypeIds": ["codecommit-repository-pull-request-created"],
        "Targets": [{ "TargetType": "SNS", "TargetAddress": address }],
    });
    let created = emulator
        .ok("recycled", "/createNotificationRule", &request)
        .await;
    let arn = string(&created, "Arn");
    let described = emulator
        .ok("recycled", "/describeNotificationRule", &json!({ "Arn": arn }))
        .await;
    assert_eq!(
        target_statuses_of(&described),
        vec!["ACTIVE".to_string()],
        "case \"before the rule goes away\": expected ACTIVE, got {described}"
    );

    emulator
        .ok("recycled", "/deleteNotificationRule", &json!({ "Arn": arn }))
        .await;
    let recreated = emulator
        .ok("recycled", "/createNotificationRule", &request)
        .await;
    let described = emulator
        .ok(
            "recycled",
            "/describeNotificationRule",
            &json!({ "Arn": string(&recreated, "Arn") }),
        )
        .await;
    assert_eq!(
        target_addresses_of(&described),
        vec![address.to_string()],
        "case \"a fresh subscription to a released address\": expected the address back, got {described}"
    );
    assert_eq!(
        target_statuses_of(&described),
        vec!["ACTIVE".to_string()],
        "case \"a fresh subscription to a released address\": expected ACTIVE, got {described}"
    );
}

/// A bad enum value is refused rather than stored.
#[tokio::test]
async fn an_unmodelled_enum_value_is_refused() {
    let emulator = Emulator::new();

    for (index, (member, value)) in [
        ("DetailType", "SUMMARY"),
        ("DetailType", "full"),
        ("Status", "PAUSED"),
        ("Status", "enabled"),
    ]
    .iter()
    .enumerate()
    {
        let mut body = json!({
            "Name": format!("enum-case-{index}"),
            "Resource": "arn:aws:codecommit:us-east-1:123456789012:MyRepo",
            "DetailType": "BASIC",
            "EventTypeIds": ["codecommit-repository-pull-request-created"],
            "Targets": [],
        });
        body[*member] = json!(value);
        let (status, parsed) = emulator.post("/createNotificationRule", &body).await;
        assert_eq!(
            status, 400,
            "case {member:?}={value:?}: expected 400, got {status} with {parsed}"
        );
    }
}
