// SPDX-License-Identifier: CC0-1.0

//! The managed products of ONE vendor, end to end over the wire.
//!
//! The rule asserted here: `DescribeManagedProductsByVendor` reports exactly the
//! managed products whose publishing vendor is the requested `VendorName` — the
//! entries of the published catalogue
//! (`service_…::state::MANAGED_RULE_GROUP_CATALOGUE`) matched on that name, in
//! published order, each carrying the same descriptor
//! `DescribeAllManagedProducts` reports for it — so a vendor that publishes
//! nothing here is reported with an empty list, and the report is the same in
//! either scope and however much this account holds.
//!
//! Every expectation below is composed FROM the catalogue and from what
//! `DescribeAllManagedProducts` answers, never written as a literal. Every call
//! goes through the router's front door with the harness in
//! [`crate::web_acl_lifecycle`], so what these tests observe is what an SDK
//! observes.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::MANAGED_RULE_GROUP_CATALOGUE;

use crate::managed_rule_groups::managed_statement;
use crate::web_acl_lifecycle::{create_web_acl, visibility, Waf};

/// The products a response reports, in the order it reported them.
fn reported(response: &Value) -> Vec<Value> {
    response["ManagedProducts"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Every vendor the catalogue publishes a group under.
fn publishing_vendors() -> BTreeSet<&'static str> {
    MANAGED_RULE_GROUP_CATALOGUE
        .iter()
        .map(|group| group.vendor_name)
        .collect()
}

/// Names no catalogued group is published under, one per shape of a miss: a word
/// nothing publishes, a publishing vendor's name in the wrong case, one with a
/// trailing space, the shortest name the model admits, and the longest.
///
/// `VendorName` is `length 1..128, pattern \S`
/// (`smithy_query(query="operation",operation="DescribeManagedProductsByVendor")`),
/// so each row below is a value the request boundary admits and the catalogue
/// still does not match.
fn vendors_that_publish_nothing() -> Vec<String> {
    let publishing = MANAGED_RULE_GROUP_CATALOGUE[0].vendor_name;
    vec![
        "SomeOtherVendor".to_string(),
        publishing.to_lowercase(),
        format!("{publishing} "),
        "x".to_string(),
        "v".repeat(128),
    ]
}

/// Each vendor is reported exactly the products it publishes, in published order
/// and with the same descriptor the whole-catalogue report carries for them, and a
/// vendor that publishes nothing here is reported an empty list.
///
/// The table crosses every vendor the catalogue publishes under, plus five names
/// it publishes nothing under (an unknown word, a case variant, a trailing space,
/// and the shortest and longest names the model admits), with both declared
/// scopes.
#[tokio::test]
async fn a_vendor_is_reported_exactly_the_products_it_publishes() {
    let waf = Waf::new();
    let vendors: Vec<String> = publishing_vendors()
        .into_iter()
        .map(str::to_string)
        .chain(vendors_that_publish_nothing())
        .collect();

    for scope in ["CLOUDFRONT", "REGIONAL"] {
        // What the whole catalogue is reported as, in this scope: the by-vendor
        // report must be this list filtered on `VendorName`, descriptor for
        // descriptor.
        let all = reported(&waf.ok("DescribeAllManagedProducts", json!({ "Scope": scope })).await);

        for vendor in &vendors {
            let case = format!("{scope}/{vendor}");
            let response = waf
                .ok(
                    "DescribeManagedProductsByVendor",
                    json!({ "Scope": scope, "VendorName": vendor }),
                )
                .await;
            let products = reported(&response);

            let expected: Vec<Value> = all
                .iter()
                .filter(|product| product["VendorName"] == json!(vendor))
                .cloned()
                .collect();
            assert_eq!(
                products, expected,
                "case {case:?}: expected the catalogue's products of this vendor \
                 ({} of them), got {response}",
                expected.len()
            );

            let catalogued = MANAGED_RULE_GROUP_CATALOGUE
                .iter()
                .filter(|group| group.vendor_name == vendor.as_str())
                .count();
            assert_eq!(
                products.len(),
                catalogued,
                "case {case:?}: expected one product per catalogued group of this \
                 vendor ({catalogued}), got {}: {response}",
                products.len()
            );
            assert!(
                products
                    .iter()
                    .all(|product| product["VendorName"] == json!(vendor)),
                "case {case:?}: every reported product must be published by the \
                 requested vendor, got {response}"
            );
            // "A unique identifier for the rule group" (`ProductId`): no two
            // reported products share one.
            let identifiers: BTreeSet<&str> = products
                .iter()
                .filter_map(|product| product["ProductId"].as_str())
                .collect();
            assert_eq!(
                identifiers.len(),
                products.len(),
                "case {case:?}: every product identifier must be unique, got {response}"
            );
        }
    }
}

/// Across vendors the report partitions the catalogue: the products reported for
/// every publishing vendor, taken together, are exactly the whole-catalogue
/// report.
#[tokio::test]
async fn the_per_vendor_reports_partition_the_catalogue() {
    let waf = Waf::new();
    let all = reported(
        &waf.ok("DescribeAllManagedProducts", json!({ "Scope": "REGIONAL" }))
            .await,
    );
    let mut per_vendor: Vec<Value> = Vec::new();
    for vendor in publishing_vendors() {
        let response = waf
            .ok(
                "DescribeManagedProductsByVendor",
                json!({ "Scope": "REGIONAL", "VendorName": vendor }),
            )
            .await;
        per_vendor.extend(reported(&response));
    }
    let partitioned: BTreeSet<String> = per_vendor.iter().map(Value::to_string).collect();
    let whole: BTreeSet<String> = all.iter().map(Value::to_string).collect();
    assert_eq!(
        partitioned.len(),
        per_vendor.len(),
        "no product may be reported twice across vendors, got {per_vendor:?}"
    );
    assert_eq!(
        partitioned, whole,
        "the per-vendor reports together must be the whole catalogue report, got \
         {per_vendor:?} against {all:?}"
    );
}

/// A `VendorName` outside the model's `length 1..128` is refused, not truncated or
/// matched loosely; a name on either published bound is answered. One row per
/// bound and one past each.
///
/// The model's own `@length` is enforced at the request boundary, so the refusal a
/// caller sees is the framework's `ValidationException` naming the member.
#[tokio::test]
async fn a_vendor_name_outside_the_modelled_length_is_refused() {
    let waf = Waf::new();

    for length in [1_usize, 128] {
        let vendor = "v".repeat(length);
        let case = format!("VendorName of {length} characters");
        let response = waf
            .ok(
                "DescribeManagedProductsByVendor",
                json!({ "Scope": "REGIONAL", "VendorName": &vendor }),
            )
            .await;
        assert_eq!(
            reported(&response).len(),
            MANAGED_RULE_GROUP_CATALOGUE
                .iter()
                .filter(|group| group.vendor_name == vendor.as_str())
                .count(),
            "case {case:?}: a name on the bound must be answered with that vendor's \
             products, got {response}"
        );
    }

    for length in [0_usize, 129, 200] {
        let vendor = "v".repeat(length);
        let case = format!("VendorName of {length} characters");
        let response = waf
            .refused(
                "DescribeManagedProductsByVendor",
                json!({ "Scope": "REGIONAL", "VendorName": &vendor }),
                "ValidationException",
                &case,
            )
            .await;
        let message = response["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("'vendorName'"),
            "case {case:?}: the refusal must name the member at fault, got {response}"
        );
        assert!(
            response["ManagedProducts"].is_null(),
            "case {case:?}: a refusal must report no products, got {response}"
        );
    }
}

/// A `Scope` that is not one of the two the model declares is refused, and the
/// refusal reports no products: the two it declares are the only namespaces a
/// request can be made in. One row per shape of a wrong value — an unknown word, a
/// declared value in the wrong case, one with a trailing space, and the empty
/// string.
#[tokio::test]
async fn a_scope_the_model_does_not_declare_is_refused() {
    let waf = Waf::new();
    let vendor = MANAGED_RULE_GROUP_CATALOGUE[0].vendor_name;
    for scope in ["GLOBAL", "regional", "CLOUDFRONT ", ""] {
        let response = waf
            .refused(
                "DescribeManagedProductsByVendor",
                json!({ "Scope": scope, "VendorName": vendor }),
                "ValidationException",
                scope,
            )
            .await;
        let message = response["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("'scope'"),
            "case {scope:?}: the refusal must name the member at fault, got {response}"
        );
        assert!(
            response["ManagedProducts"].is_null(),
            "case {scope:?}: a refusal must report no products, got {response}"
        );
    }
}

/// Reporting a vendor's products changes nothing a later caller sees, and what is
/// reported does not depend on what this account holds: the products are the
/// vendor's publications, so saving a web ACL that RUNS one of them leaves the
/// report identical.
///
/// The web ACL in the middle also crosses the two: a pair the report names is a
/// pair a save accepts, so the report names the same catalogue the save validates
/// against.
#[tokio::test]
async fn reporting_a_vendors_products_changes_nothing() {
    let waf = Waf::new();
    let vendor = MANAGED_RULE_GROUP_CATALOGUE[0].vendor_name;
    let body = json!({ "Scope": "REGIONAL", "VendorName": vendor });

    let first = waf
        .ok("DescribeManagedProductsByVendor", body.clone())
        .await;
    let second = waf
        .ok("DescribeManagedProductsByVendor", body.clone())
        .await;
    assert_eq!(
        first, second,
        "two reads in a row must report the same products, got {first} then {second}"
    );

    for product in reported(&first) {
        let name = product["ManagedRuleSetName"].as_str().unwrap_or_default();
        let case = format!("{vendor}/{name}");
        let acl_name = format!("by-vendor-{}", name.to_lowercase());
        let (acl_id, _arn, _lock) = create_web_acl(
            &waf,
            "REGIONAL",
            &acl_name,
            json!([{
                "Name": "managed",
                "Priority": 0,
                "Statement": managed_statement(vendor, name),
                "OverrideAction": { "None": {} },
                "VisibilityConfig": visibility("managed"),
            }]),
        )
        .await;
        let got = waf
            .ok(
                "GetWebACL",
                json!({ "Name": &acl_name, "Scope": "REGIONAL", "Id": &acl_id }),
            )
            .await;
        assert_eq!(
            got["WebACL"]["Rules"][0]["Statement"],
            managed_statement(vendor, name),
            "case {case:?}: a reported product must be a pair a web ACL may run, \
             got {got}"
        );
    }

    let after = waf.ok("DescribeManagedProductsByVendor", body).await;
    assert_eq!(
        after, first,
        "what this account holds must not change the reported products, got {after} \
         after {first}"
    );
}
