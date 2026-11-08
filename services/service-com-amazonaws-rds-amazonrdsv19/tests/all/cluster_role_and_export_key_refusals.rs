// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for two refusal rules this front door holds to.
//!
//! 1. The calls that manage a DB CLUSTER'S IAM ROLE ASSOCIATIONS — `AddRoleToDBCluster` and
//!    `RemoveRoleFromDBCluster` — answer a `DBClusterIdentifier` this service resolves no
//!    cluster for with the modelled `DBClusterNotFoundFault` (HTTP 404, `Error/Type`
//!    `Sender`) whose `<Message>` is the plain not-found sentence `DBCluster not found:
//!    <named>`, naming the value the REQUEST carried in the spelling it was written in —
//!    the same `<resource type> not found: <named>` form their DB instance siblings use —
//!    rather than the sentence this service words for a cluster it cannot ACCESS.
//!    Asserted over a TABLE spanning both operations, the three forms a member that names a
//!    cluster accepts (the identifier as written, upper-cased, and the cluster's ARN), and
//!    `FeatureName` given and omitted, with another cluster stored throughout so no 404 can
//!    come from an empty store.
//!
//! 2. `StartExportTask` decides what the REQUEST itself carries before it resolves anything
//!    the request NAMES: a `SourceArn` whose ARN names another Region is refused with
//!    `InvalidParameterValue` and the sentence that names the ARN, and a `KmsKeyId` naming a
//!    key KMS will not encrypt under is refused with the modelled `KMSKeyNotAccessibleFault`
//!    (HTTP 400) and its own sentence, whatever the rest of the request names; a request
//!    whose key IS accessible is then refused by whatever its `SourceArn` names, each with
//!    that source's own fault. Asserted over a TABLE spanning the forms a `KmsKeyId` takes
//!    (key ARN, bare key id, alias) and both halves of the fault (a key KMS holds nothing
//!    under, and a key it holds DISABLED), crossed with every kind of `SourceArn` the
//!    operation resolves — a DB snapshot, a DB cluster snapshot, a DB cluster, and a value
//!    that is no Amazon RDS ARN at all — in this Region and in another one. A last suite
//!    stands a real source up and pins the same rule against the store: an identifier an
//!    export is already running under is answered AFTER the key, and once the key is one
//!    KMS will encrypt under it is the identifier that refuses the request.
//!
//! Every call goes over the wire as a real awsQuery form POST, and KMS, Amazon S3 and IAM
//! are mounted beside this service on ONE engine exactly as the assembled router mounts
//! them, so the key the export resolves is a key that service really holds.

use crate::db_instances::{call, expect_error, expect_ok, subtrees, text, wait_for_settle, REGION};
use crate::rds_s3_integration::{create_bucket, emulator, kms, put_object_policy, role};
use service_com_amazonaws_rds_amazonrdsv19::state as st;
use smithy_server_runtime::MultiServiceRouter;

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// A second Region, for the rows whose `SourceArn` names a resource outside the Region the
/// call is made in.
const OTHER_REGION: &str = "eu-west-1";

/// An identifier no call below creates a cluster under.
const ABSENT_CLUSTER: &str = "emu-absent-role-cluster";

/// The one cluster these tables keep stored, so a 404 is a decision about the identifier the
/// request named and never "this service holds no cluster at all".
const PRESENT_CLUSTER: &str = "emu-present-role-cluster";

/// A well-formed `RoleArn`, of the form `IAMRoleArn`'s pattern publishes
/// (`^arn:aws[a-z-]*:iam::[0-9]*:role/.*$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/emu-cluster-role";

/// The ARN form every member that names a DB cluster also accepts.
fn cluster_arn(identifier: &str) -> String {
    format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:{identifier}")
}

/// The sentence the role-association calls answer `DBClusterNotFoundFault` with, spelled
/// out here so the wording is pinned by the test rather than by the code under test.
fn cluster_not_found(named: &str) -> String {
    format!("DBCluster not found: {named}")
}

/// The sentence `StartExportTask` answers `KMSKeyNotAccessibleFault` with, likewise spelled
/// out here.
fn key_not_accessible(key_id: &str) -> String {
    format!("The KMS key {key_id} doesn't exist or is disabled.")
}

/// The sentence a `SourceArn` of another Region is refused with, likewise spelled out here.
fn located_elsewhere(source_arn: &str) -> String {
    format!("The resource {source_arn} is located in a different AWS Region.")
}

/// The sentence a `SourceArn` that names no exportable Amazon RDS resource is refused with,
/// whatever Region it carries.
fn not_an_export_source(source_arn: &str) -> String {
    format!(
        "The parameter SourceArn must name the ARN of a DB snapshot, DB cluster snapshot or \
         DB cluster to export; got {source_arn}."
    )
}

/// Assert the status, the wire error `<Code>`, the exact `<Message>` and the `Sender` class
/// of one refusal.
fn expect_refusal(case: &str, response: (u16, String), status: u16, code: &str, message: &str) {
    let body = expect_error(case, response, status, code);
    let answered = text(&body, "Message");
    assert_eq!(
        answered, message,
        "case {case:?}: expected the message {message:?}, got {answered:?} in {body}"
    );
    let kind = text(&body, "Type");
    assert_eq!(
        kind, "Sender",
        "case {case:?}: expected a Sender fault, got {kind:?} in {body}"
    );
}

/// One row of the role-association table: the operation, and whether the request also names
/// a `FeatureName`.
struct RoleOperation {
    action: &'static str,
    feature_name: Option<&'static str>,
}

/// Both operations that manage a cluster's role associations, with `FeatureName` given and
/// omitted — the only other member either takes beyond the cluster and the role.
const ROLE_OPERATIONS: &[RoleOperation] = &[
    RoleOperation {
        action: "AddRoleToDBCluster",
        feature_name: None,
    },
    RoleOperation {
        action: "AddRoleToDBCluster",
        feature_name: Some("s3Export"),
    },
    RoleOperation {
        action: "RemoveRoleFromDBCluster",
        feature_name: None,
    },
    RoleOperation {
        action: "RemoveRoleFromDBCluster",
        feature_name: Some("s3Export"),
    },
];

/// Create a DB cluster and wait until the clock has made it `available`.
async fn available_cluster(router: &MultiServiceRouter, identifier: &str) {
    expect_ok(
        identifier,
        call(
            router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", identifier),
                ("Engine", "aurora-postgresql"),
            ],
        )
        .await,
    );
    wait_for_settle();
}

#[tokio::test]
async fn the_cluster_role_calls_word_their_own_not_found_fault() {
    let (router, _engine) = emulator();
    available_cluster(&router, PRESENT_CLUSTER).await;

    // The three forms a member that names a DB cluster accepts ("the user-supplied DB
    // cluster identifier or the Amazon Resource Name (ARN)", "this parameter isn't
    // case-sensitive"), all naming nothing.
    let forms: [(&str, String); 3] = [
        ("the identifier as written", ABSENT_CLUSTER.to_string()),
        (
            "the same identifier upper-cased",
            ABSENT_CLUSTER.to_ascii_uppercase(),
        ),
        ("the cluster's ARN form", cluster_arn(ABSENT_CLUSTER)),
    ];

    for row in ROLE_OPERATIONS {
        for (form, named) in &forms {
            let case = format!(
                "{} with {form}{}",
                row.action,
                match row.feature_name {
                    Some(feature) => format!(" and FeatureName {feature}"),
                    None => String::new(),
                }
            );
            let mut params: Vec<(&str, String)> = vec![
                ("DBClusterIdentifier", named.clone()),
                ("RoleArn", ROLE_ARN.to_string()),
            ];
            if let Some(feature) = row.feature_name {
                params.push(("FeatureName", feature.to_string()));
            }
            expect_refusal(
                &case,
                call(&router, row.action, &params).await,
                404,
                "DBClusterNotFoundFault",
                &cluster_not_found(named),
            );
        }
    }
}

/// One `KmsKeyId` the export cannot be encrypted under: how the request spells it, and
/// whether the key has to be created and disabled first.
struct KeyRow {
    case: &'static str,
    /// The identifier the request carries, `None` for the row whose key is created and
    /// disabled (its id is only known at run time).
    key_id: Option<String>,
    disabled_key: bool,
}

/// One `SourceArn` this region resolves nothing for: the fault the operation answers it with
/// once the key is accessible.
struct SourceRow {
    case: &'static str,
    source_arn: String,
    status: u16,
    code: &'static str,
}

/// Every form of `KmsKeyId` KMS will not encrypt an export under: the three forms the
/// member accepts ("the key ARN, key ID, alias ARN, or alias name") naming no key, and a
/// key that exists but is DISABLED — the other half of what the fault reports.
fn key_rows() -> Vec<KeyRow> {
    vec![
        KeyRow {
            case: "a key ARN KMS holds nothing under",
            key_id: Some(format!(
                "arn:aws:kms:{REGION}:{ACCOUNT}:key/11111111-2222-3333-4444-555555555555"
            )),
            disabled_key: false,
        },
        KeyRow {
            case: "a bare key id KMS holds nothing under",
            key_id: Some("22222222-3333-4444-5555-666666666666".to_string()),
            disabled_key: false,
        },
        KeyRow {
            case: "an alias this account holds no key under",
            key_id: Some("alias/emu-absent-export-key".to_string()),
            disabled_key: false,
        },
        KeyRow {
            case: "a key KMS holds but has disabled",
            key_id: None,
            disabled_key: true,
        },
    ]
}

/// Every kind of `SourceArn` `StartExportTask` resolves — "the Amazon Resource Name (ARN) of
/// the snapshot or cluster to export to Amazon S3" — each naming nothing in this region, with
/// the refusal an accessible key leaves it with.
fn source_rows() -> Vec<SourceRow> {
    vec![
        SourceRow {
            case: "a DB snapshot that does not exist",
            source_arn: format!("arn:aws:rds:{REGION}:{ACCOUNT}:snapshot:emu-absent-snapshot"),
            status: 404,
            code: "DBSnapshotNotFound",
        },
        SourceRow {
            case: "a DB cluster snapshot that does not exist",
            source_arn: format!(
                "arn:aws:rds:{REGION}:{ACCOUNT}:cluster-snapshot:emu-absent-cluster-snap"
            ),
            status: 404,
            code: "DBClusterSnapshotNotFoundFault",
        },
        SourceRow {
            case: "a DB cluster that does not exist",
            source_arn: format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:emu-absent-export-cluster"),
            status: 404,
            code: "DBClusterNotFoundFault",
        },
        SourceRow {
            case: "a value that is no Amazon RDS ARN at all",
            source_arn: "not-an-arn".to_string(),
            status: 400,
            code: "InvalidParameterValue",
        },
    ]
}

/// The export's fixture: the destination bucket, the role the export acts under, and one
/// ENABLED key, which is what the control rows name.
struct Fixture {
    router: MultiServiceRouter,
    bucket: String,
    role_arn: String,
    enabled_key: String,
    disabled_key: String,
}

/// Stand up that fixture. No source is created: every row below names a source this region
/// holds nothing under, which is what the precedence is asserted over.
async fn fixture() -> Fixture {
    let (router, _engine) = emulator();
    let bucket = "emu-export-key-precedence";
    create_bucket(&router, bucket, REGION).await;
    let role_arn = role(
        &router,
        "rds-export-key-precedence-role",
        st::EXPORT_SERVICE_PRINCIPAL,
        Some(put_object_policy(bucket)),
    )
    .await;
    let enabled_key = created_key(&router, "the export's enabled key").await;
    let disabled_key = created_key(&router, "the export's disabled key").await;
    kms(&router)
        .disable_key()
        .key_id(&disabled_key)
        .send()
        .await
        .expect("the emulated KMS disables the key");
    Fixture {
        router,
        bucket: bucket.to_string(),
        role_arn,
        enabled_key,
        disabled_key,
    }
}

/// One key this emulator's own KMS holds, reported by its key id.
async fn created_key(router: &MultiServiceRouter, description: &str) -> String {
    kms(router)
        .create_key()
        .description(description)
        .send()
        .await
        .expect("the emulated KMS creates the key")
        .key_metadata()
        .map(|metadata| metadata.key_id().to_string())
        .expect("the created key reports its id")
}

/// The parameters of one `StartExportTask` call.
fn export_request(
    fixture: &Fixture,
    identifier: &str,
    source_arn: &str,
    key_id: &str,
) -> Vec<(&'static str, String)> {
    vec![
        ("ExportTaskIdentifier", identifier.to_string()),
        ("SourceArn", source_arn.to_string()),
        ("S3BucketName", fixture.bucket.clone()),
        ("IamRoleArn", fixture.role_arn.clone()),
        ("KmsKeyId", key_id.to_string()),
    ]
}

#[tokio::test]
async fn an_inaccessible_export_key_is_refused_before_the_source_is_resolved() {
    let fixture = fixture().await;
    for key_row in key_rows() {
        let key_id = match (&key_row.key_id, key_row.disabled_key) {
            (Some(key_id), _) => key_id.clone(),
            (None, true) => fixture.disabled_key.clone(),
            (None, false) => unreachable!("a key row names a key or is the disabled one"),
        };
        for source_row in source_rows() {
            let case = format!("{} with {}", key_row.case, source_row.case);
            expect_refusal(
                &case,
                call(
                    &fixture.router,
                    "StartExportTask",
                    &export_request(
                        &fixture,
                        "emu-export-key-precedence-task",
                        &source_row.source_arn,
                        &key_id,
                    ),
                )
                .await,
                400,
                "KMSKeyNotAccessibleFault",
                &key_not_accessible(&key_id),
            );
        }
    }

    // A refused export records no task: the refusal is a decision about the request, so
    // nothing of it survives in the store.
    let described = expect_ok(
        "no task was recorded",
        call(
            &fixture.router,
            "DescribeExportTasks",
            &[("MaxRecords", "20")],
        )
        .await,
    );
    assert!(
        subtrees(&described, "ExportTask").is_empty(),
        "a refused export records no task, got {described}"
    );
}

#[tokio::test]
async fn an_accessible_export_key_leaves_the_source_to_answer() {
    let fixture = fixture().await;
    for source_row in source_rows() {
        let case = format!("{} with an accessible key", source_row.case);
        expect_error(
            &case,
            call(
                &fixture.router,
                "StartExportTask",
                &export_request(
                    &fixture,
                    "emu-export-accessible-key-task",
                    &source_row.source_arn,
                    &fixture.enabled_key,
                ),
            )
            .await,
            source_row.status,
            source_row.code,
        );
    }
}

#[tokio::test]
async fn a_source_in_another_region_is_refused_from_the_request_alone() {
    let fixture = fixture().await;
    // Every kind of source an export reads, each named by an ARN of another Region.
    let sources: [(&str, String); 3] = [
        (
            "a DB snapshot of another Region",
            format!("arn:aws:rds:{OTHER_REGION}:{ACCOUNT}:snapshot:emu-elsewhere-snapshot"),
        ),
        (
            "a DB cluster snapshot of another Region",
            format!(
                "arn:aws:rds:{OTHER_REGION}:{ACCOUNT}:cluster-snapshot:emu-elsewhere-cluster-snap"
            ),
        ),
        (
            "a DB cluster of another Region",
            format!("arn:aws:rds:{OTHER_REGION}:{ACCOUNT}:cluster:emu-elsewhere-cluster"),
        ),
    ];
    // The Region of the source is decided from the request alone, so the key the export
    // names does not change the answer: both an accessible key and one KMS holds nothing
    // under leave the same refusal.
    let keys: [(&str, String); 2] = [
        ("an accessible key", fixture.enabled_key.clone()),
        (
            "a key KMS holds nothing under",
            format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/33333333-4444-5555-6666-777777777777"),
        ),
    ];
    for (key_case, key_id) in &keys {
        for (source_case, source_arn) in &sources {
            let case = format!("{source_case} with {key_case}");
            expect_refusal(
                &case,
                call(
                    &fixture.router,
                    "StartExportTask",
                    &export_request(
                        &fixture,
                        "emu-export-elsewhere-task",
                        source_arn,
                        key_id,
                    ),
                )
                .await,
                400,
                "InvalidParameterValue",
                &located_elsewhere(source_arn),
            );
        }
    }

    // A value of another Region that names no EXPORTABLE source is not that refusal: the
    // Region of a resource this operation never reads decides nothing, so it stays on the
    // unexportable-source path — reached, like every other source answer, only once the
    // key the export would be encrypted with is one KMS will encrypt under.
    for (source_case, source_arn) in [
        (
            "a DB INSTANCE of another Region, which no export reads",
            format!("arn:aws:rds:{OTHER_REGION}:{ACCOUNT}:db:emu-elsewhere-instance"),
        ),
        (
            "an ARN of another service and another Region",
            format!("arn:aws:s3:{OTHER_REGION}:{ACCOUNT}:bucket:emu-elsewhere-bucket"),
        ),
    ] {
        let case = format!("{source_case} with an accessible key");
        expect_refusal(
            &case,
            call(
                &fixture.router,
                "StartExportTask",
                &export_request(
                    &fixture,
                    "emu-export-unexportable-task",
                    &source_arn,
                    &fixture.enabled_key,
                ),
            )
            .await,
            400,
            "InvalidParameterValue",
            &not_an_export_source(&source_arn),
        );
    }
}

/// The master user the source instance below is created with, and its password — test
/// credentials, and the only place in this suite they appear.
const MASTER_USER: &str = "emuadmin";
const MASTER_PASSWORD: &str = "emu-Secret-99";

/// Create a DB instance and the snapshot taken of it, and answer with that snapshot's ARN:
/// a source an export can really read, which is what the precedence below needs.
async fn exportable_snapshot(fixture: &Fixture, instance: &str, snapshot: &str) -> String {
    expect_ok(
        "create the source instance",
        call(
            &fixture.router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", instance.to_string()),
                ("Engine", "mysql".to_string()),
                ("DBInstanceClass", "db.t3.micro".to_string()),
                ("AllocatedStorage", "20".to_string()),
                ("MasterUsername", MASTER_USER.to_string()),
                ("MasterUserPassword", MASTER_PASSWORD.to_string()),
            ],
        )
        .await,
    );
    wait_for_settle();
    expect_ok(
        "snapshot the source instance",
        call(
            &fixture.router,
            "CreateDBSnapshot",
            &[
                ("DBInstanceIdentifier", instance.to_string()),
                ("DBSnapshotIdentifier", snapshot.to_string()),
            ],
        )
        .await,
    );
    wait_for_settle();
    format!("arn:aws:rds:{REGION}:{ACCOUNT}:snapshot:{snapshot}")
}

#[tokio::test]
async fn the_key_is_answered_before_an_identifier_the_store_already_holds() {
    let fixture = fixture().await;
    let source_arn = exportable_snapshot(
        &fixture,
        "emu-export-precedence-source",
        "emu-export-precedence-snapshot",
    )
    .await;
    let identifier = "emu-export-precedence-task";
    expect_ok(
        "the first export under this identifier",
        call(
            &fixture.router,
            "StartExportTask",
            &export_request(&fixture, identifier, &source_arn, &fixture.enabled_key),
        )
        .await,
    );

    // The same identifier again, with a key KMS holds nothing under: the key is a parameter
    // of the REQUEST and the identifier is a question for the store, so the key is answered
    // first.
    let absent_key =
        format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/44444444-5555-6666-7777-888888888888");
    expect_refusal(
        "an identifier already running, with a key KMS holds nothing under",
        call(
            &fixture.router,
            "StartExportTask",
            &export_request(&fixture, identifier, &source_arn, &absent_key),
        )
        .await,
        400,
        "KMSKeyNotAccessibleFault",
        &key_not_accessible(&absent_key),
    );

    // And once the key is one KMS will encrypt under, the identifier is what refuses it.
    expect_error(
        "an identifier already running, with an accessible key",
        call(
            &fixture.router,
            "StartExportTask",
            &export_request(&fixture, identifier, &source_arn, &fixture.enabled_key),
        )
        .await,
        400,
        "ExportTaskAlreadyExists",
    );
}
