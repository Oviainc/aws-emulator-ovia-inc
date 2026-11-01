// SPDX-License-Identifier: CC0-1.0

//! A `nextToken` is a POSITION the listing holds, not any value shaped like one.
//!
//! Both of this service's paginated listings resume AT a resource id, so the tokens one of
//! them can hand out are exactly the ids of the entries THAT call's filters select. These
//! cases drive both listings over the wire and assert the two halves of that rule: every
//! position a listing holds is accepted and resumes the listing exactly at it, and every
//! other value — however well it imitates an id this service mints — is refused with the
//! modelled `ValidationException` instead of being used as the page's lower bound, which
//! answers a truncated or an empty listing a caller cannot tell from the last page.
//!
//! The tables span what the rule generalises over rather than one request: cursors at every
//! position of the order (first, middle, last) for the accepted half, and for the refused
//! half values that sort below every held id, above every held id, one digit off a held id,
//! a freshly minted id, and an id the SIBLING listing holds — plus a cursor scoped to
//! filters the call does not repeat, and one naming a record deleted since it was issued.

use serde_json::Value;
use service_com_amazonaws_iotdeviceadvisor_iotsenateservice as iotdeviceadvisor;

use super::shared_paging_rule::{listed_ids, seed, LISTINGS, SEEDED};
use super::suite_lifecycle::Emulator;

/// The region every case below signs for.
const REGION: &str = "us-east-1";

/// The ids `listing` holds, in the order it lists them, read over the wire.
async fn held_positions(
    emulator: &Emulator,
    listing: &super::shared_paging_rule::Listing,
) -> Vec<String> {
    let all = emulator.call("GET", listing.path, REGION, None).await;
    assert_eq!(
        all.status, 200,
        "case {:?}: listing everything must answer HTTP 200, got {} {}",
        listing.path, all.status, all.body
    );
    let held = listed_ids(listing, &all.body);
    assert_eq!(
        held.len(),
        SEEDED,
        "case {:?}: the seed stores {SEEDED} entries, got {held:?}",
        listing.path
    );
    held
}

/// `id` with its last hex digit changed: a value that keeps the shape of an id this service
/// mints while naming a position no listing of it holds.
fn with_last_digit_changed(id: &str) -> String {
    let mut changed = id.to_string();
    let last = changed.pop().expect("an id is never empty");
    changed.push(if last == '0' { '1' } else { '0' });
    changed
}

/// Every position a listing holds is a cursor it hands out, so each is accepted and resumes
/// the listing AT it — and the cursor a one-entry page hands out is the next position in
/// that same order.
#[tokio::test]
async fn every_listing_accepts_each_position_it_holds_and_resumes_at_it() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for listing in &LISTINGS {
        let held = held_positions(&emulator, listing).await;
        for (index, position) in held.iter().enumerate() {
            let case = format!("{} resumed at position {index}", listing.path);
            let resumed = emulator
                .call(
                    "GET",
                    &format!("{}?nextToken={position}", listing.path),
                    REGION,
                    None,
                )
                .await;
            assert_eq!(
                resumed.status, 200,
                "case {case:?}: a cursor the listing holds must be accepted, got {} {}",
                resumed.status, resumed.body
            );
            let listed = listed_ids(listing, &resumed.body);
            assert_eq!(
                listed.as_slice(),
                &held[index..],
                "case {case:?}: a cursor resumes the listing AT the position it names, so the \
                 page must carry every entry from there on, got {listed:?}"
            );

            // The token a one-entry page hands out is the NEXT position, so the set of
            // tokens this listing issues is the set of positions it holds.
            let page = emulator
                .call(
                    "GET",
                    &format!("{}?maxResults=1&nextToken={position}", listing.path),
                    REGION,
                    None,
                )
                .await;
            let expected = held.get(index + 1).map(String::as_str);
            assert_eq!(
                page.member("nextToken").as_str(),
                expected,
                "case {case:?}: a one-entry page must hand out the next position {expected:?}, \
                 got {}",
                page.body
            );
        }
    }
}

/// A value shaped like an id this service mints, naming no position the listing holds, is
/// refused with the modelled `ValidationException` — never served as a truncated or empty
/// page.
#[tokio::test]
async fn every_listing_refuses_an_id_shaped_token_naming_no_position_it_holds() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for (index, listing) in LISTINGS.iter().enumerate() {
        let held = held_positions(&emulator, listing).await;
        // The other listing's ids are UUIDs this same service minted, so a cursor crossed
        // between the two listings misses only the set it is used against.
        let sibling = &LISTINGS[1 - index];
        let sibling_held = held_positions(&emulator, sibling).await;

        let middle = with_last_digit_changed(&held[SEEDED / 2]);
        let minted = iotdeviceadvisor::state::new_uuid();
        let crossed = sibling_held[0].clone();
        let unissued: [(&str, String); 5] = [
            (
                // Sorts below every id the listing holds: read as a lower bound it would
                // answer a FULL first page, hiding that the cursor was never issued.
                "the lowest id the shape admits",
                "00000000-0000-0000-0000-000000000000".to_string(),
            ),
            (
                // Sorts above every id the listing holds: read as a lower bound it would
                // answer an EMPTY page with no nextToken, which a client cannot tell from
                // an empty account.
                "the highest id the shape admits",
                "ffffffff-ffff-ffff-ffff-ffffffffffff".to_string(),
            ),
            (
                "a freshly minted id this listing never handed out",
                minted,
            ),
            (
                "a held position with its last hex digit changed",
                middle.clone(),
            ),
            ("an id the sibling listing holds", crossed.clone()),
        ];

        assert!(
            !held.contains(&middle) && !held.contains(&crossed),
            "case {:?}: the unissued rows must name no held position, got {held:?} against \
             {middle:?} and {crossed:?}",
            listing.path
        );

        for (row, token) in &unissued {
            let case = format!("{} with {row}", listing.path);
            let refused = emulator
                .call(
                    "GET",
                    &format!("{}?nextToken={token}", listing.path),
                    REGION,
                    None,
                )
                .await;
            assert_eq!(
                (refused.status, refused.error_code.as_deref()),
                (400, Some("ValidationException")),
                "case {case:?}: a cursor this listing never handed out must be refused, got \
                 {} {}",
                refused.status,
                refused.body
            );
            assert_eq!(
                refused.member("message").as_str(),
                Some("Invalid nextToken"),
                "case {case:?}: unexpected refusal message, got {}",
                refused.body
            );
            assert!(
                refused.member(listing.list).is_null(),
                "case {case:?}: a refusal carries no page of entries, got {}",
                refused.body
            );
        }

        // A refused cursor changed nothing: the listing still serves every entry.
        let after = held_positions(&emulator, listing).await;
        assert_eq!(
            after, held,
            "case {:?}: refusing a cursor must leave the listing exactly as it was, got \
             {after:?} after {held:?}",
            listing.path
        );
    }
}

/// A suite-run cursor is a position of the set the call lists, so it is accepted only by a
/// call whose filters still select the run it names.
#[tokio::test]
async fn a_suite_run_cursor_is_scoped_to_the_filters_of_the_call_that_lists_it() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let all = emulator.call("GET", "/suiteRuns", REGION, None).await;
    let entries: Vec<Value> = all.body["suiteRunsList"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        entries.len(),
        SEEDED,
        "setup: the seed stores {SEEDED} runs, got {}",
        all.body
    );

    // The cursor the unfiltered listing hands out for a one-entry page: the id of its
    // second run.
    let first_page = emulator
        .call("GET", "/suiteRuns?maxResults=1", REGION, None)
        .await;
    let issued = first_page
        .member("nextToken")
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "setup: a one-entry page of {SEEDED} runs carries a nextToken, got {}",
                first_page.body
            )
        })
        .to_string();
    let owner = entries
        .iter()
        .find(|entry| entry["suiteRunId"] == Value::String(issued.clone()))
        .and_then(|entry| entry["suiteDefinitionId"].as_str())
        .unwrap_or_else(|| panic!("setup: the cursor names a listed run, got {}", all.body))
        .to_string();
    let other = entries
        .iter()
        .filter_map(|entry| entry["suiteDefinitionId"].as_str())
        .find(|definition| *definition != owner)
        .unwrap_or_else(|| panic!("setup: the seed stores runs of several definitions"))
        .to_string();

    // Each row repeats the cursor against a differently filtered set: the one it came from,
    // one that still selects the run it names, one that selects another definition's runs,
    // and one whose version filter selects nothing.
    let cases: [(&str, String, u16); 4] = [
        (
            "the unfiltered listing that issued it",
            format!("/suiteRuns?nextToken={issued}"),
            200,
        ),
        (
            "a filter that still selects the run it names",
            format!("/suiteRuns?suiteDefinitionId={owner}&nextToken={issued}"),
            200,
        ),
        (
            "a filter that selects another definition's runs",
            format!("/suiteRuns?suiteDefinitionId={other}&nextToken={issued}"),
            400,
        ),
        (
            // Every definition the seed creates holds `v1` only, so this filter selects no
            // run at all and therefore holds no position.
            "a version filter that selects nothing",
            format!("/suiteRuns?suiteDefinitionId={owner}&suiteDefinitionVersion=v2&nextToken={issued}"),
            400,
        ),
    ];

    for (case, path, expected) in &cases {
        let answer = emulator.call("GET", path, REGION, None).await;
        assert_eq!(
            answer.status, *expected,
            "case {case:?}: expected HTTP {expected}, got {} {}",
            answer.status, answer.body
        );
        if *expected == 400 {
            assert_eq!(
                answer.error_code.as_deref(),
                Some("ValidationException"),
                "case {case:?}: a cursor of another set is a client-input error, got {}",
                answer.body
            );
        } else {
            let listed = listed_ids(&LISTINGS[1], &answer.body);
            assert_eq!(
                listed.first().map(String::as_str),
                Some(issued.as_str()),
                "case {case:?}: the page must resume AT the run the cursor names, got {listed:?}"
            );
        }
    }
}

/// A cursor naming a definition deleted since it was issued is refused too: the service
/// resumes AT a position, and a position the store no longer holds is one it cannot tell
/// from a cursor no listing ever handed out, so the caller learns the token is stale
/// instead of receiving a page silently missing what came before it.
#[tokio::test]
async fn a_cursor_naming_a_deleted_definition_is_refused_rather_than_resumed_silently() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let first_page = emulator
        .call("GET", "/suiteDefinitions?maxResults=1", REGION, None)
        .await;
    let issued = first_page
        .member("nextToken")
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "setup: a one-entry page of {SEEDED} definitions carries a nextToken, got {}",
                first_page.body
            )
        })
        .to_string();

    let before = emulator
        .call(
            "GET",
            &format!("/suiteDefinitions?nextToken={issued}"),
            REGION,
            None,
        )
        .await;
    assert_eq!(
        listed_ids(&LISTINGS[0], &before.body).first().map(String::as_str),
        Some(issued.as_str()),
        "case \"while the definition is stored\": the cursor resumes AT it, got {} {}",
        before.status,
        before.body
    );

    let deleted = emulator
        .call(
            "DELETE",
            &format!("/suiteDefinitions/{issued}"),
            REGION,
            None,
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "setup: deleting the definition the cursor names failed with {} {}",
        deleted.status, deleted.body
    );

    let after = emulator
        .call(
            "GET",
            &format!("/suiteDefinitions?nextToken={issued}"),
            REGION,
            None,
        )
        .await;
    assert_eq!(
        (after.status, after.error_code.as_deref()),
        (400, Some("ValidationException")),
        "case \"after the definition is deleted\": a cursor naming no stored position must be \
         refused, got {} {}",
        after.status,
        after.body
    );

    let remaining = emulator.call("GET", "/suiteDefinitions", REGION, None).await;
    let listed = listed_ids(&LISTINGS[0], &remaining.body);
    assert_eq!(
        listed.len(),
        SEEDED - 1,
        "case \"after the definition is deleted\": the listing still serves every remaining \
         definition, got {listed:?}"
    );
    assert!(
        !listed.contains(&issued),
        "case \"after the definition is deleted\": the deleted definition must be gone, got \
         {listed:?}"
    );
}
