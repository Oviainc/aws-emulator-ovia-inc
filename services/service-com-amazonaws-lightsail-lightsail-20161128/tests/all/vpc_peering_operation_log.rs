// SPDX-License-Identifier: CC0-1.0

//! What `UnpeerVpc` leaves behind in the append-only operation log, read back by
//! the resource each entry was recorded against, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. `PeerVpc`, `UnpeerVpc` and `IsVpcPeered` declare no input member at
//! all, so the `Authorization` header's credential SCOPE — which names the
//! Region a request is made in — is the only thing that varies between the rows
//! below.
//!
//! The rule under test: a Lightsail VPC teardown PERSISTS the operation-log
//! entry it answered against the VPC it tore down, so the same entry is reported
//! again — by id and by resource — after the call that minted it returned; the
//! log is append-only, so a teardown entry is appended AFTER the entry of the
//! peering it removes and neither replaces it nor reorders the log; and a
//! teardown the service has no work for persists NOTHING, leaving the log it
//! would have been appended to exactly as it found it. A Lightsail VPC belongs
//! to one Region, so each Region's log holds only its own entries.
//!
//! The sibling `vpc_peering` suite pins the three operations' own answers and the
//! clock-owed walk between their phases; this one pins what the store keeps.

use serde_json::{json, Value};
use std::time::{Duration, Instant};

use service_com_amazonaws_lightsail_lightsail_20161128::state::OPERATIONS_FOR_RESOURCE_PAGE_SIZE;

// The harness and the wire error code this test target already exposes: a
// sibling suite drives the service through the same front door rather than
// standing up a second one.
use crate::instance_lifecycle::{error_code, Emulator};

/// The Region a request that carries no SigV4 credential scope is served in
/// (`crate::state::DEFAULT_REGION`).
const DEFAULT_REGION: &str = "us-east-1";

/// The modelled `ResourceType` every peering entry reports
/// (`crate::state::RESOURCE_TYPE_PEERED_VPC`).
const PEERED_VPC: &str = "PeeredVpc";

/// One case: the Region the request is signed for, and the Region it is served
/// in — which is also the `resourceName` its Lightsail VPC's log entries carry.
///
/// The rows differ along everything the rule generalises over: the Region the
/// calls are made in, including a request that carries no credential scope at
/// all. No two rows are served in the same Region, so each row has its own
/// Lightsail VPC and its own log.
struct Row {
    case: &'static str,
    /// The Region the request is made in, or `None` for a request that carries
    /// no credential scope at all.
    region: Option<&'static str>,
    /// The Region the call is served in, and so the name its Lightsail VPC's log
    /// entries carry.
    served_region: &'static str,
}

/// The table. Every named served Region is one `GetRegions` lists
/// (`crate::state::REGION_SPECS`, reported by the `region_catalog` suite) and a
/// member of the modelled `RegionName` enum
/// (`smithy_query(query="shape",shape="RegionName")`).
const ROWS: &[Row] = &[
    Row {
        case: "a request carrying no credential scope at all",
        region: None,
        served_region: DEFAULT_REGION,
    },
    Row {
        case: "a Region of the same geography as the default one",
        region: Some("us-west-2"),
        served_region: "us-west-2",
    },
    Row {
        case: "a Region in Europe",
        region: Some("eu-west-1"),
        served_region: "eu-west-1",
    },
    Row {
        case: "a Region in Asia Pacific",
        region: Some("ap-southeast-2"),
        served_region: "ap-southeast-2",
    },
    Row {
        case: "a Region in Canada",
        region: Some("ca-central-1"),
        served_region: "ca-central-1",
    },
];

/// Whether `IsVpcPeered` reports the Lightsail VPC of `region` as peered.
async fn is_peered(emulator: &Emulator, region: Option<&str>) -> bool {
    let body = emulator.ok_at(None, region, "IsVpcPeered", json!({})).await;
    body["isPeered"].as_bool().unwrap_or_else(|| {
        panic!("case reading the peering in {region:?}: expected an isPeered boolean, got {body:?}")
    })
}

/// Poll `IsVpcPeered` in `region` until it reports `want`.
///
/// The transition is owed by the clock, not by the poll: it arrives without
/// anyone asking more than once.
async fn wait_until(emulator: &Emulator, case: &str, region: Option<&str>, want: bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let peered = is_peered(emulator, region).await;
        if peered == want {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: expected isPeered {want} within 5s, got {peered}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// Call `operation` (`PeerVpc` or `UnpeerVpc`) in `region` and answer the id of
/// the single operation-log entry it reported.
async fn started(emulator: &Emulator, case: &str, region: Option<&str>, operation: &str) -> String {
    let body = emulator.ok_at(None, region, operation, json!({})).await;
    body["operation"]["id"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case}: expected {operation} to answer one entry with an id, got {body:?}")
        })
        .to_string()
}

/// Call `operation` in `region` and require Lightsail's modelled
/// `OperationFailureException` refusal (HTTP 400).
async fn refused(emulator: &Emulator, case: &str, region: Option<&str>, operation: &str) {
    let (status, body) = emulator.call_in(region, operation, json!({})).await;
    assert_eq!(
        status, 400,
        "case {case}: expected the OperationFailureException status from {operation}, got \
         {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "OperationFailureException",
        "case {case}: expected OperationFailureException from {operation}, got {body:?}"
    );
}

/// Every entry `GetOperationsForResource` reports for `name`, oldest first,
/// walking every page.
async fn log(emulator: &Emulator, case: &str, name: &str) -> Vec<Value> {
    let mut entries: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let body = match &token {
            Some(token) => {
                emulator
                    .ok(
                        "GetOperationsForResource",
                        json!({"resourceName": name, "pageToken": token}),
                    )
                    .await
            }
            None => {
                emulator
                    .ok("GetOperationsForResource", json!({"resourceName": name}))
                    .await
            }
        };
        let page = body["operations"].as_array().cloned().unwrap_or_else(|| {
            panic!("case {case}: expected an operations array for {name}, got {body:?}")
        });
        assert!(
            page.len() <= OPERATIONS_FOR_RESOURCE_PAGE_SIZE,
            "case {case}: expected at most {OPERATIONS_FOR_RESOURCE_PAGE_SIZE} entries a page \
             for {name}, got {}",
            page.len()
        );
        entries.extend(page);
        match body["nextPageToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
        assert!(
            entries.len() < 1_000,
            "case {case}: the walk over {name}'s log did not terminate after {} entries",
            entries.len()
        );
    }
    entries
}

/// The `id` of every entry in `entries`, in the order the log reports them.
fn ids(case: &str, entries: &[Value]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| {
            entry["id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected an id on {entry:?}"))
                .to_string()
        })
        .collect()
}

/// The `operationType` of every entry in `entries`, in log order.
fn operation_types(case: &str, entries: &[Value]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| {
            entry["operationType"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected an operationType on {entry:?}"))
                .to_string()
        })
        .collect()
}

/// Assert every entry of a Lightsail VPC's log names that VPC: the `PeeredVpc`
/// resource type, the Region it lives in as its name, and that Region as its
/// location — with no availability zone, because a Lightsail VPC spans the
/// Region.
fn assert_peering_entries(case: &str, entries: &[Value], served_region: &str) {
    for entry in entries {
        assert_eq!(
            entry["resourceType"].as_str(),
            Some(PEERED_VPC),
            "case {case}: expected the {PEERED_VPC} resource type, got {entry:?}"
        );
        assert_eq!(
            entry["resourceName"].as_str(),
            Some(served_region),
            "case {case}: a Lightsail VPC is named by the Region it lives in, got {entry:?}"
        );
        assert_eq!(
            entry["location"]["regionName"].as_str(),
            Some(served_region),
            "case {case}: expected the entry located in {served_region}, got {entry:?}"
        );
        assert!(
            entry["location"]["availabilityZone"].is_null(),
            "case {case}: a Lightsail VPC spans the Region, so its entries name no \
             availability zone, got {entry:?}"
        );
    }
    // The log is append-only, so it is reported oldest first.
    let times: Vec<f64> = entries
        .iter()
        .map(|entry| entry["createdAt"].as_f64().unwrap_or_default())
        .collect();
    assert!(
        times.iter().all(|at| *at > 0.0) && times.windows(2).all(|pair| pair[0] <= pair[1]),
        "case {case}: expected the log oldest first with a creation time on every entry, got \
         {times:?}"
    );
}

/// RULE: a Lightsail VPC teardown persists the operation-log entry it answered
/// against the VPC it tore down, APPENDED after the entry of the peering it
/// removes — so the log read back by resource name reports both, in that order,
/// and the teardown entry carries the same id the call answered and settles in
/// the log as the clock completes the teardown.
#[tokio::test]
async fn a_teardown_appends_its_entry_to_the_vpc_s_log() {
    let emulator = Emulator::new();

    for row in ROWS {
        let case = row.case;
        let name = row.served_region;

        assert!(
            log(&emulator, case, name).await.is_empty(),
            "case {case}: a Lightsail VPC nothing has peered has no log entries yet"
        );

        let peering = started(&emulator, case, row.region, "PeerVpc").await;
        wait_until(&emulator, case, row.region, true).await;

        let teardown = started(&emulator, case, row.region, "UnpeerVpc").await;
        assert_ne!(
            teardown, peering,
            "case {case}: expected the teardown its own entry, got the peering's id back"
        );

        let entries = log(&emulator, case, name).await;
        assert_eq!(
            ids(case, &entries),
            vec![peering.clone(), teardown.clone()],
            "case {case}: expected the teardown entry appended after the peering it removes, \
             got {entries:?}"
        );
        assert_eq!(
            operation_types(case, &entries),
            vec!["PeerVpc".to_string(), "UnpeerVpc".to_string()],
            "case {case}: expected the peering then the teardown, got {entries:?}"
        );
        assert_peering_entries(case, &entries, row.served_region);

        // The clock owes the teardown its completion, and the entry the call
        // answered settles with the work it describes — in the persisted log,
        // not only in that one answer.
        wait_until(&emulator, case, row.region, false).await;
        let settled = log(&emulator, case, name).await;
        let last = settled
            .last()
            .unwrap_or_else(|| panic!("case {case}: expected the teardown entry, got {settled:?}"));
        assert_eq!(
            last["id"].as_str(),
            Some(teardown.as_str()),
            "case {case}: expected the teardown still last in the append-only log, got \
             {settled:?}"
        );
        assert_eq!(
            last["status"].as_str(),
            Some("Succeeded"),
            "case {case}: expected the persisted teardown entry to settle, got {last:?}"
        );
        assert_eq!(
            last["isTerminal"].as_bool(),
            Some(true),
            "case {case}: expected the settled teardown entry to be terminal, got {last:?}"
        );

        // The same entry, read by id: one record, two readers.
        let by_id = emulator
            .ok("GetOperation", json!({"operationId": teardown}))
            .await["operation"]
            .clone();
        for member in ["id", "resourceName", "resourceType", "operationType", "createdAt"] {
            assert_eq!(
                by_id[member], last[member],
                "case {case}: GetOperation and GetOperationsForResource must agree on \
                 {member}, got {by_id:?} and {last:?}"
            );
        }
    }
}

/// RULE: a peering call the service has no work for persists NOTHING — it
/// appends no operation-log entry to the Lightsail VPC it names, whether there
/// is no peering to tear down, one the clock has not established yet, or a
/// teardown already running.
#[tokio::test]
async fn a_refused_call_appends_nothing() {
    let emulator = Emulator::new();

    for row in ROWS {
        let case = row.case;
        let name = row.served_region;

        // Nothing peered: there is no connection to tear down, so no entry.
        refused(&emulator, case, row.region, "UnpeerVpc").await;
        assert!(
            log(&emulator, case, name).await.is_empty(),
            "case {case}: a refused teardown of an unpeered VPC must append nothing"
        );

        // The peering is still being established: whichever of the two calls the
        // clock leaves without work is refused, and a refusal appends nothing.
        let peering = started(&emulator, case, row.region, "PeerVpc").await;
        refused(&emulator, case, row.region, "PeerVpc").await;
        assert_eq!(
            ids(case, &log(&emulator, case, name).await),
            vec![peering.clone()],
            "case {case}: a refused peering must append nothing"
        );

        wait_until(&emulator, case, row.region, true).await;
        let teardown = started(&emulator, case, row.region, "UnpeerVpc").await;

        // The teardown is already running, or has already completed: either way
        // there is nothing left to tear down.
        refused(&emulator, case, row.region, "UnpeerVpc").await;
        assert_eq!(
            ids(case, &log(&emulator, case, name).await),
            vec![peering.clone(), teardown.clone()],
            "case {case}: a refused second teardown must append nothing"
        );

        // The refusals changed nothing the clock owed: the teardown the first
        // call started still completes, leaving the same two entries behind.
        wait_until(&emulator, case, row.region, false).await;
        let entries = log(&emulator, case, name).await;
        assert_eq!(
            ids(case, &entries),
            vec![peering, teardown],
            "case {case}: expected the refusals to have left the log alone, got {entries:?}"
        );
        assert_peering_entries(case, &entries, row.served_region);
    }
}

/// RULE: a Lightsail VPC belongs to ONE Region, so a teardown in one Region
/// appends to that Region's VPC log only — every other Region's log is left
/// exactly as it was, however many Regions are torn down after it.
#[tokio::test]
async fn a_teardown_leaves_every_other_region_s_log_alone() {
    let emulator = Emulator::new();
    let mut expected: Vec<(&Row, Vec<String>)> = ROWS.iter().map(|row| (row, Vec::new())).collect();

    for index in 0..ROWS.len() {
        let case = ROWS[index].case;
        let region = ROWS[index].region;

        let peering = started(&emulator, case, region, "PeerVpc").await;
        wait_until(&emulator, case, region, true).await;
        let teardown = started(&emulator, case, region, "UnpeerVpc").await;
        wait_until(&emulator, case, region, false).await;
        expected[index].1.extend([peering, teardown]);

        // Every Region's log holds exactly what its own calls appended.
        for (row, want) in &expected {
            let actual = ids(case, &log(&emulator, case, row.served_region).await);
            assert_eq!(
                &actual, want,
                "case {case}: after tearing down {}, expected {} to hold {want:?}, got \
                 {actual:?}",
                ROWS[index].served_region, row.served_region
            );
        }
    }
}
