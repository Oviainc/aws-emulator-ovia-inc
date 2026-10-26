// SPDX-License-Identifier: CC0-1.0

//! A reset of a Lightsail CDN distribution's cached content runs on a clock of
//! its OWN, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The harness is the one `instance_lifecycle.rs` already has.
//!
//! The rule under test: deleting a distribution's cached content changes none of
//! the configuration deployed to the edge locations, so a reset is served
//! whatever stage the distribution's own deployment is at, leaves every member
//! of the distribution record exactly as it was, neither cancels nor finishes
//! early the deployment the clock already owed that distribution, and settles —
//! both the reset's status and the operation-log entry the call wrote — on a
//! clock of its own.
//!
//! `distribution_cache_reset.rs` pins WHAT a reset records and which read
//! reports it; this suite pins what a reset does NOT touch, and that both
//! records it leaves behind reach their terminal value on the clock rather than
//! because somebody read them: a read taken INSIDE the span the invalidation is
//! owed still reports it being applied, and the terminal value is there once
//! that span has simply been waited out.
//!
//! So the cases are a TABLE: one row per stage of the distribution's own
//! deployment the reset is made at, crossed with the published bundle plan the
//! distribution runs on and the kind of resource it pulls content from.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::state::STATE_TRANSITION_MILLIS;

// The harness, the distribution readers and the cache-reset drivers and
// expectations this test target already exposes: a sibling suite drives the
// service through the same front door, with the same request shapes, the same
// modelled values and the same answer assertions, rather than standing up a
// second copy of them.
use crate::distribution_cache_reset::{
    assert_reset_answer, create_distribution, create_origin, latest_reset, OriginKind, COMPLETED,
    DEPLOYED, IN_PROGRESS, OPERATION_TYPE,
};
use crate::distribution_certificate_attachment::{distribution, wait_for_distribution_status};
use crate::instance_lifecycle::Emulator;

/// The operation under test, by the name its `X-Amz-Target` carries.
const RESET: &str = "ResetDistributionCache";

/// The `status` a distribution reports while the configuration a call stored is
/// still being deployed to the edge locations
/// (`crate::state::DISTRIBUTION_STATUS_IN_PROGRESS`) — the distribution's own
/// status, which is not the status of a reset of its cached content.
const DEPLOYING: &str = "InProgress";

/// The `status` an operation-log entry carries once the work it records has
/// finished (`crate::state::OPERATION_STATUS_SUCCEEDED`).
const SUCCEEDED: &str = "Succeeded";

/// The span a transition this service's clock owes takes, taken from the store's
/// own [`STATE_TRANSITION_MILLIS`] so no case here can drift from it.
const OWED_SPAN: Duration = Duration::from_millis(STATE_TRANSITION_MILLIS as u64);

/// The slack a wait for something owed at [`OWED_SPAN`] adds, so a case is not
/// racing the clock it is waiting for.
const SETTLE_MARGIN: Duration = Duration::from_millis(250);

// ---------------------------------------------------------------------------
// Driving the service
// ---------------------------------------------------------------------------

/// The stage of the distribution's OWN deployment a reset is made at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResetWhen {
    /// Straight after the create, while the configuration is still being
    /// deployed to the edge locations.
    WhileDeploying,
    /// Once the configuration is fully deployed.
    OnceDeployed,
}

/// The distribution record without the one member the clock moves, so two reads
/// taken at different stages of its deployment can be compared for everything
/// else.
fn configuration(record: &Value) -> Value {
    let mut stripped = record.clone();
    if let Some(object) = stripped.as_object_mut() {
        object.remove("status");
    }
    stripped
}

/// The operation-log entry `id` of the distribution `name`.
async fn log_entry(emulator: &Emulator, case: &str, name: &str, id: &str) -> Value {
    let body = emulator
        .ok("GetOperationsForResource", json!({"resourceName": name}))
        .await;
    assert!(
        body["nextPageToken"].is_null(),
        "case {case}: expected every log entry of {name} on one page, got {body:?}"
    );
    body["operations"]
        .as_array()
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry["id"].as_str() == Some(id))
                .cloned()
        })
        .unwrap_or_else(|| {
            panic!("case {case}: expected the log of {name} to hold {id}, got {body:?}")
        })
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One distribution and the stage of its own deployment its cache is reset at.
///
/// The rows differ along everything the rule generalises over: when the reset
/// lands relative to the deployment the clock already owed the distribution, the
/// published bundle plan the distribution runs on, and the kind of resource it
/// pulls content from.
struct Row {
    case: &'static str,
    distribution_name: &'static str,
    /// An entry of this service's published CDN plans
    /// (`crate::state::DISTRIBUTION_BUNDLE_SPECS`).
    bundle_id: &'static str,
    origin_kind: OriginKind,
    origin_name: &'static str,
    when: ResetWhen,
}

/// The table.
const ROWS: &[Row] = &[
    Row {
        case: "a reset while the distribution is still deploying, cheapest plan, instance origin",
        distribution_name: "clock-deploying-cdn",
        bundle_id: "small_1_0",
        origin_kind: OriginKind::Instance,
        origin_name: "clock-origin-instance",
        when: ResetWhen::WhileDeploying,
    },
    Row {
        case: "a reset once the distribution is deployed, middle plan, bucket origin",
        distribution_name: "clock-deployed-cdn",
        bundle_id: "medium_1_0",
        origin_kind: OriginKind::Bucket,
        origin_name: "clock-origin-bucket",
        when: ResetWhen::OnceDeployed,
    },
    Row {
        case: "a reset while the distribution is still deploying, largest plan, bucket origin",
        distribution_name: "clock_deploying_large_cdn",
        bundle_id: "large_1_0",
        origin_kind: OriginKind::Bucket,
        origin_name: "clock-origin-second-bucket",
        when: ResetWhen::WhileDeploying,
    },
    Row {
        case: "a reset once the distribution is deployed, cheapest plan, instance origin",
        distribution_name: "clock-deployed-small-cdn",
        bundle_id: "small_1_0",
        origin_kind: OriginKind::Instance,
        origin_name: "clock-origin-second-instance",
        when: ResetWhen::OnceDeployed,
    },
];

/// RULE: a reset of a distribution's cached content is served whatever stage the
/// distribution's own deployment is at, changes no member of the distribution
/// record, leaves the deployment the clock owed that distribution to finish on
/// its own, and settles its own status and its operation-log entry on a clock of
/// its own.
#[tokio::test]
async fn a_cache_reset_settles_on_its_own_clock_and_changes_no_configuration() {
    let emulator = Emulator::new();
    // How many rows read the reset back inside the span its invalidation is owed,
    // which is the read that says the record settles on the clock and not on the
    // reading.
    let mut probed_inside_the_span = 0_usize;

    for row in ROWS {
        let case = row.case;
        let name = row.distribution_name;
        create_origin(&emulator, row.origin_kind, row.origin_name).await;
        let created_at = Instant::now();
        create_distribution(&emulator, name, row.bundle_id, row.origin_name).await;
        if row.when == ResetWhen::OnceDeployed {
            wait_for_distribution_status(&emulator, name, DEPLOYED).await;
        }

        // The record as it stands before the reset, to compare everything the
        // reset must leave alone against.
        let before = distribution(&emulator, name).await;
        assert_eq!(
            before["bundleId"].as_str(),
            Some(row.bundle_id),
            "case {case}: expected the created plan on the record, got {before:?}"
        );
        if row.when == ResetWhen::OnceDeployed {
            assert_eq!(
                before["status"].as_str(),
                Some(DEPLOYED),
                "case {case}: expected a deployed distribution before the reset, got {before:?}"
            );
        }

        let answered = emulator.ok(RESET, json!({"distributionName": name})).await;
        let reset_at = Instant::now();
        // What the answer of a reset always is, held to the sibling suite's one
        // copy of those expectations: in progress, with one unfinished log entry
        // about this distribution carrying a minted id.
        assert_reset_answer(case, name, &answered);
        let created = answered["createTime"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case}: expected a createTime, got {answered:?}"));
        let entry_id = answered["operation"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // The span is non-zero, so a read taken INSIDE it still reports the
        // invalidation as being applied: the reset does not settle because
        // somebody looked at it. A read that itself took longer than the span is
        // no probe of that, so it is counted rather than asserted on — and the
        // table as a whole must have probed at least once.
        let early = latest_reset(&emulator, Some(name)).await;
        let read_at = reset_at.elapsed();
        if read_at < OWED_SPAN {
            probed_inside_the_span += 1;
            assert_eq!(
                (early["status"].as_str(), early["createTime"].as_f64()),
                (Some(IN_PROGRESS), Some(created)),
                "case {case}: expected the reset read {read_at:?} after it was made, inside the \
                 {OWED_SPAN:?} span it is owed, to still be {IN_PROGRESS}, got {early:?}"
            );
        }

        // A deployment the clock already owed this distribution is neither
        // cancelled nor finished early by the reset: a distribution that reports
        // itself deployed straight after the reset can only have got there by
        // the span it was owed having passed.
        let straight_after = distribution(&emulator, name).await;
        let status_after = straight_after["status"].as_str().unwrap_or_default();
        match row.when {
            ResetWhen::WhileDeploying => {
                assert!(
                    [DEPLOYING, DEPLOYED].contains(&status_after),
                    "case {case}: expected the distribution's own status to stay {DEPLOYING} or \
                     {DEPLOYED} across a reset, got {straight_after:?}"
                );
                let since_create = created_at.elapsed();
                assert!(
                    status_after == DEPLOYING || since_create >= OWED_SPAN,
                    "case {case}: expected the reset to leave the deployment the clock owed this \
                     distribution alone, but it was {DEPLOYED} {since_create:?} after the create, \
                     inside the {OWED_SPAN:?} span it was owed: {straight_after:?}"
                );
            }
            ResetWhen::OnceDeployed => assert_eq!(
                status_after, DEPLOYED,
                "case {case}: expected a reset to leave a deployed distribution {DEPLOYED}, got \
                 {straight_after:?}"
            ),
        }
        let settled_distribution = wait_for_distribution_status(&emulator, name, DEPLOYED).await;

        // And nothing else on the record moved: deleting cached content is not a
        // configuration change.
        assert_eq!(
            configuration(&settled_distribution),
            configuration(&before),
            "case {case}: expected every member but the deployment status to be what it was \
             before the reset, got {settled_distribution:?}"
        );

        // The read reports the reset this call recorded, by the timestamp the
        // call answered.
        let read = latest_reset(&emulator, Some(name)).await;
        assert_eq!(
            read["createTime"].as_f64(),
            Some(created),
            "case {case}: expected the read to report the reset just made, got {read:?}"
        );

        // Both records the reset left behind reach their terminal value on the
        // clock: the span it was owed is waited out, and what the read inside
        // that span reported above is what says the value settled on the clock
        // rather than on the reading.
        let waited = reset_at.elapsed();
        if let Some(remaining) = (OWED_SPAN + SETTLE_MARGIN).checked_sub(waited) {
            tokio::time::sleep(remaining).await;
        }
        let completed = latest_reset(&emulator, Some(name)).await;
        assert_eq!(
            (
                completed["status"].as_str(),
                completed["createTime"].as_f64()
            ),
            (Some(COMPLETED), Some(created)),
            "case {case}: expected the reset to be {COMPLETED} on its own timestamp once the span \
             it was owed had passed, got {completed:?}"
        );
        let finished = log_entry(&emulator, case, name, &entry_id).await;
        assert_eq!(
            (
                finished["status"].as_str(),
                finished["isTerminal"].as_bool(),
                finished["operationType"].as_str(),
            ),
            (Some(SUCCEEDED), Some(true), Some(OPERATION_TYPE)),
            "case {case}: expected the reset's log entry to have finished, got {finished:?}"
        );

        // Reading either record again changes neither of them, nor the
        // distribution they belong to.
        assert_eq!(
            latest_reset(&emulator, Some(name)).await,
            completed,
            "case {case}: expected a second read of the reset to report the same record"
        );
        assert_eq!(
            log_entry(&emulator, case, name, &entry_id).await,
            finished,
            "case {case}: expected a second read of the log entry to report the same entry"
        );
        assert_eq!(
            configuration(&distribution(&emulator, name).await),
            configuration(&before),
            "case {case}: expected the distribution record to stay as it was after the reset \
             settled"
        );
    }

    // The inside-the-span read is what this suite exists for, so it must have
    // actually happened: a machine slow enough to miss it on every row leaves
    // the clock-versus-read distinction untested, and that is a failure of the
    // case rather than a pass.
    assert!(
        probed_inside_the_span > 0,
        "case the table as a whole: expected at least one of the {} rows to read the reset back \
         inside the {OWED_SPAN:?} span it is owed, got none",
        ROWS.len()
    );
}
