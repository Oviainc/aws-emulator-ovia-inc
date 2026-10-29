// SPDX-License-Identifier: CC0-1.0

//! One instant per call, end to end over the `awsJson1_1` wire.
//!
//! THE RULE: a call reads the clock ONCE and derives every clock-dependent value it
//! reports or stores from that one instant — so a single response reports the state owed
//! at one moment, and a single write dates what it stores from the moment it resolved the
//! state it wrote to. `DescribeInstance` is the response this is observable on, because it
//! projects two independently pending transitions of one instance: a Region's extension
//! workflow (`RegionMetadata$Status`: `ACTIVE | ADDING | REMOVING`) and an encryption key
//! change (`EncryptionConfigurationDetails$EncryptionStatus`, from
//! `smithy_query(query="shape",shape="KmsKeyStatus")` → `UPDATING | ENABLED |
//! UPDATE_FAILED`). Both are pure functions of the instant they are asked at, so when the
//! two fall due at the SAME instant, no response may report one of them as settled and the
//! other as still pending: that pair is two instants in one answer.
//!
//! The rows span what the rule generalises over — which transitions are pending, the key
//! type and `KmsKeyArn` spelling the change carries, the Region whose workflow is in
//! flight, and how far the shared due instant is from the call on either side — and the
//! sweep then walks that shared instant across the call itself in microsecond steps, where
//! a second clock read inside the handler is what a mixed pair would come from.
//!
//! Where a test has to be on the far side of a due instant it moves the STORED instant
//! rather than sleeping; the transitions themselves are still applied by the service's own
//! `materialize_due`, exactly as in production, and everything asserted goes over the wire.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::json;
use service_com_amazonaws_ssoadmin_swbexternalservice::state::{
    ENCRYPTION_UPDATE_DELAY, KEY_TYPE_AWS_OWNED, KEY_TYPE_CUSTOMER_MANAGED, KMS_KEY_STATUS_ENABLED,
    KMS_KEY_STATUS_UPDATING, STATUS_ACTIVE, STATUS_ADDING,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    call, organization_router, router_over, seeded_instance_arn, with_store,
};

/// A customer managed key ARN in the hyphenated-UUID spelling the model's `KmsKeyArn`
/// pattern admits (`smithy_query(query="shape",shape="KmsKeyArn")`).
const KEY_ARN_UUID: &str =
    "arn:aws:kms:us-east-1:123456789012:key/0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d";

/// The same pattern's OTHER spelling: a multi-Region key, `mrk-` + 32 hex digits.
const KEY_ARN_MRK: &str =
    "arn:aws:kms:eu-west-1:123456789012:key/mrk-0123456789abcdef0123456789abcdef";

/// A router over a fresh engine, and that engine.
fn router_and_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    (router_over(Arc::clone(&engine)), engine)
}

/// What one `DescribeInstance` response says about the instance's two pending transitions.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedPair {
    /// `Status` of the instance itself.
    instance_status: String,
    /// `Regions[…].Status` of the Region whose extension workflow is in flight.
    region_status: String,
    /// `EncryptionConfigurationDetails.EncryptionStatus`.
    encryption_status: String,
}

/// `DescribeInstance` for `instance_arn`, projected onto the clock-dependent members of
/// `region_name`'s row and the encryption configuration.
async fn describe_pair(
    router: &MultiServiceRouter,
    instance_arn: &str,
    region_name: &str,
) -> ReportedPair {
    let (status, body) = call(
        router,
        "DescribeInstance",
        &json!({"InstanceArn": instance_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeInstance expected 200 for {instance_arn}, got {status} {body}"
    );
    let region_status = body["Regions"]
        .as_array()
        .unwrap_or_else(|| panic!("no Regions in {body}"))
        .iter()
        .find(|entry| entry["RegionName"].as_str() == Some(region_name))
        .unwrap_or_else(|| panic!("no Region {region_name} in {body}"))["Status"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    ReportedPair {
        instance_status: body["Status"].as_str().unwrap_or_default().to_string(),
        region_status,
        encryption_status: body["EncryptionConfigurationDetails"]["EncryptionStatus"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    }
}

/// Put BOTH of the instance's pending transitions on the same due instant: the extension
/// workflow of `region_name` and the encryption key change fall due at `due_at`, so one
/// instant decides both and a response that reports them differently has read the clock
/// twice.
fn hold_both_transitions(
    engine: &Arc<engine::InMemoryEngine>,
    instance_arn: &str,
    region_name: &str,
    due_at: SystemTime,
) {
    with_store(engine, |store| {
        let instance = store.instance_mut(instance_arn).expect("the instance");
        instance.encryption.effective_at = Some(due_at);
        let region = instance.region_mut(region_name).expect("the added Region");
        // The Region is put back into the status its `AddRegion` left it in through the
        // state's own transition, so only the due INSTANT is the test's doing.
        region.start_addition();
        region.active_due = Some(due_at);
    });
}

/// Start both transitions over the wire: an `UpdateInstance` that replaces the encryption
/// configuration, and an `AddRegion` whose extension workflow is in flight.
async fn start_both_transitions(
    router: &MultiServiceRouter,
    instance_arn: &str,
    case: &str,
    key_type: &str,
    kms_key_arn: Option<&str>,
    region_name: &str,
) {
    let mut configuration = json!({"KeyType": key_type});
    if let Some(key_arn) = kms_key_arn {
        configuration["KmsKeyArn"] = json!(key_arn);
    }
    let (status, body) = call(
        router,
        "UpdateInstance",
        &json!({"InstanceArn": instance_arn, "EncryptionConfiguration": configuration}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateInstance expected 200, got {status} {body}"
    );

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

/// One row of the table: the key change started, the Region added, and where the shared due
/// instant sits relative to the `DescribeInstance` that reads it.
struct PairCase {
    label: &'static str,
    key_type: &'static str,
    kms_key_arn: Option<&'static str>,
    region_name: &'static str,
    /// How far from "now" the shared due instant is placed, and on which side.
    offset: Duration,
    /// `true` when the instant is in the PAST, so both transitions are already owed.
    settled: bool,
}

/// The rows: both `KmsKeyType` values the model names (`AWS_OWNED_KMS_KEY |
/// CUSTOMER_MANAGED_KEY`), both `KmsKeyArn` spellings its pattern admits, Region names
/// spanning the `RegionName` pattern `^([a-z]+-){2,3}\d$`, and the shared due instant from
/// a millisecond to an hour away on either side.
const PAIR_CASES: &[PairCase] = &[
    PairCase {
        label: "customer managed key, due an hour ago",
        key_type: KEY_TYPE_CUSTOMER_MANAGED,
        kms_key_arn: Some(KEY_ARN_UUID),
        region_name: "us-west-2",
        offset: Duration::from_secs(3_600),
        settled: true,
    },
    PairCase {
        label: "multi-Region key, due a second ago",
        key_type: KEY_TYPE_CUSTOMER_MANAGED,
        kms_key_arn: Some(KEY_ARN_MRK),
        region_name: "eu-central-1",
        offset: Duration::from_secs(1),
        settled: true,
    },
    PairCase {
        label: "back to the AWS owned key, due a millisecond ago",
        key_type: KEY_TYPE_AWS_OWNED,
        kms_key_arn: None,
        region_name: "ap-southeast-2",
        offset: Duration::from_millis(1),
        settled: true,
    },
    PairCase {
        label: "customer managed key, due in a second",
        key_type: KEY_TYPE_CUSTOMER_MANAGED,
        kms_key_arn: Some(KEY_ARN_UUID),
        region_name: "us-gov-west-1",
        offset: Duration::from_secs(1),
        settled: false,
    },
    PairCase {
        label: "multi-Region key, due in an hour",
        key_type: KEY_TYPE_CUSTOMER_MANAGED,
        kms_key_arn: Some(KEY_ARN_MRK),
        region_name: "cn-north-1",
        offset: Duration::from_secs(3_600),
        settled: false,
    },
    PairCase {
        label: "back to the AWS owned key, due in an hour",
        key_type: KEY_TYPE_AWS_OWNED,
        kms_key_arn: None,
        region_name: "us-east-2",
        offset: Duration::from_secs(3_600),
        settled: false,
    },
];

/// The two pairs a SINGLE instant can produce when both transitions fall due together:
/// either the instant is past them both, or it is before them both.
fn expected_pair(settled: bool) -> (&'static str, &'static str) {
    if settled {
        (STATUS_ACTIVE, KMS_KEY_STATUS_ENABLED)
    } else {
        (STATUS_ADDING, KMS_KEY_STATUS_UPDATING)
    }
}

/// THE RULE, over the table: with the Region's activation and the key change due at the
/// same instant, one `DescribeInstance` reports both as settled or both as pending — never
/// one of each.
#[tokio::test]
async fn one_response_reports_both_pending_transitions_at_one_instant() {
    for case in PAIR_CASES {
        let label = case.label;
        let (router, engine) = router_and_engine();
        let instance_arn = seeded_instance_arn(&router).await;
        start_both_transitions(
            &router,
            &instance_arn,
            label,
            case.key_type,
            case.kms_key_arn,
            case.region_name,
        )
        .await;

        let due_at = if case.settled {
            SystemTime::now() - case.offset
        } else {
            SystemTime::now() + case.offset
        };
        hold_both_transitions(&engine, &instance_arn, case.region_name, due_at);

        let (expected_region, expected_encryption) = expected_pair(case.settled);
        let reported = describe_pair(&router, &instance_arn, case.region_name).await;
        assert_eq!(
            reported,
            ReportedPair {
                instance_status: STATUS_ACTIVE.to_string(),
                region_status: expected_region.to_string(),
                encryption_status: expected_encryption.to_string(),
            },
            "case {label:?}: both transitions fall due at one instant, so the response must \
             report Region {expected_region} with encryption {expected_encryption}, got \
             {reported:?}"
        );
    }
}

/// How many positions of the shared due instant the sweep below tries.
const SWEEP_STEPS: u32 = 600;

/// The step between them: one microsecond, finer than the time one call takes, so the
/// sweep walks the shared instant THROUGH the call rather than around it.
const SWEEP_STEP: Duration = Duration::from_micros(1);

/// The same rule as an INVARIANT, swept across the call itself: wherever the shared due
/// instant falls relative to the `DescribeInstance` that reads it — before, during, after —
/// the response is one of the two pairs a single instant can produce. A pair reporting the
/// Region still `ADDING` beside encryption already `ENABLED` (or the reverse) is two
/// instants in one answer.
#[tokio::test]
async fn no_sweep_position_mixes_two_instants_in_one_response() {
    let (router, engine) = router_and_engine();
    let instance_arn = seeded_instance_arn(&router).await;
    let region_name = "ap-northeast-1";
    start_both_transitions(
        &router,
        &instance_arn,
        "sweep",
        KEY_TYPE_CUSTOMER_MANAGED,
        Some(KEY_ARN_UUID),
        region_name,
    )
    .await;

    let settled = expected_pair(true);
    let pending = expected_pair(false);
    for step in 0..SWEEP_STEPS {
        let offset = SWEEP_STEP * step;
        let due_at = SystemTime::now() + offset;
        hold_both_transitions(&engine, &instance_arn, region_name, due_at);

        let reported = describe_pair(&router, &instance_arn, region_name).await;
        let pair = (
            reported.region_status.as_str(),
            reported.encryption_status.as_str(),
        );
        assert!(
            pair == settled || pair == pending,
            "case: shared due instant placed {offset:?} ahead of the row's set-up, so it \
             falls before, inside or after the call that reads it — one instant decides both \
             transitions, so the response must report {settled:?} or {pending:?}, got {pair:?}"
        );
    }
}

/// An instant as the epoch seconds a timestamp member carries on the wire.
fn epoch_seconds(instant: SystemTime) -> f64 {
    instant
        .duration_since(UNIX_EPOCH)
        .expect("the test clock is after the epoch")
        .as_secs_f64()
}

/// The slack the bounds below allow: a wire timestamp is epoch seconds as a JSON number, so
/// a value at the very edge of the interval can round onto the far side of it by well under
/// a millisecond.
const ROUNDING_SLACK: f64 = 0.001;

/// The WRITE side of the same rule: what a call stores is dated from the call's own
/// instant, so it falls inside the interval the call itself ran in — a `CreatePermissionSet`
/// `CreatedDate` in the interval, and the instant an `UpdateInstance` makes its key change
/// due exactly one span after it.
#[tokio::test]
async fn a_write_dates_what_it_stores_from_its_own_instant() {
    let (router, instance_arn) = organization_router();

    // Several creates, so the rule is asserted on more than the first record a store holds.
    for name in ["First", "Second", "a", "Named+=,.@-_"] {
        let before = epoch_seconds(SystemTime::now());
        let (status, body) = call(
            &router,
            "CreatePermissionSet",
            &json!({"InstanceArn": instance_arn, "Name": name}),
        )
        .await;
        let after = epoch_seconds(SystemTime::now());
        assert_eq!(
            status, 200,
            "case {name:?}: CreatePermissionSet expected 200, got {status} {body}"
        );
        let created = body["PermissionSet"]["CreatedDate"].as_f64();
        assert!(
            created.is_some_and(|created| {
                created >= before - ROUNDING_SLACK && created <= after + ROUNDING_SLACK
            }),
            "case {name:?}: CreatedDate must be the call's own instant, in [{before}, \
             {after}], got {:?}",
            body["PermissionSet"]["CreatedDate"]
        );
    }

    let (router, engine) = router_and_engine();
    let instance_arn = seeded_instance_arn(&router).await;
    // (case, the KeyType the update carries, its KmsKeyArn)
    let updates: [(&str, &str, Option<&str>); 3] = [
        ("into a customer managed key", KEY_TYPE_CUSTOMER_MANAGED, Some(KEY_ARN_UUID)),
        ("into a multi-Region key", KEY_TYPE_CUSTOMER_MANAGED, Some(KEY_ARN_MRK)),
        ("back to the AWS owned key", KEY_TYPE_AWS_OWNED, None),
    ];
    for (case, key_type, kms_key_arn) in updates {
        let mut configuration = json!({"KeyType": key_type});
        if let Some(key_arn) = kms_key_arn {
            configuration["KmsKeyArn"] = json!(key_arn);
        }
        let before = SystemTime::now();
        let (status, body) = call(
            &router,
            "UpdateInstance",
            &json!({"InstanceArn": instance_arn, "EncryptionConfiguration": configuration}),
        )
        .await;
        let after = SystemTime::now();
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateInstance expected 200, got {status} {body}"
        );
        let effective_at = with_store(&engine, |store| {
            store
                .instance(&instance_arn)
                .expect("the instance")
                .encryption
                .effective_at
        });
        assert!(
            effective_at.is_some_and(|effective_at| {
                effective_at >= before + ENCRYPTION_UPDATE_DELAY
                    && effective_at <= after + ENCRYPTION_UPDATE_DELAY
            }),
            "case {case:?}: the key change falls due one span after the update's own \
             instant, in [{:?}, {:?}], got {effective_at:?}",
            before + ENCRYPTION_UPDATE_DELAY,
            after + ENCRYPTION_UPDATE_DELAY
        );
    }

    // The same instance, read back over the wire: a key change whose due instant is still
    // ahead of the read is reported as in flight, which is what the stored instant above
    // means to a caller. The instant is moved rather than raced against the span, so the
    // read is on a known side of it.
    with_store(&engine, |store| {
        store
            .instance_mut(&instance_arn)
            .expect("the instance")
            .encryption
            .effective_at = Some(SystemTime::now() + Duration::from_secs(3_600));
    });
    let (status, body) = call(
        &router,
        "DescribeInstance",
        &json!({"InstanceArn": instance_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeInstance expected 200, got {status} {body}"
    );
    assert_eq!(
        body["EncryptionConfigurationDetails"]["EncryptionStatus"].as_str(),
        Some(KMS_KEY_STATUS_UPDATING),
        "a key change read inside its own span is still UPDATING, got {body}"
    );
    assert_eq!(
        body["EncryptionConfigurationDetails"]["KeyType"].as_str(),
        Some(KEY_TYPE_AWS_OWNED),
        "the last update opted back into the AWS owned key, got {body}"
    );
}
