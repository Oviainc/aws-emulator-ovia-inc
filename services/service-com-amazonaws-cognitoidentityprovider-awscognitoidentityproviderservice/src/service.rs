// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! EDIT THIS FILE. It is emitted once with working defaults and is never
//! regenerated, so a choice made here survives adding another service or
//! re-running the generator.
//!
//! `EmulatedService` is what the router mounts (`EmulatedService::new(engine)`).
//! It wraps the generated [`ServiceDefinition`], which carries the model-derived half
//! of this service — its identity, its wire protocol, and its operation routing
//! table — built from the Smithy model at compile time. You do not edit the
//! entry; you edit the behavioral seams below.
//!
//! Five opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name, which this
//!   service widens to the two names ONE awsJson endpoint answers on (see the method).
//!   Edit it for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired. This
//!   service answers two such requests itself: a GET of a pool's
//!   `/<UserPoolId>/.well-known/jwks.json`, the public key set its tokens are
//!   verified against, and a GET of a level's
//!   `/<UserPoolId>/<ClientId>/assets/login_logo.png`, the classic hosted UI logo
//!   image `SetUICustomization` stored — neither of which the Smithy model
//!   describes.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The `com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The shared engine, so the non-API routes a user-pool endpoint serves
    /// (the pool's JSON Web Key Set and a level's hosted logo image, see
    /// [`EmulatedService::override_routing_error`]) can be answered from this
    /// service's stored state. Operations reach the engine through the entry
    /// that constructed them, not through this field.
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            engine,
        }
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // This service speaks `awsJson1_1` and several of its operations are called
        // with NO SigV4 credentials — `InitiateAuth`, `SignUp`, `ConfirmSignUp`,
        // `ForgotPassword`, `RespondToAuthChallenge` and the rest of the sign-in
        // surface carry `@optionalAuth`, since a client has no credentials until it
        // has signed in. The framework resolves a routing key from the SigV4
        // credential scope FIRST and from `X-Amz-Target` only when there is no scope,
        // and those two carry different names: the scope carries the signing name
        // (`cognito-idp`), the target carries the model's service shape name
        // (`AWSCognitoIdentityProviderService.<Operation>`). Claiming the signing name
        // alone therefore serves every SIGNED call and leaves every unsigned one
        // unclaimed, which the router answers `UnknownService` — the service is
        // mounted, but nothing claims the request.
        //
        // Both names address this one endpoint, so both are its routing keys, and the
        // framework's own resolver still decides which one a given request resolves
        // to: a request signed for ANOTHER service resolves to that service's signing
        // name and is not claimed here however it is addressed. This service's signing
        // name is its own, so the claim needs no further discriminator.
        //
        // Both halves are read with the framework's OWN readers
        // (`emulator_server`), which is all this service needs: its published target
        // prefix is a single Smithy identifier, so the segment before the first dot
        // that `service_from_target` reads IS the service shape's name. A service whose
        // published prefix is a fully qualified Coral name carries dots of its own and
        // needs `service_common::rpc_target`'s reading of the LAST prefix segment
        // instead — that is what the CloudTrail and CodeConnections crates claim
        // through, and where this rule belongs for them.
        routes_to_signing_name(parts, self.entry.identity().signing_name())
            || unsigned_target_names_shape(parts, self.entry.identity().shape_id())
            // The hosted UI logo image this service hands out a locator for
            // (`UICustomizationType.ImageUrl`) is fetched by an anonymous
            // browser, with no credential scope and no `X-Amz-Target` to name
            // this service by — only the path this service itself published. A
            // GET of exactly that path is therefore claimed here, so the image
            // is reachable at the URL this service reported; every other path
            // still needs one of the two routing keys above.
            || (parts.method == http::Method::GET
                && hosted_logo_route(&parts.uri.to_string()).is_some())
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        let _ = req;
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // Amazon Cognito states a request that breaks a modelled rule — an
        // absent required member, a value outside an enumerated set, a length or
        // range violation — as `InvalidParameterException`, the 400 every
        // operation of this service declares for a bad parameter (OPERATIONS.md
        // lists `InvalidParameterException (HTTP 400)` on each one). The
        // framework's generic `ValidationException` is not a code this service
        // models, and a client branches on the code, so only the code changes
        // here: the message stays the framework's own rendering of the
        // violations, which already names the member and the constraint it
        // missed.
        match refusal {
            Refusal::Invalid(violations) => Some(Refused {
                code: "InvalidParameterException".to_string(),
                message: ::aws_smithy_schema_server::constraints::envelope(violations)
                    .unwrap_or_else(|| "invalid request".to_string()),
            }),
            // An unauthenticated caller is the emulator's own refusal, not this
            // service's, so it keeps the framework's wording.
            _ => None,
        }
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation. A user-pool endpoint serves TWO routes this
        // service's Smithy model does not describe. The first is the pool's JSON
        // Web Key Set at `/<UserPoolId>/.well-known/jwks.json`, which every
        // verifier of a Cognito token fetches to check the token's signature,
        // under the algorithm the key set itself names (Amazon
        // Cognito Developer Guide, "Verifying a JSON web token"). It carries
        // only PUBLIC key parameters and AWS serves it unauthenticated, which
        // is what this seam gives: the router consults it before it
        // authenticates anything, so an anonymous GET reaches the document the
        // same way it does on AWS. The second is the classic hosted UI logo
        // image of one level of a pool, which `UICustomizationType.ImageUrl`
        // locates for an anonymous browser to fetch. Every other unrouted
        // request keeps the framework's protocol-correct refusal.
        if req.method() != "GET" {
            return None;
        }
        let state = crate::state::service_state(&*self.engine)?;
        if let Some((user_pool_id, client_id)) = hosted_logo_route(req.uri()) {
            // A path naming no stored image is not this route: it falls through
            // to the framework's refusal rather than answering an empty body.
            // Rendering the STORED bytes is a pure read.
            let image = {
                let guard = state.lock();
                guard.pool(user_pool_id)?.hosted_logo(client_id)?.to_vec()
            };
            let media_type = logo_media_type(&image);
            let mut response = aws_smithy_runtime_api::http::Response::new(
                aws_smithy_runtime_api::http::StatusCode::try_from(200).ok()?,
                ::aws_smithy_types::body::SdkBody::from(image),
            );
            response.headers_mut().insert("content-type", media_type);
            return Some(response);
        }
        let user_pool_id = jwks_route_pool_id(req.uri())?.to_string();
        let document = {
            let guard = state.lock();
            // A path naming no stored pool is not this route: it falls through
            // to the framework's refusal rather than answering an empty key
            // set. Rendering the stored key is a pure read — no key is
            // generated here, and nothing is written.
            guard.pool(&user_pool_id)?.jwks()
        };
        let mut response = aws_smithy_runtime_api::http::Response::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(200).ok()?,
            ::aws_smithy_types::body::SdkBody::from(document.to_string()),
        );
        response
            .headers_mut()
            .insert("content-type", "application/json");
        Some(response)
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps the entry's protocol-correct
        // rendering. Return `Some(resp)` to reword that failure in your own
        // envelope. The error is borrowed, so the default renderer can still
        // consume it on `None`.
        let _ = error;
        None
    }
}

/// The `UserPoolId` whose key set `uri` addresses, for a request path of the
/// form `/<UserPoolId>/.well-known/jwks.json`, and `None` for every other path.
///
/// This is the only non-modelled route a user-pool endpoint serves; the pool id
/// is the path segment before it, exactly as the `iss` claim of a token this
/// service issued states it (`<endpoint>/<UserPoolId>`).
fn jwks_route_pool_id(uri: &str) -> Option<&str> {
    let path = uri.split(['?', '#']).next().unwrap_or(uri);
    let (user_pool_id, tail) = path.strip_prefix('/')?.split_once('/')?;
    (!user_pool_id.is_empty() && tail == ".well-known/jwks.json").then_some(user_pool_id)
}

/// The pool and the level whose hosted UI logo image `uri` addresses, for a
/// request path of the form `/<UserPoolId>/<ClientId>/assets/login_logo.png`,
/// and `None` for every other path.
///
/// This is the path [`crate::state::hosted_logo_url`] builds — the two read the
/// same [`crate::state::HOSTED_LOGO_FILE`], so a locator this service hands out
/// and the route that serves it cannot drift. Both segments have to have the
/// FORM this service mints them in, so a path of the same shape addressed to
/// another mounted service (a path-style object GET, say) is not mistaken for a
/// locator this service published.
fn hosted_logo_route(uri: &str) -> Option<(&str, &str)> {
    let path = uri.split(['?', '#']).next().unwrap_or(uri);
    let (user_pool_id, tail) = path.strip_prefix('/')?.split_once('/')?;
    let (client_id, file) = tail.split_once('/')?;
    (file == crate::state::HOSTED_LOGO_FILE
        && is_user_pool_id_shaped(user_pool_id)
        && is_client_id_shaped(client_id))
    .then_some((user_pool_id, client_id))
}

/// Whether `candidate` has the form of a `UserPoolIdType`: the modelled pattern
/// `^[\w-]+_[0-9a-zA-Z]+$` and length 1..55 (this service's model). The suffix
/// carries no underscore, so the LAST one separates the two halves.
fn is_user_pool_id_shaped(candidate: &str) -> bool {
    let Some((prefix, suffix)) = candidate.rsplit_once('_') else {
        return false;
    };
    candidate.len() <= 55
        && !prefix.is_empty()
        && !suffix.is_empty()
        && prefix
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// Whether `candidate` has the form of a `ClientIdType`: the modelled pattern
/// `^[\w+]+$` and length 1..128 (this service's model), which the pool-level
/// default [`crate::state::UI_CUSTOMIZATION_ALL_CLIENTS`] also satisfies.
fn is_client_id_shaped(candidate: &str) -> bool {
    !candidate.is_empty()
        && candidate.len() <= 128
        && candidate
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'+'))
}

/// The media type a stored logo image is served as, read from the image's own
/// leading bytes: the file formats a browser renders a login logo in each
/// announce themselves there (PNG, RFC 2083 §3.1; JPEG, ISO/IEC 10918-1;
/// GIF87a and GIF89a; the RIFF container WebP rides in). Bytes that announce
/// none of them are served as unnamed binary rather than as an image they are
/// not.
fn logo_media_type(image: &[u8]) -> &'static str {
    if image.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if image.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if image.starts_with(b"GIF87a") || image.starts_with(b"GIF89a") {
        "image/gif"
    } else if image.starts_with(b"RIFF") && image.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else {
        "application/octet-stream"
    }
}

/// Whether the routing key the framework resolves for `parts` is `signing_name`.
///
/// This is `emulator_server::can_handle_based_on_route_key`'s rule applied over
/// `Parts`: that helper takes a whole buffered `http::Request`, which the
/// `Service::can_handle` seam does not receive. Comparison is case-insensitive on a
/// trimmed name, as there. It answers for every SIGNED request (the credential scope
/// carries the signing name) and for one addressed by this service's endpoint host.
fn routes_to_signing_name(parts: &http::request::Parts, signing_name: &str) -> bool {
    smithy_server_runtime::resolve_route_key(parts)
        .is_some_and(|key| key.eq_ignore_ascii_case(signing_name.trim()))
}

/// Whether `parts` carries NO SigV4 credential scope, names the service whose shape id
/// is `service_shape_id` in its `X-Amz-Target`, and arrives on the path this service's
/// protocol uses.
///
/// The three conjuncts, each for its own reason:
///
/// * NO credential scope — neither an `Authorization` header nor a presigned
///   `X-Amz-Credential` query parameter. The framework resolves a routing key from the
///   scope FIRST (`emulator_server::resolve_route_key`), so a request signed for another
///   service resolves to that service and must not be claimed here however it is
///   targeted; reading the target only in the scope's absence keeps the framework's
///   precedence instead of competing with it.
/// * the target NAMES this service — read with the framework's own
///   `service_from_target`, which takes the segment before the first dot and normalises
///   it the way a routing key is normalised. This service's published target prefix is a
///   single Smithy identifier (`AWSCognitoIdentityProviderService`), so that segment IS
///   the shape's local name; the comparison is therefore against the shape id the
///   generated identity states, with its namespace dropped, and nothing is written out
///   here that the model does not state.
/// * the ROOT path, which is the only path an `awsJson` operation posts to. A request
///   that names this service out of band while addressing some other model's REST path
///   contradicts itself, so it must reach NOBODY rather than a handler picked off the
///   target alone.
fn unsigned_target_names_shape(parts: &http::request::Parts, service_shape_id: &str) -> bool {
    if emulator_server::service_from_sigv4(&parts.headers).is_some()
        || emulator_server::service_from_sigv4_query(&parts.uri).is_some()
    {
        return false;
    }
    let shape_name = service_shape_id
        .split('#')
        .next_back()
        .unwrap_or(service_shape_id)
        .trim();
    emulator_server::service_from_target(&parts.headers)
        .is_some_and(|named| named.eq_ignore_ascii_case(shape_name))
        && parts.uri.path().trim_matches('/').is_empty()
}

#[cfg(test)]
mod tests {
    //! The two names this ONE `awsJson` endpoint answers on, over the signals a request
    //! can carry, beside the two non-modelled paths this endpoint serves a document and
    //! an image on. `tests/all/initiate_auth.rs` asserts the routing rule on the wire,
    //! through the router, `tests/all/token_signing.rs` fetches the key set over it and
    //! `tests/all/ui_customization.rs` fetches a stored logo over it; these tables are
    //! the rules themselves, row by row.

    use super::{
        hosted_logo_route, jwks_route_pool_id, logo_media_type, routes_to_signing_name,
        unsigned_target_names_shape,
    };

    /// [`hosted_logo_route`] over the paths that reach this service: the locator
    /// this service publishes, and the paths that are not it — including one of
    /// the same SHAPE whose segments are not the form this service mints, which
    /// another mounted service may own.
    #[test]
    fn only_a_published_logo_locator_names_a_pool_and_a_level() {
        /// One row: a path that reaches this service, and the pool and level it
        /// addresses a logo of, if any.
        struct Row {
            case: &'static str,
            path: &'static str,
            expected: Option<(&'static str, &'static str)>,
        }

        let rows = [
            Row {
                case: "the pool-level default's logo",
                path: "/us-east-1_abc123def/ALL/assets/login_logo.png",
                expected: Some(("us-east-1_abc123def", "ALL")),
            },
            Row {
                case: "an app client's own logo",
                path: "/us-west-2_0a9b8c7d6/1h57kf5cpq17m0eml12eu8b4cg/assets/login_logo.png",
                expected: Some(("us-west-2_0a9b8c7d6", "1h57kf5cpq17m0eml12eu8b4cg")),
            },
            Row {
                case: "a logo with a cache-busting query",
                path: "/us-east-1_abc123def/ALL/assets/login_logo.png?v=2",
                expected: Some(("us-east-1_abc123def", "ALL")),
            },
            Row {
                case: "a path-style object of the same shape, whose first segment is no pool id",
                path: "/my-bucket/branding/assets/login_logo.png",
                expected: None,
            },
            Row {
                case: "a level whose id is outside ClientIdType's pattern",
                path: "/us-east-1_abc123def/not a client/assets/login_logo.png",
                expected: None,
            },
            Row {
                case: "another asset of the same level",
                path: "/us-east-1_abc123def/ALL/assets/background.png",
                expected: None,
            },
            Row {
                case: "the pool's key set, which is the other non-modelled route",
                path: "/us-east-1_abc123def/.well-known/jwks.json",
                expected: None,
            },
            Row {
                case: "the root path, where every modelled call goes",
                path: "/",
                expected: None,
            },
            Row {
                case: "a logo path with no leading slash",
                path: "us-east-1_abc123def/ALL/assets/login_logo.png",
                expected: None,
            },
        ];

        for row in rows {
            let actual = hosted_logo_route(row.path);
            assert_eq!(
                actual, row.expected,
                "case {:?} ({}): expected {:?}, got {actual:?}",
                row.case, row.path, row.expected
            );
        }
    }

    /// [`logo_media_type`] over the leading bytes each image format announces
    /// itself with, and over bytes that announce none.
    #[test]
    fn a_logo_is_served_as_the_format_its_own_bytes_announce() {
        // (case, leading bytes, expected media type)
        let rows: &[(&str, &[u8], &str)] = &[
            ("a PNG", b"\x89PNG\r\n\x1a\n rest", "image/png"),
            ("a JPEG", b"\xff\xd8\xff\xe0 rest", "image/jpeg"),
            ("a GIF87a", b"GIF87a rest", "image/gif"),
            ("a GIF89a", b"GIF89a rest", "image/gif"),
            ("a WebP", b"RIFF\x10\x00\x00\x00WEBPVP8 ", "image/webp"),
            (
                "a RIFF container that is not WebP",
                b"RIFF\x10\x00\x00\x00WAVEfmt ",
                "application/octet-stream",
            ),
            (
                "markup, which is never served as markup",
                b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
                "application/octet-stream",
            ),
            ("bytes too short to announce anything", b"\x89P", "application/octet-stream"),
        ];

        for (case, image, expected) in rows {
            let actual = logo_media_type(image);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// [`jwks_route_pool_id`] over the paths that reach this service: the key-set
    /// route, and the paths that are not it.
    #[test]
    fn only_a_pools_well_known_key_set_path_names_a_pool() {
        // (case, path, expected)
        let rows: &[(&str, &str, Option<&str>)] = &[
            (
                "a pool's key set",
                "/us-east-1_abc123def/.well-known/jwks.json",
                Some("us-east-1_abc123def"),
            ),
            (
                "a pool's key set with a query string",
                "/us-west-2_0a9b8c7d6/.well-known/jwks.json?cachebust=1",
                Some("us-west-2_0a9b8c7d6"),
            ),
            ("the root path, where every modelled call goes", "/", None),
            (
                "another well-known document",
                "/us-east-1_abc123def/.well-known/openid-configuration",
                None,
            ),
            ("a key set with no pool id", "//.well-known/jwks.json", None),
            (
                "a deeper path under a pool",
                "/us-east-1_abc123def/oauth2/token",
                None,
            ),
            ("a path with no leading slash", "jwks.json", None),
        ];

        for (case, path, expected) in rows {
            let actual = jwks_route_pool_id(path);
            assert_eq!(
                actual, *expected,
                "case {case:?} ({path}): expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// This service's SigV4 signing name and Smithy shape id, as the generated identity
    /// states them (`smithy_query(query="service_info")`: `sigv4Name` `cognito-idp`,
    /// service shape `com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService`).
    const SIGNING_NAME: &str = "cognito-idp";
    const SHAPE_ID: &str = "com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService";

    /// A credential scope naming `signing_name`, the primary routing signal.
    fn authorization(signing_name: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/{signing_name}/aws4_request, \
             SignedHeaders=host, Signature=abc"
        )
    }

    /// One row: how the request is addressed — its credential scope, its `X-Amz-Target`,
    /// its `Host` and its request URI, each absent as `None` — and whether this service
    /// claims it.
    struct Row {
        case: &'static str,
        scope: Option<&'static str>,
        target: Option<&'static str>,
        host: Option<&'static str>,
        uri: &'static str,
        expected: bool,
    }

    /// The path every `awsJson` operation posts to.
    const ROOT: &str = "/";

    #[test]
    fn this_endpoint_answers_on_its_signing_name_and_on_its_shape_name() {
        let rows = [
            Row {
                case: "signed for it, targeted at it",
                scope: Some(SIGNING_NAME),
                target: Some("AWSCognitoIdentityProviderService.AdminGetUser"),
                host: None,
                uri: ROOT,
                expected: true,
            },
            Row {
                // The unsigned sign-in call this rule exists for: no credential scope,
                // so the target is all that names the service.
                case: "unsigned, targeted at it",
                scope: None,
                target: Some("AWSCognitoIdentityProviderService.InitiateAuth"),
                host: None,
                uri: ROOT,
                expected: true,
            },
            Row {
                case: "unsigned, targeted at it, on the emulator's own host",
                scope: None,
                target: Some("AWSCognitoIdentityProviderService.InitiateAuth"),
                host: Some("127.0.0.1:9297"),
                uri: ROOT,
                expected: true,
            },
            Row {
                // The target tier outranks the `Host` tier in the framework's own
                // resolver, so the peer that host names resolves the same
                // target-derived key and does not claim this request either.
                case: "unsigned, targeted at it, on another service's endpoint host",
                scope: None,
                target: Some("AWSCognitoIdentityProviderService.InitiateAuth"),
                host: Some("lambda.us-east-1.amazonaws.com"),
                uri: ROOT,
                expected: true,
            },
            Row {
                case: "unsigned, lower-cased target prefix",
                scope: None,
                target: Some("awscognitoidentityproviderservice.InitiateAuth"),
                host: None,
                uri: ROOT,
                expected: true,
            },
            Row {
                case: "unsigned, targeted at the separate Cognito Identity service",
                scope: None,
                target: Some("AWSCognitoIdentityService.GetId"),
                host: None,
                uri: ROOT,
                expected: false,
            },
            Row {
                // The credential scope outranks the target, so a request signed for
                // another service does not reach this one.
                case: "signed for another service, targeted at it",
                scope: Some("lambda"),
                target: Some("AWSCognitoIdentityProviderService.InitiateAuth"),
                host: None,
                uri: ROOT,
                expected: false,
            },
            Row {
                // A request that names this service out of band while addressing
                // another model's REST path contradicts itself: it reaches nobody.
                case: "unsigned, targeted at it, on a REST path",
                scope: None,
                target: Some("AWSCognitoIdentityProviderService.InitiateAuth"),
                host: None,
                uri: "/identitypools/pool-1",
                expected: false,
            },
            Row {
                case: "unsigned, addressed by its endpoint host",
                scope: None,
                target: None,
                host: Some("cognito-idp.us-east-1.amazonaws.com"),
                uri: ROOT,
                expected: true,
            },
            Row {
                case: "no routing signal at all",
                scope: None,
                target: None,
                host: None,
                uri: ROOT,
                expected: false,
            },
        ];

        for row in rows {
            let mut builder = super::http::Request::builder()
                .method("POST")
                .uri(row.uri);
            if let Some(scope) = row.scope {
                builder = builder.header("authorization", authorization(scope));
            }
            if let Some(target) = row.target {
                builder = builder.header("x-amz-target", target);
            }
            if let Some(host) = row.host {
                builder = builder.header("host", host);
            }
            let parts = builder
                .body(())
                .expect("a well-formed test request")
                .into_parts()
                .0;
            let actual = routes_to_signing_name(&parts, SIGNING_NAME)
                || unsigned_target_names_shape(&parts, SHAPE_ID);
            assert_eq!(
                actual, row.expected,
                "case {:?}: expected this service to claim it == {}, got {actual}",
                row.case, row.expected
            );
        }
    }
}
