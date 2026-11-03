// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the URL an app authorization reports for its OAuth flow
//! (`AppAuthorization$authUrl`), driven over the wire (restJson1) through the
//! same router the emulator boots.
//!
//! The RULE every test below states over a TABLE of inputs: an app authorization
//! whose `authType` is granted through an OAuth consent step reports the URL that
//! step runs at, built from the service endpoint of the authorization's Region
//! and the authorization's own stored identifiers, so it is stable across reads
//! and is a URL this emulator answers; an authorization whose `authType` carries
//! its credential already reports none.
//!
//! The harness (the router, `Answer`, `encode`, `arn_of`, `uuid_of`) is the one
//! `app_bundle_ingestion_lifecycle` already builds.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_appfabric_fabricfrontendservice::state::OAUTH2_HANDSHAKE_SETTLES_AFTER;

use crate::app_bundle_ingestion_lifecycle::{arn_of, encode, uuid_of, Answer, Emulator};

/// The two values the model's `AuthType` enum carries
/// (`smithy_query(query="shape", shape="AuthType")` —
/// `shared_types::AUTH_TYPE_VALUES` = `apiKey` | `oauth2`), each with whether an
/// authorization of that type is granted through an OAuth consent step and
/// therefore reports an `authUrl`.
const AUTH_TYPES: &[(&str, bool)] = &[("apiKey", false), ("oauth2", true)];

/// The API key an `apiKey` authorization is created with — a recognisable string,
/// so a test can assert the reported URL carries no credential material.
const API_KEY: &str = "secret-api-key-4d2f";

/// The OAuth2 client id an `oauth2` authorization is created with.
const CLIENT_ID: &str = "secret-client-id-8b31";

/// The OAuth2 client secret an `oauth2` authorization is created with.
const CLIENT_SECRET: &str = "secret-client-secret-c07e";

/// The credential body one `authType` takes — the pairing the model's own
/// documentation of `credential` requires ("Specify credentials that match the
/// authorization type for your request").
fn credential_for(auth_type: &str) -> Value {
    if auth_type == "apiKey" {
        json!({"apiKeyCredential": {"apiKey": API_KEY}})
    } else {
        json!({"oauth2Credential": {"clientId": CLIENT_ID, "clientSecret": CLIENT_SECRET}})
    }
}

/// The service endpoint host for `region`, as the model's own endpoint rule set
/// renders it (`https://appfabric.{Region}.{PartitionResult#dnsSuffix}`) in the
/// `aws` partition.
fn endpoint_host(region: &str) -> String {
    format!("appfabric.{region}.amazonaws.com")
}

/// The URL an authorization's OAuth consent step is expected to run at: the
/// service endpoint for its Region plus the `ConnectAppAuthorization` HTTP URI
/// the model binds
/// (`/appbundles/{appBundleIdentifier}/appauthorizations/{appAuthorizationIdentifier}/connect`)
/// over the two resources' bare identifiers.
fn expected_auth_url(region: &str, bundle_arn: &str, authorization_arn: &str) -> String {
    format!(
        "https://{}/appbundles/{}/appauthorizations/{}/connect",
        endpoint_host(region),
        uuid_of(bundle_arn),
        uuid_of(authorization_arn)
    )
}

/// One app bundle in `region`, as its ARN. AppFabric holds ONE bundle per
/// account per Region, so a fixture that needs its own Region gets its own
/// emulator.
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

/// `GetAppAuthorization` for either identifier form, signed for `region`.
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

/// The `authUrl` of an `appAuthorization` body, or `None` when the member is
/// absent.
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

/// The `appAuthorizationArn` of a success body.
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

/// RULE: an app authorization granted through an OAuth consent step (`authType`
/// `oauth2`) reports the URL that step runs at — the service endpoint of the
/// authorization's Region plus its own connect path — and one that carries its
/// credential already (`apiKey`) reports no URL at all. The URL is derived from
/// the stored record, so every read reports the same one, it survives the
/// connect handshake that moves the authorization's status, and it carries no
/// credential material.
///
/// The table spans both `AuthType` values, applications the issue did not cite,
/// and several Regions — the authorization's Region is what its endpoint names.
#[tokio::test]
async fn an_oauth2_authorization_reports_its_consent_url_and_an_api_key_one_reports_none() {
    // (case, app, tenant, the Region the create call is signed for)
    let placements: &[(&str, &str, &str, &str)] = &[
        ("ZENDESK in the default Region", "ZENDESK", "tenant-z", "us-east-1"),
        ("ASANA in another Region", "ASANA", "tenant-a", "eu-west-1"),
        ("M365AUDITLOGS in a third Region", "M365AUDITLOGS", "tenant-m", "ap-southeast-2"),
    ];

    for (placement, app, tenant, region) in placements {
        for (auth_type, reports_url) in AUTH_TYPES {
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
            let reported = auth_url(&created);

            if *reports_url {
                let expected = expected_auth_url(region, &bundle, &arn);
                assert_eq!(
                    reported.as_deref(),
                    Some(expected.as_str()),
                    "case {case:?}: expected the consent URL {expected:?}, got {reported:?} in body {}",
                    created.body
                );
                for secret in [API_KEY, CLIENT_ID, CLIENT_SECRET] {
                    assert!(
                        !expected.contains(secret),
                        "case {case:?}: the reported URL must carry no credential material, got {reported:?}"
                    );
                }
            } else {
                assert_eq!(
                    reported, None,
                    "case {case:?}: an authorization with no consent step must report no authUrl, got {reported:?} in body {}",
                    created.body
                );
            }

            // The URL is part of the stored record, so a read by either
            // identifier form reports the record the create call returned.
            for identifier in [arn.clone(), uuid_of(&arn)] {
                let read = get_authorization(&emulator, region, &bundle, &identifier).await;
                assert_eq!(
                    read.status, 200,
                    "case {case:?} read by {identifier:?}: expected HTTP 200, got {} with body {}",
                    read.status, read.body
                );
                assert_eq!(
                    read.body.get("appAuthorization"),
                    created.body.get("appAuthorization"),
                    "case {case:?} read by {identifier:?}: expected the created record {}, got {}",
                    created.body,
                    read.body
                );
            }

            // Connecting the authorization moves its status; the consent URL it
            // reports is unchanged.
            let connect_body = if *auth_type == "apiKey" {
                json!({})
            } else {
                json!({"authRequest": {
                    "redirectUri": "https://example.com/oauth/callback",
                    "code": "placeholder-authorization-code",
                }})
            };
            let connected = emulator
                .call_in_region(
                    region,
                    "POST",
                    &format!(
                        "/appbundles/{}/appauthorizations/{}/connect",
                        encode(&bundle),
                        encode(&arn)
                    ),
                    Some(connect_body),
                )
                .await;
            assert_eq!(
                connected.status, 200,
                "case {case:?}: expected the connect call to succeed, got {} with body {}",
                connected.status, connected.body
            );
            let after = get_authorization(&emulator, region, &bundle, &arn).await;
            assert_eq!(
                auth_url(&after),
                reported,
                "case {case:?}: expected the consent URL {reported:?} to survive the connect, got {}",
                after.body
            );
        }
    }
}

/// RULE: the URL an `oauth2` authorization reports is one this emulator answers:
/// the path it names is where that authorization's connect step is served, so a
/// caller that follows the reported URL has that authorization's connect
/// handshake run (its outcome, which for an `oauth2` authorization is the
/// modelled `ConnectionValidationFailed`, is what the record reads once the
/// handshake falls due).
///
/// The table spans applications and Regions, and both of the ways a caller can
/// have arrived at the URL — from the create response and from a later read.
#[tokio::test]
async fn the_reported_consent_url_is_the_path_the_connect_step_is_served_at() {
    // (case, app, tenant, Region, whether the URL is taken from a later read)
    let cases: &[(&str, &str, &str, &str, bool)] = &[
        ("SMARTSHEET from the create response", "SMARTSHEET", "tenant-s", "us-east-1", false),
        ("GOOGLE from a later read", "GOOGLE", "tenant-g", "eu-central-1", true),
        ("OKTA from a later read in another Region", "OKTA", "tenant-o", "us-west-2", true),
    ];

    for (case, app, tenant, region, from_read) in cases {
        let emulator = Emulator::new();
        let bundle = app_bundle(&emulator, region).await;
        let created =
            create_authorization(&emulator, region, &bundle, app, "oauth2", tenant).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected HTTP 201, got {} with body {}",
            created.status, created.body
        );
        let arn = authorization_arn(&created);
        let source = if *from_read {
            get_authorization(&emulator, region, &bundle, &arn).await
        } else {
            created
        };
        let url = auth_url(&source).unwrap_or_else(|| {
            panic!(
                "case {case:?}: an oauth2 authorization must report an authUrl, got {}",
                source.body
            )
        });

        // Follow the URL: everything after the endpoint host is the request path
        // the emulator serves it at.
        let host_prefix = format!("https://{}", endpoint_host(region));
        let path = url.strip_prefix(&host_prefix).unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected the URL to name this service's endpoint {host_prefix:?}, got {url:?}"
            )
        });
        let followed = emulator
            .call_in_region(
                region,
                "POST",
                path,
                Some(json!({"authRequest": {
                    "redirectUri": "https://example.com/oauth/callback",
                    "code": "placeholder-authorization-code",
                }})),
            )
            .await;
        assert_eq!(
            followed.status, 200,
            "case {case:?}: following {url:?} must be served, got HTTP {} with body {}",
            followed.status, followed.body
        );
        // The handshake the followed URL started settles on the clock, so the
        // outcome is read once it is due.
        tokio::time::sleep(OAUTH2_HANDSHAKE_SETTLES_AFTER + Duration::from_millis(200)).await;
        let after = get_authorization(&emulator, region, &bundle, &arn).await;
        assert_eq!(
            after
                .body
                .get("appAuthorization")
                .and_then(|authorization| authorization.get("status")),
            Some(&json!("ConnectionValidationFailed")),
            "case {case:?}: following {url:?} must run the handshake of that authorization, got {}",
            after.body
        );
    }
}

/// RULE: the URL is per AUTHORIZATION, not per application or bundle: every
/// `oauth2` authorization a bundle holds reports its own URL, naming its own
/// identifier.
///
/// The table spans the number of authorizations held at once, which is what "one
/// URL each" has to hold across.
#[tokio::test]
async fn every_oauth2_authorization_reports_its_own_consent_url() {
    const REGION: &str = "us-east-1";

    for count in [1_usize, 2, 4] {
        let emulator = Emulator::new();
        let bundle = app_bundle(&emulator, REGION).await;
        let mut urls: Vec<String> = Vec::new();

        for index in 0..count {
            let created = create_authorization(
                &emulator,
                REGION,
                &bundle,
                "SLACK",
                "oauth2",
                &format!("tenant-{index}"),
            )
            .await;
            assert_eq!(
                created.status, 201,
                "case {count} authorizations, number {index}: expected HTTP 201, got {} with body {}",
                created.status, created.body
            );
            let arn = authorization_arn(&created);
            let expected = expected_auth_url(REGION, &bundle, &arn);
            let reported = auth_url(&created);
            assert_eq!(
                reported.as_deref(),
                Some(expected.as_str()),
                "case {count} authorizations, number {index}: expected {expected:?}, got {reported:?}"
            );
            urls.push(expected);
        }

        let distinct: BTreeSet<&String> = urls.iter().collect();
        assert_eq!(
            distinct.len(),
            urls.len(),
            "case {count} authorizations: every authorization must report its own URL, got {urls:?}"
        );
    }
}
