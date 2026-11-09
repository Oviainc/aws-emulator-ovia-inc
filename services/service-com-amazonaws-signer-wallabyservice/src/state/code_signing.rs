// SPDX-License-Identifier: CC0-1.0

//! The code-signing-job data plane: what a `StartSigningJob` job READS, what it
//! WRITES, and how the terminal status a read of the job reports follows from
//! both.
//!
//! "You must create an Amazon S3 source bucket … AWS Signer uses your S3
//! destination bucket to write your signed code" (`StartSigningJob`): a job of
//! this workflow is a transformation between two objects in Amazon S3, so the job
//! FOLLOWS the source reference and then writes the signed artifact at the
//! destination. Both halves go over the in-process callback into this same
//! emulator ([`smithy_server_runtime::OperationContext::callback`]) — a
//! `GetObject` and a `PutObject` signed with this emulator's own credentials and
//! dispatched in process — so the artifact a job reports is an object Amazon S3
//! itself serves, and nothing here opens a socket or touches a host path.
//!
//! The job's terminal status is therefore decided by that work rather than by the
//! clock alone:
//!
//! * the signed artifact was written — the job reports `Succeeded` and the
//!   `signedObject` naming it, which a client can read back from Amazon S3;
//! * nothing was written, because the source object could not be read, is not a
//!   readable archive, or the destination would not take the artifact — the job
//!   reports `Failed`, the `statusReason` saying which, and no `signedObject`.
//!
//! So a `signedObject` a read of a job reports always names an object that
//! exists, and a job that produced nothing says so.
//!
//! The work runs when the job is STARTED rather than when it is read: there is no
//! second actor to run it later, and a status is never reported for work nothing
//! did. The artifact is therefore already at the destination while the job is
//! still `InProgress` — the clock owes the job its terminal status, not its
//! bytes — and a client that polls to `Succeeded` and then fetches
//! `signedObject.s3` finds it there.
//!
//! The artifact itself is the source archive's own entries plus ONE added entry
//! carrying the signature envelope ([`SIGNATURE_ENTRY_NAME`]): the signature is
//! made by the in-process cryptography provider over the source object's bytes,
//! under the key pair of the signing profile version the job signs with
//! ([`super::payload_signing`]), so it is verifiable with the public half the
//! envelope carries. `SigningImageFormat`'s values (`JSON`, `JSONEmbedded`,
//! `JSONDetached`) are what say the envelope is a JSON document.

use std::io::{Cursor, Read as _, Write as _};
use std::time::Duration;

use sha2::Digest as _;

use super::payload_signing::PayloadSigningKey;
use crate::ops::types::{Destination, S3SignedObject, S3Source, SignedObject};

/// What one code-signing job's work produced.
#[derive(Debug, Clone)]
pub enum JobOutcome {
    /// The signed artifact was written at the destination: where it is, as a read
    /// of the job reports it.
    Signed(SignedObject),
    /// Nothing was signed, and the sentence the job reports as its
    /// `statusReason`.
    Failed(String),
}

/// The entry the signature envelope travels in, inside the signed archive.
///
/// The name is this emulator's own: nothing in the model names it, and it is
/// prefixed with the service so it cannot collide with an entry the source
/// archive already carries at the archive root.
pub const SIGNATURE_ENTRY_NAME: &str = "aws-signer-signature.json";

/// The most entries a source archive this emulator signs may carry.
///
/// A chosen bound, for the reason the two below it are: the archive is caller
/// content and this process holds its entries in memory while it builds the
/// signed one, so the work is bounded before anything is allocated.
const MAX_ARCHIVE_ENTRIES: usize = 4_096;

/// The most bytes one entry of a source archive may expand to.
const MAX_ENTRY_BYTES: usize = 16 * 1024 * 1024;

/// The most bytes a whole source archive may expand to.
const MAX_ARCHIVE_BYTES: usize = 64 * 1024 * 1024;

/// Follow the source reference `s3` names and hand back the object's own bytes,
/// for a job running in `region`.
///
/// A READ of Amazon S3: it reads and changes nothing there. `Err` is the sentence
/// the job fails with — an answer Amazon S3 refused, worded around the code and
/// message IT answered, and an answer this emulator could not read at all (no
/// Amazon S3 mounted to ask, a hop that did not complete, a body that stopped
/// part-way) worded as the read that did not happen. Either way nothing was
/// signed, so either way the job failed.
pub async fn read_source_object(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    s3: &S3Source,
) -> Result<Vec<u8>, String> {
    let read = client(ctx, region)
        .get_object()
        .bucket(&s3.bucket_name)
        .key(&s3.key)
        // `S3Source$version` is `@required` ("Version of your source image in
        // your version enabled S3 bucket"), so the object a job signs is one
        // VERSION of the source key and never whichever version is current when
        // the job runs. An empty value names no version and addresses the key
        // itself.
        .set_version_id(Some(s3.version.clone()).filter(|version| !version.is_empty()))
        .send()
        .await;
    let read = match read {
        Ok(read) => read,
        Err(error) => return Err(refusal_message("GetObject", &error_metadata(&error))),
    };
    // What the answer SAYS it carries, judged before any of it is held: an archive
    // is never larger than its own contents, so an object Amazon S3 reports as
    // long as the ceiling this emulator signs, or longer, is over it whatever it
    // holds, and is refused on the length S3 reported rather than materialised to
    // be measured.
    if read
        .content_length()
        .is_some_and(|length| length >= MAX_ARCHIVE_BYTES as i64)
    {
        return Err(too_large_message());
    }
    // The object's bytes, read whole out of the in-process answer. A body that
    // stopped part-way through is an answer this emulator could not read, which
    // shows nothing about the object; one longer than the answer claimed is over
    // the same ceiling.
    match read.body.collect().await {
        Ok(body) => {
            let bytes = body.to_vec();
            if bytes.len() >= MAX_ARCHIVE_BYTES {
                return Err(too_large_message());
            }
            Ok(bytes)
        }
        Err(_) => Err(refusal_message("GetObject", &None)),
    }
}

/// The sentence a job whose source object is at or past the ceiling this emulator
/// signs fails with ([`MAX_ARCHIVE_BYTES`]).
///
/// The bound is the one the sentence states: a source has to be SMALLER than the
/// ceiling, so the ceiling itself is over it.
fn too_large_message() -> String {
    format!("The source object must be smaller than {MAX_ARCHIVE_BYTES} bytes")
}

/// How long the artifact work may take before the job is failed for it.
///
/// A chosen bound, like the size caps above: inflating and deflating up to
/// [`MAX_ARCHIVE_BYTES`] of caller content is bounded work, but it is work a
/// REQUEST sized, so it is given a wall-clock ceiling as well — and it runs on a
/// blocking worker ([`signed_artifact`]), so this ceiling is something the
/// runtime can actually land on.
const ARTIFACT_WORK_TIMEOUT: Duration = Duration::from_secs(10);

/// Sign `source` under `key` and assemble the artifact, OFF the async worker and
/// under a wall-clock bound.
///
/// Signing and re-archiving a whole object is CPU-bound work with no I/O to await
/// in it, so it is handed to a blocking worker rather than run on the thread
/// serving the request: the request's own await point stays cancellable, and a
/// shutdown, a timeout and the emulator's health endpoint keep landing while the
/// work runs. Taking longer than [`ARTIFACT_WORK_TIMEOUT`] fails the job rather
/// than holding the request open.
///
/// `Err` is the sentence the job fails with.
pub async fn signed_artifact(
    source: Vec<u8>,
    key: PayloadSigningKey,
    profile_version_arn: String,
    job_id: String,
) -> Result<Vec<u8>, String> {
    let work = ::tokio::task::spawn_blocking(move || {
        let (signature, public_key) = key
            .signature_and_public_key(&source)
            .ok_or_else(no_signature_message)?;
        let envelope = signature_envelope(
            &source,
            &signature,
            &public_key,
            &profile_version_arn,
            &job_id,
        );
        signed_archive(&source, &envelope)
    });
    match ::tokio::time::timeout(ARTIFACT_WORK_TIMEOUT, work).await {
        Ok(Ok(artifact)) => artifact,
        // The worker did not finish — it panicked or was cancelled — which is
        // this emulator's own fault and still a job that signed nothing.
        Ok(Err(_)) => Err("The signed archive could not be assembled".to_string()),
        Err(_) => Err(format!(
            "The signed archive was not assembled within {} seconds",
            ARTIFACT_WORK_TIMEOUT.as_secs()
        )),
    }
}

/// The sentence a job whose profile names no signature scheme this emulator can
/// sign under fails with.
fn no_signature_message() -> String {
    "No signature could be produced for the signing profile's platform".to_string()
}

/// The signature envelope a signed archive carries, as the JSON document it is
/// written as.
///
/// Every member is computed from the job's own inputs: the digest and the
/// signature are over the SOURCE object's bytes, the public key is the half of
/// the profile version's pair a verifier checks that signature against, and the
/// profile version ARN and job id are the ones the job signs under and reports.
pub fn signature_envelope(
    source: &[u8],
    signature: &[u8],
    public_key: &[u8],
    profile_version_arn: &str,
    job_id: &str,
) -> Vec<u8> {
    let envelope = ::serde_json::json!({
        "profileVersionArn": profile_version_arn,
        "jobId": job_id,
        // The digest of what was signed, so a verifier can tell the envelope
        // apart from one made over other bytes without re-reading the source.
        "payloadSha384": ::hex::encode(sha2::Sha384::digest(source)),
        // The DER `Ecdsa-Sig-Value` the provider produced, and the DER X.509
        // `SubjectPublicKeyInfo` it is verified with.
        "signature": ::hex::encode(signature),
        "publicKey": ::hex::encode(public_key),
    });
    // A JSON document always renders, and the fallback is never a document a
    // verifier could mistake for one: the only member that could fail to render
    // is a non-finite number, and this object carries none.
    ::serde_json::to_vec(&envelope).unwrap_or_default()
}

/// The signed artifact: the source archive's entries, then the signature
/// envelope as one added entry.
///
/// CPU-bound and synchronous: it is called from the blocking worker
/// [`signed_artifact`] hands the work to, never from the thread serving a
/// request.
///
/// `Err` is the sentence the job fails with: a source object that is not a
/// readable zip archive — the one thing this function judges — signs nothing, and
/// so does one past the bounds this emulator holds caller content to.
fn signed_archive(source: &[u8], envelope: &[u8]) -> Result<Vec<u8>, String> {
    let mut archive =
        ::zip::ZipArchive::new(Cursor::new(source)).map_err(|err| not_an_archive(&err))?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(format!(
            "the source archive carries {} entries, past the {MAX_ARCHIVE_ENTRIES} entries this \
             emulator signs",
            archive.len()
        ));
    }
    let mut signed = ::zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = ::zip::write::SimpleFileOptions::default();
    let mut read = 0usize;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|err| not_an_archive(&err))?;
        // zip reports an entry's name as a `Result`, because a name that is not
        // valid UTF-8 cannot be written back out under that name: such an archive
        // is refused rather than signed with a lossily decoded entry in it.
        let name = entry
            .name()
            .map_err(|err| format!("a source archive entry's name could not be read: {err}"))?
            .to_string();
        if name == SIGNATURE_ENTRY_NAME {
            return Err(format!(
                "the source archive already carries a {SIGNATURE_ENTRY_NAME} entry"
            ));
        }
        if name.ends_with('/') {
            signed
                .add_directory(name, options)
                .map_err(|err| artifact_not_built(&err))?;
            continue;
        }
        // The remaining budget plus one byte: reading MORE than the budget is
        // what says the entry is over it, and the limited reader stops there
        // whatever the entry's header claimed.
        let allowed = MAX_ARCHIVE_BYTES.saturating_sub(read).min(MAX_ENTRY_BYTES);
        let mut bytes = Vec::new();
        ::std::io::Read::take(&mut entry, allowed as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|err| not_an_archive(&err))?;
        if bytes.len() > allowed {
            return Err(format!(
                "the source archive expands past the {MAX_ARCHIVE_BYTES} bytes this emulator \
                 signs"
            ));
        }
        read += bytes.len();
        signed
            .start_file(name, options)
            .map_err(|err| artifact_not_built(&err))?;
        signed
            .write_all(&bytes)
            .map_err(|err| artifact_not_built(&err))?;
    }
    signed
        .start_file(SIGNATURE_ENTRY_NAME, options)
        .map_err(|err| artifact_not_built(&err))?;
    signed
        .write_all(envelope)
        .map_err(|err| artifact_not_built(&err))?;
    let built = signed.finish().map_err(|err| artifact_not_built(&err))?;
    Ok(built.into_inner())
}

/// Write `artifact` at the destination `destination` names for the job `job_id`,
/// and report where it landed.
///
/// A WRITE of Amazon S3, and the one place the signed object's location is
/// decided, so what the job reports is the object this call actually wrote:
/// `Err` — a destination Amazon S3 would not take the object into — leaves the
/// job with no signed object at all.
pub async fn write_signed_object(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    destination: &Destination,
    job_id: &str,
    artifact: Vec<u8>,
) -> Result<SignedObject, String> {
    let s3 = destination.s3.as_ref();
    let Some(bucket_name) = s3.and_then(|s3| s3.bucket_name.clone()) else {
        return Err("the job names no destination bucket to write the signed object to".to_string());
    };
    // "The destination contains the name of your bucket and an optional prefix"
    // (`StartSigningJobRequest$destination`), and Signer names the object it
    // writes for the job that produced it — so the key is the prefix the request
    // carried followed by the job id.
    let prefix = s3.and_then(|s3| s3.prefix.clone()).unwrap_or_default();
    let key = format!("{prefix}{job_id}");
    let written = client(ctx, region)
        .put_object()
        .bucket(&bucket_name)
        .key(&key)
        .body(::aws_sdk_s3::primitives::ByteStream::from(artifact))
        .send()
        .await;
    if let Err(error) = written {
        return Err(refusal_message("PutObject", &error_metadata(&error)));
    }
    Ok(SignedObject {
        s3: Some(S3SignedObject {
            bucket_name: Some(bucket_name),
            key: Some(key),
        }),
    })
}

/// The `(code, message)` Amazon S3 classified and worded a refusal under, or
/// `None` for an answer that identified no refusal of its own.
fn error_metadata<E, R>(error: &::aws_smithy_runtime_api::client::result::SdkError<E, R>)
-> Option<(String, String)>
where
    E: ::aws_sdk_s3::error::ProvideErrorMetadata,
{
    let service = error.as_service_error()?;
    let code = ::aws_sdk_s3::error::ProvideErrorMetadata::code(service)?;
    let message = ::aws_sdk_s3::error::ProvideErrorMetadata::message(service).unwrap_or(code);
    Some((code.to_string(), message.to_string()))
}

/// The sentence a job that could not move an object through Amazon S3 fails
/// with: the call it made, and the two parts AMAZON S3 itself worded — the code
/// it classified the refusal under and the message it answered. An answer that
/// identified no refusal is reported as the call that did not complete, because
/// that is all this emulator knows about it.
fn refusal_message(call: &str, refusal: &Option<(String, String)>) -> String {
    match refusal {
        Some((code, message)) => format!(
            "Error occurred while {call}. S3 Error Code: {code}. S3 Error Message: {message}"
        ),
        None => format!("Error occurred while {call}. The call did not complete."),
    }
}

/// The sentence a job whose source is not a readable zip archive fails with.
///
/// "Your code signing job is signing a .zip file archive" is the shape of a
/// code-signing source, so an object no zip reader can open signs nothing; the
/// reader's own words say what was wrong with it.
fn not_an_archive(err: &impl ::std::fmt::Display) -> String {
    format!("The source object is not a valid zip archive: {err}")
}

/// The sentence a job whose artifact could not be assembled fails with — a
/// writer this process could not write to, which is this emulator's own fault
/// rather than anything about the request, and still means nothing was signed.
fn artifact_not_built(err: &impl ::std::fmt::Display) -> String {
    format!("The signed archive could not be assembled: {err}")
}

/// An Amazon S3 client over the in-process callback into this same emulator,
/// signing as the emulator's own credentials — the read and the write SIGNER
/// itself makes of a job's objects.
///
/// The ONE construction every emulated service reaches Amazon S3 through
/// ([`::service_common::s3_client::s3_client`]): the transport is in process, so
/// no socket is opened and no name is resolved, and the endpoint URL is
/// addressing for that transport rather than a host this process connects to.
fn client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    ::service_common::s3_client::s3_client(ctx.callback(), region)
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use super::{refusal_message, signature_envelope, signed_archive, SIGNATURE_ENTRY_NAME};

    /// One row of the artifact table: the case, and the entries the source
    /// archive carries as `(name, bytes)`.
    type ArchiveRow<'a> = (&'a str, Vec<(&'a str, &'a [u8])>);

    /// One row of the refusal table: the case, the `(code, message)` Amazon S3
    /// identified (`None` for an answer that identified none), and the sentence
    /// the job fails with.
    type RefusalRow<'a> = (&'a str, Option<(String, String)>, &'a str);

    /// A zip archive carrying `entries`, for the rows below.
    fn archive(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = ::zip::ZipWriter::new(::std::io::Cursor::new(Vec::new()));
        for (name, body) in entries {
            writer
                .start_file(*name, ::zip::write::SimpleFileOptions::default())
                .expect("the entry starts");
            writer.write_all(body).expect("the entry is written");
        }
        writer
            .finish()
            .expect("the archive is finished")
            .into_inner()
    }

    /// The signed artifact carries every entry of the source archive, with its
    /// bytes, plus the signature envelope — one row per archive shape.
    #[test]
    fn the_signed_archive_carries_the_source_entries_and_the_envelope() {
        // (case, the source archive's entries)
        let rows: Vec<ArchiveRow<'_>> = vec![
            ("one entry", vec![("index.js", b"exports.handler = 1;".as_ref())]),
            (
                "three entries, one of them empty",
                vec![
                    ("a.txt", b"a".as_ref()),
                    ("nested/b.txt", b"bb".as_ref()),
                    ("empty.txt", b"".as_ref()),
                ],
            ),
            ("no entries at all", vec![]),
        ];
        let envelope = b"{\"signature\":\"00\"}";
        for (case, entries) in rows {
            let source = archive(&entries);
            let signed = signed_archive(&source, envelope)
                .unwrap_or_else(|err| panic!("case {case:?}: expected a signed archive, got {err}"));
            let mut built = ::zip::ZipArchive::new(::std::io::Cursor::new(signed))
                .unwrap_or_else(|err| panic!("case {case:?}: the artifact must be a zip: {err}"));
            let mut names: Vec<String> = Vec::new();
            for index in 0..built.len() {
                let entry = built.by_index(index).expect("the entry reads");
                names.push(entry.name().expect("the name reads").to_string());
            }
            for (name, body) in &entries {
                assert!(
                    names.contains(&(*name).to_string()),
                    "case {case:?}: expected the entry {name:?} in the artifact, got {names:?}"
                );
                let mut carried = Vec::new();
                ::std::io::Read::read_to_end(
                    &mut built.by_name(name).expect("the entry is held"),
                    &mut carried,
                )
                .expect("the entry reads");
                assert_eq!(
                    carried, *body,
                    "case {case:?}: expected the entry {name:?} to carry its own bytes, got \
                     {carried:?}"
                );
            }
            let mut carried = Vec::new();
            ::std::io::Read::read_to_end(
                &mut built
                    .by_name(SIGNATURE_ENTRY_NAME)
                    .expect("the envelope is held"),
                &mut carried,
            )
            .expect("the envelope reads");
            assert_eq!(
                carried, envelope,
                "case {case:?}: expected the envelope entry to carry the envelope, got {carried:?}"
            );
        }
    }

    /// A source object no zip reader can open signs nothing.
    #[test]
    fn a_source_that_is_not_an_archive_signs_nothing() {
        // (case, the source object's bytes)
        let rows: [(&str, &[u8]); 4] = [
            ("plain text", b"this is not a valid zip archive"),
            ("empty", b""),
            ("a truncated local header", b"PK\x03\x04"),
            ("a gzip member", b"\x1f\x8b\x08\x00\x00\x00\x00\x00"),
        ];
        for (case, source) in rows {
            let outcome = signed_archive(source, b"{}");
            assert!(
                outcome.is_err(),
                "case {case:?}: expected no signed archive, got {:?}",
                outcome.map(|artifact| artifact.len())
            );
        }
    }

    /// The envelope is a JSON document carrying the job's own values.
    #[test]
    fn the_envelope_states_what_was_signed_and_what_signed_it() {
        let envelope = signature_envelope(
            b"payload",
            b"\x01\x02",
            b"\x03\x04",
            "arn:aws:signer:us-west-2:000000000000:/signing-profiles/P/0000000001",
            "job-1",
        );
        let parsed: ::serde_json::Value =
            ::serde_json::from_slice(&envelope).expect("the envelope is JSON");
        // (the member, the value it must carry)
        let rows: [(&str, &str); 4] = [
            (
                "profileVersionArn",
                "arn:aws:signer:us-west-2:000000000000:/signing-profiles/P/0000000001",
            ),
            ("jobId", "job-1"),
            ("signature", "0102"),
            ("publicKey", "0304"),
        ];
        for (member, expected) in rows {
            let carried = parsed[member].as_str().unwrap_or_default();
            assert_eq!(
                carried, expected,
                "member {member:?}: expected {expected:?}, got {carried:?}"
            );
        }
        let digest = parsed["payloadSha384"].as_str().unwrap_or_default();
        assert_eq!(
            digest,
            ::hex::encode(<sha2::Sha384 as sha2::Digest>::digest(b"payload")),
            "expected the digest of the signed bytes, got {digest:?}"
        );
    }

    /// A refusal states the call and what Amazon S3 said, and an answer that said
    /// nothing is reported as the call that did not complete.
    #[test]
    fn a_refusal_reports_the_call_and_what_amazon_s3_said() {
        // (case, what Amazon S3 identified, the sentence the job fails with)
        let rows: [RefusalRow<'_>; 3] = [
            (
                "a key the bucket holds nothing under",
                Some((
                    "NoSuchKey".to_string(),
                    "The specified key does not exist.".to_string(),
                )),
                "Error occurred while GetObject. S3 Error Code: NoSuchKey. S3 Error Message: The \
                 specified key does not exist.",
            ),
            (
                "a bucket this store holds no record of",
                Some((
                    "NoSuchBucket".to_string(),
                    "The specified bucket does not exist".to_string(),
                )),
                "Error occurred while GetObject. S3 Error Code: NoSuchBucket. S3 Error Message: \
                 The specified bucket does not exist",
            ),
            (
                "an answer that identified no refusal",
                None,
                "Error occurred while GetObject. The call did not complete.",
            ),
        ];
        for (case, refusal, expected) in rows {
            let worded = refusal_message("GetObject", &refusal);
            assert_eq!(
                worded, expected,
                "case {case:?}: expected {expected:?}, got {worded:?}"
            );
        }
    }
}
