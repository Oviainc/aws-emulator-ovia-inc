// SPDX-License-Identifier: CC0-1.0

//! The LINEAGE slice of this service's state: the actions, contexts, artifacts,
//! lineage groups and the associations between them that `AddAssociation`,
//! `DeleteAssociation` and the lineage readers resolve.
//!
//! Moved out of [`super`] unchanged, so the parent module stays inside the project's
//! file-length limit and the lineage entities read as one unit. Every name is
//! re-exported from [`super`], so a handler keeps naming `crate::state::…`.

use super::*;

/// The `<kind>` component of an `artifact/<id>` ARN, i.e. the kind `CreateArtifact`
/// mints and the one an `AssociationEntityArn` names an artifact by. Named here so the
/// create and the association slice mint and resolve the same kind.
pub const ARTIFACT_KIND: &str = "artifact";

/// The `<kind>` component of an `action/<name>` ARN. See [`ARTIFACT_KIND`].
pub const ACTION_KIND: &str = "action";

/// See [`MODEL_QUOTA`] for why the bound is the emulator's OWN rather than a claim about
/// any account's limit. It bounds the association store the same way, and is what
/// `AddAssociation` answers `ResourceLimitExceeded` past. It is higher than the
/// per-resource bounds because an association is an EDGE: the store grows with the PAIRS
/// of lineage entities a caller connects, not with the entities themselves, and every
/// entry it holds is released by `DeleteAssociation`.
pub const ASSOCIATION_QUOTA: usize = 10_000;

/// How SageMaker refuses the delete of a lineage entity that is still the SOURCE of an
/// association the account holds.
///
/// Entirely FIXED text: the recorded refusal interpolates nothing — no entity name, ARN or
/// edge count — so the sentence is stated once here and every lineage-entity delete words
/// the referential-integrity refusal the same way. The caller clears the condition itself
/// with `DeleteAssociation`, so this is the 400 `ValidationException` rather than an error
/// any of those deletes models.
pub const ENTITY_WITH_ASSOCIATIONS: &str = "Cannot delete entity with associations.";

/// One lineage association created by `AddAssociation`, as `DeleteAssociation` releases
/// it.
///
/// The `(source_arn, destination_arn)` pair is the association's identity — SageMaker
/// mints no id for an association and no operation addresses one by anything else — so
/// the pair is the store key and these two fields repeat it, the way every other record
/// here keeps the identifier it is filed under. `association_type` stays an `Option`
/// because `AssociationType` is an optional member: an association created without one
/// carries none, which is distinguishable on the wire from any of the modelled edge
/// types. `creation_time` is the instant the association was FIRST added, so a repeated
/// add does not pretend the edge is new, and `created_by` is stored beside it by the call
/// that first added the edge — as every other record here stores the identity that
/// created it — so a later read reports the identity that really created this one.
#[derive(Debug, Clone)]
pub struct Association {
    pub source_arn: String,
    pub destination_arn: String,
    pub association_type: Option<String>,
    pub created_by: UserContext,
    pub creation_time: SystemTime,
}

impl Association {
    /// This association's store key as ONE string, for the page token a listing mints.
    ///
    /// The identity is the `(source, destination)` pair, so the token has to name both; a
    /// space joins them because no ARN this service mints contains one (the resource
    /// segment is a lineage name or a minted id), so the pair is recoverable and two
    /// different pairs can never spell the same key. The same precedent as
    /// [`ModelPackage::key`], which spells a two-part identity as one key too.
    pub fn key(&self) -> String {
        format!("{} {}", self.source_arn, self.destination_arn)
    }

    /// Whether this edge belongs to `region`, i.e. both of its ends are ARNs this service
    /// mints there.
    ///
    /// The association store is account-wide, while every lineage entity is REGIONAL — its
    /// ARN names the Region it was created in — so a listing reports the edges of the
    /// Region the request signed for and never one between entities of another.
    pub fn in_region(&self, region: &str) -> bool {
        arn_resource(&self.source_arn, region).is_some()
            && arn_resource(&self.destination_arn, region).is_some()
    }
}

/// What a reader reports about ONE END of an association: the lineage entity's own name
/// and type, read out of the record the matching `Create*` stored.
///
/// `name` is an `Option` because an artifact need not have one (`ArtifactName` is
/// optional), and `entity_type` because only three of the five entity kinds an
/// `AssociationEntityArn` admits carry a type of their own — an artifact's `ArtifactType`,
/// an action's `ActionType`, a context's `ContextType`. An experiment and a trial
/// component take theirs from the `Source` of the job that emitted them, which
/// `CreateExperiment` and `CreateTrialComponent` do not accept, so one created directly
/// has no type to report and the member stays absent rather than being filled with an
/// invented value.
#[derive(Debug, Clone, Default)]
pub struct LineageEntityEnd {
    pub name: Option<String>,
    pub entity_type: Option<String>,
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another association fits, i.e. the store is below [`ASSOCIATION_QUOTA`].
    pub fn associations_below_quota(&self) -> bool {
        self.associations.len() < ASSOCIATION_QUOTA
    }

    /// Whether the lineage entity `arn` names is still the SOURCE of an association the
    /// account holds, i.e. an edge the graph records starts at it.
    ///
    /// The referential-integrity question every lineage-entity delete asks before releasing
    /// its record ([`ENTITY_WITH_ASSOCIATIONS`]): an entity deleted out from under an edge
    /// that starts at it would leave `ListAssociations` reporting an origin no reader can
    /// describe. The association store is keyed by the `(SourceArn, DestinationArn)` pair,
    /// so the source side is the key's first component and this asks whether any stored key
    /// carries `arn` there — the one place the question is asked, so the deletes of the five
    /// entity kinds an `AssociationEntityArn` admits cannot answer it differently.
    pub fn is_association_source(&self, arn: &str) -> bool {
        self.associations
            .keys()
            .any(|(source_arn, _)| source_arn == arn)
    }

    /// Whether `arn` names a lineage entity this service holds, i.e. one an association
    /// may point at.
    ///
    /// `AssociationEntityArn`'s modelled pattern admits five entity kinds — experiment,
    /// experiment-trial-component, artifact, action and context. Each is resolved against
    /// the store the matching create writes to, through
    /// [`entity_name_from_name_or_arn`], so an ARN naming another partition, service,
    /// Region or account names no entity here. All five kinds now have an implemented
    /// create, so each is decided by whether its own store holds the ARN rather than by
    /// the kind being unreachable.
    pub fn lineage_entity_exists(&self, region: &str, arn: &str) -> bool {
        self.lineage_entity_end(region, arn).is_some()
    }

    /// The name and type of the lineage entity `arn` names, or `None` when it names none
    /// this service holds.
    ///
    /// The resolution side of [`Self::lineage_entity_exists`], which is this returning
    /// `Some`: one walk over the five entity kinds, so "is this an end an association may
    /// have" and "what does a reader report about that end" can never disagree.
    pub fn lineage_entity_end(&self, region: &str, arn: &str) -> Option<LineageEntityEnd> {
        // Every `AssociationEntityArn` IS an ARN (its pattern starts `^arn:`), so the
        // bare-name branch of `entity_name_from_name_or_arn` is unreachable from an
        // association request; guarded rather than assumed, so a bare name can never
        // resolve to an entity through this helper.
        if !arn.starts_with("arn:") {
            return None;
        }
        if entity_name_from_name_or_arn(arn, region, ARTIFACT_KIND).is_some() {
            // Both lineage stores are keyed by the WHOLE ARN the create minted, which
            // already carries this request's Region and the emulator's account.
            return self.artifacts.get(arn).map(|artifact| LineageEntityEnd {
                name: artifact.name.clone(),
                entity_type: Some(artifact.artifact_type.clone()),
            });
        }
        if entity_name_from_name_or_arn(arn, region, ACTION_KIND).is_some() {
            return self.actions.get(arn).map(|action| LineageEntityEnd {
                name: Some(action.name.clone()),
                entity_type: Some(action.action_type.clone()),
            });
        }
        if entity_name_from_name_or_arn(arn, region, CONTEXT_KIND).is_some() {
            // The context store is keyed by the whole ARN too (see [`context_key`]), so a
            // context `CreateContext` really stored is an end an association may point at.
            return self.contexts.get(arn).map(|context| LineageEntityEnd {
                name: Some(context.name.clone()),
                entity_type: Some(context.context_type.clone()),
            });
        }
        if let Some(name) = entity_name_from_name_or_arn(arn, region, TRIAL_COMPONENT_KIND) {
            // The trial-component store is keyed by the case-folded NAME (the name "is
            // not case-sensitive" per its model doc), so the lookup goes through the same
            // key `CreateTrialComponent` stored the record under.
            return self
                .trial_components
                .get(&entity_key(name))
                .map(|component| LineageEntityEnd {
                    name: Some(component.name.clone()),
                    entity_type: None,
                });
        }
        if let Some(name) = entity_name_from_name_or_arn(arn, region, EXPERIMENT_KIND) {
            // Same keying as the trial-component store, and for the same reason: an
            // `ExperimentEntityName` is unique per account and not case-sensitive, so
            // `CreateExperiment` filed the record under [`entity_key`] of the name.
            return self
                .experiments
                .get(&entity_key(name))
                .map(|experiment| LineageEntityEnd {
                    name: Some(experiment.name.clone()),
                    entity_type: None,
                });
        }
        None
    }
}

/// A lineage group, as `DescribeLineageGroup` reports it.
///
/// SageMaker models no `CreateLineageGroup`: a lineage group comes into being with the
/// account's lineage entities (every artifact, action, context and trial component
/// belongs to one), so a record here is materialised through
/// [`ComAmazonawsSagemakerSagemakerState::ensure_lineage_group`] rather than by a create
/// handler — and a group nothing has materialised is simply absent from the store, which
/// is what the modelled `ResourceNotFound` reports.
#[derive(Debug, Clone)]
pub struct LineageGroup {
    pub name: String,
    pub arn: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub created_by: Option<UserContext>,
    pub last_modified_by: Option<UserContext>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    /// The resource-based policy document that gives another account access to this
    /// group, as `GetLineageGroupPolicy` reports it: an IAM policy document, and
    /// [`EMPTY_RESOURCE_POLICY`] while nothing has shared the group.
    ///
    /// Held on the group's own record rather than in a second map, so the policy a group
    /// carries lives beside the group a describe reports and cannot outlive it. The whole
    /// SageMaker surface models no operation that attaches one — a lineage group is shared
    /// with another account by a resource share, not by a call on this service — so a
    /// group this service materialises carries the empty document.
    pub resource_policy: String,
}

impl ComAmazonawsSagemakerSagemakerState {
    /// The lineage group `region` holds under `name`, or `None` when no lineage entity
    /// has materialised it there.
    ///
    /// A lineage group is regional, so the Region is half the key: a group materialised
    /// by an entity created in one Region is not reported to a caller signing for
    /// another, which is what keeps a replayed `LineageGroupArn` naming the Region the
    /// reader signed for.
    pub fn lineage_group(&self, region: &str, name: &str) -> Option<&LineageGroup> {
        self.lineage_groups
            .get(&(region.to_string(), name.to_string()))
    }

    /// Every lineage group `region` holds, in `LineageGroupName` order.
    ///
    /// The Region is half the key (see [`Self::lineage_group`]), so a listing reports only
    /// the groups the caller's Region holds: a group an entity materialised elsewhere is
    /// not this caller's, exactly as the describe does not report it. One accessor for it,
    /// so `ListLineageGroups` and `DescribeLineageGroup` cannot disagree about which
    /// groups a Region has.
    pub fn lineage_groups_in<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = &'a LineageGroup> + 'a {
        self.lineage_groups
            .iter()
            .filter(move |((group_region, _), _)| group_region.as_str() == region)
            .map(|(_, group)| group)
    }

    /// Materialise the lineage group `name` in `region` unless the store already holds
    /// it, and answer the ARN of the group this call CREATED.
    ///
    /// This is the ONLY way a lineage group enters the store, because SageMaker models no
    /// `CreateLineageGroup`: the service materialises a group when the account's lineage
    /// entities put something in it, so the lineage-entity creates call this with
    /// [`DEFAULT_LINEAGE_GROUP`] — `CreateArtifact`, `CreateAction`, `CreateContext` and
    /// `CreateTrialComponent` all do. Readers never call it: a reporting operation must
    /// not mint a resource.
    ///
    /// `DisplayName` is the group's own name: nothing in the API supplies one for a group
    /// the service materialises itself, and "if `DisplayName` isn't specified, the name is
    /// displayed" is the rule every other SageMaker entity's model doc states.
    /// `CreatedBy`/`LastModifiedBy` stay absent — there is no Studio user context behind a
    /// group the service created, and an invented one would be a value no request made.
    ///
    /// `None` means nothing changed: either that Region already held that group (so a
    /// second entity does not move its stored instants), or the store is at
    /// [`LINEAGE_GROUP_QUOTA`] and cannot take another, which bounds the growth a caller
    /// varying the Region or the name could otherwise cause. A caller that gets
    /// `Some(arn)` records the state change under that ARN, so a materialised group is
    /// auditable like any create. The at-the-bound case is WARNED about here rather than
    /// by each caller, so every lineage-entity create reports it the same way and none
    /// can drift.
    pub fn ensure_lineage_group(
        &mut self,
        region: &str,
        name: &str,
        now: SystemTime,
    ) -> Option<String> {
        // Read before the entry() borrow, so the bound is checked against the same store
        // this insert grows.
        let at_capacity = self.lineage_groups.len() >= LINEAGE_GROUP_QUOTA;
        match self
            .lineage_groups
            .entry((region.to_string(), name.to_string()))
        {
            ::std::collections::btree_map::Entry::Occupied(_) => None,
            ::std::collections::btree_map::Entry::Vacant(_) if at_capacity => {
                // Visible rather than silent: past the bound the entity this call creates
                // still reports its Region's lineage-group ARN, which
                // `DescribeLineageGroup` cannot answer until the store is back under it.
                tracing::warn!(
                    lineage_group = name,
                    region,
                    bound = LINEAGE_GROUP_QUOTA,
                    "the lineage-group store is at its bound, so this Region's lineage \
                     group was not materialised"
                );
                None
            }
            ::std::collections::btree_map::Entry::Vacant(slot) => {
                let arn = named_lineage_group_arn(region, name);
                slot.insert(LineageGroup {
                    name: name.to_string(),
                    arn: arn.clone(),
                    display_name: Some(name.to_string()),
                    description: None,
                    created_by: None,
                    last_modified_by: None,
                    creation_time: now,
                    last_modified_time: now,
                    // Nothing has shared a group the service has just materialised, so it
                    // carries the empty policy document.
                    resource_policy: EMPTY_RESOURCE_POLICY.to_string(),
                });
                Some(arn)
            }
        }
    }
}

/// See [`MODEL_QUOTA`]. Bounds the lineage actions `CreateAction` registers; every
/// record it holds is released by `DeleteAction`.
pub const ACTION_QUOTA: usize = 1_000;

/// A lineage action created by `CreateAction`, as `DescribeAction` replays it and
/// `DeleteAction` releases it.
///
/// Every optional member is held as an `Option` rather than flattened to a default, so
/// a reader can tell "the caller sent nothing" from "the caller sent an empty value";
/// `properties`, which is not optional but the map the action HOLDS (see this module's
/// note on an entity's own collections), is a `BTreeMap` so it serialises in one stable
/// order whatever order the request arrived in.
#[derive(Debug, Clone)]
pub struct Action {
    pub name: String,
    pub arn: String,
    /// The Region the ARN names, i.e. the one the creating request signed for. Held
    /// separately so the lineage group reported for this action does not have to be
    /// parsed back out of the ARN.
    pub region: String,
    pub source: crate::ops::types::ActionSource,
    pub action_type: String,
    pub description: Option<String>,
    pub status: Option<String>,
    pub properties: BTreeMap<String, String>,
    pub metadata_properties: Option<MetadataProperties>,
    /// What the create carried, so a later `ListTags`/`AddTags` reports it; no
    /// implemented operation surfaces it yet (`DescribeAction` models no `Tags` member).
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another lineage action fits, i.e. the store is below [`ACTION_QUOTA`].
    pub fn actions_below_quota(&self) -> bool {
        self.actions.len() < ACTION_QUOTA
    }

    /// The key [`ComAmazonawsSagemakerSagemakerState::actions`] holds an action under,
    /// for an identifier the caller gave as either a name or an ARN (the lineage
    /// readers' `ExperimentEntityNameOrArn`). See [`arn_from_name_or_arn`], which the
    /// context slice resolves its own key with too.
    pub fn action_key(region: &str, identifier: &str) -> String {
        arn_from_name_or_arn(region, ACTION_KIND, identifier)
    }
}

/// The ARN resource kind a lineage context carries, i.e. the `<kind>` in [`resource_arn`].
/// Fixed by `ContextArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:context/`), and named once here so
/// the create, the reader, the update and the delete mint and match the same ARN.
pub const CONTEXT_KIND: &str = "context";

/// See [`MODEL_QUOTA`]. Bounds the lineage-context store the same way, and is what
/// `CreateContext` answers `ResourceLimitExceeded` past; every record it holds is released
/// by `DeleteContext`.
pub const CONTEXT_QUOTA: usize = 1_000;

/// A lineage context created by `CreateContext`, as `DescribeContext` replays it and
/// `UpdateContext` edits it.
///
/// `properties` is the map the context HOLDS, empty while nothing has put an entry in it
/// (see this module's note on an entity's own collections). It is a `BTreeMap` so the map
/// serialises in one stable order whatever order the request arrived in. `created_by` /
/// `last_modified_by` are stored by the call that set them rather than rebuilt per read,
/// so every later read reports the identity that actually created or last edited the
/// context.
#[derive(Debug, Clone)]
pub struct Context {
    pub name: String,
    pub arn: String,
    /// The Region the ARN names, i.e. the one the creating request signed for. Held
    /// separately so a read does not have to parse the ARN to report the Region's lineage
    /// group.
    pub region: String,
    pub source: crate::ops::types::ContextSource,
    pub context_type: String,
    pub description: Option<String>,
    pub properties: BTreeMap<String, String>,
    pub created_by: UserContext,
    pub last_modified_by: UserContext,
    /// Stored so a later `ListTags` reports what the create carried; `DescribeContext`
    /// models no `Tags` member, so this is the one reader that surfaces it.
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
}
