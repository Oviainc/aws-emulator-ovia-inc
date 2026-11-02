// SPDX-License-Identifier: CC0-1.0

//! Comprehend's custom document-classifier family:
//! `CreateDocumentClassifier`, `DescribeDocumentClassifier`,
//! `StopTrainingDocumentClassifier`, `DeleteDocumentClassifier` and
//! `ListDocumentClassifiers`.
//!
//! RULE: a classifier is a resource the create mints an ARN for and persists, the
//! reads report back exactly what was persisted (plus the status the clock already
//! owed it), and an ARN no classifier was created under — or whose classifier a
//! completed deletion removed — is answered with `ResourceNotFoundException`.
//!
//! Each suite asserts that over a SET of creations: the rows differ along which
//! optional members are present, the training data format, whether the creation names
//! a version, and the classifier mode. The issue's single nonexistent ARN is ONE row
//! of the not-found table.
//!
//! The `ListDocumentClassifiers` suites in the second half of this file assert the
//! page rule over a SET of seeded records: the operation reports every persisted
//! classifier projected into `DocumentClassifierProperties` in creation order —
//! reversed when `SubmitTimeAfter` is the filter, as that member's own documentation
//! states — narrowed by the single filtering parameter the request set, truncated at
//! `MaxResults`, with a `NextToken` that resumes exactly after the last item of the
//! page and no token on the last one. More than one filtering parameter is refused
//! with `InvalidFilterException`, and a token this service never issued with
//! `InvalidRequestException`. Those rows differ along everything the projection and
//! the filter generalise over — the `ModelStatus` each record carries, the language,
//! the mode, a named version versus none, which optional members are set at all, and
//! the submission instant.

// `std::thread::sleep`, not `tokio::time::sleep`: the workspace's shared `tokio`
// carries no `time` feature, so there is no timer driver. It is safe here only
// because every call below is awaited sequentially on a current-thread runtime with
// nothing in the background.
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::types::{
    ClassifierEvaluationMetrics, ClassifierMetadata, DocumentClassifierInputDataConfig,
    DocumentClassifierOutputDataConfig, Tag, VpcConfig, MODEL_STATUS_VALUES,
};
use service_com_amazonaws_comprehend_comprehend_20171127::state::{
    document_classifier_arn, service_state, DocumentClassifier,
};

use crate::comprehend_client::{error_code, error_message, Comprehend, ACCOUNT_ID, REGION};

/// The message every resource-not-found refusal of this service carries.
///
/// Amazon Comprehend's own fixed sentence, carrying its reason code and NOTHING
/// derived from the request. Written out here rather than imported from the service,
/// so the suite pins the recorded text instead of whatever the code currently
/// believes.
const RESOURCE_NOT_FOUND_MESSAGE: &str = "RESOURCE_NOT_FOUND: Could not find specified resource.";

/// The sentence the model DELETION refuses an ARN no classifier was created under
/// with: the same kind of fixed text, worded for the class of resource that
/// operation deletes — a classifier is a custom model — so the sentence follows the
/// operation rather than the ARN. `not_found_refusals` is the suite that asserts
/// that over every operation of the three planes that raise a not-found.
const MODEL_NOT_FOUND_MESSAGE: &str = "Could not find specified model.";

/// The order a classifier's `ModelStatus` may only move forward along when nobody
/// stops it.
///
/// A started training ends `IN_ERROR` in THIS suite, whose emulator mounts no object
/// storage: training means reading the labelled documents from the Amazon S3
/// location `InputDataConfig` names and fitting a model on them, and nothing answers
/// those calls here, so the training reports what it could not read.
/// `classifier_training` is the suite that mounts object storage and asserts the
/// TRAINED model and the metrics it measured.
const LIFECYCLE: &[&str] = &["SUBMITTED", "TRAINING", "IN_ERROR"];

/// The status a started training reaches, as `MODEL_STATUS_VALUES` spells it.
const TERMINAL_STATUS: &str = "IN_ERROR";

/// Longest a suite waits for the clock to owe a classifier its next status. The
/// spans in `crate::state` total well under a second; this leaves ample head-room on
/// a loaded machine.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(10);

/// A `CreateDocumentClassifier` body per case, differing along every member the
/// operation persists.
fn creation(case: &str) -> Value {
    match case {
        "every optional member set" => json!({
            "DocumentClassifierName": "every-member-classifier",
            "VersionName": "v1",
            "DataAccessRoleArn": "arn:aws:iam::123456789012:role/ComprehendRole",
            "LanguageCode": "en",
            "Mode": "MULTI_LABEL",
            "InputDataConfig": {
                "DataFormat": "COMPREHEND_CSV",
                "S3Uri": "s3://bucket/training/",
                "TestS3Uri": "s3://bucket/test/",
                "LabelDelimiter": "|",
            },
            "OutputDataConfig": {
                "S3Uri": "s3://bucket/out/",
                "KmsKeyId": "arn:aws:kms:eu-west-1:000000000000:key/abcd-1234",
            },
            "VolumeKmsKeyId": "1234abcd-12ab-34cd-56ef-1234567890ab",
            "ModelKmsKeyId": "arn:aws:kms:eu-west-1:000000000000:key/beef-5678",
            "VpcConfig": {
                "SecurityGroupIds": ["sg-0123456789abcdef0"],
                "Subnets": ["subnet-0123456789abcdef0"],
            },
            "ModelPolicy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
            "Tags": [{ "Key": "team", "Value": "research" }, { "Key": "cost-centre" }],
        }),
        "only the required members" => json!({
            "DocumentClassifierName": "minimal-classifier",
            "DataAccessRoleArn": "arn:aws:iam::210987654321:role/path/to/OtherRole",
            "LanguageCode": "ja",
            "InputDataConfig": { "S3Uri": "s3://other-bucket/corpus/" },
        }),
        _ => json!({
            "DocumentClassifierName": "augmented-manifest-classifier",
            "DataAccessRoleArn": "arn:aws:iam::000000000000:role/Third",
            "LanguageCode": "es",
            "InputDataConfig": {
                "DataFormat": "AUGMENTED_MANIFEST",
                "AugmentedManifests": [{
                    "S3Uri": "s3://third/manifest.jsonl",
                    "AttributeNames": ["label"],
                    "Split": "TRAIN",
                }],
            },
        }),
    }
}

/// The cases [`creation`] builds.
const CREATION_CASES: &[&str] = &[
    "every optional member set",
    "only the required members",
    "an augmented-manifest training set",
];

/// Create a classifier and return its ARN.
async fn create(comprehend: &Comprehend, case: &str, body: &Value) -> String {
    let output = comprehend.ok("CreateDocumentClassifier", body).await;
    output["DocumentClassifierArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a DocumentClassifierArn, got {output}"))
        .to_string()
}

/// The properties `DescribeDocumentClassifier` reports for `arn`.
async fn describe(comprehend: &Comprehend, arn: &str) -> Value {
    let output = comprehend
        .ok(
            "DescribeDocumentClassifier",
            &json!({ "DocumentClassifierArn": arn }),
        )
        .await;
    output["DocumentClassifierProperties"].clone()
}

#[tokio::test]
async fn a_created_classifier_is_persisted_and_described_from_the_store() {
    let comprehend = Comprehend::new();

    for case in CREATION_CASES {
        let body = creation(case);
        let arn = create(&comprehend, case, &body).await;

        // The ARN form the output member's own pattern states, with the region the
        // request was signed for: `document-classifier/<name>`, plus
        // `/version/<VersionName>` when the creation named a version.
        let name = body["DocumentClassifierName"].as_str().unwrap_or_default();
        let expected = match body["VersionName"].as_str() {
            Some(version) => format!(
                "arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:document-classifier/{name}/version/\
                 {version}"
            ),
            None => format!("arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:document-classifier/{name}"),
        };
        assert_eq!(arn, expected, "case {case:?}: unexpected classifier ARN");

        let properties = describe(&comprehend, &arn).await;
        assert_eq!(
            properties["DocumentClassifierArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: Describe reported a different ARN: {properties}"
        );

        // Every member the creation was called with comes back unchanged.
        for member in [
            "LanguageCode",
            "InputDataConfig",
            "OutputDataConfig",
            "DataAccessRoleArn",
            "VolumeKmsKeyId",
            "ModelKmsKeyId",
            "VpcConfig",
            "VersionName",
        ] {
            let submitted = body.get(member).cloned().unwrap_or(Value::Null);
            let reported = properties.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                reported, submitted,
                "case {case:?}: Describe must report the {member} Create was called with, \
                 got {reported} for {submitted}"
            );
        }

        // `Mode` is optional on the request and defaults to MULTI_CLASS, which the
        // service documents; the reported value is always the one trained.
        let expected_mode = body["Mode"].as_str().unwrap_or("MULTI_CLASS");
        assert_eq!(
            properties["Mode"].as_str(),
            Some(expected_mode),
            "case {case:?}: expected Mode {expected_mode:?}, got {properties}"
        );

        assert!(
            MODEL_STATUS_VALUES.contains(&properties["Status"].as_str().unwrap_or_default()),
            "case {case:?}: unexpected Status in {properties}"
        );
        assert!(
            properties["SubmitTime"].as_f64().is_some_and(|t| t > 0.0),
            "case {case:?}: expected a populated SubmitTime, got {properties}"
        );
        // Nothing was trained, so the metrics of a fitted model are absent rather
        // than invented, and `Tags` is not a member of the properties shape.
        assert!(
            properties.get("ClassifierMetadata").is_none(),
            "case {case:?}: no classifier was trained, so no ClassifierMetadata may be \
             reported, got {properties}"
        );
        assert!(
            properties.get("Tags").is_none(),
            "case {case:?}: DocumentClassifierProperties models no Tags member, got \
             {properties}"
        );
    }
}

#[tokio::test]
async fn the_reads_refuse_an_arn_no_classifier_was_created_under() {
    let comprehend = Comprehend::new();
    // One real classifier, so the refusal is about the ARN and not about an empty
    // store.
    let live = create(
        &comprehend,
        "seed",
        &creation("only the required members"),
    )
    .await;

    for (operation, expected) in [
        ("DescribeDocumentClassifier", RESOURCE_NOT_FOUND_MESSAGE),
        ("StopTrainingDocumentClassifier", RESOURCE_NOT_FOUND_MESSAGE),
        ("DeleteDocumentClassifier", MODEL_NOT_FOUND_MESSAGE),
    ] {
        // Rows that differ along what is wrong with the ARN: an unknown name, a
        // version of a classifier that has none, another region, another account.
        for arn in [
            format!("arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:document-classifier/does-not-exist"),
            format!("{live}/version/v9"),
            format!(
                "arn:aws:comprehend:us-east-2:{ACCOUNT_ID}:document-classifier/minimal-classifier"
            ),
            format!("arn:aws:comprehend:{REGION}:111122223333:document-classifier/minimal-classifier"),
        ] {
            let (status, body) = comprehend
                .call(operation, &json!({ "DocumentClassifierArn": arn }))
                .await;
            assert_eq!(
                status, 400,
                "case {operation} with {arn:?}: expected 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {operation} with {arn:?}: expected ResourceNotFoundException, got {body}"
            );
            // The operation's own fixed text for a resource that is not there: it
            // carries NOTHING derived from the request, so the ARN the caller asked
            // for is never echoed back.
            assert_eq!(
                error_message(&body),
                expected,
                "case {operation} with {arn:?}: expected {expected:?}, got {body}"
            );
            assert!(
                !error_message(&body).contains(&arn),
                "case {operation} with {arn:?}: the refusal must not echo the requested ARN, \
                 got {body}"
            );
        }
    }
}

// --- `ListDocumentClassifiers`: the page it serves over the same records ---------
//
// The suites below seed classifier records through the crate's own state API — the
// SAME store, reached the same way, that the handler reads — rather than through
// `CreateDocumentClassifier`, because a seeded record can carry a `ModelStatus`,
// a submission instant and a metrics block of its own, without standing up the
// object storage a real training run reads its documents from.

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

/// Hold a seeded record at the non-terminal status it stands for.
///
/// `DocumentClassifier::submitted` derives the instants the clock owes the next
/// status change at from `SubmitTime`, which these rows deliberately set in the past
/// so the submit-time filters have something to select on — and a transition already
/// due is one every read materialises. A record seeded to STAND for a classifier
/// still `SUBMITTED` or still `TRAINING` therefore has its due instants moved clear
/// of now, which is what a classifier submitted a moment ago would carry.
fn hold_status(record: &mut DocumentClassifier) {
    record.training_due_at = SystemTime::now() + Duration::from_secs(3_600);
    record.terminal_due_at = record.training_due_at + Duration::from_secs(3_600);
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
    let mut submitted = DocumentClassifier::submitted(
        0,
        document_classifier_arn(REGION, "minimal", None),
        "minimal".to_string(),
        "ja".to_string(),
        base + SUBMISSION_GAP,
    );
    hold_status(&mut submitted);

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
    hold_status(&mut training);
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
async fn one_name_and_version_names_one_classifier() {
    let comprehend = Comprehend::new();

    // The same name twice is refused; the same name under a different version is a
    // different classifier, as the `VersionName` documentation says ("must be unique
    // among all models with the same classifier name").
    let mut body = creation("only the required members");
    create(&comprehend, "first creation", &body).await;

    let (status, response) = comprehend.call("CreateDocumentClassifier", &body).await;
    assert_eq!(
        status, 400,
        "case \"same name again\": expected 400, got {status}: {response}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceInUseException",
        "case \"same name again\": expected ResourceInUseException, got {response}"
    );

    for version in ["v1", "v2"] {
        body["VersionName"] = json!(version);
        let arn = create(&comprehend, version, &body).await;
        assert!(
            arn.ends_with(&format!("/version/{version}")),
            "case {version:?}: expected a versioned ARN, got {arn:?}"
        );
        let (status, response) = comprehend.call("CreateDocumentClassifier", &body).await;
        assert_eq!(
            status, 400,
            "case {version:?} again: expected 400, got {status}: {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceInUseException",
            "case {version:?} again: expected ResourceInUseException, got {response}"
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
async fn repeating_a_client_request_token_returns_the_first_classifier() {
    let comprehend = Comprehend::new();

    for case in CREATION_CASES {
        let mut body = creation(case);
        body["ClientRequestToken"] = json!(format!("tok-{}", case.replace(' ', "-")));

        let first = create(&comprehend, case, &body).await;
        let repeated = comprehend.ok("CreateDocumentClassifier", &body).await;
        assert_eq!(
            repeated["DocumentClassifierArn"].as_str(),
            Some(first.as_str()),
            "case {case:?}: a repeated ClientRequestToken must return the first ARN, got \
             {repeated}"
        );
    }
}

/// The classifier idempotency index is a VIEW of the persisted records, not a store
/// that grows on its own: it can never hold more entries than there are classifiers
/// for it to point at, whatever sequence of creations a caller makes.
///
/// The rows are the sequences that put pressure on that invariant, each a different
/// way a creation can land on an ARN that already names a record: the same name and
/// version submitted again under a FRESH token every time (which the service refuses
/// as `ResourceInUseException`, so no record and no index entry may appear), the same
/// name under ONE token (a replay, which returns the first ARN and creates nothing),
/// and a fresh name each time (one record per request).
#[tokio::test]
async fn the_idempotency_index_never_outgrows_the_classifiers_it_points_at() {
    for (case, names, repeats, fresh_token) in [
        ("one classifier, a fresh token each time", 1usize, 4usize, true),
        ("one classifier, one token repeated", 1, 4, false),
        ("a fresh classifier each time", 4, 1, true),
        ("three classifiers, fresh tokens", 3, 3, true),
    ] {
        let comprehend = Comprehend::new();
        let mut request = 0;
        for name in 0..names {
            for repeat in 0..repeats {
                request += 1;
                let mut body = creation("only the required members");
                body["DocumentClassifierName"] = json!(format!("index-bound-{name}"));
                body["ClientRequestToken"] = json!(if fresh_token {
                    format!("tok-{request}")
                } else {
                    format!("tok-{name}")
                });
                let (status, response) =
                    comprehend.call("CreateDocumentClassifier", &body).await;

                // A repeat under a NEW token addresses an ARN that already names a
                // record, which the service refuses; under the SAME token it is a
                // replay of the first creation.
                let expected_status = if repeat > 0 && fresh_token { 400 } else { 200 };
                assert_eq!(
                    status, expected_status,
                    "case {case:?} request {request}: expected {expected_status}, got \
                     {status}: {response}"
                );
                if status == 400 {
                    assert_eq!(
                        error_code(&response),
                        "ResourceInUseException",
                        "case {case:?} request {request}: expected ResourceInUseException, \
                         got {response}"
                    );
                }

                let state = service_state(&**comprehend.engine())
                    .expect("the comprehend store is reachable from the shared engine");
                let (records, indexed) = {
                    let store = state.lock();
                    (
                        store.document_classifiers.len(),
                        store.classifier_idempotency_tokens.len(),
                    )
                };
                assert!(
                    indexed <= records,
                    "case {case:?} after {request} request(s): the idempotency index must \
                     not outgrow the records it points at, got {indexed} entries for \
                     {records} classifier(s)"
                );
                assert_eq!(
                    records,
                    name + 1,
                    "case {case:?} after {request} request(s): expected {} persisted \
                     classifier(s), got {records}",
                    name + 1
                );
            }
        }
    }
}

#[tokio::test]
async fn a_creation_that_points_the_training_at_nothing_is_refused() {
    let comprehend = Comprehend::new();

    // The data the model documents as required for each `DataFormat`:
    // `COMPREHEND_CSV` (the default when none is named) needs `S3Uri`, and
    // `AUGMENTED_MANIFEST` needs `AugmentedManifests`.
    let rows: &[(&str, Value, bool)] = &[
        ("no DataFormat and no S3Uri", json!({}), false),
        (
            "COMPREHEND_CSV without S3Uri",
            json!({ "DataFormat": "COMPREHEND_CSV", "TestS3Uri": "s3://bucket/test/" }),
            false,
        ),
        (
            "AUGMENTED_MANIFEST without AugmentedManifests",
            json!({ "DataFormat": "AUGMENTED_MANIFEST", "S3Uri": "s3://bucket/training/" }),
            false,
        ),
        (
            "AUGMENTED_MANIFEST with an empty AugmentedManifests",
            json!({ "DataFormat": "AUGMENTED_MANIFEST", "AugmentedManifests": [] }),
            false,
        ),
        (
            "the default DataFormat with an S3Uri",
            json!({ "S3Uri": "s3://bucket/training/" }),
            true,
        ),
        (
            "COMPREHEND_CSV with an S3Uri",
            json!({ "DataFormat": "COMPREHEND_CSV", "S3Uri": "s3://bucket/training/" }),
            true,
        ),
        (
            "AUGMENTED_MANIFEST with a manifest",
            json!({
                "DataFormat": "AUGMENTED_MANIFEST",
                "AugmentedManifests": [{
                    "S3Uri": "s3://bucket/manifest.jsonl",
                    "AttributeNames": ["label"],
                }],
            }),
            true,
        ),
        (
            "a native-document training set",
            json!({
                "DocumentType": "SEMI_STRUCTURED_DOCUMENT",
                "Documents": { "S3Uri": "s3://bucket/natives/" },
            }),
            true,
        ),
    ];

    for (index, (case, input_data_config, accepted)) in rows.iter().enumerate() {
        let body = json!({
            "DocumentClassifierName": format!("input-config-{index}"),
            "DataAccessRoleArn": "arn:aws:iam::123456789012:role/ComprehendRole",
            "LanguageCode": "en",
            "InputDataConfig": input_data_config,
        });
        let (status, response) = comprehend.call("CreateDocumentClassifier", &body).await;

        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status}: {response}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status}: {response}"
            );
            assert_eq!(
                error_code(&response),
                "InvalidRequestException",
                "case {case:?}: expected InvalidRequestException, got {response}"
            );
            // A refused creation mints nothing: the name is still free, which a
            // creation that DOES carry training data proves by succeeding under it.
            let mut retry = body.clone();
            retry["InputDataConfig"] = json!({ "S3Uri": "s3://bucket/training/" });
            let (retry_status, retry_response) =
                comprehend.call("CreateDocumentClassifier", &retry).await;
            assert_eq!(
                retry_status, 200,
                "case {case:?}: a refused creation must leave the name free, got \
                 {retry_status}: {retry_response}"
            );
        }
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

#[tokio::test]
async fn a_creation_carrying_more_tags_than_a_resource_can_hold_is_refused() {
    let comprehend = Comprehend::new();

    // The bound the `TooManyTagsException` documentation states ("50 tags per
    // resource"): at it, and one past it.
    for count in [0usize, 1, 50, 51] {
        let mut body = creation("only the required members");
        body["DocumentClassifierName"] = json!(format!("tagged-{count}"));
        body["Tags"] = Value::Array(
            (0..count)
                .map(|index| json!({ "Key": format!("k{index}"), "Value": format!("v{index}") }))
                .collect(),
        );
        let (status, response) = comprehend.call("CreateDocumentClassifier", &body).await;

        if count > 50 {
            assert_eq!(
                status, 400,
                "case {count} tags: expected 400, got {status}: {response}"
            );
            assert_eq!(
                error_code(&response),
                "TooManyTagsException",
                "case {count} tags: expected TooManyTagsException, got {response}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {count} tags: expected 200, got {status}: {response}"
            );
        }
    }
}

#[tokio::test]
async fn the_status_only_moves_forward_and_ends_in_the_modelled_failure() {
    let comprehend = Comprehend::new();
    let arn = create(
        &comprehend,
        "lifecycle",
        &creation("only the required members"),
    )
    .await;

    // The first read, taken straight after the call, still sees the transient state.
    let first = describe(&comprehend, &arn).await;
    assert_eq!(
        first["Status"].as_str(),
        Some("SUBMITTED"),
        "case \"first read\": expected SUBMITTED, got {first}"
    );
    assert!(
        first["EndTime"].is_null() && first["TrainingStartTime"].is_null(),
        "case \"first read\": a classifier that has not started training has neither a \
         TrainingStartTime nor an EndTime, got {first}"
    );

    let mut seen: Vec<String> = Vec::new();
    let mut highest = 0usize;
    let started = Instant::now();
    loop {
        let properties = describe(&comprehend, &arn).await;
        let status = properties["Status"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let rank = LIFECYCLE
            .iter()
            .position(|known| *known == status)
            .unwrap_or_else(|| panic!("case \"lifecycle\": unexpected status {status:?}"));
        assert!(
            rank >= highest,
            "case \"lifecycle\": status went backwards to {status:?} after {seen:?}"
        );
        highest = rank;
        if seen.last().map(String::as_str) != Some(status.as_str()) {
            seen.push(status.clone());
        }
        if status != "SUBMITTED" {
            assert!(
                properties["TrainingStartTime"]
                    .as_f64()
                    .is_some_and(|t| t > 0.0),
                "case \"lifecycle\": a classifier that started training carries a \
                 TrainingStartTime, got {properties}"
            );
        }
        if status == TERMINAL_STATUS {
            for member in ["EndTime", "TrainingEndTime"] {
                assert!(
                    properties[member].as_f64().is_some_and(|t| t > 0.0),
                    "case \"lifecycle\": a terminal classifier carries a {member}, got \
                     {properties}"
                );
            }
            // The reason the training could not be carried out, naming the location
            // it was pointed at. No object storage answers this suite's emulator, so
            // the training's own read is what failed.
            let message = properties["Message"].as_str().unwrap_or_default();
            for expected in [
                "s3://other-bucket/corpus/",
                "Could not read the training documents",
            ] {
                assert!(
                    message.contains(expected),
                    "case \"lifecycle\": expected the failure Message to mention \
                     {expected:?}, got {message:?}"
                );
            }
            break;
        }
        assert!(
            properties["Message"].is_null(),
            "case \"lifecycle\": a classifier whose training has not ended carries no \
             Message, got {properties}"
        );
        assert!(
            started.elapsed() < SETTLE_TIMEOUT,
            "case \"lifecycle\": the classifier never reached {TERMINAL_STATUS}; statuses \
             seen: {seen:?}"
        );
        sleep(Duration::from_millis(40));
    }

    assert_eq!(
        seen.first().map(String::as_str),
        Some("SUBMITTED"),
        "case \"lifecycle\": expected the first status to be SUBMITTED, saw {seen:?}"
    );
    assert_eq!(
        seen.last().map(String::as_str),
        Some(TERMINAL_STATUS),
        "case \"lifecycle\": expected the last status to be {TERMINAL_STATUS}, saw {seen:?}"
    );

    // A read never moves a settled classifier on: two reads agree.
    let once = describe(&comprehend, &arn).await;
    let twice = describe(&comprehend, &arn).await;
    assert_eq!(
        once, twice,
        "case \"terminal reads\": reading a settled classifier changed it: {once} then \
         {twice}"
    );
}

#[tokio::test]
async fn stopping_training_ends_the_classifier_in_stopped() {
    let comprehend = Comprehend::new();
    let arn = create(&comprehend, "stop", &creation("only the required members")).await;

    // An empty 200 is what the operation documents for the stop itself.
    let stopped = comprehend
        .ok(
            "StopTrainingDocumentClassifier",
            &json!({ "DocumentClassifierArn": arn }),
        )
        .await;
    assert!(
        stopped.is_null() || stopped == json!({}),
        "case \"stop\": the operation answers 200 with an empty body, got {stopped}"
    );

    let started = Instant::now();
    loop {
        let properties = describe(&comprehend, &arn).await;
        let status = properties["Status"].as_str().unwrap_or_default();
        assert!(
            matches!(status, "STOP_REQUESTED" | "STOPPED"),
            "case \"stop\": a stopped classifier never trains on, got {properties}"
        );
        if status == "STOPPED" {
            assert!(
                properties["Message"]
                    .as_str()
                    .is_some_and(|message| message.contains("request of the caller")),
                "case \"stop\": expected the Message to name the caller's stop, got \
                 {properties}"
            );
            break;
        }
        assert!(
            started.elapsed() < SETTLE_TIMEOUT,
            "case \"stop\": the classifier never reached STOPPED"
        );
        sleep(Duration::from_millis(40));
    }

    // Stopping twice is not an error, and does not move a stopped classifier.
    let before = describe(&comprehend, &arn).await;
    comprehend
        .ok(
            "StopTrainingDocumentClassifier",
            &json!({ "DocumentClassifierArn": arn }),
        )
        .await;
    let after = describe(&comprehend, &arn).await;
    assert_eq!(
        before, after,
        "case \"stop again\": a second stop changed a stopped classifier: {before} then \
         {after}"
    );
}

#[tokio::test]
async fn a_classifier_is_deleted_only_once_its_training_has_ended() {
    let comprehend = Comprehend::new();
    let arn = create(
        &comprehend,
        "delete",
        &creation("only the required members"),
    )
    .await;

    // "Only those classifiers that are in terminated states will be deleted": a
    // classifier whose training is still running is in use.
    let (status, body) = comprehend
        .call(
            "DeleteDocumentClassifier",
            &json!({ "DocumentClassifierArn": arn }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"delete while training\": expected 400, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceInUseException",
        "case \"delete while training\": expected ResourceInUseException, got {body}"
    );

    // Once the training has ended, the delete is accepted, the classifier reports
    // DELETING, and the record is gone once the clock owes its removal.
    let started = Instant::now();
    loop {
        let properties = describe(&comprehend, &arn).await;
        if properties["Status"].as_str() == Some(TERMINAL_STATUS) {
            break;
        }
        assert!(
            started.elapsed() < SETTLE_TIMEOUT,
            "case \"delete\": the classifier never reached {TERMINAL_STATUS}, got \
             {properties}"
        );
        sleep(Duration::from_millis(40));
    }
    let deleted = comprehend
        .ok(
            "DeleteDocumentClassifier",
            &json!({ "DocumentClassifierArn": arn }),
        )
        .await;
    assert!(
        deleted.is_null() || deleted == json!({}),
        "case \"delete\": the operation answers 200 with an empty body, got {deleted}"
    );

    let started = Instant::now();
    loop {
        let (status, body) = comprehend
            .call(
                "DescribeDocumentClassifier",
                &json!({ "DocumentClassifierArn": arn }),
            )
            .await;
        if status == 400 {
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case \"delete then read\": expected ResourceNotFoundException, got {body}"
            );
            break;
        }
        assert_eq!(
            status, 200,
            "case \"delete then read\": expected 200 or 400, got {status}: {body}"
        );
        assert_eq!(
            body["DocumentClassifierProperties"]["Status"].as_str(),
            Some("DELETING"),
            "case \"delete then read\": a classifier being deleted reports DELETING, got \
             {body}"
        );
        assert!(
            started.elapsed() < SETTLE_TIMEOUT,
            "case \"delete then read\": the classifier was never removed"
        );
        sleep(Duration::from_millis(40));
    }

    // The name is free again once the record is gone.
    let recreated = create(
        &comprehend,
        "recreate after delete",
        &creation("only the required members"),
    )
    .await;
    assert_eq!(
        recreated, arn,
        "case \"recreate after delete\": the freed name must mint the same ARN again"
    );
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
