// SPDX-License-Identifier: CC0-1.0

//! The scope the Outpost inventory listing's cursor is read in.
//!
//! A `NextToken` is a position in ONE walk, so it is only a token a listing issued when it
//! names a place that walk has an order over. `ListOutpostsWithS3` walks the Outposts the
//! inventory holds in the Region the request was signed for, in `OutpostId` order, and an
//! Outpost is installed at a site anchored to exactly one Region — so a cursor naming an
//! Outpost installed in ANOTHER Region is one this Region's listing could never have
//! minted, and is refused with the one 400 `ValidationException` rather than served as a
//! page of whatever happens to sort after it (an empty one, reporting that a Region whose
//! untokened listing reports several Outposts reaches none) or silently ignored so the
//! first page is re-served.
//!
//! The rows span what the rule generalises over: which Region reads (so each Region is in
//! turn the one whose cursors are foreign), where the foreign Outpost sorts against the
//! reading Region's own keys (before, between, after), and which branch of `OutpostId`
//! (`^(op-[a-f0-9]{17}|\d{12}|ec2)$`, from `smithy_query(query="shape", shape="OutpostId")`)
//! names it. The positive controls are each Region's own cursors, which must still walk its
//! whole inventory, and a cursor naming an Outpost the inventory holds NOWHERE, which the
//! walk still serves from the first key at or after it — the Outpost a cursor names can
//! leave the control plane's inventory between two pages, and that departure must not
//! strand the rest.

use smithy_server_runtime::MultiServiceRouter;

use crate::endpoint_lifecycle::{emulator_holding, list_outposts_in, Answer, Listing};

/// The Regions this store holds Outposts in, and the Outposts installed in each.
///
/// The ids sort apart on purpose, in `OutpostId`'s own byte order
/// (`000000000012` < `ec2` < `op-…`): read from the first Region, a foreign Outpost sorts
/// both BEFORE and AFTER its own; read from the second, foreign ids sort BETWEEN its two.
/// Those are the three ways a cursor from another Region can fall against the order it is
/// replayed in. The ids also span every branch of `OutpostId`'s
/// `^(op-[a-f0-9]{17}|\d{12}|ec2)$`, so no branch is the only one the rule holds for.
const SITES: &[(&str, &[&str])] = &[
    (
        "us-east-1",
        &["op-55555555555555555", "op-77777777777777777"],
    ),
    ("us-west-2", &["000000000012", "op-fffffffffffffffff"]),
    ("eu-west-1", &["ec2", "op-00000000000000000"]),
];

/// Outpost ids the model's `OutpostId` admits that NO Region of this store holds — what a
/// cursor names once the Outpost it named has left the control plane's inventory.
///
/// They sort before, between and after the keys the Regions above hold, so the tolerance is
/// exercised at each position a departed record can sit at. The `ec2` branch is not here
/// because [`SITES`] installs it in a Region, which is what makes it a REFUSED cursor
/// elsewhere.
const UNHELD_OUTPOSTS: &[(&str, &str)] = &[
    (
        "the account-id branch, sorting before every id held",
        "000000000001",
    ),
    (
        "an op- id the inventory holds nowhere, sorting mid-order",
        "op-33333333333333333",
    ),
    (
        "an op- id the inventory holds nowhere, sorting after most ids held",
        "op-99999999999999999",
    ),
];

/// One emulator holding every Outpost of [`SITES`] in the Region it is installed in, owned
/// by the account this emulator serves — so the inventory listing of each Region reports
/// exactly that Region's own.
///
/// An Outpost is hardware nothing in this API provisions, so the store's own provisioning
/// seam stands it up; every answer asserted on below is still computed by the handler from
/// those records.
fn multi_region_emulator() -> MultiServiceRouter {
    let declared: Vec<(String, &str)> = SITES
        .iter()
        .flat_map(|(region, outposts)| {
            outposts
                .iter()
                .map(move |outpost_id| ((*outpost_id).to_string(), *region))
        })
        .collect();
    emulator_holding(&declared)
}

/// The `OutpostId`s an inventory answer reports, in the order it reported them — read
/// through the one listing harness, so this suite keeps no second copy of what a cursor key
/// is.
fn reported(answer: &Answer) -> Vec<String> {
    Listing::Outposts.reported_keys(answer)
}

/// The keys `region`'s inventory listing owes, in the store map's own key order: the ids
/// [`SITES`] installs there, sorted. The fixture is the oracle, so no order the handler
/// invents can pass.
fn keys_of(region: &str) -> Vec<String> {
    let mut keys: Vec<String> = SITES
        .iter()
        .filter(|(home, _)| *home == region)
        .flat_map(|(_, outposts)| outposts.iter().map(|id| (*id).to_string()))
        .collect();
    keys.sort();
    keys
}

/// Every Outpost id [`SITES`] installs OUTSIDE `region`, each with the Region whose listing
/// walks it.
fn foreign_outposts(region: &str) -> Vec<(String, String)> {
    SITES
        .iter()
        .filter(|(home, _)| *home != region)
        .flat_map(|(home, outposts)| {
            outposts.iter().map(move |outpost_id| {
                (
                    format!("an Outpost installed in {home} ({outpost_id})"),
                    (*outpost_id).to_string(),
                )
            })
        })
        .collect()
}

/// One page of `region`'s inventory listing.
async fn page(
    router: &MultiServiceRouter,
    region: &str,
    max_results: u32,
    token: Option<&str>,
) -> Answer {
    let query = match token {
        None => format!("?maxResults={max_results}"),
        Some(token) => format!("?maxResults={max_results}&nextToken={token}"),
    };
    list_outposts_in(router, region, &query).await
}

/// The 200 page a served cursor answers with, asserted to hold the keys at or after it.
async fn assert_serves(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    token: &str,
    expected: &[String],
) {
    let answer = page(router, region, 100, Some(token)).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?} in {region}: a cursor this listing can serve must answer 200, got {} \
         with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        reported(&answer),
        expected.to_vec(),
        "case {case:?} in {region}: the page must hold the Region's Outposts at or after \
         the cursor, expected {expected:?}, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.next_token(),
        None,
        "case {case:?} in {region}: a page that reaches the end carries no token, got {:?}",
        answer.next_token()
    );
}

/// The refusal the inventory listing answers a token it never issued with.
fn assert_refused(case: &str, region: &str, token: &str, answer: &Answer) {
    assert_eq!(
        (answer.status, answer.error_code()),
        (400, "ValidationException"),
        "case {case:?} in {region}: a token this listing never issued must be refused, got \
         {} with {:?}",
        answer.status,
        answer.body
    );
    assert_eq!(
        answer.message(),
        format!("NextToken {token:?} is not a token returned by ListOutpostsWithS3"),
        "case {case:?} in {region}: the refusal names the listing that never issued the \
         token, got {:?}",
        answer.message()
    );
}

/// A cursor naming an Outpost installed in another Region is refused by the inventory
/// listing, whichever Region reads and however the foreign id sorts against that Region's
/// own keys — never answered as the empty page a foreign id sorting last would make, nor as
/// the first page again. The refusal leaves what the Region reports untouched.
#[tokio::test]
async fn the_inventory_listing_refuses_a_cursor_of_an_outpost_of_another_region() {
    let router = multi_region_emulator();

    for (region, _) in SITES {
        // What this Region really reports: the refusals below are the alternative to
        // answering one of these pages, so the set must not be empty.
        let served = reported(&page(&router, region, 100, None).await);
        assert_eq!(
            served,
            keys_of(region),
            "case {region:?}: its untokened listing must report exactly the Outposts the \
             fixture installed there"
        );
        assert!(
            served.len() >= 2,
            "case {region:?}: the fixture must install more than one Outpost there, got \
             {served:?}"
        );

        let foreign = foreign_outposts(region);
        assert!(
            foreign.len() >= 4,
            "case {region:?}: the table must span the Outposts of every other Region, got \
             {foreign:?}"
        );

        for (case, token) in &foreign {
            assert_refused(
                case,
                region,
                token,
                &page(&router, region, 100, Some(token)).await,
            );
            // Also at the page size a client paging one record at a time sends, so the
            // refusal is not a property of the page bound.
            assert_refused(
                case,
                region,
                token,
                &page(&router, region, 1, Some(token)).await,
            );

            // The refusal changed nothing: the Region still reports exactly its own set.
            assert_eq!(
                reported(&page(&router, region, 100, None).await),
                served,
                "case {case:?} in {region}: a refused cursor must leave the Region's own \
                 Outposts exactly as they were"
            );
        }
    }
}

/// The positive control: each Region's own cursors still resume its walk at the key they
/// name, and following the tokens its listing mints walks its whole inventory one Outpost
/// per page — even though the store holds Outposts of other Regions whose cursors are
/// refused here.
///
/// The rows are each Region and each position in its own keys, so the resume point is
/// exercised at the first and at the last record.
#[tokio::test]
async fn each_region_walks_its_whole_inventory_with_the_cursors_it_mints() {
    let router = multi_region_emulator();

    for (region, _) in SITES {
        let whole = keys_of(region);

        for (index, cursor) in whole.iter().enumerate() {
            assert_serves(
                &format!("its own cursor {cursor} (record {index})"),
                &router,
                region,
                cursor,
                &whole[index..],
            )
            .await;
        }

        // One Outpost per page, following only the tokens the listing itself mints: every
        // token must be the Region's own next key, and the walk must cover the whole set.
        let mut walked: Vec<String> = Vec::new();
        let mut minted: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let answer = page(&router, region, 1, token.as_deref()).await;
            assert_eq!(
                answer.status, 200,
                "case {region:?}: a page of its own walk must answer 200, got {} with {:?}",
                answer.status, answer.body
            );
            walked.extend(reported(&answer));
            assert!(
                walked.len() <= whole.len(),
                "case {region:?}: paging must terminate, walked {walked:?}"
            );
            token = answer.next_token();
            match &token {
                None => break,
                Some(minted_token) => minted.push(minted_token.clone()),
            }
        }
        assert_eq!(
            walked, whole,
            "case {region:?}: following its own cursors walks its whole inventory, which is \
             {whole:?}"
        );
        assert_eq!(
            minted,
            whole[1..].to_vec(),
            "case {region:?}: every token minted names the first key the page did not \
             serve, got {minted:?}"
        );
    }
}

/// The other positive control: a cursor naming an Outpost the inventory holds NOWHERE is
/// still served, resuming at the first key at or after it. The inventory is the Outposts
/// control plane's own and an Outpost can leave it between two pages, so the record a
/// cursor names being gone must not strand the rest of the walk, nor turn a cursor the
/// listing did issue into a refusal.
///
/// The rows differ along which Region reads and where the departed id sorts against that
/// Region's keys — before them all (the whole set is served), between them (the tail is
/// served) and after them all (the empty page, with no token).
#[tokio::test]
async fn the_inventory_listing_serves_a_cursor_of_an_outpost_the_store_holds_nowhere() {
    let router = multi_region_emulator();

    for (region, _) in SITES {
        let whole = keys_of(region);
        for (case, outpost_id) in UNHELD_OUTPOSTS {
            let expected: Vec<String> = whole
                .iter()
                .filter(|key| key.as_str() >= *outpost_id)
                .cloned()
                .collect();
            assert_serves(case, &router, region, outpost_id, &expected).await;
        }
    }
}
