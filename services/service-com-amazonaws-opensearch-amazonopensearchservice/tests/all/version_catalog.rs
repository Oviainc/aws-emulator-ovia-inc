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
//! * paging is a partition: over a table of page sizes the pages concatenate back to
//!   the whole catalog in order, every page is within its size, only the last page
//!   carries no token, a page size outside the modelled bound is refused at either end,
//!   and a token the operation never issued is refused;
//! * a token names WHERE the catalog resumes and nothing else — the model's
//!   `@paginated(inputToken: NextToken, outputToken: NextToken, pageSize: MaxResults)`
//!   keeps the cursor and the page size separate — so over a table of (issuing page
//!   size, resuming page size) pairs the resumed walk is the catalog's suffix whatever
//!   page size either side used, and resuming from one token twice answers the same
//!   versions;
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

/// The whole catalog, walked over as many pages as `page_size` needs (`0` for the
/// unbounded single page).
async fn whole_catalog(emulator: &Emulator, page_size: i32) -> Vec<String> {
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let query = match (&token, page_size) {
            (None, 0) => String::new(),
            (None, size) => format!("maxResults={size}"),
            (Some(token), 0) => format!("nextToken={token}"),
            (Some(token), size) => format!("maxResults={size}&nextToken={token}"),
        };
        let case = format!("walk page size {page_size} token {token:?}");
        let answer = list(emulator, &query).await;
        walked.extend(versions(&answer, &case));
        token = answer.body["NextToken"].as_str().map(str::to_owned);
        if token.is_none() {
            return walked;
        }
        assert!(
            walked.len() < 10_000,
            "case {case}: paging kept issuing tokens past the end of the catalog"
        );
    }
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
async fn paging_partitions_the_catalog() {
    let emulator = Emulator::new();
    let catalog = whole_catalog(&emulator, 0).await;

    // One row per page size: the smallest, two that do not divide the catalog, and the
    // largest the modelled range allows.
    for page_size in [1_i32, 3, 7, 100] {
        let case = format!("page size {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let query = match &token {
                None => format!("maxResults={page_size}"),
                Some(token) => format!("maxResults={page_size}&nextToken={token}"),
            };
            let answer = list(&emulator, &query).await;
            let page = versions(&answer, &case);
            assert!(
                page.len() <= usize::try_from(page_size).unwrap_or_default(),
                "case {case}: a page holds at most MaxResults versions, got {}",
                page.len()
            );
            assert!(
                !page.is_empty(),
                "case {case}: a page the service issued a token for reports versions, got \
                 none"
            );
            walked.extend(page);
            pages += 1;
            token = answer.body["NextToken"].as_str().map(str::to_owned);
            if token.is_none() {
                break;
            }
            assert!(
                walked.len() <= catalog.len(),
                "case {case}: paging kept issuing tokens past the end of the catalog"
            );
        }
        assert_eq!(
            walked, catalog,
            "case {case}: expected the pages to concatenate back to the whole catalog in \
             order, got {} versions over {pages} pages",
            walked.len()
        );
        let expected_pages = catalog
            .len()
            .div_ceil(usize::try_from(page_size).unwrap_or(1));
        assert_eq!(
            pages, expected_pages,
            "case {case}: expected {expected_pages} pages, got {pages}"
        );
    }

    // `MaxResults` is `@default(0)`, so a request that sends none reports the whole
    // catalog in one page, with no token.
    for unbounded in ["", "maxResults=0"] {
        let case = format!("unbounded {unbounded:?}");
        let answer = list(&emulator, unbounded).await;
        let page = versions(&answer, &case);
        assert_eq!(
            page, catalog,
            "case {case}: expected the whole catalog in one page, got {} versions",
            page.len()
        );
        assert!(
            answer.body["NextToken"].is_null(),
            "case {case}: expected no token on the last page, got {:?}",
            answer.body["NextToken"]
        );
    }

    // A page size outside the modelled bound is refused at either end; the bound itself
    // is not.
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

    // A token the operation never issued is a malformed request, not an empty page —
    // including a pattern-valid engine version the catalog does not publish.
    for invented in [
        "nextToken=not-a-token",
        "nextToken=OpenSearch_9.9",
        "maxResults=5&nextToken=00000000-0000-0000-0000-000000000000",
    ] {
        let answer = list(&emulator, invented).await;
        assert_eq!(
            answer.status, 400,
            "case {invented}: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {invented}: expected the modelled validation refusal, got {:?}",
            answer.body
        );
    }
}

/// The versions a walk reports starting AT `token`, over pages of `page_size` (`0` for
/// the unbounded page), to the end of the catalog.
async fn walk_from(emulator: &Emulator, token: &str, page_size: i32, case: &str) -> Vec<String> {
    let mut walked: Vec<String> = Vec::new();
    let mut cursor = Some(token.to_owned());
    while let Some(token) = cursor {
        let query = if page_size == 0 {
            format!("nextToken={token}")
        } else {
            format!("maxResults={page_size}&nextToken={token}")
        };
        let answer = list(emulator, &query).await;
        walked.extend(versions(&answer, case));
        cursor = answer.body["NextToken"].as_str().map(str::to_owned);
        assert!(
            walked.len() < 10_000,
            "case {case}: paging kept issuing tokens past the end of the catalog"
        );
    }
    walked
}

#[tokio::test]
async fn a_token_resumes_where_the_previous_page_stopped() {
    let emulator = Emulator::new();
    let catalog = whole_catalog(&emulator, 0).await;

    // One row per (page size the token was issued at, page size it is resumed at):
    // the smallest both ways, each crossed with a larger one, a resume that sends no
    // page size at all, and a resume at the largest the modelled range allows.
    for (issued_with, resumed_with) in [
        (1_i32, 1_i32),
        (1, 5),
        (5, 1),
        (2, 0),
        (3, 100),
        (7, 4),
    ] {
        let case = format!("token issued at page size {issued_with}, resumed at {resumed_with}");
        let page_size = usize::try_from(issued_with).unwrap_or_default();
        assert!(
            catalog.len() > page_size,
            "case {case}: this row needs a catalog longer than its page size, got {} versions",
            catalog.len()
        );

        let first = list(&emulator, &format!("maxResults={issued_with}")).await;
        let head = versions(&first, &case);
        assert_eq!(
            head.len(),
            page_size,
            "case {case}: expected a full first page while the catalog holds more, got {head:?}"
        );
        let token = first.body["NextToken"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: expected a token while {} of {} versions are unreported, got \
                     {:?}",
                    catalog.len() - head.len(),
                    catalog.len(),
                    first.body
                )
            });

        // The cursor is a position in the catalog, so the page size used to resume it
        // changes how the suffix is cut into pages and not which versions it reports.
        let tail = walk_from(&emulator, &token, resumed_with, &case).await;
        let rejoined: Vec<String> = head.iter().chain(tail.iter()).cloned().collect();
        assert_eq!(
            rejoined, catalog,
            "case {case}: expected the resumed walk to be the catalog's suffix, got {} versions \
             after a {}-version first page",
            tail.len(),
            head.len()
        );

        // Resuming from one token is a read: the second walk reports the same versions.
        let again = walk_from(&emulator, &token, resumed_with, &case).await;
        assert_eq!(
            again, tail,
            "case {case}: expected one token to answer the same versions twice, got {} versions \
             the second time",
            again.len()
        );
    }
}

#[tokio::test]
async fn reading_the_version_catalog_changes_nothing() {
    let emulator = Emulator::new();
    let first = whole_catalog(&emulator, 0).await;

    // Paged and refused reads, all of which only report.
    list(&emulator, "maxResults=2").await;
    list(&emulator, "maxResults=101").await;
    list(&emulator, "nextToken=not-a-token").await;
    let paged = whole_catalog(&emulator, 1).await;
    assert_eq!(
        paged, first,
        "case walked one at a time: expected the same catalog the unpaged read reported, \
         got {} versions",
        paged.len()
    );

    let second = whole_catalog(&emulator, 0).await;
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
