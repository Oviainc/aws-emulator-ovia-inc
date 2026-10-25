// SPDX-License-Identifier: CC0-1.0

//! `GetDomains` — the paged listing of the domain recordsets an account holds —
//! driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: a listing reports every record the store holds of that
//! resource type, each entry exactly as the single-record read reports it, in the
//! store's own name order, carried one page at a time, with `nextPageToken`
//! NAMING the first record of the page after it while records remain and ABSENT
//! on the last page — and, being a read, it reports the same records however
//! often it is called, and follows every later create, edit and delete.
//!
//! The rows span what that rule generalises over: how many zones the account
//! holds (none, one, exactly a page, one past a page, several pages and a
//! remainder), the creation order against the reported order (every row is
//! created in reverse name order, so a listing that merely echoed the creation
//! sequence would fail), the form of each zone name, how many recordset entries
//! each zone carries and of which DNS record types.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::state;

// The harness this test target already exposes: a sibling suite drives the
// service through the same front door rather than standing up a second one.
use crate::instance_lifecycle::Emulator;

/// How many recordsets one page of `GetDomains` carries — the handler's OWN
/// constant, so the table's rows keep straddling the page boundary whatever it
/// is set to.
const PAGE_SIZE: usize = state::DOMAIN_PAGE_SIZE;

/// The recordset entries a zone is given, by the index of the zone in its row:
/// the entry type and the target in the form that type carries.
///
/// SOURCE: the entry types the modelled `DomainEntry$type` documentation lists
/// ("The type of domain entry, such as address for IPv4 (A) …"), as the crate
/// holds them in `state::DOMAIN_ENTRY_TYPES` — which the table below checks this
/// list against. Zone `index` of a row gets the first `index % (len + 1)` of
/// these, so the rows differ in how many entries a zone holds and in which types
/// they are, and no two neighbouring zones of a row look alike.
const ENTRY_SPECS: &[(&str, &str)] = &[
    ("A", "192.0.2.10"),
    ("AAAA", "2001:db8::1"),
    ("TXT", "\"listed-zone\""),
];

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// One `GetDomains` page, resuming from `page_token` when one is given.
async fn page(emulator: &Emulator, page_token: Option<&str>) -> Value {
    let body = match page_token {
        Some(token) => json!({"pageToken": token}),
        None => json!({}),
    };
    emulator.ok("GetDomains", body).await
}

/// The recordsets a page reports, in the order it reports them.
fn entries(page: &Value) -> Vec<Value> {
    page["domains"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a domains array, got {page:?}"))
        .clone()
}

/// The zone names a page reports, in the order it reports them.
fn names(page: &Value) -> Vec<String> {
    entries(page)
        .iter()
        .map(|domain| {
            domain["name"]
                .as_str()
                .unwrap_or_else(|| panic!("expected every entry to carry a name, got {page:?}"))
                .to_string()
        })
        .collect()
}

/// Walk every page of the listing, answering the records in the order reported
/// and the size of each page.
///
/// Checks the paging contract as it goes: no page carries more than `PAGE_SIZE`
/// records, a `nextPageToken` names the FIRST record of the page after it, a page
/// that is not the last one is full, and the walk terminates because the token is
/// absent rather than because it ran out of patience.
async fn walk(emulator: &Emulator, case: &str, max_pages: usize) -> (Vec<Value>, Vec<usize>) {
    let mut records = Vec::new();
    let mut page_sizes = Vec::new();
    let mut token: Option<String> = None;
    for page_index in 0..max_pages {
        let body = page(emulator, token.as_deref()).await;
        let page_names = names(&body);
        assert!(
            page_names.len() <= PAGE_SIZE,
            "case {case}: page {page_index} carries more than the {PAGE_SIZE} of a page, got \
             {page_names:?}"
        );
        if let Some(token) = &token {
            assert_eq!(
                page_names.first().map(String::as_str),
                Some(token.as_str()),
                "case {case}: a page token names the first record of the page after it, got \
                 {page_names:?}"
            );
        }
        page_sizes.push(page_names.len());
        records.extend(entries(&body));
        match body["nextPageToken"].as_str() {
            Some(next) => {
                assert_eq!(
                    page_names.len(),
                    PAGE_SIZE,
                    "case {case}: a page that is not the last one is full, got {page_names:?}"
                );
                token = Some(next.to_string());
            }
            None => return (records, page_sizes),
        }
    }
    panic!("case {case}: the walk did not terminate within {max_pages} pages");
}

/// The stored zone `name`, as `GetDomain` reports it.
async fn read_domain(emulator: &Emulator, name: &str) -> Value {
    emulator.ok("GetDomain", json!({"domainName": name})).await["domain"].clone()
}

// ---------------------------------------------------------------------------
// The listing table
// ---------------------------------------------------------------------------

/// One account's worth of zones: how many it holds, and the name form that tells
/// this row's records apart.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    /// The parent zone every name of this row sits under, so the rows differ in
    /// the number of labels, their characters and their length.
    suffix: &'static str,
    count: usize,
}

/// The table. The counts straddle every page boundary the rule can turn on.
const ROWS: &[Row] = &[
    Row {
        case: "an account holding nothing",
        suffix: "empty.example",
        count: 0,
    },
    Row {
        case: "an account holding one",
        suffix: "solo.test",
        count: 1,
    },
    Row {
        case: "exactly one page",
        suffix: "full.example.com",
        count: PAGE_SIZE,
    },
    Row {
        case: "one past a page",
        suffix: "spill-9.example.co.uk",
        count: PAGE_SIZE + 1,
    },
    Row {
        case: "several pages and a remainder",
        suffix: "wide-zone-3.my-example-7.net",
        count: 2 * PAGE_SIZE + 3,
    },
];

/// The name of this row's zone `index`. Zero-padded, so name order is index
/// order however many a row holds.
fn name_of(row: &Row, index: usize) -> String {
    format!("z{index:02}.{}", row.suffix)
}

/// The entries zone `index` of a row is given: the first
/// `index % (ENTRY_SPECS.len() + 1)` of [`ENTRY_SPECS`], so a zone holds none or
/// up to all of them.
fn entries_of(index: usize) -> Vec<(&'static str, &'static str)> {
    ENTRY_SPECS[..index % (ENTRY_SPECS.len() + 1)].to_vec()
}

/// The page sizes a listing of `total` records must report: full pages until the
/// remainder, and always at least one page — an empty account still answers once.
fn expected_page_sizes(total: usize) -> Vec<usize> {
    if total == 0 {
        return vec![0];
    }
    let mut sizes = Vec::new();
    let mut left = total;
    while left > 0 {
        let page = left.min(PAGE_SIZE);
        sizes.push(page);
        left -= page;
    }
    sizes
}

/// Create this row's zones, IN REVERSE NAME ORDER, each with its own tags and
/// its own recordset entries, and answer their names in name order.
async fn create_row(emulator: &Emulator, row: &Row) -> Vec<String> {
    let mut names = Vec::new();
    for index in (0..row.count).rev() {
        let name = name_of(row, index);
        emulator
            .ok(
                "CreateDomain",
                json!({
                    "domainName": name,
                    "tags": [{"key": "index", "value": index.to_string()}],
                }),
            )
            .await;
        for (entry_type, target) in entries_of(index) {
            emulator
                .ok(
                    "CreateDomainEntry",
                    json!({
                        "domainName": name,
                        "domainEntry": {
                            "name": format!("rr.{name}"),
                            "type": entry_type,
                            "target": target,
                        },
                    }),
                )
                .await;
        }
        names.push(name);
    }
    names.sort();
    names
}

/// RULE: `GetDomains` reports every stored recordset, each exactly as `GetDomain`
/// reports it, in the store's own name order and one page at a time.
#[tokio::test(flavor = "current_thread")]
async fn get_domains_reports_every_stored_zone_in_name_order() {
    // The hand-written entry catalog above is checked against its source: the
    // record types this service's recordsets admit.
    for (entry_type, _) in ENTRY_SPECS {
        assert!(
            state::DOMAIN_ENTRY_TYPES.contains(entry_type),
            "case the entry catalog: expected {entry_type} to be a record type the service \
             admits, got {:?}",
            state::DOMAIN_ENTRY_TYPES
        );
    }
    for row in ROWS {
        // One account per row: the store keys a recordset by domain name alone,
        // so a row reads a store holding exactly what that row created.
        let emulator = Emulator::new();
        let case = row.case;
        let created = create_row(&emulator, row).await;

        let (records, page_sizes) = walk(&emulator, case, row.count / PAGE_SIZE + 3).await;

        // The listing reports every stored record, in the store's name order.
        let reported: Vec<String> = records
            .iter()
            .map(|domain| {
                domain["name"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case}: expected a name, got {domain:?}"))
                    .to_string()
            })
            .collect();
        assert_eq!(
            reported, created,
            "case {case}: expected every stored name in name order, got {reported:?}"
        );

        // Each record is reported once; a page boundary neither repeats nor
        // skips one.
        let distinct: BTreeSet<&String> = reported.iter().collect();
        assert_eq!(
            distinct.len(),
            reported.len(),
            "case {case}: expected every record once, got {reported:?}"
        );

        // The pages are full until the remainder.
        assert_eq!(
            page_sizes,
            expected_page_sizes(row.count),
            "case {case}: expected full pages until the remainder, got {page_sizes:?}"
        );

        for (index, (record, name)) in records.iter().zip(&created).enumerate() {
            // Every listed entry is exactly what the single-record read
            // reports: one stored record, two operations.
            let single = read_domain(&emulator, name).await;
            assert_eq!(
                record, &single,
                "case {case}: the listing of {name} must match GetDomain, got {record:?}"
            );
            // And it carries THIS record's own request, not a canned body: the
            // zone's own tag, resource type and the recordset entries that were
            // appended to it.
            assert_eq!(
                record["tags"][0]["value"].as_str(),
                Some(index.to_string().as_str()),
                "case {case}: expected {name} to report the tag it was created with, got \
                 {record:?}"
            );
            assert_eq!(
                record["resourceType"].as_str(),
                Some("Domain"),
                "case {case}: expected {name} to report its resource type, got {record:?}"
            );
            let want_entries = entries_of(index);
            let reported_entries: Vec<(String, String)> = record["domainEntries"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {case}: expected a domainEntries array on {name}, got {record:?}")
                })
                .iter()
                .map(|entry| {
                    (
                        entry["type"].as_str().unwrap_or_default().to_string(),
                        entry["target"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect();
            let expected_entries: Vec<(String, String)> = want_entries
                .iter()
                .map(|(entry_type, target)| {
                    ((*entry_type).to_string(), (*target).to_string())
                })
                .collect();
            assert_eq!(
                reported_entries, expected_entries,
                "case {case}: expected {name} to report the entries appended to it, got \
                 {record:?}"
            );
        }
    }
}

/// RULE: a listing is a READ — it reports the same records, with the same
/// server-assigned identities, however often it is called, and leaves the
/// single-record read answering exactly what it answered before.
#[tokio::test(flavor = "current_thread")]
async fn listing_twice_reports_the_same_records() {
    let emulator = Emulator::new();
    let row = Row {
        case: "repeated listing",
        suffix: "repeat.example",
        count: PAGE_SIZE + 2,
    };
    let created = create_row(&emulator, &row).await;
    let before = read_domain(&emulator, &created[0]).await;

    let (first, first_sizes) = walk(&emulator, "first listing", 5).await;
    let (second, second_sizes) = walk(&emulator, "second listing", 5).await;
    assert_eq!(
        first, second,
        "case repeated listing: a read must not change what a later caller sees, got {second:?}"
    );
    assert_eq!(
        first_sizes, second_sizes,
        "case repeated listing: expected the same paging, got {second_sizes:?}"
    );
    let after = read_domain(&emulator, &created[0]).await;
    assert_eq!(
        after, before,
        "case repeated listing: expected the stored zone to be untouched by the listing, got \
         {after:?}"
    );
}

/// RULE: an empty listing is an empty LIST with no token, never an absent member
/// and never a refusal: the caller asked what it holds and the answer is
/// "nothing".
#[tokio::test(flavor = "current_thread")]
async fn an_account_holding_nothing_reports_an_empty_list_and_no_token() {
    let emulator = Emulator::new();
    for token in [None, Some("anything"), Some("")] {
        let body = page(&emulator, token).await;
        assert_eq!(
            entries(&body).len(),
            0,
            "case an empty account at {token:?}: expected an empty domains array, got {body:?}"
        );
        assert!(
            body["nextPageToken"].is_null(),
            "case an empty account at {token:?}: a last page carries no nextPageToken, got \
             {body:?}"
        );
    }
}

/// RULE: a page token NAMES a record rather than carrying an index, so resuming
/// from any stored name reports that name onward — the invariant that keeps a
/// record created or removed between two pages from making the walk skip or
/// repeat another one.
#[tokio::test(flavor = "current_thread")]
async fn resuming_from_a_stored_name_reports_that_name_onward() {
    let emulator = Emulator::new();
    let row = Row {
        case: "resuming",
        suffix: "resume.example",
        count: PAGE_SIZE + 4,
    };
    let all = create_row(&emulator, &row).await;

    for (index, token) in all.iter().enumerate() {
        let body = page(&emulator, Some(token)).await;
        let reported = names(&body);
        let expected: Vec<String> = all[index..].iter().take(PAGE_SIZE).cloned().collect();
        assert_eq!(
            reported, expected,
            "case resuming from {token}: expected the names from there onward, got {reported:?}"
        );
        let remaining = all.len() - index;
        assert_eq!(
            body["nextPageToken"].as_str(),
            (remaining > PAGE_SIZE).then(|| all[index + PAGE_SIZE].as_str()),
            "case resuming from {token}: expected the next token to name the first record of \
             the page after it, got {body:?}"
        );
    }
}

/// RULE (the same one, on a NON-empty account): because the token names a
/// record rather than indexing one, a token that names NO stored record resumes
/// at the first stored name after it, and one past every stored name reports
/// nothing. This is what keeps a page whose first record was deleted between
/// two calls from skipping the records behind it.
#[tokio::test(flavor = "current_thread")]
async fn a_token_naming_no_stored_record_resumes_at_the_next_stored_name() {
    let emulator = Emulator::new();
    let row = Row {
        case: "an unstored token",
        suffix: "gap.example",
        count: PAGE_SIZE + 2,
    };
    let all = create_row(&emulator, &row).await;

    // One token per gap in the stored order: `z07.gap.example` is stored, so
    // `z07a.gap.example` names nothing and sits strictly between it and
    // `z08.gap.example`. The empty token is the one that precedes every name.
    let mut cases: Vec<(String, usize)> = vec![(String::new(), 0)];
    for index in 0..all.len() {
        cases.push((format!("z{index:02}a.{}", row.suffix), index + 1));
    }
    // And a token past every stored name: nothing follows it.
    cases.push((format!("zz.{}", row.suffix), all.len()));

    for (token, first) in &cases {
        let body = page(&emulator, Some(token)).await;
        let reported = names(&body);
        let expected: Vec<String> = all[*first..].iter().take(PAGE_SIZE).cloned().collect();
        assert_eq!(
            reported, expected,
            "case a token naming nothing ({token:?}): expected the stored names from \
             {first} onward, got {reported:?}"
        );
        let remaining = all.len() - *first;
        assert_eq!(
            body["nextPageToken"].as_str(),
            (remaining > PAGE_SIZE).then(|| all[first + PAGE_SIZE].as_str()),
            "case a token naming nothing ({token:?}): expected the next token to name the \
             first record of the page after it, got {body:?}"
        );
    }
}

/// RULE: the listing answers from the store, so every later create, recordset
/// edit and delete is visible in it — nothing is cached and nothing is canned.
#[tokio::test(flavor = "current_thread")]
async fn the_listing_follows_creates_edits_and_deletes() {
    let emulator = Emulator::new();
    let row = Row {
        case: "mutating the account",
        suffix: "mutate.example",
        count: PAGE_SIZE + 1,
    };
    let mut expected = create_row(&emulator, &row).await;

    // A zone created after the first listing is in the next one, in its name
    // order position rather than at the end.
    let added = format!("a-new.{}", row.suffix);
    emulator
        .ok("CreateDomain", json!({"domainName": added}))
        .await;
    expected.push(added.clone());
    expected.sort();
    let (records, _) = walk(&emulator, "after a create", 5).await;
    assert_eq!(
        records
            .iter()
            .map(|domain| domain["name"].as_str().unwrap_or_default().to_string())
            .collect::<Vec<String>>(),
        expected,
        "case after a create: expected the new zone to be listed in name order, got {records:?}"
    );

    // An entry appended to a zone is reported on that zone's listed record.
    let entry_name = format!("www.{added}");
    emulator
        .ok(
            "CreateDomainEntry",
            json!({
                "domainName": added,
                "domainEntry": {"name": entry_name, "type": "A", "target": "192.0.2.77"},
            }),
        )
        .await;
    let (records, _) = walk(&emulator, "after an entry edit", 5).await;
    let listed = records
        .iter()
        .find(|domain| domain["name"].as_str() == Some(added.as_str()))
        .unwrap_or_else(|| panic!("case after an entry edit: expected {added} to be listed"));
    assert_eq!(
        listed["domainEntries"][0]["target"].as_str(),
        Some("192.0.2.77"),
        "case after an entry edit: expected the appended entry on the listed zone, got {listed:?}"
    );

    // A deleted zone leaves the listing, and the pages close up behind it.
    let removed = expected.remove(0);
    emulator
        .ok("DeleteDomain", json!({"domainName": removed}))
        .await;
    let (records, page_sizes) = walk(&emulator, "after a delete", 5).await;
    let reported: Vec<String> = records
        .iter()
        .map(|domain| domain["name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        reported, expected,
        "case after a delete: expected the deleted zone to be gone, got {reported:?}"
    );
    assert_eq!(
        page_sizes,
        expected_page_sizes(expected.len()),
        "case after a delete: expected full pages until the remainder, got {page_sizes:?}"
    );
}
