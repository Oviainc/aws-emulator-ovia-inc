// SPDX-License-Identifier: CC0-1.0

//! The `AlarmRule` expression language a composite alarm carries, and the parse that reads it.
//!
//! One concern in one file: the grammar's bounds ([`MAX_RULE_NESTING_DEPTH`],
//! [`MAX_RULE_NODES`]), the state functions a rule may test, the parsed rule ([`AlarmRule`])
//! and what it references, the refusal of a rule the grammar cannot read, and the recursive
//! descent parser itself. Evaluation of a parsed rule over the alarms this store holds is
//! [`super::evaluate_composite_alarms`]'s concern.

use super::*;

/// The deepest NESTING one `AlarmRule` may carry: the levels the parse descends through,
/// one per `(` and one per `NOT`.
///
/// The value is the EMULATOR's choice: CloudWatch's own documented rule examples group a
/// handful of levels (`(ALARM("a") OR ALARM("b")) AND NOT OK("c")` is two), so 32 is an
/// order of magnitude above any rule a caller writes, and at roughly four stack frames per
/// level it is three orders of magnitude inside a worker thread's ordinary stack.
pub(super) const MAX_RULE_NESTING_DEPTH: usize = 32;

/// The most functions and operators one `AlarmRule` may carry.
///
/// This is the general depth bound. `A OR B OR C …` parses without recursing, but builds a
/// tree as deep as the chain is long, and [`AlarmRule::evaluate`],
/// [`AlarmRule::references`] and the tree's own drop all descend with that depth — so the
/// node count is what bounds them, whatever shape the rule takes.
///
/// The value is the EMULATOR's choice, above the one quota CloudWatch publishes for a rule:
/// "The rule expression of a composite alarm can include as many as 100 underlying alarms",
/// which with its operators is a few hundred nodes.
pub(super) const MAX_RULE_NODES: usize = 1_024;

/// The three state functions a rule can carry. Each one's name is also the `StateValue` it
/// tests, which is why the parsed node keeps the name as the state.
pub(super) const RULE_STATE_FUNCTIONS: [&str; 3] = [
    ALARM_STATE_ALARM,
    ALARM_STATE_OK,
    ALARM_STATE_INSUFFICIENT_DATA,
];

/// One node of a parsed composite-alarm rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlarmRule {
    /// `TRUE` or `FALSE`.
    Literal(bool),
    /// `ALARM("x")`, `OK("x")` or `INSUFFICIENT_DATA("x")`: true when the alarm `reference`
    /// names is in `state`.
    InState {
        /// The alarm name or ARN the function names.
        reference: String,
        /// The `StateValue` the function tests for.
        state: String,
    },
    /// `NOT <rule>`.
    Not(Box<AlarmRule>),
    /// `<rule> AND <rule>`.
    And(Box<AlarmRule>, Box<AlarmRule>),
    /// `<rule> OR <rule>`.
    Or(Box<AlarmRule>, Box<AlarmRule>),
}

impl AlarmRule {
    /// Evaluate this rule, reading each referenced alarm's current `StateValue` through
    /// `state_of`.
    ///
    /// An alarm the store does not hold is in no state, so every function over it is false:
    /// the rule still has an outcome, which is what lets a composite alarm keep evaluating
    /// when a child has been deleted.
    pub fn evaluate(&self, state_of: &impl Fn(&str) -> Option<String>) -> bool {
        match self {
            Self::Literal(value) => *value,
            Self::InState { reference, state } => {
                state_of(reference).as_deref() == Some(state.as_str())
            }
            Self::Not(inner) => !inner.evaluate(state_of),
            Self::And(left, right) => left.evaluate(state_of) && right.evaluate(state_of),
            Self::Or(left, right) => left.evaluate(state_of) || right.evaluate(state_of),
        }
    }

    /// Every alarm reference this rule carries, in the order the rule names them, without
    /// repeats.
    pub fn references(&self) -> Vec<String> {
        let mut references = Vec::new();
        let mut seen = HashSet::new();
        self.collect_references(&mut references, &mut seen);
        references
    }

    /// How many nodes this rule carries — what evaluating it once costs.
    ///
    /// Bounded by the node count the parser accepted, which is what bounds this walk as it
    /// bounds [`Self::evaluate`] and [`Self::references`].
    pub fn node_count(&self) -> usize {
        match self {
            Self::Literal(_) | Self::InState { .. } => 1,
            Self::Not(inner) => 1 + inner.node_count(),
            Self::And(left, right) | Self::Or(left, right) => {
                1 + left.node_count() + right.node_count()
            }
        }
    }

    /// Collect this rule's references in order, using `seen` to skip repeats — a SET, so the
    /// walk costs one step per node rather than one scan of everything collected so far.
    pub(super) fn collect_references(&self, into: &mut Vec<String>, seen: &mut HashSet<String>) {
        match self {
            Self::Literal(_) => {}
            Self::InState { reference, .. } => {
                if seen.insert(reference.clone()) {
                    into.push(reference.clone());
                }
            }
            Self::Not(inner) => inner.collect_references(into, seen),
            Self::And(left, right) | Self::Or(left, right) => {
                left.collect_references(into, seen);
                right.collect_references(into, seen);
            }
        }
    }
}

/// The alarm NAME a rule reference names.
///
/// "You can use either alarm names or ARNs to reference the other alarms that are to be
/// evaluated." (`PutCompositeAlarmInput$AlarmRule`) — an alarm ARN's resource part is
/// `alarm:<name>` (see [`alarm_arn`]), so both forms resolve to the name the store keys on.
pub fn alarm_rule_reference_name(reference: &str) -> &str {
    if reference.starts_with("arn:") {
        if let Some((_, name)) = reference.rsplit_once(":alarm:") {
            return name;
        }
    }
    reference
}

/// The alarm names one rule references — its children — resolved from whatever form the rule
/// used, in the order the rule names them and without repeats.
pub fn alarm_rule_child_names(rule: &AlarmRule) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for reference in rule.references() {
        let name = alarm_rule_reference_name(&reference).to_string();
        if seen.insert(name.clone()) {
            names.push(name);
        }
    }
    names
}

/// A rule this parser cannot read, and why. The caller answers it as a refusal naming the
/// member, so the text is built from the request rather than fixed.
#[derive(Debug, Clone)]
pub struct AlarmRuleError {
    /// What was expected, and where in the rule.
    pub message: String,
}

/// The tokens a rule may carry where a TERM is expected, as CloudWatch lists them in the
/// diagnosis of a rule it could not parse: the open parenthesis and `NOT` that start a
/// grouped or negated term, the `AT_LEAST` quantifier, the two boolean literals, and the
/// three state functions. The list is the service's own, reproduced as it words it.
pub(super) const RULE_TERM_TOKENS: &str =
    "'(', 'NOT', AT_LEAST, TRUE or FALSE, ALARM, OK, or INSUFFICIENT_DATA";

/// What this parser states it expected where a rule ends before the term it owes: the refusal
/// class the diagnosis above has no recorded wording for.
pub(super) const TERM_EXPECTED: &str = "expected ALARM, OK, INSUFFICIENT_DATA, TRUE, FALSE, NOT or `(`";

/// One diagnosis in the envelope CloudWatch carries an `AlarmRule` refusal in, so a client
/// that classifies a refusal by its message can tell a rule it could not parse from the two
/// other refusals the same operation answers with the same error code.
pub(super) fn alarm_rule_diagnosis(diagnosis: &str) -> String {
    format!("Error in AlarmRule [{diagnosis}]")
}

impl std::fmt::Display for AlarmRuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Parse a composite alarm's `AlarmRule`, within the three bounds this module states.
pub fn parse_alarm_rule(rule: &str) -> Result<AlarmRule, AlarmRuleError> {
    if rule.len() > MAX_ALARM_RULE_LEN {
        return Err(AlarmRuleError {
            message: format!(
                "AlarmRule must not be longer than {MAX_ALARM_RULE_LEN} characters"
            ),
        });
    }
    let mut parser = RuleParser {
        text: rule,
        at: 0,
        depth: 0,
        nodes: 0,
    };
    let parsed = parser.parse_or()?;
    parser.skip_spaces();
    if !parser.rest().is_empty() {
        return Err(parser.error("expected the end of the rule"));
    }
    Ok(parsed)
}

/// A recursive-descent parser over one rule's text, tracking the byte offset it has reached
/// so a refusal can say WHERE the rule stopped making sense.
pub(super) struct RuleParser<'a> {
    pub(super) text: &'a str,
    pub(super) at: usize,
    /// How many levels the parse has descended at this point, bounded by
    /// [`MAX_RULE_NESTING_DEPTH`].
    pub(super) depth: usize,
    /// How many nodes the parse has built, bounded by [`MAX_RULE_NODES`].
    pub(super) nodes: usize,
}

impl<'a> RuleParser<'a> {
    /// The text this parser has not consumed yet.
    pub(super) fn rest(&self) -> &'a str {
        self.text.get(self.at..).unwrap_or_default()
    }

    /// Advance past any whitespace.
    pub(super) fn skip_spaces(&mut self) {
        let rest = self.rest();
        self.at += rest.len() - rest.trim_start().len();
    }

    /// Consume `ch` when it is the next non-whitespace character.
    pub(super) fn take(&mut self, ch: char) -> bool {
        self.skip_spaces();
        if self.rest().starts_with(ch) {
            self.at += ch.len_utf8();
            true
        } else {
            false
        }
    }

    /// Consume `word` when the next non-whitespace text is that word — case-insensitively,
    /// and only as a whole word, so `OK` never matches the start of an alarm name.
    pub(super) fn take_word(&mut self, word: &str) -> bool {
        self.skip_spaces();
        let rest = self.rest();
        let Some(head) = rest.get(..word.len()) else {
            return false;
        };
        if !head.eq_ignore_ascii_case(word) {
            return false;
        }
        if rest[word.len()..].starts_with(|ch: char| ch.is_alphanumeric() || ch == '_') {
            return false;
        }
        self.at += word.len();
        true
    }

    /// A refusal naming what was expected and where.
    pub(super) fn error(&self, expected: &str) -> AlarmRuleError {
        AlarmRuleError {
            message: format!("AlarmRule: {expected} at offset {}", self.at),
        }
    }

    /// The refusal for a token sitting where the rule can only carry a TERM, or `None` where
    /// the rule carries no token at that position at all — it simply STOPS before the term it
    /// owes, which is a different class, and one no recording words for this service.
    ///
    /// CloudWatch diagnoses a rule that carries such a token by naming the OFFENDING TOKEN and
    /// WHERE in the rule it sits, together with the tokens that position admits
    /// ([`RULE_TERM_TOKENS`]) — which is what tells a client it mistyped the expression rather
    /// than referenced an alarm that does not exist. Both variable parts are read off the rule
    /// the caller submitted: the token as it appears in it ([`Self::token_here`]) and its offset
    /// in CHARACTERS from the start of the rule, which is the position a client counts to in the
    /// string it sent.
    pub(super) fn unsupported_token(&self) -> Option<AlarmRuleError> {
        let token = self.token_here();
        if token.is_empty() {
            return None;
        }
        let at_char = self
            .text
            .get(..self.at)
            .unwrap_or_default()
            .chars()
            .count();
        Some(AlarmRuleError {
            message: alarm_rule_diagnosis(&format!(
                "Unsupported token '{token}' at char {at_char}, must be: {RULE_TERM_TOKENS}"
            )),
        })
    }

    /// The token the rule carries at the position this parse reached: the whole WORD where the
    /// text continues with one (`AND`, a misspelled function), otherwise the single character
    /// there (a stray operator or bracket), and nothing at all at the end of the rule — where
    /// the rule simply stops before the term it owes.
    pub(super) fn token_here(&self) -> &'a str {
        let rest = self.rest();
        let Some(first) = rest.chars().next() else {
            return "";
        };
        if first.is_alphanumeric() || first == '_' {
            let end = rest
                .find(|ch: char| !(ch.is_alphanumeric() || ch == '_'))
                .unwrap_or(rest.len());
            &rest[..end]
        } else {
            &rest[..first.len_utf8()]
        }
    }

    /// Enter one more level of nesting, refusing a rule that nests past
    /// [`MAX_RULE_NESTING_DEPTH`]. Called at the only two sites the parse recurses from, so
    /// the recursion can never grow with the request past the bound.
    pub(super) fn descend(&mut self) -> Result<(), AlarmRuleError> {
        self.depth += 1;
        if self.depth > MAX_RULE_NESTING_DEPTH {
            return Err(self.error(&format!(
                "must not nest more than {MAX_RULE_NESTING_DEPTH} levels deep"
            )));
        }
        Ok(())
    }

    /// Leave one level of nesting.
    pub(super) fn ascend(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    /// Account for one more node of the tree, refusing a rule carrying more than
    /// [`MAX_RULE_NODES`] of them — which is what bounds the DEPTH of every later walk of
    /// the tree.
    pub(super) fn node(&mut self, node: AlarmRule) -> Result<AlarmRule, AlarmRuleError> {
        self.nodes += 1;
        if self.nodes > MAX_RULE_NODES {
            return Err(self.error(&format!(
                "must not carry more than {MAX_RULE_NODES} functions and operators"
            )));
        }
        Ok(node)
    }

    pub(super) fn parse_or(&mut self) -> Result<AlarmRule, AlarmRuleError> {
        let mut left = self.parse_and()?;
        while self.take_word("OR") {
            let right = self.parse_and()?;
            left = self.node(AlarmRule::Or(Box::new(left), Box::new(right)))?;
        }
        Ok(left)
    }

    pub(super) fn parse_and(&mut self) -> Result<AlarmRule, AlarmRuleError> {
        let mut left = self.parse_unary()?;
        while self.take_word("AND") {
            let right = self.parse_unary()?;
            left = self.node(AlarmRule::And(Box::new(left), Box::new(right)))?;
        }
        Ok(left)
    }

    pub(super) fn parse_unary(&mut self) -> Result<AlarmRule, AlarmRuleError> {
        if self.take_word("NOT") {
            self.descend()?;
            let inner = self.parse_unary()?;
            self.ascend();
            return self.node(AlarmRule::Not(Box::new(inner)));
        }
        self.parse_primary()
    }

    pub(super) fn parse_primary(&mut self) -> Result<AlarmRule, AlarmRuleError> {
        if self.take('(') {
            self.descend()?;
            let inner = self.parse_or()?;
            self.ascend();
            if !self.take(')') {
                return Err(self.error("expected `)`"));
            }
            return Ok(inner);
        }
        if self.take_word("TRUE") {
            return self.node(AlarmRule::Literal(true));
        }
        if self.take_word("FALSE") {
            return self.node(AlarmRule::Literal(false));
        }
        for state in RULE_STATE_FUNCTIONS {
            if self.take_word(state) {
                let reference = self.parse_reference()?;
                return self.node(AlarmRule::InState {
                    reference,
                    state: state.to_string(),
                });
            }
        }
        // Nothing here starts a term. A rule that carries a TOKEN at this position is refused
        // with CloudWatch's own diagnosis of it — the one parse failure a recording words. A
        // rule that simply ENDS here carries no token to name, and nothing records how the
        // service words that, so it keeps this parser's own wording rather than the recorded
        // template with an empty slot in it.
        Err(self
            .unsupported_token()
            .unwrap_or_else(|| self.error(TERM_EXPECTED)))
    }

    /// The alarm a state function names: `("x")` or `(x)`, since a name "can be surrounded
    /// with double-quotes (\"), but do not have to be".
    pub(super) fn parse_reference(&mut self) -> Result<String, AlarmRuleError> {
        if !self.take('(') {
            return Err(self.error("expected `(` after a rule function"));
        }
        self.skip_spaces();
        let reference = if self.take('"') {
            let rest = self.rest();
            let Some(end) = rest.find('"') else {
                return Err(self.error("expected a closing double-quote"));
            };
            let name = rest[..end].to_string();
            self.at += end + 1;
            name
        } else {
            let rest = self.rest();
            let Some(end) = rest.find(')') else {
                return Err(self.error("expected `)` after an alarm reference"));
            };
            let name = rest[..end].trim_end().to_string();
            self.at += end;
            name
        };
        if !self.take(')') {
            return Err(self.error("expected `)` after an alarm reference"));
        }
        if reference.is_empty() {
            return Err(self.error("a rule function must name an alarm"));
        }
        Ok(reference)
    }
}
