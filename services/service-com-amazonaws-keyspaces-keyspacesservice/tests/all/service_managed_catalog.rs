// SPDX-License-Identifier: CC0-1.0

//! The service-managed (Cassandra system) keyspaces, driven end to end over the service's
//! own `awsJson1_0` wire protocol through the harness in
//! [`crate::keyspace_table_lifecycle`].
//!
//! The rule under test, over the WHOLE set of service-managed keyspaces and their system
//! tables rather than one example: Amazon Keyspaces holds the service-managed keyspaces
//! and their system tables in every account and Region before any client call, so every
//! reporting operation resolves and reports them exactly as it does a created resource —
//! and, because they are the SERVICE's resources, every operation that would create,
//! change or delete one inside them is refused with the modelled `ValidationException`
//! rather than a 404 or a silent mutation.
//!
//! Concretely:
//!
//! 1. `ListKeyspaces` on an account that created nothing reports every service-managed
//!    keyspace, each single-Region, with the published keyspace ARN.
//! 2. `GetKeyspace` resolves each of them, and `ListTables` answers 200 with that
//!    keyspace's system tables — never the `ResourceNotFoundException` reserved for a
//!    keyspace genuinely absent from state.
//! 3. `GetTable` reads each system table back by name, reporting the three `@required`
//!    members and the `ACTIVE` status, and none of the members only a `CreateTable`
//!    could have filled.
//! 4. `ListTables` pages a system keyspace in one total order for any page size, and a
//!    read leaves the catalog exactly as it was.
//! 5. Every write against a service-managed keyspace — `CreateTable`, `UpdateTable`,
//!    `DeleteTable`, `CreateType`, `DeleteKeyspace`, `TagResource`, `UntagResource` — is
//!    the modelled 400 `ValidationException`, and `CreateKeyspace` of one of their names
//!    is the modelled 409 `ConflictException` because the name is already taken.
//! 6. A keyspace that is NOT service-managed still 404s, so (5) and (2) are rules about
//!    the catalog rather than a blanket answer.

use serde_json::{json, Value};
use service_com_amazonaws_keyspaces_keyspacesservice::state as keyspaces;

use crate::keyspace_table_lifecycle::{
    assert_failure, assert_ok, keyspace_arn, minimal_schema, table_arn, Emulator,
};

/// `rs` = `SINGLE_REGION`, from the model's `rs` enum.
const SINGLE_REGION: &str = "SINGLE_REGION";

/// The `tableName`s of a `ListTables` body, in the order the page reports them.
fn table_names(body: &Value) -> Vec<String> {
    body["tables"]
        .as_array()
        .unwrap_or_else(|| panic!("ListTables must report a `tables` list, got {body}"))
        .iter()
        .map(|summary| {
            summary["tableName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Every table of `keyspace`, walked at `page_size` so pagination is exercised at the
/// same time; returns the names in page order.
async fn walk_tables(emulator: &Emulator, keyspace: &str, page_size: usize) -> Vec<String> {
    let case = format!("{keyspace} at page size {page_size}");
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut request = json!({"keyspaceName": keyspace, "maxResults": page_size});
        if let Some(resume) = &token {
            request["nextToken"] = json!(resume);
        }
        let page = assert_ok(&case, emulator.call("ListTables", request).await);
        pages += 1;
        assert!(
            pages <= seen.len() + 2,
            "case {case:?}: pagination did not terminate after {pages} pages"
        );
        let page_names = table_names(&page);
        assert!(
            page_names.len() <= page_size,
            "case {case:?}: page {pages} carries {} items, more than the requested {page_size}",
            page_names.len()
        );
        seen.extend(page_names);
        match page.get("nextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    seen
}

/// RULES 1, 2 and 4: the catalog is there before any call, every one of its keyspaces
/// resolves, and each lists exactly its own system tables — at every page size.
#[tokio::test]
async fn the_service_managed_catalog_is_present_before_any_call() {
    let emulator = Emulator::start();

    // RULE 1: the whole catalog, from the seed rather than from a create.
    let listed = assert_ok(
        "an account that created nothing",
        emulator.call("ListKeyspaces", json!({})).await,
    );
    let reported = listed["keyspaces"]
        .as_array()
        .unwrap_or_else(|| panic!("ListKeyspaces must report a `keyspaces` list, got {listed}"))
        .clone();
    assert_eq!(
        reported.len(),
        keyspaces::SERVICE_MANAGED_KEYSPACES.len(),
        "case {:?}: expected {} service-managed keyspaces, got {listed}",
        "an account that created nothing",
        keyspaces::SERVICE_MANAGED_KEYSPACES.len()
    );

    for (keyspace, system_tables) in keyspaces::SERVICE_MANAGED_KEYSPACES {
        let summary = reported
            .iter()
            .find(|summary| summary["keyspaceName"] == json!(keyspace))
            .unwrap_or_else(|| {
                panic!("case {keyspace:?}: ListKeyspaces did not report it: {listed}")
            });
        assert_eq!(
            summary,
            &json!({
                "keyspaceName": keyspace,
                "resourceArn": keyspace_arn(keyspace),
                "replicationStrategy": SINGLE_REGION,
            }),
            "case {keyspace:?}: expected the single-Region summary, got {summary}"
        );

        // RULE 2: it resolves through GetKeyspace...
        let got = assert_ok(
            keyspace,
            emulator
                .call("GetKeyspace", json!({"keyspaceName": keyspace}))
                .await,
        );
        assert_eq!(
            (&got["keyspaceName"], &got["resourceArn"], &got["replicationStrategy"]),
            (
                &json!(keyspace),
                &json!(keyspace_arn(keyspace)),
                &json!(SINGLE_REGION)
            ),
            "case {keyspace:?}: expected the stored keyspace, got {got}"
        );
        assert_eq!(
            got.get("replicationRegions"),
            None,
            "case {keyspace:?}: a single-Region keyspace reports no replicationRegions: {got}"
        );

        // ... and ListTables answers 200 with its system tables, not a 404.
        let mut expected: Vec<String> =
            system_tables.iter().map(|name| (*name).to_string()).collect();
        expected.sort();
        let whole = assert_ok(
            keyspace,
            emulator
                .call("ListTables", json!({"keyspaceName": keyspace}))
                .await,
        );
        assert_eq!(
            table_names(&whole),
            expected,
            "case {keyspace:?}: expected {expected:?}, got {whole}"
        );

        // RULE 4: one total order at every page size, from the smallest the model allows
        // (`maxResults` range 1..1000) to past the collection, and a read changes nothing.
        for page_size in [1usize, 2, 3, expected.len().max(1), 1000] {
            let walked = walk_tables(&emulator, keyspace, page_size).await;
            assert_eq!(
                walked, expected,
                "case {keyspace:?} at page size {page_size}: expected {expected:?}, got {walked:?}"
            );
        }
        let again = assert_ok(
            keyspace,
            emulator
                .call("ListTables", json!({"keyspaceName": keyspace}))
                .await,
        );
        assert_eq!(
            again, whole,
            "case {keyspace:?}: a read must not change what a later caller sees: {whole} then {again}"
        );
    }

    // RULE 6: the answers above are about the catalog, not a blanket 200 — a keyspace
    // nothing holds is still the modelled 404.
    for absent in ["system_absent", "systemish", "not_a_system_ks"] {
        assert_failure(
            absent,
            emulator
                .call("ListTables", json!({"keyspaceName": absent}))
                .await,
            404,
            "ResourceNotFoundException",
        );
        assert_failure(
            absent,
            emulator
                .call("GetKeyspace", json!({"keyspaceName": absent}))
                .await,
            404,
            "ResourceNotFoundException",
        );
    }
}

/// RULE 3: every system table reads back through `GetTable` with the members the model
/// requires and none of the settings only a create could have chosen.
#[tokio::test]
async fn every_system_table_reads_back_through_get_table() {
    let emulator = Emulator::start();

    for (keyspace, system_tables) in keyspaces::SERVICE_MANAGED_KEYSPACES {
        for table in *system_tables {
            let case = format!("{keyspace}.{table}");
            let got = assert_ok(
                &case,
                emulator
                    .call(
                        "GetTable",
                        json!({"keyspaceName": keyspace, "tableName": table}),
                    )
                    .await,
            );
            assert_eq!(
                (&got["keyspaceName"], &got["tableName"], &got["resourceArn"]),
                (
                    &json!(keyspace),
                    &json!(table),
                    &json!(table_arn(keyspace, table))
                ),
                "case {case:?}: expected the required members of the stored table, got {got}"
            );
            // A table the account already held is in service.
            assert_eq!(
                got["status"],
                json!(keyspaces::TABLE_STATUS_ACTIVE),
                "case {case:?}: expected an ACTIVE system table, got {got}"
            );
            // Nothing created it, so none of the members a create fills is reported
            // rather than a default nobody configured. Each is optional in
            // `GetTableResponse`.
            for absent in [
                "creationTimestamp",
                "schemaDefinition",
                "capacitySpecification",
                "encryptionSpecification",
                "pointInTimeRecovery",
                "ttl",
                "defaultTimeToLive",
                "comment",
                "clientSideTimestamps",
            ] {
                assert_eq!(
                    got.get(absent),
                    None,
                    "case {case:?}: a service-managed table reports no {absent}: {got}"
                );
            }

            // A table of the catalog that does not exist is still the modelled 404, so
            // the answer above is the stored table rather than any name in a system
            // keyspace.
            assert_failure(
                &format!("{keyspace}.{table}_absent"),
                emulator
                    .call(
                        "GetTable",
                        json!({"keyspaceName": keyspace, "tableName": format!("{table}_absent")}),
                    )
                    .await,
                404,
                "ResourceNotFoundException",
            );
        }
    }
}

/// RULE 5: a service-managed keyspace takes no write at all.
///
/// One row per operation that creates, changes or deletes something inside a keyspace,
/// run against EVERY service-managed keyspace — and the same operations are shown to work
/// on a keyspace the caller created, so the refusal is the rule and not a blanket no.
#[tokio::test]
async fn a_service_managed_keyspace_refuses_every_write() {
    let emulator = Emulator::start();

    /// One write, as `(operation, request body built from the keyspace and one of its
    /// tables)`.
    type Write = (&'static str, fn(&str, &str) -> Value);
    let writes: Vec<Write> = vec![
        ("CreateTable", |keyspace, _table| {
            json!({
                "keyspaceName": keyspace,
                "tableName": "injected_tbl",
                "schemaDefinition": minimal_schema("pk"),
            })
        }),
        ("UpdateTable", |keyspace, table| {
            json!({
                "keyspaceName": keyspace,
                "tableName": table,
                "addColumns": [{"name": "injected", "type": "text"}],
            })
        }),
        ("DeleteTable", |keyspace, table| {
            json!({"keyspaceName": keyspace, "tableName": table})
        }),
        ("CreateType", |keyspace, _table| {
            json!({
                "keyspaceName": keyspace,
                "typeName": "injected_type",
                "fieldDefinitions": [{"name": "one", "type": "text"}],
            })
        }),
        ("DeleteKeyspace", |keyspace, _table| {
            json!({"keyspaceName": keyspace})
        }),
        ("TagResource", |keyspace, _table| {
            json!({
                "resourceArn": keyspace_arn(keyspace),
                "tags": [{"key": "owner", "value": "someone"}],
            })
        }),
        ("UntagResource", |keyspace, table| {
            json!({
                "resourceArn": table_arn(keyspace, table),
                "tags": [{"key": "owner", "value": "someone"}],
            })
        }),
    ];

    for (keyspace, system_tables) in keyspaces::SERVICE_MANAGED_KEYSPACES {
        let table = system_tables
            .first()
            .unwrap_or_else(|| panic!("case {keyspace:?}: the seed gives it at least one table"));
        for (operation, request) in &writes {
            assert_failure(
                &format!("{operation} against {keyspace}"),
                emulator.call(operation, request(keyspace, table)).await,
                400,
                "ValidationException",
            );
        }

        // The name is already taken, so a create of it is the modelled conflict.
        assert_failure(
            &format!("CreateKeyspace {keyspace}"),
            emulator
                .call("CreateKeyspace", json!({"keyspaceName": keyspace}))
                .await,
            409,
            "ConflictException",
        );

        // And nothing above changed the catalog.
        let mut expected: Vec<String> =
            system_tables.iter().map(|name| (*name).to_string()).collect();
        expected.sort();
        let listed = assert_ok(
            keyspace,
            emulator
                .call("ListTables", json!({"keyspaceName": keyspace}))
                .await,
        );
        assert_eq!(
            table_names(&listed),
            expected,
            "case {keyspace:?}: a refused write must leave the catalog alone, got {listed}"
        );
        // Reading the tags of a resource the service manages is a report, not a 404: it
        // holds none, and the modelled `TagList` cannot carry an empty list, so the
        // member is absent.
        let tags = assert_ok(
            keyspace,
            emulator
                .call(
                    "ListTagsForResource",
                    json!({"resourceArn": keyspace_arn(keyspace)}),
                )
                .await,
        );
        assert_eq!(
            tags.get("tags"),
            None,
            "case {keyspace:?}: a service-managed keyspace holds no tags, got {tags}"
        );
    }

    // The same writes against a keyspace the CALLER created are accepted, so the rule
    // above is about the service-managed catalog.
    let own = "own_ks";
    assert_ok(
        own,
        emulator
            .call("CreateKeyspace", json!({"keyspaceName": own}))
            .await,
    );
    assert_ok(
        own,
        emulator
            .call(
                "CreateTable",
                json!({
                    "keyspaceName": own,
                    "tableName": "own_tbl",
                    "schemaDefinition": minimal_schema("pk"),
                }),
            )
            .await,
    );
    assert_ok(
        own,
        emulator
            .call(
                "TagResource",
                json!({
                    "resourceArn": keyspace_arn(own),
                    "tags": [{"key": "owner", "value": "someone"}],
                }),
            )
            .await,
    );
    assert_ok(
        own,
        emulator
            .call("DeleteKeyspace", json!({"keyspaceName": own}))
            .await,
    );
}
