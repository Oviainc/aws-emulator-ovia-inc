// SPDX-License-Identifier: CC0-1.0

//! WHOSE content the `DownloadUri` of a custom vocabulary or vocabulary filter serves,
//! end to end over the wire: two resources that differ in exactly ONE part of their
//! identity are built from different content, and each reported location is fetched
//! from the emulator's object store.
//!
//! THE RULE THESE ROWS ASSERT: the content location a reader reports holds THAT
//! resource's own current content — `DownloadUri` is "the Amazon S3 location where the
//! custom vocabulary filter is stored; use this URI to view or download the custom
//! vocabulary filter" (`smithy_query(query="operation",
//! operation="GetVocabularyFilter")`, and the same sentence for `GetVocabulary`) — so
//! two resources never share a location: each serves its own terms, a rebuild of one
//! leaves the other's content alone, and a delete of one withdraws only its own.
//!
//! WHAT a location serves for a single resource, and what a resource with no built
//! content serves, is the neighbouring suite's subject
//! (`crate::vocabulary_download_content`); this one asserts that the mapping from a
//! resource to its location is INJECTIVE over the parts of a resource's identity: its
//! name, its family and the Region that holds it. Each pair is driven in BOTH
//! directions, so a location keyed on only one side's identity cannot pass. The
//! harness — one emulator holding Transcribe beside Amazon S3 on a shared engine, the
//! row describing one resource and the two families' request shapes — is that suite's,
//! reused rather than stood up a second time.

use serde_json::json;

use crate::vocabulary_download_content::{
    create_request, served_terms, update_request, Emulator, Row, READY, VOCABULARY,
    VOCABULARY_FILTER,
};

/// Two resources whose identities differ in exactly one part, each holding content of
/// its own.
struct Pair {
    /// The part of the identity the two differ in, named in every assertion a row
    /// fails.
    differs_in: &'static str,
    left: Row,
    right: Row,
}

/// The class this rule generalises over: every part of a resource's identity that its
/// content location is built from. Two resources may share every other part, so each
/// row holds the rest fixed and differs in one — the NAME (same family, same Region),
/// the FAMILY (same name, same Region: a custom vocabulary and a vocabulary filter may
/// both be called the same thing, each family having its own namespace) and the REGION
/// (same name, same family, in two of them, which this regional service keeps apart).
/// The two sides of a row always hold different content, so a row where both read the
/// same location cannot pass.
///
/// The language codes are entries of the model's own `LanguageCode` enum
/// (`smithy_query(query="shape", shape="LanguageCode")`), and the Regions include the
/// one an unspecified bucket location constraint stands for (`us-east-1`) and four
/// that need one.
fn pairs() -> Vec<Pair> {
    vec![
        Pair {
            differs_in: "their names",
            left: Row {
                case: "a vocabulary filter named blocked-alpha in eu-central-1",
                family: &VOCABULARY_FILTER,
                region: "eu-central-1",
                name: "blocked-alpha",
                language_code: "de-DE",
                terms: vec!["eins".to_string(), "zwei".to_string()],
                rebuilt: vec!["drei".to_string()],
            },
            right: Row {
                case: "a vocabulary filter named blocked-beta in eu-central-1",
                family: &VOCABULARY_FILTER,
                region: "eu-central-1",
                name: "blocked-beta",
                language_code: "de-DE",
                terms: vec!["vier".to_string()],
                rebuilt: vec!["fünf".to_string(), "sechs".to_string()],
            },
        },
        Pair {
            differs_in: "their names",
            left: Row {
                case: "a custom vocabulary named terms-alpha in us-east-1",
                family: &VOCABULARY,
                region: "us-east-1",
                name: "terms-alpha",
                language_code: "en-US",
                terms: vec!["Aardvark".to_string(), "Zebra".to_string()],
                rebuilt: vec!["Gnu".to_string(), "Okapi".to_string(), "Ibex".to_string()],
            },
            right: Row {
                case: "a custom vocabulary named terms-beta in us-east-1",
                family: &VOCABULARY,
                region: "us-east-1",
                name: "terms-beta",
                language_code: "en-US",
                terms: vec!["Narwhal".to_string()],
                rebuilt: vec!["Amazon Transcribe".to_string()],
            },
        },
        Pair {
            differs_in: "their families",
            left: Row {
                case: "a custom vocabulary named shared-name in us-west-2",
                family: &VOCABULARY,
                region: "us-west-2",
                name: "shared-name",
                language_code: "en-US",
                terms: vec!["Amazon Transcribe".to_string(), "Okapi".to_string()],
                rebuilt: vec!["Elastic Compute Cloud".to_string()],
            },
            right: Row {
                case: "a vocabulary filter named shared-name in us-west-2",
                family: &VOCABULARY_FILTER,
                region: "us-west-2",
                name: "shared-name",
                language_code: "en-US",
                terms: vec!["foo".to_string(), "bar".to_string(), "baz".to_string()],
                rebuilt: vec!["qux".to_string()],
            },
        },
        Pair {
            differs_in: "their Regions",
            left: Row {
                case: "the vocabulary filter blocked.regional in us-east-1",
                family: &VOCABULARY_FILTER,
                region: "us-east-1",
                name: "blocked.regional",
                language_code: "en-US",
                terms: vec!["east".to_string()],
                rebuilt: vec!["east-again".to_string(), "east-still".to_string()],
            },
            right: Row {
                case: "the vocabulary filter blocked.regional in ap-northeast-1",
                family: &VOCABULARY_FILTER,
                region: "ap-northeast-1",
                name: "blocked.regional",
                language_code: "ja-JP",
                terms: vec!["西".to_string(), "東".to_string()],
                rebuilt: vec!["北".to_string()],
            },
        },
        Pair {
            differs_in: "their Regions",
            left: Row {
                case: "the custom vocabulary terms_regional in eu-west-1",
                family: &VOCABULARY,
                region: "eu-west-1",
                name: "terms_regional",
                language_code: "en-GB",
                terms: vec!["Lorry".to_string(), "Pavement".to_string()],
                rebuilt: vec!["Aubergine".to_string()],
            },
            right: Row {
                case: "the custom vocabulary terms_regional in ap-northeast-1",
                family: &VOCABULARY,
                region: "ap-northeast-1",
                name: "terms_regional",
                language_code: "ja-JP",
                terms: (0..8).map(|index| format!("項目{index}")).collect(),
                rebuilt: (0..3).map(|index| format!("語{index}")).collect(),
            },
        },
    ]
}

impl Pair {
    /// The pair seen from each side in turn: the resource a test MUTATES, and the one
    /// it must leave alone. A location keyed on only one side's identity passes one
    /// orientation and fails the other.
    fn orientations(&self) -> [(&Row, &Row); 2] {
        [(&self.left, &self.right), (&self.right, &self.left)]
    }
}

/// Build `row` and answer the location its family's reader reports for it, once the
/// build has settled.
async fn build(emulator: &Emulator, row: &Row) -> String {
    emulator
        .call(row.family.create, create_request(row), row.region)
        .await
        .ok(row.case);
    let state = emulator.settled_state(row.family, row.name, row.region).await;
    assert_eq!(
        state, READY,
        "case {:?}: content supplied inline must build, expected {READY}, got {state:?}",
        row.case
    );
    emulator
        .download_uri(row.family, row.name, row.region)
        .await
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: a built resource must report a DownloadUri, got none",
                row.case
            )
        })
}

/// Assert the location `uri`, reported for `row`, serves exactly `expected`.
async fn assert_serves(emulator: &Emulator, row: &Row, uri: &str, expected: &[String]) {
    let (status, body) = emulator.fetch(uri, row.region).await;
    assert_eq!(
        status, 200,
        "case {:?}: the reported DownloadUri {uri:?} must serve the content, got HTTP \
         {status} with body {:?}",
        row.case,
        String::from_utf8_lossy(&body)
    );
    let served = served_terms(row.case, &body);
    assert_eq!(
        served, expected,
        "case {:?}: the location must serve this resource's own terms, expected \
         {expected:?}, got {served:?}",
        row.case
    );
}

/// Neither side of a collision reads the other's location: the two locations differ and
/// each serves its own content.
#[tokio::test]
async fn resources_that_collide_serve_their_own_content() {
    let emulator = Emulator::new();
    for pair in &pairs() {
        let left = build(&emulator, &pair.left).await;
        let right = build(&emulator, &pair.right).await;

        assert_ne!(
            left, right,
            "case {:?} and {:?}: two resources differing in {} must report different \
             locations, both got {left:?}",
            pair.left.case, pair.right.case, pair.differs_in
        );
        assert_serves(&emulator, &pair.left, &left, &pair.left.terms).await;
        assert_serves(&emulator, &pair.right, &right, &pair.right.terms).await;
    }
}

/// A rebuild replaces only the content of the resource it names: the other side of the
/// collision still serves what it was built from.
#[tokio::test]
async fn a_rebuild_leaves_the_other_resources_content_alone() {
    for pair in &pairs() {
        for (rebuilt, untouched) in pair.orientations() {
            // One emulator per orientation, so neither direction is driven against a
            // store the other already changed.
            let emulator = Emulator::new();
            let rebuilt_uri = build(&emulator, rebuilt).await;
            let untouched_uri = build(&emulator, untouched).await;

            emulator
                .call(
                    rebuilt.family.update,
                    update_request(rebuilt, &rebuilt.rebuilt),
                    rebuilt.region,
                )
                .await
                .ok(rebuilt.case);
            let state = emulator
                .settled_state(rebuilt.family, rebuilt.name, rebuilt.region)
                .await;
            assert_eq!(
                state, READY,
                "case {:?}: a rebuild from content supplied inline must build, expected \
                 {READY}, got {state:?}",
                rebuilt.case
            );

            assert_serves(&emulator, rebuilt, &rebuilt_uri, &rebuilt.rebuilt).await;
            assert_serves(&emulator, untouched, &untouched_uri, &untouched.terms).await;
        }
    }
}

/// A delete withdraws only the content of the resource it names: the other side of the
/// collision is still served.
#[tokio::test]
async fn a_delete_withdraws_only_its_own_content() {
    for pair in &pairs() {
        for (deleted, kept) in pair.orientations() {
            let emulator = Emulator::new();
            let deleted_uri = build(&emulator, deleted).await;
            let kept_uri = build(&emulator, kept).await;

            emulator
                .call(
                    deleted.family.delete,
                    json!({deleted.family.name_member: deleted.name}),
                    deleted.region,
                )
                .await
                .ok(deleted.case);

            let (status, body) = emulator.fetch(&deleted_uri, deleted.region).await;
            assert_eq!(
                status, 404,
                "case {:?}: a deleted resource's content is withdrawn, so a fetch of \
                 {deleted_uri:?} must not serve one, got HTTP {status} with body {:?}",
                deleted.case,
                String::from_utf8_lossy(&body)
            );
            assert_serves(&emulator, kept, &kept_uri, &kept.terms).await;
        }
    }
}
