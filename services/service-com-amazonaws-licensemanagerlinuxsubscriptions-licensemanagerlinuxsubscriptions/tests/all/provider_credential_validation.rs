// SPDX-License-Identifier: CC0-1.0

//! The CREDENTIAL side of `RegisterSubscriptionProvider`: a registration is taken only
//! when the secret its `SecretArn` names can be read AND holds an access token of the form
//! the registered `SubscriptionProviderSource` issues — otherwise the call is refused with
//! the modelled `ValidationException` (HTTP 400) and NOTHING is created.
//!
//! Every call goes through the router's `dispatch`, with Secrets Manager mounted beside
//! this service on one engine (as the assembled emulator runs them), so the secret a case
//! stores is the secret the handler reads.
//!
//! The rules under test:
//!
//! 1. the credential decides the call: a secret holding an access token of the provider's
//!    form registers the provider (HTTP 200, `PENDING`, readable back through
//!    `GetRegisteredSubscriptionProvider`), and a secret that cannot be read or does not
//!    hold such a token is refused with `ValidationException` carrying the service's one
//!    sentence about the provider token, and leaves the provider store and
//!    `ListRegisteredSubscriptionProviders` exactly as they were;
//! 2. the refusal does not depend on concurrency: simultaneous registrations for the same
//!    source, each naming its own secret holding a value that is not an access token, are
//!    ALL refused, so no race admits one and the store is unchanged;
//! 3. a credential nobody reported anything about is not a refusal: with no Secrets Manager
//!    beside this service the lookup establishes nothing, and the registration is taken
//!    rather than refused on a secret no answer was given about.
//!
//! The table spans what rule 1 generalises over: the number of JWT segments, the encoding
//! of a segment (padded, unpadded, not base64url at all), the shape of the decoded segment
//! (object, array, non-JSON), the claims it declares (the offline token type, another token
//! type, none), surrounding whitespace, the kind of value the secret holds (string, binary)
//! and whether the secret exists at all.

use serde_json::{json, Value};

use crate::harness::{
    base64url, create_secret, get_provider, harness, harness_with_secrets_manager, jwt,
    list_providers, member, provider_arn, register, stored_provider_count, token_header, Harness,
    PENDING, RED_HAT, SIGNATURE_SEGMENT, TOKEN_REFUSAL_MESSAGE,
};

/// The issuer an offline token names: the provider's single sign-on realm. Part of the
/// token's payload, not of what makes it well formed, so a case carries it the way a real
/// token does.
const SSO_ISSUER: &str = "https://sso.example-provider.com/auth/realms/external";

/// One row: what the registration's secret holds, and whether a registration naming that
/// secret is one the service can take.
struct Case {
    /// How the row reads in an assertion message.
    label: String,
    /// The `CreateSecret` members storing the credential, or `None` for a row whose
    /// `SecretArn` is to name no secret at all.
    stored: Option<Value>,
    /// Whether `RegisterSubscriptionProvider` must take the registration.
    accepted: bool,
}

impl Case {
    /// A row whose secret holds `secret_string`, which the registration must take.
    fn usable(label: &str, secret_string: String) -> Self {
        Self {
            label: label.to_string(),
            stored: Some(json!({ "SecretString": secret_string })),
            accepted: true,
        }
    }

    /// A row whose secret holds `secret_string`, which is not an access token.
    fn refused(label: &str, secret_string: String) -> Self {
        Self {
            label: label.to_string(),
            stored: Some(json!({ "SecretString": secret_string })),
            accepted: false,
        }
    }
}

/// The payload of a well-formed OFFLINE token: it declares the offline token type, which
/// is what makes it the credential a subscription provider registration needs.
fn offline_payload() -> Value {
    json!({
        "typ": "Offline",
        "iss": SSO_ISSUER,
        "scope": "offline_access",
        "session_state": "e1b0c631-6c0e-4f43-9f2e-2a5e1f3c8a77",
    })
}

/// An offline token, unpadded — the form a provider hands the account holder.
fn offline_token() -> String {
    jwt(&token_header(), &offline_payload(), false)
}

fn credential_cases() -> Vec<Case> {
    vec![
        Case::usable("an offline token", offline_token()),
        Case::usable(
            "an offline token whose segments carry base64 padding",
            jwt(&token_header(), &offline_payload(), true),
        ),
        Case::usable(
            "an offline token stored with surrounding whitespace",
            format!("  {}\n", offline_token()),
        ),
        Case::refused(
            "an arbitrary string",
            "offline-token-placeholder-value".to_string(),
        ),
        Case::refused("a single blank", " ".to_string()),
        Case::refused(
            "one base64url segment and no other",
            base64url(b"{\"typ\":\"Offline\"}", false),
        ),
        Case::refused(
            "a token with no signature segment",
            format!(
                "{}.{}",
                base64url(
                    &serde_json::to_vec(&token_header()).expect("JSON serializes"),
                    false
                ),
                base64url(
                    &serde_json::to_vec(&offline_payload()).expect("JSON serializes"),
                    false
                ),
            ),
        ),
        Case::refused(
            "three segments that are not base64url",
            "@@@@.@@@@.@@@@".to_string(),
        ),
        Case::refused(
            "a token whose header names no signing algorithm",
            jwt(&json!({ "typ": "JWT" }), &offline_payload(), false),
        ),
        Case::refused(
            "a token whose header decodes to a JSON array",
            jwt(&json!([token_header()]), &offline_payload(), false),
        ),
        Case::refused(
            "a token whose payload is not JSON",
            format!(
                "{}.{}.{SIGNATURE_SEGMENT}",
                base64url(
                    &serde_json::to_vec(&token_header()).expect("JSON serializes"),
                    false
                ),
                base64url(b"not json at all", false),
            ),
        ),
        Case::refused(
            "a token whose payload declares no token type",
            jwt(&token_header(), &json!({ "iss": SSO_ISSUER }), false),
        ),
        Case::refused(
            "a token declaring another token type",
            jwt(
                &token_header(),
                &json!({ "typ": "Bearer", "iss": SSO_ISSUER }),
                false,
            ),
        ),
        // The shared reader bounds what a caller-supplied value can ask it to decode, so a
        // secret value past that bound is not read at all — it is refused like any other
        // value that is not a token.
        Case::refused(
            "a value longer than a token may be",
            format!(
                "{}.{}.{SIGNATURE_SEGMENT}",
                base64url(
                    &serde_json::to_vec(&token_header()).expect("JSON serializes"),
                    false
                ),
                "a".repeat(service_common::jwt::MAX_TOKEN_BYTES),
            ),
        ),
        Case {
            label: "a secret holding a binary value only".to_string(),
            stored: Some(json!({ "SecretBinary": base64url(b"an offline token", true) })),
            accepted: false,
        },
        Case {
            label: "a SecretArn naming no secret at all".to_string(),
            stored: None,
            accepted: false,
        },
    ]
}

/// How many providers `ListRegisteredSubscriptionProviders` reports for `RedHat`.
async fn listed_count(h: &Harness) -> usize {
    let reply = list_providers(h, &json!({ "SubscriptionProviderSources": [RED_HAT] })).await;
    reply
        .ok("listing the registered providers")
        .get("RegisteredSubscriptionProviders")
        .and_then(Value::as_array)
        .map_or(0, Vec::len)
}

#[tokio::test]
async fn the_stored_credential_decides_whether_a_registration_is_taken() {
    for (index, case) in credential_cases().into_iter().enumerate() {
        // An emulator per case: the account holds ONE registration per source, so a case
        // whose credential is usable must be the call that creates it rather than one that
        // addresses the case before it.
        let h = harness_with_secrets_manager();
        let label = &case.label;
        let secret_arn = match &case.stored {
            Some(members) => create_secret(&h, &format!("provider-token-{index:02}"), members).await,
            // A secret ARN of the modelled form that names nothing Secrets Manager holds.
            None => format!(
                "arn:aws:secretsmanager:us-west-2:000000000000:secret:absent-token-{index:02}"
            ),
        };
        let before = stored_provider_count(&h);
        let listed_before = listed_count(&h).await;

        let reply = register(
            &h,
            &json!({ "SubscriptionProviderSource": RED_HAT, "SecretArn": secret_arn }),
        )
        .await;

        if case.accepted {
            let arn = provider_arn(&reply, label);
            assert_eq!(
                member(&reply, label, "SubscriptionProviderStatus"),
                Some(PENDING),
                "case {label:?}: a taken registration reports the status of the registration \
                 action, got {:?}",
                reply.body
            );
            // Round trip: the created resource is readable back through the read side,
            // reporting the very secret the registration named.
            let fetched = get_provider(&h, &arn).await;
            assert_eq!(
                member(&fetched, label, "SecretArn"),
                Some(secret_arn.as_str()),
                "case {label:?}: the created provider must report the secret it was \
                 registered with, got {:?}",
                fetched.body
            );
            assert_eq!(
                stored_provider_count(&h),
                before + 1,
                "case {label:?}: a taken registration creates exactly one provider"
            );
            assert_eq!(
                listed_count(&h).await,
                listed_before + 1,
                "case {label:?}: the created provider must be listed"
            );
        } else {
            reply.error(label, 400, "ValidationException");
            // Whichever part of the credential check failed, the refusal is the service's
            // one fixed sentence about the provider token — the class
            // `tests/all/provider_token_refusal_message.rs` states the rule for.
            assert_eq!(
                reply.message(),
                TOKEN_REFUSAL_MESSAGE,
                "case {label:?}: expected {TOKEN_REFUSAL_MESSAGE:?}, got {:?}",
                reply.message()
            );
            // The refusal is answered to a caller whose authorization to read that secret
            // this service never established, so it must carry no part of what the secret
            // holds — only which member is at fault and why.
            for stored in case
                .stored
                .iter()
                .filter_map(|members| members.get("SecretString"))
                .filter_map(Value::as_str)
                .flat_map(|value| value.trim().split('.'))
                .filter(|fragment| fragment.len() > 3)
            {
                assert!(
                    !reply.message().contains(stored),
                    "case {label:?}: the refusal must not echo what the secret holds, got \
                     {:?}",
                    reply.message()
                );
            }
            assert_eq!(
                stored_provider_count(&h),
                before,
                "case {label:?}: a refused registration must create nothing"
            );
            assert_eq!(
                listed_count(&h).await,
                listed_before,
                "case {label:?}: a refused registration must leave the list unchanged, \
                 expected {listed_before}"
            );
        }
    }
}

#[tokio::test]
async fn simultaneous_registrations_of_unusable_credentials_are_all_refused() {
    let h = harness_with_secrets_manager();
    // Each registration names its OWN secret, so nothing but the credential check can
    // refuse them, and each secret holds a value that is not an access token.
    let mut secrets = Vec::new();
    for index in 0..4 {
        secrets.push(
            create_secret(
                &h,
                &format!("race-token-{index}"),
                &json!({ "SecretString": format!("offline-token-placeholder-{index}") }),
            )
            .await,
        );
    }
    let listed_before = listed_count(&h).await;

    let bodies: Vec<Value> = secrets
        .iter()
        .map(|secret_arn| json!({ "SubscriptionProviderSource": RED_HAT, "SecretArn": secret_arn }))
        .collect();
    let (first, second, third, fourth) = tokio::join!(
        register(&h, &bodies[0]),
        register(&h, &bodies[1]),
        register(&h, &bodies[2]),
        register(&h, &bodies[3]),
    );

    for (index, reply) in [first, second, third, fourth].iter().enumerate() {
        let label = format!("concurrent registration {index}");
        reply.error(&label, 400, "ValidationException");
    }
    assert_eq!(
        stored_provider_count(&h),
        0,
        "no concurrent registration of an unusable credential may create a provider"
    );
    assert_eq!(
        listed_count(&h).await,
        listed_before,
        "the registered-provider list must be unchanged, expected {listed_before}"
    );
}

#[tokio::test]
async fn a_credential_nothing_answered_about_is_not_refused() {
    // This service ALONE: the handler's in-process read of the secret reaches no Secrets
    // Manager, so nothing is established about the credential — which is not a negative.
    let h = harness();
    let secret_arn = "arn:aws:secretsmanager:us-west-2:000000000000:secret:unanswered-token";

    let reply = register(
        &h,
        &json!({ "SubscriptionProviderSource": RED_HAT, "SecretArn": secret_arn }),
    )
    .await;

    let label = "a credential nothing answered about";
    let arn = provider_arn(&reply, label);
    assert_eq!(
        member(&reply, label, "SubscriptionProviderStatus"),
        Some(PENDING),
        "case {label:?}: the registration must be taken, got {:?}",
        reply.body
    );
    let fetched = get_provider(&h, &arn).await;
    assert_eq!(
        member(&fetched, label, "SecretArn"),
        Some(secret_arn),
        "case {label:?}: the created provider must report the secret it was registered \
         with, got {:?}",
        fetched.body
    );
}
