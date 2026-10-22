// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-iotmanagedintegrations-iotmanagedintegrations service.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ops::types::{CapabilityReport, CapabilitySchemaItem, WiFiSimpleSetupConfiguration};

/// The region an ARN carries when the request names none (the emulator's default
/// region, as documented on [`engine::emulator_arn`]).
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's `aws.auth#sigv4` name, which is also the ARN service segment the
/// `ManagedThingArn` pattern (`^arn:aws:iotmanagedintegrations:…`) requires.
pub const SERVICE_ARN_NAMESPACE: &str = "iotmanagedintegrations";

/// The `Role`, `AuthMaterialType` and `HubNetworkMode` value lists the handlers
/// validate against, re-exported from the MODEL-GENERATED constants in
/// `crate::ops::types` so there is no second copy to drift from the model.
pub use crate::ops::types::{AUTH_MATERIAL_TYPE_VALUES, HUB_NETWORK_MODE_VALUES, ROLE_VALUES};

/// The `ProvisioningStatus` a freshly created managed thing carries: it is not yet
/// associated with a physical device. Leaving this state needs the device to be
/// onboarded (a discovery or provisioning flow), not the passage of time, so no
/// transition out of it is owed by the clock. The full value list is
/// `crate::ops::types::PROVISIONING_STATUS_VALUES`.
pub const INITIAL_PROVISIONING_STATUS: &str = "UNASSOCIATED";

/// One stored managed thing: the authoritative record every managed-thing operation
/// reads and writes. `CreateManagedThing` inserts it, `GetManagedThing` /
/// `ListManagedThings` project it onto their output shapes, `UpdateManagedThing`
/// mutates it in place and `DeleteManagedThing` removes it.
#[derive(Debug, Clone)]
pub struct ManagedThing {
    /// The generated `ManagedThingId` this record is keyed by.
    pub id: String,
    pub arn: String,
    /// The region the creating request was signed for. A managed thing is a regional
    /// resource: only that region's endpoint resolves, lists, updates or deletes it.
    pub region: String,
    /// `Role` enum value (`CONTROLLER` | `DEVICE`).
    pub role: String,
    /// `ProvisioningStatus` enum value.
    pub provisioning_status: String,
    /// `AuthenticationMaterial` — `@sensitive`, so it is stored but never returned by
    /// any modelled output shape.
    pub authentication_material: String,
    /// `AuthMaterialType` enum value.
    pub authentication_material_type: String,
    pub owner: Option<String>,
    pub credential_locker_id: Option<String>,
    pub serial_number: Option<String>,
    pub brand: Option<String>,
    pub model: Option<String>,
    pub name: Option<String>,
    pub classification: Option<String>,
    pub capabilities: Option<String>,
    pub capability_report: Option<CapabilityReport>,
    pub capability_schemas: Vec<CapabilitySchemaItem>,
    /// `HubNetworkMode` enum value, set only by `UpdateManagedThing`.
    pub hub_network_mode: Option<String>,
    pub wifi_simple_setup_configuration: Option<WiFiSimpleSetupConfiguration>,
    pub meta_data: HashMap<String, String>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// When the thing was activated against a physical device. `None` until an
    /// onboarding flow activates it; no operation in this slice does.
    pub activated_at: Option<SystemTime>,
    /// Monotonic insertion sequence. `ListManagedThings` pages by it, so a page token
    /// keeps resuming at the right place even when records are deleted in between.
    pub sequence: u64,
}

/// Per-service state for com-amazonaws-iotmanagedintegrations-iotmanagedintegrations. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState {
    /// The authoritative managed-thing store, keyed by the generated managed-thing id.
    pub managed_things: HashMap<String, ManagedThing>,
    /// Insertion order of managed-thing ids, so `ListManagedThings` pages
    /// deterministically. An id is appended on create and removed on delete.
    pub managed_thing_order: Vec<String>,
    /// `ClientToken` (`@idempotencyToken`) -> what the first call created, so a
    /// replayed `CreateManagedThing` returns the original record instead of a duplicate.
    /// Bounded by [`IDEMPOTENCY_TTL`] and [`MAX_IDEMPOTENCY_TOKENS`].
    pub idempotency_tokens: HashMap<String, IdempotencyRecord>,
    /// Source of [`ManagedThing::sequence`] values and of the per-record suffix that
    /// makes a generated id unique.
    pub next_sequence: u64,
}

/// The most `ManagedThingSummary` items one `ListManagedThings` page carries, and the
/// page size an absent `MaxResults` gets. A caller reaches the rest of the store
/// through the `NextToken`, so one request's response stays bounded however many
/// records are stored.
pub const MANAGED_THING_PAGE_LIMIT: usize = 100;

/// The most managed things one store holds. A create past it is refused rather than
/// grown into, so a caller cannot make this emulator's memory grow without end. This
/// is a bound CHOSEN for the emulator, not a published AWS quota.
pub const MAX_MANAGED_THINGS: usize = 10_000;

/// How long a `ClientToken` keeps replaying its original answer. A CHOSEN bound (AWS
/// does not publish this window), so the idempotency map cannot keep a record for a
/// caller-supplied token for the life of the process.
pub const IDEMPOTENCY_TTL: ::std::time::Duration = ::std::time::Duration::from_secs(60 * 60);

/// The most idempotency records one store keeps. A token minted past it evicts the
/// oldest, so the map is bounded even when every token is still inside its TTL.
pub const MAX_IDEMPOTENCY_TOKENS: usize = 10_000;

/// What a `ClientToken` replays into: the managed thing the first call created, and
/// when the token was minted (so [`IDEMPOTENCY_TTL`] can retire it).
#[derive(Debug, Clone)]
pub struct IdempotencyRecord {
    pub managed_thing_id: String,
    pub minted_at: SystemTime,
}

impl ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState {
    /// The record `region`'s endpoint holds under `id`, if any. A record created in
    /// another region is not visible here.
    pub fn managed_thing_in(&self, region: &str, id: &str) -> Option<&ManagedThing> {
        self.managed_things
            .get(id)
            .filter(|thing| thing.region == region)
    }

    /// [`Self::managed_thing_in`], mutably.
    pub fn managed_thing_in_mut(&mut self, region: &str, id: &str) -> Option<&mut ManagedThing> {
        self.managed_things
            .get_mut(id)
            .filter(|thing| thing.region == region)
    }

    /// Take the next insertion sequence number.
    pub fn take_sequence(&mut self) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        sequence
    }

    /// Retire the idempotency records the clock already made due — the ones past
    /// [`IDEMPOTENCY_TTL`] at `now`, which were owed whether or not anyone called — so
    /// an expired token neither replays nor keeps occupying the map.
    pub fn retire_expired_idempotency_tokens(&mut self, now: SystemTime) {
        self.idempotency_tokens
            .retain(|_, record| match now.duration_since(record.minted_at) {
                Ok(age) => age < IDEMPOTENCY_TTL,
                // A record minted "after" now (a clock that went backwards) is not old.
                Err(_) => true,
            });
    }

    /// Evict the oldest idempotency records while the map is at
    /// [`MAX_IDEMPOTENCY_TOKENS`], so minting one more cannot push it past the cap.
    pub fn make_room_for_idempotency_token(&mut self) {
        while self.idempotency_tokens.len() >= MAX_IDEMPOTENCY_TOKENS {
            let oldest = self
                .idempotency_tokens
                .iter()
                .min_by_key(|(_, record)| record.minted_at)
                .map(|(token, _)| token.clone());
            match oldest {
                Some(token) => {
                    self.idempotency_tokens.remove(&token);
                }
                None => break,
            }
        }
    }
}

/// A fresh opaque identifier: lowercase hex of the wall clock in nanoseconds plus a
/// per-store counter, so it is unique within a run and matches both the
/// `ManagedThingId` pattern (`^[a-zA-Z0-9:_-]*$`, len 1..64) and the `NextToken`
/// pattern (`^[a-zA-Z0-9=_-]+$`).
pub fn opaque_id(counter: u64) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    format!("{:016x}{counter:08x}", nanos as u64)
}

/// A `NextToken` carrying the [`ManagedThing::sequence`] a page resumes at, encoded so
/// it is opaque to the caller and matches the modelled `NextToken` pattern
/// (`^[a-zA-Z0-9=_-]+$`). Encoding the cursor keeps `ListManagedThings` a pure read:
/// paging stores nothing, so a list cannot change what a later caller sees.
pub fn encode_page_token(resume_at: u64) -> String {
    format!("{resume_at:016x}")
}

/// The sequence a `NextToken` resumes at, or `None` when this service cannot have
/// issued the token: `minted` is the store's next sequence, so a cursor beyond it was
/// never handed out.
pub fn decode_page_token(token: &str, minted: u64) -> Option<u64> {
    (token.len() == 16 && token.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| u64::from_str_radix(token, 16).ok())
        .flatten()
        .filter(|resume_at| *resume_at <= minted)
}

/// The modelled timestamp for a stored instant.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    match at.duration_since(UNIX_EPOCH) {
        Ok(since) => ::aws_smithy_types::DateTime::from_secs_f64(since.as_secs_f64()),
        Err(_) => ::aws_smithy_types::DateTime::from_secs(0),
    }
}

/// The region this request was signed for, falling back to [`DEFAULT_REGION`] when it
/// carries no SigV4 credential scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of a managed thing in `region`, as the `ManagedThingArn` pattern requires.
pub fn managed_thing_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(SERVICE_ARN_NAMESPACE, region, &format!("managed-thing/{id}"))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsIotmanagedintegrationsIotmanagedintegrationsState>)
}
