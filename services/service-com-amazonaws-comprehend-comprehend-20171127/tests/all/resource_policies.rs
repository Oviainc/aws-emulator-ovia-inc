// SPDX-License-Identifier: CC0-1.0

//! Comprehend's custom-model resource policies: `PutResourcePolicy`,
//! `DescribeResourcePolicy` and `DeleteResourcePolicy`.
//!
//! RULE: a resource policy is state of the custom model its ARN names. A `Put`
//! against a model that exists attaches (or replaces) the ONE policy it carries and
//! mints a fresh `PolicyRevisionId`; a `Describe` reports exactly the document that
//! was attached, with that revision and the creation and modification instants; a
//! `Delete` removes it and is a no-op for a model carrying none; and all three answer
//! `ResourceNotFoundException` — the service's client refusal, HTTP 400, with its
//! FIXED message, which echoes nothing from the request — for an ARN that names no
//! model. A `Put` judges the POLICY DOCUMENT before it resolves the ARN, so a
//! document the service will not attach is an invalid request whatever the ARN names.
//!
//! Every suite is a TABLE: the policy documents differ along their shape, their size
//! (both modelled length bounds) and their characters, and the models differ along
//! whether they carry a version. The issue's own request — a delete and a put against
//! a classifier ARN that was never created — is one row of the not-found table.

use serde_json::{json, Value};

use crate::comprehend_client::{error_code, error_message, Comprehend, ACCOUNT_ID, REGION};

/// The message every resource-not-found refusal of this service carries.
///
/// Amazon Comprehend's own fixed sentence, carrying its reason code and NOTHING
/// derived from the request: the ARN the caller asked for is never echoed back.
/// Written out here rather than imported from the service, so the suite pins the
/// recorded text instead of whatever the code currently believes.
const RESOURCE_NOT_FOUND_MESSAGE: &str = "RESOURCE_NOT_FOUND: Could not find specified resource.";

/// The modelled `Policy` length bounds, from the member's own constraint
/// (`len 1..20000`).
const MAX_POLICY_CHARACTERS: usize = 20_000;

/// The policy documents every suite below attaches, named by what makes each one
/// different.
///
/// The rule generalises over the document, so the rows differ along shape, size and
/// characters rather than along one value: a policy is stored and reported byte for
/// byte whatever it holds.
fn policy(case: &str) -> String {
    match case {
        "a full cross-account import policy" => json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": { "AWS": ["arn:aws:iam::123456789012:root"] },
                "Action": "comprehend:ImportModel",
                "Resource": "*",
            }],
        })
        .to_string(),
        "an empty statement list" => json!({ "Version": "2012-10-17", "Statement": [] }).to_string(),
        // The modelled pattern admits every character from U+0020 to U+00FF, plus
        // tab, newline and carriage return, so a Latin-1 principal name and a
        // formatted document are both policies this operation accepts.
        "a latin-1 principal and embedded whitespace" => {
            "{\n\t\"Version\": \"2012-10-17\",\n\t\"Statement\": \"café-naïve-crème\"\n}".to_string()
        }
        "a policy at the modelled length ceiling" => {
            let head = "{\"Version\":\"2012-10-17\",\"Statement\":\"";
            let tail = "\"}";
            format!(
                "{head}{}{tail}",
                "p".repeat(MAX_POLICY_CHARACTERS - head.len() - tail.len())
            )
        }
        other => panic!("unknown policy case {other:?}"),
    }
}

/// The cases [`policy`] builds.
const POLICY_CASES: &[&str] = &[
    "a full cross-account import policy",
    "an empty statement list",
    "a latin-1 principal and embedded whitespace",
    "a policy at the modelled length ceiling",
];

/// Create a custom model and return the ARN it minted, attaching `model_policy` at
/// creation when one is given.
///
/// The training itself cannot be carried out here (this suite's emulator mounts no
/// object storage), which is beside the point: a policy is attached to the model
/// record, whatever its training reports.
async fn create_classifier(
    comprehend: &Comprehend,
    name: &str,
    version: Option<&str>,
    model_policy: Option<&str>,
) -> String {
    let mut body = json!({
        "DocumentClassifierName": name,
        "DataAccessRoleArn": "arn:aws:iam::123456789012:role/ComprehendRole",
        "LanguageCode": "en",
        "InputDataConfig": { "S3Uri": "s3://policy-bucket/train.csv" },
    });
    if let Some(version) = version {
        body["VersionName"] = json!(version);
    }
    if let Some(policy) = model_policy {
        body["ModelPolicy"] = json!(policy);
    }
    let output = comprehend.ok("CreateDocumentClassifier", &body).await;
    output["DocumentClassifierArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateDocumentClassifier {name}: expected an ARN, got {output}"))
        .to_string()
}

/// Wait until `arn`'s training has ended, so the model can be deleted.
///
/// "Only those classifiers that are in terminated states will be deleted": a
/// classifier still `SUBMITTED` or `TRAINING` is in use.
async fn wait_until_training_ended(comprehend: &Comprehend, arn: &str) {
    let started = std::time::Instant::now();
    loop {
        let output = comprehend
            .ok(
                "DescribeDocumentClassifier",
                &json!({ "DocumentClassifierArn": arn }),
            )
            .await;
        let status = output["DocumentClassifierProperties"]["Status"]
            .as_str()
            .unwrap_or_default();
        if matches!(
            status,
            "TRAINED" | "TRAINED_WITH_WARNING" | "IN_ERROR" | "STOPPED"
        ) {
            return;
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(30),
            "case {arn:?}: the training never ended, last {output}"
        );
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
}

/// `DescribeResourcePolicy` for `resource_arn`, asserting a 200.
async fn describe(comprehend: &Comprehend, resource_arn: &str) -> Value {
    comprehend
        .ok(
            "DescribeResourcePolicy",
            &json!({ "ResourceArn": resource_arn }),
        )
        .await
}

/// The `PolicyRevisionId` a modification minted, asserting the modelled form.
fn revision_id(case: &str, output: &Value) -> String {
    let revision = output["PolicyRevisionId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a PolicyRevisionId, got {output}"))
        .to_string();
    assert!(
        !revision.is_empty()
            && revision.len() <= 64
            && revision.chars().all(|c| c.is_ascii_hexdigit()),
        "case {case:?}: a PolicyRevisionId is at most 64 hexadecimal characters, got \
         {revision:?}"
    );
    revision
}

#[tokio::test]
async fn an_attached_policy_round_trips_through_describe() {
    let comprehend = Comprehend::new();

    // The models differ along whether they carry a version, because the ARN a
    // versioned model names is its own resource.
    for (model, version) in [("plain-model", None), ("versioned-model", Some("v1"))] {
        for case in POLICY_CASES {
            let document = policy(case);
            // One model per row, so each row starts from a model carrying nothing.
            let name = format!("{model}-{}", case.replace(' ', "-"));
            let arn = create_classifier(&comprehend, &name, version, None).await;
            let case = format!("{model}/{case}");

            // Before anything is attached: a 200 reporting no policy, and no
            // revision or instant for a policy that does not exist.
            let empty = describe(&comprehend, &arn).await;
            for member in [
                "ResourcePolicy",
                "PolicyRevisionId",
                "CreationTime",
                "LastModifiedTime",
            ] {
                assert!(
                    empty.get(member).is_none_or(Value::is_null),
                    "case {case:?}: a model carrying no policy reports no {member}, got {empty}"
                );
            }

            let attached = comprehend
                .ok(
                    "PutResourcePolicy",
                    &json!({ "ResourceArn": arn, "ResourcePolicy": document }),
                )
                .await;
            let revision = revision_id(&case, &attached);

            let reported = describe(&comprehend, &arn).await;
            assert_eq!(
                reported["ResourcePolicy"].as_str(),
                Some(document.as_str()),
                "case {case:?}: Describe must report the document that was attached, got \
                 {reported}"
            );
            assert_eq!(
                reported["PolicyRevisionId"].as_str(),
                Some(revision.as_str()),
                "case {case:?}: Describe must report the revision Put minted, got {reported}"
            );
            let created = reported["CreationTime"].as_f64().unwrap_or_default();
            let modified = reported["LastModifiedTime"].as_f64().unwrap_or_default();
            assert!(
                created > 0.0 && modified >= created,
                "case {case:?}: expected a creation instant and a modification instant no \
                 earlier than it, got {reported}"
            );

            // A read changes nothing a later caller sees.
            let again = describe(&comprehend, &arn).await;
            assert_eq!(
                reported, again,
                "case {case:?}: reading the policy twice returned different answers: \
                 {reported} then {again}"
            );

            // Deleting it leaves the model carrying none, and a second delete is a
            // no-op rather than a failure.
            for attempt in ["first", "second"] {
                let deleted = comprehend
                    .ok(
                        "DeleteResourcePolicy",
                        &json!({ "ResourceArn": arn }),
                    )
                    .await;
                assert!(
                    deleted.is_null() || deleted == json!({}),
                    "case {case:?}/{attempt} delete: the operation answers 200 with an empty \
                     body, got {deleted}"
                );
                let gone = describe(&comprehend, &arn).await;
                assert!(
                    gone.get("ResourcePolicy").is_none_or(Value::is_null),
                    "case {case:?}/{attempt} delete: the policy must be gone, got {gone}"
                );
            }

        }
    }
}

#[tokio::test]
async fn every_modification_mints_a_new_revision_and_keeps_the_creation_instant() {
    let comprehend = Comprehend::new();
    let arn = create_classifier(&comprehend, "revisions", None, None).await;

    let mut revisions: Vec<String> = Vec::new();
    let mut created: Option<f64> = None;
    // Every row replaces the policy the row before it attached — including the SAME
    // document twice, which is still a modification and still mints a revision.
    let documents: Vec<String> = POLICY_CASES
        .iter()
        .map(|case| policy(case))
        .chain(std::iter::once(policy(POLICY_CASES[0])))
        .collect();
    for (index, document) in documents.iter().enumerate() {
        let case = format!("modification {index}");
        let attached = comprehend
            .ok(
                "PutResourcePolicy",
                &json!({ "ResourceArn": arn, "ResourcePolicy": document }),
            )
            .await;
        let revision = revision_id(&case, &attached);
        assert!(
            !revisions.contains(&revision),
            "case {case:?}: every modification mints a NEW revision, but {revision:?} was \
             already seen in {revisions:?}"
        );
        revisions.push(revision.clone());

        let reported = describe(&comprehend, &arn).await;
        assert_eq!(
            reported["ResourcePolicy"].as_str(),
            Some(document.as_str()),
            "case {case:?}: the latest document replaces the one before it, got {reported}"
        );
        assert_eq!(
            reported["PolicyRevisionId"].as_str(),
            Some(revision.as_str()),
            "case {case:?}: Describe must report the latest revision, got {reported}"
        );
        // The creation instant is the FIRST attachment's and never moves; the
        // modification instant tracks the latest one.
        let now_created = reported["CreationTime"].as_f64().unwrap_or_default();
        match created {
            None => created = Some(now_created),
            Some(first) => assert_eq!(
                now_created, first,
                "case {case:?}: a replacement must not move the creation instant, got \
                 {reported}"
            ),
        }
        assert!(
            reported["LastModifiedTime"].as_f64().unwrap_or_default() >= now_created,
            "case {case:?}: the modification instant is no earlier than the creation \
             instant, got {reported}"
        );
    }
}

#[tokio::test]
async fn a_modification_naming_a_revision_the_model_does_not_carry_is_refused() {
    let comprehend = Comprehend::new();
    let arn = create_classifier(&comprehend, "revision-guard", None, None).await;
    let document = policy(POLICY_CASES[0]);

    // A model carrying no policy: "if you are creating a new policy that has no
    // prior version, don't use this parameter", so naming a revision names one that
    // is not there.
    for operation in ["PutResourcePolicy", "DeleteResourcePolicy"] {
        let (status, response) = comprehend
            .call(
                operation,
                &json!({
                    "ResourceArn": arn,
                    "ResourcePolicy": document,
                    "PolicyRevisionId": "0123456789ABCDEF0123456789ABCDEF",
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {operation}/\"no policy attached\": expected 400, got {status}: {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidRequestException",
            "case {operation}/\"no policy attached\": expected InvalidRequestException, got \
             {response}"
        );
    }

    let attached = comprehend
        .ok(
            "PutResourcePolicy",
            &json!({ "ResourceArn": arn, "ResourcePolicy": document }),
        )
        .await;
    let revision = revision_id("attach", &attached);

    // A revision that is not the current one is refused; the current one is
    // accepted and supersedes itself.
    let stale = format!("{:0>32}", "ABC");
    for (case, named, accepted) in [
        ("a revision nobody minted", stale.as_str(), false),
        ("the current revision", revision.as_str(), true),
    ] {
        let (status, response) = comprehend
            .call(
                "PutResourcePolicy",
                &json!({
                    "ResourceArn": arn,
                    "ResourcePolicy": document,
                    "PolicyRevisionId": named,
                }),
            )
            .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status}: {response}"
            );
            let superseded = revision_id(case, &response);
            assert_ne!(
                superseded, revision,
                "case {case:?}: a modification mints a new revision, got {response}"
            );
            // And the stale revision is now refused for the delete too.
            let (status, response) = comprehend
                .call(
                    "DeleteResourcePolicy",
                    &json!({ "ResourceArn": arn, "PolicyRevisionId": revision }),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: a delete naming the superseded revision is refused, got \
                 {status}: {response}"
            );
            // The current revision deletes it.
            comprehend
                .ok(
                    "DeleteResourcePolicy",
                    &json!({ "ResourceArn": arn, "PolicyRevisionId": superseded }),
                )
                .await;
            let gone = describe(&comprehend, &arn).await;
            assert!(
                gone.get("ResourcePolicy").is_none_or(Value::is_null),
                "case {case:?}: the policy must be gone, got {gone}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {response}"
        );
        assert_eq!(
            error_code(&response),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {response}"
        );
    }
}

#[tokio::test]
async fn a_creation_that_attaches_a_model_policy_is_the_policy_describe_reports() {
    let comprehend = Comprehend::new();

    // `ModelPolicy` is "the resource-based policy to attach to your custom model", so
    // a creation that carries one is the first revision of the SAME policy the
    // resource-policy operations work on.
    for case in POLICY_CASES {
        let document = policy(case);
        let arn = create_classifier(
            &comprehend,
            &format!("at-creation-{}", case.replace(' ', "-")),
            None,
            Some(&document),
        )
        .await;
        let reported = describe(&comprehend, &arn).await;
        assert_eq!(
            reported["ResourcePolicy"].as_str(),
            Some(document.as_str()),
            "case {case:?}: Describe must report the ModelPolicy the creation attached, got \
             {reported}"
        );
        revision_id(case, &reported);

        // And it is replaceable and deletable like any other.
        let replaced = comprehend
            .ok(
                "PutResourcePolicy",
                &json!({ "ResourceArn": arn, "ResourcePolicy": policy(POLICY_CASES[1]) }),
            )
            .await;
        revision_id(case, &replaced);
        comprehend
            .ok("DeleteResourcePolicy", &json!({ "ResourceArn": arn }))
            .await;
        let gone = describe(&comprehend, &arn).await;
        assert!(
            gone.get("ResourcePolicy").is_none_or(Value::is_null),
            "case {case:?}: the policy must be gone, got {gone}"
        );
    }
}

#[tokio::test]
async fn every_resource_policy_operation_refuses_an_arn_that_names_no_model() {
    let comprehend = Comprehend::new();
    // One real model, so a refusal is about the ARN and not an empty store.
    let live = create_classifier(&comprehend, "live-model", None, None).await;

    // Well-formed `ComprehendModelArn`s that name nothing: both resource types the
    // modelled pattern admits, a versioned form, another region's model and the live
    // model's own name with a version it was never created under.
    let unknown = [
        format!("arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:document-classifier/never-created"),
        format!("arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:entity-recognizer/never-created"),
        format!(
            "arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:document-classifier/never-created/version/v1"
        ),
        format!("arn:aws:comprehend:us-east-1:{ACCOUNT_ID}:document-classifier/live-model"),
        format!("arn:aws:comprehend:{REGION}:{ACCOUNT_ID}:document-classifier/live-model/version/v9"),
        format!("arn:aws:comprehend:{REGION}:210987654321:document-classifier/live-model"),
    ];

    for arn in &unknown {
        for operation in [
            "DescribeResourcePolicy",
            "PutResourcePolicy",
            "DeleteResourcePolicy",
        ] {
            let mut body = json!({ "ResourceArn": arn });
            if operation == "PutResourcePolicy" {
                body["ResourcePolicy"] = json!(policy(POLICY_CASES[0]));
            }
            let (status, response) = comprehend.call(operation, &body).await;
            assert_eq!(
                status, 400,
                "case {operation}/{arn:?}: expected 400, got {status}: {response}"
            );
            assert_eq!(
                error_code(&response),
                "ResourceNotFoundException",
                "case {operation}/{arn:?}: expected ResourceNotFoundException, got {response}"
            );
            assert_eq!(
                error_message(&response),
                RESOURCE_NOT_FOUND_MESSAGE,
                "case {operation}/{arn:?}: the refusal carries the service's fixed message \
                 and echoes nothing from the request, got {response}"
            );
        }
    }

    // A model whose deletion has completed is one of them: its policy goes with it.
    comprehend
        .ok(
            "PutResourcePolicy",
            &json!({ "ResourceArn": live, "ResourcePolicy": policy(POLICY_CASES[0]) }),
        )
        .await;
    wait_until_training_ended(&comprehend, &live).await;
    comprehend
        .ok(
            "DeleteDocumentClassifier",
            &json!({ "DocumentClassifierArn": live }),
        )
        .await;
    let started = std::time::Instant::now();
    loop {
        let (status, response) = comprehend
            .call("DescribeResourcePolicy", &json!({ "ResourceArn": live }))
            .await;
        if status == 400 {
            assert_eq!(
                error_message(&response),
                RESOURCE_NOT_FOUND_MESSAGE,
                "case \"deleted model\": the refusal carries the service's fixed message, got \
                 {response}"
            );
            break;
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(30),
            "case \"deleted model\": the model's record was never removed, last {response}"
        );
        std::thread::sleep(std::time::Duration::from_millis(40));
    }
}

#[tokio::test]
async fn a_policy_past_the_modelled_length_is_refused_before_the_handler() {
    let comprehend = Comprehend::new();
    let arn = create_classifier(&comprehend, "length-bounds", None, None).await;

    // The modelled `Policy` length, at the ceiling and one past it. A refusal at the
    // boundary leaves the model carrying whatever it already had.
    for (case, characters, accepted) in [
        ("at the ceiling", MAX_POLICY_CHARACTERS, true),
        ("one past the ceiling", MAX_POLICY_CHARACTERS + 1, false),
    ] {
        let head = "{\"Version\":\"2012-10-17\",\"Statement\":\"";
        let tail = "\"}";
        let document = format!(
            "{head}{}{tail}",
            "p".repeat(characters - head.len() - tail.len())
        );
        assert_eq!(
            document.chars().count(),
            characters,
            "case {case:?}: the row must be exactly {characters} characters"
        );
        let (status, response) = comprehend
            .call(
                "PutResourcePolicy",
                &json!({ "ResourceArn": arn, "ResourcePolicy": document }),
            )
            .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status}: {response}"
            );
            let reported = describe(&comprehend, &arn).await;
            assert_eq!(
                reported["ResourcePolicy"].as_str(),
                Some(document.as_str()),
                "case {case:?}: the stored document is the one submitted, got a different one"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status}: {response}"
            );
        }
    }
}
