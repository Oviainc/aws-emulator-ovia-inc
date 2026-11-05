// SPDX-License-Identifier: CC0-1.0

//! The application lifecycle of the Serverless Application Repository, end to end over
//! HTTP: CreateApplication, GetApplication, CreateApplicationVersion,
//! ListApplicationVersions, ListApplications and DeleteApplication.
//!
//! Every case drives the real wire path — the router resolves this service from the
//! request's SigV4 credential scope, the restJson1 protocol routes on method + URI and
//! deserializes the body, and the handler answers out of the one shared engine — so
//! what is asserted here is what a `serverlessrepo` SDK client sees.
//!
//! The rules under test, each exercised over a SET of inputs rather than one recorded
//! request:
//!
//! 1. a create persists the record and every later read answers out of it;
//! 2. an application name is unique within the catalogue;
//! 3. a version is keyed by its semantic version, and re-creating one conflicts;
//! 4. a paged walk of a collection visits every stored item exactly once and the final
//!    page carries no token;
//! 5. a request outside a declared or documented constraint is refused with the error
//!    the model declares for an invalid parameter;
//! 6. a delete removes the application and its versions, and is observable as absence.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_serverlessapplicationrepository_serverlessapplicationrepository::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// One emulator: a single shared engine with this service mounted on it, driven through
/// the same `dispatch` the production front door uses.
///
/// `pub(crate)`, together with [`Answer`] and the helpers below, because the other
/// suites in this target drive the service through this same harness rather than each
/// building their own.
pub(crate) struct Repo {
    router: MultiServiceRouter,
    /// The one shared engine this service is mounted on, so a suite that asserts a
    /// transition the CLOCK owes can hold the store's due instant instead of sleeping for
    /// it. No test reaches it to write what a handler should have written.
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

/// One HTTP answer: the status, the raw body (so an empty 204 body can be asserted), the
/// length the answer STATES for that body, and the body parsed as JSON.
pub(crate) struct Answer {
    pub(crate) status: u16,
    raw: Vec<u8>,
    /// The `content-length` the answer states, absent when it states none — the framing of
    /// an answer with no body is a rule of its own (`tests/all/empty_response_framing.rs`).
    pub(crate) content_length: Option<String>,
    pub(crate) body: Value,
}

impl Answer {
    /// A member of the response object, or `Value::Null` when it is absent.
    pub(crate) fn at(&self, member: &str) -> &Value {
        self.body.get(member).unwrap_or(&Value::Null)
    }

    /// The wire error code restJson1 carries as the body's `__type`.
    pub(crate) fn error_code(&self) -> &str {
        self.at("__type").as_str().unwrap_or("")
    }
}

/// One answer from the artifact document plane: a download, not a modelled call, so what
/// it carries is a status, a media type, a length and BYTES rather than a JSON envelope.
pub(crate) struct Fetched {
    pub(crate) status: u16,
    pub(crate) content_type: String,
    pub(crate) content_length: Option<usize>,
    pub(crate) body: String,
}

/// An absolute URL split into its authority and its path (with any query kept on the
/// path), as a client sends them: the authority as `Host`, the path as the request target.
fn split_url(url: &str) -> (&str, &str) {
    let after_scheme = url.split_once("://").map_or(url, |(_scheme, rest)| rest);
    match after_scheme.find('/') {
        Some(path_start) => after_scheme.split_at(path_start),
        None => (after_scheme, "/"),
    }
}

impl Repo {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is not under test here; the handlers are.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    pub(crate) async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Answer {
        self.call_from_host(method, path, body, None).await
    }

    /// [`Repo::call`], from a client that reached the emulator at `host`.
    ///
    /// The host matters to one thing only, and it is a published one: an address this
    /// service announces for an artifact a publisher supplied INLINE has to be an address
    /// the caller can follow back here, so it is built from the endpoint the publishing
    /// request arrived at. `None` sends no `Host` header at all, which is what every
    /// other suite's calls do.
    pub(crate) async fn call_from_host(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        host: Option<&str>,
    ) -> Answer {
        let payload = match body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize")),
            None => bytes::Bytes::new(),
        };
        let mut request = http::Request::builder()
            .method(method)
            .uri(path)
            // The credential scope's service field is what the router routes on, and its
            // region field is the region every minted ARN carries.
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/us-west-2/serverlessrepo/\
                 aws4_request, SignedHeaders=host, Signature=deadbeef",
            )
            .header("content-type", "application/json");
        if let Some(host) = host {
            request = request.header("host", host);
        }
        let request = request.body(payload).expect("a valid request");
        self.answer(request).await
    }

    /// Follow a published artifact URL the way a client that was handed one does: a plain
    /// `GET` (or `HEAD`) of the whole URL, carrying NO credential scope of its own — a
    /// published object URL is followed unsigned — with the URL's own host as `Host`.
    ///
    /// `signed` adds the same SigV4 credential scope a client's other calls carry, for
    /// the caller that re-uses its routing headers on the fetch: the artifact is served
    /// either way, because the announced address is served by this service whether the
    /// request routes to it by host or by path.
    pub(crate) async fn follow(&self, method: &str, url: &str, signed: bool) -> Fetched {
        let (host, path) = split_url(url);
        let mut request = http::Request::builder().method(method).uri(path);
        if !host.is_empty() {
            request = request.header("host", host);
        }
        if signed {
            request = request.header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/us-west-2/serverlessrepo/\
                 aws4_request, SignedHeaders=host, Signature=deadbeef",
            );
        }
        let request = request
            .body(bytes::Bytes::new())
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        Fetched {
            status: response.status().as_u16(),
            content_type: response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string(),
            content_length: response
                .headers()
                .get("content-length")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<usize>().ok()),
            body: String::from_utf8_lossy(response.body()).to_string(),
        }
    }

    /// One dispatched request, as the status and the JSON body every modelled call of
    /// this service answers with.
    async fn answer(&self, request: http::Request<bytes::Bytes>) -> Answer {
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let content_length = response
            .headers()
            .get("content-length")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let raw = response.body().to_vec();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        Answer {
            status,
            raw,
            content_length,
            body,
        }
    }

    pub(crate) async fn create_application(&self, body: Value) -> Answer {
        self.call("POST", "/applications", Some(body)).await
    }

    pub(crate) async fn get_application(
        &self,
        application_id: &str,
        semantic_version: Option<&str>,
    ) -> Answer {
        let path = match semantic_version {
            Some(version) => format!(
                "/applications/{}?semanticVersion={version}",
                path_label(application_id)
            ),
            None => format!("/applications/{}", path_label(application_id)),
        };
        self.call("GET", &path, None).await
    }

    pub(crate) async fn create_version(
        &self,
        application_id: &str,
        semantic_version: &str,
        body: Value,
    ) -> Answer {
        let path = format!(
            "/applications/{}/versions/{semantic_version}",
            path_label(application_id)
        );
        self.call("PUT", &path, Some(body)).await
    }

    pub(crate) async fn list_applications(&self, query: &str) -> Answer {
        self.call("GET", &format!("/applications{query}"), None).await
    }

    pub(crate) async fn list_versions(&self, application_id: &str, query: &str) -> Answer {
        let path = format!(
            "/applications/{}/versions{query}",
            path_label(application_id)
        );
        self.call("GET", &path, None).await
    }

    pub(crate) async fn delete_application(&self, application_id: &str) -> Answer {
        let path = format!("/applications/{}", path_label(application_id));
        self.call("DELETE", &path, None).await
    }
}

/// An `ApplicationId` is an ARN, and the model binds it to a single, non-greedy URI
/// label — so a client percent-encodes its `:` and `/` to keep it one path segment,
/// exactly as an AWS SDK does. The same encoding serves any other single-segment label a
/// suite sends (a `SemanticVersion` that carries a space, say).
pub(crate) fn path_label(application_id: &str) -> String {
    application_id
        .chars()
        .map(|c| match c {
            ':' => "%3A".to_string(),
            '/' => "%2F".to_string(),
            ' ' => "%20".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// The smallest body CreateApplication accepts: the three members the model requires.
pub(crate) fn minimal(name: &str) -> Value {
    json!({ "name": name, "author": "example-author", "description": "a demo app" })
}

/// Create `name` and return its minted `ApplicationId`.
pub(crate) async fn created(repo: &Repo, name: &str) -> String {
    let answer = repo.create_application(minimal(name)).await;
    assert_eq!(
        answer.status, 201,
        "case {name:?}: expected 201, got {} with {}",
        answer.status, answer.body
    );
    answer
        .at("applicationId")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Every page of a paged walk, collected: the items seen in order, and the number of
/// pages it took. Fails the walk if it does not terminate.
async fn walk(
    repo: &Repo,
    page: impl Fn(&str) -> String,
    items_member: &str,
    id_member: &str,
    page_size: usize,
) -> (Vec<String>, usize) {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let query = match &token {
            Some(token) => format!("?maxItems={page_size}&nextToken={token}"),
            None => format!("?maxItems={page_size}"),
        };
        let answer = repo.call("GET", &page(&query), None).await;
        assert_eq!(
            answer.status, 200,
            "page {pages} of a walk by {page_size}: expected 200, got {} with {}",
            answer.status, answer.body
        );
        let items = answer
            .at(items_member)
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            items.len() <= page_size,
            "page {pages} of a walk by {page_size}: expected at most {page_size} items, got {}",
            items.len()
        );
        for item in &items {
            seen.push(
                item.get(id_member)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        pages += 1;
        match answer.at("nextToken").as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
        assert!(
            pages < 100,
            "a walk by {page_size} did not terminate after {pages} pages"
        );
    }
    (seen, pages)
}

/// RULE 1: a create persists the whole record, and the response is read back out of the
/// store — so GetApplication and ListApplications answer with exactly what was created.
///
/// The rows differ along what the rule generalises over: which optional members are
/// present, how many labels there are, and whether the call also creates the first
/// version.
#[tokio::test]
async fn a_created_application_is_read_back_member_for_member() {
    let repo = Repo::new();
    let cases: Vec<(&str, Value)> = vec![
        ("only the required members", minimal("minimal-app")),
        (
            "every optional member, inline bodies",
            json!({
                "name": "fully-specified-app",
                "author": "another-author",
                "description": "an app with every optional member set",
                "homePageUrl": "https://example.com/home",
                "labels": ["demo", "serverless", "a+b_c:d/e@f"],
                "licenseBody": "ALL RIGHTS RESERVED",
                "readmeBody": "# readme",
                "spdxLicenseId": "Apache-2.0",
                "sourceCodeUrl": "https://example.com/commit/abc",
                "sourceCodeArchiveUrl": "https://example.com/src.zip",
                "semanticVersion": "1.0.0",
                "templateBody": "{\"Transform\":\"AWS::Serverless-2016-10-31\",\"Resources\":{}}"
            }),
        ),
        (
            "urls instead of inline bodies, no version",
            json!({
                "name": "url-app",
                "author": "third-author",
                "description": "an app whose artefacts are links",
                "licenseUrl": "https://example.com/LICENSE",
                "readmeUrl": "https://example.com/README.md",
                "labels": ["one"]
            }),
        ),
        (
            "ten labels, the documented maximum",
            json!({
                "name": "ten-label-app",
                "author": "fourth-author",
                "description": "an app carrying the maximum number of labels",
                "labels": ["l0", "l1", "l2", "l3", "l4", "l5", "l6", "l7", "l8", "l9"]
            }),
        ),
    ];

    for (case, request) in &cases {
        let created = repo.create_application(request.clone()).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: expected 201, got {} with {}",
            created.status, created.body
        );

        let name = request["name"].as_str().expect("a name");
        let application_id = created.at("applicationId").as_str().unwrap_or_default();
        assert_eq!(
            application_id,
            format!("arn:aws:serverlessrepo:us-west-2:000000000000:applications/{name}"),
            "case {case:?}: the minted ApplicationId is an ARN over the request's region, \
             the emulator account and the application name, got {application_id:?}"
        );
        assert!(
            !created
                .at("creationTime")
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "case {case:?}: expected a CreationTime, got {}",
            created.at("creationTime")
        );
        assert_eq!(
            created.at("isVerifiedAuthor"),
            &json!(false),
            "case {case:?}: a freshly published application has no verified author, got {}",
            created.at("isVerifiedAuthor")
        );

        // The echoed members are the stored ones.
        for member in ["name", "author", "description", "homePageUrl", "spdxLicenseId"] {
            assert_eq!(
                created.at(member),
                request.get(member).unwrap_or(&Value::Null),
                "case {case:?}: member {member} must come back as stored, got {}",
                created.at(member)
            );
        }
        // Labels come back as stored; a create that named none reports the member
        // present and empty, which is what the service answers with.
        assert_eq!(
            created.at("labels"),
            &request
                .get("labels")
                .cloned()
                .unwrap_or_else(|| json!([])),
            "case {case:?}: Labels must come back as stored, got {}",
            created.at("labels")
        );
        // A URL the caller gave is reported verbatim; a body they gave inline is
        // reported as the locator the service minted for it.
        for (body_member, url_member) in [("licenseBody", "licenseUrl"), ("readmeBody", "readmeUrl")]
        {
            match request.get(url_member) {
                Some(url) => assert_eq!(
                    created.at(url_member),
                    url,
                    "case {case:?}: {url_member} must be the caller's own link"
                ),
                None if request.get(body_member).is_some() => assert!(
                    created
                        .at(url_member)
                        .as_str()
                        .unwrap_or_default()
                        .contains(name),
                    "case {case:?}: {url_member} must locate the stored {body_member}, got {}",
                    created.at(url_member)
                ),
                None => assert_eq!(
                    created.at(url_member),
                    &Value::Null,
                    "case {case:?}: {url_member} is absent when neither was supplied"
                ),
            }
        }

        // A SemanticVersion on the create makes the same call create the first version.
        match request.get("semanticVersion") {
            Some(semantic_version) => {
                let version = created.at("version");
                assert_eq!(
                    version.get("semanticVersion").unwrap_or(&Value::Null),
                    semantic_version,
                    "case {case:?}: the nested Version is the one the create named, got {version}"
                );
                assert_eq!(
                    version.get("applicationId").and_then(|v| v.as_str()),
                    Some(application_id),
                    "case {case:?}: the nested Version names its application, got {version}"
                );
                assert!(
                    !version
                        .get("templateUrl")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .is_empty(),
                    "case {case:?}: Version$TemplateUrl is @required, got {version}"
                );
            }
            None => assert_eq!(
                created.at("version"),
                &Value::Null,
                "case {case:?}: no SemanticVersion means no version was created, got {}",
                created.at("version")
            ),
        }

        // The same record, read back through GetApplication.
        let fetched = repo.get_application(application_id, None).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected 200 from GetApplication, got {} with {}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.body, created.body,
            "case {case:?}: GetApplication must answer with the stored record, got {}",
            fetched.body
        );
    }

    // And every one of them is in ListApplications, exactly once.
    let listed = repo.list_applications("").await;
    assert_eq!(listed.status, 200, "ListApplications: got {}", listed.body);
    let names: Vec<String> = listed
        .at("applications")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|app| app["name"].as_str().unwrap_or_default().to_string())
        .collect();
    for (case, request) in &cases {
        let name = request["name"].as_str().expect("a name");
        assert_eq!(
            names.iter().filter(|listed| *listed == name).count(),
            1,
            "case {case:?}: {name} must appear in ListApplications exactly once, got {names:?}"
        );
    }
}

/// RULE 1, continued: an `ApplicationSummary` carries the stored scalars, not a
/// placeholder.
#[tokio::test]
async fn a_listed_summary_carries_the_stored_members() {
    let repo = Repo::new();
    let request = json!({
        "name": "summary-app",
        "author": "summary-author",
        "description": "the app whose summary is under test",
        "homePageUrl": "https://example.com/summary",
        "labels": ["x", "y"],
        "spdxLicenseId": "MIT"
    });
    let created = repo.create_application(request.clone()).await;
    assert_eq!(created.status, 201, "create: got {}", created.body);

    let listed = repo.list_applications("").await;
    let summaries = listed
        .at("applications")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        summaries.len(),
        1,
        "one application was created, got {summaries:?}"
    );
    let summary = &summaries[0];
    for member in [
        "name",
        "author",
        "description",
        "homePageUrl",
        "spdxLicenseId",
        "labels",
    ] {
        assert_eq!(
            summary.get(member).unwrap_or(&Value::Null),
            request.get(member).unwrap_or(&Value::Null),
            "member {member} of the summary must be the stored value, got {summary}"
        );
    }
    assert_eq!(
        summary.get("applicationId").and_then(|v| v.as_str()),
        created.at("applicationId").as_str(),
        "the summary names the same application, got {summary}"
    );
    assert_eq!(
        summary.get("creationTime").and_then(|v| v.as_str()),
        created.at("creationTime").as_str(),
        "the summary carries the stored CreationTime, got {summary}"
    );
}

/// RULE 2: a name identifies one application in the catalogue, so a second create with
/// a name already taken conflicts — whatever else the request says.
#[tokio::test]
async fn a_name_already_in_the_catalogue_conflicts() {
    let repo = Repo::new();
    let cases = [
        ("same request twice", minimal("conflict-app")),
        (
            "same name, different author and description",
            json!({
                "name": "conflict-app",
                "author": "someone-else",
                "description": "a different description entirely"
            }),
        ),
        (
            "same name, with a version",
            json!({
                "name": "conflict-app",
                "author": "third-party",
                "description": "with a first version",
                "semanticVersion": "9.9.9"
            }),
        ),
    ];

    let first = repo.create_application(minimal("conflict-app")).await;
    assert_eq!(first.status, 201, "the first create: got {}", first.body);

    for (case, request) in &cases {
        let again = repo.create_application(request.clone()).await;
        assert_eq!(
            again.status, 409,
            "case {case:?}: expected 409, got {} with {}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {}",
            again.body
        );
    }

    // The conflict changed nothing: the catalogue still holds exactly one application.
    let listed = repo.list_applications("").await;
    assert_eq!(
        listed.at("applications").as_array().map(Vec::len),
        Some(1),
        "a refused create must not have written anything, got {}",
        listed.body
    );
}

/// RULE 3: a version is keyed by the `SemanticVersion` path label, every stored version
/// is listed, and re-creating one that exists conflicts.
#[tokio::test]
async fn versions_are_keyed_by_semantic_version() {
    let repo = Repo::new();
    let application_id = created(&repo, "versioned-app").await;

    // Rows differing in form: a plain release, a patch, a major bump, a prerelease and
    // a build-metadata version — every one a label the path carries verbatim.
    let versions = ["1.0.0", "1.0.1", "1.10.0", "2.0.0-beta.1", "2.0.0"];
    for semantic_version in versions {
        let created = repo
            .create_version(
                &application_id,
                semantic_version,
                json!({
                    "templateUrl": format!("https://example.com/{semantic_version}/template.yaml"),
                    "sourceCodeUrl": format!("https://example.com/commit/{semantic_version}")
                }),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "case {semantic_version:?}: expected 201, got {} with {}",
            created.status, created.body
        );
        assert_eq!(
            created.at("semanticVersion").as_str(),
            Some(semantic_version),
            "case {semantic_version:?}: the stored version is the one the path named, got {}",
            created.body
        );
        assert_eq!(
            created.at("applicationId").as_str(),
            Some(application_id.as_str()),
            "case {semantic_version:?}: the version names its application, got {}",
            created.body
        );
        assert_eq!(
            created.at("templateUrl").as_str(),
            Some(format!("https://example.com/{semantic_version}/template.yaml").as_str()),
            "case {semantic_version:?}: TemplateUrl is the caller's own link, got {}",
            created.body
        );
        assert_eq!(
            created.at("resourcesSupported"),
            &json!(true),
            "case {semantic_version:?}: ResourcesSupported is @required, got {}",
            created.body
        );
        assert!(
            created.at("parameterDefinitions").is_array(),
            "case {semantic_version:?}: ParameterDefinitions is @required, got {}",
            created.body
        );

        // The same version again is the resource already existing.
        let again = repo
            .create_version(&application_id, semantic_version, json!({}))
            .await;
        assert_eq!(
            again.status, 409,
            "case {semantic_version:?}: re-creating a version expected 409, got {} with {}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            "ConflictException",
            "case {semantic_version:?}: expected ConflictException, got {}",
            again.body
        );
    }

    // Every stored version is listed, once, and nothing else is.
    let listed = repo.list_versions(&application_id, "").await;
    assert_eq!(listed.status, 200, "ListApplicationVersions: {}", listed.body);
    let listed_versions: BTreeSet<String> = listed
        .at("versions")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|v| v["semanticVersion"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        listed_versions,
        versions.iter().map(|v| (*v).to_string()).collect(),
        "every stored version, and only those, must be listed; got {}",
        listed.body
    );
    for summary in listed.at("versions").as_array().cloned().unwrap_or_default() {
        assert_eq!(
            summary["applicationId"].as_str(),
            Some(application_id.as_str()),
            "a VersionSummary names its application, got {summary}"
        );
        assert_eq!(
            summary["sourceCodeUrl"].as_str(),
            Some(
                format!(
                    "https://example.com/commit/{}",
                    summary["semanticVersion"].as_str().unwrap_or_default()
                )
                .as_str()
            ),
            "a VersionSummary carries the stored SourceCodeUrl, got {summary}"
        );
        assert!(
            !summary["creationTime"]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "a VersionSummary carries a CreationTime, got {summary}"
        );
    }

    // GetApplication selects the version the request names, and 404s on one that was
    // never created.
    for semantic_version in versions {
        let fetched = repo
            .get_application(&application_id, Some(semantic_version))
            .await;
        assert_eq!(
            fetched.status, 200,
            "case {semantic_version:?}: expected 200, got {} with {}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.at("version").get("semanticVersion").and_then(|v| v.as_str()),
            Some(semantic_version),
            "case {semantic_version:?}: the returned Version must be that stored version, got {}",
            fetched.body
        );
    }
    for unknown in ["0.0.1", "3.0.0", "not-a-version"] {
        let fetched = repo.get_application(&application_id, Some(unknown)).await;
        assert_eq!(
            fetched.status, 404,
            "case {unknown:?}: a version that was never created expected 404, got {} with {}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.error_code(),
            "NotFoundException",
            "case {unknown:?}: expected NotFoundException, got {}",
            fetched.body
        );
    }

    // With no version named, the application reports the one created most recently.
    let latest = repo.get_application(&application_id, None).await;
    assert_eq!(
        latest
            .at("version")
            .get("semanticVersion")
            .and_then(|v| v.as_str()),
        Some("2.0.0"),
        "with no SemanticVersion the latest created version is reported, got {}",
        latest.body
    );
}

/// RULE 4: a paged walk covers the collection exactly once, at every page size, and the
/// final page omits the token. Asserted over both paginated operations and over page
/// sizes that do and do not divide the collection.
#[tokio::test]
async fn a_paged_walk_visits_every_item_exactly_once() {
    let repo = Repo::new();
    let names: Vec<String> = (0..7).map(|i| format!("paged-app-{i}")).collect();
    let mut ids = Vec::new();
    for name in &names {
        ids.push(created(&repo, name).await);
    }
    let versions: Vec<String> = (0..5).map(|i| format!("1.0.{i}")).collect();
    for semantic_version in &versions {
        let created = repo
            .create_version(&ids[0], semantic_version, json!({}))
            .await;
        assert_eq!(
            created.status, 201,
            "seeding version {semantic_version}: got {}",
            created.body
        );
    }

    for page_size in [1usize, 2, 3, 7, 100] {
        let (seen, pages) = walk(
            &repo,
            |query| format!("/applications{query}"),
            "applications",
            "applicationId",
            page_size,
        )
        .await;
        let unique: BTreeSet<&String> = seen.iter().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "ListApplications by {page_size}: an item was visited twice, got {seen:?}"
        );
        assert_eq!(
            unique,
            ids.iter().collect(),
            "ListApplications by {page_size}: the walk must cover the catalogue, got {seen:?}"
        );
        let expected_pages = ids.len().div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "ListApplications by {page_size}: expected {expected_pages} pages, got {pages}"
        );

        let (seen, pages) = walk(
            &repo,
            |query| format!("/applications/{}/versions{query}", path_label(&ids[0])),
            "versions",
            "semanticVersion",
            page_size,
        )
        .await;
        let unique: BTreeSet<&String> = seen.iter().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "ListApplicationVersions by {page_size}: a version was visited twice, got {seen:?}"
        );
        assert_eq!(
            unique,
            versions.iter().collect(),
            "ListApplicationVersions by {page_size}: the walk must cover every stored \
             version, got {seen:?}"
        );
        let expected_pages = versions.len().div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "ListApplicationVersions by {page_size}: expected {expected_pages} pages, got {pages}"
        );
    }
}

/// RULE 5: `MaxItems` is declared `range 1..100`, so each published bound is served and
/// one past it is refused — on both paginated operations.
#[tokio::test]
async fn max_items_is_served_inside_its_published_range_and_refused_outside() {
    let repo = Repo::new();
    let application_id = created(&repo, "ranged-app").await;

    // The bounds come from the `MaxItems` shape's `@range`
    // (`smithy_query(query="shape", shape="MaxItems")` -> range 1..100).
    let cases: [(&str, i64, u16); 5] = [
        ("one below the minimum", 0, 400),
        ("the published minimum", 1, 200),
        ("the published maximum", 100, 200),
        ("one above the maximum", 101, 400),
        ("negative", -1, 400),
    ];
    for (case, max_items, expected) in cases {
        for (operation, answer) in [
            (
                "ListApplications",
                repo.list_applications(&format!("?maxItems={max_items}")).await,
            ),
            (
                "ListApplicationVersions",
                repo.list_versions(&application_id, &format!("?maxItems={max_items}"))
                    .await,
            ),
        ] {
            assert_eq!(
                answer.status, expected,
                "case {case:?} ({operation}, maxItems={max_items}): expected {expected}, got {} \
                 with {}",
                answer.status, answer.body
            );
            if expected == 400 {
                assert_eq!(
                    answer.error_code(),
                    "BadRequestException",
                    "case {case:?} ({operation}): expected BadRequestException, got {}",
                    answer.body
                );
            }
        }
    }
}

/// RULE 5: a `nextToken` is this service's own or it is nothing — a token it did not
/// mint, and one naming an item the collection no longer holds, are both refused.
#[tokio::test]
async fn a_token_this_service_did_not_mint_is_refused() {
    let repo = Repo::new();
    let first = created(&repo, "token-app-a").await;
    let second = created(&repo, "token-app-b").await;

    // The token that walks past the first application, taken from a real page.
    let page = repo.list_applications("?maxItems=1").await;
    let good = page
        .at("nextToken")
        .as_str()
        .expect("a first page of two leaves a token")
        .to_string();
    let resumed = repo
        .list_applications(&format!("?maxItems=1&nextToken={good}"))
        .await;
    assert_eq!(
        resumed.at("applications")[0]["applicationId"].as_str(),
        Some(second.as_str()),
        "the service's own token resumes the walk, got {}",
        resumed.body
    );

    let cases = [
        ("not hex at all", "not-a-token".to_string(), 400),
        ("odd-length hex", "abc".to_string(), 400),
        ("hex of bytes that are not UTF-8", "ff".to_string(), 400),
        ("hex of a key no collection holds", hex_of("no-such-key"), 400),
        ("empty", String::new(), 400),
        // The one token in this table the service did mint: the key of an application
        // the catalogue still holds.
        ("hex of a stored item's key", hex_of(&first), 200),
    ];
    for (case, token, expected) in &cases {
        let answer = repo
            .list_applications(&format!("?maxItems=1&nextToken={token}"))
            .await;
        assert_eq!(
            answer.status, *expected,
            "case {case:?}: expected {expected}, got {} with {}",
            answer.status, answer.body
        );
        if *expected == 400 {
            assert_eq!(
                answer.error_code(),
                "BadRequestException",
                "case {case:?}: expected BadRequestException, got {}",
                answer.body
            );
        }
    }

    // Once the item a token named is gone, the token is no longer usable either.
    let deleted = repo.delete_application(&first).await;
    assert_eq!(deleted.status, 204, "delete: got {}", deleted.body);
    let stale = repo
        .list_applications(&format!("?maxItems=1&nextToken={}", hex_of(&first)))
        .await;
    assert_eq!(
        stale.status, 400,
        "a token naming a deleted item is refused, got {} with {}",
        stale.status, stale.body
    );
    assert_eq!(
        stale.error_code(),
        "BadRequestException",
        "expected BadRequestException, got {}",
        stale.body
    );
}

/// The token encoding the service uses, written out here so a test can build one
/// deliberately.
fn hex_of(key: &str) -> String {
    key.bytes().map(|byte| format!("{byte:02x}")).collect()
}

/// RULE 5: the members the model requires, and the bounds the member documentation
/// states that the service applies, are both refused as the one error this service
/// declares for an invalid parameter.
///
/// A character pattern that appears only in a member's PROSE is not one of those bounds
/// for free-text metadata — see `application_metadata.rs`, which pins what `Author`
/// accepts.
#[tokio::test]
async fn a_request_outside_the_contract_is_a_bad_request() {
    let repo = Repo::new();
    // Each row cites where its rule comes from: `@required` in the model, or the
    // member's own documentation
    // (`smithy_query(query="doc", shape="CreateApplicationRequest", member=...)`).
    let cases: Vec<(&str, Value)> = vec![
        (
            "Name absent (@required)",
            json!({ "author": "an-author", "description": "no name" }),
        ),
        (
            "Author absent (@required)",
            json!({ "name": "no-author", "description": "no author" }),
        ),
        (
            "Description absent (@required)",
            json!({ "name": "no-description", "author": "an-author" }),
        ),
        (
            "Name empty (minimum length=1)",
            json!({ "name": "", "author": "an-author", "description": "empty name" }),
        ),
        (
            "Name too long (maximum length=140)",
            json!({ "name": "a".repeat(141), "author": "an-author", "description": "long name" }),
        ),
        (
            "Name outside [a-zA-Z0-9\\-]+",
            json!({ "name": "not a name", "author": "an-author", "description": "bad name" }),
        ),
        (
            "Author too long (maximum length=127)",
            json!({ "name": "long-author", "author": "a".repeat(128), "description": "long author" }),
        ),
        (
            "Description empty (minimum length=1)",
            json!({ "name": "empty-description", "author": "an-author", "description": "" }),
        ),
        (
            "Description too long (maximum length=256)",
            json!({
                "name": "long-description",
                "author": "an-author",
                "description": "d".repeat(257)
            }),
        ),
        (
            "eleven labels (maximum number of labels: 10)",
            json!({
                "name": "too-many-labels",
                "author": "an-author",
                "description": "too many labels",
                "labels": ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10"]
            }),
        ),
        (
            "a label outside ^[a-zA-Z0-9+\\-_:\\/@]+$",
            json!({
                "name": "bad-label",
                "author": "an-author",
                "description": "a bad label",
                "labels": ["white space"]
            }),
        ),
        (
            "an empty label (minimum length=1)",
            json!({
                "name": "empty-label",
                "author": "an-author",
                "description": "an empty label",
                "labels": [""]
            }),
        ),
        (
            "both LicenseBody and LicenseUrl",
            json!({
                "name": "both-license",
                "author": "an-author",
                "description": "two licenses",
                "licenseBody": "text",
                "licenseUrl": "https://example.com/LICENSE"
            }),
        ),
        (
            "both ReadmeBody and ReadmeUrl",
            json!({
                "name": "both-readme",
                "author": "an-author",
                "description": "two readmes",
                "readmeBody": "text",
                "readmeUrl": "https://example.com/README.md"
            }),
        ),
        (
            "both TemplateBody and TemplateUrl",
            json!({
                "name": "both-template",
                "author": "an-author",
                "description": "two templates",
                "semanticVersion": "1.0.0",
                "templateBody": "{}",
                "templateUrl": "https://example.com/template.yaml"
            }),
        ),
    ];

    for (case, request) in &cases {
        let answer = repo.create_application(request.clone()).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {}",
            answer.body
        );
    }

    // None of them wrote anything.
    let listed = repo.list_applications("").await;
    assert_eq!(
        listed.at("applications").as_array().map(Vec::len),
        Some(0),
        "a refused create must not have written anything, got {}",
        listed.body
    );

    // The same exclusivity rule holds on CreateApplicationVersion.
    let application_id = created(&repo, "exclusive-version-app").await;
    let both = repo
        .create_version(
            &application_id,
            "1.0.0",
            json!({ "templateBody": "{}", "templateUrl": "https://example.com/t.yaml" }),
        )
        .await;
    assert_eq!(
        both.status, 400,
        "CreateApplicationVersion with both TemplateBody and TemplateUrl: expected 400, got {} \
         with {}",
        both.status, both.body
    );
    assert_eq!(
        both.error_code(),
        "BadRequestException",
        "expected BadRequestException, got {}",
        both.body
    );
}

/// RULE 1, for a version's `ParameterDefinitions`: they are read out of the template the
/// caller stored, never guessed. A template the emulator cannot read yields an empty
/// list rather than an invented one.
#[tokio::test]
async fn parameter_definitions_come_from_the_stored_template() {
    let repo = Repo::new();
    let application_id = created(&repo, "template-app").await;

    let with_parameters = json!({
        // SAM is a CloudFormation transform, so a publishable template declares it.
        "Transform": "AWS::Serverless-2016-10-31",
        "Parameters": {
            "TableName": {
                "Type": "String",
                "Default": "orders",
                "Description": "the table to write to",
                "AllowedPattern": "[A-Za-z]+",
                "MinLength": 1,
                "MaxLength": 64,
                "ConstraintDescription": "letters only",
                "NoEcho": false
            },
            "BatchSize": {
                "Type": "Number",
                "Default": 10,
                "MinValue": 1,
                "MaxValue": 100,
                "AllowedValues": [1, 10, 100]
            },
            "Prefix": { "Type": "String" }
        },
        "Resources": {
            "Writer": {
                "Type": "AWS::Serverless::Function",
                "Properties": {
                    "Environment": { "Variables": { "TABLE": { "Ref": "TableName" } } },
                    // `${!Literal}` is the documented escape and names nothing, so it
                    // must not be read as a reference.
                    "Name": { "Fn::Sub": "${Prefix}-writer-${!NotAParameter}" }
                }
            },
            "Queue": {
                "Type": "AWS::SQS::Queue",
                "Properties": { "QueueName": { "Fn::Sub": ["q-${Prefix}-${BatchSize}", {}] } }
            }
        }
    })
    .to_string();

    let created = repo
        .create_version(&application_id, "1.0.0", json!({ "templateBody": with_parameters }))
        .await;
    assert_eq!(created.status, 201, "create version: got {}", created.body);
    let definitions = created
        .at("parameterDefinitions")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        definitions.len(),
        3,
        "the template declares three parameters, got {}",
        created.body
    );
    // Sorted by name, so BatchSize comes first.
    assert_eq!(
        definitions[0],
        json!({
            "name": "BatchSize",
            "type": "Number",
            "defaultValue": "10",
            "minValue": 1,
            "maxValue": 100,
            "allowedValues": ["1", "10", "100"],
            "referencedByResources": ["Queue"]
        }),
        "the BatchSize definition must be the template's own, got {}",
        definitions[0]
    );
    assert_eq!(
        definitions[1],
        json!({
            "name": "Prefix",
            "type": "String",
            "referencedByResources": ["Queue", "Writer"]
        }),
        "a parameter two resources reference lists both, sorted, got {}",
        definitions[1]
    );
    assert_eq!(
        definitions[2],
        json!({
            "name": "TableName",
            "type": "String",
            "defaultValue": "orders",
            "description": "the table to write to",
            "allowedPattern": "[A-Za-z]+",
            "minLength": 1,
            "maxLength": 64,
            "constraintDescription": "letters only",
            "noEcho": false,
            "referencedByResources": ["Writer"]
        }),
        "the TableName definition must be the template's own, got {}",
        definitions[2]
    );

    // A template declaring no parameters — in either form — one the emulator never sees,
    // and no template at all: an empty list each time, never a guess.
    let empty_cases: Vec<(&str, &str, Value)> = vec![
        (
            "a JSON template with no Parameters",
            "2.0.0",
            json!({ "templateBody": "{\"Transform\":\"AWS::Serverless-2016-10-31\",\"Resources\":{}}" }),
        ),
        (
            "a YAML template declaring no Parameters section",
            "3.0.0",
            json!({ "templateBody": "Transform: AWS::Serverless-2016-10-31\nResources:\n  Fn:\n    Type: AWS::Serverless::Function\n" }),
        ),
        (
            "a template named only by URL, which is never fetched",
            "4.0.0",
            json!({ "templateUrl": "https://example.com/template.yaml" }),
        ),
        ("no template at all", "5.0.0", json!({})),
    ];
    for (case, semantic_version, request) in &empty_cases {
        let answer = repo
            .create_version(&application_id, semantic_version, request.clone())
            .await;
        assert_eq!(
            answer.status, 201,
            "case {case:?}: expected 201, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.at("parameterDefinitions"),
            &json!([]),
            "case {case:?}: expected no parameter definitions, got {}",
            answer.at("parameterDefinitions")
        );
    }

    // And they are READ BACK from the store, not recomputed per call.
    let fetched = repo.get_application(&application_id, Some("1.0.0")).await;
    assert_eq!(
        fetched.at("version").get("parameterDefinitions"),
        created.at("parameterDefinitions").into(),
        "GetApplication must report the stored parameter definitions, got {}",
        fetched.body
    );
}

/// RULE 5: `SemanticVersion` is a semantic version (<https://semver.org/>), on the
/// create that names the first version and on the one that adds another.
///
/// The rows span the grammar: the three numeric fields, prerelease and build metadata,
/// and the forms semver 2.0.0 refuses.
#[tokio::test]
async fn a_version_that_is_not_a_semantic_version_is_refused() {
    let repo = Repo::new();
    let application_id = created(&repo, "semver-app").await;

    // Accepted forms, per semver 2.0.0 §2 and §9.
    let valid = [
        "0.0.0",
        "1.2.3",
        "10.20.30",
        "1.0.0-alpha",
        "1.0.0-alpha.1",
        "1.0.0-0.3.7",
        "1.0.0-x-y-z.0",
        "1.0.0+build.1",
        "1.0.0-beta+exp.sha.5114f85",
    ];
    for semantic_version in valid {
        let answer = repo
            .create_version(&application_id, semantic_version, json!({}))
            .await;
        assert_eq!(
            answer.status, 201,
            "case {semantic_version:?}: a semantic version is accepted, got {} with {}",
            answer.status, answer.body
        );
    }

    // Refused forms: too few fields, a leading zero, an empty or illegal identifier,
    // and a value that is not a version at all.
    let invalid = [
        "1",
        "1.0",
        "1.0.0.0",
        "01.0.0",
        "1.0.0-",
        "1.0.0-01",
        "1.0.0-alpha..1",
        "1.0.0+",
        "v1.0.0",
        "latest",
        "1.0.0 ",
        "-1.0.0",
    ];
    for semantic_version in invalid {
        // As the first version of a new application…
        let created = repo
            .create_application(json!({
                "name": "semver-create-app",
                "author": "an-author",
                "description": "created with a bad version",
                "semanticVersion": semantic_version
            }))
            .await;
        assert_eq!(
            created.status, 400,
            "case {semantic_version:?} (CreateApplication): expected 400, got {} with {}",
            created.status, created.body
        );
        assert_eq!(
            created.error_code(),
            "BadRequestException",
            "case {semantic_version:?} (CreateApplication): got {}",
            created.body
        );

        // …and as a version added to an existing one. The path label is percent-encoded
        // so the value reaches the handler as written.
        let path = format!(
            "/applications/{}/versions/{}",
            path_label(&application_id),
            semantic_version.replace(' ', "%20")
        );
        let added = repo.call("PUT", &path, Some(json!({}))).await;
        assert_eq!(
            added.status, 400,
            "case {semantic_version:?} (CreateApplicationVersion): expected 400, got {} with {}",
            added.status, added.body
        );
        assert_eq!(
            added.error_code(),
            "BadRequestException",
            "case {semantic_version:?} (CreateApplicationVersion): got {}",
            added.body
        );
    }

    // Nothing refused was stored: only the accepted versions are listed.
    let listed = repo.list_versions(&application_id, "?maxItems=100").await;
    let stored: BTreeSet<String> = listed
        .at("versions")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|v| v["semanticVersion"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        stored,
        valid.iter().map(|v| (*v).to_string()).collect(),
        "only the accepted versions are stored, got {stored:?}"
    );
}

/// RULE 3, continued: the version reported when a request names none is the one with the
/// highest semantic-version PRECEDENCE (semver 2.0.0 §11) — not the one that happened to
/// be created last.
///
/// The rows are created in an order that disagrees with precedence, and include the two
/// cases a string comparison gets wrong: `1.10.0` above `1.9.0`, and a prerelease below
/// the release it precedes.
#[tokio::test]
async fn the_latest_version_is_the_one_with_the_highest_precedence() {
    let repo = Repo::new();
    let cases: [(&str, &[&str], &str); 4] = [
        ("created in order", &["1.0.0", "1.0.1", "2.0.0"], "2.0.0"),
        ("created out of order", &["2.0.0", "1.0.0", "1.5.0"], "2.0.0"),
        (
            "a numeric field is compared as a number, not as text",
            &["1.9.0", "1.10.0"],
            "1.10.0",
        ),
        (
            "a prerelease has lower precedence than its release",
            &["2.0.0", "2.0.0-rc.1", "2.0.0-rc.2"],
            "2.0.0",
        ),
    ];
    for (index, (case, versions, expected)) in cases.iter().enumerate() {
        let application_id = created(&repo, &format!("precedence-app-{index}")).await;
        for semantic_version in *versions {
            let answer = repo
                .create_version(&application_id, semantic_version, json!({}))
                .await;
            assert_eq!(
                answer.status, 201,
                "case {case:?}: seeding {semantic_version} got {} with {}",
                answer.status, answer.body
            );
        }
        let fetched = repo.get_application(&application_id, None).await;
        assert_eq!(
            fetched
                .at("version")
                .get("semanticVersion")
                .and_then(|v| v.as_str()),
            Some(*expected),
            "case {case:?}: expected the latest to be {expected}, got {}",
            fetched.body
        );
    }
}

/// The emulator's own bounds on what one caller can make it retain, and on the work one
/// request can make it do. Each refusal is the modelled `BadRequestException`, and each
/// bound is the documented maximum its message names.
#[tokio::test]
async fn caller_driven_growth_and_work_are_bounded() {
    let repo = Repo::new();

    // An inline body is retained for the life of the record: "Maximum size 5 MB" for the
    // license and readme, and a packaged CloudFormation template's 1 MB maximum for the
    // template.
    const FIVE_MB: usize = 5 * 1024 * 1024;
    const ONE_MB: usize = 1024 * 1024;
    let oversized: Vec<(&str, Value)> = vec![
        (
            "LicenseBody over 5 MB",
            json!({
                "name": "big-license",
                "author": "an-author",
                "description": "an oversized license",
                "licenseBody": "L".repeat(FIVE_MB + 1)
            }),
        ),
        (
            "ReadmeBody over 5 MB",
            json!({
                "name": "big-readme",
                "author": "an-author",
                "description": "an oversized readme",
                "readmeBody": "R".repeat(FIVE_MB + 1)
            }),
        ),
        (
            "TemplateBody over 1 MB",
            json!({
                "name": "big-template",
                "author": "an-author",
                "description": "an oversized template",
                "semanticVersion": "1.0.0",
                "templateBody": "T".repeat(ONE_MB + 1)
            }),
        ),
    ];
    for (case, request) in &oversized {
        let answer = repo.create_application(request.clone()).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "BadRequestException",
            "case {case:?}: got {}",
            answer.body
        );
    }
    // The documented maxima themselves are accepted.
    let at_the_bound = repo
        .create_application(json!({
            "name": "at-the-bound",
            "author": "an-author",
            "description": "bodies exactly at their documented maxima",
            "licenseBody": "L".repeat(FIVE_MB),
            "semanticVersion": "1.0.0",
            "templateBody": "T".repeat(ONE_MB)
        }))
        .await;
    assert_eq!(
        at_the_bound.status, 201,
        "a body at its documented maximum is accepted, got {} with {}",
        at_the_bound.status, at_the_bound.body
    );

    // A template's shape is bounded too: CloudFormation allows at most 200 parameters
    // and 500 resources, which is what bounds the work one request can ask for.
    let application_id = created(&repo, "bounded-template-app").await;
    let mut parameters = serde_json::Map::new();
    for index in 0..201 {
        parameters.insert(format!("P{index}"), json!({ "Type": "String" }));
    }
    // Each of these declares the SAM transform, so the only thing it can be refused for
    // is the bound under test.
    const SAM: &str = "AWS::Serverless-2016-10-31";
    let too_many_parameters =
        json!({ "Transform": SAM, "Parameters": parameters }).to_string();
    let mut resources = serde_json::Map::new();
    for index in 0..501 {
        resources.insert(format!("R{index}"), json!({ "Type": "AWS::SQS::Queue" }));
    }
    let too_many_resources = json!({
        "Transform": SAM,
        "Parameters": { "P": { "Type": "String" } },
        "Resources": resources
    })
    .to_string();
    for (case, semantic_version, body) in [
        ("201 parameters", "2.0.0", too_many_parameters),
        ("501 resources", "3.0.0", too_many_resources),
    ] {
        let answer = repo
            .create_version(&application_id, semantic_version, json!({ "templateBody": body }))
            .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "BadRequestException",
            "case {case:?}: got {}",
            answer.body
        );
    }
    // And the maxima themselves are served.
    let mut parameters = serde_json::Map::new();
    for index in 0..200 {
        parameters.insert(format!("P{index}"), json!({ "Type": "String" }));
    }
    let mut resources = serde_json::Map::new();
    for index in 0..500 {
        resources.insert(
            format!("R{index}"),
            json!({ "Type": "AWS::SQS::Queue", "Properties": { "QueueName": { "Ref": "P0" } } }),
        );
    }
    let at_the_maxima =
        json!({ "Transform": SAM, "Parameters": parameters, "Resources": resources }).to_string();
    let answer = repo
        .create_version(
            &application_id,
            "4.0.0",
            json!({ "templateBody": at_the_maxima }),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "a template at CloudFormation's maxima is served, got {} with {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer
            .at("parameterDefinitions")
            .as_array()
            .map(Vec::len),
        Some(200),
        "every parameter the template declares is reported, got {}",
        answer.at("parameterDefinitions")
    );
    assert_eq!(
        answer.at("parameterDefinitions")[0]["referencedByResources"]
            .as_array()
            .map(Vec::len),
        Some(500),
        "the resources referencing P0 are all listed, got {}",
        answer.at("parameterDefinitions")[0]
    );

    // One application retains a bounded number of versions, and the catalogue a bounded
    // number of applications: past the bound the create is refused, not served by
    // evicting someone else's record.
    let ceiling_app = created(&repo, "version-ceiling-app").await;
    for index in 0..200 {
        let answer = repo
            .create_version(&ceiling_app, &format!("1.0.{index}"), json!({}))
            .await;
        assert_eq!(
            answer.status, 201,
            "version {index} is within the ceiling, got {} with {}",
            answer.status, answer.body
        );
    }
    let past = repo.create_version(&ceiling_app, "9.9.9", json!({})).await;
    assert_eq!(
        past.status, 400,
        "a version past the ceiling is refused, got {} with {}",
        past.status, past.body
    );
    assert_eq!(
        past.error_code(),
        "BadRequestException",
        "got {}",
        past.body
    );
    let still_there = repo.list_versions(&ceiling_app, "?maxItems=1").await;
    assert_eq!(
        still_there.status, 200,
        "the stored versions are untouched by the refusal, got {}",
        still_there.body
    );
}

/// The catalogue itself is bounded: the ceiling is reached by creating applications, and
/// the create past it is refused while every stored application stays readable.
#[tokio::test]
async fn the_catalogue_is_bounded() {
    let repo = Repo::new();
    // The ceiling this emulator declares for retained applications.
    const MAX_STORED_APPLICATIONS: usize = 1_000;
    for index in 0..MAX_STORED_APPLICATIONS {
        let answer = repo
            .create_application(minimal(&format!("ceiling-app-{index:04}")))
            .await;
        assert_eq!(
            answer.status, 201,
            "application {index} is within the ceiling, got {} with {}",
            answer.status, answer.body
        );
    }
    let past = repo.create_application(minimal("one-too-many")).await;
    assert_eq!(
        past.status, 400,
        "an application past the ceiling is refused, got {} with {}",
        past.status, past.body
    );
    assert_eq!(past.error_code(), "BadRequestException", "got {}", past.body);

    // The refusal evicted nothing: the first application is still there, and deleting
    // one makes room for exactly one more.
    let first = repo
        .get_application(
            "arn:aws:serverlessrepo:us-west-2:000000000000:applications/ceiling-app-0000",
            None,
        )
        .await;
    assert_eq!(
        first.status, 200,
        "the oldest application is untouched by the refusal, got {} with {}",
        first.status, first.body
    );
    let deleted = repo
        .delete_application(
            "arn:aws:serverlessrepo:us-west-2:000000000000:applications/ceiling-app-0000",
        )
        .await;
    assert_eq!(deleted.status, 204, "delete: got {}", deleted.body);
    let room = repo.create_application(minimal("one-too-many")).await;
    assert_eq!(
        room.status, 201,
        "a delete makes room for one more, got {} with {}",
        room.status, room.body
    );
}

/// RULE 6: a delete removes the application and every version it held, and the next
/// caller sees that. Deleting what is not there is an absent resource, not a success.
#[tokio::test]
async fn a_deleted_application_is_gone_for_the_next_caller() {
    let repo = Repo::new();
    let kept = created(&repo, "kept-app").await;
    let doomed = created(&repo, "doomed-app").await;
    for semantic_version in ["1.0.0", "1.1.0"] {
        let created = repo
            .create_version(&doomed, semantic_version, json!({}))
            .await;
        assert_eq!(created.status, 201, "seeding a version: {}", created.body);
    }

    let deleted = repo.delete_application(&doomed).await;
    assert_eq!(
        deleted.status, 204,
        "expected 204, got {} with {}",
        deleted.status, deleted.body
    );
    assert!(
        deleted.raw.is_empty(),
        "a 204 carries an empty body, got {:?}",
        String::from_utf8_lossy(&deleted.raw)
    );

    let fetched = repo.get_application(&doomed, None).await;
    assert_eq!(
        fetched.status, 404,
        "a deleted application is absent, got {} with {}",
        fetched.status, fetched.body
    );
    assert_eq!(
        fetched.error_code(),
        "NotFoundException",
        "expected NotFoundException, got {}",
        fetched.body
    );

    let versions = repo.list_versions(&doomed, "").await;
    assert_eq!(
        versions.status, 404,
        "the deleted application's versions went with it, got {} with {}",
        versions.status, versions.body
    );

    let listed = repo.list_applications("").await;
    let ids: Vec<String> = listed
        .at("applications")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|app| app["applicationId"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        ids,
        vec![kept.clone()],
        "only the application that was not deleted is listed, got {ids:?}"
    );

    // Deleting it again, and deleting one that never existed, are the same absence.
    for (case, application_id) in [
        ("already deleted", doomed.clone()),
        (
            "never created",
            "arn:aws:serverlessrepo:us-west-2:000000000000:applications/never-existed".to_string(),
        ),
    ] {
        let answer = repo.delete_application(&application_id).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {}",
            answer.body
        );
    }

    // The name the deleted application held is free again, and the new record is its
    // own: a fresh create, not a resurrection of the old one with its versions.
    let recreated = repo.create_application(minimal("doomed-app")).await;
    assert_eq!(
        recreated.status, 201,
        "the name of a deleted application is free again, got {} with {}",
        recreated.status, recreated.body
    );
    let versions = repo.list_versions(&doomed, "").await;
    assert_eq!(
        versions.at("versions"),
        &json!([]),
        "the re-created application starts with no versions, got {}",
        versions.body
    );
}

/// RULE 6, mirrored: every operation that names an application refuses one the store
/// does not hold, rather than inventing it.
#[tokio::test]
async fn an_application_that_was_never_created_is_not_found() {
    let repo = Repo::new();
    let unknown = "arn:aws:serverlessrepo:us-west-2:000000000000:applications/absent-app";
    let cases = [
        ("GetApplication", repo.get_application(unknown, None).await),
        (
            "GetApplication with a version",
            repo.get_application(unknown, Some("1.0.0")).await,
        ),
        (
            "ListApplicationVersions",
            repo.list_versions(unknown, "").await,
        ),
        (
            "CreateApplicationVersion",
            repo.create_version(unknown, "1.0.0", json!({})).await,
        ),
        ("DeleteApplication", repo.delete_application(unknown).await),
    ];
    for (case, answer) in cases {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404, got {} with {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {}",
            answer.body
        );
    }

    // An empty catalogue is an empty page, not a 404.
    let listed = repo.list_applications("").await;
    assert_eq!(
        listed.status, 200,
        "an empty catalogue lists as 200, got {} with {}",
        listed.status, listed.body
    );
    assert_eq!(
        listed.at("applications"),
        &json!([]),
        "an empty catalogue has no applications, got {}",
        listed.body
    );
    assert_eq!(
        listed.at("nextToken"),
        &Value::Null,
        "the final page carries no token, got {}",
        listed.body
    );
}

/// The operations this slice does not implement still answer 501 — never an empty 200,
/// which a client cannot tell from a real answer.
///
/// `PutApplicationPolicy` and `GetApplicationPolicy` are no longer among them: the
/// sibling `application_policies` suite exercises the policy they now store and report.
#[tokio::test]
async fn the_operations_this_slice_leaves_alone_still_answer_501() {
    let repo = Repo::new();
    let application_id = created(&repo, "stub-app").await;
    let label = path_label(&application_id);
    let cases = [
        (
            "UnshareApplication",
            "POST",
            format!("/applications/{label}/unshare"),
        ),
        (
            "ListApplicationDependencies",
            "GET",
            format!("/applications/{label}/dependencies"),
        ),
    ];
    for (operation, method, path) in &cases {
        let body = match *operation {
            "UnshareApplication" => Some(json!({ "organizationId": "o-abc123" })),
            _ => Some(json!({})),
        };
        let answer = repo.call(method, path, body).await;
        assert_eq!(
            answer.status, 501,
            "case {operation:?}: an unimplemented operation answers 501, got {} with {}",
            answer.status, answer.body
        );
    }
}
