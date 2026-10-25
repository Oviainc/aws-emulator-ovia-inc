// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeDeliveryDestinations`: the account's delivery destinations,
//! enumerated.
//!
//! The rule under test: `DescribeDeliveryDestinations` "Retrieves a list of the delivery
//! destinations that have been created in the account" — every destination the store holds,
//! rendered exactly as the destination's own reader renders it, in destination-name order,
//! narrowed to the destinations whose name the request's `names` lists ("filters the results
//! to delivery destinations with any specified name") and handed back one `limit`-sized page
//! at a time, with a `nextToken` ("The token for the next set of items to return") exactly
//! while destinations remain beyond the page just returned.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so what is
//! asserted here is what a client reads off the wire, and a response that is not computed
//! from the shared store cannot pass.
//!
//! The table spans what the rule generalises over rather than one request: destinations of
//! every vendor the model's `DeliveryDestinationType` enumerates, output formats drawn from
//! the model's own `OutputFormat` enum, a destination configuration present and absent, a role
//! present and absent, tag maps of zero / one / several entries, names at both published
//! length bounds, filters of one / several / no / an unheld name, and every page size from one
//! destination to more than the account holds.

use serde_json::{json, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::ops::types::{
    DELIVERY_DESTINATION_TYPE_VALUES, OUTPUT_FORMAT_VALUES,
};
use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::delivery_destination_arn;

use crate::harness::{Logs, REGION};

/// The operations under test, plus the put that stands the destinations up and the get the
/// enumeration is checked against.
const DESCRIBE: &str = "DescribeDeliveryDestinations";
const PUT: &str = "PutDeliveryDestination";
const GET: &str = "GetDeliveryDestination";

/// The refusal `DescribeDeliveryDestinations` models for a token it never minted.
const VALIDATION: &str = "ValidationException";

/// One destination resource per vendor a delivery destination can represent — "a log group in
/// CloudWatch Logs, an Amazon S3 bucket, or a delivery stream in Firehose", plus the X-Ray
/// trace destination the model's `DeliveryDestinationType` adds, which configures no resource
/// at all.
const LOG_GROUP_ARN: &str = "arn:aws:logs:eu-west-2:123456789012:log-group:/aws/vendedlogs/one:*";
const BUCKET_ARN: &str = "arn:aws:s3:::vended-logs-bucket";
const FIREHOSE_ARN: &str = "arn:aws:firehose:eu-west-2:123456789012:deliverystream/vended-logs";

/// The IAM role a trace delivery destination may be created with ("This parameter is supported
/// only for X-Ray trace delivery destinations").
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/CWLogsTraceDelivery";

/// The `DeliveryDestinationType` of a trace destination, the one kind that needs no
/// `deliveryDestinationConfiguration`. Source: the model's `DeliveryDestinationType` enum
/// (`DELIVERY_DESTINATION_TYPE_VALUES`), asserted to be one of its values below.
const TRACE: &str = "XRAY";

/// One delivery destination to stand up, spanning what the enumeration generalises over.
#[derive(Debug)]
struct Case {
    /// What this row varies, for the assertion messages.
    label: &'static str,
    /// `name` (`DeliveryDestinationName`, 1..60, `^[\w-]*$`) — also the key the enumeration
    /// is ordered by.
    name: String,
    /// `deliveryDestinationConfiguration.destinationResourceArn`, or `None` for the trace
    /// destination, which configures no resource.
    destination_resource_arn: Option<&'static str>,
    /// `deliveryDestinationType` as the REQUEST names it, used only when the put configures
    /// no resource.
    requested_type: Option<&'static str>,
    /// `outputFormat`, absent or one value of the model's `OutputFormat`.
    output_format: Option<&'static str>,
    /// `roleArn`, absent or the role above.
    role_arn: Option<&'static str>,
    /// `tags`, zero / one / several entries.
    tags: Vec<(&'static str, &'static str)>,
}

impl Case {
    fn body(&self) -> Value {
        let mut body = json!({"name": self.name});
        if let Some(arn) = self.destination_resource_arn {
            body["deliveryDestinationConfiguration"] = json!({"destinationResourceArn": arn});
        }
        if let Some(requested_type) = self.requested_type {
            body["deliveryDestinationType"] = json!(requested_type);
        }
        if let Some(output_format) = self.output_format {
            body["outputFormat"] = json!(output_format);
        }
        if let Some(role_arn) = self.role_arn {
            body["roleArn"] = json!(role_arn);
        }
        if !self.tags.is_empty() {
            body["tags"] = Value::Object(
                self.tags
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), json!(value)))
                    .collect(),
            );
        }
        body
    }
}

/// The destinations the suite stands up: one per vendor, plus rows varying output format,
/// role, tag count and name length. Names are deliberately NOT in insertion order, so an
/// enumeration that merely replays insertion order cannot pass.
fn cases() -> Vec<Case> {
    let longest = "d".repeat(60);
    vec![
        Case {
            label: "a firehose destination, no output format, no tags",
            name: "zeta-firehose".to_string(),
            destination_resource_arn: Some(FIREHOSE_ARN),
            requested_type: None,
            output_format: None,
            role_arn: None,
            tags: Vec::new(),
        },
        Case {
            label: "a log group destination, one tag",
            name: "mid-logs".to_string(),
            destination_resource_arn: Some(LOG_GROUP_ARN),
            requested_type: None,
            output_format: Some(OUTPUT_FORMAT_VALUES[0]),
            role_arn: None,
            tags: vec![("team", "observability")],
        },
        Case {
            label: "an s3 destination, several tags",
            name: "alpha-bucket".to_string(),
            destination_resource_arn: Some(BUCKET_ARN),
            requested_type: None,
            output_format: Some(OUTPUT_FORMAT_VALUES[OUTPUT_FORMAT_VALUES.len() - 1]),
            role_arn: None,
            tags: vec![("team", "observability"), ("tier", "gold"), ("env", "prod")],
        },
        Case {
            label: "a trace destination with a role and no configuration",
            name: "trace-dest".to_string(),
            destination_resource_arn: None,
            requested_type: Some(TRACE),
            output_format: None,
            role_arn: Some(ROLE_ARN),
            tags: Vec::new(),
        },
        Case {
            label: "the shortest name the model admits",
            name: "d".to_string(),
            destination_resource_arn: Some(BUCKET_ARN),
            requested_type: None,
            output_format: None,
            role_arn: None,
            tags: Vec::new(),
        },
        Case {
            label: "the longest name the model admits",
            name: longest,
            destination_resource_arn: Some(FIREHOSE_ARN),
            requested_type: None,
            output_format: None,
            role_arn: None,
            tags: vec![("only", "one")],
        },
    ]
}

/// Stand up every case, answering the names it created in the order the enumeration must
/// report them (destination-name order).
async fn stand_up(logs: &Logs, cases: &[Case]) -> Vec<String> {
    for case in cases {
        let response = logs.ok(PUT, &case.body()).await;
        let created = response
            .get("deliveryDestination")
            .and_then(|destination| destination.get("name"))
            .and_then(Value::as_str);
        assert_eq!(
            created,
            Some(case.name.as_str()),
            "case {:?}: {PUT} must create the destination it was asked for ({}), got {created:?}",
            case.label,
            case.name
        );
    }
    let mut names: Vec<String> = cases.iter().map(|case| case.name.clone()).collect();
    names.sort();
    names
}

/// One enumeration to page through: what it is called, the `names` filter that selects it (or
/// `None` for no filter at all) and the destination names it must enumerate.
type Enumeration = (&'static str, Option<Vec<String>>, Vec<String>);

/// One page of the enumeration, as a client reads it: the destinations and the continuation
/// token, if any.
async fn describe(logs: &Logs, body: &Value) -> (Vec<Value>, Option<String>) {
    let response = logs.ok(DESCRIBE, body).await;
    let page = response
        .get("deliveryDestinations")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("{DESCRIBE} {body}: the response must carry deliveryDestinations, got {response}")
        })
        .clone();
    let token = response
        .get("nextToken")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    (page, token)
}

/// The one destination `GetDeliveryDestination` reports under `name` — the rendering the
/// enumeration must agree with.
async fn get(logs: &Logs, name: &str) -> Value {
    let response = logs.ok(GET, &json!({"name": name})).await;
    response
        .get("deliveryDestination")
        .cloned()
        .unwrap_or_else(|| panic!("case {name}: {GET} must report a deliveryDestination, got {response}"))
}

/// The `name` of a reported destination.
fn reported_name(case: &str, destination: &Value) -> String {
    destination
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case}: a reported destination carries a name, got {destination}")
        })
        .to_string()
}

/// An account holding no delivery destination enumerates none — an empty list, not a refusal
/// and not a token.
#[tokio::test]
async fn an_account_with_no_destination_enumerates_none() {
    let logs = Logs::new();
    for (case, body) in [
        ("no filter", json!({})),
        ("a limit", json!({"limit": 1})),
        ("a name no destination is held under", json!({"names": ["absent"]})),
    ] {
        let (page, token) = describe(&logs, &body).await;
        assert!(
            page.is_empty(),
            "case {case}: an account holding no destination enumerates none, got {page:?}"
        );
        assert_eq!(
            token, None,
            "case {case}: an empty enumeration has no next set, got {token:?}"
        );
    }
}

/// Every destination the account holds is enumerated, in destination-name order, rendered
/// exactly as `GetDeliveryDestination` renders it and carrying the ARN this service derives
/// from the request's Region and the destination's name.
#[tokio::test]
async fn describe_reports_every_stored_destination_in_name_order() {
    let logs = Logs::new();
    let cases = cases();
    let names = stand_up(&logs, &cases).await;

    let (page, token) = describe(&logs, &json!({"limit": 50})).await;
    assert_eq!(
        token, None,
        "case no filter: one page holding every destination carries no nextToken, got {token:?}"
    );

    let reported: Vec<String> = page
        .iter()
        .map(|destination| reported_name("no filter", destination))
        .collect();
    assert_eq!(
        reported, names,
        "case no filter: expected {names:?} in name order, got {reported:?}"
    );

    for (case, destination) in names.iter().zip(page.iter()) {
        let read = get(&logs, case).await;
        assert_eq!(
            destination, &read,
            "case {case}: the enumeration must report what {GET} reports ({read}), got {destination}"
        );
        let expected_arn = delivery_destination_arn(REGION, case);
        let arn = destination.get("arn").and_then(Value::as_str);
        assert_eq!(
            arn,
            Some(expected_arn.as_str()),
            "case {case}: expected arn {expected_arn}, got {arn:?}"
        );
    }
}

/// `names` "filters the results to delivery destinations with any specified name": the
/// enumeration is narrowed to the held destinations the request lists, whatever else it lists,
/// and is still in name order.
#[tokio::test]
async fn describe_filters_the_enumeration_by_name() {
    let logs = Logs::new();
    let cases = cases();
    let names = stand_up(&logs, &cases).await;

    let held = |index: usize| names[index].clone();
    // (case, the names the request filters by)
    let filters: Vec<(&str, Vec<String>)> = vec![
        ("one held name", vec![held(0)]),
        ("another one held name", vec![held(3)]),
        ("several held names", vec![held(1), held(4), held(2)]),
        (
            "a held name and one no destination is held under",
            vec![held(2), "no-such-destination".to_string()],
        ),
        (
            "only names no destination is held under",
            vec!["no-such-destination".to_string(), "absent".to_string()],
        ),
        ("every held name", names.clone()),
    ];

    for (case, filter) in filters {
        let mut expected: Vec<String> = names
            .iter()
            .filter(|name| filter.contains(name))
            .cloned()
            .collect();
        expected.sort();

        let (page, token) = describe(&logs, &json!({"names": filter, "limit": 50})).await;
        let reported: Vec<String> = page
            .iter()
            .map(|destination| reported_name(case, destination))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
        assert_eq!(
            token, None,
            "case {case:?}: one page holding every selected destination carries no nextToken, got {token:?}"
        );
    }
}

/// The enumeration is paged: a page holds at most `limit` destinations, a `nextToken` comes
/// back exactly while destinations remain, and the pages concatenated are the whole
/// enumeration whatever the page size — under a filter as much as without one.
#[tokio::test]
async fn describe_pages_the_enumeration_whatever_the_page_size() {
    let logs = Logs::new();
    let cases = cases();
    let names = stand_up(&logs, &cases).await;

    // (case, the `names` filter, the names it selects)
    let filtered: Vec<String> = names.iter().take(3).cloned().collect();
    let enumerations: Vec<Enumeration> = vec![
        ("no filter", None, names.clone()),
        (
            "a filter of three names",
            Some(filtered.clone()),
            filtered.clone(),
        ),
    ];

    for (enumeration, filter, expected) in enumerations {
        for limit in [None, Some(1), Some(2), Some(3), Some(names.len()), Some(50)] {
            let case = format!("{enumeration} at limit {limit:?}");
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            loop {
                let mut body = json!({});
                if let Some(filter) = &filter {
                    body["names"] = json!(filter);
                }
                if let Some(limit) = limit {
                    body["limit"] = json!(limit);
                }
                if let Some(token) = &token {
                    body["nextToken"] = json!(token);
                }
                let (page, next) = describe(&logs, &body).await;
                if let Some(limit) = limit {
                    assert!(
                        page.len() <= limit,
                        "case {case}: a page holds at most {limit} destinations, got {}",
                        page.len()
                    );
                }
                for destination in &page {
                    seen.push(reported_name(&case, destination));
                }
                match next {
                    Some(next) => token = Some(next),
                    None => break,
                }
            }
            assert_eq!(
                seen, expected,
                "case {case}: expected the pages to enumerate {expected:?}, got {seen:?}"
            );
        }
    }
}

/// A `nextToken` is a position within ONE filtered enumeration, so continuing it under another
/// filter is refused rather than silently answered from a different list — and so is a value
/// this service never minted.
#[tokio::test]
async fn a_token_is_a_position_within_one_enumeration() {
    let logs = Logs::new();
    let cases = cases();
    let names = stand_up(&logs, &cases).await;

    let (_, token) = describe(&logs, &json!({"limit": 1})).await;
    let token = token.expect("a first page of one destination out of several carries a nextToken");

    // (case, the body that continues the token under a filter it was not issued for)
    let continuations: Vec<(&str, Value)> = vec![
        (
            "a token continued under a name filter",
            json!({"limit": 1, "nextToken": token, "names": [names[0]]}),
        ),
        (
            "a token this service never minted",
            json!({"limit": 1, "nextToken": "not-a-token"}),
        ),
    ];
    for (case, body) in continuations {
        let code = logs.err(DESCRIBE, &body).await;
        assert_eq!(
            code, VALIDATION,
            "case {case}: expected {VALIDATION}, got {code}"
        );
    }

    // A token minted under a filter resumes THAT filter's enumeration.
    let filter = vec![names[0].clone(), names[1].clone()];
    let (first, token) = describe(&logs, &json!({"names": filter, "limit": 1})).await;
    let token = token.expect("a first page of one of two filtered destinations carries a nextToken");
    let (second, next) = describe(
        &logs,
        &json!({"names": filter, "limit": 1, "nextToken": token}),
    )
    .await;
    let resumed: Vec<String> = first
        .iter()
        .chain(second.iter())
        .map(|destination| reported_name("a filtered enumeration resumed", destination))
        .collect();
    assert_eq!(
        resumed, filter,
        "case a filtered enumeration resumed: expected {filter:?}, got {resumed:?}"
    );
    assert_eq!(
        next, None,
        "case a filtered enumeration resumed: the last page carries no nextToken, got {next:?}"
    );
}

/// The trace destination's type is one the model publishes, so the row above that leans on it
/// is pinned to the model's own catalogue rather than to a string typed in this suite.
#[tokio::test]
async fn the_trace_destination_type_is_one_the_model_publishes() {
    assert!(
        DELIVERY_DESTINATION_TYPE_VALUES.contains(&TRACE),
        "case {TRACE}: expected one of the model's DeliveryDestinationType values \
         {DELIVERY_DESTINATION_TYPE_VALUES:?}, got {TRACE}"
    );
    let logs = Logs::new();
    let reported = logs
        .ok(
            PUT,
            &json!({"name": "trace-only", "deliveryDestinationType": TRACE}),
        )
        .await;
    let (page, _) = describe(&logs, &json!({"names": ["trace-only"]})).await;
    let expected = reported
        .get("deliveryDestination")
        .cloned()
        .unwrap_or_else(|| panic!("case {TRACE}: {PUT} must report a deliveryDestination"));
    assert_eq!(
        page,
        vec![expected.clone()],
        "case {TRACE}: expected the enumeration to report {expected}, got {page:?}"
    );
}
