// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-codeartifact-codeartifactcontrolplaneservice` service: how it claims requests and how it words the
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
//! Two things this service needs that the model does not express live in modules beside
//! this one:
//!
//! - [`npm_registry`] — the npm REGISTRY a repository's npm endpoint serves. The URL
//!   `GetRepositoryEndpoint` hands out names the DOMAIN's own host and speaks a package
//!   manager's protocol, not this API's, so the front door could resolve no service for it
//!   and answered `404 UnknownService` for the one address this service told the client to
//!   use. [`can_handle`](Service::can_handle) also claims a request addressed to a
//!   repository endpoint this service holds, and
//!   [`override_operation`](Service::override_operation) resolves it to the one operation
//!   that serves it ([`npm_registry::NpmRegistryEndpoint`]), out of the same package and
//!   asset records the control plane writes.
//! - [`pypi_registry`] — the same for a repository's `pypi` endpoint, which speaks the PyPI
//!   upload API and the Simple Repository API (PEP 503 and PEP 691) instead
//!   ([`pypi_registry::PypiRegistryEndpoint`]).
//! - [`repository_data_plane`] — what those two planes share: how a request addresses a
//!   repository endpoint, which credential authenticates it, what the repository's own
//!   policy says about it, and how an answer is rendered.
//! - [`npm_manifest`] — the one reading of an npm metadata document, which is where every
//!   member this service reports off one comes from.
//! - [`npm_readme`] — the one reading of the README an npm publish carried, which is what
//!   `GetPackageVersionReadme` reports for a version published over the npm endpoint.
//! - [`authorization`] — whether IAM lets the caller perform a CodeArtifact action on a
//!   repository, the decision every control-plane operation addressing one takes before it
//!   answers.
//! - [`token_authorization`] — whether IAM lets the caller mint a repository bearer
//!   credential, which `GetAuthorizationToken` takes before anything is minted.
//! - [`wire`] — the media type every successful answer of this service declares, which the
//!   generic `restJson1` serializer types off the body's own target shape instead.

pub mod authorization;

pub mod npm_manifest;

pub mod npm_readme;

pub mod npm_registry;

pub mod pypi_registry;

pub mod repository_data_plane;

pub mod token_authorization;

pub mod wire;

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The error code this service answers an input-constraint violation with: its own
/// modelled `ValidationException`, which
/// `smithy_query(query="service_info")` lists among this service's common errors with
/// `@httpError 400`. It is also the framework's default code for a refusal, restated here
/// because `override_refusal` replaces the code and the message together.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// A `@pattern` expression without the anchors a model spells it with.
///
/// A model writes `^…$` to demand that the WHOLE value match; the refusal states the
/// expression a value has to satisfy, which is the expression itself. A `$` preceded by
/// an odd number of backslashes is an escaped literal dollar inside the expression, not
/// the end anchor, so it stays.
///
/// NOTE: this is one of the ~25 inlined copies of this rule the tree carries
/// (`grep -rln 'fn unanchored' services/*/src`). `services/common/src/validation.rs`
/// ("Still to migrate here") is its declared home, and says the migration is a change of
/// its own rather than a side effect of one service's issue — so this copy is pinned by
/// THIS service's tests (`tests/all/error_message_composition.rs`) and is to be deleted,
/// not copied, when that migration lands.
fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(trimmed) => {
            let backslashes = trimmed.len() - trimmed.trim_end_matches('\\').len();
            if backslashes % 2 == 0 {
                trimmed
            } else {
                body
            }
        }
        None => body,
    }
}

/// The clause one violation contributes to a `ValidationException` message.
///
/// Every violation class but `@pattern` is stated in the framework's own words
/// ([`Violation::message`]) — this service words them identically. A `@pattern` failure
/// is restated with [`unanchored`] applied to the expression, so the regular expression
/// quoted back to the caller is one it can compile and compare against the member's
/// documented constraint.
fn constraint_clause(violation: &Violation) -> String {
    let ViolationKind::Pattern(pattern) = &violation.kind else {
        return violation.message();
    };
    let requirement = format!(
        "Member must satisfy regular expression pattern: {}",
        unanchored(pattern)
    );
    match &violation.value {
        Some(shown) => format!(
            "Value '{shown}' at '{}' failed to satisfy constraint: {requirement}",
            violation.path
        ),
        // An absent member has no value to echo; it cannot fail a pattern either, but the
        // subject is spelled the way the framework spells it rather than left to chance.
        None => format!(
            "Value null at '{}' failed to satisfy constraint: {requirement}",
            violation.path
        ),
    }
}

/// The `ValidationException` message for a refusal: the AWS envelope
/// (`N validation error(s) detected: `) over one clause per violation, joined with `; `.
///
/// `None` declines the refusal, which keeps the framework's own wording: a refusal with
/// no `@pattern` violation in it is already worded the way this service words it, so
/// there is nothing to replace.
fn constraint_envelope(violations: &[Violation]) -> Option<String> {
    if !violations
        .iter()
        .any(|violation| matches!(violation.kind, ViolationKind::Pattern(_)))
    {
        return None;
    }
    let clauses: Vec<String> = violations.iter().map(constraint_clause).collect();
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// The `com-amazonaws-codeartifact-codeartifactcontrolplaneservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The SAME shared engine the generated entry threads into every operation.
    ///
    /// Held here because a repository ENDPOINT's own traffic ([`npm_registry`]) is claimed
    /// by `can_handle`, which is not an operation with a `self.engine` of its own and must
    /// read the one store the control-plane operations write, not a second copy.
    engine: Arc<Eng>,
    /// The one operation a repository endpoint's own traffic is served as
    /// ([`npm_registry::NpmRegistryEndpoint`]), built from that same engine and resolved by
    /// [`override_operation`](Service::override_operation).
    npm_registry_endpoint: npm_registry::NpmRegistryEndpoint<Eng>,
    /// The same for a `pypi` endpoint's traffic
    /// ([`pypi_registry::PypiRegistryEndpoint`]), which speaks the PyPI protocols over the
    /// same records.
    pypi_registry_endpoint: pypi_registry::PypiRegistryEndpoint<Eng>,
    /// This service's wire protocol: the model's own `restJson1`, answering with the media
    /// type of [`wire`]. Built once here because
    /// [`override_protocol`](Service::override_protocol) hands back a borrow of it.
    protocol: wire::JsonMediaTypeRestJson,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            npm_registry_endpoint: npm_registry::NpmRegistryEndpoint::new(Arc::clone(&engine)),
            pypi_registry_endpoint: pypi_registry::PypiRegistryEndpoint::new(Arc::clone(&engine)),
            protocol: wire::JsonMediaTypeRestJson::new(),
            engine,
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
        // Claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) if key == self.entry.identity().signing_name() => true,
            // And a repository ENDPOINT's own traffic: a request addressed to a repository
            // endpoint this service holds a record for ([`npm_registry::claims_parts`],
            // [`pypi_registry::claims_parts`]). Such a request names no modelled operation
            // and carries no credential scope naming this service — its host is the
            // DOMAIN's published endpoint host, whose first label names no service — so the
            // front door could resolve no target service for it and answered
            // `404 UnknownService`, for the one address `GetRepositoryEndpoint` told the
            // client to use.
            //
            // The claim is narrowed to a repository this service HOLDS, read from the one
            // shared store, rather than to a host or a path shape alone, so one request
            // keeps one claimant; the two planes claim disjoint paths, because the format
            // is the first segment of the endpoint path each is published under.
            _ => {
                npm_registry::claims_parts(&*self.engine, parts)
                    || pypi_registry::claims_parts(&*self.engine, parts)
            }
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
        // A repository ENDPOINT's own traffic is exactly such a request: it addresses the
        // DOMAIN's published host and speaks npm's protocol, so no modelled operation's
        // route can name it. It is resolved to the one operation that serves it, which is
        // what gives that plane an operation's own callback into this emulator (the
        // package version state changes a publish or a retention commits are announced on
        // the account's default event bus) and its own auth schemes — the credential it
        // carries is the repository's bearer token, not a SigV4 signature.
        if npm_registry::claims_request(&*self.engine, req) {
            return Some(&self.npm_registry_endpoint);
        }
        if pypi_registry::claims_request(&*self.engine, req) {
            return Some(&self.pypi_registry_endpoint);
        }
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Returning `None` keeps the framework's default
        // wording. To state a refusal the way the emulated service states it,
        // replace this body with a `match refusal { .. }` that returns
        // `Some(Refused { code, message })` for the cases you reword and `None`
        // for the rest. Replace BOTH the code and the message: a client branches
        // on the code, so a message without its matching code is worse than
        // leaving both alone.
        //
        // This service restates a `@pattern` failure with the expression UNANCHORED —
        // the `^`/`$` a model writes to demand a whole-string match are not part of the
        // expression a value has to satisfy, and a caller that compiles or compares the
        // quoted pattern must see the one the member's constraint publishes. Every other
        // violation class keeps the framework's wording, and the code stays the modelled
        // `ValidationException` (HTTP 400) the framework would have answered with.
        //
        // An authentication refusal keeps the framework's wording: it is not an
        // input-constraint failure.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: constraint_envelope(violations)?,
            }),
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
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501).
        //
        // A repository endpoint's own traffic does not reach here: `override_operation`
        // resolves it to the operation that serves it, ahead of the generated routing
        // table and of authentication.
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

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // The model's own restJson1 codec, with the ONE difference the generic renderer
        // cannot express: a successful answer declares this service's own media type for
        // the whole response, so a package version's asset bytes are served the way every
        // other answer of this service is (see [`wire`]). Borrowed from the field built in
        // `new`, never constructed per request.
        //
        // This seam drives the MODELLED sequence only. A framework-level refusal still
        // renders in the generated entry's envelope, which is correct: that answer is a
        // JSON document already.
        Some(&self.protocol)
    }
}
