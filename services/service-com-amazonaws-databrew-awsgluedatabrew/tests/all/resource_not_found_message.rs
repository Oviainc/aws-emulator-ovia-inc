// SPDX-License-Identifier: CC0-1.0

//! The `ResourceNotFoundException` message every DataBrew lookup miss answers with.
//!
//! The rule pinned here: the message is built FROM THE REQUEST — it names the kind of
//! resource the lookup addressed and the identifier the request carried for it
//! (`<Kind> <identifier> wasn't found.`), so two misses of two different resources
//! answer two different messages. The predicate belongs to the resource KIND: a
//! schedule's miss is stated in DataBrew's short form (`Schedule <Name> not found`),
//! which `tests/all/schedule_refusal_wording.rs` spans over the schedule operations. The sentence the model documents on the error shape
//! ("One or more resources can't be found.", `smithy_query(query="shape",
//! shape="ResourceNotFoundException")`) describes the error CLASS; it is not the
//! message a particular lookup answers with.
//!
//! A project's miss carries DataBrew's project sentence (`The project <Name> wasn't
//! found.`), which `tests/all/project_not_found_wording.rs` spans over the project
//! operations.
//!
//! A lookup that addresses a (recipe, VERSION) pair has DataBrew's own sentence for
//! that pair (`The recipe <Name> for version <RecipeVersion> wasn't found.`), which
//! `tests/all/recipe_version_resolution.rs` is where the version forms are spanned.
//!
//! The table is two-dimensional: every operation that answers the error (across every
//! resource kind this service stores) crossed with the forms a DataBrew resource name
//! takes — one character, the modelled maximum length of that operation's name shape,
//! a name carrying a space, and a name whose characters a create would refuse but a
//! lookup still simply misses.
//! Status (404) and error code are asserted on every row too, so a message fix cannot
//! quietly change either.

use serde_json::{json, Value};

use crate::databrew_harness::{label, Emulator, Reply};

/// The `RoleArn` `CreateRecipeJob` requires; the role itself is not what these cases
/// are about.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/brew";

/// The job name the `CreateRecipeJob` row creates under. Every row of that case is
/// refused by the dataset lookup, so no job is ever stored and the name never
/// conflicts.
const PENDING_JOB: &str = "job-awaiting-its-dataset";

/// One request: its method, its URI and the body it carries.
type Request = (&'static str, String, Option<Value>);

/// One operation that answers `ResourceNotFoundException` for a resource it cannot
/// find, as a request built from the missing resource's name and the message that
/// lookup must answer with.
struct NotFoundCase {
    case: &'static str,
    request: fn(&str) -> Request,
    expected: fn(&str) -> String,
    /// The modelled maximum length of the name this case looks up, so the table's
    /// upper-bound row stays a lookup rather than a length violation (`smithy_query(
    /// query="shape", shape="DatasetName,JobName,RecipeName,ScheduleName")`:
    /// `JobName` is `len 1..240`, the other three `len 1..255`).
    name_max: usize,
}

/// The `Message` of a modelled DataBrew error, whichever casing the protocol put it on
/// the wire with.
fn error_message(reply: &Reply) -> String {
    reply
        .body
        .get("Message")
        .or_else(|| reply.body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Every operation whose miss of a named resource this service answers with the error,
/// with the method and URI the model binds it to (`smithy_query(query="digest")`).
fn not_found_cases() -> Vec<NotFoundCase> {
    vec![
        NotFoundCase {
            case: "DescribeDataset",
            request: |name| ("GET", format!("/datasets/{}", label(name)), None),
            expected: |name| format!("Dataset {name} wasn't found."),
            name_max: 255,
        },
        NotFoundCase {
            case: "UpdateDataset",
            request: |name| {
                (
                    "PUT",
                    format!("/datasets/{}", label(name)),
                    Some(json!({
                        "Input": {"S3InputDefinition": {"Bucket": "brew-bucket", "Key": "in/d.csv"}}
                    })),
                )
            },
            expected: |name| format!("Dataset {name} wasn't found."),
            name_max: 255,
        },
        NotFoundCase {
            case: "DeleteDataset",
            request: |name| ("DELETE", format!("/datasets/{}", label(name)), None),
            expected: |name| format!("Dataset {name} wasn't found."),
            name_max: 255,
        },
        NotFoundCase {
            case: "CreateRecipeJob over an absent dataset",
            request: |name| {
                (
                    "POST",
                    "/recipeJobs".to_string(),
                    Some(json!({
                        "Name": PENDING_JOB,
                        "RoleArn": ROLE_ARN,
                        "DatasetName": name,
                    })),
                )
            },
            expected: |name| format!("Dataset {name} wasn't found."),
            name_max: 255,
        },
        NotFoundCase {
            case: "DescribeJob",
            request: |name| ("GET", format!("/jobs/{}", label(name)), None),
            expected: |name| format!("Job {name} wasn't found."),
            name_max: 240,
        },
        NotFoundCase {
            case: "StartJobRun",
            request: |name| (
                "POST",
                format!("/jobs/{}/startJobRun", label(name)),
                None,
            ),
            expected: |name| format!("Job {name} wasn't found."),
            name_max: 240,
        },
        NotFoundCase {
            case: "ListJobRuns",
            request: |name| ("GET", format!("/jobs/{}/jobRuns", label(name)), None),
            expected: |name| format!("Job {name} wasn't found."),
            name_max: 240,
        },
        NotFoundCase {
            case: "DescribeJobRun of a job nobody created",
            request: |name| {
                (
                    "GET",
                    format!(
                        "/jobs/{}/jobRun/db_000000000000000000000001",
                        label(name)
                    ),
                    None,
                )
            },
            expected: |name| format!("Job {name} wasn't found."),
            name_max: 240,
        },
        NotFoundCase {
            case: "DescribeRecipe",
            request: |name| ("GET", format!("/recipes/{}", label(name)), None),
            // `DescribeRecipe` addresses a (recipe, version) pair, and an omitted
            // `RecipeVersion` asks for the latest published version, so the identifier
            // reported for a versionless request is that alias.
            expected: |name| format!("The recipe {name} for version LATEST_PUBLISHED wasn't found."),
            name_max: 255,
        },
        NotFoundCase {
            case: "DescribeSchedule",
            request: |name| ("GET", format!("/schedules/{}", label(name)), None),
            // A schedule's refusals are stated in DataBrew's short form, with no
            // terminating period; see `tests/all/schedule_refusal_wording.rs`.
            expected: |name| format!("Schedule {name} not found"),
            name_max: 255,
        },
    ]
}

/// Every form of resource name the rule must carry through to the message: the modelled
/// length bound at both ends, a name with a space in it (which the URI label has to
/// encode), and a name a create would refuse for its characters — a lookup of it is
/// still just a miss.
fn name_forms(name_max: usize) -> Vec<(String, String)> {
    vec![
        ("a plain name".to_string(), "absent-ds".to_string()),
        ("digits and hyphens".to_string(), "brew-2026-07-x9".to_string()),
        (
            "one character, the modelled minimum".to_string(),
            "x".to_string(),
        ),
        (
            format!("{name_max} characters, the modelled maximum"),
            "n".repeat(name_max),
        ),
        (
            "a name carrying a space".to_string(),
            "an absent resource".to_string(),
        ),
        (
            "characters a create would refuse, so nothing can hold it".to_string(),
            "bad_name".to_string(),
        ),
    ]
}

/// Every lookup miss answers 404 `ResourceNotFoundException` whose message names the
/// resource kind and the exact identifier the request asked for.
#[tokio::test]
async fn a_lookup_miss_names_the_resource_it_could_not_find() {
    let emulator = Emulator::new();

    for operation in not_found_cases() {
        for (form, name) in name_forms(operation.name_max) {
            let case = format!("{} / {form}", operation.case);
            let (method, uri, body) = (operation.request)(&name);
            let refused = emulator.call(method, &uri, body).await;

            assert_eq!(
                refused.status, 404,
                "case {case:?}: expected 404, got {} {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_type.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case:?}: expected ResourceNotFoundException, got {:?}",
                refused.error_type
            );
            let expected = (operation.expected)(&name);
            assert_eq!(
                error_message(&refused),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                error_message(&refused)
            );
        }
    }
}

/// Two misses of two different resources answer two DIFFERENT messages: the message is
/// a function of the request, not a constant.
#[tokio::test]
async fn two_different_misses_answer_two_different_messages() {
    let emulator = Emulator::new();

    let first = emulator.call("GET", "/datasets/one-absent-ds", None).await;
    let second = emulator.call("GET", "/datasets/another-absent-ds", None).await;
    assert_ne!(
        error_message(&first),
        error_message(&second),
        "case \"two absent datasets\": each miss must name its own resource, got {:?} twice",
        error_message(&first)
    );

    // Nor can a caller confuse one resource KIND with another: the same name, missing
    // as a dataset and as a job, answers two different messages.
    let as_dataset = emulator.call("GET", "/datasets/shared-name", None).await;
    let as_job = emulator.call("GET", "/jobs/shared-name", None).await;
    assert_ne!(
        error_message(&as_dataset),
        error_message(&as_job),
        "case \"one name, two kinds\": each miss must name its resource kind, got {:?} twice",
        error_message(&as_dataset)
    );
}

/// A resource addressed by a PAIR reports the half that is missing: a run of a job that
/// exists is looked up by its run id, so an id the job never minted is what the message
/// names, and a version an existing recipe does not hold names the recipe and the
/// version asked for.
#[tokio::test]
async fn a_compound_identifier_names_the_half_that_is_missing() {
    let emulator = Emulator::new();

    let job = "live-job";
    assert_eq!(
        emulator
            .call(
                "POST",
                "/recipeJobs",
                Some(json!({"Name": job, "RoleArn": ROLE_ARN})),
            )
            .await
            .status,
        200,
        "the job must be created"
    );

    // Run ids are minted by the service, so no id is held until a run is started.
    for run_id in [
        "db_000000000000000000000001",
        "db_000000000000000000009999",
        "not-an-id-this-service-mints",
    ] {
        let refused = emulator
            .call("GET", &format!("/jobs/{job}/jobRun/{run_id}"), None)
            .await;
        assert_eq!(
            refused.status, 404,
            "case {run_id:?}: expected 404, got {} {:?}",
            refused.status, refused.body
        );
        let expected = format!("Job run {run_id} wasn't found.");
        assert_eq!(
            error_message(&refused),
            expected,
            "case {run_id:?}: expected {expected:?}, got {:?}",
            error_message(&refused)
        );
    }

    let recipe = "live-recipe";
    assert_eq!(
        emulator
            .call(
                "POST",
                "/recipes",
                Some(json!({
                    "Name": recipe,
                    "Steps": [{"Action": {"Operation": "UPPER_CASE"}}],
                })),
            )
            .await
            .status,
        200,
        "the recipe must be created"
    );

    // Every identifier here is well formed (model: `Recipe$RecipeVersion` — `X.Y` with
    // up to 6 digits each, or one of the two aliases) and still names no version this
    // freshly created recipe holds: nothing is published, and its working version is
    // the first minor of major 0.
    for version in ["LATEST_PUBLISHED", "1.0", "0.2", "999999.999999"] {
        let refused = emulator
            .call("GET", &format!("/recipes/{recipe}?recipeVersion={version}"), None)
            .await;
        assert_eq!(
            refused.status, 404,
            "case {version:?}: expected 404, got {} {:?}",
            refused.status, refused.body
        );
        let expected = format!("The recipe {recipe} for version {version} wasn't found.");
        assert_eq!(
            error_message(&refused),
            expected,
            "case {version:?}: expected {expected:?}, got {:?}",
            error_message(&refused)
        );
    }
}
