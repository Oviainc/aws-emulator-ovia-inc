// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Firewall Manager policy lifecycle and the
//! administrator association that gates it.
//!
//! Every test drives the service the way a client does: an awsJson1_1 `POST /`
//! through [`MultiServiceRouter::dispatch`], with the `X-Amz-Target` header and a
//! SigV4 credential scope, so the whole deserialize → handle → serialize walk
//! runs and the assertions are about the WIRE response, not about a Rust struct.
//!
//! The policy table below is the class the rules are asserted over: the rows
//! differ in security-service type, in resource-type form (single `ResourceType`
//! vs a `ResourceTypeList`), in which optional members are present, in tag count
//! and in policy status. Each rule is asserted on every row.
//!
//! The request harness here ([`emulator`], [`call`], [`call_in`], [`error_code`]
//! and the [`REGION`] / [`ACCOUNT`] constants) is `pub(crate)` so the sibling
//! suites in this test target drive the service through the SAME wire walk
//! instead of growing a second copy of it.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region the test requests are signed for. Deliberately not the emulator's
/// `us-east-1` default, so an ARN built from the default would not pass.
pub(crate) const REGION: &str = "us-west-2";

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
pub(crate) const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// A fresh emulator: one shared engine, this service mounted on a router.
///
/// `pub(crate)` so a sibling suite in this test target drives the service through
/// the same wire harness rather than building a second copy of it. A suite that
/// also needs to reach the store uses [`emulator_with_engine`].
pub(crate) fn emulator() -> MultiServiceRouter {
    emulator_with_engine().0
}

/// [`emulator`], with the shared engine handed back too, for a suite that seeds
/// the store the way a handler reaches it (`state::service_state`).
pub(crate) fn emulator_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// A SigV4 credential scope naming this service and Region — the signal the
/// router routes on and the handler reads the ARN Region from.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/fms/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=deadbeef"
    )
}

/// Call one operation and return `(http status, parsed body)`.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    call_in(router, operation, body, REGION).await
}

/// [`call`], signed for an explicit Region.
pub(crate) async fn call_in(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    region: &str,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSFMS_20180101.{operation}"))
        .header("authorization", authorization(region))
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|e| {
            panic!(
                "{operation} answered {status} with a non-JSON body ({e}): {:?}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, parsed)
}

/// The modelled error code an awsJson1_1 error envelope carries in `__type`.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// One row of the policy table: a name for the assertion messages, and the
/// `Policy` document a client sends.
struct Case {
    name: &'static str,
    policy: Value,
    tags: Value,
}

/// The policy class the rules are asserted over.
///
/// `SecurityServicePolicyData.Type` values are taken from the model's own
/// catalog, `ops::types::SECURITY_SERVICE_TYPE_VALUES`
/// (`com.amazonaws.fms#SecurityServiceType`), asserted below to be members of
/// it — including `NETWORK_FIREWALL` and `DNS_FIREWALL`, which the issue did not
/// cite. `PolicyStatus` values come from `CUSTOMER_POLICY_STATUS_VALUES`.
fn cases() -> Vec<Case> {
    vec![
        Case {
            // The minimum a policy can be: every required member and nothing else.
            name: "minimal-wafv2",
            policy: json!({
                "PolicyName": "demo",
                "SecurityServicePolicyData": {
                    "Type": "WAFV2",
                    "ManagedServiceData": "{\"type\":\"WAFV2\"}"
                },
                "ResourceType": "AWS::ElasticLoadBalancingV2::LoadBalancer",
                "ExcludeResourceTags": false,
                "RemediationEnabled": false
            }),
            tags: json!([]),
        },
        Case {
            // Multiple resource types, resource tags, a description, and the
            // booleans flipped the other way.
            name: "shield-with-resource-type-list",
            policy: json!({
                "PolicyName": "shield.policy-2",
                "SecurityServicePolicyData": { "Type": "SHIELD_ADVANCED" },
                "ResourceType": "ResourceTypeList",
                "ResourceTypeList": [
                    "AWS::ElasticLoadBalancingV2::LoadBalancer",
                    "AWS::EC2::EIP",
                    "AWS::CloudFront::Distribution"
                ],
                "ResourceTags": [
                    { "Key": "env", "Value": "prod" },
                    { "Key": "unused-value" }
                ],
                "ResourceTagLogicalOperator": "OR",
                "ExcludeResourceTags": true,
                "RemediationEnabled": true,
                "DeleteUnusedFMManagedResources": true,
                "PolicyDescription": "shield advanced across three resource types"
            }),
            tags: json!([{ "Key": "owner", "Value": "team-a" }]),
        },
        Case {
            // Organisation scoping by IncludeMap, and three tags.
            name: "security-groups-with-include-map",
            policy: json!({
                "PolicyName": "sg-baseline",
                "SecurityServicePolicyData": {
                    "Type": "SECURITY_GROUPS_COMMON",
                    "ManagedServiceData": "{\"type\":\"SECURITY_GROUPS_COMMON\"}"
                },
                "ResourceType": "AWS::EC2::SecurityGroup",
                "IncludeMap": { "ACCOUNT": ["111111111111", "222222222222"] },
                "ExcludeResourceTags": false,
                "RemediationEnabled": true
            }),
            tags: json!([
                { "Key": "owner", "Value": "team-b" },
                { "Key": "cost-center", "Value": "4711" },
                { "Key": "empty-value", "Value": "" }
            ]),
        },
        Case {
            // A nested PolicyOption, an ExcludeMap, and an explicitly
            // non-default PolicyStatus. No `ResourceSetIds`: those must name
            // resource sets that EXIST, which needs an id this static table
            // cannot know — the `policy_document_validation` suite creates the
            // sets and asserts both sides of that rule.
            name: "network-firewall-out-of-admin-scope",
            policy: json!({
                "PolicyName": "nfw-vpcs",
                "SecurityServicePolicyData": {
                    "Type": "NETWORK_FIREWALL",
                    "PolicyOption": {
                        "NetworkFirewallPolicy": { "FirewallDeploymentModel": "DISTRIBUTED" }
                    }
                },
                "ResourceType": "AWS::EC2::VPC",
                "ExcludeMap": { "ORG_UNIT": ["ou-1234-abcdefgh"] },
                "ExcludeResourceTags": false,
                "RemediationEnabled": false,
                "PolicyStatus": "OUT_OF_ADMIN_SCOPE"
            }),
            tags: json!([]),
        },
        Case {
            name: "dns-firewall",
            policy: json!({
                "PolicyName": "dns-fw",
                "SecurityServicePolicyData": {
                    "Type": "DNS_FIREWALL",
                    "ManagedServiceData": "{\"type\":\"DNS_FIREWALL\"}"
                },
                "ResourceType": "AWS::EC2::VPC",
                "ExcludeResourceTags": true,
                "RemediationEnabled": false
            }),
            tags: json!([{ "Key": "stage", "Value": "test" }]),
        },
    ]
}

/// Create one case's policy and return the `PutPolicy` response body.
async fn put_new(router: &MultiServiceRouter, case: &Case) -> Value {
    let (status, body) = call(
        router,
        "PutPolicy",
        &json!({ "Policy": case.policy, "TagList": case.tags }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected PutPolicy to answer 200, got {status} with {body}",
        case.name
    );
    body
}

/// `PutPolicy` mints a 36-char `PolicyId`, a `PolicyUpdateToken` and the policy
/// ARN, and `GetPolicy` reads back exactly what was stored.
#[tokio::test]
async fn put_policy_mints_identifiers_and_get_policy_reads_the_stored_policy_back() {
    let router = emulator();
    let catalog = service_com_amazonaws_fms_awsfms_20180101::ops::types::SECURITY_SERVICE_TYPE_VALUES;

    for case in cases() {
        let put = put_new(&router, &case).await;
        let stored = &put["Policy"];
        let policy_id = stored["PolicyId"].as_str().unwrap_or_default().to_string();
        let arn = put["PolicyArn"].as_str().unwrap_or_default().to_string();

        // The row's security-service type is one the model declares.
        let declared_type = case.policy["SecurityServicePolicyData"]["Type"]
            .as_str()
            .unwrap_or_default();
        assert!(
            catalog.contains(&declared_type),
            "case {:?}: {declared_type:?} is not in the model's SecurityServiceType catalog {catalog:?}",
            case.name
        );

        // PolicyId: exactly 36 characters from [a-z0-9A-Z-], per the model's
        // `com.amazonaws.fms#PolicyId` pattern `^[a-z0-9A-Z-]{36}$`.
        assert_eq!(
            policy_id.chars().count(),
            36,
            "case {:?}: expected a 36-character PolicyId, got {policy_id:?}",
            case.name
        );
        assert!(
            policy_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "case {:?}: PolicyId must match ^[a-z0-9A-Z-]{{36}}$, got {policy_id:?}",
            case.name
        );
        assert!(
            !stored["PolicyUpdateToken"]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "case {:?}: expected a minted PolicyUpdateToken, got {stored}",
            case.name
        );
        assert_eq!(
            arn,
            format!("arn:aws:fms:{REGION}:{ACCOUNT}:policy/{policy_id}"),
            "case {:?}: unexpected PolicyArn, got {arn:?}",
            case.name
        );

        // Everything the caller sent survives the round trip, and the service
        // defaults PolicyStatus to ACTIVE only when the caller sent none.
        let (status, got) = call(&router, "GetPolicy", &json!({ "PolicyId": policy_id })).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected GetPolicy to answer 200, got {status} with {got}",
            case.name
        );
        assert_eq!(
            got["Policy"], *stored,
            "case {:?}: GetPolicy must report the policy PutPolicy stored, got {}",
            case.name, got["Policy"]
        );
        assert_eq!(
            got["PolicyArn"].as_str(),
            Some(arn.as_str()),
            "case {:?}: GetPolicy must report the ARN PutPolicy minted, got {}",
            case.name,
            got["PolicyArn"]
        );
        for (member, sent) in case
            .policy
            .as_object()
            .expect("the case policy is an object")
        {
            assert_eq!(
                stored.get(member),
                Some(sent),
                "case {:?}: member {member} was not stored as sent, got {:?}",
                case.name,
                stored.get(member)
            );
        }
        let expected_status = case.policy.get("PolicyStatus").cloned().unwrap_or_else(|| {
            // `CustomerPolicyStatus` ACTIVE: the administrator can manage and
            // delete the policy.
            json!("ACTIVE")
        });
        assert_eq!(
            stored["PolicyStatus"], expected_status,
            "case {:?}: expected PolicyStatus {expected_status}, got {}",
            case.name, stored["PolicyStatus"]
        );
    }
}

/// The ARN's Region segment is the Region the request was signed for, for every
/// row — not a constant baked into the handler.
#[tokio::test]
async fn the_policy_arn_names_the_region_the_request_was_signed_for() {
    // Regions chosen to differ in partition-independent shape (prefix, digits).
    for region in ["us-east-1", "eu-central-1", "ap-southeast-4"] {
        let router = emulator();
        for case in cases() {
            let (status, body) = call_in(
                &router,
                "PutPolicy",
                &json!({ "Policy": case.policy, "TagList": case.tags }),
                region,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?} in {region}: expected 200, got {status} with {body}",
                case.name
            );
            let policy_id = body["Policy"]["PolicyId"].as_str().unwrap_or_default();
            assert_eq!(
                body["PolicyArn"].as_str(),
                Some(format!("arn:aws:fms:{region}:{ACCOUNT}:policy/{policy_id}").as_str()),
                "case {:?} in {region}: unexpected PolicyArn, got {}",
                case.name,
                body["PolicyArn"]
            );
        }
    }
}

/// An update must carry the CURRENT `PolicyUpdateToken`; a successful one
/// rotates it, so the token that just worked no longer does.
#[tokio::test]
async fn an_update_requires_the_current_token_and_rotates_it() {
    let router = emulator();

    for case in cases() {
        let put = put_new(&router, &case).await;
        let policy_id = put["Policy"]["PolicyId"]
            .as_str()
            .expect("a minted PolicyId")
            .to_string();
        let first_token = put["Policy"]["PolicyUpdateToken"]
            .as_str()
            .expect("a minted PolicyUpdateToken")
            .to_string();

        // No token at all is as stale as a wrong one.
        let mut without_token = case.policy.clone();
        without_token["PolicyId"] = json!(policy_id);
        let (status, body) = call(&router, "PutPolicy", &json!({ "Policy": without_token })).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidOperationException"),
            "case {:?}: an update with no PolicyUpdateToken must be refused, got {status} {body}",
            case.name
        );

        // A well-formed token this policy never had.
        let mut stale = case.policy.clone();
        stale["PolicyId"] = json!(policy_id);
        stale["PolicyUpdateToken"] = json!("not-the-current-token");
        let (status, body) = call(&router, "PutPolicy", &json!({ "Policy": stale })).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidOperationException"),
            "case {:?}: an update with a stale PolicyUpdateToken must be refused, got {status} {body}",
            case.name
        );

        // The current token succeeds and changes the policy.
        let mut update = case.policy.clone();
        update["PolicyId"] = json!(policy_id);
        update["PolicyUpdateToken"] = json!(first_token);
        update["PolicyDescription"] = json!("updated");
        let (status, updated) = call(&router, "PutPolicy", &json!({ "Policy": update.clone() })).await;
        assert_eq!(
            status, 200,
            "case {:?}: an update with the current token must succeed, got {status} with {updated}",
            case.name
        );
        let second_token = updated["Policy"]["PolicyUpdateToken"]
            .as_str()
            .expect("a rotated PolicyUpdateToken")
            .to_string();
        assert_ne!(
            second_token, first_token,
            "case {:?}: a successful update must rotate PolicyUpdateToken, got {second_token:?}",
            case.name
        );
        assert_eq!(
            updated["Policy"]["PolicyId"].as_str(),
            Some(policy_id.as_str()),
            "case {:?}: an update must keep the PolicyId, got {}",
            case.name,
            updated["Policy"]["PolicyId"]
        );
        assert_eq!(
            updated["PolicyArn"], put["PolicyArn"],
            "case {:?}: an update must keep the policy ARN, got {}",
            case.name, updated["PolicyArn"]
        );

        // The rotation is visible to a reader, and the old token is now stale.
        let (_, got) = call(&router, "GetPolicy", &json!({ "PolicyId": policy_id })).await;
        assert_eq!(
            got["Policy"]["PolicyUpdateToken"].as_str(),
            Some(second_token.as_str()),
            "case {:?}: GetPolicy must report the rotated token, got {}",
            case.name,
            got["Policy"]["PolicyUpdateToken"]
        );
        assert_eq!(
            got["Policy"]["PolicyDescription"].as_str(),
            Some("updated"),
            "case {:?}: the update must be persisted, got {}",
            case.name,
            got["Policy"]["PolicyDescription"]
        );
        let (status, body) = call(&router, "PutPolicy", &json!({ "Policy": update })).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidOperationException"),
            "case {:?}: replaying the rotated-away token must be refused, got {status} {body}",
            case.name
        );
    }
}

/// `ListPolicies` projects each stored policy into a `PolicySummary`, and
/// `MaxResults` bounds the page: a `NextToken` appears only while policies
/// remain, and round-trips to the next page.
#[tokio::test]
async fn list_policies_pages_the_stored_policies_and_projects_each_summary() {
    let router = emulator();
    let cases = cases();

    // Store the whole table, keeping what each row should project to.
    let mut expected: Vec<(String, Value, Value)> = Vec::new();
    for case in &cases {
        let put = put_new(&router, case).await;
        let policy = put["Policy"].clone();
        expected.push((
            policy["PolicyId"].as_str().expect("a PolicyId").to_string(),
            put["PolicyArn"].clone(),
            policy,
        ));
    }
    // The store is ordered by PolicyId, which is the order pages follow.
    expected.sort_by(|a, b| a.0.cmp(&b.0));

    // Page sizes spanning the model's MaxResults range (1..100): both published
    // bounds, one in between, and the absent case.
    for page_size in [Some(1), Some(2), Some(3), Some(100), None] {
        let mut seen: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({});
            if let Some(max) = page_size {
                request["MaxResults"] = json!(max);
            }
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let (status, body) = call(&router, "ListPolicies", &request).await;
            assert_eq!(
                status, 200,
                "case MaxResults={page_size:?}: expected 200, got {status} with {body}"
            );
            let page = body["PolicyList"]
                .as_array()
                .unwrap_or_else(|| panic!("case MaxResults={page_size:?}: no PolicyList in {body}"))
                .clone();
            if let Some(max) = page_size {
                assert!(
                    page.len() <= max as usize,
                    "case MaxResults={page_size:?}: a page carried {} summaries",
                    page.len()
                );
            }
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case MaxResults={page_size:?}: paging did not terminate after {pages} pages"
            );
            match body["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }

        assert_eq!(
            seen.len(),
            expected.len(),
            "case MaxResults={page_size:?}: expected {} summaries across all pages, got {}",
            expected.len(),
            seen.len()
        );
        for (summary, (policy_id, arn, policy)) in seen.iter().zip(&expected) {
            let want = json!({
                "PolicyArn": arn,
                "PolicyId": policy_id,
                "PolicyName": policy["PolicyName"],
                "ResourceType": policy["ResourceType"],
                "SecurityServiceType": policy["SecurityServicePolicyData"]["Type"],
                "RemediationEnabled": policy["RemediationEnabled"],
                "DeleteUnusedFMManagedResources":
                    policy.get("DeleteUnusedFMManagedResources").cloned().unwrap_or(json!(false)),
                "PolicyStatus": policy["PolicyStatus"],
            });
            assert_eq!(
                *summary, want,
                "case MaxResults={page_size:?}: summary of {policy_id} is not the stored policy's projection, got {summary}"
            );
        }
    }

    // One past each published bound of MaxResults (1..100) is refused by the
    // modelled range, not served.
    for out_of_range in [0, 101] {
        let (status, body) = call(
            &router,
            "ListPolicies",
            &json!({ "MaxResults": out_of_range }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case MaxResults={out_of_range}: a value outside 1..100 must be refused, got {status} with {body}"
        );
    }

    // A token this service never minted is refused rather than served as a page.
    let (status, body) = call(
        &router,
        "ListPolicies",
        &json!({ "NextToken": "not-a-token-this-service-minted" }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "a foreign NextToken must be refused, got {status} {body}"
    );
}

/// `DeletePolicy` removes the policy from state, for either value of
/// `DeleteAllPolicyResources`, so neither `GetPolicy` nor `ListPolicies` can see
/// it afterwards. A second delete of the same id finds nothing.
#[tokio::test]
async fn delete_policy_removes_the_policy_from_state() {
    for delete_all_resources in [None, Some(false), Some(true)] {
        let router = emulator();
        let mut ids = Vec::new();
        for case in cases() {
            let put = put_new(&router, &case).await;
            ids.push((
                case.name,
                put["Policy"]["PolicyId"]
                    .as_str()
                    .expect("a PolicyId")
                    .to_string(),
            ));
        }

        for (name, policy_id) in &ids {
            let mut request = json!({ "PolicyId": policy_id });
            if let Some(flag) = delete_all_resources {
                request["DeleteAllPolicyResources"] = json!(flag);
            }
            let (status, body) = call(&router, "DeletePolicy", &request).await;
            assert_eq!(
                status, 200,
                "case {name:?}/{delete_all_resources:?}: expected DeletePolicy to answer 200, got {status} with {body}"
            );

            let (status, body) = call(&router, "GetPolicy", &json!({ "PolicyId": policy_id })).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {name:?}/{delete_all_resources:?}: GetPolicy after delete must not find it, got {status} {body}"
            );

            let (status, body) = call(&router, "DeletePolicy", &request).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {name:?}/{delete_all_resources:?}: deleting twice must not find it, got {status} {body}"
            );

            let (_, listed) = call(&router, "ListPolicies", &json!({})).await;
            let still_listed = listed["PolicyList"]
                .as_array()
                .expect("a PolicyList")
                .iter()
                .any(|s| s["PolicyId"].as_str() == Some(policy_id.as_str()));
            assert!(
                !still_listed,
                "case {name:?}/{delete_all_resources:?}: ListPolicies still reports the deleted {policy_id}, got {}",
                listed["PolicyList"]
            );
        }

        let (_, listed) = call(&router, "ListPolicies", &json!({})).await;
        assert_eq!(
            listed["PolicyList"].as_array().map(Vec::len),
            Some(0),
            "case {delete_all_resources:?}: every policy was deleted, got {}",
            listed["PolicyList"]
        );
    }
}

/// Both growth sites are bounded: `ListPolicies` never serves a page larger
/// than the model's `MaxResults` maximum even when the caller sends no
/// `MaxResults`, and `PutPolicy` refuses to create past the store bound with the
/// modelled `LimitExceededException`.
#[tokio::test]
async fn the_page_and_the_policy_store_are_bounded() {
    // The model's `com.amazonaws.fms#PaginationMaxResults` range is 1..100, so
    // 100 is the largest page. The store bound is larger than one page on
    // purpose, so a default (`MaxResults`-less) page genuinely truncates and the
    // assertions below cannot pass vacuously; it is `MAX_POLICIES` in
    // `src/ops/put_policy.rs`.
    const MAX_PAGE_SIZE: usize = 100;
    const MAX_POLICIES: usize = 250;
    const { assert!(MAX_POLICIES > MAX_PAGE_SIZE) };

    let router = emulator();
    let case = &cases()[0];
    for created in 0..MAX_POLICIES {
        let (status, body) = call(
            &router,
            "PutPolicy",
            &json!({ "Policy": case.policy, "TagList": case.tags }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case create #{created}: expected 200 below the store bound, got {status} with {body}"
        );
    }

    // One past the bound is refused.
    let (status, body) = call(
        &router,
        "PutPolicy",
        &json!({ "Policy": case.policy, "TagList": case.tags }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "LimitExceededException"),
        "case create #{MAX_POLICIES}: creating past the store bound must be refused, got {status} {body}"
    );

    // With no MaxResults at all the FIRST page is still truncated to the page
    // bound and carries a NextToken — a handler that ignored the default bound
    // would return all MAX_POLICIES here — and every policy is reachable by
    // following the token.
    let mut total = 0;
    let mut pages = 0;
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({});
        if let Some(token) = &token {
            request["NextToken"] = json!(token);
        }
        let (status, listed) = call(&router, "ListPolicies", &request).await;
        assert_eq!(
            status, 200,
            "case unbounded-list: expected 200, got {status} with {listed}"
        );
        let page = listed["PolicyList"].as_array().expect("a PolicyList").len();
        assert!(
            page <= MAX_PAGE_SIZE,
            "case unbounded-list page {pages}: a page with no MaxResults carried {page} summaries, \
             more than the {MAX_PAGE_SIZE} the model bounds a page at"
        );
        let next = listed["NextToken"].as_str().map(str::to_string);
        if next.is_some() {
            // Every page but the last must be FULL, so the bound is what ends it.
            assert_eq!(
                page, MAX_PAGE_SIZE,
                "case unbounded-list page {pages}: a non-final default page must be the \
                 full {MAX_PAGE_SIZE}, got {page}"
            );
        }
        total += page;
        pages += 1;
        assert!(
            pages <= MAX_POLICIES,
            "case unbounded-list: paging did not terminate after {pages} pages"
        );
        match next {
            Some(next) => token = Some(next),
            None => break,
        }
    }
    assert!(
        pages > 1,
        "case unbounded-list: {MAX_POLICIES} policies must span more than one \
         {MAX_PAGE_SIZE}-summary page, got {pages}"
    );
    assert_eq!(
        total, MAX_POLICIES,
        "case unbounded-list: expected {MAX_POLICIES} summaries across all pages, got {total}"
    );
}

/// An unknown `PolicyId` is `ResourceNotFoundException` on every operation that
/// resolves one — on a fresh server and beside stored policies alike.
#[tokio::test]
async fn an_unknown_policy_id_is_not_found() {
    // 36-character ids that satisfy the modelled pattern but name nothing.
    let unknown = [
        "00000000-0000-0000-0000-000000000000",
        "ZZZZZZZZ-ZZZZ-ZZZZ-ZZZZ-ZZZZZZZZZZZZ",
        "------------------------------------",
    ];

    for stored_first in [false, true] {
        let router = emulator();
        if stored_first {
            for case in cases() {
                put_new(&router, &case).await;
            }
        }
        for policy_id in unknown {
            for (operation, request) in [
                ("GetPolicy", json!({ "PolicyId": policy_id })),
                ("DeletePolicy", json!({ "PolicyId": policy_id })),
                (
                    "PutPolicy",
                    json!({ "Policy": {
                        "PolicyId": policy_id,
                        "PolicyUpdateToken": "whatever",
                        "PolicyName": "demo",
                        "SecurityServicePolicyData": { "Type": "WAFV2" },
                        // A resource type the model lists for a WAFV2 policy,
                        // so what is under test here is the unknown PolicyId.
                        "ResourceType": "AWS::ElasticLoadBalancingV2::LoadBalancer",
                        "ExcludeResourceTags": false,
                        "RemediationEnabled": false
                    }}),
                ),
            ] {
                let (status, body) = call(&router, operation, &request).await;
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "ResourceNotFoundException"),
                    "case {operation}/{policy_id}/stored={stored_first}: expected 400 ResourceNotFoundException, got {status} {body}"
                );
            }
        }
    }
}

/// The administrator association: `GetAdminAccount` reports what
/// `AssociateAdminAccount` stored, re-associating the same account is
/// idempotent, a different account is refused, and nothing associated is
/// `ResourceNotFoundException`.
#[tokio::test]
async fn the_admin_account_association_is_persisted_and_single() {
    // Account ids differing in length and digits — the model constrains
    // `AWSAccountId` only to `^[0-9]+$`, length 1..1024.
    let accounts = ["123456789012", "000000000000", "7"];

    for account in accounts {
        let router = emulator();

        let (status, body) = call(&router, "GetAdminAccount", &json!({})).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {account:?}: GetAdminAccount before any association must not find one, got {status} {body}"
        );

        let (status, body) = call(
            &router,
            "AssociateAdminAccount",
            &json!({ "AdminAccount": account }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {account:?}: expected AssociateAdminAccount to answer 200, got {status} with {body}"
        );

        let (status, got) = call(&router, "GetAdminAccount", &json!({})).await;
        assert_eq!(
            status, 200,
            "case {account:?}: expected GetAdminAccount to answer 200, got {status} with {got}"
        );
        assert_eq!(
            got["AdminAccount"].as_str(),
            Some(account),
            "case {account:?}: GetAdminAccount must report the associated account, got {}",
            got["AdminAccount"]
        );
        // `AccountRoleStatus` READY: the association is returned ready.
        assert_eq!(
            got["RoleStatus"].as_str(),
            Some("READY"),
            "case {account:?}: expected RoleStatus READY, got {}",
            got["RoleStatus"]
        );

        // Re-associating the SAME account changes nothing.
        let (status, body) = call(
            &router,
            "AssociateAdminAccount",
            &json!({ "AdminAccount": account }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {account:?}: re-associating the same account must be idempotent, got {status} with {body}"
        );

        // A DIFFERENT account cannot become a second default administrator.
        let other = if account == "999999999999" {
            "888888888888"
        } else {
            "999999999999"
        };
        let (status, body) = call(
            &router,
            "AssociateAdminAccount",
            &json!({ "AdminAccount": other }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidOperationException"),
            "case {account:?}: associating {other} over it must be refused, got {status} {body}"
        );

        let (_, got) = call(&router, "GetAdminAccount", &json!({})).await;
        assert_eq!(
            got["AdminAccount"].as_str(),
            Some(account),
            "case {account:?}: the refused association must not have changed state, got {}",
            got["AdminAccount"]
        );
    }
}

/// Reads do not change what a later caller sees: repeating `GetAdminAccount`,
/// `GetPolicy` and `ListPolicies` answers identically every time.
#[tokio::test]
async fn reads_are_stable_across_repeated_calls() {
    let router = emulator();
    call(
        &router,
        "AssociateAdminAccount",
        &json!({ "AdminAccount": "123456789012" }),
    )
    .await;
    let mut ids = Vec::new();
    for case in cases() {
        let put = put_new(&router, &case).await;
        ids.push(put["Policy"]["PolicyId"].as_str().unwrap().to_string());
    }

    for (operation, request) in [
        ("GetAdminAccount", json!({})),
        ("ListPolicies", json!({})),
        ("ListPolicies", json!({ "MaxResults": 2 })),
        ("GetPolicy", json!({ "PolicyId": ids[0] })),
        ("GetPolicy", json!({ "PolicyId": ids[ids.len() - 1] })),
    ] {
        let (first_status, first) = call(&router, operation, &request).await;
        for round in 1..4 {
            let (status, again) = call(&router, operation, &request).await;
            assert_eq!(
                (status, &again),
                (first_status, &first),
                "case {operation}/{request} round {round}: a read changed its own answer, got {status} {again}"
            );
        }
    }
}
