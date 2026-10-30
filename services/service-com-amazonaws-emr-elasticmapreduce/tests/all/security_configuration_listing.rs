// SPDX-License-Identifier: CC0-1.0

//! `ListSecurityConfigurations`, end to end over the wire.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.ListSecurityConfigurations` and an `awsJson1_1`
//! body — through the router's `dispatch`, so these are the same bytes an SDK client
//! sends. Nothing reaches into the service's state struct: what the listing reports is
//! cross-checked against what `CreateSecurityConfiguration` answered and what
//! `DescribeSecurityConfiguration` reads back, which is what makes these tests evidence
//! that the pages are computed from persisted state rather than from the request.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_emr_elasticmapreduce::state::LIST_PAGE_SIZE as PAGE_SIZE;

// The wire harness and the JSON walker are this test target's shared ones — this suite
// adds only the listing's request shapes.
use super::job_flow_lifecycle::{at, Emr};

/// The order a row submits its creates in, so the order the listing reports is shown to
/// come from the store's keys and not from the order the creates arrived in.
#[derive(Clone, Copy, Debug)]
enum Insertion {
    Ascending,
    Descending,
    OddsThenEvens,
}

/// The name of configuration `index`: three prefixes in rotation, so name order and
/// creation order differ however a row inserts.
fn name_of(index: usize) -> String {
    const PREFIXES: [&str; 3] = ["zeta", "alpha", "mid"];
    format!("{}-cfg-{index:04}", PREFIXES[index % PREFIXES.len()])
}

/// A legal security configuration carrying nothing but a comment naming its owner, so a
/// document read back can be attributed to exactly one name.
fn document_of(name: &str) -> String {
    json!({ "Comment": name }).to_string()
}

/// The indices `count` configurations are created in, under `insertion`.
fn creation_order(count: usize, insertion: Insertion) -> Vec<usize> {
    match insertion {
        Insertion::Ascending => (0..count).collect(),
        Insertion::Descending => (0..count).rev().collect(),
        Insertion::OddsThenEvens => (0..count)
            .filter(|index| index % 2 == 1)
            .chain((0..count).filter(|index| index % 2 == 0))
            .collect(),
    }
}

/// One page's `SecurityConfigurations` entries.
fn entries(case: &str, page: &Value) -> Vec<Value> {
    at(page, &["SecurityConfigurations"])
        .as_array()
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: SecurityConfigurations must be a list, got {page}")
        })
}

/// An entry's `Name`.
fn name_at(case: &str, entry: &Value) -> String {
    at(entry, &["Name"])
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: every entry carries a Name, got {entry}"))
        .to_string()
}

/// An entry's or a create's `CreationDateTime`, as epoch seconds.
fn instant_at(case: &str, payload: &Value) -> f64 {
    at(payload, &["CreationDateTime"])
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a CreationDateTime in {payload}"))
}

/// Walk the whole listing on `Marker`, answering every page as the wire answered it.
///
/// `held` only bounds the walk, so a listing that handed back a marker forever would
/// fail the assertion rather than hang the suite.
async fn walk(emr: &Emr, case: &str, held: usize) -> Vec<Value> {
    let max_pages = held / PAGE_SIZE + 2;
    let mut pages: Vec<Value> = Vec::new();
    let mut marker: Option<String> = None;
    loop {
        let body = match marker.as_ref() {
            Some(marker) => json!({ "Marker": marker }),
            None => json!({}),
        };
        let (status, payload) = emr.call("ListSecurityConfigurations", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListSecurityConfigurations must answer 200, got {status} \
             with {payload}"
        );
        let next = at(&payload, &["Marker"]).as_str().map(str::to_string);
        pages.push(payload);
        assert!(
            pages.len() <= max_pages,
            "case {case:?}: a listing of {held} configuration(s) must end within \
             {max_pages} page(s), got {} and still a marker",
            pages.len()
        );
        match next {
            Some(next) => marker = Some(next),
            None => return pages,
        }
    }
}

/// THE RULE: `ListSecurityConfigurations` reports the name and creation instant of every
/// security configuration the store holds — each exactly once, in the store's own order —
/// in pages of at most the documented maximum, where the `Marker` a page hands back is
/// the first entry the next page reports and is absent only on the last page; the call
/// reports only, so repeating it answers the same pages.
///
/// The table's rows differ along how many configurations are held, spanning what the
/// pagination generalises over — none, one, a handful, each side of the page bound and an
/// exact multiple of it — and along the order the creates arrived in, which the listing's
/// order must not follow.
#[tokio::test]
async fn the_listing_pages_project_every_stored_configuration_once() {
    struct Case {
        held: usize,
        insertion: Insertion,
    }

    let cases = [
        Case {
            held: 0,
            insertion: Insertion::Ascending,
        },
        Case {
            held: 1,
            insertion: Insertion::Descending,
        },
        Case {
            held: 7,
            insertion: Insertion::OddsThenEvens,
        },
        Case {
            held: PAGE_SIZE - 1,
            insertion: Insertion::Descending,
        },
        Case {
            held: PAGE_SIZE,
            insertion: Insertion::Ascending,
        },
        Case {
            held: PAGE_SIZE + 1,
            insertion: Insertion::OddsThenEvens,
        },
        Case {
            held: 2 * PAGE_SIZE,
            insertion: Insertion::Descending,
        },
    ];

    for Case { held, insertion } in cases {
        let case = format!("{held} configuration(s) held, created {insertion:?}");
        let emr = Emr::new();

        // What the creates stamped, keyed by name — the facts the listing has to
        // project. A `BTreeMap` keeps them in name order, which is the order a
        // name-valued `Marker` has to be able to resume at.
        let mut created: BTreeMap<String, f64> = BTreeMap::new();
        for index in creation_order(held, insertion) {
            let name = name_of(index);
            let out = emr
                .ok(
                    "CreateSecurityConfiguration",
                    json!({ "Name": name, "SecurityConfiguration": document_of(&name) }),
                )
                .await;
            created.insert(name, instant_at(&case, &out));
        }

        let pages = walk(&emr, &case, held).await;

        // Page shape: at most the documented maximum per page, every page but the last
        // one full, the marker present on exactly the pages that are not the last, and
        // each resumed page starting at the marker the previous page handed back — so a
        // walk neither repeats nor skips an entry, and a store whose size is an exact
        // multiple of the page size answers no empty trailing page.
        for (index, page) in pages.iter().enumerate() {
            let page_entries = entries(&case, page);
            let is_last = index + 1 == pages.len();
            assert!(
                page_entries.len() <= PAGE_SIZE,
                "case {case:?}: page {index} must carry at most {PAGE_SIZE} entries, got {}",
                page_entries.len()
            );
            assert_eq!(
                at(page, &["Marker"]).is_null(),
                is_last,
                "case {case:?}: page {index} is{} the last page, so its Marker must be \
                 {}; got {page}",
                if is_last { "" } else { " not" },
                if is_last { "absent" } else { "present" }
            );
            if !is_last {
                assert_eq!(
                    page_entries.len(),
                    PAGE_SIZE,
                    "case {case:?}: page {index} is followed by another, so it must be \
                     full; got {} entries",
                    page_entries.len()
                );
            }
            if held > 0 {
                assert!(
                    !page_entries.is_empty(),
                    "case {case:?}: page {index} must report something while the store \
                     holds {held}; got {page}"
                );
                if index > 0 {
                    let resumed_at = at(&pages[index - 1], &["Marker"]).as_str();
                    assert_eq!(
                        Some(name_at(&case, &page_entries[0]).as_str()),
                        resumed_at,
                        "case {case:?}: page {index} must begin at the marker page {} \
                         handed back, got {page}",
                        index - 1
                    );
                }
            }
        }

        // Every stored name once, in the store's name order — not the order the creates
        // arrived in.
        let listed: Vec<Value> = pages.iter().flat_map(|page| entries(&case, page)).collect();
        let listed_names: Vec<String> = listed
            .iter()
            .map(|entry| name_at(&case, entry))
            .collect();
        let expected: Vec<String> = created.keys().cloned().collect();
        assert_eq!(
            listed_names, expected,
            "case {case:?}: the listing must report every stored name exactly once, in \
             the store's order; got {listed_names:?}"
        );

        for entry in &listed {
            let name = name_at(&case, entry);

            // An entry carries the name and the creation instant, and nothing else:
            // `SecurityConfigurationSummary` models exactly those two members
            // (`smithy_query(query="shape", shape="SecurityConfigurationSummary")`), so
            // the document itself is not part of a listing.
            let mut members: Vec<String> = entry
                .as_object()
                .unwrap_or_else(|| panic!("case {case:?}: an entry is an object, got {entry}"))
                .keys()
                .cloned()
                .collect();
            members.sort();
            assert_eq!(
                members,
                vec!["CreationDateTime".to_string(), "Name".to_string()],
                "case {case:?}: entry {name:?} must carry only its name and creation \
                 instant, got {members:?}"
            );

            // The instant is the one the create stamped, and the configuration itself is
            // still there, whole — so the listing is a projection of the store.
            let listed_instant = instant_at(&case, entry);
            assert_eq!(
                listed_instant, created[&name],
                "case {case:?}: entry {name:?} must report the instant its create \
                 stamped ({}), got {listed_instant}",
                created[&name]
            );
            let described = emr
                .ok("DescribeSecurityConfiguration", json!({ "Name": name }))
                .await;
            assert_eq!(
                instant_at(&case, &described),
                listed_instant,
                "case {case:?}: {name:?} must describe at the instant the listing \
                 reports, got {described}"
            );
            assert_eq!(
                at(&described, &["SecurityConfiguration"]).as_str(),
                Some(document_of(&name).as_str()),
                "case {case:?}: {name:?} must still hold its document, got {described}"
            );
        }

        // The call only reports: walking it again — every page, same markers — answers
        // exactly the same bytes, so no page consumed or moved anything.
        let again = walk(&emr, &case, held).await;
        assert_eq!(
            again, pages,
            "case {case:?}: a second walk must answer the same pages; got {again:?}"
        );
    }
}

/// THE RULE: a configuration created or deleted between two listings moves only its own
/// entry, taking its place in the store's order rather than at the end of the last page.
///
/// The rows differ along where the change falls in that order — before the first entry,
/// inside a page, at a page boundary and after the last entry — which is what the
/// pagination has to absorb.
#[tokio::test]
async fn a_created_or_deleted_configuration_moves_only_its_own_entry() {
    let emr = Emr::new();

    // A page's worth, so the inserts below land in a listing that already paginates.
    let mut held: Vec<String> = Vec::new();
    for index in 0..PAGE_SIZE {
        let name = name_of(index);
        emr.ok(
            "CreateSecurityConfiguration",
            json!({ "Name": name, "SecurityConfiguration": document_of(&name) }),
        )
        .await;
        held.push(name);
    }
    held.sort();

    // Names chosen to sort before every held name, between two of them, at the page
    // boundary and after all of them.
    let inserts = [
        "aaa-before-every-name",
        "alpha-cfg-0001-and-a-half",
        "mid-cfg-9999",
        "zzz-after-every-name",
    ];

    for insert in inserts {
        let case = format!("inserting {insert:?}");
        emr.ok(
            "CreateSecurityConfiguration",
            json!({ "Name": insert, "SecurityConfiguration": document_of(insert) }),
        )
        .await;

        let mut expected = held.clone();
        expected.push(insert.to_string());
        expected.sort();

        let listed: Vec<String> = walk(&emr, &case, expected.len())
            .await
            .iter()
            .flat_map(|page| entries(&case, page))
            .map(|entry| name_at(&case, &entry))
            .collect();
        assert_eq!(
            listed, expected,
            "case {case:?}: the new configuration must take its place in the listing's \
             order; got {listed:?}"
        );

        // Deleting it again leaves the listing exactly as it was.
        emr.ok("DeleteSecurityConfiguration", json!({ "Name": insert }))
            .await;
        let after: Vec<String> = walk(&emr, &case, held.len())
            .await
            .iter()
            .flat_map(|page| entries(&case, page))
            .map(|entry| name_at(&case, &entry))
            .collect();
        assert_eq!(
            after, held,
            "case {case:?}: deleting it must leave every other entry where it was; got \
             {after:?}"
        );
    }
}
