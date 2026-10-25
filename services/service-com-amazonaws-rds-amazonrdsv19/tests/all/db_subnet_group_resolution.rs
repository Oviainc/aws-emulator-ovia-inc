// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how this service RESOLVES a `DBSubnetGroupName`, across every
//! operation that names one: `DeleteDBSubnetGroup`, `DescribeDBSubnetGroups`,
//! `CreateDBInstance` and `CreateDBCluster`.
//!
//! The rule under test: a request that names a DB subnet group is resolved against the
//! one set of groups the service holds in the region it was signed for — the groups a
//! create persisted plus the account-owned group of the default VPC — so no operation
//! reports as absent a group another operation resolves, and all of them answer the same
//! not-found fault, naming the group in the folded spelling it is stored under, for a
//! name that set does not hold. The account-owned group is the one name the DELETE's own
//! member excludes ("Must not be default"), so that request is refused for the value it
//! carries rather than reported absent.
//!
//! The table spans the classes the rule generalises over: which operation names the
//! group, the shape of a name nothing holds (a fresh name, a near miss of a stored one,
//! a single letter, mixed case, a group another region holds) and the spellings of the
//! account-owned name.
//!
//! Every call goes over the wire through the harness in `db_instances.rs`, so the
//! assertions read the XML the emulator itself answered with.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_rds_amazonrdsv19::state::DEFAULT_DB_SUBNET_GROUP;

use crate::db_instances::{call, call_in, emulator, expect_error, expect_ok, text, texts, REGION};

/// A second region, so a group that exists but is out of the signed region's scope is a
/// row of the table.
const OTHER_REGION: &str = "eu-central-1";

/// One operation that NAMES a DB subnet group: how to call it, and the identifier member
/// it needs for the resource it would create.
struct NamingOperation {
    action: &'static str,
    /// The member carrying the identifier of the resource the call creates, if any.
    identifier_member: Option<&'static str>,
    /// The other required members of the call.
    extra: &'static [(&'static str, &'static str)],
}

/// Every operation of this service that resolves a `DBSubnetGroupName` against the store.
const NAMING_OPERATIONS: &[NamingOperation] = &[
    NamingOperation {
        action: "DeleteDBSubnetGroup",
        identifier_member: None,
        extra: &[],
    },
    NamingOperation {
        action: "DescribeDBSubnetGroups",
        identifier_member: None,
        extra: &[],
    },
    NamingOperation {
        action: "CreateDBInstance",
        identifier_member: Some("DBInstanceIdentifier"),
        extra: &[("Engine", "mysql"), ("DBInstanceClass", "db.m5.large")],
    },
    NamingOperation {
        action: "CreateDBCluster",
        identifier_member: Some("DBClusterIdentifier"),
        extra: &[("Engine", "aurora-mysql")],
    },
];

/// The names nothing holds in [`REGION`], one per shape the rule generalises over.
const ABSENT_NAMES: &[(&str, &str)] = &[
    ("a name nothing was created under", "emu-res-no-such-group"),
    ("a near miss of a stored name", "emu-res-subnets-2"),
    ("a single letter", "e"),
    ("the stored name in mixed case, misspelled", "EMU-Res-Absent"),
    ("a group only another region holds", "emu-res-elsewhere"),
];

/// One call of `op` in `region`, naming `group` and, where the operation creates a
/// resource, `identifier`.
async fn call_naming(
    router: &MultiServiceRouter,
    region: &str,
    op: &NamingOperation,
    group: &str,
    identifier: &str,
) -> (u16, String) {
    let mut params: Vec<(&str, String)> = vec![("DBSubnetGroupName", group.to_string())];
    if let Some(member) = op.identifier_member {
        params.push((member, identifier.to_string()));
    }
    params.extend(
        op.extra
            .iter()
            .map(|(name, value)| (*name, (*value).to_string())),
    );
    call_in(router, region, op.action, &params).await
}

/// `CreateDBSubnetGroup` in `region`, with the two subnets a group must name.
async fn create_group(router: &MultiServiceRouter, region: &str, name: &str) -> (u16, String) {
    call_in(
        router,
        region,
        "CreateDBSubnetGroup",
        &[
            ("DBSubnetGroupName", name),
            ("DBSubnetGroupDescription", "subnet group resolution"),
            ("SubnetIds.SubnetIdentifier.1", "subnet-0123456789abcdef0"),
            ("SubnetIds.SubnetIdentifier.2", "subnet-0fedcba987654321f"),
        ],
    )
    .await
}

#[tokio::test]
async fn every_operation_answers_the_same_not_found_for_a_group_the_region_does_not_hold() {
    let router = emulator();

    // One group this region holds, so a near miss of its name is a row, and one that
    // only another region holds.
    expect_ok(
        "the stored group",
        create_group(&router, REGION, "emu-res-subnets").await,
    );
    expect_ok(
        "the other region's group",
        create_group(&router, OTHER_REGION, "emu-res-elsewhere").await,
    );

    for (index, (case, group)) in ABSENT_NAMES.iter().enumerate() {
        for op in NAMING_OPERATIONS {
            let full_case = format!("{case} named by {}", op.action);
            let identifier = format!("emu-res-{index}-{}", op.action.to_ascii_lowercase());
            let body = expect_error(
                &full_case,
                call_naming(&router, REGION, op, group, &identifier).await,
                404,
                "DBSubnetGroupNotFoundFault",
            );
            let message = text(&body, "Message");
            assert!(
                message.contains(&group.to_ascii_lowercase()),
                "case {full_case:?}: expected the fault to name the group \
                 {:?} it could not resolve, got {message:?}",
                group.to_ascii_lowercase()
            );
        }
    }
}

#[tokio::test]
async fn the_account_owned_group_resolves_everywhere_and_its_delete_is_refused_on_the_value() {
    let router = emulator();

    // It resolves for every operation that reads or places a resource in it ...
    let described = expect_ok(
        "the account-owned group, described",
        call(
            &router,
            "DescribeDBSubnetGroups",
            &[("DBSubnetGroupName", DEFAULT_DB_SUBNET_GROUP)],
        )
        .await,
    );
    assert_eq!(
        text(&described, "DBSubnetGroupName"),
        DEFAULT_DB_SUBNET_GROUP,
        "expected the account-owned group to be reported, got {described}"
    );
    for op in NAMING_OPERATIONS
        .iter()
        .filter(|op| op.identifier_member.is_some())
    {
        let case = format!("the account-owned group named by {}", op.action);
        expect_ok(
            &case,
            call_naming(
                &router,
                REGION,
                op,
                DEFAULT_DB_SUBNET_GROUP,
                &format!("emu-res-default-{}", op.action.to_ascii_lowercase()),
            )
            .await,
        );
    }

    // ... and the one operation whose member excludes it refuses the VALUE, whatever
    // spelling it arrives in, rather than reporting the group absent.
    for spelling in [DEFAULT_DB_SUBNET_GROUP, "Default", "DEFAULT"] {
        let body = expect_error(
            spelling,
            call(
                &router,
                "DeleteDBSubnetGroup",
                &[("DBSubnetGroupName", spelling)],
            )
            .await,
            400,
            "InvalidParameterValue",
        );
        let message = text(&body, "Message");
        assert!(
            message.contains("DBSubnetGroupName") && message.contains(DEFAULT_DB_SUBNET_GROUP),
            "case {spelling:?}: expected the refusal to name the member and the value it \
             excludes, got {message:?}"
        );
    }

    // The refused delete removed nothing: the group is still the one every other
    // operation resolves.
    let after = expect_ok(
        "the account-owned group after the refusals",
        call(&router, "DescribeDBSubnetGroups", &[("MaxRecords", "20")]).await,
    );
    assert!(
        texts(&after, "DBSubnetGroupName").contains(&DEFAULT_DB_SUBNET_GROUP.to_string()),
        "expected the report to still carry the account-owned group, got {after}"
    );
}
