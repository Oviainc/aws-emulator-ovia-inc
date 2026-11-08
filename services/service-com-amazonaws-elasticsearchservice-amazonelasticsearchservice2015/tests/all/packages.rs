// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the package surface: `CreatePackage` and the
//! `DescribePackages` read of it, driven as real restJson1 requests through the
//! router.
//!
//! The rules under test, each asserted over a SET of inputs rather than one
//! recorded call:
//!
//! 1. A create PERSISTS the package and answers with the stored record's own
//!    `PackageDetails`; a later `DescribePackages` answers from that same
//!    record, with the id the service minted for it.
//!
//! `DescribePackages` reports "all packages available to Amazon ES", so every
//! answer below also carries the plugin packages the service publishes; the
//! rows here are about the packages this account CREATES, and the published
//! catalogue is the subject of `published_package_catalog.rs`.
//! 2. A created package is `COPYING` for a non-zero span and then settles on
//!    `AVAILABLE`, which is when it has an `AvailablePackageVersion` — and no
//!    read changes what the next read sees.
//! 3. A name the Region already holds cannot be created again.
//! 4. `DescribePackages` reports only the Region the call was signed for, and a
//!    filter selects by the field it names.
//! 5. `MaxResults` pages the set and `NextToken` resumes it; a token naming no
//!    package in the set is refused.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call_in, router, Answer};

/// The region the test signs for.
const REGION: &str = "us-west-2";

/// Longer than the emulator's copy span, so the transition is certainly due.
const PAST_COPYING: Duration = Duration::from_millis(900);

async fn create_package_in(router: &MultiServiceRouter, region: &str, body: &Value) -> Answer {
    call_in(router, region, "POST", "/2015-01-01/packages", Some(body)).await
}

async fn create_package(router: &MultiServiceRouter, body: &Value) -> Answer {
    create_package_in(router, REGION, body).await
}

async fn describe_packages_in(router: &MultiServiceRouter, region: &str, body: &Value) -> Answer {
    call_in(
        router,
        region,
        "POST",
        "/2015-01-01/packages/describe",
        Some(body),
    )
    .await
}

async fn describe_packages(router: &MultiServiceRouter, body: &Value) -> Answer {
    describe_packages_in(router, REGION, body).await
}

/// The `PackageDetails` entries a describe reported, in the order it gave them.
fn reported(answer: &Answer, case: &str) -> Vec<Value> {
    answer
        .at(case, "/PackageDetailsList")
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: PackageDetailsList is not a list, got {}", answer.body))
        .clone()
}

/// The one entry of a describe answer carrying `package_id`.
fn entry_for(answer: &Answer, case: &str, package_id: &str) -> Value {
    let entries = reported(answer, case);
    let mut found: Vec<Value> = entries
        .iter()
        .filter(|entry| entry.pointer("/PackageID") == Some(&json!(package_id)))
        .cloned()
        .collect();
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: {package_id} must be reported exactly once, got {found:?}"
    );
    found.remove(0)
}

/// The names a describe answer carries, restricted to the ones `names` lists —
/// the packages the case itself created, told apart from the published
/// catalogue every answer also carries.
fn own_names(answer: &Answer, case: &str, names: &[&str]) -> Vec<String> {
    let mut got: Vec<String> = reported(answer, case)
        .iter()
        .filter_map(|entry| {
            entry
                .pointer("/PackageName")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .filter(|name| names.contains(&name.as_str()))
        .collect();
    got.sort();
    got
}

/// One create request and what the stored record must then report.
struct CreateCase {
    label: &'static str,
    request: Value,
    name: &'static str,
    package_type: &'static str,
    description: Option<&'static str>,
}

/// Rows differ along what rule 1 generalises over: whether the optional
/// description is given, the length of the name and of the S3 key, and the
/// bucket the content comes from.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            label: "only the required members",
            request: json!({
                "PackageName": "dict-minimal",
                "PackageType": "TXT-DICTIONARY",
                "PackageSource": {"S3BucketName": "pkg-bucket", "S3Key": "pkg.txt"}
            }),
            name: "dict-minimal",
            package_type: "TXT-DICTIONARY",
            description: None,
        },
        CreateCase {
            label: "a description and a nested key",
            request: json!({
                "PackageName": "dict-described",
                "PackageType": "TXT-DICTIONARY",
                "PackageDescription": "synonyms for the search tier",
                "PackageSource": {
                    "S3BucketName": "another-bucket",
                    "S3Key": "dictionaries/en/synonyms-v3.txt"
                }
            }),
            name: "dict-described",
            package_type: "TXT-DICTIONARY",
            description: Some("synonyms for the search tier"),
        },
        CreateCase {
            label: "the shortest name the model admits",
            request: json!({
                "PackageName": "abc",
                "PackageType": "TXT-DICTIONARY",
                "PackageDescription": "",
                "PackageSource": {"S3BucketName": "pkg", "S3Key": "a"}
            }),
            name: "abc",
            package_type: "TXT-DICTIONARY",
            description: Some(""),
        },
    ]
}

/// RULE 1: the create persists the package, and a separate describe answers
/// from the stored record.
#[tokio::test]
async fn create_persists_the_package_and_describe_answers_from_the_store() {
    for case in create_cases() {
        let router = router();
        let label = case.label;
        let created = create_package(&router, &case.request).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: create must answer 200, got {} {}",
            created.status, created.body
        );

        let details = "/PackageDetails";
        let package_id = created.text(label, &format!("{details}/PackageID"));
        assert!(
            !package_id.is_empty(),
            "case {label:?}: PackageID must be a non-empty id, got {package_id:?}"
        );
        assert_eq!(
            created.at(label, &format!("{details}/PackageName")),
            &json!(case.name),
            "case {label:?}: PackageName must be the name given"
        );
        assert_eq!(
            created.at(label, &format!("{details}/PackageType")),
            &json!(case.package_type),
            "case {label:?}: PackageType must be the type given"
        );
        match case.description {
            Some(description) => assert_eq!(
                created.at(label, &format!("{details}/PackageDescription")),
                &json!(description),
                "case {label:?}: PackageDescription must be the description given"
            ),
            None => assert!(
                created.missing(&format!("{details}/PackageDescription")),
                "case {label:?}: a create with no description must report none, got {:?}",
                created.body.pointer(&format!("{details}/PackageDescription"))
            ),
        }
        assert_eq!(
            created.at(label, &format!("{details}/PackageStatus")),
            &json!("COPYING"),
            "case {label:?}: a freshly created package is still COPYING"
        );
        assert!(
            !created.missing(&format!("{details}/CreatedAt")),
            "case {label:?}: CreatedAt must be reported, got {}",
            created.body
        );
        assert!(
            !created.missing(&format!("{details}/LastUpdatedAt")),
            "case {label:?}: LastUpdatedAt must be reported, got {}",
            created.body
        );

        // The separate read returns the SAME stored record.
        let read = describe_packages(&router, &json!({})).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: describe must answer 200, got {} {}",
            read.status, read.body
        );
        let entry = entry_for(&read, label, &package_id);
        for member in ["PackageID", "PackageName", "PackageType", "CreatedAt"] {
            assert_eq!(
                entry.pointer(&format!("/{member}")),
                created.body.pointer(&format!("{details}/{member}")),
                "case {label:?}: describe must report the create's {member}, got {:?}",
                entry.pointer(&format!("/{member}"))
            );
        }
        assert_eq!(
            own_names(&read, label, &[case.name]),
            vec![case.name.to_string()],
            "case {label:?}: the created package must be the only one of this account's own \
             reported"
        );
    }
}

/// RULE 1 (ids): two packages created in one region get different ids, and each
/// is reported under its own.
#[tokio::test]
async fn each_created_package_gets_its_own_id() {
    let router = router();
    let mut ids = Vec::new();
    for name in ["dict-one", "dict-two", "dict-three"] {
        let created = create_package(
            &router,
            &json!({
                "PackageName": name,
                "PackageType": "TXT-DICTIONARY",
                "PackageSource": {"S3BucketName": "pkg-bucket", "S3Key": "pkg.txt"}
            }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: create must answer 200, got {} {}",
            created.status, created.body
        );
        ids.push(created.text(name, "/PackageDetails/PackageID"));
    }

    let mut unique = ids.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        ids.len(),
        "every package must get its own id, got {ids:?}"
    );

    for (name, id) in ["dict-one", "dict-two", "dict-three"].iter().zip(&ids) {
        let read = describe_packages(
            &router,
            &json!({"Filters": [{"Name": "PackageID", "Value": [id]}]}),
        )
        .await;
        let entries = reported(&read, name);
        assert_eq!(
            entries.len(),
            1,
            "case {name:?}: the id must select exactly its own package, got {entries:?}"
        );
        assert_eq!(
            entries[0].pointer("/PackageName"),
            Some(&json!(name)),
            "case {name:?}: the id must select the package created under it, got {:?}",
            entries[0].pointer("/PackageName")
        );
    }
}

/// RULE 2: a package copies for a non-zero span, settles on AVAILABLE with a
/// version, and no read moves it.
#[tokio::test]
async fn a_package_copies_then_settles_without_a_read_changing_it() {
    let router = router();
    let created = create_package(
        &router,
        &json!({
            "PackageName": "dict-settling",
            "PackageType": "TXT-DICTIONARY",
            "PackageSource": {"S3BucketName": "pkg-bucket", "S3Key": "pkg.txt"}
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "create must answer 200, got {} {}",
        created.status, created.body
    );
    let package_id = created.text("while copying", "/PackageDetails/PackageID");
    assert!(
        created.missing("/PackageDetails/AvailablePackageVersion"),
        "case {:?}: a copying package has no available version yet, got {:?}",
        "while copying",
        created.body.pointer("/PackageDetails/AvailablePackageVersion")
    );

    // Two reads inside the copy span agree: a read must not move the status on.
    for case in ["first read while copying", "second read while copying"] {
        let read = describe_packages(&router, &json!({})).await;
        let entry = entry_for(&read, case, &package_id);
        assert_eq!(
            entry.pointer("/PackageStatus"),
            Some(&json!("COPYING")),
            "case {case:?}: the package must still be COPYING, got {:?}",
            entry.pointer("/PackageStatus")
        );
    }

    // The transition is due by the clock alone; nothing polls for it.
    tokio::time::sleep(PAST_COPYING).await;

    for case in ["first read once due", "second read once due"] {
        let read = describe_packages(&router, &json!({})).await;
        let entry = entry_for(&read, case, &package_id);
        assert_eq!(
            entry.pointer("/PackageStatus"),
            Some(&json!("AVAILABLE")),
            "case {case:?}: the copy is due, so the package must be AVAILABLE, got {:?}",
            entry.pointer("/PackageStatus")
        );
        assert_eq!(
            entry.pointer("/AvailablePackageVersion"),
            Some(&json!("v1")),
            "case {case:?}: an available package reports its first version, got {:?}",
            entry.pointer("/AvailablePackageVersion")
        );
    }
}

/// RULE 3: a name the Region already holds is refused, and the stored package is
/// left alone.
#[tokio::test]
async fn a_package_name_the_region_holds_cannot_be_created_again() {
    let router = router();
    let first = create_package(
        &router,
        &json!({
            "PackageName": "dict-taken",
            "PackageType": "TXT-DICTIONARY",
            "PackageDescription": "the first content",
            "PackageSource": {"S3BucketName": "pkg-bucket", "S3Key": "first.txt"}
        }),
    )
    .await;
    assert_eq!(
        first.status, 200,
        "the first create must succeed, got {} {}",
        first.status, first.body
    );

    for (case, body) in [
        (
            "the identical request",
            json!({
                "PackageName": "dict-taken",
                "PackageType": "TXT-DICTIONARY",
                "PackageDescription": "the first content",
                "PackageSource": {"S3BucketName": "pkg-bucket", "S3Key": "first.txt"}
            }),
        ),
        (
            "the same name, another source",
            json!({
                "PackageName": "dict-taken",
                "PackageType": "TXT-DICTIONARY",
                "PackageSource": {"S3BucketName": "other-bucket", "S3Key": "second.txt"}
            }),
        ),
    ] {
        let again = create_package(&router, &body).await;
        assert_eq!(
            again.status, 409,
            "case {case:?}: a duplicate name must be refused with 409, got {} {}",
            again.status, again.body
        );
        assert!(
            again.error_type.contains("ResourceAlreadyExistsException"),
            "case {case:?}: the error must be ResourceAlreadyExistsException, got {:?} / {}",
            again.error_type,
            again.body
        );
    }

    let read = describe_packages(&router, &json!({})).await;
    assert_eq!(
        own_names(&read, "after the refusals", &["dict-taken"]),
        vec!["dict-taken".to_string()],
        "a refused duplicate must store nothing, got {}",
        read.body
    );
    let entry = entry_for(
        &read,
        "after the refusals",
        &first.text("the first create", "/PackageDetails/PackageID"),
    );
    assert_eq!(
        entry.pointer("/PackageName"),
        Some(&json!("dict-taken")),
        "the stored package must be the first one, got {entry:?}"
    );

    // The same name in another Region is another package: the name is unique
    // per Region, not per account.
    let elsewhere = create_package_in(
        &router,
        "eu-west-1",
        &json!({
            "PackageName": "dict-taken",
            "PackageType": "TXT-DICTIONARY",
            "PackageSource": {"S3BucketName": "pkg-bucket", "S3Key": "first.txt"}
        }),
    )
    .await;
    assert_eq!(
        elsewhere.status, 200,
        "the same name in another Region must be accepted, got {} {}",
        elsewhere.status, elsewhere.body
    );
}

/// RULE 4: a describe reports the Region it was signed for, and each filter
/// selects by the field it names.
#[tokio::test]
async fn a_describe_is_region_scoped_and_filters_by_the_field_it_names() {
    let router = router();
    for name in ["dict-alpha", "dict-beta"] {
        create_package(
            &router,
            &json!({
                "PackageName": name,
                "PackageType": "TXT-DICTIONARY",
                "PackageSource": {"S3BucketName": "pkg-bucket", "S3Key": "pkg.txt"}
            }),
        )
        .await;
    }
    create_package_in(
        &router,
        "eu-west-1",
        &json!({
            "PackageName": "dict-elsewhere",
            "PackageType": "TXT-DICTIONARY",
            "PackageSource": {"S3BucketName": "pkg-bucket", "S3Key": "pkg.txt"}
        }),
    )
    .await;

    let here = describe_packages(&router, &json!({})).await;
    let created_names = ["dict-alpha", "dict-beta", "dict-elsewhere"];
    assert_eq!(
        own_names(&here, "the signed region", &created_names),
        vec!["dict-alpha".to_string(), "dict-beta".to_string()],
        "a describe must report only the Region it was signed for, got {}",
        here.body
    );

    let elsewhere = describe_packages_in(&router, "eu-west-1", &json!({})).await;
    assert_eq!(
        own_names(&elsewhere, "another region", &created_names),
        vec!["dict-elsewhere".to_string()],
        "the other Region must report only its own package, got {}",
        elsewhere.body
    );

    // Rows span the three fields the filter name enum declares, a filter
    // carrying several alternative values, and a value nothing matches.
    for (case, filters, expected) in [
        (
            "one name",
            json!([{"Name": "PackageName", "Value": ["dict-alpha"]}]),
            vec!["dict-alpha"],
        ),
        (
            "either of two names",
            json!([{"Name": "PackageName", "Value": ["dict-alpha", "dict-beta"]}]),
            vec!["dict-alpha", "dict-beta"],
        ),
        (
            "a name no package carries",
            json!([{"Name": "PackageName", "Value": ["dict-absent"]}]),
            vec![],
        ),
        (
            "the status every package is in",
            json!([{"Name": "PackageStatus", "Value": ["COPYING"]}]),
            vec!["dict-alpha", "dict-beta"],
        ),
        (
            "a status no package is in yet",
            json!([{"Name": "PackageStatus", "Value": ["AVAILABLE"]}]),
            vec![],
        ),
        (
            "two filters, which are cumulative",
            json!([
                {"Name": "PackageName", "Value": ["dict-alpha", "dict-beta"]},
                {"Name": "PackageStatus", "Value": ["COPYING"]}
            ]),
            vec!["dict-alpha", "dict-beta"],
        ),
        (
            "two filters that cannot both hold",
            json!([
                {"Name": "PackageName", "Value": ["dict-alpha"]},
                {"Name": "PackageStatus", "Value": ["AVAILABLE"]}
            ]),
            vec![],
        ),
    ] {
        let answer = describe_packages(&router, &json!({"Filters": filters})).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a filtered describe must answer 200, got {} {}",
            answer.status, answer.body
        );
        // Restricted to the packages this case created: a status filter also
        // selects published packages, which the catalogue suite asserts.
        let got = own_names(&answer, case, &created_names);
        assert_eq!(
            got, expected,
            "case {case:?}: expected {expected:?}, got {got:?}"
        );
    }
}

/// RULE 5: `MaxResults` pages the set, `NextToken` resumes it exactly where the
/// page ended, and a token naming no package in the set is refused.
#[tokio::test]
async fn max_results_pages_the_set_and_next_token_resumes_it() {
    let router = router();
    for index in 0..5 {
        create_package(
            &router,
            &json!({
                "PackageName": format!("dict-page-{index}"),
                "PackageType": "TXT-DICTIONARY",
                "PackageSource": {"S3BucketName": "pkg-bucket", "S3Key": "pkg.txt"}
            }),
        )
        .await;
    }

    let whole = describe_packages(&router, &json!({})).await;
    let all: Vec<Value> = reported(&whole, "no page size");
    assert_eq!(
        own_names(
            &whole,
            "no page size",
            &[
                "dict-page-0",
                "dict-page-1",
                "dict-page-2",
                "dict-page-3",
                "dict-page-4"
            ]
        )
        .len(),
        5,
        "a request naming no page size must report every package of this account, got {all:?}"
    );
    assert!(
        all.len() > 5,
        "the answer must also carry the published catalogue, got {all:?}"
    );
    assert!(
        whole.missing("/NextToken"),
        "a page that ended the set carries no NextToken, got {:?}",
        whole.body.pointer("/NextToken")
    );

    // Rows differ along the page size, including one past the set's own size.
    for page_size in [1_usize, 2, 5, 7, all.len() + 2] {
        let case = format!("page size {page_size}");
        let mut walked: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        // One page per page-size-worth of the set, plus the page that reports
        // the set ended.
        let most_pages = all.len().div_ceil(page_size) + 1;
        loop {
            let body = match &token {
                Some(token) => json!({"MaxResults": page_size, "NextToken": token}),
                None => json!({"MaxResults": page_size}),
            };
            let page = describe_packages(&router, &body).await;
            assert_eq!(
                page.status, 200,
                "case {case:?}: a page must answer 200, got {} {}",
                page.status, page.body
            );
            let entries = reported(&page, &case);
            assert!(
                entries.len() <= page_size,
                "case {case:?}: a page must not exceed the page size, got {} entries",
                entries.len()
            );
            walked.extend(entries);
            pages += 1;
            assert!(
                pages <= most_pages,
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
            "case {case:?}: paging must walk the whole set in order, got {walked:?}"
        );
    }

    for (case, token) in [
        ("a token this service never minted", "pkg-not-a-real-token"),
        ("an empty token", ""),
    ] {
        let refused = describe_packages(&router, &json!({"NextToken": token})).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: a token naming no package must be refused with 400, got {} {}",
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
