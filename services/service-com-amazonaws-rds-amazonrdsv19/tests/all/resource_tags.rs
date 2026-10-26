// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for this service's resource-tag slice: `AddTagsToResource`,
//! `ListTagsForResource` and `RemoveTagsFromResource`.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter` exactly
//! as the production router mounts it (through `db_instances::emulator`, the one mounting
//! helper this test target has), and each request is a real awsQuery form POST carrying the
//! SigV4 credential scope the router keys on. The assertions read the XML the emulator
//! answers with, so a handler that computed nothing could not pass them.
//!
//! The rules under test, each asserted over a TABLE spanning the class it generalises over
//! rather than over one request:
//!
//! * a tag write stores its tags on the resource its `ResourceName` ARN names, replacing
//!   the value of a key the resource already carries and appending one it does not, and a
//!   tag read reports exactly what the resource carries — the same list the resource's own
//!   describe reports, whatever resource FAMILY the ARN names;
//! * a tag removal drops exactly the keys it names and leaves the rest, so naming a key the
//!   resource does not carry changes nothing;
//! * an ARN naming no stored resource is that family's own not-found fault, while a
//!   `ResourceName` that could never name a resource of this service is the
//!   `InvalidParameterValue` the Query API answers a bad value with;
//! * the published per-resource tag quota and the tag key and value bounds hold across
//!   calls, not only within one.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    MAX_TAGS_PER_RESOURCE, MAX_TAG_KEY_LEN, MAX_TAG_VALUE_LEN, RESERVED_TAG_PREFIXES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, subtrees, texts, wait_for_settle, REGION,
};

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// Two subnet ids of the published form (`subnet-` plus eight hexadecimal characters) —
/// two because a DB subnet group "must cover at least two Availability Zones".
const SUBNET_ID: &str = "subnet-0a1b2c3d";
const SECOND_SUBNET_ID: &str = "subnet-0e4f5a6b";

/// The SNS topic the event subscription below publishes to. Written out rather than built
/// from [`REGION`] so it can live in a `const` table; the test asserts the two agree.
const SNS_TOPIC: &str = "arn:aws:sns:us-west-2:000000000000:rds-events";

/// One resource family under test: how to create a resource of it, where its create reports
/// the ARN, and how an ARN of that family naming nothing is answered.
struct FamilyCase {
    case: &'static str,
    /// The create that stores a resource of this family.
    action: &'static str,
    /// That create's parameters.
    params: &'static [(&'static str, &'static str)],
    /// The element its answer reports the new resource's ARN in.
    arn_element: &'static str,
    /// An ARN of this family naming nothing this service stores.
    absent_arn: &'static str,
    /// The wire error `<Code>` an ARN of this family naming nothing is refused with.
    not_found_code: &'static str,
}

/// The resource families a tag write is applied to and read back from.
///
/// The rows differ along what the rule generalises over: the resource FAMILY, which is what
/// decides which store holds the tags and which ARN resource type addresses it — a DB
/// instance (`db`), a DB cluster (`cluster`), a DB snapshot (`snapshot`), both parameter
/// group namespaces (`pg` and `cluster-pg`, different resources of the same name), an
/// option group (`og`), a DB subnet group (`subgrp`), a DB security group (`secgrp`), an
/// event subscription (`es`) and an
/// account-scoped global cluster (`global-cluster`), whose ARN carries no region at all.
/// The snapshot row also differs in that its resource is derived from another row's.
const FAMILIES: &[FamilyCase] = &[
    FamilyCase {
        case: "DB instance",
        action: "CreateDBInstance",
        params: &[
            ("DBInstanceIdentifier", "tagged-instance"),
            ("Engine", "mysql"),
            ("DBInstanceClass", "db.m5.large"),
        ],
        arn_element: "DBInstanceArn",
        absent_arn: "arn:aws:rds:us-west-2:000000000000:db:no-such-instance",
        not_found_code: "DBInstanceNotFound",
    },
    FamilyCase {
        case: "DB cluster",
        action: "CreateDBCluster",
        params: &[
            ("DBClusterIdentifier", "tagged-cluster"),
            ("Engine", "aurora-mysql"),
        ],
        arn_element: "DBClusterArn",
        absent_arn: "arn:aws:rds:us-west-2:000000000000:cluster:no-such-cluster",
        not_found_code: "DBClusterNotFoundFault",
    },
    FamilyCase {
        case: "DB snapshot of the instance above",
        action: "CreateDBSnapshot",
        params: &[
            ("DBSnapshotIdentifier", "tagged-snapshot"),
            ("DBInstanceIdentifier", "tagged-instance"),
        ],
        arn_element: "DBSnapshotArn",
        absent_arn: "arn:aws:rds:us-west-2:000000000000:snapshot:no-such-snapshot",
        not_found_code: "DBSnapshotNotFound",
    },
    FamilyCase {
        case: "DB parameter group",
        action: "CreateDBParameterGroup",
        params: &[
            ("DBParameterGroupName", "tagged-pg"),
            ("DBParameterGroupFamily", "mysql8.0"),
            ("Description", "a group under test"),
        ],
        arn_element: "DBParameterGroupArn",
        absent_arn: "arn:aws:rds:us-west-2:000000000000:pg:no-such-group",
        not_found_code: "DBParameterGroupNotFound",
    },
    FamilyCase {
        case: "DB cluster parameter group",
        action: "CreateDBClusterParameterGroup",
        params: &[
            ("DBClusterParameterGroupName", "tagged-cluster-pg"),
            ("DBParameterGroupFamily", "aurora-mysql8.0"),
            ("Description", "a group under test"),
        ],
        arn_element: "DBClusterParameterGroupArn",
        absent_arn: "arn:aws:rds:us-west-2:000000000000:cluster-pg:no-such-group",
        not_found_code: "DBClusterParameterGroupNotFound",
    },
    FamilyCase {
        case: "option group",
        action: "CreateOptionGroup",
        params: &[
            ("OptionGroupName", "tagged-og"),
            ("EngineName", "mysql"),
            ("MajorEngineVersion", "8.0"),
            ("OptionGroupDescription", "a group under test"),
        ],
        arn_element: "OptionGroupArn",
        absent_arn: "arn:aws:rds:us-west-2:000000000000:og:no-such-group",
        not_found_code: "OptionGroupNotFoundFault",
    },
    FamilyCase {
        case: "DB subnet group",
        action: "CreateDBSubnetGroup",
        params: &[
            ("DBSubnetGroupName", "tagged-subgrp"),
            ("DBSubnetGroupDescription", "a group under test"),
            ("SubnetIds.SubnetIdentifier.1", SUBNET_ID),
            ("SubnetIds.SubnetIdentifier.2", SECOND_SUBNET_ID),
        ],
        arn_element: "DBSubnetGroupArn",
        absent_arn: "arn:aws:rds:us-west-2:000000000000:subgrp:no-such-group",
        not_found_code: "DBSubnetGroupNotFoundFault",
    },
    FamilyCase {
        case: "DB security group",
        action: "CreateDBSecurityGroup",
        params: &[
            ("DBSecurityGroupName", "tagged-secgrp"),
            ("DBSecurityGroupDescription", "a group under test"),
        ],
        arn_element: "DBSecurityGroupArn",
        absent_arn: "arn:aws:rds:us-west-2:000000000000:secgrp:no-such-group",
        not_found_code: "DBSecurityGroupNotFound",
    },
    FamilyCase {
        case: "event subscription",
        action: "CreateEventSubscription",
        params: &[
            ("SubscriptionName", "tagged-es"),
            ("SnsTopicArn", SNS_TOPIC),
        ],
        arn_element: "EventSubscriptionArn",
        absent_arn: "arn:aws:rds:us-west-2:000000000000:es:no-such-subscription",
        not_found_code: "SubscriptionNotFound",
    },
    FamilyCase {
        case: "global cluster, whose ARN carries no region",
        action: "CreateGlobalCluster",
        params: &[
            ("GlobalClusterIdentifier", "tagged-global"),
            ("Engine", "aurora-mysql"),
        ],
        arn_element: "GlobalClusterArn",
        absent_arn: "arn:aws:rds::000000000000:global-cluster:no-such-global",
        not_found_code: "GlobalClusterNotFoundFault",
    },
];

/// The families no create of this service stores a resource of yet, so an ARN of one can
/// only ever name nothing. They belong to the not-found table alone — the ARN resource
/// types are the ones each family's own record renders.
const UNPOPULATED_FAMILIES: &[(&str, &str, &str)] = &[
    (
        "DB cluster snapshot",
        "arn:aws:rds:us-west-2:000000000000:cluster-snapshot:no-such-snapshot",
        "DBClusterSnapshotNotFoundFault",
    ),
    (
        "DB shard group",
        "arn:aws:rds:us-west-2:000000000000:shard-group:no-such-shard-group",
        "DBShardGroupNotFound",
    ),
    (
        "blue/green deployment",
        "arn:aws:rds:us-west-2:000000000000:blue-green-deployment:bgd-0123456789abcdef",
        "BlueGreenDeploymentNotFoundFault",
    ),
    (
        "zero-ETL integration",
        "arn:aws:rds:us-west-2:000000000000:integration:no-such-integration",
        "IntegrationNotFoundFault",
    ),
    (
        "RDS proxy",
        "arn:aws:rds:us-west-2:000000000000:db-proxy:no-such-proxy",
        "DBProxyNotFoundFault",
    ),
    (
        "tenant database",
        "arn:aws:rds:us-west-2:000000000000:tenant-database:tdb-nosuchtenantdatabase",
        "TenantDatabaseNotFound",
    ),
    (
        "custom DB engine version",
        "arn:aws:rds:us-west-2:000000000000:cev:custom-oracle-ee/19.cdb_cev1/no-such-cev",
        "CustomDBEngineVersionNotFoundFault",
    ),
];

/// The three tag operations, so a refusal table asserts the SAME answer from each: they
/// address their resource the same way, so they refuse it the same way.
const TAG_ACTIONS: &[(&str, &[(&str, &str)])] = &[
    (
        "AddTagsToResource",
        &[("Tags.Tag.1.Key", "owner"), ("Tags.Tag.1.Value", "platform")],
    ),
    ("ListTagsForResource", &[]),
    ("RemoveTagsFromResource", &[("TagKeys.member.1", "owner")]),
];

/// The awsQuery parameters that carry `tags` on an `AddTagsToResource` call.
fn tag_params(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    tags.iter()
        .enumerate()
        .flat_map(|(index, (key, value))| {
            [
                (format!("Tags.Tag.{}.Key", index + 1), (*key).to_string()),
                (format!("Tags.Tag.{}.Value", index + 1), (*value).to_string()),
            ]
        })
        .collect()
}

/// `AddTagsToResource` for `resource_name`, in [`REGION`].
async fn add_tags(
    router: &MultiServiceRouter,
    resource_name: &str,
    tags: &[(&str, &str)],
) -> (u16, String) {
    let mut params = vec![("ResourceName".to_string(), resource_name.to_string())];
    params.extend(tag_params(tags));
    call(router, "AddTagsToResource", &params).await
}

/// `RemoveTagsFromResource` for `resource_name`, in [`REGION`].
async fn remove_tags(
    router: &MultiServiceRouter,
    resource_name: &str,
    keys: &[&str],
) -> (u16, String) {
    let mut params = vec![("ResourceName".to_string(), resource_name.to_string())];
    for (index, key) in keys.iter().enumerate() {
        params.push((
            format!("TagKeys.member.{}", index + 1),
            (*key).to_string(),
        ));
    }
    call(router, "RemoveTagsFromResource", &params).await
}

/// The tags `ListTagsForResource` reports for `resource_name`, in the order it reports
/// them.
async fn list_tags(
    case: &str,
    router: &MultiServiceRouter,
    resource_name: &str,
) -> Vec<(String, String)> {
    let body = expect_ok(
        case,
        call(
            router,
            "ListTagsForResource",
            &[("ResourceName", resource_name)],
        )
        .await,
    );
    reported_tags(case, &body)
}

/// The `(Key, Value)` of every `<Tag>` in `xml`, in document order.
fn reported_tags(case: &str, xml: &str) -> Vec<(String, String)> {
    subtrees(xml, "Tag")
        .iter()
        .map(|tag| {
            let keys = texts(tag, "Key");
            let values = texts(tag, "Value");
            assert_eq!(
                keys.len(),
                1,
                "case {case:?}: expected one <Key> per <Tag>, got {} in {tag}",
                keys.len()
            );
            let value = values.first().cloned().unwrap_or_default();
            (keys[0].clone(), value)
        })
        .collect()
}

/// The tags, as `(key, value)`, a row's resource must carry at one point of the walk.
fn expected(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    tags.iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

#[tokio::test]
async fn a_tag_write_is_read_back_by_list_for_every_resource_family() {
    assert!(
        SNS_TOPIC.contains(REGION),
        "the SNS topic the table uses must be in {REGION}, got {SNS_TOPIC}"
    );
    let router = emulator();

    for row in FAMILIES {
        // The resource the tags go on, and the ARN its own create reported for it: a tag
        // call can only ever address a resource by the ARN that resource reports.
        let created = expect_ok(row.case, call(&router, row.action, row.params).await);
        // The transition the create owes, so a resource a later row derives from (the
        // snapshot's source instance) is `available` by the time that row runs.
        wait_for_settle();
        let arns = texts(&created, row.arn_element);
        assert_eq!(
            arns.len(),
            1,
            "case {:?}: expected one <{}> in {created}, got {}",
            row.case,
            row.arn_element,
            arns.len()
        );
        let arn = arns[0].clone();
        assert!(
            arn.starts_with("arn:aws:rds:") && arn.contains(ACCOUNT),
            "case {:?}: expected an Amazon RDS ARN, got {arn:?}",
            row.case
        );

        // A resource nobody has tagged carries no tags, and reports that as an EMPTY list
        // rather than as a refusal.
        let before = list_tags(row.case, &router, &arn).await;
        assert!(
            before.is_empty(),
            "case {:?}: a resource nobody tagged must report no tags, got {before:?}",
            row.case
        );

        // The write, then the read: what comes back is what went in.
        let first = [("owner", "platform"), ("env", "test")];
        expect_ok(row.case, add_tags(&router, &arn, &first).await);
        let after_first = list_tags(row.case, &router, &arn).await;
        assert_eq!(
            after_first,
            expected(&first),
            "case {:?}: expected the tags the write carried, got {after_first:?}",
            row.case
        );

        // A second write REPLACES the value of a key the resource already carries and
        // appends the key it does not, so the count grows by one rather than by two.
        expect_ok(
            row.case,
            add_tags(&router, &arn, &[("env", "prod"), ("tier", "db")]).await,
        );
        let after_second = list_tags(row.case, &router, &arn).await;
        assert_eq!(
            after_second,
            expected(&[("owner", "platform"), ("env", "prod"), ("tier", "db")]),
            "case {:?}: expected the second write to replace env and append tier, got \
             {after_second:?}",
            row.case
        );

        // A removal drops exactly the keys it names: one the resource carries and one it
        // does not, so naming an absent key changes nothing.
        expect_ok(
            row.case,
            remove_tags(&router, &arn, &["owner", "never-set"]).await,
        );
        let after_remove = list_tags(row.case, &router, &arn).await;
        assert_eq!(
            after_remove,
            expected(&[("env", "prod"), ("tier", "db")]),
            "case {:?}: expected only the named key removed, got {after_remove:?}",
            row.case
        );

        // Removing the rest leaves the resource carrying nothing — and still resolvable,
        // which is what tells an untagged resource from one that is not there.
        expect_ok(row.case, remove_tags(&router, &arn, &["env", "tier"]).await);
        let emptied = list_tags(row.case, &router, &arn).await;
        assert!(
            emptied.is_empty(),
            "case {:?}: expected no tags left, got {emptied:?}",
            row.case
        );
    }
}

/// One row of the round trip through a resource's OWN describe: the resource, and the
/// describe that reports its `TagList`.
struct DescribeCase {
    case: &'static str,
    /// The resource type its ARN carries.
    resource_type: &'static str,
    /// The resource's identifier.
    identifier: &'static str,
    /// The describe that reports it.
    describe: &'static str,
    /// That describe's parameters.
    params: &'static [(&'static str, &'static str)],
}

#[tokio::test]
async fn a_tag_write_reaches_the_list_the_resources_own_describe_reports() {
    let router = emulator();

    // One resource of each of the two families whose describe reports a `TagList`, so the
    // assertion is that the tag operations write the resource's OWN list rather than a copy
    // beside it.
    expect_ok(
        "instance",
        call(
            &router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "described-instance"),
                ("Engine", "mysql"),
                ("DBInstanceClass", "db.m5.large"),
                // One tag from the create, so the write below merges into a list that is
                // not empty to begin with.
                ("Tags.Tag.1.Key", "created-with"),
                ("Tags.Tag.1.Value", "the-create"),
            ],
        )
        .await,
    );
    expect_ok(
        "cluster",
        call(
            &router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "described-cluster"),
                ("Engine", "aurora-mysql"),
            ],
        )
        .await,
    );
    wait_for_settle();

    let rows: &[DescribeCase] = &[
        DescribeCase {
            case: "DB instance",
            resource_type: "db",
            identifier: "described-instance",
            describe: "DescribeDBInstances",
            params: &[("DBInstanceIdentifier", "described-instance")],
        },
        DescribeCase {
            case: "DB cluster",
            resource_type: "cluster",
            identifier: "described-cluster",
            describe: "DescribeDBClusters",
            params: &[("DBClusterIdentifier", "described-cluster")],
        },
    ];

    for row in rows {
        let DescribeCase {
            case,
            resource_type,
            identifier,
            describe,
            params,
        } = row;
        let arn = format!("arn:aws:rds:{REGION}:{ACCOUNT}:{resource_type}:{identifier}");
        expect_ok(case, add_tags(&router, &arn, &[("owner", "platform")]).await);
        let described = expect_ok(case, call(&router, describe, params).await);
        let reported = reported_tags(case, &described);
        let listed = list_tags(case, &router, &arn).await;
        assert_eq!(
            reported, listed,
            "case {case:?}: the resource's own describe must report the tags \
             ListTagsForResource reports, got {reported:?} and {listed:?}"
        );
        assert!(
            listed.contains(&("owner".to_string(), "platform".to_string())),
            "case {case:?}: expected the written tag in {listed:?}"
        );

        // And the removal reaches the same list.
        expect_ok(case, remove_tags(&router, &arn, &["owner"]).await);
        let after = expect_ok(case, call(&router, describe, params).await);
        let reported = reported_tags(case, &after);
        assert!(
            !reported.iter().any(|(key, _)| key == "owner"),
            "case {case:?}: expected the removed key gone from the describe, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn an_arn_that_names_nothing_is_that_familys_not_found_fault() {
    let router = emulator();

    let mut rows: Vec<(&str, &str, &str)> = FAMILIES
        .iter()
        .map(|row| (row.case, row.absent_arn, row.not_found_code))
        .collect();
    rows.extend(
        UNPOPULATED_FAMILIES
            .iter()
            .map(|(case, arn, code)| (*case, *arn, *code)),
    );

    for (family, absent_arn, code) in rows {
        for (action, extra) in TAG_ACTIONS {
            let case = format!("{action} on an absent {family}");
            let mut params = vec![("ResourceName".to_string(), absent_arn.to_string())];
            for (name, value) in extra.iter() {
                params.push(((*name).to_string(), (*value).to_string()));
            }
            expect_error(&case, call(&router, action, &params).await, 404, code);
        }
    }
}

#[tokio::test]
async fn a_resource_name_this_service_could_not_hold_is_a_bad_parameter_value() {
    let router = emulator();

    // Each row is a `ResourceName` that names no resource this service could ever store:
    // not an ARN at all, an ARN of another service, an ARN with no resource type, and an
    // Amazon RDS ARN of a resource type this service keeps no records of.
    let rows: &[(&str, &str)] = &[
        ("a bare identifier", "tagged-instance"),
        ("another service's ARN", SNS_TOPIC),
        (
            "an RDS ARN with no resource type",
            "arn:aws:rds:us-west-2:000000000000:tagged-instance",
        ),
        (
            "an RDS ARN whose account is not an account id",
            "arn:aws:rds:us-west-2:not-an-account:db:tagged-instance",
        ),
        (
            "a reserved DB instance, a type this service stores none of",
            "arn:aws:rds:us-west-2:000000000000:ri:reserved-1",
        ),
    ];

    for (case, resource_name) in rows {
        for (action, extra) in TAG_ACTIONS {
            let case = format!("{action} with {case}");
            let mut params = vec![("ResourceName".to_string(), (*resource_name).to_string())];
            for (name, value) in extra.iter() {
                params.push(((*name).to_string(), (*value).to_string()));
            }
            expect_error(
                &case,
                call(&router, action, &params).await,
                400,
                "InvalidParameterValue",
            );
        }
    }
}

#[tokio::test]
async fn tags_are_regional_and_filters_are_refused() {
    let router = emulator();

    expect_ok(
        "create",
        call(
            &router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "regional-instance"),
                ("Engine", "mysql"),
                ("DBInstanceClass", "db.m5.large"),
            ],
        )
        .await,
    );
    wait_for_settle();
    let arn = format!("arn:aws:rds:{REGION}:{ACCOUNT}:db:regional-instance");
    expect_ok("tag it", add_tags(&router, &arn, &[("owner", "platform")]).await);

    // The SAME ARN, read by a request signed for another region: RDS is regional, so the
    // instance is not in scope there and the answer is the modelled 404, never another
    // region's tags.
    let other_region = "eu-central-1";
    expect_error(
        "a signed-for-elsewhere read of the same ARN",
        call_in(
            &router,
            other_region,
            "ListTagsForResource",
            &[("ResourceName", arn.as_str())],
        )
        .await,
        404,
        "DBInstanceNotFound",
    );
    // And a resource of the same identifier in that other region carries its own tags,
    // not the first one's.
    expect_ok(
        "create elsewhere",
        call_in(
            &router,
            other_region,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "regional-instance"),
                ("Engine", "mysql"),
                ("DBInstanceClass", "db.m5.large"),
            ],
        )
        .await,
    );
    wait_for_settle();
    let elsewhere = format!("arn:aws:rds:{other_region}:{ACCOUNT}:db:regional-instance");
    let body = expect_ok(
        "read elsewhere",
        call_in(
            &router,
            other_region,
            "ListTagsForResource",
            &[("ResourceName", elsewhere.as_str())],
        )
        .await,
    );
    let listed = reported_tags("read elsewhere", &body);
    assert!(
        listed.is_empty(),
        "case \"read elsewhere\": another region's instance of the same name must carry its \
         own tags, got {listed:?}"
    );

    // `ListTagsForResource$Filters`: "This parameter isn't currently supported" — refused
    // rather than ignored, which is what keeps a caller from reading a filtered list it
    // never got.
    expect_error(
        "a filtered read",
        call(
            &router,
            "ListTagsForResource",
            &[
                ("ResourceName", arn.as_str()),
                ("Filters.Filter.1.Name", "tag-key"),
                ("Filters.Filter.1.Values.Value.1", "owner"),
            ],
        )
        .await,
        400,
        "ValidationException",
    );
}

#[tokio::test]
async fn the_published_tag_bounds_hold_across_calls() {
    let router = emulator();

    expect_ok(
        "create",
        call(
            &router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "bounded-instance"),
                ("Engine", "mysql"),
                ("DBInstanceClass", "db.m5.large"),
            ],
        )
        .await,
    );
    wait_for_settle();
    let arn = format!("arn:aws:rds:{REGION}:{ACCOUNT}:db:bounded-instance");

    // Each bound at its published value and one past it: the key and value lengths
    // `Tag$Key` and `Tag$Value` document (128 and 256 Unicode characters), and the two
    // prefixes they reserve.
    let longest_key = "k".repeat(MAX_TAG_KEY_LEN);
    let over_long_key = "k".repeat(MAX_TAG_KEY_LEN + 1);
    let longest_value = "v".repeat(MAX_TAG_VALUE_LEN);
    let over_long_value = "v".repeat(MAX_TAG_VALUE_LEN + 1);

    let accepted: &[(&str, &[(&str, &str)])] = &[
        ("a key at its published length", &[(&longest_key, "v")]),
        ("a value at its published length", &[("at-length", &longest_value)]),
        ("a tag with no value at all", &[("value-less", "")]),
    ];
    for (case, tags) in accepted {
        expect_ok(case, add_tags(&router, &arn, tags).await);
    }

    let mut refused: Vec<(String, Vec<(&str, &str)>)> = vec![
        (
            "a key one character past its published length".to_string(),
            vec![(over_long_key.as_str(), "v")],
        ),
        (
            "a value one character past its published length".to_string(),
            vec![("past-length", over_long_value.as_str())],
        ),
        ("an empty key".to_string(), vec![("", "v")]),
    ];
    // `Tag$Key` / `Tag$Value`: "can't be prefixed with `aws:` or `rds:`".
    for prefix in RESERVED_TAG_PREFIXES {
        refused.push((
            format!("a key prefixed with {prefix}"),
            vec![(prefix, "reserved")],
        ));
    }
    for (case, tags) in &refused {
        expect_error(
            case,
            add_tags(&router, &arn, tags).await,
            400,
            "InvalidParameterValue",
        );
    }

    // The per-resource quota: one request past it is refused, and so is the request that
    // would carry a resource past it ONE TAG AT A TIME — the bound is on the resource, not
    // on the call.
    let one_per_slot: Vec<(String, String)> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|index| (format!("quota-key-{index}"), format!("v{index}")))
        .collect();
    let at_the_limit: Vec<(&str, &str)> = one_per_slot
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let mut past_the_limit = at_the_limit.clone();
    past_the_limit.push(("one-too-many", "v"));
    expect_error(
        "one request past the per-resource quota",
        add_tags(&router, &arn, &past_the_limit).await,
        400,
        "InvalidParameterValue",
    );

    // A resource with exactly the quota's worth of tags, reached in one call on a resource
    // of its own.
    expect_ok(
        "create",
        call(
            &router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "quota-instance"),
                ("Engine", "mysql"),
                ("DBInstanceClass", "db.m5.large"),
            ],
        )
        .await,
    );
    wait_for_settle();
    let quota_arn = format!("arn:aws:rds:{REGION}:{ACCOUNT}:db:quota-instance");
    expect_ok(
        "tags up to the quota",
        add_tags(&router, &quota_arn, &at_the_limit).await,
    );
    let listed = list_tags("tags up to the quota", &router, &quota_arn).await;
    assert_eq!(
        listed.len(),
        MAX_TAGS_PER_RESOURCE,
        "case \"tags up to the quota\": expected {MAX_TAGS_PER_RESOURCE} tags, got {}",
        listed.len()
    );
    // A key the resource already carries is a REPLACEMENT, so it is served at the quota.
    expect_ok(
        "replacing a key at the quota",
        add_tags(&router, &quota_arn, &[("quota-key-0", "replaced")]).await,
    );
    let replaced = list_tags("replacing a key at the quota", &router, &quota_arn).await;
    assert_eq!(
        replaced.len(),
        MAX_TAGS_PER_RESOURCE,
        "case \"replacing a key at the quota\": a replacement must not grow the list, got {}",
        replaced.len()
    );
    assert_eq!(
        replaced.first().map(|(_, value)| value.as_str()),
        Some("replaced"),
        "case \"replacing a key at the quota\": expected the replaced value, got {replaced:?}"
    );
    // One MORE key, in its own call, is past the quota.
    expect_error(
        "one key past the quota, one call at a time",
        add_tags(&router, &quota_arn, &[("one-too-many", "v")]).await,
        400,
        "InvalidParameterValue",
    );
    // And freeing a slot makes room again, so the bound is on what the resource carries.
    expect_ok(
        "freeing a slot",
        remove_tags(&router, &quota_arn, &["quota-key-0"]).await,
    );
    expect_ok(
        "the freed slot takes a new key",
        add_tags(&router, &quota_arn, &[("one-too-many", "v")]).await,
    );
}
