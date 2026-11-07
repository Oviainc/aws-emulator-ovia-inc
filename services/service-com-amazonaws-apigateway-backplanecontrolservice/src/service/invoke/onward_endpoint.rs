// SPDX-License-Identifier: CC0-1.0

//! An `HTTP_PROXY` integration whose `uri` names an endpoint THIS EMULATOR serves that is
//! not a stage of this api plane: how the request is passed on to it, and how what it
//! answered becomes the method response.
//!
//! `IntegrationType` gives `HTTP_PROXY` as "for integrating the API method request with an
//! HTTP endpoint, including a third party API, with the client request passed through
//! as-is", so a proxy integration ANSWERS WITH THE ENDPOINT'S OWN RESPONSE: its status, its
//! headers and its body, mapped by nothing. A Lambda function URL — "a dedicated HTTP(S)
//! endpoint that you can use to invoke your function" — is such an endpoint, and so is every
//! other data plane a service of this emulator publishes.
//!
//! The request is passed on over the IN-PROCESS transport `OperationContext::callback` hands
//! a handler, which dispatches it back into this same emulator: no socket is opened, no name
//! is resolved and no address the configuration named is dialled, so nothing here can reach
//! the machine the emulator runs on or the network it sits in. What answers is therefore the
//! service that CLAIMS that endpoint, deciding the call on its own records — a function URL's
//! `AuthType`, the function's own resource policy, and the status its handler returned — which
//! is what makes the passed-through status the backend's rather than one invented here. A
//! chain of such calls is bounded by the transport itself: it stamps each request with the
//! depth of the callback chain it belongs to, and the router refuses one past its own limit.
//!
//! A `uri` naming an endpoint NO service of this emulator claims reaches no backend at all:
//! the transport answers the front door's own refusal
//! ([`::service_common::in_process_refusal`]), which is not a response from the endpoint
//! that was addressed, so the call is answered as the integration this stage cannot perform
//! — the same `API_CONFIGURATION_ERROR` every other unperformable integration earns, rather
//! than that refusal envelope passed off as a backend answer.
//!
//! One thing about the endpoint's response does not travel verbatim: a header the API itself
//! sets on every response it sends is the API's to report, so the backend's own value for it
//! is remapped rather than dropped ([`remapped`]).

use ::aws_smithy_runtime_api::client::http::HttpConnector as _;
use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use ::smithy_server_runtime::OperationContext;

use super::{UnservedIntegration, API_CONFIGURATION_ERROR};

/// The response the endpoint `passed` is addressed at answered, or why the integration
/// could not be performed through it at all.
///
/// The answer is returned WHOLE — status, headers and body as the endpoint wrote them —
/// because a proxy integration maps nothing in either direction.
pub(super) async fn answered(
    ctx: &OperationContext<'_>,
    passed: RtRequest,
) -> Result<RtResponse, UnservedIntegration> {
    let answer = match ctx.callback().http_connector().call(passed).await {
        Ok(answer) => answer,
        Err(error) => {
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the endpoint the HTTP_PROXY integration names did not answer the request \
                 passed on to it"
            );
            return Err(UnservedIntegration {
                refusal: API_CONFIGURATION_ERROR,
                reason: "the endpoint the HTTP_PROXY integration's uri names did not answer \
                         the request passed on to it, so there is no backend response to \
                         pass through"
                    .to_string(),
            });
        }
    };
    // The front door's own refusal is not the addressed endpoint answering: nothing served
    // the request, so there is no response to pass through.
    let refused = answer
        .body()
        .bytes()
        .is_some_and(::service_common::in_process_refusal::is_transport_refusal);
    if refused {
        return Err(UnservedIntegration {
            refusal: API_CONFIGURATION_ERROR,
            reason: "the HTTP_PROXY integration's uri names an endpoint nothing in this \
                     emulator serves, so no backend answer exists to pass through"
                .to_string(),
        });
    }
    Ok(remapped(answer))
}

/// The header API Gateway reports the request IT answered in, on every response of this data
/// plane (`::service_common::query_error_envelope::with_request_id_header`).
const RESERVED_RESPONSE_HEADER: &str = "x-amzn-requestid";

/// The prefix API Gateway remaps a reserved response header with, so a backend's own value
/// for it still reaches the client under a name the api has not taken.
const REMAPPED_PREFIX: &str = "x-amzn-remapped-";

/// `answer` with every header the API itself sets on the response it sends REMAPPED: the
/// backend's value travels under [`REMAPPED_PREFIX`] and that name, leaving the name itself
/// to the api.
///
/// A proxy integration passes a response through, and the api's own response names the
/// request the api answered ([`RESERVED_RESPONSE_HEADER`]): both values cannot travel under
/// one name, and the one the client correlates its call by is the API's. So the backend's is
/// renamed rather than dropped or allowed to displace it — which is how a client reads the
/// backend's own id for a call it made through a proxy integration.
fn remapped(mut answer: RtResponse) -> RtResponse {
    let carried: Vec<String> = answer
        .headers()
        .get_all(RESERVED_RESPONSE_HEADER)
        .map(str::to_string)
        .collect();
    if carried.is_empty() {
        return answer;
    }
    answer.headers_mut().remove(RESERVED_RESPONSE_HEADER);
    for value in carried {
        if let Err(refused) = answer
            .headers_mut()
            .try_append(format!("{REMAPPED_PREFIX}{RESERVED_RESPONSE_HEADER}"), value)
        {
            ::tracing::warn!(
                error = %refused,
                "the backend's reserved response header cannot be remapped and is left off \
                 the response"
            );
        }
    }
    answer
}
