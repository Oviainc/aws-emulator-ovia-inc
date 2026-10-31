// SPDX-License-Identifier: CC0-1.0

//! A create dates the record it stores from the instant it RESOLVED the store, end to end
//! over the `awsJson1_1` wire.
//!
//! THE RULE: a call reads the wall clock ONCE and uses that one instant both to apply what
//! the clock already owes the store and to stamp the `CreatedDate` of what it stores — so a
//! stored date is never taken from an instant later than the one the state the call was
//! decided against was settled at.
//!
//! The rule is OBSERVABLE because the store a create resolves can itself be carrying a
//! transition the clock owes it: an instance holding a Region whose extension workflow is in
//! flight (`RegionMetadata$Status`: `ACTIVE | ADDING | REMOVING`, from
//! `smithy_query(query="shape",shape="RegionStatus")`) activates at an instant these rows
//! fix. One call then does two things with its instant: it decides whether that activation
//! is already owed, and it stamps the `CreatedDate` of the record it stores. With one instant
//! the two agree exactly — the stamp is at or past the Region's due instant if and only if
//! the same call applied that activation. A second clock reading inside the handler breaks
//! that agreement whenever the Region's due instant falls between the two readings: the call
//! resolves a Region still `ADDING` and yet dates its own record from a clock already past
//! that Region's instant.
//!
//! The rows span what the rule generalises over: every operation of this crate that stamps a
//! `CreatedDate` onto a record it stores under an organization instance
//! (`CreatePermissionSet`, `CreateApplication`, `CreateAccountAssignment`, the deletion
//! status a `DeleteAccountAssignment` records and the provisioning status a
//! `ProvisionPermissionSet` records), Region names spanning the `RegionName` pattern, and a
//! pending activation from a millisecond to an hour away on either side of the call. The
//! sweep then walks that pending instant THROUGH a create in sub-microsecond steps, which is
//! where a second reading would show.
//!
//! The write side of the same rule for a stored DUE instant (rather than a record's own date)
//! is `instance_write_stamp_instant.rs`, and the read side — one response reporting the state
//! owed at one moment — is `instance_single_instant_read.rs`.
//!
//! Where a row has to put the pending activation on a known side of the call it moves the
//! STORED instant rather than sleeping; the transition itself is still applied by the
//! service's own `materialize_due`, exactly as in production, and every call goes over the
//! wire.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state::STATUS_ADDING;
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    call, hold_region_activation, organization_engine, region_activated, router_over, with_store,
};

/// The one application provider that backs a create of a customer managed OAuth 2.0
/// application.
///
/// SOURCE: `state::BUILT_IN_APPLICATION_PROVIDERS`, the catalogue
/// `ListApplicationProviders` publishes.
const PROVIDER_CUSTOM: &str = "arn:aws:sso::aws:applicationProvider/custom";

/// The `TargetId` the assignment rows name (modelled `^\d{12}$`, length 12..12).
const ACCOUNT_ID: &str = "123456789012";

/// The `PrincipalId` the assignment rows name, in the bare-GUID form the modelled pattern
/// `^([0-9a-f]{10}-|)[A-Fa-f0-9]{8}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{12}$`
/// admits.
const PRINCIPAL_ID: &str = "f81d4fae-7dec-11d0-a765-00a0c91e6bf6";

/// The `Name` the measured `CreatePermissionSet` of a row sends (`^[\w+=,.@-]+$`, 1..32), so
/// its record is told apart from the permission set another row's set-up created.
const MEASURED_SET_NAME: &str = "StampedSet";

/// The `Name` a row's SET-UP permission set carries, for the rows whose measured call needs
/// one to exist first.
const SET_UP_SET_NAME: &str = "SetUpSet";

/// A stored timestamp as the instant it holds.
///
/// `DateTime` carries whole seconds and sub-second nanoseconds, which is the precision the
/// stamp was taken at, so the comparison below is the one the handler's own instant makes.
fn as_system_time(stamp: &::aws_smithy_types::DateTime) -> SystemTime {
    let secs = u64::try_from(stamp.secs()).expect("a stored stamp is after the epoch");
    UNIX_EPOCH + Duration::new(secs, stamp.subsec_nanos())
}

/// The `CreatedDate` stored under the permission set named `name`, as an instant.
fn permission_set_created_date(
    engine: &Arc<engine::InMemoryEngine>,
    name: &str,
) -> Option<SystemTime> {
    with_store(engine, |store| {
        store
            .permission_sets
            .values()
            .find(|record| record.name == name)
            .map(|record| as_system_time(&record.created_date))
    })
}

/// One CREATE that stamps a `CreatedDate` onto the record it stores.
#[derive(Debug, Clone, Copy)]
enum StampedCreate {
    /// `CreatePermissionSet`: the permission set's own `CreatedDate`.
    PermissionSet,
    /// `CreateApplication`: the application's own `CreatedDate`.
    Application,
    /// `CreateAccountAssignment`: the `CreatedDate` of the creation status it records.
    AccountAssignment,
    /// `DeleteAccountAssignment`: the `CreatedDate` of the deletion status it records — a
    /// delete that STORES a dated record, so the rule is not only about creates of
    /// resources.
    AccountAssignmentDeletion,
    /// `ProvisionPermissionSet`: the `CreatedDate` of the provisioning status it records.
    Provisioning,
}

impl StampedCreate {
    /// The operation whose stamp is measured.
    fn operation(self) -> &'static str {
        match self {
            Self::PermissionSet => "CreatePermissionSet",
            Self::Application => "CreateApplication",
            Self::AccountAssignment => "CreateAccountAssignment",
            Self::AccountAssignmentDeletion => "DeleteAccountAssignment",
            Self::Provisioning => "ProvisionPermissionSet",
        }
    }

    /// Everything that has to exist before the measured call, created over the wire, and the
    /// request body the measured call then sends.
    async fn prepare(self, router: &MultiServiceRouter, instance_arn: &str, case: &str) -> Value {
        match self {
            Self::PermissionSet => json!({"InstanceArn": instance_arn, "Name": MEASURED_SET_NAME}),
            Self::Application => json!({
                "InstanceArn": instance_arn,
                "ApplicationProviderArn": PROVIDER_CUSTOM,
                "Name": "StampedApp",
            }),
            Self::AccountAssignment => {
                let permission_set_arn =
                    create_permission_set(router, instance_arn, SET_UP_SET_NAME, case).await;
                assignment_body(instance_arn, &permission_set_arn)
            }
            Self::AccountAssignmentDeletion => {
                let permission_set_arn =
                    create_permission_set(router, instance_arn, SET_UP_SET_NAME, case).await;
                let body = assignment_body(instance_arn, &permission_set_arn);
                let (status, response) = call(router, "CreateAccountAssignment", &body).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the assignment this delete withdraws must be granted \
                     first, got {status} {response}"
                );
                body
            }
            Self::Provisioning => {
                let permission_set_arn =
                    create_permission_set(router, instance_arn, SET_UP_SET_NAME, case).await;
                json!({
                    "InstanceArn": instance_arn,
                    "PermissionSetArn": permission_set_arn,
                    "TargetType": "AWS_ACCOUNT",
                    "TargetId": ACCOUNT_ID,
                })
            }
        }
    }

    /// The `CreatedDate` the measured call STORED, as an instant.
    fn stamp(self, engine: &Arc<engine::InMemoryEngine>) -> Option<SystemTime> {
        match self {
            Self::PermissionSet => permission_set_created_date(engine, MEASURED_SET_NAME),
            Self::Application => with_store(engine, |store| {
                store
                    .applications
                    .values()
                    .next()
                    .map(|record| as_system_time(&record.created_date))
            }),
            Self::AccountAssignment => with_store(engine, |store| {
                store
                    .assignment_operations
                    .values()
                    .next()
                    .map(|record| as_system_time(&record.created_date))
            }),
            Self::AccountAssignmentDeletion => with_store(engine, |store| {
                store
                    .assignment_deletions
                    .last()
                    .map(|record| as_system_time(&record.created_date))
            }),
            Self::Provisioning => with_store(engine, |store| {
                store
                    .permission_set_provisionings
                    .last()
                    .map(|record| as_system_time(&record.created_date))
            }),
        }
    }
}

/// The (instance, account, permission set, principal type, principal id) tuple the
/// assignment rows grant and withdraw.
fn assignment_body(instance_arn: &str, permission_set_arn: &str) -> Value {
    json!({
        "InstanceArn": instance_arn,
        "TargetId": ACCOUNT_ID,
        "TargetType": "AWS_ACCOUNT",
        "PermissionSetArn": permission_set_arn,
        "PrincipalType": "GROUP",
        "PrincipalId": PRINCIPAL_ID,
    })
}

/// Create one permission set over the wire and answer its minted ARN.
async fn create_permission_set(
    router: &MultiServiceRouter,
    instance_arn: &str,
    name: &str,
    case: &str,
) -> String {
    let (status, body) = call(
        router,
        "CreatePermissionSet",
        &json!({"InstanceArn": instance_arn, "Name": name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreatePermissionSet expected 200, got {status} {body}"
    );
    body["PermissionSet"]["PermissionSetArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a create without a PermissionSetArn: {body}"))
        .to_string()
}

/// Start a Region's EXTENSION workflow over the wire, so the instance the measured call
/// resolves carries a transition the clock still owes it.
async fn start_region_addition(
    router: &MultiServiceRouter,
    instance_arn: &str,
    region_name: &str,
    case: &str,
) {
    let (status, body) = call(
        router,
        "AddRegion",
        &json!({"InstanceArn": instance_arn, "RegionName": region_name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: AddRegion expected 200, got {status} {body}"
    );
    assert_eq!(
        body["Status"].as_str(),
        Some(STATUS_ADDING),
        "case {case:?}: an extension workflow starts ADDING, got {body}"
    );
}

/// One row: the call whose stamp is measured, the Region whose activation is pending while it
/// runs, and where that activation's due instant sits relative to the call.
struct StampCase {
    label: &'static str,
    create: StampedCreate,
    region_name: &'static str,
    /// How far from the call the pending activation's due instant is placed.
    offset: Duration,
    /// `true` when that instant is in the PAST, so the activation is already owed.
    settled: bool,
}

/// The rows: every operation of this crate that stamps a record's own `CreatedDate` under an
/// organization instance, Region names spanning `RegionName`'s `^([a-z]+-){2,3}\d$`, and the
/// pending activation from a millisecond to an hour away on either side of the call.
const STAMP_CASES: &[StampCase] = &[
    StampCase {
        label: "a permission set, activation owed an hour before the create",
        create: StampedCreate::PermissionSet,
        region_name: "us-west-2",
        offset: Duration::from_secs(3_600),
        settled: true,
    },
    StampCase {
        label: "a permission set, activation due a second after the create",
        create: StampedCreate::PermissionSet,
        region_name: "eu-central-1",
        offset: Duration::from_secs(1),
        settled: false,
    },
    StampCase {
        label: "an application, activation owed a millisecond before the create",
        create: StampedCreate::Application,
        region_name: "ap-southeast-2",
        offset: Duration::from_millis(1),
        settled: true,
    },
    StampCase {
        label: "an account assignment, activation due an hour after the create",
        create: StampedCreate::AccountAssignment,
        region_name: "us-gov-west-1",
        offset: Duration::from_secs(3_600),
        settled: false,
    },
    StampCase {
        label: "an assignment deletion status, activation owed a second before the delete",
        create: StampedCreate::AccountAssignmentDeletion,
        region_name: "cn-north-1",
        offset: Duration::from_secs(1),
        settled: true,
    },
    StampCase {
        label: "a provisioning status, activation due an hour after the provisioning",
        create: StampedCreate::Provisioning,
        region_name: "ap-northeast-1",
        offset: Duration::from_secs(3_600),
        settled: false,
    },
];

/// THE RULE, over the table: the `CreatedDate` a call stamps onto what it stores is the very
/// instant it resolved the store with, so it is at or past a pending activation's due instant
/// exactly when that same call applied the activation — and it falls inside the interval the
/// call itself ran in.
#[tokio::test]
async fn a_stored_created_date_is_the_instant_the_call_resolved_the_store_at() {
    for case in STAMP_CASES {
        let label = case.label;
        let (engine, instance_arn) = organization_engine();
        let router = router_over(Arc::clone(&engine));
        let operation = case.create.operation();
        let body = case.create.prepare(&router, &instance_arn, label).await;
        start_region_addition(&router, &instance_arn, case.region_name, label).await;

        let due_at = if case.settled {
            SystemTime::now() - case.offset
        } else {
            SystemTime::now() + case.offset
        };
        hold_region_activation(&engine, &instance_arn, case.region_name, due_at);

        let before = SystemTime::now();
        let (status, response) = call(&router, operation, &body).await;
        let after = SystemTime::now();
        assert_eq!(
            status, 200,
            "case {label:?}: {operation} expected 200, got {status} {response}"
        );
        let stamp = case.create.stamp(&engine);
        assert!(
            stamp.is_some(),
            "case {label:?}: {operation} must stamp a CreatedDate onto the record it stores, \
             got {stamp:?}"
        );
        let stamp = stamp.expect("the stored stamp");
        let activated = region_activated(&engine, &instance_arn, case.region_name);
        assert_eq!(
            activated, case.settled,
            "case {label:?}: the call's own instant decides the pending activation, so it must \
             be applied={} after a due instant {due_at:?}, got applied={activated}",
            case.settled
        );
        assert_eq!(
            stamp >= due_at,
            activated,
            "case {label:?}: the stored CreatedDate {stamp:?} is the instant the call resolved \
             the store with, so it is at or past {due_at:?} exactly when the same call applied \
             that activation (applied={activated})"
        );
        assert!(
            stamp >= before && stamp <= after,
            "case {label:?}: the stored CreatedDate must be the call's own instant, in \
             [{before:?}, {after:?}], got {stamp:?}"
        );
    }
}

/// How many positions of the pending activation's due instant the sweep below tries.
const SWEEP_STEPS: u32 = 400;

/// The step between them: a quarter of a microsecond, finer than the handler's own work, so
/// the sweep walks that instant THROUGH the call rather than around it — the window a second
/// clock reading inside the handler would open is under a microsecond wide.
const SWEEP_STEP: Duration = Duration::from_nanos(250);

/// Half the span the positions cover, so the sweep reaches the same distance either side of
/// the call's own instant.
const SWEEP_HALF_SPAN: Duration = Duration::from_nanos(250 * (SWEEP_STEPS as u64) / 2);

/// Where the sweep's first measurement puts the pending activation: far enough ahead that the
/// warm-up create cannot reach it, so the Region is still `ADDING` and the warm-up measures
/// only how far the call's own instant falls from the instant the loop places a due instant
/// from.
const SWEEP_WARM_UP_OFFSET: Duration = Duration::from_secs(3_600);

/// The same rule as an INVARIANT, swept across the call itself: wherever the pending
/// activation's due instant falls relative to the create that resolves it — before, during,
/// after — the `CreatedDate` that create stores is at or past that instant exactly when the
/// same call applied the activation. Any position where the call resolves a Region still
/// `ADDING` and yet dates its record from a clock past that Region's instant (or the reverse)
/// is two instants in one call.
///
/// The positions are centred on the lag the previous step measured between the instant the
/// loop places the due instant from and the instant the call stamps, so the sweep stays over
/// the call however fast the machine running it is.
#[tokio::test]
async fn no_sweep_position_dates_a_record_from_a_clock_later_than_the_state_it_resolved() {
    let (engine, instance_arn) = organization_engine();
    let router = router_over(Arc::clone(&engine));
    let region_name = "ap-northeast-1";
    start_region_addition(&router, &instance_arn, region_name, "sweep").await;

    let mut centre = {
        let anchor = SystemTime::now();
        hold_region_activation(
            &engine,
            &instance_arn,
            region_name,
            anchor + SWEEP_WARM_UP_OFFSET,
        );
        let name = "SweepWarmUp";
        create_permission_set(&router, &instance_arn, name, "sweep warm-up").await;
        let stamp = permission_set_created_date(&engine, name)
            .expect("the warm-up create stores a CreatedDate");
        stamp
            .duration_since(anchor)
            .expect("the call's instant is not before the step that set it up")
    };

    for step in 0..SWEEP_STEPS {
        let offset = (centre + SWEEP_STEP * step).saturating_sub(SWEEP_HALF_SPAN);
        let anchor = SystemTime::now();
        let due_at = anchor + offset;
        hold_region_activation(&engine, &instance_arn, region_name, due_at);

        let case = format!("pending activation due {offset:?} after the sweep step's set-up");
        let name = format!("Sweep{step}");
        create_permission_set(&router, &instance_arn, &name, &case).await;
        let stamp = permission_set_created_date(&engine, &name).unwrap_or_else(|| {
            panic!("case {case:?}: the create must store a CreatedDate under {name:?}")
        });
        let activated = region_activated(&engine, &instance_arn, region_name);
        assert_eq!(
            stamp >= due_at,
            activated,
            "case {case:?}: one instant decides both, so the stored CreatedDate {stamp:?} is at \
             or past {due_at:?} exactly when the same call applied that activation \
             (applied={activated})"
        );
        centre = stamp.duration_since(anchor).unwrap_or(centre);
    }
}
