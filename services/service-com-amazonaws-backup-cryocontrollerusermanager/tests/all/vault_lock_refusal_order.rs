// SPDX-License-Identifier: CC0-1.0

//! A vault-LOCK action decides ACCESS before EXISTENCE.
//!
//! The rule under test: AWS Backup authorizes a vault-lock action against the
//! named vault as the resource, so a vault-lock call whose vault is outside the
//! caller's visibility — the account the request is scoped to does not hold it,
//! because it was never created or it has been deleted — is refused with HTTP 403
//! and the error type `AccessDeniedException`, and the refusal discloses nothing
//! about whether that vault exists. It is the same rule the vault-scoped READS
//! follow (`vault_read_visibility.rs`), which is why both share one resolution
//! step in `state`.
//!
//! Every case drives real HTTP through the router, so what is asserted is the wire
//! behaviour a `backup` SDK client sees. The tables span both vault-lock actions,
//! each reason a vault can be invisible, several vault-name spellings (both ends of
//! the name-length bound and both punctuation characters it admits) and several
//! request bodies, plus the indistinguishability of the refusals, the positive
//! control (a visible vault still takes and drops its lock, read back through
//! `DescribeBackupVault`) and the parameter refusals, which must also answer the
//! same whichever vault they name.
//!
//! The harness is the one `vault_read_visibility.rs` publishes to this test crate.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::vault_read_visibility::{
    assert_denied, call, error_code, message, prepare, router, Answer, Setup, DENIED_MESSAGE,
};

/// The vault-lock actions, as the model binds them.
///
/// From `smithy_query(query="operation",
/// operation="PutBackupVaultLockConfiguration,DeleteBackupVaultLockConfiguration")`:
/// both are bound to `/backup-vaults/{BackupVaultName}/vault-lock` — `PUT` for the
/// put and `DELETE` for the delete — take `BackupVaultName` as an `@httpLabel`, and
/// model no `BackupVaultAccountId`. They are the whole vault-lock class.
const VAULT_LOCK_ACTIONS: &[(&str, &str)] = &[
    ("PutBackupVaultLockConfiguration", "PUT"),
    ("DeleteBackupVaultLockConfiguration", "DELETE"),
];

/// Request bodies a `PutBackupVaultLockConfiguration` can carry, all within the day
/// counts the model documents (`ChangeableForDays` 3..=36500, the retention bounds
/// 1..=36500): no members at all, one bound, both bounds, and a cooling-off span.
const LOCK_BODIES: &[(&str, &str)] = &[
    ("no members at all", "{}"),
    ("a minimum retention only", r#"{"MinRetentionDays":7}"#),
    ("a maximum retention only", r#"{"MaxRetentionDays":30}"#),
    (
        "both bounds and a cooling-off span",
        r#"{"MinRetentionDays":1,"MaxRetentionDays":36500,"ChangeableForDays":3}"#,
    ),
    (
        "the longest cooling-off span",
        r#"{"ChangeableForDays":36500}"#,
    ),
];

fn lock_path(vault: &str) -> String {
    format!("/backup-vaults/{vault}/vault-lock")
}

/// One vault-lock request: the `PUT` carries `body`, the `DELETE` carries none.
async fn vault_lock(
    router: &MultiServiceRouter,
    method: &str,
    vault: &str,
    body: &str,
) -> Answer {
    let payload = if method == "PUT" { Some(body) } else { None };
    call(router, method, &lock_path(vault), payload).await
}

/// `DescribeBackupVault`, the implemented reader a lock is proved through.
async fn describe(router: &MultiServiceRouter, vault: &str) -> Answer {
    call(router, "GET", &format!("/backup-vaults/{vault}"), None).await
}

/// Every vault-lock action naming a vault outside the caller's visibility is
/// refused with 403 `AccessDeniedException`, whatever the request asks for.
#[tokio::test]
async fn a_vault_lock_action_outside_the_caller_s_visibility_is_denied() {
    let long_name: String = std::iter::repeat_n('v', 50).collect();
    let invisible: Vec<(&str, Setup, String)> = vec![
        (
            "a vault that was never created",
            Setup::Absent,
            "never-created".to_string(),
        ),
        (
            "a two-character name that was never created",
            Setup::Absent,
            "ab".to_string(),
        ),
        (
            "a fifty-character name that was never created",
            Setup::Absent,
            long_name,
        ),
        (
            "a vault that was deleted",
            Setup::Deleted,
            "deleted-vault".to_string(),
        ),
        (
            "an under_scored name that was deleted",
            Setup::Deleted,
            "deleted_vault_1".to_string(),
        ),
        (
            "a MiXeD-case name that was deleted",
            Setup::Deleted,
            "MiXeD-Case_09".to_string(),
        ),
    ];

    for (reason, setup, vault) in &invisible {
        for (operation, method) in VAULT_LOCK_ACTIONS {
            for (body_case, body) in LOCK_BODIES {
                let case = format!("{operation} with {body_case} on {reason}");
                let router = router();
                prepare(&router, *setup, vault).await;
                let answer = vault_lock(&router, method, vault, body).await;
                assert_denied(&case, &answer, vault);
                // The DELETE takes no body, so running it once per body row would
                // only repeat the same request.
                if *method == "DELETE" {
                    break;
                }
            }
        }
    }
}

/// The refusal says the same thing whether or not the vault exists: a vault-lock
/// action on a deleted vault and on a name that was never created are the SAME
/// answer, down to the body, for both actions — and a denied action changes nothing
/// a later caller sees.
#[tokio::test]
async fn the_vault_lock_refusal_does_not_distinguish_a_deleted_vault_from_an_unknown_one() {
    for (operation, method) in VAULT_LOCK_ACTIONS {
        let router = router();
        prepare(&router, Setup::Deleted, "gone-vault").await;
        prepare(&router, Setup::Created, "live-vault").await;
        let before = describe(&router, "live-vault").await;
        assert_eq!(
            before.status, 200,
            "case {operation:?}: the live vault must read back, got {before:?}"
        );

        let deleted = vault_lock(&router, method, "gone-vault", "{}").await;
        let unknown = vault_lock(&router, method, "never-made", "{}").await;
        assert_eq!(
            deleted, unknown,
            "case {operation:?}: a deleted vault and an unknown name must answer identically, \
             got {deleted:?} and {unknown:?}"
        );
        assert_eq!(
            message(&deleted.body),
            DENIED_MESSAGE,
            "case {operation:?}: expected the fixed refusal message, got {deleted:?}"
        );

        let after = describe(&router, "live-vault").await;
        assert_eq!(
            after, before,
            "case {operation:?}: a denied vault-lock action must not change what a later \
             caller sees, got {after:?} after {before:?}"
        );
    }
}

/// One row of the positive-control table: what the put asks for, and what the vault
/// must then report.
#[derive(Debug, Clone, Copy)]
struct AppliedLock {
    case: &'static str,
    body: &'static str,
    min_days: Option<i64>,
    max_days: Option<i64>,
    lock_date: bool,
}

/// The positive control: a vault the caller CAN see takes its lock, reports it
/// through `DescribeBackupVault`, and drops it again.
#[tokio::test]
async fn a_visible_vault_still_takes_and_drops_its_lock() {
    let applied = [
        AppliedLock {
            case: "no members at all",
            body: "{}",
            min_days: None,
            max_days: None,
            lock_date: false,
        },
        AppliedLock {
            case: "both retention bounds",
            body: r#"{"MinRetentionDays":7,"MaxRetentionDays":30}"#,
            min_days: Some(7),
            max_days: Some(30),
            lock_date: false,
        },
        AppliedLock {
            case: "a cooling-off span and both bounds",
            body: r#"{"MinRetentionDays":1,"MaxRetentionDays":36500,"ChangeableForDays":3}"#,
            min_days: Some(1),
            max_days: Some(36500),
            lock_date: true,
        },
    ];

    for AppliedLock {
        case,
        body,
        min_days,
        max_days,
        lock_date,
    } in applied
    {
        let router = router();
        prepare(&router, Setup::Created, "visible-vault").await;

        let put = vault_lock(&router, "PUT", "visible-vault", body).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: the put must be accepted, got {put:?}"
        );
        let locked = describe(&router, "visible-vault").await;
        assert_eq!(
            locked.body.get("Locked").and_then(Value::as_bool),
            Some(true),
            "case {case:?}: expected the vault to read back Locked, got {:?}",
            locked.body
        );
        assert_eq!(
            (
                locked.body.get("MinRetentionDays").and_then(Value::as_i64),
                locked.body.get("MaxRetentionDays").and_then(Value::as_i64),
            ),
            (min_days, max_days),
            "case {case:?}: expected the bounds applied, got {:?}",
            locked.body
        );
        assert_eq!(
            locked.body.get("LockDate").is_some(),
            lock_date,
            "case {case:?}: expected LockDate present={lock_date}, got {:?}",
            locked.body
        );

        let dropped = vault_lock(&router, "DELETE", "visible-vault", "{}").await;
        assert_eq!(
            dropped.status, 200,
            "case {case:?}: the delete must be accepted, got {dropped:?}"
        );
        let unlocked = describe(&router, "visible-vault").await;
        assert_eq!(
            unlocked.body.get("Locked").and_then(Value::as_bool),
            Some(false),
            "case {case:?}: expected the vault unlocked again, got {:?}",
            unlocked.body
        );
        for member in ["MinRetentionDays", "MaxRetentionDays", "LockDate"] {
            assert!(
                unlocked.body.get(member).is_none(),
                "case {case:?}: expected {member} unset after the delete, got {:?}",
                unlocked.body
            );
        }
    }
}

/// A vault-lock request the service refuses on its PARAMETERS answers the same
/// whichever vault it names, so that refusal discloses no more than the
/// authorization one: the day counts are checked against the bounds the model
/// documents, which are built from the request alone.
///
/// The rows walk each day count past a published bound and the pair that cannot
/// hold together; each is sent to a vault that exists and to one that never did.
#[tokio::test]
async fn a_parameter_refusal_is_the_same_for_a_visible_and_an_invisible_vault() {
    let refused: Vec<(&str, &str, &str)> = vec![
        (
            "a cooling-off span below the 3-day minimum",
            r#"{"ChangeableForDays":2}"#,
            "ChangeableForDays",
        ),
        (
            "a cooling-off span past the 36500-day maximum",
            r#"{"ChangeableForDays":36501}"#,
            "ChangeableForDays",
        ),
        (
            "a minimum retention below 1 day",
            r#"{"MinRetentionDays":0}"#,
            "MinRetentionDays",
        ),
        (
            "a maximum retention past 36500 days",
            r#"{"MaxRetentionDays":36501}"#,
            "MaxRetentionDays",
        ),
        (
            "a minimum retention longer than the maximum",
            r#"{"MinRetentionDays":30,"MaxRetentionDays":7}"#,
            "MinRetentionDays",
        ),
    ];

    for (case, body, member) in refused {
        let router = router();
        prepare(&router, Setup::Created, "param-vault").await;

        let on_visible = vault_lock(&router, "PUT", "param-vault", body).await;
        let on_invisible = vault_lock(&router, "PUT", "never-created", body).await;
        assert_eq!(
            (on_visible.status, error_code(&on_visible.body).as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?}: expected 400 InvalidParameterValueException, got {on_visible:?}"
        );
        assert_eq!(
            on_visible, on_invisible,
            "case {case:?}: the parameter refusal must not depend on whether the vault \
             exists, got {on_visible:?} and {on_invisible:?}"
        );
        assert!(
            message(&on_visible.body).contains(member),
            "case {case:?}: expected the refusal to name {member}, got {:?}",
            on_visible.body
        );

        let described = describe(&router, "param-vault").await;
        assert_eq!(
            described.body.get("Locked").and_then(Value::as_bool),
            Some(false),
            "case {case:?}: a refused configuration must store nothing, got {:?}",
            described.body
        );
    }
}
