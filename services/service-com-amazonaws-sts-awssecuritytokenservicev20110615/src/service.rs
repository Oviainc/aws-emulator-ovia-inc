// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-sts-awssecuritytokenservicev20110615` service: how it claims requests and how it words the
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
//!   service widens to the unsigned `awsQuery` request its anonymous operations are
//!   sent as (see the method). Edit it for anything the model does not express: a
//!   signing name shared by several services, or routing that keys on the
//!   access-key id.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
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
use std::cell::RefCell;
use std::sync::Arc;

/// The error code AWS answers a request whose signing credential it cannot resolve
/// with: an access key id that was never issued, or one that has been deleted. It is
/// distinct from an authorization denial — the call failed before any principal was
/// found — and is the code an SDK's credential-refresh path keys on.
const UNRESOLVED_CREDENTIAL_CODE: &str = "InvalidClientTokenId";

/// How AWS words that refusal.
const UNRESOLVED_CREDENTIAL_MESSAGE: &str =
    "The security token included in the request is invalid.";

/// The error code AWS answers a request whose access key id DOES resolve but whose
/// signature does not verify under that credential's secret: the credential was found,
/// so the caller is known; what failed is the signature.
const SIGNATURE_MISMATCH_CODE: &str = "SignatureDoesNotMatch";

/// How the SigV4 signing layer words that refusal — the fixed sentence it produces for
/// this one condition, which names the half of the credential pair that is wrong.
const SIGNATURE_MISMATCH_MESSAGE: &str =
    "The request signature we calculated does not match the signature you provided. Check \
     your AWS Secret Access Key and signing method. Consult the service documentation for \
     details.";

/// The media type an `awsQuery` request carries its form-encoded body in, which is the
/// only body this service's protocol reads.
const QUERY_CONTENT_TYPE: &str = "application/x-www-form-urlencoded";

thread_local! {
    /// The access key id the request this service is serving presented, for the length of
    /// that one request.
    ///
    /// Not state: a request-scoped note, needed because the two seams that decide an
    /// authentication refusal see different halves of it. [`Service::can_handle`] is
    /// handed the request and records the key id here; [`Service::override_refusal`] is
    /// handed only the STRUCTURAL refusal — `Refusal::Unauthenticated`, which says that
    /// authentication refused the caller and nothing about WHICH credential it refused —
    /// and has to choose between the two refusals AWS words differently for it. The
    /// framework calls both within one synchronous stretch of the one dispatch, on the
    /// thread that dispatch is running on, so the value a refusal reads is the one the
    /// claim of that same request recorded.
    static PRESENTED_ACCESS_KEY_ID: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Record the access key id `parts` presented as the one this thread is serving.
fn remember_presented_access_key_id(parts: &http::request::Parts) {
    let access_key_id = ::engine::extract_akid(&parts.headers);
    PRESENTED_ACCESS_KEY_ID.with(|presented| {
        *presented.borrow_mut() = Some(access_key_id);
    });
}

/// The access key id recorded by [`remember_presented_access_key_id`] for the request
/// this thread is serving, TAKEN: the note is request-scoped, so reading it is what ends
/// it. A later refusal on this thread that was not preceded by this service's own claim
/// then reads no credential at all rather than the previous request's.
fn presented_access_key_id() -> Option<String> {
    PRESENTED_ACCESS_KEY_ID.with(|presented| presented.borrow_mut().take())
}

/// Whether `parts` addresses this service's own wire form while naming NO service at all.
///
/// Every `awsQuery` operation is a `POST` of an `application/x-www-form-urlencoded` body
/// to the ROOT path, with the operation in the body's `Action` field — the protocol
/// publishes no `smithy.api#http` binding and no `X-Amz-Target`, so that is the whole
/// signature such a request has.
///
/// It is the signature a request has to be claimed by when nothing else in it names a
/// service: this service's `AssumeRoleWithWebIdentity` and `AssumeRoleWithSAML` are
/// modelled anonymous (`auth: []`, `optionalAuth`), so a conforming client sends them
/// UNSIGNED — no SigV4 credential scope to resolve a routing key from — and a client
/// pointed at this emulator's own endpoint (a loopback address, which names no service)
/// leaves the host tier nothing to resolve either. Claiming by the signing name alone
/// therefore serves every signed call and leaves the anonymous ones unclaimed, which the
/// router answers `UnknownService` in a JSON envelope an `awsQuery` client cannot even
/// parse.
///
/// Nothing weaker is claimed: a request that resolves to SOME service's routing key is
/// that service's however it is addressed, and a request on any other path or method, or
/// carrying any other body type, reaches nobody exactly as before.
fn addresses_unattributed_query_request(parts: &http::request::Parts) -> bool {
    if ::emulator_server::resolve_route_key(parts).is_some() {
        // SOMETHING in the request names a service — a SigV4 credential scope, an
        // `X-Amz-Target`, or an endpoint host — and it is not this service's name (the
        // claim's first conjunct asked that already). That request is its own service's,
        // however it is addressed.
        return false;
    }
    let form_encoded = parts
        .headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .unwrap_or(value)
                .trim()
                .eq_ignore_ascii_case(QUERY_CONTENT_TYPE)
        });
    parts.method == http::Method::POST
        && parts.uri.path().trim_matches('/').is_empty()
        && form_encoded
}

/// The `com-amazonaws-sts-awssecuritytokenservicev20110615` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The SAME shared engine the generated entry threaded into every operation, kept
    /// here too because the refusal wording below has to ask whether the account holds
    /// the credential a refused request presented, and the records that answer that are
    /// the ones the handlers read ([`Self::account_holds_credential`]). Never a store of
    /// its own: it is the `Arc` the router handed to `new`.
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

    /// Whether `access_key_id` names a credential this emulator ISSUED, and so one an
    /// authentication refusal has RESOLVED: the key pair the process publishes at start,
    /// an IAM user's access key, or a session this service minted.
    ///
    /// That is the whole set of access key ids that resolve to a principal here, each read
    /// from the records that own it rather than from a copy — so a key deleted by
    /// `iam:DeleteAccessKey`, or a session that expired, stops resolving at the moment its
    /// own record goes.
    fn account_holds_credential(&self, access_key_id: &str) -> bool {
        if ::emulator_server::request_authentication::SigV4Authenticator::minted_credentials()
            .is_some_and(|published| published.access_key_id == access_key_id)
        {
            return true;
        }
        if crate::state::iam_state(&*self.engine)
            .is_some_and(|iam| iam.lock().access_keys.contains_key(access_key_id))
        {
            return true;
        }
        crate::state::service_state(&*self.engine)
            .is_some_and(|sts| sts.lock().session(access_key_id).is_some())
    }

    /// How this service words a request authentication refused, which is TWO refusals AWS
    /// words differently:
    ///
    /// * a key id that resolves to no credential at all — never issued, or deleted — is
    ///   `InvalidClientTokenId`, the authentication failure an SDK's credential-refresh
    ///   path keys on; and
    /// * a key id the emulator DOES hold is `SignatureDoesNotMatch`: the credential was
    ///   found and the signature computed under its secret did not match the one
    ///   presented, which points the operator at the secret half of the pair and not at
    ///   the key id.
    ///
    /// Which of the two it is follows from the credential the request presented
    /// ([`Self::account_holds_credential`]), because the framework's structural refusal
    /// does not carry it.
    fn unauthenticated_refusal(&self) -> Refused {
        let resolves = presented_access_key_id()
            .is_some_and(|access_key_id| self.account_holds_credential(&access_key_id));
        if resolves {
            return Refused {
                code: SIGNATURE_MISMATCH_CODE.to_string(),
                message: SIGNATURE_MISMATCH_MESSAGE.to_string(),
            };
        }
        Refused {
            code: UNRESOLVED_CREDENTIAL_CODE.to_string(),
            message: UNRESOLVED_CREDENTIAL_MESSAGE.to_string(),
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
        // Claim a request whose resolved route key is this service's signing name, and —
        // when NOTHING in the request names a service — one that addresses this
        // service's own `awsQuery` wire form, which is what makes the operations the
        // model marks anonymous reachable from a client holding no credential (see
        // [`addresses_unattributed_query_request`]).
        let claimed = ::service_common::routes_to(parts, self.entry.identity().signing_name())
            || addresses_unattributed_query_request(parts);
        if claimed {
            // The credential this request presented, kept for the length of the request
            // so an authentication refusal can be worded for the credential it refused;
            // see [`PRESENTED_ACCESS_KEY_ID`].
            remember_presented_access_key_id(parts);
        }
        claimed
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
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Returning `None` keeps the framework's default
        // wording.
        match refusal {
            // AWS answers a request authentication refused with ONE of two codes, and
            // which one it is depends on the credential the request presented, not on
            // the structure of the refusal — see [`Self::unauthenticated_refusal`]. The
            // framework collapses every pre-dispatch authentication refusal into one
            // structural case, so this service states the two cases the way AWS states
            // them.
            Refusal::Unauthenticated => Some(self.unauthenticated_refusal()),
            // A refusal of the request's VALUES is stated the way AWS states one: the
            // framework's own envelope and member paths, with the code STS answers a
            // constraint violation with, and a `@pattern` quoted back WITHOUT the
            // anchors a whole-value pattern is written with — the tree's one rendering
            // of that rule. The CHECK still uses the model's pattern as written; only
            // the quoting changes.
            Refusal::Invalid(_) => {
                ::service_common::validation_message::aws_validation_refusal(refusal).map(
                    |stated| Refused {
                        code: crate::scaffolded::VALIDATION_ERROR_CODE.to_string(),
                        message: stated.message,
                    },
                )
            }
            _ => None,
        }
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
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

#[cfg(test)]
mod tests {
    //! The two rules this file owns, each over the signals a request can carry: WHICH
    //! requests this service claims, and HOW it words a request authentication refused.
    //! The served behaviour is pinned over the wire by `tests/all/web_identity_federation.rs`;
    //! these tables are the rules themselves, row by row.

    use super::{
        addresses_unattributed_query_request, remember_presented_access_key_id, EmulatedService,
        QUERY_CONTENT_TYPE, SIGNATURE_MISMATCH_CODE, SIGNATURE_MISMATCH_MESSAGE,
        UNRESOLVED_CREDENTIAL_CODE, UNRESOLVED_CREDENTIAL_MESSAGE,
    };
    use crate::state::{Identity, Session, SessionKind};
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};

    /// This service's SigV4 signing name, as the generated identity states it
    /// (`smithy_query(query="service_info")` -> `sigv4Name` `sts`).
    const SIGNING_NAME: &str = "sts";

    /// A credential scope naming `signing_name`, the primary routing signal.
    fn authorization(access_key_id: &str, signing_name: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/us-east-1/{signing_name}/aws4_request, \
             SignedHeaders=host, Signature=abc"
        )
    }

    /// The parts of a request addressed as the row describes.
    fn parts(
        method: &str,
        uri: &str,
        scope: Option<(&str, &str)>,
        target: Option<&str>,
        host: Option<&str>,
        content_type: Option<&str>,
    ) -> super::http::request::Parts {
        let mut builder = super::http::Request::builder().method(method).uri(uri);
        if let Some((access_key_id, signing_name)) = scope {
            builder = builder.header("authorization", authorization(access_key_id, signing_name));
        }
        if let Some(target) = target {
            builder = builder.header("x-amz-target", target);
        }
        if let Some(host) = host {
            builder = builder.header("host", host);
        }
        if let Some(content_type) = content_type {
            builder = builder.header("content-type", content_type);
        }
        builder
            .body(())
            .expect("a well-formed test request")
            .into_parts()
            .0
    }

    /// A request naming no service is claimed by the `awsQuery` wire form, and only by it.
    #[test]
    fn an_unattributed_request_is_claimed_only_on_this_protocol_s_wire_form() {
        // One row: how the request is addressed, and whether the wire form claims it.
        let rows: Vec<(&str, super::http::request::Parts, bool)> = vec![
            (
                "unsigned form POST to the root path",
                parts("POST", "/", None, None, Some("127.0.0.1:9297"), Some(QUERY_CONTENT_TYPE)),
                true,
            ),
            (
                "unsigned form POST with a charset parameter",
                parts(
                    "POST",
                    "/",
                    None,
                    None,
                    Some("localhost:9297"),
                    Some("application/x-www-form-urlencoded; charset=utf-8"),
                ),
                true,
            ),
            (
                "unsigned form POST with no host at all",
                parts("POST", "/", None, None, None, Some(QUERY_CONTENT_TYPE)),
                true,
            ),
            (
                // The credential scope names a service, so that service's claim decides.
                "signed for another service",
                parts("POST", "/", Some(("AKIDEXAMPLE", "iam")), None, None, Some(QUERY_CONTENT_TYPE)),
                false,
            ),
            (
                "signed for this service",
                parts("POST", "/", Some(("AKIDEXAMPLE", SIGNING_NAME)), None, None, Some(QUERY_CONTENT_TYPE)),
                false,
            ),
            (
                "targeted at an awsJson service",
                parts(
                    "POST",
                    "/",
                    None,
                    Some("AWSCognitoIdentityService.GetId"),
                    None,
                    Some(QUERY_CONTENT_TYPE),
                ),
                false,
            ),
            (
                "addressed by a service's endpoint host",
                parts("POST", "/", None, None, Some("sts.us-east-1.amazonaws.com"), Some(QUERY_CONTENT_TYPE)),
                false,
            ),
            (
                "unsigned form POST to a path no awsQuery operation uses",
                parts("POST", "/2011-06-15/assume-role", None, None, None, Some(QUERY_CONTENT_TYPE)),
                false,
            ),
            (
                "unsigned GET of the root path",
                parts("GET", "/", None, None, None, Some(QUERY_CONTENT_TYPE)),
                false,
            ),
            (
                "unsigned POST declaring a JSON body",
                parts("POST", "/", None, None, None, Some("application/json")),
                false,
            ),
            (
                "unsigned POST declaring no body type",
                parts("POST", "/", None, None, None, None),
                false,
            ),
        ];

        for (case, parts, expected) in rows {
            let actual = addresses_unattributed_query_request(&parts);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the wire-form claim == {expected}, got {actual}"
            );
        }
    }

    /// The service, over an engine holding the credentials the rows present.
    fn service() -> EmulatedService<::engine::InMemoryEngine> {
        let engine = Arc::new(::engine::InMemoryEngine::new());
        let account = ::engine::emulator_account_id().to_string();
        engine
            .state::<::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State>()
            .lock()
            .access_keys
            .insert(
                "AKIAEMULATORUSERKEY1".to_string(),
                ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::AccessKeyRecord {
                    user_key: ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::name_key("alice"),
                    status: "Active".to_string(),
                    create_date: ::aws_smithy_types::DateTime::from_secs(0),
                },
            );
        let now = SystemTime::now();
        engine
            .state::<crate::ComAmazonawsStsAwssecuritytokenservicev20110615State>()
            .lock()
            .sessions
            .insert(
                "ASIAEMULATORSESSION1".to_string(),
                Session {
                    access_key_id: "ASIAEMULATORSESSION1".to_string(),
                    created_at: now,
                    expires_at: now + Duration::from_secs(3600),
                    duration_seconds: 3600,
                    identity: Identity {
                        user_id: "AROATESTROLEID000001:probe".to_string(),
                        account: account.clone(),
                        arn: format!("arn:aws:sts::{account}:assumed-role/demo/probe"),
                    },
                    kind: SessionKind::AssumedRole,
                    source_identity: None,
                    tags: Vec::new(),
                    transitive_tag_keys: Vec::new(),
                    packed_policy_size: None,
                    external_id: None,
                    role_arn: Some(format!("arn:aws:iam::{account}:role/demo")),
                    revoked: false,
                },
            );
        EmulatedService::new(engine)
    }

    /// An authentication refusal is worded for the CREDENTIAL the request presented: a key
    /// id the account holds is a signature mismatch, and one it does not hold is a
    /// credential that resolves to nothing.
    #[test]
    fn an_authentication_refusal_names_the_failure_the_presented_credential_had() {
        let service = service();
        // One row: the access key id the refused request presented, and the code and
        // message AWS words for that credential's failure.
        let rows: Vec<(&str, Option<&str>, &str, &str)> = vec![
            (
                "an IAM user's own access key, which the account holds",
                Some("AKIAEMULATORUSERKEY1"),
                SIGNATURE_MISMATCH_CODE,
                SIGNATURE_MISMATCH_MESSAGE,
            ),
            (
                "a session this service minted",
                Some("ASIAEMULATORSESSION1"),
                SIGNATURE_MISMATCH_CODE,
                SIGNATURE_MISMATCH_MESSAGE,
            ),
            (
                "a syntactically valid key id that was never issued",
                Some("AKIAIOSFODNN7EXAMPLE"),
                UNRESOLVED_CREDENTIAL_CODE,
                UNRESOLVED_CREDENTIAL_MESSAGE,
            ),
            (
                "a key id one character off a held one",
                Some("AKIAEMULATORUSERKEY2"),
                UNRESOLVED_CREDENTIAL_CODE,
                UNRESOLVED_CREDENTIAL_MESSAGE,
            ),
            (
                // No credential scope at all: the framework reads the key id as the
                // sentinel `default`, which names no credential.
                "no credential at all",
                None,
                UNRESOLVED_CREDENTIAL_CODE,
                UNRESOLVED_CREDENTIAL_MESSAGE,
            ),
        ];

        for (case, access_key_id, expected_code, expected_message) in rows {
            let scope = access_key_id.map(|key| (key, SIGNING_NAME));
            let parts = parts("POST", "/", scope, None, None, Some(QUERY_CONTENT_TYPE));
            remember_presented_access_key_id(&parts);
            let refused = service.unauthenticated_refusal();
            assert_eq!(
                (refused.code.as_str(), refused.message.as_str()),
                (expected_code, expected_message),
                "case {case:?}: expected {expected_code} with AWS's own wording, got {refused:?}"
            );
            // The note the wording is taken from is REQUEST-scoped: a refusal that this
            // service's own claim did not precede reads no credential at all, rather than
            // the one the previous request presented.
            let unattributed = service.unauthenticated_refusal();
            assert_eq!(
                (unattributed.code.as_str(), unattributed.message.as_str()),
                (UNRESOLVED_CREDENTIAL_CODE, UNRESOLVED_CREDENTIAL_MESSAGE),
                "case {case:?}: expected a refusal with no credential noted to resolve \
                 nothing, got {unattributed:?}"
            );
        }
    }
}
