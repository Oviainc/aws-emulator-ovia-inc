// SPDX-License-Identifier: CC0-1.0

//! Selection expressions: the `$`-variable expressions this service's members carry, and
//! what each member's protocol lets one select.
//!
//! A module of its own under [`crate::state`], because this is one concern several
//! members share — `Api$RouteSelectionExpression`, `Api$ApiKeySelectionExpression`, a
//! route response's and an integration response's selection expressions — and every
//! writer of one has to hold it to the same rule, or two operations would store
//! expressions the same reader cannot evaluate alike. [`crate::state`] re-exports every
//! public item here, so a handler reaches them the way it reaches the rest of this
//! service's state helpers.

use super::WEBSOCKET_PROTOCOL_TYPE;

/// The variable namespaces a selection expression may read, lowercased for comparison.
///
/// A selection expression selects a stored key by evaluating request DATA — the
/// documented defaults name exactly these namespaces (`$request.header.x-api-key` for
/// `Api$ApiKeySelectionExpression`, `$request.method $request.path` for an HTTP api's
/// `RouteSelectionExpression`, `$context.*` for context variables) — so a `$`-token
/// naming anything else names no variable at all.
pub const SELECTION_EXPRESSION_VARIABLES: &[&str] = &["request", "context", "stagevariables"];

/// The sentence this service refuses a selection expression that names a variable it
/// does not evaluate with: fixed text, then the variable reference exactly as the
/// request wrote it (`$` included).
///
/// One wording for every member that carries a selection expression, so the refusal a
/// caller reads does not depend on which member it set.
pub fn unexpected_selection_variable_message(reference: &str) -> String {
    format!("Unexpected variable in selection expression: {reference}")
}

/// Every VARIABLE REFERENCE `expression` makes, in the order it makes them: the text
/// after each unescaped `$`, with the braces of the `${…}` form removed.
///
/// In a selection expression an unescaped `$` begins a variable reference, and a
/// reference is a dotted path into one of the namespaces the service evaluates
/// (`$request.body.action`, `${request.method}`). A `\` escapes the next character, which
/// is how a selection KEY that itself begins with `$` is written (`\$default`), so an
/// escaped `$` begins no reference. The reference is returned AS WRITTEN — neither
/// lowercased nor trimmed — because it is what a refusal echoes back.
pub fn selection_expression_references(expression: &str) -> Vec<String> {
    let characters: Vec<char> = expression.chars().collect();
    let mut references = Vec::new();
    let mut index = 0;
    while index < characters.len() {
        match characters[index] {
            // An escaped character is a literal: this is how a selection key beginning
            // with `$` is written.
            '\\' => index += 2,
            '$' => {
                index += 1;
                let braced = characters.get(index) == Some(&'{');
                if braced {
                    index += 1;
                }
                let reference: String = characters[index..]
                    .iter()
                    .take_while(|c| {
                        if braced {
                            **c != '}'
                        } else {
                            c.is_ascii_alphanumeric() || **c == '_' || **c == '.' || **c == '-'
                        }
                    })
                    .collect();
                index += reference.chars().count() + usize::from(braced);
                references.push(reference);
            }
            _ => index += 1,
        }
    }
    references
}

/// `Ok(())`, or why this selection expression cannot be stored — the message for the
/// operation's `BadRequestException`.
///
/// One rule: every variable reference the expression makes
/// ([`selection_expression_references`]) has to name one of the variable namespaces the
/// service evaluates ([`SELECTION_EXPRESSION_VARIABLES`]). A selection KEY that itself
/// begins with `$` — `$default` is the one the service mints — is a literal, and a
/// literal is written escaped (`\$default`); unescaped it reads as a variable the service
/// does not define. The refusal names the namespace that was not recognised, which is the
/// part of the reference that is wrong.
pub fn validate_selection_expression(expression: &str) -> Result<(), String> {
    for reference in selection_expression_references(expression) {
        let namespace = reference.split('.').next().unwrap_or_default();
        if !SELECTION_EXPRESSION_VARIABLES.contains(&namespace.to_ascii_lowercase().as_str()) {
            return Err(unexpected_selection_variable_message(&format!(
                "${namespace}"
            )));
        }
    }
    Ok(())
}

/// The variable reference a WEBSOCKET api's `RouteSelectionExpression` selects a route
/// key with, lowercased for comparison: a FIELD of the request body.
///
/// A WebSocket api matches each inbound message against its routes by evaluating this
/// expression over that message — `$request.body.action` is the expression AWS documents
/// for a message carrying an `action` property — so the reference has to name a field
/// INSIDE the body. The whole body (`$request.body`, no field selector) selects no key
/// a route could carry, and neither does a part of the request a frame does not have.
pub const WEBSOCKET_ROUTE_SELECTION_REFERENCE_PREFIX: &str = "request.body.";

/// `Ok(())`, or why this `RouteSelectionExpression` cannot be stored on an api of this
/// `ProtocolType` — the message for the operation's `BadRequestException`.
///
/// Two rules, in order: the expression names only namespaces the service evaluates
/// ([`validate_selection_expression`]), and — on a WEBSOCKET api, whose route keys are
/// matched against the message body — every reference it makes names a field of that body
/// ([`WEBSOCKET_ROUTE_SELECTION_REFERENCE_PREFIX`]). A reference that names anything
/// else, the whole body included, selects no route key, so the api it was aimed at would
/// stop routing messages altogether; it is refused and nothing is written. The refusal
/// echoes the reference as the request wrote it.
///
/// An api of the other protocol is held to the namespace rule alone: "For HTTP APIs, the
/// routeSelectionExpression must be ${request.method} ${request.path}. If not provided,
/// this will be the default for HTTP APIs" (`CreateApi$RouteSelectionExpression`), which
/// is the value this service gives such an api when the request names none
/// (`crate::state::HTTP_ROUTE_SELECTION_EXPRESSION`).
///
/// Shared by every writer of an api's route selection expression, so the create and the
/// patch cannot hold one to a different rule; a patch weighs the expression the api would
/// CARRY.
pub fn validate_route_selection_expression(
    protocol_type: &str,
    expression: &str,
) -> Result<(), String> {
    validate_selection_expression(expression)?;
    if protocol_type != WEBSOCKET_PROTOCOL_TYPE {
        return Ok(());
    }
    for reference in selection_expression_references(expression) {
        let selects_a_body_field = reference
            .to_ascii_lowercase()
            .strip_prefix(WEBSOCKET_ROUTE_SELECTION_REFERENCE_PREFIX)
            .is_some_and(|field| !field.is_empty());
        if !selects_a_body_field {
            return Err(unexpected_selection_variable_message(&format!(
                "${reference}"
            )));
        }
    }
    Ok(())
}
