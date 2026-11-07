// SPDX-License-Identifier: CC0-1.0

//! The cluster shape an RDS Data API caller provisions: an Aurora Serverless v2 cluster
//! whose HTTP endpoint is on and whose master user password RDS manages in Amazon Web
//! Services Secrets Manager.
//!
//! The RULE under test: `CreateDBCluster` SERVES a request that asks RDS to manage the
//! master user password — it is accepted, and the cluster reports the managed secret as
//! `MasterUserSecret` — so the documented way of preparing a cluster for the Data API
//! (hand RDS the password management, then read `MasterUserSecret.SecretArn` and pass it as
//! the Data API's `secretArn`) resolves to a secret that exists. Nothing about the request
//! may be refused for a member the API documents as valid: every member below is one
//! `CreateDBCluster` publishes for an Aurora DB cluster.
//!
//! The rows span what that provisioning generalises over: both Aurora engine families the
//! Data API serves, a cluster that names its own capacity range beside one that leaves it
//! to RDS, and a cluster created in a DB subnet group beside one created without naming
//! one — so the rule is asserted over the class of Data API provisionings rather than over
//! the one shape a single check happens to send.
//!
//! Secrets Manager and KMS are mounted beside the RDS service (the emulator
//! `managed_master_user_password` mounts), because the create calls them; the secret the
//! cluster reports is read back through Secrets Manager's own `GetSecretValue`, so what the
//! ARN names is a secret this emulator really holds.

use serde_json::Value;

use crate::db_clusters::{describe_one, the_cluster};
use crate::db_instances::{call, expect_ok, subtrees, text, REGION};
use crate::managed_master_user_password::{emulator, secrets_manager};

/// One Data API provisioning.
struct ProvisioningCase {
    case: &'static str,
    identifier: &'static str,
    engine: &'static str,
    master_username: &'static str,
    /// The members this row adds beyond the required ones, the HTTP endpoint and the
    /// managed master password.
    extra: &'static [(&'static str, &'static str)],
}

/// The clusters this suite provisions the way a Data API caller does.
const PROVISIONINGS: &[ProvisioningCase] = &[
    ProvisioningCase {
        case: "Aurora MySQL, Serverless v2 capacity named, encrypted, in a subnet group",
        identifier: "emu-dataapi-aurora-mysql",
        engine: "aurora-mysql",
        master_username: "dbadmin",
        extra: &[
            ("ServerlessV2ScalingConfiguration.MinCapacity", "0.5"),
            ("ServerlessV2ScalingConfiguration.MaxCapacity", "2.0"),
            ("StorageEncrypted", "true"),
            ("DBSubnetGroupName", "default"),
        ],
    },
    ProvisioningCase {
        case: "Aurora PostgreSQL, nothing but the required members beside the two settings",
        identifier: "emu-dataapi-aurora-pg",
        engine: "aurora-postgresql",
        master_username: "pgadmin",
        extra: &[],
    },
];

#[tokio::test]
async fn a_data_api_provisioning_is_served_and_its_secret_holds_the_credentials() {
    let router = emulator();
    let secrets = secrets_manager(&router);
    for row in PROVISIONINGS {
        let mut params: Vec<(&str, String)> = vec![
            ("DBClusterIdentifier", row.identifier.to_string()),
            ("Engine", row.engine.to_string()),
            ("MasterUsername", row.master_username.to_string()),
            // The two settings every Data API caller turns on: the HTTP endpoint the Data
            // API is served over, and RDS-managed master credentials.
            ("EnableHttpEndpoint", "true".to_string()),
            ("ManageMasterUserPassword", "true".to_string()),
        ];
        params.extend(
            row.extra
                .iter()
                .map(|(name, value)| (*name, (*value).to_string())),
        );
        let body = expect_ok(row.case, call(&router, "CreateDBCluster", &params).await);
        let cluster = the_cluster(row.case, &body);

        // The HTTP endpoint the Data API needs is on, and the cluster carries the ARN a
        // Data API request addresses as its `resourceArn`.
        let http_endpoint = text(&cluster, "HttpEndpointEnabled");
        assert_eq!(
            http_endpoint, "true",
            "case {:?}: expected the HTTP endpoint to be enabled, got {http_endpoint:?}",
            row.case
        );
        let cluster_arn = text(&cluster, "DBClusterArn");
        assert!(
            cluster_arn.starts_with(&format!("arn:aws:rds:{REGION}:")),
            "case {:?}: expected the cluster's own ARN, got {cluster_arn:?}",
            row.case
        );

        // The managed secret: its ARN is what a Data API request carries as `secretArn`, so
        // it has to name a secret Secrets Manager really holds, holding this cluster's
        // master user credentials.
        let secret = subtrees(&cluster, "MasterUserSecret")
            .into_iter()
            .next()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: a managed-password create reports its MasterUserSecret: \
                     {cluster}",
                    row.case
                )
            });
        let secret_arn = text(&secret, "SecretArn");
        assert!(
            secret_arn.starts_with(&format!("arn:aws:secretsmanager:{REGION}:")),
            "case {:?}: expected a Secrets Manager secret ARN, got {secret_arn:?}",
            row.case
        );
        let value = secrets
            .get_secret_value()
            .secret_id(&secret_arn)
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "case {:?}: the reported SecretArn {secret_arn:?} must name a secret this \
                     emulator holds, got {}",
                    row.case,
                    aws_smithy_types::error::display::DisplayErrorContext(&error)
                )
            });
        let document: Value = serde_json::from_str(
            value
                .secret_string()
                .unwrap_or_else(|| panic!("case {:?}: the secret holds a string value", row.case)),
        )
        .unwrap_or_else(|error| {
            panic!(
                "case {:?}: the secret's value must be the credentials document, got {error}",
                row.case
            )
        });
        let held_username = document.get("username").and_then(Value::as_str);
        assert_eq!(
            held_username,
            Some(row.master_username),
            "case {:?}: expected the secret to hold this cluster's master user, got \
             {held_username:?}",
            row.case
        );

        // The acknowledgement IS the record: a later read reports the same secret, so a
        // caller that polls the cluster before using the Data API reads the same ARN.
        let described = describe_one(&router, row.case, row.identifier).await;
        let described_secret = subtrees(&described, "MasterUserSecret")
            .into_iter()
            .next()
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: DescribeDBClusters must report the managed secret too: \
                     {described}",
                    row.case
                )
            });
        assert_eq!(
            text(&described_secret, "SecretArn"),
            secret_arn,
            "case {:?}: DescribeDBClusters must report the secret the create stored",
            row.case
        );
        assert_eq!(
            text(&described, "HttpEndpointEnabled"),
            "true",
            "case {:?}: DescribeDBClusters must report the HTTP endpoint as enabled too",
            row.case
        );
    }
}
