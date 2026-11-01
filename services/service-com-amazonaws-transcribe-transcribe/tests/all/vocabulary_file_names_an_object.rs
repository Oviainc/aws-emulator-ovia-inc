// SPDX-License-Identifier: CC0-1.0

//! The file a create or an update names as the source of a custom vocabulary's,
//! medical vocabulary's or vocabulary filter's terms has to BE THERE, end to end over
//! the wire with the emulator's own Amazon S3 mounted beside Transcribe on one engine —
//! the way the assembled emulator runs them.
//!
//! THE RULE THESE ROWS ASSERT: a request that names an Amazon S3 object as the source
//! of its terms has that object RESOLVED as part of validating the request ("you must
//! first upload a text file that contains your vocabulary table into an Amazon S3
//! bucket" — the file is the caller's, uploaded before calling), so a request whose
//! object the store reports it does not hold is refused SYNCHRONOUSLY with the
//! operation's modelled `BadRequestException` (HTTP 400) and NOTHING is created or
//! changed: the name a refused create asked for is still free, and a refused update
//! leaves the resource exactly as it was. The resource is not accepted and then driven
//! to `FAILED` instead.
//!
//! A refusal has to be GROUNDED in what the object store answered, so the rows include
//! the two counter-cases: an object the store DOES serve is accepted and built from,
//! and a location no object store can be asked about (an `http(s)` URI naming a host,
//! which this emulator resolves no hosts for) is accepted too — nothing about it is
//! known, so nothing is refused. What a build settles in once accepted is
//! `crate::vocabulary_file_objects`' subject.
//!
//! The rows are a TABLE spanning what the rule generalises over: which family takes the
//! file (`CreateVocabulary`/`UpdateVocabulary`, the medical pair whose terms can come
//! from nowhere else, and the filter pair, which models no build state at all), which
//! member names it (`VocabularyFileUri` / `VocabularyFilterFileUri`), whether the
//! request creates or rebuilds, and in WHICH way the address names no object (a key the
//! bucket does not hold, a bucket the store does not hold, a key that only prefixes a
//! stored one, a key that extends one, a key under a prefix nothing was staged under, a
//! URI naming only a bucket, and one whose key part is empty).
//!
//! The harness is the media-resolution suite's (`crate::media_object_resolution`): its
//! emulator with both services on one engine, its Transcribe call and its object
//! staging, reused rather than stood up a second time.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::media_object_resolution::{call, emulator, given_object, ABSENT_BUCKET};
use crate::transcription_jobs::Reply;

/// How long a poll of a family's reader waits between two calls, and how many calls it
/// makes before giving up: together comfortably past one
/// `crate::state::VOCABULARY_TRANSITION_DELAY` (500 ms) even on a slow machine.
const POLL_INTERVAL: Duration = Duration::from_millis(50);
const POLLS_FOR_SETTLED: usize = 40;

/// The refusal a request naming an object the store holds none of gets, as Amazon
/// Transcribe words it — fixed text carrying none of the request's own values.
const URI_NAMES_NO_OBJECT: &str =
    "The URI that you provided doesn't point to an S3 object. Make sure that the object \
     exists and try your request again.";

/// The caller's own bucket, created and uploaded to before calling.
const FILE_BUCKET: &str = "example-vocabulary-term-files";

/// The staged object every accepted row reads its terms from: one at the bucket root,
/// one under a nested prefix.
const STAGED_KEYS: &[&str] = &["terms.txt", "tables/clinic/terms.txt"];

/// What each staged object holds — one entry per line, the form every member that names
/// such a file documents.
const FILE_BYTES: &[u8] = b"acetaminophen\nbronchoscopy\nelectrocardiogram\n";

/// The terms [`FILE_BYTES`] holds, in the order the file lists them.
fn staged_terms() -> Vec<String> {
    vec![
        "acetaminophen".to_string(),
        "bronchoscopy".to_string(),
        "electrocardiogram".to_string(),
    ]
}

/// `en-US`: a value of the service-wide `LanguageCode` enum
/// (`smithy_query(query="shape", shape="LanguageCode")`) every family accepts, and the
/// only one Amazon Transcribe Medical supports ("US English (en-US) is the only
/// language supported with Amazon Transcribe Medical"), so one code serves every row.
const LANGUAGE_CODE: &str = "en-US";

/// One family that takes a file as the source of its terms, with the member names its
/// operations spell that file and its own name with.
struct Family {
    /// The operation that builds the resource.
    create: &'static str,
    /// The operation that rebuilds it from another file.
    update: &'static str,
    /// The family's own reader.
    get: &'static str,
    /// The member naming the resource in all three.
    name_member: &'static str,
    /// The member naming the file its terms are read from.
    file_member: &'static str,
    /// Whether the update takes a `LanguageCode`: `UpdateVocabulary` and
    /// `UpdateMedicalVocabulary` model it `@required`, `UpdateVocabularyFilter` has no
    /// such member at all (`smithy_query(query="operation",
    /// operation="UpdateVocabulary,UpdateMedicalVocabulary,UpdateVocabularyFilter")`).
    update_takes_language_code: bool,
    /// Whether the family models a build state, so a reader reports a
    /// `VocabularyState` a caller has to wait out before updating.
    has_build_state: bool,
}

/// The custom-vocabulary family.
const VOCABULARY: Family = Family {
    create: "CreateVocabulary",
    update: "UpdateVocabulary",
    get: "GetVocabulary",
    name_member: "VocabularyName",
    file_member: "VocabularyFileUri",
    update_takes_language_code: true,
    has_build_state: true,
};

/// The custom MEDICAL vocabulary family, whose terms can come from nowhere but the
/// file.
const MEDICAL_VOCABULARY: Family = Family {
    create: "CreateMedicalVocabulary",
    update: "UpdateMedicalVocabulary",
    get: "GetMedicalVocabulary",
    name_member: "VocabularyName",
    file_member: "VocabularyFileUri",
    update_takes_language_code: true,
    has_build_state: true,
};

/// The custom-vocabulary-FILTER family, which models no build state.
const VOCABULARY_FILTER: Family = Family {
    create: "CreateVocabularyFilter",
    update: "UpdateVocabularyFilter",
    get: "GetVocabularyFilter",
    name_member: "VocabularyFilterName",
    file_member: "VocabularyFilterFileUri",
    update_takes_language_code: false,
    has_build_state: false,
};

/// Every family the rule covers.
const FAMILIES: &[&Family] = &[&VOCABULARY, &MEDICAL_VOCABULARY, &VOCABULARY_FILTER];

/// `PUT /<bucket>` then `PUT /<bucket>/<key>` for every staged file, so the object
/// store really holds what the accepted rows name.
async fn given_staged_files(router: &MultiServiceRouter) {
    for key in STAGED_KEYS {
        given_object(router, FILE_BUCKET, key, FILE_BYTES).await;
    }
}

/// An `s3://` URI addressing the staged object at `key`.
fn staged_uri(key: &str) -> String {
    format!("s3://{FILE_BUCKET}/{key}")
}

/// Each way an address names NO object of the store, and the URI that spells it.
///
/// The first five need the store's own answer about [`FILE_BUCKET`] or
/// [`ABSENT_BUCKET`]; the last two are of the Amazon S3 scheme and address no object at
/// all, which needs no answer — every example of such a member names a bucket AND a key
/// (`s3://DOC-EXAMPLE-BUCKET/my-vocab-file.txt`).
fn absent_addresses() -> Vec<(&'static str, String)> {
    vec![
        (
            "a key the bucket does not hold",
            staged_uri("never-uploaded.txt"),
        ),
        (
            "a bucket the store does not hold",
            format!("s3://{ABSENT_BUCKET}/terms.txt"),
        ),
        (
            "a key that only prefixes a stored one",
            staged_uri("terms"),
        ),
        (
            "a key that extends a stored one",
            staged_uri("terms.txt/more"),
        ),
        (
            "a key under a prefix nothing was staged under",
            staged_uri("tables/surgery/terms.txt"),
        ),
        (
            "a URI naming only a bucket",
            format!("s3://{FILE_BUCKET}"),
        ),
        (
            "a URI whose key part is empty",
            format!("s3://{FILE_BUCKET}/"),
        ),
    ]
}

/// The create request of a family, naming `uri` as the file its terms come from.
fn create_request(family: &Family, name: &str, uri: &str) -> Value {
    json!({
        family.name_member: name,
        "LanguageCode": LANGUAGE_CODE,
        family.file_member: uri,
    })
}

/// The update request of a family, naming `uri` as the file the rebuild reads.
fn update_request(family: &Family, name: &str, uri: &str) -> Value {
    let mut request = json!({
        family.name_member: name,
        family.file_member: uri,
    });
    if family.update_takes_language_code {
        request["LanguageCode"] = json!(LANGUAGE_CODE);
    }
    request
}

/// Poll the family's reader until the build of `name` has FINISHED, and answer the
/// `VocabularyState` it settled in. A family that models no build state has no build to
/// wait for, so it answers `READY` at once.
async fn settled_state(router: &MultiServiceRouter, family: &Family, name: &str) -> String {
    if !family.has_build_state {
        return "READY".to_string();
    }
    let case = format!("{} {name:?}", family.get);
    for _ in 0..POLLS_FOR_SETTLED {
        let reply = call(router, family.get, json!({family.name_member: name})).await;
        let state = reply.member(&case, "VocabularyState");
        if state != "PENDING" {
            return state;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
    panic!("case {case:?}: the build never finished");
}

/// Assert `reply` is the refusal this rule states, in full: the modelled client error,
/// its HTTP status and its fixed text.
fn assert_refused(reply: &Reply, case: &str) {
    reply.refused(case, 400, "BadRequestException");
    let message = reply.error_message(case);
    assert_eq!(
        message, URI_NAMES_NO_OBJECT,
        "case {case:?}: expected the refusal to say {URI_NAMES_NO_OBJECT:?}, got \
         {message:?}"
    );
}

/// A CREATE that names an object the store does not hold is refused, and the name it
/// asked for was never taken: the very same name is created afterwards from the staged
/// file, which a stored record would have answered `ConflictException` for.
#[tokio::test]
async fn a_create_whose_file_names_no_object_is_refused_and_creates_nothing() {
    let router = emulator();
    given_staged_files(&router).await;

    for family in FAMILIES {
        for (index, (how, uri)) in absent_addresses().iter().enumerate() {
            let case = format!("{} with {how}", family.create);
            let name = format!("absent-file-{}-{index}", family.create);

            let refused = call(&router, family.create, create_request(family, &name, uri)).await;
            assert_refused(&refused, &case);

            // Nothing was recorded under that name: the family's own reader does not
            // find it.
            let read = call(&router, family.get, json!({family.name_member: &name})).await;
            read.refused(
                &format!("{case} (the reader must not find it)"),
                400,
                "BadRequestException",
            );

            // And the name is still free, which only holds if no record was stored.
            let created = call(
                &router,
                family.create,
                create_request(family, &name, &staged_uri(STAGED_KEYS[0])),
            )
            .await;
            created.ok(&format!("{case} (the name must still be free)"));
            let state = settled_state(&router, family, &name).await;
            assert_eq!(
                state, "READY",
                "case {case:?}: the create that names the staged file must build, got \
                 {state:?}"
            );
        }
    }
}

/// An UPDATE that names an object the store does not hold is refused and the resource
/// keeps what it had: the same state, the same terms and the same `LastModifiedTime`.
#[tokio::test]
async fn an_update_whose_file_names_no_object_is_refused_and_changes_nothing() {
    let router = emulator();
    given_staged_files(&router).await;

    for family in FAMILIES {
        let name = format!("rebuilt-{}", family.update);
        call(
            &router,
            family.create,
            create_request(family, &name, &staged_uri(STAGED_KEYS[0])),
        )
        .await
        .ok(&format!("{} to rebuild", family.create));
        // "Your custom vocabulary must be in a terminal state (READY or FAILED) before
        // you can update it" (`smithy_query(query="doc", shape="UpdateVocabulary")`),
        // so the build is waited out before any rebuild is attempted.
        let built = settled_state(&router, family, &name).await;
        assert_eq!(
            built, "READY",
            "case {:?}: the resource to rebuild must be built first, got {built:?}",
            family.create
        );
        let before = call(&router, family.get, json!({family.name_member: &name})).await;
        let last_modified = before.timestamp(family.get, "LastModifiedTime");

        for (how, uri) in absent_addresses() {
            let case = format!("{} with {how}", family.update);
            let refused = call(&router, family.update, update_request(family, &name, &uri)).await;
            assert_refused(&refused, &case);

            // The record the refused rebuild named is untouched.
            let after = call(&router, family.get, json!({family.name_member: &name})).await;
            after.ok(&case);
            if family.has_build_state {
                assert_eq!(
                    after.member(&case, "VocabularyState"),
                    "READY",
                    "case {case:?}: a refused rebuild leaves the resource built, got \
                     {:?}",
                    after.body
                );
                assert!(
                    after.body.get("FailureReason").is_none_or(Value::is_null),
                    "case {case:?}: a refused rebuild fails nothing, got {:?}",
                    after.body
                );
            }
            assert_eq!(
                after.timestamp(&case, "LastModifiedTime"),
                last_modified,
                "case {case:?}: a refused rebuild changes nothing, so the record's \
                 LastModifiedTime must stand, got {:?}",
                after.body
            );
        }

        // A rebuild from a file the store DOES serve still goes through afterwards, so
        // the refusals above left the resource usable rather than wedged.
        call(
            &router,
            family.update,
            update_request(family, &name, &staged_uri(STAGED_KEYS[1])),
        )
        .await
        .ok(&format!("{} from the nested staged file", family.update));
        let state = settled_state(&router, family, &name).await;
        assert_eq!(
            state, "READY",
            "case {:?}: a rebuild from a served file must build, got {state:?}",
            family.update
        );
    }
}

/// The counter-cases that keep the refusal GROUNDED: an object the store serves is
/// accepted and built from its terms, and a location no object store can be asked about
/// is accepted too.
#[tokio::test]
async fn a_served_file_and_an_unaskable_location_are_both_accepted() {
    let router = emulator();
    given_staged_files(&router).await;

    // Each row is one location and whether a build that reads it loads the staged
    // terms. The `http(s)` schemes are the ones the member's own `@pattern` admits
    // beside `s3://` — `^(s3://|http(s*)://).+$` (`smithy_query(query="operation",
    // operation="CreateVocabulary")`) — and address a HOST, which this emulator
    // resolves none of.
    let rows: Vec<(&str, String, bool)> = vec![
        ("a staged object at the bucket root", staged_uri(STAGED_KEYS[0]), true),
        ("a staged object under a nested prefix", staged_uri(STAGED_KEYS[1]), true),
        (
            "an https location naming a host",
            "https://example-bucket.s3.amazonaws.com/terms.txt".to_string(),
            false,
        ),
        (
            "an http location naming a host",
            "http://example-bucket.s3.amazonaws.com/terms.txt".to_string(),
            false,
        ),
    ];

    for family in FAMILIES {
        for (index, (how, uri, loads_terms)) in rows.iter().enumerate() {
            let case = format!("{} with {how}", family.create);
            let name = format!("accepted-{}-{index}", family.create);
            call(&router, family.create, create_request(family, &name, uri))
                .await
                .ok(&case);

            let state = settled_state(&router, family, &name).await;
            // A family that models no build state has nothing to settle: it is usable
            // from the create whatever its file read.
            let expected = if *loads_terms || !family.has_build_state {
                "READY"
            } else {
                "FAILED"
            };
            assert_eq!(
                state, expected,
                "case {case:?}: a build that {} must settle {expected}, got {state:?}",
                if *loads_terms {
                    "read the staged file"
                } else {
                    "had no object store to ask"
                }
            );

            // What a served file actually gave the resource: its terms, at the location
            // the reader reports for them.
            if *loads_terms && family.has_build_state {
                let read = call(&router, family.get, json!({family.name_member: &name})).await;
                read.ok(&case);
                assert!(
                    read.body.get("DownloadUri").is_some(),
                    "case {case:?}: a build that read the staged file has a file to \
                     download, got {:?}",
                    read.body
                );
            }
        }
    }

    // The terms a served file gave one resource are the file's own, in its order —
    // asserted once, through the location the reader reports.
    let name = format!("accepted-{}-0", VOCABULARY.create);
    let read = call(
        &router,
        VOCABULARY.get,
        json!({VOCABULARY.name_member: &name}),
    )
    .await;
    let uri = read
        .ok("the served terms")
        .get("DownloadUri")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a built vocabulary reports a DownloadUri, got {:?}", read.body))
        .to_string();
    let path = uri
        .split_once("://")
        .and_then(|(_, rest)| rest.find('/').map(|at| rest[at..].to_string()))
        .unwrap_or_else(|| panic!("a reported DownloadUri names a path, got {uri:?}"));
    let (status, body) = crate::media_object_resolution::s3(&router, "GET", &path, &[]).await;
    assert_eq!(
        status, 200,
        "the reported location must serve the built file, got HTTP {status}"
    );
    let served: Vec<String> = std::str::from_utf8(&body)
        .expect("the served file is UTF-8")
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(
        served,
        staged_terms(),
        "the location must serve the terms the staged file held, got {served:?}"
    );
}
