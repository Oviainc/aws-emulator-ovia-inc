// SPDX-License-Identifier: CC0-1.0

//! Sharing a Managed Microsoft AD directory with a consumer account, driven end to end
//! over HTTP.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: DirectoryService_20150416.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router (`crate::harness`). Nothing here reaches into
//! the state struct, so a response that is not computed from persisted state cannot pass,
//! and every created share is read back through `DescribeSharedDirectories`.
//!
//! The cases assert RULES over sets of inputs rather than one recorded call: the stored
//! share over a table of requests spanning the WHOLE modelled `ShareMethod` catalog, the
//! presence and length of `ShareNotes` (including the published `Notes` bound and one past
//! it) and several consumer account ids; the `Sharing` -> `Shared` transition an
//! organization share owes against the `PendingAcceptance` a handshake share rests at; the
//! one-share-per-consumer-account rule; the refusals over tables of targets that are not
//! account ids, of directory ids that name nothing and of directory types sharing is not
//! supported for; the narrowing and paging `DescribeSharedDirectories` performs over a
//! table of filters; and this emulator's documented per-directory share guard at the bound
//! and one past it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::directory_lifecycle::{create_directory, minimal_create, PAST_ONE_SPAN};
use crate::harness::{call, ok, refused, service, string};
use crate::microsoft_ad::{create_microsoft_ad, minimal_microsoft_ad};

/// The whole modelled `ShareMethod` catalog
/// (`smithy_query(query="shape",shape="ShareMethod")`), every value of which a directory
/// can be shared with.
pub(crate) const SHARE_METHOD_CATALOG: [&str; 2] = ["ORGANIZATIONS", "HANDSHAKE"];

/// The whole modelled `TargetType` catalog
/// (`smithy_query(query="shape",shape="TargetType")`): the one kind of identifier a
/// `ShareTarget$Id` can be.
pub(crate) const TARGET_TYPE_CATALOG: [&str; 1] = ["ACCOUNT"];

/// The `ShareStatus` a handshake share rests at until the consumer account answers: the
/// `PendingAcceptance` value of the modelled `ShareStatus` enum.
const SHARE_STATUS_PENDING_ACCEPTANCE: &str = "PendingAcceptance";

/// The `ShareStatus` an organization share carries between `ShareDirectory` and the
/// transition the clock owes: the `Sharing` value of the modelled `ShareStatus` enum.
const SHARE_STATUS_SHARING: &str = "Sharing";

/// The `ShareStatus` an organization share settles on: the enum's `Shared` value.
const SHARE_STATUS_SHARED: &str = "Shared";

/// The account id every resource of this emulator belongs to
/// (`engine::EMULATOR_ACCOUNT_ID`), which is the directory owner account a share reports.
const OWNER_ACCOUNT_ID: &str = engine::EMULATOR_ACCOUNT_ID;

/// The longest `ShareNotes` the model admits: the modelled `Notes` length bound
/// (`smithy_query(query="shape",shape="SharedDirectory")`, member `ShareNotes`:
/// `length 0..1024`).
const SHARE_NOTES_MAX_LEN: usize = 1024;

/// This emulator's documented per-directory share guard
/// (`state::MAX_SHARES_PER_DIRECTORY`).
const SHARE_GUARD: usize = 50;

/// A consumer account id of the shape the modelled `CustomerId` pattern `^(\d{12})$`
/// demands, derived from `nth` so a table row can ask for a fresh one.
pub(crate) fn consumer_account(nth: usize) -> String {
    format!("{:012}", 100_000_000_000_u64 + nth as u64)
}

/// A `ShareNotes` value of exactly `len` characters.
fn notes_of_length(len: usize) -> String {
    "n".repeat(len)
}

/// A `ShareDirectory` body: the owner directory, the consumer account as an `ACCOUNT`
/// target, the share method and, when given, the notes.
pub(crate) fn share_body(
    directory_id: &str,
    account_id: &str,
    method: &str,
    notes: Option<&str>,
) -> Value {
    let mut body = json!({
        "DirectoryId": directory_id,
        "ShareTarget": { "Id": account_id, "Type": TARGET_TYPE_CATALOG[0] },
        "ShareMethod": method
    });
    if let Some(notes) = notes {
        body["ShareNotes"] = json!(notes);
    }
    body
}

/// `ShareDirectory` with `body`, asserting it was accepted and returning the minted
/// `SharedDirectoryId`.
pub(crate) async fn share(router: &MultiServiceRouter, body: Value, case: &str) -> String {
    let answer = ok(router, "ShareDirectory", body, case).await;
    let shared_id = string(&answer, "SharedDirectoryId", case);
    assert_directory_id(&shared_id, case);
    shared_id
}

/// Assert `id` is of the shape the modelled `DirectoryId` pattern `^d-[0-9a-f]{10}$`
/// declares.
fn assert_directory_id(id: &str, case: &str) {
    let digits = id.strip_prefix("d-").unwrap_or_default();
    assert!(
        digits.len() == 10 && digits.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "case {case:?}: expected a d- prefixed ten hex digit DirectoryId, got {id:?}"
    );
}

/// One `DescribeSharedDirectories` page for `owner`, with `extra` members merged into the
/// request.
pub(crate) async fn describe_shares(
    router: &MultiServiceRouter,
    owner: &str,
    extra: Vec<(&str, Value)>,
    case: &str,
) -> Value {
    let mut body = json!({ "OwnerDirectoryId": owner });
    for (member, value) in extra {
        body[member] = value;
    }
    ok(router, "DescribeSharedDirectories", body, case).await
}

/// The `SharedDirectories` of a page, in the order the service reported them.
pub(crate) fn share_list(body: &Value, case: &str) -> Vec<Value> {
    body["SharedDirectories"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a SharedDirectories list, got {body}"))
        .clone()
}

/// The `SharedDirectoryId`s of a page, in the order the service reported them.
pub(crate) fn shared_ids(body: &Value, case: &str) -> Vec<String> {
    share_list(body, case)
        .iter()
        .map(|share| {
            share["SharedDirectoryId"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The single share `owner` holds for `shared_id`, read back through the listing.
pub(crate) async fn describe_one_share(
    router: &MultiServiceRouter,
    owner: &str,
    shared_id: &str,
    case: &str,
) -> Value {
    let page = describe_shares(
        router,
        owner,
        vec![("SharedDirectoryIds", json!([shared_id]))],
        case,
    )
    .await;
    let mut reported = share_list(&page, case);
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: expected exactly the asked-for share, got {page}"
    );
    reported.remove(0)
}

/// One row of the share table: the request and what the stored share must then report.
struct ShareCase {
    name: &'static str,
    method: &'static str,
    notes: Option<String>,
    expect: Vec<(&'static str, Value)>,
}

#[tokio::test]
async fn a_shared_directory_is_reported_from_persisted_state() {
    let router = service();
    let owner = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("owner.example.com"),
        "share-table owner",
    )
    .await;

    // The rows differ along everything `ShareDirectory` persists: the share method (the
    // whole modelled catalog), whether notes came with the request and how long they are
    // (the empty string and the published `Notes` bound included).
    let cases = [
        ShareCase {
            name: "organizations, no notes",
            method: SHARE_METHOD_CATALOG[0],
            notes: None,
            expect: vec![
                ("ShareMethod", json!("ORGANIZATIONS")),
                ("ShareStatus", json!(SHARE_STATUS_SHARING)),
                ("ShareNotes", Value::Null),
            ],
        },
        ShareCase {
            // "The ShareNotes parameter is only used when HANDSHAKE is called": an
            // organization share sends no request to carry them, so none is recorded.
            name: "organizations, notes ignored",
            method: SHARE_METHOD_CATALOG[0],
            notes: Some("please approve".to_string()),
            expect: vec![
                ("ShareMethod", json!("ORGANIZATIONS")),
                ("ShareStatus", json!(SHARE_STATUS_SHARING)),
                ("ShareNotes", Value::Null),
            ],
        },
        ShareCase {
            name: "handshake, no notes",
            method: SHARE_METHOD_CATALOG[1],
            notes: None,
            expect: vec![
                ("ShareMethod", json!("HANDSHAKE")),
                ("ShareStatus", json!(SHARE_STATUS_PENDING_ACCEPTANCE)),
                ("ShareNotes", Value::Null),
            ],
        },
        ShareCase {
            name: "handshake, notes",
            method: SHARE_METHOD_CATALOG[1],
            notes: Some("sharing with the audit account".to_string()),
            expect: vec![
                ("ShareMethod", json!("HANDSHAKE")),
                ("ShareStatus", json!(SHARE_STATUS_PENDING_ACCEPTANCE)),
                ("ShareNotes", json!("sharing with the audit account")),
            ],
        },
        ShareCase {
            name: "handshake, empty notes",
            method: SHARE_METHOD_CATALOG[1],
            notes: Some(String::new()),
            expect: vec![
                ("ShareStatus", json!(SHARE_STATUS_PENDING_ACCEPTANCE)),
                ("ShareNotes", json!("")),
            ],
        },
        ShareCase {
            name: "handshake, notes at the modelled bound",
            method: SHARE_METHOD_CATALOG[1],
            notes: Some(notes_of_length(SHARE_NOTES_MAX_LEN)),
            expect: vec![(
                "ShareNotes",
                json!(notes_of_length(SHARE_NOTES_MAX_LEN)),
            )],
        },
    ];

    let mut minted: Vec<String> = Vec::new();
    for (index, case) in cases.iter().enumerate() {
        let account = consumer_account(index);
        let shared_id = share(
            &router,
            share_body(&owner, &account, case.method, case.notes.as_deref()),
            case.name,
        )
        .await;

        // The consumer's handle on the owner's directory is an id of its own, and it is
        // never one already in use by another resource of this service.
        assert_ne!(
            shared_id, owner,
            "case {name:?}: expected the shared directory id to differ from the owner's",
            name = case.name
        );
        assert!(
            !minted.contains(&shared_id),
            "case {name:?}: expected a fresh shared directory id, got {shared_id} again",
            name = case.name
        );
        minted.push(shared_id.clone());

        let reported = describe_one_share(&router, &owner, &shared_id, case.name).await;
        for (member, expected) in &case.expect {
            assert_eq!(
                reported[*member], *expected,
                "case {name:?}: expected {member} to be {expected}, got {reported}",
                name = case.name
            );
        }
        // Every share reports the same derived values: this emulator's account as the
        // owner, the directory the request named, the consumer account it was shared
        // with, and the two timestamps.
        assert_eq!(
            reported["OwnerAccountId"],
            json!(OWNER_ACCOUNT_ID),
            "case {name:?}: expected the owner account id, got {reported}",
            name = case.name
        );
        assert_eq!(
            reported["OwnerDirectoryId"],
            json!(&owner),
            "case {name:?}: expected the owner directory id, got {reported}",
            name = case.name
        );
        assert_eq!(
            reported["SharedAccountId"],
            json!(&account),
            "case {name:?}: expected the consumer account id, got {reported}",
            name = case.name
        );
        assert!(
            reported["CreatedDateTime"].is_number()
                && reported["LastUpdatedDateTime"].is_number(),
            "case {name:?}: expected both share timestamps, got {reported}",
            name = case.name
        );
    }

    // Reading twice changes nothing a later caller sees: the listing is a report.
    let first = describe_shares(&router, &owner, vec![], "repeat read").await;
    let second = describe_shares(&router, &owner, vec![], "repeat read").await;
    assert_eq!(
        first, second,
        "expected two reads of the listing to agree, got {first} then {second}"
    );
    assert_eq!(
        shared_ids(&first, "repeat read"),
        minted,
        "expected the listing to report every share in creation order, got {first}"
    );
}

#[tokio::test]
async fn share_notes_past_the_modelled_bound_are_refused_before_anything_is_stored() {
    let router = service();
    let owner = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("notes.example.com"),
        "notes bound",
    )
    .await;

    // The modelled `Notes` length bound, at the bound and one past it.
    for (len, accepted) in [(SHARE_NOTES_MAX_LEN, true), (SHARE_NOTES_MAX_LEN + 1, false)] {
        let case = format!("ShareNotes of {len} characters");
        let body = share_body(
            &owner,
            &consumer_account(len),
            SHARE_METHOD_CATALOG[1],
            Some(&notes_of_length(len)),
        );
        let (status, answer) = call(&router, "ShareDirectory", body).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the share to be accepted, got {status} with {answer}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected the share to be refused, got {status} with {answer}"
        );
        assert_eq!(
            answer["__type"],
            json!("ValidationException"),
            "case {case:?}: expected the framework validation envelope, got {answer}"
        );
    }

    // The refused call stored nothing: only the accepted note's share is listed.
    let page = describe_shares(&router, &owner, vec![], "notes bound").await;
    assert_eq!(
        share_list(&page, "notes bound").len(),
        1,
        "expected only the accepted share to be stored, got {page}"
    );
}

#[tokio::test]
async fn an_organization_share_settles_on_shared_while_a_handshake_share_waits() {
    let router = service();
    let owner = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("transition.example.com"),
        "transition",
    )
    .await;

    let organizations = share(
        &router,
        share_body(
            &owner,
            &consumer_account(1),
            SHARE_METHOD_CATALOG[0],
            None,
        ),
        "organizations transition",
    )
    .await;
    let handshake = share(
        &router,
        share_body(&owner, &consumer_account(2), SHARE_METHOD_CATALOG[1], None),
        "handshake transition",
    )
    .await;

    // The span is never zero, so a caller acting straight after the call still sees the
    // transient status.
    let before = describe_one_share(&router, &owner, &organizations, "before the span").await;
    assert_eq!(
        before["ShareStatus"],
        json!(SHARE_STATUS_SHARING),
        "expected the organization share to still be sharing, got {before}"
    );

    tokio::time::sleep(PAST_ONE_SPAN).await;

    let settled = describe_one_share(&router, &owner, &organizations, "after the span").await;
    assert_eq!(
        settled["ShareStatus"],
        json!(SHARE_STATUS_SHARED),
        "expected the organization share to have settled, got {settled}"
    );
    assert_eq!(
        settled["CreatedDateTime"], before["CreatedDateTime"],
        "expected the creation time to be unchanged by the transition, got {settled}"
    );

    // A handshake share waits for the consumer account's answer: no clock moves it.
    let waiting = describe_one_share(&router, &owner, &handshake, "after the span").await;
    assert_eq!(
        waiting["ShareStatus"],
        json!(SHARE_STATUS_PENDING_ACCEPTANCE),
        "expected the handshake share to still be pending acceptance, got {waiting}"
    );
}

#[tokio::test]
async fn one_directory_is_shared_with_one_consumer_account_at_most_once() {
    let router = service();
    let first_owner = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("first.example.com"),
        "already shared",
    )
    .await;
    let second_owner = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("second.example.com"),
        "already shared",
    )
    .await;
    let account = consumer_account(7);

    let shared_id = share(
        &router,
        share_body(&first_owner, &account, SHARE_METHOD_CATALOG[1], None),
        "already shared",
    )
    .await;

    // The same pairing again is the directory that is already shared, whatever method the
    // second request asks for.
    for method in SHARE_METHOD_CATALOG {
        refused(
            &router,
            "ShareDirectory",
            share_body(&first_owner, &account, method, None),
            "DirectoryAlreadySharedException",
            &format!("re-share with {method}"),
        )
        .await;
    }

    // Another consumer account, and another owner directory, are different pairings and
    // are each shared on their own.
    let other_account = share(
        &router,
        share_body(
            &first_owner,
            &consumer_account(8),
            SHARE_METHOD_CATALOG[1],
            None,
        ),
        "another consumer account",
    )
    .await;
    let other_owner = share(
        &router,
        share_body(&second_owner, &account, SHARE_METHOD_CATALOG[1], None),
        "another owner directory",
    )
    .await;
    assert_ne!(
        other_owner, shared_id,
        "expected a second owner's share of one account to be its own shared directory"
    );

    let first_page = describe_shares(&router, &first_owner, vec![], "already shared").await;
    assert_eq!(
        shared_ids(&first_page, "already shared"),
        vec![shared_id, other_account],
        "expected the refused re-share to have stored nothing, got {first_page}"
    );
    let second_page = describe_shares(&router, &second_owner, vec![], "already shared").await;
    assert_eq!(
        shared_ids(&second_page, "already shared"),
        vec![other_owner],
        "expected each owner directory's listing to carry only its own shares, got \
         {second_page}"
    );
}

#[tokio::test]
async fn a_target_that_is_not_an_account_id_is_refused() {
    let router = service();
    let owner = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("target.example.com"),
        "target table",
    )
    .await;

    // An `ACCOUNT` target is identified by the twelve digits the modelled `CustomerId`
    // pattern `^(\d{12})$` admits, so the rows span the digit count on both sides of it,
    // non-digits, separators and blanks.
    let rows: [(&str, &str, bool); 9] = [
        ("twelve digits", "111122223333", true),
        ("twelve digits, leading zeros", "000000000001", true),
        ("eleven digits", "11112222333", false),
        ("thirteen digits", "1111222233334", false),
        ("one digit", "1", false),
        ("alphanumeric", "acct11112222", false),
        ("dashed", "1111-2222-3333", false),
        ("blank", " ", false),
        ("padded account id", " 111122223333 ", false),
    ];

    let mut accepted = 0usize;
    for (name, target, is_account_id) in rows {
        let body = share_body(&owner, target, SHARE_METHOD_CATALOG[1], None);
        if is_account_id {
            share(&router, body, name).await;
            accepted += 1;
            continue;
        }
        refused(
            &router,
            "ShareDirectory",
            body,
            "InvalidTargetException",
            name,
        )
        .await;
    }

    // A target `Type` outside the modelled `TargetType` catalog never reaches the
    // handler: the member constraint is the framework's to refuse.
    let (status, answer) = call(
        &router,
        "ShareDirectory",
        json!({
            "DirectoryId": &owner,
            "ShareTarget": { "Id": "111122223333", "Type": "ORGANIZATION" },
            "ShareMethod": SHARE_METHOD_CATALOG[1]
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "expected an off-catalog ShareTarget.Type to be refused, got {status} with {answer}"
    );

    let page = describe_shares(&router, &owner, vec![], "target table").await;
    assert_eq!(
        share_list(&page, "target table").len(),
        accepted,
        "expected only the account-id targets to have stored a share, got {page}"
    );
}

#[tokio::test]
async fn sharing_resolves_the_directory_and_its_type() {
    let router = service();

    // Well-formed `DirectoryId`s that name nothing this service holds.
    for absent in ["d-0123456789", "d-ffffffffff", "d-abcdef0123"] {
        refused(
            &router,
            "ShareDirectory",
            share_body(absent, &consumer_account(3), SHARE_METHOD_CATALOG[1], None),
            "EntityDoesNotExistException",
            absent,
        )
        .await;
        refused(
            &router,
            "DescribeSharedDirectories",
            json!({ "OwnerDirectoryId": absent }),
            "EntityDoesNotExistException",
            absent,
        )
        .await;
    }

    // "When you share your Managed Microsoft AD directory …": sharing is supported for
    // that directory type alone, so a directory of another type is refused.
    let simple_ad = create_directory(&router, minimal_create("simple.example.com"), "simple ad").await;
    for method in SHARE_METHOD_CATALOG {
        refused(
            &router,
            "ShareDirectory",
            share_body(
                &simple_ad,
                &consumer_account(4),
                method,
                None,
            ),
            "UnsupportedOperationException",
            &format!("SimpleAD with {method}"),
        )
        .await;
    }

    // A Managed Microsoft AD directory with no share yet is an empty listing, not a
    // refusal.
    let owner = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("unshared.example.com"),
        "unshared",
    )
    .await;
    let page = describe_shares(&router, &owner, vec![], "unshared").await;
    assert!(
        share_list(&page, "unshared").is_empty() && page["NextToken"].is_null(),
        "expected an unshared directory to report an empty listing, got {page}"
    );
    let simple_page = describe_shares(&router, &simple_ad, vec![], "simple ad").await;
    assert!(
        share_list(&simple_page, "simple ad").is_empty(),
        "expected the refused shares to have stored nothing, got {simple_page}"
    );
}

/// One row of the listing-filter table: what the request names and which shares the
/// listing must then report.
struct FilterCase {
    name: &'static str,
    extra: Vec<(&'static str, Value)>,
    expect: Vec<String>,
}

#[tokio::test]
async fn the_listing_narrows_and_pages_over_the_stored_shares() {
    let router = service();
    let owner = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("paged.example.com"),
        "paging",
    )
    .await;
    let other = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("other.example.com"),
        "paging",
    )
    .await;

    let mut ids: Vec<String> = Vec::new();
    for nth in 0..4 {
        ids.push(
            share(
                &router,
                share_body(
                    &owner,
                    &consumer_account(nth),
                    SHARE_METHOD_CATALOG[nth % SHARE_METHOD_CATALOG.len()],
                    None,
                ),
                "paging",
            )
            .await,
        );
    }
    let foreign = share(
        &router,
        share_body(&other, &consumer_account(9), SHARE_METHOD_CATALOG[1], None),
        "paging",
    )
    .await;

    // The rows differ along what the filter names and how big a page was asked for: no
    // filter, a subset, a repeated entry, an id belonging to another owner directory, and
    // an id this service never minted.
    let filters = [
        FilterCase {
            name: "no filter",
            extra: vec![],
            expect: ids.clone(),
        },
        FilterCase {
            name: "one id",
            extra: vec![("SharedDirectoryIds", json!([ids[2]]))],
            expect: vec![ids[2].clone()],
        },
        FilterCase {
            name: "two ids, reported in creation order",
            extra: vec![("SharedDirectoryIds", json!([ids[3], ids[1]]))],
            expect: vec![ids[1].clone(), ids[3].clone()],
        },
        FilterCase {
            name: "a repeated id",
            extra: vec![("SharedDirectoryIds", json!([ids[0], ids[0]]))],
            expect: vec![ids[0].clone()],
        },
        FilterCase {
            name: "another owner's share",
            extra: vec![("SharedDirectoryIds", json!([foreign]))],
            expect: vec![],
        },
        FilterCase {
            name: "an id this service never minted",
            extra: vec![("SharedDirectoryIds", json!(["d-0000000000"]))],
            expect: vec![],
        },
        FilterCase {
            name: "an empty filter list",
            extra: vec![("SharedDirectoryIds", json!([]))],
            expect: vec![],
        },
        FilterCase {
            // `Limit`'s own documentation: "If this value is zero, the maximum number of
            // items is specified by the limitations of the operation."
            name: "a zero limit",
            extra: vec![("Limit", json!(0))],
            expect: ids.clone(),
        },
        FilterCase {
            name: "a limit above the count",
            extra: vec![("Limit", json!(50))],
            expect: ids.clone(),
        },
    ];
    for case in filters {
        let page = describe_shares(&router, &owner, case.extra, case.name).await;
        assert_eq!(
            shared_ids(&page, case.name),
            case.expect,
            "case {name:?}: expected exactly those shares, got {page}",
            name = case.name
        );
        assert!(
            page["NextToken"].is_null(),
            "case {name:?}: expected a final page to carry no token, got {page}",
            name = case.name
        );
    }

    // A `Limit` under the count pages: every page is followed by its token until the
    // listing is exhausted, and the pages concatenate to the whole listing.
    for limit in 1..=3usize {
        let case = format!("limit {limit}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut extra = vec![("Limit", json!(limit))];
            if let Some(cursor) = token.clone() {
                extra.push(("NextToken", json!(cursor)));
            }
            let page = describe_shares(&router, &owner, extra, &case).await;
            let reported = shared_ids(&page, &case);
            assert!(
                reported.len() <= limit,
                "case {case:?}: expected at most {limit} shares a page, got {page}"
            );
            walked.extend(reported);
            token = page["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, ids,
            "case {case:?}: expected the pages to walk the whole listing, got {walked:?}"
        );
    }

    // A token this service did not issue, and one issued for another listing, are both
    // refused rather than served as a page of this one.
    let first = describe_shares(&router, &owner, vec![("Limit", json!(1))], "token reuse").await;
    let issued_for_the_unfiltered_listing = first["NextToken"]
        .as_str()
        .expect("a truncated page carries a token")
        .to_string();
    let rows = [
        ("not a token", "not-a-token".to_string()),
        ("hex of nothing we issued", hex::encode("v1:shares:1|")),
        (
            "issued for another filter",
            issued_for_the_unfiltered_listing.clone(),
        ),
    ];
    for (name, token) in rows {
        let mut body = json!({
            "OwnerDirectoryId": &owner,
            "Limit": 1,
            "NextToken": token
        });
        if name == "issued for another filter" {
            body["SharedDirectoryIds"] = json!([ids[0]]);
        }
        refused(
            &router,
            "DescribeSharedDirectories",
            body,
            "InvalidNextTokenException",
            name,
        )
        .await;
    }
    // The same token under the filter it was issued for still works.
    let resumed = describe_shares(
        &router,
        &owner,
        vec![
            ("Limit", json!(1)),
            ("NextToken", json!(issued_for_the_unfiltered_listing)),
        ],
        "token reuse",
    )
    .await;
    assert_eq!(
        shared_ids(&resumed, "token reuse"),
        vec![ids[1].clone()],
        "expected the token to resume the listing it was issued for, got {resumed}"
    );
}

#[tokio::test]
async fn the_per_directory_share_guard_is_enforced_at_its_bound() {
    let router = service();
    let owner = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("guard.example.com"),
        "share guard",
    )
    .await;

    for nth in 0..SHARE_GUARD {
        share(
            &router,
            share_body(
                &owner,
                &consumer_account(nth),
                SHARE_METHOD_CATALOG[1],
                None,
            ),
            &format!("share {nth}"),
        )
        .await;
    }

    // One past the bound is refused, and stores nothing.
    refused(
        &router,
        "ShareDirectory",
        share_body(
            &owner,
            &consumer_account(SHARE_GUARD),
            SHARE_METHOD_CATALOG[1],
            None,
        ),
        "ShareLimitExceededException",
        "one past the guard",
    )
    .await;

    let page = describe_shares(
        &router,
        &owner,
        vec![("Limit", json!(SHARE_GUARD + 1))],
        "share guard",
    )
    .await;
    assert_eq!(
        share_list(&page, "share guard").len(),
        SHARE_GUARD,
        "expected exactly the shares at the bound to be stored, got {page}"
    );

    // The guard is per directory: another directory starts with its whole allowance.
    let fresh = create_microsoft_ad(
        &router,
        minimal_microsoft_ad("fresh.example.com"),
        "share guard",
    )
    .await;
    share(
        &router,
        share_body(
            &fresh,
            &consumer_account(SHARE_GUARD),
            SHARE_METHOD_CATALOG[1],
            None,
        ),
        "another directory's allowance",
    )
    .await;
}
