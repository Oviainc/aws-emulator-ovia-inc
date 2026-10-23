// SPDX-License-Identifier: CC0-1.0

//! `ListKafkaVersions` — the Apache Kafka version catalog, driven end to end over
//! the same `MultiServiceRouter` the emulator serves from (the shared harness in
//! `tests/all/kafka_harness.rs`).
//!
//! Each suite asserts a RULE over a table of inputs rather than one recorded
//! request: the catalog's own entries (version spelling and support status), every
//! page size the model's `MaxResults` range allows and one past each of its
//! published bounds, and several shapes of invalid pagination token. Where the rule
//! is an invariant — the pages of any walk partitioning the catalog exactly once,
//! the catalog being independent of the caller — the input space is walked
//! EXHAUSTIVELY over the page sizes that can change the partition, which is
//! stronger than sampling it.

use aws_smithy_types::base64;
use serde_json::Value;
use service_com_amazonaws_kafka_kafka::ops::types::KAFKA_VERSION_STATUS_VALUES;
use service_com_amazonaws_kafka_kafka::state::{kafka_version_index, KAFKA_VERSION_CATALOG};

use crate::kafka_harness::{pct, Emulator, Reply};

/// `MaxResults`' published range on `ListKafkaVersions` (`MaxResults`, 1..100 in
/// the Smithy model).
const MAX_RESULTS_MIN: usize = 1;
const MAX_RESULTS_MAX: usize = 100;

/// One `ListKafkaVersions` call. `query` is the raw query string, `""` for none.
async fn list_versions(emulator: &Emulator, query: &str) -> Reply {
    emulator
        .call("GET", &format!("/v1/kafka-versions{query}"), None)
        .await
}

/// The `(version, status)` pairs a reply reports, in the order it reported them.
fn reported(reply: &Reply) -> Vec<(String, String)> {
    reply
        .at("/kafkaVersions")
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    (
                        entry["version"].as_str().unwrap_or_default().to_string(),
                        entry["status"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The whole catalog as the service stores it, in catalog order.
fn catalog() -> Vec<(String, String)> {
    KAFKA_VERSION_CATALOG
        .iter()
        .map(|entry| (entry.version.to_string(), entry.status.to_string()))
        .collect()
}

/// The status the stored catalog gives `version`, if it holds it at all — looked
/// up the way the handler resolves a version.
fn stored_status(version: &str) -> Option<&'static str> {
    kafka_version_index(version).map(|index| KAFKA_VERSION_CATALOG[index].status)
}

/// Walk every page of `ListKafkaVersions` at `page_size`, asserting the per-page
/// rules, and hand back the concatenation of the pages.
async fn walk_pages(emulator: &Emulator, page_size: usize) -> Vec<(String, String)> {
    let mut seen: Vec<(String, String)> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        pages += 1;
        assert!(
            pages <= KAFKA_VERSION_CATALOG.len() + 1,
            "case maxResults={page_size}: pagination did not end after {pages} pages"
        );
        let query = match &token {
            Some(value) => format!("?maxResults={page_size}&nextToken={}", pct(value)),
            None => format!("?maxResults={page_size}"),
        };
        let reply = list_versions(emulator, &query).await;
        assert_eq!(
            reply.status, 200,
            "case maxResults={page_size}: expected 200, got {} {}",
            reply.status, reply.body
        );
        let page = reported(&reply);
        assert!(
            page.len() <= page_size,
            "case maxResults={page_size}: page carried {} entries",
            page.len()
        );
        seen.extend(page.iter().cloned());
        match reply.body.get("nextToken").and_then(Value::as_str) {
            Some(next) => {
                assert!(
                    !page.is_empty(),
                    "case maxResults={page_size}: an empty page must close the list"
                );
                assert_eq!(
                    page.len(),
                    page_size,
                    "case maxResults={page_size}: a short page must be the last one, got {page:?}"
                );
                token = Some(next.to_string());
            }
            None => break,
        }
    }
    seen
}

/// RULE: `ListKafkaVersions` reports the service's whole Apache Kafka version
/// catalog, each entry carrying its version string and a `KafkaVersionStatus`
/// support status, in one fixed order.
#[tokio::test]
async fn the_catalog_is_reported_with_every_version_and_its_support_status() {
    let emulator = Emulator::new();
    let reply = list_versions(&emulator, "").await;
    assert_eq!(
        reply.status, 200,
        "an unparameterised ListKafkaVersions must succeed, got {} {}",
        reply.status, reply.body
    );
    let versions = reported(&reply);
    assert_eq!(
        versions,
        catalog(),
        "the default page must report the whole catalog in catalog order, got {versions:?}"
    );
    // The catalog fits one default page today (its length is below the model's
    // `MaxResults` ceiling), so an unparameterised call must close the list.
    assert!(
        KAFKA_VERSION_CATALOG.len() <= MAX_RESULTS_MAX,
        "this assertion assumes a catalog of at most {MAX_RESULTS_MAX} entries, got {}",
        KAFKA_VERSION_CATALOG.len()
    );
    assert!(
        reply.body.get("nextToken").is_none() || reply.body["nextToken"].is_null(),
        "a catalog that fits one page must close the list, got {}",
        reply.body
    );

    // Rows hand-written from the Amazon MSK developer guide's "Supported Apache
    // Kafka versions" list — entries of several SHAPES (three numbers, a fourth
    // component, the `.tiered` variant and the `.x` spelling) and both support
    // statuses, so the table spans the class rather than one example.
    let expected: &[(&str, &str)] = &[
        ("1.1.1", "DEPRECATED"),
        ("2.4.1.1", "DEPRECATED"),
        ("2.8.0", "DEPRECATED"),
        ("2.8.1", "ACTIVE"),
        ("2.8.2.tiered", "ACTIVE"),
        ("3.5.1", "ACTIVE"),
        ("3.6.0", "ACTIVE"),
        ("3.7.x", "ACTIVE"),
        ("3.9.x", "ACTIVE"),
    ];
    for (version, status) in expected {
        let found = versions.iter().find(|(name, _)| name == version);
        assert_eq!(
            found.map(|(_, status)| status.as_str()),
            Some(*status),
            "case version={version:?}: expected status {status:?}, got {found:?}"
        );
        assert_eq!(
            stored_status(version),
            Some(*status),
            "case version={version:?}: the catalog lookup must agree with the wire"
        );
    }

    // No version the service does not offer is reported, and the lookup agrees.
    for absent in ["0.10.2.1", "2.0.0", "9.9.9", "3.9.x.kraft", "", "latest"] {
        assert!(
            !versions.iter().any(|(name, _)| name == absent),
            "case version={absent:?}: must not appear in the catalog, got {versions:?}"
        );
        assert!(
            stored_status(absent).is_none(),
            "case version={absent:?}: the catalog lookup must not resolve it"
        );
    }

    // Every reported entry is well formed: a modelled status value, a non-empty
    // version, and no duplicates.
    let mut names: Vec<&str> = versions.iter().map(|(name, _)| name.as_str()).collect();
    let total = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(
        names.len(),
        total,
        "the catalog must not report a version twice, got {versions:?}"
    );
    for (name, status) in &versions {
        assert!(
            !name.is_empty(),
            "case entry={name:?}: a reported version must be non-empty"
        );
        assert!(
            KAFKA_VERSION_STATUS_VALUES.contains(&status.as_str()),
            "case version={name:?}: status {status:?} is not a modelled KafkaVersionStatus value"
        );
    }
}

/// RULE: the reported order is the catalog's own ascending version order —
/// components compared numerically where they are numbers, so `2.4.1` precedes
/// `2.4.1.1` and `2.8.1` precedes `3.1.1`.
#[tokio::test]
async fn the_catalog_is_ordered_by_ascending_version() {
    /// A version string as comparable components: a numeric component sorts
    /// numerically and before a textual one (`3.7.x` after `3.7.1`), and a
    /// prefix sorts before its extension (`3.7.x` before `3.7.x.kraft`).
    fn key(version: &str) -> Vec<(u32, u32, String)> {
        version
            .split('.')
            .map(|part| match part.parse::<u32>() {
                Ok(number) => (0, number, String::new()),
                Err(_) => (1, 0, part.to_string()),
            })
            .collect()
    }

    let emulator = Emulator::new();
    let versions = reported(&list_versions(&emulator, "").await);
    for pair in versions.windows(2) {
        let (earlier, later) = (&pair[0].0, &pair[1].0);
        assert!(
            key(earlier) < key(later),
            "case pair=({earlier:?}, {later:?}): the catalog must ascend, got {versions:?}"
        );
    }
}

/// RULE: paging the catalog is a stable partition — every entry appears exactly
/// once across the pages in catalog order, no page is longer than `MaxResults`,
/// and `NextToken` is present exactly while more entries remain.
#[tokio::test]
async fn pagination_is_a_stable_partition_of_the_catalog() {
    let emulator = Emulator::new();
    let whole = catalog();

    // Exhaustive over the page sizes that can change the partition: every size
    // from the model's lower bound up to one past the catalog's length (so a page
    // smaller than, dividing, equal to and larger than the result count), plus
    // the published upper bound.
    let sizes: Vec<usize> = (MAX_RESULTS_MIN..=whole.len() + 1)
        .chain(std::iter::once(MAX_RESULTS_MAX))
        .collect();
    for page_size in sizes {
        let seen = walk_pages(&emulator, page_size).await;
        assert_eq!(
            seen, whole,
            "case maxResults={page_size}: the pages did not cover the catalog exactly once"
        );
    }

    // The partition is stable across walks: a second walk at a different page
    // size reports the same sequence, because nothing a read does changes the
    // catalog.
    let again = walk_pages(&emulator, 3).await;
    assert_eq!(
        again, whole,
        "a repeated walk must report the same catalog, got {again:?}"
    );
}

/// RULE: a `MaxResults` outside the modelled 1..100 range is refused as a client
/// error; both published bounds are served.
#[tokio::test]
async fn a_page_size_past_the_modelled_bounds_is_refused() {
    let emulator = Emulator::new();
    for bound in [MAX_RESULTS_MIN, MAX_RESULTS_MAX] {
        let reply = list_versions(&emulator, &format!("?maxResults={bound}")).await;
        assert_eq!(
            reply.status, 200,
            "case maxResults={bound}: a published bound must be served, got {} {}",
            reply.status, reply.body
        );
        assert!(
            !reported(&reply).is_empty(),
            "case maxResults={bound}: a served page must carry entries"
        );
    }
    for past in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
        let reply = list_versions(&emulator, &format!("?maxResults={past}")).await;
        assert_eq!(
            reply.status, 400,
            "case maxResults={past}: a value past the modelled range must be refused, got {} {}",
            reply.status, reply.body
        );
    }
}

/// RULE: a `NextToken` this operation did not mint — unparseable, minted for
/// another list, or naming a version the catalog does not hold — is refused as a
/// bad request rather than served as a first page.
#[tokio::test]
async fn an_invalid_pagination_token_is_refused() {
    let emulator = Emulator::new();

    // Rows: not base64 at all; the empty token; valid base64 of unrelated bytes;
    // a token minted for `ListClusters`; a well-formed token of this list naming a
    // version the catalog does not hold.
    let rows: Vec<String> = vec![
        "!!!not-base64!!!".to_string(),
        String::new(),
        base64::encode("hello world"),
        base64::encode("kafka:clusters:arn:aws:kafka:us-west-2:000000000000:cluster/solo/abc-1"),
        base64::encode("kafka:kafka-versions:9.9.9"),
        base64::encode("kafka:kafka-versions:"),
    ];
    for token in &rows {
        let reply = list_versions(&emulator, &format!("?nextToken={}", pct(token))).await;
        assert_eq!(
            reply.status, 400,
            "case token={token:?}: expected a 400, got {} {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "BadRequestException",
            "case token={token:?}: expected BadRequestException, got {}",
            reply.body
        );
    }

    // And a token this operation DID mint resumes the walk.
    let first = list_versions(&emulator, "?maxResults=2").await;
    let token = first.body["nextToken"]
        .as_str()
        .expect("a page shorter than the catalog must carry a nextToken")
        .to_string();
    let second = list_versions(
        &emulator,
        &format!("?maxResults=2&nextToken={}", pct(&token)),
    )
    .await;
    assert_eq!(
        reported(&second),
        catalog()[2..4.min(catalog().len())].to_vec(),
        "a minted token must resume at the next catalog entry, got {}",
        second.body
    );
}

/// RULE: the version catalog is service-wide metadata, so it does not depend on
/// the caller's region and a read never changes what a later caller sees.
#[tokio::test]
async fn the_catalog_is_the_same_for_every_caller_and_every_call() {
    let emulator = Emulator::new();
    let whole = catalog();
    for region in ["us-west-2", "eu-west-1", "ap-southeast-2", "us-gov-west-1"] {
        // Two calls per region: the second must see exactly what the first did.
        for attempt in 1..=2 {
            let reply = emulator
                .call_in(region, "GET", "/v1/kafka-versions", None)
                .await;
            assert_eq!(
                reply.status, 200,
                "case region={region} attempt={attempt}: expected 200, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reported(&reply),
                whole,
                "case region={region} attempt={attempt}: the catalog must not vary"
            );
        }
    }
}
