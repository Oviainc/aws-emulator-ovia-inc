// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the Security Hub CSPM member association slice:
//! `EnableSecurityHub` → `CreateMembers` → `GetMembers`.
//!
//! Every case drives the real restJson1 wire path through `MultiServiceRouter`, so a
//! route, a status code or a serialized member that is wrong on the wire fails here.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region most cases sign for; the handler reads it out of the SigV4 credential
/// scope, exactly as a signed SDK request carries it.
const REGION: &str = "us-east-1";

/// A second Region, to pin that member associations are scoped to one hub.
const OTHER_REGION: &str = "us-west-2";

/// The account every emulated caller presents — the administrator account of every
/// association these cases create.
const ADMINISTRATOR_ID: &str = "000000000000";

/// `Member$MemberStatus` after `CreateMembers`: "Created - Indicates that the
/// administrator account added the member account, but has not yet invited the member
/// account" (`smithy_query(query="shape", shape="Member")`, member `MemberStatus`).
const STATUS_CREATED: &str = "Created";

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
}

/// One `CreateMembers` batch as a case writes it: an account id and the `Email` the
/// `AccountDetails` entry carries, if any.
type Batch = Vec<(String, Option<String>)>;

/// A 12-digit account id built from `n`, so a case can span batch sizes without a
/// hand-written list.
fn account_id(n: usize) -> String {
    format!("{:012}", 100_000_000_000u64 + n as u64)
}

/// One `AccountDetails` entry, with `Email` present only when supplied.
fn detail(account_id: &str, email: Option<&str>) -> Value {
    match email {
        Some(email) => json!({ "AccountId": account_id, "Email": email }),
        None => json!({ "AccountId": account_id }),
    }
}

/// The instant a modelled `Timestamp` member carries, in epoch seconds. `Member$UpdatedAt`
/// is `@timestampFormat("date-time")`, so it arrives as an RFC 3339 string (pinned by
/// `tests/all/timestamp_wire_form.rs`); an epoch number is read too, so the assertions
/// below are about the instant and not its encoding.
fn instant_seconds(value: &Value) -> Option<f64> {
    if let Some(seconds) = value.as_f64() {
        return Some(seconds);
    }
    let text = value.as_str()?;
    aws_smithy_types::DateTime::from_str(text, aws_smithy_types::date_time::Format::DateTime)
        .ok()
        .map(|parsed| parsed.as_secs_f64())
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

/// RULE: `CreateMembers` creates one member association per `AccountDetails` entry of
/// the calling account's hub, in the `Created` status with the administrator account
/// on both `AdministratorId` and the deprecated `MasterId`, the submitted `Email` when
/// one was given, an `UpdatedAt` instant and no `InvitedAt` (no invitation has been
/// sent); `GetMembers` reports exactly those associations, in the order asked for.
///
/// The table spans what the rule generalises over: how many accounts one call carries
/// (1 → the modelled maximum of 50), whether each entry carries an `Email`, and
/// whether the same account appears twice in one call.
#[tokio::test]
async fn create_members_round_trips_every_batch_shape() {
    // `AccountDetailsList`: "len 1..50" (`smithy_query(query="operation",
    // operation="CreateMembers")`), so 1 and 50 are the published bounds.
    let max_batch: Batch = (0..50)
        .map(|n| {
            let id = account_id(n);
            let email = (n % 2 == 0).then(|| format!("member{n}@example.com"));
            (id, email)
        })
        .collect();

    let cases: Vec<(&str, Batch)> = vec![
        (
            "one account with an email",
            vec![("111111111111".to_string(), Some("one@example.com".to_string()))],
        ),
        (
            "one account without an email",
            vec![("222222222222".to_string(), None)],
        ),
        (
            "three accounts, some with an email",
            vec![
                ("333333333333".to_string(), Some("three@example.com".to_string())),
                ("444444444444".to_string(), None),
                ("555555555555".to_string(), Some("five@example.com".to_string())),
            ],
        ),
        (
            "the same account twice in one call",
            vec![
                ("666666666666".to_string(), None),
                ("666666666666".to_string(), Some("six@example.com".to_string())),
            ],
        ),
        ("the maximum batch of 50 accounts", max_batch),
    ];

    for (case, rows) in cases {
        let harness = Harness::enabled().await;
        let details: Vec<Value> = rows
            .iter()
            .map(|(id, email)| detail(id, email.as_deref()))
            .collect();
        let (status, body) = harness.create_members(REGION, json!(details)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateMembers expected 200, got {status} with {body}"
        );
        assert!(
            unprocessed_of(&body).is_empty(),
            "case {case:?}: every account must be processed, got {body}"
        );

        // The association a repeated account id ends up with is the LAST entry for
        // it, so collapse the request the way the store does — keyed by account id,
        // last write wins — and read it back in reverse request order to pin that
        // `GetMembers` answers in the order it was asked.
        let mut expected: Batch = Vec::new();
        for (id, email) in &rows {
            match expected.iter_mut().find(|(seen, _)| seen == id) {
                Some(entry) => {
                    if email.is_some() {
                        entry.1 = email.clone();
                    }
                }
                None => expected.push((id.clone(), email.clone())),
            }
        }
        expected.reverse();

        let requested: Vec<&str> = expected.iter().map(|(id, _)| id.as_str()).collect();
        let (status, body) = harness.get_members(REGION, &requested).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetMembers expected 200, got {status} with {body}"
        );
        assert!(
            unprocessed_of(&body).is_empty(),
            "case {case:?}: every created account must be found, got {body}"
        );
        let members = members_of(&body);
        assert_eq!(
            members.len(),
            expected.len(),
            "case {case:?}: expected {} members, got {body}",
            expected.len()
        );
        for (index, (id, email)) in expected.iter().enumerate() {
            let member = &members[index];
            assert_eq!(
                member["AccountId"].as_str(),
                Some(id.as_str()),
                "case {case:?}: member {index} must be {id}, got {member}"
            );
            assert_eq!(
                member["Email"].as_str(),
                email.as_deref(),
                "case {case:?}: member {id} Email must be {email:?}, got {member}"
            );
            assert_eq!(
                member["MemberStatus"].as_str(),
                Some(STATUS_CREATED),
                "case {case:?}: member {id} must be {STATUS_CREATED}, got {member}"
            );
            assert_eq!(
                member["AdministratorId"].as_str(),
                Some(ADMINISTRATOR_ID),
                "case {case:?}: member {id} AdministratorId must be the caller, got {member}"
            );
            assert_eq!(
                member["MasterId"].as_str(),
                Some(ADMINISTRATOR_ID),
                "case {case:?}: member {id} MasterId must mirror AdministratorId, got {member}"
            );
            assert!(
                instant_seconds(&member["UpdatedAt"]).is_some_and(|seconds| seconds > 0.0),
                "case {case:?}: member {id} must carry an UpdatedAt instant, got {member}"
            );
            assert!(
                member["InvitedAt"].is_null(),
                "case {case:?}: member {id} was never invited, so InvitedAt must be absent, \
                 got {member}"
            );
        }
    }
}

/// RULE: re-creating an account that is already a member of the calling hub is
/// idempotent — it takes over the details the call carries and leaves the association
/// in the status it already had, rather than failing or duplicating it.
#[tokio::test]
async fn re_creating_a_member_updates_it_in_place() {
    let harness = Harness::enabled().await;
    let id = "777777777777";

    // Each row is one more CreateMembers call for the same account, with the Email it
    // carries and the Email the association must report afterwards.
    let rows = [
        ("first create, with an email", Some("first@example.com"), Some("first@example.com")),
        ("re-create without an email keeps the stored one", None, Some("first@example.com")),
        ("re-create with a new email replaces it", Some("second@example.com"), Some("second@example.com")),
    ];

    let mut previous_updated_at = 0.0_f64;
    for (case, submitted, expected_email) in rows {
        let (status, body) = harness
            .create_members(REGION, json!([detail(id, submitted)]))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateMembers expected 200, got {status} with {body}"
        );
        assert!(
            unprocessed_of(&body).is_empty(),
            "case {case:?}: the account must be processed, got {body}"
        );

        let (status, body) = harness.get_members(REGION, &[id]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetMembers expected 200, got {status} with {body}"
        );
        let members = members_of(&body);
        assert_eq!(
            members.len(),
            1,
            "case {case:?}: the account must be a member exactly once, got {body}"
        );
        assert_eq!(
            members[0]["Email"].as_str(),
            expected_email,
            "case {case:?}: expected Email {expected_email:?}, got {}",
            members[0]
        );
        assert_eq!(
            members[0]["MemberStatus"].as_str(),
            Some(STATUS_CREATED),
            "case {case:?}: expected {STATUS_CREATED}, got {}",
            members[0]
        );
        let updated_at = instant_seconds(&members[0]["UpdatedAt"]).unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected an UpdatedAt instant, got {}",
                members[0]
            )
        });
        assert!(
            updated_at >= previous_updated_at,
            "case {case:?}: UpdatedAt must not go backwards, got {updated_at} after \
             {previous_updated_at}"
        );
        previous_updated_at = updated_at;
    }
}

/// RULE: an account the call cannot associate is reported per account in
/// `UnprocessedAccounts` with a reason, and the accounts in the SAME call that can be
/// associated are still created. The administrator account is one such account: it is
/// the other end of the association, so it cannot be its own member. A later
/// `GetMembers` for it answers nothing at all: it is a report of stored associations,
/// and the hub holds none for that id.
#[tokio::test]
async fn the_administrator_account_is_reported_unprocessed() {
    let harness = Harness::enabled().await;
    let member = "888888888888";
    let (status, body) = harness
        .create_members(
            REGION,
            json!([
                detail(ADMINISTRATOR_ID, Some("admin@example.com")),
                detail(member, None),
            ]),
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateMembers expected 200, got {status} with {body}"
    );
    let unprocessed = unprocessed_of(&body);
    assert_eq!(
        unprocessed.len(),
        1,
        "only the administrator account must be unprocessed, got {body}"
    );
    assert_eq!(
        unprocessed[0]["AccountId"].as_str(),
        Some(ADMINISTRATOR_ID),
        "expected the administrator account to be unprocessed, got {}",
        unprocessed[0]
    );
    assert!(
        unprocessed[0]["ProcessingResult"]
            .as_str()
            .is_some_and(|reason| !reason.is_empty()),
        "an unprocessed account must say why, got {}",
        unprocessed[0]
    );

    // The administrator never became a member, and the other account in the same
    // call did.
    let (status, body) = harness.get_members(REGION, &[ADMINISTRATOR_ID, member]).await;
    assert_eq!(
        status, 200,
        "GetMembers expected 200, got {status} with {body}"
    );
    let members = members_of(&body);
    assert_eq!(
        members.len(),
        1,
        "only {member} must be a member, got {body}"
    );
    assert_eq!(
        members[0]["AccountId"].as_str(),
        Some(member),
        "expected {member}, got {}",
        members[0]
    );
    assert!(
        unprocessed_of(&body).is_empty(),
        "the hub holds no association for the administrator account, so GetMembers must \
         report it in neither list, got {body}"
    );
}

/// RULE: an account id that is not a 12-digit Amazon Web Services account id is an
/// invalid input parameter — `InvalidInputException` (400) — and the request is
/// refused whole, so none of the accounts it carries is created.
#[tokio::test]
async fn malformed_account_ids_are_refused() {
    // One row per way an id can miss the 12-digit form, plus the published bounds
    // (11 and 13 digits).
    let cases = [
        ("empty", ""),
        ("too short", "123"),
        ("one digit short", "11111111111"),
        ("one digit long", "1111111111111"),
        ("not all digits", "00000000000a"),
        ("padded with a space", " 00000000000"),
        ("hyphenated", "0000-0000-000"),
    ];
    let valid = "999999999999";

    for (case, bad) in cases {
        let harness = Harness::enabled().await;
        let (status, body) = harness
            .create_members(REGION, json!([detail(valid, None), detail(bad, None)]))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?} ({bad:?}): CreateMembers expected 400, got {status} with {body}"
        );

        let (status, body) = harness.get_members(REGION, &[valid]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetMembers expected 200, got {status} with {body}"
        );
        assert!(
            members_of(&body).is_empty(),
            "case {case:?}: a refused request must persist nothing, got {body}"
        );
    }

    // `GetMembers` applies the same rule to the ids it is asked about. The empty id
    // is left out of this half: `AccountIdList`'s member is a `NonEmptyString`
    // (pattern `\S`), so the framework's boundary validation refuses it before the
    // handler runs and answers `ValidationException` rather than this rule's
    // `InvalidInputException`.
    let harness = Harness::enabled().await;
    for (case, bad) in cases.iter().filter(|(_, bad)| !bad.trim().is_empty()) {
        let (status, body) = harness.get_members(REGION, &[bad]).await;
        assert_eq!(
            status, 400,
            "case {case:?} ({bad:?}): GetMembers expected 400, got {status} with {body}"
        );
    }
}

/// RULE: the bounds a member operation enforces are refused AT the bound and accepted
/// below it: an `Email` is kept up to the longest address SMTP allows (RFC 5321's
/// 254-character forward path) and refused past it, a `GetMembers` call addresses at
/// most the batch the model bounds `CreateMembers`' own list to (50) and is refused
/// past it, and both refusals persist nothing.
#[tokio::test]
async fn member_input_bounds_are_enforced_at_their_edges() {
    // The published bound, one below it and one past it. The address keeps a valid
    // local@domain form at every length, so only its LENGTH differs between rows.
    const MAX_EMAIL_LEN: usize = 254;
    let email_of = |len: usize| {
        let suffix = "@example.com";
        format!("{}{suffix}", "a".repeat(len - suffix.len()))
    };
    let email_rows = [
        ("one below the maximum email length", MAX_EMAIL_LEN - 1, 200),
        ("exactly the maximum email length", MAX_EMAIL_LEN, 200),
        ("one past the maximum email length", MAX_EMAIL_LEN + 1, 400),
    ];

    for (case, len, expected) in email_rows {
        let harness = Harness::enabled().await;
        let member = "141414141414";
        let email = email_of(len);
        assert_eq!(email.chars().count(), len, "case {case:?}: test built a {} character address", email.chars().count());
        let (status, body) = harness
            .create_members(REGION, json!([detail(member, Some(&email))]))
            .await;
        assert_eq!(
            status, expected,
            "case {case:?}: CreateMembers expected {expected}, got {status} with {body}"
        );

        let (status, body) = harness.get_members(REGION, &[member]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetMembers expected 200, got {status} with {body}"
        );
        assert_eq!(
            members_of(&body).len(),
            usize::from(expected == 200),
            "case {case:?}: only an accepted Email may be persisted, got {body}"
        );
    }

    // `CreateMembers`' own list is bounded by the model (`AccountDetailsList`, len
    // 1..50); `GetMembers`' `AccountIds` carries no such trait, so the handler holds
    // the same bound. Rows: the bound, and one past it.
    const MAX_BATCH: usize = 50;
    let harness = Harness::enabled().await;
    for (case, count, expected) in [
        ("the maximum batch", MAX_BATCH, 200),
        ("one past the maximum batch", MAX_BATCH + 1, 400),
    ] {
        let ids: Vec<String> = (0..count).map(account_id).collect();
        let requested: Vec<&str> = ids.iter().map(String::as_str).collect();
        let (status, body) = harness.get_members(REGION, &requested).await;
        assert_eq!(
            status, expected,
            "case {case:?} ({count} ids): GetMembers expected {expected}, got {status} with {body}"
        );
    }
}

/// RULE: a hub holds at most the published maximum of member accounts; a create that
/// would take it past that is the modelled `LimitExceededException` (429), and
/// re-creating accounts the hub already holds stays inside the bound.
#[tokio::test]
async fn the_member_account_maximum_is_enforced() {
    // The AWS Security Hub quotas table publishes 1,000 member accounts per
    // administrator account.
    const MAX_MEMBERS: usize = 1_000;
    let harness = Harness::enabled().await;

    // Fill the hub in calls of the modelled maximum batch size.
    for chunk_start in (0..MAX_MEMBERS).step_by(50) {
        let details: Vec<Value> = (chunk_start..chunk_start + 50)
            .map(|n| detail(&account_id(n), None))
            .collect();
        let (status, body) = harness.create_members(REGION, json!(details)).await;
        assert_eq!(
            status, 200,
            "filling at {chunk_start}: CreateMembers expected 200, got {status} with {body}"
        );
    }

    // Re-creating accounts the hub already holds adds nothing, so it is still inside
    // the bound.
    let held = account_id(0);
    let (status, body) = harness
        .create_members(REGION, json!([detail(&held, Some("again@example.com"))]))
        .await;
    assert_eq!(
        status, 200,
        "re-creating an existing member expected 200, got {status} with {body}"
    );

    // One more NEW account is past the bound.
    let one_too_many = account_id(MAX_MEMBERS);
    let (status, body) = harness
        .create_members(REGION, json!([detail(&one_too_many, None)]))
        .await;
    assert_eq!(
        status, 429,
        "the {}th member expected 429, got {status} with {body}",
        MAX_MEMBERS + 1
    );
    let (status, body) = harness.get_members(REGION, &[&one_too_many]).await;
    assert_eq!(
        status, 200,
        "GetMembers expected 200, got {status} with {body}"
    );
    assert!(
        members_of(&body).is_empty(),
        "a refused create must persist nothing, got {body}"
    );
}

/// RULE: the member operations act on the calling account's hub, so they are refused
/// with `InvalidAccessException` (401) while that account is not subscribed in the
/// Region of the call — "This action can only be used by an account that has Security
/// Hub CSPM enabled".
#[tokio::test]
async fn member_operations_need_a_subscribed_account() {
    let harness = Harness::new();
    let member = "121212121212";

    let (status, body) = harness
        .create_members(REGION, json!([detail(member, None)]))
        .await;
    assert_eq!(
        status, 401,
        "CreateMembers before EnableSecurityHub: expected 401, got {status} with {body}"
    );
    let (status, body) = harness.get_members(REGION, &[member]).await;
    assert_eq!(
        status, 401,
        "GetMembers before EnableSecurityHub: expected 401, got {status} with {body}"
    );

    // Subscribing in one Region does not subscribe the account in another.
    let (status, body) = harness.enable(REGION).await;
    assert_eq!(status, 200, "EnableSecurityHub: got {status} with {body}");
    let (status, body) = harness
        .create_members(OTHER_REGION, json!([detail(member, None)]))
        .await;
    assert_eq!(
        status, 401,
        "CreateMembers in an unsubscribed Region: expected 401, got {status} with {body}"
    );
}

/// RULE: a member association belongs to one hub — the administrator account AND the
/// Region it was created in — so the same account id is a member in that Region only.
#[tokio::test]
async fn members_are_scoped_to_the_hub_that_created_them() {
    let harness = Harness::new();
    for region in [REGION, OTHER_REGION] {
        let (status, body) = harness.enable(region).await;
        assert_eq!(
            status, 200,
            "EnableSecurityHub in {region}: got {status} with {body}"
        );
    }

    let member = "131313131313";
    let (status, body) = harness
        .create_members(REGION, json!([detail(member, Some("east@example.com"))]))
        .await;
    assert_eq!(
        status, 200,
        "CreateMembers in {REGION}: got {status} with {body}"
    );

    for (region, expect_member) in [(REGION, true), (OTHER_REGION, false)] {
        let (status, body) = harness.get_members(region, &[member]).await;
        assert_eq!(
            status, 200,
            "GetMembers in {region}: expected 200, got {status} with {body}"
        );
        assert_eq!(
            members_of(&body).len(),
            usize::from(expect_member),
            "GetMembers in {region}: expected member present = {expect_member}, got {body}"
        );
        assert!(
            unprocessed_of(&body).is_empty(),
            "GetMembers in {region}: an id that is not a member there is reported in \
             neither list, got {body}"
        );
    }
}
