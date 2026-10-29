// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListVersions`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds no second copy of it.
//!
//! The tests assert RULES over sets of inputs rather than one recorded call:
//!
//! * the answer is the engine-version catalog the service publishes, whole and in one
//!   order, every entry a value `VersionString`'s pattern admits, spanning both engine
//!   lines and several majors and minors of each, with no entry twice;
//! * it is the SAME catalog every other version-aware operation reports — the source
//!   versions of the unfiltered `GetCompatibleVersions` table, and a version
//!   `CreateDomain` accepts for a domain;
//! * the catalog is reported COMPLETE in a single answer, with no `NextToken` member at
//!   all, over a table of the ways a request can ask for a bounded or a resumed answer:
//!   `MaxResults` does not bound what is reported and a `NextToken` neither resumes it
//!   nor is ever handed out, while a `MaxResults` outside the modelled range is still
//!   refused at either end;
//! * the read changes nothing a later caller sees.

use std::collections::BTreeSet;

use serde_json::json;

use super::domain_lifecycle::{error_code, Answer, Emulator, PREFIX};

/// `ListVersions`: `GET /2021-01-01/opensearch/versions`, with `query` as its
/// `@httpQuery` members (empty for a request that sends none).
async fn list(emulator: &Emulator, query: &str) -> Answer {
    let uri = if query.is_empty() {
        format!("{PREFIX}/opensearch/versions")
    } else {
        format!("{PREFIX}/opensearch/versions?{query}")
    };
    emulator.call("GET", &uri, None).await
}

/// The `Versions` list of a successful answer, with the case named on failure.
fn versions(answer: &Answer, case: &str) -> Vec<String> {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected HTTP 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.body["Versions"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a Versions array, got {:?}", answer.body))
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a string version, got {value:?}"))
                .to_owned()
        })
        .collect()
}

/// The whole catalog as the answer to `query` reports it, asserting that the answer
/// carries no continuation token.
///
/// This operation reports the published catalog complete in a single answer — the
/// recorded answers carry every supported version and no `NextToken`, including for a
/// request that named a smaller `MaxResults` or carried a `NextToken` — so there is
/// never a page to walk.
async fn catalog_for(emulator: &Emulator, query: &str) -> Vec<String> {
    let case = format!("query {query:?}");
    let answer = list(emulator, query).await;
    let reported = versions(&answer, &case);
    assert_eq!(
        answer.body.get("NextToken"),
        None,
        "case {case}: the whole catalog is reported, so the answer carries no NextToken, got {:?}",
        answer.body
    );
    reported
}

/// The engine line and `(major, minor)` of a version, or `None` when the value is not
/// one `VersionString`'s pattern admits. Source: that shape's own pattern,
/// `^Elasticsearch_[0-9]{1}\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\.[0-9]{1,2}$` — so a
/// version is an engine name, an underscore, and a two-field numeric release.
fn parse_version(version: &str) -> Option<(&'static str, u32, u32)> {
    let (engine, release) = version.split_once('_')?;
    let engine = match engine {
        "Elasticsearch" => "Elasticsearch",
        "OpenSearch" => "OpenSearch",
        _ => return None,
    };
    let (major, minor) = release.split_once('.')?;
    let major_digits = if engine == "Elasticsearch" { 1..=1 } else { 1..=2 };
    if !major_digits.contains(&major.len()) || !(1..=2).contains(&minor.len()) {
        return None;
    }
    if !major.bytes().chain(minor.bytes()).all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some((engine, major.parse().ok()?, minor.parse().ok()?))
}

#[tokio::test]
async fn reports_the_published_engine_version_catalog() {
    let emulator = Emulator::new();
    let catalog = versions(&list(&emulator, "").await, "unpaged");

    assert!(
        catalog.len() >= 2,
        "case catalog size: expected the published catalog, got {catalog:?}"
    );

    // Every entry is a value the model's `VersionString` admits, and no entry appears
    // twice: the answer is a catalog, not a bag.
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for version in &catalog {
        assert!(
            parse_version(version).is_some(),
            "case {version}: expected a VersionString-shaped engine version, got {version:?}"
        );
        assert!(
            seen.insert(version.as_str()),
            "case {version}: expected each version once, got it twice in {catalog:?}"
        );
    }

    // The catalog spans the relation it exists to publish: both engine lines, several
    // majors of each, and several minors of one major — so a caller can tell an upgrade
    // forwards from one that goes nowhere.
    for engine in ["Elasticsearch", "OpenSearch"] {
        let parsed: Vec<(u32, u32)> = catalog
            .iter()
            .filter_map(|version| parse_version(version))
            .filter(|(line, _, _)| *line == engine)
            .map(|(_, major, minor)| (major, minor))
            .collect();
        let majors: BTreeSet<u32> = parsed.iter().map(|(major, _)| *major).collect();
        assert!(
            majors.len() >= 2,
            "case {engine}: expected several majors published, got {parsed:?}"
        );
        let shared_major = majors
            .iter()
            .find(|major| parsed.iter().filter(|(m, _)| m == *major).count() >= 2);
        assert!(
            shared_major.is_some(),
            "case {engine}: expected a major with several minors published, got {parsed:?}"
        );
    }

    // It is the catalog the OTHER version-aware operations report, not a second copy:
    // the unfiltered `GetCompatibleVersions` table has one entry per published version,
    // in the same order.
    let compatible = emulator
        .call("GET", &format!("{PREFIX}/opensearch/compatibleVersions"), None)
        .await;
    let sources: Vec<String> = compatible.body["CompatibleVersions"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case compatibleVersions: expected a CompatibleVersions array, got {:?}",
                compatible.body
            )
        })
        .iter()
        .map(|entry| {
            entry["SourceVersion"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    assert_eq!(
        sources, catalog,
        "case compatibleVersions: expected the same published versions ListVersions \
         reports, got {sources:?}"
    );

    // A published version is one a domain can actually be created on — tested on the
    // oldest and the newest the catalog carries, neither of which the catalog's own
    // listing order makes special.
    for version in [
        catalog.first().expect("the catalog is not empty").clone(),
        catalog.last().expect("the catalog is not empty").clone(),
    ] {
        let name = format!("cat-{}", version.to_lowercase().replace(['_', '.'], "-"));
        let created = emulator
            .create(json!({ "DomainName": name, "EngineVersion": version }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {version}: expected a domain to be created on a published version, got \
             {} {:?}",
            created.status, created.body
        );
        let reported = created.body["DomainStatus"]["EngineVersion"]
            .as_str()
            .unwrap_or_default();
        assert_eq!(
            reported, version,
            "case {version}: expected the domain to run the published version, got \
             {reported:?}"
        );
    }
}

#[tokio::test]
async fn the_whole_catalog_is_reported_whatever_the_pagination_members_ask() {
    let emulator = Emulator::new();
    let catalog = catalog_for(&emulator, "").await;
    assert!(
        catalog.len() > 7,
        "case catalog size: these rows need a catalog longer than the page sizes they name, \
         got {catalog:?}"
    );

    // One row per way a request can ask for a bounded or a resumed answer: the smallest
    // page size, sizes that divide the catalog and sizes that do not, the largest the
    // modelled range allows, the modelled default of zero, a token naming a published
    // version, a token naming nothing of the catalog, and both members at once. The
    // recorded answers of this operation report the complete catalog and no `NextToken`
    // for such a request, so every row is answered with the whole catalog: the published
    // catalog is reported complete in a single answer, and neither member bounds it nor
    // resumes it.
    for query in [
        "maxResults=1",
        "maxResults=3",
        "maxResults=7",
        "maxResults=100",
        "maxResults=0",
        "nextToken=OpenSearch_1.0",
        "nextToken=not-a-token",
        "maxResults=2&nextToken=Elasticsearch_6.0",
    ] {
        let reported = catalog_for(&emulator, query).await;
        assert_eq!(
            reported, catalog,
            "case query {query:?}: expected the whole catalog in one answer, got {} versions",
            reported.len()
        );
    }

    // A page size outside the modelled bound is still refused at either end — the bound
    // is the model's `@range(max: 100)` on `MaxResults`, and a negative count is no page
    // size at all; the bound itself is not refused.
    let at_bound = list(&emulator, "maxResults=100").await;
    assert_eq!(
        at_bound.status, 200,
        "case maxResults=100: the published bound is accepted, got {} {:?}",
        at_bound.status, at_bound.body
    );
    for out_of_bound in ["maxResults=101", "maxResults=1000", "maxResults=-1"] {
        let answer = list(&emulator, out_of_bound).await;
        assert_eq!(
            answer.status, 400,
            "case {out_of_bound}: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {out_of_bound}: expected the modelled validation refusal, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn reading_the_version_catalog_changes_nothing() {
    let emulator = Emulator::new();
    let first = catalog_for(&emulator, "").await;

    // Bounded, resumed and refused reads, all of which only report.
    list(&emulator, "maxResults=2").await;
    list(&emulator, "maxResults=101").await;
    list(&emulator, "nextToken=not-a-token").await;
    let bounded = catalog_for(&emulator, "maxResults=1").await;
    assert_eq!(
        bounded, first,
        "case a request naming one result: expected the same catalog, got {} versions",
        bounded.len()
    );

    let second = catalog_for(&emulator, "").await;
    assert_eq!(
        first, second,
        "case repeat read: expected the same catalog the first read reported, got {} \
         versions",
        second.len()
    );

    let store = emulator.store();
    let state = store.lock();
    assert!(
        state.domains.is_empty(),
        "case stored domains: reporting engine versions creates no domain, got {:?}",
        state.domains.keys().collect::<Vec<_>>()
    );
}
