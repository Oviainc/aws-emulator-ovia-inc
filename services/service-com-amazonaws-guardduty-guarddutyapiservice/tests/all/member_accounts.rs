// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListMembers`: the paged report over the member accounts
//! `CreateMembers` stored under an administrator detector.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, reusing
//! the harness in [`crate::detector_lifecycle`] and the member helpers in
//! [`crate::member_association`], so routing, restJson1 (de)serialization,
//! modelled-constraint validation and the handlers are exercised together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. `ListMembers` reports exactly the member accounts stored under the detector the
//!    request names, in account-id order, each carrying what the create stored — and
//!    an empty list, not an absent one, when no create has run.
//! 2. Paging the list with any `maxResults` yields each member exactly once, in the
//!    same order, whatever the page size; a `nextToken` this service never issued is
//!    refused naming that parameter.
//! 3. `onlyAssociated` is the boolean its member means, in any case, and narrows the
//!    list to the members that are ASSOCIATED with the administrator: a disassociated
//!    member is left out of an associated-only list and reported by a list of all
//!    members. A value that is not a boolean is refused.
//! 4. A detector id this account and Region do not hold is refused with HTTP 400 under
//!    the `InvalidInputException` class, never answered with a hollow 200.
//! 5. The report is a read: two reads in a row report the same members, and reading
//!    changes nothing a later caller sees.

use serde_json::{json, Value};

use crate::client_rejections::{invalid_parameter, not_owned, page_token_refusal};
use crate::detector_lifecycle::{
    call, call_in, enabled_detector, error_code, error_shape, new_router, Router,
    ABSENT_DETECTOR_ID,
};
use crate::member_association::{create_members, disassociate_members, invite_members};

/// The error class every GuardDuty client rejection is named by on the wire.
const CLASS: &str = "InvalidInputException";

/// The modelled error SHAPE that class is enveloped in, named by `x-amzn-errortype`.
const ENVELOPE: &str = "BadRequestException";

/// A value that cannot be a detector id at all.
const MALFORMED_DETECTOR_ID: &str = "not-a-detector-id";

/// The `RelationshipStatus` a member `CreateMembers` recorded and nothing has invited
/// carries, as `crate::state` holds it.
const CREATED: &str = service_com_amazonaws_guardduty_guarddutyapiservice::state::RELATIONSHIP_CREATED;

/// The `RelationshipStatus` a disassociated member carries.
const REMOVED: &str = service_com_amazonaws_guardduty_guarddutyapiservice::state::RELATIONSHIP_REMOVED;

/// The administrator account every member recorded here belongs to: the account this
/// emulator serves.
fn administrator() -> String {
    engine::emulator_account_id().to_string()
}

/// `ListMembers`, with `query` appended verbatim (`""` for no query parameters).
async fn list_members(router: &Router, detector_id: &str, query: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/detector/{detector_id}/member{query}"),
        None,
    )
    .await
}

/// The `accountId`s one `ListMembers` response reports, in the order it reported them.
fn reported_ids(case: &str, body: &Value) -> Vec<String> {
    body["members"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a members list, got {body:?}"))
        .iter()
        .map(|member| {
            member["accountId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected an accountId, got {member:?}"))
                .to_string()
        })
        .collect()
}

/// The member the list reports for `account_id`, or `None`.
fn reported_member(body: &Value, account_id: &str) -> Option<Value> {
    body["members"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|member| member["accountId"].as_str() == Some(account_id))
        .cloned()
}

/// Assert one rejection: HTTP 400, the error class in the body's `__type` and `type`,
/// the envelope shape in `x-amzn-errortype`, and the classified message.
fn assert_rejected(case: &str, status: u16, body: &Value, expected_message: &str) {
    assert_eq!(
        status, 400,
        "case {case}: expected HTTP 400, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        CLASS,
        "case {case}: expected the error class {CLASS}, got {body:?}"
    );
    assert_eq!(
        body["type"].as_str(),
        Some(CLASS),
        "case {case}: expected the error class in the type member, got {body:?}"
    );
    assert_eq!(
        error_shape(body),
        ENVELOPE,
        "case {case}: expected the envelope shape {ENVELOPE}, got {body:?}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(expected_message),
        "case {case}: expected the classified message, got {body:?}"
    );
}

/// Member accounts spanning what the report generalises over: the digits of an
/// account id (the low and the high end of the space, a mid-range id, ids one digit
/// apart) and the form of the email the create recorded. The rows are deliberately
/// NOT in account-id order, so the order the list reports cannot come from the
/// request.
const MEMBER_ROWS: [(&str, &str); 5] = [
    ("500000000003", "m+tag@example.io"),
    ("999999999999", "high.end@example.com"),
    ("500000000001", "a@b.co"),
    ("000000000001", "low.end@sub.example-corp.com"),
    ("500000000002", "first.last@example.net"),
];

/// Create an enabled detector, record [`MEMBER_ROWS`] against it and return its id.
async fn detector_with_members(router: &Router) -> String {
    let detector_id = enabled_detector(router).await;
    let (status, body) = create_members(router, &detector_id, &MEMBER_ROWS).await;
    assert_eq!(status, 200, "create-members: got {status} {body:?}");
    assert_eq!(
        body["unprocessedAccounts"],
        json!([]),
        "create-members: expected every account processed, got {body:?}"
    );
    detector_id
}

/// [`MEMBER_ROWS`]' account ids in the order a list must report them.
fn expected_order() -> Vec<String> {
    let mut ids: Vec<String> = MEMBER_ROWS
        .iter()
        .map(|(account_id, _)| (*account_id).to_string())
        .collect();
    ids.sort();
    ids
}

// ---------------------------------------------------------------------------
// Rules 1 + 5: create -> list reports what was stored, and reading changes nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_list_reports_the_member_accounts_a_create_stored() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let administrator = administrator();

    // Before any create the list is empty — present and empty, not absent and not
    // canned.
    let (status, body) = list_members(&router, &detector_id, "").await;
    assert_eq!(status, 200, "list on a fresh detector: got {status} {body:?}");
    assert_eq!(
        body["members"],
        json!([]),
        "list on a fresh detector: expected [], got {body:?}"
    );

    let (status, body) = create_members(&router, &detector_id, &MEMBER_ROWS).await;
    assert_eq!(status, 200, "create-members: got {status} {body:?}");

    let (status, listed) = list_members(&router, &detector_id, "").await;
    assert_eq!(status, 200, "list after create: got {status} {listed:?}");
    assert_eq!(
        reported_ids("every member", &listed),
        expected_order(),
        "list after create: expected account-id order, got {listed:?}"
    );

    for (account_id, email) in MEMBER_ROWS {
        let member = reported_member(&listed, account_id)
            .unwrap_or_else(|| panic!("case {account_id}: expected a member, got {listed:?}"));
        assert_eq!(
            member["email"].as_str(),
            Some(email),
            "case {account_id}: expected the recorded email, got {member:?}"
        );
        assert_eq!(
            member["relationshipStatus"].as_str(),
            Some(CREATED),
            "case {account_id}: a recorded, un-invited member, got {member:?}"
        );
        assert_eq!(
            member["masterId"].as_str(),
            Some(administrator.as_str()),
            "case {account_id}: expected the administrator account, got {member:?}"
        );
        assert_eq!(
            member["administratorId"].as_str(),
            Some(administrator.as_str()),
            "case {account_id}: expected the administrator account, got {member:?}"
        );
        // The member account's own detector is a resource in another account that
        // this emulator does not hold, and no invitation is modelled yet, so neither
        // optional member is invented.
        assert!(
            member.get("detectorId").is_none(),
            "case {account_id}: expected no member detectorId, got {member:?}"
        );
        assert!(
            member.get("invitedAt").is_none(),
            "case {account_id}: expected no invitedAt, got {member:?}"
        );
        let updated_at = member["updatedAt"]
            .as_str()
            .unwrap_or_else(|| panic!("case {account_id}: expected an updatedAt, got {member:?}"));
        assert!(
            updated_at.len() == 24 && updated_at.contains('T') && updated_at.ends_with('Z'),
            "case {account_id}: expected an ISO-8601 millisecond instant, got {updated_at:?}"
        );
    }

    // Rule 5: a read does not change what a later caller sees.
    let (_, again) = list_members(&router, &detector_id, "").await;
    assert_eq!(
        again, listed,
        "two reads in a row must report the same members, got {again:?}"
    );

    // And the members are reported only through the detector that administers them:
    // `ListDetectors` still reports exactly the one detector this account created.
    let (status, detectors) = call(&router, "GET", "/detector", None).await;
    assert_eq!(status, 200, "list-detectors: got {status} {detectors:?}");
    assert_eq!(
        detectors["detectorIds"],
        json!([detector_id.clone()]),
        "recording a member creates no detector of this account, got {detectors:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: paging.
// ---------------------------------------------------------------------------

/// Every member id `ListMembers` reports for `detector_id`, read a page of
/// `page_size` at a time through the tokens it hands back.
async fn all_member_ids(router: &Router, detector_id: &str, page_size: i64) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..64 {
        let query = match &token {
            Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
            None => format!("?maxResults={page_size}"),
        };
        let (status, body) = list_members(router, detector_id, &query).await;
        assert_eq!(
            status, 200,
            "case page size {page_size}: got {status} {body:?}"
        );
        let page = reported_ids(&format!("page size {page_size}"), &body);
        assert!(
            page.len() <= page_size as usize,
            "case page size {page_size}: a page carries at most maxResults items, got {page:?}"
        );
        ids.extend(page);
        match body["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return ids,
        }
    }
    panic!("case page size {page_size}: paging did not terminate, got {ids:?}");
}

#[tokio::test]
async fn paging_the_member_list_yields_each_member_exactly_once() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;
    let expected = expected_order();

    // 1 is the single-item page, 5 the exact fit, 6 one past the end and 50 the
    // modelled ceiling on `MaxResults` (range 1..50 —
    // `smithy_query(operation="ListMembers")`).
    for page_size in [1_i64, 2, 3, 4, 5, 6, 50] {
        let seen = all_member_ids(&router, &detector_id, page_size).await;
        assert_eq!(
            seen, expected,
            "case page size {page_size}: expected every member exactly once in order, got {seen:?}"
        );
    }

    // A token this service never issued is refused with the ONE sentence the whole
    // pagination surface answers: it names no member and quotes no token back.
    for (case, token) in [
        ("not a decimal offset at all", "banana"),
        ("past the end of the list", "99"),
        ("a negative offset", "-1"),
    ] {
        let (status, body) =
            list_members(&router, &detector_id, &format!("?nextToken={token}")).await;
        assert_rejected(case, status, &body, &page_token_refusal());
    }
}

// ---------------------------------------------------------------------------
// Rule 3: onlyAssociated.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn only_associated_narrows_the_list_to_the_associated_members() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;
    let disassociated = MEMBER_ROWS[0].0;
    // Left merely RECORDED: "members who haven't been invited yet" are not associated
    // members either
    // (`smithy_query(query="doc",shape="ListMembersRequest",member="OnlyAssociated")`).
    let recorded_only = MEMBER_ROWS[1].0;

    // The invitation is what associates a recorded account with its administrator.
    let invited: Vec<&str> = MEMBER_ROWS
        .iter()
        .map(|(account_id, _)| *account_id)
        .filter(|account_id| *account_id != recorded_only)
        .collect();
    let (status, body) = invite_members(&router, &detector_id, &invited).await;
    assert_eq!(status, 200, "invite-members: got {status} {body:?}");

    // Disassociating moves the RELATIONSHIP and retains the record, so the member is
    // still one of the administrator's members while no longer being associated with
    // it — which is exactly the distinction `onlyAssociated` exposes.
    let (status, body) = disassociate_members(&router, &detector_id, &[disassociated]).await;
    assert_eq!(status, 200, "disassociate-members: got {status} {body:?}");

    let all = expected_order();
    let associated: Vec<String> = all
        .iter()
        .filter(|account_id| {
            account_id.as_str() != disassociated && account_id.as_str() != recorded_only
        })
        .cloned()
        .collect();

    // The spellings a boolean takes on the wire, plus the absent and empty parameter:
    // "specifies whether to only return associated members or to return all members"
    // (`smithy_query(query="doc",shape="ListMembersRequest",member="OnlyAssociated")`),
    // so a caller that does not ask for narrowing gets every member.
    for (case, query, expected) in [
        ("absent", "", all.clone()),
        ("empty", "?onlyAssociated=", all.clone()),
        ("false", "?onlyAssociated=false", all.clone()),
        ("FALSE, upper case", "?onlyAssociated=FALSE", all.clone()),
        ("False, mixed case", "?onlyAssociated=False", all.clone()),
        ("true", "?onlyAssociated=true", associated.clone()),
        ("TRUE, upper case", "?onlyAssociated=TRUE", associated.clone()),
        ("True, mixed case", "?onlyAssociated=True", associated.clone()),
    ] {
        let (status, body) = list_members(&router, &detector_id, query).await;
        assert_eq!(
            status, 200,
            "case onlyAssociated {case}: got {status} {body:?}"
        );
        assert_eq!(
            reported_ids(case, &body),
            expected,
            "case onlyAssociated {case}: expected {expected:?}, got {body:?}"
        );
    }

    // The disassociated member is reported with the relationship the disassociation
    // left it in, not dropped from the full list.
    let (_, body) = list_members(&router, &detector_id, "").await;
    let member = reported_member(&body, disassociated)
        .unwrap_or_else(|| panic!("expected the disassociated member, got {body:?}"));
    assert_eq!(
        member["relationshipStatus"].as_str(),
        Some(REMOVED),
        "a disassociated member keeps its record, got {member:?}"
    );

    // Re-creating it records it afresh and a fresh invitation associates it again, so
    // an associated-only list reports it.
    let (status, body) =
        create_members(&router, &detector_id, &[(disassociated, MEMBER_ROWS[0].1)]).await;
    assert_eq!(status, 200, "re-create: got {status} {body:?}");
    let (status, body) = invite_members(&router, &detector_id, &[disassociated]).await;
    assert_eq!(status, 200, "re-invite: got {status} {body:?}");
    let (_, body) = list_members(&router, &detector_id, "?onlyAssociated=true").await;
    let mut expected = associated.clone();
    expected.push(disassociated.to_string());
    expected.sort();
    assert_eq!(
        reported_ids("after the re-create", &body),
        expected,
        "a re-invited member is associated again, got {body:?}"
    );

    for (case, value) in [
        ("a word that is not a boolean", "maybe"),
        ("a number", "1"),
        ("a boolean with trailing text", "true-ish"),
        ("a boolean with surrounding whitespace", "%20true"),
    ] {
        let (status, body) =
            list_members(&router, &detector_id, &format!("?onlyAssociated={value}")).await;
        assert_rejected(case, status, &body, &invalid_parameter("onlyAssociated"));
    }
}

// ---------------------------------------------------------------------------
// Rule 4: a detector id the account and Region do not hold.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_list_refuses_a_detector_the_account_does_not_hold() {
    let router = new_router();
    let detector_id = detector_with_members(&router).await;

    for (case, candidate, expected) in [
        (
            "a well-formed id no create minted",
            ABSENT_DETECTOR_ID,
            not_owned("detectorId"),
        ),
        (
            "a value that cannot be a detector id",
            MALFORMED_DETECTOR_ID,
            invalid_parameter("detectorId"),
        ),
    ] {
        let (status, body) = list_members(&router, candidate, "").await;
        assert_rejected(case, status, &body, &expected);
    }

    // A detector is regional: its own id is not served by another Region's endpoint,
    // so the members under it are not reachable there either.
    let (status, body) = call_in(
        &router,
        "eu-west-1",
        "GET",
        &format!("/detector/{detector_id}/member"),
        None,
    )
    .await;
    assert_rejected(
        "another Region's endpoint",
        status,
        &body,
        &not_owned("detectorId"),
    );

    // None of the refusals above changed what the detector holds.
    let (_, listed) = list_members(&router, &detector_id, "").await;
    assert_eq!(
        reported_ids("after the refusals", &listed),
        expected_order(),
        "a refused request changes no member, got {listed:?}"
    );
}
