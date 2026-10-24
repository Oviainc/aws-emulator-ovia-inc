// SPDX-License-Identifier: CC0-1.0

//! `ListMetadataTransferJobs`: which jobs a page reports, how `filters` narrow
//! it, how a page is bounded and resumed, and that listing changes nothing a
//! later caller sees.
//!
//! The rules pinned here, each over a SET of inputs rather than one request:
//!
//! * a listing reports exactly the account's jobs whose source type and
//!   destination type are the ones the request names — asserted over the whole
//!   cross product of the modelled `SourceType` and `DestinationType` values;
//! * every entry of `filters` narrows the listing: a job is reported only when
//!   it satisfies all of them, by the workspace at either end of its transfer or
//!   by its state;
//! * a page carries at most the page size asked for, in `metadataTransferJobId`
//!   order, and walking its `nextToken`s yields every matching job exactly once,
//!   whatever the page size;
//! * a value outside a modelled enumeration, a `maxResults` below one and a
//!   `nextToken` this listing did not mint are each refused with HTTP 400
//!   `ValidationException`;
//! * and a listing — served or refused — mints no job and removes none.
//!
//! Every case drives the real request path (`MultiServiceRouter::dispatch`), so
//! the route, the status code and the serialized body are all exercised.

use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::ops::types::{
    DESTINATION_TYPE_VALUES, METADATA_TRANSFER_JOB_STATE_VALUES, SOURCE_TYPE_VALUES,
};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    METADATA_TRANSFER_JOB_SETTLE_SPAN, METADATA_TRANSFER_JOB_STATE_ERROR,
    METADATA_TRANSFER_JOB_STATE_VALIDATING,
};

use crate::harness::{metadata_transfer_job_arn, workspace_arn, Emulator};

/// Wait until the outcome every accepted job is owed has fallen due.
///
/// The emulator stores the instant and never sleeps for it, so a case that wants
/// to observe the settled job is the one that has to wait; the margin absorbs
/// scheduling jitter.
fn wait_until_settled() {
    sleep(METADATA_TRANSFER_JOB_SETTLE_SPAN + Duration::from_millis(100));
}

/// The workspace every IoT TwinMaker end of a transfer created here names,
/// unless a case names the second one.
const WORKSPACE: &str = "listWorkspace";
/// A second workspace, so a `workspaceId` filter has something to EXCLUDE.
const OTHER_WORKSPACE: &str = "otherListWorkspace";

/// A source configuration of `source_type` naming `workspace` at an IoT
/// TwinMaker end.
///
/// One row per value of the modelled `SourceType`
/// (`smithy_query(query="shape",shape="SourceType")`), each carrying the
/// configuration member that type requires:
/// `s3` an `s3Configuration`, `iottwinmaker` an `iotTwinMakerConfiguration`, and
/// `iotsitewise` none (every member of its configuration is optional).
fn source(source_type: &str, workspace: &str) -> Value {
    match source_type {
        "s3" => json!({
            "type": "s3",
            "s3Configuration": {"location": "arn:aws:s3:::metadata-bucket/entities.json"},
        }),
        "iotsitewise" => json!({"type": "iotsitewise"}),
        "iottwinmaker" => json!({
            "type": "iottwinmaker",
            "iotTwinMakerConfiguration": {"workspace": workspace_arn(workspace)},
        }),
        other => panic!("no source configuration for the source type {other:?}"),
    }
}

/// A destination configuration of `destination_type`, the mirror of [`source`]
/// over the modelled `DestinationType` values.
fn destination(destination_type: &str, workspace: &str) -> Value {
    match destination_type {
        "s3" => json!({
            "type": "s3",
            "s3Configuration": {"location": "arn:aws:s3:::metadata-bucket/export"},
        }),
        "iotsitewise" => json!({"type": "iotsitewise"}),
        "iottwinmaker" => json!({
            "type": "iottwinmaker",
            "iotTwinMakerConfiguration": {"workspace": workspace_arn(workspace)},
        }),
        other => panic!("no destination configuration for the destination type {other:?}"),
    }
}

/// A create request for a job with these ends, under `id`, with every IoT
/// TwinMaker end naming `workspace`.
fn create_request(id: &str, source_type: &str, destination_type: &str, workspace: &str) -> Value {
    json!({
        "metadataTransferJobId": id,
        "sources": [source(source_type, workspace)],
        "destination": destination(destination_type, workspace),
    })
}

/// A listing request over these ends, with `extra`'s members merged in.
fn list_request(source_type: &str, destination_type: &str, extra: Value) -> Value {
    let mut request = json!({"sourceType": source_type, "destinationType": destination_type});
    if let Some(members) = extra.as_object() {
        for (name, value) in members {
            request[name] = value.clone();
        }
    }
    request
}

/// The `metadataTransferJobId`s a listing response reports, in the order it
/// reported them.
fn reported_ids(response: &crate::harness::Resp) -> Vec<String> {
    response
        .member("metadataTransferJobSummaries")
        .as_array()
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| summary["metadataTransferJobId"].as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Every job the listing over these ends reports, walked page by page at
/// `page_size`, with the ids of each page concatenated in the order served.
async fn walk_pages(
    emulator: &Emulator,
    source_type: &str,
    destination_type: &str,
    filters: Value,
    page_size: usize,
) -> Vec<String> {
    let mut ids = Vec::new();
    let mut next_token: Option<String> = None;
    // Bounded so a listing that never stops handing out tokens fails the test
    // instead of looping: no case here stores more than a handful of jobs.
    for _ in 0..64 {
        let mut extra = json!({"maxResults": page_size});
        if !filters.is_null() {
            extra["filters"] = filters.clone();
        }
        if let Some(token) = &next_token {
            extra["nextToken"] = json!(token);
        }
        let page = emulator
            .list_metadata_transfer_jobs(list_request(source_type, destination_type, extra))
            .await;
        assert_eq!(
            page.status, 200,
            "page size {page_size}: expected 200 from ListMetadataTransferJobs, got {page:?}"
        );
        let served = reported_ids(&page);
        assert!(
            served.len() <= page_size,
            "page size {page_size}: a page may not exceed the size asked for, got {page:?}"
        );
        ids.extend(served);
        next_token = page.member("nextToken").as_str().map(str::to_string);
        if next_token.is_none() {
            return ids;
        }
    }
    panic!("page size {page_size}: the listing kept handing out nextTokens");
}

/// RULE: a listing reports exactly the account's metadata transfer jobs whose
/// source type and destination type are the ones the request names, and reports
/// each one's stored identity, instants and state.
///
/// The rows are the whole cross product of the modelled `SourceType` and
/// `DestinationType` values, so each pair is asserted both as the pair that
/// selects its own job and as one of the pairs that must exclude it.
#[tokio::test]
async fn a_listing_reports_the_jobs_with_the_requested_ends() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace(WORKSPACE).await.status,
        200,
        "the workspace every IoT TwinMaker end names must exist"
    );

    // One job per pair of ends, named after the pair it transfers between.
    let mut expected: Vec<(String, String, String)> = Vec::new();
    for source_type in SOURCE_TYPE_VALUES {
        for destination_type in DESTINATION_TYPE_VALUES {
            let id = format!("from-{source_type}-to-{destination_type}");
            let created = emulator
                .create_metadata_transfer_job(create_request(
                    &id,
                    source_type,
                    destination_type,
                    WORKSPACE,
                ))
                .await;
            assert_eq!(
                created.status, 200,
                "case {id:?}: expected 200 from CreateMetadataTransferJob, got {created:?}"
            );
            expected.push((source_type.to_string(), destination_type.to_string(), id));
        }
    }

    for (source_type, destination_type, id) in &expected {
        let case = format!("{source_type} -> {destination_type}");
        let listed = emulator
            .list_metadata_transfer_jobs(list_request(source_type, destination_type, json!({})))
            .await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected 200 from ListMetadataTransferJobs, got {listed:?}"
        );
        assert_eq!(
            reported_ids(&listed),
            vec![id.clone()],
            "case {case:?}: a listing reports exactly the jobs with those ends, got {listed:?}"
        );

        let summary = &listed.member("metadataTransferJobSummaries")[0];
        assert_eq!(
            summary["arn"].as_str(),
            Some(metadata_transfer_job_arn(id).as_str()),
            "case {case:?}: expected the stored ARN, got {summary:?}"
        );
        // The job was just accepted, so it is in the non-terminal state its
        // create answered with or the terminal state the clock owes it.
        let state = summary["status"]["state"].as_str();
        assert!(
            state == Some(METADATA_TRANSFER_JOB_STATE_VALIDATING)
                || state == Some(METADATA_TRANSFER_JOB_STATE_ERROR),
            "case {case:?}: expected the accepted or the settled state, got {summary:?}"
        );
        assert!(
            summary["creationDateTime"]
                .as_f64()
                .is_some_and(|seconds| seconds > 0.0)
                && summary["updateDateTime"]
                    .as_f64()
                    .is_some_and(|seconds| seconds > 0.0),
            "case {case:?}: expected epoch-seconds instants, got {summary:?}"
        );
        // Nothing transferred, so there is no count to report.
        assert!(
            summary["progress"].is_null(),
            "case {case:?}: a job that transferred nothing reports no progress, got {summary:?}"
        );
    }

    // And the member is served as an EMPTY list, not omitted, for a pair of ends
    // whose jobs this account holds none of: every pair above holds exactly one,
    // so a pair taken after deleting nothing but filtering on an unreached state
    // is the empty case (see the filter suite below for the state rule).
    let empty = emulator
        .list_metadata_transfer_jobs(list_request(
            "s3",
            "iottwinmaker",
            json!({"filters": [{"state": "COMPLETED"}]}),
        ))
        .await;
    assert_eq!(
        (empty.status, empty.member("metadataTransferJobSummaries").as_array().map(Vec::len)),
        (200, Some(0)),
        "a listing selecting no job serves an empty list, got {empty:?}"
    );
}

/// RULE: every entry of `filters` narrows the listing — a job is reported only
/// when it satisfies all of them: a `workspaceId` filter selects the jobs naming
/// that workspace at EITHER end of the transfer, and a `state` filter the jobs in
/// that state.
#[tokio::test]
async fn filters_narrow_the_listing_conjunctively() {
    let emulator = Emulator::new();
    for workspace in [WORKSPACE, OTHER_WORKSPACE] {
        assert_eq!(
            emulator.create_workspace(workspace).await.status,
            200,
            "the workspace {workspace:?} the jobs transfer through must exist"
        );
    }

    // The rows differ in WHICH end names a workspace and in which workspace it
    // is: the destination names one, the source names one, and neither end names
    // any (an S3 document copied to an IoT SiteWise asset model).
    let rows: Vec<(&str, &str, &str, &str)> = vec![
        ("into-the-workspace", "s3", "iottwinmaker", WORKSPACE),
        ("out-of-the-workspace", "iottwinmaker", "s3", WORKSPACE),
        ("into-the-other-workspace", "s3", "iottwinmaker", OTHER_WORKSPACE),
        ("out-of-the-other-workspace", "iottwinmaker", "s3", OTHER_WORKSPACE),
    ];
    for (id, source_type, destination_type, workspace) in &rows {
        let created = emulator
            .create_metadata_transfer_job(create_request(
                id,
                source_type,
                destination_type,
                workspace,
            ))
            .await;
        assert_eq!(
            created.status, 200,
            "case {id:?}: expected 200 from CreateMetadataTransferJob, got {created:?}"
        );
    }

    // A `workspaceId` filter, taken over each pair of ends and each workspace in
    // turn: it selects the jobs of that workspace whichever END names it — the
    // destination for the rows whose ends are `s3 -> iottwinmaker`, the source
    // for those whose ends are `iottwinmaker -> s3`.
    let workspace_cases: Vec<(&str, &str, &str, &str, Vec<&str>)> = vec![
        (
            "the workspace, named by the destination",
            "s3",
            "iottwinmaker",
            WORKSPACE,
            vec!["into-the-workspace"],
        ),
        (
            "the other workspace, named by the destination",
            "s3",
            "iottwinmaker",
            OTHER_WORKSPACE,
            vec!["into-the-other-workspace"],
        ),
        (
            "the workspace, named by the source",
            "iottwinmaker",
            "s3",
            WORKSPACE,
            vec!["out-of-the-workspace"],
        ),
        (
            "the other workspace, named by the source",
            "iottwinmaker",
            "s3",
            OTHER_WORKSPACE,
            vec!["out-of-the-other-workspace"],
        ),
    ];
    for (case, source_type, destination_type, workspace, selected) in workspace_cases {
        // Unfiltered, BOTH workspaces' jobs with these ends are reported, so the
        // filtered listing below is narrowing a set that really held more.
        let unfiltered = emulator
            .list_metadata_transfer_jobs(list_request(source_type, destination_type, json!({})))
            .await;
        assert_eq!(
            (unfiltered.status, reported_ids(&unfiltered).len()),
            (200, 2),
            "case {case:?}: both workspaces' jobs with these ends are reported unfiltered, \
             got {unfiltered:?}"
        );

        let filtered = emulator
            .list_metadata_transfer_jobs(list_request(
                source_type,
                destination_type,
                json!({"filters": [{"workspaceId": workspace}]}),
            ))
            .await;
        assert_eq!(
            (filtered.status, reported_ids(&filtered)),
            (
                200,
                selected.iter().map(|id| id.to_string()).collect::<Vec<_>>()
            ),
            "case {case:?}: a workspaceId filter selects the jobs naming that workspace at \
             either end, got {filtered:?}"
        );
    }

    // A workspace that no job names selects none, and the filter carries an id:
    // a filter naming the whole workspace ARN names no workspace id, so it
    // selects nothing either.
    let nothing_selected: Vec<(&str, Value)> = vec![
        (
            "a workspace no job names",
            json!([{"workspaceId": "neverNamedWorkspace"}]),
        ),
        (
            "the workspace's ARN where its id belongs",
            json!([{"workspaceId": workspace_arn(WORKSPACE)}]),
        ),
    ];
    for (case, filters) in nothing_selected {
        let none = emulator
            .list_metadata_transfer_jobs(list_request(
                "s3",
                "iottwinmaker",
                json!({"filters": filters}),
            ))
            .await;
        assert_eq!(
            (none.status, reported_ids(&none)),
            (200, Vec::<String>::new()),
            "case {case:?}: expected a page selecting no job, got {none:?}"
        );
    }

    // A `state` filter, taken over EVERY value of the modelled
    // `MetadataTransferJobState`. The clock is what decides a job's state, so
    // the cases wait until the outcome every accepted job is owed has fallen
    // due: then exactly one modelled state selects the jobs with these ends, and
    // every other selects none — including the state their create answered with.
    wait_until_settled();
    let in_flight = ["into-the-other-workspace", "into-the-workspace"];
    for state in METADATA_TRANSFER_JOB_STATE_VALUES {
        let filtered = emulator
            .list_metadata_transfer_jobs(list_request(
                "s3",
                "iottwinmaker",
                json!({"filters": [{"state": state}]}),
            ))
            .await;
        assert_eq!(
            filtered.status, 200,
            "case {state:?}: expected 200 from a state filter, got {filtered:?}"
        );
        let expected: Vec<String> = if *state == METADATA_TRANSFER_JOB_STATE_ERROR {
            in_flight.iter().map(|id| id.to_string()).collect()
        } else {
            Vec::new()
        };
        assert_eq!(
            reported_ids(&filtered),
            expected,
            "case {state:?}: a state filter selects exactly the jobs in that state, \
             got {filtered:?}"
        );
    }

    // Both filters together narrow CONJUNCTIVELY: a job is reported only when it
    // names the workspace AND is in the state. The rows differ in whether each
    // half holds — both, only the workspace, only the state — so the empty
    // answers are not an accident of one filter being ignored.
    let conjunctions: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "both halves hold",
            json!([{"workspaceId": WORKSPACE}, {"state": METADATA_TRANSFER_JOB_STATE_ERROR}]),
            vec!["into-the-workspace"],
        ),
        (
            "the workspace holds, the state does not",
            json!([{"workspaceId": WORKSPACE}, {"state": METADATA_TRANSFER_JOB_STATE_VALIDATING}]),
            vec![],
        ),
        (
            "the state holds, the workspace does not",
            json!([
                {"workspaceId": "neverNamedWorkspace"},
                {"state": METADATA_TRANSFER_JOB_STATE_ERROR},
            ]),
            vec![],
        ),
    ];
    for (case, filters, selected) in conjunctions {
        let conjunction = emulator
            .list_metadata_transfer_jobs(list_request(
                "s3",
                "iottwinmaker",
                json!({"filters": filters}),
            ))
            .await;
        assert_eq!(
            (conjunction.status, reported_ids(&conjunction)),
            (
                200,
                selected.iter().map(|id| id.to_string()).collect::<Vec<_>>()
            ),
            "case {case:?}: every filter must hold for a job to be reported, got {conjunction:?}"
        );
    }
}

/// RULE: a page carries at most the number of jobs asked for, in
/// `metadataTransferJobId` order, and walking the `nextToken`s of a listing
/// yields every matching job exactly once whatever the page size.
#[tokio::test]
async fn pages_are_bounded_and_resume_where_they_left_off() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace(WORKSPACE).await.status,
        200,
        "the workspace the jobs transfer into must exist"
    );

    // Created in an order that is NOT id order, so an ordered listing is a real
    // assertion rather than an accident of insertion.
    let created = [
        "job-g", "job-a", "job-d", "job-c", "job-f", "job-b", "job-e",
    ];
    for id in created {
        assert_eq!(
            emulator
                .create_metadata_transfer_job(create_request(id, "s3", "iottwinmaker", WORKSPACE))
                .await
                .status,
            200,
            "the job {id:?} the pages report must be created"
        );
    }
    let mut all_ids: Vec<String> = created.iter().map(|id| id.to_string()).collect();
    all_ids.sort();

    // The rows span the page sizes the service distinguishes: one row per page,
    // a size that divides the set, one that does not, exactly the set, more than
    // the set, and the published maximum of the modelled `maxResults` range
    // (0..200, from `smithy_query(query="shape",shape="MaxResults")`).
    for page_size in [1usize, 2, 3, 7, 8, 200] {
        let walked = walk_pages(&emulator, "s3", "iottwinmaker", Value::Null, page_size).await;
        assert_eq!(
            walked, all_ids,
            "page size {page_size}: walking the pages must yield every job once, in id order"
        );
    }

    // A listing that names no `maxResults` serves the whole set here (it is far
    // under this service's default page size) and hands out no token.
    let defaulted = emulator
        .list_metadata_transfer_jobs(list_request("s3", "iottwinmaker", json!({})))
        .await;
    assert_eq!(
        (defaulted.status, reported_ids(&defaulted)),
        (200, all_ids.clone()),
        "a listing with no maxResults serves the default page, got {defaulted:?}"
    );
    assert!(
        defaulted.member("nextToken").is_null(),
        "the last page carries no nextToken, got {defaulted:?}"
    );

    // A filtered listing paginates over the FILTERED set: the token resumes the
    // same narrowed listing rather than the whole store.
    let filtered = walk_pages(
        &emulator,
        "s3",
        "iottwinmaker",
        json!([{"workspaceId": WORKSPACE}]),
        2,
    )
    .await;
    assert_eq!(
        filtered, all_ids,
        "a filtered listing paginates over the jobs the filter selects"
    );
}

/// RULE: a request outside the model's own values — an end type outside the
/// modelled `SourceType`/`DestinationType`, a filter naming a state outside the
/// modelled `MetadataTransferJobState`, a `maxResults` below one, or a
/// `nextToken` this listing did not mint — is refused with HTTP 400
/// `ValidationException` and reports nothing.
#[tokio::test]
async fn a_listing_refuses_what_the_model_does_not_admit() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace(WORKSPACE).await.status,
        200,
        "the workspace the held job transfers into must exist"
    );
    for id in ["held-one", "held-two"] {
        assert_eq!(
            emulator
                .create_metadata_transfer_job(create_request(id, "s3", "iottwinmaker", WORKSPACE))
                .await
                .status,
            200,
            "the job {id:?} the refusals must leave alone"
        );
    }

    // A token this listing DID mint, kept so the rows can take it over other
    // ends — where the page it names would mean something else.
    let page = emulator
        .list_metadata_transfer_jobs(list_request(
            "s3",
            "iottwinmaker",
            json!({"maxResults": 1}),
        ))
        .await;
    let minted = page
        .member("nextToken")
        .as_str()
        .expect("a page of two jobs served one at a time carries a nextToken")
        .to_string();

    // Each row takes one member one step outside what the model or this listing
    // admits; every one is the same refusal.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a source type outside the modelled SourceType",
            list_request("sitewise", "iottwinmaker", json!({})),
        ),
        (
            "a destination type outside the modelled DestinationType",
            list_request("s3", "twinmaker", json!({})),
        ),
        (
            "an empty source type",
            list_request("", "iottwinmaker", json!({})),
        ),
        (
            "a source type differing only in case",
            list_request("S3", "iottwinmaker", json!({})),
        ),
        (
            "a filter state outside the modelled MetadataTransferJobState",
            list_request("s3", "iottwinmaker", json!({"filters": [{"state": "DONE"}]})),
        ),
        (
            "a filter state differing only in case",
            list_request(
                "s3",
                "iottwinmaker",
                json!({"filters": [{"state": "validating"}]}),
            ),
        ),
        (
            "more filters than the listing accepts",
            list_request(
                "s3",
                "iottwinmaker",
                json!({"filters": [
                    {"workspaceId": WORKSPACE},
                    {"state": METADATA_TRANSFER_JOB_STATE_ERROR},
                    {"state": METADATA_TRANSFER_JOB_STATE_VALIDATING},
                ]}),
            ),
        ),
        (
            "maxResults of zero",
            list_request("s3", "iottwinmaker", json!({"maxResults": 0})),
        ),
        (
            "maxResults past the modelled range",
            list_request("s3", "iottwinmaker", json!({"maxResults": 201})),
        ),
        (
            "a nextToken nothing minted",
            list_request("s3", "iottwinmaker", json!({"nextToken": "not-a-token"})),
        ),
        (
            "a nextToken minted over other ends",
            list_request("s3", "s3", json!({"nextToken": minted.clone()})),
        ),
    ];
    for (case, request) in rows {
        let refused = emulator.list_metadata_transfer_jobs(request).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected 400 ValidationException, got {refused:?}"
        );
        assert!(
            refused.member("metadataTransferJobSummaries").is_null(),
            "case {case:?}: a refusal reports no job, got {refused:?}"
        );
    }

    // The token this listing minted still resumes the listing it was minted for.
    let resumed = emulator
        .list_metadata_transfer_jobs(list_request(
            "s3",
            "iottwinmaker",
            json!({"nextToken": minted}),
        ))
        .await;
    assert_eq!(
        (resumed.status, reported_ids(&resumed)),
        (200, vec!["held-two".to_string()]),
        "a token keeps resuming the listing it was minted for, got {resumed:?}"
    );
}

/// RULE: a listing is a report — whether it is served or refused, it mints no
/// metadata transfer job, removes none, and leaves the set of jobs this account
/// holds exactly as it was.
#[tokio::test]
async fn a_listing_mints_no_job_and_removes_none() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace(WORKSPACE).await.status,
        200,
        "the workspace the jobs transfer into must exist"
    );
    let ids = ["first-job", "second-job"];
    for id in ids {
        assert_eq!(
            emulator
                .create_metadata_transfer_job(create_request(id, "s3", "iottwinmaker", WORKSPACE))
                .await
                .status,
            200,
            "the job {id:?} the cases list must be created"
        );
    }
    let before: Vec<String> = emulator
        .state()
        .lock()
        .metadata_transfer_jobs
        .keys()
        .cloned()
        .collect();

    // The rows differ in whether the listing is served, selects nothing, or is
    // refused outright: none of the three may change the stored set.
    let rows: Vec<(&str, Value)> = vec![
        ("a served listing", list_request("s3", "iottwinmaker", json!({}))),
        (
            "a listing selecting nothing",
            list_request("iotsitewise", "s3", json!({})),
        ),
        (
            "a filtered listing",
            list_request(
                "s3",
                "iottwinmaker",
                json!({"filters": [{"workspaceId": WORKSPACE}]}),
            ),
        ),
        (
            "a refused listing",
            list_request("nowhere", "iottwinmaker", json!({})),
        ),
        ("the same listing again", list_request("s3", "iottwinmaker", json!({}))),
    ];
    for (case, request) in rows {
        let listed = emulator.list_metadata_transfer_jobs(request).await;
        assert!(
            listed.status == 200 || listed.status == 400,
            "case {case:?}: a listing answers either a page or a refusal, got {listed:?}"
        );
        let after: Vec<String> = emulator
            .state()
            .lock()
            .metadata_transfer_jobs
            .keys()
            .cloned()
            .collect();
        assert_eq!(
            after, before,
            "case {case:?}: a listing must leave the stored jobs as they were, got {after:?}"
        );
    }

    // And every job is still readable under its own id, with the identity the
    // listing reported for it.
    for id in ids {
        let read = emulator.get_metadata_transfer_job(id).await;
        assert_eq!(
            (read.status, read.member("arn").as_str()),
            (200, Some(metadata_transfer_job_arn(id).as_str())),
            "case {id:?}: every listed job stays readable, got {read:?}"
        );
    }
}
