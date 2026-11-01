// SPDX-License-Identifier: CC0-1.0

//! @op ListComponentTypes
//! Reports the component types of ONE workspace — "Lists all component types in
//! a workspace" — in `componentTypeId` order, a page at a time, each row built
//! from the STORED component type record the create persisted, so a summary says
//! about a component type exactly what `GetComponentType` says about it.
//!
//! The workspace is the listing's scope: a request naming a workspace this
//! service does not hold is refused with the `ValidationException` (HTTP 400)
//! this operation models — it declares no `ResourceNotFoundException`, so a
//! missing scope is reported through the only client error it has, the same
//! choice `ListEntities` makes.
//!
//! `filters` accepts AT MOST ONE object ("Only one object is accepted as a valid
//! input" — the filter union's own documentation), and each of its three members
//! selects on what this service STORES:
//!
//! * `extendsFrom` — the component types whose reported `extendsFrom` names that
//!   id. Membership is tested against the parents the create declared, which are
//!   exactly what `GetComponentType` reports, so the filter and the read agree on
//!   what one component type extends. DIRECT extension only, for that reason: a
//!   component type's stored parents are the ones its create named, and this
//!   service resolves no inheritance chain beyond them (`GetComponentType`
//!   reports every member `isInherited: false` for the same reason), so a
//!   grandchild is not selected by its grandparent's id.
//! * `namespace` — the component types that BELONG to that namespace. A
//!   `componentTypeId` is a dotted name (`^[a-zA-Z_\.\-0-9:]+$`) whose namespace
//!   is its dotted prefix, so a type is in the namespace when its id starts with
//!   the namespace followed by a `.`; an id carrying no such prefix belongs to no
//!   namespace and is selected by no `namespace` filter.
//! * `isAbstract` — the stored component types whose abstractness matches, read
//!   off [`crate::state::ComponentType::is_abstract`], the same field
//!   `GetComponentType` reports from, so the filter and the read cannot select
//!   different sets. A component type is abstract when it cannot be
//!   instantiated: the service's own built-ins that exist to be extended, and a
//!   created type that declares an externally stored property no data connector
//!   of its own or of a parent can serve
//!   ([`crate::state::component_type_is_abstract`]).
//!
//! `maxResults` caps the page: its documented default is 25 and its documented
//! minimum 1, so an omitted value serves 25 rows and an explicit zero is refused.
//! (The modelled `range` on the member admits 0, so that refusal is this
//! handler's, applied on the strength of the member's own documentation.) The
//! service never serves more than [`crate::state::MAX_PAGE_SIZE`] rows whatever
//! is asked for, and the response reports the page size it actually served in its
//! own `maxResults` ("Specifies the maximum number of results to display"). The
//! opaque `nextToken` resumes at the next `componentTypeId` rather than at an
//! offset, so a page boundary survives a component type being created or deleted
//! between two pages; it is bound to the workspace it was minted for AND to the
//! `filters` that decided which component types the page was of
//! ([`crate::state::filters_token_scope`]), and a token this listing did not mint
//! is refused with the same `ValidationException`.
//!
//! This is a REPORTING operation. The only thing it writes is the lifecycle
//! transition the CLOCK already made due: a component type is stored in the
//! non-terminal `CREATING`/`UPDATING` until its `settles_at` passes, and that
//! transition was owed at that instant whether or not anybody called, so each
//! component type is settled before it is reported — exactly what
//! `GetComponentType` does for the one it reads.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `ListComponentTypes::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListComponentTypes`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/smithy-model/com-amazonaws-iottwinmaker-awsiottwinmaker.json")
//! ```
//!
//! Lists all component types in a workspace.
//!
//! ## Input
//!
//! * `filters` (ListComponentTypesFilters) — A list of objects that filter the request.
//! * `maxResults` (MaxResults, range 0..200) — The maximum number of results to return at one
//!   time. The default is 25. Valid Range: Minimum value of 1. Maximum value of 250.
//! * `nextToken` (NextToken, len 0..17880, pattern .*) — The string that specifies the next
//!   page of results.
//! * `workspaceId` (Id, len 1..128, pattern ^[a-zA-Z_0-9][a-zA-Z_\-0-9]*[a-zA-Z0-9]+$)
//!   **required** — The ID of the workspace.
//!
//! ## Output
//!
//! * `componentTypeSummaries` (ComponentTypeSummaries) **required** — A list of objects that
//!   contain information about the component types.
//! * `maxResults` (MaxResults, range 0..200) — Specifies the maximum number of results to
//!   display.
//! * `nextToken` (NextToken, len 0..17880, pattern .*) — The string that specifies the next
//!   page of results.
//! * `workspaceId` (Id, len 1..128, pattern ^[a-zA-Z_0-9][a-zA-Z_\-0-9]*[a-zA-Z0-9]+$)
//!   **required** — The ID of the workspace.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — Access is denied.
//! * `InternalServerException` (HTTP 500) — An unexpected error has occurred.
//! * `ThrottlingException` (HTTP 429) — The rate exceeds the limit.
//! * `ValidationException` (HTTP 400) — Failed

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListComponentTypesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-iottwinmaker-awsiottwinmaker.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListComponentTypesError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.iottwinmaker#ListComponentTypes")]
#[derive(Debug)]
pub enum ListComponentTypesErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_component_types::ListComponentTypesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListComponentTypesError = crate::scaffolded::Scaffolded<ListComponentTypesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListComponentTypesErrorModeled> for ListComponentTypesError {
    fn from(modeled: ListComponentTypesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListComponentTypes` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListComponentTypes<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListComponentTypes<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListComponentTypes<Eng>
{
    type Input = ListComponentTypesInput;
    type Output = ListComponentTypesOutput;
    type Error = ListComponentTypesError;

    const NAME: &'static str = "ListComponentTypes";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_COMPONENT_TYPES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_COMPONENT_TYPES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Every call is recorded, successful or refused: the real service logs
        // control-plane access, and a page served is a read of tenant state.
        ::tracing::info!(
            operation = "ListComponentTypes",
            workspace_id = %input.workspace_id,
            "serving a request"
        );
        // The Region this request is signed for picks the store that answers
        // it: a workspace created through another Region's endpoint is not in
        // it, so neither are its component types.
        let region = crate::state::request_region(ctx.parts());
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListComponentTypesErrorModeled::InternalServerException(InternalServerException {
                message: Some("the iottwinmaker store is unavailable".to_string()),
            })
        })?;
        let mut state = state.lock();
        let state = state.region_mut(&region);

        let invalid = |message: String| {
            ListComponentTypesErrorModeled::ValidationException(ValidationException {
                message: Some(message),
            })
        };

        // The SCOPE is resolved before the request's own members, so a listing
        // of a workspace this service does not hold is refused for the workspace
        // whatever else the request carries — the order `ListEntities` resolves
        // them in. The mutable borrow is taken here because reporting a
        // component type settles it (below); nothing between here and there
        // writes.
        let Some(workspace) = state.workspaces.get_mut(&input.workspace_id) else {
            return Err(invalid(crate::state::workspace_not_found_message(
                &input.workspace_id,
            ))
            .into());
        };

        // "Only one object is accepted as a valid input" — the
        // `ListComponentTypesFilter` union's own documentation.
        let filters = input.filters.as_deref().unwrap_or_default();
        if filters.len() > 1 {
            return Err(invalid(format!(
                "filters accepts at most one object, got {}",
                filters.len()
            ))
            .into());
        }
        let filter = filters.first();

        // The filter decides WHICH component types are in the sequence a page is
        // of, so it is part of what the token below is bound to — and is
        // rendered before the token is read, since reading one is deciding
        // whether it belongs to THIS sequence.
        let filters_scope = filters_token_scope(filter);

        // A `nextToken` carries the `componentTypeId` the page resumes AT
        // (inclusive), not an offset, so a page boundary survives a component
        // type being created or deleted between two pages. It is bound to the
        // Region, the workspace AND the filter the page was taken under, so a
        // token from another listing — or from this listing taken over another
        // workspace, over the independent workspace another Region's endpoint
        // holds under the same id, or under another filter, whose sequence this
        // one's resume key names no position in — is refused rather than
        // silently resumed past every row the new sequence holds ahead of it.
        let resume_at = match input.next_token.as_deref() {
            None | Some("") => None,
            Some(token) => Some(
                decode_next_token(token, &region, &input.workspace_id, &filters_scope)
                    .ok_or_else(|| invalid(crate::state::invalid_next_token_message()))?,
            ),
        };

        // The page size served, under the rule every paginated operation of this
        // service shares (`crate::state::page_size`): THIS member documents a
        // minimum of 1 and the service's default of 25, so an explicit zero is
        // refused and a request that names none is served that default — never
        // the whole workspace. Whatever is asked for, the service serves at most
        // `crate::state::MAX_PAGE_SIZE`, so the page this call BUILDS is bounded
        // by that code and not only by the boundary validator.
        let page_size = crate::state::page_size(
            input.max_results,
            crate::state::PageSizeRule::minimum_one(crate::state::DEFAULT_PAGE_SIZE),
        )
        .map_err(invalid)?;

        // The clock, not this call, is what moves a component type on: the
        // transition was due at `settles_at`, so reporting the stored state
        // without materialising it would report a state that stopped being true
        // before the request arrived. Done for every component type of the
        // workspace rather than only the reported page, so two callers reading
        // different pages cannot disagree about one component type's state.
        // This pass is therefore O(the workspace's component types) under the
        // lock — deliberately, for that consistency — and bounded by
        // `crate::state::MAX_COMPONENT_TYPES_PER_WORKSPACE`, the ceiling the
        // create enforces, rather than by the page size.
        let now = ::std::time::SystemTime::now();
        for component_type in workspace.component_types.values_mut() {
            component_type.settle(now);
        }

        // `component_types` is keyed by `componentTypeId`, so iterating the map
        // IS id order and resuming is a `range` from the token's key: the page
        // starts at the first id at or past it without walking what came before.
        let mut matching = match &resume_at {
            Some(from) => workspace.component_types.range(from.clone()..),
            None => workspace.component_types.range::<String, _>(..),
        }
        .map(|(_, component_type)| component_type)
        .filter(|component_type| matches_filter(component_type, filter));

        let mut component_type_summaries = Vec::new();
        for component_type in matching.by_ref().take(page_size) {
            component_type_summaries.push(ComponentTypeSummary {
                arn: component_type.arn.clone(),
                component_type_id: component_type.component_type_id.clone(),
                creation_date_time: crate::state::timestamp(component_type.creation_date_time),
                update_date_time: crate::state::timestamp(component_type.update_date_time),
                // Reported exactly as stored, which is how `GetComponentType`
                // reports them: a component type that was created without a
                // description or a name carries neither, and the member is
                // omitted rather than served as an empty string.
                description: component_type.description.clone(),
                component_type_name: component_type.component_type_name.clone(),
                // Carries BOTH its modelled members: the stored state (settled
                // above) and the `error` structure every reported status
                // carries.
                status: Some(component_type.status()),
            });
        }
        // The token the next call resumes at: the first component type this page
        // left over. Absent on the last page, which is how a caller knows to
        // stop.
        let next_token = matching.next().map(|component_type| {
            encode_next_token(
                &region,
                &input.workspace_id,
                &filters_scope,
                &component_type.component_type_id,
            )
        });

        Ok(ListComponentTypesOutput {
            workspace_id: input.workspace_id.clone(),
            // Required by the model, so present even for a workspace holding no
            // component type (or one whose filter selects none): an empty list
            // says there is none.
            component_type_summaries,
            next_token,
            // "Specifies the maximum number of results to display" — the page
            // size this call actually served at, which is what the caller asked
            // for once the service's own ceiling and the documented default have
            // been applied.
            max_results: i32::try_from(page_size).ok(),
        })
    }
}

/// Whether `component_type` is in the list the single accepted filter selects.
/// With no filter every component type of the workspace is.
fn matches_filter(
    component_type: &crate::state::ComponentType,
    filter: Option<&ListComponentTypesFilter>,
) -> bool {
    match filter {
        None => true,
        // "The component type that the component types in the list extend":
        // the parents the create declared, which are exactly the ones
        // `GetComponentType` reports for this component type.
        Some(ListComponentTypesFilter::extendsFrom(parent)) => component_type
            .extends_from
            .iter()
            .any(|declared| declared == parent),
        // "The namespace to which the component types in the list belong": a
        // `componentTypeId` is a dotted name whose namespace is its dotted
        // prefix, so the type belongs to the namespace when its id continues it
        // with a `.`.
        Some(ListComponentTypesFilter::Namespace(namespace)) => component_type
            .component_type_id
            .strip_prefix(namespace.as_str())
            .is_some_and(|rest| rest.starts_with('.') && rest.len() > 1),
        // The abstractness the record carries, read through the same field
        // `GetComponentType` reports from, so the filter and the read cannot
        // drift apart: the writers decide it from what the component type
        // holds (`crate::state::component_type_is_abstract`), and a built-in
        // carries the abstractness the service publishes for it.
        Some(ListComponentTypesFilter::isAbstract(is_abstract)) => {
            component_type.is_abstract == *is_abstract
        }
    }
}

/// The envelope a `nextToken` of this listing carries, so a token minted
/// elsewhere — by another listing of this service, for another workspace, or by
/// hand — is recognisably not one of ours. The token itself is the shape every
/// paginated operation of this service mints
/// ([`crate::state::encode_page_token`]), scoped to the Region and the workspace
/// whose page it is and to the filter that selected it.
const NEXT_TOKEN_PREFIX: &str = "iottwinmaker:componentTypes:1:";

/// What the single accepted filter binds a token to: the member it filters on
/// and the value it selects, under the one rule every filtered listing of this
/// service binds its filters by ([`crate::state::filters_token_scope`]).
///
/// The filter is what decides which component types are in the listing at all
/// ([`matches_filter`]), so a page resumed under a different one would skip
/// every component type the new filter selects that sorts before the resume key.
/// A request naming no filter has its own scope, distinct from any filtered one:
/// the unfiltered sequence is a different sequence.
fn filters_token_scope(filter: Option<&ListComponentTypesFilter>) -> String {
    crate::state::filters_token_scope(filter.map(|filter| match filter {
        ListComponentTypesFilter::extendsFrom(parent) => {
            ("extendsFrom", ::serde_json::Value::from(parent.as_str()))
        }
        ListComponentTypesFilter::Namespace(namespace) => {
            ("namespace", ::serde_json::Value::from(namespace.as_str()))
        }
        ListComponentTypesFilter::isAbstract(is_abstract) => {
            ("isAbstract", ::serde_json::Value::from(*is_abstract))
        }
    }))
}

/// The top of the `maxResults` member's modelled `range` (0..200).
///
/// The response reports the page size it served in its own `maxResults`, so the
/// page size the service is willing to serve may never leave the range the model
/// admits. Checked at COMPILE time, so raising this service's own ceiling past
/// what the model allows fails the build instead of serialising an out-of-range
/// value.
const MODELLED_MAX_RESULTS: usize = 200;
const _: () = assert!(crate::state::MAX_PAGE_SIZE <= MODELLED_MAX_RESULTS);
const _: () = assert!(crate::state::DEFAULT_PAGE_SIZE <= MODELLED_MAX_RESULTS);

/// The opaque `nextToken` for a page of `region`'s `workspace_id` taken under
/// `filters_scope`, starting at `component_type_id`.
fn encode_next_token(
    region: &str,
    workspace_id: &str,
    filters_scope: &str,
    component_type_id: &str,
) -> String {
    crate::state::encode_page_token(
        NEXT_TOKEN_PREFIX,
        region,
        &[workspace_id, filters_scope],
        component_type_id,
    )
}

/// The component type id inside a `nextToken` minted in `region` for
/// `workspace_id` under `filters_scope`, or `None` when the token is not one
/// this listing minted for that Region's workspace and that filter.
fn decode_next_token(
    token: &str,
    region: &str,
    workspace_id: &str,
    filters_scope: &str,
) -> Option<String> {
    crate::state::decode_page_token(
        NEXT_TOKEN_PREFIX,
        region,
        &[workspace_id, filters_scope],
        token,
    )
}
