// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage for the restore-access slice of AWS Backup:
//! `CreateRestoreAccessBackupVault`, `ListRestoreAccessBackupVaults` and
//! `RevokeRestoreAccessBackupVault`.
//!
//! Every case drives real HTTP through the router (the harness is the one
//! `vault_and_backup_job` uses), so what is asserted is the wire behaviour a
//! `backup` SDK client sees. The rules under test:
//!
//! * a restore access backup vault is a STORED vault of type
//!   `RESTORE_ACCESS_BACKUP_VAULT` linked to the source vault it was created
//!   against, so the list of one source reports exactly the vaults created
//!   against that source — never another source's — and a describe of the
//!   stored name reports the same source link;
//! * a create whose output models a non-terminal `VaultState` reports
//!   `CREATING` first, and the clock alone moves the stored vault to
//!   `AVAILABLE`, which is when the access it asked for is approved;
//! * a page is `MaxResults` items plus a cursor to the next one;
//! * a revoke removes the stored vault, leaving the source vault and its
//!   recovery points untouched, so every later call naming it answers as it
//!   does for a vault that was never there — and an ARN that names none of the
//!   source's restore access vaults is refused as a parameter VALUE;
//! * a vault grants restore access only once it is itself ready to serve it and
//!   has an MPA approval team to approve it, so a source still being created
//!   and a source with no team are each refused;
//! * a source vault that cannot grant restore access — one of another type, one
//!   named by something that is not a backup-vault ARN, one this account does
//!   not hold — is refused with the modelled error for that class, and stores
//!   nothing.

use std::thread::sleep;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::logically_air_gapped_vault::create_air_gapped;
use crate::vault_and_backup_job::{
    call, create_vault, error_code, name_of_length, router, string_at, ACCOUNT, PAST_SETTLE, REGION,
};

/// `VaultType` of a restore access backup vault (model enum `VaultType`:
/// `BACKUP_VAULT | LOGICALLY_AIR_GAPPED_BACKUP_VAULT |
/// RESTORE_ACCESS_BACKUP_VAULT`).
const RESTORE_ACCESS: &str = "RESTORE_ACCESS_BACKUP_VAULT";

/// `VaultType` of the only vault type restore access can be granted on.
const AIR_GAPPED: &str = "LOGICALLY_AIR_GAPPED_BACKUP_VAULT";

/// The retention window every source vault in this suite is created with: the
/// documented floor of `MinRetentionDays` and a year.
const SOURCE_WINDOW: &str = r#"{"MinRetentionDays": 7, "MaxRetentionDays": 365}"#;

/// The ARN of the MPA approval team every source vault in this suite is
/// associated with: access to an air-gapped vault's recovery points is granted
/// "subject to MPA approval" (model doc for
/// `CreateRestoreAccessBackupVault`), so a source vault without a team can
/// grant none.
const TEAM_ARN: &str = "arn:aws:mpa:us-west-2:000000000000:approval-team/restore-access-team";

/// The ARN of a vault of this account, in the region the test harness signs for.
fn vault_arn(name: &str) -> String {
    format!("arn:aws:backup:{REGION}:{ACCOUNT}:backup-vault:{name}")
}

async fn associate_team(
    router: &MultiServiceRouter,
    vault: &str,
    team_arn: &str,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/backup-vaults/{vault}/mpaApprovalTeam"),
        Some(&json!({"MpaApprovalTeamArn": team_arn}).to_string()),
    )
    .await
}

/// Create the source vault, associate the approval team whose approval its
/// restore access is subject to, and wait out its settle span: a vault grants
/// restore access only once it is itself `AVAILABLE`, which the clock alone
/// makes it (see `a_source_vault_still_creating_cannot_grant_restore_access`).
async fn create_source(router: &MultiServiceRouter, name: &str) {
    let (status, created) = create_air_gapped(router, name, SOURCE_WINDOW).await;
    assert_eq!(
        status, 200,
        "setup: CreateLogicallyAirGappedBackupVault {name} should succeed, got {status} {created:?}"
    );
    let (status, associated) = associate_team(router, name, TEAM_ARN).await;
    assert_eq!(
        status, 204,
        "setup: AssociateBackupVaultMpaApprovalTeam {name} should succeed, got {status} \
         {associated:?}"
    );
    sleep(PAST_SETTLE);
}

async fn create_restore_access(router: &MultiServiceRouter, body: &str) -> (u16, Value) {
    call(router, "PUT", "/restore-access-backup-vaults", Some(body)).await
}

async fn list_restore_access(
    router: &MultiServiceRouter,
    source: &str,
    query: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/logically-air-gapped-backup-vaults/{source}/restore-access-backup-vaults{query}"),
        None,
    )
    .await
}

async fn revoke_restore_access(
    router: &MultiServiceRouter,
    source: &str,
    restore_access_arn: &str,
) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!(
            "/logically-air-gapped-backup-vaults/{source}/restore-access-backup-vaults/\
             {restore_access_arn}"
        ),
        None,
    )
    .await
}

/// The `RestoreAccessBackupVaults` entries of a page.
fn entries(page: &Value) -> Vec<Value> {
    page["RestoreAccessBackupVaults"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// One row of the create table.
#[derive(Debug, Clone)]
struct Case {
    label: &'static str,
    /// The `BackupVaultName` the create sends, or `None` to let the service
    /// mint one (the member is optional).
    name: Option<String>,
    tags: Option<Value>,
    creator_request_id: Option<&'static str>,
}

impl Case {
    fn body(&self, source_arn: &str) -> String {
        let mut body = json!({"SourceBackupVaultArn": source_arn});
        if let Some(name) = &self.name {
            body["BackupVaultName"] = json!(name);
        }
        if let Some(tags) = &self.tags {
            body["BackupVaultTags"] = tags.clone();
        }
        if let Some(token) = self.creator_request_id {
            body["CreatorRequestId"] = json!(token);
        }
        body["RequesterComment"] = json!("auditing a restore");
        body.to_string()
    }
}

/// The rows differ in whether the create names the vault or lets the service
/// mint a name, in the form of the name it does send (both punctuation
/// characters the `BackupVaultName` pattern admits, and both published ends of
/// its `{2,50}` length bound), in whether tags are sent and in whether the
/// request carries a `CreatorRequestId`.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "an explicit name",
            name: Some("restore-access-explicit".to_string()),
            tags: None,
            creator_request_id: None,
        },
        Case {
            label: "a name the service mints",
            name: None,
            tags: None,
            creator_request_id: Some("token-minted"),
        },
        Case {
            label: "mixed case, underscores, tags and a token",
            name: Some("MiXeD_Restore_09".to_string()),
            tags: Some(json!({"Env": "test", "Tier": "audit"})),
            creator_request_id: Some("token-tagged"),
        },
        Case {
            label: "a name at the pattern's lower bound",
            name: Some(name_of_length(2)),
            tags: None,
            creator_request_id: None,
        },
        Case {
            label: "a name at the pattern's upper bound",
            name: Some(name_of_length(50)),
            tags: Some(json!({"Team": "platform"})),
            creator_request_id: None,
        },
    ]
}

/// The whole lifecycle: a create stores the vault against its source, the list
/// and the describe both read it back, the clock settles it from `CREATING` to
/// `AVAILABLE` and records the approval that makes it usable, and a revoke
/// removes it while leaving the source vault alone.
#[tokio::test]
async fn a_restore_access_vault_is_stored_listed_settled_and_then_revoked() {
    let source = "air-gapped-source";
    let source_arn = vault_arn(source);

    for case in cases() {
        let router = router();
        create_source(&router, source).await;

        let (status, created) = create_restore_access(&router, &case.body(&source_arn)).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateRestoreAccessBackupVault 200, got {status} {created:?}",
            case.label
        );
        let name = string_at(&created, "RestoreAccessBackupVaultName");
        let arn = string_at(&created, "RestoreAccessBackupVaultArn");
        if let Some(asked) = &case.name {
            assert_eq!(
                &name, asked,
                "case {case:?}: expected the requested name echoed from state, got {created:?}"
            );
        } else {
            // A minted name has to be one a later request may send, so it must
            // satisfy the model's `BackupVaultName` pattern
            // (`^[a-zA-Z0-9\-\_]{2,50}$`).
            let admitted = (2..=50).contains(&name.chars().count())
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
            assert!(
                admitted,
                "case {case:?}: a minted name must satisfy the BackupVaultName pattern, \
                 got {name:?}"
            );
        }
        assert_eq!(
            arn,
            vault_arn(&name),
            "case {case:?}: expected the synthesized vault ARN, got {created:?}"
        );
        assert_eq!(
            created.get("VaultState").and_then(Value::as_str),
            Some("CREATING"),
            "case {case:?}: a create that models VaultState reports the non-terminal state \
             first, got {created:?}"
        );
        assert!(
            created.get("CreationDate").is_some(),
            "case {case:?}: expected a CreationDate, got {created:?}"
        );

        // The vault is listable straight away — that is the whole point of the
        // create having stored it — and still in the transient state, because
        // the settle span is non-zero.
        let (status, page) = list_restore_access(&router, source, "").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListRestoreAccessBackupVaults 200, got {status} {page:?}"
        );
        let listed = entries(&page);
        assert_eq!(
            listed.len(),
            1,
            "case {case:?}: expected the stored vault listed exactly once, got {listed:?}"
        );
        let entry = &listed[0];
        assert_eq!(
            (
                string_at(entry, "RestoreAccessBackupVaultArn"),
                entry.get("VaultState").and_then(Value::as_str),
                entry.get("CreationDate").cloned(),
                entry.get("ApprovalDate").cloned(),
            ),
            (
                arn.clone(),
                Some("CREATING"),
                created.get("CreationDate").cloned(),
                None,
            ),
            "case {case:?}: the page must report the stored vault, with no approval yet, \
             got {entry:?}"
        );
        assert!(
            page.get("NextToken").is_none(),
            "case {case:?}: one vault is one page, so no cursor is due, got {page:?}"
        );

        // The same stored vault read back through the vault describe: the type
        // the create fixed and the source link it recorded.
        let (status, described) = call(&router, "GET", &format!("/backup-vaults/{name}"), None).await;
        assert_eq!(
            (
                status,
                described.get("VaultType").and_then(Value::as_str),
                string_at(&described, "SourceBackupVaultArn"),
                described.get("VaultState").and_then(Value::as_str),
                described.get("CreationDate").cloned(),
            ),
            (
                200,
                Some(RESTORE_ACCESS),
                source_arn.clone(),
                Some("CREATING"),
                created.get("CreationDate").cloned(),
            ),
            "case {case:?}: the describe must report the stored restore access vault, \
             got {status} {described:?}"
        );

        // Nothing but the clock moves the state, and the approval that makes
        // the vault usable is dated by that same transition.
        sleep(PAST_SETTLE);
        let (status, page) = list_restore_access(&router, source, "").await;
        let settled = entries(&page);
        assert_eq!(
            (
                status,
                settled.len(),
                settled
                    .first()
                    .and_then(|entry| entry.get("VaultState"))
                    .and_then(Value::as_str),
            ),
            (200, 1, Some("AVAILABLE")),
            "case {case:?}: the clock owes the vault AVAILABLE, got {status} {page:?}"
        );
        assert!(
            settled[0].get("ApprovalDate").is_some(),
            "case {case:?}: an available restore access vault reports when its access was \
             approved, got {:?}",
            settled[0]
        );
        assert_eq!(
            settled[0].get("CreationDate").cloned(),
            created.get("CreationDate").cloned(),
            "case {case:?}: settling must not restate the creation instant, got {:?}",
            settled[0]
        );

        let (status, revoked) = revoke_restore_access(&router, source, &arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected RevokeRestoreAccessBackupVault 200, got {status} {revoked:?}"
        );
        let (status, page) = list_restore_access(&router, source, "").await;
        assert_eq!(
            (status, entries(&page).len()),
            (200, 0),
            "case {case:?}: a revoked vault is no longer listed, got {status} {page:?}"
        );
        let (status, gone) = call(&router, "GET", &format!("/backup-vaults/{name}"), None).await;
        assert_eq!(
            (status, error_code(&gone).as_str()),
            (403, "AccessDeniedException"),
            "case {case:?}: a revoked vault is outside the caller's visibility, so the describe \
             is denied rather than answered, got {status} {gone:?}"
        );
        // Revoking withdraws the access, never the source vault.
        let (status, source_described) =
            call(&router, "GET", &format!("/backup-vaults/{source}"), None).await;
        assert_eq!(
            (
                status,
                source_described.get("VaultType").and_then(Value::as_str),
            ),
            (200, Some(AIR_GAPPED)),
            "case {case:?}: the source vault must survive the revoke, got {status} \
             {source_described:?}"
        );
        // And a second revoke of the same ARN finds nothing to revoke: the ARN
        // now names no restore access vault of this source, which AWS Backup
        // reports as a parameter value it cannot accept.
        let (status, again) = revoke_restore_access(&router, source, &arn).await;
        assert_eq!(
            (status, error_code(&again).as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?}: a repeat revoke must refuse the ARN, got {status} \
             {again:?}"
        );
        assert_eq!(
            again.get("Message").and_then(Value::as_str),
            Some("The provided restore access backup vault ARN is not valid."),
            "case {case:?}: expected the condition's own sentence, got {again:?}"
        );
    }
}

/// The page of one source vault holds exactly the restore access vaults created
/// against THAT source, in name order, cut into `MaxResults`-sized pages with a
/// cursor to the next one.
///
/// The rows walk the page size over the count stored, so the same stored set is
/// paged whole, in halves and one at a time.
#[tokio::test]
async fn the_page_spans_exactly_the_restore_access_vaults_of_the_named_source() {
    let router = router();
    create_source(&router, "source-one").await;
    create_source(&router, "source-two").await;

    let one_arn = vault_arn("source-one");
    let mut expected: Vec<String> = Vec::new();
    for name in ["access-a", "access-b", "access-c"] {
        let body = json!({"SourceBackupVaultArn": one_arn, "BackupVaultName": name}).to_string();
        let (status, created) = create_restore_access(&router, &body).await;
        assert_eq!(
            status, 200,
            "setup {name}: expected 200, got {status} {created:?}"
        );
        expected.push(string_at(&created, "RestoreAccessBackupVaultArn"));
    }
    let other_body = json!({
        "SourceBackupVaultArn": vault_arn("source-two"),
        "BackupVaultName": "access-other",
    })
    .to_string();
    let (status, other) = create_restore_access(&router, &other_body).await;
    assert_eq!(
        status, 200,
        "setup access-other: expected 200, got {status} {other:?}"
    );

    // Each source reports only its own.
    let (status, page) = list_restore_access(&router, "source-one", "").await;
    let arns: Vec<String> = entries(&page)
        .iter()
        .map(|entry| string_at(entry, "RestoreAccessBackupVaultArn"))
        .collect();
    assert_eq!(
        (status, arns.clone()),
        (200, expected.clone()),
        "case \"source-one, one page\": expected its own three vaults in name order, got \
         {status} {page:?}"
    );
    let (status, page) = list_restore_access(&router, "source-two", "").await;
    assert_eq!(
        (
            status,
            entries(&page)
                .iter()
                .map(|entry| string_at(entry, "RestoreAccessBackupVaultArn"))
                .collect::<Vec<String>>(),
        ),
        (
            200,
            vec![string_at(&other, "RestoreAccessBackupVaultArn")],
        ),
        "case \"source-two\": expected only the vault created against it, got {status} {page:?}"
    );

    // The same stored set, paged. Each row is a page size; the walk must yield
    // the whole set in the same order, and only the last page omits the cursor.
    for page_size in [1_usize, 2, 3, 4] {
        let mut walked: Vec<String> = Vec::new();
        let mut query = format!("?maxResults={page_size}");
        loop {
            let (status, page) = list_restore_access(&router, "source-one", &query).await;
            assert_eq!(
                status, 200,
                "case \"maxResults={page_size}\": expected 200, got {status} {page:?}"
            );
            let listed = entries(&page);
            assert!(
                listed.len() <= page_size,
                "case \"maxResults={page_size}\": a page must hold at most the page size, \
                 got {listed:?}"
            );
            walked.extend(
                listed
                    .iter()
                    .map(|entry| string_at(entry, "RestoreAccessBackupVaultArn")),
            );
            match page.get("NextToken").and_then(Value::as_str) {
                Some(token) => query = format!("?maxResults={page_size}&nextToken={token}"),
                None => break,
            }
        }
        assert_eq!(
            walked, expected,
            "case \"maxResults={page_size}\": the walk must yield every stored vault once, \
             in name order"
        );
    }

    // The published ends of the model's `@range(1..1000)` for `MaxResults`, and
    // one past each.
    for (label, value, expected_status) in [
        ("at the published minimum", 1, 200),
        ("at the published maximum", 1000, 200),
        ("below the published minimum", 0, 400),
        ("above the published maximum", 1001, 400),
    ] {
        let (status, body) =
            list_restore_access(&router, "source-one", &format!("?maxResults={value}")).await;
        assert_eq!(
            status, expected_status,
            "case {label:?} (maxResults={value}): expected {expected_status}, got {status} {body:?}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body).as_str(),
                "InvalidParameterValueException",
                "case {label:?} (maxResults={value}): expected the modelled page-size refusal, \
                 got {body:?}"
            );
        }
    }

    // A cursor this service did not mint names no page of the result set.
    let (status, refused) = list_restore_access(&router, "source-one", "?nextToken=zzzz").await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "InvalidParameterValueException"),
        "case \"a foreign cursor\": expected 400 InvalidParameterValueException, got {status} \
         {refused:?}"
    );
}

/// A vault grants restore access only once it is ready to serve it: while the
/// source vault is still `CREATING` the create is refused for the STATE, and
/// the same request succeeds once the clock has settled the source.
///
/// The rows differ in what the create sends besides the source, so the state
/// guard holds whatever else the request carries.
#[tokio::test]
async fn a_source_vault_still_creating_cannot_grant_restore_access() {
    let bodies: Vec<(&str, String)> = vec![
        (
            "the source alone",
            json!({"SourceBackupVaultArn": vault_arn("pending-source")}).to_string(),
        ),
        (
            "a name and a token",
            json!({
                "SourceBackupVaultArn": vault_arn("pending-source"),
                "BackupVaultName": "pending-access",
                "CreatorRequestId": "token-pending",
            })
            .to_string(),
        ),
    ];

    for (label, body) in bodies {
        let router = router();
        let (status, created) =
            create_air_gapped(&router, "pending-source", SOURCE_WINDOW).await;
        assert_eq!(
            (status, created.get("VaultState").and_then(Value::as_str)),
            (200, Some("CREATING")),
            "setup {label:?}: expected a source vault in the transient state, got {status} \
             {created:?}"
        );
        let (status, associated) = associate_team(&router, "pending-source", TEAM_ARN).await;
        assert_eq!(
            status, 204,
            "setup {label:?}: AssociateBackupVaultMpaApprovalTeam should succeed, got {status} \
             {associated:?}"
        );

        let (status, refused) = create_restore_access(&router, &body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "InvalidRequestException"),
            "case {label:?}: a CREATING source must refuse for its state, got {status} {refused:?}"
        );
        let (status, page) = list_restore_access(&router, "pending-source", "").await;
        assert_eq!(
            (status, entries(&page).len()),
            (200, 0),
            "case {label:?}: a refused create must store nothing, got {status} {page:?}"
        );

        // Nothing but the clock moves the source's state, and then the very
        // same request is granted.
        sleep(PAST_SETTLE);
        let (status, granted) = create_restore_access(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {label:?}: an AVAILABLE source must grant restore access, got {status} \
             {granted:?}"
        );
        let (status, page) = list_restore_access(&router, "pending-source", "").await;
        assert_eq!(
            (
                status,
                entries(&page)
                    .iter()
                    .map(|entry| string_at(entry, "RestoreAccessBackupVaultArn"))
                    .collect::<Vec<String>>(),
            ),
            (
                200,
                vec![string_at(&granted, "RestoreAccessBackupVaultArn")],
            ),
            "case {label:?}: expected the granted vault listed, got {status} {page:?}"
        );
    }
}

/// Restore access is granted subject to MPA approval, so the source vault needs
/// the approval team that grants it: without one the create is refused, and the
/// very same request succeeds once a team is associated. The association itself
/// round-trips through the vault describe, and a vault type that supports no
/// team at all refuses it.
///
/// The rows differ in the team ARN sent — the association takes any ARN, so
/// they span two different team names — and in the vault type it is sent for.
#[tokio::test]
async fn a_source_vault_without_an_mpa_approval_team_can_grant_no_restore_access() {
    let teams: Vec<(&str, &str)> = vec![
        ("the suite's team", TEAM_ARN),
        (
            "another team of this account",
            "arn:aws:mpa:us-west-2:000000000000:approval-team/second-team",
        ),
    ];

    for (label, team_arn) in teams {
        let router = router();
        let (status, created) = create_air_gapped(&router, "team-source", SOURCE_WINDOW).await;
        assert_eq!(
            status, 200,
            "setup {label:?}: expected the source vault created, got {status} {created:?}"
        );
        sleep(PAST_SETTLE);

        let body = json!({"SourceBackupVaultArn": vault_arn("team-source")}).to_string();
        let (status, refused) = create_restore_access(&router, &body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "InvalidRequestException"),
            "case {label:?}: a source with no approval team must refuse the create, got \
             {status} {refused:?}"
        );
        assert_eq!(
            refused.get("Message").and_then(Value::as_str),
            Some(
                "Cannot create restore access backup vault. No MPA approval team associated \
                 with the backup vault."
            ),
            "case {label:?}: expected the condition's own sentence, got {refused:?}"
        );
        let (status, page) = list_restore_access(&router, "team-source", "").await;
        assert_eq!(
            (status, entries(&page).len()),
            (200, 0),
            "case {label:?}: a refused create must store nothing, got {status} {page:?}"
        );

        let (status, associated) = associate_team(&router, "team-source", team_arn).await;
        assert_eq!(
            status, 204,
            "case {label:?}: expected the modelled 204 from the association, got {status} \
             {associated:?}"
        );
        // The association is stored on the vault, so the vault describe reports
        // the team it is subject to.
        let (status, described) =
            call(&router, "GET", "/backup-vaults/team-source", None).await;
        assert_eq!(
            (status, string_at(&described, "MpaApprovalTeamArn")),
            (200, team_arn.to_string()),
            "case {label:?}: the describe must report the associated team, got {status} \
             {described:?}"
        );

        let (status, granted) = create_restore_access(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {label:?}: a source with an approval team must grant restore access, got \
             {status} {granted:?}"
        );
    }

    // A team ARN that is not an ARN, and a vault type that supports no team at
    // all, are each refused and record nothing.
    let router = router();
    create_source(&router, "assoc-source").await;
    let (status, plain) = create_vault(&router, "assoc-plain", "{}").await;
    assert_eq!(
        status, 200,
        "setup: CreateBackupVault assoc-plain should succeed, got {status} {plain:?}"
    );
    let refusals: Vec<(&str, &str, &str)> = vec![
        ("a team ARN that is not an ARN", "assoc-source", "team-1"),
        (
            "a vault type that supports no approval team",
            "assoc-plain",
            TEAM_ARN,
        ),
    ];
    for (label, vault, team_arn) in refusals {
        let (status, refused) = associate_team(&router, vault, team_arn).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "InvalidParameterValueException"),
            "case {label:?}: expected 400 InvalidParameterValueException, got {status} \
             {refused:?}"
        );
    }
    let (status, described) = call(&router, "GET", "/backup-vaults/assoc-plain", None).await;
    assert_eq!(
        (status, described.get("MpaApprovalTeamArn").cloned()),
        (200, None),
        "case \"after the refused associations\": no team may be recorded, got {status} \
         {described:?}"
    );

    // An association replaces the one before it: a vault is subject to one team.
    let (status, replaced) = associate_team(
        &router,
        "assoc-source",
        "arn:aws:mpa:us-west-2:000000000000:approval-team/replacement",
    )
    .await;
    assert_eq!(
        status, 204,
        "case \"replacing the team\": expected 204, got {status} {replaced:?}"
    );
    let (status, described) = call(&router, "GET", "/backup-vaults/assoc-source", None).await;
    assert_eq!(
        (status, string_at(&described, "MpaApprovalTeamArn")),
        (
            200,
            "arn:aws:mpa:us-west-2:000000000000:approval-team/replacement".to_string()
        ),
        "case \"replacing the team\": expected the team last associated, got {status} \
         {described:?}"
    );
}

/// A source vault that cannot grant restore access is refused with the modelled
/// error for its class, and stores nothing.
///
/// The rows span the classes: a value that is not a backup-vault ARN at all (a
/// bare word, another service's ARN, an ARN with no vault resource), an ARN
/// naming a vault this account does not hold (never created, another account's)
/// and an ARN naming a vault of a type that grants no restore access.
#[tokio::test]
async fn a_source_vault_that_cannot_grant_restore_access_is_refused() {
    let router = router();
    create_source(&router, "grantor-vault").await;
    let (status, plain) = create_vault(&router, "plain-vault", "{}").await;
    assert_eq!(
        status, 200,
        "setup: CreateBackupVault plain-vault should succeed, got {status} {plain:?}"
    );

    let cases: Vec<(&str, String, &str)> = vec![
        (
            "not an ARN at all",
            "grantor-vault".to_string(),
            "InvalidParameterValueException",
        ),
        (
            "another service's ARN",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:backup-vault:grantor-vault"),
            "InvalidParameterValueException",
        ),
        (
            "a backup ARN naming no vault",
            format!("arn:aws:backup:{REGION}:{ACCOUNT}:recovery-point:abcd"),
            "InvalidParameterValueException",
        ),
        (
            "a vault this account never created",
            vault_arn("absent-vault"),
            "ResourceNotFoundException",
        ),
        (
            "another account's vault",
            format!("arn:aws:backup:{REGION}:999999999999:backup-vault:grantor-vault"),
            "ResourceNotFoundException",
        ),
        (
            "a vault of a type that grants no restore access",
            vault_arn("plain-vault"),
            "InvalidParameterValueException",
        ),
    ];

    for (label, source_arn, expected_code) in cases {
        let body = json!({"SourceBackupVaultArn": source_arn}).to_string();
        let (status, refused) = create_restore_access(&router, &body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, expected_code),
            "case {label:?} ({source_arn}): expected 400 {expected_code}, got {status} {refused:?}"
        );
        assert!(
            refused.get("Message").and_then(Value::as_str).is_some(),
            "case {label:?} ({source_arn}): the refusal must state the condition, got {refused:?}"
        );
    }
    // Nothing was stored against either vault by any of those refusals.
    for source in ["grantor-vault", "plain-vault"] {
        let (status, page) = list_restore_access(&router, source, "").await;
        assert_eq!(
            (status, entries(&page).len()),
            (200, 0),
            "case \"after the refusals, {source}\": a refused create must store nothing, got \
             {status} {page:?}"
        );
    }
    // A vault of a type that grants no restore access is told so by name.
    let body = json!({"SourceBackupVaultArn": vault_arn("plain-vault")}).to_string();
    let (_, refused) = create_restore_access(&router, &body).await;
    assert_eq!(
        refused.get("Message").and_then(Value::as_str),
        Some("Operation not supported for the provided vault type"),
        "case \"a plain vault as the source\": expected the condition's own sentence, got \
         {refused:?}"
    );
}

/// A repeat create is answered from the STORED vault: a replay of the original
/// request when it carries the same `CreatorRequestId`,
/// `AlreadyExistsException` when it only reuses a name this account holds.
#[tokio::test]
async fn a_repeat_create_replays_the_original_or_collides_on_the_name() {
    let router = router();
    create_source(&router, "repeat-source").await;
    let source_arn = vault_arn("repeat-source");
    let original = json!({
        "SourceBackupVaultArn": source_arn,
        "BackupVaultName": "repeat-access",
        "CreatorRequestId": "token-1",
    })
    .to_string();

    let (status, created) = create_restore_access(&router, &original).await;
    assert_eq!(
        status, 200,
        "case \"first create\": expected 200, got {status} {created:?}"
    );
    let (status, replayed) = create_restore_access(&router, &original).await;
    assert_eq!(
        (
            status,
            string_at(&replayed, "RestoreAccessBackupVaultArn"),
            replayed.get("CreationDate").cloned(),
        ),
        (
            200,
            string_at(&created, "RestoreAccessBackupVaultArn"),
            created.get("CreationDate").cloned(),
        ),
        "case \"same token\": expected the stored vault reported again, got {status} {replayed:?}"
    );

    // A create that reuses a name this account holds, under no token or a
    // fresh one, collides — including on the source vault's own name.
    let colliding: Vec<(&str, String)> = vec![
        (
            "same name, no token",
            json!({"SourceBackupVaultArn": source_arn, "BackupVaultName": "repeat-access"})
                .to_string(),
        ),
        (
            "same name, a different token",
            json!({
                "SourceBackupVaultArn": source_arn,
                "BackupVaultName": "repeat-access",
                "CreatorRequestId": "token-2",
            })
            .to_string(),
        ),
        (
            "the source vault's own name",
            json!({"SourceBackupVaultArn": source_arn, "BackupVaultName": "repeat-source"})
                .to_string(),
        ),
    ];
    for (label, body) in colliding {
        let (status, refused) = create_restore_access(&router, &body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "AlreadyExistsException"),
            "case {label:?}: expected 400 AlreadyExistsException, got {status} {refused:?}"
        );
    }

    // One vault stored through all of it.
    let (status, page) = list_restore_access(&router, "repeat-source", "").await;
    assert_eq!(
        (status, entries(&page).len()),
        (200, 1),
        "case \"after the repeats\": expected exactly one stored vault, got {status} {page:?}"
    );
}

/// Both vault-scoped calls resolve the SOURCE vault before anything else: a
/// source outside the caller's visibility is denied without being told whether
/// it exists, a source that holds no restore access vault answers an empty
/// page, and an ARN that names none of the source's restore access vaults is
/// reported absent.
#[tokio::test]
async fn listing_and_revoking_resolve_the_source_vault_first() {
    let router = router();
    create_source(&router, "visible-source").await;
    let (status, plain) = create_vault(&router, "plain-source", "{}").await;
    assert_eq!(
        status, 200,
        "setup: CreateBackupVault plain-source should succeed, got {status} {plain:?}"
    );

    let unknown_access_arn = vault_arn("absent-access");
    let cases: Vec<(&str, &str, u16, &str)> = vec![
        (
            "a source this account does not hold",
            "absent-source",
            403,
            "AccessDeniedException",
        ),
        (
            "a source that holds no restore access vault",
            "visible-source",
            400,
            "InvalidParameterValueException",
        ),
        (
            "a source whose type grants no restore access",
            "plain-source",
            400,
            "InvalidParameterValueException",
        ),
    ];
    for (label, source, expected_status, expected_code) in cases {
        let (status, refused) = revoke_restore_access(&router, source, &unknown_access_arn).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (expected_status, expected_code),
            "case {label:?}: expected {expected_status} {expected_code}, got {status} {refused:?}"
        );
    }
    // A well-formed ARN that names no restore access vault of the source is
    // refused with the same sentence as a value that is not a vault ARN at
    // all: both say the member does not name a vault access can be revoked on.
    let (_, refused) = revoke_restore_access(&router, "visible-source", &unknown_access_arn).await;
    assert_eq!(
        refused.get("Message").and_then(Value::as_str),
        Some("The provided restore access backup vault ARN is not valid."),
        "case \"an ARN naming no restore access vault\": expected the condition's own sentence, \
         got {refused:?}"
    );
    // A value that is not a backup vault's ARN is refused on its FORM, before
    // any vault is resolved, and with the same code and sentence.
    let malformed: Vec<(&str, String)> = vec![
        ("not an ARN at all", "absent-access".to_string()),
        (
            "another service's ARN",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:backup-vault:absent-access"),
        ),
        (
            "a backup ARN naming no vault",
            format!("arn:aws:backup:{REGION}:{ACCOUNT}:recovery-point:abcd"),
        ),
    ];
    for (label, arn) in malformed {
        let (status, refused) = revoke_restore_access(&router, "visible-source", &arn).await;
        assert_eq!(
            (
                status,
                error_code(&refused).as_str(),
                refused.get("Message").and_then(Value::as_str),
            ),
            (
                400,
                "InvalidParameterValueException",
                Some("The provided restore access backup vault ARN is not valid."),
            ),
            "case {label:?} ({arn}): expected the modelled ARN refusal, got {status} {refused:?}"
        );
    }

    let (status, denied) = list_restore_access(&router, "absent-source", "").await;
    assert_eq!(
        (status, error_code(&denied).as_str()),
        (403, "AccessDeniedException"),
        "case \"listing a source this account does not hold\": expected 403 \
         AccessDeniedException, got {status} {denied:?}"
    );
    for source in ["visible-source", "plain-source"] {
        let (status, page) = list_restore_access(&router, source, "").await;
        assert_eq!(
            (status, entries(&page).len()),
            (200, 0),
            "case \"listing {source}\": a source with no restore access vault answers an empty \
             page, got {status} {page:?}"
        );
    }

    // An ARN belonging to ANOTHER source's restore access vault is absent as
    // far as this source is concerned.
    let body = json!({
        "SourceBackupVaultArn": vault_arn("visible-source"),
        "BackupVaultName": "scoped-access",
    })
    .to_string();
    let (status, created) = create_restore_access(&router, &body).await;
    assert_eq!(
        status, 200,
        "setup scoped-access: expected 200, got {status} {created:?}"
    );
    create_source(&router, "other-source").await;
    let arn = string_at(&created, "RestoreAccessBackupVaultArn");
    let (status, refused) = revoke_restore_access(&router, "other-source", &arn).await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "InvalidParameterValueException"),
        "case \"revoking through another source\": expected 400 \
         InvalidParameterValueException, got {status} {refused:?}"
    );
    // And it is still there, through its own source.
    let (status, page) = list_restore_access(&router, "visible-source", "").await;
    assert_eq!(
        (status, entries(&page).len()),
        (200, 1),
        "case \"after the mis-scoped revoke\": the vault must survive, got {status} {page:?}"
    );
}
