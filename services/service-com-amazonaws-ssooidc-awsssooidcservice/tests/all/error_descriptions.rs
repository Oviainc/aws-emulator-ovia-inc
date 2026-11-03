// SPDX-License-Identifier: CC0-1.0

//! The `error_description` every refusal of this service carries, end to end over
//! the wire.
//!
//! RULE: a refusal's `error_description` is ONE FIXED sentence per refusal class —
//! short, sentence-case English naming the class and, where the class turns on a
//! single request member, that member in the API's own words — and it quotes nothing
//! from the request or from stored state. So the same class reads the same whatever
//! was sent, and a rejected credential is never echoed back.
//!
//! Every case drives the service through the router the production emulator serves,
//! using the harness the device-flow suite beside this file builds.

use serde_json::{json, Value};

use ssooidc::state::{
    DESCRIPTION_ASSERTION_REQUIRED, DESCRIPTION_AUTHORIZATION_CODE_REQUIRED,
    DESCRIPTION_DEVICE_CODE_REQUIRED, DESCRIPTION_INVALID_ASSERTION,
    DESCRIPTION_INVALID_AUTHORIZATION_CODE, DESCRIPTION_INVALID_CLIENT,
    DESCRIPTION_INVALID_CLIENT_ID, DESCRIPTION_INVALID_CLIENT_NAME,
    DESCRIPTION_INVALID_CLIENT_SECRET, DESCRIPTION_INVALID_CLIENT_TYPE,
    DESCRIPTION_INVALID_DEVICE_CODE, DESCRIPTION_INVALID_GRANT_TYPE,
    DESCRIPTION_INVALID_REFRESH_TOKEN, DESCRIPTION_INVALID_REQUEST,
    DESCRIPTION_INVALID_SUBJECT_TOKEN, DESCRIPTION_REFRESH_TOKEN_REQUIRED,
    DESCRIPTION_SUBJECT_TOKEN_REQUIRED, GRANT_TYPE_AUTHORIZATION_CODE, GRANT_TYPE_DEVICE_CODE,
    GRANT_TYPE_JWT_BEARER, GRANT_TYPE_REFRESH_TOKEN, GRANT_TYPE_TOKEN_EXCHANGE,
};
use service_com_amazonaws_ssooidc_awsssooidcservice as ssooidc;

use crate::device_authorization_flow::{assert_failure, start_url, text, Emulator, REGION};

/// The route `RegisterClient` is bound to.
const REGISTER_PATH: &str = "/client/register";
/// The route `CreateToken` is bound to.
const TOKEN_PATH: &str = "/token";
/// The route `CreateTokenWithIAM` is bound to: `CreateToken`'s path with the
/// `aws_iam=t` query literal the model's `@http` URI carries.
const IAM_TOKEN_PATH: &str = "/token?aws_iam=t";
/// The route `StartDeviceAuthorization` is bound to.
const DEVICE_AUTHORIZATION_PATH: &str = "/device_authorization";

/// A never-existing IAM Identity Center application ARN, for the `CreateTokenWithIAM`
/// cases: that operation's `clientId` "is an application ARN that has OAuth grants
/// configured".
const UNKNOWN_APPLICATION_ARN: &str =
    "arn:aws:sso::111122223333:application/ssoins-0000000000000000/apl-0000000000000000";

/// Another never-existing application ARN, so no row's text can be read off one ARN.
const OTHER_UNKNOWN_APPLICATION_ARN: &str =
    "arn:aws:sso::444455556666:application/ssoins-1111111111111111/apl-2222222222222222";

/// One refused request and the whole refusal it must answer with.
struct Row {
    case: &'static str,
    path: &'static str,
    body: Value,
    status: u16,
    shape: &'static str,
    error: &'static str,
    description: &'static str,
}

#[allow(clippy::too_many_arguments)]
fn row(
    case: &'static str,
    path: &'static str,
    body: Value,
    status: u16,
    shape: &'static str,
    error: &'static str,
    description: &'static str,
) -> Row {
    Row {
        case,
        path,
        body,
        status,
        shape,
        error,
        description,
    }
}

/// Every string the request carried, so a refusal's text can be checked against all
/// of them.
fn request_strings(body: &Value) -> Vec<String> {
    match body {
        Value::String(value) => vec![value.clone()],
        Value::Array(values) => values.iter().flat_map(request_strings).collect(),
        Value::Object(members) => members.values().flat_map(request_strings).collect(),
        _ => Vec::new(),
    }
}

/// The second half of the rule: the text quotes nothing the request carried. Values
/// shorter than three characters are left out — a one- or two-character fragment says
/// nothing about whether the text was built from the request.
fn assert_quotes_nothing(case: &str, body: &Value, description: &str) {
    for value in request_strings(body)
        .into_iter()
        .filter(|value| value.chars().count() >= 3)
    {
        assert!(
            !description.contains(&value),
            "case {case:?}: the text must quote nothing the request carried, \
             but {description:?} carries {value:?}"
        );
    }
}

/// The `error_description` of a refusal body.
fn description(case: &str, body: &Value) -> String {
    body["error_description"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an `error_description` in {body}"))
        .to_string()
}

/// Assert a refusal in full: its status, modeled shape and `error` code, the fixed
/// text of its class, and that the text quotes nothing the request carried.
async fn assert_refusal(emulator: &Emulator, row: &Row) {
    let (status, body) = emulator.post(row.path, REGION, &row.body).await;
    assert_failure(row.case, status, &body, row.status, row.shape, row.error);
    let actual = description(row.case, &body);
    assert_eq!(
        actual, row.description,
        "case {:?}: expected error_description {:?}, got {actual:?}",
        row.case, row.description
    );
    assert_quotes_nothing(row.case, &row.body, &actual);
}

/// RULE (as above) across every refusal class the four operations answer. The table
/// varies what the rule generalises over: the operation, the refusal class, the
/// member the class turns on, and the VALUE of that member — its length, its
/// alphabet, its case, whether it is absent, empty or merely unresolvable — and
/// every row must answer the same sentence for its class.
#[tokio::test]
async fn every_refusal_answers_the_fixed_description_of_its_class() {
    let emulator = Emulator::new();
    let (client_id, client_secret) = emulator.registered_client(json!({})).await;

    let rows = vec![
        // RegisterClient: the client metadata it refuses.
        row(
            "a confidential clientType",
            REGISTER_PATH,
            json!({"clientName": "demo", "clientType": "confidential"}),
            400,
            "InvalidClientMetadataException",
            "invalid_client_metadata",
            DESCRIPTION_INVALID_CLIENT_TYPE,
        ),
        row(
            "a clientType in another case",
            REGISTER_PATH,
            json!({"clientName": "demo", "clientType": "Public"}),
            400,
            "InvalidClientMetadataException",
            "invalid_client_metadata",
            DESCRIPTION_INVALID_CLIENT_TYPE,
        ),
        row(
            "a clientType naming another OAuth client class",
            REGISTER_PATH,
            json!({"clientName": "demo", "clientType": "native-application"}),
            400,
            "InvalidClientMetadataException",
            "invalid_client_metadata",
            DESCRIPTION_INVALID_CLIENT_TYPE,
        ),
        row(
            "an empty clientName",
            REGISTER_PATH,
            json!({"clientName": "", "clientType": "public"}),
            400,
            "InvalidClientMetadataException",
            "invalid_client_metadata",
            DESCRIPTION_INVALID_CLIENT_NAME,
        ),
        row(
            "a clientName of blanks",
            REGISTER_PATH,
            json!({"clientName": "\t  ", "clientType": "public"}),
            400,
            "InvalidClientMetadataException",
            "invalid_client_metadata",
            DESCRIPTION_INVALID_CLIENT_NAME,
        ),
        row(
            "a grantTypes entry this service does not serve",
            REGISTER_PATH,
            json!({
                "clientName": "demo",
                "clientType": "public",
                "grantTypes": ["client_credentials"],
            }),
            400,
            "UnsupportedGrantTypeException",
            "unsupported_grant_type",
            DESCRIPTION_INVALID_GRANT_TYPE,
        ),
        row(
            "an unserved grantTypes entry beside a served one",
            REGISTER_PATH,
            json!({
                "clientName": "demo",
                "clientType": "public",
                "grantTypes": [GRANT_TYPE_DEVICE_CODE, "urn:ietf:params:oauth:grant-type:saml2-bearer"],
            }),
            400,
            "UnsupportedGrantTypeException",
            "unsupported_grant_type",
            DESCRIPTION_INVALID_GRANT_TYPE,
        ),
        // CreateToken: the member each grant needs, then the grant it resolves.
        row(
            "the device-code grant with no deviceCode",
            TOKEN_PATH,
            json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": GRANT_TYPE_DEVICE_CODE,
            }),
            400,
            "InvalidRequestException",
            "invalid_request",
            DESCRIPTION_DEVICE_CODE_REQUIRED,
        ),
        row(
            "the device-code grant with an empty deviceCode",
            TOKEN_PATH,
            json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": GRANT_TYPE_DEVICE_CODE,
                "deviceCode": "",
            }),
            400,
            "InvalidRequestException",
            "invalid_request",
            DESCRIPTION_DEVICE_CODE_REQUIRED,
        ),
        row(
            "a deviceCode this service never minted",
            TOKEN_PATH,
            json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": GRANT_TYPE_DEVICE_CODE,
                "deviceCode": "fabricated-device-code-nothing-issued",
            }),
            400,
            "InvalidGrantException",
            "invalid_grant",
            DESCRIPTION_INVALID_DEVICE_CODE,
        ),
        row(
            "a deviceCode shaped like one this service mints",
            TOKEN_PATH,
            json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": GRANT_TYPE_DEVICE_CODE,
                "deviceCode": "b".repeat(64),
            }),
            400,
            "InvalidGrantException",
            "invalid_grant",
            DESCRIPTION_INVALID_DEVICE_CODE,
        ),
        row(
            "the refresh-token grant with no refreshToken",
            TOKEN_PATH,
            json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": GRANT_TYPE_REFRESH_TOKEN,
            }),
            400,
            "InvalidRequestException",
            "invalid_request",
            DESCRIPTION_REFRESH_TOKEN_REQUIRED,
        ),
        row(
            "the refresh-token grant with an empty refreshToken",
            TOKEN_PATH,
            json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": GRANT_TYPE_REFRESH_TOKEN,
                "refreshToken": "",
            }),
            400,
            "InvalidRequestException",
            "invalid_request",
            DESCRIPTION_REFRESH_TOKEN_REQUIRED,
        ),
        row(
            "a refreshToken this service never issued",
            TOKEN_PATH,
            json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": GRANT_TYPE_REFRESH_TOKEN,
                "refreshToken": "fabricated-refresh-token-nothing-issued",
            }),
            400,
            "InvalidGrantException",
            "invalid_grant",
            DESCRIPTION_INVALID_REFRESH_TOKEN,
        ),
        // The grant is resolved against this service's own records before any
        // credential is read, so an unregistered client presenting an unknown
        // refresh token learns about the token it presented.
        row(
            "a refreshToken presented by a clientId that was never registered",
            TOKEN_PATH,
            json!({
                "clientId": "client-that-was-never-registered",
                "clientSecret": "secret-that-was-never-issued",
                "grantType": GRANT_TYPE_REFRESH_TOKEN,
                "refreshToken": "another-refresh-token-nothing-issued",
            }),
            400,
            "InvalidGrantException",
            "invalid_grant",
            DESCRIPTION_INVALID_REFRESH_TOKEN,
        ),
        row(
            "the authorization-code grant with no code",
            TOKEN_PATH,
            json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": GRANT_TYPE_AUTHORIZATION_CODE,
            }),
            400,
            "InvalidRequestException",
            "invalid_request",
            DESCRIPTION_AUTHORIZATION_CODE_REQUIRED,
        ),
        row(
            "an authorization code no endpoint of this API mints",
            TOKEN_PATH,
            json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": GRANT_TYPE_AUTHORIZATION_CODE,
                "code": "aGVsbG8td29ybGQ",
                "codeVerifier": "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
            }),
            400,
            "InvalidGrantException",
            "invalid_grant",
            DESCRIPTION_INVALID_AUTHORIZATION_CODE,
        ),
        // CreateTokenWithIAM: the grant is resolved before the application ARN, so
        // an unknown ARN with an unknown grant is refused for the grant.
        row(
            "an unknown application ARN with a refreshToken this service never issued",
            IAM_TOKEN_PATH,
            json!({
                "clientId": UNKNOWN_APPLICATION_ARN,
                "grantType": GRANT_TYPE_REFRESH_TOKEN,
                "refreshToken": "fabricated-refresh-token-for-an-application",
            }),
            400,
            "InvalidGrantException",
            "invalid_grant",
            DESCRIPTION_INVALID_REFRESH_TOKEN,
        ),
        row(
            "another unknown application ARN with another unknown refreshToken",
            IAM_TOKEN_PATH,
            json!({
                "clientId": OTHER_UNKNOWN_APPLICATION_ARN,
                "grantType": GRANT_TYPE_REFRESH_TOKEN,
                "refreshToken": "f".repeat(64),
            }),
            400,
            "InvalidGrantException",
            "invalid_grant",
            DESCRIPTION_INVALID_REFRESH_TOKEN,
        ),
        row(
            "the IAM refresh-token grant with no refreshToken",
            IAM_TOKEN_PATH,
            json!({
                "clientId": UNKNOWN_APPLICATION_ARN,
                "grantType": GRANT_TYPE_REFRESH_TOKEN,
            }),
            400,
            "InvalidRequestException",
            "invalid_request",
            DESCRIPTION_REFRESH_TOKEN_REQUIRED,
        ),
        row(
            "the token-exchange grant with no subjectToken",
            IAM_TOKEN_PATH,
            json!({
                "clientId": UNKNOWN_APPLICATION_ARN,
                "grantType": GRANT_TYPE_TOKEN_EXCHANGE,
            }),
            400,
            "InvalidRequestException",
            "invalid_request",
            DESCRIPTION_SUBJECT_TOKEN_REQUIRED,
        ),
        row(
            "a subjectToken this service never issued",
            IAM_TOKEN_PATH,
            json!({
                "clientId": UNKNOWN_APPLICATION_ARN,
                "grantType": GRANT_TYPE_TOKEN_EXCHANGE,
                "subjectToken": "fabricated-subject-token-nothing-issued",
            }),
            400,
            "InvalidGrantException",
            "invalid_grant",
            DESCRIPTION_INVALID_SUBJECT_TOKEN,
        ),
        row(
            "the JWT-bearer grant with no assertion",
            IAM_TOKEN_PATH,
            json!({
                "clientId": UNKNOWN_APPLICATION_ARN,
                "grantType": GRANT_TYPE_JWT_BEARER,
            }),
            400,
            "InvalidRequestException",
            "invalid_request",
            DESCRIPTION_ASSERTION_REQUIRED,
        ),
        row(
            "an assertion from an issuer this service does not trust",
            IAM_TOKEN_PATH,
            json!({
                "clientId": UNKNOWN_APPLICATION_ARN,
                "grantType": GRANT_TYPE_JWT_BEARER,
                "assertion": "eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJ1c2VyIn0.c2ln",
            }),
            400,
            "InvalidGrantException",
            "invalid_grant",
            DESCRIPTION_INVALID_ASSERTION,
        ),
        row(
            "the IAM authorization-code grant with no code",
            IAM_TOKEN_PATH,
            json!({
                "clientId": UNKNOWN_APPLICATION_ARN,
                "grantType": GRANT_TYPE_AUTHORIZATION_CODE,
            }),
            400,
            "InvalidRequestException",
            "invalid_request",
            DESCRIPTION_AUTHORIZATION_CODE_REQUIRED,
        ),
        row(
            "an IAM authorization code no endpoint of this API mints",
            IAM_TOKEN_PATH,
            json!({
                "clientId": UNKNOWN_APPLICATION_ARN,
                "grantType": GRANT_TYPE_AUTHORIZATION_CODE,
                "code": "c29tZS1vdGhlci1jb2Rl",
            }),
            400,
            "InvalidGrantException",
            "invalid_grant",
            DESCRIPTION_INVALID_AUTHORIZATION_CODE,
        ),
        row(
            "a grantType the IAM operation does not serve",
            IAM_TOKEN_PATH,
            json!({
                "clientId": UNKNOWN_APPLICATION_ARN,
                "grantType": GRANT_TYPE_DEVICE_CODE,
            }),
            401,
            "InvalidClientException",
            "invalid_client",
            DESCRIPTION_INVALID_CLIENT,
        ),
        // StartDeviceAuthorization: the request itself, then the pair it presents.
        row(
            "a startUrl that is no URI at all",
            DEVICE_AUTHORIZATION_PATH,
            json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "startUrl": "my-sso-portal",
            }),
            400,
            "InvalidRequestException",
            "invalid_request",
            DESCRIPTION_INVALID_REQUEST,
        ),
        row(
            "a startUrl that is a bare path",
            DEVICE_AUTHORIZATION_PATH,
            json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "startUrl": "/start",
            }),
            400,
            "InvalidRequestException",
            "invalid_request",
            DESCRIPTION_INVALID_REQUEST,
        ),
        row(
            "a clientId that was never registered",
            DEVICE_AUTHORIZATION_PATH,
            json!({
                "clientId": "client-id-that-was-never-registered",
                "clientSecret": "client-secret-that-was-never-issued",
                "startUrl": start_url(),
            }),
            401,
            "InvalidClientException",
            "invalid_client",
            DESCRIPTION_INVALID_CLIENT,
        ),
    ];

    for row in &rows {
        assert_refusal(&emulator, row).await;
    }
}

/// RULE: an `invalid_client` refusal names the member of the `clientId`/`clientSecret`
/// pair that is at fault — the pair when no registration carries the id, the SECRET
/// when the secret is no registration's own, and the ID when the secret presented is
/// another registration's own — and the registration's own pair still authenticates.
/// The table varies which operation reads the pair, which member is wrong, and the
/// form of the wrong value.
#[tokio::test]
async fn an_invalid_client_refusal_names_the_member_of_the_pair_at_fault() {
    let emulator = Emulator::new();
    let (first_id, first_secret) = emulator
        .registered_client(json!({"clientName": "first-client"}))
        .await;
    let (second_id, second_secret) = emulator
        .registered_client(json!({"clientName": "second-client"}))
        .await;
    // A device authorization of the first registration, so a `CreateToken` poll gets
    // past grant resolution and reaches the pair.
    let (status, started) = emulator
        .start_device_authorization(&json!({
            "clientId": first_id,
            "clientSecret": first_secret,
            "startUrl": start_url(),
        }))
        .await;
    assert_eq!(status, 200, "StartDeviceAuthorization answered {started}");
    let device_code = text(&started, "deviceCode");

    let poll = |secret: &str| {
        json!({
            "clientId": first_id,
            "clientSecret": secret,
            "grantType": GRANT_TYPE_DEVICE_CODE,
            "deviceCode": device_code,
        })
    };
    let start = |client_id: &str, secret: &str| {
        json!({"clientId": client_id, "clientSecret": secret, "startUrl": start_url()})
    };

    let rows = vec![
        row(
            "StartDeviceAuthorization with a short garbage secret",
            DEVICE_AUTHORIZATION_PATH,
            start(&first_id, "junk"),
            401,
            "InvalidClientException",
            "invalid_client",
            DESCRIPTION_INVALID_CLIENT_SECRET,
        ),
        row(
            "StartDeviceAuthorization with a garbage secret shaped like a minted one",
            DEVICE_AUTHORIZATION_PATH,
            start(&first_id, &"c".repeat(64)),
            401,
            "InvalidClientException",
            "invalid_client",
            DESCRIPTION_INVALID_CLIENT_SECRET,
        ),
        row(
            "StartDeviceAuthorization with another registration's own secret",
            DEVICE_AUTHORIZATION_PATH,
            start(&first_id, &second_secret),
            401,
            "InvalidClientException",
            "invalid_client",
            DESCRIPTION_INVALID_CLIENT_ID,
        ),
        row(
            "StartDeviceAuthorization pairing the second id with the first secret",
            DEVICE_AUTHORIZATION_PATH,
            start(&second_id, &first_secret),
            401,
            "InvalidClientException",
            "invalid_client",
            DESCRIPTION_INVALID_CLIENT_ID,
        ),
        row(
            "StartDeviceAuthorization with an unregistered id and a valid secret",
            DEVICE_AUTHORIZATION_PATH,
            start("client-id-no-registration-carries", &first_secret),
            401,
            "InvalidClientException",
            "invalid_client",
            DESCRIPTION_INVALID_CLIENT,
        ),
        row(
            "a CreateToken poll with a garbage secret",
            TOKEN_PATH,
            poll("not-this-registrations-secret"),
            401,
            "InvalidClientException",
            "invalid_client",
            DESCRIPTION_INVALID_CLIENT_SECRET,
        ),
        row(
            "a CreateToken poll with another registration's own secret",
            TOKEN_PATH,
            poll(&second_secret),
            401,
            "InvalidClientException",
            "invalid_client",
            DESCRIPTION_INVALID_CLIENT_ID,
        ),
    ];

    for row in &rows {
        assert_refusal(&emulator, row).await;
    }

    // The registration's own pair still mints a device authorization: the refusals
    // above narrow nothing about the positive path.
    let (status, authorized) = emulator
        .start_device_authorization(&start(&first_id, &first_secret))
        .await;
    assert_eq!(
        status, 200,
        "the registration's own pair must authenticate, got {authorized}"
    );
    let user_code = text(&authorized, "userCode");
    assert!(
        !text(&authorized, "deviceCode").is_empty() && !user_code.is_empty(),
        "the accepted pair must mint a deviceCode and a userCode, got {authorized}"
    );
    assert!(
        authorized["expiresIn"].as_i64().unwrap_or_default() > 0,
        "the minted authorization must carry a positive expiresIn, got {authorized}"
    );
    assert!(
        text(&authorized, "verificationUriComplete").ends_with(&user_code),
        "verificationUriComplete must end in the userCode, got {authorized}"
    );
}

/// RULE as an invariant: the text of a class is FIXED, so EVERY rejected `deviceCode`
/// reads the same sentence however it is built. The codes are generated rather than
/// listed — each length crossed with each alphabet — and none of them is a code this
/// service minted.
#[tokio::test]
async fn an_unresolvable_device_code_reads_the_same_whatever_the_code() {
    let emulator = Emulator::new();
    let (client_id, client_secret) = emulator.registered_client(json!({})).await;

    // Lengths around the 64 characters this service's own opaque values carry, and
    // alphabets a client could draw a code from.
    let lengths = [1_usize, 2, 7, 16, 63, 64, 65, 512];
    let alphabets = [
        "0123456789abcdef",
        "ABCDEFGHJKLMNPQRSTUVWXYZ",
        "-_.~%+/=",
    ];
    for length in lengths {
        for alphabet in alphabets {
            let code: String = alphabet.chars().cycle().take(length).collect();
            let case = format!("a {length}-character code over {alphabet:?}");
            let body = json!({
                "clientId": client_id,
                "clientSecret": client_secret,
                "grantType": GRANT_TYPE_DEVICE_CODE,
                "deviceCode": code,
            });
            let (status, response) = emulator.post(TOKEN_PATH, REGION, &body).await;
            assert_failure(
                &case,
                status,
                &response,
                400,
                "InvalidGrantException",
                "invalid_grant",
            );
            let actual = description(&case, &response);
            assert_eq!(
                actual, DESCRIPTION_INVALID_DEVICE_CODE,
                "case {case:?}: expected error_description \
                 {DESCRIPTION_INVALID_DEVICE_CODE:?}, got {actual:?}"
            );
            assert_quotes_nothing(&case, &body, &actual);
        }
    }
}
