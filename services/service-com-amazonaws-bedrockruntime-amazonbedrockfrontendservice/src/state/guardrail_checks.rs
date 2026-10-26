// SPDX-License-Identifier: CC0-1.0

//! The inline guardrail checks `InvokeGuardrailChecks` evaluates, and the text-unit
//! accounting it reports for them.
//!
//! Every check is served by an engine that runs IN PROCESS over the submitted
//! messages, so each score, offset and count below is computed from the request
//! rather than read out of it:
//!
//! * the SENSITIVE-INFORMATION check runs the PII recognizer this emulator already
//!   serves Comprehend's PII family from
//!   ([`detect_pii_spans`]), so the two services cannot disagree about what a value
//!   is, and the offsets a finding carries are that recognizer's own zero-based
//!   character offsets into the content block it was found in. The recognizer's
//!   entity catalog is this service's catalog under other names for seven entities,
//!   which [`SENSITIVE_INFORMATION_ENTITY_TYPES`] maps.
//! * the CONTENT-FILTER check runs the English profanity/abuse engine this emulator
//!   already analyses toxicity with (`rustrict`). Its categories cover three of the
//!   five modelled filter categories; the other two have no detector here, so a
//!   request naming one of them is answered about it at zero — this emulator places
//!   nothing of those two in any message.
//! * the PROMPT-ATTACK check runs a rule recognizer over the same text. A prompt
//!   attack is an INSTRUCTION to the model, published as a small set of imperative
//!   forms, so each modelled category is recognised by the forms it is published in,
//!   and a finding's severity is the evidence class of the rule that placed it.
//!
//! A check the request does not configure is never evaluated and never reported. A
//! check that IS configured answers about every category or entity type the request
//! named and about nothing else: the content filter and the prompt-attack check
//! report one entry per named category, carrying zero where they placed nothing of
//! it — which is what the recorded response of a request naming a whole category set
//! carries (`bedrock-runtime.guardrail-checks-results-mirror-requested-check-types`)
//! — and the sensitive-information check reports one entry per entity it placed,
//! because an entry of that check is a LOCATION in the text.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use regex::Regex;
use rustrict::{Censor, Type};
use service_com_amazonaws_comprehend_comprehend_20171127::ops::contains_pii_entities::detect_pii_spans;

/// The order the content filter reports its verdicts in.
///
/// It is the SERVICE's own order and not a model artefact: the model declares the
/// same five categories in another order
/// (`smithy_query(query="shape",shape="GuardrailChecksContentFilterCategory")`), the
/// generated value constant
/// ([`crate::ops::types::GUARDRAIL_CHECKS_CONTENT_FILTER_CATEGORY_VALUES`]) is
/// sorted, and the order a REFUSAL lists the same value set in
/// ([`super::CONTENT_FILTER_CATEGORY_ORDER`]) is a third one — so the order the
/// results come back in has to be stated once here rather than taken from any of
/// them. The MEMBERSHIP is the model's: the crate's own test pins this list to cover
/// exactly that value set, so a category the model adds or drops cannot go
/// unreported.
pub const CONTENT_FILTER_RESULT_ORDER: [&str; 5] =
    ["VIOLENCE", "MISCONDUCT", "HATE", "SEXUAL", "INSULTS"];

/// The order the prompt-attack check reports its verdicts in, stated here for the
/// same reason as the categories above and pinned against
/// [`crate::ops::types::GUARDRAIL_CHECKS_PROMPT_ATTACK_CATEGORY_VALUES`] by the same
/// test.
pub const PROMPT_ATTACK_RESULT_ORDER: [&str; 3] =
    ["JAILBREAK", "PROMPT_INJECTION", "PROMPT_LEAKAGE"];

/// How many characters one text unit covers.
///
/// Amazon Bedrock accounts guardrail evaluation in text units, and one text unit
/// covers up to 1,000 characters of text ("Guardrails" in Amazon Bedrock pricing),
/// so the units a check consumed is the submitted character count divided by this,
/// rounded up.
pub const CHARACTERS_PER_TEXT_UNIT: usize = 1000;

/// The most characters of submitted text one evaluation reads.
///
/// The EMULATOR's bound rather than an AWS one, and the thing that makes the work a
/// request can ask for FINITE: the three recognizers are linear in the text, so
/// without a ceiling here one request could occupy a blocking-pool slot for as long
/// as the caller cares to make its body. It is twenty-five text units
/// ([`CHARACTERS_PER_TEXT_UNIT`]), far above any conversation a client submits for an
/// inline check, and a request past it is refused before any analysis runs
/// ([`too_much_text_message`]). It bounds the number of content blocks too: the
/// modelled minimum length of a text block is one character, so a request cannot
/// carry more blocks than characters.
pub const MAX_SUBMITTED_CHARACTERS: usize = 25 * CHARACTERS_PER_TEXT_UNIT;

/// The service's message for a request carrying more text than one evaluation reads.
///
/// Both numbers come from the request and from the bound it broke, never from a
/// recorded run.
#[must_use]
pub fn too_much_text_message(characters: usize) -> String {
    format!(
        "The messages carry {characters} characters, which exceeds the maximum of \
         {MAX_SUBMITTED_CHARACTERS} characters for one request."
    )
}

/// The most sensitive-information findings one evaluation reports.
///
/// The EMULATOR's bound rather than an AWS one: one request cannot render an
/// unbounded number of findings into one response. Findings past it are dropped and
/// the result says so ([`SensitiveInformationFindings::truncated`]) rather than being
/// refused, and the bound is far above the entity count of any realistic message.
pub const MAX_SENSITIVE_INFORMATION_FINDINGS: usize = 100;

/// The text units `characters` of submitted text consume.
///
/// Every configured check reads the whole submitted text, so each one consumes the
/// same units; the modelled minimum length of a content block makes that at least
/// one unit.
#[must_use]
pub fn text_units(characters: usize) -> i32 {
    let units = characters.div_ceil(CHARACTERS_PER_TEXT_UNIT).max(1);
    i32::try_from(units).unwrap_or(i32::MAX)
}

/// One text block of a request, with where it sits in the request.
///
/// `message_index` and `content_index` are zero-based, the scale the modelled
/// `messageIndex` / `contentIndex` members carry (`range` min 0).
#[derive(Debug, Clone)]
pub struct CheckedText {
    pub message_index: usize,
    pub content_index: usize,
    pub text: String,
}

/// The checks one request configured: for each check type, the categories or entity
/// types it named, or `None` for a check it did not configure at all.
///
/// The named values are a SET: the request may name the same category or entity type
/// more than once, and each is answered once, so what a repeated name cannot do is
/// multiply the work of a scan.
#[derive(Debug, Clone, Default)]
pub struct RequestedChecks {
    pub content_filter: Option<BTreeSet<String>>,
    pub prompt_attack: Option<BTreeSet<String>>,
    pub sensitive_information: Option<BTreeSet<String>>,
}

impl RequestedChecks {
    /// Whether the request configured no check at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.content_filter.is_none()
            && self.prompt_attack.is_none()
            && self.sensitive_information.is_none()
    }
}

/// One category a check was asked about, with the severity it placed on it.
///
/// A check answers one of these per category the request named — zero severity for a
/// category it placed nothing of — so a client reading the results learns the
/// verdict for everything it asked about.
#[derive(Debug, Clone, PartialEq)]
pub struct CategoryFinding {
    /// A value of the modelled category enum of the check that answered it.
    pub category: &'static str,
    /// The severity of the placement, in the modelled 0..1 range, or zero where the
    /// check placed nothing of this category.
    pub severity_score: f64,
}

/// One sensitive-information entity the recognizer placed in the submitted text.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityFinding {
    /// A value of the modelled `GuardrailChecksSensitiveInformationEntityType` enum.
    pub entity_type: &'static str,
    pub confidence_score: f64,
    /// Zero-based character offsets into the content block the entity was found in,
    /// `end_offset` one past its last character.
    pub begin_offset: i32,
    pub end_offset: i32,
    pub message_index: i32,
    pub content_index: i32,
}

/// What the sensitive-information check found.
#[derive(Debug, Clone, Default)]
pub struct SensitiveInformationFindings {
    pub findings: Vec<EntityFinding>,
    /// Whether [`MAX_SENSITIVE_INFORMATION_FINDINGS`] cut findings the recognizer
    /// had placed.
    pub truncated: bool,
}

/// What one evaluation found, per configured check.
#[derive(Debug, Clone, Default)]
pub struct Evaluation {
    pub content_filter: Option<Vec<CategoryFinding>>,
    pub prompt_attack: Option<Vec<CategoryFinding>>,
    pub sensitive_information: Option<SensitiveInformationFindings>,
    /// The text units each configured check consumed.
    pub text_units: i32,
}

/// One served inline guardrail evaluation, as the service's own log of them
/// ([`super::ComAmazonawsBedrockruntimeAmazonbedrockfrontendserviceState::guardrail_check_evaluations`])
/// keeps it: which checks the request configured, what each of them consumed, and
/// how much they found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvaluationLogEntry {
    /// The check types the request configured, in the order
    /// [`super::GUARDRAIL_CHECK_TYPES`] lists them.
    pub checks: Vec<&'static str>,
    /// The text units each configured check consumed.
    pub text_units: i32,
    /// How many findings the evaluation reported across those checks.
    pub findings: usize,
    /// Epoch milliseconds at which the evaluation was served.
    pub at_millis: i64,
}

impl Evaluation {
    /// The check types this evaluation ran, in the order
    /// [`super::GUARDRAIL_CHECK_TYPES`] lists them.
    #[must_use]
    pub fn checks(&self) -> Vec<&'static str> {
        let ran = [
            self.content_filter.is_some(),
            self.prompt_attack.is_some(),
            self.sensitive_information.is_some(),
        ];
        super::GUARDRAIL_CHECK_TYPES
            .into_iter()
            .zip(ran)
            .filter(|(_, ran)| *ran)
            .map(|(check, _)| check)
            .collect()
    }

    /// How many placements this evaluation reported across its checks: a category
    /// result whose severity is above zero is a placement, one at zero is the
    /// answer that the check placed nothing of that category.
    #[must_use]
    pub fn findings(&self) -> usize {
        let placed = |findings: &Option<Vec<CategoryFinding>>| {
            findings.as_ref().map_or(0, |findings| {
                findings
                    .iter()
                    .filter(|finding| finding.severity_score > 0.0)
                    .count()
            })
        };
        placed(&self.content_filter)
            + placed(&self.prompt_attack)
            + self
                .sensitive_information
                .as_ref()
                .map_or(0, |found| found.findings.len())
    }

    /// This evaluation as the log entry recording it, served at `at_millis`.
    #[must_use]
    pub fn log_entry(&self, at_millis: i64) -> EvaluationLogEntry {
        EvaluationLogEntry {
            checks: self.checks(),
            text_units: self.text_units,
            findings: self.findings(),
            at_millis,
        }
    }
}

/// Run every configured check over `texts`.
///
/// Synchronous, CPU-bound work with no await of its own: a request-time caller runs
/// it off the async worker, having first refused a request carrying more than
/// [`MAX_SUBMITTED_CHARACTERS`], which is what bounds the work done here.
#[must_use]
pub fn evaluate(requested: &RequestedChecks, texts: &[CheckedText]) -> Evaluation {
    let characters: usize = submitted_characters(texts);
    Evaluation {
        content_filter: requested
            .content_filter
            .as_ref()
            .map(|categories| content_filter_findings(categories, texts)),
        prompt_attack: requested
            .prompt_attack
            .as_ref()
            .map(|categories| prompt_attack_findings(categories, texts)),
        sensitive_information: requested
            .sensitive_information
            .as_ref()
            .map(|entity_types| sensitive_information_findings(entity_types, texts)),
        text_units: text_units(characters),
    }
}

/// How many characters of text `texts` carries: what the bound
/// [`MAX_SUBMITTED_CHARACTERS`] is applied to, and what the text units are computed
/// from.
#[must_use]
pub fn submitted_characters(texts: &[CheckedText]) -> usize {
    texts.iter().map(|block| block.text.chars().count()).sum()
}

// ---------------------------------------------------------------------------
// The content filter
// ---------------------------------------------------------------------------

/// The severity reported for a category the engine placed at each of the three
/// severity levels it distinguishes.
///
/// `GuardrailChecksContentFilterResultEntry$severityScore` is modelled over 0..1
/// (`smithy_query(query="shape",shape="GuardrailChecksContentFilterResultEntry")`),
/// and the engine's verdict for a category is one of three ordered levels rather
/// than a continuous number — so each level is reported as the midpoint of its third
/// of that range. The levels are the same three the engine hands this emulator's
/// toxicity analysis, and the calibration is stated here in the type the member is
/// modelled as: this member is a `Double`, so the midpoints are computed in `f64`
/// rather than widened from the `Float` that member carries.
const MILD_SEVERITY: f64 = 1.0 / 6.0;
const MODERATE_SEVERITY: f64 = 0.5;
const SEVERE_SEVERITY: f64 = 5.0 / 6.0;

/// The modelled content-filter categories this emulator has a detector for, each
/// with the engine category that detects it.
///
/// The engine classifies English text as offensive (slurs and discriminatory
/// language), mean (demeaning, insulting) and sexual, which are the modelled
/// `HATE`, `INSULTS` and `SEXUAL` filters. The two remaining modelled categories —
/// `VIOLENCE` and `MISCONDUCT` — have no detector in this engine and no other
/// maintained engine here places them, so a request naming one of them is answered
/// about it at zero severity: the check reports a verdict for every category it was
/// asked about, and this emulator places nothing of those two in any message.
const CONTENT_FILTER_DETECTORS: [(&str, Type); 3] = [
    ("HATE", Type::OFFENSIVE),
    ("INSULTS", Type::MEAN),
    ("SEXUAL", Type::SEXUAL),
];

/// The engine category that detects `category`, or `None` for a modelled category
/// this emulator has no detector for.
fn content_filter_detector(category: &str) -> Option<Type> {
    CONTENT_FILTER_DETECTORS
        .into_iter()
        .find(|(named, _)| *named == category)
        .map(|(_, detector)| detector)
}

/// The severity the analysis places on `category`, or `None` when the engine placed
/// no content of that category in the text.
fn category_severity(analysis: Type, category: Type) -> Option<f64> {
    if analysis.is(category & Type::SEVERE) {
        Some(SEVERE_SEVERITY)
    } else if analysis.is(category & Type::MODERATE) {
        Some(MODERATE_SEVERITY)
    } else if analysis.is(category & Type::MILD) {
        Some(MILD_SEVERITY)
    } else {
        None
    }
}

/// The content-filter findings over `texts`, for the categories the request named.
///
/// One finding per named category the engine placed in ANY of the messages, carrying
/// the strongest severity it placed there — the request asks one question per
/// category about the conversation, not one per message.
/// The content-filter result over `texts`: one entry per category the request
/// named, carrying the strongest severity the analyser placed on that category in
/// ANY of the messages — the request asks one question per category about the
/// conversation, not one per message — and zero for a category it placed nothing of.
///
/// Reported in [`CONTENT_FILTER_RESULT_ORDER`], so a request naming several
/// categories gets them back in one stable order whatever order it named them in,
/// and a category it named twice comes back once.
fn content_filter_findings(
    requested: &BTreeSet<String>,
    texts: &[CheckedText],
) -> Vec<CategoryFinding> {
    let analyses: Vec<Type> = texts
        .iter()
        .map(|block| Censor::from_str(&block.text).analyze())
        .collect();

    CONTENT_FILTER_RESULT_ORDER
        .into_iter()
        .filter(|category| requested.contains(*category))
        .map(|category| {
            let severity_score = content_filter_detector(category)
                .and_then(|detector| {
                    analyses
                        .iter()
                        .filter_map(|analysis| category_severity(*analysis, detector))
                        .max_by(f64::total_cmp)
                })
                .unwrap_or(0.0);
            CategoryFinding {
                category,
                severity_score,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The prompt-attack check
// ---------------------------------------------------------------------------

/// How a rule established that a message carries a prompt attack, and therefore how
/// much severity the finding carries.
///
/// The two classes are EVIDENCE classes, not decoration: which class a finding comes
/// back with says what the recognizer actually read in the message. A finding's
/// severity is therefore the constant of its evidence class rather than a per-match
/// probability, so two messages that attack the same way are scored the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttackEvidence {
    /// The message states the attack itself: an imperative naming the instructions,
    /// the guardrails or the unrestricted persona it wants.
    Stated,
    /// The message only alludes to it: a phrase the attack families use, with no
    /// imperative of its own.
    Alluded,
}

impl AttackEvidence {
    /// The severity a finding of this evidence class carries, in the modelled 0..1
    /// range.
    fn severity(self) -> f64 {
        match self {
            Self::Stated => 0.9,
            Self::Alluded => 0.6,
        }
    }
}

/// One prompt-attack rule: a pattern over a message, the modelled category a match
/// is, and what the match establishes.
struct AttackRule {
    category: &'static str,
    evidence: AttackEvidence,
    pattern: Regex,
}

/// Every prompt-attack rule, compiled once.
///
/// The CATEGORIES are the model's own enum
/// (`GUARDRAIL_CHECKS_PROMPT_ATTACK_CATEGORY_VALUES`); the
/// FORMS are the published attack families each category names — an instruction
/// override (`PROMPT_INJECTION`), a request to shed the model's restrictions
/// (`JAILBREAK`) and a request to disclose the instructions themselves
/// (`PROMPT_LEAKAGE`) — each stated as the imperative forms those families are
/// published in. Every category has rules, so no modelled category is silent.
fn attack_rules() -> &'static [AttackRule] {
    static RULES: OnceLock<Vec<AttackRule>> = OnceLock::new();
    RULES.get_or_init(|| {
        let rule = |category: &'static str, evidence: AttackEvidence, pattern: &str| AttackRule {
            category,
            evidence,
            pattern: Regex::new(pattern).expect("a prompt-attack rule is a valid regex"),
        };
        vec![
            // An instruction override: the message tells the model to drop what it
            // was instructed with, or injects a new instruction channel into the
            // content.
            rule(
                "PROMPT_INJECTION",
                AttackEvidence::Stated,
                r"(?i)\b(?:ignore|disregard|forget|override|replace)\b[^.!?\n]{0,40}?\b(?:instructions?|rules?|directions?|guidelines?|prompt)\b",
            ),
            rule(
                "PROMPT_INJECTION",
                AttackEvidence::Stated,
                r"(?im)^\s*(?:system|assistant)\s*:",
            ),
            rule(
                "PROMPT_INJECTION",
                AttackEvidence::Alluded,
                r"(?i)\bnew\s+(?:instructions?|rules?|directions?)\b",
            ),
            // Shedding the model's restrictions, including the named personas the
            // family is published under.
            rule(
                "JAILBREAK",
                AttackEvidence::Stated,
                r"(?i)\b(?:bypass|disable|turn off|get around|work around)\b[^.!?\n]{0,40}?\b(?:guardrails?|safety|safeguards?|filters?|restrictions?|limitations?|policies)\b",
            ),
            rule(
                "JAILBREAK",
                AttackEvidence::Stated,
                r"(?i)\b(?:pretend|act|behave|respond|answer)\b[^.!?\n]{0,40}?\b(?:no (?:rules|restrictions|limits|filters|guidelines)|without (?:any )?(?:rules|restrictions|limits|filters|guidelines)|unrestricted|uncensored|unfiltered)\b",
            ),
            rule(
                "JAILBREAK",
                AttackEvidence::Alluded,
                r"(?i)\b(?:jailbreak|do anything now|dan mode|developer mode|god mode)\b",
            ),
            // Disclosing the instructions the model was given.
            rule(
                "PROMPT_LEAKAGE",
                AttackEvidence::Stated,
                r"(?i)\b(?:repeat|reveal|print|show|output|disclose|tell me|give me)\b[^.!?\n]{0,40}?\b(?:system prompt|system message|your instructions|your prompt|the prompt above|initial prompt|text above)\b",
            ),
            rule(
                "PROMPT_LEAKAGE",
                AttackEvidence::Stated,
                r"(?i)\bwhat\s+(?:are|were|is|was)\b[^.!?\n]{0,30}?\b(?:your (?:instructions|rules|guidelines)|system prompt|system message)\b",
            ),
            rule(
                "PROMPT_LEAKAGE",
                AttackEvidence::Alluded,
                r"(?i)\b(?:verbatim|word for word)\b[^.!?\n]{0,30}?\b(?:prompt|instructions?)\b",
            ),
        ]
    })
}

/// The prompt-attack result over `texts`: one entry per category the request named,
/// in [`PROMPT_ATTACK_RESULT_ORDER`], carrying the strongest evidence any rule of
/// that category placed in any message, and zero where no rule of it matched.
fn prompt_attack_findings(
    requested: &BTreeSet<String>,
    texts: &[CheckedText],
) -> Vec<CategoryFinding> {
    let mut findings: Vec<CategoryFinding> = Vec::new();
    for category in PROMPT_ATTACK_RESULT_ORDER {
        if !requested.contains(category) {
            continue;
        }
        let severity_score = attack_rules()
            .iter()
            .filter(|rule| rule.category == category)
            .filter(|rule| texts.iter().any(|block| rule.pattern.is_match(&block.text)))
            .map(|rule| rule.evidence.severity())
            .max_by(f64::total_cmp)
            .unwrap_or(0.0);
        findings.push(CategoryFinding {
            category,
            severity_score,
        });
    }
    findings
}

// ---------------------------------------------------------------------------
// The sensitive-information check
// ---------------------------------------------------------------------------

/// This service's sensitive-information entity types, each paired with the entity
/// type the shared PII recognizer reports for the same value.
///
/// The keys are the modelled catalog
/// (`GUARDRAIL_CHECKS_SENSITIVE_INFORMATION_ENTITY_TYPE_VALUES`,
/// pinned against this table by the crate's own test); the values are Comprehend's
/// catalog, which names the same entity without a country prefix for seven of them.
/// Comprehend recognises types this catalog does not name (a date, the Indian
/// identifiers), and a span of one of those is reported by neither check here,
/// because this service models no type for it.
pub const SENSITIVE_INFORMATION_ENTITY_TYPES: [(&str, &str); 31] = [
    ("ADDRESS", "ADDRESS"),
    ("AGE", "AGE"),
    ("AWS_ACCESS_KEY", "AWS_ACCESS_KEY"),
    ("AWS_SECRET_KEY", "AWS_SECRET_KEY"),
    ("CA_HEALTH_NUMBER", "CA_HEALTH_NUMBER"),
    ("CA_SOCIAL_INSURANCE_NUMBER", "CA_SOCIAL_INSURANCE_NUMBER"),
    ("CREDIT_DEBIT_CARD_CVV", "CREDIT_DEBIT_CVV"),
    ("CREDIT_DEBIT_CARD_EXPIRY", "CREDIT_DEBIT_EXPIRY"),
    ("CREDIT_DEBIT_CARD_NUMBER", "CREDIT_DEBIT_NUMBER"),
    ("DRIVER_ID", "DRIVER_ID"),
    ("EMAIL", "EMAIL"),
    (
        "INTERNATIONAL_BANK_ACCOUNT_NUMBER",
        "INTERNATIONAL_BANK_ACCOUNT_NUMBER",
    ),
    ("IP_ADDRESS", "IP_ADDRESS"),
    ("LICENSE_PLATE", "LICENSE_PLATE"),
    ("MAC_ADDRESS", "MAC_ADDRESS"),
    ("NAME", "NAME"),
    ("PASSWORD", "PASSWORD"),
    ("PHONE", "PHONE"),
    ("PIN", "PIN"),
    ("SWIFT_CODE", "SWIFT_CODE"),
    (
        "UK_NATIONAL_HEALTH_SERVICE_NUMBER",
        "UK_NATIONAL_HEALTH_SERVICE_NUMBER",
    ),
    (
        "UK_NATIONAL_INSURANCE_NUMBER",
        "UK_NATIONAL_INSURANCE_NUMBER",
    ),
    (
        "UK_UNIQUE_TAXPAYER_REFERENCE_NUMBER",
        "UK_UNIQUE_TAXPAYER_REFERENCE_NUMBER",
    ),
    ("URL", "URL"),
    ("USERNAME", "USERNAME"),
    ("US_BANK_ACCOUNT_NUMBER", "BANK_ACCOUNT_NUMBER"),
    ("US_BANK_ROUTING_NUMBER", "BANK_ROUTING"),
    (
        "US_INDIVIDUAL_TAX_IDENTIFICATION_NUMBER",
        "US_INDIVIDUAL_TAX_IDENTIFICATION_NUMBER",
    ),
    ("US_PASSPORT_NUMBER", "PASSPORT_NUMBER"),
    ("US_SOCIAL_SECURITY_NUMBER", "SSN"),
    (
        "VEHICLE_IDENTIFICATION_NUMBER",
        "VEHICLE_IDENTIFICATION_NUMBER",
    ),
];

/// This service's entity type for a span the recognizer reported, or `None` for a
/// recognizer type this service models no entity for.
fn modelled_entity_type(recognized: &str) -> Option<&'static str> {
    SENSITIVE_INFORMATION_ENTITY_TYPES
        .into_iter()
        .find(|(_, comprehend)| *comprehend == recognized)
        .map(|(modelled, _)| modelled)
}

/// The sensitive-information findings over `texts`, for the entity types the request
/// named.
///
/// Findings are in request order — by message, then by content block, then by where
/// in the block the entity starts — and are cut at
/// [`MAX_SENSITIVE_INFORMATION_FINDINGS`], where the scan STOPS rather than carrying
/// on over the blocks it has left.
fn sensitive_information_findings(
    requested: &BTreeSet<String>,
    texts: &[CheckedText],
) -> SensitiveInformationFindings {
    let mut findings: Vec<EntityFinding> = Vec::new();
    for block in texts {
        for span in detect_pii_spans(&block.text) {
            let Some(entity_type) = modelled_entity_type(span.entity_type) else {
                continue;
            };
            if !requested.contains(entity_type) {
                continue;
            }
            if findings.len() >= MAX_SENSITIVE_INFORMATION_FINDINGS {
                return SensitiveInformationFindings {
                    findings,
                    truncated: true,
                };
            }
            findings.push(EntityFinding {
                entity_type,
                confidence_score: f64::from(span.score),
                begin_offset: offset_i32(span.begin),
                end_offset: offset_i32(span.end),
                message_index: offset_i32(block.message_index),
                content_index: offset_i32(block.content_index),
            });
        }
    }
    SensitiveInformationFindings {
        findings,
        truncated: false,
    }
}

/// An offset or index as the modelled integer member carries it.
fn offset_i32(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}
