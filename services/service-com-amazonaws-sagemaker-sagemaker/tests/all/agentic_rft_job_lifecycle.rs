// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's model-customization job lifecycle:
//! `CreateJob` -> `DescribeJob`, and the registration the AgenticRFT job runtime data
//! plane resolves a job ARN against.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so what
//! these tests exercise is what a real SDK client gets. The harness (`Emulator`,
//! `error_code`, `message`, `str_field`) is the one `inference_deployment_lifecycle`
//! already defines for this target.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the
//! create/describe round trip for every `JobCategory`, name shape, Region and optional
//! member; the ARN form; the account-level uniqueness of `JobName`; the refusals a request
//! the model cannot express gets; and that a created job — and only a created job — is
//! resolvable by the data plane that serves its rollouts.

use serde_json::{json, Value};
use service_common::agentic_rft_jobs::JobDirectory;

use crate::inference_deployment_lifecycle::{error_code, message, str_field, Emulator, ACCOUNT};

/// A `RoleArn` satisfying the modelled pattern
/// `^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`.
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/service-role/AmazonSageMaker-ExecutionRole";

/// A `JobConfigSchemaVersion` satisfying the modelled `^\d+\.\d+\.\d+$` and `len 5..16`.
const SCHEMA_VERSION: &str = "1.0.0";

/// Every member of the `JobCategory` enum.
///
/// Catalog source: `smithy_query(query="shape", shape="JobCategory")` — exactly `AgentRFT`
/// and `AgentRFTEvaluation`.
const JOB_CATEGORIES: [&str; 2] = ["AgentRFT", "AgentRFTEvaluation"];

/// A `JobConfigDocument` that conforms to the schema this service publishes through
/// `DescribeJobSchemaVersion`: an object carrying every property that schema requires.
fn config_document() -> String {
    config_document_naming("demo-model")
}

/// [`config_document`] with the base model the customization starts from, so two
/// conforming documents can be told apart.
fn config_document_naming(model: &str) -> String {
    json!({
        "AgentConfig": { "MaxTurns": 8 },
        "InputDataConfig": { "S3Uri": "s3://prompts/train" },
        "ModelPackageConfig": { "BaseModel": model },
        "OutputDataConfig": { "S3Uri": "s3://artifacts/out" },
        "TrainingConfig": { "Epochs": 1 },
    })
    .to_string()
}

/// A minimal valid `CreateJob` body.
fn create_body(name: &str, category: &str) -> Value {
    json!({
        "JobName": name,
        "JobCategory": category,
        "JobConfigSchemaVersion": SCHEMA_VERSION,
        "JobConfigDocument": config_document(),
        "RoleArn": ROLE_ARN,
    })
}

/// RULE: `DescribeJob` replays exactly what `CreateJob` persisted, under the ARN
/// `arn:aws:sagemaker:<signed region>:<account>:job/<JobCategory>/<JobName>`, and reports
/// the job in the non-terminal status it was accepted in.
#[tokio::test]
async fn describe_replays_the_stored_job_for_every_category_name_and_region() {
    // Rows differ along the axes the rule generalises over: the job's category, the
    // Region the caller signed for, the name's shape (one character, digits, hyphens and
    // the modelled 63-character maximum — `JobName` is `len 1..63`, pattern
    // `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`), and whether the create carried `Tags`.
    let at_max_name = "a234567890123456789012345678901234567890123456789012345678901b";
    let rows: [(&str, &str, &str, &str, bool); 6] = [
        ("one-character name", "j", JOB_CATEGORIES[0], "us-east-1", false),
        ("digits in the name", "rft2run3", JOB_CATEGORIES[0], "us-east-1", true),
        (
            "hyphens in the name",
            "agentic-rft-rollout-1",
            JOB_CATEGORIES[0],
            "eu-west-1",
            false,
        ),
        (
            "the evaluation category",
            "eval-run-1",
            JOB_CATEGORIES[1],
            "us-west-2",
            true,
        ),
        (
            "the 63-character maximum",
            at_max_name,
            JOB_CATEGORIES[1],
            "ap-southeast-2",
            false,
        ),
        ("another signed region", "rollout-9", JOB_CATEGORIES[0], "eu-central-1", false),
    ];

    for (case, name, category, region, tagged) in rows {
        let emulator = Emulator::new();
        let mut body = create_body(name, category);
        if tagged {
            body["Tags"] = json!([{ "Key": "team", "Value": "rl" }]);
        }

        let (status, created) = emulator.call_in(region, "CreateJob", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateJob, got {status} with {created}"
        );
        let expected_arn = format!("arn:aws:sagemaker:{region}:{ACCOUNT}:job/{category}/{name}");
        assert_eq!(
            str_field(&created, "JobArn"),
            expected_arn,
            "case {case:?}: JobArn, got {created}"
        );

        let (status, described) = emulator
            .call_in(
                region,
                "DescribeJob",
                &json!({ "JobName": name, "JobCategory": category }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeJob, got {status} with {described}"
        );
        for (member, expected) in [
            ("JobName", name.to_string()),
            ("JobArn", expected_arn.clone()),
            ("JobCategory", category.to_string()),
            ("RoleArn", ROLE_ARN.to_string()),
            ("JobConfigSchemaVersion", SCHEMA_VERSION.to_string()),
            ("JobConfigDocument", config_document()),
            // `JobStatus` / `JobSecondaryStatus` members, from the model's own enums.
            ("JobStatus", "InProgress".to_string()),
            ("SecondaryStatus", "Starting".to_string()),
        ] {
            assert_eq!(
                str_field(&described, member),
                expected,
                "case {case:?}: {member} must replay what was stored, got {described}"
            );
        }
        assert_eq!(
            described
                .get("SecondaryStatusTransitions")
                .and_then(Value::as_array)
                .map(|transitions| transitions.len()),
            Some(1),
            "case {case:?}: expected the one secondary status the job has been in, got {described}"
        );
        assert!(
            described.get("EndTime").is_none() && described.get("FailureReason").is_none(),
            "case {case:?}: a running job has no EndTime and no FailureReason, got {described}"
        );
        let expected_tags = if tagged {
            json!([{ "Key": "team", "Value": "rl" }])
        } else {
            json!([])
        };
        assert_eq!(
            described.get("Tags"),
            Some(&expected_tags),
            "case {case:?}: Tags must replay what was stored, got {described}"
        );

        // Reading it again changes nothing: a describe is a read.
        let (_, again) = emulator
            .call_in(
                region,
                "DescribeJob",
                &json!({ "JobName": name, "JobCategory": category }),
            )
            .await;
        assert_eq!(
            again, described,
            "case {case:?}: a second DescribeJob must answer the same record, got {again}"
        );
    }
}

/// RULE: a created job — and only a created job — is resolvable by the AgenticRFT job
/// runtime data plane, which has no call of its own that creates one: `CreateJob` writes
/// the shared directory that data plane resolves a `JobArn` against.
#[tokio::test]
async fn a_created_job_is_registered_for_the_job_runtime_data_plane() {
    let emulator = Emulator::new();

    // Rows differ along the category and the Region the job was created in; the last row
    // is a job that was NEVER created, which must resolve to nothing.
    let rows: [(&str, &str, &str, bool); 4] = [
        ("created, default region", "rollout-1", JOB_CATEGORIES[0], true),
        ("created, evaluation category", "eval-1", JOB_CATEGORIES[1], true),
        ("created, another region", "rollout-2", JOB_CATEGORIES[0], true),
        ("never created", "ghost-1", JOB_CATEGORIES[0], false),
    ];

    for (case, name, category, created) in rows {
        if created {
            let (status, body) = emulator
                .call("CreateJob", &create_body(name, category))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from CreateJob, got {status} with {body}"
            );
        }

        // The data plane resolves the ARN it is handed, whatever Region or account it was
        // addressed through, so the row is read exactly the way `Sample` reads it.
        let arn = format!("arn:aws:sagemaker:us-west-2:{ACCOUNT}:job/{category}/{name}");
        let resolved = {
            let directory = emulator.engine.state::<JobDirectory>();
            let guard = directory.lock();
            guard.get_by_arn(&arn).map(|held| held.arn.clone())
        };
        assert_eq!(
            resolved.is_some(),
            created,
            "case {case:?}: expected the data plane to resolve={created} for {arn}, got \
             {resolved:?}"
        );
        if created {
            assert_eq!(
                resolved,
                Some(format!(
                    "arn:aws:sagemaker:us-east-1:{ACCOUNT}:job/{category}/{name}"
                )),
                "case {case:?}: expected the registration to carry the ARN CreateJob minted"
            );
        }
    }
}

/// RULE: `JobName` is unique to the account and Region, so a second create under a name
/// already held is refused `ResourceInUse` — whatever category it names — and the stored
/// job is left exactly as it was.
#[tokio::test]
async fn a_name_already_held_is_refused_as_in_use() {
    // Rows differ along what the second create changes: nothing, the category, or the
    // configuration document.
    let rows: [(&str, &str, String); 3] = [
        ("the same request again", JOB_CATEGORIES[0], config_document()),
        ("another category", JOB_CATEGORIES[1], config_document()),
        (
            "another configuration",
            JOB_CATEGORIES[0],
            config_document_naming("other"),
        ),
    ];

    for (case, category, document) in rows {
        let emulator = Emulator::new();
        let first = emulator
            .ok("CreateJob", &create_body("rollout-1", JOB_CATEGORIES[0]))
            .await;

        let mut body = create_body("rollout-1", category);
        body["JobConfigDocument"] = json!(document);
        let (status, refused) = emulator.call("CreateJob", &body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ResourceInUse"),
            "case {case:?}: expected 400 ResourceInUse, got {status} with {refused}"
        );

        let described = emulator
            .ok(
                "DescribeJob",
                &json!({ "JobName": "rollout-1", "JobCategory": JOB_CATEGORIES[0] }),
            )
            .await;
        assert_eq!(
            described.get("JobArn"),
            first.get("JobArn"),
            "case {case:?}: expected the first job to stand, got {described}"
        );
        assert_eq!(
            str_field(&described, "JobConfigDocument"),
            config_document(),
            "case {case:?}: expected the refused create to change nothing, got {described}"
        );
    }
}

/// RULE: a request outside what the model declares, or outside a documented rule the
/// model cannot express, is refused `400 ValidationException` naming what is wrong, and
/// writes nothing: an enum-typed member outside its value set (refused at the boundary,
/// with the member path and the value set), a configuration document that is not JSON, a
/// tag key taken twice.
#[tokio::test]
async fn a_request_the_model_cannot_express_is_refused_as_invalid() {
    // Rows differ along WHICH rule the request breaks, and on which operation.
    let rows: [(&str, &str, Value, &str); 5] = [
        (
            "a category of its own",
            "CreateJob",
            {
                let mut body = create_body("rollout-1", "AgentRFTTraining");
                body["JobName"] = json!("rollout-1");
                body
            },
            "at 'jobCategory'",
        ),
        (
            "the right category in the wrong case",
            "CreateJob",
            create_body("rollout-1", "agentrft"),
            "at 'jobCategory'",
        ),
        (
            "a configuration document that is not JSON",
            "CreateJob",
            {
                let mut body = create_body("rollout-1", JOB_CATEGORIES[0]);
                body["JobConfigDocument"] = json!("not a json document");
                body
            },
            "failed schema validation",
        ),
        (
            "a tag key taken twice",
            "CreateJob",
            {
                let mut body = create_body("rollout-1", JOB_CATEGORIES[0]);
                body["Tags"] =
                    json!([{ "Key": "team", "Value": "rl" }, { "Key": "team", "Value": "rl2" }]);
                body
            },
            "Tag keys must be unique",
        ),
        (
            "a category of its own, on the reader",
            "DescribeJob",
            json!({ "JobName": "rollout-1", "JobCategory": "AgentRFTTraining" }),
            "at 'jobCategory'",
        ),
    ];

    for (case, target, body, fragment) in rows {
        let emulator = Emulator::new();
        let (status, refused) = emulator.call(target, &body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException from {target}, got {status} with \
             {refused}"
        );
        assert!(
            message(&refused).contains(fragment),
            "case {case:?}: expected the refusal to state {fragment:?}, got {:?}",
            message(&refused)
        );
        let held = {
            let state = emulator.engine.state::<
                service_com_amazonaws_sagemaker_sagemaker::ComAmazonawsSagemakerSagemakerState,
            >();
            let count = state.lock().jobs.len();
            count
        };
        assert_eq!(
            held, 0,
            "case {case:?}: expected a refused request to store no job, got {held}"
        );
    }
}

/// RULE: a job the account does not hold is reported as not found — including a name held
/// under another category, since the category is part of a job's identity.
#[tokio::test]
async fn a_job_the_account_does_not_hold_is_not_found() {
    let emulator = Emulator::new();
    emulator
        .ok("CreateJob", &create_body("rollout-1", JOB_CATEGORIES[0]))
        .await;

    // Rows differ along WHY the pair resolves to nothing: the name was never created, it
    // differs in case, or it is held under the other category.
    let rows: [(&str, &str, &str); 3] = [
        ("a name never created", "rollout-2", JOB_CATEGORIES[0]),
        ("the created name in another case", "ROLLOUT-1", JOB_CATEGORIES[0]),
        (
            "the created name under the other category",
            "rollout-1",
            JOB_CATEGORIES[1],
        ),
    ];

    for (case, name, category) in rows {
        let (status, refused) = emulator
            .call(
                "DescribeJob",
                &json!({ "JobName": name, "JobCategory": category }),
            )
            .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ResourceNotFound"),
            "case {case:?}: expected 400 ResourceNotFound, got {status} with {refused}"
        );
        assert!(
            message(&refused).contains(name),
            "case {case:?}: expected the refusal to quote the identifier as sent, got {:?}",
            message(&refused)
        );
    }
}
