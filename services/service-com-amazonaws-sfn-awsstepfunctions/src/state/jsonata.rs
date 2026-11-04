// SPDX-License-Identifier: CC0-1.0

//! JSONata, the other query language the Amazon States Language selects between.
//!
//! A state machine — or one state of it — declares `"QueryLanguage": "JSONata"`, and
//! every expression that state carries is then written in JSONata rather than in
//! JSONPath: its `Output`, its `Assign`, a `Task`'s `Arguments`, a `Choice` rule's
//! `Condition`, a `Map`'s `Items`, a `Wait`'s `Seconds`. The language delimits one
//! expression with `{%` and `%}`, and a field that carries no such delimiter is the
//! literal it is written as.
//!
//! The expressions are EVALUATED, through [`::service_common::jsonata`] — this
//! workspace's one bounded seam onto the maintained JSONata interpreter — rather than
//! by a hand-written subset of the language. That seam is where an engine evaluating a
//! caller's own text is CONFINED: the ceilings on one expression's text, its parse,
//! its stack, its depth, its memory, the value it builds and how many evaluations may
//! be alive at once are stated and met there, once, for every service whose API
//! carries such an expression.
//!
//! What THIS module adds is the Amazon States Language's own half: which reserved
//! terms an expression of a state reads, and what one visit to a state may ask for.
//!
//! ## What an expression reads
//!
//! `$states` is the reserved term: `$states.input` is the state's own input,
//! `$states.context` the context object, `$states.result` what the state's work
//! produced (a `Task`'s answer, a `Map`'s or `Parallel`'s gathered results) and
//! `$states.errorOutput` the error output inside a `Catch`. Each is bound only where
//! the language binds it, which is what the definition's own validation refuses an
//! expression for reading out of scope. Every variable a state `Assign`ed is bound
//! under its own name (`$total`), and the expression's context item (`$`) is the
//! state's input, so a bare field reference reads the input.
//!
//! ## What bounds one state's whole template
//!
//! A state's template is the PRODUCT of how many expressions it carries and how large
//! a value each builds, and a definition — bounded only at the modelled 1 MiB —
//! chooses both. [`Budget`] is what bounds that product, as the members are built
//! rather than after: one allowance of what a state's document may carry, spent by
//! every member the template renders, and one count of the expressions it may
//! evaluate — the two the other query language's payload template is bounded by
//! ([`super::asl::MAX_PAYLOAD_BYTES`], [`super::asl::MAX_TEMPLATE_PATHS`]).
//!
//! One allowance covers one TEMPLATE or FIELD: the state's `Assign` and the `Output`
//! those assignments feed share one, and each of the other members a state can carry
//! — a `Task`'s or `Parallel`'s `Arguments`, a `Map`'s `Items` and `ItemSelector`, a
//! `Wait`'s own members, a `Choice` rule's `Condition` — is given its own, because
//! each is a separate document the state builds. How many of those one visit can ask
//! for is fixed by the state's TYPE, and for a `Choice` by its rule count, which is
//! itself bounded ([`super::asl::MAX_CHOICE_RULES`]); each is built and spent in turn.
//! Every evaluation is also handed what is left of the interpretation's own deadline
//! ([`super::asl::MAX_INTERPRET_DURATION`]) rather than a span of its own, so the
//! blocking thread one walk can occupy is still the span that walk is allowed however
//! many expressions its states carry.

use std::collections::BTreeMap;
use std::time::SystemTime;

use serde_json::{Map, Value};

use ::service_common::jsonata as engine;

/// The term an expression reads the state's own data through: `$states.input`,
/// `$states.context`, `$states.result`, `$states.errorOutput`.
const STATES_TERM: &str = "states";

/// The members of [`STATES_TERM`], as the language spells them.
const INPUT_MEMBER: &str = "input";
const CONTEXT_MEMBER: &str = "context";
const RESULT_MEMBER: &str = "result";
const ERROR_OUTPUT_MEMBER: &str = "errorOutput";

/// What opens and closes one JSONata expression inside a definition's field.
pub const OPEN: &str = "{%";
pub const CLOSE: &str = "%}";

/// The longest ONE expression of a state is evaluated for, in milliseconds.
///
/// Emulator bound, NOT an AWS limit: a state evaluates several expressions and the
/// walk runs on a blocking thread, so one of them is bounded well inside the
/// interpretation's own deadline — and what is LEFT of that deadline clamps it further
/// ([`Budget::charge`]), so the expressions of one state cannot hold the thread past
/// the span the whole run is allowed.
pub const EVALUATION_TIME_LIMIT_MS: usize = 250;

/// What one expression of a state is evaluated with: the reserved terms the language
/// binds there, and the variables the run has assigned.
#[derive(Debug, Clone, Copy)]
pub struct Bindings<'b> {
    /// `$states.input`, which is also the expression's own context item (`$`).
    pub input: &'b Value,
    /// `$states.context`: the context object a definition reads its execution's
    /// identity through.
    pub context: &'b Value,
    /// `$states.result`: what the state's work produced, bound only in a state whose
    /// work produces one and on its success path.
    pub result: Option<&'b Value>,
    /// `$states.errorOutput`: the error output of a state whose failure was caught,
    /// bound only inside the catcher that took it.
    pub error_output: Option<&'b Value>,
    /// The variables in scope, each read under its own name (`$total`).
    pub variables: &'b BTreeMap<String, Value>,
}

impl Bindings<'_> {
    /// `$states` as a document: the members the language binds, and no member for one
    /// it does not bind here — an expression reading an unbound member reads nothing,
    /// exactly as it does on the service.
    fn states(&self) -> Value {
        let mut states = Map::new();
        states.insert(INPUT_MEMBER.to_string(), self.input.clone());
        states.insert(CONTEXT_MEMBER.to_string(), self.context.clone());
        if let Some(result) = self.result {
            states.insert(RESULT_MEMBER.to_string(), result.clone());
        }
        if let Some(error_output) = self.error_output {
            states.insert(ERROR_OUTPUT_MEMBER.to_string(), error_output.clone());
        }
        Value::Object(states)
    }
}

/// Why a state's expressions were not evaluated, in the two classes the Amazon States
/// Language reports them under.
///
/// The classes are the caller's to act on: a template that outgrew what a state may
/// carry is the language's data-limit failure, and everything else — an expression
/// that could not be parsed or evaluated, and every ceiling on the work one visit to
/// a state may do — is its query-evaluation failure.
#[derive(Debug, Clone)]
pub enum Refusal {
    /// The template built more than one state's document may carry.
    TooLarge(String),
    /// The expressions were not evaluated.
    NotEvaluated(String),
}

impl From<engine::Refused> for Refusal {
    /// A refusal of the shared seam, in this language's own two classes.
    fn from(refused: engine::Refused) -> Self {
        match refused {
            engine::Refused::TooLarge(why) => Refusal::TooLarge(why),
            engine::Refused::NotEvaluated(why) => Refusal::NotEvaluated(why),
        }
    }
}

/// What the expressions of ONE template or field may build, and may evaluate.
///
/// A template's cost is the PRODUCT of how many expressions it carries and how large
/// a value each builds, and a definition chooses both, so the product is spent from
/// one allowance as the members are built rather than measured once they are whole. A
/// state's `Assign` and the `Output` it feeds share one allowance; every other member
/// a state can carry is a document of its own and is given its own, as the module
/// header states. The deadline is the interpretation's own: every evaluation is
/// clamped to what is left of it, so a state's expressions cannot hold the blocking
/// thread the walk runs on past the span the whole run is allowed.
#[derive(Debug)]
pub struct Budget {
    bytes: usize,
    expressions: usize,
    deadline: SystemTime,
}

impl Budget {
    /// The allowance one template or field gets, against the interpretation deadline
    /// `until`.
    pub fn new(until: SystemTime) -> Self {
        Self {
            bytes: super::asl::MAX_PAYLOAD_BYTES,
            expressions: super::asl::MAX_TEMPLATE_PATHS,
            deadline: until,
        }
    }

    /// Spend `bytes` of what this visit may still build.
    fn spend(&mut self, bytes: usize) -> Result<(), Refusal> {
        match self.bytes.checked_sub(bytes) {
            Some(left) => {
                self.bytes = left;
                Ok(())
            }
            None => Err(Refusal::TooLarge(format!(
                "this state's expressions build more than the {} bytes a state's output may carry",
                super::asl::MAX_PAYLOAD_BYTES
            ))),
        }
    }

    /// Charge ONE evaluation, and answer how many milliseconds it may run for: what
    /// is left of the interpretation's deadline, and never more than one expression
    /// is allowed on its own.
    fn charge(&mut self) -> Result<usize, Refusal> {
        let Some(left) = self.expressions.checked_sub(1) else {
            return Err(Refusal::NotEvaluated(format!(
                "this template carries more than the {} expressions this service evaluates",
                super::asl::MAX_TEMPLATE_PATHS
            )));
        };
        self.expressions = left;
        let remaining = self
            .deadline
            .duration_since(SystemTime::now())
            .map(|left| usize::try_from(left.as_millis()).unwrap_or(usize::MAX))
            .unwrap_or(0);
        if remaining == 0 {
            return Err(Refusal::NotEvaluated(format!(
                "the execution was interpreted for longer than the {} seconds this emulator \
                 interprets one definition for, so its expressions were not evaluated",
                super::asl::MAX_INTERPRET_DURATION.as_secs()
            )));
        }
        Ok(remaining.min(EVALUATION_TIME_LIMIT_MS))
    }
}

/// Whether `text` is ONE whole JSONata expression — the only form a field's value is
/// evaluated in, since the language delimits an expression with [`OPEN`] and
/// [`CLOSE`] and a field carrying anything else is the literal it is written as.
pub fn is_expression(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.len() > OPEN.len() + CLOSE.len()
        && trimmed.starts_with(OPEN)
        && trimmed.ends_with(CLOSE)
}

/// The expression inside the delimiters of `text`, for a `text` [`is_expression`]
/// answers for.
fn inner(text: &str) -> &str {
    let trimmed = text.trim();
    trimmed[OPEN.len()..trimmed.len() - CLOSE.len()].trim()
}

/// Evaluate the expression `text` carries, and report the value it produced.
///
/// `Ok(None)` is an expression that resolved to NOTHING, which the language's own
/// `undefined` is: a field whose expression produces nothing carries no value, so the
/// caller drops the member rather than writing a null the definition did not ask for.
///
/// The evaluation is charged to `budget` BEFORE it is made — which is also what hands
/// the seam the time it may run for — and what remains of the visit's allowance is the
/// size the value it produces is held to, so neither the number of expressions nor the
/// size of what they build is left to the definition.
fn evaluate(
    text: &str,
    bindings: &Bindings<'_>,
    budget: &mut Budget,
) -> Result<Option<Value>, Refusal> {
    let allowed_ms = budget.charge()?;
    let expression = inner(text);
    let states = bindings.states();
    // The reserved terms and the assigned variables, as the seam binds them: an
    // expression reads them exactly as it does on the service.
    let mut variables: Vec<(&str, &Value)> = Vec::with_capacity(bindings.variables.len() + 1);
    variables.push((STATES_TERM, &states));
    for (name, value) in bindings.variables {
        variables.push((name.as_str(), value));
    }
    let answered = engine::evaluate(&engine::Expression {
        text: expression,
        input: bindings.input,
        variables: &variables,
        allowed_ms,
        allowance: budget.bytes,
    })?;
    let Some((value, built)) = answered else {
        return Ok(None);
    };
    // What the expression produced is spent from the visit's allowance, which is the
    // allowance the seam already held it to: a template of many large members is
    // stopped at the member that takes it past what a state may carry.
    budget.spend(built)?;
    Ok(Some(value))
}

/// A JSONata-mode TEMPLATE — an `Output`, an `Arguments`, an `Assign` — as the value
/// it builds.
///
/// One rule, applied to every node: a string that is one whole expression is replaced
/// by what that expression produced, and every other value is the literal it is
/// written as. An object member whose expression produced NOTHING is left out of the
/// result, which is what the language does with `undefined`; an array element that
/// produced nothing is left out for the same reason.
///
/// Every member — the literal ones included — is spent from `budget` as it is built,
/// and every expression is charged to it, so what ONE visit to a state renders is
/// bounded in bytes and in evaluations however many members the definition declares.
pub fn render(
    node: &Value,
    bindings: &Bindings<'_>,
    budget: &mut Budget,
) -> Result<Value, Refusal> {
    render_at(node, bindings, engine::MAX_EVALUATION_DEPTH, budget)
}

fn render_at(
    node: &Value,
    bindings: &Bindings<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Result<Value, Refusal> {
    let Some(depth) = depth.checked_sub(1) else {
        return Err(Refusal::NotEvaluated(format!(
            "a field nests deeper than the {} levels this emulator evaluates",
            engine::MAX_EVALUATION_DEPTH
        )));
    };
    match node {
        Value::String(text) if is_expression(text) => {
            Ok(evaluate(text, bindings, budget)?.unwrap_or(Value::Null))
        }
        Value::Array(items) => {
            let mut rendered = Vec::with_capacity(items.len());
            for item in items {
                // The comma and the brackets each element costs the document it is
                // built into, charged before the element itself is.
                budget.spend(1)?;
                match item {
                    Value::String(text) if is_expression(text) => {
                        if let Some(value) = evaluate(text, bindings, budget)? {
                            rendered.push(value);
                        }
                    }
                    other => rendered.push(render_at(other, bindings, depth, budget)?),
                }
            }
            Ok(Value::Array(rendered))
        }
        Value::Object(members) => {
            let mut rendered = Map::new();
            for (member, child) in members {
                // The member's own name, its quotes, its colon and the comma after
                // it: what the document carries for the member whatever its value is.
                budget.spend(member.len() + 4)?;
                match child {
                    Value::String(text) if is_expression(text) => {
                        if let Some(value) = evaluate(text, bindings, budget)? {
                            rendered.insert(member.clone(), value);
                        }
                    }
                    other => {
                        rendered.insert(
                            member.clone(),
                            render_at(other, bindings, depth, budget)?,
                        );
                    }
                }
            }
            Ok(Value::Object(rendered))
        }
        // A literal the template carries through: it is part of the document the
        // state will put out, so it is spent from the same allowance.
        other => {
            budget.spend(other.to_string().len())?;
            Ok(other.clone())
        }
    }
}

/// A field that is EITHER one whole expression or the literal value it is written as:
/// a `Wait`'s `Seconds`, a `Map`'s `Items`, a `Fail`'s `Error`.
///
/// `Ok(None)` is an expression that resolved to nothing, exactly as the evaluation
/// reports one.
pub fn field(
    node: &Value,
    bindings: &Bindings<'_>,
    budget: &mut Budget,
) -> Result<Option<Value>, Refusal> {
    match node {
        Value::String(text) if is_expression(text) => evaluate(text, bindings, budget),
        other => Ok(Some(other.clone())),
    }
}

/// Whether the `Condition` of one `Choice` rule holds.
///
/// "Condition — a boolean expression": a rule whose expression produces anything
/// other than a boolean is a rule the service cannot decide, so it is reported rather
/// than taken as false.
pub fn condition(
    node: &Value,
    bindings: &Bindings<'_>,
    budget: &mut Budget,
) -> Result<bool, Refusal> {
    let Value::String(text) = node else {
        return Err(Refusal::NotEvaluated(format!(
            "a 'Choice' rule's 'Condition' must be a JSONata expression, not {node}"
        )));
    };
    if !is_expression(text) {
        return Err(Refusal::NotEvaluated(format!(
            "a 'Choice' rule's 'Condition' must be a JSONata expression delimited with \
             '{OPEN}' and '{CLOSE}', and this rule carries '{}'",
            super::asl::echoed(text)
        )));
    }
    match evaluate(text, bindings, budget)? {
        Some(Value::Bool(holds)) => Ok(holds),
        // An expression that resolved to nothing decided nothing: the rule did not
        // match, which is what JSONata's own `undefined` means in a boolean position.
        None => Ok(false),
        Some(other) => Err(Refusal::NotEvaluated(format!(
            "a 'Choice' rule's 'Condition' must produce a boolean, and this one produced {other}"
        ))),
    }
}
