// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the creates this service's PUBLISHED catalogues admit: a DB
//! cluster joining a global database, a DB instance ordered in an instance class, and a
//! DB security group named by a member the model leaves unconstrained.
//!
//! The rules under test, each asserted over a table spanning the class it generalises
//! over rather than over one request:
//!
//! * a create provisions any engine VERSION `DescribeDBEngineVersions` publishes as
//!   available for the engine, and only those — so the rows are the report's own entries,
//!   read back from the emulator, plus a well-formed build it publishes no entry for;
//! * a DB instance is created only in a combination `DescribeOrderableDBInstanceOptions`
//!   offers: the instance GENERATION of its class and a license model of its engine. A
//!   combination it offers none of is refused for the combination — and stores nothing, so
//!   the identifier it asked for is still free for a create that is offered;
//! * `DBSecurityGroupName` is a plain string in the model, so a name outside the
//!   identifier alphabet the other name members state is created, read back and deleted
//!   like any other.
//!
//! Every call goes over the wire through the harness in `db_instances.rs`, so each
//! assertion reads the XML this emulator itself answered with.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_rds_amazonrdsv19::state::region_vpc_id;

use crate::db_engine_versions::all_versions_of;
use crate::db_instances::{
    call, emulator, expect_error, expect_ok, text, texts, wait_for_settle, REGION,
};

/// The account id every generated emulator presents, as the sibling suites assert it in
/// an ARN.
const ACCOUNT: &str = "000000000000";

/// The engine whose global-database topology the cluster table below builds: a global
/// database is an Aurora resource, and `CreateGlobalCluster$Engine` publishes
/// `aurora-mysql` as one of its two valid values.
const GLOBAL_ENGINE: &str = "aurora-mysql";

/// The port an Aurora MySQL cluster listens on, whatever version it runs.
const AURORA_MYSQL_PORT: &str = "3306";

/// The status a freshly created DB cluster reports.
const CREATING: &str = "creating";

/// `CreateDBSubnetGroup` over two subnets, the fewest a group may name.
async fn create_subnet_group(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(
        router,
        "CreateDBSubnetGroup",
        &[
            ("DBSubnetGroupName", name),
            ("DBSubnetGroupDescription", "published catalogue creates"),
            ("SubnetIds.SubnetIdentifier.1", "subnet-0123456789abcdef0"),
            ("SubnetIds.SubnetIdentifier.2", "subnet-0fedcba987654321f"),
        ],
    )
    .await
}

/// The ARN a DB cluster stored in [`REGION`] under `identifier` reports.
fn cluster_arn(identifier: &str) -> String {
    format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:{identifier}")
}

/// Every version of `engine` the report publishes as AVAILABLE, as `(version, family)`.
///
/// Read from `DescribeDBEngineVersions` rather than written out here: the rule is about
/// the versions this service publishes, so the table is whatever that report answers.
async fn available_versions(router: &MultiServiceRouter, engine: &str) -> Vec<(String, String)> {
    all_versions_of(router, engine)
        .await
        .into_iter()
        .filter(|version| version.status == "available")
        .map(|version| (version.engine_version, version.parameter_group_family))
        .collect()
}

/// A DB cluster can JOIN a global database at every engine version the catalogue
/// publishes, and a version it publishes none of is refused without storing a cluster.
///
/// The rows are the published Aurora MySQL builds — both major-version families, every
/// available build of each — so a family whose rows stop short of the build an engine is
/// created with today would fail here.
#[tokio::test]
async fn a_cluster_joins_a_global_database_at_every_published_engine_version() {
    let router = emulator();
    let subnet_group = "emu-pcc-subnets";
    expect_ok(
        "the subnet group the members are placed in",
        create_subnet_group(&router, subnet_group).await,
    );

    let published = available_versions(&router, GLOBAL_ENGINE).await;
    assert!(
        published.len() > 1,
        "expected the report to publish several available {GLOBAL_ENGINE} versions, got \
         {published:?}"
    );

    for (index, (version, family)) in published.iter().enumerate() {
        let case = format!("{GLOBAL_ENGINE} {version}");
        let global = format!("emu-pcc-global-{index}");
        let identifier = format!("emu-pcc-member-{index}");
        // The global database the member joins runs the same published build, which is
        // what `CreateGlobalCluster` resolves for it.
        expect_ok(
            &case,
            call(
                &router,
                "CreateGlobalCluster",
                &[
                    ("GlobalClusterIdentifier", global.as_str()),
                    ("Engine", GLOBAL_ENGINE),
                    ("EngineVersion", version.as_str()),
                ],
            )
            .await,
        );
        let created = expect_ok(
            &case,
            call(
                &router,
                "CreateDBCluster",
                &[
                    ("DBClusterIdentifier", identifier.as_str()),
                    ("Engine", GLOBAL_ENGINE),
                    ("EngineVersion", version.as_str()),
                    ("DBSubnetGroupName", subnet_group),
                    ("GlobalClusterIdentifier", global.as_str()),
                ],
            )
            .await,
        );
        for (member, expected) in [
            ("DBClusterIdentifier", identifier.as_str()),
            ("Engine", GLOBAL_ENGINE),
            ("EngineVersion", version.as_str()),
            ("GlobalClusterIdentifier", global.as_str()),
            ("DBSubnetGroup", subnet_group),
            ("Status", CREATING),
            ("Port", AURORA_MYSQL_PORT),
            // The default cluster parameter group is named after the FAMILY the resolved
            // version belongs to, so the two reports cannot disagree about the version.
            ("DBClusterParameterGroup", &format!("default.{family}")),
        ] {
            let actual = text(&created, member);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {member} {expected:?}, got {actual:?} in {created}"
            );
        }

        // The cluster is STORED as it was answered, and the global database it joined
        // reports it as one of its members.
        let described = expect_ok(
            &case,
            call(
                &router,
                "DescribeDBClusters",
                &[("DBClusterIdentifier", identifier.as_str())],
            )
            .await,
        );
        for (member, expected) in [
            ("EngineVersion", version.as_str()),
            ("GlobalClusterIdentifier", global.as_str()),
            ("DBSubnetGroup", subnet_group),
        ] {
            let actual = text(&described, member);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the stored cluster's {member} {expected:?}, got \
                 {actual:?} in {described}"
            );
        }
        let global_body = expect_ok(
            &case,
            call(
                &router,
                "DescribeGlobalClusters",
                &[("GlobalClusterIdentifier", global.as_str())],
            )
            .await,
        );
        let members = texts(&global_body, "DBClusterArn");
        assert!(
            members.contains(&cluster_arn(&identifier)),
            "case {case:?}: expected the global database to report the member {:?}, got \
             {members:?}",
            cluster_arn(&identifier)
        );

        // Each row tears its own topology down — the member first, then the global
        // database it left — so the table is not bounded by the account's global-cluster
        // quota and every published version really is exercised.
        wait_for_settle();
        expect_ok(
            &case,
            call(
                &router,
                "DeleteDBCluster",
                &[
                    ("DBClusterIdentifier", identifier.as_str()),
                    ("SkipFinalSnapshot", "true"),
                ],
            )
            .await,
        );
        expect_ok(
            &case,
            call(
                &router,
                "DeleteGlobalCluster",
                &[("GlobalClusterIdentifier", global.as_str())],
            )
            .await,
        );
    }

    // A well-formed build of a published family that the report publishes NO entry for is
    // a version this service cannot provision: it is refused for the combination, in the
    // sentence the real service answers that with, and nothing is stored for it.
    let unpublished = "8.0.mysql_aurora.3.99.99";
    assert!(
        !published
            .iter()
            .any(|(version, _)| version == unpublished),
        "the negative row must name a build the report does not publish, got {published:?}"
    );
    let case = format!("{GLOBAL_ENGINE} {unpublished}");
    let refused = expect_error(
        &case,
        call(
            &router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "emu-pcc-unpublished"),
                ("Engine", GLOBAL_ENGINE),
                ("EngineVersion", unpublished),
            ],
        )
        .await,
        400,
        "InvalidParameterCombination",
    );
    let expected = format!("Cannot find version {unpublished} for {GLOBAL_ENGINE}");
    let message = text(&refused, "Message");
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
    expect_error(
        &case,
        call(
            &router,
            "DescribeDBClusters",
            &[("DBClusterIdentifier", "emu-pcc-unpublished")],
        )
        .await,
        404,
        "DBClusterNotFoundFault",
    );
}

/// One row of the DB instance combination table.
struct CombinationCase {
    case: &'static str,
    engine: &'static str,
    /// A version the engine-version catalogue publishes for `engine`, named explicitly so
    /// the refusal's wording is deterministic.
    engine_version: &'static str,
    db_instance_class: &'static str,
    /// The `LicenseModel` the request carries, or `None` to leave it to the service.
    license_model: Option<&'static str>,
    /// The license model the refusal names: the one the request carried, else the one the
    /// service would create the instance under.
    reported_license: &'static str,
    offered: bool,
}

/// The class the combination rule generalises over: the instance generation (a current
/// burstable, Graviton, standard and memory-optimised family, a previous-generation
/// family, and the serverless capacity class), the size inside a published family, the
/// engine (an RDS engine, an Aurora cluster engine and a sibling control plane's), and
/// whether the request names a license model its engine is offered under.
const COMBINATIONS: &[CombinationCase] = &[
    CombinationCase {
        case: "a current burstable Graviton class",
        engine: "mysql",
        engine_version: "8.0.40",
        db_instance_class: "db.t4g.micro",
        license_model: None,
        reported_license: "general-public-license",
        offered: true,
    },
    CombinationCase {
        case: "a current burstable Intel class",
        engine: "mysql",
        engine_version: "8.0.40",
        db_instance_class: "db.t3.small",
        license_model: None,
        reported_license: "general-public-license",
        offered: true,
    },
    CombinationCase {
        case: "a size inside a published family that the report does not list",
        engine: "postgres",
        engine_version: "17.2",
        db_instance_class: "db.m5.24xlarge",
        license_model: None,
        reported_license: "postgresql-license",
        offered: true,
    },
    CombinationCase {
        case: "a memory-optimised Graviton class on an Aurora cluster engine",
        engine: "aurora-mysql",
        engine_version: "8.0.mysql_aurora.3.10.3",
        db_instance_class: "db.r6g.large",
        license_model: None,
        reported_license: "general-public-license",
        offered: true,
    },
    CombinationCase {
        case: "the serverless capacity class",
        engine: "aurora-postgresql",
        engine_version: "16.6",
        db_instance_class: "db.serverless",
        license_model: None,
        reported_license: "postgresql-license",
        offered: true,
    },
    CombinationCase {
        case: "the license model the engine publishes, named explicitly",
        engine: "mysql",
        engine_version: "8.0.40",
        db_instance_class: "db.m5.large",
        license_model: Some("general-public-license"),
        reported_license: "general-public-license",
        offered: true,
    },
    CombinationCase {
        case: "a previous-generation burstable family",
        engine: "mysql",
        engine_version: "8.0.40",
        db_instance_class: "db.t2.micro",
        license_model: None,
        reported_license: "general-public-license",
        offered: false,
    },
    CombinationCase {
        case: "a previous-generation standard family, at a size the published families have",
        engine: "postgres",
        engine_version: "17.2",
        db_instance_class: "db.m4.large",
        license_model: None,
        reported_license: "postgresql-license",
        offered: false,
    },
    CombinationCase {
        case: "a family no engine is offered in, on a cluster engine",
        engine: "docdb",
        engine_version: "5.0.0",
        db_instance_class: "db.m1.small",
        license_model: None,
        reported_license: "na",
        offered: false,
    },
    CombinationCase {
        case: "a license model another engine is offered under",
        engine: "mysql",
        engine_version: "8.0.40",
        db_instance_class: "db.m5.large",
        license_model: Some("postgresql-license"),
        reported_license: "postgresql-license",
        offered: false,
    },
];

/// The sentence the real service refuses an unorderable combination with: fixed text, with
/// the four members of the combination as its only variable parts (the recorded
/// `CreateDBInstance` refusal).
fn combination_message(row: &CombinationCase) -> String {
    format!(
        "RDS does not support creating a DB instance with the following combination: \
         DBInstanceClass={}, Engine={}, EngineVersion={}, LicenseModel={}. For supported \
         combinations of instance class and database engine version, see the documentation.",
        row.db_instance_class, row.engine, row.engine_version, row.reported_license
    )
}

/// A DB instance is created only in a combination the orderable-options report offers, and
/// a create it refuses leaves the identifier free for one it offers.
#[tokio::test]
async fn a_db_instance_is_created_only_in_an_offered_combination() {
    let router = emulator();

    for (index, row) in COMBINATIONS.iter().enumerate() {
        let identifier = format!("emu-pcc-inst-{index}");
        let mut params: Vec<(&str, String)> = vec![
            ("DBInstanceIdentifier", identifier.clone()),
            ("Engine", row.engine.to_string()),
            ("EngineVersion", row.engine_version.to_string()),
            ("DBInstanceClass", row.db_instance_class.to_string()),
        ];
        if let Some(license_model) = row.license_model {
            params.push(("LicenseModel", license_model.to_string()));
        }
        let response = call(&router, "CreateDBInstance", &params).await;
        if row.offered {
            let created = expect_ok(row.case, response);
            for (member, expected) in [
                ("DBInstanceClass", row.db_instance_class),
                ("Engine", row.engine),
                ("EngineVersion", row.engine_version),
                ("DBInstanceStatus", "creating"),
            ] {
                let actual = text(&created, member);
                assert_eq!(
                    actual, expected,
                    "case {:?}: expected {member} {expected:?}, got {actual:?} in {created}",
                    row.case
                );
            }
            // The instance is STORED in the class it was created in.
            let described = expect_ok(
                row.case,
                call(
                    &router,
                    "DescribeDBInstances",
                    &[("DBInstanceIdentifier", identifier.as_str())],
                )
                .await,
            );
            let actual = text(&described, "DBInstanceClass");
            assert_eq!(
                actual, row.db_instance_class,
                "case {:?}: expected the stored instance's DBInstanceClass {:?}, got \
                 {actual:?} in {described}",
                row.case, row.db_instance_class
            );
            continue;
        }

        let refused = expect_error(row.case, response, 400, "InvalidParameterCombination");
        let expected = combination_message(row);
        let message = text(&refused, "Message");
        assert_eq!(
            message, expected,
            "case {:?}: expected {expected:?}, got {message:?}",
            row.case
        );
        // A create the report offers no option for stores NOTHING: the instance is absent,
        // and the identifier it asked for is still free for a combination that is offered
        // — which is what a later create of the same instance depends on.
        expect_error(
            row.case,
            call(
                &router,
                "DescribeDBInstances",
                &[("DBInstanceIdentifier", identifier.as_str())],
            )
            .await,
            404,
            "DBInstanceNotFound",
        );
        let retried = expect_ok(
            row.case,
            call(
                &router,
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", identifier.as_str()),
                    ("Engine", "mysql"),
                    ("EngineVersion", "8.0.40"),
                    ("DBInstanceClass", "db.t4g.micro"),
                ],
            )
            .await,
        );
        let actual = text(&retried, "DBInstanceIdentifier");
        assert_eq!(
            actual, identifier,
            "case {:?}: expected the refused identifier to be free, got {actual:?} in \
             {retried}",
            row.case
        );
    }
}

/// The DB security group names the identifier rule would refuse and this member does not:
/// the shapes that rule's clauses name, each still a name the group round-trips under.
const UNCONSTRAINED_NAMES: [(&str, &str); 5] = [
    ("two leading hyphens and a trailing one", "emu--pcc-sg-a-"),
    ("a leading hyphen", "-emu-pcc-sg-b"),
    ("a leading digit", "9emu-pcc-sg-c"),
    ("a character outside the identifier alphabet", "emu.pcc.sg.d"),
    ("an underscore", "emu_pcc_sg_e"),
];

/// A DB security group named outside the identifier alphabet is created, reported and
/// DELETED like any other: the member is a plain string in the model, so no name shape is
/// a refusal, and the delete of such a group resolves the group the create stored.
#[tokio::test]
async fn a_db_security_group_name_the_model_does_not_constrain_round_trips_and_deletes() {
    let router = emulator();

    for (case, name) in UNCONSTRAINED_NAMES {
        let created = expect_ok(
            case,
            call(
                &router,
                "CreateDBSecurityGroup",
                &[
                    ("DBSecurityGroupName", name),
                    ("DBSecurityGroupDescription", "published catalogue creates"),
                ],
            )
            .await,
        );
        let reported = text(&created, "DBSecurityGroupName");
        assert_eq!(
            reported, name,
            "case {case:?}: expected the create to echo {name:?}, got {reported:?} in \
             {created}"
        );
        let arn = text(&created, "DBSecurityGroupArn");
        let expected_arn = format!("arn:aws:rds:{REGION}:{ACCOUNT}:secgrp:{name}");
        assert_eq!(
            arn, expected_arn,
            "case {case:?}: expected the ARN {expected_arn:?}, got {arn:?}"
        );
        // Every DB security group of this account belongs to a VPC, and it is the one its
        // region resolves into — the same VPC a DB subnet group of that region reports.
        let vpc_id = text(&created, "VpcId");
        let expected_vpc = region_vpc_id(REGION);
        assert_eq!(
            vpc_id, expected_vpc,
            "case {case:?}: expected the group's VpcId {expected_vpc:?}, got {vpc_id:?} in \
             {created}"
        );

        let described = expect_ok(
            case,
            call(
                &router,
                "DescribeDBSecurityGroups",
                &[("DBSecurityGroupName", name)],
            )
            .await,
        );
        let reported = text(&described, "DBSecurityGroupName");
        assert_eq!(
            reported, name,
            "case {case:?}: expected the describe to report {name:?}, got {reported:?} in \
             {described}"
        );

        // The teardown of such a group: the delete resolves it and the group is gone.
        expect_ok(
            case,
            call(
                &router,
                "DeleteDBSecurityGroup",
                &[("DBSecurityGroupName", name)],
            )
            .await,
        );
        expect_error(
            case,
            call(
                &router,
                "DescribeDBSecurityGroups",
                &[("DBSecurityGroupName", name)],
            )
            .await,
            404,
            "DBSecurityGroupNotFound",
        );
    }
}
