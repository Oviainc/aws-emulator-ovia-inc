// SPDX-License-Identifier: CC0-1.0

//! Server-side encryption of a queue: which of the two options a queue is on, and —
//! for a queue on a customer managed KMS key — whether the caller may have the key
//! used on its behalf.
//!
//! # The two options are exclusive, and one of them is the default
//!
//! A queue is encrypted at rest either with a key Amazon SQS owns (SSE-SQS, the
//! `SqsManagedSseEnabled` attribute) or with a KMS key the account names
//! (SSE-KMS, the `KmsMasterKeyId` attribute), and "only one server-side encryption
//! option is supported per queue" (the `SqsManagedSseEnabled` entry of the
//! `SetQueueAttributes` `Attributes` documentation). So:
//!
//! * the option a request CHOOSES takes the queue off the other one
//!   ([`apply_encryption_choice`]), and a request that names both at once is refused
//!   ([`encryption_choice_refusal`]);
//! * what a queue REPORTS follows from the option it is on ([`report`]): a queue on the
//!   SQS-owned key reports `SqsManagedSseEnabled` and no KMS attribute, and a queue on a
//!   KMS key reports that key, `SqsManagedSseEnabled` of `false`, and the data key reuse
//!   period that key is used under.
//!
//! # A queue on a customer managed key
//!
//! Amazon SQS does not hold the key: it asks KMS to generate a data key when a producer
//! sends a message and to decrypt one when a consumer receives it, as the CALLER, so a
//! caller the key does not permit cannot have the queue's key used for it and the call
//! is refused with the KMS error KMS itself would answer — which is why every one of
//! those operations models the `Kms*` family. [`authorize_master_key`] is that one step,
//! for each of them.
//!
//! The key is resolved in the KMS service's OWN store, so this reads exactly what KMS's
//! handlers wrote and the two cannot disagree about a key's existence, its state or its
//! policy. A queue on an Amazon Web Services managed key (the predefined `alias/aws/sqs`
//! and anything else whose `KeyManager` is not the customer's) needs no grant from the
//! caller: AWS manages that key's policy for the service.
//!
//! The decision itself is [`::service_common::iam_authorization`], the tree's one
//! evaluation of policy documents against `(action, resource, principal)`, over the
//! three things a KMS access decision is made from:
//!
//! 1. **who** — the principal the request's access key id speaks for, from STS's own
//!    resolver (the place `sts:GetCallerIdentity` answers from), else the IAM user that
//!    owns the long-term key, else the account's own published credential;
//! 2. **what the key permits** — the key's own key policy, which for a KMS key is the
//!    document that must allow the use: a key policy naming the ACCOUNT ROOT delegates
//!    the decision to IAM, which is what makes the caller's identity-based policies part
//!    of it;
//! 3. **the decision** — an explicit `Deny` refuses; otherwise the call is allowed when
//!    the key policy allows this principal directly, or it delegates to IAM and the
//!    principal's identity-based policies allow it.
//!
//! Two callers are not judged on a GRANT at all, exactly as every other service in this
//! tree that authorizes reads them: the account's own published credential, which holds no
//! IAM policies — the position the account's administrator is in — and a principal
//! carrying a policy document this emulator cannot read (an AWS-managed policy), which may
//! hold the grant. Neither is refused for want of evidence nobody has. An explicit `Deny`
//! stated in the key's own policy is evidence that DOES exist, and it covers every
//! principal the statement matches — the account root included — so it refuses both of
//! them too: no grant, and no absence of one, overturns a deny.
//!
//! Amazon SQS does not word these refusals itself: it relays the error its KMS call
//! failed with, so each message is KMS's own sentence with the metadata block an AWS error
//! carries — see [`relayed`].
//!
//! When no KMS service is mounted beside this one the lookup answers nothing about the
//! key, and the call proceeds: a condition nobody attributed to the key never refuses a
//! send or a receive.

use std::collections::{BTreeMap, HashMap};

use ::service_com_amazonaws_kms_trentservice::state as kms_store;
use ::service_common::iam_authorization::{self as iam, Decision};

use crate::ops::types::InvalidAttributeName;

/// `SqsManagedSseEnabled`: "enables server-side queue encryption using SQS owned
/// encryption keys."
pub const SQS_MANAGED_SSE_ENABLED: &str = "SqsManagedSseEnabled";

/// `KmsMasterKeyId`: "the ID of an Amazon Web Services managed customer master key
/// (CMK) for Amazon SQS or a custom CMK."
pub const KMS_MASTER_KEY_ID: &str = "KmsMasterKeyId";

/// `KmsDataKeyReusePeriodSeconds`: "the length of time, in seconds, for which Amazon SQS
/// can reuse a data key to encrypt or decrypt messages before calling KMS again."
pub const KMS_DATA_KEY_REUSE_PERIOD_SECONDS: &str = "KmsDataKeyReusePeriodSeconds";

/// The documented default of [`KMS_DATA_KEY_REUSE_PERIOD_SECONDS`]: "an integer
/// representing seconds, between 60 seconds (1 minute) and 86,400 seconds (24 hours).
/// Default: 300 (5 minutes)." It is the period a KMS key is used under, so it is
/// reported for a queue that is ON a KMS key.
pub const DEFAULT_DATA_KEY_REUSE_PERIOD_SECONDS: &str = "300";

/// The KMS action a producer performs on the queue's key: Amazon SQS generates the data
/// key the message is encrypted under, which is the grant "you must have the
/// `kms:GenerateDataKey` permission" names.
pub const GENERATE_DATA_KEY_ACTION: &str = "kms:GenerateDataKey";

/// The KMS action a consumer performs on the queue's key: the data key the message was
/// encrypted under has to be decrypted before the body can be handed over.
pub const DECRYPT_ACTION: &str = "kms:Decrypt";

/// The condition key naming the service a KMS request was made THROUGH, which is how a
/// key policy scopes a grant to one service's use of the key. Its value is the service's
/// regional endpoint name.
const VIA_SERVICE_CONDITION_KEY: &str = "kms:ViaService";

/// The condition key naming the account whose principal is making the KMS request.
const CALLER_ACCOUNT_CONDITION_KEY: &str = "kms:CallerAccount";

/// The encryption context entry Amazon SQS seals a queue's messages under: the queue's
/// own ARN, under the `aws:sqs:arn` key. A key policy may be conditioned on it, so the
/// decision carries it.
const QUEUE_ENCRYPTION_CONTEXT_KEY: &str = "kms:EncryptionContext:aws:sqs:arn";

/// The customer master key a queue's attribute set puts it on, or `None` for a queue
/// encrypted with an Amazon SQS owned key.
///
/// An empty value is no key: an empty `KmsMasterKeyId` is how a caller takes a queue off
/// its KMS key, so nothing downstream treats the empty string as a key id.
pub fn configured_master_key(attributes: &BTreeMap<String, String>) -> Option<&str> {
    attributes
        .get(KMS_MASTER_KEY_ID)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
}

/// Apply the exclusivity of the two encryption options to the attribute set a call would
/// leave a queue carrying.
///
/// `carried` is that set — the queue's attributes with what this call supplied overlaid —
/// and `supplied` is what the call itself named, which is what says WHICH option it
/// chose: an attribute the call did not name chooses nothing.
///
/// * a call naming a `KmsMasterKeyId` puts the queue on that key, which takes it OFF the
///   SQS-owned one — so the queue carries `SqsManagedSseEnabled` of `false`, and goes on
///   carrying it once it is off the KMS key too: a queue taken off the SQS-owned key is
///   not put back on it by dropping its KMS key;
/// * a call clearing `KmsMasterKeyId` (an empty value) takes the queue off its KMS key,
///   and with it the reuse period that key was used under;
/// * a call enabling `SqsManagedSseEnabled` puts the queue on the SQS-owned key, so it
///   takes the queue off any KMS key it was on, the reuse period included.
pub fn apply_encryption_choice(
    carried: &mut BTreeMap<String, String>,
    supplied: &HashMap<String, String>,
) {
    match supplied.get(KMS_MASTER_KEY_ID) {
        Some(key) if key.trim().is_empty() => {
            carried.remove(KMS_MASTER_KEY_ID);
            carried.remove(KMS_DATA_KEY_REUSE_PERIOD_SECONDS);
        }
        Some(_) => {
            carried.insert(SQS_MANAGED_SSE_ENABLED.to_string(), "false".to_string());
        }
        None => {
            if supplied
                .get(SQS_MANAGED_SSE_ENABLED)
                .is_some_and(|value| value == "true")
            {
                carried.remove(KMS_MASTER_KEY_ID);
                carried.remove(KMS_DATA_KEY_REUSE_PERIOD_SECONDS);
            }
        }
    }
}

/// Why a request naming BOTH encryption options cannot be served, or `None` when it names
/// at most one of them.
///
/// "Only one server-side encryption option is supported per queue (for example, SSE-KMS or
/// SSE-SQS)" (the `SqsManagedSseEnabled` entry of the `Attributes` documentation), so a
/// request that asks for a KMS key AND for the Amazon SQS owned key at once asks for
/// something no queue can be. Amazon SQS refuses it as an attribute NAME rather than an
/// attribute value: each value on its own is one the attribute accepts, and what the
/// request got wrong is naming the second attribute at all.
///
/// The sentence is [`BOTH_ENCRYPTION_OPTIONS_MESSAGE`]: Amazon SQS states the rule itself
/// rather than naming the attributes the request carried.
pub fn encryption_choice_refusal(
    supplied: &HashMap<String, String>,
) -> Option<InvalidAttributeName> {
    let names_a_key = supplied
        .get(KMS_MASTER_KEY_ID)
        .is_some_and(|key| !key.trim().is_empty());
    let enables_sqs_managed = supplied
        .get(SQS_MANAGED_SSE_ENABLED)
        .is_some_and(|value| value == "true");
    (names_a_key && enables_sqs_managed).then(|| InvalidAttributeName {
        message: Some(BOTH_ENCRYPTION_OPTIONS_MESSAGE.to_string()),
    })
}

/// How Amazon SQS words the refusal of a request that asks for both server-side
/// encryption options at once: the rule itself, in fixed text. Nothing of the request is
/// in it — not the attribute names, not the key the request named, not the queue — so the
/// sentence is the same for every such request, which is what lets a client tell this
/// `InvalidAttributeName` from the other causes that share the code.
pub const BOTH_ENCRYPTION_OPTIONS_MESSAGE: &str =
    "You can use one type of server-side encryption (SSE) at one time. You can either \
     enable KMS SSE or SQS SSE.";

/// Report the encryption a queue carrying `attributes` has, into the attribute map
/// `reported` a queue reader answers with.
///
/// A queue is always on one of the two options, so `SqsManagedSseEnabled` is always
/// reported: `true` for a queue on the Amazon SQS owned key, which is what a queue whose
/// create named no encryption attribute is on, and `false` for a queue something took off
/// it — a queue on a KMS key, and one that was on a KMS key and no longer is.
///
/// `KmsMasterKeyId` is reported only for a queue that is ON a KMS key, so a cleared key id
/// is not reported at all. The data key reuse period is the period such a key is used
/// under, so a queue on a KMS key reports it — the value the queue carries, else the
/// documented default — and a queue on no KMS key reports only a value it actually
/// carries, the attribute being one a caller may set either way.
pub fn report(attributes: &BTreeMap<String, String>, reported: &mut BTreeMap<String, String>) {
    match configured_master_key(attributes) {
        Some(key) => {
            reported.insert(KMS_MASTER_KEY_ID.to_string(), key.to_string());
            reported.insert(SQS_MANAGED_SSE_ENABLED.to_string(), "false".to_string());
            reported.insert(
                KMS_DATA_KEY_REUSE_PERIOD_SECONDS.to_string(),
                attributes
                    .get(KMS_DATA_KEY_REUSE_PERIOD_SECONDS)
                    .cloned()
                    .unwrap_or_else(|| DEFAULT_DATA_KEY_REUSE_PERIOD_SECONDS.to_string()),
            );
        }
        None => {
            reported.remove(KMS_MASTER_KEY_ID);
            let sse_sqs = attributes
                .get(SQS_MANAGED_SSE_ENABLED)
                .cloned()
                .unwrap_or_else(|| "true".to_string());
            reported.insert(SQS_MANAGED_SSE_ENABLED.to_string(), sse_sqs);
        }
    }
}

/// Why the queue's customer master key cannot be used for this call, in the KMS error
/// class each operation answers with. The payload is the message, as KMS answered it.
#[derive(Debug, Clone)]
pub enum KeyRefusal {
    /// The queue's `KmsMasterKeyId` resolves to no key — `KMS.NotFoundException`.
    NotFound(String),
    /// The key is disabled — `KMS.DisabledException`.
    Disabled(String),
    /// The key is in a state no cryptographic operation can use —
    /// `KMS.InvalidStateException`.
    InvalidState(String),
    /// The key cannot encrypt or decrypt at all — `KMS.InvalidKeyUsageException`.
    InvalidKeyUsage(String),
    /// The caller holds no permission for the KMS action this call performs —
    /// `KMS.AccessDeniedException`.
    AccessDenied(String),
}

/// The service name KMS's own error metadata carries.
const KMS_SERVICE_NAME: &str = "AWSKMS";

/// The status KMS answers each of these refusals with, which is also the status the
/// `Kms*` shapes this service relays them as carry.
const KMS_REFUSAL_STATUS: u16 = 400;

/// A refusal KMS answered, as the caller sees it: Amazon SQS does not word these refusals
/// itself, it RELAYS what its KMS call failed with, so the message is KMS's own sentence
/// followed by the metadata block an AWS error carries — the service that refused, the
/// status, the error code, and the id of the request that was refused.
///
/// `code` is KMS's own error code for the class, which is not the `KMS.*` code this
/// service answers with: the first names the shape KMS returned, the second the shape SQS
/// relays it as.
fn relayed(message: &str, code: &str) -> String {
    format!(
        "{message} (Service: {KMS_SERVICE_NAME}; Status Code: {KMS_REFUSAL_STATUS}; \
         Error Code: {code}; Request ID: {}; Proxy: null)",
        ::uuid::Uuid::new_v4()
    )
}

/// Whether the queue at `queue_url` may have its key used for `action` on behalf of this
/// request's caller — the step a send and a receive each run BEFORE they touch the
/// message plane, so a call the key does not permit changes nothing.
///
/// `Ok(())` for a url that addresses no queue and for a queue on the Amazon SQS owned
/// key: neither needs anything of KMS, and a url naming no queue is what the operation's
/// own `QueueDoesNotExist` reports, out of the store it goes on to read.
pub fn authorize_queue_key<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    queue_url: &str,
    action: &str,
) -> Result<(), KeyRefusal> {
    let Some(store) = crate::state::service_state(engine) else {
        return Ok(());
    };
    // The lock is released before KMS is consulted: nothing of this service's state is
    // read or written while the decision is being made.
    let configured = {
        let state = store.lock();
        state.queue_by_url(queue_url).and_then(|queue| {
            queue
                .master_key_id()
                .map(|key| (queue.arn.clone(), key.to_string()))
        })
    };
    let Some((queue_arn, master_key_id)) = configured else {
        return Ok(());
    };
    let region = crate::state::request_region(ctx.parts());
    authorize_master_key(engine, ctx, &region, &queue_arn, &master_key_id, action)
}

/// Whether the queue's customer master key may be used for `action` on behalf of this
/// request's caller, which is what a send (generating a data key) and a receive
/// (decrypting one) each need before they touch the message plane.
///
/// `master_key_id` is the queue's `KmsMasterKeyId` in any form KMS accepts — a key id, a
/// key ARN, an alias or an alias ARN — `region` the Region the request is signed for, and
/// `queue_arn` the queue whose messages the key seals, which is the encryption context a
/// conditioned key policy is decided against.
///
/// `Ok(())` for a queue whose key KMS can use for this caller, and for a lookup that says
/// nothing about the key (no KMS mounted beside this service). See the module docs for
/// the rule.
pub fn authorize_master_key<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    queue_arn: &str,
    master_key_id: &str,
    action: &str,
) -> Result<(), KeyRefusal> {
    let Some(kms) = kms_store::service_state(engine) else {
        // Visible to an operator: a lookup nothing answered is accepted, so without
        // this line it would be indistinguishable from a key KMS permitted.
        ::tracing::warn!(
            master_key_id,
            action,
            "no KMS service is mounted beside Amazon SQS; the queue's key is accepted"
        );
        return Ok(());
    };
    let key = {
        let mut store = kms.lock();
        let resolved = match store.resolve(region, master_key_id) {
            kms_store::KeyRef::Found(key) => Some(KeySnapshot::of(key)),
            // An identifier KMS cannot parse names no key either, and this service
            // models no "invalid ARN" refusal, so it is reported as not found — the
            // reading KMS's own `Encrypt`/`Decrypt` take of such an identifier.
            kms_store::KeyRef::NotFound | kms_store::KeyRef::MalformedArn => None,
            // An identifier whose account segment names no account is one KMS refuses
            // for want of permission BEFORE it looks any key up, so the queue's key
            // cannot be used for this call at all. The refusal is KMS's own sentence,
            // taken from the function that words it for every KMS operation, and relayed
            // like every other refusal in this module. That wording prefixes the IAM
            // action with the KMS namespace itself, so the action goes in without this
            // service's own `kms:` prefix.
            kms_store::KeyRef::UnauthorizedAccount => {
                return Err(KeyRefusal::AccessDenied(relayed(
                    &kms_store::unauthorized_account_message(
                        action.strip_prefix("kms:").unwrap_or(action),
                        master_key_id,
                    ),
                    "AccessDeniedException",
                )))
            }
        };
        match resolved {
            Some(key) => key,
            // "Amazon Web Services services use DescribeKey to create Amazon Web
            // Services managed keys from a predefined Amazon Web Services alias with no
            // key ID": a queue on `alias/aws/sqs` is on the Amazon Web Services managed
            // key that alias names, and the first use of it is what associates the two.
            None => match kms_store::alias_named_by(region, master_key_id).and_then(|alias| {
                store
                    .associate_predefined_aws_alias(region, &alias)
                    .map(KeySnapshot::of)
            }) {
                Some(key) => key,
                None => {
                    return Err(KeyRefusal::NotFound(relayed(
                        &kms_store::key_not_found_message(region, master_key_id),
                        "NotFoundException",
                    )))
                }
            },
        }
    };

    if key.state == kms_store::KEY_STATE_DISABLED {
        return Err(KeyRefusal::Disabled(relayed(
            &key.state_message,
            "DisabledException",
        )));
    }
    if !key.usable {
        return Err(KeyRefusal::InvalidState(relayed(
            &key.state_message,
            "KMSInvalidStateException",
        )));
    }
    // "For encrypting, decrypting, re-encrypting, and generating data keys, the KeyUsage
    // must be ENCRYPT_DECRYPT", and a data key is generated under a symmetric encryption
    // key, so a key that is neither cannot seal this queue's messages.
    if !key.encrypt_decrypt {
        return Err(KeyRefusal::InvalidKeyUsage(relayed(
            &format!(
                "{} cannot be used to encrypt or decrypt a message; its KeyUsage must be \
                 {} and its key spec a symmetric encryption spec.",
                key.arn,
                kms_store::KEY_USAGE_ENCRYPT_DECRYPT
            ),
            "InvalidKeyUsageException",
        )));
    }
    // An Amazon Web Services managed key's policy is managed by AWS for the service that
    // owns the alias, so the caller needs no grant of its own on it.
    if key.aws_managed {
        return Ok(());
    }

    let caller = caller(engine, ctx);
    let context = [
        (
            VIA_SERVICE_CONDITION_KEY.to_string(),
            format!("{}.{region}.amazonaws.com", crate::state::ARN_SERVICE),
        ),
        (
            CALLER_ACCOUNT_CONDITION_KEY.to_string(),
            caller.account.clone(),
        ),
        (
            QUEUE_ENCRYPTION_CONTEXT_KEY.to_string(),
            queue_arn.to_string(),
        ),
    ];
    match withheld_by(&key, &caller, action, &context) {
        None => Ok(()),
        // The sentence IAM gives for a principal that may not perform an action on a
        // resource: who asked, what for, on what, and which document withheld it.
        Some(reason) => Err(KeyRefusal::AccessDenied(relayed(
            &format!(
                "User: {} is not authorized to perform: {action} on resource: {} {reason}",
                caller.arn, key.arn
            ),
            "AccessDeniedException",
        ))),
    }
}

/// Which document withholds `action` on the key from `caller`, as the reason clause the
/// refusal states, or `None` when the key lets the call through.
fn withheld_by(
    key: &KeySnapshot,
    caller: &Caller,
    action: &str,
    context: &[(String, String)],
) -> Option<String> {
    let request = iam::Request::new(action, &key.arn, &caller.principals).with_context(context);
    // An explicit deny in the key's OWN policy is decided FIRST, and against the account
    // spellings beside the caller's own: a statement that denies the account denies each of
    // its principals, so a document stating one covers every caller of that account —
    // including its own published credential, which no grant exempts from a deny.
    let mut named = caller.principals.clone();
    named.extend(caller.account_principals.iter().cloned());
    let denying = iam::Request::new(action, &key.arn, &named).with_context(context);
    if iam::resource_decision(&key.policy, &denying) == Decision::Denied {
        return Some("with an explicit deny in a resource-based policy".to_string());
    }
    // The account's own published credential holds no IAM policies, and a principal
    // carrying a document this emulator cannot read may hold the grant: neither is judged
    // on a GRANT at all — the position the account's administrator is in — so neither is
    // refused on evidence nobody has.
    if caller.account_credential || caller.unreadable_attachment {
        return None;
    }
    if iam::resource_decision(&key.policy, &request) == Decision::Allowed {
        return None;
    }
    // "The default key policy ... gives the Amazon Web Services account that owns the KMS
    // key permission to use it in all cryptographic operations": a statement naming the
    // account root does not grant to a principal, it hands the decision to that account's
    // own identity-based policies. The root is spelled as the KEY's account, so a key
    // policy delegating to another account's IAM does not delegate to this caller's.
    let delegates = key.account_root().is_some_and(|root| {
        iam::resource_decision(
            &key.policy,
            &iam::Request::new(action, &key.arn, &root).with_context(context),
        ) == Decision::Allowed
    });
    if !delegates || caller.account != key.account {
        return Some(format!(
            "because no resource-based policy allows the {action} action"
        ));
    }
    match iam::identity_decision(&caller.documents, &request) {
        Decision::Allowed => None,
        Decision::Denied => {
            Some("with an explicit deny in an identity-based policy".to_string())
        }
        Decision::NotStated => Some(format!(
            "because no identity-based policy allows the {action} action"
        )),
    }
}

/// Everything a KMS access decision is made about the key itself, taken out of the KMS
/// store so the decision is made without its lock held.
#[derive(Debug, Clone)]
struct KeySnapshot {
    /// The key's ARN: the resource a policy statement's `Resource` names, and what a
    /// refusal names.
    arn: String,
    /// The account that owns the key.
    account: String,
    /// The key's `KeyState`.
    state: String,
    /// How KMS words a refusal caused by that key state.
    state_message: String,
    /// Whether the key can be used in a cryptographic operation at all right now.
    usable: bool,
    /// Whether the key is a symmetric encryption key, which is the only kind a data key
    /// is generated under.
    encrypt_decrypt: bool,
    /// Whether AWS manages the key, and therefore its policy.
    aws_managed: bool,
    /// The key's key policy document.
    policy: String,
}

impl KeySnapshot {
    fn of(key: &kms_store::Cmk) -> Self {
        Self {
            arn: key.arn.clone(),
            account: key.aws_account_id.clone(),
            state: key.key_state.clone(),
            state_message: kms_store::invalid_state_message(key),
            usable: key.is_usable(),
            encrypt_decrypt: key.key_usage == kms_store::KEY_USAGE_ENCRYPT_DECRYPT
                && key.is_symmetric_encryption(),
            aws_managed: key.key_manager != kms_store::KEY_MANAGER_CUSTOMER,
            policy: key.policy.clone(),
        }
    }

    /// The principal spellings a key policy delegating to IAM names: the owning
    /// account's root user, and the account id, which IAM reads as that same root.
    fn account_root(&self) -> Option<Vec<String>> {
        let partition = ::service_common::arn::parse(&self.arn)?.partition.to_string();
        Some(vec![
            format!("arn:{partition}:iam::{}:root", self.account),
            self.account.clone(),
        ])
    }
}

/// The principal a request speaks for, with everything the decision is made from: the ONE
/// resolution this crate has ([`super::authorization::caller`]), which reads the same three
/// sources every service in this tree that authorizes reads — STS's own caller resolver for
/// a session credential, the IAM store's access key map for a long-term key, and the
/// account's own published credential for a key neither holds.
use super::authorization::{caller, Caller};

