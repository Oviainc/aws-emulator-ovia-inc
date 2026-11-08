// SPDX-License-Identifier: CC0-1.0

//! The grammar of an `IntegrationResponseKey`: what a writer of an integration response
//! may store it as, how a reader matches a stored one, and why.
//!
//! A module of its own under [`crate::state`], because this is the rule TWO things depend
//! on and neither owns: the writers of an integration response, which refuse a key the
//! service could never select by, and the WebSocket data plane
//! (`crate::service::websocket::dispatch`), which selects a stored response by matching
//! that key against the status the call produced. A key the data plane can never match is
//! one the control plane must not accept, so both ask the same reading of a key here.
//!
//! `IntegrationResponseKey` targets `SelectionKey`, whose documentation is "After
//! evaluating a selection expression, the result is compared against one or more selection
//! keys to find a matching key" — so a key is one of exactly two things:
//!
//! * [`DEFAULT_SELECTION_KEY`], the catch-all AWS documents for every selection key type,
//!   which matches whatever no other key does; or
//! * a regular expression written between slashes ([`PATTERN_DELIMITER`]) — `/2\d\d/` is
//!   the form AWS documents for an integration response key — which matches the evaluated
//!   selection expression.
//!
//! The selection expression an integration response is chosen by is the call's STATUS for
//! every integration whose backend answers with one — `$integration.response.statuscode` is
//! the integration response selection expression AWS documents there — where a LAMBDA
//! integration is selected on the error message its function returned instead. So a
//! non-Lambda integration's key has to be able to match a status code at all: a pattern
//! that matches none of them selects nothing a backend can ever produce.

use super::target_names_a_function;

/// The selection key that matches whatever no other key of the same parent matches: "the
/// `$default` route ... acts as a catch-all", and the same `$default` key AWS documents for
/// every selection key type. One of the two forms an `IntegrationResponseKey` takes.
pub const DEFAULT_SELECTION_KEY: &str = "$default";

/// The character a patterned selection key opens and closes with: a key written as a
/// regular expression is wrapped in it (`/2\d\d/`), which is what distinguishes a PATTERN
/// from the literal [`DEFAULT_SELECTION_KEY`].
pub const PATTERN_DELIMITER: char = '/';

/// How many decimal digits a status code is written with, and the classes a response can
/// carry: "The status code of a response is a three-digit integer code" whose first digit
/// "defines the class of response", 1xx informational through 5xx server error (RFC 9110
/// §15) — the same shape API Gateway states in its own refusal
/// ([`STATUS_CODE_PATTERN_FORM`]). Those are the SUBJECTS a non-Lambda integration's key is
/// matched against at invoke time, so every one of them is tried against the key under the
/// same anchoring and the same budgets the data plane selects with ([`compiled`]): this
/// check cannot disagree with it about which subjects a key selects.
const LOWEST_STATUS_CODE: u16 = 100;
const HIGHEST_STATUS_CODE: u16 = 599;

/// The shape API Gateway names a status-code selection key by in its refusal: a class digit
/// followed by two more digits. Quoted in [`not_status_shaped_message`], and the shape
/// [`LOWEST_STATUS_CODE`]..=[`HIGHEST_STATUS_CODE`] enumerates.
const STATUS_CODE_PATTERN_FORM: &str = r"[1-5]([0-9]|\d){2}";

/// The compiled-program budget one caller-STORED `IntegrationResponseKey` pattern may take,
/// so a key inside its stored length still cannot compile into an arbitrarily large
/// automaton. Stated here rather than left to the engine's default, as the other data planes
/// that evaluate a caller-stored pattern state theirs.
const COMPILED_PATTERN_LIMIT: usize = 64 * 1024;

/// See [`COMPILED_PATTERN_LIMIT`]: the budget for the lazy DFA a match runs through.
const PATTERN_DFA_LIMIT: usize = 64 * 1024;

/// The pattern a key carries, or `None` for a key that is not written as one.
///
/// A patterned key is the text between one leading and one trailing [`PATTERN_DELIMITER`],
/// and the text has to be non-empty: `//` wraps no pattern, so it is not one of the two
/// forms a key takes. The ONE reading of that wrapping, so the validator below and the data
/// plane agree on where a key's pattern starts and ends.
pub fn key_pattern(key: &str) -> Option<&str> {
    key.strip_prefix(PATTERN_DELIMITER)
        .and_then(|rest| rest.strip_suffix(PATTERN_DELIMITER))
        .filter(|pattern| !pattern.is_empty())
}

/// The anchored expression a patterned key matches by, compiled under the budgets above, or
/// `None` for a pattern the engine will not build under them.
///
/// A key selects a subject only when it matches the WHOLE of it, so `/2\d\d/` selects `200`
/// and not `1200` — the anchoring is part of the reading, not of the caller's pattern. One
/// compile per key, so a caller's pattern is built once however many subjects it is then
/// tried against.
fn compiled(pattern: &str) -> Option<::regex::Regex> {
    ::regex::RegexBuilder::new(&format!("^(?:{pattern})$"))
        .size_limit(COMPILED_PATTERN_LIMIT)
        .dfa_size_limit(PATTERN_DFA_LIMIT)
        .build()
        .ok()
}

/// Whether an `IntegrationResponseKey` selects `status`.
///
/// A key written between slashes is a regular expression — the form AWS documents for a
/// WebSocket api's integration response keys (`/2\d\d/`) — and it selects the call when it
/// matches the whole status. A key written without slashes names the status literally, which
/// is how a key spelled `200` selects one. The expression is compiled by the maintained
/// engine under the budgets above, whose matching is linear in the subject, so a stored key
/// cannot make a call expensive; a key those budgets refuse selects nothing.
pub fn key_matches_status(key: &str, status: &str) -> bool {
    let Some(pattern) = key_pattern(key) else {
        return key == status;
    };
    compiled(pattern).is_some_and(|expression| expression.is_match(status))
}

/// The refusal of a key that is neither [`DEFAULT_SELECTION_KEY`] nor a wrapped pattern.
///
/// Names the member, the two forms it admits and the value the request sent, so a caller
/// reads back which of its values was wrong.
fn malformed_key_message(key: &str) -> String {
    format!(
        "IntegrationResponseKey must be a regular expression pattern of format \
         '/<pattern>/' or have a value of {DEFAULT_SELECTION_KEY}. Current value is {key}"
    )
}

/// The refusal of a wrapped pattern that can match no HTTP status code, on an integration
/// whose responses are selected BY the status.
///
/// Names the member, that it is the integration's kind that requires a status code, the
/// shape a status code has ([`STATUS_CODE_PATTERN_FORM`]) and the value the request sent.
fn not_status_shaped_message(key: &str) -> String {
    format!(
        "IntegrationResponseKey for non-Lambda Integration must be a valid status code of \
         format '{STATUS_CODE_PATTERN_FORM}'. Current value is {key}"
    )
}

/// Whether this integration's responses are selected on a Lambda function's ERROR MESSAGE
/// rather than on a status code: an integration whose `IntegrationUri` names a Lambda
/// function.
///
/// The URI is read through [`super::target_names_a_function`], the one reading of a Lambda
/// function ARN this crate has, so "is this a Lambda integration" is decided the same way
/// wherever it is asked. `MOCK`, `HTTP`, `HTTP_PROXY` and an `AWS`/`AWS_PROXY` integration
/// addressing another service's action all answer `false`: each of those answers with a
/// status.
pub fn selects_on_a_lambda_error(integration_uri: Option<&str>) -> bool {
    integration_uri.is_some_and(target_names_a_function)
}

/// `Ok(())`, or why this `IntegrationResponseKey` cannot be stored — the message for the
/// operation's `BadRequestException`.
///
/// Two independent stages, in order:
///
/// 1. the key is one of the two forms a selection key takes — [`DEFAULT_SELECTION_KEY`] or
///    a wrapped pattern ([`key_pattern`]) — whatever the integration is;
/// 2. on an integration whose responses are selected by the STATUS of the call (every one
///    that is not a Lambda integration, see [`selects_on_a_lambda_error`]), the pattern can
///    match a status code at all — decided by trying it against every code in
///    [`LOWEST_STATUS_CODE`]..=[`HIGHEST_STATUS_CODE`] under ONE compile of the caller's
///    pattern; the catch-all key selects without matching anything, so it is not held to
///    this.
///
/// Shared by every writer of an integration response, so a create and a patch cannot hold
/// one to a different rule.
pub fn validate_integration_response_key(
    key: &str,
    integration_uri: Option<&str>,
) -> Result<(), String> {
    if key == DEFAULT_SELECTION_KEY {
        return Ok(());
    }
    let Some(pattern) = key_pattern(key) else {
        return Err(malformed_key_message(key));
    };
    if selects_on_a_lambda_error(integration_uri) {
        return Ok(());
    }
    let selects_a_status = compiled(pattern).is_some_and(|expression| {
        (LOWEST_STATUS_CODE..=HIGHEST_STATUS_CODE)
            .any(|status| expression.is_match(&status.to_string()))
    });
    if !selects_a_status {
        return Err(not_status_shaped_message(key));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Lambda function URI, the form an integration whose responses are selected on an
    /// error message carries.
    const FUNCTION_URI: &str = "arn:aws:lambda:us-west-2:000000000000:function:greeter";

    /// RULE: a key is `$default` or a slash-wrapped pattern, whatever the integration is.
    #[test]
    fn a_key_that_is_neither_the_catch_all_nor_a_wrapped_pattern_is_refused() {
        // The rows span the forms a value can arrive in: the catch-all, wrapped patterns,
        // and the ways a value fails to be either — bare text, a half-wrapped value, an
        // empty wrapping, the catch-all misspelled, and emptiness.
        let rows: &[(&str, bool)] = &[
            ("$default", true),
            (r"/2\d\d/", true),
            ("/200/", true),
            ("badkey", false),
            ("200", false),
            ("/200", false),
            ("200/", false),
            ("//", false),
            ("default", false),
            ("$DEFAULT", false),
            ("", false),
        ];
        for (key, well_formed) in rows {
            // Judged on a LAMBDA integration, where only this first stage applies.
            let verdict = validate_integration_response_key(key, Some(FUNCTION_URI));
            assert_eq!(
                verdict.is_ok(),
                *well_formed,
                "case {key:?}: expected well_formed={well_formed}, got {verdict:?}"
            );
            if !*well_formed {
                let message = verdict.expect_err("refused above");
                assert!(
                    message.contains(key) && message.contains("IntegrationResponseKey"),
                    "case {key:?}: expected the refusal to name the member and the value, \
                     got {message:?}"
                );
            }
        }
    }

    /// RULE: on an integration selected by the status of the call, a wrapped pattern has to
    /// be able to match an HTTP status code; on a Lambda integration, selected by the
    /// function's error message, any pattern is a key.
    #[test]
    fn a_pattern_matching_no_status_code_is_refused_only_off_a_lambda_integration() {
        // (the key, whether it can match a status code). The rows span the pattern forms a
        // status key takes — a class with digit escapes, a quantified escape, a literal
        // code, a character class, wildcards, an alternation — and patterns that match no
        // code at all.
        let rows: &[(&str, bool)] = &[
            (r"/2\d\d/", true),
            (r"/4\d{2}/", true),
            ("/200/", true),
            (r"/[45]\d\d/", true),
            ("/5../", true),
            (r"/(200|404)/", true),
            ("/notastatus/", false),
            ("/error.*/", false),
            // Three digits, but no response carries a class outside 1xx..5xx, and no
            // status code has six digits or two.
            (r"/0\d\d/", false),
            (r"/7\d\d/", false),
            (r"/\d{6}/", false),
            ("/42/", false),
            // A pattern no engine builds matches nothing, so it matches no code.
            ("/(/", false),
        ];
        for (key, status_shaped) in rows {
            let off_lambda = validate_integration_response_key(key, Some("http://backend"));
            assert_eq!(
                off_lambda.is_ok(),
                *status_shaped,
                "case {key:?}: expected a non-Lambda integration to accept it \
                 ={status_shaped}, got {off_lambda:?}"
            );
            if !*status_shaped {
                let message = off_lambda.expect_err("refused above");
                assert!(
                    message.contains("non-Lambda Integration") && message.contains(key),
                    "case {key:?}: expected the refusal to name the integration kind and \
                     the value, got {message:?}"
                );
            }
            let on_lambda = validate_integration_response_key(key, Some(FUNCTION_URI));
            assert!(
                on_lambda.is_ok(),
                "case {key:?}: expected a Lambda integration to take any wrapped pattern, \
                 got {on_lambda:?}"
            );
        }
        // The catch-all selects without matching anything, so the second stage never
        // applies to it, on either kind of integration.
        for uri in [Some(FUNCTION_URI), Some("http://backend"), None] {
            let verdict = validate_integration_response_key(DEFAULT_SELECTION_KEY, uri);
            assert!(
                verdict.is_ok(),
                "case {uri:?}: expected the catch-all key to be accepted, got {verdict:?}"
            );
        }
    }

    /// RULE: each stage states its own refusal, in the wording API Gateway answers it with:
    /// the member, the forms it admits, and the value the request sent.
    #[test]
    fn each_stage_states_its_own_refusal() {
        // (case, the key, the sentence it is refused with)
        let rows: &[(&str, &str, String)] = &[
            (
                "a value that is neither form",
                "badkey",
                "IntegrationResponseKey must be a regular expression pattern of format \
                 '/<pattern>/' or have a value of $default. Current value is badkey"
                    .to_string(),
            ),
            (
                "a wrapped pattern that is no status code",
                "/error.*/",
                format!(
                    "IntegrationResponseKey for non-Lambda Integration must be a valid \
                     status code of format '{STATUS_CODE_PATTERN_FORM}'. Current value is \
                     /error.*/"
                ),
            ),
        ];
        for (case, key, expected) in rows {
            let refused = validate_integration_response_key(key, Some("http://backend"))
                .expect_err("the key is refused");
            assert_eq!(
                refused, *expected,
                "case {case:?}: expected API Gateway's wording, got {refused:?}"
            );
        }
    }

    /// RULE: whether an integration is a Lambda one is read off its `IntegrationUri`.
    #[test]
    fn a_lambda_integration_is_the_one_whose_uri_names_a_function() {
        // (case, the integration's URI, whether its responses are selected on an error)
        let rows: &[(&str, Option<&str>, bool)] = &[
            ("a function ARN", Some(FUNCTION_URI), true),
            (
                "a qualified function ARN",
                Some("arn:aws:lambda:us-west-2:000000000000:function:greeter:3"),
                true,
            ),
            (
                "an HTTP upstream",
                Some("https://backend.example.com"),
                false,
            ),
            (
                "another service's action",
                Some("arn:aws:apigateway:us-west-2:sqs:path/000000000000/orders"),
                false,
            ),
            ("no URI at all, as a MOCK integration carries", None, false),
        ];
        for (case, uri, selects_on_error) in rows {
            let reached = selects_on_a_lambda_error(*uri);
            assert_eq!(
                reached, *selects_on_error,
                "case {case:?}: expected {selects_on_error}, got {reached}"
            );
        }
    }
}
