// SPDX-License-Identifier: CC0-1.0

//! The ONE bound on how deeply a caller-supplied YAML document NESTS, taken
//! before that document is handed to a parser.
//!
//! Several emulated services take a YAML (or JSON, which YAML 1.2 is a superset
//! of) document as a string member — a CloudFormation template, an SSM document, a
//! Greengrass recipe, a CodeDeploy AppSpec — and read it with a maintained YAML
//! implementation. A parser costs a level of recursion per nested collection, and
//! the text is a caller's, so how deeply it nests has to be bounded BEFORE the
//! parse rather than observed during or after it: a bound the parse has already
//! blown past is not a bound. A byte bound alone does not do it, because each
//! level of nesting costs as little as one byte.
//!
//! This is that bound, and it is only a scan of the TEXT: no document is parsed,
//! nothing is allocated per nesting level, and it says nothing about whether the
//! document is well formed — that is the parser's answer, asked only once the text
//! is known to be shallow enough to ask about.
//!
//! It covers every form YAML nests in: the flow collections (`[`, `{`), the block
//! sequence entries and complex keys (`-`, `?` standing alone) and the BLOCK
//! MAPPINGS, which carry no indicator byte at all and nest by indentation — a
//! level of which needs at least one column, so a line's indentation is itself an
//! upper bound on how many levels it sits inside.
//!
//! What it does NOT bound, and the caller still has to: ANCHOR AND ALIAS
//! EXPANSION. A document that nests shallowly can still expand enormously at parse
//! time, because a loader resolves an alias by copying the anchored node. Nesting
//! and expansion are different quantities; a caller that cares about the second
//! refuses an alias outright (the CodeDeploy AppSpec reader does, over the parser's
//! event stream, which copies nothing) and must not drop that guard on the strength
//! of this one.
//!
//! Several services hand-rolled an equivalent scan before it was stated here; the
//! CodeDeploy AppSpec reader is the first caller migrated onto it, and the others
//! (the SSM document content, the Greengrass recipe, the CloudFormation template
//! body, the Config conformance pack, the API Gateway REST definition, the Image
//! Builder component document and the Observability Admin pipeline configuration)
//! still hold their own — none of which should be dropped before its caller is
//! migrated here. This is the destination they are to be migrated onto; new code
//! should call it rather than write another.

/// An upper bound on the nesting `document` would make a parser walk, counted no
/// further than `limit` plus one — so a caller's bound is decided without walking
/// the whole of a document that blew past it.
///
/// The figure is the number of nesting INDICATORS the text carries plus the
/// deepest INDENTATION any of its lines is written at:
///
/// * a flow collection opens with `[` or `{`;
/// * a block sequence entry and a complex key open with `-` or `?` STANDING ALONE
///   — the byte is an indicator only when what surrounds it is whitespace or an
///   end of the document, so a hyphen inside a function name, an ARN or a date is
///   not one;
/// * a block mapping opens no indicator at all and nests by indentation, and
///   needs a column of it per level, so the columns of the deepest-indented line
///   bound how many such levels the document can hold.
///
/// It is an UPPER bound rather than an exact depth — a `-` standing alone inside a
/// quoted scalar is counted, nesting written side by side is added up rather than
/// maximised, and an indentation step of several columns counts a level per column
/// — which is the safe direction for a bound: it can refuse a document the parser
/// would have managed, never admit one it would not.
#[must_use]
pub fn nesting_depth_bound(document: &str, limit: usize) -> usize {
    let bytes = document.as_bytes();
    let separates = |index: usize| {
        bytes
            .get(index)
            .is_none_or(|byte| matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
    };
    // Where the scan is: how many indicators it has seen, the deepest indentation
    // of any line so far, this line's indentation and whether it is still being
    // counted (an indentation run ends at the line's first other byte).
    let mut indicators = 0usize;
    let mut deepest_indent = 0usize;
    let mut indent = 0usize;
    let mut counting_indent = true;
    for (index, byte) in bytes.iter().copied().enumerate() {
        match byte {
            b'\n' | b'\r' => {
                indent = 0;
                counting_indent = true;
            }
            b' ' | b'\t' if counting_indent => {
                indent += 1;
                deepest_indent = deepest_indent.max(indent);
            }
            b'[' | b'{' => {
                counting_indent = false;
                indicators += 1;
            }
            b'-' | b'?'
                if index.checked_sub(1).is_none_or(separates) && separates(index + 1) =>
            {
                counting_indent = false;
                indicators += 1;
            }
            _ => counting_indent = false,
        }
        if indicators + deepest_indent > limit {
            return limit.saturating_add(1);
        }
    }
    indicators + deepest_indent
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The limit every row below is counted against.
    const LIMIT: usize = 8;

    /// The rows differ along which form of nesting the text uses and along
    /// whether the bytes that look like structure open a level at all: flow
    /// collections, block sequence entries, complex keys, block mappings nesting
    /// by indentation alone, and the same characters inside words and numbers.
    #[test]
    fn every_form_of_nesting_is_counted_and_nothing_else_is() {
        let rows: &[(&str, &str, usize)] = &[
            ("empty text", "", 0),
            ("a flat mapping", "a: 1\nb: two\n", 0),
            ("flow collections", "{\"a\": [1, 2]}", 2),
            // Two entries, each indented two columns: two indicators and the two
            // columns the deepest line is written at.
            ("block sequence entries", "a:\n  - one\n  - two\n", 4),
            ("compact entries on one line", "- - - 1\n", 3),
            ("a complex key", "? a\n: 1\n", 1),
            (
                "a block mapping nesting by indentation alone",
                "a:\n b:\n  c:\n   d: 1\n",
                3,
            ),
            (
                "hyphens inside names, ARNs and dates",
                "arn: arn:aws:lambda:us-west-2:0:function:orders-fn-x\non: 2026-01-02\n",
                0,
            ),
            ("a negative number", "a: -1\n", 0),
            (
                "a hyphen standing alone inside a quoted scalar, counted as the \
                 upper bound this is",
                "a: \"one - two - three\"\n",
                2,
            ),
        ];
        for (case, document, expected) in rows {
            let counted = nesting_depth_bound(document, LIMIT);
            assert_eq!(
                counted, *expected,
                "case {case:?}: expected a nesting bound of {expected}, got {counted}"
            );
        }
    }

    /// The count stops at one past the limit, so a document that opens a level per
    /// byte is answered without walking it: what the caller needs is only whether
    /// the limit was passed. Each row nests in a different form, and each is a
    /// document a byte cap alone would admit.
    #[test]
    fn a_document_past_the_limit_is_answered_without_being_walked() {
        let rows: &[(&str, String)] = &[
            ("a million flow openers", "[".repeat(1_000_000)),
            (
                "a block sequence of a million entries",
                "a:\n".to_string() + &"  - x\n".repeat(1_000_000),
            ),
            (
                "a block mapping nested by indentation, which carries no indicator",
                (0..1_000)
                    .map(|level| format!("{}k{level}:\n", " ".repeat(level)))
                    .collect::<String>(),
            ),
        ];
        for (case, document) in rows {
            let counted = nesting_depth_bound(document, LIMIT);
            assert_eq!(
                counted,
                LIMIT + 1,
                "case {case:?}: a document past the limit must count {} and stop, got {counted}",
                LIMIT + 1
            );
        }
    }
}
