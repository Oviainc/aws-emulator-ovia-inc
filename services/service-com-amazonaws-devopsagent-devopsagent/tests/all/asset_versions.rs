// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `ListAssetVersions`: the revision history one asset
//! reports, how that history is paginated, and how a request that names no
//! reachable asset is refused.
//!
//! Every case drives the service through the same router the emulator serves, so
//! the routes, status codes, protocol envelope and state are all exercised.

use std::io::Write;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_devopsagent_devopsagent::state::{
    asset_not_found_message, ACCESS_DENIED_MESSAGE, MAX_ASSET_VERSIONS_RETAINED,
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

/// The file every asset in these cases is created holding.
const BUNDLED: &str = "bundled.md";

/// A second file every asset gets, so a case can delete one and still hold content.
const EXTRA: &str = "extra.md";

/// Create an asset whose content arrived as a bundle — so the file plane may add
/// files to it — and return its id.
async fn create_asset(router: &MultiServiceRouter, space: &str, case: &str) -> String {
    let zip = aws_smithy_types::base64::encode(zip_bundle(&[(BUNDLED, b"bundled")]));
    let (status, body) = call(
        router,
        "POST",
        &format!("/asset/agent-space/{space}/assets"),
        Some(json!({
            "assetType": "artifact",
            "metadata": {"title": case},
            "content": {"zip": {"zipFile": zip}},
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateAsset, got {status}: {body}"
    );
    let asset = body["asset"]["assetId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no assetId in {body}"))
        .to_string();

    let (status, created) = call(
        router,
        "POST",
        &asset_file_uri(space, &asset, EXTRA),
        Some(json!({"content": {"text": "extra"}})),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateAssetFile, got {status}: {created}"
    );
    asset
}

/// The `ListAssetVersions` URI for one asset.
fn versions_uri(space: &str, asset: &str) -> String {
    format!("/asset/agent-space/{space}/assets/{asset}/versions")
}

/// The file-plane URI for one file of one asset.
fn asset_file_uri(space: &str, asset: &str, path: &str) -> String {
    format!("/asset/agent-space/{space}/assets/{asset}/files/{path}")
}

/// The `GetAsset` URI for one asset.
fn asset_uri(space: &str, asset: &str) -> String {
    format!("/asset/agent-space/{space}/assets/{asset}")
}

/// List one asset's revisions, asserting a 200, and return the response body.
async fn list_versions(
    router: &MultiServiceRouter,
    space: &str,
    asset: &str,
    case: &str,
    query: &str,
) -> Value {
    let uri = format!("{}{query}", versions_uri(space, asset));
    let (status, body) = call(router, "GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from ListAssetVersions {uri}, got {status}: {body}"
    );
    body
}

/// The `items` of a listing, as a vector.
fn items(case: &str, body: &Value) -> Vec<Value> {
    body["items"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no items list in {body}"))
        .clone()
}

/// The epoch-seconds instant one member of a reported revision carries.
///
/// A timestamp renders as restJson1's default epoch seconds here, so a case can
/// order two instants by reading them as numbers.
fn instant(case: &str, item: &Value, member: &str) -> f64 {
    item.get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case:?}: no {member} instant in {item}"))
}

/// The asset as `GetAsset` reports it.
async fn get_asset(router: &MultiServiceRouter, space: &str, asset: &str, case: &str) -> Value {
    let (status, body) = call(router, "GET", &asset_uri(space, asset), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetAsset, got {status}: {body}"
    );
    body["asset"].clone()
}

/// One change a case applies to an asset.
#[derive(Debug, Clone, Copy)]
enum Change {
    /// Replace the content of one file: a change to the asset's content.
    Content(&'static str),
    /// Update one file's metadata only: also a change to that file.
    Metadata(&'static str),
    /// Delete one file of the asset's content.
    Delete(&'static str),
    /// An update that names neither content nor metadata, so it changes nothing.
    Nothing(&'static str),
}

impl Change {
    /// Whether this change moves the asset to its next revision.
    fn mints_a_revision(self) -> bool {
        !matches!(self, Change::Nothing(_))
    }
}

/// Apply one change to an asset through the wire, asserting it was accepted.
async fn apply(
    router: &MultiServiceRouter,
    space: &str,
    asset: &str,
    case: &str,
    change: Change,
    nonce: usize,
) {
    let (method, path, body) = match change {
        Change::Content(path) => (
            "PATCH",
            path,
            Some(json!({"content": {"text": format!("revision {nonce}")}})),
        ),
        Change::Metadata(path) => ("PATCH", path, Some(json!({"metadata": {"stage": nonce}}))),
        Change::Delete(path) => ("DELETE", path, None),
        Change::Nothing(path) => ("PATCH", path, Some(json!({}))),
    };
    let uri = asset_file_uri(space, asset, path);
    let (status, response) = call(router, method, &uri, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {method} {uri}, got {status}: {response}"
    );
}

/// One row of the history table: the changes applied to one asset.
struct Row {
    case: &'static str,
    changes: Vec<Change>,
}

/// RULE: a listing of an asset's versions reports one entry per revision the asset
/// still holds — every retained earlier revision and the revision it is at — in
/// increasing version order, each entry carrying that revision's version number,
/// the instant the asset reached it and the instant it was last changed.
#[tokio::test]
async fn the_listing_reports_every_retained_revision_of_the_asset() {
    // The rows differ along what the rule generalises over: how many revisions the
    // asset has been moved through (none, one, a handful, more than are retained)
    // and which kind of change minted each of them.
    let mut rows = vec![
        Row {
            case: "no change at all",
            changes: vec![],
        },
        Row {
            case: "one content update",
            changes: vec![Change::Content(BUNDLED)],
        },
        Row {
            case: "an update that changes nothing",
            changes: vec![Change::Nothing(BUNDLED)],
        },
        Row {
            case: "content then metadata",
            changes: vec![Change::Content(BUNDLED), Change::Metadata(BUNDLED)],
        },
        Row {
            case: "a delete after an update",
            changes: vec![Change::Content(BUNDLED), Change::Delete(EXTRA)],
        },
        Row {
            case: "a change that mints nothing between two that do",
            changes: vec![
                Change::Content(BUNDLED),
                Change::Nothing(BUNDLED),
                Change::Metadata(EXTRA),
            ],
        },
        Row {
            case: "five changes across two files",
            changes: vec![
                Change::Content(BUNDLED),
                Change::Content(EXTRA),
                Change::Metadata(BUNDLED),
                Change::Metadata(EXTRA),
                Change::Content(BUNDLED),
            ],
        },
    ];
    // More changes than this service retains revisions for, so the oldest ones have
    // been dropped and the history is the retained window.
    rows.push(Row {
        case: "more changes than revisions retained",
        changes: (0..MAX_ASSET_VERSIONS_RETAINED + 3)
            .map(|_| Change::Content(BUNDLED))
            .collect(),
    });

    let router = new_router();
    let space = create_space(&router, "asset-version-history").await;

    for row in &rows {
        let case = row.case;
        // One asset per row, so the history each row observes is its own.
        let asset = create_asset(&router, &space, case).await;
        let created = get_asset(&router, &space, &asset, case).await;

        for (nonce, change) in row.changes.iter().enumerate() {
            apply(&router, &space, &asset, case, *change, nonce).await;
        }
        let minted = row
            .changes
            .iter()
            .filter(|change| change.mints_a_revision())
            .count();

        let asset_now = get_asset(&router, &space, &asset, case).await;
        let current = asset_now["version"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: no asset version in {asset_now}"));
        assert_eq!(
            current,
            1 + minted as i64,
            "case {case:?}: expected the asset at revision {}, got {current}",
            1 + minted
        );

        let body = list_versions(&router, &space, &asset, case, "").await;
        let listed = items(case, &body);
        assert!(
            body.get("nextToken").is_none_or(Value::is_null),
            "case {case:?}: a listing that fits one page must carry no nextToken: {body}"
        );

        // Every revision the asset reached is reported, up to the number this
        // service retains behind the one it is at.
        let expected_len = (current as usize).min(MAX_ASSET_VERSIONS_RETAINED + 1);
        assert_eq!(
            listed.len(),
            expected_len,
            "case {case:?}: expected {expected_len} revisions, got {}: {body}",
            listed.len()
        );

        // Increasing version order, consecutive, ending at the revision the asset
        // is at.
        let oldest = current - listed.len() as i64 + 1;
        for (offset, item) in listed.iter().enumerate() {
            assert_eq!(
                item["version"],
                json!(oldest + offset as i64),
                "case {case:?}: entry {offset} reports version {:?}, not {}",
                item["version"],
                oldest + offset as i64
            );
            let created_at = instant(case, item, "createdAt");
            let updated_at = instant(case, item, "updatedAt");
            assert!(
                updated_at >= created_at,
                "case {case:?}: revision {:?} was updated at {updated_at} before it was created at {created_at}",
                item["version"]
            );
            if offset > 0 {
                let earlier = instant(case, &listed[offset - 1], "createdAt");
                assert!(
                    created_at >= earlier,
                    "case {case:?}: revision {:?} was created at {created_at}, before the revision below it at {earlier}",
                    item["version"]
                );
            }
        }

        // The revision the asset is at is the asset's own instant of last change,
        // and the asset's first revision — while it is still retained — was created
        // with the asset itself.
        let newest = listed.last().expect("at least the current revision");
        assert_eq!(
            newest["updatedAt"], asset_now["updatedAt"],
            "case {case:?}: the current revision reports updatedAt {:?}, not the asset's {:?}",
            newest["updatedAt"], asset_now["updatedAt"]
        );
        if oldest == 1 {
            assert_eq!(
                listed[0]["createdAt"], created["createdAt"],
                "case {case:?}: revision 1 reports createdAt {:?}, not the asset's {:?}",
                listed[0]["createdAt"], created["createdAt"]
            );
        }

        // BACKED: every version the listing names resolves to content a read can
        // serve, and a version below the oldest listed one no longer does.
        for item in &listed {
            let version = item["version"].as_i64().expect("a version number");
            let uri = format!(
                "{}?assetVersion={version}",
                asset_file_uri(&space, &asset, BUNDLED)
            );
            let (status, file) = call(&router, "GET", &uri, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 reading {BUNDLED} at listed version {version}, got {status}: {file}"
            );
        }
        if oldest > 1 {
            let dropped = oldest - 1;
            let uri = format!(
                "{}?assetVersion={dropped}",
                asset_file_uri(&space, &asset, BUNDLED)
            );
            let (status, body) = call(&router, "GET", &uri, None).await;
            assert_eq!(
                status, 404,
                "case {case:?}: version {dropped} is not listed, so reading it must be a 404, got {status}: {body}"
            );
        }

        // A read changes nothing a later caller sees.
        let again = list_versions(&router, &space, &asset, case, "").await;
        assert_eq!(
            again, body,
            "case {case:?}: two listings of one history disagree: {again}"
        );
    }
}

/// RULE: a page carries at most as many revisions as `maxResults` names, and the
/// token a non-final page answers resumes the same listing immediately after the
/// revision that page ended on, so walking the pages yields the whole history once
/// and in order.
#[tokio::test]
async fn pagination_walks_the_whole_history_once() {
    let router = new_router();
    let space = create_space(&router, "asset-version-pagination").await;
    let case = "six revisions";
    let asset = create_asset(&router, &space, case).await;
    for nonce in 0..5 {
        apply(
            &router,
            &space,
            &asset,
            case,
            Change::Content(BUNDLED),
            nonce,
        )
        .await;
    }

    let whole = items(case, &list_versions(&router, &space, &asset, case, "").await);
    assert_eq!(
        whole.len(),
        6,
        "case {case:?}: expected 6 revisions before paginating, got {}",
        whole.len()
    );

    // The rows differ along the page width: one revision at a time, a width that
    // divides the history, one that does not, exactly the history, and the largest
    // page the model admits.
    for page_size in [1usize, 2, 4, 6, 100] {
        let case = format!("maxResults={page_size}");
        let mut walked: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let query = match &token {
                None => format!("?maxResults={page_size}"),
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
            };
            let body = list_versions(&router, &space, &asset, &case, &query).await;
            let page = items(&case, &body);
            pages += 1;
            assert!(
                pages <= whole.len() + 1,
                "case {case:?}: the walk did not terminate after {pages} pages"
            );
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page carries {} revisions, more than the {page_size} asked for",
                page.len()
            );
            token = body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if let Some(issued) = &token {
                assert_eq!(
                    page.len(),
                    page_size,
                    "case {case:?}: a page that continues must be full, got {} of {page_size}",
                    page.len()
                );
                assert_ne!(
                    issued,
                    &page
                        .last()
                        .expect("a full page")
                        .get("version")
                        .expect("a version number")
                        .to_string(),
                    "case {case:?}: the pagination token must be opaque, not the version it stopped after"
                );
            }
            walked.extend(page);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, whole,
            "case {case:?}: walking the pages yielded {walked:?}, not the whole history {whole:?}"
        );
        let expected_pages = whole.len().div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, took {pages}"
        );
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

/// RULE: a listing names a reachable asset and, when it resumes, a token this
/// service issued; a request that does not is refused with the modelled error for
/// what is wrong, and nothing is listed.
#[tokio::test]
async fn an_unreachable_asset_or_a_foreign_token_is_refused() {
    let router = new_router();
    let space = create_space(&router, "asset-version-refusals").await;
    let asset = create_asset(&router, &space, "refusals").await;

    // A pagination token this service DID issue for a different listing carries a
    // cursor naming an agent space, so it names no revision of an asset: the rows
    // below build one by hand to cover that.

    let rows = vec![
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
            case: "an agent space that does not exist",
            space: Some("as-00000000deadbeef"),
            asset: None,
            query: String::new(),
            status: 403,
            code: "AccessDeniedException",
            message: ACCESS_DENIED_MESSAGE.to_string(),
        },
        Refusal {
            case: "a nextToken this service never issued",
            space: None,
            asset: None,
            query: "?nextToken=not-a-token".to_string(),
            status: 400,
            code: "ValidationException",
            message: "nextToken is not a pagination token issued by this service".to_string(),
        },
        Refusal {
            case: "a nextToken with the right prefix but no cursor",
            space: None,
            asset: None,
            query: "?nextToken=v1:zzzz".to_string(),
            status: 400,
            code: "ValidationException",
            message: "nextToken is not a pagination token issued by this service".to_string(),
        },
        Refusal {
            case: "a nextToken issued for another listing",
            space: None,
            asset: None,
            // `v1:` + hex of `as-0000000000000001`, the cursor the agent-space
            // listing issues: a cursor that names no revision of an asset.
            query: format!(
                "?nextToken=v1:{}",
                hex::encode("as-0000000000000001".as_bytes())
            ),
            status: 400,
            code: "ValidationException",
            message: "nextToken is not a pagination token issued by this service".to_string(),
        },
    ];

    for row in &rows {
        let case = row.case;
        let uri = format!(
            "{}{}",
            versions_uri(
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
        assert_eq!(
            error_message(&body),
            row.message,
            "case {case:?}: expected the message {:?}, got {:?}",
            row.message,
            error_message(&body)
        );
        assert!(
            body.get("items").is_none(),
            "case {case:?}: a refused listing must carry no items: {body}"
        );
    }

    // The asset is still listable after every refusal, so nothing above changed
    // what a later caller sees.
    let listed = items(
        "after the refusals",
        &list_versions(&router, &space, &asset, "after the refusals", "").await,
    );
    assert_eq!(
        listed.len(),
        1,
        "case \"after the refusals\": expected the asset's one revision, got {listed:?}"
    );
}
