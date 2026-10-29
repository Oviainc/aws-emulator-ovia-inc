// SPDX-License-Identifier: CC0-1.0

//! Reporting the blocklist listings of the account's addresses, end to end over HTTP.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with this
//! service registered — so routing, the repeated `@httpQuery` list binding, the handler
//! and serialization are all exercised, and writes the engine's own state where a
//! listing has to have been observed before a report can name it (a blocklist operator
//! lists an address and SES observes it out of band, so no request mints a listing).
//!
//! The rules these cases pin:
//!
//! * `GetBlacklistReports` REPORTS, for each blacklist item the request names, the
//!   listings the account's records hold for that item — each listing's blocklist name,
//!   when the listing happened and the maintainer's description — ordered by when the
//!   listing happened and then by the blocklist's name, so one item reads the same way
//!   whatever order the observations arrived in;
//! * an item with nothing recorded for it is ABSENT from the map rather than present
//!   and empty, and is never a refusal — so a request naming only such items answers an
//!   empty report with HTTP 200;
//! * the report is keyed by the item name, so naming one item twice reports it once and
//!   the order the request lists its items in does not change the answer;
//! * reading changes nothing, so a second report answers as the first did and the store
//!   is left exactly as a later caller finds it.
//!
//! The tables below span what the rules generalise over: the number of listings an item
//! carries (one, two at the same instant, three, four), the order the observations
//! arrived in, the instant of a listing (the epoch and a recent second), the form of the
//! address, and every leg of "nothing to report" (a dedicated IP of the account that no
//! blocklist lists, an address the account does not hold, and values that are no IPv4
//! address at all).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::Value;
use service_com_amazonaws_sesv2_simpleemailservice_v2::state::{
    BlacklistListingRecord, ComAmazonawsSesv2SimpleemailserviceV2State, DedicatedIpRecord,
    WARMUP_COMPLETE_PERCENTAGE,
};
use service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One answered request: the status, the error shape when there was one, and the
/// parsed body.
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// The modelled error code the response names, or an empty string on success.
    ///
    /// restJson1 resolves the error shape from `x-amzn-errortype`; the JSON body's
    /// `__type` carries the same name, so either is read.
    fn error_code(&self) -> String {
        let raw = self
            .error_type
            .as_deref()
            .or_else(|| self.body.get("__type").and_then(Value::as_str))
            .unwrap_or_default();
        let name = raw.split('#').next_back().unwrap_or(raw);
        name.split(':').next().unwrap_or(name).to_string()
    }

    /// The `BlacklistReport` map the response reports, as the wire document.
    fn report(&self) -> Value {
        self.body
            .get("BlacklistReport")
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// The item names the report carries, sorted: a map has no order on the wire, so a
    /// case that asserts WHICH items were reported compares this.
    fn reported_items(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .report()
            .as_object()
            .map(|members| members.keys().cloned().collect())
            .unwrap_or_default();
        names.sort();
        names
    }

    /// The listings the report carries for `item_name`, in the order it reported them,
    /// as `(RblName, ListingTime in seconds, Description)`.
    fn listings_of(&self, item_name: &str) -> Vec<(String, f64, String)> {
        self.report()
            .get(item_name)
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| {
                        (
                            entry
                                .get("RblName")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            entry
                                .get("ListingTime")
                                .and_then(Value::as_f64)
                                .unwrap_or(f64::NAN),
                            entry
                                .get("Description")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The emulated service behind its router, plus the shared engine so a case can record
/// an observed listing the way SES's out-of-band monitoring does.
struct Ses {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Ses {
    fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These cases exercise handler behaviour, not SigV4.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// `GetBlacklistReports`' route, from the operation's HTTP binding
    /// (`smithy_query(query="operation",operation="GetBlacklistReports")`:
    /// `GET /v2/email/deliverability-dashboard/blacklist-report`, with
    /// `BlacklistItemNames` an `@httpQuery` list — so each name travels as its own
    /// repetition of the parameter).
    async fn get_blacklist_reports(&self, item_names: &[&str]) -> Reply {
        let region = "us-east-1";
        let host = format!("email.{region}.amazonaws.com");
        let query = item_names
            .iter()
            .map(|name| format!("BlacklistItemNames={}", encode(name)))
            .collect::<Vec<_>>()
            .join("&");
        let path = "/v2/email/deliverability-dashboard/blacklist-report";
        let uri = if query.is_empty() {
            format!("https://{host}{path}")
        } else {
            format!("https://{host}{path}?{query}")
        };
        let request = http::Request::builder()
            .method("GET")
            .uri(uri)
            .header("host", host)
            .header("content-type", "application/json")
            // The SigV4 credential scope is what routes the request to the `ses`
            // signing name; the signature itself is not checked here.
            .header(
                "authorization",
                format!("AWS4-HMAC-SHA256 Credential=AKIAEMULATOR0000000/20260101/{region}/ses/aws4_request, SignedHeaders=host, Signature=00"),
            )
            .body(bytes::Bytes::new())
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }

    /// Record the listings observed for one blacklist item, in the order they arrived:
    /// no SES request creates one, so a case that reads a report back writes the records
    /// the observation would have left.
    fn observe(&self, item_name: &str, listings: &[Listing]) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let records = listings
            .iter()
            .map(|listing| {
                BlacklistListingRecord::new(
                    listing.rbl_name,
                    at_second(listing.listing_second),
                    listing.description,
                )
            })
            .collect();
        state
            .lock()
            .blacklist_listings
            .insert(item_name.to_string(), records);
    }

    /// Put a dedicated IP in the account, so a case can show that an address the account
    /// holds and no blocklist lists is reported with nothing rather than with an empty
    /// list.
    fn allocate(&self, ip: &str, pool_name: Option<&str>, warmup_percentage: i32) {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        state.lock().dedicated_ips.insert(
            ip.to_string(),
            DedicatedIpRecord::new(ip, pool_name.map(str::to_string), warmup_percentage),
        );
    }

    /// The listings the store holds, as a comparable snapshot: what an assertion about a
    /// report not changing anything compares against.
    fn stored(&self) -> Vec<(String, Vec<StoredListing>)> {
        let state = self
            .engine
            .state::<ComAmazonawsSesv2SimpleemailserviceV2State>();
        let store = state.lock();
        store
            .blacklist_listings
            .iter()
            .map(|(item_name, listings)| {
                (
                    item_name.clone(),
                    listings
                        .iter()
                        .map(|listing| {
                            (
                                listing.rbl_name.clone(),
                                listing
                                    .listing_time
                                    .duration_since(SystemTime::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_secs(),
                                listing.description.clone(),
                            )
                        })
                        .collect(),
                )
            })
            .collect()
    }
}

/// Percent-encode a query value the few ways these cases need: every name here is made
/// of unreserved characters apart from the separators a value may not carry raw.
fn encode(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            ':' => "%3A".to_string(),
            '&' => "%26".to_string(),
            '=' => "%3D".to_string(),
            ' ' => "%20".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// The instant `second` seconds after the epoch: a listing's time is an instant the
/// observation carried, so a case states it as one.
fn at_second(second: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(second)
}

/// One listing as the store holds it, for a snapshot a case can compare: the
/// blocklist's name, the second since the epoch the listing happened at, and the
/// maintainer's description.
type StoredListing = (String, u64, String);

/// One observed listing of one item on one blocklist.
struct Listing {
    rbl_name: &'static str,
    listing_second: u64,
    description: &'static str,
}

/// One row of the report table: the listings recorded for one blacklist item, in the
/// order they were observed, and the order a report of that item must answer in.
struct ReportCase {
    case: &'static str,
    item_name: &'static str,
    listings: Vec<Listing>,
    /// The blocklist names, in the order the report must list them.
    expected_order: Vec<&'static str>,
}

/// Every row of the report table: one blacklist item per row, spanning the number of
/// listings, the order the observations arrived in, the instant of a listing and the
/// form of the address.
fn report_cases() -> Vec<ReportCase> {
    vec![
        ReportCase {
            case: "one listing",
            item_name: "198.51.100.10",
            listings: vec![Listing {
                rbl_name: "zen.example-rbl.org",
                listing_second: 1_700_000_000,
                description: "Listed after mail reached a spam trap.",
            }],
            expected_order: vec!["zen.example-rbl.org"],
        },
        ReportCase {
            case: "three listings, observed newest first",
            item_name: "198.51.100.11",
            listings: vec![
                Listing {
                    rbl_name: "late.example-rbl.org",
                    listing_second: 1_700_000_300,
                    description: "Listed after a complaint.",
                },
                Listing {
                    rbl_name: "early.example-rbl.org",
                    listing_second: 1_700_000_100,
                    description: "Listed after a bounce burst.",
                },
                Listing {
                    rbl_name: "middle.example-rbl.org",
                    listing_second: 1_700_000_200,
                    description: "Listed by a policy block.",
                },
            ],
            expected_order: vec![
                "early.example-rbl.org",
                "middle.example-rbl.org",
                "late.example-rbl.org",
            ],
        },
        ReportCase {
            case: "two listings at the same instant, tied on time",
            item_name: "192.0.2.3",
            listings: vec![
                Listing {
                    rbl_name: "zz.example-rbl.org",
                    listing_second: 1_700_000_000,
                    description: "Listed by a volume rule.",
                },
                Listing {
                    rbl_name: "aa.example-rbl.org",
                    listing_second: 1_700_000_000,
                    description: "Listed by a content rule.",
                },
            ],
            expected_order: vec!["aa.example-rbl.org", "zz.example-rbl.org"],
        },
        ReportCase {
            case: "a listing at the epoch, on the highest address form",
            item_name: "255.255.255.255",
            listings: vec![Listing {
                rbl_name: "edge.example-rbl.org",
                listing_second: 0,
                description: "Listed before the record begins.",
            }],
            expected_order: vec!["edge.example-rbl.org"],
        },
        ReportCase {
            case: "four listings, observed out of order",
            item_name: "10.0.0.7",
            listings: vec![
                Listing {
                    rbl_name: "d.example-rbl.org",
                    listing_second: 1_699_000_040,
                    description: "Listed by a reputation rule.",
                },
                Listing {
                    rbl_name: "a.example-rbl.org",
                    listing_second: 1_699_000_010,
                    description: "Listed by a trap hit.",
                },
                Listing {
                    rbl_name: "c.example-rbl.org",
                    listing_second: 1_699_000_030,
                    description: "Listed by a complaint rate.",
                },
                Listing {
                    rbl_name: "b.example-rbl.org",
                    listing_second: 1_699_000_020,
                    description: "Listed by a bounce rate.",
                },
            ],
            expected_order: vec![
                "a.example-rbl.org",
                "b.example-rbl.org",
                "c.example-rbl.org",
                "d.example-rbl.org",
            ],
        },
    ]
}

/// The listings a report of `case` must answer with, in order.
fn expected_listings(case: &ReportCase) -> Vec<(String, f64, String)> {
    case.expected_order
        .iter()
        .map(|rbl_name| {
            let listing = case
                .listings
                .iter()
                .find(|listing| listing.rbl_name == *rbl_name)
                .expect("the expected order names a recorded listing");
            (
                listing.rbl_name.to_string(),
                listing.listing_second as f64,
                listing.description.to_string(),
            )
        })
        .collect()
}

/// Record every row of the report table in one account, so each report has to pick out
/// the item the request names rather than the only one there is.
fn seeded() -> (Ses, Vec<ReportCase>) {
    let ses = Ses::new();
    let cases = report_cases();
    for case in &cases {
        ses.observe(case.item_name, &case.listings);
    }
    (ses, cases)
}

#[tokio::test]
async fn a_report_answers_the_listings_recorded_for_the_item_it_names() {
    let (ses, cases) = seeded();

    for case in &cases {
        let reply = ses.get_blacklist_reports(&[case.item_name]).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200, got {} {:?}",
            case.case, reply.status, reply.body
        );
        assert_eq!(
            reply.reported_items(),
            vec![case.item_name.to_string()],
            "case {:?}: expected the report to name {:?}, got {:?}",
            case.case,
            case.item_name,
            reply.reported_items()
        );
        assert_eq!(
            reply.listings_of(case.item_name),
            expected_listings(case),
            "case {:?}: expected {:?}, got {:?}",
            case.case,
            expected_listings(case),
            reply.listings_of(case.item_name)
        );
    }
}

#[tokio::test]
async fn one_report_answers_every_item_it_names_at_once() {
    let (ses, cases) = seeded();

    let names: Vec<&str> = cases.iter().map(|case| case.item_name).collect();
    let reply = ses.get_blacklist_reports(&names).await;
    assert_eq!(
        reply.status, 200,
        "case \"every recorded item in one request\": expected 200, got {} {:?}",
        reply.status, reply.body
    );
    let mut expected_names: Vec<String> = names.iter().map(|name| name.to_string()).collect();
    expected_names.sort();
    assert_eq!(
        reply.reported_items(),
        expected_names,
        "case \"every recorded item in one request\": expected {expected_names:?}, got {:?}",
        reply.reported_items()
    );
    for case in &cases {
        assert_eq!(
            reply.listings_of(case.item_name),
            expected_listings(case),
            "case {:?} within one request: expected {:?}, got {:?}",
            case.case,
            expected_listings(case),
            reply.listings_of(case.item_name)
        );
    }
}

#[tokio::test]
async fn an_item_with_nothing_recorded_is_absent_from_the_report() {
    let (ses, cases) = seeded();
    // An address the account holds as a dedicated IP, which no blocklist lists: holding
    // the address is not what puts it in a report, a listing is.
    ses.allocate("203.0.113.5", Some("marketing-pool"), WARMUP_COMPLETE_PERCENTAGE);

    // The rows differ along WHY there is nothing to report: a dedicated IP of the
    // account that no blocklist lists, addresses the account does not hold at all, and
    // values that are no IPv4 address — none of which is a refusal.
    let nothing_recorded = [
        ("a dedicated IP of the account no blocklist lists", "203.0.113.5"),
        ("an address the account does not hold", "192.0.2.77"),
        ("a neighbour of a listed address", "198.51.100.12"),
        ("the all-zeroes address", "0.0.0.0"),
        ("a host name", "smtp.example.com"),
        ("an IPv6 address", "2001:db8::1"),
        ("a value that is no address at all", "not-an-address"),
    ];

    for (case, item_name) in nothing_recorded {
        let reply = ses.get_blacklist_reports(&[item_name]).await;
        assert_eq!(
            (reply.status, reply.error_code().as_str()),
            (200, ""),
            "case {case:?}: expected 200 with no error, got {} {} {:?}",
            reply.status,
            reply.error_code(),
            reply.body
        );
        assert_eq!(
            reply.report(),
            Value::Object(serde_json::Map::new()),
            "case {case:?}: expected an empty report, got {:?}",
            reply.report()
        );
    }

    // And a request that names them beside a recorded item reports only that item.
    let listed = cases[0].item_name;
    let mut mixed: Vec<&str> = nothing_recorded.iter().map(|(_, name)| *name).collect();
    mixed.insert(3, listed);
    let reply = ses.get_blacklist_reports(&mixed).await;
    assert_eq!(
        reply.reported_items(),
        vec![listed.to_string()],
        "case \"a recorded item among items with nothing recorded\": expected only {listed:?}, got {:?}",
        reply.reported_items()
    );
    assert_eq!(
        reply.listings_of(listed),
        expected_listings(&cases[0]),
        "case \"a recorded item among items with nothing recorded\": expected {:?}, got {:?}",
        expected_listings(&cases[0]),
        reply.listings_of(listed)
    );
}

#[tokio::test]
async fn the_report_is_keyed_by_item_name_whatever_order_or_repetition_the_request_uses() {
    let (ses, cases) = seeded();
    let first = cases[0].item_name;
    let second = cases[1].item_name;

    // The rows differ along the SHAPE of the request's name list: the two names once
    // each, the same two reversed, one of them repeated, and one repeated beside a name
    // with nothing recorded.
    let requests: [(&str, Vec<&str>); 4] = [
        ("two names once each", vec![first, second]),
        ("the same two names reversed", vec![second, first]),
        ("one name repeated", vec![first, second, first]),
        (
            "one name repeated beside an item with nothing recorded",
            vec![second, "192.0.2.77", second],
        ),
    ];

    for (case, names) in requests {
        let reply = ses.get_blacklist_reports(&names).await;
        let mut expected: Vec<String> = names
            .iter()
            .filter(|name| **name == first || **name == second)
            .map(|name| name.to_string())
            .collect();
        expected.sort();
        expected.dedup();
        assert_eq!(
            reply.reported_items(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            reply.reported_items()
        );
        for item in expected {
            let recorded = cases
                .iter()
                .find(|candidate| candidate.item_name == item)
                .expect("the expected item is a recorded one");
            assert_eq!(
                reply.listings_of(&item),
                expected_listings(recorded),
                "case {case:?}: expected {item:?} to report {:?}, got {:?}",
                expected_listings(recorded),
                reply.listings_of(&item)
            );
        }
    }
}

#[tokio::test]
async fn reporting_changes_nothing_a_later_caller_sees() {
    let (ses, cases) = seeded();
    let before = ses.stored();

    // Report every recorded item twice, and one item with nothing recorded: a report
    // must not add, drop or reorder a listing, and must not mint a record for a name it
    // was asked about.
    for case in &cases {
        let first = ses.get_blacklist_reports(&[case.item_name]).await;
        let second = ses.get_blacklist_reports(&[case.item_name]).await;
        assert_eq!(
            first.listings_of(case.item_name),
            second.listings_of(case.item_name),
            "case {:?}: expected the second report to answer as the first did ({:?}), got {:?}",
            case.case,
            first.listings_of(case.item_name),
            second.listings_of(case.item_name)
        );
    }
    let unknown = ses.get_blacklist_reports(&["192.0.2.77"]).await;
    assert_eq!(
        unknown.report(),
        Value::Object(serde_json::Map::new()),
        "case \"an item with nothing recorded\": expected an empty report, got {:?}",
        unknown.report()
    );

    assert_eq!(
        ses.stored(),
        before,
        "case \"the store after the reports\": expected {before:?}, got {:?}",
        ses.stored()
    );
}
