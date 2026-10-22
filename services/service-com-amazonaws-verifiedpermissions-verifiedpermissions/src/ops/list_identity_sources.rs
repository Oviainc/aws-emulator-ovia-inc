// SPDX-License-Identifier: CC0-1.0

//! @op ListIdentitySources
//!
//! `@readonly`, `@paginated(items=identitySources, inputToken/outputToken=nextToken,
//! pageSize=maxResults)`: enumerates the store's persisted identity sources in a
//! stable order, applies `filters`, truncates at `maxResults` and hands back an
//! opaque `nextToken` that resumes exactly where the page stopped. It writes
//! nothing.
//!
//! Each reported item is the SAME view of the stored identity source that
//! [`crate::ops::get_identity_source`] reports: `IdentitySourceItem`'s
//! `configuration` and deprecated `details` are member-identical to
//! `GetIdentitySourceOutput`'s, so this handler re-types that one view instead of
//! deciding a second time what a read reports.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListIdentitySources`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json")
//! ```
//!
//! Returns a paginated list of all of the identity sources defined in the specified policy
//! store.
//!
//! ## Input
//!
//! * `filters` (IdentitySourceFilters, len 0..10) — Specifies characteristics of an identity
//!   source that you can use to limit the output to matching identity sources.
//! * `maxResults` (ListIdentitySourcesMaxResults, range 1..) — Specifies the total number of
//!   results that you want included in each response. If additional items exist beyond the
//!   number you specify, the NextToken response element is returned with a value (not null).
//!   Include the specified value as the NextToken request parameter in the next call to the
//!   operation to get the next set of results. Note that the service might return fewer
//!   results than the maximum even when there are more results available. You should check
//!   NextToken after every operation to ensure that you receive all of the results. If you do
//!   not specify this parameter, the operation defaults to 10 identity sources per response.
//!   You can specify a maximum of 50 identity sources per response.
//! * `nextToken` (NextToken, len 1..8000, pattern ^[A-Za-z0-9-_=+/\.]*$) — Specifies that you
//!   want to receive the next page of results. Valid only if you received a NextToken
//!   response in the previous request. If you did, it indicates that more output is
//!   available. Set this parameter to the value provided by the previous call's NextToken
//!   response to request the next page of results.
//! * `policyStoreId` (PolicyStoreId, len 1..200, pattern ^[a-zA-Z0-9-/_]*$) **required** —
//!   Specifies the ID of the policy store that contains the identity sources that you want to
//!   list. To specify a policy store, use its ID or alias name. When using an alias name,
//!   prefix it with policy-store-alias/.
//!
//! ## Output
//!
//! * `identitySources` (IdentitySources) **required** — The list of identity sources stored
//!   in the specified policy store.
//! * `nextToken` (NextToken, len 1..8000, pattern ^[A-Za-z0-9-_=+/\.]*$) — If present, this
//!   value indicates that more output is available than is included in the current response.
//!   Use this value in the NextToken request parameter in a subsequent call to the operation
//!   to get the next part of the output.
//!
//! ## Errors
//!
//! * `ResourceNotFoundException` (HTTP 404) — The request failed because it references a
//!   resource that doesn't exist.
//! * `ValidationException` (HTTP 400) — a `nextToken` this service did not mint.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListIdentitySourcesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

/// The operation's MODELED errors. `InternalServerException` and
/// `ValidationException` are service-level errors this operation's own `errors`
/// list does not repeat, so they come in through `extra_errors`: the first is the
/// honest answer when the state store cannot be reached, the second refuses a
/// cursor this service did not mint.
#[smithy_macros::operation_error(
    "com.amazonaws.verifiedpermissions#ListIdentitySources",
    extra_errors = ["InternalServerException", "ValidationException"]
)]
#[derive(Debug)]
pub enum ListIdentitySourcesErrorModeled {
    ResourceNotFoundException(ResourceNotFoundException),
    InternalServerException(InternalServerException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_identity_sources::ListIdentitySourcesError`).
pub type ListIdentitySourcesError = crate::scaffolded::Scaffolded<ListIdentitySourcesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant.
impl From<ListIdentitySourcesErrorModeled> for ListIdentitySourcesError {
    fn from(modeled: ListIdentitySourcesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListIdentitySources` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListIdentitySources<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListIdentitySources<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListIdentitySources<Eng>
{
    type Input = ListIdentitySourcesInput;
    type Output = ListIdentitySourcesOutput;
    type Error = ListIdentitySourcesError;

    const NAME: &'static str = "ListIdentitySources";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_IDENTITY_SOURCES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_IDENTITY_SOURCES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let not_found = || {
            ListIdentitySourcesErrorModeled::ResourceNotFoundException(
                crate::state::policy_store_reference(&input.policy_store_id).not_found(),
            )
        };

        // A cursor this service did not mint is a bad request, not an empty page:
        // answering an empty page would hide the client's mistake.
        let resume_from = match input.next_token.as_deref() {
            None => None,
            Some(token) => Some(crate::state::decode_page_token(token).ok_or_else(|| {
                ListIdentitySourcesErrorModeled::ValidationException(
                    crate::ops::validation_exception([crate::ops::InvalidInput::new(
                        crate::ops::paths::NEXT_TOKEN,
                        "it is not a cursor this service issued",
                    )]),
                )
            })?),
        };

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListIdentitySourcesErrorModeled::InternalServerException(InternalServerException {
                message: "the service state store is unavailable".to_string(),
            })
        })?;
        let state = state.lock();
        let store_id = state
            .resolve_store_id(&input.policy_store_id)
            .ok_or_else(not_found)?;
        let store = state.policy_stores.get(&store_id).ok_or_else(not_found)?;

        // `identity_sources` is a `BTreeMap`, so the enumeration order is stable
        // across calls and a cursor resumes at the first key at or after it.
        let matching = store
            .identity_sources
            .iter()
            .filter(|(source_id, _)| {
                resume_from
                    .as_ref()
                    .is_none_or(|from| source_id.as_str() >= from.as_str())
            })
            .filter(|(_, source)| matches_filters(source, input.filters.as_deref()));

        // No `maxResults` does NOT mean "the whole store in one response": the
        // operation is `@paginated`, so an absent page size takes this service's
        // own default and the caller pages for the rest. An asked-for size is
        // clamped to the maximum, so one small request can never materialise an
        // arbitrarily large response, and to at least one, so a page is
        // structurally non-empty: a zero page size would hand back an empty page
        // and a cursor that never advances, which is a client loop that never
        // ends. The model's `range min 1` already refuses zero at the boundary;
        // this does not depend on that.
        let page_size = input
            .max_results
            .map_or(crate::state::DEFAULT_PAGE_SIZE, |n| {
                usize::try_from(n)
                    .unwrap_or(crate::state::DEFAULT_PAGE_SIZE)
                    .clamp(1, crate::state::MAX_PAGE_SIZE)
            });
        let mut identity_sources: Vec<IdentitySourceItem> = Vec::new();
        let mut next_token = None;
        for (source_id, source) in matching {
            if identity_sources.len() == page_size {
                // The page stopped here, so the cursor names THIS key: the next
                // call resumes with it as its first candidate.
                next_token = Some(crate::state::encode_page_token(source_id));
                break;
            }
            identity_sources.push(identity_source_item(source));
        }

        Ok(ListIdentitySourcesOutput {
            next_token,
            identity_sources,
        })
    }
}

/// Does `source` pass `filters`?
///
/// `filters` is a LIST of characteristics (0..10 of them), each of which is a
/// structure of optional members, so the selection is a disjunction of
/// conjunctions: a source is reported when it matches ANY one filter, and it
/// matches a filter when every member that filter sets agrees with the source.
/// Conjoining the list instead would make every list of two filters naming
/// different values select nothing — one identity source has exactly one
/// `principalEntityType` — which is not a selection worth a modelled bound of
/// ten. An absent list, an empty list, and a filter that sets no member all
/// constrain nothing and so match everything.
fn matches_filters(
    source: &crate::state::IdentitySource,
    filters: Option<&[IdentitySourceFilter]>,
) -> bool {
    let Some(filters) = filters.filter(|filters| !filters.is_empty()) else {
        return true;
    };
    filters.iter().any(|filter| {
        filter
            .principal_entity_type
            .as_deref()
            .is_none_or(|entity_type| source.principal_entity_type == entity_type)
    })
}

/// A persisted identity source as the modeled `IdentitySourceItem`.
///
/// `configuration` and the deprecated `details` are whatever
/// [`crate::ops::get_identity_source`] reports for the same stored source,
/// re-typed into this operation's own shapes: `ConfigurationItem` and
/// `IdentitySourceItemDetails` are member-for-member the same as
/// `ConfigurationDetail` and `IdentitySourceDetails`, so there is ONE place that
/// decides what a read says about an identity source.
fn identity_source_item(source: &crate::state::IdentitySource) -> IdentitySourceItem {
    IdentitySourceItem {
        policy_store_id: source.policy_store_id.clone(),
        identity_source_id: source.identity_source_id.clone(),
        principal_entity_type: source.principal_entity_type.clone(),
        created_date: source.created_date,
        last_updated_date: source.last_updated_date,
        configuration: Some(configuration_item(
            super::get_identity_source::configuration_detail(&source.configuration),
        )),
        details: super::get_identity_source::details_for(&source.configuration).map(item_details),
    }
}

/// `ConfigurationDetail` as the `ConfigurationItem` this operation reports.
fn configuration_item(detail: ConfigurationDetail) -> ConfigurationItem {
    match detail {
        ConfigurationDetail::cognitoUserPoolConfiguration(cognito) => {
            ConfigurationItem::cognitoUserPoolConfiguration(CognitoUserPoolConfigurationItem {
                user_pool_arn: cognito.user_pool_arn,
                client_ids: cognito.client_ids,
                issuer: cognito.issuer,
                group_configuration: cognito.group_configuration.map(|groups| {
                    CognitoGroupConfigurationItem {
                        group_entity_type: groups.group_entity_type,
                    }
                }),
            })
        }
        ConfigurationDetail::openIdConnectConfiguration(oidc) => {
            ConfigurationItem::openIdConnectConfiguration(OpenIdConnectConfigurationItem {
                issuer: oidc.issuer,
                entity_id_prefix: oidc.entity_id_prefix,
                group_configuration: oidc.group_configuration.map(|groups| {
                    OpenIdConnectGroupConfigurationItem {
                        group_claim: groups.group_claim,
                        group_entity_type: groups.group_entity_type,
                    }
                }),
                token_selection: match oidc.token_selection {
                    OpenIdConnectTokenSelectionDetail::accessTokenOnly(access) => {
                        OpenIdConnectTokenSelectionItem::accessTokenOnly(
                            OpenIdConnectAccessTokenConfigurationItem {
                                principal_id_claim: access.principal_id_claim,
                                audiences: access.audiences,
                            },
                        )
                    }
                    OpenIdConnectTokenSelectionDetail::identityTokenOnly(identity) => {
                        OpenIdConnectTokenSelectionItem::identityTokenOnly(
                            OpenIdConnectIdentityTokenConfigurationItem {
                                principal_id_claim: identity.principal_id_claim,
                                client_ids: identity.client_ids,
                            },
                        )
                    }
                },
            })
        }
    }
}

/// `IdentitySourceDetails` as the `IdentitySourceItemDetails` this operation
/// reports. Both shapes are DEPRECATED ("replaced by ConfigurationItem") and
/// carry the same four members.
fn item_details(details: IdentitySourceDetails) -> IdentitySourceItemDetails {
    IdentitySourceItemDetails {
        client_ids: details.client_ids,
        user_pool_arn: details.user_pool_arn,
        discovery_url: details.discovery_url,
        open_id_issuer: details.open_id_issuer,
    }
}
