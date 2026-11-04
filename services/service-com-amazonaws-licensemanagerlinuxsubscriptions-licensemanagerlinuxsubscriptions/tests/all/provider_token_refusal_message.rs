// SPDX-License-Identifier: CC0-1.0

//! The WORDING of `RegisterSubscriptionProvider`'s refusals, which is the only thing a
//! client can tell them apart by: all three are the modelled `ValidationException` with
//! HTTP 400.
//!
//! The rules under test:
//!
//! 1. every refusal about the CREDENTIAL the registration names — whatever the service
//!    could not make of the secret: it could not be read at all, it holds no text value, or
//!    the value it holds is not an access token the named provider issues — carries ONE
//!    fixed sentence stating the provider token as invalid or expired, identical across
//!    those cases, with nothing interpolated into it (no secret name or ARN, no source, no
//!    diagnosis of the check the service ran);
//! 2. the REQUEST-SHAPE refusals of the same operation keep their own wording — the
//!    `SubscriptionProviderSource` enum domain and the `SecretArn` pattern each name the
//!    member at fault — so the three refusal classes stay separable on the message.
//!
//! The rows of rule 1 span what it generalises over: the kind of value the secret holds
//! (text, binary only, nothing at all because the secret does not exist), and, for a text
//! value, its form — an arbitrary string, whitespace, a value with the token's three
//! `.`-separated segments but segments that are not base64url, a token short of a segment,
//! and a well-formed token whose claim set declares another token type. Those are
//! independent reasons the credential is unusable, and the point of the rule is that the
//! client is told the same thing about each.

use serde_json::{json, Value};

use crate::harness::{
    base64url, create_secret, harness_with_secrets_manager, jwt, register, stored_provider_count,
    token_header, ACCOUNT, RED_HAT, REGION, SIGNATURE_SEGMENT, TOKEN_REFUSAL_MESSAGE,
};

/// One row: what the registration's secret holds — or that it holds nothing because the
/// `SecretArn` names no secret.
struct Row {
    /// How the row reads in an assertion message.
    label: &'static str,
    /// The `CreateSecret` members storing the value, or `None` for a row whose `SecretArn`
    /// is to name no secret at all.
    stored: Option<Value>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "an arbitrary string",
            stored: Some(json!({ "SecretString": "offline-token-placeholder-value" })),
        },
        Row {
            label: "whitespace only",
            stored: Some(json!({ "SecretString": "   " })),
        },
        Row {
            label: "three segments that are not base64url",
            stored: Some(json!({ "SecretString": "@@@@.@@@@.@@@@" })),
        },
        Row {
            label: "a token short of its signature segment",
            stored: Some(json!({
                "SecretString": format!(
                    "{}.{}",
                    base64url(&serde_json::to_vec(&token_header()).expect("JSON serializes"), false),
                    base64url(br#"{"typ":"Offline"}"#, false),
                ),
            })),
        },
        Row {
            label: "a well-formed token declaring another token type",
            stored: Some(json!({
                "SecretString": jwt(
                    &token_header(),
                    &json!({ "typ": "Bearer", "iss": "https://sso.example-provider.com" }),
                    false,
                ),
            })),
        },
        Row {
            label: "a token whose payload is not JSON at all",
            stored: Some(json!({
                "SecretString": format!(
                    "{}.{}.{SIGNATURE_SEGMENT}",
                    base64url(&serde_json::to_vec(&token_header()).expect("JSON serializes"), false),
                    base64url(b"not json at all", false),
                ),
            })),
        },
        Row {
            label: "a secret holding a binary value only",
            stored: Some(json!({ "SecretBinary": base64url(b"an offline token", true) })),
        },
        Row {
            label: "a SecretArn naming no secret at all",
            stored: None,
        },
    ]
}

/// Rule 1: the credential refusal is one fixed sentence, whatever made the credential
/// unusable, and it interpolates nothing from the request or from the secret.
#[tokio::test]
async fn every_unusable_credential_is_refused_with_the_one_fixed_sentence() {
    let mut answered: Vec<(&str, String)> = Vec::new();
    for (index, row) in rows().into_iter().enumerate() {
        // An emulator per row, so each row's account starts with no registration and the
        // refusal under test is the only outcome it can have.
        let h = harness_with_secrets_manager();
        let label = row.label;
        let secret_name = format!("unusable-token-{index:02}");
        let secret_arn = match &row.stored {
            Some(members) => create_secret(&h, &secret_name, members).await,
            // A secret ARN of the modelled form naming nothing Secrets Manager holds.
            None => format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:{secret_name}"),
        };

        let reply = register(
            &h,
            &json!({ "SubscriptionProviderSource": RED_HAT, "SecretArn": secret_arn }),
        )
        .await;

        reply.error(label, 400, "ValidationException");
        assert_eq!(
            reply.message(),
            TOKEN_REFUSAL_MESSAGE,
            "case {label:?}: expected {TOKEN_REFUSAL_MESSAGE:?}, got {:?}",
            reply.message()
        );
        // Nothing of the request, of the secret or of the check the service ran may be
        // interpolated into a sentence that is entirely fixed text.
        for absent in [
            secret_arn.as_str(),
            secret_name.as_str(),
            RED_HAT,
            "SecretArn",
            "base64url",
            "JSON Web Token",
        ] {
            assert!(
                !reply.message().contains(absent),
                "case {label:?}: the refusal must not carry {absent:?}, got {:?}",
                reply.message()
            );
        }
        assert_eq!(
            stored_provider_count(&h),
            0,
            "case {label:?}: a refused registration must create nothing"
        );
        answered.push((label, reply.message().to_string()));
    }

    // The sentence is the SAME one for every reason the credential was unusable: a client
    // branching on it sees one credential class rather than one message per reason.
    assert!(
        answered
            .iter()
            .all(|(_, message)| message == TOKEN_REFUSAL_MESSAGE),
        "every unusable credential must answer the same sentence, got {answered:?}"
    );
}

/// Rule 2: the request-shape refusals of this operation keep their own wording, so the
/// three classes a client has to tell apart are pairwise distinct.
#[tokio::test]
async fn the_request_shape_refusals_keep_their_own_wording() {
    let h = harness_with_secrets_manager();
    // A real secret holding a value that is not an access token: were the shape of the
    // request accepted, this is the call that would reach the credential refusal — so each
    // row below differs from that one only in the member it violates.
    let secret_arn = create_secret(
        &h,
        "shape-refusal-token",
        &json!({ "SecretString": "offline-token-placeholder-value" }),
    )
    .await;

    // (case, the request, the member whose requirement it violates)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a source outside the modelled enum",
            json!({ "SubscriptionProviderSource": "Ubuntu", "SecretArn": secret_arn }),
            "subscriptionProviderSource",
        ),
        (
            "a SecretArn outside the modelled pattern",
            json!({
                "SubscriptionProviderSource": RED_HAT,
                "SecretArn": format!("arn:aws:secretsmanager:{REGION}:{ACCOUNT}:secret:team/rhel-token"),
            }),
            "secretArn",
        ),
    ];

    let mut messages: Vec<(&str, String)> = Vec::new();
    for (case, body, member) in rows {
        let reply = register(&h, &body).await;
        reply.error(case, 400, "ValidationException");
        let message = reply.message().to_string();
        assert!(
            message.contains(member),
            "case {case:?}: a request-shape refusal must name the member at fault \
             ({member:?}), got {message:?}"
        );
        assert_ne!(
            message, TOKEN_REFUSAL_MESSAGE,
            "case {case:?}: a request-shape refusal must not be worded as the credential \
             one, got {message:?}"
        );
        messages.push((case, message));
    }

    // And the two shape refusals are not each other either: three classes, three messages.
    assert_ne!(
        messages[0].1, messages[1].1,
        "the enum-domain and pattern refusals must stay distinct, got {messages:?}"
    );
}
