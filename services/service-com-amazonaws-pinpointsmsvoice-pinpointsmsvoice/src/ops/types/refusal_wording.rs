// SPDX-License-Identifier: CC0-1.0

//! How this service words a request it refuses for a parameter it cannot use.
//!
//! Every operation in this model declares ONE client-input error —
//! `BadRequestException` (HTTP 400, doc "The input you provided is invalid.";
//! `smithy_query(query="shape",shape="BadRequestException",model_path="services/service-com-amazonaws-pinpointsmsvoice-pinpointsmsvoice/smithy-model/com-amazonaws-pinpointsmsvoice-pinpointsmsvoice.json")`)
//! — so the status and the code are the same for every such refusal and only the sentence
//! differs. This module is the single home of the two sentences, so two operations cannot
//! word one class of refusal differently.
//!
//! The two classes are distinguished by WHERE the parameter failed:
//!
//! * [`missing_request_parameters`] — a `@required` parameter the request binds to the URI
//!   arrived with no value at all, so the request never named a target. The request-binding
//!   layer reports it, which names a parameter by its MODELLED member name.
//! * [`invalid_parameters`] — a parameter reached this service's own validation carrying a
//!   value it cannot use: absent where this service needs one, blank, or malformed. That
//!   layer names a parameter by the service's own spelling, which is the modelled member
//!   name with its first character lowercased (see [`parameter_name`]).
//!
//! [`unusable_required_label`] is which of the two a URI-bound parameter gets, so the layer
//! split is decided in one place rather than per handler.
//!
//! Both sentences take the parameter name(s) FROM THE REQUEST: nothing here is keyed to one
//! operation or one recorded value.

/// The refusal for `members`: `@required` request parameters that arrived with no value.
///
/// Each name is the parameter's modelled member name, as this layer reports it, and they
/// are listed in the order given — the order the operation's input declares them, so a
/// request missing two parameters reads the same way every time.
pub fn missing_request_parameters(members: &[&str]) -> String {
    format!("Missing required request parameters: [{}]", members.join(", "))
}

/// The refusal for `members`: parameters this service's validation cannot use, whether
/// they arrived null, empty, or holding a value it cannot read.
///
/// `members` are MODELLED member names; each is rendered through [`parameter_name`] and
/// single-quoted, in the order given.
pub fn invalid_parameters(members: &[&str]) -> String {
    let named: Vec<String> = members
        .iter()
        .map(|member| format!("'{}'", parameter_name(member)))
        .collect();
    format!(
        "The following parameter is null, empty or invalid: [{}]",
        named.join(", ")
    )
}

/// The refusal `member` gets when the value a `@required` URI-bound parameter arrived with
/// is one this service cannot use, or `None` when the value is usable.
///
/// This is the LAYER SPLIT the two sentences above belong to, stated once:
///
/// * an EMPTY value bound nothing at all, so the request never named a target and the
///   request-binding layer reports it ([`missing_request_parameters`]);
/// * a value that is present but BLANK did bind, so it reached this service's own
///   validation, which cannot use it ([`invalid_parameters`]).
///
/// Only the layer differs — both are the modelled `BadRequestException` at HTTP 400, and
/// both are answered before any state is read, so neither can be mistaken for the 404 a
/// name that merely does not exist gets.
pub fn unusable_required_label(member: &str, value: &str) -> Option<String> {
    if value.is_empty() {
        return Some(missing_request_parameters(&[member]));
    }
    if value.trim().is_empty() {
        return Some(invalid_parameters(&[member]));
    }
    None
}

/// A modelled member name as this service's own validation spells a parameter: the member
/// name with its FIRST character lowercased and the rest left exactly as modelled
/// (`NextToken` -> `nextToken`, `PageSize` -> `pageSize`).
///
/// The models in this namespace declare an upper-camel member name and bind it to the wire
/// under that same name, while the service's validation layer reports the lower-camel form
/// of it — so the spelling is derived from the member rather than written out per call
/// site.
pub fn parameter_name(member: &str) -> String {
    let mut characters = member.chars();
    match characters.next() {
        Some(first) => first.to_lowercase().chain(characters).collect(),
        None => String::new(),
    }
}
