// SPDX-License-Identifier: CC0-1.0

//! What `ListEventTypes` reports, over the wire.
//!
//! Each suite asserts a RULE over a TABLE of inputs rather than one recorded request:
//!
//! * the reported event types are the catalog this service publishes, in its own order,
//!   each summary carrying every member of `EventTypeSummary`;
//! * `Filters` selects from that catalog — OR over the values given for one filter name,
//!   AND across different names — for either filter name and either spelling of a value;
//! * `MaxResults` bounds one page and `NextToken` resumes exactly where the previous page
//!   stopped, so the pages of any page size concatenate to the unpaged answer;
//! * a token this service did not mint is refused as `InvalidNextTokenException`, and a
//!   filter name outside the modelled enum as `ValidationException`.
//!
//! The harness (`Emulator`, `error_code`, `string`, …) is the one
//! `notification_rule_lifecycle` already builds: one shared engine behind the same
//! `MultiServiceRouter` dispatch the emulator serves production requests with.

use serde_json::{json, Value};
use service_com_amazonaws_codestarnotifications_codestarnotifications_20191015 as codestar;

use crate::notification_rule_lifecycle::{error_code, string, Emulator};

/// The `EventTypeId` of every summary in a `ListEventTypes` body, in the order reported.
fn reported_ids(body: &Value) -> Vec<String> {
    body.get("EventTypes")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|summary| string(summary, "EventTypeId"))
                .collect()
        })
        .unwrap_or_default()
}

/// The catalog's own ids, in the order it publishes them.
fn catalog_ids() -> Vec<String> {
    codestar::state::EVENT_TYPE_CATALOG
        .iter()
        .map(|entry| entry.event_type_id.to_string())
        .collect()
}

/// RULE: the operation reports the event-type catalog this service publishes, in the
/// catalog's own order, and every summary carries all four members `EventTypeSummary`
/// declares with the catalog's published values — nothing rendered from the id.
///
/// The catalog holds fewer entries than the page size `MaxResults` documents as its
/// default ("The default number is 50"), so a request that names no page size answers
/// with the whole catalog and no `NextToken`.
#[tokio::test]
async fn the_unfiltered_answer_is_the_published_catalog_in_its_own_order() {
    let emulator = Emulator::new();
    let body = emulator.ok("unfiltered", "/listEventTypes", &json!({})).await;

    assert_eq!(
        reported_ids(&body),
        catalog_ids(),
        "case \"unfiltered\": expected the published catalog in its own order, got {body}"
    );
    assert!(
        body.get("NextToken").is_none(),
        "case \"unfiltered\": the catalog fits one default page, so no NextToken is owed, got {body}"
    );

    let summaries = body
        .get("EventTypes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for (summary, entry) in summaries.iter().zip(codestar::state::EVENT_TYPE_CATALOG) {
        let id = entry.event_type_id;
        assert_eq!(
            string(summary, "ServiceName"),
            entry.service_name,
            "case {id:?}: expected ServiceName {:?}, got {summary}",
            entry.service_name
        );
        assert_eq!(
            string(summary, "ResourceType"),
            entry.resource_type,
            "case {id:?}: expected ResourceType {:?}, got {summary}",
            entry.resource_type
        );
        assert_eq!(
            string(summary, "EventTypeName"),
            entry.event_type_name,
            "case {id:?}: expected EventTypeName {:?}, got {summary}",
            entry.event_type_name
        );
    }
}

/// One row of the filter table.
struct FilterCase {
    label: &'static str,
    filters: Value,
    /// The ids the row selects, in the catalog's order. LITERAL expectations taken from
    /// the catalog's published source, the event-type list under "Notification concepts"
    /// in the Developer Tools Console User Guide that `EventTypeSummary$EventTypeId`'s
    /// own documentation links to.
    expected: &'static [&'static str],
    /// Set when the row selects the whole catalog, whose ids are the catalog's own
    /// rather than a list repeated here.
    whole_catalog: bool,
}

/// RULE: `Filters` selects from the catalog — a filter matches an event type when the
/// catalogued member the filter NAME addresses equals its value, the values given for
/// one name are ORed and different names are ANDed — and an unmatched filter selects
/// nothing rather than failing.
///
/// The rows differ along what the rule generalises over: which filter name is used, how
/// many filters one request carries, whether repeated names OR or different names AND,
/// the spelling of the value (`ListEventTypesFilter$Value` documents a resource type in
/// lower case, "for example, pipeline", while the summaries report the capitalised
/// noun), and a value no entry carries.
#[tokio::test]
async fn filters_select_from_the_catalog_oring_one_name_and_anding_across_names() {
    // The CodeDeploy and CodeBuild slices of the catalog, named once so a row can pin
    // exactly the ids its filters select.
    const CODEDEPLOY: &[&str] = &[
        "codedeploy-application-deployment-failed",
        "codedeploy-application-deployment-succeeded",
        "codedeploy-application-deployment-started",
    ];
    // In the catalog's published order, so the CodeBuild in-progress event type comes
    // after the entries the published catalog puts before it rather than beside the other
    // build-state entries.
    const CODEBUILD: &[&str] = &[
        "codebuild-project-build-state-failed",
        "codebuild-project-build-state-succeeded",
        "codebuild-project-build-state-stopped",
        "codebuild-project-build-phase-failure",
        "codebuild-project-build-phase-success",
        "codebuild-project-build-state-in-progress",
    ];
    const CODEBUILD_THEN_CODEDEPLOY: &[&str] = &[
        "codebuild-project-build-state-failed",
        "codebuild-project-build-state-succeeded",
        "codebuild-project-build-state-stopped",
        "codebuild-project-build-phase-failure",
        "codebuild-project-build-phase-success",
        "codedeploy-application-deployment-failed",
        "codedeploy-application-deployment-succeeded",
        "codedeploy-application-deployment-started",
        "codebuild-project-build-state-in-progress",
    ];

    let cases: &[FilterCase] = &[
        FilterCase {
            label: "an empty filter list selects the whole catalog",
            filters: json!([]),
            expected: &[],
            whole_catalog: true,
        },
        FilterCase {
            label: "SERVICE_NAME selects one service",
            filters: json!([{ "Name": "SERVICE_NAME", "Value": "CodeDeploy" }]),
            expected: CODEDEPLOY,
            whole_catalog: false,
        },
        FilterCase {
            label: "RESOURCE_TYPE selects the same entries by their resource type",
            filters: json!([{ "Name": "RESOURCE_TYPE", "Value": "Application" }]),
            expected: CODEDEPLOY,
            whole_catalog: false,
        },
        FilterCase {
            label: "a resource type spelled in lower case names the same type",
            filters: json!([{ "Name": "RESOURCE_TYPE", "Value": "application" }]),
            expected: CODEDEPLOY,
            whole_catalog: false,
        },
        FilterCase {
            label: "two values for one name are ORed",
            filters: json!([
                { "Name": "SERVICE_NAME", "Value": "CodeDeploy" },
                { "Name": "SERVICE_NAME", "Value": "CodeBuild" },
            ]),
            expected: CODEBUILD_THEN_CODEDEPLOY,
            whole_catalog: false,
        },
        FilterCase {
            label: "two different names are ANDed and agree",
            filters: json!([
                { "Name": "SERVICE_NAME", "Value": "CodeBuild" },
                { "Name": "RESOURCE_TYPE", "Value": "Project" },
            ]),
            expected: CODEBUILD,
            whole_catalog: false,
        },
        FilterCase {
            label: "two different names are ANDed and disagree",
            filters: json!([
                { "Name": "SERVICE_NAME", "Value": "CodeDeploy" },
                { "Name": "RESOURCE_TYPE", "Value": "Project" },
            ]),
            expected: &[],
            whole_catalog: false,
        },
        FilterCase {
            label: "a service name no entry carries selects nothing",
            filters: json!([{ "Name": "SERVICE_NAME", "Value": "CodeStarNotifications" }]),
            expected: &[],
            whole_catalog: false,
        },
    ];

    let emulator = Emulator::new();
    for case in cases {
        let body = emulator
            .ok(
                case.label,
                "/listEventTypes",
                &json!({ "Filters": case.filters, "MaxResults": 100 }),
            )
            .await;
        let expected: Vec<String> = if case.whole_catalog {
            catalog_ids()
        } else {
            case.expected.iter().map(|id| (*id).to_string()).collect()
        };
        assert_eq!(
            reported_ids(&body),
            expected,
            "case {:?}: expected {expected:?}, got {body}",
            case.label
        );
    }
}

/// RULE: `MaxResults` bounds one page, `NextToken` is reported exactly while entries
/// remain, and a request carrying it resumes where the previous page stopped — so the
/// pages of ANY page size concatenate to the unpaged answer, each one no longer than the
/// requested size.
///
/// The sizes include both published bounds of `MaxResults` (`@range(min: 1, max: 100)`)
/// and sizes in between that do and do not divide the catalog.
#[tokio::test]
async fn pages_of_any_size_concatenate_to_the_whole_catalog() {
    let emulator = Emulator::new();
    let whole = catalog_ids();

    for page_size in [1_usize, 2, 7, 38, 100] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let mut request = json!({ "MaxResults": page_size });
            if let Some(next) = token.clone() {
                request["NextToken"] = Value::String(next);
            }
            let body = emulator
                .ok(
                    &format!("page size {page_size}"),
                    "/listEventTypes",
                    &request,
                )
                .await;
            let page = reported_ids(&body);
            assert!(
                page.len() <= page_size,
                "case {page_size}: a page holds at most the requested {page_size}, got {}",
                page.len()
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= whole.len() + 1,
                "case {page_size}: paging must terminate, got {pages} pages for {} entries",
                whole.len()
            );
            token = body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            let remaining = whole.len() - walked.len();
            assert_eq!(
                token.is_some(),
                remaining > 0,
                "case {page_size}: NextToken is owed exactly while entries remain ({remaining} left), got {body}"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, whole,
            "case {page_size}: the pages must concatenate to the whole catalog, got {walked:?}"
        );
        assert_eq!(
            pages,
            whole.len().div_ceil(page_size),
            "case {page_size}: expected {} pages for {} entries, got {pages}",
            whole.len().div_ceil(page_size),
            whole.len()
        );
    }
}

/// RULE: paging a filtered listing walks only the selected entries, so the filter stays
/// in force across every page.
#[tokio::test]
async fn a_filtered_listing_pages_over_only_the_selected_entries() {
    let emulator = Emulator::new();
    let filters = json!([{ "Name": "SERVICE_NAME", "Value": "CodeCommit" }]);
    let unpaged = emulator
        .ok(
            "codecommit unpaged",
            "/listEventTypes",
            &json!({ "Filters": filters, "MaxResults": 100 }),
        )
        .await;
    let expected = reported_ids(&unpaged);
    assert!(
        !expected.is_empty(),
        "case \"codecommit unpaged\": the catalog holds CodeCommit event types, got {unpaged}"
    );

    for page_size in [1_usize, 3] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({ "Filters": filters, "MaxResults": page_size });
            if let Some(next) = token.clone() {
                request["NextToken"] = Value::String(next);
            }
            let body = emulator
                .ok(
                    &format!("codecommit page size {page_size}"),
                    "/listEventTypes",
                    &request,
                )
                .await;
            walked.extend(reported_ids(&body));
            token = body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                walked.len() <= expected.len(),
                "case {page_size}: paging a filtered listing must terminate, got {walked:?}"
            );
        }
        assert_eq!(
            walked, expected,
            "case {page_size}: a filtered listing pages over only its own entries, got {walked:?}"
        );
    }
}

/// RULE: a `NextToken` this service did not mint is refused as HTTP 400
/// `InvalidNextTokenException` with the text the model states for it, and a `MaxResults`
/// or a filter name outside the modelled constraint is refused as HTTP 400
/// `ValidationException` — never served as an empty page.
#[tokio::test]
async fn an_unmintable_token_or_an_unmodelled_parameter_is_refused() {
    // (label, request, expected error code)
    let cases: &[(&str, Value, &str)] = &[
        (
            "a token of the right shape this service never minted",
            json!({ "NextToken": "zzzz" }),
            "InvalidNextTokenException",
        ),
        (
            // Hex of `v9:3` — decodable, but not this service's token version.
            "a token carrying another version prefix",
            json!({ "NextToken": "76393a33" }),
            "InvalidNextTokenException",
        ),
        (
            "a filter name outside ListEventTypesFilterName",
            json!({ "Filters": [{ "Name": "EVENT_TYPE_ID", "Value": "CodeBuild" }] }),
            "ValidationException",
        ),
        (
            // `MaxResults` is `@range(min: 1, max: 100)`: one past each bound.
            "MaxResults below the modelled minimum",
            json!({ "MaxResults": 0 }),
            "ValidationException",
        ),
        (
            "MaxResults above the modelled maximum",
            json!({ "MaxResults": 101 }),
            "ValidationException",
        ),
    ];

    let emulator = Emulator::new();
    for (label, request, expected_code) in cases {
        let (status, body) = emulator.post("/listEventTypes", request).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            *expected_code,
            "case {label:?}: expected {expected_code}, got {body}"
        );
    }

    // The published bounds themselves are served.
    for page_size in [1, 100] {
        let body = emulator
            .ok(
                &format!("MaxResults {page_size}"),
                "/listEventTypes",
                &json!({ "MaxResults": page_size }),
            )
            .await;
        let page = reported_ids(&body);
        assert!(
            !page.is_empty() && page.len() <= page_size,
            "case \"MaxResults {page_size}\": a bound page size is served, got {body}"
        );
    }
}

/// RULE: this operation reports and persists nothing, so repeating it answers the same
/// entries and leaves what a later caller sees unchanged.
#[tokio::test]
async fn repeating_the_listing_reports_the_same_entries() {
    let emulator = Emulator::new();
    let first = emulator.ok("first", "/listEventTypes", &json!({})).await;
    let second = emulator.ok("second", "/listEventTypes", &json!({})).await;
    assert_eq!(
        reported_ids(&first),
        reported_ids(&second),
        "case \"repeat\": a read must not change what a later caller sees, got {second}"
    );
}
