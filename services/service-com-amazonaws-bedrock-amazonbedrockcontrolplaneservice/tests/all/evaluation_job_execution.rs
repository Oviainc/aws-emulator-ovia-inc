// SPDX-License-Identifier: CC0-1.0

//! What an evaluation job DOES, asserted end to end with this emulator's own Amazon S3 and
//! its Bedrock Runtime inference plane mounted beside this control plane on one engine — the
//! way the assembled emulator runs them, and the way the real service takes a job's
//! configuration on.
//!
//! THE RULE UNDER TEST: an evaluation job is not a status record. The RESULT of an
//! evaluation is its S3 output — `outputDataConfig` is "the Amazon S3 location where the
//! results of your evaluation job are saved" and its `s3Uri` is "the Amazon S3 URI where the
//! results of the evaluation job are saved"
//! (`smithy_query(query="shape",shape="EvaluationOutputDataConfig")`) — and the `automated`
//! configuration's `datasetMetricConfigs` say WHAT is to be run and scored: a task type, a
//! dataset and the metric names
//! (`smithy_query(query="shape",shape="AutomatedEvaluationConfig,EvaluationDatasetMetricConfig")`).
//! So a job this emulator carries into a terminal status has read the records of every
//! dataset it names, has invoked each model its `inferenceConfig` names on each record, has
//! scored the metrics it was configured with, and has saved those records under its output
//! prefix. The guardrail the ACCOUNT enforces on the evaluated model is applied to that
//! inference, so a record the guardrail intervened on carries the guardrail's configured
//! `blockedOutputsMessaging` verbatim as the response that was scored. A job whose dataset
//! or output location cannot be used reaches the modelled `Failed` with `failureMessages`
//! instead of reporting `Completed` with nothing saved.
//!
//! Each half is asserted over a TABLE whose rows differ along what the rule generalises
//! over: which dataset the job names (two different built-in ones, the caller's own file,
//! two datasets in one job), the task type, how many models are evaluated and how they are
//! spelled (a bare model id, a foundation-model ARN), the shape of the output location (a
//! prefix, a nested prefix, the bucket itself), which metrics are configured, which models
//! the enforced guardrail covers, and — for the failure half — which part of the
//! configuration cannot be used.
//!
//! The harness mounts THREE services, which is why this suite builds its own rather than
//! driving `crate::support`'s (one service) or the batch inference suite's (two): the rule
//! spans the control plane that holds the job, the inference plane that serves its records
//! and enforces the account's guardrail, and the object store its results are saved in.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice as bedrock;
use smithy_server_runtime::MultiServiceRouter;

use bedrock::state::evaluation_datasets::{builtin_dataset, BuiltinDataset};
use bedrock::state::evaluation_run::METRIC_TOXICITY;

use crate::support::{error_code, label, Reply};

/// The Region every call here is signed for. Amazon S3 places a bucket created with no
/// `LocationConstraint` in us-east-1 ("If you don't specify a Region, the bucket is created
/// in the US East (N. Virginia) Region (us-east-1) by default"), which is also the Region
/// this service serves a request carrying no credential scope in, so one Region label serves
/// every call.
const REGION: &str = "us-east-1";

/// A SigV4 credential scope naming Amazon S3, which is what routes a request to the object
/// store. No authenticator is registered, so the signature is never checked; the scope is
/// what claims the request.
const S3_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/s3/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The bucket every result object below is saved to and every custom dataset is staged in.
const BUCKET: &str = "example-model-evaluation";

/// A bucket no `CreateBucket` below ever creates.
const ABSENT_BUCKET: &str = "example-model-evaluation-missing";

/// A `RoleArn` the model admits (pattern `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/BedrockEvaluationRole";

/// Two models the control plane's catalogue publishes and the inference plane's inventory
/// serves, so a row can evaluate one and enforce a guardrail on the other.
const EVALUATED_MODEL: &str = "amazon.nova-lite-v1:0";
const OTHER_MODEL: &str = "amazon.nova-pro-v1:0";

/// The built-in dataset whose prompts are published as eliciting toxic continuations, and
/// one whose prompts are news articles to summarize: the two ends of what a guardrail has to
/// act on.
const TOXICITY_DATASET: &str = "Builtin.RealToxicityPrompts";
const SUMMARIZATION_DATASET: &str = "Builtin.Gigaword";

/// The metric this emulator computes a score for, and one it does not.
const UNSCORED_METRIC: &str = "Builtin.Accuracy";

/// How long after the create a job's status is owed its move to the terminal status its run
/// ended in, read off the handler's own constant.
const COMPLETE_AFTER: Duration = bedrock::state::EVALUATION_JOB_COMPLETE_AFTER;

/// How long a case polls a job for before it concludes the lifecycle is stuck: generous next
/// to the span above, room for a slow machine rather than for another order of magnitude.
const POLL_BUDGET: Duration = Duration::from_secs(45);

/// How long a case waits between two reads of a job while it polls it.
const POLL_STEP: Duration = Duration::from_millis(100);

/// What the fixture's guardrail answers a blocked response with: the caller's own configured
/// text, which is the string an intervened record must carry verbatim.
const BLOCKED_OUTPUT: &str = "This response was blocked by the account guardrail.";

/// What it answers a blocked input with, which no result record carries.
const BLOCKED_INPUT: &str = "This request was blocked by the account guardrail.";

/// The URI `PutEnforcedGuardrailConfiguration` is bound to.
const CONFIGURATION_URI: &str = "/enforcedGuardrailsConfiguration";

// ---------------------------------------------------------------------------------------
// The harness: this control plane, the inference plane and Amazon S3 on one shared engine.
// ---------------------------------------------------------------------------------------

struct Emulator {
    router: MultiServiceRouter,
}

fn emulator() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(bedrock::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        ::service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    Emulator { router }
}

impl Emulator {
    /// One control-plane call, signed for [`REGION`] the way an SDK signs it.
    async fn bedrock(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(&value).expect("a serializable request body")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://bedrock.{REGION}.amazonaws.com{path}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock/\
                     aws4_request, SignedHeaders=host, Signature=00"
                ),
            )
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let header_error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(|value| error_code(value).to_string());
        let body: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        let error = header_error.or_else(|| {
            body.get("__type")
                .and_then(Value::as_str)
                .map(|value| error_code(value).to_string())
        });
        Reply {
            status,
            content_type,
            body,
            error,
        }
    }

    /// One path-style request to the emulator's Amazon S3 front door: the bucket and the key
    /// live in the URI and the body is the object's own bytes, so this is a raw request
    /// rather than a modelled call. The credential scope names `s3`, which is what the
    /// router claims on.
    async fn s3(&self, method: &str, path: &str, body: &[u8]) -> (u16, bytes::Bytes) {
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://s3.{REGION}.amazonaws.com{path}"))
            .header("host", format!("s3.{REGION}.amazonaws.com"))
            .header("authorization", S3_AUTHORIZATION)
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        (response.status().as_u16(), response.body().clone())
    }
}

/// `PUT /<bucket>` — `CreateBucket` — so the results have a bucket to be saved in.
async fn given_bucket(emu: &Emulator) {
    let (status, body) = emu.s3("PUT", &format!("/{BUCKET}"), &[]).await;
    assert_eq!(
        status,
        200,
        "the bucket must be created first, got {status} with body {:?}",
        String::from_utf8_lossy(&body)
    );
}

/// `PUT /<bucket>/<key>` of a custom prompt dataset: one JSON object per line carrying
/// `prompt`, which is the documented custom prompt-dataset form.
async fn given_dataset_object(emu: &Emulator, key: &str, prompts: &[&str]) {
    let file: String = prompts
        .iter()
        .map(|prompt| format!("{}\n", json!({"prompt": prompt})))
        .collect();
    let (status, body) = emu
        .s3("PUT", &format!("/{BUCKET}/{key}"), file.as_bytes())
        .await;
    assert_eq!(
        status,
        200,
        "the dataset object {BUCKET}/{key} must be stored, got {status} with body {:?}",
        String::from_utf8_lossy(&body)
    );
}

// ---------------------------------------------------------------------------------------
// Requests.
// ---------------------------------------------------------------------------------------

/// One `EvaluationDatasetMetricConfig`: the task type, the dataset (a built-in name, or a
/// name with the `datasetLocation` of the caller's own file) and the metric names.
fn dataset_metric_config(
    task_type: &str,
    dataset: &str,
    location: Option<&str>,
    metrics: &[&str],
) -> Value {
    let mut dataset = json!({"name": dataset});
    if let Some(uri) = location {
        dataset["datasetLocation"] = json!({"s3Uri": uri});
    }
    json!({
        "taskType": task_type,
        "dataset": dataset,
        "metricNames": metrics,
    })
}

/// `CreateEvaluationJob` with the four `@required` members, answering the `jobArn`.
async fn submit(
    emu: &Emulator,
    case: &str,
    name: &str,
    datasets: Vec<Value>,
    model_ids: &[&str],
    output_uri: &str,
) -> String {
    let models: Vec<Value> = model_ids
        .iter()
        .map(|model_id| json!({"bedrockModel": {"modelIdentifier": model_id}}))
        .collect();
    let reply = emu
        .bedrock(
            "POST",
            "/evaluation-jobs",
            Some(json!({
                "jobName": name,
                "roleArn": ROLE_ARN,
                "outputDataConfig": {"s3Uri": output_uri},
                "evaluationConfig": {"automated": {"datasetMetricConfigs": datasets}},
                "inferenceConfig": {"models": models},
            })),
        )
        .await;
    assert_eq!(
        reply.status, 202,
        "case {case:?}: the create must be accepted, got {} {:?}",
        reply.status, reply.body
    );
    reply.text("jobArn")
}

/// `GetEvaluationJob` on the job `arn` names.
async fn read_job(emu: &Emulator, arn: &str) -> Reply {
    emu.bedrock("GET", &format!("/evaluation-jobs/{}", label(arn)), None)
        .await
}

/// Poll `GetEvaluationJob` the way a client does until the job reports `target`, and hand
/// back the read that first reported it.
async fn poll_until_status(emu: &Emulator, case: &str, arn: &str, target: &str) -> Reply {
    let started = ::std::time::Instant::now();
    let mut seen: Vec<String> = Vec::new();
    loop {
        let reply = read_job(emu, arn).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: the job must stay readable while it is polled, got {} {:?}",
            reply.status, reply.body
        );
        let status = reply.text("status");
        if status == target {
            return reply;
        }
        if seen.last() != Some(&status) {
            seen.push(status);
        }
        assert!(
            started.elapsed() < POLL_BUDGET,
            "case {case:?}: a client polling the job must read it {target} inside \
             {POLL_BUDGET:?} (the run span is {COMPLETE_AFTER:?}), saw {seen:?}"
        );
        ::tokio::time::sleep(POLL_STEP).await;
    }
}

/// The minted job id an `EvaluationJobArn` carries, which is part of where its results live.
fn job_id(arn: &str) -> &str {
    arn.rsplit('/').next().expect("an ARN has a last segment")
}

/// The key the result file of one model, task type and dataset is saved under, built the way
/// a reader of the output builds it: the prefix the submission named, then the job's own
/// name and id, then the model, task type and dataset the file is for.
fn result_key(
    output_suffix: &str,
    job_name: &str,
    arn: &str,
    model_id: &str,
    task_type: &str,
    dataset: &str,
) -> String {
    let prefix = output_suffix.trim_matches('/');
    let path = format!(
        "{job_name}/{}/models/{model_id}/taskTypes/{task_type}/datasets/{dataset}/output.jsonl",
        job_id(arn)
    );
    if prefix.is_empty() {
        path
    } else {
        format!("{prefix}/{path}")
    }
}

/// The model id an `EvaluationBedrockModel$modelIdentifier` RESOLVES to: a result file is
/// named for the model the inference ran on, and the record names that model, so an
/// identifier spelled as a `foundation-model` ARN and the bare id of the same model are one
/// model in the output rather than two.
fn resolved_model_id(model_identifier: &str) -> &str {
    model_identifier
        .rsplit_once("foundation-model/")
        .map_or(model_identifier, |(_, model_id)| model_id)
}

/// The records of the result file at `key`, each parsed as the JSON object a line is.
async fn result_records(emu: &Emulator, case: &str, key: &str) -> Vec<Value> {
    let (status, body) = emu.s3("GET", &format!("/{BUCKET}/{key}"), &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the results must be saved at {BUCKET}/{key}, got {status} with body {:?}",
        String::from_utf8_lossy(&body)
    );
    let text = String::from_utf8(body.to_vec()).expect("the results are UTF-8 text");
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("case {case:?}: {line:?} is not JSON: {error}"))
        })
        .collect()
}

/// The built-in dataset `name` addresses, read out of the catalogue the run itself reads, so
/// this suite asserts about the records the job actually had rather than about a copy of
/// them.
fn builtin(name: &str) -> &'static BuiltinDataset {
    builtin_dataset(name).unwrap_or_else(|| panic!("{name} must be a built-in dataset"))
}

/// The prompts of a built-in dataset, in the order the run reads them.
fn builtin_prompts(name: &str) -> Vec<&'static str> {
    builtin(name)
        .records
        .iter()
        .map(|(prompt, _, _)| *prompt)
        .collect()
}

// ---------------------------------------------------------------------------------------
// 1. A job that ran has its results under its output prefix.
// ---------------------------------------------------------------------------------------

/// One row of the run table.
struct Row {
    case: &'static str,
    /// `jobName`, which is also part of where the results are saved.
    name: &'static str,
    /// The `taskType`, dataset name, custom dataset key and metrics of each
    /// `datasetMetricConfigs` entry.
    datasets: Vec<(&'static str, &'static str, Option<&'static str>, Vec<&'static str>)>,
    /// The `modelIdentifier` of each model the job evaluates.
    model_ids: Vec<&'static str>,
    /// The `outputDataConfig.s3Uri` suffix after the bucket: a prefix, a nested prefix, or
    /// nothing at all (the bucket itself).
    output_suffix: &'static str,
}

fn run_rows() -> Vec<Row> {
    vec![
        Row {
            case: "the toxicity dataset over one model, under a prefix",
            name: "eval-toxicity",
            datasets: vec![(
                "Generation",
                TOXICITY_DATASET,
                None,
                vec![METRIC_TOXICITY],
            )],
            model_ids: vec![EVALUATED_MODEL],
            output_suffix: "/eval-output",
        },
        Row {
            case: "a summarization dataset over a model named by its ARN, under a nested \
                   prefix",
            name: "eval-summaries",
            datasets: vec![(
                "Summarization",
                SUMMARIZATION_DATASET,
                None,
                vec![METRIC_TOXICITY, UNSCORED_METRIC],
            )],
            model_ids: vec![
                "arn:aws:bedrock:us-east-1::foundation-model/amazon.nova-lite-v1:0",
            ],
            output_suffix: "/reports/2026/q1",
        },
        Row {
            case: "the caller's own prompt file, under the bucket itself",
            name: "eval-own-prompts",
            datasets: vec![(
                "QuestionAndAnswer",
                "own-questions",
                Some("s3://example-model-evaluation/datasets/questions.jsonl"),
                vec![METRIC_TOXICITY],
            )],
            model_ids: vec![EVALUATED_MODEL],
            output_suffix: "",
        },
        Row {
            case: "two datasets over one model, the modelled maximum of the dataset list is \
                   five",
            name: "eval-two-datasets",
            datasets: vec![
                ("Generation", TOXICITY_DATASET, None, vec![METRIC_TOXICITY]),
                (
                    "Summarization",
                    SUMMARIZATION_DATASET,
                    None,
                    vec![UNSCORED_METRIC],
                ),
            ],
            model_ids: vec![OTHER_MODEL],
            output_suffix: "/eval-output",
        },
    ]
}

/// The prompts the caller's own dataset object carries, staged by the row that names it.
const OWN_PROMPTS: &[&str] = &[
    "How many days are in a leap year?",
    "Name the largest ocean on Earth.",
    "What is the boiling point of water at sea level?",
];

#[tokio::test]
async fn an_evaluation_job_evaluates_its_datasets_and_saves_the_results() {
    let emu = emulator();
    given_bucket(&emu).await;
    given_dataset_object(&emu, "datasets/questions.jsonl", OWN_PROMPTS).await;

    for row in run_rows() {
        let case = row.case;
        let output_uri = format!("s3://{BUCKET}{}", row.output_suffix);
        let datasets: Vec<Value> = row
            .datasets
            .iter()
            .map(|(task_type, dataset, location, metrics)| {
                dataset_metric_config(task_type, dataset, *location, metrics)
            })
            .collect();
        let arn = submit(&emu, case, row.name, datasets, &row.model_ids, &output_uri).await;

        let completed = poll_until_status(&emu, case, &arn, "Completed").await;
        assert_eq!(
            completed.get("failureMessages"),
            Value::Null,
            "case {case:?}: a job that ran reports no failureMessages, got {:?}",
            completed.body
        );

        // One result file per model, task type and dataset, each holding one record per
        // record of that dataset.
        for model_id in &row.model_ids {
            let model_id = resolved_model_id(model_id);
            for (task_type, dataset, location, metrics) in &row.datasets {
                let prompts: Vec<&str> = match location {
                    Some(_) => OWN_PROMPTS.to_vec(),
                    None => builtin_prompts(dataset),
                };
                let key = result_key(row.output_suffix, row.name, &arn, model_id, task_type, dataset);
                let records = result_records(&emu, case, &key).await;
                assert_eq!(
                    records.len(),
                    prompts.len(),
                    "case {case:?}: one result record per dataset record at {key}, expected {}, \
                     got {}",
                    prompts.len(),
                    records.len()
                );
                for (position, record) in records.iter().enumerate() {
                    let prompt = record
                        .pointer("/inputRecord/prompt")
                        .and_then(Value::as_str)
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: record {position} carries no prompt: {record}")
                        });
                    assert_eq!(
                        prompt, prompts[position],
                        "case {case:?}: record {position} is the dataset's own record, expected \
                         {:?}, got {prompt:?}",
                        prompts[position]
                    );
                    let response = record
                        .pointer("/modelResponses/0/response")
                        .and_then(Value::as_str)
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: record {position} carries no response: {record}")
                        });
                    assert!(
                        !response.is_empty() && response != prompt,
                        "case {case:?}: record {position} carries what the inference answered, \
                         not its own prompt echoed back, got {response:?}"
                    );
                    assert_eq!(
                        record
                            .pointer("/modelResponses/0/modelIdentifier")
                            .and_then(Value::as_str),
                        Some(model_id),
                        "case {case:?}: record {position} names the model it was evaluated on, \
                         got {record}"
                    );

                    // One score per configured metric this emulator computes, and one named
                    // entry per metric it does not: a reader can tell a metric that scored
                    // zero from one that was never scored.
                    let scores = record
                        .pointer("/automatedEvaluationResult/scores")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    let computed: Vec<&str> = metrics
                        .iter()
                        .copied()
                        .filter(|metric| *metric == METRIC_TOXICITY)
                        .collect();
                    let scored: Vec<&str> = scores
                        .iter()
                        .filter_map(|score| score.get("metricName").and_then(Value::as_str))
                        .collect();
                    assert_eq!(
                        scored, computed,
                        "case {case:?}: record {position} scores the configured metrics this \
                         emulator computes, expected {computed:?}, got {scored:?}"
                    );
                    for score in &scores {
                        let result = score.get("result").and_then(Value::as_f64);
                        assert!(
                            result.is_some_and(|result| (0.0..=1.0).contains(&result)),
                            "case {case:?}: record {position} scores in 0..1, got {score}"
                        );
                    }
                    let unscored: Vec<&str> = record
                        .get("unscoredMetrics")
                        .and_then(Value::as_array)
                        .map(|entries| {
                            entries
                                .iter()
                                .filter_map(|entry| {
                                    entry.get("metricName").and_then(Value::as_str)
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let expected_unscored: Vec<&str> = metrics
                        .iter()
                        .copied()
                        .filter(|metric| *metric != METRIC_TOXICITY)
                        .collect();
                    assert_eq!(
                        unscored, expected_unscored,
                        "case {case:?}: record {position} names the metrics no value was \
                         computed for, expected {expected_unscored:?}, got {unscored:?}"
                    );
                }
            }
        }
    }
}

/// RULE: the run is owed by the CREATE and is performed once, so reading the job again
/// reports the same job and leaves the results as the first reader found them.
#[tokio::test]
async fn a_read_reports_the_run_rather_than_performing_it_again() {
    let emu = emulator();
    given_bucket(&emu).await;
    let case = "one dataset over one model";
    let arn = submit(
        &emu,
        case,
        "eval-read-twice",
        vec![dataset_metric_config(
            "Generation",
            TOXICITY_DATASET,
            None,
            &[METRIC_TOXICITY],
        )],
        &[EVALUATED_MODEL],
        &format!("s3://{BUCKET}/eval-output"),
    )
    .await;

    let settled = poll_until_status(&emu, case, &arn, "Completed").await;
    let key = result_key(
        "/eval-output",
        "eval-read-twice",
        &arn,
        EVALUATED_MODEL,
        "Generation",
        TOXICITY_DATASET,
    );
    let (_, first) = emu.s3("GET", &format!("/{BUCKET}/{key}"), &[]).await;

    let again = read_job(&emu, &arn).await;
    assert_eq!(
        again.body, settled.body,
        "case {case:?}: a read reports the job, it does not move it on, got {:?} then {:?}",
        settled.body, again.body
    );
    let (_, second) = emu.s3("GET", &format!("/{BUCKET}/{key}"), &[]).await;
    assert_eq!(
        first, second,
        "case {case:?}: the run is performed once, so what a client fetched out of the output \
         location stays what it fetched"
    );
}

// ---------------------------------------------------------------------------------------
// 2. The account's enforced guardrail is applied to the job's inference.
// ---------------------------------------------------------------------------------------

/// A guardrail whose content policy filters every category it can at the strongest strength,
/// with one numbered version: the id and the version an enforced configuration names.
///
/// `GuardrailContentFilterType` is `[HATE, INSULTS, MISCONDUCT, PROMPT_ATTACK, SEXUAL,
/// VIOLENCE]` and `GuardrailFilterStrength` is `[NONE, LOW, MEDIUM, HIGH]`
/// (`smithy_query(query="shape",shape="GuardrailContentFilterType,GuardrailFilterStrength")`);
/// `PROMPT_ATTACK` is filtered on the input alone, which is why it carries `NONE` on the
/// output side.
async fn guardrail_filtering_content(emu: &Emulator, name: &str) -> (String, String) {
    let filter = |category: &str, output: &str| {
        json!({"type": category, "inputStrength": "HIGH", "outputStrength": output})
    };
    let created = emu
        .bedrock(
            "POST",
            "/guardrails",
            Some(json!({
                "name": name,
                "blockedInputMessaging": BLOCKED_INPUT,
                "blockedOutputsMessaging": BLOCKED_OUTPUT,
                "contentPolicyConfig": {"filtersConfig": [
                    filter("HATE", "HIGH"),
                    filter("INSULTS", "HIGH"),
                    filter("SEXUAL", "HIGH"),
                    filter("VIOLENCE", "HIGH"),
                    filter("MISCONDUCT", "HIGH"),
                    filter("PROMPT_ATTACK", "NONE"),
                ]},
            })),
        )
        .await;
    assert_eq!(
        created.status, 202,
        "case {name}: CreateGuardrail must answer 202, got {} {:?}",
        created.status, created.body
    );
    let id = created.text("guardrailId");
    let cut = emu
        .bedrock("POST", &format!("/guardrails/{id}"), Some(json!({})))
        .await;
    assert_eq!(
        cut.status, 202,
        "case {name}: CreateGuardrailVersion must answer 202, got {} {:?}",
        cut.status, cut.body
    );
    (id, cut.text("version"))
}

/// `PUT /enforcedGuardrailsConfiguration` installing `id` at `version` over `included`.
async fn install_enforcement(
    emu: &Emulator,
    case: &str,
    id: &str,
    version: &str,
    included: &[&str],
) {
    let installed = emu
        .bedrock(
            "PUT",
            CONFIGURATION_URI,
            Some(json!({"guardrailInferenceConfig": {
                "guardrailIdentifier": id,
                "guardrailVersion": version,
                "modelEnforcement": {"includedModels": included, "excludedModels": []},
            }})),
        )
        .await;
    assert_eq!(
        installed.status, 200,
        "case {case:?}: the put must answer 200, got {} {:?}",
        installed.status, installed.body
    );
}

/// RULE: the guardrail the account enforces on the evaluated model is applied to the job's
/// inference, and the guardrail replaces the model's output for a record it intervened on —
/// so such a record carries the guardrail's configured `blockedOutputsMessaging` verbatim,
/// while a record it passed carries the inference's own response.
///
/// The rows differ along which models the enforcement covers and which dataset the job
/// evaluates: the recorded case — the evaluated model named in `includedModels`, over the
/// dataset published as eliciting toxic continuations — is ONE row.
#[tokio::test]
async fn an_enforced_guardrail_intervention_appears_in_the_results() {
    // (case, the models the enforcement covers, the dataset, whether a record must be
    // blocked)
    let rows: Vec<(&str, Vec<&str>, &str, bool)> = vec![
        (
            "the evaluated model is enforced on, over toxicity-eliciting prompts",
            vec![EVALUATED_MODEL],
            TOXICITY_DATASET,
            true,
        ),
        (
            "every model is enforced on, over toxicity-eliciting prompts",
            vec!["ALL"],
            TOXICITY_DATASET,
            true,
        ),
        (
            "another model is enforced on",
            vec![OTHER_MODEL],
            TOXICITY_DATASET,
            false,
        ),
        (
            "the evaluated model is enforced on, over prompts nothing is placed in",
            vec![EVALUATED_MODEL],
            SUMMARIZATION_DATASET,
            false,
        ),
    ];

    for (index, (case, included, dataset, blocked)) in rows.into_iter().enumerate() {
        // One account per row: an account holds a single enforced configuration, so each row
        // installs its own rather than replacing the row before it.
        let emu = emulator();
        given_bucket(&emu).await;
        let (id, version) =
            guardrail_filtering_content(&emu, &format!("enforced-evaluation-{index}")).await;
        install_enforcement(&emu, case, &id, &version, &included).await;

        let name = format!("eval-guarded-{index}");
        let task_type = "Generation";
        let arn = submit(
            &emu,
            case,
            &name,
            vec![dataset_metric_config(
                task_type,
                dataset,
                None,
                &[METRIC_TOXICITY],
            )],
            &[EVALUATED_MODEL],
            &format!("s3://{BUCKET}/eval-output/"),
        )
        .await;
        poll_until_status(&emu, case, &arn, "Completed").await;

        let key = result_key(
            "/eval-output",
            &name,
            &arn,
            EVALUATED_MODEL,
            task_type,
            dataset,
        );
        let records = result_records(&emu, case, &key).await;
        let responses: Vec<&str> = records
            .iter()
            .filter_map(|record| {
                record
                    .pointer("/modelResponses/0/response")
                    .and_then(Value::as_str)
            })
            .collect();
        assert_eq!(
            responses.len(),
            records.len(),
            "case {case:?}: every record carries a response, got {records:?}"
        );
        let intervened = responses
            .iter()
            .filter(|response| **response == BLOCKED_OUTPUT)
            .count();
        assert_eq!(
            intervened > 0,
            blocked,
            "case {case:?}: expected a blocked record={blocked}, got the responses {responses:?}"
        );
        if blocked {
            // The dataset's own benign record is passed, so an intervention is the
            // guardrail acting on a record rather than on the whole run.
            assert!(
                intervened < responses.len(),
                "case {case:?}: a record the guardrail passed carries the inference's own \
                 response, got {responses:?}"
            );
        }
        assert!(
            !responses.contains(&BLOCKED_INPUT),
            "case {case:?}: an evaluated record carries the guardrail's blockedOutputsMessaging, \
             never its blockedInputMessaging, got {responses:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// 3. A job whose configuration cannot be run fails with its failureMessages.
// ---------------------------------------------------------------------------------------

/// RULE: a job whose dataset or output location cannot be used reaches the modelled `Failed`
/// with `failureMessages` saying why, and saves no results — rather than reporting
/// `Completed` with nothing done.
///
/// The rows differ along which part of the configuration cannot be used.
#[tokio::test]
async fn a_job_that_cannot_be_run_fails_with_failure_messages() {
    let emu = emulator();
    given_bucket(&emu).await;

    // (case, the job name, the dataset entry, the output location, what the failure names)
    let rows: Vec<(&str, &str, Value, String, String)> = vec![
        (
            "an output location in a bucket the object store holds nothing of",
            "eval-absent-bucket",
            dataset_metric_config("Generation", TOXICITY_DATASET, None, &[METRIC_TOXICITY]),
            format!("s3://{ABSENT_BUCKET}/eval-output/"),
            format!("s3://{ABSENT_BUCKET}/eval-output/"),
        ),
        (
            "a built-in dataset this emulator does not serve",
            "eval-unknown-dataset",
            dataset_metric_config("Generation", "Builtin.NotADataset", None, &[METRIC_TOXICITY]),
            format!("s3://{BUCKET}/eval-output/"),
            "Builtin.NotADataset".to_string(),
        ),
        (
            "a dataset named by neither a built-in name nor a location",
            "eval-unlocated-dataset",
            dataset_metric_config("Generation", "own-questions", None, &[METRIC_TOXICITY]),
            format!("s3://{BUCKET}/eval-output/"),
            "own-questions".to_string(),
        ),
        (
            "a dataset object the object store holds nothing at",
            "eval-absent-dataset",
            dataset_metric_config(
                "Generation",
                "own-questions",
                Some("s3://example-model-evaluation/datasets/absent.jsonl"),
                &[METRIC_TOXICITY],
            ),
            format!("s3://{BUCKET}/eval-output/"),
            "s3://example-model-evaluation/datasets/absent.jsonl".to_string(),
        ),
    ];

    for (case, name, dataset, output_uri, named) in rows {
        let model_ids: &[&str] = &[EVALUATED_MODEL];
        let arn = submit(&emu, case, name, vec![dataset], model_ids, &output_uri).await;
        let failed = poll_until_status(&emu, case, &arn, "Failed").await;

        let messages: Vec<String> = failed
            .get("failureMessages")
            .as_array()
            .map(|messages| {
                messages
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            !messages.is_empty(),
            "case {case:?}: a job that failed says why, got {:?}",
            failed.body
        );
        assert!(
            named.is_empty() || messages.iter().any(|message| message.contains(&named)),
            "case {case:?}: the failure names the value it is about ({named:?}), got {messages:?}"
        );

        // Nothing was saved for a job that failed before any record could be evaluated: the
        // key its results would have been saved under holds nothing.
        let key = result_key(
            "/eval-output",
            name,
            &arn,
            model_ids[0],
            "Generation",
            TOXICITY_DATASET,
        );
        let (status, _) = emu.s3("GET", &format!("/{BUCKET}/{key}"), &[]).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a job that failed saves no results, got {status} at {BUCKET}/{key}"
        );
    }
}

/// RULE: a record whose model this emulator serves no inference for is reported as a record
/// that could not be evaluated — in the results, beside the records that were — and a job NO
/// record of which could be evaluated reaches `Failed` with what the inference plane answered
/// about its model.
#[tokio::test]
async fn a_record_whose_model_serves_no_inference_is_reported_as_an_error() {
    let emu = emulator();
    given_bucket(&emu).await;
    let case = "a model identifier that names no model this emulator serves";
    let model_id = "arn:aws:bedrock:us-east-1::foundation-model/absent.model-v1";
    let name = "eval-absent-model";
    let task_type = "Generation";
    let arn = submit(
        &emu,
        case,
        name,
        vec![dataset_metric_config(
            task_type,
            TOXICITY_DATASET,
            None,
            &[METRIC_TOXICITY],
        )],
        &[model_id],
        &format!("s3://{BUCKET}/eval-output/"),
    )
    .await;
    let failed = poll_until_status(&emu, case, &arn, "Failed").await;
    assert!(
        failed
            .get("failureMessages")
            .as_array()
            .is_some_and(|messages| !messages.is_empty()),
        "case {case:?}: a job no record of which could be evaluated says why, got {:?}",
        failed.body
    );

    let key = result_key("/eval-output", name, &arn, model_id, task_type, TOXICITY_DATASET);
    let records = result_records(&emu, case, &key).await;
    assert_eq!(
        records.len(),
        builtin_prompts(TOXICITY_DATASET).len(),
        "case {case:?}: one result record per dataset record, got {records:?}"
    );
    for (position, record) in records.iter().enumerate() {
        assert!(
            record
                .pointer("/error/errorMessage")
                .and_then(Value::as_str)
                .is_some_and(|message| !message.is_empty()),
            "case {case:?}: record {position} says why it could not be evaluated, got {record}"
        );
        assert_eq!(
            record.pointer("/modelResponses/0/response"),
            None,
            "case {case:?}: record {position} carries no response, since nothing was inferred \
             for it, got {record}"
        );
        assert_eq!(
            record.pointer("/automatedEvaluationResult"),
            None,
            "case {case:?}: record {position} carries no score, since nothing was scored, got \
             {record}"
        );
    }
}
