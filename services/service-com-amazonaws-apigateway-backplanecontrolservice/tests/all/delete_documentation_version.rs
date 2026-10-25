// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteDocumentationVersion`: the snapshot it removes, the
//! records it leaves alone, and the refusals it answers.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness, and the create and single-snapshot
//! readers this suite reads a delete back through, are this crate's existing ones.
//!
//! The RULE under test: a delete removes exactly the snapshot its two identifiers name —
//! so a later read of it answers the modelled `NotFoundException` and the identifier is
//! free for a new snapshot — and leaves every other snapshot of that API, every snapshot
//! of every other API, and the documentation parts the snapshots were taken from, as they
//! were. An identifier naming no stored record is the modelled `NotFoundException` and
//! removes nothing.

use serde_json::json;

use crate::documentation_parts::created_part_id;
use crate::documentation_versions::{create_version, get_version, snapshot_request};
use crate::rest_api_lifecycle::{assert_failure, create_api, page_items, Gateway};

/// `DeleteDocumentationVersion` for one snapshot of `api`.
async fn delete_version(
    gateway: &Gateway,
    api: &str,
    version: &str,
) -> crate::rest_api_lifecycle::Answer {
    gateway
        .call(
            "DELETE",
            &format!("/restapis/{api}/documentation/versions/{version}"),
            None,
        )
        .await
}

/// The version identifiers `GetDocumentationVersions` reports for `api`.
async fn listed_versions(gateway: &Gateway, api: &str, case: &str) -> Vec<String> {
    let answer = gateway
        .call(
            "GET",
            &format!("/restapis/{api}/documentation/versions?limit=500"),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: GetDocumentationVersions expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    page_items(&answer.body, case)
        .iter()
        .filter_map(|item| item["version"].as_str().map(str::to_owned))
        .collect()
}

/// An API carrying one documentation part, which is what a snapshot is taken of.
async fn documented_api(gateway: &Gateway, name: &str) -> String {
    let (api, _root) = create_api(gateway, name).await;
    created_part_id(
        gateway,
        &api,
        json!({ "type": "API" }),
        &json!({ "description": name }).to_string(),
        name,
    )
    .await;
    api
}

/// Create the snapshots `versions` names, asserting each was accepted.
async fn snapshot_each(gateway: &Gateway, api: &str, versions: &[&str]) {
    for version in versions {
        let answer = create_version(gateway, api, snapshot_request(version, None)).await;
        assert_eq!(
            answer.status, 201,
            "case CreateDocumentationVersion({version:?}): expected 201, got {} with {:?}",
            answer.status, answer.body
        );
    }
}

#[tokio::test]
async fn a_deleted_documentation_version_is_gone_and_its_siblings_are_untouched() {
    let gateway = Gateway::new();
    let api = documented_api(&gateway, "delete-documentation-version").await;
    let other = documented_api(&gateway, "another-documented-api").await;

    // Rows differ along what the rule generalises over: the FORM of the version
    // identifier, how many snapshots the API holds when one is removed, and which of them
    // is removed (the first created, one in the middle, the last).
    let rows: Vec<(&str, Vec<&str>, &str)> = vec![
        ("the only snapshot", vec!["1.0.0"], "1.0.0"),
        (
            "the first of three",
            vec!["v1", "v2", "v3"],
            "v1",
        ),
        (
            "one of three in the middle",
            vec!["2026-01-01", "2026-02-01", "2026-03-01"],
            "2026-02-01",
        ),
        (
            "the last of two",
            vec!["release candidate", "release"],
            "release",
        ),
    ];

    for (case, versions, removed) in rows {
        let api = documented_api(&gateway, &format!("delete-version-{removed}")).await;
        snapshot_each(&gateway, &api, &versions).await;

        let answer = delete_version(&gateway, &api, removed).await;
        assert_eq!(
            answer.status, 202,
            "case {case}: expected 202, got {} with {:?}",
            answer.status, answer.body
        );

        // The removed snapshot is gone from the single-snapshot reader and from the
        // listing, and every sibling is still there.
        let read = get_version(&gateway, &api, removed).await;
        assert_failure(&read, 404, "NotFoundException", case);
        let mut expected: Vec<String> = versions
            .iter()
            .filter(|version| **version != removed)
            .map(|version| (*version).to_owned())
            .collect();
        expected.sort();
        let mut listed = listed_versions(&gateway, &api, case).await;
        listed.sort();
        assert_eq!(
            listed, expected,
            "case {case}: expected the siblings {expected:?} to survive, got {listed:?}"
        );

        // The identifier is free again: the same version can be snapshotted anew.
        snapshot_each(&gateway, &api, &[removed]).await;
        let reread = get_version(&gateway, &api, removed).await;
        assert_eq!(
            reread.status, 200,
            "case {case}: a re-created snapshot must read back, got {} with {:?}",
            reread.status, reread.body
        );
    }

    // The documentation parts a snapshot was taken from, and another API's snapshots, are
    // untouched by any of it.
    snapshot_each(&gateway, &api, &["1.0.0"]).await;
    snapshot_each(&gateway, &other, &["1.0.0"]).await;
    let answer = delete_version(&gateway, &api, "1.0.0").await;
    assert_eq!(
        answer.status, 202,
        "case the API whose parts must survive: expected 202, got {} with {:?}",
        answer.status, answer.body
    );
    let parts = gateway
        .call("GET", &format!("/restapis/{api}/documentation/parts"), None)
        .await;
    assert_eq!(
        parts.status, 200,
        "case the API whose parts must survive: GetDocumentationParts expected 200, got {} \
         with {:?}",
        parts.status, parts.body
    );
    assert_eq!(
        page_items(&parts.body, "the API whose parts must survive").len(),
        1,
        "case the API whose parts must survive: expected the documentation part to remain, \
         got {:?}",
        parts.body
    );
    assert_eq!(
        listed_versions(&gateway, &other, "another API's snapshots").await,
        vec!["1.0.0".to_string()],
        "case another API's snapshots: a delete on one API must not touch another's"
    );
}

#[tokio::test]
async fn an_identifier_that_names_no_record_is_a_not_found_and_removes_nothing() {
    let gateway = Gateway::new();
    let api = documented_api(&gateway, "delete-version-refusals").await;
    snapshot_each(&gateway, &api, &["1.0.0"]).await;
    let other = documented_api(&gateway, "delete-version-other-api").await;
    snapshot_each(&gateway, &other, &["2.0.0"]).await;

    // Rows span each identifier the operation resolves, one unknown at a time: an API that
    // was never created, a version this API never snapshotted, the same version in another
    // case, and a version held by ANOTHER API.
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "an API that was never created",
            "zzzzzzzzzz".to_string(),
            "1.0.0",
        ),
        (
            "a version this API never snapshotted",
            api.clone(),
            "9.9.9",
        ),
        (
            "the stored version in another case",
            api.clone(),
            "1.0.0-DRAFT",
        ),
        (
            "a version another API holds",
            api.clone(),
            "2.0.0",
        ),
    ];

    for (case, target, version) in rows {
        let answer = delete_version(&gateway, &target, version).await;
        assert_failure(&answer, 404, "NotFoundException", case);
        // Nothing was removed: both APIs still report the snapshot they held.
        assert_eq!(
            listed_versions(&gateway, &api, case).await,
            vec!["1.0.0".to_string()],
            "case {case}: a refused delete must leave the snapshots alone"
        );
        assert_eq!(
            listed_versions(&gateway, &other, case).await,
            vec!["2.0.0".to_string()],
            "case {case}: a refused delete must leave another API's snapshots alone"
        );
    }
}
