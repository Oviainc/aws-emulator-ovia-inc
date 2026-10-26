// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `AssociateThirdPartyFirewall`.
//!
//! Every test drives the service the way a client does: an awsJson1_1 `POST /`
//! through [`MultiServiceRouter::dispatch`], with the `X-Amz-Target` header and
//! a SigV4 credential scope, so the whole deserialize → handle → serialize walk
//! runs and the assertions are about the WIRE response.
//!
//! The rule under test is one sentence: designating the Firewall Manager
//! administrator as a tenant administrator of the vendor the request names puts
//! THAT vendor's association into the transient `ONBOARDING` status, which the
//! clock turns into `ONBOARD_COMPLETE` once the stored due instant has passed
//! and only then makes the vendor's tenant reachable; a designation that is
//! already in flight or already complete has nothing left to start, a torn-down
//! one is designated afresh, no other vendor is touched, and a caller that is
//! not the administrator is refused before anything is written.
//!
//! The tables below are the class the rule ranges over: the vendor rows are
//! every value of the model's `ThirdPartyFirewall` enum; the prior-state rows in
//! [`prior_cases`] are every value of the model's
//! `ThirdPartyFirewallAssociationStatus` enum (plus no association at all, and
//! each transient status both before and after its own due instant); the gate
//! rows are two Regions.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_fms_awsfms_20180101::ops::types::{
    MARKETPLACE_SUBSCRIPTION_ONBOARDING_STATUS_VALUES,
    THIRD_PARTY_FIREWALL_ASSOCIATION_STATUS_VALUES, THIRD_PARTY_FIREWALL_VALUES,
};
use service_com_amazonaws_fms_awsfms_20180101::state;
use smithy_server_runtime::MultiServiceRouter;

// The wire harness and the store seam live in one place for this test target, so
// no two suites can drift in how a request is signed or the store is seeded.
use crate::administrator_gated_operations::{associate_admin, with_store};
use crate::policy_lifecycle::{call, call_in, emulator_with_engine, error_code, ACCOUNT, REGION};

/// The operation under test, spelled once.
const OPERATION: &str = "AssociateThirdPartyFirewall";

/// A second Region, to assert the designation is organisation-scoped rather than
/// per-Region, and that the gate's refusal does not depend on which Region the
/// request addresses.
const OTHER_REGION: &str = "eu-central-1";

/// Slack added to [`state::ASSOCIATION_SETTLE`] when a test waits a transient
/// status out, so a loaded machine cannot make the wait the flaky part.
const SETTLE_SLACK: Duration = Duration::from_millis(200);

/// The vendor catalog: the model's own `ThirdPartyFirewall` enum, as
/// `ops::types::THIRD_PARTY_FIREWALL_VALUES` lists it — so every vendor the
/// model declares is covered, not only the one the issue cited.
fn vendors() -> &'static [&'static str] {
    THIRD_PARTY_FIREWALL_VALUES
}

/// `GetThirdPartyFirewallAssociationStatus` for one vendor, as
/// `(http status, ThirdPartyFirewallStatus, MarketplaceOnboardingStatus)`.
async fn read_status(router: &MultiServiceRouter, vendor: &str) -> (u16, String, String) {
    let (status, body) = call(
        router,
        "GetThirdPartyFirewallAssociationStatus",
        &json!({ "ThirdPartyFirewall": vendor }),
    )
    .await;
    (
        status,
        body["ThirdPartyFirewallStatus"]
            .as_str()
            .unwrap_or("<missing>")
            .to_string(),
        body["MarketplaceOnboardingStatus"]
            .as_str()
            .unwrap_or("<missing>")
            .to_string(),
    )
}

/// The `ThirdPartyFirewallStatus` the operation answered for one vendor.
async fn associate(router: &MultiServiceRouter, vendor: &str) -> (u16, String, Value) {
    associate_in(router, vendor, REGION).await
}

/// [`associate`], signed for an explicit Region.
async fn associate_in(
    router: &MultiServiceRouter,
    vendor: &str,
    region: &str,
) -> (u16, String, Value) {
    let (status, body) = call_in(
        router,
        OPERATION,
        &json!({ "ThirdPartyFirewall": vendor }),
        region,
    )
    .await;
    let reported = body["ThirdPartyFirewallStatus"]
        .as_str()
        .unwrap_or("<missing>")
        .to_string();
    (status, reported, body)
}

/// The stored due instant of one vendor's association, or `None` when the record
/// owes the clock nothing (or does not exist).
fn stored_due(engine: &Arc<engine::InMemoryEngine>, vendor: &str) -> Option<SystemTime> {
    with_store(engine, |store| {
        store
            .third_party_firewalls
            .get(vendor)
            .and_then(|record| record.settles_at)
    })
}

/// One row of the prior-state table: the association the store holds when the
/// designation is requested, and the status the operation must answer for it.
struct PriorCase {
    /// The row's name, carried into every assertion message.
    name: &'static str,
    /// The seeded `ThirdPartyFirewallAssociationStatus`, or `None` for a vendor
    /// with no association at all.
    status: Option<&'static str>,
    /// The transition the seeded record owes the clock, and how far from now its
    /// due instant is: positive milliseconds for one still in the future,
    /// negative for one the clock has already made due.
    settles: Option<(&'static str, i64)>,
    /// The seeded `MarketplaceOnboardingStatus`: the state of the
    /// administrator's subscription to the vendor when the designation is
    /// requested.
    subscription: &'static str,
    /// The `ThirdPartyFirewallStatus` the operation must answer.
    expected: &'static str,
    /// The `MarketplaceOnboardingStatus` a reader must report after the call.
    expected_subscription: &'static str,
    /// Whether the row asserts the due instant is NOT pushed back — the
    /// idempotence of a designation that is already in flight.
    due_must_not_move: bool,
}

/// The prior-state class the rule ranges over.
///
/// The statuses are the model's own `ThirdPartyFirewallAssociationStatus` enum,
/// as `ops::types::THIRD_PARTY_FIREWALL_ASSOCIATION_STATUS_VALUES` lists it
/// (asserted below to be covered): `NOT_EXIST` is the row with no association,
/// and each transient status appears both before and after its own due instant.
/// The subscription column is the model's `MarketplaceSubscriptionOnboardingStatus`
/// and ranges over all three of its values, so the rows also cover a
/// re-designation of a vendor the administrator is still subscribed to and one
/// of a vendor it is not.
fn prior_cases() -> Vec<PriorCase> {
    vec![
        PriorCase {
            name: "no-association",
            status: None,
            settles: None,
            subscription: state::MARKETPLACE_ONBOARDING_NO_SUBSCRIPTION,
            expected: state::THIRD_PARTY_FIREWALL_ONBOARDING,
            expected_subscription: state::MARKETPLACE_ONBOARDING_NOT_COMPLETE,
            due_must_not_move: false,
        },
        PriorCase {
            name: "onboarding-in-flight",
            status: Some(state::THIRD_PARTY_FIREWALL_ONBOARDING),
            settles: Some((state::THIRD_PARTY_FIREWALL_ONBOARD_COMPLETE, 10_000)),
            subscription: state::MARKETPLACE_ONBOARDING_NOT_COMPLETE,
            expected: state::THIRD_PARTY_FIREWALL_ONBOARDING,
            expected_subscription: state::MARKETPLACE_ONBOARDING_NOT_COMPLETE,
            due_must_not_move: true,
        },
        PriorCase {
            name: "onboarding-already-due",
            status: Some(state::THIRD_PARTY_FIREWALL_ONBOARDING),
            settles: Some((state::THIRD_PARTY_FIREWALL_ONBOARD_COMPLETE, -1_000)),
            subscription: state::MARKETPLACE_ONBOARDING_NOT_COMPLETE,
            expected: state::THIRD_PARTY_FIREWALL_ONBOARD_COMPLETE,
            expected_subscription: state::MARKETPLACE_ONBOARDING_COMPLETE,
            due_must_not_move: false,
        },
        PriorCase {
            name: "onboard-complete",
            status: Some(state::THIRD_PARTY_FIREWALL_ONBOARD_COMPLETE),
            settles: None,
            subscription: state::MARKETPLACE_ONBOARDING_COMPLETE,
            expected: state::THIRD_PARTY_FIREWALL_ONBOARD_COMPLETE,
            expected_subscription: state::MARKETPLACE_ONBOARDING_COMPLETE,
            due_must_not_move: false,
        },
        PriorCase {
            name: "offboarding-in-flight",
            status: Some(state::THIRD_PARTY_FIREWALL_OFFBOARDING),
            settles: Some((state::THIRD_PARTY_FIREWALL_OFFBOARD_COMPLETE, 10_000)),
            subscription: state::MARKETPLACE_ONBOARDING_COMPLETE,
            expected: state::THIRD_PARTY_FIREWALL_ONBOARDING,
            // An active subscription is not re-made by a re-designation.
            expected_subscription: state::MARKETPLACE_ONBOARDING_COMPLETE,
            due_must_not_move: false,
        },
        PriorCase {
            name: "offboarding-already-due",
            status: Some(state::THIRD_PARTY_FIREWALL_OFFBOARDING),
            settles: Some((state::THIRD_PARTY_FIREWALL_OFFBOARD_COMPLETE, -1_000)),
            subscription: state::MARKETPLACE_ONBOARDING_COMPLETE,
            expected: state::THIRD_PARTY_FIREWALL_ONBOARDING,
            expected_subscription: state::MARKETPLACE_ONBOARDING_COMPLETE,
            due_must_not_move: false,
        },
        PriorCase {
            name: "offboard-complete",
            status: Some(state::THIRD_PARTY_FIREWALL_OFFBOARD_COMPLETE),
            settles: None,
            subscription: state::MARKETPLACE_ONBOARDING_COMPLETE,
            expected: state::THIRD_PARTY_FIREWALL_ONBOARDING,
            expected_subscription: state::MARKETPLACE_ONBOARDING_COMPLETE,
            due_must_not_move: false,
        },
        PriorCase {
            name: "offboard-complete-unsubscribed",
            status: Some(state::THIRD_PARTY_FIREWALL_OFFBOARD_COMPLETE),
            settles: None,
            subscription: state::MARKETPLACE_ONBOARDING_NO_SUBSCRIPTION,
            expected: state::THIRD_PARTY_FIREWALL_ONBOARDING,
            // Nothing to keep: the designation subscribes afresh.
            expected_subscription: state::MARKETPLACE_ONBOARDING_NOT_COMPLETE,
            due_must_not_move: false,
        },
    ]
}

/// Seed one row of [`prior_cases`] for one vendor, through the store: the
/// transient statuses past their own due instant cannot be reached by waiting
/// without making the test sleep for them.
fn seed_prior(
    store: &mut state::ComAmazonawsFmsAwsfms20180101State,
    vendor: &str,
    case: &PriorCase,
    now: SystemTime,
) {
    let Some(status) = case.status else {
        store.third_party_firewalls.remove(vendor);
        return;
    };
    let (settles_to, settles_at) = match case.settles {
        Some((to, offset_ms)) => {
            let span = Duration::from_millis(offset_ms.unsigned_abs());
            let at = if offset_ms >= 0 { now + span } else { now - span };
            (Some(to.to_string()), Some(at))
        }
        None => (None, None),
    };
    store.third_party_firewalls.insert(
        vendor.to_string(),
        state::ThirdPartyFirewallRecord {
            status: status.to_string(),
            settles_to,
            settles_at,
            marketplace_onboarding_status: case.subscription.to_string(),
        },
    );
}

/// The table's statuses cover the model's whole
/// `ThirdPartyFirewallAssociationStatus` enum (`NOT_EXIST` is the row with no
/// association, every other value is a seeded row), and its subscription column
/// covers the whole `MarketplaceSubscriptionOnboardingStatus` enum.
#[test]
fn the_prior_state_table_covers_every_modeled_association_status() {
    for value in THIRD_PARTY_FIREWALL_ASSOCIATION_STATUS_VALUES {
        let covered = if *value == state::THIRD_PARTY_FIREWALL_NOT_EXIST {
            prior_cases().iter().any(|case| case.status.is_none())
        } else {
            prior_cases()
                .iter()
                .any(|case| case.status == Some(*value))
        };
        assert!(
            covered,
            "case {value:?}: the prior-state table must cover every modeled \
             ThirdPartyFirewallAssociationStatus, and this one is missing"
        );
    }
    for value in MARKETPLACE_SUBSCRIPTION_ONBOARDING_STATUS_VALUES {
        assert!(
            prior_cases()
                .iter()
                .any(|case| case.subscription == *value),
            "case {value:?}: the prior-state table must cover every modeled \
             MarketplaceSubscriptionOnboardingStatus, and this one is missing"
        );
    }
}

/// The designation starts in `ONBOARDING`, is observable as such straight after
/// the call, and the CLOCK — not a later writer — turns it into
/// `ONBOARD_COMPLETE`, which is also what makes the vendor's tenant reachable.
/// No other vendor is touched.
#[tokio::test]
async fn designating_a_vendor_onboards_and_settles() {
    for vendor in vendors() {
        let (router, engine) = emulator_with_engine();
        associate_admin(&router).await;

        // The precondition, read through the implemented reader rather than
        // assumed.
        let before = read_status(&router, vendor).await;
        assert_eq!(
            (
                before.0,
                before.1.as_str(),
                before.2.as_str()
            ),
            (
                200,
                state::THIRD_PARTY_FIREWALL_NOT_EXIST,
                state::MARKETPLACE_ONBOARDING_NO_SUBSCRIPTION
            ),
            "case {vendor}: no association yet, got {before:?}"
        );

        let (status, reported, body) = associate(&router, vendor).await;
        assert_eq!(
            (status, reported.as_str()),
            (200, state::THIRD_PARTY_FIREWALL_ONBOARDING),
            "case {vendor}: the designation must start in ONBOARDING, got {status} with {body}"
        );

        // Read back through the implemented reader: the transient status is
        // still observable, because the due instant is in the future.
        let in_flight = read_status(&router, vendor).await;
        assert_eq!(
            (in_flight.0, in_flight.1.as_str(), in_flight.2.as_str()),
            (
                200,
                state::THIRD_PARTY_FIREWALL_ONBOARDING,
                state::MARKETPLACE_ONBOARDING_NOT_COMPLETE
            ),
            "case {vendor}: a caller reading straight after must still see ONBOARDING, got \
             {in_flight:?}"
        );
        assert!(
            stored_due(&engine, vendor).is_some(),
            "case {vendor}: the due instant must be STORED on the record, got {:?}",
            stored_due(&engine, vendor)
        );

        // The vendor's tenant is not reachable while the designation is in
        // flight: only ONBOARD_COMPLETE makes it so.
        let (status, body) = call(
            &router,
            "ListThirdPartyFirewallFirewallPolicies",
            &json!({ "ThirdPartyFirewall": vendor, "MaxResults": 10 }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {vendor}: an onboarding tenant is not reachable yet, got {status} with {body}"
        );

        // Every other vendor is untouched.
        for other in vendors().iter().filter(|other| *other != vendor) {
            let untouched = read_status(&router, other).await;
            assert_eq!(
                (untouched.0, untouched.1.as_str()),
                (200, state::THIRD_PARTY_FIREWALL_NOT_EXIST),
                "case {vendor} then {other}: only the vendor the request named is designated, \
                 got {untouched:?}"
            );
        }

        tokio::time::sleep(state::ASSOCIATION_SETTLE + SETTLE_SLACK).await;
        let settled = read_status(&router, vendor).await;
        assert_eq!(
            (settled.0, settled.1.as_str(), settled.2.as_str()),
            (
                200,
                state::THIRD_PARTY_FIREWALL_ONBOARD_COMPLETE,
                state::MARKETPLACE_ONBOARDING_COMPLETE
            ),
            "case {vendor}: the clock must settle the designation, got {settled:?}"
        );

        // Onboarded, the vendor's tenant IS reachable — and holds exactly the
        // firewall policies the store does, which is none: a third-party
        // firewall policy is created in the VENDOR's console, not here.
        let (status, body) = call(
            &router,
            "ListThirdPartyFirewallFirewallPolicies",
            &json!({ "ThirdPartyFirewall": vendor, "MaxResults": 10 }),
        )
        .await;
        assert_eq!(
            (
                status,
                body["ThirdPartyFirewallFirewallPolicies"].as_array(),
                body.get("NextToken"),
            ),
            (200, Some(&Vec::new()), None),
            "case {vendor}: an onboarded tenant must be reachable and empty, got {status} with \
             {body}"
        );
    }
}

/// The designation is organisation-scoped, not per-Region: made from one Region
/// it is the same association every Region reports.
#[tokio::test]
async fn the_designation_is_organisation_scoped() {
    let (router, _engine) = emulator_with_engine();
    associate_admin(&router).await;
    let vendor = vendors()[0];

    let (status, reported, body) = associate_in(&router, vendor, OTHER_REGION).await;
    assert_eq!(
        (status, reported.as_str()),
        (200, state::THIRD_PARTY_FIREWALL_ONBOARDING),
        "case {vendor} in {OTHER_REGION}: the designation must start in ONBOARDING, got {status} \
         with {body}"
    );

    for region in [REGION, OTHER_REGION] {
        let (status, body) = call_in(
            &router,
            "GetThirdPartyFirewallAssociationStatus",
            &json!({ "ThirdPartyFirewall": vendor }),
            region,
        )
        .await;
        assert_eq!(
            (status, body["ThirdPartyFirewallStatus"].as_str()),
            (200, Some(state::THIRD_PARTY_FIREWALL_ONBOARDING)),
            "case {vendor} in {region}: one organisation-scoped association, got {status} with \
             {body}"
        );
    }
}

/// Every prior state the association can be in, for every vendor the model
/// declares: the status the designation answers follows from the state the store
/// holds AT the call, and a designation already in flight is not restarted — its
/// due instant does not move.
#[tokio::test]
async fn designating_from_every_prior_state() {
    for vendor in vendors() {
        for case in prior_cases() {
            let (router, engine) = emulator_with_engine();
            associate_admin(&router).await;
            let now = SystemTime::now();
            with_store(&engine, |store| seed_prior(store, vendor, &case, now));
            let due_before = stored_due(&engine, vendor);

            let (status, reported, body) = associate(&router, vendor).await;
            assert_eq!(
                (status, reported.as_str()),
                (200, case.expected),
                "case {} / {vendor}: expected {}, got {status} with {body}",
                case.name,
                case.expected
            );

            // The same answer from the implemented reader: the operation's
            // answer is the state it wrote, not a value it made up.
            let read_back = read_status(&router, vendor).await;
            assert_eq!(
                (read_back.0, read_back.1.as_str(), read_back.2.as_str()),
                (200, case.expected, case.expected_subscription),
                "case {} / {vendor}: the reader must report what the designation wrote, got \
                 {read_back:?}",
                case.name
            );

            if case.due_must_not_move {
                let due_after = stored_due(&engine, vendor);
                assert_eq!(
                    due_after, due_before,
                    "case {} / {vendor}: a designation already in flight must not push its due \
                     instant back, got {due_after:?} from {due_before:?}",
                    case.name
                );
            }

            // Repeating the call is idempotent: the answer is the same, and a
            // designation that is now in flight still settles.
            let (status, repeated, body) = associate(&router, vendor).await;
            assert_eq!(
                (status, repeated.as_str()),
                (200, case.expected),
                "case {} / {vendor}: a repeated designation must answer the same, got {status} \
                 with {body}",
                case.name
            );

            tokio::time::sleep(state::ASSOCIATION_SETTLE + SETTLE_SLACK).await;
            let settled = read_status(&router, vendor).await;
            if case.due_must_not_move {
                // The STORED instant is what governs: this row's own due instant
                // is far in the future, so waiting the emulator's span out does
                // not settle it.
                assert_eq!(
                    (settled.0, settled.1.as_str(), settled.2.as_str()),
                    (
                        200,
                        state::THIRD_PARTY_FIREWALL_ONBOARDING,
                        case.expected_subscription
                    ),
                    "case {} / {vendor}: the designation settles at its STORED instant, which is \
                     still in the future, got {settled:?}",
                    case.name
                );
            } else {
                assert_eq!(
                    (settled.0, settled.1.as_str(), settled.2.as_str()),
                    (
                        200,
                        state::THIRD_PARTY_FIREWALL_ONBOARD_COMPLETE,
                        state::MARKETPLACE_ONBOARDING_COMPLETE
                    ),
                    "case {} / {vendor}: every designation settles to ONBOARD_COMPLETE, got \
                     {settled:?}",
                    case.name
                );
            }
        }
    }
}

/// A designated vendor can be torn down and designated again: the two
/// operations are mirrors, and neither leaves the other unable to run.
#[tokio::test]
async fn a_torn_down_vendor_can_be_designated_again() {
    let (router, _engine) = emulator_with_engine();
    associate_admin(&router).await;
    let vendor = vendors()[0];

    let (_, reported, _) = associate(&router, vendor).await;
    assert_eq!(
        reported, state::THIRD_PARTY_FIREWALL_ONBOARDING,
        "case {vendor}: the first designation must start in ONBOARDING, got {reported}"
    );
    tokio::time::sleep(state::ASSOCIATION_SETTLE + SETTLE_SLACK).await;

    let (status, body) = call(
        &router,
        "DisassociateThirdPartyFirewall",
        &json!({ "ThirdPartyFirewall": vendor }),
    )
    .await;
    assert_eq!(
        (status, body["ThirdPartyFirewallStatus"].as_str()),
        (200, Some(state::THIRD_PARTY_FIREWALL_OFFBOARDING)),
        "case {vendor}: the teardown must start in OFFBOARDING, got {status} with {body}"
    );
    // Tearing the association down does not cancel the vendor subscription: the
    // subscription status the designation made active stays active through the
    // teardown and past it.
    let offboarding = read_status(&router, vendor).await;
    assert_eq!(
        (offboarding.0, offboarding.1.as_str(), offboarding.2.as_str()),
        (
            200,
            state::THIRD_PARTY_FIREWALL_OFFBOARDING,
            state::MARKETPLACE_ONBOARDING_COMPLETE
        ),
        "case {vendor}: a teardown keeps the subscription it had, got {offboarding:?}"
    );
    tokio::time::sleep(state::ASSOCIATION_SETTLE + SETTLE_SLACK).await;
    let torn_down = read_status(&router, vendor).await;
    assert_eq!(
        (torn_down.0, torn_down.1.as_str(), torn_down.2.as_str()),
        (
            200,
            state::THIRD_PARTY_FIREWALL_OFFBOARD_COMPLETE,
            state::MARKETPLACE_ONBOARDING_COMPLETE
        ),
        "case {vendor}: the clock must settle the teardown and keep the subscription, got \
         {torn_down:?}"
    );

    let (status, reported, body) = associate(&router, vendor).await;
    assert_eq!(
        (status, reported.as_str()),
        (200, state::THIRD_PARTY_FIREWALL_ONBOARDING),
        "case {vendor}: a torn-down vendor must be designated afresh, got {status} with {body}"
    );
    // The subscription the first designation made active is kept across the
    // re-designation: it never regresses to "in the process of subscribing".
    let re_designated = read_status(&router, vendor).await;
    assert_eq!(
        (
            re_designated.0,
            re_designated.1.as_str(),
            re_designated.2.as_str()
        ),
        (
            200,
            state::THIRD_PARTY_FIREWALL_ONBOARDING,
            state::MARKETPLACE_ONBOARDING_COMPLETE
        ),
        "case {vendor}: a re-designation keeps the active subscription, got {re_designated:?}"
    );
    tokio::time::sleep(state::ASSOCIATION_SETTLE + SETTLE_SLACK).await;
    let settled = read_status(&router, vendor).await;
    assert_eq!(
        (settled.0, settled.1.as_str(), settled.2.as_str()),
        (
            200,
            state::THIRD_PARTY_FIREWALL_ONBOARD_COMPLETE,
            state::MARKETPLACE_ONBOARDING_COMPLETE
        ),
        "case {vendor}: the fresh designation must settle, got {settled:?}"
    );
}

/// Authorization is decided FIRST: with no Firewall Manager administrator
/// association the designation is refused with HTTP 400 `AccessDeniedException`
/// in every Region, and writes nothing — not the scaffold's 501, and not a
/// vendor-specific error.
#[tokio::test]
async fn designating_without_an_administrator_is_refused_and_writes_nothing() {
    for region in [REGION, OTHER_REGION] {
        for vendor in vendors() {
            let (router, engine) = emulator_with_engine();
            let (status, _, body) = associate_in(&router, vendor, region).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "AccessDeniedException"),
                "case {vendor} in {region}: expected AccessDeniedException, got {status} with \
                 {body}"
            );
            let message = crate::administrator_gated_operations::error_message(&body);
            assert!(
                message.contains(ACCOUNT) && message.contains(region),
                "case {vendor} in {region}: the refusal's message is built from the request, so \
                 it must name the account and Region the administrator was looked for in, got \
                 {message}"
            );
            with_store(&engine, |store| {
                assert!(
                    store.third_party_firewalls.is_empty(),
                    "case {vendor} in {region}: a refused designation must write nothing, store \
                     holds {:?}",
                    store.third_party_firewalls
                );
            });

            // With an administrator associated the same call is served, so the
            // refusal was the gate and nothing else.
            associate_admin(&router).await;
            let (status, reported, body) = associate_in(&router, vendor, region).await;
            assert_eq!(
                (status, reported.as_str()),
                (200, state::THIRD_PARTY_FIREWALL_ONBOARDING),
                "case {vendor} in {region}: with an administrator the designation is served, got \
                 {status} with {body}"
            );
        }
    }
}
