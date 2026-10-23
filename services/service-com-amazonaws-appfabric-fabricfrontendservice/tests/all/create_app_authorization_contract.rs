// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the parts of `CreateAppAuthorization`'s contract that
//! are about the REQUEST rather than the record it persists, driven over the
//! wire (restJson1) through the same router the emulator boots.
//!
//! `app_authorization_and_ingestion_dependency` already pins what the created
//! record looks like (it is persisted under its bundle, starts `PendingConnect`
//! and is reported identically by `GetAppAuthorization` and
//! `ListAppAuthorizations`). What this suite states as rules over tables of
//! inputs is:
//!
//! * `clientToken` is an `@idempotencyToken`: replaying it with the same
//!   parameters replays the authorization the first call minted instead of
//!   minting a second one, and replaying it with DIFFERENT parameters is the
//!   modelled `ConflictException` (the `IdempotentParameterMismatch` case) and
//!   writes nothing;
//! * `credential` must be the kind the request's `authType` uses, and no
//!   response ever reports the credential material back;
//! * the `tags` a create call carries are persisted on the AUTHORIZATION and
//!   reported for its ARN by `ListTagsForResource`.
//!
//! The harness (the router, `Answer`, `encode`, `arn_of`) is the one
//! `app_bundle_ingestion_lifecycle` already builds.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::app_bundle_ingestion_lifecycle::{arn_of, encode, Answer, Emulator};

/// The API key an `apiKey` authorization is created with. Distinct, recognisable
/// strings so a test can assert no response echoes the credential material back
/// (both credential shapes are `@sensitive` and `AppAuthorization` has no
/// credential member).
const API_KEY: &str = "secret-api-key-0e1b";

/// The OAuth2 client id an `oauth2` authorization is created with.
const CLIENT_ID: &str = "secret-client-id-7c4a";

/// The OAuth2 client secret an `oauth2` authorization is created with.
const CLIENT_SECRET: &str = "secret-client-secret-91fd";

/// The two values the model's `AuthType` enum carries, each with the credential
/// union member it requires — the pairing the model documents on `credential`
/// ("Specify credentials that match the authorization type for your request.
/// For example, if the authorization type for your request is OAuth2 (oauth2),
/// then you should provide only the OAuth2 credentials").
const AUTH_TYPES: &[(&str, &str)] = &[("apiKey", "apiKeyCredential"), ("oauth2", "oauth2Credential")];

/// A `Credential` union body of one kind, carrying the constants above.
fn credential_of(kind: &str) -> Value {
    match kind {
        "apiKeyCredential" => json!({"apiKeyCredential": {"apiKey": API_KEY}}),
        "oauth2Credential" => json!({"oauth2Credential": {
            "clientId": CLIENT_ID,
            "clientSecret": CLIENT_SECRET,
        }}),
        other => panic!("not a modelled Credential member: {other:?}"),
    }
}

/// A client token in the form the model's `UUID` pattern requires
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`), built
/// from `seed` so a test can name the same token twice.
fn client_token(seed: u32) -> String {
    format!("{seed:08x}-0000-4000-8000-000000000000")
}

/// An app bundle to hang authorizations off, as its ARN.
async fn app_bundle(emulator: &Emulator) -> String {
    let created = emulator.create_app_bundle(json!({})).await;
    assert_eq!(
        created.status, 201,
        "the set-up app bundle must be created, got HTTP {} with body {}",
        created.status, created.body
    );
    arn_of(&created.body, "appBundle")
}

/// `POST /appbundles/{appBundleIdentifier}/appauthorizations` with the body as
/// given, so a case can leave a member out or set it to a value the helpers
/// would not.
async fn create_authorization(emulator: &Emulator, bundle: &str, body: &Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("/appbundles/{}/appauthorizations", encode(bundle)),
            Some(body.clone()),
        )
        .await
}

/// A create body for one application, auth type and tenant, with the matching
/// credential kind.
fn request(app: &str, auth_type: &str, tenant: &str) -> Value {
    let (_, credential_kind) = AUTH_TYPES
        .iter()
        .find(|(modelled, _)| *modelled == auth_type)
        .unwrap_or_else(|| panic!("not a modelled AuthType: {auth_type:?}"));
    json!({
        "app": app,
        "authType": auth_type,
        "credential": credential_of(credential_kind),
        "tenant": {
            "tenantIdentifier": tenant,
            "tenantDisplayName": format!("{tenant} display"),
        },
    })
}

/// The same body with one member replaced (or added).
fn with(body: &Value, member: &str, value: Value) -> Value {
    let mut next = body.clone();
    next.as_object_mut()
        .expect("a JSON object request body")
        .insert(member.to_string(), value);
    next
}

/// The `appAuthorizationArn` of a success body.
fn authorization_arn(answer: &Answer) -> String {
    answer
        .body
        .get("appAuthorization")
        .and_then(|authorization| authorization.get("appAuthorizationArn"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected appAuthorization.appAuthorizationArn in {}", answer.body))
        .to_string()
}

/// A member of an error envelope, or `""`.
fn error_member<'a>(answer: &'a Answer, member: &str) -> &'a str {
    answer
        .body
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// Every `appAuthorizationArn` `ListAppAuthorizations` reports for one bundle.
async fn listed_authorization_arns(emulator: &Emulator, bundle: &str) -> Vec<String> {
    let answer = emulator
        .call(
            "GET",
            &format!("/appbundles/{}/appauthorizations", encode(bundle)),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "ListAppAuthorizations must answer HTTP 200, got {} with body {}",
        answer.status, answer.body
    );
    answer
        .body
        .get("appAuthorizationSummaryList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected appAuthorizationSummaryList in {}", answer.body))
        .iter()
        .map(|summary| {
            summary
                .get("appAuthorizationArn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("expected appAuthorizationArn in {summary}"))
                .to_string()
        })
        .collect()
}

/// `GET /appbundles/{appBundleIdentifier}/appauthorizations/{appAuthorizationIdentifier}`.
async fn get_authorization(emulator: &Emulator, bundle: &str, authorization: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!(
                "/appbundles/{}/appauthorizations/{}",
                encode(bundle),
                encode(authorization)
            ),
            None,
        )
        .await
}

/// `GET /tags/{resourceArn}`.
async fn list_tags(emulator: &Emulator, arn: &str) -> Answer {
    emulator
        .call("GET", &format!("/tags/{}", encode(arn)), None)
        .await
}

/// The tag set a `tags` member carries, as a set of pairs (a tag list is a set
/// of key/value pairs, so order is not part of it).
fn tag_set(tags: Option<&Value>) -> BTreeSet<(String, String)> {
    tags.and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|tag| {
                    let key = tag
                        .get("key")
                        .and_then(Value::as_str)
                        .unwrap_or_else(|| panic!("expected key in {tag}"));
                    let value = tag
                        .get("value")
                        .and_then(Value::as_str)
                        .unwrap_or_else(|| panic!("expected value in {tag}"));
                    (key.to_string(), value.to_string())
                })
                .collect()
        })
        .unwrap_or_default()
}

/// What a second create carrying an already-spent `clientToken` must do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Replay {
    /// The parameters are the same request, so the first call's authorization is
    /// replayed rather than a second one minted.
    SameAuthorization,
    /// The parameters differ, so the token is the conflict AWS documents as
    /// `IdempotentParameterMismatch`.
    Mismatch,
}

/// RULE: `clientToken` is an `@idempotencyToken`, so a create call repeating a
/// token the same operation already spent mints nothing: with the SAME
/// parameters it replays the authorization the first call created (same ARN,
/// same body, HTTP 201), and with DIFFERENT parameters it is refused with the
/// modelled `ConflictException` (HTTP 409) naming the token, leaving the bundle
/// holding exactly the first authorization either way.
///
/// The table spans the members a create call identifies an authorization by —
/// `app`, each `tenant` member, `authType` with its credential kind, and `tags`
/// — plus the two forms of "the same request": byte-identical, and a tag list in
/// another order.
#[tokio::test]
async fn a_spent_client_token_replays_its_authorization_and_refuses_different_parameters() {
    let base = request("ASANA", "apiKey", "tenant-a");
    let two_tags = json!([{"key": "env", "value": "prod"}, {"key": "team", "value": "data"}]);
    let reordered_tags = json!([{"key": "team", "value": "data"}, {"key": "env", "value": "prod"}]);

    // (case, first body, second body, what the second call must do)
    let cases: Vec<(&str, Value, Value, Replay)> = vec![
        (
            "byte-identical retry",
            base.clone(),
            base.clone(),
            Replay::SameAuthorization,
        ),
        (
            "the same tags in another order",
            with(&base, "tags", two_tags.clone()),
            with(&base, "tags", reordered_tags),
            Replay::SameAuthorization,
        ),
        (
            "another app",
            base.clone(),
            request("ZENDESK", "apiKey", "tenant-a"),
            Replay::Mismatch,
        ),
        (
            "another tenantIdentifier",
            base.clone(),
            request("ASANA", "apiKey", "tenant-b"),
            Replay::Mismatch,
        ),
        (
            "another tenantDisplayName",
            base.clone(),
            with(
                &base,
                "tenant",
                json!({"tenantIdentifier": "tenant-a", "tenantDisplayName": "renamed"}),
            ),
            Replay::Mismatch,
        ),
        (
            "another authType and credential kind",
            base.clone(),
            request("ASANA", "oauth2", "tenant-a"),
            Replay::Mismatch,
        ),
        (
            "tags added",
            base.clone(),
            with(&base, "tags", two_tags.clone()),
            Replay::Mismatch,
        ),
        (
            "another tag value",
            with(&base, "tags", two_tags),
            with(
                &base,
                "tags",
                json!([{"key": "env", "value": "dev"}, {"key": "team", "value": "data"}]),
            ),
            Replay::Mismatch,
        ),
    ];

    for (case, first_body, second_body, expected) in cases {
        // A fresh emulator per row: a spent token is state, so one row must not
        // see the tokens another row spent.
        let emulator = Emulator::new();
        let bundle = app_bundle(&emulator).await;
        let token = client_token(1);

        let first = create_authorization(&emulator, &bundle, &with(&first_body, "clientToken", json!(token))).await;
        assert_eq!(
            first.status, 201,
            "case {case:?}: the first create must answer HTTP 201, got {} with body {}",
            first.status, first.body
        );
        let first_arn = authorization_arn(&first);

        let second = create_authorization(&emulator, &bundle, &with(&second_body, "clientToken", json!(token))).await;
        match expected {
            Replay::SameAuthorization => {
                assert_eq!(
                    second.status, 201,
                    "case {case:?}: a retry must answer HTTP 201, got {} with body {}",
                    second.status, second.body
                );
                assert_eq!(
                    second.body, first.body,
                    "case {case:?}: a retry must replay the first authorization {}, got {}",
                    first.body, second.body
                );
            }
            Replay::Mismatch => {
                assert_eq!(
                    second.status, 409,
                    "case {case:?}: a token replayed with different parameters must answer HTTP 409, got {} with body {}",
                    second.status, second.body
                );
                assert!(
                    second.error_code().contains("ConflictException"),
                    "case {case:?}: expected a ConflictException, got {:?} with body {}",
                    second.error_code(),
                    second.body
                );
                assert_eq!(
                    error_member(&second, "resourceId"),
                    token,
                    "case {case:?}: the refusal must name the replayed token, got body {}",
                    second.body
                );
            }
        }

        // Either way the bundle holds exactly the first authorization: a retry
        // minted nothing, and a refused call wrote nothing.
        let stored = listed_authorization_arns(&emulator, &bundle).await;
        assert_eq!(
            stored,
            vec![first_arn.clone()],
            "case {case:?}: expected the bundle to hold only {first_arn:?}, got {stored:?}"
        );
    }
}

/// RULE: a `clientToken` is scoped to the request it was first spent on, so
/// DISTINCT tokens mint distinct authorizations — one per token — and each is
/// readable back under its own ARN.
///
/// The table spans the number of distinct (token, tenant) pairs, which is what
/// "one per token" has to hold across.
#[tokio::test]
async fn distinct_client_tokens_mint_distinct_authorizations() {
    for count in [1_u32, 2, 5] {
        let emulator = Emulator::new();
        let bundle = app_bundle(&emulator).await;
        let mut minted: Vec<String> = Vec::new();

        for seed in 0..count {
            let body = request("DROPBOX", "oauth2", &format!("tenant-{seed}"));
            let created =
                create_authorization(&emulator, &bundle, &with(&body, "clientToken", json!(client_token(seed)))).await;
            assert_eq!(
                created.status, 201,
                "case {count} tokens, token {seed}: expected HTTP 201, got {} with body {}",
                created.status, created.body
            );
            minted.push(authorization_arn(&created));
        }

        let distinct: BTreeSet<&String> = minted.iter().collect();
        assert_eq!(
            distinct.len(),
            minted.len(),
            "case {count} tokens: every distinct token must mint its own authorization, got {minted:?}"
        );
        let stored: BTreeSet<String> = listed_authorization_arns(&emulator, &bundle)
            .await
            .into_iter()
            .collect();
        assert_eq!(
            stored,
            minted.iter().cloned().collect::<BTreeSet<String>>(),
            "case {count} tokens: expected the bundle to hold {minted:?}, got {stored:?}"
        );
    }
}

/// RULE: `credential` must be the union member the request's `authType` uses —
/// the pairing the model documents on the member. A matching pair is created and
/// persists its `authType`; a mismatched one is refused with the modelled
/// `ValidationException` (HTTP 400) naming the `credential` member, and writes
/// nothing. No response ever reports the credential material back.
///
/// The table is every (`AuthType`, credential kind) combination the model allows
/// — both matching and both crossed — over applications the issue did not cite.
#[tokio::test]
async fn a_credential_must_match_the_auth_type_and_is_never_reported_back() {
    for (app, tenant) in [("JIRA", "tenant-j"), ("SMARTSHEET", "tenant-s")] {
        for (auth_type, required_kind) in AUTH_TYPES {
            for (_, supplied_kind) in AUTH_TYPES {
                let case = format!("{app} {auth_type} with {supplied_kind}");
                let emulator = Emulator::new();
                let bundle = app_bundle(&emulator).await;
                let body = with(
                    &request(app, auth_type, tenant),
                    "credential",
                    credential_of(supplied_kind),
                );
                let created = create_authorization(&emulator, &bundle, &body).await;

                if supplied_kind == required_kind {
                    assert_eq!(
                        created.status, 201,
                        "case {case:?}: a matching credential must be accepted, got HTTP {} with body {}",
                        created.status, created.body
                    );
                    let read = get_authorization(&emulator, &bundle, &authorization_arn(&created)).await;
                    assert_eq!(
                        read.status, 200,
                        "case {case:?}: the created authorization must be readable, got HTTP {} with body {}",
                        read.status, read.body
                    );
                    assert_eq!(
                        read.body.get("appAuthorization").and_then(|a| a.get("authType")),
                        Some(&json!(auth_type)),
                        "case {case:?}: expected the authType to persist, got {}",
                        read.body
                    );
                    for (where_, answer) in [("the create response", &created), ("the read response", &read)] {
                        let rendered = answer.body.to_string();
                        for secret in [API_KEY, CLIENT_ID, CLIENT_SECRET] {
                            assert!(
                                !rendered.contains(secret),
                                "case {case:?}: {where_} must not report the credential material {secret:?}, got {rendered}"
                            );
                        }
                    }
                } else {
                    assert_eq!(
                        created.status, 400,
                        "case {case:?}: a crossed credential must be refused, got HTTP {} with body {}",
                        created.status, created.body
                    );
                    assert!(
                        created.error_code().contains("ValidationException"),
                        "case {case:?}: expected a ValidationException, got {:?} with body {}",
                        created.error_code(),
                        created.body
                    );
                    assert!(
                        error_member(&created, "message").contains("credential"),
                        "case {case:?}: the refusal must name the credential member, got {}",
                        created.body
                    );
                    let stored = listed_authorization_arns(&emulator, &bundle).await;
                    assert!(
                        stored.is_empty(),
                        "case {case:?}: a refused create must write nothing, got {stored:?}"
                    );
                }
            }
        }
    }
}

/// RULE: the `tags` a create call carries are persisted on the AUTHORIZATION it
/// mints, so `ListTagsForResource` reports exactly that set for the
/// authorization's own ARN — independently of the tags its app bundle holds —
/// and the modelled `AppAuthorization` itself reports no tag member.
///
/// The table spans the size of the tag list: absent, empty, one, several, and
/// the model's own `TagList` bound (`@length(min: 0, max: 50)`).
#[tokio::test]
async fn tags_given_to_create_are_reported_for_the_authorization_arn() {
    // (case, the `tags` member or None)
    let cases: Vec<(&str, Option<Value>)> = vec![
        ("no tags member", None),
        ("an empty tag list", Some(json!([]))),
        ("one tag", Some(json!([{"key": "env", "value": "prod"}]))),
        (
            "three tags",
            Some(json!([
                {"key": "env", "value": "prod"},
                {"key": "team", "value": "data"},
                {"key": "owner", "value": "platform"},
            ])),
        ),
        (
            "the modelled maximum of 50 tags",
            Some(Value::Array(
                (0..50)
                    .map(|index| json!({"key": format!("key-{index}"), "value": format!("value-{index}")}))
                    .collect(),
            )),
        ),
    ];

    for (case, tags) in cases {
        let emulator = Emulator::new();
        // The bundle carries a tag of its own, so a tag reported for the
        // authorization cannot have come from its parent.
        let bundle_answer = emulator
            .create_app_bundle(json!({"tags": [{"key": "owner", "value": "the-bundle"}]}))
            .await;
        assert_eq!(
            bundle_answer.status, 201,
            "case {case:?}: the set-up app bundle must be created, got HTTP {} with body {}",
            bundle_answer.status, bundle_answer.body
        );
        let bundle = arn_of(&bundle_answer.body, "appBundle");

        let mut body = request("CISCO", "apiKey", "tenant-c");
        if let Some(tags) = tags.clone() {
            body = with(&body, "tags", tags);
        }
        let created = create_authorization(&emulator, &bundle, &body).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );
        let arn = authorization_arn(&created);
        assert!(
            created
                .body
                .get("appAuthorization")
                .and_then(|authorization| authorization.get("tags"))
                .is_none(),
            "case {case:?}: AppAuthorization models no tags member, got {}",
            created.body
        );

        let expected = tag_set(tags.as_ref());
        let reported = list_tags(&emulator, &arn).await;
        assert_eq!(
            reported.status, 200,
            "case {case:?}: ListTagsForResource must answer HTTP 200 for {arn:?}, got {} with body {}",
            reported.status, reported.body
        );
        let actual = tag_set(reported.body.get("tags"));
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the authorization to hold {expected:?}, got {actual:?}"
        );

        let bundle_tags = list_tags(&emulator, &bundle).await;
        let bundle_actual = tag_set(bundle_tags.body.get("tags"));
        assert_eq!(
            bundle_actual,
            BTreeSet::from([("owner".to_string(), "the-bundle".to_string())]),
            "case {case:?}: the bundle's own tags must be untouched, got {bundle_actual:?}"
        );
    }
}
