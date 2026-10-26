// SPDX-License-Identifier: CC0-1.0

//! `AddRegion` end to end over the `awsJson1_1` wire.
//!
//! THE RULE: `AddRegion` enables the instance its `InstanceArn` names in ONE more Region —
//! "This operation initiates an asynchronous workflow to replicate the IAM Identity Center
//! instance to the target Region. The Region status is set to ADDING at first and changes
//! to ACTIVE when the workflow completes." — so the Region is appended to the instance's
//! own records and reports the `ADDING` the model's `RegionStatus` enum declares
//! (`smithy_query(query="shape",shape="RegionStatus")` → `ACTIVE | ADDING | REMOVING`)
//! until its activation falls due one never-zero span later, and `ACTIVE` from then on. A
//! repeat of the add while that workflow is in flight is idempotent and cannot push the
//! activation out. Every other Region of the instance, and the instance itself, are
//! untouched. What the add cannot serve is refused rather than ignored: an `InstanceArn`
//! this account holds no instance for is the modelled `AccessDeniedException` (the only
//! not-found channel `AddRegion`'s model declares), a Region the instance is already
//! enabled in and a Region of it carrying another add or remove workflow are the modelled
//! `ConflictException`, and an add past the emulator's per-instance Region ceiling
//! (`state::MAX_REGIONS_PER_INSTANCE`) is the modelled `ServiceQuotaExceededException`.
//!
//! The rows span what the rule generalises over: the Region names the model's `RegionName`
//! pattern `^([a-z]+-){2,3}\d$` admits (two- and three-prefix spellings, several
//! geographies, the GovCloud and China partitions), how many Regions one instance is
//! enabled in, the ORDER the records end up in, and the Region the CALL is made in —
//! `AddRegion` documents no Region a request must come from, unlike `RemoveRegion`, so
//! rows add from the instance's primary Region and from a Region it is not enabled in
//! alike.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints and
//! the `X-Amz-Target` routing are exercised rather than bypassed; the harness is the one
//! `ssoadmin_permission_sets.rs` owns. Where a test has to be on the far side of a due
//! instant it moves the STORED instant rather than sleeping — the transition itself is
//! still applied by the service's own `materialize_due`, exactly as in production.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

/// `AddRegion` for `region_name`, called from `called_from`.
async fn add_region(
    router: &MultiServiceRouter,
    called_from: &str,
    instance_arn: &str,
    region_name: &str,
) -> (u16, Value) {
    call_in_region(
        router,
        called_from,
        "AddRegion",
        &json!({"InstanceArn": instance_arn, "RegionName": region_name}),
    )
    .await
}

/// One `RegionMetadata` entry as a reader projects it.
#[derive(Debug, Clone, PartialEq)]
struct ReportedRegion {
    name: String,
    status: String,
    is_primary: bool,
    added_date: Option<f64>,
}

/// The `Regions` of the instance, as `DescribeInstance` reports them, in the order it
/// reports them.
async fn described_regions(
    router: &MultiServiceRouter,
    region: &str,
    instance_arn: &str,
) -> Vec<ReportedRegion> {
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
        .map(|entry| ReportedRegion {
            name: entry["RegionName"].as_str().unwrap_or_default().to_string(),
            status: entry["Status"].as_str().unwrap_or_default().to_string(),
            is_primary: entry["IsPrimaryRegion"] == Value::Bool(true),
            added_date: entry["AddedDate"].as_f64(),
        })
        .collect()
}

/// The entry `DescribeInstance` reports for `region_name`, or `None` when the instance is
/// not enabled in it.
async fn reported_region(
    router: &MultiServiceRouter,
    region: &str,
    instance_arn: &str,
    region_name: &str,
) -> Option<ReportedRegion> {
    described_regions(router, region, instance_arn)
        .await
        .into_iter()
        .find(|entry| entry.name == region_name)
}

/// The stored instant at which the extension workflow of `region_name` falls due, or
/// `None` once its activation has been applied.
fn activation_due(
    engine: &Arc<engine::InMemoryEngine>,
    instance_arn: &str,
    region_name: &str,
) -> Option<SystemTime> {
    with_store(engine, |store| {
        store
            .instance(instance_arn)
            .expect("the instance is still held")
            .region(region_name)
            .expect("the Region the add appended")
            .active_due
    })
}

/// Move the stored activation instant of `region_name` to `due_at`, so both sides of it can
/// be observed without the test sleeping.
fn hold_activation(
    engine: &Arc<engine::InMemoryEngine>,
    instance_arn: &str,
    region_name: &str,
    due_at: SystemTime,
) {
    with_store(engine, |store| {
        let instance = store.instance_mut(instance_arn).expect("the instance");
        let region = instance.region_mut(region_name).expect("the Region");
        region.active_due = Some(due_at);
    });
}

/// An instant as the epoch seconds a timestamp member carries on the wire.
fn epoch_seconds(instant: SystemTime) -> f64 {
    instant
        .duration_since(UNIX_EPOCH)
        .expect("the test clock is after the epoch")
        .as_secs_f64()
}

/// One row of the addition table.
struct AdditionCase {
    label: &'static str,
    /// The Region the account's instance is enabled in — its primary one.
    primary: &'static str,
    /// The Regions this row adds, in the order it adds them.
    added: &'static [&'static str],
    /// The Region the `AddRegion` calls are MADE in.
    called_from: &'static str,
}

/// The rows: the primary Region, the number of Regions added and the Region the call is
/// made in all vary, over Region names spanning the model's `RegionName` pattern
/// `^([a-z]+-){2,3}\d$` — two- and three-prefix spellings, several geographies, the
/// GovCloud and China partitions.
const ADDITIONS: &[AdditionCase] = &[
    AdditionCase {
        label: "one Region, added from the primary one",
        primary: "us-east-1",
        added: &["us-west-2"],
        called_from: "us-east-1",
    },
    AdditionCase {
        label: "three Regions, two- and three-prefix names",
        primary: "eu-west-1",
        added: &["eu-central-1", "ap-southeast-4", "us-gov-west-1"],
        called_from: "eu-west-1",
    },
    AdditionCase {
        label: "a China-partition instance, added from a Region it is not enabled in",
        primary: "cn-north-1",
        added: &["cn-northwest-1"],
        called_from: "us-east-1",
    },
    AdditionCase {
        label: "a GovCloud primary, two Regions added from elsewhere",
        primary: "us-gov-west-1",
        added: &["us-gov-east-1", "me-central-1"],
        called_from: "eu-north-1",
    },
];

/// THE RULE: the Region the request names is appended to the instance in `ADDING`, becomes
/// `ACTIVE` when the clock makes its extension workflow due, and leaves every other Region
/// and the instance itself as they were.
#[tokio::test]
async fn adding_a_region_runs_the_clock_and_enables_the_instance_in_it() {
    assert!(
        state::REGION_ACTIVATION_SPAN > Duration::ZERO,
        "the activation span must never be zero, so a caller that reads straight after the \
         add still observes ADDING"
    );

    for case in ADDITIONS {
        let AdditionCase {
            label,
            primary,
            added,
            called_from,
        } = case;
        // One account per row: the Region an instance is enabled in is decided once, by the
        // request that enabled it.
        let (router, engine) = router_and_engine();
        let instance_arn = instance_arn_in(&router, primary).await;
        // The order the records are expected in: the primary Region first, then every
        // Region this row adds, in the order it adds them.
        let mut expected_order: Vec<&str> = vec![primary];

        for region_name in *added {
            let case = format!("{label} / {region_name}");
            let opened_at = SystemTime::now();
            let (status, answer) = add_region(&router, called_from, &instance_arn, region_name).await;
            let closed_at = SystemTime::now();
            assert_eq!(
                status, 200,
                "case {case:?}: AddRegion expected 200, got {status} {answer}"
            );
            assert_eq!(
                answer["Status"].as_str(),
                Some("ADDING"),
                "case {case:?}: AddRegion answers the ADDING status, got {answer}"
            );

            // The add owes the activation a due INSTANT one span out, not an activation
            // that had already happened before the caller could observe the status.
            let due_at = activation_due(&engine, &instance_arn, region_name)
                .unwrap_or_else(|| panic!("case {case:?}: the add owed an activation instant"));
            assert!(
                due_at >= opened_at + state::REGION_ACTIVATION_SPAN
                    && due_at <= closed_at + state::REGION_ACTIVATION_SPAN,
                "case {case:?}: the activation must fall due one span after the call, got \
                 {due_at:?}"
            );

            // The record is the instance's own: every reader of the instance answers from
            // it, in `ADDING`, flagged as an additional Region, dated the instant it was
            // added, and appended after the Regions the instance already held.
            expected_order.push(region_name);
            hold_activation(
                &engine,
                &instance_arn,
                region_name,
                closed_at + Duration::from_secs(3_600),
            );
            let pending = described_regions(&router, primary, &instance_arn).await;
            assert_eq!(
                pending
                    .iter()
                    .map(|entry| entry.name.as_str())
                    .collect::<Vec<&str>>(),
                expected_order,
                "case {case:?}: the add appends exactly one Region and keeps the order of \
                 the rest, got {pending:?}"
            );
            let entry = pending
                .iter()
                .find(|entry| entry.name == *region_name)
                .unwrap_or_else(|| panic!("case {case:?}: the added Region is reported"));
            assert_eq!(
                entry.status, "ADDING",
                "case {case:?}: an extension workflow in flight is ADDING, got {entry:?}"
            );
            assert!(
                !entry.is_primary,
                "case {case:?}: an added Region is not the primary one, got {entry:?}"
            );
            let added_date = entry
                .added_date
                .unwrap_or_else(|| panic!("case {case:?}: no AddedDate in {entry:?}"));
            assert!(
                added_date >= epoch_seconds(opened_at) - 1.0
                    && added_date <= epoch_seconds(closed_at) + 1.0,
                "case {case:?}: AddedDate must be the instant the Region was added \
                 ({:?}..{:?}), got {added_date}",
                epoch_seconds(opened_at),
                epoch_seconds(closed_at)
            );

            // A REPEAT of the add while the workflow is in flight is idempotent: the same
            // answer, and the instant the first one fixed is kept rather than pushed out.
            let (status, again) = add_region(&router, called_from, &instance_arn, region_name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a repeated AddRegion expected 200, got {status} {again}"
            );
            assert_eq!(
                again["Status"].as_str(),
                Some("ADDING"),
                "case {case:?}: a repeated AddRegion answers ADDING, got {again}"
            );
            assert_eq!(
                activation_due(&engine, &instance_arn, region_name),
                Some(closed_at + Duration::from_secs(3_600)),
                "case {case:?}: a repeated AddRegion must not push the activation out"
            );

            // Once the activation is due the Region is ACTIVE, and stays there across
            // repeated reads.
            hold_activation(
                &engine,
                &instance_arn,
                region_name,
                closed_at - Duration::from_secs(1),
            );
            for read in 0..2 {
                let entry = reported_region(&router, primary, &instance_arn, region_name)
                    .await
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: the Region is still enabled after its activation")
                    });
                assert_eq!(
                    entry.status, "ACTIVE",
                    "case {case:?}: read {read} of a Region whose workflow completed is \
                     ACTIVE, got {entry:?}"
                );
                assert_eq!(
                    entry.added_date, added_date.into(),
                    "case {case:?}: read {read} must keep the AddedDate the add set, got \
                     {entry:?}"
                );
            }
            assert_eq!(
                activation_due(&engine, &instance_arn, region_name),
                None,
                "case {case:?}: an applied activation owes nothing further"
            );

            // And a Region the instance is now enabled in cannot be added again.
            let (status, conflict) =
                add_region(&router, called_from, &instance_arn, region_name).await;
            assert_eq!(
                status, 400,
                "case {case:?}: adding an enabled Region expected 400, got {status} {conflict}"
            );
            assert_eq!(
                error_code(&conflict),
                "ConflictException",
                "case {case:?}: expected ConflictException, got {conflict}"
            );
        }

        // The instance itself is untouched by the Regions added to it: its own status and
        // primary Region are what the enablement set, and exactly one record carries the
        // primary flag.
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
        let final_regions = described_regions(&router, primary, &instance_arn).await;
        assert_eq!(
            final_regions
                .iter()
                .filter(|entry| entry.is_primary)
                .map(|entry| entry.name.as_str())
                .collect::<Vec<&str>>(),
            vec![*primary],
            "case {label:?}: exactly the primary Region carries IsPrimaryRegion, got \
             {final_regions:?}"
        );
        for entry in &final_regions {
            assert_eq!(
                entry.status, "ACTIVE",
                "case {label:?}: every Region of the instance is ACTIVE now, got {entry:?}"
            );
        }
    }
}

/// An `InstanceArn` this account holds no instance for is the modelled
/// `AccessDeniedException` — the only not-found channel `AddRegion`'s model declares (it
/// carries no `ResourceNotFoundException`), the same channel `DescribeInstance` answers for
/// the same ARN. The rows span the spellings the model's `InstanceArn` pattern
/// `^arn:aws(-[a-z]{1,5}){0,3}:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$` admits.
#[tokio::test]
async fn adding_a_region_to_an_unknown_instance_is_denied() {
    // The ARNs: the `ssoins-` and bare `ins-` forms, and the partitions the pattern's
    // `(-[a-z]{1,5}){0,3}` segment admits.
    let unknown = [
        ("the standard partition", UNKNOWN_INSTANCE_ARN.to_string()),
        (
            "the bare ins- form",
            "arn:aws:sso:::instance/ins-1111111111111111".to_string(),
        ),
        (
            "the GovCloud partition",
            "arn:aws-us-gov:sso:::instance/ssoins-2222222222222222".to_string(),
        ),
        (
            "the China partition",
            "arn:aws-cn:sso:::instance/ssoins-3333333333333333".to_string(),
        ),
    ];

    let (router, _engine) = router_and_engine();
    // The account's own instance, so the store holds one and the refusal is about the ARN
    // the request named rather than about an account that holds nothing.
    let held = instance_arn_in(&router, "us-east-1").await;

    for (case, instance_arn) in unknown {
        assert_ne!(
            instance_arn, held,
            "case {case:?}: the row must name an instance this account does not hold"
        );
        let (status, body) = add_region(&router, "us-east-1", &instance_arn, "eu-west-3").await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected HTTP 403 for {instance_arn}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDeniedException",
            "case {case:?}: expected AccessDeniedException, got {body}"
        );
        // And nothing was enabled on the account's own instance by the refused call.
        assert_eq!(
            reported_region(&router, "us-east-1", &held, "eu-west-3").await,
            None,
            "case {case:?}: a refused add must enable no Region anywhere"
        );
    }
}

/// Which Region of the instance a conflict row makes busy, and with what.
struct ConflictCase {
    label: &'static str,
    /// The Region whose workflow is in progress, out of the instance's own Regions.
    busy: &'static str,
    /// The `RegionStatus` that Region carries.
    busy_status: &'static str,
    /// Whether its workflow is a REMOVAL (so the record owes a deletion) or an addition.
    busy_removing: bool,
    /// The Region this row then tries to add.
    target: &'static str,
}

/// A Region extension or removal workflow already running on a Region of the instance is
/// the modelled `ConflictException`: "no other add or remove Region workflows can be in
/// progress", as `RemoveRegion` states the rule for the pair. The rows span both
/// non-terminal `RegionStatus` values and both the primary and an additional Region as the
/// busy one.
#[tokio::test]
async fn adding_a_region_while_a_region_workflow_runs_is_a_conflict() {
    let rows = [
        ConflictCase {
            label: "an additional Region being removed",
            busy: "eu-west-1",
            busy_status: "REMOVING",
            busy_removing: true,
            target: "ap-northeast-2",
        },
        ConflictCase {
            label: "an additional Region still being added",
            busy: "ap-south-1",
            busy_status: "ADDING",
            busy_removing: false,
            target: "ap-northeast-2",
        },
        ConflictCase {
            label: "the primary Region still being added",
            busy: "us-east-1",
            busy_status: "ADDING",
            busy_removing: false,
            target: "eu-central-1",
        },
    ];

    for row in rows {
        let label = row.label;
        let (router, engine) = router_and_engine();
        let instance_arn = instance_arn_in(&router, "us-east-1").await;
        // Two additional Regions to be busy in, seeded ACTIVE through the service's own
        // records, then the row's workflow held the way the clock holds one: a due instant
        // far enough out that it is still running at this instant.
        let busy_until = SystemTime::now() + Duration::from_secs(3_600);
        with_store(&engine, |store| {
            let instance = store.instance_mut(&instance_arn).expect("the instance");
            for name in ["eu-west-1", "ap-south-1"] {
                instance
                    .add_region(state::Region::active(
                        name,
                        DateTime::from(SystemTime::now()),
                    ))
                    .expect("the seeded Regions stay under the per-instance ceiling");
            }
            let region = instance.region_mut(row.busy).expect("the busy Region");
            region.status = row.busy_status.to_string();
            region.removed_due = row.busy_removing.then_some(busy_until);
            region.active_due = (!row.busy_removing).then_some(busy_until);
        });

        let (status, body) = add_region(&router, "us-east-1", &instance_arn, row.target).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {label:?}: expected ConflictException, got {body}"
        );
        // The refused call enabled nothing: the target is not one of the instance's
        // Regions, and the busy one keeps the status it had.
        assert_eq!(
            reported_region(&router, "us-east-1", &instance_arn, row.target).await,
            None,
            "case {label:?}: a refused add must enable no Region"
        );
        assert_eq!(
            reported_region(&router, "us-east-1", &instance_arn, row.busy)
                .await
                .map(|entry| entry.status),
            Some(row.busy_status.to_string()),
            "case {label:?}: a refused add must leave the busy Region as it was"
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
            region.active_due = None;
        });
        let (status, added) = add_region(&router, "us-east-1", &instance_arn, row.target).await;
        assert_eq!(
            status, 200,
            "case {label:?}: with no workflow in progress AddRegion expected 200, got \
             {status} {added}"
        );
        assert_eq!(
            added["Status"].as_str(),
            Some("ADDING"),
            "case {label:?}: AddRegion answers ADDING, got {added}"
        );
    }
}

/// The number of Regions one instance holds is bounded by the emulator's own per-instance
/// memory ceiling (`state::MAX_REGIONS_PER_INSTANCE`) — a Region record lives until a
/// caller removes it, and the modelled `RegionName` admits unboundedly many names — so the
/// add that would cross it is refused with the modelled `ServiceQuotaExceededException`
/// instead of growing the process, while the one just under it is served.
#[tokio::test]
async fn adding_a_region_past_the_per_instance_ceiling_is_refused() {
    // Region names the model's `RegionName` pattern `^([a-z]+-){2,3}\d$` admits, distinct
    // and generated rather than written out: two prefixes and a trailing digit.
    let filler: Vec<String> = ('a'..='z')
        .flat_map(|letter| (1..=9).map(move |digit| format!("fill-{letter}-{digit}")))
        .take(state::MAX_REGIONS_PER_INSTANCE)
        .collect();
    assert!(
        filler.len() >= state::MAX_REGIONS_PER_INSTANCE - 1,
        "the filler must be able to fill one instance, got {} names",
        filler.len()
    );

    let (router, engine) = router_and_engine();
    let instance_arn = instance_arn_in(&router, "us-east-1").await;
    // One short of the ceiling, counting the primary Region the instance already holds:
    // seeded ACTIVE through the service's own records, so the wire add below is the one
    // that reaches the ceiling.
    with_store(&engine, |store| {
        let instance = store.instance_mut(&instance_arn).expect("the instance");
        for name in filler.iter().take(state::MAX_REGIONS_PER_INSTANCE - 2) {
            instance
                .add_region(state::Region::active(
                    name,
                    DateTime::from(SystemTime::now()),
                ))
                .expect("the seeded Regions stay under the ceiling");
        }
    });

    // One under the ceiling: served.
    let last = "eu-west-2";
    let (status, added) = add_region(&router, "us-east-1", &instance_arn, last).await;
    assert_eq!(
        status, 200,
        "case \"the last Region under the ceiling\": expected 200, got {status} {added}"
    );
    hold_activation(
        &engine,
        &instance_arn,
        last,
        SystemTime::now() - Duration::from_secs(1),
    );

    // One past it: refused, and the instance holds no more Regions than the ceiling.
    let (status, body) = add_region(&router, "us-east-1", &instance_arn, "eu-north-1").await;
    assert_eq!(
        status, 400,
        "case \"one past the ceiling\": expected 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "case \"one past the ceiling\": expected ServiceQuotaExceededException, got {body}"
    );
    let held = described_regions(&router, "us-east-1", &instance_arn).await;
    assert_eq!(
        held.len(),
        state::MAX_REGIONS_PER_INSTANCE,
        "case \"one past the ceiling\": the instance holds exactly the ceiling, got {}",
        held.len()
    );
    assert!(
        held.iter().all(|entry| entry.name != "eu-north-1"),
        "case \"one past the ceiling\": the refused add stored nothing, got {held:?}"
    );
}
