// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the plugin packages the SERVICE publishes, driven as
//! real restJson1 requests through the router.
//!
//! The rule under test: the package set this service reports is the union of
//! the packages the account created in a Region and the plugin packages the
//! service publishes to every account — `DescribePackages` describes "all
//! packages available to Amazon ES" — so a published package is listed by that
//! read with every modelled `PackageDetails` member projected, is resolvable by
//! its id by `GetPackageVersionHistory` and by `AssociatePackage`, and reports
//! the same answer on every read.
//!
//! The rows span the published catalogue rather than one entry of it: every
//! entry is checked for the modelled members, and the per-id reads are driven
//! over entries with one published version and entries with several. The
//! members asserted are `PackageDetails`'s and `PackageVersionHistory`'s own:
//!
//! ```text
//! smithy_query(query="shape",shape="PackageDetails,PackageVersionHistory",model_path="services/service-com-amazonaws-elasticsearchservice-amazonelasticsearchservice2015/smithy-model/com-amazonaws-elasticsearchservice-amazonelasticsearchservice2015.json")
//! ```

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call_in, router, Answer};

/// The region the test signs for.
const REGION: &str = "us-west-2";

/// Longer than the emulator's provisioning span, so a domain is certainly past
/// `Processing` and can take a package.
const PAST_PROVISIONING: Duration = Duration::from_millis(900);

/// The plugin type the service reports for a package whose content is a plugin
/// archive.
const ZIP_PLUGIN: &str = "ZIP-PLUGIN";

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

async fn history(router: &MultiServiceRouter, package_id: &str) -> Answer {
    call_in(
        router,
        REGION,
        "GET",
        &format!("/2015-01-01/packages/{package_id}/history"),
        None,
    )
    .await
}

/// The entries one describe answer carries.
fn reported(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: the describe must answer 200, got {} {}",
        answer.status, answer.body
    );
    answer
        .at(case, "/PackageDetailsList")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: PackageDetailsList is not a list, got {}",
                answer.body
            )
        })
        .clone()
}

/// Every published entry the catalogue reports: the plugin packages, told apart
/// from an account's own packages by the type the service reports for them.
fn published(answer: &Answer, case: &str) -> Vec<Value> {
    reported(answer, case)
        .into_iter()
        .filter(|entry| entry.pointer("/PackageType") == Some(&json!(ZIP_PLUGIN)))
        .collect()
}

fn text(entry: &Value, case: &str, member: &str) -> String {
    entry
        .pointer(&format!("/{member}"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: entry has no string {member}, got {entry}"))
        .to_string()
}

/// RULE: the catalogue is populated, and every entry projects every modelled
/// `PackageDetails` member.
#[tokio::test]
async fn the_published_catalogue_is_listed_with_every_modelled_member() {
    let case = "an account that created nothing";
    let router = router();
    let answer = describe_packages(&router, &json!({})).await;
    let entries = published(&answer, case);
    assert!(
        entries.len() > 1,
        "case {case:?}: the catalogue must carry more than one package, got {entries:?}"
    );

    for entry in &entries {
        let name = text(entry, case, "PackageName");
        let id = text(entry, case, "PackageID");
        assert!(
            id.starts_with("pkg-") && id.len() > "pkg-".len(),
            "case {name:?}: PackageID must be an opaque pkg- id, got {id:?}"
        );
        // `PackageName`'s own constraints: @length(3, 28) and
        // @pattern("^[a-z][a-z0-9\-]+$").
        assert!(
            (3..=28).contains(&name.len())
                && name.starts_with(|first: char| first.is_ascii_lowercase())
                && name
                    .chars()
                    .all(|each| each.is_ascii_lowercase() || each.is_ascii_digit() || each == '-'),
            "case {name:?}: PackageName must satisfy the modelled name constraints"
        );
        assert_eq!(
            entry.pointer("/PackageStatus"),
            Some(&json!("AVAILABLE")),
            "case {name:?}: a published package is already copied, got {:?}",
            entry.pointer("/PackageStatus")
        );
        let description = text(entry, case, "PackageDescription");
        assert!(
            !description.is_empty() && description.len() <= 1024,
            "case {name:?}: PackageDescription must be a described, bounded text, got \
             {description:?}"
        );
        let available = text(entry, case, "AvailablePackageVersion");
        assert!(
            available.starts_with('v'),
            "case {name:?}: AvailablePackageVersion must name a version, got {available:?}"
        );
        let created = entry
            .pointer("/CreatedAt")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {name:?}: CreatedAt must be a timestamp, got {entry}"));
        let updated = entry
            .pointer("/LastUpdatedAt")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| {
                panic!("case {name:?}: LastUpdatedAt must be a timestamp, got {entry}")
            });
        assert!(
            created > 0.0 && updated >= created,
            "case {name:?}: LastUpdatedAt must not precede CreatedAt, got {created} / {updated}"
        );
        assert!(
            entry.pointer("/ErrorDetails").is_none_or(Value::is_null),
            "case {name:?}: a published package is in no error state, got {:?}",
            entry.pointer("/ErrorDetails")
        );
    }

    // One entry the issue that filed this did not cite, selected by name through
    // the operation's own filter.
    let selected = describe_packages(
        &router,
        &json!({"Filters": [{"Name": "PackageName", "Value": ["mapper-murmur3"]}]}),
    )
    .await;
    let selected = published(&selected, "selected by name");
    assert_eq!(
        selected.len(),
        1,
        "case {:?}: the name must select exactly its package, got {selected:?}",
        "selected by name"
    );
    assert_eq!(
        selected[0].pointer("/PackageType"),
        Some(&json!(ZIP_PLUGIN)),
        "case {:?}: a published plugin package reports the plugin type, got {:?}",
        "selected by name",
        selected[0].pointer("/PackageType")
    );
}

/// RULE: the catalogue is the service's, so every Region reports the same
/// entries under the same ids, and two reads of it agree.
#[tokio::test]
async fn the_catalogue_is_the_same_in_every_region_and_on_every_read() {
    let router = router();
    let here = published(&describe_packages(&router, &json!({})).await, "first read");
    let again = published(
        &describe_packages(&router, &json!({})).await,
        "second read",
    );
    assert_eq!(
        here, again,
        "two reads of the catalogue must agree, got {here:?} then {again:?}"
    );

    for region in ["eu-west-1", "ap-southeast-2"] {
        let there = published(
            &describe_packages_in(&router, region, &json!({})).await,
            region,
        );
        assert_eq!(
            there, here,
            "case {region:?}: the published catalogue must be the same in every Region"
        );
    }
}

/// RULE: every listed package is resolvable by its id, and reports its version
/// history newest first with the modelled members.
#[tokio::test]
async fn every_listed_package_reports_its_version_history_by_id() {
    let case = "the whole catalogue";
    let router = router();
    let entries = published(&describe_packages(&router, &json!({})).await, case);

    let mut with_one_version = 0;
    let mut with_several = 0;
    for entry in &entries {
        let name = text(entry, case, "PackageName");
        let id = text(entry, case, "PackageID");
        let read = history(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {name:?}: the history of a listed package must answer 200, got {} {}",
            read.status, read.body
        );
        assert_eq!(
            read.at(&name, "/PackageID"),
            &json!(id),
            "case {name:?}: the history must echo the id it was read by, got {:?}",
            read.body.pointer("/PackageID")
        );
        let listed = read
            .at(&name, "/PackageVersionHistoryList")
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {name:?}: PackageVersionHistoryList is not a list, got {}",
                    read.body
                )
            })
            .clone();
        assert!(
            !listed.is_empty(),
            "case {name:?}: a listed package must report at least one version, got {listed:?}"
        );
        if listed.len() == 1 {
            with_one_version += 1;
        } else {
            with_several += 1;
        }

        // Newest first: the head is the version the describe offers, and the
        // stamps decrease down the list.
        assert_eq!(
            listed[0].pointer("/PackageVersion"),
            entry.pointer("/AvailablePackageVersion"),
            "case {name:?}: the newest version must head the history and be the available \
             one, got {:?} vs {:?}",
            listed[0].pointer("/PackageVersion"),
            entry.pointer("/AvailablePackageVersion")
        );
        let stamps: Vec<f64> = listed
            .iter()
            .map(|version| {
                version
                    .pointer("/CreatedAt")
                    .and_then(Value::as_f64)
                    .unwrap_or_else(|| {
                        panic!("case {name:?}: a version has no CreatedAt, got {version}")
                    })
            })
            .collect();
        for pair in stamps.windows(2) {
            assert!(
                pair[0] > pair[1],
                "case {name:?}: CreatedAt must decrease down a newest-first history, got \
                 {stamps:?}"
            );
        }

        // Two reads agree: reporting a published package writes nothing.
        let repeated = history(&router, &id).await;
        assert_eq!(
            repeated.body.pointer("/PackageVersionHistoryList"),
            read.body.pointer("/PackageVersionHistoryList"),
            "case {name:?}: a second history read must report the same versions"
        );
    }

    assert!(
        with_one_version > 0 && with_several > 0,
        "the catalogue must cover packages with one published version and with several, got \
         {with_one_version} and {with_several}"
    );
}

/// RULE: a published package is a package a domain can install — the point of
/// discovering it — and the association reads report it from the same record.
#[tokio::test]
async fn a_published_package_can_be_associated_with_a_domain() {
    let case = "associating a published package";
    let router = router();
    let entries = published(&describe_packages(&router, &json!({})).await, case);
    let domain_name = "catalog-domain";
    let created = call_in(
        &router,
        REGION,
        "POST",
        "/2015-01-01/es/domain",
        Some(&json!({
            "DomainName": domain_name,
            "ElasticsearchVersion": "OpenSearch_2.19",
            "ElasticsearchClusterConfig": {"InstanceType": "t3.small.elasticsearch", "InstanceCount": 1}
        })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: the domain create must answer 200, got {} {}",
        created.status, created.body
    );
    tokio::time::sleep(PAST_PROVISIONING).await;

    // Rows: the first and the last entry of the catalogue, so the association
    // is not driven over one hand-picked package.
    let rows = [
        entries.first().expect("the catalogue carries entries"),
        entries.last().expect("the catalogue carries entries"),
    ];
    for entry in rows {
        let name = text(entry, case, "PackageName");
        let id = text(entry, case, "PackageID");
        let associated = call_in(
            &router,
            REGION,
            "POST",
            &format!("/2015-01-01/packages/associate/{id}/{domain_name}"),
            None,
        )
        .await;
        assert_eq!(
            associated.status, 200,
            "case {name:?}: associating a published package must answer 200, got {} {}",
            associated.status, associated.body
        );
        let details = "/DomainPackageDetails";
        assert_eq!(
            associated.at(&name, &format!("{details}/PackageName")),
            &json!(name),
            "case {name:?}: the association must name the package it installed, got {:?}",
            associated.body.pointer(&format!("{details}/PackageName"))
        );
        assert_eq!(
            associated.at(&name, &format!("{details}/PackageType")),
            &json!(ZIP_PLUGIN),
            "case {name:?}: the association must report the package's own type, got {:?}",
            associated.body.pointer(&format!("{details}/PackageType"))
        );
        assert_eq!(
            associated.body.pointer(&format!("{details}/PackageVersion")),
            entry.pointer("/AvailablePackageVersion"),
            "case {name:?}: the association must take the package's available version, got {:?}",
            associated.body.pointer(&format!("{details}/PackageVersion"))
        );

        // The association reads answer from that same record.
        let on_domain = call_in(
            &router,
            REGION,
            "GET",
            &format!("/2015-01-01/domain/{domain_name}/packages"),
            None,
        )
        .await;
        let installed = on_domain
            .at(&name, "/DomainPackageDetailsList")
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {name:?}: DomainPackageDetailsList is not a list, got {}",
                    on_domain.body
                )
            })
            .clone();
        assert!(
            installed
                .iter()
                .any(|each| each.pointer("/PackageID") == Some(&json!(id))),
            "case {name:?}: the domain's package list must carry the association, got \
             {installed:?}"
        );

        let domains = call_in(
            &router,
            REGION,
            "GET",
            &format!("/2015-01-01/packages/{id}/domains"),
            None,
        )
        .await;
        assert_eq!(
            domains.status, 200,
            "case {name:?}: the package's domain list must answer 200, got {} {}",
            domains.status, domains.body
        );
        assert!(
            domains
                .at(&name, "/DomainPackageDetailsList")
                .as_array()
                .is_some_and(|listed| listed
                    .iter()
                    .any(|each| each.pointer("/DomainName") == Some(&json!(domain_name)))),
            "case {name:?}: the package's domain list must carry the domain, got {}",
            domains.body
        );
    }
}

/// RULE: an id the catalogue does not carry is still not found — the catalogue
/// resolves the ids it publishes, not any id at all.
#[tokio::test]
async fn an_id_outside_the_catalogue_is_still_not_found() {
    let router = router();
    let one = published(&describe_packages(&router, &json!({})).await, "catalogue")
        .first()
        .map(|entry| text(entry, "catalogue", "PackageID"))
        .expect("the catalogue carries entries");

    // Rows differ along HOW the id misses: nothing publishes or minted it, and
    // a published id with one character changed.
    let mut mutated = one.clone();
    let last = mutated.pop();
    mutated.push(if last == Some('0') { '1' } else { '0' });
    assert_ne!(
        mutated, one,
        "the mutated id must differ from the published one"
    );
    for (case, id) in [
        ("an id nothing publishes", "pkg-0000000000000000".to_string()),
        ("a published id with its last digit changed", mutated),
    ] {
        let read = history(&router, &id).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: the history must be refused with 404, got {} {}",
            read.status, read.body
        );
        assert!(
            read.error_type.contains("ResourceNotFoundException"),
            "case {case:?}: the error must be ResourceNotFoundException, got {:?} / {}",
            read.error_type,
            read.body
        );
        assert_eq!(
            read.text(case, "/message"),
            format!("Package not found: {id}"),
            "case {case:?}: unexpected message, got {}",
            read.body
        );
    }
}
