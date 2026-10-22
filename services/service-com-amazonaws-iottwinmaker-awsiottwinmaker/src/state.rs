// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-iottwinmaker-awsiottwinmaker service.
//!
//! IoT TwinMaker's control plane is a two-level tree: a *workspace* owns the
//! *entities* (and, in a later slice, the component types and scenes) created
//! inside it. That containment is modelled literally here — `Workspace::entities`
//! — so an entity cannot outlive its workspace and two operations cannot read two
//! copies of one entity.
//!
//! Nothing derivable is stored: `hasChildEntities` is computed by scanning the
//! workspace's entities for the id as a parent. An entity's `state`, by
//! contrast, IS stored — a create settles it in `ACTIVE` within the call that
//! asked for it, so there is no provisioning instant for a read to compute a
//! transient state from.

use std::collections::BTreeMap;
use std::time::SystemTime;

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

/// The most workspaces this emulator holds, and the most entities one workspace
/// holds, before `CreateWorkspace` / `CreateEntity` answer the
/// `ServiceQuotaExceededException` (HTTP 402) both operations model.
///
/// The real service's quotas are per-account and adjustable, so no fixed number
/// is AWS's; these are this emulator's own declared ceilings, so the in-memory
/// store cannot be grown without bound by a caller that keeps creating.
pub const MAX_WORKSPACES: usize = 50;
/// See [`MAX_WORKSPACES`].
pub const MAX_ENTITIES_PER_WORKSPACE: usize = 1000;

/// Per-service state for com-amazonaws-iottwinmaker-awsiottwinmaker. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsIottwinmakerAwsiottwinmakerState {
    /// Workspaces by `workspaceId`.
    pub workspaces: BTreeMap<String, Workspace>,
    /// Every ARN this service has minted, so an operation whose input accepts an
    /// id *or* an ARN (`GetWorkspace`, and the tagging operations in a later
    /// slice) resolves one without scanning every workspace.
    pub arn_index: BTreeMap<String, ResourceRef>,
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

/// One property group of a component.
#[derive(Debug, Clone, Default)]
pub struct PropertyGroupRecord {
    pub group_type: Option<String>,
    pub property_names: Vec<String>,
}

impl ComAmazonawsIottwinmakerAwsiottwinmakerState {
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
    /// index says which record an ARN points at, so a workspace and an entity
    /// are reached the same way and a reader and a writer see one tag set.
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
