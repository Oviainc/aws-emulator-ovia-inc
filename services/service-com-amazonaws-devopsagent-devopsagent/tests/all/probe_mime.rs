// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP probes of the media type this service reports for the content
//! of an asset file: one probe per body variant the model admits, driven through
//! `CreateAsset`, `CreateAssetFile` and the `GetAssetFile` read-back.
//!
//! Every case drives the service through the same router the emulator serves, so
//! the routes, status codes, protocol envelope and state are all exercised.

use std::io::Write;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_devopsagent_devopsagent::state::{
    MEDIA_TYPE_MARKDOWN, MEDIA_TYPE_OCTET_STREAM,
};

/// A SigV4 credential scope naming this service's signing name (`aidevops`), which
/// is how the router resolves which service claims a request. Signature
/// verification is switched off for these tests, so the signature is a placeholder.
const AUTHORIZATION: &str = concat!(
    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-west-2/aidevops/aws4_request, ",
    "SignedHeaders=host, Signature=0123456789abcdef"
);

/// One emulator: a single shared engine behind the production router.
fn new_router() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_devopsagent_devopsagent::EmulatedService::new(Arc::new(
            engine::InMemoryEngine::new(),
        )),
    );
    router
}

/// Send one request and return its status and parsed body (`Value::Null` for an
/// empty body).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body
        .as_ref()
        .map(|b| Bytes::from(serde_json::to_vec(b).expect("serialize request body")))
        .unwrap_or_default();
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "aidevops.us-west-2.amazonaws.com")
        .header("authorization", AUTHORIZATION);
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("build request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "response body is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// Create an agent space and return its id.
async fn create_space(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(router, "POST", "/v1/agentspaces", Some(json!({"name": name}))).await;
    assert_eq!(
        status, 201,
        "case {name:?}: expected 201 from CreateAgentSpace, got {status}: {body}"
    );
    body["agentSpace"]["agentSpaceId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: no agentSpaceId in {body}"))
        .to_string()
}

fn asset_file_uri(space: &str, asset: &str, path: &str) -> String {
    format!("/asset/agent-space/{space}/assets/{asset}/files/{path}")
}

/// A zip archive carrying `entries`, built with the same crate the handler reads
/// one with.
fn zip_bundle(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        for (path, content) in entries {
            writer
                .start_file(*path, zip::write::SimpleFileOptions::default())
                .expect("start zip entry");
            writer.write_all(content).expect("write zip entry");
        }
        writer.finish().expect("finish zip");
    }
    buffer.into_inner()
}

/// The document `described` with `field` set to `value` where it names none: the
/// expectation the rule under test states, written out independently of the
/// handler's own helper.
fn completed_with(described: Option<&Value>, field: &str, value: &str) -> Value {
    let mut members: Map<String, Value> = match described {
        Some(Value::Object(members)) => members.clone(),
        Some(other) => return other.clone(),
        None => Map::new(),
    };
    members
        .entry(field.to_string())
        .or_insert_with(|| json!(value));
    Value::Object(members)
}

/// One probe: how the file arrives, what the caller described about it, and the
/// media type its stored content has.
struct FileCase {
    name: &'static str,
    /// The file's path, as the asset holds it.
    path: &'static str,
    /// The `metadata` document the request described the file with, if any.
    described: Option<Value>,
    /// The media type of the body the file is stored with.
    media_type: &'static str,
}

/// RULE: the `metadata` document this service serves for an asset file carries the
/// members the caller described it with, completed with the member the SERVICE
/// maintains — the media type of the content it holds, under `mimeType`: text
/// content is markdown, bytes are an opaque stream — and a member the caller
/// described is never replaced.
#[tokio::test]
async fn served_file_metadata_carries_the_media_type_of_its_content() {
    // The rows differ along what the rule generalises over: the body variant the
    // content arrived in (text, bytes), whether a metadata document was described
    // for the file, whether it described the service's own member itself, and the
    // shape of the path (flat, nested, an extension that says something else about
    // the content, none at all).
    let cases: Vec<FileCase> = vec![
        FileCase {
            name: "text body, nothing described",
            path: "notes.md",
            described: None,
            media_type: MEDIA_TYPE_MARKDOWN,
        },
        FileCase {
            name: "text body at a path whose extension is not markdown",
            path: "data.json",
            described: None,
            media_type: MEDIA_TYPE_MARKDOWN,
        },
        FileCase {
            name: "text body at a path with no extension",
            path: "dir/README",
            described: None,
            media_type: MEDIA_TYPE_MARKDOWN,
        },
        FileCase {
            name: "bytes body",
            path: "payload.bin",
            described: None,
            media_type: MEDIA_TYPE_OCTET_STREAM,
        },
        FileCase {
            name: "bytes body at a path whose extension is markdown",
            path: "binary.md",
            described: None,
            media_type: MEDIA_TYPE_OCTET_STREAM,
        },
        FileCase {
            name: "text body with other members described",
            path: "described.md",
            described: Some(json!({"reviewed": true, "owner": "team"})),
            media_type: MEDIA_TYPE_MARKDOWN,
        },
        FileCase {
            name: "the caller describes the service's own member",
            path: "typed.md",
            described: Some(json!({"mimeType": "text/plain"})),
            media_type: MEDIA_TYPE_MARKDOWN,
        },
    ];

    let router = new_router();
    let space = create_space(&router, "file-media-type-space").await;
    // An asset whose content is a BUNDLE holds a set of files, so the file plane
    // can add each case's file to it.
    let (status, created) = call(
        &router,
        "POST",
        &format!("/asset/agent-space/{space}/assets"),
        Some(json!({
            "assetType": "artifact",
            "metadata": {"title": "file media types"},
            "content": {
                "zip": {
                    "zipFile": aws_smithy_types::base64::encode(
                        zip_bundle(&[("bundled.md", b"bundled")]),
                    )
                }
            },
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "expected 201 from CreateAsset, got {status}: {created}"
    );
    let asset = created["asset"]["assetId"]
        .as_str()
        .expect("no assetId from CreateAsset")
        .to_string();

    for case in &cases {
        let name = case.name;
        let body = if case.media_type == MEDIA_TYPE_OCTET_STREAM {
            json!({"bytes": "AAECAw=="})
        } else {
            json!({"text": "# content"})
        };
        let mut request = json!({"content": body});
        if let Some(described) = &case.described {
            request["metadata"] = described.clone();
        }

        let (status, created) = call(
            &router,
            "POST",
            &asset_file_uri(&space, &asset, case.path),
            Some(request),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {name:?}: expected 201 from CreateAssetFile, got {status}: {created}"
        );
        let expected = completed_with(case.described.as_ref(), "mimeType", case.media_type);
        assert_eq!(
            created["file"]["metadata"], expected,
            "case {name:?}: CreateAssetFile served metadata {:?}, expected {expected:?}",
            created["file"]["metadata"]
        );

        // READ BACK: the same document comes off a read of the stored file, so the
        // create and the read cannot disagree about it.
        let (status, fetched) = call(
            &router,
            "GET",
            &asset_file_uri(&space, &asset, case.path),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected 200 from GetAssetFile, got {status}: {fetched}"
        );
        assert_eq!(
            fetched["file"]["metadata"], expected,
            "case {name:?}: GetAssetFile served metadata {:?}, expected {expected:?}",
            fetched["file"]["metadata"]
        );
    }

    // The same rule holds for a file that arrived as an entry of a bundle, whose
    // content is bytes and which no request described anything about, and for the
    // one file an asset's content IS.
    let (status, bundled) = call(
        &router,
        "GET",
        &asset_file_uri(&space, &asset, "bundled.md"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "bundled entry: expected 200 from GetAssetFile, got {status}: {bundled}"
    );
    assert_eq!(
        bundled["file"]["metadata"],
        json!({"mimeType": MEDIA_TYPE_OCTET_STREAM}),
        "bundled entry: served metadata {:?}",
        bundled["file"]["metadata"]
    );

    let (status, one_file) = call(
        &router,
        "POST",
        &format!("/asset/agent-space/{space}/assets"),
        Some(json!({
            "assetType": "artifact",
            "metadata": {"title": "one file"},
            "content": {"file": {"path": "whole.json", "body": {"text": "# whole"}}},
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "one-file asset: expected 201 from CreateAsset, got {status}: {one_file}"
    );
    let one_file_asset = one_file["asset"]["assetId"]
        .as_str()
        .expect("no assetId from CreateAsset")
        .to_string();
    let (status, whole) = call(
        &router,
        "GET",
        &asset_file_uri(&space, &one_file_asset, "whole.json"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "one-file asset: expected 200 from GetAssetFile, got {status}: {whole}"
    );
    assert_eq!(
        whole["file"]["metadata"],
        json!({"mimeType": MEDIA_TYPE_MARKDOWN}),
        "one-file asset: served metadata {:?}",
        whole["file"]["metadata"]
    );
}
