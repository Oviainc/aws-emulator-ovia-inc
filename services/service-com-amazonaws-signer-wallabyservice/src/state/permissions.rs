// SPDX-License-Identifier: CC0-1.0

//! The cross-account permission policy of a signing profile, as this emulator
//! persists it.
//!
//! A signing profile carries a resource policy of its own: `AddProfilePermission`
//! adds one statement to it, `RemoveProfilePermission` takes one out, both mint
//! the policy's next revision id, and `ListProfilePermissions` reports the
//! statements, that revision and the size of the document they form. The policy
//! lives beside [`super::SigningProfileRecord`] rather than inside it because the
//! two have different lifetimes — see [`ProfilePermissionPolicy`].
//!
//! The store itself is [`super::ComAmazonawsSignerWallabyserviceState::profile_permissions`];
//! every handler reads and writes it through [`super::service_state`].

use std::collections::BTreeMap;

use super::SigningProfileRecord;

/// A resource the request's own values named — a statement id, a profile version —
/// does not exist: the modelled shape's own name, which is all a 404 for a
/// sub-resource Signer publishes no code for can say.
pub const ERROR_CODE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The request lost an optimistic-concurrency check, or asked for something the
/// resource's current state already holds: the modelled shape's own name.
pub const ERROR_CODE_CONFLICT: &str = "ConflictException";

/// The request would carry a resource past a service limit: the modelled shape's
/// own name.
pub const ERROR_CODE_SERVICE_LIMIT_EXCEEDED: &str = "ServiceLimitExceededException";

/// The AWS Signer actions a cross-account permission may grant, sorted.
///
/// Not a chosen list: `AddProfilePermissionRequest$action` documents "You can
/// designate the following actions to an account" and then names exactly these
/// four (`smithy_query(query="doc", shape="AddProfilePermissionRequest",
/// member="action")`), so an `action` outside this catalog is one no AWS Signer
/// policy statement could hold.
pub const GRANTABLE_ACTIONS: [&str; 4] = [
    "signer:GetSigningProfile",
    "signer:RevokeSignature",
    "signer:SignPayload",
    "signer:StartSigningJob",
];

/// Whether `action` is one of the [`GRANTABLE_ACTIONS`].
pub fn is_grantable_action(action: &str) -> bool {
    GRANTABLE_ACTIONS.contains(&action)
}

/// The refusal prose for an `action` outside the documented catalog: the member,
/// the value and the values the member admits, so the caller can fix the request
/// from the text alone.
pub fn ungrantable_action_message(action: &str) -> String {
    format!(
        "action {action} is not an AWS Signer action that can be granted to another account; \
         expected one of {}",
        GRANTABLE_ACTIONS.join(", ")
    )
}

/// The IAM policy-language version every Signer profile permission policy is
/// written in — the one AWS's policy grammar publishes
/// (`"Version": "2012-10-17"`).
const POLICY_DOCUMENT_VERSION: &str = "2012-10-17";

/// The most bytes one profile's cross-account permission policy may hold.
///
/// Signer publishes no quota for it, so this is the bound AWS puts on a
/// RESOURCE-BASED policy document elsewhere — 20,480 bytes, the documented
/// maximum for a resource policy (AWS Lambda's `AddPermission` quota, the same
/// kind of document in the same policy language). It is what keeps ONE profile's
/// policy from growing without limit, one small `AddProfilePermission` at a time:
/// a grant that would carry the document past it is refused with the modelled
/// `ServiceLimitExceededException`, which this operation declares for exactly
/// that ("The client is making a request that exceeds service limits"). This one
/// constant is where a figure Signer publishes would replace it.
pub const MAX_POLICY_SIZE_BYTES: i32 = 20_480;

/// The refusal prose for a grant that would carry a policy past
/// [`MAX_POLICY_SIZE_BYTES`], built from the request's own effect on the policy
/// and saying whose bound it is, since Signer publishes none.
pub fn policy_too_large_message(profile_name: &str, resulting: i32) -> String {
    format!(
        "this emulator bounds the permissions policy of a signing profile at \
         {MAX_POLICY_SIZE_BYTES} bytes; this request would leave the policy of \
         {profile_name} holding {resulting}"
    )
}

/// The largest page `ListProfilePermissions` serves, and the page size it uses.
///
/// `ListProfilePermissionsRequest` models no `maxResults`, so the service picks
/// the page size; this is the page ceiling the service's other paginated list
/// publishes (`ListSigningProfilesRequest$maxResults` is
/// `@range(min: 1, max: 25)` — see [`super::PROFILE_PAGE_LIMIT`]), so one page of
/// permissions costs a reader no more than one page of profiles however many
/// statements the policy holds.
pub const PERMISSION_PAGE_LIMIT: usize = 25;

/// One cross-account permission statement, as `AddProfilePermission` stored it.
#[derive(Debug, Clone)]
pub struct ProfilePermissionRecord {
    /// The statement's unique id within the profile's policy.
    pub statement_id: String,
    /// One of [`GRANTABLE_ACTIONS`].
    pub action: String,
    /// The AWS principal the action was granted to.
    pub principal: String,
    /// The profile version this statement applies to, when the grant named one;
    /// `None` for a grant that names no version and so applies to the profile.
    pub profile_version: Option<String>,
}

impl ProfilePermissionRecord {
    /// The `Permission` summary `ListProfilePermissions` reports.
    pub fn to_permission(&self) -> crate::ops::types::Permission {
        crate::ops::types::Permission {
            action: Some(self.action.clone()),
            principal: Some(self.principal.clone()),
            statement_id: Some(self.statement_id.clone()),
            profile_version: self.profile_version.clone(),
        }
    }

    /// The resource this statement grants access to: the profile version it names,
    /// or the profile itself when it names none.
    fn resource_arn(&self, profile: &SigningProfileRecord) -> String {
        match self.profile_version.as_deref() {
            Some(version) => format!("{}/{version}", profile.arn),
            None => profile.arn.clone(),
        }
    }

    /// This statement as a policy-document statement.
    fn to_statement_json(&self, profile: &SigningProfileRecord) -> ::serde_json::Value {
        ::serde_json::json!({
            "Sid": self.statement_id,
            "Effect": "Allow",
            "Principal": { "AWS": self.principal },
            "Action": self.action,
            "Resource": self.resource_arn(profile),
        })
    }
}

/// The cross-account permission policy attached to ONE signing profile: the
/// statements `AddProfilePermission` added and the revision id that identifies
/// their current state.
///
/// The policy is a resource of its own rather than a member of
/// [`SigningProfileRecord`] because the two have different lifetimes: a
/// `PutSigningProfile` on an existing name REPLACES the profile record (minting a
/// new version), while the profile's policy and its revision chain survive that
/// put untouched.
///
/// `Default` is the EMPTY policy: no statements, and a revision id no mint ever
/// produces (minted ones are UUID-shaped), so it is only ever used as the thing a
/// first grant is measured against or inserted into — every write sets the
/// revision before the policy is readable.
#[derive(Debug, Clone, Default)]
pub struct ProfilePermissionPolicy {
    /// The id of the policy's current revision — minted afresh by every
    /// `AddProfilePermission` / `RemoveProfilePermission` that changes it, which is
    /// what makes a request carrying an older id detectably stale.
    pub revision_id: String,
    /// The statements, keyed by `statementId`. A `BTreeMap` is what gives
    /// `ListProfilePermissions` the stable total order its pagination token
    /// indexes into.
    pub statements: BTreeMap<String, ProfilePermissionRecord>,
}

impl ProfilePermissionPolicy {
    /// The policy document these statements form, in AWS's policy language, with
    /// `extra` counted as one more statement when it is given.
    ///
    /// `extra` is appended rather than sorted into place: it is used to MEASURE a
    /// policy before a statement is added, and the document's length does not
    /// depend on the order its statements are written in.
    fn document(
        &self,
        profile: &SigningProfileRecord,
        extra: Option<&ProfilePermissionRecord>,
    ) -> String {
        let mut statements: Vec<::serde_json::Value> = self
            .statements
            .values()
            .map(|statement| statement.to_statement_json(profile))
            .collect();
        statements.extend(extra.map(|statement| statement.to_statement_json(profile)));
        ::serde_json::json!({
            "Version": POLICY_DOCUMENT_VERSION,
            "Statement": statements,
        })
        .to_string()
    }

    /// `policySizeBytes`: "Total size of the policy associated with the Signing
    /// Profile in bytes" — the byte length of the policy document these
    /// statements form, computed from the stored statements rather than tracked
    /// beside them, so it can never disagree with what the policy holds.
    pub fn policy_size_bytes(&self, profile: &SigningProfileRecord) -> i32 {
        size_in_bytes(&self.document(profile, None))
    }

    /// What [`Self::policy_size_bytes`] would report with `statement` added — the
    /// size a grant is admitted or refused on, measured BEFORE anything is
    /// written.
    pub fn policy_size_bytes_with(
        &self,
        profile: &SigningProfileRecord,
        statement: &ProfilePermissionRecord,
    ) -> i32 {
        size_in_bytes(&self.document(profile, Some(statement)))
    }
}

/// `document`'s byte length as the wire integer `policySizeBytes` reports.
fn size_in_bytes(document: &str) -> i32 {
    i32::try_from(document.len()).unwrap_or(i32::MAX)
}
