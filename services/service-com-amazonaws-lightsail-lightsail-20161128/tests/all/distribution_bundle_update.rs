// SPDX-License-Identifier: CC0-1.0

//! `UpdateDistributionBundle`: moving a Lightsail content delivery network (CDN)
//! distribution onto another of the published CDN plans, driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: applying a published, still-active plan to the ONE stored
//! distribution the request names replaces that distribution's `bundleId` and
//! spends the single bundle change its monthly billing cycle allows (the
//! `ableToUpdateBundle` a later read reports), leaves every other member of the
//! record alone, and answers one terminal operation-log entry for it — while a
//! request the rules do not allow is refused and changes nothing.
//!
//! So the cases are TABLES: one row per direction a plan can move in (up, down,
//! and onto the plan already applied), covering every active plan the published
//! catalog holds as a target; one row per rule a refused request breaks; the two
//! sides of the allowance the cycle owns (spent within it, available in the next
//! one); and, for the `isActive` dimension the published catalog cannot show
//! (Lightsail has retired no CDN plan), rows over a catalog that holds a retired
//! one.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::state::{
    billing_cycle, distribution_bundle_is_applicable, distribution_bundle_is_applicable_in, now_ms,
    with_state, DistributionBundleSpec, DISTRIBUTION_BUNDLE_SPECS,
};

// The harness, the wire error code, the minted-id check and the distribution
// reads this test target already exposes: a sibling suite drives the service
// through the same front door rather than standing up a second one.
use crate::distribution_certificate_attachment::{distribution, wait_for_distribution_status};
use crate::instance_lifecycle::{error_code, is_uuid_shaped, Emulator};

/// The `operationType` the log entry of a bundle update carries, from the
/// modelled `OperationType` enum.
const OPERATION_TYPE: &str = "UpdateDistributionBundle";

/// The `ResourceType` a distribution reports, from the modelled enum.
const RESOURCE_TYPE_DISTRIBUTION: &str = "Distribution";

/// The one Region a distribution lives in and its API is served from, from the
/// modelled `LightsailDistribution$location` documentation ("all distributions
/// are located in the `us-east-1` Region").
const DISTRIBUTION_REGION: &str = "us-east-1";

/// A Region that is NOT the one the distribution APIs are served from, for the
/// case that asserts the documented restriction.
const OTHER_REGION: &str = "us-west-2";

/// The `status` a distribution settles into once its configuration is rolled out.
const DEPLOYED: &str = "Deployed";

/// The terminal `OperationStatus` a finished log entry reports, from the modelled
/// enum.
const SUCCEEDED: &str = "Succeeded";

/// The value Lightsail puts in a not-found body's `code` member, which a
/// Lightsail SDK client branches on rather than on the HTTP status.
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// The instance blueprint, bundle and zone the origin instance is created from:
/// the origin's own configuration is not what any case here is about.
const ORIGIN_BLUEPRINT: &str = "amazon_linux_2023";
const ORIGIN_INSTANCE_BUNDLE: &str = "nano_3_0";
const ORIGIN_ZONE: &str = "us-east-1a";
/// The instance every distribution in this suite pulls its content from.
const ORIGIN_INSTANCE: &str = "bundle-update-origin";

/// The published CDN plans, cheapest first, as ids of the service's catalog
/// (`GetDistributionBundles`).
const SMALL: &str = "small_1_0";
const MEDIUM: &str = "medium_1_0";
const LARGE: &str = "large_1_0";

// ---------------------------------------------------------------------------
// The fixture
// ---------------------------------------------------------------------------

/// Stand up the one origin every distribution in this suite points at.
async fn stand_up(emulator: &Emulator) {
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [ORIGIN_INSTANCE],
                "availabilityZone": ORIGIN_ZONE,
                "blueprintId": ORIGIN_BLUEPRINT,
                "bundleId": ORIGIN_INSTANCE_BUNDLE,
            }),
        )
        .await;
}

/// Create the distribution `name` on the plan `bundle_id` and let it settle, so a
/// case starts from a stable record.
async fn create_distribution(emulator: &Emulator, name: &str, bundle_id: &str) -> Value {
    emulator
        .ok(
            "CreateDistribution",
            json!({
                "distributionName": name,
                "bundleId": bundle_id,
                "origin": {"name": ORIGIN_INSTANCE, "regionName": DISTRIBUTION_REGION},
                "defaultCacheBehavior": {"behavior": "cache"},
            }),
        )
        .await;
    wait_for_distribution_status(emulator, name, DEPLOYED).await
}

/// The members of a distribution the rule says a bundle update replaces: the plan
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
        .unwrap_or_else(|| panic!("case {case}: expected a distribution, got {before:?}"))
        .keys()
        .cloned()
        .collect();
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

/// Assert that `operation` is the terminal log entry a bundle update of
/// `distribution_name` appends.
fn assert_logged_entry(case: &str, operation: &Value, distribution_name: &str) {
    assert_eq!(
        operation["operationType"].as_str(),
        Some(OPERATION_TYPE),
        "case {case}: expected the {OPERATION_TYPE} operation type, got {operation:?}"
    );
    assert_eq!(
        operation["resourceType"].as_str(),
        Some(RESOURCE_TYPE_DISTRIBUTION),
        "case {case}: expected the entry to be about the distribution, got {operation:?}"
    );
    assert_eq!(
        operation["resourceName"].as_str(),
        Some(distribution_name),
        "case {case}: expected the entry to name {distribution_name}, got {operation:?}"
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
        Some(DISTRIBUTION_REGION),
        "case {case}: expected the entry located where the distribution is, got {operation:?}"
    );
    let id = operation["id"].as_str().unwrap_or_default();
    assert!(
        is_uuid_shaped(id),
        "case {case}: expected a uuid-shaped operation id, got {id:?}"
    );
}

// ---------------------------------------------------------------------------
// The update table
// ---------------------------------------------------------------------------

/// One bundle update: the plan the distribution is created on, and the plan the
/// request moves it to.
///
/// The rows differ along what the rule generalises over: the direction the plan
/// moves in (to the next plan up, straight to the dearest, back down), how far it
/// moves, and whether it moves at all (the plan already applied, named again).
struct Row {
    case: &'static str,
    from: &'static str,
    to: &'static str,
}

/// The table. Each row gets its own distribution, so each exercises a
/// distribution whose one monthly bundle change is still available.
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

/// RULE: applying a published, active plan to a distribution replaces its
/// `bundleId`, spends the one bundle change its monthly billing cycle allows,
/// leaves every other member of the record alone, and answers one terminal
/// operation-log entry for that distribution.
#[tokio::test]
async fn applying_a_published_plan_moves_the_distribution_and_spends_its_monthly_change() {
    // The table covers every plan a distribution may currently be moved onto, so
    // it cannot quietly stop spanning the catalog as that catalog grows.
    let targets: BTreeSet<&str> = ROWS.iter().map(|row| row.to).collect();
    let active: BTreeSet<&str> = DISTRIBUTION_BUNDLE_SPECS
        .iter()
        .filter(|spec| spec.is_active)
        .map(|spec| spec.bundle_id)
        .collect();
    assert_eq!(
        targets, active,
        "case the published catalog: expected the table to move a distribution onto every active \
         plan, got {targets:?}"
    );

    let emulator = Emulator::new();
    stand_up(&emulator).await;

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let name = format!("bundle-cdn-{index}");
        let before = create_distribution(&emulator, &name, row.from).await;
        assert_eq!(
            before["bundleId"].as_str(),
            Some(row.from),
            "case {case}: expected the distribution to start on {}, got {before:?}",
            row.from
        );
        assert_eq!(
            before["ableToUpdateBundle"].as_bool(),
            Some(true),
            "case {case}: expected a distribution that has never been moved to be able to \
             update its bundle, got {before:?}"
        );

        let answered = emulator
            .ok(
                "UpdateDistributionBundle",
                json!({"distributionName": name, "bundleId": row.to}),
            )
            .await;
        let operation = answered["operation"].clone();
        assert_logged_entry(case, &operation, &name);

        // The record itself, read back through the implemented reader: the plan
        // the request named, the spent allowance, and nothing else changed.
        let after = distribution(&emulator, &name).await;
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

/// RULE: a distribution's bundle may be updated only once within its monthly
/// billing cycle, so once that change is spent every further request is refused
/// with the modelled `OperationFailureException` and changes nothing — whichever
/// plan it names.
#[tokio::test]
async fn a_second_bundle_update_within_the_billing_cycle_is_refused() {
    let emulator = Emulator::new();
    stand_up(&emulator).await;

    let name = "bundle-cdn-spent";
    create_distribution(&emulator, name, SMALL).await;
    emulator
        .ok(
            "UpdateDistributionBundle",
            json!({"distributionName": name, "bundleId": MEDIUM}),
        )
        .await;
    let spent = distribution(&emulator, name).await;
    assert_eq!(
        spent["ableToUpdateBundle"].as_bool(),
        Some(false),
        "case the first update: expected the monthly change to be spent, got {spent:?}"
    );

    // A dearer plan, a cheaper one, and the plan now applied: the allowance is
    // about the change itself, not about which plan the request names.
    for bundle_id in [LARGE, SMALL, MEDIUM] {
        let case = format!("a second update to {bundle_id}");
        let (status, body) = emulator
            .call(
                "UpdateDistributionBundle",
                json!({"distributionName": name, "bundleId": bundle_id}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} {body:?}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "OperationFailureException",
            "case {case}: expected OperationFailureException, got {code} {body:?}"
        );
        assert!(
            body["message"].as_str().is_some_and(|message| {
                message.contains(name) && message.contains("billing cycle")
            }),
            "case {case}: expected a message naming the distribution and the one change its \
             billing cycle allows, got {body:?}"
        );
        let after = distribution(&emulator, name).await;
        assert_eq!(
            after, spent,
            "case {case}: expected the refused update to change nothing, got {after:?}"
        );
    }
}

/// Milliseconds in one day. Used to put a stored change far enough back to be in
/// an earlier calendar month than the one the case runs in, whichever day of the
/// month that is.
const DAY_MS: i64 = 24 * 60 * 60 * 1_000;

/// RULE, the other side of the same allowance: it belongs to the billing cycle the
/// change was made in, so a distribution whose bundle was last changed in an
/// earlier cycle can be moved again, and that move spends the cycle it is made in.
///
/// Nothing moves this emulator's clock a month on, so the earlier cycle is reached
/// through the store the handlers read, as every other clock-bound case here does.
#[tokio::test]
async fn a_distribution_moved_in_an_earlier_billing_cycle_can_be_moved_again() {
    let emulator = Emulator::new();
    stand_up(&emulator).await;

    let name = "bundle-cdn-next-cycle";
    create_distribution(&emulator, name, SMALL).await;
    emulator
        .ok(
            "UpdateDistributionBundle",
            json!({"distributionName": name, "bundleId": MEDIUM}),
        )
        .await;

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
        let stored = store
            .distribution_mut(name)
            .expect("this case created the distribution through CreateDistribution");
        stored.bundle_updated_at_ms = Some(earlier);
    })
    .expect("this case's engine is the shared in-memory one");

    // A read reports the allowance of the cycle the READ is made in, with nothing
    // having had to be materialised in between.
    let ready = distribution(&emulator, name).await;
    assert_eq!(
        ready["ableToUpdateBundle"].as_bool(),
        Some(true),
        "case the next cycle: expected the allowance of this cycle to be available, got {ready:?}"
    );

    let (status, moved) = emulator
        .call(
            "UpdateDistributionBundle",
            json!({"distributionName": name, "bundleId": LARGE}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case the next cycle: expected HTTP 200, got {status} {moved:?}"
    );
    assert_logged_entry("the next cycle", &moved["operation"], name);
    let after = distribution(&emulator, name).await;
    assert_only_expected_changed("the next cycle", &ready, &after, &expected_after(LARGE));

    // And that move spent THIS cycle's change: another one is refused.
    let (status, refused) = emulator
        .call(
            "UpdateDistributionBundle",
            json!({"distributionName": name, "bundleId": SMALL}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case a second change of the new cycle: expected HTTP 400, got {status} {refused:?}"
    );
    let code = error_code(&refused);
    assert_eq!(
        code, "OperationFailureException",
        "case a second change of the new cycle: expected OperationFailureException, got {code} \
         {refused:?}"
    );
    let unchanged = distribution(&emulator, name).await;
    assert_eq!(
        unchanged, after,
        "case a second change of the new cycle: expected the refused update to change nothing, \
         got {unchanged:?}"
    );
}

// ---------------------------------------------------------------------------
// The refusal table
// ---------------------------------------------------------------------------

/// One `UpdateDistributionBundle` request the rules do not allow, and how the
/// service refuses it.
struct Refusal {
    case: &'static str,
    /// The whole request body.
    body: fn(&str) -> Value,
    /// The Region the request is signed for, or `None` for a request that carries
    /// no credential scope at all.
    region: Option<&'static str>,
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
/// another resource's catalog, and an empty id.
const REFUSALS: &[Refusal] = &[
    Refusal {
        case: "a distribution this account does not hold",
        body: |_| json!({"distributionName": "no-such-cdn", "bundleId": MEDIUM}),
        region: None,
        error: "NotFoundException",
        does_not_exist: true,
        message_names: "no-such-cdn",
    },
    Refusal {
        case: "a plan size this service publishes no plan for",
        body: |name| json!({"distributionName": name, "bundleId": "tiny_1_0"}),
        region: None,
        error: "InvalidInputException",
        does_not_exist: false,
        message_names: "tiny_1_0",
    },
    Refusal {
        case: "a generation of a published size this service does not offer",
        body: |name| json!({"distributionName": name, "bundleId": "small_9_9"}),
        region: None,
        error: "InvalidInputException",
        does_not_exist: false,
        message_names: "small_9_9",
    },
    Refusal {
        case: "a bundle id of another resource's catalog",
        body: |name| json!({"distributionName": name, "bundleId": "nano_3_0"}),
        region: None,
        error: "InvalidInputException",
        does_not_exist: false,
        message_names: "nano_3_0",
    },
    Refusal {
        case: "an empty bundle id",
        body: |name| json!({"distributionName": name, "bundleId": ""}),
        region: None,
        error: "InvalidInputException",
        does_not_exist: false,
        message_names: "bundleId",
    },
    Refusal {
        case: "a request that names no plan to apply",
        body: |name| json!({"distributionName": name}),
        region: None,
        error: "InvalidInputException",
        does_not_exist: false,
        message_names: "bundleId",
    },
    Refusal {
        case: "a request that names no distribution",
        body: |_| json!({"bundleId": MEDIUM}),
        region: None,
        error: "InvalidInputException",
        does_not_exist: false,
        message_names: "distributionName",
    },
    Refusal {
        case: "a request made outside the one Region the distribution APIs are served from",
        body: |name| json!({"distributionName": name, "bundleId": MEDIUM}),
        region: Some(OTHER_REGION),
        error: "InvalidInputException",
        does_not_exist: false,
        message_names: DISTRIBUTION_REGION,
    },
];

/// RULE: a request the service's own rules do not allow is refused with the
/// modelled failure for that rule and changes nothing — the distribution still
/// reports the plan it was on and the change its billing cycle still allows.
#[tokio::test]
async fn a_bundle_update_the_rules_do_not_allow_changes_nothing() {
    let emulator = Emulator::new();
    stand_up(&emulator).await;

    let name = "bundle-cdn-refused";
    create_distribution(&emulator, name, SMALL).await;

    for refusal in REFUSALS {
        let case = refusal.case;
        let before = distribution(&emulator, name).await;
        let (status, body) = emulator
            .call_in(
                refusal.region,
                "UpdateDistributionBundle",
                (refusal.body)(name),
            )
            .await;
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
            body["operation"].is_null(),
            "case {case}: a refusal logs no operation, got {body:?}"
        );

        let after = distribution(&emulator, name).await;
        assert_eq!(
            after, before,
            "case {case}: expected the refused update to change nothing, got {after:?}"
        );
    }

    // Nothing above spent the distribution's monthly change: a plan it may be
    // moved onto still moves it.
    let moved = emulator
        .ok(
            "UpdateDistributionBundle",
            json!({"distributionName": name, "bundleId": LARGE}),
        )
        .await;
    assert_logged_entry("after the refusals", &moved["operation"], name);
    let after = distribution(&emulator, name).await;
    assert_eq!(
        after["bundleId"].as_str(),
        Some(LARGE),
        "case after the refusals: expected the distribution to move onto {LARGE}, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// The `isActive` dimension
// ---------------------------------------------------------------------------

/// RULE: a plan is one a distribution may be moved onto exactly when the catalog
/// holds it and it is still active — `DistributionBundle$isActive` says a bundle
/// "can be specified for a new or existing distribution".
///
/// The published catalog carries no retired plan (Lightsail has withdrawn none),
/// so the wire cases above cannot tell "a published plan" from "an active
/// published plan". These rows put that dimension under test against a catalog
/// that holds a retired plan beside active ones.
#[test]
fn only_a_plan_the_catalog_still_offers_is_one_a_distribution_can_be_moved_onto() {
    /// A plan in the catalog, active or retired — only `is_active` and the id
    /// matter to this rule.
    fn spec(bundle_id: &'static str, is_active: bool) -> DistributionBundleSpec {
        DistributionBundleSpec {
            bundle_id,
            transfer_per_month_in_gb: 50,
            price_usd_per_month: 2.5,
            is_active,
        }
    }

    let catalog = vec![
        spec("small_1_0", false),
        spec("small_2_0", true),
        spec("large_1_0", true),
    ];
    // (the id a request names, whether a distribution may be moved onto it)
    let rows: &[(&str, bool)] = &[
        ("small_2_0", true),
        ("large_1_0", true),
        ("small_1_0", false),
        ("medium_1_0", false),
        ("", false),
    ];

    for (bundle_id, expected) in rows {
        let applicable = distribution_bundle_is_applicable_in(&catalog, bundle_id);
        assert_eq!(
            applicable, *expected,
            "case {bundle_id:?}: expected a catalog of {catalog:?} to make this plan \
             applicable={expected}, got {applicable}"
        );
    }

    // The published catalog, through the same rule: every plan it offers is one a
    // distribution may be moved onto, and an id it does not hold is not.
    for spec in DISTRIBUTION_BUNDLE_SPECS {
        assert_eq!(
            distribution_bundle_is_applicable(spec.bundle_id),
            spec.is_active,
            "case the published {}: expected applicable={}, got the other answer",
            spec.bundle_id,
            spec.is_active
        );
    }
    for unpublished in ["tiny_1_0", "small_9_9", "SMALL_1_0", "nano_3_0", ""] {
        assert!(
            !distribution_bundle_is_applicable(unpublished),
            "case {unpublished:?}: an id the published catalog does not hold is no plan a \
             distribution can be moved onto"
        );
    }
}
