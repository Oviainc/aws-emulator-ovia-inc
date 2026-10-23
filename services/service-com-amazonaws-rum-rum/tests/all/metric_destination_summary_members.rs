// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the MEMBERS of a reported `MetricDestinationSummary`.
//!
//! The rule: `ListRumMetricsDestinations` reports every modelled member of
//! `MetricDestinationSummary` — `Destination`, `DestinationArn` and `IamRoleArn` — for
//! every destination it reports, and a member the destination kind carries no value for
//! is reported as the EMPTY STRING rather than omitted. (`DestinationArn` is modelled
//! `@length(min: 0, max: 2048)` and both ARN members are documented as describing the
//! `Evidently` case only, so a `CloudWatch` destination has a value for neither;
//! recorded behaviour reports them as defined empty values.)
//! Source: `smithy_query(query="shape", shape="MetricDestinationSummary")`.
//!
//! The cases pin that rule over a SET of inputs rather than one request: both
//! `MetricDestination` values, zero / one / several destinations on one monitor, both
//! kinds in one listing, a destination re-put under another role, and the same listing
//! read one entry per page — because the member set must not depend on the kind, the
//! count, the order or the page boundary.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use super::support::{call, create, emulator, REGION};

/// Every value `MetricDestination` admits.
/// Source: `smithy_query(query="shape", shape="MetricDestination")`.
const METRIC_DESTINATIONS: [&str; 2] = ["CloudWatch", "Evidently"];

/// Every member `MetricDestinationSummary` declares, which is exactly the member set a
/// reported summary must carry.
/// Source: `smithy_query(query="shape", shape="MetricDestinationSummary")`.
const SUMMARY_MEMBERS: [&str; 3] = ["Destination", "DestinationArn", "IamRoleArn"];

/// The ARN of an emulated Evidently experiment, the resource an `Evidently` destination
/// names. Built with the shared `engine::emulator_arn` helper, so it is a canonical ARN
/// rather than a hand-spelled string.
fn experiment_arn(experiment: &str) -> String {
    engine::emulator_arn("evidently", REGION, &format!("experiment/{experiment}"))
}

/// The ARN of an emulated IAM role, the role RUM assumes to write to an experiment. IAM
/// is global, so the region segment is empty.
fn role_arn(role: &str) -> String {
    engine::emulator_arn("iam", "", &format!("role/{role}"))
}

/// One extended-metrics destination, as a case states it on the put.
#[derive(Debug, Clone)]
struct Destination {
    destination: &'static str,
    destination_arn: Option<String>,
    iam_role_arn: Option<String>,
}

impl Destination {
    /// The `CloudWatch` destination: the model forbids both ARNs on the put, so the case
    /// names neither.
    fn cloud_watch() -> Self {
        Self {
            destination: METRIC_DESTINATIONS[0],
            destination_arn: None,
            iam_role_arn: None,
        }
    }

    /// An `Evidently` destination: the experiment that receives the metrics plus the role
    /// RUM writes to it through, both of which the model requires on the put.
    fn evidently(experiment: &str, role: &str) -> Self {
        Self {
            destination: METRIC_DESTINATIONS[1],
            destination_arn: Some(experiment_arn(experiment)),
            iam_role_arn: Some(role_arn(role)),
        }
    }

    /// The same destination written through another role — the one member a re-put can
    /// change without naming a different destination.
    fn with_role(&self, role: &str) -> Self {
        Self {
            iam_role_arn: Some(role_arn(role)),
            ..self.clone()
        }
    }

    /// This destination as a `PutRumMetricsDestination` body: a member the destination
    /// does not have is left out of the request entirely.
    fn put_body(&self) -> Value {
        let mut body = json!({"Destination": self.destination});
        if let Some(arn) = &self.destination_arn {
            body["DestinationArn"] = json!(arn);
        }
        if let Some(arn) = &self.iam_role_arn {
            body["IamRoleArn"] = json!(arn);
        }
        body
    }

    /// The summary a read must report: all three members, with an absent ARN as the
    /// empty string.
    fn expected_summary(&self) -> Value {
        json!({
            "Destination": self.destination,
            "DestinationArn": self.destination_arn.clone().unwrap_or_default(),
            "IamRoleArn": self.iam_role_arn.clone().unwrap_or_default(),
        })
    }
}

/// `PutRumMetricsDestination`, asserting it stored the destination.
async fn put_destination(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    destination: &Destination,
) {
    let reply = call(
        router,
        "POST",
        &format!("/rummetrics/{monitor}/metricsdestination"),
        Some(destination.put_body()),
    )
    .await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected PutRumMetricsDestination({:?}) to answer 200, got {} {:?}",
        destination.put_body(),
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
}

/// One page of `ListRumMetricsDestinations`: the summaries it reported and its token.
async fn list_page(
    router: &MultiServiceRouter,
    case: &str,
    monitor: &str,
    query: &str,
) -> (Vec<Value>, Option<String>) {
    let reply = call(
        router,
        "GET",
        &format!("/rummetrics/{monitor}/metricsdestination{query}"),
        None,
    )
    .await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected ListRumMetricsDestinations to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let body = reply.json(case);
    let destinations = body["Destinations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Destinations list, got {body:?}"))
        .clone();
    let next_token = body["NextToken"].as_str().map(str::to_string);
    (destinations, next_token)
}

/// Assert the member-set rule on ONE reported summary, and hand it back as a comparable
/// string so a case can compare whole sets.
fn assert_members(case: &str, reported: &Value) -> String {
    let object = reported
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected a summary object, got {reported:?}"));
    let members: BTreeSet<&str> = object.keys().map(String::as_str).collect();
    let expected: BTreeSet<&str> = SUMMARY_MEMBERS.iter().copied().collect();
    assert_eq!(
        members, expected,
        "case {case:?}: expected the summary to carry exactly {expected:?}, got {members:?} \
         in {reported:?}"
    );
    for member in SUMMARY_MEMBERS {
        assert!(
            object[member].is_string(),
            "case {case:?}: expected {member} to be a string, got {:?}",
            object[member]
        );
    }
    reported.to_string()
}

#[tokio::test]
async fn every_reported_destination_carries_all_three_summary_members() {
    let evidently = Destination::evidently("alpha", "alpha-writer");
    let cases: Vec<(&str, Vec<Destination>)> = vec![
        ("no destination", vec![]),
        ("one CloudWatch destination", vec![Destination::cloud_watch()]),
        ("one Evidently destination", vec![evidently.clone()]),
        (
            "both destination kinds, CloudWatch put first",
            vec![Destination::cloud_watch(), evidently.clone()],
        ),
        (
            "both destination kinds, Evidently put first",
            vec![evidently.clone(), Destination::cloud_watch()],
        ),
        (
            "several Evidently experiments beside CloudWatch",
            vec![
                Destination::evidently("beta", "beta-writer"),
                Destination::evidently("gamma", "gamma-writer"),
                Destination::cloud_watch(),
            ],
        ),
        (
            "a destination re-put under another role",
            vec![
                evidently.clone(),
                evidently.with_role("alpha-second-writer"),
            ],
        ),
        (
            "CloudWatch re-put unchanged",
            vec![Destination::cloud_watch(), Destination::cloud_watch()],
        ),
    ];

    for (index, (case, puts)) in cases.iter().enumerate() {
        let (router, _engine) = emulator();
        let monitor = format!("summary-members-{index}");
        create(&router, case, json!({"Name": monitor, "Domain": "example.com"})).await;
        for destination in puts {
            put_destination(&router, case, &monitor, destination).await;
        }

        // A later put under the SAME identity (kind + experiment ARN) updates that one
        // destination, so the expected set is keyed on that identity, last put winning.
        let mut expected: Vec<Value> = Vec::new();
        for destination in puts {
            let identity = (
                destination.destination,
                destination.destination_arn.clone().unwrap_or_default(),
            );
            expected.retain(|summary| {
                (
                    summary["Destination"].as_str().unwrap_or_default(),
                    summary["DestinationArn"].as_str().unwrap_or_default().to_string(),
                ) != identity
            });
            expected.push(destination.expected_summary());
        }
        let expected: BTreeSet<String> =
            expected.iter().map(|summary| summary.to_string()).collect();

        let (reported, next_token) = list_page(&router, case, &monitor, "").await;
        assert_eq!(
            next_token, None,
            "case {case:?}: a listing that fits in one page carries no NextToken, got \
             {next_token:?}"
        );
        let reported: BTreeSet<String> = reported
            .iter()
            .map(|summary| assert_members(case, summary))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the listing to report {expected:?}, got {reported:?}"
        );

        // The same rule on every page boundary: one entry per page must report the same
        // full member set as a single-page listing does.
        let mut paged: BTreeSet<String> = BTreeSet::new();
        let mut query = "?maxResults=1".to_string();
        loop {
            let (page, token) = list_page(&router, case, &monitor, &query).await;
            assert!(
                page.len() <= 1,
                "case {case:?}: expected at most one destination per page, got {}",
                page.len()
            );
            for summary in &page {
                paged.insert(assert_members(case, summary));
            }
            match token {
                Some(token) => {
                    query = format!(
                        "?maxResults=1&nextToken={}",
                        super::support::percent_encoded(&token)
                    );
                }
                None => break,
            }
        }
        assert_eq!(
            paged, expected,
            "case {case:?}: expected paging one entry at a time to report {expected:?}, got \
             {paged:?}"
        );
    }
}
