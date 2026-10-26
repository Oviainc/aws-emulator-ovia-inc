// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how this service WORDS the event-source not-found fault.
//!
//! The rule under test: when a request names an event source identifier that resolves to
//! no existing resource of the subscription's source type, RDS refuses it with
//! `SourceNotFoundFault` — HTTP 404, awsQuery code `SourceNotFound`
//! (`smithy_query(query="operation", operation="AddSourceIdentifierToSubscription,
//! CreateEventSubscription")`) — whose message is the FIXED text `Could not find source :`
//! followed by the identifier the request supplied, carried verbatim. Nothing else is
//! interpolated: not the source type the identifier was resolved against, not the region
//! the call was signed for, not the subscription's name, and no trailing period.
//!
//! The tables below span the classes the rule generalises over: the operation that resolves
//! the identifier (the add and the create), the subscription's source type (every type this
//! service resolves an identifier against, including ones the issue cited none of), the
//! SHAPE of the identifier that names nothing (a plain name, a single letter, mixed case,
//! digits, a near miss of a stored identifier, an identifier of a DIFFERENT stored type,
//! one at the published 255-character bound), the region the call is signed for, and — for
//! the create — the position of the offending entry in a multi-entry `SourceIds` list.
//!
//! Every call goes over the wire through the harness in `db_instances.rs`, so the
//! assertions read the XML the emulator itself answered with.

use service_com_amazonaws_rds_amazonrdsv19::state::MAX_SOURCE_ID_LEN;

use crate::db_instances::{call, call_in, emulator, expect_error, expect_ok, text, texts, REGION};
use crate::event_subscription_sources::{
    add_source, add_source_in, create_subscription, described_source_ids, seed_sources,
    OTHER_REGION, SEEDED_CLUSTER, SEEDED_PARAMETER_GROUP, SEEDED_SNAPSHOT,
};

/// The message the fault carries for `source_identifier`: the fixed text, then the
/// identifier the request named.
fn wording(source_identifier: &str) -> String {
    format!("Could not find source :{source_identifier}")
}

/// A source identifier of exactly the 255 characters its member admits
/// (`MAX_SOURCE_ID_LEN`, the bound `DBInstanceIdentifier`/`DBSnapshotIdentifier` publish),
/// so the published bound is one of the rows.
fn identifier_at_the_published_bound() -> String {
    let mut identifier = String::from("e");
    identifier.extend(std::iter::repeat_n('a', MAX_SOURCE_ID_LEN - 1));
    assert_eq!(
        identifier.chars().count(),
        MAX_SOURCE_ID_LEN,
        "the row at the published bound must be {MAX_SOURCE_ID_LEN} characters, got {}",
        identifier.chars().count()
    );
    identifier
}

/// Assert one refusal: the status, the code, and the message the rule requires — plus that
/// the message interpolates none of the values AWS's text does not carry.
fn assert_wording(case: &str, response: (u16, String), source_identifier: &str, region: &str) {
    let body = expect_error(case, response, 404, "SourceNotFound");
    let message = text(&body, "Message");
    assert_eq!(
        message,
        wording(source_identifier),
        "case {case:?}: expected the fixed text and the requested identifier, got {message:?}"
    );
    assert!(
        !message.contains(region),
        "case {case:?}: expected no region in the message, got {message:?}"
    );
    assert!(
        !message.ends_with('.'),
        "case {case:?}: expected no trailing period, got {message:?}"
    );
}

/// One row of the add table: the subscription's source type, and an identifier of that type
/// that names no stored resource.
struct AddCase {
    case: &'static str,
    source_type: &'static str,
    absent_identifier: &'static str,
}

/// The rows differ along the source type the identifier is resolved against and the shape
/// of the identifier that resolves to nothing — including `db-cluster-snapshot` and
/// `blue-green-deployment`, types the issue cited none of.
const ADD_CASES: &[AddCase] = &[
    AddCase {
        case: "a db-instance identifier nothing created",
        source_type: "db-instance",
        absent_identifier: "emu-wording-absent-instance",
    },
    AddCase {
        case: "a db-instance identifier of a single letter",
        source_type: "db-instance",
        absent_identifier: "e",
    },
    AddCase {
        case: "a db-instance identifier in mixed case",
        source_type: "db-instance",
        absent_identifier: "EMU-Wording-Absent-Instance",
    },
    AddCase {
        case: "a db-instance identifier of letters and digits",
        source_type: "db-instance",
        absent_identifier: "emu9wording0absent1instance2",
    },
    AddCase {
        case: "a db-parameter-group name nothing created",
        source_type: "db-parameter-group",
        absent_identifier: "emu-wording-absent-group",
    },
    AddCase {
        case: "a near miss of the stored parameter group's name",
        source_type: "db-parameter-group",
        absent_identifier: "emu-src-parameter-group-b",
    },
    AddCase {
        case: "a db-cluster identifier nothing created",
        source_type: "db-cluster",
        absent_identifier: "emu-wording-absent-cluster",
    },
    AddCase {
        case: "a db-snapshot identifier nothing created",
        source_type: "db-snapshot",
        absent_identifier: "emu-wording-absent-snapshot",
    },
    AddCase {
        case: "a db-cluster-snapshot identifier nothing created",
        source_type: "db-cluster-snapshot",
        absent_identifier: "emu-wording-absent-cluster-snapshot",
    },
    AddCase {
        case: "a db-proxy name nothing created",
        source_type: "db-proxy",
        absent_identifier: "emu-wording-absent-proxy",
    },
    AddCase {
        case: "a zero-etl integration name nothing created",
        source_type: "zero-etl",
        absent_identifier: "emu-wording-absent-integration",
    },
    AddCase {
        case: "a blue-green-deployment identifier nothing created",
        source_type: "blue-green-deployment",
        absent_identifier: "emu-wording-absent-deployment",
    },
    // A resource that DOES exist, but of another source type than the subscription's: it
    // resolves to no source of THIS subscription, and the message still names only the
    // identifier the request carried.
    AddCase {
        case: "a stored snapshot's identifier on a db-instance subscription",
        source_type: "db-instance",
        absent_identifier: SEEDED_SNAPSHOT,
    },
    AddCase {
        case: "a stored cluster's identifier on a db-snapshot subscription",
        source_type: "db-snapshot",
        absent_identifier: SEEDED_CLUSTER,
    },
    AddCase {
        case: "a stored parameter group's name on a db-cluster subscription",
        source_type: "db-cluster",
        absent_identifier: SEEDED_PARAMETER_GROUP,
    },
];

#[tokio::test]
async fn the_add_words_the_fault_with_the_fixed_text_and_the_requested_identifier() {
    let router = emulator();
    seed_sources(&router).await;

    for (index, row) in ADD_CASES.iter().enumerate() {
        let name = format!("emu-wording-sub-{index}");
        expect_ok(
            row.case,
            create_subscription(&router, REGION, &name, Some(row.source_type), &[]).await,
        );
        assert_wording(
            row.case,
            add_source(&router, &name, row.absent_identifier).await,
            row.absent_identifier,
            REGION,
        );
        // The refused add changed nothing, which is what makes the message the only
        // observable of the refusal.
        let reported = described_source_ids(&router, row.case, &name).await;
        assert!(
            reported.is_empty(),
            "case {:?}: expected the refused add to store no source, got {reported:?}",
            row.case
        );
    }

    // The identifier at the published length bound, and the subscription name's region, are
    // rows of their own: the message is the same template for both.
    let at_the_bound = identifier_at_the_published_bound();
    let name = "emu-wording-sub-bound";
    expect_ok(
        "a db-instance subscription for the bound rows",
        create_subscription(&router, REGION, name, Some("db-instance"), &[]).await,
    );
    assert_wording(
        "an identifier at the published 255-character bound",
        add_source(&router, name, &at_the_bound).await,
        &at_the_bound,
        REGION,
    );

    expect_ok(
        "a db-instance subscription in another region",
        create_subscription(&router, OTHER_REGION, name, Some("db-instance"), &[]).await,
    );
    assert_wording(
        "a call signed for another region",
        add_source_in(&router, OTHER_REGION, name, "emu-wording-absent-instance").await,
        "emu-wording-absent-instance",
        OTHER_REGION,
    );
}

/// One row of the create table: the `SourceIds` the create carries, and the entry it is
/// refused for — the FIRST one that resolves to nothing.
struct CreateCase {
    case: &'static str,
    source_type: &'static str,
    source_ids: &'static [&'static str],
    offending: &'static str,
}

/// The rows differ along the source type, how many `SourceIds` entries the create carries
/// and where in that list the offending entry sits.
const CREATE_CASES: &[CreateCase] = &[
    CreateCase {
        case: "one entry, which names nothing",
        source_type: "db-instance",
        source_ids: &["emu-wording-create-absent"],
        offending: "emu-wording-create-absent",
    },
    CreateCase {
        case: "a stored instance first, the absent entry second",
        source_type: "db-instance",
        source_ids: &["emu-src-instance-a", "emu-wording-create-absent-2"],
        offending: "emu-wording-create-absent-2",
    },
    CreateCase {
        case: "the absent entry first, a stored instance second",
        source_type: "db-instance",
        source_ids: &["emu-wording-create-absent-3", "emu-src-instance-a"],
        offending: "emu-wording-create-absent-3",
    },
    CreateCase {
        case: "a parameter group name nothing created",
        source_type: "db-parameter-group",
        source_ids: &["emu-wording-create-absent-group"],
        offending: "emu-wording-create-absent-group",
    },
    CreateCase {
        case: "a stored snapshot's identifier on a db-cluster subscription",
        source_type: "db-cluster",
        source_ids: &[SEEDED_SNAPSHOT],
        offending: SEEDED_SNAPSHOT,
    },
];

#[tokio::test]
async fn the_create_words_the_same_fault_the_same_way() {
    let router = emulator();
    seed_sources(&router).await;

    for (index, row) in CREATE_CASES.iter().enumerate() {
        let name = format!("emu-wording-create-{index}");
        assert_wording(
            row.case,
            create_subscription(&router, REGION, &name, Some(row.source_type), row.source_ids)
                .await,
            row.offending,
            REGION,
        );
        // The refused create stored no subscription at all.
        expect_error(
            row.case,
            call(
                &router,
                "DescribeEventSubscriptions",
                &[("SubscriptionName", name.as_str())],
            )
            .await,
            404,
            "SubscriptionNotFound",
        );
    }

    // And the create that names only stored sources is served, so the rows above are
    // refusals of the identifiers and not of the operation.
    let name = "emu-wording-create-served";
    let created = expect_ok(
        "a create naming only stored sources",
        create_subscription(
            &router,
            REGION,
            name,
            Some("db-instance"),
            &["emu-src-instance-a"],
        )
        .await,
    );
    assert_eq!(
        texts(&created, "SourceId"),
        vec!["emu-src-instance-a".to_string()],
        "expected the stored source the create named, got {created}"
    );
}

#[tokio::test]
async fn the_fault_is_worded_the_same_whichever_operation_answers_it() {
    let router = emulator();
    seed_sources(&router).await;

    // One identifier, one source type, two paths that resolve it: the add against a stored
    // subscription and the create of a new one. A caller matching on the text cannot tell
    // which operation answered.
    let absent = "emu-wording-shared-absent";
    let name = "emu-wording-shared";
    expect_ok(
        "the subscription the add resolves against",
        create_subscription(&router, REGION, name, Some("db-parameter-group"), &[]).await,
    );
    let added = expect_error(
        "the add",
        add_source(&router, name, absent).await,
        404,
        "SourceNotFound",
    );
    let created = expect_error(
        "the create",
        create_subscription(
            &router,
            REGION,
            "emu-wording-shared-2",
            Some("db-parameter-group"),
            &[absent],
        )
        .await,
        404,
        "SourceNotFound",
    );
    let (added, created) = (text(&added, "Message"), text(&created, "Message"));
    assert_eq!(
        added, created,
        "expected both paths to word the one fault identically, got {added:?} and {created:?}"
    );
    assert_eq!(
        added,
        wording(absent),
        "expected the fixed text and the requested identifier, got {added:?}"
    );

    // The same identifier added to a source type this service holds no records of is not
    // refused at all: there is nothing to resolve it against, so the wording rule has no
    // call to apply to.
    let unresolved = "emu-wording-unresolved";
    expect_ok(
        "a db-security-group subscription",
        create_subscription(&router, REGION, unresolved, Some("db-security-group"), &[]).await,
    );
    let served = expect_ok(
        "an identifier of a type this service resolves nothing against",
        call_in(
            &router,
            REGION,
            "AddSourceIdentifierToSubscription",
            &[
                ("SubscriptionName", unresolved),
                ("SourceIdentifier", absent),
            ],
        )
        .await,
    );
    assert!(
        texts(&served, "SourceId").contains(&absent.to_string()),
        "expected the identifier to be stored for a type nothing resolves it, got {served}"
    );
}
