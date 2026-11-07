// SPDX-License-Identifier: CC0-1.0

//! What a recipe-job RUN does to its dataset, driven end to end with the emulator's own
//! Amazon S3 mounted beside databrew on one engine — the way the assembled emulator runs
//! it, and the way a client stages the data a job reads.
//!
//! THE RULE UNDER TEST: a recipe-job run applies the steps of the recipe version it
//! references to the columns of its dataset's own input, in order; the FIRST step that
//! names a column the data does not have cannot be applied, so the run — which is still
//! admitted, with a `RunId` and exactly one entry in `ListJobRuns` — ends in the terminal
//! `FAILED` state carrying an `ErrorMessage` that opens `RecipeStepError`, names the
//! 1-based index of that step and the column it named, enumerates the columns the data
//! really has, and writes nothing under the job's output prefix. A recipe whose every
//! step names a column the data has (its own, or one an earlier step writes) ends
//! `SUCCEEDED` with no `ErrorMessage`.
//!
//! The columns are never asserted against a recorded answer: each case stages a real CSV
//! object with `PutObject` and the run is judged against the header that object carries,
//! so the table spans what the rule generalises over — which step of the recipe fails
//! (the first, the second, the third), which reference site names the column (an action's
//! `sourceColumn`, its `sourceColumn1`/`sourceColumn2` pair, an entry of its
//! `sourceColumns` array, a condition's `TargetColumn`), how the header is written (the
//! default comma, another delimiter, a quoted field holding the delimiter), how many
//! columns it has, and whether the named column is absent, present, or written by an
//! earlier step.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region both services are addressed in: the one a request carrying no credential
/// scope is served in, and the one Amazon S3 places a bucket created with no
/// `LocationConstraint` in.
const REGION: &str = "us-east-1";

/// The bucket every dataset object and output prefix below lives in.
const BUCKET: &str = "brew-data";

/// Comfortably past the span a run's terminal transition falls due after, so a poll that
/// has waited this long has seen it. Nothing sleeps on the request path, so the wait
/// belongs in the test.
const TERMINAL_POLL_BUDGET: Duration = Duration::from_millis(3_000);

/// How long each poll waits before reading the run again.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The states a run no longer moves out of (model: `JobRunState`).
const TERMINAL_STATES: [&str; 4] = ["SUCCEEDED", "FAILED", "STOPPED", "TIMEOUT"];

/// The role a recipe job assumes, as `CreateRecipeJobRequest$RoleArn` takes it.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/brew";

/// One emulator: databrew and Amazon S3 on a single shared engine behind the production
/// router — what the assembled emulator runs, narrowed to the two services under test.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The signature itself is not what these cases are about; the credential scope still
    // routes each request to the service that serves it.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_databrew_awsgluedatabrew::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 credential scope naming `service`, which is what claims a request.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// One databrew call in this service's own wire envelope (restJson1: the modelled method
/// and URI), answering the status and the decoded body.
async fn brew(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body
        .map(|body| serde_json::to_vec(&body).expect("the test body serializes"))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://databrew.{REGION}.amazonaws.com{uri}"))
        .header("authorization", authorization("databrew"))
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One path-style request to the emulator's Amazon S3 front door: the bucket and the key
/// live in the URI and the body is the object's own bytes, so this is a raw request rather
/// than a modelled call.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str, body: &[u8]) -> (u16, String) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://s3.{REGION}.amazonaws.com{path}"))
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", authorization("s3"))
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("the request builds");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    (status, text)
}

/// `PUT /<bucket>` — `CreateBucket` — so the staged dataset objects have a bucket.
async fn given_bucket(router: &MultiServiceRouter) {
    let (status, body) = s3(router, "PUT", &format!("/{BUCKET}"), &[]).await;
    assert_eq!(
        status, 200,
        "the dataset bucket must be created first, got {status}: {body}"
    );
}

/// `PUT /<bucket>/<key>` — `PutObject` — storing `contents` as the object's own body.
async fn given_object(router: &MultiServiceRouter, case: &str, key: &str, contents: &str) {
    let (status, body) = s3(
        router,
        "PUT",
        &format!("/{BUCKET}/{key}"),
        contents.as_bytes(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the dataset object {key:?} must be stored, got {status}: {body}"
    );
}

/// The keys `ListObjectsV2` reports under `prefix`, read out of the XML listing the store
/// answers with.
async fn keys_under(router: &MultiServiceRouter, prefix: &str) -> Vec<String> {
    let (status, body) = s3(
        router,
        "GET",
        &format!("/{BUCKET}?list-type=2&prefix={prefix}"),
        &[],
    )
    .await;
    assert_eq!(
        status, 200,
        "the output prefix {prefix:?} must be listable, got {status}: {body}"
    );
    body.split("<Key>")
        .skip(1)
        .filter_map(|tail| tail.split_once("</Key>"))
        .map(|(key, _)| key.to_string())
        .collect()
}

/// Read the run until it reaches a terminal state, or until the budget runs out.
async fn await_terminal(
    router: &MultiServiceRouter,
    case: &str,
    job: &str,
    run_id: &str,
) -> Value {
    let mut waited = Duration::ZERO;
    loop {
        let (status, run) = brew(
            router,
            "GET",
            &format!("/jobs/{job}/jobRun/{run_id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the minted RunId must resolve, got {status}: {run}"
        );
        let state = run["State"].as_str().unwrap_or_default().to_string();
        if TERMINAL_STATES.contains(&state.as_str()) {
            return run;
        }
        assert!(
            waited < TERMINAL_POLL_BUDGET,
            "case {case:?}: the run must reach a terminal state within {TERMINAL_POLL_BUDGET:?}, \
             last read {run:?}"
        );
        std::thread::sleep(POLL_INTERVAL);
        waited += POLL_INTERVAL;
    }
}

/// What a case expects of its run once it has settled.
#[derive(Debug)]
enum Expected {
    /// Every step applies: the terminal state is `SUCCEEDED` and nothing diagnoses a step.
    Succeeded,
    /// The step at this 1-based index names this column, which the data does not have.
    FailedAtStep { step: usize, column: &'static str },
}

/// One case: a dataset object, the recipe run against it, and what the run must report.
struct Case {
    /// What the assertions name this case.
    name: &'static str,
    /// The object the dataset is created over, header first.
    object: &'static str,
    /// The `FormatOptions.Csv.Delimiter` the dataset is created with, when it names one.
    delimiter: Option<&'static str>,
    /// The columns the staged header really carries, which a failure must enumerate.
    columns: &'static [&'static str],
    /// The recipe's steps, as `CreateRecipeRequest$Steps` takes them.
    steps: fn() -> Value,
    expected: Expected,
}

/// The table: every row differs from the others in what the rule generalises over.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "a condition's TargetColumn names a column the data lacks",
            object: "sku,qty\nA-1,4\n",
            delimiter: None,
            columns: &["sku", "qty"],
            steps: || {
                json!([{
                    "Action": {"Operation": "REMOVE_VALUES", "Parameters": {"sourceColumn": "dept"}},
                    "ConditionExpressions": [
                        {"Condition": "IS", "Value": "[\"sales\"]", "TargetColumn": "dept"}
                    ],
                }])
            },
            expected: Expected::FailedAtStep {
                step: 1,
                column: "dept",
            },
        },
        Case {
            name: "the SECOND step's sourceColumn names a column the data lacks",
            object: "id,employee_name,dept,salary\n7,ada,sales,10\n",
            delimiter: None,
            columns: &["id", "employee_name", "dept", "salary"],
            steps: || {
                json!([
                    {"Action": {"Operation": "UPPER_CASE", "Parameters": {"sourceColumn": "employee_name"}}},
                    {"Action": {"Operation": "LOWER_CASE", "Parameters": {"sourceColumn": "region"}}},
                ])
            },
            expected: Expected::FailedAtStep {
                step: 2,
                column: "region",
            },
        },
        Case {
            name: "the THIRD step's sourceColumns array holds a column the data lacks",
            object: "alpha,beta\n1,2\n",
            delimiter: None,
            columns: &["alpha", "beta"],
            steps: || {
                json!([
                    {"Action": {"Operation": "UPPER_CASE", "Parameters": {"sourceColumn": "alpha"}}},
                    {"Action": {"Operation": "LOWER_CASE", "Parameters": {"sourceColumn": "beta"}}},
                    {"Action": {"Operation": "DELETE", "Parameters": {"sourceColumns": "[\"alpha\",\"gamma\"]"}}},
                ])
            },
            expected: Expected::FailedAtStep {
                step: 3,
                column: "gamma",
            },
        },
        Case {
            name: "a two-column action names one column the data lacks",
            object: "left,right\nl,r\n",
            delimiter: None,
            columns: &["left", "right"],
            steps: || {
                json!([{"Action": {
                    "Operation": "MERGE",
                    "Parameters": {"sourceColumn1": "left", "sourceColumn2": "middle"},
                }}])
            },
            expected: Expected::FailedAtStep {
                step: 1,
                column: "middle",
            },
        },
        Case {
            name: "a semicolon-delimited header the recipe matches",
            object: "sku;qty\nA-1;4\n",
            delimiter: Some(";"),
            columns: &["sku", "qty"],
            steps: || {
                json!([{
                    "Action": {"Operation": "REMOVE_VALUES", "Parameters": {"sourceColumn": "qty"}},
                    "ConditionExpressions": [
                        {"Condition": "IS", "Value": "[\"0\"]", "TargetColumn": "qty"}
                    ],
                }])
            },
            expected: Expected::Succeeded,
        },
        Case {
            name: "a pipe-delimited header the recipe does not match",
            object: "one|two|three|four|five\n1|2|3|4|5\n",
            delimiter: Some("|"),
            columns: &["one", "two", "three", "four", "five"],
            steps: || {
                json!([{"Action": {"Operation": "UPPER_CASE", "Parameters": {"sourceColumn": "six"}}}])
            },
            expected: Expected::FailedAtStep {
                step: 1,
                column: "six",
            },
        },
        Case {
            name: "a step reads the column an earlier step writes",
            object: "first,last\nada,lovelace\n",
            delimiter: None,
            columns: &["first", "last"],
            steps: || {
                json!([
                    {"Action": {
                        "Operation": "MERGE",
                        "Parameters": {"sourceColumns": "[\"first\",\"last\"]", "targetColumn": "full"},
                    }},
                    {"Action": {"Operation": "UPPER_CASE", "Parameters": {"sourceColumn": "full"}}},
                ])
            },
            expected: Expected::Succeeded,
        },
        Case {
            name: "a quoted header field carrying the delimiter is one column",
            object: "\"sku, code\",qty\n\"A, 1\",4\n",
            delimiter: None,
            columns: &["sku, code", "qty"],
            steps: || {
                json!([{"Action": {"Operation": "UPPER_CASE", "Parameters": {"sourceColumn": "sku, code"}}}])
            },
            expected: Expected::Succeeded,
        },
        Case {
            name: "a recipe with no steps applies to any header",
            object: "only\n1\n",
            delimiter: None,
            columns: &["only"],
            steps: || json!([]),
            expected: Expected::Succeeded,
        },
    ]
}

#[tokio::test]
async fn a_recipe_job_run_is_judged_against_its_datasets_own_columns() {
    let router = emulator();
    given_bucket(&router).await;

    for (index, case) in cases().iter().enumerate() {
        let name = case.name;
        let suffix = format!("{index}");
        let key = format!("in/data-{suffix}.csv");
        let dataset = format!("ds-{suffix}");
        let recipe = format!("rc-{suffix}");
        let job = format!("job-{suffix}");
        let prefix = format!("out-{suffix}/");

        given_object(&router, name, &key, case.object).await;

        let mut format_options = json!({"Csv": {"HeaderRow": true}});
        if let Some(delimiter) = case.delimiter {
            format_options["Csv"]["Delimiter"] = json!(delimiter);
        }
        let (status, body) = brew(
            &router,
            "POST",
            "/datasets",
            Some(json!({
                "Name": dataset,
                "Format": "CSV",
                "FormatOptions": format_options,
                "Input": {"S3InputDefinition": {"Bucket": BUCKET, "Key": key}},
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: the dataset must be created, got {status}: {body}"
        );

        let (status, body) = brew(
            &router,
            "POST",
            "/recipes",
            Some(json!({"Name": recipe, "Steps": (case.steps)()})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: the recipe must be created, got {status}: {body}"
        );
        let (status, body) = brew(
            &router,
            "POST",
            &format!("/recipes/{recipe}/publishRecipe"),
            Some(json!({})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: the recipe must publish, got {status}: {body}"
        );
        let (status, published) = brew(&router, "GET", &format!("/recipes/{recipe}"), None).await;
        assert_eq!(
            status, 200,
            "case {name:?}: the published recipe must describe, got {status}: {published}"
        );
        let version = published["RecipeVersion"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !version.is_empty(),
            "case {name:?}: the publish must report the frozen version, got {published:?}"
        );

        let (status, body) = brew(
            &router,
            "POST",
            "/recipeJobs",
            Some(json!({
                "Name": job,
                "RoleArn": ROLE_ARN,
                "DatasetName": dataset,
                "LogSubscription": "DISABLE",
                "Timeout": 30,
                "MaxRetries": 0,
                "RecipeReference": {"Name": recipe, "RecipeVersion": version},
                "Outputs": [{
                    "Format": "CSV",
                    "Location": {"Bucket": BUCKET, "Key": prefix},
                    "Overwrite": true,
                    "MaxOutputFiles": 1,
                }],
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: the recipe job must be created, got {status}: {body}"
        );

        // A schema mismatch is NOT a refusal at the door: the start is accepted and the
        // run is admitted whichever way its execution is going to end.
        let (status, started) = brew(&router, "POST", &format!("/jobs/{job}/startJobRun"), None).await;
        assert_eq!(
            status, 200,
            "case {name:?}: StartJobRun must be accepted, got {status}: {started}"
        );
        let run_id = started["RunId"].as_str().unwrap_or_default().to_string();
        assert!(
            !run_id.is_empty(),
            "case {name:?}: StartJobRun must mint a RunId, got {started:?}"
        );

        let run = await_terminal(&router, name, &job, &run_id).await;
        let state = run["State"].as_str().unwrap_or_default().to_string();
        let error_message = run["ErrorMessage"].as_str().unwrap_or_default().to_string();

        match case.expected {
            Expected::Succeeded => {
                assert_eq!(
                    state, "SUCCEEDED",
                    "case {name:?}: every step names a column the data has, so the run must \
                     end SUCCEEDED, got {state:?} with ErrorMessage {error_message:?}"
                );
                assert!(
                    run.get("ErrorMessage").is_none(),
                    "case {name:?}: a run that applied its recipe reports no ErrorMessage, \
                     got {:?}",
                    run["ErrorMessage"]
                );
            }
            Expected::FailedAtStep { step, column } => {
                assert_eq!(
                    state, "FAILED",
                    "case {name:?}: step {step} names the absent column {column:?}, so the run \
                     must end FAILED, got {state:?} with ErrorMessage {error_message:?}"
                );
                assert!(
                    error_message.starts_with("RecipeStepError"),
                    "case {name:?}: the diagnosis must open RecipeStepError, got \
                     {error_message:?}"
                );
                assert!(
                    error_message.contains(&format!("Failed at step {step}")),
                    "case {name:?}: the diagnosis must name step {step}, got {error_message:?}"
                );
                assert!(
                    error_message.contains(&format!("Column '{column}' does not exist")),
                    "case {name:?}: the diagnosis must name the absent column {column:?}, got \
                     {error_message:?}"
                );
                for held in case.columns {
                    assert!(
                        error_message.contains(held),
                        "case {name:?}: the diagnosis must enumerate the dataset's own column \
                         {held:?}, got {error_message:?}"
                    );
                }
                let written = keys_under(&router, &prefix).await;
                assert!(
                    written.is_empty(),
                    "case {name:?}: a FAILED run writes nothing under {prefix:?}, got {written:?}"
                );
            }
        }

        // The run was admitted exactly once, whichever way it ended.
        let (status, listed) = brew(&router, "GET", &format!("/jobs/{job}/jobRuns"), None).await;
        assert_eq!(
            status, 200,
            "case {name:?}: ListJobRuns must answer 200, got {status}: {listed}"
        );
        let reported: Vec<String> = listed["JobRuns"]
            .as_array()
            .map(|runs| {
                runs.iter()
                    .map(|run| run["RunId"].as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            reported,
            vec![run_id.clone()],
            "case {name:?}: ListJobRuns must report exactly the one admitted run, got \
             {reported:?}"
        );
        assert_eq!(
            listed["JobRuns"][0]["State"],
            json!(state),
            "case {name:?}: ListJobRuns must report the same state DescribeJobRun does, got \
             {:?}",
            listed["JobRuns"][0]
        );
        assert_eq!(
            listed["JobRuns"][0]["ErrorMessage"]
                .as_str()
                .unwrap_or_default(),
            error_message,
            "case {name:?}: ListJobRuns must report the same diagnosis DescribeJobRun does, \
             got {:?}",
            listed["JobRuns"][0]
        );
    }
}

/// The dataset a job reads is the one the JOB names: two jobs over one recipe and two
/// datasets are judged separately, so the same recipe fails against the dataset that
/// lacks its column and applies to the one that has it.
#[tokio::test]
async fn one_recipe_is_judged_against_each_dataset_it_is_run_over() {
    let router = emulator();
    given_bucket(&router).await;

    let case = "one recipe over two datasets";
    given_object(
        &router,
        case,
        "in/people.csv",
        "id,employee_name,dept,salary\n7,ada,sales,10\n",
    )
    .await;
    given_object(&router, case, "in/parts.csv", "sku,qty\nA-1,4\n").await;

    for (dataset, key) in [("ds-ok", "in/people.csv"), ("ds-bad", "in/parts.csv")] {
        let (status, body) = brew(
            &router,
            "POST",
            "/datasets",
            Some(json!({
                "Name": dataset,
                "Format": "CSV",
                "FormatOptions": {"Csv": {"Delimiter": ",", "HeaderRow": true}},
                "Input": {"S3InputDefinition": {"Bucket": BUCKET, "Key": key}},
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: dataset {dataset:?} must be created, got {status}: {body}"
        );
    }

    let (status, body) = brew(
        &router,
        "POST",
        "/recipes",
        Some(json!({
            "Name": "rc-shared",
            "Steps": [{
                "Action": {"Operation": "REMOVE_VALUES", "Parameters": {"sourceColumn": "dept"}},
                "ConditionExpressions": [
                    {"Condition": "IS", "Value": "[\"sales\"]", "TargetColumn": "dept"}
                ],
            }],
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the shared recipe must be created, got {status}: {body}"
    );

    for (dataset, expected) in [("ds-ok", "SUCCEEDED"), ("ds-bad", "FAILED")] {
        let job = format!("job-{dataset}");
        let (status, body) = brew(
            &router,
            "POST",
            "/recipeJobs",
            Some(json!({
                "Name": job,
                "RoleArn": ROLE_ARN,
                "DatasetName": dataset,
                // The working draft is addressed by the alias the model documents for it,
                // so this case runs a version no publish froze.
                "RecipeReference": {"Name": "rc-shared", "RecipeVersion": "LATEST_WORKING"},
                "Outputs": [{"Location": {"Bucket": BUCKET, "Key": format!("out-{dataset}/")}}],
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: job {job:?} must be created, got {status}: {body}"
        );

        let (status, started) = brew(&router, "POST", &format!("/jobs/{job}/startJobRun"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartJobRun for {job:?} must be accepted, got {status}: {started}"
        );
        let run_id = started["RunId"].as_str().unwrap_or_default().to_string();
        let run = await_terminal(&router, case, &job, &run_id).await;
        assert_eq!(
            run["State"].as_str().unwrap_or_default(),
            expected,
            "case {case:?}: the run over {dataset:?} must end {expected}, got {run:?}"
        );
    }
}
