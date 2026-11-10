// SPDX-License-Identifier: CC0-1.0

//! A search job's scope resolves against the recovery points AWS BACKUP holds, driven
//! over HTTP across both services exactly as a client drives them: `backup`'s
//! `CreateBackupVault`/`StartBackupJob`/`ListRecoveryPointsByBackupVault`, then
//! `backup-search`'s `StartSearchJob`, `GetSearchJob` and `ListSearchJobBackups`.
//!
//! The rule under test: the inventory a `SearchScope` selects from is the set of recovery
//! points the account's AWS Backup component holds whose backup index exists, so a backup
//! created through `backup:StartBackupJob` is in scope on the next call, a backup taken
//! without an index is in scope for nothing, and a recovery point Backup no longer holds
//! stops being in scope — no copy of that inventory is kept by the search service.
//!
//! Every assertion is made over a TABLE whose rows differ in what the rule generalises
//! over — which `SearchScope` member selects (resource type, recovery-point ARNs, source
//! resource ARNs, a creation window, a filter stating no requirement), how many backups
//! are selected, which vault holds them, the resource type of the protected resource, and
//! whether the backup carries an index at all — never over one request.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backup_cryocontrollerusermanager::state as backup_state;
use service_com_amazonaws_backupsearch_cryobackupsearchservice::state;

use crate::search_jobs::{call, settle_a_span_ahead, Answer, PAST_THE_DUE_INSTANT};

const REGION: &str = "us-west-2";

/// The IAM role a backup job runs as. Any role ARN of this account: the rule under test
/// is about the recovery point the job stores, not about the role it ran as.
fn backup_role() -> String {
    format!(
        "arn:aws:iam::{}:role/service-role/AWSBackupDefaultServiceRole",
        engine::emulator_account_id()
    )
}

/// Longer than the span the Backup component owes a recovery point its completed backup
/// and built index over ([`backup_state::JOB_SETTLE_AFTER`]), so a call made after this
/// sleep observes what that clock has already made due. Derived from the span rather than
/// restated, so it cannot drift from it; a real sleep because both services read the wall
/// clock.
const PAST_THE_BACKUP_SETTLE: Duration =
    backup_state::JOB_SETTLE_AFTER.saturating_add(Duration::from_millis(400));

/// One emulator holding BOTH services over one shared engine, as the router assembles
/// them: a search job's scope is resolved against the recovery points the Backup
/// component holds, so the two have to be mounted on the same engine for anything to be
/// in scope.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_backupsearch_cryobackupsearchservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_backup_cryocontrollerusermanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
}

/// One request to the BACKUP service: the credential scope names `backup`, which is how
/// the router picks the service out of the two mounted above.
async fn backup_call(
    emulator: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map_or_else(Vec::new, |value| value.to_string().into_bytes());
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "backup.amazonaws.com")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261021/{REGION}/backup/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");
    let response = emulator.dispatch(request).await;
    let status = response.status().as_u16();
    let error_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string);
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        body,
        error_code,
    }
}

/// `backup:CreateBackupVault` for `name`.
async fn create_vault(emulator: &MultiServiceRouter, case: &str, name: &str) {
    let created = backup_call(
        emulator,
        "PUT",
        &format!("/backup-vaults/{name}"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: CreateBackupVault must succeed, got {} ({:?})",
        created.status, created.body
    );
}

/// `backup:StartBackupJob` for `resource_arn` into `vault`, asking for a backup index
/// when `index` says so, answering the `RecoveryPointArn` the job reserved.
async fn start_backup(
    emulator: &MultiServiceRouter,
    case: &str,
    vault: &str,
    resource_arn: &str,
    index: bool,
) -> String {
    // `Index` is the model's own enum (`smithy_query(query="shape", shape="Index")`,
    // values ENABLED and DISABLED): ENABLED is what asks AWS Backup to build the backup
    // index a search reads.
    let started = backup_call(
        emulator,
        "PUT",
        "/backup-jobs",
        Some(json!({
            "BackupVaultName": vault,
            "ResourceArn": resource_arn,
            "IamRoleArn": backup_role(),
            "Index": if index { "ENABLED" } else { "DISABLED" },
        })),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: StartBackupJob must succeed, got {} ({:?})",
        started.status, started.body
    );
    started.string("RecoveryPointArn")
}

/// The recovery point `arn` as `backup:ListRecoveryPointsByBackupVault` reports it out of
/// `vault`, or `Value::Null` when that listing does not report it.
async fn listed_recovery_point(
    emulator: &MultiServiceRouter,
    vault: &str,
    arn: &str,
) -> Value {
    let listed = backup_call(
        emulator,
        "GET",
        &format!("/backup-vaults/{vault}/recovery-points"),
        None,
    )
    .await;
    listed
        .get("RecoveryPoints")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|point| point["RecoveryPointArn"].as_str() == Some(arn))
        .unwrap_or(Value::Null)
}

/// `backup-search:StartSearchJob` over `scope`, answering the identifier it minted.
async fn start_search(emulator: &MultiServiceRouter, case: &str, scope: Value) -> String {
    let started = call(
        emulator,
        REGION,
        "PUT",
        "/search-jobs",
        Some(json!({"Name": "backup-plane", "SearchScope": scope})),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "case {case:?}: StartSearchJob must accept the request, got {} ({:?})",
        started.status, started.body
    );
    started.string("SearchJobIdentifier")
}

/// The recovery-point ARNs `backup-search:ListSearchJobBackups` reports for `identifier`,
/// in the order it reports them.
async fn searched_backups(emulator: &MultiServiceRouter, identifier: &str) -> Vec<String> {
    let listed = call(
        emulator,
        REGION,
        "GET",
        &format!("/search-jobs/{identifier}/backups"),
        None,
    )
    .await;
    listed
        .get("Results")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            entry["BackupResourceArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// An instant as the epoch seconds this protocol serialises a timestamp as.
fn epoch_seconds(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// The protected resources the inventory below is backed up from: a bucket for an S3
/// backup, a volume for an EBS one, a table for a backup of neither searchable type.
fn bucket(name: &str) -> String {
    format!("arn:aws:s3:::{name}")
}

fn volume(name: &str) -> String {
    format!(
        "arn:aws:ec2:{REGION}:{}:volume/{name}",
        engine::emulator_account_id()
    )
}

fn table(name: &str) -> String {
    format!(
        "arn:aws:dynamodb:{REGION}:{}:table/{name}",
        engine::emulator_account_id()
    )
}

/// The recovery points the scope table below selects from, named by the protected
/// resource each was taken of.
struct Inventory {
    alpha: String,
    beta: String,
    gamma: String,
    delta: String,
    epsilon: String,
    zeta: String,
}

const VAULT_ONE: &str = "search-vault-one";
const VAULT_TWO: &str = "search-vault-two";

/// Back up six protected resources into two vaults, through Backup's OWN API: three
/// indexed S3 buckets (one of them in a second vault), one S3 bucket taken WITHOUT an
/// index, one indexed EBS volume, and one indexed DynamoDB table — a resource type this
/// service's `ResourceType` enum does not name.
async fn backed_up_inventory(emulator: &MultiServiceRouter) -> Inventory {
    create_vault(emulator, "inventory", VAULT_ONE).await;
    create_vault(emulator, "inventory", VAULT_TWO).await;
    let inventory = Inventory {
        alpha: start_backup(emulator, "alpha", VAULT_ONE, &bucket("alpha"), true).await,
        beta: start_backup(emulator, "beta", VAULT_ONE, &bucket("beta"), true).await,
        gamma: start_backup(emulator, "gamma", VAULT_ONE, &bucket("gamma"), false).await,
        delta: start_backup(emulator, "delta", VAULT_ONE, &volume("vol-delta"), true).await,
        epsilon: start_backup(emulator, "epsilon", VAULT_ONE, &table("epsilon"), true).await,
        zeta: start_backup(emulator, "zeta", VAULT_TWO, &bucket("zeta"), true).await,
    };
    // The index is built by the job that takes the backup, so the inventory is searchable
    // once the clock has made that transition due.
    std::thread::sleep(PAST_THE_BACKUP_SETTLE);
    inventory
}

/// One row of the scope table: the `SearchScope` a job is started with and the recovery
/// points it must therefore resolve to, in the order the search reports them.
struct ScopeCase {
    case: &'static str,
    scope: Value,
    covered: Vec<String>,
}

fn scope_cases(inventory: &Inventory, before: f64, after: f64) -> Vec<ScopeCase> {
    let Inventory {
        alpha,
        beta,
        gamma,
        delta,
        epsilon,
        zeta,
    } = inventory;
    vec![
        ScopeCase {
            case: "the S3 type selects every indexed S3 backup of the account",
            scope: json!({"BackupResourceTypes": ["S3"]}),
            covered: vec![alpha.clone(), beta.clone(), zeta.clone()],
        },
        ScopeCase {
            case: "the EBS type selects only the volume backup",
            scope: json!({"BackupResourceTypes": ["EBS"]}),
            covered: vec![delta.clone()],
        },
        ScopeCase {
            case: "one recovery-point ARN",
            scope: json!({"BackupResourceTypes": ["S3"], "BackupResourceArns": [alpha]}),
            covered: vec![alpha.clone()],
        },
        ScopeCase {
            case: "two recovery-point ARNs, one of them held by the other vault",
            scope: json!({"BackupResourceTypes": ["S3"], "BackupResourceArns": [alpha, zeta]}),
            covered: vec![alpha.clone(), zeta.clone()],
        },
        ScopeCase {
            case: "the ARN of the protected resource a backup was taken of",
            scope: json!({"BackupResourceTypes": ["S3"], "SourceResourceArns": [bucket("beta")]}),
            covered: vec![beta.clone()],
        },
        ScopeCase {
            case: "a creation window open around the instant the backups were taken",
            scope: json!({
                "BackupResourceTypes": ["S3"],
                "BackupResourceCreationTime": {"CreatedAfter": before, "CreatedBefore": after},
            }),
            covered: vec![alpha.clone(), beta.clone(), zeta.clone()],
        },
        ScopeCase {
            case: "an empty ARN list, which states no requirement",
            scope: json!({"BackupResourceTypes": ["S3"], "BackupResourceArns": []}),
            covered: vec![alpha.clone(), beta.clone(), zeta.clone()],
        },
        ScopeCase {
            case: "a creation window that closed before the backups were taken",
            scope: json!({
                "BackupResourceTypes": ["S3"],
                "BackupResourceCreationTime": {"CreatedBefore": before},
            }),
            covered: Vec::new(),
        },
        ScopeCase {
            case: "the recovery-point ARN of a backup taken without an index",
            scope: json!({"BackupResourceTypes": ["S3"], "BackupResourceArns": [gamma]}),
            covered: Vec::new(),
        },
        ScopeCase {
            case: "the source resource of a backup of the other searchable type",
            scope: json!({
                "BackupResourceTypes": ["S3"],
                "SourceResourceArns": [volume("vol-delta")],
            }),
            covered: Vec::new(),
        },
        ScopeCase {
            case: "the source resource of a backup of no searchable type",
            scope: json!({"BackupResourceTypes": ["S3"], "SourceResourceArns": [table("epsilon")]}),
            covered: Vec::new(),
        },
        ScopeCase {
            case: "a recovery-point ARN no backup of this account answers to",
            scope: json!({
                "BackupResourceTypes": ["S3"],
                "BackupResourceArns": [format!("{epsilon}-absent")],
            }),
            covered: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn a_search_scope_resolves_to_the_recovery_points_aws_backup_holds() {
    let (emulator, engine) = emulator();
    let inventory = backed_up_inventory(&emulator).await;
    // Window bounds a whole hour either side of the instant the backups were taken, so
    // neither bound is racing the clock the rows are judged against.
    let hour = Duration::from_secs(60 * 60);
    let before = epoch_seconds(SystemTime::now().checked_sub(hour).expect("an instant"));
    let after = epoch_seconds(SystemTime::now().checked_add(hour).expect("an instant"));

    let mut rows: Vec<(ScopeCase, String)> = Vec::new();
    for case in scope_cases(&inventory, before, after) {
        let identifier = start_search(&emulator, case.case, case.scope.clone()).await;
        rows.push((case, identifier));
    }
    // Every job's scan ends at a STORED instant, so the clock is carried to it rather
    // than waited out.
    settle_a_span_ahead(&engine, PAST_THE_DUE_INSTANT);

    for (case, identifier) in rows {
        let ScopeCase {
            case: name,
            covered,
            ..
        } = case;
        let fetched = call(
            &emulator,
            REGION,
            "GET",
            &format!("/search-jobs/{identifier}"),
            None,
        )
        .await;
        let summary = fetched.get("SearchScopeSummary").clone();
        assert_eq!(
            summary["TotalRecoveryPointsToScanCount"].as_u64(),
            Some(covered.len() as u64),
            "case {name:?}: the scope must resolve to the backups Backup holds for it, got {summary:?}"
        );
        assert_eq!(
            searched_backups(&emulator, &identifier).await,
            covered,
            "case {name:?}: the covered backups must be the ones the scope selected, got {:?}",
            fetched.body
        );
        // A scope that resolved to a backup leaves the scan something to get through, so
        // it runs to its completion; one that resolved to none is the empty-scope failure
        // the service explains in StatusMessage.
        let expected_status = if covered.is_empty() {
            state::FAILED
        } else {
            state::COMPLETED
        };
        assert_eq!(
            fetched.string("Status"),
            expected_status,
            "case {name:?}: a scope that resolved to {} backups settles to {expected_status}, got {:?}",
            covered.len(),
            fetched.body
        );
        let expected_message = covered.is_empty().then_some(state::SCOPE_EMPTY);
        assert_eq!(
            fetched.get("StatusMessage").as_str(),
            expected_message,
            "case {name:?}: expected StatusMessage {expected_message:?}, got {:?}",
            fetched.body
        );
    }
}

#[tokio::test]
async fn a_scope_resolves_to_a_backup_only_while_aws_backup_holds_it() {
    let (emulator, engine) = emulator();
    let vault = "propagation-vault";
    create_vault(&emulator, "propagation", vault).await;
    let scope = json!({"BackupResourceTypes": ["S3"]});

    // Before any backup exists there is nothing to search, which is the empty-scope
    // failure — and no copy of an inventory that could say otherwise.
    let empty = start_search(&emulator, "no backup yet", scope.clone()).await;

    let resource_arn = bucket("propagated");
    let point_arn = start_backup(&emulator, "propagated", vault, &resource_arn, true).await;
    std::thread::sleep(PAST_THE_BACKUP_SETTLE);
    let listed = listed_recovery_point(&emulator, vault, &point_arn).await;
    assert_eq!(
        listed["IndexStatus"].as_str(),
        Some("ACTIVE"),
        "case \"propagated\": Backup must report the built index of the backup it took, got {listed:?}"
    );

    let covered = start_search(&emulator, "a backup exists", scope.clone()).await;

    // And once Backup no longer holds that recovery point, a scope over the same
    // resource type resolves to nothing again.
    let deleted = backup_call(
        &emulator,
        "DELETE",
        &format!("/backup-vaults/{vault}/recovery-points/{point_arn}"),
        None,
    )
    .await;
    assert!(
        deleted.status == 200 || deleted.status == 204,
        "case \"deleted\": DeleteRecoveryPoint must succeed, got {} ({:?})",
        deleted.status,
        deleted.body
    );
    let gone = start_search(&emulator, "the backup is gone", scope).await;

    settle_a_span_ahead(&engine, PAST_THE_DUE_INSTANT);

    for (case, identifier, expected) in [
        ("no backup yet", &empty, Vec::new()),
        ("a backup exists", &covered, vec![point_arn.clone()]),
        ("the backup is gone", &gone, Vec::new()),
    ] {
        let fetched = call(
            &emulator,
            REGION,
            "GET",
            &format!("/search-jobs/{identifier}"),
            None,
        )
        .await;
        assert_eq!(
            fetched.get("SearchScopeSummary")["TotalRecoveryPointsToScanCount"].as_u64(),
            Some(expected.len() as u64),
            "case {case:?}: expected {} backups in scope, got {:?}",
            expected.len(),
            fetched.body
        );
        assert_eq!(
            searched_backups(&emulator, identifier).await,
            expected,
            "case {case:?}: expected the covered backups {expected:?}, got {:?}",
            fetched.body
        );
        let expected_status = if expected.is_empty() {
            state::FAILED
        } else {
            state::COMPLETED
        };
        assert_eq!(
            fetched.string("Status"),
            expected_status,
            "case {case:?}: expected the job to settle to {expected_status}, got {:?}",
            fetched.body
        );
    }
}

#[tokio::test]
async fn a_covered_backup_is_reported_as_the_backup_component_holds_it() {
    let (emulator, engine) = emulator();
    let vault = "projection-vault";
    create_vault(&emulator, "projection", vault).await;

    // One row per searchable resource type, so the projection is pinned on both and not
    // on the one the issue cited.
    let rows = [
        ("S3", bucket("projected"), state::INDEX_ACTIVE),
        ("EBS", volume("vol-projected"), state::INDEX_ACTIVE),
    ];
    let mut started: Vec<(&str, String, String, String)> = Vec::new();
    for (resource_type, resource_arn, index_status) in &rows {
        let point_arn =
            start_backup(&emulator, resource_type, vault, resource_arn, true).await;
        started.push((
            resource_type,
            resource_arn.clone(),
            point_arn,
            (*index_status).to_string(),
        ));
    }
    std::thread::sleep(PAST_THE_BACKUP_SETTLE);

    let mut jobs: Vec<(&str, String, String, String, String)> = Vec::new();
    for (resource_type, resource_arn, point_arn, index_status) in started {
        let identifier = start_search(
            &emulator,
            resource_type,
            json!({"BackupResourceTypes": [resource_type]}),
        )
        .await;
        jobs.push((
            resource_type,
            resource_arn,
            point_arn,
            index_status,
            identifier,
        ));
    }
    settle_a_span_ahead(&engine, PAST_THE_DUE_INSTANT);

    for (resource_type, resource_arn, point_arn, index_status, identifier) in jobs {
        let listed = call(
            &emulator,
            REGION,
            "GET",
            &format!("/search-jobs/{identifier}/backups"),
            None,
        )
        .await;
        let results = listed
            .get("Results")
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            results.len(),
            1,
            "case {resource_type:?}: exactly the one backup of that type is covered, got {:?}",
            listed.body
        );
        let reported = &results[0];
        assert_eq!(
            reported["BackupResourceArn"].as_str(),
            Some(point_arn.as_str()),
            "case {resource_type:?}: the backup resource is the recovery point Backup stored, got {reported:?}"
        );
        assert_eq!(
            reported["SourceResourceArn"].as_str(),
            Some(resource_arn.as_str()),
            "case {resource_type:?}: the source resource is the protected resource backed up, got {reported:?}"
        );
        assert_eq!(
            reported["ResourceType"].as_str(),
            Some(resource_type),
            "case {resource_type:?}: the resource type is the one Backup derived, got {reported:?}"
        );
        assert_eq!(
            reported["Status"].as_str(),
            Some(index_status.as_str()),
            "case {resource_type:?}: the status is the state of the backup index, got {reported:?}"
        );

        // The same backup, read through the component that OWNS it: the two reads cannot
        // disagree about when the backup was created. Compared at the precision this
        // service's own timestamps carry ([`state::timestamp`]: milliseconds, which is
        // the precision the model documents for them).
        let held = listed_recovery_point(&emulator, vault, &point_arn).await;
        let millis = |value: Option<f64>| value.map(|seconds| (seconds * 1_000.0) as i64);
        assert_eq!(
            millis(reported["BackupCreationTime"].as_f64()),
            millis(held["CreationDate"].as_f64()),
            "case {resource_type:?}: the backup creation time must be the one Backup reports ({held:?}), got {reported:?}"
        );
        let index_created = reported["IndexCreationTime"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {resource_type:?}: a built index has a creation time, got {reported:?}"));
        let backup_created = reported["BackupCreationTime"].as_f64().unwrap_or_default();
        assert!(
            index_created >= backup_created,
            "case {resource_type:?}: the index cannot be built before the backup it indexes ({backup_created}), got {index_created}"
        );
    }
}
