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
            // (`SSEDescription$InaccessibleEncryptionDateTime`): this emulator's keys stay
            // reachable, so there is no such instant to report.
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
        return store
            .associate_predefined_aws_alias(region, named)
            .map(|key| key.arn.clone())
            .ok_or_else(|| SseFault::Invalid(unknown_sse_key(named)));
    }
    match store.resolve(region, named) {
        kms::state::KeyRef::Found(key) => Ok(key.arn.clone()),
        _ => Err(SseFault::Invalid(unknown_sse_key(named))),
    }
}

/// How a `SSESpecification` naming a key this Region's KMS endpoint does not serve is
/// refused: the member the caller sent and the identifier it carried, which is what the
/// caller has to change.
pub fn unknown_sse_key(key_id: &str) -> super::RequestError {
    super::RequestError::Validation(format!(
        "One or more parameter values were invalid: KMSMasterKeyId {key_id} does not resolve to \
         a KMS key in this Region"
    ))
}

/// How a `SSESpecification` naming an encryption type the service does not offer is
/// refused, over the one value the model declares ([`SSE_TYPE_KMS`]).
pub fn unsupported_sse_type(sse_type: &str) -> super::RequestError {
    super::RequestError::Validation(format!(
        "One or more parameter values were invalid: SSEType {sse_type} is not one of \
         {SSE_TYPE_KMS}"
    ))
}
