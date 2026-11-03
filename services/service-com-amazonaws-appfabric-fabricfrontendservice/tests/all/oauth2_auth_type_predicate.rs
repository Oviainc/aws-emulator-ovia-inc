// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE question both the reporting and the connect path
//! ask about a stored app authorization — "is this the kind whose grant runs
//! through an OAuth consent step?" — driven over the wire (restJson1) through the
//! same router the emulator boots.
//!
//! The RULE every test below states over a TABLE of inputs: an app authorization
//! whose stored `authType` is the OAuth2 one is EXACTLY the one that reports a
//! consent URL (`AppAuthorization$authUrl`) and EXACTLY the one whose connect
//! handshake is refused without the `authRequest` that step hands back, so the two
//! paths can never disagree about which authorizations are that kind.
//!
//! The harness (the router, `Answer`, `encode`, `arn_of`) is the one
//! `app_bundle_ingestion_lifecycle` already builds.

use serde_json::{json, Value};

use crate::app_bundle_ingestion_lifecycle::{arn_of, encode, Answer, Emulator};

/// Every value the model's `AuthType` enum carries
/// (`smithy_query(query="shape", shape="AuthType")` — also listed by this crate's
/// `ops::types::shared_types::AUTH_TYPE_VALUES` as `apiKey` | `oauth2`), with
/// whether an authorization of that type is granted through an OAuth consent
/// step.
const AUTH_TYPES: &[(&str, bool)] = &[("apiKey", false), ("oauth2", true)];

/// The credential body one `authType` takes — the pairing the model's own
/// documentation of `credential` requires ("Specify credentials that match the
/// authorization type for your request").
fn credential_for(auth_type: &str) -> Value {
    if auth_type == "apiKey" {
        json!({"apiKeyCredential": {"apiKey": "placeholder-api-key"}})
    } else {
        json!({"oauth2Credential": {
            "clientId": "placeholder-client-id",
            "clientSecret": "placeholder-client-secret",
        }})
    }
}

/// `CreateAppBundle` then its ARN, signed for `region` (AppFabric holds one
/// bundle per account per Region).
async fn app_bundle(emulator: &Emulator, region: &str) -> String {
    let created = emulator
        .create_app_bundle_in_region(region, json!({}))
        .await;
    assert_eq!(
        created.status, 201,
        "region {region:?}: the set-up app bundle must be created, got HTTP {} with body {}",
        created.status, created.body
    );
    arn_of(&created.body, "appBundle")
}

/// `CreateAppAuthorization`, signed for `region`.
async fn create_authorization(
    emulator: &Emulator,
    region: &str,
    bundle: &str,
    app: &str,
    auth_type: &str,
    tenant: &str,
) -> Answer {
    emulator
        .call_in_region(
            region,
            "POST",
            &format!("/appbundles/{}/appauthorizations", encode(bundle)),
            Some(json!({
                "app": app,
                "authType": auth_type,
                "credential": credential_for(auth_type),
                "tenant": {
                    "tenantIdentifier": tenant,
                    "tenantDisplayName": format!("{tenant} display"),
                },
            })),
        )
        .await
}

/// `GetAppAuthorization`, signed for `region` — the reporting path, whose
/// `authUrl` is one of the two answers the shared predicate gives.
async fn get_authorization(
    emulator: &Emulator,
    region: &str,
    bundle: &str,
    authorization: &str,
) -> Answer {
    emulator
        .call_in_region(
            region,
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

/// `ConnectAppAuthorization`, signed for `region`, carrying `auth_request` or
/// nothing — the mutating path, whose refusal is the other answer the shared
/// predicate gives.
async fn connect(
    emulator: &Emulator,
    region: &str,
    bundle: &str,
    authorization: &str,
    with_auth_request: bool,
) -> Answer {
    let body = if with_auth_request {
        json!({"authRequest": {
            "redirectUri": "https://example.com/oauth/callback",
            "code": "placeholder-authorization-code",
        }})
    } else {
        json!({})
    };
    emulator
        .call_in_region(
            region,
            "POST",
            &format!(
                "/appbundles/{}/appauthorizations/{}/connect",
                encode(bundle),
                encode(authorization)
            ),
            Some(body),
        )
        .await
}

/// The `authUrl` an `appAuthorization` body reports, or `None` when absent.
fn auth_url(answer: &Answer) -> Option<String> {
    answer
        .body
        .get("appAuthorization")
        .unwrap_or_else(|| panic!("expected an appAuthorization in {}", answer.body))
        .get("authUrl")
        .map(|url| {
            url.as_str()
                .unwrap_or_else(|| panic!("authUrl must be a string, got {url}"))
                .to_string()
        })
}

/// The `appAuthorizationArn` of a create response.
fn authorization_arn(answer: &Answer) -> String {
    answer
        .body
        .get("appAuthorization")
        .and_then(|authorization| authorization.get("appAuthorizationArn"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!(
                "expected appAuthorization.appAuthorizationArn in {}",
                answer.body
            )
        })
        .to_string()
}

/// The `status` an `appAuthorization` body reports.
fn status_of(answer: &Answer) -> Option<String> {
    answer
        .body
        .get("appAuthorization")
        .and_then(|authorization| authorization.get("status"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// Whether a connect answer is the refusal the model mandates for a missing
/// `authRequest`: HTTP 400, a `ValidationException`, and the member it names is
/// `authRequest`.
fn refuses_missing_auth_request(answer: &Answer, case: &str) -> bool {
    if answer.status == 200 {
        return false;
    }
    assert_eq!(
        answer.status, 400,
        "case {case:?}: a connect refusal must be HTTP 400, got {} with body {}",
        answer.status, answer.body
    );
    assert!(
        answer.error_code().contains("ValidationException"),
        "case {case:?}: expected a ValidationException, got {:?} with body {}",
        answer.error_code(),
        answer.body
    );
    let names_member = answer
        .body
        .get("fieldList")
        .and_then(Value::as_array)
        .is_some_and(|fields| {
            fields
                .iter()
                .any(|field| field.get("name") == Some(&json!("authRequest")))
        });
    assert!(
        names_member,
        "case {case:?}: the refusal must name the authRequest member, got {}",
        answer.body
    );
    true
}

/// RULE: the stored `authType` being the OAuth2 one is the single question both
/// the reporting path and the connect handshake answer — an authorization reports
/// a consent URL if and only if its connect call is refused without the
/// `authRequest` that step hands back — and an authorization of the other type
/// reports no URL and connects with no `authRequest` at all.
///
/// The table spans both `AuthType` values, applications the issue did not cite,
/// and several Regions.
#[tokio::test]
async fn the_consent_url_and_the_connect_requirement_answer_the_same_question() {
    // (case, app, tenant, the Region the calls are signed for)
    let placements: &[(&str, &str, &str, &str)] = &[
        ("CISCO in the default Region", "CISCO", "tenant-c", "us-east-1"),
        ("DROPBOX in another Region", "DROPBOX", "tenant-d", "eu-west-2"),
        ("JIRA in a third Region", "JIRA", "tenant-j", "ap-northeast-1"),
    ];

    for (placement, app, tenant, region) in placements {
        for (auth_type, oauth_kind) in AUTH_TYPES {
            let case = format!("{placement} with authType {auth_type}");
            // A fresh emulator per row: a Region holds one app bundle.
            let emulator = Emulator::new();
            let bundle = app_bundle(&emulator, region).await;

            let created =
                create_authorization(&emulator, region, &bundle, app, auth_type, tenant).await;
            assert_eq!(
                created.status, 201,
                "case {case:?}: expected HTTP 201, got {} with body {}",
                created.status, created.body
            );
            let arn = authorization_arn(&created);

            let read = get_authorization(&emulator, region, &bundle, &arn).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected the read to succeed, got {} with body {}",
                read.status, read.body
            );
            let reports_url = auth_url(&read).is_some();
            assert_eq!(
                reports_url, *oauth_kind,
                "case {case:?}: expected authUrl reported = {oauth_kind}, got {:?} in body {}",
                auth_url(&read),
                read.body
            );

            let bare = connect(&emulator, region, &bundle, &arn, false).await;
            let requires_auth_request = refuses_missing_auth_request(&bare, &case);
            assert_eq!(
                requires_auth_request, reports_url,
                "case {case:?}: expected the connect requirement ({requires_auth_request}) to \
                 agree with the reported consent URL ({reports_url}), got connect body {} and \
                 read body {}",
                bare.body, read.body
            );

            // A refused connect wrote nothing: the authorization is still
            // unconnected, and completing the handshake the way its kind
            // requires is what moves it.
            let after_bare = get_authorization(&emulator, region, &bundle, &arn).await;
            let expected_status = if requires_auth_request {
                "PendingConnect"
            } else {
                "Connected"
            };
            assert_eq!(
                status_of(&after_bare).as_deref(),
                Some(expected_status),
                "case {case:?}: expected status {expected_status:?} after a connect with no \
                 authRequest, got {}",
                after_bare.body
            );

            let carried = connect(&emulator, region, &bundle, &arn, true).await;
            assert_eq!(
                carried.status, 200,
                "case {case:?}: a connect carrying an authRequest must be served, got {} with \
                 body {}",
                carried.status, carried.body
            );
            let after = get_authorization(&emulator, region, &bundle, &arn).await;
            // The handshake of an `apiKey` authorization completes with the
            // call; an `oauth2` one has nothing this emulator can redeem, so
            // its outcome is the clock's and the record still awaits it here
            // (`app_authorization_connect_validation` asserts that outcome).
            let expected_status = if *oauth_kind {
                "PendingConnect"
            } else {
                "Connected"
            };
            assert_eq!(
                status_of(&after).as_deref(),
                Some(expected_status),
                "case {case:?}: expected the authorization to read back {expected_status}, got {}",
                after.body
            );
            // The reporting answer is unchanged by the handshake: it is the
            // stored kind that decides it, not the status.
            assert_eq!(
                auth_url(&after).is_some(),
                reports_url,
                "case {case:?}: expected authUrl reported = {reports_url} after connecting, got {}",
                after.body
            );
        }
    }
}

/// RULE: the question is asked of EACH stored authorization, so a bundle holding
/// authorizations of both kinds at once answers it per authorization: each one's
/// reported consent URL agrees with its own connect requirement, whatever the
/// others are and in whatever order they are connected.
///
/// The table spans the number of authorizations held at once and the order the
/// kinds were created in, which is what "per authorization" has to hold across.
#[tokio::test]
async fn each_stored_authorization_answers_the_question_for_itself() {
    const REGION: &str = "us-east-1";
    // (case, the authType of each authorization the bundle holds, in create order)
    let mixes: &[(&str, &[&str])] = &[
        ("one of each, oauth2 first", &["oauth2", "apiKey"]),
        ("one of each, apiKey first", &["apiKey", "oauth2"]),
        (
            "four, interleaved",
            &["apiKey", "oauth2", "oauth2", "apiKey"],
        ),
    ];

    for (case, mix) in mixes {
        let emulator = Emulator::new();
        let bundle = app_bundle(&emulator, REGION).await;
        let mut held: Vec<(String, &str)> = Vec::new();

        for (index, auth_type) in mix.iter().enumerate() {
            let created = create_authorization(
                &emulator,
                REGION,
                &bundle,
                "SLACK",
                auth_type,
                &format!("tenant-{index}"),
            )
            .await;
            assert_eq!(
                created.status, 201,
                "case {case:?}, authorization {index}: expected HTTP 201, got {} with body {}",
                created.status, created.body
            );
            held.push((authorization_arn(&created), auth_type));
        }

        for (index, (arn, auth_type)) in held.iter().enumerate() {
            let row = format!("{case}, authorization {index} ({auth_type})");
            let read = get_authorization(&emulator, REGION, &bundle, arn).await;
            let reports_url = auth_url(&read).is_some();
            let bare = connect(&emulator, REGION, &bundle, arn, false).await;
            let requires_auth_request = refuses_missing_auth_request(&bare, &row);
            assert_eq!(
                requires_auth_request, reports_url,
                "case {row:?}: expected the connect requirement ({requires_auth_request}) to \
                 agree with the reported consent URL ({reports_url}), got connect body {} and \
                 read body {}",
                bare.body, read.body
            );
            let expected = AUTH_TYPES
                .iter()
                .find(|(value, _)| value == auth_type)
                .map(|(_, oauth_kind)| *oauth_kind)
                .unwrap_or_else(|| panic!("case {row:?}: unknown authType {auth_type:?}"));
            assert_eq!(
                reports_url, expected,
                "case {row:?}: expected authUrl reported = {expected}, got {}",
                read.body
            );

            let carried = connect(&emulator, REGION, &bundle, arn, true).await;
            assert_eq!(
                carried.status, 200,
                "case {row:?}: a connect carrying an authRequest must be served, got {} with \
                 body {}",
                carried.status, carried.body
            );
        }

        // Connecting them all left each one's reported URL exactly as its own
        // kind says.
        for (index, (arn, auth_type)) in held.iter().enumerate() {
            let row = format!("{case}, authorization {index} ({auth_type})");
            let read = get_authorization(&emulator, REGION, &bundle, arn).await;
            let expected = *auth_type == "oauth2";
            assert_eq!(
                auth_url(&read).is_some(),
                expected,
                "case {row:?}: expected authUrl reported = {expected}, got {}",
                read.body
            );
        }
    }
}
