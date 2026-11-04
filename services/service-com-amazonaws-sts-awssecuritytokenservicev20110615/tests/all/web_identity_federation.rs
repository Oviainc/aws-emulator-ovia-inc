// SPDX-License-Identifier: CC0-1.0

//! `AssumeRoleWithWebIdentity` is reachable UNSIGNED, and what it answers is decided by
//! the token the request presents.
//!
//! RULE (routing): a request that names NO service — no SigV4 credential scope, no
//! `X-Amz-Target`, and a host that names no endpoint — is claimed by the service whose own
//! modelled wire form it addresses; for an `awsQuery` service that form is a `POST` of an
//! `application/x-www-form-urlencoded` body to the root path. That is how an operation the
//! model marks anonymous (`auth: []`, `optionalAuth`: `AssumeRoleWithWebIdentity`,
//! `AssumeRoleWithSAML`) is served to a client holding no credential, and it is answered in
//! the service's own protocol — an `awsQuery` XML `ErrorResponse` — rather than in a JSON
//! envelope such a client cannot parse.
//!
//! RULE (the token): the presented web identity is classified by the token itself, in the
//! order a verifier decides it — a value that is not a readable JSON Web Token (or carries
//! no expiry or issuer claim) is `InvalidIdentityToken` / HTTP 400; a token whose expiry has
//! passed is `ExpiredTokenException` / HTTP 400; a token NOTHING signed is
//! `InvalidIdentityToken` saying unsigned JWTs are not accepted; a token naming an issuer the
//! account registered no provider for is `InvalidIdentityToken` naming that issuer; and a
//! signed token of a resolvable issuer is verified against the key set that issuer publishes
//! — which for an issuer OUTSIDE this emulator cannot be retrieved at all, the one class the
//! key-retrieval message is worded for. `web_identity_verification.rs` owns the rows where
//! the key set IS readable, because the issuer is a user pool this emulator serves.
//!
//! The rows differ along what those rules generalise over: for routing, which signal the
//! request carries and which path and method it uses; for the token, its segment structure,
//! its header, its algorithm, its size and its expiry claim (past, exactly now, future). The
//! recorded cases — a non-JWT token, an expired one and a forged one — are three rows.

use super::sts_sessions::{element, emulator, form_value, member, STS_API_VERSION};
use smithy_server_runtime::MultiServiceRouter;

/// The loopback authority a client pointed at this emulator addresses it on. It names no
/// service, which is the whole point: the host tier of the framework's routing resolves
/// nothing from it, exactly as it resolves nothing from a bare IP or `localhost`.
const EMULATOR_AUTHORITY: &str = "127.0.0.1:9297";

/// The media type an `awsQuery` body is posted as.
const QUERY_CONTENT_TYPE: &str = "application/x-www-form-urlencoded";

/// One UNSIGNED awsQuery call: the request a client with no Amazon Web Services credential
/// makes, carrying no `Authorization` header at all and addressed only by the emulator's
/// own authority.
pub(crate) async fn unsigned_call(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(&str, String)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={STS_API_VERSION}");
    for (name, value) in params {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_value(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{EMULATOR_AUTHORITY}/"))
        .header("host", EMULATOR_AUTHORITY)
        .header("content-type", QUERY_CONTENT_TYPE)
        .body(bytes::Bytes::from(body))
        .expect("a well-formed awsQuery request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One base64url segment, unpadded, as a compact JWS carries it (RFC 7515 §2).
pub(crate) fn segment(value: &str) -> String {
    aws_smithy_types::base64::encode(value)
        .trim_end_matches('=')
        .replace('+', "-")
        .replace('/', "_")
}

/// A compact JWS of `header` and `claims` with a non-empty signature segment: the shape a
/// token signed by some issuer has, whoever signed it.
fn token(header: &str, claims: &str) -> String {
    format!(
        "{}.{}.{}",
        segment(header),
        segment(claims),
        segment("a-signature-this-emulator-cannot-check")
    )
}

/// Seconds since the Unix epoch, the unit a token's `exp` claim is written in.
fn now_secs() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .expect("the clock is after the Unix epoch")
            .as_secs(),
    )
    .expect("the current time fits in an i64")
}

/// A role ARN of this account. Well-formed, and naming no role the account holds: the
/// token is decided BEFORE the role is, so every row below answers the token's refusal
/// whatever the role is.
fn role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/web-identity-probe",
        engine::emulator_account_id()
    )
}

/// How AWS words the refusal of a value that is not a readable identity token.
const UNREADABLE_TOKEN_MESSAGE: &str =
    "The ID Token provided is not a valid JWT. (You may see this error if you sent an \
     Access Token)";

/// How AWS words the refusal of a token NOTHING signed: RFC 7519 §6's unsecured JWT.
const UNSIGNED_TOKEN_MESSAGE: &str = "We don't accept unsigned JWTs";

/// How AWS words the refusal of a token whose signature it could not check.
const UNVERIFIABLE_TOKEN_MESSAGE: &str =
    "Couldn't retrieve verification key from your identity provider, please reference \
     AssumeRoleWithWebIdentity documentation for requirements";

/// The issuer every Amazon Cognito identity pool's tokens name, which an account can
/// federate without registering an OpenID Connect provider for it.
const COGNITO_ISSUER: &str = "https://cognito-identity.amazonaws.com";

/// An issuer this account has registered no OpenID Connect provider for.
const UNREGISTERED_ISSUER: &str = "https://accounts.google.com";

/// How AWS words the refusal of a token whose issuer the account cannot resolve at all.
fn no_provider_message(issuer: &str) -> String {
    format!("No OpenIDConnect provider found in your account for {issuer}")
}

/// An unsigned `AssumeRoleWithWebIdentity` is served by STS and classified by its token:
/// never a JSON `UnknownService`, never a 501, and never credentials for a token nothing
/// verified.
#[tokio::test]
async fn an_unsigned_assume_role_with_web_identity_is_classified_by_its_token() {
    let (router, _engine) = emulator();
    let expired_at = now_secs() - 60;
    let valid_header = r#"{"alg":"RS256","typ":"JWT","kid":"key-1"}"#;
    // The claim set of an unexpired token of `issuer`.
    let unexpired = |issuer: &str| {
        format!(
            r#"{{"iss":"{issuer}","sub":"user-1","exp":{}}}"#,
            now_secs() + 3600
        )
    };

    // One row: the case, the presented `WebIdentityToken`, and the modelled failure the
    // token earns — its `<Code>` and the exact message AWS words for that class.
    let rows: Vec<(&str, String, &str, String)> = vec![
        (
            "not a token at all",
            "not-a-jwt".to_string(),
            "InvalidIdentityToken",
            UNREADABLE_TOKEN_MESSAGE.to_string(),
        ),
        (
            "two segments only",
            format!("{}.{}", segment(valid_header), segment("{}")),
            "InvalidIdentityToken",
            UNREADABLE_TOKEN_MESSAGE.to_string(),
        ),
        (
            "a header that is not base64url JSON",
            format!("@@@@.{}.{}", segment("{}"), segment("sig")),
            "InvalidIdentityToken",
            UNREADABLE_TOKEN_MESSAGE.to_string(),
        ),
        (
            // RFC 7519 §6's unsecured JWT: nothing signed it, which is its own refusal and
            // not the one for a key that could not be read.
            "the unsecured algorithm, unexpired, from the Cognito issuer",
            format!(
                "{}.{}.",
                segment(r#"{"alg":"none","typ":"JWT"}"#),
                segment(&unexpired(COGNITO_ISSUER))
            ),
            "InvalidIdentityToken",
            UNSIGNED_TOKEN_MESSAGE.to_string(),
        ),
        (
            // The expiry decides before the signature does: an expired token is expired
            // whether or not anything signed it.
            "the unsecured algorithm, expired",
            format!(
                "{}.{}.",
                segment(r#"{"alg":"none","typ":"JWT"}"#),
                segment(&format!(
                    r#"{{"iss":"{COGNITO_ISSUER}","exp":{expired_at}}}"#
                ))
            ),
            "ExpiredTokenException",
            format!("must be before the expiration date/time {expired_at}"),
        ),
        (
            // An empty signature segment is the same unsecured form: nothing signed it,
            // whatever its header names.
            "an empty signature segment, unexpired, from the Cognito issuer",
            format!(
                "{}.{}.",
                segment(valid_header),
                segment(&unexpired(COGNITO_ISSUER))
            ),
            "InvalidIdentityToken",
            UNSIGNED_TOKEN_MESSAGE.to_string(),
        ),
        (
            // An OpenID Connect ID token is required to carry an expiry.
            "no expiry claim",
            token(
                valid_header,
                &format!(r#"{{"iss":"{COGNITO_ISSUER}","sub":"user-1"}}"#),
            ),
            "InvalidIdentityToken",
            UNREADABLE_TOKEN_MESSAGE.to_string(),
        ),
        (
            // An ID token is required to name its issuer too.
            "no issuer claim",
            token(valid_header, &format!(r#"{{"exp":{}}}"#, now_secs() + 3600)),
            "InvalidIdentityToken",
            UNREADABLE_TOKEN_MESSAGE.to_string(),
        ),
        (
            "an expiry that has passed",
            token(
                valid_header,
                &format!(r#"{{"iss":"{COGNITO_ISSUER}","exp":{expired_at}}}"#),
            ),
            "ExpiredTokenException",
            // The clause the instants are read out of below; the current instant is the
            // emulator's clock at the moment it served the call.
            format!("must be before the expiration date/time {expired_at}"),
        ),
        (
            // RFC 7519 §2: "non-integer values can be used" for a NumericDate, so a
            // fractional expiry is a valid one and is read in whole seconds.
            "a fractional expiry that has passed",
            token(
                valid_header,
                &format!(r#"{{"iss":"{COGNITO_ISSUER}","exp":{expired_at}.515625}}"#),
            ),
            "ExpiredTokenException",
            format!("must be before the expiration date/time {expired_at}"),
        ),
        (
            "a fractional expiry still in the future",
            token(
                valid_header,
                &format!(
                    r#"{{"iss":"{COGNITO_ISSUER}","exp":{}.25}}"#,
                    now_secs() + 3600
                ),
            ),
            "InvalidIdentityToken",
            UNVERIFIABLE_TOKEN_MESSAGE.to_string(),
        ),
        (
            // Not a NumericDate at all, so the token carries no readable expiry.
            "an expiry written as a string",
            token(
                valid_header,
                &format!(r#"{{"iss":"{COGNITO_ISSUER}","exp":"{expired_at}"}}"#),
            ),
            "InvalidIdentityToken",
            UNREADABLE_TOKEN_MESSAGE.to_string(),
        ),
        (
            "an expiry of this very second",
            token(
                valid_header,
                &format!(r#"{{"iss":"{COGNITO_ISSUER}","exp":{}}}"#, now_secs()),
            ),
            "ExpiredTokenException",
            format!("must be before the expiration date/time {}", now_secs()),
        ),
        (
            // The account registered no provider for this issuer, so there is no provider
            // whose keys or condition keys the token could be read against.
            "an issuer the account has registered no provider for",
            token(valid_header, &unexpired(UNREGISTERED_ISSUER)),
            "InvalidIdentityToken",
            no_provider_message(UNREGISTERED_ISSUER),
        ),
        (
            "another unregistered issuer",
            token(valid_header, &unexpired("https://login.example.test/oauth2")),
            "InvalidIdentityToken",
            no_provider_message("https://login.example.test/oauth2"),
        ),
        (
            "a signature nothing here can verify, from the Cognito issuer",
            token(valid_header, &unexpired(COGNITO_ISSUER)),
            "InvalidIdentityToken",
            UNVERIFIABLE_TOKEN_MESSAGE.to_string(),
        ),
        (
            // Another registered algorithm: the rule is not keyed on one of them.
            "an ES256 signature nothing here can verify",
            token(r#"{"alg":"ES256","typ":"JWT"}"#, &unexpired(COGNITO_ISSUER)),
            "InvalidIdentityToken",
            UNVERIFIABLE_TOKEN_MESSAGE.to_string(),
        ),
        (
            // Inside the modelled 4..20000 length, past the bound the shared token
            // reader accepts, so it is not read at all.
            "a token too large to read",
            format!(
                "{}.{}.{}",
                segment(valid_header),
                "a".repeat(9_000),
                segment("sig")
            ),
            "InvalidIdentityToken",
            UNREADABLE_TOKEN_MESSAGE.to_string(),
        ),
    ];

    for (case, presented, expected_code, expected_message) in rows {
        let (status, body) = unsigned_call(
            &router,
            "AssumeRoleWithWebIdentity",
            &[
                ("RoleArn", role_arn()),
                ("RoleSessionName", "probe".to_string()),
                ("WebIdentityToken", presented),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some(expected_code),
            "case {case:?}: expected <Code>{expected_code}</Code>, got {body}"
        );
        let message = member(case, &body, "Message");
        // The element's text as the client reads it: XML requires `&` escaped inside a
        // text node, so a sentence carrying one arrives encoded and an awsQuery client
        // decodes it before it ever reaches application code. An apostrophe is not markup
        // there and arrives as itself; `refusal_message_bytes.rs` owns that rule.
        let message = message.replace("&amp;", "&");
        if expected_code == "ExpiredTokenException" {
            assert!(
                message.starts_with("Token expired: current date/time ")
                    && message.ends_with(&expected_message),
                "case {case:?}: expected a message naming both instants and ending \
                 {expected_message:?}, got {message:?}"
            );
        } else {
            assert_eq!(
                message, expected_message,
                "case {case:?}: expected the message AWS words for this class, got {message:?}"
            );
        }
        assert!(
            element(&body, "Credentials").is_none(),
            "case {case:?}: a refused token must vend no credential, got {body}"
        );
        assert!(
            body.contains("<ErrorResponse"),
            "case {case:?}: expected an awsQuery XML ErrorResponse, got {body}"
        );
    }
}

/// A request naming no service is claimed by the wire form it addresses, and nothing
/// weaker is: this service answers the unsigned `awsQuery` POST its anonymous operations
/// arrive as, and a request addressed any other way still reaches nobody
/// (`404 UnknownService`).
#[tokio::test]
async fn an_unattributed_request_is_claimed_by_the_wire_form_it_addresses() {
    let (router, _engine) = emulator();

    // One row: the case, how the request is addressed (method, path, content type), and
    // the status this service's own answer carries. `None` for the content type is a
    // request that declares no body type at all.
    let rows: Vec<(&str, &str, &str, Option<&str>, u16)> = vec![
        (
            "the unsigned awsQuery POST this service's anonymous operations arrive as",
            "POST",
            "/",
            Some(QUERY_CONTENT_TYPE),
            400,
        ),
        (
            "the same POST with a charset on the media type",
            "POST",
            "/",
            Some("application/x-www-form-urlencoded; charset=utf-8"),
            400,
        ),
        // Addressed in a way no awsQuery request is: claimed by nobody, which the router
        // answers 404 rather than letting this service answer for an endpoint it does
        // not serve.
        ("a GET of the root path", "GET", "/", Some(QUERY_CONTENT_TYPE), 404),
        (
            "a POST of a path no awsQuery operation uses",
            "POST",
            "/2011-06-15/assume-role",
            Some(QUERY_CONTENT_TYPE),
            404,
        ),
        ("a POST declaring a JSON body", "POST", "/", Some("application/json"), 404),
        ("a POST declaring no body type", "POST", "/", None, 404),
    ];

    let body = format!(
        "Action=AssumeRoleWithWebIdentity&Version={STS_API_VERSION}&RoleArn={}\
         &RoleSessionName=probe&WebIdentityToken=not-a-jwt",
        form_value(&role_arn())
    );
    for (case, method, path, content_type, expected_status) in rows {
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://{EMULATOR_AUTHORITY}{path}"))
            .header("host", EMULATOR_AUTHORITY);
        if let Some(content_type) = content_type {
            builder = builder.header("content-type", content_type);
        }
        let request = builder
            .body(bytes::Bytes::from(body.clone()))
            .expect("a well-formed request");
        let response = router.dispatch(request).await;
        let status = response.status().as_u16();
        let answered = String::from_utf8_lossy(response.body()).to_string();
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {answered}"
        );
        if expected_status == 404 {
            assert!(
                answered.contains("UnknownService"),
                "case {case:?}: expected the router's own UnknownService, got {answered}"
            );
        } else {
            assert_eq!(
                element(&answered, "Code").as_deref(),
                Some("InvalidIdentityToken"),
                "case {case:?}: expected this service's own modelled refusal, got {answered}"
            );
        }
    }
}

/// An unsigned request naming an action this service's model does not declare is refused
/// in this service's OWN protocol — the awsQuery `InvalidAction` — and not with a JSON
/// envelope an awsQuery client cannot parse.
#[tokio::test]
async fn an_unattributed_request_naming_no_modelled_action_is_refused_in_this_protocol() {
    let (router, _engine) = emulator();
    for action in ["NotAnStsAction", "AssumeRoleWithWebIdentityButNot"] {
        let (status, body) = unsigned_call(&router, action, &[]).await;
        assert_eq!(
            status, 400,
            "case {action:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some("InvalidAction"),
            "case {action:?}: expected the awsQuery InvalidAction refusal, got {body}"
        );
    }
}
