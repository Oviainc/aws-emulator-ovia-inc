// SPDX-License-Identifier: CC0-1.0

//! Whether a state machine's customer managed KMS key can be used for this call —
//! which is what reading data that key encrypts, and sealing data under it, needs.
//!
//! Two things decide it, in the order KMS itself decides them for a cryptographic
//! operation: the key's own KEY STATE, and then whether the authorities in force let
//! the CALLER have the key used on its behalf. Each answers the error class the
//! operations model for it ([`KeyRefusal`]).
//!
//! A state machine on `encryptionConfiguration` of
//! [`ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY`](super::ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY)
//! has its execution data and its definition encrypted under the key the account
//! named. Step Functions does not hold that key: it asks KMS to use it as the CALLER,
//! so a caller the key does not permit cannot have the data decrypted — or encrypted
//! — for it, and the call is refused with the KMS error class the operation models
//! (`KmsAccessDeniedException`, HTTP 400) — which is why every one of those
//! operations declares the `Kms*` family.
//!
//! # The key's state
//!
//! A key only some of whose key states admit cryptographic use of it cannot seal or
//! unseal anything while it is in one of the others — disabled, awaiting deletion,
//! awaiting imported key material, still being created, unavailable — so every call
//! that would use it is refused with `KmsInvalidStateException` (HTTP 400) for as long
//! as that lasts, carrying the key state the caller has to change as the `kmsKeyState`
//! member reports it. Nothing latches: the decision is taken against the key record
//! KMS holds at the instant of THIS call, so a key its owner puts back into a usable
//! state serves the very next call.
//!
//! No plaintext data key outlives a call here, so every call that needs the key
//! consults it — which `kmsDataKeyReusePeriodSeconds` permits: it is the "maximum
//! duration that Step Functions will reuse data keys"
//! (`smithy_query(query="shape",shape="EncryptionConfiguration")`), an upper bound on
//! how long a data key MAY serve without KMS being asked again, not a period during
//! which it must be reused.
//!
//! # The caller
//!
//! A READ asks for `kms:Decrypt`, the permission the `includedData` member documents
//! ("callers must have kms:Decrypt permission to decrypt the definition"). A WRITE of
//! data the caller supplied asks for the data key the payload is sealed with: "when
//! the period expires, Step Functions will call `GenerateDataKey`"
//! (`kmsDataKeyReusePeriodSeconds`), and a data key still inside that period is
//! RE-USED, which means decrypting the one already minted — so a caller allowed
//! neither cannot have its data sealed. A request that carries no such data seals
//! nothing and so asks nothing of the key.
//!
//! Two approximations are deliberate here, so a later reader does not take them for
//! the service's exact rule. Which of the two calls a given write makes depends on
//! how long ago a data key was minted, which no caller chooses, so a write asks for
//! both rather than for whichever one this instant would have made. And the decision
//! is taken against the identity that signed the REQUEST: the emulator has no
//! execution role to assume, so the caller stands in for it.
//!
//! The key is resolved in the KMS service's OWN store and the decision is KMS's own
//! ([`kms_authorization::authorize`]), so this service reads exactly what KMS's
//! handlers wrote and the two cannot disagree about a key's policy: a statement a
//! caller removes stops granting from the very next call, and one it puts back grants
//! again, because nothing here is cached.
//!
//! An Amazon Web Services owned key — what a state machine created without an
//! encryption configuration is on — needs no grant of the caller's: AWS manages that
//! key's policy for the service, so there is nothing to decide and the read proceeds.
//! The same is true when no KMS service is mounted beside this one, or when the
//! identifier the state machine carries names no key KMS holds: a condition nobody
//! attributed to the key never refuses a read.
//!
//! Resolving the caller materialises the session expiries the clock has already made
//! due — a write owed at that same instant whether or not anybody called — which is
//! why a REPORTING operation may make this decision: what a later caller sees is
//! unchanged by the read itself.

use ::service_com_amazonaws_kms_trentservice::state as kms_store;
use ::service_com_amazonaws_kms_trentservice::state::authorization as kms_authorization;

/// The KMS operation a read of data the key encrypts performs: the data was sealed
/// under that key, so reporting it means decrypting it.
const DECRYPT_OPERATION: &str = "Decrypt";

/// The KMS operation that MINTS the data key a payload is sealed with.
const GENERATE_DATA_KEY_OPERATION: &str = "GenerateDataKey";

/// What reporting data the key sealed asks of that key.
const READ_OF_SEALED_DATA: &[&str] = &[DECRYPT_OPERATION];

/// What sealing caller-supplied data under the key asks of it: a data key is minted
/// for the payload, and one still inside the reuse period the configuration names is
/// re-used instead, which decrypts it. A caller is allowed to have data sealed only
/// when it is allowed both, since which of the two a given call makes is not the
/// caller's to choose.
const WRITE_OF_SEALED_DATA: &[&str] = &[GENERATE_DATA_KEY_OPERATION, DECRYPT_OPERATION];

/// The service name KMS's own error metadata carries.
const KMS_SERVICE_NAME: &str = "Kms";

/// The status KMS answers each of these refusals with, which is also the status the
/// `Kms*` shapes this service relays them as carry.
const KMS_REFUSAL_STATUS: u16 = 400;

/// How many attempts the refused KMS call took, which the relayed metadata reports
/// last: a key state and an authorization decision are both conditions retrying cannot
/// change, so the call that was refused was made once and not again.
const KMS_ATTEMPTS: u32 = 1;

/// Why a state machine's customer managed key cannot be used for a call, in the
/// modelled error class the operations answer it with. The payload is the message, as
/// KMS worded it and this service relays it.
#[derive(Debug, Clone)]
pub enum KeyRefusal {
    /// The key's own key state admits no cryptographic use of it —
    /// `KmsInvalidStateException`, carrying that key state.
    InvalidState {
        /// The relayed sentence KMS refused the call with.
        message: String,
        /// The key state the caller has to change, as `KmsKeyState` spells it, and
        /// `None` for a state that enum does not name.
        key_state: Option<String>,
    },
    /// The authorities in force do not let this caller have the key used on its behalf
    /// — `KmsAccessDeniedException`.
    AccessDenied(String),
}

/// The refusal one of this service's operations answers a [`KeyRefusal`] with, in that
/// operation's OWN error channel.
///
/// Each operation has its own generated error enum, so the mapping from a refusal to
/// the modelled error cannot be a function over one type; it is this macro instead,
/// expanded once per handler, so no two of them word or classify the same refusal
/// differently. `$modeled` is the handler's `{Op}ErrorModeled` enum, every one of which
/// declares both of these errors.
#[macro_export]
macro_rules! kms_key_refusal {
    ($modeled:ident, $refusal:expr) => {{
        match $refusal {
            $crate::state::encryption::KeyRefusal::InvalidState { message, key_state } => {
                $modeled::KmsInvalidStateException($crate::ops::types::KmsInvalidStateException {
                    kms_key_state: key_state,
                    message: Some(message),
                })
            }
            $crate::state::encryption::KeyRefusal::AccessDenied(message) => {
                $modeled::KmsAccessDeniedException($crate::ops::types::KmsAccessDeniedException {
                    message: Some(message),
                })
            }
        }
    }};
}

/// A refusal KMS answered, as the caller sees it: Step Functions does not word these
/// refusals itself, it RELAYS what its KMS call failed with, so the message is KMS's
/// own sentence followed by the metadata an AWS error carries — the service that
/// refused, the status and the id of the request that was refused, then how many
/// attempts it took.
fn relayed(message: &str) -> String {
    format!(
        "{message} (Service: {KMS_SERVICE_NAME}, Status Code: {KMS_REFUSAL_STATUS}, \
         Request ID: {}) (SDK Attempt Count: {KMS_ATTEMPTS})",
        ::service_common::query_error_envelope::new_request_id()
    )
}

/// The customer managed key `configuration` puts a resource on, or `None` when the
/// resource is encrypted with an Amazon Web Services owned key.
///
/// Only [`ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY`](super::ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY)
/// carries a `kmsKeyId` (`super::invalid_encryption_reason` refuses one under the
/// other type), so a configuration of any other type names no key for this gate to
/// consult.
fn customer_managed_key(
    configuration: Option<&crate::ops::types::EncryptionConfiguration>,
) -> Option<&str> {
    let configuration = configuration?;
    if configuration.r#type != super::ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY {
        return None;
    }
    configuration
        .kms_key_id
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty())
}

/// Why the data of the execution `execution_arn` identifies cannot be read for this
/// request, or `None` when it can be.
///
/// The key is the one the execution's STATE MACHINE is encrypted under, which is what
/// its execution data and its definition were sealed with.
///
/// `None` for an ARN that addresses no execution: that is the operation's own
/// `ExecutionDoesNotExist`, reported out of the store it goes on to read.
pub fn execution_read_denied<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    execution_arn: &str,
) -> Option<KeyRefusal> {
    execution_key_denied(engine, ctx, region, execution_arn, READ_OF_SEALED_DATA)
}

/// Why data of this request's own cannot be SEALED into the execution `execution_arn`
/// identifies, or `None` when it can be.
///
/// The key is the one the execution's STATE MACHINE is encrypted under, which is what
/// everything the execution retains is sealed with, so the data this call records
/// goes under that same key.
///
/// `None` for an ARN that addresses no execution: that is the operation's own
/// `ExecutionDoesNotExist`, reported out of the store it goes on to write.
pub fn execution_write_denied<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    execution_arn: &str,
) -> Option<KeyRefusal> {
    execution_key_denied(engine, ctx, region, execution_arn, WRITE_OF_SEALED_DATA)
}

/// The one resolution behind both of the above: the key the execution's data lives
/// under, judged for the `operations` this call makes on it.
fn execution_key_denied<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    execution_arn: &str,
    operations: &[&str],
) -> Option<KeyRefusal> {
    let state = super::service_state(engine)?;
    // The lock is released before KMS is consulted: nothing of this service's state
    // is read or written while the decision is being made.
    let key = {
        let store = state.lock();
        let machine_arn = store.executions.get(execution_arn)?.state_machine_arn.clone();
        state_machine_key(&store, &machine_arn)?
    };
    key_denied(engine, ctx, region, &key, operations)
}

/// Why data of this request's own cannot be SEALED into the execution the activity
/// task `token` belongs to, or `None` when it can be.
///
/// A task token names its task and its execution on its own, so a reply to one is
/// judged against the key of the execution the task was scheduled by — in that
/// execution's own Region, since a KMS key is a regional resource.
///
/// `None` for a token the store holds no outstanding task under: that is the
/// operation's own refusal about a task that has been closed.
pub fn task_reply_write_denied<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    token: &str,
) -> Option<KeyRefusal> {
    let state = super::service_state(engine)?;
    // ONE acquisition resolves the whole chain — the task's execution, that
    // execution's state machine, and the key it is encrypted under — so the two reads
    // cannot disagree, and the guard is released before KMS is consulted.
    let (execution_arn, key) = {
        let store = state.lock();
        let execution_arn = store.activity_tasks.get(token)?.execution_arn.clone();
        let machine_arn = store.executions.get(&execution_arn)?.state_machine_arn.clone();
        (execution_arn, state_machine_key(&store, &machine_arn)?)
    };
    let region = super::arn_region(&execution_arn);
    key_denied(engine, ctx, &region, &key, WRITE_OF_SEALED_DATA)
}

/// Why the data of the state machine `state_machine_arn` identifies cannot be read for
/// this request, or `None` when it can be.
///
/// `None` for an ARN that addresses no state machine and no version, which is the
/// operation's own `StateMachineDoesNotExist`.
pub fn state_machine_read_denied<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    state_machine_arn: &str,
) -> Option<KeyRefusal> {
    let state = super::service_state(engine)?;
    // As above, the guard is dropped before the decision is made.
    let key = {
        let store = state.lock();
        state_machine_key(&store, state_machine_arn)?
    };
    key_denied(engine, ctx, region, &key, READ_OF_SEALED_DATA)
}

/// Why an execution of the state machine `state_machine_arn` identifies cannot be
/// STARTED under its customer managed key, or `None` when it can be.
///
/// Starting an execution seals the execution's data under the state machine's key, and
/// Step Functions does that AS THE STATE MACHINE'S EXECUTION ROLE: "the IAM role to
/// use for this state machine" is the identity the service acts under for the run, and
/// `kmsDataKeyReusePeriodSeconds` names the call that sealing makes ("when the period
/// expires, Step Functions will call `GenerateDataKey`"). So the decision here is
/// about the ROLE rather than about the caller: a role whose identity-based policies do
/// not allow that call on the key cannot have the execution's data sealed, and the
/// start is refused with the modelled `KmsAccessDeniedException` — "either your KMS key
/// policy or API caller does not have the required permissions" — having started
/// nothing.
///
/// The KEY STATE is deliberately not decided here: it is a condition of the key rather
/// than of an identity, and it is decided by the calls that read or write the sealed
/// data ([`key_denied`]), which is where this crate decides it for every operation
/// that touches that data. One question, one place.
///
/// `None` wherever nothing of the kind can be shown: a state machine on an Amazon Web
/// Services owned key (nothing of any identity's is asked of such a key), an engine
/// carrying no KMS or no IAM store, an identifier naming no key KMS holds, and a role
/// IAM holds no record for — each is evidence of nothing, so the start proceeds. Call
/// it with no store of this service locked.
pub fn execution_start_denied<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    state_machine_arn: &str,
    role_arn: &str,
) -> Option<KeyRefusal> {
    let state = super::service_state(engine)?;
    // The guard is released before KMS and IAM are consulted: no store of another
    // service is reached while this one's is held.
    let key = {
        let store = state.lock();
        state_machine_key(&store, state_machine_arn)?
    };
    let kms = kms_store::service_state(engine)?;
    // What the key itself answers, read under the KMS guard and nothing else: the
    // guard is released before the role's documents are read.
    let (customer_managed, key_arn) = {
        let store = kms.lock();
        let kms_store::KeyRef::Found(found) = store.resolve(region, &key) else {
            return None;
        };
        (
            found.key_manager == kms_store::KEY_MANAGER_CUSTOMER,
            found.arn.clone(),
        )
    };
    // An Amazon Web Services managed key's policy is managed by AWS for the service
    // that owns it, so no identity needs a grant of its own on it.
    if !customer_managed {
        return None;
    }
    super::task_authorization::role_denial(
        engine,
        role_arn,
        GENERATE_DATA_KEY_ACTION,
        &key_arn,
        KMS_SERVICE_NAME,
    )
    .map(|why| KeyRefusal::AccessDenied(relayed(&why)))
}

/// The IAM action minting the data key a payload is sealed with: the KMS operation
/// `kmsDataKeyReusePeriodSeconds` names, under the `kms` action namespace a policy
/// grants it in.
const GENERATE_DATA_KEY_ACTION: &str = "kms:GenerateDataKey";

/// The customer managed key the state machine `arn` names is encrypted under.
///
/// A version is its own stored record carrying its own encryption configuration, and
/// a version-qualified ARN names that record, so both maps are read: the version the
/// ARN names, else the state machine itself.
fn state_machine_key(
    store: &super::ComAmazonawsSfnAwsstepfunctionsState,
    arn: &str,
) -> Option<String> {
    let configuration = match store.state_machine_versions.get(arn) {
        Some(version) => version.encryption_configuration.as_ref(),
        None => store
            .state_machines
            .get(arn)?
            .encryption_configuration
            .as_ref(),
    };
    customer_managed_key(configuration).map(str::to_string)
}

/// Why `key` cannot be used for this request, as KMS words that refusal, or `None`
/// when the key and the authorities in force let the call through.
///
/// `key` is the state machine's `kmsKeyId` in any form KMS accepts — a key id, a key
/// ARN, an alias or an alias ARN — and `region` the Region the request is signed for,
/// since a KMS key is a regional resource. `operations` are the KMS operations the
/// call makes on the key; the FIRST one the authorities refuse is what the caller is
/// told, since one refused operation is already enough to stop the call.
///
/// The key's own key state is decided FIRST, the order KMS itself decides it in for a
/// cryptographic operation: a key no cryptographic operation can use refuses every
/// caller, whatever any policy says and whoever manages the key.
fn key_denied<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    key: &str,
    operations: &[&str],
) -> Option<KeyRefusal> {
    let Some(kms) = kms_store::service_state(engine) else {
        // Visible to an operator: a lookup nothing answered lets the call through, so
        // without this line it would be indistinguishable from a key KMS permitted.
        ::tracing::warn!(
            key,
            "no KMS service is mounted beside Step Functions; the state machine's key is accepted"
        );
        return None;
    };
    // The caller is resolved BEFORE the KMS store is locked: the STS and IAM stores it
    // reads are never reached while another service's guard is held.
    let caller = kms_authorization::caller(engine, ctx);
    let store = kms.lock();
    let kms_store::KeyRef::Found(found) = store.resolve(region, key) else {
        return None;
    };
    // Neither sealing data under the key nor unsealing data it sealed is something the
    // key can do while its key state admits no cryptographic use of it, so the call is
    // refused for as long as that key state lasts — and served again from the first
    // call after it changes, since nothing of this decision is kept.
    if !found.is_usable() {
        return Some(KeyRefusal::InvalidState {
            message: relayed(&kms_store::invalid_state_message(found)),
            key_state: modelled_key_state(found.reported_key_state()),
        });
    }
    // An Amazon Web Services managed key's policy is managed by AWS for the service
    // that owns it, so the caller needs no grant of its own on it.
    if found.key_manager != kms_store::KEY_MANAGER_CUSTOMER {
        return None;
    }
    operations.iter().find_map(|operation| {
        kms_authorization::authorize(
            &caller,
            &store,
            region,
            found,
            operation,
            // No encryption context is presented, so a grant constrained to one does
            // not permit this call: the authority the grant carries is not claimed
            // rather than assumed.
            None,
            None,
        )
        .err()
        .map(|why| KeyRefusal::AccessDenied(relayed(&why)))
    })
}

/// A KMS `KeyState` as the `kmsKeyState` member of `KmsInvalidStateException` spells
/// it, or `None` for a key state that member's enum does not name.
///
/// The two models word the same condition differently — KMS's `KeyState` values are
/// camel case (`PendingDeletion`) and this service's `KmsKeyState` values are upper
/// snake case (`PENDING_DELETION`) — so each word boundary becomes an underscore,
/// which converts a value this slice does not reach yet without a case for it. A state
/// outside the enum is reported as no state rather than as a value the member does not
/// accept: the relayed message still says what the key is in.
fn modelled_key_state(key_state: &str) -> Option<String> {
    let mut modelled = String::with_capacity(key_state.len() + 2);
    for (index, character) in key_state.char_indices() {
        if character.is_ascii_uppercase() && index != 0 {
            modelled.push('_');
        }
        modelled.extend(character.to_uppercase());
    }
    crate::ops::types::KMS_KEY_STATE_VALUES
        .contains(&modelled.as_str())
        .then_some(modelled)
}
