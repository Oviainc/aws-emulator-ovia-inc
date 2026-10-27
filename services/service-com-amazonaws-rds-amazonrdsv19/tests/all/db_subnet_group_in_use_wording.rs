// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how this service WORDS the refusal of `DeleteDBSubnetGroup` for a
//! group a database still uses.
//!
//! The rule under test: a DB subnet group a database is placed in cannot be deleted
//! ("Deletes a DB subnet group. The specified database subnet group must not be associated
//! with any DB instances", `smithy_query(query="operation",
//! operation="DeleteDBSubnetGroup")`), and the one modelled error for it —
//! `InvalidDBSubnetGroupStateFault`, HTTP 400 with the `awsQueryError` code
//! `InvalidDBSubnetGroupStateFault` — carries ONE sentence built from the request and from
//! the blocking resource: the fixed text "Cannot delete the subnet group '" … "' because at
//! least one database instance: " … " is still using it.", whose variable parts are the
//! group name the request named (quoted, in the folded spelling it is stored under, since
//! "This value is stored as a lowercase string") and the identifier of the database still
//! using it. Nothing of either part comes from one particular group, instance or run.
//!
//! The table spans the classes the rule generalises over: the shape of the group name (plain,
//! mixed case folded on the way in, the periods, underscores and spaces the member admits, a
//! single letter), the shape and length of the holder's identifier, the KIND of database that
//! holds the group (a DB instance and a DB cluster), and how many databases hold it (one, and
//! several at once). The last of those is also where the message's variable part is pinned to
//! the store rather than to the request: the identifier it names is one the store really
//! holds.
//!
//! Every call goes over the wire through the harness in `db_instances.rs`, so the assertions
//! read the XML the emulator itself answered with.

use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, emulator, expect_error, expect_ok, text, wait_for_settle};

/// Two subnets in the 17-character id form: the smallest list a DB subnet group can be made
/// of, which every group below is created over.
const TWO_SUBNETS: [&str; 2] = ["subnet-0123456789abcdef0", "subnet-0fedcba987654321f"];

/// What kind of database holds the group — the two records of this service that can name a
/// DB subnet group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Holder {
    /// A DB instance created with `DBSubnetGroupName` set to the group.
    DbInstance,
    /// A DB cluster created with `DBSubnetGroupName` set to the group.
    DbCluster,
}

/// The message `DeleteDBSubnetGroup` answers for a group a database still uses: the fixed
/// sentence, with the group name as the request named it (folded) and the identifier of the
/// blocking database.
fn in_use_wording(folded_name: &str, holder_identifier: &str) -> String {
    format!(
        "Cannot delete the subnet group '{folded_name}' because at least one database \
         instance: {holder_identifier} is still using it."
    )
}

/// `CreateDBSubnetGroup` over [`TWO_SUBNETS`].
async fn create_group(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(
        router,
        "CreateDBSubnetGroup",
        &[
            ("DBSubnetGroupName", name),
            ("DBSubnetGroupDescription", "in-use refusal wording"),
            ("SubnetIds.SubnetIdentifier.1", TWO_SUBNETS[0]),
            ("SubnetIds.SubnetIdentifier.2", TWO_SUBNETS[1]),
        ],
    )
    .await
}

/// Place a database of `holder`'s kind in `group`, under `identifier`.
async fn create_holder(
    router: &MultiServiceRouter,
    holder: Holder,
    identifier: &str,
    group: &str,
) -> (u16, String) {
    match holder {
        Holder::DbInstance => {
            call(
                router,
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", identifier),
                    ("Engine", "mysql"),
                    ("DBInstanceClass", "db.m5.large"),
                    ("DBSubnetGroupName", group),
                ],
            )
            .await
        }
        Holder::DbCluster => {
            call(
                router,
                "CreateDBCluster",
                &[
                    ("DBClusterIdentifier", identifier),
                    ("Engine", "aurora-mysql"),
                    ("DBSubnetGroupName", group),
                ],
            )
            .await
        }
    }
}

/// One group, the database that holds it, and the wording the delete must answer.
struct InUseCase {
    case: &'static str,
    /// The group name the create and the delete carry, in the spelling the request uses.
    group: &'static str,
    holder: Holder,
    /// The identifier of the database placed in the group.
    identifier: &'static str,
}

/// The rows the refusal is asserted over.
const IN_USE_CASES: &[InUseCase] = &[
    InUseCase {
        case: "a DB instance holds the group",
        group: "emu-inuse-subnets",
        holder: Holder::DbInstance,
        identifier: "emu-inuse-instance",
    },
    InUseCase {
        case: "a DB cluster holds the group",
        group: "emu-inuse-cluster-subnets",
        holder: Holder::DbCluster,
        identifier: "emu-inuse-cluster",
    },
    InUseCase {
        case: "a group named in mixed case, folded on the way in",
        group: "EMU-InUse-Mixed",
        holder: Holder::DbInstance,
        identifier: "emu-inuse-mixed-instance",
    },
    InUseCase {
        case: "the periods, underscores and spaces the group name admits",
        group: "emu.inuse_subnets 4",
        holder: Holder::DbInstance,
        identifier: "a",
    },
    InUseCase {
        case: "a single-letter group name and a long holder identifier",
        group: "e",
        holder: Holder::DbCluster,
        identifier: "emu-inuse-cluster-with-a-longer-identifier-0123456789",
    },
];

#[tokio::test]
async fn a_group_a_database_still_uses_is_refused_with_the_documented_sentence() {
    let router = emulator();

    for row in IN_USE_CASES {
        expect_ok(row.case, create_group(&router, row.group).await);
        expect_ok(
            row.case,
            create_holder(&router, row.holder, row.identifier, row.group).await,
        );

        let refused = expect_error(
            row.case,
            call(
                &router,
                "DeleteDBSubnetGroup",
                &[("DBSubnetGroupName", row.group)],
            )
            .await,
            400,
            "InvalidDBSubnetGroupStateFault",
        );
        let message = text(&refused, "Message");
        let expected = in_use_wording(&row.group.to_ascii_lowercase(), row.identifier);
        assert_eq!(
            message, expected,
            "case {:?}: expected the in-use sentence, got {message:?}",
            row.case
        );

        // The refused delete changed nothing: the group is still there for the report, which
        // is what a caller retrying after it released the database reads.
        let described = expect_ok(
            row.case,
            call(
                &router,
                "DescribeDBSubnetGroups",
                &[("DBSubnetGroupName", row.group)],
            )
            .await,
        );
        assert_eq!(
            text(&described, "DBSubnetGroupName"),
            row.group.to_ascii_lowercase(),
            "case {:?}: expected the refused delete to leave the group alone, got {described}",
            row.case
        );
    }
}

#[tokio::test]
async fn the_sentence_names_a_database_the_store_really_holds_when_several_hold_the_group() {
    let router = emulator();

    let group = "emu-inuse-several-subnets";
    expect_ok("the group several databases hold", create_group(&router, group).await);
    let identifiers = [
        "emu-inuse-several-a",
        "emu-inuse-several-b",
        "emu-inuse-several-c",
    ];
    for identifier in identifiers {
        expect_ok(
            identifier,
            create_holder(&router, Holder::DbInstance, identifier, group).await,
        );
    }

    let refused = expect_error(
        "several databases hold the group",
        call(
            &router,
            "DeleteDBSubnetGroup",
            &[("DBSubnetGroupName", group)],
        )
        .await,
        400,
        "InvalidDBSubnetGroupStateFault",
    );
    let message = text(&refused, "Message");
    let admissible: Vec<String> = identifiers
        .iter()
        .map(|identifier| in_use_wording(group, identifier))
        .collect();
    assert!(
        admissible.contains(&message),
        "case \"several databases hold the group\": expected the sentence to name one of the \
         databases the store holds ({admissible:?}), got {message:?}"
    );
}

#[tokio::test]
async fn the_refusal_is_answered_only_while_a_database_holds_the_group() {
    let router = emulator();

    let group = "emu-inuse-released-subnets";
    let identifier = "emu-inuse-released-instance";
    expect_ok("the group", create_group(&router, group).await);
    expect_ok(
        "the instance holding it",
        create_holder(&router, Holder::DbInstance, identifier, group).await,
    );

    let refused = expect_error(
        "while the instance holds it",
        call(
            &router,
            "DeleteDBSubnetGroup",
            &[("DBSubnetGroupName", group)],
        )
        .await,
        400,
        "InvalidDBSubnetGroupStateFault",
    );
    assert_eq!(
        text(&refused, "Message"),
        in_use_wording(group, identifier),
        "expected the in-use sentence, got {refused}"
    );

    // Released: the instance leaves the store when the removal the clock owes falls due, and
    // only then is the same delete accepted — the state fault is about the store, not about
    // the name the request carries.
    expect_ok(
        "the instance delete",
        call(
            &router,
            "DeleteDBInstance",
            &[
                ("DBInstanceIdentifier", identifier),
                ("SkipFinalSnapshot", "true"),
            ],
        )
        .await,
    );
    wait_for_settle();
    expect_ok(
        "the delete once nothing holds the group",
        call(
            &router,
            "DeleteDBSubnetGroup",
            &[("DBSubnetGroupName", group)],
        )
        .await,
    );
    // And a group nothing holds any more is NOT the state fault afterwards: it is the
    // not-found one, which is what the sibling wording suite pins.
    expect_error(
        "the deleted group",
        call(
            &router,
            "DescribeDBSubnetGroups",
            &[("DBSubnetGroupName", group)],
        )
        .await,
        404,
        "DBSubnetGroupNotFoundFault",
    );
}
