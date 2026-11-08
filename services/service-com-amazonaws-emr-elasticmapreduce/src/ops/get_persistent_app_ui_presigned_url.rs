// SPDX-License-Identifier: CC0-1.0

//! @op GetPersistentAppUIPresignedURL
//!
//! Answers the URL of the persistent application user interface `PersistentAppUIId`
//! names, derived from the stored UI ([`crate::state::PersistentAppUIRecord::presigned_url`]).
//!
//! An id no UI carries, a `PersistentAppUIType` the UI does not publish and an
//! `ExecutionRoleArn` that is not the ARN of an IAM role are each refused with HTTP 400
//! `ValidationException` — the error code the service answers a rejected input on this
//! operation with. While the UI is still starting there is nothing to open yet, which
//! is exactly what `PresignedURLReady` reports: the call answers
//! `PresignedURLReady: false` and no URL until the UI is attached.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `GetPersistentAppUIPresignedURL::new(engine)` and drives it through the centralized
//! request walk: deserialize -> validate -> `handle` -> serialize.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. The
//! store is reached through [`crate::state::service_state`].
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetPersistentAppUIPresignedURL`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` ...`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-emr-elasticmapreduce/smithy-model/com-amazonaws-emr-elasticmapreduce.json")
//! ```
//!
//! The presigned URL properties for the cluster's application user interface.
//!
//! ## Input
//!
//! * `ApplicationId` (XmlStringMaxLen256, len 0..256) - The application ID associated with
//!   the presigned URL.
//! * `AuthProxyCall` (BooleanObject) - A boolean that represents if the caller is an
//!   authentication proxy call.
//! * `ExecutionRoleArn` (ArnType, len 20..2048) - The execution role ARN associated with the
//!   presigned URL.
//! * `PersistentAppUIId` (XmlStringMaxLen256, len 0..256) **required** - The persistent
//!   application user interface ID associated with the presigned URL.
//! * `PersistentAppUIType` (PersistentAppUIType, one of: SHS | TEZ | YTS) - The persistent
//!   application user interface type associated with the presigned URL.
//!
//! ## Output
//!
//! * `PresignedURL` (XmlString, len 0..10280) - The returned presigned URL.
//! * `PresignedURLReady` (Boolean) - Used to determine if the presigned URL is ready.
//!
//! ## Errors
//!
//! * `InternalServerError` (HTTP 500) - Indicates that an error occurred while processing the
//!   request and that the request was not completed.
//! * `InvalidRequestException` (HTTP 400) - This exception occurs when there is something
//!   wrong with user input.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-emr-elasticmapreduce.json");

// The operation's MODELED errors.
#[smithy_macros::operation_error("com.amazonaws.emr#GetPersistentAppUIPresignedURL")]
#[derive(Debug)]
pub enum GetPersistentAppUIPresignedURLErrorModeled {
    InternalServerError(InternalServerError),
    InvalidRequestException(InvalidRequestException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_persistent_app_ui_presigned_url::GetPersistentAppUIPresignedURLError`).
pub type GetPersistentAppUIPresignedURLError = crate::scaffolded::Scaffolded<GetPersistentAppUIPresignedURLErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<GetPersistentAppUIPresignedURLErrorModeled> for GetPersistentAppUIPresignedURLError {
    fn from(modeled: GetPersistentAppUIPresignedURLErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetPersistentAppUIPresignedURL` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct GetPersistentAppUIPresignedURL<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetPersistentAppUIPresignedURL<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetPersistentAppUIPresignedURL<Eng>
{
    type Input = GetPersistentAppUIPresignedURLInput;
    type Output = GetPersistentAppUIPresignedURLOutput;
    type Error = GetPersistentAppUIPresignedURLError;

    const NAME: &'static str = "GetPersistentAppUIPresignedURL";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_PERSISTENT_APP_UI_PRESIGNED_URL_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_PERSISTENT_APP_UI_PRESIGNED_URL_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let now = ::std::time::SystemTime::now();

        // A rejected input on this operation is answered with HTTP 400
        // `ValidationException`: the error code the service gives it, which this
        // operation's own model does not declare (it carries only
        // `InvalidRequestException`), so it goes through the channel that can render it.
        //
        // This is DELIBERATELY not the code `DescribePersistentAppUI` answers an unknown
        // id with (its own modelled `InvalidRequestException`): the two readers of this
        // resource word their refusals differently because Amazon EMR does, so do not
        // unify them.
        let refuse = |message: String| -> Self::Error {
            crate::scaffolded::Scaffolded::validation(message)
        };

        // "The execution role ARN associated with the presigned URL."
        // (`GetPersistentAppUIPresignedURL$ExecutionRoleArn` doc.) An execution role is
        // an IAM role of this account, read through the tree's shared IAM-ARN reading
        // (`crate::state::is_this_accounts_role_arn`), so a value naming no such role
        // names nothing to assume and is refused rather than ignored.
        if let Some(role_arn) = input.execution_role_arn.as_deref() {
            if !crate::state::is_this_accounts_role_arn(role_arn) {
                return Err(refuse(format!(
                    "ExecutionRoleArn '{role_arn}' must be the ARN of an IAM role of this \
                     account."
                )));
            }
        }

        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(GetPersistentAppUIPresignedURLErrorModeled::InternalServerError(
                InternalServerError {},
            )
            .into());
        };
        let mut store = state.lock();

        // An id this account holds no UI for is a client error, not an empty success or
        // a URL for a UI that does not exist. The service answers it as a rejection of
        // the member that selects the interface.
        let Some(record) = store.persistent_app_ui_mut(&input.persistent_app_ui_id, now) else {
            return Err(refuse(
                crate::state::PRESIGNED_URL_NO_SUCH_UI_MESSAGE.to_string(),
            ));
        };

        // The URL opens one of the user interfaces this UI publishes: the one the
        // request names, or its own when the request names none. A type it does not
        // publish resolves to no interface, the same way an unknown id does.
        let ui_type = match input.persistent_app_ui_type.as_deref() {
            Some(requested) => {
                if !record.serves(requested) {
                    return Err(refuse(
                        crate::state::PRESIGNED_URL_NO_SUCH_UI_MESSAGE.to_string(),
                    ));
                }
                requested.to_string()
            }
            None => record.default_ui_type().to_string(),
        };

        // "Used to determine if the presigned URL is ready."
        // (`GetPersistentAppUIPresignedURLOutput$PresignedURLReady` doc.) A UI the
        // service has not attached to its target resource yet serves nothing, so there
        // is no URL to answer — that is what the member reports, rather than a URL that
        // opens nothing. Nothing is minted for it either.
        if !record.is_attached() {
            return Ok(GetPersistentAppUIPresignedURLOutput {
                presigned_url_ready: Some(false),
                presigned_url: None,
            });
        }

        // Every call PRESIGNS the URL afresh: the session token in it is minted for this
        // call, so two calls for one user interface never hand back the same credential
        // and a URL that was cached or shared carries a token of its own. The record is
        // read-only here — nothing about the token is stored, which is what keeps the
        // answer un-cacheable by construction.
        let Some(session_token) = crate::state::mint_presigned_url_token(ctx.credential_store())
        else {
            return Err(GetPersistentAppUIPresignedURLErrorModeled::InternalServerError(
                InternalServerError {},
            )
            .into());
        };
        let url = record.presigned_url(&ui_type, input.application_id.as_deref(), &session_token);
        drop(store);

        Ok(GetPersistentAppUIPresignedURLOutput {
            presigned_url_ready: Some(true),
            presigned_url: Some(url),
        })
    }
}
