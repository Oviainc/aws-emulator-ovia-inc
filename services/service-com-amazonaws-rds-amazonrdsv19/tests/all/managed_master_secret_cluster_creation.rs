// SPDX-License-Identifier: CC0-1.0

//! Creating an Aurora DB cluster whose master user password RDS manages, the way a client
//! creates one: a DB subnet group first, then `CreateDBCluster`.
//!
//! The RULE under test: a `CreateDBCluster` that turns `ManageMasterUserPassword` on and
//! names no `MasterUserPassword` is SERVED — RDS generates the master user password into a
//! secret it owns in Secrets Manager, in the cluster's own region, and reports that secret
//! as `DBCluster$MasterUserSecret`. The operation models no `NotImplemented` error, so no
//! member of that contract makes the request unsupported: the answer is a cluster, never a
//! 501.
//!
//! The sibling suite [`crate::managed_master_user_password`] pins what the secret HOLDS and
//! what the modelled refusals are. This suite pins the request path the rule has to hold on
//! — a cluster placed in a DB subnet group the caller created, in more than one region —
//! and the round trip through `DescribeDBClusters` that proves the cluster the create
//! answered with is really stored.
//!
//! The rows differ along what the rule generalises over: the engine family that reaches the
//! contract through a DB CLUSTER (Aurora MySQL, Aurora PostgreSQL, Neptune), whether the
//! request places the cluster in a DB subnet group of its own or leaves RDS to place it,
//! whether it names a `MasterUserSecretKmsKeyId`, whether it names a `MasterUsername` at
//! all, and the region the call is signed for.

use service_com_amazonaws_rds_amazonrdsv19::state as st;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call_in, expect_ok, optional_text, text, wait_for_settle, REGION};
use crate::managed_master_user_password::{
    emulator, held_credentials_in, kms_in, master_user_secret, the_resource, ACCOUNT,
};

/// A second region, so the rule is asserted somewhere other than the suite's default: the
/// secret belongs to the cluster, so it is created where the cluster lives.
const OTHER_REGION: &str = "eu-central-1";

/// The two subnets a DB subnet group must name at least, in the published subnet-id form.
const TWO_SUBNETS: [&str; 2] = ["subnet-0123456789abcdef0", "subnet-0fedcba987654321f"];

/// `CreateDBSubnetGroup` in `region`, over [`TWO_SUBNETS`].
async fn create_subnet_group(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
) -> (u16, String) {
    let [first, second] = TWO_SUBNETS;
    let params: [(&str, &str); 4] = [
        ("DBSubnetGroupName", name),
        (
            "DBSubnetGroupDescription",
            "the network a managed-secret cluster is placed in",
        ),
        ("SubnetIds.SubnetIdentifier.1", first),
        ("SubnetIds.SubnetIdentifier.2", second),
    ];
    call_in(router, region, "CreateDBSubnetGroup", &params).await
}

/// One row of the table: the cluster a request asks RDS to manage the password of.
struct ManagedClusterCase {
    case: &'static str,
    region: &'static str,
    engine: &'static str,
    /// `MasterUsername`, or `None` for an engine whose control plane assigns the master
    /// user itself (`st::assigned_master_username`).
    master_username: Option<&'static str>,
    /// Whether the request places the cluster in a DB subnet group of its own.
    names_subnet_group: bool,
    /// Whether the request names a customer managed key for the secret.
    names_kms_key: bool,
    /// The parameters this row adds beyond the identifier, the engine and the master user.
    extra: &'static [(&'static str, &'static str)],
}

/// The rows. The engines are the cluster-shaped ones this service publishes, and the
/// remaining columns are the choices the contract leaves a caller.
const CASES: &[ManagedClusterCase] = &[
    ManagedClusterCase {
        case: "an Aurora PostgreSQL cluster in a subnet group, with the Data API turned on",
        region: REGION,
        engine: "aurora-postgresql",
        master_username: Some("pgadmin"),
        names_subnet_group: true,
        names_kms_key: false,
        extra: &[("EnableHttpEndpoint", "true")],
    },
    ManagedClusterCase {
        case: "an Aurora MySQL cluster in a subnet group, on a key of its own",
        region: REGION,
        engine: "aurora-mysql",
        master_username: Some("dbadmin"),
        names_subnet_group: true,
        names_kms_key: true,
        extra: &[("EnableHttpEndpoint", "true")],
    },
    ManagedClusterCase {
        case: "an Aurora MySQL cluster RDS places itself, in another region",
        region: OTHER_REGION,
        engine: "aurora-mysql",
        master_username: Some("dbadmin"),
        names_subnet_group: false,
        names_kms_key: false,
        extra: &[],
    },
    ManagedClusterCase {
        case: "an Aurora PostgreSQL Serverless v2 cluster in a subnet group",
        region: OTHER_REGION,
        engine: "aurora-postgresql",
        master_username: Some("pgadmin"),
        names_subnet_group: true,
        names_kms_key: true,
        extra: &[
            ("ServerlessV2ScalingConfiguration.MinCapacity", "0.5"),
            ("ServerlessV2ScalingConfiguration.MaxCapacity", "4.0"),
        ],
    },
    ManagedClusterCase {
        case: "a Neptune cluster, whose master user the control plane assigns",
        region: REGION,
        engine: "neptune",
        master_username: None,
        names_subnet_group: true,
        names_kms_key: false,
        extra: &[],
    },
];

#[tokio::test]
async fn a_managed_master_password_cluster_is_created_and_reports_its_secret() {
    let router = emulator();
    // A key of the account's own per region a row names one in: the key encrypting a secret
    // lives where the secret does, so each is minted through KMS in that row's region.
    let mut account_keys: Vec<(&'static str, String, String)> = Vec::new();
    for row in CASES.iter().filter(|row| row.names_kms_key) {
        if account_keys.iter().any(|(region, ..)| *region == row.region) {
            continue;
        }
        let key = kms_in(&router, row.region)
            .create_key()
            .description("the key a managed master user secret is encrypted under")
            .send()
            .await
            .unwrap_or_else(|error| {
                panic!("KMS mints a customer managed key in {}: {error}", row.region)
            });
        let metadata = key
            .key_metadata()
            .expect("a minted key reports its metadata");
        account_keys.push((
            row.region,
            metadata.key_id().to_string(),
            metadata
                .arn()
                .expect("a minted key reports its ARN")
                .to_string(),
        ));
    }

    let mut seen_arns: Vec<String> = Vec::new();
    for (index, row) in CASES.iter().enumerate() {
        let identifier = format!("emu-mms-{index}");
        let subnet_group = format!("emu-mms-subnets-{index}");
        if row.names_subnet_group {
            expect_ok(
                row.case,
                create_subnet_group(&router, row.region, &subnet_group).await,
            );
        }
        let mut params: Vec<(&str, String)> = vec![
            ("DBClusterIdentifier", identifier.clone()),
            ("Engine", row.engine.to_string()),
            ("ManageMasterUserPassword", "true".to_string()),
        ];
        if let Some(username) = row.master_username {
            params.push(("MasterUsername", username.to_string()));
        }
        if row.names_subnet_group {
            params.push(("DBSubnetGroupName", subnet_group.clone()));
        }
        // The key this row's secret is encrypted under, for a row that names one.
        let named_key = account_keys
            .iter()
            .find(|(region, ..)| *region == row.region)
            .filter(|_| row.names_kms_key);
        if let Some((_, key_id, _)) = named_key {
            params.push(("MasterUserSecretKmsKeyId", key_id.clone()));
        }
        params.extend(
            row.extra
                .iter()
                .map(|(name, value)| (*name, (*value).to_string())),
        );

        let response = call_in(&router, row.region, "CreateDBCluster", &params).await;
        // The refusal this rule rules out, named so a regression says what came back
        // rather than only that the status was not 200.
        assert!(
            !response.1.contains("NotImplemented"),
            "case {:?}: the managed master user password contract is served, so the request \
             must not be refused as unimplemented; got HTTP {} with body {}",
            row.case,
            response.0,
            response.1
        );
        let created = expect_ok(row.case, response);
        let envelope = the_resource(row.case, &created, "DBCluster");

        // The cluster is the one the request asked for, placed where it asked.
        assert_eq!(
            text(&envelope, "DBClusterIdentifier"),
            identifier,
            "case {:?}: expected the cluster the request named, got {envelope}",
            row.case
        );
        // A cluster placed in a DB subnet group the request named reports that group. What a
        // cluster the request named none for reports is a different rule and a different
        // issue's, so this suite asserts nothing about it.
        if row.names_subnet_group {
            assert_eq!(
                optional_text(&envelope, "DBSubnetGroup").as_deref(),
                Some(subnet_group.as_str()),
                "case {:?}: expected the DB subnet group the request named, got {envelope}",
                row.case
            );
        }
        // The master user is the one the request named, else the one this engine's control
        // plane assigns (`st::assigned_master_username`).
        let expected_username = row
            .master_username
            .or_else(|| st::assigned_master_username(row.engine));
        assert_eq!(
            optional_text(&envelope, "MasterUsername").as_deref(),
            expected_username,
            "case {:?}: expected master user {expected_username:?}, got {envelope}",
            row.case
        );

        // The secret RDS manages for the password it generated, reported on the cluster.
        let secret = master_user_secret(row.case, &envelope).unwrap_or_else(|| {
            panic!(
                "case {:?}: expected the cluster to report MasterUserSecret, got {envelope}",
                row.case
            )
        });
        let secret_arn = text(&secret, "SecretArn");
        assert!(
            secret_arn.starts_with(&format!(
                "arn:aws:secretsmanager:{}:{ACCOUNT}:secret:",
                row.region
            )),
            "case {:?}: expected a Secrets Manager secret ARN in the cluster's own region \
             ({}), got {secret_arn:?}",
            row.case,
            row.region
        );
        assert_eq!(
            text(&secret, "SecretStatus"),
            st::MASTER_USER_SECRET_STATUS_CREATING,
            "case {:?}: a secret this call just created is still being created, got {secret}",
            row.case
        );
        // `KmsKeyId` is the key the secret is encrypted under, as KMS identifies it: the
        // key the request named, else the Amazon Web Services managed key for Secrets
        // Manager, which lives in the cluster's own region.
        let reported_key = text(&secret, "KmsKeyId");
        if let Some((_, _, key_arn)) = named_key {
            assert_eq!(
                &reported_key, key_arn,
                "case {:?}: expected the ARN of the key the request named, got \
                 {reported_key:?}",
                row.case
            );
        } else {
            assert!(
                reported_key.starts_with(&format!("arn:aws:kms:{}:{ACCOUNT}:key/", row.region)),
                "case {:?}: expected the ARN of the Amazon Web Services managed key for \
                 Secrets Manager in {}, got {reported_key:?}",
                row.case,
                row.region
            );
        }
        assert!(
            !seen_arns.contains(&secret_arn),
            "case {:?}: every cluster gets a secret of its own, but {secret_arn} was already \
             reported for another",
            row.case
        );
        seen_arns.push(secret_arn.clone());

        // The ARN names a secret this emulator's Secrets Manager really holds, whose value
        // is the credentials of the cluster's master user.
        let held = held_credentials_in(row.case, &router, row.region, &secret_arn).await;
        assert_eq!(
            held.username.as_deref(),
            expected_username,
            "case {:?}: expected the secret to hold the cluster's master user, got {:?}",
            row.case,
            held.username
        );
        assert_eq!(
            st::check_master_user_password(row.engine, &held.password),
            Ok(()),
            "case {:?}: the generated password must satisfy MasterUserPassword for engine \
             {}, and it has {} characters",
            row.case,
            row.engine,
            held.password.chars().count()
        );
        // The generated password went into the secret and nowhere else: no member of the
        // create's answer carries it.
        assert!(
            !created.contains(&held.password),
            "case {:?}: the generated master user password is sensitive and no member of the \
             response reports it, but a {}-character run of the response matches it",
            row.case,
            held.password.chars().count()
        );

        // The round trip: the cluster the create answered with is what the store holds, and
        // the statuses the clock owed have settled by the time it is read.
        wait_for_settle();
        let described = expect_ok(
            row.case,
            call_in(
                &router,
                row.region,
                "DescribeDBClusters",
                &[("DBClusterIdentifier", identifier.as_str())],
            )
            .await,
        );
        let stored = the_resource(row.case, &described, "DBCluster");
        assert_eq!(
            text(&stored, "Status"),
            st::DB_CLUSTER_STATUS_AVAILABLE,
            "case {:?}: expected the cluster to have reached available, got {stored}",
            row.case
        );
        assert_eq!(
            text(&stored, "DBClusterIdentifier"),
            identifier,
            "case {:?}: expected the describe to report the cluster the create stored, got \
             {described}",
            row.case
        );
        let stored_secret = master_user_secret(row.case, &stored).unwrap_or_else(|| {
            panic!(
                "case {:?}: expected the stored cluster to report MasterUserSecret, got \
                 {described}",
                row.case
            )
        });
        assert_eq!(
            text(&stored_secret, "SecretArn"),
            secret_arn,
            "case {:?}: expected the describe to report the secret the create minted, got \
             {stored_secret}",
            row.case
        );
        assert_eq!(
            text(&stored_secret, "SecretStatus"),
            st::MASTER_USER_SECRET_STATUS_ACTIVE,
            "case {:?}: expected the secret to be active once its span has passed, got \
             {stored_secret}",
            row.case
        );
        // And reading the cluster did not change the credentials its secret holds. The two
        // values are compared without being formatted: a live credential belongs in neither
        // a passing nor a failing test's output, so the failure reports their lengths.
        let reread = held_credentials_in(row.case, &router, row.region, &secret_arn).await;
        assert!(
            reread.password == held.password,
            "case {:?}: reading a cluster must not change the password its secret holds \
             ({} characters before the read, {} after)",
            row.case,
            held.password.chars().count(),
            reread.password.chars().count()
        );
    }
}
