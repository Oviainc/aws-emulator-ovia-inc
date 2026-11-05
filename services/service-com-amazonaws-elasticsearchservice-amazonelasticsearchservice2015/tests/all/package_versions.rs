// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the package **version** surface — `UpdatePackage` and
//! `GetPackageVersionHistory` — driven as real restJson1 requests through the
//! router.
//!
//! The rules under test, each asserted over a SET of inputs rather than one
//! recorded call:
//!
//! 1. A package's versions are a generated sequence: a create mints the first
//!    and each update the next, so the history of a package updated `n` times
//!    has `n + 1` entries and `AvailablePackageVersion` names the newest one.
//! 2. `GetPackageVersionHistory` reports that history NEWEST FIRST, each entry
//!    carrying the `CommitMessage` the update that minted it gave (none for the
//!    create's version) and a `CreatedAt` that increases from the oldest entry
//!    to the newest.
//! 3. An update gives the package new content: it goes back to `COPYING` until
//!    the copy falls due, then reports `AVAILABLE` on the new version, and a
//!    description the update omits is kept.
//! 4. The history is paged by `maxResults` / `nextToken`, and a token naming no
//!    version of the package is refused.
//! 5. A package id the Region does not hold is not found by either operation.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call_in, router, Answer};

/// The region the test signs for.
const REGION: &str = "us-west-2";

/// Longer than the emulator's copy span, so a transition is certainly due.
const PAST_COPYING: Duration = Duration::from_millis(900);

async fn create_package(router: &MultiServiceRouter, body: &Value) -> Answer {
    call_in(router, REGION, "POST", "/2015-01-01/packages", Some(body)).await
}

/// A created package's minted id.
async fn package(router: &MultiServiceRouter, name: &str, description: Option<&str>) -> String {
    let mut body = json!({
        "PackageName": name,
        "PackageType": "TXT-DICTIONARY",
        "PackageSource": {"S3BucketName": "pkg-bucket", "S3Key": "v1.txt"}
    });
    if let Some(description) = description {
        body["PackageDescription"] = json!(description);
    }
    let created = create_package(router, &body).await;
    assert_eq!(
        created.status, 200,
        "case {name:?}: the create must answer 200, got {} {}",
        created.status, created.body
    );
    created.text(name, "/PackageDetails/PackageID")
}

async fn update_package(router: &MultiServiceRouter, body: &Value) -> Answer {
    call_in(
        router,
        REGION,
        "POST",
        "/2015-01-01/packages/update",
        Some(body),
    )
    .await
}

async fn history(router: &MultiServiceRouter, package_id: &str, query: &str) -> Answer {
    call_in(
        router,
        REGION,
        "GET",
        &format!("/2015-01-01/packages/{package_id}/history{query}"),
        None,
    )
    .await
}

async fn describe_packages(router: &MultiServiceRouter, package_id: &str) -> Answer {
    call_in(
        router,
        REGION,
        "POST",
        "/2015-01-01/packages/describe",
        Some(&json!({"Filters": [{"Name": "PackageID", "Value": [package_id]}]})),
    )
    .await
}

/// The `PackageVersionHistory` entries a history read reported, in the order it
/// gave them.
fn entries(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: the history must answer 200, got {} {}",
        answer.status, answer.body
    );
    answer
        .at(case, "/PackageVersionHistoryList")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: PackageVersionHistoryList is not a list, got {body}",
                body = answer.body
            )
        })
        .clone()
}

fn versions(entries: &[Value]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| {
            entry
                .pointer("/PackageVersion")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The stamps a history read reported, as seconds since the epoch.
fn created_at(entries: &[Value], case: &str) -> Vec<f64> {
    entries
        .iter()
        .map(|entry| {
            entry
                .pointer("/CreatedAt")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: an entry has no numeric CreatedAt, got {entry}")
                })
        })
        .collect()
}

/// The `AvailablePackageVersion` a describe reports for one package, if any.
async fn available_version(router: &MultiServiceRouter, package_id: &str, case: &str) -> Value {
    let read = describe_packages(router, package_id).await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: the describe must answer 200, got {} {}",
        read.status, read.body
    );
    read.body
        .pointer("/PackageDetailsList/0/AvailablePackageVersion")
        .cloned()
        .unwrap_or(Value::Null)
}

/// RULES 1, 2 and 3: the version sequence over a SET of update counts.
///
/// Rows differ along how many times the package is updated — none, once (the
/// recorded case) and three times — and along whether each update carries a
/// commit message and a new description.
#[tokio::test]
async fn each_update_mints_the_next_version_and_the_history_is_newest_first() {
    for (case, updates) in [
        ("a never-updated package", 0_usize),
        ("one update", 1),
        ("three updates", 3),
    ] {
        let router = router();
        let package_id = package(&router, "version-dict", Some("the first content")).await;
        // The create's copy must land before the first update, so the stamps the
        // history carries are strictly apart.
        tokio::time::sleep(PAST_COPYING).await;

        for index in 0..updates {
            let answer = update_package(
                &router,
                &json!({
                    "PackageID": package_id,
                    "PackageSource": {
                        "S3BucketName": "pkg-bucket",
                        "S3Key": format!("v{}.txt", index + 2)
                    },
                    "CommitMessage": format!("revision {}", index + 2)
                }),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: update {index} must answer 200, got {} {}",
                answer.status, answer.body
            );
            // New content is COPYING again until the copy falls due.
            assert_eq!(
                answer
                    .at(case, "/PackageDetails/PackageStatus"),
                &json!("COPYING"),
                "case {case:?}: an updated package must be COPYING again, got {:?}",
                answer.body.pointer("/PackageDetails/PackageStatus")
            );
            // A description the update omits is kept.
            assert_eq!(
                answer.at(case, "/PackageDetails/PackageDescription"),
                &json!("the first content"),
                "case {case:?}: an update that names no description must keep the stored one, \
                 got {:?}",
                answer.body.pointer("/PackageDetails/PackageDescription")
            );
            tokio::time::sleep(PAST_COPYING).await;
        }

        let read = history(&router, &package_id, "").await;
        let listed = entries(&read, case);
        let expected: Vec<String> = (1..=updates + 1)
            .rev()
            .map(|ordinal| format!("v{ordinal}"))
            .collect();
        assert_eq!(
            versions(&listed),
            expected,
            "case {case:?}: expected {expected:?} newest-first, got {:?}",
            versions(&listed)
        );
        assert_eq!(
            read.at(case, "/PackageID"),
            &json!(package_id),
            "case {case:?}: the history must name the package it is of, got {:?}",
            read.body.pointer("/PackageID")
        );

        // The stamps decrease down the list, i.e. increase from oldest to newest.
        let stamps = created_at(&listed, case);
        for pair in stamps.windows(2) {
            assert!(
                pair[0] > pair[1],
                "case {case:?}: CreatedAt must decrease down a newest-first history, got {stamps:?}"
            );
        }

        // The commit message is the one the update that minted the version gave;
        // the create's version has none.
        for (index, entry) in listed.iter().enumerate() {
            let ordinal = updates + 1 - index;
            let message = entry.pointer("/CommitMessage").and_then(Value::as_str);
            if ordinal == 1 {
                assert_eq!(
                    message, None,
                    "case {case:?}: the create's version carries no commit message, got \
                     {message:?}"
                );
            } else {
                assert_eq!(
                    message,
                    Some(format!("revision {ordinal}").as_str()),
                    "case {case:?}: version v{ordinal} must carry its own commit message, got \
                     {message:?}"
                );
            }
        }

        // The newest version is the one a domain can associate.
        assert_eq!(
            available_version(&router, &package_id, case).await,
            json!(format!("v{}", updates + 1)),
            "case {case:?}: AvailablePackageVersion must name the newest version"
        );

        // Two reads agree: a history read must not mint or move anything.
        let again = history(&router, &package_id, "").await;
        assert_eq!(
            versions(&entries(&again, case)),
            expected,
            "case {case:?}: a second read must report the same history"
        );
    }
}

/// RULE 3: an update that DOES carry a description replaces the stored one, and
/// the package's own source follows the update.
#[tokio::test]
async fn an_update_that_names_a_description_replaces_the_stored_one() {
    let router = router();
    let package_id = package(&router, "version-described", Some("first")).await;
    tokio::time::sleep(PAST_COPYING).await;

    let updated = update_package(
        &router,
        &json!({
            "PackageID": package_id,
            "PackageSource": {"S3BucketName": "other-bucket", "S3Key": "v2.txt"},
            "PackageDescription": "second",
            "CommitMessage": ""
        }),
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "the update must answer 200, got {} {}",
        updated.status, updated.body
    );
    assert_eq!(
        updated.at("a new description", "/PackageDetails/PackageDescription"),
        &json!("second"),
        "the update's description must replace the stored one, got {:?}",
        updated.body.pointer("/PackageDetails/PackageDescription")
    );

    tokio::time::sleep(PAST_COPYING).await;
    let read = describe_packages(&router, &package_id).await;
    assert_eq!(
        read.body
            .pointer("/PackageDetailsList/0/PackageDescription"),
        Some(&json!("second")),
        "a separate read must answer from the updated record, got {}",
        read.body
    );
    assert_eq!(
        read.body.pointer("/PackageDetailsList/0/PackageStatus"),
        Some(&json!("AVAILABLE")),
        "the new copy is due, so the package must be AVAILABLE again, got {}",
        read.body
    );
}

/// RULE 4: the history is paged, and a token naming no version is refused.
#[tokio::test]
async fn the_history_is_paged_and_an_unknown_token_is_refused() {
    let router = router();
    let package_id = package(&router, "version-paged", None).await;
    for index in 0..4 {
        let answer = update_package(
            &router,
            &json!({
                "PackageID": package_id,
                "PackageSource": {
                    "S3BucketName": "pkg-bucket",
                    "S3Key": format!("v{}.txt", index + 2)
                }
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "update {index} must answer 200, got {} {}",
            answer.status, answer.body
        );
    }

    let whole = history(&router, &package_id, "").await;
    let all = versions(&entries(&whole, "no page size"));
    assert_eq!(
        all,
        vec!["v5", "v4", "v3", "v2", "v1"],
        "the whole history must be reported newest first, got {all:?}"
    );
    assert!(
        whole.missing("/NextToken"),
        "a page that ended the history carries no NextToken, got {:?}",
        whole.body.pointer("/NextToken")
    );

    // Rows differ along the page size, including one past the history's size.
    for page_size in [1_usize, 2, 5, 9] {
        let case = format!("page size {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
                None => format!("?maxResults={page_size}"),
            };
            let page = history(&router, &package_id, &query).await;
            let listed = entries(&page, &case);
            assert!(
                listed.len() <= page_size,
                "case {case:?}: a page must not exceed the page size, got {} entries",
                listed.len()
            );
            walked.extend(versions(&listed));
            pages += 1;
            assert!(
                pages <= 6,
                "case {case:?}: paging must terminate, walked {pages} pages"
            );
            token = page
                .body
                .pointer("/NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, all,
            "case {case:?}: paging must walk the whole history in order, got {walked:?}"
        );
    }

    for (case, token) in [
        ("a version the package never had", "v99"),
        ("a token this service never mints", "not-a-version"),
    ] {
        let refused = history(&router, &package_id, &format!("?nextToken={token}")).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a token naming no version must be refused with 400, got {} {}",
            refused.status, refused.body
        );
        assert!(
            refused.error_type.contains("ValidationException"),
            "case {case:?}: the error must be ValidationException, got {:?} / {}",
            refused.error_type,
            refused.body
        );
    }
}

/// RULE 5: a package id this Region does not hold is not found.
#[tokio::test]
async fn a_package_the_region_does_not_hold_is_not_found() {
    let router = router();
    let package_id = package(&router, "version-elsewhere", None).await;

    // Rows differ along WHY the id does not resolve: nothing ever minted it, and
    // it was minted in another Region.
    for (case, id, region) in [
        (
            "an id nothing minted",
            "pkg-0000000000000000".to_string(),
            REGION,
        ),
        (
            "an id minted in another Region",
            package_id.clone(),
            "eu-west-1",
        ),
    ] {
        let read = call_in(
            &router,
            region,
            "GET",
            &format!("/2015-01-01/packages/{id}/history"),
            None,
        )
        .await;
        assert_eq!(
            read.status, 409,
            "case {case:?}: the history must be refused with 409, got {} {}",
            read.status, read.body
        );
        assert!(
            read.error_type.contains("ResourceNotFoundException"),
            "case {case:?}: the error must be ResourceNotFoundException, got {:?} / {}",
            read.error_type,
            read.body
        );

        let updated = call_in(
            &router,
            region,
            "POST",
            "/2015-01-01/packages/update",
            Some(&json!({
                "PackageID": id,
                "PackageSource": {"S3BucketName": "pkg-bucket", "S3Key": "v2.txt"}
            })),
        )
        .await;
        assert_eq!(
            updated.status, 409,
            "case {case:?}: the update must be refused with 409, got {} {}",
            updated.status, updated.body
        );
        assert!(
            updated.error_type.contains("ResourceNotFoundException"),
            "case {case:?}: the error must be ResourceNotFoundException, got {:?} / {}",
            updated.error_type,
            updated.body
        );
    }

    // The refusals left the real package's history alone.
    let read = history(&router, &package_id, "").await;
    assert_eq!(
        versions(&entries(&read, "after every refusal")),
        vec!["v1"],
        "a refused update must mint no version"
    );
}
