// SPDX-License-Identifier: CC0-1.0

//! The custom HTTP headers an Amplify app configures for the responses its hosting edge
//! serves: the document a caller submits, the rules it declares, and the headers those
//! rules add to one response.
//!
//! `App$customHeaders` "describes the custom HTTP headers for the Amplify app"
//! (`smithy_query(query="doc",shape="App",member="customHeaders")`) and is modelled as a
//! string of at most 25000 characters with no format
//! (`smithy_query(query="shape",shape="CustomHeaders")`), because the member carries a
//! DOCUMENT: a list of rules, each a path pattern and the headers to serve for the paths
//! it matches. The document is written in YAML (of which JSON is a subset), either under
//! an outer `customHeaders` key or as the bare list.
//!
//! What this module exists to hold is that the member is a document the service PARSES
//! rather than a string it stores:
//!
//! * a submitted document is read into [`CustomHeaderRule`]s once, at the write that
//!   supplied it, and the app reports those rules re-serialized as JSON
//!   ([`custom_headers_document`]) — so a reader gets the rules the service parsed, in
//!   one notation, whatever notation they were written in;
//! * the parsed rules are what the hosting edge resolves a request path against
//!   ([`matched_headers`]), so a rule the app carries is a rule its branches' responses
//!   carry.
//!
//! The parsing is `yaml-rust2`'s — the workspace's one YAML reader — never a hand-rolled
//! scan, and the pattern matching is the workspace's one wildcard matcher
//! (`service_common::iam_authorization::wildcard_match`), applied segment by segment so a
//! `*` cannot cross a separator and the cost of every comparison is charged before it is
//! made. A recursive backtracking matcher would not do: its cost cannot be charged from
//! outside the walk.
//!
//! Both halves are BOUNDED: the document's own modelled
//! length caps what is parsed, its SIZE, the NESTING its text could make a parser descend
//! into (`service_common::yaml_nesting`, the workspace's one scan for this) and its exact
//! DEPTH are all refused before the loader builds a tree of it,
//! [`MAX_CUSTOM_HEADER_RULES_PER_APP`] caps how many rules one app may carry, and
//! [`MAX_HEADER_MATCH_STEPS_PER_VISIT`] caps the matching work one visit may spend across
//! them — charged inside the walk, so the cap binds the work and not an estimate of it.

/// One rule of an app's custom-headers document: the paths it applies to, and the headers
/// it adds to the response for each of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomHeaderRule {
    /// The rule's `pattern`, exactly as the document wrote it.
    pub pattern: String,
    /// That pattern as the segments it is matched by, from the site root: one entry per
    /// `/`-separated segment, read once at the write rather than per request.
    segments: Vec<String>,
    /// The headers this rule serves, in the order the document declared them, each a
    /// `key` and its `value`.
    pub headers: Vec<(String, String)>,
}

impl CustomHeaderRule {
    /// The rule for `pattern` serving `headers`, with the pattern read into its segments
    /// once.
    fn new(pattern: String, headers: Vec<(String, String)>) -> Self {
        let segments = from_site_root_relative(&pattern)
            .split(SEPARATOR)
            .map(str::to_string)
            .collect();
        Self {
            pattern,
            segments,
            headers,
        }
    }

    /// Whether this rule applies to the site path `path`, charging every comparison it
    /// makes to this visit's matching `budget` BEFORE making it.
    ///
    /// Both sides are compared as paths from the site root with no leading separator, so
    /// a pattern written `/index.html` applies to the same requests as one written
    /// `index.html`. The pattern language is the one a site's header configuration is
    /// written in: a `**` segment matches ZERO OR MORE path segments — which is what makes
    /// `**/*` the whole site — and every other segment matches ONE path segment, with `*`
    /// matching any run of characters inside that segment and `?` exactly one.
    ///
    /// The walk is the classic greedy two-pointer match with ONE backtrack point, for the
    /// last `**` seen: it never recurses, and each segment comparison is the workspace's
    /// one wildcard matcher (`service_common::iam_authorization::wildcard_match`, "the
    /// classic single-pass wildcard match with one backtrack point per `*`, so its cost is
    /// bounded by the product of the two lengths and it never recurses"). That product is
    /// charged to the budget before each comparison, and the walk stops the moment the
    /// budget is spent, so the work one visit does is capped by what the budget buys
    /// rather than by an estimate of it. A path longer than this plane matches, and a rule
    /// reached with the budget spent, apply nothing.
    fn applies_within(&self, path: &str, budget: &mut usize) -> bool {
        // Measured through a bounded probe, so the measuring itself cannot be made to
        // walk text of the caller's choosing, and CHARGED before the length is judged, so
        // the rejected case cannot be repeated for free either.
        let path_chars = path.chars().take(MAX_MATCHED_PATH_CHARS + 1).count();
        if !charge(budget, path_chars) {
            return false;
        }
        if path_chars > MAX_MATCHED_PATH_CHARS {
            ::tracing::warn!(
                path_chars,
                "the request path is longer than this plane matches a custom-header rule \
                 against, so the rule applies nothing"
            );
            return false;
        }
        let path = from_site_root_relative(path);
        let path: Vec<&str> = path.split(SEPARATOR).collect();
        segments_apply(&self.segments, &path, budget)
    }
}

/// Whether the pattern segments `pattern` match the path segments `path`, charging each
/// comparison to `budget`.
///
/// The classic greedy two-pointer walk: a `**` segment is remembered as the one place to
/// resume from, and a comparison that fails resumes there with that `**` swallowing one
/// more path segment. Nothing recurses, and a spent budget stops the walk at once — which
/// reports NO match, the safe direction for a configuration that would cost more than one
/// visit may spend.
fn segments_apply(pattern: &[String], path: &[&str], budget: &mut usize) -> bool {
    let mut at_pattern = 0usize;
    let mut at_path = 0usize;
    // Where to resume, and the path segment the last `**` had reached when it was seen.
    let mut resume: Option<usize> = None;
    let mut swallowed = 0usize;
    while at_path < path.len() {
        if *budget == 0 {
            return false;
        }
        match pattern.get(at_pattern) {
            Some(segment) if segment == ANY_SEGMENTS => {
                resume = Some(at_pattern + 1);
                swallowed = at_path;
                at_pattern += 1;
            }
            Some(segment) if segment_applies(segment, path[at_path], budget) => {
                at_pattern += 1;
                at_path += 1;
            }
            // Either the pattern ran out or this segment does not match: the last `**`
            // takes one more path segment, and matching resumes after it.
            _ => match resume {
                Some(after) => {
                    at_pattern = after;
                    swallowed += 1;
                    at_path = swallowed;
                }
                None => return false,
            },
        }
    }
    // A pattern ending in `**` segments has nothing left to match them against, which is
    // what makes `**` the whole site.
    while pattern
        .get(at_pattern)
        .is_some_and(|segment| segment == ANY_SEGMENTS)
    {
        at_pattern += 1;
    }
    at_pattern == pattern.len()
}

/// Whether one pattern segment applies to one path segment, charged to `budget`.
///
/// The comparison is the workspace's one wildcard matcher, which is where `*` and `?` mean
/// what they mean; it is given ONE segment of each side, so neither wildcard can cross a
/// separator. The product of the two lengths — the bound on that matcher's cost — is
/// charged before it runs.
fn segment_applies(pattern: &str, path: &str, budget: &mut usize) -> bool {
    let pattern_chars = pattern.chars().take(MAX_MATCHED_PATH_CHARS + 1).count();
    let path_chars = path.chars().take(MAX_MATCHED_PATH_CHARS + 1).count();
    if !charge(budget, pattern_chars.saturating_mul(path_chars)) {
        return false;
    }
    if pattern_chars > MAX_MATCHED_PATH_CHARS {
        return false;
    }
    ::service_common::iam_authorization::wildcard_match(pattern, path, CASE_SENSITIVE)
}

/// Charge `steps` (at least one) to `budget`, answering whether the work may be done.
///
/// A charge the budget cannot cover is not paid: it spends the budget, which stops the
/// walk rather than letting it run on credit.
fn charge(budget: &mut usize, steps: usize) -> bool {
    let steps = steps.max(1);
    if steps > *budget {
        *budget = 0;
        return false;
    }
    *budget -= steps;
    true
}

/// A site path is case-sensitive: it names a file a deployment published.
const CASE_SENSITIVE: bool = true;

/// The segment separator of a site path and of a pattern.
const SEPARATOR: char = '/';

/// The pattern segment that matches zero or more path segments.
const ANY_SEGMENTS: &str = "**";

/// How many header rules one app may carry at once, past which a writer refuses the
/// document.
///
/// CHOSEN, not a published quota, exactly as [`crate::state::MAX_CUSTOM_RULES_PER_APP`]
/// is: `CustomHeaders` is modelled by its length alone
/// (`smithy_query(query="shape",shape="CustomHeaders")`), and the stored rules are walked
/// for EVERY request a branch's hosting edge serves, so without a bound one app's
/// configuration — and the work one visit does — would grow with nothing to cap it. It is
/// far above the handful of rules a site's header configuration carries (AWS's own
/// examples state a site-wide rule and a few per-directory ones).
pub const MAX_CUSTOM_HEADER_RULES_PER_APP: usize = 100;

/// How much path matching ONE hosting-edge visit may spend, in steps, across every rule
/// the app carries.
///
/// Every input is size-bounded on its own — the document at the member's modelled 25000
/// characters, the rules at [`MAX_CUSTOM_HEADER_RULES_PER_APP`], the matched text at
/// [`MAX_MATCHED_PATH_CHARS`] — but their PRODUCT is not bounded by those sizes alone.
/// This is the ONE explicit cap on the total, spent down INSIDE the walk
/// ([`segments_apply`]) across every rule of a visit, so a single request can never occupy
/// a worker for longer than it buys, whatever shape the stored patterns have. It is the
/// same budget the workspace's other bounded matcher gives one request
/// (`service_common::event_pattern::MAX_MATCH_STEPS_PER_REQUEST`), and orders of magnitude
/// above what a real header configuration costs: a site-wide `**/*` against a 60-character
/// path spends a few hundred steps.
pub const MAX_HEADER_MATCH_STEPS_PER_VISIT: usize = 4_000_000;

/// The longest pattern segment, and the longest request path, this plane matches against.
///
/// A path longer than this names no file any deployment published (a site key is a path
/// inside an archive), and a pattern segment longer than this matches no segment, so
/// neither is matched at all.
pub const MAX_MATCHED_PATH_CHARS: usize = 1024;

/// `value` as a path from the site root WITHOUT a leading separator, which is how a site
/// key is spelled ([`crate::state::hosted_site_key`]) and how a pattern is compared
/// against one.
fn from_site_root_relative(value: &str) -> String {
    value.strip_prefix('/').unwrap_or(value).to_string()
}

/// What the `customHeaders` document a request supplied ingests to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestedCustomHeaders {
    /// The request supplied no document, so the member is left exactly as it was — which
    /// is what makes an update partial ([`crate::state::apply_optional`]).
    Unsupplied,
    /// The document, as the app reports it, and the rules it declares.
    Declared {
        /// What every reader of the member is answered with.
        document: String,
        /// The rules the hosting edge resolves a request against.
        rules: Vec<CustomHeaderRule>,
    },
    /// The document declares more rules than one app may carry
    /// ([`MAX_CUSTOM_HEADER_RULES_PER_APP`]), carrying how many it declared: the writer
    /// refuses it, and nothing is stored.
    TooManyRules(usize),
}

/// Ingest the `customHeaders` document a request supplied: the rules it declares, and the
/// document the app reports for them.
///
/// A document that declares rules is reported as those rules in JSON; the empty document
/// is reported as the empty value and declares nothing. A document this service cannot
/// read as a rule list at all is kept exactly as submitted and declares no rule: no
/// recorded behaviour shows how such a document is refused, so it is neither rewritten nor
/// turned into a refusal this service would have to invent. A document declaring more
/// rules than [`MAX_CUSTOM_HEADER_RULES_PER_APP`] is refused BY THE WRITER, before
/// anything is stored.
pub fn ingest_custom_headers(submitted: Option<String>) -> IngestedCustomHeaders {
    let Some(submitted) = submitted else {
        return IngestedCustomHeaders::Unsupplied;
    };
    if submitted.trim().is_empty() {
        return IngestedCustomHeaders::Declared {
            document: submitted,
            rules: Vec::new(),
        };
    }
    match custom_header_rules(&submitted) {
        Some(rules) if rules.len() > MAX_CUSTOM_HEADER_RULES_PER_APP => {
            IngestedCustomHeaders::TooManyRules(rules.len())
        }
        Some(rules) => IngestedCustomHeaders::Declared {
            document: custom_headers_document(&rules),
            rules,
        },
        None => IngestedCustomHeaders::Declared {
            document: submitted,
            rules: Vec::new(),
        },
    }
}

/// The `message` a writer's refusal carries when a document would leave an app carrying
/// more than [`MAX_CUSTOM_HEADER_RULES_PER_APP`] header rules.
///
/// Built FROM THE REQUEST — the limit, the number of rules the document declares, and the
/// identifier the request names the app by — in the same `Number of … exceeds the limit of
/// …` construction this crate's other quota refusals use
/// ([`crate::state::custom_rules_limit_message`]), because the model spells no message out
/// for a bound it does not state.
pub fn custom_header_rules_limit_message(app: &str, supplied: usize) -> String {
    crate::state::within_error_message_bound(format!(
        "Number of custom header rules ({supplied}) exceeds the limit of \
         {MAX_CUSTOM_HEADER_RULES_PER_APP} for app {app}"
    ))
}

/// The rules a custom-headers document declares, or `None` when the document is not one.
///
/// The document is a mapping carrying a `customHeaders` list — the form the service's own
/// documentation writes — or the bare list, which is the form a reader is answered with,
/// so reading a document back, editing it and sending it again declares the same rules.
/// Each rule carries a `pattern` and, optionally, the `headers` it serves; a document
/// whose shape is anything else declares nothing.
pub fn custom_header_rules(document: &str) -> Option<Vec<CustomHeaderRule>> {
    let root = load_one_document(document)?;
    let declared = match &root {
        ::yaml_rust2::Yaml::Hash(mapping) => {
            mapping_get(mapping, CUSTOM_HEADERS_KEY)?.as_vec()?.clone()
        }
        ::yaml_rust2::Yaml::Array(items) => items.clone(),
        _ => return None,
    };
    let mut rules = Vec::with_capacity(declared.len());
    for item in &declared {
        let mapping = item.as_hash()?;
        let pattern = scalar_string(mapping_get(mapping, PATTERN_KEY)?)?;
        let headers = match mapping_get(mapping, HEADERS_KEY) {
            // A rule that declares no header applies no header; it is still a rule the
            // document stated, so it is read and reported rather than dropped.
            None | Some(::yaml_rust2::Yaml::Null) => Vec::new(),
            Some(declared) => declared_headers(declared)?,
        };
        rules.push(CustomHeaderRule::new(pattern, headers));
    }
    Some(rules)
}

/// The `key`/`value` pairs one rule's `headers` list declares, or `None` when the list is
/// not one.
fn declared_headers(declared: &::yaml_rust2::Yaml) -> Option<Vec<(String, String)>> {
    declared
        .as_vec()?
        .iter()
        .map(|header| {
            let mapping = header.as_hash()?;
            let key = scalar_string(mapping_get(mapping, KEY_KEY)?)?;
            let value = scalar_string(mapping_get(mapping, VALUE_KEY)?)?;
            Some((key, value))
        })
        .collect()
}

/// The document a reader is answered with for `rules`: the rules themselves, as JSON.
///
/// The outer `customHeaders` key the submitted form carries is not part of the rules, so
/// the reported document is the LIST — which is also a document
/// [`custom_header_rules`] reads, so a read-modify-write round trip is stable.
///
/// Written through a serialized STRUCTURE rather than through a JSON map, so each rule's
/// members reach the document in the order the rule states them whatever map type the
/// JSON library is built with.
pub fn custom_headers_document(rules: &[CustomHeaderRule]) -> String {
    let reported: Vec<ReportedRule<'_>> = rules
        .iter()
        .map(|rule| ReportedRule {
            pattern: &rule.pattern,
            headers: rule
                .headers
                .iter()
                .map(|(key, value)| ReportedHeader { key, value })
                .collect(),
        })
        .collect();
    // The types below are strings and lists of strings, which cannot fail to serialize.
    ::serde_json::to_string(&reported).unwrap_or_default()
}

/// One rule as the reported document writes it.
#[derive(Debug, ::serde::Serialize)]
struct ReportedRule<'rule> {
    pattern: &'rule str,
    headers: Vec<ReportedHeader<'rule>>,
}

/// One header of a reported rule.
#[derive(Debug, ::serde::Serialize)]
struct ReportedHeader<'rule> {
    key: &'rule str,
    value: &'rule str,
}

/// The headers `rules` add to the response for the site path `path`, in the order the
/// document declared them.
///
/// EVERY rule whose pattern applies contributes its headers — an app states the headers
/// of the whole site under one pattern and those of a part of it under another, and a
/// response that matches both carries both — and where two of them name the same header,
/// the later rule's value is the one served, because it is the more specific statement
/// the document made last.
///
/// What one visit costs is bounded by WORK and not only by input size: the product of each
/// rule's pattern length and the matched path length is charged to this visit's budget
/// before that rule is matched ([`MAX_HEADER_MATCH_STEPS_PER_VISIT`]), and a rule reached
/// with the budget spent applies nothing — the safe direction, since a configuration that
/// would need millions of steps is not one a site's headers are written as.
pub fn matched_headers(rules: &[CustomHeaderRule], path: &str) -> Vec<(String, String)> {
    let mut budget = MAX_HEADER_MATCH_STEPS_PER_VISIT;
    let mut served: Vec<(String, String)> = Vec::new();
    for rule in rules {
        if !rule.applies_within(path, &mut budget) {
            if budget == 0 {
                ::tracing::warn!(
                    rules = rules.len(),
                    "this visit's custom-header matching budget is spent, so the rules \
                     after it apply nothing"
                );
                break;
            }
            continue;
        }
        for (key, value) in &rule.headers {
            match served
                .iter_mut()
                .find(|(served, _)| served.eq_ignore_ascii_case(key))
            {
                Some(entry) => entry.1 = value.clone(),
                None => served.push((key.clone(), value.clone())),
            }
        }
    }
    served
}

/// One YAML document loaded from `text`, or `None` when it is not exactly one readable
/// document.
///
/// THREE things are refused before the loader builds a tree of the text, in order, because
/// none of them can be stopped once it is running:
///
/// * its SIZE and the NESTING its text could make a parser descend into
///   ([`within_bounds`]) — `yaml-rust2`'s parser is recursive descent, so the depth it
///   reaches is the depth the caller's document has, and exhausting the stack aborts the
///   process instead of unwinding;
/// * its ALIASES and its exact DEPTH ([`parses_within_depth`]), both read off the parser's
///   own event stream, which copies nothing and builds no tree: an alias is what lets a
///   few hundred bytes expand without limit, and the depth is what the loader — and the
///   recursive drop of what it built — would descend into. Nesting and expansion are
///   different quantities, so neither guard stands in for the other.
///
/// A document past any of them declares no rule rather than being loaded.
fn load_one_document(text: &str) -> Option<::yaml_rust2::Yaml> {
    if !within_bounds(text) || !parses_within_depth(text) {
        return None;
    }
    let mut documents = ::yaml_rust2::YamlLoader::load_from_str(text).ok()?;
    (documents.len() == 1).then(|| documents.remove(0))
}

/// Whether the document is small enough and flat enough to hand to a parser at all: the
/// two bounds on work a caller chooses the size of ([`MAX_HEADERS_DOCUMENT_BYTES`],
/// [`MAX_HEADERS_NESTING`]), checked on the bytes before any parser sees them.
///
/// The nesting is bounded by the shared scan every service that reads a caller-supplied
/// YAML document this way uses (`service_common::yaml_nesting`), rather than by a scan of
/// this crate's own.
fn within_bounds(text: &str) -> bool {
    text.len() <= MAX_HEADERS_DOCUMENT_BYTES
        && ::service_common::yaml_nesting::nesting_depth_bound(text, MAX_HEADERS_NESTING)
            <= MAX_HEADERS_NESTING
}

/// Whether the document parses, uses no YAML alias, and nests no deeper than
/// [`MAX_HEADERS_DEPTH`] — all three read off the parser's own event stream, which copies
/// nothing and builds no tree.
///
/// The depth is measured HERE rather than on the loaded document because the loader
/// resolves an alias by COPYING the anchored node and builds (and then recursively drops)
/// the whole tree: by the time there is a tree to measure, the expansion and the recursion
/// have already happened. What keeps THIS pass in range is the text bound above.
fn parses_within_depth(text: &str) -> bool {
    use ::yaml_rust2::parser::Event;

    #[derive(Default)]
    struct Guard {
        /// Whether an alias was reported, whose expansion copies a node.
        saw_alias: bool,
        /// How many collections are open at this event.
        depth: usize,
        /// The most that were open at once.
        deepest: usize,
    }
    impl ::yaml_rust2::parser::EventReceiver for Guard {
        fn on_event(&mut self, event: Event) {
            match event {
                Event::Alias(_) => self.saw_alias = true,
                Event::SequenceStart(..) | Event::MappingStart(..) => {
                    self.depth = self.depth.saturating_add(1);
                    self.deepest = self.deepest.max(self.depth);
                }
                Event::SequenceEnd | Event::MappingEnd => {
                    self.depth = self.depth.saturating_sub(1);
                }
                _ => {}
            }
        }
    }

    let mut guard = Guard::default();
    let parsed = ::yaml_rust2::parser::Parser::new_from_str(text).load(&mut guard, true);
    parsed.is_ok() && !guard.saw_alias && guard.deepest <= MAX_HEADERS_DEPTH
}

/// The most bytes a custom-headers document this module reads may carry.
///
/// `CustomHeaders` is modelled `length 0..25000`
/// (`smithy_query(query="shape",shape="CustomHeaders")`), counted in CHARACTERS; a
/// character is at most four bytes, so this is that same maximum as bytes.
const MAX_HEADERS_DOCUMENT_BYTES: usize = 4 * 25_000;

/// The nesting a custom-headers document's TEXT may make a parser descend into, bounded
/// before it sees it (the shared scan counts the flow collection openers, the block
/// sequence and complex-key indicators standing on their own, and the indentation a block
/// mapping nests by).
///
/// Each level of nesting costs as little as a byte, so the byte bound above does not bound
/// the depth such a document reaches: this does, and it is what keeps the event-stream pass
/// below in range. EMULATOR-CHOSEN, the same figure the workspace's other reader of a
/// caller-supplied YAML document bounds this same parser at (the CodeDeploy AppSpec
/// reader). The scan is an upper bound that ADDS every sequence entry rather than
/// maximising depth, so this is also the ceiling on the rules AND headers one readable
/// document declares — far above the handful a site's header configuration carries, and
/// above a document at [`MAX_CUSTOM_HEADER_RULES_PER_APP`] rules.
const MAX_HEADERS_NESTING: usize = 256;

/// The deepest custom-headers document this plane reads, measured on the parser's own
/// event stream before the loader builds a tree of it.
///
/// EMULATOR-CHOSEN, the same figure the AppSpec reader uses for the same parser: the
/// document's own nesting is the rule list, one rule, its `headers` list and one header —
/// four levels — so this sits well above any document that declares a rule, and keeps the
/// loader (and the recursive drop of what it built) off a depth no header document has.
const MAX_HEADERS_DEPTH: usize = 32;

/// The value `key` maps to in a YAML mapping.
fn mapping_get<'document>(
    mapping: &'document ::yaml_rust2::yaml::Hash,
    key: &str,
) -> Option<&'document ::yaml_rust2::Yaml> {
    mapping.get(&::yaml_rust2::Yaml::String(key.to_string()))
}

/// A YAML scalar as the text it carries — a header's key and value, and a rule's pattern,
/// are text, and a document that wrote one unquoted (`value: 31536000`, `value: true`)
/// still states that text.
fn scalar_string(value: &::yaml_rust2::Yaml) -> Option<String> {
    match value {
        ::yaml_rust2::Yaml::String(text) | ::yaml_rust2::Yaml::Real(text) => Some(text.clone()),
        ::yaml_rust2::Yaml::Integer(number) => Some(number.to_string()),
        ::yaml_rust2::Yaml::Boolean(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// The key the submitted form of the document carries its rule list under.
const CUSTOM_HEADERS_KEY: &str = "customHeaders";

/// The members of one rule and of one header, as the document spells them.
const PATTERN_KEY: &str = "pattern";
const HEADERS_KEY: &str = "headers";
const KEY_KEY: &str = "key";
const VALUE_KEY: &str = "value";

#[cfg(test)]
mod tests {
    use super::*;

    /// The document and the rules `submitted` ingests to, for a case that expects it to
    /// be accepted.
    fn declared(case: &str, submitted: &str) -> (String, Vec<CustomHeaderRule>) {
        match ingest_custom_headers(Some(submitted.to_string())) {
            IngestedCustomHeaders::Declared { document, rules } => (document, rules),
            other => panic!("case {case:?}: expected a declared document, got {other:?}"),
        }
    }

    /// RULE: a submitted document is reported as the JSON of the rules it declares,
    /// whatever notation it was written in — and reporting is stable under a
    /// read-modify-write round trip.
    #[test]
    fn a_document_is_reported_as_the_json_of_the_rules_it_declares() {
        // (case, the submitted document, the document a reader is answered with)
        let rows: [(&str, &str, &str); 6] = [
            (
                "the documented YAML form",
                "customHeaders:\n  - pattern: '**/*'\n    headers:\n      - key: 'X-Probe'\n        value: 'on'\n",
                r#"[{"pattern":"**/*","headers":[{"key":"X-Probe","value":"on"}]}]"#,
            ),
            (
                "two rules, several headers each",
                "customHeaders:\n  - pattern: '**/*'\n    headers:\n      - key: 'Strict-Transport-Security'\n        value: 'max-age=31536000'\n      - key: 'X-Frame-Options'\n        value: 'DENY'\n  - pattern: '/*.json'\n    headers:\n      - key: 'Cache-Control'\n        value: 'no-store'\n",
                r#"[{"pattern":"**/*","headers":[{"key":"Strict-Transport-Security","value":"max-age=31536000"},{"key":"X-Frame-Options","value":"DENY"}]},{"pattern":"/*.json","headers":[{"key":"Cache-Control","value":"no-store"}]}]"#,
            ),
            (
                "a rule declaring no header",
                "customHeaders:\n  - pattern: '**'\n",
                r#"[{"pattern":"**","headers":[]}]"#,
            ),
            (
                "unquoted scalars",
                "customHeaders:\n  - pattern: /*\n    headers:\n      - key: X-Max-Age\n        value: 31536000\n",
                r#"[{"pattern":"/*","headers":[{"key":"X-Max-Age","value":"31536000"}]}]"#,
            ),
            (
                "the bare list a reader is answered with",
                r#"[{"pattern":"**/*","headers":[{"key":"X-Probe","value":"on"}]}]"#,
                r#"[{"pattern":"**/*","headers":[{"key":"X-Probe","value":"on"}]}]"#,
            ),
            ("the empty document", "", ""),
        ];
        for (case, submitted, expected) in rows {
            let (reported, _) = declared(case, submitted);
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the reported document {expected:?}, got \
                 {reported:?}"
            );
            // Round trip: what a reader is answered with declares the same rules.
            let (again, _) = declared(case, &reported);
            assert_eq!(
                again, reported,
                "case {case:?}: re-submitting the reported document must report it \
                 unchanged, got {again:?}"
            );
        }
    }

    /// RULE: a document this service cannot read as a rule list declares no rule and is
    /// kept as submitted; an alias-bearing document, and one past the size or the nesting
    /// this plane reads, are three of those — none ever reaches the recursive loader.
    #[test]
    fn an_unreadable_document_declares_nothing() {
        // Each form of nesting the shared scan counts, one step past the bound: flow
        // collections, block sequence entries, and the indentation a block mapping nests
        // by — plus a document the scan admits whose true DEPTH the event stream refuses.
        let deeply_nested = format!(
            "customHeaders: {}{}",
            "[".repeat(MAX_HEADERS_NESTING + 1),
            "]".repeat(MAX_HEADERS_NESTING + 1)
        );
        let many_entries = format!(
            "customHeaders:\n{}",
            "  - pattern: '**'\n".repeat(MAX_HEADERS_NESTING + 1)
        );
        let deeply_indented = format!(
            "customHeaders:\n{}- pattern: '**'\n",
            " ".repeat(MAX_HEADERS_NESTING + 1)
        );
        let past_the_depth = format!(
            "customHeaders: {}{}",
            "[".repeat(MAX_HEADERS_DEPTH + 1),
            "]".repeat(MAX_HEADERS_DEPTH + 1)
        );
        let too_long = format!(
            "customHeaders:\n  - pattern: '{}'\n",
            "p".repeat(MAX_HEADERS_DOCUMENT_BYTES)
        );
        for (case, submitted) in [
            ("plain text", "not a header document".to_string()),
            ("a mapping with no rule list", "somethingElse:\n  - a: 1\n".to_string()),
            ("a rule with no pattern", "customHeaders:\n  - headers: []\n".to_string()),
            (
                "a rule whose headers are not a list",
                "customHeaders:\n  - pattern: '**'\n    headers: 'X-A: 1'\n".to_string(),
            ),
            (
                "an alias-bearing document",
                "anchor: &a\n  - pattern: '**'\ncustomHeaders: *a\n".to_string(),
            ),
            ("flow collections past the nesting bound", deeply_nested),
            ("block sequence entries past the nesting bound", many_entries),
            ("indentation past the nesting bound", deeply_indented),
            ("one collection past the document's depth", past_the_depth),
            ("one byte past the document bound", too_long),
        ] {
            let (reported, rules) = declared(case, &submitted);
            assert_eq!(
                (reported.as_str(), rules.is_empty()),
                (submitted.as_str(), true),
                "case {case:?}: an unreadable document is kept as submitted and declares \
                 no rule, got {reported:?} / {rules:?}"
            );
        }

        // Inside the bounds a document still reads — including one at the rule cap, whose
        // every rule and header is an entry the nesting scan counts, so the bound admits
        // the largest configuration a writer accepts.
        let at_the_rule_cap = (0..MAX_CUSTOM_HEADER_RULES_PER_APP).fold(
            "customHeaders:\n".to_string(),
            |mut document, index| {
                document.push_str(&format!(
                    "  - pattern: 'p{index}/*'\n    headers:\n      - key: 'X-R{index}'\n        value: '{index}'\n"
                ));
                document
            },
        );
        let (_, rules) = declared("a document at the rule cap", &at_the_rule_cap);
        assert_eq!(
            rules.len(),
            MAX_CUSTOM_HEADER_RULES_PER_APP,
            "case \"a document at the rule cap\": a document inside every bound still \
             declares its rules, got {rules:?}"
        );
    }

    /// RULE: a rule applies to the paths its glob matches — `*` within one segment, `**`
    /// across them — and every applying rule's headers are served, the last statement of
    /// a header winning.
    #[test]
    fn the_headers_served_are_every_applying_rule_s() {
        let (_, rules) = declared(
            "every applying rule",
            "customHeaders:\n  - pattern: '**/*'\n    headers:\n      - key: 'X-Site'\n        \
             value: 'all'\n      - key: 'X-Scope'\n        value: 'site'\n  - pattern: \
             '/assets/*.css'\n    headers:\n      - key: 'X-Scope'\n        value: 'styles'\n  \
             - pattern: '/one.html'\n    headers:\n      - key: 'X-Exact'\n        value: 'yes'\n",
        );

        // (case, the site path, the headers the response carries)
        type Row<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)]);
        let rows: [Row<'_>; 5] = [
            (
                "a path only the site-wide rule matches",
                "index.html",
                &[("X-Site", "all"), ("X-Scope", "site")],
            ),
            (
                "a nested path the site-wide rule still matches",
                "assets/js/app.js",
                &[("X-Site", "all"), ("X-Scope", "site")],
            ),
            (
                "a path two rules match: the later value wins",
                "assets/main.css",
                &[("X-Site", "all"), ("X-Scope", "styles")],
            ),
            (
                "a segment-bound pattern does not cross a separator",
                "assets/deep/main.css",
                &[("X-Site", "all"), ("X-Scope", "site")],
            ),
            (
                "an exact pattern, written from the site root",
                "one.html",
                &[("X-Site", "all"), ("X-Scope", "site"), ("X-Exact", "yes")],
            ),
        ];
        for (case, path, expected) in rows {
            let served = matched_headers(&rules, path);
            let expected: Vec<(String, String)> = expected
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect();
            assert_eq!(
                served, expected,
                "case {case:?}: expected the headers {expected:?} for {path:?}, got \
                 {served:?}"
            );
        }
    }

    /// RULE: what one app may carry and what one visit may spend matching are both
    /// bounded — a document past the rule limit is refused by the writer, a document at
    /// the limit is accepted, and a path longer than this plane matches applies nothing.
    #[test]
    fn the_rules_and_the_matching_are_bounded() {
        // (case, how many rules the document declares, accepted?)
        for (case, declared_rules, accepted) in [
            ("one rule under the limit", MAX_CUSTOM_HEADER_RULES_PER_APP - 1, true),
            ("the limit itself", MAX_CUSTOM_HEADER_RULES_PER_APP, true),
            ("one rule past the limit", MAX_CUSTOM_HEADER_RULES_PER_APP + 1, false),
        ] {
            let document = (0..declared_rules).fold(
                "customHeaders:\n".to_string(),
                |mut document, index| {
                    document.push_str(&format!(
                        "  - pattern: 'p{index}/*'\n    headers:\n      - key: 'X-R{index}'\n        value: '{index}'\n"
                    ));
                    document
                },
            );
            match ingest_custom_headers(Some(document)) {
                IngestedCustomHeaders::Declared { rules, .. } => {
                    assert!(
                        accepted && rules.len() == declared_rules,
                        "case {case:?}: expected {declared_rules} declared rules to be \
                         accepted {accepted}, got {} rules",
                        rules.len()
                    );
                }
                IngestedCustomHeaders::TooManyRules(supplied) => assert!(
                    !accepted && supplied == declared_rules,
                    "case {case:?}: expected {declared_rules} declared rules to be \
                     accepted {accepted}, got a refusal of {supplied}"
                ),
                other => panic!("case {case:?}: unexpected ingestion {other:?}"),
            }
        }

        // A path longer than this plane matches applies nothing, however many rules are
        // configured.
        let (_, rules) = declared(
            "a site-wide rule",
            "customHeaders:\n  - pattern: '**/*'\n    headers:\n      - key: 'X-Site'\n        \
             value: 'all'\n",
        );
        for (case, path, served) in [
            ("a path at the matched maximum", "p".repeat(MAX_MATCHED_PATH_CHARS), 1),
            (
                "a path one character past it",
                "p".repeat(MAX_MATCHED_PATH_CHARS + 1),
                0,
            ),
        ] {
            let matched = matched_headers(&rules, &path);
            assert_eq!(
                matched.len(),
                served,
                "case {case:?}: expected {served} header(s) for a {}-character path, got \
                 {matched:?}",
                path.len()
            );
        }

        // The budget caps the WORK, charged inside the walk: a visit that runs out stops
        // matching, so the rules after that point serve nothing.
        let rules_within_budget = MAX_HEADER_MATCH_STEPS_PER_VISIT
            / (MAX_MATCHED_PATH_CHARS * MAX_MATCHED_PATH_CHARS);
        assert!(
            rules_within_budget < MAX_CUSTOM_HEADER_RULES_PER_APP,
            "the budget must be the binding bound for the longest patterns and paths, \
             got room for {rules_within_budget} of {MAX_CUSTOM_HEADER_RULES_PER_APP} rules"
        );
        let long_pattern = format!("{}*", "p".repeat(MAX_MATCHED_PATH_CHARS - 1));
        let long_rules: Vec<CustomHeaderRule> = (0..MAX_CUSTOM_HEADER_RULES_PER_APP)
            .map(|index| {
                CustomHeaderRule::new(
                    long_pattern.clone(),
                    vec![(format!("X-R{index}"), index.to_string())],
                )
            })
            .collect();
        let served = matched_headers(&long_rules, &"p".repeat(MAX_MATCHED_PATH_CHARS));
        assert!(
            served.len() <= rules_within_budget + 1,
            "the spent budget must stop the walk, got {} headers for \
             {MAX_CUSTOM_HEADER_RULES_PER_APP} rules",
            served.len()
        );

        // And the bound holds for the pattern shape a BACKTRACKING matcher would explore
        // combinatorially: many wildcards inside one segment, against a long
        // single-segment path that does not match. The walk is charged per comparison and
        // never recurses, so a whole app's worth of such rules answers promptly.
        let many_wildcards = format!("{}z", "*a".repeat(20));
        let adversarial: Vec<CustomHeaderRule> = (0..MAX_CUSTOM_HEADER_RULES_PER_APP)
            .map(|index| {
                CustomHeaderRule::new(
                    many_wildcards.clone(),
                    vec![(format!("X-R{index}"), index.to_string())],
                )
            })
            .collect();
        let started = ::std::time::Instant::now();
        let served = matched_headers(&adversarial, &"a".repeat(MAX_MATCHED_PATH_CHARS));
        let elapsed = started.elapsed();
        assert!(
            served.is_empty() && elapsed < ::std::time::Duration::from_secs(5),
            "case \"a many-wildcard pattern against a long path\": expected no header and \
             a prompt answer, got {} header(s) in {elapsed:?}",
            served.len()
        );
    }
}
