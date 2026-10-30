// SPDX-License-Identifier: CC0-1.0

//! The `Message` every Security Hub CSPM refusal class carries.
//!
//! RULE: a refusal's `Message` is the text the service emits for that error class —
//! its fixed wording verbatim, carrying only the variable parts the service
//! interpolates, each taken from the request member it names (or, for a batch, from
//! every rejected entry of the batch) and nothing else.
//!
//! Each suite below drives the real restJson1 wire path through `MultiServiceRouter`
//! and compares the serialized `Message` with the template rendered independently of
//! the handler, so a re-worded message, an extra interpolated value or a batch
//! refusal that names only its first offender fails here.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The account every emulated caller presents.
const ACCOUNT_ID: &str = "000000000000";

/// One emulator with the Security Hub service mounted, driven over HTTP for one
/// signing Region.
struct Harness {
    router: MultiServiceRouter,
    region: String,
}

impl Harness {
    fn new(region: &str) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_securityhub_securityhubapiservice::EmulatedService::new(engine),
        );
        Self {
            router,
            region: region.to_string(),
        }
    }

    /// Dispatch one request and return `(status, parsed body)`. An empty body parses
    /// as `Value::Null`, which is what an operation with no output members returns.
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let serialized = body.map(|b| serde_json::to_vec(&b).expect("serializable request body"));
        let region = &self.region;
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/{region}/securityhub/aws4_request, \
                     SignedHeaders=host, Signature=0000"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(serialized.unwrap_or_default()))
            .expect("valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.into_body();
        let parsed = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    async fn enable(&self, body: Value) -> (u16, Value) {
        self.call("POST", "/accounts", Some(body)).await
    }

    async fn create_action_target(&self, id: &str, name: &str, description: &str) -> (u16, Value) {
        self.call(
            "POST",
            "/actionTargets",
            Some(json!({ "Id": id, "Name": name, "Description": description })),
        )
        .await
    }

    async fn delete_action_target(&self, arn: &str) -> (u16, Value) {
        self.call("DELETE", &format!("/actionTargets/{arn}"), None)
            .await
    }

    async fn batch_enable_standards(&self, standards_arns: &[String]) -> (u16, Value) {
        let requests: Vec<Value> = standards_arns
            .iter()
            .map(|arn| json!({ "StandardsArn": arn }))
            .collect();
        self.call(
            "POST",
            "/standards/register",
            Some(json!({ "StandardsSubscriptionRequests": requests })),
        )
        .await
    }

    /// A hub subscribed in this Region: every action target operation is hub-scoped.
    async fn subscribed(region: &str) -> Self {
        let harness = Self::new(region);
        let (status, body) = harness.enable(json!({})).await;
        assert_eq!(
            status, 200,
            "EnableSecurityHub must succeed, got {status} with {body}"
        );
        harness
    }
}

/// The refusal's `Message`, under either casing the restJson1 error shape may carry.
fn message(body: &Value) -> String {
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_else(|| panic!("a refusal carries a Message: {body}"))
        .to_string()
}

/// The refusal's error code, under either casing the wire shape may carry.
fn error_code(body: &Value) -> String {
    body["Code"]
        .as_str()
        .or_else(|| body["code"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// The ARN Security Hub CSPM names a custom action target by.
fn action_target_arn(region: &str, id: &str) -> String {
    format!("arn:aws:securityhub:{region}:{ACCOUNT_ID}:action/custom/{id}")
}

/// A `StandardsArn` of the published catalog, so a row can mix enableable entries
/// with rejected ones (`crate::state::STANDARDS_CATALOG`, which is also what
/// `DescribeStandards` reports).
fn enableable_standards_arn(region: &str) -> String {
    format!("arn:aws:securityhub:{region}::standards/aws-foundational-security-best-practices/v/1.0.0")
}

/// The `Invalid StandardsSubscriptionRequest(s)` message, rendered here from the
/// rejected ARNs alone: the fixed text, then a JSON array whose entries carry the
/// lower-camel `standardsArn` member. Built with `Value::String` so a value needing
/// JSON escaping is escaped the way the wire carries it.
fn expected_invalid_requests_message(rejected: &[String]) -> String {
    let entries: Vec<String> = rejected
        .iter()
        .map(|arn| format!("{{\"standardsArn\":{}}}", Value::String(arn.clone())))
        .collect();
    format!(
        "Invalid StandardsSubscriptionRequest(s): [{}]",
        entries.join(",")
    )
}

/// RULE: `EnableSecurityHub` refuses an account/Region that is already subscribed
/// with one WHOLLY FIXED message — it interpolates neither the account id nor the
/// Region, so the rows differ in signing Region and in the enable request's members
/// and all expect the same string.
#[tokio::test]
async fn a_second_enable_is_refused_with_the_fixed_already_subscribed_message() {
    /// What the refusal says, start to end.
    const EXPECTED: &str = "Account is already subscribed to Security Hub";

    let cases: [(&str, &str, Value); 4] = [
        ("defaults", "us-east-1", json!({})),
        (
            "default standards declined",
            "eu-west-1",
            json!({ "EnableDefaultStandards": false }),
        ),
        (
            "a tagged hub",
            "ap-southeast-2",
            json!({ "Tags": { "team": "secops" } }),
        ),
        (
            "an explicit control finding generator",
            "us-west-2",
            json!({ "ControlFindingGenerator": "STANDARD_CONTROL" }),
        ),
    ];

    for (case, region, first) in cases {
        let harness = Harness::new(region);
        let (status, body) = harness.enable(first).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the first enable expected 200, got {status} with {body}"
        );

        // The second enable, and a third with different members: the refusal is about
        // the subscription that exists, so nothing in the request reaches the message.
        for (label, retry) in [
            ("identical request", json!({})),
            (
                "different members",
                json!({ "EnableDefaultStandards": false, "Tags": { "k": "v" } }),
            ),
        ] {
            let (status, body) = harness.enable(retry).await;
            assert_eq!(
                status, 409,
                "case {case:?} ({label}): expected 409, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceConflictException",
                "case {case:?} ({label}): expected ResourceConflictException, got {body}"
            );
            let actual = message(&body);
            assert_eq!(
                actual, EXPECTED,
                "case {case:?} ({label}): expected message {EXPECTED:?}, got {actual:?}"
            );
            assert!(
                !actual.contains(region) && !actual.contains(ACCOUNT_ID),
                "case {case:?} ({label}): the fixed message interpolates no Region or account, got {actual:?}"
            );
        }
    }
}

/// RULE: `CreateActionTarget` refuses an `Id` the hub already holds with a message
/// keyed on the `Id` the request carried — not on the ARN the first create derived
/// from it.
///
/// The rows span the `Id` class: shortest legal id, mixed case, all digits, an id at
/// the documented 20-character bound, and an id that is a substring of another.
#[tokio::test]
async fn a_duplicate_action_target_id_is_refused_with_the_requested_id() {
    const REGION: &str = "us-east-1";
    /// `Id`'s documented bound: "Can contain up to 20 alphanumeric characters"
    /// (`smithy_query(query="doc", shape="CreateActionTargetRequest", member="Id")`).
    const MAX_ID_LENGTH: usize = 20;

    let id_at_bound = "i".repeat(MAX_ID_LENGTH);
    let ids: [&str; 5] = [
        "a",
        "Remediation1",
        "0123456789",
        id_at_bound.as_str(),
        "Remediation12",
    ];

    let harness = Harness::subscribed(REGION).await;
    for id in ids {
        let (status, body) = harness
            .create_action_target(id, "Custom action", "Created by a test")
            .await;
        assert_eq!(
            status, 200,
            "case {id:?}: the first create expected 200, got {status} with {body}"
        );
    }

    for id in ids {
        // Re-submitting the same values and submitting different ones are the same
        // conflict: the `Id` identifies the resource, so it is what the message names.
        for (label, name, description) in [
            ("identical values", "Custom action", "Created by a test"),
            ("different values", "Renamed", "Re-described"),
        ] {
            let (status, body) = harness.create_action_target(id, name, description).await;
            assert_eq!(
                status, 409,
                "case {id:?} ({label}): expected 409, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceConflictException",
                "case {id:?} ({label}): expected ResourceConflictException, got {body}"
            );
            let expected = format!("Action with Id: {id} already exists");
            let actual = message(&body);
            assert_eq!(
                actual, expected,
                "case {id:?} ({label}): expected message {expected:?}, got {actual:?}"
            );
            assert!(
                !actual.contains(&action_target_arn(REGION, id)),
                "case {id:?} ({label}): the message names the requested Id, not the derived ARN, got {actual:?}"
            );
        }
    }
}

/// RULE: `DeleteActionTarget` refuses an `ActionTargetArn` that names no custom action
/// target of the caller's hub with a message that quotes back the ARN the request
/// carried, whatever its shape.
///
/// The rows span the forms an unknown ARN takes: never created, created then deleted,
/// the same id in another Region or account, a hub ARN, and a value that is not an ARN
/// at all.
#[tokio::test]
async fn an_unknown_action_target_arn_is_refused_with_the_arn_the_request_carried() {
    const REGION: &str = "us-east-1";

    let harness = Harness::subscribed(REGION).await;
    for id in ["kept", "removed"] {
        let (status, body) = harness
            .create_action_target(id, "Custom action", "Created by a test")
            .await;
        assert_eq!(
            status, 200,
            "case {id:?}: create expected 200, got {status} with {body}"
        );
    }
    let (status, body) = harness
        .delete_action_target(&action_target_arn(REGION, "removed"))
        .await;
    assert_eq!(
        status, 200,
        "the first delete expected 200, got {status} with {body}"
    );

    let cases: [(&str, String); 6] = [
        (
            "an id that was never created",
            action_target_arn(REGION, "missing"),
        ),
        (
            "an id that was already deleted",
            action_target_arn(REGION, "removed"),
        ),
        (
            "the same id in another Region",
            action_target_arn("eu-west-1", "kept"),
        ),
        (
            "the same id in another account",
            format!("arn:aws:securityhub:{REGION}:999999999999:action/custom/kept"),
        ),
        (
            "a hub ARN rather than an action ARN",
            format!("arn:aws:securityhub:{REGION}:{ACCOUNT_ID}:hub/default"),
        ),
        ("a bare id rather than an ARN", "kept".to_string()),
    ];

    for (case, arn) in cases {
        let (status, body) = harness.delete_action_target(&arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        let expected = format!("Given Arn: {arn} does not exist");
        let actual = message(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected message {expected:?}, got {actual:?}"
        );
    }
}

/// RULE: `BatchEnableStandards` refuses a batch carrying any `StandardsArn` that is
/// not an enableable standard, and the refusal lists EVERY rejected entry of that
/// batch — in request order, as a JSON array of `standardsArn` objects — and no entry
/// it accepted.
///
/// The rows differ in how many entries are rejected, where they sit in the batch,
/// whether a rejected ARN repeats, and whether its value needs JSON escaping.
#[tokio::test]
async fn an_unenableable_standards_batch_is_refused_with_every_rejected_entry() {
    const REGION: &str = "us-east-1";

    let good = enableable_standards_arn(REGION);
    let bogus_a = format!("arn:aws:securityhub:{REGION}::standards/sim-does-not-exist/v/1.0.0");
    let bogus_b = format!("arn:aws:securityhub:{REGION}::standards/pci-dss/v/9.9.9");
    let bogus_quoted = format!("arn:aws:securityhub:{REGION}::standards/\"quoted\\slug\"/v/1.0.0");

    // (case, the batch as sent, the entries the refusal must list, in order)
    let cases: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        (
            "one rejected entry",
            vec![bogus_a.clone()],
            vec![bogus_a.clone()],
        ),
        (
            "a rejected entry before an enableable one",
            vec![bogus_a.clone(), good.clone()],
            vec![bogus_a.clone()],
        ),
        (
            "a rejected entry after an enableable one",
            vec![good.clone(), bogus_b.clone()],
            vec![bogus_b.clone()],
        ),
        (
            "two rejected entries around an enableable one",
            vec![bogus_a.clone(), good.clone(), bogus_b.clone()],
            vec![bogus_a.clone(), bogus_b.clone()],
        ),
        (
            "the same rejected entry twice",
            vec![bogus_b.clone(), bogus_b.clone()],
            vec![bogus_b.clone(), bogus_b.clone()],
        ),
        (
            "a rejected value that is not an ARN",
            vec![good.clone(), "not-a-standard".to_string()],
            vec!["not-a-standard".to_string()],
        ),
        (
            "a rejected value needing JSON escaping",
            vec![bogus_quoted.clone()],
            vec![bogus_quoted.clone()],
        ),
        (
            "three rejected entries, reported in request order",
            vec![
                bogus_b.clone(),
                good.clone(),
                "not-a-standard".to_string(),
                bogus_a.clone(),
            ],
            vec![
                bogus_b.clone(),
                "not-a-standard".to_string(),
                bogus_a.clone(),
            ],
        ),
    ];

    let harness = Harness::subscribed(REGION).await;
    for (case, requested, rejected) in cases {
        let (status, body) = harness.batch_enable_standards(&requested).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {case:?}: expected InvalidInputException, got {body}"
        );
        let expected = expected_invalid_requests_message(&rejected);
        let actual = message(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected message {expected:?}, got {actual:?}"
        );
        if requested.contains(&good) {
            assert!(
                !actual.contains(good.as_str()),
                "case {case:?}: an accepted entry must not be listed as rejected, got {actual:?}"
            );
        }
    }

    // A batch of enableable entries is not refused at all, so the message above is
    // about the rejected entries and not about the call's shape.
    let (status, body) = harness
        .batch_enable_standards(std::slice::from_ref(&good))
        .await;
    assert_eq!(
        status, 200,
        "an enableable batch expected 200, got {status} with {body}"
    );
}
