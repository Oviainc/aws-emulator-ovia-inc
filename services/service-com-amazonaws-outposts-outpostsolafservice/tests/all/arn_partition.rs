// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the PARTITION segment of every ARN this emulator mints,
//! driven over the wire through the same front door as
//! [`crate::site_and_outpost_lifecycle`] (whose harness this suite reuses).
//!
//! The rule under test, stated once and then exercised over a SET of inputs: the
//! partition segment of a minted resource ARN is a function of the Region the
//! request is addressed to — the Region's own partition — and never a fixed literal,
//! which is why every ARN pattern in this model spells that segment `aws([a-z-]+)?`.
//! A resource therefore reports the SAME partition from every operation that reports
//! its ARN, and that ARN is accepted back as the resource's identifier.

use serde_json::{json, Value};

use crate::site_and_outpost_lifecycle::{segment, text, Outposts};

/// The account id every generated emulator presents, which the account segment of
/// every ARN below carries.
const ACCOUNT_ID: &str = "000000000000";

/// The Regions this suite mints ARNs in, each with the partition AWS places it in.
///
/// Source: AWS's published partitions and their Region prefixes — `aws` for the
/// commercial Regions, `aws-cn` for `cn-*`, `aws-us-gov` for `us-gov-*`, `aws-eusc`
/// for the European Sovereign Cloud (`eusc-de-*`), and the four ISO partitions
/// `aws-iso` (`us-iso-*`), `aws-iso-b` (`us-isob-*`), `aws-iso-e` (`eu-isoe-*`) and
/// `aws-iso-f` (`us-isof-*`). The rows span every one of those partitions, two
/// Regions of the partitions that have several, and the ISO prefixes that share a
/// leading substring (`us-iso-east-1` against `us-isob-east-1`), where reading the
/// shorter prefix first would name the wrong partition.
const REGIONS: &[(&str, &str)] = &[
    ("us-east-1", "aws"),
    ("eu-west-3", "aws"),
    ("ap-southeast-2", "aws"),
    ("us-gov-west-1", "aws-us-gov"),
    ("us-gov-east-1", "aws-us-gov"),
    ("cn-north-1", "aws-cn"),
    ("cn-northwest-1", "aws-cn"),
    ("eusc-de-east-1", "aws-eusc"),
    ("us-iso-east-1", "aws-iso"),
    ("us-isob-east-1", "aws-iso-b"),
    ("eu-isoe-west-1", "aws-iso-e"),
    ("us-isof-south-1", "aws-iso-f"),
];

/// The ARN a resource of `resource_type` with id `resource_id`, created in `region`,
/// must carry: `arn:<partition>:outposts:<region>:<account>:<type>/<id>`.
fn expected_arn(partition: &str, region: &str, resource_type: &str, resource_id: &str) -> String {
    format!("arn:{partition}:outposts:{region}:{ACCOUNT_ID}:{resource_type}/{resource_id}")
}

/// The `Site` a `GetSite` in `region` answers with.
async fn get_site_in(outposts: &Outposts, region: &str, identifier: &str) -> Value {
    outposts
        .ok_in(
            region,
            "GET",
            &format!("/sites/{}", segment(identifier)),
            None,
        )
        .await["Site"]
        .clone()
}

/// The `Outpost` a `GetOutpost` in `region` answers with.
async fn get_outpost_in(outposts: &Outposts, region: &str, identifier: &str) -> Value {
    outposts
        .ok_in(
            region,
            "GET",
            &format!("/outposts/{}", segment(identifier)),
            None,
        )
        .await["Outpost"]
        .clone()
}

/// The ARN member `arn_member` of the item of `list_member` whose `id_member` is
/// `id`, from a list the emulator answered — the empty string when the list does not
/// report that resource at all.
fn listed_arn(
    output: &Value,
    list_member: &str,
    (id_member, id): (&str, &str),
    arn_member: &str,
) -> String {
    output
        .get(list_member)
        .and_then(Value::as_array)
        .and_then(|items| items.iter().find(|item| text(item, id_member) == id))
        .map(|item| text(item, arn_member))
        .unwrap_or_default()
}

/// The rule, over the whole table: a site and an Outpost created in a Region carry
/// that Region's partition in every ARN reported for them, from the create itself,
/// from the single-resource read, from the list, and — for the Outpost ARN a quote
/// copies — from `CreateQuote`.
#[tokio::test]
async fn minted_arns_carry_the_partition_of_their_region() {
    let outposts = Outposts::new();

    for &(region, partition) in REGIONS {
        let case = format!("{region} ({partition})");

        let site = outposts
            .ok_in(
                region,
                "POST",
                "/sites",
                Some(&json!({ "Name": format!("site in {region}") })),
            )
            .await["Site"]
            .clone();
        let site_id = text(&site, "SiteId");
        let site_arn = text(&site, "SiteArn");
        let expected_site_arn = expected_arn(partition, region, "site", &site_id);
        assert_eq!(
            site_arn, expected_site_arn,
            "case {case:?}: CreateSite must mint the Region's partition, got {site_arn:?}"
        );

        let read_site = get_site_in(&outposts, region, &site_id).await;
        assert_eq!(
            text(&read_site, "SiteArn"),
            expected_site_arn,
            "case {case:?}: GetSite must report the stored ARN, got {:?}",
            text(&read_site, "SiteArn")
        );
        let listed_site = listed_arn(
            &outposts.ok_in(region, "GET", "/sites", None).await,
            "Sites",
            ("SiteId", &site_id),
            "SiteArn",
        );
        assert_eq!(
            listed_site, expected_site_arn,
            "case {case:?}: ListSites must report the stored ARN, got {listed_site:?}"
        );

        let outpost = outposts
            .ok_in(
                region,
                "POST",
                "/outposts",
                Some(&json!({ "Name": format!("outpost in {region}"), "SiteId": site_id })),
            )
            .await["Outpost"]
            .clone();
        let outpost_id = text(&outpost, "OutpostId");
        let outpost_arn = text(&outpost, "OutpostArn");
        let expected_outpost_arn = expected_arn(partition, region, "outpost", &outpost_id);
        assert_eq!(
            outpost_arn, expected_outpost_arn,
            "case {case:?}: CreateOutpost must mint the Region's partition, got {outpost_arn:?}"
        );
        assert_eq!(
            text(&outpost, "SiteArn"),
            expected_site_arn,
            "case {case:?}: an Outpost reports its site's own ARN, got {:?}",
            text(&outpost, "SiteArn")
        );

        let read_outpost = get_outpost_in(&outposts, region, &outpost_id).await;
        for member in ["OutpostArn", "SiteArn"] {
            let expected = match member {
                "OutpostArn" => &expected_outpost_arn,
                _ => &expected_site_arn,
            };
            assert_eq!(
                &text(&read_outpost, member),
                expected,
                "case {case:?}: GetOutpost must report the stored {member}, got {:?}",
                text(&read_outpost, member)
            );
        }
        let listed_outpost = listed_arn(
            &outposts.ok_in(region, "GET", "/outposts", None).await,
            "Outposts",
            ("OutpostId", &outpost_id),
            "OutpostArn",
        );
        assert_eq!(
            listed_outpost, expected_outpost_arn,
            "case {case:?}: ListOutposts must report the stored ARN, got {listed_outpost:?}"
        );

        // A quote reports the Outpost's OWN `OutpostArn`, so it carries that
        // Outpost's partition too.
        let quote = outposts
            .ok_in(
                region,
                "POST",
                "/quotes",
                Some(&json!({
                    "CountryCode": "US",
                    "RequestedCapacities": [
                        { "QuoteCapacityType": "EC2", "Unit": "instances", "Quantity": 2.0 }
                    ],
                    "OutpostIdentifier": outpost_id,
                })),
            )
            .await["Quote"]
            .clone();
        assert_eq!(
            text(&quote, "OutpostArn"),
            expected_outpost_arn,
            "case {case:?}: a quote copies the Outpost's ARN, got {:?}",
            text(&quote, "OutpostArn")
        );
    }
}

/// The minted ARN is a usable identifier: every route that takes a site or an Outpost
/// accepts the ARN form of it, whatever partition that ARN carries, and resolves the
/// same resource as the bare id.
#[tokio::test]
async fn a_minted_arn_of_any_partition_is_accepted_as_the_identifier() {
    let outposts = Outposts::new();

    for &(region, partition) in REGIONS {
        let case = format!("{region} ({partition})");
        let site = outposts
            .ok_in(
                region,
                "POST",
                "/sites",
                Some(&json!({ "Name": format!("identifier site in {region}") })),
            )
            .await["Site"]
            .clone();
        let site_id = text(&site, "SiteId");
        let site_arn = text(&site, "SiteArn");
        let outpost = outposts
            .ok_in(
                region,
                "POST",
                "/outposts",
                Some(&json!({
                    "Name": format!("identifier outpost in {region}"),
                    "SiteId": site_arn,
                })),
            )
            .await["Outpost"]
            .clone();
        let outpost_id = text(&outpost, "OutpostId");
        let outpost_arn = text(&outpost, "OutpostArn");
        assert_eq!(
            text(&outpost, "SiteId"),
            site_id,
            "case {case:?}: CreateOutpost given the site's ARN must resolve that site, got {:?}",
            text(&outpost, "SiteId")
        );

        let by_arn = get_site_in(&outposts, region, &site_arn).await;
        let by_id = get_site_in(&outposts, region, &site_id).await;
        assert_eq!(
            by_arn, by_id,
            "case {case:?}: GetSite by ARN must answer the same site as by id, got {by_arn:?}"
        );

        let outpost_by_arn = get_outpost_in(&outposts, region, &outpost_arn).await;
        assert_eq!(
            text(&outpost_by_arn, "OutpostId"),
            outpost_id,
            "case {case:?}: GetOutpost by ARN must answer the same Outpost as by id, got {:?}",
            text(&outpost_by_arn, "OutpostId")
        );
    }
}
