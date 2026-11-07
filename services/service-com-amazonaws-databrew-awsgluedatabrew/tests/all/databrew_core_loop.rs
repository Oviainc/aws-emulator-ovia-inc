// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the DataBrew core loop: dataset -> recipe job -> job run.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a
//! restJson1 request, exactly as the router serves one — so what is asserted is the
//! HTTP status, the error code and the JSON body a client sees, not an internal call.
//! The emulator and the request helpers live in `databrew_harness`, shared with this
//! crate's other suites.
//!
//! The tables below span the classes the implementation generalises over (every
//! `Source` an `Input` can imply, the documented name character set, every page size)
//! rather than one recorded request.

use std::time::Duration;

use serde_json::json;

use crate::databrew_harness::{
    create_dataset, label, s3_input, Emulator, ARN_PREFIX, CALLER_PRINCIPAL,
};

/// `Source` is the input definition the dataset carries, for every value the model
/// declares (`smithy_query(query="shape", shape="Source")`: `S3`, `DATA-CATALOG`,
/// `DATABASE`), and the stored `Input` comes back unchanged. An `Input` carrying none
/// of the three definitions describes no readable data and is refused.
#[tokio::test]
async fn describe_dataset_reports_the_source_its_input_definition_implies() {
    let cases = [
        ("s3-dataset", s3_input("in/data.csv"), "S3"),
        (
            "catalog.dataset",
            json!({"DataCatalogInputDefinition": {"DatabaseName": "sales", "TableName": "orders"}}),
            "DATA-CATALOG",
        ),
        (
            "database dataset",
            json!({"DatabaseInputDefinition": {"GlueConnectionName": "conn", "DatabaseTableName": "orders"}}),
            "DATABASE",
        ),
    ];

    let emulator = Emulator::new();
    for (name, input, expected_source) in cases {
        let created = create_dataset(&emulator, name, input.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: expected CreateDataset to answer 200, got {} {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.text("Name"),
            name,
            "case {name:?}: CreateDataset must echo the name it stored, got {:?}",
            created.body
        );

        let described = emulator
            .call("GET", &format!("/datasets/{}", label(name)), None)
            .await;
        assert_eq!(
            described.status, 200,
            "case {name:?}: expected DescribeDataset to answer 200, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.text("Source"),
            expected_source,
            "case {name:?}: expected Source {expected_source}, got {:?}",
            described.body
        );
        assert_eq!(
            described.body["Input"], input,
            "case {name:?}: expected the stored Input back, got {:?}",
            described.body["Input"]
        );
        assert_eq!(
            described.text("ResourceArn"),
            format!("{ARN_PREFIX}dataset/{name}"),
            "case {name:?}: unexpected ResourceArn, got {:?}",
            described.body
        );
        assert!(
            described.body["CreateDate"].as_f64().unwrap_or_default() > 0.0,
            "case {name:?}: expected a server-generated CreateDate, got {:?}",
            described.body["CreateDate"]
        );
        assert_eq!(
            described.text("CreatedBy"),
            CALLER_PRINCIPAL,
            "case {name:?}: expected CreatedBy to name the calling principal, got {:?}",
            described.body["CreatedBy"]
        );
        assert_eq!(
            described.text("Format"),
            "CSV",
            "case {name:?}: expected the stored Format back, got {:?}",
            described.body
        );
    }

    let refused = create_dataset(&emulator, "no-definition", json!({})).await;
    assert_eq!(
        refused.status, 400,
        "case \"Input with no definition\": expected 400, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_type.as_deref(),
        Some("ValidationException"),
        "case \"Input with no definition\": expected ValidationException, got {:?}",
        refused.error_type
    );
}

/// The character set the model documents for a resource name — "Valid characters are
/// alphanumeric (A-Z, a-z, 0-9), hyphen (-), period (.), and space"
/// (`smithy_query(query="doc", shape="CreateDatasetRequest", member="Name")`) — is
/// what a name is accepted or refused on. A name of nothing but spaces names nothing.
#[tokio::test]
async fn a_name_outside_the_documented_character_set_is_refused() {
    let accepted = [
        "plain",
        "with-hyphen",
        "with.period",
        "with space",
        "MiXeD123",
    ];
    let refused = [
        "under_score",
        "with/slash",
        "with$dollar",
        "with:colon",
        "   ",
    ];

    let emulator = Emulator::new();
    for name in accepted {
        let created = create_dataset(&emulator, name, s3_input("in/data.csv")).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: a name inside the documented set must be accepted, got {} {:?}",
            created.status, created.body
        );
        let described = emulator
            .call("GET", &format!("/datasets/{}", label(name)), None)
            .await;
        assert_eq!(
            described.status, 200,
            "case {name:?}: the accepted name must read back, got {} {:?}",
            described.status, described.body
        );
    }

    for name in refused {
        let created = create_dataset(&emulator, name, s3_input("in/data.csv")).await;
        assert_eq!(
            created.status, 400,
            "case {name:?}: a name outside the documented set must be refused with 400, got {} {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.error_type.as_deref(),
            Some("ValidationException"),
            "case {name:?}: expected ValidationException, got {:?}",
            created.error_type
        );
    }

    // A refused create stored nothing, so only the accepted names are listed.
    let listed = emulator.call("GET", "/datasets", None).await;
    let names: Vec<String> = listed.body["Datasets"]
        .as_array()
        .expect("Datasets is a required list")
        .iter()
        .map(|dataset| dataset["Name"].as_str().unwrap_or_default().to_string())
        .collect();
    let mut expected: Vec<String> = accepted.iter().map(ToString::to_string).collect();
    expected.sort();
    assert_eq!(
        names, expected,
        "expected only the accepted names to be stored, got {names:?}"
    );
}

/// A second create of a name already stored is a 409 `ConflictException`, and it
/// leaves the stored record alone.
#[tokio::test]
async fn creating_a_dataset_twice_conflicts_and_keeps_the_first_record() {
    let emulator = Emulator::new();
    assert_eq!(
        create_dataset(&emulator, "demo-ds", s3_input("first/data.csv"))
            .await
            .status,
        200,
        "the first create must succeed"
    );

    let again = create_dataset(&emulator, "demo-ds", s3_input("second/data.csv")).await;
    assert_eq!(
        again.status, 409,
        "expected the duplicate create to answer 409, got {} {:?}",
        again.status, again.body
    );
    assert_eq!(
        again.error_type.as_deref(),
        Some("ConflictException"),
        "expected ConflictException, got {:?}",
        again.error_type
    );
    // The message names the resource kind and the name the request carried, so a
    // client is told WHICH create conflicted.
    assert_eq!(
        again.text("Message"),
        "Dataset demo-ds already exists.",
        "expected the conflict message to name the dataset, got {:?}",
        again.body
    );

    let described = emulator.call("GET", "/datasets/demo-ds", None).await;
    assert_eq!(
        described.body["Input"],
        s3_input("first/data.csv"),
        "the conflicting create must not overwrite the stored record, got {:?}",
        described.body["Input"]
    );
}

/// `ListDatasets` walks the stored map: every page size reaches every dataset exactly
/// once, in name order, and the last page carries no token. The page size spans the
/// published bounds of `maxResults` (model: range 1..100) and the absent case.
#[tokio::test]
async fn list_datasets_pages_through_every_stored_dataset() {
    let emulator = Emulator::new();
    let stored: Vec<String> = (1..=7).map(|n| format!("ds-{n:02}")).collect();
    for name in &stored {
        assert_eq!(
            create_dataset(&emulator, name, s3_input("in/data.csv"))
                .await
                .status,
            200,
            "case {name:?}: the dataset must be created for the listing cases"
        );
    }

    for page_size in [None, Some(1), Some(2), Some(3), Some(7), Some(100)] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query: Vec<String> = Vec::new();
            if let Some(size) = page_size {
                query.push(format!("maxResults={size}"));
            }
            if let Some(token) = &token {
                query.push(format!("nextToken={token}"));
            }
            let uri = if query.is_empty() {
                "/datasets".to_string()
            } else {
                format!("/datasets?{}", query.join("&"))
            };

            let listed = emulator.call("GET", &uri, None).await;
            assert_eq!(
                listed.status, 200,
                "case {page_size:?}: expected 200 from {uri}, got {} {:?}",
                listed.status, listed.body
            );
            let page: Vec<String> = listed.body["Datasets"]
                .as_array()
                .expect("Datasets is a required list")
                .iter()
                .map(|dataset| dataset["Name"].as_str().unwrap_or_default().to_string())
                .collect();
            if let Some(size) = page_size {
                assert!(
                    page.len() <= size as usize,
                    "case {page_size:?}: a page held {} datasets, more than maxResults",
                    page.len()
                );
            }
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= stored.len() + 1,
                "case {page_size:?}: paging did not terminate after {pages} pages, saw {seen:?}"
            );

            let next = listed.text("NextToken");
            token = (!next.is_empty()).then_some(next);
            if token.is_none() {
                break;
            }
        }

        assert_eq!(
            seen, stored,
            "case {page_size:?}: expected every stored dataset exactly once in name order, got {seen:?}"
        );
    }

    // A token this service did not issue is a client error, not an ignored input.
    let bogus = emulator.call("GET", "/datasets?nextToken=zzz-not-a-token", None).await;
    assert_eq!(
        bogus.status, 400,
        "case \"foreign nextToken\": expected 400, got {} {:?}",
        bogus.status, bogus.body
    );
    assert_eq!(
        bogus.error_type.as_deref(),
        Some("ValidationException"),
        "case \"foreign nextToken\": expected ValidationException, got {:?}",
        bogus.error_type
    );
}

/// The whole loop the issue describes: a dataset, a recipe job over it, a run of that
/// job, the run read back, the run listed, then the dataset deleted. Each step reads
/// what the previous step stored, and each resource that does not exist is a 404.
#[tokio::test]
async fn the_dataset_recipe_job_and_job_run_loop_is_served_from_state() {
    let emulator = Emulator::new();
    let outputs = json!([{"Location": {"Bucket": "brew-bucket", "Key": "out/"}}]);
    // `Output$Overwrite` carries `@default: false`, so the stored output reports it
    // explicitly even when the request left it out.
    let stored_outputs =
        json!([{"Location": {"Bucket": "brew-bucket", "Key": "out/"}, "Overwrite": false}]);
    // The model names `RecipeReference`'s members `Name` and `RecipeVersion` (the
    // issue's `RecipeName` is not a member of that shape).
    let recipe_reference = json!({"Name": "demo-recipe", "RecipeVersion": "1.0"});
    let job_body = json!({
        "Name": "demo-job",
        "RoleArn": "arn:aws:iam::000000000000:role/brew",
        "DatasetName": "demo-ds",
        "Outputs": outputs,
        "RecipeReference": recipe_reference,
        "LogSubscription": "ENABLE",
        "MaxRetries": 2,
        "Timeout": 2880,
    });

    // A job over a dataset nobody created resolves nothing.
    let orphan = emulator
        .call("POST", "/recipeJobs", Some(job_body.clone()))
        .await;
    assert_eq!(
        orphan.status, 404,
        "case \"job over an absent dataset\": expected 404, got {} {:?}",
        orphan.status, orphan.body
    );
    assert_eq!(
        orphan.error_type.as_deref(),
        Some("ResourceNotFoundException"),
        "case \"job over an absent dataset\": expected ResourceNotFoundException, got {:?}",
        orphan.error_type
    );

    assert_eq!(
        create_dataset(&emulator, "demo-ds", s3_input("in/data.csv"))
            .await
            .status,
        200,
        "the dataset must be created"
    );

    let created_job = emulator
        .call("POST", "/recipeJobs", Some(job_body.clone()))
        .await;
    assert_eq!(
        created_job.status, 200,
        "expected CreateRecipeJob to answer 200, got {} {:?}",
        created_job.status, created_job.body
    );
    assert_eq!(
        created_job.text("Name"),
        "demo-job",
        "CreateRecipeJob must echo the job it stored, got {:?}",
        created_job.body
    );

    let duplicate_job = emulator.call("POST", "/recipeJobs", Some(job_body)).await;
    assert_eq!(
        duplicate_job.status, 409,
        "case \"duplicate job name\": expected 409, got {} {:?}",
        duplicate_job.status, duplicate_job.body
    );

    // The create round trip: the job reads back through DescribeJob exactly as stored.
    let described_job = emulator.call("GET", "/jobs/demo-job", None).await;
    assert_eq!(
        described_job.status, 200,
        "expected DescribeJob to answer 200, got {} {:?}",
        described_job.status, described_job.body
    );
    for (member, expected) in [
        ("Name", json!("demo-job")),
        ("Type", json!("RECIPE")),
        ("DatasetName", json!("demo-ds")),
        ("RoleArn", json!("arn:aws:iam::000000000000:role/brew")),
        ("LogSubscription", json!("ENABLE")),
        ("MaxRetries", json!(2)),
        ("Timeout", json!(2880)),
        ("Outputs", stored_outputs.clone()),
        ("RecipeReference", recipe_reference.clone()),
        ("ResourceArn", json!(format!("{ARN_PREFIX}job/demo-job"))),
    ] {
        assert_eq!(
            described_job.body[member], expected,
            "case {member:?}: expected the stored job's {member} back, got {:?}",
            described_job.body[member]
        );
    }
    assert_eq!(
        described_job.text("CreatedBy"),
        CALLER_PRINCIPAL,
        "expected CreatedBy to name the calling principal, got {:?}",
        described_job.body["CreatedBy"]
    );

    let absent_described_job = emulator.call("GET", "/jobs/absent-job", None).await;
    assert_eq!(
        absent_described_job.status, 404,
        "case \"describe an absent job\": expected 404, got {} {:?}",
        absent_described_job.status, absent_described_job.body
    );

    // A run of a job nobody created resolves nothing either.
    let absent_job = emulator
        .call("POST", "/jobs/absent-job/startJobRun", None)
        .await;
    assert_eq!(
        absent_job.status, 404,
        "case \"run of an absent job\": expected 404, got {} {:?}",
        absent_job.status, absent_job.body
    );

    let started = emulator
        .call("POST", "/jobs/demo-job/startJobRun", None)
        .await;
    assert_eq!(
        started.status, 200,
        "expected StartJobRun to answer 200, got {} {:?}",
        started.status, started.body
    );
    let run_id = started.text("RunId");
    assert!(
        !run_id.is_empty(),
        "StartJobRun must mint a RunId, got {:?}",
        started.body
    );

    let described = emulator
        .call("GET", &format!("/jobs/demo-job/jobRun/{run_id}"), None)
        .await;
    assert_eq!(
        described.status, 200,
        "the minted RunId must resolve, got {} {:?}",
        described.status, described.body
    );
    assert_eq!(
        described.text("RunId"),
        run_id,
        "DescribeJobRun must report the run that was started, got {:?}",
        described.body
    );
    assert_eq!(
        described.text("JobName"),
        "demo-job",
        "expected JobName demo-job, got {:?}",
        described.body
    );
    assert_eq!(
        described.text("DatasetName"),
        "demo-ds",
        "expected the job's DatasetName on the run, got {:?}",
        described.body
    );
    assert_eq!(
        described.body["Outputs"], stored_outputs,
        "expected the job's Outputs copied onto the run, got {:?}",
        described.body["Outputs"]
    );
    assert_eq!(
        described.body["RecipeReference"], recipe_reference,
        "expected the job's RecipeReference copied onto the run, got {:?}",
        described.body["RecipeReference"]
    );
    assert_eq!(
        described.text("LogSubscription"),
        "ENABLE",
        "expected the job's LogSubscription on the run, got {:?}",
        described.body
    );
    assert!(
        described.body["StartedOn"].as_f64().unwrap_or_default() > 0.0,
        "expected a server-generated StartedOn, got {:?}",
        described.body["StartedOn"]
    );
    assert_eq!(
        described.text("StartedBy"),
        CALLER_PRINCIPAL,
        "expected StartedBy to name the calling principal, got {:?}",
        described.body["StartedBy"]
    );

    // A run id nobody minted resolves nothing.
    let absent_run = emulator
        .call("GET", "/jobs/demo-job/jobRun/db_000000000000000000000099", None)
        .await;
    assert_eq!(
        absent_run.status, 404,
        "case \"absent run id\": expected 404, got {} {:?}",
        absent_run.status, absent_run.body
    );

    // A second run is a second stored record under its own id.
    let second = emulator
        .call("POST", "/jobs/demo-job/startJobRun", None)
        .await;
    let second_run_id = second.text("RunId");
    assert_ne!(
        second_run_id, run_id,
        "each StartJobRun must mint its own id, got {second_run_id:?} twice"
    );

    let listed = emulator.call("GET", "/jobs/demo-job/jobRuns", None).await;
    assert_eq!(
        listed.status, 200,
        "expected ListJobRuns to answer 200, got {} {:?}",
        listed.status, listed.body
    );
    let run_ids: Vec<String> = listed.body["JobRuns"]
        .as_array()
        .expect("JobRuns is a required list")
        .iter()
        .map(|run| run["RunId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        run_ids,
        vec![run_id.clone(), second_run_id],
        "expected both stored runs listed in id order, got {run_ids:?}"
    );

    // Deleting the dataset removes the record: the describe that follows 404s and the
    // listing no longer carries it.
    let deleted = emulator.call("DELETE", "/datasets/demo-ds", None).await;
    assert_eq!(
        deleted.status, 200,
        "expected DeleteDataset to answer 200, got {} {:?}",
        deleted.status, deleted.body
    );
    assert_eq!(
        deleted.text("Name"),
        "demo-ds",
        "DeleteDataset must name the dataset it removed, got {:?}",
        deleted.body
    );

    let gone = emulator.call("GET", "/datasets/demo-ds", None).await;
    assert_eq!(
        gone.status, 404,
        "expected the deleted dataset to be gone, got {} {:?}",
        gone.status, gone.body
    );
    assert_eq!(
        gone.error_type.as_deref(),
        Some("ResourceNotFoundException"),
        "expected ResourceNotFoundException, got {:?}",
        gone.error_type
    );

    let after_delete = emulator.call("GET", "/datasets", None).await;
    assert_eq!(
        after_delete.body["Datasets"],
        json!([]),
        "expected the deleted dataset gone from the listing, got {:?}",
        after_delete.body["Datasets"]
    );

    let deleted_again = emulator.call("DELETE", "/datasets/demo-ds", None).await;
    assert_eq!(
        deleted_again.status, 404,
        "expected a second delete to 404, got {} {:?}",
        deleted_again.status, deleted_again.body
    );
}

/// Every store a caller can grow is bounded: a run history is a rolling window of
/// the newest [`MAX_RUNS_PER_JOB`](service_com_amazonaws_databrew_awsgluedatabrew::state::MAX_RUNS_PER_JOB)
/// runs, and a create past the retained dataset/job count is refused with the
/// modelled `ServiceQuotaExceededException` instead of inserting.
#[tokio::test]
async fn the_stores_a_caller_can_grow_are_bounded() {
    use service_com_amazonaws_databrew_awsgluedatabrew::state::{
        MAX_DATASETS, MAX_JOBS, MAX_RUNS_PER_JOB,
    };

    // Case 1: the run history is a rolling window, oldest evicted first.
    let emulator = Emulator::new();
    assert_eq!(
        create_dataset(&emulator, "demo-ds", s3_input("in/data.csv"))
            .await
            .status,
        200,
        "the dataset must be created"
    );
    assert_eq!(
        emulator
            .call(
                "POST",
                "/recipeJobs",
                Some(json!({
                    "Name": "demo-job",
                    "RoleArn": "arn:aws:iam::000000000000:role/brew",
                    "DatasetName": "demo-ds",
                    "Outputs": [{"Location": {"Bucket": "brew-bucket", "Key": "out/"}}],
                })),
            )
            .await
            .status,
        200,
        "the job must be created"
    );

    let mut run_ids = Vec::new();
    for _ in 0..MAX_RUNS_PER_JOB + 3 {
        let started = emulator
            .call("POST", "/jobs/demo-job/startJobRun", None)
            .await;
        assert_eq!(
            started.status, 200,
            "every StartJobRun must be served, got {} {:?}",
            started.status, started.body
        );
        run_ids.push(started.text("RunId"));
    }

    let mut listed_runs = 0;
    let mut token: Option<String> = None;
    loop {
        let uri = match &token {
            Some(token) => format!("/jobs/demo-job/jobRuns?maxResults=100&nextToken={token}"),
            None => "/jobs/demo-job/jobRuns?maxResults=100".to_string(),
        };
        let listed = emulator.call("GET", &uri, None).await;
        listed_runs += listed.body["JobRuns"]
            .as_array()
            .expect("JobRuns is a required list")
            .len();
        let next = listed.text("NextToken");
        token = (!next.is_empty()).then_some(next);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        listed_runs, MAX_RUNS_PER_JOB,
        "expected the retained run window to hold exactly {MAX_RUNS_PER_JOB} runs, got {listed_runs}"
    );

    let evicted = emulator
        .call("GET", &format!("/jobs/demo-job/jobRun/{}", run_ids[0]), None)
        .await;
    assert_eq!(
        evicted.status, 404,
        "case \"oldest run\": expected the evicted run to be gone, got {} {:?}",
        evicted.status, evicted.body
    );
    let newest = run_ids.last().expect("a run was started");
    let retained = emulator
        .call("GET", &format!("/jobs/demo-job/jobRun/{newest}"), None)
        .await;
    assert_eq!(
        retained.status, 200,
        "case \"newest run\": the newest run must be retained, got {} {:?}",
        retained.status, retained.body
    );

    // Case 2: the dataset count has a ceiling, and the create past it is refused.
    let emulator = Emulator::new();
    for n in 0..MAX_DATASETS {
        let created = create_dataset(&emulator, &format!("ds-{n:06}"), s3_input("in/d.csv")).await;
        assert_eq!(
            created.status, 200,
            "case {n}: a create inside the ceiling must be served, got {} {:?}",
            created.status, created.body
        );
    }
    let over = create_dataset(&emulator, "one-too-many", s3_input("in/d.csv")).await;
    assert_eq!(
        over.status, 402,
        "case \"dataset past the ceiling\": expected 402, got {} {:?}",
        over.status, over.body
    );
    assert_eq!(
        over.error_type.as_deref(),
        Some("ServiceQuotaExceededException"),
        "case \"dataset past the ceiling\": expected ServiceQuotaExceededException, got {:?}",
        over.error_type
    );
    assert_eq!(
        emulator.call("GET", "/datasets/one-too-many", None).await.status,
        404,
        "case \"dataset past the ceiling\": the refused create must store nothing"
    );

    // Case 3: the job count has the same ceiling.
    let emulator = Emulator::new();
    for n in 0..MAX_JOBS {
        let created = emulator
            .call(
                "POST",
                "/recipeJobs",
                Some(json!({
                    "Name": format!("job-{n:06}"),
                    "RoleArn": "arn:aws:iam::000000000000:role/brew",
                    "Outputs": [{"Location": {"Bucket": "brew-bucket", "Key": "out/"}}],
                })),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {n}: a job inside the ceiling must be served, got {} {:?}",
            created.status, created.body
        );
    }
    let over = emulator
        .call(
            "POST",
            "/recipeJobs",
            Some(json!({
                "Name": "one-too-many",
                "RoleArn": "arn:aws:iam::000000000000:role/brew",
                "Outputs": [{"Location": {"Bucket": "brew-bucket", "Key": "out/"}}],
            })),
        )
        .await;
    assert_eq!(
        over.status, 402,
        "case \"job past the ceiling\": expected 402, got {} {:?}",
        over.status, over.body
    );
    assert_eq!(
        over.error_type.as_deref(),
        Some("ServiceQuotaExceededException"),
        "case \"job past the ceiling\": expected ServiceQuotaExceededException, got {:?}",
        over.error_type
    );
}

/// A run is returned non-terminal and reaches a terminal state on its own: the
/// transition is owed to the clock, so a caller that describes the run immediately
/// still sees `STARTING`, and one that waits sees the terminal `SUCCEEDED` with the
/// `CompletedOn`/`ExecutionTime` that go with it. Once terminal it stays put.
///
/// The `SUCCEEDED` it reaches is backed by the object the run wrote, so the emulator is
/// assembled with its own Amazon S3 beside it and the dataset's object is staged there;
/// what the run writes is the subject of `crate::recipe_job_execution`.
#[tokio::test]
async fn a_job_run_progresses_to_a_terminal_state_on_its_own() {
    let emulator = Emulator::with_object_store_and_iam();
    emulator.given_bucket("brew-bucket").await;
    emulator
        .given_object("brew-bucket", "in/data.csv", "name,size\nhalyard,2\n")
        .await;
    assert_eq!(
        create_dataset(&emulator, "demo-ds", s3_input("in/data.csv"))
            .await
            .status,
        200,
        "the dataset must be created"
    );
    assert_eq!(
        emulator
            .call(
                "POST",
                "/recipeJobs",
                Some(json!({
                    "Name": "demo-job",
                    "RoleArn": "arn:aws:iam::000000000000:role/brew",
                    "DatasetName": "demo-ds",
                    "Outputs": [{"Location": {"Bucket": "brew-bucket", "Key": "out/"}}],
                })),
            )
            .await
            .status,
        200,
        "the job must be created"
    );

    let run_id = emulator
        .call("POST", "/jobs/demo-job/startJobRun", None)
        .await
        .text("RunId");
    let run_uri = format!("/jobs/demo-job/jobRun/{run_id}");

    let fresh = emulator.call("GET", &run_uri, None).await;
    assert_eq!(
        fresh.text("State"),
        "STARTING",
        "case \"read straight after StartJobRun\": expected STARTING, got {:?}",
        fresh.body
    );
    assert!(
        fresh.body.get("CompletedOn").is_none(),
        "case \"read straight after StartJobRun\": a non-terminal run has no CompletedOn, got {:?}",
        fresh.body["CompletedOn"]
    );

    // The transition is due a sub-second span after the start; nothing sleeps on the
    // request path, so the wait belongs here.
    std::thread::sleep(Duration::from_millis(900));

    let settled = emulator.call("GET", &run_uri, None).await;
    assert_eq!(
        settled.text("State"),
        "SUCCEEDED",
        "case \"read after the transition fell due\": expected SUCCEEDED, got {:?}",
        settled.body
    );
    let completed_on = settled.body["CompletedOn"].as_f64().unwrap_or_default();
    let started_on = settled.body["StartedOn"].as_f64().unwrap_or_default();
    assert!(
        completed_on >= started_on && started_on > 0.0,
        "case \"terminal run\": expected CompletedOn at or after StartedOn, got {started_on} -> {completed_on}"
    );
    assert!(
        settled.body["ExecutionTime"].as_i64().unwrap_or(-1) >= 0,
        "case \"terminal run\": expected the seconds the run consumed, got {:?}",
        settled.body["ExecutionTime"]
    );
    assert_eq!(
        settled.text("StartedBy"),
        fresh.text("StartedBy"),
        "case \"terminal run\": StartedBy must not change as the run progresses"
    );

    let again = emulator.call("GET", &run_uri, None).await;
    assert_eq!(
        again.text("State"),
        "SUCCEEDED",
        "case \"re-read a terminal run\": a terminal state must not move, got {:?}",
        again.body
    );
    assert_eq!(
        again.body["CompletedOn"], settled.body["CompletedOn"],
        "case \"re-read a terminal run\": CompletedOn must be stable across reads, got {:?}",
        again.body["CompletedOn"]
    );

    let listed = emulator.call("GET", "/jobs/demo-job/jobRuns", None).await;
    assert_eq!(
        listed.body["JobRuns"][0]["State"],
        json!("SUCCEEDED"),
        "ListJobRuns must report the same state as DescribeJobRun, got {:?}",
        listed.body["JobRuns"]
    );

    // The success is backed: the run wrote its output under the output location's key.
    assert_eq!(
        emulator.keys_under("brew-bucket", "out/").await,
        vec!["out/demo-job_part00000.csv".to_string()],
        "case \"terminal run\": a SUCCEEDED run must have written its output object"
    );
}
