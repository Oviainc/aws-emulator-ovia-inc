// SPDX-License-Identifier: CC0-1.0

//! The ONE free-text search over a stored document's text, for every service whose API
//! answers a caller's QUERY with the documents its own records hold and an excerpt of the
//! text that matched.
//!
//! A manual-search operation has three questions to answer, and none of them is a matter
//! of taste:
//!
//! * what the query NAMES — its terms, so a document does not have to reproduce the
//!   caller's punctuation or letter case to be found;
//! * which of those terms a stored text CARRIES, which is what ranks one document above
//!   another;
//! * which WINDOW of that text to report, and where in that window the matches fall, so a
//!   client can render the hit it was handed.
//!
//! [`match_document`] answers all three for one stored document at once — which of its
//! fields matched, the share of the query it carries, and the window to report it with —
//! because a service that asked them in its own order would rank and excerpt differently
//! from the next one.
//!
//! The offsets are CHARACTER offsets into the text each one is reported with, which is
//! what a renderer of an excerpt needs; a byte offset into UTF-8 would name a position no
//! client could use, which is why the scan is over characters rather than over the bytes a
//! byte-oriented searcher (`str::match_indices`, a pattern-set crate) works in: the one
//! thing those answer — a byte position in a lowercased copy whose length need not match
//! the original's — is not what the wire member carries. The window bounds are the
//! CALLER's — this module imposes none — so a service states the size of its own excerpts
//! beside the member it fills, and the cost of preparing a text is one `char` per character
//! of it, held only while that text is being searched.
//!
//! Nothing here knows a service's shapes: it answers in plain text and offsets, and each
//! service maps those onto the members its own model declares.

/// One matched span of a text, as CHARACTER offsets: `[begin, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MatchSpan {
    /// The offset of the first character of the match.
    pub begin: usize,
    /// The offset one past its last character.
    pub end: usize,
}

/// A window of a text, with the matched spans that fall inside it stated RELATIVE to the
/// window — so the offsets index the text this struct carries, not the document it came
/// from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextWindow {
    /// The window's own text.
    pub text: String,
    /// The matches inside it, in offset order.
    pub spans: Vec<MatchSpan>,
}

/// The distinct terms a query text names: its alphanumeric runs, case-folded, in the order
/// the query states them with repeats dropped.
///
/// A query is matched term by term rather than as one literal, so the punctuation and
/// spacing the caller typed do not have to be reproduced by the document. A query naming no
/// term at all — empty, or punctuation alone — names nothing to match.
///
/// Deduplicated through a SET, so the cost is linear in the terms the query names and not
/// quadratic: the query is caller-supplied, so a service that bounds its length still hands
/// this as many terms as that length allows.
#[must_use]
pub fn query_terms(query_text: &str) -> Vec<String> {
    let mut seen: ::std::collections::HashSet<String> = ::std::collections::HashSet::new();
    let mut terms: Vec<String> = Vec::new();
    for run in query_text.split(|character: char| !character.is_alphanumeric()) {
        if run.is_empty() {
            continue;
        }
        let term: String = run.chars().map(fold).collect();
        if seen.insert(term.clone()) {
            terms.push(term);
        }
    }
    terms
}

/// One character, case-folded for matching.
///
/// The FIRST character of its lowercase mapping, so the folded text keeps one character per
/// character of the original and an offset into either is an offset into the other.
fn fold(character: char) -> char {
    character.to_lowercase().next().unwrap_or(character)
}

/// One stored text, prepared for case-insensitive term matching.
///
/// It holds the text's characters ONCE — the case folding is applied per comparison rather
/// than kept as a second copy — so preparing a document costs one `char` per character of
/// it, and a service searching a large stored asset holds one transient copy of that asset
/// and no more.
#[derive(Debug, Clone)]
pub struct SearchableText {
    /// The characters as the record holds them: what a term is compared against, folded as
    /// it is read, and what a window is cut from, unfolded.
    chars: Vec<char>,
}

impl SearchableText {
    /// Prepare `text` for matching.
    #[must_use]
    pub fn new(text: &str) -> Self {
        Self {
            chars: text.chars().collect(),
        }
    }

    /// Whether this text is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    /// Whether this text carries `term`, which [`query_terms`] has already folded.
    ///
    /// It stops at the FIRST occurrence and allocates nothing: a text is asked this once
    /// per term, and how many times the term occurs is a different question
    /// ([`Self::spans`]).
    #[must_use]
    pub fn carries(&self, term: &str) -> bool {
        let term: Vec<char> = term.chars().collect();
        self.first_occurrence(&term, 0).is_some()
    }

    /// How many of `terms` this text carries.
    #[must_use]
    pub fn carried(&self, terms: &[String]) -> usize {
        terms.iter().filter(|term| self.carries(term)).count()
    }

    /// The first occurrence of `term` at or after `from`, or `None` when there is none.
    fn first_occurrence(&self, term: &[char], from: usize) -> Option<MatchSpan> {
        if term.is_empty() || term.len() > self.chars.len() {
            return None;
        }
        let last = self.chars.len() - term.len();
        (from..=last)
            .find(|at| {
                self.chars[*at..at + term.len()]
                    .iter()
                    .zip(term)
                    .all(|(character, wanted)| fold(*character) == *wanted)
            })
            .map(|at| MatchSpan {
                begin: at,
                end: at + term.len(),
            })
    }

    /// At most `max` non-overlapping occurrences of one term, left to right.
    ///
    /// The cap is applied AS the occurrences are found, so the list this allocates is
    /// bounded by the caller's `max` and not by how often the term happens to occur in a
    /// stored document.
    fn occurrences(&self, term: &str, max: usize) -> Vec<MatchSpan> {
        let term: Vec<char> = term.chars().collect();
        let mut found: Vec<MatchSpan> = Vec::new();
        let mut at = 0usize;
        while found.len() < max {
            let Some(span) = self.first_occurrence(&term, at) else {
                break;
            };
            at = span.end;
            found.push(span);
        }
        found
    }

    /// At most `max` spans of `terms` in this text, in offset order and never overlapping:
    /// of two spans that overlap, the one beginning earlier is kept, and the longer of two
    /// beginning together.
    ///
    /// `max` bounds what is collected per term and what is answered, so the work and the
    /// allocation are bounded before either happens rather than trimmed afterwards.
    #[must_use]
    pub fn spans(&self, terms: &[String], max: usize) -> Vec<MatchSpan> {
        let mut spans: Vec<MatchSpan> = terms
            .iter()
            .flat_map(|term| self.occurrences(term, max))
            .collect();
        spans.sort_by(|left, right| {
            left.begin
                .cmp(&right.begin)
                .then_with(|| right.end.cmp(&left.end))
        });
        let mut kept: Vec<MatchSpan> = Vec::new();
        for span in spans {
            if kept.len() >= max {
                break;
            }
            if kept.last().is_none_or(|previous| span.begin >= previous.end) {
                kept.push(span);
            }
        }
        kept
    }

    /// The window of at most `bounds.max_chars` characters around the FIRST of `spans`,
    /// opening `bounds.lead_chars` characters ahead of it (clamped to the start of the
    /// text), with the spans that fall wholly inside it — at most `bounds.max_spans` of
    /// them — restated relative to the window.
    ///
    /// A text shorter than the window is reported whole, and a `spans` that is empty opens
    /// the window at the start of the text: the beginning of a document is what there is to
    /// show when the match is not in this text.
    #[must_use]
    pub fn window(&self, spans: &[MatchSpan], bounds: WindowBounds) -> TextWindow {
        let first = spans.first().map_or(0, |span| span.begin);
        let begin = first.saturating_sub(bounds.lead_chars);
        let end = begin.saturating_add(bounds.max_chars).min(self.chars.len());
        TextWindow {
            text: self.chars[begin.min(self.chars.len())..end].iter().collect(),
            spans: spans
                .iter()
                .filter(|span| span.begin >= begin && span.end <= end)
                .take(bounds.max_spans)
                .map(|span| MatchSpan {
                    begin: span.begin - begin,
                    end: span.end - begin,
                })
                .collect(),
        }
    }
}

/// The text fields of one stored document a query is matched against: how the document is
/// NAMED, its TITLE, and the BODY of the document itself — the last only when the body is
/// text the service can read, which a binary document's is not.
#[derive(Debug, Clone, Copy)]
pub struct DocumentFields<'a> {
    pub name: &'a str,
    pub title: &'a str,
    pub body: Option<&'a str>,
}

/// How large a window a match is reported with: the window's size in characters, how much
/// of the text it opens ahead of the first match, and the most matches it states.
///
/// The service's own, as the member it fills is: nothing here imposes a size.
#[derive(Debug, Clone, Copy)]
pub struct WindowBounds {
    pub max_chars: usize,
    pub lead_chars: usize,
    pub max_spans: usize,
}

/// What one document answers for a query: how well it matched, and the two windows a
/// result reports it with.
#[derive(Debug, Clone, PartialEq)]
pub struct DocumentMatch {
    /// The share of the query's distinct terms this document carries, in `0..=1`.
    pub score: f64,
    /// The document's title, with the query's terms in it highlighted.
    pub title: TextWindow,
    /// A window of the text the query was found in, with the terms in it highlighted.
    pub excerpt: TextWindow,
}

/// What `fields` answers for a query naming `terms`, or `None` when the document carries
/// none of them.
///
/// The three rules a manual search over stored documents follows, in one place:
///
/// * a document MATCHES when any of its text — its name, its title or its body — carries a
///   term of the query;
/// * it SCORES the share of the query's distinct terms it carries, so a document carrying
///   every term outranks one carrying half of them;
/// * its EXCERPT is of the text the query was found in: the body when the body carries a
///   match, otherwise the title — which is what a match on the document's own naming has to
///   show. A document whose only match is in its name therefore excerpts its title without
///   a highlight, because the matched text is not in the excerpt.
#[must_use]
pub fn match_document(
    fields: DocumentFields<'_>,
    terms: &[String],
    bounds: WindowBounds,
) -> Option<DocumentMatch> {
    if terms.is_empty() {
        return None;
    }
    let title = SearchableText::new(fields.title);
    let name = SearchableText::new(fields.name);
    let body = fields.body.map(SearchableText::new);

    let carried = terms
        .iter()
        .filter(|term| {
            title.carries(term)
                || name.carries(term)
                || body.as_ref().is_some_and(|body| body.carries(term))
        })
        .count();
    if carried == 0 {
        return None;
    }

    let title_spans = title.spans(terms, bounds.max_spans);
    let body_spans = body
        .as_ref()
        .map(|body| body.spans(terms, bounds.max_spans))
        .unwrap_or_default();
    let excerpt = match body.as_ref().filter(|_| !body_spans.is_empty()) {
        Some(body) => body.window(&body_spans, bounds),
        None if !title.is_empty() => title.window(&title_spans, bounds),
        None => body
            .as_ref()
            .map(|body| body.window(&[], bounds))
            .unwrap_or_else(|| TextWindow {
                text: String::new(),
                spans: Vec::new(),
            }),
    };
    Some(DocumentMatch {
        score: carried as f64 / terms.len() as f64,
        title: title.window(&title_spans, bounds),
        excerpt,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A query names its alphanumeric runs, folded and deduplicated, whatever punctuation
    /// and case it was typed with.
    #[test]
    fn a_query_names_its_distinct_folded_terms() {
        // The rows differ along what the rule generalises over: the separator, the letter
        // case, repeats, digits, a non-ASCII letter, and a query naming no term at all.
        let cases: Vec<(&str, Vec<&str>)> = vec![
            ("refund policy", vec!["refund", "policy"]),
            ("Refund  POLICY", vec!["refund", "policy"]),
            ("refund, refund; refund", vec!["refund"]),
            ("escalation-runbook v2.1", vec!["escalation", "runbook", "v2", "1"]),
            ("Ärger", vec!["ärger"]),
            ("", vec![]),
            ("   ,.;  ", vec![]),
        ];
        for (query, expected) in cases {
            let actual = query_terms(query);
            assert_eq!(
                actual, expected,
                "case {query:?}: expected terms {expected:?}, got {actual:?}"
            );
        }
    }

    /// A text carries exactly the terms it holds, case-insensitively, and reports where.
    #[test]
    fn a_text_reports_which_terms_it_carries_and_where() {
        let text = SearchableText::new("Refund policy: refunds are issued within 7 days.");
        let terms = query_terms("REFUND days missing");
        assert_eq!(
            text.carried(&terms),
            2,
            "case \"carried terms\": expected 2 of {terms:?}, got {}",
            text.carried(&terms)
        );
        let spans = text.spans(&terms, 8);
        let found: Vec<(usize, usize)> = spans.iter().map(|span| (span.begin, span.end)).collect();
        assert_eq!(
            found,
            vec![(0, 6), (15, 21), (43, 47)],
            "case \"spans\": expected the three occurrences, got {found:?}"
        );
    }

    /// A window carries the text around the first match with the offsets restated against
    /// itself, and a text that fits inside the window comes back whole.
    #[test]
    fn a_window_carries_the_matched_text_with_relative_offsets() {
        let text = SearchableText::new("0123456789abcdefghij MARKER tail");
        let terms = query_terms("marker");
        let spans = text.spans(&terms, 8);
        let bounds = WindowBounds {
            max_chars: 12,
            lead_chars: 4,
            max_spans: 8,
        };

        // The window opens `lead` characters ahead of the match and runs for at most
        // `max`, so the offsets it reports are its own.
        let clipped = text.window(&spans, bounds);
        assert_eq!(
            clipped.text, "hij MARKER t",
            "case \"clipped window\": expected the 12 characters from offset 17, got {:?}",
            clipped.text
        );
        assert_eq!(
            clipped.spans,
            vec![MatchSpan { begin: 4, end: 10 }],
            "case \"clipped window\": expected the match restated at 4..10, got {:?}",
            clipped.spans
        );

        let short = SearchableText::new("MARKER tail");
        let whole = short.window(
            &short.spans(&terms, 8),
            WindowBounds {
                max_chars: 128,
                ..bounds
            },
        );
        assert_eq!(
            whole.text, "MARKER tail",
            "case \"whole text\": expected the text whole, got {:?}",
            whole.text
        );
        assert_eq!(
            whole.spans,
            vec![MatchSpan { begin: 0, end: 6 }],
            "case \"whole text\": expected the match at 0..6, got {:?}",
            whole.spans
        );

        // No span: the window opens at the start of the text, with no highlight.
        let unmatched = text.window(
            &[],
            WindowBounds {
                max_chars: 10,
                ..bounds
            },
        );
        assert_eq!(
            unmatched.text, "0123456789",
            "case \"no match\": expected the first 10 characters, got {:?}",
            unmatched.text
        );
        assert!(
            unmatched.spans.is_empty(),
            "case \"no match\": expected no highlight, got {:?}",
            unmatched.spans
        );
    }

    /// The cap bounds what a scan collects, so a term occurring far more often than the cap
    /// still answers at most that many spans.
    #[test]
    fn the_span_cap_bounds_what_a_scan_collects() {
        let text = SearchableText::new(&"ab ".repeat(1_000));
        let terms = query_terms("ab");
        for max in [0usize, 1, 3, 10] {
            let spans = text.spans(&terms, max);
            assert_eq!(
                spans.len(),
                max,
                "case \"cap {max}\": expected the cap to bound the spans of 1000 occurrences, \
                 got {}",
                spans.len()
            );
        }
    }

    /// A document matches when any of its text carries a term, scores the share of the
    /// query's terms it carries, and excerpts the text the match was found in.
    #[test]
    fn a_document_scores_the_terms_it_carries_and_excerpts_where_they_were_found() {
        let bounds = WindowBounds {
            max_chars: 64,
            lead_chars: 8,
            max_spans: 8,
        };
        // The rows differ along which field carries the term, how many of the query's terms
        // the document carries, and whether the document has a readable body at all.
        let body = "The refund window is seven days from dispatch.";
        /// One row: the case, the document's fields, the query, and the score and excerpt
        /// it must answer with (or `None` for a document the query does not match).
        struct Row<'a> {
            case: &'a str,
            fields: DocumentFields<'a>,
            query: &'a str,
            expected: Option<(f64, &'a str)>,
        }
        let cases: Vec<Row<'_>> = vec![
            Row {
                case: "the body carries both terms",
                fields: DocumentFields {
                    name: "returns",
                    title: "Returns",
                    body: Some(body),
                },
                query: "refund dispatch",
                expected: Some((1.0, body)),
            },
            Row {
                case: "the body carries one of two terms",
                fields: DocumentFields {
                    name: "returns",
                    title: "Returns",
                    body: Some(body),
                },
                query: "refund missing",
                expected: Some((0.5, body)),
            },
            Row {
                case: "only the title carries the term",
                fields: DocumentFields {
                    name: "returns",
                    title: "Escalation runbook",
                    body: Some(body),
                },
                query: "escalation",
                expected: Some((1.0, "Escalation runbook")),
            },
            Row {
                case: "only the name carries the term, so the title is excerpted",
                fields: DocumentFields {
                    name: "refund-codes",
                    title: "Codes",
                    body: None,
                },
                query: "codes-refund",
                expected: Some((1.0, "Codes")),
            },
            Row {
                case: "a document with no readable body and no match",
                fields: DocumentFields {
                    name: "policy",
                    title: "Policy",
                    body: None,
                },
                query: "refund",
                expected: None,
            },
            Row {
                case: "a query naming no term",
                fields: DocumentFields {
                    name: "returns",
                    title: "Returns",
                    body: Some(body),
                },
                query: "  ,.  ",
                expected: None,
            },
        ];
        for row in cases {
            let case = row.case;
            let matched = match_document(row.fields, &query_terms(row.query), bounds);
            match (row.expected, &matched) {
                (None, found) => assert!(
                    found.is_none(),
                    "case {case:?}: expected no match, got {found:?}"
                ),
                (Some((score, excerpt)), Some(found)) => {
                    assert!(
                        (found.score - score).abs() < f64::EPSILON,
                        "case {case:?}: expected the score {score}, got {}",
                        found.score
                    );
                    assert_eq!(
                        found.excerpt.text, excerpt,
                        "case {case:?}: expected an excerpt of {excerpt:?}, got {:?}",
                        found.excerpt.text
                    );
                }
                (Some(_), None) => panic!("case {case:?}: expected a match, got none"),
            }
        }
    }
}
