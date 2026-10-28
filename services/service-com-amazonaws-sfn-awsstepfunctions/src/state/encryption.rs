// SPDX-License-Identifier: CC0-1.0

//! Whether the CALLER may have a state machine's customer managed KMS key used on
//! its behalf, which is what a read of data that key encrypts needs.
//!
//! A state machine on `encryptionConfiguration` of
//! [`ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY`](super::ENCRYPTION_TYPE_CUSTOMER_MANAGED_KMS_KEY)
//! has its execution data and its definition encrypted under the key the account
//! named. Step Functions does not hold that key: it asks KMS to decrypt the data as
//! the CALLER, so a caller the key does not permit cannot have the data decrypted for
//! it and the read is refused with the KMS error class the operation models
//! (`KmsAccessDeniedException`, HTTP 400) — which is why every one of those
//! operations declares the `Kms*` family. The grant is `kms:Decrypt`, the permission
//! the `includedData` member documents ("callers must have kms:Decrypt permission to
//! decrypt the definition").
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

/// The service name KMS's own error metadata carries.
const KMS_SERVICE_NAME: &str = "AWSKMS";

/// The status KMS answers an authorization refusal with, which is also the status the
/// `Kms*` shapes this service relays it as carry.
const KMS_REFUSAL_STATUS: u16 = 400;

/// The error code KMS's own authorization refusal carries, which is not the code this
/// service answers with: the first names the shape KMS returned, the second the shape
/// Step Functions relays it as.
const KMS_ACCESS_DENIED_CODE: &str = "AccessDeniedException";

/// A refusal KMS answered, as the caller sees it: Step Functions does not word these
/// refusals itself, it RELAYS what its KMS call failed with, so the message is KMS's
/// own sentence followed by the metadata block an AWS error carries — the service that
/// refused, the status, the error code, and the id of the request that was refused.
fn relayed(message: &str) -> String {
    format!(
        "{message} (Service: {KMS_SERVICE_NAME}; Status Code: {KMS_REFUSAL_STATUS}; \
         Error Code: {KMS_ACCESS_DENIED_CODE}; Request ID: {}; Proxy: null)",
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
    let state = super::service_state(engine)?;
    // The lock is released before KMS is consulted: nothing of this service's state
    // is read or written while the decision is being made.
    let key = {
        let store = state.lock();
        let machine_arn = store.executions.get(execution_arn)?.state_machine_arn.clone();
        state_machine_key(&store, &machine_arn)?
    };
    key_read_denied(engine, ctx, region, &key)
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
    key_read_denied(engine, ctx, region, &key)
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

/// Why this request's caller may not have `key` used to decrypt data for it, as KMS
/// words that refusal, or `None` when the authorities in force let the call through.
///
/// `key` is the state machine's `kmsKeyId` in any form KMS accepts — a key id, a key
/// ARN, an alias or an alias ARN — and `region` the Region the request is signed for,
/// since a KMS key is a regional resource.
fn key_read_denied<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    key: &str,
) -> Option<String> {
    let Some(kms) = kms_store::service_state(engine) else {
        // Visible to an operator: a lookup nothing answered lets the read through, so
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
    kms_authorization::authorize(
        &caller,
        &store,
        region,
        found,
        DECRYPT_OPERATION,
        // No encryption context is presented, so a grant constrained to one does not
        // permit this call: the authority the grant carries is not claimed rather than
        // assumed.
        None,
        None,
    )
    .err()
    .map(|why| relayed(&why))
}
