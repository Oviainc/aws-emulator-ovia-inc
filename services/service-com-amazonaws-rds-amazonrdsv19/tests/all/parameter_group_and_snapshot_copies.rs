// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two COPY operations of this service's instance-scoped
//! resources: `CopyDBParameterGroup` and `CopyDBSnapshot`.
//!
//! The rule under test for both: a copy is a NEW resource of the account carrying what the
//! source holds — the parameter group's family and the parameter values set on it, the
//! snapshot's source instance, its settings and the data it captured — under the name the
//! request gives it, and a source this service does not hold is reported as the modelled
//! not-found fault rather than copied from nothing.
//!
//! The rules are asserted over TABLES spanning what they generalise over rather than over
//! one request: the source is named by identifier, by a differently cased identifier and by
//! ARN; the target's name case and length differ; the optional members differ row by row;
//! and every refusal class each operation models is a row of its own.
//!
//! Every call goes over the wire: real awsQuery form POSTs through a `MultiServiceRouter`
//! carrying the SigV4 credential scope the router keys on, and every assertion reads the
//! XML the emulator answered with.

use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, emulator, expect_error, expect_ok, optional_text, text, texts, wait_for_settle, REGION,
};

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// A KMS key identifier of a form KMS names a key by: the 36-character key id.
const KMS_KEY_ID: &str = "11111111-2222-3333-4444-555555555555";

// ---------------------------------------------------------------------------
// CopyDBParameterGroup
// ---------------------------------------------------------------------------

/// One row of the parameter-group copy table: how the source is named, and the name the
/// copy is created under.
struct GroupCopy {
    case: &'static str,
    /// How this row names the source: a template where `{name}` is the stored name and
    /// `{arn}` the stored group's ARN.
    source: &'static str,
    requested_target: &'static str,
    stored_target: &'static str,
}

/// The ways a copy can name its source and its target, over one stored source group.
///
/// `SourceDBParameterGroupIdentifier` is "the identifier or ARN for the source DB parameter
/// group", so both forms are rows; `TargetDBParameterGroupIdentifier` "is stored as a
/// lowercase string", so the target's case is one too.
const GROUP_COPIES: &[GroupCopy] = &[
    GroupCopy {
        case: "the source by name, the target in lower case",
        source: "{name}",
        requested_target: "copy-by-name",
        stored_target: "copy-by-name",
    },
    GroupCopy {
        case: "the source by a differently cased name, the target in mixed case",
        source: "Copy-Source-PG",
        requested_target: "Copy-By-Case",
        stored_target: "copy-by-case",
    },
    GroupCopy {
        case: "the source by ARN",
        source: "{arn}",
        requested_target: "copy-by-arn",
        stored_target: "copy-by-arn",
    },
];

/// The source group every row above copies: a group of a family this service publishes a
/// parameter catalog for, with one parameter set on it, so the copy has something to carry.
const SOURCE_GROUP: &str = "copy-source-pg";

/// The family the source group belongs to.
const SOURCE_FAMILY: &str = "mysql8.0";

/// The parameter the source group sets, and the value it sets it to.
const SOURCE_PARAMETER: (&str, &str) = ("max_connections", "250");

/// Create [`SOURCE_GROUP`] and set [`SOURCE_PARAMETER`] on it.
async fn seed_source_group(router: &MultiServiceRouter) {
    expect_ok(
        "create the source group",
        call(
            router,
            "CreateDBParameterGroup",
            &[
                ("DBParameterGroupName", SOURCE_GROUP),
                ("DBParameterGroupFamily", SOURCE_FAMILY),
                ("Description", "the-group-every-copy-is-taken-of"),
            ],
        )
        .await,
    );
    expect_ok(
        "set a parameter on the source group",
        call(
            router,
            "ModifyDBParameterGroup",
            &[
                ("DBParameterGroupName", SOURCE_GROUP),
                ("Parameters.Parameter.1.ParameterName", SOURCE_PARAMETER.0),
                ("Parameters.Parameter.1.ParameterValue", SOURCE_PARAMETER.1),
            ],
        )
        .await,
    );
}

/// The value of `parameter` the group `name` reports as the CALLER's, or `None` where the
/// group sets none.
async fn user_parameter(router: &MultiServiceRouter, case: &str, name: &str) -> Option<String> {
    let body = expect_ok(
        case,
        call(
            router,
            "DescribeDBParameters",
            &[
                ("DBParameterGroupName", name),
                ("Source", "user"),
                ("MaxRecords", "100"),
            ],
        )
        .await,
    );
    let names = texts(&body, "ParameterName");
    let values = texts(&body, "ParameterValue");
    names
        .iter()
        .position(|reported| reported == SOURCE_PARAMETER.0)
        .and_then(|index| values.get(index).cloned())
}

#[tokio::test]
async fn a_parameter_group_copy_carries_the_sources_family_and_parameters() {
    let router = emulator();
    seed_source_group(&router).await;
    let source_arn = format!("arn:aws:rds:{REGION}:{ACCOUNT}:pg:{SOURCE_GROUP}");

    for row in GROUP_COPIES {
        let case = row.case;
        let source = row
            .source
            .replace("{name}", SOURCE_GROUP)
            .replace("{arn}", &source_arn);
        let copied = expect_ok(
            case,
            call(
                &router,
                "CopyDBParameterGroup",
                &[
                    ("SourceDBParameterGroupIdentifier", source.as_str()),
                    ("TargetDBParameterGroupIdentifier", row.requested_target),
                    ("TargetDBParameterGroupDescription", "a-copy"),
                    ("Tags.Tag.1.Key", "owner"),
                    ("Tags.Tag.1.Value", "platform"),
                ],
            )
            .await,
        );
        // The copy's identity is its own; its family is the source's.
        let answered = (
            text(&copied, "DBParameterGroupName"),
            text(&copied, "DBParameterGroupFamily"),
            text(&copied, "Description"),
            text(&copied, "DBParameterGroupArn"),
        );
        assert_eq!(
            answered,
            (
                row.stored_target.to_string(),
                SOURCE_FAMILY.to_string(),
                "a-copy".to_string(),
                format!("arn:aws:rds:{REGION}:{ACCOUNT}:pg:{}", row.stored_target),
            ),
            "case {case:?}: the copy must carry its own name and the source's family, got \
             {copied}"
        );

        // Read back: the copy is PERSISTED, and it carries the parameter the source set.
        let described = expect_ok(
            case,
            call(
                &router,
                "DescribeDBParameterGroups",
                &[("DBParameterGroupName", row.stored_target)],
            )
            .await,
        );
        assert_eq!(
            text(&described, "DBParameterGroupFamily"),
            SOURCE_FAMILY,
            "case {case:?}: the stored copy must report the source's family, got {described}"
        );
        assert_eq!(
            user_parameter(&router, case, row.stored_target).await.as_deref(),
            Some(SOURCE_PARAMETER.1),
            "case {case:?}: the copy must carry the parameter value the source set"
        );
        // ...and changing the COPY leaves the source as it was: two records, not one.
        expect_ok(
            case,
            call(
                &router,
                "ModifyDBParameterGroup",
                &[
                    ("DBParameterGroupName", row.stored_target),
                    ("Parameters.Parameter.1.ParameterName", SOURCE_PARAMETER.0),
                    ("Parameters.Parameter.1.ParameterValue", "300"),
                ],
            )
            .await,
        );
        assert_eq!(
            user_parameter(&router, case, SOURCE_GROUP).await.as_deref(),
            Some(SOURCE_PARAMETER.1),
            "case {case:?}: a change to the copy must leave the source as it was"
        );
    }
}

/// One refusal row: the parameters of a copy this operation cannot serve, and the status
/// and wire code it answers with.
struct Refusal {
    case: &'static str,
    source: &'static str,
    target: &'static str,
    status: u16,
    code: &'static str,
}

/// Every refusal class `CopyDBParameterGroup` models, plus the two its own documentation
/// states: a source that is a default group ("you can't copy a default DB parameter group")
/// and a target name outside the published name rule.
const GROUP_REFUSALS: &[Refusal] = &[
    Refusal {
        case: "a source name nothing created",
        source: "no-such-source-pg",
        target: "copy-of-nothing",
        status: 404,
        code: "DBParameterGroupNotFound",
    },
    Refusal {
        case: "a source ARN nothing created",
        source: "arn:aws:rds:us-west-2:000000000000:pg:no-such-source-pg",
        target: "copy-of-no-arn",
        status: 404,
        code: "DBParameterGroupNotFound",
    },
    Refusal {
        case: "a source group of another region",
        source: "arn:aws:rds:eu-west-1:000000000000:pg:copy-source-pg",
        target: "copy-of-another-region",
        status: 404,
        code: "DBParameterGroupNotFound",
    },
    Refusal {
        case: "a default group as the source",
        source: "default.mysql8.0",
        target: "copy-of-a-default",
        status: 400,
        code: "InvalidParameterValue",
    },
    Refusal {
        case: "a target name the name rule does not admit",
        source: SOURCE_GROUP,
        target: "1-not-a-name",
        status: 400,
        code: "InvalidParameterValue",
    },
    Refusal {
        case: "a target name already taken",
        source: SOURCE_GROUP,
        target: SOURCE_GROUP,
        status: 400,
        code: "DBParameterGroupAlreadyExists",
    },
];

#[tokio::test]
async fn a_parameter_group_copy_this_service_cannot_serve_is_refused_and_stores_nothing() {
    let router = emulator();
    seed_source_group(&router).await;

    for row in GROUP_REFUSALS {
        let case = row.case;
        expect_error(
            case,
            call(
                &router,
                "CopyDBParameterGroup",
                &[
                    ("SourceDBParameterGroupIdentifier", row.source),
                    ("TargetDBParameterGroupIdentifier", row.target),
                    ("TargetDBParameterGroupDescription", "not-served"),
                ],
            )
            .await,
            row.status,
            row.code,
        );
        // Nothing was stored under the target name the refused copy asked for, unless that
        // name is the source's own.
        if row.target != SOURCE_GROUP {
            expect_error(
                case,
                call(
                    &router,
                    "DescribeDBParameterGroups",
                    &[("DBParameterGroupName", row.target)],
                )
                .await,
                404,
                "DBParameterGroupNotFound",
            );
        }
    }

    // The source that IS a group is still exactly as it was created.
    let described = expect_ok(
        "the source after every refusal",
        call(
            &router,
            "DescribeDBParameterGroups",
            &[("DBParameterGroupName", SOURCE_GROUP)],
        )
        .await,
    );
    assert_eq!(
        text(&described, "DBParameterGroupFamily"),
        SOURCE_FAMILY,
        "case \"the source after every refusal\": got {described}"
    );
}

// ---------------------------------------------------------------------------
// CopyDBSnapshot
// ---------------------------------------------------------------------------

/// The instance every snapshot below is taken of.
const SOURCE_INSTANCE: &str = "copy-source-instance";

/// The snapshot every copy below is taken of.
const SOURCE_SNAPSHOT: &str = "copy-source-snapshot";

/// Create [`SOURCE_INSTANCE`] and take [`SOURCE_SNAPSHOT`] of it, waiting out the spans the
/// create and the snapshot owe so the snapshot is `available` — the state a copy can be
/// taken from.
async fn seed_source_snapshot(router: &MultiServiceRouter) {
    expect_ok(
        "create the source instance",
        call(
            router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", SOURCE_INSTANCE),
                ("Engine", "mysql"),
                ("DBInstanceClass", "db.t3.micro"),
                ("AllocatedStorage", "20"),
            ],
        )
        .await,
    );
    wait_for_settle();
    expect_ok(
        "take the source snapshot",
        call(
            router,
            "CreateDBSnapshot",
            &[
                ("DBInstanceIdentifier", SOURCE_INSTANCE),
                ("DBSnapshotIdentifier", SOURCE_SNAPSHOT),
                ("Tags.Tag.1.Key", "owner"),
                ("Tags.Tag.1.Value", "platform"),
            ],
        )
        .await,
    );
    wait_for_settle();
}

/// One row of the snapshot copy table.
struct SnapshotCopy {
    case: &'static str,
    /// A template over the source's identifier (`{name}`) and ARN (`{arn}`).
    source: &'static str,
    target: &'static str,
    /// The members beyond the two identifiers.
    extra: &'static [(&'static str, &'static str)],
    /// Whether the copy is expected to carry the source's tags.
    copies_tags: bool,
    /// The `KmsKeyId` the copy reports, if any.
    kms_key_id: Option<&'static str>,
}

/// The ways a copy can name its source and configure itself.
const SNAPSHOT_COPIES: &[SnapshotCopy] = &[
    SnapshotCopy {
        case: "the source by identifier, nothing else named",
        source: "{name}",
        target: "copy-plain",
        extra: &[],
        copies_tags: false,
        kms_key_id: None,
    },
    SnapshotCopy {
        case: "the source by ARN, copying its tags",
        source: "{arn}",
        target: "copy-by-arn",
        extra: &[("CopyTags", "true")],
        copies_tags: true,
        kms_key_id: None,
    },
    SnapshotCopy {
        case: "the source's tags copied BESIDE the request's own",
        source: "{name}",
        target: "copy-with-both-tag-sets",
        extra: &[
            ("CopyTags", "true"),
            ("Tags.Tag.1.Key", "cost-centre"),
            ("Tags.Tag.1.Value", "platform-eng"),
        ],
        copies_tags: true,
        kms_key_id: None,
    },
    SnapshotCopy {
        case: "a copy encrypted under a named key",
        source: "{name}",
        target: "copy-encrypted",
        extra: &[("KmsKeyId", KMS_KEY_ID)],
        copies_tags: false,
        kms_key_id: Some(KMS_KEY_ID),
    },
];

#[tokio::test]
async fn a_snapshot_copy_carries_the_sources_instance_and_settings() {
    let router = emulator();
    seed_source_snapshot(&router).await;
    let source_arn = format!("arn:aws:rds:{REGION}:{ACCOUNT}:snapshot:{SOURCE_SNAPSHOT}");

    for row in SNAPSHOT_COPIES {
        let case = row.case;
        let source = row
            .source
            .replace("{name}", SOURCE_SNAPSHOT)
            .replace("{arn}", &source_arn);
        let mut params: Vec<(&str, String)> = vec![
            ("SourceDBSnapshotIdentifier", source),
            ("TargetDBSnapshotIdentifier", row.target.to_string()),
        ];
        for (name, value) in row.extra {
            params.push((name, (*value).to_string()));
        }
        let copied = expect_ok(case, call(&router, "CopyDBSnapshot", &params).await);

        // The copy is a snapshot OF the same instance, under its own identifier, and RDS
        // answers it while the copy is still being made.
        let answered = (
            text(&copied, "DBSnapshotIdentifier"),
            text(&copied, "DBInstanceIdentifier"),
            text(&copied, "Engine"),
            text(&copied, "Status"),
            text(&copied, "DBSnapshotArn"),
        );
        assert_eq!(
            answered,
            (
                row.target.to_string(),
                SOURCE_INSTANCE.to_string(),
                "mysql".to_string(),
                "copying".to_string(),
                format!("arn:aws:rds:{REGION}:{ACCOUNT}:snapshot:{}", row.target),
            ),
            "case {case:?}: got {copied}"
        );
        // `SourceDBSnapshotIdentifier` "only has a value in the case of a cross-account or
        // cross-Region copy", and this copy is neither.
        assert_eq!(
            optional_text(&copied, "SourceDBSnapshotIdentifier"),
            None,
            "case {case:?}: a same-Region copy names no source snapshot, got {copied}"
        );
        assert_eq!(
            optional_text(&copied, "KmsKeyId").as_deref(),
            row.kms_key_id,
            "case {case:?}: the copy reports the key it was encrypted under, got {copied}"
        );

        // The copy reaches `available` on its own, and the read-back is what proves it was
        // persisted rather than echoed.
        wait_for_settle();
        let described = expect_ok(
            case,
            call(
                &router,
                "DescribeDBSnapshots",
                &[("DBSnapshotIdentifier", row.target)],
            )
            .await,
        );
        assert_eq!(
            (
                text(&described, "Status"),
                text(&described, "PercentProgress"),
                text(&described, "DBInstanceIdentifier"),
                text(&described, "AllocatedStorage"),
                // A copy is a snapshot the account asked for, so it is `manual` whatever
                // the source was: `automated` is reserved for the backups RDS schedules.
                text(&described, "SnapshotType"),
            ),
            (
                "available".to_string(),
                "100".to_string(),
                SOURCE_INSTANCE.to_string(),
                "20".to_string(),
                "manual".to_string(),
            ),
            "case {case:?}: got {described}"
        );
        let tags = texts(&described, "Key");
        assert_eq!(
            tags.contains(&"owner".to_string()),
            row.copies_tags,
            "case {case:?}: CopyTags decides whether the source's tags are on the copy, got \
             {described}"
        );
        // `Tags` is the caller's own list and `CopyTags` the source's: a request that sets
        // both keeps both, so neither member silences the other.
        if row
            .extra
            .iter()
            .any(|(member, _)| *member == "Tags.Tag.1.Key")
        {
            assert!(
                tags.contains(&"cost-centre".to_string()) && tags.contains(&"owner".to_string()),
                "case {case:?}: the copy must carry the request's own tags beside the \
                 source's, got {tags:?} in {described}"
            );
        }
        // A copy of the copy is servable, which is what "available" means for one.
        let again = format!("{}-again", row.target);
        expect_ok(
            case,
            call(
                &router,
                "CopyDBSnapshot",
                &[
                    ("SourceDBSnapshotIdentifier", row.target),
                    ("TargetDBSnapshotIdentifier", again.as_str()),
                ],
            )
            .await,
        );
    }
}

/// Every refusal class `CopyDBSnapshot` models that this service can reach.
#[tokio::test]
async fn a_snapshot_copy_this_service_cannot_serve_is_refused_and_stores_nothing() {
    let router = emulator();
    seed_source_snapshot(&router).await;

    // (case, source, target, status, code)
    let rows: [(&str, &str, &str, u16, &str); 5] = [
        (
            "a source identifier nothing created",
            "no-such-snapshot",
            "copy-of-nothing",
            404,
            "DBSnapshotNotFound",
        ),
        (
            "a source ARN nothing created",
            "arn:aws:rds:us-west-2:000000000000:snapshot:no-such-snapshot",
            "copy-of-no-arn",
            404,
            "DBSnapshotNotFound",
        ),
        (
            "a source snapshot of another region",
            "arn:aws:rds:eu-west-1:000000000000:snapshot:copy-source-snapshot",
            "copy-of-another-region",
            404,
            "DBSnapshotNotFound",
        ),
        (
            "a source identifier that is no identifier at all",
            "1-not-an-identifier",
            "copy-of-a-bad-name",
            400,
            "InvalidParameterValue",
        ),
        (
            "a target identifier already taken",
            SOURCE_SNAPSHOT,
            SOURCE_SNAPSHOT,
            400,
            "DBSnapshotAlreadyExists",
        ),
    ];

    for (case, source, target, status, code) in rows {
        let body = expect_error(
            case,
            call(
                &router,
                "CopyDBSnapshot",
                &[
                    ("SourceDBSnapshotIdentifier", source),
                    ("TargetDBSnapshotIdentifier", target),
                ],
            )
            .await,
            status,
            code,
        );
        // The refusal names what the request carried, so the caller can place it.
        assert!(
            text(&body, "Message").contains(source) || text(&body, "Message").contains(target),
            "case {case:?}: the refusal must name the request's own value, got {body}"
        );
        if target != SOURCE_SNAPSHOT {
            let described = expect_ok(
                case,
                call(
                    &router,
                    "DescribeDBSnapshots",
                    &[("MaxRecords", "100")],
                )
                .await,
            );
            assert!(
                !texts(&described, "DBSnapshotIdentifier").contains(&target.to_string()),
                "case {case:?}: a refused copy must store nothing, got {described}"
            );
        }
    }

    // A source that is not available yet cannot be copied: the snapshot a caller has only
    // just started is still being taken, and the modelled state fault says so.
    expect_ok(
        "take a second snapshot",
        call(
            &router,
            "CreateDBSnapshot",
            &[
                ("DBInstanceIdentifier", SOURCE_INSTANCE),
                ("DBSnapshotIdentifier", "copy-source-creating"),
            ],
        )
        .await,
    );
    expect_error(
        "a source still being taken",
        call(
            &router,
            "CopyDBSnapshot",
            &[
                ("SourceDBSnapshotIdentifier", "copy-source-creating"),
                ("TargetDBSnapshotIdentifier", "copy-of-creating"),
            ],
        )
        .await,
        400,
        "InvalidDBSnapshotState",
    );
    expect_error(
        "a KMS key identifier of no KMS form",
        call(
            &router,
            "CopyDBSnapshot",
            &[
                ("SourceDBSnapshotIdentifier", SOURCE_SNAPSHOT),
                ("TargetDBSnapshotIdentifier", "copy-with-a-bad-key"),
                ("KmsKeyId", "not-a-kms-key"),
            ],
        )
        .await,
        400,
        "KMSKeyNotAccessibleFault",
    );
}
