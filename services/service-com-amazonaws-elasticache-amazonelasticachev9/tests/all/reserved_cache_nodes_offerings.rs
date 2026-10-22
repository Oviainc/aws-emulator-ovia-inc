// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeReservedCacheNodesOfferings`: the catalog of
//! reserved cache node offerings this service publishes, the five filter
//! members that narrow it, and the `MaxRecords` / `Marker` pagination it is
//! served through.
//!
//! Every assertion goes over the wire, through the same router the production
//! emulator mounts (the harness in [`super::cache_cluster_lifecycle`]), so only
//! the HTTP status and the XML body are inspected and nothing reaches into the
//! service's state struct.
//!
//! The tests are tables and invariants rather than one recorded answer: a row
//! per filter member and per documented value of it, the published `MaxRecords`
//! bounds and one past each, and the pricing and pagination properties that
//! must hold for EVERY entry of the catalog.

use service_com_amazonaws_elasticache_amazonelasticachev9::state;

use super::cache_cluster_lifecycle::{params, Emulator, Reply};

/// The operation under test.
const DESCRIBE: &str = "DescribeReservedCacheNodesOfferings";

/// How many offerings the catalog holds in total: one per cache engine, cache
/// node type, reservation term and stocked payment option.
fn catalog_size() -> usize {
    state::ENGINES.len()
        * state::OFFERING_NODE_TYPES.len()
        * state::OFFERING_TERMS.len()
        * state::STOCKED_OFFERING_TYPES.len()
}

/// How many offerings one engine has across every node type and term, for one
/// payment option.
fn offerings_per_engine_and_type() -> usize {
    state::OFFERING_NODE_TYPES.len() * state::OFFERING_TERMS.len()
}

/// One `DescribeReservedCacheNodesOfferings` call with the given form pairs.
async fn describe(emulator: &Emulator, pairs: &[(&str, &str)]) -> Reply {
    emulator.call(DESCRIBE, &params(pairs)).await
}

/// The `ReservedCacheNodesOfferingId` of every offering in a reply, in document
/// order.
fn offering_ids(reply: &Reply) -> Vec<&str> {
    reply.all("ReservedCacheNodesOfferingId")
}

/// Every text node named `suffix` in a reply, parsed as a number.
fn numbers(reply: &Reply, suffix: &str) -> Vec<f64> {
    reply
        .all(suffix)
        .into_iter()
        .map(|text| {
            text.parse::<f64>()
                .unwrap_or_else(|_| panic!("{suffix} must be a number, got {text:?}"))
        })
        .collect()
}

/// True when `id` has the form `ReservedCacheNodesOfferingId`'s own example
/// gives (`438012d3-4052-4cc7-b2e3-8d3372e0e706`): five hyphen-separated hex
/// groups of 8, 4, 4, 4 and 12 digits.
fn is_offering_id(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .map(|group| group.len())
            .eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

/// Page the catalog to exhaustion, returning every offering id in page order.
///
/// `page_size` is the `MaxRecords` each call asks for, or `None` to let the
/// operation serve its documented default.
async fn all_offering_ids(
    emulator: &Emulator,
    filter: &[(&str, &str)],
    page_size: Option<&str>,
) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    for page in 0..64 {
        let held = marker.clone();
        let mut pairs: Vec<(&str, &str)> = filter.to_vec();
        if let Some(size) = page_size {
            pairs.push(("MaxRecords", size));
        }
        if let Some(value) = held.as_deref() {
            pairs.push(("Marker", value));
        }
        let reply = describe(emulator, &pairs).await;
        assert_eq!(
            reply.status, 200,
            "page {page} of {page_size:?}: describe must succeed, got {}",
            reply.body
        );
        let page_ids = offering_ids(&reply);
        match reply.marker() {
            Some(next) => {
                if let Some(size) = page_size {
                    assert_eq!(
                        page_ids.len().to_string(),
                        size,
                        "page {page} of {page_size:?}: a page followed by a Marker must be full, \
                         got {} records",
                        page_ids.len()
                    );
                }
                let next = next.to_string();
                ids.extend(page_ids.into_iter().map(str::to_string));
                marker = Some(next);
            }
            None => {
                ids.extend(page_ids.into_iter().map(str::to_string));
                return ids;
            }
        }
    }
    panic!("paging {page_size:?} did not reach the end of the catalog in 64 pages");
}

/// One filter row: the case name, the form pairs sent, the element values every
/// returned offering must carry, and whether the catalog must hold a match.
struct FilterCase {
    case: &'static str,
    pairs: &'static [(&'static str, &'static str)],
    expect: &'static [(&'static str, &'static str)],
    non_empty: bool,
}

#[tokio::test]
async fn every_documented_filter_member_narrows_the_catalog() {
    // The axis: each of the five filter members, over documented values of it —
    // every engine `ProductDescription` reports, node types from both
    // generations, both terms in both the year and the second form `Duration`
    // accepts, each stocked `OfferingType` and each legacy one, a value outside
    // the catalog, and all four together.
    let cases = [
        FilterCase {
            case: "product description redis",
            pairs: &[("ProductDescription", "redis")],
            expect: &[("ProductDescription", "redis")],
            non_empty: true,
        },
        FilterCase {
            case: "product description memcached",
            pairs: &[("ProductDescription", "memcached")],
            expect: &[("ProductDescription", "memcached")],
            non_empty: true,
        },
        FilterCase {
            case: "product description valkey",
            pairs: &[("ProductDescription", "valkey")],
            expect: &[("ProductDescription", "valkey")],
            non_empty: true,
        },
        FilterCase {
            case: "product description nothing runs",
            pairs: &[("ProductDescription", "postgres")],
            expect: &[],
            non_empty: false,
        },
        FilterCase {
            case: "current generation node type",
            pairs: &[("CacheNodeType", "cache.r7g.large")],
            expect: &[("CacheNodeType", "cache.r7g.large")],
            non_empty: true,
        },
        FilterCase {
            case: "previous generation node type",
            pairs: &[("CacheNodeType", "cache.m3.medium")],
            expect: &[("CacheNodeType", "cache.m3.medium")],
            non_empty: true,
        },
        FilterCase {
            case: "burstable node type",
            pairs: &[("CacheNodeType", "cache.t4g.micro")],
            expect: &[("CacheNodeType", "cache.t4g.micro")],
            non_empty: true,
        },
        FilterCase {
            case: "node type the catalog does not carry",
            pairs: &[("CacheNodeType", "cache.z9.nano")],
            expect: &[],
            non_empty: false,
        },
        FilterCase {
            case: "one year term in years",
            pairs: &[("Duration", "1")],
            expect: &[("Duration", "31536000")],
            non_empty: true,
        },
        FilterCase {
            case: "one year term in seconds",
            pairs: &[("Duration", "31536000")],
            expect: &[("Duration", "31536000")],
            non_empty: true,
        },
        FilterCase {
            case: "three year term in years",
            pairs: &[("Duration", "3")],
            expect: &[("Duration", "94608000")],
            non_empty: true,
        },
        FilterCase {
            case: "three year term in seconds",
            pairs: &[("Duration", "94608000")],
            expect: &[("Duration", "94608000")],
            non_empty: true,
        },
        FilterCase {
            case: "all upfront",
            pairs: &[("OfferingType", "All Upfront")],
            expect: &[("OfferingType", "All Upfront")],
            non_empty: true,
        },
        FilterCase {
            case: "partial upfront",
            pairs: &[("OfferingType", "Partial Upfront")],
            expect: &[("OfferingType", "Partial Upfront")],
            non_empty: true,
        },
        FilterCase {
            case: "no upfront",
            pairs: &[("OfferingType", "No Upfront")],
            expect: &[("OfferingType", "No Upfront")],
            non_empty: true,
        },
        FilterCase {
            case: "offering type in another case",
            pairs: &[("OfferingType", "no upfront")],
            expect: &[("OfferingType", "No Upfront")],
            non_empty: true,
        },
        // The three legacy utilization types are documented values of
        // `OfferingType`, so they are served rather than refused — and the
        // catalog stocks none of them.
        FilterCase {
            case: "legacy light utilization",
            pairs: &[("OfferingType", "Light Utilization")],
            expect: &[],
            non_empty: false,
        },
        FilterCase {
            case: "legacy medium utilization",
            pairs: &[("OfferingType", "Medium Utilization")],
            expect: &[],
            non_empty: false,
        },
        FilterCase {
            case: "legacy heavy utilization",
            pairs: &[("OfferingType", "Heavy Utilization")],
            expect: &[],
            non_empty: false,
        },
        FilterCase {
            case: "every filter at once",
            pairs: &[
                ("ProductDescription", "valkey"),
                ("CacheNodeType", "cache.m7g.large"),
                ("Duration", "3"),
                ("OfferingType", "Partial Upfront"),
            ],
            expect: &[
                ("ProductDescription", "valkey"),
                ("CacheNodeType", "cache.m7g.large"),
                ("Duration", "94608000"),
                ("OfferingType", "Partial Upfront"),
            ],
            non_empty: true,
        },
    ];

    let emulator = Emulator::new();
    for FilterCase {
        case,
        pairs,
        expect,
        non_empty,
    } in cases
    {
        let reply = describe(&emulator, pairs).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: a documented filter value must be served, got {}",
            reply.body
        );
        let ids = offering_ids(&reply);
        assert_eq!(
            !ids.is_empty(),
            non_empty,
            "case {case:?}: expected non_empty={non_empty}, got {} offerings",
            ids.len()
        );
        for (element, wanted) in expect {
            let values = reply.all(element);
            assert_eq!(
                values.len(),
                ids.len(),
                "case {case:?}: every offering must report {element}, got {values:?} for {} \
                 offerings",
                ids.len()
            );
            assert!(
                values.iter().all(|value| value == wanted),
                "case {case:?}: every {element} must be {wanted:?}, got {values:?}"
            );
        }
    }

    // The four-member filter names exactly one catalog entry.
    let one = describe(
        &emulator,
        &[
            ("ProductDescription", "valkey"),
            ("CacheNodeType", "cache.m7g.large"),
            ("Duration", "3"),
            ("OfferingType", "Partial Upfront"),
        ],
    )
    .await;
    assert_eq!(
        offering_ids(&one).len(),
        1,
        "a filter naming an engine, a node type, a term and a payment option must name one \
         offering, got {}",
        one.body
    );
}

/// One refusal row: the case name, the form pairs sent, and the HTTP status
/// plus `awsQuery` error code the request must be answered with (an empty code
/// meaning the request is served).
type RefusalCase = (
    &'static str,
    &'static [(&'static str, &'static str)],
    u16,
    &'static str,
);

#[tokio::test]
async fn filter_values_outside_their_documented_sets_are_refused() {
    // The axis: each member with its own documented value set — `Duration`
    // ("Valid Values: 1 | 3 | 31536000 | 94608000"), `OfferingType`,
    // `MaxRecords` ("minimum 20; maximum 100") on each published bound and one
    // past it, a `Marker` this service did not mint, and an offering id the
    // catalog does not hold.
    let cases: &[RefusalCase] = &[
        ("term of two years", &[("Duration", "2")], 400, "InvalidParameterValue"),
        ("term of zero", &[("Duration", "0")], 400, "InvalidParameterValue"),
        (
            "a second past the one year term",
            &[("Duration", "31536001")],
            400,
            "InvalidParameterValue",
        ),
        (
            "term that is not a number",
            &[("Duration", "one year")],
            400,
            "InvalidParameterValue",
        ),
        (
            "offering type nobody sells",
            &[("OfferingType", "Gold Utilization")],
            400,
            "InvalidParameterValue",
        ),
        ("page size at the published minimum", &[("MaxRecords", "20")], 200, ""),
        ("page size one below the minimum", &[("MaxRecords", "19")], 400, "InvalidParameterValue"),
        ("page size at the published maximum", &[("MaxRecords", "100")], 200, ""),
        ("page size one above the maximum", &[("MaxRecords", "101")], 400, "InvalidParameterValue"),
        (
            "marker this service did not mint",
            &[("Marker", "not-a-marker")],
            400,
            "InvalidParameterValue",
        ),
        // The example from `ReservedCacheNodesOfferingId`'s own documentation:
        // a well-formed id this catalog does not stock.
        (
            "offering id the catalog does not hold",
            &[("ReservedCacheNodesOfferingId", "438012d3-4052-4cc7-b2e3-8d3372e0e706")],
            404,
            "ReservedCacheNodesOfferingNotFound",
        ),
    ];

    let emulator = Emulator::new();
    for (case, pairs, status, code) in cases {
        let reply = describe(&emulator, pairs).await;
        assert_eq!(
            reply.status, *status,
            "case {case:?}: expected HTTP {status}, got {} with {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            *code,
            "case {case:?}: expected error code {code:?}, got {}",
            reply.body
        );
    }
}

/// One payment-option row: the `OfferingType`, whether its offerings charge a
/// fixed price up front, and whether they carry an hourly recurring charge.
struct PaymentCase {
    offering_type: &'static str,
    fixed_price_charged: bool,
    recurring_charged: bool,
}

#[tokio::test]
async fn each_payment_option_prices_every_offering_it_stocks() {
    // The rule the catalog is priced by: an all-upfront reservation pays the
    // whole term as `FixedPrice` and carries no recurring charge, a no-upfront
    // one pays nothing up front and carries the hourly recurring charge, and a
    // partial-upfront one does both. `UsagePrice` is zero for all three: the
    // hourly charge of an upfront-type reservation is the recurring charge.
    let cases = [
        PaymentCase {
            offering_type: state::OFFERING_TYPE_ALL_UPFRONT,
            fixed_price_charged: true,
            recurring_charged: false,
        },
        PaymentCase {
            offering_type: state::OFFERING_TYPE_PARTIAL_UPFRONT,
            fixed_price_charged: true,
            recurring_charged: true,
        },
        PaymentCase {
            offering_type: state::OFFERING_TYPE_NO_UPFRONT,
            fixed_price_charged: false,
            recurring_charged: true,
        },
    ];

    let emulator = Emulator::new();
    for PaymentCase {
        offering_type,
        fixed_price_charged,
        recurring_charged,
    } in cases
    {
        // One engine at a time keeps every answer inside one page (the catalog
        // holds fewer offerings per engine and payment option than the maximum
        // `MaxRecords`), so the prices in the body belong to the offerings in
        // the same body.
        for product_description in state::ENGINES {
            let case = format!("{offering_type} / {product_description}");
            let reply = describe(
                &emulator,
                &[
                    ("OfferingType", offering_type),
                    ("ProductDescription", product_description),
                    ("MaxRecords", "100"),
                ],
            )
            .await;
            assert_eq!(
                reply.status, 200,
                "case {case:?}: describe must succeed, got {}",
                reply.body
            );
            assert_eq!(reply.marker(), None, "case {case:?}: got {}", reply.body);
            let count = offering_ids(&reply).len();
            assert_eq!(
                count,
                offerings_per_engine_and_type(),
                "case {case:?}: expected one offering per node type and term, got {count}"
            );

            let fixed_prices = numbers(&reply, "FixedPrice");
            assert_eq!(
                fixed_prices.len(),
                count,
                "case {case:?}: every offering must report a FixedPrice, got {fixed_prices:?}"
            );
            for price in &fixed_prices {
                assert_eq!(
                    *price > 0.0, fixed_price_charged,
                    "case {case:?}: expected fixed_price_charged={fixed_price_charged}, got \
                     {price}"
                );
            }

            let usage_prices = numbers(&reply, "UsagePrice");
            assert_eq!(
                usage_prices.len(),
                count,
                "case {case:?}: every offering must report a UsagePrice, got {usage_prices:?}"
            );
            for price in &usage_prices {
                assert_eq!(
                    *price, 0.0,
                    "case {case:?}: an upfront-type offering charges no usage price, got {price}"
                );
            }

            let recurring = numbers(&reply, "RecurringChargeAmount");
            let frequencies = reply.all("RecurringChargeFrequency");
            let expected_charges = if recurring_charged { count } else { 0 };
            assert_eq!(
                recurring.len(),
                expected_charges,
                "case {case:?}: expected {expected_charges} recurring charges, got {recurring:?}"
            );
            assert_eq!(
                frequencies.len(),
                expected_charges,
                "case {case:?}: expected {expected_charges} recurring frequencies, got \
                 {frequencies:?}"
            );
            for amount in &recurring {
                assert!(
                    *amount > 0.0,
                    "case {case:?}: a recurring charge must be charged, got {amount}"
                );
            }
            for frequency in &frequencies {
                assert_eq!(
                    *frequency,
                    state::RECURRING_CHARGE_HOURLY,
                    "case {case:?}: a reservation's recurring charge is hourly, got {frequency:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_longer_term_is_discounted_more_than_a_shorter_one() {
    // The axis: node types across the families and both generations. The
    // invariant: the three-year reservation of one offering costs MORE in total
    // than the one-year one and LESS than three of them, so the longer term is
    // the deeper discount — the ordering a reservation is bought for.
    let cases = [
        ("redis", "cache.t3.micro"),
        ("memcached", "cache.m5.large"),
        ("valkey", "cache.r5.2xlarge"),
        ("redis", "cache.m3.medium"),
    ];

    let emulator = Emulator::new();
    for (product_description, cache_node_type) in cases {
        let case = format!("{product_description} / {cache_node_type}");
        let mut fixed_prices = Vec::new();
        for (years, _) in state::OFFERING_TERMS {
            let term = years.to_string();
            let reply = describe(
                &emulator,
                &[
                    ("ProductDescription", product_description),
                    ("CacheNodeType", cache_node_type),
                    ("OfferingType", state::OFFERING_TYPE_ALL_UPFRONT),
                    ("Duration", &term),
                ],
            )
            .await;
            assert_eq!(
                reply.status, 200,
                "case {case:?} term {years}: describe must succeed, got {}",
                reply.body
            );
            let prices = numbers(&reply, "FixedPrice");
            assert_eq!(
                prices.len(),
                1,
                "case {case:?} term {years}: expected one offering, got {}",
                reply.body
            );
            fixed_prices.push(prices[0]);
        }
        let (one_year, three_year) = (fixed_prices[0], fixed_prices[1]);
        assert!(
            three_year > one_year,
            "case {case:?}: a three year term must cost more in total than a one year term, got \
             {three_year} and {one_year}"
        );
        assert!(
            three_year < 3.0 * one_year,
            "case {case:?}: a three year term must be discounted below three one year terms, got \
             {three_year} and {one_year}"
        );
    }
}

#[tokio::test]
async fn the_year_and_second_forms_of_a_term_select_the_same_offerings() {
    // `Duration` is "specified in years or seconds", so the two spellings of
    // one term are the same filter.
    let emulator = Emulator::new();
    for (years, seconds) in state::OFFERING_TERMS {
        let in_years = all_offering_ids(
            &emulator,
            &[("ProductDescription", "redis"), ("Duration", &years.to_string())],
            Some("100"),
        )
        .await;
        let in_seconds = all_offering_ids(
            &emulator,
            &[
                ("ProductDescription", "redis"),
                ("Duration", &seconds.to_string()),
            ],
            Some("100"),
        )
        .await;
        assert!(
            !in_years.is_empty(),
            "term {years}: the catalog must stock the term, got no offerings"
        );
        assert_eq!(
            in_years, in_seconds,
            "term {years}: {years} years and {seconds} seconds must select the same offerings"
        );
    }
}

#[tokio::test]
async fn pagination_partitions_the_whole_catalog_whatever_the_page_size() {
    let emulator = Emulator::new();
    // The axis: the page size, over the published bounds of `MaxRecords` and
    // the default the operation serves when the request names none. The
    // invariant: the pages partition the catalog, in one order, whatever the
    // size of them.
    let at_minimum = all_offering_ids(&emulator, &[], Some("20")).await;
    let at_maximum = all_offering_ids(&emulator, &[], Some("100")).await;
    let by_default = all_offering_ids(&emulator, &[], None).await;

    assert_eq!(
        at_minimum.len(),
        catalog_size(),
        "paging at the minimum page size must return the whole catalog, got {} of {}",
        at_minimum.len(),
        catalog_size()
    );
    assert_eq!(
        at_minimum, at_maximum,
        "the page size must not change which offerings are served, or their order"
    );
    assert_eq!(
        at_maximum, by_default,
        "the default page size must serve the same catalog as an explicit one"
    );

    let mut distinct = at_minimum.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        at_minimum.len(),
        "every offering id must be unique across the catalog, got {} distinct of {}",
        distinct.len(),
        at_minimum.len()
    );
    for id in &at_minimum {
        assert!(
            is_offering_id(id),
            "offering id {id:?} must have the documented 8-4-4-4-12 hex form"
        );
    }
}

#[tokio::test]
async fn an_offering_is_read_back_by_the_id_the_catalog_reported() {
    // The round trip: the id a listing reports is the id a lookup answers to,
    // and the lookup answers with the same offering — the catalog does not
    // change under a reader.
    let cases = [
        (
            "redis",
            "cache.t3.small",
            state::OFFERING_TYPE_NO_UPFRONT,
            state::OFFERING_TERMS[0].0,
        ),
        (
            "memcached",
            "cache.m6g.xlarge",
            state::OFFERING_TYPE_ALL_UPFRONT,
            state::OFFERING_TERMS[1].0,
        ),
        (
            "valkey",
            "cache.r3.large",
            state::OFFERING_TYPE_PARTIAL_UPFRONT,
            state::OFFERING_TERMS[1].0,
        ),
    ];

    let emulator = Emulator::new();
    let mut seen: Vec<String> = Vec::new();
    for (product_description, cache_node_type, offering_type, years) in cases {
        let case = format!("{product_description} / {cache_node_type} / {offering_type}");
        let term = years.to_string();
        let listed = describe(
            &emulator,
            &[
                ("ProductDescription", product_description),
                ("CacheNodeType", cache_node_type),
                ("OfferingType", offering_type),
                ("Duration", &term),
            ],
        )
        .await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: describe must succeed, got {}",
            listed.body
        );
        let ids = offering_ids(&listed);
        assert_eq!(
            ids.len(),
            1,
            "case {case:?}: expected one offering, got {}",
            listed.body
        );
        let id = ids[0].to_string();
        assert!(
            !seen.contains(&id),
            "case {case:?}: each offering must carry its own id, got {id:?} twice"
        );
        seen.push(id.clone());

        let fetched = describe(&emulator, &[("ReservedCacheNodesOfferingId", &id)]).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: a lookup by id must succeed, got {}",
            fetched.body
        );
        assert_eq!(
            offering_ids(&fetched),
            vec![id.as_str()],
            "case {case:?}: a lookup by id must answer with that one offering, got {}",
            fetched.body
        );
        for element in [
            "CacheNodeType",
            "ProductDescription",
            "OfferingType",
            "Duration",
            "FixedPrice",
            "UsagePrice",
            "RecurringChargeAmount",
        ] {
            assert_eq!(
                fetched.all(element),
                listed.all(element),
                "case {case:?}: {element} must be the same on every read"
            );
        }

        // An offering id is opaque, so one that differs in case names no
        // offering and is answered with the modelled not-found rather than an
        // empty page.
        let mistyped = id.to_ascii_uppercase();
        let missing = describe(&emulator, &[("ReservedCacheNodesOfferingId", &mistyped)]).await;
        assert_eq!(
            missing.status, 404,
            "case {case:?}: id {mistyped:?} names no offering, got {}",
            missing.body
        );
        assert_eq!(
            missing.error_code(),
            "ReservedCacheNodesOfferingNotFound",
            "case {case:?}: id {mistyped:?} must be answered with the modelled fault, got {}",
            missing.body
        );
    }
}
