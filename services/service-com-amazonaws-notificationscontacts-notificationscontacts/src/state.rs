// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-notificationscontacts-notificationscontacts service.
//!
//! The service's only resource is the **email contact**. Every contact is stored
//! exactly once, keyed by its own ARN — the value `CreateEmailContact` hands back and
//! every other operation takes as its `@httpLabel` — so each operation reads and
//! writes the one record. The map is ordered, which is what lets
//! `ListEmailContacts` walk a stable total order and name the next page's first ARN
//! in its opaque `nextToken`.
//!
//! The tags a create stores live on the record, so the
//! `TagResource`/`UntagResource`/`ListTagsForResource` slice reads and writes the
//! same map rather than a second copy.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    ConflictException, EmailContact, ResourceNotFoundException, ValidationException,
    ValidationExceptionField,
};

/// Characters an email-contact id is built from.
///
/// Taken from the modelled `EmailContactArn` pattern, whose last segment is
/// `[a-z0-9]{27}`: `smithy_query(query="shape",shape="EmailContactArn")`.
const CONTACT_ID_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// Length of the id segment of an email-contact ARN, from the same pattern.
pub const CONTACT_ID_LEN: usize = 27;

/// The resource portion an email-contact ARN carries before its id, from the same
/// modelled pattern.
pub const CONTACT_ARN_RESOURCE_PREFIX: &str = "emailcontact/";

/// The resource type this service's error shapes report in `resourceType`: the
/// resource word of the modelled `EmailContactArn` pattern.
pub const CONTACT_RESOURCE_TYPE: &str = "emailcontact";

/// The ARN service namespace for this service (`arnNamespace`/sigv4 name
/// `notifications-contacts`).
pub const ARN_SERVICE: &str = "notifications-contacts";

/// Length of an activation code, from the modelled `Token` shape
/// (`len 7..7`, `^[a-z0-9]{7}$`):
/// `smithy_query(query="operation",operation="ActivateEmailContact")`.
pub const ACTIVATION_CODE_LEN: usize = 7;

/// `EmailContactStatus`' value for a contact that has not been activated yet.
///
/// One of the generated [`crate::ops::types::EMAIL_CONTACT_STATUS_VALUES`], which
/// carries exactly the values
/// `smithy_query(query="shape",shape="EmailContactStatus")` reports (`inactive`,
/// `active`).
pub const STATUS_INACTIVE: &str = "inactive";

/// `EmailContactStatus`' value for an activated contact, from the same catalog.
pub const STATUS_ACTIVE: &str = "active";

/// `ValidationExceptionReason`'s value for a request a named field made invalid,
/// from `smithy_query(query="shape",shape="ValidationExceptionReason")`.
pub const VALIDATION_REASON_FIELD: &str = "fieldValidationFailed";

/// The most tags one email contact may carry at a time.
///
/// The modelled `TagMap` is `len 0..200` (`smithy_query(query="shape",shape="TagMap")`)
/// and `ListTagsForResourceResponse$tags` reports the stored set through that very
/// shape, so a set grown past the bound could not be reported: `TagResource` refuses a
/// merge that would exceed it rather than persisting one.
pub const MAX_TAGS_PER_CONTACT: usize = 200;

/// The most email contacts this emulator will hold.
///
/// Emulator resource guard, NOT a published AWS quota: every record is created by a
/// caller and reclaimed only by an explicit `DeleteEmailContact`, so the one growth
/// site carries a cap and the store stays finite. A create that would exceed it is
/// refused with the modelled `ServiceQuotaExceededException` (HTTP 402).
pub const MAX_EMAIL_CONTACTS: usize = 500;

/// The quota code this service reports when [`MAX_EMAIL_CONTACTS`] is reached.
pub const EMAIL_CONTACTS_QUOTA_CODE: &str = "L-EMAILCONTACTS";

/// One stored email contact: every member `EmailContact` reports, plus the tags the
/// tagging operations will key off its ARN and the activation code
/// `SendActivationCode` issued for it.
#[derive(Debug, Clone, Default)]
pub struct EmailContactRecord {
    /// The contact's ARN: this record's store key and the label every other
    /// operation addresses it by.
    pub arn: String,
    pub name: String,
    /// The email address the create supplied as `emailAddress`; the wire shape
    /// reports it as `address`.
    pub address: String,
    /// One of [`EMAIL_CONTACT_STATUS_VALUES`].
    pub status: String,
    /// Creation time, in milliseconds since the Unix epoch.
    pub creation_time_millis: i64,
    /// Last-update time, in milliseconds since the Unix epoch.
    pub update_time_millis: i64,
    /// Tags attached to this contact, keyed by tag key. Bounded by
    /// [`MAX_TAGS_PER_CONTACT`].
    pub tags: BTreeMap<String, String>,
    /// The activation code the last `SendActivationCode` stored, present only once
    /// one was issued. Kept after a successful activation so re-activating with the
    /// same code is idempotent.
    pub pending_activation_code: Option<String>,
}

impl EmailContactRecord {
    /// This record as the modelled `EmailContact` wire shape.
    pub fn to_wire(&self) -> EmailContact {
        EmailContact {
            arn: self.arn.clone(),
            name: self.name.clone(),
            address: self.address.clone(),
            status: self.status.clone(),
            creation_time: wire_timestamp(self.creation_time_millis),
            update_time: wire_timestamp(self.update_time_millis),
        }
    }

    /// Whether this contact has been activated.
    pub fn is_active(&self) -> bool {
        self.status == STATUS_ACTIVE
    }
}

/// Per-service state for com-amazonaws-notificationscontacts-notificationscontacts.
/// A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsNotificationscontactsNotificationscontactsState {
    /// Every email contact, keyed by its ARN. A `BTreeMap` so `ListEmailContacts`
    /// walks a stable, total order and a page token can name the next ARN.
    pub contacts: BTreeMap<String, EmailContactRecord>,
    /// Mixed into each generated id and activation code so two calls in the same
    /// nanosecond differ.
    id_counter: u64,
    /// The last instant this store handed out, so a change recorded after an
    /// earlier one always carries a strictly later timestamp — the real service's
    /// finer clock gives the same guarantee, and the millisecond resolution of
    /// `creationTime`/`updateTime` on the wire would otherwise collapse two changes
    /// made inside one millisecond into the same value.
    last_instant_millis: i64,
}

impl ComAmazonawsNotificationscontactsNotificationscontactsState {
    /// The contact `arn` names, for a reporting operation.
    pub fn contact(&self, arn: &str) -> Option<&EmailContactRecord> {
        self.contacts.get(arn)
    }

    /// The contact `arn` names, for an operation the spec says mutates it.
    ///
    /// `arn` is this record's identity — it is the store key — so it must not be
    /// reassigned through this handle; no operation needs to.
    pub fn contact_mut(&mut self, arn: &str) -> Option<&mut EmailContactRecord> {
        self.contacts.get_mut(arn)
    }

    /// Store a newly created contact.
    pub fn insert_contact(&mut self, contact: EmailContactRecord) {
        self.contacts.insert(contact.arn.clone(), contact);
    }

    /// Remove the contact `arn` names, with its tags and activation code. `None`
    /// when nothing matched.
    pub fn remove_contact(&mut self, arn: &str) -> Option<EmailContactRecord> {
        self.contacts.remove(arn)
    }

    /// A 27-character lowercase-alphanumeric id no stored contact is using.
    pub fn fresh_contact_id(&mut self) -> String {
        for _ in 0..1024 {
            let candidate = self.draw(CONTACT_ID_LEN);
            if !self.contacts.contains_key(&contact_arn(&candidate)) {
                return candidate;
            }
        }
        // Bounded fallback, so a pathological clock can never spin here: the
        // zero-padded decimals are also `[a-z0-9]{27}`, and there are always more of
        // them than there are stored contacts.
        (0u64..=self.contacts.len() as u64)
            .map(|n| format!("{n:027}"))
            .find(|id| !self.contacts.contains_key(&contact_arn(id)))
            .unwrap_or_else(|| format!("{:027}", self.contacts.len()))
    }

    /// A fresh `^[a-z0-9]{7}$` activation code.
    pub fn fresh_activation_code(&mut self) -> String {
        self.draw(ACTIVATION_CODE_LEN)
    }

    /// Now, in milliseconds since the Unix epoch, and strictly later than every
    /// instant this store has already handed out.
    pub fn next_instant_millis(&mut self) -> i64 {
        self.last_instant_millis = now_epoch_millis().max(self.last_instant_millis + 1);
        self.last_instant_millis
    }

    /// `len` characters drawn from [`CONTACT_ID_ALPHABET`].
    fn draw(&mut self, len: usize) -> String {
        self.id_counter = self.id_counter.wrapping_add(1);
        let mut state = now_nanos() ^ self.id_counter.rotate_left(17);
        (0..len)
            .map(|_| {
                let roll = splitmix64(&mut state);
                CONTACT_ID_ALPHABET[(roll % CONTACT_ID_ALPHABET.len() as u64) as usize] as char
            })
            .collect()
    }
}

/// The ARN of the email contact `id`.
///
/// The modelled `EmailContactArn` carries an EMPTY region field
/// (`arn:aws:notifications-contacts::[0-9]{12}:emailcontact/...`), the way a global
/// resource's ARN does, so no region is interpolated.
pub fn contact_arn(id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        "",
        &format!("{CONTACT_ARN_RESOURCE_PREFIX}{id}"),
    )
}

/// The id segment of an email-contact ARN, or `None` when `candidate` is not an ARN
/// of this service's email-contact resource.
///
/// Every field must match what this emulator mints, so an ARN naming another
/// partition, service or account is not one of ours even when its id segment is
/// well formed.
pub fn contact_id_from_arn(candidate: &str) -> Option<&str> {
    let id = candidate
        .strip_prefix(&contact_arn(""))
        .filter(|id| id.len() == CONTACT_ID_LEN)?;
    id.bytes()
        .all(|b| CONTACT_ID_ALPHABET.contains(&b))
        .then_some(id)
}

/// The modelled 404 for an email contact this store does not hold.
pub fn contact_not_found(arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("no email contact exists with the arn {arn}"),
        resource_id: arn.to_string(),
        resource_type: CONTACT_RESOURCE_TYPE.to_string(),
    }
}

/// The modelled 409 for a contact whose current state refuses `why`.
pub fn contact_conflict(arn: &str, why: String) -> ConflictException {
    ConflictException {
        message: why,
        resource_id: arn.to_string(),
        resource_type: CONTACT_RESOURCE_TYPE.to_string(),
    }
}

/// The modelled 400 blaming one named input member.
pub fn field_validation_failure(member: &str, why: &str) -> ValidationException {
    ValidationException {
        message: format!("{member} {why}"),
        reason: VALIDATION_REASON_FIELD.to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message: why.to_string(),
        }]),
    }
}

/// A stored instant as the `date-time` timestamp `creationTime`/`updateTime` carry.
pub fn wire_timestamp(millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// Now, in whole milliseconds since the Unix epoch.
fn now_epoch_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Now, in nanoseconds since the Unix epoch — only an id-generator seed.
fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64)
}

/// SplitMix64: the generator's mixing step, so consecutive seeds do not produce
/// near-identical ids.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsNotificationscontactsNotificationscontactsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsNotificationscontactsNotificationscontactsState>)
}
