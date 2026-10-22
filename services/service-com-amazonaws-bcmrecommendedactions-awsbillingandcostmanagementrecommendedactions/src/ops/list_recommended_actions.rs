// SPDX-License-Identifier: CC0-1.0

//! @op ListRecommendedActions
//!
//! Reports the recommended actions held in [`crate::state`], narrowed by the
//! request's `filter` and cut into pages by `maxResults`/`nextToken`. This is the
//! ONLY file to touch for this operation.
//!
//! ## What this operation does
//!
//! * **Reporting only.** The model binds this single operation and its doc says it
//!   "returns a list of recommended actions"; it never writes, so two identical
//!   calls answer identically and a later caller sees exactly what an earlier one
//!   saw. The records come from the store, which is filled through
//!   [`crate::state::RecommendedActionsStore::seed`] — the service has no create
//!   operation because AWS itself computes the recommendations.
//! * **Every account the caller can see.** `RecommendedAction$accountId` is "the
//!   account that the recommended action is for", and `FilterName` has no account
//!   key — the real API reports the actions of the calling (management) account AND
//!   of the member accounts it pays for, which is why each record names its own
//!   account. So the store holds exactly the set this caller may see and the handler
//!   reports all of it; it does not re-filter by the caller's own account, which
//!   would hide every member-account record the shape exists to carry. This emulator
//!   presents one account id ([`engine::emulator_account_id`]), so a record is
//!   normally seeded with that id.
//! * **Authorization is deliberately unmodelled.** The emulator verifies every
//!   request signature at its boundary and holds no per-principal policy, so this
//!   handler never evaluates permissions and the modelled `AccessDeniedException`
//!   (403) stays constructible but unreached; read the silence as "not modelled",
//!   not as an enforced allow.
//! * **Filtering.** `filter.actions` is a list of `ActionFilter`s, each naming a
//!   record field (`FEATURE` / `SEVERITY` / `TYPE`), a `matchOption` (`EQUALS`
//!   keeps the records whose field is one of `values`, `NOT_EQUALS` excludes them)
//!   and the `values` to compare against. Several filters are ANDed. A filter
//!   value that is not a member of the enum its `key` names is refused with
//!   `ValidationException`, and so is an empty `values` list, whose modelled
//!   minimum length is 1.
//! * **Pagination.** Records are reported in store (insertion) order, so a page
//!   boundary is stable. `nextToken` is opaque: a 128-bit fingerprint of the
//!   effective query (the filter plus `maxResults`) followed by the offset to
//!   resume at. It is emitted only while unreported records remain, and a token
//!   that does not parse, or that was issued for a different query, is refused
//!   with `ValidationException` rather than silently restarting the listing.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `ListRecommendedActions::new(engine)` and drives it through the centralized request walk:
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
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListRecommendedActions`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bcmrecommendedactions-awsbillingandcostmanagementrecommendedactions/smithy-model/com-amazonaws-bcmrecommendedactions-awsbillingandcostmanagementrecommendedactions.json")
//! ```
//!
//! Returns a list of recommended actions that match the filter criteria.
//!
//! ## Input
//!
//! * `filter` (RequestFilter) — The criteria that you want all returned recommended actions
//!   to match.
//! * `maxResults` (MaxResults, range 1..1000) — The maximum number of results to return in
//!   the response.
//! * `nextToken` (NextToken, len 1..8192, pattern ^[\S\s]*$) — The pagination token that
//!   indicates the next set of results that you want to retrieve.
//!
//! ## Output
//!
//! * `nextToken` (NextToken, len 1..8192, pattern ^[\S\s]*$) — The pagination token that
//!   indicates the next set of results that you want to retrieve.
//! * `recommendedActions` (RecommendedActions) **required** — The list of recommended actions
//!   that satisfy the filter criteria.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient access to perform this
//!   action.
//! * `InternalServerException` (HTTP 500) — An unexpected error occurred during the
//!   processing of your request.
//! * `ThrottlingException` (HTTP 429) — The request was denied due to request throttling.
//! * `ValidationException` (HTTP 400) — The input fails to satisfy the constraints specified
//!   by an Amazon Web Services service.

#[allow(unused_imports)]
use super::types::*;
use crate::state::{FilterName, MatchOption, StoredAction};
use sha2::Digest;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListRecommendedActionsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bcmrecommendedactions-awsbillingandcostmanagementrecommendedactions.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListRecommendedActionsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bcmrecommendedactions#ListRecommendedActions")]
#[derive(Debug)]
pub enum ListRecommendedActionsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_recommended_actions::ListRecommendedActionsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListRecommendedActionsError = crate::scaffolded::Scaffolded<ListRecommendedActionsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListRecommendedActionsErrorModeled> for ListRecommendedActionsError {
    fn from(modeled: ListRecommendedActionsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// `ValidationExceptionReason$cannotParse` — the request carries a value this
/// service could not read at all (here: a malformed `nextToken`).
const REASON_CANNOT_PARSE: &str = "cannotParse";
/// `ValidationExceptionReason$fieldValidationFailed` — a named member carries a
/// value outside what the service accepts.
const REASON_FIELD_VALIDATION_FAILED: &str = "fieldValidationFailed";

/// The modelled `MaxResults` range (`smithy_query(query="shape",
/// shape="MaxResults")`: range 1..1000).
const MAX_RESULTS_RANGE: (i32, i32) = (1, 1000);

/// Hex characters of the query fingerprint a `nextToken` carries (128 bits).
const TOKEN_FINGERPRINT_LEN: usize = 32;
/// Hex characters of the resume offset a `nextToken` carries.
const TOKEN_OFFSET_LEN: usize = 8;
/// Total length of a `nextToken` this service mints.
const TOKEN_LEN: usize = TOKEN_FINGERPRINT_LEN + TOKEN_OFFSET_LEN;

/// A 400 `ValidationException` with the modelled `reason` and, when a member is to
/// blame, the `fieldList` entry naming it.
fn validation(
    message: impl Into<String>,
    reason: &str,
    field: Option<(String, String)>,
) -> ListRecommendedActionsError {
    ListRecommendedActionsErrorModeled::ValidationException(ValidationException {
        message: message.into(),
        reason: reason.to_string(),
        field_list: field.map(|(name, message)| vec![ValidationExceptionField { name, message }]),
    })
    .into()
}

/// A 500 `InternalServerException`: the service could not reach its own store.
fn internal_server_error(message: impl Into<String>) -> ListRecommendedActionsError {
    ListRecommendedActionsErrorModeled::InternalServerException(InternalServerException {
        message: message.into(),
    })
    .into()
}

/// The wire members of an enum, for a refusal message.
fn member_list<E: std::fmt::Display>(all: &[E]) -> String {
    all.iter()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// One validated `ActionFilter`: the field it reads, whether it keeps or excludes a
/// match, and the values it compares against.
#[derive(Debug)]
struct CompiledFilter {
    key: FilterName,
    match_option: MatchOption,
    values: Vec<String>,
}

impl CompiledFilter {
    /// Whether this filter keeps `action`.
    fn keeps(&self, action: &StoredAction) -> bool {
        let field = match self.key {
            FilterName::Feature => action.feature.as_str(),
            FilterName::Severity => action.severity.as_str(),
            FilterName::Type => action.action_type.as_str(),
        };
        let matched = self.values.iter().any(|value| value == field);
        match self.match_option {
            MatchOption::Equals => matched,
            MatchOption::NotEquals => !matched,
        }
    }

    /// This filter's contribution to the query fingerprint. Values are sorted and
    /// de-duplicated because a filter tests set membership, so `[A, B]` and
    /// `[B, A, B]` are the same query.
    fn canonical(&self) -> String {
        let mut values: Vec<&str> = self.values.iter().map(String::as_str).collect();
        values.sort_unstable();
        values.dedup();
        format!(
            "{}:{}:{}",
            self.key.as_str(),
            self.match_option.as_str(),
            values.join(",")
        )
    }
}

/// Validate the request's filters, refusing a key, match option or value the model
/// does not declare.
///
/// `ActionFilter$key` and `$matchOption` target enum shapes, so the framework's
/// constraint validation refuses an unmodelled member of either before this
/// handler runs; the checks here are the backstop that turns such a value into this
/// service's own `ValidationException` rather than a panic or a silent default.
/// `ActionFilter$values` is a list of plain strings, so the enum its `key` names is
/// enforced ONLY here.
fn compile_filters(
    filter: Option<&RequestFilter>,
) -> Result<Vec<CompiledFilter>, ListRecommendedActionsError> {
    let Some(actions) = filter.and_then(|filter| filter.actions.as_ref()) else {
        return Ok(Vec::new());
    };

    let mut compiled = Vec::with_capacity(actions.len());
    for (index, action_filter) in actions.iter().enumerate() {
        let key = FilterName::from_wire(&action_filter.key).ok_or_else(|| {
            validation(
                format!(
                    "'{}' is not a valid filter key; expected one of {}",
                    action_filter.key,
                    member_list(FilterName::ALL)
                ),
                REASON_FIELD_VALIDATION_FAILED,
                Some((
                    format!("filter.actions[{index}].key"),
                    format!("expected one of {}", member_list(FilterName::ALL)),
                )),
            )
        })?;
        let match_option = MatchOption::from_wire(&action_filter.match_option).ok_or_else(|| {
            validation(
                format!(
                    "'{}' is not a valid match option; expected one of {}",
                    action_filter.match_option,
                    member_list(MatchOption::ALL)
                ),
                REASON_FIELD_VALIDATION_FAILED,
                Some((
                    format!("filter.actions[{index}].matchOption"),
                    format!("expected one of {}", member_list(MatchOption::ALL)),
                )),
            )
        })?;
        // `ActionFilter$values` is `FilterValues`, whose modelled length is `min: 1`
        // (`smithy_query(query="shape", shape="ActionFilter,FilterValues")`), so an
        // empty list is refused rather than evaluated: `any()` over no value is
        // `false`, which would silently turn an `EQUALS` filter into "matches
        // nothing" and a `NOT_EQUALS` filter into a no-op. Same backstop role as the
        // `maxResults` range check below.
        if action_filter.values.is_empty() {
            return Err(validation(
                "filter.actions[].values must carry at least one value",
                REASON_FIELD_VALIDATION_FAILED,
                Some((
                    format!("filter.actions[{index}].values"),
                    "expected at least one value".to_string(),
                )),
            ));
        }
        for (value_index, value) in action_filter.values.iter().enumerate() {
            if !key.accepts_value(value) {
                return Err(validation(
                    format!(
                        "'{}' is not a member of {}, which filter key {} filters on",
                        value,
                        key.value_enum_name(),
                        key.as_str()
                    ),
                    REASON_FIELD_VALIDATION_FAILED,
                    Some((
                        format!("filter.actions[{index}].values[{value_index}]"),
                        format!("expected a member of {}", key.value_enum_name()),
                    )),
                ));
            }
        }
        compiled.push(CompiledFilter {
            key,
            match_option,
            values: action_filter.values.clone(),
        });
    }
    Ok(compiled)
}

/// The fingerprint of an effective query: the filters (order-independent, since
/// they are ANDed) plus the page size. A `nextToken` carries it so a token cannot
/// resume a listing of a different query.
fn query_fingerprint(filters: &[CompiledFilter], max_results: Option<i32>) -> String {
    let mut parts: Vec<String> = filters.iter().map(CompiledFilter::canonical).collect();
    parts.sort();
    let canonical = format!(
        "maxResults={}|filters={}",
        max_results.map_or_else(|| "none".to_string(), |value| value.to_string()),
        parts.join("&")
    );
    let digest = hex::encode(sha2::Sha256::digest(canonical.as_bytes()));
    digest[..TOKEN_FINGERPRINT_LEN].to_string()
}

/// The opaque token that resumes this query at `offset`.
fn mint_token(fingerprint: &str, offset: usize) -> String {
    format!("{fingerprint}{offset:0width$x}", width = TOKEN_OFFSET_LEN)
}

/// The offset `token` resumes at, or a `ValidationException` when it is not a token
/// this service minted for this query.
fn parse_token(token: &str, fingerprint: &str) -> Result<usize, ListRecommendedActionsError> {
    let malformed = || {
        validation(
            "The nextToken is not valid. Pass back the nextToken returned by the previous call, unchanged.",
            REASON_CANNOT_PARSE,
            Some((
                "nextToken".to_string(),
                "not a pagination token issued by this service".to_string(),
            )),
        )
    };
    // A minted token is lower-case hex throughout, so anything else — including an
    // upper-cased copy of a real token — is a token this service did not mint, and is
    // reported as unreadable rather than as a token for another query.
    if token.len() != TOKEN_LEN || !token.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) {
        return Err(malformed());
    }
    let (token_fingerprint, offset) = token.split_at(TOKEN_FINGERPRINT_LEN);
    if token_fingerprint != fingerprint {
        return Err(validation(
            "The nextToken was issued for a different request. Repeat the original filter and maxResults when paginating.",
            REASON_FIELD_VALIDATION_FAILED,
            Some((
                "nextToken".to_string(),
                "issued for a different filter or maxResults".to_string(),
            )),
        ));
    }
    usize::from_str_radix(offset, 16).map_err(|_| malformed())
}

/// Project a stored record onto the wire `RecommendedAction`.
fn to_wire(action: &StoredAction) -> RecommendedAction {
    RecommendedAction {
        id: Some(action.id.clone()),
        r#type: Some(action.action_type.as_str().to_string()),
        account_id: Some(action.account_id.clone()),
        severity: Some(action.severity.as_str().to_string()),
        feature: Some(action.feature.as_str().to_string()),
        context: Some(
            action
                .context
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        ),
        next_steps: Some(action.next_steps.clone()),
        last_updated_time_stamp: Some(action.last_updated_timestamp.clone()),
    }
}

/// The `ListRecommendedActions` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListRecommendedActions<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListRecommendedActions<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListRecommendedActions<Eng>
{
    type Input = ListRecommendedActionsInput;
    type Output = ListRecommendedActionsOutput;
    type Error = ListRecommendedActionsError;

    const NAME: &'static str = "ListRecommendedActions";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_RECOMMENDED_ACTIONS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_RECOMMENDED_ACTIONS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;

        // Refuse a bad filter BEFORE touching the store, so a rejected request is
        // refused on its own terms whatever the store holds.
        let filters = compile_filters(input.filter.as_ref())?;
        let fingerprint = query_fingerprint(&filters, input.max_results);

        // `maxResults` absent means "no page limit": report every matching record.
        // AWS documents no default page size for this operation, and the model's
        // `MaxResults` range (1..1000) is refused by the framework's constraint
        // validation before this handler runs; the same bound is enforced here as a
        // backstop, because a page size of 0 would otherwise report an empty page while
        // still owing a `nextToken` — a listing that never terminates. The range check
        // and the conversion share ONE refusal, so neither can fall through to a
        // silently unbounded page.
        let page_size = match input.max_results {
            Some(max_results) => usize::try_from(max_results)
                .ok()
                .filter(|_| (MAX_RESULTS_RANGE.0..=MAX_RESULTS_RANGE.1).contains(&max_results))
                .ok_or_else(|| {
                    validation(
                        format!(
                            "maxResults must be between {} and {}, got {max_results}",
                            MAX_RESULTS_RANGE.0, MAX_RESULTS_RANGE.1
                        ),
                        REASON_FIELD_VALIDATION_FAILED,
                        Some((
                            "maxResults".to_string(),
                            format!(
                                "expected a value between {} and {}",
                                MAX_RESULTS_RANGE.0, MAX_RESULTS_RANGE.1
                            ),
                        )),
                    )
                })?,
            None => usize::MAX,
        };

        let resume_at = match input.next_token.as_deref() {
            Some(token) => parse_token(token, &fingerprint)?,
            None => 0,
        };

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            internal_server_error(
                "the recommended-actions store is unavailable on this engine".to_string(),
            )
        })?;

        // The matching records, in store (insertion) order, so the page boundary a
        // token names stays on the same record across calls.
        let matching: Vec<RecommendedAction> = {
            let guard = state.lock();
            guard
                .recommended_actions
                .actions()
                .iter()
                .filter(|action| filters.iter().all(|filter| filter.keeps(action)))
                .map(to_wire)
                .collect()
        };

        // A token can outlive the records it was minted over (the service recomputes
        // its recommendations), so a resume point past the end reports an empty final
        // page rather than failing a caller that is simply done.
        let start = resume_at.min(matching.len());
        let end = start.saturating_add(page_size).min(matching.len());
        let page = matching[start..end].to_vec();
        // A token is owed only when this page made progress AND records remain, so no
        // answer can ever hand back a token that would repeat the same page.
        let next_token =
            (end > start && end < matching.len()).then(|| mint_token(&fingerprint, end));

        Ok(ListRecommendedActionsOutput {
            recommended_actions: page,
            next_token,
        })
    }
}
