// SPDX-License-Identifier: CC0-1.0

//! What an AWS SDK CLIENT fills into a call's input before it sends it: the members the called
//! service's own model marks as an idempotency token.
//!
//! A client generates a value for such a member when the caller left it out — "If you use the
//! Amazon Web Services CLI or one of the Amazon Web Services SDKs to call this operation, then
//! you can leave this parameter empty. The CLI or SDK generates a random UUID for you and
//! includes it as the value for this parameter in the request"
//! (`PutSecretValueRequest$ClientRequestToken`, the Secrets Manager model, which says the same
//! on every member it marks) — so the service never sees one absent from a call an SDK made,
//! and a handler that omits it is served exactly as it is on a managed runtime, where the SDK
//! on its import path fills one in.
//!
//! The fill is the CLIENT's half of a call, which is why it lives on the host side of the one
//! call seam ([`super::aws_api`]) rather than inside any one of the SDKs this runtime supplies:
//! every language's client does it, so it is done once for all of them, before the request is
//! built and signed.
//!
//! WHICH members those are is read from the called service's own Smithy model — the same file
//! that service's emulator crate is generated from, through the one reading of it this runtime
//! holds (`super::sdk_model`) — so no member name is stated here and none can drift from the
//! surface the service serves. A value is a fresh version-4 UUID, which is the "random UUID"
//! the SDKs generate and what the member's own documentation recommends; a member the caller
//! DID state is never replaced, because the whole point of the token is that the caller chooses
//! it and retries with it.

use ::serde_json::Value;

/// `body` — one call's input document — with a generated value for every idempotency-token
/// member of `operation` the document does not state, or `None` when there is nothing to fill:
/// this emulator holds no model for the service, the operation has no such member, the document
/// already states each of them, or it is not an input document at all.
///
/// `None` means the document travels exactly as the guest wrote it, so a call to a service whose
/// model this emulator does not hold is neither changed nor refused here.
#[must_use]
pub fn filled(service: &str, operation: &str, body: &str) -> Option<String> {
    let held = super::sdk_model::client_exports(service);
    let members = held.idempotency_tokens.get(operation)?;
    let Ok(Value::Object(mut document)) = ::serde_json::from_str::<Value>(body) else {
        return None;
    };
    let mut generated = false;
    for member in members {
        // A member the caller stated is the caller's, whatever it says; one it left out, and
        // one it wrote as JSON null — which is how a client that passes nothing writes an
        // absent member — is the one the SDK generates.
        if document.get(member).is_some_and(|value| !value.is_null()) {
            continue;
        }
        document.insert(member.clone(), Value::String(token()));
        generated = true;
    }
    generated.then(|| Value::Object(document).to_string())
}

/// One generated token: the random UUID the SDKs generate for such a member.
fn token() -> String {
    ::uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the members filled are the ones the called service's own model marks as an
    /// idempotency token, and nothing else is touched.
    ///
    /// The rows span what the rule generalises over: an operation whose model marks such a
    /// member and one whose model marks none, a service whose model this emulator holds and one
    /// it does not, and a name that is no operation of the service at all. The member named in
    /// a row is read back out of the answer rather than stated by the fill, so a row states
    /// only WHETHER one is expected.
    #[test]
    fn the_members_filled_are_the_ones_the_model_marks() {
        // (case, service token, operation, the member expected to be filled)
        let rows: &[(&str, &str, &str, Option<&str>)] = &[
            (
                "a write that stakes a secret version",
                "secretsmanager",
                "PutSecretValue",
                Some("ClientRequestToken"),
            ),
            (
                "a create that stakes the first one",
                "secretsmanager",
                "CreateSecret",
                Some("ClientRequestToken"),
            ),
            (
                "the same service under its hyphenated package token",
                "secrets-manager",
                "RotateSecret",
                Some("ClientRequestToken"),
            ),
            (
                "an operation whose model marks no such member",
                "secretsmanager",
                "DescribeSecret",
                None,
            ),
            ("another service's write", "sqs", "SendMessage", None),
            (
                "a service this emulator holds no model for",
                "opensearch",
                "CreateDomain",
                None,
            ),
            (
                "no operation of the service",
                "secretsmanager",
                "Nonesuch",
                None,
            ),
        ];
        for (case, service, operation, member) in rows {
            let answered = filled(service, operation, "{\"SecretId\":\"held\"}");
            let Some(expected) = member else {
                assert!(
                    answered.is_none(),
                    "case {case:?}: expected the document to travel as written, got {answered:?}"
                );
                continue;
            };
            let answered =
                answered.unwrap_or_else(|| panic!("case {case:?}: expected a filled document"));
            let document: Value = ::serde_json::from_str(&answered)
                .unwrap_or_else(|_| panic!("case {case:?}: expected a document, got {answered}"));
            let token = document
                .get(expected)
                .and_then(Value::as_str)
                .unwrap_or_default();
            // A UUID the member's own length constraint admits (`ClientRequestTokenType` is
            // 32..64 characters), and the request's own members are untouched.
            assert!(
                (32..=64).contains(&token.len()) && token.contains('-'),
                "case {case:?}: expected a generated UUID for {expected:?}, got {token:?}"
            );
            assert_eq!(
                document.get("SecretId").and_then(Value::as_str),
                Some("held"),
                "case {case:?}: expected the guest's own members untouched, got {answered}"
            );
        }
    }

    /// RULE: a member the caller stated is never replaced, and each call that leaves it out
    /// gets a value of its own.
    ///
    /// The rows differ along the FORM the document states the member in: stated, absent, and
    /// written as JSON null, which is how a client that passes nothing writes it.
    #[test]
    fn a_stated_token_is_kept_and_an_absent_one_is_generated_per_call() {
        let stated = "a-token-the-caller-generated-itself";
        // (case, the document, the value the member must keep, when it keeps one)
        let rows: &[(&str, String, Option<&str>)] = &[
            (
                "stated by the caller",
                ::serde_json::json!({"SecretId": "held", "ClientRequestToken": stated})
                    .to_string(),
                Some(stated),
            ),
            (
                "left out",
                ::serde_json::json!({"SecretId": "held"}).to_string(),
                None,
            ),
            (
                "written as null",
                ::serde_json::json!({"SecretId": "held", "ClientRequestToken": Value::Null})
                    .to_string(),
                None,
            ),
        ];
        for (case, document, expected) in rows {
            let answered = filled("secretsmanager", "PutSecretValue", document);
            match expected {
                // Nothing to fill, so the document travels exactly as written.
                Some(kept) => assert!(
                    answered.is_none(),
                    "case {case:?}: expected the document to travel as written with {kept:?}, \
                     got {answered:?}"
                ),
                None => {
                    let first = answered
                        .unwrap_or_else(|| panic!("case {case:?}: expected a filled document"));
                    let again = filled("secretsmanager", "PutSecretValue", document)
                        .unwrap_or_else(|| panic!("case {case:?}: expected a filled document"));
                    assert_ne!(
                        first, again,
                        "case {case:?}: expected each call to generate its own token, got \
                         {first} twice"
                    );
                }
            }
        }
    }

    /// A body that is not an input document at all is left alone: nothing is built from it.
    #[test]
    fn a_body_that_is_no_document_is_left_as_written() {
        for case in ["", "[]", "\"text\"", "{"] {
            let answered = filled("secretsmanager", "PutSecretValue", case);
            assert!(
                answered.is_none(),
                "case {case:?}: expected the body to travel as written, got {answered:?}"
            );
        }
    }
}
