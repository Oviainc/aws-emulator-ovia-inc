// SPDX-License-Identifier: CC0-1.0

//! Writing an entity's STORED composite component after the
//! `compositeComponentTypes` declaration it was created through has stopped
//! standing, end to end over HTTP.
//!
//! RULE under test: a `componentPath` that names a composite component the
//! entity already HOLDS addresses that stored record, so `UpdateEntity` patches
//! and removes it whatever the holding component types declare at the time of
//! the call; a declaration is what a composite component that does not exist
//! yet has to be resolved through (a `CREATE`, which is still refused when no
//! declaration on the chain supplies the segment), not what keeps a stored one
//! addressable. It is the same reading every reader of a `componentPath` uses,
//! and the same reading the reference scan uses when it counts a stored
//! composite component as an instance of the component type it names.
//!
//! The rows differ along what the rule generalises over: the DEPTH of the
//! stored composite component's path, and HOW the declaration it was created
//! through stopped standing — the holding component type's own map emptied, its
//! own map replaced by a map naming another entry, the declaring BASE type's map
//! emptied under an heir, the heir's `extendsFrom` cleared, or the
//! INTERMEDIATE type's map emptied so only the path's last segment loses its
//! declaration. A control row leaves every declaration standing, and the
//! boundary each row ends on is a re-`CREATE` of the same path, which needs the
//! declaration the stranded write did not.

use serde_json::{json, Value};

use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::STATE_DELETING;

use crate::harness::Emulator;

/// The component type the composite component under test instantiates, and the
/// property it gives it.
const TARGET: &str = "com.example.target";
/// The property [`TARGET`] defines, written by the `UPDATE` each row performs.
const PROPERTY: &str = "Pressure";
/// A component type used as the entry a replaced declaration map names INSTEAD
/// of the one the stored composite component stands on.
const SPARE: &str = "com.example.spare";
/// The intermediate type a two-deep path is resolved through: it declares
/// `inner` as an instance of [`TARGET`].
const MIDDLE: &str = "com.example.middle";
/// Declares `inner` and `middle` itself.
const HOLDER: &str = "com.example.holder";
/// Declares `inner` and `middle` for the heir that extends it.
const BASE: &str = "com.example.base";
/// Extends [`BASE`] and declares nothing of its own, so its paths resolve
/// through an INHERITED declaration.
const HEIR: &str = "com.example.heir";
/// The entity every row stands its composite components up on.
const ENTITY: &str = "holderEntity";
/// The entity's top-level component: the first segment of every path below.
const COMPONENT: &str = "Sensor";

/// How a row takes the declaration the stored composite component was created
/// through away again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Strand {
    /// Nothing is taken away: the control row.
    Keep,
    /// The holding component type's `compositeComponentTypes` is replaced by an
    /// EMPTY map (`UpdateComponentType` replaces the member whole).
    EmptyHolderMap,
    /// ... replaced by a map naming a DIFFERENT entry, so the segment is gone
    /// from a map that is not empty.
    ReplaceHolderMap,
    /// The declaration was INHERITED and the declaring base type's map is
    /// emptied.
    EmptyBaseMap,
    /// The declaration was INHERITED and the heir's `extendsFrom` is cleared, so
    /// the chain that supplied it is gone.
    ClearExtendsFrom,
    /// The INTERMEDIATE type's map is emptied, so the path's last segment is the
    /// one that loses its declaration.
    EmptyMiddleMap,
}

/// One row of the rule: a stored composite component whose declaration has
/// stopped standing in one of the ways above.
#[derive(Debug)]
struct Row {
    /// The case name, which is also the workspace the row works in.
    case: &'static str,
    /// The component type the entity's `Sensor` component instantiates.
    holder: &'static str,
    /// The `compositeComponents` the create stands up, in path order.
    created: &'static [&'static str],
    /// The path under test: the composite component that instantiates
    /// [`TARGET`].
    path: &'static str,
    /// How the declaration stops standing.
    strand: Strand,
    /// The fragment a re-`CREATE` of [`Row::path`] must be refused with once the
    /// declaration is gone, or `None` when the declaration still stands and the
    /// re-create must be accepted.
    recreate_refusal: Option<&'static str>,
}

/// Every row of the rule.
fn rows() -> Vec<Row> {
    vec![
        Row {
            // The control: the declaration stands, so this is the behaviour the
            // stranded rows must match.
            case: "declarationStanding",
            holder: HOLDER,
            created: &["Sensor/inner"],
            path: "Sensor/inner",
            strand: Strand::Keep,
            recreate_refusal: None,
        },
        Row {
            // The reported case: the holding type's map is emptied.
            case: "emptyHolderMap",
            holder: HOLDER,
            created: &["Sensor/inner"],
            path: "Sensor/inner",
            strand: Strand::EmptyHolderMap,
            recreate_refusal: Some("declares no compositeComponentTypes.inner"),
        },
        Row {
            // The map is not empty, it just no longer names this entry.
            case: "replaceHolderMap",
            holder: HOLDER,
            created: &["Sensor/inner"],
            path: "Sensor/inner",
            strand: Strand::ReplaceHolderMap,
            recreate_refusal: Some("declares no compositeComponentTypes.inner"),
        },
        Row {
            // The declaration was inherited and the declarer drops it.
            case: "emptyBaseMap",
            holder: HEIR,
            created: &["Sensor/inner"],
            path: "Sensor/inner",
            strand: Strand::EmptyBaseMap,
            recreate_refusal: Some("declares no compositeComponentTypes.inner"),
        },
        Row {
            // The declaration was inherited and the inheritance itself is cut.
            case: "clearExtendsFrom",
            holder: HEIR,
            created: &["Sensor/inner"],
            path: "Sensor/inner",
            strand: Strand::ClearExtendsFrom,
            recreate_refusal: Some("declares no compositeComponentTypes.inner"),
        },
        Row {
            // Two levels down, with the FIRST segment's declaration gone: the
            // stored record is still what the path names.
            case: "emptyHolderMapDepth2",
            holder: HOLDER,
            created: &["Sensor/middle", "Sensor/middle/inner"],
            path: "Sensor/middle/inner",
            strand: Strand::EmptyHolderMap,
            recreate_refusal: Some("declares no compositeComponentTypes.middle"),
        },
        Row {
            // Two levels down, with the LAST segment's declaration gone.
            case: "emptyMiddleMapDepth2",
            holder: HOLDER,
            created: &["Sensor/middle", "Sensor/middle/inner"],
            path: "Sensor/middle/inner",
            strand: Strand::EmptyMiddleMap,
            recreate_refusal: Some("declares no compositeComponentTypes.inner"),
        },
    ]
}

/// Create the workspace and the component types every path below is resolved
/// through.
async fn seed(emulator: &Emulator, workspace: &str) {
    let created = emulator.create_workspace(workspace).await;
    assert_eq!(
        created.status, 200,
        "workspace {workspace:?}: expected 200 from CreateWorkspace, got {created:?}"
    );
    // In reference order: `extendsFrom` and `compositeComponentTypes` may only
    // name a component type the workspace already holds.
    let declarations = json!({
        "inner": {"componentTypeId": TARGET},
        "middle": {"componentTypeId": MIDDLE},
    });
    let component_types: Vec<(&str, Value)> = vec![
        (
            TARGET,
            json!({"propertyDefinitions": {PROPERTY: {"dataType": {"type": "DOUBLE"}}}}),
        ),
        (SPARE, json!({})),
        (
            MIDDLE,
            json!({"compositeComponentTypes": {"inner": {"componentTypeId": TARGET}}}),
        ),
        (HOLDER, json!({"compositeComponentTypes": declarations})),
        (BASE, json!({"compositeComponentTypes": declarations})),
        (HEIR, json!({"extendsFrom": [BASE]})),
    ];
    for (component_type_id, body) in component_types {
        let created = emulator
            .create_component_type(workspace, component_type_id, body)
            .await;
        assert_eq!(
            created.status, 200,
            "workspace {workspace:?}: expected 200 from CreateComponentType \
             {component_type_id:?}, got {created:?}"
        );
    }
}

/// Stand the row's entity up, with the composite components its paths name.
async fn create_entity(emulator: &Emulator, row: &Row) {
    let composite_components: Value = row
        .created
        .iter()
        .map(|path| ((*path).to_string(), json!({})))
        .collect::<serde_json::Map<String, Value>>()
        .into();
    let created = emulator
        .create_entity(
            row.case,
            json!({
                "entityId": ENTITY,
                "entityName": ENTITY,
                "components": {COMPONENT: {"componentTypeId": row.holder}},
                "compositeComponents": composite_components,
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {:?}: expected 200 from CreateEntity, got {created:?}",
        row.case
    );
}

/// Take the row's declaration away, through the patch the row names.
async fn strand(emulator: &Emulator, row: &Row) {
    let patch: Option<(&str, Value)> = match row.strand {
        Strand::Keep => None,
        Strand::EmptyHolderMap => Some((row.holder, json!({"compositeComponentTypes": {}}))),
        Strand::ReplaceHolderMap => Some((
            row.holder,
            json!({"compositeComponentTypes": {"spare": {"componentTypeId": SPARE}}}),
        )),
        Strand::EmptyBaseMap => Some((BASE, json!({"compositeComponentTypes": {}}))),
        Strand::ClearExtendsFrom => Some((row.holder, json!({"extendsFrom": []}))),
        Strand::EmptyMiddleMap => Some((MIDDLE, json!({"compositeComponentTypes": {}}))),
    };
    let Some((component_type_id, body)) = patch else {
        return;
    };
    let updated = emulator
        .update_component_type(row.case, component_type_id, body)
        .await;
    assert_eq!(
        updated.status, 200,
        "case {:?}: expected 200 from UpdateComponentType {component_type_id:?}, got {updated:?}",
        row.case
    );
}

/// The composite component `GetEntity` reports under the last segment of
/// `path`, or `Value::Null` when it reports none.
async fn reported(emulator: &Emulator, workspace: &str, path: &str) -> Value {
    let read = emulator.get_entity(workspace, ENTITY).await;
    assert_eq!(
        read.status, 200,
        "GetEntity in workspace {workspace:?}: expected 200, got {read:?}"
    );
    let name = path.rsplit('/').next().unwrap_or(path);
    read.member("components")[COMPONENT]["compositeComponents"][name].clone()
}

/// RULE: a stored composite component is patched and removed through its
/// `componentPath` whatever the holding component types declare now, while a
/// path that holds no record still needs a declaration to be created at.
#[tokio::test]
async fn a_stored_composite_component_stays_writable_without_its_declaration() {
    let emulator = Emulator::new();

    for row in rows() {
        // One workspace per row, so a row's patched component types cannot
        // decide the next row's resolution.
        seed(&emulator, row.case).await;
        create_entity(&emulator, &row).await;
        strand(&emulator, &row).await;

        // The record survived the component type patch, so it is still reported.
        let before = reported(&emulator, row.case, row.path).await;
        assert_eq!(
            (
                before["componentPath"].as_str(),
                before["componentTypeId"].as_str()
            ),
            (Some(row.path), Some(TARGET)),
            "case {:?}: the stored composite component must still be reported, got {before:?}",
            row.case
        );

        // An UPDATE of the stored record is accepted and lands.
        let patched = emulator
            .update_entity(
                row.case,
                ENTITY,
                json!({"compositeComponentUpdates": {row.path: {
                    "updateType": "UPDATE",
                    "description": "patched",
                    "propertyUpdates": {PROPERTY: {"value": {"doubleValue": 12.5}}},
                }}}),
            )
            .await;
        assert_eq!(
            patched.status, 200,
            "case {:?}: a stored composite component must be updatable, got {patched:?}",
            row.case
        );
        let after = reported(&emulator, row.case, row.path).await;
        assert_eq!(
            after["description"].as_str(),
            Some("patched"),
            "case {:?}: the update must land on the stored record, got {after:?}",
            row.case
        );
        let value = emulator
            .get_property_value(
                row.case,
                json!({
                    "entityId": ENTITY,
                    "componentPath": row.path,
                    "selectedProperties": [PROPERTY],
                }),
            )
            .await;
        assert_eq!(
            value.member("propertyValues")[PROPERTY]["propertyValue"]["doubleValue"],
            json!(12.5),
            "case {:?}: the written property value must be readable back, got {value:?}",
            row.case
        );

        // ... and so is a DELETE of it, which is the only writer that can take
        // the record away short of destroying its component or its entity.
        let deleted = emulator
            .update_entity(
                row.case,
                ENTITY,
                json!({"compositeComponentUpdates": {row.path: {"updateType": "DELETE"}}}),
            )
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {:?}: a stored composite component must be removable, got {deleted:?}",
            row.case
        );
        let gone = reported(&emulator, row.case, row.path).await;
        assert!(
            gone.is_null(),
            "case {:?}: the removed composite component must be reported no more, got {gone:?}",
            row.case
        );

        // Removing it again has nothing to act on: the path now names no record,
        // so it is answered by the rule for a path that holds none — the
        // declaration's absence where there is one, and otherwise the refusal
        // for addressing something the entity does not hold.
        let again = emulator
            .update_entity(
                row.case,
                ENTITY,
                json!({"compositeComponentUpdates": {row.path: {"updateType": "DELETE"}}}),
            )
            .await;
        let message = again
            .member("message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        let expected_fragment = row.recreate_refusal.unwrap_or("which is not there");
        assert_eq!(
            (again.status, again.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {:?}: removing it twice must be refused, got {again:?}",
            row.case
        );
        assert!(
            message.contains(expected_fragment),
            "case {:?}: expected a message naming {expected_fragment:?}, got {message:?}",
            row.case
        );

        // The boundary: bringing a NEW composite component into being at the
        // same path needs the declaration, so it is accepted only while one
        // stands.
        let recreated = emulator
            .update_entity(
                row.case,
                ENTITY,
                json!({"compositeComponentUpdates": {row.path: {"updateType": "CREATE"}}}),
            )
            .await;
        match row.recreate_refusal {
            None => {
                assert_eq!(
                    recreated.status, 200,
                    "case {:?}: a create under a standing declaration must be accepted, got \
                     {recreated:?}",
                    row.case
                );
                let back = reported(&emulator, row.case, row.path).await;
                assert_eq!(
                    back["componentTypeId"].as_str(),
                    Some(TARGET),
                    "case {:?}: the re-created composite component must instantiate the \
                     declared type, got {back:?}",
                    row.case
                );
            }
            Some(fragment) => {
                assert_eq!(
                    (recreated.status, recreated.error_type.as_deref()),
                    (400, Some("ValidationException")),
                    "case {:?}: a create with no declaration must be refused, got {recreated:?}",
                    row.case
                );
                let message = recreated
                    .member("message")
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                assert!(
                    message.contains(fragment),
                    "case {:?}: expected a message naming {fragment:?}, got {message:?}",
                    row.case
                );
            }
        }
    }
}

/// RULE (what the removability is FOR): a stored composite component counts as
/// a reference to the component type it instantiates whether or not its
/// declaration still stands, so removing it through `UpdateEntity` is what
/// clears that reference and makes the component type deletable again — without
/// destroying the holding component or the whole entity.
#[tokio::test]
async fn removing_a_stranded_composite_component_frees_the_component_type() {
    let emulator = Emulator::new();
    let workspace = "strandedReference";
    seed(&emulator, workspace).await;

    let created = emulator
        .create_entity(
            workspace,
            json!({
                "entityId": ENTITY,
                "entityName": ENTITY,
                "components": {COMPONENT: {"componentTypeId": HOLDER}},
                "compositeComponents": {"Sensor/inner": {}},
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "expected 200 from CreateEntity, got {created:?}"
    );

    // Drop every declaration that names the target, so the stored composite
    // component is the only reference left to it.
    for component_type_id in [HOLDER, BASE, MIDDLE] {
        let updated = emulator
            .update_component_type(
                workspace,
                component_type_id,
                json!({"compositeComponentTypes": {}}),
            )
            .await;
        assert_eq!(
            updated.status, 200,
            "expected 200 from UpdateComponentType {component_type_id:?}, got {updated:?}"
        );
    }

    let refused = emulator.delete_component_type(workspace, TARGET).await;
    assert_eq!(
        (refused.status, refused.error_type.as_deref()),
        (400, Some("ValidationException")),
        "the stored composite component must still count as a reference, got {refused:?}"
    );

    let removed = emulator
        .update_entity(
            workspace,
            ENTITY,
            json!({"compositeComponentUpdates": {"Sensor/inner": {"updateType": "DELETE"}}}),
        )
        .await;
    assert_eq!(
        removed.status, 200,
        "the stranded composite component must be removable, got {removed:?}"
    );

    // The holding component and the entity are untouched: only the composite
    // component went.
    let read = emulator.get_entity(workspace, ENTITY).await;
    assert!(
        read.status == 200
            && read.member("components")[COMPONENT].is_object()
            && read.member("components")[COMPONENT]["compositeComponents"]
                .as_object()
                .is_none_or(|composites| composites.is_empty()),
        "the component must survive its composite component's removal, got {read:?}"
    );

    let deleted = emulator.delete_component_type(workspace, TARGET).await;
    assert_eq!(
        (deleted.status, deleted.member("state").as_str()),
        (200, Some(STATE_DELETING)),
        "expected 200 {STATE_DELETING} once nothing instantiates the type, got {deleted:?}"
    );
}
