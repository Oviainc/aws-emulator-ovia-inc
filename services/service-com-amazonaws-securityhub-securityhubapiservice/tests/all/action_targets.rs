// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the Security Hub CSPM custom action targets:
//! CreateActionTarget → DescribeActionTargets → DeleteActionTarget.
//!
//! Every case drives the real restJson1 wire path through `MultiServiceRouter`, so a
//! route, a status code or a serialized member that is wrong on the wire fails here.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region the test signs for; the handler reads it out of the SigV4 credential
/// scope, exactly as a signed SDK request carries it.
const REGION: &str = "us-east-1";

/// The account every emulated caller presents.
const ACCOUNT_ID: &str = "000000000000";

/// `Id`'s documented bound: "Can contain up to 20 alphanumeric characters"
/// (`smithy_query(query="doc", shape="CreateActionTargetRequest", member="Id")`).
const MAX_ID_LENGTH: usize = 20;

/// `Name`'s documented bound: "Can contain up to 20 characters"
/// (`smithy_query(query="doc", shape="CreateActionTargetRequest", member="Name")`).
const MAX_NAME_LENGTH: usize = 20;

/// One emulator with the Security Hub service mounted, driven over HTTP.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_securityhub_securityhubapiservice::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Dispatch one request and return `(status, parsed body)`. An empty body parses
    /// as `Value::Null`, which is what an operation with no output members returns.
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let serialized = body.map(|b| serde_json::to_vec(&b).expect("serializable request body"));
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/{REGION}/securityhub/aws4_request, \
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

    async fn enable(&self) -> (u16, Value) {
        self.call("POST", "/accounts", Some(json!({}))).await
    }

    async fn create(&self, id: &str, name: &str, description: &str) -> (u16, Value) {
        self.call(
            "POST",
            "/actionTargets",
            Some(json!({ "Id": id, "Name": name, "Description": description })),
        )
        .await
    }

    async fn describe(&self, body: Value) -> (u16, Value) {
        self.call("POST", "/actionTargets/get", Some(body)).await
    }

    async fn delete(&self, arn: &str) -> (u16, Value) {
        self.call("DELETE", &format!("/actionTargets/{arn}"), None)
            .await
    }

    /// A subscribed hub: every action target operation is hub-scoped.
    async fn subscribed() -> Self {
        let harness = Self::new();
        let (status, body) = harness.enable().await;
        assert_eq!(
            status, 200,
            "EnableSecurityHub must succeed, got {status} with {body}"
        );
        harness
    }
}

/// The ARN Security Hub CSPM names a custom action target by.
fn expected_arn(id: &str) -> String {
    format!("arn:aws:securityhub:{REGION}:{ACCOUNT_ID}:action/custom/{id}")
}

/// The `(ActionTargetArn, Name, Description)` triples a `DescribeActionTargets`
/// response reports, in the order the response carries them.
fn reported(body: &Value) -> Vec<(String, String, String)> {
    const NO_TARGETS: &[Value] = &[];
    body["ActionTargets"]
        .as_array()
        .map_or(NO_TARGETS, |targets| targets.as_slice())
        .iter()
        .map(|target| {
            (
                target["ActionTargetArn"].as_str().unwrap_or("").to_string(),
                target["Name"].as_str().unwrap_or("").to_string(),
                target["Description"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

/// RULE: `CreateActionTarget` persists one custom action target per `Id` in the
/// caller's account and Region, names it
/// `arn:aws:securityhub:<region>:<account>:action/custom/<Id>`, and
/// `DescribeActionTargets` reports exactly the `Name` and `Description` submitted.
///
/// The table spans the `Id`/`Name`/`Description` class: shortest legal id, a mixed
/// alphanumeric id, an all-digit id, an id at the documented 20-character bound, and
/// names/descriptions differing in length and in whether they carry spaces or
/// punctuation.
#[tokio::test]
async fn created_action_targets_round_trip_through_describe() {
    let long_id = "x".repeat(MAX_ID_LENGTH);
    let long_name = "n".repeat(MAX_NAME_LENGTH);
    let cases: [(&str, &str, &str); 5] = [
        ("a", "A", "Shortest legal id and name"),
        ("Remediation1", "Send to remediation", "Mixed-case id"),
        ("0123456789", "10 digits", "An all-digit id"),
        (
            long_id.as_str(),
            long_name.as_str(),
            "Id and Name at the documented 20-character bound",
        ),
        (
            "SendToJira",
            "Open a ticket",
            "Description with punctuation: commas, colons & ampersands - still fine",
        ),
    ];

    let harness = Harness::subscribed().await;
    let mut expected: Vec<(String, String, String)> = Vec::new();

    for (id, name, description) in cases {
        let case = format!("Id={id:?} Name={name:?}");
        let (status, body) = harness.create(id, name, description).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateActionTarget expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["ActionTargetArn"].as_str(),
            Some(expected_arn(id).as_str()),
            "case {case:?}: expected ARN {:?}, got {body}",
            expected_arn(id)
        );
        expected.push((expected_arn(id), name.to_string(), description.to_string()));
    }

    let (status, body) = harness.describe(json!({})).await;
    assert_eq!(
        status, 200,
        "DescribeActionTargets expected 200, got {status} with {body}"
    );
    let mut actual = reported(&body);
    actual.sort();
    expected.sort();
    assert_eq!(
        actual, expected,
        "every created target must be reported with the submitted Name and Description, got {body}"
    );
    assert!(
        body.get("NextToken").is_none() || body["NextToken"].is_null(),
        "a single full page must carry no NextToken, got {body}"
    );

    // The read is a report: calling it again sees exactly the same targets.
    let (status, again) = harness.describe(json!({})).await;
    assert_eq!(status, 200, "second DescribeActionTargets got {status}");
    let mut actual_again = reported(&again);
    actual_again.sort();
    assert_eq!(
        actual_again, expected,
        "DescribeActionTargets must not change what a later caller sees, got {again}"
    );
}

/// RULE: an `Id` the account already holds in the Region conflicts (409) and leaves
/// the stored target exactly as the first create left it; a different `Id` in the
/// same hub is accepted.
#[tokio::test]
async fn a_duplicate_id_is_rejected_and_changes_nothing() {
    let harness = Harness::subscribed().await;
    let cases: [(&str, &str, &str); 3] = [
        ("a", "First name", "First description"),
        ("Remediation1", "Send to remediation", "Routes to the queue"),
        ("0123456789", "Digits", "An all-digit id"),
    ];

    for (id, name, description) in cases {
        let (status, body) = harness.create(id, name, description).await;
        assert_eq!(
            status, 200,
            "case {id:?}: first create expected 200, got {status} with {body}"
        );
    }

    for (id, name, description) in cases {
        // Re-submitting the same values, and submitting different ones under the
        // same id, are both conflicts: the id is what identifies the resource.
        for (label, retry_name, retry_description) in [
            ("identical values", name, description),
            ("different values", "Renamed", "Re-described"),
        ] {
            let (status, body) = harness.create(id, retry_name, retry_description).await;
            assert_eq!(
                status, 409,
                "case {id:?} with {label}: expected 409, got {status} with {body}"
            );
        }
    }

    let (status, body) = harness.describe(json!({})).await;
    assert_eq!(status, 200, "DescribeActionTargets got {status}");
    let mut actual = reported(&body);
    actual.sort();
    let mut expected: Vec<(String, String, String)> = cases
        .iter()
        .map(|(id, name, description)| {
            (
                expected_arn(id),
                (*name).to_string(),
                (*description).to_string(),
            )
        })
        .collect();
    expected.sort();
    assert_eq!(
        actual, expected,
        "a refused create must leave every stored target untouched, got {body}"
    );
}

/// RULE: `Id` is up to 20 alphanumeric characters and `Name` is up to 20 characters,
/// so a value outside either documented bound is refused (400) and persists nothing.
///
/// The rows walk each published bound and one past it, plus the non-alphanumeric
/// character classes an `Id` may not carry.
#[tokio::test]
async fn ids_and_names_outside_the_documented_bounds_are_refused() {
    let id_at_bound = "i".repeat(MAX_ID_LENGTH);
    let id_past_bound = "i".repeat(MAX_ID_LENGTH + 1);
    let name_at_bound = "n".repeat(MAX_NAME_LENGTH);
    let name_past_bound = "n".repeat(MAX_NAME_LENGTH + 1);

    let cases: [(&str, &str, &str, u16); 9] = [
        ("Id at the bound", id_at_bound.as_str(), "At bound", 200),
        ("Id one past the bound", id_past_bound.as_str(), "Past", 400),
        ("Id with a hyphen", "send-to-jira", "Hyphen", 400),
        ("Id with an underscore", "send_to_jira", "Underscore", 400),
        ("Id with a space", "send to jira", "Space", 400),
        ("Id with punctuation", "send.to:jira", "Punctuation", 400),
        ("Id with a non-ASCII letter", "sendTo\u{00e9}", "Accent", 400),
        ("Name at the bound", "nameok1", name_at_bound.as_str(), 200),
        (
            "Name one past the bound",
            "nameok2",
            name_past_bound.as_str(),
            400,
        ),
    ];

    for (case, id, name, expected_status) in cases {
        let harness = Harness::subscribed().await;
        let (status, body) = harness.create(id, name, "A custom action").await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} with {body}"
        );

        let (describe_status, listed) = harness.describe(json!({})).await;
        assert_eq!(
            describe_status, 200,
            "case {case:?}: DescribeActionTargets expected 200, got {describe_status}"
        );
        let expected_len = usize::from(expected_status == 200);
        assert_eq!(
            reported(&listed).len(),
            expected_len,
            "case {case:?}: expected {expected_len} stored target(s), got {listed}"
        );
    }
}

/// RULE: `DeleteActionTarget` removes the target the ARN names from the caller's hub
/// and echoes that ARN; after it, the target is gone from `DescribeActionTargets` and
/// a second delete of the same ARN is `ResourceNotFoundException` (404). An `Id` is
/// free to be created again once deleted.
#[tokio::test]
async fn delete_removes_the_target_from_the_reported_list() {
    let ids = ["a", "Remediation1", "0123456789", "SendToJira"];
    let harness = Harness::subscribed().await;
    for id in ids {
        let (status, body) = harness.create(id, "Custom action", "Created by a test").await;
        assert_eq!(
            status, 200,
            "case {id:?}: create expected 200, got {status} with {body}"
        );
    }

    let mut remaining: Vec<String> = ids.iter().map(|id| expected_arn(id)).collect();
    for id in ids {
        let arn = expected_arn(id);
        let (status, body) = harness.delete(&arn).await;
        assert_eq!(
            status, 200,
            "case {id:?}: delete expected 200, got {status} with {body}"
        );
        assert_eq!(
            body["ActionTargetArn"].as_str(),
            Some(arn.as_str()),
            "case {id:?}: delete must echo the ARN it removed, got {body}"
        );

        remaining.retain(|candidate| *candidate != arn);
        let (status, listed) = harness.describe(json!({})).await;
        assert_eq!(status, 200, "case {id:?}: describe got {status}");
        let mut arns: Vec<String> = reported(&listed).into_iter().map(|(arn, ..)| arn).collect();
        arns.sort();
        let mut expected = remaining.clone();
        expected.sort();
        assert_eq!(
            arns, expected,
            "case {id:?}: expected {expected:?} to remain, got {listed}"
        );

        // Deleting the same ARN twice: the second call names a resource that is gone.
        let (status, body) = harness.delete(&arn).await;
        assert_eq!(
            status, 404,
            "case {id:?}: second delete expected 404, got {status} with {body}"
        );
    }

    // A deleted id is free again, and re-creating it mints the same ARN.
    let (status, body) = harness.create("a", "Recreated", "After a delete").await;
    assert_eq!(
        status, 200,
        "re-create after delete expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["ActionTargetArn"].as_str(),
        Some(expected_arn("a").as_str()),
        "re-create must mint the same ARN, got {body}"
    );
}

/// RULE: an ARN that names no custom action target of the caller's hub is
/// `ResourceNotFoundException` (404), whatever its shape.
#[tokio::test]
async fn deleting_an_arn_that_names_no_target_is_not_found() {
    let harness = Harness::subscribed().await;
    let (status, _) = harness.create("kept", "Kept", "Must survive every case").await;
    assert_eq!(status, 200, "seed create got {status}");

    let other_region = format!("arn:aws:securityhub:eu-west-1:{ACCOUNT_ID}:action/custom/kept");
    let other_account = format!("arn:aws:securityhub:{REGION}:999999999999:action/custom/kept");
    let cases = [
        ("an id that was never created", expected_arn("missing")),
        ("the same id in another Region", other_region),
        ("the same id in another account", other_account),
        (
            "a hub ARN rather than an action ARN",
            format!("arn:aws:securityhub:{REGION}:{ACCOUNT_ID}:hub/default"),
        ),
        ("a bare id rather than an ARN", "kept".to_string()),
    ];

    for (case, arn) in cases {
        let (status, body) = harness.delete(&arn).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} with {body}"
        );
    }

    let (status, listed) = harness.describe(json!({})).await;
    assert_eq!(status, 200, "describe got {status}");
    assert_eq!(
        reported(&listed).into_iter().map(|(arn, ..)| arn).collect::<Vec<_>>(),
        vec![expected_arn("kept")],
        "a refused delete must remove nothing, got {listed}"
    );
}

/// RULE: `ActionTargetArns` selects which of the hub's targets to report, so the
/// response is the intersection of the request's ARNs with the hub's targets, and
/// `MaxResults`/`NextToken` page that result without repeating or losing one.
#[tokio::test]
async fn describe_selects_by_arn_and_pages_the_result() {
    let ids = ["a1", "b2", "c3", "d4", "e5"];
    let harness = Harness::subscribed().await;
    for id in ids {
        let (status, _) = harness.create(id, id, "Paged").await;
        assert_eq!(status, 200, "case {id:?}: create got {status}");
    }

    let selections: [(&str, Vec<String>, Vec<String>); 4] = [
        ("one existing ARN", vec![expected_arn("a1")], vec![expected_arn("a1")]),
        (
            "two existing ARNs",
            vec![expected_arn("c3"), expected_arn("e5")],
            vec![expected_arn("c3"), expected_arn("e5")],
        ),
        (
            "an existing and an unknown ARN",
            vec![expected_arn("b2"), expected_arn("nope")],
            vec![expected_arn("b2")],
        ),
        (
            "only unknown ARNs",
            vec![expected_arn("nope"), "not-an-arn".to_string()],
            Vec::new(),
        ),
    ];
    for (case, requested, expected) in selections {
        let (status, body) = harness
            .describe(json!({ "ActionTargetArns": requested }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} with {body}"
        );
        let mut arns: Vec<String> = reported(&body).into_iter().map(|(arn, ..)| arn).collect();
        arns.sort();
        let mut expected = expected;
        expected.sort();
        assert_eq!(
            arns, expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
    }

    // Paging at every page size from 1 to one past the number of targets yields each
    // target exactly once.
    for page_size in 1..=ids.len() + 1 {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let body = match &token {
                Some(token) => json!({ "MaxResults": page_size, "NextToken": token }),
                None => json!({ "MaxResults": page_size }),
            };
            let (status, body) = harness.describe(body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} with {body}"
            );
            let page = reported(&body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page may not exceed MaxResults, got {body}"
            );
            seen.extend(page.into_iter().map(|(arn, ..)| arn));
            token = body["NextToken"].as_str().map(str::to_string);
            pages += 1;
            assert!(
                pages <= ids.len() + 2,
                "case {case:?}: paging must terminate, stopped after {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            seen.len(),
            "case {case:?}: paging must not repeat a target, got {seen:?}"
        );
        let mut expected: Vec<String> = ids.iter().map(|id| expected_arn(id)).collect();
        expected.sort();
        assert_eq!(
            sorted, expected,
            "case {case:?}: paging must report every target, got {seen:?}"
        );
    }

    // A token this operation did not issue is a client error, not an empty page.
    for bogus in ["not-a-token", "6e6f7065"] {
        let (status, body) = harness.describe(json!({ "NextToken": bogus })).await;
        assert_eq!(
            status, 400,
            "token {bogus:?}: expected 400, got {status} with {body}"
        );
    }
}

/// RULE: a hub holds at most 100 custom action targets (the published Security Hub
/// quota for custom actions per account per Region), so a create past that maximum is
/// `LimitExceededException` (429) and the store stops growing; and a `Description`
/// longer than the length the store keeps is `InvalidInputException` (400).
#[tokio::test]
async fn the_store_is_bounded_per_hub_and_per_record() {
    /// The maximum this crate keeps per hub (`state::MAX_ACTION_TARGETS_PER_HUB`).
    const MAX_PER_HUB: usize = 100;
    /// The longest stored `Description` (`state::MAX_ACTION_TARGET_DESCRIPTION_LEN`).
    const MAX_DESCRIPTION: usize = 1024;

    let harness = Harness::subscribed().await;
    for index in 0..MAX_PER_HUB {
        let id = format!("t{index}");
        let (status, body) = harness.create(&id, "Bounded", "Within the quota").await;
        assert_eq!(
            status, 200,
            "case {id:?}: create {index} of {MAX_PER_HUB} expected 200, got {status} with {body}"
        );
    }

    // One past the maximum, for two distinct ids: the refusal is about the count, not
    // about the id.
    for id in ["onetoomany", "anotherone"] {
        let (status, body) = harness.create(id, "Over", "Past the quota").await;
        assert_eq!(
            status, 429,
            "case {id:?}: expected 429 past the per-hub maximum, got {status} with {body}"
        );
    }

    let (status, body) = harness.describe(json!({ "MaxResults": 100 })).await;
    assert_eq!(status, 200, "describe got {status}");
    assert_eq!(
        reported(&body).len(),
        MAX_PER_HUB,
        "a refused create must not grow the store, got {} target(s)",
        reported(&body).len()
    );
    // Deleting one frees exactly one slot.
    let (status, _) = harness.delete(&expected_arn("t0")).await;
    assert_eq!(status, 200, "delete got {status}");
    let (status, body) = harness.create("freedslot", "Freed", "After a delete").await;
    assert_eq!(
        status, 200,
        "a create into the freed slot expected 200, got {status} with {body}"
    );

    // A description at the stored bound is accepted; one character past it is refused.
    let fresh = Harness::subscribed().await;
    for (case, length, expected_status) in [
        ("Description at the bound", MAX_DESCRIPTION, 200_u16),
        ("Description one past the bound", MAX_DESCRIPTION + 1, 400),
    ] {
        let description = "d".repeat(length);
        let (status, body) = fresh.create("descid", "Described", &description).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} with {body}"
        );
        if expected_status == 200 {
            let (_, listed) = fresh.describe(json!({})).await;
            assert_eq!(
                reported(&listed)
                    .into_iter()
                    .map(|(.., stored)| stored)
                    .collect::<Vec<_>>(),
                vec![description.clone()],
                "case {case:?}: the stored description must round-trip verbatim"
            );
            let (status, _) = fresh.delete(&expected_arn("descid")).await;
            assert_eq!(status, 200, "case {case:?}: cleanup delete got {status}");
        } else {
            let (_, listed) = fresh.describe(json!({})).await;
            assert_eq!(
                reported(&listed).len(),
                0,
                "case {case:?}: a refused create must store nothing, got {listed}"
            );
        }
    }
}

/// RULE: every custom action target operation is hub-scoped, so an account that is
/// not subscribed to Security Hub CSPM in the Region is refused (401) — before and
/// after a subscription that was disabled.
#[tokio::test]
async fn action_target_operations_require_a_subscribed_hub() {
    let harness = Harness::new();
    let arn = expected_arn("someid");

    for phase in ["before enable", "after disable"] {
        let cases: [(&str, (u16, Value)); 3] = [
            ("CreateActionTarget", harness.create("someid", "N", "D").await),
            ("DescribeActionTargets", harness.describe(json!({})).await),
            ("DeleteActionTarget", harness.delete(&arn).await),
        ];
        for (case, (status, body)) in cases {
            assert_eq!(
                status, 401,
                "case {case:?} {phase}: expected 401, got {status} with {body}"
            );
        }

        if phase == "before enable" {
            let (status, _) = harness.enable().await;
            assert_eq!(status, 200, "enable got {status}");
            let (status, _) = harness.create("someid", "N", "D").await;
            assert_eq!(status, 200, "create on the subscribed hub got {status}");
            let (status, _) = harness.call("DELETE", "/accounts", None).await;
            assert_eq!(status, 200, "disable got {status}");
        }
    }
}
