// SPDX-License-Identifier: CC0-1.0

//! How an emulated service words the request the emulator's authentication layer refused.
//!
//! The emulator verifies every signature; a service never checks one itself. What the layer
//! hands a service is ONE structural refusal — authentication refused the caller — whichever
//! way the credential failed, and the generic code it carries (`AccessDenied`) is an
//! AUTHORIZATION denial, which is not what AWS answers here.
//!
//! AWS splits the refusal in two, and the split is load-bearing:
//!
//! * an access key id that resolves to NO credential — never issued, deleted, or presented
//!   to a service it was not scoped for — failed AUTHENTICATION: no principal was found, so
//!   nothing could be authorized. The answer is the service's own unknown-credential code,
//!   which is what makes an SDK re-resolve its credential chain (refresh a stale profile,
//!   re-read instance metadata, fail fast on a mistyped key id);
//! * a key id that DOES resolve, whose signature does not verify under that credential's
//!   secret, failed the SIGNATURE: the caller is known, and the service's signature code
//!   points at the secret half of the pair rather than at the key id.
//!
//! `AccessDenied` is neither: it is the answer a KNOWN principal gets for an action its
//! policies do not permit, and it makes a client report a policy problem and stop. With the
//! codes collapsed, a wrong or stale key is diagnosed as a missing permission and no
//! credential-refresh path in a client is ever exercised.
//!
//! Which of the two a refusal is, the authentication layer already decided — it is the code
//! it reports — and the one seam handed that code is the service's own definition, where
//! `refusal_response` renders it. [`AuthenticationRefusalDefinition`] wraps a generated
//! definition there and restates the refusal in that service's vocabulary: the two codes the
//! service names ([`StatedAuthenticationRefusal`]) carrying the fixed sentences AWS's SigV4
//! layer produces. The codes differ by protocol family and by service, so they are the
//! service's to name and only the rule lives here.
//!
//! A service whose answer depends on the REQUEST rather than on the layer's verdict — the
//! signing diagnostics S3 reports, or a credential a service's own store refuses — words it
//! through [`smithy_server_runtime::Service::override_refusal`] instead, and this wrapper
//! defers to that: a refusal already worded there is passed through untouched.
//!
//! Nothing here reads a credential, a secret, a request or any state: one code is translated
//! into another. The HTTP status and the error envelope stay with the protocol that owns
//! them.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    ErasedServiceDefinition, OperationRoutingTable, Refused, ServerHttpProtocol, ServiceIdentity,
};

/// The code the authentication layer reports when the request's access key id resolved to no
/// credential this emulator holds. It is the layer's own generic wording, which is why a
/// service restates it: no AWS service answers an unknown credential with it.
const REPORTED_UNRESOLVED_CREDENTIAL: &str = "AccessDenied";

/// The code it reports when the key id resolved and the signature computed under that
/// credential's secret is not the one the request carried.
const REPORTED_SIGNATURE_MISMATCH: &str = "SignatureDoesNotMatch";

/// How AWS words a credential that resolves to nothing: fixed text with no variable part, so
/// a caller presenting an unknown key learns nothing about this account.
pub const UNRESOLVED_CREDENTIAL_MESSAGE: &str =
    "The security token included in the request is invalid.";

/// How AWS's SigV4 layer words a signature that does not verify: the fixed sentence for that
/// one condition, which names the half of the credential pair that is wrong.
pub const SIGNATURE_MISMATCH_MESSAGE: &str =
    "The request signature we calculated does not match the signature you provided. Check \
     your AWS Secret Access Key and signing method. Consult the service documentation for \
     details.";

/// The two codes one service answers the two authentication failures with.
///
/// Both are the service's own: the names differ by protocol family — the awsQuery identity
/// services answer `InvalidClientTokenId` / `SignatureDoesNotMatch`, an awsJson service
/// `UnrecognizedClientException` / `InvalidSignatureException` — and a single code cannot be
/// right for all of them.
#[derive(Debug, Clone, Copy)]
pub struct StatedAuthenticationRefusal {
    /// What this service calls a request whose access key id resolved to no credential.
    pub unresolved_credential: &'static str,
    /// What it calls a request whose signature did not verify under a credential that did
    /// resolve.
    pub signature_mismatch: &'static str,
}

impl StatedAuthenticationRefusal {
    /// How this service words the refusal the authentication layer reported as `reported`.
    ///
    /// `None` for anything else: a code that is already a service's own (a pre-dispatch
    /// refusal a service rendered itself) keeps the words it was given rather than being
    /// mapped a second time.
    #[must_use]
    pub fn stated(&self, reported: &str) -> Option<Refused> {
        let (code, message) = match reported {
            REPORTED_UNRESOLVED_CREDENTIAL => (
                self.unresolved_credential,
                UNRESOLVED_CREDENTIAL_MESSAGE,
            ),
            REPORTED_SIGNATURE_MISMATCH => (self.signature_mismatch, SIGNATURE_MISMATCH_MESSAGE),
            _ => return None,
        };
        Some(Refused {
            code: code.to_string(),
            message: message.to_string(),
        })
    }
}

/// A service's model-derived half with the authentication refusal stated in that service's
/// own vocabulary (see the module docs).
///
/// Identity, the routing table, the protocol and every other rendering come from the wrapped
/// definition untouched — nothing model-derived is restated here. Generic over that
/// definition rather than over an engine, so this crate names no service's generated type.
pub struct AuthenticationRefusalDefinition<D> {
    inner: D,
    stated: StatedAuthenticationRefusal,
}

impl<D: ErasedServiceDefinition> AuthenticationRefusalDefinition<D> {
    /// Wrap a service's definition, stating an authentication refusal with `stated`'s codes.
    pub fn new(inner: D, stated: StatedAuthenticationRefusal) -> Self {
        Self { inner, stated }
    }
}

impl<D: ErasedServiceDefinition> ErasedServiceDefinition for AuthenticationRefusalDefinition<D> {
    fn identity(&self) -> &ServiceIdentity {
        self.inner.identity()
    }

    fn protocol(&self) -> &dyn ServerHttpProtocol {
        self.inner.protocol()
    }

    fn routing_table(&self) -> &OperationRoutingTable {
        self.inner.routing_table()
    }

    fn routing_error_response(&self, req: &RtRequest) -> RtResponse {
        self.inner.routing_error_response(req)
    }

    fn framework_error_response(&self, error: ProtocolError, req: &RtRequest) -> RtResponse {
        self.inner.framework_error_response(error, req)
    }

    fn refusal_response(
        &self,
        error: ProtocolError,
        chosen: Option<Refused>,
        req: &RtRequest,
    ) -> RtResponse {
        // A refusal the service worded itself is passed through: this layer states the ones
        // the authentication layer reported in its own generic vocabulary, and nothing else.
        let stated = match (&chosen, &error) {
            (None, ProtocolError::Unauthenticated { code, .. }) => self.stated.stated(code),
            _ => None,
        };
        self.inner.refusal_response(error, chosen.or(stated), req)
    }

    fn wire_protocol(&self, req: &RtRequest) -> ::smithy_server_runtime::WireProtocol<'_> {
        self.inner.wire_protocol(req)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        StatedAuthenticationRefusal, SIGNATURE_MISMATCH_MESSAGE, UNRESOLVED_CREDENTIAL_MESSAGE,
    };

    /// The vocabulary of an awsQuery identity service, and of an awsJson one: two services
    /// whose codes for the same two failures disagree.
    const QUERY: StatedAuthenticationRefusal = StatedAuthenticationRefusal {
        unresolved_credential: "InvalidClientTokenId",
        signature_mismatch: "SignatureDoesNotMatch",
    };
    const JSON: StatedAuthenticationRefusal = StatedAuthenticationRefusal {
        unresolved_credential: "UnrecognizedClientException",
        signature_mismatch: "InvalidSignatureException",
    };

    /// One row: the service's vocabulary, the code the authentication layer reported, and the
    /// code and message that service answers for it (`None` where it answers with the words it
    /// was given).
    struct Row {
        case: &'static str,
        vocabulary: StatedAuthenticationRefusal,
        reported: &'static str,
        stated: Option<(&'static str, &'static str)>,
    }

    /// Each failure the authentication layer reports is stated in the service's OWN code with
    /// AWS's fixed sentence for that failure, and a code the layer does not report is left
    /// exactly as it arrived.
    #[test]
    fn each_reported_failure_is_stated_in_the_services_own_code() {
        let rows = vec![
            Row {
                case: "a query service, a credential that resolved to nothing",
                vocabulary: QUERY,
                reported: "AccessDenied",
                stated: Some(("InvalidClientTokenId", UNRESOLVED_CREDENTIAL_MESSAGE)),
            },
            Row {
                case: "a query service, a signature that did not verify",
                vocabulary: QUERY,
                reported: "SignatureDoesNotMatch",
                stated: Some(("SignatureDoesNotMatch", SIGNATURE_MISMATCH_MESSAGE)),
            },
            Row {
                case: "a json service, a credential that resolved to nothing",
                vocabulary: JSON,
                reported: "AccessDenied",
                stated: Some(("UnrecognizedClientException", UNRESOLVED_CREDENTIAL_MESSAGE)),
            },
            Row {
                case: "a json service, a signature that did not verify",
                vocabulary: JSON,
                reported: "SignatureDoesNotMatch",
                stated: Some(("InvalidSignatureException", SIGNATURE_MISMATCH_MESSAGE)),
            },
            Row {
                // Already a service's own code: a pre-dispatch refusal it rendered itself.
                case: "a json service, a refusal already worded by the service",
                vocabulary: JSON,
                reported: "UnrecognizedClientException",
                stated: None,
            },
            Row {
                case: "a query service, a refusal already worded by the service",
                vocabulary: QUERY,
                reported: "InvalidClientTokenId",
                stated: None,
            },
        ];

        for Row {
            case,
            vocabulary,
            reported,
            stated: expected,
        } in rows
        {
            let stated = vocabulary.stated(reported);
            let actual = stated
                .as_ref()
                .map(|refused| (refused.code.as_str(), refused.message.as_str()));
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?} for the reported code {reported:?}, got \
                 {actual:?}"
            );
        }
    }
}
