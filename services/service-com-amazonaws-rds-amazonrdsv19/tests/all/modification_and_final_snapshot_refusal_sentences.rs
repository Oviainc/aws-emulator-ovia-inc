// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for two refusal sentences this front door answers with NO variable
//! part, where the emulator used to build one out of the request.
//!
//! The first rule: the operation that MODIFIES a DB cluster's configuration refuses an
//! identifier it cannot resolve with `DBClusterNotFoundFault` (HTTP 404, the `awsQueryError`
//! code `DBClusterNotFoundFault` — `smithy_query(query="operation",
//! operation="ModifyDBCluster")`) carrying ONE fixed sentence: nothing the request named is
//! interpolated into it, so the sentence has the same text — and the same cardinality of one
//! — for every identifier a caller can send. The table spans what the rule generalises over:
//! the three forms the member accepts ("the user-supplied DB cluster identifier or the Amazon
//! Resource Name (ARN)", "this parameter isn't case-sensitive"), the published
//! 1-to-63-character bound of the identifier, which settings the refused modification
//! carries (none, a number, a boolean, a rename), the region the call is signed for, and
//! WHY the lookup fails (an identifier that never existed, and one whose cluster was
//! deleted a moment ago). The REPORT path is asserted beside it, because this front door
//! words the one fault by the role the cluster plays in the call and that role's template
//! does name the identifier.
//!
//! The second rule: a delete that names NEITHER of its two final-snapshot members is
//! refused `InvalidParameterCombination` (HTTP 400) with one fixed sentence naming the
//! member the request has to carry and the member that waives it, whichever resource is
//! being deleted and whatever the request named. The table spans the three deletes that
//! carry the pair, several engines and cluster types, the spelling of the identifier, a
//! resource the clock has not settled yet — and, for each row, the same call with
//! `SkipFinalSnapshot` named, which gets past the guard.
//!
//! Both sentences are spelled out HERE rather than taken from the code under test, so the
//! table pins the wording instead of re-stating whatever the implementation happens to say.
//!
//! Every call goes over the wire through `db_instances::emulator`, the one mounting helper
//! this test target has, as a real awsQuery form POST carrying the SigV4 credential scope
//! the router keys on.

use std::collections::BTreeSet;

use service_com_amazonaws_rds_amazonrdsv19::state::db_cluster_identifier_max_len;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_cluster_subject_operations::{cluster_arn, create_available_cluster};
use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, text, wait_for_settle,
};

/// The sentence the modification path answers `DBClusterNotFoundFault` with. It has no
/// variable part at all: the identifier the request carried is deliberately absent.
const MODIFY_NOT_FOUND: &str = "DBCluster not found";

/// The sentence a delete that names neither of its two final-snapshot members answers.
const FINAL_SNAPSHOT_REQUIRED: &str =
    "FinalDBSnapshotIdentifier is required unless SkipFinalSnapshot is specified.";

/// An identifier no call below ever creates a cluster for.
const ABSENT: &str = "emu-fixed-absent-cluster";

/// The one cluster the modification table keeps stored, so a 404 is a decision about the
/// identifier the request named and never "this service holds no cluster at all".
const PRESENT: &str = "emu-fixed-present-cluster";

/// A cluster the test creates and then deletes: the identifier that resolved a moment ago
/// and resolves no longer.
const DELETED: &str = "emu-fixed-deleted-cluster";

/// `(name, value)` pairs owned so a row can hold them.
fn owned(pairs: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
    pairs
        .iter()
        .map(|(name, value)| (*name, (*value).to_string()))
        .collect()
}

/// One row of the modification table: how the request names a cluster the lookup cannot
/// resolve, the region the call is signed for, and the settings the refused modification
/// carries.
struct ModifyCase {
    case: &'static str,
    named: String,
    region: &'static str,
    settings: Vec<(&'static str, String)>,
}

/// A `DBClusterIdentifier` of exactly the length its member admits ("Must contain from 1 to
/// 63 (for Aurora DB clusters) … letters, numbers, or hyphens"), read off the same published
/// bound the handlers check against.
fn identifier_at_the_published_bound(prefix: &str) -> String {
    let max = db_cluster_identifier_max_len("aurora-mysql");
    let mut identifier = prefix.to_string();
    identifier.extend(std::iter::repeat_n('a', max - prefix.len()));
    assert_eq!(
        identifier.len(),
        max,
        "the row at the published bound must be {max} characters, got {}",
        identifier.len()
    );
    identifier
}

/// The table: one row per way a caller can name a cluster the modification cannot resolve.
fn modify_cases() -> Vec<ModifyCase> {
    vec![
        ModifyCase {
            case: "the identifier as written, applying immediately",
            named: ABSENT.to_string(),
            region: crate::db_instances::REGION,
            settings: owned(&[("ApplyImmediately", "true")]),
        },
        ModifyCase {
            case: "the same identifier upper-cased, changing the backup retention",
            named: ABSENT.to_ascii_uppercase(),
            region: crate::db_instances::REGION,
            settings: owned(&[("BackupRetentionPeriod", "7")]),
        },
        ModifyCase {
            case: "the cluster's ARN form, clearing deletion protection",
            named: cluster_arn(ABSENT),
            region: crate::db_instances::REGION,
            settings: owned(&[("DeletionProtection", "false")]),
        },
        ModifyCase {
            case: "an identifier at the published 63-character bound, renaming the cluster",
            named: identifier_at_the_published_bound("emu-fixed-bound-"),
            region: crate::db_instances::REGION,
            settings: owned(&[("NewDBClusterIdentifier", "emu-fixed-renamed")]),
        },
        ModifyCase {
            case: "a cluster deleted a moment ago, with no settings at all",
            named: DELETED.to_string(),
            region: crate::db_instances::REGION,
            settings: Vec::new(),
        },
        ModifyCase {
            case: "a stored cluster named from another region",
            named: PRESENT.to_string(),
            region: "eu-central-1",
            settings: owned(&[("Port", "3307")]),
        },
    ]
}

/// `DescribeDBClusters` for one identifier.
async fn describe_cluster(
    router: &MultiServiceRouter,
    identifier: &str,
) -> (u16, String) {
    call(
        router,
        "DescribeDBClusters",
        &[("DBClusterIdentifier", identifier)],
    )
    .await
}

#[tokio::test]
async fn the_modification_path_words_its_not_found_fault_with_one_fixed_sentence() {
    let router = emulator();
    create_available_cluster(&router, PRESENT, "aurora-postgresql", &[]).await;
    create_available_cluster(&router, DELETED, "aurora-postgresql", &[]).await;
    expect_ok(
        "delete the cluster the table names",
        call(
            &router,
            "DeleteDBCluster",
            &[
                ("DBClusterIdentifier", DELETED),
                ("SkipFinalSnapshot", "true"),
            ],
        )
        .await,
    );
    wait_for_settle();

    // The REPORT path, the same fault on the same store: its template DOES name the
    // identifier, which is what keeps the two roles distinct — one wording for both would
    // fail here or in the loop below. It also shows the deleted cluster is really gone, so
    // the row about it meets the lookup rather than a state precondition.
    for named in [ABSENT, DELETED] {
        let case = format!("DescribeDBClusters on {named}");
        let body = expect_error(
            &case,
            describe_cluster(&router, named).await,
            404,
            "DBClusterNotFoundFault",
        );
        let message = text(&body, "Message");
        let expected = format!("DBCluster {named} not found.");
        assert_eq!(
            message, expected,
            "case {case:?}: expected the report path's wording {expected:?}, got {message:?}"
        );
    }

    let mut sentences: BTreeSet<String> = BTreeSet::new();
    for row in &modify_cases() {
        let case = row.case;
        let mut params: Vec<(&str, String)> =
            vec![("DBClusterIdentifier", row.named.clone())];
        params.extend(row.settings.iter().cloned());
        let body = expect_error(
            case,
            call_in(&router, row.region, "ModifyDBCluster", &params).await,
            404,
            "DBClusterNotFoundFault",
        );
        let message = text(&body, "Message");
        assert_eq!(
            message, MODIFY_NOT_FOUND,
            "case {case:?}: expected the fixed sentence {MODIFY_NOT_FOUND:?}, got {message:?}"
        );
        let folded = row.named.to_ascii_lowercase();
        assert!(
            !message.to_ascii_lowercase().contains(&folded),
            "case {case:?}: the refusal must not interpolate the identifier the request \
             named ({folded:?}), got {message:?}"
        );
        let kind = text(&body, "Type");
        assert_eq!(
            kind, "Sender",
            "case {case:?}: expected a Sender fault, got {kind:?} in {body}"
        );
        sentences.insert(message);
    }
    assert_eq!(
        sentences.len(),
        1,
        "the modification path must answer ONE sentence for every identifier, got \
         {sentences:?}"
    );

    // The stored cluster was there throughout — so none of the refusals came from an empty
    // store — and none of them changed it.
    let before = expect_ok(
        "the stored cluster",
        describe_cluster(&router, PRESENT).await,
    );
    let status = text(&before, "Status");
    assert_eq!(
        status, "available",
        "expected the stored cluster untouched by the refusals, got {status:?} in {before}"
    );

    // And the refusals were about the LOOKUP: the same modification, named in the region
    // the cluster is stored in, is taken and persisted.
    let accepted = expect_ok(
        "modify the stored cluster",
        call(
            &router,
            "ModifyDBCluster",
            &[
                ("DBClusterIdentifier", PRESENT),
                ("BackupRetentionPeriod", "7"),
                ("ApplyImmediately", "true"),
            ],
        )
        .await,
    );
    let retention = text(&accepted, "BackupRetentionPeriod");
    assert_eq!(
        retention, "7",
        "expected the accepted modification to report the retention it set, got \
         {retention:?} in {accepted}"
    );
}

/// What the SAME delete answers once `SkipFinalSnapshot` is named: the leg that shows the
/// refusal was about the member pair and about nothing else.
#[derive(Debug, Clone, Copy)]
enum PastTheGuard {
    /// The delete is taken: HTTP 200, with the resource's status member reporting the
    /// status it carries when the call returns — a DB instance reports `deleting` ("the
    /// status of the Amazon RDS DB instance is deleting"), while a DB cluster's delete
    /// acknowledges the cluster as it FOUND it and moves it on the next read.
    DeleteTaken {
        status_member: &'static str,
        status: &'static str,
    },
    /// The call reaches the lookup BEHIND the guard, which answers its own modelled fault.
    Fault { status: u16, code: &'static str },
}

/// One row of the delete table: the operation, the members that name its resource, and what
/// the same call answers once `SkipFinalSnapshot` is named.
struct DeleteCase {
    case: &'static str,
    action: &'static str,
    params: Vec<(&'static str, String)>,
    past_the_guard: PastTheGuard,
}

/// `CreateDBInstance` with its three required members plus `extra`.
async fn create_instance(
    router: &MultiServiceRouter,
    identifier: &str,
    engine: &str,
    extra: &[(&str, &str)],
) -> (u16, String) {
    let mut params = vec![
        ("DBInstanceIdentifier", identifier),
        ("Engine", engine),
        ("DBInstanceClass", "db.m5.large"),
    ];
    params.extend_from_slice(extra);
    call(router, "CreateDBInstance", &params).await
}

#[tokio::test]
async fn a_delete_that_names_neither_final_snapshot_member_answers_one_fixed_sentence() {
    let router = emulator();

    // The resources the rule is asserted over: a DB instance on three engines, both cluster
    // types this front door serves, and the Oracle container database a tenant database
    // lives in.
    for (identifier, engine) in [
        ("emu-nofinal-mysql", "mysql"),
        ("emu-nofinal-upper", "postgres"),
        ("emu-nofinal-cdb", "oracle-ee-cdb"),
    ] {
        expect_ok(
            identifier,
            create_instance(&router, identifier, engine, &[]).await,
        );
    }
    create_available_cluster(&router, "emu-nofinal-aurora", "aurora-postgresql", &[]).await;
    create_available_cluster(
        &router,
        "emu-nofinal-docdb",
        "docdb",
        &[
            ("MasterUsername", "emuadmin"),
            ("MasterUserPassword", "pw1-Secret"),
        ],
    )
    .await;
    // Created last and not waited out, so this row's delete arrives while the clock still
    // owes the instance its transition — a delete is taken whatever status it is in.
    expect_ok(
        "emu-nofinal-fresh",
        create_instance(&router, "emu-nofinal-fresh", "mariadb", &[]).await,
    );

    let table: Vec<DeleteCase> = vec![
        DeleteCase {
            case: "a MySQL DB instance",
            action: "DeleteDBInstance",
            params: owned(&[("DBInstanceIdentifier", "emu-nofinal-mysql")]),
            past_the_guard: PastTheGuard::DeleteTaken {
                status_member: "DBInstanceStatus",
                status: "deleting",
            },
        },
        DeleteCase {
            case: "a PostgreSQL DB instance named upper-cased",
            action: "DeleteDBInstance",
            params: owned(&[("DBInstanceIdentifier", "EMU-NOFINAL-UPPER")]),
            past_the_guard: PastTheGuard::DeleteTaken {
                status_member: "DBInstanceStatus",
                status: "deleting",
            },
        },
        DeleteCase {
            case: "a MariaDB instance the clock has not settled yet",
            action: "DeleteDBInstance",
            params: owned(&[("DBInstanceIdentifier", "emu-nofinal-fresh")]),
            past_the_guard: PastTheGuard::DeleteTaken {
                status_member: "DBInstanceStatus",
                status: "deleting",
            },
        },
        DeleteCase {
            case: "an Aurora PostgreSQL DB cluster",
            action: "DeleteDBCluster",
            params: owned(&[("DBClusterIdentifier", "emu-nofinal-aurora")]),
            past_the_guard: PastTheGuard::DeleteTaken {
                status_member: "Status",
                status: "available",
            },
        },
        DeleteCase {
            case: "a DocumentDB cluster",
            action: "DeleteDBCluster",
            params: owned(&[("DBClusterIdentifier", "emu-nofinal-docdb")]),
            past_the_guard: PastTheGuard::DeleteTaken {
                status_member: "Status",
                status: "available",
            },
        },
        DeleteCase {
            case: "a tenant database of an Oracle container database",
            action: "DeleteTenantDatabase",
            params: owned(&[
                ("DBInstanceIdentifier", "emu-nofinal-cdb"),
                ("TenantDBName", "emutenant"),
            ]),
            // The guard answers before the tenant database is resolved, so the same call
            // with `SkipFinalSnapshot` named reaches the lookup and reports the tenant
            // database it could not find.
            past_the_guard: PastTheGuard::Fault {
                status: 404,
                code: "TenantDatabaseNotFound",
            },
        },
    ];

    let mut sentences: BTreeSet<String> = BTreeSet::new();
    for row in &table {
        let case = row.case;
        let refused = expect_error(
            case,
            call(&router, row.action, &row.params).await,
            400,
            "InvalidParameterCombination",
        );
        let message = text(&refused, "Message");
        assert_eq!(
            message, FINAL_SNAPSHOT_REQUIRED,
            "case {case:?}: expected the fixed sentence {FINAL_SNAPSHOT_REQUIRED:?}, got \
             {message:?}"
        );
        for (member, value) in &row.params {
            assert!(
                !message.contains(value.as_str()),
                "case {case:?}: the refusal must name no value the request carried, but it \
                 carries {member}'s {value:?}: {message:?}"
            );
        }
        sentences.insert(message);

        // The refusal left the resource alone, and the same call with `SkipFinalSnapshot`
        // named gets past the guard: the pair of members is the only thing it was about.
        let mut params = row.params.clone();
        params.push(("SkipFinalSnapshot", "true".to_string()));
        match row.past_the_guard {
            PastTheGuard::DeleteTaken {
                status_member,
                status: expected,
            } => {
                let taken = expect_ok(case, call(&router, row.action, &params).await);
                let status = text(&taken, status_member);
                assert_eq!(
                    status, expected,
                    "case {case:?}: expected the accepted delete to report the resource as \
                     {expected:?}, got {status:?} in {taken}"
                );
            }
            PastTheGuard::Fault { status, code } => {
                expect_error(case, call(&router, row.action, &params).await, status, code);
            }
        }
    }
    assert_eq!(
        sentences.len(),
        1,
        "one refusal class, worded one way whichever resource is deleted, got {sentences:?}"
    );
}
