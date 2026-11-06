// SPDX-License-Identifier: CC0-1.0

//! The resource-policy slice of this service's state.
//!
//! A web function carries at most one resource-based policy, so the stored
//! [`ResourcePolicy`] lives inside that function's record
//! (`WebFunction::resource_policy`): `PutResourcePolicy`, `GetResourcePolicy` and
//! `DeleteResourcePolicy` read and write ONE copy of it, and a function's delete
//! takes its policy with it. The policy's `revisionId` is minted here, by
//! [`ComAmazonawsLambdawebLambdawebState::put_resource_policy`], so the id a caller
//! reads back is the one the store holds, and the revision rule both mutating
//! operations apply is [`ResourcePolicy::accepts_revision`] rather than two
//! copies.

use std::time::SystemTime;

use super::{hex_uuid_shape, ComAmazonawsLambdawebLambdawebState};

/// The high half of every policy `revisionId` this service mints, which keeps a
/// policy generation's id distinct from a function revision's id (minted with `0`)
/// however many of either exist.
const POLICY_REVISION_ID_PREFIX: u64 = 1;

/// The `resourceType` the resource-policy refusals report: the sub-resource the
/// request names, not the function it hangs off.
pub const RESOURCE_POLICY_RESOURCE_TYPE: &str = "ResourcePolicy";

/// One stored resource-based policy of a web function.
#[derive(Debug, Clone)]
pub struct ResourcePolicy {
    /// The policy document as the caller sent it, stored VERBATIM: the service
    /// attaches the document rather than rewriting it, so a later read reports the
    /// same text rather than a re-rendered one.
    pub document: String,
    /// The id of THIS generation of the policy. Minted afresh by every successful
    /// put, which is what makes a request carrying an older id a conflict and what
    /// keeps the id a reader reports stable until the next mutation.
    pub revision_id: String,
    /// When this generation was written.
    pub updated_at: SystemTime,
}

/// The `message` the modelled `ValidationException` reports for a `policy` that is
/// not a policy document this service can read.
///
/// ONE fixed sentence, carrying nothing from the request: the document the caller
/// sent is the thing at fault and echoing it back says nothing a caller does not
/// already have.
pub const POLICY_SYNTAX_ERROR_MESSAGE: &str = "Policy has syntax errors.";

/// The `message` both mutating resource-policy operations report when the
/// `revisionId` they were given is not the stored policy's.
///
/// ONE sentence for ONE conflict class — the policy moved on under the caller,
/// which is the conflict the model describes ("the resource is being modified by
/// another request ... or the policy revision ID that you specified doesn't match
/// the current policy", source: `smithy_query(query="shape",
/// shape="ConflictException")`) — and it carries no revision id: the id the request
/// sent is the caller's own, and the current one is what `GetResourcePolicy`
/// reports.
pub const STALE_POLICY_REVISION_MESSAGE: &str =
    "The policy was updated by another request. Try again.";

/// Whether `policy` is a document the resource-policy put can accept.
///
/// `PutResourcePolicyRequest$policy` is "The JSON-formatted resource-based policy
/// to attach to the web function." (source: `smithy_query(query="doc",
/// shape="PutResourcePolicyRequest", member="policy")`), so the document must be
/// JSON, and a resource-based policy document's top-level element is a single JSON
/// object. The model bounds the member's length (1..20480) only and carries no
/// pattern, so the boundary validator cannot check either: this is where the put
/// does, before anything is written.
pub fn is_policy_document(policy: &str) -> bool {
    ::serde_json::from_str::<::serde_json::Value>(policy)
        .is_ok_and(|document| document.is_object())
}

/// The `message` the modelled `ResourceNotFoundException` reports for a web
/// function that carries no resource-based policy.
///
/// ONE fixed sentence naming the missing sub-resource and nothing from the request:
/// the function the request addressed is already named by its own URI label.
pub const POLICY_NOT_FOUND_MESSAGE: &str = "The function does not have a policy.";

impl ResourcePolicy {
    /// Whether a request carrying `supplied` as its `revisionId` may act on this
    /// generation of the policy.
    ///
    /// A request that names no revision id proceeds — "If you don't specify a
    /// value, the update proceeds without checking the current revision." (source:
    /// `smithy_query(query="doc", shape="PutResourcePolicyRequest",
    /// member="revisionId")`) — and one that names an id may act only on the
    /// generation that id identifies.
    pub fn accepts_revision(&self, supplied: Option<&str>) -> bool {
        supplied.is_none_or(|supplied| supplied == self.revision_id)
    }
}

impl ComAmazonawsLambdawebLambdawebState {
    /// Mint a fresh policy `revisionId`: the counter rendered in the 8-4-4-4-12
    /// shape the model's `PolicyRevisionId` pattern (`^[0-9a-f-]{36}$`) requires,
    /// under [`POLICY_REVISION_ID_PREFIX`] so no two generations — of one policy or
    /// of two — ever share an id.
    fn mint_policy_revision_id(&mut self) -> String {
        self.next_policy_revision_seq = self.next_policy_revision_seq.wrapping_add(1);
        hex_uuid_shape(POLICY_REVISION_ID_PREFIX, self.next_policy_revision_seq)
    }

    /// Attach `document` to `function_name` as a NEW generation of its
    /// resource-based policy, returning the generation stored, or `None` when no
    /// such function is stored.
    ///
    /// Every successful put is a mutation of the policy, so it mints a fresh
    /// `revisionId` whatever the document says: that is the id the next request has
    /// to carry, and the id a reader reports until this is superseded.
    pub fn put_resource_policy(
        &mut self,
        function_name: &str,
        document: &str,
        now: SystemTime,
    ) -> Option<ResourcePolicy> {
        if !self.functions.contains_key(function_name) {
            return None;
        }
        let revision_id = self.mint_policy_revision_id();
        let function = self.functions.get_mut(function_name)?;
        let stored = ResourcePolicy {
            document: document.to_string(),
            revision_id,
            updated_at: now,
        };
        function.resource_policy = Some(stored.clone());
        Some(stored)
    }

    /// Remove `function_name`'s resource-based policy, returning the generation
    /// removed, or `None` when the function is not stored or carries no policy.
    pub fn remove_resource_policy(&mut self, function_name: &str) -> Option<ResourcePolicy> {
        self.functions
            .get_mut(function_name)?
            .resource_policy
            .take()
    }

    /// The resource-based policy stored for `function_name`, if it has one.
    pub fn resource_policy(&self, function_name: &str) -> Option<&ResourcePolicy> {
        self.functions
            .get(function_name)?
            .resource_policy
            .as_ref()
    }
}
