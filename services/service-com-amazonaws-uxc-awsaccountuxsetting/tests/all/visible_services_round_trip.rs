// SPDX-License-Identifier: CC0-1.0

//! What `UpdateAccountCustomizations` accepts, and what the account's record
//! reads back as afterwards.
//!
//! Two halves of one rule, both driven over the real wire path through the shared
//! harness in [`crate::wire`]:
//!
//! 1. ADMISSION — the modelled constraints are the whole rule. Every
//!    `visibleServices` / `visibleRegions` value those constraints admit is
//!    accepted, whether or not the catalog `ListServices` pages over happens to
//!    name it: that operation's doc ("For valid service identifiers, call
//!    ListServices") says where identifiers can be DISCOVERED, and neither it nor
//!    the model (`smithy_query(query="operation",
//!    operation="UpdateAccountCustomizations")`) states a refusal for one the
//!    catalog omits.
//! 2. ROUND TRIP — a list setting's value is the caller's document. The update
//!    stores and answers with the exact sequence the request supplied, a later
//!    `GetAccountCustomizations` reports that same sequence, and an update that
//!    omits the setting leaves it alone ("Omitted settings remain unchanged").
//!
//! The round-trip rows are GENERATED from the model's own `@pattern`s by the
//! shared harness's [`crate::wire::legal_service`] / [`crate::wire::legal_region`]
//! rather than written out, so the rule is asserted over the class of legal
//! values and not over the identifiers any one recording used; every generated
//! value is checked against a hand-written transcription of its pattern
//! ([`crate::wire::is_legal_service`] / [`crate::wire::is_legal_region`]) before
//! it is sent, so the generator cannot drift outside the constraint it claims to
//! cover. The generators are deterministic (no seed, nothing random), so a
//! failing case names itself.

use crate::wire::{
    build_router, generated_service_id, get_customizations, is_legal_region, is_legal_service,
    legal_region, legal_service, strings, update_customizations, whole_catalog,
};
use serde_json::{json, Value};

/// One row of the round-trip table: the sequences to supply for each list
/// setting, and a name for the failure message.
struct Row {
    name: String,
    services: Vec<String>,
    regions: Vec<String>,
}

/// The table the rule is asserted over: generated values arranged into rows that
/// differ along list SIZE (0, 1, 2, 3, 5, 8), supplied SEQUENCE (generated order,
/// reversed, rotated), element FORM (segment count, segment length and character
/// class, all walked by the generators) and DUPLICATION (a member named twice,
/// and one named three times) — which is what the rule generalises over.
fn round_trip_rows() -> Vec<Row> {
    let services: Vec<String> = (0..24).map(legal_service).collect();
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

/// The rule over the generated table: any list the modelled constraints admit is
/// accepted with HTTP 200, answered back as the EXACT sequence supplied, reported
/// again by `GetAccountCustomizations`, and preserved across an update that names
/// only the colour.
#[tokio::test]
async fn a_supplied_list_setting_round_trips_whatever_its_members() {
    for row in round_trip_rows() {
        for service in &row.services {
            assert!(
                is_legal_service(service),
                "case {:?}: the generator produced {service:?}, which is outside Service's @pattern/@length",
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
        let expected_services = json!(row.services);
        let expected_regions = json!(row.regions);

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
        // lists, sequence included.
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

/// Order is carried, not derived: for every row of at least two members,
/// supplying it and then supplying it reversed yields two different documents,
/// each answered as sent, and the record ends on the second one.
#[tokio::test]
async fn supplying_a_list_reversed_is_answered_reversed() {
    for row in round_trip_rows().into_iter().filter(|row| row.services.len() > 1) {
        let reversed: Vec<String> = row.services.iter().rev().cloned().collect();
        let router = build_router();

        let (_, _, forward) =
            update_customizations(&router, json!({"visibleServices": row.services.clone()})).await;
        assert_eq!(
            forward.get("visibleServices"),
            Some(&json!(row.services)),
            "case {:?}: the forward sequence must be answered as sent, got {forward:?}",
            row.name
        );

        let (_, _, backward) =
            update_customizations(&router, json!({"visibleServices": reversed.clone()})).await;
        assert_eq!(
            backward.get("visibleServices"),
            Some(&json!(reversed)),
            "case {:?}: the reversed sequence must be answered as sent, got {backward:?}",
            row.name
        );

        let (_, _, fetched) = get_customizations(&router).await;
        assert_eq!(
            fetched.get("visibleServices"),
            Some(&json!(reversed)),
            "case {:?}: Get must report the last document written, got {fetched:?}",
            row.name
        );
    }
}

/// Admission does not consult the catalog: an identifier legal under the member's
/// constraints is accepted and read back whether or not `ListServices` names it.
///
/// Rows differ along the identifier's form (generated, 64-character, invented
/// hyphenated, digit-bearing, real-but-omitted), its position in the list, how
/// many of the list's members are omitted, and whether the row mixes catalog
/// entries with others. The `outside` identifiers are asserted absent from
/// `ListServices` first, so each row really does exercise the rule.
#[tokio::test]
async fn an_identifier_the_catalog_omits_is_accepted_and_read_back() {
    let catalog = whole_catalog(&build_router()).await;
    let generated = generated_service_id(7);
    let longest = "a".repeat(64);
    // Identifiers no service catalog names, so this stays true however the
    // catalog's own contents change.
    for outside in [generated.as_str(), longest.as_str(), "zzqq-no-such-service"] {
        assert!(
            !catalog.contains(&outside.to_string()),
            "case {outside:?}: this row needs an identifier the catalog does NOT hold, got {catalog:?}"
        );
    }
    let in_catalog = catalog
        .first()
        .expect("the catalog must hold at least one entry")
        .clone();

    // (case, the supplied sequence)
    let rows: Vec<(&str, Vec<&str>)> = vec![
        ("the recorded pair of console identifiers", vec!["account-access", "acm"]),
        ("a real service identifier, hyphenated", vec!["s3-outposts", "ec2"]),
        ("a generated identifier", vec![generated.as_str()]),
        ("a 64-character identifier", vec![longest.as_str()]),
        ("an invented hyphenated identifier", vec!["zzqq-no-such-service"]),
        ("a digit-bearing identifier", vec!["route53domains", "s3control"]),
        (
            "an omitted identifier after a catalog entry",
            vec![in_catalog.as_str(), "zzqq-no-such-service"],
        ),
        (
            "an omitted identifier before a catalog entry",
            vec!["zzqq-no-such-service", in_catalog.as_str()],
        ),
        (
            "several omitted identifiers around a catalog entry",
            vec![generated.as_str(), in_catalog.as_str(), longest.as_str()],
        ),
    ];

    for (case, row) in rows {
        let router = build_router();
        let expected = strings(&row);

        let (status, error_type, updated) =
            update_customizations(&router, json!({"visibleServices": strings(&row)})).await;
        assert_eq!(
            status, 200,
            "case {case:?} {row:?}: expected 200, got {status} [{error_type}] ({updated})"
        );
        assert_eq!(
            updated.get("visibleServices"),
            Some(&expected),
            "case {case:?} {row:?}: expected {expected}, got {updated:?}"
        );

        let (_, _, fetched) = get_customizations(&router).await;
        assert_eq!(
            fetched.get("visibleServices"),
            Some(&expected),
            "case {case:?} {row:?}: Get must report what Update stored, got {fetched:?}"
        );
    }
}

/// The same rule walked over the whole catalog: every entry `ListServices`
/// reports is accepted, and so is every entry with one legal character appended —
/// which is inside the member's `@pattern` and `@length` but outside the catalog.
#[tokio::test]
async fn neither_a_catalog_entry_nor_a_near_miss_of_one_is_refused() {
    let router = build_router();
    let catalog = whole_catalog(&router).await;
    assert!(
        !catalog.is_empty(),
        "the catalog must hold entries for this test to say anything"
    );

    // The whole catalog in one request, in the sequence it was reported.
    let (status, _, body) =
        update_customizations(&router, json!({"visibleServices": catalog.clone()})).await;
    assert_eq!(
        status, 200,
        "the whole catalog must be accepted, got {status} ({body})"
    );
    assert_eq!(
        body.get("visibleServices"),
        Some(&json!(catalog.clone())),
        "the whole catalog must be read back in the sequence it was sent, got {body:?}"
    );

    // Every near miss in ONE request, then each on its own: the first says the
    // class is accepted, the second pins each identifier.
    let near_misses: Vec<String> = catalog.iter().map(|entry| format!("{entry}0")).collect();
    let (status, _, body) =
        update_customizations(&router, json!({"visibleServices": near_misses.clone()})).await;
    assert_eq!(
        status, 200,
        "a list of near-miss identifiers must be accepted, got {status} ({body})"
    );
    assert_eq!(
        body.get("visibleServices"),
        Some(&json!(near_misses.clone())),
        "the near-miss identifiers must be read back as sent, got {body:?}"
    );

    for near_miss in &near_misses {
        let router = build_router();
        let (status, error_type, body) =
            update_customizations(&router, json!({"visibleServices": [near_miss]})).await;
        assert_eq!(
            status, 200,
            "case {near_miss:?}: a legal identifier must be accepted, got {status} [{error_type}] ({body})"
        );
        assert_eq!(
            body.get("visibleServices"),
            Some(&json!([near_miss])),
            "case {near_miss:?}: the accepted identifier must be read back, got {body:?}"
        );
    }
}

/// The configure workflow end to end, in the shape a console client performs it:
/// set the two settings, read them back, change ONE of them, and read again — the
/// setting the second request omitted is still there, in the sequence it was
/// written.
#[tokio::test]
async fn a_configure_then_partial_update_workflow_preserves_the_omitted_setting() {
    let router = build_router();

    // Step 1: configure both lists, each in a non-sorted sequence.
    let services = ["account-access", "acm"];
    let regions = ["us-west-2", "eu-west-1"];
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
            "visibleServices": services,
            "visibleRegions": regions,
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
            "visibleServices": narrowed,
            "visibleRegions": regions,
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
        Some(&json!(regions)),
        "the preserved list must keep its sequence, got {after:?}"
    );
    assert!(
        after.get("visibleServices").and_then(Value::as_array).is_some(),
        "a configured list must never read back as null, got {after:?}"
    );
}
