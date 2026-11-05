// SPDX-License-Identifier: CC0-1.0

//! The stored static IP: the public address the account reserved under a name,
//! and the instance it is attached to while one is.

use super::*;

/// One stored static IP: the public IPv4 address `AllocateStaticIp` reserved for
/// the account under a name, and the instance it is attached to while one is.
///
/// The address itself is drawn from the store's own allocator, so it is unique
/// within this process and stable for the life of the allocation — a static IP is
/// static precisely because every later read reports the same address.
#[derive(Debug, Clone)]
pub struct StoredStaticIp {
    /// The static IP name the request named, kept verbatim — it is this record's
    /// store key and the `name` every read reports.
    pub name: String,
    pub arn: String,
    pub support_code: String,
    pub created_at_ms: i64,
    pub location: StoredLocation,
    /// The reserved address, in the dotted-quad form the modelled `IpAddress`
    /// pattern requires.
    pub ip_address: String,
    /// The instance this static IP is attached to, while one is. `AttachStaticIp`
    /// is what sets it; a newly allocated static IP is attached to nothing.
    pub attached_to: Option<String>,
}

impl StoredStaticIp {
    /// This record as the modelled `StaticIp`.
    pub fn view(&self) -> StaticIp {
        StaticIp {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            support_code: Some(self.support_code.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_STATIC_IP.to_string()),
            ip_address: Some(self.ip_address.clone()),
            attached_to: self.attached_to.clone(),
            // `isAttached` is exactly "whether the static IP is attached", so it
            // is derived from the one stored attachment rather than tracked
            // beside it, where the two could disagree.
            is_attached: Some(self.attached_to.is_some()),
        }
    }
}
