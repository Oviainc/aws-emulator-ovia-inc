// SPDX-License-Identifier: CC0-1.0

//! Which `ServiceCode`s the quota plane RESOLVES, over the wire.
//!
//! `ListServices` is the discovery entry point and the catalog it enumerates is the
//! one set of codes the quota listings resolve, so the rules asserted here are about
//! coverage rather than about any one service's quota rows:
//!
//! * every `ServiceCode` AWS's integrated-service inventory publishes is in the
//!   catalog, and answers `ListAWSDefaultServiceQuotas` / `ListServiceQuotas` with
//!   HTTP 200 and a `Quotas` list — empty for a service whose published defaults this
//!   emulator does not carry, because the code itself exists;
//! * a refusal is reserved for a code that names no service AWS publishes
//!   (`NoSuchResourceException`), while a code outside the modelled constraints is an
//!   input refusal (`IllegalArgumentException`) — so "absent from the quota rows" is
//!   never what makes a call fail;
//! * the answer does not depend on the endpoint the catalog is read at.
//!
//! The harness (`client`, `service_inventory`, `walk_pages`) is the one
//! `quota_workflow.rs` owns.

use std::collections::BTreeSet;

use serde_json::json;

use service_com_amazonaws_servicequotas_servicequotasv20190624::state::MAX_PAGE_SIZE;

use crate::quota_workflow::{client, emulator, service_inventory, walk_pages, Client};

/// Service codes AWS's Service Quotas inventory publishes, each row differing in the
/// FORM of the code and in whether this emulator carries published quota rows for it.
///
/// SOURCE: AWS's own Service Quotas integrated-service inventory — the `ServiceCode`
/// values `aws service-quotas list-services` reports
/// (`smithy_query(query="shape", shape="ServiceInfo")` for the member). The rows span
/// the shortest code the modelled pattern admits (two characters), codes carrying
/// digits and upper-case letters, a code that is the prefix of another, codes with one
/// and with three hyphens, and the longest codes in the inventory — plus codes this
/// emulator carries quota rows for and codes it does not.
const PUBLISHED_SERVICE_CODES: &[(&str, &str)] = &[
    ("ts", "the shortest code the modelled pattern admits"),
    ("sms", "a three-letter code that is the prefix of another"),
    ("sms-voice", "the hyphenated extension of that code"),
    ("snow-device-management", "a code carrying three hyphens"),
    ("snowball", "a code the inventory lists beside the hyphenated one"),
    ("social-messaging", "a renamed messaging plane"),
    ("signin", "a code whose published quota rows this catalog carries"),
    ("support", "a code that is the prefix of two others"),
    ("supportapp", "one of those two"),
    ("supportauthz", "the other, which reports no display name"),
    ("thinclient", "a single-word code"),
    ("timestream-influxdb", "a hyphenated engine-specific plane"),
    ("tnb", "a three-letter acronym code"),
    ("vmimportexport", "a long single-word code"),
    ("voiceid", "a code sorting between two hyphenated ones"),
    ("wam", "a code the inventory still lists for a retired product"),
    ("workspaces-instances", "a hyphenated sibling of workspaces"),
    // Rows the issue behind this suite never cited: codes of other shapes, and codes
    // whose quota rows the catalog does carry.
    ("ec2", "a code carrying a digit, with quota rows"),
    ("AWSCloudMap", "a code carrying upper-case letters"),
    ("datazone", "a single-word code with no quota rows"),
    ("lookoutequipment", "one of three sibling codes"),
    ("lookoutmetrics", "another of them"),
    ("notifications", "a code that is the prefix of another"),
    ("pipes", "a five-letter code"),
    ("resiliencehub", "a single-word code"),
    ("vpc", "a three-letter code with many quota rows"),
];

/// Values that are NOT service codes AWS publishes, with the refusal each earns: a
/// well-formed code naming no service is a missing RESOURCE, while a value outside the
/// modelled `ServiceCode` constraints (length 1..63, pattern
/// `^[a-zA-Z][a-zA-Z0-9-]{1,63}$`) is bad INPUT.
///
/// Every client fault of this awsJson service is served on HTTP 400 and the body's
/// `__type` is what tells one refusal from another (`src/scaffolded.rs`).
const REFUSED_SERVICE_CODES: &[(&str, &str, &str)] = &[
    (
        "nosuchservice",
        "NoSuchResourceException",
        "a well-formed code naming no AWS service",
    ),
    (
        "Signin",
        "NoSuchResourceException",
        "a published code mis-cased",
    ),
    (
        "snowball-not",
        "NoSuchResourceException",
        "a published code with a suffix",
    ),
    (
        "t",
        "IllegalArgumentException",
        "one character, below the pattern's minimum",
    ),
    (
        "1signin",
        "IllegalArgumentException",
        "a leading digit, outside the pattern",
    ),
    (
        "sms_voice",
        "IllegalArgumentException",
        "an underscore, outside the pattern",
    ),
];

/// Whether `code` satisfies the modelled `ServiceCode` constraints: length 1..63 with
/// the pattern `^[a-zA-Z][a-zA-Z0-9-]{1,63}$`, which admits two characters at the
/// least (`smithy_query(query="shape", shape="ServiceInfo")`).
fn is_modelled_service_code(code: &str) -> bool {
    let mut rest = code.chars();
    rest.next().is_some_and(|first| first.is_ascii_alphabetic())
        && (2..=63).contains(&code.len())
        && rest.all(|char| char.is_ascii_alphanumeric() || char == '-')
}

/// The `ServiceCode`s `ListServices` enumerates.
async fn listed_codes(client: &Client) -> BTreeSet<String> {
    service_inventory(client)
        .await
        .iter()
        .map(|service| {
            service["ServiceCode"]
                .as_str()
                .unwrap_or_else(|| panic!("case ListServices: a service without a code: {service}"))
                .to_string()
        })
        .collect()
}

/// Every published code is enumerated and resolves BOTH quota listings with HTTP 200
/// and a `Quotas` list, at more than one endpoint.
#[tokio::test]
async fn every_published_service_code_resolves_its_quota_listings() {
    let engine = emulator();

    for region in ["us-west-2", "eu-central-1"] {
        let client = client(&engine, region);
        let listed = listed_codes(&client).await;
        let mut with_rows = 0_usize;
        let mut without_rows = 0_usize;

        for (code, form) in PUBLISHED_SERVICE_CODES {
            let case = format!("{region} {code} ({form})");
            assert!(
                is_modelled_service_code(code),
                "case {case}: this row must name a code satisfying the modelled constraints"
            );
            assert!(
                listed.contains(*code),
                "case {case}: the inventory a client discovers codes from must enumerate it, got \
                 {} entries",
                listed.len()
            );

            for operation in ["ListAWSDefaultServiceQuotas", "ListServiceQuotas"] {
                let base = json!({"ServiceCode": code});
                let (status, body) = client.call(operation, &base).await;
                assert_eq!(
                    status, 200,
                    "case {case}: {operation} must serve a published code, got HTTP {status} with \
                     {body}"
                );
                let served = body["Quotas"]
                    .as_array()
                    .unwrap_or_else(|| {
                        panic!("case {case}: {operation} must carry a Quotas list, got {body}")
                    })
                    .len();
                // The whole set of the code, walked with the tokens the service itself
                // issues at the largest page a caller may ask for (the model's
                // `MaxResults` @range 1..100): the unasked page is its first records, and a
                // `NextToken` follows it exactly while records remain past it.
                let whole =
                    walk_pages(&client, operation, &base, "Quotas", Some(MAX_PAGE_SIZE as i64))
                        .await;
                assert_eq!(
                    !body["NextToken"].as_str().unwrap_or_default().is_empty(),
                    served < whole.len(),
                    "case {case}: {operation} follows its unasked page with a NextToken exactly \
                     while records remain ({served} of {} served), got {body}",
                    whole.len()
                );
            }

            // The listing is the same set however it is paged, for a published code
            // whose rows the catalog carries and for one whose rows it does not.
            let base = json!({"ServiceCode": code});
            let whole = walk_pages(&client, "ListAWSDefaultServiceQuotas", &base, "Quotas", None)
                .await;
            let paged =
                walk_pages(&client, "ListAWSDefaultServiceQuotas", &base, "Quotas", Some(1)).await;
            assert_eq!(
                whole, paged,
                "case {case}: paging must yield the one listing, got {} and {} items",
                whole.len(),
                paged.len()
            );
            if whole.is_empty() {
                without_rows += 1;
            } else {
                with_rows += 1;
            }
        }

        // The table must actually span both sides of the rule: codes whose published
        // defaults this catalog carries, and codes it does not.
        assert!(
            with_rows >= 3 && without_rows >= 3,
            "case {region}: the table must span codes with and without published quota rows, got \
             {with_rows} and {without_rows}"
        );
    }
}

/// A refusal belongs to a code that names no AWS service, or to a value the model
/// rejects — never to a published code whose quota rows the catalog does not carry.
#[tokio::test]
async fn only_a_code_that_names_no_service_is_refused() {
    let engine = emulator();
    let client = client(&engine, "ap-southeast-2");
    let listed = listed_codes(&client).await;

    for (code, error, why) in REFUSED_SERVICE_CODES {
        let case = format!("{code:?} ({why})");
        assert!(
            !listed.contains(*code),
            "case {case}: this row must name a value the catalog does NOT hold"
        );
        for operation in ["ListAWSDefaultServiceQuotas", "ListServiceQuotas"] {
            let (status, answered) = client
                .refused(operation, json!({"ServiceCode": code}))
                .await;
            assert_eq!(
                (status, answered.as_str()),
                (400, *error),
                "case {case}: expected {operation} to answer {error}"
            );
        }
    }
}

/// A code the inventory enumerates whose published defaults this emulator does not
/// carry still resolves: the EMPTY list is its answer, and the refusal that remains is
/// about the quota asked for, not about the service.
#[tokio::test]
async fn a_published_code_without_quota_rows_resolves_empty() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    // Rows: published codes this catalog carries no quota rows for, of several forms.
    for code in [
        "ts",
        "snow-device-management",
        "timestream-influxdb",
        "workspaces-instances",
        "datazone",
        "notifications",
    ] {
        let listing = client
            .ok(
                "ListAWSDefaultServiceQuotas",
                json!({"ServiceCode": code}),
            )
            .await;
        assert_eq!(
            listing["Quotas"],
            json!([]),
            "case {code}: a code with no published rows resolves with an empty list, got {listing}"
        );

        // The quota plane of the same code is empty too, and a quota lookup on it is
        // refused for the QUOTA — the service resolved.
        let applied = client
            .ok("ListServiceQuotas", json!({"ServiceCode": code}))
            .await;
        assert_eq!(
            applied["Quotas"],
            json!([]),
            "case {code}: the applied listing of a code with no rows is empty too, got {applied}"
        );
        let (status, error) = client
            .refused(
                "GetAWSDefaultServiceQuota",
                json!({"ServiceCode": code, "QuotaCode": "L-F678F1CE"}),
            )
            .await;
        assert_eq!(
            (status, error.as_str()),
            (400, "NoSuchResourceException"),
            "case {code}: a quota this service publishes nothing under is the missing resource"
        );
    }
}

/// The quota rows a newly covered code DOES carry read back through the single read,
/// field for field, with the ARN form an AWS default carries.
#[tokio::test]
async fn a_newly_covered_code_reads_its_published_rows_back() {
    let engine = emulator();
    let region = "us-west-2";
    let client = client(&engine, region);

    let listing = walk_pages(
        &client,
        "ListAWSDefaultServiceQuotas",
        &json!({"ServiceCode": "signin"}),
        "Quotas",
        None,
    )
    .await;
    assert!(
        !listing.is_empty(),
        "case signin: expected the published rows this catalog carries, got {listing:?}"
    );

    for quota in &listing {
        let code = quota["QuotaCode"]
            .as_str()
            .unwrap_or_else(|| panic!("case signin: a quota without a code: {quota}"));
        assert_eq!(
            quota["ServiceName"],
            json!("AWS IAM Identity Center"),
            "case signin/{code}: every row reports the service's display name, got {quota}"
        );
        assert_eq!(
            quota["QuotaArn"],
            json!(format!("arn:aws:servicequotas:{region}::signin/{code}")),
            "case signin/{code}: an AWS default quota's ARN names the Region and no account, got \
             {quota}"
        );
        let single = client
            .ok(
                "GetAWSDefaultServiceQuota",
                json!({"ServiceCode": "signin", "QuotaCode": code}),
            )
            .await;
        assert_eq!(
            single["Quota"], *quota,
            "case signin/{code}: the single read must return the listed record, got {single}"
        );
    }
}

/// The inventory holds each code once, in `ServiceCode` order, and every entry
/// satisfies the modelled constraints — so a code added to the catalog cannot shadow
/// another or render a value the model rejects.
#[tokio::test]
async fn the_inventory_holds_each_published_code_once() {
    let engine = emulator();
    let client = client(&engine, "eu-west-1");
    let inventory = service_inventory(&client).await;

    let codes: Vec<&str> = inventory
        .iter()
        .map(|service| service["ServiceCode"].as_str().unwrap_or_default())
        .collect();
    let mut sorted = codes.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        codes, sorted,
        "case ListServices: expected ServiceCode order with no repeats, got {codes:?}"
    );

    for service in &inventory {
        let code = service["ServiceCode"].as_str().unwrap_or_default();
        assert!(
            is_modelled_service_code(code),
            "case {code:?}: every enumerated code satisfies the modelled constraints, got \
             {service}"
        );
        assert!(
            service["ServiceName"]
                .as_str()
                .is_some_and(|name| !name.is_empty()),
            "case {code:?}: every enumerated service reports a name, got {service}"
        );
    }

    assert!(
        codes.len() > PUBLISHED_SERVICE_CODES.len(),
        "case ListServices: the inventory must enumerate the whole published catalog, got {} \
         entries",
        codes.len()
    );
}
