// SPDX-License-Identifier: CC0-1.0

//! What `UpdateAccountCustomizations` accepts, and what the account's record
//! reads back as afterwards.
//!
//! Two halves of one rule, both driven over the real wire path through the shared
//! harness in [`crate::wire`]:
//!
//! 1. ADMISSION — a `visibleServices` member has to satisfy the modelled
//!    constraints AND name an entry of the catalog `ListServices` enumerates
//!    ("For valid service identifiers, call ListServices"), while a
//!    `visibleRegions` member has only the modelled `Region` `@pattern` to
//!    satisfy, because this service publishes no catalog of Region codes. The
//!    refusal of a non-member is `visible_service_catalog_membership`'s subject;
//!    here every service identifier is drawn from the catalog.
//! 2. ROUND TRIP — the members of a list setting are the caller's, their ORDER is
//!    the service's. The update stores and answers with those members in the one
//!    canonical sequence (ascending — `crate::support::reported`), a later
//!    `GetAccountCustomizations` reports that same sequence, and an update that
//!    omits the setting leaves it alone ("Omitted settings remain unchanged").
//!
//! The round-trip rows are GENERATED rather than written out — the service
//! identifiers drawn from the catalog the service itself publishes
//! ([`crate::wire::catalog_services`]), the Region codes from the model's own
//! `@pattern` by [`crate::wire::legal_region`] — so the rule is asserted over the
//! class of settable values and not over the identifiers any one recording used;
//! every generated Region code is checked against a hand-written transcription of
//! its pattern ([`crate::wire::is_legal_region`]) before it is sent, so the
//! generator cannot drift outside the constraint it claims to cover. Both are
//! deterministic (no seed, nothing random), so a failing case names itself.

use crate::support::{reported, reported_owned};
use crate::wire::{
    build_router, catalog_services, get_customizations, is_legal_region, is_legal_service,
    legal_region, update_customizations,
};
use serde_json::{json, Value};

/// One row of the round-trip table: the sequences to supply for each list
/// setting, and a name for the failure message.
struct Row {
    name: String,
    services: Vec<String>,
    regions: Vec<String>,
}

/// The table the rule is asserted over: values arranged into rows that differ
/// along list SIZE (0, 1, 2, 3, 5, 8), supplied SEQUENCE (drawn order, reversed,
/// rotated), element FORM (the catalog's own span of segment counts and lengths
/// for services, the Region generator's for Regions) and DUPLICATION (a member
/// named twice, and one named three times) — which is what the rule generalises
/// over.
async fn round_trip_rows() -> Vec<Row> {
    let services: Vec<String> = catalog_services(&build_router(), 24).await;
    let regions: Vec<String> = (0..24).map(legal_region).collect();
    let take = |values: &[String], from: usize, count: usize| -> Vec<String> {
        (0..count).map(|n| values[(from + n) % values.len()].clone()).collect()
    };
    let reversed = |values: Vec<String>| -> Vec<String> {
        let mut rotated = values;
        rotated.reverse();
        rotated
    };
    let rotated = |values: Vec<String>| -> Vec<String> {
        let mut turned = values;
        turned.rotate_left(1);
        turned
    };

    let mut rows = Vec::new();
    for (index, size) in [0_usize, 1, 2, 3, 5, 8].into_iter().enumerate() {
        let drawn_services = take(&services, index * 3, size);
        let drawn_regions = take(&regions, index * 2, size);
        rows.push(Row {
            name: format!("{size} members as generated"),
            services: drawn_services.clone(),
            regions: drawn_regions.clone(),
        });
        rows.push(Row {
            name: format!("{size} members reversed"),
            services: reversed(drawn_services.clone()),
            regions: reversed(drawn_regions.clone()),
        });
        if size > 1 {
            rows.push(Row {
                name: format!("{size} members rotated"),
                services: rotated(drawn_services),
                regions: rotated(drawn_regions),
            });
        }
    }
    // Duplicates: neither list declares `@uniqueItems`, so a member named twice
    // stays twice, where it was named.
    rows.push(Row {
        name: "a member named twice around another".to_string(),
        services: vec![services[4].clone(), services[9].clone(), services[4].clone()],
        regions: vec![regions[3].clone(), regions[7].clone(), regions[3].clone()],
    });
    rows.push(Row {
        name: "one member named three times".to_string(),
        services: vec![services[11].clone(); 3],
        regions: vec![regions[11].clone(); 3],
    });
    rows
}

/// The rule over the table: any list of settable members is accepted with HTTP
/// 200, answered back with exactly its members in the canonical sequence,
/// reported again by `GetAccountCustomizations`, and preserved across an update
/// that names only the colour.
#[tokio::test]
async fn a_supplied_list_setting_round_trips_whatever_its_members() {
    for row in round_trip_rows().await {
        for service in &row.services {
            assert!(
                is_legal_service(service),
                "case {:?}: the catalog reported {service:?}, which is outside Service's @pattern/@length",
                row.name
            );
        }
        for region in &row.regions {
            assert!(
                is_legal_region(region),
                "case {:?}: the generator produced {region:?}, which is outside Region's @pattern",
                row.name
            );
        }

        let router = build_router();
        let expected_services = reported_owned(&row.services);
        let expected_regions = reported_owned(&row.regions);

        let (status, error_type, updated) = update_customizations(
            &router,
            json!({"visibleServices": row.services, "visibleRegions": row.regions}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status} [{error_type}] ({updated})",
            row.name
        );
        assert_eq!(
            updated.get("visibleServices"),
            Some(&expected_services),
            "case {:?}: expected visibleServices {expected_services}, got {updated:?}",
            row.name
        );
        assert_eq!(
            updated.get("visibleRegions"),
            Some(&expected_regions),
            "case {:?}: expected visibleRegions {expected_regions}, got {updated:?}",
            row.name
        );

        let (status, _, fetched) = get_customizations(&router).await;
        assert_eq!(
            status, 200,
            "case {:?}: Get expected 200, got {status} ({fetched})",
            row.name
        );
        assert_eq!(
            fetched,
            json!({
                "accountColor": "none",
                "visibleServices": expected_services,
                "visibleRegions": expected_regions,
            }),
            "case {:?}: Get must report what Update stored, got {fetched:?}",
            row.name
        );

        // "Omitted settings remain unchanged": a colour-only update keeps both
        // lists, canonical sequence included.
        let (_, _, partial) =
            update_customizations(&router, json!({"accountColor": "orange"})).await;
        assert_eq!(
            partial,
            json!({
                "accountColor": "orange",
                "visibleServices": expected_services,
                "visibleRegions": expected_regions,
            }),
            "case {:?}: a partial update must preserve both lists, got {partial:?}",
            row.name
        );
    }
}

/// Order is the service's, not the request's: for every row of at least two
/// members, supplying it and then supplying it reversed names the SAME setting —
/// both answers, and the record afterwards, are the one canonical sequence.
#[tokio::test]
async fn supplying_a_list_reversed_is_answered_with_the_same_canonical_sequence() {
    let rows = round_trip_rows().await;
    for row in rows.into_iter().filter(|row| row.services.len() > 1) {
        let reversed: Vec<String> = row.services.iter().rev().cloned().collect();
        let expected = reported_owned(&row.services);
        let router = build_router();

        let (_, _, forward) =
            update_customizations(&router, json!({"visibleServices": row.services.clone()})).await;
        assert_eq!(
            forward.get("visibleServices"),
            Some(&expected),
            "case {:?}: expected the canonical {expected}, got {forward:?}",
            row.name
        );

        let (_, _, backward) =
            update_customizations(&router, json!({"visibleServices": reversed.clone()})).await;
        assert_eq!(
            backward.get("visibleServices"),
            Some(&expected),
            "case {:?} reversed: expected the same canonical {expected}, got {backward:?}",
            row.name
        );

        let (_, _, fetched) = get_customizations(&router).await;
        assert_eq!(
            fetched.get("visibleServices"),
            Some(&expected),
            "case {:?}: Get must report the canonical {expected}, got {fetched:?}",
            row.name
        );
    }
}

/// The configure workflow end to end, in the shape a console client performs it:
/// set the two settings, read them back, change ONE of them, and read again — the
/// setting the second request omitted is still there, in the canonical sequence it
/// was stored in.
#[tokio::test]
async fn a_configure_then_partial_update_workflow_preserves_the_omitted_setting() {
    let router = build_router();

    // Step 1: configure both lists, the Regions in a non-canonical sequence.
    let services = ["account-access", "acm"];
    let regions = ["us-west-2", "eu-west-1"];
    let expected_services = reported(&services);
    let expected_regions = reported(&regions);
    let (status, _, configured) = update_customizations(
        &router,
        json!({"visibleServices": services, "visibleRegions": regions}),
    )
    .await;
    assert_eq!(
        status, 200,
        "configuring both lists must succeed, got {status} ({configured})"
    );
    assert_eq!(
        configured,
        json!({
            "accountColor": "none",
            "visibleServices": expected_services,
            "visibleRegions": expected_regions,
        }),
        "the update must answer the record it stored, got {configured:?}"
    );

    // Step 2: read it back.
    let (_, _, fetched) = get_customizations(&router).await;
    assert_eq!(
        fetched, configured,
        "Get must report exactly what Update stored, got {fetched:?}"
    );

    // Step 3: a partial update that names only the colour and one list.
    let narrowed = ["account-access"];
    let (status, _, partial) = update_customizations(
        &router,
        json!({"accountColor": "teal", "visibleServices": narrowed}),
    )
    .await;
    assert_eq!(
        status, 200,
        "the partial update must succeed, got {status} ({partial})"
    );
    assert_eq!(
        partial,
        json!({
            "accountColor": "teal",
            "visibleServices": reported(&narrowed),
            "visibleRegions": expected_regions,
        }),
        "the omitted visibleRegions must survive the partial update, got {partial:?}"
    );

    // Step 4: and the read after it agrees.
    let (_, _, after) = get_customizations(&router).await;
    assert_eq!(
        after, partial,
        "Get after a partial update must report the merged record, got {after:?}"
    );
    assert_eq!(
        after.get("visibleRegions"),
        Some(&expected_regions),
        "the preserved list must keep its canonical sequence, got {after:?}"
    );
    assert!(
        after.get("visibleServices").and_then(Value::as_array).is_some(),
        "a configured list must never read back as null, got {after:?}"
    );
}
