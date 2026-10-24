// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the Infrastructure Performance metric subscription trio —
//! `EnableAwsNetworkPerformanceMetricSubscription`,
//! `DisableAwsNetworkPerformanceMetricSubscription` and
//! `DescribeAwsNetworkPerformanceMetricSubscriptions` — driven over the wire the service
//! actually speaks: `ec2Query`, an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=<Operation>`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol").
//!
//! The rule under test is that the describe REPORTS THE STORE: the set it answers with is
//! exactly the subscriptions the enable has put there and the disable has not removed,
//! narrowed by the `Filters` the request names and cut into `MaxResults`-sized pages in
//! store order. Every table row is a distinct input along what that generalises over —
//! which endpoint form (Region code or Availability Zone id), which filter, how many
//! values, how many filters, which page size — rather than one recorded pair standing for
//! the rule.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, unescaped};

/// The Region every call here is signed for. A subscription names its own source and
/// destination, so the signed Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// The one `MetricType` member the model publishes.
const METRIC: &str = "aggregate-latency";

/// The one `StatisticType` member the model publishes.
const STATISTIC: &str = "p50";

/// The `PeriodType` member a subscription this API enables is reported with.
const PERIOD: &str = "five-minutes";

/// One `DescribeAwsNetworkPerformanceMetricSubscriptions` call carrying `params`.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(
        router,
        REGION,
        "DescribeAwsNetworkPerformanceMetricSubscriptions",
        params,
    )
    .await
}

/// The `Source`/`Destination`/`Metric`/`Statistic` form fields naming one subscription.
fn naming(source: &str, destination: &str, metric: &str, statistic: &str) -> Vec<(String, String)> {
    vec![
        ("Source".to_string(), source.to_string()),
        ("Destination".to_string(), destination.to_string()),
        ("Metric".to_string(), metric.to_string()),
        ("Statistic".to_string(), statistic.to_string()),
    ]
}

/// One `EnableAwsNetworkPerformanceMetricSubscription` call for the subscription named by
/// `params`.
async fn enable(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(
        router,
        REGION,
        "EnableAwsNetworkPerformanceMetricSubscription",
        params,
    )
    .await
}

/// One `DisableAwsNetworkPerformanceMetricSubscription` call.
async fn disable(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(
        router,
        REGION,
        "DisableAwsNetworkPerformanceMetricSubscription",
        params,
    )
    .await
}

/// Enable the subscription between `source` and `destination` for the published metric and
/// statistic, asserting the call succeeded and reported the subscribe as successful.
async fn enable_pair(router: &MultiServiceRouter, source: &str, destination: &str) {
    let case = format!("{source} -> {destination}");
    let (status, body) = enable(router, &naming(source, destination, METRIC, STATISTIC)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "output").as_deref(),
        Some("true"),
        "case {case:?}: expected the subscribe to report success, got {body}"
    );
}

/// The `(source, destination)` pairs a successful describe response reports, in the order it
/// reported them.
fn reported_pairs(body: &str) -> Vec<(String, String)> {
    elements(body, "source")
        .into_iter()
        .zip(elements(body, "destination"))
        .collect()
}

/// The pairs one describe call reports, asserting it succeeded.
async fn described_pairs(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<(String, String)> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    reported_pairs(&body)
}

/// The source/destination pairs the subscriptions below are written for. They differ along
/// the two endpoint forms the model documents — "the source Region (like us-east-1) or
/// Availability Zone ID (like use1-az1)" — in every combination: Region to Region, Region to
/// zone, zone to Region, and zone to zone inside one Region, plus a pair that is the reverse
/// of another so direction is part of a subscription's identity.
const SEEDED_PAIRS: [(&str, &str); 6] = [
    ("us-east-1", "eu-west-1"),
    ("us-east-1", "use2-az2"),
    ("use1-az1", "use1-az2"),
    ("use1-az1", "eu-west-1"),
    ("ap-northeast-1", "ap-south-1"),
    ("eu-west-1", "us-east-1"),
];

/// Enable every [`SEEDED_PAIRS`] subscription through the API itself, and answer the pairs
/// the store now holds IN STORE ORDER — which is the order the describe must report and page
/// in.
async fn seed(router: &MultiServiceRouter, engine: &engine::InMemoryEngine) -> Vec<(String, String)> {
    for (source, destination) in SEEDED_PAIRS {
        enable_pair(router, source, destination).await;
    }
    stored_pairs(engine)
}

/// The pairs the store holds, in its own key order.
fn stored_pairs(engine: &engine::InMemoryEngine) -> Vec<(String, String)> {
    engine
        .state::<Ec2State>()
        .lock()
        .network_performance_subscriptions
        .values()
        .map(|subscription| {
            (
                subscription.source.clone().unwrap_or_default(),
                subscription.destination.clone().unwrap_or_default(),
            )
        })
        .collect()
}

/// The `wanted` pairs, in the order the store holds them — so a table row can name its
/// expected subscriptions in any order and still assert the order they come back in.
fn in_store_order(stored: &[(String, String)], wanted: &[(&str, &str)]) -> Vec<(String, String)> {
    stored
        .iter()
        .filter(|(source, destination)| {
            wanted
                .iter()
                .any(|(want_source, want_destination)| {
                    want_source == source && want_destination == destination
                })
        })
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// A fresh account has subscribed to nothing, and describing that is a READ: two calls
/// report the same empty set and the store is still empty afterwards.
#[tokio::test]
async fn a_fresh_account_has_no_subscriptions_and_describing_it_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    let first = described_pairs(&router, "fresh account", &[]).await;
    assert!(
        first.is_empty(),
        "fresh account: expected no subscription, got {first:?}"
    );
    let second = described_pairs(&router, "second read", &[]).await;
    assert_eq!(
        second, first,
        "a read must not change what a later caller sees: {first:?} then {second:?}"
    );
    assert!(
        engine
            .state::<Ec2State>()
            .lock()
            .network_performance_subscriptions
            .is_empty(),
        "describing subscriptions must not write to the store"
    );
}

/// Every subscription the enable put in the store is reported, with all five members the
/// modelled `Subscription` carries, and a subscription enabled after a call shows up in the
/// next one — the answer is computed from the store, not fixed.
#[tokio::test]
async fn every_enabled_subscription_is_reported_with_its_members() {
    let (router, engine) = super::ec2_query::emulator();
    let stored = seed(&router, &engine).await;

    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "all subscriptions: expected HTTP 200, got {status} with body {body}"
    );
    let reported = reported_pairs(&body);
    assert_eq!(
        reported, stored,
        "all subscriptions: expected every stored subscription in store order, got {reported:?}"
    );
    assert_eq!(
        reported.len(),
        SEEDED_PAIRS.len(),
        "all subscriptions: expected {} subscriptions, got {reported:?} from {body}",
        SEEDED_PAIRS.len()
    );
    // The metric and statistic each subscription was enabled with, and the aggregation
    // period the service reports for one, on every row.
    for (member, value) in [("metric", METRIC), ("statistic", STATISTIC), ("period", PERIOD)] {
        let values = elements(&body, member);
        assert_eq!(
            values,
            vec![value.to_string(); SEEDED_PAIRS.len()],
            "all subscriptions: expected <{member}>{value}</{member}> on every row, got {body}"
        );
    }

    // A subscription enabled after that call is reported by the next one.
    enable_pair(&router, "sa-east-1", "us-west-2").await;
    let after = described_pairs(&router, "after a later enable", &[]).await;
    assert_eq!(
        after,
        stored_pairs(&engine),
        "after a later enable: expected the store's new contents, got {after:?}"
    );
    assert!(
        after.contains(&("sa-east-1".to_string(), "us-west-2".to_string())),
        "after a later enable: expected the new subscription to be reported, got {after:?}"
    );
}

/// The four members a subscription is named by are its identity: enabling the same four
/// twice leaves ONE subscription, and a pair that differs in any one of them is a different
/// subscription.
#[tokio::test]
async fn enabling_a_subscription_is_idempotent_in_its_four_members() {
    let (router, engine) = super::ec2_query::emulator();

    enable_pair(&router, "us-east-1", "eu-west-1").await;
    enable_pair(&router, "us-east-1", "eu-west-1").await;
    let reported = described_pairs(&router, "the same subscription twice", &[]).await;
    assert_eq!(
        reported,
        vec![("us-east-1".to_string(), "eu-west-1".to_string())],
        "the same subscription twice: expected one subscription, got {reported:?}"
    );

    // Swapping source and destination names a different subscription, so now there are two.
    enable_pair(&router, "eu-west-1", "us-east-1").await;
    let both = described_pairs(&router, "the reverse direction", &[]).await;
    assert_eq!(
        both,
        stored_pairs(&engine),
        "the reverse direction: expected both subscriptions in store order, got {both:?}"
    );
    assert_eq!(
        both.len(),
        2,
        "the reverse direction: expected two subscriptions, got {both:?}"
    );
}

/// One row of the disable table: the pair to disable, and the pairs that must still be
/// reported afterwards.
type DisableCase<'a> = ((&'a str, &'a str), Vec<(&'a str, &'a str)>);

/// One row of the filter table: the filter name, the values to send, and the pairs that must
/// come back.
type FilterCase<'a> = (&'a str, Vec<&'a str>, Vec<(&'a str, &'a str)>);

/// The disable removes exactly the subscription its four members name, and nothing else;
/// disabling one that is not enabled still reports success and removes nothing.
#[tokio::test]
async fn disabling_removes_only_the_subscription_it_names() {
    let (router, engine) = super::ec2_query::emulator();
    let stored = seed(&router, &engine).await;

    // Rows: a Region-to-Region pair, a zone-to-zone pair, and a pair nothing subscribed.
    let cases: Vec<DisableCase<'_>> = vec![
        (
            ("us-east-1", "eu-west-1"),
            vec![
                ("us-east-1", "use2-az2"),
                ("use1-az1", "use1-az2"),
                ("use1-az1", "eu-west-1"),
                ("ap-northeast-1", "ap-south-1"),
                ("eu-west-1", "us-east-1"),
            ],
        ),
        (
            ("use1-az1", "use1-az2"),
            vec![
                ("us-east-1", "use2-az2"),
                ("use1-az1", "eu-west-1"),
                ("ap-northeast-1", "ap-south-1"),
                ("eu-west-1", "us-east-1"),
            ],
        ),
        // Nothing is subscribed between these two, so the disable has nothing to remove.
        (
            ("us-west-1", "us-west-2"),
            vec![
                ("us-east-1", "use2-az2"),
                ("use1-az1", "eu-west-1"),
                ("ap-northeast-1", "ap-south-1"),
                ("eu-west-1", "us-east-1"),
            ],
        ),
    ];

    for ((source, destination), expected_pairs) in cases {
        let case = format!("disable {source} -> {destination}");
        let (status, body) = disable(&router, &naming(source, destination, METRIC, STATISTIC)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "output").as_deref(),
            Some("true"),
            "case {case:?}: expected the unsubscribe to report success, got {body}"
        );
        let reported = described_pairs(&router, &case, &[]).await;
        let expected = in_store_order(&stored, &expected_pairs);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// Each filter selects on its own member of the reported `Subscription`, values within one
/// filter are OR'd, the comparison is case-sensitive, and EC2's `*`/`?` wildcards apply.
#[tokio::test]
async fn each_filter_selects_the_subscriptions_whose_member_matches_a_value() {
    let (router, engine) = super::ec2_query::emulator();
    let stored = seed(&router, &engine).await;
    let all: Vec<(&str, &str)> = SEEDED_PAIRS.to_vec();

    // One row per filter name this operation defines — the members `Subscription` carries —
    // each asserted on a subscription the issue never cited.
    let cases: Vec<FilterCase<'_>> = vec![
        (
            "source",
            vec!["us-east-1"],
            vec![("us-east-1", "eu-west-1"), ("us-east-1", "use2-az2")],
        ),
        ("source", vec!["use1-az1"], vec![
            ("use1-az1", "use1-az2"),
            ("use1-az1", "eu-west-1"),
        ]),
        // Two values for one filter: joined with OR.
        (
            "source",
            vec!["ap-northeast-1", "eu-west-1"],
            vec![("ap-northeast-1", "ap-south-1"), ("eu-west-1", "us-east-1")],
        ),
        // A source nothing subscribed from: an empty set, not an error.
        ("source", vec!["us-west-2"], vec![]),
        (
            "destination",
            vec!["eu-west-1"],
            vec![("us-east-1", "eu-west-1"), ("use1-az1", "eu-west-1")],
        ),
        ("destination", vec!["use2-az2"], vec![("us-east-1", "use2-az2")]),
        ("metric", vec![METRIC], all.clone()),
        ("metric", vec!["aggregate-throughput"], vec![]),
        ("statistic", vec![STATISTIC], all.clone()),
        ("period", vec![PERIOD], all.clone()),
        ("period", vec!["one-hour"], vec![]),
        // "Filter values are case-sensitive" (`Filter$Values`).
        ("source", vec!["US-EAST-1"], vec![]),
        // EC2's wildcards: `*` for zero or more characters, `?` for exactly one ("List and
        // filter using the CLI and API", the guide the modelled `Filter` shape links to).
        (
            "source",
            vec!["us*"],
            vec![
                ("us-east-1", "eu-west-1"),
                ("us-east-1", "use2-az2"),
                ("use1-az1", "use1-az2"),
                ("use1-az1", "eu-west-1"),
            ],
        ),
        (
            "destination",
            vec!["use?-az?"],
            vec![("us-east-1", "use2-az2"), ("use1-az1", "use1-az2")],
        ),
        ("destination", vec!["use??-az?"], vec![]),
        // A defined filter carrying no value at all: there is no value for the member to
        // match, so it selects nothing rather than everything.
        ("source", vec![], vec![]),
        ("period", vec![], vec![]),
    ];

    for (name, values, expected_pairs) in cases {
        let case = format!("{name}={values:?}");
        let reported = described_pairs(&router, &case, &filters(&[(name, &values)])).await;
        let expected = in_store_order(&stored, &expected_pairs);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// One row of the AND table: the filters to send, and the pairs that must come back.
type FilterCombination<'a> = (Vec<(&'a str, &'a [&'a str])>, Vec<(&'a str, &'a str)>);

/// Several filters are joined with an AND: only the subscriptions matching every one are
/// reported.
#[tokio::test]
async fn several_filters_are_joined_with_an_and() {
    let (router, engine) = super::ec2_query::emulator();
    let stored = seed(&router, &engine).await;

    let cases: Vec<FilterCombination<'_>> = vec![
        (
            vec![
                ("source", &["us-east-1"][..]),
                ("destination", &["eu-west-1"][..]),
            ],
            vec![("us-east-1", "eu-west-1")],
        ),
        (
            vec![
                ("source", &["use1-az1"][..]),
                ("metric", &[METRIC][..]),
                ("statistic", &[STATISTIC][..]),
            ],
            vec![("use1-az1", "use1-az2"), ("use1-az1", "eu-west-1")],
        ),
        // Each filter matches something, but no subscription matches both.
        (
            vec![
                ("source", &["us-east-1"][..]),
                ("destination", &["ap-south-1"][..]),
            ],
            vec![],
        ),
    ];

    for (entries, expected_pairs) in cases {
        let case = format!("{entries:?}");
        let reported = described_pairs(&router, &case, &filters(&entries)).await;
        let expected = in_store_order(&stored, &expected_pairs);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// A filter name this operation does not define is refused with EC2's
/// `InvalidParameterValue`, rather than silently reported as an empty set.
#[tokio::test]
async fn an_undefined_filter_name_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&router, &engine).await;

    // Rows: a defined name with the wrong case, the underscore and plural spellings, a
    // member of another resource's describe, the tag filter a subscription has no tags for,
    // and an empty name.
    let cases = [
        "Source",
        "source-region",
        "sources",
        "destination_region",
        "vpc-id",
        "tag:Name",
        "",
    ];
    for name in cases {
        let (status, body) = describe(&router, &filters(&[(name, &["us-east-1"])])).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );
        // The EC2 error envelope is XML, so the message's apostrophes arrive escaped.
        assert!(
            unescaped(&body).contains(&format!("The filter '{name}' is invalid")),
            "case {name:?}: the refusal must name the filter, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Whatever the page size, walking the pages covers every subscription exactly once, in
/// store order, and the last page carries no token.
#[tokio::test]
async fn paging_covers_every_subscription_exactly_once_for_each_page_size() {
    let (router, engine) = super::ec2_query::emulator();
    let stored = seed(&router, &engine).await;

    // Rows: the smallest page that advances, a size that divides the store exactly, one that
    // does not, and the largest the model publishes (`MaxResultsParam`, `range 0..100`).
    for max_results in [1, 2, 4, 100] {
        let case = format!("MaxResults={max_results}");
        let mut params = vec![("MaxResults".to_string(), max_results.to_string())];
        let mut walked: Vec<(String, String)> = Vec::new();
        let mut pages = 0;
        loop {
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = reported_pairs(&body);
            assert!(
                page.len() <= max_results,
                "case {case:?}: a page may not exceed MaxResults, got {page:?}"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= stored.len() + 1,
                "case {case:?}: paging did not terminate, walked {walked:?}"
            );
            match elements(&body, "nextToken").first() {
                Some(token) => {
                    params = vec![
                        ("MaxResults".to_string(), max_results.to_string()),
                        ("NextToken".to_string(), token.clone()),
                    ];
                }
                None => break,
            }
        }
        assert_eq!(
            walked, stored,
            "case {case:?}: expected every subscription exactly once in store order, got \
             {walked:?}"
        );
    }
}

/// A page size is a cut of the FILTERED set, not of the whole store: `MaxResults` and a
/// filter compose.
#[tokio::test]
async fn a_page_is_cut_from_the_filtered_set() {
    let (router, engine) = super::ec2_query::emulator();
    let stored = seed(&router, &engine).await;

    let mut params = filters(&[("destination", &["eu-west-1"])]);
    params.push(("MaxResults".to_string(), "1".to_string()));
    let (status, body) = describe(&router, &params).await;
    assert_eq!(status, 200, "filtered first page: got {status} {body}");
    let first = reported_pairs(&body);
    assert_eq!(
        first,
        in_store_order(&stored, &[("us-east-1", "eu-west-1")]),
        "filtered first page: expected the first subscription to eu-west-1, got {first:?}"
    );
    let token = elements(&body, "nextToken")
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("filtered first page: expected a NextToken in {body}"));

    let mut next = filters(&[("destination", &["eu-west-1"])]);
    next.push(("MaxResults".to_string(), "1".to_string()));
    next.push(("NextToken".to_string(), token));
    let second = described_pairs(&router, "filtered second page", &next).await;
    assert_eq!(
        second,
        in_store_order(&stored, &[("use1-az1", "eu-west-1")]),
        "filtered second page: expected the remaining subscription to eu-west-1, got {second:?}"
    );
}

/// `MaxResults` is accepted at each bound the model publishes and refused past them; its
/// published minimum of zero names no page size, so it reports the whole store rather than
/// an empty page with a token that cannot advance.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    let stored = seed(&router, &engine).await;

    // `MaxResultsParam` is modelled `range 0..100`: both published bounds, and one past
    // each.
    let cases = [("-1", 400), ("0", 200), ("100", 200), ("101", 400)];
    for (max_results, expected_status) in cases {
        let params = vec![("MaxResults".to_string(), max_results.to_string())];
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} with \
             body {body}"
        );
    }

    let params = vec![("MaxResults".to_string(), "0".to_string())];
    let (status, body) = describe(&router, &params).await;
    assert_eq!(status, 200, "MaxResults=0: got {status} with body {body}");
    assert_eq!(
        reported_pairs(&body),
        stored,
        "MaxResults=0: expected the whole store, got {body}"
    );
    assert!(
        elements(&body, "nextToken").is_empty(),
        "MaxResults=0: a page covering the store carries no token, got {body}"
    );
}

/// A `NextToken` this service did not issue is refused with `InvalidPaginationToken`, rather
/// than read as a cursor.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&router, &engine).await;

    // Rows: free text, punctuation, a hex string this service never minted, the bare cursor
    // a caller might guess the token is, and that cursor hex-encoded (the shape the token
    // takes, without the marker and checksum a minted token carries).
    let cursor = format!("us-east-1 eu-west-1 {METRIC} {STATISTIC}");
    let cases = [
        "not-a-token".to_string(),
        "###".to_string(),
        "6161".to_string(),
        cursor.clone(),
        ::hex::encode(&cursor),
        ::hex::encode(format!("ec2-1:0000000000000000:{cursor}")),
    ];
    for token in cases {
        let params = vec![("NextToken".to_string(), token.clone())];
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// What a subscription may name
// ---------------------------------------------------------------------------

/// A `Source`/`Destination` that names neither a Region of the catalog nor an Availability
/// Zone id is refused with `InvalidParameterValue`, by the enable and the disable alike, and
/// nothing is written.
#[tokio::test]
async fn an_endpoint_that_is_neither_a_region_nor_a_zone_is_refused() {
    let (router, engine) = super::ec2_query::emulator();

    // Rows: a Region code that follows the grammar but names no Region, the zone NAME rather
    // than the zone ID the member documents, a zone id of a Region that has no such zone,
    // the upper-case spelling, free text, and the empty value. A Region of another partition
    // is NOT here: every Region the catalog holds is an endpoint (see
    // `an_endpoint_of_another_partition_is_accepted`).
    let cases = [
        "us-east-9",
        "us-east-1a",
        "use1-az9",
        "US-EAST-1",
        "not-a-region",
        "",
    ];
    for endpoint in cases {
        for (member, params) in [
            ("Source", naming(endpoint, "eu-west-1", METRIC, STATISTIC)),
            ("Destination", naming("us-east-1", endpoint, METRIC, STATISTIC)),
        ] {
            let case = format!("{member}={endpoint:?}");
            for (action, (status, body)) in [
                ("enable", enable(&router, &params).await),
                ("disable", disable(&router, &params).await),
            ] {
                assert_eq!(
                    status, 400,
                    "case {action} {case}: expected HTTP 400, got {status} with body {body}"
                );
                // An empty value names no endpoint at all, which EC2 reports as the missing
                // parameter it is; any other unknown value is a value error.
                let expected_code = if endpoint.is_empty() {
                    "MissingParameter"
                } else {
                    "InvalidParameterValue"
                };
                assert_eq!(
                    error_code(&body),
                    expected_code,
                    "case {action} {case}: expected {expected_code}, got {body}"
                );
            }
        }
    }

    assert!(
        engine
            .state::<Ec2State>()
            .lock()
            .network_performance_subscriptions
            .is_empty(),
        "a refused request must write nothing"
    );
}

/// "If you use Availability Zone IDs, the Source and Destination Availability Zones must be
/// in the same Region" (`Source`/`Destination`): two zones of one Region subscribe, two
/// zones of different Regions are refused — while a zone paired with a Region in another
/// Region is fine, the rule applying only when both endpoints are zones.
#[tokio::test]
async fn availability_zones_of_different_regions_cannot_be_subscribed() {
    let (router, engine) = super::ec2_query::emulator();

    // Rows: both zones in one Region, a zone and a Region in different Regions either way
    // round, and both zones in different Regions.
    let cases = [
        (("use1-az1", "use1-az3"), 200),
        (("use1-az1", "eu-west-1"), 200),
        (("eu-west-1", "use1-az1"), 200),
        (("use1-az1", "euw1-az2"), 400),
        (("apne1-az1", "use2-az2"), 400),
    ];
    for ((source, destination), expected_status) in cases {
        let case = format!("{source} -> {destination}");
        let (status, body) = enable(&router, &naming(source, destination, METRIC, STATISTIC)).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "InvalidParameterCombination",
                "case {case:?}: expected InvalidParameterCombination, got {body}"
            );
        }
    }

    let reported = described_pairs(&router, "after the refusals", &[]).await;
    assert_eq!(
        reported,
        stored_pairs(&engine),
        "after the refusals: expected exactly the accepted subscriptions, got {reported:?}"
    );
    assert_eq!(
        reported.len(),
        3,
        "after the refusals: expected the three accepted subscriptions, got {reported:?}"
    );
}

/// A member the subscription is named by that the request leaves out — or leaves empty — is
/// refused with EC2's `MissingParameter`, naming that member, by the enable and the disable
/// alike.
#[tokio::test]
async fn a_member_the_request_leaves_out_is_refused() {
    let (router, engine) = super::ec2_query::emulator();

    // One row per member of the four that name a subscription: the request carries the other
    // three.
    for omitted in ["Source", "Destination", "Metric", "Statistic"] {
        let params: Vec<(String, String)> = naming("us-east-1", "eu-west-1", METRIC, STATISTIC)
            .into_iter()
            .filter(|(name, _)| name != omitted)
            .collect();
        for (action, (status, body)) in [
            ("enable", enable(&router, &params).await),
            ("disable", disable(&router, &params).await),
        ] {
            assert_eq!(
                status, 400,
                "case {action} without {omitted}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                "MissingParameter",
                "case {action} without {omitted}: expected MissingParameter, got {body}"
            );
            assert!(
                body.contains(omitted),
                "case {action} without {omitted}: the refusal must name the member, got {body}"
            );
        }
    }

    assert!(
        engine
            .state::<Ec2State>()
            .lock()
            .network_performance_subscriptions
            .is_empty(),
        "a refused request must write nothing"
    );
}

/// `Metric` outside the modelled `MetricType` catalog, or `Statistic` outside
/// `StatisticType`, names no metric to subscribe to and is refused — at the model boundary,
/// which checks an enum member before the handler runs — so nothing is subscribed.
#[tokio::test]
async fn a_metric_or_statistic_outside_its_catalog_is_refused() {
    let (router, engine) = super::ec2_query::emulator();

    // Rows: the published member itself (accepted), its upper-case spelling, a member of the
    // other enum, a plausible value neither enum publishes, and a `PeriodType` member in the
    // statistic's place.
    let metric_cases = [
        (METRIC, 200),
        ("Aggregate-Latency", 400),
        ("p50", 400),
        ("aggregate-throughput", 400),
    ];
    for (metric, expected_status) in metric_cases {
        let params = naming("us-east-1", "eu-west-1", metric, STATISTIC);
        let (status, body) = enable(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case Metric={metric:?}: expected HTTP {expected_status}, got {status} with body \
             {body}"
        );
        if expected_status == 400 {
            // The refusal is built from the request: it names the value sent and the member
            // it was sent for.
            let message = unescaped(&body);
            assert!(
                message.contains(metric) && message.contains("metric"),
                "case Metric={metric:?}: the refusal must name the value and the member, got \
                 {body}"
            );
        }
    }

    let statistic_cases = [(STATISTIC, 200), ("P50", 400), ("p99", 400), (PERIOD, 400)];
    for (statistic, expected_status) in statistic_cases {
        let params = naming("us-east-1", "ap-south-1", METRIC, statistic);
        let (status, body) = enable(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case Statistic={statistic:?}: expected HTTP {expected_status}, got {status} with \
             body {body}"
        );
        if expected_status == 400 {
            let message = unescaped(&body);
            assert!(
                message.contains(statistic) && message.contains("statistic"),
                "case Statistic={statistic:?}: the refusal must name the value and the member, \
                 got {body}"
            );
        }
    }

    // Only the two accepted rows were written.
    let reported = described_pairs(&router, "after the refusals", &[]).await;
    assert_eq!(
        reported,
        stored_pairs(&engine),
        "after the refusals: expected exactly the accepted subscriptions, got {reported:?}"
    );
    assert_eq!(
        reported.len(),
        2,
        "after the refusals: expected the two accepted subscriptions, got {reported:?}"
    );
}

/// Every Region the catalog holds is an endpoint, whichever partition it belongs to: the
/// emulator answers for whatever Region a request is signed for, so a subscription between
/// two Regions of another partition is as real as one between two commercial Regions — and
/// so is one between two of that Region's zones.
#[tokio::test]
async fn an_endpoint_of_another_partition_is_accepted() {
    let (router, engine) = super::ec2_query::emulator();

    // Rows: two Regions of the China partition, two of GovCloud, and two zones of one
    // GovCloud Region (`us-gov-west-1` abbreviates to the `usgw1` zone-id prefix).
    let cases = [
        ("cn-north-1", "cn-northwest-1"),
        ("us-gov-east-1", "us-gov-west-1"),
        ("usgw1-az1", "usgw1-az2"),
    ];
    for (source, destination) in cases {
        enable_pair(&router, source, destination).await;
    }

    let reported = described_pairs(&router, "other partitions", &[]).await;
    assert_eq!(
        reported,
        stored_pairs(&engine),
        "other partitions: expected the store's contents, got {reported:?}"
    );
    for (source, destination) in cases {
        assert!(
            reported.contains(&(source.to_string(), destination.to_string())),
            "case {source} -> {destination}: expected it to be reported, got {reported:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------
/// `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — on all three
/// operations, and changes nothing: the enable subscribes nothing and the disable removes
/// nothing.
#[tokio::test]
async fn dry_run_reports_the_permitted_outcome_without_changing_anything() {
    let (router, engine) = super::ec2_query::emulator();
    enable_pair(&router, "us-east-1", "eu-west-1").await;
    let before = stored_pairs(&engine);

    let mut dry_enable = naming("ap-northeast-1", "ap-south-1", METRIC, STATISTIC);
    dry_enable.push(("DryRun".to_string(), "true".to_string()));
    let mut dry_disable = naming("us-east-1", "eu-west-1", METRIC, STATISTIC);
    dry_disable.push(("DryRun".to_string(), "true".to_string()));

    let cases: Vec<(&str, (u16, String))> = vec![
        ("enable", enable(&router, &dry_enable).await),
        ("disable", disable(&router, &dry_disable).await),
        (
            "describe",
            describe(&router, &[("DryRun".to_string(), "true".to_string())]).await,
        ),
    ];
    for (action, (status, body)) in cases {
        assert_eq!(
            status, 412,
            "case {action} DryRun=true: expected HTTP 412, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {action} DryRun=true: expected DryRunOperation, got {body}"
        );
    }

    let after = described_pairs(&router, "after the dry runs", &[]).await;
    assert_eq!(
        after, before,
        "after the dry runs: expected the store unchanged, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// The store's ceiling
// ---------------------------------------------------------------------------

/// The store holds at most `MAX_NETWORK_PERFORMANCE_SUBSCRIPTIONS` subscriptions: the
/// enable that would be one too many is refused with EC2's quota code and stores nothing,
/// while an enable naming one already held adds no entry and is admitted at the ceiling.
#[tokio::test]
async fn an_enable_past_the_stores_ceiling_is_refused() {
    let (router, engine) = super::ec2_query::emulator();

    // Distinct Region-to-Region pairs out of the catalog the endpoints are held to, which
    // has more ordered pairs than the ceiling.
    let regions = ec2::state::commercial_regions();
    let pairs: Vec<(&str, &str)> = regions
        .iter()
        .flat_map(|source| {
            regions
                .iter()
                .filter(move |destination| *destination != source)
                .map(move |destination| (*source, *destination))
        })
        .take(ec2::state::MAX_NETWORK_PERFORMANCE_SUBSCRIPTIONS + 1)
        .collect();
    assert_eq!(
        pairs.len(),
        ec2::state::MAX_NETWORK_PERFORMANCE_SUBSCRIPTIONS + 1,
        "the catalog must offer more ordered pairs than the ceiling, got {}",
        pairs.len()
    );

    let (filling, past) = pairs.split_at(ec2::state::MAX_NETWORK_PERFORMANCE_SUBSCRIPTIONS);
    for (source, destination) in filling {
        enable_pair(&router, source, destination).await;
    }
    assert_eq!(
        engine
            .state::<Ec2State>()
            .lock()
            .network_performance_subscriptions
            .len(),
        ec2::state::MAX_NETWORK_PERFORMANCE_SUBSCRIPTIONS,
        "filling the store: expected it to hold exactly the ceiling"
    );

    // One already held: no new entry, so it is still accepted at the ceiling.
    let (held_source, held_destination) = filling[0];
    enable_pair(&router, held_source, held_destination).await;

    // One more subscription does not fit.
    let (source, destination) = past[0];
    let case = format!("{source} -> {destination}");
    let (status, body) = enable(&router, &naming(source, destination, METRIC, STATISTIC)).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "SubscriptionLimitExceeded",
        "case {case:?}: expected SubscriptionLimitExceeded, got {body}"
    );
    assert_eq!(
        engine
            .state::<Ec2State>()
            .lock()
            .network_performance_subscriptions
            .len(),
        ec2::state::MAX_NETWORK_PERFORMANCE_SUBSCRIPTIONS,
        "case {case:?}: a refused enable must store nothing"
    );
}
