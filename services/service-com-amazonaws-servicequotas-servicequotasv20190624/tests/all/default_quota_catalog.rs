// SPDX-License-Identifier: CC0-1.0

//! The Service Quotas CATALOG contract, over the wire.
//!
//! `ListServices` is the documented discovery entry point: a client enumerates it and
//! then calls `ListServiceQuotas` / `ListAWSDefaultServiceQuotas` per code. These
//! suites assert the RULES that tie the three together, each over a TABLE of inputs
//! (the whole catalog, several regions, several page sizes) rather than one case:
//!
//! * every `ServiceCode` the catalog enumerates RESOLVES — HTTP 200 with a `Quotas`
//!   member — and `NoSuchResourceException` is reserved for a code the catalog does
//!   not hold;
//! * a service whose published defaults are not seeded still resolves, with an EMPTY
//!   `Quotas` list, because the code exists;
//! * the ARN of an AWS DEFAULT quota carries no account segment, while the APPLIED
//!   quota of the same pair carries the account;
//! * a listing called with no `MaxResults` returns every item in one page, and the
//!   same items come back however the listing is paged.
//!
//! The harness (`client`, `walk_pages`, `ACCOUNT`) is the one `quota_workflow.rs`
//! already uses, so both suites speak to the emulator the same way.

use serde_json::{json, Value};

use crate::quota_workflow::{client, emulator, walk_pages, Client, ACCOUNT};

/// Every `(ServiceCode, ServiceName)` pair `ListServices` reports, as one page.
async fn catalog(client: &Client) -> Vec<Value> {
    walk_pages(client, "ListServices", &json!({}), "Services", None).await
}

/// The `ServiceCode`s of a catalog listing.
fn codes(catalog: &[Value]) -> Vec<String> {
    catalog
        .iter()
        .map(|service| {
            service["ServiceCode"]
                .as_str()
                .unwrap_or_else(|| panic!("case ListServices: a service without a code: {service}"))
                .to_string()
        })
        .collect()
}

/// Whether `code` satisfies the modelled `ServiceCode` constraints: length 1..63 and
/// pattern `^[a-zA-Z][a-zA-Z0-9-]{1,63}$`
/// (`smithy_query(query="shape", shape="ServiceInfo")`).
fn is_modelled_service_code(code: &str) -> bool {
    let mut chars = code.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (2..=63).contains(&code.len())
        && first.is_ascii_alphabetic()
        && chars.all(|char| char.is_ascii_alphanumeric() || char == '-')
}

/// Every code the catalog enumerates resolves its AWS defaults, and only a code the
/// catalog does NOT hold is refused.
#[tokio::test]
async fn every_catalog_service_code_resolves_its_aws_default_quotas() {
    let engine = emulator();
    let client = client(&engine, "us-west-2");

    // The table is the WHOLE catalog — one row per service code, several hundred of
    // them, far more than the five the quota rows cover.
    let catalog = catalog(&client).await;
    let codes = codes(&catalog);
    assert!(
        codes.len() > 100,
        "case ListServices: the catalog must enumerate the services that publish \
         quotas, got {} entries",
        codes.len()
    );

    for code in &codes {
        let (status, body) = client
            .call(
                "ListAWSDefaultServiceQuotas",
                &json!({"ServiceCode": code}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {code}: a code ListServices enumerates must resolve, got HTTP {status} with \
             {body}"
        );
        assert!(
            body["Quotas"].is_array(),
            "case {code}: expected a Quotas list, got {body}"
        );
        // And the applied listing resolves the same set of codes.
        let (applied_status, applied_body) = client
            .call("ListServiceQuotas", &json!({"ServiceCode": code}))
            .await;
        assert_eq!(
            applied_status, 200,
            "case {code}: ListServiceQuotas must resolve the same codes as \
             ListAWSDefaultServiceQuotas, got HTTP {applied_status} with {applied_body}"
        );
    }

    // Entries the issue that prompted this suite never cited, resolved by name: a
    // service with seeded quotas, and three without.
    for code in ["ec2", "sns", "AWSCloudMap", "kinesisanalytics"] {
        assert!(
            codes.iter().any(|listed| listed == code),
            "case {code}: expected the catalog to enumerate it, got {} entries",
            codes.len()
        );
        let listed = client
            .ok(
                "ListAWSDefaultServiceQuotas",
                json!({"ServiceCode": code}),
            )
            .await;
        assert!(
            listed["Quotas"].is_array(),
            "case {code}: expected a Quotas list, got {listed}"
        );
    }

    // The complement: codes that satisfy the modelled pattern but name no service in
    // the catalog are refused — that is what NoSuchResourceException is for. Every
    // refusal here is a CLIENT fault, so this awsJson front end serves it on HTTP 400
    // whatever `@httpError` the shape carries (`crate::scaffolded::wire_http_status`);
    // the body's `__type` is what names the refusal.
    for unknown in [
        "nosuchservice",
        "dynamodb-not",
        "DynamoDB",
        "VPC",
        "zz-unlisted-service",
    ] {
        assert!(
            !codes.iter().any(|listed| listed == unknown),
            "case {unknown}: this row must name a code the catalog does NOT hold"
        );
        let (status, code) = client
            .refused(
                "ListAWSDefaultServiceQuotas",
                json!({"ServiceCode": unknown}),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "NoSuchResourceException"),
            "case {unknown}: expected NoSuchResourceException for a code outside the catalog"
        );
    }
}

/// The catalog is ordered by `ServiceCode`, holds each code once, and every entry
/// satisfies the modelled `ServiceCode` constraints.
#[tokio::test]
async fn the_catalog_is_ordered_and_every_entry_is_well_formed() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");
    let catalog = catalog(&client).await;
    let codes = codes(&catalog);

    let mut expected = codes.clone();
    expected.sort_unstable();
    expected.dedup();
    assert_eq!(
        codes, expected,
        "case ListServices: expected ServiceCode order with no repeats, got {codes:?}"
    );

    for service in &catalog {
        let code = service["ServiceCode"].as_str().unwrap_or_default();
        assert!(
            is_modelled_service_code(code),
            "case {code:?}: every ServiceCode must satisfy the modelled pattern, got {service}"
        );
        assert!(
            service["ServiceName"]
                .as_str()
                .is_some_and(|name| !name.is_empty()),
            "case {code:?}: every service reports a ServiceName, got {service}"
        );
    }
}

/// A service the catalog enumerates but whose published defaults are not seeded
/// resolves with an EMPTY quota list — never a 404 — and a quota code it does not
/// hold is still refused.
#[tokio::test]
async fn a_service_without_seeded_defaults_resolves_with_an_empty_list() {
    let engine = emulator();
    let client = client(&engine, "eu-west-1");
    let catalog = catalog(&client).await;

    // The rows are the catalog codes whose default set is empty: assert there ARE
    // such codes (the catalog is broader than the seeded quota rows), then that each
    // of them answers 200 with an empty list on both listings.
    let mut empty = Vec::new();
    for code in codes(&catalog) {
        let listed = client
            .ok(
                "ListAWSDefaultServiceQuotas",
                json!({"ServiceCode": code.clone()}),
            )
            .await;
        if listed["Quotas"].as_array().is_some_and(Vec::is_empty) {
            assert_eq!(
                listed["NextToken"],
                Value::Null,
                "case {code}: an empty listing is the last page, got {listed}"
            );
            let applied = client
                .ok("ListServiceQuotas", json!({"ServiceCode": code.clone()}))
                .await;
            assert_eq!(
                applied["Quotas"],
                json!([]),
                "case {code}: the applied listing of a service with no defaults is empty too, \
                 got {applied}"
            );
            // A pair that is not in the seeded set is still a missing RESOURCE — on
            // this awsJson front end a client fault is HTTP 400 and `__type` names it.
            let (status, error) = client
                .refused(
                    "GetAWSDefaultServiceQuota",
                    json!({"ServiceCode": code.clone(), "QuotaCode": "L-F678F1CE"}),
                )
                .await;
            assert_eq!(
                (status, error.as_str()),
                (400, "NoSuchResourceException"),
                "case {code}: a quota code this service has no default for must be refused"
            );
            empty.push(code);
        }
    }

    assert!(
        empty.len() > 10,
        "case catalog: the catalog must enumerate services whose defaults are not \
         seeded, got {empty:?}"
    );
}

/// An AWS DEFAULT quota's ARN carries an EMPTY account segment; the APPLIED quota of
/// the same pair carries the account. Asserted for every seeded pair, from two
/// regions.
#[tokio::test]
async fn an_aws_default_quota_arn_names_no_account_and_the_applied_one_does() {
    let engine = emulator();

    // The rows: every (region, service, quota) the catalog can produce, over two
    // regions — so the rule is asserted for a count quota, a measured one, a global
    // one and a regional one, at more than one endpoint.
    for region in ["us-west-2", "ap-northeast-1"] {
        let client = client(&engine, region);
        let mut pairs = 0_usize;

        for service_code in codes(&catalog(&client).await) {
            let defaults = walk_pages(
                &client,
                "ListAWSDefaultServiceQuotas",
                &json!({"ServiceCode": service_code.clone()}),
                "Quotas",
                None,
            )
            .await;

            for quota in &defaults {
                let quota_code = quota["QuotaCode"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {service_code}: a quota without a code"))
                    .to_string();
                let case = format!("{region} {service_code}/{quota_code}");
                // A global quota is account-wide, so its ARN carries no region
                // either; a regional one carries the caller's.
                let global = quota["GlobalQuota"].as_bool().unwrap_or(false);
                let arn_region = if global { "" } else { region };

                assert_eq!(
                    quota["QuotaArn"].as_str(),
                    Some(
                        format!(
                            "arn:aws:servicequotas:{arn_region}::{service_code}/{quota_code}"
                        )
                        .as_str()
                    ),
                    "case {case}: an AWS default quota's ARN carries no account, got {quota}"
                );

                let pair = json!({"ServiceCode": service_code, "QuotaCode": quota_code});
                let single = client.ok("GetAWSDefaultServiceQuota", pair.clone()).await;
                assert_eq!(
                    single["Quota"], *quota,
                    "case {case}: the single default read must match the listed entry, got \
                     {single}"
                );

                let applied = client.ok("GetServiceQuota", pair.clone()).await;
                assert_eq!(
                    applied["Quota"]["QuotaArn"].as_str(),
                    Some(
                        format!(
                            "arn:aws:servicequotas:{arn_region}:{ACCOUNT}:{service_code}/\
                             {quota_code}"
                        )
                        .as_str()
                    ),
                    "case {case}: an applied quota's ARN names the account, got {applied}"
                );
                pairs += 1;
            }
        }

        assert!(
            pairs > 0,
            "case {region}: the seeded catalog must cover at least one quota"
        );
    }
}

/// A listing called with no `MaxResults` returns EVERY item in one page; a caller
/// that names a page size gets that page size, and walking the pages yields the same
/// items.
#[tokio::test]
async fn a_listing_with_no_page_size_returns_every_item_in_one_page() {
    let engine = emulator();
    let client = client(&engine, "us-east-1");

    // Rows: a listing far longer than the modelled MaxResults ceiling (the catalog),
    // and listings shorter than it (the seeded quota sets).
    let listings = [
        ("ListServices", json!({}), "Services"),
        (
            "ListAWSDefaultServiceQuotas",
            json!({"ServiceCode": "vpc"}),
            "Quotas",
        ),
        (
            "ListAWSDefaultServiceQuotas",
            json!({"ServiceCode": "dynamodb"}),
            "Quotas",
        ),
        (
            "ListAWSDefaultServiceQuotas",
            json!({"ServiceCode": "acm"}),
            "Quotas",
        ),
    ];

    for (operation, body, items) in &listings {
        let whole = client.ok(operation, body.clone()).await;
        let page = whole[items]
            .as_array()
            .unwrap_or_else(|| panic!("case {operation}: expected a {items} list, got {whole}"))
            .clone();
        assert_eq!(
            whole["NextToken"],
            Value::Null,
            "case {operation}: a listing with no MaxResults is one whole page, got a NextToken \
             after {} items",
            page.len()
        );

        // Paging it yields exactly the same items, in the same order.
        for size in [1_i64, 7, 100] {
            if page.len() as i64 / size > 1_000 {
                continue;
            }
            let paged = walk_pages(&client, operation, body, items, Some(size)).await;
            assert_eq!(
                paged, page,
                "case {operation} MaxResults={size}: paging must yield the one listing"
            );
        }
    }

    // The catalog is longer than the largest page a caller may ask for, so a page
    // size SHORTENS it and carries a token.
    let catalog = client.ok("ListServices", json!({})).await["Services"]
        .as_array()
        .expect("a Services list")
        .len();
    let first = client.ok("ListServices", json!({"MaxResults": 100})).await;
    assert_eq!(
        first["Services"].as_array().map(Vec::len),
        Some(100),
        "case ListServices MaxResults=100: expected a full page, got {first}"
    );
    assert!(
        catalog > 100 && first["NextToken"].as_str().is_some(),
        "case ListServices MaxResults=100: a {catalog}-entry catalog must carry a NextToken, got \
         {first}"
    );
}
