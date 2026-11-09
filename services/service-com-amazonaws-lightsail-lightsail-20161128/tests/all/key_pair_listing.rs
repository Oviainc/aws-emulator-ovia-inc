// SPDX-License-Identifier: CC0-1.0

//! `GetKeyPairs` — the paged listing of the SSH key pairs an account holds —
//! driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handler and the awsJson envelope are all exercised rather than bypassed. The
//! `Authorization` header carries only a credential SCOPE, which is what names
//! the Region a request is made in.
//!
//! The rules under test:
//!
//! * A listing reports every record the store holds of that resource type, each
//!   entry exactly as the single-record read reports it, in the store's own name
//!   order, carried one page at a time, with `nextPageToken` NAMING the first
//!   record of the page after it while records remain and ABSENT on the last
//!   page — and, being a read, it reports the same records however often and
//!   from whichever Region it is called.
//! * The account's Lightsail DEFAULT key pair is reported only when the request
//!   asks for it (`includeDefaultKeyPair`), and leaving it out costs no room in a
//!   page: the pages of the others stay full.
//!
//! The rows span what those rules generalise over: how many key pairs the account
//! holds (none, one, exactly a page, one past a page, several pages and a
//! remainder), whether it holds the default pair at all, the value of the switch
//! (absent, false, true), the Region each record was created in, the instant it
//! was created at, how many tags it carries (none, one, two) and the FORM of the
//! fingerprint — the MD5 colon-hex form `ssh-keygen -E md5` prints and the
//! `SHA256:`-prefixed form it prints by default — because this operation reports
//! the fingerprint and never parses it.
//!
//! The rows below hold key pairs with chosen ids, instants and fingerprint
//! forms, none of which a create lets a caller pick — `CreateKeyPair` mints its
//! own material, and `ImportKeyPair` and `DownloadDefaultKeyPair` are still
//! scaffold stubs with issues of their own — so seeding the store the handler
//! reads (`state::with_state`) is how a case drives the account HOLDING key
//! pairs, exactly as the sibling `key_pair_read` and `key_pair_delete` suites
//! do. Everything the listing answers is asserted through the wire.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::ops::types::Tag;
use service_com_amazonaws_lightsail_lightsail_20161128::state::{
    self, StoredKeyPair, StoredLocation, DEFAULT_SSH_KEY_NAME,
};

use crate::instance_lifecycle::{error_code, Emulator};
use crate::paging::{expected_page_sizes, names};

/// How many key pairs one page of `GetKeyPairs` carries — the handler's OWN
/// constant, so the table's rows keep straddling the page boundary whatever it is
/// set to.
const PAGE_SIZE: usize = state::KEY_PAIR_PAGE_SIZE;

/// A walk over the pages of one listing cannot need more pages than this:
/// anything past it means the walk is not advancing.
const MAX_PAGES: usize = 20;

/// The values `includeDefaultKeyPair` takes: the member absent, explicitly
/// false, and explicitly true. An absent boolean and an explicit `false` ask the
/// same thing.
const INCLUDE_DEFAULT: &[Option<bool>] = &[None, Some(false), Some(true)];

// ---------------------------------------------------------------------------
// The accounts the rows describe
// ---------------------------------------------------------------------------

/// One account's worth of key pairs: how many of its own it holds, whether it
/// also holds the Lightsail default pair, and the name fragment that tells this
/// row's records apart.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    fragment: &'static str,
    count: usize,
    holds_default: bool,
}

/// The table. The counts straddle every page boundary the rule can turn on, both
/// with and without the default pair in the store.
const ROWS: &[Row] = &[
    Row {
        case: "an account holding nothing",
        fragment: "empty",
        count: 0,
        holds_default: false,
    },
    Row {
        case: "an account holding only the default pair",
        fragment: "onlydefault",
        count: 0,
        holds_default: true,
    },
    Row {
        case: "an account holding one of its own",
        fragment: "solo",
        count: 1,
        holds_default: false,
    },
    Row {
        case: "exactly one page of its own, beside the default pair",
        fragment: "full",
        count: PAGE_SIZE,
        holds_default: true,
    },
    Row {
        case: "one past a page, no default pair",
        fragment: "spill",
        count: PAGE_SIZE + 1,
        holds_default: false,
    },
    Row {
        case: "several pages and a remainder, beside the default pair",
        fragment: "wide",
        count: 2 * PAGE_SIZE + 3,
        holds_default: true,
    },
];

/// The name of this row's key pair `index`.
///
/// Half the names sort BEFORE the default pair's name and half AFTER it (ASCII
/// `A` < `L` < `z`), so the default pair sits INSIDE the listing rather than at
/// one end and a page boundary can fall on either side of it. The index is
/// zero-padded, so name order is index order within each half.
fn name_of(row: &Row, index: usize) -> String {
    let lead = if index.is_multiple_of(2) { 'A' } else { 'z' };
    format!("{lead}kp-{}-{index:02}", row.fragment)
}

/// One key pair the account holds, as the call that created or imported it would
/// have left it.
#[derive(Debug, Clone)]
struct Held {
    name: String,
    region: String,
    id: String,
    created_at_ms: i64,
    fingerprint: String,
    tags: Vec<(String, String)>,
}

impl Held {
    /// Whether this is the account's Lightsail default key pair — the record the
    /// switch decides about.
    fn is_default(&self) -> bool {
        self.name == DEFAULT_SSH_KEY_NAME
    }
}

/// What one key pair of this row carries, derived from its index so neighbouring
/// records differ in Region, creation instant, tag count and fingerprint form —
/// a canned body could not satisfy them all.
fn held(index: usize, name: String, regions: &[String]) -> Held {
    let tags: Vec<(String, String)> = match index % 3 {
        0 => Vec::new(),
        1 => vec![("env".to_string(), format!("stage-{index:02}"))],
        _ => vec![
            ("env".to_string(), format!("stage-{index:02}")),
            ("team".to_string(), "ssh".to_string()),
        ],
    };
    // The two forms `ssh-keygen` prints a public key's fingerprint in: the MD5
    // colon-hex form (`-E md5`) and the `SHA256:`-prefixed default.
    let fingerprint = if index.is_multiple_of(2) {
        format!(
            "a{:x}:1f:3c:d2:0b:8e:74:59:c1:3d:ef:90:12:ab:cd:{:02x}",
            index % 16,
            index % 256
        )
    } else {
        format!("SHA256:hXy0pZ9Qw3kLmNoPqRsTuVwXyZaBcDeFgH{index:03}")
    };
    Held {
        name,
        region: regions[index % regions.len()].clone(),
        id: format!("05859e3d-331d-48ba-9034-{:012x}", index + 1),
        created_at_ms: 1_700_000_000_000 + (index as i64) * 97_531,
        fingerprint,
        tags,
    }
}

/// Reads the service's own Region catalog through `GetRegions`, answering the
/// Region names it publishes.
///
/// The rows hold one key pair per Region out of this list, so neighbouring
/// entries carry different locations and ARNs and nothing here is hand-written.
async fn published_regions(emulator: &Emulator) -> Vec<String> {
    let body = emulator.ok("GetRegions", json!({})).await;
    let regions: Vec<String> = body["regions"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case reading the Region catalog: expected an array, got {body:?}")
        })
        .iter()
        .filter_map(|region| region["name"].as_str().map(str::to_string))
        .collect();
    assert!(
        !regions.is_empty(),
        "case reading the Region catalog: expected a non-empty list of regions, got {body:?}"
    );
    regions
}

/// Put every key pair this row describes in the store, under the ids, instants
/// and fingerprint forms the row names, answering them sorted by name.
async fn hold(emulator: &Emulator, row: &Row) -> Vec<Held> {
    let regions = published_regions(emulator).await;

    let mut records: Vec<Held> = (0..row.count)
        .map(|index| held(index, name_of(row, index), &regions))
        .collect();
    if row.holds_default {
        records.push(held(
            row.count,
            DEFAULT_SSH_KEY_NAME.to_string(),
            &regions,
        ));
    }
    records.sort_by(|left, right| left.name.cmp(&right.name));

    // Seeded in REVERSE name order, so a listing that echoed the sequence the
    // records were written in rather than the store's own order would fail.
    let seeded = state::with_state(emulator.engine(), |store| {
        for record in records.iter().rev() {
            store.insert_key_pair(StoredKeyPair {
                name: record.name.clone(),
                arn: state::key_pair_arn(&record.region, &record.id),
                support_code: state::support_code(&record.id),
                created_at_ms: record.created_at_ms,
                location: StoredLocation::regional(&record.region),
                tags: record
                    .tags
                    .iter()
                    .map(|(key, value)| Tag {
                        key: Some(key.clone()),
                        value: Some(value.clone()),
                    })
                    .collect(),
                fingerprint: record.fingerprint.clone(),
            });
        }
        store.key_pairs.len()
    });
    assert_eq!(
        seeded,
        Some(records.len()),
        "case {}: expected the store to hold {} key pairs, got {seeded:?}",
        row.case,
        records.len()
    );
    records
}

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// One `GetKeyPairs` page, resuming from `page_token` when one is given, asking
/// for the default pair when `include_default` is given, and made in `region`
/// when one is given.
async fn page(
    emulator: &Emulator,
    region: Option<&str>,
    include_default: Option<bool>,
    page_token: Option<&str>,
) -> Value {
    let mut request = serde_json::Map::new();
    if let Some(token) = page_token {
        request.insert("pageToken".to_string(), json!(token));
    }
    if let Some(include) = include_default {
        request.insert("includeDefaultKeyPair".to_string(), json!(include));
    }
    emulator
        .ok_at(None, region, "GetKeyPairs", Value::Object(request))
        .await
}

/// The entries a page reports, in the order it reports them.
fn entries(page: &Value) -> Vec<Value> {
    page["keyPairs"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a keyPairs array, got {page:?}"))
        .clone()
}

/// Walk every page of this listing, answering the records in the order reported
/// and the size of each page, checking the paging contract as it goes: no page
/// carries more than a page's worth, a `nextPageToken` names the FIRST record of
/// the page after it, a page that is not the last one is full, and the walk
/// terminates because the token is absent rather than because it ran out of
/// patience.
async fn walk(
    emulator: &Emulator,
    case: &str,
    region: Option<&str>,
    include_default: Option<bool>,
) -> (Vec<Value>, Vec<usize>) {
    let mut records: Vec<Value> = Vec::new();
    let mut page_sizes = Vec::new();
    let mut token: Option<String> = None;
    for page_index in 0..MAX_PAGES {
        let body = page(emulator, region, include_default, token.as_deref()).await;
        let reported = entries(&body);
        assert!(
            reported.len() <= PAGE_SIZE,
            "case {case}: page {page_index} carries more than the {PAGE_SIZE} of a page, got \
             {reported:?}"
        );
        if let Some(token) = &token {
            assert_eq!(
                reported.first().and_then(|record| record["name"].as_str()),
                Some(token.as_str()),
                "case {case}: a page token names the first record of the page after it, got \
                 {reported:?}"
            );
        }
        page_sizes.push(reported.len());
        records.extend(reported.iter().cloned());
        match body["nextPageToken"].as_str() {
            Some(next) => {
                assert_eq!(
                    reported.len(),
                    PAGE_SIZE,
                    "case {case}: a page that is not the last one is full, got {reported:?}"
                );
                token = Some(next.to_string());
            }
            None => return (records, page_sizes),
        }
    }
    panic!("case {case}: the walk did not terminate within {MAX_PAGES} pages");
}

/// The stored key pair `name`, as `GetKeyPair` reports it.
async fn read_key_pair(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok("GetKeyPair", json!({"keyPairName": name}))
        .await["keyPair"]
        .clone()
}

/// Every operation-log entry the account holds, as `GetOperations` reports it,
/// walking every page.
async fn operation_log(emulator: &Emulator) -> Vec<Value> {
    let mut entries = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let body = match &token {
            Some(token) => {
                emulator
                    .ok("GetOperations", json!({"pageToken": token}))
                    .await
            }
            None => emulator.ok("GetOperations", json!({})).await,
        };
        entries.extend(body["operations"].as_array().cloned().unwrap_or_default());
        match body["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    entries
}

/// The names this row's listing must report for `include_default`, in the store's
/// name order.
fn expected_names(records: &[Held], include_default: Option<bool>) -> Vec<String> {
    records
        .iter()
        .filter(|record| include_default.unwrap_or(false) || !record.is_default())
        .map(|record| record.name.clone())
        .collect()
}

// ---------------------------------------------------------------------------
// The listing table
// ---------------------------------------------------------------------------

/// RULE: `GetKeyPairs` reports every key pair the account holds, in the store's
/// own name order, each entry exactly as `GetKeyPair` reports it, a page at a
/// time — and the account's default key pair is among them only when the request
/// asks for it.
#[tokio::test(flavor = "current_thread")]
async fn get_key_pairs_reports_every_held_key_pair_in_name_order() {
    for row in ROWS {
        for include_default in INCLUDE_DEFAULT {
            // One account per case: the store keys a key pair by name alone, so a
            // case reads a store holding exactly what it seeded.
            let emulator = Emulator::new();
            let records = hold(&emulator, row).await;
            let case = format!("{} with includeDefaultKeyPair {include_default:?}", row.case);

            // The listing is made in ONE Region, whatever Region each record was
            // created in: a key pair belongs to the account.
            let (reported, page_sizes) =
                walk(&emulator, &case, Some("us-east-1"), *include_default).await;

            let expected = expected_names(&records, *include_default);
            let listed = names(&reported);
            assert_eq!(
                listed, expected,
                "case {case}: expected every reported name in name order, got {listed:?}"
            );

            // Each record is reported once; a page boundary neither repeats nor
            // skips one.
            let distinct: BTreeSet<&String> = listed.iter().collect();
            assert_eq!(
                distinct.len(),
                listed.len(),
                "case {case}: expected every record once, got {listed:?}"
            );

            // The pages are full until the remainder — leaving the default pair
            // out costs no room in a page.
            assert_eq!(
                page_sizes,
                expected_page_sizes(expected.len(), PAGE_SIZE),
                "case {case}: expected full pages until the remainder, got {page_sizes:?}"
            );

            for (entry, want) in reported.iter().zip(&expected) {
                let record = records
                    .iter()
                    .find(|record| &record.name == want)
                    .unwrap_or_else(|| panic!("case {case}: {want} is a seeded record"));

                // Every listed entry is exactly what the single-record read
                // reports: one stored record, two operations.
                let single = read_key_pair(&emulator, want).await;
                assert_eq!(
                    entry, &single,
                    "case {case}: the listing of {want} must match GetKeyPair, got {entry:?}"
                );

                // And it carries THIS record's own values, not a canned body.
                assert_eq!(
                    entry["location"]["regionName"].as_str(),
                    Some(record.region.as_str()),
                    "case {case}: expected {want} to report its own Region, got {entry:?}"
                );
                assert_eq!(
                    entry["arn"].as_str(),
                    Some(state::key_pair_arn(&record.region, &record.id).as_str()),
                    "case {case}: expected {want} to report its stored ARN, got {entry:?}"
                );
                assert_eq!(
                    entry["supportCode"].as_str(),
                    Some(state::support_code(&record.id).as_str()),
                    "case {case}: expected {want} to report its stored support code, got {entry:?}"
                );
                assert_eq!(
                    entry["createdAt"].as_f64(),
                    Some(record.created_at_ms as f64 / 1_000.0),
                    "case {case}: expected {want} to report its creation instant in epoch \
                     seconds, got {entry:?}"
                );
                assert_eq!(
                    entry["fingerprint"].as_str(),
                    Some(record.fingerprint.as_str()),
                    "case {case}: expected {want} to report its stored fingerprint verbatim, got \
                     {entry:?}"
                );
                assert_eq!(
                    entry["resourceType"].as_str(),
                    Some("KeyPair"),
                    "case {case}: expected {want} to report its resource type, got {entry:?}"
                );
                let tags: Value = record
                    .tags
                    .iter()
                    .map(|(key, value)| json!({"key": key, "value": value}))
                    .collect();
                assert_eq!(
                    entry["tags"], tags,
                    "case {case}: expected {want} to report its stored tags, got {entry:?}"
                );
            }

            // The default pair is reported only when the request asked for it,
            // whichever page it would fall on.
            assert_eq!(
                listed.iter().any(|name| name == DEFAULT_SSH_KEY_NAME),
                row.holds_default && include_default.unwrap_or(false),
                "case {case}: the default key pair is reported only when the request asks for \
                 it, got {listed:?}"
            );
        }
    }
}

/// RULE: a listing is a READ — it reports the same records however often it is
/// called and from whichever Region, leaves the store as it was and writes no
/// operation-log entry.
#[tokio::test(flavor = "current_thread")]
async fn listing_again_and_from_another_region_changes_nothing() {
    let emulator = Emulator::new();
    let row = Row {
        case: "repeated listing",
        fragment: "repeat",
        count: PAGE_SIZE + 2,
        holds_default: true,
    };
    let records = hold(&emulator, &row).await;
    let before_log = operation_log(&emulator).await;

    for include_default in INCLUDE_DEFAULT {
        let case = format!("repeated listing with includeDefaultKeyPair {include_default:?}");
        let (first, first_sizes) = walk(&emulator, &case, Some("us-east-1"), *include_default).await;
        let (second, second_sizes) =
            walk(&emulator, &case, Some("us-east-1"), *include_default).await;
        assert_eq!(
            first, second,
            "case {case}: a read must not change what a later caller sees, got {second:?}"
        );
        assert_eq!(
            first_sizes, second_sizes,
            "case {case}: expected the same paging, got {second_sizes:?}"
        );

        // The same listing, made in another Region and in no Region at all.
        for region in [Some("ap-northeast-1"), None] {
            let (elsewhere, sizes) = walk(&emulator, &case, region, *include_default).await;
            assert_eq!(
                elsewhere, first,
                "case {case} in {region:?}: a key pair belongs to the account, so the same \
                 records must be reported, got {elsewhere:?}"
            );
            assert_eq!(
                sizes, first_sizes,
                "case {case} in {region:?}: expected the same paging, got {sizes:?}"
            );
        }
    }

    // Every seeded record is still held, exactly as it was.
    let still_held = state::with_state(emulator.engine(), |store| {
        store
            .key_pairs
            .values()
            .map(|record| format!("{:?}", record.view()))
            .collect::<Vec<String>>()
    });
    assert_eq!(
        still_held.as_ref().map(Vec::len),
        Some(records.len()),
        "case repeated listing: expected the store to hold every seeded record, got \
         {still_held:?}"
    );
    assert_eq!(
        operation_log(&emulator).await,
        before_log,
        "case repeated listing: expected a listing to log no operation"
    );
}

/// RULE: an account holding none of that resource reports an empty LIST with no
/// token, never an absent member — the caller asked what it holds and the answer
/// is "nothing". A page token names a record it does not hold, though, so a
/// request carrying one is refused instead.
#[tokio::test(flavor = "current_thread")]
async fn an_account_holding_nothing_reports_an_empty_list_and_no_token() {
    let emulator = Emulator::new();
    for include_default in INCLUDE_DEFAULT {
        for token in [None, Some("")] {
            let body = page(&emulator, None, *include_default, token).await;
            assert_eq!(
                entries(&body).len(),
                0,
                "case an empty account at {token:?} with {include_default:?}: expected an empty \
                 keyPairs array, got {body:?}"
            );
            assert!(
                body["nextPageToken"].is_null(),
                "case an empty account at {token:?} with {include_default:?}: a last page carries \
                 no nextPageToken, got {body:?}"
            );
        }

        let mut request = serde_json::Map::new();
        request.insert("pageToken".to_string(), json!("zzz-no-such-key-pair"));
        if let Some(include) = include_default {
            request.insert(
                "includeDefaultKeyPair".to_string(),
                json!(include),
            );
        }
        let (status, body) = emulator
            .call("GetKeyPairs", Value::Object(request))
            .await;
        assert_eq!(
            status, 400,
            "case an empty account at a token naming no key pair with {include_default:?}: \
             expected the invalid-input status, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case an empty account at a token naming no key pair with {include_default:?}: \
             expected the modelled InvalidInputException, got {body:?}"
        );
    }
}

/// RULE: a page token NAMES a record rather than carrying an index, so resuming
/// from the name of a record this listing REPORTS answers the reported records
/// from that name onward — the invariant that keeps a record created or deleted
/// between two pages from making the walk skip or repeat another one — while a
/// token naming a record this request leaves OUT names no page of it, so it is
/// refused.
#[tokio::test(flavor = "current_thread")]
async fn resuming_from_a_stored_name_reports_that_name_onward() {
    let emulator = Emulator::new();
    let row = Row {
        case: "resuming",
        fragment: "resume",
        count: PAGE_SIZE + 4,
        holds_default: true,
    };
    let records = hold(&emulator, &row).await;

    for include_default in INCLUDE_DEFAULT {
        // Resuming from EVERY seeded name, including the default pair's — which
        // this listing reports only when the request asks for it, so a token
        // naming it is a page of one request and a refusal of another.
        let reported = expected_names(&records, *include_default);
        for record in &records {
            let token = record.name.as_str();
            if !reported.iter().any(|name| name == token) {
                let mut request = serde_json::Map::new();
                request.insert("pageToken".to_string(), json!(token));
                if let Some(include) = include_default {
                    request.insert(
                        "includeDefaultKeyPair".to_string(),
                        json!(include),
                    );
                }
                let (status, body) = emulator
                    .call("GetKeyPairs", Value::Object(request))
                    .await;
                assert_eq!(
                    status, 400,
                    "case resuming from the unreported {token} with {include_default:?}: expected \
                     the invalid-input status, got {status} {body:?}"
                );
                assert_eq!(
                    error_code(&body),
                    "InvalidInputException",
                    "case resuming from the unreported {token} with {include_default:?}: expected \
                     the modelled InvalidInputException, got {body:?}"
                );
                continue;
            }
            let body = page(&emulator, None, *include_default, Some(token)).await;
            let listed = names(&entries(&body));
            let from = reported
                .iter()
                .position(|name| name.as_str() >= token)
                .unwrap_or(reported.len());
            let expected: Vec<String> = reported[from..].iter().take(PAGE_SIZE).cloned().collect();
            assert_eq!(
                listed, expected,
                "case resuming from {token} with {include_default:?}: expected the reported names \
                 from there onward, got {listed:?}"
            );
            let remaining = reported.len() - from;
            assert_eq!(
                body["nextPageToken"].as_str(),
                (remaining > PAGE_SIZE).then(|| reported[from + PAGE_SIZE].as_str()),
                "case resuming from {token} with {include_default:?}: expected the next token to \
                 name the first record of the page after it, got {body:?}"
            );
        }
    }
}

/// RULE: a listing answers the STORE rather than a snapshot taken once — a key
/// pair the account stops holding is gone from the next listing, and the entry
/// sits in its name's place in the store's order.
#[tokio::test(flavor = "current_thread")]
async fn a_deleted_key_pair_leaves_the_listing() {
    let emulator = Emulator::new();
    let row = Row {
        case: "deleting",
        fragment: "delete",
        count: 3,
        holds_default: false,
    };
    let records = hold(&emulator, &row).await;

    let before = names(&entries(&page(&emulator, None, None, None).await));
    assert_eq!(
        before,
        expected_names(&records, None),
        "case before the delete: expected every seeded name, got {before:?}"
    );

    let gone = &records[1];
    emulator
        .ok("DeleteKeyPair", json!({"keyPairName": gone.name}))
        .await;

    let after = names(&entries(&page(&emulator, None, None, None).await));
    let expected: Vec<String> = before
        .iter()
        .filter(|name| *name != &gone.name)
        .cloned()
        .collect();
    assert_eq!(
        after, expected,
        "case after the delete: expected the deleted name to be gone, got {after:?}"
    );
}
