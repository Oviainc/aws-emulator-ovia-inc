// SPDX-License-Identifier: CC0-1.0

//! `ListDocumentClassifiers`: the page it serves over the custom
//! document-classifier records this service persists.
//!
//! The rule under test, over a SET of records rather than one: the operation
//! reports every persisted classifier projected into `DocumentClassifierProperties`
//! in creation order — reversed when `SubmitTimeAfter` is the filter, as that
//! member's own documentation states — narrowed by the single filtering parameter
//! the request set, truncated at `MaxResults`, with a `NextToken` that resumes
//! exactly after the last item of the page and no token on the last one. More than
//! one filtering parameter is refused with `InvalidFilterException`, and a token
//! this service never issued with `InvalidRequestException`.
//!
//! The seeded table is what makes it a rule rather than one example: its rows
//! differ along everything the projection and the filter generalise over — the
//! `ModelStatus` each record carries, the language, the mode, a named version
//! versus none, which optional members are set at all, and the submission instant.
//!
//! `CreateDocumentClassifier` is still a scaffold stub (it is cause 3 of
//! `issues/things-that-do-not-work/20261022T171625Z-comprehend-answers-http-501-notimplemented-for-batchdetectentities-the.md`),
//! so these suites seed the records through the crate's own state API — the SAME
//! store, reached the same way, that the handler reads — and then drive
//! `ListDocumentClassifiers` over the wire.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::types::{
    ClassifierEvaluationMetrics, ClassifierMetadata, DocumentClassifierInputDataConfig,
    DocumentClassifierOutputDataConfig, Tag, VpcConfig, MODEL_STATUS_VALUES,
};
use service_com_amazonaws_comprehend_comprehend_20171127::state::{
    document_classifier_arn, service_state, DocumentClassifier,
};

use crate::comprehend_client::{error_code, Comprehend, ACCOUNT_ID, REGION};

/// How far apart two seeded submissions are, so each record has its own instant
/// for the submit-time filters to select on.
const SUBMISSION_GAP: Duration = Duration::from_secs(60);

/// A whole-second instant in the past to anchor the seeded submissions on.
///
/// Whole seconds matter: the submit-time filters are compared on the modelled
/// timestamp scale, so a bound written as an epoch-second number can be exactly
/// equal to a record's `SubmitTime` — which is what the strict-bound suite needs.
fn base_time() -> SystemTime {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs()
        - 3_600;
    UNIX_EPOCH + Duration::from_secs(secs)
}

/// The classifier records every suite below seeds, oldest first.
///
/// `seq` is a placeholder here: [`seed`] replaces it with the sequence number the
/// store mints, which is what the operation pages on.
fn seeded_records(base: SystemTime) -> Vec<DocumentClassifier> {
    // Row 1: a trained, versioned MULTI_LABEL model with every optional member of
    // `DocumentClassifierProperties` set.
    let mut trained = DocumentClassifier::submitted(
        0,
        document_classifier_arn(REGION, "reviews-classifier", Some("v1")),
        "reviews-classifier".to_string(),
        "en".to_string(),
        base,
    );
    trained.version_name = Some("v1".to_string());
    trained.status = "TRAINED".to_string();
    trained.mode = "MULTI_LABEL".to_string();
    trained.training_start_time = Some(base + Duration::from_secs(5));
    trained.training_end_time = Some(base + Duration::from_secs(25));
    trained.end_time = Some(base + Duration::from_secs(26));
    trained.input_data_config = Some(DocumentClassifierInputDataConfig {
        data_format: Some("COMPREHEND_CSV".to_string()),
        s3_uri: Some("s3://training-bucket/reviews/".to_string()),
        test_s3_uri: Some("s3://training-bucket/reviews-test/".to_string()),
        label_delimiter: Some("|".to_string()),
        ..DocumentClassifierInputDataConfig::default()
    });
    trained.output_data_config = Some(DocumentClassifierOutputDataConfig {
        s3_uri: Some("s3://training-bucket/output/".to_string()),
        kms_key_id: Some("arn:aws:kms:eu-west-1:000000000000:key/abcd-1234".to_string()),
        flywheel_stats_s3_prefix: None,
    });
    trained.classifier_metadata = Some(ClassifierMetadata {
        number_of_labels: Some(7),
        number_of_trained_documents: Some(1_200),
        number_of_test_documents: Some(300),
        evaluation_metrics: Some(ClassifierEvaluationMetrics {
            accuracy: Some(0.91),
            f1_score: Some(0.88),
            ..ClassifierEvaluationMetrics::default()
        }),
    });
    trained.data_access_role_arn =
        Some("arn:aws:iam::123456789012:role/ComprehendRole".to_string());
    trained.volume_kms_key_id = Some("1234abcd-12ab-34cd-56ef-1234567890ab".to_string());
    trained.model_kms_key_id = Some("5678efab-56cd-78ef-90ab-5678901234cd".to_string());
    trained.vpc_config = Some(VpcConfig {
        security_group_ids: vec!["sg-0123456789abcdef0".to_string()],
        subnets: vec![
            "subnet-0123456789abcdef0".to_string(),
            "subnet-0fedcba9876543210".to_string(),
        ],
    });
    trained.tags = vec![Tag {
        key: "team".to_string(),
        value: Some("research".to_string()),
    }];

    // Row 2: the leanest record there is — no version, no data configuration, no
    // security members, and a status that has trained nothing yet.
    let submitted = DocumentClassifier::submitted(
        0,
        document_classifier_arn(REGION, "minimal", None),
        "minimal".to_string(),
        "ja".to_string(),
        base + SUBMISSION_GAP,
    );

    // Row 3: a model mid-training, carrying a version whose name is punctuated
    // differently and only the input side of its configuration.
    let mut training = DocumentClassifier::submitted(
        0,
        document_classifier_arn(REGION, "docs-classifier", Some("2026-04")),
        "docs-classifier".to_string(),
        "pt".to_string(),
        base + 2 * SUBMISSION_GAP,
    );
    training.version_name = Some("2026-04".to_string());
    training.status = "TRAINING".to_string();
    training.training_start_time = Some(base + 2 * SUBMISSION_GAP + Duration::from_secs(3));
    training.input_data_config = Some(DocumentClassifierInputDataConfig {
        data_format: Some("AUGMENTED_MANIFEST".to_string()),
        s3_uri: Some("s3://other-bucket/corpus".to_string()),
        ..DocumentClassifierInputDataConfig::default()
    });
    training.flywheel_arn = Some(::emulator_server::emulator_arn(
        "comprehend",
        REGION,
        "flywheel/docs-flywheel",
    ));

    // Row 4: a model whose training ended in the modelled error status, carrying
    // the `Message` that says why and a source model it was copied from.
    let mut in_error = DocumentClassifier::submitted(
        0,
        document_classifier_arn(REGION, "zh-tw-classifier", None),
        "zh-tw-classifier".to_string(),
        "zh-TW".to_string(),
        base + 3 * SUBMISSION_GAP,
    );
    in_error.status = "IN_ERROR".to_string();
    in_error.message = Some("The training data could not be read.".to_string());
    in_error.end_time = Some(base + 3 * SUBMISSION_GAP + Duration::from_secs(40));
    in_error.source_model_arn = Some(document_classifier_arn(REGION, "reviews-classifier", Some("v1")));

    vec![trained, submitted, training, in_error]
}

/// Seed the store with [`seeded_records`] and hand them back carrying the sequence
/// numbers the store minted, oldest first.
fn seed(comprehend: &Comprehend, base: SystemTime) -> Vec<DocumentClassifier> {
    let state = service_state(&**comprehend.engine())
        .expect("the shared engine carries this service's store");
    let mut store = state.lock();
    let mut seeded = Vec::new();
    for mut record in seeded_records(base) {
        record.seq = store.mint_document_classifier_seq();
        store.insert_document_classifier(record.clone());
        seeded.push(record);
    }
    seeded
}

/// The ARNs of `records`, oldest first — the identity each listed item is matched
/// against.
fn arns(records: &[DocumentClassifier]) -> Vec<String> {
    records
        .iter()
        .map(|record| record.classifier_arn.clone())
        .collect()
}

/// The `DocumentClassifierArn` of every item of one page, in the order served.
fn listed_arns(output: &Value) -> Vec<String> {
    output["DocumentClassifierPropertiesList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            item["DocumentClassifierArn"]
                .as_str()
                .unwrap_or("<no DocumentClassifierArn>")
                .to_string()
        })
        .collect()
}

/// One `ListDocumentClassifiers` call.
async fn list(comprehend: &Comprehend, request: &Value) -> Value {
    comprehend.ok("ListDocumentClassifiers", request).await
}

/// Page through every classifier the request selects, `page_size` at a time,
/// returning the ARNs in the order they were served.
async fn page_through(comprehend: &Comprehend, request: &Value, page_size: usize) -> Vec<String> {
    let mut collected: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut page_request = request.clone();
        page_request["MaxResults"] = json!(page_size);
        if let Some(token) = &next_token {
            page_request["NextToken"] = json!(token);
        }
        let output = list(comprehend, &page_request).await;
        let page = listed_arns(&output);
        assert!(
            page.len() <= page_size,
            "case MaxResults={page_size}: a page returned {} items, got {output}",
            page.len()
        );
        collected.extend(page);
        pages += 1;
        assert!(
            pages <= collected.len() + 1,
            "case MaxResults={page_size}: paging did not terminate after {pages} pages"
        );
        match output["NextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
    }
    collected
}

/// A member's string value on a listed item, or `None` when the item does not
/// carry it.
fn member_str(item: &Value, member: &str) -> Option<String> {
    item.get(member)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Epoch seconds of a modelled timestamp member on a listed item.
fn member_secs(item: &Value, member: &str) -> Option<i64> {
    item.get(member).and_then(Value::as_f64).map(|secs| secs as i64)
}

/// Epoch seconds of a persisted instant.
fn epoch_secs(time: SystemTime) -> i64 {
    i64::try_from(
        time.duration_since(UNIX_EPOCH)
            .expect("a seeded instant is after the epoch")
            .as_secs(),
    )
    .expect("an epoch second fits in i64")
}

/// Assert that `item` is `record` projected into `DocumentClassifierProperties`:
/// every member the record carries is reported with the persisted value, and every
/// member it does not carry is absent.
fn assert_projects(case: &str, item: &Value, record: &DocumentClassifier) {
    for (member, expected) in [
        ("DocumentClassifierArn", Some(record.classifier_arn.clone())),
        ("LanguageCode", Some(record.language_code.clone())),
        ("Status", Some(record.status.clone())),
        ("Mode", Some(record.mode.clone())),
        ("VersionName", record.version_name.clone()),
        ("Message", record.message.clone()),
        ("DataAccessRoleArn", record.data_access_role_arn.clone()),
        ("VolumeKmsKeyId", record.volume_kms_key_id.clone()),
        ("ModelKmsKeyId", record.model_kms_key_id.clone()),
        ("SourceModelArn", record.source_model_arn.clone()),
        ("FlywheelArn", record.flywheel_arn.clone()),
    ] {
        let actual = member_str(item, member);
        assert_eq!(
            actual, expected,
            "case {case:?}: {member}: expected {expected:?}, got {actual:?} in {item}"
        );
    }

    for (member, expected) in [
        ("SubmitTime", Some(record.submit_time)),
        ("EndTime", record.end_time),
        ("TrainingStartTime", record.training_start_time),
        ("TrainingEndTime", record.training_end_time),
    ] {
        let actual = member_secs(item, member);
        let expected = expected.map(epoch_secs);
        assert_eq!(
            actual, expected,
            "case {case:?}: {member}: expected {expected:?} epoch seconds, got {actual:?} in {item}"
        );
    }

    let input_s3 = item
        .get("InputDataConfig")
        .and_then(|config| config.get("S3Uri"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let expected_input_s3 = record
        .input_data_config
        .as_ref()
        .and_then(|config| config.s3_uri.clone());
    assert_eq!(
        input_s3, expected_input_s3,
        "case {case:?}: InputDataConfig.S3Uri: expected {expected_input_s3:?}, got {input_s3:?}"
    );

    let output_s3 = item
        .get("OutputDataConfig")
        .and_then(|config| config.get("S3Uri"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let expected_output_s3 = record
        .output_data_config
        .as_ref()
        .and_then(|config| config.s3_uri.clone());
    assert_eq!(
        output_s3, expected_output_s3,
        "case {case:?}: OutputDataConfig.S3Uri: expected {expected_output_s3:?}, got {output_s3:?}"
    );

    let subnets = item
        .get("VpcConfig")
        .and_then(|config| config.get("Subnets"))
        .and_then(Value::as_array)
        .map(Vec::len);
    let expected_subnets = record
        .vpc_config
        .as_ref()
        .map(|config| config.subnets.len());
    assert_eq!(
        subnets, expected_subnets,
        "case {case:?}: VpcConfig.Subnets: expected {expected_subnets:?} entries, got {subnets:?}"
    );

    let labels = item
        .get("ClassifierMetadata")
        .and_then(|metadata| metadata.get("NumberOfLabels"))
        .and_then(Value::as_i64);
    let expected_labels = record
        .classifier_metadata
        .as_ref()
        .and_then(|metadata| metadata.number_of_labels.map(i64::from));
    assert_eq!(
        labels, expected_labels,
        "case {case:?}: ClassifierMetadata.NumberOfLabels: expected {expected_labels:?}, \
         got {labels:?}"
    );

    // `DocumentClassifierName` is not a member of `DocumentClassifierProperties`:
    // the ARN is what carries the name.
    assert!(
        item.get("DocumentClassifierName").is_none(),
        "case {case:?}: DocumentClassifierProperties models no DocumentClassifierName, \
         got {item}"
    );
}

#[tokio::test]
async fn an_account_with_no_classifiers_lists_nothing() {
    let comprehend = Comprehend::new();

    for request in [
        json!({}),
        json!({ "MaxResults": 1 }),
        json!({ "MaxResults": 500 }),
        json!({ "Filter": { "Status": "TRAINED" } }),
        json!({ "Filter": { "DocumentClassifierName": "anything" } }),
    ] {
        let output = list(&comprehend, &request).await;
        assert!(
            listed_arns(&output).is_empty() && output["NextToken"].is_null(),
            "case {request}: an empty store must list nothing and offer no NextToken, \
             got {output}"
        );
    }
}

#[tokio::test]
async fn every_persisted_classifier_is_reported_from_the_store() {
    let comprehend = Comprehend::new();
    let seeded = seed(&comprehend, base_time());

    let output = list(&comprehend, &json!({ "MaxResults": 500 })).await;
    let items = output["DocumentClassifierPropertiesList"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        items.len(),
        seeded.len(),
        "expected every seeded classifier once, got {output}"
    );
    assert!(
        output["NextToken"].is_null(),
        "a page that holds every record must offer no NextToken, got {output}"
    );

    for (item, record) in items.iter().zip(seeded.iter()) {
        assert_projects(&record.classifier_name, item, record);
        assert!(
            record.classifier_arn.starts_with(&format!(
                "arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:document-classifier/"
            )),
            "case {:?}: the seeded ARN must carry the request's region and this account, got {:?}",
            record.classifier_name,
            record.classifier_arn
        );
    }
}

#[tokio::test]
async fn list_pages_through_every_classifier_in_creation_order() {
    let comprehend = Comprehend::new();
    let seeded = seed(&comprehend, base_time());
    let expected = arns(&seeded);

    // Every page size from one item to one past the whole set, and the modelled
    // `MaxResults` ceiling.
    for page_size in [1usize, 2, 3, expected.len(), expected.len() + 1, 500] {
        let collected = page_through(&comprehend, &json!({}), page_size).await;
        assert_eq!(
            collected, expected,
            "case MaxResults={page_size}: expected every classifier once, oldest to newest"
        );
    }
}

/// `MaxResults` is modelled `@range` 1..500: both bounds are served, and a value
/// past either end is refused before the handler runs.
#[tokio::test]
async fn max_results_outside_the_modelled_range_is_refused() {
    let comprehend = Comprehend::new();
    seed(&comprehend, base_time());

    for max_results in [1, 500] {
        let output = list(&comprehend, &json!({ "MaxResults": max_results })).await;
        assert!(
            !listed_arns(&output).is_empty(),
            "case MaxResults={max_results}: a value inside the modelled range must be served, \
             got {output}"
        );
    }

    for max_results in [0, 501] {
        let (status, body) = comprehend
            .call("ListDocumentClassifiers", &json!({ "MaxResults": max_results }))
            .await;
        assert_eq!(
            status, 400,
            "case MaxResults={max_results}: expected 400 for a value outside the modelled \
             range, got {status}: {body}"
        );
    }
}

#[tokio::test]
async fn a_submit_time_after_page_is_served_newest_first() {
    let comprehend = Comprehend::new();
    let base = base_time();
    let seeded = seed(&comprehend, base);

    let oldest_first = arns(&seeded);
    let mut newest_first = oldest_first.clone();
    newest_first.reverse();

    // A bound one second before the oldest submission selects every record, so the
    // only difference between the two cases is the order the member documents.
    let before_everything = epoch_secs(base) - 1;
    for (case, filter, expected) in [
        (
            "SubmitTimeAfter: descending, newest to oldest",
            json!({ "SubmitTimeAfter": before_everything }),
            &newest_first,
        ),
        (
            "SubmitTimeBefore: ascending, oldest to newest",
            json!({ "SubmitTimeBefore": epoch_secs(base) + 10_000 }),
            &oldest_first,
        ),
    ] {
        for page_size in [1usize, 2, 500] {
            let collected =
                page_through(&comprehend, &json!({ "Filter": filter }), page_size).await;
            assert_eq!(
                &&collected, &expected,
                "case {case} with MaxResults={page_size}: wrong order or membership"
            );
        }
    }
}

/// "Returns only classifiers submitted before/after the specified time": a record
/// submitted exactly AT the bound is on neither side of it.
#[tokio::test]
async fn the_submit_time_bounds_are_strict() {
    let comprehend = Comprehend::new();
    let base = base_time();
    let seeded = seed(&comprehend, base);

    for record in &seeded {
        let bound = epoch_secs(record.submit_time);

        let before = page_through(
            &comprehend,
            &json!({ "Filter": { "SubmitTimeBefore": bound } }),
            500,
        )
        .await;
        assert!(
            !before.contains(&record.classifier_arn),
            "case {:?}: SubmitTimeBefore={bound} must exclude the record submitted at it, \
             got {before:?}",
            record.classifier_name
        );
        let expected_before: Vec<String> = seeded
            .iter()
            .filter(|other| epoch_secs(other.submit_time) < bound)
            .map(|other| other.classifier_arn.clone())
            .collect();
        assert_eq!(
            before, expected_before,
            "case {:?}: SubmitTimeBefore={bound} selected the wrong set",
            record.classifier_name
        );

        let after = page_through(
            &comprehend,
            &json!({ "Filter": { "SubmitTimeAfter": bound } }),
            500,
        )
        .await;
        assert!(
            !after.contains(&record.classifier_arn),
            "case {:?}: SubmitTimeAfter={bound} must exclude the record submitted at it, \
             got {after:?}",
            record.classifier_name
        );
        let mut expected_after: Vec<String> = seeded
            .iter()
            .filter(|other| epoch_secs(other.submit_time) > bound)
            .map(|other| other.classifier_arn.clone())
            .collect();
        expected_after.reverse();
        assert_eq!(
            after, expected_after,
            "case {:?}: SubmitTimeAfter={bound} selected the wrong set or order",
            record.classifier_name
        );
    }
}

/// The `Status` filter, over the WHOLE modelled `ModelStatus` vocabulary — the
/// statuses the table seeds and the ones no record carries alike. The catalog comes
/// from the model: `ops::types::MODEL_STATUS_VALUES`.
#[tokio::test]
async fn the_status_filter_selects_exactly_the_records_carrying_it() {
    let comprehend = Comprehend::new();
    let seeded = seed(&comprehend, base_time());

    for status in MODEL_STATUS_VALUES {
        let expected: Vec<String> = seeded
            .iter()
            .filter(|record| record.status == *status)
            .map(|record| record.classifier_arn.clone())
            .collect();
        let collected = page_through(
            &comprehend,
            &json!({ "Filter": { "Status": status } }),
            500,
        )
        .await;
        assert_eq!(
            collected, expected,
            "case Filter.Status={status:?}: expected exactly the records carrying it"
        );
    }
}

#[tokio::test]
async fn the_name_filter_selects_exactly_the_classifier_carrying_it() {
    let comprehend = Comprehend::new();
    let seeded = seed(&comprehend, base_time());

    for record in &seeded {
        let collected = page_through(
            &comprehend,
            &json!({ "Filter": { "DocumentClassifierName": record.classifier_name } }),
            500,
        )
        .await;
        assert_eq!(
            collected,
            vec![record.classifier_arn.clone()],
            "case Filter.DocumentClassifierName={:?}: expected exactly that classifier",
            record.classifier_name
        );
    }

    // A name no classifier carries selects nothing and offers no token.
    for absent in ["no-such-classifier", "reviews", "minimal2"] {
        let output = list(
            &comprehend,
            &json!({ "Filter": { "DocumentClassifierName": absent } }),
        )
        .await;
        assert!(
            listed_arns(&output).is_empty() && output["NextToken"].is_null(),
            "case Filter.DocumentClassifierName={absent:?}: expected an empty last page, \
             got {output}"
        );
    }
}

/// `DocumentClassifierFilter`: "You can only specify one filtering parameter in a
/// request."
#[tokio::test]
async fn more_than_one_filtering_parameter_is_refused() {
    let comprehend = Comprehend::new();
    let base = base_time();
    seed(&comprehend, base);
    let bound = epoch_secs(base);

    for filter in [
        json!({ "Status": "TRAINED", "DocumentClassifierName": "reviews-classifier" }),
        json!({ "SubmitTimeBefore": bound + 10_000, "SubmitTimeAfter": bound - 1 }),
        json!({ "Status": "TRAINING", "SubmitTimeAfter": bound - 1 }),
        json!({
            "Status": "TRAINED",
            "DocumentClassifierName": "reviews-classifier",
            "SubmitTimeBefore": bound + 10_000,
        }),
    ] {
        let (status, body) = comprehend
            .call("ListDocumentClassifiers", &json!({ "Filter": filter }))
            .await;
        assert_eq!(
            status, 400,
            "case Filter={filter}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidFilterException",
            "case Filter={filter}: expected InvalidFilterException, got {body}"
        );
    }

    // Exactly one filtering parameter is of course served.
    for filter in [
        json!({ "Status": "TRAINED" }),
        json!({ "DocumentClassifierName": "minimal" }),
        json!({ "SubmitTimeBefore": bound + 10_000 }),
        json!({ "SubmitTimeAfter": bound - 1 }),
    ] {
        let (status, body) = comprehend
            .call("ListDocumentClassifiers", &json!({ "Filter": filter }))
            .await;
        assert_eq!(
            status, 200,
            "case Filter={filter}: one filtering parameter must be served, got {status}: {body}"
        );
    }
}

#[tokio::test]
async fn list_refuses_a_token_it_did_not_issue() {
    let comprehend = Comprehend::new();
    let seeded = seed(&comprehend, base_time());

    for token in [
        // Not hex at all, hex of the wrong width, and a well-formed cursor naming
        // a page this service never issued: zero, and one past the newest record.
        "not-a-token",
        "00",
        "0000000000000000",
        &hex::encode((seeded.len() as u64 + 1).to_be_bytes()),
        &hex::encode(u64::MAX.to_be_bytes()),
    ] {
        let (status, body) = comprehend
            .call("ListDocumentClassifiers", &json!({ "NextToken": token }))
            .await;
        assert_eq!(
            status, 400,
            "case NextToken={token:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case NextToken={token:?}: expected InvalidRequestException, got {body}"
        );
    }
}

/// Listing is a report: it answers from the store without changing it, so the same
/// request repeated, and a later full listing, see exactly what the first call saw.
#[tokio::test]
async fn listing_changes_nothing_a_later_caller_sees() {
    let comprehend = Comprehend::new();
    let base = base_time();
    let seeded = seed(&comprehend, base);
    let expected = arns(&seeded);

    let first = list(&comprehend, &json!({ "MaxResults": 500 })).await;
    // Walk a filtered, paged, reversed listing in between, so any cursor or status
    // the handler might have written would show up in the repeat.
    page_through(&comprehend, &json!({}), 1).await;
    page_through(
        &comprehend,
        &json!({ "Filter": { "SubmitTimeAfter": epoch_secs(base) - 10_000 } }),
        2,
    )
    .await;
    let second = list(&comprehend, &json!({ "MaxResults": 500 })).await;

    assert_eq!(
        first, second,
        "two identical listings must agree: a report writes nothing"
    );
    assert_eq!(
        listed_arns(&second),
        expected,
        "every record must still be listed after the reads"
    );
}
