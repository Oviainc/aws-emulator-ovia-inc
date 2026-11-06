// SPDX-License-Identifier: CC0-1.0

//! `CreateLanguageModel` resolving the data a custom language model is trained and
//! tuned from, end to end over HTTP, with the Amazon S3 service mounted beside
//! Transcribe on one engine — the way the assembled emulator runs them, and the way
//! the service's training run reads the locations a create names.
//!
//! THE RULE THIS SUITE IS ABOUT: resolving those locations is part of VALIDATING the
//! create. A location the object store itself answers cannot be read — a bucket it
//! does not hold, a prefix holding no file, a read it refuses — refuses the create
//! synchronously with `BadRequestException` (HTTP 400) carrying the service's fixed
//! "The S3 URI that you provided can't be accessed. Make sure that you have read
//! permission and try your request again.", and NOTHING is created: the family's
//! reader answers "absent" and no listing reports the name. A create whose locations
//! the store answers for is served with `ModelStatus: IN_PROGRESS` and its one
//! clock-owed transition settles it `COMPLETED` — the status that makes it usable
//! ("When the status displays as COMPLETED, your model is ready to use", the model's
//! doc for `LanguageModel$ModelStatus`) — with no `FailureReason`. Which of the two a
//! request gets is never decided by the SHAPE of the request: two creates differing
//! only in whether their data was staged are answered differently.
//!
//! The rows of each table differ along what the rule generalises over: which of the two
//! locations (`InputDataConfig.S3Uri`, `InputDataConfig.TuningDataS3Uri`) is the one
//! that does not resolve, how it fails to resolve (no such bucket, nothing under the
//! prefix, only a folder marker under it), how the prefix is shaped (the bucket root, a
//! one-segment prefix, a nested one, a prefix that is a file key, one carrying trailing
//! whitespace), how many files are staged under it, whether folder markers sort ahead of
//! those files, and whether the request names tuning data at all.
//!
//! The last test is the other half of the rule: a refusal has to be GROUNDED in what
//! the object store answered. With no S3 mounted to ask, nothing is known about the
//! locations, so nothing is refused on an assumption — which is why every other suite
//! in this crate may create models whose buckets no S3 holds.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::language_models::{ROLE_ARN, PAST_THE_TRANSITION};
use crate::transcription_jobs::{dispatch, Reply, Transcribe};

/// The region every call of this suite is signed for: the Region the models are created
/// in, and the Region their training data is staged in. A bucket is created with its
/// `LocationConstraint`, so both sides must name the same one.
const REGION: &str = "eu-west-1";

/// A SigV4 credential scope naming `s3`, which is what routes a raw request to the
/// object store. No authenticator is registered, so the signature is never checked; the
/// scope is what routes the request and names the region.
const S3_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/s3/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The bucket the training files below are staged in.
const BUCKET: &str = "transcribe-clm-training-data";

/// A bucket name no `CreateBucket` in this suite ever creates.
const ABSENT_BUCKET: &str = "transcribe-clm-training-data-missing";

/// The bytes one staged training file holds. A custom language model's training data is
/// plain text, and nothing of its CONTENT is reported by any operation of this family —
/// the `LanguageModel` shape has no member for it — so one short payload serves every
/// row.
///
/// `pub(crate)`, like [`emulator`], [`call`], [`create_bucket`], [`stage`] and
/// [`create_request`] below: the suite that asserts which model STATUS a job start may
/// apply (`crate::job_start_language_model_status`) needs an object store to ground a
/// model's training failure in, and drives it through these rather than standing up a
/// second S3-mounted emulator that could drift from them.
pub(crate) const TRAINING_FILE: &[u8] = b"the quick brown fox jumps over the lazy dog\n";

/// One emulator: Transcribe and Amazon S3 on a single shared engine behind the
/// production router — what `build_router` assembles, narrowed to the services under
/// test.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_transcribe_transcribe::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// One Transcribe call through the emulator, signed for [`REGION`] so the handler
/// resolves its data in the Region the buckets below live in.
pub(crate) async fn call(emulator: &MultiServiceRouter, operation: &str, input: Value) -> Reply {
    dispatch(emulator, operation, input, Some(REGION)).await
}

/// One path-style request to the emulator's S3 front door: the bucket and the key live
/// in the URI and the body is the object's own bytes, so this is a raw request rather
/// than a modelled call.
async fn s3(
    emulator: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: &[u8],
) -> (u16, bytes::Bytes) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://s3.{REGION}.amazonaws.com{path}"))
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", S3_AUTHORIZATION)
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("the request builds");
    let (parts, body) = emulator.dispatch(request).await.into_parts();
    (parts.status.as_u16(), body)
}

/// `PUT /<bucket>` — `CreateBucket`, naming its `LocationConstraint` so the bucket
/// lives in the Region this suite signs for.
pub(crate) async fn create_bucket(emulator: &MultiServiceRouter, bucket: &str) {
    let payload = format!(
        "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
         <LocationConstraint>{REGION}</LocationConstraint></CreateBucketConfiguration>"
    );
    let (status, body) = s3(emulator, "PUT", &format!("/{bucket}"), payload.as_bytes()).await;
    assert_eq!(
        status,
        200,
        "the bucket {bucket:?} must be created first, got {status} with body {:?}",
        String::from_utf8_lossy(&body)
    );
}

/// `PUT /<bucket>/<key>` — `PutObject`, staging one file of a model's data.
pub(crate) async fn stage(emulator: &MultiServiceRouter, bucket: &str, key: &str, bytes: &[u8]) {
    let (status, body) = s3(emulator, "PUT", &format!("/{bucket}/{key}"), bytes).await;
    assert_eq!(
        status,
        200,
        "the file {bucket}/{key} must be stored, got {status} with body {:?}",
        String::from_utf8_lossy(&body)
    );
}

/// The `CreateLanguageModel` body for `name`, training from `s3_uri` and tuning from
/// `tuning_uri` when one is given.
pub(crate) fn create_request(name: &str, s3_uri: &str, tuning_uri: Option<&str>) -> Value {
    let mut input_data_config = json!({"S3Uri": s3_uri, "DataAccessRoleArn": ROLE_ARN});
    if let Some(tuning_uri) = tuning_uri {
        input_data_config["TuningDataS3Uri"] = json!(tuning_uri);
    }
    json!({
        "ModelName": name,
        "LanguageCode": "en-US",
        "BaseModelName": "WideBand",
        "InputDataConfig": input_data_config
    })
}

/// The `LanguageModel` a `DescribeLanguageModel` of `name` answers with.
async fn describe(emulator: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let reply = call(emulator, "DescribeLanguageModel", json!({"ModelName": name})).await;
    reply.ok(case)["LanguageModel"].clone()
}

/// The refusal a create whose named location the object store says cannot be read
/// gets: entirely fixed text, with neither the URI nor the role in it.
///
/// `pub(crate)`: the suite that asserts what a start may APPLY
/// (`crate::job_start_language_model_status`) drives the same refusal rather than
/// restating it.
pub(crate) const UNREADABLE_S3_URI_MESSAGE: &str =
    "The S3 URI that you provided can't be accessed. Make sure that you have read \
     permission and try your request again.";

/// One row of the table: a model, the data staged for it, and whether the create is
/// SERVED.
struct Row {
    case: &'static str,
    name: &'static str,
    /// The keys staged under [`BUCKET`] before the create runs.
    staged: &'static [&'static str],
    s3_uri: String,
    tuning_uri: Option<String>,
    /// Whether the object store's answer about the locations above leaves the create
    /// servable: `false` when it says one of them cannot be read, which refuses the
    /// create and stores nothing.
    served: bool,
}

/// The rows: every pair of (what is staged, what the create names) this rule
/// generalises over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "training data under a one-segment prefix",
            name: "clm-prefix",
            staged: &["training/calls-1.txt", "training/calls-2.txt"],
            s3_uri: format!("s3://{BUCKET}/training/"),
            tuning_uri: None,
            served: true,
        },
        Row {
            case: "training data under a nested prefix, one file",
            name: "clm-nested",
            staged: &["corpora/en-US/2026/notes.txt"],
            s3_uri: format!("s3://{BUCKET}/corpora/en-US/2026/"),
            tuning_uri: None,
            served: true,
        },
        Row {
            case: "training data named by the file key itself",
            name: "clm-single-file",
            staged: &["one-file/corpus.txt"],
            s3_uri: format!("s3://{BUCKET}/one-file/corpus.txt"),
            tuning_uri: None,
            served: true,
        },
        Row {
            case: "training data at the bucket root, no prefix at all",
            name: "clm-bucket-root",
            staged: &["root-corpus.txt"],
            s3_uri: format!("s3://{BUCKET}"),
            tuning_uri: None,
            served: true,
        },
        Row {
            case: "training and tuning data, both staged",
            name: "clm-tuned",
            staged: &["tuned/training/a.txt", "tuned/tuning/b.txt"],
            s3_uri: format!("s3://{BUCKET}/tuned/training/"),
            tuning_uri: Some(format!("s3://{BUCKET}/tuned/tuning/")),
            served: true,
        },
        Row {
            case: "nothing staged under the training prefix",
            name: "clm-empty-prefix",
            staged: &[],
            s3_uri: format!("s3://{BUCKET}/never-staged/"),
            tuning_uri: None,
            served: false,
        },
        Row {
            case: "only a folder marker under the training prefix",
            name: "clm-marker-only",
            staged: &["marker-only/"],
            s3_uri: format!("s3://{BUCKET}/marker-only/"),
            tuning_uri: None,
            served: false,
        },
        Row {
            case: "folder markers sorting ahead of the files under the prefix",
            name: "clm-markers-then-files",
            staged: &[
                "mixed/a/",
                "mixed/b/",
                "mixed/c/",
                "mixed/d-corpus.txt",
            ],
            s3_uri: format!("s3://{BUCKET}/mixed/"),
            tuning_uri: None,
            served: true,
        },
        Row {
            // `Uri`'s `@pattern ^(s3://|http(s*)://).+$` admits a value with trailing
            // whitespace, and the location it addresses is the same one.
            case: "a training location carrying trailing whitespace",
            name: "clm-padded-uri",
            staged: &["padded/corpus.txt"],
            s3_uri: format!("s3://{BUCKET}/padded/  "),
            tuning_uri: None,
            served: true,
        },
        Row {
            case: "a training bucket the object store does not hold",
            name: "clm-absent-bucket",
            staged: &[],
            s3_uri: format!("s3://{ABSENT_BUCKET}/training/"),
            tuning_uri: None,
            served: false,
        },
        Row {
            case: "training data staged, tuning data not",
            name: "clm-tuning-missing",
            staged: &["half-tuned/training/a.txt"],
            s3_uri: format!("s3://{BUCKET}/half-tuned/training/"),
            tuning_uri: Some(format!("s3://{BUCKET}/half-tuned/tuning/")),
            served: false,
        },
        Row {
            case: "tuning data staged in a bucket the object store does not hold",
            name: "clm-tuning-absent-bucket",
            staged: &["absent-tuning/training/a.txt"],
            s3_uri: format!("s3://{BUCKET}/absent-tuning/training/"),
            tuning_uri: Some(format!("s3://{ABSENT_BUCKET}/tuning/")),
            served: false,
        },
    ]
}

/// A create resolves the data it names against the object store: a location the store
/// answers for is served and settles `COMPLETED`, and one it says cannot be read is
/// refused with the fixed access message and stores nothing.
#[tokio::test]
async fn a_create_is_refused_for_data_the_object_store_cannot_read() {
    let emulator = emulator();
    create_bucket(&emulator, BUCKET).await;
    let rows = rows();

    for row in &rows {
        for key in row.staged {
            stage(&emulator, BUCKET, key, TRAINING_FILE).await;
        }
    }

    for row in &rows {
        let created = call(
            &emulator,
            "CreateLanguageModel",
            create_request(row.name, &row.s3_uri, row.tuning_uri.as_deref()),
        )
        .await;
        if !row.served {
            created.refused(row.case, 400, "BadRequestException");
            let message = created.error_message(row.case);
            assert_eq!(
                message, UNREADABLE_S3_URI_MESSAGE,
                "case {:?}: expected the fixed unreadable-location refusal, got \
                 {message:?}",
                row.case
            );
            // The refused create stored NOTHING: the family's reader does not find the
            // name it asked for.
            call(
                &emulator,
                "DescribeLanguageModel",
                json!({"ModelName": row.name}),
            )
            .await
            .refused(row.case, 400, "BadRequestException");
            continue;
        }
        created.ok(row.case);
        assert_eq!(
            created.member(row.case, "ModelStatus"),
            "IN_PROGRESS",
            "case {:?}: a fresh custom language model must be IN_PROGRESS, got {:?}",
            row.case,
            created.body
        );
        // A model still training has not failed.
        let model = describe(&emulator, row.case, row.name).await;
        assert!(
            model["ModelStatus"] == json!("IN_PROGRESS")
                && model.get("FailureReason").is_none_or(Value::is_null),
            "case {:?}: a model still training reports no FailureReason, got {model:?}",
            row.case
        );
    }

    std::thread::sleep(PAST_THE_TRANSITION);

    for row in rows.iter().filter(|row| row.served) {
        let model = describe(&emulator, row.case, row.name).await;
        assert_eq!(
            model["ModelStatus"],
            json!("COMPLETED"),
            "case {:?}: expected the training to settle COMPLETED, got {model:?}",
            row.case
        );
        assert!(
            model.get("FailureReason").is_none_or(Value::is_null),
            "case {:?}: a model that finished training carries no FailureReason, got \
             {model:?}",
            row.case
        );
        // A settled model is terminal: a second read answers the identical record.
        let again = describe(&emulator, row.case, row.name).await;
        assert_eq!(
            again, model,
            "case {:?}: a settled model must read back identically, got {again:?}",
            row.case
        );
    }

    // The listing reports exactly the models the creates stored, and none of the names
    // a refused create asked for — under either terminal status.
    for status in ["COMPLETED", "FAILED"] {
        let listed = call(
            &emulator,
            "ListLanguageModels",
            json!({"StatusEquals": status, "MaxResults": 100}),
        )
        .await;
        let mut listed_names: Vec<String> = listed.ok(status)["Models"]
            .as_array()
            .expect("a listing carries Models")
            .iter()
            .map(|model| model["ModelName"].as_str().unwrap_or_default().to_string())
            .collect();
        listed_names.sort();
        let mut expected: Vec<String> = rows
            .iter()
            .filter(|row| row.served && status == "COMPLETED")
            .map(|row| row.name.to_string())
            .collect();
        expected.sort();
        assert_eq!(
            listed_names, expected,
            "case {status:?}: the listing must report exactly the models the creates \
             stored"
        );
    }
}

/// A `COMPLETED` model is one a job may name: the model the issue's lifecycle ends with
/// is usable, and the same request naming a model whose training data was never staged
/// reaches a `FAILED` one.
#[tokio::test]
async fn a_completed_model_is_one_a_transcription_job_can_name() {
    let emulator = emulator();
    create_bucket(&emulator, BUCKET).await;
    stage(&emulator, BUCKET, "usable/corpus.txt", TRAINING_FILE).await;
    // The media a job is started on is resolved against the same object store (a start
    // whose `Media.MediaFileUri` points at nothing is refused), so the file the job
    // below names is staged too.
    stage(&emulator, BUCKET, "media/audio.flac", TRAINING_FILE).await;

    let staged = "usable-model";
    let unstaged = "unusable-model";
    call(
        &emulator,
        "CreateLanguageModel",
        create_request(staged, &format!("s3://{BUCKET}/usable/"), None),
    )
    .await
    .ok("the model whose data is staged");
    // The same create, differing only in whether its data was staged, is refused and
    // leaves no model behind.
    call(
        &emulator,
        "CreateLanguageModel",
        create_request(unstaged, &format!("s3://{BUCKET}/not-staged/"), None),
    )
    .await
    .refused("the model whose data is not staged", 400, "BadRequestException");

    std::thread::sleep(PAST_THE_TRANSITION);

    let usable = describe(&emulator, "the staged model", staged).await;
    assert_eq!(
        usable["ModelStatus"],
        json!("COMPLETED"),
        "a model trained from staged data must be ready to use, got {usable:?}"
    );
    call(
        &emulator,
        "DescribeLanguageModel",
        json!({"ModelName": unstaged}),
    )
    .await
    .refused("the unstaged model", 400, "BadRequestException");

    // The model the completed one is: a start that applies it is served, and the stored
    // job reports the settings it was started with.
    let started = call(
        &emulator,
        "StartTranscriptionJob",
        json!({
            "TranscriptionJobName": "job-naming-a-completed-model",
            "Media": {"MediaFileUri": format!("s3://{BUCKET}/media/audio.flac")},
            "LanguageCode": "en-US",
            "ModelSettings": {"LanguageModelName": staged}
        }),
    )
    .await;
    assert_eq!(
        started.ok("a start naming the completed model")["TranscriptionJob"]["ModelSettings"],
        json!({"LanguageModelName": staged}),
        "the started job must report the model it applies, got {:?}",
        started.body
    );
}

/// Nothing is refused on an assumption: with no object store mounted to ask, nothing is
/// known about the locations a create named, so the create is served and the model
/// finishes rather than being refused on a verdict this emulator could not ground — and
/// a `FailureReason` belongs to a `FAILED` model and nothing else.
#[tokio::test]
async fn with_no_object_store_to_ask_nothing_is_refused_on_an_assumption() {
    // The harness every other suite uses: Transcribe alone on its engine.
    let transcribe = Transcribe::new();

    // Rows differ along the FORM of the location, including the `http(s)://` one
    // `Uri`'s own `@pattern` admits (`^(s3://|http(s*)://).+$`), which is not an
    // address an object store answers for either.
    let rows: Vec<(&str, Value)> = vec![
        (
            "an s3 location no object store answers for",
            create_request("ungrounded-s3", "s3://DOC-EXAMPLE-BUCKET/training/", None),
        ),
        (
            "an s3 location with tuning data beside it",
            create_request(
                "ungrounded-tuned",
                "s3://DOC-EXAMPLE-BUCKET/training/",
                Some("s3://DOC-EXAMPLE-BUCKET/tuning/"),
            ),
        ),
        (
            "an https location, which addresses no object store here",
            create_request(
                "ungrounded-https",
                "https://example-bucket.s3.amazonaws.com/training/",
                None,
            ),
        ),
    ];

    for (case, request) in &rows {
        transcribe
            .call("CreateLanguageModel", request.clone())
            .await
            .ok(case);
    }

    std::thread::sleep(PAST_THE_TRANSITION);

    for (case, request) in &rows {
        let name = request["ModelName"].as_str().expect("every row names a model");
        let reply = transcribe
            .call("DescribeLanguageModel", json!({"ModelName": name}))
            .await;
        let model = reply.ok(case)["LanguageModel"].clone();
        assert_eq!(
            model["ModelStatus"],
            json!("COMPLETED"),
            "case {case:?}: with nothing known about its data a model is created and \
             finishes, got {model:?}"
        );
        assert!(
            model.get("FailureReason").is_none_or(Value::is_null),
            "case {case:?}: a model that is not FAILED carries no FailureReason, got \
             {model:?}"
        );
    }
}
