// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the Recycle Bin retention-rule lifecycle:
//! `CreateRule`, `GetRule`, `ListRules`, `UpdateRule`, `DeleteRule`.
//!
//! Every case drives the service through the real router (`MultiServiceRouter::dispatch`),
//! so the wire protocol, the route table, the boundary validator and the handlers are all
//! exercised exactly as a signed SDK request would exercise them. Each table row states
//! its own case so a failure names the input that produced it.
//!
//! The request helpers below are `pub(crate)` so the sibling suites of this one test
//! target (`tests/all/*.rs`) drive the service through the same harness.

use std::sync::Arc;

use service_com_amazonaws_rbin_amazonrecyclebin::state::{
    RULE_STATUS_AVAILABLE, RULE_STATUS_PENDING, STATUS_TRANSITION,
};
use service_com_amazonaws_rbin_amazonrecyclebin::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The emulator's canonical account id, which every ARN it emits carries.
const ACCOUNT_ID: &str = engine::EMULATOR_ACCOUNT_ID;

/// The region the test signs for; the handler reads it from the SigV4 credential scope.
pub(crate) const REGION: &str = "eu-west-2";

/// `ResourceType` accepts exactly these values.
/// Source: smithy_query(query="shape", shape="ResourceType") — enum values.
pub(crate) const RESOURCE_TYPES: [&str; 3] = ["EBS_SNAPSHOT", "EBS_VOLUME", "EC2_IMAGE"];

/// Published bounds of `RetentionPeriod$RetentionPeriodValue`.
/// Source: smithy_query(query="shape", shape="RetentionPeriod") — range {min: 1, max: 3650}.
pub(crate) const RETENTION_VALUE_MIN: i64 = 1;
pub(crate) const RETENTION_VALUE_MAX: i64 = 3650;

/// Published bounds of `UnlockDelay$UnlockDelayValue`.
/// Source: smithy_query(query="shape", shape="UnlockDelay") — range {min: 7, max: 30}.
pub(crate) const UNLOCK_DELAY_MIN: i64 = 7;

pub(crate) fn router() -> MultiServiceRouter {
    // The single shared engine the production router builds and hands to every
    // operation of this service.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    // Pin authentication off so the suite does not depend on the process-wide switch.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` header whose credential scope names `rbin` in [`REGION`] —
/// the signal both the router's `can_handle` and the ARN's region segment read.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDRBINTEST/20260101/{REGION}/rbin/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// Dispatch one request through the router and return its status and parsed JSON body
/// (`Null` for an empty body, as a 204 has).
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> (u16, serde_json::Value) {
    let payload = match &body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(value).expect("serializable body")),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://rbin.{REGION}.amazonaws.com{path}"))
        .header("authorization", authorization())
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "{method} {path} answered {status} with a non-JSON body ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

pub(crate) async fn create_rule(
    router: &MultiServiceRouter,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    call(router, "POST", "/rules", Some(body)).await
}

pub(crate) async fn get_rule(router: &MultiServiceRouter, identifier: &str) -> (u16, serde_json::Value) {
    call(router, "GET", &format!("/rules/{identifier}"), None).await
}

pub(crate) async fn list_rules(
    router: &MultiServiceRouter,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    call(router, "POST", "/list-rules", Some(body)).await
}

pub(crate) async fn update_rule(
    router: &MultiServiceRouter,
    identifier: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    call(
        router,
        "PATCH",
        &format!("/rules/{identifier}"),
        Some(body),
    )
    .await
}

pub(crate) async fn delete_rule(router: &MultiServiceRouter, identifier: &str) -> (u16, serde_json::Value) {
    call(router, "DELETE", &format!("/rules/{identifier}"), None).await
}

/// `RuleIdentifier` is `^[0-9a-zA-Z]{11}$`.
fn is_rule_identifier(candidate: &str) -> bool {
    candidate.len() == 11 && candidate.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// `NextToken` is `^[A-Za-z0-9+/=]{1,2048}$`.
fn is_next_token(candidate: &str) -> bool {
    !candidate.is_empty()
        && candidate.len() <= 2048
        && candidate
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/' || b == b'=')
}

pub(crate) fn retention(value: i64) -> serde_json::Value {
    serde_json::json!({ "RetentionPeriodValue": value, "RetentionPeriodUnit": "DAYS" })
}

pub(crate) fn lock_configuration(unlock_delay: i64) -> serde_json::Value {
    serde_json::json!({
        "UnlockDelay": { "UnlockDelayValue": unlock_delay, "UnlockDelayUnit": "DAYS" }
    })
}

/// One row of the create/read-back table: a `CreateRule` request body and the case name
/// a failure reports.
struct CreateCase {
    case: &'static str,
    body: serde_json::Value,
}

fn create_cases() -> Vec<CreateCase> {
    let mut cases = vec![
        CreateCase {
            case: "region-level rule at the minimum retention period",
            body: serde_json::json!({
                "ResourceType": "EBS_SNAPSHOT",
                "RetentionPeriod": retention(RETENTION_VALUE_MIN),
            }),
        },
        CreateCase {
            case: "region-level rule at the maximum retention period",
            body: serde_json::json!({
                "ResourceType": "EBS_VOLUME",
                "RetentionPeriod": retention(RETENTION_VALUE_MAX),
                "Description": "keep volumes for ten years",
            }),
        },
        CreateCase {
            case: "tag-level rule with two resource tags and rule tags",
            body: serde_json::json!({
                "ResourceType": "EC2_IMAGE",
                "RetentionPeriod": retention(45),
                "Description": "tag-level AMI rule",
                "Tags": [{ "Key": "env", "Value": "dev" }, { "Key": "team", "Value": "" }],
                "ResourceTags": [
                    { "ResourceTagKey": "retain", "ResourceTagValue": "yes" },
                    { "ResourceTagKey": "tier" },
                ],
            }),
        },
        CreateCase {
            case: "region-level rule with exclusion tags",
            body: serde_json::json!({
                "ResourceType": "EBS_SNAPSHOT",
                "RetentionPeriod": retention(7),
                "ExcludeResourceTags": [{ "ResourceTagKey": "scratch", "ResourceTagValue": "1" }],
            }),
        },
        CreateCase {
            case: "locked rule at the minimum unlock delay",
            body: serde_json::json!({
                "ResourceType": "EBS_VOLUME",
                "RetentionPeriod": retention(366),
                "LockConfiguration": lock_configuration(UNLOCK_DELAY_MIN),
            }),
        },
    ];
    // Every modelled resource type is covered, including one the issue never cited.
    for resource_type in RESOURCE_TYPES {
        cases.push(CreateCase {
            case: "plain rule, one per modelled resource type",
            body: serde_json::json!({
                "ResourceType": resource_type,
                "RetentionPeriod": retention(30),
            }),
        });
    }
    cases
}

/// A response's list member, with a member the response does not carry read as the
/// empty list — which `Tags` is for a rule that carries no tags.
///
/// `pub(crate)` like the request helpers above: the sibling suites of this test target
/// read a response the same way.
pub(crate) fn list_of(value: &serde_json::Value, member: &str) -> Vec<serde_json::Value> {
    value
        .get(member)
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default()
}

#[tokio::test]
async fn a_created_rule_is_minted_persisted_and_read_back_by_get_rule() {
    let router = router();
    for CreateCase { case, body } in create_cases() {
        let minted_at = std::time::Instant::now();
        let (status, created) = create_rule(&router, body.clone()).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from CreateRule, got {status} / {created:?}"
        );

        let identifier = created["Identifier"].as_str().unwrap_or_default().to_string();
        assert!(
            is_rule_identifier(&identifier),
            "case {case:?}: expected a minted ^[0-9a-zA-Z]{{11}}$ Identifier, got {:?}",
            created["Identifier"]
        );
        assert_eq!(
            created["RuleArn"].as_str(),
            Some(format!("arn:aws:rbin:{REGION}:{ACCOUNT_ID}:rule/{identifier}").as_str()),
            "case {case:?}: expected the ARN to name this rule in the signed region, got {:?}",
            created["RuleArn"]
        );
        // A freshly minted rule is not retaining resources yet, so it is reported
        // `pending`; the clock, not this call, moves it to `available`.
        assert_eq!(
            created["Status"].as_str(),
            Some(RULE_STATUS_PENDING),
            "case {case:?}: expected Status={RULE_STATUS_PENDING}, got {:?}",
            created["Status"]
        );
        // `LockState` is absent for a rule that has never been locked, and `locked` for
        // one created with a lock configuration.
        let expected_lock_state = body.get("LockConfiguration").map(|_| "locked");
        assert_eq!(
            created["LockState"].as_str(),
            expected_lock_state,
            "case {case:?}: expected LockState {expected_lock_state:?}, got {:?}",
            created["LockState"]
        );
        assert_eq!(
            created["RetentionPeriod"], body["RetentionPeriod"],
            "case {case:?}: expected the stored retention period, got {:?}",
            created["RetentionPeriod"]
        );
        for member in ["Tags", "ResourceTags", "ExcludeResourceTags"] {
            assert_eq!(
                list_of(&created, member),
                list_of(&body, member),
                "case {case:?}: expected CreateRule to report the stored {member}, got {:?}",
                created[member]
            );
        }

        // ROUND TRIP: a later request reads the same rule back out of the store.
        let (status, fetched) = get_rule(&router, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetRule, got {status} / {fetched:?}"
        );
        // `Status` is the one member the CLOCK moves, so it is read against the
        // transition span rather than against the create response.
        let status = fetched["Status"].as_str();
        let expected_status = if minted_at.elapsed() < STATUS_TRANSITION {
            RULE_STATUS_PENDING
        } else {
            RULE_STATUS_AVAILABLE
        };
        assert_eq!(
            status,
            Some(expected_status),
            "case {case:?}: expected GetRule to report Status={expected_status}, \
             got {status:?}"
        );
        for member in [
            "Identifier",
            "RuleArn",
            "ResourceType",
            "RetentionPeriod",
            "Description",
            "LockState",
            "LockConfiguration",
        ] {
            assert_eq!(
                fetched.get(member),
                created.get(member),
                "case {case:?}: GetRule must report the persisted {member} CreateRule returned, \
                 got {:?} vs {:?}",
                fetched.get(member),
                created.get(member)
            );
        }
        for member in ["ResourceTags", "ExcludeResourceTags"] {
            assert_eq!(
                list_of(&fetched, member),
                list_of(&body, member),
                "case {case:?}: GetRule must report the persisted {member}, got {:?}",
                fetched[member]
            );
        }
    }
}

#[tokio::test]
async fn every_created_rule_gets_its_own_identifier_and_arn() {
    let router = router();
    let mut identifiers = std::collections::BTreeSet::new();
    let mut arns = std::collections::BTreeSet::new();
    for index in 0..25 {
        let case = format!("create #{index}");
        let (status, created) = create_rule(
            &router,
            serde_json::json!({
                "ResourceType": RESOURCE_TYPES[index % RESOURCE_TYPES.len()],
                "RetentionPeriod": retention(1 + (index as i64)),
            }),
        )
        .await;
        assert_eq!(status, 201, "case {case:?}: expected HTTP 201, got {status}");
        let identifier = created["Identifier"].as_str().unwrap_or_default().to_string();
        assert!(
            is_rule_identifier(&identifier),
            "case {case:?}: expected a well-formed Identifier, got {identifier:?}"
        );
        assert!(
            identifiers.insert(identifier.clone()),
            "case {case:?}: Identifier {identifier:?} was minted twice"
        );
        let arn = created["RuleArn"].as_str().unwrap_or_default().to_string();
        assert!(
            arns.insert(arn.clone()),
            "case {case:?}: RuleArn {arn:?} was minted twice"
        );
    }
}

#[tokio::test]
async fn retention_period_is_refused_outside_its_published_range() {
    let router = router();
    // Each published bound is accepted, and the value just past it is refused.
    let rows: [(&str, i64, u16); 4] = [
        ("minimum bound", RETENTION_VALUE_MIN, 201),
        ("maximum bound", RETENTION_VALUE_MAX, 201),
        ("one below the minimum", RETENTION_VALUE_MIN - 1, 400),
        ("one above the maximum", RETENTION_VALUE_MAX + 1, 400),
    ];
    for (case, value, expected) in rows {
        let (status, body) = create_rule(
            &router,
            serde_json::json!({
                "ResourceType": "EBS_SNAPSHOT",
                "RetentionPeriod": retention(value),
            }),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?} (RetentionPeriodValue={value}): expected HTTP {expected}, \
             got {status} / {body:?}"
        );
    }
}

#[tokio::test]
async fn exclusion_tags_cannot_be_combined_with_resource_tags() {
    let router = router();
    let (status, body) = create_rule(
        &router,
        serde_json::json!({
            "ResourceType": "EBS_SNAPSHOT",
            "RetentionPeriod": retention(7),
            "ResourceTags": [{ "ResourceTagKey": "retain", "ResourceTagValue": "yes" }],
            "ExcludeResourceTags": [{ "ResourceTagKey": "scratch" }],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"tag-level rule with exclusion tags\": expected HTTP 400, got {status} / {body:?}"
    );
    assert_eq!(
        body["Reason"].as_str(),
        Some("INVALID_PARAMETER_VALUE"),
        "case \"tag-level rule with exclusion tags\": expected reason \
         INVALID_PARAMETER_VALUE, got {:?}",
        body["Reason"]
    );
}

#[tokio::test]
async fn list_rules_reports_only_the_rules_of_the_requested_type() {
    let router = router();
    // Two rules per modelled resource type, so filtering cannot pass by accident.
    let mut expected: std::collections::BTreeMap<&str, std::collections::BTreeSet<String>> =
        std::collections::BTreeMap::new();
    for resource_type in RESOURCE_TYPES {
        for nth in 0..2 {
            let (status, created) = create_rule(
                &router,
                serde_json::json!({
                    "ResourceType": resource_type,
                    "RetentionPeriod": retention(10 + nth),
                }),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {resource_type:?} #{nth}: expected HTTP 201, got {status}"
            );
            expected
                .entry(resource_type)
                .or_default()
                .insert(created["Identifier"].as_str().unwrap_or_default().to_string());
        }
    }

    for resource_type in RESOURCE_TYPES {
        let (status, listed) =
            list_rules(&router, serde_json::json!({ "ResourceType": resource_type })).await;
        assert_eq!(
            status, 200,
            "case {resource_type:?}: expected HTTP 200 from ListRules, got {status} / {listed:?}"
        );
        let reported: std::collections::BTreeSet<String> = list_of(&listed, "Rules")
            .iter()
            .map(|summary| summary["Identifier"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            reported, expected[resource_type],
            "case {resource_type:?}: expected exactly the rules of that type, got {reported:?}"
        );
        assert!(
            listed.get("NextToken").is_none(),
            "case {resource_type:?}: a complete page must omit NextToken, got {:?}",
            listed["NextToken"]
        );
        // A `RuleSummary` carries exactly the members the model gives it.
        for summary in list_of(&listed, "Rules") {
            let identifier = summary["Identifier"].as_str().unwrap_or_default();
            assert_eq!(
                summary["RuleArn"].as_str(),
                Some(format!("arn:aws:rbin:{REGION}:{ACCOUNT_ID}:rule/{identifier}").as_str()),
                "case {resource_type:?}: summary must carry the persisted ARN, got {:?}",
                summary["RuleArn"]
            );
            assert!(
                summary["RetentionPeriod"]["RetentionPeriodValue"].is_i64(),
                "case {resource_type:?}: summary must carry the persisted retention period, \
                 got {:?}",
                summary["RetentionPeriod"]
            );
        }
    }

    // Filtering on a type with no rules is an empty page, not an error.
    let (status, listed) = list_rules(
        &router,
        serde_json::json!({ "ResourceType": "EC2_IMAGE", "LockState": "locked" }),
    )
    .await;
    assert_eq!(status, 200, "case \"no matching rules\": expected HTTP 200, got {status}");
    assert!(
        list_of(&listed, "Rules").is_empty(),
        "case \"no matching rules\": expected an empty Rules list, got {:?}",
        listed["Rules"]
    );
}

#[tokio::test]
async fn list_rules_pages_through_every_rule_for_each_page_size() {
    const TOTAL: usize = 7;
    let router = router();
    let mut all = std::collections::BTreeSet::new();
    for nth in 0..TOTAL {
        let (status, created) = create_rule(
            &router,
            serde_json::json!({
                "ResourceType": "EBS_SNAPSHOT",
                "RetentionPeriod": retention(1 + nth as i64),
            }),
        )
        .await;
        assert_eq!(status, 201, "setup #{nth}: expected HTTP 201, got {status}");
        all.insert(created["Identifier"].as_str().unwrap_or_default().to_string());
    }

    // Every page size from one rule per page to more than the whole collection.
    for page_size in [1usize, 2, 3, 6, TOTAL, TOTAL + 1] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = serde_json::json!({
                "ResourceType": "EBS_SNAPSHOT",
                "MaxResults": page_size,
            });
            if let Some(token) = &next_token {
                request["NextToken"] = serde_json::Value::String(token.clone());
            }
            let (status, listed) = list_rules(&router, request).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from ListRules, got {status} / {listed:?}"
            );
            let page = list_of(&listed, "Rules");
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page must hold at most MaxResults rules, got {}",
                page.len()
            );
            for summary in &page {
                seen.push(summary["Identifier"].as_str().unwrap_or_default().to_string());
            }
            pages += 1;
            assert!(
                pages <= TOTAL + 2,
                "case {case:?}: paging did not terminate after {pages} pages"
            );
            match listed.get("NextToken").and_then(serde_json::Value::as_str) {
                Some(token) => {
                    assert!(
                        is_next_token(token),
                        "case {case:?}: expected an opaque ^[A-Za-z0-9+/=]{{1,2048}}$ \
                         NextToken, got {token:?}"
                    );
                    assert!(
                        !page.is_empty(),
                        "case {case:?}: a page that hands back a NextToken must not be empty"
                    );
                    next_token = Some(token.to_string());
                }
                None => break,
            }
        }
        let unique: std::collections::BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            seen.len(),
            unique.len(),
            "case {case:?}: paging reported a rule twice: {seen:?}"
        );
        assert_eq!(
            unique, all,
            "case {case:?}: paging must report every rule exactly once, got {unique:?}"
        );
    }
}

#[tokio::test]
async fn list_rules_refuses_a_next_token_it_never_minted() {
    let router = router();
    // Not base64, base64 of something that is not an identifier, and base64 of an
    // identifier-shaped value of the wrong length.
    let rows = [
        ("not base64", "****"),
        ("base64 of a non-identifier", "bm90LWFuLWlk"),
        ("base64 of a short identifier", "YWFh"),
    ];
    for (case, token) in rows {
        let (status, body) = list_rules(
            &router,
            serde_json::json!({ "ResourceType": "EBS_SNAPSHOT", "NextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?} (NextToken={token:?}): expected HTTP 400, got {status} / {body:?}"
        );
    }
}

#[tokio::test]
async fn update_rule_applies_only_the_supplied_members_and_persists_them() {
    let router = router();
    let original = serde_json::json!({
        "ResourceType": "EBS_SNAPSHOT",
        "RetentionPeriod": retention(7),
        "Description": "bootstrap rule",
        "ResourceTags": [{ "ResourceTagKey": "retain", "ResourceTagValue": "yes" }],
    });

    // Each row patches a different subset; every member the patch omits must survive.
    let rows: Vec<(&str, serde_json::Value)> = vec![
        (
            "retention period only",
            serde_json::json!({ "RetentionPeriod": retention(30) }),
        ),
        (
            "description only",
            serde_json::json!({ "Description": "renamed" }),
        ),
        (
            "resource tags only",
            serde_json::json!({
                "ResourceTags": [{ "ResourceTagKey": "retain", "ResourceTagValue": "always" }]
            }),
        ),
        (
            "retention period and description together",
            serde_json::json!({ "RetentionPeriod": retention(3650), "Description": "both" }),
        ),
        ("nothing at all", serde_json::json!({})),
        (
            "an unsupported resource type is ignored",
            serde_json::json!({ "ResourceType": "EC2_IMAGE" }),
        ),
    ];

    for (case, patch) in rows {
        let (status, created) = create_rule(&router, original.clone()).await;
        assert_eq!(status, 201, "case {case:?}: setup CreateRule returned {status}");
        let identifier = created["Identifier"].as_str().unwrap_or_default().to_string();

        let (status, updated) = update_rule(&router, &identifier, patch.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from UpdateRule, got {status} / {updated:?}"
        );

        // The expected rule: the created one with the patch's members applied, and the
        // resource type left as created whatever the patch said.
        let expected_retention = patch
            .get("RetentionPeriod")
            .cloned()
            .unwrap_or_else(|| created["RetentionPeriod"].clone());
        let expected_description = patch
            .get("Description")
            .cloned()
            .unwrap_or_else(|| created["Description"].clone());
        let expected_resource_tags = patch
            .get("ResourceTags")
            .map(|tags| list_of(&serde_json::json!({ "t": tags }), "t"))
            .unwrap_or_else(|| list_of(&created, "ResourceTags"));

        let label = "UpdateRule";
        let actual = &updated;
        assert_eq!(
            actual["RetentionPeriod"], expected_retention,
            "case {case:?}: {label} expected RetentionPeriod {expected_retention:?}, got {:?}",
            actual["RetentionPeriod"]
        );
        assert_eq!(
            actual["Description"], expected_description,
            "case {case:?}: {label} expected Description {expected_description:?}, got {:?}",
            actual["Description"]
        );
        assert_eq!(
            list_of(actual, "ResourceTags"),
            expected_resource_tags,
            "case {case:?}: {label} expected ResourceTags {expected_resource_tags:?}, got {:?}",
            actual["ResourceTags"]
        );
        assert_eq!(
            actual["ResourceType"], created["ResourceType"],
            "case {case:?}: {label} must not change the rule's resource type, got {:?}",
            actual["ResourceType"]
        );

        // ROUND TRIP: the update is persisted, not just echoed.
        let (status, fetched) = get_rule(&router, &identifier).await;
        assert_eq!(status, 200, "case {case:?}: GetRule after update returned {status}");
        assert_eq!(
            fetched["RetentionPeriod"], expected_retention,
            "case {case:?}: GetRule must observe the updated RetentionPeriod, got {:?}",
            fetched["RetentionPeriod"]
        );
        assert_eq!(
            fetched["Description"], expected_description,
            "case {case:?}: GetRule must observe the updated Description, got {:?}",
            fetched["Description"]
        );
        assert_eq!(
            list_of(&fetched, "ResourceTags"),
            expected_resource_tags,
            "case {case:?}: GetRule must observe the updated ResourceTags, got {:?}",
            fetched["ResourceTags"]
        );

        let (status, _) = delete_rule(&router, &identifier).await;
        assert_eq!(status, 204, "case {case:?}: teardown DeleteRule returned {status}");
    }
}

#[tokio::test]
async fn a_deleted_rule_disappears_from_get_rule_and_list_rules() {
    let router = router();
    let mut identifiers = Vec::new();
    for nth in 0..3 {
        let (status, created) = create_rule(
            &router,
            serde_json::json!({
                "ResourceType": "EBS_VOLUME",
                "RetentionPeriod": retention(1 + nth),
            }),
        )
        .await;
        assert_eq!(status, 201, "setup #{nth}: expected HTTP 201, got {status}");
        identifiers.push(created["Identifier"].as_str().unwrap_or_default().to_string());
    }

    for (nth, identifier) in identifiers.iter().enumerate() {
        let case = format!("delete #{nth} ({identifier})");
        let (status, body) = delete_rule(&router, identifier).await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected HTTP 204 from DeleteRule, got {status} / {body:?}"
        );

        let (status, body) = get_rule(&router, identifier).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 from GetRule after delete, got {status} / {body:?}"
        );

        let (status, listed) =
            list_rules(&router, serde_json::json!({ "ResourceType": "EBS_VOLUME" })).await;
        assert_eq!(status, 200, "case {case:?}: ListRules returned {status}");
        let reported: Vec<String> = list_of(&listed, "Rules")
            .iter()
            .map(|summary| summary["Identifier"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            reported,
            identifiers[nth + 1..].to_vec(),
            "case {case:?}: ListRules must report exactly the rules not yet deleted, \
             got {reported:?}"
        );

        // Deleting it again is a 404: the rule is gone from the store.
        let (status, body) = delete_rule(&router, identifier).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 re-deleting a deleted rule, got {status} / {body:?}"
        );
    }
}

#[tokio::test]
async fn an_unknown_identifier_is_resource_not_found_on_every_rule_addressed_operation() {
    let router = router();
    // Create one rule, so the store is not simply empty.
    let (status, _) = create_rule(
        &router,
        serde_json::json!({ "ResourceType": "EBS_SNAPSHOT", "RetentionPeriod": retention(7) }),
    )
    .await;
    assert_eq!(status, 201, "setup: expected HTTP 201, got {status}");

    // Identifier-shaped values no rule was ever minted with.
    for identifier in ["aaaaaaaaaaa", "ZZZZZZZZZZZ", "0a1B2c3D4e5"] {
        for (operation, response) in [
            ("GetRule", get_rule(&router, identifier).await),
            (
                "UpdateRule",
                update_rule(&router, identifier, serde_json::json!({ "Description": "x" })).await,
            ),
            ("DeleteRule", delete_rule(&router, identifier).await),
        ] {
            let (status, body) = response;
            let case = format!("{operation} on {identifier}");
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status} / {body:?}"
            );
            // Recycle Bin composes the 404 from the identifier the request addressed
            // and populates no `Reason` on this path; `tests/all/error_response_bodies.rs`
            // asserts that body over the whole class.
            let expected = format!("Recycle Bin Rule '{identifier}' does not exist.");
            assert_eq!(
                body["Message"].as_str(),
                Some(expected.as_str()),
                "case {case:?}: expected Message {expected:?}, got {:?}",
                body["Message"]
            );
        }
    }
}

/// Published quota of retention rules per Region, the bound the store is held to.
/// Source: `service_com_amazonaws_rbin_amazonrecyclebin::state::MAX_RULES`.
const MAX_RULES: usize = service_com_amazonaws_rbin_amazonrecyclebin::state::MAX_RULES;

#[tokio::test]
async fn create_rule_is_refused_once_the_region_holds_its_quota_of_rules() {
    let router = router();
    // Fill the Region to its quota; every one of these must be accepted.
    for nth in 0..MAX_RULES {
        let (status, body) = create_rule(
            &router,
            serde_json::json!({
                "ResourceType": RESOURCE_TYPES[nth % RESOURCE_TYPES.len()],
                "RetentionPeriod": retention(1 + (nth as i64 % RETENTION_VALUE_MAX)),
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case \"rule {nth} of the quota\": expected HTTP 201, got {status} / {body:?}"
        );
    }

    // One past the quota is refused with the modelled ServiceQuotaExceededException.
    let over_quota = serde_json::json!({
        "ResourceType": "EBS_SNAPSHOT",
        "RetentionPeriod": retention(7),
    });
    let (status, body) = create_rule(&router, over_quota.clone()).await;
    assert_eq!(
        status, 402,
        "case \"one rule past the quota\": expected HTTP 402, got {status} / {body:?}"
    );
    assert_eq!(
        body["Reason"].as_str(),
        Some("SERVICE_QUOTA_EXCEEDED"),
        "case \"one rule past the quota\": expected reason SERVICE_QUOTA_EXCEEDED, got {:?}",
        body["Reason"]
    );

    // The bound is a live count, not a latch: freeing one slot admits one rule again,
    // and the Region is back at its quota afterwards.
    let (status, listed) = list_rules(
        &router,
        serde_json::json!({ "ResourceType": "EBS_SNAPSHOT", "MaxResults": 1 }),
    )
    .await;
    assert_eq!(status, 200, "case \"list one rule to free a slot\": got {status}");
    let victim = list_of(&listed, "Rules")
        .first()
        .and_then(|summary| summary["Identifier"].as_str())
        .unwrap_or_default()
        .to_string();
    assert!(
        is_rule_identifier(&victim),
        "case \"list one rule to free a slot\": expected a stored rule, got {victim:?}"
    );
    let (status, _) = delete_rule(&router, &victim).await;
    assert_eq!(status, 204, "case \"free a slot\": expected HTTP 204, got {status}");

    let (status, body) = create_rule(&router, over_quota.clone()).await;
    assert_eq!(
        status, 201,
        "case \"create into the freed slot\": expected HTTP 201, got {status} / {body:?}"
    );
    let (status, body) = create_rule(&router, over_quota).await;
    assert_eq!(
        status, 402,
        "case \"past the quota again\": expected HTTP 402, got {status} / {body:?}"
    );
}

#[tokio::test]
async fn a_locked_rule_cannot_be_modified_or_deleted() {
    let router = router();
    let (status, created) = create_rule(
        &router,
        serde_json::json!({
            "ResourceType": "EBS_SNAPSHOT",
            "RetentionPeriod": retention(7),
            "LockConfiguration": lock_configuration(UNLOCK_DELAY_MIN),
        }),
    )
    .await;
    assert_eq!(status, 201, "setup: expected HTTP 201, got {status}");
    let identifier = created["Identifier"].as_str().unwrap_or_default().to_string();

    for (case, response) in [
        (
            "UpdateRule on a locked rule",
            update_rule(
                &router,
                &identifier,
                serde_json::json!({ "RetentionPeriod": retention(30) }),
            )
            .await,
        ),
        (
            "DeleteRule on a locked rule",
            delete_rule(&router, &identifier).await,
        ),
    ] {
        let (status, body) = response;
        assert_eq!(
            status, 409,
            "case {case:?}: expected HTTP 409 ConflictException, got {status} / {body:?}"
        );
        assert_eq!(
            body["Reason"].as_str(),
            Some("INVALID_RULE_STATE"),
            "case {case:?}: expected reason INVALID_RULE_STATE, got {:?}",
            body["Reason"]
        );
    }

    // The rule is untouched: still locked, still at its original retention period.
    let (status, fetched) = get_rule(&router, &identifier).await;
    assert_eq!(status, 200, "case \"GetRule after refused writes\": got {status}");
    assert_eq!(
        fetched["RetentionPeriod"], created["RetentionPeriod"],
        "case \"GetRule after refused writes\": the refused update must not have applied, got {:?}",
        fetched["RetentionPeriod"]
    );
    assert_eq!(
        fetched["LockState"].as_str(),
        Some("locked"),
        "case \"GetRule after refused writes\": expected LockState=locked, got {:?}",
        fetched["LockState"]
    );
}
