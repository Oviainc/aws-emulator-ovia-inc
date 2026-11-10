// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `GetAssetContent`: the zip bundle one revision of an
//! asset's content is exported as, and how a request that names no reachable asset
//! or revision is refused.
//!
//! Every case drives the service through the same router the emulator serves, so
//! the routes, status codes, protocol envelope and state are all exercised.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_devopsagent_devopsagent::state::{
    asset_not_found_message, asset_version_not_found_message, ACCESS_DENIED_MESSAGE,
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

/// The wire error code (`__type`) of an error response.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `message` of an error response.
fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
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

/// A zip archive carrying `entries`, built with the same crate the handler writes
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

/// The `zip` content member of a `CreateAsset` request carrying `entries`.
fn zip_content(entries: &[(&str, &[u8])]) -> Value {
    json!({"zip": {"zipFile": aws_smithy_types::base64::encode(zip_bundle(entries))}})
}

/// Create an asset whose content is `content` and return its id.
async fn create_asset(
    router: &MultiServiceRouter,
    space: &str,
    case: &str,
    content: Value,
) -> String {
    let (status, body) = call(
        router,
        "POST",
        &format!("/asset/agent-space/{space}/assets"),
        Some(json!({
            "assetType": "artifact",
            "metadata": {"title": case},
            "content": content,
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateAsset, got {status}: {body}"
    );
    body["asset"]["assetId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no assetId in {body}"))
        .to_string()
}

/// The file-plane URI for one file of one asset.
fn asset_file_uri(space: &str, asset: &str, path: &str) -> String {
    format!("/asset/agent-space/{space}/assets/{asset}/files/{path}")
}

/// The `GetAssetContent` URI for one asset.
fn content_uri(space: &str, asset: &str) -> String {
    format!("/asset/agent-space/{space}/assets/{asset}/content")
}

/// Add one file to an asset through `CreateAssetFile`, asserting it was accepted.
async fn create_file(
    router: &MultiServiceRouter,
    space: &str,
    asset: &str,
    case: &str,
    path: &str,
    body: Value,
) {
    let uri = asset_file_uri(space, asset, path);
    let (status, created) = call(router, "POST", &uri, Some(json!({"content": body}))).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateAssetFile {uri}, got {status}: {created}"
    );
}

/// Replace one file's content through `UpdateAssetFile`, asserting it was accepted.
async fn update_file(
    router: &MultiServiceRouter,
    space: &str,
    asset: &str,
    case: &str,
    path: &str,
    body: Value,
) {
    let uri = asset_file_uri(space, asset, path);
    let (status, updated) = call(router, "PATCH", &uri, Some(json!({"content": body}))).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from UpdateAssetFile {uri}, got {status}: {updated}"
    );
}

/// The asset's `version`, as `GetAsset` reports it.
async fn asset_version(router: &MultiServiceRouter, space: &str, asset: &str, case: &str) -> i64 {
    let uri = format!("/asset/agent-space/{space}/assets/{asset}");
    let (status, body) = call(router, "GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetAsset, got {status}: {body}"
    );
    body["asset"]["version"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: no asset version in {body}"))
}

/// Export one asset's content, asserting a 200, and return the reported version
/// with the entries of the bundle it answered.
async fn export(
    router: &MultiServiceRouter,
    space: &str,
    asset: &str,
    case: &str,
    query: &str,
) -> (i64, BTreeMap<String, Vec<u8>>) {
    let uri = format!("{}{query}", content_uri(space, asset));
    let (status, body) = call(router, "GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetAssetContent {uri}, got {status}: {body}"
    );
    let version = body["version"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case:?}: no version in {body}"));
    let encoded = body["content"]["zipFile"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no content.zipFile in {body}"));
    let bundle = aws_smithy_types::base64::decode(encoded)
        .unwrap_or_else(|e| panic!("case {case:?}: content.zipFile is not base64 ({e}): {body}"));
    (version, unzip(case, &bundle))
}

/// The entries of one exported bundle, by the path each is stored at.
fn unzip(case: &str, bundle: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bundle.to_vec()))
        .unwrap_or_else(|e| panic!("case {case:?}: the export is not a zip archive ({e})"));
    let mut entries: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .unwrap_or_else(|e| panic!("case {case:?}: entry {index} could not be read ({e})"));
        let path = entry
            .name()
            .unwrap_or_else(|e| panic!("case {case:?}: entry {index} has no readable name ({e})"))
            .to_string();
        let mut content: Vec<u8> = Vec::new();
        entry
            .read_to_end(&mut content)
            .unwrap_or_else(|e| panic!("case {case:?}: entry {path} could not be read ({e})"));
        assert!(
            entries.insert(path.clone(), content).is_none(),
            "case {case:?}: the export carries {path} twice"
        );
    }
    entries
}

/// One row of the export table: how an asset's content was built, and the files it
/// must therefore export.
struct Row {
    case: &'static str,
    /// The `content` the asset was created with.
    content: Value,
    /// Files added to the asset afterwards through the file plane: the path and the
    /// body the request carried.
    added: Vec<(&'static str, Value)>,
    /// The content each path of the export must carry.
    expected: Vec<(&'static str, Vec<u8>)>,
}

/// RULE: an asset's content export is a zip bundle holding one entry per file the
/// named revision of the asset holds, each entry stored at that file's own path and
/// carrying exactly that file's content as bytes, reported with the version the
/// content belongs to.
#[tokio::test]
async fn asset_file_content_round_trips_through_zip_export() {
    let several_kib = "line\n".repeat(2_000);
    // The rows differ along what the rule generalises over: how the content
    // arrived (a single file, a zip bundle, files added through the file plane, an
    // import that materialises nothing), the body type of each file (text,
    // bytes), the number of files (none, one, several), the size of a file (empty,
    // small, several KiB) and the shape of its path (flat, nested, spaces and
    // parens).
    let rows: Vec<Row> = vec![
        Row {
            case: "a single text file",
            content: json!({"file": {"path": "SKILL.md", "body": {"text": "# how to deploy"}}}),
            added: vec![],
            expected: vec![("SKILL.md", b"# how to deploy".to_vec())],
        },
        Row {
            case: "a single bytes file at a nested path",
            content: json!({"file": {"path": "bin/tool.bin", "body": {"bytes": "AAECAw=="}}}),
            added: vec![],
            expected: vec![("bin/tool.bin", vec![0, 1, 2, 3])],
        },
        Row {
            case: "a single empty file",
            content: json!({"file": {"path": "empty.txt", "body": {"text": ""}}}),
            added: vec![],
            expected: vec![("empty.txt", Vec::new())],
        },
        Row {
            case: "a file of several KiB",
            content: json!({"file": {"path": "big.txt", "body": {"text": several_kib.clone()}}}),
            added: vec![],
            expected: vec![("big.txt", several_kib.clone().into_bytes())],
        },
        Row {
            case: "a bundle of one entry",
            content: zip_content(&[("only.md", b"only")]),
            added: vec![],
            expected: vec![("only.md", b"only".to_vec())],
        },
        Row {
            case: "a bundle of several entries past a directory entry",
            content: zip_content(&[
                ("dir/", b""),
                ("dir/one.md", b"one"),
                ("two (copy).md", b"two"),
                ("three.bin", &[0xff, 0x00, 0xfe]),
            ]),
            added: vec![],
            expected: vec![
                ("dir/one.md", b"one".to_vec()),
                ("three.bin", vec![0xff, 0x00, 0xfe]),
                ("two (copy).md", b"two".to_vec()),
            ],
        },
        Row {
            case: "a bundle the file plane added files to",
            content: zip_content(&[("bundled.md", b"bundled")]),
            added: vec![
                ("added/text.md", json!({"text": "added as text"})),
                ("added/bytes.bin", json!({"bytes": "CQoL"})),
            ],
            expected: vec![
                ("added/bytes.bin", vec![9, 10, 11]),
                ("added/text.md", b"added as text".to_vec()),
                ("bundled.md", b"bundled".to_vec()),
            ],
        },
        Row {
            case: "an import from a source url materialises no file",
            content: json!({"sourceUrl": {"url": "https://example.com/bundle.zip"}}),
            added: vec![],
            expected: vec![],
        },
    ];

    let router = new_router();
    let space = create_space(&router, "asset-content-export").await;

    for row in &rows {
        let case = row.case;
        let asset = create_asset(&router, &space, case, row.content.clone()).await;
        for (path, body) in &row.added {
            create_file(&router, &space, &asset, case, path, body.clone()).await;
        }

        let expected: BTreeMap<String, Vec<u8>> = row
            .expected
            .iter()
            .map(|(path, content)| ((*path).to_string(), content.clone()))
            .collect();
        let version = asset_version(&router, &space, &asset, case).await;

        // With no `assetVersion`, the latest revision is exported.
        let (reported, entries) = export(&router, &space, &asset, case, "").await;
        assert_eq!(
            reported, version,
            "case {case:?}: the export reports version {reported}, not the asset's {version}"
        );
        assert_eq!(
            entries.keys().collect::<Vec<_>>(),
            expected.keys().collect::<Vec<_>>(),
            "case {case:?}: the export holds {:?}, not {:?}",
            entries.keys().collect::<Vec<_>>(),
            expected.keys().collect::<Vec<_>>()
        );
        for (path, content) in &expected {
            let actual = &entries[path];
            assert_eq!(
                actual, content,
                "case {case:?}: entry {path} carries {} bytes, not the stored {}",
                actual.len(),
                content.len()
            );
        }

        // Naming the revision the asset is at exports the same content.
        let (named, at_version) =
            export(&router, &space, &asset, case, &format!("?assetVersion={version}")).await;
        assert_eq!(
            named, version,
            "case {case:?}: an export of version {version} reports {named}"
        );
        assert_eq!(
            at_version, entries,
            "case {case:?}: naming the current version exported different content"
        );

        // A read changes nothing a later caller sees: the asset is still at the
        // same revision and exports the same bundle.
        let after = asset_version(&router, &space, &asset, case).await;
        assert_eq!(
            after, version,
            "case {case:?}: exporting the content moved the asset to version {after}"
        );
        let (again, repeated) = export(&router, &space, &asset, case, "").await;
        assert_eq!(
            (again, &repeated),
            (version, &entries),
            "case {case:?}: two exports of one revision disagree"
        );
    }
}

/// One revision of an asset's content: the version it is, and the content each
/// file the asset held at it carried.
struct Snapshot {
    version: i64,
    files: Vec<(&'static str, Vec<u8>)>,
}

/// RULE: an export of an earlier revision of an asset carries the content that
/// revision held, and an export of the asset's current revision the content it
/// holds now.
#[tokio::test]
async fn an_earlier_revision_exports_the_content_it_held() {
    let router = new_router();
    let space = create_space(&router, "asset-content-revisions").await;
    let case = "content replaced three times";
    let asset = create_asset(&router, &space, case, zip_content(&[("doc.md", b"revision 1")])).await;

    // Replacing a file's content mints the asset's next revision, so each revision
    // is a snapshot of the content at that point: the rows below are those
    // snapshots.
    let mut snapshots: Vec<Snapshot> = vec![Snapshot {
        version: 1,
        files: vec![("doc.md", b"revision 1".to_vec())],
    }];
    for revision in 2..=4i64 {
        let text = format!("revision {revision}");
        update_file(
            &router,
            &space,
            &asset,
            case,
            "doc.md",
            json!({"text": text.clone()}),
        )
        .await;
        snapshots.push(Snapshot {
            version: revision,
            files: vec![("doc.md", text.into_bytes())],
        });
    }
    // A file ADDED to the asset joins the revision the asset is at, so the newest
    // snapshot holds two files while the earlier ones hold one: the snapshots
    // differ in how many files they hold as well as in their content.
    create_file(
        &router,
        &space,
        &asset,
        case,
        "extra.md",
        json!({"text": "extra"}),
    )
    .await;
    snapshots
        .last_mut()
        .expect("at least the asset's first revision")
        .files
        .push(("extra.md", b"extra".to_vec()));

    let current = asset_version(&router, &space, &asset, case).await;
    assert_eq!(
        current, 4,
        "case {case:?}: expected the asset at revision 4, got {current}"
    );

    for snapshot in &snapshots {
        let version = snapshot.version;
        let row = format!("{case} at version {version}");
        let (reported, entries) =
            export(&router, &space, &asset, &row, &format!("?assetVersion={version}")).await;
        assert_eq!(
            reported, version,
            "case {row:?}: the export reports version {reported}, not the {version} it was asked for"
        );
        let expected: BTreeMap<String, Vec<u8>> = snapshot
            .files
            .iter()
            .map(|(path, content)| ((*path).to_string(), content.clone()))
            .collect();
        assert_eq!(
            entries, expected,
            "case {row:?}: the export holds {:?}, not the content that revision held",
            entries.keys().collect::<Vec<_>>()
        );

        // Naming the revision the asset is at is the same request as naming none.
        if version == current {
            let (latest_version, latest) = export(&router, &space, &asset, &row, "").await;
            assert_eq!(
                (latest_version, &latest),
                (current, &entries),
                "case {row:?}: an export naming no version differs from one naming {current}"
            );
        }
    }
}

/// One refusal row: the request, and the status, error code and message it must be
/// answered with.
struct Refusal {
    case: &'static str,
    /// Built from the space and asset the suite created; `None` uses them as they
    /// are.
    space: Option<&'static str>,
    asset: Option<&'static str>,
    query: String,
    status: u16,
    code: &'static str,
    message: String,
}

/// RULE: an export names a reachable asset and a revision that asset has; a request
/// that does not is refused with the modelled error for what is wrong, and no
/// content is served.
#[tokio::test]
async fn an_unreachable_asset_or_revision_is_refused() {
    let router = new_router();
    let space = create_space(&router, "asset-content-refusals").await;
    let case = "refusals";
    let asset = create_asset(&router, &space, case, zip_content(&[("doc.md", b"doc")])).await;

    // The rows differ along what is wrong with the request: the agent space, the
    // asset, and a revision the asset never reached — the one just past the one it
    // is at, and one far beyond it.
    let rows = vec![
        Refusal {
            case: "an agent space that does not exist",
            space: Some("as-00000000deadbeef"),
            asset: None,
            query: String::new(),
            status: 403,
            code: "AccessDeniedException",
            message: ACCESS_DENIED_MESSAGE.to_string(),
        },
        Refusal {
            case: "an asset the space does not hold",
            space: None,
            asset: Some("asset-does-not-exist"),
            query: String::new(),
            status: 404,
            code: "ResourceNotFoundException",
            message: asset_not_found_message("asset-does-not-exist"),
        },
        Refusal {
            case: "the revision just past the one the asset is at",
            space: None,
            asset: None,
            query: "?assetVersion=2".to_string(),
            status: 404,
            code: "ResourceNotFoundException",
            message: asset_version_not_found_message(&asset, 2),
        },
        Refusal {
            case: "a revision far beyond the one the asset is at",
            space: None,
            asset: None,
            query: "?assetVersion=9999".to_string(),
            status: 404,
            code: "ResourceNotFoundException",
            message: asset_version_not_found_message(&asset, 9999),
        },
        Refusal {
            case: "a revision below the first one, which the model admits no value of",
            space: None,
            asset: None,
            query: "?assetVersion=0".to_string(),
            status: 400,
            code: "ValidationException",
            message: String::new(),
        },
    ];

    for row in &rows {
        let case = row.case;
        let uri = format!(
            "{}{}",
            content_uri(
                row.space.unwrap_or(space.as_str()),
                row.asset.unwrap_or(asset.as_str())
            ),
            row.query
        );
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected {} from {uri}, got {status}: {body}",
            row.status
        );
        assert_eq!(
            error_code(&body),
            row.code,
            "case {case:?}: expected {}, got {}: {body}",
            row.code,
            error_code(&body)
        );
        if !row.message.is_empty() {
            assert_eq!(
                error_message(&body),
                row.message,
                "case {case:?}: expected the message {:?}, got {:?}",
                row.message,
                error_message(&body)
            );
        }
        assert!(
            body.get("content").is_none(),
            "case {case:?}: a refused export must carry no content: {body}"
        );
    }

    // The asset's content is still exportable after every refusal, so nothing above
    // changed what a later caller sees.
    let (version, entries) = export(&router, &space, &asset, "after the refusals", "").await;
    assert_eq!(
        version, 1,
        "case \"after the refusals\": expected the asset's first revision, got {version}"
    );
    assert_eq!(
        entries.keys().collect::<Vec<_>>(),
        vec!["doc.md"],
        "case \"after the refusals\": expected the stored file, got {:?}",
        entries.keys().collect::<Vec<_>>()
    );
}
