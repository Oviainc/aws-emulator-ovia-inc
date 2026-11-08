// SPDX-License-Identifier: CC0-1.0

//! The custom email sender trigger: the one place a pool's `LambdaConfig.CustomEmailSender`
//! becomes an invocation of the function that member names.
//!
//! "The configuration of a custom email sender Lambda trigger. This trigger routes all email
//! notifications from a user pool to a Lambda function" (`LambdaConfigType$CustomEmailSender`,
//! this service's model). So on a pool that configures one, an operation that issues a code
//! does not mail it: it hands the code to that function, which is the only place the code is
//! delivered at all. The code travels encrypted "with the key that you provided in the
//! KMSKeyID parameter" — `LambdaConfigType$KMSKeyID` is "the ARN of an KMS key. Amazon
//! Cognito uses the key to encrypt codes and temporary passwords sent to
//! `CustomEmailSender` and `CustomSMSSender`" — so the key the pool configured is asked to
//! wrap the data key of the message [`super::esdk`] writes.
//!
//! The work splits the same way every other trigger of this service does
//! ([`super`]): [`custom_email_sender`] resolves the member and builds the event from the
//! pool and the user read under the state lock, and [`invoke_custom_email_sender`] encrypts
//! the code and performs the invocation once that lock is RELEASED.
//!
//! Which stage issued the code is what the event's `triggerSource` reports
//! ([`CustomEmailSource`]); the request half is the `customEmailSenderRequestV1` document,
//! whose `code` is the encrypted code and whose `userAttributes` are the user's own.

use std::collections::HashMap;

use ::base64::Engine as _;
use ::serde_json::{Map, Value};

use super::esdk;
use super::{Caller, Dispatch, Failure, Verdict};
use crate::state::{User, UserPool};

/// The ARN namespace of the key service, whose keys a custom sender's `KMSKeyID` names.
const KMS_ARN_NAMESPACE: &str = "kms";

/// The trigger whose configured function every custom email sender event is delivered to, as
/// a failure of that trigger names it.
pub const CUSTOM_EMAIL_SENDER_TRIGGER: &str = "CustomEmailSender";

/// What a custom email sender event's `request.type` reports: the request shape the function
/// is handed, as the trigger's one version defines it.
const CUSTOM_EMAIL_SENDER_REQUEST_TYPE: &str = "customEmailSenderRequestV1";

/// The error a pool that routes its messages to a custom sender without configuring a key
/// for them carries: there is no key to encrypt the code under, so the message cannot be
/// handed over at all.
const NO_KEY_CONFIGURED_ERROR: &str =
    "No KMS key is configured for the custom sender of this user pool";

/// The error a code that could not be prepared for the function carries.
const NO_RANDOMNESS_ERROR: &str = "The code could not be prepared for the custom sender";

/// The error a key service answer carrying no wrapped key carries.
const NO_WRAPPED_KEY_ERROR: &str = "The configured KMS key wrapped no key for the code";

/// Which stage issued the code a custom sender is handed, which is what the event's
/// `triggerSource` reports: the trigger's name and that stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustomEmailSource {
    /// A sign-up's confirmation code, sent by `SignUp`.
    SignUp,
    /// A sign-up confirmation code sent again, by `ResendConfirmationCode`.
    ResendCode,
    /// A password-reset code, sent by `ForgotPassword` and by `AdminResetUserPassword`.
    ForgotPassword,
    /// The code for an attribute an update of the user's attributes asked to verify.
    UpdateUserAttribute,
    /// The code for one attribute, asked for by `GetUserAttributeVerificationCode`.
    VerifyUserAttribute,
    /// The one-time password of an email sign-in challenge.
    Authentication,
}

impl CustomEmailSource {
    /// This stage's `triggerSource`.
    #[must_use]
    pub fn trigger_source(self) -> &'static str {
        match self {
            Self::SignUp => "CustomEmailSender_SignUp",
            Self::ResendCode => "CustomEmailSender_ResendCode",
            Self::ForgotPassword => "CustomEmailSender_ForgotPassword",
            Self::UpdateUserAttribute => "CustomEmailSender_UpdateUserAttribute",
            Self::VerifyUserAttribute => "CustomEmailSender_VerifyUserAttribute",
            Self::Authentication => "CustomEmailSender_Authentication",
        }
    }
}

/// One custom sender invocation an operation owes: the invocation itself, the key the code
/// is encrypted under and the code that was issued.
///
/// The code is held in the CLEAR here and encrypted only when the invocation is performed,
/// because encrypting it is a call to another service and this is built under the state
/// lock.
#[derive(Debug, Clone)]
pub struct PendingCustomEmail {
    dispatch: Dispatch,
    kms_key_id: Option<String>,
    code: String,
}

impl PendingCustomEmail {
    /// The event this invocation delivers, before the code is encrypted into it.
    #[must_use]
    pub fn event(&self) -> &Value {
        self.dispatch.event()
    }

    /// The function this invocation is of, as the pool's `LambdaConfig` names it.
    #[must_use]
    pub fn function_arn(&self) -> &str {
        self.dispatch.function_arn()
    }

    /// The key the code is encrypted under, as the pool's `LambdaConfig` names it, or `None`
    /// for a pool that configures none.
    #[must_use]
    pub fn kms_key_id(&self) -> Option<&str> {
        self.kms_key_id.as_deref()
    }

    /// The code that was issued, which the function is the only recipient of.
    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }
}

/// The custom sender invocation a code issued to `user` of `pool` owes, or `None` when the
/// pool configures no custom email sender and so the message is not routed to a function at
/// all.
///
/// Built while the caller still holds the state lock — the event reports the pool and the
/// user as the issuing operation left them — and invoked by [`invoke_custom_email_sender`]
/// once that lock is released.
#[must_use]
pub fn custom_email_sender(
    pool: &UserPool,
    user: &User,
    source: CustomEmailSource,
    caller: Caller<'_>,
    client_metadata: Option<&HashMap<String, String>>,
    code: &str,
) -> Option<PendingCustomEmail> {
    let configured = pool.lambda_config.custom_email_sender.as_ref()?;
    let function_arn = configured.lambda_arn.clone();
    if function_arn.is_empty() {
        return None;
    }
    let mut members = Map::new();
    members.insert(
        "type".to_string(),
        Value::String(CUSTOM_EMAIL_SENDER_REQUEST_TYPE.to_string()),
    );
    // The code is encrypted when the invocation is performed, so the member it will travel
    // in is there from the start and nothing else decides where it goes.
    members.insert("code".to_string(), Value::Null);
    let mut event = super::event(
        pool,
        user,
        source.trigger_source(),
        &super::event_version(&configured.lambda_version),
        caller,
        client_metadata,
        members,
    );
    // A custom sender SENDS the message; it does not amend the outcome of the operation that
    // issued it, so the event it is handed carries no response half to fill in.
    if let Value::Object(members) = &mut event {
        members.remove("response");
    }
    Some(PendingCustomEmail {
        dispatch: super::dispatch(pool, function_arn, CUSTOM_EMAIL_SENDER_TRIGGER, event),
        kms_key_id: pool
            .lambda_config
            .kms_key_id
            .clone()
            .filter(|key| !key.is_empty()),
        code: code.to_string(),
    })
}

/// Fire the custom email sender of a code issued for `username` in `user_pool_id`, and
/// report what it leaves the issuing operation to answer.
///
/// For an operation that holds NO lock: the pool and the user are read under a lock of its
/// own, which is released before the code is encrypted and the function invoked, so no other
/// request of this service waits on either. A pool that configures no custom sender is
/// answered by nothing being invoked at all.
#[allow(clippy::too_many_arguments)]
pub async fn fire_custom_email_sender(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    store: &::engine::ServiceState<super::State>,
    user_pool_id: &str,
    username: &str,
    source: CustomEmailSource,
    caller: Caller<'_>,
    client_metadata: Option<&HashMap<String, String>>,
    code: &str,
) -> Result<(), Failure> {
    let pending = {
        let guard = store.lock();
        let pool = guard.pool(user_pool_id);
        let user = pool.and_then(|pool| pool.user(username));
        match (pool, user) {
            (Some(pool), Some(user)) => {
                custom_email_sender(pool, user, source, caller, client_metadata, code)
            }
            _ => None,
        }
    };
    match pending {
        Some(pending) => invoke_custom_email_sender(ctx, pending).await,
        None => Ok(()),
    }
}

/// Encrypt the pending code under the pool's key and invoke the function it is destined for.
///
/// Call it with the state lock RELEASED: the issuing operation's own writes are already
/// persisted — the code is recorded, which is what the operation that consumes it checks a
/// presented code against — and holding the lock across the key-service call and the
/// invocation would make every other request of this service wait on them.
pub async fn invoke_custom_email_sender(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    pending: PendingCustomEmail,
) -> Result<(), Failure> {
    let PendingCustomEmail {
        mut dispatch,
        kms_key_id,
        code,
    } = pending;
    let Some(kms_key_id) = kms_key_id else {
        return Err(Failure::NotInvokable(super::failure_message(
            CUSTOM_EMAIL_SENDER_TRIGGER,
            NO_KEY_CONFIGURED_ERROR,
        )));
    };
    let region = dispatch.region.clone();
    let encrypted = encrypted_code(ctx, &region, &kms_key_id, &code).await?;
    if let Some(request) = dispatch.event.get_mut("request").and_then(Value::as_object_mut) {
        request.insert("code".to_string(), Value::String(encrypted));
    }
    // What the function ANSWERS decides nothing: a sender is handed the message to deliver,
    // not the outcome to amend. A function that could not be invoked at all, or that
    // reported an error, is the trigger's own failure and is the issuing operation's to
    // report ([`super::invoke`]).
    match super::invoke(ctx, &dispatch).await? {
        Verdict::Answered(_) | Verdict::NoVerdict(_) => Ok(()),
    }
}

/// `code`, as the message the function receives it in: a fresh data key wrapped by the
/// configured key, and the code encrypted under that data key ([`super::esdk`]), carried
/// base64-encoded because the event member is text.
///
/// `pool_region` is where the key is asked for when the configured value names no Region of
/// its own — a bare key id or an alias does not.
async fn encrypted_code(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    pool_region: &str,
    kms_key_id: &str,
    code: &str,
) -> Result<String, Failure> {
    let random = ::ring::rand::SystemRandom::new();
    let mut data_key = [0_u8; esdk::DATA_KEY_LENGTH];
    let mut message_id = [0_u8; esdk::MESSAGE_ID_LENGTH];
    ::ring::rand::SecureRandom::fill(&random, &mut data_key)
        .and_then(|()| ::ring::rand::SecureRandom::fill(&random, &mut message_id))
        .map_err(|_| trigger_failed(NO_RANDOMNESS_ERROR))?;

    let region = ::service_common::arn::region_of(kms_key_id, KMS_ARN_NAMESPACE)
        .unwrap_or(pool_region)
        .to_string();
    let wrapped = kms_client(ctx, &region)
        .encrypt()
        .key_id(kms_key_id)
        .plaintext(::aws_sdk_kms::primitives::Blob::new(data_key.to_vec()))
        .send()
        .await
        .map_err(|error| {
            trigger_failed(
                ::aws_smithy_types::error::metadata::ProvideErrorMetadata::message(&error)
                    .unwrap_or(NO_WRAPPED_KEY_ERROR),
            )
        })?;
    let Some(blob) = wrapped.ciphertext_blob() else {
        return Err(trigger_failed(NO_WRAPPED_KEY_ERROR));
    };
    // The key service reports the key it used, which is the key a reader has to ask to
    // unwrap the data key; the configured value is what names it when it reports none.
    let message = esdk::seal(
        &data_key,
        blob.as_ref(),
        wrapped.key_id().unwrap_or(kms_key_id),
        &message_id,
        code.as_bytes(),
    )
    .map_err(|fault| trigger_failed(fault.message()))?;
    Ok(::base64::engine::general_purpose::STANDARD.encode(message))
}

/// The failure of this trigger carrying `error`, worded the way every user-pool trigger's
/// failure is.
fn trigger_failed(error: &str) -> Failure {
    Failure::NotInvokable(super::failure_message(CUSTOM_EMAIL_SENDER_TRIGGER, error))
}

/// The key service's client for `region`, over the in-process callback into this same
/// emulator: the key a pool configures is one this emulator itself holds, so the call never
/// leaves the process.
///
/// The transport is in-process, so this disables what a network client needs and this one
/// cannot have, exactly as [`super::lambda_client`] does and for the same reason.
fn kms_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_kms::Client {
    let config = ::aws_sdk_kms::Config::builder()
        .behavior_version(::aws_sdk_kms::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_kms::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://{KMS_ARN_NAMESPACE}.{region}.amazonaws.com"))
        .retry_config(::aws_sdk_kms::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_kms::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_kms::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_kms::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_kms::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `triggerSource` this trigger reports names the trigger, so a function can tell
    /// a custom sender event from any other event it is handed, and the stage that issued
    /// the code.
    ///
    /// The stages are the ones Amazon Cognito documents a custom email sender source for.
    #[test]
    fn every_trigger_source_names_the_custom_email_sender_and_its_stage() {
        let cases = [
            (CustomEmailSource::SignUp, "SignUp"),
            (CustomEmailSource::ResendCode, "ResendCode"),
            (CustomEmailSource::ForgotPassword, "ForgotPassword"),
            (CustomEmailSource::UpdateUserAttribute, "UpdateUserAttribute"),
            (CustomEmailSource::VerifyUserAttribute, "VerifyUserAttribute"),
            (CustomEmailSource::Authentication, "Authentication"),
        ];
        for (source, stage) in cases {
            let reported = source.trigger_source();
            assert_eq!(
                reported,
                format!("{CUSTOM_EMAIL_SENDER_TRIGGER}_{stage}"),
                "case {source:?}: expected the trigger and the stage, got {reported:?}"
            );
        }
    }
}
