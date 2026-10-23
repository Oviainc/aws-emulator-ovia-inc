// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for Service Trust Enablement between User Notifications and
//! Amazon Web Services Organizations: `GetNotificationsAccessForOrganization`
//! and the enable and disable that write what it reports, driven over HTTP
//! through the same multi-service router the emulator boots in production.
//!
//! Every case asserts the RULE the operations follow, not one recorded request.
//! These operations take no input, so the rows vary along what the rules DO
//! generalise over: which stored `AccessStatus` the account holds when the read
//! arrives (every value of the modelled catalog
//! `crate::ops::types::ACCESS_STATUS_VALUES`, written straight into the store),
//! how many times a call is repeated, and the order enable, disable and read
//! arrive in.

use std::time::Duration;

use serde_json::Value;
use service_com_amazonaws_notifications_notifications::ops::types::ACCESS_STATUS_VALUES;
use service_com_amazonaws_notifications_notifications::state::{
    ComAmazonawsNotificationsNotificationsState, OrganizationAccessRecord,
    ORGANIZATION_ACCESS_DISABLED, ORGANIZATION_ACCESS_ENABLED, ORGANIZATION_ACCESS_PENDING,
    ORGANIZATION_ACCESS_TRANSITION_SPAN,
};

// The HTTP harness this suite drives the service through, shared with every
// other suite in this crate.
use crate::harness::Emulator;

/// Long enough that a transition the enable made due has certainly fallen due,
/// without depending on how slow the machine is.
const PAST_THE_TRANSITION: Duration =
    ORGANIZATION_ACCESS_TRANSITION_SPAN.saturating_add(Duration::from_millis(400));

/// The organization-access helpers this suite adds to the shared harness. The
/// URI and method of each are the model's own http bindings for these three
/// operations (`GET`, `POST` and `DELETE` on `/organization/access`).
impl Emulator {
    async fn get_organization_access(&self) -> (u16, Value) {
        self.send("GET", "/organization/access", None).await
    }

    async fn enable_organization_access(&self) -> (u16, Value) {
        self.send("POST", "/organization/access", None).await
    }

    async fn disable_organization_access(&self) -> (u16, Value) {
        self.send("DELETE", "/organization/access", None).await
    }

    /// Write one `AccessStatus` straight into the store, so a read can be
    /// exercised over statuses no call sequence reaches (`FAILED`, say).
    fn store_access_status(&self, status: &str) {
        self.engine
            .state::<ComAmazonawsNotificationsNotificationsState>()
            .lock()
            .organization_access = OrganizationAccessRecord {
            access_status: status.to_string(),
            due_at: None,
        };
    }
}

/// The `accessStatus` a read reported, checked against the modelled value set.
fn reported_status(case: &str, response: &Value) -> String {
    let status = response["notificationsAccessForOrganization"]["accessStatus"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: a read must carry notificationsAccessForOrganization.accessStatus, got {response:?}")
        })
        .to_string();
    assert!(
        // Catalog source: the `AccessStatus` enum shape, re-exported by the
        // service crate.
        ACCESS_STATUS_VALUES.contains(&status.as_str()),
        "case {case:?}: accessStatus must be one of the modelled AccessStatus values {ACCESS_STATUS_VALUES:?}, got {status:?}"
    );
    status
}

/// RULE: the read reports the account's PERSISTED `AccessStatus` — whichever of
/// the modelled values the store holds — and reports it unchanged however many
/// times it is called.
#[tokio::test]
async fn the_read_reports_whichever_stored_access_status_the_account_holds() {
    // Rows: every value of the modelled catalog, including the two no call
    // sequence in this emulator produces (`FAILED`) or leaves settled
    // (`PENDING`), written straight into the store.
    for status in ACCESS_STATUS_VALUES {
        let emulator = Emulator::new();
        emulator.store_access_status(status);

        let (code, response) = emulator.get_organization_access().await;
        assert_eq!(
            code, 200,
            "case {status:?}: a read answers the modelled 200, got {response:?}"
        );
        assert_eq!(
            reported_status(status, &response),
            *status,
            "case {status:?}: the read reports the stored status, got {response:?}"
        );

        // A read changes nothing a later reader sees.
        let (code, again) = emulator.get_organization_access().await;
        assert_eq!(
            code, 200,
            "case {status:?}: a repeated read answers 200, got {again:?}"
        );
        assert_eq!(
            again, response,
            "case {status:?}: a read must not change what the next read reports, got {again:?}"
        );
    }
}

/// RULE: an account nobody has enabled trust for holds no trust, so a read on a
/// store that has seen no enable reports `DISABLED`.
#[tokio::test]
async fn an_account_that_never_enabled_trust_reads_as_disabled() {
    let emulator = Emulator::new();
    let case = "a store with no enable";

    let (code, response) = emulator.get_organization_access().await;
    assert_eq!(
        code, 200,
        "case {case:?}: a read answers the modelled 200, got {response:?}"
    );
    assert_eq!(
        reported_status(case, &response),
        ORGANIZATION_ACCESS_DISABLED,
        "case {case:?}: trust is off until an account enables it, got {response:?}"
    );
}

/// RULE: the enable persists the request for trust — the read reports the
/// transitional `PENDING` straight afterwards — and the CLOCK, not a later call,
/// makes `ENABLED` due; the disable returns the account to `DISABLED`
/// immediately.
#[tokio::test]
async fn enable_is_pending_until_the_clock_enables_it_and_disable_undoes_it() {
    let emulator = Emulator::new();
    let case = "enable then read";

    let (code, enabled) = emulator.enable_organization_access().await;
    assert_eq!(
        code, 200,
        "case {case:?}: an enable answers the modelled 200, got {enabled:?}"
    );

    // The transitional status is observable: a caller reading straight after the
    // enable still sees PENDING.
    let (code, pending) = emulator.get_organization_access().await;
    assert_eq!(
        code, 200,
        "case {case:?}: a read answers 200, got {pending:?}"
    );
    assert_eq!(
        reported_status(case, &pending),
        ORGANIZATION_ACCESS_PENDING,
        "case {case:?}: an enablement in flight reads as PENDING, got {pending:?}"
    );

    // Only the clock moves it on.
    tokio::time::sleep(PAST_THE_TRANSITION).await;
    let (code, settled) = emulator.get_organization_access().await;
    assert_eq!(
        code, 200,
        "case {case:?}: a read answers 200, got {settled:?}"
    );
    assert_eq!(
        reported_status(case, &settled),
        ORGANIZATION_ACCESS_ENABLED,
        "case {case:?}: the clock enables a pending enablement, got {settled:?}"
    );

    // ENABLED is terminal: time alone does not move it again.
    tokio::time::sleep(PAST_THE_TRANSITION).await;
    let (_, still) = emulator.get_organization_access().await;
    assert_eq!(
        reported_status(case, &still),
        ORGANIZATION_ACCESS_ENABLED,
        "case {case:?}: enabled trust stays enabled, got {still:?}"
    );

    let case = "disable then read";
    let (code, disabled) = emulator.disable_organization_access().await;
    assert_eq!(
        code, 200,
        "case {case:?}: a disable answers the modelled 200, got {disabled:?}"
    );
    let (_, after) = emulator.get_organization_access().await;
    assert_eq!(
        reported_status(case, &after),
        ORGANIZATION_ACCESS_DISABLED,
        "case {case:?}: a disable withdraws trust at once, got {after:?}"
    );

    // And trust can be asked for again, starting over at PENDING.
    emulator.enable_organization_access().await;
    let (_, again) = emulator.get_organization_access().await;
    assert_eq!(
        reported_status(case, &again),
        ORGANIZATION_ACCESS_PENDING,
        "case {case:?}: a re-enable starts over at PENDING, got {again:?}"
    );
}

/// RULE: both writes are `@idempotent` — repeating an enable neither restarts
/// the transition nor moves settled trust back to `PENDING`, and repeating a
/// disable leaves the account `DISABLED` — and a disable that arrives while the
/// enablement is still pending cancels it, so the clock cannot overtake it.
#[tokio::test]
async fn repeating_a_write_leaves_the_stored_status_where_the_first_one_put_it() {
    // Rows differ in how many times the call is repeated and in which stage the
    // record is at when the repeat arrives.
    for repeats in [1usize, 2, 3] {
        let case = format!("{repeats} enables while pending");
        let emulator = Emulator::new();
        for _ in 0..repeats {
            let (code, response) = emulator.enable_organization_access().await;
            assert_eq!(
                code, 200,
                "case {case:?}: a repeated enable stays successful, got {response:?}"
            );
        }
        let (_, pending) = emulator.get_organization_access().await;
        assert_eq!(
            reported_status(&case, &pending),
            ORGANIZATION_ACCESS_PENDING,
            "case {case:?}: repeating the enable does not change the stage it is at, got {pending:?}"
        );

        // The repeats must not have pushed the due instant out: one span after
        // the FIRST enable, trust is in force.
        tokio::time::sleep(PAST_THE_TRANSITION).await;
        let (_, settled) = emulator.get_organization_access().await;
        assert_eq!(
            reported_status(&case, &settled),
            ORGANIZATION_ACCESS_ENABLED,
            "case {case:?}: a repeated enable does not restart the transition, got {settled:?}"
        );

        // Repeating it on settled trust reports the same thing.
        let case = format!("{repeats} enables while enabled");
        for _ in 0..repeats {
            let (code, response) = emulator.enable_organization_access().await;
            assert_eq!(
                code, 200,
                "case {case:?}: enabling in-force trust stays successful, got {response:?}"
            );
        }
        let (_, after) = emulator.get_organization_access().await;
        assert_eq!(
            reported_status(&case, &after),
            ORGANIZATION_ACCESS_ENABLED,
            "case {case:?}: enabling in-force trust leaves it enabled, got {after:?}"
        );

        // Repeated disables are idempotent too.
        let case = format!("{repeats} disables");
        for _ in 0..repeats {
            let (code, response) = emulator.disable_organization_access().await;
            assert_eq!(
                code, 200,
                "case {case:?}: a repeated disable stays successful, got {response:?}"
            );
        }
        let (_, disabled) = emulator.get_organization_access().await;
        assert_eq!(
            reported_status(&case, &disabled),
            ORGANIZATION_ACCESS_DISABLED,
            "case {case:?}: repeated disables leave trust withdrawn, got {disabled:?}"
        );
    }

    // A disable while the enablement is still pending cancels it: the clock has
    // nothing left to make due.
    let case = "disable while pending";
    let emulator = Emulator::new();
    emulator.enable_organization_access().await;
    emulator.disable_organization_access().await;
    let (_, disabled) = emulator.get_organization_access().await;
    assert_eq!(
        reported_status(case, &disabled),
        ORGANIZATION_ACCESS_DISABLED,
        "case {case:?}: a disable withdraws a pending enablement, got {disabled:?}"
    );
    tokio::time::sleep(PAST_THE_TRANSITION).await;
    let (_, later) = emulator.get_organization_access().await;
    assert_eq!(
        reported_status(case, &later),
        ORGANIZATION_ACCESS_DISABLED,
        "case {case:?}: the cancelled enablement never falls due, got {later:?}"
    );
}
