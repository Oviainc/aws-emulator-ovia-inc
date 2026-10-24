// SPDX-License-Identifier: CC0-1.0

//! The Lightsail VPC peering — peer, read, unpeer — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. None of the three operations declares a single input member, so the
//! `Authorization` header's credential SCOPE — which names the Region a request
//! is made in — is the only thing that varies between the rows below.
//!
//! The rule under test: a Lightsail VPC belongs to ONE Region, and these three
//! operations are the one peering record of that Region's VPC, walked by the
//! clock. `IsVpcPeered` answers whether a peering connection EXISTS in the
//! Region the request is made in — `false` for a Region nothing peered, `false`
//! while `PeerVpc`'s connection is still being established, `true` from the
//! instant it is established until the instant `UnpeerVpc`'s teardown removes it
//! — and reports it without changing it; `PeerVpc` and `UnpeerVpc` each answer
//! ONE non-terminal `PeeredVpc` operation-log entry, located in and naming that
//! Region, which settles into `Succeeded` as the work the clock owed it
//! completes; a peering request the service has no work for (peering a peered
//! VPC, unpeering an unpeered one, either while the other walk is still running)
//! is refused with the modelled `OperationFailureException` and changes nothing;
//! and peering one Region leaves every other Region's Lightsail VPC alone.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The region a request that carries no SigV4 credential scope is served in
/// (`crate::state::DEFAULT_REGION`).
const DEFAULT_REGION: &str = "us-east-1";

/// One peering case: the Region the request is signed for, and the Region it is
/// therefore served in.
///
/// The rows differ along everything the rule generalises over — the Region the
/// call is made in, including a request that carries no credential scope at all
/// — and no two rows are served in the same Region, so each row has its own
/// Lightsail VPC. Every named Region is one `GetRegions` lists as served
/// (`crate::state::REGION_SPECS`, reported by the `region_catalog` suite).
struct Row {
    case: &'static str,
    /// The region the request is made in, or `None` for a request that carries
    /// no credential scope at all.
    region: Option<&'static str>,
    /// The Region the call is served in, and so the one its Lightsail VPC, its
    /// operation-log entry and its refusal message name.
    served_region: &'static str,
}

/// The table. The Region the modelled `Instance$arn` example is written in
/// (`us-east-2`) is the first named row.
const ROWS: &[Row] = &[
    Row {
        case: "a request carrying no credential scope at all",
        region: None,
        served_region: DEFAULT_REGION,
    },
    Row {
        case: "the Region the modelled ARN example names",
        region: Some("us-east-2"),
        served_region: "us-east-2",
    },
    Row {
        case: "another Region of the same geography",
        region: Some("us-west-2"),
        served_region: "us-west-2",
    },
    Row {
        case: "a Region in Europe",
        region: Some("eu-central-1"),
        served_region: "eu-central-1",
    },
    Row {
        case: "a Region in Asia Pacific",
        region: Some("ap-northeast-1"),
        served_region: "ap-northeast-1",
    },
    Row {
        case: "a Region in South America",
        region: Some("sa-east-1"),
        served_region: "sa-east-1",
    },
];

/// Whether `IsVpcPeered` reports the Lightsail VPC of `region` as peered.
async fn is_peered(emulator: &Emulator, region: Option<&str>) -> bool {
    let body = emulator
        .ok_at(None, region, "IsVpcPeered", json!({}))
        .await;
    body["isPeered"].as_bool().unwrap_or_else(|| {
        panic!("case reading the peering in {region:?}: expected an isPeered boolean, got {body:?}")
    })
}

/// Call `operation` (`PeerVpc` or `UnpeerVpc`) in `region`, answering the single
/// operation-log entry it reported.
async fn walk(emulator: &Emulator, region: Option<&str>, operation: &str) -> Value {
    let body = emulator.ok_at(None, region, operation, json!({})).await;
    let entry = body["operation"].clone();
    assert!(
        entry.is_object(),
        "case {operation} in {region:?}: expected one operation entry, got {body:?}"
    );
    entry
}

/// The log entry `id`, as `GetOperation` reports it.
async fn log_entry(emulator: &Emulator, region: Option<&str>, id: &str) -> Value {
    emulator
        .ok_at(None, region, "GetOperation", json!({"operationId": id}))
        .await["operation"]
        .clone()
}

/// Assert every member the rule fixes on the entry `PeerVpc` or `UnpeerVpc`
/// answered: it is the one non-terminal `PeeredVpc` entry of `operation_type`,
/// located in `served_region` and naming it.
fn assert_started_entry(case: &str, entry: &Value, operation_type: &str, served_region: &str) {
    assert_eq!(
        entry["operationType"].as_str(),
        Some(operation_type),
        "case {case}: expected the {operation_type} operation type, got {entry:?}"
    );
    assert_eq!(
        entry["resourceType"].as_str(),
        Some("PeeredVpc"),
        "case {case}: expected the PeeredVpc resource type, got {entry:?}"
    );
    assert_eq!(
        entry["resourceName"].as_str(),
        Some(served_region),
        "case {case}: a Lightsail VPC is named by the Region it lives in, got {entry:?}"
    );
    assert_eq!(
        entry["location"]["regionName"].as_str(),
        Some(served_region),
        "case {case}: expected the entry in the Region of the request, got {entry:?}"
    );
    assert!(
        entry["location"]["availabilityZone"].is_null(),
        "case {case}: a Lightsail VPC spans the Region, so it names no availability \
         zone, got {:?}",
        entry["location"]
    );
    assert_eq!(
        entry["status"].as_str(),
        Some("Started"),
        "case {case}: the connection is not up yet, so the entry is Started, got {entry:?}"
    );
    assert_eq!(
        entry["isTerminal"].as_bool(),
        Some(false),
        "case {case}: the clock still owes this entry its terminal status, got {entry:?}"
    );
    assert!(
        entry["createdAt"].as_f64().unwrap_or_default() > 0.0,
        "case {case}: expected a creation timestamp, got {:?}",
        entry["createdAt"]
    );
}

/// The id of a log entry, asserted to have the shape Lightsail's ids take.
fn entry_id(case: &str, entry: &Value) -> String {
    let id = entry["id"].as_str().unwrap_or_default().to_string();
    assert!(
        is_uuid_shaped(&id),
        "case {case}: expected a uuid-shaped operation id, got {id:?}"
    );
    id
}

/// Poll `IsVpcPeered` in `region` until it reports `want`.
///
/// The walk is owed by the clock, not by the poll: it arrives without anyone
/// asking more than once.
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

/// Assert the transient the rule fixes for a walk the entry `id` describes:
/// while that walk is still running, `IsVpcPeered` answers `transient`.
///
/// Written so it cannot race the clock the walk runs on: `IsVpcPeered` is read
/// FIRST and the entry SECOND, so any other answer means the walk had already
/// come due at the earlier read — in which case the entry read after it must
/// have settled too, which is what is asserted instead. Only the clock may move
/// either one.
async fn assert_transient(
    emulator: &Emulator,
    case: &str,
    region: Option<&str>,
    id: &str,
    transient: bool,
) {
    let peered = is_peered(emulator, region).await;
    if peered == transient {
        return;
    }
    let entry = log_entry(emulator, region, id).await;
    assert_eq!(
        entry["isTerminal"].as_bool(),
        Some(true),
        "case {case}: isPeered reported {peered} while the walk it belongs to was still \
         running, which nothing but the clock may do, got {entry:?}"
    );
}

/// Call `operation` in `region` and require Lightsail's
/// `OperationFailureException` refusal, answering the parsed body.
async fn refused(emulator: &Emulator, case: &str, region: Option<&str>, operation: &str) -> Value {
    let (status, body) = emulator.call_in(region, operation, json!({})).await;
    assert_eq!(
        status, 400,
        "case {case}: expected the OperationFailureException status, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "OperationFailureException",
        "case {case}: expected OperationFailureException, got {body:?}"
    );
    body
}

/// Assert the refusal `body` says why, naming the Region and ONE of
/// `fragments`.
///
/// More than one fragment where the clock may legitimately have walked the
/// peering on to its next phase between the call that started the walk and the
/// refused one — both phases refuse, each naming the phase it is in, and nothing
/// but the clock may move between them.
fn assert_refusal_message(case: &str, body: &Value, served_region: &str, fragments: &[&str]) {
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(served_region) && fragments.iter().any(|f| message.contains(f)),
        "case {case}: expected a message naming {served_region} and one of {fragments:?}, \
         got {body:?}"
    );
}

/// RULE: the Lightsail VPC of a Region nothing has peered is not peered, a
/// `PeerVpc` there answers ONE non-terminal `PeeredVpc` entry naming that Region,
/// and the VPC reports itself peered only once the clock has established the
/// connection — at which instant the entry the call answered has settled into
/// `Succeeded`.
#[tokio::test]
async fn a_vpc_is_peered_once_the_clock_establishes_the_connection() {
    let emulator = Emulator::new();
    let mut ids: BTreeSet<String> = BTreeSet::new();

    for row in ROWS {
        let case = row.case;
        assert!(
            !is_peered(&emulator, row.region).await,
            "case {case}: a Lightsail VPC nothing peered is not peered"
        );

        let entry = walk(&emulator, row.region, "PeerVpc").await;
        assert_started_entry(case, &entry, "PeerVpc", row.served_region);
        let id = entry_id(case, &entry);
        assert!(
            ids.insert(id.clone()),
            "case {case}: expected a fresh operation id, got the already-used {id:?}"
        );

        // The connection is not up when the call returns, exactly as against
        // AWS: the call answered an entry the clock still owes its terminal
        // status, so this read reports the transient "not peered yet".
        assert_transient(&emulator, case, row.region, &id, false).await;

        wait_until(&emulator, case, row.region, true).await;

        // The entry the call answered settled with the connection it describes.
        let settled = log_entry(&emulator, row.region, &id).await;
        assert_eq!(
            settled["status"].as_str(),
            Some("Succeeded"),
            "case {case}: expected the entry to settle into Succeeded, got {settled:?}"
        );
        assert_eq!(
            settled["isTerminal"].as_bool(),
            Some(true),
            "case {case}: expected the settled entry to be terminal, got {settled:?}"
        );
        assert_eq!(
            settled["id"].as_str(),
            Some(id.as_str()),
            "case {case}: expected the same entry back, got {settled:?}"
        );
    }
}

/// RULE: a Lightsail VPC belongs to ONE Region, so peering the VPC of one Region
/// leaves every other Region's VPC unpeered — however many Regions are peered
/// after it.
#[tokio::test]
async fn peering_one_region_leaves_every_other_region_unpeered() {
    let emulator = Emulator::new();
    let mut peered: Vec<&Row> = Vec::new();

    for row in ROWS {
        let case = row.case;
        walk(&emulator, row.region, "PeerVpc").await;
        wait_until(&emulator, case, row.region, true).await;
        peered.push(row);

        // Every Region peered so far is peered; no other Region is.
        for other in ROWS {
            let want = peered.iter().any(|done| done.served_region == other.served_region);
            let actual = is_peered(&emulator, other.region).await;
            assert_eq!(
                actual, want,
                "case {case}: after peering {}, expected isPeered {want} in {}, got \
                 {actual}",
                row.served_region, other.served_region
            );
        }
    }
}

/// RULE: `UnpeerVpc` answers ONE non-terminal `PeeredVpc` entry and the
/// connection stays up until the clock removes it, after which the Region's
/// Lightsail VPC is not peered and may be peered again — a fresh walk with a
/// fresh entry.
#[tokio::test]
async fn an_unpeered_vpc_can_be_peered_again() {
    let emulator = Emulator::new();
    let mut ids: BTreeSet<String> = BTreeSet::new();

    for row in ROWS {
        let case = row.case;
        let first = entry_id(case, &walk(&emulator, row.region, "PeerVpc").await);
        wait_until(&emulator, case, row.region, true).await;

        let entry = walk(&emulator, row.region, "UnpeerVpc").await;
        assert_started_entry(case, &entry, "UnpeerVpc", row.served_region);
        let teardown = entry_id(case, &entry);
        assert!(
            !ids.contains(&teardown) && teardown != first,
            "case {case}: expected a fresh operation id, got the already-used \
             {teardown:?}"
        );

        // The connection is still up until the clock tears it down.
        assert_transient(&emulator, case, row.region, &teardown, true).await;
        wait_until(&emulator, case, row.region, false).await;
        let settled = log_entry(&emulator, row.region, &teardown).await;
        assert_eq!(
            settled["status"].as_str(),
            Some("Succeeded"),
            "case {case}: expected the teardown entry to settle, got {settled:?}"
        );

        // Peerable again, and a new walk of its own.
        let again = entry_id(case, &walk(&emulator, row.region, "PeerVpc").await);
        assert!(
            again != first && again != teardown,
            "case {case}: expected a fresh operation id for the second peering, got \
             {again:?}"
        );
        wait_until(&emulator, case, row.region, true).await;

        ids.insert(first);
        ids.insert(teardown);
        ids.insert(again);
    }
}

/// RULE: a peering call the service has no work for is refused with the modelled
/// `OperationFailureException` (HTTP 400) naming the Region, and changes nothing:
/// peering a VPC that is already peered, or whose peering is still being
/// established; unpeering one that is not peered, whose peering is still being
/// established, or whose teardown is already running.
///
/// Each row uses a Region of its own, so the phase one case set up is not the
/// phase the next one finds.
#[tokio::test]
async fn a_call_with_no_peering_work_to_do_is_refused() {
    let emulator = Emulator::new();

    // Nothing peered: there is no connection to tear down.
    let row = &ROWS[0];
    let case = "unpeering a VPC nothing peered";
    let body = refused(&emulator, case, row.region, "UnpeerVpc").await;
    assert_refusal_message(case, &body, row.served_region, &["is not peered"]);
    assert!(
        !is_peered(&emulator, row.region).await,
        "case {case}: the refusal must change nothing"
    );

    // The peering is still being established: neither call has work yet.
    let row = &ROWS[1];
    walk(&emulator, row.region, "PeerVpc").await;
    // Peering again is refused whichever of the two phases the clock has reached
    // by now — being established, or established.
    let case = "peering a VPC whose peering is still running";
    let body = refused(&emulator, case, row.region, "PeerVpc").await;
    assert_refusal_message(
        case,
        &body,
        row.served_region,
        &["is already being peered", "is already peered"],
    );
    // Unpeering has no work while the connection is still being established. The
    // instant it IS established, unpeering it is legitimate work — and that is
    // the only other phase the clock may have reached — so both answers are
    // asserted against the phase they belong to rather than against a clock this
    // test does not control.
    let case = "unpeering a VPC whose peering is still running";
    let (status, body) = emulator.call_in(row.region, "UnpeerVpc", json!({})).await;
    if status == 200 {
        assert_started_entry(case, &body["operation"], "UnpeerVpc", row.served_region);
        wait_until(&emulator, case, row.region, false).await;
    } else {
        assert_eq!(
            status, 400,
            "case {case}: expected the OperationFailureException status, got {status} \
             {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "OperationFailureException",
            "case {case}: expected OperationFailureException, got {body:?}"
        );
        assert_refusal_message(case, &body, row.served_region, &["is still being peered"]);
        // The refusals changed nothing: the peering the first call started still
        // settles.
        wait_until(&emulator, case, row.region, true).await;
    }

    // Already peered: there is nothing to peer.
    let row = &ROWS[2];
    let case = "peering an already peered VPC";
    walk(&emulator, row.region, "PeerVpc").await;
    wait_until(&emulator, case, row.region, true).await;
    let body = refused(&emulator, case, row.region, "PeerVpc").await;
    assert_refusal_message(case, &body, row.served_region, &["is already peered"]);
    assert!(
        is_peered(&emulator, row.region).await,
        "case {case}: the refusal must leave the peering up"
    );

    // The teardown is already running: there is nothing left to tear down.
    let row = &ROWS[3];
    let case = "unpeering twice";
    walk(&emulator, row.region, "PeerVpc").await;
    wait_until(&emulator, case, row.region, true).await;
    walk(&emulator, row.region, "UnpeerVpc").await;
    let body = refused(&emulator, case, row.region, "UnpeerVpc").await;
    // Refused whichever of the two phases the clock has reached by now — the
    // teardown still running, or the connection already gone.
    assert_refusal_message(
        case,
        &body,
        row.served_region,
        &["is already being unpeered", "is not peered"],
    );
    // The refusal changed nothing: the teardown the first call started still
    // completes.
    wait_until(&emulator, case, row.region, false).await;
}

/// RULE: `IsVpcPeered` REPORTS the peering and never changes it — reading an
/// unpeered Region does not create a peering record (a later `PeerVpc` there
/// still has work to do), and reading a peered one neither tears it down nor
/// re-establishes it, however many times it is read.
#[tokio::test]
async fn reading_the_peering_changes_nothing() {
    let emulator = Emulator::new();

    for row in ROWS {
        let case = row.case;
        for read in 0..3 {
            assert!(
                !is_peered(&emulator, row.region).await,
                "case {case}: read {read} of an unpeered Region must report false"
            );
        }
        // Had a read created a record, this call would be refused.
        walk(&emulator, row.region, "PeerVpc").await;
        wait_until(&emulator, case, row.region, true).await;
        for read in 0..3 {
            assert!(
                is_peered(&emulator, row.region).await,
                "case {case}: read {read} of a peered Region must report true"
            );
        }
    }
}

/// RULE: the Region a call is served in is the Region its credential scope names
/// — and a scope naming no Region the modelled `RegionName` enum declares is
/// served in the default Region, so it sees that Region's Lightsail VPC rather
/// than one of its own.
#[tokio::test]
async fn a_scope_outside_the_modelled_regions_is_served_in_the_default_region() {
    let emulator = Emulator::new();
    // `xx-nowhere-9` is no member of the modelled `RegionName` enum
    // (`smithy_query(query="shape",shape="RegionName")`).
    let outside = Some("xx-nowhere-9");
    let case = "a scope naming no modelled Region";

    let entry = walk(&emulator, outside, "PeerVpc").await;
    assert_started_entry(case, &entry, "PeerVpc", DEFAULT_REGION);
    wait_until(&emulator, case, outside, true).await;

    // The same VPC, read by a request that carries no scope at all.
    assert!(
        is_peered(&emulator, None).await,
        "case {case}: expected the default Region's VPC to be the one peered"
    );
    // And so the call that peers it again has no work to do.
    let body = refused(&emulator, case, None, "PeerVpc").await;
    assert_refusal_message(case, &body, DEFAULT_REGION, &["is already peered"]);
}
