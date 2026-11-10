// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how `CreateDBCluster` WORDS the refusals of the network and the
//! identifier it is given, over the one Query front door that serves Amazon RDS and the two
//! sibling control planes.
//!
//! Three rules, each asserted over a TABLE spanning what it generalises over rather than
//! over one request:
//!
//! * a `DBSubnetGroupName` that resolves to no group is the modelled 404
//!   `DBSubnetGroupNotFoundFault`, worded as the sentence the calls that ACT on a group
//!   answer — "DB subnet group '<name>' does not exist." — whichever engine's cluster the
//!   create was placing in it;
//! * an identifier outside the identifier rule is refused in the wording its MEMBER carries
//!   — the subject wording this front door answers for a DB cluster's identifier, whichever
//!   clause the value broke, and for a DB instance's where the value is no identifier TOKEN
//!   of that member at all — whichever control plane's engine the request's `Engine` names,
//!   because one front door answers them all;
//! * a Multi-AZ DB cluster whose named DB subnet group covers fewer than three Availability
//!   Zones is the modelled 400 `InvalidVPCNetworkStateFault`, and its message names the
//!   zones of the Region that group does not cover yet — read off the group's stored
//!   subnets, so the refusal is computed from state and not from the request.
//!
//! Every call goes over the wire through the harness in `db_instances.rs`, so the assertions
//! read the XML the emulator itself answered with.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_rds_amazonrdsv19::state;

use crate::db_instances::{call, call_in, emulator, expect_error, expect_ok, text, REGION};

/// A second region, so a group that exists but is out of the signed region's scope is a row
/// of the not-found table.
const OTHER_REGION: &str = "ap-south-1";

/// The settings `CreateDBCluster` documents as required for a Multi-AZ DB cluster ("This
/// setting is required to create a Multi-AZ DB cluster"), with an `Iops` inside the ratio
/// `Iops` documents against `AllocatedStorage`.
const MULTI_AZ_SETTINGS: &[(&str, &str)] = &[
    ("AllocatedStorage", "100"),
    ("DBClusterInstanceClass", "db.m6gd.xlarge"),
    ("StorageType", "io1"),
    ("Iops", "1000"),
];

/// `CreateDBSubnetGroup` in `region` over `subnets`.
async fn create_group(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    subnets: &[String],
) -> (u16, String) {
    let mut params: Vec<(String, String)> = vec![
        ("DBSubnetGroupName".to_string(), name.to_string()),
        (
            "DBSubnetGroupDescription".to_string(),
            "a group under test".to_string(),
        ),
    ];
    for (index, subnet) in subnets.iter().enumerate() {
        params.push((
            format!("SubnetIds.SubnetIdentifier.{}", index + 1),
            subnet.clone(),
        ));
    }
    call_in(router, region, "CreateDBSubnetGroup", &params).await
}

/// Subnet ids of the 17-character EC2 form, in the order a group would name them.
///
/// How many DISTINCT subnets a group holds is what the Multi-AZ coverage rule rests on — a
/// subnet lies in exactly one Availability Zone — so the table below is over that count.
fn subnets(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("subnet-{index:017x}"))
        .collect()
}

/// The zones of [`REGION`] the group made of `subnets` does not cover, in the order the
/// region publishes them: what the refusal's last clause names, read off the zones the group
/// KEEPS for its subnets.
///
/// Which zone a subnet lies in is the fact of the service that OWNS subnets, and this
/// harness mounts only Amazon RDS: no zone was ever answered for the ids below, so the group
/// covers none of the Region's zones and every one of them is a zone a caller could add a
/// subnet in. The zones a group reports when the owner of subnets IS mounted beside it are
/// the `db_subnet_group_ec2_zones` suite's rows.
fn uncovered_zones(subnets: &[String]) -> Vec<String> {
    assert!(
        !subnets.is_empty(),
        "a group under test names at least one subnet"
    );
    state::region_availability_zones(REGION)
}

#[tokio::test]
async fn a_cluster_create_naming_a_subnet_group_that_does_not_exist_says_so() {
    let router = emulator();

    let stored = subnets(3);
    expect_ok(
        "the stored group",
        create_group(&router, REGION, "emu-netword-stored", &stored).await,
    );
    expect_ok(
        "the other region's group",
        create_group(&router, OTHER_REGION, "emu-netword-elsewhere", &stored).await,
    );

    // Rows spanning the shape of a name that resolves to nothing, crossed with the control
    // planes whose clusters this front door creates: the wording is the call's, not the
    // engine's.
    let names: [(&str, &str); 5] = [
        ("a name nothing was created under", "emu-netword-ghost"),
        ("a near miss of a stored name", "emu-netword-stored-2"),
        ("a single letter", "e"),
        ("a name in mixed case, folded on the way in", "EMU-NetWord-Ghost"),
        ("a group only another region holds", "emu-netword-elsewhere"),
    ];
    for (engine_case, engine) in [
        ("the Amazon DocumentDB control plane's engine", "docdb"),
        ("the Amazon Neptune control plane's engine", "neptune"),
        ("an Aurora engine", "aurora-postgresql"),
        ("a Multi-AZ DB cluster engine", "mysql"),
    ] {
        for (index, (name_case, name)) in names.iter().enumerate() {
            let case = format!("{engine_case}, {name_case}");
            let identifier = format!("emu-netword-{}-{index}", engine.replace('-', ""));
            let body = expect_error(
                &case,
                call(
                    &router,
                    "CreateDBCluster",
                    &[
                        ("DBClusterIdentifier", identifier.as_str()),
                        ("Engine", engine),
                        ("DBSubnetGroupName", name),
                    ],
                )
                .await,
                404,
                "DBSubnetGroupNotFoundFault",
            );
            let message = text(&body, "Message");
            let expected = format!(
                "DB subnet group '{}' does not exist.",
                name.to_ascii_lowercase()
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
            // The refused create stored nothing: the cluster it would have created is not
            // there to be described.
            expect_error(
                &case,
                call(
                    &router,
                    "DescribeDBClusters",
                    &[("DBClusterIdentifier", identifier.as_str())],
                )
                .await,
                404,
                "DBClusterNotFoundFault",
            );
        }
    }
}

/// One identifier refusal: the engine the create names, and the subject its identifier
/// member is worded by — which is the MEMBER's, so every engine shares it.
struct EngineWording {
    case: &'static str,
    engine: &'static str,
    /// The subject Amazon RDS names a cluster identifier by.
    subject: &'static str,
}

/// Both control-plane groups this one front door serves, over every engine each publishes a
/// cluster form for.
const ENGINE_WORDINGS: &[EngineWording] = &[
    EngineWording {
        case: "the Amazon DocumentDB control plane's engine",
        engine: "docdb",
        subject: "database cluster identifier",
    },
    EngineWording {
        case: "the Amazon Neptune control plane's engine",
        engine: "neptune",
        subject: "database cluster identifier",
    },
    EngineWording {
        case: "an Aurora MySQL cluster",
        engine: "aurora-mysql",
        subject: "database cluster identifier",
    },
    EngineWording {
        case: "an Aurora PostgreSQL cluster",
        engine: "aurora-postgresql",
        subject: "database cluster identifier",
    },
    EngineWording {
        case: "a Multi-AZ DB cluster",
        engine: "mysql",
        subject: "database cluster identifier",
    },
];

/// The identifier-rule refusal a DB INSTANCE identifier that is a token of its member's
/// shape and breaks one of the two hyphen rules answers with: it names the request member
/// at fault and states the whole rule, echoing nothing the request carried.
fn states_the_rule(member: &str) -> String {
    format!(
        "The parameter {member} is not a valid identifier. Identifiers must begin with a \
         letter; must contain only ASCII letters, digits, and hyphens; and must not end with \
         a hyphen or contain two consecutive hyphens."
    )
}

#[tokio::test]
async fn an_identifier_refusal_is_worded_by_the_member_it_names() {
    let router = emulator();

    // Values spanning the clauses of the identifier rule: a leading digit, a character
    // outside the alphabet, a trailing hyphen, two consecutive hyphens and one past the
    // length its own member bounds. The DB CLUSTER identifier answers its subject for every
    // one of them.
    let too_long = format!("e{}", "a".repeat(64));
    let values: [(&str, &str); 5] = [
        ("a leading digit", "1emu-wording-cl"),
        ("a character outside the alphabet", "emu_wording_cl"),
        ("a trailing hyphen", "emu-wording-cl-"),
        ("two consecutive hyphens", "emu--wording-cl"),
        ("one character past the length bound", too_long.as_str()),
    ];

    for row in ENGINE_WORDINGS {
        for (value_case, value) in values {
            let case = format!("{}, {value_case}", row.case);
            let body = expect_error(
                &case,
                call(
                    &router,
                    "CreateDBCluster",
                    &[("DBClusterIdentifier", value), ("Engine", row.engine)],
                )
                .await,
                400,
                "InvalidParameterValue",
            );
            let message = text(&body, "Message");
            // The two spaces after the colon are part of that wording.
            let expected = format!("Invalid {}:  {value}", row.subject);
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    // The DB instance identifier, whose subject is the other one this front door words a
    // refusal by — again the member's, whichever engine the create names, and there only
    // where the value is no identifier token of that member at all: a token breaking a
    // hyphen rule states the rule instead, which is how the instance path differs from the
    // cluster path above.
    for (case, engine) in [
        ("a DocumentDB instance", "docdb"),
        ("an Amazon RDS instance", "postgres"),
    ] {
        for (value_case, value, expected) in [
            (
                "a leading digit",
                "1emu-wording-db",
                "Invalid database identifier:  1emu-wording-db".to_string(),
            ),
            (
                "a trailing hyphen",
                "emu-wording-db-",
                states_the_rule("DBInstanceIdentifier"),
            ),
        ] {
            let case = format!("{case}, {value_case}");
            let body = expect_error(
                &case,
                call(
                    &router,
                    "CreateDBInstance",
                    &[
                        ("DBInstanceIdentifier", value),
                        ("Engine", engine),
                        ("DBInstanceClass", "db.m5.large"),
                    ],
                )
                .await,
                400,
                "InvalidParameterValue",
            );
            let message = text(&body, "Message");
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_multi_az_cluster_whose_group_cannot_cover_three_zones_is_refused_with_the_zones_to_add()
{
    let router = emulator();

    // Groups holding two, three and four distinct subnets: a subnet lies in exactly one
    // zone, so the count of subnets a group holds is what settles whether it can have one
    // in the three zones a Multi-AZ DB cluster needs. Two is the fewest a group can be made
    // of, and three is where the rule turns.
    for subnet_count in 2..=4 {
        let in_group = subnets(subnet_count);
        let group = format!("emu-multiaz-group-{subnet_count}");
        expect_ok(
            &group,
            create_group(&router, REGION, &group, &in_group).await,
        );

        let identifier = format!("emu-multiaz-cl-{subnet_count}");
        let mut params: Vec<(&str, &str)> = vec![
            ("DBClusterIdentifier", identifier.as_str()),
            ("Engine", "mysql"),
            ("DBSubnetGroupName", group.as_str()),
        ];
        params.extend(MULTI_AZ_SETTINGS.iter().copied());
        let response = call(&router, "CreateDBCluster", &params).await;

        if subnet_count < 3 {
            let case = format!("a group of {subnet_count} subnets");
            let body = expect_error(&case, response, 400, "InvalidVPCNetworkStateFault");
            let message = text(&body, "Message");
            let expected = format!(
                "Can't create a Multi-AZ DB cluster because there aren't enough subnets \
                 available. At least three subnets with available IP addresses in different \
                 Availability Zones are required. You can add subnets to a DB subnet group \
                 in the following Availability Zones: {}.",
                uncovered_zones(&in_group).join(", ")
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
            // Nothing was created: the cluster the refused create named is not there.
            expect_error(
                &case,
                call(
                    &router,
                    "DescribeDBClusters",
                    &[("DBClusterIdentifier", identifier.as_str())],
                )
                .await,
                404,
                "DBClusterNotFoundFault",
            );
        } else {
            // A group that can have a subnet in three zones satisfies the precondition — WHICH
            // zones its subnets lie in is EC2's fact and this service does not hold it — so
            // the create is served and the cluster is read back from the store.
            let case = format!("a group of {subnet_count} subnets");
            expect_ok(&case, response);
            let reported = expect_ok(
                &case,
                call(
                    &router,
                    "DescribeDBClusters",
                    &[("DBClusterIdentifier", identifier.as_str())],
                )
                .await,
            );
            assert_eq!(
                text(&reported, "DBSubnetGroup"),
                group,
                "case {case:?}: expected the stored cluster to name the group it was placed \
                 in, got {reported}"
            );
        }
    }

    // The precondition is the Multi-AZ DB cluster's: an Aurora-style cluster is placed in a
    // two-subnet group, and a Multi-AZ create that names NO group is not judged against a
    // group at all (the account's default group's subnets are EC2's to answer for, and this
    // service holds none of them).
    let two_subnets = subnets(2);
    expect_ok(
        "the two-subnet group",
        create_group(&router, REGION, "emu-multiaz-two-subnets", &two_subnets).await,
    );
    expect_ok(
        "an Aurora cluster in a two-subnet group",
        call(
            &router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "emu-multiaz-aurora"),
                ("Engine", "aurora-mysql"),
                ("DBSubnetGroupName", "emu-multiaz-two-subnets"),
            ],
        )
        .await,
    );
    let mut params: Vec<(&str, &str)> = vec![
        ("DBClusterIdentifier", "emu-multiaz-nogroup"),
        ("Engine", "mysql"),
    ];
    params.extend(MULTI_AZ_SETTINGS.iter().copied());
    expect_ok(
        "a Multi-AZ cluster naming no group",
        call(&router, "CreateDBCluster", &params).await,
    );

    // And the group is answered BEFORE the cluster's own settings: a Multi-AZ create against
    // a group that cannot cover three zones is the network refusal even when it carries none
    // of the four settings that cluster type requires.
    let refused = expect_error(
        "a Multi-AZ create carrying none of its required settings",
        call(
            &router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "emu-multiaz-bare"),
                ("Engine", "mysql"),
                ("DBSubnetGroupName", "emu-multiaz-two-subnets"),
            ],
        )
        .await,
        400,
        "InvalidVPCNetworkStateFault",
    );
    let message = text(&refused, "Message");
    assert!(
        message.starts_with(
            "Can't create a Multi-AZ DB cluster because there aren't enough subnets available."
        ),
        "case \"a Multi-AZ create carrying none of its required settings\": expected the \
         network refusal, got {message:?}"
    );
}
