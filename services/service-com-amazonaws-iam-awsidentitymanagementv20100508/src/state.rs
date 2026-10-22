// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-iam-awsidentitymanagementv20100508 service.
//!
//! This module owns the persisted IAM entities plus the small helpers every
//! user-lifecycle handler needs (name keying, ARN building, id minting, opaque
//! pagination markers). Handlers reach it through [`service_state`]; they never
//! keep a second copy of a resource.

use std::collections::BTreeMap;

use crate::ops::types::User;

/// IAM's default entity path: every entity whose request omits `Path` is created
/// directly under the account root. The model's `pathType` documentation states
/// the parameter "defaults to a slash (/)".
pub const DEFAULT_PATH: &str = "/";

/// The number of entities `ListUsers` (and IAM's other list operations) returns
/// when the request supplies no `MaxItems`. IAM's `MaxItems` documentation states
/// that "if you do not include this parameter, the number of items defaults to
/// 100".
pub const DEFAULT_MAX_ITEMS: i32 = 100;

/// The one value the model's `PermissionsBoundaryAttachmentType` enum declares
/// (`smithy_query(query="shape", shape="PermissionsBoundaryAttachmentType")`),
/// reported on every entity that carries a permissions boundary.
pub const PERMISSIONS_BOUNDARY_TYPE: &str = "PermissionsBoundaryPolicy";

/// The prefix AWS gives an IAM user's unique id. `User$UserId` is modelled as
/// `idType` (len 16..128, pattern `^[\w]+$`), which this prefix plus
/// [`UNIQUE_ID_SUFFIX_LEN`] base32 characters satisfies.
const USER_ID_PREFIX: &str = "AIDA";

/// How many IAM users one account may hold. AWS's "IAM and STS quotas" page gives
/// the quota "Users in an account: 5,000", and a create past it is refused with
/// `LimitExceededException`, whose documentation says "the error message describes
/// the limit exceeded".
pub const MAX_USERS_PER_ACCOUNT: usize = 5_000;

/// The aggregate size, in characters, of all inline policies embedded in one IAM
/// user. AWS's "IAM and STS quotas" page gives the quota "User policy size cannot
/// exceed 2,048 characters"; a put that would carry the user past it is refused
/// with `LimitExceededException`.
pub const MAX_USER_INLINE_POLICY_CHARACTERS: usize = 2_048;

/// How many base32 characters follow the prefix in a minted unique id.
const UNIQUE_ID_SUFFIX_LEN: usize = 17;

/// What every pagination `Marker` this service issues carries, inside the encoded
/// payload, so a token it did not issue is recognised as foreign rather than read
/// as a position in the store.
const MARKER_PREFIX: &str = "iam:users:";

/// The alphabet the suffix is drawn from: RFC 4648 base32, every character of
/// which is an uppercase alphanumeric and so inside `idType`'s `^[\w]+$`.
const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// One inline policy embedded in an IAM entity.
///
/// The name is kept beside the document because the map is keyed by the
/// case-folded name ([`name_key`]) while a read must report the policy as the
/// account holds it.
#[derive(Debug, Clone)]
pub struct InlinePolicy {
    /// The policy name in the case the last put chose.
    pub policy_name: String,
    /// The policy document exactly as it was stored.
    pub policy_document: String,
}

/// Per-service state for com-amazonaws-iam-awsidentitymanagementv20100508. A handler reaches it through [`service_state`].
///
/// Every map is a `BTreeMap` so enumeration order is total and stable, which is
/// what lets a list operation hand out a resumable marker: the next page starts
/// strictly after the key the marker names.
#[derive(Debug, Default)]
pub struct ComAmazonawsIamAwsidentitymanagementv20100508State {
    /// The persisted IAM users, keyed by [`name_key`] of the user name. IAM names
    /// "are not distinguished by case" (`CreateUser$UserName` documentation), so
    /// the key is the case-folded name while `User::user_name` keeps the case the
    /// creator chose.
    pub users: BTreeMap<String, User>,
    /// Inline user policies: [`name_key`] of the user name -> [`name_key`] of the
    /// policy name -> the policy. Policy names are case-folded for the same reason
    /// user names are: the model's `CreateUser$UserName` documentation says "user,
    /// group, role, and policy names must be unique within the account. Names are
    /// not distinguished by case."
    pub user_inline_policies: BTreeMap<String, BTreeMap<String, InlinePolicy>>,
    /// Access key id -> [`name_key`] of the user that owns it. `GetUser` with no
    /// `UserName` resolves the caller through this map; `CreateAccessKey` (still a
    /// stub) is what will populate it.
    pub access_keys: BTreeMap<String, String>,
    /// Monotonic counter feeding [`next_user_id`](Self::next_user_id).
    pub user_id_seq: u64,
}

impl ComAmazonawsIamAwsidentitymanagementv20100508State {
    /// Mint the next user's `UserId`.
    ///
    /// Unique by construction: the counter is bijectively mixed and the whole
    /// 64-bit result is spelled out in the suffix, so two distinct counter values
    /// can never produce one id. Stable by construction too — the id is minted
    /// once, stored with the user, and every later read returns the stored value.
    pub fn next_user_id(&mut self) -> String {
        self.user_id_seq += 1;
        unique_id(USER_ID_PREFIX, self.user_id_seq)
    }

    /// The aggregate size, in characters, of the inline policies embedded in the
    /// user keyed by `user_key`, counting every policy except the one keyed by
    /// `replacing` (a put of an existing name replaces that document rather than
    /// adding to it).
    ///
    /// This is the measure IAM's user policy size quota
    /// ([`MAX_USER_INLINE_POLICY_CHARACTERS`]) is taken against.
    pub fn user_inline_policy_characters(&self, user_key: &str, replacing: &str) -> usize {
        self.user_inline_policies
            .get(user_key)
            .map(|policies| {
                policies
                    .iter()
                    .filter(|(key, _)| key.as_str() != replacing)
                    .map(|(_, policy)| policy.policy_document.chars().count())
                    .sum()
            })
            .unwrap_or(0)
    }
}

/// The case-folded form of an IAM entity name, used as its key.
///
/// IAM entity names "are not distinguished by case", so `MyUser` and `myuser`
/// name one entity. Entity names are constrained to `^[\w+=,.@-]+$`, so ASCII
/// folding is total over the accepted alphabet.
pub fn name_key(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// The ARN of an IAM user: `arn:aws:iam::<account>:user<path><name>`.
///
/// `path` already begins and ends with `/` (the model's `pathType` pattern), so
/// it joins the `user` resource type to the name without extra separators. IAM is
/// a global service, so the region segment is empty.
pub fn user_arn(path: &str, user_name: &str) -> String {
    ::engine::emulator_arn("iam", "", &format!("user{path}{user_name}"))
}

/// The wall-clock instant a resource was created or last changed, in the
/// modelled timestamp type.
pub fn now() -> ::aws_smithy_types::DateTime {
    let since_epoch = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_millis(since_epoch.as_millis() as i64)
}

/// Encode a state map key as the opaque pagination `Marker` a client echoes back.
///
/// The codec is `aws_smithy_types::base64` — already in this crate's dependency
/// tree, since it is what the wire protocols use for blob members — so the token
/// needs no dependency of its own. Base64's alphabet is inside `markerType`'s
/// `^[\u0020-\u00FF]+$` pattern and its 1..320 length for any key the entity-name
/// constraints admit, and the token carries no structure a client could depend on
/// beyond being echo-able.
pub fn encode_marker(key: &str) -> String {
    ::aws_smithy_types::base64::encode(format!("{MARKER_PREFIX}{key}"))
}

/// The state map key an opaque `Marker` names, or `None` when the token was not
/// produced by [`encode_marker`] — which the prefix is what establishes, since a
/// foreign token that happens to be valid base64 still does not carry it.
pub fn decode_marker(marker: &str) -> Option<String> {
    let decoded = String::from_utf8(::aws_smithy_types::base64::decode(marker).ok()?).ok()?;
    decoded.strip_prefix(MARKER_PREFIX).map(str::to_string)
}

/// A unique entity id: `prefix` followed by [`UNIQUE_ID_SUFFIX_LEN`] base32
/// characters that spell out `seq` mixed through SplitMix64.
///
/// The mix is a bijection on `u64` and the first 13 suffix characters carry all
/// 64 of its bits, so distinct `seq` values always yield distinct ids. The
/// remaining characters come from a second, independently mixed word purely so
/// the id has the shape and entropy of an AWS unique id rather than a visible
/// counter.
fn unique_id(prefix: &str, seq: u64) -> String {
    let mut id = String::with_capacity(prefix.len() + UNIQUE_ID_SUFFIX_LEN);
    id.push_str(prefix);
    let mut word = splitmix64(seq);
    for i in 0..UNIQUE_ID_SUFFIX_LEN {
        // 13 base32 characters cover all 64 bits of the first word; after that,
        // continue from a second word keyed to the same counter.
        if i == 13 {
            word = splitmix64(seq ^ 0x9E37_79B9_7F4A_7C15);
        }
        id.push(BASE32_ALPHABET[(word & 31) as usize] as char);
        word >>= 5;
    }
    id
}

/// SplitMix64's finalizer: a bijection on `u64` that scatters consecutive inputs.
fn splitmix64(seq: u64) -> u64 {
    let mut z = seq.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsIamAwsidentitymanagementv20100508State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsIamAwsidentitymanagementv20100508State>)
}
