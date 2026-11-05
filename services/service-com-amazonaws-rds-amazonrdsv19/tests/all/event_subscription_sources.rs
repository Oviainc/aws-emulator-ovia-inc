// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `AddSourceIdentifierToSubscription`.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter`
//! exactly as the production router mounts it (through `db_instances::emulator`, the one
//! mounting helper this test target has), and each request is a real awsQuery form POST
//! carrying the SigV4 credential scope the router keys on. The assertions read the XML
//! the emulator answers with, so a handler that computed nothing could not pass them.
//!
//! The rule under test: adding a source identifier to a stored event subscription appends
//! it to that subscription's own source list, which every later read of the subscription
//! reports; the identifier is resolved against the stored resources of the subscription's
//! source type, so one that names none is the modelled `SourceNotFoundFault`, and a
//! subscription name the region holds nothing under is the modelled
//! `SubscriptionNotFoundFault`.
//!
//! The tables below span the classes that rule generalises over: the source type the
//! subscription names (four resolved against stored resources of that type, and two this
//! service holds no records of), how many sources the subscription already carried, how
//! many are added and in what order, adding a source the subscription already covers, the
//! regional scope the subscription name resolves in, and the stored-source bound plus one
//! add past it.
//!
//! The source types come from `CreateEventSubscriptionMessage$SourceType`'s published
//! value list, and the rows include types the issue cited none of: one table walks the
//! WHOLE published list, asserting that an identifier naming no stored resource of the
//! subscription's type is refused for every type this service resolves against, and that
//! a DB proxy — a type the issue cited none of — round-trips through the store.

use service_com_amazonaws_rds_amazonrdsv19::state::{
    EVENT_SUBSCRIPTION_SOURCE_TYPES, MAX_SOURCE_IDS_PER_SUBSCRIPTION,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, text, texts, wait_for_settle, REGION,
};

/// A second region, so the regional scope of a subscription is observable.
pub(crate) const OTHER_REGION: &str = "eu-west-1";

/// The canonical account id every generated emulator presents.
pub(crate) const ACCOUNT: &str = "000000000000";

/// The SNS topic every subscription below publishes to.
pub(crate) fn topic_arn(region: &str) -> String {
    format!("arn:aws:sns:{region}:{ACCOUNT}:rds-events")
}

/// The DB instances the rows below resolve `db-instance` sources against.
const SEEDED_INSTANCES: &[&str] = &["emu-src-instance-a", "emu-src-instance-b"];

/// The DB snapshot the `db-snapshot` row resolves its source against, and the instance it
/// is taken from.
pub(crate) const SEEDED_SNAPSHOT: &str = "emu-src-snapshot-a";
const SNAPSHOT_SOURCE_INSTANCE: &str = "emu-src-snapshot-instance";

/// The DB cluster the `db-cluster` row resolves its source against.
pub(crate) const SEEDED_CLUSTER: &str = "emu-src-cluster-a";

/// The DB parameter group the `db-parameter-group` row resolves its source against.
pub(crate) const SEEDED_PARAMETER_GROUP: &str = "emu-src-parameter-group-a";

/// `CreateEventSubscription` in `region`, optionally naming a source type and the sources
/// the subscription starts with.
pub(crate) async fn create_subscription(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    source_type: Option<&str>,
    source_ids: &[&str],
) -> (u16, String) {
    let mut params: Vec<(String, String)> = vec![
        ("SubscriptionName".to_string(), name.to_string()),
        ("SnsTopicArn".to_string(), topic_arn(region)),
    ];
    if let Some(source_type) = source_type {
        params.push(("SourceType".to_string(), source_type.to_string()));
    }
    for (index, source_id) in source_ids.iter().enumerate() {
        params.push((
            format!("SourceIds.SourceId.{}", index + 1),
            (*source_id).to_string(),
        ));
    }
    call_in(router, region, "CreateEventSubscription", &params).await
}

/// `AddSourceIdentifierToSubscription` in `region`.
pub(crate) async fn add_source_in(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    source_identifier: &str,
) -> (u16, String) {
    call_in(
        router,
        region,
        "AddSourceIdentifierToSubscription",
        &[
            ("SubscriptionName", name),
            ("SourceIdentifier", source_identifier),
        ],
    )
    .await
}

/// `AddSourceIdentifierToSubscription` in [`REGION`].
pub(crate) async fn add_source(
    router: &MultiServiceRouter,
    name: &str,
    source_identifier: &str,
) -> (u16, String) {
    add_source_in(router, REGION, name, source_identifier).await
}

/// The source identifiers `DescribeEventSubscriptions` reports for one subscription.
pub(crate) async fn described_source_ids(router: &MultiServiceRouter, case: &str, name: &str) -> Vec<String> {
    let body = expect_ok(
        case,
        call(
            router,
            "DescribeEventSubscriptions",
            &[("SubscriptionName", name)],
        )
        .await,
    );
    texts(&body, "SourceId")
}

/// Create the DB instances, DB cluster, DB parameter group and DB snapshot the rows
/// resolve their sources against.
pub(crate) async fn seed_sources(router: &MultiServiceRouter) {
    for identifier in SEEDED_INSTANCES
        .iter()
        .copied()
        .chain([SNAPSHOT_SOURCE_INSTANCE])
    {
        expect_ok(
            identifier,
            call(
                router,
                "CreateDBInstance",
                &[
                    ("DBInstanceIdentifier", identifier),
                    ("Engine", "mysql"),
                    ("DBInstanceClass", "db.m5.large"),
                ],
            )
            .await,
        );
    }
    expect_ok(
        SEEDED_CLUSTER,
        call(
            router,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", SEEDED_CLUSTER),
                ("Engine", "aurora-mysql"),
            ],
        )
        .await,
    );
    expect_ok(
        SEEDED_PARAMETER_GROUP,
        call(
            router,
            "CreateDBParameterGroup",
            &[
                ("DBParameterGroupName", SEEDED_PARAMETER_GROUP),
                ("DBParameterGroupFamily", "mysql8.0"),
                ("Description", "events"),
            ],
        )
        .await,
    );
    // A snapshot is taken from an instance that has reached `available`.
    wait_for_settle();
    expect_ok(
        SEEDED_SNAPSHOT,
        call(
            router,
            "CreateDBSnapshot",
            &[
                ("DBInstanceIdentifier", SNAPSHOT_SOURCE_INSTANCE),
                ("DBSnapshotIdentifier", SEEDED_SNAPSHOT),
            ],
        )
        .await,
    );
}

/// One row of the add table.
struct AddCase {
    case: &'static str,
    /// The `SourceType` the subscription names.
    source_type: &'static str,
    /// The sources the subscription is created with.
    seeded: &'static [&'static str],
    /// The identifiers added, one call each, in this order.
    added: &'static [&'static str],
    /// The sources the subscription reports afterwards.
    expected: &'static [&'static str],
}

/// The rows differ along what the rule generalises over: the source type (four this
/// service resolves an identifier against and two it holds no records of), how many
/// sources the subscription already carried, how many adds follow, and the order the adds
/// arrive in.
const ADD_CASES: &[AddCase] = &[
    AddCase {
        case: "db-instance, the subscription's first source",
        source_type: "db-instance",
        seeded: &[],
        added: &["emu-src-instance-a"],
        expected: &["emu-src-instance-a"],
    },
    AddCase {
        case: "db-instance, appended after the source the create named",
        source_type: "db-instance",
        seeded: &["emu-src-instance-a"],
        added: &["emu-src-instance-b"],
        expected: &["emu-src-instance-a", "emu-src-instance-b"],
    },
    AddCase {
        case: "db-instance, two adds reported in the order they arrived",
        source_type: "db-instance",
        seeded: &[],
        added: &["emu-src-instance-b", "emu-src-instance-a"],
        expected: &["emu-src-instance-b", "emu-src-instance-a"],
    },
    AddCase {
        case: "db-snapshot, resolved against a stored snapshot",
        source_type: "db-snapshot",
        seeded: &[],
        added: &["emu-src-snapshot-a"],
        expected: &["emu-src-snapshot-a"],
    },
    AddCase {
        case: "db-cluster, resolved against a stored DB cluster",
        source_type: "db-cluster",
        seeded: &[],
        added: &["emu-src-cluster-a"],
        expected: &["emu-src-cluster-a"],
    },
    AddCase {
        case: "db-parameter-group, resolved against a stored parameter group",
        source_type: "db-parameter-group",
        seeded: &[],
        added: &["emu-src-parameter-group-a"],
        expected: &["emu-src-parameter-group-a"],
    },
    AddCase {
        case: "db-security-group, a source type this service holds no records of",
        source_type: "db-security-group",
        seeded: &["emu-src-security-group-a"],
        added: &["emu-src-security-group-b"],
        expected: &["emu-src-security-group-a", "emu-src-security-group-b"],
    },
    AddCase {
        case: "custom-engine-version, a source type no single identifier names here",
        source_type: "custom-engine-version",
        seeded: &[],
        added: &["emu-src-engine-version-a"],
        expected: &["emu-src-engine-version-a"],
    },
];

#[tokio::test]
async fn an_added_source_is_persisted_and_read_back_by_describe() {
    let router = emulator();
    seed_sources(&router).await;

    for (index, row) in ADD_CASES.iter().enumerate() {
        let name = format!("emu-add-sub-{index}");
        expect_ok(
            row.case,
            create_subscription(&router, REGION, &name, Some(row.source_type), row.seeded).await,
        );
        for source_identifier in row.added {
            let added = expect_ok(
                row.case,
                add_source(&router, &name, source_identifier).await,
            );
            // The add answers with the subscription it changed, not a bare acknowledgement.
            assert_eq!(
                text(&added, "CustSubscriptionId"),
                name,
                "case {:?}: expected the subscription's own name, got {added}",
                row.case
            );
            assert_eq!(
                text(&added, "SourceType"),
                row.source_type,
                "case {:?}: expected the subscription's source type, got {added}",
                row.case
            );
            assert_eq!(
                text(&added, "EventSubscriptionArn"),
                format!("arn:aws:rds:{REGION}:{ACCOUNT}:es:{name}"),
                "case {:?}: expected the subscription's own ARN, got {added}",
                row.case
            );
            assert!(
                texts(&added, "SourceId").contains(&(*source_identifier).to_string()),
                "case {:?}: expected the added source {source_identifier} in the answer, \
                 got {added}",
                row.case
            );
        }

        // The round trip through the describe is what proves the source was PERSISTED
        // rather than echoed back.
        let reported = described_source_ids(&router, row.case, &name).await;
        assert_eq!(
            reported,
            row.expected
                .iter()
                .map(|source_id| (*source_id).to_string())
                .collect::<Vec<String>>(),
            "case {:?}: expected the subscription's stored sources, got {reported:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn adding_a_source_the_subscription_already_covers_changes_nothing() {
    let router = emulator();
    seed_sources(&router).await;

    let name = "emu-add-sub-idempotent";
    expect_ok(
        "create",
        create_subscription(
            &router,
            REGION,
            name,
            Some("db-instance"),
            &["emu-src-instance-a"],
        )
        .await,
    );

    // The sources of a subscription are a SET: the identifier the create named and the
    // identifier a first add named are each stored once, however many adds name them.
    for (case, source_identifier) in [
        ("the source the create named", "emu-src-instance-a"),
        ("a source a first add named", "emu-src-instance-b"),
        ("that same source again", "emu-src-instance-b"),
    ] {
        expect_ok(case, add_source(&router, name, source_identifier).await);
    }
    let reported = described_source_ids(&router, "after the repeated adds", name).await;
    assert_eq!(
        reported,
        vec![
            "emu-src-instance-a".to_string(),
            "emu-src-instance-b".to_string()
        ],
        "expected each source stored exactly once, got {reported:?}"
    );
}

#[tokio::test]
async fn a_source_identifier_the_region_cannot_resolve_is_refused() {
    let router = emulator();
    seed_sources(&router).await;

    expect_ok(
        "a db-instance subscription",
        create_subscription(&router, REGION, "emu-add-sub-instance", Some("db-instance"), &[]).await,
    );
    expect_ok(
        "a db-parameter-group subscription",
        create_subscription(
            &router,
            REGION,
            "emu-add-sub-group",
            Some("db-parameter-group"),
            &[],
        )
        .await,
    );
    expect_ok(
        "a subscription that names no source type",
        create_subscription(&router, REGION, "emu-add-sub-everything", None, &[]).await,
    );

    let rows: &[(&str, &str, &str, u16, &str)] = &[
        (
            "a subscription name nothing created",
            "emu-add-sub-absent",
            "emu-src-instance-a",
            404,
            "SubscriptionNotFound",
        ),
        (
            "a DB instance nothing created",
            "emu-add-sub-instance",
            "emu-absent-instance",
            404,
            "SourceNotFound",
        ),
        (
            "a DB snapshot's identifier on a db-instance subscription",
            "emu-add-sub-instance",
            "emu-src-snapshot-a",
            404,
            "SourceNotFound",
        ),
        (
            "a DB parameter group nothing created",
            "emu-add-sub-group",
            "emu-absent-parameter-group",
            404,
            "SourceNotFound",
        ),
        (
            "a subscription with no source type for the identifier to belong to",
            "emu-add-sub-everything",
            "emu-src-instance-a",
            400,
            "InvalidParameterCombination",
        ),
        (
            "a source identifier that is not an identifier at all",
            "emu-add-sub-instance",
            "-leading-hyphen",
            400,
            "InvalidParameterValue",
        ),
        (
            "a source identifier with two consecutive hyphens",
            "emu-add-sub-instance",
            "emu--src-instance-a",
            400,
            "InvalidParameterValue",
        ),
    ];

    for (case, name, source_identifier, status, code) in rows {
        expect_error(
            case,
            add_source(&router, name, source_identifier).await,
            *status,
            code,
        );
    }

    // Every refusal above left each subscription exactly as it was.
    for name in [
        "emu-add-sub-instance",
        "emu-add-sub-group",
        "emu-add-sub-everything",
    ] {
        let reported = described_source_ids(&router, "after the refusals", name).await;
        assert!(
            reported.is_empty(),
            "expected {name} to still carry no sources, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_subscription_name_resolves_only_in_the_region_it_was_created_in() {
    let router = emulator();
    seed_sources(&router).await;

    let name = "emu-add-sub-regional";
    expect_ok(
        "a subscription in the other region",
        create_subscription(&router, OTHER_REGION, name, Some("db-security-group"), &[]).await,
    );

    // The same name signed for this region names no subscription at all.
    expect_error(
        "the other region's subscription, from this one",
        add_source(&router, name, "emu-src-security-group-a").await,
        404,
        "SubscriptionNotFound",
    );
    let added = expect_ok(
        "the other region's subscription, from that region",
        add_source_in(&router, OTHER_REGION, name, "emu-src-security-group-a").await,
    );
    assert_eq!(
        text(&added, "EventSubscriptionArn"),
        format!("arn:aws:rds:{OTHER_REGION}:{ACCOUNT}:es:{name}"),
        "expected the ARN of the region the subscription was created in, got {added}"
    );
    assert_eq!(
        texts(&added, "SourceId"),
        vec!["emu-src-security-group-a".to_string()],
        "expected the added source, got {added}"
    );
}

#[tokio::test]
async fn a_source_type_a_subscription_may_not_name_never_reaches_the_resolution() {
    let router = emulator();

    // A value outside the set the model's `SourceType` enum carries names no event source
    // at all, so no subscription of that type can exist for an add to resolve a source
    // against. (`db-shard-group` IS one of them, so it is not the value to probe with.)
    let unpublished = "db-replica";
    assert!(
        !EVENT_SUBSCRIPTION_SOURCE_TYPES.contains(&unpublished),
        "case {unpublished}: expected it outside the published subscription source types, \
         got {EVENT_SUBSCRIPTION_SOURCE_TYPES:?}"
    );
    expect_error(
        "a subscription source type AWS does not publish",
        create_subscription(
            &router,
            REGION,
            "emu-add-sub-unpublished-type",
            Some(unpublished),
            &[],
        )
        .await,
        400,
        "InvalidParameterValue",
    );
    expect_error(
        "an add against the subscription the refused create did not store",
        add_source(
            &router,
            "emu-add-sub-unpublished-type",
            "emu-src-unpublished-a",
        )
        .await,
        404,
        "SubscriptionNotFound",
    );
}

/// One row of the published-source-type catalog: the type a subscription may name, and
/// whether this service holds records a source identifier of that type is resolved
/// against.
struct CatalogCase {
    source_type: &'static str,
    /// True when an identifier of this type is resolved against stored resources, so one
    /// that names none is refused; false for a type this service holds no records of, for
    /// which there is nothing to resolve against.
    resolved: bool,
}

/// Every value a subscription's `SourceType` may name, in the order
/// `EVENT_SUBSCRIPTION_SOURCE_TYPES` carries them — including the ones the issue cited
/// none of (`db-cluster-snapshot`, `db-proxy`, `zero-etl`, `blue-green-deployment`,
/// `db-shard-group`).
const CATALOG_CASES: &[CatalogCase] = &[
    CatalogCase {
        source_type: "db-instance",
        resolved: true,
    },
    CatalogCase {
        source_type: "db-security-group",
        resolved: false,
    },
    CatalogCase {
        source_type: "db-parameter-group",
        resolved: true,
    },
    CatalogCase {
        source_type: "db-snapshot",
        resolved: true,
    },
    CatalogCase {
        source_type: "db-cluster",
        resolved: true,
    },
    CatalogCase {
        source_type: "db-cluster-snapshot",
        resolved: true,
    },
    CatalogCase {
        source_type: "custom-engine-version",
        resolved: false,
    },
    CatalogCase {
        source_type: "db-proxy",
        resolved: true,
    },
    CatalogCase {
        source_type: "blue-green-deployment",
        resolved: true,
    },
    CatalogCase {
        source_type: "db-shard-group",
        resolved: true,
    },
    CatalogCase {
        source_type: "zero-etl",
        resolved: true,
    },
];

#[tokio::test]
async fn an_absent_identifier_is_refused_for_every_source_type_that_resolves_one() {
    let router = emulator();

    // The table spans the WHOLE published catalog, so a value added to it cannot quietly
    // go untested.
    let covered: Vec<&str> = CATALOG_CASES.iter().map(|row| row.source_type).collect();
    assert_eq!(
        covered,
        EVENT_SUBSCRIPTION_SOURCE_TYPES.to_vec(),
        "case the published catalog: expected one row per published source type \
         ({EVENT_SUBSCRIPTION_SOURCE_TYPES:?}), got {covered:?}"
    );

    for (index, row) in CATALOG_CASES.iter().enumerate() {
        let name = format!("emu-add-sub-catalog-{index}");
        expect_ok(
            row.source_type,
            create_subscription(&router, REGION, &name, Some(row.source_type), &[]).await,
        );
        // An identifier of the subscription's own source type that names no stored
        // resource of it.
        let absent = format!("emu-absent-source-{index}");
        let answer = add_source(&router, &name, &absent).await;
        let expected: Vec<String> = if row.resolved {
            expect_error(row.source_type, answer, 404, "SourceNotFound");
            Vec::new()
        } else {
            let body = expect_ok(row.source_type, answer);
            assert!(
                texts(&body, "SourceId").contains(&absent),
                "case {:?}: expected the added source {absent} in the answer, got {body}",
                row.source_type
            );
            vec![absent.clone()]
        };
        let reported = described_source_ids(&router, row.source_type, &name).await;
        assert_eq!(
            reported, expected,
            "case {:?}: expected the subscription's stored sources to be {expected:?}, \
             got {reported:?}",
            row.source_type
        );
    }
}

/// An IAM role ARN of the form `CreateDBProxy$RoleArn` takes.
const PROXY_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/rds-proxy";

/// A VPC subnet id of the form `CreateDBProxy$VpcSubnetIds` takes.
const PROXY_SUBNET_ID: &str = "subnet-0a1b2c3d";

#[tokio::test]
async fn a_db_proxy_source_is_resolved_against_the_stored_proxies() {
    let router = emulator();
    let proxy = "emu-src-proxy-a";
    expect_ok(
        proxy,
        call(
            &router,
            "CreateDBProxy",
            &[
                ("DBProxyName", proxy),
                ("EngineFamily", "MYSQL"),
                ("RoleArn", PROXY_ROLE_ARN),
                ("VpcSubnetIds.member.1", PROXY_SUBNET_ID),
                ("Auth.member.1.UserName", "appuser"),
                ("Auth.member.1.AuthScheme", "SECRETS"),
            ],
        )
        .await,
    );

    let name = "emu-add-sub-proxy";
    expect_ok(
        "a db-proxy subscription",
        create_subscription(&router, REGION, name, Some("db-proxy"), &[]).await,
    );
    expect_error(
        "a DB proxy nothing created",
        add_source(&router, name, "emu-absent-proxy").await,
        404,
        "SourceNotFound",
    );
    let added = expect_ok("the stored DB proxy", add_source(&router, name, proxy).await);
    assert_eq!(
        texts(&added, "SourceId"),
        vec![proxy.to_string()],
        "expected the added proxy as the subscription's only source, got {added}"
    );
    let reported = described_source_ids(&router, "after the add", name).await;
    assert_eq!(
        reported,
        vec![proxy.to_string()],
        "expected the added proxy to be persisted, got {reported:?}"
    );
}

#[tokio::test]
async fn a_subscription_holds_a_bounded_number_of_sources() {
    let router = emulator();

    // One short of the bound, on a source type this service resolves nothing against, so
    // the row tests the COUNT rather than the resolution.
    let seeded: Vec<String> = (0..MAX_SOURCE_IDS_PER_SUBSCRIPTION - 1)
        .map(|index| format!("emu-bound-source-{index}"))
        .collect();
    let seeded_refs: Vec<&str> = seeded.iter().map(String::as_str).collect();
    let name = "emu-add-sub-bounded";
    expect_ok(
        "a subscription one source short of the bound",
        create_subscription(&router, REGION, name, Some("db-security-group"), &seeded_refs).await,
    );

    // The add that reaches the bound is served; the one past it is refused.
    expect_ok(
        "the add that reaches the bound",
        add_source(&router, name, "emu-bound-source-last").await,
    );
    expect_error(
        "the add past the bound",
        add_source(&router, name, "emu-bound-source-extra").await,
        400,
        "InvalidParameterValue",
    );
    let reported = described_source_ids(&router, "at the bound", name).await;
    assert_eq!(
        reported.len(),
        MAX_SOURCE_IDS_PER_SUBSCRIPTION,
        "expected the subscription to stop at {MAX_SOURCE_IDS_PER_SUBSCRIPTION} sources, \
         got {}",
        reported.len()
    );
}
