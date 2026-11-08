// SPDX-License-Identifier: CC0-1.0

//! A table's server-side encryption, and the KMS seam that resolves the key it names.
//!
//! "Indicates whether server-side encryption is done using an Amazon Web Services managed
//! key or an Amazon Web Services owned key. If enabled (true), server-side encryption type
//! is set to `KMS` ... If disabled (false) or not specified, server-side encryption is set
//! to Amazon Web Services owned key" (`SSESpecification$Enabled`). The owned key is the
//! default and has no description of its own, so a table encrypted with it carries no
//! record here at all and reports no `SSEDescription`; a table encrypted with a KMS key
//! carries one, and what it reports is the key ARN — "The KMS key ARN used for the KMS
//! encryption" (`SSEDescription$KMSMasterKeyArn`).
//!
//! The ARN is the KMS SERVICE's: a request names its key "by its key ID, Amazon Resource
//! Name (ARN), alias name, or alias ARN" (`SSESpecification$KMSMasterKeyId`), and only KMS
//! can say which key an alias stands for. So this module reaches the KMS store on the same
//! shared engine, exactly as [`super::streams_seam`] reaches the DynamoDB Streams
//! registry, rather than inventing a key ARN of its own.

use std::time::SystemTime;

use crate::ops::types::SSEDescription;

/// The default DynamoDB encryption key, which a request that names no key asks for:
/// "you should only provide this parameter if the key is different from the default
/// DynamoDB KMS key `alias/aws/dynamodb`"
/// (`SSESpecification$KMSMasterKeyId`).
pub const DEFAULT_SSE_KEY_ALIAS: &str = "alias/aws/dynamodb";

/// The one server-side encryption type the model offers: "The only supported value is:
/// `KMS` - Server-side encryption that uses Key Management Service"
/// (`SSESpecification$SSEType`).
pub const SSE_TYPE_KMS: &str = "KMS";

/// A table's server-side encryption with a KMS key: the key in force and when the change
/// that put it in force becomes effective.
///
/// Absent from a table encrypted with the Amazon Web Services OWNED key, which is the
/// default and the state a specification that disables encryption asks for.
#[derive(Debug, Clone)]
pub struct TableEncryption {
    /// The key ARN KMS resolved for the identifier the request named
    /// ([`resolve_sse_key`]), reported as `SSEDescription$KMSMasterKeyArn`.
    pub key_arn: String,
    /// When the clock makes this encryption change due. "`UPDATING` - Server-side
    /// encryption is being updated" (`SSEDescription$Status`) until then, and `ENABLED`
    /// from then on.
    pub enabled_at: SystemTime,
}

impl TableEncryption {
    /// This encryption as `SSEDescription` reports it: the status the clock owes, the one
    /// supported type, and the resolved key ARN.
    pub fn describe(&self, now: SystemTime) -> SSEDescription {
        SSEDescription {
            status: Some(self.status(now).to_string()),
            sse_type: Some(SSE_TYPE_KMS.to_string()),
            kms_master_key_arn: Some(self.key_arn.clone()),
            // "when DynamoDB detected that the table's KMS key was inaccessible"
            // (`SSEDescription$InaccessibleEncryptionDateTime`): the instant of a
            // DETECTION, which only a call that reached the key could have made. A
            // description reports what is recorded and records nothing itself, and no
            // call records one, so there is no such instant to report. The refusal a
            // request on an unusable key meets is [`check_table_key_accessible`].
            inaccessible_encryption_date_time: None,
        }
    }

    /// Where this encryption change stands at `now`.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.enabled_at {
            "UPDATING"
        } else {
            "ENABLED"
        }
    }
}

/// Why a table's encryption key could not be resolved.
#[derive(Debug, Clone)]
pub enum SseFault {
    /// The KMS store cannot be reached at all (the engine is not the shared in-memory
    /// one), which a handler answers with its modelled SERVER error rather than with a
    /// table reporting a key ARN nothing minted.
    StoreUnavailable,
    /// The specification cannot be served as sent — a key this Region's KMS endpoint does
    /// not serve, or an encryption type the service does not offer — which a handler
    /// answers with the 400 the bad request it is.
    Invalid(super::RequestError),
}

/// The encryption a table carries after a request that may have named an
/// `SSESpecification`: the key KMS resolved for it, or `None` for the Amazon Web Services
/// owned key a specification that does not enable encryption asks for.
///
/// "If enabled (true), server-side encryption type is set to `KMS` and an Amazon Web
/// Services managed key is used ... If disabled (false) or not specified, server-side
/// encryption is set to Amazon Web Services owned key" (`SSESpecification$Enabled`): the
/// two halves of that sentence are the two arms here, and a request that names no
/// specification at all leaves `in_force` exactly as it is.
///
/// `at` is the instant the change becomes effective: the creating call's own instant for a
/// table being created (its encryption is in force from its first instant), and a span
/// later for a change to a table that already exists, which reports `UPDATING` until then.
pub fn requested_encryption<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    specification: Option<&crate::ops::types::SSESpecification>,
    in_force: Option<&TableEncryption>,
    at: SystemTime,
) -> Result<Option<TableEncryption>, SseFault> {
    let Some(specification) = specification else {
        return Ok(in_force.cloned());
    };
    if let Some(sse_type) = specification.sse_type.as_deref() {
        if sse_type != SSE_TYPE_KMS {
            return Err(SseFault::Invalid(unsupported_sse_type(sse_type)));
        }
    }
    if !specification.enabled.unwrap_or(false) {
        return Ok(None);
    }
    let key_arn = resolve_sse_key(engine, region, specification.kms_master_key_id.as_deref())?;
    Ok(Some(TableEncryption {
        key_arn,
        enabled_at: at,
    }))
}

/// The key ARN a table's `SSESpecification` resolves to, answered by KMS.
///
/// `key_id` is the `KMSMasterKeyId` the request named, in any of the forms KMS accepts
/// (key id, key ARN, alias name, alias ARN); a request that names none asks for the
/// default DynamoDB key ([`DEFAULT_SSE_KEY_ALIAS`]), which KMS associates with an Amazon
/// Web Services managed key the first time it is asked for — "if you specify a predefined
/// Amazon Web Services alias (an Amazon Web Services alias with no key ID), KMS associates
/// the alias with an Amazon Web Services managed key" — so the same alias resolves to the
/// same key for every later read.
///
/// A key KMS will not USE is refused here rather than resolved ([`usable_key_arn`]): a
/// table may only be associated with a key its items can be encrypted under.
///
/// LOCKING: this takes the KMS store's lock, and a handler calls it while holding this
/// service's own. No KMS operation reaches into the DynamoDB store, so the two cannot
/// deadlock, and the work done under both is a single map lookup.
pub fn resolve_sse_key<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    key_id: Option<&str>,
) -> Result<String, SseFault> {
    use ::service_com_amazonaws_kms_trentservice as kms;

    let store = kms::state::service_state(engine).ok_or(SseFault::StoreUnavailable)?;
    let mut store = store.lock();
    let named = key_id.unwrap_or(DEFAULT_SSE_KEY_ALIAS);
    // A predefined Amazon Web Services alias exists in every Region while carrying no key
    // until something asks for it, so it is resolved through the association rather than
    // through the plain lookup — which is what makes the default key resolvable on a
    // store no KMS caller has written to.
    if kms::state::is_predefined_aws_alias(named) {
        let key = store
            .associate_predefined_aws_alias(region, named)
            .ok_or_else(|| SseFault::Invalid(unknown_sse_key(named)))?;
        return usable_key_arn(key);
    }
    match store.resolve(region, named) {
        kms::state::KeyRef::Found(key) => usable_key_arn(key),
        _ => Err(SseFault::Invalid(unknown_sse_key(named))),
    }
}

/// The ARN of a key a table may be encrypted with, which is a key KMS will USE: a key in
/// a key state that refuses cryptographic operations encrypts nothing, so associating a
/// table with it would report a table encrypted under a key no write could reach
/// ([`unusable_key_fault`]).
fn usable_key_arn(
    key: &::service_com_amazonaws_kms_trentservice::state::Cmk,
) -> Result<String, SseFault> {
    if key.is_usable() {
        Ok(key.arn.clone())
    } else {
        Err(SseFault::Invalid(unusable_key_fault(key)))
    }
}

/// Whether the KMS key a table is encrypted with can serve this request, and the refusal
/// it meets when it cannot.
///
/// DynamoDB encrypts and decrypts a table's items under the KMS key the table names, so a
/// request on a table whose key KMS will not use is refused rather than served: the fault
/// is the KEY's, which is why it is reported as a bad request that names the KMS exception
/// the attempt raised ([`wrapped_kms_fault`]) instead of as a fault of this service.
/// `SSEDescription$InaccessibleEncryptionDateTime` — "the time ... when DynamoDB detected
/// that the table's KMS key was inaccessible" — is the member that records DynamoDB
/// noticing exactly this.
///
/// A table encrypted with the Amazon Web Services owned key carries no record here and is
/// never refused: it names no customer key whose state a caller could change. Neither is a
/// table whose key cannot be looked up because this process holds no KMS store at all
/// (the engine is not the shared in-memory one) — there is no key state to consult, and a
/// refusal would report a fault nothing observed.
///
/// LOCKING: as in [`resolve_sse_key`], this takes the KMS store's lock while a handler
/// holds this service's own, which cannot deadlock because no KMS operation reaches into
/// the DynamoDB store.
pub fn check_table_key_accessible<Eng: Send + Sync + 'static>(
    engine: &Eng,
    table: &super::Table,
) -> Result<(), super::RequestError> {
    use ::service_com_amazonaws_kms_trentservice as kms;

    let Some(encryption) = table.encryption.as_ref() else {
        return Ok(());
    };
    let Some(store) = kms::state::service_state(engine) else {
        return Ok(());
    };
    let store = store.lock();
    match store.resolve(&table.region, &encryption.key_arn) {
        kms::state::KeyRef::Found(key) if key.is_usable() => Ok(()),
        kms::state::KeyRef::Found(key) => Err(unusable_key_fault(key)),
        // The key the table was created with no longer answers at this endpoint, which is
        // the same KMS fault a request naming a key that never existed raises.
        _ => Err(unknown_sse_key(&encryption.key_arn)),
    }
}

/// How a key KMS will not use, because of the key state it is in, refuses the request that
/// reached for it.
///
/// KMS words a key-state refusal "<key arn> is <that key state, as prose>.", and the CLASS
/// of the exception it raises is what tells a recoverable DISABLED key apart from one being
/// destroyed — so the two are wrapped under their own classes and DynamoDB introduces them
/// apart. One function for every request that reaches for a table's key, the control plane's
/// and the data plane's alike, so no two of them word the same fault differently.
fn unusable_key_fault(
    key: &::service_com_amazonaws_kms_trentservice::state::Cmk,
) -> super::RequestError {
    use ::service_com_amazonaws_kms_trentservice as kms;

    let detail = kms::state::invalid_state_message(key);
    if key.reported_key_state() == kms::state::KEY_STATE_DISABLED {
        wrapped_kms_fault(KMS_DISABLED_PREFIX, DISABLED_EXCEPTION, &detail)
    } else {
        wrapped_kms_fault(KMS_VALIDATION_PREFIX, INVALID_STATE_EXCEPTION, &detail)
    }
}

/// How a request that names a key this Region's KMS endpoint does not serve is refused.
///
/// The attempt to use the key raised `NotFoundException` in KMS, and DynamoDB reports the
/// KMS exception it caught rather than a sentence of its own
/// ([`wrapped_kms_fault`]) — so the class name is in the message and a caller can tell
/// this fault from a disabled or a deleted key.
pub fn unknown_sse_key(key_id: &str) -> super::RequestError {
    super::RequestError::Validation(format!(
        "{KMS_VALIDATION_PREFIX}: {KMS_EXCEPTION_PACKAGE}.{NOT_FOUND_EXCEPTION}: Key '{key_id}' \
         does not exist (Service: Kms, Status Code: 400, Request ID: {}; Proxy: null)",
        kms_request_id()
    ))
}

/// The KMS fault a request met, as DynamoDB reports it: the class of the exception the
/// attempt to use the key raised, its own message, and the context of the KMS call that
/// raised it.
///
/// `prefix` is how DynamoDB introduces the fault, `class` the simple name of the KMS
/// exception ([`KMS_EXCEPTION_PACKAGE`] is where all of them live) and `detail` the
/// sentence KMS itself words the fault with — so the message carries the whole diagnosis
/// a caller classifies on, rather than collapsing every KMS fault into one.
fn wrapped_kms_fault(prefix: &str, class: &str, detail: &str) -> super::RequestError {
    super::RequestError::Validation(format!(
        "{prefix}: {KMS_EXCEPTION_PACKAGE}.{class}: {detail} (Service: AWSKMS; Status Code: 400; \
         Error Code: {class}; Request ID: {}; Proxy: null)",
        kms_request_id()
    ))
}

/// The package every KMS exception class a wrapped fault names belongs to, which is what
/// makes the class name in the message the fully qualified one a caller matches on.
const KMS_EXCEPTION_PACKAGE: &str = "com.amazonaws.services.kms.model";

/// The KMS exception a key that does not exist raises.
const NOT_FOUND_EXCEPTION: &str = "NotFoundException";

/// The KMS exception a DISABLED key raises, which is the recoverable fault: enabling the
/// key again restores every request it refused.
const DISABLED_EXCEPTION: &str = "DisabledException";

/// The KMS exception a key in any other unusable key state raises — a key awaiting
/// deletion above all, whose window is finite.
const INVALID_STATE_EXCEPTION: &str = "KMSInvalidStateException";

/// How DynamoDB introduces a wrapped KMS fault that is not the disabled one.
const KMS_VALIDATION_PREFIX: &str = "KMS validation error";

/// How DynamoDB introduces the DISABLED-key fault, which it words apart from every other
/// KMS fault.
const KMS_DISABLED_PREFIX: &str = "KMS key disabled error";

/// The id of the KMS call whose fault is being reported, which the wrapped message carries
/// so the failing call can be found in KMS's own records.
///
/// One id per call, drawn from the `uuid` crate's v4 generator: it identifies THIS attempt
/// to use the key and nothing else, so it is never derived from the request or stored.
fn kms_request_id() -> String {
    ::uuid::Uuid::new_v4().to_string()
}

/// How a `SSESpecification` naming an encryption type the service does not offer is
/// refused, over the one value the model declares ([`SSE_TYPE_KMS`]).
pub fn unsupported_sse_type(sse_type: &str) -> super::RequestError {
    super::RequestError::Validation(format!(
        "One or more parameter values were invalid: SSEType {sse_type} is not one of \
         {SSE_TYPE_KMS}"
    ))
}
