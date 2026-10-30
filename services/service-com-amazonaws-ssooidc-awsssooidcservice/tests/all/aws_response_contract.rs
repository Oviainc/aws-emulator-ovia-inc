// SPDX-License-Identifier: CC0-1.0

//! Two things a client reads off this service's responses: which endpoints a
//! registration reports, and which portal a device authorization may start from.
//!
//! Every case drives the service through the router the production emulator serves, so
//! what is asserted is the HTTP response a real SSO OIDC client sees. The harness is the
//! one the device-flow suite beside this file builds, and the fixed text of a refusal is
//! the subject of the `error_descriptions` suite rather than of this one.

use serde_json::{json, Value};

use service_com_amazonaws_ssooidc_awsssooidcservice as ssooidc;
use ssooidc::state::{
    DESCRIPTION_INVALID_REQUEST, GRANT_TYPE_AUTHORIZATION_CODE, GRANT_TYPE_DEVICE_CODE,
    GRANT_TYPE_REFRESH_TOKEN,
};

use crate::device_authorization_flow::{start_url, text, Emulator, REGION};

/// RULE: `authorizationEndpoint` and `tokenEndpoint` are reported only for a registration
/// that declared the authorization-code grant — the one flow that sends a user agent to an
/// authorization endpoint and redeems the code it redirects back — and are then computed
/// from the registration's own issuer when it named one, else from this service's endpoint
/// in the request's Region. A registration that declared no such grant is answered with
/// both members absent.
///
/// The table varies what the rule generalises over: how many grant types are declared,
/// where the declared one sits in the list, whether an issuer URL is named, and whether
/// the registration names the grant at all.
#[tokio::test]
async fn register_client_reports_the_authorization_endpoints_only_for_that_grant() {
    let emulator = Emulator::new();
    let regional = format!("https://oidc.{REGION}.amazonaws.com");
    let issuer = "https://identitycenter.amazonaws.com/ssoins-1234567890abcdef";

    let rows: Vec<(&str, Value, Option<String>)> = vec![
        (
            "no grant types at all",
            json!({"clientName": "endpoints-none", "clientType": "public"}),
            None,
        ),
        (
            "the device-code grant only",
            json!({
                "clientName": "endpoints-device",
                "clientType": "public",
                "grantTypes": [GRANT_TYPE_DEVICE_CODE],
            }),
            None,
        ),
        (
            "two grants, neither the authorization code",
            json!({
                "clientName": "endpoints-device-refresh",
                "clientType": "public",
                "grantTypes": [GRANT_TYPE_DEVICE_CODE, GRANT_TYPE_REFRESH_TOKEN],
            }),
            None,
        ),
        (
            "the authorization-code grant alone",
            json!({
                "clientName": "endpoints-code",
                "clientType": "public",
                "grantTypes": [GRANT_TYPE_AUTHORIZATION_CODE],
            }),
            Some(regional.clone()),
        ),
        (
            "the authorization-code grant last of three",
            json!({
                "clientName": "endpoints-code-last",
                "clientType": "public",
                "grantTypes": [
                    GRANT_TYPE_DEVICE_CODE,
                    GRANT_TYPE_REFRESH_TOKEN,
                    GRANT_TYPE_AUTHORIZATION_CODE,
                ],
            }),
            Some(regional.clone()),
        ),
        (
            "the authorization-code grant with an issuer URL",
            json!({
                "clientName": "endpoints-code-issuer",
                "clientType": "public",
                "grantTypes": [GRANT_TYPE_AUTHORIZATION_CODE],
                "issuerUrl": issuer,
                "redirectUris": ["http://127.0.0.1:52000/oauth/callback"],
            }),
            Some(issuer.to_string()),
        ),
        (
            "the authorization-code grant with an issuer URL carrying a trailing slash",
            json!({
                "clientName": "endpoints-code-issuer-slash",
                "clientType": "public",
                "grantTypes": [GRANT_TYPE_AUTHORIZATION_CODE],
                "issuerUrl": format!("{issuer}/"),
            }),
            Some(issuer.to_string()),
        ),
    ];

    for (case, body, expected_base) in rows {
        let (status, response) = emulator.register_client(&body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {response}"
        );
        let expected = expected_base
            .as_ref()
            .map(|base| (format!("{base}/authorize"), format!("{base}/token")));
        assert_eq!(
            (
                response["authorizationEndpoint"].as_str(),
                response["tokenEndpoint"].as_str()
            ),
            expected
                .as_ref()
                .map_or((None, None), |(authorize, token)| (
                    Some(authorize.as_str()),
                    Some(token.as_str())
                )),
            "case {case:?}: expected the endpoints {expected:?}, got {response}"
        );

        // The registration is live whichever way its endpoints were answered: the pair it
        // minted starts a device authorization, read back off the stored record.
        let client_id = text(&response, "clientId");
        let (started, authorization) = emulator
            .start_device_authorization(&json!({
                "clientId": client_id,
                "clientSecret": text(&response, "clientSecret"),
                "startUrl": start_url(),
            }))
            .await;
        assert_eq!(
            started, 200,
            "case {case:?}: the minted pair was refused: {authorization}"
        );
        assert_eq!(
            emulator
                .state()
                .lock()
                .device_auths
                .get(&text(&authorization, "deviceCode"))
                .map(|record| record.client_id.clone()),
            Some(client_id.clone()),
            "case {case:?}: the device authorization was not persisted for {client_id}"
        );
    }
}

/// RULE: a device authorization is started only from the access portal of an IAM Identity
/// Center instance this account holds — `startUrl` is "The URL for the Amazon Web Services
/// access portal", and a portal exists only for an instance — so a `startUrl` naming no
/// instance's portal is refused with `InvalidRequestException`, named by the restJson1
/// `x-amzn-errortype` header, and nothing is stored for it.
///
/// The table varies what the rule generalises over: the scheme, the host, the subdomain,
/// how deep the host is, the path and the fragment. The accepted rows are the account's own
/// portal, read off the instance records rather than written out as a literal.
#[tokio::test]
async fn start_device_authorization_accepts_only_the_account_access_portal() {
    let emulator = Emulator::new();
    let (client_id, client_secret) = emulator.registered_client(json!({})).await;
    let portal = emulator.account_portal_url();
    let subdomain = portal
        .trim_start_matches("https://")
        .split('.')
        .next()
        .expect("the portal has a subdomain")
        .to_string();

    let rows: Vec<(&str, String, bool)> = vec![
        ("the account's own portal", portal.clone(), true),
        (
            "the account's portal with the portal page's own fragment",
            format!("{portal}/#/"),
            true,
        ),
        (
            "the account's portal with a trailing slash",
            format!("{portal}/"),
            true,
        ),
        (
            "the portal of another account's instance",
            "https://d-0123456789.awsapps.com/start".to_string(),
            false,
        ),
        (
            "a portal subdomain no instance claims",
            "https://my-sso-portal.awsapps.com/start".to_string(),
            false,
        ),
        (
            "the account's portal host without the portal path",
            format!("https://{subdomain}.awsapps.com/"),
            false,
        ),
        (
            "the account's portal host with another path",
            format!("https://{subdomain}.awsapps.com/console"),
            false,
        ),
        (
            "the account's portal over plain HTTP",
            format!("http://{subdomain}.awsapps.com/start"),
            false,
        ),
        (
            "the account's portal one label deeper",
            format!("https://portal.{subdomain}.awsapps.com/start"),
            false,
        ),
        (
            "a portal-looking URL on another domain",
            "https://example.test/start".to_string(),
            false,
        ),
        (
            "a value that is no absolute URI",
            "my-sso-portal".to_string(),
            false,
        ),
        ("an empty start URL", String::new(), false),
    ];

    for (case, start, accepted) in rows {
        let stored_before = emulator.state().lock().device_auths.len();
        let (status, error_type, response) = emulator
            .post_with_error_type(
                "/device_authorization",
                REGION,
                &json!({
                    "clientId": client_id,
                    "clientSecret": client_secret,
                    "startUrl": start,
                }),
            )
            .await;
        let stored_after = emulator.state().lock().device_auths.len();

        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the portal {start:?} to be accepted, got {status} with {response}"
            );
            let device_code = text(&response, "deviceCode");
            assert_eq!(
                emulator
                    .state()
                    .lock()
                    .device_auths
                    .get(&device_code)
                    .map(|record| record.start_url.clone()),
                Some(start.clone()),
                "case {case:?}: the authorization was not persisted for {start:?}"
            );
            assert_eq!(
                stored_after,
                stored_before + 1,
                "case {case:?}: expected one more stored authorization, got {stored_after}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the start URL {start:?} to be refused, got {status} with {response}"
            );
            assert!(
                error_type
                    .as_deref()
                    .is_some_and(|named| named.contains("InvalidRequestException")),
                "case {case:?}: expected the InvalidRequestException error-type header, got {error_type:?}"
            );
            assert_eq!(
                (
                    response["error"].as_str(),
                    response["error_description"].as_str()
                ),
                (Some("invalid_request"), Some(DESCRIPTION_INVALID_REQUEST)),
                "case {case:?}: expected the fixed invalid-request refusal, got {response}"
            );
            assert_eq!(
                stored_after, stored_before,
                "case {case:?}: a refused request stored an authorization ({stored_before} -> {stored_after})"
            );
        }
    }
}
