// SPDX-License-Identifier: CC0-1.0

//! @op ListTieringConfigurations
//!
//! Pages over the tiering configurations stored in [`crate::state`], each
//! projected through the ONE listing projection
//! ([`crate::state::TieringConfigurationRecord::listed`]), so a listed
//! configuration and the single-configuration read of it can never disagree.
//!
//! The page is `MaxResults` items plus a cursor to the next one, built the way
//! every paginated read of this service builds it
//! ([`crate::state::encode_page_token`]): the store is keyed by the name a
//! configuration is addressed by, so the walk is in name order and a cursor
//! resolves to the same position on every request.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListTieringConfigurations`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json")
//! ```
//!
//! Returns a list of tiering configurations.
//!
//! ## Input
//!
//! * `MaxResults` (MaxResults, range 1..1000) — The maximum number of items to be returned.
//! * `NextToken` (string) — The next item following a partial list of returned items. For
//!   example, if a request is made to return MaxResults number of items, NextToken allows you
//!   to return more items in your list starting at the location pointed to by the next token.
//!
//! ## Output
//!
//! * `NextToken` (string) — The next item following a partial list of returned items. For
//!   example, if a request is made to return MaxResults number of items, NextToken allows you
//!   to return more items in your list starting at the location pointed to by the next token.
//! * `TieringConfigurations` (TieringConfigurationsList) — An array of tiering configurations
//!   returned by the ListTieringConfigurations call.
//!
//! ## Errors
//!
//! * `InvalidParameterValueException` (HTTP 400) — Indicates that something is wrong with a
//!   parameter's value. For example, the value is out of range.
//! * `ServiceUnavailableException` (HTTP 500) — The request failed due to a temporary failure
//!   of the server.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListTieringConfigurationsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListTieringConfigurationsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.backup#ListTieringConfigurations")]
#[derive(Debug)]
pub enum ListTieringConfigurationsErrorModeled {
    InvalidParameterValueException(InvalidParameterValueException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_tiering_configurations::ListTieringConfigurationsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListTieringConfigurationsError = crate::scaffolded::Scaffolded<ListTieringConfigurationsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListTieringConfigurationsErrorModeled> for ListTieringConfigurationsError {
    fn from(modeled: ListTieringConfigurationsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListTieringConfigurations` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListTieringConfigurations<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListTieringConfigurations<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListTieringConfigurations<Eng>
{
    type Input = ListTieringConfigurationsInput;
    type Output = ListTieringConfigurationsOutput;
    type Error = ListTieringConfigurationsError;

    const NAME: &'static str = "ListTieringConfigurations";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_TIERING_CONFIGURATIONS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_TIERING_CONFIGURATIONS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        // A page size outside the model's `@range` is this service's own
        // `InvalidParameterValueException`, answered before anything is read.
        if let Some(refusal) = crate::state::rejected_max_results(input.max_results) {
            return Err(
                ListTieringConfigurationsErrorModeled::InvalidParameterValueException(refusal)
                    .into(),
            );
        }
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListTieringConfigurationsErrorModeled::ServiceUnavailableException(
                ServiceUnavailableException {
                    message: Some("the backup vault store is unavailable".to_string()),
                    ..Default::default()
                },
            )
        })?;
        let store = state.lock();
        // `tiering_configurations` is a `BTreeMap` keyed by the name, so this
        // walk is already in name order — which is what makes the name-valued
        // cursor below resolve to the same position on every request.
        let stored: Vec<&crate::state::TieringConfigurationRecord> =
            store.tiering_configurations.values().collect();

        let start = match input.next_token.as_deref() {
            None => 0,
            Some(token) => {
                let key = crate::state::decode_page_token(token);
                let position = key.and_then(|key| {
                    stored
                        .iter()
                        .position(|configuration| configuration.name == key)
                });
                let Some(position) = position else {
                    return Err(
                        ListTieringConfigurationsErrorModeled::InvalidParameterValueException(
                            InvalidParameterValueException {
                                message: Some(
                                    "NextToken does not name a page of this result set".to_string(),
                                ),
                                ..Default::default()
                            },
                        )
                        .into(),
                    );
                };
                position
            }
        };
        // No `MaxResults` means one page: the model's own maximum. A page size
        // the model's `@range` refuses was answered above, so the clamp here
        // only applies the default.
        let page_size = input
            .max_results
            .and_then(|max| usize::try_from(max).ok())
            .unwrap_or(crate::state::MAX_PAGE_SIZE)
            .clamp(1, crate::state::MAX_PAGE_SIZE);
        let end = start.saturating_add(page_size).min(stored.len());
        let next_token = stored
            .get(end)
            .map(|configuration| crate::state::encode_page_token(&configuration.name));

        let tiering_configurations: Vec<TieringConfigurationsListMember> = stored[start..end]
            .iter()
            .map(|configuration| configuration.listed())
            .collect();

        Ok(ListTieringConfigurationsOutput {
            tiering_configurations: Some(tiering_configurations),
            next_token,
        })
    }
}
