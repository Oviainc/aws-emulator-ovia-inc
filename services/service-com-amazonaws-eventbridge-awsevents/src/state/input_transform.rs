// SPDX-License-Identifier: CC0-1.0

//! EventBridge's INPUT TRANSFORMATION: what a rule's target receives when the
//! target does not take the whole event envelope.
//!
//! Moved out of `crate::state` unchanged, so one file holds one concern; the
//! rule it implements is stated where the module is declared
//! ([`crate::state`]).

use ::serde_json::Value;

/// The prefix EventBridge reserves for the placeholders it fills itself:
/// "the keys cannot start with `Amazon Web Services.`"
/// (`InputTransformer$InputPathsMap`), which is the `aws.` namespace on the
/// wire — so a key in it is never a caller's path and is always one of
/// [`reserved`]'s.
const RESERVED_KEY_PREFIX: &str = "aws.";

/// The placeholders EventBridge fills from the DELIVERY rather than from the
/// event: which rule matched, under which ARN, and when the event was
/// ingested.
#[derive(Debug, Clone, Copy)]
pub struct Delivery<'a> {
    /// The name of the rule whose target is being delivered to.
    pub rule: &'a str,
    /// That rule's ARN.
    pub rule_arn: &'a str,
    /// The instant the event carries, as the envelope's `time` renders it.
    pub ingestion_time: &'a str,
}

/// The value the reserved placeholder `key` carries, or `None` when the
/// reserved namespace holds no such placeholder.
///
/// The five EventBridge fills itself: the whole event (`aws.events.event`),
/// the whole event as JSON text (`aws.events.event.json`), the matched
/// rule's ARN and name, and the event's ingestion time.
fn reserved(key: &str, event: &Value, delivery: &Delivery<'_>) -> Option<Value> {
    match key {
        "aws.events.event" | "aws.events.event.json" => Some(event.clone()),
        "aws.events.rule-arn" => Some(Value::String(delivery.rule_arn.to_string())),
        "aws.events.rule-name" => Some(Value::String(delivery.rule.to_string())),
        "aws.events.event.ingestion-time" => {
            Some(Value::String(delivery.ingestion_time.to_string()))
        }
        _ => None,
    }
}

/// What `path` selects out of `event`, as an input transformation renders
/// it: the value at the one place the path names, the list of what a slice
/// or wildcard in it spans, and `null` for a path that selects NOTHING in
/// this event — JSON's empty value, which is what a placeholder for an
/// absent field carries.
///
/// `Err` is the path this service did NOT evaluate: a form the bounded
/// reader does not read, or a selection past the places it selects. That is
/// not a path that selected nothing, so it is never rendered as the empty
/// value — the caller refuses the delivery instead of handing the target a
/// payload built from a path nobody evaluated. The message names the path,
/// which is what a consumer of the invocation log has to see to fix it.
pub fn selected(path: &str, event: &Value) -> Result<Value, String> {
    use ::service_common::json_reference_path::{select_many, selection_segments, Selected};

    let Some(named) = selection_segments(path) else {
        return Err(format!(
            "the path {path} is not one this emulator evaluates"
        ));
    };
    match select_many(&named, event) {
        Selected::Value(value) => Ok(value),
        Selected::NoMatch => Ok(Value::Null),
        Selected::NotSelected => Err(format!(
            "the path {path} names more places than the {} this emulator selects",
            ::service_common::json_reference_path::MAX_SELECTED_ELEMENTS
        )),
    }
}

/// What a target carrying `InputPath` receives: the JSON text of the one
/// part of the event that path selects — and `Err` for a path this service
/// did not evaluate ([`selected`]).
pub fn by_input_path(path: &str, event: &Value) -> Result<String, String> {
    selected(path, event).map(|selected| selected.to_string())
}

/// What a target carrying an `InputTransformer` receives: `template` with
/// each `<key>` placeholder replaced by what `paths` selects for that key.
///
/// The template's own FORM decides how a value is written, because the
/// documented templates are of two kinds:
///
/// * one that opens a JSON value — an object, an array or a string — carries
///   its placeholders "in quotes or out"
///   (`InputTransformer$InputTemplate`), so a placeholder standing as a JSON
///   value carries the selected value's JSON text and one standing inside a
///   string carries that value's characters, escaped for the string it lands
///   in — `{"n": <n>}` renders a number and `{"n": "<n>"}` renders that
///   number's digits as a string, and neither can break the template's own
///   JSON;
/// * any other template is TEXT (`"<instance> is in state <status>"`): every
///   placeholder carries the value's characters and every character the
///   template itself wrote is delivered as written.
///
/// A key the map does not hold and the reserved namespace does not name is
/// not a placeholder at all: the text is left exactly as the template wrote
/// it, since nothing says what it stands for.
///
/// `Err` is a transformation this service will not build: a path it did not
/// evaluate ([`selected`]), or a rendering past [`MAX_RENDERED_BYTES`] — a
/// template may repeat one placeholder as often as its own length allows and
/// each fill carries a value as large as the event, so the PRODUCT of the
/// two is what that bound refuses. Either way nothing is delivered rather
/// than a payload the target never asked for.
pub fn render(
    template: &str,
    paths: &::std::collections::BTreeMap<String, String>,
    event: &Value,
    delivery: &Delivery<'_>,
) -> Result<String, String> {
    let structured = template
        .trim_start()
        .starts_with(['{', '[', '"']);
    let mut rendered = String::with_capacity(template.len());
    // Whether the scan stands inside a string literal of a STRUCTURED
    // template, and whether the character before it escaped this one, which
    // is what tells a closing quote from an escaped one. A text template has
    // no JSON structure to stand inside.
    let mut in_string = false;
    let mut escaped = false;
    let mut rest = template;
    while !rest.is_empty() {
        // A placeholder is read before the character opening it is
        // classified: `<` is never part of a template's own JSON structure.
        if let Some((key, tail)) = placeholder(rest).filter(|_| !escaped) {
            if let Some(value) = filling(key, paths, event, delivery)? {
                if structured && !in_string {
                    rendered.push_str(&value.to_string());
                } else if structured {
                    rendered.push_str(&escaped_text(&value));
                } else {
                    rendered.push_str(&text(&value));
                }
                if rendered.len() > MAX_RENDERED_BYTES {
                    return Err(format!(
                        "the transformation renders more than the {MAX_RENDERED_BYTES} \
                         bytes this emulator renders"
                    ));
                }
                rest = tail;
                continue;
            }
        }
        let character = rest.chars().next().unwrap_or_default();
        if structured {
            if in_string {
                if escaped {
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == '"' {
                    in_string = false;
                }
            } else if character == '"' {
                in_string = true;
            }
        }
        rendered.push(character);
        rest = &rest[character.len_utf8()..];
    }
    Ok(rendered)
}

/// The most bytes ONE rendering may produce.
///
/// This service's own bound, not a limit AWS publishes: `InputTemplate` is
/// modelled at 8192 characters and each placeholder in it may carry a value
/// as large as a published entry ([`super::MAX_ENTRY_SIZE_BYTES`]), so their
/// product is not bounded by either. One rendering is one target's payload,
/// so it is held to the size of the event it was built from.
pub const MAX_RENDERED_BYTES: usize = super::MAX_ENTRY_SIZE_BYTES;

/// What the placeholder `key` is filled with: what the caller's own path for
/// it selects, or the reserved placeholder of that name — `Ok(None)` when
/// neither names it, which is what leaves the template's text alone, and
/// `Err` for a path this service did not evaluate ([`selected`]).
fn filling(
    key: &str,
    paths: &::std::collections::BTreeMap<String, String>,
    event: &Value,
    delivery: &Delivery<'_>,
) -> Result<Option<Value>, String> {
    if let Some(path) = paths.get(key) {
        return selected(path, event).map(Some);
    }
    Ok(key
        .starts_with(RESERVED_KEY_PREFIX)
        .then(|| reserved(key, event, delivery))
        .flatten())
}

/// The key the placeholder at the head of `text` names, with the text after
/// it — `None` when `text` does not open one.
///
/// A placeholder is `<key>` with no `<`, `>` or newline inside it, so an
/// unterminated `<` and a `<` that merely occurs in prose are left alone.
fn placeholder(text: &str) -> Option<(&str, &str)> {
    let after = text.strip_prefix('<')?;
    let end = after.find('>')?;
    let key = &after[..end];
    if key.is_empty() || key.contains(['<', '\n', '\r', '"']) {
        return None;
    }
    Some((key, &after[end + 1..]))
}

/// The characters a selected value contributes to text: a string's own, and
/// any other value's compact JSON text — the value itself, never a form
/// special to one kind of value.
fn text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// [`text`] escaped for the JSON string literal it is written into, so a
/// selected value carrying a quote, a backslash or a control character
/// cannot break the template's own JSON.
fn escaped_text(value: &Value) -> String {
    let quoted = Value::String(text(value)).to_string();
    // `Value::String`'s rendering is the QUOTED form, and what the
    // placeholder stands for is its content: the quotes around it belong to
    // the template's own string, which already carries them.
    quoted
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or(&quoted)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::{render, selected, Delivery};

    use ::serde_json::json;

    /// The event every row below transforms.
    fn event() -> ::serde_json::Value {
        json!({
            "id": "e-1",
            "detail-type": "Order Created",
            "source": "com.demo",
            "time": "2026-01-01T00:00:00Z",
            "detail": {
                "order": "A-1",
                "amount": 7,
                "items": ["a", "b", "c"],
                "quoted": "he said \"hi\"",
            },
        })
    }

    fn delivery() -> Delivery<'static> {
        Delivery {
            rule: "r",
            rule_arn: "arn:aws:events:eu-west-2:000000000000:rule/r",
            ingestion_time: "2026-01-01T00:00:00Z",
        }
    }

    /// What a path selects, over a TABLE whose rows differ along the KIND of
    /// place the path names: a member, an element, a slice, a place the
    /// event does not carry, and a path this service does not evaluate at
    /// all — which is NOT a path that selected nothing.
    #[test]
    fn a_path_selects_its_place_and_an_absent_one_selects_null() {
        // (case, path, the value it selects, `None` for a path this service
        // did not evaluate)
        let rows = [
            ("a member", "$.detail.order", Some(json!("A-1"))),
            ("a number", "$.detail.amount", Some(json!(7))),
            (
                "a hyphenated member",
                "$.detail-type",
                Some(json!("Order Created")),
            ),
            ("an element", "$.detail.items[1]", Some(json!("b"))),
            ("a slice", "$.detail.items[0:2]", Some(json!(["a", "b"]))),
            ("an open slice", "$.detail.items[1:]", Some(json!(["b", "c"]))),
            ("an absent member", "$.detail.nope", Some(json!(null))),
            (
                "an element past the array",
                "$.detail.items[9]",
                Some(json!(null)),
            ),
            // Each of these is a form the bounded reader declines, so the
            // selection is REFUSED rather than answered with the empty
            // value: nothing was evaluated, so nothing may be rendered.
            ("a wildcard member", "$.detail.*", None),
            ("a descendant", "$..order", None),
            ("a union", "$.detail.items[0,1]", None),
            ("a filter", "$.detail.items[?@=='a']", None),
            ("a negative index", "$.detail.items[-1]", None),
            ("a slice with a step", "$.detail.items[0:2:1]", None),
            ("text that is not a path", "detail.order", None),
        ];
        let event = event();
        for (case, path, expected) in rows {
            let actual = selected(path, &event);
            assert_eq!(
                actual.clone().ok(),
                expected,
                "case {case:?}: expected {path} to select {expected:?}, got {actual:?}"
            );
            if expected.is_none() {
                assert!(
                    actual.is_err_and(|refusal| refusal.contains(path)),
                    "case {case:?}: the refusal names the path that was not evaluated"
                );
            }
        }
    }

    /// How a placeholder renders, over a TABLE whose rows differ along where
    /// the placeholder stands (in a string or out), what the path selects,
    /// and whether the template is JSON at all.
    #[test]
    fn a_placeholder_renders_the_value_where_it_stands() {
        let paths: ::std::collections::BTreeMap<String, String> = [
            ("order", "$.detail.order"),
            ("amount", "$.detail.amount"),
            ("items", "$.detail.items[0:2]"),
            ("absent", "$.detail.nope"),
            ("quoted", "$.detail.quoted"),
        ]
        .into_iter()
        .map(|(key, path)| (key.to_string(), path.to_string()))
        .collect();
        let rows = [
            ("a bare value", "{\"n\": <amount>}", "{\"n\": 7}"),
            (
                "a value in a string",
                "{\"n\": \"<amount>\"}",
                "{\"n\": \"7\"}",
            ),
            (
                "a string in a string",
                "{\"o\": \"<order>\"}",
                "{\"o\": \"A-1\"}",
            ),
            ("a bare string", "{\"o\": <order>}", "{\"o\": \"A-1\"}"),
            ("a slice", "{\"i\": <items>}", "{\"i\": [\"a\",\"b\"]}"),
            ("an absent path", "{\"a\": <absent>}", "{\"a\": null}"),
            (
                // The quotes the selected value carries are escaped for the
                // string the placeholder stands in, so the template stays
                // the JSON it was written as.
                "a value carrying quotes",
                "{\"q\": \"<quoted>\"}",
                "{\"q\": \"he said \\\"hi\\\"\"}",
            ),
            (
                "a quoted-string template keeps its own quoting",
                "\"<order>\"",
                "\"A-1\"",
            ),
            (
                // A selected list written inside the template's own string
                // is escaped for it, so the rendering is still one JSON
                // string rather than a string followed by an array.
                "a list inside the template's own string",
                "\"<items>\"",
                "\"[\\\"a\\\",\\\"b\\\"]\"",
            ),
            (
                "a sentence template",
                "<order> is for <amount>",
                "A-1 is for 7",
            ),
            (
                "a reserved placeholder",
                "{\"r\": \"<aws.events.rule-name>\"}",
                "{\"r\": \"r\"}",
            ),
            (
                // Nothing says what an unknown key stands for, so the
                // template's own text is delivered.
                "a key nothing names",
                "{\"u\": \"<nope>\"}",
                "{\"u\": \"<nope>\"}",
            ),
        ];
        let event = event();
        for (case, template, expected) in rows {
            let actual = render(template, &paths, &event, &delivery());
            assert_eq!(
                actual.as_deref(),
                Ok(expected),
                "case {case:?}: expected {template:?} to render {expected:?}, got {actual:?}"
            );
        }
    }

    /// A rendering past [`super::MAX_RENDERED_BYTES`] is refused rather than
    /// built: a template may repeat one placeholder as often as its own
    /// length allows, so the product of the template's length and the size
    /// of what a placeholder carries is what the bound holds.
    #[test]
    fn a_rendering_past_the_bound_is_refused() {
        let large = "x".repeat(super::MAX_RENDERED_BYTES / 2 + 1);
        let event = json!({ "detail": { "large": large } });
        let paths: ::std::collections::BTreeMap<String, String> =
            [("large".to_string(), "$.detail.large".to_string())]
                .into_iter()
                .collect();
        // (case, the template, whether it renders at all)
        let rows = [
            ("one fill, inside the bound", "{\"a\": \"<large>\"}", true),
            ("two fills, past it", "{\"a\": \"<large><large>\"}", false),
        ];
        for (case, template, renders) in rows {
            let actual = render(template, &paths, &event, &delivery());
            assert_eq!(
                actual.is_ok(),
                renders,
                "case {case:?}: expected a rendering={renders} for a template filling \
                 {} bytes at a time, got {:?}",
                large.len(),
                actual.map(|rendered| rendered.len())
            );
        }
    }
}
