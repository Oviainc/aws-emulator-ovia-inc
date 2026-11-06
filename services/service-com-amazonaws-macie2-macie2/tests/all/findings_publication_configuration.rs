// SPDX-License-Identifier: CC0-1.0

//! The Security Hub findings-publication configuration, end to end over the
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
//! * `PutFindingsPublicationConfiguration` PERSISTS the `securityHubConfiguration`
//!   it is given, so `GetFindingsPublicationConfiguration` reports exactly those
//!   two settings back — every combination of them, not one row;
//! * an account that has written no configuration is reported with the settings
//!   the two members document, and a read materialises nothing, so reading
//!   changes nothing a later reader or writer sees;
//! * the configuration belongs to the region the request signed for and to that
//!   region's Macie session: both operations are refused 403 until Macie is
//!   enabled there, and `DisableMacie` deletes the configuration with the rest
//!   of the account's settings.

use service_com_amazonaws_macie2_macie2 as macie2;
use smithy_server_runtime::MultiServiceRouter;

use super::macie_session_and_custom_data_identifiers::{
    call_in, emulator, enable_in, error_code,
};

/// The region most cases sign for.
const REGION: &str = "us-west-2";

/// A second region, for the cases that pin the per-region scope of this
/// configuration.
const OTHER_REGION: &str = "eu-west-1";

/// The configuration's URI, from the operations' `@http` bindings (`PUT` for the
/// write, `GET` for the read).
const CONFIGURATION_URI: &str = "/findings-publication-configuration";

/// Write the configuration, requiring the modelled 200 and the empty body the
/// response shape models.
async fn put(router: &MultiServiceRouter, region: &str, body: &serde_json::Value, case: &str) {
    let (status, reply) =
        call_in(router, region, "PUT", CONFIGURATION_URI, &body.to_string()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutFindingsPublicationConfiguration must answer 200, \
         got {status} with {reply:?}"
    );
    assert!(
        reply.is_null() || reply == serde_json::json!({}),
        "case {case:?}: the response models no members, got {reply:?}"
    );
}

/// Read the configuration back, requiring the modelled 200.
async fn read(router: &MultiServiceRouter, region: &str, case: &str) -> serde_json::Value {
    let (status, reply) = call_in(router, region, "GET", CONFIGURATION_URI, "").await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetFindingsPublicationConfiguration must answer 200, \
         got {status} with {reply:?}"
    );
    reply
}

/// The two settings a read reports, each required to be the boolean the
/// `SecurityHubConfiguration` members model.
fn reported(case: &str, body: &serde_json::Value) -> Settings {
    let configuration = &body["securityHubConfiguration"];
    let classification = configuration["publishClassificationFindings"]
        .as_bool()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: publishClassificationFindings must be a boolean, got {body:?}"
            )
        });
    let policy = configuration["publishPolicyFindings"]
        .as_bool()
        .unwrap_or_else(|| {
            panic!("case {case:?}: publishPolicyFindings must be a boolean, got {body:?}")
        });
    (classification, policy)
}

/// One row of the write/read table: the configuration a request names (`None`
/// when it names none) and the settings a read must report afterwards.
type Row = (Option<Settings>, Settings);

/// The two settings a `SecurityHubConfiguration` carries, classification first.
type Settings = (bool, bool);

/// The settings an account publishes findings with until something writes a
/// configuration, as the two members document them.
fn documented_defaults() -> Settings {
    (
        macie2::state::DEFAULT_PUBLISH_CLASSIFICATION_FINDINGS,
        macie2::state::DEFAULT_PUBLISH_POLICY_FINDINGS,
    )
}

/// An account that has written no configuration is reported with the settings
/// the two members document, and reading it twice reports the same thing —
/// a read materialises nothing.
#[tokio::test]
async fn an_unconfigured_account_reports_the_documented_settings() {
    let router = emulator();
    enable_in(&router, REGION, "{}").await;

    let first = read(&router, REGION, "unconfigured").await;
    assert_eq!(
        reported("unconfigured", &first),
        documented_defaults(),
        "case \"unconfigured\": the settings the members document must be reported, \
         got {first:?}"
    );

    let again = read(&router, REGION, "unconfigured twice").await;
    assert_eq!(
        again, first,
        "case \"unconfigured twice\": a read must not change what the next reader sees, \
         got {again:?}"
    );
}

/// A write persists exactly the two settings it names, and the read reports
/// them. The table spans every combination of the two booleans the modelled
/// `SecurityHubConfiguration` carries, plus the row where the request names no
/// configuration at all — which leaves the account publishing findings as the
/// members document. Each row is written on top of the previous one, so the
/// read also pins that a later write REPLACES what the earlier one stored.
#[tokio::test]
async fn a_write_persists_every_combination_of_settings_a_read_reports() {
    let router = emulator();
    enable_in(&router, REGION, "{}").await;

    let mut rows: Vec<Row> = Vec::new();
    for classification in [false, true] {
        for policy in [false, true] {
            rows.push((
                Some((classification, policy)),
                (classification, policy),
            ));
        }
    }
    // The absent member is a row of its own: a call naming no configuration
    // states the one the members document.
    rows.push((None, documented_defaults()));

    for (requested, expected) in rows {
        let case = format!("requested {requested:?}");
        let body = match requested {
            Some((classification, policy)) => serde_json::json!({
                "securityHubConfiguration": {
                    "publishClassificationFindings": classification,
                    "publishPolicyFindings": policy,
                }
            }),
            None => serde_json::json!({}),
        };
        put(&router, REGION, &body, &case).await;

        let back = read(&router, REGION, &case).await;
        assert_eq!(
            reported(&case, &back),
            expected,
            "case {case:?}: the stored settings must be reported, got {back:?}"
        );
    }
}

/// A retried write carrying the same `clientToken` and the same configuration
/// leaves exactly the configuration the first call wrote: the write states the
/// settings the account is to have, so repeating it changes nothing. The rows
/// are the two configurations a retry is exercised on.
#[tokio::test]
async fn a_retried_write_leaves_the_configuration_it_wrote() {
    let router = emulator();
    enable_in(&router, REGION, "{}").await;

    for (case, classification, policy) in [
        ("publish both", true, true),
        ("publish neither", false, false),
    ] {
        let body = serde_json::json!({
            "clientToken": format!("token-for-{case}"),
            "securityHubConfiguration": {
                "publishClassificationFindings": classification,
                "publishPolicyFindings": policy,
            }
        });
        put(&router, REGION, &body, case).await;
        let written = read(&router, REGION, case).await;

        put(&router, REGION, &body, case).await;
        let retried = read(&router, REGION, case).await;
        assert_eq!(
            retried, written,
            "case {case:?}: a retry carrying the same token must leave the configuration \
             the first call wrote, got {retried:?}"
        );
        assert_eq!(
            reported(case, &retried),
            (classification, policy),
            "case {case:?}: the settings written must still be the ones reported, \
             got {retried:?}"
        );
    }
}

/// The configuration belongs to the region the request signed for: writing it
/// in one region leaves another region's account reported with the documented
/// settings, and both operations are refused 403 `AccessDeniedException` in a
/// region where nothing has enabled Macie. The rows are the two operations.
#[tokio::test]
async fn the_configuration_is_scoped_to_the_region_and_needs_a_session() {
    let router = emulator();

    for (case, method, body) in [
        ("GetFindingsPublicationConfiguration", "GET", String::new()),
        (
            "PutFindingsPublicationConfiguration",
            "PUT",
            serde_json::json!({
                "securityHubConfiguration": {
                    "publishClassificationFindings": true,
                    "publishPolicyFindings": true,
                }
            })
            .to_string(),
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
    put(
        &router,
        REGION,
        &serde_json::json!({
            "securityHubConfiguration": {
                "publishClassificationFindings": true,
                "publishPolicyFindings": false,
            }
        }),
        "configure one region",
    )
    .await;

    let here = read(&router, REGION, "configure one region").await;
    assert_eq!(
        reported("configure one region", &here),
        (true, false),
        "case \"configure one region\": the region written reports its own settings, \
         got {here:?}"
    );
    let elsewhere = read(&router, OTHER_REGION, "another region").await;
    assert_eq!(
        reported("another region", &elsewhere),
        documented_defaults(),
        "case \"another region\": configuring one region must not configure another, \
         got {elsewhere:?}"
    );
}

/// `DisableMacie` "deletes all settings and resources for a Macie account", so
/// the findings-publication configuration goes with the session: a region
/// enabled again is reported with the documented settings rather than the ones
/// the deleted configuration carried.
#[tokio::test]
async fn disabling_macie_deletes_the_configuration() {
    let router = emulator();
    enable_in(&router, REGION, "{}").await;
    put(
        &router,
        REGION,
        &serde_json::json!({
            "securityHubConfiguration": {
                "publishClassificationFindings": true,
                "publishPolicyFindings": false,
            }
        }),
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
        reported("after re-enabling Macie", &after),
        documented_defaults(),
        "case \"after re-enabling Macie\": the configuration was deleted with the \
         session, got {after:?} after {configured:?}"
    );
}
