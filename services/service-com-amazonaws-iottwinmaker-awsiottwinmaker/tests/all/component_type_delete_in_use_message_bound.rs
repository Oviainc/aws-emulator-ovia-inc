// SPDX-License-Identifier: CC0-1.0

//! The refusal `DeleteComponentType` answers a still-referenced component type
//! with, measured against the member it travels on, end to end over HTTP.
//!
//! RULE under test: the in-use refusal of a delete is stated within the
//! `@length` maximum of the modelled member it is reported on — `ErrorMessage`,
//! 2048 characters — however many stored references the workspace holds and
//! however long the ids of the resources carrying them are. A refusal that fits
//! is reported exactly as the service words it, naming the ids of the resources
//! that have to be deleted first; one that would overrun is reported at the
//! bound, keeping its leading text and its trailing clause so the caller still
//! learns which kind of resource blocks the delete and what to do about it.
//!
//! This is the same rule [`crate::error_message_bounds`] asserts over the
//! operations that interpolate ONE identifier into a reason; the reason here is
//! grown by the caller instead, one id per stored reference, so its rows differ
//! along the NUMBER of references as well as the size of their ids — and along
//! which kind of reference they are (an entity's component, an entity's
//! composite component, a component type that extends the type, a component
//! type that names it in its composite declarations), since the two kinds are
//! worded differently.

use serde_json::json;

use crate::error_message_bounds::{assert_within_bound, ERROR_MESSAGE_MAX};
use crate::harness::Emulator;

/// The greatest number of characters the modelled `EntityId` admits:
/// `@length(1..128)`, from `smithy_query(query="shape",shape="EntityId")`.
const ENTITY_ID_MAX: usize = 128;

/// The greatest number of characters the modelled `ComponentTypeId` admits:
/// `@length(1..256)`, from
/// `smithy_query(query="shape",shape="ComponentTypeId")`.
const COMPONENT_TYPE_ID_MAX: usize = 256;

/// The component type every case below tries to delete.
const TARGET: &str = "com.example.target";

/// The component type a composite reference is reached THROUGH: it declares the
/// composite the entities below instantiate.
const HOLDER: &str = "com.example.holder";

/// The name the holding component sits under on an entity, and the declaration
/// inside it, so a composite component's path is `Assembly/inner`.
const HOLDING_COMPONENT: &str = "Assembly";
/// See [`HOLDING_COMPONENT`].
const COMPOSITE_DECLARATION: &str = "inner";

/// Which kind of stored reference a row points at [`TARGET`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reference {
    /// An entity whose top-level component instantiates the type.
    EntityComponent,
    /// An entity whose COMPOSITE component instantiates it.
    EntityCompositeComponent,
    /// Another component type that `extendsFrom` it.
    ExtendsFrom,
    /// Another component type that names it in `compositeComponentTypes`.
    CompositeComponentType,
}

impl Reference {
    /// Whether the ids a row carries are ENTITY ids (rather than component type
    /// ids), which is what decides both how a row is seeded and which of the two
    /// refusals the service words for it.
    fn by_entities(self) -> bool {
        match self {
            Reference::EntityComponent | Reference::EntityCompositeComponent => true,
            Reference::ExtendsFrom | Reference::CompositeComponentType => false,
        }
    }

    /// The greatest number of characters an id of the resource carrying this
    /// kind of reference may have.
    fn id_max(self) -> usize {
        if self.by_entities() {
            ENTITY_ID_MAX
        } else {
            COMPONENT_TYPE_ID_MAX
        }
    }

    /// The text every id of a row is built on: an id of the kind of resource
    /// this reference lives on, admitted by that resource's modelled pattern
    /// (`EntityId` starts and ends alphanumeric; `ComponentTypeId` admits
    /// letters, digits and `_.-:`).
    fn id_prefix(self) -> &'static str {
        if self.by_entities() {
            "ent"
        } else {
            "com.example.r"
        }
    }

    /// The refusal the service words for `ids` referencing `TARGET` this way, in
    /// full — before the bound of the member it travels on is applied.
    ///
    /// Entities are asked for first where both kinds reference the type, so a
    /// caller is asked for one kind at a time; nothing else of the service's
    /// state is named.
    fn whole(self, ids: &[String]) -> String {
        if self.by_entities() {
            format!(
                "Component type {TARGET} is being used by entities [{}], please delete the \
                 entities first.",
                ids.join(", ")
            )
        } else {
            format!(
                "Component type {TARGET} is being used by component types [{}], please delete \
                 the component types first.",
                ids.join(", ")
            )
        }
    }

    /// Seed a workspace in which every id of `ids` references [`TARGET`] this
    /// way.
    async fn seed(self, emulator: &Emulator, workspace: &str, case: &str, ids: &[String]) {
        if self == Reference::EntityCompositeComponent {
            let created = emulator
                .create_component_type(
                    workspace,
                    HOLDER,
                    json!({
                        "compositeComponentTypes": {
                            COMPOSITE_DECLARATION: {"componentTypeId": TARGET},
                        },
                    }),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: the holding component type must be created, got {created:?}"
            );
        }
        for id in ids {
            let created = match self {
                Reference::EntityComponent => {
                    emulator
                        .create_entity(
                            workspace,
                            json!({
                                "entityId": id,
                                "entityName": id,
                                "components": {"instance": {"componentTypeId": TARGET}},
                            }),
                        )
                        .await
                }
                Reference::EntityCompositeComponent => {
                    emulator
                        .create_entity(
                            workspace,
                            json!({
                                "entityId": id,
                                "entityName": id,
                                "components": {
                                    HOLDING_COMPONENT: {"componentTypeId": HOLDER},
                                },
                                "compositeComponents": {
                                    format!("{HOLDING_COMPONENT}/{COMPOSITE_DECLARATION}"): {},
                                },
                            }),
                        )
                        .await
                }
                Reference::ExtendsFrom => {
                    emulator
                        .create_component_type(workspace, id, json!({"extendsFrom": [TARGET]}))
                        .await
                }
                Reference::CompositeComponentType => {
                    emulator
                        .create_component_type(
                            workspace,
                            id,
                            json!({
                                "compositeComponentTypes": {
                                    COMPOSITE_DECLARATION: {"componentTypeId": TARGET},
                                },
                            }),
                        )
                        .await
                }
            };
            assert_eq!(
                created.status, 200,
                "case {case:?}: the referencing resource {id:?} must be created, got {created:?}"
            );
        }
    }
}

/// One id of `length` characters, distinguished from its siblings by `index`.
///
/// Zero-padded, so the ids of a row sort in the order their indexes were handed
/// out and the refusal's id order is a statement rather than an accident.
fn padded_id(prefix: &str, index: usize, length: usize) -> String {
    let base = format!("{prefix}{index:03}");
    assert!(
        length >= base.chars().count(),
        "an id of {length} characters cannot carry the prefix {base:?}"
    );
    let mut id = base;
    while id.chars().count() < length {
        id.push('x');
    }
    id
}

/// `count` ids of `length` characters each.
fn ids_of_size(reference: Reference, count: usize, length: usize) -> Vec<String> {
    (0..count)
        .map(|index| padded_id(reference.id_prefix(), index, length))
        .collect()
}

/// Ids whose refusal renders EXACTLY `target_length` characters.
///
/// Derived from the bound and from the fixed text the service words around the
/// ids — never from a counted literal — so a row can land ON the member's
/// maximum and one character past it whatever that fixed text is: every id but
/// the last carries `step` characters, and the last carries whatever the budget
/// leaves once the ", " between ids is paid for.
fn ids_rendering(reference: Reference, target_length: usize) -> Vec<String> {
    let fixed = reference.whole(&[]).chars().count();
    assert!(
        target_length > fixed,
        "a refusal naming no id already renders {fixed} characters, so it cannot render \
         {target_length}"
    );
    // What the ids themselves and the separators between them have to add up to.
    let budget = target_length - fixed;
    // A step short enough that the characters one id's absence frees
    // (`step` + the two of its separator) always leave the last id a length the
    // model admits.
    let step = reference.id_max() * 3 / 4;
    let mut count = 1;
    while budget > (step + 2) * (count - 1) + reference.id_max() {
        count += 1;
    }
    let last = budget - (step + 2) * (count - 1);
    let mut ids: Vec<String> = (0..count - 1)
        .map(|index| padded_id(reference.id_prefix(), index, step))
        .collect();
    ids.push(padded_id(reference.id_prefix(), count - 1, last));
    // The row is only a statement about the bound if it really lands on it.
    let rendered = reference.whole(&ids).chars().count();
    assert_eq!(
        rendered, target_length,
        "the ids of a row must render a refusal of {target_length} characters, got {rendered}"
    );
    ids
}

/// One workspace's worth of references pointed at [`TARGET`].
#[derive(Debug)]
struct Row {
    case: String,
    reference: Reference,
    ids: Vec<String>,
}

/// The rows the rule is asserted over: they differ along the KIND of stored
/// reference, the NUMBER of references and the SIZE of the ids carrying them —
/// including a row whose refusal lands exactly on the member's maximum, one a
/// single character past it, and the scale a workspace of referencing entities
/// reaches.
fn rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for reference in [
        Reference::EntityComponent,
        Reference::EntityCompositeComponent,
        Reference::ExtendsFrom,
        Reference::CompositeComponentType,
    ] {
        rows.push(Row {
            case: format!("{reference:?}: one reference with a short id"),
            reference,
            ids: ids_of_size(reference, 1, reference.id_prefix().chars().count() + 3),
        });
        rows.push(Row {
            case: format!("{reference:?}: one reference with an id at the modelled maximum"),
            reference,
            ids: ids_of_size(reference, 1, reference.id_max()),
        });
        rows.push(Row {
            case: format!(
                "{reference:?}: references whose refusal renders exactly the characters the \
                 member admits"
            ),
            reference,
            ids: ids_rendering(reference, ERROR_MESSAGE_MAX),
        });
        rows.push(Row {
            case: format!(
                "{reference:?}: references whose refusal renders one character past the \
                 member's maximum"
            ),
            reference,
            ids: ids_rendering(reference, ERROR_MESSAGE_MAX + 1),
        });
        rows.push(Row {
            case: format!(
                "{reference:?}: thirty references with ids at the modelled maximum"
            ),
            reference,
            ids: ids_of_size(reference, 30, reference.id_max()),
        });
    }
    rows
}

/// RULE: `DeleteComponentType` refuses a component type something stored still
/// references within the 2048 characters the modelled `ErrorMessage` admits,
/// however many references the workspace holds and whatever their ids are —
/// worded whole where it fits, and at the bound with its leading text and
/// trailing clause kept where it does not.
#[tokio::test]
async fn the_in_use_refusal_fits_its_modelled_member() {
    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case.as_str();
        // A workspace of its own per row, so one row's references cannot block
        // the next row's delete.
        let emulator = Emulator::new();
        let workspace = format!("inUseBound{index}");
        let created = emulator.create_workspace(&workspace).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the workspace must be created, got {created:?}"
        );
        let created = emulator
            .create_component_type(&workspace, TARGET, json!({}))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the referenced component type must be created, got {created:?}"
        );
        row.reference
            .seed(&emulator, &workspace, case, &row.ids)
            .await;

        // The service names the referencing resources in id order, each once.
        let mut expected_ids = row.ids.clone();
        expected_ids.sort();
        let whole = row.reference.whole(&expected_ids);

        let refused = emulator.delete_component_type(&workspace, TARGET).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected 400 ValidationException, got {refused:?}"
        );
        let reported = refused
            .member("message")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_within_bound(case, &reported, &whole, ERROR_MESSAGE_MAX);

        // The refusal changed nothing: the component type the delete was
        // refused for is still there, whatever the size of the refusal.
        let read = emulator.get_component_type(&workspace, TARGET).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: a refused delete must leave the component type in place, got {read:?}"
        );
    }
}

/// RULE: the bound applies to the refusal the service states, not to what it
/// knows: once the references are gone the delete succeeds, so a refusal cut to
/// the bound is never the service losing track of them.
#[tokio::test]
async fn a_refusal_cut_to_the_bound_still_clears_once_the_references_go() {
    let emulator = Emulator::new();
    let workspace = "inUseBoundCleared";
    assert_eq!(
        emulator.create_workspace(workspace).await.status,
        200,
        "the workspace must be created"
    );
    assert_eq!(
        emulator
            .create_component_type(workspace, TARGET, json!({}))
            .await
            .status,
        200,
        "the referenced component type must be created"
    );
    // Enough referencing entities, at the modelled id maximum, that the refusal
    // naming all of them cannot fit the member it travels on.
    let ids = ids_of_size(Reference::EntityComponent, 30, ENTITY_ID_MAX);
    Reference::EntityComponent
        .seed(&emulator, workspace, "cleared", &ids)
        .await;

    let refused = emulator.delete_component_type(workspace, TARGET).await;
    assert_eq!(
        (refused.status, refused.error_type.as_deref()),
        (400, Some("ValidationException")),
        "expected 400 ValidationException while the entities stand, got {refused:?}"
    );
    let length = refused
        .member("message")
        .as_str()
        .unwrap_or_default()
        .chars()
        .count();
    assert!(
        length <= ERROR_MESSAGE_MAX,
        "the refusal is stated within the {ERROR_MESSAGE_MAX}-character maximum of the member it \
         travels on, got {length} characters: {refused:?}"
    );

    for id in &ids {
        let deleted = emulator.delete_entity(workspace, id, false).await;
        assert_eq!(
            deleted.status, 200,
            "the entity {id:?} must be deletable, got {deleted:?}"
        );
    }

    let deleted = emulator.delete_component_type(workspace, TARGET).await;
    assert_eq!(
        deleted.status, 200,
        "with every reference gone the component type must be deletable, got {deleted:?}"
    );
    let read = emulator.get_component_type(workspace, TARGET).await;
    assert_eq!(
        read.status, 404,
        "the deleted component type must be gone, got {read:?}"
    );
}
