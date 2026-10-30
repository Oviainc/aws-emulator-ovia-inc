// SPDX-License-Identifier: CC0-1.0

//! A paginated walk of either discovery list while a discovery pass lands between two of
//! its pages — including the pass another call runs, which is what makes this a rule about
//! the WALK rather than about one request.
//!
//! Both discovery lists project the ONE store of discovered instances, and every report of
//! discovered data materializes the pass the clock owes it, so a walk cannot be isolated
//! from a pass that lands under it — and does not try to be. What it guarantees instead is
//! asserted here, through BOTH lists, over a table of page sizes, of positions in the walk
//! and of calls landing there:
//!
//! * the walk reports every record the store holds throughout it exactly once and in the
//!   list's order, and a record that APPEARS under it at most once — its cursor names the
//!   element the previous page reported LAST, so a pass that adds or removes a record
//!   carries nothing across it;
//! * what the walk reports does not depend on WHICH other calls landed between its pages: a
//!   first-page call to the sibling list, or to the list being walked, runs the same pass
//!   the walk's own next page runs, so the walked sequence is the one the walk would have
//!   reported had nobody else called at all;
//! * a record no pass can hold — one of a Region the account's settings do not discover —
//!   stops being reported by the page after it appears, whichever call ran the pass that
//!   dropped it.
//!
//! The records are placed through the harness's seeding seam (the same `engine.state::<…>()`
//! the handlers read) because no operation of this service writes a discovered instance: a
//! pass reads the account's EC2 inventory, which `tests/all/discovery_from_ec2.rs` drives.
//! A pass made from this suite's emulator therefore finds no EC2 to ask and records nothing,
//! which leaves exactly its pruning step observable — and pruning is what can REMOVE an
//! element from under a walk.

use serde_json::json;

use crate::harness::{
    call, enable_discovery, harness, seed_discovered_instance, Harness, ACCOUNT, REGION,
};
use crate::shared_discovery_report::{page, List, LISTS};
use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::state::discovery::SUBSCRIPTION_TYPE_INCLUDED;
use service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::state::{
    DiscoveredInstanceRecord, INSTANCE_STATUS_RUNNING,
};

/// A Region the account's settings do NOT name, so a record held for it is one no pass can
/// hold: a pass keeps the Regions the settings discover and no others.
const OTHER_REGION: &str = "us-east-1";

/// How many records the walks below page over: enough that every page size in the table
/// leaves the walk at least one page to report after the writes land.
const FLEET: usize = 8;

/// One discovered instance, stating only what these rules page, group and prune on; every
/// other member is what discovery leaves for an instance no third-party subscription
/// provider was involved in.
///
/// The subscription names are all the same length and none is a prefix of another, as are
/// the instance ids: the two lists are ordered by (name, type) and by instance id, so a
/// record's position in either of them is then the position of its identity in plain string
/// order — which is what lets a walk's own order be asserted below.
fn record(index: usize, region: &str) -> DiscoveredInstanceRecord {
    DiscoveredInstanceRecord {
        instance_id: format!("i-{index:03}"),
        account_id: ACCOUNT.to_string(),
        ami_id: "ami-0001".to_string(),
        instance_type: "t3.micro".to_string(),
        status: INSTANCE_STATUS_RUNNING.to_string(),
        region: region.to_string(),
        usage_operation: "RunInstances:0010".to_string(),
        product_codes: vec!["pc-rhel".to_string()],
        subscription_name: format!("Subscription {index:03}"),
        subscription_type: SUBSCRIPTION_TYPE_INCLUDED.to_string(),
        ..DiscoveredInstanceRecord::default()
    }
}

/// The records the store holds for the whole walk, in the Region the account discovers.
///
/// Their indexes leave room on both sides for the records a row adds mid-walk.
fn fleet() -> Vec<DiscoveredInstanceRecord> {
    (1..=FLEET)
        .map(|index| record(index * 10, REGION))
        .collect()
}

/// The record a row adds mid-walk that is ordered BEFORE every element of the fleet, so the
/// walk is already past its position: it must not be reported.
fn added_behind_the_cursor() -> DiscoveredInstanceRecord {
    record(1, REGION)
}

/// The record a row adds mid-walk that is ordered AFTER every element of the fleet, so the
/// walk has not reached its position: it is in a discovered Region, so no pass drops it and
/// the walk must report it exactly once.
fn added_ahead_of_the_cursor() -> DiscoveredInstanceRecord {
    record(990, REGION)
}

/// The record a row adds mid-walk that NO pass can hold: it is held for a Region the
/// account's settings do not discover. It is ordered ahead of the cursor too, so the walk
/// would reach its position — which is what makes "a pass dropped it" the only reason it is
/// not reported.
fn added_but_not_discoverable() -> DiscoveredInstanceRecord {
    record(995, OTHER_REGION)
}

/// What a row lets land between two pages of the walk, besides the writes themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Interleaved {
    /// Nothing: the walk's own next page is the first report after the writes.
    Nothing,
    /// A first-page call to the OTHER discovery list — the one this list shares its store
    /// with, and the call a walk must be unaffected by.
    SiblingFirstPage,
    /// A first-page call to the list being walked, from another client.
    OwnFirstPage,
    /// Both of those, in that order.
    BothFirstPages,
}

impl Interleaved {
    /// Let these calls land, discarding what they report: what is under test is what the
    /// WALK reports afterwards.
    async fn land(self, h: &Harness, list: &List, case: &str) {
        let sibling = sibling(list);
        let mut calls: Vec<&List> = Vec::new();
        match self {
            Interleaved::Nothing => {}
            Interleaved::SiblingFirstPage => calls.push(sibling),
            Interleaved::OwnFirstPage => calls.push(list),
            Interleaved::BothFirstPages => {
                calls.push(sibling);
                calls.push(list);
            }
        }
        for called in calls {
            let interleaved = format!("{case}: the interleaved {} call", called.label);
            call(h, called.path, &json!({})).await.ok(&interleaved);
        }
    }
}

/// The other discovery list: the two of them report from one store, so each is the other's
/// sibling.
fn sibling(list: &List) -> &'static List {
    LISTS
        .iter()
        .find(|other| other.label != list.label)
        .expect("the two discovery lists are each other's sibling")
}

/// Walk `list` to its end in pages of `page_size`, letting the writes and `interleaved`
/// land after page `after_page`, and answer the identities the walk reported, in order.
async fn walk(
    list: &List,
    page_size: usize,
    after_page: usize,
    interleaved: Interleaved,
    case: &str,
) -> Vec<String> {
    let h = harness();
    // The settings that make the fleet discovered data: discovery enabled for the ONE
    // Region it was found in, which is also what makes `OTHER_REGION` a Region no pass may
    // hold a record for.
    enable_discovery(&h, &[REGION]).await;
    for stored in fleet() {
        seed_discovered_instance(&h, stored);
    }

    let mut walked: Vec<String> = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut body = json!({ "MaxResults": page_size });
        if let Some(token) = &cursor {
            body["NextToken"] = json!(token);
        }
        let (reported, next) = page(&h, list, &body, case).await;
        assert!(
            reported.len() <= page_size,
            "case {case:?}: page {pages} reported {} entries, more than MaxResults \
             {page_size}: {reported:?}",
            reported.len()
        );
        walked.extend(reported);
        cursor = next;
        pages += 1;

        if pages == after_page {
            assert!(
                cursor.is_some(),
                "case {case:?}: the writes must land with pages left to report, but the walk \
                 ended after {pages} page(s): {walked:?}"
            );
            for stored in [
                added_behind_the_cursor(),
                added_ahead_of_the_cursor(),
                added_but_not_discoverable(),
            ] {
                seed_discovered_instance(&h, stored);
            }
            interleaved.land(&h, list, case).await;
        }

        assert!(
            pages <= FLEET + 3,
            "case {case:?}: the walk did not terminate after {pages} pages: {walked:?}"
        );
        if cursor.is_none() {
            break;
        }
    }
    walked
}

#[tokio::test]
async fn a_walk_of_either_discovery_list_reports_the_same_sequence_whoever_else_calls() {
    for list in LISTS {
        // The rows differ along what the rule generalises over: which list is walked, the
        // page size, how far into the walk the pass lands, and WHICH call runs it.
        for page_size in 1..=3 {
            for after_page in 1..=2 {
                let row = format!(
                    "{}: pages of {page_size}, writes after page {after_page}",
                    list.label
                );
                let alone = format!("{row}, nothing else called");
                let expected =
                    walk(list, page_size, after_page, Interleaved::Nothing, &alone).await;
                assert_walk(list, &expected, &alone);

                for interleaved in [
                    Interleaved::SiblingFirstPage,
                    Interleaved::OwnFirstPage,
                    Interleaved::BothFirstPages,
                ] {
                    let case = format!("{row}, {interleaved:?} in between");
                    let walked = walk(list, page_size, after_page, interleaved, &case).await;
                    assert_walk(list, &walked, &case);
                    assert_eq!(
                        walked, expected,
                        "case {case:?}: a walk must report what it would have reported had \
                         nobody else called, {expected:?}, got {walked:?}"
                    );
                }
            }
        }
    }
}

/// What every walk of the table must satisfy, whatever landed between its pages.
fn assert_walk(list: &List, walked: &[String], case: &str) {
    // In the list's order, each element at most once: the identities are strictly
    // ascending, which is what a cursor naming the previous page's last element gives.
    assert!(
        walked.windows(2).all(|pair| pair[0] < pair[1]),
        "case {case:?}: a walk must report the list's order with no element twice, got \
         {walked:?}"
    );

    // Every record the store held for the whole walk is reported, and so is the record that
    // appeared ahead of the cursor: it is in a discovered Region, so no pass drops it.
    let mut held = fleet();
    held.push(added_ahead_of_the_cursor());
    for stored in &held {
        let identity = (list.stored)(stored);
        assert!(
            walked.contains(&identity),
            "case {case:?}: {identity:?} is in the store, so the walk must report it, got \
             {walked:?}"
        );
    }

    // The record that appeared BEHIND the cursor is not: the walk had already reported the
    // part of the list its position belongs to.
    let behind = (list.stored)(&added_behind_the_cursor());
    assert!(
        !walked.contains(&behind),
        "case {case:?}: {behind:?} appeared behind the cursor, which the walk is already \
         past, got {walked:?}"
    );

    // And the record no pass can hold is reported by no page after it appeared: every
    // report runs a pass, so the next page of the walk drops it even when nothing else was
    // called.
    let undiscoverable = (list.stored)(&added_but_not_discoverable());
    assert!(
        !walked.contains(&undiscoverable),
        "case {case:?}: {undiscoverable:?} is held for a Region the settings do not \
         discover, so a pass drops it before the next page, got {walked:?}"
    );
}

#[tokio::test]
async fn a_walk_whose_remaining_records_a_pass_drops_ends_without_a_refusal() {
    for list in LISTS {
        let case = &format!("{}: everything left dropped mid-walk", list.label);
        let h = harness();
        enable_discovery(&h, &[REGION]).await;
        for stored in fleet() {
            seed_discovered_instance(&h, stored);
        }

        let (walked, cursor) = page(&h, list, &json!({ "MaxResults": 1 }), case).await;
        let token = cursor
            .unwrap_or_else(|| panic!("case {case:?}: a truncated page must carry a NextToken"));

        // The account stops discovering the Region its whole inventory was found in, so
        // nothing a pass may hold is left: the cursor then names an element ordered before
        // nothing at all.
        enable_discovery(&h, &[OTHER_REGION]).await;

        let (reported, next) = page(
            &h,
            list,
            &json!({ "MaxResults": 1, "NextToken": token }),
            case,
        )
        .await;
        assert!(
            reported.is_empty(),
            "case {case:?}: nothing the account discovers is left, so the walk is over, got \
             {reported:?} after {walked:?}"
        );
        assert!(
            next.is_none(),
            "case {case:?}: the walk is over, so no NextToken, got {next:?}"
        );
    }
}
