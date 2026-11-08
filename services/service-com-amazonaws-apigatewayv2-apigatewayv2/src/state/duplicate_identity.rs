// SPDX-License-Identifier: CC0-1.0

//! What this service answers a write that would claim an identity another resource of
//! the same collection already holds.
//!
//! Every such refusal is API Gateway's own, PER KIND of resource: the status and the
//! error shape it answers with, and the sentence it words it in — fixed text for the
//! kinds whose sentence states the requirement, fixed text plus the identifiers the
//! REQUEST carried for the kinds whose sentence names what collided. A module of its
//! own under [`crate::state`], because a client branches on these refusals (a duplicate
//! create is how "already there" is discovered), so the create and the later update of
//! one kind cannot word, or classify, the same collision two ways.
//!
//! Two identities' refusals are NOT here, each because it lives with the rules it is
//! only meaningful beside: the base path of a custom domain name
//! ([`crate::state::API_MAPPING_KEY_CONFLICT`], with the rest of the `ApiMappingKey`
//! rules) and the name of a stage ([`crate::state::STAGE_ALREADY_EXISTS_MESSAGE`], with
//! the stage-name rules). Each is still stated exactly once, which is the invariant.

/// The sentence a create refuses a custom domain name the account already holds with.
///
/// The whole sentence is fixed text: it names NEITHER the domain name the request
/// carried — the caller already holds it — nor the account. The refusal is the request's
/// own fault rather than a conflict with the resource's current state, so it is answered
/// as the modelled `BadRequestException` ("The request is not valid, for example, the
/// input is incomplete or incorrect"), 400, and a client branching on that to treat a
/// duplicate as a benign already-exists sees it.
pub const DOMAIN_NAME_ALREADY_EXISTS_MESSAGE: &str = "The domain name you provided already exists.";

/// The sentence a create refuses a model name the addressed api already carries with,
/// answered as the modelled `ConflictException` (409): a second model of the name would
/// conflict with the model the api already holds under it.
///
/// The whole sentence is fixed text — it interpolates neither the name nor the api — and
/// it says REST API, which is how API Gateway words this one sentence for either
/// protocol; it is not a spelling to "correct" to the kind of api the request addressed.
pub const MODEL_NAME_ALREADY_EXISTS_MESSAGE: &str = "Model name already exists for this REST API";

/// The sentence a writer of a route key refuses one the api already holds with, answered
/// as the modelled `ConflictException` (409): a second route under the key would conflict
/// with the route the api already serves it from.
///
/// The key comes from the request; the api is NOT named, since the caller addressed it by
/// path. Shared by the create, the update and the quick-create patch, so the one
/// collision cannot be worded three ways.
pub fn route_key_in_use_message(route_key: &str) -> String {
    format!("Route with key {route_key} already exists for this API")
}

/// The sentence a write refuses an integration-response key the addressed integration
/// already holds a response under.
///
/// The key is what an integration response is SELECTED by, so the sentence names the key
/// that collided AND the integration it collided in — both taken from the request (the
/// key from its body, the integration from its path), since those are what the caller
/// can act on. Answered as the modelled `BadRequestException`, 400.
pub fn integration_response_key_in_use_message(
    integration_response_key: &str,
    integration_id: &str,
) -> String {
    format!(
        "IntegrationResponse with key {integration_response_key} is already present in \
         Integration with ID {integration_id}"
    )
}

/// The sentence a write refuses a route-response key the addressed route already holds a
/// response under.
///
/// Two sentences: the collision, naming the key from the request, and the requirement it
/// breaks. The route itself is NOT named — a caller addressed it by path and already
/// holds its id. Answered as the modelled `BadRequestException`, 400.
pub fn route_response_key_in_use_message(route_response_key: &str) -> String {
    format!(
        "RouteResponse with key {route_response_key} already exists on this Route. \
         RouteResponse keys must be unique within a given Route."
    )
}
