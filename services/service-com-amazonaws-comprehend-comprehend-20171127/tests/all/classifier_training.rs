// SPDX-License-Identifier: CC0-1.0

//! `CreateDocumentClassifier`'s training run, over object storage mounted beside
//! Comprehend on the same engine.
//!
//! RULE: a creation's training run READS the labelled documents from the Amazon S3
//! location `InputDataConfig` names, FITS a model on them and ends the classifier
//! `TRAINED` — having walked `SUBMITTED` -> `TRAINING` -> `TRAINED` — with
//! `ClassifierMetadata` counted from the documents it actually read; a training the
//! emulator cannot carry out ends `IN_ERROR` with the reason instead.
//!
//! Every suite is a TABLE whose rows differ along what the training data holds: how
//! many labels and documents, which `DocumentClassifierMode`, which label delimiter,
//! whether the rows are quoted, whether some rows are unusable, and whether the
//! creation names its own test data. `document_classifiers.rs` covers the record and
//! the lifecycle without object storage.

// `std::thread::sleep`, not `tokio::time::sleep`: the workspace's shared `tokio`
// carries no `time` feature, so there is no timer driver. It is safe here only
// because every call below is awaited sequentially on a current-thread runtime with
// nothing in the background.
use std::collections::BTreeSet;
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_comprehend_comprehend_20171127::state::{
    service_state, TrainingBudget, MAX_CLASSIFIER_FIT_OPERATIONS,
};

use crate::comprehend_client::Comprehend;

/// How long the budget a suite classifies through may run for. The service's own
/// training budget is its business; this is only so a test that asks the model for a
/// prediction cannot hang.
const TRAINING_BUDGET_WAIT: Duration = Duration::from_secs(10);

/// The bucket every row's training data lives in.
const BUCKET: &str = "classifier-training-bucket";

/// Longest a suite waits for the clock to owe a classifier its next status.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(20);

/// The share of the training documents held back to test the model, as a divisor:
/// every `TRAINING_TEST_SPLIT`-th document is a test document.
///
/// From Amazon Comprehend's own documentation of a creation that names no
/// `TestS3Uri`: "Amazon Comprehend uses 10 percent of your training documents to test
/// the custom classifier." Written out here rather than imported from the service, so
/// the suite pins the documented share.
const TRAINING_TEST_SPLIT: usize = 10;

/// One row of the training table: the data it writes and what the run owes it.
struct Corpus {
    case: &'static str,
    /// The `DocumentClassifierMode` the classifier is created in, or `None` to leave
    /// the member out (which trains the documented `MULTI_CLASS` default).
    mode: Option<&'static str>,
    /// The `LabelDelimiter` the creation names, if any.
    delimiter: Option<&'static str>,
    /// The `COMPREHEND_CSV` body written at the input location.
    csv: &'static str,
    /// The labels every usable row carries, in row order — what the suite counts the
    /// expected metadata from, rather than a hard-coded total.
    rows: &'static [(&'static str, &'static str)],
    /// How many rows of `csv` hold no usable label/document pair.
    skipped: usize,
}

/// Every label of `rows`, counted once.
fn labels_of(corpus: &Corpus) -> BTreeSet<String> {
    let delimiter = corpus.delimiter.unwrap_or("|");
    let multi_label = corpus.mode == Some("MULTI_LABEL");
    corpus
        .rows
        .iter()
        .flat_map(|(labels, _)| {
            if multi_label {
                labels.split(delimiter).map(str::to_string).collect()
            } else {
                vec![labels.to_string()]
            }
        })
        .collect()
}

/// The training corpora every suite below writes.
///
/// Each label's documents share their own vocabulary, so a model really fitted on
/// them classifies them back — which is what the metrics are measured against.
const CORPORA: &[Corpus] = &[
    Corpus {
        case: "two labels, fewer documents than a test split",
        mode: None,
        delimiter: None,
        csv: "BILLING,invoice payment refund due\n\
              BILLING,refund invoice payment pending\n\
              SUPPORT,laptop screen broken repair\n\
              SUPPORT,screen repair laptop cracked\n",
        rows: &[
            ("BILLING", "invoice payment refund due"),
            ("BILLING", "refund invoice payment pending"),
            ("SUPPORT", "laptop screen broken repair"),
            ("SUPPORT", "screen repair laptop cracked"),
        ],
        skipped: 0,
    },
    Corpus {
        case: "three labels and enough documents for a held-out test share",
        mode: Some("MULTI_CLASS"),
        delimiter: None,
        csv: "BILLING,invoice payment refund\n\
              BILLING,refund invoice payment\n\
              BILLING,payment invoice refund\n\
              BILLING,invoice refund payment\n\
              SUPPORT,laptop screen repair\n\
              SUPPORT,screen repair laptop\n\
              SUPPORT,repair laptop screen\n\
              SUPPORT,laptop repair screen\n\
              SALES,quote discount contract\n\
              SALES,discount contract quote\n\
              SALES,contract quote discount\n\
              SALES,quote contract discount\n",
        rows: &[
            ("BILLING", "invoice payment refund"),
            ("BILLING", "refund invoice payment"),
            ("BILLING", "payment invoice refund"),
            ("BILLING", "invoice refund payment"),
            ("SUPPORT", "laptop screen repair"),
            ("SUPPORT", "screen repair laptop"),
            ("SUPPORT", "repair laptop screen"),
            ("SUPPORT", "laptop repair screen"),
            ("SALES", "quote discount contract"),
            ("SALES", "discount contract quote"),
            ("SALES", "contract quote discount"),
            ("SALES", "quote contract discount"),
        ],
        skipped: 0,
    },
    Corpus {
        case: "multi-label rows under the default delimiter",
        mode: Some("MULTI_LABEL"),
        delimiter: None,
        csv: "BILLING|URGENT,invoice payment overdue now\n\
              BILLING,invoice payment overdue later\n\
              SUPPORT|URGENT,laptop screen broken now\n\
              SUPPORT,laptop screen broken later\n",
        rows: &[
            ("BILLING|URGENT", "invoice payment overdue now"),
            ("BILLING", "invoice payment overdue later"),
            ("SUPPORT|URGENT", "laptop screen broken now"),
            ("SUPPORT", "laptop screen broken later"),
        ],
        skipped: 0,
    },
    Corpus {
        case: "multi-label rows under a delimiter the request names",
        mode: Some("MULTI_LABEL"),
        delimiter: Some(";"),
        csv: "BILLING;URGENT,invoice payment overdue now\n\
              BILLING,invoice payment overdue later\n\
              SUPPORT;URGENT,laptop screen broken now\n\
              SUPPORT,laptop screen broken later\n",
        rows: &[
            ("BILLING;URGENT", "invoice payment overdue now"),
            ("BILLING", "invoice payment overdue later"),
            ("SUPPORT;URGENT", "laptop screen broken now"),
            ("SUPPORT", "laptop screen broken later"),
        ],
        skipped: 0,
    },
    Corpus {
        // RFC 4180 quoting: a document holding the separator, and rows the training
        // cannot use (no label, no document, blank).
        case: "quoted documents beside rows that cannot be used",
        mode: None,
        delimiter: None,
        csv: "BILLING,\"invoice, payment and refund\"\n\
              BILLING,\"refund, invoice and payment\"\n\
              ,orphaned document with no label\n\
              SUPPORT,\n\
              SUPPORT,\"laptop, screen and repair\"\n\
              SUPPORT,\"screen, repair and laptop\"\n",
        rows: &[
            ("BILLING", "invoice, payment and refund"),
            ("BILLING", "refund, invoice and payment"),
            ("SUPPORT", "laptop, screen and repair"),
            ("SUPPORT", "screen, repair and laptop"),
        ],
        skipped: 2,
    },
];

/// Create `BUCKET`, so the training's reads have somewhere to go.
async fn create_bucket(comprehend: &Comprehend) {
    let (status, body) = comprehend.s3("PUT", &format!("/{BUCKET}"), b"").await;
    assert_eq!(
        status,
        200,
        "CreateBucket {BUCKET}: expected 200, got {status}: {:?}",
        String::from_utf8_lossy(&body)
    );
}

/// Write one object.
async fn put_object(comprehend: &Comprehend, key: &str, body: &[u8]) {
    let (status, response) = comprehend
        .s3("PUT", &format!("/{BUCKET}/{key}"), body)
        .await;
    assert_eq!(
        status,
        200,
        "PutObject {key}: expected 200, got {status}: {:?}",
        String::from_utf8_lossy(&response)
    );
}

/// Create a classifier over the training data at `s3_uri` and return its ARN.
async fn create(
    comprehend: &Comprehend,
    name: &str,
    corpus: &Corpus,
    s3_uri: &str,
    test_s3_uri: Option<&str>,
) -> String {
    let mut input = json!({ "S3Uri": s3_uri });
    if let Some(delimiter) = corpus.delimiter {
        input["LabelDelimiter"] = json!(delimiter);
    }
    if let Some(test_s3_uri) = test_s3_uri {
        input["TestS3Uri"] = json!(test_s3_uri);
    }
    let mut body = json!({
        "DocumentClassifierName": name,
        "DataAccessRoleArn": "arn:aws:iam::123456789012:role/ComprehendRole",
        "LanguageCode": "en",
        "InputDataConfig": input,
    });
    if let Some(mode) = corpus.mode {
        body["Mode"] = json!(mode);
    }
    let output = comprehend.ok("CreateDocumentClassifier", &body).await;
    output["DocumentClassifierArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {:?}: expected an ARN, got {output}", corpus.case))
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

/// Poll `arn` until its training has ended, returning every status seen in order and
/// the properties it ended with.
async fn settled(comprehend: &Comprehend, case: &str, arn: &str) -> (Vec<String>, Value) {
    let started = Instant::now();
    let mut seen: Vec<String> = Vec::new();
    loop {
        let properties = describe(comprehend, arn).await;
        let status = properties["Status"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if seen.last() != Some(&status) {
            seen.push(status.clone());
        }
        if matches!(
            status.as_str(),
            "TRAINED" | "TRAINED_WITH_WARNING" | "IN_ERROR" | "STOPPED"
        ) {
            return (seen, properties);
        }
        assert!(
            started.elapsed() < SETTLE_TIMEOUT,
            "case {case:?}: the training never ended; statuses seen: {seen:?}"
        );
        sleep(Duration::from_millis(20));
    }
}

#[tokio::test]
async fn a_training_run_fits_a_model_on_the_documents_it_read() {
    for corpus in CORPORA {
        let case = corpus.case;
        let comprehend = Comprehend::with_object_storage();
        create_bucket(&comprehend).await;
        let key = "train.csv";
        put_object(&comprehend, key, corpus.csv.as_bytes()).await;
        let arn = create(
            &comprehend,
            "fitted",
            corpus,
            &format!("s3://{BUCKET}/{key}"),
            None,
        )
        .await;

        let (seen, properties) = settled(&comprehend, case, &arn).await;
        // The documented lifecycle, and the rule the issue reported broken: the
        // classifier passes through TRAINING and ends TRAINED. A run with something
        // to report about its data ends `TRAINED_WITH_WARNING` instead: rows it
        // could not use, or a corpus too small to hold a test share back.
        let held_back = corpus.rows.len() / TRAINING_TEST_SPLIT;
        let expected_terminal = if corpus.skipped == 0 && held_back > 0 {
            "TRAINED"
        } else {
            "TRAINED_WITH_WARNING"
        };
        assert_eq!(
            seen.last().map(String::as_str),
            Some(expected_terminal),
            "case {case:?}: expected the training to end {expected_terminal}, saw {seen:?}: \
             {properties}"
        );
        assert!(
            seen.contains(&"TRAINING".to_string()),
            "case {case:?}: a training classifier must be observable as TRAINING, saw {seen:?}"
        );
        assert_eq!(
            seen.first().map(String::as_str),
            Some("SUBMITTED"),
            "case {case:?}: expected the first status to be SUBMITTED, saw {seen:?}"
        );
        for member in ["TrainingStartTime", "TrainingEndTime", "EndTime"] {
            assert!(
                properties[member].as_f64().is_some_and(|at| at > 0.0),
                "case {case:?}: a trained classifier carries a {member}, got {properties}"
            );
        }
        // A run with something to report about its data says so; one with nothing to
        // report carries no Message.
        if expected_terminal == "TRAINED" {
            assert!(
                properties["Message"].is_null(),
                "case {case:?}: a clean training reports no Message, got {properties}"
            );
        } else {
            let message = properties["Message"].as_str().unwrap_or_default();
            if corpus.skipped > 0 {
                assert!(
                    message.contains(&corpus.skipped.to_string()),
                    "case {case:?}: expected the Message to name the {} skipped row(s), got \
                     {message:?}",
                    corpus.skipped
                );
            }
            if held_back == 0 {
                // The metrics below were measured on the training documents, which a
                // caller reading NumberOfTestDocuments: 0 has to be told.
                assert!(
                    message.contains("measured on the training documents"),
                    "case {case:?}: expected the Message to say where the metrics were \
                     measured, got {message:?}"
                );
            }
        }

        // The metrics are COUNTED from the documents the run read: the labels the
        // data holds, and every usable row either trained or tested.
        let labels = labels_of(corpus);
        let metadata = &properties["ClassifierMetadata"];
        assert_eq!(
            metadata["NumberOfLabels"].as_u64(),
            Some(labels.len() as u64),
            "case {case:?}: expected the {} label(s) the data holds, got {metadata}",
            labels.len()
        );
        let trained = metadata["NumberOfTrainedDocuments"].as_u64().unwrap_or(0);
        let tested = metadata["NumberOfTestDocuments"].as_u64().unwrap_or(0);
        assert_eq!(
            trained + tested,
            corpus.rows.len() as u64,
            "case {case:?}: expected the {} usable row(s) to be trained or tested, got \
             {metadata}",
            corpus.rows.len()
        );
        // Ten percent of the training documents are held back to test the model,
        // and a corpus too small for that holds none back.
        let expected_tested = held_back as u64;
        assert_eq!(
            tested, expected_tested,
            "case {case:?}: expected {expected_tested} test document(s) out of {}, got \
             {metadata}",
            corpus.rows.len()
        );
        let metrics = &metadata["EvaluationMetrics"];
        for member in [
            "Accuracy",
            "Precision",
            "Recall",
            "F1Score",
            "MicroPrecision",
            "MicroRecall",
            "MicroF1Score",
            "HammingLoss",
        ] {
            let value = metrics[member].as_f64();
            assert!(
                value.is_some_and(|value| (0.0..=1.0).contains(&value)),
                "case {case:?}: expected {member} in 0..=1, got {metrics}"
            );
        }
        // Each label's documents share their own vocabulary, so a model really
        // fitted on them scores them back perfectly.
        assert_eq!(
            metrics["Accuracy"].as_f64(),
            Some(1.0),
            "case {case:?}: a model fitted on separable documents classifies them back, got \
             {metrics}"
        );

        // The model itself is persisted, and it answers every training document with
        // the label its row carried: the training really fitted something.
        let state = service_state(&**comprehend.engine())
            .expect("the shared engine carries this service's store");
        let store = state.lock();
        let record = store
            .document_classifiers
            .get(&arn)
            .unwrap_or_else(|| panic!("case {case:?}: the classifier record must be held"));
        let model = record
            .model
            .as_ref()
            .unwrap_or_else(|| panic!("case {case:?}: a trained classifier holds its model"));
        assert_eq!(
            model.labels.keys().cloned().collect::<BTreeSet<String>>(),
            labels,
            "case {case:?}: the fitted model must know the labels the data held"
        );
        // The store accounts for the model it holds, which is what its store-wide
        // bound on fitted models is counted against.
        assert!(
            model.model_entries() > 0,
            "case {case:?}: a fitted model holds per-label token counts"
        );
        assert_eq!(
            store.stored_model_entries(),
            model.model_entries(),
            "case {case:?}: the store must account for exactly the one model it holds"
        );
        // The model is asked through the same bounded budget a training run uses, so
        // a test cannot classify past what the service itself would spend.
        let mut budget = TrainingBudget::new(MAX_CLASSIFIER_FIT_OPERATIONS, TRAINING_BUDGET_WAIT);
        for (row_labels, document) in corpus.rows {
            let expected: BTreeSet<String> = if corpus.mode == Some("MULTI_LABEL") {
                row_labels
                    .split(corpus.delimiter.unwrap_or("|"))
                    .map(str::to_string)
                    .collect()
            } else {
                BTreeSet::from([row_labels.to_string()])
            };
            let predicted: BTreeSet<String> = model
                .predict(document, &mut budget)
                .unwrap_or_else(|reason| {
                    panic!("case {case:?}: classifying {document:?} was cut short: {reason}")
                })
                .into_iter()
                .collect();
            assert_eq!(
                predicted, expected,
                "case {case:?}: the fitted model must give {document:?} the labels its row \
                 carried"
            );
        }
    }
}

#[tokio::test]
async fn test_documents_come_from_the_location_the_creation_names() {
    let corpus = &CORPORA[1];
    let case = "a creation naming its own test data";
    let comprehend = Comprehend::with_object_storage();
    create_bucket(&comprehend).await;
    put_object(&comprehend, "train/data.csv", corpus.csv.as_bytes()).await;
    // Two test rows of their own, so the count cannot come from the training share.
    let test_csv = "BILLING,invoice payment refund\nSALES,quote discount contract\n";
    put_object(&comprehend, "test/data.csv", test_csv.as_bytes()).await;

    let arn = create(
        &comprehend,
        "own-test-data",
        corpus,
        &format!("s3://{BUCKET}/train/"),
        Some(&format!("s3://{BUCKET}/test/")),
    )
    .await;
    let (seen, properties) = settled(&comprehend, case, &arn).await;
    assert_eq!(
        seen.last().map(String::as_str),
        Some("TRAINED"),
        "case {case:?}: expected the training to end TRAINED, saw {seen:?}: {properties}"
    );
    let metadata = &properties["ClassifierMetadata"];
    assert_eq!(
        metadata["NumberOfTestDocuments"].as_u64(),
        Some(2),
        "case {case:?}: the test documents are the ones at TestS3Uri, got {metadata}"
    );
    assert_eq!(
        metadata["NumberOfTrainedDocuments"].as_u64(),
        Some(corpus.rows.len() as u64),
        "case {case:?}: naming test data trains on every training row, got {metadata}"
    );
    // The configuration comes back exactly as submitted.
    assert_eq!(
        properties["InputDataConfig"]["TestS3Uri"].as_str(),
        Some(format!("s3://{BUCKET}/test/").as_str()),
        "case {case:?}: Describe must report the TestS3Uri the creation named, got \
         {properties}"
    );
}

#[tokio::test]
async fn a_training_the_emulator_cannot_carry_out_ends_in_error_with_the_reason() {
    // Rows differing along WHY the training cannot be carried out, each naming what
    // it could not do rather than reporting a model nothing fitted.
    let rows: &[(&str, &str, Value, &[&str])] = &[
        (
            "an input location holding nothing",
            "empty-prefix",
            json!({ "S3Uri": format!("s3://{BUCKET}/missing/") }),
            &["No input documents were found", "missing/"],
        ),
        (
            "training data whose every row is unusable",
            "unusable-rows",
            json!({ "S3Uri": format!("s3://{BUCKET}/unusable.csv") }),
            &["holds no labelled document"],
        ),
        (
            "a data format this emulator does not read",
            "augmented-manifest",
            json!({
                "DataFormat": "AUGMENTED_MANIFEST",
                "AugmentedManifests": [{
                    "S3Uri": format!("s3://{BUCKET}/manifest.jsonl"),
                    "AttributeNames": ["label"],
                }],
            }),
            &["AUGMENTED_MANIFEST", "no classifier was trained"],
        ),
        (
            "native documents whose text extraction is Amazon Textract's work",
            "native-documents",
            json!({
                "Documents": { "S3Uri": format!("s3://{BUCKET}/natives/") },
            }),
            &["Textract", "no classifier was trained"],
        ),
    ];

    let comprehend = Comprehend::with_object_storage();
    create_bucket(&comprehend).await;
    put_object(&comprehend, "unusable.csv", b",no label\n\n,\n").await;

    for (case, name, input, expected) in rows {
        let output = comprehend
            .ok(
                "CreateDocumentClassifier",
                &json!({
                    "DocumentClassifierName": name,
                    "DataAccessRoleArn": "arn:aws:iam::123456789012:role/ComprehendRole",
                    "LanguageCode": "en",
                    "InputDataConfig": input,
                }),
            )
            .await;
        let arn = output["DocumentClassifierArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an ARN, got {output}"))
            .to_string();
        let (seen, properties) = settled(&comprehend, case, &arn).await;
        assert_eq!(
            seen.last().map(String::as_str),
            Some("IN_ERROR"),
            "case {case:?}: expected the training to end IN_ERROR, saw {seen:?}: {properties}"
        );
        let message = properties["Message"].as_str().unwrap_or_default();
        for fragment in *expected {
            assert!(
                message.contains(fragment),
                "case {case:?}: expected the Message to mention {fragment:?}, got {message:?}"
            );
        }
        assert!(
            properties["ClassifierMetadata"].is_null(),
            "case {case:?}: a training that fitted nothing reports no metrics, got {properties}"
        );
    }
}

#[tokio::test]
async fn a_corpus_past_the_label_bound_is_refused_rather_than_fitted() {
    // Amazon Comprehend publishes 1,000 as the per-classifier quota for both modes
    // (the "number of classes" of a multi-class classifier and the "number of
    // labels" of a multi-label one). Written out here rather than imported from the
    // service, so the suite pins the published quota: at it a model is fitted, past
    // it the training says so rather than fitting a model of unbounded size.
    const MAX_CLASSIFIER_LABELS: usize = 1_000;

    for (case, labels, trained) in [
        ("at the published label quota", MAX_CLASSIFIER_LABELS, true),
        ("one label past the quota", MAX_CLASSIFIER_LABELS + 1, false),
    ] {
        let comprehend = Comprehend::with_object_storage();
        create_bucket(&comprehend).await;
        let column = (0..labels)
            .map(|index| format!("L{index}"))
            .collect::<Vec<String>>()
            .join("|");
        put_object(
            &comprehend,
            "labels.csv",
            format!("{column},invoice payment refund\n").as_bytes(),
        )
        .await;

        let output = comprehend
            .ok(
                "CreateDocumentClassifier",
                &json!({
                    "DocumentClassifierName": "label-bound",
                    "DataAccessRoleArn": "arn:aws:iam::123456789012:role/ComprehendRole",
                    "LanguageCode": "en",
                    "Mode": "MULTI_LABEL",
                    "InputDataConfig": { "S3Uri": format!("s3://{BUCKET}/labels.csv") },
                }),
            )
            .await;
        let arn = output["DocumentClassifierArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an ARN, got {output}"))
            .to_string();
        let (seen, properties) = settled(&comprehend, case, &arn).await;

        if trained {
            // One row holds no test share back, so the training has that to report:
            // either terminal status is a trained model.
            let status = seen.last().map(String::as_str).unwrap_or_default();
            assert!(
                matches!(status, "TRAINED" | "TRAINED_WITH_WARNING"),
                "case {case:?}: expected a trained model, saw {seen:?}: {properties}"
            );
            assert_eq!(
                properties["ClassifierMetadata"]["NumberOfLabels"].as_u64(),
                Some(labels as u64),
                "case {case:?}: expected the {labels} labels the row named, got {properties}"
            );
            continue;
        }
        assert_eq!(
            seen.last().map(String::as_str),
            Some("IN_ERROR"),
            "case {case:?}: expected the training to end IN_ERROR, saw {seen:?}: {properties}"
        );
        let message = properties["Message"].as_str().unwrap_or_default();
        assert!(
            message.contains(&MAX_CLASSIFIER_LABELS.to_string()),
            "case {case:?}: expected the Message to name the {MAX_CLASSIFIER_LABELS}-label \
             bound, got {message:?}"
        );
        assert!(
            properties["ClassifierMetadata"].is_null(),
            "case {case:?}: a refused training fits nothing, so it reports no metrics, got \
             {properties}"
        );
    }
}

#[tokio::test]
async fn the_training_budget_stops_at_its_operation_limit() {
    // The budget the fit and the measuring pass are charged against: rows differing
    // along how the charges fall, since the rule is about the SUM and not about any
    // one charge. Every row spends the same limit; only the last charge differs.
    for (case, charges, exhausted) in [
        ("one charge inside the limit", &[9u64][..], false),
        ("charges summing exactly to the limit", &[4, 3, 3][..], false),
        ("one charge past the limit", &[11][..], true),
        ("charges summing past the limit", &[6, 4, 1][..], true),
    ] {
        let mut budget = TrainingBudget::new(10, TRAINING_BUDGET_WAIT);
        let mut stopped: Option<String> = None;
        for charge in charges {
            if let Err(reason) = budget.charge(*charge) {
                stopped = Some(reason);
                break;
            }
        }
        assert_eq!(
            stopped.is_some(),
            exhausted,
            "case {case:?}: expected exhausted={exhausted}, got {stopped:?}"
        );
        if let Some(reason) = stopped {
            assert!(
                reason.contains("10 token operations"),
                "case {case:?}: expected the reason to name the budget, got {reason:?}"
            );
        }
    }
}

#[tokio::test]
async fn reading_a_trained_classifier_changes_nothing_a_later_caller_sees() {
    let corpus = &CORPORA[0];
    let comprehend = Comprehend::with_object_storage();
    create_bucket(&comprehend).await;
    put_object(&comprehend, "train.csv", corpus.csv.as_bytes()).await;
    let arn = create(
        &comprehend,
        "stable",
        corpus,
        &format!("s3://{BUCKET}/train.csv"),
        None,
    )
    .await;
    settled(&comprehend, "stable", &arn).await;

    let once = describe(&comprehend, &arn).await;
    let twice = describe(&comprehend, &arn).await;
    assert_eq!(
        once, twice,
        "reading a trained classifier changed it: {once} then {twice}"
    );
}
