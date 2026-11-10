// SPDX-License-Identifier: CC0-1.0

//! Which treatment an inline override may assign an entity to.
//!
//! `TreatmentOverrides$Inline` is "a map of entity IDs to treatment keys. Each entry
//! assigns the specified entity to the specified treatment, bypassing random assignment"
//! (`smithy_query(query="doc",shape="TreatmentOverrides",member="Inline")`). So an entry's
//! VALUE names a treatment of the experiment the request addresses — the modelled
//! `TreatmentKey` pattern says what such a name may look like, not which names exist — and
//! a name no treatment of that experiment carries assigns the entity to nothing. AppConfig
//! refuses such an assignment with `BadRequestException` (HTTP 400) and writes nothing, so
//! a stale or mistyped key is reported at the call rather than silently delivering an
//! entity something other than what the caller pinned.
//!
//! The names a request may use are the keys this service minted for the experiment, read
//! from the stored record: the control's first, then one per compared treatment in the order
//! the definition lists them (see [`super::treatment_key`]). Every operation that writes
//! treatment overrides refuses through this one module, so the start side and the update
//! side cannot drift apart.

use crate::ops::types::{ExperimentDefinitionSnapshot, Treatment, TreatmentOverrides};

use super::ExperimentDefinition;

/// The treatment names `definition` defines, in the order AppConfig lists them.
pub fn definition_treatment_keys(definition: &ExperimentDefinition) -> Vec<String> {
    treatment_keys(Some(&definition.control), &definition.treatments)
}

/// The treatment names the definition a RUN was started from defines: the run carries that
/// definition as it stood at the start, and those are the treatments the run is delivering,
/// so they are the names an override of this run may assign to.
pub fn snapshot_treatment_keys(snapshot: &ExperimentDefinitionSnapshot) -> Vec<String> {
    treatment_keys(
        snapshot.control.as_ref(),
        snapshot.treatments.as_deref().unwrap_or(&[]),
    )
}

/// The keys of a control and the treatments compared against it, control first and the rest
/// in list order — the order a definition reports them in, which is the order the refusal
/// below states them in. A treatment carrying no key names nothing and is skipped rather
/// than contributing an empty name.
fn treatment_keys(control: Option<&Treatment>, treatments: &[Treatment]) -> Vec<String> {
    control
        .into_iter()
        .chain(treatments)
        .filter_map(|treatment| treatment.key.clone())
        .collect()
}

/// Why `overrides` assigns an entity to a treatment the experiment does not define, or
/// `None` when every assignment names one of `defined`.
///
/// The entries are read by entity id, so a request that assigns two undefined names is
/// refused with the same sentence whichever order the map happened to iterate in.
pub fn unknown_treatment_rejection(
    overrides: &TreatmentOverrides,
    defined: &[String],
) -> Option<String> {
    let TreatmentOverrides::Inline(assignments) = overrides;
    let mut entries: Vec<(&String, &String)> = assignments.iter().collect();
    entries.sort_by(|left, right| left.0.cmp(right.0));
    entries
        .into_iter()
        .map(|(_, treatment)| treatment.as_str())
        .find(|treatment| !defined.iter().any(|key| key == treatment))
        .map(|treatment| invalid_treatment_name(treatment, defined))
}

/// How AppConfig states that refusal: the name the request assigned, then the names it could
/// have assigned, in the order the experiment defines them.
fn invalid_treatment_name(treatment: &str, defined: &[String]) -> String {
    format!(
        "Invalid treatment name: {treatment}. Valid treatment names are: [{}]",
        defined.join(", ")
    )
}
