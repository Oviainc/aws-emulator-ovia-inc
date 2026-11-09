// SPDX-License-Identifier: CC0-1.0

//! Modifying an instance fleet a cluster already carries: which capacities Amazon EMR
//! will hold a fleet to, and what a modification leaves on the fleet's record.
//!
//! One concern, split out of [`super`]: `ModifyInstanceFleet` "modifies the target
//! On-Demand and target Spot capacities for the instance fleet" (that operation's doc,
//! Smithy model), so the submitted configuration is applied to the stored
//! [`InstanceFleetRecord`] ([`InstanceFleetRecord::apply_modification`]) and every later
//! `ListInstanceFleets` reports what the modification left there.
//!
//! A target capacity is a number of Amazon EC2 units, so no fleet can be asked for a
//! NEGATIVE one: Amazon EMR checks the submitted capacities itself and refuses such a
//! request with its own modelled client error, naming the value it rejected and the fleet
//! the request named ([`instance_fleet_capacity_violation`]). The rule is stated once here
//! because the request reaches it by two paths — the handler, and the order-restoring
//! layer in [`crate::service`] for a value the modelled `@range(min: 0)` makes the generic
//! request validator reach first — so the two cannot word one refusal differently.

use super::*;

/// How an instance-fleet-scoped operation refuses an `InstanceFleetId` the named cluster
/// carries no fleet for.
///
/// AWS does not document the wording, so the text is built from the request: it names the
/// id the caller asked for, through the same tail [`unknown_instance_group_message`] uses.
/// What IS the contract is the refusal itself — HTTP 400 with the client error the
/// operation models — rather than an empty success.
pub fn unknown_instance_fleet_message(instance_fleet_id: &str) -> String {
    format!("Instance fleet id {}", not_valid_message(instance_fleet_id))
}

/// How Amazon EMR refuses a target capacity no fleet could be asked for: the value the
/// request sent, the fleet it named, and the rule the value broke.
///
/// Recorded behavior for the text. Its variable parts both come from the REQUEST — the
/// rejected capacity exactly as it was submitted, and the id of the fleet it was submitted
/// for — because a caller fixing the call has to know which of the two capacities of which
/// fleet it has to change.
pub fn invalid_instance_fleet_capacity_message(capacity: i64, instance_fleet_id: &str) -> String {
    format!(
        "Invalid instance fleet capacity '{capacity}' specified for instance fleet \
         '{instance_fleet_id}', must be a non-negative number."
    )
}

/// The refusal an instance fleet modification answers an unusable target capacity with:
/// that sentence under the client error the operation models.
///
/// No `ErrorCode`: Amazon EMR publishes no discriminator for this class, the way it does
/// for an unresolvable cluster id ([`unknown_cluster`]).
pub fn invalid_instance_fleet_capacity(
    capacity: i64,
    instance_fleet_id: &str,
) -> InvalidRequestException {
    InvalidRequestException {
        error_code: None,
        message: Some(invalid_instance_fleet_capacity_message(
            capacity,
            instance_fleet_id,
        )),
    }
}

/// Why this fleet modification's capacities cannot be accepted, if they cannot.
///
/// `TargetOnDemandCapacity` is "the target capacity of On-Demand units for the instance
/// fleet" and `TargetSpotCapacity` the same for Spot units (`InstanceFleetModifyConfig`
/// member docs, Smithy model): a count of units, so a negative one names no capacity the
/// fleet could be resized to. A capacity the request omits asks for no change and is not
/// judged, and the two are judged in the order the model declares them, so a request
/// getting both wrong is refused for the first.
///
/// Takes the capacities as `i64` so the same rule serves the typed handler input and the
/// raw request body the order-restoring layer reads them out of.
pub fn instance_fleet_capacity_violation(
    instance_fleet_id: &str,
    target_on_demand_capacity: Option<i64>,
    target_spot_capacity: Option<i64>,
) -> Option<InvalidRequestException> {
    [target_on_demand_capacity, target_spot_capacity]
        .into_iter()
        .flatten()
        .find(|capacity| *capacity < 0)
        .map(|capacity| invalid_instance_fleet_capacity(capacity, instance_fleet_id))
}

/// How many instance type configurations one instance fleet holds at once.
///
/// CHOSEN, not documented: every submitted configuration is retained on the fleet's
/// record and reported on every later read of it, and a modification MERGES into what is
/// already there, so the stored list needs a ceiling of its own — the same reasoning that
/// bounds an instance group's configuration tree
/// ([`MAX_INSTANCE_GROUP_CONFIGURATIONS`]).
pub const MAX_INSTANCE_TYPES_PER_FLEET: usize = 30;

/// Why this modification's instance type configurations cannot be accepted, if they
/// cannot: the merge they ask for would leave the fleet holding more than
/// [`MAX_INSTANCE_TYPES_PER_FLEET`] of them.
///
/// The SIZE the merge would leave behind is computed without copying either list, so a
/// request past the ceiling is refused before anything is allocated for it and the fleet
/// is left exactly as it was. The count is the resulting one rather than the submitted
/// length, because a configuration for an instance type the fleet already holds replaces
/// it rather than adding another ([`InstanceFleetRecord::apply_modification`]).
pub fn instance_type_configs_violation(
    instance_fleet_id: &str,
    held: &[InstanceTypeConfig],
    submitted: &[InstanceTypeConfig],
) -> Option<InvalidRequestException> {
    let carried: HashSet<&str> = held
        .iter()
        .map(|config| config.instance_type.as_str())
        .collect();
    let added: HashSet<&str> = submitted
        .iter()
        .map(|config| config.instance_type.as_str())
        .filter(|instance_type| !carried.contains(instance_type))
        .collect();
    if held.len() + added.len() <= MAX_INSTANCE_TYPES_PER_FLEET {
        return None;
    }
    Some(InvalidRequestException {
        error_code: None,
        message: Some(format!(
            "InstanceTypeConfigs would leave instance fleet '{instance_fleet_id}' holding \
             more than the {MAX_INSTANCE_TYPES_PER_FLEET} instance type configurations an \
             instance fleet may hold"
        )),
    })
}

impl ClusterRecord {
    /// The instance fleet `instance_fleet_id` of this cluster, for an operation that
    /// changes it.
    pub fn fleet_mut(&mut self, instance_fleet_id: &str) -> Option<&mut InstanceFleetRecord> {
        self.fleets
            .iter_mut()
            .find(|fleet| fleet.id == instance_fleet_id)
    }
}

impl InstanceFleetRecord {
    /// Apply a submitted `InstanceFleetModifyConfig` to this fleet.
    ///
    /// Every member the configuration carries replaces what the fleet holds for it, and a
    /// member it omits leaves that part of the fleet exactly as it was — "modifies the
    /// target On-Demand and target Spot capacities" is a change to the targets alone, and
    /// the same holds for the resize specification and the reserved context beside them.
    ///
    /// `InstanceTypeConfigs` is "an array of InstanceTypeConfig objects that specify how
    /// Amazon EMR provisions Amazon EC2 instances when it fulfills On-Demand and Spot
    /// capacities" (`InstanceFleetModifyConfig$InstanceTypeConfigs` doc), and one fleet
    /// provisions one kind of instance once, so a submitted entry whose instance type the
    /// fleet already holds a configuration for REPLACES it and one it does not is added to
    /// the end — the merge a tag set gets ([`merge_tags`]), keyed by instance type.
    pub fn apply_modification(&mut self, modification: &InstanceFleetModifyConfig) {
        if let Some(capacity) = modification.target_on_demand_capacity {
            self.config.target_on_demand_capacity = Some(capacity);
        }
        if let Some(capacity) = modification.target_spot_capacity {
            self.config.target_spot_capacity = Some(capacity);
        }
        if let Some(resize_specifications) = modification.resize_specifications.as_ref() {
            self.config.resize_specifications = Some(resize_specifications.clone());
        }
        if let Some(context) = modification.context.as_ref() {
            self.config.context = Some(context.clone());
        }
        if let Some(submitted) = modification.instance_type_configs.as_deref() {
            let mut held = self.config.instance_type_configs.clone().unwrap_or_default();
            for config in submitted {
                match held
                    .iter_mut()
                    .find(|holding| holding.instance_type == config.instance_type)
                {
                    Some(holding) => *holding = config.clone(),
                    None => held.push(config.clone()),
                }
            }
            self.config.instance_type_configs = Some(held);
        }
    }
}
