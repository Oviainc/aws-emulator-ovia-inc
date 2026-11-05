// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-appconfig-amazonappconfig` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! EDIT THIS FILE. It is emitted once with working defaults and is never
//! regenerated, so a choice made here survives adding another service or
//! re-running the generator.
//!
//! `EmulatedService` is what the router mounts (`EmulatedService::new(engine)`).
//! It wraps the generated [`ServiceDefinition`], which carries the model-derived half
//! of this service — its identity, its wire protocol, and its operation routing
//! table — built from the Smithy model at compile time. You do not edit the
//! entry; you edit the behavioral seams below.
//!
//! Five opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name. Edit it
//!   for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

mod immutable_profile_type;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::{envelope, Violation, ViolationKind};
use aws_smithy_types::body::SdkBody;
use serde_json::Value;
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refusal,
    Refused, Service,
};
use std::sync::Arc;

/// The body member every error shape in this service's model reports its detail text in
/// (`smithy_query(query="shape",
/// shape="BadRequestException,ResourceNotFoundException,ConflictException,InternalServerException,ServiceQuotaExceededException,PayloadTooLargeException")`
/// — each declares `Message`, none with a `@jsonName`). This is a restJson1 service, so a
/// member's wire name IS its Smithy member name, and that is the one member an AppConfig
/// client reads an error's text out of.
const DETAIL_MEMBER: &str = "Message";

/// The member the FRAMEWORK's own error renderers write that text in: the member name of
/// the synthetic `smithy.framework#ValidationException` / `NotImplemented` shapes the
/// pre-handler refusal and the scaffold 501 are built from.
const RENDERED_DETAIL_MEMBER: &str = "message";

/// Every error shape this service's model declares, by the wire code (`__type`) it renders
/// under — the same `smithy_query` call [`DETAIL_MEMBER`] cites, which is where this list
/// comes from and the only thing that may extend it.
///
/// The rename below is keyed on THIS set rather than on the status, so a failure whose
/// class AppConfig does not model keeps the member the framework wrote its text in: the
/// scaffold's own 501 `NotImplemented` (the emulator saying the operation is not written
/// yet) and an authentication refusal, which is the emulator's and which
/// [`EmulatedService::override_refusal`] deliberately leaves in the framework's words.
const MODELLED_ERROR_SHAPES: [&str; 6] = [
    "BadRequestException",
    "ConflictException",
    "InternalServerException",
    "PayloadTooLargeException",
    "ResourceNotFoundException",
    "ServiceQuotaExceededException",
];

/// A `@pattern` requirement as this service quotes it: a WHOLE-VALUE expression without
/// its end anchor.
///
/// The requirement a constraint refusal states is the expression a value has to satisfy,
/// and AppConfig states it without the trailing `$` a model writes to demand a match over
/// the whole value — satisfying the expression IS the whole-value demand, so the end
/// anchor is not part of what the refusal advertises. Everything else in the expression is
/// quoted verbatim: the leading `^`, an anchor a branch of the expression carries of its
/// own (`DeploymentStrategyId` is modelled
/// `^(^[a-z0-9]{4,7}$|^AppConfig\.[A-Za-z0-9]{9,40}$)$`), and an expression that is not a
/// whole-value one at all (`VersionLabel` is modelled `[^0-9]`), which is quoted as
/// modelled because no anchor pair wraps it.
///
/// The drop therefore needs BOTH ends: a `^` at the start and a `$` at the end, the pair a
/// model writes around the expression it means to match in full. An expression anchored at
/// one end only is a different expression, and nothing records how this service would
/// quote one, so it is quoted as modelled. A trailing `$` preceded by an ODD number of
/// backslashes is an escaped literal dollar rather than the anchor, so nothing is dropped
/// there either.
///
/// Only the RENDERING drops it. The CHECK runs the model's pattern as written, so a value
/// that satisfies the quoted expression over a PREFIX of itself is still refused.
///
/// This is NOT the fleet rule `service_common::validation::unanchored` states (which drops
/// the whole `^`/`$` PAIR, the rendering those services' own recordings show): AppConfig's
/// refusals keep the leading `^`, so quoting a pattern is part of this service's
/// validation-message dialect and is stated here, for every operation at once.
fn quoted_pattern(pattern: &str) -> &str {
    if !pattern.starts_with('^') {
        return pattern;
    }
    without_end_anchor(pattern).unwrap_or(pattern)
}

/// `pattern` without its end anchor, or `None` when it carries none.
///
/// The same three-line primitive is inlined in several service crates beside
/// `service_common::validation::unanchored`'s pair-dropping rule; what is AppConfig's own
/// is [`quoted_pattern`]'s choice to drop ONLY this anchor.
fn without_end_anchor(pattern: &str) -> Option<&str> {
    let body = pattern.strip_suffix('$')?;
    let backslashes = body.len() - body.trim_end_matches('\\').len();
    (backslashes % 2 == 0).then_some(body)
}

/// The same violation with a `@pattern` quoted the way this service quotes one. Every
/// other requirement class — a null member, a length, a numeric bound, an enum value set,
/// a uniqueness demand — is handed back untouched, so its wording, the member path and the
/// echoed value stay the framework's and cannot drift per operation.
pub(crate) fn as_stated(violation: &Violation) -> Violation {
    match &violation.kind {
        ViolationKind::Pattern(pattern) => Violation {
            path: violation.path.clone(),
            kind: ViolationKind::Pattern(quoted_pattern(pattern).to_string()),
            value: violation.value.clone(),
            sensitive: violation.sensitive,
        },
        _ => violation.clone(),
    }
}

/// The error body with its detail member renamed to [`DETAIL_MEMBER`], or `None` when
/// there is nothing to rename: the body is not a JSON object, its `__type` names no error
/// shape this service's model declares (see [`MODELLED_ERROR_SHAPES`]), or it carries no
/// rendered detail member.
///
/// Idempotent, and non-destructive: a body that already carries a `Message` member keeps
/// that value, so a member of the MODELLED error shape always wins over the renamed one.
fn with_detail_member(body: &[u8]) -> Option<Vec<u8>> {
    let mut document: Value = serde_json::from_slice(body).ok()?;
    let object = document.as_object_mut()?;
    // The `__type` may be namespaced (`com.amazonaws.appconfig#BadRequestException`), so
    // the shape name is what is matched.
    let shape = object
        .get("__type")
        .and_then(Value::as_str)?
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    if !MODELLED_ERROR_SHAPES.contains(&shape.as_str()) {
        return None;
    }
    let detail = object.remove(RENDERED_DETAIL_MEMBER)?;
    object.entry(DETAIL_MEMBER).or_insert(detail);
    serde_json::to_vec(&document).ok()
}

/// The response as this service answers it: an error body whose detail text is in the
/// `Message` member its model declares for it.
///
/// A response that is not a failure, or whose body is not a modelled error body carrying a
/// rendered detail member (see [`with_detail_member`]), is returned untouched — status,
/// headers and every byte of the body exactly as the renderer wrote them. A modelled
/// error a handler returns is serialized from the model's own shape, so it already
/// carries `Message` and nothing is rewritten.
fn with_service_detail_member(mut resp: RtResponse) -> RtResponse {
    if resp.status().as_u16() < 400 {
        return resp;
    }
    let rewritten = {
        // An error body is built in memory by its renderer, so the bytes are readable
        // here; a streamed body is not an error body and is left alone.
        let Some(bytes) = resp.body().bytes() else {
            return resp;
        };
        match with_detail_member(bytes) {
            Some(rewritten) => rewritten,
            None => return resp,
        }
    };
    // Only a renderer that advertised a length needs it corrected; the framework's error
    // renderers set `content-type` alone and leave framing to the transport.
    if resp.headers().get("content-length").is_some() {
        let length = rewritten.len().to_string();
        // A decimal length is always a valid header value, so this cannot fail today; if
        // it ever did, the header is REMOVED rather than left describing the body before
        // the rename, so the two can never disagree.
        if resp
            .headers_mut()
            .try_insert("content-length", length)
            .is_err()
        {
            resp.headers_mut().remove("content-length");
        }
    }
    *resp.body_mut() = SdkBody::from(rewritten);
    resp
}

/// The error code this service names for a request that broke a modelled constraint.
///
/// AppConfig's model declares NO `ValidationException` anywhere: every operation lists
/// `BadRequestException` (HTTP 400) for an input its constraints refuse, so that is the
/// code a client switches on, and it is stated once here for both paths that render such a
/// refusal ([`InServiceDialect`] and [`EmulatedService::override_refusal`]).
const CONSTRAINT_REFUSAL_CODE: &str = "BadRequestException";

/// This service's answer to the violations `req` broke, or `None` when it broke none.
///
/// The sentence is the framework's own `N validation error(s) detected: Value '<v>' at
/// '<member>' failed to satisfy constraint: …` envelope — already the text AWS states for a
/// constraint violation, and built from the request's own member path and value — with each
/// violation restated through [`as_stated`] for the one thing AppConfig words differently,
/// how a `@pattern` requirement is quoted. The status and the envelope stay the
/// framework's, through the shared renderer every service that words its own pre-handler
/// refusal answers from (`service_common::front_door::worded_rejection`): a constraint
/// failure is a 400 in every protocol.
///
/// The values it quotes are the bytes the CALLER SENT. `service_common::validation`
/// re-reads the same built input the constraint walk validated to restore each echo,
/// because the framework stores a long value already shortened to a prefix plus a
/// byte-count annotation — text no AWS service emits — and the echoed value is part of the
/// request-derived rule a caller reads the rejected input back out of.
fn constraint_refusal<O: Operation>(svc: &dyn Service, req: &RtRequest) -> Option<RtResponse> {
    // The protocol is read, used and dropped inside this one function, so nothing borrowed
    // from the service definition is held across the caller's await: `&dyn
    // ErasedServiceDefinition` is not `Sync`.
    let violations =
        service_common::validation::refusing_violations::<O>(svc.definition().protocol(), req);
    let stated: Vec<Violation> = violations.iter().map(as_stated).collect();
    let message = envelope(&stated)?;
    Some(service_common::front_door::worded_rejection(
        svc,
        req,
        violations,
        Refused {
            code: CONSTRAINT_REFUSAL_CODE.to_string(),
            message,
        },
    ))
}

/// Serves one modelled operation, with every failure of it stated in this service's own
/// dialect: a pre-handler constraint refusal QUOTING the value the caller submitted, and
/// the detail text of whatever is answered reported in the `Message` member this service's
/// model declares for it.
///
/// WHY a wrapper and not [`Service::override_refusal`]: that seam is handed the violations
/// ALONE — no operation, no request — so it can neither recover a value the framework
/// already shortened as it BUILT the violation, nor name the operation whose input schema
/// the request would have to be re-read against. Nor can it choose the body member the
/// words are serialized into: the framework writes a framework-rendered error's text into
/// the synthetic shapes' own `message`. A per-operation wrapper has the request, the
/// operation and the whole response, so it can do all three.
///
/// WHY not `service_common::front_door::FrontDoor`, which is the same wrapping for the
/// services whose own front door refuses a body: that wrapper narrows its refusal to the
/// violations a member BOUND TO THE BODY broke (`binds_to_body`) and words them as that
/// layer's fixed sentence, and AppConfig does neither — it refuses a `@httpLabel`, an
/// `@httpQuery` and an `@httpHeader` outside its constraints in the same modelled
/// `BadRequestException` as a body member, which `pattern_refusal_quoting` pins per
/// binding. Everything that is NOT this service's own is taken from the shared module all
/// the same: the collecting walk ([`constraint_refusal`]), the rendering
/// (`front_door::worded_rejection`) and the key an operation is filed under ([`served`]).
/// What stays here is the two things that are AppConfig's: refusing over EVERY violation
/// rather than the body's, and the `Message` member its model declares.
///
/// Nothing else is re-implemented. A request this wrapper does not refuse is handed to the
/// generated [`ErasedOperation::run`], which owns deserialize -> validate -> handle ->
/// serialize, telemetry and every modelled error. The violations come from the framework's
/// own machinery (the protocol's deserializer and its constraint walk over the built
/// input), so this cannot find a different set from the walk that follows it; the cost is
/// that a request it does not refuse is read twice, bounded by the body limits the
/// framework already enforces.
struct InServiceDialect<O> {
    operation: O,
}

impl<O> InServiceDialect<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for InServiceDialect<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            if let Some(refused) = constraint_refusal::<O>(svc, req) {
                return with_service_detail_member(refused);
            }
            with_service_detail_member(self.operation.run(svc, req, ctx).await)
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// Whether this service serves the request these `parts` came from.
///
/// `emulator_server::can_handle_based_on_route_key_and_path` is the agent-immutable
/// owner of this rule, and it is CALLED here rather than re-implemented: the request's
/// route key must match this service's signing name AND its first URI path segment
/// must be one the generated, model-derived `ROUTE_PATH_SEGMENTS` declares, by exact
/// segment equality. That path conjunct is what separates two services sharing one
/// SigV4 signing name — the AppConfig control plane and the AppConfigData data plane
/// both sign as `appconfig`, so the key alone matches both and the router would reject
/// every such request as `500 AmbiguousService` — and their modelled path spaces are
/// disjoint (`applications`, `deploymentstrategies`, `extensions`, … here;
/// `configuration` and `configurationsessions` there).
///
/// The helper takes a whole `http::Request` while [`Service::can_handle`] is handed
/// only the request's parts, so the two things it reads — the URI and the headers —
/// are re-presented to it as a bodiless request. It reads neither the body nor the
/// method, so nothing it decides on is missing.
fn can_handle_parts(parts: &http::request::Parts) -> bool {
    let Ok(mut request) = http::Request::builder()
        .uri(parts.uri.clone())
        .body(bytes::Bytes::new())
    else {
        return false;
    };
    *request.headers_mut() = parts.headers.clone();
    emulator_server::can_handle_based_on_route_key_and_path(
        &request,
        crate::ROUTE_KEY,
        crate::ROUTE_PATH_SEGMENTS,
    )
}

/// The `com-amazonaws-appconfig-amazonappconfig` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each served through
    /// [`InServiceDialect`]: a pre-handler constraint refusal quotes the value the caller
    /// submitted, and every error's detail text is reported in the `Message` member this
    /// service's model declares for it. Keyed by the routing key the protocol resolves —
    /// the key the generated table uses — and built from the SAME shared engine, so a
    /// wrapped operation reads and writes the one store.
    in_service_dialect: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table` through [`served`].
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            served(
                &mut $table,
                crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
            );
        )+
    };
}

/// Serve `operation` through [`InServiceDialect`], filed under its own modelled
/// [`Operation::NAME`] — the routing key the protocol resolves and the generated table is
/// keyed by, so the key cannot be spelled by hand. The same keying
/// `service_common::front_door::FrontDoorTable::serve` does for the shared wrapper.
fn served<O: Operation + ErasedOperation + 'static>(
    table: &mut OperationRoutingTable,
    operation: O,
) {
    table.add_operation(O::NAME, Box::new(InServiceDialect::new(operation)));
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut in_service_dialect = OperationRoutingTable::new();
        // EVERY modelled operation, so both rules are applied by ONE code path rather
        // than per operation: an operation whose input declares no constraint simply
        // never has a violation to find, which the wrapper reads off that operation's own
        // input schema. `every_modelled_operation_is_served_in_this_services_dialect`
        // pins the list against the model.
        wrap_operations!(
            in_service_dialect,
            engine,
            create_application::CreateApplication,
            create_configuration_profile::CreateConfigurationProfile,
            create_deployment_strategy::CreateDeploymentStrategy,
            create_environment::CreateEnvironment,
            create_experiment_definition::CreateExperimentDefinition,
            create_extension::CreateExtension,
            create_extension_association::CreateExtensionAssociation,
            create_hosted_configuration_version::CreateHostedConfigurationVersion,
            delete_application::DeleteApplication,
            delete_configuration_profile::DeleteConfigurationProfile,
            delete_deployment_strategy::DeleteDeploymentStrategy,
            delete_environment::DeleteEnvironment,
            delete_experiment_definition::DeleteExperimentDefinition,
            delete_extension::DeleteExtension,
            delete_extension_association::DeleteExtensionAssociation,
            delete_hosted_configuration_version::DeleteHostedConfigurationVersion,
            get_account_settings::GetAccountSettings,
            get_application::GetApplication,
            get_configuration::GetConfiguration,
            get_configuration_profile::GetConfigurationProfile,
            get_deployment::GetDeployment,
            get_deployment_strategy::GetDeploymentStrategy,
            get_environment::GetEnvironment,
            get_experiment_definition::GetExperimentDefinition,
            get_experiment_run::GetExperimentRun,
            get_extension::GetExtension,
            get_extension_association::GetExtensionAssociation,
            get_hosted_configuration_version::GetHostedConfigurationVersion,
            list_applications::ListApplications,
            list_configuration_profiles::ListConfigurationProfiles,
            list_deployment_strategies::ListDeploymentStrategies,
            list_deployments::ListDeployments,
            list_environments::ListEnvironments,
            list_experiment_definitions::ListExperimentDefinitions,
            list_experiment_run_events::ListExperimentRunEvents,
            list_experiment_runs::ListExperimentRuns,
            list_extension_associations::ListExtensionAssociations,
            list_extensions::ListExtensions,
            list_hosted_configuration_versions::ListHostedConfigurationVersions,
            list_tags_for_resource::ListTagsForResource,
            start_deployment::StartDeployment,
            start_experiment_run::StartExperimentRun,
            stop_deployment::StopDeployment,
            stop_experiment_run::StopExperimentRun,
            tag_resource::TagResource,
            untag_resource::UntagResource,
            update_account_settings::UpdateAccountSettings,
            update_application::UpdateApplication,
            update_deployment_strategy::UpdateDeploymentStrategy,
            update_environment::UpdateEnvironment,
            update_experiment_definition::UpdateExperimentDefinition,
            update_experiment_run::UpdateExperimentRun,
            update_extension::UpdateExtension,
            update_extension_association::UpdateExtensionAssociation,
            validate_configuration::ValidateConfiguration,
        );
        // `UpdateConfigurationProfile` is served through the same dialect as every
        // operation above, behind the one guard a handler cannot apply itself: a request
        // asking the profile to become a different `Type` names a member the model does
        // not declare, which is dropped before any handler sees it. See
        // [`immutable_profile_type`].
        immutable_profile_type::served(&mut in_service_dialect, Arc::clone(&engine));
        Self {
            entry: crate::ServiceDefinition::new(engine),
            in_service_dialect,
        }
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // Delegated to the framework's own predicate — see [`can_handle_parts`]:
        // this service's signing name (`appconfig`) is shared with the AppConfigData
        // data plane, so the claim is the route key AND the model-derived first URI
        // path segment. Matching on the signing name alone would have both services
        // claim every `appconfig`-signed request, which the router answers with 500
        // `AmbiguousService` instead of serving either one.
        can_handle_parts(parts)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // NOT a re-route: the key is the one the generated table's own protocol
        // resolves, and the operation it names is the one served — wrapped in
        // [`InServiceDialect`] so a request refused for breaking a modelled constraint
        // quotes the value the caller submitted, and so an error the FRAMEWORK renders
        // (that refusal, the scaffold's 501) reports its detail text in the `Message`
        // member every error shape in this service's model declares, rather than in the
        // synthetic shapes' own `message`. A key the model does not name resolves to
        // `None` here exactly as it does in the generated table, so the router's
        // routing-error path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.in_service_dialect.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // Every modelled operation is served through [`InServiceDialect`], which refuses
        // a constraint violation itself — in this code, in these words, with the value
        // the caller submitted quoted in full — so this seam is the FALLBACK for a
        // refusal that reaches the framework's own walk anyway. Wording it here from the
        // same violations keeps the two paths from disagreeing: a value outside a
        // declared bound is this service's `BadRequestException` either way, with the
        // same sentence.
        //
        // AppConfig's model declares NO `ValidationException` anywhere: a request that
        // violates a declared constraint is refused with the modelled
        // `BadRequestException` (HTTP 400), which every operation in this service
        // lists, so that is the code a client switches on. The framework's default
        // names `ValidationException` — a class this service does not model, which an
        // SDK cannot classify — so the code is replaced here.
        //
        // The one thing this seam cannot do is quote a long value as the caller sent it:
        // it is handed the violations alone, not the request they were collected from,
        // and the framework shortens such a value as it BUILDS the violation. That is
        // why the wrapper collects the refusal itself.
        //
        // `Unauthenticated` keeps the framework's wording: authentication is the
        // emulator's, not AppConfig's, and this service models no error for it.
        match refusal {
            Refusal::Invalid(violations) => {
                let stated: Vec<Violation> = violations.iter().map(as_stated).collect();
                Some(Refused {
                    code: CONSTRAINT_REFUSAL_CODE.to_string(),
                    message: envelope(&stated)
                        .unwrap_or_else(|| "invalid request".to_string()),
                })
            }
            _ => None,
        }
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps the entry's protocol-correct
        // rendering. Return `Some(resp)` to reword that failure in your own
        // envelope. The error is borrowed, so the default renderer can still
        // consume it on `None`.
        let _ = error;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{as_stated, quoted_pattern, with_detail_member, EmulatedService};
    use aws_smithy_schema_server::{Violation, ViolationKind};
    use std::sync::Arc;

    /// RULE: a `@pattern` is quoted as the model's expression without its END anchor —
    /// the trailing `$` — whenever the model wrapped it as a whole-value expression, and
    /// nothing else about the expression is touched.
    ///
    /// The rows are every distinct SHAPE of `@pattern` this service's model carries
    /// (`smithy_query(query="shape", shape="FlagKey,Id,DeploymentStrategyId,VersionLabel,Arn")`
    /// and `smithy_query(query="operation", operation="CreateConfigurationProfile")` for the
    /// role-ARN and type members), plus the regex constructs that can hide an anchor: an
    /// escaped dollar, a dollar inside the expression, and each half-anchored form.
    #[test]
    fn a_pattern_is_quoted_without_its_end_anchor() {
        // (case, the modelled expression, the expression a clause quotes)
        let rows: [(&str, &str, &str); 12] = [
            (
                "the experiment flag key",
                r"^[a-z][a-zA-Z0-9_-]{1,64}$",
                r"^[a-z][a-zA-Z0-9_-]{1,64}",
            ),
            ("a minted resource id", r"^[a-z0-9]{4,7}$", r"^[a-z0-9]{4,7}"),
            // Branches that carry anchors of their own: only the one at the very end
            // goes, so the expression's own interior anchors are quoted as modelled.
            (
                "a strategy id, whose branches are anchored individually",
                r"^(^[a-z0-9]{4,7}$|^AppConfig\.[A-Za-z0-9]{9,40}$)$",
                r"^(^[a-z0-9]{4,7}$|^AppConfig\.[A-Za-z0-9]{9,40}$)",
            ),
            ("a version label", r"[^0-9]", r"[^0-9]"),
            (
                "a resource ARN",
                r"^arn:(aws[a-zA-Z-]*)?:[a-z]+:(\d{12})?:[a-zA-Z0-9-_/:.]+$",
                r"^arn:(aws[a-zA-Z-]*)?:[a-z]+:(\d{12})?:[a-zA-Z0-9-_/:.]+",
            ),
            (
                "a configuration profile type",
                r"^[a-zA-Z0-9\.\-]+$",
                r"^[a-zA-Z0-9\.\-]+",
            ),
            // An escaped trailing dollar is a literal dollar sign, not the anchor.
            ("an escaped trailing dollar", r"^costs\$", r"^costs\$"),
            // An escaped backslash before the `$` leaves the `$` an anchor.
            ("an escaped backslash then the anchor", r"^[a-z]\\$", r"^[a-z]\\"),
            // A dollar that is not at the end is not the end anchor either.
            ("a dollar inside the expression", r"^a$b$", r"^a$b"),
            ("anchored at the start only", r"^arn:aws.*", r"^arn:aws.*"),
            // Anchored at the END only: not the whole-value form a model wraps, and no
            // recording says how this service would quote one, so it is quoted as
            // modelled.
            ("anchored at the end only", r"[0-9]{12}$", r"[0-9]{12}$"),
            ("no anchor at all", r"\d{3}", r"\d{3}"),
        ];
        for (case, modelled, expected) in rows {
            let actual = quoted_pattern(modelled);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {modelled:?} to be quoted as {expected:?}, got \
                 {actual:?}"
            );
        }
    }

    /// RULE: only a `@pattern` requirement is restated — every other class keeps the
    /// framework's own wording, member path and echoed value.
    #[test]
    fn only_a_pattern_requirement_is_restated() {
        let pattern = Violation {
            path: "flagKey".to_string(),
            kind: ViolationKind::Pattern(r"^[a-z][a-zA-Z0-9_-]{1,64}$".to_string()),
            value: Some("1invalid".to_string()),
            sensitive: false,
        };
        let stated = as_stated(&pattern);
        assert_eq!(
            stated.kind,
            ViolationKind::Pattern(r"^[a-z][a-zA-Z0-9_-]{1,64}".to_string()),
            "expected the pattern quoted without its end anchor, got {:?}",
            stated.kind
        );
        assert_eq!(
            (stated.path.as_str(), stated.value.as_deref()),
            ("flagKey", Some("1invalid")),
            "expected the framework's own path and echoed value, got {:?}",
            (stated.path.as_str(), stated.value.as_deref())
        );

        for other in [
            ViolationKind::MissingRequiredMember,
            ViolationKind::MinLength(1),
            ViolationKind::MaxLength(64),
            ViolationKind::MinValue("0".to_string()),
            ViolationKind::MaxValue("1440".to_string()),
            ViolationKind::EnumValue(vec!["NONE".to_string()]),
            ViolationKind::UniqueItems,
        ] {
            let violation = Violation {
                path: "name".to_string(),
                kind: other.clone(),
                value: Some("x".to_string()),
                sensitive: false,
            };
            let stated = as_stated(&violation);
            assert_eq!(
                stated, violation,
                "case {other:?}: expected the framework's own violation, got {stated:?}"
            );
        }
    }

    /// RULE: a framework-rendered error body reports its detail text in the `Message`
    /// member this service's error shapes declare — and ONLY a body whose class the model
    /// declares, so the scaffold's 501 signal and any other framework class keep the
    /// member the framework wrote it in. Nothing else about the body changes, and the
    /// rename is idempotent.
    #[test]
    fn an_error_bodys_detail_text_moves_to_the_modelled_member() {
        // (case, the body a renderer wrote, the body this service answers — `None` for a
        // body there is nothing to rename in, which is returned untouched)
        let rows: [(&str, &str, Option<&str>); 9] = [
            (
                "the pre-handler refusal",
                r#"{"__type":"BadRequestException","message":"1 validation error detected"}"#,
                Some(r#"{"__type":"BadRequestException","Message":"1 validation error detected"}"#),
            ),
            (
                // The `__type` an error shape renders under may be namespaced.
                "a namespaced modelled error",
                r#"{"__type":"com.amazonaws.appconfig#ConflictException","message":"in flight"}"#,
                Some(
                    r#"{"__type":"com.amazonaws.appconfig#ConflictException","Message":"in flight"}"#,
                ),
            ),
            (
                "the scaffold's 501 signal, which is not a modelled error",
                r#"{"__type":"NotImplemented","message":"operation X is scaffolded"}"#,
                None,
            ),
            (
                // The class the framework names for a refusal this service does not word
                // itself, and which its model does not declare either.
                "a framework error class this model does not declare",
                r#"{"__type":"ValidationException","message":"not a modelled class"}"#,
                None,
            ),
            (
                "a body with no error class at all",
                r#"{"message":"no __type"}"#,
                None,
            ),
            (
                "a modelled error, already in the modelled member",
                r#"{"__type":"ResourceNotFoundException","Message":"no such thing"}"#,
                None,
            ),
            (
                // Both members present: the modelled one wins and the rendered one goes,
                // so one body never states two texts.
                "both members present",
                r#"{"__type":"BadRequestException","Message":"modelled","message":"rendered"}"#,
                Some(r#"{"__type":"BadRequestException","Message":"modelled"}"#),
            ),
            (
                "another member of the error shape, which is left alone",
                r#"{"__type":"BadRequestException","Reason":"InvalidConfiguration","message":"no"}"#,
                Some(
                    r#"{"__type":"BadRequestException","Reason":"InvalidConfiguration","Message":"no"}"#,
                ),
            ),
            ("a body that is not a JSON object", "not json at all", None),
        ];
        for (case, rendered, expected) in rows {
            let answered = with_detail_member(rendered.as_bytes());
            match expected {
                Some(expected) => {
                    let answered = answered.unwrap_or_else(|| {
                        panic!("case {case:?}: expected {expected:?}, got no rewrite")
                    });
                    let answered: serde_json::Value = serde_json::from_slice(&answered)
                        .unwrap_or_else(|error| {
                            panic!("case {case:?}: the answer must be JSON, got {error}")
                        });
                    let expected: serde_json::Value =
                        serde_json::from_str(expected).expect("the expected body is JSON");
                    assert_eq!(
                        answered, expected,
                        "case {case:?}: expected {expected:?}, got {answered:?}"
                    );
                    // Idempotent: answering the same body twice changes nothing.
                    let again = with_detail_member(
                        serde_json::to_vec(&answered)
                            .expect("the answer serializes")
                            .as_slice(),
                    );
                    assert!(
                        again.is_none(),
                        "case {case:?}: the rename must be idempotent, got {again:?}"
                    );
                }
                None => assert!(
                    answered.is_none(),
                    "case {case:?}: expected the body untouched, got {:?}",
                    answered.as_deref().map(String::from_utf8_lossy)
                ),
            }
        }
    }

    /// This service's Smithy model, as it ships beside this crate. Read at test time
    /// rather than embedded: it is JSON that no non-test build needs.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-appconfig-amazonappconfig.json"
    );

    /// How many operations this service's model declares, as a second, independent
    /// statement of its surface: a model that grows or shrinks fails here rather than
    /// quietly leaving the new operation out of the wrapper.
    const MODELLED_OPERATIONS: usize = 56;

    /// RULE applied to the whole surface: every operation the model declares is served
    /// through [`InServiceDialect`], under the same key the generated routing table is
    /// keyed by — so a constraint refusal quotes the submitted value, and an error's
    /// detail text lands in the modelled member, for EVERY operation rather than for the
    /// ones someone remembered.
    ///
    /// The names come from the MODEL rather than from a second hand-written list, so an
    /// operation added to the model and not to `wrap_operations!` fails here instead of
    /// silently answering in the framework's wording — and a key spelled differently from
    /// the generated table's fails here instead of silently never being reached by
    /// `override_operation`.
    #[test]
    fn every_modelled_operation_is_served_in_this_services_dialect() {
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        let model: serde_json::Value =
            serde_json::from_str(&model).expect("the model that generated this crate is JSON");
        let shapes = model["shapes"]
            .as_object()
            .expect("a Smithy model declares shapes");
        let operations: Vec<&str> = shapes
            .iter()
            .filter(|(_, shape)| shape["type"] == "operation")
            .filter_map(|(id, _)| id.rsplit('#').next())
            .collect();
        assert_eq!(
            operations.len(),
            MODELLED_OPERATIONS,
            "case the model's surface: expected this service's {MODELLED_OPERATIONS} \
             operations, got {}",
            operations.len()
        );

        let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));
        for operation in operations {
            assert!(
                service.entry.routing_table().get(operation).is_some(),
                "case {operation}: the generated table keys an operation by its shape \
                 name; this one resolved to nothing, so the wrapper's key spelling is wrong"
            );
            assert!(
                service.in_service_dialect.get(operation).is_some(),
                "case {operation}: every modelled operation is served through \
                 InServiceDialect; this one is missing from wrap_operations!, so a \
                 constraint refusal of it quotes a shortened value and its detail text \
                 lands in the framework's own member"
            );
        }
    }
}
