// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListPipelines`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the wire
//! behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * a listing enumerates exactly the pipelines the writes of the workspace the request
//!   names persisted, and no pipeline of another workspace;
//! * every summary is read off the stored pipeline, so a listed pipeline and the same
//!   pipeline read through `DescribePipeline` never disagree, and a listing REPORTS: it
//!   moves no pipeline on;
//! * an accepted write is visible to the next listing — the version a listed pipeline
//!   reports is its current one, with that version's description and update instant —
//!   and a deleted pipeline is listed no more;
//! * `maxResults`/`nextToken` page one stable enumeration: the pages joined are the
//!   whole listing once, in one order, whatever page size the caller asks for, and a
//!   token this listing never issued is refused;
//! * a workspace this service holds no record of is the modelled not-found refusal,
//!   not an empty page, while a held workspace with no pipeline is an empty page.
//!
//! The HTTP harness (`call`, `router`, `error_code`, `hex_token`) and the pipeline
//! routes are the ones the sibling suites already have; this suite reuses them rather
//! than growing a second copy.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    ACTIVATION_DELAY, FIRST_PIPELINE_VERSION,
};

use crate::application_lifecycle::create_workspace;
use crate::asset_lifecycle::{call, error_code, hex_token, router};
use crate::pipeline_lifecycle::{
    create, describe, expected_arn, node, pipeline_uri, pipelines_uri,
};

/// The workspace every suite below lists unless it is about workspace scoping.
const WORKSPACE: &str = "plant-floor";

/// A second workspace, so a listing can be shown to report only the pipelines of the
/// one it names.
const OTHER_WORKSPACE: &str = "plant-roof";

/// Long enough that the transition a create or an update owes has fallen due, so every
/// pipeline the suite wrote reports its settled state.
fn settled() -> Duration {
    ACTIVATION_DELAY + Duration::from_millis(200)
}

/// One row of the fixture: the pipeline a create writes.
///
/// The rows differ along what a summary reports and along what it must NOT depend on:
/// the name, a present / absent description, and `computations` spanning the DAG shapes
/// `ComputeNodeList` admits (none, one, independent nodes, a chain) — so what a listing
/// reports cannot be an echo of one request's shape.
struct PipelineCase {
    name: &'static str,
    description: Option<&'static str>,
    computations: Value,
}

/// The rows every suite writes into `WORKSPACE`, in the order they are created.
fn cases() -> Vec<PipelineCase> {
    vec![
        PipelineCase {
            name: "ingest",
            description: Some("the ingest pipeline"),
            computations: json!([node("only", "clean", &[])]),
        },
        PipelineCase {
            name: "Rollup",
            description: None,
            computations: json!([]),
        },
        PipelineCase {
            name: "edge-2",
            description: Some("two independent nodes"),
            computations: json!([node("left", "clean", &[]), node("right", "score", &[])]),
        },
        PipelineCase {
            name: "chain_3",
            description: None,
            computations: json!([
                node("first", "clean", &[]),
                node("second", "score", &["first"]),
                node("third", "store", &["second"]),
            ]),
        },
        PipelineCase {
            name: "a",
            description: Some("a one-character name"),
            computations: json!([node("only", "store", &[])]),
        },
        PipelineCase {
            name: "ZZ-last",
            description: Some("a name late in every collation"),
            computations: json!([node("only", "clean", &[])]),
        },
        PipelineCase {
            name: "metrics-99",
            description: None,
            computations: json!([node("only", "score", &[])]),
        },
    ]
}

/// Write every row of `cases()` into `workspace`, asserting each create was accepted,
/// and the names in the order they were created.
async fn write_fixture(router: &MultiServiceRouter, workspace: &str) -> Vec<String> {
    let mut written = Vec::new();
    for case in cases() {
        let mut body = json!({
            "pipelineName": case.name,
            "computations": case.computations,
        });
        if let Some(description) = case.description {
            body.as_object_mut()
                .expect("an object body")
                .insert("description".to_string(), json!(description));
        }
        let (status, created) = create(router, workspace, body).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreatePipeline in {workspace} got {status} {created}",
            case.name
        );
        written.push(case.name.to_string());
    }
    written
}

/// `ListPipelines` on `workspace` with the raw query string `query` (empty for none).
async fn list_raw(router: &MultiServiceRouter, workspace: &str, query: &str) -> (u16, Value) {
    let uri = if query.is_empty() {
        pipelines_uri(workspace)
    } else {
        format!("{}?{query}", pipelines_uri(workspace))
    };
    call(router, "GET", &uri, None).await
}

/// `ListPipelines`, asserting it answered and that the required `pipelineSummaries`
/// member is a list, and the body it answered with.
async fn listed(router: &MultiServiceRouter, workspace: &str, query: &str) -> Value {
    let (status, body) = list_raw(router, workspace, query).await;
    assert_eq!(
        status, 200,
        "list {workspace} ?{query}: expected 200, got {status} {body}"
    );
    assert!(
        body["pipelineSummaries"].is_array(),
        "list {workspace} ?{query}: pipelineSummaries is a required list, got {body}"
    );
    body
}

/// The summaries a listing body reports, in the order it reported them.
fn summaries(body: &Value) -> &Vec<Value> {
    body["pipelineSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("a listing holds a pipelineSummaries list, got {body}"))
}

/// The pipeline names a listing body reports, in the order it reported them.
fn names(body: &Value) -> Vec<String> {
    summaries(body)
        .iter()
        .map(|summary| {
            summary["pipelineName"]
                .as_str()
                .unwrap_or_else(|| panic!("a summary carries a pipelineName, got {summary}"))
                .to_string()
        })
        .collect()
}

/// Every page of a listing of `workspace`, walked with `page_size` when given and with
/// no `maxResults` otherwise: the names in order, and how many pages it took.
async fn walk(
    router: &MultiServiceRouter,
    workspace: &str,
    page_size: Option<i64>,
    expected_total: usize,
) -> (Vec<String>, usize) {
    let case = match page_size {
        Some(size) => format!("maxResults={size}"),
        None => "the default page size".to_string(),
    };
    let mut collected: Vec<String> = Vec::new();
    let mut pages = 0usize;
    let mut token: Option<String> = None;
    loop {
        let mut query: Vec<String> = Vec::new();
        if let Some(size) = page_size {
            query.push(format!("maxResults={size}"));
        }
        if let Some(token) = &token {
            query.push(format!("nextToken={token}"));
        }
        let body = listed(router, workspace, &query.join("&")).await;
        let page = names(&body);
        pages += 1;
        if let Some(size) = page_size {
            assert!(
                page.len() as i64 <= size,
                "case {case:?}: page {pages} holds {} summaries, more than asked: {body}",
                page.len()
            );
        }
        collected.extend(page);
        match body["nextToken"].as_str() {
            Some(next) => {
                // A page followed by a token has to leave something to resume from.
                assert!(
                    collected.len() < expected_total,
                    "case {case:?}: page {pages} ends the listing yet carries {next}"
                );
                token = Some(next.to_string());
            }
            None => break,
        }
        assert!(
            pages <= expected_total + 2,
            "case {case:?}: paging did not terminate after {pages} pages"
        );
    }
    (collected, pages)
}

#[tokio::test]
async fn a_listing_reports_the_pipelines_of_the_workspace_it_names() {
    let router = router();

    let here = write_fixture(&router, WORKSPACE).await;
    let there = write_fixture(&router, OTHER_WORKSPACE).await;
    // A workspace this service holds a record of, that no create ever wrote a pipeline
    // into.
    create_workspace(&router, "plant-empty").await;

    for (case, workspace, expected) in [
        ("the workspace the pipelines were created in", WORKSPACE, &here),
        ("another workspace with pipelines of its own", OTHER_WORKSPACE, &there),
    ] {
        let body = listed(&router, workspace, "maxResults=250").await;
        let reported: BTreeSet<String> = names(&body).into_iter().collect();
        let wanted: BTreeSet<String> = expected.iter().cloned().collect();
        assert_eq!(
            reported, wanted,
            "case {case:?}: a listing reports exactly that workspace's pipelines, got {body}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case {case:?}: a listing that reported everything carries no nextToken, got {body}"
        );
        // A pipeline name is unique only within its workspace, so the ARN a summary
        // reports has to carry the workspace the listing was scoped to.
        for summary in summaries(&body) {
            let name = summary["pipelineName"].as_str().unwrap_or_default();
            assert_eq!(
                summary["pipelineArn"].as_str().unwrap_or_default(),
                expected_arn(workspace, name),
                "case {case:?}: the ARN of {name} must carry its own workspace, got {summary}"
            );
        }
    }

    // A held workspace nobody created a pipeline in is an EMPTY page — absence of
    // pipelines is not absence of the workspace.
    let empty = listed(&router, "plant-empty", "").await;
    assert!(
        summaries(&empty).is_empty() && empty["nextToken"].is_null(),
        "a workspace with no pipeline answers an empty page, got {empty}"
    );

    // A workspace this service holds no record of, in every form of a name neither a
    // workspace nor a pipeline was ever written under — including ones differing from a
    // held name only in case, since a workspace is named exactly. The workspace the PATH
    // names is resolved before the page is cut, so a request that is wrong in both ways
    // still reports the resource that is absent.
    for name in ["plant-9", "PLANT-FLOOR", "Plant-Floor", "plant-floor-copy"] {
        for query in ["", "maxResults=1", "nextToken=not-hex"] {
            let (status, body) = list_raw(&router, name, query).await;
            assert_eq!(
                status, 404,
                "workspace {name:?} ?{query}: expected 404, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "workspace {name:?} ?{query}: unexpected error code in {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_summary_agrees_with_a_read_of_that_one_pipeline() {
    let router = router();
    let written = write_fixture(&router, WORKSPACE).await;

    // A listing REPORTS: two listings of one unchanged store agree, and neither moves a
    // pipeline on.
    let first = listed(&router, WORKSPACE, "maxResults=250").await;
    let again = listed(&router, WORKSPACE, "maxResults=250").await;
    assert_eq!(
        names(&again),
        names(&first),
        "two listings of one store must report the same pipelines in the same order"
    );

    // No pipeline has been written twice, so every one of them is at its first version.
    for summary in summaries(&first) {
        let name = summary["pipelineName"].as_str().unwrap_or_default();
        assert_eq!(
            summary["version"].as_str(),
            Some(FIRST_PIPELINE_VERSION.to_string().as_str()),
            "case {name}: a freshly created pipeline is at its first version, got {summary}"
        );
    }

    // A create owes a transition the clock makes due, so the pipeline of a create this
    // listing immediately follows is still settling. Asserted on ONE freshly created
    // pipeline, so the assertion does not depend on how long the whole fixture took to
    // write.
    let settling_name = "just-created";
    let (status, created) = create(
        &router,
        WORKSPACE,
        json!({
            "pipelineName": settling_name,
            "computations": [node("only", "clean", &[])],
        }),
    )
    .await;
    assert_eq!(status, 200, "CreatePipeline got {status} {created}");
    let settling = listed(&router, WORKSPACE, "maxResults=250").await;
    let summary = summaries(&settling)
        .iter()
        .find(|summary| summary["pipelineName"].as_str() == Some(settling_name))
        .unwrap_or_else(|| panic!("the created pipeline is listed, got {settling}"))
        .clone();
    assert_eq!(
        summary["status"]["state"].as_str(),
        Some("CREATING"),
        "case {settling_name}: a pipeline read straight after its create is still settling, \
         got {summary}"
    );

    tokio::time::sleep(settled()).await;

    let body = listed(&router, WORKSPACE, "maxResults=250").await;
    assert_eq!(
        summaries(&body).len(),
        written.len() + 1,
        "every created pipeline is still listed once, got {body}"
    );
    for summary in summaries(&body) {
        let name = summary["pipelineName"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            summary["status"]["state"].as_str(),
            Some("ACTIVE"),
            "case {name}: the transition the create owed has fallen due, got {summary}"
        );
        // The same pipeline, read on its own: a listing and a read of one stored
        // pipeline cannot disagree about any member they both report.
        let (status, read) = describe(&router, WORKSPACE, &name, None).await;
        assert_eq!(status, 200, "case {name}: DescribePipeline got {status} {read}");
        for member in [
            "pipelineName",
            "pipelineArn",
            "version",
            "description",
            "createdAt",
            "updatedAt",
            "status",
        ] {
            assert_eq!(
                summary.get(member),
                read.get(member),
                "case {name}: the summary {summary} disagrees with the read {read} on {member}"
            );
        }
    }
}

#[tokio::test]
async fn a_listing_follows_the_writes_of_the_pipelines_it_reports() {
    let router = router();
    write_fixture(&router, WORKSPACE).await;
    tokio::time::sleep(settled()).await;

    // An accepted update APPENDS a version, so the next listing reports the new version
    // and that version's description — never the configuration it replaced.
    let updated_name = "ingest";
    let (status, updated) = call(
        &router,
        "PUT",
        &pipeline_uri(WORKSPACE, updated_name),
        Some(json!({ "description": "the ingest pipeline, rewritten" })),
    )
    .await;
    assert_eq!(status, 200, "UpdatePipeline got {status} {updated}");

    let body = listed(&router, WORKSPACE, "maxResults=250").await;
    let summary = summaries(&body)
        .iter()
        .find(|summary| summary["pipelineName"].as_str() == Some(updated_name))
        .unwrap_or_else(|| panic!("the updated pipeline is still listed, got {body}"))
        .clone();
    assert_eq!(
        summary["version"].as_str(),
        Some((FIRST_PIPELINE_VERSION + 1).to_string().as_str()),
        "case {updated_name}: a listing reports the version the update wrote, got {summary}"
    );
    assert_eq!(
        summary["description"].as_str(),
        Some("the ingest pipeline, rewritten"),
        "case {updated_name}: a listing reports the current version's description, got {summary}"
    );
    assert_eq!(
        summary["status"]["state"].as_str(),
        Some("UPDATING"),
        "case {updated_name}: a pipeline read straight after an update is still settling, \
         got {summary}"
    );
    assert!(
        summary["updatedAt"].as_f64().unwrap_or(-1.0) > summary["createdAt"].as_f64().unwrap_or(0.0),
        "case {updated_name}: the update instant follows the create instant, got {summary}"
    );
    // Every other pipeline is untouched by that one write.
    for other in summaries(&body)
        .iter()
        .filter(|summary| summary["pipelineName"].as_str() != Some(updated_name))
    {
        let name = other["pipelineName"].as_str().unwrap_or_default();
        assert_eq!(
            other["version"].as_str(),
            Some(FIRST_PIPELINE_VERSION.to_string().as_str()),
            "case {name}: updating one pipeline must not bump another's version, got {other}"
        );
    }

    // A deleted pipeline is listed no more, and every other one still is.
    let deleted_name = "Rollup";
    let (status, body) = call(
        &router,
        "DELETE",
        &pipeline_uri(WORKSPACE, deleted_name),
        None,
    )
    .await;
    assert_eq!(status, 200, "DeletePipeline got {status} {body}");
    let after = listed(&router, WORKSPACE, "maxResults=250").await;
    let reported: BTreeSet<String> = names(&after).into_iter().collect();
    let expected: BTreeSet<String> = cases()
        .into_iter()
        .map(|case| case.name.to_string())
        .filter(|name| name != deleted_name)
        .collect();
    assert_eq!(
        reported, expected,
        "case {deleted_name}: a deleted pipeline is listed no more, got {after}"
    );
}

#[tokio::test]
async fn pagination_walks_one_stable_enumeration_whatever_the_page_size() {
    let router = router();
    let written = write_fixture(&router, WORKSPACE).await;
    let total = written.len();

    // The whole listing in one page is the order every paged walk has to reproduce.
    let baseline = names(&listed(&router, WORKSPACE, "maxResults=250").await);
    assert_eq!(
        baseline.len(),
        total,
        "the unpaged listing holds every created pipeline"
    );

    // Rows: a page smaller than the listing, sizes that divide it and sizes that do not,
    // exactly the listing, more than it, the modelled maximum, and no `maxResults` at
    // all.
    let cases: [Option<i64>; 8] = [
        Some(1),
        Some(2),
        Some(3),
        Some(4),
        Some(total as i64),
        Some(total as i64 + 1),
        Some(250),
        None,
    ];
    for page_size in cases {
        let case = format!("maxResults={page_size:?}");
        let (walked, pages) = walk(&router, WORKSPACE, page_size, total).await;
        assert_eq!(
            walked, baseline,
            "case {case:?}: the pages joined must be the whole listing once, in one order, \
             got {walked:?}"
        );
        let expected_pages = match page_size {
            Some(size) => total.div_ceil(size as usize),
            // An absent `maxResults` arrives as the modelled default of 50, which is
            // more than this fixture holds, so one page carries it all.
            None => 1,
        };
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, walked {pages}"
        );
    }

    // The token a page carries names the pipeline the NEXT page starts at, in the form
    // this listing issues: the hex of that pipeline's name, built by an oracle that does
    // not share the handler's encoder.
    let first = listed(&router, WORKSPACE, "maxResults=1").await;
    assert_eq!(
        first["nextToken"].as_str(),
        Some(hex_token(&baseline[1]).as_str()),
        "the first page's token resumes at the second pipeline, got {first}"
    );

    // A token this listing never issued is refused rather than answered with an empty
    // page: unhexable text, hex of a name no pipeline of this workspace carries, and hex
    // of the name of a pipeline of ANOTHER workspace.
    let (status, elsewhere) = create(
        &router,
        OTHER_WORKSPACE,
        json!({
            "pipelineName": "only-over-there",
            "computations": [node("only", "clean", &[])],
        }),
    )
    .await;
    assert_eq!(status, 200, "a pipeline in another workspace: {elsewhere}");
    for (case, token) in [
        ("not a token this service issues", "not-hex".to_string()),
        ("hex of an unknown name", hex_token("never-created")),
        ("hex of another workspace's pipeline", hex_token("only-over-there")),
    ] {
        let (status, body) = list_raw(&router, WORKSPACE, &format!("nextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {token}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // `maxResults` carries @range 1..250 (`smithy_query(query="operation",
    // operation="ListPipelines")`): each published bound is served and one past it on
    // either side is refused.
    for (case, max_results, served) in [
        ("the range minimum", 1_i64, true),
        ("the range maximum", 250, true),
        ("one under the minimum", 0, false),
        ("one past the maximum", 251, false),
    ] {
        let (status, body) = list_raw(&router, WORKSPACE, &format!("maxResults={max_results}")).await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?} (maxResults={max_results}): an in-range page size must be served, \
                 got {status} {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?} (maxResults={max_results}): an out-of-range page size must be \
                 refused, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case {case:?} (maxResults={max_results}): unexpected error code in {body}"
            );
        }
    }
}
