// SPDX-License-Identifier: CC0-1.0

//! `UpdateLanguageModel` end to end over the wire: the one change it makes to a
//! stored custom language model, what it leaves alone, and the two states it refuses.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, so these tests exercise the awsJson1_1 envelope, the boundary
//! constraint validation and the handler's own refusals — not just the handler body.
//! The emulator, the call and the answer assertions are the transcription-job suite's
//! harness (`crate::transcription_jobs`), and the creates and reads are
//! `crate::language_models`' own helpers, reused rather than stood up a second time.
//!
//! THE RULE THIS SUITE IS ABOUT: "Updates the encryption configuration for an
//! existing custom language model. … The model artifacts are re-encrypted in place.
//! No model training is required" (the operation's own doc). So the request's
//! `EncryptionConfiguration` REPLACES the stored one — and a request carrying none
//! leaves the artifacts under the AWS-owned key, which a record spells as no
//! configuration of its own — the optional `DataAccessRoleArn` replaces the role the
//! model reads its data with when it is given, and everything the training
//! established (the language, the base model, the training and tuning locations, the
//! creation instant and the terminal status that data decided) is untouched. The
//! re-encryption is work the service still owes when it answers — "You cannot submit
//! another update while a previous update is in progress. Use [the describe
//! operation] to check the current state of your model" — so the model reports
//! `IN_PROGRESS` until the clock makes that transition due and then settles back in
//! the status it had, and a model the service has not finished with is refused
//! instead of updated.

use serde_json::{json, Value};

use crate::language_models::{
    create, described, language_model_request, PAST_THE_TRANSITION, ROLE_ARN,
};
use crate::transcription_jobs::{Reply, Transcribe};

/// A second IAM role ARN, inside `DataAccessRoleArn`'s own `@pattern`
/// (`^arn:(aws|aws-cn|aws-us-gov|aws-iso-{0,1}[a-z]{0,1}):iam::[0-9]{0,63}:role/...$`,
/// `smithy_query(query="operation", operation="UpdateLanguageModel")`): the role a
/// request rotates a model onto, so a row can tell "the update's role" from
/// [`ROLE_ARN`], the one its create named.
const ROTATED_ROLE_ARN: &str = "arn:aws:iam::111122223333:role/Transcribe-Rotation";

/// A KMS key ARN of the form `KMSKey`'s own doc gives
/// (`arn:partition:kms:region:account:key/key-id`, inside its `@pattern`
/// `^[A-Za-z0-9][A-Za-z0-9:_/+=,@.-]{0,2048}$` —
/// `smithy_query(query="shape", shape="EncryptionConfiguration")`).
fn kms_key(id: &str) -> String {
    format!("arn:aws:kms:us-east-1:111122223333:key/{id}")
}

/// One row of the class: a model to create, the update to apply to it, and what the
/// record must hold afterwards.
struct Row {
    case: &'static str,
    /// The `CreateLanguageModel` request that stores the model.
    create: Value,
    /// The `UpdateLanguageModel` request, `ModelName` included.
    update: Value,
    /// The `EncryptionConfiguration` the record carries after the update, or `None`
    /// for artifacts left under the AWS-owned key.
    expected_encryption: Option<Value>,
}

/// The class of `UpdateLanguageModel` request, one row per axis the rule generalises
/// over: whether the model was created WITH an encryption configuration, whether the
/// update names one, whether that configuration carries an encryption context map,
/// whether the update rotates the data access role, whether the model has tuning data,
/// and the language, base model and form of name the update must leave alone.
///
/// The languages are entries of the model's own `CLMLanguageCode` enum and the base
/// models the whole `BaseModelName` enum (`smithy_query(query="shape",
/// shape="CLMLanguageCode,BaseModelName")`), as `crate::language_models`' create table
/// spans them; the names are built through that suite's own request helper so this
/// family's request shape keeps one home.
fn rows() -> Vec<Row> {
    let with_tuning = |mut request: Value, uri: &str| {
        request["InputDataConfig"]["TuningDataS3Uri"] = json!(uri);
        request
    };
    let with_encryption = |mut request: Value, configuration: Value| {
        request["EncryptionConfiguration"] = configuration;
        request
    };
    vec![
        Row {
            case: "no configuration at all, the update supplies a KMS key",
            create: language_model_request("support-calls", "en-US", "WideBand"),
            update: json!({
                "ModelName": "support-calls",
                "EncryptionConfiguration": {"KMSKey": kms_key("1111aaaa-1111")}
            }),
            expected_encryption: Some(json!({"KMSKey": kms_key("1111aaaa-1111")})),
        },
        Row {
            case: "one key replaced by another, with an encryption context pair",
            create: with_encryption(
                language_model_request("german-support", "de-DE", "NarrowBand"),
                json!({"KMSKey": kms_key("2222bbbb-2222")}),
            ),
            update: json!({
                "ModelName": "german-support",
                "EncryptionConfiguration": {
                    "KMSKey": kms_key("3333cccc-3333"),
                    "KMSEncryptionContext": {"team": "speech"}
                }
            }),
            expected_encryption: Some(json!({
                "KMSKey": kms_key("3333cccc-3333"),
                "KMSEncryptionContext": {"team": "speech"}
            })),
        },
        Row {
            case: "a configuration dropped: no key supplied, so the AWS-owned key",
            create: with_encryption(
                language_model_request("japanese_meetings", "ja-JP", "WideBand"),
                json!({"KMSKey": kms_key("4444dddd-4444")}),
            ),
            update: json!({"ModelName": "japanese_meetings"}),
            expected_encryption: None,
        },
        Row {
            case: "a rotated role beside a new key, on a model with tuning data",
            create: with_tuning(
                language_model_request("aussie-calls", "en-AU", "NarrowBand"),
                "s3://DOC-EXAMPLE-BUCKET/au-tuning/",
            ),
            update: json!({
                "ModelName": "aussie-calls",
                "DataAccessRoleArn": ROTATED_ROLE_ARN,
                "EncryptionConfiguration": {"KMSKey": kms_key("5555eeee-5555")}
            }),
            expected_encryption: Some(json!({"KMSKey": kms_key("5555eeee-5555")})),
        },
        Row {
            case: "only a rotated role, on a single-character name and no configuration",
            create: language_model_request("e", "es-US", "WideBand"),
            update: json!({"ModelName": "e", "DataAccessRoleArn": ROTATED_ROLE_ARN}),
            expected_encryption: None,
        },
        Row {
            case: "a context map of several pairs, on a name at the modelled 200-character ceiling",
            create: with_encryption(
                language_model_request(&"z".repeat(200), "en-GB", "WideBand"),
                json!({
                    "KMSKey": kms_key("6666ffff-6666"),
                    "KMSEncryptionContext": {"env": "staging"}
                }),
            ),
            update: json!({
                "ModelName": "z".repeat(200),
                "EncryptionConfiguration": {
                    "KMSKey": kms_key("7777aaaa-7777"),
                    "KMSEncryptionContext": {"env": "test", "team": "speech"}
                }
            }),
            expected_encryption: Some(json!({
                "KMSKey": kms_key("7777aaaa-7777"),
                "KMSEncryptionContext": {"env": "test", "team": "speech"}
            })),
        },
        Row {
            case: "a role rotated while the configuration is dropped, with tuning data",
            create: with_tuning(
                with_encryption(
                    language_model_request("Hindi.Calls.v2", "hi-IN", "NarrowBand"),
                    json!({"KMSKey": kms_key("8888bbbb-8888")}),
                ),
                "s3://DOC-EXAMPLE-BUCKET/hi-tuning/",
            ),
            update: json!({
                "ModelName": "Hindi.Calls.v2",
                "DataAccessRoleArn": ROTATED_ROLE_ARN
            }),
            expected_encryption: None,
        },
    ]
}

/// The `ModelName` of a row's requests.
fn name_of(request: &Value) -> String {
    request["ModelName"]
        .as_str()
        .expect("every row names a model")
        .to_string()
}

/// The `InputDataConfig` the record holds once `update` has been applied to a model
/// created by `create`: the training and tuning locations are the create's — this
/// operation models no input for either — and the role is the update's only when it
/// named one.
fn expected_input_data_config(create: &Value, update: &Value) -> Value {
    let mut config = create["InputDataConfig"].clone();
    if let Some(role) = update.get("DataAccessRoleArn") {
        config["DataAccessRoleArn"] = role.clone();
    }
    config
}

/// The `EncryptionConfiguration` a described model carries, or `None` when the record
/// holds none (an absent member, or one present as JSON `null`).
fn encryption_of(model: &Value) -> Option<Value> {
    model
        .get("EncryptionConfiguration")
        .filter(|value| !value.is_null())
        .cloned()
}

/// Read a model back through the family's own reader, in `region` when one is given
/// (the unsigned default is the Region this emulator serves a request carrying no
/// SigV4 credential scope in).
async fn describe_in(
    transcribe: &Transcribe,
    case: &str,
    name: &str,
    region: Option<&str>,
) -> Value {
    described(
        &transcribe
            .call_in_region("DescribeLanguageModel", json!({"ModelName": name}), region)
            .await,
        case,
    )
}

/// [`describe_in`] in the Region an unsigned request is served in.
async fn describe(transcribe: &Transcribe, case: &str, name: &str) -> Value {
    describe_in(transcribe, case, name, None).await
}

/// The `ModelStatus` of the model `name`, held in `region`.
async fn status_of(transcribe: &Transcribe, case: &str, name: &str, region: Option<&str>) -> String {
    let model = describe_in(transcribe, case, name, region).await;
    model["ModelStatus"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ModelStatus, got {model:?}"))
        .to_string()
}

/// Poll `DescribeLanguageModel` until the clock-owed transition of the model `name`,
/// held in `region`, has been materialised, and answer the `ModelStatus` it settled
/// in — one home for the poll loop and its bound, whichever Region a row is served
/// in.
async fn settled_status(
    transcribe: &Transcribe,
    case: &str,
    name: &str,
    region: Option<&str>,
) -> String {
    for _ in 0..40 {
        let status = status_of(transcribe, case, name, region).await;
        if status != "IN_PROGRESS" {
            return status;
        }
        std::thread::sleep(PAST_THE_TRANSITION / 10);
    }
    panic!("case {case:?}: the model never left IN_PROGRESS");
}

/// Apply a row's update, asserting the answer reports the record just written.
async fn update(transcribe: &Transcribe, case: &str, request: &Value) -> Reply {
    let reply = transcribe
        .call("UpdateLanguageModel", request.clone())
        .await;
    reply.ok(case);
    assert_eq!(
        reply.member(case, "ModelName"),
        name_of(request),
        "case {case:?}: the answer must name the model just updated, got {:?}",
        reply.body
    );
    reply
}

/// An update replaces the stored encryption configuration and, when it names one, the
/// data access role; it leaves every other fact of the record alone, reports the
/// re-encryption the service still owes as `IN_PROGRESS`, and settles back in the
/// status the model's training data decided.
#[tokio::test]
async fn updating_a_language_model_re_encrypts_it_and_changes_nothing_else() {
    let transcribe = Transcribe::new();
    let rows = rows();

    // A model nothing updates, to show an update reaches exactly the record it names.
    let control = json!({
        "ModelName": "control-model",
        "LanguageCode": "en-US",
        "BaseModelName": "WideBand",
        "InputDataConfig": {
            "S3Uri": "s3://DOC-EXAMPLE-BUCKET/control-training/",
            "DataAccessRoleArn": ROLE_ARN
        },
        "EncryptionConfiguration": {"KMSKey": kms_key("9999cccc-9999")}
    });
    create(&transcribe, "control", &control).await;

    for row in &rows {
        create(&transcribe, row.case, &row.create).await;
    }

    // An update is refused while the service is still working on a model, so every row
    // waits out the training its create started before updating.
    let mut updated: Vec<(&str, String, f64)> = Vec::new();
    for row in &rows {
        let name = name_of(&row.create);
        let settled = settled_status(&transcribe, row.case, &name, None).await;
        assert_eq!(
            settled, "COMPLETED",
            "case {:?}: no object store answered for this row's training data, so its \
             training must settle COMPLETED, got {settled:?}",
            row.case
        );
        let before = describe(&transcribe, row.case, &name).await;
        let create_time = before["CreateTime"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {:?}: expected a CreateTime", row.case));

        let reply = update(&transcribe, row.case, &row.update).await;
        // The re-encryption is work the service still owes when it answers, and the
        // operation's own doc sends the caller to the describe operation to watch for
        // it finishing.
        assert_eq!(
            reply.member(row.case, "ModelStatus"),
            "IN_PROGRESS",
            "case {:?}: an update that has not finished re-encrypting must report \
             IN_PROGRESS, got {:?}",
            row.case,
            reply.body
        );
        let answered_modified = reply.timestamp(row.case, "LastModifiedTime");
        assert!(
            answered_modified > create_time,
            "case {:?}: the update changed the record, so its LastModifiedTime \
             {answered_modified} must move past its CreateTime {create_time}, got {:?}",
            row.case,
            reply.body
        );

        // The new configuration is what a reader sees at once: the artifacts are
        // re-encrypted in place, and only the status still says the service is working.
        let model = describe(&transcribe, row.case, &name).await;
        assert_eq!(
            encryption_of(&model),
            row.expected_encryption,
            "case {:?}: expected the encryption configuration the update named, got \
             {model:?}",
            row.case
        );
        assert_eq!(
            model["InputDataConfig"],
            expected_input_data_config(&row.create, &row.update),
            "case {:?}: an update may only change the data access role, got {model:?}",
            row.case
        );
        assert_eq!(
            (
                model["ModelName"].clone(),
                model["LanguageCode"].clone(),
                model["BaseModelName"].clone(),
                model["CreateTime"].as_f64(),
                model["ModelStatus"].clone(),
            ),
            (
                json!(name),
                row.create["LanguageCode"].clone(),
                row.create["BaseModelName"].clone(),
                Some(create_time),
                json!("IN_PROGRESS"),
            ),
            "case {:?}: no model training is required, so an update leaves every fact \
             the training established, got {model:?}",
            row.case
        );
        updated.push((row.case, name, answered_modified));
    }

    // The transition the clock owes each re-encryption: the model settles back in the
    // status its training data decided — an update runs no training, so it cannot
    // change which terminal status a model ends in.
    for (case, name, answered_modified) in &updated {
        let settled = settled_status(&transcribe, case, name, None).await;
        assert_eq!(
            settled, "COMPLETED",
            "case {case:?}: a re-encrypted model settles back in the status its \
             training data decided, got {settled:?}"
        );
        let model = describe(&transcribe, case, name).await;
        assert!(
            model["LastModifiedTime"].as_f64().unwrap_or_default() > *answered_modified,
            "case {case:?}: the record changed again when the re-encryption ended, so \
             its LastModifiedTime must move past the {answered_modified} the update \
             answered, got {model:?}"
        );
        assert!(
            model.get("FailureReason").is_none_or(Value::is_null),
            "case {case:?}: a model that finished carries no FailureReason, got {model:?}"
        );
    }

    // Every row kept its own configuration: an update reaches exactly the record it
    // names.
    for row in &rows {
        let model = describe(&transcribe, row.case, &name_of(&row.create)).await;
        assert_eq!(
            encryption_of(&model),
            row.expected_encryption,
            "case {:?}: the configuration this row's update named must still be the \
             record's, got {model:?}",
            row.case
        );
    }
    let control_model = describe(&transcribe, "control", "control-model").await;
    assert_eq!(
        (
            encryption_of(&control_model),
            control_model["InputDataConfig"].clone()
        ),
        (
            Some(control["EncryptionConfiguration"].clone()),
            control["InputDataConfig"].clone()
        ),
        "case \"control\": a model no update named keeps the configuration its create \
         gave it, got {control_model:?}"
    );
}

/// An update is refused while the service is working on the model — the training a
/// create started, or a re-encryption a previous update started — and the refusal
/// changes nothing.
#[tokio::test]
async fn an_update_is_refused_while_the_service_is_working_on_the_model() {
    let transcribe = Transcribe::new();

    for row in &rows() {
        let name = name_of(&row.create);
        create(&transcribe, row.case, &row.create).await;

        // "Your custom language model must not be in the IN_PROGRESS state when you
        // call this operation" (the operation's own doc), which is the refusal
        // `BadRequestException`'s own documentation covers ("if it's in a non-terminal
        // state (such as IN PROGRESS)").
        // The precondition this row is about, read before the refusal so a lost race
        // against the transition fails here and says so, rather than as a confusing
        // mismatch below.
        let working = status_of(&transcribe, row.case, &name, None).await;
        assert_eq!(
            working, "IN_PROGRESS",
            "case {:?}: this row needs the create's training still running, got \
             {working:?}",
            row.case
        );
        let refused = transcribe
            .call("UpdateLanguageModel", row.update.clone())
            .await;
        refused.refused(row.case, 400, "BadRequestException");
        let message = refused.error_message(row.case);
        assert!(
            message.contains("IN_PROGRESS") && message.contains(&name),
            "case {:?}: the refusal must name the model and the state that blocks the \
             update, got {message:?}",
            row.case
        );
        let model = describe(&transcribe, row.case, &name).await;
        assert_eq!(
            encryption_of(&model),
            row.create
                .get("EncryptionConfiguration")
                .filter(|value| !value.is_null())
                .cloned(),
            "case {:?}: a refused update must leave the record as its create stored \
             it, got {model:?}",
            row.case
        );

        // Once the training has finished the same update goes through — and a SECOND
        // update, submitted while that re-encryption is still in progress, is refused
        // the same way: "You cannot submit another update while a previous update is
        // in progress."
        let settled = settled_status(&transcribe, row.case, &name, None).await;
        assert_eq!(
            settled, "COMPLETED",
            "case {:?}: this row's training must settle COMPLETED, got {settled:?}",
            row.case
        );
        let first = update(&transcribe, row.case, &row.update).await;
        assert_eq!(
            first.member(row.case, "ModelStatus"),
            "IN_PROGRESS",
            "case {:?}: the second update below needs the first one's re-encryption \
             still in progress, got {:?}",
            row.case,
            first.body
        );

        let again = &format!("{}, a second update while the first is in progress", row.case);
        let overlapping = transcribe
            .call("UpdateLanguageModel", row.update.clone())
            .await;
        overlapping.refused(again, 400, "BadRequestException");
        let model = describe(&transcribe, again, &name).await;
        assert_eq!(
            encryption_of(&model),
            row.expected_encryption,
            "case {again:?}: the refused second update must leave the first one's \
             configuration, got {model:?}"
        );

        // And once that re-encryption has finished, an update is accepted again.
        let settled = settled_status(&transcribe, again, &name, None).await;
        assert_eq!(
            settled, "COMPLETED",
            "case {again:?}: the re-encryption must settle back COMPLETED, got \
             {settled:?}"
        );
        update(&transcribe, again, &row.update).await;
    }
}

/// An update needs the model: a name the Region this request is served in holds
/// nothing under is the service's absent-resource refusal, and nothing is created by
/// it.
#[tokio::test]
async fn updating_a_language_model_needs_that_model_in_this_region() {
    let transcribe = Transcribe::new();

    // A name no model was ever created under, in several of the forms `ModelName`
    // admits (`^[0-9a-zA-Z._-]+$`, length 1..200).
    for name in ["unknown-model", "e", "Dotted.Name_9", &"z".repeat(200)] {
        let case = &format!("absent model {name:?}");
        let refused = transcribe
            .call("UpdateLanguageModel", json!({"ModelName": name}))
            .await;
        refused.refused(case, 400, "BadRequestException");
        let message = refused.error_message(case);
        assert!(
            message.contains("language model") && message.contains("couldn't be found"),
            "case {case:?}: the refusal must be this service's absent-resource text, \
             got {message:?}"
        );
        // The refusal created nothing: the family's own reader still finds no model.
        transcribe
            .call("DescribeLanguageModel", json!({"ModelName": name}))
            .await
            .refused(case, 400, "BadRequestException");
    }

    // A model belongs to the one Region whose endpoint created it, so an update signed
    // for another Region reaches nothing — and the record it did not reach is unchanged.
    let case = "a model held in another Region";
    let request = language_model_request("regional-model", "en-US", "WideBand");
    let created = transcribe
        .call_in_region("CreateLanguageModel", request.clone(), Some("us-west-2"))
        .await;
    created.ok(case);
    let settled = settled_status(&transcribe, case, "regional-model", Some("us-west-2")).await;
    assert_eq!(
        settled, "COMPLETED",
        "case {case:?}: this model's training must settle COMPLETED, got {settled:?}"
    );

    let elsewhere = json!({
        "ModelName": "regional-model",
        "EncryptionConfiguration": {"KMSKey": kms_key("aaaa1111-aaaa")}
    });
    transcribe
        .call_in_region("UpdateLanguageModel", elsewhere.clone(), Some("eu-west-1"))
        .await
        .refused(case, 400, "BadRequestException");

    // In its own Region the same update goes through.
    let accepted = transcribe
        .call_in_region("UpdateLanguageModel", elsewhere.clone(), Some("us-west-2"))
        .await;
    accepted.ok(case);
    let model = describe_in(&transcribe, case, "regional-model", Some("us-west-2")).await;
    assert_eq!(
        encryption_of(&model),
        Some(json!({"KMSKey": kms_key("aaaa1111-aaaa")})),
        "case {case:?}: the update must reach the model held in the Region it was \
         signed for, got {model:?}"
    );
}
