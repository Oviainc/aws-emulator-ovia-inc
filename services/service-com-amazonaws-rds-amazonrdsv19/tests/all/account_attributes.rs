// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeAccountAttributes`.
//!
//! The rule under test: the report names every quota of the published account-quota
//! catalog, and each entry's `Used` is counted out of the records the region the request
//! was signed for actually holds while its `Max` is the limit this service enforces for
//! that resource — so the usage follows every create and delete, a region never reports
//! another's records, and no entry can claim more usage than its own maximum allows.
//!
//! The table below spans the classes the rule generalises over: the resource kind each
//! quota counts (instances, clusters, snapshots, the four group kinds, subscriptions),
//! how the usage is derived (a count of records, a sum over them, a maximum over them,
//! and a kind no operation creates yet), and the counts themselves (none, one, several).
//! The quota names, the maxima of the resources this service does not store and the
//! usage each name is defined by come from the `AccountQuota` shape's own documentation
//! (`smithy_query(query="doc",shape="AccountQuota")`) and Amazon RDS's published
//! quotas; the maxima of the resources it does store are the constants its own create
//! paths enforce, imported here rather than copied.
//!
//! Every call goes over the wire through the harness in `db_instances.rs`, so the
//! assertions read the XML the emulator itself answered with.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    DB_CLUSTER_PARAMETER_GROUP_QUOTA_PER_REGION, DB_CLUSTER_QUOTA_PER_REGION,
    DB_CLUSTER_SNAPSHOT_QUOTA_PER_REGION, DB_INSTANCE_QUOTA_PER_REGION,
    DB_PARAMETER_GROUP_QUOTA_PER_REGION, DB_SNAPSHOT_QUOTA_PER_REGION,
    DB_SUBNET_GROUP_QUOTA_PER_REGION, EVENT_SUBSCRIPTION_QUOTA_PER_REGION,
    MAX_SUBNETS_PER_DB_SUBNET_GROUP, OPTION_GROUP_QUOTA_PER_REGION,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, emulator, expect_ok, subtrees, text, wait_for_settle, REGION,
};

/// A call that carries no parameters beyond the `Action` and `Version` the harness
/// adds: `DescribeAccountAttributes` "doesn't take any parameters".
const NO_PARAMS: [(&str, &str); 0] = [];

/// A second region, so records out of the signed region's scope are a row of the table.
const OTHER_REGION: &str = "eu-central-1";

/// The SNS topic an event subscription publishes to.
const SNS_TOPIC: &str = "arn:aws:sns:us-west-2:000000000000:rds-events";

/// One quota of the report: the name it is reported under, the maximum the emulator must
/// report for it, and the usage expected before and after [`populate`].
struct QuotaCase {
    name: &'static str,
    max: i64,
    /// What a region holding nothing uses toward it.
    used_when_empty: i64,
    /// What the region uses toward it once [`populate`] has run.
    used_after_setup: i64,
}

/// Every entry the report must carry, in the order `AccountQuota`'s documentation lists
/// them. The maxima that are not imported constants are Amazon RDS's published default
/// quotas for a resource this service stores none of ("Total storage for all DB
/// instances": 100,000 GiB; "DB security groups": 25; "Authorizations per DB security
/// group": 20; "Custom endpoints per DB cluster": 5; "IAM roles per DB cluster" and "IAM
/// roles per DB instance": 5; "Read replicas per primary": 15; "Reserved DB instances":
/// 40).
const QUOTA_CASES: &[QuotaCase] = &[
    QuotaCase {
        name: "AllocatedStorage",
        max: 100_000,
        used_when_empty: 0,
        // The 20 GiB and 100 GiB the two instances below allocate.
        used_after_setup: 120,
    },
    QuotaCase {
        name: "AuthorizationsPerDBSecurityGroup",
        max: 20,
        used_when_empty: 0,
        used_after_setup: 0,
    },
    QuotaCase {
        name: "CustomEndpointsPerDBCluster",
        max: 5,
        used_when_empty: 0,
        used_after_setup: 0,
    },
    QuotaCase {
        name: "DBClusterParameterGroups",
        max: DB_CLUSTER_PARAMETER_GROUP_QUOTA_PER_REGION as i64,
        used_when_empty: 0,
        used_after_setup: 1,
    },
    QuotaCase {
        name: "DBClusterRoles",
        max: 5,
        used_when_empty: 0,
        used_after_setup: 0,
    },
    QuotaCase {
        name: "DBClusters",
        max: DB_CLUSTER_QUOTA_PER_REGION as i64,
        used_when_empty: 0,
        used_after_setup: 1,
    },
    QuotaCase {
        name: "DBInstanceRoles",
        max: 5,
        used_when_empty: 0,
        used_after_setup: 0,
    },
    QuotaCase {
        name: "DBInstances",
        max: DB_INSTANCE_QUOTA_PER_REGION as i64,
        used_when_empty: 0,
        used_after_setup: 2,
    },
    QuotaCase {
        name: "DBParameterGroups",
        max: DB_PARAMETER_GROUP_QUOTA_PER_REGION as i64,
        used_when_empty: 0,
        used_after_setup: 3,
    },
    QuotaCase {
        name: "DBSecurityGroups",
        max: 25,
        used_when_empty: 0,
        used_after_setup: 0,
    },
    QuotaCase {
        name: "DBSubnetGroups",
        max: DB_SUBNET_GROUP_QUOTA_PER_REGION as i64,
        used_when_empty: 0,
        used_after_setup: 2,
    },
    QuotaCase {
        name: "EventSubscriptions",
        max: EVENT_SUBSCRIPTION_QUOTA_PER_REGION as i64,
        used_when_empty: 0,
        used_after_setup: 1,
    },
    QuotaCase {
        name: "ManualClusterSnapshots",
        max: DB_CLUSTER_SNAPSHOT_QUOTA_PER_REGION as i64,
        used_when_empty: 0,
        used_after_setup: 0,
    },
    QuotaCase {
        name: "ManualSnapshots",
        max: DB_SNAPSHOT_QUOTA_PER_REGION as i64,
        used_when_empty: 0,
        used_after_setup: 1,
    },
    QuotaCase {
        name: "OptionGroups",
        max: OPTION_GROUP_QUOTA_PER_REGION as i64,
        used_when_empty: 0,
        used_after_setup: 2,
    },
    QuotaCase {
        name: "ReadReplicasPerMaster",
        max: 15,
        used_when_empty: 0,
        used_after_setup: 0,
    },
    QuotaCase {
        name: "ReservedDBInstances",
        max: 40,
        used_when_empty: 0,
        used_after_setup: 0,
    },
    QuotaCase {
        name: "SubnetsPerDBSubnetGroup",
        max: MAX_SUBNETS_PER_DB_SUBNET_GROUP as i64,
        used_when_empty: 0,
        // The fuller of the two subnet groups below names three subnets.
        used_after_setup: 3,
    },
];

/// The `<AccountQuota>` entries of a report, as `(name, used, max)`, in the order the
/// emulator reported them.
fn quotas(xml: &str) -> Vec<(String, i64, i64)> {
    subtrees(xml, "AccountQuota")
        .iter()
        .map(|entry| {
            (
                text(entry, "AccountQuotaName"),
                number(entry, "Used"),
                number(entry, "Max"),
            )
        })
        .collect()
}

/// One `<tag>` of an entry, as the `Long` it is modelled as.
fn number(entry: &str, tag: &str) -> i64 {
    let value = text(entry, tag);
    value
        .parse()
        .unwrap_or_else(|error| panic!("expected <{tag}> to be a number, got {value:?}: {error}"))
}

/// `DescribeAccountAttributes` in `region`.
async fn describe_in(router: &MultiServiceRouter, region: &str) -> Vec<(String, i64, i64)> {
    let body = expect_ok(
        &format!("DescribeAccountAttributes in {region}"),
        call_in(router, region, "DescribeAccountAttributes", &NO_PARAMS).await,
    );
    quotas(&body)
}

/// The usage the report carries for `name`.
fn used_of(report: &[(String, i64, i64)], name: &str) -> i64 {
    report
        .iter()
        .find(|(quota, _, _)| quota == name)
        .map(|(_, used, _)| *used)
        .unwrap_or_else(|| panic!("the report must carry a {name} quota, got {report:?}"))
}

/// Create one of each resource the table expects usage for, in `region`: two DB
/// instances allocating different storage, a snapshot of one of them, a DB cluster,
/// three DB parameter groups, one cluster parameter group, two option groups, two DB
/// subnet groups of different sizes and one event subscription.
async fn populate(router: &MultiServiceRouter, region: &str) {
    for (identifier, storage) in [("emu-quota-db-1", "20"), ("emu-quota-db-2", "100")] {
        expect_ok(
            identifier,
            call_in(
                router,
                region,
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", identifier),
                    ("Engine", "mysql"),
                    ("DBInstanceClass", "db.m5.large"),
                    ("AllocatedStorage", storage),
                ],
            )
            .await,
        );
    }
    // A snapshot is taken of an instance the clock has finished creating.
    wait_for_settle();
    expect_ok(
        "emu-quota-snap",
        call_in(
            router,
            region,
            "CreateDBSnapshot",
            &[
                ("DBSnapshotIdentifier", "emu-quota-snap"),
                ("DBInstanceIdentifier", "emu-quota-db-1"),
            ],
        )
        .await,
    );
    expect_ok(
        "emu-quota-cluster",
        call_in(
            router,
            region,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "emu-quota-cluster"),
                ("Engine", "aurora-mysql"),
            ],
        )
        .await,
    );
    for name in ["emu-quota-pg-1", "emu-quota-pg-2", "emu-quota-pg-3"] {
        expect_ok(
            name,
            call_in(
                router,
                region,
                "CreateDBParameterGroup",
                &[
                    ("DBParameterGroupName", name),
                    ("DBParameterGroupFamily", "mysql8.0"),
                    ("Description", "an account-quota fixture"),
                ],
            )
            .await,
        );
    }
    expect_ok(
        "emu-quota-cluster-pg",
        call_in(
            router,
            region,
            "CreateDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", "emu-quota-cluster-pg"),
                ("DBParameterGroupFamily", "aurora-mysql8.0"),
                ("Description", "an account-quota fixture"),
            ],
        )
        .await,
    );
    for name in ["emu-quota-og-1", "emu-quota-og-2"] {
        expect_ok(
            name,
            call_in(
                router,
                region,
                "CreateOptionGroup",
                &[
                    ("OptionGroupName", name),
                    ("EngineName", "mysql"),
                    ("MajorEngineVersion", "8.0"),
                    ("OptionGroupDescription", "an account-quota fixture"),
                ],
            )
            .await,
        );
    }
    // Two subnet groups of different sizes, so the subnets-per-group quota reports the
    // fuller one rather than either a count of groups or the last one created.
    expect_ok(
        "emu-quota-subgrp-1",
        call_in(
            router,
            region,
            "CreateDBSubnetGroup",
            &[
                ("DBSubnetGroupName", "emu-quota-subgrp-1"),
                ("DBSubnetGroupDescription", "an account-quota fixture"),
                ("SubnetIds.SubnetIdentifier.1", "subnet-0a1b2c3d"),
                ("SubnetIds.SubnetIdentifier.2", "subnet-0e4f5a6b"),
            ],
        )
        .await,
    );
    expect_ok(
        "emu-quota-subgrp-2",
        call_in(
            router,
            region,
            "CreateDBSubnetGroup",
            &[
                ("DBSubnetGroupName", "emu-quota-subgrp-2"),
                ("DBSubnetGroupDescription", "an account-quota fixture"),
                ("SubnetIds.SubnetIdentifier.1", "subnet-0a1b2c3d"),
                ("SubnetIds.SubnetIdentifier.2", "subnet-0e4f5a6b"),
                ("SubnetIds.SubnetIdentifier.3", "subnet-0c7d8e9f"),
            ],
        )
        .await,
    );
    expect_ok(
        "emu-quota-es",
        call_in(
            router,
            region,
            "CreateEventSubscription",
            &[
                ("SubscriptionName", "emu-quota-es"),
                ("SnsTopicArn", SNS_TOPIC),
            ],
        )
        .await,
    );
}

#[tokio::test]
async fn a_region_holding_nothing_reports_every_published_quota_with_no_usage() {
    let router = emulator();
    let report = describe_in(&router, REGION).await;

    let reported: Vec<&str> = report.iter().map(|(name, _, _)| name.as_str()).collect();
    let expected: Vec<&str> = QUOTA_CASES.iter().map(|case| case.name).collect();
    assert_eq!(
        reported, expected,
        "expected the published quota catalog, got {reported:?}"
    );

    for case in QUOTA_CASES {
        let (_, used, max) = report
            .iter()
            .find(|(name, _, _)| name == case.name)
            .expect("the names matched just above");
        assert_eq!(
            *used, case.used_when_empty,
            "case {:?}: expected a region holding nothing to use {}, got {used}",
            case.name, case.used_when_empty
        );
        assert_eq!(
            *max, case.max,
            "case {:?}: expected the maximum this service enforces ({}), got {max}",
            case.name, case.max
        );
    }
}

#[tokio::test]
async fn every_quota_counts_the_records_the_signed_region_holds() {
    let router = emulator();
    populate(&router, REGION).await;

    let report = describe_in(&router, REGION).await;
    for case in QUOTA_CASES {
        let used = used_of(&report, case.name);
        assert_eq!(
            used, case.used_after_setup,
            "case {:?}: expected the usage counted off the region's records ({}), got {used}",
            case.name, case.used_after_setup
        );
    }

    // No entry may claim more usage than its own maximum admits, whatever the store
    // holds: the report and the quota it reports come from one place.
    for (name, used, max) in &report {
        assert!(
            (0..=*max).contains(used),
            "case {name:?}: expected usage within 0..={max}, got {used}"
        );
    }
}

#[tokio::test]
async fn one_region_never_reports_another_regions_records() {
    let router = emulator();
    populate(&router, REGION).await;

    // The other region holds one instance of its own and nothing else.
    expect_ok(
        "the other region's instance",
        call_in(
            &router,
            OTHER_REGION,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-quota-elsewhere"),
                ("Engine", "mysql"),
                ("DBInstanceClass", "db.m5.large"),
                ("AllocatedStorage", "7"),
            ],
        )
        .await,
    );

    let elsewhere = describe_in(&router, OTHER_REGION).await;
    for (case, expected) in [
        ("DBInstances", 1),
        ("AllocatedStorage", 7),
        ("DBClusters", 0),
        ("DBParameterGroups", 0),
        ("OptionGroups", 0),
        ("DBSubnetGroups", 0),
        ("EventSubscriptions", 0),
        ("ManualSnapshots", 0),
        ("SubnetsPerDBSubnetGroup", 0),
    ] {
        let used = used_of(&elsewhere, case);
        assert_eq!(
            used, expected,
            "case {case:?}: expected the other region to report only its own records \
             ({expected}), got {used}"
        );
    }

    // ... and the signed region's own report is untouched by them.
    let here = describe_in(&router, REGION).await;
    for case in QUOTA_CASES {
        let used = used_of(&here, case.name);
        assert_eq!(
            used, case.used_after_setup,
            "case {:?}: expected the signed region's own usage ({}), got {used}",
            case.name, case.used_after_setup
        );
    }
}

#[tokio::test]
async fn the_usage_follows_a_delete_and_the_report_changes_nothing() {
    let router = emulator();
    populate(&router, REGION).await;

    // Reading the report twice reports the same thing: it counts records, it does not
    // consume anything.
    let first = describe_in(&router, REGION).await;
    let again = describe_in(&router, REGION).await;
    assert_eq!(
        first, again,
        "expected two reads of the report to agree, got {first:?} then {again:?}"
    );

    // The records it counted are still there for a later caller.
    let instances = expect_ok(
        "the instances after the report",
        call(&router, "DescribeDBInstances", &NO_PARAMS).await,
    );
    assert_eq!(
        subtrees(&instances, "DBInstance").len(),
        2,
        "expected both instances to survive the report, got {instances}"
    );

    // A delete frees the quota it consumed, because the usage is counted and never
    // accumulated.
    expect_ok(
        "delete one DB parameter group",
        call(
            &router,
            "DeleteDBParameterGroup",
            &[("DBParameterGroupName", "emu-quota-pg-3")],
        )
        .await,
    );
    let after = describe_in(&router, REGION).await;
    assert_eq!(
        used_of(&after, "DBParameterGroups"),
        2,
        "expected the deleted group to stop consuming its quota, got {after:?}"
    );
    assert_eq!(
        used_of(&after, "DBInstances"),
        2,
        "expected the other quotas to be unmoved by the delete, got {after:?}"
    );
}
