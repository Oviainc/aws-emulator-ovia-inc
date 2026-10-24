// SPDX-License-Identifier: CC0-1.0

//! The vault + archive + retrieval-job slice, driven end to end over the wire.
//!
//! Every case goes through the real router: an HTTP request in, the serialized restJson1
//! response out. Nothing reaches into the store, so a hollow handler cannot pass.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! case:
//!
//! 1. An archive's `x-amz-sha256-tree-hash` is the SHA-256 tree hash of the bytes the
//!    service stored, for content of any length.
//! 2. `DescribeVault`/`ListVaults` report `NumberOfArchives`/`SizeInBytes` counted and
//!    summed from the archives actually in the vault.
//! 3. An archive is readable only through a retrieval job, and what `GetJobOutput`
//!    returns is byte-for-byte what `UploadArchive` received.
//! 4. A job is `InProgress` until the completion the clock owes it, then `Succeeded`.
//! 5. A `Range` request returns exactly that slice, as a 206 with its `Content-Range`,
//!    and reports a checksum only for a tree-hash-aligned range.
//! 6. Listing pages through every record exactly once at any page size.
//! 7. A request naming a vault, archive or job that is not stored is refused with the
//!    error the model declares for it.
//! 8. A byte range that is not a range of the content — inverted, past the end or
//!    saturated — is refused, and the number of retained retrieval jobs is bounded.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::{Request, Response};
use serde_json::Value;
use sha2::{Digest, Sha256};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_glacier_glacier::EmulatedService;

/// The chunk the SHA-256 tree hash is built over, and the alignment unit a retrieval
/// range must respect. Source: `JobParameters$RetrievalByteRange` — "the byte range must
/// be megabyte (1024*1024) aligned".
const MIB: usize = 1024 * 1024;

/// Long enough to cover the span `crate::state::JOB_COMPLETION_DELAY` sets (500 ms)
/// without making the suite slow.
const SETTLE_WAIT: Duration = Duration::from_millis(900);

/// The account label the SDKs default to; `-` means "the account that signed this".
const CALLER_ACCOUNT: &str = "-";

/// The canonical account id every emulator presents, and the region these requests are
/// signed for — both appear in the ARNs the service builds.
fn account_id() -> &'static str {
    engine::emulator_account_id()
}
const REGION: &str = "us-west-2";

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These requests carry an unsigned, syntactically valid credential scope: it is
        // what routes them to `glacier`, not what authenticates them.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn send(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Response<Bytes> {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header("host", format!("glacier.{REGION}.amazonaws.com"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDGLACIERTEST/20261021/{REGION}/glacier/aws4_request, \
                     SignedHeaders=host, Signature=0000"
                ),
            );
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(Bytes::copy_from_slice(body))
            .expect("a well-formed test request");
        self.router.dispatch(request).await
    }

    async fn create_vault(&self, account: &str, vault: &str) -> Response<Bytes> {
        self.send("PUT", &format!("/{account}/vaults/{vault}"), &[], &[])
            .await
    }

    async fn upload(
        &self,
        vault: &str,
        body: &[u8],
        extra: &[(&str, &str)],
    ) -> Response<Bytes> {
        let mut headers = vec![("content-type", "application/octet-stream")];
        headers.extend_from_slice(extra);
        self.send(
            "POST",
            &format!("/{CALLER_ACCOUNT}/vaults/{vault}/archives"),
            &headers,
            body,
        )
        .await
    }

    async fn describe_vault(&self, vault: &str) -> Response<Bytes> {
        self.send(
            "GET",
            &format!("/{CALLER_ACCOUNT}/vaults/{vault}"),
            &[],
            &[],
        )
        .await
    }

    async fn initiate_job(&self, vault: &str, parameters: &Value) -> Response<Bytes> {
        self.send(
            "POST",
            &format!("/{CALLER_ACCOUNT}/vaults/{vault}/jobs"),
            &[("content-type", "application/json")],
            serde_json::to_vec(parameters)
                .expect("the test's job parameters serialize")
                .as_slice(),
        )
        .await
    }

    async fn describe_job(&self, vault: &str, job_id: &str) -> Response<Bytes> {
        self.send(
            "GET",
            &format!("/{CALLER_ACCOUNT}/vaults/{vault}/jobs/{job_id}"),
            &[],
            &[],
        )
        .await
    }

    async fn job_output(&self, vault: &str, job_id: &str, range: Option<&str>) -> Response<Bytes> {
        let headers: Vec<(&str, &str)> = range.map(|r| vec![("range", r)]).unwrap_or_default();
        self.send(
            "GET",
            &format!("/{CALLER_ACCOUNT}/vaults/{vault}/jobs/{job_id}/output"),
            &headers,
            &[],
        )
        .await
    }
}

fn status(resp: &Response<Bytes>) -> u16 {
    resp.status().as_u16()
}

fn header(resp: &Response<Bytes>, name: &str) -> Option<String> {
    resp.headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

fn json(resp: &Response<Bytes>) -> Value {
    serde_json::from_slice(resp.body()).unwrap_or(Value::Null)
}

/// The wire error code: restJson1 carries it in `x-amzn-errortype`, with `__type` in the
/// body as the fallback.
fn error_code(resp: &Response<Bytes>) -> String {
    header(resp, "x-amzn-errortype")
        .or_else(|| {
            json(resp)
                .get("__type")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default()
}

fn expect_error(case: &str, resp: &Response<Bytes>, expected_status: u16, expected_code: &str) {
    assert_eq!(
        status(resp),
        expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {} with body {:?}",
        status(resp),
        String::from_utf8_lossy(resp.body())
    );
    assert_eq!(
        error_code(resp),
        expected_code,
        "case {case:?}: expected error code {expected_code}, got {:?}",
        error_code(resp)
    );
}

/// The SHA-256 tree hash, written independently of the implementation under test: an
/// explicit index walk over the levels rather than the production chunk/fold.
fn expected_tree_hash(data: &[u8]) -> String {
    let mut level: Vec<Vec<u8>> = Vec::new();
    let mut offset = 0;
    while offset < data.len() {
        let end = (offset + MIB).min(data.len());
        level.push(Sha256::digest(&data[offset..end]).to_vec());
        offset = end;
    }
    if level.is_empty() {
        level.push(Sha256::digest([]).to_vec());
    }
    while level.len() > 1 {
        let mut parents: Vec<Vec<u8>> = Vec::new();
        let mut i = 0;
        while i < level.len() {
            if i + 1 < level.len() {
                let mut hasher = Sha256::new();
                hasher.update(&level[i]);
                hasher.update(&level[i + 1]);
                parents.push(hasher.finalize().to_vec());
            } else {
                parents.push(level[i].clone());
            }
            i += 2;
        }
        level = parents;
    }
    hex::encode(&level[0])
}

/// Deterministic pseudo-random content of `len` bytes, so a case's payload is neither a
/// repeated byte (which hides a mis-ordered chunk) nor random between runs.
fn payload(len: usize, seed: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut x = seed as u32 | 1;
    for _ in 0..len {
        x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        out.push((x >> 16) as u8);
    }
    out
}

/// Run a job to completion and return its id: initiate, then wait out the span the clock
/// owes before the output exists.
async fn completed_job(emulator: &Emulator, vault: &str, parameters: &Value) -> String {
    let resp = emulator.initiate_job(vault, parameters).await;
    assert_eq!(
        status(&resp),
        202,
        "InitiateJob({parameters}) must be accepted, got {} with body {:?}",
        status(&resp),
        String::from_utf8_lossy(resp.body())
    );
    let job_id = header(&resp, "x-amz-job-id").unwrap_or_default();
    assert!(
        !job_id.is_empty(),
        "InitiateJob({parameters}) must mint a job id in x-amz-job-id"
    );
    tokio::time::sleep(SETTLE_WAIT).await;
    job_id
}

// ---------------------------------------------------------------------------
// 1. The tree hash is computed over the stored bytes, at every content length
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_reported_checksum_is_the_tree_hash_of_the_stored_bytes() {
    let emulator = Emulator::new();
    assert_eq!(status(&emulator.create_vault(CALLER_ACCOUNT, "hashes").await), 201);

    // Content under one chunk hashes to its plain SHA-256 — a known answer that does not
    // depend on the tree walk at all.
    let resp = emulator.upload("hashes", b"hello", &[]).await;
    assert_eq!(
        header(&resp, "x-amz-sha256-tree-hash").unwrap_or_default(),
        hex::encode(Sha256::digest(b"hello")),
        "a single-chunk archive's tree hash is the SHA-256 of its content"
    );

    // Lengths chosen around the 1 MiB chunk boundary: either side of it, exactly on it,
    // and spans that leave the tree with an odd digest to promote.
    let lengths = [
        1usize,
        1024,
        MIB - 1,
        MIB,
        MIB + 1,
        2 * MIB,
        2 * MIB + 7,
        3 * MIB,
    ];
    let mut running_count = 1i64;
    let mut running_size = 5i64;
    for (seed, len) in lengths.iter().enumerate() {
        let case = format!("{len} bytes");
        let body = payload(*len, seed as u8 + 1);
        let resp = emulator.upload("hashes", &body, &[]).await;
        assert_eq!(
            status(&resp),
            201,
            "case {case:?}: expected HTTP 201, got {} with body {:?}",
            status(&resp),
            String::from_utf8_lossy(resp.body())
        );
        let reported = header(&resp, "x-amz-sha256-tree-hash").unwrap_or_default();
        assert_eq!(
            reported,
            expected_tree_hash(&body),
            "case {case:?}: expected the tree hash of the stored bytes, got {reported:?}"
        );
        let archive_id = header(&resp, "x-amz-archive-id").unwrap_or_default();
        assert!(
            !archive_id.is_empty(),
            "case {case:?}: expected a minted x-amz-archive-id, got nothing"
        );
        assert_eq!(
            header(&resp, "location").unwrap_or_default(),
            format!("/{}/vaults/hashes/archives/{archive_id}", account_id()),
            "case {case:?}: Location must address the archive that was minted"
        );

        // The counts track what is stored, call after call.
        running_count += 1;
        running_size += i64::try_from(*len).expect("a test payload fits in i64");
        let described = json(&emulator.describe_vault("hashes").await);
        assert_eq!(
            described["NumberOfArchives"].as_i64(),
            Some(running_count),
            "case {case:?}: expected NumberOfArchives {running_count}, got {:?}",
            described["NumberOfArchives"]
        );
        assert_eq!(
            described["SizeInBytes"].as_i64(),
            Some(running_size),
            "case {case:?}: expected SizeInBytes {running_size}, got {:?}",
            described["SizeInBytes"]
        );
    }
}

#[tokio::test]
async fn a_checksum_the_client_claims_is_verified_against_the_content() {
    let emulator = Emulator::new();
    assert_eq!(status(&emulator.create_vault(CALLER_ACCOUNT, "claims").await), 201);

    let body = payload(4096, 9);
    let truth = expected_tree_hash(&body);
    // Rows: the right claim, the hash of OTHER content, and a syntactically plausible
    // but wrong digest. Only the first may be accepted.
    let wrong_other = expected_tree_hash(b"something else entirely");
    let cases: [(&str, &str, u16); 3] = [
        ("the content's own tree hash", truth.as_str(), 201),
        ("another content's tree hash", wrong_other.as_str(), 400),
        ("a zeroed digest", &"0".repeat(64), 400),
    ];
    for (case, claimed, expected) in cases {
        let resp = emulator
            .upload("claims", &body, &[("x-amz-sha256-tree-hash", claimed)])
            .await;
        assert_eq!(
            status(&resp),
            expected,
            "case {case:?}: expected HTTP {expected}, got {} with body {:?}",
            status(&resp),
            String::from_utf8_lossy(resp.body())
        );
        if expected == 201 {
            assert_eq!(
                header(&resp, "x-amz-sha256-tree-hash").unwrap_or_default(),
                truth,
                "case {case:?}: the reported checksum is computed, not echoed"
            );
        } else {
            assert_eq!(
                error_code(&resp),
                "InvalidParameterValueException",
                "case {case:?}: a checksum mismatch is refused as an invalid parameter"
            );
        }
    }

    // Only the accepted upload is stored.
    let described = json(&emulator.describe_vault("claims").await);
    assert_eq!(
        described["NumberOfArchives"].as_i64(),
        Some(1),
        "a refused upload must not be stored, got {described}"
    );
}

// ---------------------------------------------------------------------------
// 2. The numbered workflow, over a table of payloads
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_retrieval_workflow_round_trips_every_uploaded_archive() {
    // Rows differ in the thing the rule generalises over: content length, and so the
    // shape of the hash tree and the number of retrievable chunks.
    let rows: [(&str, usize); 3] = [
        ("one-byte archive", 1),
        ("sub-chunk archive", 11),
        ("two-chunk archive", 2 * MIB),
    ];

    for (case, len) in rows {
        let emulator = Emulator::new();
        let vault = "workflow";
        let body = payload(len, 7);
        let tree_hash = expected_tree_hash(&body);

        // 1. CreateVault -> 201 + Location.
        let resp = emulator.create_vault(CALLER_ACCOUNT, vault).await;
        assert_eq!(
            status(&resp),
            201,
            "case {case:?}: CreateVault must answer 201, got {}",
            status(&resp)
        );
        assert_eq!(
            header(&resp, "location").unwrap_or_default(),
            format!("/{}/vaults/{vault}", account_id()),
            "case {case:?}: CreateVault's Location names the created vault"
        );

        // 2. UploadArchive -> 201 + minted id + computed hash.
        let resp = emulator
            .upload(vault, &body, &[("x-amz-archive-description", "a test archive")])
            .await;
        assert_eq!(status(&resp), 201, "case {case:?}: UploadArchive must answer 201");
        let archive_id = header(&resp, "x-amz-archive-id").unwrap_or_default();

        // 3. DescribeVault -> the computed count and size.
        let described = json(&emulator.describe_vault(vault).await);
        assert_eq!(
            described["NumberOfArchives"].as_i64(),
            Some(1),
            "case {case:?}: expected NumberOfArchives 1, got {described}"
        );
        assert_eq!(
            described["SizeInBytes"].as_i64(),
            Some(i64::try_from(len).expect("fits")),
            "case {case:?}: expected SizeInBytes {len}, got {described}"
        );
        assert_eq!(
            described["VaultARN"].as_str(),
            Some(
                format!(
                    "arn:aws:glacier:{REGION}:{}:vaults/{vault}",
                    account_id()
                )
                .as_str()
            ),
            "case {case:?}: expected the vault's ARN, got {described}"
        );
        assert!(
            described["CreationDate"]
                .as_str()
                .is_some_and(|d| d.starts_with("20") && d.ends_with('Z')),
            "case {case:?}: expected an ISO-8601 CreationDate, got {described}"
        );

        // 4. ListVaults -> the same record as DescribeVault.
        let listed = json(
            &emulator
                .send("GET", &format!("/{CALLER_ACCOUNT}/vaults"), &[], &[])
                .await,
        );
        assert_eq!(
            listed["VaultList"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: expected the vault in VaultList, got {listed}"
        );
        assert_eq!(
            listed["VaultList"][0], described,
            "case {case:?}: ListVaults and DescribeVault must render one record"
        );

        // 5. InitiateJob -> 202 + a job that is not finished yet.
        let parameters = serde_json::json!({
            "Type": "archive-retrieval",
            "ArchiveId": archive_id,
            "Description": "retrieve it",
        });
        let resp = emulator.initiate_job(vault, &parameters).await;
        assert_eq!(status(&resp), 202, "case {case:?}: InitiateJob must answer 202");
        let job_id = header(&resp, "x-amz-job-id").unwrap_or_default();
        assert!(
            !job_id.is_empty(),
            "case {case:?}: InitiateJob must mint a job id"
        );
        assert_eq!(
            header(&resp, "location").unwrap_or_default(),
            format!("/{}/vaults/{vault}/jobs/{job_id}", account_id()),
            "case {case:?}: InitiateJob's Location names the job that was recorded"
        );

        // The job is reported InProgress until the clock owes its completion, and its
        // output cannot be downloaded before then.
        let in_progress = json(&emulator.describe_job(vault, &job_id).await);
        assert_eq!(
            in_progress["StatusCode"].as_str(),
            Some("InProgress"),
            "case {case:?}: a job just initiated is InProgress, got {in_progress}"
        );
        assert_eq!(
            in_progress["Completed"].as_bool(),
            Some(false),
            "case {case:?}: a job just initiated is not Completed, got {in_progress}"
        );
        let early = emulator.job_output(vault, &job_id, None).await;
        expect_error(
            case,
            &early,
            400,
            "InvalidParameterValueException",
        );

        tokio::time::sleep(SETTLE_WAIT).await;

        // 6. DescribeJob -> read back from the stored job.
        let job = json(&emulator.describe_job(vault, &job_id).await);
        assert_eq!(
            job["StatusCode"].as_str(),
            Some("Succeeded"),
            "case {case:?}: the completion the clock owed must be materialised, got {job}"
        );
        assert_eq!(
            job["Completed"].as_bool(),
            Some(true),
            "case {case:?}: expected Completed true, got {job}"
        );
        assert_eq!(
            job["Action"].as_str(),
            Some("ArchiveRetrieval"),
            "case {case:?}: expected Action ArchiveRetrieval, got {job}"
        );
        assert_eq!(
            job["JobId"].as_str(),
            Some(job_id.as_str()),
            "case {case:?}: DescribeJob must answer for the job that was minted"
        );
        assert_eq!(
            job["ArchiveId"].as_str(),
            Some(archive_id.as_str()),
            "case {case:?}: expected the retrieved archive's id, got {job}"
        );
        assert_eq!(
            job["ArchiveSizeInBytes"].as_i64(),
            Some(i64::try_from(len).expect("fits")),
            "case {case:?}: expected ArchiveSizeInBytes {len}, got {job}"
        );
        assert_eq!(
            job["ArchiveSHA256TreeHash"].as_str(),
            Some(tree_hash.as_str()),
            "case {case:?}: expected the archive's tree hash, got {job}"
        );
        assert_eq!(
            job["SHA256TreeHash"].as_str(),
            Some(tree_hash.as_str()),
            "case {case:?}: a whole-archive retrieval hashes to the archive's own hash"
        );
        assert_eq!(
            job["Tier"].as_str(),
            Some("Standard"),
            "case {case:?}: Standard is the documented default tier, got {job}"
        );
        assert!(
            job["CompletionDate"].as_str().is_some(),
            "case {case:?}: a completed job carries a CompletionDate, got {job}"
        );
        assert_eq!(
            job["VaultARN"].as_str(),
            described["VaultARN"].as_str(),
            "case {case:?}: the job names the vault it belongs to"
        );

        // 7. ListJobs -> the same record as DescribeJob.
        let jobs = json(
            &emulator
                .send(
                    "GET",
                    &format!("/{CALLER_ACCOUNT}/vaults/{vault}/jobs"),
                    &[],
                    &[],
                )
                .await,
        );
        assert_eq!(
            jobs["JobList"].as_array().map(Vec::len),
            Some(1),
            "case {case:?}: expected the job in JobList, got {jobs}"
        );
        assert_eq!(
            jobs["JobList"][0], job,
            "case {case:?}: ListJobs and DescribeJob must render one record"
        );

        // 8. GetJobOutput -> the stored bytes, byte for byte.
        let out = emulator.job_output(vault, &job_id, None).await;
        assert_eq!(status(&out), 200, "case {case:?}: GetJobOutput must answer 200");
        assert_eq!(
            out.body().as_ref(),
            body.as_slice(),
            "case {case:?}: the retrieved bytes must equal the uploaded bytes"
        );
        assert_eq!(
            header(&out, "x-amz-sha256-tree-hash").unwrap_or_default(),
            tree_hash,
            "case {case:?}: the whole output reports the archive's tree hash"
        );
        assert_eq!(
            header(&out, "accept-ranges").as_deref(),
            Some("bytes"),
            "case {case:?}: GetJobOutput accepts ranges"
        );
        assert_eq!(
            header(&out, "content-type").as_deref(),
            Some("application/octet-stream"),
            "case {case:?}: archive data is octet-stream"
        );
        assert_eq!(
            header(&out, "x-amz-archive-description").as_deref(),
            Some("a test archive"),
            "case {case:?}: the archive's stored description is reported"
        );

        // 9. DeleteArchive -> the counts drop with it.
        let resp = emulator
            .send(
                "DELETE",
                &format!("/{CALLER_ACCOUNT}/vaults/{vault}/archives/{archive_id}"),
                &[],
                &[],
            )
            .await;
        assert_eq!(status(&resp), 204, "case {case:?}: DeleteArchive must answer 204");
        let described = json(&emulator.describe_vault(vault).await);
        assert_eq!(
            described["NumberOfArchives"].as_i64(),
            Some(0),
            "case {case:?}: expected NumberOfArchives 0 after the delete, got {described}"
        );
        assert_eq!(
            described["SizeInBytes"].as_i64(),
            Some(0),
            "case {case:?}: expected SizeInBytes 0 after the delete, got {described}"
        );

        // 10. DeleteVault -> the vault is really gone.
        let resp = emulator
            .send(
                "DELETE",
                &format!("/{CALLER_ACCOUNT}/vaults/{vault}"),
                &[],
                &[],
            )
            .await;
        assert_eq!(status(&resp), 204, "case {case:?}: DeleteVault must answer 204");
        expect_error(
            case,
            &emulator.describe_vault(vault).await,
            404,
            "ResourceNotFoundException",
        );
        let listed = json(
            &emulator
                .send("GET", &format!("/{CALLER_ACCOUNT}/vaults"), &[], &[])
                .await,
        );
        assert_eq!(
            listed["VaultList"].as_array().map(Vec::len),
            Some(0),
            "case {case:?}: a deleted vault is no longer listed, got {listed}"
        );
    }
}

// ---------------------------------------------------------------------------
// 3. Ranges
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_range_request_returns_exactly_that_slice() {
    let emulator = Emulator::new();
    let vault = "ranges";
    assert_eq!(status(&emulator.create_vault(CALLER_ACCOUNT, vault).await), 201);

    // Two archives: one smaller than a chunk, one exactly two chunks, so both the
    // unaligned and the aligned branch of the checksum rule are exercised.
    let small = payload(8, 3);
    let large = payload(2 * MIB, 4);
    let mut ids = Vec::new();
    for body in [&small, &large] {
        let resp = emulator.upload(vault, body, &[]).await;
        assert_eq!(status(&resp), 201);
        ids.push(header(&resp, "x-amz-archive-id").unwrap_or_default());
    }

    // (case, archive index, Range header, expected status, expected Content-Range,
    //  expected slice, whether a checksum header is expected)
    #[allow(clippy::type_complexity)]
    let rows: Vec<(&str, usize, Option<String>, u16, Option<String>, (usize, usize), bool)> = vec![
        ("no range at all", 0, None, 200, None, (0, 8), true),
        (
            "a range covering the whole archive",
            0,
            Some("bytes=0-7".to_string()),
            206,
            Some("bytes 0-7/8".to_string()),
            (0, 8),
            true,
        ),
        (
            "a short unaligned prefix",
            0,
            Some("bytes=0-3".to_string()),
            206,
            Some("bytes 0-3/8".to_string()),
            (0, 4),
            false,
        ),
        (
            "an open-ended unaligned suffix",
            0,
            Some("bytes=4-".to_string()),
            206,
            Some("bytes 4-7/8".to_string()),
            (4, 8),
            false,
        ),
        (
            "an aligned leading chunk",
            1,
            Some(format!("bytes=0-{}", MIB - 1)),
            206,
            Some(format!("bytes 0-{}/{}", MIB - 1, 2 * MIB)),
            (0, MIB),
            true,
        ),
        (
            "an aligned trailing chunk",
            1,
            Some(format!("bytes={MIB}-")),
            206,
            Some(format!("bytes {}-{}/{}", MIB, 2 * MIB - 1, 2 * MIB)),
            (MIB, 2 * MIB),
            true,
        ),
    ];

    for (case, which, range, expected_status, expected_content_range, (from, to), checksum) in rows {
        let content: &[u8] = if which == 0 { &small } else { &large };
        let job_id = completed_job(
            &emulator,
            vault,
            &serde_json::json!({ "Type": "archive-retrieval", "ArchiveId": ids[which] }),
        )
        .await;
        let out = emulator.job_output(vault, &job_id, range.as_deref()).await;
        assert_eq!(
            status(&out),
            expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {}",
            status(&out)
        );
        assert_eq!(
            header(&out, "content-range"),
            expected_content_range,
            "case {case:?}: expected Content-Range {expected_content_range:?}, got {:?}",
            header(&out, "content-range")
        );
        assert_eq!(
            out.body().as_ref(),
            &content[from..to],
            "case {case:?}: expected the bytes [{from}, {to}) of the archive"
        );
        let reported = header(&out, "x-amz-sha256-tree-hash");
        if checksum {
            assert_eq!(
                reported.as_deref(),
                Some(expected_tree_hash(&content[from..to]).as_str()),
                "case {case:?}: a tree-hash-aligned range reports the hash of that range"
            );
        } else {
            assert_eq!(
                reported, None,
                "case {case:?}: an unaligned range reports no checksum, got {reported:?}"
            );
        }
    }

    // A range the output cannot satisfy is refused, not silently clamped.
    let job_id = completed_job(
        &emulator,
        vault,
        &serde_json::json!({ "Type": "archive-retrieval", "ArchiveId": ids[0] }),
    )
    .await;
    for case in ["bytes=9-20", "bytes=5-2", "rows=0-1", "0-1"] {
        expect_error(
            case,
            &emulator.job_output(vault, &job_id, Some(case)).await,
            400,
            "InvalidParameterValueException",
        );
    }
}

// ---------------------------------------------------------------------------
// 4. Inventory retrieval
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_inventory_retrieval_lists_the_vaults_stored_archives() {
    let emulator = Emulator::new();
    let vault = "inventory";
    assert_eq!(status(&emulator.create_vault(CALLER_ACCOUNT, vault).await), 201);

    let mut expected: Vec<(String, String, i64)> = Vec::new();
    for (index, len) in [3usize, 17, 64].into_iter().enumerate() {
        let body = payload(len, index as u8 + 20);
        let resp = emulator
            .upload(
                vault,
                &body,
                &[("x-amz-archive-description", "inventoried")],
            )
            .await;
        assert_eq!(status(&resp), 201);
        expected.push((
            header(&resp, "x-amz-archive-id").unwrap_or_default(),
            expected_tree_hash(&body),
            i64::try_from(len).expect("fits"),
        ));
    }

    // The documented default format, and asking for it explicitly.
    for case in ["JSON", "(default)"] {
        let mut parameters = serde_json::json!({ "Type": "inventory-retrieval" });
        if case != "(default)" {
            parameters["Format"] = Value::String(case.to_string());
        }
        let job_id = completed_job(&emulator, vault, &parameters).await;

        let job = json(&emulator.describe_job(vault, &job_id).await);
        assert_eq!(
            job["Action"].as_str(),
            Some("InventoryRetrieval"),
            "case {case:?}: expected Action InventoryRetrieval, got {job}"
        );
        assert!(
            job["InventorySizeInBytes"].as_i64().is_some_and(|n| n > 0),
            "case {case:?}: a completed inventory job reports its size, got {job}"
        );
        assert_eq!(
            job["ArchiveId"].as_str(),
            None,
            "case {case:?}: an inventory job retrieves no single archive, got {job}"
        );

        let out = emulator.job_output(vault, &job_id, None).await;
        assert_eq!(status(&out), 200, "case {case:?}: GetJobOutput must answer 200");
        let rendered = String::from_utf8_lossy(out.body()).to_string();
        assert_eq!(
            header(&out, "content-type").as_deref(),
            Some("application/json"),
            "case {case:?}: expected a JSON inventory, got {:?}",
            header(&out, "content-type")
        );
        for (archive_id, tree_hash, size) in &expected {
            assert!(
                rendered.contains(archive_id.as_str()),
                "case {case:?}: the inventory must list archive {archive_id}, got {rendered}"
            );
            assert!(
                rendered.contains(tree_hash.as_str()),
                "case {case:?}: the inventory must carry each archive's tree hash"
            );
            assert!(
                rendered.contains(&size.to_string()),
                "case {case:?}: the inventory must carry each archive's size"
            );
        }
        let parsed: Value =
            serde_json::from_str(&rendered).expect("a JSON inventory parses as JSON");
        assert_eq!(
            parsed["ArchiveList"].as_array().map(Vec::len),
            Some(expected.len()),
            "case {case:?}: expected {} inventoried archives, got {parsed}",
            expected.len()
        );

        // A completed inventory retrieval is what gives the vault a LastInventoryDate.
        let described = json(&emulator.describe_vault(vault).await);
        assert!(
            described["LastInventoryDate"].as_str().is_some(),
            "case {case:?}: a settled inventory job dates the vault's inventory, got {described}"
        );
    }

    // `CSV` is a format the model documents and this emulator does not render, so the job
    // is refused with the scaffold's 501 rather than accepted and answered with a JSON
    // document under a success status.
    let resp = emulator
        .initiate_job(
            vault,
            &serde_json::json!({ "Type": "inventory-retrieval", "Format": "CSV" }),
        )
        .await;
    expect_error("the CSV inventory format", &resp, 501, "NotImplemented");

    // And it reads the same through DescribeVault and ListVaults with no job-reporting
    // call in between: the completion the clock owed is materialised wherever it shows.
    let fresh = Emulator::new();
    assert_eq!(status(&fresh.create_vault(CALLER_ACCOUNT, vault).await), 201);
    assert_eq!(status(&fresh.upload(vault, &payload(8, 31), &[]).await), 201);
    let resp = fresh
        .initiate_job(vault, &serde_json::json!({ "Type": "inventory-retrieval" }))
        .await;
    assert_eq!(status(&resp), 202);
    let before = json(&fresh.describe_vault(vault).await);
    assert_eq!(
        before["LastInventoryDate"].as_str(),
        None,
        "case {:?}: an inventory job that has not finished dates nothing, got {before}",
        "before the job is due"
    );
    tokio::time::sleep(SETTLE_WAIT).await;
    let after = json(&fresh.describe_vault(vault).await);
    assert!(
        after["LastInventoryDate"].as_str().is_some(),
        "case {:?}: DescribeVault must materialise the owed completion itself, got {after}",
        "after the job is due"
    );
    let listed = json(
        &fresh
            .send("GET", &format!("/{CALLER_ACCOUNT}/vaults"), &[], &[])
            .await,
    );
    assert_eq!(
        listed["VaultList"][0], after,
        "ListVaults and DescribeVault must agree about the inventory date"
    );

    // The inventory is of the instant the job was initiated: an archive uploaded after
    // that is not in it, and one deleted after it still is.
    let snapshot = Emulator::new();
    assert_eq!(status(&snapshot.create_vault(CALLER_ACCOUNT, vault).await), 201);
    let resp = snapshot.upload(vault, &payload(12, 41), &[]).await;
    assert_eq!(status(&resp), 201);
    let present_at_initiation = header(&resp, "x-amz-archive-id").unwrap_or_default();
    let job_id = completed_job(
        &snapshot,
        vault,
        &serde_json::json!({ "Type": "inventory-retrieval" }),
    )
    .await;
    let resp = snapshot.upload(vault, &payload(13, 42), &[]).await;
    assert_eq!(status(&resp), 201);
    let uploaded_afterwards = header(&resp, "x-amz-archive-id").unwrap_or_default();
    assert_eq!(
        status(
            &snapshot
                .send(
                    "DELETE",
                    &format!("/{CALLER_ACCOUNT}/vaults/{vault}/archives/{present_at_initiation}"),
                    &[],
                    &[],
                )
                .await
        ),
        204
    );
    let rendered = String::from_utf8_lossy(
        snapshot.job_output(vault, &job_id, None).await.body().as_ref(),
    )
    .to_string();
    assert!(
        rendered.contains(present_at_initiation.as_str()),
        "case {:?}: the inventory is of the instant the job was initiated, got {rendered}",
        "an archive deleted after the job was initiated"
    );
    assert!(
        !rendered.contains(uploaded_afterwards.as_str()),
        "case {:?}: the inventory must not grow after the job was initiated, got {rendered}",
        "an archive uploaded after the job was initiated"
    );
}

// ---------------------------------------------------------------------------
// 5. InitiateJob's parameter rules
// ---------------------------------------------------------------------------

#[tokio::test]
async fn job_parameters_are_validated_against_the_documented_rules() {
    let emulator = Emulator::new();
    let vault = "params";
    assert_eq!(status(&emulator.create_vault(CALLER_ACCOUNT, vault).await), 201);
    let resp = emulator.upload(vault, &payload(2 * MIB, 5), &[]).await;
    assert_eq!(status(&resp), 201);
    let archive_id = header(&resp, "x-amz-archive-id").unwrap_or_default();

    // Every row names the rule it pins. 202 means the job was recorded; 4xx/501 name the
    // refusal the model declares.
    let rows: Vec<(&str, Value, u16, &str)> = vec![
        (
            "no Type at all",
            serde_json::json!({}),
            400,
            "MissingParameterValueException",
        ),
        (
            "a Type outside the documented values",
            serde_json::json!({ "Type": "tape-retrieval" }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "archive-retrieval without an ArchiveId",
            serde_json::json!({ "Type": "archive-retrieval" }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "archive-retrieval of an archive that is not in the vault",
            serde_json::json!({ "Type": "archive-retrieval", "ArchiveId": "not-an-archive" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "inventory-retrieval naming an ArchiveId",
            serde_json::json!({ "Type": "inventory-retrieval", "ArchiveId": archive_id }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "inventory-retrieval naming a RetrievalByteRange",
            serde_json::json!({ "Type": "inventory-retrieval", "RetrievalByteRange": "0-1048575" }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "a RetrievalByteRange that is not megabyte aligned",
            serde_json::json!({
                "Type": "archive-retrieval",
                "ArchiveId": archive_id,
                "RetrievalByteRange": "0-1023",
            }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "a RetrievalByteRange past the end of the archive",
            serde_json::json!({
                "Type": "archive-retrieval",
                "ArchiveId": archive_id,
                "RetrievalByteRange": "0-4194303",
            }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "a megabyte-aligned RetrievalByteRange",
            serde_json::json!({
                "Type": "archive-retrieval",
                "ArchiveId": archive_id,
                "RetrievalByteRange": "0-1048575",
            }),
            202,
            "",
        ),
        (
            "an inventory Format outside the documented values",
            serde_json::json!({ "Type": "inventory-retrieval", "Format": "XML" }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "a Tier outside the documented values",
            serde_json::json!({ "Type": "archive-retrieval", "ArchiveId": archive_id, "Tier": "Rapid" }),
            400,
            "InvalidParameterValueException",
        ),
        (
            "a select job, which this emulator runs no engine for",
            serde_json::json!({ "Type": "select", "ArchiveId": archive_id }),
            501,
            "NotImplemented",
        ),
    ];

    for (case, parameters, expected_status, expected_code) in rows {
        let resp = emulator.initiate_job(vault, &parameters).await;
        if expected_status == 202 {
            assert_eq!(
                status(&resp),
                202,
                "case {case:?}: expected HTTP 202, got {} with body {:?}",
                status(&resp),
                String::from_utf8_lossy(resp.body())
            );
        } else {
            expect_error(case, &resp, expected_status, expected_code);
        }
    }

    // Each documented tier is accepted and reported back from the stored job.
    for tier in ["Bulk", "Expedited", "Standard"] {
        let job_id = completed_job(
            &emulator,
            vault,
            &serde_json::json!({
                "Type": "archive-retrieval",
                "ArchiveId": archive_id,
                "Tier": tier,
            }),
        )
        .await;
        let job = json(&emulator.describe_job(vault, &job_id).await);
        assert_eq!(
            job["Tier"].as_str(),
            Some(tier),
            "case {tier:?}: the stored job must report the tier it was initiated with"
        );
    }

    // A tree-hash-aligned retrieval range retrieves exactly that range, and hashes it.
    let whole = payload(2 * MIB, 5);
    let job_id = completed_job(
        &emulator,
        vault,
        &serde_json::json!({
            "Type": "archive-retrieval",
            "ArchiveId": archive_id,
            "RetrievalByteRange": format!("{MIB}-{}", 2 * MIB - 1),
        }),
    )
    .await;
    let job = json(&emulator.describe_job(vault, &job_id).await);
    assert_eq!(
        job["SHA256TreeHash"].as_str(),
        Some(expected_tree_hash(&whole[MIB..]).as_str()),
        "a ranged retrieval hashes the requested range, got {job}"
    );
    assert_eq!(
        job["ArchiveSHA256TreeHash"].as_str(),
        Some(expected_tree_hash(&whole).as_str()),
        "a ranged retrieval still reports the whole archive's hash, got {job}"
    );
    let out = emulator.job_output(vault, &job_id, None).await;
    assert_eq!(
        out.body().as_ref(),
        &whole[MIB..],
        "a ranged retrieval's output is the requested range of the archive"
    );
}

// ---------------------------------------------------------------------------
// 5b. Range inventory retrieval
// ---------------------------------------------------------------------------

#[tokio::test]
async fn range_inventory_options_select_and_page_the_snapshot() {
    let emulator = Emulator::new();
    let vault = "range-inventory";
    assert_eq!(status(&emulator.create_vault(CALLER_ACCOUNT, vault).await), 201);

    // Three archives, uploaded in order, so the inventory has a known sequence.
    let mut ids = Vec::new();
    for (index, len) in [5usize, 6, 7].into_iter().enumerate() {
        let resp = emulator.upload(vault, &payload(len, index as u8 + 50), &[]).await;
        assert_eq!(status(&resp), 201);
        ids.push(header(&resp, "x-amz-archive-id").unwrap_or_default());
    }

    // `Limit` returns at most that many items and leaves a continuation Marker — "where
    // to continue pagination of the vault inventory retrieval results ... If there are no
    // more inventory items, this value is null."
    let rows: Vec<(&str, Value, usize, Option<usize>)> = vec![
        ("no options at all", serde_json::json!({}), 3, None),
        (
            "Limit 1 of three",
            serde_json::json!({ "Limit": "1" }),
            1,
            Some(1),
        ),
        (
            "Limit 2 of three",
            serde_json::json!({ "Limit": "2" }),
            2,
            Some(2),
        ),
        (
            "Limit 3 of three",
            serde_json::json!({ "Limit": "3" }),
            3,
            None,
        ),
        (
            "Limit larger than the inventory",
            serde_json::json!({ "Limit": "9" }),
            3,
            None,
        ),
        (
            "a date range that starts after every archive",
            serde_json::json!({ "StartDate": "2099-01-01T00:00:00Z" }),
            0,
            None,
        ),
        (
            "a date range that ends before every archive",
            serde_json::json!({ "EndDate": "2000-01-01T00:00:00Z" }),
            0,
            None,
        ),
        (
            "a date range that spans every archive",
            serde_json::json!({
                "StartDate": "2000-01-01T00:00:00Z",
                "EndDate": "2099-01-01T00:00:00Z",
            }),
            3,
            None,
        ),
    ];

    for (case, options, expected_items, next) in rows {
        let job_id = completed_job(
            &emulator,
            vault,
            &serde_json::json!({
                "Type": "inventory-retrieval",
                "InventoryRetrievalParameters": options,
            }),
        )
        .await;
        let document: Value = serde_json::from_slice(
            emulator.job_output(vault, &job_id, None).await.body().as_ref(),
        )
        .expect("a JSON inventory parses as JSON");
        assert_eq!(
            document["ArchiveList"].as_array().map(Vec::len),
            Some(expected_items),
            "case {case:?}: expected {expected_items} inventoried archives, got {document}"
        );
        let job = json(&emulator.describe_job(vault, &job_id).await);
        assert_eq!(
            job["InventoryRetrievalParameters"]["Marker"].as_str(),
            next.map(|index| ids[index].as_str()),
            "case {case:?}: expected the continuation marker to be {next:?}, got {job}"
        );
        assert_eq!(
            job["InventoryRetrievalParameters"]["Limit"].as_str(),
            options["Limit"].as_str(),
            "case {case:?}: the job echoes the Limit it was initiated with, got {job}"
        );
    }

    // A Marker continues where the limited job stopped, so paging the inventory one item
    // at a time lists each archive exactly once.
    let mut seen: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    for _ in 0..ids.len() + 1 {
        let mut options = serde_json::json!({ "Limit": "1" });
        if let Some(m) = marker.as_deref() {
            options["Marker"] = Value::String(m.to_string());
        }
        let job_id = completed_job(
            &emulator,
            vault,
            &serde_json::json!({
                "Type": "inventory-retrieval",
                "InventoryRetrievalParameters": options,
            }),
        )
        .await;
        let document: Value = serde_json::from_slice(
            emulator.job_output(vault, &job_id, None).await.body().as_ref(),
        )
        .expect("a JSON inventory parses as JSON");
        for entry in document["ArchiveList"].as_array().cloned().unwrap_or_default() {
            seen.push(entry["ArchiveId"].as_str().unwrap_or_default().to_string());
        }
        let job = json(&emulator.describe_job(vault, &job_id).await);
        marker = job["InventoryRetrievalParameters"]["Marker"]
            .as_str()
            .map(str::to_string);
        if marker.is_none() {
            break;
        }
    }
    assert_eq!(
        seen, ids,
        "paging the inventory with Limit 1 must list every archive exactly once, in order"
    );

    // Options that are not options of this inventory are refused, not ignored.
    for (case, options) in [
        ("a StartDate that is not a date", serde_json::json!({ "StartDate": "last tuesday" })),
        ("an EndDate that is not a date", serde_json::json!({ "EndDate": "2013-03-20" })),
        ("a zero Limit", serde_json::json!({ "Limit": "0" })),
        ("a non-numeric Limit", serde_json::json!({ "Limit": "many" })),
        ("a negative Limit", serde_json::json!({ "Limit": "-1" })),
        ("a Marker naming no archive", serde_json::json!({ "Marker": "not-an-archive" })),
    ] {
        expect_error(
            case,
            &emulator
                .initiate_job(
                    vault,
                    &serde_json::json!({
                        "Type": "inventory-retrieval",
                        "InventoryRetrievalParameters": options,
                    }),
                )
                .await,
            400,
            "InvalidParameterValueException",
        );
    }
}

// ---------------------------------------------------------------------------
// 6. Ranges and job counts a caller could use to make the emulator grow
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_retrieval_range_that_is_not_a_range_of_the_archive_is_refused() {
    let emulator = Emulator::new();
    let vault = "bounds";
    assert_eq!(status(&emulator.create_vault(CALLER_ACCOUNT, vault).await), 201);
    let resp = emulator.upload(vault, &payload(2 * MIB, 23), &[]).await;
    assert_eq!(status(&resp), 201);
    let archive_id = header(&resp, "x-amz-archive-id").unwrap_or_default();

    // Rows span the ways a `StartByteValue-EndByteValue` string can fail to name a range
    // of this archive, including the two saturated bounds and an inverted pair whose
    // halves are each individually megabyte-aligned.
    let rows = [
        "1048576-0",
        "2097152-1048575",
        "0-18446744073709551615",
        "18446744073709551615-18446744073709551615",
        "18446744073709551615-0",
        "0-2097152",
        "-",
        "0-",
        "-1048575",
        "not-a-range",
        "1048576",
    ];
    for case in rows {
        expect_error(
            case,
            &emulator
                .initiate_job(
                    vault,
                    &serde_json::json!({
                        "Type": "archive-retrieval",
                        "ArchiveId": archive_id,
                        "RetrievalByteRange": case,
                    }),
                )
                .await,
            400,
            "InvalidParameterValueException",
        );
    }

    // The same for the HTTP `Range` on the download: a start past the end, an inverted
    // pair, and a saturated bound are refusals, never a wrapped length.
    let job_id = completed_job(
        &emulator,
        vault,
        &serde_json::json!({ "Type": "archive-retrieval", "ArchiveId": archive_id }),
    )
    .await;
    for case in [
        "bytes=2097152-2097200",
        "bytes=10-5",
        "bytes=0-",
        "bytes=18446744073709551615-18446744073709551615",
    ] {
        let resp = emulator.job_output(vault, &job_id, Some(case)).await;
        if case == "bytes=0-" {
            // An open-ended range from zero IS the whole output — and a request that
            // specified a range is still answered 206 with its Content-Range.
            assert_eq!(
                status(&resp),
                206,
                "case {case:?}: expected a partial response for an explicit range, got {}",
                status(&resp)
            );
            assert_eq!(
                header(&resp, "content-range").as_deref(),
                Some("bytes 0-2097151/2097152"),
                "case {case:?}: expected the whole span as the Content-Range"
            );
        } else {
            expect_error(case, &resp, 400, "InvalidParameterValueException");
        }
    }
}

#[tokio::test]
async fn the_number_of_retained_retrieval_jobs_is_bounded() {
    use service_com_amazonaws_glacier_glacier::state::MAX_JOBS_PER_VAULT;

    let emulator = Emulator::new();
    let vault = "bounded";
    assert_eq!(status(&emulator.create_vault(CALLER_ACCOUNT, vault).await), 201);

    // Every job this vault can hold is accepted...
    for index in 0..MAX_JOBS_PER_VAULT {
        let resp = emulator
            .initiate_job(vault, &serde_json::json!({ "Type": "inventory-retrieval" }))
            .await;
        assert_eq!(
            status(&resp),
            202,
            "case job {index}: expected HTTP 202 below the limit, got {} with body {:?}",
            status(&resp),
            String::from_utf8_lossy(resp.body())
        );
    }
    // ...and the next one is refused with the modelled retrieval-limit error rather than
    // growing the store without end.
    expect_error(
        "one job past the limit",
        &emulator
            .initiate_job(vault, &serde_json::json!({ "Type": "inventory-retrieval" }))
            .await,
        400,
        "PolicyEnforcedException",
    );

    // The refusal did not disturb what is stored: every accepted job is still listed.
    let listed = json(
        &emulator
            .send(
                "GET",
                &format!("/{CALLER_ACCOUNT}/vaults/{vault}/jobs?limit={MAX_JOBS_PER_VAULT}"),
                &[],
                &[],
            )
            .await,
    );
    assert_eq!(
        listed["JobList"].as_array().map(Vec::len),
        Some(MAX_JOBS_PER_VAULT),
        "expected every accepted job to be listed, got {:?}",
        listed["JobList"].as_array().map(Vec::len)
    );
}

// ---------------------------------------------------------------------------
// 7. Vault naming, the account label, and idempotence
// ---------------------------------------------------------------------------

#[tokio::test]
async fn vault_names_are_checked_against_the_documented_character_rule() {
    let emulator = Emulator::new();

    // The rule's source: CreateVault's own documentation — "Names can be between 1 and
    // 255 characters long. Allowed characters are a-z, A-Z, 0-9, '_' (underscore), '-'
    // (hyphen), and '.' (period)." Rows cover each allowed class, both published length
    // bounds, and one character past the upper bound.
    let max_length = "v".repeat(255);
    let past_max = "v".repeat(256);
    let rows: Vec<(&str, &str, u16)> = vec![
        ("a single character", "v", 201),
        ("lower case", "examplevault", 201),
        ("upper case", "EXAMPLEVAULT", 201),
        ("digits", "2026", 201),
        ("underscore, hyphen and period", "my_vault-2.0", 201),
        ("exactly the 255-character bound", max_length.as_str(), 201),
        ("one character past the bound", past_max.as_str(), 400),
        ("a disallowed asterisk", "star*vault", 400),
        ("a disallowed plus", "plus+vault", 400),
        ("a disallowed colon", "colon:vault", 400),
        ("a disallowed non-ASCII letter", "vault-\u{e9}t\u{e9}", 400),
    ];

    for (case, vault, expected) in rows {
        let resp = emulator.create_vault(CALLER_ACCOUNT, vault).await;
        assert_eq!(
            status(&resp),
            expected,
            "case {case:?}: expected HTTP {expected} for vault name {vault:?}, got {} with body {:?}",
            status(&resp),
            String::from_utf8_lossy(resp.body())
        );
        if expected == 400 {
            assert_eq!(
                error_code(&resp),
                "InvalidParameterValueException",
                "case {case:?}: a bad vault name is an invalid parameter value"
            );
        }
    }
}

#[tokio::test]
async fn create_vault_is_idempotent_and_the_account_label_resolves_to_the_caller() {
    let emulator = Emulator::new();
    let vault = "idempotent";

    let first = emulator.create_vault(CALLER_ACCOUNT, vault).await;
    assert_eq!(status(&first), 201);
    let location = header(&first, "location").unwrap_or_default();
    let created = json(&emulator.describe_vault(vault).await);

    let body = payload(32, 11);
    assert_eq!(status(&emulator.upload(vault, &body, &[]).await), 201);

    // "This operation is idempotent": the same answer, and the archives survive.
    let second = emulator.create_vault(CALLER_ACCOUNT, vault).await;
    assert_eq!(
        status(&second),
        201,
        "re-creating an existing vault answers 201 again"
    );
    assert_eq!(
        header(&second, "location").unwrap_or_default(),
        location,
        "re-creating an existing vault answers the same Location"
    );
    let after = json(&emulator.describe_vault(vault).await);
    assert_eq!(
        after["NumberOfArchives"].as_i64(),
        Some(1),
        "re-creating a vault must not wipe its archives, got {after}"
    );
    assert_eq!(
        after["CreationDate"], created["CreationDate"],
        "re-creating a vault must not restamp its CreationDate"
    );

    // The `-` label and the caller's own account id address the SAME vault: this
    // emulator presents one account, and the path can only ever name it.
    for label in [CALLER_ACCOUNT.to_string(), account_id().to_string()] {
        let resp = emulator
            .send("GET", &format!("/{label}/vaults/{vault}"), &[], &[])
            .await;
        assert_eq!(
            status(&resp),
            200,
            "case {label:?}: the account label must resolve to the caller's account"
        );
        assert_eq!(
            json(&resp),
            after,
            "case {label:?}: both account labels name one vault"
        );
    }
}

// ---------------------------------------------------------------------------
// 8. Pagination
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listing_pages_through_every_record_exactly_once() {
    let emulator = Emulator::new();
    let names = ["alpha", "beta", "delta", "epsilon", "gamma"];
    for name in names {
        assert_eq!(status(&emulator.create_vault(CALLER_ACCOUNT, name).await), 201);
    }

    // Every page size from "one at a time" to "more than there are".
    for limit in [1usize, 2, 3, 5, 10] {
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut path = format!("/{CALLER_ACCOUNT}/vaults?limit={limit}");
            if let Some(m) = marker.as_deref() {
                path.push_str(&format!("&marker={m}"));
            }
            let resp = emulator.send("GET", &path, &[], &[]).await;
            assert_eq!(
                status(&resp),
                200,
                "case limit={limit}: ListVaults must answer 200, got {}",
                status(&resp)
            );
            let page = json(&resp);
            let entries = page["VaultList"].as_array().cloned().unwrap_or_default();
            assert!(
                entries.len() <= limit,
                "case limit={limit}: a page never exceeds the limit, got {}",
                entries.len()
            );
            for entry in &entries {
                seen.push(
                    entry["VaultName"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            pages += 1;
            assert!(
                pages <= names.len() + 1,
                "case limit={limit}: pagination did not terminate"
            );
            marker = page["Marker"].as_str().map(str::to_string);
            if marker.is_none() {
                break;
            }
        }
        assert_eq!(
            seen,
            names.iter().map(|n| (*n).to_string()).collect::<Vec<_>>(),
            "case limit={limit}: every vault is listed exactly once, in name order"
        );
    }

    // A marker that names nothing is refused rather than silently restarting the list.
    expect_error(
        "an unknown ListVaults marker",
        &emulator
            .send(
                "GET",
                &format!("/{CALLER_ACCOUNT}/vaults?marker=arn:aws:glacier:{REGION}:{}:vaults/nope", account_id()),
                &[],
                &[],
            )
            .await,
        400,
        "InvalidParameterValueException",
    );
    expect_error(
        "a zero ListVaults limit",
        &emulator
            .send("GET", &format!("/{CALLER_ACCOUNT}/vaults?limit=0"), &[], &[])
            .await,
        400,
        "InvalidParameterValueException",
    );
}

#[tokio::test]
async fn list_jobs_pages_and_filters_the_stored_jobs() {
    let emulator = Emulator::new();
    let vault = "jobs";
    assert_eq!(status(&emulator.create_vault(CALLER_ACCOUNT, vault).await), 201);
    let resp = emulator.upload(vault, &payload(64, 13), &[]).await;
    assert_eq!(status(&resp), 201);
    let archive_id = header(&resp, "x-amz-archive-id").unwrap_or_default();

    // Four jobs that are allowed to settle, then one that is not: the filters must split
    // them by what is stored, not by what was asked for.
    let mut settled: Vec<String> = Vec::new();
    for _ in 0..4 {
        let resp = emulator
            .initiate_job(
                vault,
                &serde_json::json!({ "Type": "archive-retrieval", "ArchiveId": archive_id }),
            )
            .await;
        assert_eq!(status(&resp), 202);
        settled.push(header(&resp, "x-amz-job-id").unwrap_or_default());
    }
    tokio::time::sleep(SETTLE_WAIT).await;
    let resp = emulator
        .initiate_job(
            vault,
            &serde_json::json!({ "Type": "inventory-retrieval" }),
        )
        .await;
    assert_eq!(status(&resp), 202);
    let in_progress = header(&resp, "x-amz-job-id").unwrap_or_default();

    let all: Vec<String> = settled
        .iter()
        .cloned()
        .chain(std::iter::once(in_progress.clone()))
        .collect();

    for limit in [1usize, 2, 5, 50] {
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut path = format!("/{CALLER_ACCOUNT}/vaults/{vault}/jobs?limit={limit}");
            if let Some(m) = marker.as_deref() {
                path.push_str(&format!("&marker={m}"));
            }
            let resp = emulator.send("GET", &path, &[], &[]).await;
            assert_eq!(
                status(&resp),
                200,
                "case limit={limit}: ListJobs must answer 200, got {}",
                status(&resp)
            );
            let page = json(&resp);
            let entries = page["JobList"].as_array().cloned().unwrap_or_default();
            assert!(
                entries.len() <= limit,
                "case limit={limit}: a page never exceeds the limit, got {}",
                entries.len()
            );
            for entry in &entries {
                seen.push(entry["JobId"].as_str().unwrap_or_default().to_string());
            }
            pages += 1;
            assert!(
                pages <= all.len() + 1,
                "case limit={limit}: pagination did not terminate"
            );
            marker = page["Marker"].as_str().map(str::to_string);
            if marker.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, all,
            "case limit={limit}: every job is listed exactly once, oldest first"
        );
    }

    // statuscode / completed select from the same stored records.
    let filters: Vec<(&str, String, BTreeSet<String>)> = vec![
        (
            "statuscode=Succeeded",
            "statuscode=Succeeded".to_string(),
            settled.iter().cloned().collect(),
        ),
        (
            "statuscode=InProgress",
            "statuscode=InProgress".to_string(),
            std::iter::once(in_progress.clone()).collect(),
        ),
        (
            "statuscode=Failed",
            "statuscode=Failed".to_string(),
            BTreeSet::new(),
        ),
        (
            "completed=true",
            "completed=true".to_string(),
            settled.iter().cloned().collect(),
        ),
        (
            "completed=false",
            "completed=false".to_string(),
            std::iter::once(in_progress.clone()).collect(),
        ),
    ];
    for (case, query, expected) in filters {
        let resp = emulator
            .send(
                "GET",
                &format!("/{CALLER_ACCOUNT}/vaults/{vault}/jobs?{query}"),
                &[],
                &[],
            )
            .await;
        assert_eq!(
            status(&resp),
            200,
            "case {case:?}: ListJobs must answer 200, got {}",
            status(&resp)
        );
        let page = json(&resp);
        let got: BTreeSet<String> = page["JobList"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|entry| entry["JobId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            got, expected,
            "case {case:?}: expected exactly those jobs, got {page}"
        );
    }

    for (case, query) in [
        ("an unknown statuscode", "statuscode=Pending"),
        ("a non-boolean completed", "completed=maybe"),
        ("an unknown marker", "marker=not-a-job"),
    ] {
        expect_error(
            case,
            &emulator
                .send(
                    "GET",
                    &format!("/{CALLER_ACCOUNT}/vaults/{vault}/jobs?{query}"),
                    &[],
                    &[],
                )
                .await,
            400,
            "InvalidParameterValueException",
        );
    }
}

// ---------------------------------------------------------------------------
// 9. What is not stored is refused
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_resource_that_is_not_stored_is_refused_with_the_modelled_error() {
    let emulator = Emulator::new();
    let vault = "present";
    assert_eq!(status(&emulator.create_vault(CALLER_ACCOUNT, vault).await), 201);
    let resp = emulator.upload(vault, &payload(16, 17), &[]).await;
    assert_eq!(status(&resp), 201);
    let archive_id = header(&resp, "x-amz-archive-id").unwrap_or_default();

    // Every operation of this slice that addresses a vault, archive or job by name.
    let rows: Vec<(&str, &str, String, Vec<u8>)> = vec![
        (
            "DescribeVault on an unknown vault",
            "GET",
            format!("/{CALLER_ACCOUNT}/vaults/absent"),
            Vec::new(),
        ),
        (
            "UploadArchive into an unknown vault",
            "POST",
            format!("/{CALLER_ACCOUNT}/vaults/absent/archives"),
            b"bytes".to_vec(),
        ),
        (
            "InitiateJob against an unknown vault",
            "POST",
            format!("/{CALLER_ACCOUNT}/vaults/absent/jobs"),
            br#"{"Type":"inventory-retrieval"}"#.to_vec(),
        ),
        (
            "DescribeJob in an unknown vault",
            "GET",
            format!("/{CALLER_ACCOUNT}/vaults/absent/jobs/anything"),
            Vec::new(),
        ),
        (
            "ListJobs of an unknown vault",
            "GET",
            format!("/{CALLER_ACCOUNT}/vaults/absent/jobs"),
            Vec::new(),
        ),
        (
            "GetJobOutput in an unknown vault",
            "GET",
            format!("/{CALLER_ACCOUNT}/vaults/absent/jobs/anything/output"),
            Vec::new(),
        ),
        (
            "DeleteArchive in an unknown vault",
            "DELETE",
            format!("/{CALLER_ACCOUNT}/vaults/absent/archives/{archive_id}"),
            Vec::new(),
        ),
        (
            "DeleteVault of an unknown vault",
            "DELETE",
            format!("/{CALLER_ACCOUNT}/vaults/absent"),
            Vec::new(),
        ),
        (
            "DescribeJob for a job the vault does not hold",
            "GET",
            format!("/{CALLER_ACCOUNT}/vaults/{vault}/jobs/absent-job"),
            Vec::new(),
        ),
        (
            "GetJobOutput for a job the vault does not hold",
            "GET",
            format!("/{CALLER_ACCOUNT}/vaults/{vault}/jobs/absent-job/output"),
            Vec::new(),
        ),
    ];

    for (case, method, path, body) in rows {
        let resp = emulator.send(method, &path, &[], &body).await;
        expect_error(case, &resp, 404, "ResourceNotFoundException");
    }

    // A vault that still holds archives is not deleted: "Amazon Glacier will delete a
    // vault only if there are no archives in the vault."
    expect_error(
        "DeleteVault on a vault that still holds an archive",
        &emulator
            .send("DELETE", &format!("/{CALLER_ACCOUNT}/vaults/{vault}"), &[], &[])
            .await,
        400,
        "InvalidParameterValueException",
    );
    assert_eq!(
        status(&emulator.describe_vault(vault).await),
        200,
        "a refused DeleteVault leaves the vault in place"
    );

    // DeleteArchive is the exception the model documents: "This operation is idempotent.
    // Attempting to delete an already-deleted archive does not result in an error." Both
    // an archive that was never there and one deleted a moment ago answer 204, and the
    // vault's count drops exactly once.
    for case in [
        "an archive the vault never held",
        "the stored archive",
        "the same archive again",
    ] {
        let target = if case == "an archive the vault never held" {
            "absent-archive".to_string()
        } else {
            archive_id.clone()
        };
        let resp = emulator
            .send(
                "DELETE",
                &format!("/{CALLER_ACCOUNT}/vaults/{vault}/archives/{target}"),
                &[],
                &[],
            )
            .await;
        assert_eq!(
            status(&resp),
            204,
            "case {case:?}: a delete of an already-deleted archive is not an error, got {}",
            status(&resp)
        );
    }
    let described = json(&emulator.describe_vault(vault).await);
    assert_eq!(
        described["NumberOfArchives"].as_i64(),
        Some(0),
        "the repeated deletes removed the one archive exactly once, got {described}"
    );

    // An accountId that is not the caller's own names an account these credentials do not
    // hold: "This value must match the AWS account ID associated with the credentials
    // used to sign the request."
    for case in ["123456789012", "000000000001", "0", "not-an-account"] {
        expect_error(
            case,
            &emulator
                .send("GET", &format!("/{case}/vaults/{vault}"), &[], &[])
                .await,
            400,
            "InvalidParameterValueException",
        );
    }
}
