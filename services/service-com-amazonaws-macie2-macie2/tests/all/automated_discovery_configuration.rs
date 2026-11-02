// SPDX-License-Identifier: CC0-1.0

//! Automated sensitive data discovery's configuration, end to end over the
//! wire.
//!
//! Each case drives the production `MultiServiceRouter` through the harness
//! `macie_session_and_custom_data_identifiers` builds, so what is pinned is the
//! HTTP behaviour and the response BODIES rather than a handler called
//! directly.
//!
//! The rules under test, each asserted over a SET of inputs rather than one
//! request:
//!
//! * `UpdateAutomatedDiscoveryConfiguration` PERSISTS the status it is given
//!   and the auto-enable mode it names — the documented default for an omitted
//!   one — so `GetAutomatedDiscoveryConfiguration` reports exactly that;
//! * the write materialises the classification scope and the sensitivity
//!   inspection template the analyses use, once, and both reads report their
//!   identifiers unchanged afterwards;
//! * `firstEnabledAt` is the instant of the FIRST enable and never moves,
//!   `disabledAt` is the instant of the most recent disable and is absent while
//!   automated discovery is enabled, and `lastUpdatedAt` moves only when the
//!   status or the settings actually changed;
//! * reading the configuration changes nothing a later reader sees, and nothing
//!   is materialised by a read;
//! * the configuration belongs to the region the request signed for and to that
//!   region's Macie session: both operations are refused 403 until Macie is
//!   enabled there, and `DisableMacie` deletes the configuration with the rest
//!   of the account's settings.

use std::time::Duration;

use service_com_amazonaws_macie2_macie2 as macie2;
use smithy_server_runtime::MultiServiceRouter;

use super::macie_session_and_custom_data_identifiers::{
    call_in, emulator, enable_in, error_code, now_seconds, reported_instant,
};

/// The region most cases sign for.
const REGION: &str = "us-west-2";

/// A second region, for the cases that pin the per-region scope of this
/// configuration.
const OTHER_REGION: &str = "eu-west-1";

/// The automated-discovery configuration's URI, from the operations' `@http`
/// bindings (`PUT` for the update, `GET` for the read).
const CONFIGURATION_URI: &str = "/automated-discovery/configuration";

/// Long enough that two instants stamped either side of it differ in the
/// second-and-fraction a response reports, so "this stamp moved" is observable
/// rather than a race with the clock's resolution.
const CLOCK_STEP: Duration = Duration::from_millis(50);

/// Write the configuration, requiring the modelled 200 and the empty body the
/// response shape models.
async fn update(
    router: &MultiServiceRouter,
    region: &str,
    body: &serde_json::Value,
    case: &str,
) {
    let (status, reply) = call_in(router, region, "PUT", CONFIGURATION_URI, &body.to_string()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateAutomatedDiscoveryConfiguration must answer 200, \
         got {status} with {reply:?}"
    );
    assert!(
        reply.is_null() || reply == serde_json::json!({}),
        "case {case:?}: the response models no members, got {reply:?}"
    );
}

/// Read the configuration back, requiring the modelled 200.
async fn read(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
) -> serde_json::Value {
    let (status, reply) = call_in(router, region, "GET", CONFIGURATION_URI, "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetAutomatedDiscoveryConfiguration must answer 200, \
         got {status} with {reply:?}"
    );
    reply
}

/// Whether a member is absent or null, which is how an instant that has not
/// happened and an identifier that has not been materialised are both reported.
fn is_absent(body: &serde_json::Value, member: &str) -> bool {
    body.get(member).is_none_or(serde_json::Value::is_null)
}

/// An account whose automated sensitive data discovery nothing has configured
/// reports the never-configured state: none of its activities are performed,
/// the auto-enable mode is the one its member documents as the default, and
/// there is no scope, no template and no instant to report. Reading it twice
/// reports the same thing, because a read materialises nothing.
#[tokio::test]
async fn an_unconfigured_account_reports_the_never_configured_state() {
    let router = emulator();
    enable_in(&router, REGION, "{}").await;

    let first = read(&router, REGION, "unconfigured").await;
    assert_eq!(
        first["status"].as_str(),
        Some(macie2::state::DEFAULT_AUTOMATED_DISCOVERY_STATUS),
        "case \"unconfigured\": nothing has asked for the activities, got {first:?}"
    );
    assert_eq!(
        first["autoEnableOrganizationMembers"].as_str(),
        Some(macie2::state::DEFAULT_AUTO_ENABLE_ORGANIZATION_MEMBERS),
        "case \"unconfigured\": the documented default mode must be reported, got {first:?}"
    );
    for member in [
        "classificationScopeId",
        "sensitivityInspectionTemplateId",
        "firstEnabledAt",
        "disabledAt",
        "lastUpdatedAt",
    ] {
        assert!(
            is_absent(&first, member),
            "case \"unconfigured\": {member} must be absent until a write produces it, \
             got {:?}",
            first.get(member)
        );
    }

    let again = read(&router, REGION, "unconfigured twice").await;
    assert_eq!(
        again, first,
        "case \"unconfigured twice\": a read must not change what the next reader sees, \
         got {again:?}"
    );
}

/// A write persists the status it is given and the auto-enable mode it names,
/// and the read reports exactly that. The table spans every value of
/// `AutomatedDiscoveryStatus` against every value of `AutoEnableMode` and
/// against the member being omitted — the enums as `smithy_query` reports them,
/// which this crate's `AUTOMATED_DISCOVERY_STATUS_VALUES` and
/// `AUTO_ENABLE_MODE_VALUES` carry — so the rows differ along both members and
/// along the presence of the optional one.
#[tokio::test]
async fn an_update_persists_every_status_and_auto_enable_mode_a_read_reports() {
    let router = emulator();
    enable_in(&router, REGION, "{}").await;

    let mut modes: Vec<Option<&str>> = macie2::ops::types::AUTO_ENABLE_MODE_VALUES
        .iter()
        .map(|mode| Some(*mode))
        .collect();
    // The absent member is a row of its own: its doc names ALL as the value a
    // request that omits it takes.
    modes.push(None);

    let mut materialised: Option<(String, String)> = None;
    for status in macie2::ops::types::AUTOMATED_DISCOVERY_STATUS_VALUES {
        for mode in &modes {
            let case = format!("status {status} / mode {mode:?}");
            let mut body = serde_json::json!({ "status": status });
            if let Some(mode) = mode {
                body["autoEnableOrganizationMembers"] = serde_json::json!(mode);
            }
            update(&router, REGION, &body, &case).await;

            let reported = read(&router, REGION, &case).await;
            assert_eq!(
                reported["status"].as_str(),
                Some(*status),
                "case {case:?}: the stored status must be reported, got {reported:?}"
            );
            assert_eq!(
                reported["autoEnableOrganizationMembers"].as_str(),
                Some(mode.unwrap_or(macie2::state::DEFAULT_AUTO_ENABLE_ORGANIZATION_MEMBERS)),
                "case {case:?}: the mode named, or the documented default for an omitted \
                 one, must be reported, got {reported:?}"
            );

            let scope = reported["classificationScopeId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a configured account reports its classification \
                            scope, got {reported:?}")
                })
                .to_string();
            let template = reported["sensitivityInspectionTemplateId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a configured account reports its sensitivity \
                            inspection template, got {reported:?}")
                })
                .to_string();
            // `ClassificationScopeId` is modelled with `@pattern("^[0-9a-z]*$")`.
            assert!(
                scope
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()),
                "case {case:?}: a classification scope id is lowercase alphanumeric, \
                 got {scope:?}"
            );
            assert_ne!(
                scope, template,
                "case {case:?}: the scope and the template are two resources, so their \
                 identifiers differ, got {scope:?}"
            );
            match &materialised {
                // The scope and the template are materialised ONCE, with the
                // configuration; a later write reports the same two.
                Some((first_scope, first_template)) => {
                    assert_eq!(
                        (&scope, &template),
                        (first_scope, first_template),
                        "case {case:?}: a later write must not re-materialise the scope \
                         and the template, got {scope:?} and {template:?}"
                    );
                }
                None => materialised = Some((scope, template)),
            }
        }
    }
}

/// The instants the read reports follow their own documentation:
/// `firstEnabledAt` names the FIRST enable and never moves, `disabledAt` names
/// the most recent disable and is absent while automated discovery is enabled,
/// and `lastUpdatedAt` moves exactly when the status or the settings changed.
/// The rows are the steps of an enable/disable cycle, each naming what it
/// changes.
#[tokio::test]
async fn the_reported_instants_follow_the_enable_and_disable_cycle() {
    let router = emulator();
    enable_in(&router, REGION, "{}").await;

    let opened = now_seconds() - 1.0;
    update(
        &router,
        REGION,
        &serde_json::json!({ "status": "ENABLED", "autoEnableOrganizationMembers": "ALL" }),
        "first enable",
    )
    .await;
    let enabled = read(&router, REGION, "first enable").await;
    let first_enabled_at = reported_instant("first enable", "firstEnabledAt", &enabled);
    assert!(
        (opened..=now_seconds() + 1.0).contains(&first_enabled_at),
        "case \"first enable\": firstEnabledAt must be the instant of that call, \
         got {first_enabled_at}"
    );
    assert!(
        is_absent(&enabled, "disabledAt"),
        "case \"first enable\": disabledAt is null while automated discovery is enabled, \
         got {:?}",
        enabled.get("disabledAt")
    );
    let first_update = reported_instant("first enable", "lastUpdatedAt", &enabled);

    // A call asking for the configuration the account already has changes
    // nothing, so no stamp moves.
    std::thread::sleep(CLOCK_STEP);
    update(
        &router,
        REGION,
        &serde_json::json!({ "status": "ENABLED", "autoEnableOrganizationMembers": "ALL" }),
        "re-state the same configuration",
    )
    .await;
    let unchanged = read(&router, REGION, "re-state the same configuration").await;
    assert_eq!(
        unchanged, enabled,
        "case \"re-state the same configuration\": a write that changes nothing must \
         move no stamp, got {unchanged:?}"
    );

    // Only the mode changes: `lastUpdatedAt` moves, the enable stamps do not.
    std::thread::sleep(CLOCK_STEP);
    update(
        &router,
        REGION,
        &serde_json::json!({ "status": "ENABLED", "autoEnableOrganizationMembers": "NEW" }),
        "settings only",
    )
    .await;
    let resettled = read(&router, REGION, "settings only").await;
    let settings_update = reported_instant("settings only", "lastUpdatedAt", &resettled);
    assert!(
        settings_update > first_update,
        "case \"settings only\": changing a setting moves lastUpdatedAt, \
         got {settings_update} after {first_update}"
    );
    assert_eq!(
        reported_instant("settings only", "firstEnabledAt", &resettled),
        first_enabled_at,
        "case \"settings only\": firstEnabledAt names the first enable and never moves"
    );
    assert!(
        is_absent(&resettled, "disabledAt"),
        "case \"settings only\": still enabled, so disabledAt stays null, got {:?}",
        resettled.get("disabledAt")
    );

    // Disabling stamps `disabledAt` and moves `lastUpdatedAt`.
    std::thread::sleep(CLOCK_STEP);
    update(
        &router,
        REGION,
        &serde_json::json!({ "status": "DISABLED", "autoEnableOrganizationMembers": "NEW" }),
        "disable",
    )
    .await;
    let disabled = read(&router, REGION, "disable").await;
    assert_eq!(
        disabled["status"].as_str(),
        Some("DISABLED"),
        "case \"disable\": the stored status must be reported, got {disabled:?}"
    );
    let disabled_at = reported_instant("disable", "disabledAt", &disabled);
    assert!(
        disabled_at > settings_update,
        "case \"disable\": disabledAt must be the instant of the disabling call, \
         got {disabled_at} after {settings_update}"
    );
    assert_eq!(
        reported_instant("disable", "lastUpdatedAt", &disabled),
        disabled_at,
        "case \"disable\": the call that disabled it is also the last change"
    );
    assert_eq!(
        reported_instant("disable", "firstEnabledAt", &disabled),
        first_enabled_at,
        "case \"disable\": firstEnabledAt still names the first enable"
    );

    // Re-enabling clears `disabledAt` and still leaves `firstEnabledAt` alone.
    std::thread::sleep(CLOCK_STEP);
    update(
        &router,
        REGION,
        &serde_json::json!({ "status": "ENABLED", "autoEnableOrganizationMembers": "NEW" }),
        "re-enable",
    )
    .await;
    let re_enabled = read(&router, REGION, "re-enable").await;
    assert!(
        is_absent(&re_enabled, "disabledAt"),
        "case \"re-enable\": disabledAt is null while automated discovery is enabled, \
         got {:?}",
        re_enabled.get("disabledAt")
    );
    assert_eq!(
        reported_instant("re-enable", "firstEnabledAt", &re_enabled),
        first_enabled_at,
        "case \"re-enable\": a later enable must not move firstEnabledAt"
    );
    assert!(
        reported_instant("re-enable", "lastUpdatedAt", &re_enabled) > disabled_at,
        "case \"re-enable\": the status changed, so lastUpdatedAt moves"
    );
}

/// The configuration belongs to the region the request signed for: configuring
/// it in one region leaves another region's account never-configured, and both
/// operations are refused 403 `AccessDeniedException` in a region where nothing
/// has enabled Macie. The rows are the two operations, against a region with no
/// session and against a region whose session knows nothing of the other's
/// configuration.
#[tokio::test]
async fn the_configuration_is_scoped_to_the_region_and_needs_a_session() {
    let router = emulator();

    for (case, method, body) in [
        ("GetAutomatedDiscoveryConfiguration", "GET", String::new()),
        (
            "UpdateAutomatedDiscoveryConfiguration",
            "PUT",
            serde_json::json!({ "status": "ENABLED" }).to_string(),
        ),
    ] {
        let (status, reply) = call_in(&router, REGION, method, CONFIGURATION_URI, &body).await;
        assert_eq!(
            status, 403,
            "case {case:?}: Macie is not enabled here, so the call is refused, \
             got {status} with {reply:?}"
        );
        assert_eq!(
            error_code(&reply),
            "AccessDeniedException",
            "case {case:?}: the refusal must be the modelled AccessDeniedException, \
             got {reply:?}"
        );
    }

    enable_in(&router, REGION, "{}").await;
    enable_in(&router, OTHER_REGION, "{}").await;
    update(
        &router,
        REGION,
        &serde_json::json!({ "status": "ENABLED", "autoEnableOrganizationMembers": "NONE" }),
        "configure one region",
    )
    .await;

    let elsewhere = read(&router, OTHER_REGION, "another region").await;
    assert_eq!(
        elsewhere["status"].as_str(),
        Some(macie2::state::DEFAULT_AUTOMATED_DISCOVERY_STATUS),
        "case \"another region\": configuring one region must not configure another, \
         got {elsewhere:?}"
    );
    assert!(
        is_absent(&elsewhere, "classificationScopeId"),
        "case \"another region\": no write has materialised a scope here, got {:?}",
        elsewhere.get("classificationScopeId")
    );
}

/// `DisableMacie` "deletes all settings and resources for a Macie account", so
/// the automated-discovery configuration goes with the session: a region
/// enabled again reports the never-configured state, and the scope the first
/// configuration named is gone.
#[tokio::test]
async fn disabling_macie_deletes_the_configuration() {
    let router = emulator();
    enable_in(&router, REGION, "{}").await;
    update(
        &router,
        REGION,
        &serde_json::json!({ "status": "ENABLED", "autoEnableOrganizationMembers": "NEW" }),
        "configure before disabling",
    )
    .await;
    let configured = read(&router, REGION, "configure before disabling").await;

    let (status, reply) = call_in(&router, REGION, "DELETE", "/macie", "").await;
    assert_eq!(
        status, 200,
        "case \"disable Macie\": DisableMacie must answer 200, got {status} with {reply:?}"
    );

    enable_in(&router, REGION, "{}").await;
    let after = read(&router, REGION, "after re-enabling Macie").await;
    assert_eq!(
        after["status"].as_str(),
        Some(macie2::state::DEFAULT_AUTOMATED_DISCOVERY_STATUS),
        "case \"after re-enabling Macie\": the configuration was deleted with the \
         session, got {after:?}"
    );
    assert!(
        is_absent(&after, "classificationScopeId")
            && is_absent(&after, "firstEnabledAt")
            && is_absent(&after, "lastUpdatedAt"),
        "case \"after re-enabling Macie\": nothing of the deleted configuration may \
         survive, got {after:?} after {configured:?}"
    );
}

/// A value outside the modelled enums, and a request omitting the required
/// `status`, are refused 400 `ValidationException` — the rows are the two ways
/// a request can miss the input contract.
#[tokio::test]
async fn an_update_outside_the_modelled_input_is_refused() {
    let router = emulator();
    enable_in(&router, REGION, "{}").await;

    for (case, body) in [
        (
            "status outside AutomatedDiscoveryStatus",
            serde_json::json!({ "status": "PAUSED" }),
        ),
        (
            "mode outside AutoEnableMode",
            serde_json::json!({ "status": "ENABLED", "autoEnableOrganizationMembers": "SOME" }),
        ),
        ("required status omitted", serde_json::json!({})),
    ] {
        let (status, reply) =
            call_in(&router, REGION, "PUT", CONFIGURATION_URI, &body.to_string()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a request outside the modelled input is refused 400, \
             got {status} with {reply:?}"
        );
        assert_eq!(
            error_code(&reply),
            "ValidationException",
            "case {case:?}: the refusal must be the modelled ValidationException, \
             got {reply:?}"
        );
    }

    // Nothing of a refused request reached the store.
    let after = read(&router, REGION, "after the refusals").await;
    assert_eq!(
        after["status"].as_str(),
        Some(macie2::state::DEFAULT_AUTOMATED_DISCOVERY_STATUS),
        "case \"after the refusals\": a refused write persists nothing, got {after:?}"
    );
}
