// SPDX-License-Identifier: CC0-1.0

//! The DECISION a `REQUEST` authorizer makes about one opening handshake, and the status the
//! handshake is refused with when it says no.
//!
//! The authorization of the `$connect` route is applied before anything that route fronts
//! ([`super::connect_authorization`]), and it has two halves. The first is the service's
//! own and is computed from stored records: every `IdentitySource` must be "present in the
//! request, not null, and non-empty. Only when this is true does the authorizer invoke the
//! authorizer Lambda function. Otherwise, it returns a 401 Unauthorized response without
//! calling the Lambda function" (`Authorizer$IdentitySource`). The second is the
//! FUNCTION'S: past that precondition the decision IS the answer of the Lambda the
//! authorizer's `AuthorizerUri` names, so it is obtained by invoking it — over the
//! in-process transport the api's socket was opened with — and never derived here.
//!
//! That invocation, the two response formats a decision can arrive in, the policy
//! evaluation and the cache the decision is kept in are the HTTP data plane's own
//! ([`crate::service::invoke::request_authorizer`]), shared verbatim: an authorizer behaves
//! the same whichever protocol's route it fronts. What this module owns is what differs —
//! the EVENT a WebSocket connection request's authorizer is called with
//! ([`super::event::authorizer_event`]), the ARN of the `$connect` route it decides about,
//! and the statuses a handshake is refused with:
//!
//! * a DENY is `403` `{"message":"Forbidden"}` — the authorizer ran and said no, which AWS
//!   answers the opening handshake with before any route Lambda runs;
//! * an authorizer whose decision could not be obtained at all is `500`
//!   `{"message":"Internal server error"}`, a fault of the api's configuration rather than
//!   of the client.
//!
//! What is NOT shared is the CACHE, nor WHICH member of the answer states the decision.
//! Caching an authorizer's decision is a feature of the HTTP data plane, not of this one:
//! "For HTTP APIs, identity sources are also used as the cache key when caching is enabled"
//! (`Authorizer$IdentitySource`), and the operation that clears those entries is "Supported
//! only for HTTP APIs" (`ResetAuthorizersCache`) — so a WebSocket api's connection request is
//! decided by an invocation of its own, every time, and two connections presenting the same
//! identity get the decision their own call earned rather than one shared entry. And the two
//! members that would declare the answer's FORMAT are an HTTP api's too
//! (`Authorizer$EnableSimpleResponses`, `Authorizer$AuthorizerPayloadFormatVersion`), so
//! here the answer itself states which format it is in
//! ([`request_authorizer::Reading::AsAnswered`]).
//!
//! What it ALLOWS travels with the connection: the `context` the function returned and the
//! principal it named are carried into the event of every later call of that connection
//! ([`super::event::Connection`]), which is the only way a route's handler reads what the
//! authorizer decided.

use ::smithy_server_runtime::EmbeddedEmulator;

use super::dispatch::CONNECT_ROUTE_KEY;
use super::event::{self, Connection};
use crate::service::invoke::request_authorizer::{self, About, Decided, Reading, Refused};
use crate::state::Authorizer;

/// The decision `authorizer` makes about the handshake that offered `connection`.
///
/// `Err` is `(status, sentence)`: the refusal the handshake is answered with, in the same
/// shape the steps before it answer theirs ([`super::connect_authorization`]).
pub async fn decided(
    callback: &EmbeddedEmulator,
    connection: &Connection,
    authorizer: &Authorizer,
    identity: &[String],
) -> Result<Decided, (u16, &'static str)> {
    let route_arn = request_authorizer::websocket_route_arn(
        &connection.region,
        &connection.api_id,
        &connection.stage_name,
        CONNECT_ROUTE_KEY,
    );
    let event = event::authorizer_event(connection, CONNECT_ROUTE_KEY, &route_arn);
    let about = About {
        authorizer,
        api_id: &connection.api_id,
        stage_name: &connection.stage_name,
        identity,
        route_arn: &route_arn,
        // A WEBSOCKET api's authorizer carries neither of the two members that would
        // declare a format, so the answer itself states which it is; see
        // [`request_authorizer::Reading`].
        reading: Reading::AsAnswered,
    };
    // No cache: see the module docs for why a connection request is decided by its own
    // invocation.
    match request_authorizer::decided(None, callback, &about, &event).await {
        Ok(decided) => Ok(decided),
        Err(Refused::Denied) => {
            ::tracing::info!(
                api_id = %connection.api_id,
                stage = %connection.stage_name,
                authorizer_id = %authorizer.authorizer_id,
                "refusing a WebSocket opening handshake: the connection request's authorizer \
                 did not authorize it"
            );
            Err((
                crate::service::data_plane::FORBIDDEN_STATUS,
                crate::service::data_plane::FORBIDDEN_MESSAGE,
            ))
        }
        Err(Refused::Unplaceable) => Err((
            crate::service::data_plane::UNPLACEABLE_STATUS,
            super::INTERNAL_MESSAGE,
        )),
    }
}
