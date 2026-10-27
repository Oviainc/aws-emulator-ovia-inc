// SPDX-License-Identifier: CC0-1.0

//! `CreateInvalidationForDistributionTenant`, end to end over HTTP, against the agreement
//! rule every CloudFront list wrapper is held to.
//!
//! The rule under test: in the `InvalidationBatch` a create submits, the `Paths` wrapper's
//! stated `Quantity` must equal the number of `Path` items it carries — a disagreement in
//! EITHER direction is refused with `InconsistentQuantities` (HTTP 400), carrying the
//! sentence that names the wrapper, the quantity the request stated and the number it
//! really carried, and nothing is persisted for it; an agreeing batch is accepted (201),
//! stored whole and reported by a later read of the same tenant.
//!
//! The rows differ along what the rule generalises over: which side is larger, by how much,
//! and whether the `Items` the wrapper carries is empty or not — and, for the accepted
//! rows, how many paths the batch names and in which form (a wildcard, one object, several
//! objects, a deep path). Because a refusal stores nothing, a refused `CallerReference` is
//! still free: each refused row's reference is submitted again on an agreeing batch and
//! creates an invalidation.
//!
//! The harness (the router, the request helpers, the tenant creation and the element
//! scanner) is the distribution and tenant suites', reused rather than copied.

use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{
    batch_xml, batch_xml_with_quantity, emulator, inconsistent_quantities_message,
    is_invalidation_id, Node, API_VERSION,
};
use crate::tenant_invalidations::{
    batch_paths, create_invalidation_for_tenant, distribution_with_tenant,
    get_invalidation_for_tenant, list_invalidations_for_tenant, summaries,
};

/// How many invalidations a tenant's listing reports.
async fn invalidation_count(router: &MultiServiceRouter, tenant_id: &str) -> usize {
    let list = list_invalidations_for_tenant(router, tenant_id, "").await;
    assert_eq!(
        list.status, 200,
        "listing {tenant_id}: expected HTTP 200, got {}",
        list.raw
    );
    summaries(list.root()).len()
}

/// Rows whose `Paths.Quantity` disagrees with the `Path` items beside it: more stated than
/// carried, fewer stated than carried, none stated against some carried, and some stated
/// against an empty `Items`.
fn disagreeing_rows() -> Vec<(&'static str, Vec<&'static str>, i32)> {
    vec![
        ("states-one-more-than-it-carries", vec!["/index.html"], 2),
        ("states-two-fewer-than-it-carries", vec!["/a.png", "/b.png", "/c.png"], 1),
        ("states-none-but-carries-one", vec!["/*"], 0),
        ("states-four-but-carries-none", vec![], 4),
        (
            "states-far-more-than-it-carries",
            vec!["/css/site.css", "/js/app.js"],
            99,
        ),
    ]
}

/// Rows whose `Paths.Quantity` agrees with its items, differing in how many paths the batch
/// names and in which form.
fn agreeing_rows() -> Vec<(&'static str, Vec<&'static str>)> {
    vec![
        ("one-wildcard", vec!["/*"]),
        ("one-object", vec!["/index.html"]),
        ("two-objects", vec!["/css/site.css", "/js/app.js"]),
        (
            "four-objects-and-a-deep-path",
            vec![
                "/images/a.png",
                "/images/b.png",
                "/images/c.png",
                "/deep/nested/path/to/object.json",
            ],
        ),
    ]
}

/// A batch whose `Paths.Quantity` disagrees with its `Items` is refused with
/// `InconsistentQuantities` and persists nothing — so the listing after it is the listing
/// before it, and the `CallerReference` it carried is still free.
#[tokio::test]
async fn a_tenant_batch_whose_quantity_disagrees_with_its_items_is_refused() {
    let router = emulator();
    let tenant_id = distribution_with_tenant(&router, "batch-quantity-refused").await;

    for (case, paths, stated) in disagreeing_rows() {
        let before = invalidation_count(&router, &tenant_id).await;
        let reference = format!("quantity-{case}");
        let refused = create_invalidation_for_tenant(
            &router,
            &tenant_id,
            &batch_xml_with_quantity(&reference, &paths, stated),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected HTTP 400, got {} with {}",
            refused.status, refused.raw
        );
        assert_eq!(
            refused.error_code(),
            "InconsistentQuantities",
            "case {case}: expected InconsistentQuantities, got {:?} with {}",
            refused.error_code(),
            refused.raw
        );
        let message = refused.root().find_text("Message").unwrap_or_default();
        assert_eq!(
            message,
            // `Paths` is the wrapper an `InvalidationBatch` carries.
            inconsistent_quantities_message("Paths", stated, paths.len()),
            "case {case}: expected the refusal to name the wrapper and both counts, got \
             {message:?}"
        );

        // Nothing was stored for a refused batch.
        let after = invalidation_count(&router, &tenant_id).await;
        assert_eq!(
            after, before,
            "case {case}: expected the refusal to persist nothing, got {after} invalidation(s) \
             where there were {before}"
        );

        // And the reference it carried was never taken, so the same reference on an
        // agreeing batch creates an invalidation. A batch is submitted to invalidate
        // something, so a row that carried no path at all retries with one of its own
        // rather than with an empty batch.
        let retry: Vec<&str> = if paths.is_empty() {
            vec!["/retried.html"]
        } else {
            paths.clone()
        };
        let accepted =
            create_invalidation_for_tenant(&router, &tenant_id, &batch_xml(&reference, &retry))
                .await;
        assert_eq!(
            accepted.status, 201,
            "case {case}: expected the agreeing batch under the same CallerReference to be \
             accepted, got {} with {}",
            accepted.status, accepted.raw
        );
        let stored_count = invalidation_count(&router, &tenant_id).await;
        assert_eq!(
            stored_count,
            before + 1,
            "case {case}: expected exactly one more invalidation stored, got {stored_count} \
             where there were {before}"
        );
    }
}

/// An agreeing batch is accepted, stored whole under a minted id the `Location` addresses,
/// and reported by a later read of the same tenant.
#[tokio::test]
async fn a_tenant_batch_whose_quantity_agrees_is_accepted_and_stored_whole() {
    let router = emulator();
    let tenant_id = distribution_with_tenant(&router, "batch-quantity-accepted").await;

    for (index, (case, paths)) in agreeing_rows().into_iter().enumerate() {
        let reference = format!("quantity-ok-{case}");
        let created = create_invalidation_for_tenant(
            &router,
            &tenant_id,
            &batch_xml_with_quantity(&reference, &paths, paths.len() as i32),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            created.status, created.raw
        );
        let answer = created.root();
        let id = answer.text_at("Id").unwrap_or_default().to_string();
        assert!(
            is_invalidation_id(&id),
            "case {case}: expected an `I`+13 invalidation id, got {id:?}"
        );
        let location = created.header("location");
        assert!(
            location.ends_with(&format!(
                "/{API_VERSION}/distribution-tenant/{tenant_id}/invalidation/{id}"
            )),
            "case {case}: expected the Location to address the new invalidation, got \
             {location:?}"
        );

        // The record a read answers from is the batch as submitted: the paths in the order
        // they were given, and the quantity the wrapper stated.
        let read = get_invalidation_for_tenant(&router, &tenant_id, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected HTTP 200 reading {id}, got {} with {}",
            read.status, read.raw
        );
        let stored: &Node = read.root();
        let expected: Vec<String> = paths.iter().map(|path| (*path).to_string()).collect();
        assert_eq!(
            batch_paths(stored),
            expected,
            "case {case}: expected the read to report the submitted paths, got {:?}",
            batch_paths(stored)
        );
        assert_eq!(
            stored.text_at("InvalidationBatch/Paths/Quantity"),
            Some(paths.len().to_string().as_str()),
            "case {case}: expected Paths.Quantity {}, got {:?}",
            paths.len(),
            stored.text_at("InvalidationBatch/Paths/Quantity")
        );
        assert_eq!(
            stored.text_at("InvalidationBatch/CallerReference"),
            Some(reference.as_str()),
            "case {case}: expected the submitted CallerReference, got {:?}",
            stored.text_at("InvalidationBatch/CallerReference")
        );

        // Every accepted row is held, so the tenant's listing grows by exactly one per row.
        let count = invalidation_count(&router, &tenant_id).await;
        assert_eq!(
            count,
            index + 1,
            "case {case}: expected {} invalidation(s) stored, got {count}",
            index + 1
        );
    }
}
