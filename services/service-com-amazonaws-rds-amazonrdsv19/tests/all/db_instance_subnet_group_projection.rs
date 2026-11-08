// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `DBSubnetGroup` a DB INSTANCE reports.
//!
//! RULE: where a response embeds another resource's modelled shape, that shape is projected
//! from the other resource's own record — so `DBInstance$DBSubnetGroup` carries everything
//! the named DB subnet group holds (its description, its VPC, its status and its `Subnets`,
//! each with the Availability Zone the service that OWNS subnets reports it in), and reads
//! the same way as `DescribeDBSubnetGroups` reports that group, including after the group
//! itself is modified.
//!
//! A client inspecting placement reads the instance's own envelope — `['DBSubnetGroup']
//! ['Subnets']` — so a projection carrying only the group's name says nothing about which
//! subnets and zones the instance can be placed in.
//!
//! The network is built by Amazon EC2, mounted beside Amazon RDS on ONE engine
//! (`db_subnet_group_ec2_zones::Network`, shared rather than copied), so every subnet id in
//! a request is one EC2 minted and every expected zone is one EC2 reported.
//!
//! The rows span what the rule generalises over: how many subnets the group holds, how many
//! zones they cover, the order they are named in, which operation reports the instance
//! (the create, the read back and a reboot), and a group whose subnet list CHANGED after the
//! instance was created.

use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call_in, expect_ok, optional_text, subtrees, text, wait_for_settle, REGION,
};
use crate::db_subnet_group_ec2_zones::{assert_reported_zones, Network};

/// The engine every instance below runs, and a class that engine is orderable in: the
/// subject here is the group projection, which no engine of the catalogue changes.
const ENGINE: &str = "postgres";
const CLASS: &str = "db.t3.medium";

/// `CreateDBInstance` in the group `group`.
async fn create_instance(
    router: &MultiServiceRouter,
    identifier: &str,
    group: &str,
) -> (u16, String) {
    call_in(
        router,
        REGION,
        "CreateDBInstance",
        &[
            ("DBInstanceIdentifier".to_string(), identifier.to_string()),
            ("Engine".to_string(), ENGINE.to_string()),
            ("DBInstanceClass".to_string(), CLASS.to_string()),
            ("AllocatedStorage".to_string(), "20".to_string()),
            ("DBSubnetGroupName".to_string(), group.to_string()),
        ],
    )
    .await
}

/// One instance-shaped call that carries no member beyond the identifier.
async fn instance_call(
    router: &MultiServiceRouter,
    action: &str,
    identifier: &str,
) -> (u16, String) {
    call_in(
        router,
        REGION,
        action,
        &[("DBInstanceIdentifier".to_string(), identifier.to_string())],
    )
    .await
}

/// The one `<DBSubnetGroup>` subtree the `<DBInstance>` in `xml` carries.
fn subnet_group_of(case: &str, label: &str, xml: &str) -> String {
    let instances = subtrees(xml, "DBInstance");
    assert_eq!(
        instances.len(),
        1,
        "case {case:?} ({label}): expected exactly one DBInstance, got {} in {xml}",
        instances.len()
    );
    let instance = instances
        .into_iter()
        .next()
        .expect("checked just above");
    let groups = subtrees(&instance, "DBSubnetGroup");
    assert_eq!(
        groups.len(),
        1,
        "case {case:?} ({label}): an instance created in a DB subnet group must report that \
         group, got {} in {instance}",
        groups.len()
    );
    groups.into_iter().next().expect("checked just above")
}

/// RULE, over a table of groups: an instance reports the whole record of the DB subnet group
/// it was created in, and reports it identically through every operation that answers with a
/// `DBInstance`.
#[tokio::test]
async fn an_instance_reports_the_record_of_the_subnet_group_it_was_created_in() {
    let network = Network::build().await;

    // Rows differ along the subnet count, the zones covered and the order the group names
    // them in — each of which the instance's own projection must follow.
    let rows: [(&str, &str, &str, Vec<String>); 3] = [
        (
            "two subnets in two zones",
            "emu-instgrp-two",
            "emu-instgrp-inst-two",
            vec![
                network.first_zone[0].clone(),
                network.second_zone.clone(),
            ],
        ),
        (
            "the same two subnets named in the other order",
            "emu-instgrp-reversed",
            "emu-instgrp-inst-reversed",
            vec![
                network.second_zone.clone(),
                network.first_zone[0].clone(),
            ],
        ),
        (
            "three subnets, two of them in one zone",
            "emu-instgrp-three",
            "emu-instgrp-inst-three",
            vec![
                network.first_zone[0].clone(),
                network.first_zone[1].clone(),
                network.second_zone.clone(),
            ],
        ),
    ];

    for (case, group, identifier, subnets) in rows {
        let expected_zones = network.zones_of(&subnets).await;
        let created_group = expect_ok(case, network.account.create_group(group, &subnets).await);
        assert_reported_zones(
            case,
            "CreateDBSubnetGroup",
            &created_group,
            &subnets,
            &expected_zones,
        );

        let created = expect_ok(
            case,
            create_instance(&network.account.router, identifier, group).await,
        );
        // Every operation that answers with a DBInstance projects the group the same way:
        // the create, the read back, and a call that moves the instance's status. The
        // reboot waits out the span the create owes, because an instance still coming up
        // cannot be rebooted at all.
        let mut envelopes = vec![("CreateDBInstance".to_string(), created)];
        envelopes.push((
            "DescribeDBInstances".to_string(),
            expect_ok(
                case,
                instance_call(&network.account.router, "DescribeDBInstances", identifier).await,
            ),
        ));
        wait_for_settle();
        envelopes.push((
            "RebootDBInstance".to_string(),
            expect_ok(
                case,
                instance_call(&network.account.router, "RebootDBInstance", identifier).await,
            ),
        ));

        for (label, xml) in &envelopes {
            let reported = subnet_group_of(case, label, xml);
            let name = text(&reported, "DBSubnetGroupName");
            assert_eq!(
                name, group,
                "case {case:?} ({label}): expected DBSubnetGroupName {group:?}, got {name:?}"
            );
            // The members that belong to the GROUP's record, which a projection carrying
            // only the name reports nothing of.
            let description = optional_text(&reported, "DBSubnetGroupDescription");
            assert_eq!(
                description.as_deref(),
                optional_text(&created_group, "DBSubnetGroupDescription").as_deref(),
                "case {case:?} ({label}): expected the group's own description, got \
                 {description:?}"
            );
            let vpc = optional_text(&reported, "VpcId");
            assert_eq!(
                vpc.as_deref(),
                optional_text(&created_group, "VpcId").as_deref(),
                "case {case:?} ({label}): expected the group's own VpcId, got {vpc:?}"
            );
            let status = text(&reported, "SubnetGroupStatus");
            assert_eq!(
                status,
                text(&created_group, "SubnetGroupStatus"),
                "case {case:?} ({label}): expected the group's own SubnetGroupStatus, got \
                 {status:?}"
            );
            assert_reported_zones(case, label, &reported, &subnets, &expected_zones);
        }
    }
}

/// RULE, continued: the embedded group is read from the GROUP's record, so a group whose
/// subnet list is changed after the instance was created is reported as it is NOW.
#[tokio::test]
async fn an_instance_reports_its_subnet_group_as_the_group_reads_now() {
    let network = Network::build().await;
    let group = "emu-instgrp-moving";
    let identifier = "emu-instgrp-inst-moving";

    let created_with = vec![
        network.first_zone[0].clone(),
        network.second_zone.clone(),
    ];
    expect_ok(
        "the group the instance is created in",
        network.account.create_group(group, &created_with).await,
    );
    expect_ok(
        "the instance in that group",
        create_instance(&network.account.router, identifier, group).await,
    );

    // Rows: the list the group was created with, then two lists a modification leaves it
    // holding — a different pair, and a longer list.
    let rows: [(&str, Vec<String>); 3] = [
        ("the list the group was created with", created_with.clone()),
        (
            "a modification to another pair of subnets",
            vec![
                network.second_zone.clone(),
                network.first_zone[1].clone(),
            ],
        ),
        (
            "a modification to three subnets",
            vec![
                network.first_zone[0].clone(),
                network.first_zone[1].clone(),
                network.second_zone.clone(),
            ],
        ),
    ];

    for (case, subnets) in rows {
        if subnets != created_with {
            expect_ok(case, network.account.modify_group(group, &subnets).await);
        }
        let expected_zones = network.zones_of(&subnets).await;
        // The group's own report and the instance's embedded copy are one answer.
        let group_report = expect_ok(case, network.account.describe_group(group).await);
        assert_reported_zones(
            case,
            "DescribeDBSubnetGroups",
            &group_report,
            &subnets,
            &expected_zones,
        );
        let instance = expect_ok(
            case,
            instance_call(&network.account.router, "DescribeDBInstances", identifier).await,
        );
        let reported = subnet_group_of(case, "DescribeDBInstances", &instance);
        assert_reported_zones(
            case,
            "DescribeDBInstances",
            &reported,
            &subnets,
            &expected_zones,
        );
    }
}
