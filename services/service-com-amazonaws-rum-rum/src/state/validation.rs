// SPDX-License-Identifier: CC0-1.0

//! The modelled constraints this service applies ITSELF, and the words every constraint
//! refusal of this service is stated in.
//!
//! Two jobs, in one module because they are two halves of one rule: a request whose value
//! breaks a constraint the model declares is refused with the modelled
//! `ValidationException` (HTTP 400) before anything is written, and the message that
//! refusal carries is built from the request and quotes the model's own text.
//!
//! # What this applies
//!
//! Nothing here invents a constraint. Every check runs a `@length` bound or a `@pattern`
//! the model declares ([`ConstrainedString::constraints`]), for the shapes the FRAMEWORK's
//! boundary walk leaves unapplied. A shape is left unapplied there for either of two
//! reasons:
//!
//! * its `@pattern` is written with LOOK-AROUND, which the `regex` crate the walk compiles a
//!   pattern with cannot express — and a pattern that fails to compile is treated as
//!   matching (`pattern_matches` in `vendor/smithy-schema-server/src/constraints.rs`), so
//!   such a pattern would otherwise never be applied to anything:
//!   * `AppMonitorDomain` — `(?=…)` / `(?!…)` guards around the host-name grammar;
//!   * `AppMonitorName` — a leading `(?!\.)`;
//!   * `TagKey` — a leading `(?!aws:)`;
//!   * `DimensionName` — a leading `(?!:)` in front of the non-blank run `.*[^\s].*`;
//! * the shape is reached only as a MAP KEY, which the walk does not validate at all — it
//!   captures the key as the path segment of the entry's VALUE and moves on ("the key itself
//!   is not a constrained leaf"), so EVERY constraint the key shape declares is unapplied
//!   there, its `@length` included: `TagKey` again, and `DimensionKey`, whose `@length` is
//!   the only constraint it declares.
//!
//! Each pattern is applied TERM BY TERM, in linear time, by the functions behind
//! [`matches_pattern`] — which say why there, and which map the terms of the model's own
//! text one for one, so a refusal can never state a constraint this module does not apply.
//! Every other constraint of every other member is the boundary walk's, unchanged.
//!
//! BOTH KINDS IN ONE ENVELOPE: the boundary walk runs BEFORE a handler and
//! `crate::service` states its violations in front of the operation, so the handler that
//! applies the checks here never runs for a request that ALSO breaks a boundary-applied
//! constraint. So the gate asks the operation the request names for the violations these
//! checks find in its input ([`SelfApplied`], which each operation's own
//! `self_applied` builds) and states both sets together: one clause per failing
//! (member, requirement), whichever layer applied it, which is what rule 1 below promises.
//! The one request still stated with the boundary's clauses alone is one that omits a
//! `@required` member, where the codec builds no input for these checks to read at all.
//!
//! # How a refusal is stated
//!
//! The framework hands a service a refusal STRUCTURALLY — one [`Violation`] per constraint
//! the request broke — and lets the service choose the words, because no model records
//! them. [`message`] is this service's choice, and it is the ONE place a constraint
//! refusal of this service is worded, whether the violations were collected in front of the
//! operation or handed back by the centralized walk (`crate::service` reaches it from both)
//! or found by a check here.
//! The rules, each holding for every constrained member of every operation rather than for
//! the members one report cited:
//!
//! 1. the envelope is AWS's `N validation error(s) detected: ` over one clause per failing
//!    (member, requirement), joined with `; `, in the order the walk found them;
//! 2. a clause states the value submitted, the member's dotted path and the requirement it
//!    failed in the FRAMEWORK's own words — read back out of [`Violation::message`] rather
//!    than re-spelled, so a requirement class cannot drift from the way the framework
//!    words it;
//! 3. a reported `@pattern` is the expression WITHOUT the `^` / `$` pair a model writes
//!    around it to demand a whole-string match
//!    (`service_common::validation::unanchored`): a caller that lifts the advertised
//!    expression out of the message and re-applies it locally must get the character
//!    grammar the service publishes. The CHECK always runs the model's pattern as written;
//! 4. a constraint broken by an ELEMENT of a list is stated at the LIST member — no
//!    element position and no `member` step in the path — with the element shape's WHOLE
//!    modelled constraint set nested inside `Member must satisfy constraint: [ … ]` and the
//!    list rendered as the value. The set is the element shape's and not just the
//!    constraint that happened to fail, so the clause tells a caller what every element of
//!    that member has to satisfy, and two elements of one list are ONE validation error
//!    rather than one per element. A constraint broken by the list ITSELF (its own
//!    `@length`) is not nested, and neither is one broken by a member inside a structure
//!    element, which keeps its full path;
//! 5. the bracketed set of an enum clause states the membership the model declares, in the
//!    order this service lists that enum's values ([`listed`]).
//!
//! Rule 4 needs two things no [`Violation`] carries: which collection a failing element
//! belongs to, and the value that collection held. The first is the path spelling, owned by
//! `service_common::validation::list_of_element` and matched against the LIST members this
//! service's input shapes declare ([`LIST_ELEMENT_CONSTRAINTS`], read off the generated
//! schemas so the set a clause states is the model's). The second is passed in by the
//! caller that HAS the request's lists — the gate in front of every operation
//! (`crate::service`) and the checks here; a caller that has only the violations falls back
//! to the REFUSED elements of that collection, which is as much of the member's value as
//! the violations themselves know. The member named and the constraint set stated are exact
//! either way.
//!
//! The one refusal of this service NOT worded here is a refused TAG call: that frontend has
//! its own error vocabulary and its own sentence for a `ResourceArn` which is not an ARN
//! (`crate::service`), and what it words a refusal of any other tag member in is not known,
//! so those keep the framework's own clause.

use std::collections::HashMap;
use std::sync::LazyLock;

use aws_smithy_schema::{Schema, ShapeId, ShapeType, TraitMap};
use aws_smithy_schema_server::constraints::{
    self, ElementConstraintsTrait, EnumTrait, LengthTrait, PatternTrait, RangeTrait, Violation,
    ViolationKind,
};
use service_common::validation::{list_of_element, unanchored};

/// The wire error code a constraint refusal of an app-monitor call carries: the
/// `ValidationException` this service's model declares, which is also the code the
/// framework's own default picks on this `restJson1` service. Restated here because a
/// chosen [`Refused`] replaces the code and the message together.
pub const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The text separating a clause's subject from the requirement it states, in the
/// framework's own sentence ([`Violation::message`]). Split on, never written.
const CLAUSE_MARKER: &str = "failed to satisfy constraint: ";

/// What a clause states around the constraint set of a collection's ELEMENT (rule 4).
const ELEMENT_SET_OPEN: &str = "Member must satisfy constraint: [";

// =============================================================================
// The constraints this service applies itself
// =============================================================================

/// A string shape of this service whose modelled constraints the boundary walk leaves
/// unapplied — a `@pattern` written with look-around, or a shape reached as a map KEY (see
/// the module docs). Every member targeting one of these shapes is checked by the operation
/// that accepts it, before it writes anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstrainedString {
    /// `AppMonitorName`: the name a create gives an app monitor.
    AppMonitorName,
    /// `AppMonitorDomain`: the internet domain an application has authority over, named
    /// either as `Domain` or as an element of `DomainList`.
    AppMonitorDomain,
    /// `TagKey`: the key shape of `TagMap`, which every `Tags` member of this service
    /// targets. A map KEY is not a constrained leaf for the boundary walk at all (it
    /// captures the key as the path segment of the entry's VALUE), so this shape is
    /// unapplied there twice over.
    TagKey,
    /// `DimensionKey`: the key shape of `DimensionKeysMap`, which the `DimensionKeys` of
    /// every metric definition targets — the dotted field path a dimension's value is read
    /// out of the matched event. A map key is not a constrained leaf for the boundary walk,
    /// so the `@length` this shape declares — the only constraint it declares — is unapplied
    /// there.
    DimensionKey,
    /// `DimensionName`: the value shape of `DimensionKeysMap` — the CloudWatch dimension
    /// name an entry maps its field path to. Its `@pattern` opens with a negative
    /// look-ahead, so the boundary walk admits any value for it.
    DimensionName,
}

/// The modelled constraints of one string shape: its `@length` bounds and its `@pattern`
/// source text.
///
/// Held as the model's own numbers and text, so a clause — which quotes them — cannot
/// state a constraint [`string_violation`] does not apply.
#[derive(Debug, Clone, Copy)]
pub struct StringConstraints {
    /// `@length`'s inclusive minimum, in CHARACTERS (a `@length` on a string counts
    /// characters, not bytes).
    pub min_length: i64,
    /// `@length`'s inclusive maximum, in characters.
    pub max_length: i64,
    /// The `@pattern` source, verbatim from the model, or `None` for a shape that declares
    /// no `@pattern` at all — a `@length` alone is still unapplied at the boundary when the
    /// shape is reached as a map key.
    pub pattern: Option<&'static str>,
}

impl ConstrainedString {
    /// The constraints the model declares on this shape.
    ///
    /// Source: `smithy_query(query="shape",
    /// shape="AppMonitorName,AppMonitorDomain,TagKey,DimensionKey,DimensionName")` against
    /// `services/service-com-amazonaws-rum-rum/smithy-model/com-amazonaws-rum-rum.json`.
    #[must_use]
    pub const fn constraints(self) -> StringConstraints {
        match self {
            Self::AppMonitorName => StringConstraints {
                min_length: 1,
                max_length: 255,
                pattern: Some(r"^(?!\.)[\.\-_#A-Za-z0-9]+$"),
            },
            Self::AppMonitorDomain => StringConstraints {
                min_length: 1,
                max_length: 253,
                pattern: Some(r"^(localhost)$|^((25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\.){3}(25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)$|(?=^[a-zA-Z0-9\.\*-]{4,253}$)(?!.*\.-)(?!.*-\.)(?!.*\.\.)(?!.*[^\.]{64,})^(\*\.)?(?![-\.\*])[^\*]{1,}\.(\*|(?!.*--)(?=.*[a-zA-Z])[^\*]{1,}[^\*-])$"),
            },
            Self::TagKey => StringConstraints {
                min_length: 1,
                max_length: 128,
                pattern: Some(r"^(?!aws:)[a-zA-Z+-=._:/]+$"),
            },
            Self::DimensionKey => StringConstraints {
                min_length: 1,
                max_length: 280,
                pattern: None,
            },
            Self::DimensionName => StringConstraints {
                min_length: 1,
                max_length: 255,
                pattern: Some(r"^(?!:).*[^\s].*"),
            },
        }
    }
}

/// Whether `value` satisfies the `@pattern` `shape` declares.
///
/// The pattern is applied TERM BY TERM rather than by a regex engine, which is forced by
/// two facts about this path. The engine behind `@pattern` is the linear-time `regex`
/// crate, which cannot express the look-around these three patterns are written with at all
/// (that is why the boundary walk admits any value for them); and a BACKTRACKING engine,
/// which could, is not available in a per-service handler path that matches a value an
/// untrusted caller chose. So each pattern's terms are applied as explicit checks whose
/// cost is linear in the value's length — after the `@length` bound, which caps the value
/// at 255 characters ([`string_violation`]).
///
/// Each function below maps the terms of the model's own text ONE FOR ONE, and names the
/// term it applies, so a clause — which quotes that text — cannot state a constraint that
/// is not applied; the suites' tables are what pin the reading from the outside, with a row
/// per branch of the grammar, per way a value falls outside it, and per published bound
/// with one past it. The same shape is in
/// `service-com-amazonaws-fsx-awssimbaapiservice-v20180301/src/service/validation.rs`,
/// which transcribes a look-around pattern for the same reason.
fn matches_pattern(shape: ConstrainedString, value: &str) -> bool {
    match shape {
        ConstrainedString::AppMonitorName => is_app_monitor_name(value),
        ConstrainedString::AppMonitorDomain => is_app_monitor_domain(value),
        ConstrainedString::TagKey => is_tag_key(value),
        ConstrainedString::DimensionName => is_dimension_name(value),
        // `DimensionKey` declares no `@pattern`, so it has no pattern term to apply and
        // [`string_violation`] never asks: its `@length` is its whole constraint set.
        ConstrainedString::DimensionKey => true,
    }
}

/// Whether `name` satisfies `AppMonitorName`'s `@pattern`,
/// `^(?!\.)[\.\-_#A-Za-z0-9]+$`: it does not open with the dot the negative look-ahead
/// excludes, and between the anchors it is a non-empty run of characters the class admits
/// (the ASCII letters and digits, `.`, `-`, `_` and `#`).
fn is_app_monitor_name(name: &str) -> bool {
    !name.starts_with('.')
        && !name.is_empty()
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ".-_#".contains(character))
}

/// Whether `name` satisfies `DimensionName`'s `@pattern`, `^(?!:).*[^\s].*`: it does not
/// open with the colon the negative look-ahead excludes, and it carries at least one
/// character that is not whitespace — the `[^\s]` the pattern's two `.*` runs surround. Both
/// terms are anchored at the start of the value by the `^`, and `.` matches no line break,
/// so the non-whitespace character has to stand on the value's FIRST line.
fn is_dimension_name(name: &str) -> bool {
    // `split_once` hands back the head before the FIRST line break, and the whole value when
    // it carries none.
    let first_line = name
        .split_once(LINE_BREAK)
        .map_or(name, |(head, _)| head);
    !name.starts_with(DIMENSION_NAME_EXCLUDED_PREFIX)
        && first_line
            .chars()
            .any(|character| !character.is_whitespace())
}

/// The prefix `DimensionName`'s leading `(?!:)` excludes. Applied as a `starts_with`, which
/// is what a negative look-ahead anchored at the start of the pattern says.
const DIMENSION_NAME_EXCLUDED_PREFIX: char = ':';

/// The one character a regex `.` does not match, which is what bounds `.*[^\s].*` to the
/// first line of the value it is anchored at the start of. The line break of the engine the
/// model's patterns are applied with here; an engine that also treats a carriage return or
/// one of Unicode's line separators that way would bound the run one character earlier.
const LINE_BREAK: char = '\n';

/// Whether `key` satisfies `TagKey`'s `@pattern`, `^(?!aws:)[a-zA-Z+-=._:/]+$`: it is
/// outside the reserved namespace the negative look-ahead excludes AND it is a non-empty
/// run of characters the class admits.
fn is_tag_key(key: &str) -> bool {
    !key.starts_with(TAG_KEY_RESERVED_PREFIX)
        && !key.is_empty()
        && key.chars().all(is_tag_key_character)
}

/// The prefix `TagKey`'s leading `(?!aws:)` excludes: AWS's reserved tag namespace. Applied
/// as a `starts_with`, which is what a negative look-ahead anchored at the start of the
/// pattern says.
const TAG_KEY_RESERVED_PREFIX: &str = "aws:";

/// Whether `character` is one the class `[a-zA-Z+-=._:/]` of `TagKey`'s `@pattern` admits.
///
/// That class admits the ASCII letters, `_`, and — because `+-=` inside a character class is
/// the RANGE from `+` (0x2B) to `=` (0x3D) — every character in that range: the digits, `,`,
/// `-`, `.`, `/`, `:`, `;`, `<` and `=`. The `.`, `:` and `/` the class also lists
/// explicitly are inside that range too, so the three terms below are the whole class.
fn is_tag_key_character(character: char) -> bool {
    character.is_ascii_alphabetic() || character == '_' || ('+'..='=').contains(&character)
}

/// Whether `domain` satisfies `AppMonitorDomain`'s `@pattern`, whose three top-level
/// branches are the three forms a domain authority can take: the literal host, a
/// dotted-quad address, or a DNS name.
fn is_app_monitor_domain(domain: &str) -> bool {
    domain == LOCALHOST || is_dotted_quad(domain) || is_dns_name(domain)
}

/// The host the pattern's first branch, `^(localhost)$`, admits on its own.
const LOCALHOST: &str = "localhost";

/// Whether `domain` is the pattern's second branch,
/// `^((25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\.){3}(25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)$`:
/// four dot-separated octets.
fn is_dotted_quad(domain: &str) -> bool {
    let octets: Vec<&str> = domain.split('.').collect();
    octets.len() == 4 && octets.iter().all(|octet| is_octet(octet))
}

/// Whether `octet` is one of that branch's three alternatives, which between them admit one
/// to three ASCII digits whose value is at most 255: `25[0-5]` and `2[0-4][0-9]` take the
/// three-digit values from 200 up, and `[01]?[0-9][0-9]?` every shorter run and every
/// three-digit run opening with `0` or `1` — so leading zeroes are admitted and `256` is
/// not.
fn is_octet(octet: &str) -> bool {
    (1..=3).contains(&octet.len())
        && octet.bytes().all(|byte| byte.is_ascii_digit())
        && octet.parse::<u16>().is_ok_and(|value| value <= 255)
}

/// Whether `domain` is the pattern's third branch, the DNS-name grammar: four look-ahead
/// guards over the whole value, then the name's own structure.
///
/// Each guard is one term of the model's text. The first, `(?=^[a-zA-Z0-9\.\*-]{4,253}$)`,
/// bounds the whole value and fixes its alphabet; the next three forbid the three
/// label-boundary spellings `.-`, `-.` and `..`; the last, `(?!.*[^\.]{64,})`, bounds one
/// label at 63 characters. All four are anchored at the start of the value — `^` matches
/// only there — so they hold for the value as a whole rather than at some offset into it.
fn is_dns_name(domain: &str) -> bool {
    let length = domain.chars().count();
    if !(DNS_MIN_LENGTH..=DNS_MAX_LENGTH).contains(&length) {
        return false;
    }
    if !domain
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || ".*-".contains(character))
    {
        return false;
    }
    if domain.contains(".-") || domain.contains("-.") || domain.contains("..") {
        return false;
    }
    if domain
        .split('.')
        .any(|label| label.chars().count() >= DNS_LABEL_CEILING)
    {
        return false;
    }
    // `^(\*\.)?`: a name may open with the wildcard label, and only in that one form — a
    // value whose first character is `*` and whose second is not `.` satisfies neither this
    // group nor the class that follows it.
    let body = domain.strip_prefix("*.").unwrap_or(domain);
    // `(?![-\.\*])`: whatever is left cannot open with a hyphen, a dot or a wildcard.
    if body.starts_with(['-', '.', '*']) {
        return false;
    }
    // `[^\*]{1,}\.(…)$`: the rest is a non-empty run carrying no wildcard, then a dot, then
    // the last label. The run may itself contain dots, so EVERY dot is a candidate split and
    // the name is admitted when one of them works — which is what the quantifier says.
    body.char_indices()
        .filter(|(_, character)| *character == '.')
        .any(|(at, _)| {
            let (head, tail) = body.split_at(at);
            // `tail` opens with the one-byte `.` this split is at, so the remainder after
            // it is the last label.
            let last_label = tail.get(1..).unwrap_or_default();
            !head.is_empty() && !head.contains('*') && is_last_label(last_label)
        })
}

/// Whether `label` is what the DNS branch's last group,
/// `(\*|(?!.*--)(?=.*[a-zA-Z])[^\*]{1,}[^\*-])`, admits: the bare wildcard, or a label that
/// carries no doubled hyphen, holds at least one ASCII letter, carries no wildcard, and is
/// at least two characters of which the last is neither a wildcard nor a hyphen.
fn is_last_label(label: &str) -> bool {
    if label == "*" {
        return true;
    }
    !label.contains("--")
        && label.chars().any(|character| character.is_ascii_alphabetic())
        && !label.contains('*')
        && label.chars().count() >= 2
        && !label.ends_with('-')
}

/// The whole-value bounds the DNS branch's leading look-ahead states,
/// `[a-zA-Z0-9\.\*-]{4,253}`.
const DNS_MIN_LENGTH: usize = 4;
const DNS_MAX_LENGTH: usize = 253;

/// The run of non-dot characters the DNS branch's last look-ahead forbids,
/// `(?!.*[^\.]{64,})` — so one label is at most 63 characters.
const DNS_LABEL_CEILING: usize = 64;

/// The violation `value` at `path` commits against the constraints `shape` declares, or
/// `None` when it satisfies every one of them.
///
/// The constraints are taken in the order the framework's own leaf walk takes them —
/// `@length` before `@pattern` — and the first one a value fails is the one reported, so a
/// member refused here is refused with the same clause the boundary would have produced had
/// it been able to apply the pattern. `path` is the dotted member path the framework's walk
/// spells that position as, and the value is echoed into the clause exactly as the caller
/// sent it.
#[must_use]
pub fn string_violation(shape: ConstrainedString, path: &str, value: &str) -> Option<Violation> {
    let constraints = shape.constraints();
    // `@length` on a string counts characters, the way the framework's walk counts them.
    let length = i64::try_from(value.chars().count()).unwrap_or(i64::MAX);
    let kind = if length < constraints.min_length {
        ViolationKind::MinLength(constraints.min_length)
    } else if length > constraints.max_length {
        ViolationKind::MaxLength(constraints.max_length)
    } else {
        // A shape that declares no `@pattern` has nothing left to fail once its bounds hold.
        match constraints.pattern {
            Some(pattern) if !matches_pattern(shape, value) => {
                ViolationKind::Pattern(pattern.to_string())
            }
            _ => return None,
        }
    };
    Some(Violation {
        path: path.to_string(),
        kind,
        value: Some(value.to_string()),
        sensitive: false,
    })
}

/// What one request breaks of the constraints this service applies ITSELF: the violations,
/// and the request's own collections a clause about one of their ELEMENTS is stated over
/// (rule 4).
///
/// Handed back structurally, unworded, because two callers state it and neither owns the
/// words: the operation's own handler, which answers it as that operation's modelled
/// refusal ([`SelfApplied::refusal`] for the frontend this module words), and the gate in
/// front of every operation (`crate::service`), which states it in the SAME envelope as the
/// violations the framework's boundary walk found — so one request's two kinds of
/// constraint failure are one answer and not two round trips (rule 1).
#[derive(Debug, Default)]
pub struct SelfApplied {
    /// The violations, in the order the checks that found them judged the members.
    pub violations: Vec<Violation>,
    /// The request's lists, keyed by the member path the walk reports that list at and
    /// already rendered: the value a clause stated AT a list member quotes (rule 4).
    pub lists: HashMap<String, String>,
}

impl SelfApplied {
    /// Take `other`'s violations and lists onto these, for an operation whose input carries
    /// members of SEVERAL of the shapes above — the create that both names a monitor's
    /// domain authority and seeds its tag map — so one request is still one envelope.
    pub fn extend(&mut self, other: Self) {
        self.violations.extend(other.violations);
        for (path, rendered) in other.lists {
            self.lists.entry(path).or_insert(rendered);
        }
    }

    /// Whether these checks found nothing to refuse.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.violations.is_empty()
    }

    /// Why these violations refuse the request, in the words this service states a
    /// constraint refusal in ([`message`]) — `None` when there are none.
    #[must_use]
    pub fn refusal(&self) -> Option<String> {
        message(&self.violations, &self.lists)
    }
}

/// Why the members a create or an update names cannot be applied, or `None` when every one
/// of them satisfies the constraints its shape declares.
///
/// `name` is the `Name` a create gives a monitor, and `None` for the operations that bind
/// the name to the URI (`@httpLabel`) to RESOLVE an existing monitor: a name outside the
/// pattern names no stored monitor — every stored one satisfied the pattern when it was
/// created — so those answer the `ResourceNotFoundException` they model. The create is
/// where a name enters the store, so it is the call that has to refuse one the model
/// forbids.
///
/// The domain half is shared by every operation that accepts the pair (the create that
/// seeds a monitor's domain authority and the update that replaces it), because the
/// constraints are the shape's — declared once on `AppMonitorDomain`, which `Domain`
/// targets directly and `DomainList`'s member targets too — and not the property of
/// whichever operation carries them.
///
/// Every member is judged, and the envelope states them TOGETHER in the order the input
/// declares them, because a request that breaks two members is two validation errors and
/// not one (rule 1 in the module docs). A `DomainList` violation is stated at the LIST
/// member over the whole list the request sent (rule 4), which is why the list is carried
/// on the [`SelfApplied`] rather than only the element that offended.
///
/// Neither the work nor the message is sized by the request: the elements judged stop at the
/// count the framework's own walk collects to
/// (`service_common::validation::MAX_VIOLATIONS`), and the list a clause quotes is held to
/// the ceiling that module states for one echoed value
/// (`service_common::validation::quoted_echo`), which is the ceiling every other clause of
/// this tree is quoted up to. Both matter because a caller can send a list far longer than
/// `AppMonitorDomainList`'s `@length` admits: the framework's walk refuses it for the
/// LENGTH, and these checks are run beside that refusal (`crate::service`).
#[must_use]
pub fn app_monitor_self_applied(
    name: Option<&str>,
    domain: Option<&str>,
    domain_list: Option<&[String]>,
) -> SelfApplied {
    let mut violations: Vec<Violation> = Vec::new();
    if let Some(name) = name {
        violations.extend(string_violation(
            ConstrainedString::AppMonitorName,
            NAME_PATH,
            name,
        ));
    }
    let shape = ConstrainedString::AppMonitorDomain;
    if let Some(domain) = domain {
        violations.extend(string_violation(shape, DOMAIN_PATH, domain));
    }
    let domains = domain_list.unwrap_or_default();
    violations.extend(
        domains
            .iter()
            .enumerate()
            .filter_map(|(position, domain)| {
                let path = format!(
                    "{DOMAIN_LIST_PATH}.{}",
                    constraints::path::element(position + 1)
                );
                string_violation(shape, &path, domain)
            })
            .take(service_common::validation::MAX_VIOLATIONS),
    );
    let rendered = rendered_list(domains);
    SelfApplied {
        violations,
        lists: HashMap::from([(
            DOMAIN_LIST_PATH.to_string(),
            service_common::validation::quoted_echo(&rendered).into_owned(),
        )]),
    }
}

/// The dotted member path a violation on `Name` is reported at.
const NAME_PATH: &str = "name";

/// The dotted member path a violation on `Domain` is reported at.
const DOMAIN_PATH: &str = "domain";

/// The dotted member path a violation on an element of `DomainList` is reported at.
const DOMAIN_LIST_PATH: &str = "domainList";

/// `elements` as a list renders — the elements in order, comma-separated, in brackets:
/// the form AWS echoes a list-valued member back in.
fn rendered_list<S: AsRef<str>>(elements: &[S]) -> String {
    let rendered: Vec<&str> = elements.iter().map(AsRef::as_ref).collect();
    format!("[{}]", rendered.join(", "))
}

// =============================================================================
// The words
// =============================================================================

/// The `message` a `ValidationException` carries for `violations`: the AWS envelope over
/// one clause per offending member, in the order the walk found them (rule 1).
///
/// `lists` holds the request's lists, keyed by the member path the walk reports that list
/// at and already rendered, for the clause a list-element violation is stated at (rule 4).
/// A caller that does not have them passes an empty map and the clause states the refused
/// elements instead.
///
/// `None` only when there is nothing to state.
#[must_use]
pub fn message(violations: &[Violation], lists: &HashMap<String, String>) -> Option<String> {
    // The element constraint set each list member's clause states, resolved ONCE per
    // member from every requirement its elements broke: every element violation of that
    // member then states the same requirement, which is what folds them onto one clause.
    let mut broken: HashMap<&str, Vec<String>> = HashMap::new();
    for violation in violations {
        let Some(list) = enclosing_list(&violation.path) else {
            continue;
        };
        if let Some(failing) = requirement(&violation.kind) {
            let failures = broken.entry(list).or_default();
            if !failures.contains(&failing) {
                failures.push(failing);
            }
        }
    }
    let element_sets: HashMap<&str, String> = broken
        .iter()
        .map(|(list, failures)| (*list, element_constraint_set(list, failures).join(", ")))
        .collect();

    let mut order: Vec<String> = Vec::new();
    let mut clauses: HashMap<String, Clause> = HashMap::new();
    for violation in violations {
        let nested = enclosing_list(&violation.path)
            .zip(requirement(&violation.kind))
            .map(|(list, failing)| {
                let set = element_sets.get(list).unwrap_or(&failing);
                (list, format!("{ELEMENT_SET_OPEN}{set}]"))
            });
        let Some((list, requirement)) = nested else {
            // Not a violation on a list's element: the framework's own clause, with only
            // the requirement restated the way this service states it (rules 2, 3 and 5).
            let clause = stated_clause(violation);
            if clauses
                .insert(clause.clone(), Clause::Verbatim(clause.clone()))
                .is_none()
            {
                order.push(clause);
            }
            continue;
        };
        // A clause is keyed by the member it is reported at and the requirement it states,
        // so every element of ONE list accumulates into a single clause — and so do two
        // constraints of one element shape, because both state that shape's whole set.
        let key = format!("{list}\u{1f}{requirement}");
        let clause = match clauses.get_mut(&key) {
            Some(clause) => clause,
            None => {
                order.push(key.clone());
                clauses.entry(key).or_insert(Clause::List {
                    path: list.to_string(),
                    requirement,
                    rendered: lists.get(list).cloned(),
                    refused: Vec::new(),
                })
            }
        };
        if let (Clause::List { refused, .. }, Some(value)) = (clause, violation.value.as_deref()) {
            // One rendered value per violation PATH: the same element breaking two of its
            // shape's constraints is one value, two elements are two.
            if !refused.iter().any(|(seen, _)| seen == &violation.path) {
                refused.push((violation.path.clone(), value.to_string()));
            }
        }
    }

    let rendered: Vec<String> = order
        .iter()
        .filter_map(|key| clauses.get(key))
        .map(Clause::render)
        .collect();
    match rendered.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", rendered[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            rendered.join("; ")
        )),
    }
}

/// One clause of the envelope, accumulated as the violations are walked.
#[derive(Debug)]
enum Clause {
    /// A clause about one member's own constraint, already stated.
    Verbatim(String),
    /// One LIST member's clause: the member it is reported at, the element shape's
    /// constraint set, the whole list where the caller passed it, and the refused elements
    /// in walk order (each with the violation PATH it was found at) as the fallback value.
    List {
        path: String,
        requirement: String,
        rendered: Option<String>,
        refused: Vec<(String, String)>,
    },
}

impl Clause {
    /// The clause as the envelope states it.
    fn render(&self) -> String {
        match self {
            Self::Verbatim(clause) => clause.clone(),
            Self::List {
                path,
                requirement,
                rendered,
                refused,
            } => {
                // The value the member HELD where the caller had it, and otherwise the
                // list formed from the elements the violations named — as much of it as a
                // refusal that carries only the offending elements knows. Inventing the
                // rest would echo bytes the caller never sent.
                let value = rendered.clone().unwrap_or_else(|| {
                    let elements: Vec<&str> =
                        refused.iter().map(|(_, value)| value.as_str()).collect();
                    rendered_list(&elements)
                });
                format!("Value '{value}' at '{path}' {CLAUSE_MARKER}{requirement}")
            }
        }
    }
}

/// The clause this service states for a violation that is not on a list's element: the
/// framework's own sentence, over the requirement as this service reports it.
fn stated_clause(violation: &Violation) -> String {
    Violation {
        path: violation.path.clone(),
        kind: stated_kind(&violation.kind),
        value: violation.value.clone(),
        sensitive: violation.sensitive,
    }
    .message()
}

/// The requirement sentence of one violation class, as this service reports it.
///
/// Read back out of [`Violation::message`] — the ONE place that sentence is written —
/// rather than restated here, so a requirement nested into a list clause can never be
/// worded differently from the clause a non-list violation keeps. `None` for a class whose
/// sentence does not follow that shape, which leaves that violation with the framework's
/// own clause.
fn requirement(kind: &ViolationKind) -> Option<String> {
    let probe = Violation {
        path: String::new(),
        kind: stated_kind(kind),
        value: None,
        sensitive: false,
    };
    probe
        .message()
        .split_once(CLAUSE_MARKER)
        .map(|(_, requirement)| requirement.to_string())
}

/// The violation class as this service reports it: a `@pattern` without the anchor pair a
/// model writes around it (rule 3), and an enum value set in the order this service lists
/// that enum's values (rule 5). Every other class is the framework's, untouched.
fn stated_kind(kind: &ViolationKind) -> ViolationKind {
    match kind {
        ViolationKind::Pattern(regex) => ViolationKind::Pattern(unanchored(regex)),
        ViolationKind::EnumValue(values) => ViolationKind::EnumValue(listed(values)),
        other => other.clone(),
    }
}

/// The order this service's refusals list `MetricDestination`'s admitted values in.
///
/// RECORDED: a refusal of an out-of-enum `Destination` states the set in this order, which
/// no order derived from the model reproduces — the model declares the two values the other
/// way round (`smithy_query(query="shape", shape="MetricDestination")`), and so does the
/// documentation of every member that targets it. The values themselves are the model's,
/// read off the two constants the rest of this crate resolves a destination through rather
/// than spelled again here, so the set stated is exactly the membership the model declares.
const METRIC_DESTINATION_LISTING: [&str; 2] = [
    super::EVIDENTLY_DESTINATION,
    super::CLOUD_WATCH_DESTINATION,
];

/// The orders this service is known to list an enum's values in. An enum that is not here
/// keeps the order the model declares, which is what the framework's clause already states.
const ENUM_LISTINGS: [&[&str]; 1] = [&METRIC_DESTINATION_LISTING];

/// `values` in the order this service lists that enum's values: its own listing where one
/// is known, and the order handed in — the model's — for every other enum.
///
/// A listing is matched on MEMBERSHIP, so it applies to every member of every operation
/// that targets the enum rather than to the one member a report cited, and a listing whose
/// membership the model changes stops matching rather than stating a set the model no longer
/// declares.
fn listed(values: &[String]) -> Vec<String> {
    let mut sorted: Vec<&str> = values.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    for listing in ENUM_LISTINGS {
        let mut candidate: Vec<&str> = listing.to_vec();
        candidate.sort_unstable();
        if candidate == sorted {
            return listing.iter().map(|value| (*value).to_string()).collect();
        }
    }
    values.to_vec()
}

// =============================================================================
// The element constraint sets this model declares
// =============================================================================

/// The LIST member `path` points INTO, when `path` is the path of one of that list's
/// elements AND this model declares element constraints for it.
///
/// The path spelling is `service_common::validation::list_of_element`'s, so this cannot
/// disagree with the walk that produced it about which positions are element positions. The
/// catalogue lookup is the second half: a MAP entry's step carries the key the CALLER chose,
/// so a key spelled like a list position is indistinguishable from one by the path alone,
/// and asking whether the member is a declared LIST is what tells the two apart.
fn enclosing_list(path: &str) -> Option<&str> {
    let list = list_of_element(path)?;
    let member = list.rsplit('.').next()?;
    LIST_ELEMENT_CONSTRAINTS.contains_key(member).then_some(list)
}

/// The element constraint sets this service's input shapes declare for their LIST members,
/// keyed by the member's own name — the last segment of the path the walk reports that
/// member under.
///
/// Built once, from the generated schemas, so what a clause states is the model's and a
/// member whose element constraints change in the model changes here with it. A name can
/// hold SEVERAL sets, because two shapes may name a list member the same way over different
/// element shapes; which one a clause states is then decided by the constraints that
/// actually failed (see [`element_constraint_set`]).
///
/// Keyed by the member NAME rather than by its full dotted path because a list member of
/// this service can sit inside a nested structure (`appMonitorConfiguration.excludedPages`)
/// and a `Schema` exposes no link from a member to the shape it targets, so a full path
/// cannot be derived from the schemas alone.
static LIST_ELEMENT_CONSTRAINTS: LazyLock<HashMap<String, Vec<Vec<String>>>> = LazyLock::new(
    || {
        let mut catalog: HashMap<String, Vec<Vec<String>>> = HashMap::new();
        for schema in input_schemas() {
            for member in schema.members() {
                if !matches!(member.shape_type(), ShapeType::List) {
                    continue;
                }
                let Some(name) = member.member_name() else {
                    continue;
                };
                let Some(traits) = member.traits() else {
                    continue;
                };
                let Some(wrapper) = constraint_trait::<ElementConstraintsTrait>(
                    traits,
                    &ElementConstraintsTrait::TRAIT_ID,
                ) else {
                    continue;
                };
                let requirements = element_requirements(wrapper.inner());
                if requirements.is_empty() {
                    continue;
                }
                let sets = catalog.entry(constraints::path::member(name)).or_default();
                if !sets.contains(&requirements) {
                    sets.push(requirements);
                }
            }
        }
        catalog
    },
);

/// The element constraint set a clause at the list `path` states, given every requirement
/// the walk reported `path`'s elements as having broken.
///
/// Those failures are what identify the element shape when a member name is declared by
/// several shapes: a set that does not contain all of them is not the set this request
/// broke. When no set is found, or several candidates contain them all, the clause states
/// the broken requirements alone — those are the thing known to be true of the request,
/// where a guessed set would not be.
fn element_constraint_set(path: &str, failing: &[String]) -> Vec<String> {
    let alone = || failing.to_vec();
    let Some(member) = path.rsplit('.').next() else {
        return alone();
    };
    let Some(sets) = LIST_ELEMENT_CONSTRAINTS.get(member) else {
        return alone();
    };
    let mut matching = sets
        .iter()
        .filter(|set| failing.iter().all(|broken| set.contains(broken)));
    let Some(found) = matching.next() else {
        return alone();
    };
    if matching.any(|other| other != found) {
        return alone();
    }
    found.clone()
}

/// One constraint trait off a trait map, downcast to the type that carries its bounds.
fn constraint_trait<'map, T: 'static>(traits: &'map TraitMap, id: &ShapeId) -> Option<&'map T> {
    traits.get(id)?.as_any().downcast_ref::<T>()
}

/// Every requirement an element shape declares, in the order a clause states them.
///
/// The ORDER is the one this service's recorded refusals show: the `@length` maximum, then
/// its minimum, then the numeric bounds the same way, then the `@pattern`, then an
/// enumerated value set. A `@length` with no declared minimum states `0`, the smallest
/// length a Smithy `@length` can bound. Each sentence comes from [`requirement`], so it is
/// the wording a clause about that bound uses anywhere else in this module.
///
/// Empty when the element shape declares no constraint at all.
fn element_requirements(traits: &TraitMap) -> Vec<String> {
    let mut kinds: Vec<ViolationKind> = Vec::new();
    if let Some(length) = constraint_trait::<LengthTrait>(traits, &LengthTrait::TRAIT_ID) {
        if let Some(max) = length.max {
            kinds.push(ViolationKind::MaxLength(max));
        }
        kinds.push(ViolationKind::MinLength(length.min.unwrap_or(0)));
    }
    if let Some(range) = constraint_trait::<RangeTrait>(traits, &RangeTrait::TRAIT_ID) {
        if let Some(max) = range.max {
            kinds.push(ViolationKind::MaxValue(render_bound(max)));
        }
        if let Some(min) = range.min {
            kinds.push(ViolationKind::MinValue(render_bound(min)));
        }
    }
    if let Some(pattern) = constraint_trait::<PatternTrait>(traits, &PatternTrait::TRAIT_ID) {
        kinds.push(ViolationKind::Pattern(pattern.regex.to_string()));
    }
    if let Some(allowed) = constraint_trait::<EnumTrait>(traits, &EnumTrait::TRAIT_ID) {
        kinds.push(ViolationKind::EnumValue(
            allowed
                .values
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
        ));
    }
    kinds.iter().filter_map(requirement).collect()
}

/// A `@range` bound as it displays: an integral value without a trailing `.0`, a fractional
/// value with its decimals — the rendering the framework's own clause uses, so a bound read
/// off the model here and one read off a violation agree.
fn render_bound(bound: f64) -> String {
    if bound.fract() == 0.0 && bound.is_finite() {
        format!("{bound:.0}")
    } else {
        format!("{bound}")
    }
}

/// Every INPUT shape of this service, plus every structure an input reaches, as the
/// generated schema statics.
///
/// ALL of them, not the ones one report cited: an element constraint is a property of a
/// shape, so leaving one out would silently word its list clauses with less than the model
/// declares. The input shapes are exactly the statics
/// `grep -h 'const INPUT_SCHEMA' src/ops/*.rs` names; the rest are the structures those
/// inputs carry (`crate::ops::types`), which is where this service's constrained list
/// members mostly live (`AppMonitorConfiguration`'s page lists). A shape missing from this
/// list is not a wrong clause, only a clause that states the constraints which failed rather
/// than the element shape's whole set.
fn input_schemas() -> Vec<&'static Schema> {
    use crate::ops::types as t;
    vec![
        t::BATCH_CREATE_RUM_METRIC_DEFINITIONS_INPUT_SCHEMA,
        t::BATCH_DELETE_RUM_METRIC_DEFINITIONS_INPUT_SCHEMA,
        t::BATCH_GET_RUM_METRIC_DEFINITIONS_INPUT_SCHEMA,
        t::CREATE_APP_MONITOR_INPUT_SCHEMA,
        t::DELETE_APP_MONITOR_INPUT_SCHEMA,
        t::DELETE_RESOURCE_POLICY_INPUT_SCHEMA,
        t::DELETE_RUM_METRICS_DESTINATION_INPUT_SCHEMA,
        t::GET_APP_MONITOR_DATA_INPUT_SCHEMA,
        t::GET_APP_MONITOR_INPUT_SCHEMA,
        t::GET_RESOURCE_POLICY_INPUT_SCHEMA,
        t::LIST_APP_MONITORS_INPUT_SCHEMA,
        t::LIST_RUM_METRICS_DESTINATIONS_INPUT_SCHEMA,
        t::LIST_TAGS_FOR_RESOURCE_INPUT_SCHEMA,
        t::PUT_RESOURCE_POLICY_INPUT_SCHEMA,
        t::PUT_RUM_EVENTS_INPUT_SCHEMA,
        t::PUT_RUM_METRICS_DESTINATION_INPUT_SCHEMA,
        t::TAG_RESOURCE_INPUT_SCHEMA,
        t::UNTAG_RESOURCE_INPUT_SCHEMA,
        t::UPDATE_APP_MONITOR_INPUT_SCHEMA,
        t::UPDATE_RUM_METRIC_DEFINITION_INPUT_SCHEMA,
        // The structures an input carries.
        t::APP_MONITOR_CONFIGURATION_SCHEMA,
        t::APP_MONITOR_DETAILS_SCHEMA,
        t::CUSTOM_EVENTS_SCHEMA,
        t::DEOBFUSCATION_CONFIGURATION_SCHEMA,
        t::JAVA_SCRIPT_SOURCE_MAPS_SCHEMA,
        t::METRIC_DEFINITION_REQUEST_SCHEMA,
        t::QUERY_FILTER_SCHEMA,
        t::RUM_EVENT_SCHEMA,
        t::TIME_RANGE_SCHEMA,
        t::USER_DETAILS_SCHEMA,
    ]
}
