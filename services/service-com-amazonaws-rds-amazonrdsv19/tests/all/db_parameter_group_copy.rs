// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a COPIED DB parameter group is afterwards: the calls that
//! address a group must reach the one a copy created exactly as they reach one a create
//! made, and the modelled not-found fault must be answered only once a delete has removed
//! it.
//!
//! `parameter_group_and_snapshot_copies.rs` covers the copy itself — the forms the source
//! is named by, the family and parameter values it carries, and every refusal class the
//! operation models. This suite covers the other half: the copy's own TAG list, read back
//! through the ARN the copy published, and its DELETE, which is where a group that was
//! reported as created but never registered shows up.
//!
//! Every call goes over the wire: real awsQuery form POSTs through a `MultiServiceRouter`
//! carrying the SigV4 credential scope the router keys on, and every assertion reads the
//! XML the emulator answered with.
//!
//! The table spans what the rule generalises over: three engine families (two this front
//! door publishes a parameter catalog for and one it publishes none for), zero, one and two
//! parameters set on the source, both forms the source may be named by, and tag lists of
//! three lengths.

use service_com_amazonaws_rds_amazonrdsv19::state::PARAMETER_SOURCE_USER;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, emulator, expect_error, expect_ok, subtrees, text, REGION};
use crate::db_parameters::{create_group, modify_in};

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// The description every copy below asks for, so an answer echoing the SOURCE's description
/// instead of the request's one fails.
const COPY_DESCRIPTION: &str = "the-copied-db-parameter-group";

/// The `awsQueryError` code of `DBParameterGroupNotFoundFault`.
const NOT_FOUND: &str = "DBParameterGroupNotFound";

/// The query parameters of one call, owned so a table row can hold them.
type Params = Vec<(String, String)>;

/// One parameter a row sets on its source before copying it: `(name, value)`.
type Setting = (&'static str, &'static str);

/// One row of the copy round trip.
struct CopyCase {
    case: &'static str,
    source: &'static str,
    family: &'static str,
    /// The parameters set on the source before the copy, which the copy must carry.
    settings: &'static [Setting],
    /// Whether the copy names its source by the group's ARN rather than by its name — both
    /// forms `SourceDBParameterGroupIdentifier` accepts.
    source_by_arn: bool,
    target: &'static str,
    /// What the target is stored and reported as: "This value is stored as a lowercase
    /// string".
    stored_target: &'static str,
    tags: &'static [(&'static str, &'static str)],
}

/// The copies this suite drives, then reads, tags and deletes.
const COPY_CASES: &[CopyCase] = &[
    CopyCase {
        case: "a mysql group carrying two parameters, source by name, no tags",
        source: "emu-cpg-mysql",
        family: "mysql8.0",
        settings: &[("wait_timeout", "600"), ("autocommit", "0")],
        source_by_arn: false,
        target: "emu-cpg-mysql-copy",
        stored_target: "emu-cpg-mysql-copy",
        tags: &[],
    },
    CopyCase {
        case: "a postgres group carrying one parameter, source by its ARN, one tag",
        source: "emu-cpg-postgres",
        family: "postgres16",
        settings: &[("autovacuum", "0")],
        source_by_arn: true,
        target: "Emu-CPG-Postgres-Copy",
        stored_target: "emu-cpg-postgres-copy",
        tags: &[("tier", "staging")],
    },
    CopyCase {
        case: "a family this front door publishes no parameter catalog for, three tags",
        source: "emu-cpg-db2",
        family: "db2-se",
        settings: &[],
        source_by_arn: false,
        // The shortest name the published rule admits: one letter.
        target: "e",
        stored_target: "e",
        tags: &[("team", "db"), ("env", "test"), ("owner", "fixtures")],
    },
];

/// The `pg` ARN this service reports for a DB parameter group.
fn group_arn(name: &str) -> String {
    format!("arn:aws:rds:{REGION}:{ACCOUNT}:pg:{name}")
}

/// The awsQuery parameters of one `CopyDBParameterGroup` call.
fn copy_params(source: &str, target: &str, tags: &[(&str, &str)]) -> Params {
    let mut params: Params = vec![
        (
            "SourceDBParameterGroupIdentifier".to_string(),
            source.to_string(),
        ),
        (
            "TargetDBParameterGroupIdentifier".to_string(),
            target.to_string(),
        ),
        (
            "TargetDBParameterGroupDescription".to_string(),
            COPY_DESCRIPTION.to_string(),
        ),
    ];
    params.extend(tags.iter().enumerate().flat_map(|(index, (key, value))| {
        [
            (format!("Tags.Tag.{}.Key", index + 1), (*key).to_string()),
            (format!("Tags.Tag.{}.Value", index + 1), (*value).to_string()),
        ]
    }));
    params
}

/// The `(name, value)` pairs a group reports as its OWN parameters — the ones a caller set
/// on it, which `Source` `user` selects.
async fn user_parameters(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
) -> Vec<(String, String)> {
    let body = expect_ok(
        case,
        call(
            router,
            "DescribeDBParameters",
            &[
                ("DBParameterGroupName", group),
                ("Source", PARAMETER_SOURCE_USER),
                ("MaxRecords", "100"),
            ],
        )
        .await,
    );
    subtrees(&body, "Parameter")
        .iter()
        .map(|parameter| {
            (
                text(parameter, "ParameterName"),
                text(parameter, "ParameterValue"),
            )
        })
        .collect()
}

/// Whether `DescribeDBParameterGroups` reports a group under `name`.
async fn is_reported(router: &MultiServiceRouter, name: &str) -> bool {
    let (status, body) = call(
        router,
        "DescribeDBParameterGroups",
        &[("DBParameterGroupName", name)],
    )
    .await;
    if status == 404 {
        return false;
    }
    assert_eq!(
        status, 200,
        "reading {name} must be a 200 or the modelled 404, got {status} with body {body}"
    );
    text(&body, "DBParameterGroupName") == name
}

#[tokio::test]
async fn a_copied_group_is_tagged_read_and_deleted_like_any_other_group() {
    let router = emulator();

    for row in COPY_CASES {
        let case = row.case;
        let stored_source = create_group(&router, case, row.source, row.family).await;
        let settings: Vec<(&str, &str, Option<&str>)> = row
            .settings
            .iter()
            .map(|(name, value)| (*name, *value, None))
            .collect();
        if !settings.is_empty() {
            expect_ok(
                case,
                modify_in(&router, REGION, &stored_source, &settings).await,
            );
        }
        let named_source = if row.source_by_arn {
            group_arn(&stored_source)
        } else {
            row.source.to_string()
        };

        // The copy answers with the group it created: the request's name and description,
        // the SOURCE's family, and the ARN built from the name it is stored under.
        let copied = expect_ok(
            case,
            call(
                &router,
                "CopyDBParameterGroup",
                &copy_params(&named_source, row.target, row.tags),
            )
            .await,
        );
        for (member, expected) in [
            ("DBParameterGroupName", row.stored_target.to_string()),
            ("DBParameterGroupFamily", row.family.to_string()),
            ("Description", COPY_DESCRIPTION.to_string()),
            ("DBParameterGroupArn", group_arn(row.stored_target)),
        ] {
            let actual = text(&copied, member);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {member} {expected:?}, got {actual:?}"
            );
        }

        // The tags the copy request carried are the COPY's own, read back through the tag
        // report by the ARN the copy published — the resolution a copied group needs just as
        // a created one does.
        let listed = expect_ok(
            case,
            call(
                &router,
                "ListTagsForResource",
                &[("ResourceName", group_arn(row.stored_target))],
            )
            .await,
        );
        let tags: Vec<(String, String)> = subtrees(&listed, "Tag")
            .iter()
            .map(|tag| (text(tag, "Key"), text(tag, "Value")))
            .collect();
        let asked: Vec<(String, String)> = row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            tags, asked,
            "case {case:?}: expected the copy to carry the request's tags {asked:?}, got \
             {tags:?}"
        );

        // The copy carries the source's parameter values, and the DELETE of it is accepted
        // whether it carries any or none.
        let mut expected: Vec<(String, String)> = row
            .settings
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect();
        expected.sort();
        let carried = user_parameters(&router, case, row.stored_target).await;
        assert_eq!(
            carried, expected,
            "case {case:?}: expected the copy to carry the source's parameters {expected:?}, \
             got {carried:?}"
        );
        let deleted = expect_ok(
            case,
            call(
                &router,
                "DeleteDBParameterGroup",
                // Named in the case the request was given, which the store keys lowercased.
                &[("DBParameterGroupName", row.target)],
            )
            .await,
        );
        assert!(
            deleted.contains("<DeleteDBParameterGroupResponse"),
            "case {case:?}: expected the delete's own empty-output envelope, got {deleted}"
        );

        // Only NOW is the copy not found — by the group report, by the parameter report, by
        // the tag read on its ARN and by a second delete alike — and the source it came from
        // is still there.
        for action in [
            "DescribeDBParameterGroups",
            "DescribeDBParameters",
            "DeleteDBParameterGroup",
        ] {
            expect_error(
                case,
                call(
                    &router,
                    action,
                    &[("DBParameterGroupName", row.stored_target)],
                )
                .await,
                404,
                NOT_FOUND,
            );
        }
        expect_error(
            case,
            call(
                &router,
                "ListTagsForResource",
                &[("ResourceName", group_arn(row.stored_target))],
            )
            .await,
            404,
            NOT_FOUND,
        );
        assert!(
            is_reported(&router, &stored_source).await,
            "case {case:?}: deleting the copy must leave the source in place"
        );
    }
}
