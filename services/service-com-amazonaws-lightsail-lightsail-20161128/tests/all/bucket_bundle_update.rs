// SPDX-License-Identifier: CC0-1.0

//! `UpdateBucketBundle`: moving a stored Lightsail bucket onto another of the
//! published object-storage plans, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The harness is the one `instance_lifecycle.rs` already has.
//!
//! The rule under test: applying a published, still-active plan to the ONE stored
//! bucket the request addresses in the Region it is made in replaces that
//! bucket's `bundleId` and spends the single bundle change its monthly billing
//! cycle allows (the `ableToUpdateBundle` a later read reports), leaves every
//! other member of the record alone, and answers one terminal operation-log entry
//! for it — while a request the rules do not allow is refused and changes nothing.
//!
//! So the cases are TABLES: one row per direction a plan can move in (up, down,
//! and onto the plan already applied), covering every active plan the published
//! catalog holds as a target; one row per rule a refused request breaks; one row
//! per boundary a billing cycle can be crossed at, since the allowance belongs
//! to the cycle the change was made in; and, for the `isActive` dimension the
//! published catalog cannot show (Lightsail has retired no object-storage plan),
//! rows over a catalog that holds a retired one.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::state::{
    applicable_bucket_bundle, applicable_bucket_bundle_in, billing_cycle, bundle_change_available,
    now_ms, with_state, BucketBundleSpec, BUCKET_BUNDLE_SPECS,
};

use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The endpoint these cases reach the emulator on (its `Host`), which is the
/// address a bucket's `url` is announced under — fixed across a case so a read
/// before and a read after announce the same address.
const ENDPOINT_HOST: &str = "127.0.0.1:9297";

/// The Region every bucket here is created and addressed in.
const REGION: &str = "us-east-1";

/// A Region that holds none of these buckets, for the case that asserts a bucket
/// is addressed in the Region the request is made in.
const OTHER_REGION: &str = "us-west-2";

/// The `operationType` the log entry of a bucket bundle update carries, from the
/// modelled `OperationType` enum.
const OPERATION_TYPE: &str = "UpdateBucketBundle";

/// The `ResourceType` a bucket reports, from the modelled enum.
const RESOURCE_TYPE_BUCKET: &str = "Bucket";

/// The terminal `OperationStatus` a finished log entry reports, from the modelled
/// enum.
const SUCCEEDED: &str = "Succeeded";

/// The value Lightsail puts in a not-found body's `code` member, which a
/// Lightsail SDK client branches on rather than on the HTTP status.
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// The published object-storage plans, cheapest first, as ids of the service's
/// catalog (`GetBucketBundles`).
const SMALL: &str = "small_1_0";
const MEDIUM: &str = "medium_1_0";
const LARGE: &str = "large_1_0";

// ---------------------------------------------------------------------------
// The fixture
// ---------------------------------------------------------------------------

/// Create the bucket `name` on the plan `bundle_id` in [`REGION`].
async fn create(emulator: &Emulator, name: &str, bundle_id: &str) {
    emulator
        .ok_at(
            Some(ENDPOINT_HOST),
            Some(REGION),
            "CreateBucket",
            json!({"bucketName": name, "bundleId": bundle_id}),
        )
        .await;
}

/// The bucket `name` as `GetBuckets` reports it in [`REGION`] — the implemented
/// reader a bundle update is read back through.
async fn read(emulator: &Emulator, name: &str) -> Value {
    emulator
        .ok_at(
            Some(ENDPOINT_HOST),
            Some(REGION),
            "GetBuckets",
            json!({"bucketName": name}),
        )
        .await["buckets"][0]
        .clone()
}

/// Call `UpdateBucketBundle` with the members `body` carries, signed for
/// `region`, answering the HTTP status and the parsed body.
async fn update(emulator: &Emulator, region: &str, body: Value) -> (u16, Value) {
    emulator
        .call_at(
            Some(ENDPOINT_HOST),
            Some(region),
            "UpdateBucketBundle",
            body,
        )
        .await
}

/// The members of a bucket the rule says a bundle update replaces: the plan
/// applied, and the allowance it spends.
fn expected_after(bundle_id: &str) -> BTreeMap<&'static str, Value> {
    BTreeMap::from([
        ("bundleId", json!(bundle_id)),
        ("ableToUpdateBundle", json!(false)),
    ])
}

/// Assert that `after` reports `expected` and, in every other member, exactly
/// what `before` reported.
fn assert_only_expected_changed(
    case: &str,
    before: &Value,
    after: &Value,
    expected: &BTreeMap<&'static str, Value>,
) {
    let mut members: BTreeSet<String> = before
        .as_object()
        .unwrap_or_else(|| panic!("case {case}: expected a bucket, got {before:?}"))
        .keys()
        .cloned()
        .collect();
    // The union of both reads' members, so a member the update NEWLY writes is
    // checked too rather than slipping past because the earlier read had none.
    members.extend(
        after
            .as_object()
            .unwrap_or_else(|| panic!("case {case}: expected a bucket, got {after:?}"))
            .keys()
            .cloned(),
    );
    members.extend(expected.keys().map(|member| (*member).to_string()));
    for member in members {
        let want = expected
            .get(member.as_str())
            .cloned()
            .unwrap_or_else(|| before[&member].clone());
        let got = after[&member].clone();
        assert_eq!(
            got, want,
            "case {case}: expected {member} to be {want:?} after the bundle update, got {got:?}"
        );
    }
}

/// Assert that `operations` is the single terminal log entry a bundle update of
/// `bucket_name` appends, and answer that entry.
fn assert_logged_entry(case: &str, operations: &Value, bucket_name: &str) -> Value {
    let entries = operations
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an operations array, got {operations:?}"));
    assert_eq!(
        entries.len(),
        1,
        "case {case}: expected one entry for the one bucket moved, got {operations:?}"
    );
    let operation = entries[0].clone();
    assert_eq!(
        operation["operationType"].as_str(),
        Some(OPERATION_TYPE),
        "case {case}: expected the {OPERATION_TYPE} operation type, got {operation:?}"
    );
    assert_eq!(
        operation["resourceType"].as_str(),
        Some(RESOURCE_TYPE_BUCKET),
        "case {case}: expected the entry to be about the bucket, got {operation:?}"
    );
    assert_eq!(
        operation["resourceName"].as_str(),
        Some(bucket_name),
        "case {case}: expected the entry to name {bucket_name}, got {operation:?}"
    );
    // The new plan is in force as the call returns, so the entry is finished.
    assert_eq!(
        operation["status"].as_str(),
        Some(SUCCEEDED),
        "case {case}: expected a {SUCCEEDED} operation, got {operation:?}"
    );
    assert_eq!(
        operation["isTerminal"].as_bool(),
        Some(true),
        "case {case}: expected a terminal operation, got {operation:?}"
    );
    assert_eq!(
        operation["location"]["regionName"].as_str(),
        Some(REGION),
        "case {case}: expected the entry located where the bucket is, got {operation:?}"
    );
    let id = operation["id"].as_str().unwrap_or_default();
    assert!(
        is_uuid_shaped(id),
        "case {case}: expected a uuid-shaped operation id, got {id:?}"
    );
    operation
}

// ---------------------------------------------------------------------------
// The update table
// ---------------------------------------------------------------------------

/// One bundle update: the plan the bucket is created on, and the plan the request
/// moves it to.
///
/// The rows differ along what the rule generalises over: the direction the plan
/// moves in (to the next plan up, straight to the dearest, back down), how far it
/// moves, and whether it moves at all (the plan already applied, named again).
struct Row {
    case: &'static str,
    from: &'static str,
    to: &'static str,
}

/// The table. Each row gets its own bucket, so each exercises a bucket whose one
/// monthly bundle change is still available.
const ROWS: &[Row] = &[
    Row {
        case: "the cheapest plan moves up to the next one",
        from: SMALL,
        to: MEDIUM,
    },
    Row {
        case: "the cheapest plan moves straight to the dearest",
        from: SMALL,
        to: LARGE,
    },
    Row {
        case: "the dearest plan moves back down to the cheapest",
        from: LARGE,
        to: SMALL,
    },
    Row {
        case: "the plan already applied is applied again",
        from: MEDIUM,
        to: MEDIUM,
    },
    Row {
        case: "the middle plan moves up to the dearest",
        from: MEDIUM,
        to: LARGE,
    },
];

/// RULE: applying a published, active plan to a bucket replaces its `bundleId`,
/// spends the one bundle change its monthly billing cycle allows, leaves every
/// other member of the record alone, and answers one terminal operation-log entry
/// for that bucket.
#[tokio::test]
async fn applying_a_published_plan_moves_the_bucket_and_spends_its_monthly_change() {
    // The table covers every plan a bucket may currently be moved onto, so it
    // cannot quietly stop spanning the catalog as that catalog grows.
    let targets: BTreeSet<&str> = ROWS.iter().map(|row| row.to).collect();
    let active: BTreeSet<&str> = BUCKET_BUNDLE_SPECS
        .iter()
        .filter(|spec| spec.is_active)
        .map(|spec| spec.bundle_id)
        .collect();
    assert_eq!(
        targets, active,
        "case the published catalog: expected the table to move a bucket onto every active \
         plan, got {targets:?}"
    );

    let emulator = Emulator::new();

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let name = format!("bundle-bucket-{index}");
        create(&emulator, &name, row.from).await;
        let before = read(&emulator, &name).await;
        assert_eq!(
            before["bundleId"].as_str(),
            Some(row.from),
            "case {case}: expected the bucket to start on {}, got {before:?}",
            row.from
        );
        assert_eq!(
            before["ableToUpdateBundle"].as_bool(),
            Some(true),
            "case {case}: expected a bucket that has never been moved to be able to update its \
             bundle, got {before:?}"
        );

        let (status, answered) = update(
            &emulator,
            REGION,
            json!({"bucketName": name, "bundleId": row.to}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} {answered:?}"
        );
        let operation = assert_logged_entry(case, &answered["operations"], &name);

        // The record itself, read back through the implemented reader: the plan
        // the request named, the spent allowance, and nothing else changed.
        let after = read(&emulator, &name).await;
        assert_only_expected_changed(case, &before, &after, &expected_after(row.to));

        // The entry the call answered is the entry the log kept.
        let id = operation["id"].as_str().unwrap_or_default();
        let logged = emulator
            .ok("GetOperation", json!({"operationId": id}))
            .await["operation"]
            .clone();
        assert_eq!(
            logged, operation,
            "case {case}: expected the log to keep the entry the call answered, got {logged:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The one change a billing cycle allows
// ---------------------------------------------------------------------------

/// RULE: a bucket's bundle may be updated only one time within a monthly billing
/// cycle, so once that change is spent every further request is refused and
/// changes nothing — whichever plan it names.
#[tokio::test]
async fn a_second_bundle_update_within_the_billing_cycle_is_refused() {
    let emulator = Emulator::new();

    let name = "bundle-bucket-spent";
    create(&emulator, name, SMALL).await;
    let (status, answered) = update(
        &emulator,
        REGION,
        json!({"bucketName": name, "bundleId": MEDIUM}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the first update: expected HTTP 200, got {status} {answered:?}"
    );
    let spent = read(&emulator, name).await;
    assert_eq!(
        spent["ableToUpdateBundle"].as_bool(),
        Some(false),
        "case the first update: expected the monthly change to be spent, got {spent:?}"
    );

    // A dearer plan, a cheaper one, and the plan now applied: the allowance is
    // about the change itself, not about which plan the request names.
    for bundle_id in [LARGE, SMALL, MEDIUM] {
        let case = format!("a second update to {bundle_id}");
        let (status, body) = update(
            &emulator,
            REGION,
            json!({"bucketName": name, "bundleId": bundle_id}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body:?}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "InvalidInputException",
            "case {case}: expected InvalidInputException, got {code} {body:?}"
        );
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| message.contains(name)
                    && message.contains("billing cycle")),
            "case {case}: expected a message naming the bucket and the one change its billing \
             cycle allows, got {body:?}"
        );
        let after = read(&emulator, name).await;
        assert_eq!(
            after, spent,
            "case {case}: expected the refused update to change nothing, got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The billing cycle the one change belongs to
// ---------------------------------------------------------------------------

/// Milliseconds in one day. Used to put a stored change far enough back to be in
/// an earlier calendar month than the one the case runs in, whichever day of the
/// month that is.
const DAY_MS: i64 = 24 * 60 * 60 * 1_000;

/// The instant `utc` names, in epoch milliseconds. The calendar conversion is the
/// library's, so these cases re-derive no month arithmetic of their own.
fn instant(utc: &str) -> i64 {
    let parsed = ::aws_smithy_types::DateTime::from_str(
        utc,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|error| panic!("case {utc}: expected a parsable instant, got {error:?}"));
    parsed
        .to_millis()
        .unwrap_or_else(|error| panic!("case {utc}: expected epoch milliseconds, got {error:?}"))
}

/// RULE: the one bundle change a bucket is allowed belongs to the monthly billing
/// cycle it was made in — a calendar month, which is what an account is billed by
/// — so it is spent for the rest of that cycle and available again in the next
/// one.
#[test]
fn the_one_bundle_change_belongs_to_the_billing_cycle_it_was_made_in() {
    // (the case, when the bucket's bundle was last changed, when the request is
    // made, whether a change is available then). The rows differ along how far
    // apart those two instants are and which boundary lies between them: none at
    // all, a day, a whole month, a leap February's end, and a year.
    let rows: &[(&str, Option<&str>, &str, bool)] = &[
        (
            "a bucket that has never been moved",
            None,
            "2026-10-24T01:05:05Z",
            true,
        ),
        (
            "the very instant of the change",
            Some("2026-10-24T01:05:05Z"),
            "2026-10-24T01:05:05Z",
            false,
        ),
        (
            "later the same day",
            Some("2026-10-24T01:05:05Z"),
            "2026-10-24T23:59:59Z",
            false,
        ),
        (
            "the first and the last instant of one month",
            Some("2026-10-01T00:00:00Z"),
            "2026-10-31T23:59:59Z",
            false,
        ),
        (
            "one second later, over a month boundary",
            Some("2026-10-31T23:59:59Z"),
            "2026-11-01T00:00:00Z",
            true,
        ),
        (
            "the month after the change",
            Some("2026-10-24T01:05:05Z"),
            "2026-11-24T01:05:05Z",
            true,
        ),
        (
            "one second later, over a year boundary",
            Some("2026-12-31T23:59:59Z"),
            "2027-01-01T00:00:00Z",
            true,
        ),
        (
            "the last day of a leap February, within that month",
            Some("2024-02-01T00:00:00Z"),
            "2024-02-29T23:59:59Z",
            false,
        ),
        (
            "the day after a leap February",
            Some("2024-02-29T23:59:59Z"),
            "2024-03-01T00:00:00Z",
            true,
        ),
        (
            "the same month of the following year",
            Some("2026-10-24T01:05:05Z"),
            "2027-10-24T01:05:05Z",
            true,
        ),
    ];

    for (case, changed, at, expected) in rows {
        let changed_ms = changed.map(instant);
        let at_ms = instant(at);
        let available = bundle_change_available(changed_ms, at_ms);
        assert_eq!(
            available,
            *expected,
            "case {case}: expected a change in the {} cycle to be available={expected} to a \
             bucket last moved in {:?}, got {available}",
            billing_cycle(at_ms),
            changed_ms.map(billing_cycle)
        );
    }
}

/// RULE, over the wire: the allowance is the CYCLE's, so a bucket whose bundle was
/// last changed in an earlier billing cycle can be moved again, and that move
/// spends the cycle it is made in.
///
/// Nothing moves this emulator's clock a month on, so the earlier cycle is reached
/// the way every other clock-bound case here reaches a state no request can put a
/// resource in: through the store the handlers read.
#[tokio::test]
async fn a_bucket_moved_in_an_earlier_billing_cycle_can_be_moved_again() {
    let emulator = Emulator::new();

    let name = "bundle-bucket-next-cycle";
    create(&emulator, name, SMALL).await;
    let (status, answered) = update(
        &emulator,
        REGION,
        json!({"bucketName": name, "bundleId": MEDIUM}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the change of this cycle: expected HTTP 200, got {status} {answered:?}"
    );

    // Backdate the change out of the cycle this case runs in. 60 days is more
    // than any month is long, so the instant is in an earlier calendar month
    // whatever day the case runs on — asserted rather than assumed.
    let at = now_ms();
    let earlier = at - 60 * DAY_MS;
    assert_ne!(
        billing_cycle(earlier),
        billing_cycle(at),
        "case the earlier cycle: expected {earlier} to be in a cycle before {at}"
    );
    with_state(emulator.engine(), |store| {
        let bucket = store
            .bucket_mut(REGION, name)
            .expect("this case created the bucket through CreateBucket");
        bucket.bundle_updated_at_ms = Some(earlier);
    })
    .expect("this case's engine is the shared in-memory one");

    // A read reports the allowance of the cycle the READ is made in, with nothing
    // having had to be materialised in between.
    let ready = read(&emulator, name).await;
    assert_eq!(
        ready["ableToUpdateBundle"].as_bool(),
        Some(true),
        "case the next cycle: expected the allowance of this cycle to be available, got {ready:?}"
    );
    assert_eq!(
        ready["bundleId"].as_str(),
        Some(MEDIUM),
        "case the next cycle: expected the plan of the earlier change to be in force, got \
         {ready:?}"
    );

    let (status, moved) = update(
        &emulator,
        REGION,
        json!({"bucketName": name, "bundleId": LARGE}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the next cycle: expected HTTP 200, got {status} {moved:?}"
    );
    assert_logged_entry("the next cycle", &moved["operations"], name);
    let after = read(&emulator, name).await;
    assert_only_expected_changed("the next cycle", &ready, &after, &expected_after(LARGE));

    // And that move spent THIS cycle's change: another one is refused.
    let (status, refused) = update(
        &emulator,
        REGION,
        json!({"bucketName": name, "bundleId": SMALL}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case a second change of the new cycle: expected HTTP 400, got {status} {refused:?}"
    );
    let code = error_code(&refused);
    assert_eq!(
        code, "InvalidInputException",
        "case a second change of the new cycle: expected InvalidInputException, got {code} \
         {refused:?}"
    );
    let unchanged = read(&emulator, name).await;
    assert_eq!(
        unchanged, after,
        "case a second change of the new cycle: expected the refused update to change nothing, \
         got {unchanged:?}"
    );
}

// ---------------------------------------------------------------------------
// The refusal table
// ---------------------------------------------------------------------------

/// One `UpdateBucketBundle` request the rules do not allow, and how the service
/// refuses it.
struct Refusal {
    case: &'static str,
    /// The whole request body, given the name of the stored bucket.
    body: fn(&str) -> Value,
    /// The Region the request is signed for.
    region: &'static str,
    /// The wire error code (`__type`) the refusal carries.
    error: &'static str,
    /// Whether the body carries Lightsail's `DoesNotExist` code.
    does_not_exist: bool,
    /// A fragment the message has to name, so the caller can fix the request from
    /// the text alone.
    message_names: &'static str,
}

/// The table. The unpublished plan ids span the shapes an id can be wrong in: a
/// size the catalog has no plan for, a generation it does not offer, a plan of
/// another resource's catalog, and an id that differs only in case.
const REFUSALS: &[Refusal] = &[
    Refusal {
        case: "a bucket this account does not hold",
        body: |_| json!({"bucketName": "no-such-bucket", "bundleId": MEDIUM}),
        region: REGION,
        error: "NotFoundException",
        does_not_exist: true,
        message_names: "no-such-bucket",
    },
    Refusal {
        case: "a bucket of this account addressed in another Region",
        body: |name| json!({"bucketName": name, "bundleId": MEDIUM}),
        region: OTHER_REGION,
        error: "NotFoundException",
        does_not_exist: true,
        message_names: "bundle-bucket-refused",
    },
    Refusal {
        case: "a plan size this service publishes no plan for",
        body: |name| json!({"bucketName": name, "bundleId": "tiny_1_0"}),
        region: REGION,
        error: "InvalidInputException",
        does_not_exist: false,
        message_names: "tiny_1_0",
    },
    Refusal {
        case: "a generation of a published size this service does not offer",
        body: |name| json!({"bucketName": name, "bundleId": "small_9_9"}),
        region: REGION,
        error: "InvalidInputException",
        does_not_exist: false,
        message_names: "small_9_9",
    },
    Refusal {
        case: "a bundle id of another resource's catalog",
        body: |name| json!({"bucketName": name, "bundleId": "nano_3_0"}),
        region: REGION,
        error: "InvalidInputException",
        does_not_exist: false,
        message_names: "nano_3_0",
    },
    Refusal {
        case: "a published plan id in the wrong case",
        body: |name| json!({"bucketName": name, "bundleId": "SMALL_1_0"}),
        region: REGION,
        error: "InvalidInputException",
        does_not_exist: false,
        message_names: "SMALL_1_0",
    },
];

/// RULE: a request the service's own rules do not allow is refused with the
/// modelled failure for that rule and changes nothing — the bucket still reports
/// the plan it was on and the change its billing cycle still allows.
#[tokio::test]
async fn a_bundle_update_the_rules_do_not_allow_changes_nothing() {
    let emulator = Emulator::new();

    let name = "bundle-bucket-refused";
    create(&emulator, name, SMALL).await;

    for refusal in REFUSALS {
        let case = refusal.case;
        let before = read(&emulator, name).await;
        let (status, body) = update(&emulator, refusal.region, (refusal.body)(name)).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body:?}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, refusal.error,
            "case {case}: expected {}, got {code} {body:?}",
            refusal.error
        );
        if refusal.does_not_exist {
            assert_eq!(
                body["code"].as_str(),
                Some(DOES_NOT_EXIST),
                "case {case}: expected the {DOES_NOT_EXIST} code, got {body:?}"
            );
        }
        assert!(
            body["message"]
                .as_str()
                .is_some_and(|message| message.contains(refusal.message_names)),
            "case {case}: expected a message naming {}, got {body:?}",
            refusal.message_names
        );
        assert!(
            body["operations"].is_null(),
            "case {case}: a refusal logs no operation, got {body:?}"
        );

        let after = read(&emulator, name).await;
        assert_eq!(
            after, before,
            "case {case}: expected the refused update to change nothing, got {after:?}"
        );
    }

    // Nothing above spent the bucket's monthly change: a plan it may be moved
    // onto still moves it.
    let (status, moved) = update(
        &emulator,
        REGION,
        json!({"bucketName": name, "bundleId": LARGE}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case after the refusals: expected HTTP 200, got {status} {moved:?}"
    );
    assert_logged_entry("after the refusals", &moved["operations"], name);
    let after = read(&emulator, name).await;
    assert_eq!(
        after["bundleId"].as_str(),
        Some(LARGE),
        "case after the refusals: expected the bucket to move onto {LARGE}, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// The modelled constraints the boundary refuses
// ---------------------------------------------------------------------------

/// RULE: both members are modelled `@required`, and `bundleId` carries the
/// modelled `\S` pattern, so a request that leaves either out — or names an empty
/// plan — is refused by the modelled-constraint walk before any bucket is read.
#[tokio::test]
async fn a_request_outside_the_modelled_constraints_is_refused_at_the_boundary() {
    let emulator = Emulator::new();

    let name = "bundle-bucket-modelled";
    create(&emulator, name, SMALL).await;
    let before = read(&emulator, name).await;

    // (the case, the request body, the member the refusal is about)
    let rows: &[(&str, Value, &str)] = &[
        ("no bundleId at all", json!({"bucketName": name}), "bundleId"),
        (
            "an empty bundleId",
            json!({"bucketName": name, "bundleId": ""}),
            "bundleId",
        ),
        (
            "a blank bundleId",
            json!({"bucketName": name, "bundleId": " "}),
            "bundleId",
        ),
        (
            "no bucketName at all",
            json!({"bundleId": MEDIUM}),
            "bucketName",
        ),
        (
            "a bucketName shorter than the modelled minimum",
            json!({"bucketName": "ab", "bundleId": MEDIUM}),
            "bucketName",
        ),
        (
            "a bucketName outside the modelled pattern",
            json!({"bucketName": "Not-A-Bucket", "bundleId": MEDIUM}),
            "bucketName",
        ),
    ];

    for (case, body, member) in rows {
        let (status, answered) = update(&emulator, REGION, body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {answered:?}"
        );
        // The boundary's own refusal, which is a different envelope from the
        // handler's modelled `InvalidInputException`: the constraint walk answers
        // before any bucket is read.
        let code = error_code(&answered);
        assert_eq!(
            code, "ValidationException",
            "case {case}: expected the modelled-constraint walk's ValidationException, got \
             {code} {answered:?}"
        );
        assert!(
            answered["message"]
                .as_str()
                .is_some_and(|message| message.contains(member)),
            "case {case}: expected a message naming {member}, got {answered:?}"
        );
        let after = read(&emulator, name).await;
        assert_eq!(
            after, before,
            "case {case}: expected the refused update to change nothing, got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The `isActive` dimension
// ---------------------------------------------------------------------------

/// RULE: a plan is one a bucket may be put on exactly when the catalog holds it
/// and it is still active — `BucketBundle$isActive` says a bundle that is active
/// is for "a new or existing bucket".
///
/// The published catalog carries no retired plan (Lightsail has withdrawn none),
/// so the wire cases above cannot tell "a published plan" from "an active
/// published plan". These rows put that dimension under test against a catalog
/// that holds a retired plan beside active ones.
#[test]
fn only_a_plan_the_catalog_still_offers_is_one_a_bucket_can_be_put_on() {
    /// A plan in the catalog, active or retired — only `is_active` and the id
    /// matter to this rule.
    fn spec(bundle_id: &'static str, is_active: bool) -> BucketBundleSpec {
        BucketBundleSpec {
            bundle_id,
            storage_per_month_in_gb: 5,
            transfer_per_month_in_gb: 25,
            price_usd_per_month: 1.0,
            is_active,
        }
    }

    let catalog = vec![
        spec("small_1_0", false),
        spec("small_2_0", true),
        spec("large_1_0", true),
    ];
    // (the id a request names, whether a bucket may be put on it)
    let rows: &[(&str, bool)] = &[
        ("small_2_0", true),
        ("large_1_0", true),
        ("small_1_0", false),
        ("medium_1_0", false),
        ("", false),
    ];

    for (bundle_id, expected) in rows {
        let applicable = applicable_bucket_bundle_in(&catalog, bundle_id).is_some();
        assert_eq!(
            applicable, *expected,
            "case {bundle_id:?}: expected a catalog of {catalog:?} to make this plan \
             applicable={expected}, got {applicable}"
        );
    }

    // The published catalog, through the same rule: every plan it offers is one a
    // bucket may be put on, and an id it does not hold is not.
    for spec in BUCKET_BUNDLE_SPECS {
        let applicable = applicable_bucket_bundle(spec.bundle_id);
        assert_eq!(
            applicable.is_some(),
            spec.is_active,
            "case the published {}: expected applicable={}, got {applicable:?}",
            spec.bundle_id,
            spec.is_active
        );
    }
    for unpublished in ["tiny_1_0", "small_9_9", "SMALL_1_0", "nano_3_0", ""] {
        assert!(
            applicable_bucket_bundle(unpublished).is_none(),
            "case {unpublished:?}: an id the published catalog does not hold is no plan a bucket \
             can be put on"
        );
    }
}
