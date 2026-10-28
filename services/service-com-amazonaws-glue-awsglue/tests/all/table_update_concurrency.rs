// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateTable`'s optimistic concurrency: the `VersionId` an update
//! names.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: an update that names the version the table is CURRENTLY under
//! applies and advances the version pointer, exactly as an update naming no version does;
//! an update that names any other version lost the race and is refused with
//! `ConcurrentModificationException` — writing NOTHING, so the live definition, the version
//! pointer and the archived version history are all left as they were.
//!
//! The rows below differ along what the rule generalises over: how the named version fails
//! to be the current one (one the table has already left, one it never had, one that is not
//! a version counter at all), how many updates the table has taken, and whether the
//! accepted update archived the definition it replaced.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::messages::TABLE_UPDATE_CONCURRENT_MODIFICATION;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, error_message, ok};

/// The database every row creates its table in.
const DATABASE: &str = "concurrency_db";

/// Create the database and one table in it, answering nothing.
async fn seed(router: &MultiServiceRouter, table: &str) {
    ok(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": DATABASE } }),
    )
    .await;
    ok(
        router,
        "CreateTable",
        &json!({
            "DatabaseName": DATABASE,
            "TableInput": { "Name": table, "Description": "the first definition" }
        }),
    )
    .await;
}

/// The table as a read serves it.
async fn read(router: &MultiServiceRouter, table: &str) -> Value {
    ok(
        router,
        "GetTable",
        &json!({ "DatabaseName": DATABASE, "Name": table }),
    )
    .await
}

/// The `VersionId` the table is currently under.
fn version_of(table: &Value) -> String {
    table["Table"]["VersionId"]
        .as_str()
        .unwrap_or_else(|| panic!("a stored table reports its VersionId, got {table}"))
        .to_string()
}

/// How many versions the table's history holds.
async fn version_count(router: &MultiServiceRouter, table: &str) -> usize {
    let listed = ok(
        router,
        "GetTableVersions",
        &json!({ "DatabaseName": DATABASE, "TableName": table }),
    )
    .await;
    listed["TableVersions"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

/// An update naming the CURRENT version applies and advances the pointer, so the version
/// the accepted update named is stale for the next one.
///
/// The rows differ along how many updates the table has taken before the one under test (so
/// the current version is 0, 1 and 2 in turn) and along whether the update archives the
/// definition it replaces.
#[tokio::test]
async fn an_update_naming_the_current_version_applies() {
    // (case, whether the update archives the definition it replaces)
    for (case, skip_archive) in [
        ("archiving each replaced definition", false),
        ("skipping the archive", true),
    ] {
        let router = emulator();
        let table = "events";
        seed(&router, table).await;

        for round in 0..3u32 {
            let before = read(&router, table).await;
            let current = version_of(&before);
            assert_eq!(
                current,
                round.to_string(),
                "case {case}: round {round} must start at version {round}, got {current}"
            );
            let description = format!("definition {}", round + 1);
            ok(
                &router,
                "UpdateTable",
                &json!({
                    "DatabaseName": DATABASE,
                    "TableInput": { "Name": table, "Description": description },
                    "VersionId": current,
                    "SkipArchive": skip_archive
                }),
            )
            .await;

            let after = read(&router, table).await;
            assert_eq!(
                version_of(&after),
                (round + 1).to_string(),
                "case {case}: an accepted update advances the version pointer, got {after}"
            );
            assert_eq!(
                after["Table"]["Description"].as_str(),
                Some(description.as_str()),
                "case {case}: the accepted update's definition must be what a read serves, \
                 got {after}"
            );

            // The version the accepted update named is no longer current, so naming it
            // again is the refusal.
            let (status, body) = call(
                &router,
                "UpdateTable",
                &json!({
                    "DatabaseName": DATABASE,
                    "TableInput": { "Name": table, "Description": "a lost race" },
                    "VersionId": current
                }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case}: round {round}'s version is stale now, got {status} with {body}"
            );
            assert!(
                error_code(&body).ends_with("ConcurrentModificationException"),
                "case {case}: expected ConcurrentModificationException, got {} in {body}",
                error_code(&body)
            );
        }
    }
}

/// An update naming any version other than the current one writes NOTHING: the live
/// definition, the version pointer and the version history are identical before and after.
///
/// The rows differ along how the named version fails to be the current one: one the table
/// has already left, one far ahead of it, one it never had, and a value that is not a
/// version counter at all.
#[tokio::test]
async fn an_update_naming_a_stale_version_changes_nothing() {
    let router = emulator();
    let table = "events";
    seed(&router, table).await;
    // One accepted update, so the table has a version it has LEFT and an archived
    // definition a refusal could disturb.
    ok(
        &router,
        "UpdateTable",
        &json!({
            "DatabaseName": DATABASE,
            "TableInput": { "Name": table, "Description": "the second definition" },
            "VersionId": "0"
        }),
    )
    .await;

    let before = read(&router, table).await;
    let versions_before = version_count(&router, table).await;
    assert_eq!(
        version_of(&before),
        "1",
        "the accepted update must have advanced the pointer, got {before}"
    );

    for (case, version_id) in [
        ("a version the table has left", "0"),
        ("a version far ahead of the current one", "99"),
        ("a version the table never had", "7"),
        ("a value that is no version counter", "not-a-version"),
    ] {
        let (status, body) = call(
            &router,
            "UpdateTable",
            &json!({
                "DatabaseName": DATABASE,
                "TableInput": { "Name": table, "Description": "a lost race" },
                "VersionId": version_id
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: a version that is not the current one must be refused, got {status} \
             with {body}"
        );
        assert!(
            error_code(&body).ends_with("ConcurrentModificationException"),
            "case {case}: expected ConcurrentModificationException, got {} in {body}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            TABLE_UPDATE_CONCURRENT_MODIFICATION,
            "case {case}: the refusal must state that the update lost a race, got {body}"
        );

        let after = read(&router, table).await;
        assert_eq!(
            after, before,
            "case {case}: a refused update must leave the live table as it was; expected \
             {before}, got {after}"
        );
        assert_eq!(
            version_count(&router, table).await,
            versions_before,
            "case {case}: a refused update must archive nothing, got a history of {} versions",
            version_count(&router, table).await
        );
    }
}

/// An update that names NO version still applies, whatever version the table is under: the
/// `VersionId` is the caller's own concurrency control and not a requirement.
#[tokio::test]
async fn an_update_naming_no_version_applies() {
    let router = emulator();
    let table = "events";
    seed(&router, table).await;

    for round in 0..3u32 {
        let description = format!("definition {}", round + 1);
        ok(
            &router,
            "UpdateTable",
            &json!({
                "DatabaseName": DATABASE,
                "TableInput": { "Name": table, "Description": description }
            }),
        )
        .await;
        let after = read(&router, table).await;
        assert_eq!(
            version_of(&after),
            (round + 1).to_string(),
            "round {round}: an update naming no version advances the pointer, got {after}"
        );
        assert_eq!(
            after["Table"]["Description"].as_str(),
            Some(description.as_str()),
            "round {round}: the update must be what a read serves, got {after}"
        );
    }
}
