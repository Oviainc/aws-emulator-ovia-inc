// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListDatasets`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with, so every assertion is about the
//! wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * a listing enumerates exactly the datasets the creates persisted that the
//!   request's filters select — `sourceType` (required), and `datasetType` and
//!   `workspaceName` when present — and no other dataset;
//! * every summary is read off the stored dataset, so a listed dataset and the same
//!   dataset read through `DescribeDataset` never disagree, and a listing REPORTS:
//!   it moves no dataset on;
//! * an accepted write is visible to the next listing, including a write that moves
//!   a dataset out of the selection it used to fall in;
//! * `maxResults`/`nextToken` page one stable enumeration: the pages joined are the
//!   whole listing once, in one order, whatever page size the caller asks for, and a
//!   token this listing never issued is refused;
//! * a `workspaceName` this service holds no record of is the modelled not-found
//!   refusal, not an empty page, while a held workspace with no dataset is an empty
//!   page that still reports the workspace it was scoped to.
//!
//! The HTTP harness (`call`, `router`, `error_code`, `hex_token`) is the one the
//! asset suite already has; this suite reuses it rather than growing a second copy.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::ACTIVATION_DELAY;

use crate::application_lifecycle::create_workspace;
use crate::asset_lifecycle::{call, error_code, hex_token, router, REGION};

/// Where every dataset request goes, as `OPERATIONS.md` reports the routes:
/// `POST /datasets`, `GET /datasets`, `GET|PUT /datasets/{datasetId}`.
const DATASETS: &str = "/datasets";

/// The workspace most rows of the fixture are created in.
const WORKSPACE: &str = "plant-floor";

/// A second workspace, so a listing scoped to one can be shown to report none of
/// the other's datasets.
const OTHER_WORKSPACE: &str = "plant-roof";

/// A workspace this service holds a record of that no create ever wrote a dataset
/// into.
const EMPTY_WORKSPACE: &str = "plant-empty";

/// The whole `DatasetSourceType` value list, from the enum itself:
/// `smithy_query(query="shape", shape="DatasetSourceType")` — KENDRA | SITEWISE.
const SOURCE_TYPES: [&str; 2] = ["KENDRA", "SITEWISE"];

/// The whole `DatasetTypeEnum` value list, from the enum itself:
/// `smithy_query(query="shape", shape="DatasetTypeEnum")` — SESSION | CURATED |
/// EXTERNAL.
const DATASET_TYPES: [&str; 3] = ["SESSION", "CURATED", "EXTERNAL"];

/// Long enough that the transition a create or an update owes has fallen due, so
/// every dataset the suite wrote reports its settled state.
fn settled() -> Duration {
    ACTIVATION_DELAY + Duration::from_millis(200)
}

/// A `datasetSource` of `source_type`, with the `sourceFormat` that type carries
/// and, for a Kendra source, the detail `KendraSourceDetail` requires.
///
/// `sourceType` / `sourceFormat` values come from the enums themselves
/// (`smithy_query(query="shape", shape="DatasetSourceType")` — KENDRA | SITEWISE;
/// `shape="DatasetSourceFormat"` — KNOWLEDGE_BASE | TIMESERIES).
fn source_of(source_type: &str, label: &str) -> Value {
    match source_type {
        "KENDRA" => json!({
            "sourceType": "KENDRA",
            "sourceFormat": "KNOWLEDGE_BASE",
            "sourceDetail": {
                "kendra": {
                    "knowledgeBaseArn":
                        format!("arn:aws:kendra:{REGION}:123456789012:index/{label}"),
                    "roleArn": format!("arn:aws:iam::123456789012:role/{label}"),
                }
            },
        }),
        _ => json!({ "sourceType": "SITEWISE", "sourceFormat": "TIMESERIES" }),
    }
}

/// One row of the fixture: the dataset a create writes.
///
/// The rows differ along every dimension the filters select on — the data-source
/// type, the dataset type (including a dataset created without one), and the
/// workspace (including a dataset created outside any) — and along what a summary
/// must report whatever the filters were: a present or absent description.
#[derive(Debug, Clone)]
struct DatasetCase {
    name: &'static str,
    description: Option<&'static str>,
    dataset_type: Option<&'static str>,
    workspace: Option<&'static str>,
    source_type: &'static str,
}

impl DatasetCase {
    /// `true` when a listing filtered by `source_type`, `dataset_type` and
    /// `workspace` has to report this row. The oracle the suite checks the
    /// emulator against: an independent statement of the rule, not a call into it.
    fn selected_by(
        &self,
        source_type: &str,
        dataset_type: Option<&str>,
        workspace: Option<&str>,
    ) -> bool {
        self.source_type == source_type
            && dataset_type.is_none_or(|wanted| self.dataset_type == Some(wanted))
            && workspace.is_none_or(|wanted| self.workspace == Some(wanted))
    }
}

/// The rows every suite writes, in the order they are created.
///
/// `workspaceName` follows the rule `CreateDataset` states on it — required for a
/// session or curated dataset, omitted for an external one — so a row's workspace
/// is not free of its type; a row with no `datasetType` is free either way, and
/// both are present.
fn cases() -> Vec<DatasetCase> {
    vec![
        DatasetCase {
            name: "session-kendra",
            description: Some("a session dataset over a knowledge base"),
            dataset_type: Some("SESSION"),
            workspace: Some(WORKSPACE),
            source_type: "KENDRA",
        },
        DatasetCase {
            name: "session-sitewise",
            description: None,
            dataset_type: Some("SESSION"),
            workspace: Some(WORKSPACE),
            source_type: "SITEWISE",
        },
        DatasetCase {
            name: "curated-sitewise",
            description: Some("a curated dataset over time series"),
            dataset_type: Some("CURATED"),
            workspace: Some(WORKSPACE),
            source_type: "SITEWISE",
        },
        DatasetCase {
            name: "curated-kendra elsewhere",
            description: Some("a curated dataset in the other workspace"),
            dataset_type: Some("CURATED"),
            workspace: Some(OTHER_WORKSPACE),
            source_type: "KENDRA",
        },
        DatasetCase {
            name: "session-sitewise elsewhere",
            description: None,
            dataset_type: Some("SESSION"),
            workspace: Some(OTHER_WORKSPACE),
            source_type: "SITEWISE",
        },
        DatasetCase {
            name: "external-kendra",
            description: None,
            dataset_type: Some("EXTERNAL"),
            workspace: None,
            source_type: "KENDRA",
        },
        DatasetCase {
            name: "external-sitewise",
            description: Some("an external datasource connection"),
            dataset_type: Some("EXTERNAL"),
            workspace: None,
            source_type: "SITEWISE",
        },
        DatasetCase {
            name: "untyped-sitewise",
            description: None,
            dataset_type: None,
            workspace: Some(WORKSPACE),
            source_type: "SITEWISE",
        },
        DatasetCase {
            name: "untyped-kendra",
            description: Some("no datasetType and no workspace"),
            dataset_type: None,
            workspace: None,
            source_type: "KENDRA",
        },
    ]
}

/// The JSON body a row's create carries, with every absent member OMITTED rather
/// than sent as null.
fn create_body(case: &DatasetCase) -> Value {
    let mut body = json!({
        "datasetName": case.name,
        "datasetSource": source_of(case.source_type, "kb"),
    });
    let object = body.as_object_mut().expect("an object body");
    if let Some(description) = case.description {
        object.insert("datasetDescription".to_string(), json!(description));
    }
    if let Some(dataset_type) = case.dataset_type {
        object.insert("datasetType".to_string(), json!(dataset_type));
    }
    if let Some(workspace) = case.workspace {
        object.insert("workspaceName".to_string(), json!(workspace));
    }
    body
}

/// Write every row of `cases()`, asserting each create was accepted, and return the
/// rows paired with the ids the creates minted, in creation order.
async fn write_fixture(router: &MultiServiceRouter) -> Vec<(DatasetCase, String)> {
    create_workspace(router, WORKSPACE).await;
    create_workspace(router, OTHER_WORKSPACE).await;
    create_workspace(router, EMPTY_WORKSPACE).await;

    let mut written = Vec::new();
    for case in cases() {
        let (status, created) = call(router, "POST", DATASETS, Some(create_body(&case))).await;
        assert_eq!(
            status, 202,
            "case {:?}: CreateDataset got {status} {created}",
            case.name
        );
        let id = created["datasetId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {:?}: a create answers a datasetId, got {created}", case.name))
            .to_string();
        written.push((case, id));
    }
    written
}

/// `ListDatasets` with the raw query string `query` (empty for none).
async fn list_raw(router: &MultiServiceRouter, query: &str) -> (u16, Value) {
    let uri = if query.is_empty() {
        DATASETS.to_string()
    } else {
        format!("{DATASETS}?{query}")
    };
    call(router, "GET", &uri, None).await
}

/// The query string a filtered listing sends, with `extra` parameters appended.
fn filter_query(
    source_type: &str,
    dataset_type: Option<&str>,
    workspace: Option<&str>,
    extra: &[String],
) -> String {
    let mut parts = vec![format!("sourceType={source_type}")];
    if let Some(dataset_type) = dataset_type {
        parts.push(format!("datasetType={dataset_type}"));
    }
    if let Some(workspace) = workspace {
        parts.push(format!("workspaceName={workspace}"));
    }
    parts.extend(extra.iter().cloned());
    parts.join("&")
}

/// `ListDatasets`, asserting it answered 200 and that the required
/// `datasetSummaries` member is a list, and the body it answered with.
async fn listed(router: &MultiServiceRouter, query: &str) -> Value {
    let (status, body) = list_raw(router, query).await;
    assert_eq!(
        status, 200,
        "list ?{query}: expected 200, got {status} {body}"
    );
    assert!(
        body["datasetSummaries"].is_array(),
        "list ?{query}: datasetSummaries is a required list, got {body}"
    );
    body
}

/// The summaries a listing body reports, in the order it reported them.
fn summaries(body: &Value) -> &Vec<Value> {
    body["datasetSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("a listing holds a datasetSummaries list, got {body}"))
}

/// The dataset ids a listing body reports, in the order it reported them.
fn ids(body: &Value) -> Vec<String> {
    summaries(body)
        .iter()
        .map(|summary| {
            summary["id"]
                .as_str()
                .unwrap_or_else(|| panic!("a summary carries an id, got {summary}"))
                .to_string()
        })
        .collect()
}

/// Every page of the listing `query` selects, walked with `page_size` when given
/// and with no `maxResults` otherwise: the ids in order, and how many pages it took.
async fn walk(
    router: &MultiServiceRouter,
    source_type: &str,
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
        let mut extra: Vec<String> = Vec::new();
        if let Some(size) = page_size {
            extra.push(format!("maxResults={size}"));
        }
        if let Some(token) = &token {
            extra.push(format!("nextToken={token}"));
        }
        let body = listed(router, &filter_query(source_type, None, None, &extra)).await;
        let page = ids(&body);
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
async fn a_listing_reports_exactly_the_datasets_its_filters_select() {
    let router = router();
    let written = write_fixture(&router).await;

    // The whole cross product of the filters: every `sourceType` the enum admits,
    // against no `datasetType` and each one the enum admits, against no workspace
    // and each workspace the fixture wrote into — plus a held workspace with no
    // dataset at all.
    let mut dataset_types: Vec<Option<&str>> = vec![None];
    dataset_types.extend(DATASET_TYPES.iter().map(|value| Some(*value)));
    let workspaces: Vec<Option<&str>> = vec![
        None,
        Some(WORKSPACE),
        Some(OTHER_WORKSPACE),
        Some(EMPTY_WORKSPACE),
    ];

    for source_type in SOURCE_TYPES {
        for dataset_type in &dataset_types {
            for workspace in &workspaces {
                let query = filter_query(
                    source_type,
                    *dataset_type,
                    *workspace,
                    &["maxResults=250".to_string()],
                );
                let body = listed(&router, &query).await;
                let reported: BTreeSet<String> = ids(&body).into_iter().collect();
                let wanted: BTreeSet<String> = written
                    .iter()
                    .filter(|(case, _)| case.selected_by(source_type, *dataset_type, *workspace))
                    .map(|(_, id)| id.clone())
                    .collect();
                assert_eq!(
                    reported, wanted,
                    "case {query:?}: a listing reports exactly the datasets its filters \
                     select, got {body}"
                );
                assert!(
                    body["nextToken"].is_null(),
                    "case {query:?}: a listing that reported everything carries no \
                     nextToken, got {body}"
                );
                // "The name of the workspace": the one the listing was scoped to,
                // and none when it was not scoped to one.
                assert_eq!(
                    body["workspaceName"].as_str(),
                    *workspace,
                    "case {query:?}: a listing reports the workspace it was scoped to, \
                     got {body}"
                );
                // Every summary reports its OWN dataset's source type, which under
                // this filter is the one asked for.
                for summary in summaries(&body) {
                    assert_eq!(
                        summary["sourceType"].as_str(),
                        Some(source_type),
                        "case {query:?}: a summary reports its own sourceType, got {summary}"
                    );
                }
            }
        }
    }

    // A held workspace nobody created a dataset in is an EMPTY page — absence of
    // datasets is not absence of the workspace.
    for source_type in SOURCE_TYPES {
        let query = filter_query(source_type, None, Some(EMPTY_WORKSPACE), &[]);
        let body = listed(&router, &query).await;
        assert!(
            summaries(&body).is_empty() && body["nextToken"].is_null(),
            "case {query:?}: a workspace with no dataset answers an empty page, got {body}"
        );
        assert_eq!(
            body["workspaceName"].as_str(),
            Some(EMPTY_WORKSPACE),
            "case {query:?}: an empty page still reports its workspace, got {body}"
        );
    }
}

#[tokio::test]
async fn a_summary_agrees_with_a_read_of_that_one_dataset() {
    let router = router();
    let written = write_fixture(&router).await;

    // A create owes a transition the clock makes due, so a dataset listed straight
    // after its create is still settling. Asserted on the LAST row written, so the
    // assertion does not depend on how long the whole fixture took to write.
    let (last_case, last_id) = written.last().expect("a written fixture").clone();
    let settling = listed(
        &router,
        &filter_query(last_case.source_type, None, None, &["maxResults=250".to_string()]),
    )
    .await;
    let settling_summary = summaries(&settling)
        .iter()
        .find(|summary| summary["id"].as_str() == Some(last_id.as_str()))
        .unwrap_or_else(|| panic!("the created dataset is listed, got {settling}"))
        .clone();
    assert_eq!(
        settling_summary["status"]["state"].as_str(),
        Some("CREATING"),
        "case {:?}: a dataset read straight after its create is still settling, got \
         {settling_summary}",
        last_case.name
    );

    tokio::time::sleep(settled()).await;

    // A listing REPORTS: two listings of one unchanged store agree, and neither
    // moves a dataset on.
    for source_type in SOURCE_TYPES {
        let query = filter_query(source_type, None, None, &["maxResults=250".to_string()]);
        let first = listed(&router, &query).await;
        let again = listed(&router, &query).await;
        assert_eq!(
            ids(&again),
            ids(&first),
            "case {query:?}: two listings of one store report the same datasets in the \
             same order"
        );

        for summary in summaries(&first) {
            let id = summary["id"].as_str().unwrap_or_default().to_string();
            assert_eq!(
                summary["status"]["state"].as_str(),
                Some("ACTIVE"),
                "case {id}: the transition the create owed has fallen due, got {summary}"
            );
            // The same dataset, read on its own: a listing and a read of one stored
            // dataset cannot disagree about any member they both report.
            let (status, read) =
                call(&router, "GET", &format!("{DATASETS}/{id}"), None).await;
            assert_eq!(status, 200, "case {id}: DescribeDataset got {status} {read}");
            for (listed_member, read_member) in [
                ("id", "datasetId"),
                ("arn", "datasetArn"),
                ("name", "datasetName"),
                ("description", "datasetDescription"),
                ("datasetType", "datasetType"),
                ("creationDate", "datasetCreationDate"),
                ("lastUpdateDate", "datasetLastUpdateDate"),
                ("status", "datasetStatus"),
            ] {
                assert_eq!(
                    summary.get(listed_member),
                    read.get(read_member),
                    "case {id}: the summary {summary} disagrees with the read {read} on \
                     {listed_member}/{read_member}"
                );
            }
            assert_eq!(
                summary["sourceType"].as_str(),
                read["datasetSource"]["sourceType"].as_str(),
                "case {id}: a summary's sourceType is its own source's, got {summary} \
                 against {read}"
            );
        }
    }
}

#[tokio::test]
async fn a_listing_follows_the_writes_of_the_datasets_it_reports() {
    let router = router();
    let written = write_fixture(&router).await;
    tokio::time::sleep(settled()).await;

    // The row the update rewrites: a SITEWISE dataset whose source the write moves
    // to KENDRA, so the listing follows the write along the very dimension it
    // filters on.
    let (moved_case, moved_id) = written
        .iter()
        .find(|(case, _)| case.name == "curated-sitewise")
        .expect("the fixture holds that row")
        .clone();
    let (status, updated) = call(
        &router,
        "PUT",
        &format!("{DATASETS}/{moved_id}"),
        Some(json!({
            "datasetName": "curated-sitewise rewritten",
            "datasetDescription": "the source this write replaced it with",
            "datasetSource": source_of("KENDRA", "rewritten"),
        })),
    )
    .await;
    // `UpdateDataset` answers 202 (`OPERATIONS.md`: `PUT /datasets/{datasetId} -> 202`).
    assert_eq!(status, 202, "UpdateDataset got {status} {updated}");

    // Gone from the listing it used to fall in...
    let sitewise = listed(
        &router,
        &filter_query("SITEWISE", None, None, &["maxResults=250".to_string()]),
    )
    .await;
    assert!(
        !ids(&sitewise).contains(&moved_id),
        "case {:?}: a dataset whose source the write replaced leaves the listing it \
         used to fall in, got {sitewise}",
        moved_case.name
    );

    // ...and present, with what the write wrote, in the one it now falls in.
    let kendra = listed(
        &router,
        &filter_query("KENDRA", None, None, &["maxResults=250".to_string()]),
    )
    .await;
    let summary = summaries(&kendra)
        .iter()
        .find(|summary| summary["id"].as_str() == Some(moved_id.as_str()))
        .unwrap_or_else(|| panic!("the rewritten dataset is listed, got {kendra}"))
        .clone();
    assert_eq!(
        summary["name"].as_str(),
        Some("curated-sitewise rewritten"),
        "case {:?}: a listing reports the name the update wrote, got {summary}",
        moved_case.name
    );
    assert_eq!(
        summary["description"].as_str(),
        Some("the source this write replaced it with"),
        "case {:?}: a listing reports the description the update wrote, got {summary}",
        moved_case.name
    );
    assert_eq!(
        summary["status"]["state"].as_str(),
        Some("UPDATING"),
        "case {:?}: a dataset listed straight after an update is still settling, got \
         {summary}",
        moved_case.name
    );
    assert!(
        summary["lastUpdateDate"].as_f64().unwrap_or(-1.0)
            > summary["creationDate"].as_f64().unwrap_or(0.0),
        "case {:?}: the update instant follows the create instant, got {summary}",
        moved_case.name
    );
    // The dataset's type and workspace are not the caller's to change on an update,
    // so the filters that selected it on those still do.
    assert_eq!(
        summary["datasetType"].as_str(),
        moved_case.dataset_type,
        "case {:?}: an update leaves the dataset's type alone, got {summary}",
        moved_case.name
    );
    let scoped = listed(
        &router,
        &filter_query("KENDRA", Some("CURATED"), Some(WORKSPACE), &[]),
    )
    .await;
    assert!(
        ids(&scoped).contains(&moved_id),
        "case {:?}: an update leaves the dataset's workspace alone, got {scoped}",
        moved_case.name
    );

    // Updating one dataset touches no other: every other row is still at the state
    // the clock settled it to.
    for source_type in SOURCE_TYPES {
        let body = listed(
            &router,
            &filter_query(source_type, None, None, &["maxResults=250".to_string()]),
        )
        .await;
        for other in summaries(&body)
            .iter()
            .filter(|summary| summary["id"].as_str() != Some(moved_id.as_str()))
        {
            let id = other["id"].as_str().unwrap_or_default();
            assert_eq!(
                other["status"]["state"].as_str(),
                Some("ACTIVE"),
                "case {id}: updating one dataset must not move another on, got {other}"
            );
        }
    }
}

#[tokio::test]
async fn pagination_walks_one_stable_enumeration_whatever_the_page_size() {
    let router = router();
    let written = write_fixture(&router).await;

    // The suite pages the SITEWISE listing, the larger of the two.
    let paged_source = "SITEWISE";
    let total = written
        .iter()
        .filter(|(case, _)| case.selected_by(paged_source, None, None))
        .count();
    assert!(total >= 4, "the fixture holds enough rows to page: {total}");

    // The whole listing in one page is the order every paged walk has to reproduce.
    let baseline = ids(&listed(
        &router,
        &filter_query(paged_source, None, None, &["maxResults=250".to_string()]),
    )
    .await);
    assert_eq!(
        baseline.len(),
        total,
        "the unpaged listing holds every selected dataset"
    );

    // Rows: a page smaller than the listing, sizes that divide it and sizes that do
    // not, exactly the listing, more than it, the modelled maximum, and no
    // `maxResults` at all.
    let page_sizes: [Option<i64>; 8] = [
        Some(1),
        Some(2),
        Some(3),
        Some(4),
        Some(total as i64),
        Some(total as i64 + 1),
        Some(250),
        None,
    ];
    for page_size in page_sizes {
        let case = format!("maxResults={page_size:?}");
        let (walked, pages) = walk(&router, paged_source, page_size, total).await;
        assert_eq!(
            walked, baseline,
            "case {case:?}: the pages joined must be the whole listing once, in one \
             order, got {walked:?}"
        );
        let expected_pages = match page_size {
            Some(size) => total.div_ceil(size as usize),
            // An absent `maxResults` is served at this crate's conventional page
            // size of 50, which is more than this fixture holds, so one page
            // carries it all.
            None => 1,
        };
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, walked {pages}"
        );
    }

    // The token a page carries names the dataset the NEXT page starts at, in the
    // form this listing issues: the hex of that dataset's id, built by an oracle
    // that does not share the handler's encoder.
    let first = listed(
        &router,
        &filter_query(paged_source, None, None, &["maxResults=1".to_string()]),
    )
    .await;
    assert_eq!(
        first["nextToken"].as_str(),
        Some(hex_token(&baseline[1]).as_str()),
        "the first page's token resumes at the second dataset, got {first}"
    );

    // A token this listing never issued is refused rather than answered with an
    // empty page: unhexable text, hex of an id no dataset carries, and hex of the id
    // of a dataset this filter does not select.
    let other_source_id = written
        .iter()
        .find(|(case, _)| case.source_type == "KENDRA")
        .map(|(_, id)| id.clone())
        .expect("the fixture holds a KENDRA dataset");
    for (case, token) in [
        ("not a token this service issues", "notatoken".to_string()),
        (
            "hex of an unknown id",
            hex_token("3d9f1c6e-0000-4000-8000-000000000001"),
        ),
        ("hex of a dataset this filter excludes", hex_token(&other_source_id)),
    ] {
        let query = filter_query(
            paged_source,
            None,
            None,
            &[format!("nextToken={token}")],
        );
        let (status, body) = list_raw(&router, &query).await;
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
    // operation="ListDatasets")`): each published bound is served and one past it on
    // either side is refused.
    for (case, max_results, served) in [
        ("the range minimum", 1_i64, true),
        ("the range maximum", 250, true),
        ("one under the minimum", 0, false),
        ("one past the maximum", 251, false),
    ] {
        let query = filter_query(
            paged_source,
            None,
            None,
            &[format!("maxResults={max_results}")],
        );
        let (status, body) = list_raw(&router, &query).await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?} (maxResults={max_results}): an in-range page size must be \
                 served, got {status} {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?} (maxResults={max_results}): an out-of-range page size must \
                 be refused, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case {case:?} (maxResults={max_results}): unexpected error code in {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_workspace_this_service_holds_no_record_of_is_refused() {
    let router = router();
    write_fixture(&router).await;

    // Every form of a name neither a workspace nor a dataset was ever written
    // under — including ones differing from a held name only in case, since a
    // workspace is named exactly. The workspace is resolved BEFORE the page is cut,
    // so a request that is wrong in both ways still reports the resource that is
    // absent.
    for workspace in ["plant-9", "PLANT-FLOOR", "Plant-Floor", "plant-floor-copy"] {
        for extra in [
            Vec::new(),
            vec!["maxResults=1".to_string()],
            vec!["nextToken=notatoken".to_string()],
        ] {
            for source_type in SOURCE_TYPES {
                let query = filter_query(source_type, None, Some(workspace), &extra);
                let (status, body) = list_raw(&router, &query).await;
                assert_eq!(
                    status, 404,
                    "case {query:?}: expected 404, got {status} {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "ResourceNotFoundException",
                    "case {query:?}: unexpected error code in {body}"
                );
            }
        }
    }

    // `sourceType` is required, so a listing that names none is not served at all,
    // rather than answered with every dataset: the request never reaches this
    // operation, so it is the framework's refusal rather than a modelled one.
    for query in ["", "maxResults=10", "datasetType=SESSION"] {
        let (status, body) = list_raw(&router, query).await;
        assert!(
            !(200..300).contains(&status),
            "case {query:?}: a listing naming no sourceType must not be served, got \
             {status} {body}"
        );
    }

    // A `sourceType` or `datasetType` outside its enum is refused too, rather than
    // selecting nothing.
    for (member, value) in [
        ("sourceType", "sitewise"),
        ("sourceType", "GLUE"),
        ("datasetType", "session"),
    ] {
        let query = if member == "sourceType" {
            format!("sourceType={value}")
        } else {
            format!("sourceType=SITEWISE&datasetType={value}")
        };
        let (status, body) = list_raw(&router, &query).await;
        assert_eq!(
            status, 400,
            "case {query:?}: a value outside the {member} enum is refused, got {status} {body}"
        );
    }
}
