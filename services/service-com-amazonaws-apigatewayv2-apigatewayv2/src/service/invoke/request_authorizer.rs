// SPDX-License-Identifier: CC0-1.0

//! The decision a `REQUEST` (Lambda) authorizer makes about one invoke, and what is done
//! with it: the invocation of the authorizer's own function, the two response formats that
//! decision can arrive in, and the cache the decision is kept in.
//!
//! "REQUEST … for a Lambda function using incoming request parameters"
//! (`Authorizer$AuthorizerType`) and "for REQUEST authorizers, this must be a well-formed
//! Lambda function URI" (`Authorizer$AuthorizerUri`): the decision IS that function's
//! answer, so it is obtained by invoking it ([`super::lambda`]) rather than derived from
//! anything here. Two things around it are the service's own, and both are computed from
//! stored records:
//!
//! * WHETHER the function is invoked at all. Every identity source must be "present in the
//!   request, not null, and non-empty", and "only when this is true does the authorizer
//!   invoke the authorizer Lambda function. Otherwise, it returns a 401 Unauthorized
//!   response without calling the Lambda function" (`Authorizer$IdentitySource`) — the
//!   precondition `super::invoke` applies before this module is reached — and, past it,
//!   whether a decision for this identity is already cached.
//! * HOW the answer is read. "Specifies whether a Lambda authorizer returns a response in a
//!   simple format. By default, a Lambda authorizer must return an IAM policy. If enabled,
//!   the Lambda authorizer can return a boolean value instead of an IAM policy"
//!   (`Authorizer$EnableSimpleResponses`), and the simple format is defined for payload
//!   format `2.0` only — "supported only for HTTP API Lambda authorizers". So an authorizer
//!   that did not enable it, and every `1.0` authorizer, is read as a policy DOCUMENT, and a
//!   function that answered the other format answered something this authorizer's
//!   configuration cannot read: that is a configuration fault of the api, answered with the
//!   gateway's own 500, not an allow.
//!
//! A policy document is evaluated by the tree's ONE evaluation of policy documents
//! (`::service_common::iam_authorization`), against the `execute-api:Invoke` action on the
//! route the request addressed — which is the ARN the authorizer event names, so the
//! function and the gateway are deciding about the same resource.
//!
//! What a refused request is answered with is the status AWS answers for each kind of
//! refusal: a decision of DENY is `403` `{"message":"Forbidden"}` — the authorizer ran and
//! said no — while a request whose identity sources are not all present never reaches the
//! function and is `401` `{"message":"Unauthorized"}`.

use std::time::{Duration, Instant};

use aws_smithy_runtime_api::http::Response as RtResponse;
use serde_json::Value;
use smithy_server_runtime::OperationContext;

use super::super::data_plane::{forbidden, json_response, UNPLACEABLE_STATUS};
use super::lambda;
use super::proxy_event::{self, Invocation, ProxyRequest, PAYLOAD_FORMAT_1_0, PAYLOAD_FORMAT_2_0};
use crate::state::authorizer_cache::{result_ttl, CachedDecision};
use crate::state::Authorizer;

/// The IAM action an invoke of an api's route asks for, which a policy-document authorizer's
/// statements name: AWS publishes the `execute-api` endpoint's invoke permission as
/// `execute-api:Invoke`.
const INVOKE_ACTION: &str = "execute-api:Invoke";

/// The service namespace the route ARN an authorizer decides about is in.
const EXECUTE_API_SERVICE: &str = "execute-api";

/// How long an authorizer's own function is given to answer.
///
/// `Authorizer` declares no timeout member of its own, and AWS publishes 10 s as the limit
/// an authorizer function has to answer in, after which the gateway answers its own failure
/// rather than waiting. It bounds the run the way an integration's `TimeoutInMillis` bounds
/// a route's, so a function that never returns cannot hold the request open.
const AUTHORIZER_TIMEOUT: Duration = Duration::from_secs(10);

/// What the authorizer decided, for the caller to carry into the integration's event.
#[derive(Debug, Clone)]
pub struct Decided {
    /// The context the function returned beside its decision, carried into the integration
    /// event unchanged.
    pub context: Option<Value>,
    /// The principal a policy-document authorizer named.
    pub principal_id: Option<String>,
}

/// The decision for one invoke: the authorizer allowed it (with what it returned), or the
/// request is refused with this response.
pub enum Outcome {
    /// The authorizer authorized the request.
    Allowed(Decided),
    /// The request is refused, with the answer the caller receives.
    Refused(Box<RtResponse>),
}

/// Decide one invoke with `authorizer`, re-using a cached decision while one is live and
/// invoking the function otherwise.
///
/// `identity` is the identity-source values the decision is about — the cache key AWS uses
/// ("API Gateway uses the identity sources as the cache key") and the `identitySource` the
/// event carries.
#[allow(clippy::too_many_arguments)]
pub async fn decide<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &OperationContext<'_>,
    authorizer: &Authorizer,
    request: &ProxyRequest,
    invocation: &Invocation,
    identity: &[String],
    method: &str,
) -> Outcome {
    let api_id = request.api_id.as_str();
    let stage = request.stage_name.as_str();
    let decided = crate::state::authorizer_cache::Decided {
        api_id,
        stage_name: stage,
        authorizer_id: &authorizer.authorizer_id,
        identity,
    };
    // A decision already made for this identity, while the TTL the authorizer declared has
    // not passed: AWS does not invoke the function again for it, so neither does this.
    if let Some(state) = crate::state::service_state(engine) {
        let cached = state.lock().authorizer_cache.live(decided, Instant::now());
        if let Some(cached) = cached {
            ::tracing::debug!(
                api_id = %api_id,
                stage = %stage,
                authorizer_id = %authorizer.authorizer_id,
                authorized = cached.authorized,
                "serving an invoke from the authorizer decision already cached for this \
                 identity"
            );
            return outcome_of(&cached);
        }
    }

    let payload_format = payload_format(authorizer);
    let Some(target) = authorizer
        .authorizer_uri
        .as_deref()
        .and_then(lambda::function_target)
    else {
        return unplaceable(
            authorizer,
            "the authorizer's AuthorizerUri names no Lambda function to invoke",
        );
    };
    let route_arn = route_arn(request, method);
    let event = proxy_event::authorizer_event(
        request,
        invocation,
        &payload_format,
        identity,
        &route_arn,
    );
    let payload = serde_json::to_vec(&event).unwrap_or_default();
    let ran = match lambda::invoke_function(ctx, &target, payload, AUTHORIZER_TIMEOUT).await {
        Ok(ran) => ran,
        Err(unreached) => return unplaceable(authorizer, &unreached.reason()),
    };
    if ran.failed() {
        // "An unhandled error is still HTTP 200" to Lambda's caller, so the run is told
        // from a decision by the signal rather than by the status: a function that raised
        // returned no decision at all.
        return unplaceable(
            authorizer,
            "the authorizer's function ended in an unhandled error, so it returned no \
             decision",
        );
    }
    let decision = match read_decision(&ran.payload, &payload_format, authorizer, &route_arn) {
        Ok(decision) => decision,
        Err(reason) => return unplaceable(authorizer, &reason),
    };
    // The decision is kept for the TTL the authorizer declares, so a later request
    // presenting the same identity is served by it without the function running again.
    if let Some(state) = crate::state::service_state(engine) {
        state.lock().authorizer_cache.store(
            decided,
            decision.clone(),
            result_ttl(authorizer.authorizer_result_ttl_in_seconds),
            Instant::now(),
        );
    }
    outcome_of(&decision)
}

/// The answer a decision owes the caller: the integration for an authorized request, and
/// `403` `{"message":"Forbidden"}` for one the authorizer refused — the authorizer ran and
/// said no, which is not the `401` of a request whose identity sources were not all there.
fn outcome_of(decision: &CachedDecision) -> Outcome {
    if decision.authorized {
        return Outcome::Allowed(Decided {
            context: decision
                .context
                .as_deref()
                .and_then(|context| serde_json::from_str(context).ok()),
            principal_id: decision.principal_id.clone(),
        });
    }
    Outcome::Refused(Box::new(forbidden()))
}

/// The refusal for an authorizer whose decision could not be obtained at all: the gateway's
/// own `500`, with the reason logged rather than told to the caller. An allow here would be
/// a decision no authorizer made.
fn unplaceable(authorizer: &Authorizer, reason: &str) -> Outcome {
    ::tracing::warn!(
        authorizer_id = %authorizer.authorizer_id,
        authorizer_uri = ?authorizer.authorizer_uri,
        reason = %reason,
        "refusing an invoke: this route's Lambda authorizer returned no decision"
    );
    Outcome::Refused(Box::new(json_response(
        UNPLACEABLE_STATUS,
        super::INTERNAL_MESSAGE,
        None,
    )))
}

/// The payload format the authorizer's function is called in: its own
/// `AuthorizerPayloadFormatVersion`, and `1.0` for one that declares none — the format a
/// `REQUEST` authorizer predates the `2.0` event with, and the one whose response is a
/// policy document.
fn payload_format(authorizer: &Authorizer) -> String {
    authorizer
        .authorizer_payload_format_version
        .as_deref()
        .map(str::trim)
        .filter(|version| !version.is_empty())
        .unwrap_or(PAYLOAD_FORMAT_1_0)
        .to_string()
}

/// Whether this authorizer's answer is read as the SIMPLE response: it enabled simple
/// responses AND its payload format defines them. "Supported only for HTTP API Lambda
/// authorizers" with payload format `2.0`, so a `1.0` authorizer's answer is a policy
/// document whatever the flag says.
fn reads_simple_response(authorizer: &Authorizer, payload_format: &str) -> bool {
    authorizer.enable_simple_responses == Some(true) && payload_format == PAYLOAD_FORMAT_2_0
}

/// The decision the function's `payload` states, or the reason it states none this
/// authorizer can read.
fn read_decision(
    payload: &[u8],
    payload_format: &str,
    authorizer: &Authorizer,
    route_arn: &str,
) -> Result<CachedDecision, String> {
    let document: Value = serde_json::from_slice(payload)
        .map_err(|error| format!("the authorizer's response is not valid JSON: {error}"))?;
    let context = document
        .get("context")
        .filter(|context| !context.is_null())
        .map(|context| context.to_string());
    if reads_simple_response(authorizer, payload_format) {
        // "the Lambda authorizer can return a boolean value instead of an IAM policy": the
        // member that carries it is `isAuthorized`, and a response naming none states no
        // decision in this format.
        let authorized = document
            .get("isAuthorized")
            .and_then(Value::as_bool)
            .ok_or_else(|| {
                "the authorizer enables simple responses and its function returned no \
                 isAuthorized member, so it stated no decision"
                    .to_string()
            })?;
        return Ok(CachedDecision {
            authorized,
            context,
            principal_id: None,
        });
    }
    // Otherwise the answer must be an IAM policy: "By default, a Lambda authorizer must
    // return an IAM policy". A response that carries none states no decision this
    // authorizer can read — which is what a SIMPLE response under payload format `1.0`
    // is.
    let policy = document
        .get("policyDocument")
        .filter(|policy| !policy.is_null())
        .ok_or_else(|| {
            "the authorizer returns an IAM policy and its function returned no \
             policyDocument member, so it stated no decision"
                .to_string()
        })?;
    let decision = ::service_common::iam_authorization::identity_decision(
        &[policy.to_string()],
        &::service_common::iam_authorization::Request::new(INVOKE_ACTION, route_arn, &[]),
    );
    let authorized = matches!(
        decision,
        ::service_common::iam_authorization::Decision::Allowed
    );
    Ok(CachedDecision {
        authorized,
        context,
        principal_id: document
            .get("principalId")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

/// The ARN of the route this decision is about, which the event names and the policy
/// document's `Resource` is matched against:
/// `arn:aws:execute-api:{region}:{account}:{api-id}/{stage}/{method}/{path}`, the form AWS
/// publishes for an `execute-api` resource.
///
/// The path is the one WITHIN the stage, with its leading `/` removed, because the ARN's
/// own separator between the method and the path is already a `/`.
pub fn route_arn(request: &ProxyRequest, method: &str) -> String {
    let path = request.route_path.trim_start_matches('/');
    ::emulator_server::emulator_arn(
        EXECUTE_API_SERVICE,
        &request.region,
        &format!(
            "{api}/{stage}/{method}/{path}",
            api = request.api_id,
            stage = request.stage_name
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// An authorizer of the given payload format, with simple responses as `simple` says.
    fn authorizer(payload_format: &str, simple: bool) -> Authorizer {
        Authorizer {
            authorizer_id: "auth1".to_string(),
            authorizer_type: "REQUEST".to_string(),
            authorizer_payload_format_version: Some(payload_format.to_string()),
            enable_simple_responses: Some(simple),
            ..Authorizer::default()
        }
    }

    /// The route ARN the policy rows below are matched against.
    const ARN: &str = "arn:aws:execute-api:us-east-1:000000000000:abc123/$default/GET/orders";

    #[test]
    fn a_simple_response_is_read_only_where_the_configuration_defines_one() {
        // Rows differ along the pair (payload format, EnableSimpleResponses) and the
        // response the function gave: each combination is read the way the authorizer's own
        // configuration says, and a response in the other format states no decision.
        let simple = r#"{"isAuthorized":true,"context":{"tier":["gold"]}}"#;
        let policy = format!(
            r#"{{"principalId":"user","policyDocument":{{"Version":"2012-10-17",
                "Statement":[{{"Action":"execute-api:Invoke","Effect":"Allow",
                "Resource":"{ARN}"}}]}}}}"#
        );
        let cases: [(&str, &str, bool, &str, Option<bool>); 6] = [
            (
                "a simple response from a 2.0 authorizer that enabled them",
                PAYLOAD_FORMAT_2_0,
                true,
                simple,
                Some(true),
            ),
            (
                "a simple response from a 2.0 authorizer that did not",
                PAYLOAD_FORMAT_2_0,
                false,
                simple,
                None,
            ),
            (
                "a simple response under payload format 1.0, which defines none",
                PAYLOAD_FORMAT_1_0,
                true,
                simple,
                None,
            ),
            (
                "a policy document from a 1.0 authorizer",
                PAYLOAD_FORMAT_1_0,
                false,
                policy.as_str(),
                Some(true),
            ),
            (
                "a policy document from a 2.0 authorizer that did not enable simple \
                 responses",
                PAYLOAD_FORMAT_2_0,
                false,
                policy.as_str(),
                Some(true),
            ),
            (
                "a policy document from an authorizer that enabled simple responses",
                PAYLOAD_FORMAT_2_0,
                true,
                policy.as_str(),
                None,
            ),
        ];
        for (case, format, simple_responses, response, expected) in cases {
            let read = read_decision(
                response.as_bytes(),
                format,
                &authorizer(format, simple_responses),
                ARN,
            );
            let actual = read.as_ref().ok().map(|decision| decision.authorized);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                read.map(|decision| decision.authorized)
            );
        }
    }

    #[test]
    fn a_policy_documents_statements_decide_the_route_it_names() {
        // Rows differ along what the document says about THIS route: an allow naming it, an
        // allow naming it through a wildcard, an explicit deny, an allow of another route,
        // an allow of another action, and a document with no statement at all.
        let cases: [(&str, &str, bool); 6] = [
            (
                "an allow naming this route",
                r#"{"Statement":[{"Action":"execute-api:Invoke","Effect":"Allow","Resource":"arn:aws:execute-api:us-east-1:000000000000:abc123/$default/GET/orders"}]}"#,
                true,
            ),
            (
                "an allow naming every route of the api",
                r#"{"Statement":[{"Action":"execute-api:Invoke","Effect":"Allow","Resource":"arn:aws:execute-api:us-east-1:000000000000:abc123/*"}]}"#,
                true,
            ),
            (
                "an explicit deny of this route",
                r#"{"Statement":[{"Action":"execute-api:Invoke","Effect":"Deny","Resource":"*"}]}"#,
                false,
            ),
            (
                "an allow of another route only",
                r#"{"Statement":[{"Action":"execute-api:Invoke","Effect":"Allow","Resource":"arn:aws:execute-api:us-east-1:000000000000:abc123/$default/GET/invoices"}]}"#,
                false,
            ),
            (
                "an allow of another action",
                r#"{"Statement":[{"Action":"s3:GetObject","Effect":"Allow","Resource":"*"}]}"#,
                false,
            ),
            ("a document that states nothing", r#"{"Statement":[]}"#, false),
        ];
        for (case, policy, expected) in cases {
            let response = format!(r#"{{"principalId":"user","policyDocument":{policy}}}"#);
            let read = read_decision(
                response.as_bytes(),
                PAYLOAD_FORMAT_1_0,
                &authorizer(PAYLOAD_FORMAT_1_0, false),
                ARN,
            );
            let actual = read.as_ref().map(|decision| decision.authorized);
            assert_eq!(
                actual.ok(),
                Some(expected),
                "case {case:?}: expected authorized {expected}, got {actual:?}"
            );
        }
    }

    #[test]
    fn the_route_arn_names_the_api_the_stage_the_method_and_the_path() {
        let request = ProxyRequest {
            method: "GET".to_string(),
            raw_path: "/prod/orders".to_string(),
            route_path: "/orders".to_string(),
            raw_query_string: String::new(),
            headers: BTreeMap::new(),
            query_parameters: BTreeMap::new(),
            body: Vec::new(),
            route_key: "GET /orders".to_string(),
            api_id: "abc123".to_string(),
            stage_name: "prod".to_string(),
            stage_variables: BTreeMap::new(),
            region: "eu-west-1".to_string(),
            domain_name: "abc123.execute-api.eu-west-1.amazonaws.com".to_string(),
        };
        let actual = route_arn(&request, "GET");
        let expected = format!(
            "arn:aws:execute-api:eu-west-1:{}:abc123/prod/GET/orders",
            ::engine::emulator_account_id()
        );
        assert_eq!(
            actual, expected,
            "case {:?}: expected {expected}, got {actual}",
            "the ARN of a route of a named stage"
        );
    }
}
