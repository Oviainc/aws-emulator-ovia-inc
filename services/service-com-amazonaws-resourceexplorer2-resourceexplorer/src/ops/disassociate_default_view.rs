// SPDX-License-Identifier: CC0-1.0

//! @op DisassociateDefaultView
//!
//! IMPLEMENTED: clears the default-view pointer `crate::state` holds for the Region the
//! request was signed for, so `GetDefaultView` reports nothing and `Search` has to be
//! told which view to use.
//!
//! The call is `@idempotent` and the pointer is optional, so clearing a Region that has
//! no default is a success that changes nothing — the state it asks for is the state the
//! Region is already in. It leaves the views themselves alone: only the pointer goes.
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
//! [`crate::state::service_state`] and write it through the guard it hands back; never
//! construct an engine in a handler.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! the SigV4 credential scope this handler reads the Region from lives there.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `DisassociateDefaultView`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-resourceexplorer2-resourceexplorer/smithy-model/com-amazonaws-resourceexplorer2-resourceexplorer.json")
//! ```
//!
//! After you call this operation, the affected Amazon Web Services Region no longer has a
//! default view. All Search operations in that Region must explicitly specify a view or
//! the operation fails. You can configure a new default by calling the
//! AssociateDefaultView operation. If an Amazon Web Services Region doesn't have a default
//! view configured, then users must explicitly specify a view with every `Search`
//! operation performed in that Region.
//!
//! ## Input
//!
//! `smithy.api#Unit` — no members.
//!
//! ## Output
//!
//! `smithy.api#Unit` — no members.
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
// `DisassociateDefaultViewError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-resourceexplorer2-resourceexplorer.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DisassociateDefaultViewError` alias below.
#[smithy_macros::operation_error("com.amazonaws.resourceexplorer2#DisassociateDefaultView")]
#[derive(Debug)]
pub enum DisassociateDefaultViewErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::disassociate_default_view::DisassociateDefaultViewError`).
pub type DisassociateDefaultViewError =
    crate::scaffolded::Scaffolded<DisassociateDefaultViewErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the handler
/// writes `Err(SomeModeledError.into())` rather than naming the wrapper variant.
impl From<DisassociateDefaultViewErrorModeled> for DisassociateDefaultViewError {
    fn from(modeled: DisassociateDefaultViewErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DisassociateDefaultView` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type.
pub struct DisassociateDefaultView<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DisassociateDefaultView<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DisassociateDefaultView<Eng>
{
    type Input = DisassociateDefaultViewInput;
    type Output = DisassociateDefaultViewOutput;
    type Error = DisassociateDefaultViewError;

    const NAME: &'static str = "DisassociateDefaultView";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::DISASSOCIATE_DEFAULT_VIEW_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::DISASSOCIATE_DEFAULT_VIEW_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The input is `smithy.api#Unit`, so there is nothing to read from it: which
        // Region loses its default is decided by the Region the request was signed for.
        let _ = &input;
        let Some(store) = crate::state::service_state(&*self.engine) else {
            return Err(
                DisassociateDefaultViewErrorModeled::InternalServerException(
                    InternalServerException {
                        message: Some("the Resource Explorer store is unavailable".to_string()),
                    },
                )
                .into(),
            );
        };
        let region = crate::state::request_region(ctx.parts());
        let now = SystemTime::now();
        let mut guard = store.lock();
        guard.settle(now);

        // Only the pointer goes; the view it named stays. A Region that had no default
        // is already in the state this call asks for, so there is nothing to refuse.
        guard.default_view_arns.remove(&region);
        Ok(DisassociateDefaultViewOutput {})
    }
}
