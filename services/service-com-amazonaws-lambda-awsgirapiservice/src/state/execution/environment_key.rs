// SPDX-License-Identifier: CC0-1.0

//! Whether the key a function's ENVIRONMENT VARIABLES are encrypted with can decrypt them.
//!
//! A function's environment variables are encrypted at rest with a key: "The ARN of the
//! Key Management Service (KMS) customer managed key that's used to encrypt […] your
//! function's environment variables" (`KMSKeyArn`). Every invocation of such a function
//! therefore begins with a DECRYPT, and an invocation whose key cannot decrypt is refused
//! before the handler runs — which is what the four KMS failures `Invoke` binds are:
//! `KMSDisabledException`, `KMSInvalidStateException`, `KMSNotFoundException` and
//! `KMSAccessDeniedException`, each `@httpError(502)`.
//!
//! The key's state is not this service's to know: it belongs to the key service. So it is
//! ASKED, over the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]) — the framework's seam for one
//! emulated service calling another — and what that answer says decides which of the four
//! the invocation is refused with. Only a DEFINITE answer refuses: a key the key service
//! reports as usable, and an answer that establishes nothing at all (the key service is
//! not mounted beside this one, it answered about itself, or it was not reached), both
//! leave the invocation to run.
//!
//! A function with no `KMSKeyArn`, or with no environment variables to decrypt, has nothing
//! to ask about.

use std::collections::BTreeMap;

/// Why the key a function's environment variables are encrypted with cannot decrypt them.
///
/// One variant per KMS failure the invocation operations bind, carrying the sentence the
/// refusal states.
#[derive(Debug, Clone)]
pub enum EnvironmentKeyFault {
    /// The key is disabled.
    Disabled(String),
    /// The key's state does not admit `Decrypt`.
    InvalidState(String),
    /// The key is not there.
    NotFound(String),
    /// The key is there and may not be used.
    AccessDenied(String),
}

/// The sentence a refusal states when the key is DISABLED.
///
/// Every one of these names the operation Lambda could not perform (decrypting the
/// environment variables), why, and where the caller fixes it — the form AWS words this
/// class of failure in, and the wording the modelled exception documents
/// (`smithy_query(query="shape", shape="KMSDisabledException")`: "Lambda couldn't decrypt
/// the environment variables because the KMS key used is disabled. Check the Lambda
/// function's KMS key settings.").
const DISABLED_MESSAGE: &str = "Lambda was unable to decrypt the environment variables because \
                                the KMS key used is disabled. Please check the function's KMS \
                                key settings.";

/// See [`DISABLED_MESSAGE`]; the state of the key does not admit a decrypt.
const INVALID_STATE_MESSAGE: &str = "Lambda was unable to decrypt the environment variables \
                                     because the state of the KMS key used is not valid for \
                                     Decrypt. Please check the function's KMS key settings.";

/// See [`DISABLED_MESSAGE`]; the key is not there.
const NOT_FOUND_MESSAGE: &str = "Lambda was unable to decrypt the environment variables because \
                                 the KMS key was not found. Please check the function's KMS key \
                                 settings.";

/// See [`DISABLED_MESSAGE`]; the key may not be used by this function.
const ACCESS_DENIED_MESSAGE: &str = "Lambda was unable to decrypt the environment variables \
                                     because KMS access was denied. Please check the function's \
                                     KMS permissions.";

/// The `KeyState` values that are not valid for `Decrypt`, other than `Disabled` — the
/// states a key is in while it is being created, imported, deleted or is unavailable
/// (source: `smithy_query(query="shape", shape="KeyState")` of the key service's model).
const STATES_INVALID_FOR_DECRYPT: &[&str] = &[
    "Creating",
    "PendingDeletion",
    "PendingImport",
    "PendingReplicaDeletion",
    "Unavailable",
    "Updating",
];

/// The `KeyState` a key that can decrypt is in.
const STATE_ENABLED: &str = "Enabled";

/// Why the environment variables of a function configured with `kms_key_arn` cannot be
/// decrypted, or `None` when nothing says they cannot.
///
/// `variables` is the function's own environment: a function with none has nothing
/// encrypted with the key, so there is nothing to decrypt and nothing to ask about.
///
/// `callback` is the in-process seam the key service is asked over, which a request reaches
/// through [`smithy_server_runtime::OperationContext::callback`] and the asynchronous
/// delivery loop ([`super::super::async_delivery`]) holds its own clone of: an invocation
/// begins with this decrypt however it arrived.
pub async fn environment_key_fault(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    kms_key_arn: Option<&str>,
    variables: &BTreeMap<String, String>,
) -> Option<EnvironmentKeyFault> {
    let key_arn = kms_key_arn?;
    if variables.is_empty() {
        return None;
    }
    let described = kms_client(callback, region)
        .describe_key()
        .key_id(key_arn)
        .send()
        .await;
    match described {
        Ok(answer) => {
            let state = answer
                .key_metadata()
                .and_then(::aws_sdk_kms::types::KeyMetadata::key_state)
                .map(|state| state.as_str().to_string());
            match state.as_deref() {
                // A key the key service reports as usable decrypts, so the invocation runs.
                None | Some(STATE_ENABLED) => None,
                Some("Disabled") => Some(EnvironmentKeyFault::Disabled(
                    DISABLED_MESSAGE.to_string(),
                )),
                Some(state) if STATES_INVALID_FOR_DECRYPT.contains(&state) => Some(
                    EnvironmentKeyFault::InvalidState(INVALID_STATE_MESSAGE.to_string()),
                ),
                // A state this emulator does not know is not evidence that the key cannot
                // decrypt.
                Some(_) => None,
            }
        }
        // Only an answer FROM the key service says anything about the key. A transport
        // failure — the key service is not mounted beside this one, or the in-process call
        // did not reach it — is read without the panicking accessor, because it carries no
        // service error at all, and leaves the invocation to run.
        Err(failure) => match failure {
            ::aws_sdk_kms::error::SdkError::ServiceError(answered) => {
                match answered.into_err() {
                    error if error.is_not_found_exception() => Some(
                        EnvironmentKeyFault::NotFound(NOT_FOUND_MESSAGE.to_string()),
                    ),
                    error if error.is_invalid_arn_exception() => Some(
                        EnvironmentKeyFault::NotFound(NOT_FOUND_MESSAGE.to_string()),
                    ),
                    error if is_access_denied(&error) => Some(
                        EnvironmentKeyFault::AccessDenied(ACCESS_DENIED_MESSAGE.to_string()),
                    ),
                    // Every other answer about the key establishes nothing that says it
                    // cannot decrypt.
                    _ => None,
                }
            }
            _ => None,
        },
    }
}

/// How the refusal of a read of a durable execution's DATA is worded: the operation Lambda
/// could not perform (decrypting that execution's data), why, and WHICH key it could not
/// perform it under.
///
/// The sentence is the same class of failure [`DISABLED_MESSAGE`] and its siblings word for
/// the environment-variable decrypt, said of the data this read reports and naming the key
/// the function's `DurableConfig` names — the one variable part, taken from the stored
/// configuration.
fn durable_data_message(why: &str, key_arn: &str) -> String {
    format!(
        "Lambda was unable to decrypt the durable execution data because the KMS key \
         {key_arn} {why}. Please check the function's KMS key settings."
    )
}

/// Why the DATA of a durable execution encrypted under `kms_key_arn` cannot be read, or
/// `None` when nothing says it cannot.
///
/// Every operation that reports a durable execution's data binds the same four KMS
/// failures `Invoke` binds (`smithy_query(query="operation",
/// operation="GetDurableExecution,GetDurableExecutionHistory,GetDurableExecutionState")`:
/// `KMSAccessDeniedException`, `KMSDisabledException`, `KMSInvalidStateException`,
/// `KMSNotFoundException`, each `@httpError(502)`), because reporting that data decrypts it
/// under the key the function's `DurableConfig` names. So the key is asked about at the
/// point of use, over the same seam and with the same reading as
/// [`environment_key_fault`]: only a DEFINITE answer refuses.
///
/// It differs from that reading in ONE respect, and deliberately: a key the key service
/// holds no record of is NOT a refusal here. A durable function's key is named in its
/// configuration and may belong to another account, so an absent record establishes
/// nothing about it — while a key the key service HOLDS and reports unusable is evidence
/// that the data behind it cannot be read.
pub async fn durable_data_key_fault(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    kms_key_arn: Option<&str>,
) -> Option<EnvironmentKeyFault> {
    let key_arn = kms_key_arn?;
    let described = kms_client(callback, region)
        .describe_key()
        .key_id(key_arn)
        .send()
        .await;
    match described {
        Ok(answer) => {
            let state = answer
                .key_metadata()
                .and_then(::aws_sdk_kms::types::KeyMetadata::key_state)
                .map(|state| state.as_str().to_string());
            match state.as_deref() {
                None | Some(STATE_ENABLED) => None,
                Some("Disabled") => Some(EnvironmentKeyFault::Disabled(durable_data_message(
                    "is disabled",
                    key_arn,
                ))),
                Some(state) if STATES_INVALID_FOR_DECRYPT.contains(&state) => {
                    Some(EnvironmentKeyFault::InvalidState(durable_data_message(
                        "is not in a valid state for Decrypt",
                        key_arn,
                    )))
                }
                Some(_) => None,
            }
        }
        Err(failure) => match failure {
            ::aws_sdk_kms::error::SdkError::ServiceError(answered) => {
                match answered.into_err() {
                    error if is_access_denied(&error) => {
                        Some(EnvironmentKeyFault::AccessDenied(durable_data_message(
                            "may not be used",
                            key_arn,
                        )))
                    }
                    // Every other answer about the key — including one about a key this
                    // key service holds no record of — establishes nothing.
                    _ => None,
                }
            }
            _ => None,
        },
    }
}

/// Whether `error` is the key service saying this caller may not use the key.
///
/// Read off the error CODE rather than a variant, because an access denial is not one of
/// the modelled exceptions of `DescribeKey`: it arrives as the generic refusal every
/// authorization check answers with.
fn is_access_denied(error: &::aws_sdk_kms::operation::describe_key::DescribeKeyError) -> bool {
    ::aws_sdk_kms::error::ProvideErrorMetadata::code(error)
        .is_some_and(|code| code.contains("AccessDenied"))
}

/// A key-service client over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated service: it
/// routes in process, so no network client is configured here. `region` is the Region the
/// invocation was addressed to, which is where the function's key lives.
fn kms_client(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_kms::Client {
    let config = ::aws_sdk_kms::Config::builder()
        .behavior_version(::aws_sdk_kms::config::BehaviorVersion::latest())
        .region(::aws_sdk_kms::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://kms.{region}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given. The same goes for caching the identity: the credentials are handed
        // over directly.
        .retry_config(::aws_sdk_kms::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_kms::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_kms::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_kms::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_kms::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each fault states what Lambda could not do and why — one row per class of KMS
    /// failure the invocation operations bind.
    #[test]
    fn every_fault_states_the_decrypt_it_could_not_perform() {
        let cases: &[(&str, EnvironmentKeyFault, &str)] = &[
            (
                "disabled",
                EnvironmentKeyFault::Disabled(DISABLED_MESSAGE.to_string()),
                "the KMS key used is disabled",
            ),
            (
                "an invalid state",
                EnvironmentKeyFault::InvalidState(INVALID_STATE_MESSAGE.to_string()),
                "not valid for Decrypt",
            ),
            (
                "not found",
                EnvironmentKeyFault::NotFound(NOT_FOUND_MESSAGE.to_string()),
                "the KMS key was not found",
            ),
            (
                "access denied",
                EnvironmentKeyFault::AccessDenied(ACCESS_DENIED_MESSAGE.to_string()),
                "KMS access was denied",
            ),
        ];
        for (case, fault, expected) in cases {
            let message = match fault {
                EnvironmentKeyFault::Disabled(message)
                | EnvironmentKeyFault::InvalidState(message)
                | EnvironmentKeyFault::NotFound(message)
                | EnvironmentKeyFault::AccessDenied(message) => message.clone(),
            };
            assert!(
                message.starts_with("Lambda was unable to decrypt the environment variables")
                    && message.contains(expected),
                "case {case}: expected the decrypt failure and {expected:?}, got {message:?}"
            );
        }
    }
}
