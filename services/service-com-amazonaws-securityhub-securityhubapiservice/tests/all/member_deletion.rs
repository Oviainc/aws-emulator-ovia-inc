// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `DeleteMembers`: `EnableSecurityHub` →
//! `CreateMembers` → `DeleteMembers` → `GetMembers`.
//!
//! Every case drives the real restJson1 wire path through `MultiServiceRouter`, so a
//! route, a status code or a serialized result that is wrong on the wire fails here.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region most cases sign for; the handler reads it out of the SigV4 credential
/// scope, exactly as a signed SDK request carries it.
const REGION: &str = "us-east-1";

/// A second Region, to pin that a delete acts on the hub of the Region it was signed
/// for and on no other.
const OTHER_REGION: &str = "us-west-2";

/// The account every emulated caller presents — the administrator account of every
/// association these cases create.
const ADMINISTRATOR_ID: &str = "000000000000";

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

    /// Create `ids` as member associations of the caller's hub in `region`.
    async fn with_members(&self, region: &str, ids: &[String]) {
        let details: Vec<Value> = ids
            .iter()
            .map(|id| json!({ "AccountId": id, "Email": format!("{id}@example.com") }))
            .collect();
        let (status, body) = self.create_members(region, json!(details)).await;
        assert_eq!(
            status, 200,
            "CreateMembers for {ids:?} expected 200, got {status} with {body}"
        );
    }
}

/// A 12-digit account id built from `n`, so a case can span batch sizes without a
/// hand-written list.
fn account_id(n: usize) -> String {
    format!("{:012}", 100_000_000_000u64 + n as u64)
}

/// The `Members` list of a `GetMembers` response.
fn members_of(body: &Value) -> &Vec<Value> {
    body["Members"]
        .as_array()
        .unwrap_or_else(|| panic!("GetMembers must answer a Members list, got {body}"))
}

/// The `UnprocessedAccounts` list of a response.
fn unprocessed_of(body: &Value) -> &Vec<Value> {
    body["UnprocessedAccounts"]
        .as_array()
        .unwrap_or_else(|| panic!("response must carry UnprocessedAccounts, got {body}"))
}

/// The account ids an `UnprocessedAccounts` list names, in response order.
fn unprocessed_ids(body: &Value) -> Vec<&str> {
    unprocessed_of(body)
        .iter()
        .filter_map(|entry| entry["AccountId"].as_str())
        .collect()
}

/// RULE: `DeleteMembers` removes the member association of each requested account from
/// the calling account's hub, so afterwards that account is no longer a member at all —
/// `GetMembers` answers no `Member` for it and reports it in neither list — while the
/// associations the call did not name stay exactly as they were. Every account it did
/// delete is absent from `UnprocessedAccounts`.
///
/// The table spans what the rule generalises over: how many of the hub's members one
/// call deletes (none, one, some, all), the size of the batch (up to the bound every
/// member operation holds), and the order the ids are given in.
#[tokio::test]
async fn delete_members_removes_exactly_the_requested_associations() {
    // `MAX_MEMBER_BATCH`: the bound the model puts on `CreateMembers`'
    // `AccountDetailsList` ("len 1..50", smithy_query(query="operation",
    // operation="CreateMembers")), which every member operation holds.
    const MAX_BATCH: usize = 50;

    let held: Vec<String> = (0..MAX_BATCH).map(account_id).collect();

    // (case, the indices of `held` to delete)
    let cases: Vec<(&str, Vec<usize>)> = vec![
        ("no account at all", vec![]),
        ("a single member", vec![3]),
        ("three members in request order", vec![0, 1, 2]),
        ("three members in reverse order", vec![9, 8, 7]),
        ("the same member twice in one call", vec![4, 4]),
        (
            "every member of the hub, which is also the maximum batch",
            (0..MAX_BATCH).collect(),
        ),
    ];

    for (case, indices) in cases {
        let harness = Harness::enabled().await;
        harness.with_members(REGION, &held).await;

        let requested: Vec<&str> = indices.iter().map(|n| held[*n].as_str()).collect();
        let (status, body) = harness.delete_members(REGION, &requested).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteMembers expected 200, got {status} with {body}"
        );

        // An account named twice in one call is deleted by the first mention, so the
        // second is reported as an account that was not deleted.
        let mut expected_unprocessed: Vec<&str> = Vec::new();
        let mut seen: Vec<&str> = Vec::new();
        for id in &requested {
            if seen.contains(id) {
                expected_unprocessed.push(id);
            }
            seen.push(id);
        }
        assert_eq!(
            unprocessed_ids(&body),
            expected_unprocessed,
            "case {case:?}: expected {expected_unprocessed:?} not deleted, got {body}"
        );

        // Read every member of the hub back: the deleted ones are gone, the rest are
        // untouched.
        let all: Vec<&str> = held.iter().map(String::as_str).collect();
        let (status, body) = harness.get_members(REGION, &all).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetMembers expected 200, got {status} with {body}"
        );
        let surviving: Vec<&str> = members_of(&body)
            .iter()
            .filter_map(|member| member["AccountId"].as_str())
            .collect();
        let expected_surviving: Vec<&str> = all
            .iter()
            .copied()
            .filter(|id| !requested.contains(id))
            .collect();
        assert_eq!(
            surviving, expected_surviving,
            "case {case:?}: expected the members {expected_surviving:?} to survive, got {body}"
        );
        let mut deleted: Vec<&str> = requested.clone();
        deleted.sort_unstable();
        deleted.dedup();
        assert!(
            unprocessed_of(&body).is_empty(),
            "case {case:?}: a deleted account has no association left, so GetMembers must \
             report {deleted:?} in neither list, got {body}"
        );
        // Every surviving association still carries the details it was created with.
        for member in members_of(&body) {
            let id = member["AccountId"].as_str().unwrap_or_default();
            assert_eq!(
                member["Email"].as_str(),
                Some(format!("{id}@example.com").as_str()),
                "case {case:?}: surviving member {id} must keep its Email, got {member}"
            );
        }
    }
}

/// RULE: an account the call cannot delete is reported per account in
/// `UnprocessedAccounts` with a reason, and the accounts in the SAME call that CAN be
/// deleted are still deleted. An account that is not an association of the calling hub
/// is one such account — including the administrator account itself, which is the other
/// end of an association and never a member of its own hub, and an account whose
/// association was already deleted by an earlier call (so a repeated delete is
/// idempotent).
#[tokio::test]
async fn accounts_that_cannot_be_deleted_are_reported_per_account() {
    let member = account_id(1);
    let stranger = account_id(2);

    let harness = Harness::enabled().await;
    harness
        .with_members(REGION, std::slice::from_ref(&member))
        .await;

    // Rows: (case, the ids one call deletes, the ids it must report unprocessed)
    let rows: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        (
            "a member beside the administrator account and a stranger",
            vec![ADMINISTRATOR_ID, member.as_str(), stranger.as_str()],
            vec![ADMINISTRATOR_ID, stranger.as_str()],
        ),
        (
            "deleting the same member again",
            vec![member.as_str()],
            vec![member.as_str()],
        ),
    ];

    for (case, requested, expected) in rows {
        let (status, body) = harness.delete_members(REGION, &requested).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DeleteMembers expected 200, got {status} with {body}"
        );
        assert_eq!(
            unprocessed_ids(&body),
            expected,
            "case {case:?}: expected {expected:?} not deleted, got {body}"
        );
        for entry in unprocessed_of(&body) {
            assert!(
                entry["ProcessingResult"]
                    .as_str()
                    .is_some_and(|reason| !reason.is_empty()),
                "case {case:?}: an account that was not deleted must say why, got {entry}"
            );
        }
    }

    // The member really is gone, and the administrator account never became one.
    let (status, body) = harness
        .get_members(REGION, &[ADMINISTRATOR_ID, member.as_str()])
        .await;
    assert_eq!(
        status, 200,
        "GetMembers expected 200, got {status} with {body}"
    );
    assert!(
        members_of(&body).is_empty(),
        "every association of the hub was deleted, so GetMembers must answer no member, \
         got {body}"
    );
}

/// RULE: a deleted association can be created again, and comes back in the `Created`
/// status with no invitation sent — "CreateMembers is only used in the following cases:
/// … The account was disassociated or deleted in Security Hub CSPM".
#[tokio::test]
async fn a_deleted_member_can_be_created_again() {
    let harness = Harness::enabled().await;
    let member = account_id(7);
    harness
        .with_members(REGION, std::slice::from_ref(&member))
        .await;

    let (status, body) = harness.delete_members(REGION, &[member.as_str()]).await;
    assert_eq!(
        status, 200,
        "DeleteMembers expected 200, got {status} with {body}"
    );

    let (status, body) = harness
        .create_members(REGION, json!([{ "AccountId": member }]))
        .await;
    assert_eq!(
        status, 200,
        "re-creating a deleted member expected 200, got {status} with {body}"
    );
    assert!(
        unprocessed_of(&body).is_empty(),
        "re-creating a deleted member must process it, got {body}"
    );

    let (status, body) = harness.get_members(REGION, &[member.as_str()]).await;
    assert_eq!(
        status, 200,
        "GetMembers expected 200, got {status} with {body}"
    );
    let members = members_of(&body);
    assert_eq!(
        members.len(),
        1,
        "the re-created account must be a member exactly once, got {body}"
    );
    assert_eq!(
        members[0]["MemberStatus"].as_str(),
        Some("Created"),
        "a re-created member starts in Created, got {}",
        members[0]
    );
    assert!(
        members[0]["InvitedAt"].is_null(),
        "a re-created member has had no invitation sent, got {}",
        members[0]
    );
    assert!(
        members[0]["Email"].is_null(),
        "the deleted association's details are gone, so an entry that carries no Email \
         creates one without it, got {}",
        members[0]
    );
}

/// RULE: a member association belongs to one hub — the administrator account AND the
/// Region it was created in — so a delete signed for one Region leaves the same account
/// id a member in every other Region.
#[tokio::test]
async fn deleting_a_member_acts_on_the_hub_of_the_calling_region_only() {
    let harness = Harness::new();
    for region in [REGION, OTHER_REGION] {
        let (status, body) = harness.enable(region).await;
        assert_eq!(
            status, 200,
            "EnableSecurityHub in {region}: got {status} with {body}"
        );
    }

    let member = account_id(11);
    for region in [REGION, OTHER_REGION] {
        harness
            .with_members(region, std::slice::from_ref(&member))
            .await;
    }

    let (status, body) = harness.delete_members(REGION, &[member.as_str()]).await;
    assert_eq!(
        status, 200,
        "DeleteMembers in {REGION}: expected 200, got {status} with {body}"
    );
    assert!(
        unprocessed_of(&body).is_empty(),
        "the member of {REGION} must be deleted, got {body}"
    );

    for (region, expect_member) in [(REGION, false), (OTHER_REGION, true)] {
        let (status, body) = harness.get_members(region, &[member.as_str()]).await;
        assert_eq!(
            status, 200,
            "GetMembers in {region}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            members_of(&body).len(),
            usize::from(expect_member),
            "GetMembers in {region}: expected member present = {expect_member}, got {body}"
        );
    }
}

/// RULE: `DeleteMembers` holds the same input rules as every other member operation —
/// an account id that is not 12 decimal digits and a batch past the bound are
/// `InvalidInputException` (400), the request is refused whole so nothing is deleted,
/// and the call needs the calling account to be subscribed in the Region it is signed
/// for (`InvalidAccessException`, 401).
#[tokio::test]
async fn delete_members_input_rules_are_enforced() {
    // One row per way an id can miss the 12-digit form, plus the published bounds (11
    // and 13 digits). The empty id is left out: `AccountIdList`'s member is a
    // `NonEmptyString` (pattern `\S`), which the framework refuses as a
    // `ValidationException` before the handler runs.
    let malformed = [
        ("too short", "123"),
        ("one digit short", "11111111111"),
        ("one digit long", "1111111111111"),
        ("not all digits", "00000000000a"),
        ("padded with a space", " 00000000000"),
        ("hyphenated", "0000-0000-000"),
    ];

    let member = account_id(21);
    for (case, bad) in malformed {
        let harness = Harness::enabled().await;
        harness
            .with_members(REGION, std::slice::from_ref(&member))
            .await;

        let (status, body) = harness
            .delete_members(REGION, &[member.as_str(), bad])
            .await;
        assert_eq!(
            status, 400,
            "case {case:?} ({bad:?}): DeleteMembers expected 400, got {status} with {body}"
        );

        let (status, body) = harness.get_members(REGION, &[member.as_str()]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetMembers expected 200, got {status} with {body}"
        );
        assert_eq!(
            members_of(&body).len(),
            1,
            "case {case:?}: a refused request must delete nothing, got {body}"
        );
    }

    // The batch bound every member operation holds: at the bound, and one past it.
    const MAX_BATCH: usize = 50;
    let harness = Harness::enabled().await;
    for (case, count, expected) in [
        ("the maximum batch", MAX_BATCH, 200),
        ("one past the maximum batch", MAX_BATCH + 1, 400),
    ] {
        let ids: Vec<String> = (0..count).map(account_id).collect();
        let requested: Vec<&str> = ids.iter().map(String::as_str).collect();
        let (status, body) = harness.delete_members(REGION, &requested).await;
        assert_eq!(
            status, expected,
            "case {case:?} ({count} ids): DeleteMembers expected {expected}, got {status} \
             with {body}"
        );
    }

    // Unsubscribed: in a Region the caller never enabled, and in a fresh emulator.
    let (status, body) = harness
        .delete_members(OTHER_REGION, &[member.as_str()])
        .await;
    assert_eq!(
        status, 401,
        "DeleteMembers in an unsubscribed Region: expected 401, got {status} with {body}"
    );
    let fresh = Harness::new();
    let (status, body) = fresh.delete_members(REGION, &[member.as_str()]).await;
    assert_eq!(
        status, 401,
        "DeleteMembers before EnableSecurityHub: expected 401, got {status} with {body}"
    );
}
