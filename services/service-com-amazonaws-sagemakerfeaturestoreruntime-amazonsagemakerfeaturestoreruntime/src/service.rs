// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime` service: how it claims requests and how it words the
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
//! - `override_operation` — EDITED HERE. The routing DECISION is still the model's: this
//!   service serves the operation the generated table names, through a wrapper that lets a
//!   refusal of a COLLECTION member state the collection the request carried, which the
//!   `override_refusal` seam cannot do because it never sees the request. See "How a
//!   clause on a LIST member states its value" below.
//! - `override_refusal` — EDITED HERE. A request refused for violating a modelled
//!   `@pattern` reports that pattern as the EXPRESSION a value has to satisfy, without
//!   the `^`/`$` anchors the model spells it with. Everything else about the refusal —
//!   the envelope, the clause, the echoed value, the error code — stays the
//!   framework's. See "How a `@pattern` is reported" below.
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
//! # How a `@pattern` is reported
//!
//! A constraint-violation message is BUILT FROM THE REQUEST —
//! `N validation error(s) detected: Value '<what was sent>' at '<member path>' failed to
//! satisfy constraint: <requirement>` — and for a `@pattern` the requirement AWS states
//! is the regular expression a value must satisfy, WITHOUT the leading `^` and trailing
//! `$` a model writes to turn Smithy's unanchored find into a whole-string match. That
//! anchoring is how the model demands the whole value, not part of the expression a
//! client shows a user or re-compiles to explain the rejection.
//!
//! So [`unanchored`] drops a whole-expression anchor pair from the REPORTED text only.
//! The MATCH is untouched: the framework still checks the fully anchored pattern the
//! model declares, so a value that would satisfy the expression only as a substring is
//! still refused. The rule holds for every pattern-constrained member this service
//! models — `FeatureGroupNameOrArn` (the feature group name of all seven operations,
//! whether it arrives as an `@httpLabel` or nested inside a batch entry), `FeatureName`,
//! and `ValueAsString`, whose `.*` carries no anchors and is therefore reported exactly
//! as declared — because the clause is rendered from the violation rather than from a
//! per-operation string.
//!
//! Everything else stays the framework's, deliberately: the envelope and its violation
//! count ([`aws_smithy_schema_server::constraints::envelope`]), the clause wording, the
//! echoed value, and the `ValidationException` code a client branches on. A refusal with
//! no `@pattern` violation in it is not reworded at all.
//!
//! This is the SAME rule ~24 other service crates in this tree carry inlined
//! (`grep -rln 'fn unanchored' services/*/src`); `services/common/src/validation.rs`
//! ("Still to migrate here") is where it belongs, and migrating all of them is one
//! change of its own rather than a side effect of this service's issue, so it is
//! inlined here with this note, exactly as that module asks.
//!
//! # How a clause on a LIST member states its value
//!
//! The same rule — a constraint message is BUILT FROM THE REQUEST — decides the VALUE
//! half of a clause about a collection member: the value such a clause rejects is the
//! collection itself, which AWS echoes rendered the way a list renders. The framework's
//! validating walk has no rendering for a collection and stores its own summary of one
//! (`<n> items`), which no caller ever sent, so that one clause is completed from the
//! request through the tree's shared echo walk
//! (`service_common::validation::refusing_violations_with_declared_echoes_over`) — see
//! [`QuotingSubmittedCollections`], which exists because [`Service::override_refusal`] is
//! handed the violations alone and never the request they were found in. Every other
//! clause keeps the framework's own rendering, which already states the bytes the caller
//! submitted.

use service_common::validation::SubmittedEcho;
use service_common::validation_message::quoting_the_submitted_collection;
use smithy_server_runtime::{
    http, ErasedOperation, Operation, OperationRoutingTable, Refusal, Refused, Service, Violation,
    ViolationKind,
};
use std::sync::Arc;

/// The wire error code for a request refused before any handler ran: the framework's
/// own code for a constraint violation, restated here because [`Refused`] replaces the
/// code and the message together. Every operation of this service also declares the
/// modelled `ValidationError` with `@httpError` 400, which a handler answers with; the
/// pre-handler refusal keeps the framework's `ValidationException`, which is what a
/// client sees in `x-amzn-errortype`.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// A `@pattern` expression without the anchors a model may spell it with.
///
/// Only a `^…$` PAIR bracketing the whole expression is dropped: `^` alone, or `$`
/// alone, is an expression anchored at one end — a different language, not an anchored
/// spelling of this one — and an anchor left INSIDE after the pair is removed means the
/// pair bracketed one branch of an alternation (`^a$|^b$`) rather than the whole
/// expression, so such a pattern is reported exactly as the model spells it. A trailing
/// `$` escaped by a backslash is a literal dollar sign, not the end anchor, so it stays.
fn unanchored(pattern: &str) -> &str {
    let Some(body) = pattern.strip_prefix('^') else {
        return pattern;
    };
    let Some(body) = without_end_anchor(body) else {
        return pattern;
    };
    if body.contains('^') || body.contains('$') {
        return pattern;
    }
    body
}

/// `pattern` without its end anchor, or `None` when it carries none. A trailing `$`
/// preceded by an ODD number of backslashes is escaped — a literal dollar sign inside
/// the expression — so it is not an anchor.
fn without_end_anchor(pattern: &str) -> Option<&str> {
    let body = pattern.strip_suffix('$')?;
    let escapes = body.len() - body.trim_end_matches('\\').len();
    (escapes % 2 == 0).then_some(body)
}

/// The same violation with a `@pattern` reported unanchored; every other class is
/// returned untouched, so the requirement each one words stays the framework's.
fn as_stated(violation: &Violation) -> Violation {
    let ViolationKind::Pattern(pattern) = &violation.kind else {
        return violation.clone();
    };
    Violation {
        kind: ViolationKind::Pattern(unanchored(pattern).to_string()),
        ..violation.clone()
    }
}

/// The member POSITIONS the echo-collecting walk may visit for one request of this
/// service, stated from this model rather than taken from the shared default (256).
///
/// A collection is quoted in a clause only when every one of its elements was walked, and
/// the largest collection of STRUCTURES this model declares is `BatchGetRecordIdentifiers`
/// (`@length` max 100 over `BatchGetRecordIdentifier`:
/// `smithy_query(query="shape", shape="BatchGetRecordIdentifiers")`), so the ceiling has
/// to cover the 101 elements that break that bound. Each element contributes a position
/// per leaf its three members bottom out in and one per composite it and its two list
/// members are — eight for an identifier asking for two records and two features — so 101
/// of them need a little over 800. Past this the framework's own count summary stands,
/// which is the fail-safe direction: a request whose identifiers each ask for a hundred
/// records (`RecordIdentifiers` is itself `@length` max 100) is not quoted at all rather
/// than quoted short.
const MAX_ECHO_POSITIONS: usize = 1024;

/// One modelled operation, served so that a refusal of a `@length` on one of its
/// COLLECTION members states the collection the caller submitted.
///
/// Why it exists: [`Service::override_refusal`] is handed the violations alone, never the
/// request they were found in, so the one clause whose value the framework could not
/// render cannot be completed there. Here the input can be re-read.
///
/// Nothing is re-implemented and no check is repeated in other words: the violations and
/// the echoes come from the tree's shared collecting walk
/// (`service_common::validation::refusing_violations_with_declared_echoes_over`, which
/// runs the protocol's own deserializer and the framework's own `constraints::validate`
/// and owns every bound on what one clause may quote), the restatement of a collection's
/// clause from `service_common::validation_message`, the envelope and every other clause
/// from the framework, and the status and the error body from the framework's refusal
/// channel. A request this wrapper does not refuse is handed to the generated operation,
/// which owns deserialize -> validate -> handle -> serialize and every modelled error.
/// The cost is DELIBERATE: an accepted request's body is read twice, here and in the
/// operation's own sequence, because the framework hands no already-built input to an
/// operation.
struct QuotingSubmittedCollections<Op> {
    operation: Op,
}

impl<Op> QuotingSubmittedCollections<Op> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: Op) -> Self {
        Self { operation }
    }
}

impl<Op> ErasedOperation for QuotingSubmittedCollections<Op>
where
    Op: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r smithy_server_runtime::OperationContext<'r>,
    ) -> smithy_server_runtime::BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            // The protocol is read, used and dropped inside this one statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await
            // below would make this future non-`Send`.
            let refused: Vec<Violation> =
                ::service_common::validation::refusing_violations_with_declared_echoes_over::<Op>(
                    svc.definition().protocol(),
                    req,
                    MAX_ECHO_POSITIONS,
                )
                .into_iter()
                .map(|(violation, echo)| stated_clause(violation, echo))
                .collect();
            if let Some(message) = aws_smithy_schema_server::constraints::envelope(&refused) {
                return svc.definition().refusal_response(
                    aws_smithy_schema_server::ProtocolError::Invalid(refused),
                    Some(Refused {
                        code: VALIDATION_EXCEPTION.to_string(),
                        message,
                    }),
                    req,
                );
            }
            self.operation.run(svc, req, ctx).await
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// One violation as this service states it: a `@pattern` requirement unanchored
/// ([`as_stated`]) and a COLLECTION member's `@length` clause quoting the collection the
/// request carried (`service_common::validation_message::quoting_the_submitted_collection`,
/// the tree's one home for that restoring rule). Every other clause is the framework's,
/// untouched.
fn stated_clause(violation: Violation, echo: SubmittedEcho) -> Violation {
    as_stated(&quoting_the_submitted_collection(violation, echo))
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`QuotingSubmittedCollections`] and keyed by its operation shape name — the routing key
/// `ServerHttpProtocol::routing_key` resolves and the generated table is keyed by.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(QuotingSubmittedCollections::new(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                )) as ::std::boxed::Box<dyn ErasedOperation>,
            );
        )+
    };
}

/// The `com-amazonaws-sagemakerfeaturestoreruntime-amazonsagemakerfeaturestoreruntime` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped in
    /// [`QuotingSubmittedCollections`]. Keyed by the routing key the protocol resolves —
    /// the key the generated table uses — and built from the SAME shared engine, so a
    /// wrapped operation reads and writes the one store. Held on the service because
    /// [`Service::override_operation`] hands back a borrow of `self`.
    quoting_submitted_collections: OperationRoutingTable,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut quoting_submitted_collections = OperationRoutingTable::new();
        wrap_operations!(
            quoting_submitted_collections,
            engine,
            batch_get_record::BatchGetRecord,
            batch_write_record::BatchWriteRecord,
            delete_record::DeleteRecord,
            get_record::GetRecord,
            list_records::ListRecords,
            put_record::PutRecord,
            update_record::UpdateRecord,
        );
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            quoting_submitted_collections,
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
        // This service signs as `sagemaker` — a signing name it SHARES with every other
        // SageMaker-family service, while serving its own endpoint
        // (`featurestore-runtime.sagemaker`). The router resolves the routing key from
        // the SigV4 credential scope first, so every one of them sees the key
        // `sagemaker`: claiming on the signing name ALONE claims the siblings' traffic
        // too, and the router — which refuses rather than guesses — answers
        // `500 AmbiguousService` for ALL of it.
        //
        // So the claim is narrowed to this service's own modelled wire surface, by the
        // one rule `service-common` states for every service under a shared signing
        // name. This model is `restJson1`: its surface is the URI path space its
        // operations bind (`/FeatureGroup/…`, `/BatchGetRecord`, `/BatchWriteRecord`),
        // which is what `claims_request` matches against the generated
        // `ROUTE_PATH_SEGMENTS` when a service declares no RPC protocol.
        service_common::claims_request(
            parts,
            self.entry.identity().signing_name(),
            crate::ROUTE_PATH_SEGMENTS,
            None,
        )
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Serve the operation the generated routing table names, through the wrapper that
        // lets a refusal of a collection member state the collection the request carried
        // (see [`QuotingSubmittedCollections`]). The routing DECISION is not overridden:
        // the key is resolved by the model's own protocol, and a request the table cannot
        // serve is declined here so the framework's routing-error path stays exactly as
        // it was.
        let operation = self.definition().protocol().routing_key(req).ok()?;
        self.definition().routing_table().get(operation)?;
        self.quoting_submitted_collections.get(operation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it. The ONE thing this service states differently is how a `@pattern`
        // requirement is rendered — the expression without the anchors the model spells
        // it with (see "How a `@pattern` is reported" in the module docs).
        let Refusal::Invalid(violations) = refusal else {
            // Authentication is the framework's to word: the request never reached an
            // operation, so there is nothing of this service's own to say about it.
            return None;
        };
        // Nothing else is worded differently, so a refusal carrying no `@pattern`
        // violation keeps the framework's own rendering rather than being re-rendered
        // here: the blast radius stays on the one rule above.
        if !violations
            .iter()
            .any(|violation| matches!(violation.kind, ViolationKind::Pattern(_)))
        {
            return None;
        }
        let stated: Vec<Violation> = violations.iter().map(as_stated).collect();
        Some(Refused {
            code: VALIDATION_EXCEPTION.to_string(),
            // The framework's own envelope and clause (`1 validation error detected:
            // Value '<sent>' at '<path>' failed to satisfy constraint: …`), rendered
            // from the restated violations, so the count and the wording stay in the
            // one place that owns them and only the pattern text differs.
            message: aws_smithy_schema_server::constraints::envelope(&stated)?,
        })
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
    use super::{as_stated, unanchored};
    use smithy_server_runtime::{Violation, ViolationKind};

    /// The anchoring a model spells a `@pattern` with is dropped only when the `^…$`
    /// pair brackets the WHOLE expression; every other form is the expression itself
    /// and is reported as the model writes it. The first three rows are this service's
    /// own modelled patterns (`FeatureGroupNameOrArn`, `FeatureName`, `ValueAsString`,
    /// from `smithy_query(query="shape")` on each); the rest are the forms the rule has
    /// to leave alone, which this model does not happen to contain.
    #[test]
    fn a_pattern_loses_only_a_whole_expression_anchor_pair() {
        let cases = [
            (
                "the feature group name or ARN",
                r"^(arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:feature-group/)?([a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63})$",
                r"(arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:feature-group/)?([a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63})",
            ),
            (
                "a feature name",
                "^[a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63}$",
                "[a-zA-Z0-9]([-_]*[a-zA-Z0-9]){0,63}",
            ),
            ("an unanchored expression", ".*", ".*"),
            ("anchored at the start only", "^arn:", "^arn:"),
            ("anchored at the end only", "[0-9]+$", "[0-9]+$"),
            ("no anchor at all", r"\d{3}", r"\d{3}"),
            ("a fully anchored alternation", "^a$|^b$", "^a$|^b$"),
            ("an escaped trailing dollar", r"^cost\$", r"^cost\$"),
            ("an escaped dollar before the anchor", r"^cost\\$", r"cost\\"),
            ("the empty expression", "", ""),
        ];
        for (case, modelled, expected) in cases {
            let actual = unanchored(modelled);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// Only the `@pattern` class is restated: the clause of every other constraint —
    /// and the member path and the echoed value of a pattern clause — stays exactly
    /// what the framework rendered.
    #[test]
    fn only_a_pattern_clause_is_restated() {
        let outside_pattern = Violation {
            path: "identifiers.1.member.featureGroupName".to_string(),
            kind: ViolationKind::Pattern("^[a-zA-Z0-9]+$".to_string()),
            value: Some("not a valid name!!".to_string()),
            sensitive: false,
        };
        assert_eq!(
            as_stated(&outside_pattern).message(),
            "Value 'not a valid name!!' at 'identifiers.1.member.featureGroupName' failed to \
             satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9]+",
            "the pattern loses its anchors; the path and the echoed value do not change"
        );
        let out_of_range = Violation {
            path: "maxResults".to_string(),
            kind: ViolationKind::MaxValue("100".to_string()),
            value: Some("1000".to_string()),
            sensitive: false,
        };
        assert_eq!(
            as_stated(&out_of_range),
            out_of_range,
            "a non-pattern violation is untouched"
        );
    }
}
