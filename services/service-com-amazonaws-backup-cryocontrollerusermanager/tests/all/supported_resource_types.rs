// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage for `GetSupportedResourceTypes`, Backup's
//! capability-discovery call.
//!
//! Every case drives real HTTP through the router, so what is asserted is the
//! wire behaviour a `backup` SDK client sees. The rules under test:
//!
//! * the operation takes no input, so it answers 200 with the service's
//!   catalog of supported resource types on EVERY request — the same list each
//!   time, whatever the request carries and whatever the account holds;
//! * the catalog is exactly the set the service supports, each entry a
//!   non-empty string within the length the `ResourceType` shape admits,
//!   listed once;
//! * it is a reporting call: it stores nothing, so a later read sees what the
//!   earlier writes left and nothing else;
//! * the catalog is the service's single one — every `ResourceType` this
//!   service derives for a protected resource is advertised by it.

use std::collections::BTreeSet;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::vault_and_backup_job::{call, create_vault, router, start_job, string_at};

/// The operation's route (`GET /supported-resource-types`, `OPERATIONS.md`).
const PATH: &str = "/supported-resource-types";

/// The resource types the model documents for the member this operation
/// returns, each with the service it stands for.
///
/// SOURCE: the `ResourceTypes` member's own documentation —
/// `smithy_query(query="doc", shape="GetSupportedResourceTypesOutput",
/// member="ResourceTypes",
/// model_path="services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json")`.
/// The second column is that documentation's gloss, used as the case label so a
/// failure names the service whose type is missing.
const MODELLED_RESOURCE_TYPES: &[(&str, &str)] = &[
    ("Aurora", "Amazon Aurora"),
    ("CloudFormation", "CloudFormation"),
    ("DocumentDB", "Amazon DocumentDB (with MongoDB compatibility)"),
    ("DynamoDB", "Amazon DynamoDB"),
    ("EBS", "Amazon Elastic Block Store"),
    ("EC2", "Amazon Elastic Compute Cloud"),
    ("EFS", "Amazon Elastic File System"),
    ("EKS", "Amazon Elastic Kubernetes Service"),
    ("FSx", "Amazon FSx"),
    ("Neptune", "Amazon Neptune"),
    ("RDS", "Amazon Relational Database Service"),
    ("Redshift", "Amazon Redshift"),
    ("S3", "Amazon Simple Storage Service (Amazon S3)"),
    (
        "SAP HANA on Amazon EC2",
        "SAP HANA databases on Amazon Elastic Compute Cloud instances",
    ),
    ("Storage Gateway", "Storage Gateway"),
    ("Timestream", "Amazon Timestream"),
    ("VirtualMachine", "VMware virtual machines"),
];

/// The types for services AWS Backup supports whose spelling the member
/// documentation above predates; AWS reports them in the list today. Kept apart
/// from the modelled rows only to keep each row's provenance visible.
const NEWER_RESOURCE_TYPES: &[(&str, &str)] = &[
    ("DSQL", "Aurora DSQL"),
    ("Redshift Serverless", "Amazon Redshift Serverless"),
];

/// Every type the catalog must advertise: the modelled rows plus the newer
/// ones, which is what `state::SUPPORTED_RESOURCE_TYPES` holds.
fn expected_resource_types() -> Vec<(&'static str, &'static str)> {
    MODELLED_RESOURCE_TYPES
        .iter()
        .chain(NEWER_RESOURCE_TYPES)
        .copied()
        .collect()
}

/// The length bound the `ResourceType` shape's `@pattern`
/// (`^[a-zA-Z0-9\-\_\.]{1,50}$`) puts on one entry.
const RESOURCE_TYPE_LENGTH: std::ops::RangeInclusive<usize> = 1..=50;

/// Call the operation at `path` and return its `ResourceTypes` list, asserting
/// the status and the shape of the answer on the way through.
async fn supported_resource_types(
    router: &MultiServiceRouter,
    case: &str,
    path: &str,
) -> Vec<String> {
    let (status, body) = call(router, "GET", path, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetSupportedResourceTypes 200, got {status} {body:?}"
    );
    let listed = body
        .get("ResourceTypes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        !listed.is_empty(),
        "case {case:?}: expected a non-empty ResourceTypes list, got {body:?}"
    );
    listed
        .iter()
        .map(|entry| {
            let value = entry.as_str().unwrap_or_default().to_string();
            assert!(
                RESOURCE_TYPE_LENGTH.contains(&value.chars().count()),
                "case {case:?}: every entry must be a string within the modelled 1..=50 \
                 length, got {entry:?}"
            );
            value
        })
        .collect()
}

/// The operation reports the service's catalog, and reports the SAME catalog
/// however the request is shaped and whatever the account holds.
///
/// The table's rows differ along everything that could reach this operation: a
/// bare request, the same request repeated, a request carrying a query
/// parameter the operation does not model, and a request made after the account
/// has acquired a vault, a backup job and a recovery point. None of them is
/// input to the answer, so all four must agree.
#[tokio::test]
async fn the_catalog_is_reported_for_every_request() {
    let router = router();

    let mut answers: Vec<(&str, Vec<String>)> = Vec::new();
    answers.push((
        "a bare request",
        supported_resource_types(&router, "a bare request", PATH).await,
    ));
    answers.push((
        "the same request repeated",
        supported_resource_types(&router, "the same request repeated", PATH).await,
    ));
    answers.push((
        "a query parameter the operation does not model",
        supported_resource_types(
            &router,
            "a query parameter the operation does not model",
            &format!("{PATH}?unmodelled=1&MaxResults=2"),
        )
        .await,
    ));

    // Give the account some state, so the next call is made against a store
    // that is no longer empty.
    let (status, created) = create_vault(&router, "catalog-vault", "{}").await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBackupVault 200, got {status} {created:?}"
    );
    let role = "arn:aws:iam::000000000000:role/service-role/AWSBackupDefaultServiceRole";
    let body = format!(
        r#"{{"BackupVaultName":"catalog-vault",
             "ResourceArn":"arn:aws:dynamodb:us-west-2:000000000000:table/demo",
             "IamRoleArn":"{role}"}}"#
    );
    let (status, started) = start_job(&router, &body).await;
    assert_eq!(
        status, 200,
        "setup: expected StartBackupJob 200, got {status} {started:?}"
    );
    answers.push((
        "a request against an account that holds resources",
        supported_resource_types(
            &router,
            "a request against an account that holds resources",
            PATH,
        )
        .await,
    ));

    let (first_case, first) = answers[0].clone();
    for (case, answer) in &answers {
        assert_eq!(
            answer, &first,
            "case {case:?}: the catalog cannot depend on the request or on stored \
             resources — case {first_case:?} answered {first:?}, this one {answer:?}"
        );

        let reported: BTreeSet<&str> = answer.iter().map(String::as_str).collect();
        assert_eq!(
            reported.len(),
            answer.len(),
            "case {case:?}: expected each resource type listed once, got {answer:?}"
        );
        let expected = expected_resource_types();
        for (resource_type, service) in &expected {
            assert!(
                reported.contains(resource_type),
                "case {case:?}: expected the type for {service} \
                 ({resource_type:?}) to be supported, got {answer:?}"
            );
        }
        assert_eq!(
            reported.len(),
            expected.len(),
            "case {case:?}: expected exactly the catalog's {} types, got {answer:?}",
            expected.len()
        );
    }

    // A reporting operation changes nothing: the vault and the job the setup
    // above created are all the account holds after four calls.
    let (status, listed) = call(&router, "GET", "/backup-vaults", None).await;
    assert_eq!(
        status, 200,
        "expected ListBackupVaults 200, got {status} {listed:?}"
    );
    let names: Vec<String> = listed["BackupVaultList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| string_at(entry, "BackupVaultName"))
        .collect();
    assert_eq!(
        names,
        vec!["catalog-vault".to_string()],
        "reading the catalog must not change what a later caller sees, got {names:?}"
    );
}

/// The catalog is the service's single one: every `ResourceType` this service
/// derives for a protected resource is a type the catalog advertises.
///
/// The rows span every service namespace the derivation recognises, including
/// both namespaces that carry two Backup types (an `rds` cluster is `Aurora`
/// and an `rds` instance is `RDS`; an `ec2` volume is `EBS` and an `ec2`
/// instance is `EC2`).
#[tokio::test]
async fn every_derived_resource_type_is_advertised_as_supported() {
    let router = router();
    let supported: BTreeSet<String> = supported_resource_types(&router, "setup", PATH)
        .await
        .into_iter()
        .collect();

    let (status, created) = create_vault(&router, "derived-vault", "{}").await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBackupVault 200, got {status} {created:?}"
    );
    let role = "arn:aws:iam::000000000000:role/service-role/AWSBackupDefaultServiceRole";

    let resources = [
        "arn:aws:dynamodb:us-west-2:000000000000:table/demo",
        "arn:aws:elasticfilesystem:us-west-2:000000000000:file-system/fs-01234567",
        "arn:aws:fsx:us-west-2:000000000000:file-system/fs-89abcdef",
        "arn:aws:s3:::demo-bucket",
        "arn:aws:neptune:us-west-2:000000000000:cluster:neptune-demo",
        "arn:aws:docdb:us-west-2:000000000000:cluster:docdb-demo",
        "arn:aws:redshift:us-west-2:000000000000:cluster:redshift-demo",
        "arn:aws:timestream:us-west-2:000000000000:database/demo",
        "arn:aws:cloudformation:us-west-2:000000000000:stack/demo/0123",
        "arn:aws:rds:us-west-2:000000000000:cluster:aurora-demo",
        "arn:aws:rds:us-west-2:000000000000:db:postgres-demo",
        "arn:aws:ec2:us-west-2:000000000000:volume/vol-01234567",
        "arn:aws:ec2:us-west-2:000000000000:instance/i-01234567",
    ];

    for resource_arn in resources {
        let body = format!(
            r#"{{"BackupVaultName":"derived-vault","ResourceArn":"{resource_arn}","IamRoleArn":"{role}"}}"#
        );
        let (status, started) = start_job(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {resource_arn:?}: expected StartBackupJob 200, got {status} {started:?}"
        );
        let job_id = string_at(&started, "BackupJobId");
        let (status, described) = call(&router, "GET", &format!("/backup-jobs/{job_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {resource_arn:?}: expected DescribeBackupJob 200, got {status} {described:?}"
        );
        let derived = string_at(&described, "ResourceType");
        assert!(
            supported.contains(&derived),
            "case {resource_arn:?}: the derived ResourceType {derived:?} must be one \
             GetSupportedResourceTypes advertises, which are {supported:?}"
        );
    }
}
