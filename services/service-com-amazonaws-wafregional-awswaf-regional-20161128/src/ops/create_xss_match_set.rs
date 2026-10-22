// SPDX-License-Identifier: CC0-1.0

//! @op CreateXssMatchSet
//!
//! Consumes the caller's change token, mints an `XssMatchSetId`, and stores a new, empty
//! `XssMatchSet` under it, so `GetXssMatchSet`, `ListXssMatchSets` and
//! `UpdateXssMatchSet` all see the record this call created. Answers the stored record
//! and the change token it consumed.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `CreateXssMatchSet::new(engine)` and drives it through the centralized request walk:
//! deserialize -> validate -> `handle` -> serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request, so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only:
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateXssMatchSet`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped. For the FULL text of any one item, ask the
//! model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-wafregional-awswaf-regional-20161128/smithy-model/com-amazonaws-wafregional-awswaf-regional-20161128.json")
//! ```
//!
//! This is AWS WAF Classic documentation. For more information, see AWS WAF Classic in the
//! developer guide. For the latest version of AWS WAF, use the AWS WAFV2 API and see the
//! AWS WAF Developer Guide. With the latest version, AWS WAF has a single set of endpoints
//! for regional and global use. Creates an XssMatchSet, which you use to allow, block, or
//! count requests that contain cross-site scripting attacks in the specified part of web
//! requests. AWS WAF searches for character sequences that are likely to be malicious
//! strings. To create and configure an XssMatchSet, perform the following steps: Use
//! GetChangeToken to get the change token that you provide in the ChangeToken parameter of
//! a CreateXssMatchSet request. Submit a CreateXssMatchSet request. Use GetChangeToken to
//! get the change token that you provide in the ChangeToken parameter of an
//! UpdateXssMatchSet request. Submit an UpdateXssMatchSet request to specify the parts of
//! web requests in which you want to allow, block, or count cross-site scripting attacks.
//! For more information about how to use the AWS WAF API to allow or block HTTP requests,
//! see the AWS WAF Developer Guide.
//!
//! ## Input
//!
//! * `ChangeToken` (ChangeToken, len 1..128, pattern \S) **required** - The value returned by
//!   the most recent call to GetChangeToken.
//! * `Name` (ResourceName, len 1..128, pattern \S) **required** - A friendly name or
//!   description for the XssMatchSet that you're creating. You can't change Name after you
//!   create the XssMatchSet.
//!
//! ## Output
//!
//! * `ChangeToken` (ChangeToken, len 1..128, pattern \S) - The ChangeToken that you used to
//!   submit the CreateXssMatchSet request. You can also use this value to query the status of
//!   the request. For more information, see GetChangeTokenStatus.
//! * `XssMatchSet` (XssMatchSet) - An XssMatchSet.
//!
//! ## Errors
//!
//! * `WAFDisallowedNameException` (HTTP 400) - The name specified is invalid.
//! * `WAFInternalErrorException` (HTTP 500) - The operation failed because of a system
//!   problem, even though the request was valid. Retry your request.
//! * `WAFInvalidAccountException` (HTTP 400) - The operation failed because you tried to
//!   create, update, or delete an object by using an invalid account identifier.
//! * `WAFInvalidParameterException` (HTTP 400) - The operation failed because AWS WAF didn't
//!   recognize a parameter in the request.
//! * `WAFLimitsExceededException` (HTTP 400) - The operation exceeds a resource limit, for
//!   example, the maximum number of WebACL objects that you can create for an AWS account.
//!   For more information, see Limits in the AWS WAF Developer Guide.
//! * `WAFStaleDataException` (HTTP 400) - The operation failed because you tried to create,
//!   update, or delete an object by using a change token that has already been used.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateXssMatchSetError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-wafregional-awswaf-regional-20161128.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateXssMatchSetError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.wafregional#CreateXssMatchSet")]
#[derive(Debug)]
pub enum CreateXssMatchSetErrorModeled {
    WAFDisallowedNameException(WAFDisallowedNameException),
    WAFInternalErrorException(WAFInternalErrorException),
    WAFInvalidAccountException(WAFInvalidAccountException),
    WAFInvalidParameterException(WAFInvalidParameterException),
    WAFLimitsExceededException(WAFLimitsExceededException),
    WAFStaleDataException(WAFStaleDataException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_xss_match_set::CreateXssMatchSetError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateXssMatchSetError = crate::scaffolded::Scaffolded<CreateXssMatchSetErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateXssMatchSetErrorModeled> for CreateXssMatchSetError {
    fn from(modeled: CreateXssMatchSetErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateXssMatchSet` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateXssMatchSet<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateXssMatchSet<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateXssMatchSet<Eng>
{
    type Input = CreateXssMatchSetInput;
    type Output = CreateXssMatchSetOutput;
    type Error = CreateXssMatchSetError;

    const NAME: &'static str = "CreateXssMatchSet";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_XSS_MATCH_SET_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_XSS_MATCH_SET_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreateXssMatchSetErrorModeled::WAFInternalErrorException(WAFInternalErrorException {
                message: Some(crate::state::MESSAGE_INTERNAL_ERROR.to_string()),
            })
        })?;
        let mut state = state.lock();

        // Every XssMatchSet is a stored record, so the number a caller can create is
        // bounded, as it is on the real service.
        if state.xss_match_sets.len() >= crate::state::MAX_XSS_MATCH_SETS {
            return Err(CreateXssMatchSetErrorModeled::WAFLimitsExceededException(
                WAFLimitsExceededException {
                    message: Some(crate::state::MESSAGE_LIMITS_EXCEEDED.to_string()),
                },
            )
            .into());
        }

        // The change-token protocol every mutating AWS WAF Classic call follows: the
        // token must be one `GetChangeToken` minted and no other mutating call has
        // used, since "each create, update, or delete request must use a unique change
        // token".
        if !state.consume_change_token(&input.change_token) {
            return Err(CreateXssMatchSetErrorModeled::WAFStaleDataException(
                WAFStaleDataException {
                    message: Some(crate::state::MESSAGE_STALE_DATA.to_string()),
                },
            )
            .into());
        }

        // A fresh id per call: `ResourceName` carries no uniqueness constraint, so two
        // XssMatchSets created with the same `Name` are two distinct resources, each
        // stored under its own `XssMatchSetId`.
        let xss_match_set_id = state.mint_id();
        // A new XssMatchSet holds no filters: they arrive through `UpdateXssMatchSet`.
        let entry = crate::state::XssMatchSetEntry::new(xss_match_set_id.clone(), input.name);
        // The response is the projection of the record that was STORED, not of the
        // request.
        let xss_match_set = entry.to_modelled();
        state.xss_match_sets.insert(xss_match_set_id, entry);

        Ok(CreateXssMatchSetOutput {
            xss_match_set: Some(xss_match_set),
            change_token: Some(input.change_token),
        })
    }
}
