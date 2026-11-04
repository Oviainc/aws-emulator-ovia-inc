// SPDX-License-Identifier: CC0-1.0

//! The suppression-list plane of this service's state: one suppressed address, how the
//! entries of one list are keyed, and the wording a lookup that finds none refuses
//! with.
//!
//! Split out of the state module root so one resource plane's records live together;
//! every item is re-exported from [`crate::state`], so a handler still reaches it as
//! `crate::state::…`.

use std::time::SystemTime;

use super::identity_key;

/// One address on a suppression list.
///
/// SES keeps a suppression list per account and, when a tenant configures one, per
/// tenant; an entry carries the reason the address was suppressed, when the entry
/// was last updated, and optionally what caused it.
#[derive(Debug, Clone)]
pub struct SuppressedDestinationRecord {
    /// `SuppressedDestination$EmailAddress`, as the request that added the entry
    /// spelled it: the local part belongs to the receiving mailbox, so a response
    /// reports it unchanged.
    pub email_address: String,
    /// `SuppressedDestination$Reason`: the `SuppressionListReason` the address was
    /// suppressed for.
    pub reason: String,
    /// `SuppressedDestination$LastUpdateTime`: when the entry was last written.
    pub last_update_time: SystemTime,
    /// `SuppressedDestinationAttributes$MessageId`: the message that caused the
    /// suppression, when one did.
    pub message_id: Option<String>,
    /// `SuppressedDestinationAttributes$FeedbackId`: the identifier generated when
    /// the address was added, when the entry carries one.
    pub feedback_id: Option<String>,
    /// The tenant whose suppression list holds this entry; `None` for the
    /// account-level list. `SuppressedDestination$TenantName` "is present only when
    /// the suppressed destination is on a tenant's suppression list".
    pub tenant_name: Option<String>,
}

impl SuppressedDestinationRecord {
    /// The modelled `SuppressedDestination` view of this record.
    ///
    /// One projection shared by every operation that reports a suppressed address,
    /// so a write and the read after it cannot disagree.
    pub fn detail(&self) -> crate::ops::types::SuppressedDestination {
        crate::ops::types::SuppressedDestination {
            email_address: self.email_address.clone(),
            reason: self.reason.clone(),
            last_update_time: ::aws_smithy_types::DateTime::from(self.last_update_time),
            // "An optional value that can contain additional information about the
            // reasons that the address was added": an entry that knows neither the
            // message nor the feedback id reports no attributes at all, rather than
            // an object whose members are all absent.
            attributes: (self.message_id.is_some() || self.feedback_id.is_some()).then(|| {
                crate::ops::types::SuppressedDestinationAttributes {
                    message_id: self.message_id.clone(),
                    feedback_id: self.feedback_id.clone(),
                }
            }),
            tenant_name: self.tenant_name.clone(),
        }
    }

    /// The modelled `SuppressedDestinationSummary` view of this record.
    ///
    /// The summary is the listing's projection of the same stored entry
    /// [`Self::detail`] reports, so a listed address and the read after it cannot
    /// disagree. `SuppressedDestinationSummary` models only the address, the reason
    /// and the last-update instant — the list the entry belongs to is the one the
    /// request selected, so it is not repeated per summary.
    pub fn summary(&self) -> crate::ops::types::SuppressedDestinationSummary {
        crate::ops::types::SuppressedDestinationSummary {
            email_address: self.email_address.clone(),
            reason: self.reason.clone(),
            last_update_time: ::aws_smithy_types::DateTime::from(self.last_update_time),
        }
    }

    /// Whether this entry's `LastUpdateTime` lies in the window `start`..`end`, each
    /// bound given as epoch seconds and each absent bound leaving that side open.
    ///
    /// The bounds come from `ListSuppressedDestinationsRequest$StartDate` ("only
    /// includes addresses that were added to the list before a specific date" for
    /// `$EndDate`, "after a specific date" for `$StartDate`). Neither the model nor
    /// that wording settles an entry stamped exactly ON a bound, so the reading is
    /// CHOSEN: the window is CLOSED, since a bound names an instant the caller asked
    /// about and an entry stamped there is one the caller asked for.
    pub fn updated_within(&self, start: Option<f64>, end: Option<f64>) -> bool {
        let at = ::aws_smithy_types::DateTime::from(self.last_update_time).as_secs_f64();
        start.is_none_or(|from| at >= from) && end.is_none_or(|to| at <= to)
    }
}

/// The key a suppression-list entry is stored under: the list it belongs to — a
/// tenant's, or the account's when `tenant_name` is `None` — and the address within
/// it.
///
/// The address part is its [`identity_key`], so a lookup matches the domain without
/// regard to case while keeping the local part exactly as supplied.
///
/// The list part is LENGTH-PREFIXED: the number of bytes in the list's name, the
/// separator, then the name itself. Where the split between the two parts falls is
/// therefore decided by a count this function writes, not by a character the name is
/// assumed not to contain, so the key is one-to-one with the (list, address) pair
/// whatever either part holds — no name can collide with another list's keys or forge
/// another list's prefix.
pub fn suppression_key(tenant_name: Option<&str>, email_address: &str) -> String {
    format!(
        "{}{}",
        suppression_list_prefix(tenant_name),
        identity_key(email_address)
    )
}

/// The key prefix every entry of ONE suppression list shares: the length and the name
/// of the tenant whose list it is (the empty name for the account-level list), joined
/// by the separator from [`suppression_key`].
///
/// A key carries this prefix only if it belongs to this very list — a different name
/// either differs in the length that opens the prefix or, at equal length, in the name
/// itself — which is what lets a listing tell its own page token from one minted for a
/// different list.
///
/// Every entry of one list shares the prefix, so within a list the keys are ordered by
/// address, which is the order a page walks. Across lists they group by name length
/// rather than alphabetically; no listing depends on that, since each enumerates the
/// one list its request selects.
pub fn suppression_list_prefix(tenant_name: Option<&str>) -> String {
    let name = tenant_name.unwrap_or_default();
    format!("{}\u{1f}{name}", name.len())
}

/// How this service words `NotFoundException` for an address that is not on the
/// suppression list being read.
///
/// One wording for every operation that looks an address up on a suppression list,
/// so a client sees the same sentence whichever call first tells it the address is
/// not suppressed.
pub fn suppressed_destination_not_found(email_address: &str) -> String {
    format!("Email address {email_address} does not exist on your suppression list.")
}
