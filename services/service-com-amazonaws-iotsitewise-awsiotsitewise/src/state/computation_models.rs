// SPDX-License-Identifier: CC0-1.0

//! The computation-model records of
//! [`super::ComAmazonawsIotsitewiseAwsiotsitewiseState`].

use super::*;

/// The ARN resource-type segment of a computation model's ARN, as the
/// `computationModelArn` member documents the form:
/// `arn:${Partition}:iotsitewise:${Region}:${Account}:computation-model/${ComputationModelId}`.
pub const COMPUTATION_MODEL_RESOURCE_TYPE: &str = "computation-model";

/// See [`MAX_ASSET_MODELS`]: the same bound, for the computation-model map.
/// `CreateComputationModel` is the only thing that adds a computation model and
/// it declares the modelled `LimitExceededException` (HTTP 410), so that is how
/// this store refuses past the cap. The emulator's own bound, not a published
/// AWS default.
pub const MAX_COMPUTATION_MODELS: usize = 1_000;

/// The `computationModelVersion` a freshly created computation model reports.
///
/// The version counts the writes the model has accepted — the create is the
/// first — so it is derived from the stored record rather than minted, and a
/// read never has to write for it to stay the same. The modelled `Version` type
/// is a decimal string (`^(0|([1-9]{1}\d*))$`), which is how the counter reaches
/// the wire.
pub const COMPUTATION_MODEL_INITIAL_VERSION: u64 = 1;

/// The `ComputationModelType` of a model whose configuration names anomaly
/// detection — the only value the modelled enum has (see
/// [`crate::ops::types::COMPUTATION_MODEL_TYPE_VALUES`]).
pub const COMPUTATION_MODEL_TYPE_ANOMALY_DETECTION: &str = "ANOMALY_DETECTION";

/// The `actionType` of each action an anomaly-detection computation model
/// exposes, as the model NAMES them: `DescribeExecution`'s `executionDetails`
/// documents "the training action (Amazon Web Services/ANOMALY_DETECTION_TRAINING)"
/// and "the inference action (Amazon Web Services/ANOMALY_DETECTION_INFERENCE)"
/// — the two actions such a model publishes, spelled the way the model spells
/// them rather than in a prefix form nothing in the model states.
///
/// The model carries no enum of action types, so this is the catalog for the one
/// [`COMPUTATION_MODEL_TYPE_ANOMALY_DETECTION`] computation type; a model of a
/// computation type this list does not cover exposes no action rather than a
/// guessed one.
pub const ANOMALY_DETECTION_ACTION_TYPES: [&str; 2] = [
    "Amazon Web Services/ANOMALY_DETECTION_TRAINING",
    "Amazon Web Services/ANOMALY_DETECTION_INFERENCE",
];

/// The opening and closing delimiters of the `${VariableName}` reference a
/// computation model's configuration names a data-binding variable with, as the
/// configuration members document the format ("Define the variable name
/// associated with input properties, with the following format
/// `${VariableName}`").
pub(super) const BINDING_REFERENCE_OPEN: &str = "${";
pub(super) const BINDING_REFERENCE_CLOSE: char = '}';

/// A stored computation model: everything `DescribeComputationModel` reports,
/// plus the `tags` the tagging operations read and the `clientToken` of the
/// create that made it.
///
/// `settling_state` holds a `ComputationModelState` value, which spells its
/// states exactly as `AssetState` does — `CREATING`, `ACTIVE`, `UPDATING`,
/// `DELETING`, `FAILED` — so this record reuses [`STATE_CREATING`] /
/// [`STATE_ACTIVE`] and the one [`state_at`] rule rather than a second spelling
/// of the same values.
#[derive(Debug, Clone)]
pub struct ComputationModelRecord {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub configuration: ComputationModelConfiguration,
    pub data_binding: HashMap<String, ComputationModelDataBindingValue>,
    /// The actions this model exposes, materialised ONCE by the create from the
    /// computation type its configuration names (see
    /// [`action_definitions_for`]), so every read reports the same ids.
    pub action_definitions: Vec<ActionDefinition>,
    pub creation_date: SystemTime,
    pub last_update_date: SystemTime,
    /// The instant the transition the clock owes — from [`Self::settling_state`]
    /// to `ACTIVE` — falls due.
    pub active_at: SystemTime,
    /// The `ComputationModelState` this model reports until [`Self::active_at`]
    /// falls due: [`STATE_CREATING`] for a freshly created model,
    /// [`STATE_UPDATING`] while a mutating call it already accepted settles.
    pub settling_state: String,
    /// How many writes this model has accepted, which is the
    /// `computationModelVersion` it reports; see
    /// [`COMPUTATION_MODEL_INITIAL_VERSION`].
    pub version: u64,
    /// The `clientToken` the create that made this model carried, if it carried
    /// one, recorded as part of what that call asked for. What recognises a
    /// RETRY of that call is the store's registry of spent tokens
    /// ([`ComAmazonawsIotsitewiseAwsiotsitewiseState::idempotent_verdict`]),
    /// which compares the request too: a token alone says only that some call
    /// used it, not that this one repeats it.
    pub client_token: Option<String>,
    pub tags: HashMap<String, String>,
}

impl ComputationModelRecord {
    /// The model's `ComputationModelState` as of `now`: its
    /// [`Self::settling_state`] until the owed transition falls due, `ACTIVE`
    /// from then on. Derived, so a read never has to write.
    pub fn state(&self, now: SystemTime) -> &str {
        state_at(&self.settling_state, self.active_at, now)
    }

    /// The model's status as of `now`.
    pub fn status(&self, now: SystemTime) -> ComputationModelStatus {
        ComputationModelStatus {
            state: self.state(now).to_string(),
            // No error: nothing in this emulator fails to provision a
            // computation model, so the modelled `error` details have nothing
            // to report.
            error: None,
        }
    }

    /// The `computationModelVersion` this model reports: its accepted-write
    /// count as the decimal string the modelled `Version` type is.
    pub fn version_label(&self) -> String {
        self.version.to_string()
    }

    /// Record that a mutating call — an update of this model's definition —
    /// landed at `now`: the definition now stored counts one write more, so it
    /// carries the next version, and the model reports `UPDATING` until the
    /// transition that call owes falls due.
    ///
    /// The due instant is STORED, never slept on, exactly as
    /// [`AssetRecord::begin_update`] does it for an asset, so the state a later
    /// read reports stays derived from the clock.
    pub fn begin_update(&mut self, now: SystemTime) {
        self.last_update_date = now;
        self.version += 1;
        self.settling_state = STATE_UPDATING.to_string();
        self.active_at = now + ACTIVATION_DELAY;
    }

    /// Whether this model answers a read scoped to the version the
    /// `ComputationModelVersionFilter` `filter` names (`LATEST | ACTIVE | <n>`),
    /// as of `now`.
    ///
    /// This emulator keeps ONE version of a computation model, so that version
    /// is always its latest one — which is what an absent filter and `LATEST`
    /// ask for — and is also its ACTIVE one once the transition the clock owes
    /// has fallen due; while one is still pending the model has no active
    /// version at all, since nothing here keeps a definition a write replaced.
    /// An explicit number therefore names a version this store
    /// holds only when it is that one version's own number.
    pub fn serves_version(&self, filter: Option<&str>, now: SystemTime) -> bool {
        match filter {
            None | Some(VERSION_TYPE_LATEST) => true,
            Some(VERSION_TYPE_ACTIVE) => self.state(now) == STATE_ACTIVE,
            Some(explicit) => explicit == self.version_label().as_str(),
        }
    }
}

/// The canonical ARN of a computation model in `region`; see
/// [`COMPUTATION_MODEL_RESOURCE_TYPE`].
pub fn computation_model_arn(region: &str, computation_model_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{COMPUTATION_MODEL_RESOURCE_TYPE}/{computation_model_id}"),
    )
}

/// The `ComputationModelType` `configuration` names, or `None` when it names
/// none.
///
/// `ComputationModelConfiguration` carries one optional member per computation
/// type, so the type of a model is DERIVED from the configuration it was created
/// with rather than stored beside it and able to drift.
pub fn computation_model_type(
    configuration: &ComputationModelConfiguration,
) -> Option<&'static str> {
    configuration
        .anomaly_detection
        .as_ref()
        .map(|_| COMPUTATION_MODEL_TYPE_ANOMALY_DETECTION)
}

/// The action definitions a computation model of `model_type` exposes, each with
/// a freshly minted `actionDefinitionId`.
///
/// Called ONCE, by the create, and stored on the model: the ids are then stable
/// across reads and unique per model, and a read of the model's available
/// actions writes nothing.
pub fn action_definitions_for(model_type: &str) -> Vec<ActionDefinition> {
    let action_types: &[&str] = match model_type {
        COMPUTATION_MODEL_TYPE_ANOMALY_DETECTION => &ANOMALY_DETECTION_ACTION_TYPES,
        _ => &[],
    };
    action_types
        .iter()
        .map(|action_type| ActionDefinition {
            action_definition_id: new_id(),
            // The model documents `actionName` as "The name of the action
            // definition" and `actionType` as "The type of the action
            // definition", and an action of a computation model is addressed by
            // the kind of work it does, so each action is named for its type.
            action_name: (*action_type).to_string(),
            action_type: (*action_type).to_string(),
        })
        .collect()
}

/// The data-binding variable a `${VariableName}` reference names, or `None` when
/// `reference` is not in that form; see [`BINDING_REFERENCE_OPEN`].
pub(super) fn binding_variable(reference: &str) -> Option<&str> {
    reference
        .strip_prefix(BINDING_REFERENCE_OPEN)?
        .strip_suffix(BINDING_REFERENCE_CLOSE)
        .filter(|variable| !variable.is_empty())
}

/// Refuse a computation model whose configuration and data binding do not
/// describe one model, in the one place every operation that supplies those two
/// shapes holds the rule.
///
/// Three things have to hold:
///
/// * the configuration names a computation type — it carries one optional member
///   per type, and a configuration naming none describes no computation at all;
/// * every `${VariableName}` the configuration references is a variable the data
///   binding defines, which is what the binding is documented to be ("Key is a
///   variable name defined in configuration");
/// * every binding value sets exactly one of its members, which is what
///   `ComputationModelDataBindingValue` documents ("can be one of
///   `assetModelProperty`, `list`") — checked through a `list` as well, since a
///   list's elements are binding values themselves, down to
///   [`MAX_BINDING_VALUE_DEPTH`], past which the value is refused rather than
///   walked further.
pub fn check_computation_model(
    configuration: &ComputationModelConfiguration,
    data_binding: &HashMap<String, ComputationModelDataBindingValue>,
) -> Result<(), String> {
    let Some(model_type) = computation_model_type(configuration) else {
        return Err("computationModelConfiguration must name exactly one computation type."
            .to_string());
    };
    for (member, reference) in configuration_references(configuration) {
        let Some(variable) = binding_variable(reference) else {
            return Err(format!(
                "computationModelConfiguration.{model_type}.{member} must reference a data \
                 binding variable in the form ${{VariableName}}, but is {reference}."
            ));
        };
        if !data_binding.contains_key(variable) {
            return Err(format!(
                "computationModelConfiguration.{model_type}.{member} references the variable \
                 {variable}, which computationModelDataBinding does not define."
            ));
        }
    }
    for (variable, value) in data_binding {
        check_binding_value(variable, value, 1)?;
    }
    Ok(())
}
