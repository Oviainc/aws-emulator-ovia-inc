// SPDX-License-Identifier: CC0-1.0

//! `ListOfferings`: which offerings the catalog holds, what each one reports,
//! which of them a filter admits, and how a page is cut — end to end over the
//! restJson1 wire.
//!
//! Every test drives the service the way a client does, through
//! `MultiServiceRouter::dispatch` with real HTTP requests, so what is asserted is
//! the emulator's wire behaviour and not an internal function's return value.
//!
//! The suites are TABLES over the model's own value sets: one unfiltered
//! `ListOfferings` call — the case the stub issue reproduces — is ONE row of
//! them. The expected catalog is RE-DERIVED here from the
//! `ReservationResourceSpecification` member enums (`smithy_query(query="shape",
//! shape="ReservationResourceSpecification")` and the `*_VALUES` constants the
//! model generates for each of them), so this suite agrees with the model rather
//! than with the handler.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_medialive_medialive::ops::types::{
    CHANNEL_CLASS_VALUES, RESERVATION_CODEC_VALUES, RESERVATION_MAXIMUM_BITRATE_VALUES,
    RESERVATION_MAXIMUM_FRAMERATE_VALUES, RESERVATION_RESOLUTION_VALUES,
    RESERVATION_RESOURCE_TYPE_VALUES, RESERVATION_SPECIAL_FEATURE_VALUES,
    RESERVATION_VIDEO_QUALITY_VALUES,
};
use service_com_amazonaws_medialive_medialive::state::MAX_LIST_PAGE_SIZE;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, call_in_region, error_code, medialive, ACCOUNT, DEFAULT_REGION};

/// The collection URI `ListOfferings` is bound to
/// (`smithy_query(query="operation", operation="ListOfferings")`: GET
/// /prod/offerings).
const OFFERINGS: &str = "/prod/offerings";

/// Every member of `ReservationResourceSpecification`, by its wire name, in the
/// one order this suite canonicalises a specification in.
const SPECIFICATION_MEMBERS: [&str; 8] = [
    "resourceType",
    "channelClass",
    "codec",
    "resolution",
    "maximumBitrate",
    "maximumFramerate",
    "videoQuality",
    "specialFeature",
];

/// One resource specification, canonicalised so two of them can be compared and
/// ordered: the value of each member of [`SPECIFICATION_MEMBERS`], absent where
/// the specification does not carry that member.
type Specification = [Option<String>; 8];

/// The `ReservationResourceType` values the three scoped members belong to: the
/// model documents `MaximumFramerate` and `VideoQuality` as "(Outputs only)" and
/// `SpecialFeature` as "(Channels only)".
const OUTPUT: &str = "OUTPUT";
const CHANNEL: &str = "CHANNEL";

/// The lease term every offering is for and the units it is in
/// (`OfferingDurationUnits` declares MONTHS and nothing else; `Duration` is
/// documented "Lease duration, e.g. '12'").
const DURATION: i64 = 12;
const DURATION_UNITS: &str = "MONTHS";

/// The one offering type the model declares (`OfferingType`: NO_UPFRONT), and
/// the one-time charge `FixedPrice` documents for it ("e.g. '0.0' for a
/// NO_UPFRONT offering").
const OFFERING_TYPE: &str = "NO_UPFRONT";

/// `ListOfferings` with `query` appended verbatim (every input member is
/// `@httpQuery`), signed for `region`.
async fn list(
    router: &MultiServiceRouter,
    region: &str,
    query: &str,
) -> (u16, Value) {
    call_in_region(router, region, "GET", &format!("{OFFERINGS}{query}"), None).await
}

/// The `offerings` member as a list, whatever its length
/// (`ListOfferingsResponse$Offerings`, jsonName `offerings`).
fn listed(case: &str, body: &Value) -> Vec<Value> {
    body["offerings"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected an offerings list, got {body}"))
}

/// Walk every page of the listing under `filters` (a query string without its
/// leading separator, or empty) and answer the offerings reported, in order.
///
/// Shared with the `DescribeOffering` suite, which reads the same catalog back
/// one offering at a time.
///
/// The token discipline is asserted on the way: a page never carries more than
/// the effective page size, and a token is handed out only beside a non-empty
/// page.
pub(crate) async fn every_page(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    filters: &str,
    page_size: Option<i32>,
) -> Vec<Value> {
    let mut offerings: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut query = String::new();
        let push = |parameter: &str, query: &mut String| {
            query.push(if query.is_empty() { '?' } else { '&' });
            query.push_str(parameter);
        };
        if !filters.is_empty() {
            push(filters, &mut query);
        }
        if let Some(max_results) = page_size {
            push(&format!("maxResults={max_results}"), &mut query);
        }
        if let Some(next) = &token {
            push(&format!("nextToken={next}"), &mut query);
        }

        let (status, body) = list(router, region, &query).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListOfferings to answer 200, got {status}: {body}"
        );
        let page = listed(case, &body);
        let allowed = page_size.map_or(MAX_LIST_PAGE_SIZE, |size| {
            (size as usize).min(MAX_LIST_PAGE_SIZE)
        });
        assert!(
            page.len() <= allowed,
            "case {case:?}: expected at most {allowed} offerings in a page, got {}",
            page.len()
        );
        offerings.extend(page.iter().cloned());

        token = body["nextToken"].as_str().map(ToString::to_string);
        if token.is_some() {
            assert!(
                !page.is_empty(),
                "case {case:?}: expected no nextToken beside an empty page, got {body}"
            );
        }
        pages += 1;
        if token.is_none() {
            break;
        }
        assert!(
            pages <= 4096,
            "case {case:?}: expected pagination to terminate, got {pages} pages"
        );
    }
    offerings
}

/// The canonical form of one reported offering's resource specification.
fn specification_of(offering: &Value) -> Specification {
    let specification = &offering["resourceSpecification"];
    SPECIFICATION_MEMBERS.map(|member| {
        specification
            .get(member)
            .and_then(Value::as_str)
            .map(ToString::to_string)
    })
}

/// One member of a canonicalised specification, by its wire name.
fn member(specification: &Specification, name: &str) -> Option<String> {
    let index = SPECIFICATION_MEMBERS
        .iter()
        .position(|candidate| *candidate == name)
        .unwrap_or_else(|| panic!("{name} is not a specification member"));
    specification[index].clone()
}

/// The values a member that applies only to one resource type takes across that
/// type's offerings: each modelled value, plus the member's ABSENCE when
/// `optional`.
fn scoped(applies: bool, values: &[&str], optional: bool) -> Vec<Option<String>> {
    if !applies {
        return vec![None];
    }
    let absent = if optional { vec![None] } else { Vec::new() };
    absent
        .into_iter()
        .chain(values.iter().map(|value| Some((*value).to_string())))
        .collect()
}

/// Every resource configuration that can be reserved, re-derived from the model:
/// the product of the `ReservationResourceSpecification` member enums, with each
/// member the model scopes to one resource type carried only by that type's
/// offerings, and `SpecialFeature` also absent on the plain channel offerings
/// (a reserved channel either carries one of the special features or carries
/// none).
fn expected_specifications() -> BTreeSet<Specification> {
    let mut expected = BTreeSet::new();
    for resource_type in RESERVATION_RESOURCE_TYPE_VALUES.iter().copied() {
        let framerates = scoped(
            resource_type == OUTPUT,
            RESERVATION_MAXIMUM_FRAMERATE_VALUES,
            false,
        );
        let qualities = scoped(
            resource_type == OUTPUT,
            RESERVATION_VIDEO_QUALITY_VALUES,
            false,
        );
        let features = scoped(
            resource_type == CHANNEL,
            RESERVATION_SPECIAL_FEATURE_VALUES,
            true,
        );
        for channel_class in CHANNEL_CLASS_VALUES {
            for codec in RESERVATION_CODEC_VALUES {
                for resolution in RESERVATION_RESOLUTION_VALUES {
                    for maximum_bitrate in RESERVATION_MAXIMUM_BITRATE_VALUES {
                        for framerate in &framerates {
                            for quality in &qualities {
                                for feature in &features {
                                    expected.insert([
                                        Some(resource_type.to_string()),
                                        Some(channel_class.to_string()),
                                        Some(codec.to_string()),
                                        Some(resolution.to_string()),
                                        Some(maximum_bitrate.to_string()),
                                        framerate.clone(),
                                        quality.clone(),
                                        feature.clone(),
                                    ]);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    expected
}

/// Rule: `ListOfferings` reports the service's whole offering catalog — one
/// offering per resource configuration that can be reserved, which is the
/// product of the `ReservationResourceSpecification` member enums with each
/// member the model scopes to one resource type carried only by that type's
/// offerings — each exactly once, under an id that is stable across calls.
///
/// The rows differ along the PAGE SIZE the walk asks for, because that is what
/// the cut generalises over: the default (no size at all), a size past the
/// service's ceiling, and a size that divides the catalog unevenly. The
/// offerings reported must be the same set however the walk is cut.
#[tokio::test]
async fn list_offerings_reports_every_reservable_configuration_once() {
    let router = medialive();
    let expected = expected_specifications();
    assert!(
        !expected.is_empty(),
        "the model's own value sets must derive a non-empty catalog"
    );

    for (case, page_size) in [
        ("default page size", None),
        ("page size past the ceiling", Some(MAX_LIST_PAGE_SIZE as i32 + 500)),
        ("page of 37", Some(37)),
    ] {
        let offerings = every_page(&router, case, DEFAULT_REGION, "", page_size).await;
        assert_eq!(
            offerings.len(),
            expected.len(),
            "case {case:?}: expected the catalog to hold {} offerings, got {}",
            expected.len(),
            offerings.len()
        );

        let reported: BTreeSet<Specification> =
            offerings.iter().map(specification_of).collect();
        assert_eq!(
            reported.len(),
            offerings.len(),
            "case {case:?}: expected every offering to reserve a different configuration, \
             got {} configurations for {} offerings",
            reported.len(),
            offerings.len()
        );
        let missing: Vec<&Specification> = expected.difference(&reported).take(3).collect();
        let extra: Vec<&Specification> = reported.difference(&expected).take(3).collect();
        assert!(
            missing.is_empty() && extra.is_empty(),
            "case {case:?}: expected the catalog to be exactly the modelled product; \
             missing {missing:?}, unexpected {extra:?}"
        );

        let ids: BTreeSet<String> = offerings
            .iter()
            .map(|offering| offering["offeringId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            ids.len(),
            offerings.len(),
            "case {case:?}: expected every offering to carry its own id, got {} ids for {} \
             offerings",
            ids.len(),
            offerings.len()
        );
    }
}

/// Rule: every member an offering reports is derived from the configuration it
/// reserves and the region the request was signed for — the id names the
/// configuration, the ARN is built from that id, the lease term and type are the
/// ones the model declares, and the prices are stable for a configuration.
///
/// Asserted over EVERY offering the catalog holds rather than over one row, so a
/// member that is right for the cited configuration and wrong for another is
/// caught.
#[tokio::test]
async fn every_offering_reports_the_configuration_it_reserves() {
    let router = medialive();
    let offerings = every_page(&router, "the whole catalog", DEFAULT_REGION, "", None).await;

    for offering in &offerings {
        let id = offering["offeringId"].as_str().unwrap_or_default().to_string();
        let case = format!("offering {id}");
        assert!(
            !id.is_empty() && id.chars().all(|digit| digit.is_ascii_digit()),
            "case {case:?}: expected a numeric offeringId, got {offering}"
        );
        assert_eq!(
            offering["arn"],
            json!(format!(
                "arn:aws:medialive:{DEFAULT_REGION}:{ACCOUNT}:offering:{id}"
            )),
            "case {case:?}: expected the ARN built from the signed region and the offering id, \
             got {:?}",
            offering["arn"]
        );
        assert_eq!(
            offering["region"],
            json!(DEFAULT_REGION),
            "case {case:?}: expected the signed region, got {:?}",
            offering["region"]
        );
        assert_eq!(
            offering["duration"],
            json!(DURATION),
            "case {case:?}: expected a {DURATION}-{DURATION_UNITS} lease, got {:?}",
            offering["duration"]
        );
        assert_eq!(
            offering["durationUnits"],
            json!(DURATION_UNITS),
            "case {case:?}: expected the one modelled duration unit, got {:?}",
            offering["durationUnits"]
        );
        assert_eq!(
            offering["offeringType"],
            json!(OFFERING_TYPE),
            "case {case:?}: expected the one modelled offering type, got {:?}",
            offering["offeringType"]
        );
        assert_eq!(
            offering["fixedPrice"].as_f64(),
            Some(0.0),
            "case {case:?}: expected a {OFFERING_TYPE} offering to carry no one-time charge, \
             got {:?}",
            offering["fixedPrice"]
        );
        assert_eq!(
            offering["currencyCode"],
            json!("USD"),
            "case {case:?}: expected an ISO-4217 currency code, got {:?}",
            offering["currencyCode"]
        );
        let usage_price = offering["usagePrice"].as_f64().unwrap_or_default();
        assert!(
            usage_price > 0.0,
            "case {case:?}: expected a recurring charge for the reserved resource, got {:?}",
            offering["usagePrice"]
        );

        // The description names the configuration it reserves, so a caller
        // reading it sees the same configuration the structured member carries.
        let description = offering["offeringDescription"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let specification = specification_of(offering);
        for name in SPECIFICATION_MEMBERS {
            if let Some(value) = member(&specification, name) {
                assert!(
                    description.contains(&value),
                    "case {case:?}: expected the description to name {name} {value:?}, \
                     got {description:?}"
                );
            }
        }
        assert!(
            description.contains(DEFAULT_REGION),
            "case {case:?}: expected the description to name the region, got {description:?}"
        );
    }

    // A configuration that reserves more never costs less than one that reserves
    // less: the two channel classes differ only in their pipeline count ("A
    // standard channel has two encoding pipelines and a single pipeline channel
    // only has one"), so the standard offering of a configuration costs more
    // than its single-pipeline twin.
    let prices: BTreeMap<Specification, f64> = offerings
        .iter()
        .map(|offering| {
            (
                specification_of(offering),
                offering["usagePrice"].as_f64().unwrap_or_default(),
            )
        })
        .collect();
    for (specification, price) in &prices {
        if member(specification, "channelClass").as_deref() != Some("STANDARD") {
            continue;
        }
        let mut single = specification.clone();
        single[1] = Some("SINGLE_PIPELINE".to_string());
        let twin = prices.get(&single).copied().unwrap_or_default();
        assert!(
            *price > twin,
            "case {specification:?}: expected a two-pipeline reservation to cost more than its \
             single-pipeline twin, got {price} against {twin}"
        );
    }
}

/// Rule: a filter on a specification member admits exactly the offerings that
/// carry that value for it, so an offering that does not carry the member at all
/// is admitted by no value of it, and filters combine by intersection.
///
/// The table spans EVERY filter member and EVERY value its own enum declares
/// (`smithy_query(query="shape", shape="ReservationCodec,ReservationResolution,
/// ReservationResourceType,ReservationMaximumBitrate,
/// ReservationMaximumFramerate,ReservationSpecialFeature,
/// ReservationVideoQuality,ChannelClass")`), each checked against the same
/// predicate applied to the unfiltered catalog — so no value is special-cased.
#[tokio::test]
async fn each_filter_admits_exactly_the_offerings_carrying_its_value() {
    let router = medialive();
    let catalog: Vec<Specification> =
        every_page(&router, "the whole catalog", DEFAULT_REGION, "", None)
            .await
            .iter()
            .map(specification_of)
            .collect();

    let filters: [(&str, &[&str]); 8] = [
        ("resourceType", RESERVATION_RESOURCE_TYPE_VALUES),
        ("channelClass", CHANNEL_CLASS_VALUES),
        ("codec", RESERVATION_CODEC_VALUES),
        ("resolution", RESERVATION_RESOLUTION_VALUES),
        ("maximumBitrate", RESERVATION_MAXIMUM_BITRATE_VALUES),
        ("maximumFramerate", RESERVATION_MAXIMUM_FRAMERATE_VALUES),
        ("videoQuality", RESERVATION_VIDEO_QUALITY_VALUES),
        ("specialFeature", RESERVATION_SPECIAL_FEATURE_VALUES),
    ];

    for (name, values) in filters {
        for value in values.iter().copied() {
            let case = format!("{name}={value}");
            let expected: BTreeSet<Specification> = catalog
                .iter()
                .filter(|specification| member(specification, name).as_deref() == Some(value))
                .cloned()
                .collect();
            assert!(
                !expected.is_empty(),
                "case {case:?}: the catalog must hold an offering for every modelled value"
            );

            let admitted: BTreeSet<Specification> =
                every_page(&router, &case, DEFAULT_REGION, &case, None)
                    .await
                    .iter()
                    .map(specification_of)
                    .collect();
            assert_eq!(
                admitted, expected,
                "case {case:?}: expected the filter to admit exactly the {} offerings carrying \
                 that value, got {}",
                expected.len(),
                admitted.len()
            );
        }
    }

    // Filters COMBINE: a request naming several of them admits the offerings
    // carrying every one of those values, down to the single offering that
    // carries all seven.
    let combined = "resourceType=OUTPUT&channelClass=STANDARD&codec=HEVC&resolution=UHD\
                    &maximumBitrate=MAX_50_MBPS&maximumFramerate=MAX_60_FPS&videoQuality=PREMIUM";
    let admitted = every_page(&router, combined, DEFAULT_REGION, combined, None).await;
    assert_eq!(
        admitted.len(),
        1,
        "case {combined:?}: expected the one offering carrying every named value, got {}",
        admitted.len()
    );

    // A member scoped to one resource type is carried by no offering of
    // another, so naming both admits nothing: `VideoQuality` is "(Outputs
    // only)" and `SpecialFeature` is "(Channels only)".
    for case in [
        "resourceType=INPUT&videoQuality=STANDARD",
        "resourceType=INPUT&maximumFramerate=MAX_30_FPS",
        "resourceType=MULTIPLEX&specialFeature=ADVANCED_AUDIO",
        "resourceType=OUTPUT&specialFeature=MGUHD",
        "resourceType=CHANNEL&videoQuality=PREMIUM",
    ] {
        let admitted = every_page(&router, case, DEFAULT_REGION, case, None).await;
        assert!(
            admitted.is_empty(),
            "case {case:?}: expected a member scoped to another resource type to admit no \
             offering, got {} ({:?})",
            admitted.len(),
            admitted.first()
        );
    }
}

/// Rule: the `Duration` filter admits the offerings leased for that many
/// duration units, so the catalog's own term admits every offering and any other
/// well-formed term admits none.
#[tokio::test]
async fn the_duration_filter_admits_the_offerings_leased_for_that_term() {
    let router = medialive();
    let whole = every_page(&router, "unfiltered", DEFAULT_REGION, "", None).await;

    for (case, duration, expected) in [
        ("the catalog's own term", DURATION, whole.len()),
        ("one month", 1, 0),
        ("six months", 6, 0),
        ("three years", 36, 0),
    ] {
        let query = format!("duration={duration}");
        let admitted = every_page(&router, case, DEFAULT_REGION, &query, None).await;
        assert_eq!(
            admitted.len(),
            expected,
            "case {case:?}: expected {expected} offerings leased for {duration} \
             {DURATION_UNITS}, got {}",
            admitted.len()
        );
        for offering in &admitted {
            assert_eq!(
                offering["duration"],
                json!(duration),
                "case {case:?}: expected every admitted offering to carry that term, got {:?}",
                offering["duration"]
            );
        }
    }
}

/// Create a channel of `channel_class`, optionally with `input_specification`,
/// and answer its minted id.
async fn create_channel(
    router: &MultiServiceRouter,
    case: &str,
    channel_class: &str,
    input_specification: Option<Value>,
) -> String {
    let mut request = json!({
        "name": format!("offering-filter-{case}"),
        "channelClass": channel_class,
    });
    if let Some(specification) = input_specification {
        request["inputSpecification"] = specification;
    }
    let (status, body) = call(router, "POST", "/prod/channels", Some(request)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateChannel: expected 201, got {status}: {body}"
    );
    body["channel"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Rule: the `ChannelConfiguration` filter names an EXISTING channel and admits
/// the offerings whose reserved configuration matches that channel's own — its
/// class and each member of its `InputSpecification` the channel states — so a
/// member the channel does not state constrains nothing.
///
/// The rows differ along the channel's class and along which members its input
/// specification carries, because that is what the match generalises over; a
/// channel that states no specification is the row where only the class
/// constrains.
#[tokio::test]
async fn the_channel_configuration_filter_admits_the_offerings_matching_that_channel() {
    let router = medialive();
    let catalog: Vec<Specification> =
        every_page(&router, "the whole catalog", DEFAULT_REGION, "", None)
            .await
            .iter()
            .map(specification_of)
            .collect();

    // (case, channel class, the channel's InputSpecification)
    let table: [(&str, &str, Option<Value>); 4] = [
        (
            "standard hd avc",
            "STANDARD",
            Some(json!({
                "codec": "AVC",
                "resolution": "HD",
                "maximumBitrate": "MAX_10_MBPS",
            })),
        ),
        (
            "single pipeline uhd hevc",
            "SINGLE_PIPELINE",
            Some(json!({
                "codec": "HEVC",
                "resolution": "UHD",
                "maximumBitrate": "MAX_50_MBPS",
            })),
        ),
        (
            "codec only",
            "SINGLE_PIPELINE",
            Some(json!({ "codec": "MPEG2" })),
        ),
        ("no input specification", "STANDARD", None),
    ];

    for (case, channel_class, input_specification) in table {
        let id = create_channel(&router, case, channel_class, input_specification.clone()).await;

        // What the channel's configuration constrains, read off the row.
        let mut wanted: Vec<(&str, String)> = vec![("channelClass", channel_class.to_string())];
        if let Some(specification) = &input_specification {
            for (filter, member_name) in [
                ("codec", "codec"),
                ("resolution", "resolution"),
                ("maximumBitrate", "maximumBitrate"),
            ] {
                if let Some(value) = specification.get(member_name).and_then(Value::as_str) {
                    wanted.push((filter, value.to_string()));
                }
            }
        }
        let expected: BTreeSet<Specification> = catalog
            .iter()
            .filter(|specification| {
                wanted
                    .iter()
                    .all(|(name, value)| member(specification, name).as_deref() == Some(value))
            })
            .cloned()
            .collect();
        assert!(
            !expected.is_empty(),
            "case {case:?}: the catalog must hold an offering for this channel's configuration"
        );

        let query = format!("channelConfiguration={id}");
        let admitted: BTreeSet<Specification> =
            every_page(&router, case, DEFAULT_REGION, &query, None)
                .await
                .iter()
                .map(specification_of)
                .collect();
        assert_eq!(
            admitted, expected,
            "case {case:?}: expected the filter to admit exactly the {} offerings matching the \
             channel's configuration, got {}",
            expected.len(),
            admitted.len()
        );

        // It intersects with the explicit filters like any other: the same
        // channel, narrowed to one resource type.
        let narrowed = format!("{query}&resourceType=INPUT");
        let narrowed_admitted: BTreeSet<Specification> =
            every_page(&router, case, DEFAULT_REGION, &narrowed, None)
                .await
                .iter()
                .map(specification_of)
                .collect();
        let narrowed_expected: BTreeSet<Specification> = expected
            .iter()
            .filter(|specification| {
                member(specification, "resourceType").as_deref() == Some("INPUT")
            })
            .cloned()
            .collect();
        assert_eq!(
            narrowed_admitted, narrowed_expected,
            "case {case:?}: expected the channel filter and the resourceType filter to \
             intersect, got {} offerings against {}",
            narrowed_admitted.len(),
            narrowed_expected.len()
        );
    }
}

/// Rule: a request this operation cannot serve as asked is refused with its
/// modelled `BadRequestException` (400) and reports no offerings — a page size
/// that asks for nothing, a filter value outside the set its member accepts, a
/// duration that is not a positive term, a token that is not one of this
/// operation's own, and a channel the account does not have.
///
/// The rows differ along WHICH member is unserveable and how, because that is
/// what the rule generalises over; the admissible bound beside each is asserted
/// below, so the refusal is not just a blanket rejection.
#[tokio::test]
async fn a_request_this_operation_cannot_serve_is_refused() {
    let router = medialive();

    for case in [
        // A page size that asks for no results at all is not a page size.
        "maxResults=0",
        "maxResults=-1",
        "maxResults=-50",
        // A filter value outside its member's modelled enum names no offering.
        "resourceType=INPUTS",
        "resourceType=input",
        "channelClass=standard",
        "codec=H264",
        "resolution=4K",
        "maximumBitrate=MAX_100_MBPS",
        "maximumFramerate=MAX_120_FPS",
        "videoQuality=BEST",
        "specialFeature=NONE",
        // A duration that is not a positive count of months is not a term.
        "duration=twelve",
        "duration=0",
        "duration=-12",
        "duration=12.5",
        // A token this operation never hands out.
        "nextToken=not-a-token",
        "nextToken=0",
        "nextToken=99999999",
        // A channel the account does not have has no configuration to match.
        "channelConfiguration=9999999",
        "channelConfiguration=not-a-channel",
    ] {
        let (status, body) = list(&router, DEFAULT_REGION, &format!("?{case}")).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the request to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body}"
        );
        assert!(
            body.get("offerings").is_none(),
            "case {case:?}: expected a refusal to report no offerings, got {body}"
        );
    }

    // The bounds themselves: the smallest page size, the catalog's own term, the
    // first page's own token and a channel that exists all succeed.
    let id = create_channel(&router, "bounds", "STANDARD", None).await;
    let (status, first) = list(&router, DEFAULT_REGION, "?maxResults=1").await;
    assert_eq!(
        status, 200,
        "case \"maxResults=1\": expected the smallest page size to succeed, got {status}: {first}"
    );
    assert_eq!(
        listed("maxResults=1", &first).len(),
        1,
        "case \"maxResults=1\": expected a page of one, got {first}"
    );
    let token = first["nextToken"].as_str().unwrap_or_default().to_string();
    for case in [
        format!("duration={DURATION}"),
        format!("nextToken={token}"),
        format!("channelConfiguration={id}"),
    ] {
        let (status, body) = list(&router, DEFAULT_REGION, &format!("?{case}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected an admissible request to succeed, got {status}: {body}"
        );
        assert!(
            !listed(&case, &body).is_empty(),
            "case {case:?}: expected the request to report offerings, got {body}"
        );
    }
}

/// Rule: a listing is a report, so it never changes what a later caller sees —
/// the same request answers the same offerings however many times it is made,
/// and an offering's id names the same configuration in every region, where only
/// the region-scoped members (its ARN and its `Region`) differ.
#[tokio::test]
async fn a_listing_is_a_report_and_an_id_names_one_configuration() {
    let router = medialive();

    for case in ["", "resourceType=CHANNEL&codec=AVC", "channelClass=STANDARD"] {
        let first = every_page(&router, case, DEFAULT_REGION, case, None).await;
        let second = every_page(&router, case, DEFAULT_REGION, case, Some(13)).await;
        assert_eq!(
            first, second,
            "case {case:?}: expected a repeated listing to report the same offerings"
        );
    }

    // Another region's endpoint: the same catalog, under the same ids, reported
    // for that region.
    let filters = "resourceType=MULTIPLEX&codec=AV1";
    let home = every_page(&router, filters, DEFAULT_REGION, filters, None).await;
    let away = every_page(&router, filters, "eu-west-1", filters, None).await;
    assert_eq!(
        home.len(),
        away.len(),
        "expected every region to serve the same catalog, got {} offerings against {}",
        home.len(),
        away.len()
    );
    for (here, there) in home.iter().zip(&away) {
        assert_eq!(
            here["offeringId"], there["offeringId"],
            "expected an offering id to name the same configuration in every region, got {:?} \
             against {:?}",
            here["offeringId"], there["offeringId"]
        );
        assert_eq!(
            specification_of(here),
            specification_of(there),
            "expected the configuration under one id to be the same in every region"
        );
        let id = there["offeringId"].as_str().unwrap_or_default();
        assert_eq!(
            there["region"],
            json!("eu-west-1"),
            "expected the offering to be reported for the signed region, got {:?}",
            there["region"]
        );
        assert_eq!(
            there["arn"],
            json!(format!("arn:aws:medialive:eu-west-1:{ACCOUNT}:offering:{id}")),
            "expected the ARN built from the signed region, got {:?}",
            there["arn"]
        );
    }
}
