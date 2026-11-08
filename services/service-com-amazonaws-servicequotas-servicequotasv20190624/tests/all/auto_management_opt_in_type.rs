// SPDX-License-Identifier: CC0-1.0

//! Which `OptInType` values the Automatic Management writers REFUSE, end to end over
//! the wire.
//!
//! Every call here is a real `POST /` with `X-Amz-Target:
//! ServiceQuotasV20190624.<Op>` dispatched through the same `MultiServiceRouter` the
//! emulator serves with, over ONE shared engine. The harness is `quota_workflow.rs`'s
//! (`emulator`, `client`, `Client`) and the readers of a configuration and of a refusal
//! are `auto_management.rs`'s (`opt_in_type`, `scalars`, `configuration`, `refused`), so
//! this suite adds assertions and no fixtures of its own.
//!
//! The rule asserted, over a table of values generated from the model's own value list:
//! which values an enum-valued member accepts is decided by the ENUM the model
//! declares, so a value OUTSIDE it — a member mis-cased, misspelt, padded, the empty
//! string, or a mode the model never declared — is what gets refused, as the
//! `ValidationException` the request-validation front end answers a value the API's own
//! types cannot hold, and the refused call records nothing. It is a property of the
//! member, so BOTH writers are driven: the
//! FULL writer (`StartAutoManagement`) and the PARTIAL one (`UpdateAutoManagement`) over
//! a running configuration, which must keep the mode it was started on.
//!
//! The other side of the same rule — every member the enum DOES declare is served and
//! stored, the mode the member's prose note calls unavailable included, which the
//! behavioural check `service-quotas.auto-management-start-update-stop-lifecycle`
//! records — is `auto_management.rs`'s
//! `every_modeled_opt_in_type_is_stored_by_both_writers` and its lifecycle table.

use serde_json::{json, Value};

use service_com_amazonaws_servicequotas_servicequotasv20190624::ops::types::{
    OPT_IN_LEVEL_VALUES, OPT_IN_TYPE_VALUES,
};
use service_com_amazonaws_servicequotas_servicequotasv20190624::state::{
    OPT_IN_STATUS_DISABLED, OPT_IN_STATUS_ENABLED,
};

use super::auto_management::{configuration, opt_in_type, refused, scalars};
use crate::quota_workflow::{client, emulator, Client};

/// The only `OptInLevel` member the model declares, taken from the model's own value
/// list rather than spelled again here.
const ACCOUNT_LEVEL: &str = OPT_IN_LEVEL_VALUES[0];

/// Every modelled client fault is HTTP 400 on this awsJson service, whatever
/// `@httpError` its shape declares — see the `error_envelope` suite.
const CLIENT_FAULT: u16 = 400;

/// The writers of `OptInType`, each as `(operation, a request naming `requested`)`: the
/// FULL writer, which also requires the opt-in level, and the PARTIAL one, which may
/// name the member alone.
fn writes(requested: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "StartAutoManagement",
            json!({"OptInLevel": ACCOUNT_LEVEL, "OptInType": requested}),
        ),
        ("UpdateAutoManagement", json!({"OptInType": requested})),
    ]
}

/// Start Automatic Management on the mode the running rows begin from, so the PARTIAL
/// writer has a configuration to be applied to.
async fn start_running(caller: &Client, case: &str, mode: &str) {
    let started = caller
        .ok(
            "StartAutoManagement",
            json!({"OptInLevel": ACCOUNT_LEVEL, "OptInType": mode}),
        )
        .await;
    assert_eq!(
        started,
        json!({}),
        "case {case}: StartAutoManagement models no output members, got {started}"
    );
}

/// A value OUTSIDE the `OptInType` enum is what the member refuses — on BOTH writers,
/// with the `ValidationException` the request-validation front end answers a value the
/// enumerated type cannot hold, naming the member as the wire spells it and reporting
/// the value set it does admit — and the refused call records nothing: an account that
/// had nothing running still reports `DISABLED` with no mode, and a running
/// configuration keeps the mode it was started on.
///
/// The rows are GENERATED from the model's own value list, one near-miss of each form a
/// caller gets wrong — the member lower-cased, upper-cased, suffixed, padded with a
/// space — plus the empty string and a mode the model never declared, so the refusal is
/// a property of enum MEMBERSHIP rather than of any one spelling, and a member the model
/// gains is covered without this table being edited.
#[tokio::test]
async fn a_value_outside_the_opt_in_type_enum_is_refused_by_both_writers() {
    let mut rows: Vec<String> = vec![String::new(), "NotifySometimes".to_string()];
    for member in OPT_IN_TYPE_VALUES {
        rows.push(member.to_lowercase());
        rows.push(member.to_uppercase());
        rows.push(format!("{member}Plus"));
        rows.push(format!("{member} "));
    }

    // The mode a running configuration starts on, so an update row has something to
    // keep: the model's own first member, read through the shared checker.
    let running_mode = opt_in_type(OPT_IN_TYPE_VALUES[0]);

    for requested in rows {
        assert!(
            !OPT_IN_TYPE_VALUES.contains(&requested.as_str()),
            "case {requested:?}: a refusal row must be OUTSIDE the enum, which declares \
             {OPT_IN_TYPE_VALUES:?}"
        );

        for (operation, request) in writes(&requested) {
            let case = format!("{operation} {requested:?}");
            let engine = emulator();
            let caller = client(&engine, "us-east-1");

            let running = operation == "UpdateAutoManagement";
            if running {
                start_running(&caller, &case, running_mode).await;
            }

            let (status, code, message) = refused(&caller, operation, &request).await;
            assert_eq!(
                status, CLIENT_FAULT,
                "case {case}: expected {request} to be refused with HTTP {CLIENT_FAULT}, \
                 got {status}"
            );
            assert_eq!(
                code, "ValidationException",
                "case {case}: expected ValidationException, got {code}"
            );
            assert!(
                message.starts_with(
                    "1 validation error detected: Value at 'optInType' failed to satisfy \
                     constraint: Member must satisfy enum value set: ["
                ),
                "case {case}: the refusal must name the member that was wrong and the \
                 value set it admits, got {message:?}"
            );

            // The refusal recorded nothing: either the account still has no
            // configuration at all, or the one it had is untouched.
            let read = configuration(&caller, &case).await;
            let expected = if running {
                (
                    Some(ACCOUNT_LEVEL),
                    Some(running_mode),
                    Some(OPT_IN_STATUS_ENABLED),
                    None,
                )
            } else {
                (None, None, Some(OPT_IN_STATUS_DISABLED), None)
            };
            assert_eq!(
                scalars(&read),
                expected,
                "case {case}: a refused write must leave the configuration as it was, \
                 got {read}"
            );
        }
    }
}
