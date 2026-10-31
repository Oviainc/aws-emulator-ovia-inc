// SPDX-License-Identifier: CC0-1.0

//! How this service STATUSES the refusals it answers, and when `PutResourcePolicy`
//! judges the policy document it was handed.
//!
//! RULE 1: Comprehend's `awsJson1_1` frontend serves every modelled CLIENT refusal
//! with HTTP 400 — the status the shape's `@error: "client"` kind states — whatever
//! more specific `@httpError` the shape carries, so a refusal the caller can only
//! clear by naming a different resource (the shapes the model pins at 404:
//! `ResourceNotFoundException`, `JobNotFoundException`, `ResourceUnavailableException`)
//! still arrives as a modelled client error rather than as an endpoint or routing
//! fault. The two statuses the frontend keeps are throttling's 429 and a server
//! fault's 5xx; the scaffold channel's 501 is not a modelled refusal at all.
//! [`served_status`] is that rule, and every row of
//! [`every_modelled_refusal_is_served_with_the_status_the_frontend_gives_its_class`]
//! is checked against it rather than against a status written out per row.
//!
//! RULE 2: `PutResourcePolicy` judges the policy DOCUMENT it was given — a property
//! of the request, not of any resource — BEFORE it resolves the `ResourceArn`, so a
//! document the service will not attach earns the same invalid-request refusal
//! whatever ARN the request named, and only a document it would attach makes the ARN
//! worth resolving. A custom-model resource policy exists to let another account
//! import the model, so `comprehend:ImportModel` is the one action it can grant; a
//! statement naming any other, and a document that is not JSON at all, are refused.
//!
//! Both suites are TABLES. Rule 1's rows differ along the operation, the resource
//! class the refusal is about (a model, an endpoint, a job of three families, a tag
//! set) and the status the shape DECLARES (404, 400, and the 501 channel). Rule 2's
//! rows differ along the form the action takes (one action, a list, each wildcard
//! form, another case, another service), along how many statements the document
//! carries, and along whether it is JSON at all — each tried against an ARN that
//! exists AND one that does not. The recorded unit is one row of each.

use serde_json::{json, Value};

use crate::comprehend_client::{error_code, error_message, Comprehend, ACCOUNT_ID, REGION};

/// The message every resource-not-found refusal of this service carries: the
/// service's own fixed sentence, echoing nothing from the request. Written out here
/// rather than imported from the crate, so the suite pins the text instead of
/// whatever the code currently believes.
const RESOURCE_NOT_FOUND_MESSAGE: &str = "RESOURCE_NOT_FOUND: Could not find specified resource.";

/// The fixed sentence a policy naming an action a custom-model policy cannot grant
/// is refused with, behind the service's `POLICY_INVALID_INPUT` reason code.
const UNSUPPORTED_POLICY_ACTION_MESSAGE: &str =
    "POLICY_INVALID_INPUT: The provided policy uses unsupported action.";

/// The fixed sentence a document that is not JSON is refused with, behind the same
/// reason code.
const MALFORMED_POLICY_MESSAGE: &str = "POLICY_INVALID_INPUT: The provided policy is not valid JSON.";

/// RULE 1: the status this service's frontend serves a refusal with, given the status
/// the refusal's own shape declares.
fn served_status(declared: u16) -> u16 {
    match declared {
        429 => 429,
        client if (400..500).contains(&client) => 400,
        other => other,
    }
}

/// One refusal this service answers: how to provoke it, the error code it carries,
/// and the status the SHAPE declares for it (its `@httpError`, or 501 for the
/// scaffold channel, which is not a modelled error).
///
/// `body` is handed the ARN of a model that EXISTS, for a row whose refusal is about
/// something other than the resource (a bound on a tag set, which a resource has to
/// exist to hold).
struct Refusal {
    case: &'static str,
    operation: &'static str,
    body: fn(&str) -> Value,
    code: &'static str,
    declared: u16,
}

/// A well-formed `ComprehendModelArn` naming a classifier nobody created.
fn absent_model() -> String {
    format!("arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:document-classifier/never-created")
}

/// A policy document whose one statement allows `action`, which may be a string or a
/// list of them.
fn single_statement(action: Value) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "AWS": "arn:aws:iam::123456789012:root" },
            "Action": action,
            "Resource": "*",
        }],
    })
    .to_string()
}

/// The policy document every row that needs an acceptable one uses.
fn import_policy() -> String {
    single_statement(json!("comprehend:ImportModel"))
}

/// A `TagList` of `count` distinct tags.
fn tag_list(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({ "Key": format!("k{index}"), "Value": format!("v{index}") }))
            .collect(),
    )
}

#[tokio::test]
async fn every_modelled_refusal_is_served_with_the_status_the_frontend_gives_its_class() {
    let comprehend = Comprehend::new();
    // One model that exists, so the rows about a tag bound are about the bound and
    // not about an empty store.
    let live = create_classifier(&comprehend, "refusal-statuses", None).await;

    // The refusals this service can be made to answer, one row per class. `declared`
    // is the status the shape's own `@httpError` states, read from the model
    // (`smithy_query(query="shape", shape="ResourceNotFoundException")` and its
    // siblings): 404 for the three not-found shapes, 400 for the shapes the model
    // already pins there, and 501 for an operation that is still a scaffold stub.
    let rows: &[Refusal] = &[
        Refusal {
            case: "a resource policy read for a model that does not exist",
            operation: "DescribeResourcePolicy",
            body: |_| json!({ "ResourceArn": absent_model() }),
            code: "ResourceNotFoundException",
            declared: 404,
        },
        Refusal {
            case: "a resource policy attached to a model that does not exist",
            operation: "PutResourcePolicy",
            body: |_| {
                json!({ "ResourceArn": absent_model(), "ResourcePolicy": import_policy() })
            },
            code: "ResourceNotFoundException",
            declared: 404,
        },
        Refusal {
            case: "a resource policy deleted from a model that does not exist",
            operation: "DeleteResourcePolicy",
            body: |_| json!({ "ResourceArn": absent_model() }),
            code: "ResourceNotFoundException",
            declared: 404,
        },
        Refusal {
            case: "the tags of a resource that does not exist",
            operation: "ListTagsForResource",
            body: |_| json!({ "ResourceArn": absent_model() }),
            code: "ResourceNotFoundException",
            declared: 404,
        },
        Refusal {
            case: "tags associated with a resource that does not exist",
            operation: "TagResource",
            body: |_| {
                json!({
                    "ResourceArn": absent_model(),
                    "Tags": [{ "Key": "team", "Value": "research" }],
                })
            },
            code: "ResourceNotFoundException",
            declared: 404,
        },
        Refusal {
            case: "tags removed from a resource that does not exist",
            operation: "UntagResource",
            body: |_| json!({ "ResourceArn": absent_model(), "TagKeys": ["team"] }),
            code: "ResourceNotFoundException",
            declared: 404,
        },
        Refusal {
            case: "a classifier read that names no classifier",
            operation: "DescribeDocumentClassifier",
            body: |_| json!({ "DocumentClassifierArn": absent_model() }),
            code: "ResourceNotFoundException",
            declared: 404,
        },
        Refusal {
            case: "a training stop that names no classifier",
            operation: "StopTrainingDocumentClassifier",
            body: |_| json!({ "DocumentClassifierArn": absent_model() }),
            code: "ResourceNotFoundException",
            declared: 404,
        },
        Refusal {
            case: "an endpoint read that names no endpoint",
            operation: "DescribeEndpoint",
            body: |_| {
                json!({
                    "EndpointArn": format!(
                        "arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:\
                         document-classifier-endpoint/never-created"
                    ),
                })
            },
            code: "ResourceNotFoundException",
            declared: 404,
        },
        Refusal {
            case: "a sentiment job read that names no job",
            operation: "DescribeSentimentDetectionJob",
            body: |_| json!({ "JobId": "doesnotexist" }),
            code: "JobNotFoundException",
            declared: 404,
        },
        Refusal {
            case: "a sentiment job stop that names no job",
            operation: "StopSentimentDetectionJob",
            body: |_| json!({ "JobId": "doesnotexist" }),
            code: "JobNotFoundException",
            declared: 404,
        },
        Refusal {
            case: "a dominant-language job read that names no job",
            operation: "DescribeDominantLanguageDetectionJob",
            body: |_| json!({ "JobId": "0".repeat(32) }),
            code: "JobNotFoundException",
            declared: 404,
        },
        Refusal {
            case: "a PII job read that names no job",
            operation: "DescribePiiEntitiesDetectionJob",
            body: |_| json!({ "JobId": "Job.Name:with/odd=chars" }),
            code: "JobNotFoundException",
            declared: 404,
        },
        Refusal {
            case: "inference through an endpoint this service holds none of",
            operation: "DetectEntities",
            body: |_| {
                json!({
                    "Text": "Jane Doe visited Seattle.",
                    "EndpointArn": format!(
                        "arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:\
                         entity-recognizer-endpoint/analytics"
                    ),
                })
            },
            code: "ResourceUnavailableException",
            declared: 404,
        },
        // Shapes the model already pins at 400: the rule leaves them where they are.
        // The two tag bounds are about the SET a request would leave behind, so they
        // are asked of a resource that exists.
        Refusal {
            case: "a policy document naming an action a model policy cannot grant",
            operation: "PutResourcePolicy",
            body: |_| {
                json!({
                    "ResourceArn": absent_model(),
                    "ResourcePolicy": single_statement(json!("comprehend:DetectEntities")),
                })
            },
            code: "InvalidRequestException",
            declared: 400,
        },
        Refusal {
            case: "more tags than a resource can hold",
            operation: "TagResource",
            body: |live| json!({ "ResourceArn": live, "Tags": tag_list(51) }),
            code: "TooManyTagsException",
            declared: 400,
        },
        Refusal {
            case: "more tag keys than a resource can hold",
            operation: "UntagResource",
            body: |live| {
                json!({
                    "ResourceArn": live,
                    "TagKeys": (0..51).map(|index| format!("k{index}")).collect::<Vec<_>>(),
                })
            },
            code: "TooManyTagKeysException",
            declared: 400,
        },
        // The framework's own boundary refusal, which never had another status.
        Refusal {
            case: "a value below a modelled range bound",
            operation: "ListSentimentDetectionJobs",
            body: |_| json!({ "MaxResults": 0 }),
            code: "ValidationException",
            declared: 400,
        },
        // Not a modelled refusal at all: the scaffold channel, which must stay 501
        // so a client does not retry an operation nobody has written.
        Refusal {
            case: "an operation that is still a scaffold stub",
            operation: "ListDominantLanguageDetectionJobs",
            body: |_| json!({}),
            code: "NotImplemented",
            declared: 501,
        },
    ];

    for row in rows {
        let case = row.case;
        let expected = served_status(row.declared);
        let (status, response) = comprehend.call(row.operation, &(row.body)(&live)).await;
        assert_eq!(
            status, expected,
            "case {case:?} ({}): a refusal whose shape declares {} is served with \
             {expected}, got {status}: {response}",
            row.operation, row.declared
        );
        assert_eq!(
            error_code(&response),
            row.code,
            "case {case:?} ({}): expected {}, got {response}",
            row.operation,
            row.code
        );
        assert!(
            !error_message(&response).is_empty(),
            "case {case:?} ({}): expected a refusal message, got {response}",
            row.operation
        );
    }

    // The status changed, the resolution did not: the model that DOES exist is still
    // served, so nothing above turned a found resource into a refusal.
    let reported = comprehend
        .ok(
            "DescribeDocumentClassifier",
            &json!({ "DocumentClassifierArn": &live }),
        )
        .await;
    assert_eq!(
        reported["DocumentClassifierProperties"]["DocumentClassifierArn"].as_str(),
        Some(live.as_str()),
        "case \"the model that exists\": expected it to still be served, got {reported}"
    );
}

/// One policy document, and the refusal it earns — `None` when the service attaches
/// it.
struct PolicyRow {
    case: &'static str,
    document: fn() -> String,
    refusal: Option<&'static str>,
}

#[tokio::test]
async fn a_policy_document_is_judged_before_the_resource_arn_is_resolved() {
    let comprehend = Comprehend::new();
    let live = create_classifier(&comprehend, "policy-documents", None).await;
    let absent = absent_model();

    // The rows differ along the FORM the action takes, along how many statements the
    // document carries, and along whether the document is JSON at all. An action in a
    // policy statement is matched case-insensitively and admits the `*` and `?`
    // wildcards, so a statement allowing `comprehend:*` names the import action a
    // model policy grants while one allowing another operation names none.
    let rows: &[PolicyRow] = &[
        PolicyRow {
            case: "the one action a model policy grants",
            document: import_policy,
            refusal: None,
        },
        PolicyRow {
            case: "that action as a one-element list",
            document: || single_statement(json!(["comprehend:ImportModel"])),
            refusal: None,
        },
        PolicyRow {
            case: "the whole service as a wildcard",
            document: || single_statement(json!("comprehend:*")),
            refusal: None,
        },
        PolicyRow {
            case: "a wildcard suffix over the action name",
            document: || single_statement(json!("comprehend:Import*")),
            refusal: None,
        },
        PolicyRow {
            case: "a single-character wildcard inside the action name",
            document: || single_statement(json!("comprehend:Impor?Model")),
            refusal: None,
        },
        PolicyRow {
            case: "every action everywhere",
            document: || single_statement(json!("*")),
            refusal: None,
        },
        PolicyRow {
            case: "the action in another case",
            document: || single_statement(json!("COMPREHEND:IMPORTMODEL")),
            refusal: None,
        },
        PolicyRow {
            case: "two statements, each naming the one action",
            document: || {
                json!({
                    "Version": "2012-10-17",
                    "Statement": [
                        {
                            "Effect": "Allow",
                            "Principal": { "AWS": "arn:aws:iam::123456789012:root" },
                            "Action": "comprehend:ImportModel",
                            "Resource": "*",
                        },
                        {
                            "Effect": "Allow",
                            "Principal": { "AWS": "arn:aws:iam::210987654321:root" },
                            "Action": ["comprehend:ImportModel"],
                            "Resource": "*",
                        },
                    ],
                })
                .to_string()
            },
            refusal: None,
        },
        PolicyRow {
            case: "a document naming no action at all",
            document: || json!({ "Version": "2012-10-17" }).to_string(),
            refusal: None,
        },
        PolicyRow {
            case: "another operation of this service",
            document: || single_statement(json!("comprehend:DetectEntities")),
            refusal: Some(UNSUPPORTED_POLICY_ACTION_MESSAGE),
        },
        PolicyRow {
            case: "another service's action",
            document: || single_statement(json!("s3:GetObject")),
            refusal: Some(UNSUPPORTED_POLICY_ACTION_MESSAGE),
        },
        PolicyRow {
            case: "the one action alongside an unsupported one",
            document: || {
                single_statement(json!([
                    "comprehend:ImportModel",
                    "comprehend:DeleteDocumentClassifier",
                ]))
            },
            refusal: Some(UNSUPPORTED_POLICY_ACTION_MESSAGE),
        },
        PolicyRow {
            case: "another service's wildcard",
            document: || single_statement(json!("iam:*")),
            refusal: Some(UNSUPPORTED_POLICY_ACTION_MESSAGE),
        },
        PolicyRow {
            case: "an unsupported action in the second statement",
            document: || {
                json!({
                    "Version": "2012-10-17",
                    "Statement": [
                        {
                            "Effect": "Allow",
                            "Principal": { "AWS": "arn:aws:iam::123456789012:root" },
                            "Action": "comprehend:ImportModel",
                            "Resource": "*",
                        },
                        {
                            "Effect": "Allow",
                            "Principal": { "AWS": "arn:aws:iam::210987654321:root" },
                            "Action": "comprehend:StartTrainingDocumentClassifier",
                            "Resource": "*",
                        },
                    ],
                })
                .to_string()
            },
            refusal: Some(UNSUPPORTED_POLICY_ACTION_MESSAGE),
        },
        PolicyRow {
            case: "a document that is not JSON",
            document: || "allow everyone to import this model".to_string(),
            refusal: Some(MALFORMED_POLICY_MESSAGE),
        },
        PolicyRow {
            case: "a document that is JSON with an unbalanced brace",
            document: || "{\"Version\": \"2012-10-17\", \"Statement\": [".to_string(),
            refusal: Some(MALFORMED_POLICY_MESSAGE),
        },
    ];

    for row in rows {
        let document = (row.document)();
        // The SAME document against a model that exists and against an ARN that names
        // none: a refusal of the document is identical either way, because the
        // document is judged before the ARN is resolved.
        for (target, arn) in [
            ("a model that exists", &live),
            ("an ARN that names no model", &absent),
        ] {
            let case = format!("{} / {target}", row.case);
            let (status, response) = comprehend
                .call(
                    "PutResourcePolicy",
                    &json!({ "ResourceArn": arn, "ResourcePolicy": &document }),
                )
                .await;
            match (row.refusal, target) {
                (Some(message), _) => {
                    assert_eq!(
                        status, 400,
                        "case {case:?}: expected 400, got {status}: {response}"
                    );
                    assert_eq!(
                        error_code(&response),
                        "InvalidRequestException",
                        "case {case:?}: a document the service will not attach is an \
                         invalid request, got {response}"
                    );
                    assert_eq!(
                        error_message(&response),
                        message,
                        "case {case:?}: expected the service's fixed sentence, got {response}"
                    );
                }
                (None, "a model that exists") => {
                    assert_eq!(
                        status, 200,
                        "case {case:?}: expected 200, got {status}: {response}"
                    );
                    let reported = comprehend
                        .ok("DescribeResourcePolicy", &json!({ "ResourceArn": arn }))
                        .await;
                    assert_eq!(
                        reported["ResourcePolicy"].as_str(),
                        Some(document.as_str()),
                        "case {case:?}: the attached document must read back, got {reported}"
                    );
                }
                (None, _) => {
                    assert_eq!(
                        status, 400,
                        "case {case:?}: expected 400, got {status}: {response}"
                    );
                    assert_eq!(
                        error_code(&response),
                        "ResourceNotFoundException",
                        "case {case:?}: a document the service accepts leaves the ARN to \
                         resolve, got {response}"
                    );
                    assert_eq!(
                        error_message(&response),
                        RESOURCE_NOT_FOUND_MESSAGE,
                        "case {case:?}: expected the service's fixed sentence, got {response}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn a_refused_document_leaves_the_attached_policy_alone() {
    let comprehend = Comprehend::new();
    let live = create_classifier(&comprehend, "refused-documents", None).await;

    let attached = import_policy();
    let minted = comprehend
        .ok(
            "PutResourcePolicy",
            &json!({ "ResourceArn": &live, "ResourcePolicy": &attached }),
        )
        .await;
    let revision = minted["PolicyRevisionId"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a PolicyRevisionId, got {minted}"))
        .to_string();

    for (case, document) in [
        (
            "an unsupported action",
            single_statement(json!("comprehend:DetectEntities")),
        ),
        ("a document that is not JSON", "not a policy".to_string()),
    ] {
        let (status, response) = comprehend
            .call(
                "PutResourcePolicy",
                &json!({ "ResourceArn": &live, "ResourcePolicy": document }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
        let reported = comprehend
            .ok("DescribeResourcePolicy", &json!({ "ResourceArn": &live }))
            .await;
        assert_eq!(
            reported["ResourcePolicy"].as_str(),
            Some(attached.as_str()),
            "case {case:?}: a refused modification must leave the document alone, got \
             {reported}"
        );
        assert_eq!(
            reported["PolicyRevisionId"].as_str(),
            Some(revision.as_str()),
            "case {case:?}: a refused modification mints no revision, got {reported}"
        );
    }
}

#[tokio::test]
async fn a_creation_judges_the_model_policy_it_carries_by_the_same_rule() {
    let comprehend = Comprehend::new();

    // `ModelPolicy` is the same policy `PutResourcePolicy` attaches, so the same
    // documents are accepted and refused, and a refused creation persists nothing.
    for (case, document, refusal) in [
        ("the one action a model policy grants", import_policy(), None),
        (
            "another operation of this service",
            single_statement(json!("comprehend:ListDocumentClassifiers")),
            Some(UNSUPPORTED_POLICY_ACTION_MESSAGE),
        ),
        (
            "a document that is not JSON",
            "allow everyone".to_string(),
            Some(MALFORMED_POLICY_MESSAGE),
        ),
    ] {
        let name = format!("model-policy-{}", case.replace(' ', "-"));
        let (status, response) = comprehend
            .call("CreateDocumentClassifier", &creation(&name, Some(&document)))
            .await;
        match refusal {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200, got {status}: {response}"
                );
                let arn = response["DocumentClassifierArn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected an ARN, got {response}"));
                let reported = comprehend
                    .ok("DescribeResourcePolicy", &json!({ "ResourceArn": arn }))
                    .await;
                assert_eq!(
                    reported["ResourcePolicy"].as_str(),
                    Some(document.as_str()),
                    "case {case:?}: the creation's ModelPolicy must read back, got {reported}"
                );
            }
            Some(message) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected 400, got {status}: {response}"
                );
                assert_eq!(
                    error_code(&response),
                    "InvalidRequestException",
                    "case {case:?}: expected InvalidRequestException, got {response}"
                );
                assert_eq!(
                    error_message(&response),
                    message,
                    "case {case:?}: expected the service's fixed sentence, got {response}"
                );
                // A refused creation persists no classifier, so the ARN it would have
                // minted names nothing.
                let (status, body) = comprehend
                    .call(
                        "DescribeDocumentClassifier",
                        &json!({
                            "DocumentClassifierArn": format!(
                                "arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:\
                                 document-classifier/{name}"
                            ),
                        }),
                    )
                    .await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: a refused creation persists nothing, got {status}: {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "ResourceNotFoundException",
                    "case {case:?}: a refused creation persists nothing, got {body}"
                );
            }
        }
    }
}

/// A `CreateDocumentClassifier` body for `name`, carrying `model_policy` when one is
/// given.
fn creation(name: &str, model_policy: Option<&str>) -> Value {
    let mut body = json!({
        "DocumentClassifierName": name,
        "DataAccessRoleArn": "arn:aws:iam::123456789012:role/ComprehendRole",
        "LanguageCode": "en",
        "InputDataConfig": { "S3Uri": "s3://policy-bucket/train.csv" },
    });
    if let Some(policy) = model_policy {
        body["ModelPolicy"] = json!(policy);
    }
    body
}

/// Create a custom model and return the ARN its creation minted.
///
/// The training itself cannot be carried out here (this suite's emulator mounts no
/// object storage), which is beside the point: a policy and a tag set live on the
/// model's record whatever its training reports.
async fn create_classifier(
    comprehend: &Comprehend,
    name: &str,
    model_policy: Option<&str>,
) -> String {
    let output = comprehend
        .ok("CreateDocumentClassifier", &creation(name, model_policy))
        .await;
    output["DocumentClassifierArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateDocumentClassifier {name}: expected an ARN, got {output}"))
        .to_string()
}
