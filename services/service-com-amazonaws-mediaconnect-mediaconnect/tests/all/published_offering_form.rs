// SPDX-License-Identifier: CC0-1.0

//! The published FORM of a reservation offering, end to end over HTTP: `GET /v1/offerings`
//! and `GET /v1/offerings/{OfferingArn}`.
//!
//! The subject is the rule the catalogue's own fields follow, whatever the entries are:
//!
//! * `Offering$OfferingArn` is `arn:<partition>:mediaconnect:<region>:offering:<numeric
//!   offering id>` — six colon-separated fields, with NO account-id field, because the
//!   catalogue belongs to the service rather than to one account ("the Amazon Resource Name
//!   (ARN) that MediaConnect assigns to the offering",
//!   `smithy_query(query="shape",shape="Offering",member="OfferingArn")`);
//! * `Offering$OfferingDescription` is the wording the service publishes an offering under,
//!   `Reserved outbound bandwidth of <N>Mbps at $<price>/hour`, whose two variable parts are
//!   that entry's own `ResourceSpecification$ReservedBitrate` and `PricePerUnit`;
//! * `Offering$PricePerUnit` is the per-hour price of THAT entry, quoted at the scale the
//!   catalogue publishes, and the entries are a price list rather than one rate multiplied
//!   by each bandwidth.
//!
//! The cases vary what the rule generalises over: every entry of the catalogue (not one
//! tier), the Region the request is signed for, and whether the entry is reached through
//! the listing or through DescribeOffering. The entries themselves are read from the
//! service, so a change to the catalogue is covered by these cases without editing them.
//!
//! Every call goes over the wire through the same `MultiServiceRouter` the emulator's front
//! door uses.

use serde_json::Value;

use service_com_amazonaws_mediaconnect_mediaconnect::state as mc;

use crate::flow_lifecycle::{Emulator, REGION};

/// The Regions the catalogue is listed in below: the one every other suite signs for, one
/// in another geography and one with a digit in its name.
const REGIONS: &[&str] = &[REGION, "eu-west-2", "ap-southeast-4"];

/// Every offering the service publishes for `region`, as the listing reports it.
async fn published(emulator: &Emulator, region: &str) -> Vec<Value> {
    let (status, code, body) = emulator
        .send_signed_for(region, "GET", "/v1/offerings?maxResults=1000", None)
        .await;
    assert_eq!(
        (status, code.as_deref()),
        (200, None),
        "region {region:?}: listing expected 200, got {status} {code:?}: {body}"
    );
    let offerings = body["offerings"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        offerings.len(),
        mc::OFFERING_CATALOGUE.len(),
        "region {region:?}: the whole published catalogue is listed, got {body}"
    );
    offerings
}

/// Assert the published form of one offering, as it was read back in `region`.
fn assert_published_form(case: &str, region: &str, offering: &Value) {
    let arn = offering["offeringArn"].as_str().unwrap_or_default();
    let fields: Vec<&str> = arn.split(':').collect();
    assert_eq!(
        fields.len(),
        6,
        "case {case:?}: an offering ARN has six colon-separated fields, got {arn:?}"
    );
    assert_eq!(
        (fields[0], fields[2], fields[3], fields[4]),
        ("arn", "mediaconnect", region, "offering"),
        "case {case:?}: prefix, service, Region and resource type, got {arn:?}"
    );
    assert!(
        fields[1].starts_with("aws"),
        "case {case:?}: the partition is an aws partition, got {arn:?}"
    );
    assert!(
        !fields[5].is_empty() && fields[5].bytes().all(|b| b.is_ascii_digit()),
        "case {case:?}: the offering id is numeric, got {arn:?}"
    );
    assert!(
        !arn.contains(&format!(":{}:", engine::emulator_account_id())),
        "case {case:?}: an offering ARN carries no account-id field, got {arn:?}"
    );

    // The price is quoted as a decimal string at the catalogue's scale, and the description
    // is the published wording with this entry's own bandwidth and price in it.
    let price = offering["pricePerUnit"].as_str().unwrap_or_default();
    let fraction = price.split_once('.').map(|(_, fraction)| fraction.len());
    assert_eq!(
        fraction,
        Some(3),
        "case {case:?}: a price is a decimal string quoted to the thousandth, got {price:?}"
    );
    let bitrate = offering["resourceSpecification"]["reservedBitrate"]
        .as_i64()
        .unwrap_or_default();
    assert!(
        bitrate > 0,
        "case {case:?}: an offering reserves a positive amount of bandwidth, got {offering}"
    );
    assert_eq!(
        offering["offeringDescription"].as_str(),
        Some(format!("Reserved outbound bandwidth of {bitrate}Mbps at ${price}/hour").as_str()),
        "case {case:?}: the description is the published wording of this entry's terms, got \
         {offering}"
    );
}

/// Every entry of the published catalogue carries the published form of an offering, in
/// every Region it is listed in — and the entries are the same catalogue in each Region,
/// addressed by that Region's ARNs.
#[tokio::test]
async fn every_published_offering_carries_the_published_form() {
    let emulator = Emulator::new();

    let mut ids_per_region: Vec<Vec<String>> = Vec::new();
    for region in REGIONS {
        let offerings = published(&emulator, region).await;
        let mut ids: Vec<String> = Vec::new();
        for offering in &offerings {
            let bitrate = offering["resourceSpecification"]["reservedBitrate"]
                .as_i64()
                .unwrap_or_default();
            let case = format!("the {bitrate} Mbps offering in {region}");
            assert_published_form(&case, region, offering);
            let arn = offering["offeringArn"].as_str().unwrap_or_default();
            ids.push(arn.rsplit(':').next().unwrap_or_default().to_string());
        }
        ids_per_region.push(ids);
    }

    for (region, ids) in REGIONS.iter().zip(&ids_per_region) {
        assert_eq!(
            ids, &ids_per_region[0],
            "case {region:?}: an offering id names the catalogue entry, not the Region, got \
             {ids:?}"
        );
        assert!(
            ids.iter().all(|id| id.len() == ids_per_region[0][0].len()),
            "case {region:?}: offering ids are the same width, so their ARNs sort by the \
             entry they name, got {ids:?}"
        );
    }
}

/// An offering ARN read from the listing addresses the same entry through DescribeOffering,
/// and the published form is part of that identity: the same ARN with an account-id field
/// inserted, or with a non-numeric offering id, addresses no offering.
#[tokio::test]
async fn a_published_offering_arn_round_trips_and_its_form_is_its_identity() {
    let emulator = Emulator::new();
    let offerings = published(&emulator, REGION).await;

    for offering in &offerings {
        let arn = offering["offeringArn"].as_str().unwrap_or_default();
        let bitrate = offering["resourceSpecification"]["reservedBitrate"]
            .as_i64()
            .unwrap_or_default();
        let case = format!("the {bitrate} Mbps offering");
        let (status, code, body) = emulator
            .send("GET", &format!("/v1/offerings/{arn}"), None)
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (200, None),
            "case {case:?}: expected 200 for {arn:?}, got {status} {code:?}: {body}"
        );
        assert_eq!(
            &body["offering"], offering,
            "case {case:?}: the described offering is the listed one, member for member"
        );
        assert_published_form(&case, REGION, &body["offering"]);
    }

    let published_arn = offerings
        .first()
        .and_then(|offering| offering["offeringArn"].as_str())
        .unwrap_or_default()
        .to_string();
    // Rows: the ways a caller can hold an identifier of the wrong FORM — the account-id
    // field AWS's offering ARNs do not carry, an offering id that is not numeric, and the
    // resource type on its own.
    let wrong_form = [
        (
            "an account-id field inserted into the published ARN",
            published_arn.replace(
                &format!(":{}:", mc::SERVICE_NAMESPACE),
                &format!(
                    ":{}:{}:",
                    mc::SERVICE_NAMESPACE,
                    engine::emulator_account_id()
                ),
            ),
        ),
        (
            "an offering id that is not numeric",
            mc::offering_arn(REGION, "offering:twelve-thousand"),
        ),
        (
            "the resource type with no offering id",
            mc::offering_arn(REGION, "offering:"),
        ),
    ];
    for (case, arn) in wrong_form {
        assert_ne!(
            arn, published_arn,
            "case {case:?}: the row must address something other than the published offering"
        );
        let (status, code, body) = emulator
            .send("GET", &format!("/v1/offerings/{arn}"), None)
            .await;
        assert_eq!(
            (status, code.as_deref()),
            (404, Some("NotFoundException")),
            "case {case:?}: expected 404 NotFoundException, got {status} {code:?}: {body}"
        );
        assert!(
            !mc::is_mintable_offering_arn(&arn),
            "case {case:?}: {arn:?} is not an offering ARN this service could mint"
        );
    }
}

/// The catalogue is a price list: each entry prices its own amount of bandwidth, so a
/// larger commitment costs more per hour and never more per Mbps than a smaller one — which
/// no single rate multiplied by each bandwidth can satisfy.
#[tokio::test]
async fn the_catalogue_prices_each_entry_as_a_volume_commitment() {
    let emulator = Emulator::new();
    let offerings = published(&emulator, REGION).await;

    // (reserved bitrate, price in thousandths of a dollar), ascending by bandwidth.
    let mut priced: Vec<(i64, i64)> = offerings
        .iter()
        .map(|offering| {
            let bitrate = offering["resourceSpecification"]["reservedBitrate"]
                .as_i64()
                .unwrap_or_default();
            let price = offering["pricePerUnit"].as_str().unwrap_or_default();
            let thousandths = price
                .split_once('.')
                .and_then(|(dollars, fraction)| {
                    let dollars = dollars.parse::<i64>().ok()?;
                    let fraction = fraction.parse::<i64>().ok()?;
                    Some(dollars * 1_000 + fraction)
                })
                .unwrap_or_else(|| panic!("a price is a decimal string, got {price:?}"));
            (bitrate, thousandths)
        })
        .collect();
    priced.sort_unstable();
    assert!(
        priced.len() > 1,
        "the catalogue offers more than one amount of bandwidth, got {priced:?}"
    );

    let mut rate_falls_somewhere = false;
    for pair in priced.windows(2) {
        let [(smaller, cheaper), (larger, dearer)] = pair else {
            continue;
        };
        let case = format!("the {smaller} Mbps and {larger} Mbps offerings");
        assert!(
            cheaper > &0 && dearer > cheaper,
            "case {case:?}: reserving more bandwidth costs more per hour, got {cheaper} and \
             {dearer} thousandths"
        );
        assert!(
            dearer * smaller <= cheaper * larger,
            "case {case:?}: the rate per Mbps does not rise with the amount reserved, got \
             {cheaper} thousandths for {smaller} Mbps and {dearer} for {larger}"
        );
        rate_falls_somewhere |= dearer * smaller < cheaper * larger;
    }
    assert!(
        rate_falls_somewhere,
        "the catalogue discounts a larger commitment somewhere, so it is a price list rather \
         than one rate per Mbps, got {priced:?}"
    );
}
