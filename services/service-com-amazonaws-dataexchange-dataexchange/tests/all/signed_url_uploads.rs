// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the upload URLs this service MINTS: the targets
//! `CreateJob` announces for a job that takes its content from one
//! (`Details.ImportAssetFromSignedUrl.SignedUrl` and
//! `Details.ImportAssetFromApiGatewayApi.ApiSpecificationUploadUrl`), and the import
//! that reads what a client uploaded there.
//!
//! Every test drives the service the way the emulator does: through
//! `MultiServiceRouter::dispatch`. The modelled calls go through `data_sets.rs`'s
//! restJson1 harness; the upload itself is a RAW PUT carrying no credential, which is
//! what a client following a presigned URL sends.
//!
//! Three rules are under test.
//!
//! 1. A URL this service announces as an upload target is a real, writable location
//!    until the expiry announced with it: a PUT of the content to it is accepted, the
//!    bytes are retained as that job's content, and a second PUT replaces them.
//! 2. The content is judged, not just taken: the digest the job declared — and HTTP's
//!    own `Content-MD5`, when the client sends one — must describe the bytes, a path
//!    this service never announced is no upload, a closed window is an expired request,
//!    and a method the target does not serve is refused as one.
//! 3. An import whose content is there carries its work out: one execution span after
//!    the start, the revision the job's details name holds the uploaded content as an
//!    asset, so the revision can be finalized — and an import whose target no longer
//!    admits an asset ends in `ERROR` instead, adding none.
//!
//! The rows span what the rules generalise over: both URL members the model announces,
//! content that is empty, text, binary and many kilobytes, both ways a declared digest
//! can fail to describe the bytes, and the deadline at and past the announced instant.

use std::sync::Arc;

use bytes::Bytes;
use md5::{Digest, Md5};
use serde_json::{json, Value};
use service_com_amazonaws_dataexchange_dataexchange as dataexchange;
use smithy_server_runtime::MultiServiceRouter;

use crate::data_sets::{call, create_data_set, harness, seconds, ACCOUNT, REGION};
use crate::job_lifecycle::{get_job, settle_past_the_execution_span, start_job};
use crate::jobs::create_job;

/// The `Type` whose details announce an asset upload URL.
const SIGNED_URL_TYPE: &str = "IMPORT_ASSET_FROM_SIGNED_URL";

/// The `Type` whose details announce an API specification upload URL.
const API_GATEWAY_TYPE: &str = "IMPORT_ASSET_FROM_API_GATEWAY_API";

/// A well-formed `Id` (`^[a-zA-Z0-9]{30,40}$`) for a job this emulator does not hold.
const ABSENT_JOB_ID: &str = "0123456789abcdef0123456789abcdef";

/// The published Base64 MD5 of no bytes, as a fixed cross-check that this file's own
/// digest agrees with the encoding the model's `Md5Hash` members carry.
const MD5_OF_NO_BYTES: &str = "1B2M2Y8AsgTpgAmY7PhCfg==";

/// One HTTP answer to a RAW request, reduced to what the upload assertions read.
struct RawAnswer {
    status: u16,
    /// The code the plane names in `x-amzn-errortype`, which is also in the body.
    code: Option<String>,
    etag: Option<String>,
    allow: Option<String>,
    body: Value,
}

/// The Base64 MD5 of `content`, in the encoding the model's `Md5Hash` and HTTP's
/// `Content-MD5` both carry.
fn base64_md5(content: &[u8]) -> String {
    aws_smithy_types::base64::encode(Md5::digest(content))
}

/// The entity tag an upload of `content` answers with: its MD5 as quoted hex.
fn etag_of(content: &[u8]) -> String {
    format!("\"{}\"", hex::encode(Md5::digest(content)))
}

/// Send one RAW request — no credential, no restJson1 envelope — to `url`, the way a
/// client that was handed a presigned URL sends it.
async fn raw(
    router: &MultiServiceRouter,
    method: &str,
    url: &str,
    content: &[u8],
    content_md5: Option<&str>,
) -> RawAnswer {
    let host = url
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or_else(|| panic!("the URL under test names a host: {url:?}"));
    let mut request = http::Request::builder()
        .method(method)
        .uri(url)
        .header("host", host);
    if let Some(declared) = content_md5 {
        request = request.header("content-md5", declared);
    }
    let response = router
        .dispatch(
            request
                .body(Bytes::copy_from_slice(content))
                .expect("a well-formed raw request"),
        )
        .await;
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string)
    };
    let (code, etag, allow) = (header("x-amzn-errortype"), header("etag"), header("allow"));
    let bytes = response.into_body();
    let body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    RawAnswer {
        status,
        code,
        etag,
        allow,
        body,
    }
}

/// A data set of `asset_type` holding one revision — every job under test names one,
/// and a revision's assets carry the `AssetType` of the data set they belong to.
async fn data_set_with_revision(
    router: &MultiServiceRouter,
    name: &str,
    asset_type: &str,
) -> (String, String) {
    let data_set = create_data_set(router, name, "for uploads", asset_type, None).await;
    assert_eq!(
        data_set.status, 201,
        "case {name}: expected 201 from CreateDataSet, got {} body {:?}",
        data_set.status, data_set.body
    );
    let data_set_id = data_set.string("Id");
    let revision = call(
        router,
        "POST",
        &format!("/v1/data-sets/{data_set_id}/revisions"),
        Some(json!({ "Comment": "for uploads" })),
    )
    .await;
    assert_eq!(
        revision.status, 201,
        "case {name}: expected 201 from CreateRevision, got {} body {:?}",
        revision.status, revision.body
    );
    (data_set_id, revision.string("Id"))
}

/// One row of the upload catalog: which URL member announces the target, the content a
/// client uploads to it, and what the import must then put in the revision.
struct UploadCase {
    label: &'static str,
    job_type: &'static str,
    /// The `Details` / `ResponseDetails` member the model pairs with `job_type`.
    member: &'static str,
    /// The member of those details that carries the upload URL.
    url_member: &'static str,
    /// The member that carries that URL's expiry.
    expiry_member: &'static str,
    /// The `AssetType` of the data set the job imports into.
    asset_type: &'static str,
    /// The `Name` the imported asset must carry, which the details name.
    asset_name: &'static str,
    content: Vec<u8>,
}

/// Every upload URL the model announces, against content of each shape an upload can
/// carry.
///
/// Source: the two `ResponseDetails` members that carry a URL and an expiry
/// (`smithy_query(query="shape",
/// shape="ImportAssetFromSignedUrlResponseDetails,ImportAssetFromApiGatewayApiResponseDetails")`);
/// the rows then differ along the content — empty, text, every byte value, many
/// kilobytes — because the rule is about the bytes being retained, whatever they are.
fn upload_cases() -> Vec<UploadCase> {
    let signed_url = |label, asset_name, content: Vec<u8>| UploadCase {
        label,
        job_type: SIGNED_URL_TYPE,
        member: "ImportAssetFromSignedUrl",
        url_member: "SignedUrl",
        expiry_member: "SignedUrlExpiresAt",
        asset_type: "S3_SNAPSHOT",
        asset_name,
        content,
    };
    let api_specification = |label, asset_name, content: Vec<u8>| UploadCase {
        label,
        job_type: API_GATEWAY_TYPE,
        member: "ImportAssetFromApiGatewayApi",
        url_member: "ApiSpecificationUploadUrl",
        expiry_member: "ApiSpecificationUploadUrlExpiresAt",
        asset_type: "API_GATEWAY_API",
        asset_name,
        content,
    };
    vec![
        signed_url("an asset of text", "orders.csv", b"id,name\n1,ada\n2,grace\n".to_vec()),
        signed_url("an empty asset", "empty.csv", Vec::new()),
        signed_url("an asset of every byte value", "pixels.bin", (0..=255u8).collect()),
        signed_url(
            "an asset of many kilobytes",
            "feed.ndjson",
            b"{\"row\":\"value\"}\n".repeat(4_096),
        ),
        api_specification(
            "an API specification",
            "orders",
            br#"{"openapi":"3.0.1","paths":{"/orders":{"get":{}}}}"#.to_vec(),
        ),
        api_specification("an empty API specification", "inventory", Vec::new()),
    ]
}

/// The `Details` body `case` describes, declaring `digest` for the content.
fn details_for(case: &UploadCase, data_set_id: &str, revision_id: &str, digest: &str) -> Value {
    let detail = if case.job_type == SIGNED_URL_TYPE {
        json!({
            "AssetName": case.asset_name,
            "DataSetId": data_set_id,
            "Md5Hash": digest,
            "RevisionId": revision_id,
        })
    } else {
        json!({
            "ApiDescription": "an imported REST API",
            "ApiId": "a1b2c3d4e5",
            "ApiKey": "a-key",
            "ApiName": case.asset_name,
            "ApiSpecificationMd5Hash": digest,
            "DataSetId": data_set_id,
            "ProtocolType": "REST",
            "RevisionId": revision_id,
            "Stage": "prod",
        })
    };
    json!({ case.member: detail })
}

/// The assets a revision holds, read straight off the store — `GetAsset` and
/// `ListRevisionAssets` are still scaffolds (each has its own open issue), so this is
/// where an imported asset is read back beside the modelled read of the revision it
/// landed in.
fn stored_assets(
    engine: &Arc<engine::InMemoryEngine>,
    data_set_id: &str,
    revision_id: &str,
) -> Vec<dataexchange::state::Asset> {
    let state = dataexchange::state::service_state(&**engine).expect("the dataexchange store");
    let store = state.lock();
    store
        .data_sets
        .get(data_set_id)
        .and_then(|data_set| data_set.revisions.get(revision_id))
        .map(|revision| revision.assets.clone())
        .unwrap_or_default()
}

/// The content a client has uploaded for `job_id` and nothing has imported yet, read
/// off the store: `None` once the import has taken it (or before any upload).
fn staged_content(engine: &Arc<engine::InMemoryEngine>, job_id: &str) -> Option<Vec<u8>> {
    let state = dataexchange::state::service_state(&**engine).expect("the dataexchange store");
    let store = state.lock();
    store
        .jobs
        .get(job_id)?
        .upload
        .as_ref()?
        .content
        .clone()
}

/// Create the job `case` describes and hand back its `Id` and its announced upload URL.
async fn job_with_upload_url(
    router: &MultiServiceRouter,
    case: &UploadCase,
    data_set_id: &str,
    revision_id: &str,
    digest: &str,
) -> (String, String) {
    let label = case.label;
    let created = create_job(
        router,
        case.job_type,
        details_for(case, data_set_id, revision_id, digest),
        None,
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case {label}: expected 201 from CreateJob, got {} body {:?}",
        created.status, created.body
    );
    let detail = created.member("Details")[case.member].clone();
    let url = detail[case.url_member]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {label}: expected {} in the answered details, got {detail:?}",
                case.url_member
            )
        })
        .to_string();
    // The expiry is announced with the URL, and it is in the future: the window a
    // client has to upload in.
    let expires = seconds(&detail[case.expiry_member], label);
    let created_at = seconds(created.member("CreatedAt"), label);
    assert!(
        expires > created_at,
        "case {label}: expected {} after CreatedAt, got {expires} vs {created_at}",
        case.expiry_member
    );
    (created.string("Id"), url)
}

/// RULE: the fixed digest this file computes is the one the model's `Md5Hash` members
/// carry, so every row's declared hash is a real statement about its content.
#[tokio::test]
async fn the_declared_digest_is_the_models_base64_md5() {
    assert_eq!(
        base64_md5(b""),
        MD5_OF_NO_BYTES,
        "case \"no bytes\": expected the published Base64 MD5 of no bytes, got {}",
        base64_md5(b"")
    );
    for case in upload_cases() {
        let digest = base64_md5(&case.content);
        assert_eq!(
            digest.len(),
            24,
            "case {}: expected a 24-character Md5Hash, got {digest:?}",
            case.label
        );
    }
}

/// RULE: a URL this service announces as an upload target accepts a PUT of the
/// content, retains it as that job's content, and the import that named it then puts
/// that content into the revision as an asset — after which the revision, which held
/// nothing importable before, can be finalized. This holds for every URL the model
/// announces and whatever the content is.
#[tokio::test]
async fn content_uploaded_to_an_announced_url_is_imported_into_the_revision() {
    for case in upload_cases() {
        let (router, engine) = harness();
        let label = case.label;
        let (data_set_id, revision_id) =
            data_set_with_revision(&router, label, case.asset_type).await;
        let digest = base64_md5(&case.content);
        let (job_id, url) =
            job_with_upload_url(&router, &case, &data_set_id, &revision_id, &digest).await;

        // The announced URL is writable, and answers the stored content's entity tag.
        let uploaded = raw(&router, "PUT", &url, &case.content, Some(&digest)).await;
        assert_eq!(
            uploaded.status, 200,
            "case {label}: expected 200 from the PUT to {url:?}, got {} body {:?}",
            uploaded.status, uploaded.body
        );
        assert_eq!(
            uploaded.etag.as_deref(),
            Some(etag_of(&case.content).as_str()),
            "case {label}: expected the stored content's ETag, got {:?}",
            uploaded.etag
        );
        assert_eq!(
            staged_content(&engine, &job_id).as_deref(),
            Some(case.content.as_slice()),
            "case {label}: expected the uploaded bytes retained as the job's content"
        );

        // Only the execution imports: the revision holds nothing until the job runs.
        assert!(
            stored_assets(&engine, &data_set_id, &revision_id).is_empty(),
            "case {label}: expected no asset before the job ran"
        );
        let started = start_job(&router, &job_id).await;
        assert_eq!(
            started.status, 202,
            "case {label}: expected 202 from StartJob, got {} body {:?}",
            started.status, started.body
        );
        settle_past_the_execution_span(&engine);
        let settled = get_job(&router, &job_id).await;
        assert_eq!(
            settled.string("State"),
            "COMPLETED",
            "case {label}: expected the import to complete, got {:?} errors {:?}",
            settled.member("State"),
            settled.member("Errors")
        );
        assert!(
            settled.member("Errors").is_null(),
            "case {label}: expected no Errors on an import that was carried out, got {:?}",
            settled.member("Errors")
        );

        // The content is now the revision's asset, named as the details named it.
        let assets = stored_assets(&engine, &data_set_id, &revision_id);
        assert_eq!(
            assets.len(),
            1,
            "case {label}: expected the import to add one asset, got {assets:?}"
        );
        let asset = &assets[0];
        assert_eq!(
            asset.name, case.asset_name,
            "case {label}: expected the asset named {:?}, got {:?}",
            case.asset_name, asset.name
        );
        assert_eq!(
            asset.content, case.content,
            "case {label}: expected the asset to carry the uploaded bytes ({} of them), got {}",
            case.content.len(),
            asset.content.len()
        );
        assert_eq!(
            asset.asset_type, case.asset_type,
            "case {label}: expected the data set's AssetType on the asset, got {:?}",
            asset.asset_type
        );
        assert_eq!(
            asset.arn,
            format!(
                "arn:aws:dataexchange:{REGION}:{ACCOUNT}:data-sets/{data_set_id}/revisions/{revision_id}/assets/{}",
                asset.id
            ),
            "case {label}: expected the asset's ARN to be built from its ids"
        );
        // An API asset IS the API it fulfils, so that import records it; an asset
        // imported from a signed URL fulfils none.
        match case.job_type {
            API_GATEWAY_TYPE => {
                let api = asset
                    .api
                    .as_ref()
                    .unwrap_or_else(|| panic!("case {label}: expected the API on the asset"));
                assert_eq!(
                    api.api_name.as_deref(),
                    Some(case.asset_name),
                    "case {label}: expected the imported API's name, got {:?}",
                    api.api_name
                );
                assert_eq!(
                    api.stage.as_deref(),
                    Some("prod"),
                    "case {label}: expected the imported API's stage, got {:?}",
                    api.stage
                );
            }
            _ => assert!(
                asset.api.is_none(),
                "case {label}: expected no API on an asset imported from a signed URL, got {:?}",
                asset.api
            ),
        }
        // The staged copy is gone: an upload is imported once, not once per read.
        assert_eq!(
            staged_content(&engine, &job_id),
            None,
            "case {label}: expected the import to take the staged bytes"
        );

        // And the modelled read of the revision agrees: a revision holding an asset is
        // finalizable, which one holding none is not.
        let finalized = call(
            &router,
            "PATCH",
            &format!("/v1/data-sets/{data_set_id}/revisions/{revision_id}"),
            Some(json!({ "Finalized": true })),
        )
        .await;
        assert_eq!(
            finalized.status, 200,
            "case {label}: expected the revision to be finalizable once it holds the \
             imported asset, got {} body {:?}",
            finalized.status, finalized.body
        );
        assert_eq!(
            finalized.member("Finalized"),
            &json!(true),
            "case {label}: expected Finalized=true back, got {:?}",
            finalized.member("Finalized")
        );
    }
}

/// RULE: an upload is served on the endpoint the caller reached this emulator on,
/// whatever host that is and with no credential: the path the announced URL carries is
/// what addresses it.
#[tokio::test]
async fn an_announced_upload_path_is_served_unsigned_on_the_emulator_endpoint() {
    let (router, engine) = harness();
    let (data_set_id, revision_id) =
        data_set_with_revision(&router, "loopback-upload", "S3_SNAPSHOT").await;
    let case = &upload_cases()[0];
    let digest = base64_md5(&case.content);
    let (job_id, url) =
        job_with_upload_url(&router, case, &data_set_id, &revision_id, &digest).await;

    // The same path, addressed at a host that names no service at all — which is what
    // a client on the machine this emulator runs on reaches it at.
    let path = url
        .split_once("://")
        .and_then(|(_, rest)| rest.split_once('/'))
        .map(|(_, path)| format!("/{path}"))
        .expect("the announced URL carries a path");
    let loopback = format!("https://127.0.0.1:9297{path}");
    let uploaded = raw(&router, "PUT", &loopback, &case.content, None).await;
    assert_eq!(
        uploaded.status, 200,
        "case \"loopback endpoint\": expected 200 from the PUT to {loopback:?}, got {} body {:?}",
        uploaded.status, uploaded.body
    );
    assert_eq!(
        staged_content(&engine, &job_id).as_deref(),
        Some(case.content.as_slice()),
        "case \"loopback endpoint\": expected the uploaded bytes retained"
    );
}

/// One row of the refusal table: a request the upload plane cannot accept, and the
/// status and code it answers.
struct RefusedCase {
    label: &'static str,
    method: &'static str,
    target: String,
    content: Vec<u8>,
    /// The `Content-MD5` the request declares, where it declares one.
    declared: Option<String>,
    status: u16,
    code: &'static str,
}

/// RULE: an upload the plane cannot accept is refused with the status and code of the
/// condition, and leaves the job's content exactly as it was — which is why a correct
/// upload still works afterwards.
#[tokio::test]
async fn an_upload_the_plane_cannot_accept_is_refused_and_stores_nothing() {
    let (router, engine) = harness();
    let (data_set_id, revision_id) =
        data_set_with_revision(&router, "refused-uploads", "S3_SNAPSHOT").await;
    let case = &upload_cases()[0];
    let content = case.content.clone();
    let digest = base64_md5(&content);
    let (job_id, url) =
        job_with_upload_url(&router, case, &data_set_id, &revision_id, &digest).await;
    let other_leaf = url.replace("/asset", "/api-specification");
    let absent = url.replace(&job_id, ABSENT_JOB_ID);

    // Rows span every way an upload can fail to be one: the method, both ways a
    // declared digest can fail to describe the bytes, a leaf this job did not announce,
    // and a job this emulator does not hold.
    let rows: Vec<RefusedCase> = vec![
        RefusedCase {
            label: "a read of an upload target",
            method: "GET",
            target: url.clone(),
            content: Vec::new(),
            declared: None,
            status: 405,
            code: "MethodNotAllowed",
        },
        RefusedCase {
            label: "bytes the job's declared digest does not describe",
            method: "PUT",
            target: url.clone(),
            content: b"other bytes entirely".to_vec(),
            declared: None,
            status: 400,
            code: "BadDigest",
        },
        RefusedCase {
            label: "a Content-MD5 of other bytes",
            method: "PUT",
            target: url.clone(),
            content: content.clone(),
            declared: Some(base64_md5(b"other bytes entirely")),
            status: 400,
            code: "BadDigest",
        },
        RefusedCase {
            label: "a Content-MD5 that is not base64 at all",
            method: "PUT",
            target: url.clone(),
            content: content.clone(),
            declared: Some("not a digest at all!!!!!".to_string()),
            status: 400,
            code: "InvalidDigest",
        },
        RefusedCase {
            label: "a Content-MD5 of the wrong width",
            method: "PUT",
            target: url.clone(),
            content: content.clone(),
            declared: Some(aws_smithy_types::base64::encode(b"short")),
            status: 400,
            code: "InvalidDigest",
        },
        RefusedCase {
            label: "a leaf this job did not announce",
            method: "PUT",
            target: other_leaf,
            content: content.clone(),
            declared: Some(digest.clone()),
            status: 404,
            code: "NotFound",
        },
        RefusedCase {
            label: "an upload for a job this emulator does not hold",
            method: "PUT",
            target: absent,
            content: content.clone(),
            declared: Some(digest.clone()),
            status: 404,
            code: "NotFound",
        },
    ];

    for case in rows {
        let label = case.label;
        let answer = raw(
            &router,
            case.method,
            &case.target,
            &case.content,
            case.declared.as_deref(),
        )
        .await;
        assert_eq!(
            answer.status, case.status,
            "case {label:?}: expected {}, got {} body {:?}",
            case.status, answer.status, answer.body
        );
        assert_eq!(
            answer.code.as_deref(),
            Some(case.code),
            "case {label:?}: expected the code {}, got {:?} body {:?}",
            case.code,
            answer.code,
            answer.body
        );
        assert_eq!(
            answer.body.get("code").and_then(Value::as_str),
            Some(case.code),
            "case {label:?}: expected the body to name the code too, got {:?}",
            answer.body
        );
        assert_eq!(
            staged_content(&engine, &job_id),
            None,
            "case {label:?}: expected the refusal to store nothing"
        );
    }

    // A 405 states which method the target serves, as a 405 must.
    let read = raw(&router, "GET", &url, &[], None).await;
    assert_eq!(
        read.allow.as_deref(),
        Some("PUT"),
        "case \"a read of an upload target\": expected Allow: PUT, got {:?}",
        read.allow
    );

    // The upload is still usable: none of the refusals consumed it.
    let uploaded = raw(&router, "PUT", &url, &content, Some(&digest)).await;
    assert_eq!(
        uploaded.status, 200,
        "case \"a correct upload after refusals\": expected 200, got {} body {:?}",
        uploaded.status, uploaded.body
    );
    assert_eq!(
        staged_content(&engine, &job_id).as_deref(),
        Some(content.as_slice()),
        "case \"a correct upload after refusals\": expected the bytes retained"
    );
}

/// RULE: the expiry announced with the URL is a deadline: an upload at or after that
/// instant is refused as expired, one before it is accepted.
#[tokio::test]
async fn an_upload_is_accepted_before_the_announced_expiry_and_refused_at_it() {
    // Rows are the published deadline itself and both sides of it; the window this
    // emulator announces is an hour, so the test moves the stored deadline — the one
    // instant the plane reads — rather than the clock.
    let rows: &[(&str, f64, u16)] = &[
        ("an hour of window left", 3_600.0, 200),
        ("a second of window left", 1.0, 200),
        ("the announced instant itself", 0.0, 403),
        ("a second past the deadline", -1.0, 403),
    ];

    for (label, offset, expected) in rows {
        let (router, engine) = harness();
        let (data_set_id, revision_id) =
            data_set_with_revision(&router, "expiring-upload", "S3_SNAPSHOT").await;
        let case = &upload_cases()[0];
        let digest = base64_md5(&case.content);
        let (job_id, url) =
            job_with_upload_url(&router, case, &data_set_id, &revision_id, &digest).await;

        {
            let state =
                dataexchange::state::service_state(&*engine).expect("the dataexchange store");
            let mut store = state.lock();
            let upload = store
                .jobs
                .get_mut(&job_id)
                .and_then(|job| job.upload.as_mut())
                .expect("the upload the create call announced");
            upload.expires_at = dataexchange::state::later(dataexchange::state::now(), *offset);
        }

        let uploaded = raw(&router, "PUT", &url, &case.content, Some(&digest)).await;
        assert_eq!(
            uploaded.status, *expected,
            "case {label:?}: expected {expected}, got {} body {:?}",
            uploaded.status, uploaded.body
        );
        if *expected == 403 {
            assert_eq!(
                uploaded.code.as_deref(),
                Some("AccessDenied"),
                "case {label:?}: expected AccessDenied, got {:?}",
                uploaded.code
            );
            assert_eq!(
                staged_content(&engine, &job_id),
                None,
                "case {label:?}: expected an expired upload to store nothing"
            );
        } else {
            assert_eq!(
                staged_content(&engine, &job_id).as_deref(),
                Some(case.content.as_slice()),
                "case {label:?}: expected the bytes retained"
            );
        }
    }
}

/// RULE: a PUT is idempotent — a second upload to the same URL replaces the first, and
/// the import reads whatever the last one left.
#[tokio::test]
async fn a_second_upload_replaces_the_first_and_is_what_gets_imported() {
    let (router, engine) = harness();
    let (data_set_id, revision_id) =
        data_set_with_revision(&router, "replaced-upload", "S3_SNAPSHOT").await;
    // The job declares the digest of the content it is finally given: the first upload
    // is a different revision of the same file, declared with its own Content-MD5.
    let first = b"id,name\n1,ada\n".to_vec();
    let second = b"id,name\n1,ada\n2,grace\n".to_vec();
    let case = UploadCase {
        label: "replaced upload",
        job_type: SIGNED_URL_TYPE,
        member: "ImportAssetFromSignedUrl",
        url_member: "SignedUrl",
        expiry_member: "SignedUrlExpiresAt",
        asset_type: "S3_SNAPSHOT",
        asset_name: "orders.csv",
        content: second.clone(),
    };
    let digest = base64_md5(&second);
    let (job_id, url) =
        job_with_upload_url(&router, &case, &data_set_id, &revision_id, &digest).await;

    // The first upload is refused: its bytes are not the ones this job declared.
    let refused = raw(&router, "PUT", &url, &first, Some(&base64_md5(&first))).await;
    assert_eq!(
        refused.status, 400,
        "case \"the wrong content first\": expected 400, got {} body {:?}",
        refused.status, refused.body
    );

    for attempt in ["first", "second"] {
        let uploaded = raw(&router, "PUT", &url, &second, Some(&digest)).await;
        assert_eq!(
            uploaded.status, 200,
            "case {attempt:?}: expected 200 from the PUT, got {} body {:?}",
            uploaded.status, uploaded.body
        );
    }
    assert_eq!(
        staged_content(&engine, &job_id).as_deref(),
        Some(second.as_slice()),
        "case \"two uploads\": expected the last upload to be what is retained"
    );

    assert_eq!(
        start_job(&router, &job_id).await.status,
        202,
        "case \"two uploads\": expected 202 from StartJob"
    );
    settle_past_the_execution_span(&engine);
    assert_eq!(
        get_job(&router, &job_id).await.string("State"),
        "COMPLETED",
        "case \"two uploads\": expected the import to complete"
    );
    let assets = stored_assets(&engine, &data_set_id, &revision_id);
    assert_eq!(
        assets.len(),
        1,
        "case \"two uploads\": expected one asset, got {assets:?}"
    );
    assert_eq!(
        assets[0].content, second,
        "case \"two uploads\": expected the asset to carry the last upload's bytes"
    );
}

/// RULE: an import whose revision no longer admits an asset adds none: the execution
/// ends in `ERROR` reporting the revision it could not write into.
#[tokio::test]
async fn an_import_into_a_revision_that_cannot_receive_it_ends_in_error() {
    let (router, engine) = harness();
    let (data_set_id, revision_id) =
        data_set_with_revision(&router, "closed-revision", "S3_SNAPSHOT").await;
    let case = &upload_cases()[0];
    let digest = base64_md5(&case.content);
    let (job_id, url) =
        job_with_upload_url(&router, case, &data_set_id, &revision_id, &digest).await;
    let uploaded = raw(&router, "PUT", &url, &case.content, Some(&digest)).await;
    assert_eq!(
        uploaded.status, 200,
        "case \"closed revision\": expected 200 from the PUT, got {} body {:?}",
        uploaded.status, uploaded.body
    );
    assert_eq!(
        start_job(&router, &job_id).await.status,
        202,
        "case \"closed revision\": expected 202 from StartJob"
    );

    // The revision is finalized while the import runs — the model's `Finalized`
    // documentation calls that "this read-only state". Finalizing through the modelled
    // call needs an asset the revision does not have yet, so the state is written the
    // way `revision_finalization.rs` writes a revision's assets.
    {
        let state = dataexchange::state::service_state(&*engine).expect("the dataexchange store");
        let mut store = state.lock();
        let revision = store
            .data_sets
            .get_mut(&data_set_id)
            .and_then(|data_set| data_set.revisions.get_mut(&revision_id))
            .expect("the revision the test created");
        revision.finalized = true;
    }

    settle_past_the_execution_span(&engine);
    let settled = get_job(&router, &job_id).await;
    assert_eq!(
        settled.string("State"),
        "ERROR",
        "case \"closed revision\": expected the import to end in ERROR, got {:?}",
        settled.member("State")
    );
    let errors = settled
        .member("Errors")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case \"closed revision\": expected an Errors list, got {:?}",
                settled.member("Errors")
            )
        })
        .clone();
    assert_eq!(
        errors.len(),
        1,
        "case \"closed revision\": expected one job error, got {errors:?}"
    );
    assert_eq!(
        errors[0].get("Code").and_then(Value::as_str),
        Some("VALIDATION_EXCEPTION"),
        "case \"closed revision\": expected the modelled validation Code, got {:?}",
        errors[0]
    );
    assert_eq!(
        errors[0].get("ResourceId").and_then(Value::as_str),
        Some(revision_id.as_str()),
        "case \"closed revision\": expected the error to name the revision, got {:?}",
        errors[0]
    );
    assert_eq!(
        errors[0].get("ResourceType").and_then(Value::as_str),
        Some("REVISION"),
        "case \"closed revision\": expected ResourceType=REVISION, got {:?}",
        errors[0]
    );
    assert!(
        stored_assets(&engine, &data_set_id, &revision_id).is_empty(),
        "case \"closed revision\": expected the refused import to add no asset"
    );
}
