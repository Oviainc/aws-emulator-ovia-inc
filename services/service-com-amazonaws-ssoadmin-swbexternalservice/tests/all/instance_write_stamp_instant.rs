// SPDX-License-Identifier: CC0-1.0

//! A write dates what it STORES from the instant it resolved the store, end to end over
//! the `awsJson1_1` wire.
//!
//! THE RULE: a call reads the wall clock ONCE and uses that one instant both to apply
//! what the clock already owes the store and to stamp every due instant it writes — so a
//! stored transition is measured from exactly the moment the call was judged against,
//! never from a later reading taken further into the handler.
//!
//! The rule is OBSERVABLE because a stamped write resolves a store that can itself be
//! carrying a pending transition. An instance holds a Region whose extension workflow is
//! in flight (`RegionMetadata$Status`: `ACTIVE | ADDING | REMOVING`, from
//! `smithy_query(query="shape",shape="RegionStatus")`), due at an instant the test fixes.
//! One call then does two things with its instant: it decides whether that Region's
//! activation is already owed, and it fixes the due instant of what it stores. With one
//! instant the two agree exactly — the stored change is dated at or after the Region's due
//! instant if and only if the same call found that activation owed. A second clock reading
//! inside the handler breaks that agreement whenever the Region's due instant falls between
//! the two readings: the call resolves a Region still `ADDING` and yet stamps its own write
//! from a clock already past that Region's instant.
//!
//! The stored stamp is recovered from the due instant the write fixed, by subtracting the
//! span that due instant is measured over (`ENCRYPTION_UPDATE_DELAY` for an encryption key
//! change, `ABAC_ENABLE_SPAN` for an ABAC enablement), so the assertion is about the
//! instant the call accepted the change at.
//!
//! The rows span what the rule generalises over: the two writes of this crate that stamp a
//! stored due instant onto an instance (`UpdateInstance`'s encryption key change and
//! `CreateInstanceAccessControlAttributeConfiguration`'s ABAC enablement), both
//! `KmsKeyType` values and both `KmsKeyArn` spellings the model admits, a request that also
//! carries `Name`, Region names spanning the `RegionName` pattern, and a pending activation
//! from a millisecond to an hour away on either side of the call. The sweep then walks that
//! pending instant THROUGH the call in microsecond steps, which is where a second reading
//! would show.
//!
//! Where a test has to put the pending activation on a known side of the call it moves the
//! STORED instant rather than sleeping; the transition itself is still applied by the
//! service's own `materialize_due`, exactly as in production, and the writes all go over
//! the wire.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state::{
    ABAC_ENABLE_SPAN, ENCRYPTION_UPDATE_DELAY, KEY_TYPE_AWS_OWNED, KEY_TYPE_CUSTOMER_MANAGED,
    STATUS_ADDING,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    call, hold_region_activation, organization_engine, region_activated, router_over, with_store,
};

/// A customer managed key ARN in the hyphenated-UUID spelling the model's `KmsKeyArn`
/// pattern admits (`smithy_query(query="shape",shape="KmsKeyArn")`).
const KEY_ARN_UUID: &str =
    "arn:aws:kms:us-east-1:123456789012:key/0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d";

/// The same pattern's OTHER spelling: a multi-Region key, `mrk-` + 32 hex digits.
const KEY_ARN_MRK: &str =
    "arn:aws:kms:eu-west-1:123456789012:key/mrk-0123456789abcdef0123456789abcdef";

/// A router over an ORGANIZATION instance, that instance's ARN, and the engine behind it:
/// the instance kind that serves both of the writes below, so one fixture carries every
/// row.
fn fixture() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>, String) {
    let (engine, instance_arn) = organization_engine();
    (router_over(Arc::clone(&engine)), engine, instance_arn)
}

/// One WRITE that stamps a stored due instant onto the instance it resolves.
#[derive(Debug, Clone, Copy)]
enum StampedWrite {
    /// `UpdateInstance` replacing the encryption configuration: the key change is
    /// asynchronous, so the record carries the instant it becomes effective.
    EncryptionChange {
        key_type: &'static str,
        kms_key_arn: Option<&'static str>,
        /// `Name`, which the same request may carry independently of the key change.
        name: Option<&'static str>,
    },
    /// `CreateInstanceAccessControlAttributeConfiguration`: the enablement is
    /// asynchronous, so the record carries the instant it becomes `ENABLED`.
    AbacEnablement {
        attribute_key: &'static str,
        source: &'static str,
    },
}

impl StampedWrite {
    /// The operation this write calls.
    fn operation(&self) -> &'static str {
        match self {
            Self::EncryptionChange { .. } => "UpdateInstance",
            Self::AbacEnablement { .. } => "CreateInstanceAccessControlAttributeConfiguration",
        }
    }

    /// Its request body for `instance_arn`.
    fn request(&self, instance_arn: &str) -> Value {
        match self {
            Self::EncryptionChange {
                key_type,
                kms_key_arn,
                name,
            } => {
                let mut configuration = json!({"KeyType": key_type});
                if let Some(key_arn) = kms_key_arn {
                    configuration["KmsKeyArn"] = json!(key_arn);
                }
                let mut body =
                    json!({"InstanceArn": instance_arn, "EncryptionConfiguration": configuration});
                if let Some(name) = name {
                    body["Name"] = json!(name);
                }
                body
            }
            Self::AbacEnablement {
                attribute_key,
                source,
            } => json!({
                "InstanceArn": instance_arn,
                "InstanceAccessControlAttributeConfiguration": {
                    "AccessControlAttributes": [
                        {"Key": attribute_key, "Value": {"Source": [source]}},
                    ],
                },
            }),
        }
    }

    /// The instant the stored record says this write was ACCEPTED at: the due instant it
    /// fixed, less the span that instant is measured over.
    fn accepted_at(
        &self,
        engine: &Arc<engine::InMemoryEngine>,
        instance_arn: &str,
    ) -> Option<SystemTime> {
        with_store(engine, |store| {
            let instance = store.instance(instance_arn).expect("the instance");
            match self {
                Self::EncryptionChange { .. } => instance
                    .encryption
                    .effective_at
                    .map(|effective_at| effective_at - ENCRYPTION_UPDATE_DELAY),
                Self::AbacEnablement { .. } => instance
                    .access_control_attributes
                    .as_ref()
                    .map(|configuration| configuration.enabled_at - ABAC_ENABLE_SPAN),
            }
        })
    }
}

/// Make the write, and return the instant its stored record was accepted at together with
/// the interval the call itself ran in.
async fn perform(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    instance_arn: &str,
    write: StampedWrite,
    case: &str,
) -> (SystemTime, SystemTime, SystemTime) {
    let operation = write.operation();
    let before = SystemTime::now();
    let (status, body) = call(router, operation, &write.request(instance_arn)).await;
    let after = SystemTime::now();
    assert_eq!(
        status, 200,
        "case {case:?}: {operation} expected 200, got {status} {body}"
    );
    let accepted_at = write.accepted_at(engine, instance_arn);
    assert!(
        accepted_at.is_some(),
        "case {case:?}: {operation} must store the instant its change was accepted at, got \
         {accepted_at:?}"
    );
    (accepted_at.expect("the stored stamp"), before, after)
}

/// Start a Region's EXTENSION workflow over the wire, so the instance the write below
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

/// One row: the write, the Region whose activation is pending while it runs, and where
/// that activation's due instant sits relative to the call.
struct StampCase {
    label: &'static str,
    write: StampedWrite,
    region_name: &'static str,
    /// How far from the call the pending activation's due instant is placed.
    offset: Duration,
    /// `true` when that instant is in the PAST, so the activation is already owed.
    settled: bool,
}

/// The rows: both writes that stamp a due instant, both `KmsKeyType` values and both
/// `KmsKeyArn` spellings, a request that also carries `Name`, Region names spanning
/// `RegionName`'s `^([a-z]+-){2,3}\d$`, and the pending instant from a millisecond to an
/// hour away on either side.
const STAMP_CASES: &[StampCase] = &[
    StampCase {
        label: "customer managed key, activation owed an hour before the write",
        write: StampedWrite::EncryptionChange {
            key_type: KEY_TYPE_CUSTOMER_MANAGED,
            kms_key_arn: Some(KEY_ARN_UUID),
            name: None,
        },
        region_name: "us-west-2",
        offset: Duration::from_secs(3_600),
        settled: true,
    },
    StampCase {
        label: "multi-Region key and a new name, activation owed a millisecond before",
        write: StampedWrite::EncryptionChange {
            key_type: KEY_TYPE_CUSTOMER_MANAGED,
            kms_key_arn: Some(KEY_ARN_MRK),
            name: Some("Renamed+=,.@-"),
        },
        region_name: "eu-central-1",
        offset: Duration::from_millis(1),
        settled: true,
    },
    StampCase {
        label: "back to the AWS owned key, activation due a second after",
        write: StampedWrite::EncryptionChange {
            key_type: KEY_TYPE_AWS_OWNED,
            kms_key_arn: None,
            name: None,
        },
        region_name: "ap-southeast-2",
        offset: Duration::from_secs(1),
        settled: false,
    },
    StampCase {
        label: "customer managed key, activation due an hour after",
        write: StampedWrite::EncryptionChange {
            key_type: KEY_TYPE_CUSTOMER_MANAGED,
            kms_key_arn: Some(KEY_ARN_UUID),
            name: None,
        },
        region_name: "us-gov-west-1",
        offset: Duration::from_secs(3_600),
        settled: false,
    },
    StampCase {
        label: "ABAC enabled, activation owed a second before the write",
        write: StampedWrite::AbacEnablement {
            attribute_key: "Department",
            source: "${path:enterprise.department}",
        },
        region_name: "cn-north-1",
        offset: Duration::from_secs(1),
        settled: true,
    },
    StampCase {
        label: "ABAC enabled, activation due an hour after",
        write: StampedWrite::AbacEnablement {
            attribute_key: "CostCenter",
            source: "${path:enterprise.costCenter}",
        },
        region_name: "ap-northeast-1",
        offset: Duration::from_secs(3_600),
        settled: false,
    },
];

/// THE RULE, over the table: the instant a write stamps onto what it stores is the very
/// instant it resolved the store with, so it is at or past a pending activation's due
/// instant exactly when that same call applied the activation.
#[tokio::test]
async fn a_stamped_write_dates_what_it_stores_from_the_instant_it_resolved_the_store() {
    for case in STAMP_CASES {
        let label = case.label;
        let (router, engine, instance_arn) = fixture();
        start_region_addition(&router, &instance_arn, case.region_name, label).await;

        let due_at = if case.settled {
            SystemTime::now() - case.offset
        } else {
            SystemTime::now() + case.offset
        };
        hold_region_activation(&engine, &instance_arn, case.region_name, due_at);

        let (accepted_at, before, after) =
            perform(&router, &engine, &instance_arn, case.write, label).await;
        let activated = region_activated(&engine, &instance_arn, case.region_name);
        assert_eq!(
            activated, case.settled,
            "case {label:?}: the write's own instant decides the pending activation, so it \
             must be applied={} after a due instant {due_at:?}, got applied={activated}",
            case.settled
        );
        assert_eq!(
            accepted_at >= due_at,
            activated,
            "case {label:?}: the stored stamp {accepted_at:?} is the instant the call \
             resolved the store with, so it is at or past {due_at:?} exactly when the same \
             call applied that activation (applied={activated})"
        );
        assert!(
            accepted_at >= before && accepted_at <= after,
            "case {label:?}: the stored stamp must be the call's own instant, in [{before:?}, \
             {after:?}], got {accepted_at:?}"
        );
    }
}

/// How many positions of the pending activation's due instant the sweep below tries.
const SWEEP_STEPS: u32 = 800;

/// The step between them: a quarter of a microsecond, finer than the handler's own work,
/// so the sweep walks that instant THROUGH the call rather than around it — the window a
/// second clock reading inside the handler would open is under a microsecond wide.
const SWEEP_STEP: Duration = Duration::from_nanos(250);

/// Half the span the positions cover, so the sweep reaches the same distance either side of
/// the call's own instant.
const SWEEP_HALF_SPAN: Duration = Duration::from_nanos(250 * (SWEEP_STEPS as u64) / 2);

/// Where the sweep's first measurement puts the pending activation: far enough ahead that
/// the warm-up write cannot reach it, so the Region is still `ADDING` and the warm-up
/// measures only how far the call's own instant falls from the instant the loop places a
/// due instant from.
const SWEEP_WARM_UP_OFFSET: Duration = Duration::from_secs(3_600);

/// The same rule as an INVARIANT, swept across the call itself: wherever the pending
/// activation's due instant falls relative to the write that resolves it — before, during,
/// after — the stamp the write stores is at or past that instant exactly when the same call
/// applied the activation. Any position where the call resolves a Region still `ADDING` and
/// yet stamps from a clock past that Region's instant (or the reverse) is two instants in
/// one call.
///
/// The positions are centred on the lag the previous step measured between the instant the
/// loop places the due instant from and the instant the call stamps, so the sweep stays over
/// the call however fast the machine running it is.
#[tokio::test]
async fn no_sweep_position_dates_a_write_from_a_clock_later_than_the_state_it_resolved() {
    let (router, engine, instance_arn) = fixture();
    let region_name = "ap-northeast-1";
    let write = StampedWrite::EncryptionChange {
        key_type: KEY_TYPE_CUSTOMER_MANAGED,
        kms_key_arn: Some(KEY_ARN_UUID),
        name: None,
    };
    start_region_addition(&router, &instance_arn, region_name, "sweep").await;

    let mut centre = {
        let anchor = SystemTime::now();
        hold_region_activation(
            &engine,
            &instance_arn,
            region_name,
            anchor + SWEEP_WARM_UP_OFFSET,
        );
        let (accepted_at, _, _) =
            perform(&router, &engine, &instance_arn, write, "sweep warm-up").await;
        accepted_at
            .duration_since(anchor)
            .expect("the call's instant is not before the step that set it up")
    };

    for step in 0..SWEEP_STEPS {
        let offset = (centre + SWEEP_STEP * step).saturating_sub(SWEEP_HALF_SPAN);
        let anchor = SystemTime::now();
        let due_at = anchor + offset;
        hold_region_activation(&engine, &instance_arn, region_name, due_at);

        let case = format!("pending activation due {offset:?} after the sweep step's set-up");
        let (accepted_at, _, _) = perform(&router, &engine, &instance_arn, write, &case).await;
        let activated = region_activated(&engine, &instance_arn, region_name);
        assert_eq!(
            accepted_at >= due_at,
            activated,
            "case {case:?}: one instant decides both, so the stored stamp {accepted_at:?} is \
             at or past {due_at:?} exactly when the same call applied that activation \
             (applied={activated})"
        );
        centre = accepted_at.duration_since(anchor).unwrap_or(centre);
    }
}
