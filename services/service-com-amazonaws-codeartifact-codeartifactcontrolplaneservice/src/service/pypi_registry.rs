// SPDX-License-Identifier: CC0-1.0

//! The PyPI index and upload API a repository's `pypi` endpoint serves — the package data
//! plane behind the URL `GetRepositoryEndpoint` hands out for that format.
//!
//! A CodeArtifact repository is reached by a package manager, not by an AWS SDK: the
//! endpoint that operation returns speaks the native protocol of the format it was asked
//! for, and for `pypi` that protocol is two things (AWS CodeArtifact user guide, *Use
//! CodeArtifact with Python*):
//!
//! * the UPLOAD API `twine` posts a distribution to — a `multipart/form-data` POST to the
//!   endpoint root carrying `:action=file_upload`, the project `name` and `version`, and the
//!   file itself — which answers `201` once the distribution is stored; and
//! * the SIMPLE REPOSITORY API `pip` resolves and downloads through: `GET
//!   <endpoint>simple/<project>/`, answered as the PEP 503 HTML page of links by default and
//!   as the PEP 691 JSON document for a client that asks for it by `Accept`, each link
//!   pinning the file's SHA-256, and a `GET` of a link serving that file's bytes.
//!
//! The project a path addresses is NORMALISED as PEP 503 defines it
//! ([`normalized_project`]): the index is keyed by the normalised name while the file name
//! keeps whatever the build wrote (`Sim_Pkg-1.0.0-py3-none-any.whl` is served from
//! `/simple/sim-pkg/`), which is why the two are different strings and why the stored
//! package name is the normalised one.
//!
//! Everything it serves comes out of the SAME records the control plane writes
//! ([`crate::state`]): a distribution uploaded over this endpoint is the version
//! `DescribePackageVersion` describes, its file is the asset `ListPackageVersionAssets`
//! lists — under the file's own name, with the SHA-256 the index pinned — and a version
//! whose status the control plane moved is served (or withheld) here accordingly. There is
//! no second store.
//!
//! ## What authenticates a request
//!
//! The credential a Python client presents is the authorization token
//! `GetAuthorizationToken` minted, carried as the PASSWORD of an HTTP Basic credential
//! (`twine upload --username aws --password <token>`, the same pair in a `pip.conf` index
//! URL), so this plane reads that presentation — and the bearer one beside it — and resolves
//! the token through the reading every repository endpoint shares
//! ([`crate::service::repository_data_plane::authenticated_principal`]): a token this
//! service did not mint, one the clock has expired, one scoped to another domain, and no
//! token at all are each `401`.
//!
//! ## What it does not serve yet
//!
//! * No EXTERNAL-connection resolution: a project this repository holds nowhere is not
//!   fetched from the public index, which is the behaviour of
//!   `AssociateExternalConnection` and belongs with the issue that owns it — reaching a
//!   third-party registry needs outbound egress this emulator does not have.
//! * No UPSTREAM repository resolution for this format, which is the issue that owns
//!   upstream resolution for a pypi repository (npm's plane resolves its own, see
//!   [`crate::service::npm_registry`]); a project is served out of the addressed
//!   repository's own records.

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use ::serde_json::{json, Value};
use ::std::sync::Arc;
use ::std::time::SystemTime;

// The addressing of a repository endpoint, the credential that authenticates one, the
// repository policy's say over what a request may do there, and the rendering of an answer
// are the same for every format a repository publishes an endpoint for, so they live once in
// [`crate::service::repository_data_plane`]; only PyPI's own protocol is here.
use crate::service::repository_data_plane::{
    authenticated_principal, bad_request, blocked_download, downloadable, endpoint_url, forbidden,
    method_of, not_found, parsed_uri, percent_decoded, policy_refusal, presented_basic_or_bearer,
    publish_blocked, record_created, rendered, resolve, Addressed, Coordinate, Resolved, Served,
    BASIC_CHALLENGE, PUBLISH_ACTION, READ_ACTION,
};
use crate::state::{
    service_state, Asset, ComAmazonawsCodeartifactCodeartifactcontrolplaneserviceState as CaState,
    Package, PackageVersion,
};

/// The one `PackageFormat` this plane serves ([`crate::state::PACKAGE_FORMAT_PYPI`]). It is
/// also the first segment of the endpoint path CodeArtifact publishes for it.
pub const PACKAGE_FORMAT_PYPI: &str = crate::state::PACKAGE_FORMAT_PYPI;

/// The path segment the Simple Repository API is served under, which is what makes the
/// index URL of a CodeArtifact repository `<endpoint>simple/` (PEP 503).
const SIMPLE_ROUTE: &str = "simple";

/// The form field naming what an upload POST asks for, and the one value this plane serves:
/// the legacy PyPI upload API's `:action=file_upload`, which is what `twine upload` sends.
const ACTION_FIELD: &str = ":action";
/// See [`ACTION_FIELD`].
const FILE_UPLOAD_ACTION: &str = "file_upload";

/// The form field carrying the project name the distribution belongs to.
const NAME_FIELD: &str = "name";

/// The form field carrying the version the distribution is of.
const VERSION_FIELD: &str = "version";

/// The form field carrying the distribution FILE itself.
const CONTENT_FIELD: &str = "content";

/// The form field carrying the project's long description, which is the readme
/// `GetPackageVersionReadme` reports for a Python package version.
const DESCRIPTION_FIELD: &str = "description";

/// The form field a client declares the uploaded file's SHA-256 in, lowercase hex. The
/// upload API's own integrity check: a file whose bytes hash to something else did not
/// arrive intact.
const SHA256_DIGEST_FIELD: &str = "sha256_digest";

/// The media type the PEP 503 index page carries.
const HTML_MEDIA_TYPE: &str = "text/html; charset=utf-8";

/// The media type a PEP 691 JSON index carries, which is also the value a client asks for it
/// by in `Accept`.
const JSON_SIMPLE_MEDIA_TYPE: &str = "application/vnd.pypi.simple.v1+json";

/// The `meta.api-version` a PEP 691 document declares.
const SIMPLE_API_VERSION: &str = "1.0";

/// How many fields of one upload form this plane reads.
///
/// An EMULATOR ceiling, and the companion to the body the framework has already buffered:
/// the upload API carries a bounded set of fields (the action, the coordinate, the file and
/// its metadata), so a document naming more than this is not a form a package manager
/// submitted.
const MAX_FORM_FIELDS: usize = 64;

/// The longest long description this plane retains for one version, in bytes.
///
/// The emulator's OWN bound, like every other ceiling in this store: a `README.md` is
/// kilobytes in practice, and the text is retained caller content
/// ([`crate::state::PackageVersion::readme`]), so what one upload can leave behind needs a
/// declared ceiling. A description past it is not retained; the distribution still is.
const MAX_DESCRIPTION_BYTES: usize = 1024 * 1024;

/// Whether this plane serves the request these `parts` came from — read by
/// `Service::can_handle`, so a request addressed to a repository endpoint this service
/// holds reaches this service instead of the front door's `UnknownService`.
pub fn claims_parts<Eng: Send + Sync + 'static>(
    engine: &Eng,
    parts: &::http::request::Parts,
) -> bool {
    resolve(engine, &Addressed::of_parts(parts), PACKAGE_FORMAT_PYPI).is_some()
}

/// [`claims_parts`] for the runtime-API request the serving seam is handed, resolved by the
/// same rule.
pub fn claims_request<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> bool {
    let uri = parsed_uri(req.uri());
    resolve(engine, &Addressed::of_request(req, &uri), PACKAGE_FORMAT_PYPI).is_some()
}

/// Answer a request this plane claimed, out of the records the control plane holds.
///
/// Reached from the ONE operation this service serves a `pypi` endpoint's traffic with
/// ([`PypiRegistryEndpoint`]), which the routing seam resolves before the request is
/// authenticated — which is what lets a client carrying an authorization token rather than a
/// SigV4 signature be answered at all.
///
/// Every state change it makes is RECORDED on the store
/// ([`CaState::record_version_change`]); the operation announces them once this function has
/// returned and the store's lock is released.
pub async fn serve<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> RtResponse {
    let uri = parsed_uri(req.uri());
    let addressed = Addressed::of_request(req, &uri);
    let Some(resolved) = resolve(engine, &addressed, PACKAGE_FORMAT_PYPI) else {
        // The addressing resolved for the claim and no longer does (the repository was
        // deleted by another request in between), so there is no index at this address.
        return rendered(&not_found(
            "the repository endpoint was not found".to_string(),
        ));
    };
    let Some(state) = service_state(engine) else {
        return rendered(&Served::json(
            503,
            &json!({ "error": "the CodeArtifact store is unavailable" }),
        ));
    };
    let now = SystemTime::now();

    // The credential first, for every route: an index that resolved a project or accepted an
    // upload from an unauthenticated client would be an index no credential gates. The
    // principal the token speaks for is carried out of the record, because what that
    // principal may do HERE is what this repository's policy states.
    let presented = presented_basic_or_bearer(req.headers().get("authorization"));
    let principal = match authenticated_principal(
        &state.lock(),
        &resolved,
        &presented,
        BASIC_CHALLENGE,
        now,
    ) {
        Ok(principal) => principal,
        Err(refusal) => return rendered(&refusal),
    };

    let Some(request) = PypiRequest::of(&method_of(req), &resolved.path, accepts_json(req)) else {
        return rendered(&not_found("the index route was not found".to_string()));
    };
    // What this request ASKS to do in the repository, which is the action its policy is
    // evaluated for.
    let action = match request {
        PypiRequest::Upload => PUBLISH_ACTION,
        PypiRequest::RootIndex | PypiRequest::ProjectIndex(..) | PypiRequest::File(..) => {
            READ_ACTION
        }
    };
    if let Some(refusal) = policy_refusal(&state.lock(), &resolved, &principal, action) {
        return rendered(&refusal);
    }

    let served = match request {
        PypiRequest::Upload => {
            let body = req.body().bytes().unwrap_or_default();
            let content_type = req.headers().get("content-type").unwrap_or_default();
            match read_upload(content_type, body).await {
                Ok(upload) => store_upload(&mut state.lock(), &resolved, upload, now),
                Err(refusal) => refusal,
            }
        }
        PypiRequest::RootIndex => root_index(&state.lock(), &resolved),
        PypiRequest::ProjectIndex(project, json_requested) => project_index(
            &state.lock(),
            &resolved,
            &project,
            &endpoint_url(&addressed, &resolved),
            json_requested,
        ),
        PypiRequest::File(project, file_name) => {
            distribution(&state.lock(), &resolved, &project, &file_name)
        }
    };
    rendered(&served)
}

// ---------------------------------------------------------------------------
// The PyPI protocol: what a path and a method ask for
// ---------------------------------------------------------------------------

/// What one request of the PyPI protocols asks for.
#[derive(Debug, PartialEq, Eq)]
enum PypiRequest {
    /// `POST /`: a distribution upload, as the legacy upload API carries one.
    Upload,
    /// `GET /simple/`: the index of every project this repository holds (PEP 503's root
    /// page).
    RootIndex,
    /// `GET /simple/<project>/`: one project's index, and whether the client asked for the
    /// PEP 691 JSON rendering of it.
    ProjectIndex(String, bool),
    /// `GET /simple/<project>/<file>`: one distribution file's bytes.
    File(String, String),
}

impl PypiRequest {
    /// What `method` on the endpoint path `path` asks for, or `None` for a route this plane
    /// does not serve. `json_requested` is what the request's `Accept` asked for, which
    /// decides the RENDERING of an index and nothing else.
    fn of(method: &str, path: &str, json_requested: bool) -> Option<Self> {
        let segments: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
        match (method, segments.as_slice()) {
            // The endpoint root itself, which is where `twine` posts a distribution.
            ("POST", []) => Some(Self::Upload),
            ("GET", [simple]) if *simple == SIMPLE_ROUTE => Some(Self::RootIndex),
            ("GET", [simple, project]) if *simple == SIMPLE_ROUTE => Some(Self::ProjectIndex(
                normalized_project(&percent_decoded(project)?),
                json_requested,
            )),
            ("GET", [simple, project, file]) if *simple == SIMPLE_ROUTE => Some(Self::File(
                normalized_project(&percent_decoded(project)?),
                percent_decoded(file)?,
            )),
            _ => None,
        }
    }
}

/// Whether the request asked for the PEP 691 JSON rendering of an index, by naming its
/// media type in `Accept` ("clients that wish to use the JSON serialization send that
/// serialization's content type in `Accept`").
fn accepts_json(req: &RtRequest) -> bool {
    req.headers()
        .get("accept")
        .is_some_and(|accept| accept.contains(JSON_SIMPLE_MEDIA_TYPE))
}

/// The project name as PEP 503 normalises one: every run of `-`, `_` and `.` collapsed to a
/// single `-`, lowercased.
///
/// This is the name the index is keyed by and the name CodeArtifact stores a Python package
/// under, which is why a path spelled with underscores or capitals addresses the same
/// project as the normalised one — while the FILE name keeps whatever the build wrote.
pub fn normalized_project(name: &str) -> String {
    let mut normalized = String::with_capacity(name.len());
    let mut separator_run = false;
    for character in name.chars() {
        if matches!(character, '-' | '_' | '.') {
            separator_run = true;
            continue;
        }
        if separator_run && !normalized.is_empty() {
            normalized.push('-');
        }
        separator_run = false;
        normalized.extend(character.to_lowercase());
    }
    normalized
}

// ---------------------------------------------------------------------------
// The index
// ---------------------------------------------------------------------------

/// The index of every project the addressed repository holds for this format — the page a
/// client lands on at `<endpoint>simple/`.
fn root_index(store: &CaState, resolved: &Resolved) -> Served {
    let projects = held_projects(store, resolved);
    let links: String = projects
        .iter()
        .map(|project| {
            format!(
                "    <a href=\"{0}/\">{0}</a><br/>\n",
                escaped(project)
            )
        })
        .collect();
    Served::text(
        200,
        HTML_MEDIA_TYPE,
        page("Simple index", &links),
    )
}

/// The index of ONE project: every file the addressed repository holds for it, each pinning
/// its own SHA-256, in the rendering the client asked for.
///
/// Only a version in [`crate::state::PACKAGE_VERSION_STATUS_PUBLISHED`] is listed. That is
/// the one status in which a version is both listed and resolvable: an `Unlisted` version is
/// by its own definition hidden from the listing while its content stays downloadable
/// ([`distribution`] serves it), and an `Archived`, `Disposed` or `Unfinished` one is not a
/// version a client can resolve at all. A project the repository does not hold, and one
/// holding no listed version, is `404` — the answer `pip` reads as "this index has no such
/// project".
fn project_index(
    store: &CaState,
    resolved: &Resolved,
    project: &str,
    endpoint: &str,
    json_requested: bool,
) -> Served {
    let listed = listed_files(store, resolved, project);
    if listed.is_empty() {
        return not_found(project_not_found(project));
    }
    if json_requested {
        let files: Vec<Value> = listed
            .iter()
            .map(|file| {
                json!({
                    "filename": file.name,
                    "url": file_url(endpoint, project, &file.name),
                    "hashes": { "sha256": file.sha256 },
                    "yanked": false,
                })
            })
            .collect();
        let versions: Vec<Value> = listed
            .iter()
            .map(|file| Value::String(file.version.clone()))
            .collect();
        return Served::text(
            200,
            JSON_SIMPLE_MEDIA_TYPE,
            json!({
                "meta": { "api-version": SIMPLE_API_VERSION },
                "name": project,
                "files": files,
                "versions": versions,
            })
            .to_string(),
        );
    }
    let links: String = listed
        .iter()
        .map(|file| {
            format!(
                "    <a href=\"{}#sha256={}\">{}</a><br/>\n",
                escaped(&file_url(endpoint, project, &file.name)),
                escaped(&file.sha256),
                escaped(&file.name)
            )
        })
        .collect();
    Served::text(
        200,
        HTML_MEDIA_TYPE,
        page(&format!("Links for {project}"), &links),
    )
}

/// One PEP 503 page: the title, the same text as its heading, and the links.
fn page(title: &str, links: &str) -> String {
    format!(
        "<!DOCTYPE html>\n<html>\n  <head>\n    <meta name=\"pypi:repository-version\" \
         content=\"{SIMPLE_API_VERSION}\">\n    <title>{0}</title>\n  </head>\n  <body>\n    \
         <h1>{0}</h1>\n{links}  </body>\n</html>\n",
        escaped(title)
    )
}

/// The URL this plane serves one distribution file at, under the address the client
/// actually reached.
fn file_url(endpoint: &str, project: &str, file_name: &str) -> String {
    format!("{endpoint}/{SIMPLE_ROUTE}/{project}/{file_name}")
}

/// Text with the characters that would be markup in an HTML page escaped.
fn escaped(text: &str) -> String {
    text.chars()
        .map(|character| match character {
            '&' => "&amp;".to_string(),
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '"' => "&quot;".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// One file an index lists: its name, the version it belongs to, and the digest the index
/// pins it by — the digest of the bytes AS STORED, so a client verifying integrity verifies
/// against the bytes it will actually receive.
#[derive(Debug)]
struct IndexedFile {
    name: String,
    version: String,
    sha256: String,
}

/// Every file the addressed repository lists for `project`, version by version in the stored
/// order.
fn listed_files(store: &CaState, resolved: &Resolved, project: &str) -> Vec<IndexedFile> {
    let Some(held) = held_project(store, resolved, project) else {
        return Vec::new();
    };
    held.versions
        .values()
        .filter(|version| version.status == crate::state::PACKAGE_VERSION_STATUS_PUBLISHED)
        .flat_map(|version| {
            version.assets.values().map(|asset| IndexedFile {
                name: asset.name().to_string(),
                version: version.version.clone(),
                sha256: asset.sha256_hex().to_string(),
            })
        })
        .collect()
}

/// Every project the addressed repository holds a LISTED version for, by the name it is
/// stored under (which is the normalised one) — the projects a client can resolve, so the
/// root page names exactly the projects whose own index answers.
fn held_projects(store: &CaState, resolved: &Resolved) -> Vec<String> {
    store
        .domain(&resolved.domain, Some(&resolved.domain_owner))
        .and_then(|domain| domain.repositories.get(&resolved.repository))
        .map(|repository| {
            repository
                .packages
                .values()
                .filter(|package| package.format == PACKAGE_FORMAT_PYPI)
                .filter(|package| {
                    package.versions.values().any(|version| {
                        version.status == crate::state::PACKAGE_VERSION_STATUS_PUBLISHED
                    })
                })
                .map(|package| package.name.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// The bytes of the distribution file `file_name` names, for `project`.
///
/// A version's status decides whether its CONTENT is served, independently of whether the
/// version is listed ([`downloadable`]).
fn distribution(
    store: &CaState,
    resolved: &Resolved,
    project: &str,
    file_name: &str,
) -> Served {
    let Some(held) = held_project(store, resolved, project) else {
        return not_found(project_not_found(project));
    };
    let holding = held
        .versions
        .values()
        .find(|version| version.assets.contains_key(file_name));
    let Some(version) = holding else {
        return not_found(format!(
            "Package version not found in repository. Asset = {{format={PACKAGE_FORMAT_PYPI}, \
             name={project}, asset={file_name}}}"
        ));
    };
    if !downloadable(&version.status) {
        return blocked_download(format!(
            "the package version {} of {project} is {} and its assets cannot be downloaded",
            version.version, version.status
        ));
    }
    let Some(asset) = version.assets.get(file_name) else {
        return not_found(format!(
            "Package version not found in repository. Asset = {{format={PACKAGE_FORMAT_PYPI}, \
             name={project}, asset={file_name}}}"
        ));
    };
    Served::bytes(asset.content().to_vec())
}

/// The sentence this plane states for a project the addressed repository does not hold: the
/// one the control plane states for the same absence
/// ([`crate::state::package_not_found_message`]), so a client reading both reads one
/// wording.
fn project_not_found(project: &str) -> String {
    crate::state::package_not_found_message(PACKAGE_FORMAT_PYPI, None, project)
}

/// The package record the addressed repository holds for `project`, or `None`.
fn held_project<'s>(
    store: &'s CaState,
    resolved: &Resolved,
    project: &str,
) -> Option<&'s Package> {
    store
        .domain(&resolved.domain, Some(&resolved.domain_owner))?
        .repositories
        .get(&resolved.repository)?
        .package(PACKAGE_FORMAT_PYPI, None, project)
}

// ---------------------------------------------------------------------------
// Uploading
// ---------------------------------------------------------------------------

/// One distribution an upload form carries: the coordinate it declared, the file itself as
/// it will be stored, and the long description declared beside it.
#[derive(Debug)]
struct Upload {
    /// The project name as PEP 503 normalises it, which is what the package is stored under.
    project: String,
    version: String,
    /// The distribution file, under the name the build gave it, with its digests already
    /// computed — the asset the version will hold, so the bytes are never copied again.
    asset: Asset,
    /// The `description` field, when the form carried one inside
    /// [`MAX_DESCRIPTION_BYTES`]: the version's readme.
    description: Option<String>,
}

/// What an upload form carries, or the refusal the document itself earns.
///
/// `content_type` is the request's own, which is where the multipart BOUNDARY travels. The
/// body is already buffered by the time any operation runs, so the parser reads it as a
/// one-chunk stream.
async fn read_upload(content_type: &str, body: &[u8]) -> Result<Upload, Served> {
    let unreadable = |reason: &str| {
        bad_request(format!(
            "the upload is not a multipart/form-data document the Python upload API sends: \
             {reason}"
        ))
    };
    let Ok(boundary) = ::multer::parse_boundary(content_type) else {
        return Err(unreadable("its content type names no multipart boundary"));
    };
    let payload = ::bytes::Bytes::copy_from_slice(body);
    let stream = ::futures_util::stream::once(async move {
        Ok::<::bytes::Bytes, ::std::io::Error>(payload)
    });
    let mut document = ::multer::Multipart::new(stream, boundary);

    let mut fields: Vec<(String, String)> = Vec::new();
    let mut file: Option<(String, Vec<u8>)> = None;
    loop {
        let next = document.next_field().await;
        let Ok(next) = next else {
            return Err(unreadable("one of its parts could not be read"));
        };
        let Some(field) = next else { break };
        if fields.len() >= MAX_FORM_FIELDS {
            return Err(unreadable("it carries more parts than an upload form does"));
        }
        // Read off the part's head before its bytes, which consume the field.
        let name = field.name().unwrap_or_default().to_string();
        let part_file_name = field.file_name().map(str::to_string);
        let Ok(bytes) = field.bytes().await else {
            return Err(unreadable("one of its parts could not be read"));
        };
        if name == CONTENT_FIELD {
            if bytes.len() > crate::state::MAX_ASSET_BYTES {
                return Err(Served::json(
                    413,
                    &json!({
                        "error": format!(
                            "the distribution is {} bytes, more than this emulator's maximum of \
                             {} bytes",
                            bytes.len(),
                            crate::state::MAX_ASSET_BYTES
                        )
                    }),
                ));
            }
            file = Some((part_file_name.unwrap_or_default(), bytes.to_vec()));
            continue;
        }
        // Every other field of the upload form is text.
        let Ok(value) = String::from_utf8(bytes.to_vec()) else {
            return Err(unreadable("one of its text fields is not UTF-8"));
        };
        fields.push((name, value));
    }

    let field = |wanted: &str| {
        fields
            .iter()
            .find(|(name, _)| name == wanted)
            .map(|(_, value)| value.as_str())
    };
    let action = field(ACTION_FIELD).unwrap_or_default();
    if action != FILE_UPLOAD_ACTION {
        return Err(bad_request(format!(
            "the upload asks for {action:?}, and this endpoint serves \
             {FILE_UPLOAD_ACTION:?} uploads"
        )));
    }
    let Some(declared_name) = field(NAME_FIELD).filter(|name| !name.is_empty()) else {
        return Err(bad_request(
            "the upload declares no project name".to_string(),
        ));
    };
    let Some(version) = field(VERSION_FIELD).filter(|version| !version.is_empty()) else {
        return Err(bad_request("the upload declares no version".to_string()));
    };
    let Some((file_name, content)) = file else {
        return Err(bad_request(
            "the upload attaches no distribution file".to_string(),
        ));
    };
    if file_name.is_empty() {
        return Err(bad_request(
            "the attached distribution carries no file name".to_string(),
        ));
    }
    // The upload API's own integrity check, where the client declared a digest: a file whose
    // bytes hash to something else is not the file that was built.
    let stored = Asset::new(&file_name, content);
    if let Some(declared) = field(SHA256_DIGEST_FIELD).filter(|digest| !digest.is_empty()) {
        if !declared.eq_ignore_ascii_case(stored.sha256_hex()) {
            return Err(bad_request(
                "The digest supplied does not match a digest calculated from the uploaded file."
                    .to_string(),
            ));
        }
    }
    let description = field(DESCRIPTION_FIELD)
        .filter(|description| !description.is_empty())
        .and_then(|description| {
            if description.len() > MAX_DESCRIPTION_BYTES {
                ::tracing::warn!(
                    bytes = description.len(),
                    "the uploaded long description is larger than this emulator retains, so the \
                     version is stored without a readme"
                );
                return None;
            }
            Some(description.to_string())
        });
    Ok(Upload {
        project: normalized_project(declared_name),
        version: version.to_string(),
        asset: stored,
        description,
    })
}

/// Store the distribution an upload carries, into the same records
/// `PublishPackageVersion` writes.
///
/// Every refusal is taken BEFORE anything is stored, so a rejected upload leaves nothing
/// behind: the package's origin control has to permit a direct publish, the file must not be
/// one the version already holds, and the content has to fit inside what this emulator
/// retains. A second distribution of a version the repository already holds is accepted as
/// another ASSET of that version, which is what a project that ships both a wheel and a
/// source distribution uploads.
fn store_upload(
    store: &mut CaState,
    resolved: &Resolved,
    upload: Upload,
    now: SystemTime,
) -> Served {
    // What the service already retains, read before the records are borrowed mutably: the
    // ceiling is on the TOTAL, so accepted uploads cannot accumulate past it.
    let stored_content_bytes = store.stored_content_bytes();
    let incoming = upload.asset.size_bytes().saturating_add(
        i64::try_from(upload.description.as_ref().map_or(0, String::len)).unwrap_or(i64::MAX),
    );
    if stored_content_bytes.saturating_add(incoming) > crate::state::MAX_STORED_ASSET_BYTES {
        return forbidden(format!(
            "this emulator already retains {stored_content_bytes} bytes of package content \
             and its ceiling is {} bytes",
            crate::state::MAX_STORED_ASSET_BYTES
        ));
    }

    let Some(repository) = store
        .domain_mut(&resolved.domain, Some(&resolved.domain_owner))
        .and_then(|domain| domain.repositories.get_mut(&resolved.repository))
    else {
        return not_found("the repository endpoint was not found".to_string());
    };
    let repository_name = repository.name.clone();
    let key = crate::state::package_key(PACKAGE_FORMAT_PYPI, None, &upload.project);

    // Origin control decides whether this package accepts a direct publish at all, checked
    // against the STORED package: one that does not exist yet is created with publishing
    // allowed, while one whose first version arrived by ingestion is created with publishing
    // blocked, and `PutPackageOriginConfiguration` is what moves either.
    if repository
        .packages
        .get(&key)
        .is_some_and(|existing| !existing.publish_allowed())
    {
        return publish_blocked();
    }
    if !repository.packages.contains_key(&key)
        && repository.packages.len() >= crate::state::MAX_PACKAGES_PER_REPOSITORY
    {
        return forbidden(format!(
            "repository {repository_name} already holds the maximum of {} packages",
            crate::state::MAX_PACKAGES_PER_REPOSITORY
        ));
    }

    let held = repository.packages.entry(key).or_insert_with(|| {
        // A package record this upload CREATES carries the controls CodeArtifact sets for a
        // package that came to exist by a direct publish: publishing allowed, upstream
        // ingestion blocked.
        Package::published(PACKAGE_FORMAT_PYPI, None, &upload.project)
    });
    if !held.versions.contains_key(&upload.version)
        && held.versions.len() >= crate::state::MAX_PACKAGE_VERSIONS_PER_PACKAGE
    {
        return forbidden(format!(
            "the package already holds the maximum of {} versions",
            crate::state::MAX_PACKAGE_VERSIONS_PER_PACKAGE
        ));
    }
    let version = held
        .versions
        .entry(upload.version.clone())
        .or_insert_with(|| PackageVersion {
            version: upload.version.clone(),
            status: crate::state::PACKAGE_VERSION_STATUS_PUBLISHED.to_string(),
            published_time: now,
            // Uploaded straight into this repository, not fetched from an upstream or an
            // external connection.
            origin_type: crate::state::PACKAGE_VERSION_ORIGIN_INTERNAL.to_string(),
            entry_point_repository: repository_name.clone(),
            assets: ::std::collections::BTreeMap::new(),
            // A Python distribution carries no metadata document of its own in the upload;
            // what it declares beside the file is the readme below.
            manifest: None,
            readme: None,
            sequence_number: 1,
        });
    // A file the version already holds is immutable, which is the upload API's own rule for
    // a file name it has already seen.
    if version.assets.contains_key(upload.asset.name()) {
        return bad_request(format!(
            "File already exists. The file {} is already stored for version {} of {}.",
            upload.asset.name(),
            upload.version,
            upload.project
        ));
    }
    if version.assets.len() >= crate::state::MAX_ASSETS_PER_PACKAGE_VERSION {
        return forbidden(format!(
            "the package version already holds the maximum of {} assets",
            crate::state::MAX_ASSETS_PER_PACKAGE_VERSION
        ));
    }
    let file_name = upload.asset.name().to_string();
    version.assets.insert(file_name.clone(), upload.asset);
    // The long description the upload declared is the version's readme. A later upload of
    // another distribution of the same version does not unset the one the first declared.
    if version.readme.is_none() {
        version.readme = upload.description;
    }
    let created = version.assets.len() == 1;

    ::tracing::info!(
        domain = %resolved.domain,
        repository = %repository_name,
        package = %upload.project,
        version = %upload.version,
        asset = %file_name,
        "a package manager uploaded a Python distribution over the repository endpoint"
    );
    // A version this upload CREATED is a state change of that version, announced on the
    // account's default event bus once this request has released the store. A further
    // distribution of a version that already existed changes the version's content, which
    // the control plane's own update path announces; this upload announces the creation it
    // performed.
    if created {
        record_created(
            store,
            resolved,
            &Coordinate {
                format: PACKAGE_FORMAT_PYPI,
                namespace: None,
                name: &upload.project,
            },
            ::std::slice::from_ref(&upload.version),
            now,
        );
    }
    // `201 Created`, which is what the upload API answers for a distribution it stored.
    Served::status_only(201)
}

// ---------------------------------------------------------------------------
// The operation a repository endpoint's traffic is served as
// ---------------------------------------------------------------------------

/// The ONE operation this service serves a `pypi` endpoint's own traffic with.
///
/// A repository endpoint's traffic names no modelled operation — no modelled operation is
/// bound under a repository endpoint's path — so the service RESOLVES it to this one
/// ([`crate::service::EmulatedService::override_operation`]) rather than letting it fall
/// through to a routing refusal. Being an operation is what gives this plane the two things
/// a routing refusal cannot have:
///
/// * the request's own callback into this emulator, so the package version state changes an
///   upload committed are announced on the account's default event bus
///   ([`crate::state::package_version_events`]) the way every other service announces its
///   own events;
/// * `auth_schemes` of its own, which is what says this request is NOT SigV4-signed: the
///   credential a Python client presents is the repository's authorization token, resolved
///   by [`serve`] against the tokens this service remembers minting.
pub struct PypiRegistryEndpoint<Eng> {
    /// The SAME shared engine every operation of this service holds: a distribution
    /// uploaded over this endpoint is a version of the repository the control plane
    /// describes.
    engine: Arc<Eng>,
}

impl<Eng> PypiRegistryEndpoint<Eng> {
    /// Build the endpoint's operation from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: Send + Sync + 'static> ::smithy_server_runtime::ErasedOperation
    for PypiRegistryEndpoint<Eng>
{
    fn run<'r>(
        &'r self,
        _svc: &'r dyn ::smithy_server_runtime::Service,
        req: &'r RtRequest,
        ctx: &'r ::smithy_server_runtime::OperationContext<'r>,
    ) -> ::smithy_server_runtime::BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The answer is composed with no store lock held past it — [`serve`] takes and
            // releases the guard inside itself — so the announcement that follows cannot
            // hold this request's own store.
            let response = serve(&*self.engine, req).await;
            crate::state::package_version_events::publish(ctx, &*self.engine).await;
            response
        })
    }

    /// Served UNSIGNED: a Python client presents the repository's authorization token, not a
    /// SigV4 signature, so there is no signature for the framework to verify. The credential
    /// is not skipped — [`serve`] resolves the presented token against the tokens this
    /// service minted and answers `401` for anything else.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        Some(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a project name normalises as PEP 503 defines it — runs of `-`, `_` and `.`
    /// collapse to one `-`, and the result is lowercase.
    #[test]
    fn a_project_name_normalises_the_way_pep_503_defines_it() {
        // (case, the name as a build or a path spells it, the normalised form). The rows
        // span what the rule generalises over: each separator, a run of them, capitals, and
        // a name that needs no change.
        let rows: &[(&str, &str, &str)] = &[
            ("a name needing no change", "requests", "requests"),
            ("underscores", "sim2real_pkg", "sim2real-pkg"),
            ("dots", "zope.interface", "zope-interface"),
            ("capitals", "Django", "django"),
            ("a run of separators", "a--_.b", "a-b"),
            ("every form at once", "Sim2Real_Pkg.Core", "sim2real-pkg-core"),
        ];
        for (case, spelled, expected) in rows {
            let normalized = normalized_project(spelled);
            assert_eq!(
                normalized, *expected,
                "case {case:?}: expected {expected:?}, got {normalized:?}"
            );
        }
    }

    /// RULE: an endpoint path and a method name one PyPI route, and the project a path
    /// addresses is the normalised one.
    #[test]
    fn a_path_and_a_method_name_one_pypi_route() {
        // (case, method, path, whether JSON was asked for, the route)
        let rows: &[(&str, &str, &str, bool, Option<PypiRequest>)] = &[
            ("an upload to the endpoint root", "POST", "/", false, Some(PypiRequest::Upload)),
            ("the root index", "GET", "/simple/", false, Some(PypiRequest::RootIndex)),
            (
                "a project index",
                "GET",
                "/simple/sim-pkg/",
                false,
                Some(PypiRequest::ProjectIndex("sim-pkg".to_string(), false)),
            ),
            (
                "a project index spelled unnormalised",
                "GET",
                "/simple/Sim_Pkg/",
                true,
                Some(PypiRequest::ProjectIndex("sim-pkg".to_string(), true)),
            ),
            (
                "a distribution file",
                "GET",
                "/simple/sim-pkg/sim_pkg-1.0.0-py3-none-any.whl",
                false,
                Some(PypiRequest::File(
                    "sim-pkg".to_string(),
                    "sim_pkg-1.0.0-py3-none-any.whl".to_string(),
                )),
            ),
            ("a route this plane does not serve", "GET", "/pypi/x", false, None),
            ("a method this plane does not serve", "DELETE", "/simple/sim-pkg/", false, None),
            ("an upload under the index", "POST", "/simple/", false, None),
        ];
        for (case, method, path, json_requested, expected) in rows {
            let named = PypiRequest::of(method, path, *json_requested);
            assert_eq!(
                named, *expected,
                "case {case:?}: expected {expected:?}, got {named:?}"
            );
        }
    }
}
