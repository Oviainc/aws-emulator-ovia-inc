// SPDX-License-Identifier: CC0-1.0

//! The constraints a start call's MEMBERS carry, end to end over HTTP.
//!
//! The rule under test is one rule of the service, not one value: a member the model
//! constrains is held to that constraint before anything is minted, so a request the
//! model does not admit is refused with 400 `ValidationException` and leaves no task, no
//! assessment and no record behind — and a value the model DOES admit is accepted, right
//! up to the published bound.
//!
//! The constraints come from
//! `smithy_query(query="operation", operation="StartImportFileTask")` and
//! `smithy_query(query="shape", shape="importS3Bucket")`:
//! `StartImportFileTaskRequest$name` `@length(1,50)` `@pattern [a-zA-Z0-9_-]+`,
//! `$s3key` `@length(0,1024)` `@pattern .*\S.*`, and each bucket member
//! `@length(0,63)` `@pattern [0-9a-z]+[0-9a-z\.\-]*[0-9a-z]+`. The rows span what each
//! rule generalises over: the character classes a pattern admits and the ones it does
//! not, both ends of a length bound and one past each, and both bucket members of both
//! start calls.
//!
//! The refused rows assert the service's own one sentence for a modelled constraint
//! (`tests/all/refusal_messages.rs` owns that rule); here the point is WHICH requests
//! earn it.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::import_and_assess::{call, emulator, Answer};

/// This service's message for a value that fails a rule the model declares, spelled out
/// here rather than imported so a wrong message in the crate fails here too.
const INVALID_REQUEST_BODY: &str = "Invalid request body";

/// A bucket name the service can use: it carries the `migrationhub-strategy-` prefix each
/// bucket member's documentation requires and is inside the modelled pattern.
const USABLE_BUCKET: &str = "migrationhub-strategy-import";

/// A `StartImportFileTask` body carrying the three required members, each legal, with
/// `name` replaced by `name`.
fn import_named(name: &str) -> Value {
    json!({
        "name": name,
        "S3Bucket": USABLE_BUCKET,
        "s3key": "inventory/servers.json",
    })
}

async fn start_import_file_task(router: &MultiServiceRouter, body: Value) -> Answer {
    call(router, "POST", "/start-import-file-task", Some(body)).await
}

async fn start_assessment(router: &MultiServiceRouter, body: Value) -> Answer {
    call(router, "POST", "/start-assessment", Some(body)).await
}

/// Assert `answer` is the service's modelled-constraint refusal.
fn assert_refused(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected HTTP 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.string("/message"),
        INVALID_REQUEST_BODY,
        "case {case:?}: expected the service's {INVALID_REQUEST_BODY:?}, got {:?}",
        answer.body
    );
}

/// RULE: `name` is held to its `@pattern [a-zA-Z0-9_-]+` and its `@length(1,50)`, so a
/// name carrying anything else — a space, punctuation, a non-ASCII letter — or one past
/// the bound is refused with 400 `ValidationException` and mints no task, while a name of
/// letters, digits, underscores and hyphens at any admitted length is accepted.
#[tokio::test]
async fn an_import_name_is_held_to_the_pattern_and_length_the_model_declares() {
    let router = emulator();

    // Rows: a space (the recorded probe's class), each punctuation family, a non-ASCII
    // letter, a blank name, and one character past the published maximum of 50.
    let past_bound = "a".repeat(51);
    let refused: &[(&str, &str)] = &[
        ("a space", "my import 1"),
        ("an exclamation mark", "bad!name"),
        ("a dot", "import.1"),
        ("a slash", "import/1"),
        ("a plus", "import+1"),
        ("a non-ASCII letter", "impörted"),
        ("the empty name", ""),
        ("whitespace alone", "   "),
        ("51 characters, one past the @length max of 50", past_bound.as_str()),
    ];
    for (what, name) in refused {
        let case = format!("{what}: {name:?}");
        let answer = start_import_file_task(&router, import_named(name)).await;
        assert_refused(&case, &answer);
    }

    // Nothing above was minted: the account still holds no import file task.
    let listed = call(&router, "GET", "/list-import-file-task", None).await;
    assert_eq!(
        listed.status, 200,
        "expected the listing to succeed, got {} with {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        listed
            .body
            .pointer("/taskInfos")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "a refused start minted a task: expected none, got {:?}",
        listed.body
    );

    // Rows: the classes the pattern admits, at the shortest and the longest length it
    // admits.
    let at_bound = "n".repeat(50);
    let accepted: &[(&str, &str)] = &[
        ("one character, the @length min of 1", "i"),
        ("letters and digits", "import1"),
        ("mixed case, underscores and hyphens", "IMPORT-4_mixed-case_and-underscores"),
        ("50 characters, the @length max", at_bound.as_str()),
    ];
    for (what, name) in accepted {
        let case = format!("{what}: {name:?}");
        let answer = start_import_file_task(&router, import_named(name)).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected the start to be accepted, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            !answer.string("/id").is_empty(),
            "case {case:?}: an accepted start must answer the task's id, got {:?}",
            answer.body
        );
    }
}

/// RULE: `s3key` names the import file, so it is held to its `@pattern .*\S.*` — a value
/// has to carry something that is not whitespace — within its `@length(0,1024)`; a blank
/// key, or one past the bound, is refused and mints no task.
#[tokio::test]
async fn an_import_key_must_name_an_object() {
    let router = emulator();

    let past_bound = "k".repeat(1025);
    let refused: &[(&str, &str)] = &[
        ("the empty key", ""),
        ("a single space", " "),
        ("whitespace alone", " \t "),
        (
            "1025 characters, one past the @length max of 1024",
            past_bound.as_str(),
        ),
    ];
    for (what, key) in refused {
        let case = format!("{what}: {key:?}");
        let mut body = import_named("key-rule");
        body["s3key"] = json!(key);
        assert_refused(&case, &start_import_file_task(&router, body).await);
    }

    // And a key that carries a non-whitespace character is accepted, at the bound too.
    let at_bound = "k".repeat(1024);
    for (what, key) in [
        ("one character", "k"),
        ("a path with separators", "inventory/2026/servers.json"),
        ("1024 characters, the @length max", at_bound.as_str()),
    ] {
        let case = format!("{what}: {}", key.len());
        let mut body = import_named("key-rule");
        body["s3key"] = json!(key);
        let answer = start_import_file_task(&router, body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected the start to be accepted, got {} with {:?}",
            answer.status, answer.body
        );
    }
}

/// RULE: a bucket member is held to its `@pattern [0-9a-z]+[0-9a-z\.\-]*[0-9a-z]+` — a
/// lower-case name of at least two characters that starts and ends with a letter or a
/// digit — on EVERY member of EVERY start call that names a bucket, so a name the model
/// does not admit is refused before the service's own prefix rule is reached and nothing
/// is minted.
#[tokio::test]
async fn a_bucket_member_is_held_to_the_pattern_the_model_declares() {
    let router = emulator();

    // Rows: an upper-case character (the class S3 bucket names exclude), an underscore, a
    // leading and a trailing separator, a one-character name (below the pattern's own two
    // mandatory characters), and the empty name. Each is spelled with the service's
    // required prefix where the pattern allows it, so the row fails on the PATTERN and not
    // on the prefix rule.
    let refused: &[(&str, &str)] = &[
        ("an upper-case letter", "migrationhub-strategy-UPPER"),
        ("an underscore", "migrationhub-strategy-under_score"),
        ("a leading hyphen", "-migrationhub-strategy-import"),
        ("a trailing hyphen", "migrationhub-strategy-import-"),
        ("a trailing dot", "migrationhub-strategy-import."),
        ("one character", "m"),
        ("the empty name", ""),
    ];
    for (what, bucket) in refused {
        // Both of `StartImportFileTask`'s bucket members, and both of
        // `StartAssessment`'s: one rule, every member it holds for.
        let mut input = import_named("bucket-rule");
        input["S3Bucket"] = json!(bucket);
        let case = format!("StartImportFileTask S3Bucket {what}: {bucket:?}");
        assert_refused(&case, &start_import_file_task(&router, input).await);

        let mut report = import_named("bucket-rule");
        report["s3bucketForReportData"] = json!(bucket);
        let case = format!("StartImportFileTask s3bucketForReportData {what}: {bucket:?}");
        assert_refused(&case, &start_import_file_task(&router, report).await);

        let analysis = json!({"s3bucketForAnalysisData": bucket});
        let case = format!("StartAssessment s3bucketForAnalysisData {what}: {bucket:?}");
        assert_refused(&case, &start_assessment(&router, analysis).await);

        let both = json!({
            "s3bucketForAnalysisData": USABLE_BUCKET,
            "s3bucketForReportData": bucket,
        });
        let case = format!("StartAssessment s3bucketForReportData {what}: {bucket:?}");
        assert_refused(&case, &start_assessment(&router, both).await);
    }

    // A name the pattern admits but the service may not use is a DIFFERENT refusal: the
    // service's permissions sentence for a bucket outside the documented prefix, which is
    // what tells a caller the name is the problem rather than its spelling.
    let outside_prefix = start_import_file_task(
        &router,
        json!({
            "name": "bucket-rule",
            "S3Bucket": "not-a-strategy-bucket",
            "s3key": "servers.json",
        }),
    )
    .await;
    assert_eq!(
        outside_prefix.status, 400,
        "a bucket outside the documented prefix must be refused, got {} with {:?}",
        outside_prefix.status, outside_prefix.body
    );
    assert!(
        outside_prefix
            .string("/message")
            .contains("migrationhub-strategy"),
        "a bucket outside the documented prefix must earn the service's prefix refusal, \
         got {:?}",
        outside_prefix.body
    );

    // And nothing above minted a task.
    let listed = call(&router, "GET", "/list-import-file-task", None).await;
    assert_eq!(
        listed
            .body
            .pointer("/taskInfos")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "a refused start minted a task: expected none, got {:?}",
        listed.body
    );
}
