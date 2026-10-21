// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-workspacesthinclient-thinclient service.
//!
//! The service's primary resource is the *environment*: every environment the
//! control plane mints lives here, and every operation computes its response from
//! this store. The ids, ARNs, activation codes and timestamps an environment
//! carries are generated ONCE, when it is created, and never regenerated on a
//! read.
//!
//! # Every growth site here is bounded, and the store can shrink
//!
//! * `environments` is capped at [`ENVIRONMENT_QUOTA`]; `CreateEnvironment`
//!   refuses past it with the modelled `ServiceQuotaExceededException`.
//! * `environment_order`, `issued_environment_ids`, `issued_activation_codes`
//!   and `create_tokens` hold one entry per live environment, all released by
//!   [`ComAmazonawsWorkspacesthinclientThinclientState::release_environment`].
//! * `tags` holds one map per live environment, each capped at [`MAX_TAGS`]
//!   entries of at most [`MAX_TAG_KEY_CHARS`] / [`MAX_TAG_VALUE_CHARS`]
//!   characters, enforced by [`bound_tags`] before anything is stored.
//! * `delete_tokens` is capped at [`DELETE_TOKEN_HISTORY`], oldest evicted
//!   first, because a delete releases the record its token names.
//! * every other stored string and map is length-constrained by the model, so
//!   the boundary validator refuses an oversized one before a handler runs.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::ops::types::{Environment, EnvironmentSummary, MaintenanceWindow, DESKTOP_TYPE_VALUES};

/// This service's ARN namespace and sigv4 signing name (`OPERATIONS.md`, "Wire
/// protocol"), used as the service segment of every ARN it mints.
pub const ARN_SERVICE: &str = "thinclient";

/// The region an ARN carries when the request names none.
pub const DEFAULT_REGION: &str = "us-east-1";

/// Characters an `EnvironmentId` (`^[a-z0-9]{9}$`) is drawn from.
const ID_ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// Characters the first two positions of an `ActivationCode`
/// (`^[a-z]{2}[a-z0-9]{6}$`) are drawn from.
const LETTER_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz";

/// Length of an `EnvironmentId`, from the model's `^[a-z0-9]{9}$`.
const ENVIRONMENT_ID_LEN: usize = 9;

/// Length of an `ActivationCode`, from the model's `^[a-z]{2}[a-z0-9]{6}$`.
const ACTIVATION_CODE_LEN: usize = 8;

/// `softwareSetComplianceStatus` for an environment no device has registered
/// with — the `EnvironmentSoftwareSetComplianceStatus` value the model declares
/// for exactly that case.
const NO_REGISTERED_DEVICES: &str = "NO_REGISTERED_DEVICES";

/// Prefix every `ListEnvironments` pagination token carries, so a token minted
/// by another API (or hand-written) is recognised as invalid rather than read as
/// a cursor. The token is opaque to clients: only this module reads it.
const PAGE_TOKEN_PREFIX: &str = "tc-env-";

/// The largest page `ListEnvironments` returns when the caller names no
/// `maxResults`, taken from the model's `MaxResults` range (`1..50`).
pub const MAX_PAGE_SIZE: i32 = 50;

/// The `resourceType` this service reports for an id or ARN that names no
/// environment.
pub const ENVIRONMENT_RESOURCE_TYPE: &str = "environment";

/// How many environments the store holds before `CreateEnvironment` refuses with
/// the modelled `ServiceQuotaExceededException`.
///
/// WorkSpaces Thin Client publishes an adjustable quota on environments per
/// account per Region, and this emulator models neither Service Quotas nor an
/// account's adjustments — so this is the emulator's OWN bound on the store
/// rather than a claim about any account's limit. It is set far above any
/// workflow an emulator serves, and it is what stops the state growing without
/// end: every record it holds is released by `DeleteEnvironment`, which also
/// forgets that environment's id, activation code, tags and client tokens, so
/// none of those maps can outgrow this bound either.
pub const ENVIRONMENT_QUOTA: usize = 1_000;

/// How many `DeleteEnvironment` client tokens the store remembers, so a retried
/// delete still reports the original success.
///
/// A delete releases its environment, so the live-record bound above cannot bound
/// this map: it is bounded here instead, oldest-first, which is also what makes a
/// replay window finite. Far wider than any retry an SDK performs.
pub const DELETE_TOKEN_HISTORY: usize = 256;

/// AWS's tagging limit of 50 tags per resource.
///
/// The model constrains `DeviceCreationTagsMap` to 0..50 entries, which the
/// boundary validator enforces before a handler runs, but it puts NO length on
/// `TagsMap` — so this is where the same limit is applied to `tags`, both the
/// map `CreateEnvironment` carries and the merge `TagResource` performs.
pub const MAX_TAGS: usize = 50;

/// AWS's tagging limit on a tag key: 128 characters. (The model states it for
/// `DeviceCreationTagKey`, 1..128, and nothing for `TagsMap`'s key.)
pub const MAX_TAG_KEY_CHARS: usize = 128;

/// AWS's tagging limit on a tag value: 256 characters. (The model states it for
/// `DeviceCreationTagValue`, 0..256, and nothing for `TagsMap`'s value.)
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// Refuse a caller-sized tag map that breaks the tagging limits, or that would
/// take a resource past [`MAX_TAGS`] tags once written.
///
/// `resulting_count` is how many tags the resource would hold AFTER the write,
/// so a merge is judged on its result rather than on one request's size. `Err`
/// carries the message, naming the bound it broke, for the caller to fix the
/// request from; the caller refuses WITHOUT storing anything.
pub fn bound_tags(tags: &HashMap<String, String>, resulting_count: usize) -> Result<(), String> {
    if resulting_count > MAX_TAGS {
        return Err(format!(
            "a resource may carry at most {MAX_TAGS} tags, and this request would leave it \
             with {resulting_count}"
        ));
    }
    for (key, value) in tags {
        let key_chars = key.chars().count();
        if key_chars > MAX_TAG_KEY_CHARS {
            return Err(format!(
                "a tag key may be at most {MAX_TAG_KEY_CHARS} characters, got {key_chars}"
            ));
        }
        let value_chars = value.chars().count();
        if value_chars > MAX_TAG_VALUE_CHARS {
            return Err(format!(
                "a tag value may be at most {MAX_TAG_VALUE_CHARS} characters, got {value_chars}"
            ));
        }
    }
    Ok(())
}

/// An environment exactly as the service stores it.
///
/// Every member of the modelled `Environment` shape has a home here, so a read
/// projects stored bytes rather than recomputing anything.
#[derive(Debug, Clone)]
pub struct StoredEnvironment {
    /// Creation order, ascending. `ListEnvironments` pages on this, so deleting an
    /// environment never shifts another one's cursor.
    pub seq: u64,
    pub id: String,
    pub arn: String,
    pub name: Option<String>,
    pub desktop_arn: String,
    pub desktop_endpoint: Option<String>,
    pub desktop_type: String,
    pub activation_code: String,
    pub registered_devices_count: i32,
    pub software_set_update_schedule: Option<String>,
    pub software_set_update_mode: Option<String>,
    pub desired_software_set_id: Option<String>,
    pub pending_software_set_id: Option<String>,
    pub pending_software_set_version: Option<String>,
    /// The compliance status reported by the devices registered to this
    /// environment. `None` while nothing has reported; with no registered device
    /// the projection answers `NO_REGISTERED_DEVICES`.
    pub software_set_compliance_status: Option<String>,
    pub maintenance_window: Option<MaintenanceWindow>,
    pub kms_key_arn: Option<String>,
    pub device_creation_tags: HashMap<String, String>,
    pub created_at_millis: u64,
    pub updated_at_millis: u64,
}

impl StoredEnvironment {
    /// `softwareSetComplianceStatus`: with no registered device the environment
    /// reports `NO_REGISTERED_DEVICES`; otherwise whatever its devices reported.
    fn compliance_status(&self) -> Option<String> {
        if self.registered_devices_count == 0 {
            return Some(NO_REGISTERED_DEVICES.to_string());
        }
        self.software_set_compliance_status.clone()
    }

    /// The stored record projected into the modelled `EnvironmentSummary`.
    pub fn to_summary(&self) -> EnvironmentSummary {
        EnvironmentSummary {
            id: Some(self.id.clone()),
            name: self.name.clone(),
            desktop_arn: Some(self.desktop_arn.clone()),
            desktop_endpoint: self.desktop_endpoint.clone(),
            desktop_type: Some(self.desktop_type.clone()),
            activation_code: Some(self.activation_code.clone()),
            software_set_update_schedule: self.software_set_update_schedule.clone(),
            maintenance_window: self.maintenance_window.clone(),
            software_set_update_mode: self.software_set_update_mode.clone(),
            desired_software_set_id: self.desired_software_set_id.clone(),
            pending_software_set_id: self.pending_software_set_id.clone(),
            created_at: Some(millis_to_date_time(self.created_at_millis)),
            updated_at: Some(millis_to_date_time(self.updated_at_millis)),
            arn: Some(self.arn.clone()),
        }
    }

    /// The stored record projected into the modelled `Environment`.
    pub fn to_environment(&self) -> Environment {
        Environment {
            id: Some(self.id.clone()),
            name: self.name.clone(),
            desktop_arn: Some(self.desktop_arn.clone()),
            desktop_endpoint: self.desktop_endpoint.clone(),
            desktop_type: Some(self.desktop_type.clone()),
            activation_code: Some(self.activation_code.clone()),
            registered_devices_count: Some(self.registered_devices_count),
            software_set_update_schedule: self.software_set_update_schedule.clone(),
            maintenance_window: self.maintenance_window.clone(),
            software_set_update_mode: self.software_set_update_mode.clone(),
            desired_software_set_id: self.desired_software_set_id.clone(),
            pending_software_set_id: self.pending_software_set_id.clone(),
            pending_software_set_version: self.pending_software_set_version.clone(),
            software_set_compliance_status: self.compliance_status(),
            created_at: Some(millis_to_date_time(self.created_at_millis)),
            updated_at: Some(millis_to_date_time(self.updated_at_millis)),
            arn: Some(self.arn.clone()),
            kms_key_arn: self.kms_key_arn.clone(),
            device_creation_tags: Some(self.device_creation_tags.clone()),
        }
    }
}

/// The parameters a `CreateEnvironment` call carried, in a form that compares by
/// value.
///
/// `clientToken` is documented as requiring that "you also pass the same value
/// for all other parameters", so a replay is only a replay when every parameter
/// agrees; this is what the stored token is compared against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateEnvironmentFingerprint {
    pub name: Option<String>,
    pub desktop_arn: String,
    pub desktop_endpoint: Option<String>,
    pub software_set_update_schedule: Option<String>,
    pub software_set_update_mode: Option<String>,
    pub desired_software_set_id: Option<String>,
    pub kms_key_arn: Option<String>,
    pub maintenance_window: Option<Vec<String>>,
    pub tags: Vec<(String, String)>,
    pub device_creation_tags: Vec<(String, String)>,
}

/// A map flattened into key order, so two maps with the same entries compare
/// equal whatever order they were deserialized in.
pub fn sorted_pairs(map: Option<&HashMap<String, String>>) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = map
        .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    pairs.sort();
    pairs
}

/// A maintenance window flattened into comparable text (the shape itself is not
/// `PartialEq`), with the days of the week in a canonical order.
pub fn maintenance_window_key(window: Option<&MaintenanceWindow>) -> Option<Vec<String>> {
    let window = window?;
    let mut days = window.days_of_the_week.clone().unwrap_or_default();
    days.sort();
    let mut key = vec![
        window.r#type.clone(),
        optional_number(window.start_time_hour),
        optional_number(window.start_time_minute),
        optional_number(window.end_time_hour),
        optional_number(window.end_time_minute),
        window.apply_time_of.clone().unwrap_or_default(),
    ];
    key.extend(days);
    Some(key)
}

/// An optional number as comparable text; an absent one as the empty string.
fn optional_number(value: Option<i32>) -> String {
    value.map(|v| v.to_string()).unwrap_or_default()
}

/// What a recorded `clientToken` resolved to, and the request that minted it.
#[derive(Debug, Clone)]
pub struct CreateEnvironmentToken {
    pub environment_id: String,
    pub fingerprint: CreateEnvironmentFingerprint,
}

/// Per-service state for com-amazonaws-workspacesthinclient-thinclient. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsWorkspacesthinclientThinclientState {
    /// Every live environment, keyed by its `^[a-z0-9]{9}$` id.
    pub environments: HashMap<String, StoredEnvironment>,
    /// Environment ids in creation order, so `ListEnvironments` pages stably.
    pub environment_order: Vec<String>,
    /// Tags, keyed by the ARN of the resource they are attached to.
    pub tags: HashMap<String, HashMap<String, String>>,
    /// `CreateEnvironment` idempotency: a replayed `clientToken` resolves to the
    /// environment the first call minted.
    pub create_tokens: HashMap<String, CreateEnvironmentToken>,
    /// `DeleteEnvironment` idempotency: a replayed `clientToken` reports the
    /// original success rather than a 404 for the record it already removed.
    /// Bounded at [`DELETE_TOKEN_HISTORY`] entries by
    /// [`Self::record_delete_token`].
    pub delete_tokens: HashMap<String, String>,
    /// The order `delete_tokens` were recorded in, so the oldest is the one
    /// evicted when the history is full.
    delete_token_order: VecDeque<String>,
    /// The id of every LIVE environment, so a mint can reject a collision in
    /// O(1). [`Self::release_environment`] drops the entry with the record, so
    /// this set never outgrows [`ENVIRONMENT_QUOTA`].
    issued_environment_ids: HashSet<String>,
    /// The activation code of every LIVE environment, for the same reason.
    issued_activation_codes: HashSet<String>,
    /// Next creation sequence number to hand out.
    next_seq: u64,
    /// The last timestamp issued, in epoch milliseconds. Timestamps are handed
    /// out strictly increasing so two writes landing in the same millisecond are
    /// still ordered (`updatedAt` after an update is always later than before).
    /// Issue the next one with [`issue_timestamp_millis`].
    pub last_timestamp_millis: u64,
    /// Draw counter mixed into every minted id, so two ids minted in the same
    /// millisecond still differ.
    mint_counter: u64,
}

impl ComAmazonawsWorkspacesthinclientThinclientState {
    /// The creation sequence number for the next environment.
    pub fn next_seq(&mut self) -> u64 {
        self.next_seq += 1;
        self.next_seq
    }

    /// The current time in epoch milliseconds, never earlier than (or equal to)
    /// the last value this store handed out.
    pub fn next_timestamp_millis(&mut self) -> u64 {
        issue_timestamp_millis(&mut self.last_timestamp_millis)
    }

    /// A fresh `^[a-z0-9]{9}$` environment id no live environment holds.
    pub fn mint_environment_id(&mut self) -> String {
        loop {
            let candidate = render(self.draw(), ENVIRONMENT_ID_LEN, ID_ALPHABET);
            if self.issued_environment_ids.insert(candidate.clone()) {
                return candidate;
            }
        }
    }

    /// A fresh `^[a-z]{2}[a-z0-9]{6}$` activation code no live environment holds.
    pub fn mint_activation_code(&mut self) -> String {
        loop {
            let mut candidate = render(self.draw(), 2, LETTER_ALPHABET);
            candidate.push_str(&render(self.draw(), ACTIVATION_CODE_LEN - 2, ID_ALPHABET));
            if self.issued_activation_codes.insert(candidate.clone()) {
                return candidate;
            }
        }
    }

    /// The id of the environment an ARN names, if this store holds it.
    pub fn environment_id_for_arn(&self, arn: &str) -> Option<String> {
        self.environments
            .values()
            .find(|env| env.arn == arn)
            .map(|env| env.id.clone())
    }

    /// Live environments in creation order.
    pub fn ordered_environments(&self) -> Vec<&StoredEnvironment> {
        self.environment_order
            .iter()
            .filter_map(|id| self.environments.get(id))
            .collect()
    }

    /// Everything this store holds for one environment, released together: the
    /// record, its position in the listing order, its tags, the create tokens
    /// that named it, and the id and activation code it held.
    ///
    /// This is what lets the store SHRINK — down to empty once every emulated
    /// environment is deleted — so no map here outlives the resources it
    /// describes.
    pub fn release_environment(&mut self, id: &str) -> Option<StoredEnvironment> {
        let removed = self.environments.remove(id)?;
        self.environment_order.retain(|held| held != id);
        self.tags.remove(&removed.arn);
        self.create_tokens
            .retain(|_, recorded| recorded.environment_id != id);
        self.issued_environment_ids.remove(id);
        self.issued_activation_codes
            .remove(&removed.activation_code);
        Some(removed)
    }

    /// Remember that `token` deleted `environment_id`, keeping at most
    /// [`DELETE_TOKEN_HISTORY`] tokens: the oldest is evicted to make room, so a
    /// stream of deletes cannot grow this map without end.
    pub fn record_delete_token(&mut self, token: String, environment_id: String) {
        if self.delete_tokens.insert(token.clone(), environment_id).is_none() {
            self.delete_token_order.push_back(token);
        }
        while self.delete_token_order.len() > DELETE_TOKEN_HISTORY {
            if let Some(evicted) = self.delete_token_order.pop_front() {
                self.delete_tokens.remove(&evicted);
            }
        }
    }

    /// A pseudo-random 64-bit draw. The seed is the process's randomly-keyed
    /// `RandomState` plus the wall clock and a per-store counter, so ids minted
    /// by one emulator never repeat and are not predictable from each other.
    fn draw(&mut self) -> u64 {
        use std::hash::{BuildHasher, Hasher};
        self.mint_counter = self.mint_counter.wrapping_add(1);
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(self.mint_counter);
        hasher.write_u64(unix_millis());
        hasher.finish()
    }
}

/// Render `value` as `len` digits of `alphabet`.
fn render(mut value: u64, len: usize, alphabet: &[u8]) -> String {
    let base = alphabet.len() as u64;
    let mut out = String::with_capacity(len);
    for _ in 0..len {
        out.push(char::from(alphabet[(value % base) as usize]));
        value /= base;
    }
    out
}

/// Wall-clock time in epoch milliseconds.
fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

/// The current time in epoch milliseconds, never earlier than (or equal to) the
/// last value `last_timestamp_millis` recorded, which it advances.
///
/// A free function over the one field it moves, so a handler holding a mutable
/// borrow of another field of the store (the record it is updating) can still
/// issue a timestamp.
pub fn issue_timestamp_millis(last_timestamp_millis: &mut u64) -> u64 {
    let issued = unix_millis().max(last_timestamp_millis.saturating_add(1));
    *last_timestamp_millis = issued;
    issued
}

/// Epoch milliseconds as the modelled timestamp type.
pub fn millis_to_date_time(millis: u64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_secs_f64(millis as f64 / 1_000.0)
}

/// The `desktopType` an environment gets from its `desktopArn`: the ARN's
/// service segment, which the model's `DesktopType` enum constrains to a
/// WorkSpaces, WorkSpaces Secure Browser or AppStream 2.0 desktop.
///
/// `None` when the ARN names some other service — there is no desktop type for
/// it, so the request is refused.
pub fn desktop_type_from_desktop_arn(desktop_arn: &str) -> Option<&'static str> {
    let service = desktop_arn.split(':').nth(2)?;
    DESKTOP_TYPE_VALUES
        .iter()
        .copied()
        .find(|candidate| *candidate == service)
}

/// The ARN region for a resource this request mints: the region of the caller's
/// sigv4 credential scope, or [`DEFAULT_REGION`] when the request carries none.
///
/// The scope is parsed by `emulator_server::sigv4_credential_scope`, the single
/// source of SigV4-scope parsing, so a header that is not a well-formed sigv4
/// `Authorization` yields the default rather than a bogus region.
pub fn region_from_request(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The pagination token that resumes after the environment with `seq`.
pub fn encode_page_token(seq: u64) -> String {
    format!("{PAGE_TOKEN_PREFIX}{seq:016x}")
}

/// The sequence number a pagination token resumes after, or `None` when the
/// token was not minted by this service.
pub fn decode_page_token(token: &str) -> Option<u64> {
    let digits = token.strip_prefix(PAGE_TOKEN_PREFIX)?;
    u64::from_str_radix(digits, 16).ok()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsWorkspacesthinclientThinclientState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsWorkspacesthinclientThinclientState>)
}
