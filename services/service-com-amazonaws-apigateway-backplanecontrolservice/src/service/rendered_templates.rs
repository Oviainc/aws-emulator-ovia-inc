// SPDX-License-Identifier: CC0-1.0

//! How an `IntegrationResponse`'s mapping templates are RENDERED on the wire: a content
//! type whose template body is empty carries no template, and API Gateway reports that
//! entry's value as null rather than as an empty string.
//!
//! `IntegrationResponse$responseTemplates` is a `MapOfStringToString`
//! (`smithy_query(query="shape",shape="IntegrationResponse")`), so the entry's value has
//! the string type in every rendering the schema drives — the one value a stored empty
//! template cannot be written as is the absent one. This module is where that rendering is
//! completed, over the response the modelled walk already serialized: the body is re-read
//! as JSON and every empty template value in it is replaced by null.
//!
//! The walk steps only through the MODELLED members an `IntegrationResponse` travels in —
//! `item` (a page of `Resource`s), `Resource$resourceMethods`, `Method$methodIntegration`,
//! `Integration$integrationResponses` — from the shape the body is rooted at, so a body
//! that is not one of those renderings (an exported API definition, which carries its own
//! `responseTemplates` inside the definition it exports) is left byte for byte as it was.

use serde_json::Value;

/// The member that carries one mapping template per content type: "the templates used to
/// transform the integration response body … a key/value map, with a content-type as the
/// key and a template as the value".
const RESPONSE_TEMPLATES: &str = "responseTemplates";

/// `body` with every empty response template in it rendered as null, or `None` when the
/// body carries no such template and so is to be answered exactly as it was serialized.
///
/// The cheap byte test comes first, so a body that cannot be affected is never parsed.
pub fn with_empty_templates_as_null(body: &[u8]) -> Option<Vec<u8>> {
    if !carries_a_template_member(body) {
        return None;
    }
    let mut document: Value = serde_json::from_slice(body).ok()?;
    if !nulled_in_rendered_shape(&mut document) {
        return None;
    }
    serde_json::to_vec(&document).ok()
}

/// Whether `body` names the template member at all: an anchored substring test over the
/// serialized bytes, which is what keeps the parse off every other response. Anchored on
/// the member's first byte, so a large body that does not carry it (an exported definition,
/// a generated SDK) costs one comparison per byte.
fn carries_a_template_member(body: &[u8]) -> bool {
    let Some((&first, rest)) = RESPONSE_TEMPLATES.as_bytes().split_first() else {
        return true;
    };
    body.iter()
        .enumerate()
        .filter(|(_, byte)| **byte == first)
        .any(|(index, _)| body.get(index + 1..index + 1 + rest.len()) == Some(rest))
}

/// Null every empty template of every `IntegrationResponse` reachable from the shape
/// `root` is a rendering of, answering whether anything changed.
///
/// One arm per shape that embeds the next: the body of this service's reads is an
/// `IntegrationResponse`, an `Integration`, a `Method`, a `Resource` or a page of
/// `Resource`s, and each of those reaches the templates through its own modelled member.
fn nulled_in_rendered_shape(root: &mut Value) -> bool {
    let mut changed = nulled_in_integration_response(root);
    changed |= nulled_in_integration(root);
    changed |= nulled_in_method(root);
    changed |= nulled_in_resource(root);
    if let Some(page) = root.get_mut("item").and_then(Value::as_array_mut) {
        for resource in page.iter_mut() {
            changed |= nulled_in_resource(resource);
        }
    }
    changed
}

/// `Resource$resourceMethods`, "a key-value map of the methods of this resource", keyed by
/// HTTP verb.
fn nulled_in_resource(resource: &mut Value) -> bool {
    let Some(methods) = resource
        .get_mut("resourceMethods")
        .and_then(Value::as_object_mut)
    else {
        return false;
    };
    let mut changed = false;
    for method in methods.values_mut() {
        changed |= nulled_in_method(method);
    }
    changed
}

/// `Method$methodIntegration`, "the method's integration".
fn nulled_in_method(method: &mut Value) -> bool {
    method
        .get_mut("methodIntegration")
        .is_some_and(nulled_in_integration)
}

/// `Integration$integrationResponses`, "the integration's responses", keyed by status
/// code.
fn nulled_in_integration(integration: &mut Value) -> bool {
    let Some(responses) = integration
        .get_mut("integrationResponses")
        .and_then(Value::as_object_mut)
    else {
        return false;
    };
    let mut changed = false;
    for response in responses.values_mut() {
        changed |= nulled_in_integration_response(response);
    }
    changed
}

/// The rendering itself: a template whose body is empty is no template, so the content
/// type it is keyed by carries a null value.
fn nulled_in_integration_response(response: &mut Value) -> bool {
    let Some(templates) = response
        .get_mut(RESPONSE_TEMPLATES)
        .and_then(Value::as_object_mut)
    else {
        return false;
    };
    let mut changed = false;
    for template in templates.values_mut() {
        if template.as_str() == Some("") {
            *template = Value::Null;
            changed = true;
        }
    }
    changed
}
