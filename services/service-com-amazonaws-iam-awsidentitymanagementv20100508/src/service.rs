// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-iam-awsidentitymanagementv20100508` service: how it claims requests and how it words the
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
//!
//! ## Credentials the account has deactivated
//!
//! An access key this account holds authenticates a request only while its stored
//! `Status` is `Active`: `UpdateAccessKey$Status` documents that "Active means that the
//! key can be used for programmatic calls to Amazon Web Services, while Inactive means
//! that the key cannot be used", so deactivating a key must cut the credential off
//! while leaving it listed. That is an AUTHENTICATION outcome — no principal is
//! resolved at all — so it is refused BEFORE any operation runs, and with the code an
//! authentication failure carries ([`UNUSABLE_CREDENTIAL_CODE`]) rather than with an
//! authorization denial or one of the modelled resource errors.
//!
//! The decision reads the records `iam:CreateAccessKey` and `iam:UpdateAccessKey` wrote
//! ([`crate::state::ComAmazonawsIamAwsidentitymanagementv20100508State::access_key_is_usable`]);
//! this file only reaches it ahead of dispatch, through [`Service::override_operation`]
//! — the one seam that sees a whole request before the modelled walk deserializes,
//! validates and dispatches it.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::{envelope, ProtocolError, Violation, ViolationKind};
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, OperationContext, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The first `/`-separated segment of `path`, or `None` for a path with no non-empty
/// first segment (`"/"`, `""`).
fn first_path_segment(path: &str) -> Option<&str> {
    let segment = path.trim_start_matches('/').split('/').next()?;
    (!segment.is_empty()).then_some(segment)
}

/// Whether `parts` addresses a path inside `path_segments` — the first URI path segments
/// this service's wire protocol puts its operations under.
///
/// This mirrors the framework's own discriminator,
/// `emulator_server::can_handle_based_on_route_key_and_path`, which the generated
/// `can_handle` entry point uses. The framework helper takes a whole buffered
/// `http::Request`, which the `Service::can_handle` seam below does not receive, so the
/// rule is applied here over `Parts` instead of called. It is kept identical, including
/// both edge cases: an EMPTY list means no usable discriminator and is "no constraint",
/// and the framework's own `ROOT_PATH_SEGMENT` sentinel is how a service that answers
/// `POST /` claims the root path without claiming every path. Matching is EXACT segment
/// equality, deliberately never a prefix test.
fn claims_modeled_path(parts: &http::request::Parts, path_segments: &[&str]) -> bool {
    if path_segments.is_empty() {
        return true;
    }
    match first_path_segment(parts.uri.path()) {
        Some(segment) => path_segments.contains(&segment),
        None => path_segments.contains(&emulator_server::ROOT_PATH_SEGMENT),
    }
}

/// The URI path space IAM's wire protocol uses: the ROOT path, and nothing else.
///
/// IAM speaks `awsQuery` (`smithy_query(query="service_info")`: protocols `["awsQuery"]`)
/// and no operation in its model carries a `smithy.api#http` binding, so every one of its
/// 180 operations arrives as `POST /` with the operation named by the `Action` form field.
/// The generated `crate::ROUTE_PATH_SEGMENTS` is EMPTY for exactly that case — the
/// generator keeps it empty for an RPC service with no bindings so as not to narrow
/// services that already work — and an empty list means "claims every path", which is
/// what made IAM collide with its signing-name peer. The root claim is therefore stated
/// here, using the framework's own sentinel for it. It is deliberately NOT called
/// `ROUTE_PATH_SEGMENTS`: that name belongs to the generated crate-level constant, which
/// is empty.
const WIRE_PATH_SEGMENTS: &[&str] = &[emulator_server::ROOT_PATH_SEGMENT];

/// The error code the query protocols name a refusal of an invalid request value with
/// (`vendor/smithy-server-runtime/src/pre_dispatch.rs`), and the code the framework itself
/// would have answered this refusal with. It is restated here because [`Refused`] replaces
/// the code and the message TOGETHER and only the message changes below: a client branches
/// on the code, so it must stay the one it would have seen.
const VALUE_REFUSAL_CODE: &str = "ValidationError";

/// Whether `violation` is a value outside a BOUND the model declares for its member — a
/// number outside its `@range`, or a string or collection outside its `@length`.
///
/// The other classes the framework reports are not bounds: a string outside its
/// `@pattern`, a value outside an enumerated set, a repeated element, an absent required
/// member.
fn is_declared_bound(violation: &Violation) -> bool {
    matches!(
        violation.kind,
        ViolationKind::MinValue(_)
            | ViolationKind::MaxValue(_)
            | ViolationKind::MinLength(_)
            | ViolationKind::MaxLength(_)
    )
}

/// The admissible value sets this service states in an order of its OWN, each written in
/// the order IAM publishes for that enum.
///
/// A refusal of a value outside an enumerated set states the set, and the order it is
/// stated in belongs to the service: for some of IAM's enums it is not the order the model
/// happens to declare the values in. Where the two differ the published order is carried
/// here and replaces the renderer's; a set this table does not name keeps the model's
/// order, which is what the framework already states.
///
/// SOURCE, for adding an enum: the order AWS itself states the set in — the `Valid Values`
/// listing for that member in the IAM API Reference, or the bracketed set in IAM's own
/// refusal of a value outside it. That is AWS data about the enum, so an entry is added
/// only when such a statement is in hand and it disagrees with the model's declaration
/// order; an enum nobody has that for is simply absent from this table, which is why the
/// table is short rather than one entry per enum.
///
/// An entry is matched by its MEMBERSHIP — the set of values, in any order — so it answers
/// for the enum itself rather than for one member referencing it, and every member
/// targeting that enum, in any operation, is stated the same way.
const PUBLISHED_ENUM_ORDERS: &[&[&str]] = &[
    // The scope of a managed-policy listing (`iam#policyScopeType`, declared by the model
    // as `All, AWS, Local`).
    &["All", "Local", "AWS"],
];

/// The order this service states the admissible set `allowed` in, or `None` when the
/// model's own order stands.
///
/// The match is on membership and size, never on position, so a model that reordered its
/// own declaration of the same enum would still be stated in the published order.
fn published_enum_order(allowed: &[String]) -> Option<&'static [&'static str]> {
    PUBLISHED_ENUM_ORDERS.iter().copied().find(|published| {
        published.len() == allowed.len()
            && published
                .iter()
                .all(|value| allowed.iter().any(|declared| declared == value))
    })
}

/// A boundary refusal restated so a value this service refuses WITHOUT echoing it names
/// the member and the requirement and nothing of the value, or `None` when the request
/// broke no such requirement and the framework's own wording stands.
///
/// Two classes are restated, and both read `Value at '<member>' failed to satisfy
/// constraint: <requirement>`:
///
/// - a value outside a BOUND its member declares ([`is_declared_bound`]), whose
///   requirement is the bound itself;
/// - a value outside an enumerated set, whose requirement states the admissible set —
///   rendered in the order this service publishes for that enum
///   ([`published_enum_order`]) rather than the model's declaration order.
///
/// The other classes (a string outside its `@pattern`, an absent required member) keep the
/// framework's sentence, value and all. Only the `Value '<the value>' at` opening is
/// lifted out, and only the bracketed set is reordered: the envelope, the member path and
/// the requirement are the framework's, built from the same fields the renderer
/// interpolated, so the two cannot drift and the replacement matches the echo whatever the
/// value is. The ONE thing it couples to is the renderer's wording of that opening and of
/// the set (`vendor/smithy-schema-server/src/constraints.rs::Violation::message`, `Value
/// '<shown>' at '<path>'` and `Member must satisfy enum value set: [<values>]`); the
/// suites assert the whole message and that the value appears nowhere in it, so a renderer
/// that worded either differently would fail there rather than turn this into a silent
/// no-op.
///
/// Each rule covers every member of its class, not one member: the bound, the set and the
/// member path all come from the model, so a page size, a session duration, a
/// description's length and a tag list's size are one class and are worded alike
/// (`tests/all/range_refusal_wording.rs`), and a listing's scope, a usage filter, an
/// access key's status and an entity filter are another
/// (`tests/all/enum_refusal_wording.rs`), read back over the operations and members that
/// carry them.
///
/// A refusal carrying an ABSENT required member is declined whole: that one renders under
/// a different code than a failed value, and restating it under [`VALUE_REFUSAL_CODE`]
/// would change what a client branches on.
fn without_refused_value_echo(violations: &[Violation]) -> Option<Refused> {
    let absent_member = violations
        .iter()
        .any(|violation| matches!(violation.kind, ViolationKind::MissingRequiredMember));
    let restated = |violation: &Violation| {
        is_declared_bound(violation) || matches!(violation.kind, ViolationKind::EnumValue(_))
    };
    if absent_member || !violations.iter().any(restated) {
        return None;
    }
    let mut message = envelope(violations)?;
    for violation in violations.iter().filter(|violation| restated(violation)) {
        if let Some(value) = violation.value.as_deref() {
            message = message.replace(
                &format!("Value '{value}' at '{}'", violation.path),
                &format!("Value at '{}'", violation.path),
            );
        }
        if let ViolationKind::EnumValue(allowed) = &violation.kind {
            if let Some(published) = published_enum_order(allowed) {
                message = message.replace(
                    &format!("enum value set: [{}]", allowed.join(", ")),
                    &format!("enum value set: [{}]", published.join(", ")),
                );
            }
        }
    }
    Some(Refused {
        code: VALUE_REFUSAL_CODE.to_string(),
        message,
    })
}

/// The error code AWS answers a signed request whose credential it will not accept
/// with: a key id that was never issued, one that has been deleted, and one whose
/// `Status` the account has moved off `Active`. It is an AUTHENTICATION failure — no
/// principal was resolved — rather than an authorization denial, so it is the code an
/// SDK's credential-refresh path keys on, the same code this emulator's STS front door
/// answers an unresolvable credential with.
const UNUSABLE_CREDENTIAL_CODE: &str = "InvalidClientTokenId";

/// How AWS words that refusal: a fixed sentence carrying nothing from the request, so a
/// caller presenting a credential the service will not accept learns nothing about the
/// credential or about this account.
const UNUSABLE_CREDENTIAL_MESSAGE: &str = "The security token included in the request is invalid.";

/// Answers a request that presented a credential this account will not accept, as a
/// pre-dispatch operation.
///
/// Rendered through the service's own refusal channel — the same path the framework's
/// authentication refusal takes — so the envelope, the status (403) and the `<Code>` a
/// client branches on are the ones every other refusal from this service carries.
struct UnusableCredential;

impl ErasedOperation for UnusableCredential {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        // The refusal is reached from the credential alone: nothing of the request's
        // body or metadata is read, and nothing is written.
        let _ = (req, ctx);
        Box::pin(async move {
            svc.definition().refusal_response(
                ProtocolError::Unauthenticated {
                    code: UNUSABLE_CREDENTIAL_CODE,
                    message: UNUSABLE_CREDENTIAL_MESSAGE,
                },
                None,
            )
        })
    }
}

/// The access key id `req` presented, or `None` for a request carrying no SigV4
/// `Authorization` header at all.
///
/// Read through the framework's ONE parser of that header
/// (`emulator_server::extract_akid`, which every handler in this crate resolves a
/// nameless caller through). That parser takes an `http::HeaderMap`, which this
/// pre-dispatch seam is not handed — it sees the runtime-API request — so the one
/// header it reads is lifted into a map rather than the parse being written again here.
fn presented_access_key_id(req: &RtRequest) -> Option<String> {
    let authorization = req.headers().get(http::header::AUTHORIZATION.as_str())?;
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        http::HeaderValue::from_str(authorization).ok()?,
    );
    Some(::engine::extract_akid(&headers))
}

/// The `com-amazonaws-iam-awsidentitymanagementv20100508` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The SAME shared engine the generated entry threaded into every operation, kept
    /// here too because the pre-dispatch credential check below has to read the access
    /// keys this service's own operations wrote. Never a store of its own: it is the
    /// `Arc` the router handed to `new`.
    engine: Arc<Eng>,
    /// Answers a request presenting a credential this account will not accept.
    unusable_credential: UnusableCredential,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            engine,
            unusable_credential: UnusableCredential,
        }
    }

    /// Whether `req` presented an access key this account holds and will not accept —
    /// one whose stored `Status` is not `Active` (see the module docs).
    ///
    /// A request that names no modelled operation is left alone: there is nothing to
    /// serve it with, and the routing failure is the answer it gets. A store this
    /// process cannot reach holds no key status either, so such a request is admitted
    /// and the handler reports the unreachable store as the modelled server error.
    ///
    /// A READ: it takes the store's lock to look one key up and writes nothing, so a
    /// refused request changes nothing a later caller sees.
    fn presents_an_unusable_credential(&self, req: &RtRequest) -> bool {
        if self.definition().protocol().routing_key(req).is_err() {
            return false;
        }
        let Some(access_key_id) = presented_access_key_id(req) else {
            return false;
        };
        crate::state::service_state(&*self.engine)
            .is_some_and(|state| !state.lock().access_key_is_usable(&access_key_id))
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
        // This service signs as `iam` — a signing name it SHARES with IAM Toolbox
        // (`services/service-com-amazonaws-iamtoolbox-authrequestservice`, sigv4Name
        // `iam`, endpointPrefix `iam-toolbox`). The router's routing key comes from the
        // SigV4 credential scope first, so both services resolve to `iam` and matching on
        // the signing name ALONE makes both claim every request — which the router
        // answers `500 AmbiguousService` for, in a JSON body an awsQuery client cannot
        // even parse.
        //
        // So the claim is narrowed the way the framework narrows the other signing-name
        // collisions: the signing name must match AND the request must address this
        // service's own URI path space, which for an awsQuery service is the root path
        // (see `WIRE_PATH_SEGMENTS`). IAM Toolbox's `/authorization-details/…` is then
        // claimed by it alone.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) if key == self.entry.identity().signing_name() => {
                claims_modeled_path(parts, WIRE_PATH_SEGMENTS)
            }
            _ => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        //
        // A credential the account has deactivated is refused HERE, before the
        // operation it named runs (see the module docs): the modelled walk
        // deserializes, validates and dispatches before any handler could answer, so
        // this pre-dispatch seam is the only place an authentication outcome is
        // reachable from.
        //
        // The request's auth schemes are unaffected: the router reads them off the
        // MODELLED operation the routing key names, so this changes nothing about which
        // requests must be signed.
        self.presents_an_unusable_credential(req)
            .then_some(&self.unusable_credential as &dyn ErasedOperation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound or an enumerated
        // set) to let this service reword it. Returning `None` keeps the
        // framework's default wording; the classes this service words differently
        // are a value outside a bound its member declares and a value outside an
        // enumerated set, both of which it states without echoing the value (see
        // [`without_refused_value_echo`], which answers with the code the framework
        // would have chosen).
        match refusal {
            Refusal::Invalid(violations) => without_refused_value_echo(violations),
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
