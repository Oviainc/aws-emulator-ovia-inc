// SPDX-License-Identifier: CC0-1.0

//! A legal hold's vault selection, and the refusals a legal hold id and a copy
//! job id the account does not hold earn, driven as real HTTP through the
//! router.
//!
//! The rules under test:
//!
//! * a recovery-point selection's `VaultNames` entries are plain strings — the
//!   model types the list's member as `string` with neither a `@pattern` nor a
//!   `@length` (`smithy_query(query="shape", shape="VaultNames")`), unlike the
//!   patterned `BackupVaultName` — so a create is accepted whatever a selection
//!   names, and the hold echoes and stores the selection as sent;
//! * the entry `*` names every vault rather than a vault, since the
//!   `BackupVaultName` pattern admits no `*` and so no vault can carry that
//!   name: a hold selected that way is on the stored recovery points of every
//!   vault, while one naming vaults is on those vaults' points alone;
//! * a legal hold id this account holds nothing under is ONE condition for every
//!   hold-scoped call: 400 `InvalidParameterValueException`, with the
//!   condition's own `Code` and a sentence that names no part of the request;
//! * a copy job id this account holds nothing under is 400
//!   `ResourceNotFoundException` with the condition's own `Code` and, again, a
//!   sentence that names no part of the request.
//!
//! The request harness (`router`, `router_with_engine`, `call`, `error_code`)
//! and the recovery-point setup (`start_point`) are the ones the sibling suites
//! in this test binary expose, so there is ONE wire path and ONE
//! recovery-point seam for the service's suites.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::recovery_point_deletion::{router_with_engine, start_point};
use super::vault_and_backup_job::{
    call, call_reported, create_vault, error_code, name_of_length, router, string_at, ACCOUNT,
    REGION,
};

/// The entry that selects every vault (`crate::state`'s
/// `VAULT_NAMES_WILDCARD`).
const EVERY_VAULT: &str = "*";

/// The `LegalHoldStatus` a fresh hold reports (`smithy_query` shape
/// `LegalHoldStatus`: `CREATING | ACTIVE | CANCELING | CANCELED`).
const STATUS_CREATING: &str = "CREATING";

/// The error shape, `Code` and sentence AWS answers a legal hold id the account
/// does not hold with (`crate::state`'s `ERROR_CODE_LEGAL_HOLD_NOT_FOUND` /
/// `MESSAGE_LEGAL_HOLD_NOT_FOUND`).
const INVALID_PARAMETER: &str = "InvalidParameterValueException";
const LEGAL_HOLD_NOT_FOUND_CODE: &str = "ERROR_4218";
const LEGAL_HOLD_NOT_FOUND_MESSAGE: &str = "Can not find Legal Hold ARN.";

/// The same three for a copy job id the account does not hold
/// (`crate::state`'s `ERROR_CODE_COPY_JOB_NOT_FOUND` /
/// `MESSAGE_COPY_JOB_NOT_FOUND`).
const NOT_FOUND: &str = "ResourceNotFoundException";
const COPY_JOB_NOT_FOUND_CODE: &str = "ERROR_21001";
const COPY_JOB_NOT_FOUND_MESSAGE: &str = "Copy job not found";

/// The vaults the coverage case stores recovery points in.
const FIRST_VAULT: &str = "selection-first";
const SECOND_VAULT: &str = "selection-second";

/// `POST /legal-holds` — `CreateLegalHold`'s HTTP binding.
async fn create_legal_hold(router: &MultiServiceRouter, request: &Value) -> (u16, Value) {
    call(router, "POST", "/legal-holds", Some(&request.to_string())).await
}

/// `GET /legal-holds/{LegalHoldId}` — `GetLegalHold`'s HTTP binding.
async fn get_legal_hold(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/legal-holds/{id}"), None).await
}

/// `GET /legal-holds/{LegalHoldId}/recovery-points` —
/// `ListRecoveryPointsByLegalHold`'s HTTP binding.
async fn held_points(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/legal-holds/{id}/recovery-points"),
        None,
    )
    .await
}

/// The `RecoveryPointArn`s a coverage page reports, as a set.
fn held_arns(page: &Value) -> BTreeSet<String> {
    page.get("RecoveryPoints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|point| string_at(point, "RecoveryPointArn"))
        .collect()
}

/// The `VaultNames` lists the acceptance table walks. The rows differ in the
/// length, the alphabet and the count of the entries, and in whether an entry
/// could ever be a stored vault's name — which is what "the model constrains
/// these entries not at all" generalises over. The `BackupVaultName` pattern
/// (`^[a-zA-Z0-9\-\_]{2,50}$`) is what rows 2 to 6 stand outside of, by the
/// alphabet, the floor and the ceiling in turn.
fn vault_names_rows() -> Vec<(&'static str, Value)> {
    vec![
        ("the entry that names every vault", json!([EVERY_VAULT])),
        (
            "every vault beside a named one",
            json!([EVERY_VAULT, "named-vault"]),
        ),
        ("an entry one character long", json!(["v"])),
        (
            "an entry carrying spaces and punctuation",
            json!(["not a vault name!"]),
        ),
        (
            "an entry longer than the vault-name ceiling",
            json!([name_of_length(51)]),
        ),
        ("an entry that is a prefix pattern", json!(["prod-*"])),
        ("a non-ASCII entry", json!(["b\u{00f3}veda-1"])),
        (
            "two entries this account does hold nothing under",
            json!(["first-vault", "second-vault"]),
        ),
        ("no entry at all", json!([])),
    ]
}

/// Whatever a selection's `VaultNames` names, the create is accepted and the
/// stored hold reports the selection as sent — on the create's own answer and
/// on the read of the hold it minted.
#[tokio::test]
async fn every_vault_names_entry_the_model_admits_is_accepted_and_echoed() {
    for (case, vault_names) in vault_names_rows() {
        // One router per row: each row's hold is the only one the account holds.
        let router = router();
        let selection = json!({"VaultNames": vault_names});
        let request = json!({
            "Title": "Selection",
            "Description": format!("a hold selected by {case}"),
            "RecoveryPointSelection": selection,
        });
        let (status, created) = create_legal_hold(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateLegalHold 200, got {status} {created:?}"
        );
        assert_eq!(
            created.get("RecoveryPointSelection"),
            Some(&selection),
            "case {case:?}: the create must echo the selection as sent, got {created:?}"
        );
        assert_eq!(
            string_at(&created, "Status"),
            STATUS_CREATING,
            "case {case:?}: a fresh hold is still being placed, got {created:?}"
        );
        for member in ["LegalHoldId", "LegalHoldArn"] {
            assert!(
                !string_at(&created, member).is_empty(),
                "case {case:?}: the create must mint {member}, got {created:?}"
            );
        }

        let id = string_at(&created, "LegalHoldId");
        let (status, read) = get_legal_hold(&router, &id).await;
        assert_eq!(
            (status, read.get("RecoveryPointSelection")),
            (200, Some(&selection)),
            "case {case:?}: the stored hold must report the selection as sent, got {status} {read:?}"
        );
    }
}

/// The entry that names every vault covers the stored recovery points of all of
/// them, while a selection naming vaults covers those vaults' points alone and
/// one naming a vault the account does not hold covers none.
#[tokio::test]
async fn a_selection_naming_every_vault_holds_the_recovery_points_of_all_of_them() {
    let (router, _engine) = router_with_engine();
    for vault in [FIRST_VAULT, SECOND_VAULT] {
        let (status, created) = create_vault(&router, vault, "{}").await;
        assert_eq!(
            status, 200,
            "setup: expected CreateBackupVault 200 for {vault:?}, got {status} {created:?}"
        );
    }
    let first = start_point(
        &router,
        FIRST_VAULT,
        &format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/ledger"),
        None,
    )
    .await;
    let second = start_point(
        &router,
        SECOND_VAULT,
        &format!("arn:aws:ec2:{REGION}:{ACCOUNT}:volume/vol-0abc"),
        None,
    )
    .await;

    let both: BTreeSet<String> = [first.clone(), second.clone()].into_iter().collect();
    let cases: Vec<(&str, Value, BTreeSet<String>)> = vec![
        ("every vault", json!([EVERY_VAULT]), both.clone()),
        (
            "every vault beside one named",
            json!([EVERY_VAULT, FIRST_VAULT]),
            both.clone(),
        ),
        (
            "the first vault alone",
            json!([FIRST_VAULT]),
            [first.clone()].into_iter().collect(),
        ),
        (
            "the second vault alone",
            json!([SECOND_VAULT]),
            [second.clone()].into_iter().collect(),
        ),
        (
            "both vaults named",
            json!([FIRST_VAULT, SECOND_VAULT]),
            both.clone(),
        ),
        (
            "a vault the account does not hold",
            json!(["no-such-vault"]),
            BTreeSet::new(),
        ),
    ];

    for (case, vault_names, expected) in cases {
        let (status, created) = create_legal_hold(
            &router,
            &json!({
                "Title": "Coverage",
                "Description": format!("a hold over {case}"),
                "RecoveryPointSelection": {"VaultNames": vault_names},
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateLegalHold 200, got {status} {created:?}"
        );
        let id = string_at(&created, "LegalHoldId");
        let (status, page) = held_points(&router, &id).await;
        let covered = held_arns(&page);
        assert_eq!(
            (status, &covered),
            (200, &expected),
            "case {case:?}: expected the hold to cover {expected:?}, got {status} {page:?}"
        );
    }
}

/// Every call that addresses a legal hold answers ONE refusal for an id the
/// account holds nothing under: the modelled parameter-value exception, the
/// condition's `Code`, and a sentence that interpolates no part of the request.
#[tokio::test]
async fn a_hold_scoped_call_refuses_an_unheld_id_with_the_conditions_token() {
    let router = router();
    // The control: the account holds exactly one hold, so every refusal below is
    // about the id it was given and not about an empty store.
    let (status, created) = create_legal_hold(
        &router,
        &json!({
            "Title": "Held",
            "Description": "the one hold this account holds",
            "RecoveryPointSelection": {"VaultNames": [EVERY_VAULT]},
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: expected CreateLegalHold 200, got {status} {created:?}"
    );
    let held = string_at(&created, "LegalHoldId");

    // The ids differ in form: one of no particular shape, one of the shape this
    // service mints, and the held id with a character changed.
    let mut neighbour: Vec<char> = held.chars().collect();
    let last = neighbour.len() - 1;
    neighbour[last] = if neighbour[last] == '0' { '1' } else { '0' };
    let neighbour: String = neighbour.into_iter().collect();
    let unheld = [
        "never-minted".to_string(),
        "00000000-0000-0000-0000-000000000000".to_string(),
        neighbour,
    ];

    for id in &unheld {
        // Each hold-scoped operation, by the HTTP binding the model gives it.
        let calls: Vec<(&str, &str, String)> = vec![
            ("GetLegalHold", "GET", format!("/legal-holds/{id}")),
            (
                "ListRecoveryPointsByLegalHold",
                "GET",
                format!("/legal-holds/{id}/recovery-points"),
            ),
            (
                "CancelLegalHold",
                "DELETE",
                format!("/legal-holds/{id}?cancelDescription=nothing-to-cancel"),
            ),
        ];
        for (operation, method, path) in calls {
            let case = format!("{operation} for {id:?}");
            let (status, errortype, refused) = call_reported(&router, method, &path, None).await;
            assert_eq!(
                (status, errortype.as_str(), error_code(&refused).as_str()),
                (400, INVALID_PARAMETER, INVALID_PARAMETER),
                "case {case:?}: expected 400 {INVALID_PARAMETER}, got {status} {refused:?}"
            );
            assert_eq!(
                (
                    string_at(&refused, "Code").as_str(),
                    string_at(&refused, "Message").as_str()
                ),
                (LEGAL_HOLD_NOT_FOUND_CODE, LEGAL_HOLD_NOT_FOUND_MESSAGE),
                "case {case:?}: expected the condition's token and sentence, got {refused:?}"
            );
            assert!(
                !string_at(&refused, "Message").contains(id.as_str()),
                "case {case:?}: the sentence names no part of the request, got {refused:?}"
            );
        }
    }

    // The control again: the id the account does hold still reads back.
    let (status, read) = get_legal_hold(&router, &held).await;
    assert_eq!(
        (status, read.get("LegalHoldArn")),
        (200, created.get("LegalHoldArn")),
        "the id the account holds reads back, got {status} {read:?}"
    );
}

/// A copy job id the account holds nothing under is refused with the modelled
/// not-found exception, the condition's `Code` and a sentence that interpolates
/// no part of the request.
#[tokio::test]
async fn describe_copy_job_refuses_an_unheld_id_with_the_conditions_token() {
    let router = router();
    // The ids differ in form: one of no particular shape, one of the shape a
    // job id takes, and one carrying characters a path has to escape nothing of.
    let unheld = [
        "never-started",
        "00000000-0000-0000-0000-000000000000",
        "copy-job-17",
    ];
    for id in unheld {
        let (status, errortype, refused) =
            call_reported(&router, "GET", &format!("/copy-jobs/{id}"), None).await;
        assert_eq!(
            (status, errortype.as_str(), error_code(&refused).as_str()),
            (400, NOT_FOUND, NOT_FOUND),
            "id {id:?}: expected 400 {NOT_FOUND}, got {status} {refused:?}"
        );
        assert_eq!(
            (
                string_at(&refused, "Code").as_str(),
                string_at(&refused, "Message").as_str()
            ),
            (COPY_JOB_NOT_FOUND_CODE, COPY_JOB_NOT_FOUND_MESSAGE),
            "id {id:?}: expected the condition's token and sentence, got {refused:?}"
        );
        assert!(
            !string_at(&refused, "Message").contains(id),
            "id {id:?}: the sentence names no part of the request, got {refused:?}"
        );
    }
}
