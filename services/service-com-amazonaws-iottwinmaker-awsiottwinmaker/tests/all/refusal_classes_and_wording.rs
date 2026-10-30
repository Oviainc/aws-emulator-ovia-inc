// SPDX-License-Identifier: CC0-1.0

//! How this service CLASSIFIES and WORDS the refusals of the component-type
//! control plane, the property data plane and the tagging quota, end to end over
//! HTTP.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch`, the
//! same entry the served emulator uses — so a route, a status code, an error code
//! and a serialized body are all exercised, not just a handler body.
//!
//! The RULES under test, each asserted over a table whose rows differ along what
//! the rule generalises over rather than over one recorded request:
//!
//! 1. `BatchPutPropertyValues` resolves EVERY entry's reference before anything
//!    is written and refuses the WHOLE call for a reference it cannot write
//!    through — 404 `ResourceNotFoundException` when the reference names no
//!    stored entity, 400 `ValidationException` when it resolves to no writable
//!    property — reserving `errorEntries` for an entry whose VALUES it cannot
//!    record.
//! 2. A component type the workspace does not hold reads the same whichever
//!    operation looked for it.
//! 3. A delete blocked by what still references a component type names the IDS
//!    of the resources to delete first and nothing else of the service's state.
//! 4. The property data plane tells a missing ENTITY (404
//!    `ResourceNotFoundException`) from a component an existing entity does not
//!    have (400 `ValidationException`).
//! 5. A tagging call past the per-resource tag QUOTA is a quota refusal (402
//!    `ServiceQuotaExceededException`), which is a different failure from the
//!    per-call bound on the request's own tag map.

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    DATA_WRITER_FUNCTION, MAX_TAGS_PER_CALL, MAX_TAGS_PER_RESOURCE,
};

use crate::harness::{data_writer_functions, workspace_arn, Emulator, Resp};

/// The workspace every case below acts in.
const WORKSPACE: &str = "refusalClasses";

/// A component type declaring the write connector a value is set THROUGH, so a
/// write to its properties is one the data plane can serve.
const WRITABLE: &str = "com.example.writable";
/// The same property definitions with NO write connector declared: nothing can
/// set the values of its properties.
const UNWRITABLE: &str = "com.example.unwritable";
/// A component type whose composite declaration makes [`WRITABLE`] reachable at
/// the path `Assembly/inner` of an entity component of it.
const ASSEMBLY: &str = "com.example.assembly";

/// The entity carrying a [`WRITABLE`] component named `Sensor`.
const ENTITY: &str = "unit-1";
/// The entity carrying an [`UNWRITABLE`] component named `Sensor`.
const UNWRITABLE_ENTITY: &str = "unit-2";
/// The entity carrying an [`ASSEMBLY`] component named `Assembly` with a
/// composite [`WRITABLE`] component inside it.
const COMPOSITE_ENTITY: &str = "unit-3";

/// The time series property every write below sets.
const SERIES: &str = "Reading";
/// A property of the same component types that is NOT a time series property.
const PLAIN: &str = "Label";

/// The instant every write below carries.
const T0: &str = "2026-04-01T00:00:00Z";

/// The `message` an error body carries.
fn message(resp: &Resp) -> String {
    resp.member("message")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The property definitions every component type below declares.
fn property_definitions() -> Value {
    json!({
        SERIES: {"dataType": {"type": "DOUBLE"}, "isTimeSeries": true},
        PLAIN: {"dataType": {"type": "STRING"}},
    })
}

/// The workspace, component types and entities the data-plane cases act on.
async fn seed(emulator: &Emulator) {
    assert_eq!(
        emulator.create_workspace(WORKSPACE).await.status,
        200,
        "the workspace every case acts in must be created"
    );

    let component_types: Vec<(&str, Value)> = vec![
        (
            WRITABLE,
            json!({
                "propertyDefinitions": property_definitions(),
                "functions": data_writer_functions(),
            }),
        ),
        (
            UNWRITABLE,
            json!({"propertyDefinitions": property_definitions()}),
        ),
        (
            ASSEMBLY,
            json!({
                "propertyDefinitions": property_definitions(),
                "compositeComponentTypes": {"inner": {"componentTypeId": WRITABLE}},
                "functions": data_writer_functions(),
            }),
        ),
    ];
    for (component_type_id, body) in component_types {
        let created = emulator
            .create_component_type(WORKSPACE, component_type_id, body)
            .await;
        assert_eq!(
            created.status, 200,
            "CreateComponentType {component_type_id:?}: expected 200, got {created:?}"
        );
    }

    let entities: Vec<Value> = vec![
        json!({
            "entityId": ENTITY,
            "entityName": ENTITY,
            "components": {"Sensor": {"componentTypeId": WRITABLE}},
        }),
        json!({
            "entityId": UNWRITABLE_ENTITY,
            "entityName": UNWRITABLE_ENTITY,
            "components": {"Sensor": {"componentTypeId": UNWRITABLE}},
        }),
        json!({
            "entityId": COMPOSITE_ENTITY,
            "entityName": COMPOSITE_ENTITY,
            "components": {"Assembly": {"componentTypeId": ASSEMBLY}},
            "compositeComponents": {"Assembly/inner": {}},
        }),
    ];
    for body in entities {
        let entity_id = body["entityId"].as_str().unwrap_or_default().to_string();
        let created = emulator.create_entity(WORKSPACE, body).await;
        assert_eq!(
            created.status, 200,
            "CreateEntity {entity_id:?}: expected 200, got {created:?}"
        );
    }
}

/// One entry for `BatchPutPropertyValues`: the reference members merged with the
/// property name, and the values the entry carries.
fn entry_with(reference: Value, property_name: &str, values: Value) -> Value {
    let mut entity_property_reference = reference;
    entity_property_reference["propertyName"] = json!(property_name);
    json!({
        "entityPropertyReference": entity_property_reference,
        "propertyValues": values,
    })
}

/// One entry carrying a single servable value at [`T0`].
fn entry(reference: Value, property_name: &str) -> Value {
    entry_with(
        reference,
        property_name,
        json!([{"time": T0, "value": {"doubleValue": 1.0}}]),
    )
}

/// A reference naming a component of an entity by name.
fn by_name(entity_id: &str, component_name: &str) -> Value {
    json!({"entityId": entity_id, "componentName": component_name})
}

/// One unresolvable reference: the entry it travels in, and the refusal the
/// whole call earns.
struct Unresolvable {
    case: &'static str,
    reference: fn() -> Value,
    property_name: &'static str,
    status: u16,
    error: &'static str,
    expected_message: fn() -> String,
}

/// The reference classes the rule is asserted over, each with the refusal the
/// whole request earns for it.
fn unresolvable_references() -> Vec<Unresolvable> {
    vec![
        Unresolvable {
            case: "an entityId the workspace does not hold",
            reference: || by_name("ghost-9", "Sensor"),
            property_name: SERIES,
            status: 404,
            error: "ResourceNotFoundException",
            expected_message: || {
                format!("Entity ghost-9 in workspace {WORKSPACE} not found.")
            },
        },
        Unresolvable {
            case: "a componentName the entity does not have",
            reference: || by_name(ENTITY, "NoSuchComponent"),
            property_name: SERIES,
            status: 400,
            error: "ValidationException",
            expected_message: || {
                format!(
                    "Component NoSuchComponent of Entity {ENTITY} in workspace {WORKSPACE} \
                     not found."
                )
            },
        },
        Unresolvable {
            case: "a componentPath no component of the entity sits at",
            reference: || json!({"entityId": COMPOSITE_ENTITY, "componentPath": "Assembly/absent"}),
            property_name: SERIES,
            status: 400,
            error: "ValidationException",
            expected_message: || {
                format!(
                    "Component Assembly/absent of Entity {COMPOSITE_ENTITY} in workspace \
                     {WORKSPACE} not found."
                )
            },
        },
        Unresolvable {
            case: "a propertyName the addressed component does not have",
            reference: || by_name(ENTITY, "Sensor"),
            property_name: "Nowhere",
            status: 400,
            error: "ValidationException",
            expected_message: || {
                format!(
                    "Property Nowhere is not a property of component Sensor of Entity \
                     {ENTITY} in workspace {WORKSPACE}."
                )
            },
        },
        Unresolvable {
            case: "a well-formed reference whose component type declares no write connector",
            reference: || by_name(UNWRITABLE_ENTITY, "Sensor"),
            property_name: SERIES,
            status: 400,
            error: "ValidationException",
            expected_message: || {
                format!(
                    "No {DATA_WRITER_FUNCTION} connector is defined for component Sensor of \
                     Entity {UNWRITABLE_ENTITY} in workspace {WORKSPACE}, so the values of \
                     property {SERIES} cannot be set."
                )
            },
        },
        Unresolvable {
            case: "a property of a writable component type that is not a time series property",
            reference: || by_name(ENTITY, "Sensor"),
            property_name: PLAIN,
            status: 400,
            error: "ValidationException",
            expected_message: || format!("{PLAIN} is not marked as a time-series property."),
        },
    ]
}

/// The values of [`SERIES`] the data plane holds for `ENTITY`'s `Sensor`.
async fn held_values(emulator: &Emulator) -> Vec<Value> {
    let read = emulator
        .get_property_value_history(
            WORKSPACE,
            json!({
                "entityId": ENTITY,
                "componentName": "Sensor",
                "selectedProperties": [SERIES],
            }),
        )
        .await;
    assert_eq!(
        read.status, 200,
        "reading the series back must be served, got {read:?}"
    );
    read.member("propertyValues")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .flat_map(|series| series["values"].as_array().cloned().unwrap_or_default())
        .collect()
}

/// RULE: a reference `BatchPutPropertyValues` cannot write through refuses the
/// WHOLE call at the HTTP level — the modelled `ResourceNotFoundException` for a
/// reference naming no stored entity, the modelled `ValidationException` for one
/// that resolves to no writable property — and nothing of that call is applied,
/// not even the entries beside it that would have been servable.
#[tokio::test]
async fn an_unresolvable_reference_refuses_the_whole_batch() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // The entry a batch is servable BUT FOR the row under test: it shares every
    // refused call, so a refusal is proven to have applied nothing.
    let servable = || entry(by_name(ENTITY, "Sensor"), SERIES);

    for row in unresolvable_references() {
        let case = row.case;
        let refused = emulator
            .batch_put_property_values(
                WORKSPACE,
                json!([servable(), entry((row.reference)(), row.property_name)]),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (row.status, Some(row.error)),
            "case {case:?}: expected {} {}, got {refused:?}",
            row.status,
            row.error
        );
        let reported = message(&refused);
        assert_eq!(
            reported,
            (row.expected_message)(),
            "case {case:?}: expected the message {:?}, got {reported:?}",
            (row.expected_message)()
        );
        assert_eq!(
            held_values(&emulator).await,
            Vec::<Value>::new(),
            "case {case:?}: a refused call must apply none of its entries"
        );
    }

    // A batch whose entries are unresolvable in TWO classes at once is still one
    // HTTP-level failure, taken from the first entry that does not resolve.
    let mixed = emulator
        .batch_put_property_values(
            WORKSPACE,
            json!([
                entry(by_name(ENTITY, "NoSuchComponent"), SERIES),
                entry(by_name("ghost-9", "Sensor"), SERIES),
            ]),
        )
        .await;
    assert_eq!(
        (mixed.status, mixed.error_type.as_deref()),
        (400, Some("ValidationException")),
        "case \"a batch mixing a missing component with a missing entity\": expected one \
         HTTP-level failure of the first entry's class, got {mixed:?}"
    );

    // What the references NAME is resolved for the whole batch BEFORE what they
    // resolved to is judged writable, so an entry addressing an unwritable
    // property does not mask an unresolvable reference behind it.
    let ordered = emulator
        .batch_put_property_values(
            WORKSPACE,
            json!([
                entry(by_name(UNWRITABLE_ENTITY, "Sensor"), SERIES),
                entry(by_name(ENTITY, "NoSuchComponent"), SERIES),
            ]),
        )
        .await;
    let expected_reference_message = format!(
        "Component NoSuchComponent of Entity {ENTITY} in workspace {WORKSPACE} not found."
    );
    assert_eq!(
        (
            ordered.status,
            ordered.error_type.as_deref(),
            message(&ordered)
        ),
        (
            400,
            Some("ValidationException"),
            expected_reference_message.clone()
        ),
        "case \"an unwritable property before an unresolvable reference\": expected \
         {expected_reference_message:?}, got {ordered:?}"
    );

    // The same reference, resolvable, is served — so the refusals above are
    // about the reference and not about the operation.
    let written = emulator
        .batch_put_property_values(WORKSPACE, json!([servable()]))
        .await;
    assert_eq!(
        (written.status, written.member("errorEntries").clone()),
        (200, json!([])),
        "a resolvable reference must be served, got {written:?}"
    );
    assert_eq!(
        held_values(&emulator).await,
        vec![json!({"time": T0, "value": {"doubleValue": 1.0}})],
        "the served write must be readable back"
    );
}

/// RULE: once an entry's reference HAS resolved, what is wrong with that entry's
/// values is that entry's own failure, reported in `errorEntries` beside the
/// entries the call applied — the one thing this operation answers 200 with a
/// partial result for.
#[tokio::test]
async fn an_unrecordable_value_is_reported_per_entry() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // (case, the values the entry carries) — every row's reference resolves, so
    // what fails is the value.
    let rows: Vec<(&str, Value)> = vec![
        (
            "a value of another data type than the property's",
            json!([{"time": T0, "value": {"stringValue": "not-a-double"}}]),
        ),
        (
            "a value carrying neither time nor timestamp",
            json!([{"value": {"doubleValue": 1.0}}]),
        ),
        (
            "a value whose time is not an ISO8601 DateTime",
            json!([{"time": "not-a-timestamp-at-all", "value": {"doubleValue": 1.0}}]),
        ),
    ];

    for (case, values) in rows {
        let written = emulator
            .batch_put_property_values(
                WORKSPACE,
                json!([
                    entry(by_name(COMPOSITE_ENTITY, "Assembly"), SERIES),
                    entry_with(by_name(ENTITY, "Sensor"), SERIES, values),
                ]),
            )
            .await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: an unrecordable value is reported per entry, not by status, \
             got {written:?}"
        );
        let codes: Vec<String> = written
            .member("errorEntries")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .flat_map(|reported| reported["errors"].as_array().cloned().unwrap_or_default())
            .map(|error| {
                error["errorCode"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            codes,
            vec!["ValidationException".to_string()],
            "case {case:?}: expected one reported entry error, got {written:?}"
        );
        assert_eq!(
            held_values(&emulator).await,
            Vec::<Value>::new(),
            "case {case:?}: the failed entry must store nothing"
        );
    }
}

/// RULE: a component type the workspace does not hold is the modelled
/// `ResourceNotFoundException` (HTTP 404) carrying ONE construction — "Cannot
/// find component type <componentTypeId> in workspace <workspaceId>." — whichever
/// operation looked for it and whatever the id looks like.
#[tokio::test]
async fn a_missing_component_type_reads_the_same_wherever_it_is_looked_for() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace(WORKSPACE).await.status,
        200,
        "the workspace the cases look in must be created"
    );
    // One component type that DID exist and was deleted, so the rule is checked
    // on a miss the caller created itself as well as on ids never created.
    let deleted = "com.example.deletedType";
    assert_eq!(
        emulator
            .create_component_type(WORKSPACE, deleted, json!({}))
            .await
            .status,
        200,
        "the component type the delete case removes must be created"
    );
    assert_eq!(
        emulator
            .delete_component_type(WORKSPACE, deleted)
            .await
            .status,
        200,
        "the delete of that component type must be served"
    );

    // The forms a `componentTypeId` takes: the modelled `ComponentTypeId` is
    // `@length(1..256)` and `^[a-zA-Z_\.\-0-9:]+$`
    // (`smithy_query(query="operation",operation="GetComponentType")`), so the
    // rows run from a single character to the 256 it caps at, through the
    // dotted, dashed and colon-separated forms the pattern admits.
    let ids: Vec<(&str, String)> = vec![
        ("dotted", "com.example.absent".to_string()),
        ("single-character", "a".to_string()),
        ("dashed-and-underscored", "com.example-absent_2".to_string()),
        ("colon-separated", "com:example:absent".to_string()),
        ("longest", format!("com.{}", "z".repeat(252))),
        ("already-deleted", deleted.to_string()),
    ];

    for (id_case, component_type_id) in &ids {
        let expected = format!(
            "Cannot find component type {component_type_id} in workspace {WORKSPACE}."
        );
        let reads: Vec<(&str, Resp)> = vec![
            (
                "GetComponentType",
                emulator
                    .get_component_type(WORKSPACE, component_type_id)
                    .await,
            ),
            (
                "UpdateComponentType",
                emulator
                    .update_component_type(
                        WORKSPACE,
                        component_type_id,
                        json!({"propertyDefinitions": {"Pressure": {"dataType": {"type": "DOUBLE"}}}}),
                    )
                    .await,
            ),
            (
                "DeleteComponentType",
                emulator
                    .delete_component_type(WORKSPACE, component_type_id)
                    .await,
            ),
        ];
        for (operation, refused) in reads {
            let case = format!("{operation} on a {id_case} id");
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?}: expected 404 ResourceNotFoundException, got {refused:?}"
            );
            let reported = message(&refused);
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the message {expected:?}, got {reported:?}"
            );
        }
    }
}

/// RULE: a delete refused because something still references the component type
/// names the IDS of the resources that have to go first, as a bracketed
/// comma-separated list, and asks for them to be deleted — nothing else of the
/// service's state is named, and the component type is left in place.
#[tokio::test]
async fn an_in_use_refusal_names_the_resources_to_delete_first() {
    // (case, the entities to create and the component they hold of the target,
    // the component types that reference the target, the expected message tail)
    struct InUse {
        case: &'static str,
        /// Entity ids holding a top-level component of the target.
        holders: &'static [&'static str],
        /// Entity ids holding the target through a COMPOSITE component.
        composite_holders: &'static [&'static str],
        /// Component type ids extending the target.
        extenders: &'static [&'static str],
    }

    let target = "com.example.target";
    let rows: [InUse; 5] = [
        InUse {
            case: "one entity",
            holders: &["holder-1"],
            composite_holders: &[],
            extenders: &[],
        },
        InUse {
            case: "three entities",
            holders: &["holder-1", "holder-2", "holder-3"],
            composite_holders: &[],
            extenders: &[],
        },
        InUse {
            case: "one entity through a composite component",
            holders: &[],
            composite_holders: &["holder-4"],
            extenders: &[],
        },
        InUse {
            case: "an entity and a component type at once",
            holders: &["holder-5"],
            composite_holders: &[],
            extenders: &["com.example.child"],
        },
        InUse {
            case: "a component type only",
            holders: &[],
            composite_holders: &[],
            extenders: &["com.example.child"],
        },
    ];

    for row in rows {
        let case = row.case;
        // A workspace of its own per row, so one row's references cannot block
        // the next row's delete.
        let emulator = Emulator::new();
        let workspace = format!("inUse{}", case.replace(' ', ""));
        assert_eq!(
            emulator.create_workspace(&workspace).await.status,
            200,
            "case {case:?}: the workspace must be created"
        );
        assert_eq!(
            emulator
                .create_component_type(&workspace, target, json!({}))
                .await
                .status,
            200,
            "case {case:?}: the referenced component type must be created"
        );
        // The holder type is what a composite reference is reached through.
        let holder_type = "com.example.holder";
        if !row.composite_holders.is_empty() {
            assert_eq!(
                emulator
                    .create_component_type(
                        &workspace,
                        holder_type,
                        json!({"compositeComponentTypes": {"inner": {"componentTypeId": target}}}),
                    )
                    .await
                    .status,
                200,
                "case {case:?}: the holding component type must be created"
            );
        }
        for entity_id in row.holders {
            let created = emulator
                .create_entity(
                    &workspace,
                    json!({
                        "entityId": entity_id,
                        "entityName": entity_id,
                        "components": {"instance": {"componentTypeId": target}},
                    }),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: the entity {entity_id:?} must be created, got {created:?}"
            );
        }
        for entity_id in row.composite_holders {
            let created = emulator
                .create_entity(
                    &workspace,
                    json!({
                        "entityId": entity_id,
                        "entityName": entity_id,
                        "components": {"Assembly": {"componentTypeId": holder_type}},
                        "compositeComponents": {"Assembly/inner": {}},
                    }),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: the entity {entity_id:?} must be created, got {created:?}"
            );
        }
        for component_type_id in row.extenders {
            let created = emulator
                .create_component_type(
                    &workspace,
                    component_type_id,
                    json!({"extendsFrom": [target]}),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: the extending component type must be created, got {created:?}"
            );
        }

        // Entities come first where both kinds reference the type, so the caller
        // is asked for one kind at a time.
        let mut blocking: Vec<&str> = row.holders.to_vec();
        blocking.extend(row.composite_holders.iter().copied());
        blocking.sort_unstable();
        let expected = if blocking.is_empty() {
            format!(
                "Component type {target} is being used by component types [{}], please \
                 delete the component types first.",
                row.extenders.join(", ")
            )
        } else {
            format!(
                "Component type {target} is being used by entities [{}], please delete the \
                 entities first.",
                blocking.join(", ")
            )
        };

        let refused = emulator.delete_component_type(&workspace, target).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected 400 ValidationException, got {refused:?}"
        );
        let reported = message(&refused);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the message {expected:?}, got {reported:?}"
        );
        // The refusal changed nothing: the component type is still there.
        assert_eq!(
            emulator
                .get_component_type(&workspace, target)
                .await
                .status,
            200,
            "case {case:?}: a refused delete must leave the component type in place"
        );
    }
}

/// RULE: the property data plane resolves a reference outside in, so a missing
/// ENTITY and a component an existing entity does not have are DIFFERENT
/// refusals — the former the modelled `ResourceNotFoundException` (HTTP 404)
/// naming the entity, the latter the modelled `ValidationException` (HTTP 400)
/// naming the component — on every operation that addresses a property through
/// one.
#[tokio::test]
async fn the_data_plane_tells_a_missing_entity_from_a_missing_component() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // (case, the members addressing the component, the refusal expected)
    let rows: Vec<(&str, Value, u16, &str, String)> = vec![
        (
            "an entityId the workspace does not hold",
            by_name("ghost-9", "Sensor"),
            404,
            "ResourceNotFoundException",
            format!("Entity ghost-9 in workspace {WORKSPACE} not found."),
        ),
        (
            "an entityId in the uuid form the model admits, unheld",
            by_name("a793021f-afb7-4515-adc7-853db7e4048f", "Sensor"),
            404,
            "ResourceNotFoundException",
            format!(
                "Entity a793021f-afb7-4515-adc7-853db7e4048f in workspace {WORKSPACE} not \
                 found."
            ),
        ),
        (
            "a componentName the entity does not have",
            by_name(ENTITY, "NoSuchComponent"),
            400,
            "ValidationException",
            format!(
                "Component NoSuchComponent of Entity {ENTITY} in workspace {WORKSPACE} not \
                 found."
            ),
        ),
        (
            "a componentName differing from the entity's only in case",
            by_name(ENTITY, "sensor"),
            400,
            "ValidationException",
            format!(
                "Component sensor of Entity {ENTITY} in workspace {WORKSPACE} not found."
            ),
        ),
        (
            "a componentPath no component of the entity sits at",
            json!({"entityId": COMPOSITE_ENTITY, "componentPath": "Assembly/absent"}),
            400,
            "ValidationException",
            format!(
                "Component Assembly/absent of Entity {COMPOSITE_ENTITY} in workspace \
                 {WORKSPACE} not found."
            ),
        ),
    ];

    for (case, address, status, error, expected) in rows {
        let mut value_request = address.clone();
        value_request["selectedProperties"] = json!([SERIES]);
        let read = emulator
            .get_property_value(WORKSPACE, value_request.clone())
            .await;
        assert_eq!(
            (read.status, read.error_type.as_deref(), message(&read)),
            (status, Some(error), expected.clone()),
            "case {case:?}: GetPropertyValue expected {status} {error} {expected:?}, \
             got {read:?}"
        );

        let history = emulator
            .get_property_value_history(WORKSPACE, value_request)
            .await;
        assert_eq!(
            (
                history.status,
                history.error_type.as_deref(),
                message(&history)
            ),
            (status, Some(error), expected.clone()),
            "case {case:?}: GetPropertyValueHistory expected {status} {error} \
             {expected:?}, got {history:?}"
        );

        let written = emulator
            .batch_put_property_values(WORKSPACE, json!([entry(address, SERIES)]))
            .await;
        assert_eq!(
            (
                written.status,
                written.error_type.as_deref(),
                message(&written)
            ),
            (status, Some(error), expected.clone()),
            "case {case:?}: BatchPutPropertyValues expected {status} {error} \
             {expected:?}, got {written:?}"
        );
    }
}

/// The keys of a tag batch of `count` entries, prefixed so two batches cannot
/// share a key.
fn tag_batch(count: usize, prefix: &str) -> Value {
    let mut tags = serde_json::Map::new();
    for index in 0..count {
        tags.insert(format!("{prefix}-{index:02}"), json!("v"));
    }
    Value::Object(tags)
}

/// RULE: the per-CALL bound on the request's own tag map is a different failure
/// from the per-resource quota — a request naming more tags than `TagMap` admits
/// is outside the shape itself, so it is refused for its own size (HTTP 400),
/// whatever the resource carries, and nothing it named is stored.
///
/// The bound is a modelled `@length` on the request shape, so the refusal is the
/// one the centralized request walk states for ANY boundary breach of this
/// service, in its validation envelope, before a handler runs: a per-operation
/// code for it would have to be chosen by that shared walk rather than by this
/// operation, which is beyond what this fix changes. What is asserted here is
/// therefore the distinction the quota refusal rests on — this call is refused
/// for its SIZE and stores nothing, where the quota refusal below is about the
/// set the resource would be left holding.
#[tokio::test]
async fn a_request_past_the_per_call_tag_bound_is_refused() {
    let emulator = Emulator::new();
    let workspace = "percallbound";
    assert_eq!(
        emulator.create_workspace(workspace).await.status,
        200,
        "the workspace must be created"
    );
    let arn = workspace_arn(workspace);

    // The row at the bound is served, so the refusal below is about the size and
    // not about the call.
    let served = emulator
        .tag_resource(&arn, tag_batch(MAX_TAGS_PER_CALL, "within"))
        .await;
    assert_eq!(
        served.status, 200,
        "a request of exactly the tags the map admits must be served, got {served:?}"
    );
    assert_eq!(
        emulator.untag_resource(&arn, &["within-00"]).await.status,
        200,
        "making room for the over-sized request must be served"
    );

    let over = tag_batch(MAX_TAGS_PER_CALL + 1, "over");
    let refused = emulator.tag_resource(&arn, over.clone()).await;
    assert_eq!(
        (refused.status, refused.error_type.as_deref()),
        (400, Some("ValidationException")),
        "a request past the per-call bound is refused for its own size, got {refused:?}"
    );
    let listed = emulator
        .list_tags_for_resource(&arn, json!({"maxResults": 200}))
        .await;
    let carried = listed.member("tags").clone();
    for key in over.as_object().into_iter().flat_map(|tags| tags.keys()) {
        assert!(
            carried[key].is_null(),
            "the refused request must store none of its tags, got {carried:?}"
        );
    }
}

/// RULE: a `TagResource` call whose tags, added to the ones the resource already
/// carries, would carry it past the per-resource tag QUOTA
/// ([`MAX_TAGS_PER_RESOURCE`]) is refused as the quota breach it is — HTTP 402
/// `ServiceQuotaExceededException` with an empty message — stores nothing, and
/// becomes servable once the resource has room again. A request INSIDE the
/// per-resource quota is served however many tags it carries, up to the bound its
/// own tag map states.
#[tokio::test]
async fn tagging_past_the_per_resource_quota_is_a_quota_refusal() {
    // (case, tags already carried, tags the call applies, whether it is served)
    // The rows differ along how full the resource is and how many keys the call
    // adds: both sides of the quota boundary, one past it and far past it.
    let rows: Vec<(&str, usize, usize, bool)> = vec![
        ("fortyThenFifteen", 40, 15, false),
        ("atTheQuotaPlusOne", MAX_TAGS_PER_RESOURCE, 1, false),
        ("twoBelowPlusThree", MAX_TAGS_PER_RESOURCE - 2, 3, false),
        ("exactlyTheQuota", MAX_TAGS_PER_RESOURCE - 15, 15, true),
        ("oneBelowTheQuota", MAX_TAGS_PER_RESOURCE - 16, 15, true),
    ];

    for (case, stored, applied, served) in rows {
        let emulator = Emulator::new();
        let workspace = format!("quota{case}");
        assert_eq!(
            emulator.create_workspace(&workspace).await.status,
            200,
            "case {case:?}: the workspace must be created"
        );
        let arn = workspace_arn(&workspace);
        let first = emulator.tag_resource(&arn, tag_batch(stored, "first")).await;
        assert_eq!(
            first.status, 200,
            "case {case:?}: storing the first {stored} tags must be served, got {first:?}"
        );

        let second = tag_batch(applied, "second");
        let resp = emulator.tag_resource(&arn, second.clone()).await;
        let listed = emulator.list_tags_for_resource(&arn, json!({"maxResults": 200})).await;
        let carried = listed.member("tags").clone();
        if served {
            assert_eq!(
                resp.status, 200,
                "case {case:?}: a call inside the quota must be served, got {resp:?}"
            );
            for key in second.as_object().into_iter().flat_map(|tags| tags.keys()) {
                assert!(
                    !carried[key].is_null(),
                    "case {case:?}: the served call must store {key:?}, got {carried:?}"
                );
            }
            continue;
        }

        assert_eq!(
            (resp.status, resp.error_type.as_deref()),
            (402, Some("ServiceQuotaExceededException")),
            "case {case:?}: expected 402 ServiceQuotaExceededException, got {resp:?}"
        );
        assert_eq!(
            message(&resp),
            String::new(),
            "case {case:?}: the quota refusal names no value of the request, got {resp:?}"
        );
        for key in second.as_object().into_iter().flat_map(|tags| tags.keys()) {
            assert!(
                carried[key].is_null(),
                "case {case:?}: the refused call must store none of its tags, got {carried:?}"
            );
        }

        // Make room, and the very same call is served: the refusal is about the
        // resource's cumulative count, not about the request.
        let removed: Vec<String> = tag_batch(stored, "first")
            .as_object()
            .map(|tags| tags.keys().take(applied + 1).cloned().collect())
            .unwrap_or_default();
        let keys: Vec<&str> = removed.iter().map(String::as_str).collect();
        let untagged = emulator.untag_resource(&arn, &keys).await;
        assert_eq!(
            untagged.status, 200,
            "case {case:?}: untagging must be served, got {untagged:?}"
        );
        let retried = emulator.tag_resource(&arn, second).await;
        assert_eq!(
            retried.status, 200,
            "case {case:?}: the same call must be served once the resource has room, \
             got {retried:?}"
        );
    }
}
