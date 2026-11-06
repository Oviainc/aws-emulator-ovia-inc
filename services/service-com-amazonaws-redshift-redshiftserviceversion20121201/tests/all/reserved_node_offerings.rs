// SPDX-License-Identifier: CC0-1.0

//! The reserved-node offering catalog `DescribeReservedNodeOfferings` reports,
//! end to end over the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds, reusing the harness the cluster-lifecycle suite
//! owns rather than keeping a second copy of it.
//!
//! The suite asserts the RULE rather than one recorded request: the catalog is
//! the product of the offered node types, the published terms and the published
//! payment options; each offering charges the share of the term its payment
//! option takes upfront as the `FixedPrice` and whatever is left as ONE hourly
//! recurring charge, charging nothing per running hour; every offering of this
//! catalog is a `Regular` one; `ReservedNodeOfferingId`
//! selects the one offering it names and an id the catalog does not publish is
//! refused; the shared `MaxRecords` window and `Marker` cursor paginate it; and
//! reading it changes nothing. The catalog tables are written out here, quoting
//! the source each comes from, so the suite pins them independently of the
//! constants the handler reads.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, element_blocks, Answer, Emulator};

/// The hourly rate the catalog publishes for one node of each offered node type,
/// in the order the catalog offers them.
///
/// Source: the service's own published rate catalog
/// (`state::node_type_hourly_rate`), which covers every node type
/// `CreateCluster` documents ("Valid Values: dc2.large | dc2.8xlarge | rg.large
/// | rg.xlarge | rg.4xlarge | rg.12xlarge | ra3.large | ra3.xlplus |
/// ra3.4xlarge | ra3.16xlarge"). Written out here so a changed rate fails this
/// suite, and checked below against the helper the handler reads.
const NODE_TYPE_RATES: [(&str, f64); 10] = [
    ("dc2.large", 0.25),
    ("dc2.8xlarge", 4.80),
    ("rg.large", 0.36),
    ("rg.xlarge", 0.72),
    ("rg.4xlarge", 2.88),
    ("rg.12xlarge", 8.64),
    ("ra3.large", 0.52),
    ("ra3.xlplus", 1.09),
    ("ra3.4xlarge", 3.26),
    ("ra3.16xlarge", 13.04),
];

/// Every published term, as `(Duration in seconds, share of the hourly rate the
/// term is charged at)`, shortest first: a one-year and a three-year commitment
/// of 365-day years.
const TERMS: [(i32, f64); 2] = [(31_536_000, 0.70), (94_608_000, 0.45)];

/// Every published offering type, as `(OfferingType, share of the term's cost
/// charged upfront)`, in catalog order: the payment options Amazon Redshift's
/// reserved-node catalog is published under, from the one that charges nothing
/// upfront to the one that charges all of it.
const OFFERING_TYPES: [(&str, f64); 3] = [
    ("No Upfront", 0.0),
    ("Partial Upfront", 0.5),
    ("All Upfront", 1.0),
];

/// The payment option that charges the whole term upfront, leaving a recurring
/// charge of nothing.
const ALL_UPFRONT: &str = "All Upfront";

/// The `ReservedNodeOfferingType` every offering of the PURCHASE catalog
/// carries: it is bought outright, not exchanged onto.
///
/// Source: `smithy_query(query="shape", shape="ReservedNodeOfferingType")` —
/// the enum's values are `Regular` and `Upgradable`, and
/// `smithy_query(query="doc", shape="GetReservedNodeExchangeOfferings")` is what
/// reports the `Upgradable` ones ("Returns an array of DC2 ReservedNodeOfferings
/// that matches the payment type, term, and usage price of the given DC1
/// reserved node").
const REGULAR: &str = "Regular";

/// The node-type prefix of an offering an exchange can target.
///
/// Source: `smithy_query(query="doc", shape="GetReservedNodeExchangeOfferings")`
/// — "Returns an array of DC2 ReservedNodeOfferings that matches the payment
/// type, term, and usage price of the given DC1 reserved node."
const EXCHANGE_TARGET_PREFIX: &str = "dc2.";

/// The currency every published price is quoted in.
const CURRENCY_CODE: &str = "USD";

/// How closely a served price must match the rule, in currency units per hour:
/// a published price carries cents on the fixed price and mills on the hourly
/// charge, so the per-hour sum of the two can differ from the exact term cost by
/// at most that rounding.
const PRICE_TOLERANCE: f64 = 1e-3;

/// One served offering, reduced to what the assertions read.
#[derive(Debug, Clone, PartialEq)]
struct Offering {
    id: String,
    node_type: String,
    duration: i32,
    fixed_price: f64,
    usage_price: f64,
    currency_code: String,
    offering_type: String,
    /// Every `RecurringCharges` entry, as `(amount, frequency)`.
    recurring: Vec<(f64, String)>,
    /// The `ReservedNodeOfferingType` the offering carries.
    offering_class: String,
}

impl Offering {
    /// What identifies the offering in the catalog: its three dimensions.
    fn key(&self) -> (String, i32, String) {
        (
            self.node_type.clone(),
            self.duration,
            self.offering_type.clone(),
        )
    }

    /// The whole hourly cost of the reservation: the upfront price spread over
    /// the term's hours plus whatever is charged per hour.
    fn effective_hourly(&self) -> f64 {
        let hourly: f64 = self.usage_price + self.recurring.iter().map(|(amount, _)| amount).sum::<f64>();
        self.fixed_price / self.hours() + hourly
    }

    /// The term, in hours.
    fn hours(&self) -> f64 {
        f64::from(self.duration) / 3600.0
    }
}

/// The text of the one `<tag>` element in one offering's XML, asserting the
/// offering carries exactly one.
fn one(case: &str, block: &str, tag: &str) -> String {
    let found = all_text(block, tag);
    assert_eq!(
        found.len(),
        1,
        "case {case}: expected exactly one <{tag}> per offering, got {found:?} in {block}"
    );
    found.into_iter().next().unwrap_or_default()
}

/// One offering's `<tag>` as a price.
fn price(case: &str, block: &str, tag: &str) -> f64 {
    let text = one(case, block, tag);
    text.parse::<f64>()
        .unwrap_or_else(|_| panic!("case {case}: <{tag}> must be a number, got {text:?}"))
}

/// The inner XML of every `<ReservedNodeOffering>` element in a response body,
/// in document order — the harness's shared element scanner, named for the one
/// element this suite reads.
fn offering_blocks(xml: &str) -> Vec<String> {
    element_blocks(xml, "ReservedNodeOffering")
}

/// Every offering a `DescribeReservedNodeOfferings` body carries, in document
/// order.
fn listed_offerings(case: &str, answer: &Answer) -> Vec<Offering> {
    offering_blocks(&answer.body)
        .into_iter()
        .map(|block| {
            let amounts = all_text(&block, "RecurringChargeAmount");
            let frequencies = all_text(&block, "RecurringChargeFrequency");
            assert_eq!(
                amounts.len(),
                frequencies.len(),
                "case {case}: a recurring charge carries an amount and a frequency, got {amounts:?} / {frequencies:?}"
            );
            let duration = one(case, &block, "Duration");
            Offering {
                id: one(case, &block, "ReservedNodeOfferingId"),
                node_type: one(case, &block, "NodeType"),
                duration: duration
                    .parse()
                    .unwrap_or_else(|_| panic!("case {case}: <Duration> must be an integer, got {duration:?}")),
                fixed_price: price(case, &block, "FixedPrice"),
                usage_price: price(case, &block, "UsagePrice"),
                currency_code: one(case, &block, "CurrencyCode"),
                offering_type: one(case, &block, "OfferingType"),
                recurring: amounts
                    .into_iter()
                    .map(|amount| {
                        amount.parse::<f64>().unwrap_or_else(|_| {
                            panic!("case {case}: a recurring amount must be a number, got {amount:?}")
                        })
                    })
                    .zip(frequencies)
                    .collect(),
                offering_class: one(case, &block, "ReservedNodeOfferingType"),
            }
        })
        .collect()
}

/// The offerings one `DescribeReservedNodeOfferings` call reports, asserting the
/// call succeeded.
async fn offerings(emulator: &Emulator, case: &str, params: &[(&str, &str)]) -> Vec<Offering> {
    let answer = emulator
        .call("DescribeReservedNodeOfferings", params)
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} / {:?} with body {}",
        answer.status,
        answer.error_code(),
        answer.body
    );
    listed_offerings(case, &answer)
}

/// The whole catalog the rule describes, as the three dimensions of each
/// offering: node type, then term, then offering type.
fn expected_keys() -> Vec<(String, i32, String)> {
    let mut expected = Vec::new();
    for (node_type, _) in NODE_TYPE_RATES {
        for (duration, _) in TERMS {
            for (offering_type, _) in OFFERING_TYPES {
                expected.push((node_type.to_string(), duration, offering_type.to_string()));
            }
        }
    }
    expected
}

/// The share of the hourly rate the given term is charged at.
fn term_share(duration: i32) -> f64 {
    TERMS
        .into_iter()
        .find(|(offered, _)| *offered == duration)
        .map(|(_, share)| share)
        .unwrap_or_else(|| panic!("the catalog publishes the term {duration}"))
}

/// The share of the term's cost the given offering type charges upfront.
fn upfront_share(offering_type: &str) -> f64 {
    OFFERING_TYPES
        .into_iter()
        .find(|(offered, _)| *offered == offering_type)
        .map(|(_, share)| share)
        .unwrap_or_else(|| panic!("the catalog publishes the offering type {offering_type}"))
}

/// The hourly rate the catalog publishes for the given node type.
fn node_rate(node_type: &str) -> f64 {
    NODE_TYPE_RATES
        .into_iter()
        .find(|(offered, _)| *offered == node_type)
        .map(|(_, rate)| rate)
        .unwrap_or_else(|| panic!("the catalog publishes the node type {node_type}"))
}

#[tokio::test]
async fn the_catalog_is_the_product_of_node_type_term_and_payment_option() {
    let emulator = Emulator::new();

    let served = offerings(&emulator, "the whole catalog", &[]).await;
    let keys: Vec<(String, i32, String)> = served.iter().map(Offering::key).collect();
    assert_eq!(
        keys,
        expected_keys(),
        "case the whole catalog: expected every node-type/term/offering-type combination, got {keys:?}"
    );

    // The tables the handler reads must be the ones this suite pins, and the
    // catalog must offer every node type a cluster can be created with.
    assert_eq!(
        state::RESERVED_NODE_TERMS.to_vec(),
        TERMS.to_vec(),
        "case catalog terms: the handler's terms must match the pinned ones"
    );
    assert_eq!(
        state::RESERVED_NODE_OFFERING_TYPES.to_vec(),
        OFFERING_TYPES.to_vec(),
        "case catalog offering types: the handler's offering types must match the pinned ones"
    );
    for node_type in state::ORDERABLE_NODE_TYPES {
        assert_eq!(
            state::node_type_hourly_rate(node_type),
            Some(node_rate(node_type)),
            "case rate of {node_type}: every orderable node type carries the pinned rate"
        );
    }

    // "You then use the unique offering ID in you call to
    // PurchaseReservedNodeOffering": an id names ONE offering, so no two
    // offerings share one.
    let mut ids: Vec<String> = served.iter().map(|offering| offering.id.clone()).collect();
    let total = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(
        ids.len(),
        total,
        "case unique ids: expected {total} distinct offering ids, got {}",
        ids.len()
    );
}

#[tokio::test]
async fn every_offering_charges_the_term_cost_the_way_its_payment_option_says() {
    let emulator = Emulator::new();

    let served = offerings(&emulator, "the whole catalog", &[]).await;
    for offering in &served {
        let case = format!(
            "{} / {}s / {}",
            offering.node_type, offering.duration, offering.offering_type
        );
        let rate = node_rate(&offering.node_type) * term_share(offering.duration);
        let upfront = upfront_share(&offering.offering_type);

        assert_eq!(
            offering.currency_code, CURRENCY_CODE,
            "case {case}: expected every price quoted in {CURRENCY_CODE}, got {:?}",
            offering.currency_code
        );

        // The upfront share of the term's whole cost is the `FixedPrice`.
        let expected_fixed = rate * offering.hours() * upfront;
        assert!(
            (offering.fixed_price - expected_fixed).abs() <= PRICE_TOLERANCE * offering.hours(),
            "case {case}: expected a fixed price of about {expected_fixed}, got {}",
            offering.fixed_price
        );

        // Whatever the payment option does not take upfront is charged for every
        // hour of the term as the offering's ONE recurring charge, and nothing is
        // charged per hour the cluster using the offering runs.
        let expected_hourly = rate * (1.0 - upfront);
        assert_eq!(
            offering.usage_price, 0.0,
            "case {case}: a published offering charges nothing per running hour, got {}",
            offering.usage_price
        );
        assert_eq!(
            offering.recurring.len(),
            1,
            "case {case}: expected one recurring charge on every offering, got {:?}",
            offering.recurring
        );
        let (amount, frequency) = &offering.recurring[0];
        assert_eq!(
            frequency, "Hourly",
            "case {case}: expected an hourly recurring charge, got {frequency:?}"
        );
        assert!(
            (amount - expected_hourly).abs() <= PRICE_TOLERANCE,
            "case {case}: expected a recurring charge of about {expected_hourly}, got {amount}"
        );
        if offering.offering_type == ALL_UPFRONT {
            assert_eq!(
                *amount, 0.0,
                "case {case}: paying the whole term upfront leaves a recurring charge of nothing, got {amount}"
            );
        } else {
            assert!(
                *amount > 0.0,
                "case {case}: a term not paid in full upfront carries an hourly charge, got {amount}"
            );
        }

        // Upfront plus hourly is the term's whole cost, whichever way it is
        // split.
        assert!(
            (offering.effective_hourly() - rate).abs() <= PRICE_TOLERANCE,
            "case {case}: expected the term to cost about {rate} per hour in all, got {}",
            offering.effective_hourly()
        );

        // Every offering of this catalog is bought outright, whatever its node
        // type: the offerings an exchange may move onto are a catalog of their
        // own.
        assert_eq!(
            offering.offering_class, REGULAR,
            "case {case}: expected the offering class {REGULAR}, got {:?}",
            offering.offering_class
        );
    }

    // A longer commitment is charged at a lower hourly cost, and a larger node
    // costs more than a smaller one of the same family, for every offering type.
    for (offering_type, _) in OFFERING_TYPES {
        for (node_type, _) in NODE_TYPE_RATES {
            let of = |duration: i32| {
                served
                    .iter()
                    .find(|offering| {
                        offering.key() == (node_type.to_string(), duration, offering_type.to_string())
                    })
                    .unwrap_or_else(|| panic!("the catalog offers {node_type} for {duration}s"))
            };
            let (short, long) = (of(TERMS[0].0), of(TERMS[1].0));
            assert!(
                long.effective_hourly() < short.effective_hourly(),
                "case {node_type} / {offering_type}: expected the {}s term to cost less per hour than the {}s term, got {} and {}",
                long.duration,
                short.duration,
                long.effective_hourly(),
                short.effective_hourly()
            );
        }
        let large = served
            .iter()
            .find(|offering| {
                offering.key()
                    == ("ra3.16xlarge".to_string(), TERMS[0].0, offering_type.to_string())
            })
            .expect("the catalog offers ra3.16xlarge");
        let small = served
            .iter()
            .find(|offering| {
                offering.key() == ("ra3.large".to_string(), TERMS[0].0, offering_type.to_string())
            })
            .expect("the catalog offers ra3.large");
        assert!(
            large.effective_hourly() > small.effective_hourly(),
            "case node size / {offering_type}: expected ra3.16xlarge to cost more per hour than ra3.large, got {} and {}",
            large.effective_hourly(),
            small.effective_hourly()
        );
    }
}

#[tokio::test]
async fn an_offering_id_selects_the_one_offering_it_names() {
    let emulator = Emulator::new();

    let catalog = offerings(&emulator, "the whole catalog", &[]).await;

    // Rows differ along which offering the id names: the first and the last of
    // the catalog, one of an exchange-target node type, a fully-upfront one and
    // a three-year one.
    let wanted: Vec<(&str, &Offering)> = vec![
        ("the first offering", &catalog[0]),
        (
            "the last offering",
            catalog.last().expect("the catalog is not empty"),
        ),
        (
            "an offering of an exchange-target node type",
            catalog
                .iter()
                .find(|offering| offering.node_type.starts_with(EXCHANGE_TARGET_PREFIX))
                .expect("the catalog offers a dc2 node type"),
        ),
        (
            "a fully-upfront offering",
            catalog
                .iter()
                .find(|offering| offering.offering_type == ALL_UPFRONT)
                .expect("the catalog offers the fully-upfront payment option"),
        ),
        (
            "a three-year offering",
            catalog
                .iter()
                .find(|offering| offering.duration == TERMS[1].0)
                .expect("the catalog offers the longer term"),
        ),
    ];

    for (case, expected) in wanted {
        let served = offerings(
            &emulator,
            case,
            &[("ReservedNodeOfferingId", &expected.id)],
        )
        .await;
        assert_eq!(
            served,
            vec![expected.clone()],
            "case {case}: expected just the offering the id names, got {served:?}"
        );
    }

    // An id the catalog does not publish names no offering at all: "Specified
    // offering does not exist" (HTTP 404), not an empty list.
    let published = catalog[0].id.clone();
    let uppercased = published.to_uppercase();
    let truncated = published[..published.len() - 1].to_string();
    for (case, offering_id) in [
        ("an id nobody published", "offering-1"),
        ("a well-formed id nobody published", "00000000-0000-0000-0000-000000000000"),
        ("a published id in upper case", uppercased.as_str()),
        ("a published id missing its last character", truncated.as_str()),
        ("an id with surrounding space", " offering-1 "),
    ] {
        let answer = emulator
            .call(
                "DescribeReservedNodeOfferings",
                &[("ReservedNodeOfferingId", offering_id)],
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (404, Some("ReservedNodeOfferingNotFound".to_string())),
            "case {case} ({offering_id:?}): expected a 404 refusal, got {} / {:?} with body {}",
            answer.status,
            answer.error_code(),
            answer.body
        );
    }
}

#[tokio::test]
async fn the_documented_max_records_window_and_marker_cursor_govern_the_page() {
    let emulator = Emulator::new();

    // "Default: 100 Constraints: minimum 20, maximum 100." — each published
    // bound, and one past each; only the minimum is enforced.
    for (case, max_records, accepted) in [
        ("one below the minimum", state::MAX_RECORDS_MIN - 1, false),
        ("at the minimum", state::MAX_RECORDS_MIN, true),
        ("inside the window", state::MAX_RECORDS_MIN + 1, true),
        ("at the maximum", state::MAX_RECORDS_MAX, true),
        ("one above the maximum", state::MAX_RECORDS_MAX + 1, true),
        ("zero", 0, false),
    ] {
        let value = max_records.to_string();
        let answer = emulator
            .call("DescribeReservedNodeOfferings", &[("MaxRecords", &value)])
            .await;
        assert_eq!(
            answer.status == 200,
            accepted,
            "case MaxRecords {case} ({value}): expected accepted={accepted}, got {} / {:?}",
            answer.status,
            answer.error_code()
        );
        if !accepted {
            assert_eq!(
                (answer.status, answer.error_code()),
                (400, Some("InvalidParameterValue".to_string())),
                "case MaxRecords {case} ({value}): expected a 400 refusal, got {} / {:?}",
                answer.status,
                answer.error_code()
            );
        }
    }

    // The catalog is longer than the smallest page the window allows, so a page
    // is truncated and the `Marker` it hands back resumes the next one: walking
    // the cursor reports the whole catalog, in order, exactly once.
    let whole = offerings(&emulator, "the whole catalog", &[]).await;
    let page_size = state::MAX_RECORDS_MIN.to_string();
    let mut walked: Vec<Offering> = Vec::new();
    let mut marker: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut params: Vec<(&str, &str)> = vec![("MaxRecords", page_size.as_str())];
        if let Some(cursor) = marker.as_deref() {
            params.push(("Marker", cursor));
        }
        let answer = emulator
            .call("DescribeReservedNodeOfferings", &params)
            .await;
        assert_eq!(
            answer.status, 200,
            "case page {pages}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        let page = listed_offerings("a walked page", &answer);
        assert!(
            page.len() <= state::MAX_RECORDS_MIN as usize,
            "case page {pages}: expected at most {} offerings, got {}",
            state::MAX_RECORDS_MIN,
            page.len()
        );
        walked.extend(page);
        pages += 1;
        assert!(
            pages <= whole.len(),
            "case marker walk: the cursor must terminate, got {pages} pages"
        );
        marker = answer.text("Marker");
        if marker.is_none() {
            break;
        }
    }
    assert_eq!(
        walked, whole,
        "case marker walk: expected the whole catalog across {pages} page(s), got {} offerings",
        walked.len()
    );
    assert!(
        pages > 1,
        "case marker walk: the catalog must span more than one smallest page, got {pages}"
    );

    // A cursor nobody handed out is refused rather than served as an offset.
    for (case, cursor) in [
        ("not a number", "not-a-cursor"),
        ("an invented offset", "7"),
        ("an offset with a forged tag", "20.00000000"),
    ] {
        let bogus = emulator
            .call("DescribeReservedNodeOfferings", &[("Marker", cursor)])
            .await;
        assert_eq!(
            (bogus.status, bogus.error_code()),
            (400, Some("InvalidParameterValue".to_string())),
            "case marker {case} ({cursor:?}): expected a 400 refusal, got {} / {:?}",
            bogus.status,
            bogus.error_code()
        );
    }
}

#[tokio::test]
async fn reading_the_catalog_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();

    let first = offerings(&emulator, "first read", &[]).await;
    let second = offerings(&emulator, "second read", &[]).await;
    assert_eq!(
        first, second,
        "case repeated read: the catalog is stable across reads, got {second:?}"
    );

    // What the account holds does not change what is offered, and reading what
    // is offered does not touch what the account holds.
    let created = emulator.create_minimal("offering-probe").await;
    assert_eq!(
        created.status, 200,
        "case seed a cluster: expected 200, got {} with body {}",
        created.status, created.body
    );
    let after_create = offerings(&emulator, "read after a create", &[]).await;
    assert_eq!(
        after_create, first,
        "case read after a create: an account's clusters do not change the catalog, got {after_create:?}"
    );

    // The store still holds exactly the published catalog: a read neither
    // reserves a node nor consumes an offering.
    let store = state::service_state(&*emulator.engine).expect("the Redshift store");
    let held = store.lock().reserved_node_offerings.len();
    assert_eq!(
        held,
        state::published_reserved_node_offerings().len(),
        "case the store after the reads: expected the published catalog untouched, got {held} offering(s)"
    );
    assert_eq!(
        held,
        first.len(),
        "case the store after the reads: the catalog a read reports is the one the store holds, got {held} offering(s)"
    );
}
