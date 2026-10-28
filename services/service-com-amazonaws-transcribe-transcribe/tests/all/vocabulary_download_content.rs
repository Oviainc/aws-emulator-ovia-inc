// SPDX-License-Identifier: CC0-1.0

//! What the `DownloadUri` of a custom vocabulary or vocabulary filter SERVES, end to
//! end over the wire: `CreateVocabulary` / `CreateVocabularyFilter` (and their
//! updates and deletes) followed by the family's reader, and then a plain object-store
//! GET of the location that reader reported.
//!
//! THE RULE THESE ROWS ASSERT: a reported service-managed content location holds the
//! resource's CURRENT content — `DownloadUri` is "the Amazon S3 location where the
//! custom vocabulary filter is stored; use this URI to view or download the custom
//! vocabulary filter" (`smithy_query(query="operation",
//! operation="GetVocabularyFilter")`, and the same sentence for `GetVocabulary`) — so
//! fetching it returns the terms the create or the update supplied, one per line, a
//! later write replaces them, and a resource that holds no built content serves
//! nothing there.
//!
//! WHICH STATE a vocabulary reports the member in is a different suite's subject
//! (`crate::vocabulary_download_uri_state`): this one asserts what is AT the location
//! the member names.
//!
//! The emulator mounted here holds the Amazon S3 service beside Transcribe on ONE
//! shared engine, which is what the assembled router does: the content is stored by
//! calling object storage in process, so a suite that fetches it needs object storage
//! answering. The Transcribe calls go through the same `MultiServiceRouter` dispatcher
//! the transcription-job suite drives (`crate::transcription_jobs::dispatch`), reused
//! rather than stood up a second time.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::transcription_jobs::{dispatch, Reply};

/// How long [`Transcribe::settled_state`] waits between two polls, and how many polls
/// it makes before giving up: together comfortably past one
/// `crate::state::VOCABULARY_TRANSITION_DELAY` (500 ms) even on a slow machine.
const POLL_INTERVAL: Duration = Duration::from_millis(50);
const POLLS_FOR_SETTLED: usize = 40;

/// The state whose documentation makes a custom vocabulary usable, and the only one
/// that reports a `DownloadUri`.
pub(crate) const READY: &str = "READY";

/// One emulator: Transcribe and Amazon S3 on a single shared engine behind the router.
///
/// `pub(crate)`, like the two [`Family`] constants and the helpers below it: the suite
/// that asserts WHOSE content a reported location serves
/// (`crate::vocabulary_download_isolation`) drives its calls and its fetches through
/// this harness instead of standing up a second one.
pub(crate) struct Emulator {
    router: Arc<MultiServiceRouter>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite asserts on service behaviour, not on signature verification, and
        // signs with a fixed scope so the region each row is served in is deterministic.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_transcribe_transcribe::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        Self {
            router: Arc::new(router),
        }
    }

    /// One Transcribe call, signed for `region`.
    pub(crate) async fn call(&self, operation: &str, input: Value, region: &str) -> Reply {
        dispatch(&self.router, operation, input, Some(region)).await
    }

    /// Fetch `uri` — a `DownloadUri` as a reader reported it — from the emulator's
    /// Amazon S3 front door, answering the HTTP status and the raw body.
    ///
    /// The URI is path-style (`https://s3.<region>.amazonaws.com/<bucket>/<key>`), so
    /// what a client resolves to a request line is everything from the host's first
    /// slash. S3 is a REST service: the bucket and the key live in the URI, which is
    /// why this is a raw request rather than an `x-amz-target` call.
    pub(crate) async fn fetch(&self, uri: &str, region: &str) -> (u16, Vec<u8>) {
        let path = uri
            .split_once("://")
            .and_then(|(_, rest)| rest.find('/').map(|at| rest[at..].to_string()))
            .unwrap_or_else(|| panic!("a reported DownloadUri names a path, got {uri:?}"));
        let request = ::http::Request::builder()
            .method("GET")
            .uri(path)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/s3/\
                     aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(::bytes::Bytes::new())
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        (
            response.status().as_u16(),
            response.body().as_ref().to_vec(),
        )
    }

    /// Poll the family's reader until the build of `name` has FINISHED, and answer the
    /// `VocabularyState` it settled in. A family with no build state answers [`READY`]
    /// at once.
    pub(crate) async fn settled_state(&self, family: &Family, name: &str, region: &str) -> String {
        if !family.has_build_state {
            return READY.to_string();
        }
        let case = format!("{} {name:?}", family.get);
        for _ in 0..POLLS_FOR_SETTLED {
            let reply = self
                .call(family.get, json!({family.name_member: name}), region)
                .await;
            let state = reply.member(&case, "VocabularyState");
            if state != "PENDING" {
                return state;
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        panic!("case {case:?}: the build never finished");
    }

    /// The `DownloadUri` the family's reader reports for `name`, if it reports one.
    pub(crate) async fn download_uri(
        &self,
        family: &Family,
        name: &str,
        region: &str,
    ) -> Option<String> {
        let reply = self
            .call(family.get, json!({family.name_member: name}), region)
            .await;
        reply.ok(family.get);
        reply
            .body
            .get("DownloadUri")
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// One of the two families that report a `DownloadUri` for content a request supplies
/// inline, with the member names its operations spell that content with.
pub(crate) struct Family {
    /// The operation that builds the resource.
    pub(crate) create: &'static str,
    /// The operation that rebuilds it from new content.
    pub(crate) update: &'static str,
    /// The family's own reader, the one that reports `DownloadUri`.
    get: &'static str,
    /// The operation that removes it.
    pub(crate) delete: &'static str,
    /// The member naming the resource in every one of those.
    pub(crate) name_member: &'static str,
    /// The member carrying the content inline (`Phrases` / `Words`).
    pub(crate) terms_member: &'static str,
    /// The member naming a file to read the content from instead.
    file_member: &'static str,
    /// Whether the update takes a `LanguageCode`: `UpdateVocabulary` models it
    /// `@required`, `UpdateVocabularyFilter` has no such member at all
    /// (`smithy_query(query="operation",operation="UpdateVocabulary,UpdateVocabularyFilter")`).
    pub(crate) update_takes_language_code: bool,
    /// Whether the family models a build state, so a reader reports the location only
    /// once the build has settled. A vocabulary has `VocabularyState`; a vocabulary
    /// filter models none, so it is usable — and downloadable — from the create.
    has_build_state: bool,
}

/// The custom-vocabulary family.
pub(crate) const VOCABULARY: Family = Family {
    create: "CreateVocabulary",
    update: "UpdateVocabulary",
    get: "GetVocabulary",
    delete: "DeleteVocabulary",
    name_member: "VocabularyName",
    terms_member: "Phrases",
    file_member: "VocabularyFileUri",
    update_takes_language_code: true,
    has_build_state: true,
};

/// The custom-vocabulary-FILTER family.
pub(crate) const VOCABULARY_FILTER: Family = Family {
    create: "CreateVocabularyFilter",
    update: "UpdateVocabularyFilter",
    get: "GetVocabularyFilter",
    delete: "DeleteVocabularyFilter",
    name_member: "VocabularyFilterName",
    terms_member: "Words",
    file_member: "VocabularyFilterFileUri",
    update_takes_language_code: false,
    has_build_state: false,
};

/// One resource built from content a request lists inline, and the content a rebuild
/// replaces it with.
///
/// `pub(crate)` with the request shaping below it ([`create_request`],
/// [`update_request`]): the suite that asserts WHOSE content a reported location serves
/// (`crate::vocabulary_download_isolation`) describes its resources with this same row,
/// so the two families' request shapes live in one place.
pub(crate) struct Row {
    /// What this row is, named in every assertion it fails.
    pub(crate) case: &'static str,
    pub(crate) family: &'static Family,
    /// The Region the row's calls are signed for — which is the Region the reported
    /// location lives in.
    pub(crate) region: &'static str,
    pub(crate) name: &'static str,
    pub(crate) language_code: &'static str,
    /// The content the create supplies.
    pub(crate) terms: Vec<String>,
    /// The content the update supplies instead.
    pub(crate) rebuilt: Vec<String>,
}

/// The class of content this rule generalises over: how MANY terms there are, how long
/// one is, which characters and which of the model's `LanguageCode` values they are in,
/// which of the two families holds them, and which Region serves them. The rebuild of
/// each row differs from its build in the same dimensions, so a row that only happens
/// to serve its first content cannot pass.
///
/// The language codes are entries of the model's own `LanguageCode` enum
/// (`smithy_query(query="shape", shape="LanguageCode")`), including ones the issue did
/// not cite. The two Regions are one that needs no bucket location constraint
/// (`us-east-1`, the one an unspecified constraint stands for) and one that does, so
/// the store the content is written to is exercised both ways.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one word, en-US, in the default Region",
            family: &VOCABULARY_FILTER,
            region: "us-east-1",
            name: "blocked",
            language_code: "en-US",
            terms: vec!["foo".to_string()],
            rebuilt: vec!["baz".to_string(), "qux".to_string()],
        },
        Row {
            case: "several words with non-ASCII characters, fr-FR, in eu-west-1",
            family: &VOCABULARY_FILTER,
            region: "eu-west-1",
            name: "blocked.accented",
            language_code: "fr-FR",
            terms: vec![
                "zèbre".to_string(),
                "gnou".to_string(),
                "hippopotame".to_string(),
            ],
            rebuilt: vec!["åäö".to_string()],
        },
        Row {
            case: "a word at the published 256-character ceiling, de-DE",
            family: &VOCABULARY_FILTER,
            region: "eu-west-1",
            name: "blocked_long-word",
            language_code: "de-DE",
            terms: vec!["a".repeat(256)],
            rebuilt: vec!["b".repeat(256), "kurz".to_string()],
        },
        Row {
            case: "one inline phrase, en-US, in the default Region",
            family: &VOCABULARY,
            region: "us-east-1",
            name: "terms",
            language_code: "en-US",
            terms: vec!["Aardvark".to_string()],
            rebuilt: vec!["Zebra".to_string(), "Gnu".to_string()],
        },
        Row {
            case: "multi-word phrases, en-GB, in eu-west-1",
            family: &VOCABULARY,
            region: "eu-west-1",
            name: "terms.multi-word",
            language_code: "en-GB",
            terms: vec![
                "Amazon Transcribe".to_string(),
                "Amazon Web Services".to_string(),
            ],
            rebuilt: vec!["Elastic Compute Cloud".to_string()],
        },
        Row {
            case: "a long list of phrases, ja-JP, in eu-west-1",
            family: &VOCABULARY,
            region: "eu-west-1",
            name: "terms_many",
            language_code: "ja-JP",
            terms: (0..64).map(|index| format!("項目{index}")).collect(),
            rebuilt: (0..3).map(|index| format!("語{index}")).collect(),
        },
    ]
}

/// The create request of a row.
pub(crate) fn create_request(row: &Row) -> Value {
    json!({
        row.family.name_member: row.name,
        "LanguageCode": row.language_code,
        row.family.terms_member: row.terms,
    })
}

/// The update request of a row, carrying `terms` as its new content.
pub(crate) fn update_request(row: &Row, terms: &[String]) -> Value {
    let mut request = json!({
        row.family.name_member: row.name,
        row.family.terms_member: terms,
    });
    if row.family.update_takes_language_code {
        request["LanguageCode"] = json!(row.language_code);
    }
    request
}

/// The terms a fetched location served, in the order the file lists them.
pub(crate) fn served_terms(case: &str, body: &[u8]) -> Vec<String> {
    let text = std::str::from_utf8(body)
        .unwrap_or_else(|error| panic!("case {case:?}: the served file is not UTF-8: {error}"));
    text.lines().map(str::to_string).collect()
}

/// Fetch the location the family's reader reports for a row, and assert it serves
/// exactly `expected`.
async fn assert_location_serves(emulator: &Emulator, row: &Row, expected: &[String], when: &str) {
    let case = format!("{} ({when})", row.case);
    let state = emulator
        .settled_state(row.family, row.name, row.region)
        .await;
    assert_eq!(
        state, READY,
        "case {case:?}: content supplied inline must build, expected {READY}, got \
         {state:?}"
    );
    let uri = emulator
        .download_uri(row.family, row.name, row.region)
        .await
        .unwrap_or_else(|| {
            panic!("case {case:?}: a built resource must report a DownloadUri, got none")
        });
    assert!(
        uri.contains(row.region) && uri.ends_with(row.name),
        "case {case:?}: a reported DownloadUri names this resource's own location, got \
         {uri:?}"
    );
    let (status, body) = emulator.fetch(&uri, row.region).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the reported DownloadUri {uri:?} must serve the content, got \
         HTTP {status} with body {:?}",
        String::from_utf8_lossy(&body)
    );
    let served = served_terms(&case, &body);
    assert_eq!(
        served, expected,
        "case {case:?}: the location must serve the terms that were stored, expected \
         {expected:?}, got {served:?}"
    );
}

/// Assert nothing is served at `uri`.
async fn assert_location_serves_nothing(emulator: &Emulator, uri: &str, region: &str, case: &str) {
    let (status, body) = emulator.fetch(uri, region).await;
    assert_eq!(
        status, 404,
        "case {case:?}: no content is stored at {uri:?}, so a fetch of it must not \
         serve one, got HTTP {status} with body {:?}",
        String::from_utf8_lossy(&body)
    );
}

/// The content a create supplied is fetchable from the location the reader reports.
#[tokio::test]
async fn the_reported_location_serves_the_content_a_create_supplied() {
    let emulator = Emulator::new();
    for row in &rows() {
        let created = emulator
            .call(row.family.create, create_request(row), row.region)
            .await;
        created.ok(row.case);
        assert_location_serves(&emulator, row, &row.terms, "after the create").await;
    }
}

/// A rebuild replaces what the location serves: the new content is there, and nothing
/// of the old content is.
#[tokio::test]
async fn a_rebuild_replaces_the_content_the_reported_location_serves() {
    let emulator = Emulator::new();
    for row in &rows() {
        emulator
            .call(row.family.create, create_request(row), row.region)
            .await
            .ok(row.case);
        assert_location_serves(&emulator, row, &row.terms, "after the create").await;

        let updated = emulator
            .call(
                row.family.update,
                update_request(row, &row.rebuilt),
                row.region,
            )
            .await;
        updated.ok(row.case);
        assert_location_serves(&emulator, row, &row.rebuilt, "after the rebuild").await;

        // "The new list you provide overwrites all previous entries": nothing of the
        // content the create supplied is left at the location.
        let (_, body) = {
            let uri = emulator
                .download_uri(row.family, row.name, row.region)
                .await
                .expect("a built resource reports a DownloadUri");
            emulator.fetch(&uri, row.region).await
        };
        let served = served_terms(row.case, &body);
        for stale in &row.terms {
            assert!(
                !served.contains(stale),
                "case {:?}: the rebuilt location must not still serve {stale:?}, got \
                 {served:?}",
                row.case
            );
        }
    }
}

/// A resource that holds no built content serves nothing at the location it used to:
/// a rebuild whose content was to be READ from a caller-named file no object store
/// serves reports no `DownloadUri` and leaves nothing behind, and a delete withdraws
/// the content with the resource.
#[tokio::test]
async fn a_resource_with_no_built_content_serves_nothing() {
    let emulator = Emulator::new();

    // Each row is first built from its inline content, so there IS something at its
    // location to withdraw, and then taken away by one of the two paths that leave a
    // resource with no built content.
    for (index, row) in rows().iter().enumerate() {
        emulator
            .call(row.family.create, create_request(row), row.region)
            .await
            .ok(row.case);
        assert_location_serves(&emulator, row, &row.terms, "after the create").await;
        let uri = emulator
            .download_uri(row.family, row.name, row.region)
            .await
            .expect("a built resource reports a DownloadUri");

        // Alternating the two paths keeps either from being the only one exercised.
        if index % 2 == 0 {
            let case = format!("{} (rebuilt from a file)", row.case);
            let mut request = json!({
                row.family.name_member: row.name,
                row.family.file_member: "s3://DOC-EXAMPLE-BUCKET/my-vocab-file.txt",
            });
            if row.family.update_takes_language_code {
                request["LanguageCode"] = json!(row.language_code);
            }
            emulator
                .call(row.family.update, request, row.region)
                .await
                .ok(&case);
            // The rebuild loaded no content, so the family's reader reports no location
            // and the one it used to report serves nothing.
            let state = emulator
                .settled_state(row.family, row.name, row.region)
                .await;
            let reported = emulator
                .download_uri(row.family, row.name, row.region)
                .await;
            assert!(
                reported.is_none(),
                "case {case:?}: a resource whose file the object store does not hold \
                 reports no DownloadUri (state {state:?}), got {reported:?}"
            );
            assert_location_serves_nothing(&emulator, &uri, row.region, &case).await;
        } else {
            let case = format!("{} (deleted)", row.case);
            emulator
                .call(
                    row.family.delete,
                    json!({row.family.name_member: row.name}),
                    row.region,
                )
                .await
                .ok(&case);
            assert_location_serves_nothing(&emulator, &uri, row.region, &case).await;
        }
    }
}
