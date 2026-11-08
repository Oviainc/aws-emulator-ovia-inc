// SPDX-License-Identifier: CC0-1.0

//! The refusal a request earns for the BUCKET NAME it addresses, before anything asks
//! whether such a bucket is there.
//!
//! A bucket name outside the syntax Amazon S3 can address a bucket by
//! ([`super::names_a_bucket`]) names no bucket in any account, so S3 refuses the request for
//! the NAME — `InvalidBucketName` (400), naming the rejected name in the `<BucketName>`
//! member its error document carries it in — rather than reporting a bucket that is merely
//! absent. A client told "no such bucket" about a name no bucket can ever have retries the
//! create forever.
//!
//! Every operation of this service that addresses a bucket is refused the same way, so the
//! rule lives in ONE place: this operation, which [`super::EmulatedService::override_operation`]
//! routes such a request to instead of the operation its address names. Answering it as an
//! OPERATION rather than at the request boundary is what lets it carry the member — a refusal
//! the boundary makes is rendered by the framework from a code and a sentence alone, with
//! nowhere to put the name — and gives it the request-correlation ids every other answer of
//! this service is named by.
//!
//! The one operation it does NOT stand in front of is `CreateBucket`: that call is the one
//! that MINTS a name, so it judges the name itself, against the (narrower) rules a bucket may
//! be created under today — see `crate::ops::create_bucket`.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::protocol::ServerProtocol;
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

use super::wire::UnwrappedXmlRestXml;

/// The routing key this operation is held in the service's own table under — a key the
/// model's protocol never resolves, so no modelled request can reach it by name.
pub const REFUSE_INVALID_BUCKET_NAME: &str = "@refuse-invalid-bucket-name";

/// The operation that answers the refusal, holding the codec it renders the envelope with.
pub struct RefuseInvalidBucketName {
    /// This service's own protocol, built ONCE (the service's constructor builds this
    /// operation): the envelope a refusal of this service goes out in — the bare `<Error>`
    /// root with the member naming the rejected bucket, and the `x-amz-request-id` /
    /// `x-amz-id-2` pair S3 names every answer by.
    protocol: UnwrappedXmlRestXml,
}

impl RefuseInvalidBucketName {
    /// Build the refusal operation. INFALLIBLE, and it holds no state: what it answers is
    /// read from the request it is handed.
    pub fn new() -> Self {
        Self {
            protocol: UnwrappedXmlRestXml::new(),
        }
    }

    /// The envelope naming `bucket`: `InvalidBucketName` with the sentence S3's published
    /// error list states for that code, and the `<BucketName>` member carrying the name as
    /// the request addressed it.
    fn refusal(&self, bucket: &str) -> Option<RtResponse> {
        let refusal = crate::scaffolded::UnmodeledWithDetail {
            message: crate::state::INVALID_BUCKET_NAME_MESSAGE.to_string(),
            detail: crate::scaffolded::ErrorDetail::Bucket(bucket.to_string()),
        };
        self.protocol
            .serialize_error(
                &refusal,
                &crate::scaffolded::UNMODELED_SCHEMA,
                crate::state::INVALID_BUCKET_NAME_CODE,
                crate::state::INVALID_ARGUMENT_STATUS,
            )
            .ok()
    }
}

impl Default for RefuseInvalidBucketName {
    fn default() -> Self {
        Self::new()
    }
}

impl ErasedOperation for RefuseInvalidBucketName {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The name the request addressed, read the way every other seam of this service
            // reads one ([`super::addressed_bucket_name`]), so the name refused and the name
            // the envelope reports are the same text. The request reaching here is a
            // PATH-STYLE one: a virtual-hosted-style request's bucket is the first label of
            // its host and is read as a bucket only when this emulator HOLDS it
            // ([`super::addressed_in_host`]), and a held bucket's name is one S3 can
            // address, so such a request is never routed here.
            let bucket =
                super::addressed_bucket_name(super::wire::request_path(req.uri())).unwrap_or_default();
            let _ = ctx;
            match self.refusal(&bucket) {
                Some(response) => response,
                // The serializer cannot fail on a fixed code, a fixed sentence and one
                // string member; if it ever did, the request keeps the framework's own
                // answer rather than panicking in the request path.
                None => svc.definition().routing_error_response(req),
            }
        })
    }
}
