// SPDX-License-Identifier: CC0-1.0

//! The managed PRODUCTS this service publishes, end to end over the wire.
//!
//! The rule asserted here: `DescribeAllManagedProducts` reports one descriptor per
//! managed rule group the service publishes — the same catalogue
//! (`service_…::state::MANAGED_RULE_GROUP_CATALOGUE`) that decides whether a
//! `ManagedRuleGroupStatement` names a group that exists — carrying that group's
//! published identity and properties, and reports it identically in either scope
//! and however much this account holds, because a managed product is a vendor's
//! publication rather than a resource of the account.
//!
//! Every expectation below is composed FROM that catalogue rather than written as a
//! literal, and the table spans every group in it, in both scopes. Every call goes
//! through the router's front door with the harness in
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

/// The notification topic every change to a published managed rule group is
/// announced on: ONE topic, in the region its publisher publishes it in, whichever
/// group is reported and whatever region the caller signed for.
fn expected_topic() -> String {
    format!(
        "arn:aws:sns:us-east-1:{}:aws-managed-waf-rule-notifications",
        ::engine::emulator_account_id()
    )
}

/// Every group the catalogue publishes is reported as a managed product, in
/// published order, with its vendor, name, identifier, title, description, the two
/// flags it is published with and the ARN of the topic its changes are announced
/// on — and the same products are reported in either scope, because the products
/// are the vendors' and a scope is only the namespace a request is made in.
///
/// The table is the WHOLE catalogue, one row per published group, crossed with both
/// declared scopes: it spans versioned and unversioned groups, advanced and
/// ordinary ones, and capacities from the smallest to the largest the catalogue
/// publishes.
#[tokio::test]
async fn every_published_group_is_reported_as_a_managed_product() {
    // The rows below are only worth crossing if the catalogue spans both values of
    // each published flag.
    assert!(
        MANAGED_RULE_GROUP_CATALOGUE
            .iter()
            .any(|group| group.is_versioning_supported)
            && MANAGED_RULE_GROUP_CATALOGUE
                .iter()
                .any(|group| !group.is_versioning_supported),
        "the catalogue must publish both versioned and unversioned groups for this \
         table to span the class"
    );

    let waf = Waf::new();
    for scope in ["CLOUDFRONT", "REGIONAL"] {
        let response = waf
            .ok("DescribeAllManagedProducts", json!({ "Scope": scope }))
            .await;
        let products = reported(&response);
        assert_eq!(
            products.len(),
            MANAGED_RULE_GROUP_CATALOGUE.len(),
            "case {scope:?}: expected one product per published group \
             ({}), got {}: {response}",
            MANAGED_RULE_GROUP_CATALOGUE.len(),
            products.len()
        );

        for (group, product) in MANAGED_RULE_GROUP_CATALOGUE.iter().zip(products.iter()) {
            let case = format!("{scope}/{}", group.name);
            assert_eq!(
                product["VendorName"],
                json!(group.vendor_name),
                "case {case:?}: expected the publishing vendor {:?}, got {product}",
                group.vendor_name
            );
            assert_eq!(
                product["ManagedRuleSetName"],
                json!(group.name),
                "case {case:?}: expected the published group name, got {product}"
            );
            assert_eq!(
                product["ProductId"],
                json!(group.name),
                "case {case:?}: expected the group's own name as its identifier, \
                 got {product}"
            );
            assert_eq!(
                product["ProductTitle"],
                json!(group.product_title),
                "case {case:?}: expected the published title {:?}, got {product}",
                group.product_title
            );
            assert_eq!(
                product["ProductDescription"],
                json!(group.product_description),
                "case {case:?}: expected the published description, got {product}"
            );
            assert_eq!(
                product["IsVersioningSupported"],
                json!(group.is_versioning_supported),
                "case {case:?}: expected IsVersioningSupported {}, got {product}",
                group.is_versioning_supported
            );
            assert_eq!(
                product["IsAdvancedManagedRuleSet"],
                json!(group.is_advanced_managed_rule_set),
                "case {case:?}: expected IsAdvancedManagedRuleSet {}, got {product}",
                group.is_advanced_managed_rule_set
            );
            // One topic for every announced change, in the emulator's account, like
            // every other ARN this service answers.
            let expected_topic = expected_topic();
            assert_eq!(
                product["SnsTopicArn"],
                json!(expected_topic),
                "case {case:?}: expected the notification topic {expected_topic:?}, \
                 got {product}"
            );
            // "For Amazon Web Services Marketplace managed rule groups only"
            // (`ManagedProductDescriptor$ProductLink`), and this catalogue publishes
            // no marketplace group.
            assert!(
                product["ProductLink"].is_null(),
                "case {case:?}: a group its own vendor publishes has no marketplace \
                 product link, got {product}"
            );
        }

        // "A unique identifier for the rule group" (`ProductId`): no two reported
        // products share one.
        let identifiers: BTreeSet<&str> = products
            .iter()
            .filter_map(|product| product["ProductId"].as_str())
            .collect();
        assert_eq!(
            identifiers.len(),
            products.len(),
            "case {scope:?}: every product identifier must be unique, got {response}"
        );
    }
}

/// A `Scope` that is not one of the two the model declares is refused, and the
/// refusal reports no products: the two it declares are the only namespaces a
/// request can be made in. One row per shape of a wrong value — an unknown word, a
/// declared value in the wrong case, one with a trailing space, and the empty
/// string.
///
/// `Scope` is a modelled enum, so the value set is enforced at the request
/// boundary and the refusal names the member and the admitted values; the
/// handler's own `is_known_scope` guard covers anything that gets past it.
#[tokio::test]
async fn a_scope_the_model_does_not_declare_is_refused() {
    let waf = Waf::new();
    for scope in ["GLOBAL", "regional", "CLOUDFRONT ", ""] {
        let response = waf
            .refused(
                "DescribeAllManagedProducts",
                json!({ "Scope": scope }),
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

/// Reporting the managed products changes nothing a later caller sees, and what is
/// reported does not depend on what this account holds: the products are the
/// vendors' publications, so saving a web ACL that RUNS one of them leaves the
/// report identical.
///
/// The web ACL in the middle also crosses the two: a pair the report names is a
/// pair a save accepts, so the describe reports the same catalogue the save
/// validates against.
#[tokio::test]
async fn reporting_the_managed_products_changes_nothing() {
    let waf = Waf::new();
    let body = json!({ "Scope": "REGIONAL" });

    let first = waf.ok("DescribeAllManagedProducts", body.clone()).await;
    let second = waf.ok("DescribeAllManagedProducts", body.clone()).await;
    assert_eq!(
        first, second,
        "two reads in a row must report the same products, got {first} then {second}"
    );

    // Run a group the report named, from a web ACL of this account.
    let products = reported(&first);
    for product in &products {
        let vendor = product["VendorName"].as_str().unwrap_or_default();
        let name = product["ManagedRuleSetName"].as_str().unwrap_or_default();
        let case = format!("{vendor}/{name}");
        let acl_name = format!("product-{}", name.to_lowercase());
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

    let after = waf.ok("DescribeAllManagedProducts", body).await;
    assert_eq!(
        after, first,
        "what this account holds must not change the reported products, got {after} \
         after {first}"
    );
}
