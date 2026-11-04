// SPDX-License-Identifier: CC0-1.0

//! The GROUP a trace query scopes itself to.
//!
//! A trace-summaries query carries its group alongside its `FilterExpression`:
//! the group is a stored resource with a name and an ARN
//! ([`super::GroupRecord`]), not a field of a trace, so a reference to one is
//! RESOLVED against the stored groups — and the expression the group carries is
//! what then narrows the query — rather than being handed to the
//! filter-expression grammar, which knows only fields a trace has a value for.
//! Feeding it to that grammar reports a group that does not exist as a syntax
//! error the caller did not make, and leaves a group that DOES exist unable to
//! scope anything.
//!
//! So this module SPLITS a query into the two parts it is made of
//! ([`split`]): the group references it names, and what is left of the filter
//! expression, which is the part the grammar reads. A reference is written in
//! any of the forms that name one of the two selectors a group has — the
//! modelled `GroupName` / `GroupARN` of every other group-scoped read
//! (`smithy_query(query="operation",operation="GetServiceGraph")`):
//!
//! ```text
//! group = "<name>"            group("<name>")
//! group.name = "<name>"       group.groupname = "<name>"
//! group.arn = "<arn>"         group.grouparn = "<arn>"
//! ```
//!
//! and a value carrying the prefix every ARN starts with
//! ([`super::ARN_PREFIX`]) names the group by ARN whichever of the two
//! name-shaped forms carried it, because an ARN is one of the two identities a
//! group has and no group is named one.
//!
//! Anything else is left exactly where it stood, so a term this module does not
//! recognise is still read — and still refused — by the one parser the crate
//! reads this language with, unchanged. A reference is taken out with ONE
//! conjunction beside it, written as the word or as the doubled symbol, so what
//! is left is the expression the caller wrote about the traces themselves.

use super::ARN_PREFIX;

/// Which of a group's two selectors a reference names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupSelector {
    /// The group's `GroupName`.
    Name(String),
    /// The group's `GroupARN`.
    Arn(String),
}

impl GroupSelector {
    /// The `GroupName` this selector supplies, if it selects by name.
    #[must_use]
    pub fn group_name(&self) -> Option<&str> {
        match self {
            Self::Name(name) => Some(name),
            Self::Arn(_) => None,
        }
    }

    /// The `GroupARN` this selector supplies, if it selects by ARN.
    #[must_use]
    pub fn group_arn(&self) -> Option<&str> {
        match self {
            Self::Arn(arn) => Some(arn),
            Self::Name(_) => None,
        }
    }

    /// The selector a reference's value names: an ARN when the value carries
    /// the prefix every ARN starts with, else the name.
    fn of_value(value: String, by_arn: bool) -> Self {
        if by_arn || value.starts_with(ARN_PREFIX) {
            Self::Arn(value)
        } else {
            Self::Name(value)
        }
    }
}

/// A trace query split into its group scope and its filter expression.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScopedQuery {
    /// Every group the query scoped itself to, in the order it named them. A
    /// query naming more than one is scoped to all of them.
    pub groups: Vec<GroupSelector>,
    /// What is left for the filter-expression parser, or `None` when the query
    /// was nothing but its group scope.
    pub filter_expression: Option<String>,
}

/// Split the group references `source` names out of the filter expression
/// around them.
///
/// The scan steps over quoted strings, so text inside a literal is never read
/// as a reference, and it removes one conjunction beside each reference it
/// takes, so what is left is the expression the caller wrote about the traces
/// themselves. A reference in none of the recognised forms is left in place.
///
/// One pass over the source, and only over a source the parser would read at
/// all: an expression longer than the bound that parser declares
/// ([`super::filter_expression::MAX_LENGTH`]) is handed straight back for it to
/// refuse by that bound, so the work this does is bounded by the same number of
/// characters the parse is.
#[must_use]
pub fn split(source: &str) -> ScopedQuery {
    if source.chars().count() > super::filter_expression::MAX_LENGTH {
        return ScopedQuery {
            groups: Vec::new(),
            filter_expression: Some(source.to_string()),
        };
    }
    let characters: Vec<char> = source.chars().collect();
    let mut groups: Vec<GroupSelector> = Vec::new();
    // The spans taken out of the source, in the order they were read, so what
    // is left can be rebuilt in one pass below.
    let mut taken: Vec<(usize, usize)> = Vec::new();
    let mut at = 0;
    while at < characters.len() {
        match characters[at] {
            quote @ ('"' | '\'') => at = past_literal(&characters, at, quote),
            character if starts_a_term(&characters, at, character) => {
                let (path, after_path) = word(&characters, at);
                match group_reference(&characters, &path, after_path) {
                    Some((selector, end)) => {
                        groups.push(selector);
                        taken.push(connective_around(&characters, at, end));
                        at = end;
                    }
                    None => at = after_path,
                }
            }
            _ => at += 1,
        }
    }
    ScopedQuery {
        filter_expression: remainder(&characters, &taken),
        groups,
    }
}

/// Where the literal opened at `at` by `quote` ends, one past its closing
/// quote. An unclosed literal runs to the end of the input, which is what the
/// parser itself makes of one.
fn past_literal(characters: &[char], at: usize, quote: char) -> usize {
    let mut scan = at + 1;
    while scan < characters.len() {
        match characters[scan] {
            '\\' => scan += 2,
            closing if closing == quote => return scan + 1,
            _ => scan += 1,
        }
    }
    characters.len()
}

/// Whether a term can BEGIN at `at`: the character is one a keyword path starts
/// with, and what precedes it is not part of a path, so the `group` inside
/// another keyword is never read as one.
fn starts_a_term(characters: &[char], at: usize, character: char) -> bool {
    if !(character.is_alphabetic() || character == '_') {
        return false;
    }
    at == 0 || !is_path_character(characters[at - 1])
}

/// The characters a keyword path is written with, which is also what decides
/// where one ends.
fn is_path_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_' || character == '.' || character == '-'
}

/// The path starting at `at`, lowercased, and the offset just past it.
fn word(characters: &[char], at: usize) -> (String, usize) {
    let mut end = at;
    while end < characters.len() && is_path_character(characters[end]) {
        end += 1;
    }
    (
        characters[at..end]
            .iter()
            .collect::<String>()
            .to_ascii_lowercase(),
        end,
    )
}

/// The group reference whose path is `path`, read from `after_path` on, and the
/// offset just past the whole reference — or `None` when `path` names no group
/// selector, or the text after it is not one of the recognised forms.
fn group_reference(
    characters: &[char],
    path: &str,
    after_path: usize,
) -> Option<(GroupSelector, usize)> {
    let by_arn = match path {
        "group" | "group.name" | "group.groupname" => false,
        "group.arn" | "group.grouparn" => true,
        _ => return None,
    };
    let at = past_whitespace(characters, after_path);
    match characters.get(at)? {
        // `group.name = "<value>"`: the comparison every selector form makes.
        '=' => {
            let (value, end) = value(characters, past_whitespace(characters, at + 1))?;
            Some((GroupSelector::of_value(value, by_arn), end))
        }
        // `group("<value>")`: the keyword called with the group it names, which
        // only the bare keyword takes.
        '(' if path == "group" => {
            let (value, after_value) = value(characters, past_whitespace(characters, at + 1))?;
            let closing = past_whitespace(characters, after_value);
            if characters.get(closing) != Some(&')') {
                return None;
            }
            Some((GroupSelector::of_value(value, by_arn), closing + 1))
        }
        _ => None,
    }
}

/// The first offset at or after `at` that is not whitespace.
fn past_whitespace(characters: &[char], at: usize) -> usize {
    let mut scan = at;
    while scan < characters.len() && characters[scan].is_whitespace() {
        scan += 1;
    }
    scan
}

/// The value at `at` — a quoted string with its escapes resolved, or a bare run
/// of value characters — and the offset just past it. `None` for a place no
/// value is written at, which is what makes the text no reference at all.
fn value(characters: &[char], at: usize) -> Option<(String, usize)> {
    match characters.get(at)? {
        quote @ ('"' | '\'') => {
            let mut text = String::new();
            let mut scan = at + 1;
            while scan < characters.len() {
                match characters[scan] {
                    '\\' => {
                        text.push(*characters.get(scan + 1)?);
                        scan += 2;
                    }
                    closing if closing == *quote => return Some((text, scan + 1)),
                    other => {
                        text.push(other);
                        scan += 1;
                    }
                }
            }
            // A literal the expression never closes names no value.
            None
        }
        _ => {
            let mut scan = at;
            while characters
                .get(scan)
                .is_some_and(|character| is_value_character(*character))
            {
                scan += 1;
            }
            (scan > at).then(|| (characters[at..scan].iter().collect(), scan))
        }
    }
}

/// The characters an UNQUOTED value is written with: a group's name, or an ARN,
/// which carries colons and a slash.
fn is_value_character(character: char) -> bool {
    is_path_character(character) || character == ':' || character == '/'
}

/// The span to take out for a reference occupying `[start, end)`: the reference
/// itself plus ONE conjunction beside it — worded or doubled-symbol — so
/// removing the scope leaves the expression around it well formed.
fn connective_around(characters: &[char], start: usize, end: usize) -> (usize, usize) {
    if let Some(before) = connective_before(characters, start) {
        return (before, end);
    }
    (start, connective_after(characters, end))
}

/// The conjunction written as a doubled symbol, which joins terms exactly as the
/// word does.
const SYMBOL_CONJUNCTION: &str = "&&";

/// Where a conjunction that ENDS just before `start` begins, skipping the
/// whitespace between the two.
fn connective_before(characters: &[char], start: usize) -> Option<usize> {
    let mut scan = start;
    while scan > 0 && characters[scan - 1].is_whitespace() {
        scan -= 1;
    }
    let after = scan;
    if after >= SYMBOL_CONJUNCTION.len()
        && characters[after - SYMBOL_CONJUNCTION.len()..after]
            .iter()
            .collect::<String>()
            == SYMBOL_CONJUNCTION
    {
        return Some(after - SYMBOL_CONJUNCTION.len());
    }
    while scan > 0 && is_path_character(characters[scan - 1]) {
        scan -= 1;
    }
    let word: String = characters[scan..after]
        .iter()
        .collect::<String>()
        .to_ascii_lowercase();
    (word == "and").then_some(scan)
}

/// Where a conjunction that STARTS just after `end` leaves off, skipping the
/// whitespace between the two.
fn connective_after(characters: &[char], end: usize) -> usize {
    let at = past_whitespace(characters, end);
    if characters[at..]
        .iter()
        .take(SYMBOL_CONJUNCTION.len())
        .collect::<String>()
        == SYMBOL_CONJUNCTION
    {
        return at + SYMBOL_CONJUNCTION.len();
    }
    let (word, after_word) = word(characters, at);
    if word == "and" {
        after_word
    } else {
        end
    }
}

/// What is left of the source once `taken` is removed: the text around the
/// spans, or `None` when nothing but whitespace remains.
///
/// A source nothing was taken out of is handed back EXACTLY as it was
/// submitted, down to its leading whitespace: the parser that reads it next
/// reports where in the caller's own text it gave up, so this module must not
/// move a character it did not have to.
fn remainder(characters: &[char], taken: &[(usize, usize)]) -> Option<String> {
    if taken.is_empty() {
        return Some(characters.iter().collect());
    }
    let mut left = String::new();
    let mut at = 0;
    for (start, end) in taken {
        if *start > at {
            left.extend(&characters[at..*start]);
        }
        at = (*end).max(at);
    }
    if at < characters.len() {
        left.extend(&characters[at..]);
    }
    let left = left.trim().to_string();
    (!left.is_empty()).then_some(left)
}

#[cfg(test)]
mod tests {
    use super::{split, GroupSelector, ScopedQuery};

    /// Every recognised form names the group it carries, and leaves the rest of
    /// the expression — and only the rest — for the parser.
    #[test]
    fn a_reference_is_taken_out_of_the_expression_it_stands_in() {
        let name = |name: &str| GroupSelector::Name(name.to_string());
        let arn = |arn: &str| GroupSelector::Arn(arn.to_string());
        // (case, source, the groups named, what is left for the parser)
        let cases: Vec<(&str, &str, Vec<GroupSelector>, Option<&str>)> = vec![
            (
                "the bare keyword compared to a name",
                r#"group = "checkout""#,
                vec![name("checkout")],
                None,
            ),
            (
                "the name selector, with an expression after it",
                r#"group.name = "checkout" AND fault"#,
                vec![name("checkout")],
                Some("fault"),
            ),
            (
                "the name selector, with an expression before it",
                r#"fault and group.name = "checkout""#,
                vec![name("checkout")],
                Some("fault"),
            ),
            (
                "the modelled member spelling",
                r#"group.groupName = 'checkout'"#,
                vec![name("checkout")],
                None,
            ),
            (
                "the arn selector",
                r#"group.arn = "arn:aws:xray:us-east-1:1:group/checkout/7""#,
                vec![arn("arn:aws:xray:us-east-1:1:group/checkout/7")],
                None,
            ),
            (
                "an arn carried by a name-shaped form",
                r#"group = "arn:aws:xray:us-east-1:1:group/checkout/7""#,
                vec![arn("arn:aws:xray:us-east-1:1:group/checkout/7")],
                None,
            ),
            (
                "the call form",
                r#"group("checkout") and http.status = 500"#,
                vec![name("checkout")],
                Some("http.status = 500"),
            ),
            (
                "an unquoted value",
                "group.name = checkout and user = \"ana\"",
                vec![name("checkout")],
                Some(r#"user = "ana""#),
            ),
            (
                "two references scope the query to both",
                r#"group.name = "checkout" and group.name = "payments""#,
                vec![name("checkout"), name("payments")],
                None,
            ),
            (
                "the keyword inside a literal is a value, not a reference",
                r#"annotation.note = "group.name = \"checkout\"""#,
                vec![],
                Some(r#"annotation.note = "group.name = \"checkout\"""#),
            ),
            (
                "a suffix that names no selector is left for the parser",
                r#"group.owner = "ana""#,
                vec![],
                Some(r#"group.owner = "ana""#),
            ),
            (
                "a path the keyword only prefixes is left alone",
                r#"groupname = "checkout""#,
                vec![],
                Some(r#"groupname = "checkout""#),
            ),
            (
                "a reference joined by the doubled-symbol conjunction",
                r#"group.name = "checkout" && fault"#,
                vec![name("checkout")],
                Some("fault"),
            ),
            ("no reference at all", "fault", vec![], Some("fault")),
            (
                "a reference the input cuts short names no group",
                r#"group.name ="#,
                vec![],
                Some(r#"group.name ="#),
            ),
            (
                "a reference whose value nothing terminates names no group",
                r#"group.name = "checkout"#,
                vec![],
                Some(r#"group.name = "checkout"#),
            ),
            (
                "a bare keyword with nothing after it names no group",
                "group",
                vec![],
                Some("group"),
            ),
        ];
        for (case, source, groups, filter_expression) in cases {
            let expected = ScopedQuery {
                groups,
                filter_expression: filter_expression.map(str::to_string),
            };
            let scoped = split(source);
            assert_eq!(
                scoped, expected,
                "case {case:?}: expected {expected:?}, got {scoped:?}"
            );
        }
    }

    /// A source longer than the parser's own bound is handed straight back, so
    /// the scan this module does is bounded by the same number of characters the
    /// parse is — and the expression is then refused by that bound rather than
    /// by anything here.
    #[test]
    fn a_source_past_the_parsers_bound_is_handed_back_whole() {
        let bound = super::super::filter_expression::MAX_LENGTH;
        let oversized = format!("group.name = \"{}\"", "c".repeat(bound));
        let scoped = split(&oversized);
        assert_eq!(
            scoped,
            ScopedQuery {
                groups: Vec::new(),
                filter_expression: Some(oversized.clone()),
            },
            "an oversized source must be handed back whole, got {scoped:?}"
        );
        // And one just inside the bound is still split, so the guard is on the
        // length and not on the form.
        let inside = format!("group.name = \"{}\"", "c".repeat(16));
        let scoped = split(&inside);
        assert_eq!(
            scoped.groups,
            vec![GroupSelector::Name("c".repeat(16))],
            "a source inside the bound must still be split, got {scoped:?}"
        );
    }
}
