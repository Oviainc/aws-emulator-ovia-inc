// SPDX-License-Identifier: CC0-1.0

//! Serves the export file of an export job at the pre-signed URL `GetExportJob` hands
//! out for it.
//!
//! "SES writes the export file to a location that it manages. After the job completes,
//! call `GetExportJob` and use the `S3Url` that's returned to download the file." The
//! location SES manages is this emulator: `GetExportJob` signs a URL addressing the
//! path [`crate::state::export_file_path`] builds for the job, so the URL leads back
//! here and verifies, and this route answers a `GET` on that path with the bytes the
//! job's run wrote, under the media type its data format is served as.
//!
//! The model binds no operation to that path — the file is not an API call, it is the
//! artifact the API points at — so the route is claimed through
//! [`smithy_server_runtime::Service::override_operation`] rather than through the
//! generated routing table, the same seam this service's SDK collection listings are
//! claimed on. It is claimed only for a PRE-SIGNED request ([`is_presigned`]), whose
//! signature and expiry window the framework verifies before the route runs, so the
//! file's only address is the URL `GetExportJob` handed out for it.

use std::sync::Arc;

use smithy_server_runtime::{http, BoxFuture, ErasedOperation, OperationContext, Service};

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;

/// The method a download is fetched on.
const DOWNLOAD_METHOD: &str = "GET";

/// The media type the one answer of this route that is not a file is served as.
const PLAIN_MEDIA_TYPE: &str = "text/plain; charset=utf-8";

/// What this route answers a URL that names no file: a pre-signed download URL is
/// fetched by a plain HTTP client rather than by an AWS SDK, so the answer is the
/// status itself with a one-line reason, and no API error envelope.
const NO_FILE: &str = "no export file at this location\n";

/// Serves a download of one export job's file.
///
/// Owns the shared engine, as an operation does, because the file it serves is the job
/// record's: nothing is held beside the record, so a download serves exactly the bytes
/// the job reports.
pub struct ExportFileRoute<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> ExportFileRoute<Eng> {
    /// Build the route from the shared engine, as the service's operations are built.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }

    /// The id of the job whose export file `req` asks for, or `None` when the request
    /// is not a download this route serves.
    ///
    /// Two things make it one: the path, read the way the signer wrote it — its
    /// segments are percent-decoded, so a job id that had to be encoded in the URL
    /// names the same job here as it does in the record — and the SigV4 query
    /// authentication of a PRE-SIGNED request ([`is_presigned`]). Nothing else is
    /// served from this path at all, so the file is reachable only through a URL
    /// `GetExportJob` handed out for that one job, whose signature the framework
    /// verifies over this method and this path before this route runs.
    pub fn claims(&self, req: &RtRequest) -> Option<String> {
        if req.method() != DOWNLOAD_METHOD || !is_presigned(req.uri()) {
            return None;
        }
        let segments = decoded_segments(req.uri());
        crate::state::export_file_path_job_id(&segments).map(str::to_string)
    }

    /// The file `job_id`'s run wrote, as `(media type, bytes)`, or `None` when the
    /// account holds no such job, its run has written no file, or the call is not
    /// scoped to the account whose store this is.
    ///
    /// `scope` is the account and Region the CALL belongs to, read from the request
    /// rather than from anything it carries ([`crate::state::CallerScope`]). This
    /// emulator serves ONE account — the store [`crate::state::service_state`] hands
    /// back is that account's export jobs, and `CallerScope::account_id` is that same
    /// account — so a job id selects a job WITHIN the caller's own account and can
    /// never reach another's. That invariant is checked here rather than assumed: a
    /// call scoped to any other account is answered exactly as one naming a job that
    /// does not exist.
    fn stored_file(
        &self,
        scope: &crate::state::CallerScope,
        job_id: &str,
    ) -> Option<(&'static str, Vec<u8>)> {
        if scope.account_id != ::emulator_server::emulator_account_id() {
            return None;
        }
        let state = crate::state::service_state(&*self.engine)?;
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        // The one thing this materialises is the progress the stored runs had already
        // made at `now`, which was due whether or not anybody fetched anything — so a
        // download and a read of the same job agree about whether there is a file.
        store.settle(now);
        let file = store.export_jobs.get(job_id)?.file.as_ref()?;
        Some((file.media_type, file.body.clone().into_bytes()))
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for ExportFileRoute<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        // The account and Region this call belongs to, which is what decides whose
        // export jobs the id may name; nothing this route does waits on anything.
        let scope = crate::state::CallerScope::of(ctx.parts());
        let answer = match self.claims(req) {
            Some(job_id) => match self.stored_file(&scope, &job_id) {
                Some((media_type, body)) => file_response(200, media_type, body),
                // The URL names a job this caller's account does not hold, or one whose
                // run has written no file: there is nothing at this location.
                None => file_response(404, PLAIN_MEDIA_TYPE, NO_FILE.as_bytes().to_vec()),
            },
            // Unreachable through an `override_operation` that hands this route only
            // the requests it claims; answered as the framework answers a request that
            // named no operation rather than guessed at.
            None => svc.definition().routing_error_response(req),
        };
        Box::pin(async move { answer })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` keeps the framework's own decision for this service: the URL this
        // route serves is SigV4 query-signed, and nothing here may weaken that.
        None
    }
}

/// One download response: the bytes, their media type and their length.
fn file_response(status: u16, media_type: &str, body: Vec<u8>) -> RtResponse {
    let length = body.len();
    let mut response = RtResponse::new(download_status(status), SdkBody::from(body));
    response
        .headers_mut()
        .insert("content-type", media_type.to_string());
    response
        .headers_mut()
        .insert("content-length", length.to_string());
    response
}

/// A status this route answers with. Every call site passes a constant, so the
/// conversion cannot fail.
fn download_status(code: u16) -> StatusCode {
    StatusCode::try_from(code).expect("an export-file download answers constant statuses")
}

/// Whether `uri` carries the SigV4 query authentication of a pre-signed request: the
/// credential scope and the signature [`smithy_server_runtime::UrlSigner`] puts on a
/// URL it signs.
///
/// Presence is all this reads, and all it has to: the framework's own request
/// authentication is what VERIFIES such a request before any route runs — the signature
/// over the method, the path and the query, and the `X-Amz-Expires` window the signer
/// wrote, a URL past which is refused as `AccessDenied` (`vendor/server`'s
/// `request_authentication`). So neither the signature nor the five-minute lifetime
/// `GetExportJob` signs for is re-checked here. What this test adds is that nothing
/// which is not a pre-signed request is served from this path at all, so the file's only
/// address is a URL this service signed for that one job's file.
fn is_presigned(uri: &str) -> bool {
    const PRESIGNED_FIELDS: [&str; 2] = ["X-Amz-Credential", "X-Amz-Signature"];
    let query = uri.split_once('?').map(|(_, query)| query).unwrap_or("");
    PRESIGNED_FIELDS.iter().all(|field| {
        query.split('&').any(|pair| {
            pair.split_once('=')
                .is_some_and(|(name, _)| name.eq_ignore_ascii_case(field))
        })
    })
}

/// The path segments of `uri`, percent-decoded, empty segments dropped — the same view
/// of a path the signer built the URL from.
fn decoded_segments(uri: &str) -> Vec<String> {
    uri.parse::<http::Uri>()
        .map(|parsed| {
            parsed
                .path()
                .split('/')
                .filter(|segment| !segment.is_empty())
                .map(smithy_server_runtime::percent_decode)
                .collect()
        })
        .unwrap_or_default()
}
