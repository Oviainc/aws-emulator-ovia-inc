// SPDX-License-Identifier: CC0-1.0

//! The lineage-association slice of this service's state: the association records
//! `AddAssociation` writes, the lineage entity kinds an end can name, and the lookups
//! the readers and the delete share.

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
