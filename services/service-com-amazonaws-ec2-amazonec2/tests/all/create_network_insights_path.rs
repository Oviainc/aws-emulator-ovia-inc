// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateNetworkInsightsPath`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=CreateNetworkInsightsPath`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rules under test are:
//!
//! 1. the create STORES one Reachability Analyzer path and answers it as stored: a minted
//!    long-form `nip-` id, an ARN naming the Region the request was signed for and that id, a
//!    creation date, and every member the caller chose — the source and destination, the
//!    addresses, the protocol, the destination port, the filters scoping it at either end and
//!    the tags — reported back exactly as given, and reported identically by a later
//!    `DescribeNetworkInsightsPaths`, which is also how `StartNetworkInsightsAnalysis`
//!    resolves it;
//! 2. each create mints a path of its OWN, so two creates leave the account holding two
//!    paths;
//! 3. a filter at one end of the path and the individual members describing that same end
//!    cannot be combined: `FilterAtSource` with `SourceIp` or with `DestinationPort`, and
//!    `FilterAtDestination` with `DestinationIp`, are refused with EC2's
//!    `InvalidParameterCombination` naming both parameters — while every other pairing is an
//!    ordinary request;
//! 4. the constraints the model publishes are refused before anything is stored: a protocol
//!    outside the `Protocol` catalog, a port outside `Port`'s published range, an address
//!    outside `IpAddress`'s pattern and length, and a missing `Source`; each published port
//!    bound itself is accepted. An end of the path naming a resource of a kind no path can be
//!    analyzed between, or naming no resource at all, is refused with EC2's
//!    `InvalidParameterValue` naming the value, the member and the catalog that member accepts;
//! 5. a tag specification may name only the resource type this action tags, and the tags it
//!    carries are what the stored path reports;
//! 6. `DryRun` reports `DryRunOperation` and stores nothing — unless the request would have
//!    been refused anyway, which is the answer it gets instead;
//! 7. a `ClientToken` longer than this crate's bound is refused, and one at the bound is
//!    accepted.
//!
//! Each table row is a distinct input along what the rules generalise over — what a path
//! names (a resource pair, an interface pair, a source alone, a cross-account ARN), which
//! protocol, whether a destination port is set and at which published bound, whether a filter
//! scopes the path at the source, at the destination, at both or at neither, whether a port
//! range carries both of its bounds or one, and how many tags it carries — so no one recorded
//! call is the specification.

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, DRY_RUN_OPERATION, INVALID_PARAMETER_COMBINATION,
    INVALID_PARAMETER_VALUE, MAX_CLIENT_TOKEN_CHARS, MISSING_PARAMETER,
    NETWORK_INSIGHTS_PATH_ID_PREFIX, RESOURCE_TYPE_NETWORK_INSIGHTS_PATH,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, call, error_code, error_message};
use super::wire::{Param, child, child_text, document, is_long_id, param, set_items};

/// The Region every call here is signed for. A path's ARN names the Region it was created
/// for, so it is read back out of that ARN below.
const REGION: &str = "us-west-2";

/// The HTTP status EC2 answers `DryRunOperation` with: the request was refused because a
/// precondition the CALLER set holds, not because anything was wrong with it.
const DRY_RUN_STATUS: u16 = 412;

/// One port range of a requested filter: `RequestFilterPortRange`'s two bounds, each given or
/// left out.
type RequestedRange = (Option<i32>, Option<i32>);

/// One `PathRequestFilter` as a request spells it.
#[derive(Debug, Clone, Default)]
struct RequestFilter {
    source_address: Option<&'static str>,
    source_port_range: Option<RequestedRange>,
    destination_address: Option<&'static str>,
    destination_port_range: Option<RequestedRange>,
}

/// One `CreateNetworkInsightsPath` request, as the members a caller may choose.
#[derive(Debug, Clone, Default)]
struct PathRequest {
    source: Option<&'static str>,
    destination: Option<&'static str>,
    source_ip: Option<&'static str>,
    destination_ip: Option<&'static str>,
    protocol: Option<&'static str>,
    destination_port: Option<i32>,
    filter_at_source: Option<RequestFilter>,
    filter_at_destination: Option<RequestFilter>,
    tags: &'static [(&'static str, &'static str)],
}

/// The form fields of one requested filter, nested under `prefix` the way ec2Query flattens a
/// structure member.
fn filter_params(prefix: &str, filter: &RequestFilter) -> Vec<Param> {
    let mut params = Vec::new();
    if let Some(address) = filter.source_address {
        params.push(param(&format!("{prefix}.SourceAddress"), address));
    }
    if let Some((from, to)) = filter.source_port_range {
        params.extend(range_params(&format!("{prefix}.SourcePortRange"), from, to));
    }
    if let Some(address) = filter.destination_address {
        params.push(param(&format!("{prefix}.DestinationAddress"), address));
    }
    if let Some((from, to)) = filter.destination_port_range {
        params.extend(range_params(
            &format!("{prefix}.DestinationPortRange"),
            from,
            to,
        ));
    }
    params
}

/// The form fields of one requested port range: whichever of its two bounds the request gives.
fn range_params(prefix: &str, from: Option<i32>, to: Option<i32>) -> Vec<Param> {
    let mut params = Vec::new();
    if let Some(from) = from {
        params.push(param(&format!("{prefix}.FromPort"), &from.to_string()));
    }
    if let Some(to) = to {
        params.push(param(&format!("{prefix}.ToPort"), &to.to_string()));
    }
    params
}

/// One request as the ec2Query form fields that carry it, with `client_token` as its
/// `ClientToken`.
fn params(request: &PathRequest, client_token: &str) -> Vec<Param> {
    let mut params = vec![param("ClientToken", client_token)];
    if let Some(source) = request.source {
        params.push(param("Source", source));
    }
    if let Some(destination) = request.destination {
        params.push(param("Destination", destination));
    }
    if let Some(source_ip) = request.source_ip {
        params.push(param("SourceIp", source_ip));
    }
    if let Some(destination_ip) = request.destination_ip {
        params.push(param("DestinationIp", destination_ip));
    }
    if let Some(protocol) = request.protocol {
        params.push(param("Protocol", protocol));
    }
    if let Some(port) = request.destination_port {
        params.push(param("DestinationPort", &port.to_string()));
    }
    if let Some(filter) = &request.filter_at_source {
        params.extend(filter_params("FilterAtSource", filter));
    }
    if let Some(filter) = &request.filter_at_destination {
        params.extend(filter_params("FilterAtDestination", filter));
    }
    if !request.tags.is_empty() {
        params.push(param(
            "TagSpecification.1.ResourceType",
            RESOURCE_TYPE_NETWORK_INSIGHTS_PATH,
        ));
        for (index, (key, value)) in request.tags.iter().enumerate() {
            let position = index + 1;
            params.push(param(&format!("TagSpecification.1.Tag.{position}.Key"), key));
            params.push(param(
                &format!("TagSpecification.1.Tag.{position}.Value"),
                value,
            ));
        }
    }
    params
}

/// The table every rule here is asserted over: one row per input form the rules generalise
/// over, so no single call is the specification.
fn requests() -> Vec<(&'static str, PathRequest)> {
    vec![
        (
            "a resource pair on tcp port 443, unfiltered and untagged",
            PathRequest {
                source: Some("igw-0123456789abcdef0"),
                destination: Some("i-0123456789abcdef0"),
                protocol: Some("tcp"),
                destination_port: Some(443),
                ..PathRequest::default()
            },
        ),
        (
            "an interface pair on udp carrying both addresses and one tag",
            PathRequest {
                source: Some("eni-0123456789abcdef0"),
                destination: Some("eni-0fedcba9876543210"),
                source_ip: Some("10.0.1.4"),
                destination_ip: Some("10.0.2.7"),
                protocol: Some("udp"),
                tags: &[("Name", "east-west")],
                ..PathRequest::default()
            },
        ),
        (
            "a source and a protocol only",
            PathRequest {
                source: Some("tgw-0123456789abcdef0"),
                protocol: Some("tcp"),
                ..PathRequest::default()
            },
        ),
        (
            "scoped at the source by a filter carrying every member",
            PathRequest {
                source: Some("eni-0123456789abcdef0"),
                destination: Some("eni-0fedcba9876543210"),
                protocol: Some("tcp"),
                filter_at_source: Some(RequestFilter {
                    source_address: Some("10.0.1.4"),
                    source_port_range: Some((Some(49152), Some(65535))),
                    destination_address: Some("10.0.2.7"),
                    destination_port_range: Some((Some(443), Some(443))),
                }),
                tags: &[("Name", "filtered-at-source"), ("Owner", "TeamA")],
                ..PathRequest::default()
            },
        ),
        (
            "scoped at the destination by a one-bound port range, beside a source address \
             and a destination port",
            PathRequest {
                source: Some("i-0123456789abcdef0"),
                destination: Some("i-0fedcba9876543210"),
                source_ip: Some("192.0.2.10"),
                protocol: Some("udp"),
                destination_port: Some(53),
                filter_at_destination: Some(RequestFilter {
                    destination_port_range: Some((Some(3389), None)),
                    ..RequestFilter::default()
                }),
                ..PathRequest::default()
            },
        ),
        (
            "scoped at both ends, with three tags and a long tag value",
            PathRequest {
                source: Some("eni-0123456789abcdef0"),
                destination: Some("vpce-0123456789abcdef0"),
                protocol: Some("tcp"),
                filter_at_source: Some(RequestFilter {
                    source_address: Some("10.0.1.4"),
                    ..RequestFilter::default()
                }),
                filter_at_destination: Some(RequestFilter {
                    source_address: Some("198.51.100.7"),
                    source_port_range: Some((Some(0), Some(1023))),
                    destination_address: Some("203.0.113.9"),
                    destination_port_range: Some((None, Some(8080))),
                }),
                tags: &[
                    ("Name", "filtered-at-both-ends"),
                    ("Owner", "TeamB"),
                    ("Description", "a-long-value-0123456789012345678901234567890"),
                ],
                ..PathRequest::default()
            },
        ),
        (
            "a cross-account ARN as the source, on the lowest published port",
            PathRequest {
                source: Some(
                    "arn:aws:ec2:us-west-2:210987654321:network-interface/eni-0123456789abcdef0",
                ),
                destination: Some("i-0123456789abcdef0"),
                protocol: Some("tcp"),
                destination_port: Some(0),
                ..PathRequest::default()
            },
        ),
        (
            "a short-form interface id, the other form EC2 has issued ids in",
            PathRequest {
                source: Some("eni-01234567"),
                destination: Some("i-0fedcba9876543210"),
                protocol: Some("tcp"),
                destination_port: Some(22),
                ..PathRequest::default()
            },
        ),
        (
            "the highest published port, on udp",
            PathRequest {
                source: Some("eni-0123456789abcdef0"),
                destination: Some("eni-0fedcba9876543210"),
                protocol: Some("udp"),
                destination_port: Some(65535),
                ..PathRequest::default()
            },
        ),
    ]
}

/// One `CreateNetworkInsightsPath` call carrying `request`, as its HTTP status and XML body.
async fn create(
    router: &MultiServiceRouter,
    request: &PathRequest,
    client_token: &str,
) -> (u16, String) {
    create_with(router, &params(request, client_token)).await
}

/// One `CreateNetworkInsightsPath` call carrying `params` exactly as given — the form a row
/// asserting a refusal builds for itself.
async fn create_with(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    call(router, REGION, "CreateNetworkInsightsPath", params).await
}

/// One `CreateNetworkInsightsPath` call that has to succeed, and the body it answered.
async fn created(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    let (status, body) = create_with(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    body
}

/// What `DescribeNetworkInsightsPaths` reports for the one path `path_id` names.
async fn described(case: &str, router: &MultiServiceRouter, path_id: &str) -> String {
    let (status, body) = call(
        router,
        REGION,
        "DescribeNetworkInsightsPaths",
        &[param("NetworkInsightsPathId.1", path_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: describing {path_id} must succeed, got {status} with body {body}"
    );
    body
}

/// How many paths the account holds, read straight out of the store so a refusal that must
/// store nothing can be told from one that stored something.
fn stored(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<Ec2State>()
        .lock()
        .network_insights_paths
        .len()
}

/// One port range as a response reported it.
type ReportedRange = (Option<String>, Option<String>);

/// One `PathFilter` as a response reported it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ReportedFilter {
    source_address: Option<String>,
    source_port_range: Option<ReportedRange>,
    destination_address: Option<String>,
    destination_port_range: Option<ReportedRange>,
}

/// One path as a response reported it: every member of `NetworkInsightsPath`, read out of the
/// PARSED document so a member of the path itself is never confused with one of the same name
/// nested in a filter at either end.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ReportedPath {
    path_id: String,
    path_arn: String,
    created_date: String,
    source: Option<String>,
    destination: Option<String>,
    source_ip: Option<String>,
    destination_ip: Option<String>,
    protocol: Option<String>,
    destination_port: Option<String>,
    filter_at_source: Option<ReportedFilter>,
    filter_at_destination: Option<ReportedFilter>,
    tags: Vec<(String, String)>,
}

/// The `name` port range of a reported filter, or `None` when it carries none.
fn reported_range(filter: roxmltree::Node<'_, '_>, name: &str) -> Option<ReportedRange> {
    let range = child(filter, name)?;
    Some((child_text(range, "fromPort"), child_text(range, "toPort")))
}

/// The `name` filter of a reported path, or `None` when the path carries none.
fn reported_filter(path: roxmltree::Node<'_, '_>, name: &str) -> Option<ReportedFilter> {
    let filter = child(path, name)?;
    Some(ReportedFilter {
        source_address: child_text(filter, "sourceAddress"),
        source_port_range: reported_range(filter, "sourcePortRange"),
        destination_address: child_text(filter, "destinationAddress"),
        destination_port_range: reported_range(filter, "destinationPortRange"),
    })
}

/// Every path a response reported — the create answers one, the describe a set — read as the
/// elements that carry a `networkInsightsPathId`.
fn reported(case: &str, body: &str) -> Vec<ReportedPath> {
    let doc = document(case, body);
    doc.descendants()
        .filter(|node| node.is_element() && child(*node, "networkInsightsPathId").is_some())
        .map(|node| ReportedPath {
            path_id: child_text(node, "networkInsightsPathId").unwrap_or_default(),
            path_arn: child_text(node, "networkInsightsPathArn").unwrap_or_default(),
            created_date: child_text(node, "createdDate").unwrap_or_default(),
            source: child_text(node, "source"),
            destination: child_text(node, "destination"),
            source_ip: child_text(node, "sourceIp"),
            destination_ip: child_text(node, "destinationIp"),
            protocol: child_text(node, "protocol"),
            destination_port: child_text(node, "destinationPort"),
            filter_at_source: reported_filter(node, "filterAtSource"),
            filter_at_destination: reported_filter(node, "filterAtDestination"),
            tags: set_items(node, "tagSet")
                .into_iter()
                .map(|item| {
                    (
                        child_text(item, "key").unwrap_or_default(),
                        child_text(item, "value").unwrap_or_default(),
                    )
                })
                .collect(),
        })
        .collect()
}

/// The ONE path a response reported, or a panic naming the case.
fn one_path(case: &str, body: &str) -> ReportedPath {
    let mut found = reported(case, body);
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one path, got {found:?} from {body}"
    );
    found.remove(0)
}

/// One requested range as the response must report it.
fn expected_range(range: Option<RequestedRange>) -> Option<ReportedRange> {
    range.map(|(from, to)| {
        (
            from.map(|port| port.to_string()),
            to.map(|port| port.to_string()),
        )
    })
}

/// One requested filter as the response must report it.
fn expected_filter(filter: &RequestFilter) -> ReportedFilter {
    ReportedFilter {
        source_address: filter.source_address.map(str::to_string),
        source_port_range: expected_range(filter.source_port_range),
        destination_address: filter.destination_address.map(str::to_string),
        destination_port_range: expected_range(filter.destination_port_range),
    }
}

/// The path a request must be answered with, bar the three members the SERVICE owns (the id,
/// the ARN and the creation date), which are asserted on their own form.
fn expected(request: &PathRequest) -> ReportedPath {
    ReportedPath {
        path_id: String::new(),
        path_arn: String::new(),
        created_date: String::new(),
        source: request.source.map(str::to_string),
        destination: request.destination.map(str::to_string),
        source_ip: request.source_ip.map(str::to_string),
        destination_ip: request.destination_ip.map(str::to_string),
        protocol: request.protocol.map(str::to_string),
        destination_port: request.destination_port.map(|port| port.to_string()),
        filter_at_source: request.filter_at_source.as_ref().map(expected_filter),
        filter_at_destination: request.filter_at_destination.as_ref().map(expected_filter),
        tags: request
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect(),
    }
}

/// A reported path with the three service-owned members blanked, so what is left is exactly
/// what the caller chose.
fn caller_chosen(path: &ReportedPath) -> ReportedPath {
    ReportedPath {
        path_id: String::new(),
        path_arn: String::new(),
        created_date: String::new(),
        ..path.clone()
    }
}

// ---------------------------------------------------------------------------
// RULE 1: the create stores one path and answers it as stored
// ---------------------------------------------------------------------------

/// RULE 1: every row is stored and answered as stored — a minted long-form id, an ARN naming
/// the Region and that id, a creation date, and every caller-chosen member echoed — and a
/// later describe reports the very same path.
#[tokio::test]
async fn the_create_stores_the_path_it_answers_with() {
    for (case, request) in requests() {
        let (router, engine) = super::ec2_query::emulator();
        assert_eq!(
            stored(&engine),
            0,
            "case {case:?}: the account must start with no path, got {}",
            stored(&engine)
        );

        let body = created(case, &router, &params(&request, "token-create")).await;
        let path = one_path(case, &body);

        assert!(
            is_long_id(NETWORK_INSIGHTS_PATH_ID_PREFIX, &path.path_id),
            "case {case:?}: expected a long {NETWORK_INSIGHTS_PATH_ID_PREFIX}- id, got {:?}",
            path.path_id
        );
        let expected_arn_tail = format!(
            "{RESOURCE_TYPE_NETWORK_INSIGHTS_PATH}/{path_id}",
            path_id = path.path_id
        );
        assert!(
            path.path_arn.starts_with("arn:")
                && path.path_arn.contains(&format!(":{REGION}:"))
                && path.path_arn.ends_with(&expected_arn_tail),
            "case {case:?}: expected an ARN naming {REGION} and {expected_arn_tail}, got {:?}",
            path.path_arn
        );
        assert!(
            as_timestamp(&path.created_date).is_some(),
            "case {case:?}: expected an ISO 8601 createdDate, got {:?}",
            path.created_date
        );
        assert_eq!(
            caller_chosen(&path),
            expected(&request),
            "case {case:?}: expected the path to carry what the request chose, got {path:?}"
        );
        assert_eq!(
            stored(&engine),
            1,
            "case {case:?}: expected the account to hold exactly one path, got {}",
            stored(&engine)
        );

        // The round trip: the stored path is what a later reader reports, member for member.
        let described_body = described(case, &router, &path.path_id).await;
        assert_eq!(
            one_path(case, &described_body),
            path,
            "case {case:?}: expected the describe to report the created path unchanged, got \
             {described_body}"
        );
    }
}

/// RULE 1: the path the create stored is the one `StartNetworkInsightsAnalysis` resolves, so
/// an analysis started on it names it. One row per request form.
#[tokio::test]
async fn an_analysis_can_be_started_on_the_path_the_create_stored() {
    for (case, request) in requests() {
        let (router, _engine) = super::ec2_query::emulator();
        let body = created(case, &router, &params(&request, "token-analyze")).await;
        let path_id = one_path(case, &body).path_id;

        let (status, analysis) = call(
            &router,
            REGION,
            "StartNetworkInsightsAnalysis",
            &[
                param("ClientToken", "token-analysis"),
                param("NetworkInsightsPathId", &path_id),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: an analysis of {path_id} must start, got {status} with {analysis}"
        );
        assert!(
            analysis.contains(&format!("<networkInsightsPathId>{path_id}</networkInsightsPathId>")),
            "case {case:?}: expected the analysis to name {path_id}, got {analysis}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 2: each create mints a path of its own
// ---------------------------------------------------------------------------

/// RULE 2: two creates leave the account holding two paths with two ids, each reported with
/// the members its own request chose. The rows are each pair of neighbouring table entries,
/// so the second create is a different request from the first.
#[tokio::test]
async fn each_create_mints_a_path_of_its_own() {
    let table = requests();
    for position in 0..table.len() {
        let (first_case, first) = &table[position];
        let (second_case, second) = &table[(position + 1) % table.len()];
        let case = format!("{first_case} then {second_case}");
        let (router, engine) = super::ec2_query::emulator();

        let first_body = created(&case, &router, &params(first, "token-one")).await;
        let second_body = created(&case, &router, &params(second, "token-two")).await;
        let first_path = one_path(&case, &first_body);
        let second_path = one_path(&case, &second_body);

        assert_ne!(
            first_path.path_id, second_path.path_id,
            "case {case:?}: expected two ids, got {:?} twice",
            first_path.path_id
        );
        assert_eq!(
            stored(&engine),
            2,
            "case {case:?}: expected the account to hold two paths, got {}",
            stored(&engine)
        );
        assert_eq!(
            caller_chosen(&one_path(&case, &described(&case, &router, &first_path.path_id).await)),
            expected(first),
            "case {case:?}: expected the first path to keep its own members"
        );
        assert_eq!(
            caller_chosen(&one_path(
                &case,
                &described(&case, &router, &second_path.path_id).await
            )),
            expected(second),
            "case {case:?}: expected the second path to keep its own members"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 3: a filter at one end excludes the members describing that end
// ---------------------------------------------------------------------------

/// RULE 3: a filter at one end beside a member describing that same end is refused with
/// `InvalidParameterCombination` naming both parameters, and nothing is stored. The rows are
/// each excluded pairing the model documents.
#[tokio::test]
async fn a_filter_cannot_be_combined_with_the_members_of_the_end_it_scopes() {
    let scoping = RequestFilter {
        source_address: Some("10.0.1.4"),
        source_port_range: Some((Some(1024), Some(2048))),
        ..RequestFilter::default()
    };
    let cases: Vec<(&str, PathRequest, &str, &str)> = vec![
        (
            "FilterAtSource beside SourceIp",
            PathRequest {
                source: Some("eni-0123456789abcdef0"),
                destination: Some("eni-0fedcba9876543210"),
                protocol: Some("tcp"),
                source_ip: Some("10.0.1.4"),
                filter_at_source: Some(scoping.clone()),
                ..PathRequest::default()
            },
            "FilterAtSource",
            "SourceIp",
        ),
        (
            "FilterAtSource beside DestinationPort",
            PathRequest {
                source: Some("eni-0123456789abcdef0"),
                destination: Some("eni-0fedcba9876543210"),
                protocol: Some("tcp"),
                destination_port: Some(443),
                filter_at_source: Some(scoping.clone()),
                ..PathRequest::default()
            },
            "FilterAtSource",
            "DestinationPort",
        ),
        (
            "FilterAtDestination beside DestinationIp",
            PathRequest {
                source: Some("eni-0123456789abcdef0"),
                destination: Some("eni-0fedcba9876543210"),
                protocol: Some("udp"),
                destination_ip: Some("10.0.2.7"),
                filter_at_destination: Some(scoping.clone()),
                ..PathRequest::default()
            },
            "FilterAtDestination",
            "DestinationIp",
        ),
    ];

    for (case, request, first, second) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let (status, body) = create(&router, &request, "token-combination").await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_PARAMETER_COMBINATION,
            "case {case:?}: expected {INVALID_PARAMETER_COMBINATION}, got {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains(first) && message.contains(second),
            "case {case:?}: expected the refusal to name {first} and {second}, got {message:?}"
        );
        assert_eq!(
            stored(&engine),
            0,
            "case {case:?}: a refused create must store nothing, got {}",
            stored(&engine)
        );
    }
}

/// RULE 3: every pairing the model does NOT exclude is an ordinary request — a filter at the
/// source beside the destination address, and a filter at the destination beside the source
/// address and the destination port.
#[tokio::test]
async fn the_pairings_the_model_does_not_exclude_are_ordinary_requests() {
    let scoping = RequestFilter {
        source_address: Some("10.0.1.4"),
        destination_port_range: Some((Some(443), Some(443))),
        ..RequestFilter::default()
    };
    let cases: Vec<(&str, PathRequest)> = vec![
        (
            "FilterAtSource beside DestinationIp",
            PathRequest {
                source: Some("eni-0123456789abcdef0"),
                destination: Some("eni-0fedcba9876543210"),
                protocol: Some("tcp"),
                destination_ip: Some("10.0.2.7"),
                filter_at_source: Some(scoping.clone()),
                ..PathRequest::default()
            },
        ),
        (
            "FilterAtDestination beside SourceIp and DestinationPort",
            PathRequest {
                source: Some("eni-0123456789abcdef0"),
                destination: Some("eni-0fedcba9876543210"),
                protocol: Some("udp"),
                source_ip: Some("10.0.1.4"),
                destination_port: Some(53),
                filter_at_destination: Some(scoping.clone()),
                ..PathRequest::default()
            },
        ),
        (
            "both filters at once",
            PathRequest {
                source: Some("eni-0123456789abcdef0"),
                destination: Some("eni-0fedcba9876543210"),
                protocol: Some("tcp"),
                filter_at_source: Some(scoping.clone()),
                filter_at_destination: Some(scoping.clone()),
                ..PathRequest::default()
            },
        ),
    ];

    for (case, request) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let body = created(case, &router, &params(&request, "token-allowed")).await;
        assert_eq!(
            caller_chosen(&one_path(case, &body)),
            expected(&request),
            "case {case:?}: expected the path to carry what the request chose, got {body}"
        );
        assert_eq!(
            stored(&engine),
            1,
            "case {case:?}: expected the account to hold one path, got {}",
            stored(&engine)
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 4: the constraints the model publishes
// ---------------------------------------------------------------------------

/// RULE 4: a value outside what the model publishes for the member carrying it is refused and
/// nothing is stored. The rows are a protocol outside the `Protocol` catalog (`tcp | udp`),
/// including one differing only in case, a port one past each published bound of `Port`
/// (`range 0..65535`), an address outside `IpAddress`'s pattern and one past its published
/// length (`length 0..15`), and a request leaving out the required `Source`.
#[tokio::test]
async fn a_value_outside_the_model_is_refused_and_stores_nothing() {
    let base = PathRequest {
        source: Some("eni-0123456789abcdef0"),
        destination: Some("eni-0fedcba9876543210"),
        protocol: Some("tcp"),
        ..PathRequest::default()
    };
    let cases: Vec<(&str, PathRequest)> = vec![
        (
            "a protocol outside the catalog",
            PathRequest {
                protocol: Some("icmp"),
                ..base.clone()
            },
        ),
        (
            "a protocol differing only in case",
            PathRequest {
                protocol: Some("TCP"),
                ..base.clone()
            },
        ),
        (
            "an empty protocol",
            PathRequest {
                protocol: Some(""),
                ..base.clone()
            },
        ),
        (
            "a port one past the top of the published range",
            PathRequest {
                destination_port: Some(65536),
                ..base.clone()
            },
        ),
        (
            "a port one below the bottom of the published range",
            PathRequest {
                destination_port: Some(-1),
                ..base.clone()
            },
        ),
        (
            "a source address outside the published pattern",
            PathRequest {
                source_ip: Some("not-an-ip"),
                ..base.clone()
            },
        ),
        (
            "a destination address one past the published length",
            PathRequest {
                destination_ip: Some("1111.1111.1111."),
                ..base.clone()
            },
        ),
        (
            "no Source at all",
            PathRequest {
                source: None,
                ..base.clone()
            },
        ),
    ];

    for (case, request) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let (status, body) = create(&router, &request, "token-constraint").await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            stored(&engine),
            0,
            "case {case:?}: a refused create must store nothing, got {}",
            stored(&engine)
        );
    }
}

/// The resource types an end of a path may name, as the service's own refusal of any other
/// kind lists them: `<value> is not a supported <Member> type. Supported <Member> types
/// include: [instance, network-interface, internet-gateway, vpn-gateway, vpc-endpoint,
/// vpc-endpoint-service, vpc-peering-connection, transit-gateway, transit-gateway-attachment]`.
const SUPPORTED_ENDPOINT_TYPES: &str = "instance, network-interface, internet-gateway, \
     vpn-gateway, vpc-endpoint, vpc-endpoint-service, vpc-peering-connection, transit-gateway, \
     transit-gateway-attachment";

/// A well-formed id of a type both ends accept, for the row that is asserting the OTHER end.
const AN_INSTANCE_ID: &str = "i-0123456789abcdef0";

/// One request naming `value` as `member` and a plain instance at the other end.
fn one_end_named(member: &str, value: &str) -> Vec<Param> {
    let mut params = vec![
        param("ClientToken", "token-endpoint"),
        param("Protocol", "tcp"),
    ];
    if member == "Source" {
        params.push(param("Source", value));
        params.push(param("Destination", AN_INSTANCE_ID));
    } else {
        params.push(param("Source", AN_INSTANCE_ID));
        params.push(param("Destination", value));
    }
    params
}

/// RULE 4: every type the catalog names is accepted at either end, named by its id in either
/// form EC2 issues ids in or by an ARN, and the value is stored verbatim. One row per catalog
/// entry — including the entries the issue never mentioned — plus two ARN forms.
#[tokio::test]
async fn each_type_the_catalog_names_is_accepted_at_either_end() {
    // One id per type the handler's catalog names, under the prefix EC2 issues that type's
    // ids with, then the same resources named by ARN as a cross-account end must be.
    let named = [
        ("instance", AN_INSTANCE_ID),
        ("network-interface, short form", "eni-01234567"),
        ("internet-gateway", "igw-0123456789abcdef0"),
        ("vpn-gateway", "vgw-0123456789abcdef0"),
        ("vpc-endpoint", "vpce-0123456789abcdef0"),
        ("vpc-endpoint-service", "vpce-svc-0123456789abcdef0"),
        ("vpc-peering-connection", "pcx-0123456789abcdef0"),
        ("transit-gateway", "tgw-0123456789abcdef0"),
        (
            "transit-gateway-attachment",
            "tgw-attach-0123456789abcdef0",
        ),
        (
            "a network interface by ARN",
            "arn:aws:ec2:us-west-2:210987654321:network-interface/eni-0123456789abcdef0",
        ),
        (
            "a transit gateway attachment by ARN",
            "arn:aws:ec2:us-west-2:210987654321:transit-gateway-attachment/\
             tgw-attach-0123456789abcdef0",
        ),
    ];

    for (kind, value) in named {
        for member in ["Source", "Destination"] {
            let case = format!("{kind} as {member}");
            let (router, engine) = super::ec2_query::emulator();
            let body = created(&case, &router, &one_end_named(member, value)).await;
            let path = one_path(&case, &body);
            let reported_end = if member == "Source" {
                path.source.clone()
            } else {
                path.destination.clone()
            };
            assert_eq!(
                reported_end.as_deref(),
                Some(value),
                "case {case:?}: expected the end to be stored verbatim, got {reported_end:?}"
            );
            assert_eq!(
                stored(&engine),
                1,
                "case {case:?}: expected the account to hold one path, got {}",
                stored(&engine)
            );
        }
    }
}

/// RULE 4: an end naming a resource of a kind no path can be analyzed between — or naming no
/// resource at all — is refused with EC2's `InvalidParameterValue`, worded with the value, the
/// member that carried it and the catalog that member accepts, and nothing is stored. The rows
/// are each way a value falls outside the catalog, asserted on `Source` and on `Destination`
/// alike.
#[tokio::test]
async fn an_end_outside_the_catalog_is_refused() {
    // Free text, a prefix with no hexadecimal suffix, a suffix of neither published length, a
    // non-hexadecimal digit, upper case, an empty value, a truncated ARN, an ARN naming no
    // resource, well-formed ids of kinds outside the catalog, and an ARN of one of those
    // kinds.
    let outside = [
        "banana",
        "source-1",
        "i-0123",
        "i-0123456789abcdefg",
        "I-0123456789ABCDEF0",
        "",
        "arn:aws:ec2",
        "arn:aws:ec2:us-west-2:123456789012:network-interface/",
        "vpc-0123456789abcdef0",
        "nat-0123456789abcdef0",
        "sg-0123456789abcdef0",
        "vol-0123456789abcdef0",
        "tgw-rtb-0123456789abcdef0",
        "arn:aws:ec2:us-west-2:123456789012:vpc/vpc-0123456789abcdef0",
    ];

    for value in outside {
        for member in ["Source", "Destination"] {
            let case = format!("{member}={value:?}");
            let (router, engine) = super::ec2_query::emulator();

            let (status, body) = create_with(&router, &one_end_named(member, value)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                INVALID_PARAMETER_VALUE,
                "case {case:?}: expected {INVALID_PARAMETER_VALUE}, got {body}"
            );
            assert_eq!(
                error_message(&body),
                format!(
                    "{value} is not a supported {member} type. Supported {member} types \
                     include: [{SUPPORTED_ENDPOINT_TYPES}]"
                ),
                "case {case:?}: expected the refusal to name the value, the member and the \
                 catalog, got {:?}",
                error_message(&body)
            );
            assert_eq!(
                stored(&engine),
                0,
                "case {case:?}: a refused create must store nothing, got {}",
                stored(&engine)
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE 5: the tags a specification may carry
// ---------------------------------------------------------------------------

/// RULE 5: a tag specification may name only the resource type this action tags — a
/// specification naming another type is refused with `InvalidParameterValue` naming it, one
/// naming no type at all with `MissingParameter` — and nothing is stored either way.
#[tokio::test]
async fn a_tag_specification_may_only_name_the_resource_this_action_tags() {
    let cases: Vec<(&str, Vec<Param>, &str)> = vec![
        (
            "another resource type",
            vec![
                param("TagSpecification.1.ResourceType", "vpc"),
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", "misdirected"),
            ],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "the analysis rather than the path",
            vec![
                param(
                    "TagSpecification.1.ResourceType",
                    "network-insights-analysis",
                ),
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", "misdirected"),
            ],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "no resource type at all",
            vec![
                param("TagSpecification.1.Tag.1.Key", "Name"),
                param("TagSpecification.1.Tag.1.Value", "untyped"),
            ],
            MISSING_PARAMETER,
        ),
    ];

    for (case, tag_params, expected_code) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let mut request_params = params(
            &PathRequest {
                source: Some("eni-0123456789abcdef0"),
                destination: Some("eni-0fedcba9876543210"),
                protocol: Some("tcp"),
                ..PathRequest::default()
            },
            "token-tags",
        );
        request_params.extend(tag_params);

        let (status, body) = create_with(&router, &request_params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        assert_eq!(
            stored(&engine),
            0,
            "case {case:?}: a refused create must store nothing, got {}",
            stored(&engine)
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 6: DryRun
// ---------------------------------------------------------------------------

/// RULE 6: a dry run of a request that WOULD have worked reports `DryRunOperation` and stores
/// nothing, so the same request without the flag still creates the first path of the account.
/// One row per table entry.
#[tokio::test]
async fn a_dry_run_stores_nothing_and_reports_the_permitted_outcome() {
    for (case, request) in requests() {
        let (router, engine) = super::ec2_query::emulator();
        let mut dry = params(&request, "token-dry-run");
        dry.push(param("DryRun", "true"));

        let (status, body) = create_with(&router, &dry).await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            DRY_RUN_OPERATION,
            "case {case:?}: expected {DRY_RUN_OPERATION}, got {body}"
        );
        assert_eq!(
            stored(&engine),
            0,
            "case {case:?}: a dry run must store nothing, got {}",
            stored(&engine)
        );

        let body = created(case, &router, &params(&request, "token-after-dry-run")).await;
        assert_eq!(
            caller_chosen(&one_path(case, &body)),
            expected(&request),
            "case {case:?}: the same request without the flag must create the path, got {body}"
        );
    }
}

/// RULE 6: a dry run asks whether the request WOULD have worked, so one that would have been
/// refused is refused with THAT reason rather than reported as permitted. The rows are an
/// excluded parameter pairing and a value outside the model.
#[tokio::test]
async fn a_dry_run_that_would_have_been_refused_gets_that_reason() {
    let cases: Vec<(&str, PathRequest, &str)> = vec![
        (
            "an excluded pairing",
            PathRequest {
                source: Some("eni-0123456789abcdef0"),
                protocol: Some("tcp"),
                source_ip: Some("10.0.1.4"),
                filter_at_source: Some(RequestFilter {
                    source_address: Some("10.0.1.4"),
                    ..RequestFilter::default()
                }),
                ..PathRequest::default()
            },
            INVALID_PARAMETER_COMBINATION,
        ),
        (
            "a protocol outside the catalog",
            PathRequest {
                source: Some("eni-0123456789abcdef0"),
                protocol: Some("icmp"),
                ..PathRequest::default()
            },
            "",
        ),
    ];

    for (case, request, expected_code) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let mut dry = params(&request, "token-dry-refusal");
        dry.push(param("DryRun", "true"));

        let (status, body) = create_with(&router, &dry).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the refusal's own HTTP 400, got {status} with body {body}"
        );
        assert_ne!(
            error_code(&body),
            DRY_RUN_OPERATION,
            "case {case:?}: expected the refusal rather than a permitted dry run, got {body}"
        );
        if !expected_code.is_empty() {
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?}: expected {expected_code}, got {body}"
            );
        }
        assert_eq!(
            stored(&engine),
            0,
            "case {case:?}: a refused dry run must store nothing, got {}",
            stored(&engine)
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 7: the client token's bound
// ---------------------------------------------------------------------------

/// RULE 7: a `ClientToken` at this crate's bound is accepted and one past it is refused with
/// `InvalidParameterValue` naming the bound, storing nothing. The rows are the bound itself
/// and one character past it.
#[tokio::test]
async fn a_client_token_past_the_bound_is_refused() {
    let request = PathRequest {
        source: Some("eni-0123456789abcdef0"),
        destination: Some("eni-0fedcba9876543210"),
        protocol: Some("tcp"),
        ..PathRequest::default()
    };

    for (length, expected_status, expected_paths) in [
        (MAX_CLIENT_TOKEN_CHARS, 200, 1),
        (MAX_CLIENT_TOKEN_CHARS + 1, 400, 0),
    ] {
        let case = format!("a {length}-character ClientToken");
        let (router, engine) = super::ec2_query::emulator();
        let (status, body) = create(&router, &request, &"t".repeat(length)).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                INVALID_PARAMETER_VALUE,
                "case {case:?}: expected {INVALID_PARAMETER_VALUE}, got {body}"
            );
            assert!(
                error_message(&body).contains(&MAX_CLIENT_TOKEN_CHARS.to_string()),
                "case {case:?}: expected the refusal to name the bound, got {:?}",
                error_message(&body)
            );
        }
        assert_eq!(
            stored(&engine),
            expected_paths,
            "case {case:?}: expected the account to hold {expected_paths} paths, got {}",
            stored(&engine)
        );
    }
}
