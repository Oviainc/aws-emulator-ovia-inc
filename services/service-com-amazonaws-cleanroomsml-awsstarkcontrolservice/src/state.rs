// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cleanroomsml-awsstarkcontrolservice service.
//!
//! The training dataset and the audience model are the first link of Clean Rooms
//! ML's resource chain: `CreateTrainingDataset` mints a training dataset,
//! `CreateAudienceModel` trains a model from one, the Get operations report them,
//! the List operation pages over them and the Delete operations release them.
//! The records below are the one copy of each resource every one of those
//! handlers reads and writes, plus the summary projections the list operations
//! return.
//!
//! The ML input channel is the collaboration-scoped link of the same chain: a
//! channel is created under a membership, and every member of the collaboration
//! that membership belongs to can list the channels it holds.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    CollaborationMLInputChannelSummary, ConflictException, Dataset, PayerConfiguration,
    ResourceNotFoundException, TrainingDatasetSummary,
};

/// The region a minted ARN names when the request carries no SigV4 credential
/// scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN service segment every Clean Rooms ML resource carries, as
/// `OPERATIONS.md` records it ("ARN namespace: `cleanrooms-ml`") and as
/// `TrainingDatasetArn`'s own pattern requires
/// (`^arn:aws[-a-z]*:cleanrooms-ml:…`).
pub const ARN_SERVICE: &str = "cleanrooms-ml";

/// The largest page `ListTrainingDatasets` serves, and the page size it uses
/// when the request names no `maxResults`:
/// `ListTrainingDatasetsRequest$maxResults` is modelled
/// `@range(min: 1, max: 100)`.
pub const TRAINING_DATASET_PAGE_LIMIT: i32 = 100;

/// How many training datasets the store holds before `CreateTrainingDataset`
/// refuses, and how many audience models before `CreateAudienceModel` does.
///
/// Clean Rooms ML publishes adjustable per-account quotas on both, and this
/// emulator models neither Service Quotas nor an account's adjustments — so
/// this is the emulator's OWN bound on the store rather than a claim about any
/// account's limit. It is set far above any workflow an emulator serves, and it
/// is what stops the state growing without end: every record it holds is
/// released by that resource's delete, which is what frees room for the next
/// create.
pub const RESOURCE_QUOTA: usize = 1_000;

/// How long an audience model stays in the status `CreateAudienceModel`
/// returned before the clock owes it `ACTIVE`.
///
/// Clean Rooms ML trains the model asynchronously, so `AudienceModelStatus`
/// carries `CREATE_PENDING` / `CREATE_IN_PROGRESS` before `ACTIVE`; real
/// training takes far longer, and the emulator chooses this span. It is
/// deliberately NOT zero, so a caller reading the model straight after the
/// create still observes the transient status. The instant is stored on the
/// record and materialised by whichever later call reads it — nothing sleeps.
pub const AUDIENCE_MODEL_TRAINING: Duration = Duration::from_millis(500);

/// The status an audience model carries from the moment `CreateAudienceModel`
/// accepted it until [`AUDIENCE_MODEL_TRAINING`] has passed.
pub const AUDIENCE_MODEL_CREATING: &str = "CREATE_PENDING";

/// The status an audience model carries once its training window has passed,
/// and the only status `TrainingDatasetStatus` models at all.
pub const STATUS_ACTIVE: &str = "ACTIVE";

/// The largest page `ListCollaborationMLInputChannels` serves, and the page size
/// it uses when the request names no `maxResults`:
/// `ListCollaborationMLInputChannelsRequest$maxResults` is modelled
/// `@range(min: 1, max: 100)`.
pub const ML_INPUT_CHANNEL_PAGE_LIMIT: i32 = 100;

/// A status change the clock owes a resource: the instant it falls due, and the
/// status the resource carries from then on.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    pub due: SystemTime,
    pub status: String,
}

/// Materialise the status change `pending` owes `status`, if `now` is past its
/// due instant — the one place a clock-owed transition is settled, whichever
/// record holds it.
///
/// Only the status moves: a resource's `updateTime` reports the last call that
/// changed it, and the work this settles was started by the call that set the
/// transition, which already set that time.
fn settle_pending(status: &mut String, pending: &mut Option<PendingTransition>, now: SystemTime) {
    let Some(transition) = pending.as_ref() else {
        return;
    };
    if now < transition.due {
        return;
    }
    *status = transition.status.clone();
    *pending = None;
}

/// One Clean Rooms ML training dataset, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct TrainingDataset {
    /// The `TrainingDatasetArn` this record is keyed by.
    pub arn: String,
    /// The region the create was signed for, which the ARN names. It is the scope
    /// `CreateTrainingDatasetRequest$name`'s documented uniqueness is checked in
    /// ("unique in your account and region").
    pub region: String,
    pub name: String,
    pub role_arn: String,
    pub training_data: Vec<Dataset>,
    pub description: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// One of `TrainingDatasetStatus`'s values, which is `ACTIVE` and nothing
    /// else: a training dataset is metadata pointing at a Glue table, so the
    /// create has nothing to do asynchronously and the resource is ready when
    /// it returns.
    pub status: String,
    pub create_time: SystemTime,
    pub update_time: SystemTime,
}

impl TrainingDataset {
    /// The `TrainingDatasetSummary` projection `ListTrainingDatasets` returns.
    pub fn to_summary(&self) -> TrainingDatasetSummary {
        TrainingDatasetSummary {
            create_time: ::aws_smithy_types::DateTime::from(self.create_time),
            update_time: ::aws_smithy_types::DateTime::from(self.update_time),
            training_dataset_arn: self.arn.clone(),
            name: self.name.clone(),
            status: self.status.clone(),
            description: self.description.clone(),
        }
    }
}

/// One Clean Rooms ML audience model, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct AudienceModel {
    /// The `AudienceModelArn` this record is keyed by.
    pub arn: String,
    /// The region the create was signed for, which the ARN names. A regional
    /// endpoint reports only its own region's resources, so this is the scope
    /// every read of this record is checked against.
    pub region: String,
    pub name: String,
    /// The training dataset this model was trained from. `CreateAudienceModel`
    /// only accepts one that exists, so this always named a persisted training
    /// dataset at create time.
    pub training_dataset_arn: String,
    /// The training window the create named, held as the modelled timestamp the
    /// request carried so the get reports exactly what was stored.
    pub training_data_start_time: Option<::aws_smithy_types::DateTime>,
    pub training_data_end_time: Option<::aws_smithy_types::DateTime>,
    pub kms_key_arn: Option<String>,
    pub description: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// One of `AudienceModelStatus`'s values.
    pub status: String,
    /// The status change the clock owes this model, if any.
    pub pending: Option<PendingTransition>,
    pub create_time: SystemTime,
    pub update_time: SystemTime,
}

impl AudienceModel {
    /// Materialise the status change the clock owes this model, if `now` is past
    /// its due instant.
    pub fn settle(&mut self, now: SystemTime) {
        settle_pending(&mut self.status, &mut self.pending, now);
    }
}

/// One Clean Rooms ML input channel, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct MlInputChannel {
    /// The `MLInputChannelArn` this record is keyed by.
    pub arn: String,
    /// The region the create was signed for, which the ARN names. A regional
    /// endpoint reports only its own region's resources, so this is the scope
    /// every read of this record is checked against.
    pub region: String,
    /// The membership the channel was created under, which its ARN names.
    pub membership_identifier: String,
    /// The collaboration that membership belongs to. It is the scope
    /// `ListCollaborationMLInputChannels` reports in: a channel is visible to
    /// that call only through the collaboration it was created in.
    pub collaboration_identifier: String,
    pub name: String,
    /// The configured model algorithm associations the channel was created
    /// from, which `ConfiguredModelAlgorithmAssociationArnList` bounds to one
    /// entry.
    pub configured_model_algorithm_associations: Vec<String>,
    pub description: Option<String>,
    pub payer_configuration: Option<PayerConfiguration>,
    /// The account of the member that created the channel.
    pub creator_account_id: String,
    /// One of `MLInputChannelStatus`'s values.
    pub status: String,
    /// The status change the clock owes this channel, if any.
    pub pending: Option<PendingTransition>,
    pub create_time: SystemTime,
    pub update_time: SystemTime,
}

impl MlInputChannel {
    /// Materialise the status change the clock owes this channel, if `now` is
    /// past its due instant.
    pub fn settle(&mut self, now: SystemTime) {
        settle_pending(&mut self.status, &mut self.pending, now);
    }

    /// The `CollaborationMLInputChannelSummary` projection
    /// `ListCollaborationMLInputChannels` returns.
    pub fn to_collaboration_summary(&self) -> CollaborationMLInputChannelSummary {
        CollaborationMLInputChannelSummary {
            create_time: ::aws_smithy_types::DateTime::from(self.create_time),
            update_time: ::aws_smithy_types::DateTime::from(self.update_time),
            membership_identifier: self.membership_identifier.clone(),
            collaboration_identifier: self.collaboration_identifier.clone(),
            name: self.name.clone(),
            configured_model_algorithm_associations: self
                .configured_model_algorithm_associations
                .clone(),
            ml_input_channel_arn: self.arn.clone(),
            status: self.status.clone(),
            creator_account_id: self.creator_account_id.clone(),
            description: self.description.clone(),
            payer_configuration: self.payer_configuration.clone(),
        }
    }
}

/// Per-service state for com-amazonaws-cleanroomsml-awsstarkcontrolservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCleanroomsmlAwsstarkcontrolserviceState {
    /// Every training dataset, keyed by its ARN. A `BTreeMap` is what gives
    /// `ListTrainingDatasets` the stable total order its pagination token
    /// indexes into: ids are minted zero-padded, so ARNs sort in the order the
    /// datasets were created.
    pub training_datasets: BTreeMap<String, TrainingDataset>,
    /// Every audience model, keyed by its ARN.
    pub audience_models: BTreeMap<String, AudienceModel>,
    /// Every ML input channel, keyed by its ARN, which gives the
    /// collaboration- and membership-scoped lists the one stable total order
    /// their pagination tokens index into.
    pub ml_input_channels: BTreeMap<String, MlInputChannel>,
    /// The counter the resource segment of every minted ARN comes from.
    resource_id_seq: u64,
}

impl ComAmazonawsCleanroomsmlAwsstarkcontrolserviceState {
    /// Mint the resource id a new ARN carries: the hyphenated 32-hex-digit form
    /// Clean Rooms ML gives the resource segment of a training dataset or an
    /// audience model ARN, which `TrainingDatasetArn`'s
    /// `training-dataset/[-a-zA-Z0-9_/.]+` admits.
    ///
    /// Minted from a counter rather than at random, so the ids a run hands out
    /// are distinct, reproducible, and sort in the order they were minted.
    pub fn mint_resource_id(&mut self) -> String {
        self.resource_id_seq = self.resource_id_seq.wrapping_add(1);
        let hex = format!("{:032x}", self.resource_id_seq);
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }

    /// The ARNs of the persisted audience models trained from the training
    /// dataset `arn` names — the dependency `DeleteTrainingDataset` documents
    /// ("You can't delete a training dataset if there are any audience models
    /// that depend on the training dataset").
    pub fn audience_models_depending_on(&self, arn: &str) -> Vec<String> {
        self.audience_models
            .values()
            .filter(|model| model.training_dataset_arn == arn)
            .map(|model| model.arn.clone())
            .collect()
    }

    /// The training dataset `arn` names, if the endpoint `region` serves it.
    ///
    /// A Clean Rooms ML endpoint is regional: it reports the resources created
    /// under its own region and nothing else, so a record from another region is
    /// invisible here even though its ARN is well formed and the store holds it.
    pub fn training_dataset(&self, arn: &str, region: &str) -> Option<&TrainingDataset> {
        self.training_datasets
            .get(arn)
            .filter(|dataset| dataset.region == region)
    }

    /// The audience model `arn` names, if the endpoint `region` serves it, with
    /// any status change the clock owes it already materialised.
    pub fn settled_audience_model_mut(
        &mut self,
        arn: &str,
        region: &str,
        now: SystemTime,
    ) -> Option<&mut AudienceModel> {
        let model = self.audience_models.get_mut(arn)?;
        if model.region != region {
            return None;
        }
        model.settle(now);
        Some(model)
    }
}

/// The ARN of a training dataset with resource id `id`, in `region`.
pub fn training_dataset_arn(id: &str, region: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("training-dataset/{id}"))
}

/// The ARN of an audience model with resource id `id`, in `region`.
pub fn audience_model_arn(id: &str, region: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("audience-model/{id}"))
}

/// The ARN of the ML input channel with resource id `id`, created under the
/// membership `membership_identifier` names, in `region`.
///
/// `MLInputChannelArn` nests the channel under its membership
/// (`membership/{uuid}/ml-input-channel/{id}`), so the membership a channel
/// belongs to is readable from its ARN.
pub fn ml_input_channel_arn(membership_identifier: &str, id: &str, region: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("membership/{membership_identifier}/ml-input-channel/{id}"),
    )
}

/// The resource portion of `value` when it is shaped like a Clean Rooms ML ARN
/// (`^arn:aws[-a-z]*:cleanrooms-ml:[-a-z0-9]+:[0-9]{12}:` and a non-empty
/// remainder), and `None` otherwise.
///
/// Every ARN shape in this service shares that prefix and differs only in the
/// resource portion, so each predicate below checks the prefix through this and
/// then its own resource.
fn cleanrooms_ml_arn_resource(value: &str) -> Option<&str> {
    let mut fields = value.splitn(6, ':');
    let (Some("arn"), Some(partition), Some(service), Some(region), Some(account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    let partition_tail_ok = partition
        .strip_prefix("aws")
        .is_some_and(|tail| tail.bytes().all(|b| b == b'-' || b.is_ascii_lowercase()));
    let region_ok = !region.is_empty()
        && region
            .bytes()
            .all(|b| b == b'-' || b.is_ascii_lowercase() || b.is_ascii_digit());
    let account_ok = account.len() == 12 && account.bytes().all(|b| b.is_ascii_digit());
    (partition_tail_ok && service == ARN_SERVICE && region_ok && account_ok).then_some(resource)
}

/// True when `id` is shaped like the resource id Clean Rooms ML ARNs carry,
/// `[-a-zA-Z0-9_/.]+`.
fn is_arn_resource_id(id: &str) -> bool {
    !id.is_empty()
        && id.bytes().all(|b| {
            b == b'-' || b == b'_' || b == b'/' || b == b'.' || b.is_ascii_alphanumeric()
        })
}

/// True when `value` is shaped like a modelled `UUID`
/// (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`), the form
/// every collaboration and membership identifier in this service carries.
fn is_uuid(value: &str) -> bool {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let mut groups = value.split('-');
    for width in GROUPS {
        let Some(group) = groups.next() else {
            return false;
        };
        let hex = group
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
        if group.len() != width || !hex {
            return false;
        }
    }
    groups.next().is_none()
}

/// True when `value` is shaped like a `TrainingDatasetArn`
/// (`^arn:aws[-a-z]*:cleanrooms-ml:[-a-z0-9]+:[0-9]{12}:training-dataset/[-a-zA-Z0-9_/.]+$`).
///
/// The boundary validator already refuses a modelled `TrainingDatasetArn`
/// member, so this is for a value the model does not constrain to that
/// pattern — a `ListTrainingDatasets` `nextToken`, which this service's
/// pagination carries a training dataset ARN in.
pub fn is_training_dataset_arn(value: &str) -> bool {
    cleanrooms_ml_arn_resource(value).is_some_and(|resource| {
        resource
            .strip_prefix("training-dataset/")
            .is_some_and(is_arn_resource_id)
    })
}

/// True when `value` is shaped like an `MLInputChannelArn`
/// (`^arn:aws[-a-z]*:cleanrooms-ml:[-a-z0-9]+:[0-9]{12}:membership/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/ml-input-channel/[-a-zA-Z0-9_/.]+$`).
///
/// As with [`is_training_dataset_arn`], this is for a value the model does not
/// constrain to that pattern — the `nextToken` of a list that pages over ML
/// input channels, which carries the ARN the next page resumes at.
pub fn is_ml_input_channel_arn(value: &str) -> bool {
    let Some(resource) = cleanrooms_ml_arn_resource(value) else {
        return false;
    };
    let Some(nested) = resource.strip_prefix("membership/") else {
        return false;
    };
    let Some((membership_identifier, id)) = nested.split_once("/ml-input-channel/") else {
        return false;
    };
    is_uuid(membership_identifier) && is_arn_resource_id(id)
}

/// The region this request was signed for — what a minted ARN names.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The `ResourceNotFoundException` payload for an ARN that names no persisted
/// resource. `ResourceNotFoundException` models `message` and nothing else, so
/// the message is where the ARN that was asked about goes.
pub fn resource_not_found(arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("Resource {arn} not found"),
    }
}

/// The `ConflictException` payload for a resource another resource depends on.
pub fn dependency_conflict(arn: &str, dependents: &[String]) -> ConflictException {
    ConflictException {
        message: format!(
            "Resource {arn} cannot be deleted because {} resource(s) depend on it: {}",
            dependents.len(),
            dependents.join(", ")
        ),
    }
}

/// `tags` as the modelled `TagMap`, which the generated types hold as a
/// `HashMap`.
pub fn tag_map(tags: &BTreeMap<String, String>) -> HashMap<String, String> {
    tags.iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCleanroomsmlAwsstarkcontrolserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCleanroomsmlAwsstarkcontrolserviceState>)
}
