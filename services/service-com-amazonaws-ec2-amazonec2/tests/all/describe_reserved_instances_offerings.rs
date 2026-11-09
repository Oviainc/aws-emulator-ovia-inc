// SPDX-License-Identifier: CC0-1.0

//! What `DescribeReservedInstancesOfferings` publishes, how it is priced, and how a caller
//! narrows and pages it — end to end over the wire the service actually speaks (`ec2Query`,
//! an `application/x-www-form-urlencoded` POST answered with XML, through the crate's shared
//! [`super::ec2_query`] harness).
//!
//! The rules under test:
//!
//! 1. THE PUBLISHED CATALOG is the product of the fields an offering is defined by: every
//!    place the reservation can be used (the Region itself and each Availability Zone of it
//!    that offers the instance type at all), every modelled `RIProductDescription`, every
//!    modelled `OfferingClassType`, each payment option Reserved Instances are sold on and
//!    each term the `duration` filter documents. The places are cross-checked against
//!    `DescribeInstanceTypeOfferings`, so an offering is never published for a zone that does
//!    not offer the type.
//! 2. EVERY PRICE IS COMPUTED from the offering's own terms: a payment option says WHEN the
//!    term is paid for, so for one term the three options split the same total; a longer term
//!    and the `standard` class are discounted further than a shorter one and `convertible`; a
//!    Windows platform costs more than Linux/UNIX on the same hardware, the `(Amazon VPC)`
//!    spelling of a platform costs the same as the plain one, and a type with twice the vCPUs
//!    and memory costs twice as much.
//! 3. WHAT THE REQUEST NAMES NARROWS THE CATALOG and nothing else: the zone (by name or by
//!    id), the instance type, the product description, the offering class, the offering type,
//!    the duration bounds, the offering ids and every documented filter — ANDed with each
//!    other and ORed over one filter's values, with EC2's wildcards.
//! 4. A REQUEST THIS OPERATION CANNOT ANSWER IS REFUSED with the code EC2 answers for the
//!    member at fault, and reports nothing.
//! 5. `MaxResults` AND `NextToken` PAGE the selected offerings: every one of them is reported
//!    exactly once over the walk, each page holds at most the page size, and the last page
//!    carries no token.
//! 6. THE DESCRIBE IS A READ: two calls report the same offerings under the same ids and the
//!    store is untouched — and an id it reports is the one the exchange operations resolve,
//!    "the offering ID used in GetReservedInstancesExchangeQuote to confirm that an exchange
//!    can be made" (`ReservedInstancesOffering$ReservedInstancesOfferingId`).
//!
//! Offering ORDER within a response is deliberately not asserted — "the order of the elements
//! in the response, including those within nested structures, might vary" — so every
//! comparison here is over a set. What the pagination test does rely on is that the order is
//! STABLE across calls, which is what makes a cursor token see each offering exactly once.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, DRY_RUN_OPERATION, INVALID_PAGINATION_TOKEN,
    INVALID_PARAMETER_COMBINATION, INVALID_PARAMETER_VALUE,
    INVALID_RESERVED_INSTANCES_OFFERING_ID, MAX_FILTER_VALUE_BYTES, MAX_FILTER_VALUES, MAX_FILTERS,
    MAX_RESOURCE_IDS, RESERVED_INSTANCE_CURRENCY_CODE, RESERVED_INSTANCE_SCOPE_REGION,
    RESERVED_INSTANCE_SCOPE_ZONE, RESERVED_INSTANCE_STATE_ACTIVE, ReservedInstancesSpec,
    decode_reserved_instances_offering_id,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, elements, error_code, error_message, filters, indexed};
use super::wire::{Param, child_text, document, param, set_items};

/// The Region every call here is signed for: an offering is published for the Region the
/// request reached, and its places are that Region's own zones.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers `DryRunOperation` with: the request was refused because a
/// precondition the CALLER set holds, not because it was malformed.
const DRY_RUN_STATUS: u16 = 412;

/// The form-field prefix ec2Query flattens `ReservedInstancesOfferingIds` into: the member's
/// `@xmlName`
/// (`smithy_query(query="operation",operation="DescribeReservedInstancesOfferings")` gives
/// `ReservedInstancesOfferingId`).
const IDS: &str = "ReservedInstancesOfferingId";

/// A one-year term in seconds and a three-year one: the two values the `duration` filter
/// documents (`31536000` | `94608000`).
const ONE_YEAR_SECONDS: i64 = 31_536_000;

/// The other of those two terms.
const THREE_YEAR_SECONDS: i64 = 94_608_000;

/// The seconds in an hour, for turning a term into the hours its usage price is charged for.
const SECONDS_PER_HOUR: f64 = 3_600.0;

/// The payment options Reserved Instances are sold on, as the modelled `OfferingTypeValues`
/// enum spells them.
const ALL_UPFRONT: &str = "All Upfront";

/// The option that pays part at purchase and the rest per hour.
const PARTIAL_UPFRONT: &str = "Partial Upfront";

/// The option that pays nothing at purchase.
const NO_UPFRONT: &str = "No Upfront";

/// The two members of the modelled `OfferingClassType` enum.
const STANDARD: &str = "standard";

/// The class that can be exchanged for another offering.
const CONVERTIBLE: &str = "convertible";

/// The product description most rows here use, one of the four members of the modelled
/// `RIProductDescription` enum.
const LINUX_UNIX: &str = "Linux/UNIX";

/// A Windows platform of the same enum, which carries an operating-system licence.
const WINDOWS: &str = "Windows";

/// The current-generation instance type the catalog table is read for.
const M5_LARGE: &str = "m5.large";

/// The same family at twice the size: twice the vCPUs and twice the memory.
const M5_XLARGE: &str = "m5.xlarge";

/// A PREVIOUS-generation instance type, which every zone of a Region offers — unlike a
/// current-generation one, which the Region's last zone does not
/// (`crate::state::location_offers_instance_type`). Not cited by the issue this suite closes.
const M1_SMALL: &str = "m1.small";

/// How far two amounts this suite compares may differ: the prices are `Float` members, so a
/// total built from a purchase price and an hourly price times the hours of a term carries the
/// rounding of a binary32 value and of the cent the purchase price is quoted to.
const AMOUNT_TOLERANCE: f64 = 0.05;

/// One offering as a response carries it.
#[derive(Debug, Clone, PartialEq)]
struct Offering {
    offering_id: String,
    instance_type: String,
    availability_zone: Option<String>,
    availability_zone_id: Option<String>,
    duration: i64,
    fixed_price: f64,
    usage_price: f64,
    product_description: String,
    offering_class: String,
    offering_type: String,
    scope: String,
    currency_code: String,
    instance_tenancy: String,
    marketplace: String,
    pricing_details: usize,
    recurring_charges: usize,
}

impl Offering {
    /// What the whole term costs: what is due at purchase plus the hourly price for every hour
    /// of the term.
    fn total(&self) -> f64 {
        self.fixed_price + self.usage_price * (self.duration as f64 / SECONDS_PER_HOUR)
    }

    /// The fields that identify this offering within one search: where it can be used, its
    /// class, its payment option and its term.
    fn key(&self) -> (String, String, String, i64) {
        (
            self.availability_zone.clone().unwrap_or_default(),
            self.offering_class.clone(),
            self.offering_type.clone(),
            self.duration,
        )
    }
}

/// One `DescribeReservedInstancesOfferings` call carrying `params`, as its HTTP status and XML
/// body.
async fn describe(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    call(
        router,
        REGION,
        "DescribeReservedInstancesOfferings",
        params,
    )
    .await
}

/// Every offering a response reports, read off the document so a nested member is never taken
/// for one of the offering's own.
fn offerings(case: &str, body: &str) -> Vec<Offering> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), "reservedInstancesOfferingsSet")
        .into_iter()
        .map(|item| {
            let text = |name: &str| child_text(item, name);
            let required = |name: &str| {
                text(name).unwrap_or_else(|| {
                    panic!("case {case:?}: an offering must carry {name}: {body}")
                })
            };
            let number = |name: &str| -> f64 {
                let value = required(name);
                value.parse::<f64>().unwrap_or_else(|_| {
                    panic!("case {case:?}: {name} must be a number, got {value:?}")
                })
            };
            Offering {
                offering_id: required("reservedInstancesOfferingId"),
                instance_type: required("instanceType"),
                availability_zone: text("availabilityZone"),
                availability_zone_id: text("availabilityZoneId"),
                duration: number("duration") as i64,
                fixed_price: number("fixedPrice"),
                usage_price: number("usagePrice"),
                product_description: required("productDescription"),
                offering_class: required("offeringClass"),
                offering_type: required("offeringType"),
                scope: required("scope"),
                currency_code: required("currencyCode"),
                instance_tenancy: required("instanceTenancy"),
                marketplace: required("marketplace"),
                pricing_details: set_items(item, "pricingDetailsSet").len(),
                recurring_charges: set_items(item, "recurringCharges").len(),
            }
        })
        .collect()
}

/// One call that must be served, as the offerings it reported — and as the WHOLE of what the
/// search selects: the call is asserted to carry no `NextToken`, so a table that compares a
/// reported set against an expected one can never be comparing only the first page of it.
async fn reported(router: &MultiServiceRouter, case: &str, params: &[Param]) -> Vec<Offering> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        super::ec2_query::element(&body, "nextToken"),
        None,
        "case {case:?}: this search fits one page, so no token may follow it: {body}"
    );
    offerings(case, &body)
}

/// The same call, WITHOUT that assertion: a search whose answer may fill a page — the whole
/// catalog, or a derived filter over it — is read through this instead.
async fn reported_page(router: &MultiServiceRouter, case: &str, params: &[Param]) -> Vec<Offering> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    offerings(case, &body)
}

/// Every offering a request reports over a whole `NextToken` walk, with the number of pages it
/// took; each page is asserted to hold at most `page_size` offerings when one is named.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    params: &[Param],
    page_size: Option<usize>,
) -> (Vec<Offering>, usize) {
    let mut seen: Vec<Offering> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut call_params = params.to_vec();
        if let Some(page_size) = page_size {
            call_params.push(param("MaxResults", &page_size.to_string()));
        }
        if let Some(token) = &token {
            call_params.push(param("NextToken", token));
        }
        let (status, body) = describe(router, &call_params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 on page {}, got {status} with body {body}",
            pages + 1
        );
        let page = offerings(case, &body);
        if let Some(page_size) = page_size {
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most {page_size} offerings, got {}",
                page.len()
            );
        }
        seen.extend(page);
        pages += 1;
        assert!(
            pages <= 1_000,
            "case {case:?}: the walk must terminate, got {pages} pages"
        );
        token = super::ec2_query::element(&body, "nextToken");
        if token.is_none() {
            return (seen, pages);
        }
    }
}

/// The form fields selecting one instance type and one product description — the search every
/// table below is read against, which is small enough to fit one page.
fn one_type_and_product(instance_type: &str, product_description: &str) -> Vec<Param> {
    vec![
        param("InstanceType", instance_type),
        param("ProductDescription", product_description),
    ]
}

/// The Availability Zones that offer `instance_type`, as `DescribeInstanceTypeOfferings`
/// reports them — the other operation that publishes what a zone offers, so the places this
/// operation publishes offerings for are cross-checked against it rather than against a list
/// written here.
async fn zones_offering(
    router: &MultiServiceRouter,
    case: &str,
    instance_type: &str,
) -> BTreeSet<String> {
    let mut params = vec![param("LocationType", "availability-zone")];
    params.extend(filters(&[("instance-type", &[instance_type])]));
    let (status, body) = call(router, REGION, "DescribeInstanceTypeOfferings", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInstanceTypeOfferings must report where {instance_type} is \
         offered, got {status} with body {body}"
    );
    elements(&body, "location").into_iter().collect()
}

/// The places offerings of `instance_type` are expected to be published for: the Region itself
/// — reported with no Availability Zone at all — and each zone that offers the type.
async fn expected_places(
    router: &MultiServiceRouter,
    case: &str,
    instance_type: &str,
) -> BTreeSet<String> {
    let mut places: BTreeSet<String> = zones_offering(router, case, instance_type).await;
    places.insert(String::new());
    places
}

/// Every key the published catalog is expected to hold for one instance type and product
/// description: each place, on each class, on each payment option, for each term.
async fn expected_keys(
    router: &MultiServiceRouter,
    case: &str,
    instance_type: &str,
) -> BTreeSet<(String, String, String, i64)> {
    let places = expected_places(router, case, instance_type).await;
    let mut keys = BTreeSet::new();
    for place in &places {
        for class in [STANDARD, CONVERTIBLE] {
            for payment_option in [ALL_UPFRONT, PARTIAL_UPFRONT, NO_UPFRONT] {
                for duration in [ONE_YEAR_SECONDS, THREE_YEAR_SECONDS] {
                    keys.insert((
                        place.clone(),
                        class.to_string(),
                        payment_option.to_string(),
                        duration,
                    ));
                }
            }
        }
    }
    keys
}

// ---------------------------------------------------------------------------
// Rule 1: the published catalog
// ---------------------------------------------------------------------------

/// The catalog published for one instance type and product description is exactly every place
/// it can be used × every offering class × every payment option × both terms, each entry
/// carrying the members the service itself owns.
///
/// Read for a CURRENT-generation type, whose last zone does not offer it, and for a PREVIOUS
/// -generation one, which every zone offers — so the places are the zones that really offer the
/// type rather than every zone of the Region.
#[tokio::test]
async fn the_catalog_is_every_place_class_payment_option_and_term() {
    let (router, _engine) = super::ec2_query::emulator();

    for instance_type in [M5_LARGE, M1_SMALL] {
        let case = format!("the catalog for {instance_type}");
        let reported_offerings =
            reported(&router, &case, &one_type_and_product(instance_type, LINUX_UNIX)).await;
        let keys: BTreeSet<(String, String, String, i64)> =
            reported_offerings.iter().map(Offering::key).collect();
        let expected = expected_keys(&router, &case, instance_type).await;
        assert_eq!(
            keys.len(),
            reported_offerings.len(),
            "case {case:?}: each place, class, payment option and term is offered once, got {} \
             offerings for {} keys",
            reported_offerings.len(),
            keys.len()
        );
        assert_eq!(
            keys, expected,
            "case {case:?}: expected exactly the published product; missing {:?}, unexpected {:?}",
            expected.difference(&keys).collect::<Vec<_>>(),
            keys.difference(&expected).collect::<Vec<_>>()
        );

        let ids: BTreeSet<&String> = reported_offerings
            .iter()
            .map(|offering| &offering.offering_id)
            .collect();
        assert_eq!(
            ids.len(),
            reported_offerings.len(),
            "case {case:?}: every offering carries its own id, got {} ids for {} offerings",
            ids.len(),
            reported_offerings.len()
        );

        for offering in &reported_offerings {
            assert_eq!(
                offering.instance_type, instance_type,
                "case {case:?}: expected the instance type the request named, got {offering:?}"
            );
            assert_eq!(
                offering.product_description, LINUX_UNIX,
                "case {case:?}: expected the product description the request named, got \
                 {offering:?}"
            );
            assert_eq!(
                offering.currency_code, RESERVED_INSTANCE_CURRENCY_CODE,
                "case {case:?}: the only modelled currency is {RESERVED_INSTANCE_CURRENCY_CODE}, \
                 got {offering:?}"
            );
            assert_eq!(
                offering.instance_tenancy, "default",
                "case {case:?}: expected the default tenancy, got {offering:?}"
            );
            assert_eq!(
                offering.marketplace, "false",
                "case {case:?}: this service publishes no Marketplace offering, got {offering:?}"
            );
            assert_eq!(
                offering.pricing_details, 0,
                "case {case:?}: pricing details belong to a Marketplace offering, got {offering:?}"
            );
            assert_eq!(
                offering.recurring_charges, 0,
                "case {case:?}: the hourly price is the usage price, so no recurring charge is \
                 assigned, got {offering:?}"
            );
            // The scope follows from whether the offering names a zone, and a zonal offering
            // names that zone's id beside its name.
            match &offering.availability_zone {
                Some(zone_name) => {
                    assert_eq!(
                        offering.scope, RESERVED_INSTANCE_SCOPE_ZONE,
                        "case {case:?}: an offering naming a zone is zonal, got {offering:?}"
                    );
                    let zone_id = offering.availability_zone_id.clone().unwrap_or_default();
                    let expected_id = ec2::state::availability_zones_in_region(REGION)
                        .into_iter()
                        .find(|zone| &zone.zone_name == zone_name)
                        .map(|zone| zone.zone_id)
                        .unwrap_or_default();
                    assert_eq!(
                        zone_id, expected_id,
                        "case {case:?}: expected the id of {zone_name}, got {offering:?}"
                    );
                }
                None => {
                    assert_eq!(
                        offering.scope, RESERVED_INSTANCE_SCOPE_REGION,
                        "case {case:?}: an offering naming no zone is Regional, got {offering:?}"
                    );
                    assert_eq!(
                        offering.availability_zone_id, None,
                        "case {case:?}: a Regional offering names no zone id, got {offering:?}"
                    );
                }
            }
            // The id carries the offering's terms, which is what lets a purchase or an
            // exchange resolve an offering a describe reported.
            let terms = decode_reserved_instances_offering_id(&offering.offering_id)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: an offering id must carry its terms, got {offering:?}")
                });
            assert_eq!(
                (
                    terms.instance_type.as_str(),
                    terms.availability_zone.clone(),
                    terms.product_description.as_str(),
                    terms.offering_class.as_str(),
                    terms.offering_type.as_str(),
                    terms.duration_seconds,
                ),
                (
                    offering.instance_type.as_str(),
                    offering.availability_zone.clone(),
                    offering.product_description.as_str(),
                    offering.offering_class.as_str(),
                    offering.offering_type.as_str(),
                    offering.duration,
                ),
                "case {case:?}: the id must carry the terms the offering was reported with, got \
                 {offering:?} for {terms:?}"
            );
        }
    }
}

/// The three product descriptions and the legacy offering types the catalog treats
/// differently from the rows above: every modelled `RIProductDescription` is published, and a
/// pre-`2011-11-01` offering type names no current offering.
#[tokio::test]
async fn every_modelled_product_description_is_published_and_a_legacy_offering_type_is_not() {
    let (router, _engine) = super::ec2_query::emulator();

    // Each value of the modelled `RIProductDescription` enum
    // (`smithy_query(query="shape",shape="RIProductDescription")`), read from the generated
    // value list so the assertion is over the whole class rather than a sample.
    for product_description in ec2::ops::types::R_I_PRODUCT_DESCRIPTION_VALUES {
        let case = format!("the catalog for {product_description}");
        let reported_offerings = reported(
            &router,
            &case,
            &one_type_and_product(M5_LARGE, product_description),
        )
        .await;
        assert!(
            !reported_offerings.is_empty(),
            "case {case:?}: every modelled product description is offered, got none"
        );
        for offering in &reported_offerings {
            assert_eq!(
                &offering.product_description, product_description,
                "case {case:?}: expected the product description the request named, got \
                 {offering:?}"
            );
        }
    }

    // "If you are using tools that predate the 2011-11-01 API version, you only have access to
    // the Medium Utilization Reserved Instance offering type" (`$OfferingType`): the three
    // Utilization members of the modelled enum are those older offering types, and no current
    // offering is published on one.
    for offering_type in ["Heavy Utilization", "Light Utilization", "Medium Utilization"] {
        let case = format!("the legacy offering type {offering_type}");
        let mut params = one_type_and_product(M5_LARGE, LINUX_UNIX);
        params.push(param("OfferingType", offering_type));
        let reported_offerings = reported(&router, &case, &params).await;
        assert!(
            reported_offerings.is_empty(),
            "case {case:?}: expected no current offering on a pre-2011-11-01 offering type, got \
             {reported_offerings:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: what the terms say about the price
// ---------------------------------------------------------------------------

/// For one term, the three payment options split the SAME total, each the way its own name
/// says; a longer term and the `standard` class are discounted further; and the discount is
/// taken off a price built from the instance type's own size and platform.
#[tokio::test]
async fn the_payment_option_splits_the_term_and_the_terms_set_the_total() {
    let (router, _engine) = super::ec2_query::emulator();
    let case = "the prices of one search";
    let reported_offerings =
        reported(&router, case, &one_type_and_product(M5_LARGE, LINUX_UNIX)).await;

    // The Regional offerings of that search, by (class, term, payment option): one place is
    // enough for the price rules, and the Region's own offerings are the ones every zone's
    // repeat.
    let regional: BTreeMap<(String, i64, String), Offering> = reported_offerings
        .iter()
        .filter(|offering| offering.availability_zone.is_none())
        .map(|offering| {
            (
                (
                    offering.offering_class.clone(),
                    offering.duration,
                    offering.offering_type.clone(),
                ),
                offering.clone(),
            )
        })
        .collect();
    let priced = |class: &str, duration: i64, payment_option: &str| -> Offering {
        regional
            .get(&(class.to_string(), duration, payment_option.to_string()))
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a Regional {class} {payment_option} offering of \
                     {duration} seconds, got {} offerings",
                    regional.len()
                )
            })
    };

    // Rows differ along what the rule generalises over: both classes and both terms.
    for (class, duration) in [
        (STANDARD, ONE_YEAR_SECONDS),
        (STANDARD, THREE_YEAR_SECONDS),
        (CONVERTIBLE, ONE_YEAR_SECONDS),
        (CONVERTIBLE, THREE_YEAR_SECONDS),
    ] {
        let case = format!("{class} offerings of {duration} seconds");
        let all = priced(class, duration, ALL_UPFRONT);
        let partial = priced(class, duration, PARTIAL_UPFRONT);
        let none = priced(class, duration, NO_UPFRONT);

        assert_eq!(
            all.usage_price, 0.0,
            "case {case:?}: All Upfront charges nothing per hour, got {all:?}"
        );
        assert!(
            all.fixed_price > 0.0,
            "case {case:?}: All Upfront charges the term at purchase, got {all:?}"
        );
        assert_eq!(
            none.fixed_price, 0.0,
            "case {case:?}: No Upfront charges nothing at purchase, got {none:?}"
        );
        assert!(
            none.usage_price > 0.0,
            "case {case:?}: No Upfront charges the term per hour, got {none:?}"
        );
        assert!(
            partial.fixed_price > 0.0 && partial.usage_price > 0.0,
            "case {case:?}: Partial Upfront charges both ways, got {partial:?}"
        );
        for other in [&partial, &none] {
            let difference = (all.total() - other.total()).abs();
            assert!(
                difference <= AMOUNT_TOLERANCE,
                "case {case:?}: the payment options split the same total; All Upfront {} vs {} \
                 {} (difference {difference})",
                all.total(),
                other.offering_type,
                other.total()
            );
        }
        assert!(
            (partial.usage_price * 2.0 - none.usage_price).abs() <= AMOUNT_TOLERANCE,
            "case {case:?}: Partial Upfront charges half the hourly price No Upfront does, got \
             {} and {}",
            partial.usage_price,
            none.usage_price
        );
    }

    // A longer term is discounted further, so its hourly rate over the whole term is lower.
    for class in [STANDARD, CONVERTIBLE] {
        let short = priced(class, ONE_YEAR_SECONDS, ALL_UPFRONT);
        let long = priced(class, THREE_YEAR_SECONDS, ALL_UPFRONT);
        let short_rate = short.total() / (short.duration as f64 / SECONDS_PER_HOUR);
        let long_rate = long.total() / (long.duration as f64 / SECONDS_PER_HOUR);
        assert!(
            long_rate < short_rate,
            "case {class:?}: a three-year term is discounted further than a one-year one, got \
             {long_rate} per hour against {short_rate}"
        );
    }

    // A convertible offering — one that may be exchanged — is discounted less than a standard
    // one of the same term.
    for duration in [ONE_YEAR_SECONDS, THREE_YEAR_SECONDS] {
        let standard = priced(STANDARD, duration, ALL_UPFRONT);
        let convertible = priced(CONVERTIBLE, duration, ALL_UPFRONT);
        assert!(
            convertible.total() > standard.total(),
            "case {duration}: a convertible term costs more than a standard one, got {} against \
             {}",
            convertible.total(),
            standard.total()
        );
    }
}

/// The price is built from the instance type's own size and its platform: twice the hardware
/// costs twice as much, a Windows platform costs more than Linux/UNIX on the same hardware,
/// and the `(Amazon VPC)` spelling of a platform costs what the plain one does.
#[tokio::test]
async fn the_price_follows_the_instance_type_and_the_platform() {
    let (router, _engine) = super::ec2_query::emulator();

    // One Regional All Upfront one-year offering of a type and a platform: the price every row
    // below compares.
    async fn purchase_price(
        router: &MultiServiceRouter,
        case: &str,
        instance_type: &str,
        product_description: &str,
    ) -> f64 {
        let mut params = one_type_and_product(instance_type, product_description);
        params.push(param("OfferingClass", STANDARD));
        params.push(param("OfferingType", ALL_UPFRONT));
        params.push(param("MinDuration", &ONE_YEAR_SECONDS.to_string()));
        params.push(param("MaxDuration", &ONE_YEAR_SECONDS.to_string()));
        let reported_offerings = reported(router, case, &params).await;
        let offering = reported_offerings
            .iter()
            .find(|offering| offering.availability_zone.is_none())
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected a Regional offering of {instance_type} on \
                     {product_description}, got {reported_offerings:?}"
                )
            });
        offering.fixed_price
    }

    let large = purchase_price(&router, "m5.large on Linux/UNIX", M5_LARGE, LINUX_UNIX).await;
    let xlarge = purchase_price(&router, "m5.xlarge on Linux/UNIX", M5_XLARGE, LINUX_UNIX).await;
    let windows = purchase_price(&router, "m5.large on Windows", M5_LARGE, WINDOWS).await;
    let linux_vpc = purchase_price(
        &router,
        "m5.large on Linux/UNIX (Amazon VPC)",
        M5_LARGE,
        "Linux/UNIX (Amazon VPC)",
    )
    .await;
    let windows_vpc = purchase_price(
        &router,
        "m5.large on Windows (Amazon VPC)",
        M5_LARGE,
        "Windows (Amazon VPC)",
    )
    .await;

    assert!(
        (xlarge - large * 2.0).abs() <= AMOUNT_TOLERANCE.max(large * 0.001),
        "case \"twice the hardware\": m5.xlarge has twice the vCPUs and memory of m5.large, so \
         it costs twice as much; got {xlarge} against {large}"
    );
    assert!(
        windows > large,
        "case \"the Windows licence\": a Windows platform costs more than Linux/UNIX on the same \
         hardware, got {windows} against {large}"
    );
    assert!(
        (linux_vpc - large).abs() <= AMOUNT_TOLERANCE,
        "case \"the VPC spelling of Linux/UNIX\": the (Amazon VPC) spelling says where the \
         reservation applies, not what it runs, got {linux_vpc} against {large}"
    );
    assert!(
        (windows_vpc - windows).abs() <= AMOUNT_TOLERANCE,
        "case \"the VPC spelling of Windows\": the (Amazon VPC) spelling says where the \
         reservation applies, not what it runs, got {windows_vpc} against {windows}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: what narrows the catalog
// ---------------------------------------------------------------------------

/// The rule: what the request names narrows the published catalog, and nothing else.
///
/// Each row names a narrowing and the predicate the offerings it selects satisfy; the expected
/// set is the base search's own catalog filtered by that predicate, so a row that reports too
/// little fails as loudly as one that reports too much. The rows span what the rule generalises
/// over: each narrowing MEMBER, each documented filter, both spellings of a zone, an exact
/// value and a wildcard, one filter carrying two values, two filters that must both hold, and
/// narrowings that select nothing.
#[tokio::test]
async fn what_the_request_names_narrows_the_catalog() {
    let (router, _engine) = super::ec2_query::emulator();
    let case = "the base search";
    let base = reported(&router, case, &one_type_and_product(M5_LARGE, LINUX_UNIX)).await;
    assert!(
        base.len() > 12,
        "case {case:?}: the base search should cover several places and terms, got {}",
        base.len()
    );
    let zones = ec2::state::availability_zones_in_region(REGION);
    let first_zone = zones[0].clone();
    let second_zone = zones[1].clone();
    // One offering's own id and prices, read off the response rather than written here, so the
    // rows pin the rule instead of one run's arithmetic.
    let sample = base
        .iter()
        .find(|offering| {
            offering.availability_zone.is_none()
                && offering.offering_type == PARTIAL_UPFRONT
                && offering.duration == ONE_YEAR_SECONDS
                && offering.offering_class == STANDARD
        })
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected a standard Partial Upfront offering"));
    let sample_fixed = sample.fixed_price.to_string();
    let sample_usage = sample.usage_price.to_string();

    type Row = (&'static str, Vec<Param>, fn(&Offering) -> bool);
    let rows: Vec<Row> = vec![
        ("no further narrowing", Vec::new(), |_| true),
        (
            "the convertible offering class",
            vec![param("OfferingClass", CONVERTIBLE)],
            |offering| offering.offering_class == CONVERTIBLE,
        ),
        (
            "one payment option",
            vec![param("OfferingType", NO_UPFRONT)],
            |offering| offering.offering_type == NO_UPFRONT,
        ),
        (
            "a zone by name",
            vec![param("AvailabilityZone", "us-east-1a")],
            |offering| offering.availability_zone.as_deref() == Some("us-east-1a"),
        ),
        (
            "a zone by id",
            vec![param("AvailabilityZoneId", "use1-az2")],
            |offering| offering.availability_zone.as_deref() == Some("us-east-1b"),
        ),
        (
            "a minimum duration that admits only the longer term",
            vec![param("MinDuration", &THREE_YEAR_SECONDS.to_string())],
            |offering| offering.duration == THREE_YEAR_SECONDS,
        ),
        (
            "a maximum duration that admits only the shorter term",
            vec![param("MaxDuration", &ONE_YEAR_SECONDS.to_string())],
            |offering| offering.duration == ONE_YEAR_SECONDS,
        ),
        (
            "a minimum duration past every term",
            vec![param("MinDuration", &(THREE_YEAR_SECONDS + 1).to_string())],
            |_| false,
        ),
        (
            "the default tenancy, which is the tenancy published",
            vec![param("InstanceTenancy", "default")],
            |_| true,
        ),
        (
            "a tenancy no offering is published for",
            vec![param("InstanceTenancy", "dedicated")],
            |_| false,
        ),
        (
            "including Marketplace offerings, of which there are none to add",
            vec![param("IncludeMarketplace", "true")],
            |_| true,
        ),
        (
            "a maximum instance count, which selects among Marketplace offerings only",
            vec![param("MaxInstanceCount", "5")],
            |_| true,
        ),
        (
            "the availability-zone filter",
            filters(&[("availability-zone", &["us-east-1a"])]),
            |offering| offering.availability_zone.as_deref() == Some("us-east-1a"),
        ),
        (
            "the availability-zone filter with a wildcard",
            filters(&[("availability-zone", &["us-east-1*"])]),
            |offering| offering.availability_zone.is_some(),
        ),
        (
            "the availability-zone-id filter",
            filters(&[("availability-zone-id", &["use1-az1"])]),
            |offering| offering.availability_zone.as_deref() == Some("us-east-1a"),
        ),
        (
            "the scope filter naming the Region",
            filters(&[("scope", &[RESERVED_INSTANCE_SCOPE_REGION])]),
            |offering| offering.availability_zone.is_none(),
        ),
        (
            "the scope filter naming a zone",
            filters(&[("scope", &[RESERVED_INSTANCE_SCOPE_ZONE])]),
            |offering| offering.availability_zone.is_some(),
        ),
        (
            "the duration filter",
            filters(&[("duration", &["31536000"])]),
            |offering| offering.duration == ONE_YEAR_SECONDS,
        ),
        (
            "the duration filter carrying both terms",
            filters(&[("duration", &["31536000", "94608000"])]),
            |_| true,
        ),
        (
            "the instance-type filter",
            filters(&[("instance-type", &[M5_LARGE])]),
            |_| true,
        ),
        (
            "the instance-type filter naming another type",
            filters(&[("instance-type", &[M5_XLARGE])]),
            |_| false,
        ),
        (
            "the product-description filter",
            filters(&[("product-description", &[LINUX_UNIX])]),
            |_| true,
        ),
        (
            "the product-description filter naming another platform",
            filters(&[("product-description", &[WINDOWS])]),
            |_| false,
        ),
        (
            "the marketplace filter asking for Amazon Web Services offerings",
            filters(&[("marketplace", &["false"])]),
            |_| true,
        ),
        (
            "the marketplace filter asking for resale offerings",
            filters(&[("marketplace", &["true"])]),
            |_| false,
        ),
        (
            "two filters that must both hold",
            filters(&[
                ("scope", &[RESERVED_INSTANCE_SCOPE_ZONE]),
                ("duration", &["94608000"]),
            ]),
            |offering| {
                offering.availability_zone.is_some() && offering.duration == THREE_YEAR_SECONDS
            },
        ),
        (
            "a zone member ANDed with a scope filter that excludes it",
            {
                let mut params = vec![param("AvailabilityZone", "us-east-1a")];
                params.extend(filters(&[("scope", &[RESERVED_INSTANCE_SCOPE_REGION])]));
                params
            },
            |_| false,
        ),
    ];

    for (row, extra, admits) in rows {
        let mut params = one_type_and_product(M5_LARGE, LINUX_UNIX);
        params.extend(extra);
        let actual: BTreeSet<(String, String, String, i64)> = reported(&router, row, &params)
            .await
            .iter()
            .map(Offering::key)
            .collect();
        let expected: BTreeSet<(String, String, String, i64)> = base
            .iter()
            .filter(|offering| admits(offering))
            .map(Offering::key)
            .collect();
        assert_eq!(
            actual, expected,
            "case {row:?}: expected exactly the offerings the narrowing selects; missing {:?}, \
             unexpected {:?}",
            expected.difference(&actual).collect::<Vec<_>>(),
            actual.difference(&expected).collect::<Vec<_>>()
        );
    }

    // The two price filters and the offering-id filter compare against values the service
    // DERIVES, so each is read back off the response and handed straight to the filter.
    let price_rows: Vec<(String, Vec<Param>, usize)> = vec![
        (
            format!("the fixed-price filter for {sample_fixed}"),
            filters(&[("fixed-price", &[sample_fixed.as_str()])]),
            base.iter()
                .filter(|offering| offering.fixed_price == sample.fixed_price)
                .count(),
        ),
        (
            format!("the usage-price filter for {sample_usage}"),
            filters(&[("usage-price", &[sample_usage.as_str()])]),
            base.iter()
                .filter(|offering| offering.usage_price == sample.usage_price)
                .count(),
        ),
        (
            "the reserved-instances-offering-id filter".to_string(),
            filters(&[(
                "reserved-instances-offering-id",
                &[sample.offering_id.as_str()],
            )]),
            1,
        ),
        (
            "the offering ids member".to_string(),
            indexed(IDS, &[sample.offering_id.as_str()]),
            1,
        ),
    ];
    for (row, extra, expected_count) in price_rows {
        let mut params = one_type_and_product(M5_LARGE, LINUX_UNIX);
        params.extend(extra);
        let actual = reported(&router, &row, &params).await;
        assert_eq!(
            actual.len(),
            expected_count,
            "case {row:?}: expected {expected_count} offerings, got {actual:?}"
        );
        assert!(
            !actual.is_empty(),
            "case {row:?}: a value read off the response must select the offering it came from"
        );
    }

    // Two zones' offerings differ only in where they can be used, which is what makes the two
    // spellings of a zone select the same place.
    for (member, value, zone) in [
        ("AvailabilityZone", first_zone.zone_name.clone(), &first_zone),
        ("AvailabilityZoneId", first_zone.zone_id.clone(), &first_zone),
        (
            "AvailabilityZone",
            second_zone.zone_name.clone(),
            &second_zone,
        ),
        (
            "AvailabilityZoneId",
            second_zone.zone_id.clone(),
            &second_zone,
        ),
    ] {
        let row = format!("{member}={value}");
        let mut params = one_type_and_product(M5_LARGE, LINUX_UNIX);
        params.push(param(member, &value));
        let actual = reported(&router, &row, &params).await;
        assert!(
            !actual.is_empty(),
            "case {row:?}: expected the offerings of {}, got none",
            zone.zone_name
        );
        for offering in &actual {
            assert_eq!(
                offering.availability_zone.as_deref(),
                Some(zone.zone_name.as_str()),
                "case {row:?}: expected offerings of {}, got {offering:?}",
                zone.zone_name
            );
            assert_eq!(
                offering.availability_zone_id.as_deref(),
                Some(zone.zone_id.as_str()),
                "case {row:?}: expected the id of {}, got {offering:?}",
                zone.zone_name
            );
        }
    }
}

/// A filter on a DERIVED value selects over the whole published catalog, not only over a
/// search the request has already narrowed — and where evaluating it over that whole catalog
/// would cost more comparisons than this service spends on one request, the request is TOLD
/// so rather than answered from a scan that stopped half way.
///
/// The price read off one offering of a narrowed call is handed back as the only narrowing, so
/// the filter is answered against every offering the Region publishes. The offering id is the
/// other derived value, and it is long: the matcher charges per character of a filter value,
/// so an id compared against the whole product passes this crate's one matching budget and is
/// refused with `RequestLimitExceeded` — naming the offerings through
/// `ReservedInstancesOfferingIds`, or narrowing the search first (the row of the table above),
/// is the exact way to select by id.
#[tokio::test]
async fn a_derived_filter_selects_over_the_whole_catalog() {
    let (router, _engine) = super::ec2_query::emulator();
    let case = "one offering to read a derived value off";
    let sample = reported(&router, case, &one_type_and_product(M5_LARGE, LINUX_UNIX))
        .await
        .into_iter()
        .find(|offering| {
            offering.availability_zone.is_none()
                && offering.offering_class == STANDARD
                && offering.offering_type == PARTIAL_UPFRONT
                && offering.duration == ONE_YEAR_SECONDS
        })
        .unwrap_or_else(|| panic!("case {case:?}: expected a standard Partial Upfront offering"));

    let row = "the fixed-price filter over the whole catalog";
    let actual = reported_page(
        &router,
        row,
        &filters(&[("fixed-price", &[sample.fixed_price.to_string().as_str()])]),
    )
    .await;
    assert!(
        actual
            .iter()
            .any(|offering| offering.offering_id == sample.offering_id),
        "case {row:?}: expected the offering the price came from ({}) among {} reported",
        sample.offering_id,
        actual.len()
    );
    for offering in &actual {
        assert_eq!(
            offering.fixed_price, sample.fixed_price,
            "case {row:?}: every reported offering carries the price the filter named, got \
             {offering:?}"
        );
    }

    let row = "the reserved-instances-offering-id filter over the whole catalog";
    let (status, body) = describe(
        &router,
        &filters(&[(
            "reserved-instances-offering-id",
            &[sample.offering_id.as_str()],
        )]),
    )
    .await;
    assert_eq!(
        status, 503,
        "case {row:?}: expected the request-limit refusal, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "RequestLimitExceeded",
        "case {row:?}: expected RequestLimitExceeded, got {body}"
    );
    assert!(
        super::ec2_query::element(&body, "reservedInstancesOfferingsSet").is_none(),
        "case {row:?}: a refusal reports no offerings, got {body}"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: what the describe refuses
// ---------------------------------------------------------------------------
/// The rule: a request this operation cannot answer is refused with the code EC2 answers for
/// the member at fault, rather than reported as an empty result — and nothing is reported.
///
/// The rows span the classes of value it refuses: both spellings of a zone at once, a zone the
/// Region does not have (by either spelling), the one tenancy the member excludes, an
/// undocumented filter, each request-controlled ceiling this crate holds a describe to, an
/// offering id this service never minted, and a pagination token it never minted.
#[tokio::test]
async fn a_request_the_catalog_cannot_answer_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    let oversize_value = "a".repeat(MAX_FILTER_VALUE_BYTES + 1);
    let too_many_ids: Vec<String> = (0..=MAX_RESOURCE_IDS)
        .map(|index| format!("rio-{index:016x}"))
        .collect();
    let too_many_values: Vec<String> = (0..=MAX_FILTER_VALUES)
        .map(|index| format!("us-east-1{index}"))
        .collect();

    // (case, request fields, the code expected, the message expected when it is fixed text)
    let rows: Vec<(&str, Vec<Param>, &str, Option<String>)> = vec![
        (
            "both spellings of a zone at once",
            vec![
                param("AvailabilityZone", "us-east-1a"),
                param("AvailabilityZoneId", "use1-az1"),
            ],
            INVALID_PARAMETER_COMBINATION,
            Some(
                "Either availabilityZone or availabilityZoneId can be specified, but not both"
                    .to_string(),
            ),
        ),
        (
            "a zone name the Region does not have",
            vec![param("AvailabilityZone", "us-west-2a")],
            INVALID_PARAMETER_VALUE,
            Some(
                "Invalid value 'us-west-2a' for AvailabilityZone. Region 'us-east-1' has no such \
                 Availability Zone"
                    .to_string(),
            ),
        ),
        (
            "a zone id the Region does not have",
            vec![param("AvailabilityZoneId", "usw2-az1")],
            INVALID_PARAMETER_VALUE,
            Some(
                "Invalid value 'usw2-az1' for AvailabilityZoneId. Region 'us-east-1' has no such \
                 Availability Zone"
                    .to_string(),
            ),
        ),
        (
            "the one tenancy this parameter excludes",
            vec![param("InstanceTenancy", "host")],
            INVALID_PARAMETER_VALUE,
            Some(
                "Invalid value 'host' for InstanceTenancy. Use the default or dedicated values \
                 only"
                    .to_string(),
            ),
        ),
        (
            "a filter this operation does not document",
            filters(&[("instance-tenancy", &["default"])]),
            INVALID_PARAMETER_VALUE,
            Some("The filter 'instance-tenancy' is invalid".to_string()),
        ),
        (
            "a tag filter, which this operation does not document either",
            filters(&[("tag:Owner", &["TeamA"])]),
            INVALID_PARAMETER_VALUE,
            Some("The filter 'tag:Owner' is invalid".to_string()),
        ),
        (
            "one filter past the filter ceiling",
            (1..=MAX_FILTERS + 1)
                .flat_map(|index| {
                    [
                        param(&format!("Filter.{index}.Name"), "instance-type"),
                        param(&format!("Filter.{index}.Value.1"), M5_LARGE),
                    ]
                })
                .collect(),
            INVALID_PARAMETER_VALUE,
            None,
        ),
        (
            "one filter value past the values ceiling",
            filters(&[(
                "availability-zone",
                &too_many_values
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<&str>>(),
            )]),
            INVALID_PARAMETER_VALUE,
            None,
        ),
        (
            "one byte past the filter value size ceiling",
            filters(&[("availability-zone", &[oversize_value.as_str()])]),
            INVALID_PARAMETER_VALUE,
            None,
        ),
        (
            "one id past the id ceiling",
            indexed(
                IDS,
                &too_many_ids
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<&str>>(),
            ),
            INVALID_PARAMETER_VALUE,
            None,
        ),
        (
            "an offering id this service never minted",
            indexed(IDS, &["rio-0123456789abcdef0"]),
            INVALID_RESERVED_INSTANCES_OFFERING_ID,
            Some(
                "The Reserved Instances offering ID 'rio-0123456789abcdef0' does not exist"
                    .to_string(),
            ),
        ),
        (
            "a value that is no offering id at all",
            indexed(IDS, &["not-an-offering-id"]),
            INVALID_RESERVED_INSTANCES_OFFERING_ID,
            Some(
                "The Reserved Instances offering ID 'not-an-offering-id' does not exist"
                    .to_string(),
            ),
        ),
        (
            "a pagination token this service never minted",
            vec![param("NextToken", "not-a-token")],
            INVALID_PAGINATION_TOKEN,
            None,
        ),
    ];

    for (case, params, expected_code, expected_message) in rows {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected the code {expected_code}, got {body}"
        );
        if let Some(expected_message) = expected_message {
            assert_eq!(
                error_message(&body),
                expected_message,
                "case {case:?}: expected the message {expected_message:?}, got {body}"
            );
        }
        assert!(
            super::ec2_query::element(&body, "reservedInstancesOfferingsSet").is_none(),
            "case {case:?}: a refusal reports no offerings, got {body}"
        );
    }

    // "If you have the required permissions, the error response is DryRunOperation": the probe
    // answers the permission question and reports no offerings.
    let (status, body) = describe(&router, &[param("DryRun", "true")]).await;
    assert_eq!(
        status, DRY_RUN_STATUS,
        "case \"a dry run\": expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        DRY_RUN_OPERATION,
        "case \"a dry run\": expected the code {DRY_RUN_OPERATION}, got {body}"
    );
}

/// A token minted for one search is refused when it is presented back against another, rather
/// than resuming at a position inside a set it never described.
#[tokio::test]
async fn a_token_from_another_search_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    let case = "a token from another search";
    let mut params = one_type_and_product(M5_LARGE, LINUX_UNIX);
    params.push(param("MaxResults", "2"));
    let (status, body) = describe(&router, &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let token = super::ec2_query::element(&body, "nextToken")
        .unwrap_or_else(|| panic!("case {case:?}: a page of two must carry a token: {body}"));

    // The same page size, a different search: the token names a position in the first search's
    // own product.
    let mut other = one_type_and_product(M5_XLARGE, LINUX_UNIX);
    other.push(param("MaxResults", "2"));
    other.push(param("NextToken", &token));
    let (status, body) = describe(&router, &other).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        INVALID_PAGINATION_TOKEN,
        "case {case:?}: expected the code {INVALID_PAGINATION_TOKEN}, got {body}"
    );
}

// ---------------------------------------------------------------------------
// Rule 5: paging the selected offerings
// ---------------------------------------------------------------------------

/// `MaxResults` and `NextToken` page the selected offerings: each page holds at most the page
/// size, the walk reports every selected offering exactly once, and the last page carries no
/// token. A page size above the published maximum is served that maximum.
#[tokio::test]
async fn max_results_and_next_token_page_the_selected_offerings() {
    let (router, _engine) = super::ec2_query::emulator();
    let case = "the whole of one search";
    let whole: BTreeSet<String> = reported(&router, case, &one_type_and_product(M5_LARGE, LINUX_UNIX))
        .await
        .into_iter()
        .map(|offering| offering.offering_id)
        .collect();

    // Rows differ along the page size: one offering at a time, a size that divides the search
    // unevenly, and a size larger than the search itself.
    for page_size in [1usize, 5, 7, whole.len() + 1] {
        let row = format!("a page size of {page_size}");
        let (walked, pages) = walk(
            &router,
            &row,
            &one_type_and_product(M5_LARGE, LINUX_UNIX),
            Some(page_size),
        )
        .await;
        let walked_ids: BTreeSet<String> = walked
            .iter()
            .map(|offering| offering.offering_id.clone())
            .collect();
        assert_eq!(
            walked_ids.len(),
            walked.len(),
            "case {row:?}: the walk reports each offering once, got {} offerings for {} ids",
            walked.len(),
            walked_ids.len()
        );
        assert_eq!(
            walked_ids, whole,
            "case {row:?}: the walk reports exactly the search's offerings; missing {:?}, \
             unexpected {:?}",
            whole.difference(&walked_ids).collect::<Vec<_>>(),
            walked_ids.difference(&whole).collect::<Vec<_>>()
        );
        let expected_pages = whole.len().div_ceil(page_size.max(1)).max(1);
        assert_eq!(
            pages, expected_pages,
            "case {row:?}: expected {expected_pages} pages for {} offerings, got {pages}",
            whole.len()
        );
    }

    // "The maximum is 100": a request asking for more is served that maximum, and the search it
    // asks over is larger than one page, so the page is full and carries a token.
    let (status, body) = describe(&router, &[param("MaxResults", "1000")]).await;
    assert_eq!(
        status, 200,
        "case \"a page size above the maximum\": expected HTTP 200, got {status} with body {body}"
    );
    let page = offerings("a page size above the maximum", &body);
    assert_eq!(
        page.len(),
        100,
        "case \"a page size above the maximum\": expected the published maximum of 100 offerings, \
         got {}",
        page.len()
    );
    assert!(
        super::ec2_query::element(&body, "nextToken").is_some(),
        "case \"a page size above the maximum\": the whole catalog is more than one page, so a \
         token must follow it: {body}"
    );
}

// ---------------------------------------------------------------------------
// Rule 6: the describe is a read, and its ids are the ones the exchange resolves
// ---------------------------------------------------------------------------

/// The describe writes nothing: two calls with nothing in between report the same offerings
/// under the same ids, and the store holds exactly what it held before — which is what lets a
/// later call purchase or exchange into an offering an earlier call reported.
#[tokio::test]
async fn the_describe_changes_nothing_and_its_ids_resolve_in_an_exchange() {
    let (router, engine) = super::ec2_query::emulator();
    let case = "a repeated read";
    let first = reported(&router, case, &one_type_and_product(M5_LARGE, LINUX_UNIX)).await;
    let second = reported(&router, case, &one_type_and_product(M5_LARGE, LINUX_UNIX)).await;
    assert_eq!(
        first, second,
        "case {case:?}: two reads must report the same offerings under the same ids"
    );
    assert!(
        engine
            .state::<Ec2State>()
            .lock()
            .reserved_instances
            .is_empty(),
        "case {case:?}: a read of the catalog must not mint a Reserved Instance"
    );

    // "This is the offering ID used in GetReservedInstancesExchangeQuote to confirm that an
    // exchange can be made" (`ReservedInstancesOffering$ReservedInstancesOfferingId`): the id
    // this describe reports is the one that operation resolves, which is what makes the two
    // agree about one offering.
    let purchase = engine
        .state::<Ec2State>()
        .lock()
        .insert_reserved_instances(ReservedInstancesSpec {
            instance_type: M5_LARGE.to_string(),
            availability_zone: Some("us-east-1a".to_string()),
            region: REGION.to_string(),
            instance_count: 1,
            duration_seconds: ONE_YEAR_SECONDS,
            offering_class: CONVERTIBLE.to_string(),
            offering_type: ALL_UPFRONT.to_string(),
            product_description: LINUX_UNIX.to_string(),
            state: RESERVED_INSTANCE_STATE_ACTIVE.to_string(),
            start: SystemTime::now(),
            fixed_price: 1.0,
            usage_price: 0.0,
        })
        .expect("the store has room for the one purchase this suite seeds");
    let target = first
        .iter()
        .find(|offering| {
            offering.offering_class == CONVERTIBLE
                && offering.availability_zone.as_deref() == Some("us-east-1a")
                && offering.duration == ONE_YEAR_SECONDS
                && offering.offering_type == ALL_UPFRONT
        })
        .unwrap_or_else(|| panic!("case {case:?}: expected a convertible offering in us-east-1a"));
    let (status, body) = call(
        &router,
        REGION,
        "GetReservedInstancesExchangeQuote",
        &[
            param("ReservedInstanceId.1", &purchase),
            param("TargetConfiguration.1.OfferingId", &target.offering_id),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: an offering id this describe reported must resolve in an exchange quote, \
         got {status} with body {body}"
    );
    assert!(
        body.contains(&target.offering_id),
        "case {case:?}: the quote must name the offering it was asked about ({}), got {body}",
        target.offering_id
    );
}
