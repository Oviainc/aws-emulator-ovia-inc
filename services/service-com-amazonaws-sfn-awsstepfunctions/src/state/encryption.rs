// SPDX-License-Identifier: CC0-1.0

//! Whether the CALLER may have a state machine's customer managed KMS key used on
//! its behalf, which is what reading data that key encrypts — and sealing data under
//! it — needs.
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

/// The status KMS answers an authorization refusal with, which is also the status the
/// `Kms*` shapes this service relays it as carry.
const KMS_REFUSAL_STATUS: u16 = 400;

/// How many attempts the refused KMS call took, which the relayed metadata reports
/// last: an authorization refusal is a condition retrying cannot change, so the call
/// that was refused was made once and not again.
const KMS_ATTEMPTS: u32 = 1;

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

/// Why the data of the execution `execution_arn` identifies may not be read by this
/// request's caller, or `None` when it may be.
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
) -> Option<String> {
    execution_key_denied(engine, ctx, region, execution_arn, READ_OF_SEALED_DATA)
}

/// Why this request's caller may not have data of its own SEALED into the execution
/// `execution_arn` identifies, or `None` when it may be.
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
) -> Option<String> {
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
) -> Option<String> {
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

/// Why this request's caller may not have data of its own SEALED into the execution
/// the activity task `token` belongs to, or `None` when it may be.
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
) -> Option<String> {
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

/// Why the data of the state machine `state_machine_arn` identifies may not be read
/// by this request's caller, or `None` when it may be.
///
/// `None` for an ARN that addresses no state machine and no version, which is the
/// operation's own `StateMachineDoesNotExist`.
pub fn state_machine_read_denied<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    state_machine_arn: &str,
) -> Option<String> {
    let state = super::service_state(engine)?;
    // As above, the guard is dropped before the decision is made.
    let key = {
        let store = state.lock();
        state_machine_key(&store, state_machine_arn)?
    };
    key_denied(engine, ctx, region, &key, READ_OF_SEALED_DATA)
}

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

/// Why this request's caller may not have `key` used for it, as KMS words that
/// refusal, or `None` when the authorities in force let the call through.
///
/// `key` is the state machine's `kmsKeyId` in any form KMS accepts — a key id, a key
/// ARN, an alias or an alias ARN — and `region` the Region the request is signed for,
/// since a KMS key is a regional resource. `operations` are the KMS operations the
/// call makes on the key; the FIRST one the authorities refuse is what the caller is
/// told, since one refused operation is already enough to stop the call.
fn key_denied<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    key: &str,
    operations: &[&str],
) -> Option<String> {
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
        .map(|why| relayed(&why))
    })
}
