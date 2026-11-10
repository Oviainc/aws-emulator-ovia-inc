// SPDX-License-Identifier: CC0-1.0

//! `DisassociateBackupVaultMpaApprovalTeam`: multi-party approval is a facility
//! of ONE vault type, so the call is decided by the TYPE of the vault it names.
//!
//! The rules under test:
//!
//! * a vault whose type does not support multi-party approval supports no
//!   approval team at all, so the call is refused 400
//!   `InvalidParameterValueException` with this service's sentence for that
//!   condition, and the vault's MPA state is left exactly as it was — whichever
//!   unsupported type the vault is, and whether or not a team was ever named;
//! * on the one type that does support it, the association is cleared and the
//!   modelled 204 answered, so a vault whose team was just removed reads like
//!   one that never had a team;
//! * a vault outside the caller's visibility is refused for lack of privileges
//!   rather than told apart from one that does not exist — the decision every
//!   vault-scoped call shares.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `backup` SDK client sees; the harness is the shared one in
//! `vault_and_backup_job.rs`.

use std::thread::sleep;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::logically_air_gapped_vault::create_air_gapped;
use crate::vault_and_backup_job::{
    call, create_vault, error_code, name_of_length, router, string_at, ACCOUNT, PAST_SETTLE, REGION,
};

/// The sentence AWS states for "this vault's type does not support the
/// multi-party approval call that was made on it" — fixed text, naming no part
/// of the request.
const UNSUPPORTED_VAULT_TYPE: &str = "Operation not supported for the provided vault type";

/// The retention window the air-gapped vaults here are created with: the
/// documented floor of `MinRetentionDays` and a year.
const AIR_GAPPED_WINDOW: &str = r#"{"MinRetentionDays": 7, "MaxRetentionDays": 365}"#;

/// The ARN of an MPA approval team of this account, in the region the harness
/// signs for.
fn team_arn(team: &str) -> String {
    format!("arn:aws:mpa:{REGION}:{ACCOUNT}:approval-team/{team}")
}

/// `POST /backup-vaults/{BackupVaultName}/mpaApprovalTeam?delete`, as the model
/// binds this operation.
async fn disassociate_team(router: &MultiServiceRouter, vault: &str) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/backup-vaults/{vault}/mpaApprovalTeam?delete"),
        Some(&json!({"RequesterComment": "removing the team"}).to_string()),
    )
    .await
}

async fn associate_team(router: &MultiServiceRouter, vault: &str, team: &str) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/backup-vaults/{vault}/mpaApprovalTeam"),
        Some(&json!({"MpaApprovalTeamArn": team_arn(team)}).to_string()),
    )
    .await
}

/// `MpaApprovalTeamArn` as the vault read reports it, or `"<unset>"`.
async fn reported_team(router: &MultiServiceRouter, vault: &str) -> String {
    let (status, described) = call(router, "GET", &format!("/backup-vaults/{vault}"), None).await;
    assert_eq!(
        status, 200,
        "DescribeBackupVault {vault} must answer 200, got {status} {described:?}"
    );
    described
        .get("MpaApprovalTeamArn")
        .and_then(Value::as_str)
        .unwrap_or("<unset>")
        .to_string()
}

/// Which vault a row of the table is made of.
#[derive(Debug, Clone, Copy)]
enum VaultKind {
    /// A standard `BACKUP_VAULT`.
    Standard,
    /// A `LOGICALLY_AIR_GAPPED_BACKUP_VAULT`, the one type that supports
    /// multi-party approval.
    AirGapped,
    /// A `RESTORE_ACCESS_BACKUP_VAULT`, created against an air-gapped source.
    RestoreAccess,
}

/// Create the vault a row names and answer the name it is stored under.
async fn prepare(router: &MultiServiceRouter, kind: VaultKind, name: &str) -> String {
    match kind {
        VaultKind::Standard => {
            let (status, created) = create_vault(router, name, "{}").await;
            assert_eq!(
                status, 200,
                "setup: CreateBackupVault {name} should succeed, got {status} {created:?}"
            );
            name.to_string()
        }
        VaultKind::AirGapped => {
            let (status, created) = create_air_gapped(router, name, AIR_GAPPED_WINDOW).await;
            assert_eq!(
                status, 200,
                "setup: CreateLogicallyAirGappedBackupVault {name} should succeed, got {status} \
                 {created:?}"
            );
            sleep(PAST_SETTLE);
            name.to_string()
        }
        VaultKind::RestoreAccess => {
            let source = format!("{name}-source");
            let (status, created) = create_air_gapped(router, &source, AIR_GAPPED_WINDOW).await;
            assert_eq!(
                status, 200,
                "setup: the source vault {source} should be created, got {status} {created:?}"
            );
            let (status, associated) = associate_team(router, &source, "source-team").await;
            assert_eq!(
                status, 204,
                "setup: the source vault's team should associate, got {status} {associated:?}"
            );
            sleep(PAST_SETTLE);
            let (status, granted) = call(
                router,
                "PUT",
                "/restore-access-backup-vaults",
                Some(
                    &json!({
                        "BackupVaultName": name,
                        "SourceBackupVaultArn":
                            format!("arn:aws:backup:{REGION}:{ACCOUNT}:backup-vault:{source}"),
                        "RequesterComment": "restoring from the air-gapped vault",
                    })
                    .to_string(),
                ),
            )
            .await;
            assert_eq!(
                status, 200,
                "setup: CreateRestoreAccessBackupVault {name} should succeed, got {status} \
                 {granted:?}"
            );
            sleep(PAST_SETTLE);
            name.to_string()
        }
    }
}

/// One row: the case name, the kind of vault, the name to create it under, the
/// team to associate first (`None` for a vault that never names one), and
/// whether the vault's type supports multi-party approval.
type Row = (&'static str, VaultKind, String, Option<&'static str>, bool);

fn rows() -> Vec<Row> {
    vec![
        (
            "a standard vault that never named a team",
            VaultKind::Standard,
            "plain-vault".to_string(),
            None,
            false,
        ),
        (
            "a standard vault at the shortest admitted name",
            VaultKind::Standard,
            name_of_length(2),
            None,
            false,
        ),
        (
            "a standard vault at the longest admitted name",
            VaultKind::Standard,
            name_of_length(50),
            None,
            false,
        ),
        (
            "a restore access vault",
            VaultKind::RestoreAccess,
            "restore-access-vault".to_string(),
            None,
            false,
        ),
        (
            "an air-gapped vault with a team associated",
            VaultKind::AirGapped,
            "air-gapped-vault".to_string(),
            Some("air-gapped-team"),
            true,
        ),
        (
            "an air-gapped vault that never named a team",
            VaultKind::AirGapped,
            "air-gapped-bare".to_string(),
            None,
            true,
        ),
    ]
}

/// The disassociation is decided by the vault's TYPE: a type that supports no
/// multi-party approval is refused with the service's fixed sentence and keeps
/// whatever MPA state it had, while the type that does support it has the
/// association cleared and answers the modelled 204.
#[tokio::test]
async fn the_vault_type_decides_whether_an_approval_team_can_be_disassociated() {
    for (case, kind, name, team, supports_mpa) in rows() {
        let router = router();
        let vault = prepare(&router, kind, &name).await;
        if let Some(team) = team {
            let (status, associated) = associate_team(&router, &vault, team).await;
            assert_eq!(
                status, 204,
                "case {case:?}: setup AssociateBackupVaultMpaApprovalTeam should succeed, got \
                 {status} {associated:?}"
            );
            assert_eq!(
                reported_team(&router, &vault).await,
                team_arn(team),
                "case {case:?}: setup must leave the team associated"
            );
        }

        let (status, answered) = disassociate_team(&router, &vault).await;
        if supports_mpa {
            assert_eq!(
                (status, error_code(&answered).as_str()),
                (204, "<none>"),
                "case {case:?}: expected the modelled 204, got {status} {answered:?}"
            );
        } else {
            assert_eq!(
                (
                    status,
                    error_code(&answered).as_str(),
                    string_at(&answered, "Message").as_str()
                ),
                (400, "InvalidParameterValueException", UNSUPPORTED_VAULT_TYPE),
                "case {case:?}: expected the unsupported-vault-type refusal, got {status} \
                 {answered:?}"
            );
        }
        // Either way the vault now reports no approval team: the supported type
        // because the association was cleared, the unsupported ones because
        // they could never carry one.
        let reported = reported_team(&router, &vault).await;
        assert_eq!(
            reported, "<unset>",
            "case {case:?}: expected no MpaApprovalTeamArn after the call, got {reported}"
        );
    }
}

/// A refused disassociation changes nothing about the vault it named: the team
/// an air-gapped vault carries survives a refusal aimed at a standard vault
/// beside it, and a repeat of the refused call answers the same way.
#[tokio::test]
async fn a_refused_disassociation_leaves_every_vaults_association_untouched() {
    let router = router();
    prepare(&router, VaultKind::Standard, "plain-vault").await;
    prepare(&router, VaultKind::AirGapped, "air-gapped-vault").await;
    let (status, associated) = associate_team(&router, "air-gapped-vault", "kept-team").await;
    assert_eq!(
        status, 204,
        "setup: the air-gapped vault's team should associate, got {status} {associated:?}"
    );

    for attempt in 1..=2 {
        let (status, refused) = disassociate_team(&router, "plain-vault").await;
        assert_eq!(
            (
                status,
                error_code(&refused).as_str(),
                string_at(&refused, "Message").as_str()
            ),
            (400, "InvalidParameterValueException", UNSUPPORTED_VAULT_TYPE),
            "attempt {attempt}: expected the unsupported-vault-type refusal, got {status} \
             {refused:?}"
        );
        let kept = reported_team(&router, "air-gapped-vault").await;
        assert_eq!(
            kept,
            team_arn("kept-team"),
            "attempt {attempt}: the other vault's team must survive the refusal, got {kept}"
        );
    }

    // And the supported vault's own disassociation still works afterwards.
    let (status, answered) = disassociate_team(&router, "air-gapped-vault").await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (204, "<none>"),
        "the air-gapped vault's team must be removable, got {status} {answered:?}"
    );
    let reported = reported_team(&router, "air-gapped-vault").await;
    assert_eq!(
        reported, "<unset>",
        "expected the association cleared, got {reported}"
    );
}

/// A vault this account does not hold is refused for lack of privileges, not
/// told apart from one of an unsupported type: the access decision comes first,
/// so the answer discloses nothing about whether the vault exists.
#[tokio::test]
async fn a_vault_the_caller_cannot_see_is_refused_before_its_type_is_read() {
    let router = router();
    let cases = [
        ("a vault that was never created", "never-created"),
        ("a vault that was deleted", "deleted-vault"),
    ];
    let (status, created) = create_vault(&router, "deleted-vault", "{}").await;
    assert_eq!(
        status, 200,
        "setup: CreateBackupVault should succeed, got {status} {created:?}"
    );
    let (status, dropped) = call(&router, "DELETE", "/backup-vaults/deleted-vault", None).await;
    assert_eq!(
        status, 200,
        "setup: the empty vault should be deletable, got {status} {dropped:?}"
    );

    for (case, vault) in cases {
        let (status, refused) = disassociate_team(&router, vault).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (403, "AccessDeniedException"),
            "case {case:?}: expected the access refusal, got {status} {refused:?}"
        );
    }
}
