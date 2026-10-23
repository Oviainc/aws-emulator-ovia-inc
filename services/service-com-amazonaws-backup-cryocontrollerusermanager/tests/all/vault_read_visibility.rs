// SPDX-License-Identifier: CC0-1.0

//! A vault-scoped read decides ACCESS before EXISTENCE.
//!
//! The rule under test: a vault-scoped read whose named vault is outside the
//! caller's visibility — the account the request is scoped to does not hold it
//! (never created, or deleted), or the request addresses another account's vault
//! with `BackupVaultAccountId` — is refused with HTTP 403 and the error type
//! `AccessDeniedException`, and the refusal discloses nothing about whether the
//! vault exists.
//!
//! Every case drives real HTTP through the router, so what is asserted is the wire
//! behaviour a `backup` SDK client sees. The tables span the two vault-scoped reads
//! the model binds a vault name into the URI for, each reason a vault can be
//! invisible, several vault-name spellings and several account ids, plus the polled
//! form (a client that waits for the terminal refusal) and the positive control
//! (a visible vault still answers 200).
//!
//! The harness below (the router, the one-request `call` that also returns the
//! `x-amzn-errortype` header, the setup ladder and `assert_denied`) is
//! `pub(crate)`, so the sibling suite covering the same access-before-existence
//! rule for the vault-LOCK actions (`vault_lock_refusal_order.rs`) drives the same
//! wire path instead of growing a second copy of it.

use std::sync::Arc;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` naming this service's signing name (`backup`) — how the
/// router picks the service — and the region in its credential scope.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTONLY/20260101/us-west-2/backup/aws4_request, \
     SignedHeaders=host, Signature=00";

/// The one account id a generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// The message AWS Backup answers an authorization refusal with. A FIXED service
/// string, built from nothing in the request — which is why it can be asserted
/// verbatim and why it leaks nothing.
pub(crate) const DENIED_MESSAGE: &str = "Insufficient privileges to perform this action.";

/// The vault-scoped reads: the operation name and the path suffix that reaches it.
///
/// From the model (`smithy_query(query="operation",
/// operation="DescribeBackupVault,ListRecoveryPointsByBackupVault")`): both are GET
/// with `BackupVaultName` bound into the URI — `/backup-vaults/{BackupVaultName}`
/// and `/backup-vaults/{BackupVaultName}/recovery-points` — and both carry
/// `BackupVaultAccountId` as the `backupVaultAccountId` query parameter. Every other
/// operation that takes a vault name is either a mutation or still a 501 stub, so
/// these two are the whole class today.
const VAULT_READS: &[(&str, &str)] = &[
    ("DescribeBackupVault", ""),
    ("ListRecoveryPointsByBackupVault", "/recovery-points"),
];

/// What the store must hold before the read under test runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Setup {
    /// Nothing: the vault was never created.
    Absent,
    /// The vault exists (so only the account scoping can make it invisible).
    Created,
    /// The vault was created and then deleted.
    Deleted,
}

pub(crate) fn router() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_backup_cryocontrollerusermanager::EmulatedService::new(engine),
    );
    router
}

/// One request's status, `x-amzn-errortype` header and parsed JSON body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) error_type: Option<String>,
    pub(crate) body: Value,
}

pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://localhost{path}"))
        .header("authorization", AUTHORIZATION);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let payload = body.map_or_else(bytes::Bytes::new, |b| {
        bytes::Bytes::copy_from_slice(b.as_bytes())
    });
    let request = builder.body(payload).expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type,
        body,
    }
}

/// The wire error code in the body (`__type`), or `"<none>"`.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<none>")
        .to_string()
}

pub(crate) fn message(body: &Value) -> String {
    body.get("Message")
        .and_then(Value::as_str)
        .unwrap_or("<none>")
        .to_string()
}

/// The path of a vault-scoped read, with the `backupVaultAccountId` query parameter
/// when the case names an account.
fn read_path(suffix: &str, vault: &str, account: Option<&str>) -> String {
    match account {
        Some(account) => {
            format!("/backup-vaults/{vault}{suffix}?backupVaultAccountId={account}")
        }
        None => format!("/backup-vaults/{vault}{suffix}"),
    }
}

/// Put the store into `setup` for `vault`.
pub(crate) async fn prepare(router: &MultiServiceRouter, setup: Setup, vault: &str) {
    if setup == Setup::Absent {
        return;
    }
    let created = call(router, "PUT", &format!("/backup-vaults/{vault}"), Some("{}")).await;
    assert_eq!(
        created.status, 200,
        "setup {setup:?} for {vault:?}: CreateBackupVault must succeed, got {created:?}"
    );
    if setup == Setup::Deleted {
        let deleted = call(router, "DELETE", &format!("/backup-vaults/{vault}"), None).await;
        assert_eq!(
            deleted.status, 200,
            "setup {setup:?} for {vault:?}: DeleteBackupVault must succeed, got {deleted:?}"
        );
    }
}

/// Assert one answer is the authorization refusal, and that it discloses nothing.
pub(crate) fn assert_denied(case: &str, answer: &Answer, vault: &str) {
    assert_eq!(
        answer.status, 403,
        "case {case:?}: expected HTTP 403, got {} {answer:?}",
        answer.status
    );
    assert_eq!(
        answer.error_type.as_deref(),
        Some("AccessDeniedException"),
        "case {case:?}: expected x-amzn-errortype AccessDeniedException, got {:?}",
        answer.error_type
    );
    assert_eq!(
        error_code(&answer.body),
        "AccessDeniedException",
        "case {case:?}: expected the body's __type to be AccessDeniedException, got {:?}",
        answer.body
    );
    assert_eq!(
        message(&answer.body),
        DENIED_MESSAGE,
        "case {case:?}: expected the fixed refusal message, got {:?}",
        answer.body
    );
    let rendered = answer.body.to_string();
    assert!(
        !rendered.contains(vault),
        "case {case:?}: the refusal must not name the vault {vault:?}, got {rendered}"
    );
    assert!(
        !rendered.contains("NotFound"),
        "case {case:?}: the refusal must not report the vault as missing, got {rendered}"
    );
}

/// Every vault-scoped read of a vault outside the caller's visibility is refused
/// with 403 `AccessDeniedException`.
///
/// The rows differ along the reason the vault is invisible, the vault-name spelling
/// (both ends of the `^[a-zA-Z0-9\-\_]{2,50}$` length bound and both punctuation
/// characters it admits) and the account id addressed, and each row is run against
/// each read in [`VAULT_READS`].
#[tokio::test]
async fn a_vault_scoped_read_outside_the_caller_s_visibility_is_denied() {
    let long_name: String = std::iter::repeat_n('v', 50).collect();
    let invisible: Vec<(&str, Setup, String, Option<&str>)> = vec![
        (
            "a vault that was never created",
            Setup::Absent,
            "never-created".to_string(),
            None,
        ),
        (
            "a two-character name that was never created",
            Setup::Absent,
            "ab".to_string(),
            None,
        ),
        (
            "a fifty-character name that was never created",
            Setup::Absent,
            long_name.clone(),
            None,
        ),
        (
            "a vault that was deleted",
            Setup::Deleted,
            "deleted-vault".to_string(),
            None,
        ),
        (
            "an under_scored name that was deleted",
            Setup::Deleted,
            "deleted_vault_1".to_string(),
            None,
        ),
        (
            "a MiXeD-case name that was deleted",
            Setup::Deleted,
            "MiXeD-Case_09".to_string(),
            None,
        ),
        (
            "a stored vault addressed with another account",
            Setup::Created,
            "cross-account".to_string(),
            Some("111122223333"),
        ),
        (
            "a stored vault addressed with a different other account",
            Setup::Created,
            "cross-account".to_string(),
            Some("210987654321"),
        ),
        (
            "a deleted vault addressed with another account",
            Setup::Deleted,
            "deleted-vault".to_string(),
            Some("111122223333"),
        ),
        (
            "a never-created vault addressed with another account",
            Setup::Absent,
            "never-created".to_string(),
            Some("123456789012"),
        ),
    ];

    for (reason, setup, vault, account) in &invisible {
        for (operation, suffix) in VAULT_READS {
            let case = format!("{operation} on {reason}");
            let router = router();
            prepare(&router, *setup, vault).await;
            let answer = call(&router, "GET", &read_path(suffix, vault, *account), None).await;
            assert_denied(&case, &answer, vault);
        }
    }
}

/// The refusal says the same thing whether or not the vault exists: a read of a
/// deleted vault and a read of a name that was never created are the SAME answer,
/// down to the body. That indistinguishability is what stops a read disclosing a
/// vault the caller may not address.
#[tokio::test]
async fn the_refusal_does_not_distinguish_a_deleted_vault_from_an_unknown_one() {
    for (operation, suffix) in VAULT_READS {
        let router = router();
        prepare(&router, Setup::Deleted, "gone-vault").await;
        prepare(&router, Setup::Created, "live-vault").await;

        let deleted = call(&router, "GET", &read_path(suffix, "gone-vault", None), None).await;
        let unknown = call(&router, "GET", &read_path(suffix, "never-made", None), None).await;
        let cross = call(
            &router,
            "GET",
            &read_path(suffix, "live-vault", Some("111122223333")),
            None,
        )
        .await;

        assert_eq!(
            deleted, unknown,
            "case {operation:?}: a deleted vault and an unknown name must answer identically, \
             got {deleted:?} and {unknown:?}"
        );
        assert_eq!(
            deleted, cross,
            "case {operation:?}: a vault in another account must answer like a deleted one, \
             got {deleted:?} and {cross:?}"
        );
    }
}

/// A client polling a deleted vault converges: every attempt is the same terminal
/// 403, and no attempt changes what a later caller sees — the vault that IS visible
/// still reads back unchanged after the denied polls.
#[tokio::test]
async fn polling_a_deleted_vault_settles_on_the_same_refusal() {
    const POLLS: usize = 5;

    for (operation, suffix) in VAULT_READS {
        let router = router();
        prepare(&router, Setup::Deleted, "polled-vault").await;
        prepare(&router, Setup::Created, "kept-vault").await;
        let before = call(&router, "GET", &read_path(suffix, "kept-vault", None), None).await;
        assert_eq!(
            before.status, 200,
            "case {operation:?}: the kept vault must read back, got {before:?}"
        );

        let mut answers = Vec::with_capacity(POLLS);
        for attempt in 0..POLLS {
            let answer = call(&router, "GET", &read_path(suffix, "polled-vault", None), None).await;
            assert_denied(
                &format!("{operation} poll attempt {attempt}"),
                &answer,
                "polled-vault",
            );
            answers.push(answer);
        }
        assert!(
            answers.windows(2).all(|pair| pair[0] == pair[1]),
            "case {operation:?}: every poll must answer identically, got {answers:?}"
        );

        let after = call(&router, "GET", &read_path(suffix, "kept-vault", None), None).await;
        assert_eq!(
            after, before,
            "case {operation:?}: a denied read must not change what a later caller sees, \
             got {after:?} after {before:?}"
        );
    }
}

/// The positive control: a vault the caller CAN see still answers 200 — with no
/// `BackupVaultAccountId`, and with this emulator's own account id spelled out.
#[tokio::test]
async fn a_visible_vault_still_reads_back() {
    for (operation, suffix) in VAULT_READS {
        for (case, account) in [
            ("no BackupVaultAccountId", None),
            ("this emulator's own account id", Some(ACCOUNT)),
        ] {
            let router = router();
            prepare(&router, Setup::Created, "visible-vault").await;
            let answer = call(
                &router,
                "GET",
                &read_path(suffix, "visible-vault", account),
                None,
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {operation:?} with {case:?}: expected 200, got {answer:?}"
            );
            assert_eq!(
                answer.error_type, None,
                "case {operation:?} with {case:?}: a success carries no error type, got {answer:?}"
            );
        }
    }
}
