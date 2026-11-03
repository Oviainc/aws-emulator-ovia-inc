// SPDX-License-Identifier: CC0-1.0

//! How this service STATES a request its modelled constraints refuse, and the body member
//! it reports the detail text of any failure in.
//!
//! Two facts about IAM Identity Center's wire behaviour live here, both of them about an
//! error body and neither recorded in a model.
//!
//! # 1. The member the detail text is in is `Message`
//!
//! Every error shape this service's model declares spells its detail member `Message`,
//! with no `@jsonName`
//! (`smithy_query(query="shape",shape="ValidationException,AccessDeniedException,ConflictException,InternalServerException,ResourceNotFoundException,ServiceQuotaExceededException,ThrottlingException",model_path="services/service-com-amazonaws-ssoadmin-swbexternalservice/smithy-model/com-amazonaws-ssoadmin-swbexternalservice.json")`),
//! and this is an `awsJson1_1` service, so a member's wire name IS its Smithy member name:
//! a modelled failure a handler returns renders as `{"__type":…,"Message":…}`. The error
//! bodies the FRAMEWORK renders rather than a handler — the boundary refusal raised for a
//! value outside a member's declared constraint, and the scaffold's 501 — are built from
//! synthetic shapes in the `smithy.framework` namespace that spell that member `message`,
//! so the one member an IAM Identity Center client reads the detail text out of is absent
//! from exactly the responses that have the most to say. [`with_detail_member`] renames
//! it, so the text is readable wherever it was produced; everything else about the
//! response — the status, the `__type`, every other member and header, and the whole
//! success path — is left exactly as the renderer wrote it.
//!
//! # 2. How a constraint refusal is worded
//!
//! The framework hands a service the refusal STRUCTURALLY — `Refusal::Invalid` holding one
//! [`Violation`] per constraint the request broke — and lets the service choose the words,
//! because no model records them. IAM Identity Center states one clause per violation:
//!
//! ```text
//! Value of input '<member path>' failed to satisfy constraint: <requirement>
//! ```
//!
//! with no `N validation error(s) detected:` envelope around it, several clauses joined by
//! `"; "`, and a `@pattern` requirement quoted as the service publishes the expression —
//! without the `^…$` pair the model wraps it in to mean "the whole value"
//! ([`unanchored`]). The member half is the framework's own
//! lowerCamel member path and the requirement half is the framework's own requirement
//! sentence, so only what this service words differently is worded here. The clause form
//! does not quote the rejected VALUE back at the caller — it names the member and the
//! constraint it broke, which is what the recorded refusals carry.
//!
//! Several clauses come in the REVERSE of the order the model declares the members they
//! name, which is the reverse of the order the framework's validating walk finds them in
//! (that walk follows the input shape's member declaration order). Recorded behaviour of
//! the live service, which no model records: a `ListTagsForResource` carrying a malformed
//! `ResourceArn` and a malformed `InstanceArn` — declared `InstanceArn`, `ResourceArn` in
//! that order — is refused with the `resourceArn` clause first. The same accumulation
//! shows in other recorded AWS services (`directoryservicedata` `ListGroupMembers`,
//! declared `Realm` before `MemberRealm`, answers the `memberRealm` clause first), so it
//! is a property of how the validator accumulates failures rather than of one operation,
//! and it is stated once, here, for every operation of this service.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::constraints::{Violation, ViolationKind};
use aws_smithy_types::body::SdkBody;
use serde_json::Value;
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, OperationContext, Refusal, Refused, Service,
};

/// The error code this service answers a constraint refusal with: its own modelled
/// `ValidationException`, which the model declares with `@httpError 400`. Restated here
/// because `override_refusal` replaces the code and the message together, and it is also
/// the code the framework's own default picks on the awsJson protocols — so a client
/// branches on the same code however the refusal was worded.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The body member this service reports an exception's detail text in.
pub const DETAIL_MEMBER: &str = "Message";

/// The body member the framework's own renderers write that text in: the member name of
/// the synthetic `smithy.framework#ValidationException` / `NotImplemented` shapes.
const RENDERED_DETAIL_MEMBER: &str = "message";

/// The fixed text the framework puts between a clause's member half and the requirement
/// it failed, in [`Violation::message`].
const CONSTRAINT_MARKER: &str = " failed to satisfy constraint: ";

/// What this service puts between the clauses of one refusal.
const CLAUSE_JOINER: &str = "; ";

/// State `refusal` the way IAM Identity Center states it, or `None` to keep the
/// framework's own wording.
///
/// This is the whole `Service::override_refusal` body: a constraint refusal (a value
/// outside a declared constraint, or a `@required` member absent — both arrive as
/// `Refusal::Invalid`) is reworded for every constraint class and every operation, because
/// the wording is a property of the service's validator and not of one member. Anything
/// that is not a constraint refusal — an unauthenticated caller, which the framework
/// answers 403 — keeps the framework's own code and wording, and so does a refusal with
/// nothing to state.
pub fn refused(refusal: &Refusal) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    // Reversed: the live service reports the clause of the LAST-declared refused member
    // first (see the module docs), and the walk hands them over in declaration order.
    let clauses: Vec<String> = violations.iter().rev().map(clause).collect();
    if clauses.is_empty() {
        return None;
    }
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_owned(),
        message: clauses.join(CLAUSE_JOINER),
    })
}

/// One violation as this service states it.
fn clause(violation: &Violation) -> String {
    format!(
        "Value of input '{}'{CONSTRAINT_MARKER}{}",
        stated_path(&violation.path),
        requirement_of(violation)
    )
}

/// The clause this service states for a value past the maximum of a `@range`, for a
/// handler to refuse with.
///
/// The validating walk checks the constraints the SCHEMA carries — the ones the member's
/// TARGET shape declares — so an operation whose member narrows that range is the one
/// place the narrower bound can be checked. Worded through [`clause`] rather than spelled
/// out, so such a refusal is indistinguishable from the one the walk states for the same
/// member and the same constraint class.
pub fn value_above_maximum(member_path: &str, maximum: i32) -> String {
    clause(&Violation {
        path: member_path.to_string(),
        kind: ViolationKind::MaxValue(maximum.to_string()),
        value: None,
        sensitive: false,
    })
}

/// The member path as the clause names it: the framework's own path, with the three
/// sequences the message format itself uses taken out of it.
///
/// A path segment is usually a MODEL name (lowerCamel) or a list position, neither of
/// which can carry them — but the segment of a map entry is the KEY AS SENT, so a caller
/// chooses that text. Left verbatim, a key spelling the clause separator or the quote
/// would render one violation as what a client parses as several, naming members the
/// request never carried. Removing those sequences keeps the clause a faithful report of
/// one violation; every path this model's own members produce is unchanged by it.
fn stated_path(path: &str) -> String {
    path.replace(CONSTRAINT_MARKER, " ")
        .replace(CLAUSE_JOINER, " ")
        .replace('\'', "")
}

/// `pattern` as the service publishes the expression: without the anchors bracketing it —
/// a leading `^` and a trailing `$` the expression itself has not escaped.
///
/// A Smithy `@pattern` is an unanchored regular-expression SEARCH, so a model that wants
/// the whole value to match writes the anchors in; they say "all of it" and are not part of
/// the expression a caller has to satisfy, which is why the published expression does not
/// carry them. The CHECK always runs the model's pattern as written: only this RENDERING
/// drops the pair, and only when the expression carries BOTH — one anchor alone is a
/// different expression, and so is one whose own branches carry them (`(^a$)|(^b$)`). A `$`
/// that is ESCAPED is a literal dollar, so nothing is dropped.
///
/// The rule is stated for the fleet, with the same semantics and its own table of forms, in
/// `service_common::validation::unanchored`, and that is where it belongs; it is restated
/// here — as the `evs`, `codeguruprofiler` and `fsx` service crates restate it in theirs —
/// so this crate adds no dependency EDGE, which is recorded in `Cargo.lock`: the one file
/// every landing fix also writes, and which the squash merge refuses when both sides have
/// touched it.
fn unanchored(pattern: &str) -> &str {
    let Some(body) = pattern.strip_prefix('^') else {
        return pattern;
    };
    match body.strip_suffix('$') {
        // An ODD number of trailing backslashes escapes the `$`, making it a literal
        // dollar the value must carry rather than an anchor.
        Some(inner) if trailing_backslashes(inner).is_multiple_of(2) => inner,
        _ => pattern,
    }
}

/// How many `\` characters `text` ends with.
fn trailing_backslashes(text: &str) -> usize {
    text.chars().rev().take_while(|c| *c == '\\').count()
}

/// The requirement half of a clause: what the value had to satisfy.
///
/// Every constraint keeps the framework's own requirement wording, so a constraint class
/// it learns to check is reported rather than dropped. It is read off the clause the
/// framework builds for this SAME violation with no value attached, at the LAST marker in
/// it: both halves of that clause can carry text the CALLER chose — the rejected value,
/// and, for a map entry whose path segment is the key as sent, the member path — while a
/// requirement never contains the marker, so the last occurrence is the real separator.
///
/// A `@pattern` is the one requirement reworded: the anchors are the model's way of saying
/// "the whole value", not part of the expression the service publishes.
fn requirement_of(violation: &Violation) -> String {
    if let ViolationKind::Pattern(pattern) = &violation.kind {
        return format!(
            "Member must satisfy regular expression pattern: {}",
            unanchored(pattern)
        );
    }
    let valueless = Violation {
        value: None,
        ..violation.clone()
    };
    let rendered = valueless.message();
    match rendered.rsplit_once(CONSTRAINT_MARKER) {
        Some((_member_half, requirement)) => requirement.to_string(),
        // A wording with no requirement half: keep what the framework rendered.
        None => rendered,
    }
}

/// The error body with its detail member renamed to [`DETAIL_MEMBER`], or `None` when
/// there is nothing to rename: the body is not a JSON object, or it carries no rendered
/// detail member.
///
/// Idempotent, and non-destructive: a body that already carries a `Message` member keeps
/// that value, so a modelled member of the error shape always wins over the renamed one.
fn with_detail_member(body: &[u8]) -> Option<Vec<u8>> {
    let mut document: Value = serde_json::from_slice(body).ok()?;
    let object = document.as_object_mut()?;
    let detail = object.remove(RENDERED_DETAIL_MEMBER)?;
    object.entry(DETAIL_MEMBER).or_insert(detail);
    serde_json::to_vec(&document).ok()
}

/// The response as this service answers it: an error body whose detail text is in its
/// `Message` member.
///
/// A response that is not a failure, or whose body is not an error body carrying a
/// rendered detail member (see [`with_detail_member`]), is returned untouched — status,
/// headers and every byte of the body exactly as the renderer wrote them.
fn with_service_detail_member(mut resp: RtResponse) -> RtResponse {
    if resp.status().as_u16() < 400 {
        return resp;
    }
    let rewritten = {
        // An error body is built in memory by its renderer, so the bytes are readable
        // here; a streamed body is not an error body and is left alone.
        let Some(bytes) = resp.body().bytes() else {
            return resp;
        };
        match with_detail_member(bytes) {
            Some(rewritten) => rewritten,
            None => return resp,
        }
    };
    // Only a renderer that advertised a length needs it corrected; the framework's error
    // renderers set `content-type` alone and leave framing to the transport.
    if resp.headers().get("content-length").is_some() {
        let length = rewritten.len().to_string();
        // A decimal length is always a valid header value, so this cannot fail today; if
        // it ever did, the header is REMOVED rather than left describing the body before
        // the rename, so the two can never disagree (framing then falls to the transport,
        // which is what the framework's own error renderers already leave it to).
        if resp
            .headers_mut()
            .try_insert("content-length", length)
            .is_err()
        {
            resp.headers_mut().remove("content-length");
        }
    }
    *resp.body_mut() = SdkBody::from(rewritten);
    resp
}

/// The operation the generated routing table names for this request.
fn modeled_operation<'s>(
    svc: &'s dyn Service,
    req: &'s RtRequest,
) -> Option<&'s dyn ErasedOperation> {
    let key = svc.definition().protocol().routing_key(req).ok()?;
    svc.definition().routing_table().get(key)
}

/// Whether the generated routing table can serve this request — the condition under which
/// this service claims it for [`DetailMemberOperation`].
pub fn routes<Eng: Send + Sync + 'static>(
    entry: &crate::ServiceDefinition<Eng>,
    req: &RtRequest,
) -> bool {
    entry
        .protocol()
        .routing_key(req)
        .ok()
        .and_then(|key| entry.routing_table().get(key))
        .is_some()
}

/// Serves a modelled operation through the framework exactly as the routing table would,
/// then reports the detail text of whatever it answered in this service's `Message`
/// member.
///
/// It stores no operation of its own: the one it delegates to is the one the service's
/// routing table names for the request, resolved from the `svc` the framework passes into
/// [`ErasedOperation::run`]. So the framework owns the whole request walk and every status,
/// code and text it answers with; this wrapper owns only which member that text is in.
pub struct DetailMemberOperation;

impl ErasedOperation for DetailMemberOperation {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            match modeled_operation(svc, req) {
                Some(op) => with_service_detail_member(op.run(svc, req, ctx).await),
                // Unreachable through `Service::override_operation`, which claims a
                // request only when the routing table holds its key; answered as the
                // framework answers an unroutable request rather than guessed at.
                None => svc.definition().routing_error_response(req),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names" — which is
        // the operation this wrapper delegates to, so its model-declared schemes decide
        // and this wrapper cannot weaken them.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One violation of `kind` at `path`, for the value a request sent.
    fn violation(path: &str, kind: ViolationKind, value: Option<&str>) -> Violation {
        Violation {
            path: path.to_owned(),
            kind,
            value: value.map(str::to_owned),
            sensitive: false,
        }
    }

    /// The instance-ARN pattern every `InstanceArn` member of this model carries
    /// (`smithy_query(query="shape",shape="InstanceArn")`).
    const INSTANCE_ARN_PATTERN: &str =
        r"^arn:aws(-[a-z]{1,5}){0,3}:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}$";

    /// The message `refused` states for `violations`.
    fn stated(violations: Vec<Violation>) -> Refused {
        refused(&Refusal::Invalid(violations)).expect("a constraint refusal is stated")
    }

    /// Every constraint class is stated in the one clause form, with the requirement the
    /// framework words it with — and a `@pattern` quoted without the model's anchors.
    #[test]
    fn every_constraint_class_is_stated_in_this_services_clause_form() {
        // (case, the violation, the clause it must be stated as)
        let rows: Vec<(&str, Violation, &str)> = vec![
            (
                "a pattern, quoted without the model's anchors",
                violation(
                    "instanceArn",
                    ViolationKind::Pattern(INSTANCE_ARN_PATTERN.to_owned()),
                    Some("arn:aws:sso:::instance/not-a-valid-id"),
                ),
                "Value of input 'instanceArn' failed to satisfy constraint: Member must \
                 satisfy regular expression pattern: arn:aws(-[a-z]{1,5}){0,3}:sso:::instance/\
                 (sso)?ins-[a-zA-Z0-9-.]{16}",
            ),
            (
                "an absent required member",
                violation("resourceArn", ViolationKind::MissingRequiredMember, None),
                "Value of input 'resourceArn' failed to satisfy constraint: Member must not \
                 be null",
            ),
            (
                "a minimum length",
                violation("instanceArn", ViolationKind::MinLength(10), Some("")),
                "Value of input 'instanceArn' failed to satisfy constraint: Member must have \
                 length greater than or equal to 10",
            ),
            (
                "a maximum length",
                violation(
                    "name",
                    ViolationKind::MaxLength(32),
                    Some("x".repeat(33).as_str()),
                ),
                "Value of input 'name' failed to satisfy constraint: Member must have length \
                 less than or equal to 32",
            ),
            (
                "a maximum value",
                violation(
                    "maxResults",
                    ViolationKind::MaxValue("100".to_owned()),
                    Some("1000"),
                ),
                "Value of input 'maxResults' failed to satisfy constraint: Member must have \
                 value less than or equal to 100",
            ),
            (
                "a minimum value",
                violation(
                    "maxResults",
                    ViolationKind::MinValue("1".to_owned()),
                    Some("0"),
                ),
                "Value of input 'maxResults' failed to satisfy constraint: Member must have \
                 value greater than or equal to 1",
            ),
            (
                "an enum value set",
                violation(
                    "principalType",
                    ViolationKind::EnumValue(vec!["USER".to_owned(), "GROUP".to_owned()]),
                    Some("ROBOT"),
                ),
                "Value of input 'principalType' failed to satisfy constraint: Member must \
                 satisfy enum value set: [USER, GROUP]",
            ),
            (
                "a nested member, named by its own path",
                violation("tags.1.member.key", ViolationKind::MinLength(1), Some("")),
                "Value of input 'tags.1.member.key' failed to satisfy constraint: Member must \
                 have length greater than or equal to 1",
            ),
            (
                "a repeated element of a @uniqueItems list",
                violation("tagKeys", ViolationKind::UniqueItems, Some("2 items")),
                "Value of input 'tagKeys' failed to satisfy constraint: Member must have \
                 unique values",
            ),
            (
                // The segment of a map entry is the KEY AS SENT, so a caller chooses it:
                // a key spelling the clause separator cannot forge a second clause.
                "a map key that spells the clause separator",
                violation(
                    "tags.k' failed to satisfy constraint: Member must not be null; Value of \
                     input 'forged.member",
                    ViolationKind::MinLength(1),
                    Some(""),
                ),
                "Value of input 'tags.k Member must not be null Value of input \
                 forged.member' failed to satisfy constraint: Member must have length \
                 greater than or equal to 1",
            ),
        ];

        for (case, violation, expected) in rows {
            let stated = stated(vec![violation]);
            assert_eq!(
                stated.code, VALIDATION_EXCEPTION,
                "case {case:?}: expected the modelled ValidationException code, got {:?}",
                stated.code
            );
            assert_eq!(
                stated.message, expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                stated.message
            );
        }
    }

    /// Several clauses are joined by `"; "` in the reverse of the order the walk reports
    /// them — the last-declared refused member first, whatever the constraint classes are.
    #[test]
    fn several_clauses_are_joined_in_reverse_declaration_order() {
        // The walk reports violations in the input shape's member declaration order; each
        // row is one such sequence, with the members the message must name in order.
        let rows: Vec<(&str, Vec<Violation>, &str)> = vec![
            (
                "two malformed ARNs, as ListTagsForResource declares them",
                vec![
                    violation(
                        "instanceArn",
                        ViolationKind::Pattern(INSTANCE_ARN_PATTERN.to_owned()),
                        Some("not-an-arn"),
                    ),
                    violation(
                        "resourceArn",
                        ViolationKind::Pattern(r"^arn:aws:sso::.*$".to_owned()),
                        Some("not-an-arn"),
                    ),
                ],
                "resourceArn|instanceArn",
            ),
            (
                "three members of mixed constraint classes",
                vec![
                    violation("instanceArn", ViolationKind::MissingRequiredMember, None),
                    violation("name", ViolationKind::MaxLength(32), Some("x")),
                    violation(
                        "maxResults",
                        ViolationKind::MaxValue("100".to_owned()),
                        Some("1000"),
                    ),
                ],
                "maxResults|name|instanceArn",
            ),
            (
                "one member only",
                vec![violation(
                    "resourceArn",
                    ViolationKind::MissingRequiredMember,
                    None,
                )],
                "resourceArn",
            ),
        ];

        for (case, walked, expected) in rows {
            let message = stated(walked).message;
            let named: Vec<&str> = message
                .split(CLAUSE_JOINER)
                .map(|clause| {
                    clause
                        .trim_start_matches("Value of input '")
                        .split('\'')
                        .next()
                        .unwrap_or_default()
                })
                .collect();
            assert_eq!(
                named.join("|"),
                expected,
                "case {case:?}: expected the clauses to name {expected:?}, got {message:?}"
            );
        }
    }

    /// How a `@pattern` is quoted, over the forms a model writes: a wrapping pair is
    /// dropped, and anything else — one anchor alone, branches carrying their own, an
    /// escaped `$` — is published exactly as the model wrote it.
    #[test]
    fn a_pattern_is_quoted_as_the_service_publishes_it() {
        // (case, the model's pattern, the expression the clause must state)
        let rows: [(&str, &str, &str); 7] = [
            (
                "a wrapping pair, as every ARN member of this model carries",
                INSTANCE_ARN_PATTERN,
                r"arn:aws(-[a-z]{1,5}){0,3}:sso:::instance/(sso)?ins-[a-zA-Z0-9-.]{16}",
            ),
            (
                // The model's own `Token` pattern, which a `NextToken` carries.
                "a wrapping pair around a character class",
                r"^[-a-zA-Z0-9+=/_]*$",
                r"[-a-zA-Z0-9+=/_]*",
            ),
            ("a leading anchor only", r"^arn:aws.*", r"^arn:aws.*"),
            ("a trailing anchor only", r"[0-9]{12}$", r"[0-9]{12}$"),
            ("no anchor at all", r"[0-9]{12}", r"[0-9]{12}"),
            (
                "branches carrying the anchors themselves",
                r"(^a$)|(^b$)",
                r"(^a$)|(^b$)",
            ),
            (
                // An escaped `$` is a literal dollar the value must carry, not an anchor.
                "an escaped trailing dollar",
                r"^[a-z]+\$",
                r"^[a-z]+\$",
            ),
        ];

        for (case, pattern, expected) in rows {
            let stated = stated(vec![violation(
                "name",
                ViolationKind::Pattern(pattern.to_owned()),
                Some("x"),
            )])
            .message;
            let reported = stated
                .rsplit("regular expression pattern: ")
                .next()
                .unwrap_or_default();
            assert_eq!(
                reported, expected,
                "case {case:?}: expected {pattern:?} to be stated as {expected:?}, got \
                 {reported:?}"
            );
        }
    }

    /// A refusal that is not a constraint violation, and one with nothing to state, keep
    /// the framework's own wording.
    #[test]
    fn a_refusal_this_service_does_not_word_keeps_the_frameworks_wording() {        assert!(
            refused(&Refusal::Unauthenticated).is_none(),
            "expected an unauthenticated caller to keep the framework's 403 wording"
        );
        assert!(
            refused(&Refusal::Invalid(Vec::new())).is_none(),
            "expected no violations to state nothing"
        );
    }

    /// Every error body this service can answer with reports its detail text in
    /// `Message` — the modelled failures a handler returns already do, and the two the
    /// framework renders itself are renamed onto it.
    #[test]
    fn every_error_body_reports_its_detail_text_in_the_message_member() {
        for (code, detail) in [
            ("ValidationException", "Value of input 'instanceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws:sso:::instance/ins-x"),
            ("AccessDeniedException", "no instance of this account carries that ARN"),
            ("ConflictException", "the instance is being deleted"),
            ("InternalServerException", "the SSO Admin state store is unavailable"),
            ("ResourceNotFoundException", "no permission set with that ARN"),
            ("ServiceQuotaExceededException", "this account already has an instance"),
            ("ThrottlingException", "slow down"),
            // The framework's own signal, rendered by the scaffold channel rather than by
            // a handler.
            ("NotImplemented", "operation ListTagsForResource is scaffolded but not implemented in this emulator"),
        ] {
            let body = format!(r#"{{"__type":"{code}","message":"{detail}"}}"#);
            let rewritten = with_detail_member(body.as_bytes())
                .unwrap_or_else(|| panic!("case {code:?}: expected a rename, got no rewrite"));
            let document: Value =
                serde_json::from_slice(&rewritten).expect("the rewritten body is JSON");
            assert_eq!(
                document.get(DETAIL_MEMBER).and_then(Value::as_str),
                Some(detail),
                "case {code:?}: expected the detail text in {DETAIL_MEMBER}, got {document}"
            );
            assert!(
                document.get(RENDERED_DETAIL_MEMBER).is_none(),
                "case {code:?}: the body must carry the text under ONE key, got {document}"
            );
            assert_eq!(
                document.get("__type").and_then(Value::as_str),
                Some(code),
                "case {code:?}: the discriminator must survive, got {document}"
            );
        }
    }

    /// Nothing to rename: a body with no rendered detail member, one that already spells
    /// it the way this service does, one that is not an object, and one that is not JSON.
    #[test]
    fn a_body_with_nothing_to_rename_is_left_alone() {
        for (case, body) in [
            ("no detail member", r#"{"__type":"ThrottlingException"}"#),
            (
                "already renamed",
                r#"{"__type":"ValidationException","Message":"stated"}"#,
            ),
            ("not an object", r#"["ValidationException"]"#),
            ("not JSON", "<Error/>"),
        ] {
            assert!(
                with_detail_member(body.as_bytes()).is_none(),
                "case {case:?}: expected no rewrite"
            );
        }
    }

    /// A body carrying BOTH spellings keeps the one the error shape modelled.
    #[test]
    fn a_modelled_message_member_wins_over_the_renamed_one() {
        let body = r#"{"__type":"ValidationException","Message":"modelled","message":"rendered"}"#;
        let rewritten = with_detail_member(body.as_bytes()).expect("a rewrite");
        let document: Value =
            serde_json::from_slice(&rewritten).expect("the rewritten body is JSON");
        assert_eq!(
            document.get(DETAIL_MEMBER).and_then(Value::as_str),
            Some("modelled"),
            "the modelled member must survive, got {document}"
        );
        assert!(
            document.get(RENDERED_DETAIL_MEMBER).is_none(),
            "the rendered member must not remain beside it, got {document}"
        );
    }

    /// A successful response is not an error body: the rename never reaches it.
    #[test]
    fn a_success_is_returned_untouched() {
        let body = r#"{"Instances":[{"Status":"ACTIVE","message":"not an error"}]}"#;
        let resp = RtResponse::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(200u16).expect("200"),
            SdkBody::from(body),
        );
        let answered = with_service_detail_member(resp);
        assert_eq!(
            answered.body().bytes().expect("bytes"),
            body.as_bytes(),
            "a success body must be byte-identical"
        );
    }
}
