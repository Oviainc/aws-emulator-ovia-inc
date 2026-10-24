// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's `ListModelCardExportJobs`.
//!
//! Every case goes over the wire (`POST /` with
//! `X-Amz-Target: SageMaker.ListModelCardExportJobs`, `awsJson1_1`) through the same
//! `MultiServiceRouter` the production router uses, via the `Emulator` harness
//! `inference_deployment_lifecycle` owns, so what these tests exercise is what a real SDK
//! client gets. The export-job helpers are reused from `model_card_export_jobs`.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: every
//! documented `SortBy`/`SortOrder` pair, every filter member, every member of the
//! modelled `ModelCardExportJobStatus` catalog, and every page size from one entry to
//! past the published `MaxResults` bound.

use std::time::Instant;

use serde_json::{json, Value};

use service_com_amazonaws_sagemaker_sagemaker::ops::types::MODEL_CARD_EXPORT_JOB_STATUS_VALUES;
use service_com_amazonaws_sagemaker_sagemaker::state::{
    MAX_PAGE_SIZE, MODEL_CARD_EXPORT_JOB_SETTLES_AFTER,
};

use crate::inference_deployment_lifecycle::{
    error_code, f64_field, str_field, Emulator, CREATE_SPACING,
};
use crate::model_card_export_jobs::{
    export_body, job_arn, with_card, JOB_NAMES, OUTPUT_PATHS, PAST_SETTLE_DUE,
};
use crate::model_card_governance::{content_with_description, create_body, number_field};

/// The card whose export jobs the tables below list.
const CARD: &str = "listed-card";

/// A second card, carrying export jobs of the SAME names: a job's name is unique within
/// its card, so these must never show up in `CARD`'s listing.
const OTHER_CARD: &str = "other-card";

/// The export-job names the sort and filter tables range over, created in this order.
/// Deliberately NOT in creation order, so a `Name` sort and a `CreationTime` sort cannot
/// accidentally agree.
const SEEDED: [&str; 4] = ["alpha-one", "beta-two", "demo-three", "demo-four"];

/// The service's own server-side page ceiling, read from the service rather than
/// restated, so changing it does not fail this test for an unrelated reason.
const PAGE_CEILING: usize = MAX_PAGE_SIZE as usize;

/// The `ModelCardExportJobName`s a response lists, in response order.
fn listed_names(body: &Value) -> Vec<String> {
    summaries(body)
        .iter()
        .map(|summary| str_field(summary, "ModelCardExportJobName"))
        .collect()
}

/// The response's `ModelCardExportJobSummaries`, which the model marks required — so a
/// response without it is a defect, including on an empty result.
fn summaries(body: &Value) -> Vec<Value> {
    body.get("ModelCardExportJobSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a ModelCardExportJobSummaries list in {body}"))
        .clone()
}

/// Start one export job per name against `card`, spaced so `CreatedAt` ordering is
/// unambiguous and a filter bound can be placed strictly between two jobs.
async fn create_jobs(emulator: &Emulator, card: &str, names: &[String], version: Option<i32>) {
    for name in names {
        emulator
            .ok(
                "CreateModelCardExportJob",
                &export_body(card, name, OUTPUT_PATHS[0], version),
            )
            .await;
        tokio::time::sleep(CREATE_SPACING).await;
    }
}

/// Seed [`SEEDED`] against [`CARD`] and against [`OTHER_CARD`], then wait past the
/// terminal instant the clock owes every job, so each one's status and `LastModifiedAt`
/// have settled and the assertions are not racing the clock.
async fn seed_settled_jobs(emulator: &Emulator) -> Vec<String> {
    with_card(emulator, CARD, 1).await;
    with_card(emulator, OTHER_CARD, 1).await;
    let names: Vec<String> = SEEDED.iter().map(|name| name.to_string()).collect();
    create_jobs(emulator, CARD, &names, None).await;
    create_jobs(emulator, OTHER_CARD, &names, None).await;
    tokio::time::sleep(PAST_SETTLE_DUE).await;
    names
}

/// RULE: the listing returns the export jobs of the ONE card `ModelCardName` names,
/// filtered by `ModelCardExportJobNameContains` and ordered by `SortBy`/`SortOrder` —
/// defaulting to creation time, as `SortBy`'s model doc states — with the job's name as
/// the tie-break, so equal sort keys keep one order whichever way the page is sorted.
#[tokio::test]
async fn list_model_card_export_jobs_honours_every_documented_sort_and_filter() {
    let emulator = Emulator::new();
    seed_settled_jobs(&emulator).await;

    // Rows differ along sort field (each member of `ModelCardExportJobSortBy`), sort
    // order (each member of `ModelCardExportJobSortOrder`), and the name filter's
    // selectivity.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "documented-defaults",
            json!({}),
            vec!["demo-four", "demo-three", "beta-two", "alpha-one"],
        ),
        (
            "name-ascending",
            json!({ "SortBy": "Name", "SortOrder": "Ascending" }),
            vec!["alpha-one", "beta-two", "demo-four", "demo-three"],
        ),
        (
            "name-descending",
            json!({ "SortBy": "Name", "SortOrder": "Descending" }),
            vec!["demo-three", "demo-four", "beta-two", "alpha-one"],
        ),
        (
            "creation-time-ascending",
            json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
            vec!["alpha-one", "beta-two", "demo-three", "demo-four"],
        ),
        (
            "creation-time-descending",
            json!({ "SortBy": "CreationTime", "SortOrder": "Descending" }),
            vec!["demo-four", "demo-three", "beta-two", "alpha-one"],
        ),
        // Every job has settled, so the status key is equal across the result and the
        // name tie-break decides the order — the same both ways round.
        (
            "status-ascending-falls-back-to-the-name-tie-break",
            json!({ "SortBy": "Status", "SortOrder": "Ascending" }),
            vec!["alpha-one", "beta-two", "demo-four", "demo-three"],
        ),
        (
            "status-descending-falls-back-to-the-name-tie-break",
            json!({ "SortBy": "Status", "SortOrder": "Descending" }),
            vec!["alpha-one", "beta-two", "demo-four", "demo-three"],
        ),
        (
            "name-contains-two-matches",
            json!({
                "ModelCardExportJobNameContains": "demo",
                "SortBy": "Name",
                "SortOrder": "Ascending",
            }),
            vec!["demo-four", "demo-three"],
        ),
        (
            "name-contains-one-match",
            json!({ "ModelCardExportJobNameContains": "alpha" }),
            vec!["alpha-one"],
        ),
        (
            "name-contains-no-match",
            json!({ "ModelCardExportJobNameContains": "nothing" }),
            vec![],
        ),
    ];

    for (case, extra, expected) in cases {
        let mut body = json!({ "ModelCardName": CARD });
        for (member, value) in extra.as_object().expect("the case body is an object") {
            body[member] = value.clone();
        }
        let listed = emulator.ok("ListModelCardExportJobs", &body).await;
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {case}: ListModelCardExportJobs order, got {listed:?}"
        );
        assert!(
            listed.get("NextToken").is_none(),
            "case {case}: a complete page must carry no NextToken, got {listed:?}"
        );
        // Every row belongs to the card that was asked about, never to the other card
        // holding jobs of the same names.
        for summary in summaries(&listed) {
            assert_eq!(
                str_field(&summary, "ModelCardName"),
                CARD,
                "case {case}: only {CARD}'s jobs may be listed, got {summary:?}"
            );
        }
    }
}

/// RULE: every `ModelCardExportJobSummary` replays the stored job — its name, its minted
/// ARN, its status, the card and version it exports and the instants the service recorded
/// — so the listing and `DescribeModelCardExportJob` report the same job at the same
/// instant.
#[tokio::test]
async fn export_job_summaries_replay_what_describe_reports() {
    let emulator = Emulator::new();
    let names = seed_settled_jobs(&emulator).await;

    let listed = emulator
        .ok(
            "ListModelCardExportJobs",
            &json!({
                "ModelCardName": CARD,
                "SortBy": "CreationTime",
                "SortOrder": "Ascending",
            }),
        )
        .await;
    let rows = summaries(&listed);
    assert_eq!(
        rows.len(),
        names.len(),
        "every stored export job of the card must be listed, got {listed:?}"
    );

    for (index, name) in names.iter().enumerate() {
        let summary = &rows[index];
        let arn = job_arn("us-east-1", CARD, name);
        assert_eq!(
            str_field(summary, "ModelCardExportJobName"),
            *name,
            "case {name}: ModelCardExportJobName in the summary, got {summary:?}"
        );
        assert_eq!(
            str_field(summary, "ModelCardExportJobArn"),
            arn,
            "case {name}: ModelCardExportJobArn in the summary, got {summary:?}"
        );
        assert_eq!(
            str_field(summary, "ModelCardName"),
            CARD,
            "case {name}: ModelCardName in the summary, got {summary:?}"
        );

        // The reader is the reference: the summary carries the same members it reports.
        let described = emulator
            .ok(
                "DescribeModelCardExportJob",
                &json!({ "ModelCardExportJobArn": arn }),
            )
            .await;
        for member in ["Status", "ModelCardName", "ModelCardExportJobName"] {
            assert_eq!(
                str_field(summary, member),
                str_field(&described, member),
                "case {name}: {member} must agree with DescribeModelCardExportJob, got \
                 {summary:?} and {described:?}"
            );
        }
        for member in ["ModelCardVersion", "CreatedAt", "LastModifiedAt"] {
            assert_eq!(
                number_field(summary, member),
                number_field(&described, member),
                "case {name}: {member} must agree with DescribeModelCardExportJob, got \
                 {summary:?} and {described:?}"
            );
        }
    }

    // The same holds for every shape the modelled `EntityName` admits for a job name:
    // one character, digits, hyphens and the 63-character maximum (`JOB_NAMES`).
    for job in JOB_NAMES {
        emulator
            .ok(
                "CreateModelCardExportJob",
                &export_body(CARD, job, OUTPUT_PATHS[1], None),
            )
            .await;
    }
    let listed = emulator
        .ok(
            "ListModelCardExportJobs",
            &json!({
                "ModelCardName": CARD,
                "MaxResults": 100,
                "SortBy": "Name",
                "SortOrder": "Ascending",
            }),
        )
        .await;
    let names_listed = listed_names(&listed);
    for job in JOB_NAMES {
        assert!(
            names_listed.contains(&job.to_string()),
            "case name-shape {job}: expected the job to be listed, got {names_listed:?}"
        );
    }
    for summary in summaries(&listed) {
        let name = str_field(&summary, "ModelCardExportJobName");
        assert_eq!(
            str_field(&summary, "ModelCardExportJobArn"),
            job_arn("us-east-1", CARD, &name),
            "case name-shape {name}: the summary must carry the minted ARN, got {summary:?}"
        );
    }
}

/// RULE: `ModelCardVersion` lists the export jobs of that version of the card — the
/// version each job's create resolved — and a version the card never had selects none.
/// `ModelCardName` scopes the result to one card, so a card with no export jobs, and a
/// card the store does not hold, both list nothing (this operation models no error).
#[tokio::test]
async fn export_job_listings_are_scoped_to_one_card_and_version() {
    let emulator = Emulator::new();
    with_card(&emulator, CARD, 3).await;
    with_card(&emulator, OTHER_CARD, 1).await;
    emulator
        .ok(
            "CreateModelCard",
            &create_body(
                "card-without-jobs",
                "Draft",
                &content_with_description("no exports"),
            ),
        )
        .await;

    // One job per pinned version, plus one that pinned none and so exported the card's
    // latest (3) — two jobs then share a version, which the filter must report together.
    let pinned: [(&str, Option<i32>); 4] = [
        ("pins-version-1", Some(1)),
        ("pins-version-2", Some(2)),
        ("pins-version-3", Some(3)),
        ("pins-no-version", None),
    ];
    for (job, version) in pinned {
        emulator
            .ok(
                "CreateModelCardExportJob",
                &export_body(CARD, job, OUTPUT_PATHS[0], version),
            )
            .await;
    }
    // Another card's job of a version number the filter below also names.
    emulator
        .ok(
            "CreateModelCardExportJob",
            &export_body(OTHER_CARD, "pins-version-1", OUTPUT_PATHS[1], Some(1)),
        )
        .await;

    // Rows: each version the card has, a version it never had, and the two cards that
    // hold no job of the version asked about.
    let cases: Vec<(&str, &str, Option<i32>, Vec<&str>)> = vec![
        (
            "every-version-of-the-card",
            CARD,
            None,
            vec!["pins-no-version", "pins-version-1", "pins-version-2", "pins-version-3"],
        ),
        ("first-version", CARD, Some(1), vec!["pins-version-1"]),
        ("middle-version", CARD, Some(2), vec!["pins-version-2"]),
        (
            "latest-version-covers-the-pinned-and-the-resolved-job",
            CARD,
            Some(3),
            vec!["pins-no-version", "pins-version-3"],
        ),
        ("version-the-card-never-had", CARD, Some(4), vec![]),
        ("version-zero", CARD, Some(0), vec![]),
        ("the-other-card-holds-only-its-own-job", OTHER_CARD, Some(1), vec!["pins-version-1"]),
        ("the-other-card-has-no-job-of-version-2", OTHER_CARD, Some(2), vec![]),
        ("a-card-with-no-export-jobs", "card-without-jobs", None, vec![]),
        ("a-card-the-store-does-not-hold", "no-such-card", None, vec![]),
    ];

    for (case, card, version, expected) in cases {
        let mut body = json!({
            "ModelCardName": card,
            "SortBy": "Name",
            "SortOrder": "Ascending",
        });
        if let Some(version) = version {
            body["ModelCardVersion"] = json!(version);
        }
        let listed = emulator.ok("ListModelCardExportJobs", &body).await;
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {case}: selection, got {listed:?}"
        );
        for summary in summaries(&listed) {
            assert_eq!(
                str_field(&summary, "ModelCardName"),
                card,
                "case {case}: only {card}'s jobs may be listed, got {summary:?}"
            );
            if let Some(version) = version {
                assert_eq!(
                    number_field(&summary, "ModelCardVersion"),
                    f64::from(version),
                    "case {case}: only version {version} may be listed, got {summary:?}"
                );
            }
        }
    }
}

/// RULE: `StatusEquals` returns exactly the jobs whose current status equals it, and the
/// listing reports the same status the reader does — including the terminal status the
/// clock already owes, which the listing materialises rather than reporting a status the
/// job has already left. The catalog it ranges over is the generated
/// `MODEL_CARD_EXPORT_JOB_STATUS_VALUES`, i.e. the modelled `ModelCardExportJobStatus`
/// members.
#[tokio::test]
async fn export_job_listings_filter_on_status_and_report_the_status_the_clock_owes() {
    let emulator = Emulator::new();
    with_card(&emulator, CARD, 1).await;
    let names: Vec<String> = SEEDED.iter().map(|name| name.to_string()).collect();
    // When the creates started, so the pre-due assertion below can tell a listing that
    // really did reach the jobs before their terminal status fell due from one a loaded
    // runner delayed past it.
    let started = Instant::now();
    for name in &names {
        emulator
            .ok(
                "CreateModelCardExportJob",
                &export_body(CARD, name, OUTPUT_PATHS[0], None),
            )
            .await;
    }
    let mut sorted = names.clone();
    sorted.sort();

    // Before the owed transition is due: every job is still running. Only ONE listing is
    // taken here, so the pre-due assertion rests on a single wire call rather than on a
    // loop that would have to finish inside the transition's span on a loaded host.
    let listed = emulator
        .ok(
            "ListModelCardExportJobs",
            &json!({ "ModelCardName": CARD, "SortBy": "Name", "SortOrder": "Ascending" }),
        )
        .await;
    let statuses: Vec<String> = summaries(&listed)
        .iter()
        .map(|summary| str_field(summary, "Status"))
        .collect();
    if started.elapsed() < MODEL_CARD_EXPORT_JOB_SETTLES_AFTER {
        assert_eq!(
            statuses,
            vec!["InProgress".to_string(); names.len()],
            "case before-due: a freshly created export job is reported InProgress, got \
             {listed:?}"
        );
    }
    assert_eq!(
        listed_names(&listed),
        sorted,
        "case before-due: every created job is listed, got {listed:?}"
    );

    // Past the owed transition: every job carries the terminal status, and `InProgress`
    // now selects none of them.
    tokio::time::sleep(PAST_SETTLE_DUE).await;
    for status in MODEL_CARD_EXPORT_JOB_STATUS_VALUES {
        let listed = emulator
            .ok(
                "ListModelCardExportJobs",
                &json!({
                    "ModelCardName": CARD,
                    "StatusEquals": status,
                    "SortBy": "Name",
                    "SortOrder": "Ascending",
                }),
            )
            .await;
        let expected = if *status == "Failed" {
            sorted.clone()
        } else {
            Vec::new()
        };
        assert_eq!(
            listed_names(&listed),
            expected,
            "case past-due StatusEquals={status}: selection, got {listed:?}"
        );
    }

    // The listing and the reader agree, and the status is stable across further reads.
    for name in &names {
        let described = emulator
            .ok(
                "DescribeModelCardExportJob",
                &json!({ "ModelCardExportJobArn": job_arn("us-east-1", CARD, name) }),
            )
            .await;
        assert_eq!(
            str_field(&described, "Status"),
            "Failed",
            "case {name}: the reader must agree with the listing, got {described:?}"
        );
    }
    let listed = emulator
        .ok(
            "ListModelCardExportJobs",
            &json!({ "ModelCardName": CARD, "StatusEquals": "Failed" }),
        )
        .await;
    assert_eq!(
        listed_names(&listed).len(),
        names.len(),
        "case stability: the settled status must not change on a later read, got {listed:?}"
    );
}

/// RULE: each creation-time bound selects exactly the jobs on its side of the instant,
/// and the bounds compose with each other and with the name filter. The bounds are placed
/// strictly between neighbouring instants (the creates are spaced further apart than the
/// offsets used), because an `awsJson1_1` epoch-seconds `Timestamp` is an `f64` and does
/// not round-trip a nanosecond instant exactly — so a bound landing within that
/// resolution of a stored instant is deliberately not asserted either way.
#[tokio::test]
async fn export_job_listings_filter_on_the_creation_time_bounds() {
    let emulator = Emulator::new();
    let names = seed_settled_jobs(&emulator).await;

    let listed = emulator
        .ok(
            "ListModelCardExportJobs",
            &json!({
                "ModelCardName": CARD,
                "SortBy": "CreationTime",
                "SortOrder": "Ascending",
            }),
        )
        .await;
    let created: Vec<f64> = summaries(&listed)
        .iter()
        .map(|summary| f64_field(summary, "CreatedAt"))
        .collect();
    assert_eq!(
        created.len(),
        names.len(),
        "expected one instant per seeded job, got {listed:?}"
    );
    // Bounds strictly between two neighbours, so each split is unambiguous.
    let split = (created[1] + created[2]) / 2.0;
    let upper_split = (created[2] + created[3]) / 2.0;

    // Rows differ along which bound is used, which side it selects, how many bounds
    // compose, whether a bound sits exactly on a stored instant, and whether it excludes
    // everything.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "before-the-split",
            json!({ "CreationTimeBefore": split }),
            vec!["alpha-one", "beta-two"],
        ),
        (
            "after-the-split",
            json!({ "CreationTimeAfter": split }),
            vec!["demo-three", "demo-four"],
        ),
        (
            "both-bounds-around-one-job",
            json!({ "CreationTimeAfter": split, "CreationTimeBefore": upper_split }),
            vec!["demo-three"],
        ),
        // A bound 1 ms either side of the last job's instant: the creates are spaced
        // further apart than that (`CREATE_SPACING`), so each side is unambiguous. The
        // bound is NOT placed exactly on a stored instant, because an epoch-seconds `f64`
        // does not round-trip a nanosecond instant exactly and the row would then be
        // asserting the float's rounding rather than the filter.
        (
            "a-bound-just-below-the-last-instant-selects-it",
            json!({ "CreationTimeAfter": created[3] - 0.001 }),
            vec!["demo-four"],
        ),
        (
            "a-bound-just-above-the-last-instant-selects-nothing",
            json!({ "CreationTimeAfter": created[3] + 0.001 }),
            vec![],
        ),
        (
            "a-bound-just-past-the-last-instant-selects-every-job",
            json!({ "CreationTimeBefore": created[3] + 0.001 }),
            vec!["alpha-one", "beta-two", "demo-three", "demo-four"],
        ),
        (
            "a-bound-that-excludes-everything",
            json!({ "CreationTimeAfter": created[3] + 3600.0 }),
            vec![],
        ),
        (
            "a-bound-combined-with-the-name-filter",
            json!({
                "CreationTimeAfter": split,
                "ModelCardExportJobNameContains": "demo-four",
            }),
            vec!["demo-four"],
        ),
    ];

    for (case, extra, expected) in cases {
        let mut body = json!({
            "ModelCardName": CARD,
            "SortBy": "CreationTime",
            "SortOrder": "Ascending",
        });
        for (member, value) in extra.as_object().expect("the case body is an object") {
            body[member] = value.clone();
        }
        let listed = emulator.ok("ListModelCardExportJobs", &body).await;
        assert_eq!(
            listed_names(&listed),
            expected,
            "case {case}: selection, got {listed:?}"
        );
    }
}

/// RULE: paging walks the whole ordered result exactly once — each page holds at most the
/// effective page size, a `NextToken` is present only while more remain, and feeding it
/// back resumes immediately after the previous page. The page is capped server-side, so
/// no single call serialises the whole store however large `MaxResults` is.
#[tokio::test]
async fn export_job_paging_walks_the_whole_result_exactly_once() {
    let emulator = Emulator::new();
    with_card(&emulator, CARD, 1).await;
    // More jobs than the service's page ceiling, so a capped page and an uncapped one are
    // distinguishable.
    let names: Vec<String> = (0..PAGE_CEILING + 3)
        .map(|index| format!("job-{index:03}"))
        .collect();
    for name in &names {
        emulator
            .ok(
                "CreateModelCardExportJob",
                &export_body(CARD, name, OUTPUT_PATHS[0], None),
            )
            .await;
    }

    // Rows: every page size from one entry up to past the end of the result set, plus
    // both published bounds of `MaxResults` (`range 1..100`, from
    // smithy_query(query="operation",operation="ListModelCardExportJobs")), and the
    // absent one, which takes the service's own ceiling.
    for page_size in [Some(1_usize), Some(2), Some(7), Some(52), Some(53), Some(100), None] {
        let case = format!("MaxResults={page_size:?}");
        let cap = page_size.map_or(PAGE_CEILING, |size| size.min(PAGE_CEILING));
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut body = json!({
                "ModelCardName": CARD,
                "SortBy": "Name",
                "SortOrder": "Ascending",
            });
            if let Some(size) = page_size {
                body["MaxResults"] = json!(size);
            }
            if let Some(ref value) = token {
                body["NextToken"] = Value::String(value.clone());
            }
            let listed = emulator.ok("ListModelCardExportJobs", &body).await;
            let page = listed_names(&listed);
            assert!(
                page.len() <= cap,
                "case {case}: a page must hold at most {cap} entries, got {}",
                page.len()
            );
            seen.extend(page);
            token = listed
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            pages += 1;
            assert!(
                pages <= names.len() + 1,
                "case {case}: paging must terminate, got {} entries",
                seen.len()
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, names,
            "case {case}: paging must walk the whole ordered result exactly once"
        );
    }
}

/// RULE: an input outside a documented value set is refused with HTTP 400 rather than
/// silently ignored, and the refusal writes nothing.
#[tokio::test]
async fn export_job_listings_refuse_inputs_outside_the_documented_value_sets() {
    let emulator = Emulator::new();
    let names = seed_settled_jobs(&emulator).await;

    // Rows: one outside each documented value set — `ModelCardExportJobSortBy`,
    // `ModelCardExportJobSortOrder`, `ModelCardExportJobStatus`, an unminted `NextToken`,
    // `MaxResults` one past each published bound (`range 1..100`), and the two
    // `EntityName` members outside len 1..63 / `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`.
    let too_long = "a".repeat(64);
    let cases: Vec<(&str, Value)> = vec![
        ("sort-by-outside-the-enum", json!({ "SortBy": "Nope" })),
        (
            "sort-order-outside-the-enum",
            json!({ "SortOrder": "Sideways" }),
        ),
        (
            "status-equals-outside-the-enum",
            json!({ "StatusEquals": "Cancelled" }),
        ),
        (
            "next-token-that-names-no-boundary",
            json!({ "NextToken": "not-a-token" }),
        ),
        ("max-results-below-1", json!({ "MaxResults": 0 })),
        ("max-results-above-100", json!({ "MaxResults": 101 })),
        (
            "name-contains-with-an-illegal-character",
            json!({ "ModelCardExportJobNameContains": "bad name" }),
        ),
        (
            "name-contains-past-63",
            json!({ "ModelCardExportJobNameContains": too_long.clone() }),
        ),
    ];

    for (case, extra) in cases {
        let mut body = json!({ "ModelCardName": CARD });
        for (member, value) in extra.as_object().expect("the case body is an object") {
            body[member] = value.clone();
        }
        let (status, parsed) = emulator.call("ListModelCardExportJobs", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: the listing must be refused, got {status} with {parsed}"
        );
        assert_eq!(
            error_code(&parsed),
            "ValidationException",
            "case {case}: refusal code, got {parsed:?}"
        );
    }

    // A `ModelCardName` outside the modelled `EntityName` is refused too.
    let (status, parsed) = emulator
        .call(
            "ListModelCardExportJobs",
            &json!({ "ModelCardName": "bad card name" }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case card-name-with-an-illegal-character: expected HTTP 400, got {status} with {parsed}"
    );

    // Nothing above reached state: every seeded job is still listed, and still readable
    // through the reader.
    let listed = emulator
        .ok(
            "ListModelCardExportJobs",
            &json!({ "ModelCardName": CARD, "SortBy": "Name", "SortOrder": "Ascending" }),
        )
        .await;
    assert_eq!(
        listed_names(&listed).len(),
        names.len(),
        "a refused request must write nothing, got {listed:?}"
    );
    for name in &names {
        let described = emulator
            .ok(
                "DescribeModelCardExportJob",
                &json!({ "ModelCardExportJobArn": job_arn("us-east-1", CARD, name) }),
            )
            .await;
        assert_eq!(
            str_field(&described, "ModelCardExportJobName"),
            *name,
            "case {name}: the job must still be readable, got {described:?}"
        );
    }
}
