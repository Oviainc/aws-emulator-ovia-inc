// SPDX-License-Identifier: CC0-1.0

//! `DescribeLanguageModel` as a REPORTER, end to end over the wire.
//!
//! THE RULE THIS SUITE PINS: a describe answers the record stored under the name the
//! request gives EXACTLY — names are compared byte for byte, so two names differing
//! only in case are two different models ("Model names are case sensitive", the
//! model's own doc for `DescribeLanguageModelRequest$ModelName`) — it refuses a name
//! the account holds no model under, and it never changes what a later caller sees:
//! the only write it performs is the status transition the CLOCK already owed the
//! record, which falls due at its stored instant whether or not anybody calls.
//!
//! The lifecycle facts a describe reports (`IN_PROGRESS`, then the modelled `FAILED`
//! with a reason naming the Amazon S3 location this emulator reads nothing from) are
//! asserted by `crate::language_models`; this suite is about the READ itself, so its
//! rows repeat the read and compare answers with each other.
//!
//! Every call goes through the same `MultiServiceRouter` the production router
//! dispatches on, and the harness, the create helper, the wait and the role ARN are
//! the ones `crate::transcription_jobs` and `crate::language_models` already define —
//! reused rather than copied.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::language_models::{
    create, described, language_model_request, language_model_rows, PAST_THE_TRANSITION,
};
use crate::transcription_jobs::Transcribe;

/// Wall-clock now as the epoch seconds awsJson1_1 renders a modelled `DateTime` as.
fn now_epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a clock after the epoch")
        .as_secs_f64()
}

/// Assert `model` is the record `request` created: every member a create names that
/// the `LanguageModel` shape carries back, so an answer sourced from another record
/// cannot pass. `why` says what the comparison is for.
fn assert_is_the_record(model: &Value, request: &Value, case: &str, why: &str) {
    let name = request["ModelName"].as_str().expect("every row names a model");
    assert_eq!(
        (
            model["ModelName"].clone(),
            model["LanguageCode"].clone(),
            model["BaseModelName"].clone(),
            model["InputDataConfig"].clone(),
            model.get("EncryptionConfiguration").cloned(),
        ),
        (
            json!(name),
            request["LanguageCode"].clone(),
            request["BaseModelName"].clone(),
            request["InputDataConfig"].clone(),
            request.get("EncryptionConfiguration").cloned(),
        ),
        "case {case:?}: {why} — expected the record stored under exactly {name:?}, got \
         {model:?}"
    );
}

/// The `LanguageModel` a `DescribeLanguageModel` of `name` answers with.
async fn describe(transcribe: &Transcribe, case: &str, name: &str) -> Value {
    described(
        &transcribe
            .call("DescribeLanguageModel", json!({"ModelName": name}))
            .await,
        case,
    )
}

/// A describe answers the record stored under exactly the name it was given: names
/// differing only in case name different models, and a name the account holds nothing
/// under is refused rather than matched loosely.
#[tokio::test]
async fn a_describe_answers_only_the_exactly_named_model() {
    let transcribe = Transcribe::new();

    // The family's own create table (`crate::language_models::language_model_rows`) is
    // the base: its rows already differ along language, base model, name length (one
    // character and the modelled 200-character ceiling) and the punctuation
    // `ModelName`'s `@pattern ^[0-9a-zA-Z._-]+$` admits, and each row names a
    // different training location — so an answer sourced from the wrong record cannot
    // pass. Two rows are added here, whose names differ from an existing row's ONLY in
    // case, which is the axis this suite is about.
    let mut rows = language_model_rows();
    rows.push((
        "a name differing from another row's only by upper case",
        language_model_request("SUPPORT-CALLS", "hi-IN", "NarrowBand"),
    ));
    rows.push((
        "a name differing from another row's only by mixed case",
        language_model_request("Support-Calls", "ja-JP", "WideBand"),
    ));

    for (case, request) in &rows {
        create(&transcribe, case, request).await;
    }

    for (case, request) in &rows {
        let name = request["ModelName"].as_str().expect("every row names a model");
        let model = describe(&transcribe, case, name).await;
        assert_is_the_record(&model, request, case, "the describe must answer this record");
    }

    // Names the account holds no model under, each one a variant of a name it DOES
    // hold: a different case, a different separator, a prefix, a suffix, and the
    // 200-character name one character short. None of them matches.
    let one_short_of_the_ceiling = "z".repeat(199);
    let near_misses = [
        "support-CALLS",
        "Support_Calls",
        "support-calls-2",
        "upport-calls",
        "E",
        "Hindi.calls.v2",
        "JAPANESE_MEETINGS",
        &one_short_of_the_ceiling,
    ];
    for name in near_misses {
        let case = format!("a describe of {name:?}, which was never created");
        transcribe
            .call("DescribeLanguageModel", json!({"ModelName": name}))
            .await
            .refused(&case, 400, "BadRequestException");

        // The refused read stored nothing, so it reads the same way the second time.
        transcribe
            .call("DescribeLanguageModel", json!({"ModelName": name}))
            .await
            .refused(&format!("{case}, read twice"), 400, "BadRequestException");
    }

    // Every record the near misses did not match is still exactly as it was — the same
    // comparison the first read loop made, so nothing the refused reads could have
    // touched goes unchecked.
    for (case, request) in &rows {
        let name = request["ModelName"].as_str().expect("every row names a model");
        let model = describe(&transcribe, case, name).await;
        assert_is_the_record(
            &model,
            request,
            case,
            "a refused read of another name must leave this record alone",
        );
    }
}

/// Describing a model changes nothing a later caller sees: repeated reads answer the
/// identical record, and the one status transition a read materialises is the one the
/// clock owed at its stored instant — a model nobody read until after that instant
/// reports it as having happened then, not when it was read.
#[tokio::test]
async fn describing_a_model_changes_nothing_a_later_caller_sees() {
    let transcribe = Transcribe::new();

    // Rows differ along what the record carries, so the comparison of one read with
    // the next covers an absent member as well as a present one. Every member here is
    // one the `LanguageModel` shape carries back, so each one is actually compared.
    let mut rows: Vec<(&str, Value)> = vec![
        (
            "training data only",
            language_model_request("read-twice-plain", "en-US", "WideBand"),
        ),
        (
            "with tuning data",
            language_model_request("read-twice-tuned", "hi-IN", "NarrowBand"),
        ),
        (
            "with an encryption configuration",
            language_model_request("read-twice-encrypted", "ja-JP", "WideBand"),
        ),
    ];
    rows[1].1["InputDataConfig"]["TuningDataS3Uri"] = json!("s3://DOC-EXAMPLE-BUCKET/tuning/");
    rows[2].1["EncryptionConfiguration"] =
        json!({"KMSKey": "arn:aws:kms:us-east-1:111122223333:key/abcd-1234"});

    // A model nobody describes before its transition falls due: its answer must still
    // report the transition as having happened when it was OWED.
    let unread = language_model_request("never-read-until-failed", "en-AU", "NarrowBand");
    create(&transcribe, "the model nobody reads yet", &unread).await;

    // Three reads of a record the service is still working on answer the identical
    // body: a read neither advances the status nor touches the timestamps. The record
    // is only still `IN_PROGRESS` while its transition has not fallen due, so each row
    // is created and read in one tight window of four in-process calls — microseconds
    // of work against the service's own transition span, which is read from the
    // service rather than assumed here and reported when a stalled host misses it.
    let transition_delay =
        service_com_amazonaws_transcribe_transcribe::state::MODEL_TRANSITION_DELAY;
    let mut before_transition: Vec<(&str, &str, Value)> = Vec::new();
    for (case, request) in &rows {
        let name = request["ModelName"].as_str().expect("every row names a model");
        let window = Instant::now();
        create(&transcribe, case, request).await;
        let mut reads = Vec::new();
        for _ in 1..=3 {
            reads.push(describe(&transcribe, case, name).await);
        }
        let elapsed = window.elapsed();
        assert_eq!(
            reads[0]["ModelStatus"],
            json!("IN_PROGRESS"),
            "case {case:?}: a model whose transition is not yet due reads back \
             IN_PROGRESS ({elapsed:?} of a {transition_delay:?} span), got {:?}",
            reads[0]
        );
        for (index, again) in reads.iter().enumerate().skip(1) {
            assert_eq!(
                again,
                &reads[0],
                "case {case:?}: read {read} must answer the identical record \
                 ({elapsed:?} of a {transition_delay:?} span), got {again:?}",
                read = index + 1
            );
        }
        before_transition.push((case, name, reads.swap_remove(0)));
    }

    std::thread::sleep(PAST_THE_TRANSITION);

    // The instant the first post-transition read happens. Everything the records
    // report as having changed must predate it.
    let read_at = now_epoch_seconds();

    for (case, name, before) in &before_transition {
        let after = describe(&transcribe, case, name).await;
        assert_eq!(
            after["ModelStatus"],
            json!("FAILED"),
            "case {case:?}: the transition the clock owed this record must be \
             materialised, got {after:?}"
        );
        // Everything the transition does not touch is untouched.
        for member in [
            "ModelName",
            "LanguageCode",
            "BaseModelName",
            "InputDataConfig",
            "EncryptionConfiguration",
            "CreateTime",
            "UpgradeAvailability",
        ] {
            assert_eq!(
                after.get(member),
                before.get(member),
                "case {case:?}: the transition must leave {member} as it was, got \
                 {after:?}"
            );
        }
        let last_modified = after["LastModifiedTime"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: expected a LastModifiedTime, got {after:?}")
        });
        assert!(
            last_modified < read_at,
            "case {case:?}: the transition was due before this read, so its instant \
             {last_modified} must precede the read at {read_at}"
        );

        // Two more reads answer the identical body: now that the record is terminal a
        // read has nothing left to materialise.
        for read in 2..=3 {
            let again = describe(&transcribe, case, name).await;
            assert_eq!(
                again, after,
                "case {case:?}: read {read} after the transition must answer the \
                 identical record, got {again:?}"
            );
        }
    }

    // The model nobody read until now: its transition is dated when it fell due, not
    // when this first read happened, which is what makes the read a report and not the
    // thing that failed the model.
    let case = "the model nobody read until after its transition";
    let model = describe(&transcribe, case, "never-read-until-failed").await;
    assert_eq!(
        model["ModelStatus"],
        json!("FAILED"),
        "case {case:?}: the clock, not a read, is what makes the transition due, got \
         {model:?}"
    );
    let create_time = model["CreateTime"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a CreateTime, got {model:?}"));
    let last_modified = model["LastModifiedTime"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a LastModifiedTime, got {model:?}"));
    assert!(
        last_modified > create_time && last_modified < read_at,
        "case {case:?}: the record changed when the training ended, between its \
         creation at {create_time} and this first read at {read_at}, got {model:?}"
    );
}

/// A describe outside the modelled constraints on `ModelName` is refused at the
/// boundary, before the handler runs — the same answer whether or not a model of that
/// family exists.
#[tokio::test]
async fn a_describe_outside_the_modelled_constraints_is_refused_at_the_boundary() {
    let transcribe = Transcribe::new();
    create(
        &transcribe,
        "a model to make the store non-empty",
        &language_model_request("constraint-neighbour", "en-US", "WideBand"),
    )
    .await;

    // `ModelName` is `@length 1..200` with `@pattern ^[0-9a-zA-Z._-]+$`
    // (`smithy_query(query="operation", operation="DescribeLanguageModel")`): the rows
    // are each published bound and one past it, plus a character the pattern excludes.
    let one_past_the_ceiling = "z".repeat(201);
    let rows: Vec<(&str, &str)> = vec![
        ("the empty name, one below the modelled floor", ""),
        (
            "a name one past the modelled length ceiling",
            &one_past_the_ceiling,
        ),
        ("a name carrying a character the pattern excludes", "has spaces"),
        ("a name carrying a slash", "language-model/constraint-neighbour"),
    ];
    for (case, name) in &rows {
        transcribe
            .call("DescribeLanguageModel", json!({"ModelName": name}))
            .await
            .refused(case, 400, "ValidationException");
    }

    // Each published bound of the length constraint is served, not refused: one
    // character and 200 characters are both inside it, and neither names a model here.
    for (case, name) in [
        ("the modelled length floor", "z".to_string()),
        ("the modelled length ceiling", "z".repeat(200)),
    ] {
        transcribe
            .call("DescribeLanguageModel", json!({"ModelName": name}))
            .await
            .refused(case, 400, "BadRequestException");
    }

    // The refused reads stored nothing and took nothing: the one model there is still
    // reads back.
    let model = describe(&transcribe, "the neighbour", "constraint-neighbour").await;
    assert_eq!(
        model["ModelName"],
        json!("constraint-neighbour"),
        "a refused read must leave the stored record alone, got {model:?}"
    );
}
