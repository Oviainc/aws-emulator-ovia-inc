// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE clock-due pending-transition rule every record
//! type in this crate that carries a `(status, settles_to, settles_at)` triple
//! shares: once the stored due instant has arrived, the status IS the one the
//! record settles to and nothing is owed any more, so both pending members are
//! cleared; while the instant is still ahead, a report changes nothing.
//!
//! Every test drives the service the way a client does — an awsJson1_1 `POST /`
//! through the router, reusing the request harness in
//! [`crate::policy_lifecycle`] — so the assertions are about the WIRE response,
//! and the store is only read (and, in the due-instant table, only its due
//! instant moved) through the same seam a handler uses.
//!
//! The table below is the class the rule ranges over: ONE row per record type
//! that carries such a triple, each reached through a different pair of
//! operations — the administrator onboarding `PutAdminAccount` starts and
//! `ListAdminAccountsForOrganization` reports, and the vendor designation
//! `AssociateThirdPartyFirewall` starts and
//! `GetThirdPartyFirewallAssociationStatus` reports, for every vendor the
//! model's `ThirdPartyFirewall` enum carries. The second table ranges the due
//! instant itself from a minute past to a minute ahead.

use std::sync::Arc;
use std::thread::sleep;
use std::time::{Duration, SystemTime};

use serde_json::json;
use service_com_amazonaws_fms_awsfms_20180101::ops::types::{
    MARKETPLACE_SUBSCRIPTION_ONBOARDING_STATUS_VALUES,
    THIRD_PARTY_FIREWALL_ASSOCIATION_STATUS_VALUES, THIRD_PARTY_FIREWALL_VALUES,
};
use service_com_amazonaws_fms_awsfms_20180101::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::admin_account_scopes::{organization_status, put_admin_account};
use crate::administrator_gated_operations::{associate_admin, with_store};
use crate::policy_lifecycle::{call, emulator_with_engine};

/// The account onboarded as a (non-default) Firewall Manager administrator
/// below. The model constrains `AWSAccountId` only to `^[0-9]+$`, and which
/// account it is decides nothing here.
const DELEGATED_ADMIN: &str = "444455556666";

/// Slack added to a record's own settling span when a test waits one out, so a
/// loaded machine cannot make the wait the flaky part.
const SETTLE_SLACK: Duration = Duration::from_millis(250);

/// One row of the record-type table: a record type that carries a pending
/// transition, identified by the resource whose transition it is.
#[derive(Clone, Copy, Debug)]
enum Record {
    /// The administrator-onboarding transition `PutAdminAccount` starts, read
    /// back through `ListAdminAccountsForOrganization`.
    OrganizationAdmin,
    /// The vendor-designation transition `AssociateThirdPartyFirewall` starts
    /// for ONE vendor, read back through
    /// `GetThirdPartyFirewallAssociationStatus`.
    ThirdPartyFirewall(&'static str),
}

/// Every record type that carries a pending transition: the administrator
/// record, and the vendor association for every vendor the model's own
/// `ThirdPartyFirewall` catalog lists (`ops::types::THIRD_PARTY_FIREWALL_VALUES`).
fn records() -> Vec<Record> {
    let mut records = vec![Record::OrganizationAdmin];
    records.extend(
        THIRD_PARTY_FIREWALL_VALUES
            .iter()
            .copied()
            .map(Record::ThirdPartyFirewall),
    );
    records
}

impl Record {
    /// The row's name, carried into every assertion message.
    fn name(self) -> String {
        match self {
            Record::OrganizationAdmin => "organization administrator onboarding".to_string(),
            Record::ThirdPartyFirewall(vendor) => format!("designation of vendor {vendor}"),
        }
    }

    /// How long this record's transition stays pending, from the service's own
    /// constant rather than a number written out here.
    fn span(self) -> Duration {
        match self {
            Record::OrganizationAdmin => state::ADMIN_ONBOARDING_SPAN,
            Record::ThirdPartyFirewall(_) => state::ASSOCIATION_SETTLE,
        }
    }

    /// The status the record holds while the transition is still pending.
    fn transient(self) -> &'static str {
        match self {
            Record::OrganizationAdmin => organization_status(state::ORGANIZATION_STATUS_ONBOARDING),
            Record::ThirdPartyFirewall(_) => {
                association_status(state::THIRD_PARTY_FIREWALL_ONBOARDING)
            }
        }
    }

    /// The status the clock owes it.
    fn terminal(self) -> &'static str {
        match self {
            Record::OrganizationAdmin => {
                organization_status(state::ORGANIZATION_STATUS_ONBOARDING_COMPLETE)
            }
            Record::ThirdPartyFirewall(_) => {
                association_status(state::THIRD_PARTY_FIREWALL_ONBOARD_COMPLETE)
            }
        }
    }

    /// Start the transition through the operation that owns it.
    async fn start(self, router: &MultiServiceRouter) {
        let case = self.name();
        match self {
            Record::OrganizationAdmin => {
                put_admin_account(router, &case, DELEGATED_ADMIN, None).await;
            }
            Record::ThirdPartyFirewall(vendor) => {
                let (status, body) = call(
                    router,
                    "AssociateThirdPartyFirewall",
                    &json!({ "ThirdPartyFirewall": vendor }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected AssociateThirdPartyFirewall to answer 200, got \
                     {status} with {body:?}"
                );
            }
        }
    }

    /// The status the reporting operation answers for this record.
    async fn reported_status(self, router: &MultiServiceRouter) -> String {
        let case = self.name();
        match self {
            Record::OrganizationAdmin => {
                let (status, body) =
                    call(router, "ListAdminAccountsForOrganization", &json!({})).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected ListAdminAccountsForOrganization to answer 200, got \
                     {status} with {body:?}"
                );
                let summaries = body["AdminAccounts"].as_array().cloned().unwrap_or_default();
                let summary = summaries
                    .iter()
                    .find(|summary| summary["AdminAccount"].as_str() == Some(DELEGATED_ADMIN))
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: the enumeration must report the onboarded \
                             administrator {DELEGATED_ADMIN}, got {summaries:?}"
                        )
                    });
                let reported = summary["Status"].as_str().unwrap_or("<missing>").to_string();
                organization_status(&reported).to_string()
            }
            Record::ThirdPartyFirewall(vendor) => read_association(router, vendor).await.0,
        }
    }

    /// This record's `(status, settles_to, settles_at)` triple as the store
    /// holds it.
    fn stored(
        self,
        engine: &Arc<engine::InMemoryEngine>,
    ) -> (String, Option<String>, Option<SystemTime>) {
        let case = self.name();
        with_store(engine, |store| match self {
            Record::OrganizationAdmin => {
                let record = store
                    .organization_admins
                    .get(DELEGATED_ADMIN)
                    .unwrap_or_else(|| panic!("case {case:?}: the store must hold the onboarded administrator"));
                (
                    record.status.clone(),
                    record.settles_to.clone(),
                    record.settles_at,
                )
            }
            Record::ThirdPartyFirewall(vendor) => {
                let record = store
                    .third_party_firewalls
                    .get(vendor)
                    .unwrap_or_else(|| panic!("case {case:?}: the store must hold the association"));
                (
                    record.status.clone(),
                    record.settles_to.clone(),
                    record.settles_at,
                )
            }
        })
    }

    /// Move this record's stored due instant to `at`, leaving everything else
    /// the starting operation wrote exactly as it wrote it — so a row of the
    /// due-instant table varies the due instant and nothing else.
    fn set_due(self, engine: &Arc<engine::InMemoryEngine>, at: SystemTime) {
        let case = self.name();
        with_store(engine, |store| match self {
            Record::OrganizationAdmin => {
                let record = store
                    .organization_admins
                    .get_mut(DELEGATED_ADMIN)
                    .unwrap_or_else(|| panic!("case {case:?}: the store must hold the onboarded administrator"));
                record.settles_at = Some(at);
            }
            Record::ThirdPartyFirewall(vendor) => {
                let record = store
                    .third_party_firewalls
                    .get_mut(vendor)
                    .unwrap_or_else(|| panic!("case {case:?}: the store must hold the association"));
                record.settles_at = Some(at);
            }
        });
    }
}

/// `name` as the model's own `ThirdPartyFirewallAssociationStatus` catalog
/// spells it, so an expectation below names a value the enum actually carries.
fn association_status(name: &str) -> &'static str {
    THIRD_PARTY_FIREWALL_ASSOCIATION_STATUS_VALUES
        .iter()
        .copied()
        .find(|value| *value == name)
        .unwrap_or_else(|| {
            panic!(
                "{name} must be a modelled ThirdPartyFirewallAssociationStatus, catalog is \
                 {THIRD_PARTY_FIREWALL_ASSOCIATION_STATUS_VALUES:?}"
            )
        })
}

/// `name` as the model's own `MarketplaceSubscriptionOnboardingStatus` catalog
/// spells it.
fn subscription_status(name: &str) -> &'static str {
    MARKETPLACE_SUBSCRIPTION_ONBOARDING_STATUS_VALUES
        .iter()
        .copied()
        .find(|value| *value == name)
        .unwrap_or_else(|| {
            panic!(
                "{name} must be a modelled MarketplaceSubscriptionOnboardingStatus, catalog is \
                 {MARKETPLACE_SUBSCRIPTION_ONBOARDING_STATUS_VALUES:?}"
            )
        })
}

/// `GetThirdPartyFirewallAssociationStatus` for one vendor, as
/// `(ThirdPartyFirewallStatus, MarketplaceOnboardingStatus)`, each checked
/// against the model's own catalog on the way out.
async fn read_association(router: &MultiServiceRouter, vendor: &str) -> (String, String) {
    let (status, body) = call(
        router,
        "GetThirdPartyFirewallAssociationStatus",
        &json!({ "ThirdPartyFirewall": vendor }),
    )
    .await;
    assert_eq!(
        status, 200,
        "vendor {vendor:?}: expected GetThirdPartyFirewallAssociationStatus to answer 200, got \
         {status} with {body:?}"
    );
    let association = body["ThirdPartyFirewallStatus"]
        .as_str()
        .unwrap_or("<missing>")
        .to_string();
    let subscription = body["MarketplaceOnboardingStatus"]
        .as_str()
        .unwrap_or("<missing>")
        .to_string();
    (
        association_status(&association).to_string(),
        subscription_status(&subscription).to_string(),
    )
}

/// Every record type settles the same way: pending until its stored instant,
/// terminal from it onwards, with both pending members cleared once the
/// transition has been materialised, and a further report stable.
#[tokio::test]
async fn every_record_type_settles_its_pending_transition_the_same_way() {
    for record in records() {
        let case = record.name();
        let (router, engine) = emulator_with_engine();
        associate_admin(&router).await;
        record.start(&router).await;

        let reported = record.reported_status(&router).await;
        assert_eq!(
            reported,
            record.transient(),
            "case {case:?}: before the stored due instant the report must be the transient \
             status {:?}, got {reported:?}",
            record.transient()
        );
        let (stored_status, settles_to, settles_at) = record.stored(&engine);
        assert_eq!(
            (stored_status.as_str(), settles_to.as_deref()),
            (record.transient(), Some(record.terminal())),
            "case {case:?}: a record whose transition is still pending must hold the transient \
             status and the status it settles to, got {stored_status:?} / {settles_to:?}"
        );
        assert!(
            settles_at.is_some(),
            "case {case:?}: a pending transition must carry its due instant, got {settles_at:?}"
        );

        sleep(record.span() + SETTLE_SLACK);

        let reported = record.reported_status(&router).await;
        assert_eq!(
            reported,
            record.terminal(),
            "case {case:?}: once the due instant has passed the report must be the terminal \
             status {:?}, got {reported:?}",
            record.terminal()
        );
        let (stored_status, settles_to, settles_at) = record.stored(&engine);
        assert_eq!(
            stored_status,
            record.terminal(),
            "case {case:?}: reporting a due transition must persist it, got {stored_status:?}"
        );
        assert_eq!(
            (settles_to.as_deref(), settles_at),
            (None, None),
            "case {case:?}: a settled record owes the clock nothing, so BOTH pending members \
             must be cleared, got {settles_to:?} / {settles_at:?}"
        );

        let again = record.reported_status(&router).await;
        assert_eq!(
            again,
            record.terminal(),
            "case {case:?}: a settled status does not move on a further report, got {again:?}"
        );
    }
}

/// The due instant decides, for every record type alike: a transition whose
/// instant the report is at or past is materialised, one whose instant is still
/// ahead leaves the record exactly as it was.
#[tokio::test]
async fn the_due_instant_decides_every_record_type_alike() {
    // The due-instant rows, as an offset from the instant the row is seeded at:
    // negative is already past, zero is the seeding instant itself (which the
    // report that follows is at or past), positive is still ahead.
    let rows: &[(&str, i64, bool)] = &[
        ("due a minute ago", -60_000, true),
        ("due a millisecond ago", -1, true),
        ("due at this very instant", 0, true),
        ("due in a second", 1_000, false),
        ("due in a minute", 60_000, false),
    ];

    for record in records() {
        for (row, offset_millis, expected_due) in rows.iter().copied() {
            let case = format!("{} / {row}", record.name());
            let (router, engine) = emulator_with_engine();
            associate_admin(&router).await;
            record.start(&router).await;

            let now = SystemTime::now();
            let offset = Duration::from_millis(offset_millis.unsigned_abs());
            let due = if offset_millis < 0 {
                now - offset
            } else {
                now + offset
            };
            record.set_due(&engine, due);

            let reported = record.reported_status(&router).await;
            let expected = if expected_due {
                record.terminal()
            } else {
                record.transient()
            };
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the report to be {expected:?}, got {reported:?}"
            );

            let (stored_status, settles_to, settles_at) = record.stored(&engine);
            if expected_due {
                assert_eq!(
                    (stored_status.as_str(), settles_to.as_deref(), settles_at),
                    (record.terminal(), None, None),
                    "case {case:?}: a materialised transition leaves the terminal status and no \
                     pending members, got {stored_status:?} / {settles_to:?} / {settles_at:?}"
                );
            } else {
                assert_eq!(
                    (stored_status.as_str(), settles_to.as_deref(), settles_at),
                    (record.transient(), Some(record.terminal()), Some(due)),
                    "case {case:?}: a report before the due instant must leave the record \
                     untouched, got {stored_status:?} / {settles_to:?} / {settles_at:?}"
                );
            }
        }
    }
}

/// The vendor association's own side effect of settling — the Marketplace
/// subscription following the designation — still happens, and the teardown
/// transition settles through the same rule as the designation without
/// cancelling the subscription.
#[tokio::test]
async fn the_vendor_associations_own_settling_side_effect_still_follows() {
    for vendor in THIRD_PARTY_FIREWALL_VALUES.iter().copied() {
        let (router, engine) = emulator_with_engine();
        associate_admin(&router).await;
        Record::ThirdPartyFirewall(vendor).start(&router).await;

        let reported = read_association(&router, vendor).await;
        assert_eq!(
            (reported.0.as_str(), reported.1.as_str()),
            (
                association_status(state::THIRD_PARTY_FIREWALL_ONBOARDING),
                subscription_status(state::MARKETPLACE_ONBOARDING_NOT_COMPLETE)
            ),
            "vendor {vendor:?}: a designation in flight carries a subscription in flight, got \
             {reported:?}"
        );

        sleep(state::ASSOCIATION_SETTLE + SETTLE_SLACK);
        let reported = read_association(&router, vendor).await;
        assert_eq!(
            (reported.0.as_str(), reported.1.as_str()),
            (
                association_status(state::THIRD_PARTY_FIREWALL_ONBOARD_COMPLETE),
                subscription_status(state::MARKETPLACE_ONBOARDING_COMPLETE)
            ),
            "vendor {vendor:?}: the designation completing is what makes the subscription \
             active, got {reported:?}"
        );

        let (status, body) = call(
            &router,
            "DisassociateThirdPartyFirewall",
            &json!({ "ThirdPartyFirewall": vendor }),
        )
        .await;
        assert_eq!(
            status, 200,
            "vendor {vendor:?}: expected DisassociateThirdPartyFirewall to answer 200, got \
             {status} with {body:?}"
        );
        let reported = read_association(&router, vendor).await;
        assert_eq!(
            (reported.0.as_str(), reported.1.as_str()),
            (
                association_status(state::THIRD_PARTY_FIREWALL_OFFBOARDING),
                subscription_status(state::MARKETPLACE_ONBOARDING_COMPLETE)
            ),
            "vendor {vendor:?}: a teardown in flight does not cancel the subscription, got \
             {reported:?}"
        );

        sleep(state::ASSOCIATION_SETTLE + SETTLE_SLACK);
        let reported = read_association(&router, vendor).await;
        assert_eq!(
            (reported.0.as_str(), reported.1.as_str()),
            (
                association_status(state::THIRD_PARTY_FIREWALL_OFFBOARD_COMPLETE),
                subscription_status(state::MARKETPLACE_ONBOARDING_COMPLETE)
            ),
            "vendor {vendor:?}: the teardown settles through the same rule and keeps the \
             subscription it had, got {reported:?}"
        );
        let (_, settles_to, settles_at) = Record::ThirdPartyFirewall(vendor).stored(&engine);
        assert_eq!(
            (settles_to.as_deref(), settles_at),
            (None, None),
            "vendor {vendor:?}: a settled teardown owes the clock nothing, got {settles_to:?} / \
             {settles_at:?}"
        );
    }
}
