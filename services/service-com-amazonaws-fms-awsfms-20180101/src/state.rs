// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-fms-awsfms-20180101 service.
//!
//! Firewall Manager's control plane is organisation-scoped: ONE administrator
//! account association, and a flat set of policies keyed by the `PolicyId` the
//! service mints. Both live here so every operation reads and writes the same
//! store; the helpers below are the bits of derivation several handlers share
//! (the request's Region, the policy ARN, the `PolicySummary` projection and the
//! pagination token codec), kept in one place so two handlers cannot disagree.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::SystemTime;

use crate::ops::types::{Policy, PolicySummary, Tag};

/// The Region an emulated service reports when the request carries no SigV4
/// credential scope to read one from (an unsigned request).
const DEFAULT_REGION: &str = "us-east-1";

/// The ARN namespace / signing name of this service, per its Smithy model
/// (`aws.api#service` arnNamespace `fms`).
const ARN_NAMESPACE: &str = "fms";

/// The Firewall Manager administrator account association.
///
/// `role_status` is an `AccountRoleStatus` value (see
/// [`crate::ops::types::ACCOUNT_ROLE_STATUS_VALUES`]); the association is
/// returned ready, so it is `READY` from the moment it is written.
///
/// `associated_at` is recorded here but not yet reported by any implemented
/// operation: it is what the later `DisassociateAdminAccount` /
/// `ListAdminAccountsForOrganization` slice reads, so it is stored now rather
/// than being a derivation a reader computes.
#[derive(Debug, Clone)]
pub struct AdminAccountRecord {
    pub account_id: String,
    pub role_status: String,
    pub associated_at: SystemTime,
}

/// `AccountRoleStatus` of an administrator account whose role exists and is
/// usable. One of [`crate::ops::types::ACCOUNT_ROLE_STATUS_VALUES`].
pub const ROLE_STATUS_READY: &str = "READY";

/// The `CustomerPolicyScopeIdType` key under which a policy's `IncludeMap` /
/// `ExcludeMap` holds ACCOUNT IDS (the only other key, `ORG_UNIT`, holds
/// organizational-unit ids). One of
/// [`crate::ops::types::CUSTOMER_POLICY_SCOPE_ID_TYPE_VALUES`].
pub const SCOPE_ID_TYPE_ACCOUNT: &str = "ACCOUNT";

/// The default `CustomerPolicyStatus` of a policy an administrator created: the
/// administrator can manage and delete it. One of
/// [`crate::ops::types::CUSTOMER_POLICY_STATUS_VALUES`].
pub const POLICY_STATUS_ACTIVE: &str = "ACTIVE";

/// One stored Firewall Manager policy: the modelled `Policy` exactly as the
/// caller sent it (plus the identifiers the service minted), its ARN, the
/// current update token, and its tags.
///
/// `created_at` / `updated_at` are recorded but not yet reported: no modelled
/// member of `Policy` or `PolicySummary` carries them, and they are what the
/// later compliance-status slice (`ListComplianceStatus`, which reports a
/// policy's last evaluation) reads.
#[derive(Debug, Clone)]
pub struct PolicyRecord {
    pub policy_id: String,
    pub policy_arn: String,
    pub update_token: String,
    pub policy: Policy,
    pub tags: Vec<Tag>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl PolicyRecord {
    /// The stored policy as `GetPolicy`/`PutPolicy` return it: the caller's own
    /// members, with the service-owned `PolicyId` and `PolicyUpdateToken`
    /// filled in from the record so a read never reports a stale token.
    pub fn policy(&self) -> Policy {
        let mut policy = self.policy.clone();
        policy.policy_id = Some(self.policy_id.clone());
        policy.policy_update_token = Some(self.update_token.clone());
        policy
    }

    /// The `PolicySummary` projection `ListPolicies` reports, every member read
    /// off the stored policy (`SecurityServiceType` is the stored
    /// `SecurityServicePolicyData.Type`).
    pub fn summary(&self) -> PolicySummary {
        PolicySummary {
            policy_arn: Some(self.policy_arn.clone()),
            policy_id: Some(self.policy_id.clone()),
            policy_name: Some(self.policy.policy_name.clone()),
            resource_type: Some(self.policy.resource_type.clone()),
            security_service_type: Some(self.policy.security_service_policy_data.r#type.clone()),
            remediation_enabled: self.policy.remediation_enabled,
            delete_unused_fm_managed_resources: self.policy.delete_unused_fm_managed_resources,
            policy_status: self.policy.policy_status.clone(),
        }
    }
}

/// Per-service state for com-amazonaws-fms-awsfms-20180101. A handler reaches it through [`service_state`].
///
/// `policies` is a `BTreeMap` so a page boundary is a key in a total order:
/// `ListPolicies` can resume from a token without holding a cursor, and a
/// policy added or deleted between pages cannot reshuffle the ones already
/// returned.
#[derive(Debug, Default)]
pub struct ComAmazonawsFmsAwsfms20180101State {
    /// The single Firewall Manager administrator association, if one was made.
    pub admin_account: Option<AdminAccountRecord>,
    /// Stored policies, keyed by the `PolicyId` this service minted.
    pub policies: BTreeMap<String, PolicyRecord>,
    /// Tags of every Firewall Manager resource, keyed by its ARN: the ARN-keyed
    /// INDEX the later `ListTagsForResource` / `TagResource` / `UntagResource`
    /// slice needs (those operations resolve a resource by ARN, not by policy
    /// id), still a 501 tracked by its own gap issues.
    ///
    /// Which copy wins: for a policy, [`PolicyRecord::tags`] is authoritative and
    /// this map is maintained from it — `PutPolicy` writes the record's tags into
    /// it under the minted ARN and `DeletePolicy` removes the entry. A tagging
    /// operation must write the record first and refresh this entry from it, or
    /// (better) the index can be derived by iterating `policies` by `policy_arn`
    /// and this map dropped, once a Firewall Manager resource other than a policy
    /// is stored here too.
    pub tags_by_arn: HashMap<String, Vec<Tag>>,
}

impl ComAmazonawsFmsAwsfms20180101State {
    /// The `PolicyId` of the stored policy whose ARN is `arn`, or `None` when no
    /// stored Firewall Manager resource carries that ARN.
    ///
    /// This is the ARN -> resource resolution every tagging operation does
    /// before it reads or writes a tag: a policy is the only taggable Firewall
    /// Manager resource this service persists today (applications lists and
    /// protocols lists are still 501s), so an ARN naming anything else resolves
    /// to nothing and the caller gets the modelled not-found.
    pub fn policy_id_for_arn(&self, arn: &str) -> Option<String> {
        self.policies
            .iter()
            .find(|(_, record)| record.policy_arn == arn)
            .map(|(policy_id, _)| policy_id.clone())
    }

    /// The Firewall Manager administrator accounts that manage the calling
    /// account, in account-id order.
    ///
    /// An organisation has ONE default administrator and its `AdminScope` is the
    /// whole organisation, so while an association exists it manages every
    /// account in that organisation — including the caller. With no association
    /// there is no administrator to enumerate at all, which the caller is told
    /// with the modelled `ResourceNotFoundException` rather than an empty page.
    pub fn managing_admin_accounts(&self) -> Vec<String> {
        let mut accounts: Vec<String> = self
            .admin_account
            .iter()
            .map(|admin| admin.account_id.clone())
            .collect();
        accounts.sort();
        accounts
    }

    /// The accounts of the administrator's organization this store records, in
    /// ascending account-id order.
    ///
    /// Firewall Manager learns its organization's accounts from Organizations,
    /// which this emulator does not stand in for; what it DOES hold is every
    /// account id its own control plane was told about:
    ///
    /// * the associated administrator account — `AssociateAdminAccount`
    ///   documents that it "must be a member account of the organization in
    ///   Organizations whose resources you want to protect";
    /// * every account id a stored policy names under the
    ///   [`SCOPE_ID_TYPE_ACCOUNT`] key of its `IncludeMap` or `ExcludeMap` —
    ///   `CustomerPolicyScopeMap`'s accounts are the organization's accounts the
    ///   policy applies to or leaves out.
    ///
    /// Values that are not `AWSAccountId`s (the model's
    /// `com.amazonaws.fms#AWSAccountId`: length 1..1024, `^[0-9]+$`) are left
    /// out: a scope map's `CustomerPolicyScopeId` is a free-form string, and an
    /// account list may only carry account ids.
    ///
    /// A `BTreeSet` so the order is total and an account named by two policies
    /// is reported once: a page boundary is then a key, not a cursor.
    pub fn member_accounts(&self) -> BTreeSet<String> {
        let mut accounts = BTreeSet::new();
        if let Some(admin) = self.admin_account.as_ref() {
            accounts.insert(admin.account_id.clone());
        }
        for record in self.policies.values() {
            for scope_map in [&record.policy.include_map, &record.policy.exclude_map]
                .into_iter()
                .flatten()
            {
                let Some(scoped) = scope_map.get(SCOPE_ID_TYPE_ACCOUNT) else {
                    continue;
                };
                accounts.extend(
                    scoped
                        .iter()
                        .filter(|id| is_account_id(id))
                        .map(String::clone),
                );
            }
        }
        accounts
    }
}

/// Merge `added` into `existing` the way a tag write does: a key already present
/// takes the new value in place, a key that is new is appended.
///
/// Tags are a key:value MAP (`com.amazonaws.fms#Tag` is one key and one value),
/// so a resource can never hold two values for one key; and "add one or more
/// tags" leaves the keys the request did not mention alone.
pub fn merge_tags(existing: &mut Vec<Tag>, added: &[Tag]) {
    for tag in added {
        match existing.iter_mut().find(|held| held.key == tag.key) {
            Some(held) => held.value = tag.value.clone(),
            None => existing.push(tag.clone()),
        }
    }
}

/// The reason `arn` is not an Amazon Resource Name at all, or `None` when it is
/// shaped like one.
///
/// Firewall Manager checks the FORM of a `ResourceArn` before it looks anything
/// up, and the modelled `ResourceArn` pattern cannot express it (that pattern
/// accepts any string of letters, digits and punctuation), so this check lives
/// here and answers a 400 `ValidationException` — a malformed ARN is a caller
/// programming error, distinct from an ARN that is well formed but names
/// nothing.
///
/// The form is the documented ARN layout
/// `arn:<partition>:<service>:<region>:<account-id>:<resource>`: the literal
/// `arn` prefix, six colon-separated fields, and a non-empty partition, service
/// and resource. Region and account id are deliberately allowed to be empty —
/// ARNs of global and account-less resources leave them so.
pub fn arn_format_error(arn: &str) -> Option<String> {
    if !arn.starts_with("arn:") {
        return Some(format!(
            "Incorrect arn format. error:ARNs must start with 'arn:': {arn}"
        ));
    }
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    if fields.len() < 6 {
        return Some(format!(
            "Incorrect arn format. error:ARNs must have six colon-separated fields, \
             found {}: {arn}",
            fields.len()
        ));
    }
    let empty = match (fields[1], fields[2], fields[5]) {
        ("", _, _) => Some("partition"),
        (_, "", _) => Some("service"),
        (_, _, "") => Some("resource"),
        _ => None,
    };
    empty.map(|field| format!("Incorrect arn format. error:ARNs must name a {field}: {arn}"))
}

/// Whether `value` is an `AWSAccountId` as this service's model declares it
/// (`com.amazonaws.fms#AWSAccountId`: length 1..1024, pattern `^[0-9]+$`).
pub fn is_account_id(value: &str) -> bool {
    (1..=1024).contains(&value.len()) && value.bytes().all(|b| b.is_ascii_digit())
}

/// The Firewall Manager administrator the organization has associated, or the
/// service's authorization refusal for a caller that is not entitled to an
/// organization-scoped call.
///
/// Firewall Manager answers an organization-scoped request only for a Firewall
/// Manager administrator or the organization's management account
/// (`ListMemberAccounts`: "Either an Firewall Manager administrator or the
/// organization's management account can make this request"). This emulator
/// presents ONE account, [`::engine::emulator_account_id`], and it is the
/// account that associates the administrator — so what decides the gate is
/// whether an administrator association exists at all. With none, the caller is
/// neither of the two, and the live service refuses the call with the error code
/// `AccessDeniedException`, which this service's model declares for no
/// operation (hence [`crate::scaffolded::Scaffolded::access_denied`], HTTP 400).
///
/// The refusal's message is BUILT FROM THE REQUEST — it names the calling
/// account and the Region the request was signed for, the two things that decide
/// which administrator association was looked for — so `region` is the caller's
/// Region ([`request_region`]), never a constant.
///
/// Generic over the operation's modeled error enum, so every administrator-gated
/// handler shares this one gate.
pub fn require_administrator<'store, E>(
    store: &'store ComAmazonawsFmsAwsfms20180101State,
    region: &str,
) -> Result<&'store AdminAccountRecord, crate::scaffolded::Scaffolded<E>> {
    store.admin_account.as_ref().ok_or_else(|| {
        crate::scaffolded::Scaffolded::access_denied(format!(
            "No default admin could be found for account {} in Region {region}",
            ::engine::emulator_account_id()
        ))
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsFmsAwsfms20180101State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsFmsAwsfms20180101State>)
}

/// The Region this request was signed for, which is the Region whose resources
/// the request addresses — so it is the Region segment of any ARN the response
/// carries. Falls back to [`DEFAULT_REGION`] for an unsigned request.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of a Firewall Manager policy:
/// `arn:aws:fms:<region>:<account>:policy/<PolicyId>`.
pub fn policy_arn(region: &str, policy_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("policy/{policy_id}"))
}

/// Encode a page boundary (the key the next page starts at — a `PolicyId` for
/// `ListPolicies`, an account id for `ListMemberAccounts` and
/// `ListAdminsManagingAccount`) as an opaque
/// `PaginationToken`.
///
/// Hex keeps the token inside the modelled `PaginationToken` pattern
/// (`^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`) and its 1..4096 length, and makes it
/// opaque to a caller: nothing in it is meaningful without decoding.
pub fn encode_page_token(policy_id: &str) -> String {
    ::hex::encode(policy_id)
}

/// Decode a `PaginationToken` produced by [`encode_page_token`]. `None` when the
/// token was not produced by this service.
///
/// The decoded value must still be a `PolicyId` (the model's
/// `com.amazonaws.fms#PolicyId`: 36 characters matching `^[a-z0-9A-Z-]{36}$`),
/// so a foreign token that happens to be valid hex is refused rather than
/// resumed from as an arbitrary key.
pub fn decode_page_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let is_policy_id = decoded.len() == 36
        && decoded
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-');
    is_policy_id.then_some(decoded)
}

/// Decode an account-keyed `PaginationToken` produced by
/// [`encode_page_token`]. `None` when the token was not produced by this
/// service.
///
/// The decoded value must still be an `AWSAccountId` ([`is_account_id`]), so a
/// token minted for a policy page — or any foreign token that happens to be
/// valid hex — is refused rather than resumed from as an arbitrary key.
pub fn decode_account_page_token(token: &str) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    is_account_id(&decoded).then_some(decoded)
}

/// Mint a fresh service-owned identifier: 36 lowercase hex characters, which
/// satisfies the model's `PolicyId` pattern (`^[a-z0-9A-Z-]{36}$`, length 36)
/// and also `PolicyUpdateToken`'s (`^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`, length
/// 1..1024), so one mint serves both.
///
/// The value is the SHA-256 of a process-wide monotonic counter and the current
/// instant, hex-encoded and truncated to the modelled 36 characters: the counter
/// makes two mints in one process distinct whatever the clock does (so a rotated
/// `PolicyUpdateToken` always differs from the one it replaced, even within the
/// same nanosecond), and the instant separates two processes. Hashing and
/// hex-encoding are the `sha2` and `hex` crates' work, not hand-written here,
/// and nothing from the request goes into it.
pub fn mint_identifier() -> String {
    use ::sha2::Digest;

    /// Distinguishes mints within one process; `Relaxed` is enough because only
    /// the uniqueness of the returned value matters, not the order.
    static MINTED: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);

    let counter = MINTED.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map_or(0_u128, |since_epoch| since_epoch.as_nanos());
    let mut hasher = ::sha2::Sha256::new();
    hasher.update(counter.to_be_bytes());
    hasher.update(nanos.to_be_bytes());
    let mut id = ::hex::encode(hasher.finalize());
    // SHA-256 renders as 64 hex characters, so the modelled 36 are always there.
    id.truncate(36);
    id
}
