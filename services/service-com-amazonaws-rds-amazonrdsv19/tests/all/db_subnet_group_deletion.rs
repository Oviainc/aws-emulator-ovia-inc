// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the DB subnet group DELETE path: `DeleteDBSubnetGroup` against
//! what `CreateDBSubnetGroup` persisted, and against what this service no longer holds.
//!
//! Every call goes over the wire, through the same mounting helper and awsQuery form POST
//! the sibling suites use, so the assertions read the XML the emulator itself answered
//! with.
//!
//! The rule under test is the one `option_group_deletion.rs` pins for its resource, one
//! resource type along: a group a create persisted is visible to every later call that
//! looks it up — so the delete of a group that exists is ACCEPTED, and the group is gone
//! afterwards — while the modelled not-found fault is answered only for a name nothing
//! created, or one whose group a delete has already removed. The one documented refusal in
//! between is a group that is in use: "the specified database subnet group must not be
//! associated with any DB instances", which is a state fault rather than a not-found.
//!
//! The table spans the classes the rule generalises over: the shape of the name, how many
//! subnets the group names and in which spelling of the EC2 subnet id, the description it
//! carries, whether it is tagged, and the region it was created in.

use smithy_server_runtime::MultiServiceRouter;

use std::thread::sleep;
use std::time::Duration;

use service_com_amazonaws_rds_amazonrdsv19::state::STATUS_TRANSITION_DELAY;

use crate::db_instances::{call, call_in, emulator, expect_error, expect_ok, text, REGION};

/// A second region, so the regional scope of a delete is observable.
const OTHER_REGION: &str = "eu-west-1";

/// One row of the create/delete round trip.
struct DeletionCase {
    case: &'static str,
    name: &'static str,
    description: &'static str,
    /// The subnets the group names: at least two distinct ones, as a group covering two
    /// Availability Zones must name.
    subnets: &'static [&'static str],
    /// A tag the create carries, if any: a tagged group is deleted the same way.
    tag: Option<(&'static str, &'static str)>,
}

/// The groups the round trip creates, deletes and then looks up again.
const DELETION_CASES: &[DeletionCase] = &[
    DeletionCase {
        case: "two subnets of the 17-character id form",
        name: "emu-del-subnets",
        description: "deletion-rule subnets",
        subnets: &["subnet-0123456789abcdef0", "subnet-0fedcba987654321f"],
        tag: None,
    },
    DeletionCase {
        case: "three subnets of the legacy 8-character id form, a tagged group",
        name: "emu.del_subnets 2",
        description: "a description with spaces and punctuation.",
        subnets: &["subnet-0a1b2c3d", "subnet-4e5f6071", "subnet-82939495"],
        tag: Some(("env", "probe")),
    },
    DeletionCase {
        case: "a name at the edge of the alphabet its member admits",
        name: "EMU-Del-Subnets-Mixed",
        description: "mixed case is folded to the stored name",
        subnets: &["subnet-00000000", "subnet-11111111"],
        tag: None,
    },
];

/// `CreateDBSubnetGroup` with its three required members, in `region`.
async fn create_group(
    router: &MultiServiceRouter,
    region: &str,
    row: &DeletionCase,
) -> (u16, String) {
    let mut params: Vec<(String, String)> = vec![
        ("DBSubnetGroupName".to_string(), row.name.to_string()),
        (
            "DBSubnetGroupDescription".to_string(),
            row.description.to_string(),
        ),
    ];
    for (index, subnet) in row.subnets.iter().enumerate() {
        let position = index + 1;
        params.push((
            format!("SubnetIds.SubnetIdentifier.{position}"),
            (*subnet).to_string(),
        ));
    }
    if let Some((key, value)) = row.tag {
        params.push(("Tags.Tag.1.Key".to_string(), key.to_string()));
        params.push(("Tags.Tag.1.Value".to_string(), value.to_string()));
    }
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    call_in(router, region, "CreateDBSubnetGroup", &borrowed).await
}

#[tokio::test]
async fn deleting_a_persisted_group_is_accepted_and_only_then_is_it_not_found() {
    let router = emulator();

    for row in DELETION_CASES {
        expect_ok(row.case, create_group(&router, REGION, row).await);
        let deleted = expect_ok(
            row.case,
            call(
                &router,
                "DeleteDBSubnetGroup",
                &[("DBSubnetGroupName", row.name)],
            )
            .await,
        );
        assert!(
            deleted.contains("<DeleteDBSubnetGroupResponse"),
            "case {:?}: expected the delete's own empty-output envelope, got {deleted}",
            row.case
        );
        // Only now is the name not found — by the report and by a second delete alike,
        // which is what lets a caller tell "deleted" from "never existed".
        for action in ["DescribeDBSubnetGroups", "DeleteDBSubnetGroup"] {
            expect_error(
                row.case,
                call(&router, action, &[("DBSubnetGroupName", row.name)]).await,
                404,
                "DBSubnetGroupNotFoundFault",
            );
        }
        // And a DB instance can no longer be placed in it: the group the create persisted
        // is really gone from the store every other operation reads.
        expect_error(
            row.case,
            call(
                &router,
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", "emu-del-subnets-probe"),
                    ("Engine", "mysql"),
                    ("DBInstanceClass", "db.m5.large"),
                    ("DBSubnetGroupName", row.name),
                ],
            )
            .await,
            404,
            "DBSubnetGroupNotFoundFault",
        );
    }
}

#[tokio::test]
async fn a_group_a_db_instance_is_using_is_refused_with_the_state_fault_then_released() {
    let router = emulator();

    expect_ok(
        "the subnet group",
        create_group(
            &router,
            REGION,
            &DeletionCase {
                case: "the group in use",
                name: "emu-del-inuse-subnets",
                description: "in use by an instance",
                subnets: &["subnet-0123456789abcdef0", "subnet-0fedcba987654321f"],
                tag: None,
            },
        )
        .await,
    );
    expect_ok(
        "the DB instance using it",
        call(
            &router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-del-inuse-subnet-instance"),
                ("Engine", "mysql"),
                ("DBInstanceClass", "db.m5.large"),
                ("DBSubnetGroupName", "emu-del-inuse-subnets"),
            ],
        )
        .await,
    );
    // A group in use is refused with the STATE fault, not the not-found: the group exists,
    // and the caller has to release it first.
    let refused = expect_error(
        "the group while an instance uses it",
        call(
            &router,
            "DeleteDBSubnetGroup",
            &[("DBSubnetGroupName", "emu-del-inuse-subnets")],
        )
        .await,
        400,
        "InvalidDBSubnetGroupStateFault",
    );
    assert!(
        refused.contains("emu-del-inuse-subnet-instance"),
        "expected the refusal to name the instance using the group, got {refused}"
    );
    // The refused delete left the group exactly where it was.
    let described = expect_ok(
        "the group after the refusal",
        call(
            &router,
            "DescribeDBSubnetGroups",
            &[("DBSubnetGroupName", "emu-del-inuse-subnets")],
        )
        .await,
    );
    assert_eq!(
        text(&described, "DBSubnetGroupName"),
        "emu-del-inuse-subnets",
        "expected the refused delete to change nothing, got {described}"
    );

    // Once the instance is gone the same delete is accepted — the teardown order a caller
    // that created both has to follow. The delete of the instance is ACCEPTED at once and
    // the instance leaves the store when the removal the clock owes falls due, so the
    // group is released then rather than before.
    expect_ok(
        "the instance delete",
        call(
            &router,
            "DeleteDBInstance",
            &[
                ("DBInstanceIdentifier", "emu-del-inuse-subnet-instance"),
                ("SkipFinalSnapshot", "true"),
            ],
        )
        .await,
    );
    sleep(STATUS_TRANSITION_DELAY + Duration::from_millis(200));
    expect_ok(
        "the group once nothing uses it",
        call(
            &router,
            "DeleteDBSubnetGroup",
            &[("DBSubnetGroupName", "emu-del-inuse-subnets")],
        )
        .await,
    );
}

#[tokio::test]
async fn a_delete_reaches_only_the_region_it_was_signed_for() {
    let router = emulator();

    // One name, a group of that name in each of two regions: the delete signed for one
    // region must not reach the other's.
    let row = DeletionCase {
        case: "the regional group",
        name: "emu-del-regional-subnets",
        description: "one name in two regions",
        subnets: &["subnet-0123456789abcdef0", "subnet-0fedcba987654321f"],
        tag: None,
    };
    for region in [REGION, OTHER_REGION] {
        expect_ok("the regional group", create_group(&router, region, &row).await);
    }
    expect_ok(
        "the delete in the signed region",
        call_in(
            &router,
            REGION,
            "DeleteDBSubnetGroup",
            &[("DBSubnetGroupName", row.name)],
        )
        .await,
    );
    expect_error(
        "the deleted group in the signed region",
        call(
            &router,
            "DescribeDBSubnetGroups",
            &[("DBSubnetGroupName", row.name)],
        )
        .await,
        404,
        "DBSubnetGroupNotFoundFault",
    );
    let other = expect_ok(
        "the other region's group of the same name",
        call_in(
            &router,
            OTHER_REGION,
            "DescribeDBSubnetGroups",
            &[("DBSubnetGroupName", row.name)],
        )
        .await,
    );
    assert_eq!(
        text(&other, "DBSubnetGroupName"),
        row.name.to_ascii_lowercase(),
        "expected the other region's group to be untouched, got {other}"
    );
}
