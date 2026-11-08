// SPDX-License-Identifier: CC0-1.0

//! The SHAPE of the identifiers CodeBuild takes as plain strings.
//!
//! Most identifiers a request carries are constrained by the model — a fleet's own
//! `CreateFleetInput$name` targets `FleetName`, which carries the `@pattern` — so the
//! boundary validator refuses a malformed one before any handler runs. The batch and
//! per-sandbox operations do NOT: their identifier members target `NonEmptyString`
//! (`BatchGetFleetsInput$names`, `ListCommandExecutionsForSandboxInput$sandboxId`), so
//! the constraint walk passes anything non-empty and the rule belongs to the service.
//!
//! Each rule below is the SYNTAX question only — whether the value can name a resource
//! of that kind at all — which is what keeps the two absent-input classes apart: a
//! value that is not an identifier is refused with the modelled `InvalidInputException`
//! before any lookup, while a well-formed identifier this account holds nothing for is
//! answered from the store (an empty listing, or an entry in the not-found half of a
//! batch read).

use super::{is_arn, resource_from_arn};

/// The characters a RESOURCE NAME of this service is written with, from the `@pattern`
/// the model puts on each of them — `FleetName`
/// (`^[A-Za-z0-9][A-Za-z0-9\-_]{1,127}$`) and `ProjectName`
/// (`^[A-Za-z0-9][A-Za-z0-9\-_]{1,149}$`) allow the same set and differ only in how long
/// a name may be: ASCII letters and digits, dash and underscore.
///
/// One predicate for both, so the set a refusal names ([`INVALID_FLEET_NAME_MESSAGE`],
/// [`INVALID_PROJECT_NAME_MESSAGE`]) and the set that is checked cannot drift apart.
pub fn is_supported_name_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || character == '-' || character == '_'
}

/// What CodeBuild states as the reason a value handed to a fleet-addressing operation
/// cannot be a fleet name, carried by the modelled `InvalidInputException`.
///
/// Wholly fixed text — recorded behaviour, as the model states no message for the error:
/// the sentence names the character set that IS supported rather than quoting the
/// rejected value, so every malformed name is refused with the same words and a client
/// can key on them.
pub const INVALID_FLEET_NAME_MESSAGE: &str =
    "Invalid characters in fleet name. Only alphanumeric characters, dash, and underscore \
     are supported";

/// Why `identifier` cannot name a compute fleet, as the message the modelled
/// `InvalidInputException` carries — or `None` when it can.
///
/// `BatchGetFleetsInput$names` takes "the names or ARNs of the compute fleets", so an
/// identifier is one of two things: an ARN, whose own fields are checked by the resolver
/// that looks it up ([`super::ComAmazonawsCodebuildCodebuild20161006State::fleet_key`]),
/// or a fleet NAME, which has to be written with the characters a fleet name is written
/// with ([`is_supported_name_character`]). A name carrying anything else is no fleet name,
/// so there is nothing to look up and the request is refused rather than answered with
/// an empty result that would read as "this account holds no such fleet".
///
/// The CHARACTER SET is the whole of the rule stated here, because it is the whole of
/// what CodeBuild's recorded sentence for this refusal states
/// ([`INVALID_FLEET_NAME_MESSAGE`]). `FleetName`'s other two clauses — its 2..=128 length
/// and its leading character — are not refused here: a name that satisfies the character
/// set but not those simply names no fleet this account holds, which is the store's
/// answer, and nothing records CodeBuild stating this sentence for them.
pub fn fleet_identifier_violation(identifier: &str) -> Option<&'static str> {
    if is_arn(identifier) {
        return None;
    }
    (!identifier.chars().all(is_supported_name_character)).then_some(INVALID_FLEET_NAME_MESSAGE)
}

/// What CodeBuild states as the reason a value cannot be a build project name, carried by
/// the modelled `InvalidInputException`.
///
/// Wholly fixed text, under [`INVALID_FLEET_NAME_MESSAGE`]'s rule and the sibling of it
/// for the other resource this service addresses by name: the sentence names the
/// character set that IS supported, so it quotes neither the rejected value, the member
/// it came in on nor the model's pattern, and every malformed name is refused with the
/// same words.
pub const INVALID_PROJECT_NAME_MESSAGE: &str =
    "Invalid characters in project name. Only alphanumeric characters, dash, and \
     underscore are supported";

/// Why `identifier` cannot name a build project on the BATCH READ of projects, as the
/// message the modelled `InvalidInputException` carries — or `None` when it can.
///
/// `BatchGetProjectsInput$names` takes "the names or ARNs of the build projects", so an
/// identifier is one of two things: an ARN, whose own fields are checked by the resolver
/// that looks it up
/// ([`super::ComAmazonawsCodebuildCodebuild20161006State::project_by_identifier`]), or a
/// project NAME, which has to be written with the characters a project name is written
/// with ([`is_supported_name_character`]). A name carrying anything else is no project
/// name, so there is nothing to look up and the request is refused rather than answered
/// with a not-found entry that would read as "this account holds no such project".
///
/// The BATCH READ is the one project-addressing operation that applies this, because it
/// is the one whose refusal is recorded; the operations that RESOLVE a project by name to
/// act on it (`StartBuild`, `UpdateProject`, the webhook operations) keep their own
/// recorded answer for a name this account holds nothing under. Where the MODEL constrains
/// the member instead — `CreateProject$name` targets `ProjectName`, which carries the
/// `@pattern` — the boundary validator refuses the name before any handler runs and
/// `crate::service` words that refusal with [`INVALID_PROJECT_NAME_MESSAGE`], so one
/// unwritable name is refused with one sentence either way.
///
/// The CHARACTER SET is the whole of the rule stated here, for the reason
/// [`fleet_identifier_violation`] states for the other resource kind: it is the whole of
/// what CodeBuild's recorded sentence for this refusal states
/// ([`INVALID_PROJECT_NAME_MESSAGE`]). `ProjectName`'s other two clauses — its 2..=150
/// length and its leading character — are not refused here: a name that satisfies the
/// character set but not those simply names no project this account holds, which is the
/// store's answer.
pub fn project_identifier_violation(identifier: &str) -> Option<&'static str> {
    if is_arn(identifier) {
        return None;
    }
    (!identifier.chars().all(is_supported_name_character)).then_some(INVALID_PROJECT_NAME_MESSAGE)
}

/// What CodeBuild states as the reason a value handed to a sandbox-addressing operation
/// cannot be a sandbox id, carried by the modelled `InvalidInputException`.
///
/// Fixed text, under [`INVALID_FLEET_NAME_MESSAGE`]'s rule: the sentence names the
/// member at fault and never quotes the value the request sent.
pub const INVALID_SANDBOX_ID_MESSAGE: &str = "Invalid sandbox ID provided";

/// Why `identifier` cannot name a sandbox, as the message the modelled
/// `InvalidInputException` carries — or `None` when it can.
///
/// Every sandbox-addressing member takes "a sandboxId or sandboxArn", and a sandbox id
/// is STRUCTURED: it is the `<projectName>:<id>` pair [`super::sandbox_uuid`] mints,
/// which is what lets an id name the project the sandbox belongs to as well as the
/// sandbox itself. So an identifier is either a sandbox ARN, whose resource segment
/// carries that pair, or the pair itself; a bare, unstructured value is neither, and
/// carries no sandbox to look up.
///
/// The SHAPE question only — whether the value is a sandbox id at all — so a well-formed
/// id this account holds no sandbox for stays the store's answer rather than this
/// refusal.
pub fn sandbox_identifier_violation(identifier: &str) -> Option<&'static str> {
    let id = if identifier.starts_with("arn:") {
        // An ARN names a sandbox only through this service's `sandbox/` resource
        // segment; any other ARN carries no sandbox id at all.
        match resource_from_arn(identifier, "sandbox/") {
            Some(resource) => resource,
            None => return Some(INVALID_SANDBOX_ID_MESSAGE),
        }
    } else {
        identifier
    };
    let structured = id
        .split_once(':')
        .is_some_and(|(project, rest)| !project.is_empty() && !rest.is_empty());
    (!structured).then_some(INVALID_SANDBOX_ID_MESSAGE)
}
