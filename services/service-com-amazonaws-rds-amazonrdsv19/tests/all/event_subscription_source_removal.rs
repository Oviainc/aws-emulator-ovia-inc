// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `RemoveSourceIdentifierFromSubscription`.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter`
//! exactly as the production router mounts it (through `db_instances::emulator`, the one
//! mounting helper this test target has), and each request is a real awsQuery form POST
//! carrying the SigV4 credential scope the router keys on. The assertions read the XML
//! the emulator answers with, so a handler that computed nothing could not pass them.
//!
//! The rule under test: removing a source identifier from a stored event subscription
//! drops exactly that identifier from the subscription's own source list, which every
//! later read of the subscription reports, and leaves the other sources in the order the
//! subscription held them; the identifier is resolved against the subscription's OWN
//! sources rather than against the region's resources, so one the subscription does not
//! cover is the modelled `SourceNotFoundFault`, and a subscription name the region holds
//! nothing under is the modelled `SubscriptionNotFoundFault`.
//!
//! The tables below span the classes that rule generalises over: the source type the
//! subscription names (four resolved against stored resources of that type, and two this
//! service holds no records of), how many sources the subscription carried, which
//! position in that list is removed (first, middle, last, all of them) and in what order,
//! removing a source twice, the regional scope the subscription name resolves in, and a
//! source whose own resource was deleted after the subscription took it on.
//!
//! The source types come from `CreateEventSubscriptionMessage$SourceType`'s published
//! value list, through `EVENT_SUBSCRIPTION_SOURCE_TYPES`, and the rows include types the
//! issue cited none of.

use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, text, texts, wait_for_settle, REGION,
};
use crate::event_subscription_sources::{
    add_source, create_subscription, described_source_ids, seed_sources, ACCOUNT, OTHER_REGION,
    SEEDED_CLUSTER, SEEDED_PARAMETER_GROUP, SEEDED_SNAPSHOT,
};

/// `RemoveSourceIdentifierFromSubscription` in `region`.
async fn remove_source_in(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    source_identifier: &str,
) -> (u16, String) {
    call_in(
        router,
        region,
        "RemoveSourceIdentifierFromSubscription",
        &[
            ("SubscriptionName", name),
            ("SourceIdentifier", source_identifier),
        ],
    )
    .await
}

/// `RemoveSourceIdentifierFromSubscription` in [`REGION`].
async fn remove_source(
    router: &MultiServiceRouter,
    name: &str,
    source_identifier: &str,
) -> (u16, String) {
    remove_source_in(router, REGION, name, source_identifier).await
}

/// One row of the remove table.
struct RemoveCase {
    case: &'static str,
    /// The `SourceType` the subscription names.
    source_type: &'static str,
    /// The sources the subscription is created with, in this order.
    seeded: &'static [&'static str],
    /// The identifiers removed, one call each, in this order.
    removed: &'static [&'static str],
    /// The sources the subscription reports afterwards.
    expected: &'static [&'static str],
}

/// The rows differ along what the rule generalises over: the source type (four this
/// service resolves an identifier against and two it holds no records of), how many
/// sources the subscription carried, which position in that list the removal names, how
/// many removals follow and in what order.
const REMOVE_CASES: &[RemoveCase] = &[
    RemoveCase {
        case: "db-instance, the subscription's only source",
        source_type: "db-instance",
        seeded: &["emu-src-instance-a"],
        removed: &["emu-src-instance-a"],
        expected: &[],
    },
    RemoveCase {
        case: "db-instance, the first of two",
        source_type: "db-instance",
        seeded: &["emu-src-instance-a", "emu-src-instance-b"],
        removed: &["emu-src-instance-a"],
        expected: &["emu-src-instance-b"],
    },
    RemoveCase {
        case: "db-instance, the last of two",
        source_type: "db-instance",
        seeded: &["emu-src-instance-a", "emu-src-instance-b"],
        removed: &["emu-src-instance-b"],
        expected: &["emu-src-instance-a"],
    },
    RemoveCase {
        case: "db-instance, both sources, one call each",
        source_type: "db-instance",
        seeded: &["emu-src-instance-a", "emu-src-instance-b"],
        removed: &["emu-src-instance-a", "emu-src-instance-b"],
        expected: &[],
    },
    RemoveCase {
        case: "db-instance, both sources removed back to front",
        source_type: "db-instance",
        seeded: &["emu-src-instance-a", "emu-src-instance-b"],
        removed: &["emu-src-instance-b", "emu-src-instance-a"],
        expected: &[],
    },
    RemoveCase {
        case: "db-security-group, the middle of three on a type nothing resolves",
        source_type: "db-security-group",
        seeded: &[
            "emu-src-security-group-a",
            "emu-src-security-group-b",
            "emu-src-security-group-c",
        ],
        removed: &["emu-src-security-group-b"],
        expected: &["emu-src-security-group-a", "emu-src-security-group-c"],
    },
    RemoveCase {
        case: "db-snapshot, a source resolved against a stored snapshot",
        source_type: "db-snapshot",
        seeded: &[SEEDED_SNAPSHOT],
        removed: &[SEEDED_SNAPSHOT],
        expected: &[],
    },
    RemoveCase {
        case: "db-cluster, a source resolved against a stored DB cluster",
        source_type: "db-cluster",
        seeded: &[SEEDED_CLUSTER],
        removed: &[SEEDED_CLUSTER],
        expected: &[],
    },
    RemoveCase {
        case: "db-parameter-group, a source resolved against a stored parameter group",
        source_type: "db-parameter-group",
        seeded: &[SEEDED_PARAMETER_GROUP],
        removed: &[SEEDED_PARAMETER_GROUP],
        expected: &[],
    },
    RemoveCase {
        case: "custom-engine-version, a source type no single identifier names here",
        source_type: "custom-engine-version",
        seeded: &["emu-src-engine-version-a", "emu-src-engine-version-b"],
        removed: &["emu-src-engine-version-a"],
        expected: &["emu-src-engine-version-b"],
    },
];

#[tokio::test]
async fn a_removed_source_is_dropped_from_the_subscription_and_from_describe() {
    let router = emulator();
    seed_sources(&router).await;

    for (index, row) in REMOVE_CASES.iter().enumerate() {
        let name = format!("emu-remove-sub-{index}");
        expect_ok(
            row.case,
            create_subscription(&router, REGION, &name, Some(row.source_type), row.seeded).await,
        );

        for source_identifier in row.removed {
            let removed = expect_ok(
                row.case,
                remove_source(&router, &name, source_identifier).await,
            );
            // The removal answers with the subscription it changed, not a bare
            // acknowledgement.
            assert_eq!(
                text(&removed, "CustSubscriptionId"),
                name,
                "case {:?}: expected the subscription's own name, got {removed}",
                row.case
            );
            assert_eq!(
                text(&removed, "SourceType"),
                row.source_type,
                "case {:?}: expected the subscription's source type, got {removed}",
                row.case
            );
            assert_eq!(
                text(&removed, "EventSubscriptionArn"),
                format!("arn:aws:rds:{REGION}:{ACCOUNT}:es:{name}"),
                "case {:?}: expected the subscription's own ARN, got {removed}",
                row.case
            );
            let answered = texts(&removed, "SourceId");
            assert!(
                !answered.contains(&(*source_identifier).to_string()),
                "case {:?}: expected the removed source {source_identifier} to be gone from \
                 the answer, got {answered:?}",
                row.case
            );
        }

        // The round trip through the describe is what proves the removal was PERSISTED
        // rather than only reflected in the answer.
        let reported = described_source_ids(&router, row.case, &name).await;
        assert_eq!(
            reported,
            row.expected
                .iter()
                .map(|source_id| (*source_id).to_string())
                .collect::<Vec<String>>(),
            "case {:?}: expected the subscription's remaining sources, got {reported:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_source_the_subscription_does_not_cover_is_refused() {
    let router = emulator();
    seed_sources(&router).await;

    expect_ok(
        "a db-instance subscription with one source",
        create_subscription(
            &router,
            REGION,
            "emu-remove-sub-instance",
            Some("db-instance"),
            &["emu-src-instance-a"],
        )
        .await,
    );
    expect_ok(
        "a subscription that names no source type and no sources",
        create_subscription(&router, REGION, "emu-remove-sub-everything", None, &[]).await,
    );

    let rows: &[(&str, &str, &str, u16, &str)] = &[
        (
            "a subscription name nothing created",
            "emu-remove-sub-absent",
            "emu-src-instance-a",
            404,
            "SubscriptionNotFound",
        ),
        (
            // The differentiator: the identifier names a DB instance this region really
            // holds, and the subscription still does not cover it.
            "a stored DB instance the subscription does not cover",
            "emu-remove-sub-instance",
            "emu-src-instance-b",
            404,
            "SourceNotFound",
        ),
        (
            "a DB instance nothing created",
            "emu-remove-sub-instance",
            "emu-absent-instance",
            404,
            "SourceNotFound",
        ),
        (
            "a subscription that covers every source of the account",
            "emu-remove-sub-everything",
            "emu-src-instance-a",
            404,
            "SourceNotFound",
        ),
        (
            "a source identifier that is not an identifier at all",
            "emu-remove-sub-instance",
            "-leading-hyphen",
            400,
            "InvalidParameterValue",
        ),
        (
            "a source identifier with two consecutive hyphens",
            "emu-remove-sub-instance",
            "emu--src-instance-a",
            400,
            "InvalidParameterValue",
        ),
    ];

    for (case, name, source_identifier, status, code) in rows {
        expect_error(
            case,
            remove_source(&router, name, source_identifier).await,
            *status,
            code,
        );
    }

    // Every refusal above left each subscription exactly as it was.
    let reported = described_source_ids(&router, "after the refusals", "emu-remove-sub-instance").await;
    assert_eq!(
        reported,
        vec!["emu-src-instance-a".to_string()],
        "expected the subscription to still carry the source the create named, got {reported:?}"
    );
    let reported =
        described_source_ids(&router, "after the refusals", "emu-remove-sub-everything").await;
    assert!(
        reported.is_empty(),
        "expected the all-sources subscription to still carry no sources, got {reported:?}"
    );
}

#[tokio::test]
async fn removing_the_same_source_twice_is_refused_the_second_time() {
    let router = emulator();
    seed_sources(&router).await;

    let name = "emu-remove-sub-twice";
    expect_ok(
        "create",
        create_subscription(
            &router,
            REGION,
            name,
            Some("db-instance"),
            &["emu-src-instance-a", "emu-src-instance-b"],
        )
        .await,
    );

    expect_ok(
        "the first removal",
        remove_source(&router, name, "emu-src-instance-a").await,
    );
    // The source is gone from the subscription, so the second call names a source the
    // subscription no longer covers.
    expect_error(
        "the same removal again",
        remove_source(&router, name, "emu-src-instance-a").await,
        404,
        "SourceNotFound",
    );
    let reported = described_source_ids(&router, "after the two removals", name).await;
    assert_eq!(
        reported,
        vec!["emu-src-instance-b".to_string()],
        "expected the one remaining source, got {reported:?}"
    );

    // The source can be taken back on, and removed again: the subscription's source list
    // is what the two operations read and write.
    expect_ok(
        "adding the removed source back",
        add_source(&router, name, "emu-src-instance-a").await,
    );
    let reported = described_source_ids(&router, "after the add back", name).await;
    assert_eq!(
        reported,
        vec![
            "emu-src-instance-b".to_string(),
            "emu-src-instance-a".to_string()
        ],
        "expected the re-added source at the end of the list, got {reported:?}"
    );
    expect_ok(
        "removing it again",
        remove_source(&router, name, "emu-src-instance-a").await,
    );
    let reported = described_source_ids(&router, "after the second removal", name).await;
    assert_eq!(
        reported,
        vec!["emu-src-instance-b".to_string()],
        "expected the one remaining source again, got {reported:?}"
    );
}

#[tokio::test]
async fn a_subscription_name_resolves_only_in_the_region_it_was_created_in() {
    let router = emulator();

    let name = "emu-remove-sub-regional";
    expect_ok(
        "a subscription in the other region",
        create_subscription(
            &router,
            OTHER_REGION,
            name,
            Some("db-security-group"),
            &["emu-src-security-group-a"],
        )
        .await,
    );

    // The same name signed for this region names no subscription at all.
    expect_error(
        "the other region's subscription, from this one",
        remove_source(&router, name, "emu-src-security-group-a").await,
        404,
        "SubscriptionNotFound",
    );
    let removed = expect_ok(
        "the other region's subscription, from that region",
        remove_source_in(&router, OTHER_REGION, name, "emu-src-security-group-a").await,
    );
    assert_eq!(
        text(&removed, "EventSubscriptionArn"),
        format!("arn:aws:rds:{OTHER_REGION}:{ACCOUNT}:es:{name}"),
        "expected the ARN of the region the subscription was created in, got {removed}"
    );
    assert!(
        texts(&removed, "SourceId").is_empty(),
        "expected the subscription to carry no sources after the removal, got {removed}"
    );
}

#[tokio::test]
async fn a_source_whose_resource_is_gone_is_still_removable() {
    let router = emulator();
    let instance = "emu-remove-src-gone";
    expect_ok(
        instance,
        call(
            &router,
            "CreateDBInstance",
            &[
                ("DBInstanceIdentifier", instance),
                ("Engine", "mysql"),
                ("DBInstanceClass", "db.m5.large"),
            ],
        )
        .await,
    );

    let name = "emu-remove-sub-gone";
    expect_ok(
        "a db-instance subscription carrying that instance",
        create_subscription(&router, REGION, name, Some("db-instance"), &[instance]).await,
    );

    // The instance leaves the store, so nothing in the region resolves its identifier any
    // more.
    wait_for_settle();
    expect_ok(
        "deleting the instance",
        call(
            &router,
            "DeleteDBInstance",
            &[
                ("DBInstanceIdentifier", instance),
                ("SkipFinalSnapshot", "true"),
            ],
        )
        .await,
    );
    wait_for_settle();
    expect_error(
        "reading the deleted instance back",
        call(
            &router,
            "DescribeDBInstances",
            &[("DBInstanceIdentifier", instance)],
        )
        .await,
        404,
        "DBInstanceNotFound",
    );
    // Taking the identifier ON resolves it against the region's resources, and there is
    // no longer one to resolve.
    expect_error(
        "adding the deleted instance as a source",
        add_source(&router, name, instance).await,
        404,
        "SourceNotFound",
    );

    // Removing it is resolved against the SUBSCRIPTION's own sources, which still carry
    // it, so the removal is served and the subscription is left without it.
    let removed = expect_ok(
        "removing the deleted instance as a source",
        remove_source(&router, name, instance).await,
    );
    assert!(
        texts(&removed, "SourceId").is_empty(),
        "expected the subscription to carry no sources after the removal, got {removed}"
    );
    let reported = described_source_ids(&router, "after the removal", name).await;
    assert!(
        reported.is_empty(),
        "expected the removal to be persisted, got {reported:?}"
    );
}
