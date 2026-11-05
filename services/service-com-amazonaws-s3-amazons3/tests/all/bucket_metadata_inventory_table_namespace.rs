// SPDX-License-Identifier: CC0-1.0

//! The AWS-managed table bucket's namespace and the inventory table that
//! `UpdateBucketMetadataInventoryTableConfiguration` provisions in it.
//!
//! The rule under test: **a metadata table OUTLIVES the configuration that provisioned it,
//! whichever operation provisioned it, and no S3 operation on a general purpose bucket takes a
//! name out of the AWS-managed table bucket's namespace — so a table provisioned under a name
//! that namespace already holds is not created: the call is accepted and a reader reports that
//! table `FAILED` with the published failure for the name being taken, its OWN name judged
//! before the journal table it relies on.**
//!
//! The update is in that rule because it really does provision a table:
//! `s3tables:CreateTable` is among the permissions the operation requires, and none of the
//! permissions it lists deletes a table
//! (`smithy_query(query="doc",shape="UpdateBucketMetadataInventoryTableConfiguration",
//! model_path="services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json")`).
//! So enabling the live inventory table takes its name in the managed namespace exactly as a
//! create does, and disabling it or deleting the whole configuration gives that name back to
//! nobody.
//!
//! The codes and sentences are the published V2 provisioning failures
//! (`smithy_query(query="doc",shape="com.amazonaws.s3#ErrorDetails",member="ErrorCode")`):
//! `InventoryTableAlreadyExists` for the inventory table's own name being held, and
//! `JournalTableNotAvailable` — "an inventory table requires a journal table with an `ACTIVE`
//! status" — for one whose own name is free but whose journal table could not be created.
//!
//! The rows differ along what the rule generalises over: which operation provisioned the name
//! first (the create naming an enabled inventory table, or the update enabling one), what
//! happened in between (the configuration deleted, or the table disabled, or nothing at all),
//! which bucket provisions the second table (the same one, or another whose name folds to the
//! same table name), which operation does it (a create naming the table, or the update
//! enabling it), and whether the name it would take is held at all — the last row being the
//! control, where nothing holds the name and the update's table reaches service.
//!
//! Every assertion drives real HTTP requests through the router (the harness the
//! bucket-lifecycle suite owns), with the metadata suites' own senders and readers, so what is
//! pinned is the wire behaviour rather than any handler's internals.

use service_com_amazonaws_s3_amazons3::state::{
    METADATA_INVENTORY_TABLE_EXISTS_CODE, METADATA_INVENTORY_TABLE_EXISTS_MESSAGE,
    METADATA_JOURNAL_TABLE_UNAVAILABLE_CODE, METADATA_JOURNAL_TABLE_UNAVAILABLE_MESSAGE,
    METADATA_TABLE_STATUS_ACTIVE, METADATA_TABLE_STATUS_CREATING, METADATA_TABLE_STATUS_FAILED,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router};
use super::bucket_metadata_configuration::{
    create_metadata_configuration, get_metadata_configuration, inside, text as element,
    wait_for_the_tables,
};
use super::bucket_metadata_configuration_deletion::{
    delete_metadata_configuration, payload, reported,
};
use super::bucket_metadata_inventory_table_update::{update_inventory_table, updates};
use super::object_and_tagging_roundtrip::text;

/// The container a read reports the inventory table in.
const INVENTORY: &str = "InventoryTableConfigurationResult";

/// Which operation provisions the inventory table's name first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum First {
    /// `CreateBucketMetadataConfiguration` naming an enabled inventory table.
    CreateEnables,
    /// `UpdateBucketMetadataInventoryTableConfiguration` enabling one the create named none
    /// of.
    UpdateEnables,
    /// Nothing: the first configuration has a journal table only, so no inventory table name
    /// is ever taken.
    Nothing,
}

/// What happens to that first configuration before the second table is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Between {
    /// `DeleteBucketMetadataConfiguration`.
    ConfigurationDeleted,
    /// The inventory table disabled, which leaves the configuration reporting no table.
    TableDisabled,
    /// Nothing at all: the first configuration is still there, in service.
    Nothing,
}

/// Which operation asks for the second inventory table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Second {
    /// A create naming an enabled inventory table.
    CreateEnables,
    /// The update enabling one.
    UpdateEnables,
}

/// One row of the table.
struct Row {
    /// What this row is, carried by every assertion.
    case: &'static str,
    /// The general purpose bucket whose configuration comes first.
    first_bucket: &'static str,
    /// Which operation provisions the inventory table name on it.
    first: First,
    /// What happens before the second table is asked for.
    between: Between,
    /// The bucket that asks for the second table: the same one, or another whose name folds to
    /// the same table names.
    second_bucket: &'static str,
    /// Which operation asks for it.
    second: Second,
    /// The code and sentence that second inventory table must report, or `None` for a row
    /// where nothing holds the name and the table reaches service.
    expected: Option<(&'static str, &'static str)>,
}

/// The published failure for an inventory table's own name being held.
const NAME_TAKEN: Option<(&str, &str)> = Some((
    METADATA_INVENTORY_TABLE_EXISTS_CODE,
    METADATA_INVENTORY_TABLE_EXISTS_MESSAGE,
));

/// Every row, spanning the forms the rule generalises over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the update provisioned it, the configuration was deleted, a create names it",
            first_bucket: "inventory-ns-update-then-create",
            first: First::UpdateEnables,
            between: Between::ConfigurationDeleted,
            second_bucket: "inventory-ns-update-then-create",
            second: Second::CreateEnables,
            expected: NAME_TAKEN,
        },
        Row {
            case: "the update provisioned it, the configuration was deleted, the update again",
            first_bucket: "inventory-ns-update-then-update",
            first: First::UpdateEnables,
            between: Between::ConfigurationDeleted,
            second_bucket: "inventory-ns-update-then-update",
            second: Second::UpdateEnables,
            expected: NAME_TAKEN,
        },
        Row {
            case: "the create provisioned it, the table was disabled, the update enables it",
            first_bucket: "inventory-ns-create-then-disable",
            first: First::CreateEnables,
            between: Between::TableDisabled,
            second_bucket: "inventory-ns-create-then-disable",
            second: Second::UpdateEnables,
            expected: NAME_TAKEN,
        },
        Row {
            case: "the update provisioned it, the table was disabled, the update enables it",
            first_bucket: "inventory-ns-update-then-disable",
            first: First::UpdateEnables,
            between: Between::TableDisabled,
            second_bucket: "inventory-ns-update-then-disable",
            second: Second::UpdateEnables,
            expected: NAME_TAKEN,
        },
        Row {
            case: "another bucket whose name folds to the same table name enables it",
            first_bucket: "inventory-ns-folded-name",
            first: First::CreateEnables,
            between: Between::Nothing,
            second_bucket: "inventory.ns.folded.name",
            second: Second::UpdateEnables,
            expected: NAME_TAKEN,
        },
        Row {
            // The inventory table's own name is free here — no inventory table was ever
            // provisioned — so what it reports is the journal table it relies on, whose own
            // name the deleted configuration's journal table still holds.
            case: "an inventory name nothing holds, on a configuration whose journal failed",
            first_bucket: "inventory-ns-journal-failed",
            first: First::Nothing,
            between: Between::ConfigurationDeleted,
            second_bucket: "inventory-ns-journal-failed",
            second: Second::UpdateEnables,
            expected: Some((
                METADATA_JOURNAL_TABLE_UNAVAILABLE_CODE,
                METADATA_JOURNAL_TABLE_UNAVAILABLE_MESSAGE,
            )),
        },
        Row {
            // The control: nothing holds the name and the journal table is in service, so the
            // update provisions its table and that table reaches service.
            case: "an inventory name nothing holds, on a healthy configuration",
            first_bucket: "inventory-ns-free-name",
            first: First::Nothing,
            between: Between::Nothing,
            second_bucket: "inventory-ns-free-name",
            second: Second::UpdateEnables,
            expected: None,
        },
    ]
}

/// `PUT /<bucket>?metadataInventoryTable` with `state`, asserting the modelled 200 the
/// operation answers whatever its table could do.
async fn set_inventory_state(router: &MultiServiceRouter, bucket: &str, case: &str, state: &str) {
    let answer = update_inventory_table(router, bucket, &[], &updates(state, None, false)).await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: the update naming {state:?} must answer the modelled 200, got {} with \
         body {:?}",
        answer.status,
        text(&answer)
    );
}

/// A general purpose bucket carrying a metadata configuration whose inventory table
/// configuration is named exactly when `inventory` asks for it.
async fn configuration_on(
    router: &MultiServiceRouter,
    bucket: &str,
    case: &str,
    inventory: bool,
) {
    // A row that re-creates a configuration on the bucket it already made asks for that
    // bucket again, which the legacy 200 answers and which is `BucketAlreadyOwnedByYou`
    // outside North Virginia: either way the bucket this row needs is there.
    let created = create_bucket(router, bucket, None, None).await;
    assert!(
        created.status == 200 || created.error_code() == "BucketAlreadyOwnedByYou",
        "case {case:?}: the bucket {bucket:?} must be there for this row, got {} with body {:?}",
        created.status,
        text(&created)
    );
    let configured =
        create_metadata_configuration(router, bucket, &[], &payload(inventory, false)).await;
    assert_eq!(
        configured.status,
        200,
        "case {case:?}: the configuration on {bucket:?} must be created, got {} with body {:?}",
        configured.status,
        text(&configured)
    );
}

#[tokio::test]
async fn an_inventory_table_the_update_provisioned_keeps_its_name_in_the_managed_namespace() {
    let router = router();

    for row in rows() {
        let Row {
            case,
            first_bucket,
            first,
            between,
            second_bucket,
            second,
            expected,
        } = row;

        // The configuration that takes the name, through whichever operation this row names.
        configuration_on(&router, first_bucket, case, first == First::CreateEnables).await;
        if first == First::UpdateEnables {
            // A table of a configuration may be reconfigured only once that configuration's
            // journal table is in service, so the row waits for it exactly as a caller must.
            wait_for_the_tables();
            set_inventory_state(&router, first_bucket, case, "ENABLED").await;
        }
        // Its tables reach service first, so the row acts on the state the API asks a caller
        // to wait for — and a table provisioned under a FREE name really does reach it.
        wait_for_the_tables();
        let body = text(&get_metadata_configuration(&router, first_bucket, &[]).await);
        if first != First::Nothing {
            let (status, code, _) = reported(&body, INVENTORY);
            assert_eq!(
                status.as_deref(),
                Some(METADATA_TABLE_STATUS_ACTIVE),
                "case {case:?}: the first inventory table must reach \
                 {METADATA_TABLE_STATUS_ACTIVE}, got {body:?}"
            );
            assert_eq!(
                code, None,
                "case {case:?}: a table that was created reports no error code, got {code:?}"
            );
        }

        match between {
            Between::ConfigurationDeleted => {
                let deleted = delete_metadata_configuration(&router, first_bucket, &[]).await;
                assert_eq!(
                    deleted.status,
                    204,
                    "case {case:?}: the configuration delete must answer 204, got {} with body \
                     {:?}",
                    deleted.status,
                    text(&deleted)
                );
            }
            Between::TableDisabled => {
                set_inventory_state(&router, first_bucket, case, "DISABLED").await;
                let body = text(&get_metadata_configuration(&router, first_bucket, &[]).await);
                let (status, _, _) = reported(&body, INVENTORY);
                assert_eq!(
                    status, None,
                    "case {case:?}: a disabled inventory table is reported with no table behind \
                     it, got {body:?}"
                );
            }
            Between::Nothing => {}
        }

        // A row whose second table is asked for by ANOTHER bucket needs that bucket, and a
        // configuration on it; so does a row whose configuration was just deleted.
        let needs_a_configuration =
            second_bucket != first_bucket || between == Between::ConfigurationDeleted;
        match second {
            Second::CreateEnables => {
                assert!(
                    needs_a_configuration,
                    "case {case:?}: a create can only name a bucket carrying no configuration"
                );
                configuration_on(&router, second_bucket, case, true).await;
            }
            Second::UpdateEnables => {
                if needs_a_configuration {
                    configuration_on(&router, second_bucket, case, false).await;
                    // A table of a configuration may be reconfigured only once that
                    // configuration's journal table is in service, so the row waits for the
                    // configuration it has just created exactly as a caller must.
                    wait_for_the_tables();
                }
                set_inventory_state(&router, second_bucket, case, "ENABLED").await;
            }
        }

        // What the second inventory table reports: the published failure for the name it
        // could not be created under, or service for the row where nothing holds that name.
        // Read before and after the span a creation would take, since a table nothing is
        // going to create never falls due.
        for (wait, when) in [
            (false, "straight after the call"),
            (true, "after the activation span"),
        ] {
            if wait {
                wait_for_the_tables();
            }
            let body = text(&get_metadata_configuration(&router, second_bucket, &[]).await);
            let container = inside(&body, INVENTORY).unwrap_or_else(|| {
                panic!("case {case:?} ({when}): the inventory table must be reported, got {body:?}")
            });
            // Whatever its table could do, the state the request named is the one the
            // configuration carries.
            assert_eq!(
                element(container, "ConfigurationState").as_deref(),
                Some("ENABLED"),
                "case {case:?} ({when}): expected the enabled state, got {container:?}"
            );
            let (status, code, message) = reported(&body, INVENTORY);
            match expected {
                None => {
                    let expected_status = match wait {
                        false => METADATA_TABLE_STATUS_CREATING,
                        true => METADATA_TABLE_STATUS_ACTIVE,
                    };
                    assert_eq!(
                        status.as_deref(),
                        Some(expected_status),
                        "case {case:?} ({when}): a table under a free name must report \
                         {expected_status}, got {body:?}"
                    );
                    assert_eq!(
                        code, None,
                        "case {case:?} ({when}): a table that was created reports no error \
                         code, got {code:?}"
                    );
                }
                Some((expected_code, expected_message)) => {
                    assert_eq!(
                        status.as_deref(),
                        Some(METADATA_TABLE_STATUS_FAILED),
                        "case {case:?} ({when}): the inventory table must report \
                         {METADATA_TABLE_STATUS_FAILED}, got {body:?}"
                    );
                    assert_eq!(
                        code.as_deref(),
                        Some(expected_code),
                        "case {case:?} ({when}): expected the {expected_code} error code, got \
                         {code:?}"
                    );
                    assert_eq!(
                        message.as_deref(),
                        Some(expected_message),
                        "case {case:?} ({when}): expected the published sentence, got \
                         {message:?}"
                    );
                }
            }
        }
    }
}
