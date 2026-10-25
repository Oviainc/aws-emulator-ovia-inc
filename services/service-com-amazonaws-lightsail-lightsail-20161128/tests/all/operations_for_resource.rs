// SPDX-License-Identifier: CC0-1.0

//! `GetOperationsForResource` — the operation log read back by the resource each
//! entry was recorded against — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: `GetOperationsForResource` reports exactly the
//! operation-log entries the store holds whose `resourceName` is the name the
//! request carries — oldest first, in pages of
//! `crate::state::OPERATIONS_FOR_RESOURCE_PAGE_SIZE` with a `nextPageToken` naming the first
//! entry of the page after it and none once no entries remain — whatever kind of
//! resource the name belongs to and however many entries it has. The log is
//! append-only, so an entry is still reported after the resource it names is
//! deleted, and a name the log holds nothing for reports an EMPTY list rather
//! than a refusal. Reading the log changes nothing a later caller sees,
//! `nextPageCount` is discontinued and therefore never reported, and a
//! `pageToken` that names no entry of the named resource — including one that
//! names an entry of a DIFFERENT resource — is refused with the modelled
//! `InvalidInputException` (HTTP 400).

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use service_com_amazonaws_lightsail_lightsail_20161128::state::OPERATIONS_FOR_RESOURCE_PAGE_SIZE;

// The harness, the wire error code and the minted-id check this test target
// already exposes: a sibling suite drives the service through the same front
// door rather than standing up a second one.
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The operation under test.
const OPERATION: &str = "GetOperationsForResource";

/// The zone every resource in this suite is created in; the operation log is not
/// regional in this emulator (an entry is reported by name, exactly as
/// `GetOperation` reports it by id), so one zone serves every case.
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

/// The first port a row opens; each further rule takes the next port, so every
/// `OpenInstancePublicPorts` call in a row is a distinct firewall change and
/// therefore its own log entry.
const FIRST_OPENED_PORT: i64 = 8_000;

/// The two modelled `OperationStatus` members an entry of this emulator's log
/// can report: `Started` while the clock still owes the work its terminal
/// status, `Succeeded` once it has settled
/// (`crate::state::OPERATION_STATUS_STARTED` /
/// `OPERATION_STATUS_SUCCEEDED`).
const OPERATION_STATUSES: &[&str] = &["Started", "Succeeded"];

/// What a row builds before it reads the log back.
#[derive(Debug, Clone, Copy)]
enum Setup {
    /// Nothing at all: a name the log holds no entry for.
    Nothing,
    /// Reserve a static IP — one terminal `AllocateStaticIp` entry.
    StaticIp,
    /// Create a block storage disk — one `CreateDisk` entry, which the clock
    /// still owes its terminal status when the call returns.
    Disk,
    /// Create an instance and open `opened_ports` distinct ports on it — one
    /// `CreateInstance` entry and one `OpenInstancePublicPorts` entry per port.
    Instance { opened_ports: usize },
}

/// One case: the resource a row builds and the name it is read back under.
///
/// The rows differ along everything the rule generalises over: the KIND of
/// resource the entries were recorded against (and so the modelled
/// `ResourceType` they report), the NUMBER of entries the log holds for it —
/// none, one, a handful, exactly one page, one past a page and two pages past
/// one — and the form of the name the modelled `ResourceName` pattern
/// `^\w[\w\-]*\w$` admits.
struct Row {
    case: &'static str,
    /// The `resourceName` every entry of this row carries, and the one the read
    /// asks about.
    name: &'static str,
    /// The modelled `ResourceType` every entry of this row must report.
    resource_type: &'static str,
    setup: Setup,
}

/// A name no row ever creates anything under: the log holds nothing for it.
const UNTOUCHED_NAME: &str = "ops-for-resource-never-created";

const ROWS: &[Row] = &[
    Row {
        case: "a name the log holds no entry for",
        name: UNTOUCHED_NAME,
        // No entry is reported, so no resource type is either.
        resource_type: "",
        setup: Setup::Nothing,
    },
    Row {
        case: "a static IP, one terminal entry",
        name: "0ops_static-ip",
        resource_type: "StaticIp",
        setup: Setup::StaticIp,
    },
    Row {
        case: "a block storage disk, one entry the clock still owes",
        name: "ops-disk",
        resource_type: "Disk",
        setup: Setup::Disk,
    },
    Row {
        case: "an instance touched once",
        name: "ops_instance_created_only",
        resource_type: "Instance",
        setup: Setup::Instance { opened_ports: 0 },
    },
    Row {
        case: "an instance touched a handful of times",
        name: "ops-instance-firewall",
        resource_type: "Instance",
        setup: Setup::Instance { opened_ports: 3 },
    },
    Row {
        case: "an instance with exactly one full page of entries",
        name: "ops-instance-one-full-page",
        resource_type: "Instance",
        setup: Setup::Instance {
            opened_ports: OPERATIONS_FOR_RESOURCE_PAGE_SIZE - 1,
        },
    },
    Row {
        case: "an instance with one entry past a page",
        name: "ops-instance-one-past-a-page",
        resource_type: "Instance",
        setup: Setup::Instance {
            opened_ports: OPERATIONS_FOR_RESOURCE_PAGE_SIZE,
        },
    },
    Row {
        case: "an instance spanning three pages",
        name: "ops-instance-three-pages",
        resource_type: "Instance",
        setup: Setup::Instance {
            opened_ports: 2 * OPERATIONS_FOR_RESOURCE_PAGE_SIZE,
        },
    },
];

/// Every operation-log entry a mutating response reports, in the order it lists
/// them: a Lightsail call answers either one `operation` or a list of
/// `operations`, depending on how many resources it can act on.
fn answered_entries(case: &str, body: &Value) -> Vec<Value> {
    if let Some(entries) = body["operations"].as_array() {
        return entries.clone();
    }
    assert!(
        body["operation"].is_object(),
        "case {case}: expected an operation or operations member, got {body:?}"
    );
    vec![body["operation"].clone()]
}

/// The `id` of each entry of `entries`, in order.
fn ids(entries: &[Value]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| {
            entry["id"]
                .as_str()
                .unwrap_or_else(|| panic!("every operation-log entry carries an id, got {entry:?}"))
                .to_string()
        })
        .collect()
}

/// The `operationType` of each entry of `entries`, in order.
fn operation_types(entries: &[Value]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| {
            entry["operationType"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("every operation-log entry carries an operationType, got {entry:?}")
                })
                .to_string()
        })
        .collect()
}

/// Build `row`'s resource, answering the log entries the calls that built it
/// reported, in the order they were made.
async fn setup(emulator: &Emulator, row: &Row) -> Vec<Value> {
    let case = row.case;
    match row.setup {
        Setup::Nothing => Vec::new(),
        Setup::StaticIp => {
            let answered = emulator
                .ok("AllocateStaticIp", json!({"staticIpName": row.name}))
                .await;
            answered_entries(case, &answered)
        }
        Setup::Disk => {
            let answered = emulator
                .ok(
                    "CreateDisk",
                    json!({
                        "diskName": row.name,
                        "availabilityZone": ZONE,
                        "sizeInGb": DISK_SIZE_IN_GB,
                    }),
                )
                .await;
            answered_entries(case, &answered)
        }
        Setup::Instance { opened_ports } => {
            let answered = emulator
                .ok(
                    "CreateInstances",
                    json!({
                        "instanceNames": [row.name],
                        "availabilityZone": ZONE,
                        "blueprintId": BLUEPRINT,
                        "bundleId": BUNDLE,
                    }),
                )
                .await;
            let mut entries = answered_entries(case, &answered);
            for index in 0..opened_ports {
                let port = FIRST_OPENED_PORT + index as i64;
                let answered = emulator
                    .ok(
                        "OpenInstancePublicPorts",
                        json!({
                            "instanceName": row.name,
                            "portInfo": {"fromPort": port, "toPort": port, "protocol": "tcp"},
                        }),
                    )
                    .await;
                entries.extend(answered_entries(case, &answered));
            }
            entries
        }
    }
}

/// The `operationType` sequence `row`'s calls must have left in the log, oldest
/// first.
fn expected_operation_types(row: &Row) -> Vec<String> {
    let types: Vec<&str> = match row.setup {
        Setup::Nothing => Vec::new(),
        // `AllocateStaticIp` and `CreateDisk` are the modelled `OperationType`
        // members those two calls record
        // (`crate::state::OPERATION_TYPE_ALLOCATE_STATIC_IP` /
        // `OPERATION_TYPE_CREATE_DISK`).
        Setup::StaticIp => vec!["AllocateStaticIp"],
        Setup::Disk => vec!["CreateDisk"],
        Setup::Instance { opened_ports } => {
            // `CreateInstances` records one entry PER instance it creates, and
            // the modelled member for it is the singular `CreateInstance`
            // (`crate::state::OPERATION_TYPE_CREATE_INSTANCE`).
            let mut types = vec!["CreateInstance"];
            types.extend(vec!["OpenInstancePublicPorts"; opened_ports]);
            types
        }
    };
    types.into_iter().map(str::to_string).collect()
}

/// The page sizes a walk over `count` entries must report: full pages while
/// entries remain, and one last page with the rest — a single empty page when
/// the log holds nothing for the resource.
fn expected_page_sizes(count: usize) -> Vec<usize> {
    if count == 0 {
        return vec![0];
    }
    let mut sizes = Vec::new();
    let mut remaining = count;
    while remaining > 0 {
        let page = remaining.min(OPERATIONS_FOR_RESOURCE_PAGE_SIZE);
        sizes.push(page);
        remaining -= page;
    }
    sizes
}

/// Every entry `GetOperationsForResource` reports for `name`, walking every
/// page: the entries in page order, together with the size of each page.
///
/// Checks the paging invariants every page must satisfy on the way: no page is
/// larger than one page, `nextPageCount` is never reported, and a
/// `nextPageToken` names exactly the first entry of the page after it.
async fn walk(emulator: &Emulator, case: &str, name: &str) -> (Vec<Value>, Vec<usize>) {
    let mut entries: Vec<Value> = Vec::new();
    let mut page_sizes = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let body = match &token {
            Some(token) => {
                emulator
                    .ok(OPERATION, json!({"resourceName": name, "pageToken": token}))
                    .await
            }
            None => emulator.ok(OPERATION, json!({"resourceName": name})).await,
        };
        let page = body["operations"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case}: expected an operations array for {name}, got {body:?}")
            })
            .clone();
        assert!(
            page.len() <= OPERATIONS_FOR_RESOURCE_PAGE_SIZE,
            "case {case}: expected at most {OPERATIONS_FOR_RESOURCE_PAGE_SIZE} entries in a page, got {}",
            page.len()
        );
        assert!(
            body["nextPageCount"].is_null(),
            "case {case}: nextPageCount is discontinued and must not be reported, got {:?}",
            body["nextPageCount"]
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
            "case {case}: the walk over {name} did not terminate after {} pages",
            page_sizes.len()
        );
    }
    (entries, page_sizes)
}

/// Assert `entry` is a well-formed log entry of `name`, of `resource_type`, as
/// `GetOperation` also reports it when asked for its id.
async fn assert_entry(
    emulator: &Emulator,
    case: &str,
    entry: &Value,
    name: &str,
    resource_type: &str,
) {
    let id = entry["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected an id on {entry:?}"));
    assert!(
        is_uuid_shaped(id),
        "case {case}: expected a UUID-shaped operation id, got {id:?}"
    );
    assert_eq!(
        entry["resourceName"].as_str(),
        Some(name),
        "case {case}: expected the entry to name {name}, got {entry:?}"
    );
    assert_eq!(
        entry["resourceType"].as_str(),
        Some(resource_type),
        "case {case}: expected the {resource_type} resource type, got {entry:?}"
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
    for member in ["id", "resourceName", "resourceType", "operationType", "createdAt"] {
        assert_eq!(
            by_id[member], entry[member],
            "case {case}: GetOperation and {OPERATION} must agree on {member}, got {by_id:?} and {entry:?}"
        );
    }
}

/// Poll `GetOperationsForResource` for `name` until every entry it reports is
/// terminal, answering them.
///
/// The transitions are owed by the clock, not by the poll: they arrive without
/// anyone asking more than once.
async fn wait_until_settled(emulator: &Emulator, case: &str, name: &str) -> Vec<Value> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let (entries, _) = walk(emulator, case, name).await;
        if entries
            .iter()
            .all(|entry| entry["isTerminal"].as_bool() == Some(true))
        {
            return entries;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: expected every entry of {name} to settle within 5s, got {entries:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// RULE: the read reports exactly the log entries recorded against the name it
/// is asked about — every one of them, nobody else's, oldest first, paged.
#[tokio::test]
async fn each_resource_reports_exactly_the_entries_recorded_against_it() {
    let emulator = Emulator::new();

    // Build EVERY row first, so each read happens with every other row's
    // entries already in the same log: a page may only carry the entries of the
    // resource it was asked about.
    let mut answered_at_setup: Vec<Vec<Value>> = Vec::new();
    for row in ROWS {
        answered_at_setup.push(setup(&emulator, row).await);
    }

    let mut every_reported_id: BTreeSet<String> = BTreeSet::new();
    for (row, answered) in ROWS.iter().zip(&answered_at_setup) {
        let case = row.case;
        let (reported, page_sizes) = walk(&emulator, case, row.name).await;

        assert_eq!(
            ids(&reported),
            ids(answered),
            "case {case}: expected exactly the entries the mutating calls answered, in the order \
             they were made, got {reported:?}"
        );
        assert_eq!(
            operation_types(&reported),
            expected_operation_types(row),
            "case {case}: expected the log to hold one entry per call, oldest first, got \
             {reported:?}"
        );
        assert_eq!(
            page_sizes,
            expected_page_sizes(answered.len()),
            "case {case}: expected {} entries to page as {:?}, got {page_sizes:?}",
            answered.len(),
            expected_page_sizes(answered.len())
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
        for entry in &reported {
            assert_entry(&emulator, case, entry, row.name, row.resource_type).await;
            every_reported_id.insert(
                entry["id"]
                    .as_str()
                    .expect("every reported entry carries an id")
                    .to_string(),
            );
        }
    }

    // No entry was reported under two different resources: the ids reported
    // across every row are exactly the ids the setup minted, each once.
    let minted: BTreeSet<String> = answered_at_setup
        .iter()
        .flat_map(|entries| ids(entries))
        .collect();
    assert_eq!(
        every_reported_id, minted,
        "case every row: expected each minted entry reported under exactly its own resource"
    );
    assert_eq!(
        minted.len(),
        answered_at_setup
            .iter()
            .map(|entries| entries.len())
            .sum::<usize>(),
        "case every row: expected every minted operation id to be distinct, got {minted:?}"
    );
}

/// RULE: the log is append-only and keyed by resource NAME, so an entry is
/// still reported after the resource it names is gone — which is what lets a
/// caller read back the operation that deleted it.
#[tokio::test]
async fn a_deleted_resource_still_reports_the_entries_that_name_it() {
    let emulator = Emulator::new();
    let case = "an instance created, changed and then deleted";
    let name = "ops-deleted-instance";

    let created = emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [name],
                "availabilityZone": ZONE,
                "blueprintId": BLUEPRINT,
                "bundleId": BUNDLE,
            }),
        )
        .await;
    let opened = emulator
        .ok(
            "OpenInstancePublicPorts",
            json!({
                "instanceName": name,
                "portInfo": {"fromPort": FIRST_OPENED_PORT, "toPort": FIRST_OPENED_PORT, "protocol": "tcp"},
            }),
        )
        .await;
    let deleted = emulator
        .ok("DeleteInstance", json!({"instanceName": name}))
        .await;

    let mut expected = answered_entries(case, &created);
    expected.extend(answered_entries(case, &opened));
    expected.extend(answered_entries(case, &deleted));

    // The instance itself is gone.
    let (status, body) = emulator
        .call("GetInstance", json!({"instanceName": name}))
        .await;
    assert_eq!(
        status, 400,
        "case {case}: expected the deleted instance to be refused, got {status} with {body:?}"
    );

    // Its log entries are not.
    let (reported, page_sizes) = walk(&emulator, case, name).await;
    assert_eq!(
        ids(&reported),
        ids(&expected),
        "case {case}: expected every entry recorded against the deleted name, got {reported:?}"
    );
    assert_eq!(
        operation_types(&reported),
        vec![
            "CreateInstance".to_string(),
            "OpenInstancePublicPorts".to_string(),
            "DeleteInstance".to_string(),
        ],
        "case {case}: expected the create, the firewall change and the delete, oldest first, got \
         {reported:?}"
    );
    assert_eq!(
        page_sizes,
        expected_page_sizes(reported.len()),
        "case {case}: expected one page, got {page_sizes:?}"
    );
    for entry in &reported {
        assert_entry(&emulator, case, entry, name, "Instance").await;
    }
}

/// RULE: a name the log holds no entry for reports an EMPTY list — the caller
/// asked which operations touched it, and the answer is "none" — rather than a
/// refusal, and that stays true for a name that could never exist as well as
/// for one of a resource the account simply has not touched.
#[tokio::test]
async fn a_name_with_no_entries_reports_an_empty_list() {
    let emulator = Emulator::new();

    // One resource exists and has entries, so an empty answer is the log being
    // filtered rather than the log being empty.
    emulator
        .ok("AllocateStaticIp", json!({"staticIpName": "ops-empty-peer"}))
        .await;

    // Rows differ along the forms the modelled `ResourceName` pattern
    // `^\w[\w\-]*\w$` admits that nothing was created under.
    for name in [
        UNTOUCHED_NAME,
        "ops-empty-peer-2",
        "0z",
        "Ops_Empty_Mixed-9",
    ] {
        let body = emulator.ok(OPERATION, json!({"resourceName": name})).await;
        assert_eq!(
            body["operations"].as_array().map(Vec::len),
            Some(0),
            "case {name}: expected an empty operations list, got {body:?}"
        );
        assert!(
            body["nextPageToken"].is_null(),
            "case {name}: expected no nextPageToken with nothing to report, got {body:?}"
        );
        assert!(
            body["nextPageCount"].is_null(),
            "case {name}: nextPageCount is discontinued and must not be reported, got {body:?}"
        );
    }
}

/// RULE: reading the log is reporting only — it changes nothing a later caller
/// sees, however many times and however it is paged.
#[tokio::test]
async fn reading_the_log_does_not_change_what_a_later_caller_sees() {
    let emulator = Emulator::new();
    let case = "an instance past one page of entries";
    let name = "ops-reporting-only";
    let row = Row {
        case,
        name,
        resource_type: "Instance",
        setup: Setup::Instance {
            opened_ports: OPERATIONS_FOR_RESOURCE_PAGE_SIZE,
        },
    };
    let answered = setup(&emulator, &row).await;

    // Once every entry has settled, nothing the clock owes can differ between
    // two reads, so the whole answer must repeat verbatim.
    let settled = wait_until_settled(&emulator, case, name).await;
    assert_eq!(
        ids(&settled),
        ids(&answered),
        "case {case}: expected the settled log to hold exactly the entries the calls answered"
    );

    let first = emulator.ok(OPERATION, json!({"resourceName": name})).await;
    let again = emulator.ok(OPERATION, json!({"resourceName": name})).await;
    assert_eq!(
        first, again,
        "case {case}: expected a repeated read to answer identically, got {first:?} then {again:?}"
    );

    let (walked, page_sizes) = walk(&emulator, case, name).await;
    let after_the_walk = emulator.ok(OPERATION, json!({"resourceName": name})).await;
    assert_eq!(
        first, after_the_walk,
        "case {case}: expected a read after a paged walk to answer identically, got {first:?} then \
         {after_the_walk:?}"
    );
    assert_eq!(
        ids(&walked),
        ids(&settled),
        "case {case}: expected the paged walk to report exactly the same entries, got {walked:?}"
    );
    assert_eq!(
        page_sizes,
        expected_page_sizes(answered.len()),
        "case {case}: expected {} entries to page as {:?}, got {page_sizes:?}",
        answered.len(),
        expected_page_sizes(answered.len())
    );
}

/// RULE: a `pageToken` that names no entry of the resource asked about names no
/// page, so it is refused with the modelled `InvalidInputException` (HTTP 400) —
/// including a token that names a real entry of a DIFFERENT resource — and the
/// refusal changes nothing.
#[tokio::test]
async fn a_page_token_naming_no_entry_of_the_resource_is_refused() {
    let emulator = Emulator::new();
    let name = "ops-token-subject";
    let other = "ops-token-stranger";
    let subject = Row {
        case: "the resource under test",
        name,
        resource_type: "Instance",
        setup: Setup::Instance { opened_ports: 2 },
    };
    let answered = setup(&emulator, &subject).await;
    let strangers = setup(
        &emulator,
        &Row {
            case: "another resource of the same account",
            name: other,
            resource_type: "StaticIp",
            setup: Setup::StaticIp,
        },
    )
    .await;
    let stranger_id = ids(&strangers)
        .first()
        .expect("the other resource has one entry")
        .clone();

    // Rows differ along the ways a token can fail to name an entry of this
    // resource: an id of ANOTHER resource's entry, an opaque cursor from
    // elsewhere, a UUID-shaped id nothing minted, an index where an id belongs,
    // and blank-but-not-empty.
    for token in [
        stranger_id.as_str(),
        "not-a-page-token",
        "00000000-0000-0000-0000-000000000000",
        "1",
        " ",
    ] {
        let (status, body) = emulator
            .call(OPERATION, json!({"resourceName": name, "pageToken": token}))
            .await;
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
    let (reported, _) = walk(&emulator, "after the refusals", name).await;
    assert_eq!(
        ids(&reported),
        ids(&answered),
        "case after the refusals: expected the log untouched, got {reported:?}"
    );
    let (stranger_entries, _) = walk(&emulator, "after the refusals", other).await;
    assert_eq!(
        ids(&stranger_entries),
        ids(&strangers),
        "case after the refusals: expected the other resource's log untouched, got \
         {stranger_entries:?}"
    );

    // An EMPTY token is not a token that names nothing: it asks for the first
    // page, exactly as an absent one does. The two reads are compared on the
    // members a clock-owed settle cannot change between them — a settle landing
    // in between moves `status`, `statusChangedAt` and `isTerminal`.
    let empty = emulator
        .ok(OPERATION, json!({"resourceName": name, "pageToken": ""}))
        .await;
    let absent = emulator.ok(OPERATION, json!({"resourceName": name})).await;
    let empty_entries = answered_entries("an empty pageToken", &empty);
    let absent_entries = answered_entries("an absent pageToken", &absent);
    assert_eq!(
        ids(&empty_entries),
        ids(&absent_entries),
        "case an empty pageToken: expected the first page, got {empty:?} and {absent:?}"
    );
    assert_eq!(
        empty["nextPageToken"], absent["nextPageToken"],
        "case an empty pageToken: expected the same nextPageToken as an absent one, got {empty:?} \
         and {absent:?}"
    );
}
