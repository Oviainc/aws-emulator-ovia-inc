// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for walking the telemetry-pipeline collection.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — through
//! the ONE harness this test crate has (`telemetry_rules`), so what is asserted is
//! what a signed AWS SDK request observes. The create/read side of the slice is
//! covered by [`crate::telemetry_pipelines`]; what is asserted here is the operation
//! that walks the collection.
//!
//! The rules under test:
//!
//! 1. a listing reports a summary of every pipeline the store holds in the region the
//!    request was signed for and of no pipeline of another region, in the store's own
//!    (name) order, and every member of a summary is read from the same record a read
//!    of that one pipeline reports — so a listing and a read never disagree;
//! 2. `MaxResults` bounds ONE page and the `NextToken` beside it resumes exactly where
//!    that page stopped, so the pages of one walk cover the region's pipelines once
//!    each; a page with nothing left beyond it carries no token;
//! 3. `MaxResults` is held to the range its own shape declares: each published bound is
//!    served, and a value past either bound is refused with the modelled 400 and
//!    reports no page at all;
//! 4. a `NextToken` carrying anything this operation could not have issued — a value
//!    outside the form a pipeline name takes — is refused with the modelled 400 rather
//!    than silently answering a page, while a well-formed name the store no longer
//!    holds stays a POSITION: the walk resumes at it;
//! 5. the status a summary reports is derived from the record's own due instant and the
//!    clock, and the listing itself writes nothing.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_observabilityadmin_observabilityadmin::ops::types::TELEMETRY_PIPELINE_STATUS_VALUES;
use service_com_amazonaws_observabilityadmin_observabilityadmin::state::{
    now_epoch_millis, pipeline_arn, ComAmazonawsObservabilityadminObservabilityadminState,
    PIPELINE_ACTIVATION_DELAY_MS,
};

use crate::telemetry_rules::{call, emulator, error_code, Emulator};

/// The region most requests below sign for.
const HOME_REGION: &str = "us-west-2";

/// A second region, so "only the pipelines of the caller's own region" can be shown to
/// be per-region rather than store-wide.
const OTHER_REGION: &str = "eu-west-1";

/// Long enough after a create for the `CREATING` -> `ACTIVE` transition to have come
/// due, whatever scheduling delay the test run adds on top.
fn past_activation_delay() -> Duration {
    Duration::from_millis(PIPELINE_ACTIVATION_DELAY_MS as u64 + 250)
}

/// Well-formed configuration documents, so the `ConfigurationSummary` a listing reports
/// differs row to row: one with no processors, one with two processors under a named
/// section, and one with two sinks. The round trip of a document through the create is
/// pinned by [`crate::telemetry_pipelines`]; what varies here is only that the summaries
/// a listing reports are not all alike.
const CONFIGURATIONS: [&str; 3] = [
    "source:\n  cloudwatch_logs:\n    log_group: /aws/lambda/example\n\
     sink:\n  - cloudwatch_logs:\n      log_group: /aws/telemetry/example\n",
    "version: \"2\"\nlog-pipeline:\n  source:\n    s3:\n      bucket: example-telemetry\n  \
     processor:\n    - grok:\n        match: {}\n    - date:\n        from_time_received: true\n  \
     sink:\n    - cloudwatch_logs:\n        log_group: /aws/telemetry/example\n",
    "source:\n  http: {}\nsink:\n  - cloudwatch_logs: {}\n  - s3:\n      bucket: example-archive\n",
];

/// Create one pipeline named `name` in `region` over the wire. `index` picks the
/// configuration document and whether the create carries tags, so a filled region holds
/// pipelines that differ in both.
async fn create(emulator: &Emulator, region: &str, name: &str, index: usize, case: &str) {
    let body = CONFIGURATIONS[index % CONFIGURATIONS.len()];
    let mut request = json!({ "Name": name, "Configuration": { "Body": body } });
    if index.is_multiple_of(2) {
        request["Tags"] = json!({ "team": "observability", "tier": index.to_string() });
    }
    let (status, response) = call(emulator, region, "/CreateTelemetryPipeline", &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the create of {name} in {region} to answer 200, got {status} \
         {response:?}"
    );
}

/// Put `count` pipelines in `region` and answer their names in the order the store keys
/// them (by name).
///
/// The creates are issued in REVERSE name order, so the order a listing reports is the
/// store's own rather than the order the creates arrived in.
async fn fill(
    emulator: &Emulator,
    region: &str,
    count: usize,
    prefix: &str,
    case: &str,
) -> Vec<String> {
    let mut names: Vec<String> = (0..count)
        .map(|index| format!("{prefix}-pipeline-{index:02}"))
        .collect();
    for index in (0..count).rev() {
        create(emulator, region, &names[index], index, case).await;
    }
    names.sort();
    names
}

/// One listing page: the summaries it carried and the `NextToken` beside them.
async fn list(
    emulator: &Emulator,
    region: &str,
    request: &Value,
    case: &str,
) -> (Vec<Value>, Option<String>) {
    let (status, response) = call(emulator, region, "/ListTelemetryPipelines", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the listing to answer 200, got {status} {response:?}"
    );
    let summaries = response
        .get("PipelineSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: a listing answers PipelineSummaries, got {response:?}")
        })
        .clone();
    let next = response
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    (summaries, next)
}

/// Every pipeline `region` reports, walked page by page with `page_size` (or in one call
/// when it is `None`), with the number of pages the walk took.
async fn walk(
    emulator: &Emulator,
    region: &str,
    page_size: Option<usize>,
    case: &str,
) -> (Vec<Value>, usize) {
    let mut seen: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut request = json!({});
        if let Some(size) = page_size {
            request["MaxResults"] = json!(size);
        }
        if let Some(token) = &token {
            request["NextToken"] = json!(token);
        }
        let (page, next) = list(emulator, region, &request, case).await;
        pages += 1;
        if let Some(size) = page_size {
            assert!(
                page.len() <= size,
                "case {case:?}: expected at most {size} summaries per page, got {}",
                page.len()
            );
        }
        if next.is_some() {
            assert_eq!(
                page.len(),
                page_size.unwrap_or(page.len()),
                "case {case:?}: a NextToken may only follow a page that filled the limit, got \
                 {page:?}"
            );
        }
        seen.extend(page);
        match next {
            Some(next) => token = Some(next),
            None => break,
        }
        assert!(
            pages < 64,
            "case {case:?}: expected the walk to terminate, got {pages} pages"
        );
    }
    (seen, pages)
}

/// The `Name` of each summary, in the order the listing reported them.
fn names_of(summaries: &[Value]) -> Vec<String> {
    summaries
        .iter()
        .map(|summary| {
            summary
                .get("Name")
                .and_then(Value::as_str)
                .unwrap_or("<no Name>")
                .to_string()
        })
        .collect()
}

/// The `Status` one summary reported.
fn status_of(summary: &Value) -> String {
    summary
        .get("Status")
        .and_then(Value::as_str)
        .unwrap_or("<no Status>")
        .to_string()
}

async fn get(emulator: &Emulator, region: &str, identifier: &str) -> (u16, Value) {
    call(
        emulator,
        region,
        "/GetTelemetryPipeline",
        &json!({ "PipelineIdentifier": identifier }),
    )
    .await
}

/// How many pipeline records the shared store holds.
fn stored_count(emulator: &Emulator) -> usize {
    emulator
        .engine
        .state::<ComAmazonawsObservabilityadminObservabilityadminState>()
        .lock()
        .telemetry_pipelines
        .len()
}

/// The create instant and the instant the `CREATING` -> `ACTIVE` transition comes due,
/// read straight off the stored record.
fn record_instants(emulator: &Emulator, region: &str, name: &str) -> (i64, i64) {
    let state = emulator
        .engine
        .state::<ComAmazonawsObservabilityadminObservabilityadminState>();
    let store = state.lock();
    let record = store
        .pipeline(region, name)
        .unwrap_or_else(|| panic!("the create stored a record for {name}"));
    (record.created_timestamp_ms, record.active_at_ms)
}

/// A listing reports every pipeline of the caller's own region and none of another
/// region's, whatever the number each region holds — each summary read from the same
/// record the read of that one pipeline reports, and the listing changes nothing a later
/// caller sees.
#[tokio::test]
async fn a_listing_reports_this_regions_pipelines_and_agrees_with_a_read() {
    // The rows differ along what the rule generalises over: how many pipelines the
    // caller's region holds (none, one, several) and how many another region holds
    // beside them.
    for (case, home, other) in [
        ("no pipeline at all", 0usize, 0usize),
        ("one pipeline, nothing elsewhere", 1, 0),
        ("one pipeline beside another region's", 1, 2),
        ("several pipelines beside another region's", 4, 3),
        ("nothing here, several elsewhere", 0, 2),
    ] {
        let emulator = emulator();
        let home_names = fill(&emulator, HOME_REGION, home, "home", case).await;
        let other_names = fill(&emulator, OTHER_REGION, other, "other", case).await;
        // Past the activation span, so the status a summary and a read report is the
        // terminal one for both and the comparison below carries no wall-clock race.
        tokio::time::sleep(past_activation_delay()).await;

        let (summaries, pages) = walk(&emulator, HOME_REGION, None, case).await;
        assert_eq!(
            pages, 1,
            "case {case:?}: expected one unpaged call to answer every pipeline, got {pages} pages"
        );
        assert_eq!(
            names_of(&summaries),
            home_names,
            "case {case:?}: expected the listing to report exactly {HOME_REGION}'s pipelines, in \
             the store's own name order, got {summaries:?}"
        );

        for summary in &summaries {
            let name = summary
                .get("Name")
                .and_then(Value::as_str)
                .unwrap_or("<no Name>")
                .to_string();
            let (status, read) = get(&emulator, HOME_REGION, &name).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the read of {name} to answer 200, got {status} {read:?}"
            );
            let pipeline = read
                .get("Pipeline")
                .unwrap_or_else(|| panic!("case {case:?}: a read answers a Pipeline: {read:?}"));
            // Every member `TelemetryPipelineSummary` declares comes from the record the
            // read reports, so the two cannot disagree on any of them.
            for member in [
                "Arn",
                "Name",
                "Status",
                "Tags",
                "CreatedTimeStamp",
                "LastUpdateTimeStamp",
                "ConfigurationSummary",
            ] {
                assert_eq!(
                    summary.get(member),
                    pipeline.get(member),
                    "case {case:?}: expected the summary of {name} to report the {member} the \
                     read does, got {summary:?} against {pipeline:?}"
                );
            }
            // The ARN is the one this service mints for that record, in the caller's own
            // region.
            let arn = summary.get("Arn").and_then(Value::as_str).unwrap_or_default();
            let id = arn.rsplit('/').next().unwrap_or_default();
            assert_eq!(
                arn,
                pipeline_arn(HOME_REGION, id),
                "case {case:?}: expected a {HOME_REGION} pipeline ARN, got {arn:?}"
            );
            // `TelemetryPipelineSummary` declares no `Configuration`: the configuration
            // document itself is what a read of one pipeline carries.
            assert!(
                summary.get("Configuration").is_none(),
                "case {case:?}: a summary carries no configuration body, got {summary:?}"
            );
            assert_eq!(
                status_of(summary),
                "ACTIVE",
                "case {case:?}: past the activation span every pipeline is ACTIVE, got {summary:?}"
            );
        }

        // The other region's listing is its own, and neither listing moved a record.
        let (elsewhere, _) = walk(&emulator, OTHER_REGION, None, case).await;
        assert_eq!(
            names_of(&elsewhere),
            other_names,
            "case {case:?}: expected {OTHER_REGION}'s listing to report its own pipelines, got \
             {elsewhere:?}"
        );
        assert_eq!(
            stored_count(&emulator),
            home + other,
            "case {case:?}: expected the listings to leave all {} records alone, got {}",
            home + other,
            stored_count(&emulator)
        );
    }
}

/// `MaxResults` bounds one page and the `NextToken` beside it resumes exactly where that
/// page stopped: for every page size, the walk reports each pipeline once, in the order
/// one unpaged call reports them.
#[tokio::test]
async fn max_results_bounds_a_page_and_next_token_resumes_after_it() {
    let count = 7usize;
    let emulator = emulator();
    let names = fill(&emulator, HOME_REGION, count, "home", "paged walk").await;
    let (unpaged, _) = walk(&emulator, HOME_REGION, None, "paged walk").await;
    assert_eq!(
        names_of(&unpaged),
        names,
        "expected the unpaged listing to report every pipeline, got {unpaged:?}"
    );

    // The rows span the page size against the number of pipelines: the smallest size the
    // model admits, sizes under the count (one of them an exact divisor of it), the count
    // itself, one over it, and the largest size the model admits.
    for size in [1usize, 2, 3, count, count + 1, 100] {
        let case = format!("MaxResults {size} over {count} pipelines");
        let (seen, pages) = walk(&emulator, HOME_REGION, Some(size), &case).await;
        assert_eq!(
            names_of(&seen),
            names,
            "case {case:?}: expected the paged walk to report the unpaged order once each, got \
             {seen:?}"
        );
        let expected_pages = (count / size + usize::from(!count.is_multiple_of(size))).max(1);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, got {pages}"
        );
    }
}

/// `MaxResults` is served at each bound its own shape publishes and refused past either
/// one, and a refused call reports no page and leaves the collection alone.
#[tokio::test]
async fn max_results_is_served_at_each_published_bound_and_refused_past_it() {
    // The bounds are the ones `ListTelemetryPipelinesMaxResults` declares:
    // smithy_query(query="shape",shape="ListTelemetryPipelinesMaxResults") reports
    // range 1..100.
    const MIN_MAX_RESULTS: i64 = 1;
    const MAX_MAX_RESULTS: i64 = 100;
    let count = 3usize;
    let emulator = emulator();
    fill(&emulator, HOME_REGION, count, "home", "MaxResults bounds").await;
    let (everything, _) = walk(&emulator, HOME_REGION, None, "MaxResults bounds").await;

    for served in [MIN_MAX_RESULTS, MAX_MAX_RESULTS] {
        let case = format!("MaxResults {served}, a published bound");
        let (page, next) = list(
            &emulator,
            HOME_REGION,
            &json!({ "MaxResults": served }),
            &case,
        )
        .await;
        let expected_len = (served as usize).min(count);
        assert_eq!(
            page.len(),
            expected_len,
            "case {case:?}: expected {expected_len} summaries, got {page:?}"
        );
        assert_eq!(
            names_of(&page),
            names_of(&everything[..expected_len]),
            "case {case:?}: expected the page to be the first {expected_len} of the listing's own \
             order, got {page:?}"
        );
        assert_eq!(
            next.is_some(),
            expected_len < count,
            "case {case:?}: a NextToken follows a page only when something is left beyond it, got \
             {next:?}"
        );
    }

    // One step past each published bound, and the values beyond any page a caller could
    // ask for.
    for refused in [
        MIN_MAX_RESULTS - 1,
        MAX_MAX_RESULTS + 1,
        -1,
        i64::from(i32::MAX),
    ] {
        let case = format!("MaxResults {refused}, outside the modelled range");
        let (status, response) = call(
            &emulator,
            HOME_REGION,
            "/ListTelemetryPipelines",
            &json!({ "MaxResults": refused }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled 400, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response:?}"
        );
        assert!(
            response.get("PipelineSummaries").is_none(),
            "case {case:?}: expected a refusal to report no page, got {response:?}"
        );
    }

    // The refusals changed nothing, so the collection still reads back exactly as it did.
    let (after, _) = walk(&emulator, HOME_REGION, None, "MaxResults bounds").await;
    assert_eq!(
        names_of(&after),
        names_of(&everything),
        "case {:?}: expected the refused calls to leave the collection alone, got {after:?}",
        "MaxResults bounds"
    );
    assert_eq!(
        stored_count(&emulator),
        count,
        "case {:?}: expected {count} records after the refusals, got {}",
        "MaxResults bounds",
        stored_count(&emulator)
    );
}

/// A `NextToken` this operation did not issue is refused with the modelled 400, a token
/// it did issue is served, and a well-formed position the store no longer holds is a
/// position rather than a forgery.
#[tokio::test]
async fn a_next_token_this_operation_did_not_issue_is_refused() {
    let emulator = emulator();
    let names = fill(&emulator, HOME_REGION, 3, "home", "forged token").await;
    let (_, issued) = list(
        &emulator,
        HOME_REGION,
        &json!({ "MaxResults": 1 }),
        "forged token",
    )
    .await;
    let issued = issued.expect("a page that stopped short of the store issues a NextToken");

    // The rows differ along the ways a token can fail to carry a value a pipeline name
    // can take. The form is the modelled `TelemetryPipelineName` one:
    // smithy_query(query="shape",shape="TelemetryPipelineSummary") reports length 3..28
    // and pattern [a-z][a-z0-9\-]+ for that member — so each row below breaks one of
    // them, plus a token that is not hex at all and one that is hex of no UTF-8.
    let rows: Vec<(&str, String)> = vec![
        ("not hex at all", "not-a-token".to_string()),
        ("hex that is not UTF-8", "ff".to_string()),
        ("hex of a name one character short", hex::encode("ab")),
        (
            "hex of a name one character past the maximum",
            hex::encode("a".repeat(29)),
        ),
        ("hex of a name in upper case", hex::encode("Pipeline")),
        ("hex of a name starting with a digit", hex::encode("0pipe")),
        (
            "hex of a name carrying a character the pattern excludes",
            hex::encode("pipe_line"),
        ),
        (
            "hex of an ARN rather than the name inside it",
            hex::encode(pipeline_arn(HOME_REGION, "abc")),
        ),
        (
            "the issued token with one hex digit dropped",
            issued[..issued.len() - 1].to_string(),
        ),
    ];
    for (case, token) in rows {
        let (status, response) = call(
            &emulator,
            HOME_REGION,
            "/ListTelemetryPipelines",
            &json!({ "NextToken": token }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for {token:?}, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {response:?}"
        );
    }

    // The token the operation did issue still resumes the walk, at the pipeline after
    // the one the first page ended on.
    let (page, _) = list(
        &emulator,
        HOME_REGION,
        &json!({ "MaxResults": 1, "NextToken": issued }),
        "the issued token",
    )
    .await;
    assert_eq!(
        names_of(&page),
        vec![names[1].clone()],
        "case {:?}: expected the issued token to resume the walk at the second pipeline, got \
         {page:?}",
        "the issued token"
    );

    // And a token of the form this operation mints whose name the store does not hold is
    // a POSITION, not a forgery: the page resumes AT it. This name sorts past every
    // stored one, so nothing is left beyond it and the page is empty and final.
    let case = "a well-formed name the store does not hold";
    let (page, next) = list(
        &emulator,
        HOME_REGION,
        &json!({ "NextToken": hex::encode("zzz-nothing-here") }),
        case,
    )
    .await;
    assert!(
        page.is_empty() && next.is_none(),
        "case {case:?}: expected the page after the last position to be empty and final, got \
         {page:?} with {next:?}"
    );
}

/// The status a summary reports is the one the record's own due instant and the clock
/// decide, and the listing that reported it wrote nothing.
#[tokio::test]
async fn a_summary_reports_the_status_the_clock_owes_and_the_listing_writes_nothing() {
    let case = "activation span";
    let emulator = emulator();
    create(&emulator, HOME_REGION, "activation-listed", 0, case).await;

    let (creating, _) = list(&emulator, HOME_REGION, &json!({}), case).await;
    let (created_ms, active_at_ms) = record_instants(&emulator, HOME_REGION, "activation-listed");
    assert!(
        active_at_ms > created_ms,
        "case {case:?}: the CREATING -> ACTIVE span must be non-zero: created {created_ms}, due \
         {active_at_ms}"
    );
    let first = status_of(&creating[0]);
    assert!(
        TELEMETRY_PIPELINE_STATUS_VALUES.contains(&first.as_str()),
        "case {case:?}: Status must be one of {TELEMETRY_PIPELINE_STATUS_VALUES:?}, got {first:?}"
    );
    if now_epoch_millis() < active_at_ms {
        assert_eq!(
            first, "CREATING",
            "case {case:?}: a listing inside the span reports CREATING, got {creating:?}"
        );
    }

    tokio::time::sleep(past_activation_delay()).await;

    let (active, _) = list(&emulator, HOME_REGION, &json!({}), case).await;
    assert_eq!(
        status_of(&active[0]),
        "ACTIVE",
        "case {case:?}: once the transition comes due the summary reports ACTIVE, got {active:?}"
    );
    // The clock moved the status; the listings moved nothing.
    assert_eq!(
        record_instants(&emulator, HOME_REGION, "activation-listed"),
        (created_ms, active_at_ms),
        "case {case:?}: expected the listings to leave the record's own instants alone"
    );
    assert_eq!(
        active[0].get("CreatedTimeStamp").and_then(Value::as_i64),
        Some(created_ms),
        "case {case:?}: expected the summary to report the create instant, got {active:?}"
    );
    let (status, read) = get(&emulator, HOME_REGION, "activation-listed").await;
    assert_eq!(
        (status, read.pointer("/Pipeline/Status").and_then(Value::as_str)),
        (200, Some("ACTIVE")),
        "case {case:?}: expected the read to report the status the listing did, got {status} \
         {read:?}"
    );
}
