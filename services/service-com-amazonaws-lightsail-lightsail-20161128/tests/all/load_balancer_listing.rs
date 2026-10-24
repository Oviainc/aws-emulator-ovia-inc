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
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire
/// protocol").
const TARGET_PREFIX: &str = "Lightsail_20161128";

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

/// A walk over the pages of one listing cannot need more pages than this: the
/// per-Region cap bounds how many records exist, so anything past it means the
/// walk is not advancing.
const MAX_PAGES: usize = MAX_LOAD_BALANCERS_PER_REGION + 2;

/// One emulator with one Lightsail service mounted on one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing; routing is by `X-Amz-Target`, and the
        // `Authorization` header below carries only the credential SCOPE, which
        // is what names the Region a request is made in.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_lightsail_lightsail_20161128::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `operation` with `body`, in `region` when one is given, answering its
    /// HTTP status and parsed body.
    async fn call(&self, region: Option<&str>, operation: &str, body: Value) -> (u16, Value) {
        let mut builder = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "127.0.0.1:9297")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("content-type", "application/x-amz-json-1.1");
        if let Some(region) = region {
            builder = builder.header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261022/{region}/lightsail/\
                     aws4_request, SignedHeaders=host, Signature=not-verified"
                ),
            );
        }
        let request = builder
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call `operation` in `region` (or in none at all) and require HTTP 200,
    /// answering the body.
    async fn ok(&self, region: Option<&str>, operation: &str, body: Value) -> Value {
        let (status, parsed) = self.call(region, operation, body).await;
        assert_eq!(
            status, 200,
            "case {operation} in {region:?}: expected HTTP 200, got {status} {parsed:?}"
        );
        parsed
    }

    /// The stored load balancer `name` in `region`, as `GetLoadBalancer` reports
    /// it.
    async fn read(&self, region: &str, name: &str) -> Value {
        self.ok(
            Some(region),
            "GetLoadBalancer",
            json!({"loadBalancerName": name}),
        )
        .await["loadBalancer"]
            .clone()
    }

    /// One `GetLoadBalancers` page in `region` (or in no named Region at all),
    /// resuming from `page_token` when one is given.
    async fn page(&self, region: Option<&str>, page_token: Option<&str>) -> Value {
        let body = match page_token {
            Some(token) => json!({"pageToken": token}),
            None => json!({}),
        };
        self.ok(region, "GetLoadBalancers", body).await
    }
}

/// The names a page reports, in the order it reports them.
fn names(page: &Value) -> Vec<String> {
    page["loadBalancers"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a loadBalancers array, got {page:?}"))
        .iter()
        .map(|balancer| {
            balancer["name"]
                .as_str()
                .unwrap_or_else(|| panic!("expected every entry to carry a name, got {page:?}"))
                .to_string()
        })
        .collect()
}

/// Walk every page of `region`'s listing, answering the records in the order
/// reported and the size of each page.
///
/// Checks the paging contract as it goes: no page carries more than `PAGE_SIZE`
/// records, a `nextPageToken` names the FIRST record of the page after it, and
/// the walk terminates because the token is absent rather than because it ran
/// out of patience.
async fn walk(emulator: &Emulator, case: &str, region: Option<&str>) -> (Vec<Value>, Vec<usize>) {
    let mut records = Vec::new();
    let mut page_sizes = Vec::new();
    let mut token: Option<String> = None;
    for page_index in 0..MAX_PAGES {
        let page = emulator.page(region, token.as_deref()).await;
        let page_names = names(&page);
        assert!(
            page_names.len() <= PAGE_SIZE,
            "case {case}: page {page_index} carries more than the {PAGE_SIZE} of a page, \
             got {page_names:?}"
        );
        if let Some(token) = &token {
            assert_eq!(
                page_names.first().map(String::as_str),
                Some(token.as_str()),
                "case {case}: a page token names the first record of the page after it, \
                 got {page_names:?}"
            );
        }
        page_sizes.push(page_names.len());
        records.extend(
            page["loadBalancers"]
                .as_array()
                .expect("the page carries a loadBalancers array")
                .iter()
                .cloned(),
        );
        match page["nextPageToken"].as_str() {
            Some(next) => {
                assert_eq!(
                    page_names.len(),
                    PAGE_SIZE,
                    "case {case}: a page that is not the last one is full, got {page_names:?}"
                );
                token = Some(next.to_string());
            }
            None => return (records, page_sizes),
        }
    }
    panic!("case {case}: the walk did not terminate within {MAX_PAGES} pages");
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
        let balancer = emulator.read(region, name).await;
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

/// The page sizes a listing of `total` records must report: full pages until the
/// remainder, and always at least one page — an empty Region still answers once.
fn expected_page_sizes(total: usize) -> Vec<usize> {
    if total == 0 {
        return vec![0];
    }
    let mut sizes = Vec::new();
    let mut left = total;
    while left > 0 {
        let page = left.min(PAGE_SIZE);
        sizes.push(page);
        left -= page;
    }
    sizes
}

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
                .ok(
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
        let reported: Vec<String> = records
            .iter()
            .map(|balancer| {
                balancer["name"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case}: expected a name, got {balancer:?}"))
                    .to_string()
            })
            .collect();
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
            expected_page_sizes(row.created.len()),
            "case {case}: expected full pages until the remainder, got {page_sizes:?}"
        );

        // Every listed record is exactly what the single-record read reports.
        for record in &records {
            let name = record["name"].as_str().unwrap_or_default().to_string();
            let single = emulator.read(row.region, &name).await;
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
            .ok(
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
        .ok(
            Some(DEFAULT_REGION),
            "CreateLoadBalancer",
            json!({"loadBalancerName": "default-region-lb", "instancePort": 80}),
        )
        .await;
    emulator
        .ok(
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
        let page = emulator.page(Some(region), None).await;
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
