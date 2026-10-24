// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-iottwinmaker-awsiottwinmaker service.
//!
//! IoT TwinMaker's control plane is a two-level tree: a *workspace* owns the
//! *entities* and *component types* (and, in a later slice, the scenes) created
//! inside it. That containment is modelled literally here —
//! `Workspace::entities` and `Workspace::component_types` — so neither can
//! outlive its workspace and two operations cannot read two copies of one
//! resource.
//!
//! Nothing derivable is stored: `hasChildEntities` is computed by scanning the
//! workspace's entities for the id as a parent. A resource's `state`, by
//! contrast, IS stored. An entity's create settles it in `ACTIVE` within the
//! call that asked for it, so there is no provisioning instant for a read to
//! compute a transient state from; a component type's create answers the
//! non-terminal `CREATING`, so its record also carries the instant its
//! transition to `ACTIVE` falls due (`ComponentType::settles_at`), which a
//! reader materialises rather than sleeps for.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

/// The `ParentEntityId` sentinel the service uses for an entity with no parent.
/// Taken from the `ParentEntityId` pattern, which admits the literal `$ROOT`
/// beside a uuid and a plain id.
pub const ROOT_PARENT_ENTITY_ID: &str = "$ROOT";

/// The `State` enum values this slice produces. The full value list is
/// `STATE_VALUES` in `ops::types` (`CREATING`, `UPDATING`, `DELETING`, `ACTIVE`,
/// `ERROR`).
///
/// `ACTIVE` is the state an entity SETTLES in, and the one a stored entity
/// carries: a create completes within the call that asked for it, so no entity
/// is ever persisted in the non-terminal `CREATING` state — recorded traffic
/// reads a just-created entity back as `ACTIVE`, with no step anywhere in the
/// corpus observing it `CREATING`. The other values belong to transitions a
/// later slice owns (`UPDATING` for an update in flight, `ERROR` for a failed
/// one); `DELETING` is what `DeleteEntity` reports for the removal it performs.
pub const STATE_ACTIVE: &str = "ACTIVE";
/// See [`STATE_ACTIVE`].
pub const STATE_DELETING: &str = "DELETING";
/// See [`STATE_ACTIVE`]. A COMPONENT TYPE, unlike an entity, is reported in
/// this non-terminal state by the call that creates it and settles in
/// [`STATE_ACTIVE`] once [`COMPONENT_TYPE_SETTLE_SPAN`] has elapsed.
pub const STATE_CREATING: &str = "CREATING";
/// See [`STATE_ACTIVE`]. The non-terminal state an UPDATE in flight leaves a
/// component type in: `UpdateComponentType` answers with it, and the clock
/// settles the component type back in [`STATE_ACTIVE`] once
/// [`COMPONENT_TYPE_SETTLE_SPAN`] has elapsed, exactly as it does after a
/// create.
pub const STATE_UPDATING: &str = "UPDATING";

/// How long a component type stays in a non-terminal state — [`STATE_CREATING`]
/// after a create, [`STATE_UPDATING`] after an update — before the clock makes
/// its transition to [`STATE_ACTIVE`] due.
///
/// `CreateComponentType` answers with the component type's `state`, and the
/// state of a component type the service has just accepted is the non-terminal
/// `CREATING` — so there IS a provisioning window here, unlike an entity, which
/// is reported `ACTIVE` by its own create. `UpdateComponentType` opens the same
/// window with `UPDATING`: the call reports the change as in flight and the
/// component type is `ACTIVE` again once it has landed. AWS documents no
/// duration for either window, so this span is CHOSEN by this emulator; it is
/// deliberately non-zero, so a caller that reads the component type back in the
/// same breath as the create or the update still observes the transient state,
/// and deliberately short, so a client that polls reaches `ACTIVE` without a
/// long wait. Nothing sleeps for it: the instant it is due is stored on the
/// record (see [`ComponentType::settles_at`]) and a reader materialises the
/// transition the clock already owed.
pub const COMPONENT_TYPE_SETTLE_SPAN: Duration = Duration::from_millis(500);

/// The name of the function whose presence makes a component type's schema
/// initialized.
///
/// `isSchemaInitialized` is documented as "whether the component type has a
/// schema initializer and that the schema initializer has run": the schema
/// initializer is the entry of a component type's `functions` map under this
/// name, so a component type whose `functions` carry none has no schema to
/// initialize. See [`ComponentType::is_schema_initialized`].
pub const SCHEMA_INITIALIZER_FUNCTION: &str = "schemaInitializer";

/// The region an emulated ARN is built in when a request carries no SigV4
/// credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The largest page any list operation of this service serves, applied
/// SERVER-SIDE so a request cannot ask for an unbounded one, and the page size
/// served when a request names none.
///
/// Both come from the `maxResults` member's own documentation: "The maximum
/// number of results to return at one time. The default is 25." The ceiling is
/// the top of the member's modelled range (0..200), applied by the service
/// whatever the caller asks for, so the page is bounded by this code and not
/// only by the boundary validator.
pub const MAX_PAGE_SIZE: usize = 200;
/// See [`MAX_PAGE_SIZE`].
pub const DEFAULT_PAGE_SIZE: usize = 25;

/// The most components `GetEntity` reports for one entity, and the most
/// properties it reports for one component.
///
/// Both are documented on the flags that report whether the response was
/// truncated: "The maximum number of components returned is 30"
/// (`areAllComponentsReturned`) and "The maximum number of properties returned
/// is 800" (`areAllPropertiesReturned`).
pub const MAX_COMPONENTS_RETURNED: usize = 30;
/// See [`MAX_COMPONENTS_RETURNED`].
pub const MAX_PROPERTIES_RETURNED: usize = 800;

/// The most tags one taggable resource of this service carries.
///
/// Taken from the model, not chosen here: `TagMap` is `@length(min: 0, max:
/// 50)` (`smithy_query(query="shape",shape="TagMap")`), so fifty entries is the
/// largest tag set any of this service's shapes can carry on a resource. A
/// `TagResource` call that would leave a resource past it is refused with the
/// `TooManyTagsException` (HTTP 400) that operation models.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The most workspaces this emulator holds, the most entities one workspace
/// holds, and the most component types one workspace holds, before
/// `CreateWorkspace` / `CreateEntity` / `CreateComponentType` answer the
/// `ServiceQuotaExceededException` (HTTP 402) each operation models.
///
/// The real service's quotas are per-account and adjustable, so no fixed number
/// is AWS's; these are this emulator's own declared ceilings, so the in-memory
/// store cannot be grown without bound by a caller that keeps creating.
pub const MAX_WORKSPACES: usize = 50;
/// See [`MAX_WORKSPACES`].
pub const MAX_ENTITIES_PER_WORKSPACE: usize = 1000;
/// See [`MAX_WORKSPACES`].
pub const MAX_COMPONENT_TYPES_PER_WORKSPACE: usize = 1000;

/// The `PricingMode` an account that has never changed its pricing plan is on,
/// and the `UpdateReason` such a plan carries.
///
/// `BASIC` is the first value of the model's `PricingMode` enum
/// (`ops::types::PRICING_MODE_VALUES`) and the pay-as-you-go mode the service
/// bills an untouched account in; `DEFAULT` is the `UpdateReason` value that
/// names exactly that — a plan nobody has updated
/// (`ops::types::UPDATE_REASON_VALUES`).
pub const PRICING_MODE_BASIC: &str = "BASIC";
/// See [`PRICING_MODE_BASIC`].
pub const UPDATE_REASON_DEFAULT: &str = "DEFAULT";

/// Per-service state for com-amazonaws-iottwinmaker-awsiottwinmaker. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsIottwinmakerAwsiottwinmakerState {
    /// Workspaces by `workspaceId`.
    pub workspaces: BTreeMap<String, Workspace>,
    /// Every ARN this service has minted, so an operation whose input accepts an
    /// id *or* an ARN (`GetWorkspace`, and the tagging operations in a later
    /// slice) resolves one without scanning every workspace.
    pub arn_index: BTreeMap<String, ResourceRef>,
    /// The pricing plan this account is billed under for the current billing
    /// cycle — the one `GetPricingPlan` reports as `currentPricingPlan`.
    ///
    /// Account-wide, not per workspace: the pricing operations carry no
    /// workspace in their input, so there is ONE plan per account and region,
    /// held here beside the workspaces rather than on each of them. Present
    /// rather than optional, because an account always HAS a plan: until
    /// something changes it, that plan is the service default
    /// ([`PricingPlanRecord::default_plan`]), effective from the instant this
    /// store came into being — so a read never has to mint one, and two reads
    /// of an unchanged plan report the same instants.
    pub pricing_plan: PricingPlanRecord,
    /// The plan change that is accepted but not yet in force — what
    /// `GetPricingPlan` reports as `pendingPricingPlan` — and `None` while
    /// nothing is pending, which is what an account whose plan nobody has
    /// changed looks like. Written by `UpdatePricingPlan`.
    pub pending_pricing_plan: Option<PricingPlanRecord>,
}

impl Default for ComAmazonawsIottwinmakerAwsiottwinmakerState {
    fn default() -> Self {
        Self {
            workspaces: BTreeMap::new(),
            arn_index: BTreeMap::new(),
            // The account's plan dates from the moment the account's store
            // exists, so the default plan's instants are captured here, ONCE,
            // rather than by whichever call happens to read it first.
            pricing_plan: PricingPlanRecord::default_plan(SystemTime::now()),
            pending_pricing_plan: None,
        }
    }
}

/// One stored pricing plan: the mode and bundles it was set to, and when.
///
/// `billableEntityCount` is NOT a field here. It is the account's current
/// billable entity count, which the stored entities already say
/// ([`ComAmazonawsIottwinmakerAwsiottwinmakerState::billable_entity_count`]) —
/// keeping a copy beside the plan would let an entity created or deleted after
/// the plan was set leave it stale, the same reason `hasChildEntities` is
/// derived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PricingPlanRecord {
    /// A `PricingMode` value (`ops::types::PRICING_MODE_VALUES`).
    pub pricing_mode: String,
    /// An `UpdateReason` value (`ops::types::UPDATE_REASON_VALUES`): why this
    /// plan replaced the one before it.
    pub update_reason: String,
    /// The bundles this plan was set with, empty when it names none — a plan
    /// without bundles reports no `bundleInformation` at all, since
    /// `BundleInformation::bundleNames` is `@required` and `@length(1..10)`, so
    /// an empty list is not a value the shape admits.
    pub bundle_names: Vec<String>,
    /// The `PricingTier` the bundles place the account in
    /// (`ops::types::PRICING_TIER_VALUES`), absent when the plan carries no
    /// tier.
    pub pricing_tier: Option<String>,
    /// When this plan takes (or took) effect.
    pub effective_date_time: SystemTime,
    /// When this plan was set.
    pub update_date_time: SystemTime,
}

impl PricingPlanRecord {
    /// The plan an account that has never changed its pricing plan is on:
    /// [`PRICING_MODE_BASIC`] with no bundles, set and effective at `at` and
    /// carrying the [`UPDATE_REASON_DEFAULT`] reason.
    pub fn default_plan(at: SystemTime) -> Self {
        Self {
            pricing_mode: PRICING_MODE_BASIC.to_string(),
            update_reason: UPDATE_REASON_DEFAULT.to_string(),
            bundle_names: Vec::new(),
            pricing_tier: None,
            effective_date_time: at,
            update_date_time: at,
        }
    }

    /// This record as the modelled `PricingPlan`, reported with the account's
    /// current `billable_entity_count`.
    ///
    /// Shared by every reader of a stored plan, so the current and the pending
    /// plan — and `GetPricingPlan` and `UpdatePricingPlan` — report one plan the
    /// same way.
    pub fn as_pricing_plan(&self, billable_entity_count: i64) -> crate::ops::types::PricingPlan {
        crate::ops::types::PricingPlan {
            billable_entity_count: Some(billable_entity_count),
            // Omitted while the plan names no bundle: `bundleNames` is required
            // inside `BundleInformation`, so there is no empty form of it.
            bundle_information: (!self.bundle_names.is_empty()).then(|| {
                crate::ops::types::BundleInformation {
                    bundle_names: self.bundle_names.clone(),
                    pricing_tier: self.pricing_tier.clone(),
                }
            }),
            effective_date_time: timestamp(self.effective_date_time),
            pricing_mode: self.pricing_mode.clone(),
            update_date_time: timestamp(self.update_date_time),
            update_reason: self.update_reason.clone(),
        }
    }
}

/// What an ARN in [`ComAmazonawsIottwinmakerAwsiottwinmakerState::arn_index`] points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceRef {
    /// A workspace, by id.
    Workspace { workspace_id: String },
    /// An entity, by the workspace that owns it and its own id.
    Entity {
        workspace_id: String,
        entity_id: String,
    },
    /// A component type, by the workspace that owns it and its own id.
    ComponentType {
        workspace_id: String,
        component_type_id: String,
    },
}

/// One stored workspace.
#[derive(Debug, Clone)]
pub struct Workspace {
    pub workspace_id: String,
    pub arn: String,
    /// The workspace's description, the empty string when the create named
    /// none: `Description` is modelled `@length 0..2048`, so the empty string
    /// is a legal value, and the service carries the member on every resource
    /// rather than leaving it absent. See [`Entity::description`].
    pub description: String,
    pub s3_location: Option<String>,
    pub role: Option<String>,
    /// Other AWS services linked into this workspace. Populated by the sync-job
    /// slice; empty until then.
    pub linked_services: Vec<String>,
    pub tags: BTreeMap<String, String>,
    pub creation_date_time: SystemTime,
    pub update_date_time: SystemTime,
    /// The entities created in this workspace, keyed by `entityId`.
    pub entities: BTreeMap<String, Entity>,
    /// The component types created in this workspace, keyed by
    /// `componentTypeId`. Owned by the workspace for the same reason entities
    /// are: a component type is addressed as
    /// `/workspaces/{workspaceId}/component-types/{componentTypeId}`, so it
    /// cannot outlive the workspace that contains it, and two operations must
    /// not read two copies of one component type.
    pub component_types: BTreeMap<String, ComponentType>,
}

impl Workspace {
    /// Whether any stored entity names `entity_id` as its parent.
    ///
    /// Derived on every read rather than kept as a flag beside the entity, so a
    /// child created or deleted after the parent cannot leave it stale.
    pub fn has_child_entities(&self, entity_id: &str) -> bool {
        self.entities
            .values()
            .any(|entity| entity.parent_entity_id == entity_id)
    }

    /// The ids of the entities whose parent is `entity_id`, in id order.
    pub fn children_of(&self, entity_id: &str) -> Vec<String> {
        self.entities
            .values()
            .filter(|entity| entity.parent_entity_id == entity_id)
            .map(|entity| entity.entity_id.clone())
            .collect()
    }

    /// Everything stored in this workspace that still REFERENCES the component
    /// type `component_type_id`, each as a short description of where the
    /// reference lives, in a stable (id-ordered) order.
    ///
    /// A stored reference to a component type is one of:
    ///
    /// * an entity's component, through `ComponentRequest::componentTypeId` —
    ///   the component IS an instance of the type, so the type's definitions are
    ///   what give the component's properties their meaning;
    /// * another component type that `extendsFrom` it — the child inherits its
    ///   property definitions;
    /// * another component type that names it in its `compositeComponentTypes`
    ///   — the composite type is built out of it.
    ///
    /// A `RELATIONSHIP` property definition's `targetComponentTypeId` is NOT
    /// counted: it is a declared target no writer in this service requires to
    /// resolve (a create stores it without checking, and it may name one of the
    /// service's built-in types, of which this emulator holds no registry), so
    /// counting it would refuse deletes the real service allows.
    ///
    /// Derived on every call rather than kept as a count beside the component
    /// type, so a reference created or dropped afterwards cannot leave it stale
    /// — the same choice [`has_child_entities`](Self::has_child_entities) makes.
    pub fn component_type_references(&self, component_type_id: &str) -> Vec<String> {
        let mut references = Vec::new();
        for entity in self.entities.values() {
            for component in entity.components.values() {
                if component.component_type_id.as_deref() == Some(component_type_id) {
                    references.push(format!(
                        "entity {} component {}",
                        entity.entity_id, component.component_name
                    ));
                }
            }
        }
        for component_type in self.component_types.values() {
            // A component type referencing ITSELF is not something that keeps
            // it alive: the reference goes away with the record that carries it.
            if component_type.component_type_id == component_type_id {
                continue;
            }
            if component_type
                .extends_from
                .iter()
                .any(|parent| parent == component_type_id)
            {
                references.push(format!(
                    "componentType {} extendsFrom",
                    component_type.component_type_id
                ));
            }
            for (composite_name, composite) in &component_type.composite_component_types {
                if composite.component_type_id.as_deref() == Some(component_type_id) {
                    references.push(format!(
                        "componentType {} compositeComponentTypes.{composite_name}",
                        component_type.component_type_id
                    ));
                }
            }
        }
        references
    }

    /// Every id that some stored entity names as its parent, in ONE pass.
    ///
    /// A listing needs `hasChildEntities` for each entity it reports; asking
    /// [`has_child_entities`](Self::has_child_entities) per reported entity would
    /// make the page cost quadratic in the workspace's size, so a list builds
    /// this set once and looks each id up in it.
    pub fn parent_ids(&self) -> std::collections::BTreeSet<&str> {
        self.entities
            .values()
            .map(|entity| entity.parent_entity_id.as_str())
            .collect()
    }

    /// `entity_id` plus every entity below it, parents before children.
    ///
    /// TERMINATION is structural, not assumed: an id is expanded only the first
    /// time it is seen, and the ids come from this workspace's own entity map,
    /// so the walk visits at most `entities.len() + 1` of them however the
    /// stored `parent_entity_id` values point — a self-parenting entity or a
    /// cycle cannot make it loop or grow without bound. The explicit cap below
    /// makes that bound local to this function rather than a property a reader
    /// has to derive.
    pub fn subtree_of(&self, entity_id: &str) -> Vec<String> {
        let cap = self.entities.len() + 1;
        let mut seen: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::from([entity_id.to_string()]);
        let mut queue: std::collections::VecDeque<String> =
            std::collections::VecDeque::from([entity_id.to_string()]);
        let mut ordered = Vec::new();
        while let Some(parent) = queue.pop_front() {
            if ordered.len() >= cap {
                break;
            }
            ordered.push(parent.clone());
            for child in self.children_of(&parent) {
                if seen.insert(child.clone()) {
                    queue.push_back(child);
                }
            }
        }
        ordered
    }
}

/// One stored entity.
#[derive(Debug, Clone)]
pub struct Entity {
    pub entity_id: String,
    pub entity_name: String,
    pub arn: String,
    pub workspace_id: String,
    /// The entity's description, the empty string when the create named none.
    ///
    /// A description is a member every entity carries rather than one it may
    /// lack: `Description` admits the empty string (`@length 0..2048`), and a
    /// read reports the member for an entity whose description is empty instead
    /// of omitting it, so a client that reads it back needs no null guard.
    pub description: String,
    /// [`ROOT_PARENT_ENTITY_ID`] when the entity was created without a parent.
    pub parent_entity_id: String,
    /// The entity's components, keyed by component name.
    pub components: BTreeMap<String, ComponentRecord>,
    pub tags: BTreeMap<String, String>,
    pub creation_date_time: SystemTime,
    pub update_date_time: SystemTime,
    /// The entity's `State`: the state the STORED record carries, so every
    /// reader reports the same one and no read has to recompute it. A create
    /// settles the entity in [`STATE_ACTIVE`] (see its docs); the operations a
    /// later slice implements are what write any other value here.
    pub state: String,
}

impl Entity {
    /// The entity's modelled `Status`: its stored state, with the `error`
    /// structure every reported status carries. See [`status`].
    pub fn status(&self) -> crate::ops::types::Status {
        status(&self.state)
    }
}

/// One stored component type.
///
/// A component type's property definitions, functions and composite component
/// types are kept as the modelled REQUEST shapes the caller supplied — the same
/// choice [`ComponentRecord`] makes for an entity's properties — so a read
/// reports back exactly what was persisted rather than a lossy projection of
/// it.
#[derive(Debug, Clone)]
pub struct ComponentType {
    pub component_type_id: String,
    pub arn: String,
    pub workspace_id: String,
    /// The friendly name the create supplied, absent when it named none.
    pub component_type_name: Option<String>,
    /// The description the create supplied, absent when it named none.
    pub description: Option<String>,
    /// "Whether an entity can have more than one component of this type" — the
    /// member's own documentation. Stored resolved: a create that names none
    /// leaves it `false`.
    pub is_singleton: bool,
    /// The parent component types this one extends, in the order the create
    /// gave them.
    pub extends_from: Vec<String>,
    pub property_definitions: BTreeMap<String, crate::ops::types::PropertyDefinitionRequest>,
    pub functions: BTreeMap<String, crate::ops::types::FunctionRequest>,
    pub property_groups: BTreeMap<String, PropertyGroupRecord>,
    pub composite_component_types:
        BTreeMap<String, crate::ops::types::CompositeComponentTypeRequest>,
    pub tags: BTreeMap<String, String>,
    pub creation_date_time: SystemTime,
    pub update_date_time: SystemTime,
    /// The component type's `State`, as STORED: [`STATE_CREATING`] from the
    /// create (or [`STATE_UPDATING`] from an update) until
    /// [`settles_at`](Self::settles_at) passes, then [`STATE_ACTIVE`].
    pub state: String,
    /// The instant the transition out of [`STATE_CREATING`] or
    /// [`STATE_UPDATING`] falls due — the instant of the call that opened the
    /// window plus [`COMPONENT_TYPE_SETTLE_SPAN`]. Stored, never slept for, so
    /// a reader can materialise a transition the clock already owed without any
    /// caller having had to poll.
    pub settles_at: SystemTime,
}

impl ComponentType {
    /// Materialise the lifecycle transition the CLOCK has already made due.
    ///
    /// Called by a reader before it reports this component type: the
    /// `CREATING` -> `ACTIVE` (or `UPDATING` -> `ACTIVE`) transition was owed at
    /// [`settles_at`](Self::settles_at) whether or not anybody called, so
    /// writing it here is making the record say what was already true, not a
    /// change a read caused. Idempotent, and a no-op before the instant falls
    /// due.
    pub fn settle(&mut self, now: SystemTime) {
        let in_flight = self.state == STATE_CREATING || self.state == STATE_UPDATING;
        if in_flight && now >= self.settles_at {
            self.state = STATE_ACTIVE.to_string();
        }
    }

    /// Open the provisioning window an UPDATE owes.
    ///
    /// The update has landed in the store by the time this is called, so the
    /// record's `updateDateTime` moves to the instant of the call that made the
    /// change; its state leaves [`STATE_ACTIVE`] for the non-terminal
    /// [`STATE_UPDATING`] the update answers with, and the instant the clock
    /// owes the return to [`STATE_ACTIVE`] is stored beside it — the same span a
    /// create opens, and never slept for. See [`settle`](Self::settle).
    pub fn begin_update(&mut self, now: SystemTime) {
        self.update_date_time = now;
        self.state = STATE_UPDATING.to_string();
        self.settles_at = now + COMPONENT_TYPE_SETTLE_SPAN;
    }

    /// The component type's modelled `Status`: its stored state plus the
    /// `error` structure every reported status carries. See [`status`].
    pub fn status(&self) -> crate::ops::types::Status {
        status(&self.state)
    }

    /// Whether the component type "has a schema initializer and the schema
    /// initializer has run" (the `isSchemaInitialized` member's own
    /// documentation): it has one when its `functions` carry
    /// [`SCHEMA_INITIALIZER_FUNCTION`], and that initializer has run once the
    /// component type has settled, which is exactly when the stored state is
    /// [`STATE_ACTIVE`]: neither a create nor an update still in flight has
    /// finished running it.
    pub fn is_schema_initialized(&self) -> bool {
        self.functions.contains_key(SCHEMA_INITIALIZER_FUNCTION) && self.state == STATE_ACTIVE
    }
}

/// The modelled `Status` of a resource in `state`.
///
/// Both of `Status`'s members are optional in the model, but the service
/// reports the structure WHOLE: `error` is a present `ErrorDetails` — carrying
/// neither a code nor a message while nothing has failed — rather than an
/// absent member, so a client that reads `status.error` without a null guard
/// sees an empty structure instead of nothing. A failed resource is the same
/// structure with its `code`/`message` filled in.
pub fn status(state: &str) -> crate::ops::types::Status {
    crate::ops::types::Status {
        state: Some(state.to_string()),
        error: Some(crate::ops::types::ErrorDetails {
            code: None,
            message: None,
        }),
    }
}

/// The single value the model's `GroupType` enum defines
/// (`ops::types::GROUP_TYPE_VALUES`).
///
/// `groupType` is required on every property-group RESPONSE but optional on the
/// request, so a group stored without one is reported with the only value the
/// model admits rather than with an invented string.
pub const TABULAR_GROUP_TYPE: &str = "TABULAR";

/// Whether storing this property definition would leave a component type that
/// `extends_from` these parents holding a definition no read can report.
///
/// `dataType` is optional on `PropertyDefinitionRequest` but REQUIRED on the
/// `PropertyDefinitionResponse` a read reports (see
/// [`property_definition_response`]), so a definition may omit it only where it
/// refines one it inherits. A component type that extends nothing inherits
/// nothing, so each of its own definitions must carry one, and a writer refuses
/// the request rather than storing a definition a reader would have to drop.
/// Shared by every writer of a definition so a create and an update hold one
/// definition to one rule.
pub fn property_definition_needs_data_type(
    extends_from: &[String],
    definition: &crate::ops::types::PropertyDefinitionRequest,
) -> bool {
    extends_from.is_empty() && definition.data_type.is_none()
}

/// One stored `PropertyDefinitionRequest` as the modelled
/// `PropertyDefinitionResponse` a read reports.
///
/// Shared by every reader of a stored definition — an entity's component
/// properties (`GetEntity`) and a component type's own property definitions
/// (`GetComponentType`) — so both report one definition the same way.
///
/// `None` when the stored definition carries no `dataType`, which the response
/// shape requires: the writers refuse a new definition without one, so this
/// stays fallible rather than inventing a `Type` the model does not define.
/// Nothing imports, finalises or inherits a definition yet: `isImported`,
/// `isFinal` and `isInherited` all describe a definition that came from
/// somewhere else, and every definition stored here was written by the create
/// that declared it.
pub fn property_definition_response(
    definition: &crate::ops::types::PropertyDefinitionRequest,
) -> Option<crate::ops::types::PropertyDefinitionResponse> {
    let Some(data_type) = definition.data_type.clone() else {
        // Logged rather than dropped silently so a regression in a writer that
        // is supposed to refuse a definition without a `dataType` is visible
        // instead of quietly costing the member.
        ::tracing::warn!("a stored property definition carries no dataType; omitting it");
        return None;
    };
    Some(crate::ops::types::PropertyDefinitionResponse {
        data_type,
        is_time_series: definition.is_time_series.unwrap_or(false),
        is_required_in_entity: definition.is_required_in_entity.unwrap_or(false),
        is_external_id: definition.is_external_id.unwrap_or(false),
        is_stored_externally: definition.is_stored_externally.unwrap_or(false),
        is_imported: false,
        is_final: false,
        is_inherited: false,
        default_value: definition.default_value.clone(),
        configuration: definition.configuration.clone(),
        display_name: definition.display_name.clone(),
    })
}

/// One component of an entity.
///
/// A component's properties and property groups are stored as the modelled
/// request shapes the caller supplied, so `GetEntity` reports back exactly the
/// definition and value that were persisted rather than a lossy projection of
/// them. Inheritance from a component *type* is not represented: component types
/// are a later slice, so every definition stored here is the entity-local one.
#[derive(Debug, Clone, Default)]
pub struct ComponentRecord {
    pub component_name: String,
    pub component_type_id: Option<String>,
    pub description: Option<String>,
    /// The component's properties, keyed by property name.
    pub properties: BTreeMap<String, PropertyRecord>,
    /// The component's property groups, keyed by group name.
    pub property_groups: BTreeMap<String, PropertyGroupRecord>,
}

impl ComponentRecord {
    /// The external id this component carries, if any: the string value of the
    /// property its definition marked `isExternalId`. `ListEntities`'
    /// `externalId` filter matches on this.
    pub fn external_ids(&self) -> impl Iterator<Item = &str> {
        self.properties
            .values()
            .filter_map(PropertyRecord::external_id)
    }
}

/// One property of a component: the definition and value the request gave it.
#[derive(Debug, Clone, Default)]
pub struct PropertyRecord {
    pub definition: Option<crate::ops::types::PropertyDefinitionRequest>,
    pub value: Option<crate::ops::types::DataValue>,
}

impl PropertyRecord {
    /// Whether this property's definition marks it as the external id — the
    /// primary key of an external storage system.
    pub fn is_external_id(&self) -> bool {
        self.definition
            .as_ref()
            .and_then(|definition| definition.is_external_id)
            .unwrap_or(false)
    }

    /// This property's value as an external id: its string value, and only when
    /// its definition marks it as the external id.
    pub fn external_id(&self) -> Option<&str> {
        if !self.is_external_id() {
            return None;
        }
        self.value.as_ref()?.string_value.as_deref()
    }
}

/// One property group of a component or of a component type.
#[derive(Debug, Clone, Default)]
pub struct PropertyGroupRecord {
    pub group_type: Option<String>,
    pub property_names: Vec<String>,
}

impl PropertyGroupRecord {
    /// One modelled `PropertyGroupRequest` as the record the store holds.
    ///
    /// Shared by every writer of a COMPONENT TYPE's property groups
    /// (`CreateComponentType`, `UpdateComponentType`), so one shape has one
    /// conversion. `propertyNames` is stored RESOLVED, so an absent list is an
    /// empty one rather than a missing collection every reader has to guard
    /// against. An entity component's groups are a different modelled shape
    /// (`ComponentPropertyGroupRequest`, which also carries the `updateType`
    /// that says what to do with the group), so `CreateEntity` builds its
    /// records from that shape instead.
    pub fn from_request(request: crate::ops::types::PropertyGroupRequest) -> Self {
        Self {
            group_type: request.group_type,
            property_names: request.property_names.unwrap_or_default(),
        }
    }
}

impl ComAmazonawsIottwinmakerAwsiottwinmakerState {
    /// The account's billable entity count: how many entities this service
    /// currently holds, across every workspace.
    ///
    /// IoT TwinMaker bills the entities an account keeps in its workspaces, so
    /// the count a pricing plan is reported with is a count of the stored
    /// entities — derived on every read, never a counter something bumps, so a
    /// create or a delete cannot leave it stale and reading the plan does not
    /// change it. Saturating rather than panicking on the (unreachable, given
    /// [`MAX_WORKSPACES`] × [`MAX_ENTITIES_PER_WORKSPACE`]) overflow of the
    /// modelled `Long`.
    pub fn billable_entity_count(&self) -> i64 {
        let count: usize = self
            .workspaces
            .values()
            .map(|workspace| workspace.entities.len())
            .sum();
        i64::try_from(count).unwrap_or(i64::MAX)
    }

    /// Resolve an `IdOrArn` input to a stored `workspaceId`.
    ///
    /// `GetWorkspace`'s label accepts the workspace id or its ARN (its
    /// `IdOrArn` pattern is the union of both), and both must resolve to the one
    /// stored record.
    pub fn resolve_workspace_id(&self, id_or_arn: &str) -> Option<String> {
        if self.workspaces.contains_key(id_or_arn) {
            return Some(id_or_arn.to_string());
        }
        match self.arn_index.get(id_or_arn) {
            Some(ResourceRef::Workspace { workspace_id }) => Some(workspace_id.clone()),
            _ => None,
        }
    }

    /// The stored tag map of the resource `resource_arn` names, or `None` when
    /// this service holds no resource under that ARN.
    ///
    /// ONE resolution path for every taggable resource: the tags live on the
    /// resource record itself (never in a second map beside it), and the ARN
    /// index says which record an ARN points at, so a workspace, an entity and
    /// a component type are reached the same way and a reader and a writer see
    /// one tag set.
    pub fn tags_for_resource(&self, resource_arn: &str) -> Option<&BTreeMap<String, String>> {
        match self.arn_index.get(resource_arn)? {
            ResourceRef::Workspace { workspace_id } => {
                Some(&self.workspaces.get(workspace_id)?.tags)
            }
            ResourceRef::Entity {
                workspace_id,
                entity_id,
            } => Some(
                &self
                    .workspaces
                    .get(workspace_id)?
                    .entities
                    .get(entity_id)?
                    .tags,
            ),
            ResourceRef::ComponentType {
                workspace_id,
                component_type_id,
            } => Some(
                &self
                    .workspaces
                    .get(workspace_id)?
                    .component_types
                    .get(component_type_id)?
                    .tags,
            ),
        }
    }

    /// [`tags_for_resource`](Self::tags_for_resource), for a caller that writes.
    pub fn tags_for_resource_mut(
        &mut self,
        resource_arn: &str,
    ) -> Option<&mut BTreeMap<String, String>> {
        // Cloned out of the index first: the index and the resource records are
        // both fields of `self`, so holding a borrow of one would forbid the
        // mutable borrow of the other.
        let resource = self.arn_index.get(resource_arn)?.clone();
        match resource {
            ResourceRef::Workspace { workspace_id } => {
                Some(&mut self.workspaces.get_mut(&workspace_id)?.tags)
            }
            ResourceRef::Entity {
                workspace_id,
                entity_id,
            } => Some(
                &mut self
                    .workspaces
                    .get_mut(&workspace_id)?
                    .entities
                    .get_mut(&entity_id)?
                    .tags,
            ),
            ResourceRef::ComponentType {
                workspace_id,
                component_type_id,
            } => Some(
                &mut self
                    .workspaces
                    .get_mut(&workspace_id)?
                    .component_types
                    .get_mut(&component_type_id)?
                    .tags,
            ),
        }
    }
}

/// The message for a resource no ARN in this service's index names.
///
/// Built from the request, naming the ARN that was looked for — the same
/// construction as [`workspace_not_found_message`], one level up from an id to
/// the ARN the tagging operations address a resource by.
pub fn resource_not_found_message(resource_arn: &str) -> String {
    format!("Resource {resource_arn} not found.")
}

/// The message for an entity the workspace the request named does not hold.
///
/// Built from the request: it names the entity that was looked for and the
/// workspace it was looked for in, which is the construction the service's own
/// `ResourceNotFoundException` for an entity uses.
pub fn entity_not_found_message(workspace_id: &str, entity_id: &str) -> String {
    format!("Entity {entity_id} in workspace {workspace_id} not found.")
}

/// The message for a component type the workspace the request named does not
/// hold. Built from the request, naming what was looked for and where, the same
/// construction as [`entity_not_found_message`] one resource across.
pub fn component_type_not_found_message(workspace_id: &str, component_type_id: &str) -> String {
    format!("ComponentType {component_type_id} in workspace {workspace_id} not found.")
}

/// The message for a workspace this service does not hold: built from the
/// request, naming the workspace that was looked for. Same construction as
/// [`entity_not_found_message`], one level up the resource tree.
pub fn workspace_not_found_message(workspace_id: &str) -> String {
    format!("Workspace {workspace_id} not found.")
}

/// The region to build an ARN in for this request: the region of its SigV4
/// credential scope, or [`DEFAULT_REGION`] when it is unsigned.
///
/// A scope region that is not of the shape the modelled `TwinMakerArn` admits
/// for its region segment (`[a-z0-9-]+`) is replaced by the default, so a
/// caller cannot drive this service into minting an ARN its own model forbids.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of a workspace.
pub fn workspace_arn(region: &str, workspace_id: &str) -> String {
    ::engine::emulator_arn("iottwinmaker", region, &format!("workspace/{workspace_id}"))
}

/// The ARN of an entity, nested under its workspace's resource path.
pub fn entity_arn(region: &str, workspace_id: &str, entity_id: &str) -> String {
    ::engine::emulator_arn(
        "iottwinmaker",
        region,
        &format!("workspace/{workspace_id}/entity/{entity_id}"),
    )
}

/// The ARN of a component type, nested under its workspace's resource path the
/// way [`entity_arn`] nests an entity's.
pub fn component_type_arn(region: &str, workspace_id: &str, component_type_id: &str) -> String {
    ::engine::emulator_arn(
        "iottwinmaker",
        region,
        &format!("workspace/{workspace_id}/componentType/{component_type_id}"),
    )
}

/// A stored instant as the modelled `Timestamp` the wire carries.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_nanos(),
    )
}

/// The opaque `nextToken` every paginated operation of this service mints.
///
/// One shape, shared by all of them so a token means the same thing wherever it
/// is read: hex (so the token is opaque to a caller and safe in a JSON body)
/// over an `envelope` naming the LISTING it belongs to, the resume `key` it
/// resumes at, and the `scope` the listing was taken in — the parent resource
/// whose page it is, which makes a token minted for another resource, by another
/// listing, or by hand recognisably not this listing's.
///
/// The parts are separated by newlines, which the ids, ARNs and tag keys this
/// service's listings resume at all exclude; the LAST scope part keeps any
/// newline it carries (see [`decode_page_token`]), so a scope part that is free
/// text — a query statement — goes last.
pub fn encode_page_token(envelope: &str, scope: &[&str], key: &str) -> String {
    let mut body = format!("{envelope}{key}");
    for part in scope {
        body.push('\n');
        body.push_str(part);
    }
    ::hex::encode(body)
}

/// The resume key inside a `nextToken` [`encode_page_token`] minted for this
/// `envelope` and `scope`, or `None` when the token is not one of them.
///
/// The key is split off first and each scope part is compared in turn, so a
/// token whose envelope, arity or scope disagrees is rejected rather than
/// resumed; the final part absorbs the remainder, so the last scope part may
/// itself contain newlines. A token carrying an EMPTY key is rejected too: a
/// listing never mints one, since a page is only resumed at a row that exists.
pub fn decode_page_token(envelope: &str, scope: &[&str], token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let body = decoded.strip_prefix(envelope)?;
    let mut parts = body.splitn(scope.len() + 1, '\n');
    let key = parts.next()?;
    for expected in scope {
        if parts.next()? != *expected {
            return None;
        }
    }
    (!key.is_empty()).then(|| key.to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsIottwinmakerAwsiottwinmakerState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsIottwinmakerAwsiottwinmakerState>)
}
