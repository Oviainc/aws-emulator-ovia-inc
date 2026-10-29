// SPDX-License-Identifier: CC0-1.0

//! What a TABULAR `GetPropertyValue` page's `nextToken` resumes, end to end over
//! HTTP.
//!
//! RULE under test: a page that resumes at an ORDINAL is only resumed by a
//! request whose own members decide the same sequence that ordinal was counted
//! in — so a tabular token is resumed by a request carrying the
//! `tabularConditions` it was minted under, and refused with a 400
//! `ValidationException` by one whose filters or ordering decide another
//! sequence, rather than having the ordinal applied to rows it was never
//! counted in.
//!
//! The rows below differ along what the rule generalises over: WHICH condition
//! the resuming request changes (an ordering key's direction, which key is
//! primary, how many keys there are, whether there is an ordering at all, a
//! filter's property, its operator, its comparison value, how many filters there
//! are, whether there are any conditions at all) and, on the other side of the
//! rule, which SPELLING of the same conditions still resumes (the same request
//! repeated, a default spelled out, the filters listed in another order, a
//! filter repeated) — because a rule that refused every change of spelling would
//! break the paginator that resumes by repeating the whole request.

use serde_json::{json, Value};

use crate::harness::{Emulator, Resp};

/// The workspace every case below reads from.
const WORKSPACE: &str = "tabulartoken";

/// The component type whose instances are the table's rows.
const SENSOR: &str = "com.example.sensorRows";

/// The workspace, component type and entities every case below pages over.
///
/// Four instances, told apart by EVERY property a row can carry, so a page that
/// repeats or skips a row is visible in the rows it reports; `Zone` is the one
/// property they share, which is what a filter that selects all four compares
/// against.
async fn seed(emulator: &Emulator) {
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace: expected 200, got {created:?}"
    );

    let created = emulator
        .create_component_type(
            WORKSPACE,
            SENSOR,
            json!({"propertyDefinitions": {
                "Tag": {"dataType": {"type": "STRING"}},
                "Threshold": {"dataType": {"type": "DOUBLE"}},
                "Serial": {"dataType": {"type": "LONG"}},
                "Zone": {"dataType": {"type": "STRING"}},
            }}),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateComponentType {SENSOR:?}: expected 200, got {created:?}"
    );

    let instances: Vec<(&str, &str, f64, i64)> = vec![
        ("unit-1", "alpha", 1.0, 101),
        ("unit-2", "bravo", 2.0, 102),
        ("unit-3", "charlie", 3.0, 103),
        ("unit-4", "delta", 4.0, 104),
    ];
    for (entity_id, tag, threshold, serial) in instances {
        let created = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": entity_id,
                    "entityName": entity_id,
                    "components": {"Spec": {
                        "componentTypeId": SENSOR,
                        "properties": {
                            "Tag": {"value": {"stringValue": tag}},
                            "Threshold": {"value": {"doubleValue": threshold}},
                            "Serial": {"value": {"longValue": serial}},
                            "Zone": {"value": {"stringValue": "north"}},
                        },
                    }},
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "CreateEntity {entity_id:?}: expected 200, got {created:?}"
        );
    }
}

/// The rows of the ONE table a tabular answer carries.
fn table_rows(page: &Resp) -> Vec<Value> {
    page.member("tabularPropertyValues")
        .as_array()
        .and_then(|tables| tables.first())
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// One resume of a tabular page: the `tabularConditions` the resuming request
/// carries, and whether they are the ones the token's ordinal was counted under.
#[derive(Debug)]
struct Resume {
    /// What this row is about, quoted by every assertion it makes.
    what: &'static str,
    conditions: Value,
    /// `true` where these conditions are the minting request's — however they
    /// are spelled — so the page resumes at the row after the first page's last;
    /// `false` where they are different conditions, so the ordinal would land in
    /// a sequence it was not counted in and the token is refused instead.
    resumes: bool,
}

/// The ordering keys the token below is minted under: the DOUBLE property, then
/// the STRING one, each left to the order it is sorted by.
fn minting_orders() -> Value {
    json!([{"propertyName": "Threshold"}, {"propertyName": "Tag"}])
}

/// The filters the token below is minted under: two the four instances all
/// pass, so the sequence is the whole table and a shorter one is the resuming
/// request's doing.
fn minting_filters() -> Value {
    json!([
        {"propertyName": "Threshold", "operator": ">=", "value": {"doubleValue": 1.0}},
        {"propertyName": "Zone", "operator": "=", "value": {"stringValue": "north"}},
    ])
}

/// The `tabularConditions` of the minting request.
fn minting_conditions() -> Value {
    json!({"orderBy": minting_orders(), "propertyFilters": minting_filters()})
}

/// A tabular request over the seeded component type, carrying `conditions` and
/// resuming at `next_token` where one is given.
fn tabular_request(
    conditions: &Value,
    max_results: Option<u32>,
    next_token: Option<&str>,
) -> Value {
    let mut body = json!({
        "componentTypeId": SENSOR,
        "selectedProperties": ["Tag", "Threshold"],
        "tabularConditions": conditions,
    });
    if let Some(max_results) = max_results {
        body["maxResults"] = json!(max_results);
    }
    if let Some(next_token) = next_token {
        body["nextToken"] = json!(next_token);
    }
    body
}

/// Every spelling of the minting conditions that still names the same sequence,
/// and every change that names another one.
fn resumes() -> Vec<Resume> {
    let threshold_filter = json!(
        {"propertyName": "Threshold", "operator": ">=", "value": {"doubleValue": 1.0}}
    );
    let zone_filter = json!(
        {"propertyName": "Zone", "operator": "=", "value": {"stringValue": "north"}}
    );
    vec![
        Resume {
            what: "the whole request repeated, which is what an SDK paginator does",
            conditions: minting_conditions(),
            resumes: true,
        },
        Resume {
            what: "the ASCENDING default spelled out, which is the order it sorted by",
            conditions: json!({
                "orderBy": [
                    {"propertyName": "Threshold", "order": "ASCENDING"},
                    {"propertyName": "Tag", "order": "ASCENDING"},
                ],
                "propertyFilters": minting_filters(),
            }),
            resumes: true,
        },
        Resume {
            what: "the = operator left to the default it was applied under",
            conditions: json!({
                "orderBy": minting_orders(),
                "propertyFilters": [
                    threshold_filter,
                    {"propertyName": "Zone", "value": {"stringValue": "north"}},
                ],
            }),
            resumes: true,
        },
        Resume {
            what: "the two filters listed in the other order, which is the same conjunction",
            conditions: json!({
                "orderBy": minting_orders(),
                "propertyFilters": [zone_filter, threshold_filter],
            }),
            resumes: true,
        },
        Resume {
            what: "a filter repeated, which leaves the conjunction the rows pass unchanged",
            conditions: json!({
                "orderBy": minting_orders(),
                "propertyFilters": [threshold_filter, zone_filter, threshold_filter],
            }),
            resumes: true,
        },
        Resume {
            what: "the primary ordering key's direction flipped",
            conditions: json!({
                "orderBy": [
                    {"propertyName": "Threshold", "order": "DESCENDING"},
                    {"propertyName": "Tag"},
                ],
                "propertyFilters": minting_filters(),
            }),
            resumes: false,
        },
        Resume {
            what: "the ordering keys swapped, so the other one is primary",
            conditions: json!({
                "orderBy": [{"propertyName": "Tag"}, {"propertyName": "Threshold"}],
                "propertyFilters": minting_filters(),
            }),
            resumes: false,
        },
        Resume {
            what: "the secondary ordering key dropped",
            conditions: json!({
                "orderBy": [{"propertyName": "Threshold"}],
                "propertyFilters": minting_filters(),
            }),
            resumes: false,
        },
        Resume {
            what: "a third ordering key added",
            conditions: json!({
                "orderBy": [
                    {"propertyName": "Threshold"},
                    {"propertyName": "Tag"},
                    {"propertyName": "Serial"},
                ],
                "propertyFilters": minting_filters(),
            }),
            resumes: false,
        },
        Resume {
            what: "the ordering dropped altogether",
            conditions: json!({"propertyFilters": minting_filters()}),
            resumes: false,
        },
        Resume {
            what: "a filter dropped",
            conditions: json!({
                "orderBy": minting_orders(),
                "propertyFilters": [threshold_filter],
            }),
            resumes: false,
        },
        Resume {
            what: "a filter's comparison value changed, which shortens the sequence",
            conditions: json!({
                "orderBy": minting_orders(),
                "propertyFilters": [
                    {"propertyName": "Threshold", "operator": ">=", "value": {"doubleValue": 3.0}},
                    zone_filter,
                ],
            }),
            resumes: false,
        },
        Resume {
            what: "a filter's operator changed",
            conditions: json!({
                "orderBy": minting_orders(),
                "propertyFilters": [
                    {"propertyName": "Threshold", "operator": ">", "value": {"doubleValue": 1.0}},
                    zone_filter,
                ],
            }),
            resumes: false,
        },
        Resume {
            what: "a filter on another property, which is another conjunction even where it \
                   happens to keep the same rows",
            conditions: json!({
                "orderBy": minting_orders(),
                "propertyFilters": [
                    threshold_filter,
                    {"propertyName": "Tag", "operator": "!=", "value": {"stringValue": "zz"}},
                ],
            }),
            resumes: false,
        },
        Resume {
            what: "a filter added that leaves the sequence shorter than the ordinal",
            conditions: json!({
                "orderBy": minting_orders(),
                "propertyFilters": [
                    threshold_filter,
                    zone_filter,
                    {"propertyName": "Threshold", "operator": ">", "value": {"doubleValue": 3.5}},
                ],
            }),
            resumes: false,
        },
        Resume {
            what: "the conditions emptied",
            conditions: json!({}),
            resumes: false,
        },
    ]
}

#[tokio::test]
async fn a_tabular_next_token_is_bound_to_the_conditions_that_produced_it() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // The whole sequence the minting conditions decide, read in ONE page, so the
    // rows a resumed page has to carry are that sequence's own rather than a
    // literal written down here.
    let whole = emulator
        .get_property_value(WORKSPACE, tabular_request(&minting_conditions(), None, None))
        .await;
    assert_eq!(
        whole.status, 200,
        "the whole sequence: expected 200, got {whole:?}"
    );
    let sequence = table_rows(&whole);
    assert_eq!(
        sequence.len(),
        4,
        "the whole sequence: expected a row per instance of the component type, got {whole:?}"
    );

    // The page the token is minted by: the first two rows of that sequence.
    let minted = emulator
        .get_property_value(
            WORKSPACE,
            tabular_request(&minting_conditions(), Some(2), None),
        )
        .await;
    assert_eq!(
        minted.status, 200,
        "the minting page: expected 200, got {minted:?}"
    );
    assert_eq!(
        table_rows(&minted),
        sequence[..2].to_vec(),
        "the minting page: expected the sequence's first two rows, got {minted:?}"
    );
    let token = minted
        .member("nextToken")
        .as_str()
        .unwrap_or_else(|| {
            panic!("the minting page left rows over, so it must mint a token: {minted:?}")
        })
        .to_string();

    for Resume {
        what,
        conditions,
        resumes,
    } in resumes()
    {
        let page = emulator
            .get_property_value(
                WORKSPACE,
                tabular_request(&conditions, Some(2), Some(&token)),
            )
            .await;
        if resumes {
            assert_eq!(page.status, 200, "case {what:?}: expected 200, got {page:?}");
            assert_eq!(
                table_rows(&page),
                sequence[2..].to_vec(),
                "case {what:?}: expected the rows the minting page left over, got {page:?}"
            );
            assert!(
                page.member("nextToken").is_null(),
                "case {what:?}: expected the walk to end with those rows, got {page:?}"
            );
        } else {
            assert_eq!(page.status, 400, "case {what:?}: expected 400, got {page:?}");
            assert_eq!(
                page.error_type.as_deref(),
                Some("ValidationException"),
                "case {what:?}: expected a ValidationException, got {page:?}"
            );
            assert!(
                table_rows(&page).is_empty(),
                "case {what:?}: expected no rows at all, got {page:?}"
            );
        }
    }
}

/// RULE, the other side of it: the members that do NOT decide the sequence leave
/// the token resumable — a token is refused for naming another sequence, not for
/// every difference between two requests.
#[tokio::test]
async fn a_tabular_next_token_outlives_a_change_that_decides_no_row() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let minted = emulator
        .get_property_value(
            WORKSPACE,
            tabular_request(&minting_conditions(), Some(2), None),
        )
        .await;
    assert_eq!(
        minted.status, 200,
        "the minting page: expected 200, got {minted:?}"
    );
    let token = minted
        .member("nextToken")
        .as_str()
        .unwrap_or_else(|| {
            panic!("the minting page left rows over, so it must mint a token: {minted:?}")
        })
        .to_string();

    // The page SIZE decides how many rows a page carries, not which rows the
    // sequence holds or what order they come in: the token resumes at the same
    // ordinal under another one.
    let resumed = emulator
        .get_property_value(
            WORKSPACE,
            tabular_request(&minting_conditions(), Some(1), Some(&token)),
        )
        .await;
    assert_eq!(
        resumed.status, 200,
        "another maxResults: expected 200, got {resumed:?}"
    );
    let rows = table_rows(&resumed);
    assert_eq!(
        rows.len(),
        1,
        "another maxResults: expected the one row asked for, got {resumed:?}"
    );
    assert_eq!(
        rows[0]["Threshold"],
        json!({"doubleValue": 3.0}),
        "another maxResults: expected the third row of the sequence, got {resumed:?}"
    );
    assert!(
        !resumed.member("nextToken").is_null(),
        "another maxResults: expected a token for the row still left over, got {resumed:?}"
    );
}
