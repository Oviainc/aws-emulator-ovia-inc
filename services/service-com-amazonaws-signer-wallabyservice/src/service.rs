// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-signer-wallabyservice` service: how it claims requests and how it words the
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
//!   before any handler ran. This service rewords ONE class — an input member
//!   the model binds a LIST to the query string, for which the request supplied
//!   nothing — and keeps the framework's wording for every other refusal. See
//!   the inline note on that method.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! `override_operation` is also where this service states the LENGTH of an answer
//! that carries no body: the operations whose modelled output is `Unit` are served
//! wrapped in [`StatedEmptyLength`], at the one place every response of theirs
//! passes through rather than once per operation. See [`EMPTY_BODY_LENGTH`].
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::constraints::{Violation, ViolationKind};
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refusal,
    Refused, Service,
};
use std::sync::Arc;

/// The length an answer with no body states.
///
/// A response that carries no body still has a length, and the recorded AWS
/// answer to a Signer call whose output carries none states it: a client (or an
/// intermediary) that frames a bodyless response by its stated length rather
/// than by the status alone has nothing to read otherwise. The value is a fact
/// about the body — it has no bytes — so it is stated for every such answer
/// rather than per operation.
///
/// It is the LAST word only up to the transport: the HTTP/1.1 writer this
/// emulator is served by removes `content-length` from a `204` (RFC 9110
/// §15.3.5 forbids it there), and that writer is a dependency of the vendored
/// HTTP layer rather than anything this crate decides — so a caller on the
/// in-process front door (the inter-service callback path and the SDK
/// connector, which both serve `dispatch`'s response unchanged) reads it, while
/// one on a socket reads the status and the empty body alone.
const EMPTY_BODY_LENGTH: &str = "0";

/// The response header that states it.
const CONTENT_LENGTH: &str = "content-length";

/// The operations whose modelled output carries NO body, and whose answers
/// therefore state [`EMPTY_BODY_LENGTH`].
///
/// Source: the three operations this model binds to `smithy.api#Unit`
/// (`smithy_query(query="digest", model_path="services/\
/// service-com-amazonaws-signer-wallabyservice/smithy-model/\
/// com-amazonaws-signer-wallabyservice.json")` — `CancelSigningProfile`,
/// `RevokeSignature` and `RevokeSigningProfile` are the ones whose `output` is
/// `Unit`). Every other operation of this service answers a modelled structure,
/// so its body is a document and its length is the document's.
///
/// The wrapper decides on the ANSWER rather than on the operation, so a response
/// of one of these that does carry a body (a refusal's error document) keeps the
/// length its renderer stated.
macro_rules! frame_empty_answers {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                ::std::boxed::Box::new(StatedEmptyLength::new(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                )) as ::std::boxed::Box<dyn ErasedOperation>,
            );
        )+
    };
}

/// One modelled operation, served with the length its empty answer has stated on
/// it.
///
/// Everything else about the operation is the operation's: the request walk, the
/// status, the body and every other header come from the wrapped
/// `impl Operation`, and the auth schemes are the wrapped operation's own —
/// wrapping must not change which schemes sign a request.
struct StatedEmptyLength<O> {
    operation: O,
}

impl<O> StatedEmptyLength<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for StatedEmptyLength<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move { with_stated_length(self.operation.run(svc, req, ctx).await) })
    }

    /// The wrapped operation's modelled auth schemes.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// `resp` with the length of an EMPTY body stated on it, and anything else left
/// exactly as the operation answered it.
///
/// A body with bytes in it states the length its renderer stated, a header
/// already present is left alone (whoever wrote it first is its one writer), and
/// a streamed body — whose length is not known here — is left to the transport.
/// The insert is FALLIBLE: a header the HTTP layer cannot represent is dropped
/// rather than turned into a transport failure.
fn with_stated_length(mut resp: RtResponse) -> RtResponse {
    let empty_body = resp.body().bytes().is_some_and(<[u8]>::is_empty);
    if empty_body && resp.headers().get(CONTENT_LENGTH).is_none() {
        let _ = resp
            .headers_mut()
            .try_insert(CONTENT_LENGTH, EMPTY_BODY_LENGTH);
    }
    resp
}

/// The operation `GET /revocations` addresses, as the generated routing table keys
/// it (the operation's own name).
const GET_REVOCATION_STATUS: &str = "GetRevocationStatus";

/// The method that route is bound to.
const REVOCATIONS_METHOD: &str = "GET";

/// Its path, as the single path segment it consists of. Compared the way the router
/// compares it: `route_lookup` trims the path's leading and trailing `/` and splits
/// the rest into segments, so `/revocations`, `/revocations/` and `//revocations//`
/// are all that one segment and anything else is another route.
const REVOCATIONS_PATH_SEGMENT: &str = "revocations";

/// Whether `req` addresses the `GET /revocations` route, whatever query parameters
/// it carries.
///
/// Parsed with `http::Uri` rather than by hand, so an absolute-form URI, an
/// origin-form one, and the userinfo/port/fragment variants all yield the same path
/// the router reads. A URI that does not parse addresses no route.
fn addresses_revocation_status(req: &aws_smithy_runtime_api::http::Request) -> bool {
    if req.method() != REVOCATIONS_METHOD {
        return false;
    }
    let Ok(uri) = req.uri().parse::<http::Uri>() else {
        return false;
    };
    uri.path().trim_matches('/') == REVOCATIONS_PATH_SEGMENT
}

/// The error code every refusal of a request's own values carries on this
/// service's 400: the modelled shape's own name (see
/// [`crate::state::ERROR_CODE_VALIDATION`]), which is also the code the
/// framework's own refusal renderer picks — so rewording a refusal's message
/// leaves the code a client branches on exactly where it was.
const VALIDATION_EXCEPTION_CODE: &str = "ValidationException";

/// The input schemas of the operations that bind a LIST input member to the
/// query string — the only two this model has, `GetRevocationStatus`'s
/// `certificateHashes` and `UntagResource`'s `tagKeys`
/// (`smithy_query(query="operation",operation="GetRevocationStatus,UntagResource",\
/// model_path="services/service-com-amazonaws-signer-wallabyservice/smithy-model/\
/// com-amazonaws-signer-wallabyservice.json")`).
///
/// The member NAMES are deliberately not written out here: the schemas are, and
/// [`is_query_bound_list_member`] reads the binding and the list-ness off them,
/// so the model stays the one source of which members the rule covers.
const QUERY_BOUND_LIST_INPUTS: &[&::aws_smithy_schema::Schema] = &[
    crate::ops::types::GET_REVOCATION_STATUS_INPUT_SCHEMA,
    crate::ops::types::UNTAG_RESOURCE_INPUT_SCHEMA,
];

/// Whether `member_name` names an input member this model binds a LIST to on the
/// query string, read off the generated input schemas in
/// [`QUERY_BOUND_LIST_INPUTS`].
fn is_query_bound_list_member(member_name: &str) -> bool {
    QUERY_BOUND_LIST_INPUTS.iter().any(|input| {
        input.members().iter().any(|member| {
            member.http_query().map(|query| query.value()) == Some(member_name)
                && matches!(member.shape_type(), ::aws_smithy_schema::ShapeType::List)
        })
    })
}

/// How this service states a required query-bound LIST member the request
/// supplied nothing for, as a sentence naming the member.
///
/// A list is the case the framework's generic wording gets wrong: on the query
/// string a list that is ABSENT and one that is EMPTY are the same request, so
/// the refusal cannot claim the value was null. What it states instead is that
/// the member must carry something — the one sentence for both forms of the same
/// request. Shared with the handler
/// ([`crate::ops::get_revocation_status`]), so a member the boundary validator
/// finds empty and one a handler finds empty are stated identically.
pub fn missing_list_member_constraint(member_name: &str) -> String {
    format!("{member_name} must not be null or empty")
}

/// How this service states a refusal whose every violation is a query-bound LIST
/// member the request supplied nothing for: one clause per member, in the order
/// the violations arrive (the model's member order), each naming the member.
///
/// `None` for every other refusal, which keeps the framework's own wording: a
/// single-value member that is absent really was null, and a value present but
/// outside a declared bound is already described accurately, with the offending
/// value echoed.
fn missing_list_member_refusal(violations: &[Violation]) -> Option<Refused> {
    if violations.is_empty() {
        return None;
    }
    let mut clauses = Vec::with_capacity(violations.len());
    for violation in violations {
        if violation.kind != ViolationKind::MissingRequiredMember
            || !is_query_bound_list_member(&violation.path)
        {
            return None;
        }
        clauses.push(missing_list_member_constraint(&violation.path));
    }
    Some(Refused {
        code: VALIDATION_EXCEPTION_CODE.to_string(),
        message: clauses.join("; "),
    })
}

/// The `com-amazonaws-signer-wallabyservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The operations whose modelled output carries no body, each wrapped in
    /// [`StatedEmptyLength`] so their empty answer states its length. Keyed by
    /// the routing key the protocol resolves — the key the generated table uses
    /// — and built from the SAME shared engine, so a wrapped operation reads and
    /// writes the one store.
    empty_answers: OperationRoutingTable,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut empty_answers = OperationRoutingTable::new();
        frame_empty_answers!(
            empty_answers,
            engine,
            cancel_signing_profile::CancelSigningProfile,
            revoke_signature::RevokeSignature,
            revoke_signing_profile::RevokeSigningProfile,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            empty_answers,
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
        // AWS Signer and the Signer data plane are two emulated services behind ONE
        // SigV4 signing name (`signer`), which the model cannot express. The router
        // resolves a request's routing key from the credential scope FIRST and falls
        // back to the `Host` label, so the key names an ENDPOINT: `signer` for this
        // service (whose endpoint prefix IS the shared signing name, and which every
        // signed request to either endpoint therefore resolves to) and `data-signer`
        // for the peer's own endpoint, which only a request addressed at that host
        // without a credential scope resolves to.
        //
        // So this service claims the signing name's traffic WHOLE, its own
        // `GetRevocationStatus` (`GET /revocations`) included: it holds the records
        // that answer it — `RevokeSigningProfile` and `RevokeSignature` write them —
        // and a request signed for `signer` is one this service serves whichever of
        // the two endpoint hosts it was sent to. The peer
        // (`services/service-com-amazonaws-signerdata-signerdataplane`) claims the key
        // of its OWN endpoint and no longer the shared signing name, so the two claims
        // are disjoint by key and no request has the two claimants the router answers
        // `500 AmbiguousService` for.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // `GetRevocationStatus` binds `GET /revocations` with ALL FIVE of its input
        // members `@required` + `@httpQuery`
        // (`smithy_query(query="operation",operation="GetRevocationStatus")`), so the
        // REST router's required-binding check matches that route only for a request
        // that carries every one of those query parameters. A request leaving one out
        // resolves to NO operation, and the protocol default for that is 501
        // `NotImplemented` — which says nobody implemented the operation, when what
        // actually happened is that the request left a required parameter out.
        //
        // Naming the operation here hands such a request to the operation it
        // addressed, so the centralized walk's own boundary validator refuses it with
        // the modelled 400 naming each absent member. A request that carries them all
        // resolves to this same operation through the routing table, so nothing else
        // changes.
        if addresses_revocation_status(req) {
            return self.entry.routing_table().get(GET_REVOCATION_STATUS);
        }

        // NOT a re-route: the key is the one the generated table's own protocol
        // resolves, and the operation it names is the one served — wrapped in
        // `StatedEmptyLength` so an answer with no body states its length. A key
        // no wrapped operation holds resolves to `None` here, so every other
        // request is served by the generated table unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.empty_answers.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // ONE class is reworded here, and only for a member the model binds a
        // LIST to the query string: a request that supplied nothing for it. The
        // framework states that as "Value null at '<member>' failed to satisfy
        // constraint: Member must not be null", which asserts the value was NULL
        // — a claim the wire cannot support for a query-bound list, because a
        // list that is ABSENT and one that is EMPTY are the same request (an SDK
        // serializing an empty list emits no parameter at all). This service
        // states instead that such a member must carry something, naming it (see
        // [`missing_list_member_constraint`]), and leaves every other refusal —
        // a single-value member that really was absent, a value present but
        // outside a declared bound — to the framework's own wording, which is
        // accurate for those.
        match refusal {
            Refusal::Invalid(violations) => missing_list_member_refusal(violations),
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
    use super::{
        is_query_bound_list_member, missing_list_member_constraint, missing_list_member_refusal,
        Violation, ViolationKind, VALIDATION_EXCEPTION_CODE,
    };

    /// One violation of `kind` against `path`, carrying `value`.
    fn violation(path: &str, kind: ViolationKind, value: Option<&str>) -> Violation {
        Violation {
            path: path.to_string(),
            kind,
            value: value.map(str::to_string),
        }
    }

    /// Which members the rule covers is read off the model: BOTH query-bound list
    /// members are covered, and no single-value member is — whatever its own
    /// shape or operation.
    #[test]
    fn only_query_bound_list_members_are_covered() {
        // Covered: the two members the model binds a list to on the query string.
        // Not covered: a query-bound string, a query-bound timestamp, a path
        // label, and a member no operation has.
        let cases: &[(&str, bool)] = &[
            ("certificateHashes", true),
            ("tagKeys", true),
            ("platformId", false),
            ("signatureTimestamp", false),
            ("profileVersionArn", false),
            ("resourceArn", false),
            ("tags", false),
            ("notAMember", false),
        ];
        for (member, covered) in cases {
            assert_eq!(
                is_query_bound_list_member(member),
                *covered,
                "case {member:?}: expected covered={covered}, got {}",
                is_query_bound_list_member(member)
            );
        }
    }

    /// A refusal whose every violation is a query-bound list member the request
    /// supplied nothing for is stated this service's way, naming each member.
    #[test]
    fn a_supplied_nothing_list_member_is_stated_by_name() {
        for member in ["certificateHashes", "tagKeys"] {
            let refused = missing_list_member_refusal(&[violation(
                member,
                ViolationKind::MissingRequiredMember,
                None,
            )])
            .unwrap_or_else(|| panic!("case {member:?}: expected a reworded refusal, got None"));
            assert_eq!(
                refused.message,
                missing_list_member_constraint(member),
                "case {member:?}: expected the member named, got {:?}",
                refused.message
            );
            assert_eq!(
                refused.code, VALIDATION_EXCEPTION_CODE,
                "case {member:?}: expected the code left where it was, got {:?}",
                refused.code
            );
        }
    }

    /// Two such members are stated in one message, in the order the violations
    /// arrive.
    #[test]
    fn two_supplied_nothing_list_members_are_both_stated() {
        let refused = missing_list_member_refusal(&[
            violation("certificateHashes", ViolationKind::MissingRequiredMember, None),
            violation("tagKeys", ViolationKind::MissingRequiredMember, None),
        ])
        .expect("case \"two list members\": expected a reworded refusal, got None");
        assert_eq!(
            refused.message, "certificateHashes must not be null or empty; tagKeys must not be null or empty",
            "case \"two list members\": expected both members named in order, got {:?}",
            refused.message
        );
    }

    /// Every other refusal keeps the framework's own wording: a single-value
    /// member that really was absent, a value present but outside a declared
    /// bound, a mixture of the two, and the empty violation list.
    #[test]
    fn every_other_refusal_keeps_the_framework_wording() {
        let cases: &[(&str, Vec<Violation>)] = &[
            (
                "an absent single-value member",
                vec![violation(
                    "platformId",
                    ViolationKind::MissingRequiredMember,
                    None,
                )],
            ),
            (
                "a list member present but too short",
                vec![violation("tagKeys", ViolationKind::MinLength(1), Some("[]"))],
            ),
            (
                "a value outside a declared bound",
                vec![violation(
                    "jobArn",
                    ViolationKind::MinLength(20),
                    Some("arn:aws:signer"),
                )],
            ),
            (
                "an absent list member beside an absent single-value member",
                vec![
                    violation("certificateHashes", ViolationKind::MissingRequiredMember, None),
                    violation("jobArn", ViolationKind::MissingRequiredMember, None),
                ],
            ),
            ("no violations at all", Vec::new()),
        ];
        for (case, violations) in cases {
            let refused = missing_list_member_refusal(violations);
            assert!(
                refused.is_none(),
                "case {case:?}: expected the framework's wording kept, got {refused:?}"
            );
        }
    }
}
