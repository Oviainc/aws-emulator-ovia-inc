// SPDX-License-Identifier: CC0-1.0

//! What the operations that work on an add-on share besides the store itself:
//! where in a request the add-ons came from, and the one sentence a request
//! naming no add-on-carrying resource is refused with.
//!
//! An ADD-ON is extra behaviour a Lightsail resource carries — the automatic
//! snapshot and the idle-stop add-on the modelled `AddOnType` enum names. The
//! records they live on, the resolution of a name to one of those records
//! ([`super::ComAmazonawsLightsailLightsail20161128State::with_add_on_record`])
//! and the conversion from a request to a stored add-on
//! ([`super::add_ons_from_requests`]) all live in [`super`], where every other
//! operation reaches them; this module adds only what those two need from the
//! CALLER: which member of which request is being converted, and how a name
//! that resolves to nothing is refused.

/// Where in a request the add-ons being converted came from, so a refusal names
/// the member the CALLER sent rather than the member of whichever operation the
/// conversion happens to be shared with.
///
/// The operations that create a resource carry a list of add-on requests under
/// `addOns`; the operation that enables one afterwards carries a single request
/// under `addOnRequest`. Both go through the same conversion
/// ([`super::add_ons_from_requests`]), so the member path is a parameter of it
/// rather than a sentence one caller rewrites after the fact:
/// [`ADD_ONS_LIST_MEMBER`] and [`ADD_ON_REQUEST_MEMBER`] are the two the model
/// gives.
#[derive(Debug, Clone, Copy)]
pub struct AddOnMember {
    /// The path of the member carrying the add-on requests, for a refusal about
    /// how many of them there are.
    pub carrier: &'static str,
    /// The path of ONE add-on request within it, for a refusal about what that
    /// request says.
    pub entry: &'static str,
}

/// The `addOns` list a create call carries, whose entries are addressed as
/// elements of it.
pub const ADD_ONS_LIST_MEMBER: AddOnMember = AddOnMember {
    carrier: "addOns",
    entry: "addOns[]",
};

/// The single `addOnRequest` of `EnableAddOn`, which IS the one entry.
pub const ADD_ON_REQUEST_MEMBER: AddOnMember = AddOnMember {
    carrier: "addOnRequest",
    entry: "addOnRequest",
};

/// The message an add-on operation's `DoesNotExist` rejection carries when
/// `name` names no add-on-carrying resource of this account.
///
/// Built from the request, and shared by every operation that resolves a
/// resource through
/// [`super::ComAmazonawsLightsailLightsail20161128State::with_add_on_record`],
/// so the one resolution rule is refused with one sentence. It names the
/// request's own member rather than a resource type, because such a request
/// names a resource without saying what kind it is.
pub fn add_on_resource_not_found(name: &str) -> String {
    format!("resourceName '{name}' does not name a resource of this account that carries add-ons")
}
