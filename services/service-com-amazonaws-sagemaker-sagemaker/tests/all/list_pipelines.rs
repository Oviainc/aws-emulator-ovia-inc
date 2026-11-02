// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for `ListPipelines`.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over TABLES of inputs rather than one recorded call: the
//! listing reports one summary per STORED pipeline, projecting what the create stored;
//! `PipelineNamePrefix` and the creation-time bounds select which pipelines it reports;
//! `SortBy`/`SortOrder` order them and a value outside either modelled enum is refused;
//! `MaxResults`/`NextToken` walk the result set exactly once whatever the page size; and
//! a deleted pipeline is listed no more.

use serde_json::{json, Value};

use super::inference_deployment_lifecycle::{
    error_code, str_field, Emulator, ACCOUNT, CREATE_SPACING,
};
use super::pipeline_lifecycle::{definition, step, token};

/// The `PipelineName`s of a listing response, in the order it reported them.
fn listed_names(response: &Value) -> Vec<String> {
    response
        .get("PipelineSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a PipelineSummaries list in {response}"))
        .iter()
        .map(|summary| str_field(summary, "PipelineName"))
        .collect()
}

/// Create the pipeline `name`, carrying `extra`'s optional members, and answer its ARN.
async fn create(emulator: &Emulator, name: &str, extra: &Value) -> String {
    let mut body = json!({
        "PipelineName": name,
        "PipelineDefinition": definition(json!([step("Only", &[])])),
        "ClientRequestToken": token(name),
        "RoleArn": format!("arn:aws:iam::{ACCOUNT}:role/service-role/pipeline-role"),
    });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member.as_str()] = value.clone();
        }
    }
    let created = emulator.ok("CreatePipeline", &body).await;
    // The creates are spaced, so `CreationTime` orders the pipelines by creation.
    tokio::time::sleep(CREATE_SPACING).await;
    str_field(&created, "PipelineArn")
}

/// RULE: the listing reports ONE summary per stored pipeline, projecting what the create
/// stored — the minted ARN, the name, the optional display name and description, the role
/// and the creation and last-modified instants — and reports no execution time for a
/// pipeline that never ran. A pipeline deleted since is reported no more.
#[tokio::test]
async fn a_listing_projects_every_stored_pipeline() {
    let emulator = Emulator::new();

    // Rows differ along the optional members the create carried.
    let cases: Vec<(&str, Value)> = vec![
        ("required-only", json!({})),
        (
            "with-display-name",
            json!({ "PipelineDisplayName": "Shown-To-Humans" }),
        ),
        (
            "with-display-name-and-description",
            json!({
                "PipelineDisplayName": "Also-Shown",
                "PipelineDescription": "a one-step pipeline",
            }),
        ),
    ];

    let mut arns = Vec::new();
    for (name, extra) in &cases {
        arns.push(create(&emulator, name, extra).await);
    }

    let listing = emulator.ok("ListPipelines", &json!({})).await;
    let summaries = listing
        .get("PipelineSummaries")
        .and_then(Value::as_array)
        .expect("a PipelineSummaries list")
        .clone();
    assert_eq!(
        summaries.len(),
        cases.len(),
        "case every-stored-pipeline: one summary per stored pipeline, got {listing}"
    );

    for ((name, extra), arn) in cases.iter().zip(&arns) {
        let summary = summaries
            .iter()
            .find(|summary| str_field(summary, "PipelineName") == *name)
            .unwrap_or_else(|| panic!("case {name}: no summary for the pipeline in {listing}"));
        assert_eq!(
            (
                str_field(summary, "PipelineArn"),
                str_field(summary, "RoleArn"),
            ),
            (
                arn.clone(),
                format!("arn:aws:iam::{ACCOUNT}:role/service-role/pipeline-role"),
            ),
            "case {name}: the summary replays the stored ARN and role, got {summary}"
        );
        assert_eq!(
            summary.get("PipelineDescription"),
            extra.get("PipelineDescription"),
            "case {name}: PipelineDescription must be what the create stored, got {summary}"
        );
        // The display name is the one the create stated, or the pipeline's own name when
        // it stated none (pinned in `service_resolved_members`).
        assert_eq!(
            summary.get("PipelineDisplayName").and_then(Value::as_str),
            Some(
                extra
                    .get("PipelineDisplayName")
                    .and_then(Value::as_str)
                    .unwrap_or(name)
            ),
            "case {name}: PipelineDisplayName, got {summary}"
        );
        assert_eq!(
            summary.get("CreationTime"),
            summary.get("LastModifiedTime"),
            "case {name}: an untouched pipeline was last modified when it was created, \
             got {summary}"
        );
        assert!(
            summary.get("LastExecutionTime").is_none(),
            "case {name}: a pipeline that never ran reports no LastExecutionTime, got \
             {summary}"
        );
    }

    // A pipeline that HAS run reports when its run began; one that has not still does
    // not, so the member follows the stored record rather than the listing.
    emulator
        .ok(
            "StartPipelineExecution",
            &json!({ "PipelineName": "required-only", "ClientRequestToken": token("run") }),
        )
        .await;
    let listing = emulator.ok("ListPipelines", &json!({})).await;
    let summaries = listing
        .get("PipelineSummaries")
        .and_then(Value::as_array)
        .expect("a PipelineSummaries list")
        .clone();
    for summary in &summaries {
        let ran = str_field(summary, "PipelineName") == "required-only";
        assert_eq!(
            summary.get("LastExecutionTime").is_some(),
            ran,
            "case after-a-run: only the pipeline that ran reports LastExecutionTime, got \
             {summary}"
        );
    }

    // And a deleted pipeline is listed no more.
    emulator
        .ok(
            "DeletePipeline",
            &json!({ "PipelineName": "required-only", "ClientRequestToken": token("gone") }),
        )
        .await;
    let listing = emulator.ok("ListPipelines", &json!({})).await;
    assert_eq!(
        listed_names(&listing),
        vec![
            "with-display-name-and-description".to_string(),
            "with-display-name".to_string()
        ],
        "case deleted: a deleted pipeline is listed no more, got {listing}"
    );
}

/// RULE: `PipelineNamePrefix` selects the pipelines whose name STARTS with it and the
/// creation-time bounds select by stored creation instant; `SortBy` and `SortOrder` order
/// the result set, and a value outside either modelled enum is refused with 400
/// `ValidationException` rather than silently ordered by another key.
#[tokio::test]
async fn filters_and_sort_order_decide_which_pipelines_come_back_and_in_what_order() {
    let emulator = Emulator::new();

    // Created in this order, so creation time and name disagree — an order that follows
    // one cannot be mistaken for the other.
    for name in ["gamma-one", "alpha-two", "gamma-three", "beta-four"] {
        create(&emulator, name, &json!({})).await;
    }

    // Rows differ along the filter and the ordering asked for.
    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "no-filter-default-order",
            json!({}),
            // The documented defaults: by `CreationTime`, `Descending` — the newest
            // pipeline first, which is the reverse of the creation order above.
            vec!["beta-four", "gamma-three", "alpha-two", "gamma-one"],
        ),
        (
            "creation-time-ascending",
            json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
            vec!["gamma-one", "alpha-two", "gamma-three", "beta-four"],
        ),
        (
            "by-name-ascending",
            json!({ "SortBy": "Name", "SortOrder": "Ascending" }),
            vec!["alpha-two", "beta-four", "gamma-one", "gamma-three"],
        ),
        (
            "by-name-descending",
            json!({ "SortBy": "Name", "SortOrder": "Descending" }),
            vec!["gamma-three", "gamma-one", "beta-four", "alpha-two"],
        ),
        (
            "prefix-matching-two",
            json!({ "PipelineNamePrefix": "gamma", "SortBy": "Name", "SortOrder": "Ascending" }),
            vec!["gamma-one", "gamma-three"],
        ),
        (
            "prefix-matching-one",
            json!({ "PipelineNamePrefix": "beta" }),
            vec!["beta-four"],
        ),
        (
            // A prefix is not a substring match: `one` ENDS a stored name.
            "prefix-matching-nothing",
            json!({ "PipelineNamePrefix": "one" }),
            vec![],
        ),
    ];

    for (case, request, expected) in cases {
        let listing = emulator.ok("ListPipelines", &request).await;
        assert_eq!(
            listed_names(&listing),
            expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<String>>(),
            "case {case}: the pipelines the request selects, in order, got {listing}"
        );
    }

    // The creation-time bounds select by the pipeline's STORED creation instant: a bound
    // between two creates splits the result set there, and the two bounds are
    // complementary over the same instant.
    let ascending = emulator
        .ok(
            "ListPipelines",
            &json!({ "SortBy": "CreationTime", "SortOrder": "Ascending" }),
        )
        .await;
    let summaries = ascending
        .get("PipelineSummaries")
        .and_then(Value::as_array)
        .expect("a PipelineSummaries list")
        .clone();
    // Halfway between the second and third create, so the split falls between them
    // whatever precision the reported instants carry.
    let created_at = |index: usize| -> f64 {
        summaries[index]
            .get("CreationTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("a CreationTime on summary {index} of {ascending}"))
    };
    let between = (created_at(1) + created_at(2)) / 2.0;
    for (case, bound, expected) in [
        (
            "created-after-the-split",
            "CreatedAfter",
            vec!["gamma-three", "beta-four"],
        ),
        (
            "created-before-the-split",
            "CreatedBefore",
            vec!["gamma-one", "alpha-two"],
        ),
    ] {
        let listing = emulator
            .ok(
                "ListPipelines",
                &json!({
                    bound: between,
                    "SortBy": "CreationTime",
                    "SortOrder": "Ascending",
                }),
            )
            .await;
        assert_eq!(
            listed_names(&listing),
            expected
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<String>>(),
            "case {case}: the bound selects by the stored creation instant, got {listing}"
        );
    }

    // A sort key or order outside the modelled enums is refused, not quietly replaced.
    for (case, request) in [
        ("unknown-sort-by", json!({ "SortBy": "LastModifiedTime" })),
        ("unknown-sort-order", json!({ "SortOrder": "Rising" })),
    ] {
        let (status, body) = emulator.call("ListPipelines", &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case}: a value outside the modelled enum is refused, got {status} \
             {body:?}"
        );
    }
}

/// RULE: `MaxResults` caps one page and `NextToken` resumes after the page boundary, so a
/// client walking the token chain sees the whole result set exactly once, in the sort
/// order asked for, whatever the page size — and the last page carries no token.
#[tokio::test]
async fn pagination_walks_the_whole_result_set_exactly_once() {
    let emulator = Emulator::new();

    let names: Vec<String> = (0..7).map(|index| format!("p{index}")).collect();
    for name in &names {
        create(&emulator, name, &json!({})).await;
    }

    // Rows differ along the page size: one at a time, an uneven split, the whole set, and
    // more than the set holds.
    for page_size in [1_usize, 3, 7, 50] {
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({
                "MaxResults": page_size,
                "SortBy": "Name",
                "SortOrder": "Ascending",
            });
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let listing = emulator.ok("ListPipelines", &request).await;
            let page = listed_names(&listing);
            assert!(
                page.len() <= page_size,
                "case page-size-{page_size}: a page is capped at MaxResults, got {listing}"
            );
            walked.extend(page);
            pages += 1;
            next_token = listing
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if next_token.is_none() {
                break;
            }
            assert!(
                pages <= names.len(),
                "case page-size-{page_size}: the token chain must terminate, walked \
                 {walked:?}"
            );
        }
        assert_eq!(
            walked, names,
            "case page-size-{page_size}: the token chain reports every pipeline exactly \
             once, in order"
        );
    }

    // A token this listing never issued is refused rather than silently restarting.
    let (status, body) = emulator
        .call("ListPipelines", &json!({ "NextToken": "not-a-token" }))
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case unknown-token: a token no listing issued is refused, got {status} {body:?}"
    );

    // An account holding no pipeline lists the empty list rather than an absent member.
    let empty = Emulator::new();
    let listing = empty.ok("ListPipelines", &json!({})).await;
    assert_eq!(
        listing.get("PipelineSummaries"),
        Some(&json!([])),
        "case nothing-stored: the listing is empty, not absent, got {listing}"
    );
    assert!(
        listing.get("NextToken").is_none(),
        "case nothing-stored: an exhausted listing carries no token, got {listing}"
    );
}
