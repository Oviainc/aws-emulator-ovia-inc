// SPDX-License-Identifier: CC0-1.0

//! The `ListComponentTypes` `isAbstract` filter, end to end over HTTP.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch`, the
//! same entry the served emulator uses — so a route, a label, a status code and
//! a serialized body are all exercised, not just a handler body.
//!
//! The RULE under test: a component type is ABSTRACT when it cannot be
//! instantiated — one of the service's own base types, or a type that declares
//! an externally stored property (its own or inherited) that no data connector
//! of its own or of a parent can serve — and the `isAbstract` filter reports
//! exactly the stored component types whose abstractness matches, which is the
//! same abstractness `GetComponentType` reports for each of them. The two
//! filter values therefore PARTITION the workspace's component types, and
//! because abstractness is decided from what a component type holds, a patch of
//! a PARENT moves every component type that inherits from it across that
//! partition.
//!
//! The rows differ along what the rule generalises over — whether the external
//! property is a time series, which data-connector name backs it, whether the
//! property or the connector is the type's own or inherited, how deep the
//! inheritance runs, and whether the type is a caller's or one the workspace was
//! provisioned with — and the expectations are read off the service's own
//! catalogues (`state::DATA_CONNECTOR_FUNCTION_NAMES`, the built-in set) rather
//! than restated, so a catalogue change moves them with it.

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    DATA_CONNECTOR_FUNCTION_NAMES, MAX_PAGE_SIZE, SCHEMA_INITIALIZER_FUNCTION,
};

use crate::harness::{built_in_component_type_ids, with_built_in_component_types, Emulator, Resp};

/// The workspace every case of this suite lists over.
const WORKSPACE: &str = "abstractListWs";

/// One externally stored property definition: `isStoredExternally` is "a Boolean
/// value that specifies whether the property is stored externally" (the member's
/// own documentation), which is what leaves its values unservable without a data
/// connector.
fn external_property(is_time_series: bool) -> Value {
    json!({
        "dataType": {"type": "DOUBLE"},
        "isTimeSeries": is_time_series,
        "isStoredExternally": true,
    })
}

/// One function entry under `function_name`, implemented by a lambda the way
/// `FunctionRequest::implementedBy` (a `DataConnector`) models it.
fn function(function_name: &str) -> Value {
    json!({
        function_name: {
            "scope": "ENTITY",
            "implementedBy": {
                "lambda": {"arn": "arn:aws:lambda:us-west-2:000000000000:function:connector"},
            },
        },
    })
}

/// The `componentTypeId`s of a page, in the order the page served them.
fn page_ids(case: &str, resp: &Resp) -> Vec<String> {
    resp.member("componentTypeSummaries")
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: componentTypeSummaries must be a present array, got {resp:?}")
        })
        .iter()
        .map(|summary| {
            summary["componentTypeId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: every summary carries a componentTypeId, got {resp:?}")
                })
                .to_string()
        })
        .collect()
}

/// The ids ONE page of `filters: [{"isAbstract": <is_abstract>}]` reports, taken
/// at the service's own ceiling so the whole selected set is on it.
async fn filtered_ids(emulator: &Emulator, case: &str, is_abstract: bool) -> Vec<String> {
    let listed = emulator
        .list_component_types(
            WORKSPACE,
            json!({"maxResults": MAX_PAGE_SIZE, "filters": [{"isAbstract": is_abstract}]}),
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: expected 200 from a filtered listing, got {listed:?}"
    );
    assert!(
        listed.member("nextToken").is_null(),
        "case {case:?}: a page at the ceiling leaves nothing over, got {listed:?}"
    );
    page_ids(case, &listed)
}

/// The abstractness `GetComponentType` reports for one component type.
async fn read_is_abstract(emulator: &Emulator, case: &str, component_type_id: &str) -> bool {
    let read = emulator
        .get_component_type(WORKSPACE, component_type_id)
        .await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: {component_type_id:?} must read back, got {read:?}"
    );
    read.member("isAbstract").as_bool().unwrap_or_else(|| {
        panic!("case {case:?}: {component_type_id:?} must report isAbstract, got {read:?}")
    })
}

/// A workspace holding only the built-ins it was provisioned with.
async fn seeded() -> Emulator {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace(WORKSPACE).await.status,
        200,
        "the workspace every case lists over must exist"
    );
    emulator
}

/// RULE: the `isAbstract` filter reports exactly the stored component types
/// whose abstractness matches — the same abstractness the single-resource read
/// reports — so its two values partition the workspace.
#[tokio::test]
async fn the_is_abstract_filter_partitions_the_workspace() {
    let emulator = seeded().await;

    // A connector-carrying parent and an unservable-property parent, so the
    // inheritance rows below can take each half of the rule from a parent.
    let connector_parent = "com.example.connector.parent";
    let external_parent = "com.example.external.parent";

    // (componentTypeId, the create body, whether the type is abstract).
    let mut rows: Vec<(String, Value, bool)> = vec![
        (
            connector_parent.to_string(),
            json!({
                "propertyDefinitions": {"Label": {"dataType": {"type": "STRING"}}},
                "functions": function("dataReader"),
            }),
            false,
        ),
        (
            external_parent.to_string(),
            json!({"propertyDefinitions": {"Inherited": external_property(true)}}),
            true,
        ),
        (
            // The recorded shape: one externally stored time-series property
            // and no functions at all.
            "com.example.timeSeriesExternal".to_string(),
            json!({"propertyDefinitions": {"Temperature": external_property(true)}}),
            true,
        ),
        (
            // The same gap with a property that is NOT a time series.
            "com.example.plainExternal".to_string(),
            json!({"propertyDefinitions": {"Serial": external_property(false)}}),
            true,
        ),
        (
            // A schema initializer is not a data connector: it initializes the
            // schema and serves no property's values.
            "com.example.schemaInitializerOnly".to_string(),
            json!({
                "propertyDefinitions": {"Temperature": external_property(true)},
                "functions": function(SCHEMA_INITIALIZER_FUNCTION),
            }),
            true,
        ),
        (
            // No externally stored property at all: nothing is unservable.
            "com.example.plainOnly".to_string(),
            json!({"propertyDefinitions": {"Label": {"dataType": {"type": "STRING"}}}}),
            false,
        ),
        (
            // The property is the type's own, the connector INHERITED.
            "com.example.inheritedConnector".to_string(),
            json!({
                "extendsFrom": [connector_parent],
                "propertyDefinitions": {"Temperature": external_property(true)},
            }),
            false,
        ),
        (
            // The unservable property is INHERITED and no connector reaches it.
            "com.example.inheritedExternalProperty".to_string(),
            json!({"extendsFrom": [external_parent]}),
            true,
        ),
        (
            // The property comes from one parent and the connector from another.
            "com.example.inheritedBoth".to_string(),
            json!({"extendsFrom": [external_parent, connector_parent]}),
            false,
        ),
    ];
    // One row per data-connector name the service publishes: each of them backs
    // an externally stored property, so each leaves the type concrete.
    for connector in DATA_CONNECTOR_FUNCTION_NAMES {
        rows.push((
            format!("com.example.backedBy.{connector}"),
            json!({
                "propertyDefinitions": {"Temperature": external_property(true)},
                "functions": function(connector),
            }),
            false,
        ));
    }

    let mut expected_abstract: Vec<String> = Vec::new();
    for (component_type_id, request, is_abstract) in &rows {
        let created = emulator
            .create_component_type(WORKSPACE, component_type_id, request.clone())
            .await;
        assert_eq!(
            created.status, 200,
            "case {component_type_id:?}: expected 200 from CreateComponentType, got {created:?}"
        );
        let read = read_is_abstract(&emulator, component_type_id, component_type_id).await;
        assert_eq!(
            read, *is_abstract,
            "case {component_type_id:?}: expected isAbstract {is_abstract}, got {read}"
        );
        if *is_abstract {
            expected_abstract.push(component_type_id.clone());
        }
    }
    // The built-ins are part of the partition too, and the abstractness they
    // carry is the service's own: taken from the single-resource read rather
    // than restated, so the catalogue decides which side each falls on.
    for component_type_id in built_in_component_type_ids() {
        if read_is_abstract(&emulator, "builtIn", &component_type_id).await {
            expected_abstract.push(component_type_id);
        }
    }
    expected_abstract.sort();

    let every_id = with_built_in_component_types(
        rows.iter()
            .map(|(component_type_id, _, _)| component_type_id.clone()),
    );
    let expected_concrete: Vec<String> = every_id
        .iter()
        .filter(|component_type_id| !expected_abstract.contains(component_type_id))
        .cloned()
        .collect();
    assert!(
        !expected_abstract.is_empty() && !expected_concrete.is_empty(),
        "the suite must span both halves of the partition, got {expected_abstract:?} \
         and {expected_concrete:?}"
    );

    assert_eq!(
        filtered_ids(&emulator, "isAbstractTrue", true).await,
        expected_abstract,
        "case \"isAbstractTrue\": the filter must select exactly {expected_abstract:?}"
    );
    assert_eq!(
        filtered_ids(&emulator, "isAbstractFalse", false).await,
        expected_concrete,
        "case \"isAbstractFalse\": the filter must select exactly {expected_concrete:?}"
    );

    // The two halves partition the workspace: together they are the unfiltered
    // listing, with nothing listed twice and nothing left out.
    let unfiltered = emulator
        .list_component_types(WORKSPACE, json!({"maxResults": MAX_PAGE_SIZE}))
        .await;
    let mut both_halves = expected_abstract.clone();
    both_halves.extend(expected_concrete.clone());
    both_halves.sort();
    assert_eq!(
        page_ids("unfiltered", &unfiltered),
        both_halves,
        "case \"unfiltered\": the two filter values must cover the listing, got {unfiltered:?}"
    );

    // A filtered listing still PAGINATES: the filter picks the rows and the page
    // size cuts them, so walking one row at a time reports the selected set once.
    let mut walked: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    for _ in 0..=expected_abstract.len() {
        let request = match &next_token {
            Some(token) => json!({
                "maxResults": 1,
                "nextToken": token,
                "filters": [{"isAbstract": true}],
            }),
            None => json!({"maxResults": 1, "filters": [{"isAbstract": true}]}),
        };
        let page = emulator.list_component_types(WORKSPACE, request).await;
        assert_eq!(
            page.status, 200,
            "case \"isAbstractTruePaged\": expected 200 from a filtered page, got {page:?}"
        );
        walked.extend(page_ids("isAbstractTruePaged", &page));
        next_token = page
            .member("nextToken")
            .as_str()
            .map(|token| token.to_string());
        if next_token.is_none() {
            break;
        }
    }
    assert_eq!(
        next_token, None,
        "case \"isAbstractTruePaged\": the walk must end, got {next_token:?} with {walked:?}"
    );
    assert_eq!(
        walked, expected_abstract,
        "case \"isAbstractTruePaged\": a one-row-at-a-time walk must report \
         {expected_abstract:?}, got {walked:?}"
    );
}

/// RULE: abstractness is decided from the properties, functions and inheritance
/// a component type holds, so a patch of a PARENT moves the parent AND every
/// component type that inherits from it — however deep — across both the
/// single-resource read and the `isAbstract` filter.
#[tokio::test]
async fn a_patch_of_a_parent_moves_every_component_type_that_inherits_from_it() {
    let emulator = seeded().await;

    // A three-deep chain whose only property is an unservable one declared at
    // the top, so every member of the chain depends on the parent's functions
    // for it: the base, its child, and the child's child.
    let base = "com.example.chain.base";
    let child = "com.example.chain.child";
    let grandchild = "com.example.chain.grandchild";
    // A concrete control outside the chain, which no patch below touches.
    let outsider = "com.example.chain.outsider";
    for (component_type_id, request) in [
        (
            base,
            json!({"propertyDefinitions": {"Temperature": external_property(true)}}),
        ),
        (child, json!({"extendsFrom": [base]})),
        (grandchild, json!({"extendsFrom": [child]})),
        (
            outsider,
            json!({"propertyDefinitions": {"Label": {"dataType": {"type": "STRING"}}}}),
        ),
    ] {
        let created = emulator
            .create_component_type(WORKSPACE, component_type_id, request)
            .await;
        assert_eq!(
            created.status, 200,
            "case {component_type_id:?}: expected 200 from CreateComponentType, got {created:?}"
        );
    }
    let chain = [base, child, grandchild];

    // (case, the patch applied to the BASE, whether the chain is abstract after
    // it). The connector names are the service's own catalogue, so every name it
    // publishes is exercised in both directions: added, then taken away again.
    let mut rows: Vec<(String, Option<Value>, bool)> = vec![("asCreated".to_string(), None, true)];
    for connector in DATA_CONNECTOR_FUNCTION_NAMES {
        rows.push((
            format!("baseGains.{connector}"),
            Some(json!({"functions": function(connector)})),
            false,
        ));
        // The functions map is replaced WHOLE, so naming it empty drops the
        // connector and the inherited property is unservable again.
        rows.push((
            format!("baseLoses.{connector}"),
            Some(json!({"functions": {}})),
            true,
        ));
    }
    // The property goes instead of the connector: with no externally stored
    // property anywhere in the chain nothing is unservable.
    rows.push((
        "basePropertyDropped".to_string(),
        Some(json!({
            "propertyDefinitions": {"Label": {"dataType": {"type": "STRING"}}},
        })),
        false,
    ));

    for (case, patch, is_abstract) in &rows {
        if let Some(patch) = patch {
            let updated = emulator
                .update_component_type(WORKSPACE, base, patch.clone())
                .await;
            assert_eq!(
                updated.status, 200,
                "case {case:?}: expected 200 from UpdateComponentType, got {updated:?}"
            );
        }

        let selected = filtered_ids(&emulator, case, *is_abstract).await;
        let rejected = filtered_ids(&emulator, case, !*is_abstract).await;
        for component_type_id in chain {
            let read = read_is_abstract(&emulator, case, component_type_id).await;
            assert_eq!(
                read, *is_abstract,
                "case {case:?}: {component_type_id:?} must report isAbstract {is_abstract}, \
                 got {read}"
            );
            assert!(
                selected.contains(&component_type_id.to_string()),
                "case {case:?}: isAbstract {is_abstract} must select {component_type_id:?}, \
                 got {selected:?}"
            );
            assert!(
                !rejected.contains(&component_type_id.to_string()),
                "case {case:?}: isAbstract {} must not select {component_type_id:?}, \
                 got {rejected:?}",
                !*is_abstract
            );
        }
        // The control never moves: a patch decides abstractness from what each
        // component type holds, not from when it was written.
        let outsider_read = read_is_abstract(&emulator, case, outsider).await;
        assert!(
            !outsider_read,
            "case {case:?}: {outsider:?} holds nothing unservable and must stay concrete"
        );
        assert!(
            filtered_ids(&emulator, case, false)
                .await
                .contains(&outsider.to_string()),
            "case {case:?}: isAbstract false must keep selecting {outsider:?}"
        );
    }
}
