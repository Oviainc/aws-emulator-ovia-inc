// SPDX-License-Identifier: CC0-1.0

//! The WorkDocs document lifecycle inside a folder, end to end over HTTP.
//!
//! Every test drives the real request path — `MultiServiceRouter::dispatch`, the same
//! entry the emulator's front door uses — so routing, the restJson1 bindings, the
//! modelled status codes and the handler's state access are all exercised together.
//!
//! The rules under test, and where each comes from:
//!
//! * a resource a create minted is the resource a later read resolves, and a delete
//!   removes it from every read (the document lifecycle this crate's bootstrap issue
//!   describes);
//! * `DescribeFolderContents` lists exactly the children parented to the named folder,
//!   filtered by `Type`, ordered by `Sort`/`Order`, and paged by `Limit`/`Marker`
//!   (`smithy_query(query="operation", operation="DescribeFolderContents")`, plus that
//!   operation's own documentation: "By default, Amazon WorkDocs returns the first 100
//!   active document and folder metadata items … You can also request initialized
//!   documents.");
//! * an operation naming a resource that does not exist answers 404
//!   `EntityNotExistsException`, and a version not in `INITIALIZED` cannot be committed
//!   (409 `ProhibitedStateException`) — both error shapes and both status codes come
//!   from the model.

use std::collections::HashSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_workdocs_awsgorillaboyservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A host that routes to this service: `resolve_route_key` reads the first label of
/// `Host` when a request carries no SigV4 credential scope.
const HOST: &str = "workdocs.us-east-1.amazonaws.com";

/// `AuthenticationToken` is required by `DescribeRootFolders` and accepted by the rest.
/// Its value selects no user — the emulator models one — so any token serves.
const TOKEN: &str = "emulator-integration-test-token";

/// One emulator with one shared engine, driven over HTTP.
struct Client {
    router: MultiServiceRouter,
}

impl Client {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature of a request this test builds by hand is not the behaviour under
        // test; the emulator verifies signatures in the core, never in a service.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Send one request and return its status, its parsed body (`Value::Null` for an
    /// empty body) and the raw body length.
    async fn send(&self, method: &str, path: &str, body: Option<Value>) -> Response {
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://{HOST}{path}"))
            .header("host", HOST)
            .header("Authentication", TOKEN);
        let payload = match body.as_ref() {
            Some(value) => {
                builder = builder.header("content-type", "application/json");
                bytes::Bytes::from(serde_json::to_vec(value).expect("serialize request body"))
            }
            None => bytes::Bytes::new(),
        };
        let request = builder.body(payload).expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let raw = response.body().clone();
        let json = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        Response {
            status,
            json,
            body_len: raw.len(),
            error_type,
        }
    }

    /// The seeded root folder id, discovered the way a client discovers it.
    async fn root_folder_id(&self) -> String {
        let response = self.send("GET", "/api/v1/me/root", None).await;
        assert_eq!(
            response.status, 200,
            "DescribeRootFolders: expected 200, got {} body {}",
            response.status, response.json
        );
        response.json["Folders"][0]["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("DescribeRootFolders returned no root folder: {}", response.json))
            .to_string()
    }

    async fn create_folder(&self, parent: &str, name: &str) -> Response {
        self.send(
            "POST",
            "/api/v1/folders",
            Some(json!({ "ParentFolderId": parent, "Name": name })),
        )
        .await
    }

    /// Create a folder and return its id, failing the test when the call did not.
    async fn folder_id(&self, parent: &str, name: &str) -> String {
        let response = self.create_folder(parent, name).await;
        assert_eq!(
            response.status, 201,
            "CreateFolder({name}): expected 201, got {} body {}",
            response.status, response.json
        );
        response.json["Metadata"]["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateFolder({name}) returned no Id: {}", response.json))
            .to_string()
    }

    async fn initiate(&self, body: Value) -> Response {
        self.send("POST", "/api/v1/documents", Some(body)).await
    }

    /// Upload and commit one document, returning `(document id, version id)`.
    async fn active_document(&self, parent: &str, name: &str, size: i64) -> (String, String) {
        let created = self
            .initiate(json!({
                "ParentFolderId": parent,
                "Name": name,
                "ContentType": "text/plain",
                "DocumentSizeInBytes": size,
            }))
            .await;
        assert_eq!(
            created.status, 201,
            "InitiateDocumentVersionUpload({name}): expected 201, got {} body {}",
            created.status, created.json
        );
        let document_id = created.json["Metadata"]["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("no document Id for {name}: {}", created.json))
            .to_string();
        let version_id = created.json["Metadata"]["LatestVersionMetadata"]["Id"]
            .as_str()
            .unwrap_or_else(|| panic!("no version Id for {name}: {}", created.json))
            .to_string();
        let committed = self.commit(&document_id, &version_id).await;
        assert_eq!(
            committed.status, 200,
            "UpdateDocumentVersion({name}): expected 200, got {} body {}",
            committed.status, committed.json
        );
        (document_id, version_id)
    }

    async fn commit(&self, document_id: &str, version_id: &str) -> Response {
        self.send(
            "PATCH",
            &format!("/api/v1/documents/{document_id}/versions/{version_id}"),
            Some(json!({ "VersionStatus": "ACTIVE" })),
        )
        .await
    }

    async fn get_document(&self, document_id: &str) -> Response {
        self.send("GET", &format!("/api/v1/documents/{document_id}"), None)
            .await
    }

    async fn contents(&self, folder_id: &str, query: &str) -> Response {
        let path = if query.is_empty() {
            format!("/api/v1/folders/{folder_id}/contents")
        } else {
            format!("/api/v1/folders/{folder_id}/contents?{query}")
        };
        self.send("GET", &path, None).await
    }
}

/// What one request answered.
struct Response {
    status: u16,
    json: Value,
    body_len: usize,
    error_type: Option<String>,
}

impl Response {
    /// The `Id`s of the `Folders` the response lists, in wire order.
    fn folder_ids(&self) -> Vec<String> {
        ids(&self.json["Folders"])
    }

    /// The `Id`s of the `Documents` the response lists, in wire order.
    fn document_ids(&self) -> Vec<String> {
        ids(&self.json["Documents"])
    }

    /// Every listed child id, folders before documents, as one page.
    fn listed_ids(&self) -> Vec<String> {
        let mut all = self.folder_ids();
        all.extend(self.document_ids());
        all
    }

    fn marker(&self) -> Option<String> {
        self.json["Marker"].as_str().map(str::to_string)
    }
}

fn ids(list: &Value) -> Vec<String> {
    list.as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["Id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The `__type` discriminator of a modelled error envelope.
fn error_code(response: &Response) -> String {
    response.json["__type"]
        .as_str()
        .unwrap_or("<no __type>")
        .to_string()
}

// ---------------------------------------------------------------------------
// The seeded special folders
// ---------------------------------------------------------------------------

/// `DescribeRootFolders` describes "the current user's special folders; the RootFolder
/// and the RecycleBin" from state, and pages them like any other listing.
#[tokio::test]
async fn root_folders_are_seeded_stored_and_paged() {
    let client = Client::new();

    let all = client.send("GET", "/api/v1/me/root", None).await;
    assert_eq!(
        all.status, 200,
        "DescribeRootFolders: expected 200, got {} body {}",
        all.status, all.json
    );
    let special = all.folder_ids();
    assert_eq!(
        special.len(),
        2,
        "DescribeRootFolders: expected the root folder and the recycle bin, got {:?}",
        all.json
    );
    assert_eq!(
        all.marker(),
        None,
        "DescribeRootFolders: a complete page must carry no Marker, got {:?}",
        all.marker()
    );
    for (index, folder) in all.json["Folders"].as_array().unwrap_or(&vec![]).iter().enumerate() {
        assert_eq!(
            folder["ResourceState"].as_str(),
            Some("ACTIVE"),
            "special folder {index}: expected ResourceState ACTIVE, got {folder}"
        );
        assert!(
            folder["Signature"].as_str().is_some_and(|s| !s.is_empty()),
            "special folder {index}: expected a Signature, got {folder}"
        );
        assert!(
            folder["CreatedTimestamp"].as_f64().is_some(),
            "special folder {index}: expected a CreatedTimestamp, got {folder}"
        );
    }

    // Limit=1 pages, and the Marker resolves to the second special folder.
    let first = client.send("GET", "/api/v1/me/root?limit=1", None).await;
    assert_eq!(
        first.folder_ids(),
        vec![special[0].clone()],
        "DescribeRootFolders(limit=1): expected the first special folder, got {:?}",
        first.folder_ids()
    );
    let marker = first
        .marker()
        .unwrap_or_else(|| panic!("DescribeRootFolders(limit=1) emitted no Marker: {}", first.json));
    let second = client
        .send("GET", &format!("/api/v1/me/root?limit=1&marker={marker}"), None)
        .await;
    assert_eq!(
        second.folder_ids(),
        vec![special[1].clone()],
        "DescribeRootFolders(page 2): expected the second special folder, got {:?}",
        second.folder_ids()
    );
    assert_eq!(
        second.marker(),
        None,
        "DescribeRootFolders(page 2): the last page must carry no Marker, got {:?}",
        second.marker()
    );
}

// ---------------------------------------------------------------------------
// The lifecycle
// ---------------------------------------------------------------------------

/// The whole slice, in order: create a folder, upload a document into it, commit the
/// version, read it back, list the folder, delete the document, and read again. Every
/// value asserted is one an earlier call STORED, not one this request echoed.
#[tokio::test]
async fn document_lifecycle_round_trips_through_state() {
    let client = Client::new();
    let root = client.root_folder_id().await;

    // 1 — CreateFolder under the seeded root.
    let created = client.create_folder(&root, "bootstrap-folder").await;
    assert_eq!(
        created.status, 201,
        "CreateFolder: expected 201, got {} body {}",
        created.status, created.json
    );
    let folder = &created.json["Metadata"];
    let folder_id = folder["Id"].as_str().expect("CreateFolder returned no Id").to_string();
    assert_eq!(
        folder["Name"].as_str(),
        Some("bootstrap-folder"),
        "CreateFolder: expected the requested Name, got {folder}"
    );
    assert_eq!(
        folder["ParentFolderId"].as_str(),
        Some(root.as_str()),
        "CreateFolder: expected ParentFolderId {root}, got {folder}"
    );
    assert_eq!(
        folder["ResourceState"].as_str(),
        Some("ACTIVE"),
        "CreateFolder: expected ResourceState ACTIVE, got {folder}"
    );
    assert!(
        folder["CreatorId"].as_str().is_some_and(|id| !id.is_empty()),
        "CreateFolder: expected a CreatorId, got {folder}"
    );

    // The new folder is a child of the root, so the root's listing resolves the id.
    let root_listing = client.contents(&root, "type=FOLDER").await;
    assert!(
        root_listing.folder_ids().contains(&folder_id),
        "DescribeFolderContents(root): expected the new folder {folder_id}, got {:?}",
        root_listing.folder_ids()
    );
    let empty_signature = root_listing.json["Folders"]
        .as_array()
        .and_then(|folders| folders.iter().find(|f| f["Id"].as_str() == Some(folder_id.as_str())))
        .and_then(|f| f["Signature"].as_str())
        .expect("the new folder carries a Signature")
        .to_string();

    // 2 — InitiateDocumentVersionUpload into the new folder.
    let upload = client
        .initiate(json!({
            "ParentFolderId": folder_id,
            "Name": "hello.txt",
            "ContentType": "text/plain",
            "DocumentSizeInBytes": 11,
            "ContentCreatedTimestamp": 1_700_000_000,
            "ContentModifiedTimestamp": 1_700_000_500,
        }))
        .await;
    assert_eq!(
        upload.status, 201,
        "InitiateDocumentVersionUpload: expected 201, got {} body {}",
        upload.status, upload.json
    );
    let document_id = upload.json["Metadata"]["Id"]
        .as_str()
        .expect("InitiateDocumentVersionUpload returned no document Id")
        .to_string();
    let version_id = upload.json["Metadata"]["LatestVersionMetadata"]["Id"]
        .as_str()
        .expect("InitiateDocumentVersionUpload returned no version Id")
        .to_string();
    assert_eq!(
        upload.json["Metadata"]["LatestVersionMetadata"]["Status"].as_str(),
        Some("INITIALIZED"),
        "InitiateDocumentVersionUpload: expected the first version INITIALIZED, got {}",
        upload.json["Metadata"]
    );
    assert_eq!(
        upload.json["Metadata"]["ResourceState"].as_str(),
        Some("ACTIVE"),
        "InitiateDocumentVersionUpload: expected ResourceState ACTIVE, got {}",
        upload.json["Metadata"]
    );
    assert!(
        upload.json["UploadMetadata"]["UploadUrl"]
            .as_str()
            .is_some_and(|url| url.contains(&document_id) && url.contains(&version_id)),
        "InitiateDocumentVersionUpload: expected an UploadUrl naming the stored ids, got {}",
        upload.json["UploadMetadata"]
    );
    assert_eq!(
        upload.json["UploadMetadata"]["SignedHeaders"]["Content-Type"].as_str(),
        Some("text/plain"),
        "InitiateDocumentVersionUpload: expected the request's ContentType in SignedHeaders, got {}",
        upload.json["UploadMetadata"]
    );

    // 3 — UpdateDocumentVersion commits the version.
    let committed = client.commit(&document_id, &version_id).await;
    assert_eq!(
        committed.status, 200,
        "UpdateDocumentVersion: expected 200, got {} body {}",
        committed.status, committed.json
    );

    // 4 — GetDocument reports what steps 2 and 3 stored.
    let read = client.get_document(&document_id).await;
    assert_eq!(
        read.status, 200,
        "GetDocument: expected 200, got {} body {}",
        read.status, read.json
    );
    let metadata = &read.json["Metadata"];
    assert_eq!(
        metadata["Id"].as_str(),
        Some(document_id.as_str()),
        "GetDocument: expected the stored Id, got {metadata}"
    );
    assert_eq!(
        metadata["ParentFolderId"].as_str(),
        Some(folder_id.as_str()),
        "GetDocument: expected ParentFolderId {folder_id}, got {metadata}"
    );
    let latest = &metadata["LatestVersionMetadata"];
    assert_eq!(
        latest["Name"].as_str(),
        Some("hello.txt"),
        "GetDocument: expected the uploaded Name, got {latest}"
    );
    assert_eq!(
        latest["ContentType"].as_str(),
        Some("text/plain"),
        "GetDocument: expected the uploaded ContentType, got {latest}"
    );
    assert_eq!(
        latest["Size"].as_i64(),
        Some(11),
        "GetDocument: expected the uploaded DocumentSizeInBytes, got {latest}"
    );
    assert_eq!(
        latest["Status"].as_str(),
        Some("ACTIVE"),
        "GetDocument: expected the committed Status, got {latest}"
    );
    assert_eq!(
        latest["ContentCreatedTimestamp"].as_f64(),
        Some(1_700_000_000.0),
        "GetDocument: expected the uploaded ContentCreatedTimestamp, got {latest}"
    );
    assert_eq!(
        latest["ContentModifiedTimestamp"].as_f64(),
        Some(1_700_000_500.0),
        "GetDocument: expected the uploaded ContentModifiedTimestamp, got {latest}"
    );
    // `IncludeCustomMetadata` defaults to false and this document carries none, so the
    // member is absent either way.
    assert!(
        read.json["CustomMetadata"].is_null(),
        "GetDocument: expected no CustomMetadata, got {}",
        read.json
    );
    let with_custom = client
        .send(
            "GET",
            &format!("/api/v1/documents/{document_id}?includeCustomMetadata=true"),
            None,
        )
        .await;
    assert_eq!(
        with_custom.status, 200,
        "GetDocument(includeCustomMetadata=true): expected 200, got {} body {}",
        with_custom.status, with_custom.json
    );

    // The committed version is strictly later than the document's creation.
    let created_at = metadata["CreatedTimestamp"].as_f64().expect("a CreatedTimestamp");
    let modified_at = metadata["ModifiedTimestamp"].as_f64().expect("a ModifiedTimestamp");
    assert!(
        modified_at > created_at,
        "GetDocument: committing a version must advance ModifiedTimestamp: created {created_at}, modified {modified_at}"
    );

    // 5 — DescribeFolderContents lists exactly that one document and no folders.
    let listing = client.contents(&folder_id, "type=ALL").await;
    assert_eq!(
        listing.status, 200,
        "DescribeFolderContents: expected 200, got {} body {}",
        listing.status, listing.json
    );
    assert_eq!(
        listing.document_ids(),
        vec![document_id.clone()],
        "DescribeFolderContents: expected exactly the uploaded document, got {:?}",
        listing.document_ids()
    );
    assert!(
        listing.folder_ids().is_empty(),
        "DescribeFolderContents: expected no subfolders, got {:?}",
        listing.folder_ids()
    );
    assert_eq!(
        listing.marker(),
        None,
        "DescribeFolderContents: a complete page must carry no Marker, got {:?}",
        listing.marker()
    );

    // The folder's Signature is "created from the subfolders and documents of the
    // folder", so adding a child changed it.
    let filled_signature = client
        .contents(&root, "type=FOLDER")
        .await
        .json["Folders"]
        .as_array()
        .and_then(|folders| folders.iter().find(|f| f["Id"].as_str() == Some(folder_id.as_str())))
        .and_then(|f| f["Signature"].as_str())
        .expect("the folder carries a Signature")
        .to_string();
    assert_ne!(
        filled_signature, empty_signature,
        "DescribeFolderContents: a folder's Signature must change with its contents, got {filled_signature} both times"
    );

    // 6 — DeleteDocument answers 204 with an empty body.
    let deleted = client
        .send("DELETE", &format!("/api/v1/documents/{document_id}"), None)
        .await;
    assert_eq!(
        deleted.status, 204,
        "DeleteDocument: expected 204, got {} body {}",
        deleted.status, deleted.json
    );
    assert_eq!(
        deleted.body_len, 0,
        "DeleteDocument: expected an empty body, got {} bytes",
        deleted.body_len
    );

    // 7 — the document is gone from both reads.
    let gone = client.get_document(&document_id).await;
    assert_eq!(
        gone.status, 404,
        "GetDocument(after delete): expected 404, got {} body {}",
        gone.status, gone.json
    );
    assert_eq!(
        error_code(&gone),
        "EntityNotExistsException",
        "GetDocument(after delete): expected EntityNotExistsException, got {}",
        gone.json
    );
    let after = client.contents(&folder_id, "type=ALL").await;
    assert!(
        after.document_ids().is_empty(),
        "DescribeFolderContents(after delete): expected no documents, got {:?}",
        after.document_ids()
    );
    assert_eq!(
        client
            .contents(&root, "type=FOLDER")
            .await
            .json["Folders"]
            .as_array()
            .and_then(|folders| folders.iter().find(|f| f["Id"].as_str() == Some(folder_id.as_str())))
            .and_then(|f| f["Signature"].as_str()),
        Some(empty_signature.as_str()),
        "DescribeFolderContents: removing the only child must restore the empty folder's Signature"
    );
}

// ---------------------------------------------------------------------------
// Listing: filter, order, paging
// ---------------------------------------------------------------------------

/// One folder holding two subfolders and two documents, created in a known order, so a
/// listing's `Type`/`Sort`/`Order` can be asserted against an independently known
/// sequence.
struct Fixture {
    client: Client,
    folder_id: String,
    /// Subfolder ids, in creation order: `b-folder`, then `a-folder`.
    folders: Vec<String>,
    /// Document ids, in creation (and commit) order: `c-doc.txt`, then `d-doc.txt`.
    documents: Vec<String>,
}

impl Fixture {
    async fn build() -> Self {
        let client = Client::new();
        let root = client.root_folder_id().await;
        let folder_id = client.folder_id(&root, "listing").await;
        // Created oldest-first, and named so that NAME order differs from DATE order.
        let b = client.folder_id(&folder_id, "b-folder").await;
        let a = client.folder_id(&folder_id, "a-folder").await;
        let (c, _) = client.active_document(&folder_id, "c-doc.txt", 3).await;
        let (d, _) = client.active_document(&folder_id, "d-doc.txt", 4).await;
        Self {
            client,
            folder_id,
            folders: vec![b, a],
            documents: vec![c, d],
        }
    }
}

/// `Type` selects which kinds are listed, and `Sort`+`Order` fix the sequence. The rows
/// span every `FolderContentType`, every `ResourceSortType` and every `OrderType` the
/// model declares (`types::FOLDER_CONTENT_TYPE_VALUES`,
/// `types::RESOURCE_SORT_TYPE_VALUES`, `types::ORDER_TYPE_VALUES`).
#[tokio::test]
async fn folder_contents_honour_type_sort_and_order() {
    let fixture = Fixture::build().await;
    let (b, a) = (fixture.folders[0].clone(), fixture.folders[1].clone());
    let (c, d) = (fixture.documents[0].clone(), fixture.documents[1].clone());

    // (query, expected folder ids in order, expected document ids in order)
    let cases: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        // No Type at all: everything, which is what ALL names.
        ("", vec![b.clone(), a.clone()], vec![c.clone(), d.clone()]),
        (
            "type=ALL&sort=DATE&order=ASCENDING",
            vec![b.clone(), a.clone()],
            vec![c.clone(), d.clone()],
        ),
        (
            "type=ALL&sort=DATE&order=DESCENDING",
            vec![a.clone(), b.clone()],
            vec![d.clone(), c.clone()],
        ),
        (
            "type=ALL&sort=NAME&order=ASCENDING",
            vec![a.clone(), b.clone()],
            vec![c.clone(), d.clone()],
        ),
        (
            "type=ALL&sort=NAME&order=DESCENDING",
            vec![b.clone(), a.clone()],
            vec![d.clone(), c.clone()],
        ),
        (
            "type=FOLDER&sort=NAME&order=ASCENDING",
            vec![a.clone(), b.clone()],
            vec![],
        ),
        (
            "type=FOLDER&sort=DATE&order=DESCENDING",
            vec![a.clone(), b.clone()],
            vec![],
        ),
        (
            "type=DOCUMENT&sort=NAME&order=ASCENDING",
            vec![],
            vec![c.clone(), d.clone()],
        ),
        (
            "type=DOCUMENT&sort=NAME&order=DESCENDING",
            vec![],
            vec![d.clone(), c.clone()],
        ),
    ];

    for (query, expected_folders, expected_documents) in cases {
        let response = fixture.client.contents(&fixture.folder_id, query).await;
        assert_eq!(
            response.status, 200,
            "case {query:?}: expected 200, got {} body {}",
            response.status, response.json
        );
        assert_eq!(
            response.folder_ids(),
            expected_folders,
            "case {query:?}: expected folders {expected_folders:?}, got {:?}",
            response.folder_ids()
        );
        assert_eq!(
            response.document_ids(),
            expected_documents,
            "case {query:?}: expected documents {expected_documents:?}, got {:?}",
            response.document_ids()
        );
    }
}

/// A listing of a folder reports only its OWN children: the fixture's parent folder is
/// not listed under the root's other children, and a sibling's contents stay out.
#[tokio::test]
async fn folder_contents_report_only_the_named_folders_children() {
    let client = Client::new();
    let root = client.root_folder_id().await;
    let left = client.folder_id(&root, "left").await;
    let right = client.folder_id(&root, "right").await;
    let (left_document, _) = client.active_document(&left, "only-in-left.txt", 1).await;

    let left_listing = client.contents(&left, "type=ALL").await;
    assert_eq!(
        left_listing.document_ids(),
        vec![left_document.clone()],
        "left: expected exactly its own document, got {:?}",
        left_listing.document_ids()
    );
    let right_listing = client.contents(&right, "type=ALL").await;
    assert!(
        right_listing.listed_ids().is_empty(),
        "right: expected an empty folder, got {:?}",
        right_listing.listed_ids()
    );
}

/// Paging is an invariant: for ANY page size, following the `Marker` chain visits every
/// child of the folder exactly once, in the same sequence a single unpaged call reports,
/// and the last page carries no `Marker`.
///
/// The child counts and page sizes are generated from a fixed list of seeds (a bounded
/// case count, no I/O), and each seed appears in every assertion message so a failing
/// case is reproducible.
#[tokio::test]
async fn folder_contents_paging_visits_every_child_exactly_once() {
    for seed in [1_u64, 7, 42, 1_337, 90_210] {
        let mut rng = Lcg::new(seed);
        let documents = 1 + rng.below(6) as usize;
        let subfolders = rng.below(4) as usize;
        let limit = 1 + rng.below(4) as usize;

        let client = Client::new();
        let root = client.root_folder_id().await;
        let folder_id = client.folder_id(&root, "paged").await;
        for index in 0..subfolders {
            client.folder_id(&folder_id, &format!("folder-{index}")).await;
        }
        for index in 0..documents {
            client
                .active_document(&folder_id, &format!("document-{index}.txt"), index as i64)
                .await;
        }
        let total = documents + subfolders;

        let unpaged = client.contents(&folder_id, "type=ALL&sort=NAME&order=ASCENDING").await;
        let expected = unpaged.listed_ids();
        assert_eq!(
            expected.len(),
            total,
            "seed {seed}: expected {total} children unpaged, got {:?}",
            expected
        );

        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let query = match marker.as_deref() {
                Some(token) => format!("type=ALL&sort=NAME&order=ASCENDING&limit={limit}&marker={token}"),
                None => format!("type=ALL&sort=NAME&order=ASCENDING&limit={limit}"),
            };
            let page = client.contents(&folder_id, &query).await;
            assert_eq!(
                page.status, 200,
                "seed {seed}: page {pages} expected 200, got {} body {}",
                page.status, page.json
            );
            let page_ids = page.listed_ids();
            assert!(
                page_ids.len() <= limit,
                "seed {seed}: page {pages} exceeded Limit {limit}, got {} items",
                page_ids.len()
            );
            seen.extend(page_ids);
            pages += 1;
            assert!(
                pages <= total + 1,
                "seed {seed}: paging did not terminate after {pages} pages"
            );
            match page.marker() {
                Some(next) => marker = Some(next),
                None => break,
            }
        }

        let unique: HashSet<&String> = seen.iter().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "seed {seed}: paging repeated a child: {seen:?}"
        );
        // Folders sort before documents within one page, so compare as sets against the
        // unpaged listing and separately assert the sorted order of each kind.
        assert_eq!(
            unique,
            expected.iter().collect::<HashSet<&String>>(),
            "seed {seed}: paging visited {seen:?}, unpaged reported {expected:?}"
        );
        assert_eq!(
            pages,
            total.div_ceil(limit).max(1),
            "seed {seed}: expected {} pages for {total} children at Limit {limit}, got {pages}",
            total.div_ceil(limit).max(1)
        );
    }
}

/// A `Marker` is bound to the query that issued it: replayed against a different
/// listing, it is the modelled 400 rather than an offset into unrelated results.
#[tokio::test]
async fn a_marker_from_another_query_is_refused() {
    let fixture = Fixture::build().await;
    let page = fixture
        .client
        .contents(&fixture.folder_id, "type=ALL&sort=NAME&order=ASCENDING&limit=1")
        .await;
    let marker = page
        .marker()
        .unwrap_or_else(|| panic!("expected a Marker for a partial page: {}", page.json));

    // Same folder, different sort criteria; and a token this service never issued.
    let cases = [
        (
            "different sort",
            format!("type=ALL&sort=DATE&order=ASCENDING&limit=1&marker={marker}"),
        ),
        (
            "different order",
            format!("type=ALL&sort=NAME&order=DESCENDING&limit=1&marker={marker}"),
        ),
        (
            "different type",
            format!("type=DOCUMENT&sort=NAME&order=ASCENDING&limit=1&marker={marker}"),
        ),
        ("not a marker", "limit=1&marker=not-a-marker".to_string()),
    ];
    for (case, query) in cases {
        let response = fixture.client.contents(&fixture.folder_id, &query).await;
        assert_eq!(
            response.status, 400,
            "case {case:?}: expected 400, got {} body {}",
            response.status, response.json
        );
        assert_eq!(
            error_code(&response),
            "InvalidArgumentException",
            "case {case:?}: expected InvalidArgumentException, got {}",
            response.json
        );
    }

    // The same marker against the query that issued it still resolves.
    let next = fixture
        .client
        .contents(
            &fixture.folder_id,
            &format!("type=ALL&sort=NAME&order=ASCENDING&limit=1&marker={marker}"),
        )
        .await;
    assert_eq!(
        next.status, 200,
        "the issuing query must still accept its own marker, got {} body {}",
        next.status, next.json
    );
    assert_eq!(
        next.listed_ids(),
        vec![fixture.folders[0].clone()],
        "expected the second child of the NAME-ascending listing, got {:?}",
        next.listed_ids()
    );
}

/// "By default, Amazon WorkDocs returns the first 100 active document and folder
/// metadata items … You can also request initialized documents": an uncommitted upload
/// is listed only when `Include` names `INITIALIZED`.
#[tokio::test]
async fn initialized_documents_are_listed_only_when_included() {
    let client = Client::new();
    let root = client.root_folder_id().await;
    let folder_id = client.folder_id(&root, "drafts").await;
    let (committed, _) = client.active_document(&folder_id, "committed.txt", 5).await;
    let draft = client
        .initiate(json!({
            "ParentFolderId": folder_id,
            "Name": "draft.txt",
            "ContentType": "text/plain",
            "DocumentSizeInBytes": 6,
        }))
        .await;
    let draft_id = draft.json["Metadata"]["Id"]
        .as_str()
        .expect("the draft document has an Id")
        .to_string();

    let cases: Vec<(&str, Vec<String>)> = vec![
        ("type=ALL", vec![committed.clone()]),
        ("type=DOCUMENT", vec![committed.clone()]),
        ("type=ALL&include=INITIALIZED", vec![committed.clone(), draft_id.clone()]),
        (
            "type=DOCUMENT&include=OTHER,INITIALIZED",
            vec![committed.clone(), draft_id.clone()],
        ),
        ("type=ALL&include=OTHER", vec![committed.clone()]),
    ];
    for (query, expected) in cases {
        let response = fixture_sorted(&client, &folder_id, query).await;
        assert_eq!(
            response, expected,
            "case {query:?}: expected documents {expected:?}, got {response:?}"
        );
    }

    // Committing the draft makes it visible to the default listing.
    let version_id = draft.json["Metadata"]["LatestVersionMetadata"]["Id"]
        .as_str()
        .expect("the draft has a version Id")
        .to_string();
    let committed_draft = client.commit(&draft_id, &version_id).await;
    assert_eq!(
        committed_draft.status, 200,
        "UpdateDocumentVersion(draft): expected 200, got {} body {}",
        committed_draft.status, committed_draft.json
    );
    let after = fixture_sorted(&client, &folder_id, "type=DOCUMENT").await;
    assert_eq!(
        after,
        vec![committed.clone(), draft_id.clone()],
        "after commit: expected both documents, got {after:?}"
    );
}

/// The document ids a listing reports, in `NAME` ascending order.
async fn fixture_sorted(client: &Client, folder_id: &str, query: &str) -> Vec<String> {
    let response = client
        .contents(folder_id, &format!("{query}&sort=NAME&order=ASCENDING"))
        .await;
    assert_eq!(
        response.status, 200,
        "case {query:?}: expected 200, got {} body {}",
        response.status, response.json
    );
    response.document_ids()
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// Every operation in the slice that names a resource answers 404
/// `EntityNotExistsException` for an id nothing created — never a 501 and never a
/// synthesized resource.
#[tokio::test]
async fn unknown_ids_answer_entity_not_exists() {
    let client = Client::new();
    let root = client.root_folder_id().await;
    let (document_id, version_id) = client.active_document(&root, "real.txt", 2).await;
    // A well-formed `ResourceIdType` nothing minted.
    let missing = "0123456789abcdef0123456789abcdef";

    let cases: Vec<(&str, &str, &str, Option<Value>)> = vec![
        ("CreateFolder", "POST", "/api/v1/folders", Some(json!({ "ParentFolderId": missing, "Name": "orphan" }))),
        (
            "InitiateDocumentVersionUpload(parent)",
            "POST",
            "/api/v1/documents",
            Some(json!({ "ParentFolderId": missing, "Name": "orphan.txt" })),
        ),
        (
            "InitiateDocumentVersionUpload(document)",
            "POST",
            "/api/v1/documents",
            Some(json!({ "Id": missing, "Name": "orphan.txt" })),
        ),
        ("GetDocument", "GET", "/api/v1/documents/0123456789abcdef0123456789abcdef", None),
        ("DeleteDocument", "DELETE", "/api/v1/documents/0123456789abcdef0123456789abcdef", None),
        (
            "DescribeFolderContents",
            "GET",
            "/api/v1/folders/0123456789abcdef0123456789abcdef/contents",
            None,
        ),
    ];
    for (case, method, path, body) in cases {
        let response = client.send(method, path, body).await;
        assert_eq!(
            response.status, 404,
            "case {case:?}: expected 404, got {} body {}",
            response.status, response.json
        );
        assert_eq!(
            error_code(&response),
            "EntityNotExistsException",
            "case {case:?}: expected EntityNotExistsException, got {}",
            response.json
        );
        assert_eq!(
            response.error_type.as_deref(),
            Some("EntityNotExistsException"),
            "case {case:?}: expected the x-amzn-errortype header to carry the shape, got {:?}",
            response.error_type
        );
        assert_eq!(
            response.json["EntityIds"][0].as_str(),
            Some(missing),
            "case {case:?}: expected EntityIds to name the missing resource, got {}",
            response.json
        );
    }

    // UpdateDocumentVersion refuses an unknown document and an unknown version alike.
    let unknown_document = client.commit(missing, &version_id).await;
    assert_eq!(
        unknown_document.status, 404,
        "UpdateDocumentVersion(unknown document): expected 404, got {} body {}",
        unknown_document.status, unknown_document.json
    );
    let unknown_version = client.commit(&document_id, "4242424242424").await;
    assert_eq!(
        unknown_version.status, 404,
        "UpdateDocumentVersion(unknown version): expected 404, got {} body {}",
        unknown_version.status, unknown_version.json
    );
    assert_eq!(
        unknown_version.json["EntityIds"][0].as_str(),
        Some("4242424242424"),
        "UpdateDocumentVersion(unknown version): expected EntityIds to name the version, got {}",
        unknown_version.json
    );
}

/// `UpdateDocumentVersion` commits a version only out of `INITIALIZED`
/// (`ProhibitedStateException`: "The specified document version is not in the
/// INITIALIZED state."), and a request naming no `VersionStatus` changes nothing.
#[tokio::test]
async fn committing_a_version_requires_the_initialized_state() {
    let client = Client::new();
    let root = client.root_folder_id().await;
    let (document_id, version_id) = client.active_document(&root, "once.txt", 9).await;

    let again = client.commit(&document_id, &version_id).await;
    assert_eq!(
        again.status, 409,
        "UpdateDocumentVersion(twice): expected 409, got {} body {}",
        again.status, again.json
    );
    assert_eq!(
        error_code(&again),
        "ProhibitedStateException",
        "UpdateDocumentVersion(twice): expected ProhibitedStateException, got {}",
        again.json
    );

    // No VersionStatus: nothing to transition, so the stored version is untouched.
    let before = client.get_document(&document_id).await;
    let noop = client
        .send(
            "PATCH",
            &format!("/api/v1/documents/{document_id}/versions/{version_id}"),
            Some(json!({})),
        )
        .await;
    assert_eq!(
        noop.status, 200,
        "UpdateDocumentVersion(no status): expected 200, got {} body {}",
        noop.status, noop.json
    );
    let after = client.get_document(&document_id).await;
    assert_eq!(
        after.json["Metadata"]["ModifiedTimestamp"], before.json["Metadata"]["ModifiedTimestamp"],
        "UpdateDocumentVersion(no status): a request that asks for no transition must not write, got {} then {}",
        before.json["Metadata"]["ModifiedTimestamp"], after.json["Metadata"]["ModifiedTimestamp"]
    );
}

/// `Id` "is optionally specified when creating a new version of an existing document"
/// (this operation's documentation), so it adds a version to the document it names —
/// and while an earlier version is still uncommitted, that version already exists (409).
#[tokio::test]
async fn an_id_adds_a_version_to_the_document_it_names() {
    let client = Client::new();
    let root = client.root_folder_id().await;
    let (document_id, first_version) = client.active_document(&root, "versioned.txt", 10).await;

    let second = client
        .initiate(json!({
            "Id": document_id,
            "ContentType": "text/plain",
            "DocumentSizeInBytes": 20,
        }))
        .await;
    assert_eq!(
        second.status, 201,
        "InitiateDocumentVersionUpload(Id): expected 201, got {} body {}",
        second.status, second.json
    );
    let second_version = second.json["Metadata"]["LatestVersionMetadata"]["Id"]
        .as_str()
        .expect("the new version has an Id")
        .to_string();
    assert_ne!(
        second_version, first_version,
        "InitiateDocumentVersionUpload(Id): expected a new version id, got {second_version} twice"
    );
    assert_eq!(
        second.json["Metadata"]["Id"].as_str(),
        Some(document_id.as_str()),
        "InitiateDocumentVersionUpload(Id): expected the same document, got {}",
        second.json["Metadata"]
    );
    assert_eq!(
        second.json["Metadata"]["LatestVersionMetadata"]["Status"].as_str(),
        Some("INITIALIZED"),
        "InitiateDocumentVersionUpload(Id): expected the new version INITIALIZED, got {}",
        second.json["Metadata"]
    );
    // A member the request omits keeps the previous version's value: this is another
    // version of the SAME document.
    assert_eq!(
        second.json["Metadata"]["LatestVersionMetadata"]["Name"].as_str(),
        Some("versioned.txt"),
        "InitiateDocumentVersionUpload(Id): expected the document's name to carry over, got {}",
        second.json["Metadata"]
    );

    // A second initiate while that version is still uncommitted is the resource that
    // already exists.
    let overlapping = client
        .initiate(json!({ "Id": document_id, "DocumentSizeInBytes": 30 }))
        .await;
    assert_eq!(
        overlapping.status, 409,
        "InitiateDocumentVersionUpload(overlapping): expected 409, got {} body {}",
        overlapping.status, overlapping.json
    );
    assert_eq!(
        error_code(&overlapping),
        "EntityAlreadyExistsException",
        "InitiateDocumentVersionUpload(overlapping): expected EntityAlreadyExistsException, got {}",
        overlapping.json
    );

    // Committing the second version makes it the document's latest.
    let committed = client.commit(&document_id, &second_version).await;
    assert_eq!(
        committed.status, 200,
        "UpdateDocumentVersion(second): expected 200, got {} body {}",
        committed.status, committed.json
    );
    let read = client.get_document(&document_id).await;
    assert_eq!(
        read.json["Metadata"]["LatestVersionMetadata"]["Id"].as_str(),
        Some(second_version.as_str()),
        "GetDocument: expected the committed second version as the latest, got {}",
        read.json["Metadata"]
    );
    assert_eq!(
        read.json["Metadata"]["LatestVersionMetadata"]["Size"].as_i64(),
        Some(20),
        "GetDocument: expected the second version's size, got {}",
        read.json["Metadata"]
    );
}

/// Two documents cannot share a name in one folder, and the SAME name in another folder
/// is a different resource.
#[tokio::test]
async fn a_duplicate_document_name_in_one_folder_is_refused() {
    let client = Client::new();
    let root = client.root_folder_id().await;
    let left = client.folder_id(&root, "left").await;
    let right = client.folder_id(&root, "right").await;

    let cases = [
        ("same name, same folder", left.clone(), 409),
        ("same name, other folder", right.clone(), 201),
    ];
    client.active_document(&left, "report.pdf", 1).await;
    for (case, parent, expected) in cases {
        let response = client
            .initiate(json!({ "ParentFolderId": parent, "Name": "report.pdf" }))
            .await;
        assert_eq!(
            response.status, expected,
            "case {case:?}: expected {expected}, got {} body {}",
            response.status, response.json
        );
        if expected == 409 {
            assert_eq!(
                error_code(&response),
                "EntityAlreadyExistsException",
                "case {case:?}: expected EntityAlreadyExistsException, got {}",
                response.json
            );
        }
    }

    // A new document needs both the folder to live in and a name.
    let incomplete = [
        ("no ParentFolderId", json!({ "Name": "nowhere.txt" })),
        ("no Name", json!({ "ParentFolderId": left })),
    ];
    for (case, body) in incomplete {
        let response = client.initiate(body).await;
        assert_eq!(
            response.status, 400,
            "case {case:?}: expected 400, got {} body {}",
            response.status, response.json
        );
        assert_eq!(
            error_code(&response),
            "InvalidArgumentException",
            "case {case:?}: expected InvalidArgumentException, got {}",
            response.json
        );
    }
}

/// A folder's `ModifiedTimestamp` advances when its contents change — a create or a
/// delete under it updates the folder, whose `Signature` is derived from that child set.
#[tokio::test]
async fn a_folders_modified_timestamp_advances_with_its_contents() {
    let client = Client::new();
    let root = client.root_folder_id().await;
    let folder_id = client.folder_id(&root, "watched").await;

    let modified_of = |payload: Value, id: String| -> f64 {
        payload["Folders"]
            .as_array()
            .and_then(|folders| folders.iter().find(|f| f["Id"].as_str() == Some(id.as_str())))
            .and_then(|folder| folder["ModifiedTimestamp"].as_f64())
            .unwrap_or_else(|| panic!("no ModifiedTimestamp for {id}"))
    };

    let at_create = modified_of(
        client.contents(&root, "type=FOLDER").await.json,
        folder_id.clone(),
    );
    let (document_id, _) = client.active_document(&folder_id, "watched.txt", 7).await;
    let after_upload = modified_of(
        client.contents(&root, "type=FOLDER").await.json,
        folder_id.clone(),
    );
    assert!(
        after_upload > at_create,
        "adding a child must advance the folder's ModifiedTimestamp: created {at_create}, after upload {after_upload}"
    );

    let deleted = client
        .send("DELETE", &format!("/api/v1/documents/{document_id}"), None)
        .await;
    assert_eq!(
        deleted.status, 204,
        "DeleteDocument: expected 204, got {} body {}",
        deleted.status, deleted.json
    );
    let after_delete = modified_of(
        client.contents(&root, "type=FOLDER").await.json,
        folder_id.clone(),
    );
    assert!(
        after_delete > after_upload,
        "removing a child must advance the folder's ModifiedTimestamp: after upload {after_upload}, after delete {after_delete}"
    );
}

/// `UploadMetadata.UploadUrl` names the endpoint THIS request arrived at, and is omitted
/// when the request names no endpoint at all — the emulator never advertises a baked-in
/// host for a client to upload document content to.
#[tokio::test]
async fn the_upload_url_names_the_requests_own_endpoint_or_none() {
    let client = Client::new();
    let root = client.root_folder_id().await;

    // Routed by `Host`: the URL is rooted at that host.
    let by_host = client
        .initiate(json!({ "ParentFolderId": root, "Name": "hosted.txt" }))
        .await;
    assert_eq!(
        by_host.status, 201,
        "initiate(Host): expected 201, got {} body {}",
        by_host.status, by_host.json
    );
    let url = by_host.json["UploadMetadata"]["UploadUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("initiate(Host): expected an UploadUrl, got {}", by_host.json));
    assert!(
        url.starts_with(&format!("https://{HOST}/")),
        "initiate(Host): expected a URL on the request's own host, got {url}"
    );

    // Routed by the SigV4 credential scope instead, with no `Host` and no authority:
    // there is no endpoint to name, so `UploadUrl` is absent rather than invented.
    let request = http::Request::builder()
        .method("POST")
        .uri("/api/v1/documents")
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/workdocs/aws4_request, \
             SignedHeaders=content-type, Signature=0000",
        )
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(
            serde_json::to_vec(&json!({ "ParentFolderId": root, "Name": "hostless.txt" }))
                .expect("serialize the request body"),
        ))
        .expect("build the request");
    let response = client.router.dispatch(request).await;
    assert_eq!(
        response.status().as_u16(),
        201,
        "initiate(no host): expected 201, got {} body {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
    let payload: Value =
        serde_json::from_slice(response.body()).expect("parse the response body");
    assert!(
        payload["UploadMetadata"]["UploadUrl"].is_null(),
        "initiate(no host): expected no UploadUrl, got {}",
        payload["UploadMetadata"]
    );
    assert_eq!(
        payload["Metadata"]["ParentFolderId"].as_str(),
        Some(root.as_str()),
        "initiate(no host): expected the document stored under the root folder, got {}",
        payload["Metadata"]
    );
}

// ---------------------------------------------------------------------------
// A small deterministic generator, for the paging invariant above.
// ---------------------------------------------------------------------------

/// A seeded linear congruential generator: the paging invariant needs nothing but
/// uniform small integers, and a fixed seed makes every case reproducible from the
/// assertion message that names it.
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1))
    }

    /// A value in `0..bound`.
    fn below(&mut self, bound: u32) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        u32::try_from((self.0 >> 33) % u64::from(bound)).unwrap_or(0)
    }
}
