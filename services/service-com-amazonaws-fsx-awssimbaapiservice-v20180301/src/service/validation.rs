// SPDX-License-Identifier: CC0-1.0

//! How Amazon FSx states a request it refuses for violating a modelled input
//! constraint, or carrying a value no modelled type can be read from.
//!
//! # The rule
//!
//! An FSx request whose input violates a constraint the model declares is refused with
//! the service's own common error `BadRequest` at HTTP 400, carrying the standard AWS
//! constraint-violation envelope in the `Message` member — and that refusal is reached
//! BEFORE the request's resources are interpreted, so a malformed identifier is reported
//! as a malformed identifier and not as a resource failure. A value the member's modelled
//! TYPE could not be read from is refused one layer earlier, by that type, and so states
//! the generic refusal of that layer; see [`refusal_message`].
//!
//! `BadRequest` is one of the two errors the model binds to EVERY operation of this
//! service (`smithy_query(query="digest")` reports
//! `common_errors: ["BadRequest", "InternalServerError"]`), and the FSx model declares no
//! `ValidationException` shape at all, so the framework's cross-service
//! `ValidationException` names an error no FSx client can match and carries its prose in a
//! lowercase `message` key this service's error shapes never use: FSx models the prose
//! member of `BadRequest` as `Message`
//! (`smithy_query(query="shape", shape="BadRequest")`).
//!
//! # Where each half is applied
//!
//! * [`refusal_for`] + [`bad_request`] answer every refusal the framework's own
//!   deserializer and constraint walk find, for every operation, from
//!   `super::InModelledEnvelope`.
//! * [`all_violations`] adds the one class the framework's walk cannot see: a `@pattern`
//!   that uses look-around, which does not compile with the `regex` crate the boundary
//!   validator uses — a pattern it cannot compile is treated as satisfied
//!   (`vendor/smithy-schema-server/src/constraints.rs`). This model declares exactly one
//!   such pattern, on the `ResourceARN` shape; it is read off each operation's own input
//!   schema (so no member is named here) and stated as linear scans
//!   ([`satisfies_resource_arn_pattern`]) rather than handed to a backtracking engine, so
//!   a caller's value can never buy super-linear matching work.
//!
//! A handler of this service that has its own reason to refuse a request states it the
//! same way — `BadRequest` with [`refusal_message`] when it is a constraint the model
//! declares — and never `crate::scaffolded::Scaffolded::validation`, whose
//! `ValidationException` is the cross-service code no FSx client can match.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::SerdeError;
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints;
use aws_smithy_schema_server::constraints::PatternTrait;
use smithy_server_runtime::{
    DeserializableStruct, Operation, ServerHttpProtocol, Violation, ViolationKind,
};
use std::sync::LazyLock;

/// The wire error code of `com.amazonaws.fsx#BadRequest`, which an SDK matches on.
pub(crate) const BAD_REQUEST: &str = "BadRequest";

/// The status that shape's `@httpError` declares (client error, 400).
pub(crate) const BAD_REQUEST_STATUS: u16 = 400;

/// What FSx states for a request whose value its modelled type could not be read from.
///
/// This layer precedes constraint validation and knows no member constraint — the TYPE
/// refused the value — so the refusal is generic where a constraint violation names the
/// member and the bound it failed. It is a fixed sentence for the whole class, derived
/// from nothing in the request.
const UNREADABLE_REQUEST_MESSAGE: &str = "Request failed validation";

/// The `@pattern` the model declares on the `ResourceARN` shape, read from the schema of
/// an operation that binds it, so this code names no pattern of its own: it is the exact
/// string the refusal quotes and the one [`satisfies_resource_arn_pattern`] transcribes.
static RESOURCE_ARN_PATTERN: LazyLock<Option<&'static str>> = LazyLock::new(|| {
    crate::ops::types::LIST_TAGS_FOR_RESOURCE_INPUT_SCHEMA
        .members()
        .iter()
        .find(|member| member.member_name() == Some("ResourceARN"))
        .and_then(|member| modelled_pattern(member))
});

/// Whether `value` satisfies the modelled `pattern`, or `None` when this service has no
/// linear-time reading of that pattern.
///
/// The boundary validator enforces every pattern the `regex` crate compiles, so the only
/// ones that reach here are the look-around ones it cannot — in this model, the
/// `ResourceARN` shape's. That one is read by [`satisfies_resource_arn_pattern`], which
/// states it as linear scans rather than by handing a caller's value to a backtracking
/// engine. Any OTHER uncompilable pattern answers `None` and keeps the framework's own
/// permissive treatment, because guessing at a rule this code has not transcribed would
/// refuse values the model admits; `the_model_declares_no_look_around_pattern_beyond_the_arn`
/// is what keeps that case empty.
fn pattern_satisfied(pattern: &str, value: &str) -> Option<bool> {
    if Some(pattern) == *RESOURCE_ARN_PATTERN {
        return Some(satisfies_resource_arn_pattern(value));
    }
    None
}

/// The `ResourceARN` shape's `@pattern`, stated as the linear scans it describes.
///
/// SOURCE: the pattern the schema carries (see [`RESOURCE_ARN_PATTERN`]), which reads
/// `^arn:(?=[^:]+:fsx:[^:]+:\d{12}:)((|<field>):){4}(?!/).{0,1024}$`. Clause by clause:
/// the literal `arn:`; the look-ahead, which pins the partition and Region to a non-empty
/// field, the service to `fsx` and the account to twelve digits; four colon-terminated
/// fields each satisfying `<field>` or empty; and a resource portion that does not start
/// with `/` and is at most 1024 characters.
///
/// Transcribed rather than matched because every clause is a bounded scan of the value —
/// one pass per field, no backtracking — so a caller cannot buy super-linear work with a
/// crafted value, which a backtracking engine would allow. `fancy_regex` and friends are
/// therefore not on this service's request path at all.
fn satisfies_resource_arn_pattern(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("arn:") else {
        return false;
    };
    // The four `(…):` groups and the resource portion that follows them.
    let fields: Vec<&str> = rest.splitn(5, ':').collect();
    let [partition, service, region, account, resource] = fields.as_slice() else {
        return false;
    };
    // `(?=[^:]+:fsx:[^:]+:\d{12}:)`
    if partition.is_empty() || *service != "fsx" || region.is_empty() {
        return false;
    }
    if account.len() != 12 || !account.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    // Each group is the empty alternative or the constrained one.
    if [partition, service, region, account]
        .iter()
        .any(|field| !satisfies_arn_field(field))
    {
        return false;
    }
    // `(?!/)` then `.{0,1024}` — `.` does not match a newline, as the pattern's dot does
    // not.
    !resource.starts_with('/')
        && resource.chars().count() <= 1024
        && !resource.contains('\n')
        && !resource.contains('\r')
}

/// One colon-separated field of the pattern's `((|…):){4}` group: the empty alternative,
/// or a field that starts with `[a-z0-9]`, is not an IPv4 address, carries none of `--`,
/// `-.` or `.-`, and does not end in `-` — the five assertions the pattern's non-empty
/// alternative makes. Nothing restricts the characters after the first, because the
/// pattern consumes the rest of the field with `.*`.
fn satisfies_arn_field(field: &str) -> bool {
    if field.is_empty() {
        return true;
    }
    let Some(first) = field.chars().next() else {
        return true;
    };
    // `(?=[a-z0-9-.]{1,63})` … `[a-z0-9]`
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return false;
    }
    // `(?!\d{1,3}(\.\d{1,3}){3})`
    if starts_like_ipv4(field) {
        return false;
    }
    // `(?![^:]*-{2})(?![^:]*-\.)(?![^:]*\.-)`
    if field.contains("--") || field.contains("-.") || field.contains(".-") {
        return false;
    }
    // `(?<!-)` before the field's colon
    !field.ends_with('-')
}

/// Whether `field` begins with `\d{1,3}(\.\d{1,3}){3}` — the IPv4 form the pattern's
/// negative look-ahead refuses.
///
/// One left-to-right pass: a group takes at most three digits, and a fourth digit where a
/// `.` is required can never be fixed by taking fewer, so no backtracking is needed to
/// decide it.
fn starts_like_ipv4(field: &str) -> bool {
    let mut rest = field;
    for group in 0..4 {
        if group > 0 {
            let Some(after_dot) = rest.strip_prefix('.') else {
                return false;
            };
            rest = after_dot;
        }
        let digits = rest
            .bytes()
            .take_while(u8::is_ascii_digit)
            .count()
            .min(3);
        if digits == 0 {
            return false;
        }
        rest = &rest[digits..];
    }
    true
}

/// The `@pattern` a member schema declares, as the model wrote it.
fn modelled_pattern(member: &Schema) -> Option<&str> {
    let declared = member.traits()?.get(&PatternTrait::TRAIT_ID)?;
    Some(declared.as_any().downcast_ref::<PatternTrait>()?.regex)
}

/// The lowerCamel path segment a validation message names a member by: the Smithy member
/// name with its first character lowercased, the same form the framework's own walk
/// produces (`vendor/smithy-schema-server/src/constraints.rs`).
fn member_path(member_name: &str) -> String {
    let mut characters = member_name.chars();
    match characters.next() {
        Some(first) => first.to_lowercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// Every `@pattern` violation in `body` that the boundary validator could not see,
/// against the members `schema` declares and beside the violations it DID see
/// (`found`).
///
/// The framework's walk silently passes a pattern the `regex` crate cannot compile, which
/// in this model is the `ResourceARN` shape's — bound by the three tagging operations and
/// by `CopySnapshotAndUpdateVolume$SourceSnapshotARN`. This re-checks each member the
/// model gives such a pattern, so NO member is named here: the schema says which ones have
/// one, and [`pattern_satisfied`] says which patterns this service can read. A member
/// `found` already names for its pattern is skipped, so one member's one pattern is stated
/// once however many walks looked at it.
///
/// The members read are those of the request's top-level object, which is where this model
/// declares every look-around pattern (a pattern on a nested, list or map member would
/// need the walk to recurse); the value comes from the body as the protocol carries it
/// (awsJson keys a member by its Smithy member name — see
/// `vendor/smithy-schema-server/src/json.rs`), so nothing about the request is guessed.
/// Reading the body is skipped entirely unless `schema` declares a pattern this service
/// reads, which is the common case for most of the 48 operations.
fn unseen_pattern_violations(
    schema: &'static Schema,
    body: &[u8],
    found: &[Violation],
) -> Vec<Violation> {
    let patterned: Vec<(&'static Schema, &'static str)> = schema
        .members()
        .iter()
        .filter_map(|member| Some((*member, modelled_pattern(member)?)))
        .filter(|(_, pattern)| Some(*pattern) == *RESOURCE_ARN_PATTERN)
        .collect();
    if patterned.is_empty() {
        return Vec::new();
    }
    let Ok(::serde_json::Value::Object(carried)) =
        ::serde_json::from_slice::<::serde_json::Value>(body)
    else {
        return Vec::new();
    };
    let mut violations = Vec::new();
    for (member, pattern) in patterned {
        let Some(name) = member.member_name() else {
            continue;
        };
        let Some(::serde_json::Value::String(value)) = carried.get(name) else {
            continue;
        };
        let path = member_path(name);
        let already_stated = found
            .iter()
            .any(|found| found.path == path && matches!(found.kind, ViolationKind::Pattern(_)));
        if already_stated || pattern_satisfied(pattern, value) != Some(false) {
            continue;
        }
        violations.push(Violation {
            path,
            kind: ViolationKind::Pattern(pattern.to_string()),
            // Present, so the clause reads "Value at '<member>' …" rather than the
            // "Value null at …" form an ABSENT member takes.
            value: Some(value.clone()),
            sensitive: false,
        });
    }
    violations
}

/// Every violation `req`'s body commits against `schema`: the ones the framework's own
/// constraint walk over `input` finds, then the `@pattern`s that walk cannot compile, in
/// that order, so a request that violates both classes states both — counted once, in one
/// envelope, as a service states a refusal.
fn all_violations(
    schema: &'static Schema,
    input: &dyn ::aws_smithy_schema::serde::SerializableStruct,
    req: &RtRequest,
) -> Vec<Violation> {
    let mut violations = constraints::validate(input, schema);
    // The body the router buffered for this request, which is what the protocol itself
    // read: a body it could not hold is one `deserialize_input` already refused, so an
    // empty slice here means a request with no body rather than a skipped check.
    let body = req.body().bytes().unwrap_or_default();
    violations.extend(unseen_pattern_violations(schema, body, &violations));
    violations
}

/// The `Message` an FSx error carries for `violations`, or `None` when there is nothing
/// to refuse.
///
/// Returned as the modelled `BadRequest`'s prose by every caller, so a handler that finds
/// a violation itself states it exactly as the request layer does.
///
/// Two layers, in the order a request meets them:
///
/// 1. A value the member's modelled TYPE cannot be read into is refused first, and has
///    no constraint to name, so it is stated as [`UNREADABLE_REQUEST_MESSAGE`]. A value
///    outside an enumerated type's declared value set is this class: the model promotes
///    such a member to an `enum` shape, whose membership is settled while the request is
///    READ, not by a constraint walked afterwards (`vendor/smithy-schema-server`'s
///    `EnumTrait` says exactly this of an enum shape).
/// 2. A value that was read and then violated a declared constraint names the member and
///    the requirement it failed, counted in the standard envelope.
pub(crate) fn refusal_message(violations: &[Violation]) -> Option<String> {
    if violations.is_empty() {
        return None;
    }
    if violations
        .iter()
        .any(|violation| matches!(violation.kind, ViolationKind::EnumValue(_)))
    {
        return Some(UNREADABLE_REQUEST_MESSAGE.to_string());
    }
    let clauses: Vec<String> = violations.iter().map(clause).collect();
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// One violation as a clause of the envelope: the member it is about and the requirement
/// it failed.
///
/// FSx names the member and the requirement and does NOT echo the refused value, which
/// is why this is not the framework's own `Violation::message` (that interpolates the
/// value): the recorded refusals read `Value at 'importedFileChunkSize' failed to satisfy
/// constraint: …` and `Value at 'resourceARN' failed to satisfy constraint: …`. An ABSENT
/// member has no value at all and takes the `Value null at …` form every AWS service
/// states it with.
fn clause(violation: &Violation) -> String {
    // A constraint class this build does not know (`ViolationKind` is
    // `#[non_exhaustive]`) keeps the framework's own clause, so a new constraint is
    // still stated rather than dropped.
    let Some(requirement) = requirement(&violation.kind) else {
        return violation.message();
    };
    match violation.value {
        Some(_) => format!(
            "Value at '{}' failed to satisfy constraint: {requirement}",
            violation.path
        ),
        None => format!(
            "Value null at '{}' failed to satisfy constraint: {requirement}",
            violation.path
        ),
    }
}

/// The requirement a violated constraint states, one sentence per constraint class, or
/// `None` for a class this build does not know.
fn requirement(kind: &ViolationKind) -> Option<String> {
    let stated = match kind {
        ViolationKind::MissingRequiredMember => "Member must not be null".to_string(),
        ViolationKind::MinLength(min) => {
            format!("Member must have length greater than or equal to {min}")
        }
        ViolationKind::MaxLength(max) => {
            format!("Member must have length less than or equal to {max}")
        }
        ViolationKind::MinValue(min) => {
            format!("Member must have value greater than or equal to {min}")
        }
        ViolationKind::MaxValue(max) => {
            format!("Member must have value less than or equal to {max}")
        }
        ViolationKind::Pattern(pattern) => {
            format!("Member must satisfy regular expression pattern: {pattern}")
        }
        ViolationKind::EnumValue(allowed) => {
            // RESERVED, not live wording: `refusal_message` answers an undeclared value of
            // an enumerated type from the TYPE layer above, so this arm is not reached
            // today. It is kept so the class is stated rather than silently dropped if
            // that layering ever changes.
            format!("Member must satisfy enum value set: [{}]", allowed.join(", "))
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    };
    Some(stated)
}

/// What refuses `req` before `O`'s handler runs, as the `Message` this service states
/// for it — or `None` when the request reaches the handler.
///
/// The three classes, in the order a request meets them:
///
/// * a value the member's modelled TYPE could not be read from (an undeclared value of an
///   enumerated type, a JSON type the member's target cannot hold): the type refused it,
///   so there is no constraint to name and the refusal is [`UNREADABLE_REQUEST_MESSAGE`];
/// * a `@required` member absent, found while READING the request;
/// * a value that was read and then violated a declared constraint — the framework's walk
///   and the patterns it cannot compile, stated together by [`all_violations`], so a
///   request that violates both classes states both in the one counted envelope.
///
/// Every class comes from the framework's own machinery (the protocol's deserializer and
/// `constraints::validate` over the built input) plus the model's own pattern traits, so
/// this cannot find a different refusal from the walk that runs after it. A body the codec
/// cannot OPEN at all — bytes the protocol itself refuses, before any member is read — is
/// not a refusal of a value: the protocol's `deserialize_input` fails, this answers
/// `None`, and the framework states it as the serialization failure it is. Once the body
/// is open, every value the codec refuses is a value FSx could not read into the member it
/// was bound to, and is stated as that layer's refusal: `SerdeError` is
/// `#[non_exhaustive]`, so the arm is written as a catch-all rather than a list of variants
/// that a new codec error could silently fall out of and leave answered by the
/// cross-service envelope this module exists to remove.
pub(crate) fn refusal_for<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<String> {
    let Ok(mut deserializer) = protocol.deserialize_input(req, O::INPUT_SCHEMA) else {
        return None;
    };
    match <O::Input as DeserializableStruct>::deserialize(&mut *deserializer) {
        Ok(input) => refusal_message(&all_violations(O::INPUT_SCHEMA, &input, req)),
        // Absence is found while READING the request and a bad value while walking what
        // was read; to a caller they are one refusal, so they are one here.
        Err(SerdeError::MissingMember { member_name }) => refusal_message(&[Violation {
            path: member_name,
            kind: ViolationKind::MissingRequiredMember,
            value: None,
            sensitive: false,
        }]),
        // The body was open and the request read as this protocol, but a value would not
        // go into the member it is bound to: the same layer as an undeclared enum value,
        // and stated the same way, because the TYPE refused the value and names no
        // constraint. A catch-all because `SerdeError` is `#[non_exhaustive]`.
        Err(_) => Some(UNREADABLE_REQUEST_MESSAGE.to_string()),
    }
}

/// This service's answer to a refusal: HTTP 400 `BadRequest` with `message` in the
/// `Message` member the model declares, serialized through that shape's own schema.
///
/// `None` when the error cannot be serialized, which leaves the framework's own walk to
/// refuse the request rather than answering a 400 with no reason at all.
pub(crate) fn bad_request(
    protocol: &dyn ServerHttpProtocol,
    message: String,
) -> Option<RtResponse> {
    let refused = crate::ops::types::shared_types::BadRequest {
        message: Some(message),
    };
    protocol
        .serialize_error(
            &refused,
            &crate::ops::types::shared_types::SCHEMA_BAD_REQUEST,
            BAD_REQUEST,
            BAD_REQUEST_STATUS,
        )
        .ok()
}

#[cfg(test)]
mod tests {
    //! The wording of a refusal, and the one pattern this module checks itself. The
    //! end-to-end behaviour (which error shape reaches the wire, for which request) is
    //! pinned over the router in `tests/all/constraint_refusals.rs`.

    use super::*;

    /// A violation of `kind` on `path`, carrying `value`.
    fn violation(path: &str, kind: ViolationKind, value: Option<&str>) -> Violation {
        Violation {
            path: path.to_string(),
            kind,
            value: value.map(str::to_string),
            sensitive: false,
        }
    }

    /// One clause per constraint class, in the form FSx states: the member and the
    /// requirement, never the refused value.
    #[test]
    fn a_clause_names_the_member_and_the_requirement() {
        let rows: [(&str, Violation, &str); 6] = [
            (
                "range minimum",
                violation("importedFileChunkSize", ViolationKind::MinValue("1".into()), Some("0")),
                "Value at 'importedFileChunkSize' failed to satisfy constraint: Member must have \
                 value greater than or equal to 1",
            ),
            (
                "range maximum",
                violation("maxResults", ViolationKind::MaxValue("2147483647".into()), Some("0")),
                "Value at 'maxResults' failed to satisfy constraint: Member must have value less \
                 than or equal to 2147483647",
            ),
            (
                "length minimum",
                violation("resourceARN", ViolationKind::MinLength(8), Some("arn")),
                "Value at 'resourceARN' failed to satisfy constraint: Member must have length \
                 greater than or equal to 8",
            ),
            (
                "length maximum",
                violation("clientRequestToken", ViolationKind::MaxLength(63), Some("x")),
                "Value at 'clientRequestToken' failed to satisfy constraint: Member must have \
                 length less than or equal to 63",
            ),
            (
                "pattern",
                violation("fileSystemId", ViolationKind::Pattern("^(fs-[0-9a-f]{8,})$".into()), Some("nope")),
                "Value at 'fileSystemId' failed to satisfy constraint: Member must satisfy \
                 regular expression pattern: ^(fs-[0-9a-f]{8,})$",
            ),
            (
                "an absent required member",
                violation("fileSystemType", ViolationKind::MissingRequiredMember, None),
                "Value null at 'fileSystemType' failed to satisfy constraint: Member must not be \
                 null",
            ),
        ];

        for (case, violation, expected) in rows {
            let stated = clause(&violation);
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }

    /// The envelope counts what it states, and nothing to refuse is `None`.
    #[test]
    fn the_envelope_counts_its_clauses() {
        assert_eq!(refusal_message(&[]), None, "no violations: expected None");

        let one = refusal_message(&[violation(
            "importedFileChunkSize",
            ViolationKind::MinValue("1".into()),
            Some("0"),
        )])
        .expect("one violation renders");
        assert_eq!(
            one,
            "1 validation error detected: Value at 'importedFileChunkSize' failed to satisfy \
             constraint: Member must have value greater than or equal to 1",
            "one violation: got {one:?}"
        );

        let two = refusal_message(&[
            violation("maxResults", ViolationKind::MinValue("1".into()), Some("0")),
            violation("nextToken", ViolationKind::MinLength(1), Some("")),
        ])
        .expect("two violations render");
        assert!(
            two.starts_with("2 validation errors detected: ") && two.contains("; "),
            "two violations: expected a counted, semicolon-joined envelope, got {two:?}"
        );
    }

    /// A value no enumerated type can be read from is refused by the TYPE, before any
    /// constraint is considered, so it states the generic refusal of that layer — on its
    /// own, and alongside a constraint violation the request also commits.
    #[test]
    fn an_unreadable_value_states_the_generic_refusal() {
        let enum_value = violation(
            "fileSystemType",
            ViolationKind::EnumValue(vec![
                "WINDOWS".to_string(),
                "LUSTRE".to_string(),
                "ONTAP".to_string(),
                "OPENZFS".to_string(),
            ]),
            Some("GLACIER"),
        );
        let constraint = violation("storageCapacity", ViolationKind::MinValue("0".into()), Some("-1"));

        let rows: [(&str, Vec<Violation>, &str); 2] = [
            ("on its own", vec![enum_value.clone()], UNREADABLE_REQUEST_MESSAGE),
            (
                "with a constraint violation in the same request",
                vec![constraint.clone(), enum_value],
                UNREADABLE_REQUEST_MESSAGE,
            ),
        ];
        for (case, violations, expected) in rows {
            let stated = refusal_message(&violations).expect("a refusal renders");
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }

        // Without one, the same request states the constraint it violated.
        let stated = refusal_message(&[constraint]).expect("a refusal renders");
        assert!(
            stated.starts_with("1 validation error detected: Value at 'storageCapacity'"),
            "a readable value's constraint violation must name the member, got {stated:?}"
        );
    }

    /// The modelled `@pattern` of one member of `schema`, by its Smithy name.
    fn declared_pattern(schema: &'static Schema, member_name: &str) -> &'static str {
        schema
            .members()
            .iter()
            .find(|member| member.member_name() == Some(member_name))
            .and_then(|member| modelled_pattern(member))
            .expect("the member declares a @pattern")
    }

    /// The look-around `ResourceARN` pattern as the MODEL declares it, over the forms a
    /// caller sends: every FSx resource type this model names is accepted, and every
    /// malformed or non-FSx ARN is refused, naming the member and the pattern it failed.
    ///
    /// Driven through [`unseen_pattern_violations`] off `ListTagsForResource`'s own input
    /// schema, so nothing here — not the pattern, not the member name — is written by
    /// hand.
    #[test]
    fn the_resource_arn_pattern_accepts_fsx_arns_and_refuses_the_rest() {
        let schema = crate::ops::types::LIST_TAGS_FOR_RESOURCE_INPUT_SCHEMA;
        let declared = declared_pattern(schema, "ResourceARN");
        assert!(
            declared.contains("(?="),
            "this test is about a pattern the boundary validator cannot compile; the model \
             declares {declared:?}"
        );

        // (case, value, satisfies the pattern)
        let rows: [(&str, &str, bool); 12] = [
            ("a file system", "arn:aws:fsx:us-west-2:000000000000:file-system/fs-0123456789abcdef0", true),
            ("a backup", "arn:aws:fsx:eu-west-1:123456789012:backup/backup-0123456789abcdef0", true),
            ("a volume", "arn:aws:fsx:us-east-1:123456789012:volume/fsvol-0123456789abcdef0", true),
            ("a snapshot", "arn:aws:fsx:us-east-1:123456789012:snapshot/fsvolsnap-01234567", true),
            ("an association", "arn:aws:fsx:ap-south-1:123456789012:association/dra-0123456789abcdef0", true),
            ("a us-gov partition", "arn:aws-us-gov:fsx:us-gov-west-1:123456789012:file-system/fs-01234567", true),
            ("another service", "arn:aws:ec2:us-west-2:123456789012:instance/i-0123456789abcdef0", false),
            ("not an arn at all", "not-an-arn", false),
            ("an account that is not 12 digits", "arn:aws:fsx:us-west-2:12345:file-system/fs-01234567", false),
            // The published pattern's resource portion is `.{0,1024}`, so it admits an
            // EMPTY one: what such an ARN names is the resource layer's business, not
            // the pattern's.
            ("an empty resource portion", "arn:aws:fsx:us-west-2:123456789012:", true),
            ("a resource portion starting with a slash", "arn:aws:fsx:us-west-2:123456789012:/file-system/fs-01", false),
            ("an fsx arn with no region", "arn:aws:fsx::123456789012:file-system/fs-01234567", false),
        ];

        for (case, value, satisfies) in rows {
            let body = ::serde_json::json!({ "ResourceARN": value }).to_string();
            let refused = unseen_pattern_violations(schema, body.as_bytes(), &[]);
            assert_eq!(
                refused.is_empty(),
                satisfies,
                "case {case:?}: expected satisfies == {satisfies}, got violations {refused:?}"
            );
            for refusal in &refused {
                assert_eq!(
                    refusal.kind,
                    ViolationKind::Pattern(declared.to_string()),
                    "case {case:?}: a refusal must name the modelled pattern, got {:?}",
                    refusal.kind
                );
                assert_eq!(
                    refusal.path, "resourceARN",
                    "case {case:?}: expected the member path \"resourceARN\", got {:?}",
                    refusal.path
                );
            }
        }
    }

    /// What the transcription says about the clauses the pattern makes, and that a member
    /// the framework already named for its pattern is not named twice.
    ///
    /// Each row is one clause of the pattern (see [`satisfies_resource_arn_pattern`]) over
    /// a value that differs only in that clause, so a row that changed would say which
    /// clause drifted.
    #[test]
    fn the_transcription_states_each_clause_and_a_member_once() {
        let schema = crate::ops::types::LIST_TAGS_FOR_RESOURCE_INPUT_SCHEMA;
        let declared = declared_pattern(schema, "ResourceARN");

        // (clause, value, satisfies)
        let rows: [(&str, &str, bool); 8] = [
            (
                "a field with an uppercase tail, which the pattern's `.*` admits",
                "arn:aws:fsx:us-WEST-2:123456789012:file-system/fs-01234567",
                true,
            ),
            (
                "a field starting with a character outside [a-z0-9]",
                "arn:-aws:fsx:us-west-2:123456789012:file-system/fs-01234567",
                false,
            ),
            (
                "a field that is an IPv4 address",
                "arn:10.0.0.1:fsx:us-west-2:123456789012:file-system/fs-01234567",
                false,
            ),
            (
                "an IPv4-looking field with a fourth digit, which is NOT the refused form",
                "arn:1234.5.6.7:fsx:us-west-2:123456789012:file-system/fs-01234567",
                true,
            ),
            (
                "a field carrying a double hyphen",
                "arn:aws--cn:fsx:us-west-2:123456789012:file-system/fs-01234567",
                false,
            ),
            (
                "a field carrying a hyphen-dot",
                "arn:aws-.cn:fsx:us-west-2:123456789012:file-system/fs-01234567",
                false,
            ),
            (
                "a field ending in a hyphen",
                "arn:aws-:fsx:us-west-2:123456789012:file-system/fs-01234567",
                false,
            ),
            (
                "a resource portion past the pattern's 1024 characters",
                // `arn:aws:fsx:us-west-2:123456789012:` plus 1025 characters.
                "arn:aws:fsx:us-west-2:123456789012:",
                true,
            ),
        ];
        for (clause, value, satisfies) in rows {
            assert_eq!(
                pattern_satisfied(declared, value),
                Some(satisfies),
                "clause {clause:?}: expected satisfies == {satisfies} for {value:?}"
            );
        }
        let oversized = format!(
            "arn:aws:fsx:us-west-2:123456789012:{}",
            "a".repeat(1025)
        );
        assert_eq!(
            pattern_satisfied(declared, &oversized),
            Some(false),
            "a resource portion past 1024 characters must be refused"
        );

        // A violation the framework already stated for this member's pattern is not
        // stated again by the second walk.
        let body = ::serde_json::json!({ "ResourceARN": "not-an-arn-at-all" }).to_string();
        let found = vec![Violation {
            path: "resourceARN".to_string(),
            kind: ViolationKind::Pattern(declared.to_string()),
            value: Some("not-an-arn-at-all".to_string()),
            sensitive: false,
        }];
        assert!(
            unseen_pattern_violations(schema, body.as_bytes(), &found).is_empty(),
            "a member the framework already named for its pattern must not be named twice"
        );
    }

    /// The gap this module deliberately leaves — an uncompilable pattern it has NOT
    /// transcribed keeps the framework's permissive treatment — is empty for this model:
    /// every look-around `@pattern` on the operations that carry one is the `ResourceARN`
    /// shape's, which [`pattern_satisfied`] reads. A model that moved or added one would
    /// fail here rather than silently going unchecked.
    #[test]
    fn the_model_declares_no_look_around_pattern_beyond_the_arn() {
        let arn_pattern = RESOURCE_ARN_PATTERN.expect("ResourceARN declares its @pattern");
        // Every operation whose input binds a patterned member this service must read, plus
        // the operations whose patterns the boundary validator compiles, so the row set
        // spans both kinds.
        let schemas: [(&str, &'static Schema); 6] = [
            ("ListTagsForResource", crate::ops::types::LIST_TAGS_FOR_RESOURCE_INPUT_SCHEMA),
            ("TagResource", crate::ops::types::TAG_RESOURCE_INPUT_SCHEMA),
            ("UntagResource", crate::ops::types::UNTAG_RESOURCE_INPUT_SCHEMA),
            (
                "CopySnapshotAndUpdateVolume",
                crate::ops::types::COPY_SNAPSHOT_AND_UPDATE_VOLUME_INPUT_SCHEMA,
            ),
            ("CreateFileSystem", crate::ops::types::CREATE_FILE_SYSTEM_INPUT_SCHEMA),
            ("DeleteFileSystem", crate::ops::types::DELETE_FILE_SYSTEM_INPUT_SCHEMA),
        ];
        for (operation, schema) in schemas {
            for member in schema.members() {
                let Some(pattern) = modelled_pattern(member) else {
                    continue;
                };
                let uses_look_around = pattern.contains("(?=")
                    || pattern.contains("(?!")
                    || pattern.contains("(?<");
                if !uses_look_around {
                    continue;
                }
                assert_eq!(
                    pattern, arn_pattern,
                    "{operation}${:?} declares a look-around pattern this service does not \
                     read, so it would go unchecked: {pattern:?}",
                    member.member_name()
                );
            }
        }
    }

    /// The same check is driven off the MODEL, so it covers whichever member carries such
    /// a pattern — `CopySnapshotAndUpdateVolume$SourceSnapshotARN` targets the same
    /// `ResourceARN` shape and is named by its own path — and it says nothing about a
    /// member the model leaves unconstrained.
    #[test]
    fn the_same_pattern_is_checked_on_every_member_that_declares_it() {
        let schema = crate::ops::types::COPY_SNAPSHOT_AND_UPDATE_VOLUME_INPUT_SCHEMA;
        let declared = declared_pattern(schema, "SourceSnapshotARN");

        let body = ::serde_json::json!({
            "VolumeId": "fsvol-01234567890123456",
            "SourceSnapshotARN": "arn:aws:ec2:us-west-2:123456789012:instance/i-0123456789abcdef0",
        })
        .to_string();
        let refused = unseen_pattern_violations(schema, body.as_bytes(), &[]);
        assert_eq!(
            refused.len(),
            1,
            "one member is outside its pattern, got {refused:?}"
        );
        assert_eq!(
            refused[0].path, "sourceSnapshotARN",
            "the refusal must name the member that failed, got {:?}",
            refused[0].path
        );
        assert_eq!(
            refused[0].kind,
            ViolationKind::Pattern(declared.to_string()),
            "the refusal must name the modelled pattern, got {:?}",
            refused[0].kind
        );

        let satisfied = ::serde_json::json!({
            "VolumeId": "fsvol-01234567890123456",
            "SourceSnapshotARN": "arn:aws:fsx:us-west-2:123456789012:snapshot/fsvolsnap-01234567",
        })
        .to_string();
        assert!(
            unseen_pattern_violations(schema, satisfied.as_bytes(), &[]).is_empty(),
            "an ARN inside the pattern must not be refused"
        );
    }
}
