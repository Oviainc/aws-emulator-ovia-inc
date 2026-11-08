// SPDX-License-Identifier: CC0-1.0

//! What the AWS-managed table bucket's namespace keeps when a metadata configuration is taken
//! away before the tables it asked for ever came into service.
//!
//! The rule under test: **a metadata table's name belongs to the AWS-managed table bucket's
//! namespace for as long as a TABLE stands under it, and a table holds the name it was created
//! under from the instant its creation falls due — so taking the configuration away (deleting
//! it, or deleting the general purpose bucket carrying it) while a table of it is still being
//! created leaves no table behind for anyone to delete and frees that name, and the next create
//! provisions afresh under it, while a table that reached `ACTIVE` keeps its name and makes the
//! next create report the published `*AlreadyExists` provisioning failure.**
//!
//! That is the recovery path the published sentence asks a caller to take: "delete the journal
//! table, and then try again. To create a new S3 Metadata table, you must delete the S3
//! Metadata configuration for this bucket, and then create a new S3 Metadata configuration"
//! ([`METADATA_JOURNAL_TABLE_EXISTS_MESSAGE`]) is about a table that is really there — a
//! configuration torn down before its tables were provisioned leaves nothing to clear, so
//! delete-then-create is not a one-way door.
//!
//! The rows differ along what the rule generalises over: which tables the first configuration
//! asked for (the journal table alone, with an inventory table, with both optional tables),
//! whether those tables came into service before the configuration went away (which is the
//! boundary the rule turns on, so both sides of it are rows), how it went away (the
//! configuration deleted, deleted twice, or the general purpose bucket itself deleted and
//! created again with the configuration still on it), which bucket makes the second create (the
//! same one, or another whose name folds to the same table names) and which tables that second
//! create asks for (the same set, or one more).
//!
//! The codes and sentences are the published V2 provisioning failures
//! (`smithy_query(query="doc",shape="com.amazonaws.s3#ErrorDetails",member="ErrorCode",
//! model_path="services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json")`):
//! `JournalTableAlreadyExists`, `InventoryTableAlreadyExists` and `JournalTableNotAvailable`.
//!
//! Every assertion drives real HTTP requests through the router (the harness the
//! bucket-lifecycle suite owns), with the metadata suites' own senders and readers, so what is
//! pinned is the wire behaviour rather than any handler's internals.

use service_com_amazonaws_s3_amazons3::state::{
    METADATA_INVENTORY_TABLE_EXISTS_CODE, METADATA_INVENTORY_TABLE_EXISTS_MESSAGE,
    METADATA_JOURNAL_TABLE_EXISTS_CODE, METADATA_JOURNAL_TABLE_EXISTS_MESSAGE,
    METADATA_JOURNAL_TABLE_UNAVAILABLE_CODE, METADATA_JOURNAL_TABLE_UNAVAILABLE_MESSAGE,
    METADATA_TABLE_STATUS_ACTIVE, METADATA_TABLE_STATUS_CREATING, METADATA_TABLE_STATUS_FAILED,
};
use super::bucket_lifecycle::{create_bucket, router};
use super::bucket_metadata_configuration::{
    create_metadata_configuration, get_metadata_configuration, wait_for_the_tables,
};
use super::bucket_metadata_configuration_deletion::{
    delete_metadata_configuration, payload, reported,
};
use super::bucket_metadata_inventory_table_update::{update_inventory_table, updates};
use super::object_and_tagging_roundtrip::text;
use super::object_storage::delete_bucket;

/// The container a read reports each table of the configuration in.
const JOURNAL: &str = "JournalTableConfigurationResult";
/// See [`JOURNAL`].
const INVENTORY: &str = "InventoryTableConfigurationResult";
/// See [`JOURNAL`].
const ANNOTATION: &str = "AnnotationTableConfigurationResult";

/// How the first configuration is taken away before the second create.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Away {
    /// `DeleteBucketMetadataConfiguration` once.
    ConfigurationDeleted,
    /// The same delete twice: the second one is the idempotent no-op, which must free no more
    /// and no less than the first did.
    ConfigurationDeletedTwice,
    /// The general purpose bucket itself deleted — with the configuration still on it — and
    /// created again under the same name.
    BucketDeleted,
}

/// One row: the configuration that asks for the tables, whether they came into service, how it
/// is taken away, and the create that follows.
struct Row {
    /// What this row is, carried by every assertion.
    case: &'static str,
    /// The general purpose bucket the first configuration is created on.
    first_bucket: &'static str,
    /// Whether that configuration asks for an inventory table, and an annotation table.
    first: (bool, bool),
    /// Whether the row lets the tables fall due before taking the configuration away — the
    /// boundary the rule turns on.
    came_into_service: bool,
    /// How the configuration is taken away.
    away: Away,
    /// The bucket the second create names: the same one, or another whose name folds to the
    /// same table names (a dot where the first has a hyphen).
    second_bucket: &'static str,
    /// Whether the second create asks for an inventory table, and an annotation table.
    second: (bool, bool),
}

/// The rows, spanning the forms the rule generalises over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the journal table alone, deleted while it was being created",
            first_bucket: "unprovisioned-journal",
            first: (false, false),
            came_into_service: false,
            away: Away::ConfigurationDeleted,
            second_bucket: "unprovisioned-journal",
            second: (false, false),
        },
        Row {
            case: "a journal and an inventory table, deleted while they were being created",
            first_bucket: "unprovisioned-inventory",
            first: (true, false),
            came_into_service: false,
            away: Away::ConfigurationDeleted,
            second_bucket: "unprovisioned-inventory",
            second: (true, false),
        },
        Row {
            case: "every table of the configuration, after an idempotent second delete",
            first_bucket: "unprovisioned-annotation",
            first: (true, true),
            came_into_service: false,
            away: Away::ConfigurationDeletedTwice,
            second_bucket: "unprovisioned-annotation",
            second: (true, true),
        },
        Row {
            case: "the bucket itself deleted while its journal table was being created",
            first_bucket: "unprovisioned-bucket-cycle",
            first: (false, false),
            came_into_service: false,
            away: Away::BucketDeleted,
            second_bucket: "unprovisioned-bucket-cycle",
            second: (false, false),
        },
        Row {
            case: "another bucket whose name folds to the same table names",
            first_bucket: "unprovisioned-folded-name",
            first: (false, false),
            came_into_service: false,
            away: Away::ConfigurationDeleted,
            second_bucket: "unprovisioned.folded.name",
            second: (false, false),
        },
        Row {
            case: "a journal table alone, re-created asking for an inventory table too",
            first_bucket: "unprovisioned-added-inventory",
            first: (false, false),
            came_into_service: false,
            away: Away::ConfigurationDeleted,
            second_bucket: "unprovisioned-added-inventory",
            second: (true, false),
        },
        // The other side of the boundary: a table that reached service keeps its name, so the
        // same sequence reports the published failure for each name that is still held.
        Row {
            case: "a journal and an inventory table that came into service first",
            first_bucket: "provisioned-inventory",
            first: (true, false),
            came_into_service: true,
            away: Away::ConfigurationDeleted,
            second_bucket: "provisioned-inventory",
            second: (true, false),
        },
        Row {
            case: "a journal table that came into service, re-created with an inventory table",
            first_bucket: "provisioned-journal",
            first: (false, false),
            came_into_service: true,
            away: Away::ConfigurationDeleted,
            second_bucket: "provisioned-journal",
            second: (true, false),
        },
        Row {
            case: "the bucket itself deleted once its journal table was in service",
            first_bucket: "provisioned-bucket-cycle",
            first: (false, false),
            came_into_service: true,
            away: Away::BucketDeleted,
            second_bucket: "provisioned-bucket-cycle",
            second: (false, false),
        },
    ]
}

/// What a table of the second configuration must report: the published failure for the name it
/// could not be created under, or `None` for a table this really provisions.
type Expected = Option<(&'static str, &'static str)>;

/// The `TableStatus`, `ErrorCode` and `ErrorMessage` the table reported in `container` must
/// carry: `expected` names a provisioning failure, or `None` for a table that is created and
/// reaches service — `CREATING` straight after the call and `ACTIVE` once the span has passed.
fn assert_table(case: &str, body: &str, container: &str, expected: Expected, due: bool) {
    let (status, code, message) = reported(body, container);
    match expected {
        Some((expected_code, expected_message)) => {
            assert_eq!(
                status.as_deref(),
                Some(METADATA_TABLE_STATUS_FAILED),
                "case {case:?}: {container} must report {METADATA_TABLE_STATUS_FAILED}, got \
                 {body:?}"
            );
            assert_eq!(
                code.as_deref(),
                Some(expected_code),
                "case {case:?}: {container} must report the {expected_code} error code, got \
                 {code:?}"
            );
            assert_eq!(
                message.as_deref(),
                Some(expected_message),
                "case {case:?}: {container} must report the published sentence, got {message:?}"
            );
        }
        None => {
            let expected_status = match due {
                false => METADATA_TABLE_STATUS_CREATING,
                true => METADATA_TABLE_STATUS_ACTIVE,
            };
            assert_eq!(
                status.as_deref(),
                Some(expected_status),
                "case {case:?}: {container} must report {expected_status}, got {body:?}"
            );
            assert_eq!(
                code, None,
                "case {case:?}: {container} is a table that was created, so it reports no error \
                 code, got {code:?}"
            );
        }
    }
}

/// Assert that `container` is not reported at all — a create that named no such table.
fn assert_no_table(case: &str, body: &str, container: &str) {
    let (status, _, _) = reported(body, container);
    assert_eq!(
        status, None,
        "case {case:?}: a create naming no {container} reports none, got {body:?}"
    );
}

#[tokio::test]
async fn a_configuration_taken_away_before_its_tables_came_into_service_frees_their_names() {
    let router = router();

    for row in rows() {
        let Row {
            case,
            first_bucket,
            first,
            came_into_service,
            away,
            second_bucket,
            second,
        } = row;

        let created = create_bucket(&router, first_bucket, None, None).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the bucket must be created, got {} with body {:?}",
            created.status,
            text(&created)
        );
        let configured =
            create_metadata_configuration(&router, first_bucket, &[], &payload(first.0, first.1))
                .await;
        assert_eq!(
            configured.status, 200,
            "case {case:?}: the first create must be accepted, got {} with body {:?}",
            configured.status,
            text(&configured)
        );

        // Which side of the boundary this row acts on: the tables left to fall due, or taken
        // away while their creation was still under way.
        if came_into_service {
            wait_for_the_tables();
        }
        let body = text(&get_metadata_configuration(&router, first_bucket, &[]).await);
        let expected_status = match came_into_service {
            false => METADATA_TABLE_STATUS_CREATING,
            true => METADATA_TABLE_STATUS_ACTIVE,
        };
        assert_eq!(
            reported(&body, JOURNAL).0.as_deref(),
            Some(expected_status),
            "case {case:?}: the first journal table must report {expected_status} before the \
             configuration is taken away, got {body:?}"
        );

        match away {
            Away::ConfigurationDeleted | Away::ConfigurationDeletedTwice => {
                let deletes = match away {
                    Away::ConfigurationDeletedTwice => 2,
                    _ => 1,
                };
                for attempt in 1..=deletes {
                    let deleted = delete_metadata_configuration(&router, first_bucket, &[]).await;
                    assert_eq!(
                        deleted.status, 204,
                        "case {case:?}: delete {attempt} must answer the modelled 204, got {} \
                         with body {:?}",
                        deleted.status,
                        text(&deleted)
                    );
                }
            }
            Away::BucketDeleted => {
                // The configuration is still on the bucket: its whole life goes with the
                // bucket, tables included.
                let dropped = delete_bucket(&router, first_bucket).await;
                assert_eq!(
                    dropped.status, 204,
                    "case {case:?}: the bucket delete must answer 204, got {} with body {:?}",
                    dropped.status,
                    text(&dropped)
                );
                let remade = create_bucket(&router, first_bucket, None, None).await;
                assert_eq!(
                    remade.status, 200,
                    "case {case:?}: the bucket must be created again, got {} with body {:?}",
                    remade.status,
                    text(&remade)
                );
            }
        }

        // A row whose second create comes from ANOTHER bucket needs that bucket first.
        if second_bucket != first_bucket {
            let sibling = create_bucket(&router, second_bucket, None, None).await;
            assert_eq!(
                sibling.status, 200,
                "case {case:?}: the second bucket must be created, got {} with body {:?}",
                sibling.status,
                text(&sibling)
            );
        }

        let recreated =
            create_metadata_configuration(&router, second_bucket, &[], &payload(second.0, second.1))
                .await;
        assert_eq!(
            recreated.status, 200,
            "case {case:?}: the second create must be accepted, got {} with body {:?}",
            recreated.status,
            text(&recreated)
        );

        // What each table of the second configuration reports follows from which names a table
        // still stands under: only a table that came into service kept its own, and an
        // inventory table whose own name is free reports the journal table it relies on when
        // that one could not be created.
        let journal: Expected = came_into_service.then_some((
            METADATA_JOURNAL_TABLE_EXISTS_CODE,
            METADATA_JOURNAL_TABLE_EXISTS_MESSAGE,
        ));
        let inventory: Expected = match (came_into_service, first.0) {
            (true, true) => Some((
                METADATA_INVENTORY_TABLE_EXISTS_CODE,
                METADATA_INVENTORY_TABLE_EXISTS_MESSAGE,
            )),
            (true, false) => Some((
                METADATA_JOURNAL_TABLE_UNAVAILABLE_CODE,
                METADATA_JOURNAL_TABLE_UNAVAILABLE_MESSAGE,
            )),
            (false, _) => None,
        };

        // Read straight after the create and again once the activation span has passed: a
        // table that was really provisioned moves from CREATING to ACTIVE, and one that could
        // not be created reports its failure at both instants, since nothing is going to
        // create it.
        for due in [false, true] {
            if due {
                wait_for_the_tables();
            }
            let body = text(&get_metadata_configuration(&router, second_bucket, &[]).await);
            assert_table(case, &body, JOURNAL, journal, due);
            match second.0 {
                false => assert_no_table(case, &body, INVENTORY),
                true => assert_table(case, &body, INVENTORY, inventory, due),
            }
            match second.1 {
                false => assert_no_table(case, &body, ANNOTATION),
                // No provisioning failure is published for an annotation table, so it is
                // created whatever the other two could do.
                true => assert_table(case, &body, ANNOTATION, None, due),
            }
        }
    }
}

#[tokio::test]
async fn an_inventory_table_disabled_before_it_came_into_service_frees_its_name() {
    let router = router();

    // The same boundary, for the one table an update of a live configuration provisions and
    // drops: disabling it while its creation is still under way leaves nothing behind, so
    // enabling it again provisions afresh; disabling one that reached service does not delete
    // the table, so the name stays taken.
    for (case, bucket, came_into_service) in [
        (
            "disabled while it was still being created",
            "unprovisioned-inventory-disable",
            false,
        ),
        (
            "disabled once it was in service",
            "provisioned-inventory-disable",
            true,
        ),
    ] {
        let created = create_bucket(&router, bucket, None, None).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the bucket must be created, got {} with body {:?}",
            created.status,
            text(&created)
        );
        let configured =
            create_metadata_configuration(&router, bucket, &[], &payload(false, false)).await;
        assert_eq!(
            configured.status, 200,
            "case {case:?}: the create must be accepted, got {} with body {:?}",
            configured.status,
            text(&configured)
        );
        // A table of a configuration may be reconfigured only once that configuration's
        // journal table is in service, so the row waits for it exactly as a caller must.
        wait_for_the_tables();

        for (step, state) in [(1, "ENABLED"), (2, "DISABLED"), (3, "ENABLED")] {
            if step == 2 && came_into_service {
                // This row lets the inventory table the enable provisioned fall due before it
                // is disabled.
                wait_for_the_tables();
            }
            let answer =
                update_inventory_table(&router, bucket, &[], &updates(state, None, false)).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: the update naming {state:?} must answer the modelled 200, got {} \
                 with body {:?}",
                answer.status,
                text(&answer)
            );
        }

        let expected: Expected = came_into_service.then_some((
            METADATA_INVENTORY_TABLE_EXISTS_CODE,
            METADATA_INVENTORY_TABLE_EXISTS_MESSAGE,
        ));
        for due in [false, true] {
            if due {
                wait_for_the_tables();
            }
            let body = text(&get_metadata_configuration(&router, bucket, &[]).await);
            assert_table(case, &body, INVENTORY, expected, due);
            // The journal table is untouched by any of this: it came into service and stayed
            // there.
            assert_eq!(
                reported(&body, JOURNAL).0.as_deref(),
                Some(METADATA_TABLE_STATUS_ACTIVE),
                "case {case:?}: the journal table must still report \
                 {METADATA_TABLE_STATUS_ACTIVE}, got {body:?}"
            );
        }
    }
}
