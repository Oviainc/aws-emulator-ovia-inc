// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-dynamodb-dynamodb-20120810` service: how it claims requests and how it words the
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
//! - `override_operation` — EDITED HERE. It still serves the operation the generated
//!   routing table names; what it adds is the SERVED OPERATION in scope for that
//!   operation's whole run, which is what tells [`Service::override_refusal`] which
//!   form a constraint clause takes (see "Which form a constraint clause takes").
//!   The routing decision itself is not overridden.
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
//! ## Naming the request a response answers
//!
//! Every response AWS sends names the request it answered, in the response header the
//! protocol reports that id in (`x-amzn-RequestId` for `awsJson1_0`) — on a success
//! exactly as on a failure, and freshly per HTTP ATTEMPT, so two deliveries of one
//! retried call are two responses naming two different requests. It is a property of the
//! RESPONSE, not of any modelled shape: no output and no error shape of this model
//! declares it, so no handler can set it, and the framework's `awsJson` renderers do not
//! write it.
//!
//! The rule is not restated here: [`service_common::query_error_envelope`] owns it for
//! every emulated service, and this file only chooses the report this protocol takes (the
//! header alone — `awsJson1_0` has no envelope element for the id) and wires it at the two
//! layers every response of this service passes through:
//!
//! * [`OperationScopedClauseForm`], the dispatch [`Service::override_operation`] hands the
//!   router in place of each modelled operation, covers everything an operation answers —
//!   a success, a modelled error, and a constraint refusal raised while that operation was
//!   being served;
//! * [`RequestIdEnvelopeDefinition::header_only`], the definition wrapper
//!   [`Service::definition`] hands back, covers a response NO operation produced: a
//!   routing failure, a transport failure detected up front, and a pre-dispatch
//!   authentication refusal.
//!
//! Nothing else about a response changes: the status, the other headers and every byte of
//! the body stay as the renderer wrote them.
//!
//! ## Wording a request authentication refused
//!
//! The emulator verifies every signature and hands this service one structural refusal for
//! every way a credential can fail, carrying a generic code that is an AUTHORIZATION denial
//! — which is not what AWS answers a credential failure with. The rule, and the sentences,
//! live in [`service_common::authentication_refusal`]; this file names only the two codes
//! this service answers the two failures with ([`AUTHENTICATION_REFUSAL`]) and wires that
//! layer under the definition wrapper above, so the id is reported on its response too.
//!
//! It is the definition rather than [`Service::override_refusal`] because the seam that is
//! handed WHICH failure it was is the definition's `refusal_response`; the refusal hook sees
//! the structural refusal alone, which is why this service uses it for input constraints.

use service_common::authentication_refusal::{
    AuthenticationRefusalDefinition, StatedAuthenticationRefusal,
};
use service_common::query_error_envelope::{
    with_request_id_header, RequestIdEnvelopeDefinition,
};
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, ErasedServiceDefinition, OperationContext, Refusal, Refused,
    Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The error code this service answers an input-constraint violation with: the
/// `ValidationException` (HTTP 400) every DynamoDB operation refuses a malformed
/// request with. Restated here because `override_refusal` replaces the code and
/// the message together.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The path segment the framework contributes when it steps from a collection
/// into one of its elements (`RequestItems.<table>.member.Keys`). It is a fixed
/// marker rather than a model name, so it is never re-cased — and the segment
/// BEFORE it is the caller's own map key or the element's 1-based position, so
/// that one is never re-cased either.
const COLLECTION_STEP: &str = "member";

/// A member path as DynamoDB states it: the framework builds the path in
/// lowerCamel (its own convention, `path::member`), and this service names a
/// member the way its model spells it on the wire — every DynamoDB member name
/// is UpperCamel, so restoring the wire spelling is restoring the first
/// character of each MODEL segment. A segment the caller chose (a map key) and
/// the collection marker itself pass through untouched.
///
/// The path arrives FLATTENED (`RequestItems.<table>.member.Keys`), so a map key
/// that itself contains a `.` — a table name may — is indistinguishable from two
/// segments. The segment immediately before the marker is the one taken as the
/// caller's, which is exact for every key without a dot and leaves the earlier
/// part of a dotted key re-cased; the alternative (never re-casing a run) would
/// leave real model segments lowerCamel, which is the error a client reading the
/// path would actually trip over.
fn wire_member_path(path: &str) -> String {
    let segments: Vec<&str> = path.split('.').collect();
    segments
        .iter()
        .enumerate()
        .map(|(index, segment)| {
            let is_collection_marker = *segment == COLLECTION_STEP;
            // A list index or a caller-chosen map key is the segment the
            // collection marker follows: `<key>.member`, `<position>.member`.
            let is_collection_selector =
                segments.get(index + 1).copied() == Some(COLLECTION_STEP);
            if is_collection_marker || is_collection_selector {
                (*segment).to_string()
            } else {
                upper_camel(segment)
            }
        })
        .collect::<Vec<String>>()
        .join(".")
}

/// Upper-case the first character of a member name, leaving the rest alone
/// (`requestItems` -> `RequestItems`, `sSESpecification` -> `SSESpecification`).
fn upper_camel(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The requirement a violation states, in AWS's fixed phrasing, for the clause
/// form the framework's own renderer cannot produce — the one that echoes NO
/// value. Every phrase comes from `crate::state`, which is where this service
/// spells each of them once (and says why they are not borrowed from the
/// framework).
///
/// `None` for a violation class this service has no wording for, which lets
/// [`validation_envelope`] decline the whole refusal and keep the framework's
/// wording rather than invent a sentence ([`ViolationKind`] is
/// `#[non_exhaustive]`, so a class can be added upstream).
fn requirement(kind: &ViolationKind) -> Option<String> {
    Some(match kind {
        ViolationKind::MinLength(min) => {
            crate::state::min_length_requirement(usize::try_from(*min).unwrap_or(0))
        }
        ViolationKind::MaxLength(max) => {
            crate::state::max_length_requirement(usize::try_from(*max).unwrap_or(0))
        }
        ViolationKind::MinValue(min) => crate::state::min_value_requirement(min),
        ViolationKind::MaxValue(max) => crate::state::max_value_requirement(max),
        ViolationKind::Pattern(pattern) => crate::state::pattern_requirement(pattern),
        ViolationKind::UniqueItems => crate::state::unique_items_requirement(),
        // Everything else has no no-echo phrase here: the enum and the
        // absent-member classes render exactly as the framework renders them, so
        // `violation_clause` hands those to `Violation::message` instead, and a
        // class added upstream has no phrase at all, which declines the refusal
        // and keeps the framework's wording.
        _ => return None,
    })
}

/// The input member DynamoDB's own request code validates rather than leaving to the
/// generic request walk, named by the path that walk reports it under.
///
/// Its refusal is worded differently from every other enumerated member's: the clause
/// states the requirement ALONE — neither the rejected value nor the member path — which
/// is what a check by the service's own code looks like next to the generic validator's
/// `Value '<v>' at '<path>' failed to satisfy constraint: …`.
///
/// This is ONE member, not a class, and it is here because the service's refusal of it
/// is observed to take that shape while the refusals of the service's other enumerated
/// members (`ReturnConsumedCapacity`, `Select`, `StreamViewType`, `ProjectionType`, …)
/// are observed to take the generic one. Nothing about the model distinguishes them, so
/// there is nothing to derive the difference from: a member later found to be checked
/// the same way needs its own entry, and
/// `tests/all/refusal_contracts_and_expression_pre_images.rs` pins both shapes side by
/// side so the difference is visible rather than silent.
const SERVICE_CHECKED_ENUM_MEMBER: &str = "returnValues";

/// The value set that refusal states for it.
///
/// It is the SERVICE's own set rather than the model's `ReturnValue` enum
/// (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#ReturnValue")` gives
/// `NONE ALL_OLD UPDATED_OLD ALL_NEW UPDATED_NEW`): the order differs and `UPDATED_NEW`
/// is spelled `UPDATE_NEW`. Both differences are the service's and are reproduced rather
/// than normalised, because a client matching the message has to read the same text from
/// the emulator and from DynamoDB; deriving the set from the model would state a
/// different sentence from the one the service sends.
const SERVICE_CHECKED_ENUM_SET: [&str; 5] =
    ["ALL_OLD", "UPDATED_OLD", "ALL_NEW", "UPDATE_NEW", "NONE"];

// =============================================================================
// WHICH FORM A CONSTRAINT CLAUSE TAKES
//
// DynamoDB states a clause about a member's declared bound in one of two forms:
//
//   * the ITEM-REQUEST form, which names the member by its WIRE spelling and
//     echoes NO value — `Value at 'Limit' failed to satisfy constraint: Member
//     must have value greater than or equal to 1` for a `Query` page size below
//     its `@range` minimum, and `Value at 'RequestItems.<table>.member.Keys' …`
//     for a batched key list past its cap;
//   * the REQUEST-VALIDATOR form, which names the member by the lowerCamel path
//     that validator builds and DOES echo the rejected value — `Value '0' at
//     'limit' …` for a backup listing's page size, `Value '26' at 'maxResults'
//     …` for an export listing's, `Value '' at
//     'timeToLiveSpecification.attributeName' …` for an empty TTL attribute name.
//
// Both forms are observed for the SAME violation class on the SAME member name
// (`Limit`, `@range` min 1, on `Query` and on `ListBackups`), so nothing about
// the model or about the violation distinguishes them: only the OPERATION being
// served does, which is why [`ITEM_REQUEST_FORM_OPERATIONS`] lists the operations
// whose refusals are observed to take the first form and every other operation
// takes the second. [`OperationScopedClauseForm`] is how the served operation
// reaches [`Service::override_refusal`], which the framework hands no request.
//
// TWO CLASSES ARE NOT PART OF THAT CHOICE, because neither is found by a
// validating walk at all: a value outside an enumerated member's set and a
// required member that is absent are both found while READING the request, and
// both keep the reader's own rendering whichever operation was served (`Value
// 'NOPE' at 'returnConsumedCapacity' …` on `GetItem`, `Value null at
// 'TableName' …`).
// =============================================================================

/// The operations whose refusal of a value outside a bound the model declares is
/// observed to take the item-request form above.
///
/// Nothing in the model distinguishes them from the rest — the same member name,
/// the same trait, a different sentence — so this list is the only place the
/// difference is recorded, and an operation later found to state that form needs
/// its own entry here. `tests/all/validation_message_fidelity.rs`,
/// `tests/all/validation_message_wording.rs` and
/// `tests/all/index_key_types_and_range_refusals.rs` pin both forms side by side so
/// the difference is visible rather than silent.
///
/// Sorted, which [`clause_form`] relies on for `binary_search` and
/// `item_request_form_operations_are_sorted` pins.
const ITEM_REQUEST_FORM_OPERATIONS: &[&str] = &[
    "BatchGetItem",
    "BatchWriteItem",
    "Query",
    "TransactGetItems",
    "TransactWriteItems",
];

/// Which of the two forms above a clause names its member and its value by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClauseForm {
    /// The member as the model spells it on the wire, with no value echoed.
    WireMemberOnly,
    /// The validator's lowerCamel path, with the rejected value echoed.
    PathAndValue,
}

/// The form `operation`'s refusals take.
fn clause_form(operation: &str) -> ClauseForm {
    if ITEM_REQUEST_FORM_OPERATIONS.binary_search(&operation).is_ok() {
        ClauseForm::WireMemberOnly
    } else {
        ClauseForm::PathAndValue
    }
}

tokio::task_local! {
    /// The form the operation this request is being served as states its clauses in,
    /// set by [`OperationScopedClauseForm::run`] around that operation's whole run and
    /// read back by [`EmulatedService::override_refusal`].
    static SERVED_CLAUSE_FORM: ClauseForm;
}

/// The form the operation this task is serving states its clauses in.
///
/// Absent altogether means the refusal happened outside an operation's run, where
/// there is no served operation to read a form off; the request validator's form is
/// the general one, so that is what such a refusal states.
fn served_clause_form() -> ClauseForm {
    SERVED_CLAUSE_FORM
        .try_with(|form| *form)
        .unwrap_or(ClauseForm::PathAndValue)
}

/// Serves a request as the operation the generated routing table names, with that
/// operation's clause form ([`clause_form`]) in scope for the whole run, and names the
/// request in the response's header (see "Naming the request a response answers").
///
/// It adds nothing else: the request, the service and the context are handed to the
/// modelled operation unchanged, so deserialization, validation, the handler and the
/// response are the framework's and the operation's own. The routing table holds only
/// generated operations, so the inner lookup can never resolve back to this type.
struct OperationScopedClauseForm;

impl ErasedOperation for OperationScopedClauseForm {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            // `override_operation` returns this type only for a request the routing
            // table can serve, so the lookup below resolves; a request that cannot be
            // routed keeps the framework's routing-error answer either way.
            let routed = svc
                .definition()
                .protocol()
                .routing_key(req)
                .ok()
                .and_then(|key| Some((key, svc.definition().routing_table().get(key)?)));
            let Some((operation, op)) = routed else {
                return svc.definition().routing_error_response(req);
            };
            let answered = SERVED_CLAUSE_FORM
                .scope(clause_form(operation), op.run(svc, req, ctx))
                .await;
            // This is the ONE place every response an operation of this service produces
            // passes through, so no operation can answer without naming the request it
            // answered — and each attempt gets its own id, because each attempt is served
            // here once.
            with_request_id_header(answered)
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names" — the
        // operation this dispatch delegates to — so its model-declared schemes decide and
        // this wrapper cannot weaken them.
        None
    }
}

/// One clause for one violated member, in the `form` the served operation states
/// its clauses in (see "Which form a constraint clause takes").
///
/// Two classes are rendered the same way whichever operation was served, because
/// neither is found by the validating walk:
///
/// * a string outside an enumerated member's value set is reported the way the
///   reader reports it — the member's lowerCamel path with the rejected value
///   echoed, so a caller can read out of the message which value of which member
///   was refused. That is exactly the framework's own rendering, so the framework
///   renders it (`Violation::message`) rather than this service restating the
///   sentence. [`SERVICE_CHECKED_ENUM_MEMBER`] is the one member that is not
///   reported that way, because the service checks it itself;
/// * an absent member states the `null` it was, which is a fact of the request's
///   shape — the framework's rendering again, over the member path spelled as the
///   model spells it.
///
/// A COLLECTION member past its cap is a clause like any other: in the item-request
/// form the member and the bound, with nothing echoed. Some recorded AWS refusals of a
/// capped collection — the transactional `TransactItems` lists — instead echo the
/// rejected list as AWS's OWN internal object rendering
/// (`[TransactGetItem(get=Get(key={pk=AttributeValue(s=k0, n=null, …,
/// iNT=null, dECIMALSET=null, …)}, …, workloadProfileName=null)), …]`), over
/// members this service's model does not declare:
/// `smithy_query(query="shape", shape="com.amazonaws.dynamodb#AttributeValue")`
/// gives `S N B SS NS BS M L NULL BOOL` and no `iNT`/`dECIMALSET`, and `Get`
/// and `Put` carry no `workloadProfileName`. No rule reachable from the model
/// or the docs renders that text, two recorded refusals of the SAME violation
/// class (a `BatchGetItem` key list past the same 100 cap) state the
/// no-value, wire-path form this function builds, and the framework hands this
/// seam a count rather than the collection — so the clause states what is
/// derivable: the member and the bound it broke.
/// `tests/all/collection_cap_constraint_reports.rs` pins that over the capped
/// collections of the batched and transactional operations.
fn violation_clause(violation: &Violation, form: ClauseForm) -> Option<String> {
    match &violation.kind {
        // The one member the service checks itself: the requirement with nothing
        // echoed, over the set that check states.
        ViolationKind::EnumValue(_) if violation.path == SERVICE_CHECKED_ENUM_MEMBER => {
            Some(format!(
                "Failed to satisfy constraint: {}",
                crate::state::enum_value_requirement(&SERVICE_CHECKED_ENUM_SET)
            ))
        }
        ViolationKind::MissingRequiredMember | ViolationKind::EnumValue(_) => Some(
            Violation {
                path: match &violation.kind {
                    // An absent member names itself as the model spells it; an
                    // enum violation keeps the reader's own path, which is what
                    // the recorded refusals for this service state.
                    ViolationKind::MissingRequiredMember => wire_member_path(&violation.path),
                    _ => violation.path.clone(),
                },
                ..violation.clone()
            }
            .message(),
        ),
        // Every bound the validating walk checks — a `@range`, a `@length`, a
        // `@pattern`, `@uniqueItems` — in the served operation's form.
        _ => match form {
            ClauseForm::PathAndValue => Some(violation.message()),
            ClauseForm::WireMemberOnly => Some(crate::state::constraint_clause(
                &wire_member_path(&violation.path),
                None,
                &requirement(&violation.kind)?,
            )),
        },
    }
}

/// The `ValidationException` message for a framework refusal: the same envelope
/// `crate::state::validation_message` builds for the bounds this service checks
/// itself, over one clause per violated member.
///
/// `None` declines the whole refusal — see [`requirement`].
fn validation_envelope(violations: &[Violation]) -> Option<String> {
    let form = served_clause_form();
    let clauses: Vec<String> = violations
        .iter()
        .map(|violation| violation_clause(violation, form))
        .collect::<Option<Vec<_>>>()?;
    crate::state::validation_message(clauses)
}

/// The service key `::emulator_server::service_from_target` reads out of a
/// DynamoDB STREAMS request's `X-Amz-Target` header. That service signs as
/// `dynamodb` too, so this key is what tells its traffic apart from ours: the
/// shared reader takes the segment before the `.`, strips the `_<version>`
/// suffix and lower-cases it, so `DynamoDBStreams_20120810.<Op>` normalises to
/// this key while `DynamoDB_20120810.<Op>` normalises to `dynamodb`. The streams
/// crate's own `can_handle` compares against the same key, so one discriminator
/// is read one way on both sides.
const STREAMS_TARGET_SERVICE_KEY: &str = "dynamodbstreams";

/// The two codes this service answers a request the emulator's authentication layer refused
/// with (see [`service_common::authentication_refusal`], which owns the rule and the
/// sentences): the `awsJson` unknown-credential code for an access key id that resolves to
/// no credential, and the `awsJson` signature code for one that resolves and whose signature
/// does not verify. Neither is `AccessDenied`, which is this service's answer to a KNOWN
/// principal whose policies do not permit the action.
const AUTHENTICATION_REFUSAL: StatedAuthenticationRefusal = StatedAuthenticationRefusal {
    unresolved_credential: "UnrecognizedClientException",
    signature_mismatch: "InvalidSignatureException",
};

/// The `com-amazonaws-dynamodb-dynamodb-20120810` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    /// The generated entry, with its three framework-level error renderers wrapped so a
    /// response no operation produced still names the request it answers, and inside that
    /// the layer that states an authentication refusal in this service's own codes
    /// ([`AUTHENTICATION_REFUSAL`]).
    entry: RequestIdEnvelopeDefinition<
        AuthenticationRefusalDefinition<crate::ServiceDefinition<Eng>>,
    >,
    /// The dispatch `override_operation` hands back, which carries the served
    /// operation's clause form into [`Service::override_refusal`] and names the request
    /// on the response. Held on the service because that seam returns a borrow of
    /// `self`.
    clause_form_scope: OperationScopedClauseForm,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: RequestIdEnvelopeDefinition::header_only(AuthenticationRefusalDefinition::new(
                crate::ServiceDefinition::new(engine),
                AUTHENTICATION_REFUSAL,
            )),
            clause_form_scope: OperationScopedClauseForm,
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
        // DynamoDB and DynamoDB Streams are two services behind ONE SigV4 signing
        // name (`dynamodb`), which the model cannot express, so the default
        // signing-name match makes BOTH claim every DynamoDB-signed request and the
        // router answers `500 AmbiguousService` for all of it. On awsJson1_0 the
        // request names its service in `X-Amz-Target`, so a
        // `DynamoDBStreams_20120810.*` target belongs to the streams service and is
        // declined here.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) if key == self.entry.identity().signing_name() => {
                ::emulator_server::service_from_target(&parts.headers).as_deref()
                    != Some(STREAMS_TARGET_SERVICE_KEY)
            }
            _ => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Serve the operation the generated routing table names, through the scope
        // that carries its clause form into `override_refusal` (see "Which form a
        // constraint clause takes"). The routing decision itself is NOT overridden:
        // the key is resolved by the model's own protocol, and a request the table
        // cannot serve is declined here so the framework's routing-error path stays
        // exactly as it was.
        let operation = self.definition().protocol().routing_key(req).ok()?;
        self.definition().routing_table().get(operation)?;
        Some(&self.clause_form_scope)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // DynamoDB words such a refusal as a `ValidationException` (HTTP 400 —
        // the status and code the framework would also have answered, so only
        // the wording changes) carrying the `<N> validation error(s) detected: `
        // envelope over one clause per violated member, each clause naming the
        // member and the constraint it failed in the form the SERVED OPERATION
        // states its clauses in (see "Which form a constraint clause takes"). An
        // authentication refusal is declined here — which failure it was is not in the
        // structural refusal this seam is handed — and worded by the definition layer
        // instead (see "Wording a request authentication refused").
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: VALIDATION_EXCEPTION.to_string(),
                message: validation_envelope(violations)?,
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
    use super::{clause_form, ClauseForm, ITEM_REQUEST_FORM_OPERATIONS};

    #[test]
    fn item_request_form_operations_are_sorted() {
        let mut sorted = ITEM_REQUEST_FORM_OPERATIONS.to_vec();
        sorted.sort_unstable();
        assert_eq!(
            ITEM_REQUEST_FORM_OPERATIONS,
            sorted.as_slice(),
            "case the item-request list: expected it sorted for `binary_search`, got \
             {ITEM_REQUEST_FORM_OPERATIONS:?}"
        );
    }

    #[test]
    fn the_served_operation_chooses_the_clause_form() {
        for operation in ITEM_REQUEST_FORM_OPERATIONS {
            let form = clause_form(operation);
            assert_eq!(
                form,
                ClauseForm::WireMemberOnly,
                "case {operation:?}: expected the item-request form, got {form:?}"
            );
        }
        for operation in [
            "Scan",
            "ListBackups",
            "ListExports",
            "UpdateTimeToLive",
            "CreateTable",
            "DescribeTable",
        ] {
            let form = clause_form(operation);
            assert_eq!(
                form,
                ClauseForm::PathAndValue,
                "case {operation:?}: expected the request-validator form, got {form:?}"
            );
        }
    }
}
