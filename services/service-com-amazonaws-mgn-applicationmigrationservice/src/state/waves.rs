// SPDX-License-Identifier: CC0-1.0

//! The WAVE concern of this service's state: the waves `CreateWave` mints and
//! `ListWaves` reports, and the refusals those operations word.
//!
//! A wave groups the applications a migration moves together. The applications
//! themselves record which wave they belong to (`Application::wave_id`), so the
//! `totalApplications` a wave reports is COUNTED from them rather than kept as a second
//! counter that could disagree with the records.

use super::*;

/// The most waves this store holds at a time — per account AND in total.
///
/// The emulator's own bound on a caller-driven growth site, not a claim about any AWS
/// quota: the same reasoning as [`APPLICATION_QUOTA`]. `CreateWave` checks
/// [`ComAmazonawsMgnApplicationmigrationserviceState::wave_store_is_full`] before it
/// mints anything and refuses past it with the modelled `ServiceQuotaExceededException`,
/// so the record count cannot be grown past the bound by varying the request's
/// `accountID`.
pub const WAVE_QUOTA: usize = 1_000;

/// The `waveID` prefix the model's `^wave-[0-9a-zA-Z]{17}$` pattern requires.
const WAVE_ID_PREFIX: &str = "wave-";

/// How many `[0-9a-zA-Z]` characters follow the prefix of a `WaveID`
/// (model pattern `^wave-[0-9a-zA-Z]{17}$`, length 22..22).
const WAVE_ID_BODY_LEN: usize = 17;

/// The `resourceType` a wave refusal names: the modelled resource shape
/// `com.amazonaws.mgn#WaveResource` without the `Resource` suffix, as the API spells the
/// kind.
const WAVE_RESOURCE_TYPE: &str = "Wave";

/// One stored wave: everything `CreateWave` and `ListWaves` report, so a read never
/// computes a value a write did not store — except `totalApplications`, which is counted
/// from the applications assigned to the wave (see
/// [`ComAmazonawsMgnApplicationmigrationserviceState::wave_shape`]).
#[derive(Debug, Clone, Default)]
pub struct Wave {
    pub wave_id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub is_archived: bool,
    /// The stored half of the wave's `WaveAggregatedStatus`. The three members this
    /// carries are the same ones an application's aggregated status stores, and the
    /// model's `WaveHealthStatus` / `WaveProgressStatus` declare the same values as their
    /// application counterparts, so the record is shared rather than duplicated.
    pub aggregated_status: AggregatedStatus,
    /// When the first source server of the wave started replicating, as the model's
    /// `replicationStartedDateTime` reports it; absent until something starts
    /// replication for one of the wave's source servers.
    pub replication_started_date_time: Option<String>,
    pub creation_date_time: String,
    pub last_modified_date_time: String,
    pub tags: BTreeMap<String, String>,
    /// The account the wave lives in (the request's `accountID`, or the emulator's own
    /// account when the request names none).
    pub account_id: String,
}

impl Wave {
    /// The `tags` member: the stored tags, or absent when the wave carries none.
    pub fn tags_shape(&self) -> Option<std::collections::HashMap<String, String>> {
        if self.tags.is_empty() {
            None
        } else {
            Some(
                self.tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            )
        }
    }

    /// This wave as the model's `Wave` structure — the output shape `CreateWave` returns
    /// and `ListWaves` lists.
    ///
    /// `total_applications` is passed in because it is not this record's to know: the
    /// assignment lives on the applications, so the store counts them (see
    /// [`ComAmazonawsMgnApplicationmigrationserviceState::wave_shape`]).
    pub fn to_shape(&self, total_applications: usize) -> CreateWaveOutput {
        CreateWaveOutput {
            wave_id: Some(self.wave_id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            is_archived: Some(self.is_archived),
            wave_aggregated_status: Some(WaveAggregatedStatus {
                last_update_date_time: Some(self.aggregated_status.last_update_date_time.clone()),
                replication_started_date_time: self.replication_started_date_time.clone(),
                health_status: self.aggregated_status.health_status.clone(),
                progress_status: self.aggregated_status.progress_status.clone(),
                total_applications: total_applications as i64,
            }),
            creation_date_time: Some(self.creation_date_time.clone()),
            last_modified_date_time: Some(self.last_modified_date_time.clone()),
            tags: self.tags_shape(),
        }
    }
}

impl ComAmazonawsMgnApplicationmigrationserviceState {
    /// Every wave of `account_id`, in creation order.
    pub fn waves_of(&self, account_id: &str) -> Vec<&Wave> {
        self.wave_order
            .iter()
            .filter_map(|id| self.waves.get(id))
            .filter(|wave| wave.account_id == account_id)
            .collect()
    }

    /// The wave `wave_id` names within `account_id`, or `None` — a wave of another
    /// account is not visible to this one.
    pub fn wave_of(&self, account_id: &str, wave_id: &str) -> Option<&Wave> {
        self.waves
            .get(wave_id)
            .filter(|wave| wave.account_id == account_id)
    }

    /// How many of the account's applications are assigned to the wave `wave_id`: the
    /// `totalApplications` its aggregated status reports.
    pub fn applications_in_wave(&self, account_id: &str, wave_id: &str) -> usize {
        self.applications
            .values()
            .filter(|app| {
                app.account_id == account_id
                    && app.wave_id.as_deref().is_some_and(|id| id == wave_id)
            })
            .count()
    }

    /// `wave` as the model's `Wave` structure, with `totalApplications` counted from the
    /// applications assigned to it — the ONE place a wave is rendered for the wire, so
    /// every operation that reports one reports the same members.
    pub fn wave_shape(&self, wave: &Wave) -> CreateWaveOutput {
        wave.to_shape(self.applications_in_wave(&wave.account_id, &wave.wave_id))
    }

    /// A fresh `waveID` no stored wave holds.
    pub fn mint_wave_id(&self) -> String {
        loop {
            let candidate = mint_id(WAVE_ID_PREFIX, WAVE_ID_BODY_LEN);
            if !self.waves.contains_key(&candidate) {
                return candidate;
            }
        }
    }

    /// Whether the wave store is full: either the account already holds [`WAVE_QUOTA`]
    /// waves, or the whole store does. Checked before a create mints anything, so
    /// neither the per-account count nor the total can be grown past the bound by
    /// varying the request's `accountID`.
    pub fn wave_store_is_full(&self, account_id: &str) -> bool {
        self.waves.len() >= WAVE_QUOTA || self.waves_of(account_id).len() >= WAVE_QUOTA
    }

    /// Store a newly created wave, remembering its place in creation order.
    ///
    /// Answers whether it was stored, exactly as [`Self::insert_application`] does: a
    /// store already at [`WAVE_QUOTA`] refuses rather than growing past the bound, and
    /// re-storing an id the map already holds keeps its place in the creation order
    /// rather than duplicating it.
    pub fn insert_wave(&mut self, wave: Wave) -> bool {
        let known = self.waves.contains_key(&wave.wave_id);
        if !known && self.waves.len() >= WAVE_QUOTA {
            return false;
        }
        if !known {
            self.wave_order.push(wave.wave_id.clone());
        }
        self.waves.insert(wave.wave_id.clone(), wave);
        true
    }
}

/// The ARN of a wave: `arn:aws:mgn:<region>:<account>:wave/<waveID>`.
pub fn wave_arn(region: &str, wave_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("wave/{wave_id}"))
}

/// The modelled refusal for a `name` another of the account's waves already carries:
/// within one account a wave name identifies the wave a caller asked for, so a second
/// one under the same name conflicts with it — the narrowing `CreateApplication` states
/// for an application name, which the model cannot express for either.
pub fn wave_name_in_use(name: &str, wave_id: &str) -> ConflictException {
    ConflictException {
        message: Some(format!("Wave {wave_id} already uses the name {name}.")),
        code: None,
        resource_id: Some(wave_id.to_string()),
        resource_type: Some(WAVE_RESOURCE_TYPE.to_string()),
        errors: None,
    }
}

/// The modelled refusal for an account already holding [`WAVE_QUOTA`] waves.
pub fn wave_quota_exceeded(account_id: &str) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: Some(format!(
            "Account {account_id} already holds the maximum of {WAVE_QUOTA} waves."
        )),
        code: None,
        resource_id: None,
        resource_type: Some(WAVE_RESOURCE_TYPE.to_string()),
        service_code: Some(ARN_NAMESPACE.to_string()),
        quota_code: None,
        quota_value: i32::try_from(WAVE_QUOTA).ok(),
    }
}
