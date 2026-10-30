// SPDX-License-Identifier: CC0-1.0

//! The template document a publisher supplies INLINE: read, and quoted back by a refusal.
//!
//! A publish carries "the raw packaged AWS SAM template of your application"
//! (`smithy_query(query="doc", shape="CreateApplicationVersionRequest",
//! member="TemplateBody")`), and a packaged AWS SAM template *is* a CloudFormation template
//! — which CloudFormation accepts in EITHER of the two forms it documents: JSON, or YAML
//! with the short-form intrinsic tags (`!Ref`, `!Sub`, `!GetAtt`, …). So everything
//! [`crate::state`] derives from a supplied template (the SAM `Transform` requirement, the
//! `ParameterDefinitions` a version reports, the capabilities a deployment of it must
//! acknowledge) is derived from the document, never from the form the body arrived in.
//!
//! Reading that format is NOT done again here: the emulator already has exactly one reader
//! of it — AWS CloudFormation's own
//! [`template::document_within`](service_com_amazonaws_cloudformation_cloudformation::state::template::document_within),
//! which tries JSON, then walks the YAML event stream so the short-form tags survive as the
//! long form a JSON template spells (`!Ref x` → `{"Ref": "x"}`), bounds an untrusted body's
//! nesting before the parser sees it, and reads neither a second document nor a YAML alias.
//! This service reads its own bodies through it under its own documented ceiling
//! ([`crate::state::MAX_TEMPLATE_BYTES`], the maximum of a packaged template stored as an S3
//! object), so the two services cannot disagree about what one template says.

use ::serde_json::Value;
use ::service_com_amazonaws_cloudformation_cloudformation::state::template;

/// `body` as the template document it spells, in whichever of the two forms it is written,
/// or `None` for a body that is not a template document at all.
///
/// A template's top level is a MAPPING of the sections CloudFormation's template anatomy
/// names (`Transform`, `Parameters`, `Resources`, …), so that — and only that — is what a
/// template document is here: a body that reads as a scalar, a list, several documents, or
/// as neither form is not a document this service can judge, and is stored as supplied
/// without one derived member being guessed at.
pub fn read(body: &str) -> Option<Value> {
    match template::document_within(body, crate::state::MAX_TEMPLATE_BYTES)? {
        object @ Value::Object(_) => Some(object),
        _ => None,
    }
}

/// The template document as the refusal of one carrying no SAM `Transform` quotes it back.
///
/// That refusal names the document it rejected, and what it names is the PARSED document
/// rather than the bytes the publisher sent: an ordered mapping as
/// `OrderedDict({'<key>': <value>, …})` (an empty one as `OrderedDict()`), a list as
/// `['a', 'b']`, text in quotes, a number as the document spells it, and `True`, `False`
/// and `None` for a boolean and a null. Rendering it from the document that was READ is
/// what makes the quote the same for both forms a publisher may write one in.
///
/// A mapping's entries are rendered in KEY order, deliberately: the tree the reader hands
/// back is keyed, not ordered, so the order the sections were WRITTEN in is not available
/// here — and an order that is a function of the document alone is what makes two refusals
/// of one template read identically. Every value in the quote is the document's own.
pub fn echoed(template: &Value) -> String {
    let mut rendered = String::new();
    render(template, &mut rendered);
    rendered
}

/// One node of the echo; see [`echoed`].
fn render(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("None"),
        Value::Bool(true) => out.push_str("True"),
        Value::Bool(false) => out.push_str("False"),
        Value::Number(number) => out.push_str(&number.to_string()),
        Value::String(text) => render_text(text, out),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                render(item, out);
            }
            out.push(']');
        }
        Value::Object(entries) => {
            if entries.is_empty() {
                out.push_str("OrderedDict()");
                return;
            }
            // Sorted here rather than relying on the map's own iteration order, which is
            // the one thing about the tree that depends on how `serde_json` was built.
            let mut keyed: Vec<(&String, &Value)> = entries.iter().collect();
            keyed.sort_by_key(|(key, _)| *key);
            out.push_str("OrderedDict({");
            for (index, (key, entry)) in keyed.into_iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                render_text(key, out);
                out.push_str(": ");
                render(entry, out);
            }
            out.push_str("})");
        }
    }
}

/// One string of the echo, quoted: in `'…'`, or in `"…"` for text that itself carries a
/// `'` and no `"`, with the quote in use and the control characters escaped.
fn render_text(text: &str, out: &mut String) {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    out.push(quote);
    for character in text.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            same if same == quote => {
                out.push('\\');
                out.push(same);
            }
            other => out.push(other),
        }
    }
    out.push(quote);
}
