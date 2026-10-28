// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `ListReusableDelegationSets`.
//!
//! The rule under test: the operation REPORTS the reusable delegation sets the
//! account holds at the moment of the call, read from the service's state in one
//! stable total order over their ids, as a page that starts at the requested
//! `Marker`, carries at most `MaxItems` of them, echoes both of those back and
//! names in `NextMarker` the set the next page starts at — so the pages
//! concatenate to the whole population exactly once, and nothing a caller reads
//! changes what a later caller sees.
//!
//! The tables therefore vary the axes the rule generalises over rather than
//! re-creating one set: the POPULATION (none, then one more set per creation
//! row, where the rows differ along the caller reference and along whether the
//! set is minted fresh or taken from an existing public hosted zone's
//! delegation), the PAGE SIZE (every size from one to one past the population,
//! plus the documented maximum and one past it), the MARKER (every position in
//! the listing, in both spellings a set id is accepted in, plus one past the
//! end) and the number of REPEATED reads. Every listed set is cross-checked
//! against what `GetReusableDelegationSet` reports for the same id, so the
//! listing is proved against another reader of the same state rather than
//! against itself.
//!
//! Every test drives the service through the real router, using the SAME harness
//! the other delegation-set suites use, so what is asserted is the HTTP/restXml
//! behaviour a Route 53 client sees.

use smithy_server_runtime::MultiServiceRouter;

use crate::hosted_zone_records::{call, router, Reply};
use crate::reusable_delegation_sets::{bare_set_id, create_set, get_set, set_count};
use crate::vpc_associations::{bare_zone_id, create_zone};

/// The most reusable delegation sets one response carries, from
/// `ListReusableDelegationSetsRequest$MaxItems`'s own documentation: "The number
/// of reusable delegation sets that you want Amazon Route 53 to return in the
/// response to this request. If you specify a value greater than 100, Route 53
/// returns only the first 100 reusable delegation sets."
const DOCUMENTED_MAX_ITEMS: usize = 100;

/// The other published bound of `Nonce`, which `CallerReference` targets
/// (smithy_query(query="shape",shape="Nonce") reports `len 1..128`).
const CALLER_REFERENCE_MAX_LEN: usize = 128;

/// How many name servers a delegation set delegates to.
///
/// `CreateReusableDelegationSet`'s own documentation calls a delegation set "a
/// group of four name servers".
const NAME_SERVER_COUNT: usize = 4;

/// Send one `ListReusableDelegationSets` request, with `query` as its query
/// string.
async fn list_sets(router: &MultiServiceRouter, query: &str) -> Reply {
    call(router, "GET", &format!("/2013-04-01/delegationset{query}"), "").await
}

/// List every reusable delegation set in one call, asserting the call succeeded.
async fn listing(router: &MultiServiceRouter, case: &str) -> Reply {
    let reply = list_sets(router, "").await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from ListReusableDelegationSets, got {} with body {}",
        reply.status, reply.body
    );
    reply
}

/// The set ids a listing reports, in the order it reports them.
fn listed_ids(reply: &Reply) -> Vec<String> {
    reply
        .all("DelegationSet/Id")
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// One listed `DelegationSet`, as the wire reports it.
#[derive(Debug, PartialEq, Eq)]
struct ListedSet {
    id: String,
    caller_reference: Option<String>,
    name_servers: Vec<String>,
}

/// Every `DelegationSet` a listing carries, in document order. The name servers
/// come back as one flat run, so they are split back into the fixed group of
/// four each set delegates to.
fn listed_sets(reply: &Reply, case: &str) -> Vec<ListedSet> {
    let ids = listed_ids(reply);
    let references = reply.all("DelegationSet/CallerReference");
    let servers = reply.all("NameServers/NameServer");
    assert_eq!(
        references.len(),
        ids.len(),
        "case {case:?}: every listed set must carry a CallerReference, got {} for {} set(s); \
         body {}",
        references.len(),
        ids.len(),
        reply.body
    );
    assert_eq!(
        servers.len(),
        ids.len() * NAME_SERVER_COUNT,
        "case {case:?}: expected {NAME_SERVER_COUNT} name servers per listed set, got {} \
         for {} set(s); body {}",
        servers.len(),
        ids.len(),
        reply.body
    );
    ids.into_iter()
        .enumerate()
        .map(|(index, id)| ListedSet {
            id,
            caller_reference: references.get(index).map(|text| (*text).to_string()),
            name_servers: servers[index * NAME_SERVER_COUNT..(index + 1) * NAME_SERVER_COUNT]
                .iter()
                .map(|server| (*server).to_string())
                .collect(),
        })
        .collect()
}

/// The set `GetReusableDelegationSet` reports for `id`, in the same shape the
/// listing reports one in, so the two readers of the same state can be compared.
async fn read_set(router: &MultiServiceRouter, id: &str, case: &str) -> ListedSet {
    let reply = get_set(router, bare_set_id(id)).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: the listed set {id} must be readable, got {} with body {}",
        reply.status, reply.body
    );
    let sets = listed_sets(&reply, case);
    assert_eq!(
        sets.len(),
        1,
        "case {case:?}: GetReusableDelegationSet must report exactly one set, got {sets:?}"
    );
    sets.into_iter()
        .next()
        .expect("the reply carries exactly one set")
}

/// One row of the population: how the set under it is created.
struct SetRow {
    case: &'static str,
    caller_reference: String,
    /// `Some(zone name)` when the row marks an existing PUBLIC hosted zone's
    /// delegation reusable instead of minting four fresh name servers.
    zone: Option<String>,
}

/// `base`, padded with `c` to exactly `len` characters, so a row can sit on a
/// published `CallerReference` bound and still be unique to its suite.
fn padded(base: &str, len: usize) -> String {
    let mut reference = base.to_string();
    reference.truncate(len);
    while reference.len() < len {
        reference.push('c');
    }
    reference
}

/// The creation rows, differing along the caller reference (shortest, longest,
/// one carrying XML special characters) and along whether the set is minted
/// fresh or taken from an existing public hosted zone.
fn population_rows(prefix: &str) -> Vec<SetRow> {
    vec![
        SetRow {
            case: "a set of four fresh name servers",
            caller_reference: format!("{prefix}-plain"),
            zone: None,
        },
        SetRow {
            case: "a caller reference at the 1-character minimum",
            caller_reference: "x".to_string(),
            zone: None,
        },
        SetRow {
            case: "a caller reference at the 128-character maximum",
            caller_reference: padded(&format!("{prefix}-max-"), CALLER_REFERENCE_MAX_LEN),
            zone: None,
        },
        SetRow {
            case: "a caller reference carrying XML special characters",
            caller_reference: format!("{prefix}-a&b<c>d\"e'f"),
            zone: None,
        },
        SetRow {
            case: "a set taken from an existing public hosted zone's delegation",
            caller_reference: format!("{prefix}-from-zone"),
            zone: Some(format!("{prefix}.example.com")),
        },
    ]
}

/// Create the set one row describes and return its reported (`/delegationset/`
/// prefixed) id.
async fn create_row(router: &MultiServiceRouter, row: &SetRow) -> String {
    let zone_id = match &row.zone {
        None => None,
        Some(name) => {
            let created = create_zone(router, name, &row.caller_reference, "").await;
            assert_eq!(
                created.status, 201,
                "case {:?}: expected 201 from CreateHostedZone, got {} with body {}",
                row.case, created.status, created.body
            );
            Some(bare_zone_id(&created))
        }
    };
    let created = create_set(router, &row.caller_reference, zone_id.as_deref()).await;
    assert_eq!(
        created.status, 201,
        "case {:?}: expected 201 from CreateReusableDelegationSet, got {} with body {}",
        row.case, created.status, created.body
    );
    created
        .first("DelegationSet/Id")
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: the created set must carry DelegationSet/Id; body {}",
                row.case, created.body
            )
        })
        .to_string()
}

/// Create one set per row and return every reported id.
async fn populate(router: &MultiServiceRouter, prefix: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for row in &population_rows(prefix) {
        ids.push(create_row(router, row).await);
    }
    ids
}

#[tokio::test]
async fn the_listing_reports_every_reusable_delegation_set_the_account_holds() {
    // The population is the varying axis: none, then one more set after every
    // row, so the listing is asserted against a set that grows by a set created
    // a DIFFERENT way each time.
    let router = router();
    let mut created: Vec<String> = Vec::new();

    for row in &population_rows("listed") {
        let case = format!("{} set(s), before creating {}", created.len(), row.case);
        let reply = listing(&router, &case).await;
        let listed = listed_ids(&reply);

        let mut expected = created.clone();
        expected.sort();
        assert_eq!(
            listed, expected,
            "case {case:?}: the listing must report every reusable delegation set the account \
             holds, in id order; body {}",
            reply.body
        );
        assert!(
            reply.first("IsTruncated") != Some("true"),
            "case {case:?}: a listing that fits one page must not be truncated; body {}",
            reply.body
        );
        assert!(
            reply.first("NextMarker").is_none(),
            "case {case:?}: an untruncated listing must carry no NextMarker; body {}",
            reply.body
        );

        // The same population, counted through another operation over the same
        // state.
        let counted = set_count(&router).await;
        assert_eq!(
            counted,
            listed.len().to_string(),
            "case {case:?}: the listing must hold as many sets as GetAccountLimit counts"
        );

        // Every listed set is reported whole, and exactly as the single-set read
        // of the same state reports it.
        for set in listed_sets(&reply, &case) {
            let read = read_set(&router, &set.id, &case).await;
            assert_eq!(
                set, read,
                "case {case:?}: the listing must report {} as GetReusableDelegationSet does",
                read.id
            );
            assert!(
                set.caller_reference.is_some_and(|reference| !reference.is_empty()),
                "case {case:?}: every listed set must carry its CallerReference; body {}",
                reply.body
            );
        }

        created.push(create_row(&router, row).await);
    }
}

#[tokio::test]
async fn delegation_set_paging_covers_the_listing_exactly_once() {
    // The rule as an invariant: a page holds at most MaxItems sets and
    // NextMarker names where the next page starts, so the pages concatenate to
    // the full listing for EVERY page size. The page size is the varying axis.
    let router = router();
    let created = populate(&router, "paged").await;

    let unpaged = listing(&router, "the whole listing").await;
    let all = listed_ids(&unpaged);
    assert_eq!(
        all.len(),
        created.len(),
        "every created reusable delegation set must be listed; body {}",
        unpaged.body
    );

    for page_size in 1..=created.len() + 1 {
        let mut walked: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &marker {
                Some(resume) => format!("?maxitems={page_size}&marker={resume}"),
                None => format!("?maxitems={page_size}"),
            };
            let page = list_sets(&router, &query).await;
            assert_eq!(
                page.status, 200,
                "case maxitems={page_size}: expected 200, got {} with body {}",
                page.status, page.body
            );
            assert_eq!(
                page.first("Marker"),
                marker.as_deref(),
                "case maxitems={page_size}: the response must echo the request's marker; body {}",
                page.body
            );
            assert_eq!(
                page.first("MaxItems"),
                Some(page_size.to_string()).as_deref(),
                "case maxitems={page_size}: the response must echo the request's maxitems; \
                 body {}",
                page.body
            );
            let ids = listed_ids(&page);
            assert!(
                ids.len() <= page_size,
                "case maxitems={page_size}: a page held {} set(s); body {}",
                ids.len(),
                page.body
            );
            walked.extend(ids);
            pages += 1;
            assert!(
                pages <= created.len() + 2,
                "case maxitems={page_size}: paging did not terminate"
            );
            if page.first("IsTruncated") != Some("true") {
                assert!(
                    page.first("NextMarker").is_none(),
                    "case maxitems={page_size}: a final page must carry no NextMarker; body {}",
                    page.body
                );
                break;
            }
            marker = Some(
                page.first("NextMarker")
                    .expect("a truncated page must carry a NextMarker")
                    .to_string(),
            );
        }
        assert_eq!(
            walked, all,
            "case maxitems={page_size}: paging must cover the listing exactly once"
        );
    }
}

#[tokio::test]
async fn a_marker_resumes_the_listing_at_the_set_it_names() {
    // `Marker` is "the ID of the first reusable delegation set that Amazon Route
    // 53 will return if you submit another request", so a listing resumed at any
    // position reports exactly the tail from that position. Every position is a
    // row, in both spellings a set id is accepted in, plus an id past the end of
    // the listing.
    let router = router();
    populate(&router, "resumed").await;
    let all = listed_ids(&listing(&router, "the whole listing").await);
    assert!(
        all.len() > 1,
        "the suite needs several reusable delegation sets to resume at, got {}",
        all.len()
    );

    for (index, id) in all.iter().enumerate() {
        let bare = bare_set_id(id).to_string();
        for (spelling, marker) in [("the bare id", bare.clone()), ("the prefixed id", id.clone())] {
            let case = format!("marker={marker} ({spelling})");
            let page = list_sets(&router, &format!("?marker={marker}")).await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: expected 200, got {} with body {}",
                page.status, page.body
            );
            assert_eq!(
                page.first("Marker"),
                Some(marker.as_str()),
                "case {case:?}: the response must echo the marker it was sent; body {}",
                page.body
            );
            let tail = listed_ids(&page);
            assert_eq!(
                tail,
                all[index..].to_vec(),
                "case {case:?}: the listing must resume at the set the marker names, got {tail:?}; \
                 body {}",
                page.body
            );
        }
    }

    // A marker past every id reports an empty, untruncated page rather than
    // wrapping around to the start.
    let past_the_end = "NZZZZZZZZZZZZZZ";
    let page = list_sets(&router, &format!("?marker={past_the_end}")).await;
    let tail = listed_ids(&page);
    assert!(
        tail.is_empty() && page.first("IsTruncated") != Some("true"),
        "case marker={past_the_end}: expected an empty final page, got {tail:?}; body {}",
        page.body
    );
}

#[tokio::test]
async fn the_listing_echoes_the_page_size_it_was_asked_for() {
    // `MaxItems` in the response is "the value that you specified for the
    // maxitems parameter", so it is echoed whatever the caller sent — while the
    // page itself never exceeds the documented maximum. The rows are sizes below
    // the population, the documented bound and one past it.
    let router = router();
    let created = populate(&router, "sized").await;

    for requested in [1, 2, DOCUMENTED_MAX_ITEMS, DOCUMENTED_MAX_ITEMS + 1] {
        let page = list_sets(&router, &format!("?maxitems={requested}")).await;
        assert_eq!(
            page.status, 200,
            "case maxitems={requested}: expected 200, got {} with body {}",
            page.status, page.body
        );
        let echoed = page.first("MaxItems").unwrap_or("");
        assert_eq!(
            echoed,
            requested.to_string(),
            "case maxitems={requested}: expected the requested maxitems echoed, got {echoed:?}; \
             body {}",
            page.body
        );
        let ids = listed_ids(&page);
        let expected = requested.min(DOCUMENTED_MAX_ITEMS).min(created.len());
        assert_eq!(
            ids.len(),
            expected,
            "case maxitems={requested}: expected {expected} set(s), got {}; body {}",
            ids.len(),
            page.body
        );
        let truncated = page.first("IsTruncated") == Some("true");
        assert_eq!(
            truncated,
            ids.len() < created.len(),
            "case maxitems={requested}: IsTruncated must say whether more remain; body {}",
            page.body
        );
    }

    // With no maxitems at all the whole population comes back in one page, and
    // the response reports the page size the service applied.
    let whole = listing(&router, "no maxitems").await;
    assert_eq!(
        listed_ids(&whole).len(),
        created.len(),
        "case {:?}: a request without maxitems must list every set; body {}",
        "no maxitems",
        whole.body
    );
    assert_eq!(
        whole.first("MaxItems"),
        Some(DOCUMENTED_MAX_ITEMS.to_string()).as_deref(),
        "case {:?}: a request without maxitems must report the page size applied; body {}",
        "no maxitems",
        whole.body
    );
}

#[tokio::test]
async fn listing_the_reusable_delegation_sets_leaves_the_account_exactly_as_it_was() {
    // The rule is a report: repeating it answers the same listing, creates and
    // consumes nothing, and the population it reports still moves with a later
    // create. The number of repeated reads is the varying axis.
    let router = router();
    let created = populate(&router, "reported").await;
    let first = listed_sets(&listing(&router, "the first listing").await, "the first listing");
    assert_eq!(
        first.len(),
        created.len(),
        "every created reusable delegation set must be listed"
    );
    let before = set_count(&router).await;

    for attempt in 1..=3 {
        let case = format!("listing {attempt}");
        let again = listed_sets(&listing(&router, &case).await, &case);
        assert_eq!(
            again, first,
            "case {case:?}: a read must report what the first read did"
        );
        let counted = set_count(&router).await;
        assert_eq!(
            counted, before,
            "case {case:?}: listing must not change the population, got {counted}"
        );
    }

    for set in &first {
        let read = read_set(&router, &set.id, "after the repeated listings").await;
        assert_eq!(
            &read, set,
            "case {:?}: every listed set must still read back unchanged",
            "after the repeated listings"
        );
    }

    let added = create_row(
        &router,
        &SetRow {
            case: "a set created after the listing",
            caller_reference: "reported-after-the-listing".to_string(),
            zone: None,
        },
    )
    .await;
    let after = listed_ids(&listing(&router, "after a later create").await);
    assert!(
        after.contains(&added) && after.len() == first.len() + 1,
        "case {:?}: the listing must follow the state, got {after:?}",
        "after a later create"
    );
}
