// SPDX-License-Identifier: CC0-1.0

//! The THREE request forms a body member has, over the wire.
//!
//! One rule, asserted over a table spanning its class (the shared harness is
//! `crate::wire`): a JSON request body says one of three different things about a
//! list setting, and `UpdateAccountCustomizations` gives each its own meaning —
//!
//! * OMITTED: "Omitted settings remain unchanged" — the stored value survives;
//! * carried with an explicit `null`: "To reset a setting to its default
//!   behavior, set the value to null for visible Regions and visible services" —
//!   the setting returns to UNCONFIGURED, which `GetAccountCustomizations`
//!   reports as an absent member ("visible Regions and visible services return
//!   null");
//! * carried with an array: the setting is configured to those members, an EMPTY
//!   array included — a present, empty list is the setting naming nothing, a
//!   different state from unconfigured.
//!
//! (`smithy_query(query="doc", shape="UpdateAccountCustomizations")`, the member
//! docs on `UpdateAccountCustomizationsInput$visibleServices`/`$visibleRegions`,
//! and `smithy_query(query="doc", shape="GetAccountCustomizations")` for what the
//! default reads back as.)
//!
//! What a list setting reads back as once CONFIGURED is `account_customizations`'
//! and `list_setting_order`'s rule; this suite owns the three forms and the
//! transitions between them.

use crate::support::reported_owned;
use crate::wire::{
    build_router, catalog_services, get_customizations, legal_region, update_customizations,
};
use serde_json::{json, Value};

/// One request of a scenario, with the WHOLE record the service must answer
/// afterwards.
struct Step {
    /// What this step names about the record.
    did: &'static str,
    /// The request body it sends.
    request: Value,
    /// The document `UpdateAccountCustomizations` must answer and
    /// `GetAccountCustomizations` must answer again: an absent member is the
    /// setting unconfigured.
    expected: Value,
}

/// Drive one scenario's steps in order against a fresh record, asserting after
/// each that the update's echo, a following read and a REPEAT of the same request
/// (`@idempotent`) all report the one expected record.
async fn walk(case: &str, steps: Vec<Step>) {
    let router = build_router();
    for (at, step) in steps.iter().enumerate() {
        let position = format!("case {case:?} step {at} ({})", step.did);

        let (status, _, updated) = update_customizations(&router, step.request.clone()).await;
        assert_eq!(
            status, 200,
            "{position}: Update expected 200, got {status} ({updated})"
        );
        assert_eq!(
            updated, step.expected,
            "{position}: Update must answer {:?}, got {updated:?}",
            step.expected
        );

        let (status, _, fetched) = get_customizations(&router).await;
        assert_eq!(
            status, 200,
            "{position}: Get expected 200, got {status} ({fetched})"
        );
        assert_eq!(
            fetched, step.expected,
            "{position}: Get must answer what Update stored, got {fetched:?}"
        );

        let (_, _, repeated) = update_customizations(&router, step.request.clone()).await;
        assert_eq!(
            repeated, step.expected,
            "{position}: the same request again must answer identically, got {repeated:?}"
        );
        let (_, _, after) = get_customizations(&router).await;
        assert_eq!(
            after, step.expected,
            "{position}: the same request again must leave state identical, got {after:?}"
        );
    }
}

/// The rule, over a table spanning the class: which setting is reset, the state it
/// is reset FROM (unconfigured, one member, ten members, an empty list), what else
/// the resetting request carries (nothing, the other list, the colour), and what
/// can follow a reset (configuring the setting again).
#[tokio::test]
async fn an_explicit_null_resets_a_list_setting_to_its_default() {
    // Service identifiers are drawn from the catalog `ListServices` enumerates,
    // which is what this setting admits; the Region codes come from the harness's
    // generator for the member's own `@pattern`.
    let catalog = build_router();
    let services = catalog_services(&catalog, 10).await;
    let ten: Vec<String> = services.clone();
    let one: Vec<String> = vec![services[3].clone()];
    let two: Vec<String> = vec![services[1].clone(), services[7].clone()];
    let regions: Vec<String> = (0..4).map(legal_region).collect();

    let scenarios: Vec<(&str, Vec<Step>)> = vec![
        (
            "a one-member services list",
            vec![
                Step {
                    did: "configures one service",
                    request: json!({"visibleServices": one}),
                    expected: json!({
                        "accountColor": "none",
                        "visibleServices": reported_owned(&one),
                    }),
                },
                Step {
                    did: "resets it with null",
                    request: json!({"visibleServices": null}),
                    expected: json!({"accountColor": "none"}),
                },
            ],
        ),
        (
            "a ten-member services list beside a colour and Regions",
            vec![
                Step {
                    did: "configures all three settings",
                    request: json!({
                        "accountColor": "teal",
                        "visibleServices": ten,
                        "visibleRegions": regions,
                    }),
                    expected: json!({
                        "accountColor": "teal",
                        "visibleServices": reported_owned(&ten),
                        "visibleRegions": reported_owned(&regions),
                    }),
                },
                Step {
                    did: "resets the services only",
                    request: json!({"visibleServices": null}),
                    expected: json!({
                        "accountColor": "teal",
                        "visibleRegions": reported_owned(&regions),
                    }),
                },
                Step {
                    did: "resets the Regions too",
                    request: json!({"visibleRegions": null}),
                    expected: json!({"accountColor": "teal"}),
                },
            ],
        ),
        (
            "both lists reset by one request",
            vec![
                Step {
                    did: "configures both lists",
                    request: json!({"visibleServices": two, "visibleRegions": regions}),
                    expected: json!({
                        "accountColor": "none",
                        "visibleServices": reported_owned(&two),
                        "visibleRegions": reported_owned(&regions),
                    }),
                },
                Step {
                    did: "nulls both in one body",
                    request: json!({"visibleServices": null, "visibleRegions": null}),
                    expected: json!({"accountColor": "none"}),
                },
            ],
        ),
        (
            "a null for a setting that was never configured",
            vec![
                Step {
                    did: "nulls an unconfigured services list",
                    request: json!({"visibleServices": null}),
                    expected: json!({"accountColor": "none"}),
                },
                Step {
                    did: "nulls an unconfigured Regions list",
                    request: json!({"visibleRegions": null}),
                    expected: json!({"accountColor": "none"}),
                },
            ],
        ),
        (
            "a null beside a value for the other setting",
            vec![
                Step {
                    did: "configures both lists",
                    request: json!({"visibleServices": ten, "visibleRegions": regions}),
                    expected: json!({
                        "accountColor": "none",
                        "visibleServices": reported_owned(&ten),
                        "visibleRegions": reported_owned(&regions),
                    }),
                },
                Step {
                    did: "resets the services while re-setting the Regions",
                    request: json!({
                        "visibleServices": null,
                        "visibleRegions": [regions[0].clone()],
                    }),
                    expected: json!({
                        "accountColor": "none",
                        "visibleRegions": [regions[0].clone()],
                    }),
                },
            ],
        ),
        (
            "a null beside a colour",
            vec![
                Step {
                    did: "configures a colour and a services list",
                    request: json!({"accountColor": "pink", "visibleServices": two}),
                    expected: json!({
                        "accountColor": "pink",
                        "visibleServices": reported_owned(&two),
                    }),
                },
                Step {
                    did: "resets the list and sets the colour in one body",
                    request: json!({"accountColor": "orange", "visibleServices": null}),
                    expected: json!({"accountColor": "orange"}),
                },
            ],
        ),
        (
            "an empty list is configured, and null unconfigures it",
            vec![
                Step {
                    did: "configures both lists empty",
                    request: json!({"visibleServices": [], "visibleRegions": []}),
                    expected: json!({
                        "accountColor": "none",
                        "visibleServices": [],
                        "visibleRegions": [],
                    }),
                },
                Step {
                    did: "nulls both",
                    request: json!({"visibleServices": null, "visibleRegions": null}),
                    expected: json!({"accountColor": "none"}),
                },
            ],
        ),
        (
            "a setting configured again after a reset",
            vec![
                Step {
                    did: "configures a services list",
                    request: json!({"visibleServices": ten}),
                    expected: json!({
                        "accountColor": "none",
                        "visibleServices": reported_owned(&ten),
                    }),
                },
                Step {
                    did: "resets it",
                    request: json!({"visibleServices": null}),
                    expected: json!({"accountColor": "none"}),
                },
                Step {
                    did: "configures it again, with other members",
                    request: json!({"visibleServices": one}),
                    expected: json!({
                        "accountColor": "none",
                        "visibleServices": reported_owned(&one),
                    }),
                },
                Step {
                    did: "resets it once more",
                    request: json!({"visibleServices": null}),
                    expected: json!({"accountColor": "none"}),
                },
            ],
        ),
    ];

    for (case, steps) in scenarios {
        walk(case, steps).await;
    }
}

/// The OTHER two forms, pinned against the reset so the three cannot collapse into
/// each other: omitting a member leaves the stored list exactly as it was, and an
/// empty array is a stored, reported empty list — neither is the absent member a
/// `null` leaves behind.
#[tokio::test]
async fn omitting_a_member_and_sending_it_empty_are_not_the_reset() {
    let router = build_router();
    let services = catalog_services(&router, 3).await;
    let regions: Vec<String> = (0..3).map(legal_region).collect();

    let (status, _, body) = update_customizations(
        &router,
        json!({"visibleServices": services, "visibleRegions": regions}),
    )
    .await;
    assert_eq!(status, 200, "configuring both lists: got {status} ({body})");

    // OMITTED: three bodies that name no list at all, including the empty body.
    for request in [
        json!({}),
        json!({"accountColor": "green"}),
        json!({"accountColor": "none"}),
    ] {
        let (status, _, body) = update_customizations(&router, request.clone()).await;
        assert_eq!(
            status, 200,
            "case {request:?}: expected 200, got {status} ({body})"
        );
        assert_eq!(
            body.get("visibleServices"),
            Some(&reported_owned(&services)),
            "case {request:?}: an omitted member must leave the stored list alone, got {body:?}"
        );
        assert_eq!(
            body.get("visibleRegions"),
            Some(&reported_owned(&regions)),
            "case {request:?}: an omitted member must leave the stored list alone, got {body:?}"
        );
    }

    // EMPTY: a present, empty array — not the absent member a reset leaves.
    let (status, _, body) =
        update_customizations(&router, json!({"visibleServices": [], "visibleRegions": []})).await;
    assert_eq!(
        status, 200,
        "empty lists: expected 200, got {status} ({body})"
    );
    assert_eq!(
        (body.get("visibleServices"), body.get("visibleRegions")),
        (Some(&json!([])), Some(&json!([]))),
        "an empty array must be stored and reported as an empty array, got {body:?}"
    );

    // NULL: the member goes away, for both settings.
    let (status, _, body) = update_customizations(
        &router,
        json!({"visibleServices": null, "visibleRegions": null}),
    )
    .await;
    assert_eq!(
        status, 200,
        "nulled lists: expected 200, got {status} ({body})"
    );
    assert_eq!(
        (body.get("visibleServices"), body.get("visibleRegions")),
        (None, None),
        "a nulled list must be reported as an absent member, got {body:?}"
    );
}

/// The reset is a request FORM, not a value: a `null` member supplies no list, so
/// the rules a supplied list is judged by have nothing to judge — while the
/// colour, whose documented reset is its own `none` value, keeps reporting that
/// value rather than going absent.
#[tokio::test]
async fn a_nulled_member_is_not_a_supplied_value() {
    let router = build_router();
    let services = catalog_services(&router, 2).await;
    let (status, _, body) = update_customizations(
        &router,
        json!({"accountColor": "red", "visibleServices": services}),
    )
    .await;
    assert_eq!(status, 200, "priming: expected 200, got {status} ({body})");

    // A `null` carries no identifier, so the catalog-membership rule a supplied
    // `visibleServices` is judged by has nothing to refuse: the reset is accepted
    // where a list naming a non-catalog identifier is refused with 400.
    let (status, error_type, body) =
        update_customizations(&router, json!({"visibleServices": ["not-a-console-service-id"]}))
            .await;
    assert_eq!(
        status, 400,
        "a non-catalog identifier must still be refused, got {status} ({body})"
    );
    assert_eq!(
        error_type, "ValidationException",
        "a non-catalog identifier must be refused with a ValidationException, got {error_type:?}"
    );
    let (status, _, body) = update_customizations(&router, json!({"visibleServices": null})).await;
    assert_eq!(
        status, 200,
        "a nulled visibleServices names no identifier to refuse, got {status} ({body})"
    );
    assert_eq!(
        body,
        json!({"accountColor": "red"}),
        "the reset must leave only the colour configured, got {body:?}"
    );

    // The colour's reset is its modelled `none` value: it is reported, not absent.
    let (status, _, body) = update_customizations(&router, json!({"accountColor": "none"})).await;
    assert_eq!(
        status, 200,
        "resetting the colour: expected 200, got {status} ({body})"
    );
    assert_eq!(
        body,
        json!({"accountColor": "none"}),
        "an unconfigured colour is the reported value none, got {body:?}"
    );
}
