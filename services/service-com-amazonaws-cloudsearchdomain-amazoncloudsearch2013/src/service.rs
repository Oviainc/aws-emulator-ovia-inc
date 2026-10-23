// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-cloudsearchdomain-amazoncloudsearch2013` service: how it claims requests and how it words the
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

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// Whether the request these `parts` came from resolves to `route_key` AND
/// addresses one of the URI path segments this model binds
/// (`crate::ROUTE_PATH_SEGMENTS`).
///
/// `emulator_server::can_handle_based_on_route_key_and_path` is the
/// agent-immutable owner of that rule and is CALLED here rather than
/// re-implemented: the request's route key must match the key passed AND its
/// first URI path segment must be one of the segments passed, by exact segment
/// equality.
///
/// The helper takes a whole `http::Request` while [`Service::can_handle`] is
/// handed only the request's parts, so the two things it reads — the URI and the
/// headers — are re-presented to it as a bodiless request. It reads neither the
/// body nor the method, so nothing it decides on is missing.
fn claims_own_key_and_path(parts: &http::request::Parts, route_key: &str) -> bool {
    let Ok(mut request) = http::Request::builder()
        .uri(parts.uri.clone())
        .body(bytes::Bytes::new())
    else {
        return false;
    };
    *request.headers_mut() = parts.headers.clone();
    emulator_server::can_handle_based_on_route_key_and_path(
        &request,
        route_key,
        crate::ROUTE_PATH_SEGMENTS,
    )
}

/// The `com-amazonaws-cloudsearchdomain-amazoncloudsearch2013` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
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
        // This service and the CloudSearch CONFIGURATION service share one SigV4
        // signing name (`cloudsearch`), so the signing name alone cannot tell
        // them apart and a router that matched on it would see two claimants and
        // answer 500 `AmbiguousService`. What separates them is the endpoint: a
        // domain endpoint serves only this service's three dated data-plane paths
        // (`/2013-01-01/search`, `/2013-01-01/suggest`,
        // `/2013-01-01/documents/batch`), while the configuration service is an
        // awsQuery service rooted at `/`.
        //
        // The rule is the framework's own — `emulator_server`'s
        // `can_handle_based_on_route_key_and_path`, the agent-immutable owner of
        // the "route key AND modelled path space" test — so it is CALLED rather
        // than restated here, exactly as the cloudwatchevents and IAM services
        // call it. (`service_common::claims_request` wraps this same helper; this
        // crate reaches the owner directly so the fix adds no dependency, and so
        // no `Cargo.lock` edit, which the merge gate's file-level squash cannot
        // land while mainline rewrites that file every few minutes.) This model
        // is `restJson1`, so its surface is the URI path space its operations
        // bind: the generated `ROUTE_PATH_SEGMENTS`, matched by exact
        // first-segment equality.
        claims_own_key_and_path(parts, self.entry.identity().signing_name())
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        let _ = req;
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // This service models no `ValidationException`: a bad value is a
        // `SearchException` on the two search operations and a
        // `DocumentServiceException` on `UploadDocuments`, both HTTP 400. The
        // framework refuses a modelled constraint (the `QueryParser` and
        // `ContentType` enums, an absent required member) before any handler
        // runs, so without this the same bad request would answer one error code
        // from the boundary and another from a handler. The violation names the
        // member it refused, which names the operation: the members of
        // `UploadDocumentsRequest` belong to `UploadDocuments`, every other
        // member to `Search` or `Suggest`.
        match refusal {
            // An empty violation list says nothing about which operation was
            // refused, and a message built from it would read "0 validation
            // errors detected: ". The framework's own wording is better than
            // that, so it is left to render the refusal.
            Refusal::Invalid(violations) if violations.is_empty() => None,
            Refusal::Invalid(violations) => {
                let document_service = violations.iter().any(|violation| {
                    let member = violation.path.split('.').next().unwrap_or_default();
                    UPLOAD_DOCUMENTS_MEMBERS.contains(&member)
                });
                let messages: Vec<String> = violations
                    .iter()
                    .map(smithy_server_runtime::Violation::message)
                    .collect();
                Some(Refused {
                    code: if document_service {
                        "DocumentServiceException".to_string()
                    } else {
                        "SearchException".to_string()
                    },
                    message: format!(
                        "{} validation error{} detected: {}",
                        messages.len(),
                        if messages.len() == 1 { "" } else { "s" },
                        messages.join("; ")
                    ),
                })
            }
            // Authentication is the emulator's own refusal, not this service's.
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

/// The members of `UploadDocumentsRequest`, as the Smithy model declares them
/// (`smithy_query(query="operation",operation="UploadDocuments")`: `documents`
/// and `contentType`).
///
/// A boundary refusal names the member it refused, and the member names the
/// operation, which is what lets [`EmulatedService::override_refusal`] choose
/// between the two modelled error codes. Listed as the WHOLE member set rather
/// than matched ad hoc, so a model that renames or adds a member is a one-line
/// change here with a test (`a_refused_request_answers_the_modelled_error_with_http_400`)
/// that fails loudly if the mapping stops holding.
const UPLOAD_DOCUMENTS_MEMBERS: [&str; 2] = ["documents", "contentType"];
