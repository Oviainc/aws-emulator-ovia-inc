// SPDX-License-Identifier: CC0-1.0

//! Whether the SigV4 credential an `AWS_IAM`-authorized invoke presents AUTHENTICATES: the
//! access key id names a credential this emulator holds, and the signature was computed
//! with that credential's secret.
//!
//! `Method$authorizationType` gives `AWS_IAM` "for using AWS IAM permissions", and AWS takes
//! two decisions for such a request — it AUTHENTICATES the credential, and then AUTHORIZES
//! `execute-api:Invoke` for the principal that credential speaks for
//! ([`super::execute_api_authorization`]). The second decision is only about a principal the
//! first one established: a made-up access key id, and a real one signed with the wrong
//! secret, are both refused before the api's own documents are read and before any
//! integration runs.
//!
//! ## Why this plane authenticates its own requests
//!
//! Signature verification belongs to the emulator, not to a service, and for every MODELLED
//! operation it stays there: the front door verifies the credential against the signing name
//! of the service that claimed the request. An invoke of a deployed stage is not a modelled
//! operation and is not signed under this service's name — it is signed under
//! [`EXECUTE_API_SIGNING_NAME`], the service AWS publishes the invoke endpoint and the
//! `execute-api:Invoke` action under — so the front door cannot be pointed at it from here
//! and serves the invoke unsigned ([`crate::service`]'s module docs state the whole rule).
//! What is NOT written here is the verification itself: it is the emulator's own
//! [`SigV4Authenticator`], ASKED about this plane's signing name and this emulator's own
//! credential store, so an invoke is held to the same canonical request, the same clock skew
//! and the same chunk signatures every other request of this process is, and no second
//! reading of SigV4 exists in this crate.
//!
//! ## When it is asked at all
//!
//! Only where the emulator serving the request authenticates requests — which is what the
//! router hands the handler as the credentials it accepts ([`vended`]). An emulator running
//! with authentication off checks no signature anywhere, and a stage of it must not be the
//! one surface that does, or a client signing with a key that emulator never published would
//! be refused by this plane alone.
//!
//! Nothing here reads or writes this service's records.

use std::borrow::Cow;

use aws_smithy_runtime_api::http::Request as RtRequest;
use smithy_server_runtime::OperationContext;

use ::emulator_server::{RequestAuthenticator, SigV4Authenticator, VendedCredentials};

/// The signing name an invoke of a deployed stage is signed under: the `execute-api` service
/// AWS publishes the invoke endpoint and the `execute-api:Invoke` action under, which is NOT
/// this service's own (`apigateway`, the control plane's).
const EXECUTE_API_SIGNING_NAME: &str = super::EXECUTE_API_LABEL;

/// The error code the emulator's own authenticator refuses a SIGNATURE with, as against a
/// credential it holds no secret for or will not act on; see [`Unauthenticated`] for the
/// classes this plane answers.
const SIGNATURE_MISMATCH_CODE: &str = "SignatureDoesNotMatch";

/// Why an invoke's credential did not authenticate. The classes AWS words differently, each
/// refused `403`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unauthenticated {
    /// The authenticator will not act on the credential the request presented: it holds no
    /// secret for that access key id, the session token presented does not belong to it, or
    /// the signing window the request names has passed. The authenticator reports those
    /// alike, and this plane does not re-derive the distinction from the request.
    UnknownCredential,
    /// The credential's scope names another service than the one an invoke of a deployed
    /// stage is signed under, so the signature is over a canonical request for a different
    /// endpoint whatever secret produced it.
    MisscopedCredential,
    /// The access key id names a credential, and the signature the request carries is not
    /// the one that credential's secret produces over this canonical request — a wrong
    /// secret, or a signature over a different request.
    SignatureMismatch,
}

impl Unauthenticated {
    /// The sentence AWS refuses this class with — fixed text for the class, except for the
    /// signing name a mis-scoped credential is told to use, which is this plane's own and is
    /// taken from nothing the request sent.
    #[must_use]
    pub fn message(self) -> Cow<'static, str> {
        match self {
            Self::UnknownCredential => {
                Cow::Borrowed("The security token included in the request is invalid.")
            }
            Self::MisscopedCredential => Cow::Owned(format!(
                "Credential should be scoped to correct service: '{EXECUTE_API_SIGNING_NAME}'."
            )),
            Self::SignatureMismatch => Cow::Borrowed(
                "The request signature we calculated does not match the signature you \
                 provided. Check your AWS Secret Access Key and signing method. Consult the \
                 service documentation for details.",
            ),
        }
    }

    /// The exception class the `execute-api` endpoint reports this class in the error-type
    /// header as, so a client branches on the class rather than on the prose.
    #[must_use]
    pub fn error_type(self) -> &'static str {
        match self {
            Self::UnknownCredential => "UnrecognizedClientException",
            Self::MisscopedCredential | Self::SignatureMismatch => "InvalidSignatureException",
        }
    }
}

/// Why the credential `req` presents does not authenticate, or `None` when it does — and
/// `None` for every request of an emulator that authenticates nothing at all (see the module
/// docs).
///
/// Asked only for a method whose `authorizationType` is `AWS_IAM`, and only once the request
/// is known to CARRY a credential: a method that authenticates nobody
/// (`authorizationType` `NONE`, a key-required method, an authorizer-backed one) is served
/// whatever a request's `Authorization` header says, as AWS serves it, and a request
/// carrying no credential at all is the missing-token refusal rather than this one.
#[must_use]
pub fn refusal(req: &RtRequest, ctx: &OperationContext<'_>) -> Option<Unauthenticated> {
    if !vended(ctx) {
        return None;
    }
    // Past the guard above this emulator DOES accept access keys, so the authenticator it
    // accepts them through has to be there: a credential nothing can verify is refused
    // rather than served, which is what the `execute-api` endpoint does with one.
    let Some(authenticator) = SigV4Authenticator::minted_credentials() else {
        return Some(Unauthenticated::UnknownCredential);
    };
    // The SCOPE first, because the authenticator refuses a credential scoped to another
    // service in the same breath as one it holds no secret for, and AWS words the two
    // differently: a scope naming another service is told which service to scope to.
    if super::sigv4_credential_scope_service(req)
        .is_some_and(|service| service != EXECUTE_API_SIGNING_NAME)
    {
        return Some(Unauthenticated::MisscopedCredential);
    }
    // The verification reads the request as the front door would: its method, its URI and
    // its headers, beside the body the signature covers. A request this plane cannot
    // re-present at all carries no signature anything could verify, so it is refused as a
    // signature that does not match.
    let Some(parts) = parts_of(req) else {
        return Some(Unauthenticated::SignatureMismatch);
    };
    let body = req.body().bytes().unwrap_or_default();
    match authenticator.verify(
        &parts,
        body,
        EXECUTE_API_SIGNING_NAME,
        ctx.credential_store(),
    ) {
        Ok(()) => None,
        Err(refused) if refused.code == SIGNATURE_MISMATCH_CODE => {
            Some(Unauthenticated::SignatureMismatch)
        }
        // Everything else the authenticator refuses is a credential it will not act on —
        // see [`Unauthenticated::UnknownCredential`], which states which those are.
        Err(_) => Some(Unauthenticated::UnknownCredential),
    }
}

/// Whether the emulator serving this request authenticates requests at all: the credentials
/// it ACCEPTS, which the router fills from the authenticators registered on it and leaves
/// empty when none is.
fn vended(ctx: &OperationContext<'_>) -> bool {
    ctx.callback()
        .credentials()
        .iter()
        .any(|credential| matches!(credential, VendedCredentials::AwsAccessKey { .. }))
}

/// `req` as the request parts the emulator's own authenticator reads: the method, the URI
/// and every header, in the shapes `http` names them.
///
/// `None` when the request cannot be re-presented — a method, a URI or a header value the
/// `http` types refuse — which is a request no signature could have been computed over
/// either.
fn parts_of(req: &RtRequest) -> Option<http::request::Parts> {
    let mut builder = http::Request::builder()
        .method(req.method())
        .uri(req.uri());
    for (name, value) in req.headers().iter() {
        builder = builder.header(name, value);
    }
    Some(builder.body(()).ok()?.into_parts().0)
}

#[cfg(test)]
mod tests {
    //! The pure rule this module owns: the sentence and the class each outcome states. That
    //! a deployed `AWS_IAM` method REFUSES an unknown key and a forged signature, and serves
    //! a correctly signed invoke, is pinned end to end — against an emulator whose
    //! authentication is on — in
    //! `core/router/tests/all/apigateway_iam_authenticated_invoke.rs`.

    use super::*;

    /// RULE: each class carries AWS's own sentence for it and the exception class the
    /// endpoint reports it in — a client can tell a credential it must replace from one it
    /// must re-scope from a signature it must recompute.
    #[test]
    fn each_class_states_its_own_sentence_and_exception() {
        let rows = [
            (
                "an access key id naming no credential",
                Unauthenticated::UnknownCredential,
                "The security token included in the request is invalid.",
                "UnrecognizedClientException",
            ),
            (
                "a credential scoped to another service",
                Unauthenticated::MisscopedCredential,
                "Credential should be scoped to correct service: 'execute-api'.",
                "InvalidSignatureException",
            ),
            (
                "a signature computed with another secret",
                Unauthenticated::SignatureMismatch,
                "The request signature we calculated does not match the signature you \
                 provided. Check your AWS Secret Access Key and signing method. Consult the \
                 service documentation for details.",
                "InvalidSignatureException",
            ),
        ];
        for (case, class, message, error_type) in rows {
            assert_eq!(
                (class.message().as_ref(), class.error_type()),
                (message, error_type),
                "case {case:?}: got {:?}",
                (class.message(), class.error_type())
            );
        }
        assert_ne!(
            Unauthenticated::UnknownCredential.error_type(),
            Unauthenticated::SignatureMismatch.error_type(),
            "case \"a credential against a signature\": expected distinct exception classes, \
             got {:?}",
            Unauthenticated::UnknownCredential.error_type()
        );
        assert_ne!(
            Unauthenticated::MisscopedCredential.message(),
            Unauthenticated::SignatureMismatch.message(),
            "case \"a mis-scoped credential against a wrong signature\": expected distinct \
             sentences, got {:?}",
            Unauthenticated::MisscopedCredential.message()
        );
    }

    /// RULE: the signing name asked about is the data plane's own, never the control
    /// plane's — a credential scoped to `apigateway` does not authenticate an invoke.
    #[test]
    fn the_signing_name_is_the_data_planes_own() {
        assert_eq!(
            EXECUTE_API_SIGNING_NAME, "execute-api",
            "case \"the signing name an invoke is signed under\": got \
             {EXECUTE_API_SIGNING_NAME:?}"
        );
    }
}
