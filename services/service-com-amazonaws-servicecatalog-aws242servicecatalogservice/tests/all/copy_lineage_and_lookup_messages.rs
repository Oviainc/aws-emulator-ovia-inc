// SPDX-License-Identifier: CC0-1.0

//! Two rules, over the wire as awsJson1_1 requests through the same
//! `MultiServiceRouter` the production emulator mounts.
//!
//! 1. A product version a copy created reports WHAT IT IS A COPY OF: its
//!    `ProvisioningArtifactMetadata` carries the id of the product it was copied from
//!    and the id that version holds in that product, beside the template source the
//!    version was created with. A version nothing copied carries neither key. The
//!    lineage is the copy's IMMEDIATE source, so copying a copy names the product the
//!    request read.
//! 2. A lookup refused because the identifier it was given resolves to nothing answers
//!    the message the real service words for THAT operation and that kind of resource:
//!    fixed text naming the parameter or the resource kind, with the identifier from the
//!    request as the only variable part — never one service-wide sentence, and never one
//!    asserting an access failure the service does not report.
//!
//! Every assertion reads STORED state back through `DescribeProductAsAdmin`,
//! `ListProvisioningArtifacts` and `DescribeProvisionedProduct`, never a value this file
//! hands the service and takes straight back out of the same response.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    associate_product_with_portfolio, call, create_portfolio, create_product, emulator, error_code,
    first_provisioning_artifact_id, message,
};

/// The `ProvisioningArtifactMetadata` keys a copied version carries, from the service's
/// own constants — the two this suite's rule is about.
const SOURCE_PRODUCT_ID: &str =
    service_com_amazonaws_servicecatalog_aws242servicecatalogservice::state::ARTIFACT_METADATA_SOURCE_PRODUCT_ID;
const SOURCE_PROVISIONING_ARTIFACT_ID: &str =
    service_com_amazonaws_servicecatalog_aws242servicecatalogservice::state::ARTIFACT_METADATA_SOURCE_PROVISIONING_ARTIFACT_ID;

/// The sentence no refusal may carry: it asserts an authorization dimension the service
/// does not report for a resource it simply cannot find.
const GENERIC_SENTENCE: &str = "does not exist or access was denied";

/// Every version of `product_id` as the ADMIN view reports it: artifact id ->
/// `ProvisioningArtifactMetadata`.
async fn stored_metadata(
    router: &MultiServiceRouter,
    product_id: &str,
) -> BTreeMap<String, BTreeMap<String, String>> {
    let (status, described) = call(router, "DescribeProductAsAdmin", &json!({"Id": product_id}))
        .await;
    assert_eq!(
        status, 200,
        "DescribeProductAsAdmin({product_id}): expected 200, got {status} with {described}"
    );
    described["ProvisioningArtifactSummaries"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("DescribeProductAsAdmin({product_id}): no summaries in {described}")
        })
        .iter()
        .map(|summary| {
            let id = summary["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("a summary with no Id: {summary}"))
                .to_string();
            let metadata = summary["ProvisioningArtifactMetadata"]
                .as_object()
                .unwrap_or_else(|| {
                    panic!("version {id}: no ProvisioningArtifactMetadata in {summary}")
                })
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value
                            .as_str()
                            .unwrap_or_else(|| panic!("version {id}: {key} is not a string"))
                            .to_string(),
                    )
                })
                .collect();
            (id, metadata)
        })
        .collect()
}

/// The stored ARN of `product_id` — the one identifier a copy addresses its source by.
async fn stored_arn(router: &MultiServiceRouter, product_id: &str) -> String {
    let (status, described) = call(router, "DescribeProductAsAdmin", &json!({"Id": product_id}))
        .await;
    assert_eq!(
        status, 200,
        "DescribeProductAsAdmin({product_id}): expected 200, got {status} with {described}"
    );
    described["ProductViewDetail"]["ProductARN"]
        .as_str()
        .unwrap_or_else(|| panic!("DescribeProductAsAdmin({product_id}): no ARN in {described}"))
        .to_string()
}

/// Copy `source_arn` and answer the `CopyProductToken` and the target product the copy
/// reports, having read the target back out of `DescribeCopyProductStatus`.
async fn copy(
    router: &MultiServiceRouter,
    case: &str,
    request: &Value,
) -> (String, String) {
    let (status, copied) = call(router, "CopyProduct", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CopyProduct expected 200, got {status} with {copied}"
    );
    let token = copied["CopyProductToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no CopyProductToken in {copied}"))
        .to_string();
    let (status, reported) = call(
        router,
        "DescribeCopyProductStatus",
        &json!({"CopyProductToken": token}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeCopyProductStatus expected 200, got {status} with {reported}"
    );
    let target = reported["TargetProductId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no TargetProductId in {reported}"))
        .to_string();
    (token, target)
}

/// A version a copy created reports the product and the version it was copied from; one
/// a create minted reports only its template source.
///
/// The source product carries TWO versions, one of them itself a copy, so each copied
/// version is checked against the source version it really came from and the lineage is
/// shown to name the product this copy read rather than the one further back. Rows
/// differ along the versions the request selects (all by default, one named) and along
/// whether the target is a new product or one that already carries a version of its own.
#[tokio::test]
async fn a_copied_version_reports_the_product_and_version_it_was_copied_from() {
    let router = emulator();

    // The product every row copies FROM. Its second version is a copy of another
    // product's, so this product's own versions carry two different lineages.
    let origin_id = create_product(&router, "lineage-origin", "tok-origin").await;
    let origin_arn = stored_arn(&router, &origin_id).await;
    let source_id = create_product(&router, "lineage-source", "tok-source").await;
    let source_arn = stored_arn(&router, &source_id).await;
    let source_own_version = first_provisioning_artifact_id(&router, &source_id).await;
    copy(
        &router,
        "seed: the source gains a copied version",
        &json!({
            "SourceProductArn": origin_arn,
            "TargetProductId": source_id,
            "IdempotencyToken": "tok-seed-copy",
        }),
    )
    .await;

    let source_versions = stored_metadata(&router, &source_id).await;
    assert_eq!(
        source_versions.len(),
        2,
        "the source must carry its own version and the copied one, got {source_versions:?}"
    );
    // The source's OWN version is a copy of nothing; the one the seed copy added names
    // the product it came from.
    let own = &source_versions[&source_own_version];
    assert!(
        !own.contains_key(SOURCE_PRODUCT_ID) && !own.contains_key(SOURCE_PROVISIONING_ARTIFACT_ID),
        "a created version is a copy of nothing, got {own:?}"
    );
    assert!(
        own.contains_key("LoadTemplateFromURL"),
        "a created version keeps the template source it was created with, got {own:?}"
    );
    let seeded_version = source_versions
        .keys()
        .find(|id| *id != &source_own_version)
        .expect("the copied version")
        .clone();
    assert_eq!(
        source_versions[&seeded_version].get(SOURCE_PRODUCT_ID),
        Some(&origin_id),
        "the seeded copy must name the product it was copied from, got {:?}",
        source_versions[&seeded_version]
    );

    /// One copy whose target's versions are checked against the source's.
    struct Case {
        case: &'static str,
        /// Whether the request names one of the source's versions instead of defaulting
        /// to all of them.
        name_one_version: bool,
        /// Whether the copy goes into a product that already carries a version.
        into_existing: bool,
    }

    let cases = [
        Case {
            case: "every version into a new product",
            name_one_version: false,
            into_existing: false,
        },
        Case {
            case: "one named version into a new product",
            name_one_version: true,
            into_existing: false,
        },
        Case {
            case: "every version into a product that already has one",
            name_one_version: false,
            into_existing: true,
        },
        Case {
            case: "one named version into a product that already has one",
            name_one_version: true,
            into_existing: true,
        },
    ];

    for (index, row) in cases.iter().enumerate() {
        let label = row.case;
        let mut request = json!({
            "SourceProductArn": source_arn,
            "IdempotencyToken": format!("tok-case-{index}"),
        });
        let mut expected_sources: BTreeSet<String> = source_versions.keys().cloned().collect();
        if row.name_one_version {
            request["SourceProvisioningArtifactIdentifiers"] =
                json!([{"Id": seeded_version.clone()}]);
            expected_sources = BTreeSet::from([seeded_version.clone()]);
        }
        let existing: BTreeSet<String> = if row.into_existing {
            let target_id = create_product(
                &router,
                &format!("lineage-target-{index}"),
                &format!("tok-target-{index}"),
            )
            .await;
            request["TargetProductId"] = Value::String(target_id.clone());
            stored_metadata(&router, &target_id).await.into_keys().collect()
        } else {
            BTreeSet::new()
        };

        let (_, target_id) = copy(&router, label, &request).await;
        let target_versions = stored_metadata(&router, &target_id).await;
        assert_eq!(
            target_versions.len(),
            existing.len() + expected_sources.len(),
            "case {label:?}: expected {} version(s), got {target_versions:?}",
            existing.len() + expected_sources.len()
        );

        let mut reported_sources: BTreeSet<String> = BTreeSet::new();
        for (artifact_id, metadata) in &target_versions {
            if existing.contains(artifact_id) {
                // A version the target already carried is untouched by the copy.
                assert!(
                    !metadata.contains_key(SOURCE_PRODUCT_ID),
                    "case {label:?}: the target's own version {artifact_id} must keep no copy \
                     lineage, got {metadata:?}"
                );
                continue;
            }
            assert_eq!(
                metadata.get(SOURCE_PRODUCT_ID),
                Some(&source_id),
                "case {label:?}: version {artifact_id} must name the product it was copied \
                 from, got {metadata:?}"
            );
            let from = metadata
                .get(SOURCE_PROVISIONING_ARTIFACT_ID)
                .unwrap_or_else(|| {
                    panic!(
                        "case {label:?}: version {artifact_id} carries no \
                         {SOURCE_PROVISIONING_ARTIFACT_ID}: {metadata:?}"
                    )
                });
            assert_ne!(
                from, artifact_id,
                "case {label:?}: a copied version is a version of the target, so it must carry \
                 its own id: {metadata:?}"
            );
            assert_eq!(
                metadata.get("LoadTemplateFromURL"),
                source_versions[from].get("LoadTemplateFromURL"),
                "case {label:?}: version {artifact_id} must keep the source version's template \
                 source, got {metadata:?}"
            );
            reported_sources.insert(from.clone());
        }
        assert_eq!(
            reported_sources, expected_sources,
            "case {label:?}: the copied versions must name exactly the source versions the \
             request selected, got {reported_sources:?}"
        );
    }

    // Nothing above changed what the source reports.
    assert_eq!(
        stored_metadata(&router, &source_id).await,
        source_versions,
        "the copies must leave the source's versions as they were"
    );
}

/// A copy of a copy names the product the request read, not the one at the head of the
/// chain: the lineage a version carries is its immediate source.
#[tokio::test]
async fn copying_a_copy_reports_the_product_the_copy_read() {
    let router = emulator();
    let first_id = create_product(&router, "chain-first", "tok-chain-first").await;
    let first_arn = stored_arn(&router, &first_id).await;

    let (_, second_id) = copy(
        &router,
        "the first copy",
        &json!({"SourceProductArn": first_arn, "IdempotencyToken": "tok-chain-1"}),
    )
    .await;
    let second_versions = stored_metadata(&router, &second_id).await;
    let second_arn = stored_arn(&router, &second_id).await;

    let (_, third_id) = copy(
        &router,
        "a copy of the copy",
        &json!({"SourceProductArn": second_arn, "IdempotencyToken": "tok-chain-2"}),
    )
    .await;
    let third_versions = stored_metadata(&router, &third_id).await;

    for (artifact_id, metadata) in &third_versions {
        assert_eq!(
            metadata.get(SOURCE_PRODUCT_ID),
            Some(&second_id),
            "version {artifact_id}: the lineage names the product this copy read, got \
             {metadata:?}"
        );
        assert!(
            metadata
                .get(SOURCE_PROVISIONING_ARTIFACT_ID)
                .is_some_and(|from| second_versions.contains_key(from)),
            "version {artifact_id}: the lineage names a version of that product, got \
             {metadata:?}"
        );
    }
    // The middle product still names the one IT was copied from.
    for (artifact_id, metadata) in &second_versions {
        assert_eq!(
            metadata.get(SOURCE_PRODUCT_ID),
            Some(&first_id),
            "version {artifact_id}: the first copy's lineage must be unchanged, got {metadata:?}"
        );
    }
}

/// `DescribeCopyProductStatus` refuses a `CopyProductToken` that names no copy job with
/// the message naming that parameter and echoing the token.
///
/// Rows span the token shapes the modelled `Id` admits (`len 1..100`, pattern
/// `^[a-zA-Z0-9_\-]*$`, `smithy_query query="operation"
/// operation="DescribeCopyProductStatus"`): the minimum and the maximum length, the
/// prefixed form a copy answers, and one carrying each admitted punctuation.
#[tokio::test]
async fn describe_copy_product_status_names_the_copy_product_token_it_cannot_resolve() {
    let router = emulator();
    // One copy really exists, so the refusals below are about the token rather than
    // about an empty store.
    let product_id = create_product(&router, "token-source", "tok-token-source").await;
    let product_arn = stored_arn(&router, &product_id).await;
    let (real_token, _) = copy(
        &router,
        "a copy that exists",
        &json!({"SourceProductArn": product_arn, "IdempotencyToken": "tok-token-copy"}),
    )
    .await;

    let longest = format!("copy-{}", "a".repeat(95));
    let unknown: [(&str, String); 5] = [
        ("the minimum length", "z".to_string()),
        ("the prefixed form a copy answers", "copy-zzzzzzzzzzzzz".to_string()),
        ("underscores and dashes", "copy_product-9_X".to_string()),
        ("the maximum length", longest.clone()),
        ("a token of another resource kind", "prod-abcdefghijkl".to_string()),
    ];

    for (case, token) in unknown {
        assert!(
            token.len() <= 100 && !token.is_empty(),
            "case {case:?}: the token must satisfy the modelled length, got {} characters",
            token.len()
        );
        let (status, refused) = call(
            &router,
            "DescribeCopyProductStatus",
            &json!({"CopyProductToken": token}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error in {refused}"
        );
        let reported = message(&refused);
        assert_eq!(
            reported,
            format!("Invalid CopyProductToken {token}"),
            "case {case:?}: unexpected message, got {reported:?}"
        );
        assert!(
            !reported.contains(GENERIC_SENTENCE),
            "case {case:?}: the refusal must not assert an access failure, got {reported:?}"
        );
    }

    // The token a copy really answered still resolves after all of that.
    let (status, reported) = call(
        &router,
        "DescribeCopyProductStatus",
        &json!({"CopyProductToken": real_token}),
    )
    .await;
    assert_eq!(
        status, 200,
        "the token a copy answered must resolve, got {status} with {reported}"
    );
}

/// `DescribeProvisionedProduct` refuses an `Id` or a `Name` that names no provisioned
/// product with the message naming that kind of resource and echoing the identifier.
///
/// Rows span both members and the identifier shapes each one's model admits (`Id`
/// `len 1..100` pattern `^[a-zA-Z0-9_\-]*$`, `Name` `len 1..128` pattern
/// `^[a-zA-Z0-9][a-zA-Z0-9._-]*$`, `smithy_query query="operation"
/// operation="DescribeProvisionedProduct"`): each member's minimum and maximum length,
/// the prefixed id form the service mints, and a name that only looks like a stored one.
#[tokio::test]
async fn describe_provisioned_product_names_the_provisioned_product_it_cannot_resolve() {
    let router = emulator();
    // One provisioned product really exists, so the refusals below are about the
    // identifier rather than about an empty store.
    let portfolio_id = create_portfolio(&router, "pp-portfolio", "acme", &[], "tok-pp-port").await;
    let product_id = create_product(&router, "pp-product", "tok-pp-product").await;
    let artifact_id = first_provisioning_artifact_id(&router, &product_id).await;
    associate_product_with_portfolio(&router, &product_id, &portfolio_id).await;
    let stored_name = "stored-stack";
    let (status, provisioned) = call(
        &router,
        "ProvisionProduct",
        &json!({
            "ProductId": product_id,
            "ProvisioningArtifactId": artifact_id,
            "ProvisionedProductName": stored_name,
            "ProvisionToken": "tok-provision",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "ProvisionProduct: expected 200, got {status} with {provisioned}"
    );

    let longest_id = format!("pp-{}", "b".repeat(97));
    let longest_name = format!("s{}", "x".repeat(127));
    let unknown: [(&str, &str, String); 7] = [
        ("Id", "the minimum length", "q".to_string()),
        (
            "Id",
            "the prefixed form the service mints",
            "pp-zzzzzzzzzzzzz".to_string(),
        ),
        ("Id", "underscores and dashes", "pp_stack-7".to_string()),
        ("Id", "the maximum length", longest_id.clone()),
        ("Name", "the minimum length", "q".to_string()),
        (
            "Name",
            "a name only like a stored one",
            format!("{stored_name}-2"),
        ),
        ("Name", "the maximum length", longest_name.clone()),
    ];

    for (member, shape, identifier) in unknown {
        let case = format!("{member}: {shape}");
        let (status, refused) = call(
            &router,
            "DescribeProvisionedProduct",
            &json!({member: identifier}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error in {refused}"
        );
        let reported = message(&refused);
        assert_eq!(
            reported,
            format!("Provisioned product not found: {identifier}"),
            "case {case:?}: unexpected message, got {reported:?}"
        );
        assert!(
            !reported.contains(GENERIC_SENTENCE),
            "case {case:?}: the refusal must not assert an access failure, got {reported:?}"
        );
    }

    // The provisioned product that really exists still resolves, by either identifier,
    // and both reads report the same stored record.
    let (status, by_name) = call(
        &router,
        "DescribeProvisionedProduct",
        &json!({"Name": stored_name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "the stored provisioned product must resolve by Name, got {status} with {by_name}"
    );
    let stored_id = by_name["ProvisionedProductDetail"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("no Id in {by_name}"))
        .to_string();
    let (status, by_id) = call(
        &router,
        "DescribeProvisionedProduct",
        &json!({"Id": stored_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "the stored provisioned product must resolve by Id, got {status} with {by_id}"
    );
    assert_eq!(
        by_id["ProvisionedProductDetail"], by_name["ProvisionedProductDetail"],
        "both identifiers must report the same stored record: {by_name} then {by_id}"
    );
}

/// Every ADMIN-side product lookup refuses an identifier it cannot resolve by naming
/// the kind of resource and the identifier the request used, whichever operation and
/// whichever member the request addressed the product by — and none of them claims an
/// access failure.
///
/// The END-USER view, `DescribeProduct`, is not one of these: it reaches a product only
/// through a launch path, so the service words its refusal as one sentence covering both
/// a product that does not exist and one the caller cannot see. That sentence is pinned
/// in `end_user_product_view_messages`.
#[tokio::test]
async fn product_lookups_name_the_product_they_cannot_resolve() {
    let router = emulator();
    // A product that exists, so each refusal below is about the identifier it was given.
    let product_id = create_product(&router, "lookup-product", "tok-lookup").await;
    let product_arn = stored_arn(&router, &product_id).await;

    let unknown_id = "prod-zzzzzzzzzzzz";
    let unknown_arn = format!(
        "{}-missing",
        product_arn.trim_end_matches(product_id.as_str())
    );
    // Rows differ along the operation, the member the product is addressed by, and the
    // form of the identifier (a product id, a product ARN).
    let cases: [(&str, &str, Value, String); 4] = [
        (
            "UpdateProduct by Id",
            "UpdateProduct",
            json!({"Id": unknown_id, "Name": "renamed"}),
            unknown_id.to_string(),
        ),
        (
            "DeleteProduct by Id",
            "DeleteProduct",
            json!({"Id": unknown_id}),
            unknown_id.to_string(),
        ),
        (
            "CopyProduct by SourceProductArn",
            "CopyProduct",
            json!({"SourceProductArn": unknown_arn.clone(), "IdempotencyToken": "tok-bad-source"}),
            unknown_arn.clone(),
        ),
        (
            "CopyProduct by TargetProductId",
            "CopyProduct",
            json!({
                "SourceProductArn": product_arn,
                "TargetProductId": unknown_id,
                "IdempotencyToken": "tok-bad-target",
            }),
            unknown_id.to_string(),
        ),
    ];

    for (case, operation, request, identifier) in cases {
        let (status, refused) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error in {refused}"
        );
        let reported = message(&refused);
        assert_eq!(
            reported,
            format!("Product {identifier} not found."),
            "case {case:?}: unexpected message, got {reported:?}"
        );
        assert!(
            !reported.contains(GENERIC_SENTENCE),
            "case {case:?}: the refusal must not assert an access failure, got {reported:?}"
        );
    }

    // None of the refused calls changed the product that does exist.
    let (status, described) = call(&router, "DescribeProductAsAdmin", &json!({"Id": product_id}))
        .await;
    assert_eq!(
        status, 200,
        "the stored product must still be readable, got {status} with {described}"
    );
}
