// SPDX-License-Identifier: CC0-1.0

//! The query language, matching, sorting, faceting, statistics and highlighting
//! that `Search` and `Suggest` compute from a domain's index.
//!
//! Nothing here touches the wire types: a parser answers a [`Matcher`] (or the
//! message of the `SearchException` the request earns), and the computations
//! answer plain data the operation handlers map onto the modelled shapes.
//!
//! # What a term matches
//!
//! A stored value is tokenized into runs of letters and digits, lower-cased: a
//! `Term` matches a document when one of the searched fields has a value with
//! that token, a `Prefix` when a token starts with it, and a `Phrase` when a
//! value's tokens contain the phrase's tokens consecutively. Matching is
//! therefore case-insensitive and whole-token, as CloudSearch's text analysis is,
//! rather than a raw substring test.
//!
//! # What a FIELD is here
//!
//! LIMITATION, stated here because it is the one place a reader can check it: a
//! field exists for this endpoint exactly when an indexed document carries a
//! value under that name. On AWS the index fields are defined out of band, by the
//! configuration service's `DefineIndexField` (still a scaffold in this
//! emulator), and a query naming a field the domain has not defined earns a
//! `SearchException`. This endpoint cannot see those definitions, so a query,
//! filter, sort, facet, statistic, projection or highlight naming a field no
//! document carries is answered as what it is — a field with no values, matching
//! no document, contributing no bucket and no statistic — rather than refused on
//! a schema this endpoint does not have. Everything it genuinely cannot evaluate
//! (`expr`, range syntax, `near`, the Lucene modifiers, a Lucene-family grouping
//! parenthesis, a matching-changing `q.options`) is refused instead, so nothing
//! here reports work it did not do.

use super::Document;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// The default page size is documented on [`super::DEFAULT_PAGE_SIZE`]; a facet
/// has the same documented default ("size: specifies the maximum number of facets
/// to include in the results. … The default is 10").
pub const DEFAULT_FACET_SIZE: usize = 10;

/// The cursor value a client sends to start a cursor-paged search, from the
/// `cursor` documentation: "To get the first cursor, set the cursor value to
/// initial."
pub const INITIAL_CURSOR: &str = "initial";

/// Opaque prefix of a cursor this endpoint mints, so a token from somewhere else
/// is refused rather than silently read as an offset.
const CURSOR_PREFIX: &str = "cs";

/// The sort field carrying a hit's relevance score.
pub const SCORE_FIELD: &str = "_score";
/// The sort field carrying a hit's document id.
pub const ID_FIELD: &str = "_id";
/// The longest `q`, `fq` or suggester query this endpoint parses.
///
/// A query string is caller-controlled and drives tokenizing, parsing and (for
/// the structured syntax) nesting, so its length is bounded BEFORE any of that
/// work starts; a longer one earns the modelled `SearchException`. The value is
/// the largest search request CloudSearch itself accepts ("Maximum size of a
/// search request: 8,190 bytes", Limits, Amazon CloudSearch Developer Guide), so
/// no query a real client can send is refused by this bound.
pub const MAX_QUERY_BYTES: usize = 8_190;

/// The deepest nesting of `(and …)`/`(or …)`/`(not …)` sub-expressions the
/// structured parser descends into.
///
/// Nesting depth is chosen by the caller and parsing (and evaluating) a nested
/// expression recurses, so the depth is bounded rather than left to grow the
/// stack: a deeper query earns the modelled `SearchException`. The bound mirrors
/// the framework's own request-nesting limit (`MAX_DEPTH` in
/// `vendor/smithy-schema-server`), scaled down because a query expression that
/// deep is already pathological.
pub const MAX_QUERY_DEPTH: usize = 32;

/// The most fields one request can name in `q.options`, `facet`, `stats` or
/// `highlight`.
///
/// Each list is caller-supplied, and every field it names costs another pass over
/// the values of the matching documents, so the count is bounded like the query
/// string itself; a longer list earns the modelled `SearchException`. CloudSearch
/// itself allows at most 200 index fields in a domain ("Maximum number of index
/// fields: 200", Limits, Amazon CloudSearch Developer Guide), and a request
/// cannot usefully name more fields than a domain can hold.
pub const MAX_QUERY_FIELDS: usize = 200;

/// The `return` value asking for every stored field (the default).
pub const ALL_FIELDS: &str = "_all_fields";
/// The `return` value asking for document ids only.
pub const NO_FIELDS: &str = "_no_fields";

/// The four query parsers the modelled `QueryParser` enum declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parser {
    /// Search every field for the terms of the query string.
    Simple,
    /// The CloudSearch structured query language.
    Structured,
    /// The Apache Lucene query syntax.
    Lucene,
    /// The DisMax subset of the Lucene syntax.
    Dismax,
}

impl Parser {
    /// The parser a request's `q.parser` names; the `simple` parser when it names
    /// none ("If queryParser is not specified, Amazon CloudSearch uses the simple
    /// query parser").
    pub fn from_wire(value: Option<&str>) -> Result<Self, String> {
        match value {
            None => Ok(Self::Simple),
            Some("simple") => Ok(Self::Simple),
            Some("structured") => Ok(Self::Structured),
            Some("lucene") => Ok(Self::Lucene),
            Some("dismax") => Ok(Self::Dismax),
            Some(other) => Err(format!(
                "Invalid q.parser value '{other}'; the supported query parsers are \
                 simple, structured, lucene and dismax."
            )),
        }
    }
}

/// How individual terms of a term-syntax query combine when the query names no
/// operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Operator {
    /// Every term must match (CloudSearch's default operator).
    #[default]
    And,
    /// Any term may match.
    Or,
}

/// The `q.options` a request configures its parser with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueryOptions {
    /// `fields`: the fields a field-less term searches, instead of all of them.
    ///
    /// Held as a shared slice, so the leaf matcher of each of a query's terms
    /// REFERENCES the one list the request sent rather than owning a copy of it:
    /// otherwise the memory one request costs is the product of its term count
    /// and its field count.
    pub fields: Option<Arc<[String]>>,
    /// `defaultOperator`: how the terms of one query combine.
    pub default_operator: Operator,
}

/// Parse the `q.options` JSON object.
///
/// Only the two options that change which documents match are honoured; the
/// remaining documented options tune analysis and scoring, which this index does
/// not model, so they are accepted and have no effect.
pub fn parse_query_options(spec: Option<&str>) -> Result<QueryOptions, String> {
    let Some(spec) = spec.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(QueryOptions::default());
    };
    // The same bound as `q` and `fq`, for the same reason: the parameter is
    // caller-supplied and is parsed before anything else happens.
    guard_query_size("q.options", spec)?;
    let value: serde_json::Value = serde_json::from_str(spec)
        .map_err(|e| format!("The q.options parameter is not valid JSON: {e}."))?;
    let object = value
        .as_object()
        .ok_or_else(|| "The q.options parameter must be a JSON object.".to_string())?;
    let mut options = QueryOptions::default();
    if let Some(fields) = object.get("fields") {
        let fields = fields
            .as_array()
            .ok_or_else(|| "The q.options 'fields' option must be an array.".to_string())?;
        if fields.len() > MAX_QUERY_FIELDS {
            return Err(format!(
                "The q.options 'fields' option names {} fields; a query can search \
                 at most {MAX_QUERY_FIELDS} fields.",
                fields.len()
            ));
        }
        let mut names = Vec::with_capacity(fields.len());
        for field in fields {
            let name = field.as_str().ok_or_else(|| {
                "Every entry of the q.options 'fields' option must be a field name.".to_string()
            })?;
            // A field may carry a boost (`title^2`), which changes ranking only.
            let name = name.split('^').next().unwrap_or(name).trim();
            if !name.is_empty() {
                names.push(name.to_string());
            }
        }
        if !names.is_empty() {
            options.fields = Some(names.into());
        }
    }
    if let Some(operator) = object.get("defaultOperator") {
        let operator = operator.as_str().ok_or_else(|| {
            "The q.options 'defaultOperator' option must be a string.".to_string()
        })?;
        options.default_operator = match operator.trim().to_ascii_lowercase().as_str() {
            "and" => Operator::And,
            "or" => Operator::Or,
            // The dismax parser states the default operator as the percentage of
            // terms that must match: 0% is OR, 100% is AND.
            percent if percent.ends_with('%') => {
                let value: f64 = percent
                    .trim_end_matches('%')
                    .parse()
                    .map_err(|_| format!("Invalid defaultOperator value '{operator}'."))?;
                if value >= 100.0 {
                    Operator::And
                } else {
                    Operator::Or
                }
            }
            _ => {
                return Err(format!(
                    "Invalid defaultOperator value '{operator}'; the supported values \
                     are 'and', 'or' and a percentage."
                ))
            }
        };
    }
    // An option that changes WHICH documents match is refused rather than
    // accepted and ignored, which would answer a result set the request did not
    // ask for:
    //
    // * `operators` turns the term syntax's `+`/`-`/`|`/`*` handling on and off,
    //   and this endpoint always evaluates those operators;
    // * a non-zero `phraseSlop`/`explicitPhraseSlop` lets a phrase match with
    //   other words between its terms, and this endpoint matches a phrase as
    //   consecutive tokens only. A slop of 0 asks for exactly that, so it is
    //   accepted.
    //
    // Every remaining documented option (`weights`, `tieBreaker`,
    // `phraseFields`, `bq`, …) boosts the RANKING of documents that already
    // match, which this index does not model, so it is accepted and has no
    // effect on the hits or their order beyond the modelled `sort`.
    if object.contains_key("operators") {
        return Err(
            "The q.options 'operators' option is not supported by this endpoint.".to_string(),
        );
    }
    for option in ["phraseSlop", "explicitPhraseSlop"] {
        let Some(value) = object.get(option) else {
            continue;
        };
        let slop = value.as_f64().ok_or_else(|| {
            format!("The q.options '{option}' option must be a number.")
        })?;
        if slop != 0.0 {
            return Err(format!(
                "The q.options '{option}' option is not supported by this endpoint; \
                 a phrase matches as consecutive terms, so only a slop of 0 is \
                 supported."
            ));
        }
    }
    Ok(options)
}

/// Which of a document's values a leaf matcher reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Every field of the document.
    All,
    /// Only the named fields. SHARED, so the leaves of one query's many terms
    /// reference one field list instead of each owning a copy.
    Fields(Arc<[String]>),
}

impl Target {
    /// A target reading only `name`.
    fn field(name: &str) -> Self {
        Self::Fields(Arc::from(vec![name.to_string()]))
    }
}

/// The most leaf matchers one query may produce — one term, prefix or phrase to
/// look for.
///
/// Each leaf reads every searched value of every document, so the evaluation work
/// of one request is (leaves × index), a PRODUCT of two caller-controlled
/// numbers. Bounding the query string alone does not bound it: an 8 KB `q` of
/// one-letter terms yields thousands of leaves. A query with more leaves than
/// this earns the modelled `SearchException`; no query a client writes by hand
/// comes close.
pub const MAX_QUERY_TERMS: usize = 64;

/// The most work one `Search` request spends reading the index: matching its
/// query, and then counting facets, computing statistics and highlighting over
/// the documents that matched.
///
/// Counted in bytes of searched text compared, the unit the work is actually made
/// of. The scale follows from the other bounds: the largest query this endpoint
/// accepts holds [`MAX_QUERY_TERMS`] leaves, and the largest index a domain holds
/// is [`super::MAX_DOCUMENTS_PER_DOMAIN`] documents, so matching a worst-case
/// legitimate query over a full index of documents carrying a few hundred bytes
/// of searched text each costs on the order of 64 × 10_000 × 500 = 320 million.
/// This budget leaves that room and refuses anything beyond it with the modelled
/// `SearchException`, so one request cannot pin a worker on an unbounded
/// CPU-bound scan — which a wall-clock timeout could not preempt, because the
/// scan is synchronous.
///
/// What it costs in TIME: the charged unit is a byte compared inside a token
/// comparison, which runs at memory-bandwidth speed, so the ceiling is on the
/// order of a second of CPU for a request that spends all of it — the cost of
/// the largest legitimate query over the largest index this endpoint holds, not
/// of a cheap one, which spends a few thousand. The scan yields every
/// [`SCAN_YIELD_INTERVAL`] documents, so even a request that spends the whole
/// budget stays preemptible by a cancellation or a request timeout.
pub const MAX_QUERY_STEPS: usize = 512_000_000;

/// How many documents a scan reads between yields to the runtime.
///
/// Matching is synchronous, so without a yield point a cancellation or a request
/// timeout has nowhere to land while a long scan runs.
pub const SCAN_YIELD_INTERVAL: usize = 256;

/// The work one request has left to spend on reading the index.
///
/// Counted in bytes of searched text compared, the unit the work is actually made
/// of: comparing an n-byte token costs n, and a phrase of k tokens costs k times
/// that, because that is what comparing it does. Every phase that reads a
/// document — matching, faceting, statistics, highlighting — charges this budget,
/// so the PRODUCT of the request's terms, fields and the index size is bounded,
/// not each factor alone. A request that runs out is refused rather than served
/// slowly for ever.
#[derive(Debug)]
pub struct Budget {
    remaining: usize,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            remaining: MAX_QUERY_STEPS,
        }
    }
}

impl Budget {
    /// A fresh budget of [`MAX_QUERY_STEPS`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Spend `steps`, or answer the message of the `SearchException` a request
    /// that has run out earns. Work that exactly fits the budget is served.
    fn spend(&mut self, steps: usize) -> Result<(), String> {
        if steps > self.remaining {
            return Err(format!(
                "The request needs more than {MAX_QUERY_STEPS} bytes of comparison \
                 over this domain's documents; narrow the query, or request fewer \
                 fields, facets or statistics."
            ));
        }
        self.remaining -= steps;
        Ok(())
    }

    /// Charge reading `values` once each: the form a phase that scans stored
    /// values outside the matcher (Suggest's prefix scan, faceting, statistics)
    /// uses, so every index-reading phase draws on one budget.
    pub fn charge_scan(&mut self, values: &[String]) -> Result<(), String> {
        self.charge(values, 1)
    }

    /// Charge reading `values`: their total length, times `each` — 1 for a single
    /// comparison per value, k for a comparison repeated k times, as a phrase of
    /// k tokens does.
    fn charge(&mut self, values: &[String], each: usize) -> Result<(), String> {
        let bytes: usize = values
            .iter()
            // Reading an empty value still costs something.
            .map(|value| value.len().max(1))
            .sum();
        self.spend(bytes.saturating_mul(each.max(1)))
    }
}

/// A document with the tokens of each of its values computed ONCE.
///
/// A query holds up to [`MAX_QUERY_TERMS`] leaves and each leaf reads the same
/// values, so tokenizing per (leaf, value) would redo the same work for every
/// term of the query. The tokens are computed once per document per request and
/// every leaf reads them.
#[derive(Debug)]
pub struct Indexed {
    document: Document,
    /// Field name -> the tokens of each of that field's values, in value order.
    tokens: BTreeMap<String, Vec<Vec<String>>>,
}

impl Indexed {
    /// Tokenize `document`'s values for one request's scan.
    pub fn new(document: Document) -> Self {
        let tokens = document
            .fields
            .iter()
            .map(|(field, values)| {
                (
                    field.clone(),
                    values.iter().map(|value| value_tokens(value)).collect(),
                )
            })
            .collect();
        Self { document, tokens }
    }

    /// The document itself, once the scan is done with its tokens.
    pub fn into_document(self) -> Document {
        self.document
    }

    /// The token list of each value `target` reads.
    fn token_lists(&self, target: &Target) -> Vec<&Vec<String>> {
        match target {
            Target::All => self.tokens.values().flatten().collect(),
            Target::Fields(fields) => fields
                .iter()
                .filter_map(|field| self.tokens.get(field))
                .flatten()
                .collect(),
        }
    }
}

/// A parsed query: what a document must look like to match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Matcher {
    /// Every document in the index (`matchall`).
    MatchAll,
    /// No document — what an empty term list reduces to.
    MatchNone,
    /// A whole token.
    Term { target: Target, token: String },
    /// A token prefix.
    Prefix { target: Target, prefix: String },
    /// Consecutive tokens within one value.
    Phrase { target: Target, tokens: Vec<String> },
    /// Every child must match.
    And(Vec<Matcher>),
    /// At least one child must match.
    Or(Vec<Matcher>),
    /// The child must not match.
    Not(Box<Matcher>),
}

impl Matcher {
    /// How many leaf matchers this tree holds: the number of times evaluating it
    /// reads the searched values of one document.
    fn leaf_count(&self) -> usize {
        match self {
            Self::MatchAll | Self::MatchNone => 0,
            Self::Term { .. } | Self::Prefix { .. } | Self::Phrase { .. } => 1,
            Self::And(children) | Self::Or(children) => {
                children.iter().map(Self::leaf_count).sum()
            }
            Self::Not(child) => child.leaf_count(),
        }
    }

    /// The widest phrase this tree looks for, in tokens: a phrase of k tokens
    /// compares k tokens per window, so its width is a cost factor of its own.
    fn max_phrase_tokens(&self) -> usize {
        match self {
            Self::MatchAll | Self::MatchNone | Self::Term { .. } | Self::Prefix { .. } => 0,
            Self::Phrase { tokens, .. } => tokens.len(),
            Self::And(children) | Self::Or(children) => children
                .iter()
                .map(Self::max_phrase_tokens)
                .max()
                .unwrap_or_default(),
            Self::Not(child) => child.max_phrase_tokens(),
        }
    }

    /// Whether `hit` matches, spending `budget`.
    pub fn matches(&self, hit: &Indexed, budget: &mut Budget) -> Result<bool, String> {
        Ok(self.evaluate(hit, budget)?.0)
    }

    /// Whether `hit` matches and its relevance score: how many times the query's
    /// terms occur in the fields the query searched. `matchall` scores every
    /// document 0, as CloudSearch does for a query that states no criteria.
    #[allow(clippy::cast_precision_loss)]
    pub fn evaluate_hit(&self, hit: &Indexed, budget: &mut Budget) -> Result<(bool, f64), String> {
        let (matched, occurrences) = self.evaluate(hit, budget)?;
        Ok((matched, occurrences as f64))
    }

    /// Whether `hit` matches, and how many times this matcher's terms occur in
    /// it.
    ///
    /// Both answers come out of ONE walk of the matcher tree: a compound
    /// operator asks each child once, instead of asking it whether it matches and
    /// then asking it again how often, which would re-descend every subtree.
    /// The tree's depth is bounded by [`MAX_QUERY_DEPTH`] and its leaf count by
    /// [`MAX_QUERY_TERMS`] at parse time, and every comparison is charged to
    /// `budget`, so this walk is bounded in depth, in width and in work.
    fn evaluate(&self, hit: &Indexed, budget: &mut Budget) -> Result<(bool, usize), String> {
        match self {
            Self::MatchAll => Ok((true, 0)),
            Self::MatchNone => Ok((false, 0)),
            Self::Term { target, token } => {
                let mut count = 0;
                for tokens in hit.token_lists(target) {
                    budget.charge(tokens, 1)?;
                    count += count_tokens(tokens, |candidate| candidate == token);
                }
                Ok((count > 0, count))
            }
            Self::Prefix { target, prefix } => {
                let mut count = 0;
                for tokens in hit.token_lists(target) {
                    budget.charge(tokens, 1)?;
                    count += count_tokens(tokens, |candidate| candidate.starts_with(prefix.as_str()));
                }
                Ok((count > 0, count))
            }
            Self::Phrase { target, tokens: phrase } => {
                let mut count = 0;
                for tokens in hit.token_lists(target) {
                    // A phrase of k tokens compares k tokens per window, so it
                    // costs k times what one comparison per token does.
                    budget.charge(tokens, phrase.len())?;
                    count += count_phrases(tokens, phrase);
                }
                Ok((count > 0, count))
            }
            Self::And(children) => {
                let mut matched = true;
                let mut occurrences = 0;
                for child in children {
                    let (child_matched, child_occurrences) = child.evaluate(hit, budget)?;
                    matched &= child_matched;
                    if child_matched {
                        occurrences += child_occurrences;
                    }
                }
                Ok((matched, occurrences))
            }
            Self::Or(children) => {
                let mut matched = false;
                let mut occurrences = 0;
                for child in children {
                    let (child_matched, child_occurrences) = child.evaluate(hit, budget)?;
                    matched |= child_matched;
                    if child_matched {
                        occurrences += child_occurrences;
                    }
                }
                Ok((matched, occurrences))
            }
            // A prohibited term is what a hit must NOT contain, so it contributes
            // nothing to the hit's relevance.
            Self::Not(child) => Ok((!child.evaluate(hit, budget)?.0, 0)),
        }
    }

    /// The terms this query looks for, for highlighting. Terms under a `not` are
    /// left out: they are what a hit must NOT contain.
    pub fn terms(&self) -> Vec<String> {
        match self {
            Self::MatchAll | Self::MatchNone | Self::Not(_) => Vec::new(),
            Self::Term { token, .. } => vec![token.clone()],
            Self::Prefix { prefix, .. } => vec![prefix.clone()],
            Self::Phrase { tokens, .. } => vec![tokens.join(" ")],
            Self::And(children) | Self::Or(children) => {
                children.iter().flat_map(Self::terms).collect()
            }
        }
    }
}

/// The tokens of a stored value: runs of letters and digits, lower-cased.
pub fn value_tokens(value: &str) -> Vec<String> {
    value
        .split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// How many of `tokens` satisfy `predicate`.
fn count_tokens(tokens: &[String], predicate: impl Fn(&str) -> bool) -> usize {
    tokens
        .iter()
        .filter(|token| predicate(token))
        .count()
}

/// How many times `phrase` occurs as consecutive entries of `tokens`.
fn count_phrases(tokens: &[String], phrase: &[String]) -> usize {
    if phrase.is_empty() || tokens.len() < phrase.len() {
        return 0;
    }
    tokens
        .windows(phrase.len())
        .filter(|window| *window == phrase)
        .count()
}

/// Build the leaf matcher for one raw query value.
///
/// A value that tokenizes to several tokens is a phrase, so `genre:'science
/// fiction'` and the bare text `science fiction` look for the same thing.
fn leaf(target: Target, raw: &str) -> Matcher {
    let prefix = raw.ends_with('*');
    let raw = raw.trim_end_matches('*');
    let mut tokens = value_tokens(raw);
    match tokens.len() {
        0 => Matcher::MatchNone,
        1 => {
            let token = tokens.remove(0);
            if prefix {
                Matcher::Prefix { target, prefix: token }
            } else {
                Matcher::Term { target, token }
            }
        }
        _ => Matcher::Phrase { target, tokens },
    }
}

/// The target a field-less term searches, given the request's `q.options`.
///
/// Cloning a `Fields` target is a reference-count bump, not a copy of the list.
fn default_target(options: &QueryOptions) -> Target {
    match &options.fields {
        Some(fields) => Target::Fields(Arc::clone(fields)),
        None => Target::All,
    }
}

/// Refuse a request naming more than [`MAX_QUERY_FIELDS`] fields in one
/// parameter.
fn guard_field_count(parameter: &str, fields: usize) -> Result<(), String> {
    if fields > MAX_QUERY_FIELDS {
        return Err(format!(
            "The {parameter} parameter names {fields} fields; a request can name at \
             most {MAX_QUERY_FIELDS} fields."
        ));
    }
    Ok(())
}

/// Refuse a query parameter longer than [`MAX_QUERY_BYTES`], before anything
/// parses it. `parameter` names the request parameter the refusal is about.
pub fn guard_query_size(parameter: &str, query: &str) -> Result<(), String> {
    if query.len() > MAX_QUERY_BYTES {
        return Err(format!(
            "The {parameter} parameter is {} bytes; a query cannot exceed \
             {MAX_QUERY_BYTES} bytes.",
            query.len()
        ));
    }
    Ok(())
}

/// Refuse a query that would read the index more times than
/// [`MAX_QUERY_TERMS`], or that looks for a phrase of more than
/// [`MAX_QUERY_TERMS`] tokens, after it is parsed and before it is evaluated.
fn guard_query_complexity(parameter: &str, matcher: &Matcher) -> Result<(), String> {
    let leaves = matcher.leaf_count();
    if leaves > MAX_QUERY_TERMS {
        return Err(format!(
            "The {parameter} parameter looks for {leaves} terms; a query can look \
             for at most {MAX_QUERY_TERMS}."
        ));
    }
    let phrase = matcher.max_phrase_tokens();
    if phrase > MAX_QUERY_TERMS {
        return Err(format!(
            "The {parameter} parameter looks for a phrase of {phrase} terms; a \
             phrase can hold at most {MAX_QUERY_TERMS} terms."
        ));
    }
    Ok(())
}

/// Parse `query` with `parser`.
pub fn parse_query(
    parser: Parser,
    query: &str,
    options: &QueryOptions,
) -> Result<Matcher, String> {
    guard_query_size("q", query)?;
    let matcher = match parser {
        Parser::Structured => parse_structured_expression(query),
        // The simple and dismax parsers search fields for terms and do not name
        // fields in the query string; lucene names them as `field:value`. The
        // two Lucene-family parsers also DEFINE the modifier syntax the simple
        // parser treats as ordinary text, so only they refuse it.
        Parser::Simple => parse_terms(query, options, false, false),
        Parser::Dismax => parse_terms(query, options, false, true),
        Parser::Lucene => parse_terms(query, options, true, true),
    }?;
    guard_query_complexity("q", &matcher)?;
    Ok(matcher)
}

/// Parse a term-syntax query: whitespace-separated terms, `"quoted phrases"`,
/// `+required` and `-prohibited` terms, `trailing*` prefixes, `|` for OR, and —
/// when `fielded` — `field:value` clauses.
///
/// Boolean precedence follows the syntax it implements: AND binds tighter than
/// OR, so `a AND b OR c` matches `(a AND b) OR c`, and a term joined by neither
/// operator takes the request's `defaultOperator`. A `-term`, `!term` or
/// `NOT term` is prohibited across the whole query, as the term syntax defines.
///
/// `lucene_syntax` says the parser is one of the Lucene family, whose syntax
/// gives `^`, `~`, an interior wildcard, `/regex/` and a GROUPING parenthesis a
/// meaning this endpoint does not evaluate: those are REFUSED rather than read as
/// ordinary text, which would answer a wrong result set with a success status.
/// The simple parser gives them no meaning, so there they stay text, which is
/// what its own text analysis does with them.
fn parse_terms(
    query: &str,
    options: &QueryOptions,
    fielded: bool,
    lucene_syntax: bool,
) -> Result<Matcher, String> {
    // The positive terms, grouped: AND binds tighter than OR, as the Lucene
    // syntax defines and the simple syntax's `|` implies, so each group is an
    // AND-list and the groups combine with OR. A flat list with one "any OR"
    // flag would answer `(a AND b) OR c` as `a OR b OR c` — a result set the
    // request did not ask for.
    let mut groups: Vec<Vec<Matcher>> = Vec::new();
    let mut negative: Vec<Matcher> = Vec::new();
    // The operator joining a term to the one before it: the one the query names
    // between them, or the request's default when it names none.
    let mut pending_join: Option<Operator> = None;
    // A `NOT` keyword negates the term that FOLLOWS it, so the flag travels to
    // the next term rather than applying to the keyword itself.
    let mut pending_negate = false;
    for raw in split_terms(query)? {
        // A GROUPING parenthesis re-associates the booleans around it
        // (`(a OR b) AND c` is not `a OR (b AND c)`), and this endpoint's term
        // parser combines its clauses flatly, so a grouped query is refused
        // rather than read with `(a` as a term and its operators re-associated —
        // which would answer a plausible result set the request never asked for.
        // Only OUTSIDE a quoted phrase: inside one the character is literal text
        // of the phrase, which this endpoint does evaluate.
        if lucene_syntax && raw.grouping {
            return Err(format!(
                "The query term '{}' uses grouping parentheses, which this \
                 endpoint does not evaluate; remove them, or express the \
                 grouping with the structured query parser.",
                raw.text
            ));
        }
        let mut text = raw.text.as_str();
        let mut negate = std::mem::take(&mut pending_negate);
        let mut join = pending_join.take();
        loop {
            match text.chars().next() {
                Some('-' | '!') => {
                    negate = true;
                    text = &text[1..];
                }
                Some('+') => text = &text[1..],
                Some('|') => {
                    join = Some(Operator::Or);
                    text = &text[1..];
                }
                _ => break,
            }
        }
        let upper = text.to_ascii_uppercase();
        if upper == "OR" {
            pending_join = Some(Operator::Or);
            continue;
        }
        if upper == "AND" {
            pending_join = Some(Operator::And);
            continue;
        }
        if upper == "NOT" {
            pending_negate = true;
            continue;
        }
        let (target, value) = match text.split_once(':') {
            Some((field, value)) if fielded && !field.is_empty() && !value.is_empty() => {
                (Target::field(field), value)
            }
            _ => (default_target(options), text),
        };
        reject_range_syntax(value)?;
        if lucene_syntax {
            reject_unevaluated_lucene_syntax(value)?;
        }
        let matcher = leaf(target, value);
        if matches!(matcher, Matcher::MatchNone) {
            continue;
        }
        if negate {
            negative.push(matcher);
            continue;
        }
        let join = join.unwrap_or(options.default_operator);
        match groups.last_mut() {
            Some(group) if join == Operator::And => group.push(matcher),
            _ => groups.push(vec![matcher]),
        }
    }
    if groups.is_empty() && negative.is_empty() {
        return Err("The q parameter must specify at least one search term.".to_string());
    }
    let mut clauses: Vec<Matcher> = groups
        .into_iter()
        .map(|group| {
            if group.len() == 1 {
                group.into_iter().next().unwrap_or(Matcher::MatchAll)
            } else {
                Matcher::And(group)
            }
        })
        .collect();
    let matched = match clauses.len() {
        0 => Matcher::MatchAll,
        1 => clauses.pop().unwrap_or(Matcher::MatchAll),
        _ => Matcher::Or(clauses),
    };
    if negative.is_empty() {
        Ok(matched)
    } else {
        Ok(Matcher::And(vec![
            matched,
            Matcher::Not(Box::new(Matcher::Or(negative))),
        ]))
    }
}

/// One term of a term-syntax query, as [`split_terms`] read it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct SplitTerm {
    /// The term's text, with the quotes of a `"quoted phrase"` removed.
    text: String,
    /// A `(` or `)` appeared in this term OUTSIDE a quoted phrase, which is
    /// where the Lucene family gives it its grouping meaning. Inside a phrase it
    /// is literal text and this flag stays false.
    grouping: bool,
}

/// Split a term-syntax query into its terms, keeping a `"quoted phrase"` whole.
///
/// Only the double quote groups a phrase, as the term syntax documents; an
/// apostrophe is part of the word it sits in (`don't`), so it does not open a
/// quote that the rest of the query never closes.
///
/// A quote the query never closes is a REFUSAL, not a phrase running to the end
/// of the query: the structured syntax refuses the same mistake, and the two
/// surfaces state one rule rather than answering a narrower result set for a
/// query the caller mistyped.
fn split_terms(query: &str) -> Result<Vec<SplitTerm>, String> {
    let mut terms: Vec<SplitTerm> = Vec::new();
    let mut current = SplitTerm::default();
    let mut quoted = false;
    for c in query.chars() {
        match c {
            '"' => quoted = !quoted,
            c if c.is_whitespace() && !quoted => {
                // `grouping` is only ever set when a character is pushed, so an
                // EMPTY term carries nothing to remember and is simply dropped.
                if !current.text.is_empty() {
                    terms.push(std::mem::take(&mut current));
                }
            }
            c => {
                if !quoted && (c == '(' || c == ')') {
                    current.grouping = true;
                }
                current.text.push(c);
            }
        }
    }
    if quoted {
        return Err("The query has an unterminated quoted value.".to_string());
    }
    if !current.text.is_empty() {
        terms.push(current);
    }
    Ok(terms)
}

/// Refuse the range syntax, which this endpoint does not evaluate, instead of
/// reading `{2000,2010]` as a term and answering a wrong result set.
fn reject_range_syntax(value: &str) -> Result<(), String> {
    if value.starts_with('[') || value.starts_with('{') {
        return Err(format!(
            "The range expression '{value}' is not supported by this endpoint."
        ));
    }
    Ok(())
}

/// Refuse the Lucene-family modifiers this endpoint does not evaluate — a boost
/// (`title^2`), a fuzzy or proximity distance (`hamlet~2`), a wildcard inside a
/// term (`h?mlet`, `h*t`) and a regular expression (`/h.*t/`) — instead of
/// tokenizing them into part of the searched text and answering a result set the
/// request did not ask for. A TRAILING `*` is the prefix search the endpoint does
/// evaluate, so it is left alone.
fn reject_unevaluated_lucene_syntax(value: &str) -> Result<(), String> {
    let unevaluated = |what: &str| {
        Err(format!(
            "The query term '{value}' uses {what}, which this endpoint does not \
             evaluate; remove it or use the simple query parser."
        ))
    };
    if value.len() > 1 && value.starts_with('/') && value.ends_with('/') {
        return unevaluated("a regular expression");
    }
    if value.contains('^') {
        return unevaluated("a boost (^)");
    }
    if value.contains('~') {
        return unevaluated("a fuzzy or proximity distance (~)");
    }
    if value.contains('?') {
        return unevaluated("a single-character wildcard (?)");
    }
    // A `*` anywhere but at the end is an interior wildcard; the trailing one is
    // the prefix search this endpoint evaluates.
    if value.trim_end_matches('*').contains('*') {
        return unevaluated("a wildcard inside a term (*)");
    }
    Ok(())
}

/// One token of the structured query syntax.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Open,
    Close,
    /// A quoted value.
    Quoted(String),
    /// A bare word: an operator, an option (`field=title`), or a value.
    Word(String),
}

/// Tokenize a structured query.
fn tokenize_structured(query: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let mut chars = query.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '(' => tokens.push(Token::Open),
            ')' => tokens.push(Token::Close),
            '\'' | '"' => {
                let quote = c;
                let mut value = String::new();
                let mut closed = false;
                while let Some(c) = chars.next() {
                    match c {
                        '\\' => {
                            if let Some(escaped) = chars.next() {
                                value.push(escaped);
                            }
                        }
                        c if c == quote => {
                            closed = true;
                            break;
                        }
                        c => value.push(c),
                    }
                }
                if !closed {
                    return Err("The query has an unterminated quoted value.".to_string());
                }
                tokens.push(Token::Quoted(value));
            }
            c if c.is_whitespace() => {}
            c => {
                let mut word = String::from(c);
                while let Some(&next) = chars.peek() {
                    // A word ends where the syntax's own punctuation starts. BOTH
                    // quote characters end it, because both open a quoted value
                    // above: otherwise `field:"v"` would tokenize as one word
                    // carrying its quotes while `field:'v'` tokenized correctly.
                    if next.is_whitespace()
                        || next == '('
                        || next == ')'
                        || next == '\''
                        || next == '"'
                    {
                        break;
                    }
                    word.push(next);
                    chars.next();
                }
                tokens.push(Token::Word(word));
            }
        }
    }
    Ok(tokens)
}

/// Parse the `fq` (`filterQuery`) parameter, which "supports the full structured
/// query syntax".
///
/// Supported: `matchall`, the `and`/`or`/`not` compound operators, the
/// `term`/`prefix`/`phrase` operators with their `field=` option, and the
/// `field:value` shorthand. An operator this endpoint does not evaluate, or a
/// malformed expression, is refused rather than answered with a wrong result set.
/// A query longer than [`MAX_QUERY_BYTES`] or nesting deeper than
/// [`MAX_QUERY_DEPTH`] is refused before it is parsed that far.
pub fn parse_filter_query(query: &str) -> Result<Matcher, String> {
    guard_query_size("fq", query)?;
    let matcher = parse_structured_expression(query)?;
    guard_query_complexity("fq", &matcher)?;
    Ok(matcher)
}

/// Parse the structured syntax of an already size-checked query string.
fn parse_structured_expression(query: &str) -> Result<Matcher, String> {
    let tokens = tokenize_structured(query)?;
    if tokens.is_empty() {
        return Err("The structured query is empty.".to_string());
    }
    let mut position = 0;
    let matcher = parse_structured_expr(&tokens, &mut position, 1)?;
    if position != tokens.len() {
        return Err("The structured query has trailing input after the expression.".to_string());
    }
    Ok(matcher)
}

/// Parse one structured expression, advancing `position` past it.
///
/// `depth` is how many enclosing sub-expressions this one sits inside, counting
/// itself; past [`MAX_QUERY_DEPTH`] the query is refused instead of recursing
/// deeper.
fn parse_structured_expr(
    tokens: &[Token],
    position: &mut usize,
    depth: usize,
) -> Result<Matcher, String> {
    if depth > MAX_QUERY_DEPTH {
        return Err(format!(
            "The structured query nests sub-expressions deeper than the limit of \
             {MAX_QUERY_DEPTH}."
        ));
    }
    let token = tokens
        .get(*position)
        .ok_or_else(|| "The structured query ends with an incomplete expression.".to_string())?;
    *position += 1;
    match token {
        Token::Close => Err("The structured query has an unbalanced ')'.".to_string()),
        Token::Quoted(value) => Ok(leaf(Target::All, value)),
        Token::Word(word) => {
            // `field:'value'` tokenizes as the word `field:` followed by the
            // quoted value, so a trailing colon takes the next token as its value.
            if let Some(field) = word.strip_suffix(':') {
                if !field.is_empty() {
                    let value = match tokens.get(*position) {
                        Some(Token::Quoted(value) | Token::Word(value)) => value.clone(),
                        _ => {
                            return Err(format!(
                                "The expression '{word}' names the field '{field}' with \
                                 no value to search for."
                            ))
                        }
                    };
                    *position += 1;
                    reject_range_syntax(&value)?;
                    return Ok(leaf(Target::field(field), &value));
                }
            }
            parse_structured_word(word)
        }
        Token::Open => {
            let operator = match tokens.get(*position) {
                Some(Token::Word(word)) => word.to_ascii_lowercase(),
                _ => {
                    return Err(
                        "A '(' in the structured query must be followed by an operator."
                            .to_string(),
                    )
                }
            };
            *position += 1;
            let mut field: Option<String> = None;
            let mut children: Vec<Matcher> = Vec::new();
            let mut values: Vec<String> = Vec::new();
            loop {
                match tokens.get(*position) {
                    None => return Err("The structured query has an unbalanced '('.".to_string()),
                    Some(Token::Close) => {
                        *position += 1;
                        break;
                    }
                    Some(Token::Word(word)) if word.contains('=') => {
                        let (name, value) = word.split_once('=').unwrap_or((word.as_str(), ""));
                        let value = value.trim_matches(|c| c == '\'' || c == '"');
                        if name.eq_ignore_ascii_case("field") {
                            field = Some(value.to_string());
                        }
                        // Every other option (boost, among others) tunes ranking,
                        // which does not change which documents match.
                        *position += 1;
                    }
                    Some(Token::Quoted(value)) => {
                        values.push(value.clone());
                        *position += 1;
                    }
                    Some(_) => {
                        let child = parse_structured_expr(tokens, position, depth + 1)?;
                        children.push(child);
                    }
                }
            }
            build_structured_operator(&operator, field, children, values)
        }
    }
}

/// Build the matcher for one structured compound or leaf operator.
fn build_structured_operator(
    operator: &str,
    field: Option<String>,
    mut children: Vec<Matcher>,
    values: Vec<String>,
) -> Result<Matcher, String> {
    let target = match field {
        Some(field) => Target::field(&field),
        None => Target::All,
    };
    match operator {
        "and" | "or" => {
            children.extend(values.iter().map(|value| leaf(target.clone(), value)));
            if children.is_empty() {
                return Err(format!(
                    "The '{operator}' operator requires at least one sub-expression."
                ));
            }
            Ok(if operator == "and" {
                Matcher::And(children)
            } else {
                Matcher::Or(children)
            })
        }
        "not" => {
            children.extend(values.iter().map(|value| leaf(target.clone(), value)));
            if children.len() != 1 {
                return Err(
                    "The 'not' operator requires exactly one sub-expression.".to_string()
                );
            }
            Ok(Matcher::Not(Box::new(children.remove(0))))
        }
        "term" | "prefix" | "phrase" => {
            if !children.is_empty() {
                return Err(format!(
                    "The '{operator}' operator takes a value, not a sub-expression."
                ));
            }
            let value = values.first().ok_or_else(|| {
                format!("The '{operator}' operator requires a value to search for.")
            })?;
            reject_range_syntax(value)?;
            Ok(match operator {
                "prefix" => leaf(target, &format!("{value}*")),
                "phrase" => Matcher::Phrase {
                    target,
                    tokens: value_tokens(value),
                },
                _ => leaf(target, value),
            })
        }
        "matchall" => Ok(Matcher::MatchAll),
        other => Err(format!(
            "The '{other}' operator is not supported by this endpoint."
        )),
    }
}

/// Parse a bare word of a structured query: `matchall`, `field:value`, or a term.
fn parse_structured_word(word: &str) -> Result<Matcher, String> {
    if word.eq_ignore_ascii_case("matchall") {
        return Ok(Matcher::MatchAll);
    }
    match word.split_once(':') {
        Some((field, value)) if !field.is_empty() && !value.is_empty() => {
            reject_range_syntax(value)?;
            Ok(leaf(Target::field(field), value))
        }
        Some((field, _)) if !field.is_empty() => Err(format!(
            "The expression '{word}' names the field '{field}' with no value to search for."
        )),
        _ => {
            reject_range_syntax(word)?;
            Ok(leaf(Target::All, word))
        }
    }
}

/// One `sort` key: a field (or `_score`/`_id`) and its direction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SortKey {
    /// The field to order by.
    pub field: String,
    /// Whether to order it descending (`desc`).
    pub descending: bool,
}

/// The order CloudSearch applies when a request specifies no `sort`: "results are
/// sorted by their default relevance scores in descending order: _score desc",
/// with the document id as the tie-break so one page of a result set is stable.
pub fn default_sort() -> Vec<SortKey> {
    vec![
        SortKey {
            field: SCORE_FIELD.to_string(),
            descending: true,
        },
        SortKey {
            field: ID_FIELD.to_string(),
            descending: false,
        },
    ]
}

/// Parse the `sort` parameter: a comma-separated list of `<field> <asc|desc>`.
///
/// The direction is required of every key: "You must specify the sort direction
/// (asc or desc) for each field".
pub fn parse_sort(spec: &str) -> Result<Vec<SortKey>, String> {
    guard_query_size("sort", spec)?;
    let mut keys = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let mut words = part.split_whitespace();
        let field = words.next().unwrap_or_default().to_string();
        let direction = words.next().unwrap_or_default();
        if words.next().is_some() {
            return Err(format!("Invalid sort expression '{part}'."));
        }
        let descending = match direction.to_ascii_lowercase().as_str() {
            "asc" => false,
            "desc" => true,
            "" => {
                return Err(format!(
                    "The sort expression '{part}' does not specify a sort direction; \
                     each field must be sorted 'asc' or 'desc'."
                ))
            }
            other => {
                return Err(format!(
                    "Invalid sort direction '{other}' for field '{field}'; the sort \
                     direction must be 'asc' or 'desc'."
                ))
            }
        };
        keys.push(SortKey { field, descending });
    }
    if keys.is_empty() {
        return Err("The sort parameter names no field.".to_string());
    }
    guard_field_count("sort", keys.len())?;
    if !keys.iter().any(|key| key.field == ID_FIELD) {
        // The document id breaks a tie, so one page of a result set is stable.
        keys.push(SortKey {
            field: ID_FIELD.to_string(),
            descending: false,
        });
    }
    Ok(keys)
}

/// A matching document and its relevance score.
pub type ScoredHit = (Document, f64);

/// The order one request sorts its hits in: its sort keys, and how each key's
/// field compares.
#[derive(Debug, Clone)]
pub struct SortOrder {
    /// The keys, in precedence order.
    keys: Vec<SortKey>,
    /// The fields of `keys` whose values compare as numbers.
    numeric: BTreeSet<String>,
}

impl SortOrder {
    /// Decide how each of `keys`' fields compares, ONCE for the whole result set:
    /// numerically when every value of that field in `hits` parses as a finite
    /// number (CloudSearch's int, double and date fields), and lexicographically
    /// otherwise.
    ///
    /// Choosing per PAIR of documents instead would not be a total order, and a
    /// comparator that is not one misorders a page (and trips the standard
    /// library's own total-order check): with the stored values `10`, `9` and
    /// `1a`, `10 < 9` numerically while `10 < 1a < 9` lexicographically, which is
    /// a cycle. A non-finite parse (`NaN`, `inf`) is not a number here either,
    /// for the same reason: `NaN` compares equal to everything.
    ///
    /// The classification reads every key's value of every hit, and the ordering
    /// that follows compares them, so both are charged to `budget` — the key count
    /// is the caller's and the hit count is the index's, and their product is what
    /// the budget exists to bound.
    pub fn new(
        keys: Vec<SortKey>,
        hits: &[ScoredHit],
        budget: &mut Budget,
    ) -> Result<Self, String> {
        // The comparisons `sort_by` will make: about n log n per key.
        let comparisons = keys
            .len()
            .saturating_mul(hits.len())
            .saturating_mul(hits.len().max(2).ilog2() as usize);
        budget.spend(comparisons)?;
        let mut numeric = BTreeSet::new();
        for key in &keys {
            let mut any = false;
            let mut all_numeric = true;
            for (document, _) in hits {
                let values = document.values(&key.field);
                budget.charge(values, 1)?;
                for value in values {
                    any = true;
                    if !value.parse::<f64>().is_ok_and(f64::is_finite) {
                        all_numeric = false;
                    }
                }
            }
            if any && all_numeric {
                numeric.insert(key.field.clone());
            }
        }
        Ok(Self { keys, numeric })
    }

    /// Order two hits by this request's keys.
    pub fn compare(&self, left: &ScoredHit, right: &ScoredHit) -> Ordering {
        for key in &self.keys {
            let ordering = match key.field.as_str() {
                // Scores are occurrence counts, so they are always finite.
                SCORE_FIELD => left.1.total_cmp(&right.1),
                ID_FIELD => left.0.id.cmp(&right.0.id),
                field => compare_field(
                    &left.0,
                    &right.0,
                    field,
                    self.numeric.contains(field),
                ),
            };
            let ordering = if key.descending {
                ordering.reverse()
            } else {
                ordering
            };
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        Ordering::Equal
    }
}

/// Order two documents by one stored field, comparing as numbers when `numeric`
/// (decided once per result set by [`SortOrder::new`]) and lexicographically
/// otherwise. A document with no value for the field sorts after one that has a
/// value.
fn compare_field(left: &Document, right: &Document, field: &str, numeric: bool) -> Ordering {
    match (left.values(field).first(), right.values(field).first()) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(l), Some(r)) if numeric => {
            // `numeric` means every value of this field parsed as a finite
            // number, so neither of these parses can fail or be NaN.
            let (l, r) = (
                l.parse::<f64>().unwrap_or_default(),
                r.parse::<f64>().unwrap_or_default(),
            );
            l.total_cmp(&r)
        }
        (Some(l), Some(r)) => l.cmp(r),
    }
}

/// What a request's `return` parameter asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReturnSpec {
    /// Every stored field (`_all_fields`, the default).
    AllFields,
    /// Document ids only (`_no_fields`).
    NoFields,
    /// Only these fields, in the order given; `_score` names the relevance score.
    Named(Vec<String>),
}

/// Parse the `return` parameter, under the same bounds as every other
/// caller-supplied field list: each named field is projected out of every hit of
/// the page, so the list's length is bounded before it is used.
pub fn parse_return(spec: Option<&str>) -> Result<ReturnSpec, String> {
    let Some(spec) = spec.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(ReturnSpec::AllFields);
    };
    guard_query_size("return", spec)?;
    let names: Vec<String> = spec
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect();
    guard_field_count("return", names.len())?;
    if names.iter().any(|name| name == NO_FIELDS) {
        return Ok(ReturnSpec::NoFields);
    }
    if names.is_empty() || names.iter().any(|name| name == ALL_FIELDS) {
        return Ok(ReturnSpec::AllFields);
    }
    Ok(ReturnSpec::Named(names))
}

/// How a facet's buckets are ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FacetSort {
    /// Highest count first ("count").
    Count,
    /// Ordered by value ("bucket").
    Bucket,
}

/// One field's entry in the `facet` parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacetRequest {
    /// The field to count values of.
    pub field: String,
    /// The bucket order.
    pub sort: FacetSort,
    /// How many buckets to return.
    pub size: usize,
    /// The explicit buckets to count, when the request listed them.
    pub buckets: Option<Vec<String>>,
}

/// The most buckets one facet may answer with, and the most buckets a facet
/// request may name.
///
/// The response's size is bounded by the request's, rather than by how many
/// distinct values the index happens to hold: CloudSearch documents the facet
/// `size` option as "the maximum number of facets to include in the results… The
/// default is 10" and bounds a response's facets, so a larger `size` earns the
/// modelled `SearchException`. The value is the index-field limit reused as a
/// bucket limit, in the absence of a documented bucket maximum.
pub const MAX_FACET_SIZE: usize = MAX_QUERY_FIELDS;

/// Parse the `facet` parameter: `{"FIELD":{"OPTION":VALUE,…},…}`.
pub fn parse_facets(spec: &str) -> Result<Vec<FacetRequest>, String> {
    guard_query_size("facet", spec)?;
    let value: serde_json::Value = serde_json::from_str(spec)
        .map_err(|e| format!("The facet parameter is not valid JSON: {e}."))?;
    let object = value
        .as_object()
        .ok_or_else(|| "The facet parameter must be a JSON object.".to_string())?;
    guard_field_count("facet", object.len())?;
    let mut requests = Vec::with_capacity(object.len());
    for (field, options) in object {
        let options = options.as_object().ok_or_else(|| {
            format!("The facet options for field '{field}' must be a JSON object.")
        })?;
        let mut request = FacetRequest {
            field: field.clone(),
            sort: FacetSort::Count,
            size: DEFAULT_FACET_SIZE,
            buckets: None,
        };
        for (option, value) in options {
            match option.as_str() {
                "sort" => {
                    request.sort = match value.as_str() {
                        Some("count") => FacetSort::Count,
                        Some("bucket") => FacetSort::Bucket,
                        _ => {
                            return Err(format!(
                                "Invalid facet sort for field '{field}'; the supported \
                                 values are 'bucket' and 'count'."
                            ))
                        }
                    };
                }
                "size" => {
                    let size = value.as_u64().ok_or_else(|| {
                        format!("The facet size for field '{field}' must be a number.")
                    })?;
                    let size = usize::try_from(size).unwrap_or(usize::MAX);
                    if size > MAX_FACET_SIZE {
                        return Err(format!(
                            "The facet size {size} for field '{field}' is larger than \
                             the maximum of {MAX_FACET_SIZE} buckets."
                        ));
                    }
                    request.size = size;
                }
                "buckets" => {
                    let buckets = value.as_array().ok_or_else(|| {
                        format!("The facet buckets for field '{field}' must be an array.")
                    })?;
                    if buckets.len() > MAX_FACET_SIZE {
                        return Err(format!(
                            "The facet for field '{field}' names {} buckets; a facet \
                             can name at most {MAX_FACET_SIZE}.",
                            buckets.len()
                        ));
                    }
                    let mut names = Vec::with_capacity(buckets.len());
                    for bucket in buckets {
                        let name = bucket.as_str().ok_or_else(|| {
                            format!(
                                "Every bucket of the facet for field '{field}' must be \
                                 a string."
                            )
                        })?;
                        // A bucket may also be written as a RANGE (`[1970,1979]`,
                        // `{1980,1989}`), which counts the values falling inside
                        // it. This endpoint counts a bucket by the value it
                        // names, so a range would be matched literally and every
                        // such bucket would answer 0: it is refused instead,
                        // exactly as the range syntax is on the query side.
                        reject_range_syntax(name)?;
                        names.push(name.to_string());
                    }
                    request.buckets = Some(names);
                }
                other => {
                    return Err(format!(
                        "Invalid facet option '{other}' for field '{field}'; the \
                         supported options are 'buckets', 'size' and 'sort'."
                    ))
                }
            }
        }
        // "Buckets are returned in the order they are specified in the request."
        if request.buckets.is_some() {
            request.sort = FacetSort::Bucket;
        }
        requests.push(request);
    }
    Ok(requests)
}

/// Count `request`'s buckets over the matching documents, charging `budget` for
/// the values it reads.
///
/// A document counts once per distinct value it holds in the field, so a document
/// listing the same value twice does not count twice.
pub fn compute_facet(
    request: &FacetRequest,
    hits: &[ScoredHit],
    budget: &mut Budget,
) -> Result<Vec<(String, i64)>, String> {
    let mut counts: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
    for (document, _) in hits {
        let field_values = document.values(&request.field);
        budget.charge(field_values, 1)?;
        let values: BTreeSet<&String> = field_values.iter().collect();
        for value in values {
            *counts.entry(value.clone()).or_insert(0) += 1;
        }
    }
    Ok(match &request.buckets {
        // A request that NAMES its buckets gets every one of them, counted:
        // `size` is "the maximum number of facets to include in the results"
        // for the buckets the service chooses, and dropping a bucket the caller
        // asked for would answer a short list with a success status. The named
        // list is already bounded by `MAX_FACET_SIZE` in `parse_facets`.
        Some(buckets) => buckets
            .iter()
            .map(|bucket| {
                (
                    bucket.clone(),
                    counts.get(bucket).copied().unwrap_or_default(),
                )
            })
            .collect(),
        None => {
            let mut buckets: Vec<(String, i64)> = counts.into_iter().collect();
            if matches!(request.sort, FacetSort::Count) {
                // Highest count first, ties broken by value so the page is stable.
                buckets.sort_by(|left, right| {
                    right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0))
                });
            }
            buckets.truncate(request.size);
            buckets
        }
    })
}

/// Parse the `stats` parameter: `{"FIELD-A":{},"FIELD-B":{}}`. "There are
/// currently no options supported for statistics."
pub fn parse_stats(spec: &str) -> Result<Vec<String>, String> {
    guard_query_size("stats", spec)?;
    let value: serde_json::Value = serde_json::from_str(spec)
        .map_err(|e| format!("The stats parameter is not valid JSON: {e}."))?;
    let object = value
        .as_object()
        .ok_or_else(|| "The stats parameter must be a JSON object.".to_string())?;
    guard_field_count("stats", object.len())?;
    let mut fields = Vec::with_capacity(object.len());
    for (field, options) in object {
        let options = options.as_object().ok_or_else(|| {
            format!("The stats options for field '{field}' must be a JSON object.")
        })?;
        if let Some(option) = options.keys().next() {
            return Err(format!(
                "Invalid stats option '{option}' for field '{field}'; there are no \
                 options supported for statistics."
            ));
        }
        fields.push(field.clone());
    }
    Ok(fields)
}

/// One field's statistics over a result set.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FieldStatistics {
    /// The smallest value, as a string.
    pub min: Option<String>,
    /// The largest value, as a string.
    pub max: Option<String>,
    /// How many documents of the result set have a value in the field.
    pub count: i64,
    /// How many have none.
    pub missing: i64,
    /// The sum of the numeric values.
    pub sum: f64,
    /// The sum of the squares of the numeric values.
    pub sum_of_squares: f64,
    /// The mean of the numeric values, as a string.
    pub mean: Option<String>,
    /// The population standard deviation of the numeric values.
    pub stddev: f64,
}

/// Compute `field`'s statistics over the matching documents.
///
/// `min`, `max` and `mean` are strings in the model: for a numeric field they are
/// the string form of a double, for any other field `min`/`max` are the first and
/// last value in order and there is no mean.
pub fn compute_stats(
    field: &str,
    hits: &[ScoredHit],
    budget: &mut Budget,
) -> Result<FieldStatistics, String> {
    let mut stats = FieldStatistics::default();
    let mut numbers: Vec<f64> = Vec::new();
    let mut strings: Vec<&String> = Vec::new();
    let mut numeric = true;
    for (document, _) in hits {
        let values = document.values(field);
        budget.charge(values, 1)?;
        if values.is_empty() {
            stats.missing += 1;
            continue;
        }
        stats.count += 1;
        for value in values {
            match value.parse::<f64>() {
                Ok(number) if number.is_finite() => numbers.push(number),
                _ => numeric = false,
            }
            strings.push(value);
        }
    }
    if strings.is_empty() {
        return Ok(stats);
    }
    if numeric && !numbers.is_empty() {
        let count = numbers.len();
        let sum: f64 = numbers.iter().sum();
        let sum_of_squares: f64 = numbers.iter().map(|n| n * n).sum();
        #[allow(clippy::cast_precision_loss)]
        let mean = sum / count as f64;
        #[allow(clippy::cast_precision_loss)]
        let variance = (sum_of_squares / count as f64) - (mean * mean);
        stats.min = numbers
            .iter()
            .copied()
            .reduce(f64::min)
            .map(format_number);
        stats.max = numbers
            .iter()
            .copied()
            .reduce(f64::max)
            .map(format_number);
        stats.sum = sum;
        stats.sum_of_squares = sum_of_squares;
        stats.mean = Some(format_number(mean));
        stats.stddev = variance.max(0.0).sqrt();
    } else {
        strings.sort();
        stats.min = strings.first().map(|value| (*value).clone());
        stats.max = strings.last().map(|value| (*value).clone());
    }
    Ok(stats)
}

/// Render a double the way CloudSearch's string-valued statistics do: an integral
/// value without a fractional part, anything else with one.
fn format_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{value:.0}")
    } else {
        format!("{value}")
    }
}

/// The most occurrences one `highlight` request may mark in a field.
///
/// Every marked occurrence emits both of the request's tags, for every hit of the
/// page, so the count is bounded like every other request-chosen factor. The
/// value is the index-field limit reused, in the absence of a documented
/// `max_phrases` maximum; a larger one earns the modelled `SearchException`.
pub const MAX_HIGHLIGHT_PHRASES: usize = MAX_QUERY_FIELDS;

/// The longest `pre_tag`/`post_tag` a `highlight` request may ask for.
///
/// The tags are the one part of a response whose bytes the CALLER writes: each
/// highlighted occurrence emits both of them, up to `max_phrases` occurrences per
/// field, for every hit of the page — so their length multiplies the response
/// rather than the work of reading the index. CloudSearch publishes no tag-length
/// limit, so this is the emulator's own ceiling, large enough for any markup the
/// option is for (`<em class="hit">` and the like) and small enough that the
/// product of tag length, occurrences and page size cannot amplify one request
/// into megabytes of generated markup. A longer tag earns the modelled
/// `SearchException`.
pub const MAX_HIGHLIGHT_TAG_BYTES: usize = 64;

/// The deepest hit `start`+`size` paging reaches, from the `start` documentation:
/// "you can use the `start` and `size` parameters to page through the first
/// 10,000 hits" — past that CloudSearch requires a `cursor` (Paginating Results,
/// Amazon CloudSearch Developer Guide). The page a request asks for is therefore
/// bounded by the same number, so one call cannot ask for an unbounded page of
/// hits; a deeper or wider one earns the modelled `SearchException`.
pub const MAX_PAGE_DEPTH: usize = 10_000;

/// One field's entry in the `highlight` parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightRequest {
    /// The field to highlight matches in.
    pub field: String,
    /// What to put before a match.
    pub pre_tag: String,
    /// What to put after a match.
    pub post_tag: String,
    /// How many occurrences to highlight ("By default, the first occurrence is
    /// highlighted").
    pub max_phrases: usize,
}

/// Parse the `highlight` parameter: `{"FIELD":{"OPTION":VALUE,…},…}`.
pub fn parse_highlights(spec: &str) -> Result<Vec<HighlightRequest>, String> {
    guard_query_size("highlight", spec)?;
    let value: serde_json::Value = serde_json::from_str(spec)
        .map_err(|e| format!("The highlight parameter is not valid JSON: {e}."))?;
    let object = value
        .as_object()
        .ok_or_else(|| "The highlight parameter must be a JSON object.".to_string())?;
    guard_field_count("highlight", object.len())?;
    let mut requests = Vec::with_capacity(object.len());
    for (field, options) in object {
        let options = options.as_object().ok_or_else(|| {
            format!("The highlight options for field '{field}' must be a JSON object.")
        })?;
        // "format: specifies the format of the data in the text field: text or
        // html. … The default is html." The text format marks a match with `*`.
        let mut html = true;
        if let Some(format) = options.get("format") {
            html = match format.as_str() {
                Some("html") => true,
                Some("text") => false,
                _ => {
                    return Err(format!(
                        "Invalid highlight format for field '{field}'; the supported \
                         values are 'html' and 'text'."
                    ))
                }
            };
        }
        let mut request = HighlightRequest {
            field: field.clone(),
            pre_tag: if html { "<em>".to_string() } else { "*".to_string() },
            post_tag: if html { "</em>".to_string() } else { "*".to_string() },
            max_phrases: 1,
        };
        for (option, value) in options {
            match option.as_str() {
                "format" => {}
                "pre_tag" => {
                    request.pre_tag = highlight_tag(field, "pre_tag", value)?;
                }
                "post_tag" => {
                    request.post_tag = highlight_tag(field, "post_tag", value)?;
                }
                "max_phrases" => {
                    let max = value.as_u64().ok_or_else(|| {
                        format!("The highlight max_phrases for field '{field}' must be a number.")
                    })?;
                    let max = usize::try_from(max).unwrap_or(usize::MAX);
                    // The option says how many occurrences to mark, and the
                    // documented default is the first one, so zero asks for a
                    // highlight that highlights nothing: refused, rather than
                    // answered with the field present and unmarked.
                    if max == 0 {
                        return Err(format!(
                            "The highlight max_phrases for field '{field}' must be at \
                             least 1."
                        ));
                    }
                    if max > MAX_HIGHLIGHT_PHRASES {
                        return Err(format!(
                            "The highlight max_phrases {max} for field '{field}' is \
                             larger than the maximum of {MAX_HIGHLIGHT_PHRASES}."
                        ));
                    }
                    request.max_phrases = max;
                }
                other => {
                    return Err(format!(
                        "Invalid highlight option '{other}' for field '{field}'; the \
                         supported options are 'format', 'max_phrases', 'post_tag' and \
                         'pre_tag'."
                    ))
                }
            }
        }
        requests.push(request);
    }
    Ok(requests)
}

/// The `pre_tag`/`post_tag` value of one highlight option, refused when it is not
/// a string or is longer than [`MAX_HIGHLIGHT_TAG_BYTES`].
fn highlight_tag(field: &str, option: &str, value: &serde_json::Value) -> Result<String, String> {
    let tag = value
        .as_str()
        .ok_or_else(|| format!("The highlight {option} for field '{field}' must be a string."))?;
    if tag.len() > MAX_HIGHLIGHT_TAG_BYTES {
        return Err(format!(
            "The highlight {option} for field '{field}' is {} bytes; a tag can be at \
             most {MAX_HIGHLIGHT_TAG_BYTES} bytes long.",
            tag.len()
        ));
    }
    Ok(tag.to_string())
}

/// Highlight `terms` in `document`'s value of the requested field, charging
/// `budget` for the text it scans, for the indexes and candidate spans it
/// allocates, AND for the bytes it emits.
///
/// `Ok(None)` when the document has no value there or no term occurs in it.
///
/// LIMITATION: an array-typed field is highlighted on its FIRST value, because
/// the modelled `highlights` member holds one string per field and there is no
/// place in the response for a second one; a term that occurs only in a later
/// value of the same field therefore yields no highlight.
pub fn compute_highlight(
    request: &HighlightRequest,
    document: &Document,
    terms: &[String],
    budget: &mut Budget,
) -> Result<Option<String>, String> {
    let values = document.values(&request.field);
    // Each term is searched over the value, so the scan costs the value's length
    // once per term.
    budget.charge(&values[..values.len().min(1)], terms.len())?;
    let Some(value) = values.first() else {
        return Ok(None);
    };
    // Match case-insensitively, but mark up the ORIGINAL value: lower-casing is
    // not length-preserving for every character (U+0130 lower-cases to three
    // bytes), so each byte of the lower-cased copy carries the offset of the
    // original character it came from and every span is a valid slice of `value`.
    //
    // The copy and its offset index are memory this call ALLOCATES, sized by the
    // value: one byte per byte for the copy and one `usize` per byte for the
    // index. Charged BEFORE they are built, so the one budget bounds this phase's
    // memory as well as its comparisons.
    budget.spend(value.len().saturating_mul(1 + std::mem::size_of::<usize>()))?;
    let mut lower = String::with_capacity(value.len());
    let mut origin: Vec<usize> = Vec::with_capacity(value.len() + 1);
    for (offset, character) in value.char_indices() {
        for lowered in character.to_lowercase() {
            lower.push(lowered);
        }
        while origin.len() < lower.len() {
            origin.push(offset);
        }
    }
    origin.push(value.len());
    // The occurrences of each term, by byte offset, longest match first so a
    // phrase wins over one of its own words.
    //
    // At most `max_phrases` occurrences are kept PER TERM, and each kept one is
    // charged for the bytes it occupies: the loop below marks at most
    // `max_phrases` occurrences in total, so the candidates kept here cover
    // every occurrence it can reach, and the collection cannot grow with the
    // length of the value. Without the cap a one-character term over a long
    // value — which the query syntax can ask for, and which the text scan is
    // charged for by LENGTH, not by occurrence — would materialise one entry
    // per byte, so a single request could allocate far more than the work it was
    // charged for.
    //
    // The cap is reached only when one term occurs more than `max_phrases` times
    // in one value, and then only an occurrence that every earlier occurrence of
    // the SAME term overlapped could have been marked and now is not; the count
    // of marked occurrences is unchanged.
    let per_term = request.max_phrases.max(1);
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for term in terms {
        let needle = term.to_lowercase();
        if needle.is_empty() {
            continue;
        }
        let mut from = 0;
        let mut kept = 0;
        while kept < per_term {
            let Some(found) = lower[from..].find(&needle) else {
                break;
            };
            let start = from + found;
            let end = start + needle.len();
            // Back onto the original value's offsets. A match that begins or ends
            // inside one character's lower-cased expansion takes that whole
            // character, which is the span a reader expects to see marked.
            match (origin.get(start), origin.get(end)) {
                (Some(&start), Some(&end)) if start < end => {
                    budget.spend(std::mem::size_of::<(usize, usize)>())?;
                    spans.push((start, end));
                    kept += 1;
                }
                _ => {}
            }
            from = end;
        }
    }
    if spans.is_empty() {
        return Ok(None);
    }
    spans.sort_by(|left, right| left.0.cmp(&right.0).then(right.1.cmp(&left.1)));
    // The marked-up copy of the value is output this call GENERATES, so it is
    // charged to the same budget as the text it read: the value is copied once,
    // and each occurrence the loop below marks adds both tags (charged inside the
    // loop). Without this the product of tag length, `max_phrases` and the page
    // size would be the one phase of a request whose growth nothing bounds.
    budget.spend(value.len())?;
    let mut highlighted = String::with_capacity(value.len());
    let mut cursor = 0;
    let mut used = 0;
    for (start, end) in spans {
        // The offsets are character boundaries of `value` by construction; the
        // guard keeps a later edit from slicing mid-character.
        if used >= request.max_phrases
            || start < cursor
            || !value.is_char_boundary(start)
            || !value.is_char_boundary(end)
        {
            continue;
        }
        budget.spend(request.pre_tag.len() + request.post_tag.len())?;
        highlighted.push_str(&value[cursor..start]);
        highlighted.push_str(&request.pre_tag);
        highlighted.push_str(&value[start..end]);
        highlighted.push_str(&request.post_tag);
        cursor = end;
        used += 1;
    }
    highlighted.push_str(&value[cursor..]);
    Ok(Some(highlighted))
}

/// The offset a `cursor` value resumes at.
///
/// `initial` starts the result set; any other value must be a token this endpoint
/// minted with [`encode_cursor`].
pub fn parse_cursor(cursor: &str) -> Result<usize, String> {
    let cursor = cursor.trim();
    if cursor.eq_ignore_ascii_case(INITIAL_CURSOR) {
        return Ok(0);
    }
    cursor
        .strip_prefix(CURSOR_PREFIX)
        .and_then(|offset| usize::from_str_radix(offset, 16).ok())
        .ok_or_else(|| {
            format!(
                "The cursor value '{cursor}' is not valid; to get the first cursor, \
                 set the cursor to '{INITIAL_CURSOR}'."
            )
        })
}

/// The cursor token that resumes a result set at `offset`.
///
/// LIMITATION: the token carries a POSITION in the result set, and the result set
/// is recomputed from the index on every call, so a batch uploaded between two
/// cursor pages shifts the positions: a document inserted before the resume point
/// pushes one hit onto the next page (it is seen twice) and a deleted one pulls a
/// hit back (it is not seen at all). CloudSearch's own cursor is likewise a
/// position in a result set it recomputes, so a concurrent upload changes what a
/// cursor resumes at there too; what this endpoint does not have is a snapshot of
/// the index per cursor, which would make a page sequence stable against writes.
pub fn encode_cursor(offset: usize) -> String {
    format!("{CURSOR_PREFIX}{offset:x}")
}
