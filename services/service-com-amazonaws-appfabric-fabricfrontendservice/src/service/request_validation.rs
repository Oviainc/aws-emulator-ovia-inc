// SPDX-License-Identifier: CC0-1.0

//! Where this service states a REFUSED REQUEST: in the modelled
//! `ValidationException`, for every operation and whichever layer found the fault.
//!
//! AppFabric models `ValidationException` with `message`, `reason` (REQUIRED) and
//! `fieldList`, and the generated type makes `reason` non-optional, so every refusal a
//! handler of this crate builds carries it ([`crate::state::fields_not_valid`],
//! [`crate::state::precondition_not_met`]).
//!
//! A request the framework refuses BEFORE any handler runs — an absent `@required`
//! member, a value outside a declared `@range`, `@length` or `@pattern`, a body the
//! codec cannot read — reaches no handler. The framework words that refusal through
//! [`smithy_server_runtime::Service::override_refusal`], whose answer is a code and a
//! message, so the body it renders carries `message` alone: a client deserializing the
//! modelled shape finds no value for a member the model requires, and cannot branch on
//! `reason` or report the member at fault. Two refusals of ONE operation then answer in
//! two differently shaped bodies.
//!
//! So the refusal is stated HERE, in front of the operation, where the violations are
//! still in hand. The input is read and validated exactly as the generated walk reads
//! and validates it — through the operation's own `INPUT_SCHEMA` and `Input`, and
//! `aws_smithy_schema_server::constraints::validate` — and anything it faults is
//! answered in the modelled shape, through the same constructors the handlers use. A
//! request with nothing to fault is handed to the generated walk unchanged, which reads
//! the body once more; that second read is the price of answering a refusal in the
//! modelled shape, paid on a body the transport has already bounded.
//!
//! EVERY modelled operation of this service goes through it (see [`checks`]): the
//! framework's generic refusal body is reached through one seam shared by all 26, so
//! the restatement is single-homed here rather than repeated per operation, and a
//! refusal of any of them carries the same members.
//!
//! Because every operation is here, this is also where the request that reaches no
//! operation at all is resolved ([`RequestValidation::serving_key`]): the REST router
//! reads an operation's `@required` query/header bindings as route DISCRIMINATORS and
//! matches no route for a request that leaves one out, so a request whose method and
//! path address a route of this service — but which the router could not match — is
//! served by the operation the model binds to that route, and refused on its own values
//! here, rather than reported as an operation nobody implemented.
//!
//! Authentication is unaffected: the router verifies the request against the schemes of
//! the operation the model's routing key names before it runs anything, and this
//! wrapper declares no schemes of its own, so it cannot weaken them.

use std::collections::HashMap;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    BoxFuture, DeserializableStruct, ErasedOperation, ErasedServiceDefinition, Operation,
    OperationContext, Refusal, ServerHttpProtocol, Service, Violation,
};

use crate::ops::types::{ValidationException, ValidationExceptionField, SCHEMA_VALIDATION_EXCEPTION};

/// The HTTP status the model gives `ValidationException` (`@httpError(400)`) — the
/// status the framework's own refusal renderer answers with too, so restating the
/// refusal changes the body and nothing else.
const VALIDATION_STATUS: u16 = 400;

/// Why an operation refuses the request it was handed, in the two classes the modelled
/// `ValidationExceptionReason` draws for a request that reached this service.
enum InputRefusal {
    /// Values the service READ and does not accept: an absent `@required` member, or a
    /// value outside a constraint the model declares for it. Stated as
    /// `fieldValidationFailed`, with one `fieldList` entry per violation.
    Invalid(Vec<Violation>),
    /// A request the service could not READ at all — a member carrying a value of the
    /// wrong type, a body that is not the operation's input. Stated as `cannotParse`,
    /// with the codec's own explanation as the message.
    Unreadable(String),
}

impl InputRefusal {
    /// This refusal as the modelled `ValidationException`.
    ///
    /// `None` only where there is no violation to state a message from, which the
    /// construction of [`Self::Invalid`] already rules out; such a refusal is left to
    /// the framework rather than answered with an empty one.
    fn exception(&self) -> Option<ValidationException> {
        match self {
            Self::Invalid(violations) => {
                let fields = violations
                    .iter()
                    .map(|violation| ValidationExceptionField {
                        name: violation.path.clone(),
                        message: super::constraint_clause(violation),
                    })
                    .collect();
                Some(crate::state::fields_not_valid(
                    super::constraint_envelope(violations)?,
                    fields,
                ))
            }
            Self::Unreadable(detail) => Some(crate::state::cannot_parse(detail.clone())),
        }
    }
}

/// How ONE operation's input is read and faulted: the generic walk below, with that
/// operation's schema and input type baked in.
type InputCheck = fn(&dyn ServerHttpProtocol, &RtRequest) -> Option<InputRefusal>;

/// How `req` is refused as the input of `O`, read the way the framework's own walk
/// reads it: deserialize against the operation's schema, then validate the built value
/// against it.
///
/// `None` means this seam has nothing to restate — the input is valid, or the request
/// failed for a reason that is not its content (a body the transport could not hand
/// back), whose status and envelope the framework owns.
fn input_refusal<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<InputRefusal> {
    let schema = O::INPUT_SCHEMA;
    let error = match protocol.deserialize_input(req, schema) {
        Ok(mut deser) => match <O::Input as DeserializableStruct>::deserialize(&mut *deser) {
            // A built input: every modelled constraint, in one walk.
            Ok(input) => {
                let violations = aws_smithy_schema_server::constraints::validate(&input, schema);
                return (!violations.is_empty()).then_some(InputRefusal::Invalid(violations));
            }
            // Absence is found while READING the request, so it arrives as a codec
            // error; `refusal_of` below gives it the member-level form a constraint
            // violation has.
            Err(e) => ProtocolError::Serde(e),
        },
        Err(e) => e,
    };
    match smithy_server_runtime::refusal_of(&error) {
        Some(Refusal::Invalid(violations)) if !violations.is_empty() => {
            Some(InputRefusal::Invalid(violations))
        }
        // The rest of `ProtocolError::Serde`: a codec failure with no member-level
        // form. The service could not read the request, so it is refused as
        // `cannotParse` rather than handed on to the generated walk, which would read
        // the same body, fail the same way and render the body with no `reason`.
        _ => match error {
            ProtocolError::Serde(e) => Some(InputRefusal::Unreadable(e.to_string())),
            _ => None,
        },
    }
}

/// One operation of this service, as this module needs it: the check that reads and
/// faults ITS input, and the input schema that carries the `@http` route the model binds
/// it to.
struct Checked {
    /// That operation's own `Operation::INPUT_SCHEMA`, so the METHOD and the URI pattern
    /// of its route are read off the model rather than copied into a literal here.
    input: &'static Schema,
    /// How a request is read and faulted as that operation's input.
    refuse: InputCheck,
}

/// One table entry: the routing key the model names `O` by, and what this module needs
/// of `O`. Both halves are read off the operation rather than spelled out, so an entry
/// cannot name a key the routing table does not use, nor a route the model does not bind
/// to it.
fn check<O: Operation>() -> (&'static str, Checked) {
    (
        O::NAME,
        Checked {
            input: O::INPUT_SCHEMA,
            refuse: input_refusal::<O> as InputCheck,
        },
    )
}

/// Every modelled operation of this service, by the routing key the model names it by.
///
/// All 26 are here because all 26 share the seam this module exists for: an operation
/// left out would answer its boundary refusals without the `reason` the model requires.
fn checks<Eng: Send + Sync + 'static>() -> HashMap<&'static str, Checked> {
    HashMap::from([
        check::<crate::ops::batch_get_user_access_tasks::BatchGetUserAccessTasks<Eng>>(),
        check::<crate::ops::connect_app_authorization::ConnectAppAuthorization<Eng>>(),
        check::<crate::ops::create_app_authorization::CreateAppAuthorization<Eng>>(),
        check::<crate::ops::create_app_bundle::CreateAppBundle<Eng>>(),
        check::<crate::ops::create_ingestion::CreateIngestion<Eng>>(),
        check::<crate::ops::create_ingestion_destination::CreateIngestionDestination<Eng>>(),
        check::<crate::ops::delete_app_authorization::DeleteAppAuthorization<Eng>>(),
        check::<crate::ops::delete_app_bundle::DeleteAppBundle<Eng>>(),
        check::<crate::ops::delete_ingestion::DeleteIngestion<Eng>>(),
        check::<crate::ops::delete_ingestion_destination::DeleteIngestionDestination<Eng>>(),
        check::<crate::ops::get_app_authorization::GetAppAuthorization<Eng>>(),
        check::<crate::ops::get_app_bundle::GetAppBundle<Eng>>(),
        check::<crate::ops::get_ingestion::GetIngestion<Eng>>(),
        check::<crate::ops::get_ingestion_destination::GetIngestionDestination<Eng>>(),
        check::<crate::ops::list_app_authorizations::ListAppAuthorizations<Eng>>(),
        check::<crate::ops::list_app_bundles::ListAppBundles<Eng>>(),
        check::<crate::ops::list_ingestion_destinations::ListIngestionDestinations<Eng>>(),
        check::<crate::ops::list_ingestions::ListIngestions<Eng>>(),
        check::<crate::ops::list_tags_for_resource::ListTagsForResource<Eng>>(),
        check::<crate::ops::start_ingestion::StartIngestion<Eng>>(),
        check::<crate::ops::start_user_access_tasks::StartUserAccessTasks<Eng>>(),
        check::<crate::ops::stop_ingestion::StopIngestion<Eng>>(),
        check::<crate::ops::tag_resource::TagResource<Eng>>(),
        check::<crate::ops::untag_resource::UntagResource<Eng>>(),
        check::<crate::ops::update_app_authorization::UpdateAppAuthorization<Eng>>(),
        check::<crate::ops::update_ingestion_destination::UpdateIngestionDestination<Eng>>(),
    ])
}

/// Serves a modelled operation through the framework exactly as the routing table
/// would, after stating a refusal of its input in the modelled `ValidationException`.
///
/// It holds no operation of its own: the one it delegates to is the one the service's
/// routing table names for the request, resolved from the `svc` the framework passes
/// into [`ErasedOperation::run`]. So the framework owns the whole request walk and
/// every answer a request that IS valid gets; this wrapper owns only the shape of a
/// refusal of the request.
pub struct RequestValidation {
    checks: HashMap<&'static str, Checked>,
}

impl RequestValidation {
    /// The one value a service holds, built once in its constructor: the table is
    /// per-engine (an operation is generic over the engine it was constructed from), so
    /// it is built where the engine type is known rather than per request.
    pub fn new<Eng: Send + Sync + 'static>() -> Self {
        Self {
            checks: checks::<Eng>(),
        }
    }

    /// Whether one of this service's modelled operations serves `req` — the condition
    /// under which the service claims it for this wrapper, so a request that addresses
    /// no operation of this service at all keeps the framework's own routing refusal.
    pub fn serves(&self, definition: &dyn ErasedServiceDefinition, req: &RtRequest) -> bool {
        self.serving_key(definition, req).is_some()
    }

    /// The routing key of the operation that serves `req`: the one the model's own
    /// routing table names for it, or — for a request the REST router could match to no
    /// route — the one whose modelled route `req` ADDRESSES.
    ///
    /// The second case exists because the REST router rejects a candidate route whose
    /// `@required` header/query bindings are absent from the request
    /// (`required_binding_score`, `vendor/smithy-schema-server/src/rest_xml.rs`), which is
    /// how a model that overloads ONE method + URI template across several operations
    /// tells them apart by the shape of the request. That discrimination picks BETWEEN
    /// operations; it cannot un-name the only operation a route has. So for a model that
    /// binds a method + URI template to exactly one operation, a request leaving such a
    /// parameter out matches no route and would be answered as an operation nobody
    /// implemented, although the operation is implemented and the request is simply one
    /// its own values refuse. Naming the addressed operation here hands the request to it,
    /// so it is answered from the request's own values — through this module, so an absent
    /// member is stated in the modelled `ValidationException` exactly as an absent body or
    /// path member of the same operation already is.
    ///
    /// Which route a request addresses is read off each operation's OWN `@http` binding,
    /// through the shared reader (`service_common::addresses_modeled_http_route`), so no
    /// method, URI template or parameter name is written out here and an operation can
    /// only ever serve the route the model binds to it. A request addressing more than one
    /// operation's route is left to the framework: choosing between overloaded routes is
    /// the router's own job, and this model overloads none.
    fn serving_key<'r>(
        &self,
        definition: &dyn ErasedServiceDefinition,
        req: &'r RtRequest,
    ) -> Option<&'r str> {
        if let Ok(key) = definition.protocol().routing_key(req) {
            if definition.routing_table().get(key).is_some() {
                return Some(key);
            }
        }
        let addressed = self.route_addressed(req)?;
        definition
            .routing_table()
            .get(addressed)
            .is_some()
            .then_some(addressed)
    }

    /// The one operation of this service whose modelled route `req` addresses, or `None`
    /// when it addresses none of them — or more than one.
    fn route_addressed(&self, req: &RtRequest) -> Option<&'static str> {
        let mut addressing = self
            .checks
            .iter()
            .filter(|(_, checked)| service_common::addresses_modeled_http_route(req, checked.input))
            .map(|(operation, _)| *operation);
        let addressed = addressing.next()?;
        addressing.next().is_none().then_some(addressed)
    }
}

impl ErasedOperation for RequestValidation {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let definition = svc.definition();
            let protocol = definition.protocol();
            let Some(key) = self.serving_key(definition, req) else {
                // Unreachable through `Service::override_operation`, which claims a
                // request only when an operation serves it; answered as the framework
                // answers an unroutable request rather than guessed at.
                return definition.routing_error_response(req);
            };
            let refused = self
                .checks
                .get(key)
                .and_then(|checked| (checked.refuse)(protocol, req))
                .and_then(|refusal| refusal.exception());
            if let Some(exception) = refused {
                return protocol
                    .serialize_error(
                        &exception,
                        &SCHEMA_VALIDATION_EXCEPTION,
                        super::VALIDATION_EXCEPTION,
                        VALIDATION_STATUS,
                    )
                    .unwrap_or_else(|e| definition.framework_error_response(e, req));
            }
            // Nothing to restate: run the generated deserialize -> validate -> handle
            // -> serialize walk. Reached through the routing table rather than through
            // `override_operation`, which would route straight back here.
            match definition.routing_table().get(key) {
                Some(op) => op.run(svc, req, ctx).await,
                None => definition.routing_error_response(req),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names" — the
        // operation this wrapper delegates to, so its model-declared schemes decide and
        // this wrapper cannot weaken them.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every key the table holds is one this service's generated routing table serves.
    ///
    /// The keys are read off the operations themselves, so this is what pins the table
    /// to the service: a checked operation the routing table cannot serve would be one
    /// this wrapper validates a request for and then has nothing to delegate to.
    #[test]
    fn every_checked_operation_is_one_the_routing_table_serves() {
        // The engine is a type parameter and nothing in the table reads it (each
        // operation holds its own), so the unit type stands in for one here.
        let entry = crate::ServiceDefinition::<()>::new(::std::sync::Arc::new(()));
        for key in checks::<()>().keys() {
            assert!(
                entry.routing_table().get(key).is_some(),
                "case {key:?}: the routing table must serve every checked operation"
            );
        }
    }

    /// No two operations of this model bind the same method + URI template.
    ///
    /// What [`RequestValidation::serving_key`] rests on: where exactly one operation is
    /// bound to a route, a request the router matched to no route addresses that one
    /// operation and nothing else, so handing it there cannot take a request off an
    /// operation the router would have chosen. A model that did overload a route would
    /// make this fail here rather than let the fallback pick between the two (it picks
    /// neither — it declines, and the framework answers).
    #[test]
    fn every_checked_operation_binds_its_own_method_and_uri_template() {
        let mut bound: Vec<(String, &'static str)> = Vec::new();
        for (operation, checked) in &checks::<()>() {
            let Some(http) = checked.input.http() else {
                continue;
            };
            let route = format!("{} {}", http.method(), http.uri());
            if let Some((_, other)) = bound.iter().find(|(seen, _)| *seen == route) {
                panic!("case {route:?}: bound to both {other:?} and {operation:?}");
            }
            bound.push((route, operation));
        }
        assert_eq!(
            bound.len(),
            checks::<()>().len(),
            "case {bound:?}: every operation of this model binds a route"
        );
    }
}
