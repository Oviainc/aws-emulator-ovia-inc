// SPDX-License-Identifier: CC0-1.0

//! What an AWS Backup refusal SAYS when the request names something the call
//! cannot be served for: the member it blames, and the token a client branches
//! on.
//!
//! Every case drives real HTTP through the router (the harness is the one
//! `vault_and_backup_job` uses), so what is asserted is the body and the
//! `x-amzn-errortype` a `backup` SDK client parses. The rules under test, each
//! over a table rather than one request:
//!
//! * a member whose VALUE is not of the form that member requires is refused
//!   with `InvalidParameterValueException` carrying the class's `Code`
//!   (`INVALID_PARAMETER_VALUE`), the rejected member's name in `Context`, and
//!   the one-sentence `Parameter <MemberName> is invalid.` — on every operation
//!   and every member of that class, with the member name the only thing that
//!   varies between two of them;
//! * which values name an MPA approval team is the form the service that MINTS
//!   one publishes, so a value outside it is refused as that member and a value
//!   inside it is recorded on the vault and read back;
//! * a team can only be put in charge of a vault whose type supports
//!   multi-party approval, and the member that cannot be honoured is the one
//!   naming the team, so a vault of another type earns that same refusal;
//! * a vault-type precondition AWS states a token for carries that token and
//!   that sentence: `ERROR_4134` for the source vault of a restore access
//!   create, `ERROR_3936` for a call only a logically air-gapped vault answers;
//! * those refusals are not blanket answers — the same requests with values and
//!   vault types the rules admit are served.

use std::thread::sleep;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::logically_air_gapped_vault::create_air_gapped;
use crate::vault_and_backup_job::{
    call, call_reported, create_vault, error_code, router, string_at, ACCOUNT, PAST_SETTLE, REGION,
};

/// The error shape every refusal in this suite is serialized as.
const INVALID_PARAMETER: &str = "InvalidParameterValueException";

/// The `Code` AWS Backup fills in for "a member of the request carries a value
/// this service cannot accept": one token for the class, with the member named
/// in `Context`.
const INVALID_PARAMETER_VALUE: &str = "INVALID_PARAMETER_VALUE";

/// The retention window every air-gapped vault in this suite is created with:
/// the documented floor of `MinRetentionDays` and a year.
const WINDOW: &str = r#"{"MinRetentionDays": 7, "MaxRetentionDays": 365}"#;

/// An IAM role ARN a backup job may be started with.
fn role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/service-role/AWSBackupDefaultServiceRole")
}

/// A protected resource's ARN a backup job may name.
fn resource_arn() -> String {
    format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo")
}

/// The ARN of a vault of this account, in the region the harness signs for.
fn vault_arn(name: &str) -> String {
    format!("arn:aws:backup:{REGION}:{ACCOUNT}:backup-vault:{name}")
}

/// One row of the parameter-refusal table: the request to send and the member
/// its value belongs to.
struct Rejected {
    case: String,
    member: &'static str,
    method: &'static str,
    path: String,
    body: Option<String>,
}

/// Values that are not an ARN at all, walking each structural part of
/// `arn:<partition>:<service>:<region>:<account>:<resource>`. None of them
/// carries a space, so each travels in a query parameter as written.
const NOT_ARNS: [(&str, &str); 6] = [
    ("a bare word", "not-an-arn"),
    ("the scheme only", "arn:"),
    ("too few fields", "arn:aws:kms:us-west-2:000000000000"),
    ("an empty partition", "arn::kms:us-west-2:000000000000:key/abc"),
    ("an empty service", "arn:aws::us-west-2:000000000000:key/abc"),
    ("a URL, not an ARN", "https://example.invalid/key/abc"),
];

/// Values that are an ARN but not a BACKUP VAULT's, which is what a member
/// typed as a vault ARN requires: the form every such member's own doc gives
/// (`arn:aws:backup:us-east-1:123456789012:backup-vault:aBackupVault`).
fn not_vault_arns() -> Vec<(&'static str, String)> {
    vec![
        ("a bare word", "not-an-arn".to_string()),
        (
            "another service's ARN",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:backup-vault:source"),
        ),
        (
            "a backup ARN naming another resource",
            format!("arn:aws:backup:{REGION}:{ACCOUNT}:recovery-point:abcd"),
        ),
        (
            "a vault ARN with no name",
            format!("arn:aws:backup:{REGION}:{ACCOUNT}:backup-vault:"),
        ),
    ]
}

/// Every (operation, member) pair that takes a member typed as a plain `ARN`,
/// each sent `value`.
fn plain_arn_rows(case: &str, value: &str) -> Vec<Rejected> {
    let job = |member: &str| {
        let mut payload = json!({
            "BackupVaultName": "refusal-vault",
            "ResourceArn": resource_arn(),
            "IamRoleArn": role_arn(),
        });
        payload[member] = json!(value);
        payload.to_string()
    };
    vec![
        Rejected {
            case: format!("CreateBackupVault EncryptionKeyArn, {case}"),
            member: "EncryptionKeyArn",
            method: "PUT",
            path: "/backup-vaults/refusal-key-vault".to_string(),
            body: Some(json!({ "EncryptionKeyArn": value }).to_string()),
        },
        Rejected {
            case: format!("CreateLogicallyAirGappedBackupVault EncryptionKeyArn, {case}"),
            member: "EncryptionKeyArn",
            method: "PUT",
            path: "/logically-air-gapped-backup-vaults/refusal-gapped".to_string(),
            body: Some(
                json!({
                    "MinRetentionDays": 7,
                    "MaxRetentionDays": 365,
                    "EncryptionKeyArn": value,
                })
                .to_string(),
            ),
        },
        Rejected {
            case: format!("StartBackupJob ResourceArn, {case}"),
            member: "ResourceArn",
            method: "PUT",
            path: "/backup-jobs".to_string(),
            body: Some(job("ResourceArn")),
        },
        Rejected {
            case: format!("StartBackupJob IamRoleArn, {case}"),
            member: "IamRoleArn",
            method: "PUT",
            path: "/backup-jobs".to_string(),
            body: Some(job("IamRoleArn")),
        },
        Rejected {
            case: format!("ListRecoveryPointsByBackupVault ByResourceArn, {case}"),
            member: "ByResourceArn",
            method: "GET",
            path: format!("/backup-vaults/refusal-vault/recovery-points?resourceArn={value}"),
            body: None,
        },
        Rejected {
            case: format!("CreateLegalHold ResourceIdentifiers, {case}"),
            member: "ResourceIdentifiers",
            method: "POST",
            path: "/legal-holds".to_string(),
            body: Some(
                json!({
                    "Title": "hold",
                    "Description": "a resource is addressed by ARN",
                    "RecoveryPointSelection": { "ResourceIdentifiers": [value] },
                })
                .to_string(),
            ),
        },
    ]
}

/// Every (operation, member) pair that takes a member typed as the ARN of a
/// BACKUP VAULT, each sent `value`.
fn vault_arn_rows(case: &str, value: &str) -> Vec<Rejected> {
    vec![
        Rejected {
            case: format!("CreateRestoreAccessBackupVault SourceBackupVaultArn, {case}"),
            member: "SourceBackupVaultArn",
            method: "PUT",
            path: "/restore-access-backup-vaults".to_string(),
            body: Some(json!({ "SourceBackupVaultArn": value }).to_string()),
        },
        Rejected {
            case: format!("StartCopyJob DestinationBackupVaultArn, {case}"),
            member: "DestinationBackupVaultArn",
            method: "PUT",
            path: "/copy-jobs".to_string(),
            body: Some(
                json!({
                    "RecoveryPointArn":
                        format!("arn:aws:backup:{REGION}:{ACCOUNT}:recovery-point:abcd"),
                    "SourceBackupVaultName": "refusal-vault",
                    "DestinationBackupVaultArn": value,
                    "IamRoleArn": role_arn(),
                })
                .to_string(),
            ),
        },
    ]
}

/// The sentence AWS states for a member it will not accept, whose only variable
/// part is the member's own name.
fn parameter_sentence(member: &str) -> String {
    format!("Parameter {member} is invalid.")
}

/// Assert one row earns the class's refusal, naming its member.
async fn assert_parameter_refusal(router: &MultiServiceRouter, row: &Rejected) {
    let (status, errortype, refused) =
        call_reported(router, row.method, &row.path, row.body.as_deref()).await;
    let answered = (
        status,
        errortype.clone(),
        error_code(&refused),
        string_at(&refused, "Code"),
        string_at(&refused, "Context"),
        string_at(&refused, "Message"),
    );
    assert_eq!(
        answered,
        (
            400,
            INVALID_PARAMETER.to_string(),
            INVALID_PARAMETER.to_string(),
            INVALID_PARAMETER_VALUE.to_string(),
            row.member.to_string(),
            parameter_sentence(row.member),
        ),
        "case {:?}: expected 400 {INVALID_PARAMETER} with Code={INVALID_PARAMETER_VALUE}, \
         Context={:?} and the member's own sentence, got {status} {refused:?}",
        row.case,
        row.member
    );
}

/// A member whose value is not of the form that member requires is refused as
/// THAT member, with the class's token and sentence — on every operation and
/// member of the class.
///
/// The rows cross each (operation, member) pair with each way a value can fail
/// to be what the member names, so the member's name is the only part of the
/// answer that varies.
#[tokio::test]
async fn a_member_value_of_the_wrong_form_is_refused_as_that_member() {
    let router = router();
    let (status, created) = create_vault(&router, "refusal-vault", "{}").await;
    assert_eq!(
        status, 200,
        "setup: CreateBackupVault refusal-vault should succeed, got {status} {created:?}"
    );

    let mut rows: Vec<Rejected> = Vec::new();
    for (case, value) in NOT_ARNS {
        rows.extend(plain_arn_rows(case, value));
    }
    for (case, value) in not_vault_arns() {
        rows.extend(vault_arn_rows(case, &value));
    }
    for row in &rows {
        assert_parameter_refusal(&router, row).await;
    }

    // Non-vacuity: the same operations SERVE the same requests when every
    // member carries a value of the form it names.
    let key = format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/1111-2222");
    let (status, accepted) =
        create_vault(&router, "ok-vault", &json!({ "EncryptionKeyArn": key }).to_string()).await;
    assert_eq!(
        status, 200,
        "case \"a well-formed EncryptionKeyArn\": expected 200, got {status} {accepted:?}"
    );
    let job = json!({
        "BackupVaultName": "refusal-vault",
        "ResourceArn": resource_arn(),
        "IamRoleArn": role_arn(),
    })
    .to_string();
    let (status, started) = call(&router, "PUT", "/backup-jobs", Some(&job)).await;
    assert_eq!(
        status, 200,
        "case \"a well-formed job\": expected 200, got {status} {started:?}"
    );
}

/// Values that DO name an MPA approval team, from the form the service that
/// mints one publishes:
/// `^arn:aws(-[^:]+)?:mpa:[a-z0-9-]{1,20}:[0-9]{12}:approval-team/[a-zA-Z0-9._-]+$`
/// (`smithy_query(query="shape", shape="ApprovalTeamArn",
/// model_path="services/service-com-amazonaws-mpa-awsfluffycoreservice/smithy-model/com-amazonaws-mpa-awsfluffycoreservice.json")`).
///
/// The rows span what that form generalises over: the characters a team name may
/// hold, the published maximum team-name length (64) and the partition.
fn team_arns() -> Vec<(&'static str, String)> {
    vec![
        (
            "a plain team name",
            format!("arn:aws:mpa:{REGION}:{ACCOUNT}:approval-team/restore-access-team"),
        ),
        (
            "a team name with the dot and underscore the pattern admits",
            format!("arn:aws:mpa:{REGION}:{ACCOUNT}:approval-team/Team.Alpha_1"),
        ),
        (
            "a one-character team name",
            format!("arn:aws:mpa:{REGION}:{ACCOUNT}:approval-team/a"),
        ),
        (
            "a team name at the published maximum length",
            format!(
                "arn:aws:mpa:{REGION}:{ACCOUNT}:approval-team/{}",
                "n".repeat(64)
            ),
        ),
        (
            "another partition",
            format!("arn:aws-us-gov:mpa:us-gov-west-1:{ACCOUNT}:approval-team/gov-team"),
        ),
    ]
}

/// Values that do NOT name one: each row breaks exactly one part of that form.
fn not_team_arns() -> Vec<(&'static str, String)> {
    vec![
        ("a bare team name", "restore-access-team".to_string()),
        (
            "another service's ARN",
            format!("arn:aws:iam::{ACCOUNT}:role/approval-team"),
        ),
        (
            "an approval-team resource under another service",
            format!("arn:aws:backup:{REGION}:{ACCOUNT}:approval-team/team"),
        ),
        (
            "another resource type of the approval service",
            format!("arn:aws:mpa:{REGION}:{ACCOUNT}:identity-source/ssoins-0001"),
        ),
        (
            "an empty team name",
            format!("arn:aws:mpa:{REGION}:{ACCOUNT}:approval-team/"),
        ),
        (
            "a team name outside the admitted characters",
            format!("arn:aws:mpa:{REGION}:{ACCOUNT}:approval-team/team@1"),
        ),
        (
            "an account id that is not twelve digits",
            format!("arn:aws:mpa:{REGION}:00000000000:approval-team/team"),
        ),
        (
            "an account id that is not digits",
            format!("arn:aws:mpa:{REGION}:00000000000a:approval-team/team"),
        ),
        (
            "an upper-case region",
            format!("arn:aws:mpa:US-WEST-2:{ACCOUNT}:approval-team/team"),
        ),
        (
            "no region at all",
            format!("arn:aws:mpa::{ACCOUNT}:approval-team/team"),
        ),
        (
            "an empty partition",
            format!("arn::mpa:{REGION}:{ACCOUNT}:approval-team/team"),
        ),
    ]
}

/// `PUT /backup-vaults/{BackupVaultName}/mpaApprovalTeam`, as the model binds
/// `AssociateBackupVaultMpaApprovalTeam`.
async fn associate_team(
    router: &MultiServiceRouter,
    vault: &str,
    team_arn: &str,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/backup-vaults/{vault}/mpaApprovalTeam"),
        Some(&json!({ "MpaApprovalTeamArn": team_arn }).to_string()),
    )
    .await
}

/// Only a value that names an approval team is recorded on a vault, and a value
/// that does not is refused as the member that carries it.
///
/// Each accepted row round-trips through `DescribeBackupVault`, so what the
/// association stored is what a later read reports.
#[tokio::test]
async fn only_an_approval_team_arn_is_recorded_on_a_vault() {
    for (case, team_arn) in team_arns() {
        let router = router();
        let (status, created) = create_air_gapped(&router, "team-vault", WINDOW).await;
        assert_eq!(
            status, 200,
            "setup {case:?}: CreateLogicallyAirGappedBackupVault should succeed, got {status} \
             {created:?}"
        );
        let (status, associated) = associate_team(&router, "team-vault", &team_arn).await;
        assert_eq!(
            status, 204,
            "case {case:?} ({team_arn}): expected the modelled 204, got {status} {associated:?}"
        );
        let (status, described) = call(&router, "GET", "/backup-vaults/team-vault", None).await;
        assert_eq!(
            (status, string_at(&described, "MpaApprovalTeamArn")),
            (200, team_arn.clone()),
            "case {case:?}: the describe must report the team the association stored, got \
             {status} {described:?}"
        );
    }

    let router = router();
    let (status, created) = create_air_gapped(&router, "team-vault", WINDOW).await;
    assert_eq!(
        status, 200,
        "setup: CreateLogicallyAirGappedBackupVault should succeed, got {status} {created:?}"
    );
    for (case, value) in not_team_arns() {
        let row = Rejected {
            case: format!("AssociateBackupVaultMpaApprovalTeam, {case}"),
            member: "MpaApprovalTeamArn",
            method: "PUT",
            path: "/backup-vaults/team-vault/mpaApprovalTeam".to_string(),
            body: Some(json!({ "MpaApprovalTeamArn": value }).to_string()),
        };
        assert_parameter_refusal(&router, &row).await;
        // A refused association recorded nothing.
        let (status, described) = call(&router, "GET", "/backup-vaults/team-vault", None).await;
        assert_eq!(
            (status, described.get("MpaApprovalTeamArn").cloned()),
            (200, None),
            "case {case:?}: a refused association must record no team, got {status} \
             {described:?}"
        );
    }
}

/// A vault whose type supports no multi-party approval cannot be given a team,
/// and the member that cannot be honoured is the one naming it — the same answer
/// a value that is not a team ARN earns.
///
/// The rows span the vault types that support none: an ordinary vault and a
/// restore access vault (which is itself subject to the SOURCE vault's team).
#[tokio::test]
async fn a_vault_type_that_supports_no_approval_team_refuses_the_member() {
    let router = router();
    let (status, created) = create_air_gapped(&router, "gapped-source", WINDOW).await;
    assert_eq!(
        status, 200,
        "setup: the air-gapped source should be created, got {status} {created:?}"
    );
    let team = format!("arn:aws:mpa:{REGION}:{ACCOUNT}:approval-team/source-team");
    let (status, associated) = associate_team(&router, "gapped-source", &team).await;
    assert_eq!(
        status, 204,
        "setup: the source's association should succeed, got {status} {associated:?}"
    );
    let (status, plain) = create_vault(&router, "plain-vault", "{}").await;
    assert_eq!(
        status, 200,
        "setup: CreateBackupVault plain-vault should succeed, got {status} {plain:?}"
    );
    // A vault grants restore access only once the clock has settled its own
    // creation, so the create below waits that span out.
    sleep(PAST_SETTLE);
    let body = json!({
        "SourceBackupVaultArn": vault_arn("gapped-source"),
        "BackupVaultName": "access-vault",
    })
    .to_string();
    let (status, access) =
        call(&router, "PUT", "/restore-access-backup-vaults", Some(&body)).await;
    assert_eq!(
        status, 200,
        "setup: the restore access vault should be created, got {status} {access:?}"
    );

    for case in ["plain-vault", "access-vault"] {
        let row = Rejected {
            case: format!("AssociateBackupVaultMpaApprovalTeam on {case}"),
            member: "MpaApprovalTeamArn",
            method: "PUT",
            path: format!("/backup-vaults/{case}/mpaApprovalTeam"),
            body: Some(json!({ "MpaApprovalTeamArn": team }).to_string()),
        };
        assert_parameter_refusal(&router, &row).await;
        let (status, described) = call(&router, "GET", &format!("/backup-vaults/{case}"), None)
            .await;
        assert_eq!(
            (status, described.get("MpaApprovalTeamArn").cloned()),
            (200, None),
            "case {case:?}: a refused association must record no team, got {status} \
             {described:?}"
        );
    }
}

/// `GET /logically-air-gapped-backup-vaults/{BackupVaultName}/restore-access-backup-vaults`,
/// as the model binds `ListRestoreAccessBackupVaults`.
async fn list_restore_access(
    router: &MultiServiceRouter,
    vault: &str,
) -> (u16, String, Value) {
    call_reported(
        router,
        "GET",
        &format!("/logically-air-gapped-backup-vaults/{vault}/restore-access-backup-vaults"),
        None,
    )
    .await
}

/// A vault-type precondition AWS states a token for answers with THAT token and
/// THAT sentence: the restore access create's source vault and the listing that
/// only a logically air-gapped vault answers each carry their own.
///
/// The rows span the vault types each call refuses — an ordinary vault and a
/// restore access vault — and both calls are then made on an air-gapped vault,
/// which is the type each admits.
#[tokio::test]
async fn a_vault_type_precondition_carries_its_own_token_and_sentence() {
    let router = router();
    let (status, created) = create_air_gapped(&router, "gapped-source", WINDOW).await;
    assert_eq!(
        status, 200,
        "setup: the air-gapped source should be created, got {status} {created:?}"
    );
    let team = format!("arn:aws:mpa:{REGION}:{ACCOUNT}:approval-team/source-team");
    let (status, associated) = associate_team(&router, "gapped-source", &team).await;
    assert_eq!(
        status, 204,
        "setup: the source's association should succeed, got {status} {associated:?}"
    );
    let (status, plain) = create_vault(&router, "plain-vault", "{}").await;
    assert_eq!(
        status, 200,
        "setup: CreateBackupVault plain-vault should succeed, got {status} {plain:?}"
    );
    // A vault grants restore access only once the clock has settled its own
    // creation, so the creates below wait that span out.
    sleep(PAST_SETTLE);
    let body = json!({
        "SourceBackupVaultArn": vault_arn("gapped-source"),
        "BackupVaultName": "access-vault",
    })
    .to_string();
    let (status, access) =
        call(&router, "PUT", "/restore-access-backup-vaults", Some(&body)).await;
    assert_eq!(
        status, 200,
        "setup: the restore access vault should be created, got {status} {access:?}"
    );

    for case in ["plain-vault", "access-vault"] {
        // The create: the source vault's own condition, with its token.
        let body = json!({ "SourceBackupVaultArn": vault_arn(case) }).to_string();
        let (status, errortype, refused) =
            call_reported(&router, "PUT", "/restore-access-backup-vaults", Some(&body)).await;
        assert_eq!(
            (
                status,
                errortype,
                string_at(&refused, "Code"),
                string_at(&refused, "Message"),
            ),
            (
                400,
                INVALID_PARAMETER.to_string(),
                "ERROR_4134".to_string(),
                "Provided backup vault does not support this operation.".to_string(),
            ),
            "case {case:?} as the source: expected the source-vault condition's token and \
             sentence, got {status} {refused:?}"
        );

        // The listing: the route is scoped to air-gapped vaults, so a vault of
        // another type is refused for its TYPE rather than answered with the
        // empty page a vault that merely holds none gets.
        let (status, errortype, refused) = list_restore_access(&router, case).await;
        assert_eq!(
            (
                status,
                errortype,
                string_at(&refused, "Code"),
                string_at(&refused, "Message"),
                refused.get("RestoreAccessBackupVaults").cloned(),
            ),
            (
                400,
                INVALID_PARAMETER.to_string(),
                "ERROR_3936".to_string(),
                "The vault type is invalid. Accepted type is \
                 LOGICALLY_AIR_GAPPED_BACKUP_VAULT."
                    .to_string(),
                None,
            ),
            "case {case:?}: expected the vault-type condition's token and sentence with no \
             listing, got {status} {refused:?}"
        );
    }

    // Non-vacuity: the type each call admits is served. The listing reports the
    // restore access vault created above, and a create against the air-gapped
    // source succeeds.
    let (status, errortype, page) = list_restore_access(&router, "gapped-source").await;
    let listed = page
        .get("RestoreAccessBackupVaults")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        (status, errortype, listed.len()),
        (200, "<unset>".to_string(), 1),
        "case \"the air-gapped source\": expected its one restore access vault listed, got \
         {status} {page:?}"
    );
    let body = json!({
        "SourceBackupVaultArn": vault_arn("gapped-source"),
        "BackupVaultName": "second-access",
    })
    .to_string();
    let (status, granted) =
        call(&router, "PUT", "/restore-access-backup-vaults", Some(&body)).await;
    assert_eq!(
        status, 200,
        "case \"an air-gapped source\": expected the create served, got {status} {granted:?}"
    );

    // A vault this account does not hold is still decided on ACCESS first, so
    // the type refusal above discloses nothing about which vaults exist.
    let (status, _errortype, denied) = list_restore_access(&router, "absent-vault").await;
    assert_eq!(
        (status, error_code(&denied).as_str()),
        (403, "AccessDeniedException"),
        "case \"a vault this account does not hold\": expected 403 AccessDeniedException, got \
         {status} {denied:?}"
    );
}
