// SPDX-License-Identifier: CC0-1.0

//! @op ListTypeRegistrations
//! IMPLEMENTED: lists the registration tokens of this account and Region's extension
//! REGISTRATION REQUESTS ([`crate::state::registry::StoredTypeRegistration`]), narrowed by
//! the request's status filter and by the extension it names.
//!
//! ## What the operation reports
//!
//! "Returns a list of registration tokens for the specified extension(s)." A registration
//! request is a record of its own — the one `RegisterType` is answered with a token for — so
//! the listing is served from the requests this account and Region holds and nothing is
//! invented: a request that matches no record answers an empty list rather than a fabricated
//! token. The status each record carries is the one the clock has made due, installed by the
//! `settle` walk every operation makes first, so a request still running is reported
//! `IN_PROGRESS` and one whose conclusion is due is reported as concluded.
//!
//! ## What narrows the listing
//!
//! `RegistrationStatusFilter` is "the current status of the extension registration request.
//! The default is IN_PROGRESS", so a request naming none lists the registrations still
//! running. Beside it the request may name ONE extension, and the model states that identity
//! across three members at once — "Conditional: You must specify either TypeName and Type,
//! or Arn" — so `TypeArn` names the extension on its own (an extension-VERSION ARN names the
//! extension it is a version of, since a registration belongs to the extension rather than
//! to one version of it) and `Type` and `TypeName` name it together; half of that name form
//! identifies no extension and is refused with the registry's own modelled
//! `CFNRegistryException`. A request naming NO extension lists every registration of the
//! account and Region, which is the listing those members narrow.
//!
//! ## Ordering and paging
//!
//! `@paginated(inputToken: NextToken, outputToken: NextToken, items: RegistrationTokenList,
//! pageSize: MaxResults)`: one response carries at most `MaxResults` tokens and the token
//! that resumes just past them. The listing is in the order the requests were started, which
//! is a property of the records rather than of the walk, so a caller paging through the
//! collection sees each registration exactly once.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read it through the guard it hands back.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`; the
//! Region a request is served in comes from there ([`crate::state::request_region`]).
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListTypeRegistrations`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json")
//! ```
//!
//! Returns a list of registration tokens for the specified extension(s).
//!
//! ## Input
//!
//! * `MaxResults` (MaxResults, range 1..100) — The maximum number of results to be returned
//!   with a single call. If the number of available results exceeds this maximum, the
//!   response includes a NextToken value that you can assign to the NextToken request
//!   parameter to get the next set of results.
//! * `NextToken` (NextToken, len 1..1024) — The token for the next set of items to return.
//!   (You received this token from a previous call.)
//! * `RegistrationStatusFilter` (RegistrationStatus, one of: COMPLETE | FAILED | IN_PROGRESS)
//!   — The current status of the extension registration request. The default is IN_PROGRESS.
//! * `Type` (RegistryType, one of: HOOK | MODULE | RESOURCE) — The kind of extension.
//!   Conditional: You must specify either TypeName and Type, or Arn.
//! * `TypeArn` (TypeArn, len 0..1024, pattern
//!   ^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}:([0-9]{12})?:type/.+$) —
//!   The Amazon Resource Name (ARN) of the extension. Conditional: You must specify either
//!   TypeName and Type, or Arn.
//! * `TypeName` (TypeName, len 10..204, pattern
//!   ^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}(::MODULE){0,1}$) — The name of
//!   the extension. Conditional: You must specify either TypeName and Type, or Arn.
//!
//! ## Output
//!
//! * `NextToken` (NextToken, len 1..1024) — If the request doesn't return all the remaining
//!   results, NextToken is set to a token. To retrieve the next set of results, call this
//!   action again and assign that token to the request object's NextToken parameter. If the
//!   request returns all results, NextToken is set to null.
//! * `RegistrationTokenList` (RegistrationTokenList) — A list of extension registration
//!   tokens. Use DescribeTypeRegistration to return detailed information about a type
//!   registration request.
//!
//! ## Errors
//!
//! * `CFNRegistryException` (HTTP 400) — An error occurred during a CloudFormation registry
//!   operation.

#[allow(unused_imports)]
use super::types::*;
use crate::state::registry::{self, StoredTypeRegistration};

/// Which extension the request narrows the listing to.
///
/// The model states that identity across three members at once — "Conditional: You must
/// specify either TypeName and Type, or Arn" — and this listing is of the registrations of
/// the whole account and Region, so naming NO extension is a listing of all of them rather
/// than a request that identifies nothing.
#[derive(Debug, Clone, Copy)]
enum Identified<'a> {
    /// No extension named: every registration this account and Region holds.
    EveryExtension,
    /// `TypeArn`, "the Amazon Resource Name (ARN) of the extension", which addresses the
    /// extension on its own.
    TypeArn(&'a str),
    /// `Type` and `TypeName`, which name it together, resolved in the Region the request was
    /// served in.
    Name {
        type_kind: &'a str,
        type_name: &'a str,
    },
}

impl Identified<'_> {
    /// Whether this registration is one the request asked for.
    fn admits(&self, registration: &StoredTypeRegistration) -> bool {
        match self {
            Self::EveryExtension => true,
            Self::TypeArn(type_arn) => registration.is_of_extension_arn(type_arn),
            Self::Name {
                type_kind,
                type_name,
            } => registration.is_of_extension_named(type_kind, type_name),
        }
    }
}

/// Read which extension the request narrows the listing to, or `Err(message)` for a request
/// that names half of the name form, which identifies no extension at all.
///
/// Every modelled constraint (each member's length and pattern, `Type`'s enum and
/// `RegistrationStatusFilter`'s) is already enforced before the handler runs; what is left is
/// the conditional rule the model states in prose. The ARN names the extension on its own, so
/// it is what the request identifies whatever else it spells out — the kind and the name
/// beside it are the same extension said another way, which is the reading
/// `SetTypeConfiguration` takes of the same three members.
fn read_request(input: &ListTypeRegistrationsInput) -> Result<Identified<'_>, String> {
    match (
        input.type_arn.as_deref(),
        input.r#type.as_deref(),
        input.type_name.as_deref(),
    ) {
        (Some(type_arn), _, _) => Ok(Identified::TypeArn(type_arn)),
        (None, Some(type_kind), Some(type_name)) => Ok(Identified::Name {
            type_kind,
            type_name,
        }),
        // Neither member of the name form: the listing is of every registration of this
        // account and Region, which is what the filters narrow.
        (None, None, None) => Ok(Identified::EveryExtension),
        // Half of the name form, which names no extension: the kind without the name admits
        // every extension of that kind and the name without the kind admits none, so neither
        // half is an identity this listing can be narrowed by.
        (None, _, _) => Err(
            "Either TypeArn, or both Type and TypeName, must be specified to identify an \
             extension"
                .to_string(),
        ),
    }
}

/// The registry's own modelled refusal: HTTP 400 `CFNRegistryException`, "an error occurred
/// during a CloudFormation registry operation".
fn registry_exception(message: String) -> ListTypeRegistrationsError {
    ListTypeRegistrationsErrorModeled::CFNRegistryException(CFNRegistryException {
        message: Some(message),
    })
    .into()
}

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListTypeRegistrationsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloudformation-cloudformation.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListTypeRegistrationsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.cloudformation#ListTypeRegistrations")]
#[derive(Debug)]
pub enum ListTypeRegistrationsErrorModeled {
    CFNRegistryException(CFNRegistryException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_type_registrations::ListTypeRegistrationsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListTypeRegistrationsError = crate::scaffolded::Scaffolded<ListTypeRegistrationsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListTypeRegistrationsErrorModeled> for ListTypeRegistrationsError {
    fn from(modeled: ListTypeRegistrationsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListTypeRegistrations` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListTypeRegistrations<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListTypeRegistrations<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListTypeRegistrations<Eng>
{
    type Input = ListTypeRegistrationsInput;
    type Output = ListTypeRegistrationsOutput;
    type Error = ListTypeRegistrationsError;

    const NAME: &'static str = "ListTypeRegistrations";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_TYPE_REGISTRATIONS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_TYPE_REGISTRATIONS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Which extension the request narrows the listing to. A combination of members that
        // identifies none is refused with the registry's own modelled 400 before any state
        // is touched.
        let identified = read_request(&input).map_err(registry_exception)?;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            // Not the shared in-memory engine: there are no registration requests to list,
            // so this answers 501 rather than an empty, truthful-looking list.
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "ListTypeRegistrations",
            ));
        };
        let region = crate::state::request_region(ctx.parts());
        // "The current status of the extension registration request. The default is
        // IN_PROGRESS": a request naming no filter lists the registrations still running.
        let wanted_status = input
            .registration_status_filter
            .as_deref()
            .unwrap_or(registry::registration_status::IN_PROGRESS);

        let mut store = state.lock();
        // The settle walk every operation makes first. A registration is asynchronous, so
        // this is what installs the conclusion the clock had ALREADY made due — the read
        // reports a status that was owed at this instant whether or not anybody called,
        // rather than moving a status because it was read.
        store.settle(::std::time::SystemTime::now());

        // In the order the requests were started, which is the order the store holds them
        // in, so the pages of one collection partition it.
        let tokens: Vec<String> = store
            .type_registrations_in(&region)
            .filter(|registration| {
                registration.status == wanted_status && identified.admits(registration)
            })
            .map(|registration| registration.registration_token.clone())
            .collect();

        // `MaxResults` is "the maximum number of results to be returned with a single
        // call", and the modelled range (1..100) is already enforced before this handler
        // runs.
        let max_results = input
            .max_results
            .and_then(|max_results| usize::try_from(max_results).ok());
        let (registration_token_list, next_token) = crate::state::paginate_at_most(
            crate::state::PageOf::RegistrationTokens,
            tokens,
            input.next_token.as_deref(),
            max_results,
        )
        .map_err(crate::scaffolded::Scaffolded::validation_error)?;

        Ok(ListTypeRegistrationsOutput {
            registration_token_list: Some(registration_token_list),
            next_token,
        })
    }
}
