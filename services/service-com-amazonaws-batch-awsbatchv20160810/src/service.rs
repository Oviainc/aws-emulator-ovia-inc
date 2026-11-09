// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-batch-awsbatchv20160810` service: how it claims requests and how it words the
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
//! A sixth seam, `override_protocol`, chooses the protocol every modelled answer is
//! serialized in. This service takes it for one framing rule only — an answer that carries
//! NO CONTENT carries no body — which lives in [`no_content`].
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! ## Naming the request a response answers
//!
//! Every response AWS sends names the request it answered, in the header this protocol
//! reports that id in (`x-amzn-RequestId`), on a success exactly as on a failure. This file
//! wires that report at the layers a response of this service is rendered by:
//! [`request_id_header::RequestIdHeaderOperation`] for everything an operation answers —
//! which reports the id a Batch refusal STATES in its message rather than a second one —
//! and `override_routing_error` below for the answer to a request that named no modelled
//! operation. See [`request_id_header`] for the rule and for the shared module that owns it
//! for every emulated service.

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

pub mod no_content;
pub mod request_id_header;

use request_id_header::RequestIdHeaderOperation;

// The code a refusal of this service's TAG MAP carries is the tag frontend's, not Batch's
// own: one definition, in `crate::scaffolded`, which the handlers on the tagging endpoint
// answer through as well, so the boundary and the handlers cannot drift to two codes for
// one layer.
use crate::scaffolded::TAGGING_REFUSAL_CODE;

/// The error code AWS answers every OTHER refusal of a Batch request with.
///
/// This service's model declares exactly two errors on every operation —
/// `ClientException` (HTTP 400) and `ServerException` (HTTP 500) — so a client-side
/// refusal is a `ClientException`, whoever found it. The framework's synthetic
/// `ValidationException` is not in Batch's error set at all, so an SDK branching on the
/// code cannot match it.
const REFUSAL_CODE: &str = "ClientException";

/// The input member Batch's tag map arrives on, on every operation that takes one
/// (`TagResource$tags`, `CreateJobQueue$tags`, `RegisterJobDefinition$tags`, …).
const TAGS_MEMBER: &str = "tags";

/// Whether `violation` is about the tag map (the member itself, or a key or value inside
/// it), which is what makes the refusal Tagris's rather than Batch's.
fn is_tag_violation(violation: &Violation) -> bool {
    violation.path == TAGS_MEMBER || violation.path.starts_with(&format!("{TAGS_MEMBER}."))
}

/// Whether `violation` is the tag map carrying MORE entries than its declared maximum —
/// the one tag-map rule the tagging layer states in its own words
/// ([`crate::state::tag_set_limit_message`]) rather than as a constraint clause.
///
/// It is the MEMBER's own length that fires this, so a key or a value inside the map is not
/// one of these: those are bounds of the key and value shapes, which the layer reports as
/// the framework words them.
fn is_tag_count_violation(violation: &Violation) -> bool {
    violation.path == TAGS_MEMBER && matches!(violation.kind, ViolationKind::MaxLength(_))
}

/// The message Batch answers an ENUM-MEMBERSHIP refusal with, and `None` for a violation
/// of any other class.
///
/// A value outside the closed set a member's enum declares is a rule AWS Batch states
/// ITSELF: the frontend envelope over the service's own sentence for that rule
/// ([`crate::state::enum_membership_detail`]), naming the member and the set it enforced
/// and nothing of the value it rejected. Every enum member of this service is a plain
/// `String` in the generated input types, so the boundary validator is what finds the
/// value — but the words are the service's, which is why they are chosen here instead of
/// left to the framework's generic constraint rendering.
fn enum_membership_message(violation: &Violation) -> Option<String> {
    let ViolationKind::EnumValue(allowed) = &violation.kind else {
        return None;
    };
    Some(crate::state::client_exception_message(
        &crate::state::enum_membership_detail(&violation.path, allowed),
        &crate::state::new_request_id(),
    ))
}

/// The `com-amazonaws-batch-awsbatchv20160810` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// Serves every routable request through the generated routing table and then reports
    /// the request that answer belongs to; see [`request_id_header`]. Held, not built per
    /// request: `override_operation` hands back a borrow of it.
    request_id_header: RequestIdHeaderOperation,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            request_id_header: RequestIdHeaderOperation,
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

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // Every response a modelled operation of this service produces is serialized by the
        // protocol returned here: the model's own `restJson1`, with the one framing
        // correction [`no_content`] states — an answer at the no-content status carries no
        // body, where the serializer renders the empty JSON object for an output structure
        // that declares no member. The tag mutations are the operations that answer at that
        // status; every other answer of this service passes through untouched.
        Some(self)
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // Claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Every request the generated routing table can serve is served through the
        // header wrapper, which delegates to that same table entry and then reports the
        // request that answer belongs to in the header AWS carries it in (see
        // [`request_id_header`]). The routing stays the model's: a request whose target
        // names no modelled operation is declined here, so the framework answers it as
        // the unroutable request it is (and `override_routing_error` below names the
        // request on that answer).
        let routes = self
            .entry
            .protocol()
            .routing_key(req)
            .ok()
            .and_then(|key| self.entry.routing_table().get(key))
            .is_some();
        routes.then_some(&self.request_id_header as _)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) so this service can
        // answer with a code a Batch client actually branches on. The framework's
        // synthetic `ValidationException` is in no Batch operation's error set, so BOTH
        // the code and the message are replaced here:
        //
        // * a refusal of the tag map is Tagris's, HTTP 400 `BadRequestException` — the
        //   code recorded for a 51-entry `tags` map;
        // * every other refusal is the `ClientException` (HTTP 400) the model declares.
        //
        // The MESSAGE is the service's own wherever Batch states the rule itself: a
        // member outside the closed set its enum declares is answered with Batch's
        // frontend envelope over that rule's sentence ([`enum_membership_message`]),
        // because the framework's generic constraint rendering is not what this API
        // emits — it names a dotted member path and echoes the rejected value, neither of
        // which appears in a Batch refusal. Every other class keeps the message the
        // framework built from the request: it names the member and the constraint it
        // failed, which is what the caller has to fix. Authentication refusals keep the
        // framework's wording (403 is not a Batch error shape at all).
        match refusal {
            Refusal::Invalid(violations) => {
                if violations.iter().any(is_tag_violation) {
                    // A tag map longer than the maximum it declares is the ONE tag-plane
                    // rule the tagging layer words itself: it states the ceiling it holds a
                    // resource's resultant tag set to
                    // ([`crate::state::tag_set_limit_message`]) and not the entry count of
                    // the request, which is what the framework's constraint clause would
                    // report. Every other refusal of the map keeps that clause: it names
                    // the member and the bound it failed, which is what the caller fixes.
                    let message = if violations.iter().any(is_tag_count_violation) {
                        crate::state::tag_set_limit_message()
                    } else {
                        aws_smithy_schema_server::envelope(violations)
                            .unwrap_or_else(|| "invalid request".to_string())
                    };
                    return Some(Refused {
                        code: TAGGING_REFUSAL_CODE.to_string(),
                        message,
                    });
                }
                // A member outside its enum's value set is a rule Batch words itself, in
                // its own envelope. Only the FIRST violation is considered, so the order
                // the validating walk found the rules in is the order they are reported
                // in: a request that also breaks a rule the walk reports earlier is still
                // refused with that one.
                if let Some(message) = violations.first().and_then(enum_membership_message) {
                    return Some(Refused {
                        code: REFUSAL_CODE.to_string(),
                        message,
                    });
                }
                Some(Refused {
                    code: REFUSAL_CODE.to_string(),
                    message: aws_smithy_schema_server::envelope(violations)
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
        // routing table resolved it. The ENVELOPE stays the framework's
        // protocol-correct default (restJson1 -> 501); what is added is the one header
        // every response of this service names its request in, because an answer no
        // operation produced is still an answer a client correlates on (see
        // [`request_id_header`]).
        Some(request_id_header::with_request_id_header(
            self.entry.routing_error_response(req),
        ))
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
