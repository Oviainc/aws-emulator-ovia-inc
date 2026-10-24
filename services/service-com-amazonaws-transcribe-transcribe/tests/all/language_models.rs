// SPDX-License-Identifier: CC0-1.0

//! The custom LANGUAGE model family, end to end over the wire:
//! `CreateLanguageModel` -> `DescribeLanguageModel` -> `DeleteLanguageModel`, plus
//! the tag set of a language model through the three tagging operations.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, so these tests exercise the awsJson1_1 envelope, the boundary
//! constraint validation and the handlers' own refusals — not just the handler
//! bodies. The emulator, the call and the answer assertions are the transcription-job
//! suite's harness (`crate::transcription_jobs`), reused rather than stood up a
//! second time.
//!
//! WHAT A CUSTOM LANGUAGE MODEL IS HERE: its training and tuning data can only come
//! from the Amazon S3 location at `InputDataConfig$S3Uri`, which `CreateLanguageModel`
//! models `@required` with no inline alternative, and this emulator reads no object
//! store. So the RECORD is real — name, language, base model, input data config,
//! status, timestamps and tags — while the training it cannot run ends in the
//! service's own modelled `FAILED` state with a `FailureReason`. These tests assert
//! exactly that, and never a `COMPLETED` model.
//!
//! THE RULE THIS SUITE IS ABOUT (`DeleteLanguageModel`): a delete removes the named
//! record and nothing else, refuses a record the service has not finished with
//! (`BadRequestException`, whose own documentation covers "a non-terminal state (such
//! as IN PROGRESS)"), and is idempotent for a name the account holds nothing under.

use std::time::Duration;

use serde_json::{json, Value};

use crate::transcription_jobs::{Reply, Transcribe};

/// Long enough for the `IN_PROGRESS -> FAILED` transition of a model created during
/// the test to have fallen due: one span of `crate::state::MODEL_TRANSITION_DELAY`
/// (500 ms), with room for a slow machine.
///
/// Waited out with `std::thread::sleep`: the workspace enables tokio's `macros`,
/// `rt` and `rt-multi-thread` features only, so `tokio::time::sleep` is not
/// available here.
const PAST_THE_TRANSITION: Duration = Duration::from_millis(900);

/// The region every row of this suite is served in (the rows are unsigned, and this
/// is the region this emulator serves a request carrying no SigV4 credential scope
/// in), so a tagging ARN is predictable.
const REGION: &str = "us-east-1";

/// An IAM role ARN inside `DataAccessRoleArn`'s own `@pattern`
/// (`^arn:(aws|aws-cn|aws-us-gov|aws-iso-{0,1}[a-z]{0,1}):iam::[0-9]{0,63}:role/...$`,
/// `smithy_query(query="shape", shape="InputDataConfig")`).
const ROLE_ARN: &str = "arn:aws:iam::111122223333:role/Admin";

/// The class of `CreateLanguageModel` request, one row per axis the rules generalise
/// over: the model's language, its base model, the form of its name, which URI scheme
/// the training data is named with, whether tuning data and an encryption
/// configuration are given, and whether the request carries tags.
///
/// The languages are entries of the model's own `CLMLanguageCode` enum (`en-US |
/// hi-IN | es-US | en-GB | en-AU | de-DE | ja-JP`) and the base models are the whole
/// `BaseModelName` enum (`NarrowBand | WideBand`), both from
/// `smithy_query(query="shape", shape="CLMLanguageCode,BaseModelName")` — so the rows
/// cover entries no recorded request used. The URI schemes are the ones `Uri`'s own
/// `@pattern` admits (`^(s3://|http(s*)://).+$`).
fn language_model_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "en-US on WideBand, training data only",
            json!({
                "ModelName": "support-calls",
                "LanguageCode": "en-US",
                "BaseModelName": "WideBand",
                "InputDataConfig": {
                    "S3Uri": "s3://DOC-EXAMPLE-BUCKET/my-model-training-data/",
                    "DataAccessRoleArn": ROLE_ARN
                }
            }),
        ),
        (
            "hi-IN on NarrowBand, with tuning data and a dotted, mixed-case name",
            json!({
                "ModelName": "Hindi.Calls.v2",
                "LanguageCode": "hi-IN",
                "BaseModelName": "NarrowBand",
                "InputDataConfig": {
                    "S3Uri": "s3://DOC-EXAMPLE-BUCKET/hi-training/",
                    "TuningDataS3Uri": "s3://DOC-EXAMPLE-BUCKET/hi-tuning/",
                    "DataAccessRoleArn": ROLE_ARN
                }
            }),
        ),
        (
            "ja-JP on WideBand, https training data, an underscored name and tags",
            json!({
                "ModelName": "japanese_meetings",
                "LanguageCode": "ja-JP",
                "BaseModelName": "WideBand",
                "InputDataConfig": {
                    "S3Uri": "https://example-bucket.s3.amazonaws.com/ja-training/",
                    "DataAccessRoleArn": ROLE_ARN
                },
                "Tags": [{"Key": "team", "Value": "speech"}, {"Key": "env", "Value": "test"}]
            }),
        ),
        (
            "de-DE on NarrowBand, http training data and an encryption configuration",
            json!({
                "ModelName": "german-support",
                "LanguageCode": "de-DE",
                "BaseModelName": "NarrowBand",
                "InputDataConfig": {
                    "S3Uri": "http://example-bucket.s3.amazonaws.com/de-training/",
                    "DataAccessRoleArn": ROLE_ARN
                },
                "EncryptionConfiguration": {
                    "KMSKey": "arn:aws:kms:us-east-1:111122223333:key/abcd-1234"
                }
            }),
        ),
        (
            "en-GB on WideBand, a name at the modelled 200-character ceiling",
            json!({
                "ModelName": "z".repeat(200),
                "LanguageCode": "en-GB",
                "BaseModelName": "WideBand",
                "InputDataConfig": {
                    "S3Uri": "s3://DOC-EXAMPLE-BUCKET/long-name-training/",
                    "DataAccessRoleArn": ROLE_ARN
                }
            }),
        ),
        (
            "en-AU on NarrowBand, training and tuning data in one bucket",
            json!({
                "ModelName": "aussie-calls",
                "LanguageCode": "en-AU",
                "BaseModelName": "NarrowBand",
                "InputDataConfig": {
                    "S3Uri": "s3://DOC-EXAMPLE-BUCKET/au-training/",
                    "TuningDataS3Uri": "s3://DOC-EXAMPLE-BUCKET/au-tuning/",
                    "DataAccessRoleArn": ROLE_ARN
                }
            }),
        ),
        (
            "es-US on WideBand, a single-character name",
            json!({
                "ModelName": "e",
                "LanguageCode": "es-US",
                "BaseModelName": "WideBand",
                "InputDataConfig": {
                    "S3Uri": "s3://DOC-EXAMPLE-BUCKET/es-training/",
                    "DataAccessRoleArn": ROLE_ARN
                }
            }),
        ),
    ]
}

/// The `ModelName` member of a request row.
fn name_of(request: &Value) -> String {
    request["ModelName"]
        .as_str()
        .expect("every row names a model")
        .to_string()
}

/// The `InputDataConfig$S3Uri` member of a request row.
fn s3_uri_of(request: &Value) -> String {
    request["InputDataConfig"]["S3Uri"]
        .as_str()
        .expect("every row names its training data")
        .to_string()
}

/// Whether a member is absent from a JSON object (or present as JSON `null`), which is
/// how an unset modelled member reaches the wire.
fn is_absent(object: &Value, member: &str) -> bool {
    object.get(member).is_none_or(Value::is_null)
}

/// The `LanguageModel` a `DescribeLanguageModel` answer carries.
fn described(reply: &Reply, case: &str) -> Value {
    reply.ok(case)["LanguageModel"].clone()
}

/// Create the model `request` describes, asserting the answer is the record just
/// stored.
async fn create(transcribe: &Transcribe, case: &str, request: &Value) -> Reply {
    let created = transcribe.call("CreateLanguageModel", request.clone()).await;
    created.ok(case);
    assert_eq!(
        (
            created.member(case, "ModelName"),
            created.member(case, "LanguageCode"),
            created.member(case, "BaseModelName"),
        ),
        (
            name_of(request),
            request["LanguageCode"].as_str().expect("a language").to_string(),
            request["BaseModelName"].as_str().expect("a base model").to_string(),
        ),
        "case {case:?}: the answer must report the record just stored, got {:?}",
        created.body
    );
    // "When the status displays as COMPLETED, your model is ready to use" (model doc
    // for `CreateLanguageModelResponse$ModelStatus`): a new model is still training.
    assert_eq!(
        created.member(case, "ModelStatus"),
        "IN_PROGRESS",
        "case {case:?}: a fresh custom language model must be IN_PROGRESS, got {:?}",
        created.body
    );
    assert_eq!(
        created.body["InputDataConfig"], request["InputDataConfig"],
        "case {case:?}: the answer must carry the stored input data config, got {:?}",
        created.body
    );
    created
}

/// A created language model is stored, reported back as `IN_PROGRESS`, readable
/// through `DescribeLanguageModel`, and ends `FAILED` with a reason naming the Amazon
/// S3 location this emulator does not read once the clock makes that transition due.
#[tokio::test]
async fn a_created_language_model_is_in_progress_then_fails_and_describes_back() {
    let transcribe = Transcribe::new();
    let rows = language_model_rows();
    let mut created_at: Vec<(&str, String, String, f64)> = Vec::new();

    for (case, request) in &rows {
        let name = name_of(request);
        create(&transcribe, case, request).await;

        // The record reads back through the family's own reader, with the same facts.
        let model = described(
            &transcribe
                .call("DescribeLanguageModel", json!({"ModelName": name}))
                .await,
            case,
        );
        assert_eq!(
            (
                model["ModelName"].clone(),
                model["LanguageCode"].clone(),
                model["BaseModelName"].clone(),
                model["ModelStatus"].clone(),
                model["InputDataConfig"].clone(),
            ),
            (
                json!(name),
                request["LanguageCode"].clone(),
                request["BaseModelName"].clone(),
                json!("IN_PROGRESS"),
                request["InputDataConfig"].clone(),
            ),
            "case {case:?}: the reader must report the record just created, got {model:?}"
        );
        // "If `false`, your custom language model is using the most up-to-date base
        // model" (model doc for `LanguageModel$UpgradeAvailability`): this emulator
        // never supersedes a base model.
        assert_eq!(
            model["UpgradeAvailability"],
            json!(false),
            "case {case:?}: no base model here is ever superseded, got {model:?}"
        );
        assert!(
            is_absent(&model, "FailureReason"),
            "case {case:?}: a model still training has not failed, got {model:?}"
        );
        // The encryption configuration round-trips when the create named one, and
        // stays absent when it did not — nothing is invented for it.
        assert_eq!(
            model.get("EncryptionConfiguration").cloned(),
            request.get("EncryptionConfiguration").cloned(),
            "case {case:?}: the encryption configuration must round-trip exactly, got \
             {model:?}"
        );
        let create_time = model["CreateTime"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: expected a CreateTime, got {model:?}"));
        assert_eq!(
            model["LastModifiedTime"].as_f64(),
            Some(create_time),
            "case {case:?}: a record nothing has changed yet was last modified when it \
             was created, got {model:?}"
        );

        created_at.push((case, name, s3_uri_of(request), create_time));
    }

    // The transition the clock owes every one of these records: the training that
    // would have had to READ the Amazon S3 location ends in the modelled failure
    // state.
    std::thread::sleep(PAST_THE_TRANSITION);

    for (case, name, s3_uri, create_time) in &created_at {
        let model = described(
            &transcribe
                .call("DescribeLanguageModel", json!({"ModelName": name}))
                .await,
            case,
        );
        assert_eq!(
            model["ModelStatus"],
            json!("FAILED"),
            "case {case:?}: a model whose training data is never read must end FAILED, \
             got {model:?}"
        );
        let reason = model["FailureReason"].as_str().unwrap_or_default();
        assert!(
            reason.contains(s3_uri),
            "case {case:?}: the failure reason must name the location that was not read \
             ({s3_uri}), got {reason:?}"
        );
        let last_modified = model["LastModifiedTime"].as_f64().unwrap_or_default();
        assert!(
            last_modified > *create_time
                && model["CreateTime"].as_f64() == Some(*create_time),
            "case {case:?}: the record changed when the training ended, so its \
             LastModifiedTime must move past its unchanged CreateTime {create_time}, got \
             {model:?}"
        );
    }
}

/// `DeleteLanguageModel` removes the named record once the service has finished with
/// it, refuses one it has not, is idempotent for a name the account holds nothing
/// under, and touches no other record.
#[tokio::test]
async fn deleting_a_language_model_removes_exactly_that_record() {
    let transcribe = Transcribe::new();
    let rows = language_model_rows();

    // A delete of a record the service is still working on is refused, and the record
    // survives the refusal: "This can occur when the entity you're trying to delete
    // doesn't exist or if it's in a non-terminal state (such as IN PROGRESS)"
    // (`BadRequestException`'s own documentation).
    for (case, request) in &rows {
        let name = name_of(request);
        create(&transcribe, case, request).await;

        let refused = transcribe
            .call("DeleteLanguageModel", json!({"ModelName": name}))
            .await;
        refused.refused(case, 400, "BadRequestException");
        let message = refused.error_message(case);
        assert!(
            message.contains("IN_PROGRESS"),
            "case {case:?}: the refusal must name the state that blocks the delete, got \
             {message:?}"
        );
        let model = described(
            &transcribe
                .call("DescribeLanguageModel", json!({"ModelName": name}))
                .await,
            case,
        );
        assert_eq!(
            model["ModelStatus"],
            json!("IN_PROGRESS"),
            "case {case:?}: a refused delete must leave the record as it was, got \
             {model:?}"
        );
    }

    // Once each record has reached a terminal status the delete goes through, and only
    // the named record goes: every row is deleted in turn and the ones not yet deleted
    // are still readable.
    std::thread::sleep(PAST_THE_TRANSITION);

    let names: Vec<String> = rows.iter().map(|(_, request)| name_of(request)).collect();
    for (index, (case, request)) in rows.iter().enumerate() {
        let name = name_of(request);
        transcribe
            .call("DeleteLanguageModel", json!({"ModelName": name}))
            .await
            .ok_status(case, 200);

        // The record is gone: the family's own reader no longer finds it.
        transcribe
            .call("DescribeLanguageModel", json!({"ModelName": name}))
            .await
            .refused(case, 400, "BadRequestException");

        // A delete is idempotent: the same call again is this operation's ordinary
        // empty success, not a failure.
        transcribe
            .call("DeleteLanguageModel", json!({"ModelName": name}))
            .await
            .ok_status(&format!("{case}, deleted twice"), 200);

        // Nothing else was removed: every row after this one is still readable.
        for other in &names[index + 1..] {
            let model = described(
                &transcribe
                    .call("DescribeLanguageModel", json!({"ModelName": other}))
                    .await,
                case,
            );
            assert_eq!(
                model["ModelName"],
                json!(other),
                "case {case:?}: deleting {name:?} must leave {other:?} alone, got \
                 {model:?}"
            );
        }
    }

    // A name this account holds no model under leaves nothing to do, whatever form it
    // takes: absence is the state the caller asked for. This operation models no
    // `NotFoundException` at all.
    for name in ["never-created", "a", "Mixed.Case_name-9", &"z".repeat(200)] {
        transcribe
            .call("DeleteLanguageModel", json!({"ModelName": name}))
            .await
            .ok_status(&format!("a delete of {name:?}, which was never created"), 200);
    }
}

/// A language model name is claimed by exactly one create, and a delete frees it
/// again — the store, not a cache, is what answers the second create.
#[tokio::test]
async fn a_language_model_name_is_claimed_once_and_a_delete_frees_it() {
    let transcribe = Transcribe::new();
    let request = json!({
        "ModelName": "shared-name",
        "LanguageCode": "en-US",
        "BaseModelName": "WideBand",
        "InputDataConfig": {
            "S3Uri": "s3://DOC-EXAMPLE-BUCKET/my-model-training-data/",
            "DataAccessRoleArn": ROLE_ARN
        }
    });

    create(&transcribe, "the first create of a name", &request).await;

    let conflict = transcribe
        .call("CreateLanguageModel", request.clone())
        .await;
    conflict.refused("a second create of one name", 400, "ConflictException");
    let message = conflict.error_message("a second create of one name");
    assert!(
        message.contains("The requested language model name already exists"),
        "the conflict must name the language model kind, got {message:?}"
    );
    assert!(
        !message.contains("shared-name"),
        "the conflict must not echo the requested name, got {message:?}"
    );

    // The same name in another family is a different resource, so it is free.
    transcribe
        .call(
            "CreateVocabulary",
            json!({
                "VocabularyName": "shared-name",
                "LanguageCode": "en-US",
                "Phrases": ["Aardvark"]
            }),
        )
        .await
        .ok("the same name as a custom vocabulary");

    // Once the model is deleted its name is free again, and the model created under it
    // is a NEW record: the one the second create described, not the first.
    std::thread::sleep(PAST_THE_TRANSITION);
    transcribe
        .call("DeleteLanguageModel", json!({"ModelName": "shared-name"}))
        .await
        .ok_status("delete the model holding the name", 200);

    let mut recreated = request.clone();
    recreated["LanguageCode"] = json!("es-US");
    recreated["BaseModelName"] = json!("NarrowBand");
    create(&transcribe, "a create after the delete", &recreated).await;

    let model = described(
        &transcribe
            .call("DescribeLanguageModel", json!({"ModelName": "shared-name"}))
            .await,
        "a create after the delete",
    );
    assert_eq!(
        (
            model["LanguageCode"].clone(),
            model["BaseModelName"].clone(),
            model["ModelStatus"].clone()
        ),
        (json!("es-US"), json!("NarrowBand"), json!("IN_PROGRESS")),
        "the recreated model must be the record the second create described, got \
         {model:?}"
    );
}

/// A language model carries the tags its create named, the three tagging operations
/// address it through its own ARN resource type, and the tag set goes when the model
/// does.
#[tokio::test]
async fn tags_round_trip_on_a_language_model_and_go_with_it() {
    let transcribe = Transcribe::new();
    let request = json!({
        "ModelName": "tagged-model",
        "LanguageCode": "en-US",
        "BaseModelName": "WideBand",
        "InputDataConfig": {
            "S3Uri": "s3://DOC-EXAMPLE-BUCKET/my-model-training-data/",
            "DataAccessRoleArn": ROLE_ARN
        },
        "Tags": [{"Key": "team", "Value": "speech"}]
    });
    create(&transcribe, "create the model to tag", &request).await;

    // The resource type is `language-model`, one of the valid values
    // `TagResource`'s own `ResourceArn` doc lists ("transcription-job,
    // medical-transcription-job, vocabulary, medical-vocabulary, vocabulary-filter,
    // and language-model").
    let resource_arn = format!(
        "arn:aws:transcribe:{REGION}:{account}:language-model/tagged-model",
        account = engine::emulator_account_id()
    );

    let listed = transcribe
        .call("ListTagsForResource", json!({"ResourceArn": resource_arn}))
        .await;
    listed.ok("the tags the create named");
    assert_eq!(
        listed.body["Tags"],
        json!([{"Key": "team", "Value": "speech"}]),
        "the create's own tags must be stored on the record, got {:?}",
        listed.body
    );

    transcribe
        .call(
            "TagResource",
            json!({
                "ResourceArn": resource_arn,
                "Tags": [{"Key": "env", "Value": "test"}]
            }),
        )
        .await
        .ok("tag the model");
    let after_tag = transcribe
        .call("ListTagsForResource", json!({"ResourceArn": resource_arn}))
        .await;
    after_tag.ok("after tagging");
    assert_eq!(
        after_tag.body["Tags"],
        json!([{"Key": "team", "Value": "speech"}, {"Key": "env", "Value": "test"}]),
        "tagging must merge into the stored set, got {:?}",
        after_tag.body
    );

    transcribe
        .call(
            "UntagResource",
            json!({"ResourceArn": resource_arn, "TagKeys": ["team"]}),
        )
        .await
        .ok_status("untag the model", 200);
    let after_untag = transcribe
        .call("ListTagsForResource", json!({"ResourceArn": resource_arn}))
        .await;
    after_untag.ok("after untagging");
    assert_eq!(
        after_untag.body["Tags"],
        json!([{"Key": "env", "Value": "test"}]),
        "untagging must remove exactly the named keys, got {:?}",
        after_untag.body
    );

    // Deleting the model takes its tag set with it: the ARN then names nothing, which
    // is this service's `BadRequestException` rather than an empty tag set.
    std::thread::sleep(PAST_THE_TRANSITION);
    transcribe
        .call("DeleteLanguageModel", json!({"ModelName": "tagged-model"}))
        .await
        .ok_status("delete the tagged model", 200);
    transcribe
        .call("ListTagsForResource", json!({"ResourceArn": resource_arn}))
        .await
        .refused("the tags of a deleted model", 400, "BadRequestException");
}

/// A create outside a modelled constraint is refused at the boundary, before the
/// handler runs, and stores nothing a delete or a describe could find.
#[tokio::test]
async fn a_create_outside_the_modelled_constraints_stores_nothing() {
    let transcribe = Transcribe::new();

    // Rows differ in WHICH modelled constraint they break: `ModelName`'s
    // `@pattern ^[0-9a-zA-Z._-]+$`, one character past its `@length` ceiling of 200,
    // and `Uri`'s `@pattern ^(s3://|http(s*)://).+$` on the training data location
    // (`smithy_query(query="operation", operation="CreateLanguageModel")`).
    let rows: Vec<(&str, Value)> = vec![
        (
            "a model name outside the modelled pattern",
            json!({
                "ModelName": "has spaces",
                "LanguageCode": "en-US",
                "BaseModelName": "WideBand",
                "InputDataConfig": {
                    "S3Uri": "s3://DOC-EXAMPLE-BUCKET/training/",
                    "DataAccessRoleArn": ROLE_ARN
                }
            }),
        ),
        (
            "a model name one past the modelled length ceiling",
            json!({
                "ModelName": "z".repeat(201),
                "LanguageCode": "en-US",
                "BaseModelName": "WideBand",
                "InputDataConfig": {
                    "S3Uri": "s3://DOC-EXAMPLE-BUCKET/training/",
                    "DataAccessRoleArn": ROLE_ARN
                }
            }),
        ),
        (
            "training data under a scheme the modelled pattern excludes",
            json!({
                "ModelName": "ftp-training",
                "LanguageCode": "en-US",
                "BaseModelName": "WideBand",
                "InputDataConfig": {
                    "S3Uri": "ftp://DOC-EXAMPLE-BUCKET/training/",
                    "DataAccessRoleArn": ROLE_ARN
                }
            }),
        ),
        (
            "a data access role ARN outside the modelled pattern",
            json!({
                "ModelName": "bad-role",
                "LanguageCode": "en-US",
                "BaseModelName": "WideBand",
                "InputDataConfig": {
                    "S3Uri": "s3://DOC-EXAMPLE-BUCKET/training/",
                    "DataAccessRoleArn": "not-an-arn"
                }
            }),
        ),
    ];

    for (case, request) in &rows {
        transcribe
            .call("CreateLanguageModel", request.clone())
            .await
            .refused(case, 400, "ValidationException");
    }

    // A refused create stored nothing: the one name among the rows that is itself
    // valid names no record.
    transcribe
        .call("DescribeLanguageModel", json!({"ModelName": "ftp-training"}))
        .await
        .refused("a model whose create was refused", 400, "BadRequestException");
    transcribe
        .call("DeleteLanguageModel", json!({"ModelName": "ftp-training"}))
        .await
        .ok_status("deleting a model whose create was refused", 200);
}
