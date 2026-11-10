// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a CREATE resolves when the request names nothing, and for the
//! two refusals that go with the master credential.
//!
//! The rules under test, each asserted over a table that spans what it generalises over
//! rather than one recorded request:
//!
//! * `EngineVersion` is optional, and an instance created without one runs the engine's
//!   CURRENT default version — the one `DescribeDBEngineVersions(DefaultOnly)` publishes for
//!   that engine — and DERIVES its default DB parameter group (`default.<family>`) and
//!   default option group (`default:<engine>-<major version>`) from that assigned version,
//!   so none of the three can be stale while another is current. The rows span the engines
//!   whose family names take different shapes (a major version alone, a two-part version,
//!   the Aurora families).
//! * `DBSubnetGroupName` is optional, and an instance created without one is placed in the
//!   account's `default` DB subnet group — the group Amazon RDS provides with the account's
//!   default VPC — which reports that VPC and the default subnets the service that OWNS
//!   subnets holds for it. An instance that joins a DB cluster takes the cluster's group,
//!   and one that names a group takes the named one.
//! * `PendingModifiedValues` is "returned only when there are pending changes": a resource
//!   still being provisioned with a master credential reports that credential as pending,
//!   masked because the member is `@sensitive`, and a resource with nothing pending reports
//!   no such element at all.
//! * `MasterUserPassword` and `ManageMasterUserPassword` are mutually exclusive, and a
//!   request carrying both is refused — HTTP 400 `InvalidParameterValue` with one fixed
//!   sentence — by EVERY front door that publishes both members, before anything is
//!   provisioned and whether or not the resource it names exists.
//!
//! Every call goes over the wire through the harness `db_instances.rs` keeps, so each
//! assertion reads the XML the emulator itself answered with. The default-VPC case mounts
//! Amazon EC2 beside Amazon RDS on ONE engine (`db_subnet_group_ec2_zones::Account`), so
//! every subnet id and zone an assertion names is one the owner of subnets reported.

use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, optional_text, subtrees, text, texts,
    wait_for_settle, REGION,
};
use crate::db_subnet_group_ec2_zones::Account;
use service_com_amazonaws_rds_amazonrdsv19::state::OPTION_GROUP_ENGINE_NAMES;
use smithy_server_runtime::MultiServiceRouter;

/// A compute class the engines below are all orderable in.
const CLASS: &str = "db.t3.medium";

/// The name Amazon RDS keeps for the DB subnet group it provides with the account's default
/// VPC, and the description it gives it.
const DEFAULT_GROUP: &str = "default";

/// What a master user password is reported as wherever a pending one is reported: the
/// member is `@sensitive`, so the credential itself is never published. Written out here
/// rather than read from the implementation, because the masking is part of what this suite
/// pins.
const MASKED: &str = "****";

/// The one `<DBInstance>` subtree of a response.
fn the_instance(case: &str, xml: &str) -> String {
    let found = subtrees(xml, "DBInstance");
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one DBInstance, got {found:?} in {xml}"
    );
    found.into_iter().next().expect("checked just above")
}

/// The one `<DBCluster>` subtree of a response.
fn the_cluster(case: &str, xml: &str) -> String {
    let found = subtrees(xml, "DBCluster");
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one DBCluster, got {found:?} in {xml}"
    );
    found.into_iter().next().expect("checked just above")
}

/// `CreateDBInstance` with its three required members plus `extra`.
async fn create_instance(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
    engine: &str,
    extra: &[(&str, String)],
) -> String {
    let mut params: Vec<(&str, String)> = vec![
        ("DBInstanceIdentifier", identifier.to_string()),
        ("Engine", engine.to_string()),
        ("DBInstanceClass", CLASS.to_string()),
    ];
    params.extend(extra.iter().cloned());
    expect_ok(case, call(router, "CreateDBInstance", &params).await)
}

/// `DescribeDBInstances` for one identifier, as the `<DBInstance>` it reports.
async fn describe_instance(router: &MultiServiceRouter, case: &str, identifier: &str) -> String {
    let body = expect_ok(
        case,
        call(
            router,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", identifier.to_string())],
        )
        .await,
    );
    the_instance(case, &body)
}

// ---------------------------------------------------------------------------
// The engine version a create with no `EngineVersion` resolves, and the groups
// derived from it
// ---------------------------------------------------------------------------

/// The engines this table spans: one whose major version is a single number, one whose
/// family carries a two-part version, one MariaDB family, and both Aurora families — the
/// shapes a derived group name takes.
const DERIVING_ENGINES: &[&str] = &[
    "postgres",
    "mysql",
    "mariadb",
    "aurora-postgresql",
    "aurora-mysql",
];

/// What `DescribeDBEngineVersions(DefaultOnly)` publishes as `engine`'s default version:
/// the version itself, its DB parameter group family and its major engine version.
///
/// Read from the catalogue the service publishes rather than written out here, so the
/// assertion is that the create and the catalogue AGREE — which is the rule — instead of
/// pinning whichever version the catalogue carries today.
async fn published_default(router: &MultiServiceRouter, engine: &str) -> (String, String, String) {
    let body = expect_ok(
        engine,
        call(
            router,
            "DescribeDBEngineVersions",
            &[
                ("Engine", engine.to_string()),
                ("DefaultOnly", "true".to_string()),
            ],
        )
        .await,
    );
    let found = subtrees(&body, "DBEngineVersion");
    assert_eq!(
        found.len(),
        1,
        "case {engine:?}: DefaultOnly must report exactly one version, got {found:?} in {body}"
    );
    let entry = found.into_iter().next().expect("checked just above");
    (
        text(&entry, "EngineVersion"),
        text(&entry, "DBParameterGroupFamily"),
        text(&entry, "MajorEngineVersion"),
    )
}

#[tokio::test]
async fn a_create_with_no_engine_version_takes_the_engine_default_and_derives_its_groups() {
    let router = emulator();

    for (index, engine) in DERIVING_ENGINES.iter().enumerate() {
        let case = format!("{engine}, no EngineVersion named");
        let (version, family, major) = published_default(&router, engine).await;
        let identifier = format!("emu-derived-{index}");
        let created = create_instance(&router, &case, &identifier, engine, &[]).await;
        let instance = the_instance(&case, &created);

        let reported_version = optional_text(&instance, "EngineVersion");
        assert_eq!(
            reported_version.as_deref(),
            Some(version.as_str()),
            "case {case:?}: expected the engine's published default version {version:?}, got \
             {reported_version:?}"
        );
        // `default.<family>` and `default:<engine>-<major version>`, the forms Amazon RDS
        // names the groups it provides itself with; the major version's dots become hyphens.
        let expected_parameter_group = format!("default.{family}");
        let reported_parameter_group = optional_text(&instance, "DBParameterGroupName");
        assert_eq!(
            reported_parameter_group.as_deref(),
            Some(expected_parameter_group.as_str()),
            "case {case:?}: expected the parameter group derived from the assigned version, \
             {expected_parameter_group:?}, got {reported_parameter_group:?}"
        );
        let expected_option_group = format!("default:{engine}-{}", major.replace('.', "-"));
        let reported_option_group = optional_text(&instance, "OptionGroupName");
        assert_eq!(
            reported_option_group.as_deref(),
            Some(expected_option_group.as_str()),
            "case {case:?}: expected the option group derived from the assigned version, \
             {expected_option_group:?}, got {reported_option_group:?}"
        );
        // The derived names address groups that really resolve: a client applying one by
        // name must not address a group this service answers nothing for. An option group
        // is resolved only for the engines one can be associated with at all — the Aurora
        // engines take none — so only those rows read the group back.
        let groups = expect_ok(
            &case,
            call(
                &router,
                "DescribeDBParameterGroups",
                &[("DBParameterGroupName", expected_parameter_group.clone())],
            )
            .await,
        );
        assert!(
            texts(&groups, "DBParameterGroupFamily").contains(&family),
            "case {case:?}: the derived group {expected_parameter_group:?} must be the one of \
             family {family:?}, got {groups}"
        );
        if OPTION_GROUP_ENGINE_NAMES.contains(engine) {
            let options = expect_ok(
                &case,
                call(
                    &router,
                    "DescribeOptionGroups",
                    &[("OptionGroupName", expected_option_group.clone())],
                )
                .await,
            );
            assert!(
                texts(&options, "MajorEngineVersion").contains(&major),
                "case {case:?}: the derived group {expected_option_group:?} must be the one \
                 of major engine version {major:?}, got {options}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Where a create with no `DBSubnetGroupName` places the instance
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_instance_created_without_a_subnet_group_joins_the_accounts_default_group() {
    let account = Account::new();

    // What the owner of subnets holds for the account's default VPC: its default subnet in
    // each Availability Zone. Asked of EC2, never assumed, so every id and zone below is
    // one EC2 reported.
    let default_subnets = account
        .ec2
        .describe_subnets()
        .filters(
            ::aws_sdk_ec2::types::Filter::builder()
                .name("default-for-az")
                .values("true")
                .build(),
        )
        .send()
        .await
        .expect("EC2 must report the default VPC's default subnets");
    let mut expected: Vec<(String, String, String)> = default_subnets
        .subnets()
        .iter()
        .filter_map(|subnet| {
            Some((
                subnet.availability_zone()?.to_string(),
                subnet.subnet_id()?.to_string(),
                subnet.vpc_id()?.to_string(),
            ))
        })
        .collect();
    expected.sort();
    assert!(
        expected.len() >= 2,
        "the account's default VPC must have a default subnet per zone, got {expected:?}"
    );
    let default_vpc = expected[0].2.clone();

    let case = "an instance created with no DBSubnetGroupName";
    let created = expect_ok(
        case,
        call_in(
            &account.router,
            REGION,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier".to_string(), "emu-default-placed".to_string()),
                ("Engine".to_string(), "postgres".to_string()),
                ("DBInstanceClass".to_string(), CLASS.to_string()),
            ],
        )
        .await,
    );
    let instance = the_instance(case, &created);
    let group = subtrees(&instance, "DBSubnetGroup")
        .into_iter()
        .next()
        .unwrap_or_else(|| {
            panic!("case {case:?}: every instance is placed in a DB subnet group: {instance}")
        });
    for (member, value) in [
        ("DBSubnetGroupName", DEFAULT_GROUP),
        ("DBSubnetGroupDescription", DEFAULT_GROUP),
        ("SubnetGroupStatus", "Complete"),
        ("VpcId", default_vpc.as_str()),
    ] {
        let reported = optional_text(&group, member);
        assert_eq!(
            reported.as_deref(),
            Some(value),
            "case {case:?}: expected the default group's {member} to be {value:?}, got \
             {reported:?}"
        );
    }
    // The subnets the group covers are the default VPC's own, each with the zone EC2
    // reported it in.
    let reported_subnets: Vec<(String, String)> = subtrees(&group, "Subnet")
        .into_iter()
        .map(|subnet| {
            (
                text(&subnet, "SubnetIdentifier"),
                // `SubnetAvailabilityZone` is an `AvailabilityZone`, whose `Name` is the
                // zone EC2 reported.
                text(&subnet, "Name"),
            )
        })
        .collect();
    let expected_subnets: Vec<(String, String)> = expected
        .iter()
        .map(|(zone, subnet_id, _)| (subnet_id.clone(), zone.clone()))
        .collect();
    assert_eq!(
        reported_subnets, expected_subnets,
        "case {case:?}: expected the default VPC's subnets as EC2 reports them, got \
         {reported_subnets:?}"
    );
    // The group a read of the GROUP reports is the same one, so the instance's copy cannot
    // drift from `DescribeDBSubnetGroups`.
    let described = expect_ok(
        case,
        account.describe_group(DEFAULT_GROUP).await,
    );
    let described_group = subtrees(&described, "DBSubnetGroup")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: the default group must be reported: {described}"));
    assert_eq!(
        texts(&described_group, "SubnetIdentifier"),
        texts(&group, "SubnetIdentifier"),
        "case {case:?}: the group the instance reports and the group DescribeDBSubnetGroups \
         reports must be one answer, got {described_group} against {group}"
    );
}

/// One row of the placement table: the create, and the DB subnet group the instance it
/// creates must be placed in.
struct PlacementCase {
    case: &'static str,
    identifier: &'static str,
    engine: &'static str,
    /// The group the instance must report being placed in.
    expected_group: &'static str,
    /// The members this row adds beyond the three a create always carries.
    extra: &'static [(&'static str, &'static str)],
}

/// Where the group an instance is placed in comes from: the request, the DB cluster the
/// instance joins, and the account's default group when neither names one. The cluster row's
/// cluster itself names no group, so it is placed in the default group and its member
/// inherits THAT.
const PLACEMENT_CASES: &[PlacementCase] = &[
    PlacementCase {
        case: "the group the request names",
        identifier: "emu-placed-named",
        engine: "postgres",
        expected_group: "emu-placed-group",
        extra: &[("DBSubnetGroupName", "emu-placed-group")],
    },
    PlacementCase {
        case: "the group of the cluster the instance joins",
        identifier: "emu-placed-member",
        engine: "aurora-postgresql",
        expected_group: DEFAULT_GROUP,
        extra: &[("DBClusterIdentifier", "emu-placed-cluster")],
    },
    PlacementCase {
        case: "the account's default group when neither names one",
        identifier: "emu-placed-standalone",
        engine: "postgres",
        expected_group: DEFAULT_GROUP,
        extra: &[],
    },
];

#[tokio::test]
async fn a_named_group_and_a_clusters_group_are_what_an_instance_is_placed_in() {
    let router = emulator();

    expect_ok(
        "a group to name",
        call(
            &router,
            "CreateDBSubnetGroup",
            &[
                ("DBSubnetGroupName", "emu-placed-group".to_string()),
                ("DBSubnetGroupDescription", "a named group".to_string()),
                ("SubnetIds.SubnetIdentifier.1", "subnet-00000011".to_string()),
                ("SubnetIds.SubnetIdentifier.2", "subnet-00000012".to_string()),
            ],
        )
        .await,
    );
    expect_ok(
        "a cluster to join",
        call(
            &router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "emu-placed-cluster".to_string()),
                ("Engine", "aurora-postgresql".to_string()),
            ],
        )
        .await,
    );
    wait_for_settle();

    for row in PLACEMENT_CASES {
        let (case, identifier, expected_group) = (row.case, row.identifier, row.expected_group);
        let extra: Vec<(&str, String)> = row
            .extra
            .iter()
            .map(|(member, value)| (*member, (*value).to_string()))
            .collect();
        let created = create_instance(&router, case, identifier, row.engine, &extra).await;
        let instance = the_instance(case, &created);
        let reported = optional_text(&instance, "DBSubnetGroupName");
        assert_eq!(
            reported.as_deref(),
            Some(expected_group),
            "case {case:?}: expected the instance to be placed in {expected_group:?}, got \
             {reported:?}"
        );
        // And the placement is stored, not just acknowledged.
        let read_back = describe_instance(&router, case, identifier).await;
        let stored = optional_text(&read_back, "DBSubnetGroupName");
        assert_eq!(
            stored.as_deref(),
            Some(expected_group),
            "case {case:?}: expected the stored instance to report {expected_group:?}, got \
             {stored:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The credential a create is still applying
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_master_credential_still_being_applied_is_the_only_pending_value() {
    let router = emulator();

    // The rows span both resource kinds and both ways a create names its credential: one
    // the request supplied (pending until the resource is up) and none at all (nothing
    // pending at any point).
    let rows: &[(&str, &str, &str, bool)] = &[
        ("an instance created with a password", "DBInstance", "emu-pending-instance", true),
        ("an instance created without one", "DBInstance", "emu-nopending-instance", false),
        ("a cluster created with a password", "DBCluster", "emu-pending-cluster", true),
        ("a cluster created without one", "DBCluster", "emu-nopending-cluster", false),
    ];
    for (case, element, identifier, with_password) in rows {
        let (action, identifier_member, engine) = if *element == "DBCluster" {
            ("CreateDBCluster", "DBClusterIdentifier", "aurora-postgresql")
        } else {
            ("CreateDBInstance", "DBInstanceIdentifier", "postgres")
        };
        let mut params: Vec<(&str, String)> = vec![
            (identifier_member, (*identifier).to_string()),
            ("Engine", engine.to_string()),
        ];
        if *element == "DBInstance" {
            params.push(("DBInstanceClass", CLASS.to_string()));
        }
        if *with_password {
            params.push(("MasterUsername", "pgadmin".to_string()));
            params.push(("MasterUserPassword", "a-strong-password".to_string()));
        }
        let created = expect_ok(case, call(&router, action, &params).await);
        let envelope = if *element == "DBCluster" {
            the_cluster(case, &created)
        } else {
            the_instance(case, &created)
        };
        let pending = subtrees(&envelope, "PendingModifiedValues");
        if *with_password {
            let pending = pending.into_iter().next().unwrap_or_else(|| {
                panic!(
                    "case {case:?}: a credential the create is still applying must be reported \
                     as pending: {envelope}"
                )
            });
            let reported = optional_text(&pending, "MasterUserPassword");
            assert_eq!(
                reported.as_deref(),
                Some(MASKED),
                "case {case:?}: a pending master credential is reported masked, got \
                 {reported:?}"
            );
        } else {
            assert!(
                pending.is_empty(),
                "case {case:?}: nothing is pending for a create that named no credential, got \
                 {pending:?}"
            );
        }
    }

    // Once the clock has brought each resource up, the create has applied what it carried
    // and nothing is pending any more.
    wait_for_settle();
    for (case, element, identifier, _) in rows {
        let (action, identifier_member) = if *element == "DBCluster" {
            ("DescribeDBClusters", "DBClusterIdentifier")
        } else {
            ("DescribeDBInstances", "DBInstanceIdentifier")
        };
        let body = expect_ok(
            case,
            call(
                &router,
                action,
                &[(identifier_member, (*identifier).to_string())],
            )
            .await,
        );
        let envelope = if *element == "DBCluster" {
            the_cluster(case, &body)
        } else {
            the_instance(case, &body)
        };
        assert!(
            subtrees(&envelope, "PendingModifiedValues").is_empty(),
            "case {case:?}: a resource that is up has nothing pending, got {envelope}"
        );
    }
}

// ---------------------------------------------------------------------------
// The two password sources are mutually exclusive
// ---------------------------------------------------------------------------

/// The sentence a request carrying BOTH password sources is refused with, written out here
/// rather than read from the implementation: the wording is part of what this suite pins.
const MUTUALLY_EXCLUSIVE: &str = "MasterUserPassword and ManageMasterUserPassword are \
     mutually exclusive. Specify only one of these parameters.";

/// One row of the mutual-exclusion table: the call, and the parameters it carries.
struct ExclusiveCase {
    case: &'static str,
    action: &'static str,
    params: &'static [(&'static str, &'static str)],
}

#[tokio::test]
async fn naming_both_password_sources_is_refused_wherever_both_members_are_published() {
    let router = emulator();

    // A multi-tenant container DB instance, so the tenant-database row addresses a real
    // instance: the refusal must not depend on the resource being absent.
    expect_ok(
        "a container DB instance",
        call(
            &router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-exclusive-cdb".to_string()),
                ("Engine", "oracle-ee-cdb".to_string()),
                ("DBInstanceClass", "db.m5.large".to_string()),
                ("MasterUsername", "oracleadmin".to_string()),
                ("MasterUserPassword", "the-callers-own-password".to_string()),
                ("MultiTenant", "true".to_string()),
            ],
        )
        .await,
    );
    wait_for_settle();

    // The rows span every front door that publishes both members: the two creates, the two
    // modifications — which answer the refusal whether or not the resource exists, because
    // it is a statement about the REQUEST — and the tenant-database create.
    let rows: &[ExclusiveCase] = &[
        ExclusiveCase {
            case: "CreateDBInstance",
            action: "CreateDBInstance",
            params: &[
                ("DBInstanceIdentifier", "emu-exclusive-instance"),
                ("Engine", "postgres"),
                ("DBInstanceClass", CLASS),
                ("MasterUsername", "pgadmin"),
                ("MasterUserPassword", "a-strong-password"),
                ("ManageMasterUserPassword", "true"),
            ],
        },
        ExclusiveCase {
            case: "CreateDBCluster",
            action: "CreateDBCluster",
            params: &[
                ("DBClusterIdentifier", "emu-exclusive-cluster"),
                ("Engine", "aurora-postgresql"),
                ("MasterUsername", "pgadmin"),
                ("MasterUserPassword", "a-strong-password"),
                ("ManageMasterUserPassword", "true"),
            ],
        },
        ExclusiveCase {
            case: "ModifyDBInstance over an instance that does not exist",
            action: "ModifyDBInstance",
            params: &[
                ("DBInstanceIdentifier", "emu-exclusive-absent"),
                ("MasterUserPassword", "a-strong-password"),
                ("ManageMasterUserPassword", "true"),
            ],
        },
        ExclusiveCase {
            case: "ModifyDBCluster over a cluster that does not exist",
            action: "ModifyDBCluster",
            params: &[
                ("DBClusterIdentifier", "emu-exclusive-absent"),
                ("MasterUserPassword", "a-strong-password"),
                ("ManageMasterUserPassword", "true"),
            ],
        },
        ExclusiveCase {
            case: "CreateTenantDatabase",
            action: "CreateTenantDatabase",
            params: &[
                ("DBInstanceIdentifier", "emu-exclusive-cdb"),
                ("TenantDBName", "emutenant"),
                ("MasterUsername", "tenantadmin"),
                ("MasterUserPassword", "a-strong-password"),
                ("ManageMasterUserPassword", "true"),
            ],
        },
    ];
    for row in rows {
        let (case, action) = (row.case, row.action);
        let params: Vec<(&str, String)> = row
            .params
            .iter()
            .map(|(member, value)| (*member, (*value).to_string()))
            .collect();
        let body = expect_error(
            case,
            call(&router, action, &params).await,
            400,
            "InvalidParameterValue",
        );
        let message = text(&body, "Message");
        assert_eq!(
            message, MUTUALLY_EXCLUSIVE,
            "case {case:?}: expected the fixed sentence {MUTUALLY_EXCLUSIVE:?}, got {message:?}"
        );
        // The awsQuery envelope types a refusal of what the request carries `Sender`: a
        // client that reads `Receiver` would retry a request only it can fix.
        let fault_type = text(&body, "Type");
        assert_eq!(
            fault_type, "Sender",
            "case {case:?}: expected a Sender fault, got {fault_type:?} in {body}"
        );
    }

    // Nothing the refused creates named was provisioned, and no secret was minted for it.
    for (case, action, identifier_member, identifier) in [
        (
            "the refused instance create",
            "DescribeDBInstances",
            "DBInstanceIdentifier",
            "emu-exclusive-instance",
        ),
        (
            "the refused cluster create",
            "DescribeDBClusters",
            "DBClusterIdentifier",
            "emu-exclusive-cluster",
        ),
    ] {
        let (status, body) = call(
            &router,
            action,
            &[(identifier_member, identifier.to_string())],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused create must leave nothing behind, got {status} with \
             {body}"
        );
    }
}
