// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-securityagent-securityagent` service: how it claims requests and how it words the
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
//!   before any handler ran. EDITED HERE: everything about such a refusal stays the
//!   framework's — the envelope, each clause, the member paths and every requirement
//!   sentence — except the ORDER an enum's value set is listed in, which this service
//!   does not share with the public model for the enums `PUBLISHED_VALUE_SETS` names
//!   (see `published_order_refusal`).
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

use aws_smithy_schema_server::{envelope, Violation, ViolationKind};
use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The `com-amazonaws-securityagent-securityagent` emulated service: the generated [`ServiceDefinition`] plus
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
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        let _ = req;
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Everything about the refusal is the framework's —
        // the envelope, the member paths and every requirement sentence — except
        // the ORDER an enum's value set is listed in, which this service does
        // not share with the public model; see [`published_order_refusal`].
        published_order_refusal(refusal)
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

/// The order this service lists the `Provider` value set in when it refuses a value
/// outside that enum.
///
/// The public model declares the enum as `[GITHUB, GITLAB, BITBUCKET, CONFLUENCE,
/// AZURE_DEVOPS, JIRA]` (`smithy_query(query="shape", shape="Provider")`) and the
/// framework lists a value set in the order the model declares it. The recorded refusal of
/// a `provider` outside the enum lists the same six values in the order below instead, so
/// the order is a property of the ENUM SHAPE — the order the service's own model declares
/// its values in — rather than of any one request, and is listed here as such.
const PROVIDER_VALUE_SET: [&str; 6] = [
    "GITHUB",
    "AZURE_DEVOPS",
    "JIRA",
    "BITBUCKET",
    "CONFLUENCE",
    "GITLAB",
];

/// The order this service lists the `ArtifactType` value set in when it refuses a value
/// outside that enum.
///
/// Same property as [`PROVIDER_VALUE_SET`], for a second enum of this service: the public
/// model declares it as `[TXT, PNG, JPEG, MD, PDF, DOCX, DOC, JSON, YAML]`
/// (`smithy_query(query="shape", shape="ArtifactType")`), while the recorded refusal of an
/// `artifactType` outside the enum lists the same nine values in the order below.
const ARTIFACT_TYPE_VALUE_SET: [&str; 9] = [
    "DOCX", "YAML", "JSON", "JPEG", "PNG", "TXT", "PDF", "MD", "DOC",
];

/// Every enum of this service whose value set it publishes in an order of its own.
///
/// A value set NOT listed here is stated in the order the public model preserves and is
/// passed through untouched, so the one order each entry gives holds for every member
/// targeting that enum, on every operation.
const PUBLISHED_VALUE_SETS: [&[&str]; 2] = [&PROVIDER_VALUE_SET, &ARTIFACT_TYPE_VALUE_SET];

/// The error code the framework's own default answers a constraint refusal with on this
/// service's JSON protocol, named here because a chosen [`Refused`] replaces the code and
/// the message TOGETHER: a client branches on the code, so a reworded message has to
/// travel with the code it would otherwise have carried.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// This service's wording for `refusal` when it lists a value set of an enum whose order
/// this service publishes ([`PUBLISHED_VALUE_SETS`]), and `None` — the framework's own
/// wording, unchanged — for every other refusal.
///
/// Only the ORDER of such a value set is this service's: the envelope, each clause, the
/// member paths and every requirement sentence are the framework's own, reached through
/// the same [`Violation`] the walk produced, so a constraint class cannot arrive here with
/// wording of its own.
fn published_order_refusal(refusal: &Refusal) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    if !violations
        .iter()
        .any(|violation| published_order(&violation.kind).is_some())
    {
        return None;
    }
    let restated: Vec<Violation> = violations.iter().map(as_stated).collect();
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_string(),
        message: envelope(&restated)?,
    })
}

/// The order this service publishes for `kind`'s value set, or `None` when `kind` is not a
/// value set this service states an order of its own for.
///
/// A runtime [`Violation`] carries the SET of permitted values and not the shape they came
/// from, so an enum is recognised by its own value set — which is why any member targeting
/// it, on any operation, is stated in the one order.
fn published_order(kind: &ViolationKind) -> Option<&'static [&'static str]> {
    let ViolationKind::EnumValue(values) = kind else {
        return None;
    };
    PUBLISHED_VALUE_SETS
        .into_iter()
        .find(|published| is_set_of(values, published))
}

/// One violation as this service states it: a published value set in the order above, and
/// every other violation exactly as the framework found it.
fn as_stated(violation: &Violation) -> Violation {
    let Some(published) = published_order(&violation.kind) else {
        return violation.clone();
    };
    Violation {
        path: violation.path.clone(),
        kind: ViolationKind::EnumValue(
            published.iter().map(|value| (*value).to_string()).collect(),
        ),
        value: violation.value.clone(),
        sensitive: violation.sensitive,
    }
}

/// Whether `values` holds exactly the values `expected` does, in any order.
fn is_set_of(values: &[String], expected: &[&str]) -> bool {
    values.len() == expected.len()
        && expected
            .iter()
            .all(|wanted| values.iter().any(|value| value == wanted))
}

#[cfg(test)]
mod tests {
    //! The one rendering this file owns, over a set of value sets rather than the single
    //! recorded one. The end-to-end wording is pinned on the wire in
    //! `tests/all/error_message_wording.rs`.

    use super::*;

    /// One `EnumValue` violation over `values` at `path`, as the validating walk reports
    /// one, for the value `rejected`.
    fn enum_violation_at(path: &str, rejected: &str, values: &[&str]) -> Violation {
        Violation {
            path: path.to_string(),
            kind: ViolationKind::EnumValue(
                values.iter().map(|value| (*value).to_string()).collect(),
            ),
            value: Some(rejected.to_string()),
            sensitive: false,
        }
    }

    /// The same over the `provider` member, the refusal every row of the whole-envelope
    /// case below is built from.
    fn enum_violation(values: &[&str]) -> Violation {
        enum_violation_at("provider", "NOT_A_PROVIDER", values)
    }

    /// RULE: a value set this service publishes an order for is stated in that order
    /// whatever order it arrives in, and no other value set is reordered.
    #[test]
    fn only_a_published_value_set_is_restated_in_the_services_order() {
        // (case, the set as it arrives, the set as it is stated, whether this service
        // states the refusal at all)
        let rows: [(&str, &[&str], &[&str], bool); 9] = [
            (
                "Provider, in the public model's declaration order",
                &["GITHUB", "GITLAB", "BITBUCKET", "CONFLUENCE", "AZURE_DEVOPS", "JIRA"],
                &PROVIDER_VALUE_SET,
                true,
            ),
            (
                "Provider, already in the service's order",
                &PROVIDER_VALUE_SET,
                &PROVIDER_VALUE_SET,
                true,
            ),
            (
                "Provider, in some other order",
                &["JIRA", "CONFLUENCE", "GITHUB", "GITLAB", "BITBUCKET", "AZURE_DEVOPS"],
                &PROVIDER_VALUE_SET,
                true,
            ),
            (
                "ArtifactType, in the public model's declaration order",
                &["TXT", "PNG", "JPEG", "MD", "PDF", "DOCX", "DOC", "JSON", "YAML"],
                &ARTIFACT_TYPE_VALUE_SET,
                true,
            ),
            (
                "ArtifactType, already in the service's order",
                &ARTIFACT_TYPE_VALUE_SET,
                &ARTIFACT_TYPE_VALUE_SET,
                true,
            ),
            (
                "ArtifactType, sorted",
                &["DOC", "DOCX", "JPEG", "JSON", "MD", "PDF", "PNG", "TXT", "YAML"],
                &ARTIFACT_TYPE_VALUE_SET,
                true,
            ),
            // Source: `smithy_query(query="shape", shape="PrivateConnectionType")`.
            (
                "another enum of this service",
                &["SELF_MANAGED", "SERVICE_MANAGED"],
                &["SELF_MANAGED", "SERVICE_MANAGED"],
                false,
            ),
            (
                "a set holding only some of Provider's values",
                &["GITHUB", "GITLAB"],
                &["GITHUB", "GITLAB"],
                false,
            ),
            (
                "a set holding only some of ArtifactType's values",
                &["TXT", "PNG", "JPEG", "MD", "PDF", "DOCX", "DOC", "JSON"],
                &["TXT", "PNG", "JPEG", "MD", "PDF", "DOCX", "DOC", "JSON"],
                false,
            ),
        ];

        for (case, arrived, expected, restated) in rows {
            let found = enum_violation_at("member", "NOT_A_MEMBER", arrived);
            let stated = published_order_refusal(&Refusal::Invalid(vec![found.clone()]));
            assert_eq!(
                stated.is_some(),
                restated,
                "case {case:?}: expected restated={restated}, got {stated:?}"
            );
            let listed = match &as_stated(&found).kind {
                ViolationKind::EnumValue(values) => values.clone(),
                other => panic!("case {case:?}: expected an enum value set, got {other:?}"),
            };
            assert_eq!(
                listed, expected,
                "case {case:?}: expected the set {expected:?}, got {listed:?}"
            );
        }
    }

    /// RULE: every published order is a PERMUTATION of the value set it restates — it adds
    /// no value and drops none — so only the order of a refusal's set is this service's.
    #[test]
    fn a_published_order_holds_exactly_the_values_the_model_declares() {
        // (case, the enum as the public model declares it, the order this service states)
        // Source: `smithy_query(query="shape", shape="Provider,ArtifactType")`.
        let rows: [(&str, &[&str], &[&str]); 2] = [
            (
                "Provider",
                &["GITHUB", "GITLAB", "BITBUCKET", "CONFLUENCE", "AZURE_DEVOPS", "JIRA"],
                &PROVIDER_VALUE_SET,
            ),
            (
                "ArtifactType",
                &["TXT", "PNG", "JPEG", "MD", "PDF", "DOCX", "DOC", "JSON", "YAML"],
                &ARTIFACT_TYPE_VALUE_SET,
            ),
        ];

        for (case, declared, published) in rows {
            let mut sorted_declared: Vec<&str> = declared.to_vec();
            sorted_declared.sort_unstable();
            let mut sorted_published: Vec<&str> = published.to_vec();
            sorted_published.sort_unstable();
            assert_eq!(
                sorted_published, sorted_declared,
                "case {case:?}: expected the declared values {sorted_declared:?}, got \
                 {sorted_published:?}"
            );
            assert!(
                PUBLISHED_VALUE_SETS.contains(&published),
                "case {case:?}: expected {published:?} to be a published order, got \
                 {PUBLISHED_VALUE_SETS:?}"
            );
            // An enum is recognised by its value set alone, so two published sets holding
            // the same values would make the order ambiguous: each must name a different
            // set.
            let sharing = PUBLISHED_VALUE_SETS
                .into_iter()
                .filter(|other| {
                    is_set_of(
                        &other.iter().map(|value| (*value).to_string()).collect::<Vec<_>>(),
                        published,
                    )
                })
                .count();
            assert_eq!(
                sharing, 1,
                "case {case:?}: expected exactly one published order over {published:?}, \
                 got {sharing}"
            );
        }
    }

    /// RULE: a refusal this service does not restate keeps the framework's wording, so an
    /// unauthenticated caller and a refusal naming no published value set are left alone.
    #[test]
    fn a_refusal_without_a_published_value_set_keeps_the_frameworks_wording() {
        // (case, the refusal the framework made)
        let rows: [(&str, Refusal); 4] = [
            ("an unauthenticated caller", Refusal::Unauthenticated),
            ("nothing to state", Refusal::Invalid(Vec::new())),
            (
                "an absent required member",
                Refusal::Invalid(vec![Violation {
                    path: "provider".to_string(),
                    kind: ViolationKind::MissingRequiredMember,
                    value: None,
                    sensitive: false,
                }]),
            ),
            (
                "a length bound",
                Refusal::Invalid(vec![Violation {
                    path: "targetUrl".to_string(),
                    kind: ViolationKind::MaxLength(2048),
                    value: Some("x".to_string()),
                    sensitive: false,
                }]),
            ),
        ];

        for (case, refusal) in rows {
            let stated = published_order_refusal(&refusal);
            assert!(
                stated.is_none(),
                "case {case:?}: expected the framework's own wording, got {stated:?}"
            );
        }
    }

    /// RULE: a request breaking a published value set AND another constraint is stated
    /// as ONE envelope carrying both clauses — the framework's, with only the value set
    /// reordered.
    #[test]
    fn every_clause_of_a_restated_refusal_is_the_frameworks_own() {
        let refusal = Refusal::Invalid(vec![
            enum_violation(&[
                "GITHUB",
                "GITLAB",
                "BITBUCKET",
                "CONFLUENCE",
                "AZURE_DEVOPS",
                "JIRA",
            ]),
            Violation {
                path: "clientId".to_string(),
                kind: ViolationKind::MissingRequiredMember,
                value: None,
                sensitive: false,
            },
        ]);
        let expected = "2 validation errors detected: Value 'NOT_A_PROVIDER' at 'provider' \
             failed to satisfy constraint: Member must satisfy enum value set: [GITHUB, \
             AZURE_DEVOPS, JIRA, BITBUCKET, CONFLUENCE, GITLAB]; Value null at 'clientId' \
             failed to satisfy constraint: Member must not be null";

        let stated = published_order_refusal(&refusal)
            .unwrap_or_else(|| panic!("case two clauses: expected a restated refusal"));
        assert_eq!(
            stated.code, VALIDATION_EXCEPTION,
            "case two clauses: expected the code {VALIDATION_EXCEPTION}, got {:?}",
            stated.code
        );
        assert_eq!(
            stated.message, expected,
            "case two clauses: expected {expected:?}, got {:?}",
            stated.message
        );
    }
}
