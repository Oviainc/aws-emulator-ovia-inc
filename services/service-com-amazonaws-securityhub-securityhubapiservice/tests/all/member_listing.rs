// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `ListMembers`: `EnableSecurityHub` → `CreateMembers`
//! → `ListMembers` (paged) → `DeleteMembers` → `ListMembers`.
//!
//! Every case drives the real restJson1 wire path through `MultiServiceRouter`, so a
//! route, a query-bound parameter, a status code or a serialized member that is wrong
//! on the wire fails here.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region most cases sign for; the handler reads it out of the SigV4 credential
/// scope, exactly as a signed SDK request carries it.
const REGION: &str = "us-east-1";

/// A second Region, to pin that a list answers for the hub of the Region it was
/// signed for and for no other.
const OTHER_REGION: &str = "us-west-2";

/// The account every emulated caller presents — the administrator account of every
/// association these cases create.
const ADMINISTRATOR_ID: &str = "000000000000";

/// `Member$MemberStatus` after `CreateMembers`: "Created - Indicates that the
/// administrator account added the member account, but has not yet invited the member
/// account" (`smithy_query(query="shape", shape="Member")`, member `MemberStatus`).
const STATUS_CREATED: &str = "Created";

/// `MaxResults`' published bounds: `CrossAccountMaxResults` is `@range(min: 1, max:
/// 50)` (`smithy_query(query="operation", operation="ListMembers")`).
const MAX_RESULTS_MIN: usize = 1;
const MAX_RESULTS_MAX: usize = 50;

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

    /// Dispatch one request signed for `region` and return `(status, parsed body)`.
    async fn call(
        &self,
        method: &str,
        path: &str,
        region: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let serialized = body.map(|b| serde_json::to_vec(&b).expect("serializable request body"));
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

    async fn enable(&self, region: &str) -> (u16, Value) {
        self.call("POST", "/accounts", region, Some(json!({}))).await
    }

    async fn create_members(&self, region: &str, account_details: Value) -> (u16, Value) {
        self.call(
            "POST",
            "/members",
            region,
            Some(json!({ "AccountDetails": account_details })),
        )
        .await
    }

    async fn delete_members(&self, region: &str, account_ids: &[&str]) -> (u16, Value) {
        self.call(
            "POST",
            "/members/delete",
            region,
            Some(json!({ "AccountIds": account_ids })),
        )
        .await
    }

    async fn get_members(&self, region: &str, account_ids: &[&str]) -> (u16, Value) {
        self.call(
            "POST",
            "/members/get",
            region,
            Some(json!({ "AccountIds": account_ids })),
        )
        .await
    }

    /// One `ListMembers` call. Every parameter is `@httpQuery`-bound
    /// (`smithy_query(query="operation", operation="ListMembers")`), so each is sent
    /// on the query string exactly as an SDK would.
    async fn list_members(
        &self,
        region: &str,
        max_results: Option<usize>,
        next_token: Option<&str>,
        only_associated: Option<bool>,
    ) -> (u16, Value) {
        let mut query: Vec<String> = Vec::new();
        if let Some(max_results) = max_results {
            query.push(format!("MaxResults={max_results}"));
        }
        if let Some(next_token) = next_token {
            query.push(format!("NextToken={next_token}"));
        }
        if let Some(only_associated) = only_associated {
            query.push(format!("OnlyAssociated={only_associated}"));
        }
        let path = if query.is_empty() {
            "/members".to_string()
        } else {
            format!("/members?{}", query.join("&"))
        };
        self.call("GET", &path, region, None).await
    }

    /// A harness whose caller is subscribed in `REGION`.
    async fn enabled() -> Self {
        let harness = Self::new();
        let (status, body) = harness.enable(REGION).await;
        assert_eq!(
            status, 200,
            "EnableSecurityHub must succeed, got {status} with {body}"
        );
        harness
    }

    /// Create `ids` as member associations of the hub in `REGION`.
    async fn with_members(ids: &[String]) -> Self {
        let harness = Self::enabled().await;
        if ids.is_empty() {
            return harness;
        }
        for chunk in ids.chunks(MAX_RESULTS_MAX) {
            let details: Vec<Value> = chunk
                .iter()
                .map(|id| json!({ "AccountId": id, "Email": format!("{id}@example.com") }))
                .collect();
            let (status, body) = harness.create_members(REGION, json!(details)).await;
            assert_eq!(
                status, 200,
                "CreateMembers for {} accounts expected 200, got {status} with {body}",
                chunk.len()
            );
        }
        harness
    }
}

/// A 12-digit account id built from `n`, so a case can span member counts without a
/// hand-written list.
fn account_id(n: usize) -> String {
    format!("{:012}", 100_000_000_000u64 + n as u64)
}

/// The `Members` list of a `ListMembers`/`GetMembers` response.
fn members_of(body: &Value) -> &Vec<Value> {
    body["Members"]
        .as_array()
        .unwrap_or_else(|| panic!("the response must carry a Members list, got {body}"))
}

/// The `AccountId` of every member in a response, in response order.
fn listed_ids(body: &Value) -> Vec<String> {
    members_of(body)
        .iter()
        .map(|member| {
            member["AccountId"]
                .as_str()
                .unwrap_or_else(|| panic!("every member must carry an AccountId, got {member}"))
                .to_string()
        })
        .collect()
}

/// Walk every page of `ListMembers` with `max_results` per call, following
/// `NextToken`, and return the ids in the order the pages produced them.
async fn walk_pages(harness: &Harness, case: &str, max_results: Option<usize>) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    // One page per member at worst, plus a page that ends the walk; a token that
    // never clears would otherwise loop forever.
    for page in 0..=(MAX_RESULTS_MAX * 2 + 2) {
        let (status, body) = harness
            .list_members(REGION, max_results, token.as_deref(), None)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?} page {page}: ListMembers expected 200, got {status} with {body}"
        );
        let page_ids = listed_ids(&body);
        if let Some(limit) = max_results {
            assert!(
                page_ids.len() <= limit,
                "case {case:?} page {page}: a page must hold at most MaxResults={limit}, got {}",
                page_ids.len()
            );
        }
        ids.extend(page_ids);
        match body["NextToken"].as_str() {
            Some(next) => {
                assert!(
                    !next.is_empty(),
                    "case {case:?} page {page}: NextToken is a NonEmptyString, got {body}"
                );
                token = Some(next.to_string());
            }
            None => return ids,
        }
    }
    panic!("case {case:?}: ListMembers never stopped handing back a NextToken");
}

/// RULE: `ListMembers` reports the member associations the calling account's hub holds
/// — every one of them, each exactly once, in member-account-id order — a page of at
/// most `MaxResults` at a time, handing back a `NextToken` only while associations
/// remain, and nothing once the associations are gone.
///
/// The table spans what the rule generalises over: how many associations the hub holds
/// (none through the modelled page maximum and past it) and the page size the caller
/// asks for (absent, and each published bound of `MaxResults`).
#[tokio::test]
async fn list_members_pages_every_association_in_account_id_order() {
    // Member counts: none, one, a handful, exactly one page, and more than one page.
    for count in [0_usize, 1, 7, MAX_RESULTS_MAX, MAX_RESULTS_MAX + 11] {
        let ids: Vec<String> = (0..count).map(account_id).collect();
        let harness = Harness::with_members(&ids).await;

        // Account-id order is the order the rule promises, and these ids are minted
        // in ascending order, so the expectation is the sorted request order.
        let mut expected = ids.clone();
        expected.sort();

        // Page sizes: the published bounds of `MaxResults`, one in between, and the
        // call that names none at all.
        for max_results in [
            None,
            Some(MAX_RESULTS_MIN),
            Some(3),
            Some(MAX_RESULTS_MAX),
        ] {
            let case = format!("{count} members, MaxResults={max_results:?}");
            let walked = walk_pages(&harness, &case, max_results).await;
            assert_eq!(
                walked, expected,
                "case {case:?}: expected every association once in account-id order, got {walked:?}"
            );
        }
    }
}

/// RULE: a deleted member association is no longer one the hub holds, so `ListMembers`
/// stops reporting it while reporting the associations the same hub still holds.
#[tokio::test]
async fn deleted_associations_leave_the_list() {
    let ids: Vec<String> = (0..5).map(account_id).collect();
    let harness = Harness::with_members(&ids).await;

    // Delete them one at a time, so each row has a different remaining set.
    let mut remaining: Vec<String> = ids.clone();
    remaining.sort();
    for deleted in &ids {
        let (status, body) = harness.delete_members(REGION, &[deleted.as_str()]).await;
        assert_eq!(
            status, 200,
            "case delete {deleted}: DeleteMembers expected 200, got {status} with {body}"
        );
        remaining.retain(|id| id != deleted);

        let listed = walk_pages(&harness, &format!("after deleting {deleted}"), None).await;
        assert_eq!(
            listed, remaining,
            "case after deleting {deleted}: expected {remaining:?}, got {listed:?}"
        );
    }

    // Every association deleted: the hub holds none, so the answer is an empty list
    // and no `NextToken`.
    let (status, body) = harness.list_members(REGION, None, None, None).await;
    assert_eq!(
        status, 200,
        "ListMembers on an empty hub expected 200, got {status} with {body}"
    );
    assert!(
        members_of(&body).is_empty(),
        "case every association deleted: expected an empty Members list, got {body}"
    );
    assert!(
        body["NextToken"].is_null(),
        "case every association deleted: there is no next page, so NextToken must be absent, \
         got {body}"
    );
}

/// RULE: `OnlyAssociated` selects which associations are reported by their
/// relationship status — narrowed (the default) the answer is the associations still
/// in force with the administrator, and `false` reports every association the hub
/// holds. An association `CreateMembers` made is in force (`Created`), so it is
/// reported either way.
#[tokio::test]
async fn only_associated_selects_by_relationship_status() {
    let ids: Vec<String> = (0..3).map(account_id).collect();
    let harness = Harness::with_members(&ids).await;
    let mut expected = ids.clone();
    expected.sort();

    // Rows: the parameter absent (its documented default, TRUE), explicitly TRUE, and
    // explicitly FALSE.
    for only_associated in [None, Some(true), Some(false)] {
        let case = format!("OnlyAssociated={only_associated:?}");
        let (status, body) = harness
            .list_members(REGION, None, None, only_associated)
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListMembers expected 200, got {status} with {body}"
        );
        let listed = listed_ids(&body);
        assert_eq!(
            listed, expected,
            "case {case:?}: a created association is in force, so expected {expected:?}, got \
             {listed:?}"
        );
    }
}

/// RULE: `ListMembers` reports each association exactly as the hub holds it, which is
/// the same projection `GetMembers` answers with for the same account.
#[tokio::test]
async fn a_listed_member_matches_the_one_get_members_reports() {
    let ids: Vec<String> = (0..4).map(account_id).collect();
    let harness = Harness::with_members(&ids).await;

    let (status, listed) = harness.list_members(REGION, None, None, None).await;
    assert_eq!(
        status, 200,
        "ListMembers expected 200, got {status} with {listed}"
    );
    let requested: Vec<&str> = ids.iter().map(String::as_str).collect();
    let (status, fetched) = harness.get_members(REGION, &requested).await;
    assert_eq!(
        status, 200,
        "GetMembers expected 200, got {status} with {fetched}"
    );

    for id in &ids {
        let from_list = members_of(&listed)
            .iter()
            .find(|member| member["AccountId"].as_str() == Some(id.as_str()))
            .unwrap_or_else(|| panic!("case {id}: ListMembers must report it, got {listed}"));
        let from_get = members_of(&fetched)
            .iter()
            .find(|member| member["AccountId"].as_str() == Some(id.as_str()))
            .unwrap_or_else(|| panic!("case {id}: GetMembers must report it, got {fetched}"));
        assert_eq!(
            from_list, from_get,
            "case {id}: the two reads must describe one association, got {from_list} and \
             {from_get}"
        );
        assert_eq!(
            from_list["MemberStatus"].as_str(),
            Some(STATUS_CREATED),
            "case {id}: expected {STATUS_CREATED}, got {from_list}"
        );
        assert_eq!(
            from_list["AdministratorId"].as_str(),
            Some(ADMINISTRATOR_ID),
            "case {id}: AdministratorId must be the calling account, got {from_list}"
        );
        assert_eq!(
            from_list["MasterId"].as_str(),
            Some(ADMINISTRATOR_ID),
            "case {id}: MasterId must mirror AdministratorId, got {from_list}"
        );
        assert_eq!(
            from_list["Email"].as_str(),
            Some(format!("{id}@example.com").as_str()),
            "case {id}: Email must be the one submitted, got {from_list}"
        );
    }
}

/// RULE: a value outside a `ListMembers` parameter's documented range, and a
/// `NextToken` this operation never issued, are refused with a 4xx client error rather
/// than silently read as something else.
#[tokio::test]
async fn out_of_range_parameters_and_foreign_tokens_are_refused() {
    let ids: Vec<String> = (0..4).map(account_id).collect();
    let harness = Harness::with_members(&ids).await;

    // `MaxResults` is `@range(min: 1, max: 50)`: each bound is accepted, and the value
    // one past each is refused.
    for (case, max_results, expected) in [
        ("the minimum page size", MAX_RESULTS_MIN, 200),
        ("the maximum page size", MAX_RESULTS_MAX, 200),
        ("one below the minimum page size", MAX_RESULTS_MIN - 1, 400),
        ("one past the maximum page size", MAX_RESULTS_MAX + 1, 400),
    ] {
        let (status, body) = harness
            .list_members(REGION, Some(max_results), None, None)
            .await;
        assert_eq!(
            status, expected,
            "case {case:?} (MaxResults={max_results}): expected {expected}, got {status} with \
             {body}"
        );
    }

    // A token the caller invented, and one minted by another list of this service
    // (`DescribeActionTargets`' cursor for offset 0, hex of its own prefix), are both
    // rejected: a `NextToken` is only ever one this operation issued.
    for (case, token) in [
        ("not a token at all", "not-a-token".to_string()),
        ("an empty-looking cursor", "00".to_string()),
        (
            "a token minted by another list",
            ::hex::encode("securityhub:actiontargets:0"),
        ),
    ] {
        let (status, body) = harness
            .list_members(REGION, None, Some(&token), None)
            .await;
        assert_eq!(
            status, 400,
            "case {case:?} ({token}): expected 400, got {status} with {body}"
        );
    }
}

/// RULE: `ListMembers` answers for the hub of the calling account AND the Region the
/// call was signed for — so it is refused with `InvalidAccessException` (401) while
/// that account is not subscribed there, and never reports another Region's
/// associations.
#[tokio::test]
async fn the_list_is_scoped_to_the_calling_hub() {
    let harness = Harness::new();
    let (status, body) = harness.list_members(REGION, None, None, None).await;
    assert_eq!(
        status, 401,
        "ListMembers before EnableSecurityHub: expected 401, got {status} with {body}"
    );

    let member = account_id(0);
    let harness = Harness::with_members(std::slice::from_ref(&member)).await;
    let (status, body) = harness.list_members(OTHER_REGION, None, None, None).await;
    assert_eq!(
        status, 401,
        "ListMembers in an unsubscribed Region: expected 401, got {status} with {body}"
    );

    let (status, body) = harness.enable(OTHER_REGION).await;
    assert_eq!(
        status, 200,
        "EnableSecurityHub in {OTHER_REGION}: got {status} with {body}"
    );
    for (region, expected) in [(REGION, vec![member.clone()]), (OTHER_REGION, Vec::new())] {
        let (status, body) = harness.list_members(region, None, None, None).await;
        assert_eq!(
            status, 200,
            "case {region}: ListMembers expected 200, got {status} with {body}"
        );
        assert_eq!(
            listed_ids(&body),
            expected,
            "case {region}: expected {expected:?}, got {body}"
        );
    }
}
