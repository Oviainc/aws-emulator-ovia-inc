// SPDX-License-Identifier: CC0-1.0

//! `GetLoadBalancers` — the paged listing of the load balancers an account holds
//! — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed.
//!
//! The rule under test: a listing reports every record the store holds of that
//! resource type in the Region the request is made in — and nothing from another
//! Region — in the store's own name order, each entry exactly as the single-record
//! read reports it, carried one page at a time, with `nextPageToken` naming the
//! first record of the next page while records remain and ABSENT on the last
//! page.
//!
//! The rows span what that rule generalises over: how many records a Region holds
//! (none, one, exactly a page, one past a page, the documented per-Region cap of
//! ten), the creation order against the reported order, which Region the request
//! is made in (including a request that names none and so takes the service's
//! default Region), and resuming from a token the service itself handed out.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::instance_lifecycle::Emulator;
use crate::paging::{expected_page_sizes, names, walk_pages};

/// How many load balancers one page carries
/// (`crate::state::LOAD_BALANCER_PAGE_SIZE`).
const PAGE_SIZE: usize = 5;

/// The documented per-Region cap (`CreateLoadBalancer`: "You can create up to 10
/// load balancers per AWS Region in your account").
const MAX_LOAD_BALANCERS_PER_REGION: usize = 10;

/// The Region a request that carries no credential scope is served in
/// (`crate::state::DEFAULT_REGION`), which is also Lightsail's own default
/// endpoint Region.
const DEFAULT_REGION: &str = "us-east-1";

/// The stored load balancer `name` in `region`, as `GetLoadBalancer` reports it.
async fn read(emulator: &Emulator, region: &str, name: &str) -> Value {
    emulator
        .ok_at(
            None,
            Some(region),
            "GetLoadBalancer",
            json!({"loadBalancerName": name}),
        )
        .await["loadBalancer"]
        .clone()
}

/// One `GetLoadBalancers` page in `region` (or in no named Region at all),
/// resuming from `page_token` when one is given.
async fn page(emulator: &Emulator, region: Option<&str>, page_token: Option<&str>) -> Value {
    let body = match page_token {
        Some(token) => json!({"pageToken": token}),
        None => json!({}),
    };
    emulator.ok_at(None, region, "GetLoadBalancers", body).await
}

/// Every page of this listing, through the shared checked paging walk every
/// listing suite in this test target uses.
async fn walk(emulator: &Emulator, case: &str, region: Option<&str>) -> (Vec<Value>, Vec<usize>) {
    walk_pages(
        emulator,
        case,
        region,
        "GetLoadBalancers",
        "loadBalancers",
        PAGE_SIZE,
    )
    .await
}

/// Poll `GetLoadBalancer` until `name` reports `active`.
///
/// The transition is owed by the clock, not by the poll: waiting for it here
/// means the listing and the single-record read are compared in the SAME
/// lifecycle state, so a difference between them is a difference in what the two
/// operations report, never a transition that landed between the two calls.
async fn wait_for_active(emulator: &Emulator, region: &str, name: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let balancer = read(emulator, region, name).await;
        if balancer["state"].as_str() == Some("active") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case waiting for 'active' on {name}: expected it within 5s, got {:?}",
            balancer["state"]
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// ---------------------------------------------------------------------------
// The listing table
// ---------------------------------------------------------------------------

/// One Region and the load balancers created in it, IN CREATION ORDER — which
/// each row deliberately keeps different from name order, so a listing that
/// merely echoed the creation sequence would fail.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    region: &'static str,
    created: &'static [&'static str],
}

/// The table. Every name is globally unique across the rows, so each Region's
/// listing reporting exactly its own names is also the proof that a listing
/// never reaches into another Region.
const ROWS: &[Row] = &[
    Row {
        case: "a Region holding nothing",
        region: "ap-southeast-2",
        created: &[],
    },
    Row {
        case: "a Region holding one",
        region: "eu-central-1",
        created: &["solo-lb"],
    },
    Row {
        case: "exactly one page",
        region: "us-east-1",
        created: &[
            "east-sierra-lb",
            "east-alpha-lb",
            "east-romeo-lb",
            "east-charlie-lb",
            "east-mike-lb",
        ],
    },
    Row {
        case: "one past a page",
        region: "us-west-2",
        created: &[
            "west-zulu-lb",
            "west-delta-lb",
            "west-yankee-lb",
            "west-bravo-lb",
            "west-tango-lb",
            "west-foxtrot-lb",
        ],
    },
    Row {
        case: "the documented per-Region cap",
        region: "eu-west-1",
        created: &[
            "ew-9-lb", "ew-3-lb", "ew-7-lb", "ew-1-lb", "ew-5-lb", "ew-0-lb", "ew-8-lb",
            "ew-2-lb", "ew-6-lb", "ew-4-lb",
        ],
    },
];

#[tokio::test(flavor = "current_thread")]
async fn get_load_balancers_reports_this_regions_records_in_name_order() {
    let emulator = Emulator::new();

    // Create every row's load balancers first, so each Region's listing is read
    // from a store that holds other Regions' records too.
    for row in ROWS {
        assert!(
            row.created.len() <= MAX_LOAD_BALANCERS_PER_REGION,
            "case {}: the table must stay inside the documented per-Region cap, got {}",
            row.case,
            row.created.len()
        );
        for (index, name) in row.created.iter().enumerate() {
            // The instance port differs per record, so two entries of one page
            // are told apart by what the caller asked for, not only by name.
            emulator
                .ok_at(
                    None,
                    Some(row.region),
                    "CreateLoadBalancer",
                    json!({
                        "loadBalancerName": name,
                        "instancePort": 8000 + i64::try_from(index).unwrap_or(0),
                    }),
                )
                .await;
        }
    }
    for row in ROWS {
        for name in row.created {
            wait_for_active(&emulator, row.region, name).await;
        }
    }

    for row in ROWS {
        let case = row.case;
        let (records, page_sizes) = walk(&emulator, case, Some(row.region)).await;

        // The listing reports this Region's records, in the store's name order.
        let reported: Vec<String> = names(&records);
        let mut expected: Vec<String> = row.created.iter().map(|name| name.to_string()).collect();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case}: expected this Region's names in name order, got {reported:?}"
        );

        // Each record is reported once; a page boundary neither repeats nor
        // skips one.
        let distinct: BTreeSet<&String> = reported.iter().collect();
        assert_eq!(
            distinct.len(),
            reported.len(),
            "case {case}: expected every record once, got {reported:?}"
        );

        // The pages are full until the remainder.
        assert_eq!(
            page_sizes,
            expected_page_sizes(row.created.len(), PAGE_SIZE),
            "case {case}: expected full pages until the remainder, got {page_sizes:?}"
        );

        // Every listed record is exactly what the single-record read reports.
        for record in &records {
            let name = record["name"].as_str().unwrap_or_default().to_string();
            let single = read(&emulator, row.region, &name).await;
            assert_eq!(
                record, &single,
                "case {case}: the listing of {name} must match GetLoadBalancer, \
                 got {record:?}"
            );
        }
    }
}

/// A listing is a READ: it reports the same records, with the same
/// server-assigned identities, however often it is called.
#[tokio::test(flavor = "current_thread")]
async fn listing_twice_reports_the_same_records() {
    let emulator = Emulator::new();
    for name in ["repeat-a-lb", "repeat-b-lb", "repeat-c-lb"] {
        emulator
            .ok_at(
                None,
                Some("us-west-1"),
                "CreateLoadBalancer",
                json!({"loadBalancerName": name, "instancePort": 80}),
            )
            .await;
    }
    for name in ["repeat-a-lb", "repeat-b-lb", "repeat-c-lb"] {
        wait_for_active(&emulator, "us-west-1", name).await;
    }

    let (first, first_sizes) = walk(&emulator, "first listing", Some("us-west-1")).await;
    let (second, second_sizes) = walk(&emulator, "second listing", Some("us-west-1")).await;
    assert_eq!(
        first, second,
        "case repeated listing: a read must not change what a later caller sees, \
         got {second:?}"
    );
    assert_eq!(
        first_sizes, second_sizes,
        "case repeated listing: expected the same paging, got {second_sizes:?}"
    );
}

/// A request that carries no credential scope is served in the service's default
/// Region, so it lists that Region's records — and only those.
#[tokio::test(flavor = "current_thread")]
async fn an_unscoped_request_lists_the_default_region() {
    let emulator = Emulator::new();
    emulator
        .ok_at(
            None,
            Some(DEFAULT_REGION),
            "CreateLoadBalancer",
            json!({"loadBalancerName": "default-region-lb", "instancePort": 80}),
        )
        .await;
    emulator
        .ok_at(
            None,
            Some("eu-west-2"),
            "CreateLoadBalancer",
            json!({"loadBalancerName": "elsewhere-lb", "instancePort": 80}),
        )
        .await;

    let (records, _) = walk(&emulator, "no credential scope", None).await;
    let reported: Vec<&str> = records
        .iter()
        .map(|balancer| balancer["name"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        reported,
        vec!["default-region-lb"],
        "case no credential scope: expected the default Region's records, got {reported:?}"
    );
}

/// An empty listing is an empty LIST with no token, never an absent member: the
/// caller asked what it holds and the answer is "nothing".
#[tokio::test(flavor = "current_thread")]
async fn an_empty_region_reports_an_empty_list_and_no_token() {
    let emulator = Emulator::new();
    for region in ["us-east-2", "ap-northeast-1", "ca-central-1"] {
        let page = page(&emulator, Some(region), None).await;
        assert_eq!(
            page["loadBalancers"].as_array().map(Vec::len),
            Some(0),
            "case {region}: expected an empty loadBalancers array, got {page:?}"
        );
        assert!(
            page["nextPageToken"].is_null(),
            "case {region}: a last page carries no nextPageToken, got {page:?}"
        );
    }
}
