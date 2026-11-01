// SPDX-License-Identifier: CC0-1.0

//! A probe of the Amazon Neptune control plane's parameter catalogs, which this one `rds`
//! front door serves (`src/service.rs`): an AWS SDK Neptune client signs `rds`, so its
//! `DescribeDBParameters` and `DescribeDBClusterParameters` are answered here.
//!
//! The rule under test: a parameter's published metadata belongs to the PARAMETER, so the
//! engine's two catalogs — the one a DB parameter group of a family reports and the one a DB
//! CLUSTER parameter group of the same family reports — report one apply type and one
//! description for a parameter both of them carry. `ApplyType` is what tells a client
//! whether the change it is about to make needs a reboot, so two catalogs disagreeing about
//! it would answer that question differently depending on which group was asked.
//!
//! Every call goes over the wire through the harness in `db_instances.rs`.

use crate::db_instances::{call, emulator, expect_ok, subtrees, text};

#[tokio::test]
async fn the_two_catalogs_of_one_engine_agree_about_a_parameter_both_carry() {
    let router = emulator();

    // The parameter an Amazon Neptune DB parameter group carries, which its DB CLUSTER
    // parameter groups carry too.
    let parameter = "neptune_query_timeout";
    let family = "neptune1.4";

    expect_ok(
        "a Neptune DB parameter group",
        call(
            &router,
            "CreateDBParameterGroup",
            &[
                ("DBParameterGroupName", "emu-neptune-pg"),
                ("DBParameterGroupFamily", family),
                ("Description", "the engine's own metadata"),
            ],
        )
        .await,
    );
    expect_ok(
        "a Neptune DB cluster parameter group",
        call(
            &router,
            "CreateDBClusterParameterGroup",
            &[
                ("DBClusterParameterGroupName", "emu-neptune-cpg"),
                ("DBParameterGroupFamily", family),
                ("Description", "the engine's own metadata"),
            ],
        )
        .await,
    );

    // (case, the describe, the member naming the group, the group)
    let catalogs: [(&str, &str, &str, &str); 2] = [
        (
            "the DB parameter group's catalog",
            "DescribeDBParameters",
            "DBParameterGroupName",
            "emu-neptune-pg",
        ),
        (
            "the DB cluster parameter group's catalog",
            "DescribeDBClusterParameters",
            "DBClusterParameterGroupName",
            "emu-neptune-cpg",
        ),
    ];

    let mut reported: Vec<(&str, String, String)> = Vec::new();
    for (case, action, member, group) in catalogs {
        let body = expect_ok(case, call(&router, action, &[(member, group)]).await);
        let parameters = subtrees(&body, "Parameter");
        let found = parameters
            .iter()
            .find(|candidate| text(candidate, "ParameterName") == parameter)
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected {family} to report {parameter}, got {body}")
            });
        reported.push((
            case,
            text(found, "ApplyType"),
            text(found, "Description"),
        ));
    }

    // A change to the graph query timeout takes effect only once the instance is rebooted,
    // which is what `static` tells a caller.
    for (case, apply_type, description) in &reported {
        assert_eq!(
            apply_type, "static",
            "case {case:?}: expected {parameter} to report ApplyType \"static\", got \
             {apply_type:?}"
        );
        assert!(
            !description.is_empty(),
            "case {case:?}: expected {parameter} to carry the engine's own description, got \
             {description:?}"
        );
    }
    assert_eq!(
        reported[0].1, reported[1].1,
        "the two catalogs must report one apply type for {parameter}, got {reported:?}"
    );
    assert_eq!(
        reported[0].2, reported[1].2,
        "the two catalogs must report one description for {parameter}, got {reported:?}"
    );
}
