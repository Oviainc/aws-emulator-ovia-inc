// SPDX-License-Identifier: CC0-1.0

//! What a recipe JOB is: the role DataBrew runs it as, and the objects its run writes.
//!
//! Driven end to end with the emulator's own Amazon S3 and IAM mounted beside this
//! service on one engine — what `core/router` assembles — so every case resolves the role
//! in the service that owns roles and reads and writes the objects in the service that
//! owns objects.
//!
//! RULE ONE: `CreateRecipeJob` resolves the `RoleArn` the job would RUN AS against this
//! account's IAM before it stores anything, and refuses the create with HTTP 400
//! `ValidationException` — persisting no job — when the trust policy of the role IAM holds
//! does not let `databrew.amazonaws.com` assume it. The message carries DataBrew's own
//! sentence and the rejected role ARN from the request. Wherever nothing is KNOWN about a
//! role of this account (no role held under that name, no IAM to ask) the create is
//! served: a refusal has to be grounded in the trust document IAM itself returned.
//!
//! RULE TWO: a job with nowhere to write is refused — a create selecting none of the
//! three modelled output lists answers 400 with "No outputs were selected for this job."
//! — and a create that asks for no `MaxCapacity` carries the service's default of 5
//! nodes, which `DescribeJob` reports.
//!
//! RULE THREE: a run of a recipe job READS the dataset's object, applies the steps of the
//! recipe version the job references and WRITES the result to each output's Amazon S3
//! location, as one part file per part under the location's key, named
//! `<key><job name>_part<NNNNN>.<extension>`. The terminal state the run reports is what
//! that work reached: `SUCCEEDED` only once every output object is written, and `FAILED`
//! with the reason as its `ErrorMessage` otherwise.
//!
//! The tables span what the rules generalise over: which principal a role trusts (this
//! service, another service, several, a denial, a role IAM does not hold); which
//! transformation a recipe step names and how it is gated (an unconditioned rename, a
//! conditioned row removal over each of several conditions, a type coercion that cannot
//! read a cell, a column delete, a case mapping, several steps in one recipe, and a recipe
//! with no step at all); how the output is addressed (a bare prefix, a nested prefix, no
//! prefix at all); how many part files it is split into; and which output format carries
//! it.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_databrew_awsgluedatabrew::state::recipe_steps::MAX_CONDITIONS_PER_STEP;

use crate::databrew_harness::{job_outputs, Emulator, Reply};

/// The bucket every case stages its dataset object in and writes its output to.
const BUCKET: &str = "brew-data";

/// The key the staged dataset object lives under.
const INPUT_KEY: &str = "in/people.csv";

/// The dataset every run below reads: a header and four rows, so a row filter, a type
/// coercion and a rename each have more than one row to be observed on.
const INPUT_CSV: &str = "name,status,size\nhalyard,Active,2\nboom,Inactive,x\ngaff,Active,7\n\
                         sprit,Inactive,11\n";

/// Comfortably past the run's terminal transition span, so a run's terminal state is due.
/// `tokio`'s `time` feature is not enabled for generated crates, so this blocks the test
/// thread rather than the runtime's timer.
const PAST_TERMINAL: Duration = Duration::from_millis(900);

/// The fixed text DataBrew words the untrusted-role refusal with. Pinned as the literal a
/// client reads, not as the constant the handler carries.
const UNTRUSTED_ROLE_TEXT: &str = "DataBrew is not a trusted entity for the data access role";

/// A trust policy with one statement: `effect` on `sts:AssumeRole` for `principal`, which
/// may be one service or several.
fn trust(effect: &str, principal: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Principal": { "Service": principal },
            "Action": "sts:AssumeRole",
        }],
    })
}

/// An emulator holding the bucket and the staged dataset object every run reads.
async fn emulator_with_input(input: &str) -> Emulator {
    let emulator = Emulator::with_object_store_and_iam();
    emulator.given_bucket(BUCKET).await;
    emulator.given_object(BUCKET, INPUT_KEY, input).await;
    emulator
}

/// `CreateDataset` over the staged object.
async fn create_dataset(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call(
            "POST",
            "/datasets",
            Some(json!({
                "Name": name,
                "Format": "CSV",
                "Input": {"S3InputDefinition": {"Bucket": BUCKET, "Key": INPUT_KEY}},
            })),
        )
        .await
}

/// `CreateRecipe` holding `steps`, then `PublishRecipe`, so the job can reference the
/// published version `1.0` exactly as a client does.
async fn create_published_recipe(emulator: &Emulator, name: &str, steps: Value) {
    let created = emulator
        .call("POST", "/recipes", Some(json!({"Name": name, "Steps": steps})))
        .await;
    assert_eq!(
        created.status, 200,
        "the recipe {name} must be created, got {} {:?}",
        created.status, created.body
    );
    let published = emulator
        .call("POST", &format!("/recipes/{name}/publishRecipe"), Some(json!({})))
        .await;
    assert_eq!(
        published.status, 200,
        "the recipe {name} must be published, got {} {:?}",
        published.status, published.body
    );
}

/// `CreateRecipeJob` with `body`.
async fn create_job(emulator: &Emulator, body: Value) -> Reply {
    emulator.call("POST", "/recipeJobs", Some(body)).await
}

/// Start a run of `job`, wait out its terminal span and answer the `DescribeJobRun` the
/// caller then reads.
async fn run_to_completion(emulator: &Emulator, job: &str) -> Reply {
    let started = emulator
        .call("POST", &format!("/jobs/{job}/startJobRun"), None)
        .await;
    assert_eq!(
        started.status, 200,
        "StartJobRun must be served for {job}, got {} {:?}",
        started.status, started.body
    );
    let run_id = started.text("RunId");
    std::thread::sleep(PAST_TERMINAL);
    emulator
        .call("GET", &format!("/jobs/{job}/jobRun/{run_id}"), None)
        .await
}

/// A job is deleted by name, with the runs it held, and a delete of a job nobody created
/// resolves nothing — the same lookup every other operation that addresses a job by name
/// makes. (`DeleteJob` was a scaffold stub; its own census line is deleted with it.)
#[tokio::test]
async fn a_job_is_deleted_by_name_together_with_its_runs() {
    let emulator = emulator_with_input(INPUT_CSV).await;
    assert_eq!(
        create_dataset(&emulator, "doomed-ds").await.status,
        200,
        "the dataset must be created"
    );
    assert_eq!(
        create_job(
            &emulator,
            json!({
                "Name": "doomed-job",
                "DatasetName": "doomed-ds",
                "RoleArn": "arn:aws:iam::000000000000:role/brew",
                "Outputs": job_outputs("out/"),
            }),
        )
        .await
        .status,
        200,
        "the job must be created"
    );
    let run_id = emulator
        .call("POST", "/jobs/doomed-job/startJobRun", None)
        .await
        .text("RunId");

    // Case "a job nobody created": the delete resolves nothing.
    let absent = emulator.call("DELETE", "/jobs/absent-job", None).await;
    assert_eq!(
        absent.status, 404,
        "case \"a job nobody created\": expected 404, got {} {:?}",
        absent.status, absent.body
    );
    assert_eq!(
        absent.error_type.as_deref(),
        Some("ResourceNotFoundException"),
        "case \"a job nobody created\": expected ResourceNotFoundException, got {:?}",
        absent.error_type
    );
    assert_eq!(
        absent.error_message(),
        "Job absent-job wasn't found.",
        "case \"a job nobody created\": expected the service's own sentence, got {:?}",
        absent.error_message()
    );

    // Case "a job that exists": the delete echoes the name and the job is gone.
    let deleted = emulator.call("DELETE", "/jobs/doomed-job", None).await;
    assert_eq!(
        deleted.status, 200,
        "case \"a job that exists\": expected 200, got {} {:?}",
        deleted.status, deleted.body
    );
    assert_eq!(
        deleted.text("Name"),
        "doomed-job",
        "case \"a job that exists\": expected the deleted job's name back, got {:?}",
        deleted.body
    );
    assert_eq!(
        emulator.call("GET", "/jobs/doomed-job", None).await.status,
        404,
        "case \"a job that exists\": the deleted job must resolve nothing"
    );
    assert_eq!(
        emulator
            .call(
                "GET",
                &format!("/jobs/doomed-job/jobRun/{run_id}"),
                None
            )
            .await
            .status,
        404,
        "case \"a job that exists\": the runs of a deleted job must resolve nothing"
    );
    // A second delete of the same job is the absent case.
    assert_eq!(
        emulator
            .call("DELETE", "/jobs/doomed-job", None)
            .await
            .status,
        404,
        "case \"a job deleted twice\": the second delete must resolve nothing"
    );
}

/// A job's `RoleArn` is resolved against IAM, and the trust policy of the role IAM holds
/// is what decides the create.
#[tokio::test]
async fn a_job_is_refused_when_its_data_access_role_cannot_be_assumed() {
    struct Case {
        /// What the role's trust policy says, or `None` for a role IAM never held.
        trust: Option<Value>,
        /// Whether DataBrew may create a job on it.
        created: bool,
    }
    let cases = [
        (
            "a role trusting this service",
            Case {
                trust: Some(trust("Allow", json!("databrew.amazonaws.com"))),
                created: true,
            },
        ),
        (
            "a role trusting another service",
            Case {
                trust: Some(trust("Allow", json!("lambda.amazonaws.com"))),
                created: false,
            },
        ),
        (
            "a role trusting several services, this one among them",
            Case {
                trust: Some(trust(
                    "Allow",
                    json!(["glue.amazonaws.com", "databrew.amazonaws.com"]),
                )),
                created: true,
            },
        ),
        (
            "a role trusting several services, none of them this one",
            Case {
                trust: Some(trust(
                    "Allow",
                    json!(["glue.amazonaws.com", "lambda.amazonaws.com"]),
                )),
                created: false,
            },
        ),
        (
            "a role denying this service the assume",
            Case {
                trust: Some(trust("Deny", json!("databrew.amazonaws.com"))),
                created: false,
            },
        ),
        (
            // Nothing is KNOWN about a role this account's IAM holds nothing under: a
            // data access role is routinely provisioned out of band, so the create is
            // served rather than refused on this emulator's own store.
            "a role IAM holds nothing under",
            Case {
                trust: None,
                created: true,
            },
        ),
    ];

    for (index, (case, Case { trust, created })) in cases.iter().enumerate() {
        let emulator = Emulator::with_object_store_and_iam();
        let role_name = format!("brew-role-{index}");
        let role_arn = match trust {
            Some(document) => emulator.given_role(&role_name, document).await,
            None => format!("arn:aws:iam::000000000000:role/{role_name}"),
        };
        let job = format!("job-{index}");
        let replied = create_job(
            &emulator,
            json!({
                "Name": job,
                "RoleArn": role_arn,
                "Outputs": job_outputs("out/"),
            }),
        )
        .await;

        if *created {
            assert_eq!(
                replied.status, 200,
                "case {case:?}: expected the job to be created, got {} {:?}",
                replied.status, replied.body
            );
            assert_eq!(
                replied.text("Name"),
                job,
                "case {case:?}: expected the created job's name back, got {:?}",
                replied.body
            );
            continue;
        }

        assert_eq!(
            replied.status, 400,
            "case {case:?}: expected 400, got {} {:?}",
            replied.status, replied.body
        );
        assert_eq!(
            replied.error_type.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?}",
            replied.error_type
        );
        let message = replied.error_message();
        assert!(
            message.contains(UNTRUSTED_ROLE_TEXT),
            "case {case:?}: expected the message to state {UNTRUSTED_ROLE_TEXT:?}, got \
             {message:?}"
        );
        assert!(
            message.contains(&role_arn),
            "case {case:?}: expected the message to name the rejected role {role_arn:?}, got \
             {message:?}"
        );
        // The refused create minted nothing: the job name is still free.
        let described = emulator.call("GET", &format!("/jobs/{job}"), None).await;
        assert_eq!(
            described.status, 404,
            "case {case:?}: a refused create must store no job, got {} {:?}",
            described.status, described.body
        );
    }
}

/// The same reading of a caller-supplied role decides `CreateProject`, which is acted
/// upon as the role it names: the rule is the role's, not one operation's.
#[tokio::test]
async fn a_project_is_refused_when_its_role_cannot_be_assumed() {
    for (case, principal, created) in [
        ("a role trusting this service", "databrew.amazonaws.com", true),
        ("a role trusting another service", "glue.amazonaws.com", false),
    ] {
        let emulator = emulator_with_input(INPUT_CSV).await;
        assert_eq!(
            create_dataset(&emulator, "project-ds").await.status,
            200,
            "case {case:?}: the dataset a project acts upon must be created"
        );
        create_published_recipe(
            &emulator,
            "project-rc",
            json!([{
                "Action": {"Operation": "UPPER_CASE", "Parameters": {"sourceColumn": "name"}},
            }]),
        )
        .await;
        let role_arn = emulator
            .given_role("project-role", &trust("Allow", json!(principal)))
            .await;
        let replied = emulator
            .call(
                "POST",
                "/projects",
                Some(json!({
                    "Name": "brew-project",
                    "DatasetName": "project-ds",
                    "RecipeName": "project-rc",
                    "RoleArn": role_arn,
                })),
            )
            .await;

        if created {
            assert_eq!(
                replied.status, 200,
                "case {case:?}: expected the project to be created, got {} {:?}",
                replied.status, replied.body
            );
            continue;
        }
        assert_eq!(
            replied.status, 400,
            "case {case:?}: expected 400, got {} {:?}",
            replied.status, replied.body
        );
        let message = replied.error_message();
        assert!(
            message.contains(UNTRUSTED_ROLE_TEXT) && message.contains(&role_arn),
            "case {case:?}: expected the refusal to state {UNTRUSTED_ROLE_TEXT:?} and name \
             {role_arn:?}, got {message:?}"
        );
        assert_eq!(
            emulator
                .call("GET", "/projects/brew-project", None)
                .await
                .status,
            404,
            "case {case:?}: a refused create must store no project"
        );
    }
}

/// A job must select somewhere to write, and a job that asks for no capacity carries the
/// service's default.
#[tokio::test]
async fn a_job_selects_an_output_and_carries_the_default_capacity() {
    let emulator = Emulator::with_object_store_and_iam();

    // Case "no output list at all" and "an EMPTY output list": a list carrying no entry
    // selects no output exactly as an absent one does, so neither is stored — the empty
    // one against the modelled `len 1..` on the list itself, the absent one against the
    // service's own rule, which is the only one a request can reach.
    for (case, body, expected_message) in [
        (
            "no output list at all",
            json!({"Name": "no-output-job", "RoleArn": "arn:aws:iam::000000000000:role/brew"}),
            "No outputs were selected for this job.",
        ),
        (
            "an empty output list",
            json!({
                "Name": "no-output-job",
                "RoleArn": "arn:aws:iam::000000000000:role/brew",
                "Outputs": [],
            }),
            "Member must have length greater than or equal to 1",
        ),
    ] {
        let refused = create_job(&emulator, body).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?}",
            refused.error_type
        );
        assert!(
            refused.error_message().contains(expected_message),
            "case {case:?}: expected the refusal to state {expected_message:?}, got {:?}",
            refused.error_message()
        );
        assert_eq!(
            emulator
                .call("GET", "/jobs/no-output-job", None)
                .await
                .status,
            404,
            "case {case:?}: the refused create must store no job"
        );
    }

    // Each selectable output list satisfies the rule on its own, and the capacity a
    // create asks for is the one `DescribeJob` reports — else the service's default.
    let cases = [
        ("Outputs", json!({"Outputs": job_outputs("out/")}), 5),
        (
            "Outputs with a capacity",
            json!({"Outputs": job_outputs("out/"), "MaxCapacity": 12}),
            12,
        ),
        (
            "DataCatalogOutputs",
            json!({"DataCatalogOutputs": [{"DatabaseName": "sales", "TableName": "people"}]}),
            5,
        ),
        (
            "DatabaseOutputs",
            json!({
                "DatabaseOutputs": [{
                    "GlueConnectionName": "warehouse",
                    "DatabaseOptions": {"TableName": "people"},
                }],
            }),
            5,
        ),
    ];
    for (index, (case, selection, expected_capacity)) in cases.iter().enumerate() {
        let name = format!("output-job-{index}");
        let mut body = json!({"Name": name, "RoleArn": "arn:aws:iam::000000000000:role/brew"});
        for (member, value) in selection.as_object().expect("the selection is an object") {
            body[member] = value.clone();
        }
        let created = create_job(&emulator, body).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the job to be created, got {} {:?}",
            created.status, created.body
        );
        let described = emulator.call("GET", &format!("/jobs/{name}"), None).await;
        assert_eq!(
            described.body["MaxCapacity"],
            json!(expected_capacity),
            "case {case:?}: expected MaxCapacity {expected_capacity}, got {:?}",
            described.body["MaxCapacity"]
        );
    }
}

/// A run applies the recipe version's steps to the dataset and writes the result under the
/// output's key.
#[tokio::test]
async fn a_run_writes_the_recipe_applied_to_the_dataset() {
    struct Case {
        /// The steps the recipe holds, or `None` for a job that references no recipe.
        steps: Option<Value>,
        /// The CSV the output object must hold, header included.
        expected: &'static str,
    }
    let cases = [
        (
            "no recipe referenced at all: the dataset is the output",
            Case {
                steps: None,
                expected: INPUT_CSV,
            },
        ),
        (
            "RENAME replaces the column name in the header",
            Case {
                steps: Some(json!([{
                    "Action": {
                        "Operation": "RENAME",
                        "Parameters": {"sourceColumn": "name", "targetColumn": "sailor"},
                    },
                }])),
                expected: "sailor,status,size\nhalyard,Active,2\nboom,Inactive,x\n\
                           gaff,Active,7\nsprit,Inactive,11\n",
            },
        ),
        (
            "REMOVE_VALUES gated by IS drops the matching rows",
            Case {
                steps: Some(json!([{
                    "Action": {"Operation": "REMOVE_VALUES", "Parameters": {"sourceColumn": "status"}},
                    "ConditionExpressions": [{
                        "Condition": "IS",
                        "Value": "[\"Inactive\"]",
                        "TargetColumn": "status",
                    }],
                }])),
                expected: "name,status,size\nhalyard,Active,2\ngaff,Active,7\n",
            },
        ),
        (
            "REMOVE_VALUES gated by GREATER_THAN keeps the rows under the bound",
            Case {
                steps: Some(json!([{
                    "Action": {"Operation": "REMOVE_VALUES", "Parameters": {"sourceColumn": "size"}},
                    "ConditionExpressions": [{
                        "Condition": "GREATER_THAN",
                        "Value": "5",
                        "TargetColumn": "size",
                    }],
                }])),
                expected: "name,status,size\nhalyard,Active,2\nboom,Inactive,x\n",
            },
        ),
        (
            "REMOVE_VALUES gated by STARTS_WITH matches on the cell's prefix",
            Case {
                steps: Some(json!([{
                    "Action": {"Operation": "REMOVE_VALUES", "Parameters": {"sourceColumn": "name"}},
                    "ConditionExpressions": [{
                        "Condition": "STARTS_WITH",
                        "Value": "[\"g\", \"s\"]",
                        "TargetColumn": "name",
                    }],
                }])),
                expected: "name,status,size\nhalyard,Active,2\nboom,Inactive,x\n",
            },
        ),
        (
            "CHANGE_DATA_TYPE to int NULLs the cell it cannot read and keeps its row",
            Case {
                steps: Some(json!([{
                    "Action": {
                        "Operation": "CHANGE_DATA_TYPE",
                        "Parameters": {"sourceColumn": "size", "columnDataType": "int"},
                    },
                }])),
                expected: "name,status,size\nhalyard,Active,2\nboom,Inactive,\n\
                           gaff,Active,7\nsprit,Inactive,11\n",
            },
        ),
        (
            "DELETE drops the column and its cells",
            Case {
                steps: Some(json!([{
                    "Action": {"Operation": "DELETE", "Parameters": {"sourceColumn": "status"}},
                }])),
                expected: "name,size\nhalyard,2\nboom,x\ngaff,7\nsprit,11\n",
            },
        ),
        (
            "MERGE writes a new column holding the joined source cells",
            Case {
                steps: Some(json!([{
                    "Action": {
                        "Operation": "MERGE",
                        "Parameters": {
                            "sourceColumns": "[\"name\",\"status\"]",
                            "targetColumn": "label",
                            "delimiter": "-",
                        },
                    },
                }])),
                expected: "name,status,size,label\nhalyard,Active,2,halyard-Active\n\
                           boom,Inactive,x,boom-Inactive\ngaff,Active,7,gaff-Active\n\
                           sprit,Inactive,11,sprit-Inactive\n",
            },
        ),
        (
            "a step reads the column an earlier step merged into",
            Case {
                steps: Some(json!([
                    {"Action": {
                        "Operation": "MERGE",
                        "Parameters": {
                            "sourceColumn1": "name",
                            "sourceColumn2": "status",
                            "targetColumn": "label",
                        },
                    }},
                    {"Action": {"Operation": "UPPER_CASE", "Parameters": {"sourceColumn": "label"}}},
                ])),
                expected: "name,status,size,label\nhalyard,Active,2,HALYARDACTIVE\n\
                           boom,Inactive,x,BOOMINACTIVE\ngaff,Active,7,GAFFACTIVE\n\
                           sprit,Inactive,11,SPRITINACTIVE\n",
            },
        ),
        (
            "UPPER_CASE maps only the rows its condition holds for",
            Case {
                steps: Some(json!([{
                    "Action": {"Operation": "UPPER_CASE", "Parameters": {"sourceColumn": "name"}},
                    "ConditionExpressions": [{
                        "Condition": "IS",
                        "Value": "[\"Active\"]",
                        "TargetColumn": "status",
                    }],
                }])),
                expected: "name,status,size\nHALYARD,Active,2\nboom,Inactive,x\n\
                           GAFF,Active,7\nsprit,Inactive,11\n",
            },
        ),
        (
            "several steps are applied in order",
            Case {
                steps: Some(json!([
                    {
                        "Action": {"Operation": "REMOVE_VALUES", "Parameters": {"sourceColumn": "status"}},
                        "ConditionExpressions": [{
                            "Condition": "IS_NOT",
                            "Value": "[\"Active\"]",
                            "TargetColumn": "status",
                        }],
                    },
                    {
                        "Action": {
                            "Operation": "RENAME",
                            "Parameters": {"sourceColumn": "size", "targetColumn": "crew"},
                        },
                    },
                    {
                        "Action": {
                            "Operation": "CHANGE_DATA_TYPE",
                            "Parameters": {"sourceColumn": "crew", "columnDataType": "int"},
                        },
                    },
                ])),
                expected: "name,status,crew\nhalyard,Active,2\ngaff,Active,7\n",
            },
        ),
    ];

    for (index, (case, Case { steps, expected })) in cases.iter().enumerate() {
        let emulator = emulator_with_input(INPUT_CSV).await;
        let dataset = format!("ds-{index}");
        assert_eq!(
            create_dataset(&emulator, &dataset).await.status,
            200,
            "case {case:?}: the dataset must be created"
        );
        let job = format!("brew-job-{index}");
        let mut body = json!({
            "Name": job,
            "DatasetName": dataset,
            "RoleArn": "arn:aws:iam::000000000000:role/brew",
            "Outputs": [{
                "Format": "CSV",
                "Location": {"Bucket": BUCKET, "Key": "out/"},
                "Overwrite": true,
                "MaxOutputFiles": 1,
            }],
        });
        if let Some(steps) = steps {
            let recipe = format!("rc-{index}");
            create_published_recipe(&emulator, &recipe, steps.clone()).await;
            body["RecipeReference"] = json!({"Name": recipe, "RecipeVersion": "1.0"});
        }
        assert_eq!(
            create_job(&emulator, body).await.status,
            200,
            "case {case:?}: the job must be created"
        );

        let described = run_to_completion(&emulator, &job).await;
        assert_eq!(
            described.text("State"),
            "SUCCEEDED",
            "case {case:?}: expected a SUCCEEDED run, got {:?}",
            described.body
        );

        let key = format!("out/{job}_part00000.csv");
        assert_eq!(
            emulator.keys_under(BUCKET, "out/").await,
            vec![key.clone()],
            "case {case:?}: expected exactly the one part file the output asked for"
        );
        let written = emulator.object_text(BUCKET, &key).await;
        assert_eq!(
            written.as_deref(),
            Some(*expected),
            "case {case:?}: expected the transformed dataset, got {written:?}"
        );
    }
}

/// How the output is ADDRESSED and how many files it is split into are the output's own
/// members: the key is the prefix the part names hang off, `MaxOutputFiles` is how many
/// parts there are, and `Format` decides the extension and the encoding.
#[tokio::test]
async fn a_run_writes_one_part_file_per_part_under_the_output_key() {
    struct Case {
        key: &'static str,
        /// The prefix the written objects are listed under, which is the output's own key
        /// except where the output carries none.
        list_prefix: &'static str,
        format: &'static str,
        max_output_files: i32,
        /// The keys the run must have written, in listing order.
        expected_keys: Vec<String>,
        /// What the FIRST of them must hold.
        expected_first: &'static str,
    }
    let cases = [
        (
            "a bare prefix, one file",
            Case {
                key: "out/",
                list_prefix: "out/",
                format: "CSV",
                max_output_files: 1,
                expected_keys: vec!["out/parts-job_part00000.csv".to_string()],
                expected_first: "name,status,size\nhalyard,Active,2\nboom,Inactive,x\n\
                                 gaff,Active,7\nsprit,Inactive,11\n",
            },
        ),
        (
            "a nested prefix, one file",
            Case {
                key: "exports/2026/",
                list_prefix: "exports/2026/",
                format: "CSV",
                max_output_files: 1,
                expected_keys: vec!["exports/2026/parts-job_part00000.csv".to_string()],
                expected_first: "name,status,size\nhalyard,Active,2\nboom,Inactive,x\n\
                                 gaff,Active,7\nsprit,Inactive,11\n",
            },
        ),
        (
            // The modelled `Key` is `len 1..1280`, so an output with no prefix at all is
            // one that carries no `Key` member: its parts land at the bucket root.
            "no Key member at all, one file",
            Case {
                key: "",
                list_prefix: "parts-job",
                format: "CSV",
                max_output_files: 1,
                expected_keys: vec!["parts-job_part00000.csv".to_string()],
                expected_first: "name,status,size\nhalyard,Active,2\nboom,Inactive,x\n\
                                 gaff,Active,7\nsprit,Inactive,11\n",
            },
        ),
        (
            "two files split the rows, each carrying the header",
            Case {
                key: "out/",
                list_prefix: "out/",
                format: "CSV",
                max_output_files: 2,
                expected_keys: vec![
                    "out/parts-job_part00000.csv".to_string(),
                    "out/parts-job_part00001.csv".to_string(),
                ],
                expected_first: "name,status,size\nhalyard,Active,2\nboom,Inactive,x\n",
            },
        ),
        (
            "more files than rows writes one file per row",
            Case {
                key: "out/",
                list_prefix: "out/",
                format: "CSV",
                max_output_files: 9,
                expected_keys: vec![
                    "out/parts-job_part00000.csv".to_string(),
                    "out/parts-job_part00001.csv".to_string(),
                    "out/parts-job_part00002.csv".to_string(),
                    "out/parts-job_part00003.csv".to_string(),
                ],
                expected_first: "name,status,size\nhalyard,Active,2\n",
            },
        ),
        (
            "a JSON output carries the .json extension and one record per line",
            Case {
                key: "out/",
                list_prefix: "out/",
                format: "JSON",
                max_output_files: 1,
                expected_keys: vec!["out/parts-job_part00000.json".to_string()],
                expected_first: "{\"name\":\"halyard\",\"status\":\"Active\",\"size\":\"2\"}\n\
                                 {\"name\":\"boom\",\"status\":\"Inactive\",\"size\":\"x\"}\n\
                                 {\"name\":\"gaff\",\"status\":\"Active\",\"size\":\"7\"}\n\
                                 {\"name\":\"sprit\",\"status\":\"Inactive\",\"size\":\"11\"}\n",
            },
        ),
    ];

    for (
        case,
        Case {
            key,
            list_prefix,
            format,
            max_output_files,
            expected_keys,
            expected_first,
        },
    ) in &cases
    {
        let emulator = emulator_with_input(INPUT_CSV).await;
        assert_eq!(
            create_dataset(&emulator, "parts-ds").await.status,
            200,
            "case {case:?}: the dataset must be created"
        );
        let mut location = json!({"Bucket": BUCKET});
        if !key.is_empty() {
            location["Key"] = json!(key);
        }
        assert_eq!(
            create_job(
                &emulator,
                json!({
                    "Name": "parts-job",
                    "DatasetName": "parts-ds",
                    "RoleArn": "arn:aws:iam::000000000000:role/brew",
                    "Outputs": [{
                        "Format": format,
                        "Location": location,
                        "Overwrite": true,
                        "MaxOutputFiles": max_output_files,
                    }],
                }),
            )
            .await
            .status,
            200,
            "case {case:?}: the job must be created"
        );

        let described = run_to_completion(&emulator, "parts-job").await;
        assert_eq!(
            described.text("State"),
            "SUCCEEDED",
            "case {case:?}: expected a SUCCEEDED run, got {:?}",
            described.body
        );
        let written = emulator.keys_under(BUCKET, list_prefix).await;
        assert_eq!(
            &written, expected_keys,
            "case {case:?}: expected exactly the part files the output asked for"
        );
        let first = emulator
            .object_text(BUCKET, expected_keys.first().expect("a key is expected"))
            .await;
        assert_eq!(
            first.as_deref(),
            Some(*expected_first),
            "case {case:?}: expected the first part's own rows, got {first:?}"
        );
    }
}

/// A run that could not do its work reports `FAILED` with the reason, and writes nothing:
/// a terminal state is what the work reached, never what the clock assumed.
#[tokio::test]
async fn a_run_that_cannot_do_its_work_reports_failed_and_writes_nothing() {
    struct Case {
        /// Whether the dataset's object is staged at all.
        staged: bool,
        /// The steps the recipe holds.
        steps: Value,
        /// Text the run's `ErrorMessage` must carry.
        reason: &'static str,
    }
    let cases = [
        (
            "the dataset's object is not in the store",
            Case {
                staged: false,
                steps: json!([]),
                reason: "could not be read",
            },
        ),
        (
            "a recipe step names a column the dataset does not hold",
            Case {
                staged: true,
                steps: json!([{
                    "Action": {
                        "Operation": "RENAME",
                        "Parameters": {"sourceColumn": "absent", "targetColumn": "sailor"},
                    },
                }]),
                reason: "Column 'absent' does not exist",
            },
        ),
        (
            "a recipe step names a transformation this emulator does not perform",
            Case {
                staged: true,
                steps: json!([{
                    "Action": {
                        "Operation": "FLAG_COLUMN_FROM_NULL",
                        "Parameters": {"sourceColumn": "size"},
                    },
                }]),
                reason: "is not a transformation this emulator performs",
            },
        ),
        (
            // A merge JOINS values, so a source set naming one column twice would double
            // its cells and a recipe of such steps would multiply them.
            "a MERGE names the same source column twice",
            Case {
                staged: true,
                steps: json!([{
                    "Action": {
                        "Operation": "MERGE",
                        "Parameters": {
                            "sourceColumns": "[\"name\",\"name\"]",
                            "targetColumn": "label",
                        },
                    },
                }]),
                reason: "names the column name more than once",
            },
        ),
        (
            "a MERGE names the column it writes among the columns it reads",
            Case {
                staged: true,
                steps: json!([{
                    "Action": {
                        "Operation": "MERGE",
                        "Parameters": {
                            "sourceColumns": "[\"name\",\"status\"]",
                            "targetColumn": "name",
                        },
                    },
                }]),
                reason: "names the column name it writes among the columns it reads",
            },
        ),
        (
            // Each condition is evaluated once per row, so the number of them is capped
            // (`recipe_steps::MAX_CONDITIONS_PER_STEP`) and a step past it is not applied.
            "a step carries more conditions than this emulator evaluates",
            Case {
                staged: true,
                steps: json!([{
                    "Action": {"Operation": "REMOVE_VALUES", "Parameters": {"sourceColumn": "name"}},
                    "ConditionExpressions": (0..MAX_CONDITIONS_PER_STEP + 1)
                        .map(|_| json!({
                            "Condition": "IS_NOT",
                            "Value": "[\"halyard\"]",
                            "TargetColumn": "name",
                        }))
                        .collect::<Vec<Value>>(),
                }]),
                reason: "more than the 16 this emulator evaluates for one step",
            },
        ),
    ];

    for (index, (case, Case { staged, steps, reason })) in cases.iter().enumerate() {
        let emulator = Emulator::with_object_store_and_iam();
        emulator.given_bucket(BUCKET).await;
        if *staged {
            emulator.given_object(BUCKET, INPUT_KEY, INPUT_CSV).await;
        }
        let dataset = format!("failing-ds-{index}");
        let recipe = format!("failing-rc-{index}");
        let job = format!("failing-job-{index}");
        assert_eq!(
            create_dataset(&emulator, &dataset).await.status,
            200,
            "case {case:?}: the dataset must be created"
        );
        create_published_recipe(&emulator, &recipe, steps.clone()).await;
        assert_eq!(
            create_job(
                &emulator,
                json!({
                    "Name": job,
                    "DatasetName": dataset,
                    "RoleArn": "arn:aws:iam::000000000000:role/brew",
                    "RecipeReference": {"Name": recipe, "RecipeVersion": "1.0"},
                    "Outputs": job_outputs("out/"),
                }),
            )
            .await
            .status,
            200,
            "case {case:?}: the job must be created"
        );

        let described = run_to_completion(&emulator, &job).await;
        assert_eq!(
            described.text("State"),
            "FAILED",
            "case {case:?}: expected a FAILED run, got {:?}",
            described.body
        );
        let message = described.text("ErrorMessage");
        assert!(
            message.contains(reason),
            "case {case:?}: expected the ErrorMessage to state {reason:?}, got {message:?}"
        );
        assert!(
            emulator.keys_under(BUCKET, "out/").await.is_empty(),
            "case {case:?}: a run that did not finish must have written no output, got {:?}",
            emulator.keys_under(BUCKET, "out/").await
        );
    }
}

/// An output kind this emulator does not write is REPORTED by the create's read-back and
/// is the reason its run could not be performed — never passed over on the way to a
/// success nothing backs.
#[tokio::test]
async fn a_run_fails_for_an_output_kind_this_emulator_does_not_write() {
    for (case, selection, reported, reason) in [
        (
            "a Glue Data Catalog output",
            json!({"DataCatalogOutputs": [{"DatabaseName": "sales", "TableName": "people"}]}),
            "DataCatalogOutputs",
            "Glue Data Catalog",
        ),
        (
            "a JDBC database output",
            json!({
                "DatabaseOutputs": [{
                    "GlueConnectionName": "warehouse",
                    "DatabaseOptions": {"TableName": "people"},
                }],
            }),
            "DatabaseOutputs",
            "JDBC database",
        ),
    ] {
        let emulator = emulator_with_input(INPUT_CSV).await;
        assert_eq!(
            create_dataset(&emulator, "catalog-ds").await.status,
            200,
            "case {case:?}: the dataset must be created"
        );
        let mut body = json!({
            "Name": "catalog-job",
            "DatasetName": "catalog-ds",
            "RoleArn": "arn:aws:iam::000000000000:role/brew",
        });
        for (member, value) in selection.as_object().expect("the selection is an object") {
            body[member] = value.clone();
        }
        assert_eq!(
            create_job(&emulator, body).await.status,
            200,
            "case {case:?}: the job must be created"
        );

        // What the create selected is what a read reports: the selection is held, not
        // discarded. The reported entry carries the members the request named (beside the
        // modelled defaults the shape fills in).
        let described = emulator.call("GET", "/jobs/catalog-job", None).await;
        let reported_entry = described.member(reported)[0].clone();
        for (member, value) in selection[reported][0]
            .as_object()
            .expect("the selected output is an object")
        {
            assert_eq!(
                reported_entry[member], *value,
                "case {case:?}: expected DescribeJob to report the selected {reported} member \
                 {member}, got {:?}",
                described.member(reported)
            );
        }

        let run = run_to_completion(&emulator, "catalog-job").await;
        assert_eq!(
            run.text("State"),
            "FAILED",
            "case {case:?}: expected a FAILED run, got {:?}",
            run.body
        );
        let message = run.text("ErrorMessage");
        assert!(
            message.contains(reason),
            "case {case:?}: expected the ErrorMessage to name the {reason:?} output, got \
             {message:?}"
        );
        assert_eq!(
            run.member(reported),
            described.member(reported),
            "case {case:?}: expected the run to report what it ran with, got {:?}",
            run.member(reported)
        );
    }
}

/// `Overwrite` decides whether a run may replace what the output location already holds:
/// "a value that, if true, means that any data in the location specified for output is
/// overwritten with new output" (model: `Output$Overwrite`), so an output that did not ask
/// for it leaves the object that is there and the run fails.
#[tokio::test]
async fn an_output_that_does_not_overwrite_leaves_what_the_location_holds() {
    for (case, overwrite, expected_state, expected_body) in [
        ("the output overwrites", true, "SUCCEEDED", INPUT_CSV),
        (
            "the output does not overwrite",
            false,
            "FAILED",
            "held,bytes\n1,2\n",
        ),
    ] {
        let emulator = emulator_with_input(INPUT_CSV).await;
        assert_eq!(
            create_dataset(&emulator, "held-ds").await.status,
            200,
            "case {case:?}: the dataset must be created"
        );
        // The key the run's first part would be written under already holds an object.
        emulator
            .given_object(BUCKET, "out/held-job_part00000.csv", "held,bytes\n1,2\n")
            .await;
        assert_eq!(
            create_job(
                &emulator,
                json!({
                    "Name": "held-job",
                    "DatasetName": "held-ds",
                    "RoleArn": "arn:aws:iam::000000000000:role/brew",
                    "Outputs": [{
                        "Format": "CSV",
                        "Location": {"Bucket": BUCKET, "Key": "out/"},
                        "Overwrite": overwrite,
                        "MaxOutputFiles": 1,
                    }],
                }),
            )
            .await
            .status,
            200,
            "case {case:?}: the job must be created"
        );

        let run = run_to_completion(&emulator, "held-job").await;
        assert_eq!(
            run.text("State"),
            expected_state,
            "case {case:?}: expected a {expected_state} run, got {:?}",
            run.body
        );
        let written = emulator
            .object_text(BUCKET, "out/held-job_part00000.csv")
            .await;
        assert_eq!(
            written.as_deref(),
            Some(expected_body),
            "case {case:?}: expected the object the run left behind, got {written:?}"
        );
    }
}
