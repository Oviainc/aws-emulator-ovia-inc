// SPDX-License-Identifier: CC0-1.0

//! `DescribeRuntimeVersions`, end to end over the wire (the same `MultiServiceRouter` the
//! emulator boots, through the harness in `canary_lifecycle`).
//!
//! The rule pinned here: `DescribeRuntimeVersions` reports the service's published catalog
//! of canary runtime versions — one modelled `RuntimeVersion` per catalogued version, in a
//! total and stable order, paged by `MaxResults`/`NextToken` so that walking the pages
//! yields exactly the catalog once, with a `NextToken` emitted only while entries remain —
//! and it reports it without changing anything a later caller sees.
//!
//! The assertions below therefore span what the rule generalises over rather than one
//! recorded call: every catalogued entry (not a cited one), a table of page sizes from 1 to
//! past the catalog's length, both token forms this operation issues, the tokens it must
//! refuse, and the `MaxResults` boundary the model declares.
//!
//! Every modelled constraint quoted here is the model's own, read with
//! `smithy_query(query="operation",operation="DescribeRuntimeVersions",model_path="services/service-com-amazonaws-synthetics-synthetics/smithy-model/com-amazonaws-synthetics-synthetics.json")`
//! and `smithy_query(query="shape",shape="RuntimeVersion")`.

use std::collections::BTreeSet;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};

use super::canary_lifecycle::{create_body, Answer, Synthetics};

/// `MaxSize100`'s modelled range (`DescribeRuntimeVersions$MaxResults`, range 1..100),
/// which is also the documented default ("If you omit this parameter, the default of 100 is
/// used").
const MAX_RESULTS_MIN: i64 = 1;
const MAX_RESULTS_MAX: i64 = 100;

/// `Token`'s modelled length (`DescribeRuntimeVersions$NextToken`, len 4..252).
const TOKEN_MIN_LEN: usize = 4;

/// Runtime versions the CloudWatch User Guide's canary runtime versions page publishes —
/// the page the model's own `RuntimeVersion` doc links to
/// (<https://docs.aws.amazon.com/AmazonCloudWatch/latest/monitoring/CloudWatch_Synthetics_Canaries_Library.html>).
///
/// One per runtime FAMILY the service offers, so the catalog is checked across the Node.js
/// Puppeteer, Node.js Playwright and Python Selenium runtimes rather than at the single
/// Puppeteer version the issue's recordings happen to create canaries on.
const PUBLISHED_VERSIONS: &[&str] = &[
    "syn-nodejs-puppeteer-9.0",
    "syn-nodejs-playwright-1.0",
    "syn-python-selenium-5.1",
];

/// Every runtime version name the service reports, in the order it reports them, paging with
/// `max_results` when given.
async fn walk_catalog(aws: &Synthetics, max_results: Option<i64>) -> Vec<String> {
    let mut names = Vec::new();
    let mut token: Option<String> = None;
    // One more iteration than the catalog can possibly need, so a non-terminating paging
    // loop fails the test instead of hanging it.
    for _ in 0..1_000 {
        let answer = describe(aws, max_results, token.as_deref()).await;
        assert_eq!(
            answer.status, 200,
            "case max_results={max_results:?}: expected HTTP 200, got {} with body {:?}",
            answer.status, answer.body
        );
        let page = runtime_versions(&answer);
        for entry in page {
            names.push(version_name(entry));
        }
        token = answer
            .at("NextToken")
            .as_str()
            .map(std::string::ToString::to_string);
        if token.is_none() {
            return names;
        }
    }
    panic!("case max_results={max_results:?}: paging did not terminate after 1000 pages");
}

/// One `DescribeRuntimeVersions` call: `POST /runtime-versions`, as the model binds it.
async fn describe(aws: &Synthetics, max_results: Option<i64>, next_token: Option<&str>) -> Answer {
    let mut body = json!({});
    if let Some(max) = max_results {
        body["MaxResults"] = json!(max);
    }
    if let Some(token) = next_token {
        body["NextToken"] = json!(token);
    }
    aws.call("POST", "/runtime-versions", Some(body)).await
}

/// The `RuntimeVersions` list of a successful answer.
fn runtime_versions(answer: &Answer) -> &Vec<Value> {
    answer
        .at("RuntimeVersions")
        .as_array()
        .unwrap_or_else(|| panic!("expected a RuntimeVersions array, got {:?}", answer.body))
}

fn version_name(entry: &Value) -> String {
    entry["VersionName"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a VersionName string, got {entry:?}"))
        .to_string()
}

/// A modelled `Timestamp` as the `restJson1` protocol renders it (epoch seconds, possibly
/// fractional) or as RFC 3339 text, whichever the serializer chose.
fn timestamp_seconds(value: &Value, case: &str) -> f64 {
    match value {
        Value::Number(seconds) => seconds
            .as_f64()
            .unwrap_or_else(|| panic!("case {case}: timestamp {value:?} is not a number")),
        Value::String(text) => DateTime::from_str(text, Format::DateTime)
            .unwrap_or_else(|error| panic!("case {case}: timestamp {text:?} is not RFC 3339: {error}"))
            .as_secs_f64(),
        other => panic!("case {case}: expected a timestamp, got {other:?}"),
    }
}

#[tokio::test]
async fn every_reported_runtime_version_carries_the_modelled_shape() {
    let aws = Synthetics::new("us-west-2");

    let answer = describe(&aws, None, None).await;
    assert_eq!(
        answer.status, 200,
        "case default page: expected HTTP 200, got {} with body {:?}",
        answer.status, answer.body
    );
    let entries = runtime_versions(&answer).clone();
    assert!(
        !entries.is_empty(),
        "case default page: expected the published runtime catalog, got an empty list"
    );

    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut previous_release: Option<f64> = None;
    for entry in &entries {
        let name = version_name(entry);
        let case = name.clone();
        // `syn-` is how every published Synthetics runtime version is named (the runtime
        // versions page above).
        assert!(
            name.starts_with("syn-"),
            "case {case}: a published runtime version name starts with \"syn-\", got {name:?}"
        );
        assert!(
            seen.insert(name.clone()),
            "case {case}: the catalog reported the same version twice"
        );

        // `RuntimeVersion$Description`: "A description of the runtime version, created by
        // Amazon" — modelled `len 1..1024`.
        let description = entry["Description"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a Description string, got {entry:?}"));
        assert!(
            (1..=1024).contains(&description.len()),
            "case {case}: Description must satisfy len 1..1024, got {} characters",
            description.len()
        );

        // `RuntimeVersion$ReleaseDate`: "The date that the runtime version was released" —
        // so a released runtime carries one, and the catalog's order is that ladder.
        let release = timestamp_seconds(&entry["ReleaseDate"], &case);
        if let Some(earlier) = previous_release {
            assert!(
                release >= earlier,
                "case {case}: the catalog is ordered by release date, got {release} after {earlier}"
            );
        }
        previous_release = Some(release);

        // `RuntimeVersion$DeprecationDate`: "If this runtime version is deprecated, this
        // value is the date of deprecation" — the service offers only versions a canary can
        // still be created on, so no reported version carries one.
        assert!(
            entry["DeprecationDate"].is_null(),
            "case {case}: an offered runtime version has no DeprecationDate, got {:?}",
            entry["DeprecationDate"]
        );
    }

    for published in PUBLISHED_VERSIONS {
        assert!(
            seen.contains(*published),
            "case {published}: a published runtime version must be reported, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn paging_spans_the_catalog_exactly_once_for_every_page_size() {
    let aws = Synthetics::new("eu-central-1");

    let whole = walk_catalog(&aws, None).await;
    assert!(
        whole.len() > 1,
        "the catalog needs more than one entry for paging to mean anything, got {whole:?}"
    );

    // Page sizes spanning the class: the modelled minimum, a size that divides nothing
    // evenly, one either side of the catalog's own length, and the modelled maximum (which
    // is also the default).
    let sizes: Vec<i64> = vec![
        MAX_RESULTS_MIN,
        2,
        5,
        i64::try_from(whole.len()).expect("the catalog length fits an i64") - 1,
        i64::try_from(whole.len()).expect("the catalog length fits an i64"),
        MAX_RESULTS_MAX,
    ];
    for size in sizes {
        let paged = walk_catalog(&aws, Some(size)).await;
        assert_eq!(
            paged, whole,
            "case max_results={size}: paging must yield the catalog once, in order, got {paged:?}"
        );

        // A page is as long as the page size allows, and a NextToken appears only while
        // entries remain.
        let mut remaining = whole.len();
        let mut token: Option<String> = None;
        loop {
            let answer = describe(&aws, Some(size), token.as_deref()).await;
            let page = runtime_versions(&answer).len();
            let expected = usize::try_from(size).expect("a page size fits a usize").min(remaining);
            assert_eq!(
                page, expected,
                "case max_results={size}: expected a page of {expected}, got {page}"
            );
            remaining -= page;
            match answer.at("NextToken").as_str() {
                Some(next) => {
                    assert!(
                        remaining > 0,
                        "case max_results={size}: NextToken {next:?} was issued with nothing left"
                    );
                    assert!(
                        (TOKEN_MIN_LEN..=252).contains(&next.len()),
                        "case max_results={size}: NextToken must satisfy len 4..252, got {} characters",
                        next.len()
                    );
                    token = Some(next.to_string());
                }
                None => {
                    assert_eq!(
                        remaining, 0,
                        "case max_results={size}: the last page must carry no NextToken, {remaining} left"
                    );
                    break;
                }
            }
        }
    }
}

#[tokio::test]
async fn a_token_this_operation_did_not_issue_is_refused() {
    let aws = Synthetics::new("ap-southeast-2");

    // A token the operation DID issue resumes where it left off; the rows after it are
    // tokens it never issued, which are bad requests rather than empty pages.
    let first = describe(&aws, Some(1), None).await;
    let issued = first
        .at("NextToken")
        .as_str()
        .expect("the first of several pages carries a NextToken")
        .to_string();
    let resumed = describe(&aws, Some(1), Some(&issued)).await;
    assert_eq!(
        resumed.status, 200,
        "case issued token: expected HTTP 200, got {} with body {:?}",
        resumed.status, resumed.body
    );
    let whole = walk_catalog(&aws, None).await;
    assert_eq!(
        version_name(&runtime_versions(&resumed)[0]),
        whole[1],
        "case issued token: a token resumes at the entry after the page that issued it"
    );

    // The offset form this operation falls back to for a key too long to carry resumes at
    // that position when the position is one the catalog holds.
    let by_offset = describe(&aws, Some(1), Some("ix#4")).await;
    assert_eq!(
        by_offset.status, 200,
        "case ix#4: expected HTTP 200, got {} with body {:?}",
        by_offset.status, by_offset.body
    );
    assert_eq!(
        version_name(&runtime_versions(&by_offset)[0]),
        whole[4],
        "case ix#4: an offset token resumes at that position in the catalog"
    );

    let refused = [
        // Not one of the token forms this operation issues.
        "not-a-token",
        // The version form, naming a runtime version the service does not publish.
        "rv#syn-nodejs-puppeteer-0.0",
        // The offset form, with a payload that is not an offset.
        "ix#not-a-number",
        // The offset form, past the end of the catalog: a token this operation could not
        // have issued, so a bad request rather than an empty page.
        "ix#99999",
        // Legal `Token` length, nothing else.
        "    ",
    ];
    for token in refused {
        let answer = describe(&aws, None, Some(token)).await;
        assert_eq!(
            answer.status, 400,
            "case {token:?}: expected HTTP 400, got {} with body {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ValidationException"),
            "case {token:?}: expected ValidationException, got {:?}",
            answer.error_code
        );
    }

    // And the model's own `Token` length constraint is refused the same way, on each side of
    // the published bound.
    for token in ["abc", &"t".repeat(253)] {
        let answer = describe(&aws, None, Some(token)).await;
        assert_eq!(
            answer.status, 400,
            "case NextToken of {} characters: expected HTTP 400, got {} with body {:?}",
            token.len(),
            answer.status,
            answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ValidationException"),
            "case NextToken of {} characters: expected ValidationException, got {:?}",
            token.len(),
            answer.error_code
        );
    }
}

#[tokio::test]
async fn max_results_is_accepted_on_its_bounds_and_refused_outside_them() {
    let aws = Synthetics::new("us-east-2");

    for size in [MAX_RESULTS_MIN, MAX_RESULTS_MAX] {
        let answer = describe(&aws, Some(size), None).await;
        assert_eq!(
            answer.status, 200,
            "case max_results={size}: a value on the modelled bound is accepted, got {} with body {:?}",
            answer.status, answer.body
        );
    }
    for size in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
        let answer = describe(&aws, Some(size), None).await;
        assert_eq!(
            answer.status, 400,
            "case max_results={size}: a value outside range 1..100 is refused, got {} with body {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code.as_deref(),
            Some("ValidationException"),
            "case max_results={size}: expected ValidationException, got {:?}",
            answer.error_code
        );
    }
}

#[tokio::test]
async fn reporting_the_catalog_changes_nothing_a_later_caller_sees() {
    let aws = Synthetics::new("eu-west-1");

    let before = describe(&aws, None, None).await;
    let after = describe(&aws, None, None).await;
    assert_eq!(
        before.body, after.body,
        "a read must not change what a later caller sees: {:?} then {:?}",
        before.body, after.body
    );

    // Nor does it disturb the canaries in the store: a canary created on a catalogued
    // runtime version is still there, on that version, after the catalog is read.
    let names = walk_catalog(&aws, None).await;
    for (index, runtime) in [names[0].clone(), names[names.len() / 2].clone(), names[names.len() - 1].clone()]
        .into_iter()
        .enumerate()
    {
        let canary = format!("canary-{index}");
        let mut body = create_body(&canary);
        body["RuntimeVersion"] = json!(runtime);
        let created = aws.create(body).await;
        assert_eq!(
            created.status, 200,
            "case {runtime}: expected HTTP 200 from CreateCanary, got {} with body {:?}",
            created.status, created.body
        );

        let catalog = describe(&aws, None, None).await;
        assert_eq!(
            catalog.body, before.body,
            "case {runtime}: the catalog a caller sees does not depend on the canaries in the store"
        );

        let read_back = aws.get(&canary).await;
        assert_eq!(
            read_back.string_at("Canary.RuntimeVersion"),
            runtime,
            "case {runtime}: GetCanary must report the runtime version the canary was created on"
        );
    }
}
