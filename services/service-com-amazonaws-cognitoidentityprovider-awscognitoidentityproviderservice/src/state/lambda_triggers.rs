// SPDX-License-Identifier: CC0-1.0

//! The user-pool Lambda-trigger dispatch: the one place a pool's `LambdaConfig` member
//! becomes an invocation of the function that member names.
//!
//! "A collection of user pool Lambda triggers. Amazon Cognito invokes triggers at several
//! possible stages of user pool operations. Triggers can modify the outcome of the
//! operations that invoked them." (`LambdaConfigType`, this service's model.) A configured
//! trigger is therefore not decoration an operation may ignore: the operation that reaches
//! the stage a member is configured for invokes that member's function with the event for
//! that stage, and reports what the function answered.
//!
//! The member dispatched from here is the post-confirmation one: "the configuration of a
//! post confirmation Lambda trigger in a user pool. This trigger can take custom actions
//! after a user confirms their user account and their email address or phone number"
//! (`LambdaConfigType$PostConfirmation`). Every other member belongs to the operation that
//! reaches ITS stage and is dispatched the same way, through the two steps this module
//! splits the work into:
//!
//! 1. [`post_confirmation`] resolves the member and builds the event, from the pool and
//!    the user the operation already holds under the state lock;
//! 2. [`invoke`] performs the invocation, after the operation has RELEASED that lock, so
//!    no other request of this service waits on a function.
//!
//! The function is invoked over the in-process callback into this same emulator
//! ([`lambda_client`]), so a trigger reaches only a function this emulator itself holds.

use std::collections::HashMap;

use ::serde_json::{Map, Value};

use super::{User, UserPool};

/// The ARN namespace of AWS Lambda, whose functions a user-pool trigger names.
const LAMBDA_ARN_NAMESPACE: &str = "lambda";

/// A user-pool trigger event's `version`: the one version Amazon Cognito documents an
/// event shape for.
const EVENT_VERSION: &str = "1";

/// `callerContext.clientId` of an event fired by an operation that names no app client —
/// an `Admin*` operation, which IAM credentials authorize rather than an app client. The
/// value Amazon Cognito reports in that member's place.
const CLIENT_ID_NOT_APPLICABLE: &str = "CLIENT_ID_NOT_APPLICABLE";

/// `callerContext.awsSdkVersion` of an event fired by a request that says which SDK made
/// it nowhere: the placeholder Amazon Cognito reports for an SDK it cannot name.
const UNKNOWN_AWS_SDK_VERSION: &str = "aws-sdk-unknown-unknown";

/// The reserved name a trigger event's `request.userAttributes` carries the user's STATUS
/// under. The status is not one of the user's stored attributes — it is rendered beside
/// them, from the status the pool holds for the user.
const USER_STATUS_ATTRIBUTE: &str = "cognito:user_status";

/// The trigger whose configured function every post-confirmation event is delivered to,
/// as a failure of that trigger names it ([`failure_message`]).
const POST_CONFIRMATION_TRIGGER: &str = "PostConfirmation";

/// Which completed confirmation fires a post-confirmation event, which is what the
/// event's `triggerSource` reports: the trigger's name and the operation that reached it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirmation {
    /// A sign-up confirmed — by `ConfirmSignUp` with the code the sign-up sent, or by
    /// `AdminConfirmSignUp` without one.
    SignUp,
    /// A forgotten password reset and the reset code answered, by
    /// `ConfirmForgotPassword`.
    ForgotPassword,
}

impl Confirmation {
    /// This confirmation's `triggerSource`.
    #[must_use]
    pub fn trigger_source(self) -> &'static str {
        match self {
            Self::SignUp => "PostConfirmation_ConfirmSignUp",
            Self::ForgotPassword => "PostConfirmation_ConfirmForgotPassword",
        }
    }
}

/// What the request that fired a trigger contributes to its event's `callerContext`.
#[derive(Debug, Clone, Copy)]
pub struct Caller<'request> {
    /// The app client the request named, or `None` for an operation that names none.
    pub client_id: Option<&'request str>,
    /// The SDK the request announced itself as.
    pub aws_sdk_version: Option<&'request str>,
}

impl<'request> Caller<'request> {
    /// The caller of the request `parts`, which named the app client `client_id`.
    ///
    /// `awsSdkVersion` is read from the request's own `User-Agent`, the only place a
    /// request says which SDK made it; a request carrying none, or one whose value is not
    /// text, contributes none and the event carries
    /// [`UNKNOWN_AWS_SDK_VERSION`] in its place.
    #[must_use]
    pub fn of(parts: &'request ::http::request::Parts, client_id: Option<&'request str>) -> Self {
        Self {
            client_id,
            aws_sdk_version: parts
                .headers
                .get(::http::header::USER_AGENT)
                .and_then(|value| value.to_str().ok()),
        }
    }
}

/// One invocation an operation owes: the function to invoke, the Region whose Lambda
/// endpoint holds it, the trigger it serves and the event to deliver.
#[derive(Debug, Clone)]
pub struct Dispatch {
    function_arn: String,
    region: String,
    trigger: &'static str,
    event: Value,
}

impl Dispatch {
    /// The event this invocation delivers to the function.
    #[must_use]
    pub fn event(&self) -> &Value {
        &self.event
    }

    /// The function this invocation is of, as the pool's `LambdaConfig` names it.
    #[must_use]
    pub fn function_arn(&self) -> &str {
        &self.function_arn
    }
}

/// The invocation a completed confirmation of `user` in `pool` owes, or `None` when the
/// pool configures no post-confirmation function and so nothing is invoked.
///
/// Built while the caller still holds the state lock — the event reports the pool and the
/// user as the completed operation left them — and invoked by [`invoke`] once that lock is
/// released.
///
/// The Region is the one the configured function's own ARN names, so a trigger function in
/// another Region is reached through that Region's Lambda endpoint; a member that names no
/// Region label falls back to the pool's own Region.
#[must_use]
pub fn post_confirmation(
    pool: &UserPool,
    user: &User,
    confirmation: Confirmation,
    caller: Caller<'_>,
    client_metadata: Option<&HashMap<String, String>>,
) -> Option<Dispatch> {
    let function_arn = pool.lambda_config.as_ref()?.post_confirmation.clone()?;
    let region = ::service_common::arn::region_of(&function_arn, LAMBDA_ARN_NAMESPACE)
        .unwrap_or(pool.region.as_str())
        .to_string();
    let event = event(
        pool,
        user,
        confirmation.trigger_source(),
        caller,
        client_metadata,
    );
    Some(Dispatch {
        function_arn,
        region,
        trigger: POST_CONFIRMATION_TRIGGER,
        event,
    })
}

/// The event a user-pool trigger is invoked with: which trigger fired, in which pool, for
/// which user, on behalf of which caller, with the user's attributes as the request half
/// of the event.
fn event(
    pool: &UserPool,
    user: &User,
    trigger_source: &str,
    caller: Caller<'_>,
    client_metadata: Option<&HashMap<String, String>>,
) -> Value {
    let mut request = Map::new();
    request.insert(
        "userAttributes".to_string(),
        Value::Object(user_attributes(user)),
    );
    // `ClientMetadata` is "a map of custom key-value pairs that you can provide as input
    // for any custom workflows that this action triggers […] This payload contains a
    // clientMetadata attribute that provides the data that you assigned to the
    // ClientMetadata parameter in your request" (every operation that models it), so the
    // member appears only for a request that assigned one.
    if let Some(metadata) = client_metadata.filter(|metadata| !metadata.is_empty()) {
        let members: Map<String, Value> = metadata
            .iter()
            .map(|(name, value)| (name.clone(), Value::String(value.clone())))
            .collect();
        request.insert("clientMetadata".to_string(), Value::Object(members));
    }
    ::serde_json::json!({
        "version": EVENT_VERSION,
        "triggerSource": trigger_source,
        // The pool's own Region and id, not the request's: the event names the resource
        // the trigger is configured on.
        "region": pool.region,
        "userPoolId": pool.id,
        "userName": user.username,
        "callerContext": {
            "awsSdkVersion": caller.aws_sdk_version.unwrap_or(UNKNOWN_AWS_SDK_VERSION),
            "clientId": caller.client_id.unwrap_or(CLIENT_ID_NOT_APPLICABLE),
        },
        "request": Value::Object(request),
        // A post-confirmation function is handed nothing to amend: the trigger "can take
        // custom actions after a user confirms", which is after the outcome it would
        // otherwise modify is already settled, so the response half of the event is empty.
        "response": Value::Object(Map::new()),
    })
}

/// The user's attributes as a trigger event's `request.userAttributes`: every attribute
/// the pool stores for the user, rendered the way every other read of this service renders
/// them, with the user's status beside them under [`USER_STATUS_ATTRIBUTE`].
fn user_attributes(user: &User) -> Map<String, Value> {
    let mut attributes: Map<String, Value> = user
        .attribute_list()
        .into_iter()
        .map(|attribute| {
            (
                attribute.name,
                Value::String(attribute.value.unwrap_or_default()),
            )
        })
        .collect();
    attributes.insert(
        USER_STATUS_ATTRIBUTE.to_string(),
        Value::String(user.status.as_str().to_string()),
    );
    attributes
}

/// What a trigger invocation leaves the operation that owed it to report, when the
/// operation's own success is no longer what it answers.
///
/// Both variants carry the message the refusal states, built by [`failure_message`]; the
/// operation wraps it in its OWN modelled error, because the error channel belongs to the
/// operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The function RAN and reported an error. The operation's
    /// `UserLambdaValidationException` — "a user validation exception with the Lambda
    /// service" (this service's model) — is what that amounts to.
    Reported(String),
    /// Lambda reports the configured function as one it cannot invoke at all, so the
    /// trigger did not run. The operation's `UnexpectedLambdaException` — "an unexpected
    /// exception with Lambda" (this service's model) — is what that amounts to.
    NotInvokable(String),
}

/// How Amazon Cognito states a user-pool trigger's failure: the trigger's name, then the
/// error that trigger produced, terminated once — an error that already ends in a
/// terminator keeps its own rather than being given a second.
#[must_use]
fn failure_message(trigger: &str, error: &str) -> String {
    let error = error.trim();
    if error.ends_with('.') {
        format!("{trigger} failed with error {error}")
    } else {
        format!("{trigger} failed with error {error}.")
    }
}

/// Invoke the function of `dispatch` and report what the trigger leaves the operation to
/// answer.
///
/// The invocation is SYNCHRONOUS: a trigger "can modify the outcome of the operations that
/// invoked them" (`LambdaConfigType`), which the invoking operation can only honour by
/// waiting for the function's answer.
///
/// Call it with the state lock RELEASED: the operation's own writes are already persisted,
/// and holding the lock across the invocation would make every other request of this
/// service wait on the function.
pub async fn invoke(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    dispatch: &Dispatch,
) -> Result<(), Failure> {
    let answer = lambda_client(ctx, &dispatch.region)
        .invoke()
        .function_name(&dispatch.function_arn)
        .invocation_type(::aws_sdk_lambda::types::InvocationType::RequestResponse)
        .payload(::aws_sdk_lambda::primitives::Blob::new(
            dispatch.event.to_string().into_bytes(),
        ))
        .send()
        .await;
    let output = match answer {
        Ok(output) => output,
        // Lambda says this account holds no such function: the pool's trigger names a
        // function that cannot be invoked, which is the operation's failure to report.
        Err(error) if function_is_absent(&error) => {
            return Err(Failure::NotInvokable(failure_message(
                dispatch.trigger,
                &error_detail(&error),
            )));
        }
        // Any other answer is this emulator failing to ASK, not Lambda reporting a
        // function it cannot invoke — so the function's verdict is not known, and none is
        // invented for it. The operation's own answer stands: a post-confirmation trigger
        // "can take custom actions AFTER a user confirms their user account", so the
        // confirmation this invocation follows is already complete and is not undone by
        // an invocation that did not reach the function. The warning is what says the
        // trigger's own side effects did not happen.
        Err(error) => {
            ::tracing::warn!(
                function_arn = %dispatch.function_arn,
                trigger = dispatch.trigger,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a user pool Lambda trigger could not be invoked; the operation that \
                 fired it reports no verdict from the function"
            );
            return Ok(());
        }
    };
    // "FunctionError: if present, indicates that an error occurred during function
    // execution. Details about the error are included in the response payload"
    // (Lambda's `Invoke` model) — so the payload is where the error the trigger
    // produced is read from, and the handling kind is the fallback when it carries none.
    if let Some(function_error) = output.function_error() {
        let detail = reported_error(output.payload())
            .unwrap_or_else(|| function_error.to_string());
        return Err(Failure::Reported(failure_message(
            dispatch.trigger,
            &detail,
        )));
    }
    // A post-confirmation function's response is not read: the trigger runs after the
    // outcome it could have modified, so there is nothing in its answer for this
    // service to apply.
    Ok(())
}

/// The error a failed function reported, read from the `errorMessage` of the payload
/// Lambda returns for it. `None` for a payload that carries none, including one that is
/// not a JSON object.
fn reported_error(payload: Option<&::aws_sdk_lambda::primitives::Blob>) -> Option<String> {
    let parsed: Value = ::serde_json::from_slice(payload?.as_ref()).ok()?;
    Some(parsed.get("errorMessage")?.as_str()?.to_string())
}

/// Whether an `Invoke` failure is Lambda reporting that the function is not there, rather
/// than this emulator failing to ask.
fn function_is_absent(
    error: &::aws_sdk_lambda::error::SdkError<
        ::aws_sdk_lambda::operation::invoke::InvokeError,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> bool {
    error
        .as_service_error()
        .is_some_and(::aws_sdk_lambda::operation::invoke::InvokeError::is_resource_not_found_exception)
}

/// The message a refused invocation carries, as Lambda itself worded it.
fn error_detail<E: ::aws_smithy_types::error::metadata::ProvideErrorMetadata>(
    error: &E,
) -> String {
    error
        .message()
        .unwrap_or("the function could not be invoked")
        .to_string()
}

/// Lambda's client for `region`, over the in-process callback into this same emulator: a
/// trigger function is one this emulator holds, so the invocation never leaves the
/// process.
///
/// The transport is in-process, so this disables what a network client needs and this one
/// cannot have: retries, timeouts, stalled-stream protection and identity caching each
/// need a sleep implementation the callback transport is not given, and there is nothing
/// to retry behind it anyway. The same client policy every other service's in-process
/// Lambda invocation builds (`service-com-amazonaws-s3-amazons3/src/state/notifications.rs`).
fn lambda_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_lambda::Client {
    let config = ::aws_sdk_lambda::Config::builder()
        .behavior_version(::aws_sdk_lambda::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_lambda::config::Region::new(region.to_string()))
        .endpoint_url(format!(
            "https://{LAMBDA_ARN_NAMESPACE}.{region}.amazonaws.com"
        ))
        .retry_config(::aws_sdk_lambda::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_lambda::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_lambda::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_lambda::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_lambda::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `triggerSource` this module reports names the trigger whose configured
    /// function serves it, so a function cannot be handed an event of another trigger.
    #[test]
    fn every_trigger_source_names_the_post_confirmation_trigger() {
        for confirmation in [Confirmation::SignUp, Confirmation::ForgotPassword] {
            let source = confirmation.trigger_source();
            assert!(
                source.starts_with(&format!("{POST_CONFIRMATION_TRIGGER}_")),
                "case {confirmation:?}: expected a {POST_CONFIRMATION_TRIGGER} trigger \
                 source, got {source:?}"
            );
        }
    }

    /// The two confirmations are distinct sources: one operation's event never claims to
    /// be another's.
    #[test]
    fn the_two_confirmations_report_different_sources() {
        assert_ne!(
            Confirmation::SignUp.trigger_source(),
            Confirmation::ForgotPassword.trigger_source(),
            "expected one trigger source per confirming operation, got the same for both"
        );
    }

    /// A failure message states the trigger and the error once, with one terminating
    /// period whatever the error's own punctuation.
    #[test]
    fn a_failure_message_states_the_trigger_and_the_error() {
        // (case, error as the invocation reported it, expected message)
        let rows = [
            (
                "a message with no punctuation",
                "Function not found: arn:aws:lambda:us-east-1:000000000000:function:f",
                "PostConfirmation failed with error Function not found: \
                 arn:aws:lambda:us-east-1:000000000000:function:f.",
            ),
            (
                "a message that ends in a period",
                "the handler raised KeyError.",
                "PostConfirmation failed with error the handler raised KeyError.",
            ),
            (
                "a message padded with whitespace",
                "  not authorized  ",
                "PostConfirmation failed with error not authorized.",
            ),
            (
                "a message that ends in an ellipsis",
                "the handler timed out...",
                "PostConfirmation failed with error the handler timed out...",
            ),
        ];
        for (case, error, expected) in rows {
            let message = failure_message(POST_CONFIRMATION_TRIGGER, error);
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }
}
