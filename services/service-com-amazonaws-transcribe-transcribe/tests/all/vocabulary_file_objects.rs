// SPDX-License-Identifier: CC0-1.0

//! The FILE a create or an update names as the source of a custom vocabulary's terms,
//! resolved against the emulator's own Amazon S3 — that service mounted beside
//! Transcribe on one engine, exactly the way the assembled emulator runs them, and the
//! way the real service takes a vocabulary's table on when it builds one.
//!
//! THE RULE UNDER TEST: where an operation's content comes from a file at a
//! caller-named URI, the service READS that file from the object store the caller
//! uploaded it to ("you must first upload a text file that contains your vocabulary
//! table into an Amazon S3 bucket"), so the build settles on what the store answered:
//! a file the store serves gives the resource its terms and settles it in the state
//! that makes it usable ("If the state is READY, you can use the custom vocabulary in a
//! StartTranscriptionJob request"), with those terms served at the `DownloadUri` its
//! reader reports ("use this URI to view or download the custom vocabulary"), while a
//! file nothing read loads no terms and ends the build in the service's own modelled
//! `FAILED` state with a `FailureReason` naming the member and the URI. A request whose
//! object the store reports it does not HOLD never reaches a build at all — it is
//! refused outright and nothing is recorded, which is
//! `crate::vocabulary_file_names_an_object`'s subject — so the rows that load no terms
//! here are the ones no object store could be asked about or whose served object holds
//! nothing to build from.
//!
//! The rows are a TABLE spanning what the rule generalises over: which family takes the
//! file (`CreateVocabulary`, `CreateMedicalVocabulary` — whose terms can come from
//! nowhere else — and `CreateVocabularyFilter`, which models no build state at all),
//! which member names it (`VocabularyFileUri` / `VocabularyFilterFileUri`), how the
//! file is addressed (a key at the bucket root, a key under a nested prefix), what it
//! holds (one term per line, a tab-separated vocabulary table, CRLF line endings and
//! blank lines, a single term with no trailing newline, two hundred terms), and in
//! WHICH way a build loads no terms (an `http(s)` URI naming a host, an object holding
//! no terms and one that is not text).
//!
//! The suites after it pin the rest of the rule: the listings select on the state each
//! build reached, a rebuild reads the file the UPDATE named, a file past the published
//! size ceiling is refused rather than built from, and a vocabulary the store served a
//! file for can actually be APPLIED to a job of its family.
//!
//! The harness is the media-resolution suite's (`crate::media_object_resolution`): its
//! emulator with both services on one engine, its Transcribe call and its raw
//! path-style object-store request, reused rather than stood up a second time. One
//! Region serves every call; which Region a record belongs to is another suite's
//! subject (`crate::regional_resource_scoping`).

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::media_object_resolution::{call, emulator, given_staged_media, s3, BUCKET, STAGED_KEYS};
use crate::transcription_jobs::Reply;

/// How long a poll of a family's reader waits between two calls, and how many calls it
/// makes before giving up: together comfortably past one
/// `crate::state::VOCABULARY_TRANSITION_DELAY` (500 ms) even on a slow machine.
const POLL_INTERVAL: Duration = Duration::from_millis(50);
const POLLS_FOR_SETTLED: usize = 40;

/// The state a build that loaded its terms settles in, and the only one that reports a
/// `DownloadUri`.
const READY: &str = "READY";
/// The state a build that loaded none settles in.
const FAILED: &str = "FAILED";

/// The bucket every vocabulary file below is staged in — the caller's own bucket, which
/// the caller creates and uploads to before calling, as the real service requires.
const FILE_BUCKET: &str = "example-vocabulary-tables";

/// The files staged in [`FILE_BUCKET`]: the key, the bytes the object holds, and the
/// terms a build must read out of it.
///
/// The forms span what such a file may be: one term per line; a vocabulary TABLE, whose
/// first line is the `Phrase  IPA  SoundsLike  DisplayAs` column header
/// `CreateMedicalVocabulary` documents and whose other lines are its rows; Windows line
/// endings with a blank line between entries; a single term with no trailing newline;
/// and a long list.
fn staged_files() -> Vec<(&'static str, Vec<u8>, Vec<String>)> {
    let table = "Phrase\tIPA\tSoundsLike\tDisplayAs\n\
                 Tympanic-membrane\t\t\tTympanic membrane\n\
                 Myocardial-infarction\t\tmy-oh-car-dee-al in-farc-shun\tMyocardial infarction\n";
    vec![
        (
            "terms.txt",
            b"biopsy\ncarcinoma\nelectrocardiogram\n".to_vec(),
            vec![
                "biopsy".to_string(),
                "carcinoma".to_string(),
                "electrocardiogram".to_string(),
            ],
        ),
        (
            "nested/tables/medical-table.txt",
            table.as_bytes().to_vec(),
            table.lines().map(str::to_string).collect(),
        ),
        (
            "windows-line-endings.txt",
            b"alpha\r\n\r\nbeta\r\n".to_vec(),
            vec!["alpha".to_string(), "beta".to_string()],
        ),
        (
            "single-term.txt",
            b"sphygmomanometer".to_vec(),
            vec!["sphygmomanometer".to_string()],
        ),
        (
            "many-terms.txt",
            (0..200)
                .map(|index| format!("term-{index}\n"))
                .collect::<String>()
                .into_bytes(),
            (0..200).map(|index| format!("term-{index}")).collect(),
        ),
        // An object the store serves that holds no terms, and one that is not text: the
        // store answered, and there is still nothing to build from.
        ("empty.txt", Vec::new(), Vec::new()),
        ("not-text.bin", vec![0xff, 0xfe, 0x00, 0x01], Vec::new()),
    ]
}

/// `PUT /<bucket>` — `CreateBucket` — then `PUT /<bucket>/<key>` — `PutObject` — for
/// every staged file, so the object store really holds the files the rows name.
async fn given_staged_files(router: &MultiServiceRouter) {
    let (status, body) = s3(router, "PUT", &format!("/{FILE_BUCKET}"), &[]).await;
    assert!(
        status == 200 || status == 409,
        "the vocabulary-file bucket must exist, got HTTP {status} with body {:?}",
        String::from_utf8_lossy(&body)
    );
    for (key, bytes, _) in staged_files() {
        let (status, body) = s3(router, "PUT", &format!("/{FILE_BUCKET}/{key}"), &bytes).await;
        assert_eq!(
            status, 200,
            "the vocabulary file {FILE_BUCKET}/{key} must be stored, got HTTP {status} \
             with body {:?}",
            String::from_utf8_lossy(&body)
        );
    }
}

/// One family that takes its content from a file, with the member names its operations
/// spell that content and that resource with.
struct Family {
    create: &'static str,
    update: &'static str,
    get: &'static str,
    list: &'static str,
    name_member: &'static str,
    /// The member naming the file the content is read from.
    file_member: &'static str,
    /// Whether the family models a `VocabularyState` its build settles in. A
    /// vocabulary and a medical vocabulary do; a vocabulary FILTER models none, so the
    /// only thing it reports about its build is whether there is a file to download.
    has_build_state: bool,
    /// Whether the update carries a `LanguageCode`: `UpdateVocabulary` and
    /// `UpdateMedicalVocabulary` model it `@required`, `UpdateVocabularyFilter` has no
    /// such member
    /// (`smithy_query(query="operation", operation="UpdateVocabulary,UpdateMedicalVocabulary,UpdateVocabularyFilter")`).
    update_takes_language_code: bool,
}

/// The custom-vocabulary family, whose terms may come from a file instead of `Phrases`.
const VOCABULARY: Family = Family {
    create: "CreateVocabulary",
    update: "UpdateVocabulary",
    get: "GetVocabulary",
    list: "ListVocabularies",
    name_member: "VocabularyName",
    file_member: "VocabularyFileUri",
    has_build_state: true,
    update_takes_language_code: true,
};

/// The custom MEDICAL vocabulary family, whose terms can come from nowhere else.
const MEDICAL_VOCABULARY: Family = Family {
    create: "CreateMedicalVocabulary",
    update: "UpdateMedicalVocabulary",
    get: "GetMedicalVocabulary",
    list: "ListMedicalVocabularies",
    name_member: "VocabularyName",
    file_member: "VocabularyFileUri",
    has_build_state: true,
    update_takes_language_code: true,
};

/// The custom-vocabulary-FILTER family, whose words may come from a file instead of
/// `Words`.
const VOCABULARY_FILTER: Family = Family {
    create: "CreateVocabularyFilter",
    update: "UpdateVocabularyFilter",
    get: "GetVocabularyFilter",
    list: "ListVocabularyFilters",
    name_member: "VocabularyFilterName",
    file_member: "VocabularyFilterFileUri",
    has_build_state: false,
    update_takes_language_code: false,
};

/// One row: the family, the resource name, the file URI its create names and the terms
/// the build must load from it — EMPTY for a file the object store does not serve.
struct Row {
    /// What this row varies, named in every assertion it fails.
    case: &'static str,
    family: &'static Family,
    name: &'static str,
    uri: String,
    terms: Vec<String>,
}

impl Row {
    /// Whether the object store serves this row's file, which is what the build settles
    /// on.
    fn served(&self) -> bool {
        !self.terms.is_empty()
    }
}

/// The terms the staged file at `key` holds.
fn staged_terms(key: &str) -> Vec<String> {
    staged_files()
        .into_iter()
        .find(|(staged, _, _)| *staged == key)
        .map(|(_, _, terms)| terms)
        .unwrap_or_else(|| panic!("{key} is one of the staged files"))
}

/// An `s3://` URI addressing the staged file at `key`.
fn staged_uri(key: &str) -> String {
    format!("s3://{FILE_BUCKET}/{key}")
}

/// The rows. Every family is driven over a file the store serves and over each way a
/// build loads no terms, so neither outcome is asserted for one family alone.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a vocabulary from a file of one term per line",
            family: &VOCABULARY,
            name: "clinic-terms",
            uri: staged_uri("terms.txt"),
            terms: staged_terms("terms.txt"),
        },
        Row {
            case: "a vocabulary from a file under a nested prefix",
            family: &VOCABULARY,
            name: "nested.table",
            uri: staged_uri("nested/tables/medical-table.txt"),
            terms: staged_terms("nested/tables/medical-table.txt"),
        },
        Row {
            case: "a vocabulary from a file with CRLF line endings and a blank line",
            family: &VOCABULARY,
            name: "crlf_terms",
            uri: staged_uri("windows-line-endings.txt"),
            terms: staged_terms("windows-line-endings.txt"),
        },
        Row {
            case: "a vocabulary from a file of two hundred terms",
            family: &VOCABULARY,
            name: "many-terms",
            uri: staged_uri("many-terms.txt"),
            terms: staged_terms("many-terms.txt"),
        },
        Row {
            case: "a vocabulary whose URI names a host rather than an object",
            family: &VOCABULARY,
            name: "hosted-file",
            uri: "https://example.invalid/vocabulary-file.txt".to_string(),
            terms: Vec::new(),
        },
        Row {
            case: "a vocabulary whose file holds no terms",
            family: &VOCABULARY,
            name: "empty-file",
            uri: staged_uri("empty.txt"),
            terms: Vec::new(),
        },
        Row {
            case: "a medical vocabulary from a tab-separated vocabulary table",
            family: &MEDICAL_VOCABULARY,
            name: "med-vocab",
            uri: staged_uri("nested/tables/medical-table.txt"),
            terms: staged_terms("nested/tables/medical-table.txt"),
        },
        Row {
            case: "a medical vocabulary from a single-term file with no trailing newline",
            family: &MEDICAL_VOCABULARY,
            name: "med-vocab.single",
            uri: staged_uri("single-term.txt"),
            terms: staged_terms("single-term.txt"),
        },
        Row {
            case: "a medical vocabulary whose URI names a host rather than an object",
            family: &MEDICAL_VOCABULARY,
            name: "med-vocab-hosted",
            uri: "https://example.invalid/vocabulary-table.txt".to_string(),
            terms: Vec::new(),
        },
        Row {
            case: "a medical vocabulary whose file is not text",
            family: &MEDICAL_VOCABULARY,
            name: "med-vocab-binary",
            uri: staged_uri("not-text.bin"),
            terms: Vec::new(),
        },
        Row {
            case: "a vocabulary filter from a file of one word per line",
            family: &VOCABULARY_FILTER,
            name: "blocked-words",
            uri: staged_uri("terms.txt"),
            terms: staged_terms("terms.txt"),
        },
        Row {
            case: "a vocabulary filter whose file holds no words",
            family: &VOCABULARY_FILTER,
            name: "blocked-empty",
            uri: staged_uri("empty.txt"),
            terms: Vec::new(),
        },
    ]
}

/// The create request of a row: the smallest valid one of its family naming the file.
///
/// `en-US` is the only language Amazon Transcribe Medical supports ("US English (en-US)
/// is the only language supported with Amazon Transcribe Medical"), and it is a value of
/// the service-wide `LanguageCode` enum every family accepts, so one code serves every
/// row; which codes a family accepts is another suite's subject
/// (`crate::medical_vocabulary_constraints`).
fn create_request(row: &Row) -> Value {
    json!({
        row.family.name_member: row.name,
        "LanguageCode": "en-US",
        row.family.file_member: row.uri,
    })
}

/// The update request of a row, naming `uri` as the file the rebuild reads.
fn update_request(row: &Row, uri: &str) -> Value {
    let mut request = json!({
        row.family.name_member: row.name,
        row.family.file_member: uri,
    });
    if row.family.update_takes_language_code {
        request["LanguageCode"] = json!("en-US");
    }
    request
}

/// Whether a member is absent from an answer — which is how an optional member the
/// service has nothing to report for travels.
fn is_absent(reply: &Reply, member: &str) -> bool {
    reply.body.get(member).is_none_or(Value::is_null)
}

/// Poll the family's reader until the build of `name` has FINISHED, and answer the
/// `VocabularyState` it settled in. A family that models no build state answers
/// [`READY`] at once: it has no build to wait for.
async fn settled_state(router: &MultiServiceRouter, family: &Family, name: &str) -> String {
    if !family.has_build_state {
        return READY.to_string();
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

/// The path a client resolves a reported `DownloadUri` to: everything from the host's
/// first slash, the bucket and key of a path-style location.
fn object_path(case: &str, uri: &str) -> String {
    uri.split_once("://")
        .and_then(|(_, rest)| rest.find('/').map(|at| rest[at..].to_string()))
        .unwrap_or_else(|| panic!("case {case:?}: a reported DownloadUri names a path, got {uri:?}"))
}

/// The terms a fetched location served, in the order the file lists them.
fn served_terms(case: &str, body: &[u8]) -> Vec<String> {
    let text = std::str::from_utf8(body)
        .unwrap_or_else(|error| panic!("case {case:?}: the served file is not UTF-8: {error}"));
    text.lines().map(str::to_string).collect()
}

/// The `DownloadUri` the family's reader reports for `name`, if it reports one.
async fn reported_download_uri(
    router: &MultiServiceRouter,
    family: &Family,
    name: &str,
) -> Option<String> {
    let reply = call(router, family.get, json!({family.name_member: name})).await;
    reply.ok(family.get);
    reply
        .body
        .get("DownloadUri")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Assert the location the family's reader reports for a row serves exactly
/// `expected`.
async fn assert_location_serves(
    router: &MultiServiceRouter,
    row: &Row,
    expected: &[String],
    when: &str,
) {
    let case = format!("{} ({when})", row.case);
    let uri = reported_download_uri(router, row.family, row.name)
        .await
        .unwrap_or_else(|| {
            panic!("case {case:?}: a built resource must report a DownloadUri, got none")
        });
    let path = object_path(&case, &uri);
    let (status, body) = s3(router, "GET", &path, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the reported DownloadUri {uri:?} must serve the terms the build \
         loaded, got HTTP {status} with body {:?}",
        String::from_utf8_lossy(&body)
    );
    let served = served_terms(&case, &body);
    assert_eq!(
        served, expected,
        "case {case:?}: the location must serve the terms the file held, expected \
         {expected:?}, got {served:?}"
    );
}

/// Every row: the build settles on what the object store answered about its file.
#[tokio::test]
async fn a_file_backed_build_settles_on_what_the_object_store_served() {
    let router = emulator();
    given_staged_files(&router).await;

    for row in &rows() {
        let created = call(&router, row.family.create, create_request(row)).await;
        created.ok(row.case);
        if row.family.has_build_state {
            // The build is work the service does AFTER answering, so a caller reading
            // the record straight back still sees it running.
            assert_eq!(
                created.member(row.case, "VocabularyState"),
                "PENDING",
                "case {:?}: the create answers the record it stored, still being \
                 processed, got {:?}",
                row.case,
                created.body
            );
            assert!(
                is_absent(&created, "FailureReason"),
                "case {:?}: a build still running has not failed, got {:?}",
                row.case,
                created.body
            );
        }

        let state = settled_state(&router, row.family, row.name).await;
        let read = call(
            &router,
            row.family.get,
            json!({row.family.name_member: row.name}),
        )
        .await;
        read.ok(row.case);

        if row.served() {
            assert_eq!(
                state, READY,
                "case {:?}: a file the object store served gives the build its terms, \
                 expected {READY}, got {state:?} in {:?}",
                row.case, read.body
            );
            assert!(
                is_absent(&read, "FailureReason"),
                "case {:?}: a build that loaded its terms carries no FailureReason, got \
                 {:?}",
                row.case,
                read.body
            );
            assert_location_serves(&router, row, &row.terms, "after the create").await;
        } else {
            if row.family.has_build_state {
                assert_eq!(
                    state, FAILED,
                    "case {:?}: a file nothing read loads no terms, \
                     expected {FAILED}, got {state:?} in {:?}",
                    row.case, read.body
                );
                let reason = read.member(row.case, "FailureReason");
                assert!(
                    reason.contains(row.family.file_member) && reason.contains(&row.uri),
                    "case {:?}: the FailureReason names the member and the file that \
                     was not read ({} {}), got {reason:?}",
                    row.case,
                    row.family.file_member,
                    row.uri
                );
            }
            let reported = reported_download_uri(&router, row.family, row.name).await;
            assert!(
                reported.is_none(),
                "case {:?}: a resource whose file nothing read has \
                 nothing built to download (state {state:?}), got {reported:?}",
                row.case
            );
        }
    }
}

/// The family's listing selects on the state each build actually reached, so a caller
/// can find the vocabularies it may use.
#[tokio::test]
async fn a_listing_selects_on_the_state_each_build_reached() {
    let router = emulator();
    given_staged_files(&router).await;

    for family in [&VOCABULARY, &MEDICAL_VOCABULARY] {
        let served = Row {
            case: "a build the object store served a file for",
            family,
            name: "listed-ready",
            uri: staged_uri("terms.txt"),
            terms: staged_terms("terms.txt"),
        };
        let unserved = Row {
            case: "a build whose served file holds no terms",
            family,
            name: "listed-failed",
            uri: staged_uri("empty.txt"),
            terms: Vec::new(),
        };
        for row in [&served, &unserved] {
            call(&router, family.create, create_request(row))
                .await
                .ok(row.case);
            settled_state(&router, family, row.name).await;
        }

        for (filter, expected, unexpected) in [
            (READY, served.name, unserved.name),
            (FAILED, unserved.name, served.name),
        ] {
            let case = format!("{} StateEquals {filter}", family.list);
            let listed = call(
                &router,
                family.list,
                json!({"StateEquals": filter, "MaxResults": 100}),
            )
            .await;
            let names: Vec<String> = listed.ok(&case)["Vocabularies"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: a listing reports rows, got {:?}", listed.body))
                .iter()
                .filter_map(|row| row["VocabularyName"].as_str())
                .map(str::to_string)
                .collect();
            assert!(
                names.contains(&expected.to_string()),
                "case {case:?}: the listing must report {expected}, got {names:?}"
            );
            assert!(
                !names.contains(&unexpected.to_string()),
                "case {case:?}: the listing must not report {unexpected}, got {names:?}"
            );
        }
    }
}

/// A rebuild reads the file the UPDATE named: "This operation overwrites all existing
/// information with your new values; you cannot append new terms onto an existing
/// custom vocabulary." So the location serves the new file's terms, a rebuild from a
/// file that loads no terms leaves nothing there, and one from a file the store serves
/// builds again.
#[tokio::test]
async fn a_rebuild_reads_the_file_the_update_named() {
    let router = emulator();
    given_staged_files(&router).await;

    for family in [&VOCABULARY, &MEDICAL_VOCABULARY] {
        let row = Row {
            case: "a file-backed vocabulary rebuilt from another file",
            family,
            name: "rebuilt",
            uri: staged_uri("terms.txt"),
            terms: staged_terms("terms.txt"),
        };
        call(&router, family.create, create_request(&row))
            .await
            .ok(row.case);
        let state = settled_state(&router, family, row.name).await;
        assert_eq!(
            state, READY,
            "case {:?}: the create's build settles {READY} before the update, got \
             {state:?}",
            row.case
        );
        assert_location_serves(&router, &row, &row.terms, "after the create").await;
        let built_at = reported_download_uri(&router, family, row.name)
            .await
            .expect("a built resource reports a DownloadUri");

        // Rebuilt from another file the store serves: the location serves THAT file's
        // terms and nothing of the first one's.
        let other = staged_terms("windows-line-endings.txt");
        call(
            &router,
            family.update,
            update_request(&row, &staged_uri("windows-line-endings.txt")),
        )
        .await
        .ok(row.case);
        let state = settled_state(&router, family, row.name).await;
        assert_eq!(
            state, READY,
            "case {:?}: a rebuild from a file the store served settles {READY}, got \
             {state:?}",
            row.case
        );
        assert_location_serves(&router, &row, &other, "after the rebuild").await;

        // Rebuilt from a file that loads NO terms: nothing was loaded, so nothing
        // is reported and the location the previous build left serves nothing.
        call(
            &router,
            family.update,
            update_request(&row, &staged_uri("empty.txt")),
        )
        .await
        .ok(row.case);
        let state = settled_state(&router, family, row.name).await;
        assert_eq!(
            state, FAILED,
            "case {:?}: a rebuild from a file that loads no terms settles {FAILED}, \
             got {state:?}",
            row.case
        );
        let reported = reported_download_uri(&router, family, row.name).await;
        assert!(
            reported.is_none(),
            "case {:?}: a rebuild that loaded nothing reports no DownloadUri, got \
             {reported:?}",
            row.case
        );
        let path = object_path(row.case, &built_at);
        let (status, body) = s3(&router, "GET", &path, &[]).await;
        assert_eq!(
            status, 404,
            "case {:?}: the location the previous build left must serve nothing, got \
             HTTP {status} with body {:?}",
            row.case,
            String::from_utf8_lossy(&body)
        );

        // And a rebuild from a file the store does serve builds again.
        call(&router, family.update, update_request(&row, &row.uri))
            .await
            .ok(row.case);
        let state = settled_state(&router, family, row.name).await;
        assert_eq!(
            state, READY,
            "case {:?}: a failed build is rebuilt by an update naming a file the store \
             serves, got {state:?}",
            row.case
        );
        assert_location_serves(&router, &row, &row.terms, "after the second rebuild").await;
    }
}

/// A file holding more than the published ceiling a custom vocabulary may is REFUSED
/// rather than built from — the modelled `LimitExceededException`, whose own
/// documentation covers it ("You've either sent too many requests or your input file is
/// too long ... use a smaller file and try your request again") — and nothing is
/// recorded for it.
#[tokio::test]
async fn a_file_past_the_published_ceiling_is_refused() {
    let ceiling = service_com_amazonaws_transcribe_transcribe::state::MAX_VOCABULARY_CONTENT_BYTES;
    let router = emulator();
    given_staged_files(&router).await;

    // One byte past the ceiling, staged as an object of the caller's own bucket.
    let key = "oversized.txt";
    let oversized: Vec<u8> = std::iter::repeat_n(b'a', ceiling + 1).collect();
    let (status, body) = s3(&router, "PUT", &format!("/{FILE_BUCKET}/{key}"), &oversized).await;
    assert_eq!(
        status, 200,
        "the oversized vocabulary file must be stored, got HTTP {status} with body {:?}",
        String::from_utf8_lossy(&body)
    );

    for family in [&VOCABULARY, &MEDICAL_VOCABULARY, &VOCABULARY_FILTER] {
        let row = Row {
            case: "a file one byte past the published ceiling",
            family,
            name: "oversized",
            uri: staged_uri(key),
            terms: Vec::new(),
        };
        let case = format!("{} ({})", row.case, family.create);
        let refused = call(&router, family.create, create_request(&row)).await;
        // Every client error of this awsJson1_1 service answers HTTP 400 and names its
        // shape in `__type`; the model's own `@httpError` 429 is inert on the wire (the
        // rule `crate::client_error_wire_status` pins).
        refused.refused(&case, 400, "LimitExceededException");
        let message = refused.error_message(&case);
        assert!(
            message.contains(family.file_member)
                && message.contains(&row.uri)
                && message.contains(&ceiling.to_string()),
            "case {case:?}: the refusal names the member, the file and the ceiling \
             ({ceiling}), got {message:?}"
        );

        // Nothing was recorded, so the name is still free and no reader finds it.
        let read = call(&router, family.get, json!({family.name_member: row.name})).await;
        read.refused(&case, 400, "BadRequestException");
    }
}

/// The point of the rule: a vocabulary the object store served a file for is one a job
/// may actually APPLY, in both families — "If the state is READY, you can use the
/// custom vocabulary in a StartTranscriptionJob request", and the medical family's
/// start says the same of its own.
#[tokio::test]
async fn a_vocabulary_built_from_a_file_can_be_applied_to_a_job() {
    let router = emulator();
    given_staged_files(&router).await;
    given_staged_media(&router).await;
    let media = json!({"MediaFileUri": format!("s3://{BUCKET}/{}", STAGED_KEYS[0])});

    for (family, start, request) in [
        (
            &VOCABULARY,
            "StartTranscriptionJob",
            json!({
                "TranscriptionJobName": "job-with-file-vocabulary",
                "LanguageCode": "en-US",
                "Media": media.clone(),
                "Settings": {"VocabularyName": "applied-vocab"},
            }),
        ),
        (
            &MEDICAL_VOCABULARY,
            "StartMedicalTranscriptionJob",
            json!({
                "MedicalTranscriptionJobName": "medical-job-with-file-vocabulary",
                "LanguageCode": "en-US",
                "Media": media.clone(),
                "OutputBucketName": "example-medical-output",
                "Specialty": "PRIMARYCARE",
                "Type": "DICTATION",
                "Settings": {"VocabularyName": "applied-vocab"},
            }),
        ),
    ] {
        let case = format!("a {start} naming a vocabulary built from a file");
        let row = Row {
            case: "a vocabulary a job applies",
            family,
            name: "applied-vocab",
            uri: staged_uri("terms.txt"),
            terms: staged_terms("terms.txt"),
        };
        call(&router, family.create, create_request(&row))
            .await
            .ok(&case);
        let state = settled_state(&router, family, row.name).await;
        assert_eq!(
            state, READY,
            "case {case:?}: the vocabulary must be usable before the start, got \
             {state:?}"
        );

        let started = call(&router, start, request).await;
        started.ok(&case);
    }
}
