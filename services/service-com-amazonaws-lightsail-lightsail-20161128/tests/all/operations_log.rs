// SPDX-License-Identifier: CC0-1.0

//! `GetOperations` — the account's WHOLE operation log, whatever resource each
//! entry was recorded against — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `GetOperations` reports exactly the operation-log
//! entries the store holds — every one the account's mutating calls recorded,
//! and nothing else — oldest first, in pages of
//! `crate::state::OPERATIONS_PAGE_SIZE` with a `nextPageToken` naming the first
//! entry of the page after it and none once no entries remain, whatever kind of
//! resource each entry names and however many entries there are. The log is
//! append-only, so an entry is still reported after the resource it names is
//! deleted; an account whose log holds nothing reports an EMPTY list rather than
//! a refusal; reading the log changes nothing a later caller sees; and a
//! `pageToken` that names no entry of the log is refused with the modelled
//! `InvalidInputException` (HTTP 400).

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_lightsail_lightsail_20161128::state::OPERATIONS_PAGE_SIZE;

// The harness, the wire error code and the minted-id check this test target
// already exposes: a sibling suite drives the service through the same front
// door rather than standing up a second one.
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};
// The same readers the per-resource read of this log is checked with, so both
// suites take a mutating answer apart exactly the same way.
use crate::operations_for_resource::{answered_entries, ids, operation_types};

/// The operation under test.
const OPERATION: &str = "GetOperations";

/// The per-resource read of the same log, which the account-wide read must be
/// the union of.
const PER_RESOURCE: &str = "GetOperationsForResource";

/// The zone every resource in this suite is created in; the operation log is not
/// regional in this emulator (an entry is reported by id and by name, never by
/// Region), so one zone serves every case.
const ZONE: &str = "us-east-1a";

/// A Linux/Unix blueprint and the smallest plan: neither selects anything about
/// the log.
const BLUEPRINT: &str = "amazon_linux_2023";
/// See [`BLUEPRINT`].
const BUNDLE: &str = "nano_3_0";

/// The smallest disk the service accepts
/// (`crate::state::MIN_DISK_SIZE_IN_GB`): the size selects nothing about the
/// log.
const DISK_SIZE_IN_GB: i64 = 8;

/// The first port a row opens; each further firewall step takes the next port,
/// so every `OpenInstancePublicPorts` call is a distinct change and therefore
/// its own log entry.
const FIRST_OPENED_PORT: i64 = 8_000;

/// The two modelled `OperationStatus` members an entry of this emulator's log
/// can report: `Started` while the clock still owes the work its terminal
/// status, `Succeeded` once it has settled
/// (`crate::state::OPERATION_STATUS_STARTED` /
/// `OPERATION_STATUS_SUCCEEDED`).
const OPERATION_STATUSES: &[&str] = &["Started", "Succeeded"];

/// One call a row makes, each recording exactly one operation-log entry.
#[derive(Debug, Clone, Copy)]
enum Step {
    /// Reserve a static IP — a terminal entry of resource type `StaticIp`.
    StaticIp,
    /// Create a block storage disk — an entry of resource type `Disk` that the
    /// clock still owes its terminal status when the call returns.
    Disk,
    /// Create the row's instance — an entry of resource type `Instance`.
    Instance,
    /// Open one more port on the row's instance.
    OpenPort,
    /// Create a DNS zone — an entry of resource type `Domain`.
    Domain,
    /// Delete the row's instance — an entry still reported under the name of a
    /// resource the account no longer holds.
    DeleteInstance,
}

impl Step {
    /// The modelled `OperationType` member this call records
    /// (`crate::state::OPERATION_TYPE_*`). `CreateInstances` records one entry
    /// per instance it creates, and the modelled member for it is the singular
    /// `CreateInstance`.
    fn operation_type(self) -> &'static str {
        match self {
            Step::StaticIp => "AllocateStaticIp",
            Step::Disk => "CreateDisk",
            Step::Instance => "CreateInstance",
            Step::OpenPort => "OpenInstancePublicPorts",
            Step::Domain => "CreateDomain",
            Step::DeleteInstance => "DeleteInstance",
        }
    }
}

/// One case: the calls an account makes before its whole log is read back.
///
/// The rows differ along everything the rule generalises over: the NUMBER of
/// entries the log holds — none, one, a handful, exactly one page, one past a
/// page and two pages past one — the KINDS of call that recorded them (and so
/// the modelled `ResourceType` and `OperationType` each entry reports), and
/// whether the resource an entry names still exists.
struct Row {
    case: &'static str,
    /// Every resource name this row creates starts with this, so no two rows
    /// can collide even though each runs against its own emulator.
    prefix: &'static str,
    steps: &'static [Step],
    /// Further `AllocateStaticIp` calls made after `steps`, which is how a row
    /// reaches a page boundary without needing a new kind of call.
    extra_static_ips: usize,
}

const ROWS: &[Row] = &[
    Row {
        case: "an account that has made no call",
        prefix: "ops-log-empty",
        steps: &[],
        extra_static_ips: 0,
    },
    Row {
        case: "one call",
        prefix: "ops-log-one",
        steps: &[Step::StaticIp],
        extra_static_ips: 0,
    },
    Row {
        case: "a handful of calls across resource kinds, one of them a delete",
        prefix: "ops-log-mixed",
        steps: &[
            Step::Instance,
            Step::OpenPort,
            Step::OpenPort,
            Step::Disk,
            Step::StaticIp,
            Step::Domain,
            Step::DeleteInstance,
        ],
        extra_static_ips: 0,
    },
    Row {
        case: "exactly one full page of entries",
        prefix: "ops-log-one-full-page",
        steps: &[Step::Instance],
        extra_static_ips: OPERATIONS_PAGE_SIZE - 1,
    },
    Row {
        case: "one entry past a page",
        prefix: "ops-log-one-past-a-page",
        steps: &[Step::Instance],
        extra_static_ips: OPERATIONS_PAGE_SIZE,
    },
    Row {
        case: "three pages, the last a partial one",
        prefix: "ops-log-three-pages",
        steps: &[Step::Instance, Step::Disk, Step::Domain],
        extra_static_ips: 2 * OPERATIONS_PAGE_SIZE,
    },
];

/// What a row's calls left behind.
struct Built {
    /// The entries those calls answered, in the order they were made.
    answered: Vec<Value>,
    /// The `operationType` each one must report, in the same order.
    types: Vec<String>,
    /// The resource names whose own per-resource log is cross-checked against
    /// the account-wide one. A domain name is left out: the modelled
    /// `ResourceName` pattern `^\w[\w\-]*\w$` admits no dot, so a domain's
    /// entries cannot be asked for by name at all.
    names: BTreeSet<String>,
}

/// Make `row`'s calls, answering what they recorded.
async fn build(emulator: &Emulator, row: &Row) -> Built {
    let case = row.case;
    let instance = format!("{}-instance", row.prefix);
    let mut built = Built {
        answered: Vec::new(),
        types: Vec::new(),
        names: BTreeSet::new(),
    };
    let mut opened_ports = 0i64;

    for (index, step) in row.steps.iter().enumerate() {
        let body = match step {
            Step::StaticIp => {
                let name = format!("{}-ip-{index}", row.prefix);
                let body = emulator
                    .ok("AllocateStaticIp", json!({"staticIpName": name}))
                    .await;
                built.names.insert(name);
                body
            }
            Step::Disk => {
                let name = format!("{}-disk-{index}", row.prefix);
                let body = emulator
                    .ok(
                        "CreateDisk",
                        json!({
                            "diskName": name,
                            "availabilityZone": ZONE,
                            "sizeInGb": DISK_SIZE_IN_GB,
                        }),
                    )
                    .await;
                built.names.insert(name);
                body
            }
            Step::Instance => {
                let body = emulator
                    .ok(
                        "CreateInstances",
                        json!({
                            "instanceNames": [&instance],
                            "availabilityZone": ZONE,
                            "blueprintId": BLUEPRINT,
                            "bundleId": BUNDLE,
                        }),
                    )
                    .await;
                built.names.insert(instance.clone());
                body
            }
            Step::OpenPort => {
                let port = FIRST_OPENED_PORT + opened_ports;
                opened_ports += 1;
                emulator
                    .ok(
                        "OpenInstancePublicPorts",
                        json!({
                            "instanceName": &instance,
                            "portInfo": {"fromPort": port, "toPort": port, "protocol": "tcp"},
                        }),
                    )
                    .await
            }
            Step::Domain => {
                emulator
                    .ok(
                        "CreateDomain",
                        json!({"domainName": format!("{}-{index}.example", row.prefix)}),
                    )
                    .await
            }
            Step::DeleteInstance => {
                emulator
                    .ok("DeleteInstance", json!({"instanceName": &instance}))
                    .await
            }
        };
        let entries = answered_entries(case, &body);
        // Each step is one call acting on one resource, so it records exactly
        // one entry — which is what makes the expected `operationType` sequence
        // below a statement about the log's order.
        assert_eq!(
            entries.len(),
            1,
            "case {case}: expected {step:?} to record exactly one log entry, got {entries:?}"
        );
        built.answered.extend(entries);
        built.types.push(step.operation_type().to_string());
    }

    for index in 0..row.extra_static_ips {
        let name = format!("{}-extra-{index}", row.prefix);
        let body = emulator
            .ok("AllocateStaticIp", json!({"staticIpName": name}))
            .await;
        built.answered.extend(answered_entries(case, &body));
        built.types.push(Step::StaticIp.operation_type().to_string());
    }

    built
}

/// The page sizes a walk over `count` entries must report: full pages while
/// entries remain, and one last page with the rest — a single empty page when
/// the log holds nothing at all.
fn expected_page_sizes(count: usize) -> Vec<usize> {
    if count == 0 {
        return vec![0];
    }
    let mut sizes = Vec::new();
    let mut remaining = count;
    while remaining > 0 {
        let page = remaining.min(OPERATIONS_PAGE_SIZE);
        sizes.push(page);
        remaining -= page;
    }
    sizes
}

/// Every entry `GetOperations` reports, walking every page: the entries in page
/// order, together with the size of each page.
///
/// Checks the paging invariants every page must satisfy on the way: no page is
/// larger than one page, and a `nextPageToken` names exactly the first entry of
/// the page after it.
///
/// `pub(crate)` so a sibling suite in this test target reads the account-wide log
/// through this ONE walk rather than carrying a second copy of it.
pub(crate) async fn walk(emulator: &Emulator, case: &str) -> (Vec<Value>, Vec<usize>) {
    let mut entries: Vec<Value> = Vec::new();
    let mut page_sizes = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let body = match &token {
            Some(token) => emulator.ok(OPERATION, json!({"pageToken": token})).await,
            None => emulator.ok(OPERATION, json!({})).await,
        };
        let page = body["operations"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {body:?}"))
            .clone();
        assert!(
            page.len() <= OPERATIONS_PAGE_SIZE,
            "case {case}: expected at most {OPERATIONS_PAGE_SIZE} entries in a page, got {}",
            page.len()
        );
        // The token this page was fetched with must name the page's FIRST entry:
        // a page starts at the entry its token names.
        if let Some(token) = &token {
            assert_eq!(
                page.first().and_then(|entry| entry["id"].as_str()),
                Some(token.as_str()),
                "case {case}: expected the page to start at the entry its token names, got {page:?}"
            );
        }
        page_sizes.push(page.len());
        entries.extend(page);
        match body["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
        assert!(
            page_sizes.len() < 100,
            "case {case}: the walk did not terminate after {} pages",
            page_sizes.len()
        );
    }
    (entries, page_sizes)
}

/// Every entry the per-resource read reports for `name`, walking its pages.
///
/// `pub(crate)` for the same reason [`walk`] is.
pub(crate) async fn walk_one_resource(emulator: &Emulator, case: &str, name: &str) -> Vec<Value> {
    let mut entries: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let request = match &token {
            Some(token) => json!({"resourceName": name, "pageToken": token}),
            None => json!({"resourceName": name}),
        };
        let body = emulator.ok(PER_RESOURCE, request).await;
        let page = body["operations"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case}: expected an operations array for {name}, got {body:?}")
            })
            .clone();
        entries.extend(page);
        match body["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return entries,
        }
    }
}

/// Assert `entry` is a well-formed log entry, reported identically when asked
/// for by its own id.
async fn assert_entry(emulator: &Emulator, case: &str, entry: &Value) {
    let id = entry["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected an id on {entry:?}"));
    assert!(
        is_uuid_shaped(id),
        "case {case}: expected a UUID-shaped operation id, got {id:?}"
    );
    assert!(
        entry["resourceName"].as_str().is_some_and(|name| !name.is_empty()),
        "case {case}: expected the entry to name a resource, got {entry:?}"
    );
    assert!(
        entry["resourceType"].as_str().is_some_and(|kind| !kind.is_empty()),
        "case {case}: expected the entry to report a resource type, got {entry:?}"
    );
    let status = entry["status"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a status on {entry:?}"));
    assert!(
        OPERATION_STATUSES.contains(&status),
        "case {case}: expected one of {OPERATION_STATUSES:?}, got {status:?}"
    );
    assert_eq!(
        entry["isTerminal"].as_bool(),
        Some(status == "Succeeded"),
        "case {case}: expected isTerminal to agree with the status {status:?}, got {entry:?}"
    );
    assert!(
        entry["createdAt"].is_number(),
        "case {case}: expected an epoch-seconds createdAt, got {:?}",
        entry["createdAt"]
    );
    assert!(
        entry["location"]["regionName"].is_string(),
        "case {case}: expected the entry's location to name a region, got {entry:?}"
    );

    // The same entry, reached the other way round: by its id. The members a
    // settle cannot change must agree — `status` and `statusChangedAt` are left
    // out, because the clock may settle the entry BETWEEN the two reads.
    let by_id = emulator
        .ok("GetOperation", json!({"operationId": id}))
        .await["operation"]
        .clone();
    for member in [
        "id",
        "resourceName",
        "resourceType",
        "operationType",
        "createdAt",
    ] {
        assert_eq!(
            by_id[member], entry[member],
            "case {case}: GetOperation and {OPERATION} must agree on {member}, got {by_id:?} and \
             {entry:?}"
        );
    }
}

/// Poll `GetOperations` until every entry it reports is terminal, answering
/// them.
///
/// The transitions are owed by the clock, not by the poll: they arrive without
/// anyone asking more than once.
async fn wait_until_settled(emulator: &Emulator, case: &str) -> Vec<Value> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (entries, _) = walk(emulator, case).await;
        if entries
            .iter()
            .all(|entry| entry["isTerminal"].as_bool() == Some(true))
        {
            return entries;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: expected every entry to settle within 5s, got {entries:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// RULE: the read reports exactly the entries the account's calls recorded —
/// every one of them, nothing else, oldest first, paged — and it is the union of
/// the per-resource reads of the same log.
#[tokio::test]
async fn the_log_reports_every_entry_the_account_recorded() {
    for row in ROWS {
        let case = row.case;
        // The log is the ACCOUNT's, so a row that pins how many entries it
        // holds has to be the only account in its emulator.
        let emulator = Emulator::new();
        let built = build(&emulator, row).await;

        let (reported, page_sizes) = walk(&emulator, case).await;
        assert_eq!(
            ids(&reported),
            ids(&built.answered),
            "case {case}: expected exactly the entries the mutating calls answered, in the order \
             they were made, got {} entries",
            reported.len()
        );
        assert_eq!(
            operation_types(&reported),
            built.types,
            "case {case}: expected one entry per call, oldest first"
        );
        assert_eq!(
            page_sizes,
            expected_page_sizes(built.answered.len()),
            "case {case}: expected {} entries to page as {:?}, got {page_sizes:?}",
            built.answered.len(),
            expected_page_sizes(built.answered.len())
        );
        // `createdAt` never decreases along the reported order: oldest first.
        let created_at: Vec<f64> = reported
            .iter()
            .map(|entry| entry["createdAt"].as_f64().unwrap_or_default())
            .collect();
        assert!(
            created_at.windows(2).all(|pair| pair[0] <= pair[1]),
            "case {case}: expected the entries oldest first, got {created_at:?}"
        );
        // Every id is distinct: no entry is reported twice by a paged walk.
        let distinct: BTreeSet<String> = ids(&reported).into_iter().collect();
        assert_eq!(
            distinct.len(),
            reported.len(),
            "case {case}: expected every reported entry to appear once, got {} of {}",
            distinct.len(),
            reported.len()
        );
        for entry in &reported {
            assert_entry(&emulator, case, entry).await;
        }

        // The account-wide log is the union of the per-resource logs: what each
        // resource reports is exactly this read's entries that name it.
        for name in &built.names {
            let per_resource = walk_one_resource(&emulator, case, name).await;
            let expected: Vec<Value> = reported
                .iter()
                .filter(|entry| entry["resourceName"].as_str() == Some(name.as_str()))
                .cloned()
                .collect();
            assert_eq!(
                ids(&per_resource),
                ids(&expected),
                "case {case}: expected {OPERATION} to report exactly {PER_RESOURCE}'s entries for \
                 {name}, got {per_resource:?}"
            );
            assert!(
                !expected.is_empty(),
                "case {case}: expected the log to hold an entry naming {name}"
            );
        }
    }
}

/// RULE: an account whose log holds nothing reports an EMPTY list — the caller
/// asked which operations it has made, and the answer is "none" — rather than a
/// refusal or an absent member, and no page token, because there is no further
/// page.
#[tokio::test]
async fn an_account_that_has_made_no_call_reports_an_empty_list() {
    let emulator = Emulator::new();

    // Rows differ along the ways a caller can ask for the first page: no token
    // member at all, and the empty token that names no entry and so asks for
    // the first page too.
    for request in [json!({}), json!({"pageToken": ""})] {
        let body = emulator.ok(OPERATION, request.clone()).await;
        assert_eq!(
            body["operations"].as_array().map(Vec::len),
            Some(0),
            "case {request:?}: expected an empty operations list, got {body:?}"
        );
        assert!(
            body["nextPageToken"].is_null(),
            "case {request:?}: expected no nextPageToken with nothing to report, got {body:?}"
        );
    }

    // One call later the same read reports it, so the empty answer above was
    // the log being empty rather than the read reporting nothing.
    let answered = emulator
        .ok("AllocateStaticIp", json!({"staticIpName": "ops-log-first-call"}))
        .await;
    let body = emulator.ok(OPERATION, json!({})).await;
    assert_eq!(
        ids(body["operations"].as_array().expect("an operations array")),
        ids(&answered_entries("the first call", &answered)),
        "case the first call: expected the one entry it recorded, got {body:?}"
    );
}

/// RULE: reading the log is reporting only — it changes nothing a later caller
/// sees, however many times and however it is paged.
#[tokio::test]
async fn reading_the_log_does_not_change_what_a_later_caller_sees() {
    let emulator = Emulator::new();
    let case = "an account past one page of entries";
    let row = Row {
        case,
        prefix: "ops-log-reporting-only",
        steps: &[Step::Instance, Step::Disk],
        extra_static_ips: OPERATIONS_PAGE_SIZE,
    };
    let built = build(&emulator, &row).await;

    // Once every entry has settled, nothing the clock owes can differ between
    // two reads, so the whole answer must repeat verbatim.
    let settled = wait_until_settled(&emulator, case).await;
    assert_eq!(
        ids(&settled),
        ids(&built.answered),
        "case {case}: expected the settled log to hold exactly the entries the calls answered"
    );

    let first = emulator.ok(OPERATION, json!({})).await;
    let again = emulator.ok(OPERATION, json!({})).await;
    assert_eq!(
        first, again,
        "case {case}: expected a repeated read to answer identically"
    );

    let (walked, page_sizes) = walk(&emulator, case).await;
    let after_the_walk = emulator.ok(OPERATION, json!({})).await;
    assert_eq!(
        first, after_the_walk,
        "case {case}: expected a read after a paged walk to answer identically"
    );
    assert_eq!(
        ids(&walked),
        ids(&settled),
        "case {case}: expected the paged walk to report exactly the same entries"
    );
    assert_eq!(
        page_sizes,
        expected_page_sizes(built.answered.len()),
        "case {case}: expected {} entries to page as {:?}, got {page_sizes:?}",
        built.answered.len(),
        expected_page_sizes(built.answered.len())
    );
}

/// RULE: a page starts at the entry its token names, so the token of any entry
/// answers that entry and the ones recorded after it.
#[tokio::test]
async fn a_page_starts_at_the_entry_its_token_names() {
    let emulator = Emulator::new();
    let case = "an account with a handful of entries";
    let row = Row {
        case,
        prefix: "ops-log-token-positions",
        steps: &[
            Step::Instance,
            Step::OpenPort,
            Step::Disk,
            Step::StaticIp,
            Step::DeleteInstance,
        ],
        extra_static_ips: 0,
    };
    let built = build(&emulator, &row).await;
    let recorded = ids(&built.answered);

    // Rows differ along the POSITION the token names: the first entry, each
    // middle one, and the last.
    for (position, token) in recorded.iter().enumerate() {
        let body = emulator.ok(OPERATION, json!({"pageToken": token})).await;
        let page = body["operations"]
            .as_array()
            .unwrap_or_else(|| panic!("case position {position}: expected an array, got {body:?}"));
        assert_eq!(
            ids(page),
            recorded[position..].to_vec(),
            "case position {position}: expected the entry its token names and every later one, \
             got {page:?}"
        );
        assert!(
            body["nextPageToken"].is_null(),
            "case position {position}: expected no nextPageToken with the rest of the log in one \
             page, got {body:?}"
        );
    }
}

/// RULE: a `pageToken` that names no entry of the log names no page, so it is
/// refused with the modelled `InvalidInputException` (HTTP 400), and the refusal
/// changes nothing.
#[tokio::test]
async fn a_page_token_naming_no_entry_is_refused() {
    let emulator = Emulator::new();
    let case = "an account with entries";
    let row = Row {
        case,
        prefix: "ops-log-bad-token",
        steps: &[Step::Instance, Step::StaticIp],
        extra_static_ips: 0,
    };
    let built = build(&emulator, &row).await;

    // Rows differ along the ways a token can fail to name an entry: an opaque
    // cursor from elsewhere, a UUID-shaped id nothing minted, an index where an
    // id belongs, blank-but-not-empty, and a real id with one character changed.
    let mutated = {
        let first = ids(&built.answered)
            .first()
            .expect("the row recorded an entry")
            .clone();
        // A real id that names nothing once its last character is rotated.
        let (head, last) = first.split_at(first.len() - 1);
        let rotated = if last == "0" { "1" } else { "0" };
        format!("{head}{rotated}")
    };
    for token in [
        "not-a-page-token",
        "00000000-0000-0000-0000-000000000000",
        "1",
        " ",
        mutated.as_str(),
    ] {
        let (status, body) = emulator.call(OPERATION, json!({"pageToken": token})).await;
        assert_eq!(
            status, 400,
            "case pageToken {token:?}: expected HTTP 400, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case pageToken {token:?}: expected the modelled InvalidInputException, got {body:?}"
        );
    }

    // Every refusal left the log exactly as it was.
    let (reported, _) = walk(&emulator, "after the refusals").await;
    assert_eq!(
        ids(&reported),
        ids(&built.answered),
        "case after the refusals: expected the log untouched, got {reported:?}"
    );

    // An empty log has no entry for ANY token to name, so a token is refused
    // there too rather than answering the first page.
    let empty = Emulator::new();
    let (status, body) = empty
        .call(OPERATION, json!({"pageToken": "not-a-page-token"}))
        .await;
    assert_eq!(
        status, 400,
        "case an empty log: expected HTTP 400, got {status} with {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "case an empty log: expected the modelled InvalidInputException, got {body:?}"
    );
}
