// SPDX-License-Identifier: CC0-1.0

//! @op GetDefaultView
//!
//! IMPLEMENTED: reports the default-view pointer `crate::state` holds for the Region the
//! request was signed for.
//!
//! The pointer is OPTIONAL on the wire and that is the whole contract of this operation:
//! a Region that has no default view is answered with HTTP 200 and the `ViewArn` member
//! ABSENT, never with an error — "if an Amazon Web Services Region doesn't have a default
//! view configured, then users must explicitly specify a view with every Search
//! operation", which is something a caller learns by reading this and getting nothing
//! back. After `AssociateDefaultView` it reports the ARN that association stored, so the
//! most recent association is what this reads.
//!
//! Reporting only: the sole write is the index lifecycle transition the clock had already
//! made due ([`crate::state::ComAmazonawsResourceexplorer2ResourceexplorerState::settle`]),
//! which was owed at this instant whether or not anybody called.
//!
//! Hand-authored rather than scaffolded: the op-stub generator declined this operation
//! because its input is `smithy.api#Unit` (recorded in `src/ops/SKIPPED_STUBS.md`), so it
//! had no handler file and no entry in the service's `operations(...)` allow-list and was
//! answered by the framework's unroutable-operation path with a 501 the model does not
//! declare. `build.rs` derives that allow-list from the `//! @op <Name>` header of every
//! `src/ops/*.rs`, so this file is what wires it.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read it through the guard it hands back; never
//! construct an engine in a handler.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! the SigV4 credential scope this handler reads the Region from lives there.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetDefaultView`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-resourceexplorer2-resourceexplorer/smithy-model/com-amazonaws-resourceexplorer2-resourceexplorer.json")
//! ```
//!
//! Retrieves the Amazon Resource Name (ARN) of the view that is the default for the Amazon
//! Web Services Region in which you call this operation. You can then call GetView to
//! retrieve the details of that view.
//!
//! ## Input
//!
//! `smithy.api#Unit` — no members.
//!
//! ## Output
//!
//! * `ViewArn` (String) — The Amazon resource name (ARN) of the view that is the current
//!   default for the Amazon Web Services Region in which you called this operation.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The credentials that you used to call this
//!   operation don't have the minimum required permissions.
//! * `InternalServerException` (HTTP 500) — The request failed because of internal service
//!   error. Try your request again later.
//! * `ResourceNotFoundException` (HTTP 404) — You specified a resource that doesn't exist.
//!   Check the ID or ARN that you used to identity the resource, and try again.
//! * `ThrottlingException` (HTTP 429) — The request failed because you exceeded a rate limit
//!   for this operation. For more information, see Quotas for Resource Explorer.
//! * `ValidationException` (HTTP 400) — You provided an invalid value for one of the
//!   operation's parameters. Check the syntax for the operation, and try again.

#[allow(unused_imports)]
use super::types::*;

use std::time::SystemTime;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetDefaultViewError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-resourceexplorer2-resourceexplorer.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetDefaultViewError` alias below.
#[smithy_macros::operation_error("com.amazonaws.resourceexplorer2#GetDefaultView")]
#[derive(Debug)]
pub enum GetDefaultViewErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_default_view::GetDefaultViewError`).
pub type GetDefaultViewError = crate::scaffolded::Scaffolded<GetDefaultViewErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the handler
/// writes `Err(SomeModeledError.into())` rather than naming the wrapper variant.
impl From<GetDefaultViewErrorModeled> for GetDefaultViewError {
    fn from(modeled: GetDefaultViewErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetDefaultView` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct GetDefaultView<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetDefaultView<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetDefaultView<Eng>
{
    type Input = GetDefaultViewInput;
    type Output = GetDefaultViewOutput;
    type Error = GetDefaultViewError;

    const NAME: &'static str = "GetDefaultView";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GET_DEFAULT_VIEW_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::GET_DEFAULT_VIEW_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The input is `smithy.api#Unit`, so there is nothing to read from it: which
        // Region's default this reports is decided by the Region the request was signed
        // for, which is the Region `AssociateDefaultView` keyed the pointer under.
        let _ = &input;
        let Some(store) = crate::state::service_state(&*self.engine) else {
            return Err(GetDefaultViewErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some("the Resource Explorer store is unavailable".to_string()),
                },
            )
            .into());
        };
        let region = crate::state::request_region(ctx.parts());
        let now = SystemTime::now();
        let mut guard = store.lock();
        // A read, so the only write is the index lifecycle the clock already owed at
        // `now` — a deleted index takes its Region's views, and its default, with it.
        guard.settle(now);

        // Resolved THROUGH the view it points at, so this cannot report a default that
        // no longer exists: the pointer is only an answer while the view it names is one.
        let view_arn = guard
            .default_view_arns
            .get(&region)
            .and_then(|arn| guard.views.get(arn))
            .map(|view| view.view_arn.clone());
        Ok(GetDefaultViewOutput { view_arn })
    }
}
