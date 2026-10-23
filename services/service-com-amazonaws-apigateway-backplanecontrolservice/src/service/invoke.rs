// SPDX-License-Identifier: CC0-1.0

//! The `execute-api` DATA plane: how a request addressed to a deployed REST API stage's
//! invoke URL is recognised and answered.
//!
//! A REST API with a deployment is reached at
//! `https://{rest-api-id}.execute-api.{region}.amazonaws.com/{stage}/{resource-path}` —
//! the endpoint the `RestApi$disableExecuteApiEndpoint` documentation names ("the default
//! `https://{api_id}.execute-api.{region}.amazonaws.com` endpoint"). That host is NOT the
//! control plane's (`apigateway.{region}.amazonaws.com`): its first label is the API's own
//! id, which names no service, so the emulator's front door can resolve no target service
//! from such a request and answers it `404 UnknownService` — for a URL this very service
//! told the caller to use. The one thing that is never correct is for the host to be
//! unknown to the service.
//!
//! So this module is the second half of [`crate::service::EmulatedService`]: the claim
//! ([`invoke_api_id`], read by `Service::can_handle`) and the serving
//! ([`serve`], reached from the two seams the router offers before and after
//! authentication). It is a READ of the control plane's own records — a request to a
//! stage changes nothing — and every status it answers is computed from them.
//!
//! # What it resolves, in order
//!
//! 1. the host's first label names a stored `RestApi` (the claim), whose
//!    `disableExecuteApiEndpoint` must not have turned this endpoint off;
//! 2. the first path segment names a `Stage` of that API, and that stage's
//!    `deploymentId` names a stored `Deployment`;
//! 3. the rest of the path matches one of the resource paths THAT DEPLOYMENT holds
//!    (`Deployment$apiSummary`), most specific match first: a literal segment beats a
//!    `{parameter}`, which beats a greedy `{proxy+}`;
//! 4. the request's verb names a method of that resource in the deployment, or the
//!    `ANY` catch-all;
//! 5. the method's deployed `authorizationType` and `apiKeyRequired` are enforced;
//! 6. the method's integration decides the response.
//!
//! A step that resolves nothing is answered the way API Gateway answers it: through the
//! API's GATEWAY RESPONSE for the matching response type ([`GatewayRefusal`]), so a
//! developer's `PutGatewayResponse` customization of that type — its status code and its
//! template — is what the caller sees, and the uncustomized case falls back to the status
//! code the response type documents ([`crate::state::default_gateway_status_code`]).
//!
//! # What it does not do, and what it answers instead
//!
//! Only the `MOCK` integration type returns a backend's answer without a backend: it is
//! "for testing without actually invoking the backend" (`IntegrationType`), so its
//! response is entirely the API's own records — the integration response selected for the
//! call, the response template for the negotiated content type, and the headers the
//! method response declares. That path is served in full, from state.
//!
//! Every other integration type names a backend this emulator cannot call:
//!
//! * `AWS` / `AWS_PROXY` invoke a Lambda function or another service's action. Lambda's
//!   own `Invoke` answers `501` for `RequestResponse` and `Event` — this emulator has no
//!   in-process runtime for a deployment package (see
//!   `services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs`) — so no
//!   function's answer exists to map;
//! * `HTTP` / `HTTP_PROXY` address an endpoint over the network, and this emulator makes
//!   no outbound connection.
//!
//! None of those is faked: a body, a status or a header invented for a backend that never
//! ran would be indistinguishable from a real one. They are answered as API Gateway
//! answers an integration it could not perform — the `API_CONFIGURATION_ERROR` gateway
//! response, `500` with `{"message":"Internal server error"}` — and the reason is logged.
//! The same goes for a mapping template that is a Velocity TEMPLATE rather than a literal
//! body: evaluating Velocity is not something this emulator does, and a hand-written
//! subset of it would silently produce the wrong body, so such a template is an
//! integration this call cannot perform rather than a body built by guesswork.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::http;

use crate::ops::put_integration::MOCK_INTEGRATION_TYPE;
use crate::ops::put_integration_response::is_static_mapping_value;
use crate::ops::put_method::method_key;
use crate::ops::put_method_response::METHOD_RESPONSE_HEADER_PREFIX;
use crate::ops::types::{GetIntegrationOutput, GetIntegrationResponseOutput};
use crate::state::{ApiKeyEntry, DeploymentEntry, MethodEntry, RestApiEntry};

/// The second label of every `execute-api` endpoint host: `{api-id}.execute-api.…`.
///
/// From `RestApi$disableExecuteApiEndpoint`: "clients can invoke your API with the default
/// `https://{api_id}.execute-api.{region}.amazonaws.com` endpoint".
pub const EXECUTE_API_LABEL: &str = "execute-api";

/// The path segment that belongs to ApiGatewayManagementApi rather than to a stage.
///
/// `@connections` is bound on the SAME `execute-api` host — that service's documentation
/// gives its endpoint as `https://{api-id}.execute-api.{region}.amazonaws.com/{stage}` and
/// binds `DELETE|GET|POST /@connections/{ConnectionId}` under it — so a request naming it
/// is not a stage invoke and is left to the service that models it. Claiming it here would
/// make two services claim one request, which the router rejects as `AmbiguousService`.
const CONNECTIONS_SEGMENT: &str = "@connections";

/// The verb-independent catch-all a method may be put under, from
/// [`crate::ops::put_method::HTTP_METHODS`]: "plus the `ANY` catch-all that matches every
/// verb".
const ANY_METHOD: &str = "ANY";

/// The `authorizationType` that lets a request through with no authorization at all. From
/// `Method$authorizationType`: "the method's authorization type. Valid values are `NONE`
/// for open access …".
const AUTHORIZATION_NONE: &str = "NONE";

/// The `authorizationType` whose caller must sign the request: "`AWS_IAM` for using AWS
/// IAM permissions" (`Method$authorizationType`).
const AUTHORIZATION_AWS_IAM: &str = "AWS_IAM";

/// The `authorizationType`s served by an `Authorizer` of the API: "`CUSTOM` for using a
/// custom authorizer, or `COGNITO_USER_POOLS` for using a Cognito user pool"
/// (`Method$authorizationType`).
const AUTHORIZATION_BY_AUTHORIZER: &[&str] = &["CUSTOM", "COGNITO_USER_POOLS"];

/// The header an API key travels in when the API's `apiKeySource` is `HEADER`, which is
/// what that member documents: "`HEADER` to read the API key from the `X-API-Key` header of
/// a request".
const API_KEY_HEADER: &str = "x-api-key";

/// The `apiKeySource` that reads the key from the request header; see [`API_KEY_HEADER`].
const API_KEY_SOURCE_HEADER: &str = "HEADER";

/// The identity-source prefix of a request header, from `Authorizer$identitySource`: "For
/// a `TOKEN` or `COGNITO_USER_POOLS` authorizer, this is required and specifies the request
/// header mapping expression for the custom header holding the authorization token …
/// `method.request.header.Auth`".
const IDENTITY_SOURCE_HEADER_PREFIX: &str = "method.request.header.";

/// The identity-source prefix of a query-string parameter, the other request-bound form
/// `Authorizer$identitySource` names for a `REQUEST` authorizer ("for example,
/// `$request.querystring.auth`" in the comma-separated list it documents).
const IDENTITY_SOURCE_QUERY_PREFIX: &str = "method.request.querystring.";

/// The media type a gateway response and an untemplated integration response are rendered
/// with. API Gateway's own default gateway-response templates are
/// `application/json`-keyed, and a `MOCK` integration with no response template answers an
/// empty JSON body.
const JSON_CONTENT_TYPE: &str = "application/json";

/// The member a gateway response body carries the reason under: API Gateway's default
/// gateway-response template is `{"message":$context.error.messageString}`.
const MESSAGE_MEMBER: &str = "message";

/// The status code a refusal falls back to when its response type names none and no
/// customization overrides it. `DEFAULT_5XX`'s own class: this emulator could not place
/// the refusal, which is a server-side fault, never a caller's.
const UNPLACEABLE_REFUSAL_STATUS: u16 = 500;

/// The gateway response type whose customization applies to any 4xx the API has no
/// specific customization for; from [`crate::state`]'s own note on the two rows that carry
/// no status code of their own, "each is a fallback for a whole class of responses".
const DEFAULT_4XX: &str = "DEFAULT_4XX";

/// The gateway response type whose customization applies to any 5xx; see [`DEFAULT_4XX`].
const DEFAULT_5XX: &str = "DEFAULT_5XX";

/// One refusal of the data plane: the GATEWAY RESPONSE TYPE it is answered through — which
/// decides its status code, and which customization of the API applies to it — and the
/// sentence API Gateway's own default body carries for it.
///
/// The type is the unit of customization (`PutGatewayResponse` takes a `responseType`), so
/// naming it here is what lets a developer's customization reach the caller without this
/// module knowing anything about it.
#[derive(Debug, Clone, Copy)]
pub struct GatewayRefusal {
    /// A value of the model's `GatewayResponseType` enum.
    response_type: &'static str,
    /// The sentence the uncustomized response body carries.
    message: &'static str,
}

/// The API, the stage or the endpoint does not serve this request at all: API Gateway
/// answers `403` with `{"message":"Forbidden"}` for an API id it does not serve, a stage
/// that has no deployment, and an endpoint a developer has turned off.
const FORBIDDEN: GatewayRefusal = GatewayRefusal {
    response_type: "ACCESS_DENIED",
    message: "Forbidden",
};

/// Nothing in the DEPLOYMENT matches the path and verb, or the method's authorization
/// needs a credential the request did not carry: both are
/// `{"message":"Missing Authentication Token"}` with `403`, which is what API Gateway
/// answers for a route it cannot place and for an `AWS_IAM` method reached unsigned.
const MISSING_AUTHENTICATION_TOKEN: GatewayRefusal = GatewayRefusal {
    response_type: "MISSING_AUTHENTICATION_TOKEN",
    message: "Missing Authentication Token",
};

/// The method requires an API key and the request presented none this API's records
/// associate with this stage: `403` `{"message":"Forbidden"}` through `INVALID_API_KEY`.
const INVALID_API_KEY: GatewayRefusal = GatewayRefusal {
    response_type: "INVALID_API_KEY",
    message: "Forbidden",
};

/// An authorizer's identity source is absent from the request, so the authorizer is not
/// even called: `401` `{"message":"Unauthorized"}`.
const UNAUTHORIZED: GatewayRefusal = GatewayRefusal {
    response_type: "UNAUTHORIZED",
    message: "Unauthorized",
};

/// The method's configuration cannot serve the call: no integration on the deployed
/// method, an integration whose type names a backend this emulator cannot call, or a
/// mapping template it cannot evaluate. `500` `{"message":"Internal server error"}`.
const API_CONFIGURATION_ERROR: GatewayRefusal = GatewayRefusal {
    response_type: "API_CONFIGURATION_ERROR",
    message: "Internal server error",
};

/// The method is authorized by an `Authorizer` whose decision this emulator cannot
/// obtain — the authorizer runs a Lambda function, and no deployment package runs here.
/// `500` `{"message":"Internal server error"}`, the same class API Gateway reports when it
/// cannot invoke the authorizer.
const AUTHORIZER_CONFIGURATION_ERROR: GatewayRefusal = GatewayRefusal {
    response_type: "AUTHORIZER_CONFIGURATION_ERROR",
    message: "Internal server error",
};

/// The API id a request addresses on an `execute-api` host, or `None` when the request is
/// not one this data plane serves.
///
/// `None` for a host that is not an `execute-api` endpoint (the control plane's
/// `apigateway.{region}.amazonaws.com` included, whose first label names the service), and
/// for the `@connections` path space, which belongs to ApiGatewayManagementApi — see
/// [`CONNECTIONS_SEGMENT`].
pub fn invoke_api_id(parts: &http::request::Parts) -> Option<&str> {
    let host = request_host(parts)?;
    let api_id = api_id_of_host(host)?;
    (!addresses_connections(parts.uri.path())).then_some(api_id)
}

/// [`invoke_api_id`] for the runtime-API request the router hands an operation and the
/// routing-error seam, which carries its URI as a string.
pub fn invoke_api_id_of_request(req: &RtRequest) -> Option<&str> {
    let host = req.headers().get("host").or_else(|| authority_of(req.uri()))?;
    let api_id = api_id_of_host(host)?;
    (!addresses_connections(path_of(req.uri()))).then_some(api_id)
}

/// The first label of `host` when `host` is an `execute-api` endpoint: the API id.
///
/// The shape is `{api-id}.execute-api.{region}.amazonaws.com` — a first label, then the
/// [`EXECUTE_API_LABEL`] label, then at least a region and a domain. Any `:port` and
/// userinfo is removed first, as the front door's own host reader does. The region and the
/// domain are not matched against a list: an emulator serves whatever region a caller
/// addresses, and a FIPS or a dualstack spelling of the same endpoint is the same endpoint.
fn api_id_of_host(host: &str) -> Option<&str> {
    let without_userinfo = host.rsplit('@').next().unwrap_or(host);
    let without_port = without_userinfo
        .split(':')
        .next()
        .unwrap_or(without_userinfo);
    let mut labels = without_port.split('.');
    let api_id = labels.next()?;
    let second = labels.next()?;
    // The label is `execute-api` itself, or one of its endpoint VARIANTS, which AWS spells
    // with a suffix after a hyphen (`execute-api-fips`). A label that merely STARTS with
    // those characters (`execute-apifoo`) is a different host and is not claimed.
    let execute_api = second == EXECUTE_API_LABEL
        || second
            .strip_prefix(EXECUTE_API_LABEL)
            .is_some_and(|variant| variant.starts_with('-') && variant.len() > 1);
    if api_id.is_empty() || !execute_api {
        return None;
    }
    // A region label and a domain: `{api-id}.execute-api.{region}.amazonaws.com` has four
    // labels after the id, and the shortest form this accepts still has two, so a bare
    // `execute-api.something` (no api id in front) is not read as one.
    (labels.count() >= 2).then_some(api_id)
}

/// Whether `path` addresses the `@connections` space of ApiGatewayManagementApi, in either
/// of the two forms that service's callers use: with the stage prefix the SDK's endpoint
/// carries (`/{stage}/@connections/{id}`) and without it (`/@connections/{id}`).
fn addresses_connections(path: &str) -> bool {
    path_segments(path)
        .into_iter()
        .take(2)
        .any(|segment| segment == CONNECTIONS_SEGMENT)
}

/// Serve one request addressed to a deployed stage's invoke URL, from the control plane's
/// own records.
///
/// Free-standing and reached with the ONE shared engine, so the two seams that serve this
/// data plane — `Service::override_routing_error` for a request carrying no credential and
/// `CredentialedDataPlane` for one carrying a SigV4 credential — answer identically; they
/// differ only in whether the emulator verified a signature on the way in.
pub fn serve<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> RtResponse {
    let Some(api_id) = invoke_api_id_of_request(req) else {
        // Unreachable through either seam: both name this only for a request whose host is
        // an `execute-api` endpoint. Answered as an endpoint that serves nothing.
        return refusal_response(None, FORBIDDEN);
    };
    let Some(state) = crate::state::service_state(engine) else {
        return refusal_response(None, API_CONFIGURATION_ERROR);
    };
    let store = state.lock();
    let Some(api) = store.rest_apis.get(api_id) else {
        // The API was deleted between the claim and now, or another process minted the
        // host: an id this account does not serve is `403 Forbidden`.
        return refusal_response(None, FORBIDDEN);
    };
    if api.disable_execute_api_endpoint {
        // `disableExecuteApiEndpoint`: "Specifies whether clients can invoke your API by
        // using the default execute-api endpoint." Turned off, the endpoint serves nothing.
        return refusal_response(Some(api), FORBIDDEN);
    }

    let path = path_of(req.uri());
    let Some((stage_name, resource_path)) = split_stage(path) else {
        return refusal_response(Some(api), FORBIDDEN);
    };
    let Some(stage) = api.stages.get(stage_name) else {
        return refusal_response(Some(api), FORBIDDEN);
    };
    let Some(deployment) = api.deployments.get(&stage.deployment_id) else {
        return refusal_response(Some(api), FORBIDDEN);
    };

    // The DEPLOYMENT decides what is served: a method put on the API after the stage's
    // deployment was created is not in it, and one deleted afterwards still is.
    let Some(matched_path) = match_resource_path(deployment, &resource_path) else {
        return refusal_response(Some(api), MISSING_AUTHENTICATION_TOKEN);
    };
    let Some((verb, snapshot)) = match_method(deployment, matched_path, req.method()) else {
        return refusal_response(Some(api), MISSING_AUTHENTICATION_TOKEN);
    };

    let authorization_type = snapshot
        .authorization_type
        .as_deref()
        .unwrap_or(AUTHORIZATION_NONE);
    // The live method carries the integration and the authorizer the deployed method
    // names; `Deployment$apiSummary`'s `MethodSnapshot` models only `authorizationType`
    // and `apiKeyRequired`, so the rest of the method is read from the API itself.
    let live_method = api
        .resources
        .values()
        .find(|resource| resource.path == matched_path)
        .and_then(|resource| resource.methods.get(verb));

    if let Some(refusal) = authorization_refusal(api, live_method, authorization_type, req) {
        return refusal_response(Some(api), refusal);
    }
    if snapshot.api_key_required
        && !api_key_admits(&store.api_keys, api, &stage.stage_name, req)
    {
        return refusal_response(Some(api), INVALID_API_KEY);
    }

    let Some(integration) = live_method.and_then(|method| method.integration.as_ref()) else {
        tracing::warn!(
            %api_id,
            stage = %stage.stage_name,
            path = %matched_path,
            verb = %verb,
            "the deployed method carries no integration, so the stage has nothing to call"
        );
        return refusal_response(Some(api), API_CONFIGURATION_ERROR);
    };
    serve_integration(api, live_method, integration, req)
}

/// The refusal the method's `authorizationType` earns this request, or `None` when the
/// request may proceed.
///
/// `NONE` admits every request. `AWS_IAM` needs a SigV4 credential, whose SIGNATURE the
/// emulator itself verifies (a service never checks signatures), so only its presence is
/// read here. An `Authorizer`-backed type is a decision this emulator cannot obtain — the
/// authorizer runs code — except for the one part of it that is settled before the
/// authorizer is called: an identity source the request does not carry is `401` without
/// the authorizer running at all (`Authorizer$identitySource`).
///
/// What `AWS_IAM` here does NOT do, stated so a passing test is not read as more than it
/// is: no authorization DECISION is made. AWS both authenticates the signature and
/// evaluates `execute-api:Invoke` for the resolved principal against the api's resource
/// policy and the caller's identity policies, refusing an authenticated but unprivileged
/// caller. This emulator models no caller identity and no identity-policy evaluation, so
/// the decision an under-privileged principal would get from AWS is not one it can
/// produce; a verified credential admits the call, and a test asserting the `403` an
/// unprivileged principal earns is not one this emulator can satisfy.
fn authorization_refusal(
    api: &RestApiEntry,
    method: Option<&MethodEntry>,
    authorization_type: &str,
    req: &RtRequest,
) -> Option<GatewayRefusal> {
    if authorization_type == AUTHORIZATION_NONE {
        return None;
    }
    if authorization_type == AUTHORIZATION_AWS_IAM {
        return (!carries_sigv4_credential(req)).then_some(MISSING_AUTHENTICATION_TOKEN);
    }
    if !AUTHORIZATION_BY_AUTHORIZER.contains(&authorization_type) {
        // A value outside the documented set is a method this emulator cannot authorize.
        return Some(API_CONFIGURATION_ERROR);
    }
    // A method whose `authorizerId` names no authorizer of this API cannot authorize the
    // call at all, which is a configuration the stage cannot serve.
    let Some(authorizer) = method
        .and_then(|method| method.authorizer_id.as_deref())
        .and_then(|id| api.authorizers.get(id))
    else {
        return Some(API_CONFIGURATION_ERROR);
    };
    for source in authorizer
        .identity_source
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|source| !source.is_empty())
    {
        // Each identity source the authorizer declares must be PRESENT, or the authorizer
        // is not called at all. The two request-bound forms are read here; a source bound
        // to something else (a stage variable, say) is not one this emulator can resolve,
        // and is left to the refusal below rather than treated as satisfied.
        if let Some(header) = source.strip_prefix(IDENTITY_SOURCE_HEADER_PREFIX) {
            if req
                .headers()
                .get(header.to_ascii_lowercase())
                .is_none_or(str::is_empty)
            {
                return Some(UNAUTHORIZED);
            }
            continue;
        }
        if let Some(parameter) = source.strip_prefix(IDENTITY_SOURCE_QUERY_PREFIX) {
            let present = query_of(req.uri()).split('&').any(|pair| {
                pair.split_once('=')
                    .is_some_and(|(key, value)| key == parameter && !value.is_empty())
            });
            if !present {
                return Some(UNAUTHORIZED);
            }
        }
    }
    tracing::warn!(
        authorizer = %authorizer.name,
        kind = %authorizer.r#type,
        "the authorizer's decision cannot be obtained: this emulator runs no authorizer code"
    );
    Some(AUTHORIZER_CONFIGURATION_ERROR)
}

/// Whether the request presents an API key this API's records associate with `stage`.
///
/// The key travels in [`API_KEY_HEADER`] when the API's `apiKeySource` is `HEADER`; the
/// other documented source reads it from a custom authorizer's returned context, which is
/// a decision this emulator cannot obtain, so no key is presented for it. A key admits the
/// call when it is enabled and its `stageKeys` name this API's stage — the association
/// `UpdateApiKey`'s `/stages` patch writes.
fn api_key_admits(
    keys: &std::collections::BTreeMap<String, ApiKeyEntry>,
    api: &RestApiEntry,
    stage: &str,
    req: &RtRequest,
) -> bool {
    let source = api
        .api_key_source
        .as_deref()
        .unwrap_or(crate::state::DEFAULT_API_KEY_SOURCE);
    if source != API_KEY_SOURCE_HEADER {
        return false;
    }
    let Some(presented) = req.headers().get(API_KEY_HEADER).filter(|key| !key.is_empty()) else {
        return false;
    };
    let stage_key = format!("{}/{stage}", api.id);
    keys.values()
        .any(|key| key.value == presented && key.enabled && key.stage_keys.contains(&stage_key))
}

/// Serve the call through the method's integration.
///
/// `MOCK` is the one type that needs no backend, and it is served in full from the API's
/// records. Every other type names a backend this emulator cannot call, which is an
/// integration this call cannot perform — see the module docs.
fn serve_integration(
    api: &RestApiEntry,
    method: Option<&MethodEntry>,
    integration: &GetIntegrationOutput,
    req: &RtRequest,
) -> RtResponse {
    let integration_type = integration.r#type.as_deref().unwrap_or_default();
    if integration_type != MOCK_INTEGRATION_TYPE {
        tracing::warn!(
            integration_type,
            uri = integration.uri.as_deref().unwrap_or_default(),
            "the integration names a backend this emulator cannot call: no deployment \
             package runs in process and no outbound connection is made, so no backend \
             answer exists to map"
        );
        return refusal_response(Some(api), API_CONFIGURATION_ERROR);
    }

    // A `MOCK` invokes nothing, so there is no backend answer for a `selectionPattern` to
    // match: the DEFAULT integration response — the one with no pattern — is the response
    // of the call.
    let Some(selected) = default_integration_response(integration) else {
        tracing::warn!(
            "the MOCK integration has no default integration response, so the call maps to \
             no method response"
        );
        return refusal_response(Some(api), API_CONFIGURATION_ERROR);
    };
    let Some(status) = selected
        .status_code
        .as_deref()
        .and_then(|code| code.parse::<u16>().ok())
    else {
        return refusal_response(Some(api), API_CONFIGURATION_ERROR);
    };

    let (content_type, body) = match response_body(selected, req) {
        Ok(rendered) => rendered,
        Err(template) => {
            tracing::warn!(
                content_type = %template.content_type,
                bytes = template.bytes,
                "the response template is a Velocity template, which this emulator does not \
                 evaluate: no body is invented for it"
            );
            return refusal_response(Some(api), API_CONFIGURATION_ERROR);
        }
    };

    let mut response = response_with(status, &content_type, body);
    // API Gateway returns only the headers the METHOD response declares, mapped as the
    // INTEGRATION response says. A `MOCK` has no backend response to map from, so the one
    // mapping it can resolve is the documented static value in single quotes.
    let declared = method
        .and_then(|method| method.method_responses.get(&status.to_string()))
        .and_then(|response| response.response_parameters.as_ref());
    for (key, value) in selected.response_parameters.iter().flatten() {
        let Some(name) = key.strip_prefix(METHOD_RESPONSE_HEADER_PREFIX) else {
            continue;
        };
        if declared.is_none_or(|declared| !declared.contains_key(key)) {
            continue;
        }
        // A `MOCK` has no backend response to map from, so the one mapping it can resolve
        // is the documented static value in single quotes
        // ([`is_static_mapping_value`]): the literal between the quotes is the header's
        // value. Read by stripping the quotes rather than by an index, so neither the read
        // nor the length arithmetic can fail whatever was stored.
        let literal = is_static_mapping_value(value)
            .then(|| value.strip_prefix('\'').and_then(|rest| rest.strip_suffix('\'')))
            .flatten();
        if let Some(literal) = literal {
            // `try_insert`, not the panicking `insert`: the name and the value come from
            // records a control-plane request wrote, and a stored mapping the wire cannot
            // carry (a value with a newline in it, say) must not fail the REQUEST — it is
            // skipped, and said so, so the rest of the response is still served.
            if let Err(refused) = response
                .headers_mut()
                .try_insert(name.to_ascii_lowercase(), literal.to_string())
            {
                tracing::warn!(
                    header = %name,
                    error = %refused,
                    "the mapped response header cannot be sent on the wire and is skipped"
                );
            }
        }
    }
    response
}

/// The integration response a `MOCK` call maps to: the one with no `selectionPattern`.
///
/// `selectionPattern` is "the regular expression (regex) pattern used to choose an
/// integration response based on the response from the back end" — there is no back-end
/// response here, so a patterned response is never chosen and the default one is.
fn default_integration_response(
    integration: &GetIntegrationOutput,
) -> Option<&GetIntegrationResponseOutput> {
    let responses = integration.integration_responses.as_ref()?;
    let mut defaults: Vec<&GetIntegrationResponseOutput> = responses
        .values()
        .filter(|response| {
            response
                .selection_pattern
                .as_deref()
                .is_none_or(str::is_empty)
        })
        .collect();
    // Keyed by status code in the record, so the choice is stable across calls whatever
    // order the map yields.
    defaults.sort_by(|left, right| left.status_code.cmp(&right.status_code));
    defaults.first().copied()
}

/// A response template this emulator does not render, described for the log: the content
/// type it is keyed by and its size. The template itself is a developer's configuration and
/// is not copied into the log.
#[derive(Debug)]
struct UnrenderableTemplate {
    content_type: String,
    bytes: usize,
}

/// The content type and body the selected integration response renders.
///
/// The template is chosen by the content type the caller asked for (`Accept`), then the
/// JSON one, then the single template an integration response that declares exactly one
/// carries. `Ok` with an EMPTY body when no template applies — a `MOCK` with no template
/// answers no body — and `Err` when rendering the chosen template would mean evaluating
/// Velocity, which this emulator does not do.
fn response_body(
    selected: &GetIntegrationResponseOutput,
    req: &RtRequest,
) -> Result<(String, Vec<u8>), UnrenderableTemplate> {
    let Some(templates) = selected
        .response_templates
        .as_ref()
        .filter(|templates| !templates.is_empty())
    else {
        return Ok((JSON_CONTENT_TYPE.to_string(), Vec::new()));
    };
    let accepted = req
        .headers()
        .get("accept")
        .and_then(|accept| accept.split(',').next())
        .map(|media| media.split(';').next().unwrap_or(media).trim().to_string());
    let chosen = accepted
        .as_deref()
        .and_then(|media| templates.get_key_value(media))
        .or_else(|| templates.get_key_value(JSON_CONTENT_TYPE))
        .or_else(|| {
            let mut entries: Vec<(&String, &String)> = templates.iter().collect();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            (entries.len() == 1).then(|| entries[0])
        });
    let Some((content_type, template)) = chosen else {
        return Ok((JSON_CONTENT_TYPE.to_string(), Vec::new()));
    };
    if is_velocity_template(template) {
        return Err(UnrenderableTemplate {
            content_type: content_type.clone(),
            bytes: template.len(),
        });
    }
    Ok((content_type.clone(), template.clone().into_bytes()))
}

/// The Velocity directives a mapping template may carry, from the Velocity Template
/// Language reference (the `#`-introduced statements of the language a mapping template is
/// written in: "a map of Velocity templates", `requestTemplates`). `#` followed by anything
/// else — `#fff` in a colour, say — is literal text.
const VELOCITY_DIRECTIVES: &[&str] = &[
    "set", "if", "elseif", "else", "end", "foreach", "break", "stop", "macro", "parse",
    "include", "evaluate", "define",
];

/// Whether `template` carries Velocity Template Language rather than being a literal body.
///
/// The language has exactly two constructs: a REFERENCE, `$` (optionally `$!`) followed by
/// an identifier or `{`, and a DIRECTIVE, `#` followed by one of [`VELOCITY_DIRECTIVES`]
/// (optionally braced) or by a comment marker (`##`, `#*`). A template carrying neither
/// needs no evaluation at all: it IS the body, and passing it through is not interpreting a
/// language. One carrying either does need evaluation, and this emulator does not evaluate
/// Velocity, so it is reported as a template that cannot be rendered rather than passed
/// through as if it were literal.
///
/// TOTAL over every `&str`, including a template carrying non-ASCII text: a template is
/// caller-stored data that nothing restricts to ASCII, and this runs while serving a
/// request, so it must not be able to fail. The two bytes it keys on are ASCII, so the
/// position after one is always a character boundary; the remainder is still taken with
/// `str::get`, which answers `None` rather than panicking for any position that is not.
fn is_velocity_template(template: &str) -> bool {
    for (index, byte) in template.as_bytes().iter().enumerate() {
        if *byte != b'$' && *byte != b'#' {
            continue;
        }
        let Some(rest) = template.get(index + 1..) else {
            continue;
        };
        if *byte == b'$' {
            let reference = rest.strip_prefix('!').unwrap_or(rest);
            let identifier = reference.strip_prefix('{').unwrap_or(reference);
            if identifier
                .chars()
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
            {
                return true;
            }
            continue;
        }
        if rest.starts_with('#') || rest.starts_with('*') {
            return true;
        }
        let directive = rest.strip_prefix('{').unwrap_or(rest);
        if VELOCITY_DIRECTIVES.iter().any(|name| {
            // The NAME, not a word that starts with it: `#sets` and `#ifconfig` are literal
            // text, so the name must be followed by something that cannot continue an
            // identifier (`(`, `}`, a space, the end of the template).
            directive.strip_prefix(name).is_some_and(|after| {
                after
                    .chars()
                    .next()
                    .is_none_or(|next| !next.is_ascii_alphanumeric() && next != '_')
            })
        }) {
            return true;
        }
    }
    false
}

/// The resource path of the DEPLOYMENT that matches `request_path`, most specific first.
///
/// A literal segment is more specific than a `{parameter}`, which is more specific than a
/// greedy `{proxy+}` — the order API Gateway resolves a request in. Returns `None` when the
/// deployment holds no path that matches.
fn match_resource_path<'d>(
    deployment: &'d DeploymentEntry,
    request_path: &str,
) -> Option<&'d str> {
    deployment
        .api_summary
        .iter()
        .filter(|(_, methods)| !methods.is_empty())
        .filter_map(|(path, _)| match_score(path, request_path).map(|score| (score, path)))
        .max_by(|(left, left_path), (right, right_path)| {
            left.cmp(right).then_with(|| right_path.cmp(left_path))
        })
        .map(|(_, path)| path.as_str())
}

/// How specifically the resource path `template` matches `request_path`, or `None` when it
/// does not match at all.
///
/// One score per template segment, so the comparison is lexicographic: a longer match of
/// equally specific segments wins, and a literal (3) beats a path parameter (2), which
/// beats a greedy parameter (1).
fn match_score(template: &str, request_path: &str) -> Option<Vec<u8>> {
    let template_segments = path_segments(template);
    let request_segments = path_segments(request_path);
    let mut score = Vec::with_capacity(template_segments.len());
    let mut consumed = 0usize;
    for segment in template_segments {
        let parameter = segment
            .strip_prefix('{')
            .and_then(|name| name.strip_suffix('}'));
        match parameter {
            // A greedy parameter consumes every remaining segment, and needs at least one.
            Some(name) if name.ends_with('+') => {
                if consumed >= request_segments.len() {
                    return None;
                }
                score.push(1);
                return Some(score);
            }
            Some(_) => {
                if consumed >= request_segments.len() {
                    return None;
                }
                score.push(2);
                consumed += 1;
            }
            None => {
                if request_segments.get(consumed) != Some(&segment) {
                    return None;
                }
                score.push(3);
                consumed += 1;
            }
        }
    }
    (consumed == request_segments.len()).then_some(score)
}

/// The deployed method of `path` that serves `verb`: the verb's own method, else the `ANY`
/// catch-all.
fn match_method<'d>(
    deployment: &'d DeploymentEntry,
    path: &str,
    verb: &str,
) -> Option<(&'d str, &'d crate::ops::types::MethodSnapshot)> {
    let methods = deployment.api_summary.get(path)?;
    let requested = method_key(verb);
    methods
        .get_key_value(&requested)
        .or_else(|| methods.get_key_value(ANY_METHOD))
        .map(|(verb, snapshot)| (verb.as_str(), snapshot))
}

/// The stage name and the resource path of an invoke path: the first segment is the stage,
/// and what follows is the path within the API (`/` when nothing follows).
///
/// `None` when the path names no stage at all, which is an endpoint that serves nothing.
fn split_stage(path: &str) -> Option<(&str, String)> {
    let segments = path_segments(path);
    let (stage, rest) = segments.split_first()?;
    Some((stage, format!("/{}", rest.join("/"))))
}

/// The non-empty `/`-separated segments of a path.
fn path_segments(path: &str) -> Vec<&str> {
    path.split('/').filter(|segment| !segment.is_empty()).collect()
}

/// Whether the request presents a SigV4 credential at all — in the `Authorization` header a
/// signed SDK request carries, or in the `X-Amz-Credential` query parameter of a presigned
/// URL.
///
/// Only PRESENCE is read. Whether the signature over that credential is correct is the
/// emulator's own check, never a service's — which is why this is the predicate BOTH the
/// routing seam and this module use: `Service::override_operation` names the data plane as
/// an operation exactly when this is true, so the router verifies the credential before
/// anything is served, and an `AWS_IAM` method is admitted below exactly when the same
/// predicate holds. Two different predicates would let a presigned invoke of an `AWS_IAM`
/// method be served from the pre-authentication seam.
pub fn carries_sigv4_credential(req: &RtRequest) -> bool {
    if req
        .headers()
        .get("authorization")
        .is_some_and(|value| value.contains("Credential="))
    {
        return true;
    }
    query_of(req.uri())
        .split('&')
        .any(|pair| pair.starts_with("X-Amz-Credential="))
}

/// One gateway-response refusal, rendered as the API serves it.
///
/// The status code is the API's customization of this response type when it has one, then
/// its customization of the whole class (`DEFAULT_4XX` / `DEFAULT_5XX`), then the status
/// code the response type itself documents. The body is the customized
/// `application/json` template when that template is a literal, and otherwise the default
/// `{"message": …}` body — a template carrying Velocity is not evaluated here, for the
/// reason the module docs give.
fn refusal_response(api: Option<&RestApiEntry>, refusal: GatewayRefusal) -> RtResponse {
    let documented = crate::state::default_gateway_status_code(refusal.response_type)
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(UNPLACEABLE_REFUSAL_STATUS);
    let class_default = if documented < 500 { DEFAULT_4XX } else { DEFAULT_5XX };
    let customized = api.and_then(|api| {
        api.gateway_responses
            .get(refusal.response_type)
            .or_else(|| api.gateway_responses.get(class_default))
    });
    let status = customized
        .and_then(|response| response.status_code.as_deref())
        .and_then(|code| code.parse::<u16>().ok())
        .unwrap_or(documented);
    let templated = customized
        .and_then(|response| response.response_templates.as_ref())
        .and_then(|templates| templates.get(JSON_CONTENT_TYPE))
        .filter(|template| !is_velocity_template(template));
    let body = match templated {
        Some(template) => template.clone().into_bytes(),
        None => ::serde_json::json!({ MESSAGE_MEMBER: refusal.message })
            .to_string()
            .into_bytes(),
    };
    response_with(status, JSON_CONTENT_TYPE, body)
}

/// One data-plane response: a status, a content type and a body.
///
/// The data plane is plain HTTPS rather than a modelled operation, so it answers in the
/// envelope API Gateway answers in — a JSON body — instead of this service's restJson1
/// error envelope.
///
/// The content type is a key of a stored `responseTemplates` map, which no operation
/// restricts to a media type, so it is inserted with `try_insert` rather than the panicking
/// `insert`: a value the wire cannot carry is left off the response instead of failing the
/// request.
fn response_with(status: u16, content_type: &str, body: Vec<u8>) -> RtResponse {
    let length = body.len();
    let mut response = RtResponse::new(
        StatusCode::try_from(status).unwrap_or(
            StatusCode::try_from(UNPLACEABLE_REFUSAL_STATUS).expect("500 is a valid status"),
        ),
        SdkBody::from(body),
    );
    if let Err(refused) = response
        .headers_mut()
        .try_insert("content-type", content_type.to_string())
    {
        tracing::warn!(
            error = %refused,
            "the integration response's content type cannot be sent on the wire and is \
             left off the response"
        );
    }
    let _ = response
        .headers_mut()
        .try_insert("content-length", length.to_string());
    response
}

/// The `Host` header of a buffered request, or the URI authority when the request is
/// HTTP/2 (which carries `:authority` on the URI instead of a `Host` header).
fn request_host(parts: &http::request::Parts) -> Option<&str> {
    match parts.headers.get("host").and_then(|host| host.to_str().ok()) {
        Some(host) => Some(host),
        None => parts.uri.authority().map(http::uri::Authority::as_str),
    }
}

/// The authority of an absolute request URI, or `None` when the URI is origin-form.
fn authority_of(uri: &str) -> Option<&str> {
    let (_scheme, rest) = uri.split_once("://")?;
    Some(match rest.find('/') {
        Some(path_start) => &rest[..path_start],
        None => rest,
    })
}

/// The path component of a request URI, with any scheme, authority and query removed.
fn path_of(uri: &str) -> &str {
    // The query is dropped FIRST: a query VALUE may carry `://`, which a scheme scan over
    // the whole URI would mistake for the authority.
    let without_query = uri.split('?').next().unwrap_or(uri);
    match without_query.split_once("://") {
        Some((_scheme, authority_and_path)) => match authority_and_path.find('/') {
            Some(path_start) => &authority_and_path[path_start..],
            None => "/",
        },
        None => without_query,
    }
}

/// The query component of a request URI: everything after the first `?`.
fn query_of(uri: &str) -> &str {
    match uri.find('?') {
        Some(mark) => &uri[mark + 1..],
        None => "",
    }
}

#[cfg(test)]
mod tests {
    //! The two pure rules this module owns that a wire test cannot vary cheaply: which
    //! host is an `execute-api` endpoint, and how specifically a deployed resource path
    //! matches a request path. The served behaviour is pinned end to end in
    //! `tests/all/execute_api_data_plane.rs`.

    use super::*;

    #[test]
    fn an_execute_api_host_is_recognised_by_its_second_label() {
        // Rows differ along the api id, the region, the port, the scheme-level userinfo
        // and the endpoint variant — everything the rule generalises over — plus the hosts
        // that are NOT an api's invoke endpoint.
        let cases: &[(&str, Option<&str>)] = &[
            ("abc1230000.execute-api.us-east-1.amazonaws.com", Some("abc1230000")),
            ("0123456789.execute-api.eu-west-3.amazonaws.com", Some("0123456789")),
            ("a.execute-api.ap-southeast-2.amazonaws.com:443", Some("a")),
            (
                "abc1230000.execute-api-fips.us-gov-west-1.amazonaws.com",
                Some("abc1230000"),
            ),
            ("abc1230000.execute-api.cn-north-1.amazonaws.com.cn", Some("abc1230000")),
            // A label that merely starts with the endpoint's: a different host.
            ("abc1230000.execute-apifoo.us-east-1.amazonaws.com", None),
            // The control plane, whose first label names the service.
            ("apigateway.us-east-1.amazonaws.com", None),
            // The management API's own endpoint host: no api id in front of the label.
            ("execute-api.us-east-1.amazonaws.com", None),
            // Not an endpoint at all.
            ("127.0.0.1:9297", None),
            ("localhost", None),
            ("abc1230000.execute-api", None),
        ];
        for (host, expected) in cases {
            let resolved = api_id_of_host(host);
            assert_eq!(
                resolved, *expected,
                "case {host:?}: expected the api id {expected:?}, got {resolved:?}"
            );
        }
    }

    #[test]
    fn a_template_is_velocity_only_when_it_carries_a_reference_or_a_directive() {
        // Rows differ along the construct and along the literal characters that merely
        // LOOK like one: a body that needs no evaluation is served verbatim, so the
        // detector must not claim it does.
        let cases: &[(&str, bool)] = &[
            (r#"{"ok":true}"#, false),
            ("", false),
            // Literal `#` and `$` that introduce nothing.
            (r##"{"color":"#fff"}"##, false),
            (r#"{"price":"$5"}"#, false),
            (r##"{"anchor":"#"}"##, false),
            // Non-ASCII text, which nothing restricts a stored template to: the scan must
            // answer for it rather than fail on a byte index inside a character.
            (r#"{"msg":"café"}"#, false),
            ("☃", false),
            ("🙂 $ 🙂", false),
            (r#"{"msg":"café $input.params('x')"}"#, true),
            ("☃#set($x = 1)", true),
            // References.
            (r#"{"id":"$input.params('id')"}"#, true),
            (r#"{"id":"${input}"}"#, true),
            (r#"{"id":"$!input"}"#, true),
            // Directives and comments.
            ("#set($root = $input.path('$'))", true),
            ("#if($x)a#end", true),
            // A word that merely starts with a directive's name is literal text.
            ("#sets", false),
            ("#ifconfig", false),
            (r##"{"tag":"#end2end"}"##, false),
            ("#end", true),
            ("#set($x = 1)", true),
            ("## a comment", true),
            ("#* a block comment *#", true),
        ];
        for (template, velocity) in cases {
            let detected = is_velocity_template(template);
            assert_eq!(
                detected, *velocity,
                "case {template:?}: expected is_velocity_template to be {velocity}, got \
                 {detected}"
            );
        }
    }

    #[test]
    fn a_literal_segment_beats_a_parameter_which_beats_a_greedy_parameter() {
        // Rows differ along the template's form and the request's segment count: the
        // dimensions the specificity rule generalises over.
        let cases: &[(&str, &str, bool)] = &[
            ("/", "/", true),
            ("/pets", "/pets", true),
            ("/pets", "/pets/1", false),
            ("/pets/{petId}", "/pets/1", true),
            ("/pets/{petId}", "/pets", false),
            ("/pets/{petId}", "/pets/1/toys", false),
            ("/{proxy+}", "/pets/1/toys", true),
            ("/{proxy+}", "/", false),
            ("/pets/{proxy+}", "/pets/1/toys", true),
        ];
        for (template, request, matches) in cases {
            let scored = match_score(template, request).is_some();
            assert_eq!(
                scored, *matches,
                "case {template:?} vs {request:?}: expected a match of {matches}, got {scored}"
            );
        }
        let literal = match_score("/pets/mine", "/pets/mine").expect("the literal matches");
        let parameter = match_score("/pets/{petId}", "/pets/mine").expect("the parameter matches");
        let greedy = match_score("/{proxy+}", "/pets/mine").expect("the greedy parameter matches");
        assert!(
            literal > parameter && parameter > greedy,
            "case specificity: expected literal > parameter > greedy, got \
             {literal:?} > {parameter:?} > {greedy:?}"
        );
    }
}
