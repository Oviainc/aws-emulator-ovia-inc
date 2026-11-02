// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-detective-amazondetective` service: how it claims requests and how it words the
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
    http, ErasedOperation, Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

/// The error code every refusal below keeps: Detective words these refusals its own
/// way, but a client still branches on the modelled `ValidationException` code.
const VALIDATION_EXCEPTION_CODE: &str = "ValidationException";

/// The path segment the validating walk puts between a collection and its element
/// (`accounts.1.member.accountId`).
const COLLECTION_STEP: &str = "member";

/// `CreateMembers$Accounts`' path prefix, as the validating walk spells it: the member
/// name in lowerCamel.
const ACCOUNTS_PATH_PREFIX: &str = "accounts.";

/// `UpdateDatasourcePackages$DatasourcePackages`' path prefix, likewise.
const DATASOURCE_PACKAGES_PATH_PREFIX: &str = "datasourcePackages.";

/// What completes a start-anchored regular expression into one that matches a whole
/// value — the form a `@pattern` refusal reports. See [`reported_pattern`].
const WHOLE_VALUE_PATTERN_SUFFIX: &str = ".*";

/// How Detective states a refusal the framework described structurally.
///
/// Two of its operations validate a member themselves rather than leaving it to the
/// generic constraint reporter, and both say so with a message that does not read like
/// one:
///
/// * `CreateMembers` reports a `@pattern` failure of a member of one of its `Accounts`
///   entries as `<Shape>.<Member> did not match the pattern.` — a fixed sentence that
///   names the member in its API-level spelling and echoes neither the offending value
///   nor its position in the list. Recorded: two different 12-character non-numeric
///   `AccountId`s produce the one same message.
/// * `UpdateDatasourcePackages` reports values outside the `DatasourcePackage` value
///   set by LISTING them, which is the only place the service names which packages it
///   did not recognise.
///
/// A third sentence of the same family never reaches this seam at all: the `@pattern`
/// failure of the ELEMENTS of a list member is stated with a clause naming the operation's
/// INPUT SHAPE (`crate::state::list_members_pattern_message`), and a [`Refusal`] carries
/// the violations without the shape they were found in — two operations of this service
/// declare an `AccountIds` list, so the input shape cannot be recovered from the member
/// path. Those two operations therefore own that one class themselves; see
/// `crate::ops::get_members`.
///
/// Everything else keeps the framework's generic `N validation error(s) detected: …`
/// envelope, which is what AWS answers for the rest of this service's members
/// (`GetInvestigation$InvestigationId`, say) — except that a `@pattern` clause inside it
/// reports the expression the SERVICE states rather than the raw modelled one; see
/// [`reported_pattern`].
fn detective_refusal(violations: &[Violation]) -> Option<Refused> {
    unknown_datasource_packages(violations)
        .or_else(|| account_member_pattern(violations))
        .or_else(|| pattern_as_reported(violations))
        .map(|message| Refused {
            code: VALIDATION_EXCEPTION_CODE.to_string(),
            message,
        })
}

/// How this service reports the regular expression a `@pattern` refusal refused
/// against.
///
/// The service validates a value against the WHOLE of it, so the expression it reports
/// is the whole-value form of the modelled pattern: a pattern anchored at the start but
/// not terminated at the end is reported with the trailing `.*` that completes it, while
/// one that already terminates (`…$`) is reported exactly as modelled. Recorded
/// behaviour: the only member of this service whose pattern is unterminated
/// (`StartInvestigation$EntityArn`, `smithy_query(query="shape",shape="EntityArn")`) is
/// refused with `^arn:.*`, while every terminated one is refused with its own pattern.
fn reported_pattern(pattern: &str) -> String {
    if pattern.starts_with('^') && !pattern.ends_with('$') && !pattern.ends_with(".*") {
        format!("{pattern}{WHOLE_VALUE_PATTERN_SUFFIX}")
    } else {
        pattern.to_string()
    }
}

/// The framework's envelope re-rendered with every `@pattern` clause reporting the
/// expression [`reported_pattern`] states, or `None` when no clause's rendering differs
/// — in which case the framework's wording already is this service's.
fn pattern_as_reported(violations: &[Violation]) -> Option<String> {
    let reported: Vec<Violation> = violations
        .iter()
        .map(|violation| match &violation.kind {
            ViolationKind::Pattern(pattern) => Violation {
                kind: ViolationKind::Pattern(reported_pattern(pattern)),
                ..violation.clone()
            },
            _ => violation.clone(),
        })
        .collect();
    if reported.as_slice() == violations {
        return None;
    }
    // The clause itself, the value it echoes and the member path it names are the
    // framework's: only the expression it reports is this service's choice.
    aws_smithy_schema_server::constraints::envelope(&reported)
}

/// The `UpdateDatasourcePackages` message naming every requested package outside the
/// `DatasourcePackage` value set, in the order the request listed them, or `None` when
/// no violation is one of those.
fn unknown_datasource_packages(violations: &[Violation]) -> Option<String> {
    let unknown: Vec<&str> = violations
        .iter()
        .filter(|violation| {
            matches!(violation.kind, ViolationKind::EnumValue(_))
                && violation.path.starts_with(DATASOURCE_PACKAGES_PATH_PREFIX)
                && violation.path.ends_with(COLLECTION_STEP)
        })
        .filter_map(|violation| violation.value.as_deref())
        .collect();
    if unknown.is_empty() {
        return None;
    }
    Some(crate::state::unknown_datasource_packages_message(&unknown))
}

/// The `CreateMembers` message for a `@pattern` failure of a member of one of its
/// `Accounts` entries, or `None` when no violation is one of those.
fn account_member_pattern(violations: &[Violation]) -> Option<String> {
    violations
        .iter()
        .filter(|violation| matches!(violation.kind, ViolationKind::Pattern(_)))
        .find_map(|violation| account_member_name(&violation.path))
        .map(|member| crate::state::account_member_pattern_message(&member))
}

/// The API-level spelling of the `Account` member `path` names
/// (`accounts.1.member.accountId` -> `AccountId`), or `None` when `path` does not name
/// one.
///
/// The validating walk spells a member in lowerCamel, and Detective's own message
/// spells it as the model does, so the one difference is the leading character.
fn account_member_name(path: &str) -> Option<String> {
    let member = path
        .strip_prefix(ACCOUNTS_PATH_PREFIX)?
        .split_once(&format!(".{COLLECTION_STEP}."))
        .map(|(position, member)| (position.bytes().all(|b| b.is_ascii_digit()), member))
        .filter(|(positional, member)| *positional && !member.contains('.'))
        .map(|(_, member)| member)?;
    let mut chars = member.chars();
    let first = chars.next()?;
    Some(format!("{}{}", first.to_ascii_uppercase(), chars.as_str()))
}

/// The `com-amazonaws-detective-amazondetective` emulated service: the generated [`ServiceDefinition`] plus
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
        // required member, or a value outside a declared bound) to let this service
        // reword it. Two of this service's operations validate a member themselves
        // rather than leaving it to the generic constraint reporter, and
        // `detective_refusal` above states those two the way the service does —
        // replacing the code and the message together. Everything else keeps the
        // framework's envelope (the form AWS itself returns for the rest of this
        // service's members), with the one adjustment `reported_pattern` makes to the
        // expression a `@pattern` clause reports.
        match refusal {
            Refusal::Invalid(violations) => detective_refusal(violations),
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
