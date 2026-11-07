// SPDX-License-Identifier: CC0-1.0

//! How this endpoint answers a refusal raised by a TERMS-DOCUMENT, a
//! MANAGED-LOGIN-BRANDING or a PASSKEY-REGISTRATION call, over the wire (`POST /` with
//! `X-Amz-Target: AWSCognitoIdentityProviderService.<Op>`).
//!
//! The rule under test is the one [`super::error_wire`] states for this endpoint, asserted
//! here over the writers and the by-client readers its own table does not drive: a refusal
//! of the CALLER is answered with HTTP 400 and names its error shape in the
//! `x-amzn-errortype` response header, as the shape's simple name followed by a colon,
//! whatever `@httpError` the model annotates that shape with. The model gives
//! `ResourceNotFoundException` 404 and `NotAuthorizedException` 403, and leaves
//! `InvalidParameterException` and `WebAuthnChallengeNotFoundException` at 400
//! (`smithy_query(query="operation",operation="DeleteTerms,UpdateTerms,DescribeTermsByClient,DescribeManagedLoginBrandingByClient,UpdateManagedLoginBranding,StartWebAuthnRegistration,CompleteWebAuthnRegistration")`);
//! this endpoint applies none of the non-400 ones, carrying the error class in the body's
//! `__type` and in the header an AWS SDK resolves the exception type from first, while the
//! status says only that the request was refused. That matters for a client that branches
//! on the status: a 404 from an RPC endpoint reads as "no such route", so reconciliation
//! code that tolerates a missing terms document or branding style aborts instead, and a 403
//! on a registration call reads as a credential failure, so a client re-signs rather than
//! prompting the user to authenticate again.
//!
//! The rows span what the rule generalises over, so no row is the rule: the error CLASS —
//! and so the modelled status — the refusal is raised with (404, 403 and 400), the resource
//! FAMILY the call addresses (a pool's terms documents, an app client's branding style, a
//! user's passkey registration), the KIND of call (a delete, an update, a read by app
//! client, the start and the completion of a ceremony), the KIND of refusal (a pool nothing
//! created, an id nothing minted, an app client that holds no such resource, a token no
//! sign-in issued, a parameter the request has to correct, a challenge nothing handed out)
//! and the LAYER the refusal is raised at (a handler's own modelled error, and the
//! framework's refusal of a request that breaks a modelled rule, which no handler sees).
//! The control is the same operations against the resources this service HOLDS: they are
//! served, keep their own status and name no error shape.
//!
//! Every assertion names its case and reports the value it saw.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::state;

use super::error_wire::{
    call, error_code, ok, router, Answered, ERRORTYPE, REFUSED, UNMINTED_CLIENT, UNMINTED_POOL,
};

/// A password satisfying a default pool's password policy, so a created user can be given a
/// permanent one and sign in.
const GOOD_PASSWORD: &str = "Passw0rd!2024";

/// `AllowedFirstAuthFactors` of a pool that offers passkey sign-in
/// (`smithy_query(query="shape",shape="SignInPolicyType")`, whose values are
/// `AuthFactorType`'s), so a passkey registration of this pool is refused for what the row
/// names rather than for a pool that offers no passkeys.
const PASSKEY_FACTORS: [&str; 2] = ["PASSWORD", "WEB_AUTHN"];

/// The relying party the pool below registers passkeys under.
const RELYING_PARTY: &str = "passkeys.example.com";

/// A value submitted as an `AccessToken` that no sign-in of this emulator issued, in the
/// form `TokenModelType`'s pattern demands (`^[A-Za-z0-9-_=.]+$`, from
/// `smithy_query(query="operation",operation="StartWebAuthnRegistration")`).
const UNISSUED_TOKEN: &str = "not.an.access.token";

/// A terms-document id nothing created, in the form `TermsIdType`'s pattern demands (a
/// version-4 UUID, from `smithy_query(query="operation",operation="DeleteTerms")`).
const UNMINTED_TERMS: &str = "3ff8c9d4-8b3c-4b5e-ad1f-4e6f8a0b2c3d";

/// A managed-login-branding id nothing created, in the form
/// `ManagedLoginBrandingIdType`'s pattern demands (a version-4 UUID, from
/// `smithy_query(query="operation",operation="UpdateManagedLoginBranding")`).
const UNMINTED_BRANDING: &str = "4aa9d0e5-9c4d-4c6f-be20-5f708b1c3d4e";

/// The two document types `TermsNameType`'s pattern admits
/// (`^(terms-of-use|privacy-policy)$`, from
/// `smithy_query(query="operation",operation="DescribeTermsByClient")`): the fixture below
/// stores the first for one app client, so the second names no stored document.
const TERMS_NAMES: [&str; 2] = ["terms-of-use", "privacy-policy"];

/// A credential id a passkey provider chose, base64url as `RegistrationResponseJSON`
/// carries it.
const CREDENTIAL_ID: &str = "Y3JlZGVudGlhbC1vbmU";

/// An attestation statement of format `none`, which is what the creation options this
/// service hands out ask for.
const ATTESTATION_OBJECT: &str = "o2NmbXRkbm9uZWdhdHRTdG10oGhhdXRoRGF0YUA";

/// A challenge no `StartWebAuthnRegistration` of this emulator handed out.
const UNHELD_CHALLENGE: &str = "Y2hhbGxlbmdlLW5vdGhpbmctaGFuZGVkLW91dA";

/// One emulator, with the pool, app clients, terms documents, branding style and signed-in
/// user every row below is answered against.
struct Fixture {
    router: MultiServiceRouter,
    user_pool_id: String,
    /// The app client holding a terms document and a branding style.
    client_with_resources: String,
    /// Another app client of the same pool, holding neither.
    bare_client: String,
    terms_id: String,
    branding_id: String,
    /// The access token a real sign-in of this pool issued.
    access_token: String,
}

/// The `Links` of a terms document: one language and one URL, which is the map's modelled
/// minimum length (`smithy_query(query="shape",shape="LinksType")`).
fn links() -> Value {
    json!({ "cognito:default": "https://terms.example.com" })
}

/// `RegistrationResponseJSON` as a passkey provider reports it for the registration
/// answering `challenge`, collected at the relying party's own origin.
fn registration_response(challenge: &str) -> Value {
    let client_data = json!({
        "type": "webauthn.create",
        "challenge": challenge,
        "origin": format!("https://{RELYING_PARTY}"),
        "crossOrigin": false,
    });
    json!({
        "id": CREDENTIAL_ID,
        "rawId": CREDENTIAL_ID,
        "type": "public-key",
        "response": {
            "clientDataJSON": state::base64_url(client_data.to_string().as_bytes()),
            "attestationObject": ATTESTATION_OBJECT,
        },
        "clientExtensionResults": {},
    })
}

/// Create an app client that allows the password sign-in this suite drives, and return its
/// minted `ClientId`.
async fn app_client(router: &MultiServiceRouter, user_pool_id: &str, name: &str) -> String {
    let created = ok(
        router,
        "CreateUserPoolClient",
        &json!({
            "UserPoolId": user_pool_id,
            "ClientName": name,
            "ExplicitAuthFlows": ["ALLOW_USER_PASSWORD_AUTH", "ALLOW_REFRESH_TOKEN_AUTH"],
        }),
    )
    .await;
    created["UserPoolClient"]["ClientId"]
        .as_str()
        .unwrap_or_else(|| panic!("a minted app client id: {created}"))
        .to_string()
}

/// Everything the tables below are answered against: a pool that offers passkeys under a
/// relying party, two app clients, a terms document and a branding style on one of them,
/// and a signed-in user of the pool.
async fn fixture(pool_name: &str) -> Fixture {
    let router = router();
    let created = ok(
        &router,
        "CreateUserPool",
        &json!({
            "PoolName": pool_name,
            "Policies": { "SignInPolicy": { "AllowedFirstAuthFactors": PASSKEY_FACTORS } },
        }),
    )
    .await;
    let user_pool_id = created["UserPool"]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("a minted pool id: {created}"))
        .to_string();
    ok(
        &router,
        "SetUserPoolMfaConfig",
        &json!({
            "UserPoolId": &user_pool_id,
            "WebAuthnConfiguration": { "RelyingPartyId": RELYING_PARTY },
        }),
    )
    .await;

    let client_with_resources = app_client(&router, &user_pool_id, "with-resources").await;
    let bare_client = app_client(&router, &user_pool_id, "bare").await;

    let terms = ok(
        &router,
        "CreateTerms",
        &json!({
            "UserPoolId": &user_pool_id,
            "ClientId": &client_with_resources,
            "TermsName": TERMS_NAMES[0],
            // The only values `TermsSourceType` and `TermsEnforcementType` admit
            // (`smithy_query(query="shape",shape="TermsSourceType")`).
            "TermsSource": "LINK",
            "Enforcement": "NONE",
            "Links": links(),
        }),
    )
    .await;
    let terms_id = terms["Terms"]["TermsId"]
        .as_str()
        .unwrap_or_else(|| panic!("a minted TermsId: {terms}"))
        .to_string();

    let branding = ok(
        &router,
        "CreateManagedLoginBranding",
        &json!({
            "UserPoolId": &user_pool_id,
            "ClientId": &client_with_resources,
            "UseCognitoProvidedValues": true,
        }),
    )
    .await;
    let branding_id = branding["ManagedLoginBranding"]["ManagedLoginBrandingId"]
        .as_str()
        .unwrap_or_else(|| panic!("a minted ManagedLoginBrandingId: {branding}"))
        .to_string();

    ok(
        &router,
        "AdminCreateUser",
        &json!({ "UserPoolId": &user_pool_id, "Username": "ada" }),
    )
    .await;
    ok(
        &router,
        "AdminSetUserPassword",
        &json!({
            "UserPoolId": &user_pool_id,
            "Username": "ada",
            "Password": GOOD_PASSWORD,
            "Permanent": true,
        }),
    )
    .await;
    let authenticated = ok(
        &router,
        "InitiateAuth",
        &json!({
            "ClientId": &client_with_resources,
            "AuthFlow": "USER_PASSWORD_AUTH",
            "AuthParameters": { "USERNAME": "ada", "PASSWORD": GOOD_PASSWORD },
        }),
    )
    .await;
    let access_token = authenticated["AuthenticationResult"]["AccessToken"]
        .as_str()
        .unwrap_or_else(|| panic!("a sign-in issues an access token: {authenticated}"))
        .to_string();

    Fixture {
        router,
        user_pool_id,
        client_with_resources,
        bare_client,
        terms_id,
        branding_id,
        access_token,
    }
}

/// One row: a call this service refuses, and the error class it refuses it with.
struct Refusal {
    case: &'static str,
    operation: &'static str,
    body: Value,
    class: &'static str,
}

#[tokio::test]
async fn every_terms_branding_and_passkey_refusal_is_a_400_naming_its_error_shape() {
    let Fixture {
        router,
        user_pool_id,
        client_with_resources,
        bare_client,
        terms_id,
        branding_id,
        access_token,
    } = fixture("refusal-wire").await;

    let rows = vec![
        // A pool nothing created, and an id of a resource the pool does not hold, across
        // the delete, the update and the two by-client reads: the model gives this class
        // 404, the wire gives it 400.
        Refusal {
            case: "DeleteTerms for a terms id the pool does not hold",
            operation: "DeleteTerms",
            body: json!({ "UserPoolId": &user_pool_id, "TermsId": UNMINTED_TERMS }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "DeleteTerms on a pool nothing created",
            operation: "DeleteTerms",
            body: json!({ "UserPoolId": UNMINTED_POOL, "TermsId": &terms_id }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "UpdateTerms for a terms id the pool does not hold",
            operation: "UpdateTerms",
            body: json!({
                "UserPoolId": &user_pool_id,
                "TermsId": UNMINTED_TERMS,
                "Links": links(),
            }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "UpdateTerms on a pool nothing created",
            operation: "UpdateTerms",
            body: json!({
                "UserPoolId": UNMINTED_POOL,
                "TermsId": &terms_id,
                "Links": links(),
            }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "DescribeTermsByClient for the document type the app client holds none of",
            operation: "DescribeTermsByClient",
            body: json!({
                "UserPoolId": &user_pool_id,
                "ClientId": &client_with_resources,
                "TermsName": TERMS_NAMES[1],
            }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "DescribeTermsByClient for an app client holding no terms documents at all",
            operation: "DescribeTermsByClient",
            body: json!({
                "UserPoolId": &user_pool_id,
                "ClientId": &bare_client,
                "TermsName": TERMS_NAMES[0],
            }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "DescribeTermsByClient for an app client nothing minted",
            operation: "DescribeTermsByClient",
            body: json!({
                "UserPoolId": &user_pool_id,
                "ClientId": UNMINTED_CLIENT,
                "TermsName": TERMS_NAMES[0],
            }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "DescribeManagedLoginBrandingByClient for an app client with no branding",
            operation: "DescribeManagedLoginBrandingByClient",
            body: json!({ "UserPoolId": &user_pool_id, "ClientId": &bare_client }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "DescribeManagedLoginBrandingByClient on a pool nothing created",
            operation: "DescribeManagedLoginBrandingByClient",
            body: json!({ "UserPoolId": UNMINTED_POOL, "ClientId": &client_with_resources }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "UpdateManagedLoginBranding for a branding id the pool does not hold",
            operation: "UpdateManagedLoginBranding",
            body: json!({
                "UserPoolId": &user_pool_id,
                "ManagedLoginBrandingId": UNMINTED_BRANDING,
                "Settings": { "components": {} },
            }),
            class: "ResourceNotFoundException",
        },
        Refusal {
            case: "UpdateManagedLoginBranding on a pool nothing created",
            operation: "UpdateManagedLoginBranding",
            body: json!({
                "UserPoolId": UNMINTED_POOL,
                "ManagedLoginBrandingId": &branding_id,
                "Settings": { "components": {} },
            }),
            class: "ResourceNotFoundException",
        },
        // A registration ceremony addressed by a token no sign-in issued: modelled at 403.
        Refusal {
            case: "StartWebAuthnRegistration on a token no sign-in issued",
            operation: "StartWebAuthnRegistration",
            body: json!({ "AccessToken": UNISSUED_TOKEN }),
            class: "NotAuthorizedException",
        },
        Refusal {
            case: "CompleteWebAuthnRegistration on a token no sign-in issued",
            operation: "CompleteWebAuthnRegistration",
            body: json!({
                "AccessToken": UNISSUED_TOKEN,
                "Credential": registration_response(UNHELD_CHALLENGE),
            }),
            class: "NotAuthorizedException",
        },
        // Classes the model annotates 400 already, so only the header is new: a parameter
        // the request has to correct, and a challenge this emulator is not holding.
        Refusal {
            case: "UpdateManagedLoginBranding naming no branding to update",
            operation: "UpdateManagedLoginBranding",
            body: json!({
                "UserPoolId": &user_pool_id,
                "Settings": { "components": {} },
            }),
            class: "InvalidParameterException",
        },
        Refusal {
            case: "CompleteWebAuthnRegistration presenting a document that is no credential",
            operation: "CompleteWebAuthnRegistration",
            body: json!({ "AccessToken": &access_token, "Credential": "not-a-credential" }),
            class: "InvalidParameterException",
        },
        Refusal {
            case: "CompleteWebAuthnRegistration answering a challenge nothing handed out",
            operation: "CompleteWebAuthnRegistration",
            body: json!({
                "AccessToken": &access_token,
                "Credential": registration_response(UNHELD_CHALLENGE),
            }),
            class: "WebAuthnChallengeNotFoundException",
        },
        // And the one refusal no handler of these operations sees: a request breaking a
        // modelled rule, refused by the framework's own walk before any handler runs.
        Refusal {
            case: "DeleteTerms with no TermsId at all, which the model requires",
            operation: "DeleteTerms",
            body: json!({ "UserPoolId": &user_pool_id }),
            class: "InvalidParameterException",
        },
        Refusal {
            case: "DescribeTermsByClient with a document type the pattern excludes",
            operation: "DescribeTermsByClient",
            body: json!({
                "UserPoolId": &user_pool_id,
                "ClientId": &client_with_resources,
                "TermsName": "tos",
            }),
            class: "InvalidParameterException",
        },
    ];

    for Refusal {
        case,
        operation,
        body,
        class,
    } in rows
    {
        let answered = call(&router, operation, &body).await;
        assert_eq!(
            answered.status, REFUSED,
            "case {case:?}: a refusal of the caller is HTTP {REFUSED}, got {} with {}",
            answered.status, answered.payload
        );
        assert_eq!(
            error_code(&answered.payload),
            class,
            "case {case:?}: expected the body to name {class}, got {}",
            answered.payload
        );
        let expected = format!("{class}:");
        assert_eq!(
            answered.errortype.as_deref(),
            Some(expected.as_str()),
            "case {case:?}: expected {ERRORTYPE} {expected:?}, got {:?}",
            answered.errortype
        );
    }

    // None of the refusals above touched what this service holds: the terms document and
    // the branding style are still read back through their own app client.
    let terms = call(
        &router,
        "DescribeTermsByClient",
        &json!({
            "UserPoolId": &user_pool_id,
            "ClientId": &client_with_resources,
            "TermsName": TERMS_NAMES[0],
        }),
    )
    .await;
    assert_eq!(
        terms.payload["Terms"]["TermsId"],
        json!(terms_id),
        "a refused call must leave the stored terms documents in place, got {} with {}",
        terms.status,
        terms.payload
    );
    let branding = call(
        &router,
        "DescribeManagedLoginBrandingByClient",
        &json!({ "UserPoolId": &user_pool_id, "ClientId": &client_with_resources }),
    )
    .await;
    assert_eq!(
        branding.payload["ManagedLoginBranding"]["ManagedLoginBrandingId"],
        json!(branding_id),
        "a refused call must leave the stored branding style in place, got {} with {}",
        branding.status,
        branding.payload
    );
}

/// One row of the control: a call this service SERVES, the members its answer must carry
/// out of stored state, and the members it MINTS per call. Stating what the answer carries
/// is what keeps the row from passing against a handler that answers an empty 200.
struct Served {
    case: &'static str,
    operation: &'static str,
    body: Value,
    /// (JSON pointer, the value stored state makes it).
    carries: Vec<(&'static str, Value)>,
    /// JSON pointers that must carry a non-empty string the service mints per call, which
    /// no row can state the value of.
    mints: Vec<&'static str>,
}

/// The control: the same operations, against the resources this service HOLDS and with the
/// token a sign-in really issued, are served — they keep their own status, name no error
/// shape and answer the stored resource — so the 400s above are computed from state rather
/// than answered to every call of this surface.
#[tokio::test]
async fn the_same_calls_are_served_for_the_resources_this_service_holds() {
    let Fixture {
        router,
        user_pool_id,
        client_with_resources,
        bare_client: _,
        terms_id,
        branding_id,
        access_token,
    } = fixture("served-wire").await;

    // One call per operation the table above refuses on, in an order that ends on the
    // delete, so each earlier row still has its resource.
    let rows = vec![
        Served {
            case: "the terms documents read back through their app client",
            operation: "DescribeTermsByClient",
            body: json!({
                "UserPoolId": &user_pool_id,
                "ClientId": &client_with_resources,
                "TermsName": TERMS_NAMES[0],
            }),
            carries: vec![
                ("/Terms/TermsId", json!(terms_id)),
                ("/Terms/TermsName", json!(TERMS_NAMES[0])),
                (
                    "/Terms/Links/cognito:default",
                    json!("https://terms.example.com"),
                ),
            ],
            mints: Vec::new(),
        },
        Served {
            case: "the branding style read back through its app client",
            operation: "DescribeManagedLoginBrandingByClient",
            body: json!({ "UserPoolId": &user_pool_id, "ClientId": &client_with_resources }),
            carries: vec![
                (
                    "/ManagedLoginBranding/ManagedLoginBrandingId",
                    json!(branding_id),
                ),
                (
                    "/ManagedLoginBranding/UseCognitoProvidedValues",
                    json!(true),
                ),
            ],
            mints: Vec::new(),
        },
        Served {
            case: "the branding style updated by its own id",
            operation: "UpdateManagedLoginBranding",
            body: json!({
                "UserPoolId": &user_pool_id,
                "ManagedLoginBrandingId": &branding_id,
                "Settings": { "components": { "form": { "borderRadius": 4 } } },
            }),
            carries: vec![
                (
                    "/ManagedLoginBranding/ManagedLoginBrandingId",
                    json!(branding_id),
                ),
                (
                    "/ManagedLoginBranding/Settings/components/form/borderRadius",
                    json!(4),
                ),
            ],
            mints: Vec::new(),
        },
        Served {
            case: "a passkey registration started on the token its own sign-in issued",
            operation: "StartWebAuthnRegistration",
            body: json!({ "AccessToken": &access_token }),
            carries: vec![
                ("/CredentialCreationOptions/rp/id", json!(RELYING_PARTY)),
                ("/CredentialCreationOptions/user/name", json!("ada")),
            ],
            // The challenge is minted per registration, so the row states that one is
            // handed out rather than which one.
            mints: vec!["/CredentialCreationOptions/challenge"],
        },
        Served {
            case: "the terms documents updated by their own id",
            operation: "UpdateTerms",
            body: json!({
                "UserPoolId": &user_pool_id,
                "TermsId": &terms_id,
                "Links": { "cognito:english": "https://terms.example.com/en" },
            }),
            carries: vec![
                ("/Terms/TermsId", json!(terms_id)),
                (
                    "/Terms/Links/cognito:english",
                    json!("https://terms.example.com/en"),
                ),
            ],
            mints: Vec::new(),
        },
        Served {
            case: "the terms documents deleted by their own id",
            operation: "DeleteTerms",
            // "Deletes the terms documents with the requested ID from your app client"
            // (`DeleteTerms` in this service's model): the answer carries no members, so
            // the row states what it leaves behind below instead.
            body: json!({ "UserPoolId": &user_pool_id, "TermsId": &terms_id }),
            carries: Vec::new(),
            mints: Vec::new(),
        },
    ];

    for Served {
        case,
        operation,
        body,
        carries,
        mints,
    } in rows
    {
        let Answered {
            status,
            errortype,
            payload,
        } = call(&router, operation, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {payload}"
        );
        assert_eq!(
            errortype, None,
            "case {case:?}: a served call names no error shape, got {errortype:?}"
        );
        for (pointer, expected) in carries {
            let actual = payload.pointer(pointer);
            assert_eq!(
                actual,
                Some(&expected),
                "case {case:?}: expected {pointer} to be {expected}, got {actual:?} in {payload}"
            );
        }
        for pointer in mints {
            let minted = payload.pointer(pointer).and_then(Value::as_str);
            assert!(
                minted.is_some_and(|value| !value.is_empty()),
                "case {case:?}: expected {pointer} to carry a minted value, got {minted:?} in {payload}"
            );
        }
    }

    // And the delete the last row drove removed the documents it named: the read that
    // served them above is now the refusal the table asserts.
    let gone = call(
        &router,
        "DescribeTermsByClient",
        &json!({
            "UserPoolId": &user_pool_id,
            "ClientId": &client_with_resources,
            "TermsName": TERMS_NAMES[0],
        }),
    )
    .await;
    assert_eq!(
        (gone.status, error_code(&gone.payload)),
        (REFUSED, "ResourceNotFoundException"),
        "the deleted terms documents must no longer be found, got {} with {}",
        gone.status,
        gone.payload
    );
}
