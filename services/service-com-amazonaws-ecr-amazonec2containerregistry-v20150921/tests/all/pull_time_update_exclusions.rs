// SPDX-License-Identifier: CC0-1.0

//! The registry's pull time update exclusion list, end to end over the wire.
//!
//! Every case goes through the router, so it exercises routing, deserialization, the
//! handler, the persisted state and serialization — the same path a client takes.
//!
//! The rule under test: an exclusion is a registry-level record keyed by the IAM
//! principal ARN, so `RegisterPullTimeUpdateExclusion` either stores one record for a
//! principal the registry does not yet exclude — answering that ARN and the instant the
//! record was created — or, for a principal it already excludes, is refused with
//! `ExclusionAlreadyExistsException` and stores nothing; and
//! `ListPullTimeUpdateExclusions` reports exactly the stored ARNs, in principal-ARN
//! order, one page at a time.
//!
//! The ARN rows span the forms `PrincipalArn` admits — its modelled
//! `@length(min: 0, max: 200)` and
//! `@pattern("^arn:aws(-[a-z]+)*:iam::[0-9]{12}:(role|user)/[\w+=,.@-]+(/[\w+=,.@-]+)*$")`,
//! read with
//! `smithy_query(query="shape",shape="PrincipalArn")` — rather than one recorded ARN:
//! partition, principal kind, account, path depth, name alphabet and length.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request to ECR.
const SIGNED_FOR_EU_WEST_1: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/ecr/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire protocol").
const TARGET_PREFIX: &str = "AmazonEC2ContainerRegistry_V20150921";

/// The longest ARN prefix the rows below share, used to build a principal ARN of
/// exactly the modelled maximum length and one character past it.
const ROLE_PREFIX: &str = "arn:aws:iam::000000000000:role/";

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
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }
}

/// Call one operation of this service on the awsJson1_1 wire: `POST /` with
/// `X-Amz-Target: <prefix>.<operation>` and a JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> Answer {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "api.ecr.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_EU_WEST_1)
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// A successful call, or a panic naming the case and what came back instead.
async fn ok(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let answer = call(router, operation, body).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from {operation}, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body
}

/// One `ListPullTimeUpdateExclusions` page: the ARNs it reported and its `nextToken`.
async fn page(
    router: &MultiServiceRouter,
    case: &str,
    request: &Value,
) -> (Vec<String>, Option<String>) {
    let body = ok(router, case, "ListPullTimeUpdateExclusions", request).await;
    let listed = body["pullTimeUpdateExclusions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a pullTimeUpdateExclusions list, got {body:?}"))
        .iter()
        .map(|arn| {
            arn.as_str()
                .unwrap_or_else(|| panic!("case {case:?}: an exclusion must be an ARN string, got {arn:?}"))
                .to_string()
        })
        .collect();
    let token = body["nextToken"].as_str().map(ToString::to_string);
    (listed, token)
}

/// Every ARN the listing reports, walked page by page with `max_results` per page.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    max_results: Option<usize>,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({});
        if let Some(size) = max_results {
            request["maxResults"] = json!(size);
        }
        if let Some(token) = token.as_ref() {
            request["nextToken"] = json!(token);
        }
        let (listed, next) = page(router, case, &request).await;
        if let Some(size) = max_results {
            assert!(
                listed.len() <= size,
                "case {case:?}: a page may hold at most {size} entries, got {}",
                listed.len()
            );
        }
        seen.extend(listed);
        token = next;
        if token.is_none() {
            break;
        }
    }
    seen
}

/// Seconds since the epoch, for bracketing a `createdAt` the service generated.
fn now_epoch_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// The principal ARN forms this emulator must accept, one row per dimension of
/// `PrincipalArn`'s modelled pattern: partition (including the multi-segment isolated
/// partitions `(-[a-z]+)*` admits), `role` vs `user`, the account id, path depth, the
/// `[\w+=,.@-]` name alphabet, and the length bound.
fn principal_arns() -> Vec<(String, String)> {
    let longest = format!(
        "{ROLE_PREFIX}{}",
        "a".repeat(200 - ROLE_PREFIX.len())
    );
    vec![
        (
            "a role in the commercial partition".to_string(),
            "arn:aws:iam::000000000000:role/pull-time-role".to_string(),
        ),
        (
            "a user in the commercial partition".to_string(),
            "arn:aws:iam::000000000000:user/pull-time-user".to_string(),
        ),
        (
            "a role in a one-segment partition".to_string(),
            "arn:aws-us-gov:iam::210987654321:role/gov-role".to_string(),
        ),
        (
            "a user in a two-segment partition".to_string(),
            "arn:aws-iso-b:iam::123456789012:user/iso-user".to_string(),
        ),
        (
            "a role carrying a path".to_string(),
            "arn:aws:iam::123456789012:role/team/sub/path-role".to_string(),
        ),
        (
            "a name spanning the admitted alphabet".to_string(),
            "arn:aws:iam::123456789012:user/odd+name=x,y.z@example-1_2".to_string(),
        ),
        (
            "a principal of another account".to_string(),
            "arn:aws:iam::999999999999:role/cross-account".to_string(),
        ),
        ("an ARN of the maximum length".to_string(), longest),
    ]
}

/// A register stores one record per principal and answers the ARN with the instant the
/// record was created; the listing reads back exactly what was stored; a second register
/// of the same principal is refused and changes nothing.
#[tokio::test]
async fn registering_a_principal_persists_one_exclusion_the_listing_reports() {
    let router = emulator();
    let rows = principal_arns();
    let before = now_epoch_secs();

    for (case, arn) in &rows {
        let registered = ok(
            &router,
            case,
            "RegisterPullTimeUpdateExclusion",
            &json!({"principalArn": arn}),
        )
        .await;
        assert_eq!(
            registered["principalArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: the register reports the principal it excluded, got {:?}",
            registered["principalArn"]
        );
        let created_at = registered["createdAt"].as_f64().unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a createdAt timestamp, got {:?}",
                registered["createdAt"]
            )
        });
        let after = now_epoch_secs();
        assert!(
            created_at >= before - 1.0 && created_at <= after + 1.0,
            "case {case:?}: createdAt must be the instant of the register, \
             expected between {before} and {after}, got {created_at}"
        );

        // The record is keyed by the principal: a second register of the same one is
        // refused rather than replacing it.
        let again = call(
            &router,
            "RegisterPullTimeUpdateExclusion",
            &json!({"principalArn": arn}),
        )
        .await;
        assert_eq!(
            again.status, 400,
            "case {case:?}: expected HTTP 400 on the second register, got {} with {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            "ExclusionAlreadyExistsException",
            "case {case:?}: expected ExclusionAlreadyExistsException, got {:?}",
            again.body
        );
    }

    // The listing reports every stored ARN exactly once, in principal-ARN order, and a
    // listing that fits in one page carries no token.
    let (listed, token) = page(&router, "the whole list", &json!({})).await;
    let mut expected: Vec<String> = rows.iter().map(|(_, arn)| arn.clone()).collect();
    expected.sort();
    assert_eq!(
        listed, expected,
        "the listing reports the stored exclusions in principal ARN order, got {listed:?}"
    );
    assert!(
        token.is_none(),
        "an untruncated listing carries no nextToken, got {token:?}"
    );
}

/// The listing pages the list in one deterministic order: every page size walks exactly
/// the same ARNs, exactly once, sorted; a token this service did not mint is refused;
/// and `maxResults` is honoured at each published bound and refused past them.
#[tokio::test]
async fn the_listing_pages_the_exclusion_list_in_a_stable_order() {
    use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state::DEFAULT_PAGE_SIZE;

    let router = emulator();
    let rows = principal_arns();
    for (case, arn) in &rows {
        ok(
            &router,
            case,
            "RegisterPullTimeUpdateExclusion",
            &json!({"principalArn": arn}),
        )
        .await;
    }
    let mut expected: Vec<String> = rows.iter().map(|(_, arn)| arn.clone()).collect();
    expected.sort();

    // Every page size from one entry to one past the whole list, plus the request that
    // names no page size at all.
    for size in 1..=(expected.len() + 1) {
        let case = format!("maxResults {size}");
        assert_eq!(
            walk(&router, &case, Some(size)).await,
            expected,
            "case {case:?}: paging must walk the whole list once, in order"
        );
    }
    assert_eq!(
        walk(&router, "no maxResults", None).await,
        expected,
        "a listing with no maxResults reports the whole list, in order"
    );

    // `MaxResults` is modelled `@range(min: 1, max: 1000)`: both bounds are served and
    // anything past either is refused at the boundary.
    for size in [1usize, 1000] {
        let case = format!("the published bound maxResults {size}");
        let (listed, _) = page(&router, &case, &json!({"maxResults": size})).await;
        assert_eq!(
            listed.len(),
            size.min(expected.len()).min(DEFAULT_PAGE_SIZE),
            "case {case:?}: expected a page of {} entries, got {listed:?}",
            size.min(expected.len()).min(DEFAULT_PAGE_SIZE)
        );
    }
    for size in [0i64, 1001] {
        let case = format!("maxResults {size}, past the modelled range");
        let answer = call(
            &router,
            "ListPullTimeUpdateExclusions",
            &json!({"maxResults": size}),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        // A refusal against a constraint the model declares is stated with the
        // client-input error this service models — `InvalidParameterException`, which
        // this operation declares — naming the member and the bound it failed. See
        // `override_refusal` in `src/service.rs`.
        assert_eq!(
            answer.error_code(),
            "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {:?}",
            answer.body
        );
    }

    // A token that was not minted by this service is refused rather than served.
    let bad = call(
        &router,
        "ListPullTimeUpdateExclusions",
        &json!({"nextToken": "not-a-token"}),
    )
    .await;
    assert_eq!(
        bad.status, 400,
        "an unreadable nextToken is refused, got {} with {:?}",
        bad.status, bad.body
    );
    assert_eq!(
        bad.error_code(),
        "InvalidParameterException",
        "expected InvalidParameterException for a bad token, got {:?}",
        bad.body
    );
}

/// A principal ARN outside `PrincipalArn`'s modelled constraints is refused at the
/// boundary, and the refused register stores nothing.
#[tokio::test]
async fn a_principal_arn_outside_the_modelled_constraints_is_refused() {
    let router = emulator();
    let too_long = format!(
        "{ROLE_PREFIX}{}",
        "a".repeat(200 - ROLE_PREFIX.len() + 1)
    );
    let cases: Vec<(&str, String)> = vec![
        ("an empty ARN", String::new()),
        (
            "a principal kind the pattern does not admit",
            "arn:aws:iam::000000000000:group/admins".to_string(),
        ),
        (
            "an account id of eleven digits",
            "arn:aws:iam::00000000000:role/short-account".to_string(),
        ),
        (
            "another service's ARN",
            "arn:aws:ecr:eu-west-1:000000000000:repository/not-a-principal".to_string(),
        ),
        (
            "a role with no name",
            "arn:aws:iam::000000000000:role/".to_string(),
        ),
        ("an ARN one character past the maximum length", too_long),
    ];

    for (case, arn) in &cases {
        let answer = call(
            &router,
            "RegisterPullTimeUpdateExclusion",
            &json!({"principalArn": arn}),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        // Same rule as above: the modelled client-input error, naming the member and
        // the constraint it failed (`override_refusal` in `src/service.rs`).
        assert_eq!(
            answer.error_code(),
            "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {:?}",
            answer.body
        );
        assert!(
            answer.body["message"]
                .as_str()
                .unwrap_or_default()
                .contains("at 'principalArn' failed to satisfy constraint:"),
            "case {case:?}: the message must name the member and the constraint, got {:?}",
            answer.body
        );
    }

    let (listed, _) = page(&router, "after the refusals", &json!({})).await;
    assert!(
        listed.is_empty(),
        "a refused register stores no exclusion, got {listed:?}"
    );
}

/// The list is bounded: past its cap a register is refused with the
/// `LimitExceededException` the operation models, rather than growing without end. A
/// page is bounded too — `PullTimeUpdateExclusionList` is modelled `@length(max: 100)`,
/// so one call can never report more than that however much it asks for.
#[tokio::test]
async fn the_exclusion_list_and_the_page_it_serves_are_bounded() {
    use service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::state::{
        DEFAULT_PAGE_SIZE, MAX_PULL_TIME_UPDATE_EXCLUSIONS,
    };

    let router = emulator();
    for index in 0..MAX_PULL_TIME_UPDATE_EXCLUSIONS {
        ok(
            &router,
            "filling the exclusion list",
            "RegisterPullTimeUpdateExclusion",
            &json!({"principalArn": format!("{ROLE_PREFIX}bulk-{index:05}")}),
        )
        .await;
    }

    let refused = call(
        &router,
        "RegisterPullTimeUpdateExclusion",
        &json!({"principalArn": format!("{ROLE_PREFIX}one-too-many")}),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "a register past the cap is refused, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "LimitExceededException",
        "expected LimitExceededException past {MAX_PULL_TIME_UPDATE_EXCLUSIONS} exclusions, \
         got {:?}",
        refused.body
    );

    // A page is never larger than the documented maximum, however much is asked for.
    // The whole list cannot be larger than that maximum either — the cap is
    // `PullTimeUpdateExclusionList`'s modelled `@length(max: 100)` — so a full list is
    // served in one page and that page carries no token.
    let (listed, token) = page(
        &router,
        "a page larger than the maximum",
        &json!({"maxResults": 1000}),
    )
    .await;
    assert_eq!(
        listed.len(),
        MAX_PULL_TIME_UPDATE_EXCLUSIONS.min(DEFAULT_PAGE_SIZE),
        "a page holds at most {DEFAULT_PAGE_SIZE} entries, got {}",
        listed.len()
    );
    assert!(
        token.is_none(),
        "a list that fits in one page carries no nextToken, got {token:?}"
    );

    // And paging through the filled list still walks every record exactly once.
    let walked = walk(&router, "the filled list", Some(100)).await;
    assert_eq!(
        walked.len(),
        MAX_PULL_TIME_UPDATE_EXCLUSIONS,
        "paging reports every stored exclusion, got {} of {MAX_PULL_TIME_UPDATE_EXCLUSIONS}",
        walked.len()
    );
    assert_eq!(
        walked.iter().collect::<BTreeSet<_>>().len(),
        MAX_PULL_TIME_UPDATE_EXCLUSIONS,
        "paging reports each exclusion exactly once"
    );
}
