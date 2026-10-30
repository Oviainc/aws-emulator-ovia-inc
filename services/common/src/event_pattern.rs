// SPDX-License-Identifier: CC0-1.0

//! EventBridge event patterns: parsing, structural validation, and matching an
//! event against one.
//!
//! An event pattern is a JSON document whose shape mirrors the event it
//! matches. Every field the pattern names must match (they are ANDed); a field
//! holds either a nested object, to descend into the event's own nested object,
//! or an ARRAY of candidates, of which any ONE matching is enough (they are
//! ORed). A candidate is either a literal value, compared for equality, or a
//! single-key object naming one of the content filters EventBridge documents
//! (`prefix`, `suffix`, `anything-but`, `numeric`, `exists`, `cidr`,
//! `equals-ignore-case`, `wildcard`), and `$or` at any level holds a list of
//! alternative patterns.
//!
//! The JSON itself is parsed by `serde_json`; only the matching semantics, which
//! are AWS's own and have no crate, are implemented here.
//!
//! The pattern LANGUAGE is one language wherever a service accepts it — an
//! EventBridge rule, a Pipes `FilterCriteria` filter — so it is read and matched
//! here ONCE: two services handed the same pattern and the same event cannot
//! disagree about whether it matches, or about whether the pattern is one at all.
//!
//! [`parse`] is the single gate: an operation that stores a pattern runs it, so
//! anything a store holds is a pattern [`matches_within`] understands, and a
//! pattern the service cannot match is refused at the call that submitted it
//! rather than silently never firing.

use std::net::IpAddr;

use serde_json::{Map, Value};

/// The key holding a list of alternative patterns, any one of which matching is
/// enough.
const OR: &str = "$or";

/// The nested operator `prefix` and `suffix` accept to compare without regard
/// to case, e.g. `{"prefix": {"equals-ignore-case": "abc"}}`.
const IGNORE_CASE: &str = "equals-ignore-case";

/// The comparison operators `{"numeric": [...]}` accepts.
const NUMERIC_OPERATORS: &[&str] = &["=", "<", "<=", ">", ">="];

/// The filters `{"anything-but": {...}}` may negate.
const NEGATABLE_FILTERS: &[&str] = &["prefix", "suffix", "wildcard", IGNORE_CASE];

/// The largest event pattern this matcher accepts, in bytes.
///
/// `EventPattern` carries no modelled length bound on an EventBridge rule, and a
/// stored pattern is walked on every event against every rule or filter that
/// holds one, so this is what bounds that work and what stops one call storing
/// an arbitrarily large document. EventBridge publishes no figure this emulator
/// can honour, so the bound is its own, set far above any pattern a workflow
/// writes.
pub const MAX_PATTERN_BYTES: usize = 8 * 1024;

/// How deeply an event pattern may nest.
///
/// Validation and matching are recursive, and both walk caller-supplied
/// nesting, so the depth is bounded explicitly rather than left to the JSON
/// parser's own incidental recursion cap. An event pattern mirrors the shape of
/// an event, which nests a handful of levels; this is well above that.
pub const MAX_PATTERN_DEPTH: usize = 32;

/// How much work ONE request may spend matching, in steps.
///
/// Every input to matching is size-bounded, but their PRODUCT — a request's
/// events times the patterns in force times the pattern and the observed values
/// — is not bounded by those sizes alone. This is the ONE explicit cap on the
/// total, spent down by [`MatchBudget`] across every event and pattern of a
/// request, so a single call can never occupy a worker for longer than it buys.
/// It is orders of magnitude above what any real pattern set costs: a pattern
/// field compare costs its own byte length, and a realistic rule set spends a
/// few hundred steps per event.
pub const MAX_MATCH_STEPS_PER_REQUEST: u64 = 4_000_000;

/// The work budget a match spends down.
///
/// Matching is driven by caller-supplied data on both sides, so it is bounded
/// by WORK and not only by input size: each node visited and each byte compared
/// costs a step, and an exhausted budget stops the walk. A walk that stops
/// early reports NO match, which is the safe direction: a pattern that would
/// need millions of steps is not one EventBridge's own matcher would be given,
/// and the alternative is occupying the worker indefinitely.
#[derive(Debug)]
pub struct MatchBudget {
    limit: u64,
    remaining: u64,
    stopped: bool,
}

impl MatchBudget {
    /// A budget of `steps`.
    pub fn new(steps: u64) -> Self {
        Self {
            limit: steps,
            remaining: steps,
            stopped: false,
        }
    }

    /// Whether the budget is spent, so the caller can stop offering work.
    pub fn exhausted(&self) -> bool {
        self.remaining == 0
    }

    /// Whether a charge this budget could NOT cover has cut a walk short, which
    /// is what tells an answer the walk did not finish from one it did.
    ///
    /// Distinct from [`MatchBudget::exhausted`]: a walk whose charges fit
    /// exactly leaves nothing remaining and still decided everything it was
    /// asked, so the work the budget did buy is not discarded.
    pub fn stopped(&self) -> bool {
        self.stopped
    }

    /// How much work has been charged so far — what makes the COST of a match
    /// assertable, rather than only its answer.
    pub fn spent(&self) -> u64 {
        self.limit - self.remaining
    }

    /// Charge `steps` (at least one), answering whether work may continue.
    ///
    /// A charge the budget cannot cover is not paid: it exhausts the budget and
    /// stops the walk. A charge it covers exactly IS paid and reports `true`,
    /// so work the budget did buy is never discarded at the boundary.
    fn spend(&mut self, steps: usize) -> bool {
        let steps = u64::try_from(steps).unwrap_or(u64::MAX).max(1);
        if steps > self.remaining {
            self.remaining = 0;
            self.stopped = true;
            return false;
        }
        self.remaining -= steps;
        true
    }
}

/// Parse `source` as an event pattern, refusing one this matcher cannot honour.
///
/// The `Err` is the reason, for the caller to word into its own modelled
/// refusal (`InvalidEventPatternException` for a rule).
pub fn parse(source: &str) -> Result<Value, String> {
    if source.len() > MAX_PATTERN_BYTES {
        return Err(format!(
            "event pattern is {} bytes, over the {MAX_PATTERN_BYTES}-byte limit",
            source.len()
        ));
    }
    let pattern: Value = serde_json::from_str(source)
        .map_err(|error| format!("Event pattern is not valid JSON: {error}"))?;
    validate(&pattern, 1)?;
    Ok(pattern)
}

/// Whether `event` matches `pattern`, spending `budget`.
///
/// Both must be JSON objects: an event pattern names fields, so there is
/// nothing for it to match in a scalar.
pub fn matches_within(pattern: &Value, event: &Value, budget: &mut MatchBudget) -> bool {
    let (Value::Object(fields), Value::Object(_)) = (pattern, event) else {
        return false;
    };
    if !budget.spend(fields.len()) {
        return false;
    }
    fields.iter().all(|(field, spec)| {
        if field == OR {
            return spec.as_array().is_some_and(|branches| {
                branches
                    .iter()
                    .any(|branch| matches_within(branch, event, budget))
            });
        }
        let observed = event.get(field);
        match spec {
            // A nested object descends into the event's own nested object.
            Value::Object(_) => observed.is_some_and(|value| matches_nested(spec, value, budget)),
            // An array lists candidates: any one matching is enough.
            Value::Array(candidates) => candidates
                .iter()
                .any(|candidate| candidate_matches(candidate, observed, budget)),
            // `validate` refuses anything else, so a stored pattern never
            // reaches here.
            _ => false,
        }
    })
}

// ---------------------------------------------------------------------------
// Structural validation
// ---------------------------------------------------------------------------

fn validate(pattern: &Value, depth: usize) -> Result<(), String> {
    if depth > MAX_PATTERN_DEPTH {
        return Err(format!(
            "event pattern nests deeper than {MAX_PATTERN_DEPTH} levels"
        ));
    }
    let Value::Object(fields) = pattern else {
        return Err("event pattern must be a JSON object".to_string());
    };
    if fields.is_empty() {
        return Err("event pattern must name at least one field".to_string());
    }
    for (field, spec) in fields {
        if field == OR {
            let Value::Array(branches) = spec else {
                return Err(format!("\"{OR}\" must hold an array of event patterns"));
            };
            if branches.is_empty() {
                return Err(format!("\"{OR}\" must hold at least one event pattern"));
            }
            for branch in branches {
                validate(branch, depth + 1)?;
            }
            continue;
        }
        match spec {
            Value::Object(_) => validate(spec, depth + 1)?,
            Value::Array(candidates) => {
                if candidates.is_empty() {
                    return Err(format!("\"{field}\" must list at least one value"));
                }
                for candidate in candidates {
                    validate_candidate(field, candidate)?;
                }
            }
            _ => {
                return Err(format!(
                    "\"{field}\" must hold an array of values or a nested object"
                ))
            }
        }
    }
    Ok(())
}

fn validate_candidate(field: &str, candidate: &Value) -> Result<(), String> {
    match candidate {
        Value::Object(filter) => validate_filter(field, filter),
        Value::Array(_) => Err(format!("\"{field}\" cannot list a nested array")),
        // A literal value is compared for equality; every JSON scalar can be.
        _ => Ok(()),
    }
}

fn validate_filter(field: &str, filter: &Map<String, Value>) -> Result<(), String> {
    let Some((operator, argument)) = single_entry(filter) else {
        return Err(format!(
            "\"{field}\" must name exactly one matching operator per object"
        ));
    };
    match operator.as_str() {
        "prefix" | "suffix" => {
            if affix_argument(argument).is_some() {
                Ok(())
            } else {
                Err(format!(
                    "\"{operator}\" takes a string or {{\"{IGNORE_CASE}\": <string>}}"
                ))
            }
        }
        IGNORE_CASE => require_string(operator, argument),
        "wildcard" => {
            let wildcard = argument
                .as_str()
                .ok_or_else(|| format!("\"{operator}\" takes a string"))?;
            // EventBridge refuses consecutive wildcard characters, so a pattern
            // carrying them is wrong at the call that submits it rather than at
            // the first event.
            if wildcard.contains("**") {
                return Err("consecutive wildcard characters are not supported".to_string());
            }
            Ok(())
        }
        "cidr" => {
            let spec = argument
                .as_str()
                .ok_or_else(|| format!("\"{operator}\" takes a CIDR block"))?;
            if parse_cidr(spec).is_some() {
                Ok(())
            } else {
                Err(format!("{spec:?} is not a CIDR block"))
            }
        }
        "exists" => {
            if argument.is_boolean() {
                Ok(())
            } else {
                Err(format!("\"{operator}\" takes true or false"))
            }
        }
        "numeric" => validate_numeric(argument),
        "anything-but" => validate_anything_but(argument),
        other => Err(format!(
            "\"{other}\" is not an event pattern matching operator"
        )),
    }
}

fn require_string(operator: &str, argument: &Value) -> Result<(), String> {
    if argument.is_string() {
        Ok(())
    } else {
        Err(format!("\"{operator}\" takes a string"))
    }
}

fn validate_numeric(argument: &Value) -> Result<(), String> {
    let Value::Array(terms) = argument else {
        return Err("\"numeric\" takes an array of operator/value pairs".to_string());
    };
    if terms.is_empty() || terms.len() % 2 != 0 {
        return Err("\"numeric\" takes one operator and one value per comparison".to_string());
    }
    for pair in terms.as_chunks::<2>().0 {
        let operator = pair[0]
            .as_str()
            .ok_or_else(|| "\"numeric\" takes a comparison operator first".to_string())?;
        if !NUMERIC_OPERATORS.contains(&operator) {
            return Err(format!("{operator:?} is not a numeric comparison operator"));
        }
        if !pair[1].is_number() {
            return Err(format!("\"numeric\" {operator:?} takes a number"));
        }
    }
    Ok(())
}

fn validate_anything_but(argument: &Value) -> Result<(), String> {
    match argument {
        Value::Array(excluded) => {
            if excluded.is_empty() {
                return Err("\"anything-but\" must list at least one value".to_string());
            }
            for value in excluded {
                if value.is_array() || value.is_object() {
                    return Err(
                        "\"anything-but\" lists plain values, not nested documents".to_string()
                    );
                }
            }
            Ok(())
        }
        Value::Object(filter) => match single_entry(filter) {
            Some((operator, _)) if NEGATABLE_FILTERS.contains(&operator.as_str()) => {
                validate_filter("anything-but", filter)
            }
            _ => Err(format!(
                "\"anything-but\" negates one of {}",
                NEGATABLE_FILTERS.join(", ")
            )),
        },
        Value::Null => Err("\"anything-but\" takes a value to exclude".to_string()),
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------------

/// Whether any element of `values` matches, charging a step for EVERY element
/// visited rather than one for the array as a whole.
///
/// Both sides of a match are caller-supplied, so an event field holding a large
/// array must not be able to buy more work than the budget sold it. An
/// exhausted budget stops the walk and reports no match, the direction every
/// other walk here takes.
fn any_element(
    values: &[Value],
    budget: &mut MatchBudget,
    mut matched: impl FnMut(&Value, &mut MatchBudget) -> bool,
) -> bool {
    for value in values {
        if !budget.spend(1) {
            return false;
        }
        if matched(value, budget) {
            return true;
        }
    }
    false
}

/// A nested pattern against the event's value for that field. An event field
/// holding a LIST of objects matches when any element does.
fn matches_nested(spec: &Value, observed: &Value, budget: &mut MatchBudget) -> bool {
    if !budget.spend(1) {
        return false;
    }
    match observed {
        Value::Array(values) => any_element(values, budget, |value, budget| {
            matches_nested(spec, value, budget)
        }),
        Value::Object(_) => matches_within(spec, observed, budget),
        _ => false,
    }
}

/// One candidate from a pattern field's array against the event's value for
/// that field, which may be absent.
fn candidate_matches(candidate: &Value, observed: Option<&Value>, budget: &mut MatchBudget) -> bool {
    if !budget.spend(1) {
        return false;
    }
    // `exists` is the one filter that is about the field's PRESENCE, so it is
    // answered before the absent case is rejected.
    if let Value::Object(filter) = candidate {
        if let Some((operator, argument)) = single_entry(filter) {
            if operator == "exists" {
                let present = observed.is_some_and(|value| !value.is_null());
                return argument.as_bool() == Some(present);
            }
        }
    }
    let Some(observed) = observed else {
        return false;
    };
    match observed {
        // An event field holding a list matches when any of its values does —
        // this is what makes `{"resources": ["<arn>"]}` match an event whose
        // `resources` list carries that ARN.
        Value::Array(values) => any_element(values, budget, |value, budget| {
            value_matches(candidate, value, budget)
        }),
        _ => value_matches(candidate, observed, budget),
    }
}

fn value_matches(candidate: &Value, observed: &Value, budget: &mut MatchBudget) -> bool {
    match candidate {
        Value::Object(filter) => filter_matches(filter, observed, budget),
        // Numbers compare by value, so 1, 1.0 and 1e0 are one number.
        Value::Number(wanted) => {
            if !budget.spend(1) {
                return false;
            }
            observed
                .as_f64()
                .zip(wanted.as_f64())
                .is_some_and(|(observed, wanted)| observed == wanted)
        }
        _ => {
            // An equality compare reads both values, so it costs their length.
            if !budget.spend(candidate.as_str().map_or(1, str::len)) {
                return false;
            }
            candidate == observed
        }
    }
}

fn filter_matches(filter: &Map<String, Value>, observed: &Value, budget: &mut MatchBudget) -> bool {
    // Charged on ENTRY rather than on each arm's success path, so a filter this
    // walk visits and answers on a type mismatch pays for the visit too.
    if !budget.spend(1) {
        return false;
    }
    let Some((operator, argument)) = single_entry(filter) else {
        return false;
    };
    match operator.as_str() {
        // Reading the field's presence is a step like any other node visited.
        "exists" => budget.spend(1) && argument.as_bool() == Some(!observed.is_null()),
        "prefix" => affix_matches(argument, observed, Affix::Prefix, budget),
        "suffix" => affix_matches(argument, observed, Affix::Suffix, budget),
        IGNORE_CASE => match (argument.as_str(), observed.as_str()) {
            (Some(wanted), Some(observed)) => {
                // Compared in place: no lowercased copy of either side.
                if !budget.spend(wanted.len()) {
                    return false;
                }
                observed.eq_ignore_ascii_case(wanted)
            }
            _ => false,
        },
        "wildcard" => match (argument.as_str(), observed.as_str()) {
            (Some(wildcard), Some(observed)) => wildcard_matches(wildcard, observed, budget),
            _ => false,
        },
        "cidr" => match (argument.as_str(), observed.as_str()) {
            (Some(spec), Some(observed)) => {
                if !budget.spend(spec.len()) {
                    return false;
                }
                cidr_matches(spec, observed)
            }
            _ => false,
        },
        "numeric" => numeric_matches(argument, observed, budget),
        "anything-but" => {
            // The only filter whose answer is a NEGATION, so it is the one place
            // a walk cut short could be read as a match: a walk the budget
            // stopped decided nothing, and an undecided walk reports no match.
            // A walk whose charges fitted exactly decided everything, so its
            // answer stands.
            let is_excluded = excluded(argument, observed, budget);
            !is_excluded && !budget.stopped()
        }
        _ => false,
    }
}

enum Affix {
    Prefix,
    Suffix,
}

/// The string a `prefix` / `suffix` argument compares against, and whether it
/// compares without regard to case.
fn affix_argument(argument: &Value) -> Option<(&str, bool)> {
    match argument {
        Value::String(wanted) => Some((wanted.as_str(), false)),
        Value::Object(inner) => match single_entry(inner)? {
            (operator, Value::String(wanted)) if operator == IGNORE_CASE => {
                Some((wanted.as_str(), true))
            }
            _ => None,
        },
        _ => None,
    }
}

fn affix_matches(
    argument: &Value,
    observed: &Value,
    affix: Affix,
    budget: &mut MatchBudget,
) -> bool {
    // Charged on ENTRY, so the type-mismatch answer below pays for its visit.
    if !budget.spend(1) {
        return false;
    }
    let (Some(observed), Some((wanted, ignore_case))) =
        (observed.as_str(), affix_argument(argument))
    else {
        return false;
    };
    // Only the affix's own length is ever read, whichever end it is at.
    if !budget.spend(wanted.len()) {
        return false;
    }
    let wanted = wanted.as_bytes();
    let observed = observed.as_bytes();
    // Compared as bytes, so a case-insensitive affix needs no lowercased copy
    // of the observed value. A slice that would not fit is simply absent.
    let candidate = match affix {
        Affix::Prefix => observed.get(..wanted.len()),
        Affix::Suffix => observed
            .len()
            .checked_sub(wanted.len())
            .and_then(|at| observed.get(at..)),
    };
    let Some(candidate) = candidate else {
        return false;
    };
    if ignore_case {
        candidate.eq_ignore_ascii_case(wanted)
    } else {
        candidate == wanted
    }
}

/// Whether `observed` matches the wildcard pattern `pattern`.
///
/// `*` is the only wildcard character EventBridge supports, and it stands for
/// any run of characters, including none; every other character is matched
/// literally and case-sensitively, and the pattern describes the WHOLE value.
/// `pub` because that is one grammar wherever a service accepts a wildcard
/// pattern — an EventBridge event pattern, an Amazon SNS subscription filter
/// policy — so two services handed the same pattern and the same value cannot
/// disagree about whether it matches.
///
/// The pattern is therefore a sequence of literal segments separated by `*`, and
/// a single left-to-right cursor decides the match: the first segment has to be
/// a prefix, the last a suffix, and each one in between has to occur after the
/// previous one. That is LINEAR in the two lengths — a backtracking walk would
/// be quadratic, which a caller choosing both the pattern and the value could
/// turn into seconds of CPU per candidate.
pub fn matches_wildcard(pattern: &str, observed: &str) -> bool {
    // No `*` at all: the whole pattern is one literal.
    let Some((first, tail)) = pattern.split_once('*') else {
        return observed == pattern;
    };
    // Everything after the LAST `*` has to land at the end of the value;
    // everything between the two has to occur, in order, in what is left.
    let (middle, last) = match tail.rsplit_once('*') {
        Some((middle, last)) => (Some(middle), last),
        None => (None, tail),
    };
    if observed.len() < first.len() + last.len()
        || !observed.starts_with(first)
        || !observed.ends_with(last)
    {
        return false;
    }
    // The span the inner segments have to be found in. Both ends are segment
    // boundaries that `starts_with` / `ends_with` just confirmed, so they are
    // character boundaries.
    let mut rest = &observed[first.len()..observed.len() - last.len()];
    for segment in middle.into_iter().flat_map(|middle| middle.split('*')) {
        if segment.is_empty() {
            continue;
        }
        match rest.find(segment) {
            Some(at) => rest = &rest[at + segment.len()..],
            None => return false,
        }
    }
    true
}

/// The same match, charged to one event's matching budget: both lengths are read,
/// so both are what it costs.
fn wildcard_matches(wildcard: &str, observed: &str, budget: &mut MatchBudget) -> bool {
    if !budget.spend(wildcard.len() + observed.len()) {
        return false;
    }
    matches_wildcard(wildcard, observed)
}

fn numeric_matches(argument: &Value, observed: &Value, budget: &mut MatchBudget) -> bool {
    // Charged on ENTRY, so the type-mismatch answer below pays for its visit.
    if !budget.spend(1) {
        return false;
    }
    let (Some(terms), Some(observed)) = (argument.as_array(), observed.as_f64()) else {
        return false;
    };
    if terms.is_empty() || terms.len() % 2 != 0 || !budget.spend(terms.len()) {
        return false;
    }
    // Every comparison must hold: `[">", 0, "<=", 5]` is a half-open range.
    terms.as_chunks::<2>().0.iter().all(|pair| {
        let (Some(operator), Some(wanted)) = (pair[0].as_str(), pair[1].as_f64()) else {
            return false;
        };
        match operator {
            "=" => observed == wanted,
            "<" => observed < wanted,
            "<=" => observed <= wanted,
            ">" => observed > wanted,
            ">=" => observed >= wanted,
            _ => false,
        }
    })
}

/// Whether `observed` is one of the values an `anything-but` argument excludes.
///
/// Each value visited costs a step of its own, so a long exclusion list is
/// walked no more cheaply than the pattern that carries it.
fn excluded(argument: &Value, observed: &Value, budget: &mut MatchBudget) -> bool {
    match argument {
        Value::Array(values) => any_element(values, budget, |value, budget| {
            value_matches(value, observed, budget)
        }),
        _ => value_matches(argument, observed, budget),
    }
}

/// The network and prefix length of a CIDR block, or `None` when `spec` is not
/// one.
fn parse_cidr(spec: &str) -> Option<(IpAddr, u32)> {
    let (network, length) = spec.split_once('/')?;
    let network: IpAddr = network.parse().ok()?;
    let length: u32 = length.parse().ok()?;
    let width = if network.is_ipv4() { 32 } else { 128 };
    (length <= width).then_some((network, length))
}

fn cidr_matches(spec: &str, observed: &str) -> bool {
    let (Some((network, length)), Ok(address)) = (parse_cidr(spec), observed.parse::<IpAddr>())
    else {
        return false;
    };
    match (network, address) {
        (IpAddr::V4(network), IpAddr::V4(address)) => {
            same_prefix(&network.octets(), &address.octets(), length)
        }
        (IpAddr::V6(network), IpAddr::V6(address)) => {
            same_prefix(&network.octets(), &address.octets(), length)
        }
        // A v4 address is never inside a v6 block, or the other way round.
        _ => false,
    }
}

/// Whether the first `bits` bits of two equal-length addresses agree.
fn same_prefix(network: &[u8], address: &[u8], bits: u32) -> bool {
    let whole = (bits / 8) as usize;
    if network[..whole] != address[..whole] {
        return false;
    }
    let remainder = bits % 8;
    if remainder == 0 {
        return true;
    }
    let mask = 0xffu8 << (8 - remainder);
    (network[whole] & mask) == (address[whole] & mask)
}

fn single_entry(map: &Map<String, Value>) -> Option<(&String, &Value)> {
    if map.len() != 1 {
        return None;
    }
    map.iter().next()
}
