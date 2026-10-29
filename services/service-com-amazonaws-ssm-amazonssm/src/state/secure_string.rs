// SPDX-License-Identifier: CC0-1.0

//! Whether a read may have a `SecureString` parameter's value decrypted for its caller.
//!
//! A `SecureString`'s value is sealed under the KMS key the put named
//! ([`super::DEFAULT_SECURE_STRING_KEY_ID`] when it named none), and Parameter Store does
//! not hold that key: it asks KMS to open the value, as the CALLER and through its own
//! service path, and the ciphertext it presents is bound to the one parameter it belongs
//! to by the encryption context [`PARAMETER_ARN_CONTEXT_KEY`] naming that parameter's ARN.
//!
//! Two things follow, and they are the whole of this module:
//!
//! * a caller the key does not permit `kms:Decrypt` cannot have the value opened for it,
//!   so the read is refused rather than answered with the value;
//! * the decision is made under that encryption context, so a key policy statement scoped
//!   to one parameter's entry admits the read of THAT parameter and leaves the reads of
//!   every other parameter sealed under the same key to the policy's other statements —
//!   which is how one key serves many parameters while each parameter's value stays
//!   readable only by the principals its own statement names.
//!
//! The decision is the KMS service's OWN ([`kms_store::authorization`]), made against the
//! key policy, the caller's identity-based policies and the grants on the key that KMS's
//! handlers wrote, so this service and KMS cannot disagree about who may use a key, and
//! the refusal a caller sees is the sentence KMS itself words for it.
//!
//! A read that asks for no decryption decrypts nothing, and neither does a type that is
//! not encrypted at all: "This flag is ignored for String and StringList parameter types"
//! (`smithy_query(query="doc",shape="GetParameterRequest",member="WithDecryption")`).
//!
//! Nothing here is attributed to a key this emulator holds nothing about: a key id that
//! resolves to no key, and a deployment with no KMS mounted beside this service, say
//! nothing about who may use it, so such a read is served. An Amazon Web Services managed
//! key — the one [`super::DEFAULT_SECURE_STRING_KEY_ID`] names — carries a policy AWS
//! manages for the service that owns it, so a caller needs no grant of its own on it.

use std::collections::HashMap;

use ::service_com_amazonaws_kms_trentservice::state as kms_store;

/// The encryption-context entry a `SecureString`'s ciphertext is bound to: the parameter's
/// own Amazon Resource Name, under this entry name.
pub const PARAMETER_ARN_CONTEXT_KEY: &str = "PARAMETER_ARN";

/// The KMS operation a read that reports a decrypted `SecureString` performs on the key.
const DECRYPT_OPERATION: &str = "Decrypt";

/// One value a read would report decrypted: the parameter its ciphertext is bound to, and
/// the key that ciphertext is sealed under.
#[derive(Debug, Clone)]
pub struct DecryptedValue {
    /// The parameter's ARN, which is the value of [`PARAMETER_ARN_CONTEXT_KEY`].
    pub parameter_arn: String,
    /// The `KeyId` the version was stored under, in any form KMS accepts.
    pub key_id: String,
}

/// The decrypt reporting `version` of `record` performs, or `None` when it performs none:
/// a read that did not ask for decryption, and a version of a type that is not encrypted,
/// each report the stored value without opening anything.
pub fn decrypted_value(
    with_decryption: Option<bool>,
    record: &super::ParameterRecord,
    version: &super::ParameterVersionRecord,
) -> Option<DecryptedValue> {
    if !with_decryption.unwrap_or(false) || version.type_ != super::TYPE_SECURE_STRING {
        return None;
    }
    Some(DecryptedValue {
        parameter_arn: record.arn(),
        key_id: version
            .key_id
            .clone()
            .unwrap_or_else(|| super::DEFAULT_SECURE_STRING_KEY_ID.to_string()),
    })
}

/// Whether every value in `values` may be decrypted for this request's caller, and the
/// refusal KMS words when one of them may not.
///
/// `region` is the Region the request was signed for, which is where the keys the
/// parameters name are resolved: a parameter and the key it is sealed under live at the
/// same endpoint.
///
/// Reaches no state of THIS service, so a handler calls it with its own store guard
/// released — the stores this consults (KMS's, and through it STS's and IAM's) are never
/// read while this service's guard is held.
pub fn authorize_decryption<'a, Eng, Values>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    values: Values,
) -> Result<(), String>
where
    Eng: Send + Sync + 'static,
    Values: IntoIterator<Item = &'a DecryptedValue>,
{
    let values: Vec<&DecryptedValue> = values.into_iter().collect();
    if values.is_empty() {
        return Ok(());
    }
    let Some(kms) = kms_store::service_state(engine) else {
        // Visible to an operator: a decision nothing answered serves the read, so
        // without this line it would be indistinguishable from a key that permitted it.
        ::tracing::warn!(
            "no KMS service is mounted beside Systems Manager; a SecureString read is \
             served without asking the key"
        );
        return Ok(());
    };
    // The caller is resolved before the KMS store is locked, because resolving it reads
    // the STS and IAM stores.
    let caller = kms_store::authorization::caller(engine, ctx);
    let store = kms.lock();
    for value in values {
        let key = match store.resolve(region, &value.key_id) {
            kms_store::KeyRef::Found(key) => key,
            // An identifier whose account segment is not an account id is refused for
            // want of permission before any key is looked up, so the value cannot be
            // opened under it at all.
            kms_store::KeyRef::UnauthorizedAccount => {
                return Err(kms_store::unauthorized_account_message(DECRYPT_OPERATION))
            }
            // Nothing is attributed to a key this service holds nothing about.
            kms_store::KeyRef::NotFound | kms_store::KeyRef::MalformedArn => continue,
        };
        if key.key_manager != kms_store::KEY_MANAGER_CUSTOMER {
            continue;
        }
        let context = HashMap::from([(
            PARAMETER_ARN_CONTEXT_KEY.to_string(),
            value.parameter_arn.clone(),
        )]);
        kms_store::authorization::authorize_use(
            &caller,
            &store,
            region,
            key,
            kms_store::authorization::RequestedUse::through_service(
                DECRYPT_OPERATION,
                &via_service(region),
            ),
            Some(&context),
            None,
        )?;
    }
    Ok(())
}

/// The value of the `kms:ViaService` condition key for a decrypt this service performs on
/// a caller's behalf: this service's regional endpoint name, which is what a key policy
/// confining a key to Parameter Store's use of it names.
fn via_service(region: &str) -> String {
    format!("{}.{region}.amazonaws.com", super::ARN_SERVICE)
}
