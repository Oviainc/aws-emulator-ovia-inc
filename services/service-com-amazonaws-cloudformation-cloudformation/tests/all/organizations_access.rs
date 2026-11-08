// SPDX-License-Identifier: CC0-1.0

//! Trusted access with Organizations, end to end over the awsQuery wire: what
//! `ActivateOrganizationsAccess` and `DeactivateOrganizationsAccess` write, and what
//! `DescribeOrganizationsAccess` reads back.
//!
//! The rule every case here holds the service to: **a trusted-access read reports the setting
//! the last activation or deactivation left — `ENABLED` once trusted access has been
//! activated, `DISABLED` for an organization nobody activated it for and for one a
//! deactivation put back — the same setting whichever Region the call is signed for, since
//! trusted access is the organization's rather than one Region's; a read that says it is made
//! as a delegated administrator is refused with the modelled `InvalidOperationException`
//! (400), because the account every request reaches this emulator from is the organization's
//! management account and is registered as a delegated administrator of nothing; and no read,
//! answered or refused, moves the setting it reports.**
//!
//! The table spans the class the rule generalises over: every sequence of writes that can
//! precede a read (none at all, one activation, a repeated activation, an activation a
//! deactivation follows, a deactivation of an organization nobody activated, a repeated
//! deactivation, and an activation after a deactivation), the Region each write is signed for
//! and each read is signed for, and each way a read can address the administrator account it
//! is made from (naming no `CallAs`, `SELF`, `DELEGATED_ADMIN`).
//!
//! Every call goes through the real request path (form-encoded body in, XML envelope out), so
//! what is asserted is what a client sees.

use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{call_in, member, router_with_engine, store, xml_text, REGION};

/// The Regions every row is driven from: trusted access is organization-wide, so the Region a
/// write or a read is signed for selects nothing.
const REGIONS: &[&str] = &[REGION, "us-west-2", "eu-west-1"];

/// The two modelled `OrganizationStatus` values a call of this API can leave the setting in
/// (`smithy_query(query="shape", shape="OrganizationStatus")`: `ENABLED | DISABLED |
/// DISABLED_PERMANENTLY`; the third is trusted access withdrawn outside CloudFormation, which
/// no operation here performs).
///
/// Spelled out rather than read from the crate, because the value a client reads IS the
/// requirement.
const ENABLED: &str = "ENABLED";
const DISABLED: &str = "DISABLED";

/// The modelled `CallAs` values (`smithy_query(query="shape", shape="CallAs")`: `SELF |
/// DELEGATED_ADMIN`).
const SELF_CALL: &str = "SELF";
const DELEGATED_ADMIN: &str = "DELEGATED_ADMIN";

/// The operations that write the setting.
const ACTIVATE: &str = "ActivateOrganizationsAccess";
const DEACTIVATE: &str = "DeactivateOrganizationsAccess";

/// The error a read made as a delegated administrator is refused with: the modelled
/// `InvalidOperationException`, "the specified operation isn't valid" (HTTP 400).
const INVALID_OPERATION: &str = "InvalidOperationException";

/// The sentence that refusal carries. Entirely fixed text, spelled out here rather than read
/// from the crate, because the wording IS the requirement and a reworded one must fail this
/// suite.
const NOT_A_DELEGATED_ADMINISTRATOR: &str = "Account used is not a delegated administrator";

/// Every sequence of writes a read can follow, and the status the setting is left in by it.
const SEQUENCES: &[(&str, &[&str], &str)] = &[
    ("no write at all", &[], DISABLED),
    ("one activation", &[ACTIVATE], ENABLED),
    ("a repeated activation", &[ACTIVATE, ACTIVATE], ENABLED),
    ("an activation a deactivation follows", &[ACTIVATE, DEACTIVATE], DISABLED),
    (
        "a deactivation of an organization nobody activated",
        &[DEACTIVATE],
        DISABLED,
    ),
    ("a repeated deactivation", &[DEACTIVATE, DEACTIVATE], DISABLED),
    (
        "an activation after a deactivation",
        &[ACTIVATE, DEACTIVATE, ACTIVATE],
        ENABLED,
    ),
];

/// One `DescribeOrganizationsAccess` read, naming `call_as` where the row names one.
async fn describe(
    router: &MultiServiceRouter,
    region: &str,
    call_as: Option<&str>,
) -> (u16, String) {
    let members: Vec<(String, String)> = call_as
        .map(|value| vec![member("CallAs", value)])
        .unwrap_or_default();
    call_in(router, region, "DescribeOrganizationsAccess", &members).await
}

/// The `Status` a read that is expected to ANSWER reports.
async fn reported_status(
    case: &str,
    router: &MultiServiceRouter,
    region: &str,
    call_as: Option<&str>,
) -> String {
    let (status_code, body) = describe(router, region, call_as).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected the read to succeed, got {status_code} {body}"
    );
    xml_text(&body, "Status")
        .unwrap_or_else(|| panic!("case {case:?}: expected a Status, got {body}"))
}

/// One write of the setting, signed for `region`, held to a success that reports nothing of its
/// own: the model gives both writers an output with no members.
async fn write_trusted_access(case: &str, router: &MultiServiceRouter, region: &str, action: &str) {
    let (status_code, body) = call_in(router, region, action, &[]).await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected {action} to succeed, got {status_code} {body}"
    );
    let reported = xml_text(&body, "Status");
    assert!(
        reported.is_none(),
        "case {case:?}: expected {action} to report no status of its own, got {reported:?}"
    );
}

#[tokio::test]
async fn a_read_reports_the_trusted_access_the_writes_left() {
    for (sequence, writes, expected) in SEQUENCES {
        let (router, engine) = router_with_engine();

        // Each write of the sequence is signed for a different Region, since where trusted
        // access is activated from does not scope it.
        for (index, action) in writes.iter().enumerate() {
            let region = REGIONS[index % REGIONS.len()];
            let case = format!("{sequence}: {action} signed for {region}");
            write_trusted_access(&case, &router, region, action).await;
        }

        for region in REGIONS {
            for (addressing, call_as) in [
                ("naming no CallAs", None),
                ("naming CallAs=SELF", Some(SELF_CALL)),
                // "By default, SELF is specified", so both address the management account the
                // writes above were made by.
            ] {
                let case = format!("{sequence}, read {addressing} in {region}");
                let reported = reported_status(&case, &router, region, call_as).await;
                assert_eq!(
                    reported, *expected,
                    "case {case:?}: expected {expected:?}, got {reported:?}"
                );
            }
        }

        // None of those reads moved what they reported: the setting a later caller sees is
        // still the one the writes left.
        let held = store(&engine).lock().organizations_access.status();
        assert_eq!(
            held, *expected,
            "case {sequence:?}: expected the reads to leave the setting at {expected:?}, got \
             {held:?}"
        );
    }
}

#[tokio::test]
async fn a_read_made_as_a_delegated_administrator_is_refused() {
    for (state, writes, expected) in [
        (
            "an organization nobody activated trusted access for",
            &[][..],
            DISABLED,
        ),
        (
            "an organization with trusted access activated",
            &[ACTIVATE][..],
            ENABLED,
        ),
    ] {
        let (router, engine) = router_with_engine();
        for action in writes {
            let case = format!("{state}: {action}");
            write_trusted_access(&case, &router, REGION, action).await;
        }

        for region in REGIONS {
            let case = format!("{state}, read as a delegated administrator in {region}");
            let (status_code, body) = describe(&router, region, Some(DELEGATED_ADMIN)).await;
            assert_eq!(
                status_code, 400,
                "case {case:?}: expected the read to be refused, got {status_code} {body}"
            );
            let code = xml_text(&body, "Code");
            assert_eq!(
                code.as_deref(),
                Some(INVALID_OPERATION),
                "case {case:?}: expected the modelled {INVALID_OPERATION}, got {code:?} in {body}"
            );
            let message = xml_text(&body, "Message");
            assert_eq!(
                message.as_deref(),
                Some(NOT_A_DELEGATED_ADMINISTRATOR),
                "case {case:?}: expected the message {NOT_A_DELEGATED_ADMINISTRATOR:?}, got \
                 {message:?}"
            );
            // A refusal reports no setting at all, rather than a status beside the error.
            let reported = xml_text(&body, "Status");
            assert!(
                reported.is_none(),
                "case {case:?}: expected no Status in a refusal, got {reported:?}"
            );
        }

        // The refusals wrote nothing, so the management account still reads back what its own
        // writes left.
        let held = store(&engine).lock().organizations_access.status();
        assert_eq!(
            held, expected,
            "case {state:?}: expected the refusals to leave the setting at {expected:?}, got \
             {held:?}"
        );
        let case = format!("{state}, read by the management account after the refusals");
        let reported = reported_status(&case, &router, REGION, None).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}
