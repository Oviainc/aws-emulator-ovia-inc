// SPDX-License-Identifier: CC0-1.0

//! `RemoveRegion` end to end over the `awsJson1_1` wire.
//!
//! THE RULE: `RemoveRegion` starts the removal of ONE additional Region of the instance its
//! `InstanceArn` names — "The Region status is set to REMOVING and the Region record is
//! deleted when the workflow completes" — so the Region reports the `REMOVING` the model's
//! `RegionStatus` enum declares (`smithy_query(query="shape",shape="RegionStatus")` →
//! `ACTIVE | ADDING | REMOVING`) until the removal falls due one never-zero span later, and
//! once it is due the instance is no longer enabled in that Region. Every other Region of
//! the instance, and the instance itself, are untouched. The operation's documented
//! preconditions are refused rather than ignored: the request "must be made from the
//! primary Region", "the target Region cannot be the primary Region", "no other add or
//! remove Region workflows can be in progress", and a Region or instance that does not
//! exist is the modelled `ResourceNotFoundException`.
//!
//! The rows span what the rule generalises over: the primary Region of the instance, the
//! Region names the model's `RegionName` pattern `^([a-z]+-){2,3}\d$` admits (two- and
//! three-prefix spellings, several geographies, the GovCloud and China partitions), how
//! many additional Regions the instance holds, and the POSITION of the removed one among
//! them (first, middle, last, only).
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints and
//! the `X-Amz-Target` routing are exercised rather than bypassed; the harness is the one
//! `ssoadmin_permission_sets.rs` owns. Additional Regions are seeded through the service's
//! own `state::Instance::add_region`, because the operation that adds one (`AddRegion`) is
//! still a scaffold stub with an issue of its own; and where a test has to be on the far
//! side of a due instant it moves the STORED instant rather than sleeping — the transition
//! itself is still applied by the service's own `materialize_due`, exactly as in
//! production.
//!
//! One further rule lives here, because this is where a Region record is seeded: the
//! number of Regions one instance holds is bounded by the emulator's own per-instance
//! memory ceiling (`state::MAX_REGIONS_PER_INSTANCE`) — enabling a Region appends a record
//! that lives until a caller removes it, and the modelled `RegionName` admits unboundedly
//! many names — so an add past the ceiling is refused instead of growing the process.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    call_in_region, error_code, router_over, with_store, UNKNOWN_INSTANCE_ARN,
};

/// A router over a fresh engine, and that engine.
fn router_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (router_over(Arc::clone(&engine)), engine)
}

/// The ARN of the account's instance, read the way a client reads it — from `region`, so
/// the instance this account holds is enabled in `region` as its primary one.
async fn instance_arn_in(router: &MultiServiceRouter, region: &str) -> String {
    let (status, body) = call_in_region(router, region, "ListInstances", &json!({})).await;
    assert_eq!(
        status, 200,
        "region {region}: ListInstances expected 200, got {status} {body}"
    );
    body["Instances"][0]["InstanceArn"]
        .as_str()
        .unwrap_or_else(|| panic!("region {region}: no instance in {body}"))
        .to_string()
}

/// An account whose instance is enabled in `primary` and additionally enabled in
/// `additional`, and that instance's ARN.
///
/// The additional Regions are seeded through `state::Instance::add_region` — the one
/// place the service's own records append a Region — because `AddRegion`, the operation
/// that enables one over the wire, is still a scaffold stub tracked by its own issue.
/// Everything asserted afterwards goes over the wire.
async fn instance_enabled_in(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    primary: &str,
    additional: &[&str],
) -> String {
    let instance_arn = instance_arn_in(router, primary).await;
    with_store(engine, |store| {
        let instance = store
            .instance_mut(&instance_arn)
            .expect("the account's instance was just read over the wire");
        for name in additional {
            instance
                .add_region(state::Region::active(
                    name,
                    DateTime::from(SystemTime::now()),
                ))
                .expect("the seeded Regions stay under the emulator's per-instance ceiling");
        }
    });
    instance_arn
}

/// The `Regions` of the instance, as `DescribeInstance` reports them, keyed by name with
/// the reported `Status`.
async fn described_regions(
    router: &MultiServiceRouter,
    region: &str,
    instance_arn: &str,
) -> Vec<(String, String, bool)> {
    let (status, body) = call_in_region(
        router,
        region,
        "DescribeInstance",
        &json!({"InstanceArn": instance_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeInstance expected 200, got {status} {body}"
    );
    body["Regions"]
        .as_array()
        .unwrap_or_else(|| panic!("no Regions in {body}"))
        .iter()
        .map(|entry| {
            (
                entry["RegionName"].as_str().unwrap_or_default().to_string(),
                entry["Status"].as_str().unwrap_or_default().to_string(),
                entry["IsPrimaryRegion"] == Value::Bool(true),
            )
        })
        .collect()
}

/// The status `DescribeInstance` reports for `region_name`, or `None` when the instance is
/// no longer enabled in it.
async fn reported_status(
    router: &MultiServiceRouter,
    region: &str,
    instance_arn: &str,
    region_name: &str,
) -> Option<String> {
    described_regions(router, region, instance_arn)
        .await
        .into_iter()
        .find(|(name, _, _)| name == region_name)
        .map(|(_, status, _)| status)
}

/// `RemoveRegion` for `region_name`, called from `called_from`.
async fn remove_region(
    router: &MultiServiceRouter,
    called_from: &str,
    instance_arn: &str,
    region_name: &str,
) -> (u16, Value) {
    call_in_region(
        router,
        called_from,
        "RemoveRegion",
        &json!({"InstanceArn": instance_arn, "RegionName": region_name}),
    )
    .await
}

/// The stored instant at which the removal of `region_name` falls due.
fn removal_due(
    engine: &Arc<engine::InMemoryEngine>,
    instance_arn: &str,
    region_name: &str,
) -> Option<SystemTime> {
    with_store(engine, |store| {
        store
            .instance(instance_arn)
            .expect("the instance is still held")
            .region(region_name)
            .expect("the Region is still held while it is being removed")
            .removed_due
    })
}

/// Move the stored removal instant of `region_name` to `due_at`, so both sides of it can be
/// observed without the test sleeping.
fn hold_removal(
    engine: &Arc<engine::InMemoryEngine>,
    instance_arn: &str,
    region_name: &str,
    due_at: SystemTime,
) {
    with_store(engine, |store| {
        let instance = store.instance_mut(instance_arn).expect("the instance");
        let region = instance.region_mut(region_name).expect("the Region");
        region.removed_due = Some(due_at);
    });
}

/// One row of the removal table.
struct RemovalCase {
    label: &'static str,
    /// The Region the instance is enabled in — the one `RemoveRegion` must be called from.
    primary: &'static str,
    /// The additional Regions the instance holds, in the order it holds them.
    additional: &'static [&'static str],
    /// Which of them this row removes.
    target: &'static str,
}

/// The rows: the primary Region, the number of additional Regions and the POSITION of the
/// removed one all vary, over Region names spanning the model's `RegionName` pattern
/// `^([a-z]+-){2,3}\d$` — two- and three-prefix spellings, four geographies, the GovCloud
/// and China partitions.
const REMOVALS: &[RemovalCase] = &[
    RemovalCase {
        label: "the only additional Region",
        primary: "us-east-1",
        additional: &["us-west-2"],
        target: "us-west-2",
    },
    RemovalCase {
        label: "the first of three additional Regions",
        primary: "eu-west-1",
        additional: &["eu-central-1", "ap-southeast-4", "us-gov-west-1"],
        target: "eu-central-1",
    },
    RemovalCase {
        label: "the middle of three additional Regions",
        primary: "ap-northeast-1",
        additional: &["cn-north-1", "me-central-1", "us-gov-east-1"],
        target: "me-central-1",
    },
    RemovalCase {
        label: "the last of three additional Regions, a three-prefix name",
        primary: "us-gov-west-1",
        additional: &["us-east-2", "eu-north-1", "ap-southeast-4"],
        target: "ap-southeast-4",
    },
];

/// THE RULE: the Region the request names goes `REMOVING`, and its record is deleted when
/// the clock makes the removal due — leaving every other Region, and the instance, as they
/// were.
#[tokio::test]
async fn removing_a_region_runs_the_clock_and_deletes_the_region_record() {
    assert!(
        state::REGION_REMOVAL_SPAN > Duration::ZERO,
        "the removal span must never be zero, so a caller that reads straight after the \
         remove still observes REMOVING"
    );

    for case in REMOVALS {
        let RemovalCase {
            label,
            primary,
            additional,
            target,
        } = case;
        // One account per row: the Region an instance is enabled in is decided once, by
        // the request that enabled it.
        let (router, engine) = router_and_engine();
        let instance_arn = instance_enabled_in(&router, &engine, primary, additional).await;

        // Before the remove: every Region the instance holds is ACTIVE, and the primary
        // one is flagged.
        let before = described_regions(&router, primary, &instance_arn).await;
        assert_eq!(
            before.len(),
            additional.len() + 1,
            "case {label:?}: the instance must hold the primary Region and every additional \
             one, got {before:?}"
        );
        for (name, status, is_primary) in &before {
            assert_eq!(
                status, "ACTIVE",
                "case {label:?}: Region {name} must be ACTIVE before the remove, got \
                 {status:?}"
            );
            assert_eq!(
                *is_primary,
                name == primary,
                "case {label:?}: only the primary Region {primary} carries IsPrimaryRegion, \
                 got {name} => {is_primary}"
            );
        }

        let opened_at = SystemTime::now();
        let (status, removed) = remove_region(&router, primary, &instance_arn, target).await;
        let closed_at = SystemTime::now();
        assert_eq!(
            status, 200,
            "case {label:?}: RemoveRegion expected 200, got {status} {removed}"
        );
        assert_eq!(
            removed["Status"].as_str(),
            Some("REMOVING"),
            "case {label:?}: RemoveRegion answers the REMOVING status, got {removed}"
        );

        // The remove owes the deletion a due INSTANT one span out, not a deletion that had
        // already happened before the caller could observe the status.
        let due_at = removal_due(&engine, &instance_arn, target)
            .unwrap_or_else(|| panic!("case {label:?}: the remove owed a deletion instant"));
        assert!(
            due_at >= opened_at + state::REGION_REMOVAL_SPAN
                && due_at <= closed_at + state::REGION_REMOVAL_SPAN,
            "case {label:?}: the deletion must fall due one span after the call, got {due_at:?}"
        );

        // Before that instant the Region is still held, in the transient status, and every
        // other Region is as it was.
        hold_removal(
            &engine,
            &instance_arn,
            target,
            closed_at + Duration::from_secs(3_600),
        );
        let pending = described_regions(&router, primary, &instance_arn).await;
        assert_eq!(
            pending.len(),
            before.len(),
            "case {label:?}: a Region being removed is still reported, got {pending:?}"
        );
        for (name, status, _) in &pending {
            let expected = if name == target { "REMOVING" } else { "ACTIVE" };
            assert_eq!(
                status, expected,
                "case {label:?}: Region {name} expected {expected}, got {status:?}"
            );
        }

        // A REPEAT of the remove is idempotent: the same answer, and the instant the first
        // one fixed is kept rather than pushed out.
        let (status, again) = remove_region(&router, primary, &instance_arn, target).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a repeated RemoveRegion expected 200, got {status} {again}"
        );
        assert_eq!(
            again["Status"].as_str(),
            Some("REMOVING"),
            "case {label:?}: a repeated RemoveRegion answers REMOVING, got {again}"
        );
        assert_eq!(
            removal_due(&engine, &instance_arn, target),
            Some(closed_at + Duration::from_secs(3_600)),
            "case {label:?}: a repeated RemoveRegion must not push the deletion out"
        );

        // Once the deletion is due the Region record is gone, and nothing else went with
        // it.
        hold_removal(
            &engine,
            &instance_arn,
            target,
            closed_at - Duration::from_secs(1),
        );
        let after = described_regions(&router, primary, &instance_arn).await;
        let survivors: Vec<&str> = before
            .iter()
            .map(|(name, _, _)| name.as_str())
            .filter(|name| name != target)
            .collect();
        assert_eq!(
            after
                .iter()
                .map(|(name, _, _)| name.as_str())
                .collect::<Vec<&str>>(),
            survivors,
            "case {label:?}: exactly the removed Region goes, and the order of the rest is \
             kept, got {after:?}"
        );
        for (name, status, is_primary) in &after {
            assert_eq!(
                status, "ACTIVE",
                "case {label:?}: Region {name} must still be ACTIVE after the removal, got \
                 {status:?}"
            );
            assert_eq!(
                *is_primary,
                name == primary,
                "case {label:?}: the primary flag survives the removal, got {name} => \
                 {is_primary}"
            );
        }

        // The instance itself is untouched by the removal of one of its Regions.
        let (status, instance) = call_in_region(
            &router,
            primary,
            "DescribeInstance",
            &json!({"InstanceArn": instance_arn}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the instance still resolves, got {status} {instance}"
        );
        assert_eq!(
            instance["Status"].as_str(),
            Some("ACTIVE"),
            "case {label:?}: the instance stays ACTIVE, got {instance}"
        );
        assert_eq!(
            instance["PrimaryRegion"].as_str(),
            Some(*primary),
            "case {label:?}: the primary Region is unchanged, got {instance}"
        );

        // And a Region the instance is no longer enabled in names no resource.
        let (status, gone) = remove_region(&router, primary, &instance_arn, target).await;
        assert_eq!(
            status, 400,
            "case {label:?}: removing an already-removed Region expected 400, got {status} \
             {gone}"
        );
        assert_eq!(
            error_code(&gone),
            "ResourceNotFoundException",
            "case {label:?}: expected ResourceNotFoundException, got {gone}"
        );
    }
}

/// Which refusal a row expects.
struct RefusalCase {
    label: &'static str,
    /// The Region the request is made from.
    called_from: &'static str,
    /// The `RegionName` it asks to remove.
    target: &'static str,
    /// The `InstanceArn` it names, or `None` for the account's own instance.
    instance_arn: Option<&'static str>,
    /// The wire error code and HTTP status it must earn. IAM Identity Center answers every
    /// non-403 refusal of a request with HTTP 400 (`crate::scaffolded::modeled_error_status`).
    code: &'static str,
}

/// The primary Region of the instance every refusal row runs against.
const REFUSAL_PRIMARY: &str = "us-east-1";

/// Its additional Regions: one ACTIVE, so there is a legitimately removable Region to show
/// the refusals are about the row and not about the instance.
const REFUSAL_ADDITIONAL: &[&str] = &["eu-west-1", "ap-south-1"];

/// THE DOCUMENTED PRECONDITIONS: each one refused, with the modelled error, over a table of
/// inputs — and none of them changes what a later reader sees.
#[tokio::test]
async fn remove_region_refuses_what_its_preconditions_reject() {
    let rows = &[
        RefusalCase {
            label: "called from a Region that is not the primary one",
            called_from: "eu-west-1",
            target: "ap-south-1",
            instance_arn: None,
            code: "ValidationException",
        },
        RefusalCase {
            label: "called from a Region the instance is not even enabled in",
            called_from: "ap-southeast-4",
            target: "eu-west-1",
            instance_arn: None,
            code: "ValidationException",
        },
        RefusalCase {
            label: "the target Region is the primary Region",
            called_from: REFUSAL_PRIMARY,
            target: REFUSAL_PRIMARY,
            instance_arn: None,
            code: "ValidationException",
        },
        RefusalCase {
            label: "a Region the instance is not enabled in",
            called_from: REFUSAL_PRIMARY,
            target: "cn-northwest-1",
            instance_arn: None,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            label: "a three-prefix Region the instance is not enabled in",
            called_from: REFUSAL_PRIMARY,
            target: "us-gov-east-1",
            instance_arn: None,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            label: "an instance the account does not hold",
            called_from: REFUSAL_PRIMARY,
            target: "eu-west-1",
            instance_arn: Some(UNKNOWN_INSTANCE_ARN),
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            label: "an instance the account does not hold, in the China partition",
            called_from: REFUSAL_PRIMARY,
            target: "eu-west-1",
            instance_arn: Some("arn:aws-cn:sso:::instance/ssoins-1111111111111111"),
            code: "ResourceNotFoundException",
        },
    ];

    for row in rows {
        let (router, engine) = router_and_engine();
        let held_arn =
            instance_enabled_in(&router, &engine, REFUSAL_PRIMARY, REFUSAL_ADDITIONAL).await;
        let asked_arn = row.instance_arn.unwrap_or(held_arn.as_str());

        let (status, body) = remove_region(&router, row.called_from, asked_arn, row.target).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400, got {status} {body}",
            row.label
        );
        assert_eq!(
            error_code(&body),
            row.code,
            "case {:?}: expected {}, got {body}",
            row.label,
            row.code
        );
        let message = body["Message"]
            .as_str()
            .or_else(|| body["message"].as_str())
            .unwrap_or_default();
        assert!(
            !message.is_empty(),
            "case {:?}: the refusal must say what was wrong, got {body}",
            row.label
        );

        // A refused remove is not a remove: every Region the instance held is still there,
        // still ACTIVE.
        let after = described_regions(&router, REFUSAL_PRIMARY, &held_arn).await;
        assert_eq!(
            after.len(),
            REFUSAL_ADDITIONAL.len() + 1,
            "case {:?}: a refused remove must leave every Region in place, got {after:?}",
            row.label
        );
        for (name, region_status, _) in &after {
            assert_eq!(
                region_status, "ACTIVE",
                "case {:?}: Region {name} must be untouched by a refused remove, got \
                 {region_status:?}",
                row.label
            );
        }

        // And the Region a legitimate call names is still removable, from the primary
        // Region: the refusals above left the instance usable.
        let (status, removed) =
            remove_region(&router, REFUSAL_PRIMARY, &held_arn, REFUSAL_ADDITIONAL[0]).await;
        assert_eq!(
            status, 200,
            "case {:?}: a legitimate RemoveRegion after the refusal expected 200, got \
             {status} {removed}",
            row.label
        );
    }
}

/// "no other add or remove Region workflows can be in progress" — a workflow already
/// running on a Region of the instance is the modelled `ConflictException`, whichever of
/// the model's two non-terminal `RegionStatus` values it is in, and the target Region is
/// left as it was.
#[tokio::test]
async fn remove_region_conflicts_with_another_region_workflow_in_progress() {
    /// One row: which Region of the instance is busy, in which workflow, which Region the
    /// refused call asks to remove, and what the target still reports afterwards.
    struct ConflictCase {
        label: &'static str,
        busy: &'static str,
        busy_status: &'static str,
        /// Whether that workflow carries a removal instant — a remove workflow does, an
        /// add workflow does not.
        busy_removing: bool,
        target: &'static str,
        target_status_after: &'static str,
    }

    // The rows span the two non-terminal values of `RegionStatus` on ANOTHER Region, and
    // the target's OWN add workflow — an `ADDING` target is in progress too, so the remove
    // is refused rather than overwriting it (a target already `REMOVING` is this very
    // operation's own work, which `..._runs_the_clock_...` above drives as idempotent).
    let rows = &[
        ConflictCase {
            label: "another Region is ADDING",
            busy: "ap-south-1",
            busy_status: "ADDING",
            busy_removing: false,
            target: "eu-west-1",
            target_status_after: "ACTIVE",
        },
        ConflictCase {
            label: "another Region is REMOVING",
            busy: "ap-south-1",
            busy_status: "REMOVING",
            busy_removing: true,
            target: "eu-west-1",
            target_status_after: "ACTIVE",
        },
        ConflictCase {
            label: "the target Region's own add workflow is in progress",
            busy: "eu-west-1",
            busy_status: "ADDING",
            busy_removing: false,
            target: "eu-west-1",
            target_status_after: "ADDING",
        },
    ];

    for row in rows {
        let label = row.label;
        let (router, engine) = router_and_engine();
        let instance_arn =
            instance_enabled_in(&router, &engine, "us-east-1", &["eu-west-1", "ap-south-1"]).await;
        // The workflow in progress, seeded the way the clock holds one: a due instant far
        // enough out that it is still running at this instant.
        let busy_until = SystemTime::now() + Duration::from_secs(3_600);
        with_store(&engine, |store| {
            let region = store
                .instance_mut(&instance_arn)
                .expect("the instance")
                .region_mut(row.busy)
                .expect("the busy Region");
            region.status = row.busy_status.to_string();
            region.removed_due = row.busy_removing.then_some(busy_until);
        });

        let (status, body) = remove_region(&router, "us-east-1", &instance_arn, row.target).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {label:?}: expected ConflictException, got {body}"
        );

        // The refused call started no removal of its own: the target keeps the status it
        // had and owes no deletion.
        assert_eq!(
            reported_status(&router, "us-east-1", &instance_arn, row.target).await,
            Some(row.target_status_after.to_string()),
            "case {label:?}: a refused remove must leave the target Region as it was"
        );
        assert_eq!(
            removal_due(&engine, &instance_arn, row.target),
            None,
            "case {label:?}: a refused remove owes no deletion instant"
        );

        // Once the workflow is over, the same call succeeds — the conflict was about the
        // workflow, not about the request.
        with_store(&engine, |store| {
            let region = store
                .instance_mut(&instance_arn)
                .expect("the instance")
                .region_mut(row.busy)
                .expect("the busy Region");
            region.status = "ACTIVE".to_string();
            region.removed_due = None;
        });
        let (status, removed) =
            remove_region(&router, "us-east-1", &instance_arn, row.target).await;
        assert_eq!(
            status, 200,
            "case {label:?}: with no workflow in progress RemoveRegion expected 200, got \
             {status} {removed}"
        );
        assert_eq!(
            removed["Status"].as_str(),
            Some("REMOVING"),
            "case {label:?}: RemoveRegion answers REMOVING, got {removed}"
        );
    }
}

/// A Region name the model's `RegionName` pattern `^([a-z]+-){2,3}\d$` admits, distinct
/// for each `index`: the middle segment is the index written in lowercase letters, which
/// is what the pattern allows there (letters only, digits only in the last segment).
fn seeded_region_name(index: usize) -> String {
    let mut remaining = index;
    let mut letters = String::new();
    loop {
        letters.insert(
            0,
            char::from(b'a' + u8::try_from(remaining % 26).expect("a remainder below 26")),
        );
        remaining /= 26;
        if remaining == 0 {
            break;
        }
    }
    format!("eu-{letters}-1")
}

/// The emulator's own per-instance memory ceiling bounds caller-driven growth of an
/// instance's Regions: enabling a Region appends a record that lives until a caller
/// removes it, and the modelled `RegionName` admits unboundedly many names, so an add
/// past `state::MAX_REGIONS_PER_INSTANCE` is refused — naming the instance — rather than
/// growing the process, and no read ever reports more Regions than the ceiling.
#[tokio::test]
async fn an_instance_holds_at_most_the_emulators_region_ceiling() {
    let (router, engine) = router_and_engine();
    let instance_arn = instance_arn_in(&router, "us-east-1").await;

    // More adds than the ceiling admits, each a distinct Region name, through the one
    // place a Region record is appended.
    let attempts = state::MAX_REGIONS_PER_INSTANCE + 5;
    let (accepted, refusals) = with_store(&engine, |store| {
        let instance = store
            .instance_mut(&instance_arn)
            .expect("the account's instance was just read over the wire");
        let mut accepted = 0usize;
        let mut refusals: Vec<String> = Vec::new();
        for index in 0..attempts {
            match instance.add_region(state::Region::active(
                &seeded_region_name(index),
                DateTime::from(SystemTime::now()),
            )) {
                Ok(()) => accepted += 1,
                Err(message) => refusals.push(message),
            }
        }
        (accepted, refusals)
    });

    // The primary Region already occupies one of the places the ceiling admits.
    assert_eq!(
        accepted,
        state::MAX_REGIONS_PER_INSTANCE - 1,
        "the instance must accept Regions up to the ceiling and no further, it accepted \
         {accepted}"
    );
    assert_eq!(
        refusals.len(),
        attempts - accepted,
        "every add past the ceiling must be refused, got {refusals:?}"
    );
    for message in &refusals {
        assert!(
            message.contains(&instance_arn),
            "a refusal must name the instance the ceiling was reached on, got {message:?}"
        );
    }

    // What a client reads is bounded by the same ceiling.
    let reported = described_regions(&router, "us-east-1", &instance_arn).await;
    assert_eq!(
        reported.len(),
        state::MAX_REGIONS_PER_INSTANCE,
        "the instance must report at most {} Regions, got {reported:?}",
        state::MAX_REGIONS_PER_INSTANCE
    );
}
