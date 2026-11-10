// SPDX-License-Identifier: CC0-1.0

//! @op DescribeInstanceInformation
//! Report the managed nodes this Region's Systems Manager endpoint holds — the ones that
//! registered with it — narrowed by the request's filters and served one page at a time.
//! This is the ONLY file to touch for this operation.
//!
//! The answer is computed from the node inventory in [`crate::state`]
//! (`ComAmazonawsSsmAmazonssmState::managed_nodes`), which a node enters only by
//! registering: an EC2 instance whose SSM Agent registers, or a hybrid node installed
//! with an activation code. Nothing here invents a node, so an account that has
//! registered none is reported as holding none — the same answer AWS gives for an
//! instance that never came under Systems Manager management. Nothing here writes
//! either: a report must not change what a later caller sees.
//!
//! ## Which requests are refused, and with which modelled error
//!
//! * A filter key the operation does not document — anything outside
//!   [`crate::state::INSTANCE_INFORMATION_FILTER_KEYS`] and the two tag forms — is a
//!   `ValidationException` naming the key the caller submitted
//!   ([`crate::state::filter_key_rejection`]), the same refusal the sibling
//!   `DescribeInstanceProperties` gives an unknown key: whether a filter key is one the
//!   operation knows is decided as a validation of the request, so the modelled
//!   `InvalidFilterKey` — which models no `message` member and so could not name the key —
//!   is not what a filter of this operation is refused with. The behavioural check this
//!   emulator is replayed against asserts exactly that for an unrecognised `Filters` key
//!   (the unknown-filter-key step of the managed-node filter check, recorded alongside the
//!   malformed-node-id one); the issue report that asked for the combination rule below
//!   assumed the modelled error instead, and the recording is what this follows.
//! * A tag filter combined with a filter on a property of a node is a
//!   `ValidationException` too: "Tag filters can't be combined with other filter types"
//!   (`DescribeInstanceInformationRequest$Filters`). BOTH keys are keys the operation
//!   accepts, so what is refused is their COMBINATION, not either key — two tag filters,
//!   and two property filters, are accepted
//!   ([`crate::state::tag_filter_combination_rejection`]).
//! * A value that could not be a value of the node attribute its key filters is
//!   `InvalidInstanceInformationFilterValue` ("The specified filter value isn't valid"),
//!   except for a node id, where the operation documents its own error: "If you specify
//!   a node ID that isn't valid … you receive an error" → `InvalidInstanceId`, carrying
//!   the constraint the value violated ([`crate::state::MANAGED_NODE_ID_PATTERN`]). A
//!   node id that is well formed but names no registered node is NOT a refusal: it simply
//!   matches nothing, which is how a caller asks whether an instance is managed.
//! * `Filters` and `InstanceInformationFilterList` together are a `ValidationException`:
//!   "Attempting to use InstanceInformationFilterList and Filters leads to an exception
//!   error" (`DescribeInstanceInformationRequest$InstanceInformationFilterList`), which
//!   this operation models no error for.
//! * A `NextToken` this service did not issue is `InvalidNextToken`.
//!
//! ## Filtering and paging
//!
//! Filters intersect and each filter's values union: a node is reported when, for every
//! filter, one of that filter's values is one of the node's values for that key. A page
//! holds `MaxResults` nodes (default 10, the modelled range 5..50), and the `NextToken`
//! is the id of the node the next page starts at — the same cursor the sibling
//! `DescribeInstanceProperties` hands out — so a page resumes at an id rather than at an
//! offset into a list that may have changed. That cursor is total because the inventory
//! is keyed by `(region, node id)` and therefore walked in ascending node-id order,
//! whatever order its nodes registered in
//! ([`crate::state::ComAmazonawsSsmAmazonssmState::managed_nodes_in_region`]).
//!
//! ## Nodes this report leaves out
//!
//! "This operation does not return information for nodes that are either Stopped or
//! Terminated" — an instance lifecycle state the inventory does not model, because the
//! inventory records REGISTRATIONS rather than instances and no operation of this
//! emulator registers or deregisters one yet (`DeregisterManagedInstance` is still a
//! scaffold, as is every other node API). So no node is dropped for its state here;
//! whatever registered is what is reported. The exclusion becomes implementable with the
//! registration path, which is that operation's own issue — inventing a lifecycle state
//! for a node nothing registered would be inventing the node.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `DescribeInstanceInformation::new(engine)` and drives it through the centralized request walk:
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
//! # Modeled contract for `DescribeInstanceInformation`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json")
//! ```
//!
//! Provides information about one or more of your managed nodes, including the operating
//! system platform, SSM Agent version, association status, and IP address. This operation
//! does not return information for nodes that are either Stopped or Terminated. If you
//! specify one or more node IDs, the operation returns information for those managed nodes.
//! If you don't specify node IDs, it returns information for all your managed nodes. If you
//! specify a node ID that isn't valid or a node that you don't own, you receive an error.
//! The IamRole field returned for this API operation is the role assigned to an Amazon EC2
//! instance configured with a Systems Manager Quick Setup host management configuration or
//! the role assigned to an on-premises managed node.
//!
//! ## Input
//!
//! * `Filters` (InstanceInformationStringFilterList, len 0..) — One or more filters. Use a
//!   filter to return a more specific list of managed nodes. You can filter based on tags
//!   applied to your managed nodes. Tag filters can't be combined with other filter types.
//!   Use this Filters data type instead of InstanceInformationFilterList, which is
//!   deprecated.
//! * `InstanceInformationFilterList` (InstanceInformationFilterList, len 0..) — This is a
//!   legacy method. We recommend that you don't use this method. Instead, use the Filters
//!   data type. Filters enables you to return node information by filtering based on tags
//!   applied to managed nodes. Attempting to use InstanceInformationFilterList and Filters
//!   leads to an exception error.
//! * `MaxResults` (MaxResultsEC2Compatible, range 5..50) — The maximum number of items to
//!   return for this call. The call also returns a token that you can specify in a subsequent
//!   call to get the next set of results. The default value is 10 items.
//! * `NextToken` (NextToken) — The token for the next set of items to return. (You received
//!   this token from a previous call.)
//!
//! ## Output
//!
//! * `InstanceInformationList` (InstanceInformationList) — The managed node information list.
//! * `NextToken` (NextToken) — The token to use when requesting the next set of items. If
//!   there are no additional items to return, the string is empty.
//!
//! ## Errors
//!
//! * `InternalServerError` (HTTP 500) — An error occurred on the server side.
//! * `InvalidFilterKey` (HTTP 400) — The specified key isn't valid.
//! * `InvalidInstanceId` (HTTP 400) — The following problems can cause this exception: You
//!   don't have permission to access the managed node. Amazon Web Services Systems Manager
//!   Agent (SSM Agent) isn't running. Verify that SSM Agent is running. SSM Agent isn't
//!   registered with the SSM endpoint. Try reinstalling SSM Agent. The managed node isn't in
//!   a valid state. Valid states are: Running, Pending, Stopped, and Stopping. Invalid states
//!   are: Shutting-down and Terminated.
//! * `InvalidInstanceInformationFilterValue` (HTTP 400) — The specified filter value isn't
//!   valid.
//! * `InvalidNextToken` (HTTP 400) — The specified token isn't valid.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeInstanceInformationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ssm-amazonssm.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeInstanceInformationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ssm#DescribeInstanceInformation")]
#[derive(Debug)]
pub enum DescribeInstanceInformationErrorModeled {
    InternalServerError(InternalServerError),
    InvalidFilterKey(InvalidFilterKey),
    InvalidInstanceId(InvalidInstanceId),
    InvalidInstanceInformationFilterValue(InvalidInstanceInformationFilterValue),
    InvalidNextToken(InvalidNextToken),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_instance_information::DescribeInstanceInformationError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type DescribeInstanceInformationError = crate::scaffolded::Scaffolded<DescribeInstanceInformationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<DescribeInstanceInformationErrorModeled> for DescribeInstanceInformationError {
    fn from(modeled: DescribeInstanceInformationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeInstanceInformation` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeInstanceInformation<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeInstanceInformation<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeInstanceInformation<Eng>
{
    type Input = DescribeInstanceInformationInput;
    type Output = DescribeInstanceInformationOutput;
    type Error = DescribeInstanceInformationError;

    const NAME: &'static str = "DescribeInstanceInformation";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_INSTANCE_INFORMATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_INSTANCE_INFORMATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        use crate::state::{
            filter_key_rejection, is_instance_information_filter_key, is_legacy_node_filter_key,
            managed_node_id_is_well_formed, node_page_size, request_region,
            tag_filter_combination_rejection, INVALID_NEXT_TOKEN, OPERATOR_EQUAL,
        };

        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(DescribeInstanceInformationErrorModeled::InternalServerError(
                InternalServerError {
                    message: Some(
                        "this service is mounted on an engine that holds no state".to_string(),
                    ),
                },
            )
            .into());
        };

        let string_filters = input.filters.as_deref().unwrap_or_default();
        let legacy_filters = input
            .instance_information_filter_list
            .as_deref()
            .unwrap_or_default();

        // "Attempting to use InstanceInformationFilterList and Filters leads to an
        // exception error" — the two members are alternatives, not a union, and the
        // refusal is the one both node describes state for the same request shape.
        if !string_filters.is_empty() && !legacy_filters.is_empty() {
            return Err(crate::scaffolded::Scaffolded::validation(
                crate::state::LEGACY_AND_NEW_FILTERS_REJECTION,
            ));
        }

        // A key outside the documented catalog names no attribute of a node, so the
        // filter cannot be served at all: the key is validated — and the request refused,
        // naming it — before anything is asked about the nodes.
        for filter in string_filters {
            if !is_instance_information_filter_key(&filter.key) {
                return Err(crate::scaffolded::Scaffolded::validation(
                    filter_key_rejection(&filter.key),
                ));
            }
        }

        // "Tag filters can't be combined with other filter types." Every key above is one
        // the operation accepts, so a tag filter beside a filter on a property of a node
        // is a combination the request may not express — a validation of the request,
        // rather than a key the operation does not know.
        if let Some(why) = tag_filter_combination_rejection(
            string_filters.iter().map(|filter| filter.key.as_str()),
        ) {
            return Err(crate::scaffolded::Scaffolded::validation(why));
        }

        for filter in string_filters {
            check_filter_values(&filter.key, &filter.values)?;
        }
        for filter in legacy_filters {
            // The legacy list's key is the modelled `InstanceInformationFilterKey` enum,
            // which carries no tag form: "The InstanceInformationFilterList method is a
            // legacy method and doesn't support tags". The boundary validator refuses a
            // key outside that enum before this handler runs, so this is a backstop.
            if !is_legacy_node_filter_key(&filter.key) {
                return Err(crate::scaffolded::Scaffolded::validation(
                    filter_key_rejection(&filter.key),
                ));
            }
            check_filter_values(&filter.key, &filter.value_set)?;
        }

        let page_size = node_page_size(input.max_results)
            .map_err(crate::scaffolded::Scaffolded::validation)?;

        // The token this operation hands out is the id of the node the next page starts
        // at — the sibling node describe's cursor — so a token that is not shaped like a
        // node id was not one of ours.
        let resume_from = match input.next_token.as_deref() {
            None | Some("") => None,
            Some(token) if managed_node_id_is_well_formed(token) => Some(token),
            Some(_) => {
                return Err(DescribeInstanceInformationErrorModeled::InvalidNextToken(
                    InvalidNextToken {
                        message: Some(INVALID_NEXT_TOKEN.to_string()),
                    },
                )
                .into())
            }
        };

        // Systems Manager is regional: a node is reported through the endpoint it
        // registered with, which is the Region this request was signed for. The
        // inventory yields that Region's nodes in ascending node-id order, which is what
        // makes the resume cursor total.
        let region = request_region(ctx.parts());
        let store = state.lock();
        let mut instance_information_list = Vec::new();
        let mut next_token = None;
        for (id, node) in store.managed_nodes_in_region(&region) {
            if resume_from.is_some_and(|from| id < from) {
                continue;
            }
            // A string filter compares whole values and carries no operator, so every
            // filter of either list is served with `Equal`.
            if !string_filters
                .iter()
                .all(|filter| node.matches_filter(&filter.key, &filter.values, OPERATOR_EQUAL))
            {
                continue;
            }
            if !legacy_filters
                .iter()
                .all(|filter| node.matches_filter(&filter.key, &filter.value_set, OPERATOR_EQUAL))
            {
                continue;
            }
            if instance_information_list.len() == page_size {
                // The page is full and a node is still waiting: hand the caller the id it
                // resumes at, so the next request continues where this one stopped.
                next_token = Some(id.to_string());
                break;
            }
            instance_information_list.push(node.to_instance_information());
        }

        Ok(DescribeInstanceInformationOutput {
            instance_information_list: Some(instance_information_list),
            next_token,
        })
    }
}

/// Refuse every value of one filter that could not be a value of the node attribute its
/// key filters.
///
/// A node id has its own modelled error — "If you specify a node ID that isn't valid …
/// you receive an error" — and every other key's bad value is the filter-value error.
///
/// The node-id refusal REPORTS THE CONSTRAINT the value violated, built from the member
/// the id fills and its modelled pattern
/// ([`crate::state::managed_node_id_constraint_violation`]), so the refusal says which
/// input to fix and what it has to satisfy rather than describing it in prose.
fn check_filter_values(key: &str, values: &[String]) -> Result<(), DescribeInstanceInformationError> {
    for value in values {
        if key == "InstanceIds" {
            if !crate::state::managed_node_id_is_well_formed(value) {
                return Err(DescribeInstanceInformationErrorModeled::InvalidInstanceId(
                    InvalidInstanceId {
                        message: Some(crate::state::managed_node_id_constraint_violation()),
                    },
                )
                .into());
            }
            continue;
        }
        if let Some(reason) = crate::state::node_filter_value_rejection(key, value) {
            return Err(
                DescribeInstanceInformationErrorModeled::InvalidInstanceInformationFilterValue(
                    InvalidInstanceInformationFilterValue {
                        message: Some(reason),
                    },
                )
                .into(),
            );
        }
    }
    Ok(())
}
