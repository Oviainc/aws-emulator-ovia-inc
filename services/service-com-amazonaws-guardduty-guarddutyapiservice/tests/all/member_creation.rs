// SPDX-License-Identifier: CC0-1.0

//! `CreateMembers` as a BATCH write: what one call does to the member store of the
//! detector it names, read back through `GetMembers`.
//!
//! Every request goes over the wire through `MultiServiceRouter::dispatch`, the same
//! path the production router uses, so these tests exercise routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers together. The
//! create/read/disassociate loop, the detector-ownership refusals and the documented
//! email rules are covered by [`crate::member_association`]; this suite covers what a
//! BATCH of `AccountDetails` does, which is the part a single recorded call does not
//! show.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. One call records every DISTINCT `AccountDetails` entry it carries, each with the
//!    account details that entry carries, whatever the batch's SIZE inside the
//!    modelled `AccountDetails` length range — and a batch outside that range is
//!    refused with HTTP 400 under the modelled client error, recording none of it.
//! 2. Recording is idempotent in the account details: re-recording an account with the
//!    details it already carries leaves the record exactly as it was (its `updatedAt`
//!    does not move), while re-recording it with different details records them and
//!    moves `updatedAt`. One account is one member, so a repeated entry inside ONE
//!    call leaves exactly one record.
//! 3. An entry the administrator cannot have as a member is a PER-ACCOUNT failure: it
//!    is named in `unprocessedAccounts` with the reason and persists nothing, while
//!    every other entry of the SAME call is recorded and the call answers HTTP 200 —
//!    at any position in the batch.

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    call, enabled_detector, error_code, error_shape, new_router, Router,
};
use crate::member_association::{create_members, get_members};

/// The modelled error SHAPE and CODE a refused request is named by: the only client
/// error GuardDuty's model binds to an operation
/// (`smithy_query(operation="CreateMembers")` reports `BadRequestException` and
/// `InternalServerErrorException` and nothing else).
const MODELED_CLIENT_ERROR: &str = "BadRequestException";

/// The error class a refusal carries in the body, as this service classifies a client
/// failure.
const CLASS: &str = "InvalidInputException";

/// The published bounds on `AccountDetails`: `len 1..50`
/// (`smithy_query(operation="CreateMembers")`).
const ACCOUNT_DETAILS_MIN: usize = 1;
const ACCOUNT_DETAILS_MAX: usize = 50;

/// The `RelationshipStatus` a recorded, un-invited member carries, read from the
/// service so the test cannot drift from it.
const CREATED: &str =
    service_com_amazonaws_guardduty_guarddutyapiservice::state::RELATIONSHIP_CREATED;

/// The reason the administrator's own account id is reported with, read from the
/// service for the same reason.
const SELF_AS_MEMBER: &str =
    service_com_amazonaws_guardduty_guarddutyapiservice::state::SELF_AS_MEMBER_MESSAGE;

/// Email forms a row can carry, spanning the shapes `AccountDetail.Email` accepts
/// (`smithy_query(query="doc",shape="AccountDetail",member="Email")`): the shortest
/// accepted address, an ordinary one, a plus tag, and dots plus a hyphen in the
/// domain.
const EMAIL_FORMS: [&str; 4] = [
    "a@b.co",
    "member@example.com",
    "m+tag@example.io",
    "first.last@sub.example-corp.com",
];

/// The administrator account every member recorded here belongs to: the account this
/// emulator serves.
fn administrator() -> String {
    ::engine::emulator_account_id().to_string()
}

/// A 12-digit account id in `case`'s own namespace, so one case cannot collide with
/// another's members. The leading `7` keeps it clear of the administrator's own id.
fn account_id(case: usize, index: usize) -> String {
    format!("7{case:02}{index:09}")
}

/// The email row `index` carries, cycling [`EMAIL_FORMS`] so one batch spans them.
fn email_for(index: usize) -> &'static str {
    EMAIL_FORMS[index % EMAIL_FORMS.len()]
}

/// `CreateMembers` with `account_details` passed verbatim, so a row can send a batch
/// the typed helper could not build (an empty list, a list past the modelled
/// maximum).
async fn create_members_raw(router: &Router, detector_id: &str, account_details: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/member"),
        Some(json!({"accountDetails": account_details})),
    )
    .await
}

/// The `members` entry for `account_id`, if the response carries one.
fn member_of(body: &Value, account_id: &str) -> Option<Value> {
    body["members"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|member| member["accountId"].as_str() == Some(account_id))
        .cloned()
}

/// The account ids in `unprocessedAccounts`, in the order the response lists them.
fn unprocessed_ids(body: &Value) -> Vec<String> {
    body["unprocessedAccounts"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|entry| entry["accountId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The `result` reported for `account_id` in `unprocessedAccounts`, if any.
fn unprocessed_result(body: &Value, account_id: &str) -> Option<String> {
    body["unprocessedAccounts"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|entry| entry["accountId"].as_str() == Some(account_id))
        .map(|entry| entry["result"].as_str().unwrap_or_default().to_string())
}

/// The `updatedAt` the detector reports for `account_id`, read through `GetMembers`.
async fn updated_at(case: &str, router: &Router, detector_id: &str, account_id: &str) -> String {
    let (status, read) = get_members(router, detector_id, &[account_id]).await;
    assert_eq!(
        status, 200,
        "case {case}: get-members expected 200, got {status} {read:?}"
    );
    let member = member_of(&read, account_id)
        .unwrap_or_else(|| panic!("case {case}: expected a member, got {read:?}"));
    member["updatedAt"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected an updatedAt, got {member:?}"))
        .to_string()
}

// ---------------------------------------------------------------------------
// Rule 1: the whole batch is recorded, whatever its size.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn one_call_records_every_entry_of_the_batch() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let administrator = administrator();

    // The published minimum, the sizes between, the published maximum and one below
    // it: `AccountDetails` is `len 1..50`.
    for (case, size) in [ACCOUNT_DETAILS_MIN, 2, 7, 49, ACCOUNT_DETAILS_MAX]
        .into_iter()
        .enumerate()
    {
        let ids: Vec<String> = (0..size).map(|index| account_id(case, index)).collect();
        let rows: Vec<(&str, &str)> = ids
            .iter()
            .enumerate()
            .map(|(index, id)| (id.as_str(), email_for(index)))
            .collect();

        let (status, body) = create_members(&router, &detector_id, &rows).await;
        assert_eq!(
            status, 200,
            "case batch of {size}: expected HTTP 200, got {status} {body:?}"
        );
        assert_eq!(
            unprocessed_ids(&body),
            Vec::<String>::new(),
            "case batch of {size}: expected every entry processed, got {body:?}"
        );

        // Read the whole batch back in ONE GetMembers, so the records and not just
        // the response are what is asserted.
        let requested: Vec<&str> = ids.iter().map(String::as_str).collect();
        let (status, read) = get_members(&router, &detector_id, &requested).await;
        assert_eq!(
            status, 200,
            "case batch of {size}: get-members expected HTTP 200, got {status} {read:?}"
        );
        assert_eq!(
            unprocessed_ids(&read),
            Vec::<String>::new(),
            "case batch of {size}: expected every account to be a member, got {read:?}"
        );
        for (index, (id, email)) in rows.iter().enumerate() {
            let member = member_of(&read, id).unwrap_or_else(|| {
                panic!("case batch of {size} entry {index}: expected a member, got {read:?}")
            });
            assert_eq!(
                member["email"].as_str(),
                Some(*email),
                "case batch of {size} entry {index}: expected the recorded email, got {member:?}"
            );
            assert_eq!(
                member["relationshipStatus"].as_str(),
                Some(CREATED),
                "case batch of {size} entry {index}: expected a recorded member, got {member:?}"
            );
            assert_eq!(
                member["administratorId"].as_str(),
                Some(administrator.as_str()),
                "case batch of {size} entry {index}: expected the administrator account, got {member:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_batch_outside_the_published_length_range_is_refused() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    let past_the_maximum: Vec<Value> = (0..=ACCOUNT_DETAILS_MAX)
        .map(|index| json!({"accountId": account_id(90, index), "email": email_for(index)}))
        .collect();
    let far_past_the_maximum: Vec<Value> = (0..200)
        .map(|index| json!({"accountId": account_id(91, index), "email": email_for(index)}))
        .collect();

    for (case, details) in [
        ("an empty list: one below the published minimum of 1", json!([])),
        (
            "51 entries: one above the published maximum of 50",
            json!(past_the_maximum),
        ),
        (
            "200 entries: far above the published maximum",
            json!(far_past_the_maximum),
        ),
    ] {
        let (status, body) = create_members_raw(&router, &detector_id, details).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_shape(&body),
            MODELED_CLIENT_ERROR,
            "case {case}: expected the modelled error shape, got {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("accountDetails"),
            "case {case}: expected the message to name accountDetails, got {message:?}"
        );
    }

    // A refused batch recorded none of its entries: the first id of each over-long
    // batch is still not a member.
    for case in [90_usize, 91] {
        let id = account_id(case, 0);
        let (_, read) = get_members(&router, &detector_id, &[id.as_str()]).await;
        assert!(
            member_of(&read, &id).is_none(),
            "case namespace {case}: a refused batch must record nothing, got {read:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: recording is idempotent in the account details.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn re_recording_the_same_details_changes_nothing() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    // One row per email form, so the invariant is asserted over the shapes an
    // `AccountDetail` takes rather than over one address.
    for (index, email) in EMAIL_FORMS.into_iter().enumerate() {
        let id = account_id(10, index);
        let case = format!("{id} with {email}");

        let (status, body) = create_members(&router, &detector_id, &[(id.as_str(), email)]).await;
        assert_eq!(
            status, 200,
            "case {case}: first create expected HTTP 200, got {status} {body:?}"
        );
        let first = updated_at(&case, &router, &detector_id, &id).await;

        // The same details again: a create that changes nothing moves nothing.
        let (status, body) = create_members(&router, &detector_id, &[(id.as_str(), email)]).await;
        assert_eq!(
            status, 200,
            "case {case}: repeated create expected HTTP 200, got {status} {body:?}"
        );
        assert_eq!(
            unprocessed_ids(&body),
            Vec::<String>::new(),
            "case {case}: a repeated create processes the account, got {body:?}"
        );
        let repeated = updated_at(&case, &router, &detector_id, &id).await;
        assert_eq!(
            repeated, first,
            "case {case}: expected updatedAt not to move on an unchanged create, got {repeated:?} after {first:?}"
        );

        // Different details: recorded, and the record's updatedAt moves with them.
        let changed_email = EMAIL_FORMS[(index + 1) % EMAIL_FORMS.len()];
        let (status, body) =
            create_members(&router, &detector_id, &[(id.as_str(), changed_email)]).await;
        assert_eq!(
            status, 200,
            "case {case}: changed create expected HTTP 200, got {status} {body:?}"
        );
        let (_, read) = get_members(&router, &detector_id, &[id.as_str()]).await;
        let member = member_of(&read, &id)
            .unwrap_or_else(|| panic!("case {case}: expected a member, got {read:?}"));
        assert_eq!(
            member["email"].as_str(),
            Some(changed_email),
            "case {case}: expected the re-recorded email {changed_email:?}, got {member:?}"
        );
        assert_ne!(
            member["updatedAt"].as_str(),
            Some(first.as_str()),
            "case {case}: expected updatedAt to move with the changed details, got {member:?}"
        );
    }
}

#[tokio::test]
async fn a_repeated_entry_in_one_call_leaves_exactly_one_member() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let repeated = account_id(11, 0);
    let other = account_id(11, 1);
    let email = EMAIL_FORMS[1];

    // The same account, at three positions of one batch, around a second account:
    // `AccountDetails` names the accounts to associate, and one account is one
    // member.
    let rows: Vec<(&str, &str)> = vec![
        (repeated.as_str(), email),
        (other.as_str(), EMAIL_FORMS[2]),
        (repeated.as_str(), email),
        (repeated.as_str(), email),
    ];
    let (status, body) = create_members(&router, &detector_id, &rows).await;
    assert_eq!(
        status, 200,
        "case a repeated entry: expected HTTP 200, got {status} {body:?}"
    );
    assert_eq!(
        unprocessed_ids(&body),
        Vec::<String>::new(),
        "case a repeated entry: expected every account processed, got {body:?}"
    );

    let (status, read) = get_members(&router, &detector_id, &[repeated.as_str(), other.as_str()]).await;
    assert_eq!(
        status, 200,
        "case a repeated entry: get-members expected HTTP 200, got {status} {read:?}"
    );
    let reported: Vec<String> = read["members"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|member| {
            member["accountId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    let occurrences = reported.iter().filter(|id| **id == repeated).count();
    assert_eq!(
        occurrences, 1,
        "case a repeated entry: expected exactly one member record, got {reported:?}"
    );
    let member = member_of(&read, &repeated)
        .unwrap_or_else(|| panic!("case a repeated entry: expected a member, got {read:?}"));
    assert_eq!(
        member["email"].as_str(),
        Some(email),
        "case a repeated entry: expected the recorded email, got {member:?}"
    );
    // The second account of the same batch is recorded too: deduplicating one entry
    // does not drop the rest.
    assert!(
        member_of(&read, &other).is_some(),
        "case a repeated entry: expected the other account recorded, got {read:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: a per-account failure is reported, and the rest of the batch recorded.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_account_that_cannot_be_a_member_is_reported_and_the_rest_recorded() {
    let router = new_router();
    let administrator = administrator();
    // One detector per account per Region, so every case shares it and works in its
    // own account-id namespace instead.
    let detector_id = enabled_detector(&router).await;

    // The administrator's own account id at the front, the middle and the back of a
    // batch, and twice in one batch: the position must not change the outcome.
    let positions: [(&str, usize); 4] = [
        ("first in the batch", 0),
        ("in the middle of the batch", 1),
        ("last in the batch", 2),
        ("twice in one batch", 3),
    ];

    for (case, position) in positions {
        let others: Vec<String> = (0..2).map(|index| account_id(20 + position, index)).collect();
        let mut rows: Vec<(&str, &str)> = others
            .iter()
            .enumerate()
            .map(|(index, id)| (id.as_str(), email_for(index)))
            .collect();
        let self_row = (administrator.as_str(), "admin@example.com");
        match position {
            0 => rows.insert(0, self_row),
            1 => rows.insert(1, self_row),
            3 => {
                rows.insert(0, self_row);
                rows.push(self_row);
            }
            _ => rows.push(self_row),
        }

        let (status, body) = create_members(&router, &detector_id, &rows).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} {body:?}"
        );
        // Reported ONCE, whatever how many entries named it, and with the reason.
        assert_eq!(
            unprocessed_ids(&body),
            vec![administrator.clone()],
            "case {case}: expected exactly the administrator reported once, got {body:?}"
        );
        assert_eq!(
            unprocessed_result(&body, &administrator).as_deref(),
            Some(SELF_AS_MEMBER),
            "case {case}: expected the self-as-member reason, got {body:?}"
        );

        // Nothing was persisted for it, and every other entry of the same call was.
        let mut requested: Vec<&str> = others.iter().map(String::as_str).collect();
        requested.push(administrator.as_str());
        let (status, read) = get_members(&router, &detector_id, &requested).await;
        assert_eq!(
            status, 200,
            "case {case}: get-members expected HTTP 200, got {status} {read:?}"
        );
        assert!(
            member_of(&read, &administrator).is_none(),
            "case {case}: expected no member record for the administrator, got {read:?}"
        );
        for (index, id) in others.iter().enumerate() {
            let member = member_of(&read, id).unwrap_or_else(|| {
                panic!("case {case} entry {index}: expected a member, got {read:?}")
            });
            assert_eq!(
                member["email"].as_str(),
                Some(email_for(index)),
                "case {case} entry {index}: expected the recorded email, got {member:?}"
            );
            assert_eq!(
                member["relationshipStatus"].as_str(),
                Some(CREATED),
                "case {case} entry {index}: expected a recorded member, got {member:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_batch_is_refused_whole_when_one_entry_cannot_be_a_value_of_its_member() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let good_one = account_id(30, 0);
    let good_two = account_id(30, 1);

    // A parameter failure is not a per-account failure: the request carries a value
    // that cannot be a value of its member, so it is refused WHOLE and the
    // well-formed entries beside it are not recorded either. The rows differ along
    // which member is at fault and where in the batch it sits.
    // 12 characters, so the batch reaches the handler instead of being refused at the
    // modelled boundary: `AccountId` is `len 12..12` and constrains the LENGTH only.
    let bad_account = json!({"accountId": "abcdefghijkl", "email": "member@example.com"});
    let bad_email = json!({"accountId": account_id(30, 2), "email": "nope"});
    let good = json!({"accountId": good_one, "email": EMAIL_FORMS[0]});
    let good_other = json!({"accountId": good_two, "email": EMAIL_FORMS[1]});

    for (case, details) in [
        (
            "a malformed account id first",
            json!([bad_account.clone(), good.clone(), good_other.clone()]),
        ),
        (
            "a malformed account id last",
            json!([good.clone(), good_other.clone(), bad_account.clone()]),
        ),
        (
            "a malformed email in the middle",
            json!([good.clone(), bad_email.clone(), good_other.clone()]),
        ),
    ] {
        let (status, body) = create_members_raw(&router, &detector_id, details).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            CLASS,
            "case {case}: expected the {CLASS} class, got {body:?}"
        );
        assert_eq!(
            error_shape(&body),
            MODELED_CLIENT_ERROR,
            "case {case}: expected the {MODELED_CLIENT_ERROR} envelope, got {body:?}"
        );

        for id in [&good_one, &good_two] {
            let (_, read) = get_members(&router, &detector_id, &[id.as_str()]).await;
            assert!(
                member_of(&read, id).is_none(),
                "case {case}: a refused batch records none of it, got {read:?} for {id}"
            );
        }
    }
}
