// SPDX-License-Identifier: CC0-1.0

//! How an email template's content becomes one destination's message: the message
//! variables a send supplies, substituted into the template's subject line and its two
//! body parts.
//!
//! The RULE this module implements is the model's own reading of a template and its data.
//! `Template$TemplateData` is "an object that defines the values to use for message
//! variables in the template. This object is a set of key-value pairs. Each key defines a
//! message variable in the template. The corresponding value defines the value to use for
//! that variable", and `Template$TemplateContent` adds that Amazon SES "supports only
//! simple substitutions" for template content supplied in the request. So a render is
//! exactly that substitution: every `{{ name }}` a part writes is replaced by the value
//! the data gives that name, a name the data does not give renders as nothing, and the
//! part is otherwise the template's own text.
//!
//! What this does NOT do is interpret a template. There is no template LANGUAGE here and
//! no parser for one: a part that writes a section, an inverted section, a partial, a
//! comment or a helper call is reported as [`RenderRefusal::Unsupported`] rather than
//! rendered, so no operation ever answers with a message body this emulator invented for a
//! construct it does not evaluate. The calling operation decides what that refusal is on
//! the wire. The one text this module reads with a parser — the template DATA, which is
//! JSON — is read with `serde_json` ([`message_variables`]) rather than by hand.
//!
//! What bounds it, since the text it scans is a caller's: every input is bounded before it
//! gets here — a stored template is at most [`crate::state::MAX_EMAIL_TEMPLATE_BYTES`] and
//! `EmailTemplateData` is at most 262144 bytes in the model — the scan is ITERATIVE, so
//! there is no recursion on anything a caller controls and no nesting to limit (nesting is
//! what the refusal above covers), the substitution is a SINGLE pass that never re-reads
//! what it produced, so no value can be substituted into a value, and each part is built
//! against [`MAX_RENDERED_PART_BYTES`], which is the one way a template amplifies its
//! input: naming one variable many times. Malformed, truncated and oversized input is
//! covered by the operation's own suite
//! (`tests/all/send_bulk_email.rs`): text that is no JSON, an unterminated reference and a
//! part that renders past the ceiling.

use ::serde_json::Value;

/// The largest one rendered part — the subject line, the text body or the HTML body — may
/// be.
///
/// An EMULATOR ceiling, not an AWS quota: substitution is a single pass, but one template
/// may name the same variable many times, so a rendered part can be larger than the
/// template and the data that produced it together. The ceiling is the size a stored
/// template may itself have ([`crate::state::MAX_EMAIL_TEMPLATE_BYTES`], SES's documented
/// maximum template size), which is far above any message a template of that size
/// describes, and a render that reaches it produces no message at all.
pub const MAX_RENDERED_PART_BYTES: usize = crate::state::MAX_EMAIL_TEMPLATE_BYTES;

/// What a template's content renders to for one destination: the parts the template
/// carries, substituted.
///
/// A part the template does not carry is absent here too — a template with no HTML part
/// renders no HTML part — because the parts of the message are the parts of the template.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RenderedMessage {
    /// The rendered subject line.
    pub subject: Option<String>,
    /// The rendered text-only body.
    pub text: Option<String>,
    /// The rendered HTML body.
    pub html: Option<String>,
}

/// Why a render produced no message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderRefusal {
    /// A part writes something other than a message variable: a section (`{{#…}}`), an
    /// inverted section, a partial (`{{>…}}`), a comment or a helper call. Substituting
    /// variables is all this emulator does with a template, so a part that asks to be
    /// interpreted is not rendered at all.
    Unsupported,
    /// A part would render larger than [`MAX_RENDERED_PART_BYTES`].
    TooLarge,
}

/// The message variables `raw` supplies, or `None` when it is no set of key-value pairs.
///
/// "This object is a set of key-value pairs": a JSON object is the only value that is one,
/// so an array, a bare scalar and text that is no JSON at all supply no variables. The
/// EMPTY string is the one exception, and is the empty set: `EmailTemplateData` is
/// `length: min 0` in the model, so a caller may send it, and a send that states no value
/// for any variable is what it states.
///
/// The parse is `serde_json`'s, which returns an error rather than panicking on anything a
/// caller sends, and the input is bounded by the member's own 262144-byte maximum.
pub fn message_variables(raw: &str) -> Option<Value> {
    if raw.is_empty() {
        return Some(Value::Object(::serde_json::Map::new()));
    }
    let parsed: Value = ::serde_json::from_str(raw).ok()?;
    parsed.is_object().then_some(parsed)
}

/// `content` rendered with `variables`.
///
/// The HTML part's substituted values are HTML-escaped and the subject line's and the text
/// part's are not: the subject line and the text body are not HTML, so a value that
/// carries an HTML metacharacter belongs in them as the caller wrote it. A part that wants
/// a value in HTML as written says so with the unescaped form (`{{{name}}}` or
/// `{{&name}}`).
pub fn render(
    content: &crate::ops::types::EmailTemplateContent,
    variables: &Value,
) -> Result<RenderedMessage, RenderRefusal> {
    Ok(RenderedMessage {
        subject: substitute_part(content.subject.as_deref(), variables, Escape::None)?,
        text: substitute_part(content.text.as_deref(), variables, Escape::None)?,
        html: substitute_part(content.html.as_deref(), variables, Escape::Html)?,
    })
}

/// How a substituted value is written into the part it goes in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Escape {
    /// As the caller wrote it.
    None,
    /// With the HTML metacharacters replaced by their entities.
    Html,
}

/// One optional part, substituted.
fn substitute_part(
    part: Option<&str>,
    variables: &Value,
    escape: Escape,
) -> Result<Option<String>, RenderRefusal> {
    match part {
        None => Ok(None),
        Some(source) => substitute(source, variables, escape).map(Some),
    }
}

/// The opening of a variable reference, and of everything else a template may write.
const OPEN: &str = "{{";

/// The prefixes that make a reference something other than a message variable: a section,
/// a closing tag, an inverted section, a partial, an inline partial and a comment, in the
/// spellings a template writes them.
const NOT_A_VARIABLE: [char; 6] = ['#', '/', '^', '>', '!', '*'];

/// `source` with every message variable it names replaced by the value `variables` gives
/// that name.
///
/// ONE pass: the text a substitution produces is never scanned again, so a value that
/// itself reads like a reference is written out as the value it is. Unterminated text — a
/// `{{` with no closing `}}` after it — is no reference at all and is written out as the
/// template wrote it.
fn substitute(
    source: &str,
    variables: &Value,
    escape: Escape,
) -> Result<String, RenderRefusal> {
    let mut rendered = String::new();
    let mut rest = source;
    while let Some(at) = rest.find(OPEN) {
        let (literal, reference) = rest.split_at(at);
        push_bounded(&mut rendered, literal)?;
        // The unescaped form of a reference is written with three braces; the escaped one
        // with two.
        let (open, close) = if reference.starts_with("{{{") {
            ("{{{", "}}}")
        } else {
            (OPEN, "}}")
        };
        let body = &reference[open.len()..];
        let Some(end) = body.find(close) else {
            push_bounded(&mut rendered, reference)?;
            return Ok(rendered);
        };
        let name = body[..end].trim();
        rest = &body[end + close.len()..];
        // `{{&name}}` is the other spelling of the unescaped form, so the value goes in as
        // written whichever of the two the part used.
        let (name, escape) = match name.strip_prefix('&') {
            Some(unescaped) => (unescaped.trim(), Escape::None),
            None if open.len() == 3 => (name, Escape::None),
            None => (name, escape),
        };
        if !is_variable_path(name) {
            return Err(RenderRefusal::Unsupported);
        }
        let value = variable_text(variables, name);
        match escape {
            Escape::None => push_bounded(&mut rendered, &value)?,
            Escape::Html => push_bounded(&mut rendered, &html_escaped(&value))?,
        }
    }
    push_bounded(&mut rendered, rest)?;
    Ok(rendered)
}

/// Append `text` to `rendered`, refusing before the part grows past
/// [`MAX_RENDERED_PART_BYTES`].
///
/// The check precedes the append, so the ceiling bounds what is allocated rather than
/// reporting on it afterwards.
fn push_bounded(rendered: &mut String, text: &str) -> Result<(), RenderRefusal> {
    if rendered.len().saturating_add(text.len()) > MAX_RENDERED_PART_BYTES {
        return Err(RenderRefusal::TooLarge);
    }
    rendered.push_str(text);
    Ok(())
}

/// Is `name` the name of a message variable?
///
/// A key of the data, or a dotted path of keys into it: "each key defines a message
/// variable in the template". Anything else — empty text, a prefix that makes the
/// reference a section, a partial or a comment, a helper call (which carries a space
/// between its name and its arguments), a subscript — is not a variable, so a template
/// that writes one is not rendered here.
fn is_variable_path(name: &str) -> bool {
    let Some(first) = name.chars().next() else {
        return false;
    };
    if NOT_A_VARIABLE.contains(&first) {
        return false;
    }
    name.split('.').all(|segment| {
        !segment.is_empty()
            && segment
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "_-".contains(character))
    })
}

/// The text the value at `path` is written as, or the empty string when the data states no
/// value for it.
///
/// A name the data does not give renders as nothing, which is what a template's own
/// non-strict reading of a missing variable is. A value that is text is written as that
/// text; any other JSON value is written as the JSON it is, which is the only rendering of
/// it the data states.
fn variable_text(variables: &Value, path: &str) -> String {
    let mut found = variables;
    for segment in path.split('.') {
        match found.get(segment) {
            Some(next) => found = next,
            None => return String::new(),
        }
    }
    match found {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// `value` with the HTML metacharacters replaced by their entities, so a substituted value
/// cannot close or open an element of the part it is written into.
fn html_escaped(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#x27;"),
            '`' => escaped.push_str("&#x60;"),
            '=' => escaped.push_str("&#x3D;"),
            other => escaped.push(other),
        }
    }
    escaped
}
