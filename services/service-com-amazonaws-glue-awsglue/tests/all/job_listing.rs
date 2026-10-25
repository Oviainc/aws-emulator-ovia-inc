// SPDX-License-Identifier: CC0-1.0

//! `ListJobs`: the names of the job definitions this account holds.
//!
//! RULE: `ListJobs` "retrieves the names of all job resources in this Amazon Web Services
//! account, or the resources with the specified tag" — so it reports the name of every job
//! the store holds, from the same records `CreateJob` wrote and `GetJob` reads back, one
//! `MaxResults`-sized page at a time with a `NextToken` that resumes the walk, and nothing
//! but the jobs carrying every tag a `Tags` filter names.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: how many jobs are stored, the
//! page size asked for (including the published bounds 1 and 1000 and a size past the
//! stored count), and which tags the filter names (none, one, a pair, a value that matches
//! nothing).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, ok};

/// A role ARN `CreateJob` accepts.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/glue-listing-role";

/// Create one job, with the tags the row asks for.
async fn given_job(router: &MultiServiceRouter, name: &str, tags: Value) {
    let mut body = json!({
        "Name": name,
        "Role": ROLE_ARN,
        "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
    });
    if !tags.is_null() {
        body["Tags"] = tags;
    }
    ok(router, "CreateJob", &body).await;
}

/// The `JobNames` of one answer, in the order the service returned them.
fn job_names(page: &Value) -> Vec<String> {
    page["JobNames"]
        .as_array()
        .expect("JobNames is a list")
        .iter()
        .map(|name| name.as_str().expect("a job name is a string").to_string())
        .collect()
}

#[tokio::test]
async fn every_stored_job_is_listed_and_paged() {
    // (case, how many jobs are stored, the page size asked for)
    let rows: &[(&str, usize, Option<i32>)] = &[
        ("no jobs at all", 0, None),
        ("one job, no page size", 1, None),
        // The published lower bound of `MaxResults` (`PageSize` range 1..1000).
        ("the smallest page the model accepts", 5, Some(1)),
        ("a page smaller than the stored count", 7, Some(3)),
        ("a page larger than the stored count", 3, Some(50)),
        // The published upper bound.
        ("the largest page the model accepts", 4, Some(1_000)),
    ];

    for (case, stored, page_size) in rows {
        let router = emulator();
        // Created out of name order, so the listing's own order is what is asserted.
        let expected: Vec<String> = (0..*stored).map(|index| format!("job-{index:02}")).collect();
        for name in expected.iter().rev() {
            given_job(&router, name, Value::Null).await;
        }

        // Walk every page the service hands out, following its own token.
        let mut listed: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({});
            if let Some(size) = page_size {
                request["MaxResults"] = json!(size);
            }
            if let Some(cursor) = &token {
                request["NextToken"] = json!(cursor);
            }
            let page = ok(&router, "ListJobs", &request).await;
            let names = job_names(&page);
            if let Some(size) = page_size {
                assert!(
                    names.len() <= usize::try_from(*size).unwrap_or(usize::MAX),
                    "case {case:?}: a page must not exceed MaxResults {size}, got {names:?}"
                );
            }
            listed.extend(names);
            token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                listed.len() <= *stored,
                "case {case:?}: the walk must terminate, got {listed:?} for {stored} jobs"
            );
        }

        assert_eq!(
            listed, expected,
            "case {case:?}: expected every stored job name in name order, got {listed:?}"
        );
        // A name the listing reports is a job `GetJob` resolves: one record, two reads.
        for name in &listed {
            let read = ok(&router, "GetJob", &json!({ "JobName": name })).await;
            assert_eq!(
                read["Job"]["Name"].as_str(),
                Some(name.as_str()),
                "case {case:?}: GetJob({name}) must report the job the listing named, got \
                 {read}"
            );
        }
    }
}

#[tokio::test]
async fn a_tag_filter_narrows_the_listing_to_the_jobs_carrying_every_pair() {
    let router = emulator();
    given_job(&router, "tagged-both", json!({"team": "data", "env": "prod"})).await;
    given_job(&router, "tagged-team", json!({"team": "data"})).await;
    given_job(&router, "tagged-other", json!({"team": "ops"})).await;
    given_job(&router, "untagged", Value::Null).await;

    // (case, the Tags filter, the names it must report)
    let rows: &[(&str, Value, &[&str])] = &[
        (
            "no filter: every job",
            Value::Null,
            &["tagged-both", "tagged-other", "tagged-team", "untagged"],
        ),
        (
            "an empty filter narrows nothing",
            json!({}),
            &["tagged-both", "tagged-other", "tagged-team", "untagged"],
        ),
        (
            "one pair",
            json!({"team": "data"}),
            &["tagged-both", "tagged-team"],
        ),
        (
            "a pair that must BOTH match",
            json!({"team": "data", "env": "prod"}),
            &["tagged-both"],
        ),
        (
            "a value nothing carries",
            json!({"team": "finance"}),
            &[],
        ),
        (
            "a key nothing carries",
            json!({"owner": "data"}),
            &[],
        ),
    ];

    for (case, filter, expected) in rows {
        let mut request = json!({});
        if !filter.is_null() {
            request["Tags"] = filter.clone();
        }
        let page = ok(&router, "ListJobs", &request).await;
        assert_eq!(
            job_names(&page),
            expected.iter().map(|name| (*name).to_string()).collect::<Vec<_>>(),
            "case {case:?}: expected the jobs carrying every pair of {filter}, got {page}"
        );
    }
}

#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let router = emulator();
    given_job(&router, "bounded-job", Value::Null).await;

    // `MaxResults` is `PageSize`, range 1..1000: one past each published bound is refused
    // by the modelled constraint rather than served as some other page size.
    for (case, max_results) in [("below the minimum", 0), ("above the maximum", 1_001)] {
        let (status, body) = call(&router, "ListJobs", &json!({"MaxResults": max_results})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: MaxResults={max_results} must be refused, got {status} {body}"
        );
    }
}
