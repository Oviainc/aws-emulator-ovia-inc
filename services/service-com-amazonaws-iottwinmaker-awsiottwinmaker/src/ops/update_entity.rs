// SPDX-License-Identifier: CC0-1.0

//! @op UpdateEntity
//! Patches a stored entity in place, so `GetEntity`, `ListEntities` and
//! `ListComponents` read back what the update wrote.
//!
//! An update PATCHES: a member the request names replaces the stored value for
//! that member, and a member it leaves out keeps what is already persisted
//! (`entityName`, `description`). The two collection members are addressed
//! per ENTRY rather than replaced whole, because each entry carries its own
//! `updateType` saying what to do with the component the key names:
//! `componentUpdates` is keyed by component name and
//! `compositeComponentUpdates` by the `componentPath` of a composite component
//! (resolved by [`crate::state::resolve_composite_component_path`], the one
//! path rule a create and an update share) — `CREATE` adds the component,
//! `UPDATE` patches the one that is there, `DELETE` removes it, and removing
//! one takes with it whatever is addressed THROUGH it: a component's composite
//! components, and the composite components declared below a composite one. An
//! entry that names no `updateType` is applied as the entity's own contents
//! make it: a create of a component the entity does not have, a patch of one it
//! does.
//!
//! Inside a component the same reading applies one level down. A
//! `propertyUpdates` entry's `PropertyUpdateType` is `DELETE` (drop the
//! property) or `RESET_VALUE` (clear its value, keep its definition) — each
//! addressing a property the component has — or `CREATE`/`UPDATE`, which both
//! write the definition and the value the entry carries whether or not the
//! property was there, since both describe the property the entry defines and a
//! property is wholly described by the entry rather than brought into being by
//! it. A `propertyGroupUpdates` entry's `PropertyGroupUpdateType` is `CREATE`,
//! `UPDATE` or `DELETE`, read the way a component's is.
//! `parentEntityUpdate` re-parents the entity (`UPDATE`, with the
//! `parentEntityId` it requires) or detaches it to the root sentinel
//! (`DELETE`).
//!
//! Both labels address an EXISTING resource, so a workspace or an entity the
//! store does not hold is a `ResourceNotFoundException` — the reading
//! `GetEntity` gives the same two labels. Every other refusal this handler
//! owns is a `ValidationException` built from the request: an `updateType`
//! outside its modelled value list, an entry whose action disagrees with what
//! the entity holds (a `CREATE` of a component, composite component or
//! property group that is there, and an `UPDATE` or `DELETE` of one that is
//! not, with a property addressed the same way by `DELETE` and `RESET_VALUE`,
//! which both act on a property that has to be there), a `componentPath` that
//! does not resolve, a component of an ABSTRACT component type
//! ([`crate::state::abstract_component_type_violation`]), a property
//! definition carrying no `dataType`, and a `parentEntityId` that names no
//! entity of the workspace, names the entity being updated, or names an entity
//! BELOW it (which would make the stored parent graph cyclic). The patch is
//! built on a COPY and committed last, so a refused request leaves the stored
//! entity exactly as it was.
//!
//! An entity's contents are bounded: a patch that would take one entity past
//! this emulator's declared ceilings on the components, composite components,
//! properties or property groups it holds is refused with the modelled
//! `ServiceQuotaExceededException` (HTTP 402) and nothing is stored, so neither
//! one huge request nor a long series of small ones can grow the store without
//! bound (`crate::state::entity_contents_over_ceiling`, the ceiling rule
//! `CreateEntity` applies to the record it writes).
//!
//! The call answers the non-terminal `UPDATING` and the CLOCK settles the
//! entity back in `ACTIVE` once [`crate::state::ENTITY_SETTLE_SPAN`] has
//! elapsed; `updateDateTime` is the instant of the call that made the change.
//!
//! Two modelled errors are deliberately unreachable: authorization is not
//! emulated (`AccessDeniedException`) and no rate state is kept
//! (`ThrottlingException`). `ConflictException` is not produced either: one
//! call at a time holds the store, so no concurrent change can conflict with
//! this one.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `UpdateEntity::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdateEntity`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/smithy-model/com-amazonaws-iottwinmaker-awsiottwinmaker.json")
//! ```
//!
//! Updates an entity.
//!
//! ## Input
//!
//! * `componentUpdates` (ComponentUpdatesMapRequest) — An object that maps strings to the
//!   component updates in the request. Each string in the mapping must be unique to this
//!   object.
//! * `compositeComponentUpdates` (CompositeComponentUpdatesMapRequest) — This is an object
//!   that maps strings to compositeComponent updates in the request. Each key of the map
//!   represents the componentPath of the compositeComponent.
//! * `description` (Description, len 0..2048, pattern .*) — The description of the entity.
//! * `entityId` (EntityId, len 1..128, pattern
//!   ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|^[a-zA-Z0-9][a-zA-Z_\-0-9.:]*[a-zA-Z0-9]+$)
//!   **required** — The ID of the entity.
//! * `entityName` (EntityName, len 1..256, pattern ^[^\u0000-\u001F\u007F]+$) — The name of
//!   the entity.
//! * `parentEntityUpdate` (ParentEntityUpdateRequest) — An object that describes the update
//!   request for a parent entity.
//! * `workspaceId` (Id, len 1..128, pattern ^[a-zA-Z_0-9][a-zA-Z_\-0-9]*[a-zA-Z0-9]+$)
//!   **required** — The ID of the workspace that contains the entity.
//!
//! ## Output
//!
//! * `state` (State, one of: ACTIVE | CREATING | DELETING | ERROR | UPDATING) **required** —
//!   The current state of the entity update.
//! * `updateDateTime` (Timestamp) **required** — The date and time when the entity was last
//!   updated.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — Access is denied.
//! * `ConflictException` (HTTP 409) — A conflict occurred.
//! * `InternalServerException` (HTTP 500) — An unexpected error has occurred.
//! * `ResourceNotFoundException` (HTTP 404) — The resource wasn't found.
//! * `ServiceQuotaExceededException` (HTTP 402) — The service quota was exceeded.
//! * `ThrottlingException` (HTTP 429) — The rate exceeds the limit.
//! * `ValidationException` (HTTP 400) — Failed

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateEntityError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-iottwinmaker-awsiottwinmaker.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateEntityError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.iottwinmaker#UpdateEntity")]
#[derive(Debug)]
pub enum UpdateEntityErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_entity::UpdateEntityError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateEntityError = crate::scaffolded::Scaffolded<UpdateEntityErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateEntityErrorModeled> for UpdateEntityError {
    fn from(modeled: UpdateEntityErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateEntity` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateEntity<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateEntity<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateEntity<Eng>
{
    type Input = UpdateEntityInput;
    type Output = UpdateEntityOutput;
    type Error = UpdateEntityError;

    const NAME: &'static str = "UpdateEntity";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_ENTITY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_ENTITY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Every call is recorded, successful or refused: the real service logs
        // control-plane access, and this one mutates tenant state.
        ::tracing::info!(
            operation = "UpdateEntity",
            workspace_id = %input.workspace_id,
            entity_id = %input.entity_id,
            "serving a request"
        );
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            UpdateEntityErrorModeled::InternalServerException(InternalServerException {
                message: Some("the iottwinmaker store is unavailable".to_string()),
            })
        })?;
        let mut state = state.lock();

        // Both labels address an EXISTING resource, so neither naming something
        // the store does not hold is anything but a 404 — the same reading
        // `GetEntity` gives the same two labels.
        let Some(workspace) = state.workspaces.get(&input.workspace_id) else {
            return Err(UpdateEntityErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(crate::state::workspace_not_found_message(
                        &input.workspace_id,
                    )),
                },
            )
            .into());
        };
        let Some(stored) = workspace.entities.get(&input.entity_id) else {
            return Err(UpdateEntityErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(crate::state::entity_not_found_message(
                        &input.workspace_id,
                        &input.entity_id,
                    )),
                },
            )
            .into());
        };

        let now = ::std::time::SystemTime::now();
        // The patch is built on a COPY, so a request refused below leaves the
        // stored record exactly as it was.
        let mut updated = stored.clone();

        // The two scalar members: one the request names replaces the stored
        // value, one it leaves out keeps it.
        if let Some(entity_name) = input.entity_name {
            updated.entity_name = entity_name;
        }
        if let Some(description) = input.description {
            updated.description = description;
        }

        // The component entries are applied in component-NAME order and the
        // composite ones in component-PATH order (both maps are collected into
        // a `BTreeMap`), so which entry a refusal names does not depend on the
        // request map's iteration order. The composite entries are applied
        // AFTER the top-level ones, because a composite component's path is
        // resolved against the entity's components: a component this very
        // request creates can hold one.
        for (component_name, update) in sorted(input.component_updates) {
            apply_component_update(workspace, &mut updated, &component_name, update)?;
        }
        for (component_path, update) in sorted(input.composite_component_updates) {
            apply_composite_component_update(workspace, &mut updated, &component_path, update)?;
        }

        if let Some(parent_entity_update) = input.parent_entity_update {
            updated.parent_entity_id = updated_parent_entity_id(
                workspace,
                &input.workspace_id,
                &input.entity_id,
                parent_entity_update,
            )?;
        }

        // The emulator's declared ceilings on ONE entity's contents, checked
        // over the whole patched record (`crate::state::entity_contents_over_ceiling`),
        // so neither one huge request nor a long series of small ones can grow
        // an entity without bound. Checked AFTER every other rule and BEFORE
        // anything is committed.
        if let Some(message) = crate::state::entity_contents_over_ceiling(&updated) {
            return Err(UpdateEntityErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException {
                    message: Some(message),
                },
            )
            .into());
        }

        // The change has landed on the copy, so its `updateDateTime` moves to
        // this call's instant and the entity reports the non-terminal state an
        // update in flight is in; the clock, not a caller, settles it back in
        // `ACTIVE`.
        updated.begin_update(now);
        let output = UpdateEntityOutput {
            update_date_time: crate::state::timestamp(updated.update_date_time),
            state: updated.state_at(now).to_string(),
        };

        // Commit LAST: the record is re-borrowed mutably here — the lookups
        // that validated the request held the workspace immutably, so the
        // component types a component update names could be resolved against it
        // — and nothing past this point can fail.
        let Some(stored) = state
            .workspaces
            .get_mut(&input.workspace_id)
            .and_then(|workspace| workspace.entities.get_mut(&input.entity_id))
        else {
            return Err(UpdateEntityErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some(format!(
                        "entity {} of workspace {} vanished",
                        input.entity_id, input.workspace_id
                    )),
                },
            )
            .into());
        };
        *stored = updated;

        Ok(output)
    }
}

/// A requested map in KEY order, so which entry a refusal names does not depend
/// on the request map's iteration order.
fn sorted<T>(
    requested: Option<::std::collections::HashMap<String, T>>,
) -> ::std::collections::BTreeMap<String, T> {
    requested.unwrap_or_default().into_iter().collect()
}

/// What one entry of a request map asks for: the entry's `updateType`, read as
/// the action to take on the thing its key names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    /// Add the component, composite component or property group the key names.
    Create,
    /// Patch the one the entity already holds.
    Update,
    /// Remove it.
    Delete,
}

/// The `ComponentUpdateType`, `PropertyGroupUpdateType` and
/// `PropertyUpdateType` values this handler reads.
///
/// Each is one entry of the modelled value lists in `ops::types`
/// (`COMPONENT_UPDATE_TYPE_VALUES`, `PROPERTY_GROUP_UPDATE_TYPE_VALUES`,
/// `PROPERTY_UPDATE_TYPE_VALUES`); a value outside the list the member targets
/// is refused by [`unknown_update_type`], which names that list.
const UPDATE_TYPE_CREATE: &str = "CREATE";
/// See [`UPDATE_TYPE_CREATE`].
const UPDATE_TYPE_UPDATE: &str = "UPDATE";
/// See [`UPDATE_TYPE_CREATE`].
const UPDATE_TYPE_DELETE: &str = "DELETE";
/// See [`UPDATE_TYPE_CREATE`]. `PropertyUpdateType` only: a property keeps its
/// definition and loses its value.
const UPDATE_TYPE_RESET_VALUE: &str = "RESET_VALUE";

/// The request member the composite component entries arrive in, and the member
/// a refusal about one of them names.
const COMPOSITE_MEMBER: &str = "compositeComponentUpdates";

/// The action one `CREATE`/`UPDATE`/`DELETE` entry asks for.
///
/// `exists` is whether the entity already holds the thing the key names, which
/// is what an entry naming NO `updateType` is read against: the update is then
/// the one the entity's own contents make it — a create of something it does
/// not have, a patch of something it does.
fn map_entry_action(
    member: &str,
    update_type: Option<&str>,
    accepted: &[&str],
    exists: bool,
) -> Result<Action, UpdateEntityError> {
    match update_type {
        None if exists => Ok(Action::Update),
        None => Ok(Action::Create),
        Some(UPDATE_TYPE_CREATE) => Ok(Action::Create),
        Some(UPDATE_TYPE_UPDATE) => Ok(Action::Update),
        Some(UPDATE_TYPE_DELETE) => Ok(Action::Delete),
        Some(other) => Err(unknown_update_type(member, other, accepted)),
    }
}

/// Apply one `componentUpdates` entry to the entity being patched.
fn apply_component_update(
    workspace: &crate::state::Workspace,
    entity: &mut crate::state::Entity,
    component_name: &str,
    update: ComponentUpdateRequest,
) -> Result<(), UpdateEntityError> {
    let member = format!("componentUpdates.{component_name}");
    let exists = entity.components.contains_key(component_name);
    let action = map_entry_action(
        &member,
        update.update_type.as_deref(),
        COMPONENT_UPDATE_TYPE_VALUES,
        exists,
    )?;
    if action != Action::Create && !exists {
        return Err(absent(&member, action, "component", component_name));
    }
    if action == Action::Create && exists {
        return Err(present(&member, "component", component_name));
    }
    if action == Action::Delete {
        // The composite components of a component are stored INSIDE it, so
        // removing it removes them with it: they are addressed by a path
        // through this component, and nothing can reach them once it is gone.
        entity.components.remove(component_name);
        return Ok(());
    }
    // A component INSTANTIATES the component type its `componentTypeId` names,
    // so an abstract one is refused before anything is written — the rule
    // `CreateEntity` applies to the components it writes.
    if let Some(message) = crate::state::abstract_component_type_violation(
        workspace,
        update.component_type_id.as_deref(),
    ) {
        return Err(validation(message));
    }
    let component = entity
        .components
        .entry(component_name.to_string())
        .or_insert_with(|| crate::state::ComponentRecord {
            component_name: component_name.to_string(),
            ..crate::state::ComponentRecord::default()
        });
    if let Some(component_type_id) = update.component_type_id {
        component.component_type_id = Some(component_type_id);
    }
    if let Some(description) = update.description {
        component.description = Some(description);
    }
    apply_property_updates(&mut component.properties, &member, update.property_updates)?;
    apply_property_group_updates(
        &mut component.property_groups,
        &member,
        update.property_group_updates,
    )
}

/// Apply one `compositeComponentUpdates` entry to the entity being patched.
///
/// The key is the composite component's `componentPath`, resolved against the
/// entity's components as they stand after the `componentUpdates` have been
/// applied and against the workspace's component types — the same resolution a
/// create uses, so one path rule answers both.
fn apply_composite_component_update(
    workspace: &crate::state::Workspace,
    entity: &mut crate::state::Entity,
    component_path: &str,
    update: CompositeComponentUpdateRequest,
) -> Result<(), UpdateEntityError> {
    let member = format!("{COMPOSITE_MEMBER}.{component_path}");
    let resolved = crate::state::resolve_composite_component_path(
        &entity.components,
        &workspace.component_types,
        COMPOSITE_MEMBER,
        component_path,
    )
    .map_err(validation)?;
    let exists = entity
        .components
        .get(&resolved.component_name)
        .is_some_and(|component| {
            component
                .composite_components
                .contains_key(&resolved.composite_path)
        });
    let action = map_entry_action(
        &member,
        update.update_type.as_deref(),
        COMPONENT_UPDATE_TYPE_VALUES,
        exists,
    )?;
    if action != Action::Create && !exists {
        return Err(absent(
            &member,
            action,
            "composite component",
            component_path,
        ));
    }
    if action == Action::Create && exists {
        return Err(present(&member, "composite component", component_path));
    }
    // A composite component instantiates the component type the declaration its
    // path resolved to names, so the abstractness rule holds for it as for a
    // top-level component.
    if action == Action::Create {
        if let Some(message) = crate::state::abstract_component_type_violation(
            workspace,
            Some(&resolved.component_type_id),
        ) {
            return Err(validation(message));
        }
    }
    // The holding component is the one the path's first segment was resolved
    // against, so it is present.
    let Some(component) = entity.components.get_mut(&resolved.component_name) else {
        return Err(validation(format!(
            "{member} names component {}, which the entity does not have",
            resolved.component_name
        )));
    };
    if action == Action::Delete {
        // The composite components DECLARED inside the one being removed are
        // addressed by a path THROUGH it, so nothing can reach them once it is
        // gone: the keys below this one go with it, exactly as removing a
        // top-level component takes the composite components inside it.
        let below = format!(
            "{}{}",
            resolved.composite_path,
            crate::state::COMPONENT_PATH_SEPARATOR
        );
        component.composite_components.retain(|composite_path, _| {
            composite_path.as_str() != resolved.composite_path
                && !composite_path.starts_with(&below)
        });
        return Ok(());
    }
    let composite = component
        .composite_components
        .entry(resolved.composite_path.clone())
        .or_insert_with(|| crate::state::CompositeComponentRecord {
            // The WHOLE path this call addressed it by, which is what
            // `ComponentSummary::componentPath` reports, and the path's last
            // segment as its own name — the key `GetEntity` reports it under.
            component_path: component_path.to_string(),
            component_name: resolved.composite_name.clone(),
            component_type_id: resolved.component_type_id.clone(),
            ..crate::state::CompositeComponentRecord::default()
        });
    if let Some(description) = update.description {
        composite.description = Some(description);
    }
    apply_property_updates(&mut composite.properties, &member, update.property_updates)?;
    apply_property_group_updates(
        &mut composite.property_groups,
        &member,
        update.property_group_updates,
    )
}

/// Apply a component's or a composite component's `propertyUpdates` to the
/// properties it holds.
///
/// Shared by both for the reason `CreateEntity` shares its conversions: a
/// component and a composite component carry the same modelled `PropertyRequests`
/// map, so one shape has one rule.
fn apply_property_updates(
    properties: &mut ::std::collections::BTreeMap<String, crate::state::PropertyRecord>,
    member_path: &str,
    updates: Option<::std::collections::HashMap<String, PropertyRequest>>,
) -> Result<(), UpdateEntityError> {
    for (property_name, update) in sorted(updates) {
        let member = format!("{member_path}.propertyUpdates.{property_name}");
        match update.update_type.as_deref() {
            Some(UPDATE_TYPE_DELETE) => {
                if properties.remove(&property_name).is_none() {
                    return Err(absent(
                        &member,
                        Action::Delete,
                        "property",
                        &property_name,
                    ));
                }
            }
            Some(UPDATE_TYPE_RESET_VALUE) => {
                let Some(property) = properties.get_mut(&property_name) else {
                    return Err(absent(
                        &member,
                        Action::Update,
                        "property",
                        &property_name,
                    ));
                };
                // The definition stays: a reset takes the property's VALUE
                // away, not the declaration that gives it meaning.
                property.value = None;
            }
            // `CREATE` and `UPDATE` both describe the property the entry
            // defines, so both write the definition and the value it carries;
            // an entry that names no update type does the same.
            None | Some(UPDATE_TYPE_CREATE) | Some(UPDATE_TYPE_UPDATE) => {
                // A definition the request carries REPLACES the property's
                // definition, so it must be one a read can report: every
                // reported `PropertyDefinitionResponse` requires a `dataType`,
                // which is the rule `CreateEntity` applies to the definitions
                // it stores.
                if update
                    .definition
                    .as_ref()
                    .is_some_and(|definition| definition.data_type.is_none())
                {
                    return Err(validation(format!("{member}.definition.dataType is required")));
                }
                let property = properties.entry(property_name).or_default();
                if let Some(definition) = update.definition {
                    property.definition = Some(definition);
                }
                if let Some(value) = update.value {
                    property.value = Some(value);
                }
            }
            Some(other) => {
                return Err(unknown_update_type(
                    &member,
                    other,
                    PROPERTY_UPDATE_TYPE_VALUES,
                ))
            }
        }
    }
    Ok(())
}

/// Apply a component's or a composite component's `propertyGroupUpdates` to the
/// property groups it holds. Shared by both for the same reason
/// [`apply_property_updates`] is.
fn apply_property_group_updates(
    property_groups: &mut ::std::collections::BTreeMap<String, crate::state::PropertyGroupRecord>,
    member_path: &str,
    updates: Option<::std::collections::HashMap<String, ComponentPropertyGroupRequest>>,
) -> Result<(), UpdateEntityError> {
    for (group_name, update) in sorted(updates) {
        let member = format!("{member_path}.propertyGroupUpdates.{group_name}");
        let exists = property_groups.contains_key(&group_name);
        let action = map_entry_action(
            &member,
            update.update_type.as_deref(),
            PROPERTY_GROUP_UPDATE_TYPE_VALUES,
            exists,
        )?;
        if action != Action::Create && !exists {
            return Err(absent(&member, action, "property group", &group_name));
        }
        if action == Action::Create && exists {
            return Err(present(&member, "property group", &group_name));
        }
        if action == Action::Delete {
            property_groups.remove(&group_name);
            continue;
        }
        let group = property_groups.entry(group_name).or_default();
        if let Some(group_type) = update.group_type {
            group.group_type = Some(group_type);
        }
        // Stored RESOLVED, the way a create stores it: an absent list is an
        // empty one rather than a missing collection every reader has to guard
        // against.
        if let Some(property_names) = update.property_names {
            group.property_names = property_names;
        }
    }
    Ok(())
}

/// The `parentEntityId` the entity ends up with, given the request's
/// `parentEntityUpdate`.
///
/// `DELETE` detaches the entity, which means re-parenting it to the sentinel the
/// service uses for an entity with no parent
/// ([`crate::state::ROOT_PARENT_ENTITY_ID`]) — the value a create with no
/// `parentEntityId` stores. `UPDATE` re-parents it to the id it carries, which
/// has to name an entity of this workspace and must not be the entity itself or
/// one BELOW it: either would make the stored parent graph cyclic, which the
/// tree walks over it must never be asked to handle.
fn updated_parent_entity_id(
    workspace: &crate::state::Workspace,
    workspace_id: &str,
    entity_id: &str,
    update: ParentEntityUpdateRequest,
) -> Result<String, UpdateEntityError> {
    const MEMBER: &str = "parentEntityUpdate";
    match update.update_type.as_str() {
        UPDATE_TYPE_DELETE => Ok(crate::state::ROOT_PARENT_ENTITY_ID.to_string()),
        UPDATE_TYPE_UPDATE => {
            let Some(parent_entity_id) = update.parent_entity_id else {
                return Err(validation(format!(
                    "{MEMBER}.parentEntityId is required when updateType is \
                     {UPDATE_TYPE_UPDATE}"
                )));
            };
            if parent_entity_id == crate::state::ROOT_PARENT_ENTITY_ID {
                return Ok(parent_entity_id);
            }
            if parent_entity_id == entity_id {
                return Err(validation(format!(
                    "{MEMBER}.parentEntityId {parent_entity_id} must not name the entity being \
                     updated"
                )));
            }
            if !workspace.entities.contains_key(&parent_entity_id) {
                return Err(validation(format!(
                    "{MEMBER}.parentEntityId {parent_entity_id} does not name an entity in \
                     workspace {workspace_id}"
                )));
            }
            if workspace.subtree_of(entity_id).contains(&parent_entity_id) {
                return Err(validation(format!(
                    "{MEMBER}.parentEntityId {parent_entity_id} names an entity below \
                     {entity_id}, which would make the parent graph cyclic"
                )));
            }
            Ok(parent_entity_id)
        }
        other => Err(unknown_update_type(
            MEMBER,
            other,
            PARENT_ENTITY_UPDATE_TYPE_VALUES,
        )),
    }
}

/// One refusal of this operation's `ValidationException`, with the message the
/// rule that refused the request produced.
///
/// The message is built from the REQUEST — the member and the id, name or path
/// the service could not resolve — so a caller is told what to change.
fn validation(message: String) -> UpdateEntityError {
    UpdateEntityErrorModeled::ValidationException(ValidationException {
        message: Some(message),
    })
    .into()
}

/// The refusal an entry earns for addressing something the entity does not
/// hold: only a create brings one into being, so patching or removing one that
/// is not there has nothing to act on.
fn absent(member: &str, action: Action, kind: &str, name: &str) -> UpdateEntityError {
    let verb = match action {
        Action::Delete => "removes",
        Action::Create | Action::Update => "updates",
    };
    validation(format!(
        "{member} {verb} {kind} {name}, which is not there"
    ))
}

/// The refusal a `CREATE` entry earns for naming something the entity already
/// holds: a create brings one into being, so there has to be nothing under that
/// key yet.
fn present(member: &str, kind: &str, name: &str) -> UpdateEntityError {
    validation(format!(
        "{member} creates {kind} {name}, which is already there"
    ))
}

/// The refusal an `updateType` outside its modelled value list earns, naming
/// the values that list publishes.
fn unknown_update_type(member: &str, update_type: &str, accepted: &[&str]) -> UpdateEntityError {
    validation(format!(
        "{member}.updateType {update_type} is not one of {}",
        accepted.join(", ")
    ))
}
