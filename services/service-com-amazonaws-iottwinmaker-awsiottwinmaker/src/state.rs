// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-iottwinmaker-awsiottwinmaker service.
//!
//! The store is REGIONAL before it is anything else: IoT TwinMaker is a
//! regional service, so everything below lives inside one
//! [`RegionalState`] and the top-level
//! [`ComAmazonawsIottwinmakerAwsiottwinmakerState`] is just the map from a
//! Region to its store. A request is served out of the store of the Region its
//! own credential scope names ([`request_region`]), so a resource created
//! through one Region's endpoint is invisible through another's and the same id
//! may name an independent resource in each.
//!
//! Inside one Region, IoT TwinMaker's control plane is a two-level tree: a
//! *workspace* owns the
//! *entities*, *component types*, *scenes* and *sync jobs* created inside it.
//! That containment is modelled literally here — `Workspace::entities`,
//! `Workspace::component_types`, `Workspace::scenes` and
//! `Workspace::sync_jobs` — so none of them can outlive its workspace and two
//! operations cannot read two copies of one resource.
//!
//! Nothing derivable is stored: `hasChildEntities` is computed by scanning the
//! workspace's entities for the id as a parent, and a workspace's
//! `linkedServices` by reading the sources of the sync jobs it holds
//! (`Workspace::linked_services`). A resource's `state`, by
//! contrast, IS stored. An entity's create settles it in `ACTIVE` within the
//! call that asked for it, so a created entity has no provisioning instant for
//! a read to compute a transient state from; an entity's UPDATE does open such
//! a window, so the record carries the instant the state it was left in falls
//! due (`Entity::settles_at`), which a reader reports through rather than
//! sleeps for. A component type's create answers the
//! non-terminal `CREATING`, so its record also carries the instant its
//! transition to `ACTIVE` falls due (`ComponentType::settles_at`), which a
//! reader materialises rather than sleeps for.
//!
//! Not everything is inside that tree: a *metadata transfer job* is addressed
//! with no workspace in its path and may name a different workspace at each
//! end of the transfer, so the jobs live beside the workspaces
//! (`metadata_transfer_jobs`). A job is accepted in the non-terminal
//! `VALIDATING` state and carries the instant its outcome falls due
//! (`MetadataTransferJob::settles_at`), the same way a component type carries
//! its own — and so does a *sync job* (`SyncJob::settles_at`), which
//! `CreateSyncJob` accepts in the non-terminal `CREATING` state.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
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
/// corpus observing it `CREATING`. `UPDATING` is what an update in flight
/// leaves an entity in ([`Entity::begin_update`]) until the clock settles it
/// back in `ACTIVE`; `ERROR` belongs to a failed transition this emulator never
/// produces, and `DELETING` is what `DeleteEntity` reports for the removal it
/// performs.
pub const STATE_ACTIVE: &str = "ACTIVE";
/// See [`STATE_ACTIVE`].
pub const STATE_DELETING: &str = "DELETING";
/// See [`STATE_ACTIVE`]. A COMPONENT TYPE, unlike an entity, is reported in
/// this non-terminal state by the call that creates it and settles in
/// [`STATE_ACTIVE`] once [`COMPONENT_TYPE_SETTLE_SPAN`] has elapsed.
pub const STATE_CREATING: &str = "CREATING";
/// See [`STATE_ACTIVE`]. The non-terminal state an UPDATE in flight leaves a
/// resource in: `UpdateComponentType` and `UpdateEntity` answer with it, and
/// the clock settles the resource back in [`STATE_ACTIVE`] once the matching
/// span ([`COMPONENT_TYPE_SETTLE_SPAN`], [`ENTITY_SETTLE_SPAN`]) has elapsed —
/// for a component type, exactly as it does after a create.
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

/// How long an entity stays in [`STATE_UPDATING`] after an update before the
/// clock makes its transition back to [`STATE_ACTIVE`] due.
///
/// `UpdateEntity` answers with "the current state of the entity update", and the
/// state of an update the service has just accepted is the non-terminal
/// `UPDATING`: the change is reported as in flight and the entity is `ACTIVE`
/// again once it has landed. (A CREATE opens no such window — an entity's
/// create settles it in [`STATE_ACTIVE`] inside the call that asked for it; see
/// that constant.) AWS documents no duration for this window, so this span is
/// CHOSEN by this emulator; it is deliberately non-zero, so a caller that reads
/// the entity back in the same breath as the update still observes the
/// transient state, and deliberately short, so a client that polls reaches
/// `ACTIVE` without a long wait. Nothing sleeps for it: the instant it is due is
/// stored on the record (see [`Entity::settles_at`]) and every read reports the
/// state the clock has already made true.
pub const ENTITY_SETTLE_SPAN: Duration = Duration::from_millis(500);

/// The name of the function whose presence makes a component type's schema
/// initialized.
///
/// `isSchemaInitialized` is documented as "whether the component type has a
/// schema initializer and that the schema initializer has run": the schema
/// initializer is the entry of a component type's `functions` map under this
/// name, so a component type whose `functions` carry none has no schema to
/// initialize. See [`ComponentType::is_schema_initialized`].
pub const SCHEMA_INITIALIZER_FUNCTION: &str = "schemaInitializer";

/// The Region a request is served in when it carries no SigV4 credential scope
/// to read one from: the Region its ARNs are built in, and the Region whose
/// store ([`RegionalState`]) answers it.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The most Regions this store holds resources for at once, counted per Region
/// and not per resource: past it a create that would give a further Region its
/// FIRST resource is refused as a full store is, and nothing is written.
///
/// The Region of a request is its own SigV4 credential scope, which the caller
/// chooses, so without a ceiling a caller could repeat a create under fresh
/// Region names until the process ran out of memory, each name carrying its own
/// [`RegionalState`] and its own per-Region room for resources. A partition's
/// Regions are a fixed, published set; AWS publishes no limit of this kind, so
/// the figure is this emulator's own, set well above the largest partition.
pub const MAX_REGIONS: usize = 64;

/// The message a create refused for want of room for a further Region carries:
/// it names the ceiling that was reached, the way the per-Region workspace
/// quota's refusal names its own.
pub fn region_ceiling_message() -> String {
    format!("the limit of {MAX_REGIONS} regions has been reached")
}

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

/// What ONE paginated operation's `maxResults` member documents, and so the two
/// parameters the shared page-size rule ([`page_size`]) is applied with.
///
/// The rule is the same for every paginated operation of this service; only
/// these two numbers differ between them, so they are parameters rather than
/// part of the rule: `ListSyncJobs` documents a default of 50 where the other
/// listings document [`DEFAULT_PAGE_SIZE`], and a member documenting a minimum
/// of 1 refuses the zero a member documenting a minimum of 0 serves as a page of
/// no rows. A rule is built with [`PageSizeRule::minimum_one`] or
/// [`PageSizeRule::minimum_zero`], so its minimum is never negative.
#[derive(Debug, Clone, Copy)]
pub struct PageSizeRule {
    /// The page size served for a request that names no `maxResults`, from the
    /// member's documented default.
    default: usize,
    /// The smallest `maxResults` the member's documentation admits.
    minimum: i32,
}

impl PageSizeRule {
    /// The rule for a `maxResults` documenting a minimum of 1 — an explicit zero
    /// is refused — served `default` when a request names none.
    pub const fn minimum_one(default: usize) -> Self {
        Self {
            default,
            minimum: 1,
        }
    }

    /// The rule for a `maxResults` documenting a minimum of 0 — a zero is the
    /// maximum number of results to return at one time, literally, so it is
    /// served as a page of no rows rather than refused — served `default` when a
    /// request names none.
    pub const fn minimum_zero(default: usize) -> Self {
        Self {
            default,
            minimum: 0,
        }
    }

    /// The refusal message for a `maxResults` of `requested`, which this rule's
    /// minimum does not admit.
    ///
    /// A member documenting a POSITIVE minimum is asked for a smaller value by a
    /// caller that paged at zero, so the message names the minimum and the
    /// default the member can be omitted for. A member documenting a minimum of
    /// 0 admits every value its published range carries, so a value below it
    /// arrives only from a caller that got past that range, and the message names
    /// the range and the value instead.
    fn refusal(self, requested: i32) -> String {
        if self.minimum > 0 {
            format!(
                "maxResults must be at least {}; omit it for the default of {}",
                self.minimum, self.default
            )
        } else {
            format!(
                "maxResults must be between {} and {MAX_PAGE_SIZE}, got {requested}",
                self.minimum
            )
        }
    }
}

/// The page size a paginated operation serves for a request's `maxResults` under
/// `rule`, or the message refusing a value the rule does not admit.
///
/// ONE rule, shared by every paginated operation of this service so the page a
/// caller is served means the same thing wherever it is read: a request naming
/// no `maxResults` is served the operation's documented default — never every
/// row it could report — a value below the member's documented minimum is
/// refused, and anything else is served as asked for, capped at
/// [`MAX_PAGE_SIZE`], the most this service ever serves. The page a handler
/// BUILDS is therefore bounded by this code and not only by the boundary
/// validator.
///
/// The refusal is a message rather than an error: every operation models its own
/// `ValidationException` (HTTP 400), so the caller wraps the message in its own.
pub fn page_size(max_results: Option<i32>, rule: PageSizeRule) -> Result<usize, String> {
    match max_results {
        None => Ok(rule.default),
        Some(requested) if requested < rule.minimum => Err(rule.refusal(requested)),
        // A value below the rule's minimum is already refused above and no rule
        // has a negative minimum, so the ceiling is applied in the type the
        // member arrived in: there is no conversion fallback left for two copies
        // of this rule to disagree about. The CEILING is what decides the answer
        // here; the lower bound is unreachable belt-and-braces, kept only so a
        // value that somehow got past the arm above cannot become an enormous
        // page.
        Some(requested) => Ok(requested.clamp(0, MAX_PAGE_SIZE_I32) as usize),
    }
}

/// [`MAX_PAGE_SIZE`] as the modelled `maxResults` carries it, so a requested page
/// size is capped while it is still the integer the request named.
const MAX_PAGE_SIZE_I32: i32 = MAX_PAGE_SIZE as i32;

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

/// The tag-key prefix Amazon Web Services reserves for the tags it applies
/// itself.
///
/// AWS's tagging rules reserve this prefix across every tagging API: a caller
/// may neither create nor edit nor delete a tag whose key begins with it, so a
/// request naming one is refused and nothing it asked for is applied. The
/// reservation is of the NAMESPACE rather than of one spelling of it, so the
/// prefix is matched however it is cased (see [`is_reserved_tag_key`]).
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` lies in the namespace [`RESERVED_TAG_KEY_PREFIX`] reserves.
///
/// Only the prefix is reserved, so a key that merely CONTAINS it further along,
/// or one that starts with the letters without the colon, is an ordinary
/// caller-owned key. A key too short to carry the prefix cannot be in the
/// namespace, and neither can one whose first bytes are the middle of a
/// multi-byte character — `str::get` answers `None` for both.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// The requirement `keys` fails when any of them lies in the namespace
/// [`RESERVED_TAG_KEY_PREFIX`] reserves, as the message of the refusal that
/// earns, or `None` when every key is the caller's own to write.
///
/// `member` is the member of the request that carries the keys (`tags`,
/// `tagKeys`), so the message is built from the request: it names the member at
/// fault, the keys that failed and the requirement they failed, the same
/// construction as this service's other validation messages. The offending keys
/// are reported in key order, so the refusal does not depend on the iteration
/// order of the map the request was deserialized into.
///
/// Decided on the keys ALONE, so every writer of a tag set — the tagging
/// operations and every create that takes a `tags` map — refuses the whole
/// request before touching the store: a request mixing a legal key with a
/// reserved one applies neither.
pub fn reserved_tag_keys_violation<'a, I>(member: &str, keys: I) -> Option<String>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut reserved: Vec<&str> = keys
        .into_iter()
        .filter(|key| is_reserved_tag_key(key))
        .collect();
    if reserved.is_empty() {
        return None;
    }
    reserved.sort_unstable();
    reserved.dedup();
    Some(format!(
        "{member} names the tag {} {} in the {RESERVED_TAG_KEY_PREFIX} namespace, which is \
         reserved for the tags Amazon Web Services applies itself: such a key cannot be \
         created, changed or removed by a caller.",
        if reserved.len() == 1 { "key" } else { "keys" },
        reserved.join(", "),
    ))
}

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
/// See [`MAX_WORKSPACES`].
pub const MAX_SCENES_PER_WORKSPACE: usize = 1000;

/// The most components one ENTITY holds, the most composite components one of
/// those components holds, and the most properties and property groups one
/// component — top-level or composite — holds, before `CreateEntity` /
/// `UpdateEntity` answer the `ServiceQuotaExceededException` (HTTP 402) each
/// operation models. See [`entity_contents_over_ceiling`], the one rule both
/// writers apply.
///
/// The same reading as [`MAX_WORKSPACES`]: the real service's quotas are
/// per-account and adjustable, so no fixed number is AWS's; these are this
/// emulator's own declared ceilings, so the contents of one entity cannot be
/// grown without bound by a caller that keeps writing components or properties
/// into it. Each sits ABOVE what a read can report
/// ([`MAX_COMPONENTS_RETURNED`], [`MAX_PROPERTIES_RETURNED`]), so a write is
/// never refused merely for exceeding what one response carries.
pub const MAX_COMPONENTS_PER_ENTITY: usize = 100;
/// See [`MAX_COMPONENTS_PER_ENTITY`].
pub const MAX_COMPOSITE_COMPONENTS_PER_COMPONENT: usize = 100;
/// See [`MAX_COMPONENTS_PER_ENTITY`].
pub const MAX_PROPERTIES_PER_COMPONENT: usize = 1000;
/// See [`MAX_COMPONENTS_PER_ENTITY`].
pub const MAX_PROPERTY_GROUPS_PER_COMPONENT: usize = 100;

/// Which of this emulator's declared ceilings the contents of `entity` exceed,
/// as the message a `ServiceQuotaExceededException` carries — `None` when they
/// exceed none.
///
/// Checked over the WHOLE record a writer is about to store rather than over
/// the entries one call carries, so neither a single huge request nor a long
/// series of small ones can grow one entity past the ceilings
/// ([`MAX_COMPONENTS_PER_ENTITY`] and the three beside it). Shared by
/// `CreateEntity` and `UpdateEntity`, so both writers of an entity's contents
/// are bounded by one rule, and applied BEFORE anything is committed.
pub fn entity_contents_over_ceiling(entity: &Entity) -> Option<String> {
    let scope = format!("entity {}", entity.entity_id);
    over(
        &scope,
        "components",
        entity.components.len(),
        MAX_COMPONENTS_PER_ENTITY,
    )
    .or_else(|| {
        entity
            .components
            .iter()
            .find_map(|(component_name, component)| {
                let scope = format!("{scope} component {component_name}");
                over(
                    &scope,
                    "properties",
                    component.properties.len(),
                    MAX_PROPERTIES_PER_COMPONENT,
                )
                .or_else(|| {
                    over(
                        &scope,
                        "property groups",
                        component.property_groups.len(),
                        MAX_PROPERTY_GROUPS_PER_COMPONENT,
                    )
                })
                .or_else(|| {
                    over(
                        &scope,
                        "composite components",
                        component.composite_components.len(),
                        MAX_COMPOSITE_COMPONENTS_PER_COMPONENT,
                    )
                })
                .or_else(|| {
                    component
                        .composite_components
                        .values()
                        .find_map(|composite| {
                            let scope =
                                format!("{scope} compositeComponent {}", composite.component_path);
                            over(
                                &scope,
                                "properties",
                                composite.properties.len(),
                                MAX_PROPERTIES_PER_COMPONENT,
                            )
                            .or_else(|| {
                                over(
                                    &scope,
                                    "property groups",
                                    composite.property_groups.len(),
                                    MAX_PROPERTY_GROUPS_PER_COMPONENT,
                                )
                            })
                        })
                })
            })
    })
}

/// The message [`entity_contents_over_ceiling`] builds for one ceiling, or
/// `None` while `count` is within it.
fn over(scope: &str, kind: &str, count: usize, ceiling: usize) -> Option<String> {
    (count > ceiling).then(|| {
        format!("{scope} would hold {count} {kind}, past this service's limit of {ceiling}")
    })
}
/// See [`MAX_WORKSPACES`]. A metadata transfer job is an ACCOUNT-level
/// resource (`/metadata-transfer-jobs/{metadataTransferJobId}` names no
/// workspace), so its ceiling is per account rather than per workspace, and
/// `CreateMetadataTransferJob` answers the `ServiceQuotaExceededException` it
/// models once the account holds this many.
pub const MAX_METADATA_TRANSFER_JOBS: usize = 1000;

/// The `MetadataTransferJobState` a job the service has just accepted is in,
/// and the terminal state it reaches.
///
/// `VALIDATING` is the state `CreateMetadataTransferJob` answers with: the job
/// has been accepted and nothing has transferred yet, so it is non-terminal and
/// the clock owes its outcome ([`METADATA_TRANSFER_JOB_SETTLE_SPAN`]). `ERROR`
/// is the modelled failure state that outcome is in this emulator — see
/// [`MetadataTransferJob::settle`] — and `CANCELLED`/`CANCELLING` belong to
/// `CancelMetadataTransferJob`, which owns them. The full value list is
/// `METADATA_TRANSFER_JOB_STATE_VALUES` in `ops::types`.
pub const METADATA_TRANSFER_JOB_STATE_VALIDATING: &str = "VALIDATING";
/// See [`METADATA_TRANSFER_JOB_STATE_VALIDATING`].
pub const METADATA_TRANSFER_JOB_STATE_ERROR: &str = "ERROR";

/// How long a metadata transfer job stays in
/// [`METADATA_TRANSFER_JOB_STATE_VALIDATING`] before the clock makes its
/// outcome due.
///
/// AWS documents no duration for a metadata transfer, so this span is CHOSEN by
/// this emulator, the same way (and for the same reasons) as
/// [`COMPONENT_TYPE_SETTLE_SPAN`]: deliberately non-zero, so a caller that
/// reads the job back in the same breath as the create still observes the
/// non-terminal state the create answered with, and deliberately short, so a
/// client that polls reaches the terminal state without a long wait. Nothing
/// sleeps for it: the instant it is due is stored on the record
/// ([`MetadataTransferJob::settles_at`]) and a reader materialises the
/// transition the clock already owed.
pub const METADATA_TRANSFER_JOB_SETTLE_SPAN: Duration = Duration::from_millis(500);

/// The `ErrorCode` a metadata transfer job that never ran carries
/// (`ops::types::ERROR_CODE_VALUES`), and the message that says why.
///
/// Transferring metadata means reading an Amazon S3 object, an IoT SiteWise
/// asset model or another workspace's metadata document and writing what it
/// describes into the destination — work this emulator does not perform.
/// Nothing therefore runs for an accepted job, and a job nothing ran ends in
/// the service's own modelled failure state with the reason that says so,
/// rather than being reported `COMPLETED` for a transfer that never happened.
/// The message is this emulator's, not AWS's: it states the emulator's
/// limitation, which no AWS rule produces.
pub const METADATA_TRANSFER_JOB_ERROR_CODE: &str = "PROCESSING_ERROR";
/// See [`METADATA_TRANSFER_JOB_ERROR_CODE`].
pub const METADATA_TRANSFER_JOB_ERROR_MESSAGE: &str =
    "this emulator transfers no metadata: nothing was read from the job's sources and nothing \
     was written to its destination";

/// The resource portion of the ARN of the service-linked role a metadata
/// transfer job runs under.
///
/// `GetMetadataTransferJob` reports `metadataTransferJobRole` as `@required`,
/// but `CreateMetadataTransferJobRequest` has no role member at all, so the
/// role is the SERVICE's own and never the caller's input: the role IoT
/// TwinMaker assumes to transfer metadata. The ARN is built through
/// [`metadata_transfer_job_role`], so its account segment is the emulator's
/// canonical one.
pub const METADATA_TRANSFER_JOB_ROLE_PATH: &str =
    "role/aws-service-role/iottwinmaker.amazonaws.com/AWSServiceRoleForIoTTwinMaker";

/// The `MetadataTransferJobState` values `CancelMetadataTransferJob` owns, and
/// the third TERMINAL value of the enum.
///
/// `CANCELLING` is the state a cancellation the service has accepted puts the
/// job in: the transfer is being torn down, so it is non-terminal and the clock
/// owes `CANCELLED` ([`METADATA_TRANSFER_JOB_CANCEL_SPAN`]). `CANCELLED` and
/// `COMPLETED` are terminal, as [`METADATA_TRANSFER_JOB_STATE_ERROR`] is:
/// nothing moves a job out of one, so there is nothing left to cancel
/// ([`metadata_transfer_job_state_is_terminal`]). `COMPLETED` is a state no
/// operation of this emulator writes — it is the outcome of a transfer that ran,
/// and none does (see [`METADATA_TRANSFER_JOB_ERROR_CODE`]) — but it is named
/// here because the terminal rule holds for it too. The full value list is
/// `METADATA_TRANSFER_JOB_STATE_VALUES` in `ops::types`.
pub const METADATA_TRANSFER_JOB_STATE_CANCELLING: &str = "CANCELLING";
/// See [`METADATA_TRANSFER_JOB_STATE_CANCELLING`]. TERMINAL.
pub const METADATA_TRANSFER_JOB_STATE_CANCELLED: &str = "CANCELLED";
/// See [`METADATA_TRANSFER_JOB_STATE_CANCELLING`]. TERMINAL.
pub const METADATA_TRANSFER_JOB_STATE_COMPLETED: &str = "COMPLETED";

/// How long a metadata transfer job stays in
/// [`METADATA_TRANSFER_JOB_STATE_CANCELLING`] before the clock makes its
/// transition to [`METADATA_TRANSFER_JOB_STATE_CANCELLED`] due.
///
/// `CancelMetadataTransferJob` answers with the job's status, and the status of
/// a job whose cancellation the service has just accepted is the non-terminal
/// `CANCELLING` — the transfer is being torn down, and the job is `CANCELLED`
/// once that teardown has landed. AWS documents no duration for that window, so
/// this span is CHOSEN by this emulator, exactly as
/// [`METADATA_TRANSFER_JOB_SETTLE_SPAN`] and [`COMPONENT_TYPE_SETTLE_SPAN`] are:
/// deliberately non-zero, so a caller that reads the job back in the same breath
/// as the cancel still observes `CANCELLING`, and deliberately short, so a client
/// that polls reaches `CANCELLED` without a long wait. Nothing sleeps for it: the
/// instant it is due is stored on the record
/// ([`MetadataTransferJob::settles_at`]) and a reader materialises the
/// transition the clock already owed.
pub const METADATA_TRANSFER_JOB_CANCEL_SPAN: Duration = Duration::from_millis(500);

/// The one `SyncSource` this service supports.
///
/// Taken from the model, not chosen here: the `syncSource` member of every sync
/// operation documents "Currently the only supported syncSource is SITEWISE"
/// (`smithy_query(query="operation",operation="CreateSyncJob")`), and the
/// `SyncSource` shape itself is a plain `^[a-zA-Z_0-9]+$` string that cannot say
/// so — so a create naming any other source is refused with the
/// `ValidationException` the operation models rather than stored as a job
/// against a source nothing can sync from.
pub const SYNC_SOURCE_SITEWISE: &str = "SITEWISE";

/// The `SyncJobState` a sync job the service has just accepted is in, and the
/// terminal state it reaches.
///
/// `CREATING` is the state `CreateSyncJob` answers with: the job has been
/// accepted and nothing has been synchronised yet, so it is non-terminal and the
/// clock owes its outcome ([`SYNC_JOB_SETTLE_SPAN`]). `ERROR` is the modelled
/// failure state that outcome is in this emulator — see [`SyncJob::settle`] —
/// while `INITIALIZING` and `ACTIVE` belong to a synchronisation that really
/// ran and `DELETING` to `DeleteSyncJob`, which owns it. The full value list is
/// `SYNC_JOB_STATE_VALUES` in `ops::types`.
pub const SYNC_JOB_STATE_CREATING: &str = "CREATING";
/// See [`SYNC_JOB_STATE_CREATING`].
pub const SYNC_JOB_STATE_ERROR: &str = "ERROR";
/// See [`SYNC_JOB_STATE_CREATING`]. The state `DeleteSyncJob` answers with for
/// the removal it performs: the job is gone from the workspace that held it, and
/// the call reports the removal rather than a job's standing state.
pub const SYNC_JOB_STATE_DELETING: &str = "DELETING";

/// How long a sync job stays in [`SYNC_JOB_STATE_CREATING`] before the clock
/// makes its outcome due.
///
/// AWS documents no duration for bringing a sync job up, so this span is CHOSEN
/// by this emulator, the same way (and for the same reasons) as
/// [`METADATA_TRANSFER_JOB_SETTLE_SPAN`]: deliberately non-zero, so a caller
/// that reads the job back in the same breath as the create still observes the
/// non-terminal state the create answered with, and deliberately short, so a
/// client that polls reaches the terminal state without a long wait. Nothing
/// sleeps for it: the instant it is due is stored on the record
/// ([`SyncJob::settles_at`]) and a reader materialises the transition the clock
/// already owed.
pub const SYNC_JOB_SETTLE_SPAN: Duration = Duration::from_millis(500);

/// The `ErrorCode` a sync job that never synchronised anything carries
/// (`ops::types::ERROR_CODE_VALUES`), and the message that says why.
///
/// Synchronising means reading the sync source — an IoT SiteWise asset model
/// hierarchy — and creating, updating or deleting the corresponding entities
/// and component types in the workspace, which is work this emulator does not
/// perform: it holds no link to the source service. Nothing therefore runs for
/// an accepted job, and a job nothing ran ends in the service's own modelled
/// failure state with the reason that says so, rather than being reported
/// `ACTIVE` for a synchronisation that is not happening. `SYNC_INITIALIZING_ERROR`
/// is the code for a job that failed bringing the sync up, which is where this
/// one stops. The message is this emulator's, not AWS's: it states the
/// emulator's limitation, which no AWS rule produces.
pub const SYNC_JOB_ERROR_CODE: &str = "SYNC_INITIALIZING_ERROR";
/// See [`SYNC_JOB_ERROR_CODE`].
pub const SYNC_JOB_ERROR_MESSAGE: &str =
    "this emulator synchronises nothing: no resource was read from the sync source and none was \
     created, updated or deleted in the workspace";

/// This service's own principal, as an IAM trust policy names it: the principal
/// a role this service has to act as must admit.
///
/// A `syncRole` "is used by the SyncJob to read from the syncSource, and create,
/// update, or delete the corresponding resources"
/// (`smithy_query(query="operation",operation="CreateSyncJob")`), so the service
/// itself assumes that role — which IAM only lets it do when the role's trust
/// policy grants this principal `sts:AssumeRole`. The rule that reads a role's
/// trust policy against this principal lives with the operation that owns the
/// only member naming such a role
/// (`crate::ops::create_sync_job::assume_role_refusal`).
pub const SERVICE_PRINCIPAL: &str = "iottwinmaker.amazonaws.com";

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

/// The `UpdateReason` a plan change records when it replaces a plan whose
/// `pricingMode` differs from the one in force
/// (`ops::types::UPDATE_REASON_VALUES`).
pub const UPDATE_REASON_PRICING_MODE_UPDATE: &str = "PRICING_MODE_UPDATE";
/// The `UpdateReason` a plan change records when the mode is unchanged, so the
/// bundles — and with them the tier the bundles place the account in — are all
/// the change can have moved. See [`UPDATE_REASON_PRICING_MODE_UPDATE`].
pub const UPDATE_REASON_PRICING_TIER_UPDATE: &str = "PRICING_TIER_UPDATE";
/// The `UpdateReason` a plan change records when it supersedes a plan change
/// that had been accepted but had not taken effect: that earlier change is
/// overwritten and never becomes the plan in force. See
/// [`UPDATE_REASON_PRICING_MODE_UPDATE`].
pub const UPDATE_REASON_OVERWRITTEN: &str = "OVERWRITTEN";

/// Whether `pricing_mode` names a BUNDLED pricing plan — one whose billing is
/// described by the bundles it was set with — rather than the pay-as-you-go
/// mode.
///
/// [`PRICING_MODE_BASIC`] is the pay-as-you-go mode, and it is the mode of the
/// plan an untouched account is on, which names no bundle at all
/// ([`PricingPlanRecord::default_plan`]). Every other `PricingMode` value
/// (`ops::types::PRICING_MODE_VALUES`) names a plan bought as bundles, and a
/// bundled plan is reported through `BundleInformation`, whose `bundleNames` is
/// `@required` with `@length(1..10)` — so there is no form of a bundled plan
/// that names no bundle, and no form of the pay-as-you-go plan that names one.
pub fn pricing_mode_is_bundled(pricing_mode: &str) -> bool {
    pricing_mode != PRICING_MODE_BASIC
}

/// Per-service state for com-amazonaws-iottwinmaker-awsiottwinmaker. A handler reaches it through [`service_state`].
///
/// IoT TwinMaker is a REGIONAL service, so this holds ONE store per Region
/// ([`RegionalState`]) rather than one store for the account: a resource exists
/// only in the Region whose endpoint created it, the same id may name a
/// different resource in another Region, and every resource's ARN carries the
/// Region of the store that holds it. A handler picks its store with
/// [`region`](Self::region) (to report) or [`region_mut`](Self::region_mut) (to
/// write), passing the Region of the request's own credential scope
/// ([`request_region`]) — never the Region stored on another record.
#[derive(Debug)]
pub struct ComAmazonawsIottwinmakerAwsiottwinmakerState {
    /// The Regions this account has created something in, keyed by Region.
    ///
    /// Private: a Region's store is reached through
    /// [`region`](Self::region)/[`region_mut`](Self::region_mut), so no caller
    /// can read one Region's resources while serving another's request.
    regions: BTreeMap<String, RegionalState>,
    /// When this account's store came into being.
    ///
    /// Every Region's store dates from here rather than from the first call
    /// that touched that Region, so the pricing plan an untouched Region
    /// reports ([`PricingPlanRecord::default_plan`]) does not move when a
    /// later create materialises the Region's store — a read and the write
    /// after it report one plan.
    created_at: SystemTime,
    /// The read view of a Region nothing has been created in: an empty store,
    /// so a REPORTING operation answers for such a Region (no resources, the
    /// default pricing plan) without materialising anything a later caller
    /// could observe. Never mutated.
    empty_region: RegionalState,
}

impl ComAmazonawsIottwinmakerAwsiottwinmakerState {
    /// The store of `region`, for a caller that REPORTS: the Region's own
    /// store, or an empty one when nothing has been created in that Region.
    pub fn region(&self, region: &str) -> &RegionalState {
        self.regions.get(region).unwrap_or(&self.empty_region)
    }

    /// The store of `region`, for a caller that WRITES: materialised on first
    /// use, since a Region's store comes into being with the first resource
    /// created in it.
    ///
    /// Materialising one is never what GROWS this map: a store holding nothing
    /// a caller put there answers every read exactly as the absence of one does
    /// ([`region`](Self::region)), so the stores left behind by calls that
    /// wrote nothing are dropped here rather than kept — a caller cannot grow
    /// the map by naming Regions it creates nothing in, and what it CAN create
    /// is bounded by [`MAX_REGIONS`] through
    /// [`region_ceiling_reached`](Self::region_ceiling_reached).
    pub fn region_mut(&mut self, region: &str) -> &mut RegionalState {
        let created_at = self.created_at;
        if !self.regions.contains_key(region) {
            let default_plan = PricingPlanRecord::default_plan(created_at);
            self.regions
                .retain(|_, store| !store.is_untouched(&default_plan));
        }
        self.regions
            .entry(region.to_string())
            .or_insert_with(|| RegionalState::new(created_at))
    }

    /// Whether this store has no room to give `region` its FIRST resource: it
    /// already holds resources for [`MAX_REGIONS`] Regions and `region` is not
    /// one of them.
    ///
    /// Checked by every CREATE, which is the only kind of call that gives a
    /// Region its first resource; a Region the store already holds something
    /// for is never refused, and neither is a call that only changes or removes
    /// what is already there.
    pub fn region_ceiling_reached(&self, region: &str) -> bool {
        let default_plan = PricingPlanRecord::default_plan(self.created_at);
        let holds_something = |store: &RegionalState| !store.is_untouched(&default_plan);
        if self.regions.get(region).is_some_and(holds_something) {
            return false;
        }
        self.regions
            .values()
            .filter(|store| holds_something(store))
            .count()
            >= MAX_REGIONS
    }
}

/// One Region's store: everything this service holds for the account in a
/// single Region.
///
/// Keyed by nothing — the Region is the key it is filed under in
/// [`ComAmazonawsIottwinmakerAwsiottwinmakerState::regions`] — so a record in
/// here cannot be reached while serving another Region's request.
#[derive(Debug)]
pub struct RegionalState {
    /// Workspaces by `workspaceId`.
    pub workspaces: BTreeMap<String, Workspace>,
    /// Every ARN this service has minted, so an operation whose input accepts an
    /// id *or* an ARN (`GetWorkspace`, and the tagging operations in a later
    /// slice) resolves one without scanning every workspace.
    pub arn_index: BTreeMap<String, ResourceRef>,
    /// The pricing plan this account is billed under in this Region for the
    /// current billing cycle — the one `GetPricingPlan` reports as
    /// `currentPricingPlan`.
    ///
    /// Region-wide, not per workspace: the pricing operations carry no
    /// workspace in their input, so there is ONE plan per account and Region,
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
    /// changed looks like.
    ///
    /// Nothing in this emulator leaves a change pending: `UpdatePricingPlan`
    /// applies one at once ([`Self::set_pricing_plan`]), so the only thing that
    /// call does to this field is CLEAR it, superseding a change that had not
    /// taken effect. The field stays because the plan in force and the plan
    /// pending are two distinct members of the API's output, and a reader has
    /// to report whichever the store holds rather than assume there is only
    /// ever one.
    pub pending_pricing_plan: Option<PricingPlanRecord>,
    /// The metadata transfer jobs this account holds in this Region, keyed by
    /// `metadataTransferJobId`.
    ///
    /// Region-wide, beside the workspaces rather than inside one: a job is
    /// addressed as `/metadata-transfer-jobs/{metadataTransferJobId}`, which
    /// names no workspace, and one job's source and destination may name
    /// different workspaces (or none at all, for an Amazon S3 or an IoT
    /// SiteWise end), so no single workspace can own it. A job's ARN is
    /// deliberately NOT in [`arn_index`](Self::arn_index): that index exists to
    /// resolve an id-or-ARN input and to find a taggable resource's tag map,
    /// and a metadata transfer job is addressed only by its id and carries no
    /// `tags` member on any of its shapes.
    pub metadata_transfer_jobs: BTreeMap<String, MetadataTransferJob>,
}

impl Default for ComAmazonawsIottwinmakerAwsiottwinmakerState {
    fn default() -> Self {
        // The account's plan dates from the moment the account's store exists,
        // so the default plan's instants are captured here, ONCE, rather than
        // by whichever call happens to read it first — or by whichever Region
        // is touched first.
        let created_at = SystemTime::now();
        Self {
            regions: BTreeMap::new(),
            created_at,
            empty_region: RegionalState::new(created_at),
        }
    }
}

impl RegionalState {
    /// An empty Region store whose default pricing plan dates from
    /// `created_at`, the instant the account's store came into being.
    fn new(created_at: SystemTime) -> Self {
        Self {
            workspaces: BTreeMap::new(),
            arn_index: BTreeMap::new(),
            pricing_plan: PricingPlanRecord::default_plan(created_at),
            pending_pricing_plan: None,
            metadata_transfer_jobs: BTreeMap::new(),
        }
    }

    /// Whether this store holds nothing a caller put in this Region: no
    /// workspace, no metadata transfer job, nothing pending, and still the
    /// `default_plan` an untouched Region is on.
    ///
    /// Such a store answers every read exactly as the absence of one does, so
    /// it is the store [`ComAmazonawsIottwinmakerAwsiottwinmakerState`] may drop
    /// and the one that counts against no Region ceiling. The ARN index is not
    /// consulted: it only ever holds the ARNs of the resources above.
    fn is_untouched(&self, default_plan: &PricingPlanRecord) -> bool {
        self.workspaces.is_empty()
            && self.metadata_transfer_jobs.is_empty()
            && self.pending_pricing_plan.is_none()
            && &self.pricing_plan == default_plan
    }
}

/// One stored pricing plan: the mode and bundles it was set to, and when.
///
/// `billableEntityCount` is NOT a field here. It is the account's current
/// billable entity count, which the stored entities already say
/// ([`RegionalState::billable_entity_count`]) —
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

/// What an ARN in [`RegionalState::arn_index`] points at.
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
    /// A scene, by the workspace that owns it and its own id.
    Scene {
        workspace_id: String,
        scene_id: String,
    },
    /// A sync job, by the workspace that owns it and the sync source that
    /// identifies it inside that workspace.
    SyncJob {
        workspace_id: String,
        sync_source: String,
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
    pub tags: BTreeMap<String, String>,
    pub creation_date_time: SystemTime,
    pub update_date_time: SystemTime,
    /// The entities created in this workspace, keyed by `entityId`.
    pub entities: BTreeMap<String, Entity>,
    /// The component types this workspace holds, keyed by `componentTypeId`.
    /// Owned by the workspace for the same reason entities are: a component
    /// type is addressed as
    /// `/workspaces/{workspaceId}/component-types/{componentTypeId}`, so it
    /// cannot outlive the workspace that contains it, and two operations must
    /// not read two copies of one component type.
    ///
    /// Holds BOTH the component types a caller created here and the service's
    /// own built-in ones, which every workspace is provisioned with when it is
    /// created ([`built_in_component_types`]) — they are addressed through the
    /// same path and read through the same operations, so they live in the same
    /// map rather than in a second one beside it. The two are told apart by the
    /// reserved namespace their ids lie in
    /// ([`built_in_component_type_id`]): see
    /// [`created_component_types`](Workspace::created_component_types) for the
    /// caller's own.
    pub component_types: BTreeMap<String, ComponentType>,
    /// The scenes created in this workspace, keyed by `sceneId`. Owned by the
    /// workspace for the same reason entities and component types are: a scene
    /// is addressed as `/workspaces/{workspaceId}/scenes/{sceneId}`, so it
    /// cannot outlive the workspace that contains it, and two operations must
    /// not read two copies of one scene.
    pub scenes: BTreeMap<String, Scene>,
    /// The sync jobs created in this workspace, keyed by `syncSource`.
    ///
    /// Owned by the workspace for the same reason its scenes are: a sync job is
    /// addressed as `/workspaces/{workspaceId}/sync-jobs/{syncSource}`, so it
    /// cannot outlive the workspace that contains it. The KEY is the sync
    /// source, because that label is the job's whole identity inside its
    /// workspace — the create, the read and the delete all address a job by it
    /// and nothing else — so one workspace holds at most one job per source.
    pub sync_jobs: BTreeMap<String, SyncJob>,
}

impl Workspace {
    /// The services linked into this workspace — what `GetWorkspace` and
    /// `ListWorkspaces` report as `linkedServices`, documented as "a list of
    /// services that are linked to the workspace".
    ///
    /// DERIVED from the workspace's sync jobs rather than stored beside them:
    /// what links another service into a workspace is a sync job against that
    /// service as its source (the sync role "is used by the SyncJob to read from
    /// the syncSource, and create, update, or delete the corresponding
    /// resources"), so the linked services ARE the sources of the jobs the
    /// workspace holds — and deriving them means a job created or deleted after
    /// a read cannot leave a stored list stale, the same choice
    /// [`has_child_entities`](Self::has_child_entities) makes.
    ///
    /// In sync-source order, because the map is keyed by source: two reads of an
    /// unchanged workspace report the same list.
    pub fn linked_services(&self) -> Vec<String> {
        self.sync_jobs.keys().cloned().collect()
    }

    /// The component types a CALLER created in this workspace: the stored ones
    /// whose ids lie outside the service's reserved namespace, which is exactly
    /// what `CreateComponentType` wrote.
    ///
    /// The built-ins every workspace is provisioned with are the SERVICE's own
    /// ([`built_in_component_types`]), not the caller's contents, so a caller
    /// that created none has an empty workspace however many built-ins it
    /// holds: this is what the per-workspace component-type ceiling counts
    /// against and what a `DeleteWorkspace` that requires an emptied workspace
    /// asks about. ONE definition of "the caller's component types", so a
    /// quota, a delete guard and any later reader cannot disagree.
    pub fn created_component_types(&self) -> impl Iterator<Item = &ComponentType> {
        self.component_types
            .values()
            .filter(|component_type| !built_in_component_type_id(&component_type.component_type_id))
    }

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
    /// resolve (a create stores it without checking, and it may name a built-in
    /// type of the real service this emulator does not model), so
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

    /// Whether a declared reference to the component type `component_type_id`
    /// RESOLVES in this workspace — whether the service holds the component
    /// type the reference names.
    ///
    /// A reference resolves when this workspace stores a component type under
    /// the id — which includes the built-ins the workspace was provisioned with
    /// ([`built_in_component_types`]), since a component type is addressed
    /// inside its workspace and a reference never reaches across one — or when
    /// the id lies in the service's reserved namespace
    /// ([`BUILT_IN_COMPONENT_TYPE_ID_PREFIX`]) without being one of the
    /// built-ins this emulator models: the real service ships more of them than
    /// are modelled here, and refusing such an id would reject inheritance the
    /// real service accepts, which is the opposite error from the dangling
    /// reference this rule exists to catch.
    pub fn resolves_component_type(&self, component_type_id: &str) -> bool {
        self.component_types.contains_key(component_type_id)
            || built_in_component_type_id(component_type_id)
    }

    /// The transitive `extendsFrom` closure of `seeds`, in ONE walk: the stored
    /// component types it reaches (breadth-first, nearest first, each at most
    /// once), every id it reached whether stored or not, and whether any of
    /// those ids is NOT stored in this workspace — a built-in type of the real
    /// service that this emulator does not model, whose own definitions it
    /// therefore does not hold.
    ///
    /// The single walk over `extendsFrom` every reader of inherited structure
    /// shares, so an inheritance chain is resolved ONE way:
    /// [`component_type_chain`](Self::component_type_chain) takes the component
    /// types it reached,
    /// [`ancestor_component_type_ids`](Self::ancestor_component_type_ids) their
    /// ids.
    ///
    /// TERMINATION is structural, the same way [`subtree_of`](Self::subtree_of)
    /// gets it: an id is expanded only the first time it is seen and the ids
    /// come from this workspace's own component-type map, so the walk visits at
    /// most one id per stored component type plus the declared parents however
    /// the stored `extendsFrom` values point — a self-extending component type
    /// or an already-stored cycle cannot make it loop.
    fn component_type_closure(
        &self,
        seeds: impl IntoIterator<Item = String>,
    ) -> (Vec<&ComponentType>, BTreeSet<String>, bool) {
        let mut reached: BTreeSet<String> = BTreeSet::new();
        let mut queue: VecDeque<String> = seeds.into_iter().collect();
        let mut chain = Vec::new();
        let mut holds_unstored = false;
        while let Some(ancestor) = queue.pop_front() {
            if !reached.insert(ancestor.clone()) {
                continue;
            }
            match self.component_types.get(&ancestor) {
                Some(stored) => {
                    chain.push(stored);
                    queue.extend(stored.extends_from.iter().cloned());
                }
                None => holds_unstored = true,
            }
        }
        (chain, reached, holds_unstored)
    }

    /// Every component type `extends_from` reaches — the ids it names plus,
    /// transitively, the ids those stored component types themselves extend —
    /// and whether any of them is NOT stored in this workspace, i.e. is a
    /// built-in type of the real service this emulator does not model, whose
    /// own definitions it therefore does not hold.
    pub fn ancestor_component_type_ids(&self, extends_from: &[String]) -> (BTreeSet<String>, bool) {
        let (_, reached, holds_unstored) =
            self.component_type_closure(extends_from.iter().cloned());
        (reached, holds_unstored)
    }

    /// The property names a component type extending `extends_from` INHERITS —
    /// the own definitions of every component type the inheritance chain
    /// reaches — and whether some ancestor's definitions are unknown to this
    /// emulator (a built-in type), in which case a name the set does not carry
    /// cannot be called undefined.
    pub fn inherited_property_names(&self, extends_from: &[String]) -> (BTreeSet<String>, bool) {
        let (ancestors, holds_unstored) = self.ancestor_component_type_ids(extends_from);
        let mut names: BTreeSet<String> = BTreeSet::new();
        for ancestor in &ancestors {
            if let Some(stored) = self.component_types.get(ancestor) {
                names.extend(stored.property_definitions.keys().cloned());
            }
        }
        (names, holds_unstored)
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

    /// The component type `component_type_id` and every component type it
    /// extends, nearest first.
    ///
    /// `extendsFrom` is where a property definition a component type does not
    /// declare itself comes from, so a reader that has to resolve one walks this
    /// chain rather than a single map. The walk is
    /// [`component_type_closure`](Self::component_type_closure)'s, seeded with
    /// the component type itself; a parent this workspace does not hold (a
    /// built-in type of the real service this emulator does not model)
    /// contributes nothing rather than failing the walk.
    pub fn component_type_chain(&self, component_type_id: &str) -> Vec<&ComponentType> {
        let (chain, _, _) = self.component_type_closure([component_type_id.to_string()]);
        chain
    }

    /// Every component type an instance of `component_type_id` may name: the id
    /// itself plus every stored component type that extends it, directly or
    /// through another.
    ///
    /// The scope of a query addressed to a component TYPE, resolved in ONE pass
    /// over this workspace's component types (their `extendsFrom` edges,
    /// reversed) plus a walk of the ids those edges reach, so a scope is
    /// resolved once per request rather than once per component tested against
    /// it. Empty when this workspace holds no such component type, which is
    /// refused before a scope is wanted.
    ///
    /// TERMINATION is structural, as in
    /// [`component_type_closure`](Self::component_type_closure): an id is
    /// expanded only the first time it is seen and every id comes from this
    /// workspace's own component-type map.
    pub fn component_type_instance_ids(&self, component_type_id: &str) -> BTreeSet<&str> {
        let Some(named) = self.component_types.get(component_type_id) else {
            return BTreeSet::new();
        };
        let mut extenders: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for component_type in self.component_types.values() {
            for parent in &component_type.extends_from {
                extenders
                    .entry(parent.as_str())
                    .or_default()
                    .push(component_type.component_type_id.as_str());
            }
        }
        let mut reached: BTreeSet<&str> = BTreeSet::new();
        let mut queue: VecDeque<&str> = VecDeque::from([named.component_type_id.as_str()]);
        while let Some(id) = queue.pop_front() {
            if !reached.insert(id) {
                continue;
            }
            if let Some(children) = extenders.get(id) {
                queue.extend(children.iter().copied());
            }
        }
        reached
    }

    /// The definition that gives one property of a component its meaning: the
    /// component's OWN definition where it declares one, else the definition it
    /// inherits from the component type the component instantiates (or from a
    /// type that type extends).
    ///
    /// A component may carry a property its entity's create never restated —
    /// naming the component's `componentTypeId` is what gives the component that
    /// type's properties — so a reader of a property's definition, its default
    /// value or its data type resolves it through here rather than off the
    /// component alone.
    pub fn effective_property_definition<'a>(
        &'a self,
        component_type_id: Option<&str>,
        properties: &'a BTreeMap<String, PropertyRecord>,
        property_name: &str,
    ) -> Option<&'a crate::ops::types::PropertyDefinitionRequest> {
        if let Some(definition) = properties
            .get(property_name)
            .and_then(|property| property.definition.as_ref())
        {
            return Some(definition);
        }
        self.component_type_chain(component_type_id?)
            .into_iter()
            .find_map(|component_type| component_type.property_definitions.get(property_name))
    }

    /// The property group `group_name` as it applies to a component: the group
    /// the component itself declares, else the one it inherits from the
    /// component type it instantiates — the same resolution
    /// [`effective_property_definition`](Self::effective_property_definition)
    /// makes for a definition.
    pub fn effective_property_group<'a>(
        &'a self,
        component_type_id: Option<&str>,
        property_groups: &'a BTreeMap<String, PropertyGroupRecord>,
        group_name: &str,
    ) -> Option<&'a PropertyGroupRecord> {
        if let Some(group) = property_groups.get(group_name) {
            return Some(group);
        }
        self.component_type_chain(component_type_id?)
            .into_iter()
            .find_map(|component_type| component_type.property_groups.get(group_name))
    }

    /// The external ids a component carries: the value of every property of it
    /// whose EFFECTIVE definition marks the property as the external id — "the
    /// primary key of an external storage system".
    ///
    /// `isExternalId` is a member of the property DEFINITION, and a definition
    /// normally lives on the component TYPE the component instantiates — naming
    /// a `componentTypeId` is what gives a component that type's properties, so
    /// a component usually supplies only the VALUE of its external-id property.
    /// Which property is the external id is therefore resolved through
    /// [`effective_property_definition`](Self::effective_property_definition),
    /// exactly as every other member of a definition is: the component's own
    /// inline definition where it declares one, else the one it inherits from
    /// the component type it instantiates or from a type that type extends. The
    /// value is the one a READ of the property reports
    /// ([`latest_property_value`]), so a property whose value comes from its
    /// definition's `defaultValue` carries the same external id a read of it
    /// shows.
    ///
    /// `properties` are the component's stored properties and
    /// `component_type_id` the type it instantiates, so one component and one
    /// composite component — which carry the same property records — resolve
    /// their external ids the same way. `ListEntities`' `externalId` filter
    /// selects on this.
    ///
    /// The cost is bounded by the component's own properties plus the
    /// definitions on its inheritance chain, which
    /// [`component_type_chain`](Self::component_type_chain) walks at most once
    /// per component type reached.
    pub fn component_external_ids(
        &self,
        component_type_id: Option<&str>,
        properties: &BTreeMap<String, PropertyRecord>,
    ) -> BTreeSet<String> {
        // Every property the component HAS: the ones it stores itself plus the
        // ones its component type gives it and that it never restated — the
        // external-id property is normally one of the latter.
        let mut property_names: BTreeSet<&str> =
            properties.keys().map(String::as_str).collect();
        if let Some(component_type_id) = component_type_id {
            for component_type in self.component_type_chain(component_type_id) {
                property_names.extend(
                    component_type
                        .property_definitions
                        .keys()
                        .map(String::as_str),
                );
            }
        }
        let mut external_ids = BTreeSet::new();
        for property_name in property_names {
            let Some(definition) =
                self.effective_property_definition(component_type_id, properties, property_name)
            else {
                continue;
            };
            if !definition.is_external_id.unwrap_or(false) {
                continue;
            }
            if let Some(value) = latest_property_value(properties.get(property_name), Some(definition))
            {
                // Only a STRING value is an external id: `externalId` on the
                // filter is a string, so a property of any other data type
                // carries nothing to match against.
                if let Some(string_value) = value.string_value {
                    external_ids.insert(string_value);
                }
            }
        }
        external_ids
    }
}

/// One stored scene.
///
/// A scene has no lifecycle `state` member anywhere in its API — `CreateScene`
/// answers only the ARN and the creation instant, and `GetScene` reports no
/// status — so a scene is READY as soon as it is created and there is no
/// transition for the clock to owe. Its only reported failure channel is
/// `GetScene`'s `error` (`SceneError`), whose single modelled code
/// (`MATTERPORT_ERROR`) belongs to a Matterport import this emulator never
/// performs, so a scene it stores carries none.
#[derive(Debug, Clone)]
pub struct Scene {
    pub scene_id: String,
    pub arn: String,
    pub workspace_id: String,
    /// The `s3://…` path of the scene's content definition file. Required on
    /// both the create and the read, so it is stored, never derived.
    pub content_location: String,
    /// The scene's description, absent when the create named none:
    /// `description` is optional on `CreateSceneRequest` AND on
    /// `GetSceneResponse`, so unlike a workspace's it is carried as an option
    /// rather than resolved to the empty string.
    pub description: Option<String>,
    /// The capabilities the scene renders itself with, in the order the create
    /// gave them: `SceneCapabilities` is a LIST, so its order is part of the
    /// value.
    pub capabilities: Vec<String>,
    /// The scene metadata the caller supplied (`sceneMetadata`), which
    /// `GetScene` reports back as the response metadata.
    pub scene_metadata: BTreeMap<String, String>,
    pub tags: BTreeMap<String, String>,
    pub creation_date_time: SystemTime,
    pub update_date_time: SystemTime,
}

/// One stored sync job.
///
/// A sync job is a workspace's standing link to another service: its
/// `syncSource` says which service's resources are synchronised into the
/// workspace, and its `syncRole` is the role the synchronisation would run
/// under. The source is the job's identity inside the workspace, so it is the
/// key of [`Workspace::sync_jobs`] and is not repeated here.
///
/// Like a [`MetadataTransferJob`], the job is accepted in a non-terminal state
/// and carries the instant its outcome falls due ([`settles_at`](Self::settles_at)),
/// which a reader materialises rather than sleeps for.
#[derive(Debug, Clone)]
pub struct SyncJob {
    pub sync_source: String,
    pub arn: String,
    pub workspace_id: String,
    /// The ARN of the IAM role the create supplied: "this IAM role is used by
    /// the SyncJob to read from the syncSource, and create, update, or delete
    /// the corresponding resources". Required on both the create and the read,
    /// so it is stored, never derived.
    pub sync_role: String,
    /// The tags the create supplied. `CreateSyncJob` accepts a `tags` map, so a
    /// sync job is a taggable resource of this service: its ARN is indexed in
    /// [`RegionalState::arn_index`] and the
    /// tagging operations reach this map through it, exactly as they reach a
    /// scene's.
    pub tags: BTreeMap<String, String>,
    pub creation_date_time: SystemTime,
    /// When the job last changed — the create, until the clock settles its
    /// outcome, which moves it to the instant that outcome fell due.
    pub update_date_time: SystemTime,
    /// The job's `SyncJobState`, as STORED: [`SYNC_JOB_STATE_CREATING`] from the
    /// create until [`settles_at`](Self::settles_at) passes, then the terminal
    /// state [`settle`](Self::settle) writes.
    pub state: String,
    /// Why the job is in a failed state, and `None` while nothing has failed.
    pub error: Option<crate::ops::types::ErrorDetails>,
    /// The instant the transition out of [`SYNC_JOB_STATE_CREATING`] falls due
    /// — the instant of the create plus [`SYNC_JOB_SETTLE_SPAN`]. Stored, never
    /// slept for, so a reader can materialise a transition the clock already
    /// owed without any caller having had to poll.
    pub settles_at: SystemTime,
}

impl SyncJob {
    /// The job a `CreateSyncJob` call accepted at `now`: accepted, nothing
    /// synchronised yet, and its outcome due [`SYNC_JOB_SETTLE_SPAN`] later.
    pub fn accepted(
        sync_source: String,
        arn: String,
        workspace_id: String,
        sync_role: String,
        tags: BTreeMap<String, String>,
        now: SystemTime,
    ) -> Self {
        Self {
            sync_source,
            arn,
            workspace_id,
            sync_role,
            tags,
            creation_date_time: now,
            // Nothing has changed the job yet, so it was last updated when it
            // was created.
            update_date_time: now,
            state: SYNC_JOB_STATE_CREATING.to_string(),
            error: None,
            settles_at: now + SYNC_JOB_SETTLE_SPAN,
        }
    }

    /// Materialise the lifecycle transition the CLOCK has already made due.
    ///
    /// Called by a reader before it reports this job, exactly as
    /// [`MetadataTransferJob::settle`] is: the outcome was owed at
    /// [`settles_at`](Self::settles_at) whether or not anybody called, so
    /// writing it here makes the record say what was already true rather than
    /// changing anything a read caused.
    ///
    /// The outcome is the modelled failure state: nothing was synchronised,
    /// because this emulator synchronises nothing (see [`SYNC_JOB_ERROR_CODE`]),
    /// so the job carries the error that says so instead of an `ACTIVE` nothing
    /// earned. Idempotent, a no-op before the instant falls due, and a no-op on
    /// a job some other operation has already moved out of the non-terminal
    /// state.
    pub fn settle(&mut self, now: SystemTime) {
        if self.state != SYNC_JOB_STATE_CREATING || now < self.settles_at {
            return;
        }
        self.state = SYNC_JOB_STATE_ERROR.to_string();
        self.error = Some(crate::ops::types::ErrorDetails {
            code: Some(SYNC_JOB_ERROR_CODE.to_string()),
            message: Some(SYNC_JOB_ERROR_MESSAGE.to_string()),
        });
        // The change fell due at `settles_at`, not at the instant a caller
        // happened to ask, so that is when the job was last updated: two reads
        // of a settled job report the same instant.
        self.update_date_time = self.settles_at;
    }

    /// The job's modelled `SyncJobStatus`: its stored state and, when it has
    /// failed, why.
    ///
    /// The `error` structure is reported WHOLE, the way every other status of
    /// this service is (see [`status`]): a job that has not failed carries an
    /// `ErrorDetails` with neither a code nor a message, so a client that reads
    /// `status.error` needs no null guard.
    pub fn status(&self) -> crate::ops::types::SyncJobStatus {
        crate::ops::types::SyncJobStatus {
            state: Some(self.state.clone()),
            error: Some(self.error.clone().unwrap_or_default()),
        }
    }
}

/// One stored metadata transfer job.
///
/// The job's `sources` and `destination` are kept as the modelled REQUEST
/// shapes the caller supplied — the same choice [`ComponentType`] makes for a
/// component type's definitions — so a read reports back exactly what was
/// persisted rather than a lossy projection of it.
#[derive(Debug, Clone)]
pub struct MetadataTransferJob {
    pub metadata_transfer_job_id: String,
    pub arn: String,
    /// The job's description, absent when the create named none: `description`
    /// is optional on `CreateMetadataTransferJobRequest` AND on
    /// `GetMetadataTransferJobResponse`, so it is carried as an option rather
    /// than resolved to the empty string, as a [`Scene`]'s is.
    pub description: Option<String>,
    /// The job's sources, in the order the create gave them
    /// (`SourceConfigurations` is a `@length(1..1)` LIST).
    pub sources: Vec<crate::ops::types::SourceConfiguration>,
    pub destination: crate::ops::types::DestinationConfiguration,
    /// The ARN of the role the job runs under
    /// ([`metadata_transfer_job_role`]): the service's own, since the create
    /// accepts no role.
    pub metadata_transfer_job_role: String,
    pub creation_date_time: SystemTime,
    /// When the job last changed — the create, until the clock settles its
    /// outcome, which moves it to the instant that outcome fell due.
    pub update_date_time: SystemTime,
    /// The job's `MetadataTransferJobState`, as STORED:
    /// [`METADATA_TRANSFER_JOB_STATE_VALIDATING`] from the create until
    /// [`settles_at`](Self::settles_at) passes, then the terminal state
    /// [`settle`](Self::settle) writes.
    pub state: String,
    /// Why the job is in a failed state, and `None` while nothing has failed.
    pub error: Option<crate::ops::types::ErrorDetails>,
    /// The instant the transition out of the job's current NON-TERMINAL state
    /// falls due — the instant of the create plus
    /// [`METADATA_TRANSFER_JOB_SETTLE_SPAN`] for the outcome a created job is
    /// owed, or the instant of the cancel plus
    /// [`METADATA_TRANSFER_JOB_CANCEL_SPAN`] once a cancellation has been
    /// accepted ([`begin_cancel`](Self::begin_cancel)). Stored, never slept
    /// for, so a reader can materialise a transition the clock already owed
    /// without any caller having had to poll.
    pub settles_at: SystemTime,
    /// The `MetadataTransferJobState` the transition due at
    /// [`settles_at`](Self::settles_at) puts the job in:
    /// [`METADATA_TRANSFER_JOB_STATE_ERROR`] for the outcome of a job nothing
    /// transferred, or [`METADATA_TRANSFER_JOB_STATE_CANCELLED`] once a
    /// cancellation has been accepted. It is read only while the job is
    /// non-terminal: a job that has settled owes nothing more.
    pub settles_to: String,
}

impl MetadataTransferJob {
    /// The job a `CreateMetadataTransferJob` call accepted at `now`: accepted,
    /// nothing transferred yet, and its outcome due
    /// [`METADATA_TRANSFER_JOB_SETTLE_SPAN`] later.
    pub fn accepted(
        metadata_transfer_job_id: String,
        arn: String,
        description: Option<String>,
        sources: Vec<crate::ops::types::SourceConfiguration>,
        destination: crate::ops::types::DestinationConfiguration,
        now: SystemTime,
    ) -> Self {
        Self {
            metadata_transfer_job_id,
            arn,
            description,
            sources,
            destination,
            metadata_transfer_job_role: metadata_transfer_job_role(),
            creation_date_time: now,
            // Nothing has changed the job yet, so it was last updated when it
            // was created.
            update_date_time: now,
            state: METADATA_TRANSFER_JOB_STATE_VALIDATING.to_string(),
            error: None,
            settles_at: now + METADATA_TRANSFER_JOB_SETTLE_SPAN,
            // The outcome an accepted job is owed: the modelled failure state,
            // because this emulator transfers no metadata.
            settles_to: METADATA_TRANSFER_JOB_STATE_ERROR.to_string(),
        }
    }

    /// Whether this job has finished for good: see
    /// [`metadata_transfer_job_state_is_terminal`]. A job that has not is one
    /// the clock still owes the transition at [`settles_at`](Self::settles_at).
    pub fn is_terminal(&self) -> bool {
        metadata_transfer_job_state_is_terminal(&self.state)
    }

    /// Materialise the lifecycle transition the CLOCK has already made due.
    ///
    /// Called by a reader before it reports this job, exactly as
    /// [`ComponentType::settle`] is: the outcome was owed at
    /// [`settles_at`](Self::settles_at) whether or not anybody called, so
    /// writing it here makes the record say what was already true rather than
    /// changing anything a read caused.
    ///
    /// The outcome of a CREATED job is the modelled failure state: no metadata
    /// was transferred, because this emulator transfers none (see
    /// [`METADATA_TRANSFER_JOB_ERROR_CODE`]), so the job carries the error that
    /// says so instead of a `COMPLETED` nothing earned. The outcome of a job
    /// whose CANCELLATION was accepted is
    /// [`METADATA_TRANSFER_JOB_STATE_CANCELLED`], which carries no error:
    /// nothing failed, a caller asked the transfer to stop. Which of the two is
    /// due is [`settles_to`](Self::settles_to). Idempotent, a no-op before the
    /// instant falls due, and a no-op on a job that has already settled.
    pub fn settle(&mut self, now: SystemTime) {
        if self.is_terminal() || now < self.settles_at {
            return;
        }
        self.state = self.settles_to.clone();
        if self.state == METADATA_TRANSFER_JOB_STATE_ERROR {
            self.error = Some(crate::ops::types::ErrorDetails {
                code: Some(METADATA_TRANSFER_JOB_ERROR_CODE.to_string()),
                message: Some(METADATA_TRANSFER_JOB_ERROR_MESSAGE.to_string()),
            });
        }
        // The change fell due at `settles_at`, not at the instant a caller
        // happened to ask, so that is when the job was last updated: two reads
        // of a settled job report the same instant.
        self.update_date_time = self.settles_at;
    }

    /// Accept a cancellation at `now`: the job leaves whichever non-terminal
    /// state it was in for the non-terminal
    /// [`METADATA_TRANSFER_JOB_STATE_CANCELLING`] — the teardown is in flight —
    /// and the instant the clock owes
    /// [`METADATA_TRANSFER_JOB_STATE_CANCELLED`] is stored beside it, never
    /// slept for. See [`settle`](Self::settle).
    ///
    /// A job that has FINISHED is left exactly as it is, and so is one whose
    /// cancellation the service has ALREADY accepted: the transition the clock
    /// already owed is materialised first, a job that is terminal once it has
    /// has nothing left to cancel, and a job already in
    /// [`METADATA_TRANSFER_JOB_STATE_CANCELLING`] is being torn down under the
    /// instant the clock already owes it — so neither that instant nor
    /// `updateDateTime` moves when the same cancellation arrives twice. Both
    /// rules live here rather than in the caller;
    /// `CancelMetadataTransferJob` tests terminality too, because that is what
    /// it answers the caller's `ConflictException` for.
    pub fn begin_cancel(&mut self, now: SystemTime) {
        self.settle(now);
        if self.is_terminal() || self.state == METADATA_TRANSFER_JOB_STATE_CANCELLING {
            return;
        }
        self.state = METADATA_TRANSFER_JOB_STATE_CANCELLING.to_string();
        self.update_date_time = now;
        self.settles_at = now + METADATA_TRANSFER_JOB_CANCEL_SPAN;
        self.settles_to = METADATA_TRANSFER_JOB_STATE_CANCELLED.to_string();
    }

    /// The job's modelled `MetadataTransferJobStatus`: its stored state and,
    /// when it has failed, why.
    ///
    /// The `error` structure is reported WHOLE, the way every other status of
    /// this service is (see [`status`]): a job that has not failed carries an
    /// `ErrorDetails` with neither a code nor a message, so a client that reads
    /// `status.error` needs no null guard.
    ///
    /// `queuedPosition` is left absent: it is a job's place in the queue of
    /// jobs waiting to run, and this emulator queues no transfer behind
    /// another, so there is no position to report.
    pub fn status(&self) -> crate::ops::types::MetadataTransferJobStatus {
        crate::ops::types::MetadataTransferJobStatus {
            state: Some(self.state.clone()),
            error: Some(self.error.clone().unwrap_or_default()),
            queued_position: None,
        }
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
    /// The entity's `State`, as the WRITER that last touched the record left
    /// it: [`STATE_ACTIVE`] from a create, which settles the entity inside the
    /// call that asked for it (see that constant), or [`STATE_UPDATING`] from
    /// an update, until [`settles_at`](Self::settles_at) passes.
    pub state: String,
    /// The instant the transition out of [`STATE_UPDATING`] falls due — the
    /// instant of the call that opened the window plus [`ENTITY_SETTLE_SPAN`].
    /// Stored, never slept for, so a read reports the transition the clock
    /// already owed without any caller having had to poll. Irrelevant while the
    /// stored state is terminal, which is how a create leaves it.
    pub settles_at: SystemTime,
}

impl Entity {
    /// The entity's `State` AS OF `now`: the stored state, with the transition
    /// the CLOCK has already made due applied.
    ///
    /// The `UPDATING` -> `ACTIVE` transition was owed at
    /// [`settles_at`](Self::settles_at) whether or not anybody called, so
    /// reporting it is saying what was already true. Computed rather than
    /// written back, so a read of an entity leaves the store exactly as it
    /// found it.
    pub fn state_at(&self, now: SystemTime) -> &str {
        if self.state == STATE_UPDATING && now >= self.settles_at {
            STATE_ACTIVE
        } else {
            &self.state
        }
    }

    /// The entity's modelled `Status` as of `now`: its state
    /// ([`state_at`](Self::state_at)), with the `error` structure every reported
    /// status carries. See [`status`].
    pub fn status(&self, now: SystemTime) -> crate::ops::types::Status {
        status(self.state_at(now))
    }

    /// Open the window an UPDATE owes.
    ///
    /// The update has landed on the record by the time this is called, so its
    /// `updateDateTime` moves to the instant of the call that made the change;
    /// its state leaves [`STATE_ACTIVE`] for the non-terminal
    /// [`STATE_UPDATING`] the update answers with, and the instant the clock
    /// owes the return to [`STATE_ACTIVE`] is stored beside it — never slept
    /// for. See [`state_at`](Self::state_at).
    pub fn begin_update(&mut self, now: SystemTime) {
        self.update_date_time = now;
        self.state = STATE_UPDATING.to_string();
        self.settles_at = now + ENTITY_SETTLE_SPAN;
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
    /// "Whether the component type is abstract" — a base type an entity holds
    /// no component of, which exists to be extended.
    ///
    /// STORED rather than derived at each reader, so the value
    /// `GetComponentType` reports, the set a `ListComponentTypes` `isAbstract`
    /// filter selects and the component types `CreateEntity` refuses to
    /// instantiate cannot drift apart. `isAbstract` is not a member of
    /// `CreateComponentTypeRequest` — a caller does not declare it — so a
    /// writer DECIDES it from what the request declares
    /// ([`component_type_is_abstract`]) and stores the answer beside the
    /// record; the service's own built-ins carry the abstractness the service
    /// publishes for them, which the workspace is provisioned with rather than
    /// asked for ([`built_in_component_types`]).
    pub is_abstract: bool,
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

/// Whether a `MetadataTransferJobState` is TERMINAL — a state the job has
/// finished in and never leaves, so there is nothing left for a cancellation to
/// stop.
///
/// The model's `MetadataTransferJobState` enum
/// (`ops::types::METADATA_TRANSFER_JOB_STATE_VALUES`) holds three such values:
/// the transfer ran to the end (`COMPLETED`), it was cancelled (`CANCELLED`),
/// or it failed (`ERROR`). Every other value is a state the job is still
/// moving through.
pub fn metadata_transfer_job_state_is_terminal(state: &str) -> bool {
    matches!(
        state,
        METADATA_TRANSFER_JOB_STATE_COMPLETED
            | METADATA_TRANSFER_JOB_STATE_CANCELLED
            | METADATA_TRANSFER_JOB_STATE_ERROR
    )
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

/// One stored property group of a COMPONENT as the modelled
/// `ComponentPropertyGroupResponse`.
///
/// Shared by every reader of a component's property groups (`GetEntity`,
/// `ListComponents`), so one group is reported the same way wherever it is read.
/// `groupType` is required on the response but optional on the request, so a
/// group stored without one reports [`TABULAR_GROUP_TYPE`]; nothing is inherited
/// here, which is why `isInherited` is false (see [`property_definition_response`]).
/// A component TYPE's own groups are a different modelled shape
/// (`PropertyGroupResponse`), which `GetComponentType` builds.
pub fn component_property_group_response(
    group: &PropertyGroupRecord,
) -> crate::ops::types::ComponentPropertyGroupResponse {
    crate::ops::types::ComponentPropertyGroupResponse {
        group_type: group
            .group_type
            .clone()
            .unwrap_or_else(|| TABULAR_GROUP_TYPE.to_string()),
        property_names: group.property_names.clone(),
        is_inherited: false,
    }
}

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

/// The names under which a component type's `functions` map carries a DATA
/// CONNECTOR — the function that fetches, or writes, the values of a property
/// whose values live outside the service.
///
/// The map's keys are free-form in the model (`functions` is a
/// `map<String, FunctionRequest>`), so these are the names the AWS IoT
/// TwinMaker user guide publishes for a component type's data connectors:
/// a reader at entity scope, a reader at component-type scope, the plain
/// reader, and the writer. The schema initializer
/// ([`SCHEMA_INITIALIZER_FUNCTION`]) is NOT one of them: it initializes the
/// component type's schema and serves no property's values.
pub const DATA_CONNECTOR_FUNCTION_NAMES: &[&str] = &[
    "dataReader",
    "dataReaderByEntity",
    "dataReaderByComponentType",
    "dataWriter",
];

/// Whether `definition` declares a property whose values live OUTSIDE this
/// service — `isStoredExternally`, "a Boolean value that specifies whether the
/// property is stored externally" (the member's own documentation), resolved so
/// an absent member reads as `false`.
pub fn property_is_stored_externally(
    definition: &crate::ops::types::PropertyDefinitionRequest,
) -> bool {
    definition.is_stored_externally.unwrap_or(false)
}

/// Whether a component type is ABSTRACT — whether it is a base type that exists
/// to be extended rather than instantiated, which is what `isAbstract`, "a
/// Boolean value that specifies whether the component type is abstract",
/// reports.
///
/// A component type is abstract exactly when it has no way to serve the values
/// of some property it declares: it carries an externally stored property
/// ([`property_is_stored_externally`]) and no data connector
/// ([`DATA_CONNECTOR_FUNCTION_NAMES`]) that could read or write one. Such a
/// type is incomplete on its own — a type extending it supplies the connector —
/// so no component instantiates it.
///
/// Both halves are resolved over the record AND the inheritance it declares
/// (`extendsFrom`, walked by [`Workspace::component_type_chain`]): a property
/// inherited from a parent is as much the type's as one it restates, and a
/// connector inherited from a parent serves it just as well. `record` is passed
/// in rather than looked up, so the rule holds for a component type a create
/// has not stored yet and for one an update has patched on a copy.
pub fn component_type_is_abstract(workspace: &Workspace, record: &ComponentType) -> bool {
    let inherited = record
        .extends_from
        .iter()
        .flat_map(|parent| workspace.component_type_chain(parent));
    let mut stores_externally = false;
    let mut has_connector = false;
    for component_type in std::iter::once(record).chain(inherited) {
        stores_externally |= component_type
            .property_definitions
            .values()
            .any(property_is_stored_externally);
        has_connector |= component_type
            .functions
            .keys()
            .any(|function_name| DATA_CONNECTOR_FUNCTION_NAMES.contains(&function_name.as_str()));
    }
    stores_externally && !has_connector
}

/// Why a component of the component type `component_type_id` cannot be created,
/// as the message a `ValidationException` carries: the component type is
/// ABSTRACT ([`component_type_is_abstract`]), so it is extended rather than
/// instantiated. Built from the request — it names the component type the call
/// asked for — so a caller is told which one to change.
pub fn abstract_component_type_message(component_type_id: &str) -> String {
    format!("Abstract ComponentType {component_type_id} can not be used for creating a component")
}

/// Why a component instantiating `component_type_id` cannot be written — `None`
/// when it can — as the message a `ValidationException` carries.
///
/// The component type is resolved in the WORKSPACE's own registry, which holds
/// both the component types a caller created and the built-ins the workspace was
/// provisioned with. A component INSTANTIATES the type its `componentTypeId`
/// names, so that type has to be one that can be instantiated at all: an
/// ABSTRACT one cannot, which is what makes it a base type to extend rather than
/// one to hold a component of ([`abstract_component_type_message`]). An id the
/// workspace does not hold is left alone — a real workspace carries more
/// built-in component types than this emulator models, so refusing one would
/// reject writes the real service accepts — and a component that names no
/// component type instantiates nothing and so has nothing to resolve.
///
/// Shared by every writer of an entity's components, top-level and composite
/// alike, so a create and an update hold one path to one rule.
pub fn abstract_component_type_violation(
    workspace: &Workspace,
    component_type_id: Option<&str>,
) -> Option<String> {
    let component_type_id = component_type_id?;
    let component_type = workspace.component_types.get(component_type_id)?;
    component_type
        .is_abstract
        .then(|| abstract_component_type_message(component_type_id))
}

/// The id prefix the service's own BUILT-IN component types carry.
///
/// IoT TwinMaker publishes the component types it ships under the reserved
/// `com.amazon.` namespace — `com.amazon.iottwinmaker.alarm.basic`,
/// `com.amazon.iotsitewise.connector` and the rest of the user guide's
/// "built-in component types" — while a caller's own ids live outside it. The
/// namespace is therefore the SERVICE's: the component types in it are
/// provisioned with every workspace ([`built_in_component_types`]) and no
/// caller creates, changes or deletes one. An id inside it that this emulator
/// does not model still RESOLVES as a reference rather than being refused: the
/// real service ships more built-ins than are modelled here, and refusing one
/// would reject inheritance the real service accepts, which is the opposite
/// error from the dangling reference that rule exists to catch.
pub const BUILT_IN_COMPONENT_TYPE_ID_PREFIX: &str = "com.amazon.";

/// Whether `component_type_id` names one of the service's built-in component
/// types. See [`BUILT_IN_COMPONENT_TYPE_ID_PREFIX`].
pub fn built_in_component_type_id(component_type_id: &str) -> bool {
    component_type_id.starts_with(BUILT_IN_COMPONENT_TYPE_ID_PREFIX)
}

/// The `Type` values ([`crate::ops::types::TYPE_VALUES`]) the built-in
/// catalogue's property definitions carry.
const TYPE_STRING: &str = "STRING";
/// See [`TYPE_STRING`].
const TYPE_MAP: &str = "MAP";

/// One entry of the built-in catalogue: a component type IoT TwinMaker SHIPS,
/// as this emulator models it. See [`built_in_component_types`].
struct BuiltInComponentType {
    /// Its published `componentTypeId`, inside
    /// [`BUILT_IN_COMPONENT_TYPE_ID_PREFIX`].
    component_type_id: &'static str,
    /// Whether it is abstract — see [`ComponentType::is_abstract`]. The
    /// abstractness the SERVICE publishes for the built-in, carried as declared
    /// rather than decided by [`component_type_is_abstract`]: a built-in is
    /// provisioned with the workspace, and no caller can create or update one
    /// in the reserved namespace, so no writer of a component type's
    /// definitions or functions ever reaches a built-in's record.
    is_abstract: bool,
    /// The property definitions it declares itself, which a component type
    /// extending it inherits.
    property_definitions: &'static [BuiltInProperty],
}

/// One property definition of a built-in component type.
struct BuiltInProperty {
    /// The property's name, as the service publishes it.
    name: &'static str,
    /// Its `dataType`'s `type`: a [`crate::ops::types::TYPE_VALUES`] value.
    data_type: &'static str,
    /// The `nestedType`'s `type` for a container `data_type`, `None` for a
    /// scalar one.
    nested_type: Option<&'static str>,
    /// Whether the property "ID comes from an external data store"
    /// (`isExternalId`): what names the resource of the linked service the
    /// built-in connects a component to.
    is_external_id: bool,
    /// Whether the property "consists of time series data" (`isTimeSeries`).
    is_time_series: bool,
}

impl BuiltInProperty {
    /// This declaration as the modelled `PropertyDefinitionRequest` a stored
    /// component type carries, so a built-in's definitions are stored and read
    /// back exactly like a created one's.
    fn definition(&self) -> crate::ops::types::PropertyDefinitionRequest {
        crate::ops::types::PropertyDefinitionRequest {
            data_type: Some(crate::ops::types::DataType {
                r#type: self.data_type.to_string(),
                nested_type: self.nested_type.map(|nested| {
                    Box::new(crate::ops::types::DataType {
                        r#type: nested.to_string(),
                        ..Default::default()
                    })
                }),
                ..Default::default()
            }),
            is_external_id: Some(self.is_external_id),
            is_time_series: Some(self.is_time_series),
            ..Default::default()
        }
    }
}

/// The built-in component types this emulator models, from the AWS IoT
/// TwinMaker user guide's "built-in component types": the IoT SiteWise
/// connector a workspace's own types extend to read an asset's property values,
/// the basic alarm type the service's alarm pattern extends, and the document
/// type an entity carries document links in.
///
/// The service ships these; nothing creates them through the API, which is why
/// [`built_in_component_types`] provisions a workspace with them and
/// `CreateComponentType` refuses the namespace they live in. The set is the one
/// the user guide publishes and this emulator models — an id inside the
/// reserved namespace that is NOT here still resolves as a reference (see
/// [`BUILT_IN_COMPONENT_TYPE_ID_PREFIX`]) rather than being refused.
const BUILT_IN_COMPONENT_TYPES: &[BuiltInComponentType] = &[
    // The IoT SiteWise connector. ABSTRACT: a workspace's own component type
    // extends it and the entity's component then names the SiteWise asset and
    // asset model the component reads its values from, so the connector itself
    // is never instantiated. Its two properties are those external ids.
    BuiltInComponentType {
        component_type_id: "com.amazon.iotsitewise.connector",
        is_abstract: true,
        property_definitions: &[
            BuiltInProperty {
                name: "sitewiseAssetModelId",
                data_type: TYPE_STRING,
                nested_type: None,
                is_external_id: true,
                is_time_series: false,
            },
            BuiltInProperty {
                name: "sitewiseAssetId",
                data_type: TYPE_STRING,
                nested_type: None,
                is_external_id: true,
                is_time_series: false,
            },
        ],
    },
    // The basic alarm. ABSTRACT: it is the parent the service's documented
    // alarm pattern declares in `extendsFrom`, so a workspace's own alarm type
    // inherits the alarm key that names the alarm in the system that raises it
    // and the alarm status that system reports over time.
    BuiltInComponentType {
        component_type_id: "com.amazon.iottwinmaker.alarm.basic",
        is_abstract: true,
        property_definitions: &[
            BuiltInProperty {
                name: "alarm_key",
                data_type: TYPE_STRING,
                nested_type: None,
                is_external_id: true,
                is_time_series: false,
            },
            BuiltInProperty {
                name: "alarm_status",
                data_type: TYPE_STRING,
                nested_type: None,
                is_external_id: false,
                is_time_series: true,
            },
        ],
    },
    // The document type. CONCRETE: an entity holds a component of it directly,
    // whose one property maps each document's name to its location.
    BuiltInComponentType {
        component_type_id: "com.amazon.iottwinmaker.documents",
        is_abstract: false,
        property_definitions: &[BuiltInProperty {
            name: "documents",
            data_type: TYPE_MAP,
            nested_type: Some(TYPE_STRING),
            is_external_id: false,
            is_time_series: false,
        }],
    },
];

/// The built-in component types a workspace created in `region` under
/// `workspace_id` at `at` is PROVISIONED with, keyed by `componentTypeId` — the
/// value `CreateWorkspace` stores in [`Workspace::component_types`].
///
/// The service's own component types are not created through this API: they
/// exist in a workspace from the instant it does, which is why a workspace is
/// seeded with them rather than a reader synthesising them. Each is stored as
/// an ordinary [`ComponentType`] record — same ARN minting, same reported
/// members — so `GetComponentType`, `ListComponentTypes` and every walk of an
/// inheritance chain reach a built-in exactly as they reach a created one, and
/// a component type that extends one really has a parent whose definitions can
/// be read.
///
/// Each record is ACTIVE from that same instant, with no transition owed: a
/// provisioned component type has no create in flight for the clock to settle
/// ([`ComponentType::settle`] is a no-op on it). It carries no tags and no
/// friendly name or description — this emulator does not invent the service's
/// prose — and its ARN is deliberately NOT put in
/// [`RegionalState::arn_index`], for the same
/// reason a metadata transfer job's is not: that index resolves an id-or-ARN
/// input and finds a TAGGABLE resource's tag map, and a built-in is the
/// service's own resource, which carries no caller's tags.
pub fn built_in_component_types(
    region: &str,
    workspace_id: &str,
    at: SystemTime,
) -> BTreeMap<String, ComponentType> {
    BUILT_IN_COMPONENT_TYPES
        .iter()
        .map(|built_in| {
            (
                built_in.component_type_id.to_string(),
                ComponentType {
                    component_type_id: built_in.component_type_id.to_string(),
                    arn: component_type_arn(region, workspace_id, built_in.component_type_id),
                    workspace_id: workspace_id.to_string(),
                    component_type_name: None,
                    description: None,
                    is_singleton: false,
                    is_abstract: built_in.is_abstract,
                    extends_from: Vec::new(),
                    property_definitions: built_in
                        .property_definitions
                        .iter()
                        .map(|property| (property.name.to_string(), property.definition()))
                        .collect(),
                    functions: BTreeMap::new(),
                    property_groups: BTreeMap::new(),
                    composite_component_types: BTreeMap::new(),
                    tags: BTreeMap::new(),
                    creation_date_time: at,
                    update_date_time: at,
                    state: STATE_ACTIVE.to_string(),
                    settles_at: at,
                },
            )
        })
        .collect()
}

/// Why a `componentTypeId` a caller asked to create cannot be minted, as the
/// message a `ValidationException` carries: it lies in the namespace the
/// service reserves for the component types it ships.
///
/// Built from the request — it names the id and the namespace — so a caller is
/// told what to change. See [`BUILT_IN_COMPONENT_TYPE_ID_PREFIX`].
pub fn reserved_component_type_namespace_message(component_type_id: &str) -> String {
    format!(
        "componentTypeId {component_type_id} lies in the \
         {BUILT_IN_COMPONENT_TYPE_ID_PREFIX} namespace, which is reserved for the service's \
         built-in component types"
    )
}

/// Why a write addressed to a stored built-in component type cannot be
/// performed, as the message a `ValidationException` carries: the component
/// type is the service's own, provisioned with the workspace, so no caller's
/// update or delete applies to it. Built from the request, naming what was
/// addressed and where.
pub fn built_in_component_type_not_writable_message(
    workspace_id: &str,
    component_type_id: &str,
) -> String {
    format!(
        "ComponentType {component_type_id} in workspace {workspace_id} is a built-in component \
         type of the service and cannot be changed or deleted."
    )
}

/// Why `extends_from` cannot be stored as the inheritance of
/// `component_type_id`, as the message a `ValidationException` carries — `None`
/// when it can.
///
/// A component type's declared inheritance has to be DESCRIBABLE: a read
/// reports each property definition's required `dataType` and `isInherited`,
/// which is only answerable when the service holds every component type the
/// chain names, and an id that reaches back to the component type itself is an
/// inheritance cycle no chain can be walked up. Shared by every writer of a
/// component type's `extendsFrom`, so one declaration meets one rule wherever
/// it is written.
pub fn extends_from_violation(
    workspace: &Workspace,
    component_type_id: &str,
    extends_from: &[String],
) -> Option<String> {
    for parent in extends_from {
        // The self-reference check comes FIRST, and so gets its own message
        // whether or not the id is stored: a writer that mints the component
        // type (a create) has not stored it yet, so resolving first would tell
        // such a caller its parent "does not exist" rather than that a
        // component type cannot extend itself.
        if parent == component_type_id {
            return Some(format!(
                "extendsFrom names component type {component_type_id} itself; a component type \
                 cannot extend itself"
            ));
        }
        if !workspace.resolves_component_type(parent) {
            return Some(format!(
                "extendsFrom names component type {parent}, which does not exist in workspace {}",
                workspace.workspace_id
            ));
        }
    }
    let (ancestors, _) = workspace.ancestor_component_type_ids(extends_from);
    if ancestors.contains(component_type_id) {
        return Some(format!(
            "extendsFrom names a component type that already extends \
             {component_type_id}; inheritance cannot form a cycle"
        ));
    }
    None
}

/// Why these `compositeComponentTypes` cannot be stored on the component type
/// `component_type_id`, as the message a `ValidationException` carries — `None`
/// when they can.
///
/// A composite component type IS the component type its `componentTypeId`
/// refers to, so the service has to hold that one for the composition to mean
/// anything; an entry that names none declares no reference to resolve.
/// Shared by every writer of a component type's `compositeComponentTypes`.
///
/// An entry naming `component_type_id` itself RESOLVES: the write that carries
/// it is the write that leaves the workspace holding that component type, so
/// the reference is not a dangling one — and this way a create and an update
/// carrying the same entry agree, instead of the create refusing what an
/// update of the stored record would accept.
pub fn composite_component_types_violation(
    workspace: &Workspace,
    component_type_id: &str,
    composite_component_types: &BTreeMap<String, crate::ops::types::CompositeComponentTypeRequest>,
) -> Option<String> {
    for (composite_name, composite) in composite_component_types {
        let Some(referenced) = composite.component_type_id.as_deref() else {
            continue;
        };
        if referenced == component_type_id || workspace.resolves_component_type(referenced) {
            continue;
        }
        return Some(format!(
            "compositeComponentTypes.{composite_name}.componentTypeId names component type \
             {referenced}, which does not exist in workspace {}",
            workspace.workspace_id
        ));
    }
    None
}

/// Why `record`'s `propertyGroups` cannot be stored, as the message a
/// `ValidationException` carries — `None` when they can.
///
/// A property group GROUPS properties of the component type, so each name it
/// carries has to be one the component type defines: one of its own
/// definitions, or one it inherits from the component types it extends. The
/// groups and the names are both read off `record` as the write leaves it, so a
/// group may name a property the same request defines and no caller can check
/// one component type's groups against another's definitions. Where the chain
/// reaches a built-in type this emulator does not model, it does not hold its
/// definitions, so a
/// name it does not otherwise account for cannot be called undefined and the
/// group stands. Shared by every writer of a component type's `propertyGroups`.
pub fn property_groups_violation(workspace: &Workspace, record: &ComponentType) -> Option<String> {
    let (inherited, inherits_unknown_definitions) =
        workspace.inherited_property_names(&record.extends_from);
    if inherits_unknown_definitions {
        return None;
    }
    for (group_name, group) in &record.property_groups {
        for property_name in &group.property_names {
            if record.property_definitions.contains_key(property_name)
                || inherited.contains(property_name)
            {
                continue;
            }
            return Some(format!(
                "propertyGroups.{group_name}.propertyNames names property {property_name}, which \
                 component type {} does not define",
                record.component_type_id
            ));
        }
    }
    None
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

/// The value a read reports for ONE property: the value written to the
/// property, else the DEFAULT its definition declares, and nothing at all when
/// neither exists.
///
/// `defaultValue` is "an object that contains the default value" of the
/// property, so a property nothing has written to still reads as the value its
/// definition gave it, and a write to the property replaces it. Shared by every
/// reader of a property's current value, so one property reads the same way
/// wherever it is reported; `definition` is the one
/// [`Workspace::effective_property_definition`] resolved, which is what makes a
/// default declared only on the component TYPE apply to a component that
/// restates nothing.
pub fn latest_property_value(
    property: Option<&PropertyRecord>,
    definition: Option<&crate::ops::types::PropertyDefinitionRequest>,
) -> Option<crate::ops::types::DataValue> {
    property
        .and_then(|property| property.value.clone())
        .or_else(|| definition.and_then(|definition| definition.default_value.clone()))
}

/// Whether `definition` declares a property whose values are a TIME SERIES —
/// `isTimeSeries`, "a Boolean value that specifies whether the property
/// consists of time series data", resolved so an absent member reads as
/// `false`, and so a property with no definition at all is not one.
///
/// Which properties the time-series data plane serves is decided here once:
/// `BatchPutPropertyValues` "sets values for multiple time series properties"
/// and `GetPropertyValueHistory` reads "the history of a time series property
/// value", so both resolve the property's EFFECTIVE definition
/// ([`Workspace::effective_property_definition`]) and ask this.
pub fn property_is_time_series(
    definition: Option<&crate::ops::types::PropertyDefinitionRequest>,
) -> bool {
    definition.is_some_and(|definition| definition.is_time_series.unwrap_or(false))
}

/// Which `Type` the value in a `DataValue` belongs to — the member it travels
/// in, spelled as the `Type` enum spells it
/// (`crate::ops::types::TYPE_VALUES`) — or `None` when it carries no typed
/// value at all.
///
/// A `DataValue` carries ONE value, so the members are tried in a fixed order
/// and the first one present names the type. `expression` is none of them: it
/// carries a parameter reference to be resolved rather than a value of a
/// declared type.
pub fn data_value_type(value: &crate::ops::types::DataValue) -> Option<&'static str> {
    if value.string_value.is_some() {
        return Some("STRING");
    }
    if value.double_value.is_some() {
        return Some("DOUBLE");
    }
    if value.integer_value.is_some() {
        return Some("INTEGER");
    }
    if value.long_value.is_some() {
        return Some("LONG");
    }
    if value.boolean_value.is_some() {
        return Some("BOOLEAN");
    }
    if value.list_value.is_some() {
        return Some("LIST");
    }
    if value.map_value.is_some() {
        return Some("MAP");
    }
    if value.relationship_value.is_some() {
        return Some("RELATIONSHIP");
    }
    None
}

/// Why `value` cannot be stored as a value of the property `definition`
/// defines, as the message that earns — `None` when it can.
///
/// A property's `dataType` is "an object that specifies the data type of a
/// property", so a value written to the property carries a value of THAT type:
/// one carrying another type, or no value at all, is refused rather than
/// stored, which would leave a read reporting a value of a type the definition
/// does not declare. A definition declaring no `dataType` constrains nothing,
/// so only the presence of a value is required there.
pub fn data_value_type_violation(
    property_name: &str,
    definition: Option<&crate::ops::types::PropertyDefinitionRequest>,
    value: &crate::ops::types::DataValue,
) -> Option<String> {
    let carried = data_value_type(value);
    let Some(carried) = carried else {
        return Some(format!(
            "the value written to property {property_name} carries no value of any data type"
        ));
    };
    let declared = definition
        .and_then(|definition| definition.data_type.as_ref())
        .map(|data_type| data_type.r#type.as_str())?;
    (carried != declared).then(|| {
        format!(
            "the value written to property {property_name} is a {carried} where that \
             property's dataType is {declared}"
        )
    })
}

/// The comparison operators a property FILTER may carry.
///
/// `operator` is modelled as a plain string with no value list, so this set is
/// not read off the model: these are the comparisons this service's own query
/// language spells in a `WHERE` clause (`crate::ops::execute_query`), asked of
/// the same stored property values. An operator outside the set is refused by
/// name rather than ignored, which would answer a filtered question with
/// unfiltered rows. Shared by every operation that filters on a property's
/// value, so one filter means one thing wherever it is read.
pub const COMPARISON_OPERATORS: &[&str] = &["=", "!=", "<", "<=", ">", ">="];

/// The operator a filter that names none compares with: a filter carries a
/// value to compare against, and equality is the comparison that value alone
/// describes.
pub const COMPARISON_OPERATOR_EQUALS: &str = "=";

/// Whether a comparison that came out `ordering` satisfies `operator`.
pub fn comparison_satisfies(operator: &str, ordering: ::std::cmp::Ordering) -> bool {
    use ::std::cmp::Ordering;
    match operator {
        "=" => ordering == Ordering::Equal,
        "!=" => ordering != Ordering::Equal,
        "<" => ordering == Ordering::Less,
        "<=" => ordering != Ordering::Greater,
        ">" => ordering == Ordering::Greater,
        ">=" => ordering != Ordering::Less,
        // Unreachable: a filter's operator is checked against
        // [`COMPARISON_OPERATORS`] before it is applied, and every one of them
        // is matched above.
        _ => false,
    }
}

/// Compare two modelled `DataValue`s for a FILTER, or `None` when they carry
/// nothing comparable: two values of different kinds, or a kind with no order
/// (a list, a map, a relationship, an expression).
///
/// `INTEGER`, `LONG` and `DOUBLE` are widths of ONE domain, so a filter
/// comparing a property against a threshold compares numbers whichever member
/// each of them travels in; two integral values compare exactly, so two `LONG`s
/// too far apart for a double to tell apart still compare as the distinct
/// values they are.
pub fn compare_data_values(
    left: &crate::ops::types::DataValue,
    right: &crate::ops::types::DataValue,
) -> Option<::std::cmp::Ordering> {
    if let (Some(left), Some(right)) = (DataNumber::of(left), DataNumber::of(right)) {
        return left.compare(&right);
    }
    if let (Some(left), Some(right)) = (
        left.string_value.as_deref(),
        right.string_value.as_deref(),
    ) {
        return Some(left.cmp(right));
    }
    if let (Some(left), Some(right)) = (left.boolean_value, right.boolean_value) {
        return Some(left.cmp(&right));
    }
    None
}

/// The number a `DataValue` carries placed on the DOUBLE, the one width every
/// number can be placed on — what an ORDERING compares, where a total relation
/// matters more than the last bits of a very large integer — or `None` when it
/// carries no number.
pub fn data_value_as_double(value: &crate::ops::types::DataValue) -> Option<f64> {
    DataNumber::of(value).map(|number| number.as_double())
}

/// A number a `DataValue` carries, at the width it travels in. The integral
/// widths are kept as an integer rather than folded into a double, so two
/// values too far apart for a double to tell apart still compare as the
/// distinct values they are.
enum DataNumber {
    Integer(i128),
    Double(f64),
}

impl DataNumber {
    /// The number this value carries, or `None` when it carries none.
    fn of(value: &crate::ops::types::DataValue) -> Option<Self> {
        if let Some(double) = value.double_value {
            return Some(Self::Double(double));
        }
        if let Some(integer) = value.integer_value {
            return Some(Self::Integer(i128::from(integer)));
        }
        value.long_value.map(|long| Self::Integer(i128::from(long)))
    }

    /// See [`data_value_as_double`].
    fn as_double(&self) -> f64 {
        match self {
            Self::Integer(integer) => *integer as f64,
            Self::Double(double) => *double,
        }
    }

    /// Compare two numbers for a FILTER, which judges one pair at a time: two
    /// integral values compare exactly, and a comparison that involves a double
    /// compares on the double — `None` when a `NaN` makes the pair
    /// incomparable.
    fn compare(&self, other: &Self) -> Option<::std::cmp::Ordering> {
        match (self, other) {
            (Self::Integer(left), Self::Integer(right)) => Some(left.cmp(right)),
            _ => self.as_double().partial_cmp(&other.as_double()),
        }
    }
}

/// Nanoseconds in one second: the unit a stored time-series instant is kept in,
/// so a value written with a fractional second keeps it.
pub const NANOS_PER_SECOND: i128 = 1_000_000_000;

/// The instant an ISO 8601 `Time` names, in nanoseconds since the Unix epoch,
/// or `None` when the text names none.
///
/// `Time` is "the ISO8601 DateTime of a value for a time series property",
/// which is a STRING in the model, so nothing before a handler parses it. The
/// zone it declares is read by the one reader of an RFC 3339 offset
/// (`service_common::rfc3339::split_utc_offset`) and text carrying no zone at
/// all is read as UTC, the zone the format's own designator stands for; the
/// calendar conversion itself is the library's.
pub fn instant_of_time(text: &str) -> Option<i128> {
    let (civil, offset_seconds) = match ::service_common::rfc3339::split_utc_offset(text) {
        Some(split) => split,
        None => (text, 0),
    };
    let parsed = ::aws_smithy_types::DateTime::from_str(
        &format!("{civil}Z"),
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    Some(instant_of_timestamp(&parsed) - i128::from(offset_seconds) * NANOS_PER_SECOND)
}

/// The instant a modelled `Timestamp` names, in nanoseconds since the Unix
/// epoch — the same unit [`instant_of_time`] answers in, so a value written
/// with the deprecated `timestamp` member and one written with `time` land on
/// one timeline.
pub fn instant_of_timestamp(at: &::aws_smithy_types::DateTime) -> i128 {
    i128::from(at.secs()) * NANOS_PER_SECOND + i128::from(at.subsec_nanos())
}

/// A stored instant as the ISO 8601 `Time` the wire carries, or `None` for an
/// instant outside the range the library's calendar covers.
pub fn time_of_instant(at: i128) -> Option<String> {
    let secs = i64::try_from(at.div_euclid(NANOS_PER_SECOND)).ok()?;
    let nanos = u32::try_from(at.rem_euclid(NANOS_PER_SECOND)).ok()?;
    ::aws_smithy_types::DateTime::from_secs_and_nanos(secs, nanos)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()
}

/// The most values this emulator keeps for ONE time series property.
///
/// A time series has no end: `BatchPutPropertyValues` may be called forever, so
/// the series a property holds is a bounded window of the most recent values
/// rather than everything ever written. The real service keeps values for a
/// retention period this emulator has no clock to age them out with, so the
/// bound is on the COUNT and is this emulator's own declared ceiling — the
/// same reading as [`MAX_WORKSPACES`] and the ceilings beside it.
pub const MAX_TIME_SERIES_VALUES_PER_PROPERTY: usize = 10_000;

/// One value of a time series property: what was written, and the instant text
/// it was written with.
///
/// The instant itself is the key the value is held under
/// ([`PropertyRecord::time_series`]), so two values written for the same
/// instant are one value — the later write replaces the earlier one — and the
/// series is in time order whatever order it was written in. The TEXT is kept
/// beside the value so a read reports the instant as the writer spelled it
/// rather than as this service would re-render it.
#[derive(Debug, Clone)]
pub struct TimeSeriesValue {
    pub value: crate::ops::types::DataValue,
    /// The ISO 8601 `Time` the write carried, or the rendering of the instant
    /// for a write that carried only the deprecated `timestamp`.
    pub time: String,
}

/// One component of an entity.
///
/// A component's properties and property groups are stored as the modelled
/// request shapes the caller supplied, so `GetEntity` reports back exactly the
/// definition and value that were persisted rather than a lossy projection of
/// them. Inheritance from a component *type* is not resolved: every definition
/// stored here is the entity-local one, which is why every `isInherited` this
/// service reports is `false`.
#[derive(Debug, Clone, Default)]
pub struct ComponentRecord {
    pub component_name: String,
    pub component_type_id: Option<String>,
    pub description: Option<String>,
    /// The component's properties, keyed by property name.
    pub properties: BTreeMap<String, PropertyRecord>,
    /// The component's property groups, keyed by group name.
    pub property_groups: BTreeMap<String, PropertyGroupRecord>,
    /// The composite components created INSIDE this component, keyed by the
    /// part of their `ComponentPath` below this component (`inner`, or
    /// `inner/deeper` for one nested a level further down) — the path with its
    /// first segment, which is this component's own name, removed. Keyed that
    /// way because the path below the component is what identifies a composite
    /// component within it, whatever depth it sits at. See
    /// [`resolve_composite_component_path`].
    pub composite_components: BTreeMap<String, CompositeComponentRecord>,
}

/// One composite component of an entity's component: an instance of the
/// component type that the holding component's own component type declares
/// under this name in its `compositeComponentTypes`.
///
/// Its properties and property groups are stored as the modelled request shapes
/// the caller supplied, the same choice [`ComponentRecord`] makes.
#[derive(Debug, Clone, Default)]
pub struct CompositeComponentRecord {
    /// The whole `ComponentPath` the create addressed this composite component
    /// by, starting at the entity's top-level component. Reported as
    /// `ComponentSummary::componentPath`.
    pub component_path: String,
    /// This composite component's own name: the LAST segment of its path, which
    /// is what `GetEntity` keys `ComponentResponse::compositeComponents` by
    /// (`CompositeComponentResponse`'s key is a `Name`, whose pattern admits no
    /// `/`).
    pub component_name: String,
    /// The component type this composite component is an instance of — the one
    /// the declaration its path resolved to names.
    pub component_type_id: String,
    pub description: Option<String>,
    /// The composite component's properties, keyed by property name.
    pub properties: BTreeMap<String, PropertyRecord>,
    /// The composite component's property groups, keyed by group name.
    pub property_groups: BTreeMap<String, PropertyGroupRecord>,
}

/// The separator between the segments of a `ComponentPath`.
///
/// A composite component is addressed by the path of names that leads to it:
/// the entity's top-level component, then one name per
/// `compositeComponentTypes` declaration walked through. `ComponentPath`'s
/// pattern (`^[a-zA-Z_\-0-9/]+$`) is a `Name`'s (`^[a-zA-Z_\-0-9]+$`) plus
/// `/`, so `/` is what joins those names.
pub const COMPONENT_PATH_SEPARATOR: &str = "/";

/// A `ComponentPath` resolved against an entity's components and its
/// workspace's component types. See [`resolve_composite_component_path`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedComponentPath {
    /// The entity's top-level component the path starts at: its first segment.
    pub component_name: String,
    /// The path BELOW that component — the remaining segments, still joined by
    /// [`COMPONENT_PATH_SEPARATOR`]. This is the key the composite component is
    /// held under in [`ComponentRecord::composite_components`].
    pub composite_path: String,
    /// The composite component's own name: the path's last segment.
    pub composite_name: String,
    /// The component type the declaration the path ends at names: the type the
    /// composite component is an instance of.
    pub component_type_id: String,
}

/// Resolve a `ComponentPath` that addresses a COMPOSITE component of an entity.
///
/// A composite component path is valid when its first segment names one of the
/// entity's components and each following segment names an entry in the
/// `compositeComponentTypes` of the component type reached by the segment
/// before it — which is why resolution needs both the entity's components and
/// the workspace's component types. A path that resolves yields the component it
/// hangs under and the component type it instantiates; one that does not yields
/// the reason, built from the request so the refusal names the member and the
/// segment that failed to resolve.
///
/// Shared by every writer of an entity's composite components, so a create and
/// an update hold one path to one rule. `member` is the request member the path
/// arrived in (`compositeComponents` on a create,
/// `compositeComponentUpdates` on an update), so a refusal names the member the
/// caller has to fix rather than another operation's.
///
/// Two limits are deliberate, both inherited from how this service stores
/// component types. A path of a single segment names a top-level component and
/// no composite component, so it does not resolve; and resolution walks only
/// the component type's OWN `compositeComponentTypes`, because this service
/// resolves no inheritance (every `isInherited` it reports is `false`) and does
/// not model every built-in component type a real workspace carries — so a
/// path through a component type this workspace does not hold cannot be
/// resolved, and is refused rather than guessed at.
pub fn resolve_composite_component_path(
    components: &BTreeMap<String, ComponentRecord>,
    component_types: &BTreeMap<String, ComponentType>,
    member: &str,
    component_path: &str,
) -> Result<ResolvedComponentPath, String> {
    let segments: Vec<&str> = component_path.split(COMPONENT_PATH_SEPARATOR).collect();
    if segments.len() < 2 || segments.iter().any(|segment| segment.is_empty()) {
        return Err(format!(
            "{member}.{component_path} is not the path of a composite component: a \
             component path names one of the entity's components and at least one \
             compositeComponentType declared inside it, separated by \
             '{COMPONENT_PATH_SEPARATOR}'"
        ));
    }
    let component_name = segments[0];
    let Some(component) = components.get(component_name) else {
        return Err(format!(
            "{member}.{component_path} names component {component_name}, which the \
             entity does not have"
        ));
    };
    let Some(mut component_type_id) = component.component_type_id.clone() else {
        return Err(format!(
            "{member}.{component_path} names component {component_name}, which \
             instantiates no componentType and so declares no composite component"
        ));
    };
    for segment in &segments[1..] {
        let Some(parent) = component_types.get(&component_type_id) else {
            return Err(format!(
                "{member}.{component_path}: componentType {component_type_id} does \
                 not exist in this workspace, so {segment} cannot be resolved against it"
            ));
        };
        let Some(declaration) = parent.composite_component_types.get(*segment) else {
            return Err(format!(
                "{member}.{component_path}: componentType {component_type_id} \
                 declares no compositeComponentTypes.{segment}"
            ));
        };
        let Some(declared) = declaration.component_type_id.clone() else {
            return Err(format!(
                "{member}.{component_path}: componentType {component_type_id} \
                 declares compositeComponentTypes.{segment} with no componentTypeId, so it names \
                 no component type to instantiate"
            ));
        };
        component_type_id = declared;
    }
    Ok(ResolvedComponentPath {
        component_name: component_name.to_string(),
        composite_path: segments[1..].join(COMPONENT_PATH_SEPARATOR),
        composite_name: segments[segments.len() - 1].to_string(),
        component_type_id,
    })
}

/// One property of a component: the definition and value the request gave it.
///
/// Whether the property is the component's external id is NOT answerable from
/// this record alone: `isExternalId` lives on the property DEFINITION, which a
/// component normally inherits from the component type it instantiates rather
/// than restating, so it is resolved against the workspace
/// ([`Workspace::component_external_ids`]).
#[derive(Debug, Clone, Default)]
pub struct PropertyRecord {
    pub definition: Option<crate::ops::types::PropertyDefinitionRequest>,
    pub value: Option<crate::ops::types::DataValue>,
    /// The values written to this property through the time-series data plane
    /// (`BatchPutPropertyValues`), keyed by the instant each was written FOR in
    /// nanoseconds since the Unix epoch ([`instant_of_time`]).
    ///
    /// A map keyed by the instant, so the series is in time order however it
    /// was written, a value written twice for one instant is stored once, and a
    /// read of a window takes a range of it. It is EMPTY for every property
    /// nothing has written a time-series value to, which is every property of a
    /// definition that is not `isTimeSeries`
    /// ([`property_is_time_series`]) — the data plane writes to no other.
    pub time_series: BTreeMap<i128, TimeSeriesValue>,
}

impl PropertyRecord {
    /// Record one time-series value for this property, at the instant `at`.
    ///
    /// A value written for an instant the series already holds REPLACES it: the
    /// instant is the value's identity, so writing the same instant twice
    /// leaves one value rather than two readings of one moment. Past this
    /// emulator's declared ceiling
    /// ([`MAX_TIME_SERIES_VALUES_PER_PROPERTY`]) the OLDEST values are
    /// dropped, so a caller that keeps writing grows the store no further and
    /// keeps the most recent window.
    pub fn record_time_series_value(&mut self, at: i128, value: TimeSeriesValue) {
        self.time_series.insert(at, value);
        while self.time_series.len() > MAX_TIME_SERIES_VALUES_PER_PROPERTY {
            self.time_series.pop_first();
        }
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

impl RegionalState {
    /// The account's billable entity count in this Region: how many entities
    /// this service currently holds there, across every workspace.
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

    /// Set the account's pricing plan to `pricing_mode` with `bundle_names` at
    /// `now`, and hand back the plan that is now in force.
    ///
    /// The change takes effect AT ONCE: the new plan's `effectiveDateTime` is
    /// the instant of the call that set it, the same instant as its
    /// `updateDateTime`. The service publishes no rule for deferring an accepted
    /// change to some later instant, so this emulator defers none — which also
    /// means a change that was accepted but had not taken effect
    /// ([`RegionalState::pending_pricing_plan`])
    /// is SUPERSEDED here rather than left to land later, and after this call
    /// nothing is pending.
    ///
    /// The `UpdateReason` recorded on the new plan says what this change was,
    /// read off the plan it replaces — never off the request alone:
    ///
    /// * [`UPDATE_REASON_OVERWRITTEN`] when it superseded a pending change,
    ///   which is the most specific thing that can be said about it;
    /// * else [`UPDATE_REASON_PRICING_MODE_UPDATE`] when the mode differs from
    ///   the mode in force;
    /// * else [`UPDATE_REASON_PRICING_TIER_UPDATE`], the mode being unchanged,
    ///   so the bundles — and the tier they place the account in — are all the
    ///   change can have moved.
    ///
    /// The two remaining `UpdateReason` values are deliberately NOT produced
    /// here, because neither describes a change a caller asked for:
    /// [`UPDATE_REASON_DEFAULT`] marks the plan of an account nobody has
    /// updated, and `ENTITY_COUNT_UPDATE` a tier the service moved on its own
    /// when the account's entity count crossed a threshold.
    ///
    /// `pricingTier` is left unset. The tier a bundled plan sits in is a
    /// function of AWS's published tiered-bundle thresholds, which this
    /// service's model does not carry; the member is optional precisely so a
    /// plan whose tier is not determined is representable, so the plan is
    /// reported without one rather than with a threshold table this emulator
    /// would have invented.
    pub fn set_pricing_plan(
        &mut self,
        pricing_mode: String,
        bundle_names: Vec<String>,
        now: SystemTime,
    ) -> &PricingPlanRecord {
        let superseded_pending = self.pending_pricing_plan.take().is_some();
        let update_reason = if superseded_pending {
            UPDATE_REASON_OVERWRITTEN
        } else if pricing_mode != self.pricing_plan.pricing_mode {
            UPDATE_REASON_PRICING_MODE_UPDATE
        } else {
            UPDATE_REASON_PRICING_TIER_UPDATE
        };
        self.pricing_plan = PricingPlanRecord {
            pricing_mode,
            update_reason: update_reason.to_string(),
            bundle_names,
            pricing_tier: None,
            effective_date_time: now,
            update_date_time: now,
        };
        &self.pricing_plan
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
            ResourceRef::Scene {
                workspace_id,
                scene_id,
            } => Some(&self.workspaces.get(workspace_id)?.scenes.get(scene_id)?.tags),
            ResourceRef::SyncJob {
                workspace_id,
                sync_source,
            } => Some(
                &self
                    .workspaces
                    .get(workspace_id)?
                    .sync_jobs
                    .get(sync_source)?
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
            ResourceRef::Scene {
                workspace_id,
                scene_id,
            } => Some(
                &mut self
                    .workspaces
                    .get_mut(&workspace_id)?
                    .scenes
                    .get_mut(&scene_id)?
                    .tags,
            ),
            ResourceRef::SyncJob {
                workspace_id,
                sync_source,
            } => Some(
                &mut self
                    .workspaces
                    .get_mut(&workspace_id)?
                    .sync_jobs
                    .get_mut(&sync_source)?
                    .tags,
            ),
        }
    }
}

/// The message for a resource no ARN in this service's index names.
///
/// Built from the request, naming the ARN that was looked for — the same
/// construction as [`entity_not_found_message`], one level up from an id to
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

/// The message for a scene the workspace the request named does not hold.
/// Built from the request, naming what was looked for and where — the same
/// construction as [`entity_not_found_message`] one resource across.
pub fn scene_not_found_message(workspace_id: &str, scene_id: &str) -> String {
    format!("Scene {scene_id} in workspace {workspace_id} not found.")
}

/// The message for a cancellation of a job that has already finished. Built
/// from the request and the store: it names the job and the terminal state it
/// finished in, which is what makes the conflict actionable for a caller that
/// polled a moment too late.
pub fn metadata_transfer_job_terminal_message(
    metadata_transfer_job_id: &str,
    state: &str,
) -> String {
    format!(
        "MetadataTransferJob {metadata_transfer_job_id} is {state} and can no longer be cancelled."
    )
}

/// The message for a sync job the workspace the request named does not hold.
/// Built from the request, naming the SOURCE that was looked for and the
/// workspace it was looked for in — a sync job has no id of its own, the source
/// being its identity inside the workspace. Same construction as
/// [`scene_not_found_message`] one resource across.
pub fn sync_job_not_found_message(workspace_id: &str, sync_source: &str) -> String {
    format!("SyncJob for sync source {sync_source} in workspace {workspace_id} not found.")
}

/// The message for a workspace this service does not hold, on a path that LOOKS
/// a workspace UP to serve the request: built from the request, naming the
/// workspace that was looked for.
///
/// The service's not-found message is per-operation, not one string for the
/// whole API: the look-up path reports what it could not find for the id it was
/// given, while the workspace's own UPDATE path reports the account-scoped form
/// [`workspace_does_not_exist_message`] builds. Every operation that resolves a
/// workspace in order to read or write something it contains uses this one.
pub fn workspace_not_found_message(workspace_id: &str) -> String {
    format!("Unable to find workspace for id: {workspace_id}")
}

/// The message the workspace UPDATE path reports for a workspace this service
/// does not hold: built from the request and the caller's account, which is what
/// scopes a workspace id.
///
/// The other construction of the same miss is [`workspace_not_found_message`];
/// see it for why the two differ.
pub fn workspace_does_not_exist_message(workspace_id: &str) -> String {
    format!(
        "Workspace with id {}|{workspace_id} does not exist.",
        ::emulator_server::emulator_account_id()
    )
}

/// The message for a delete of an entity that still has children, which only
/// `isRecursive` removes: built from the request, naming the entity the delete
/// addressed and the workspace it addressed it in.
pub fn entity_has_children_message(workspace_id: &str, entity_id: &str) -> String {
    format!("Cannot delete entity {entity_id} in {workspace_id} as it has child entities.")
}

/// The message for a create whose id is already TAKEN, as the message a
/// `ConflictException` carries.
///
/// Built entirely from the request, in the service's own construction: the kind
/// of resource and the id the create asked for first, then the workspace that
/// holds it where the resource lives inside one, then what is wrong with it —
/// the same order the not-found family above reads in
/// ([`entity_not_found_message`]), with "already exists" in place of "not
/// found". `kind` is the resource's name as the service writes it in this
/// sentence; `workspace_id` is `None` for a resource that nests under no
/// workspace. Shared by every create of this service, so one conflict reads one
/// way wherever it is refused.
pub fn already_exists_message(kind: &str, resource_id: &str, workspace_id: Option<&str>) -> String {
    match workspace_id {
        Some(workspace_id) => {
            format!("{kind} {resource_id} in workspace {workspace_id} already exists.")
        }
        None => format!("{kind} {resource_id} already exists."),
    }
}

/// The Region this request is served in: the region of its SigV4 credential
/// scope, or [`DEFAULT_REGION`] when it is unsigned.
///
/// The ONE answer to "which Region is this?", used both to pick the store that
/// answers the request
/// ([`region`](ComAmazonawsIottwinmakerAwsiottwinmakerState::region)) and to
/// build the ARNs it mints, so a resource can never be filed in one Region and
/// stamped with another.
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
    ::engine::emulator_arn(
        "iottwinmaker",
        region,
        &format!("{WORKSPACE_ARN_RESOURCE_PREFIX}{workspace_id}"),
    )
}

/// The resource prefix of a workspace's ARN, shared by the ARN [`workspace_arn`]
/// mints and the id [`workspace_id_in_arn`] reads back out of it, so one
/// definition says what the shape is.
const WORKSPACE_ARN_RESOURCE_PREFIX: &str = "workspace/";

/// The `workspaceId` inside a workspace ARN, or `None` when the value is not
/// one: the inverse of [`workspace_arn`], for a caller holding a stored ARN and
/// comparing it against an id.
///
/// Unlike
/// [`resolve_workspace_id`](RegionalState::resolve_workspace_id),
/// this reads the id out of the ARN rather than resolving it against the stored
/// workspaces, so it still names the workspace of an ARN whose workspace has
/// been deleted. An ARN's resource portion is its sixth colon-separated field.
pub fn workspace_id_in_arn(arn: &str) -> Option<&str> {
    arn.splitn(6, ':')
        .nth(5)?
        .strip_prefix(WORKSPACE_ARN_RESOURCE_PREFIX)
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

/// The ARN of a scene, nested under its workspace's resource path the way
/// [`entity_arn`] nests an entity's.
pub fn scene_arn(region: &str, workspace_id: &str, scene_id: &str) -> String {
    ::engine::emulator_arn(
        "iottwinmaker",
        region,
        &format!("workspace/{workspace_id}/scene/{scene_id}"),
    )
}

/// The ARN of a sync job, nested under its workspace's resource path the way
/// [`scene_arn`] nests a scene's. The sync SOURCE takes the place of the id,
/// being the job's identity inside its workspace.
pub fn sync_job_arn(region: &str, workspace_id: &str, sync_source: &str) -> String {
    ::engine::emulator_arn(
        "iottwinmaker",
        region,
        &format!("workspace/{workspace_id}/sync-job/{sync_source}"),
    )
}

/// The ARN of a metadata transfer job. An ACCOUNT-level resource, so — unlike
/// [`scene_arn`] — its resource portion nests under no workspace.
pub fn metadata_transfer_job_arn(region: &str, metadata_transfer_job_id: &str) -> String {
    ::engine::emulator_arn(
        "iottwinmaker",
        region,
        &format!("metadata-transfer-job/{metadata_transfer_job_id}"),
    )
}

/// The ARN of the role a metadata transfer job runs under: the service-linked
/// role at [`METADATA_TRANSFER_JOB_ROLE_PATH`], in this emulator's account.
///
/// IAM is global, so the ARN carries no region — which is also what the
/// modelled `RoleArn` pattern (`^arn:…:iam::[0-9]{12}:role/`) requires.
pub fn metadata_transfer_job_role() -> String {
    ::engine::emulator_arn("iam", "", METADATA_TRANSFER_JOB_ROLE_PATH)
}

/// The message for a metadata transfer job this account does not hold. Built
/// from the request, naming the job that was looked for — the same construction
/// as [`resource_not_found_message`], one resource across.
pub fn metadata_transfer_job_not_found_message(metadata_transfer_job_id: &str) -> String {
    format!("MetadataTransferJob {metadata_transfer_job_id} not found.")
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
