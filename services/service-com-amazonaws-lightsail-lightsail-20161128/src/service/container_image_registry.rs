// SPDX-License-Identifier: CC0-1.0

//! The container image registry of the account: the data plane behind the
//! address `CreateContainerServiceRegistryLogin` hands out.
//!
//! # Not a modelled operation
//!
//! `ContainerServiceRegistryLogin$registry` is "The address to use to push
//! container images to the container image registry of a Lightsail account", and
//! `RegisterContainerImage` then "[r]egisters a container image to your Amazon
//! Lightsail container service" — an image that is already IN that registry,
//! which is why its own documentation says the action "is not required if you
//! install and use the Lightsail Control (lightsailctl) plugin to push container
//! images". So the push is a real step of the documented workflow, and what it
//! speaks is the container image registry HTTP API, not an AWS protocol: it
//! carries no `X-Amz-Target`, names no modelled operation, and authenticates with
//! the HTTP Basic credentials that operation created. Nothing in the generated
//! routing table can name it, so it is reached through the two seams any emulated
//! data plane uses:
//!
//! * [`crate::service::EmulatedService::can_handle`] claims a request addressed
//!   to the published registry path space
//!   (`crate::state::CONTAINER_IMAGE_REGISTRY_PATH_PREFIX`), which carries no
//!   routing key of its own;
//! * [`smithy_server_runtime::Service::override_routing_error`] serves it, after
//!   the log in check in [`crate::service`] — the router reaches that seam before
//!   SigV4 authentication, which is what lets a registry client authenticate the
//!   way a registry client does.
//!
//! # What is served
//!
//! The registry API under the announced address, which is what a push walks:
//!
//! * `GET|HEAD /v2/` — the version check that says this is a v2 registry;
//! * `POST /v2/<name>/blobs/uploads/` — start an upload, or push a whole blob in
//!   one request when it carries `?digest=`;
//! * `PATCH|PUT /v2/<name>/blobs/uploads/<session>` — send a blob in chunks and
//!   finish it; `GET` reports how far one got and `DELETE` cancels it;
//! * `GET|HEAD /v2/<name>/blobs/<digest>` — read a blob back;
//! * `PUT /v2/<name>/manifests/<reference>` — push the image manifest, which is
//!   what completes an image and what gives it the digest a push reports;
//! * `GET|HEAD /v2/<name>/manifests/<reference>` — read a manifest back;
//! * `GET /v2/<name>/tags/list` — the tags of one repository.
//!
//! Everything it holds is CONTENT-ADDRESSED, exactly as the registry API
//! defines: a blob and a manifest are stored under the digest of their own bytes
//! ([`crate::state::registry_digest`]), the digest a client claims is checked
//! against the digest of what actually arrived, a chunked upload takes its
//! chunks in the order `Content-Range` says, and a manifest is accepted only
//! once every blob it references is in the registry and only in one of the
//! manifest media types this registry serves ([`MANIFEST_MEDIA_TYPES`]). That is
//! what makes `RegisterContainerImage` meaningful: the digest it takes names an
//! image whose bytes this emulator holds, and a digest nothing pushed still
//! names nothing.
//!
//! Two bounds apply to every write, both judged before any byte is stored:
//! [`crate::state::MAX_REGISTRY_BLOB_BYTES`] for one blob and
//! [`crate::state::MAX_REGISTRY_BYTES_PER_REGION`] for one region's registry.
//!
//! # The error envelope
//!
//! A refusal is the registry API's own envelope — `{"errors":[{"code":…,
//! "message":…,"detail":…}]}` with that API's code and its sentence — never this
//! service's awsJson shapes, which belong to the modelled operations. The
//! `detail` is built from the request and from this registry's own bounds; no
//! message carries anything else a caller did not already send.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;

use crate::state::{
    self, ContainerImageRegistryTarget, RegistryWrite, UploadAppend, UploadFinish,
    MAX_REGISTRY_BLOB_BYTES,
};

/// The header a registry answers its version check with.
const API_VERSION_HEADER: &str = "docker-distribution-api-version";

/// The value of [`API_VERSION_HEADER`]: this registry speaks version 2 of the
/// registry API.
const API_VERSION: &str = "registry/2.0";

/// The header carrying the digest of the content a response is about.
const CONTENT_DIGEST_HEADER: &str = "docker-content-digest";

/// The header carrying the id of an upload session.
const UPLOAD_SESSION_HEADER: &str = "docker-upload-uuid";

/// The media type a blob is read back as: a blob is opaque bytes to the
/// registry, whatever the manifest that references it says they are.
const BLOB_CONTENT_TYPE: &str = "application/octet-stream";

/// The media type the registry's own JSON — a version check, a tag list, an
/// error — is answered in.
const JSON_CONTENT_TYPE: &str = "application/json";

/// The longest repository name this registry accepts, which is the registry
/// API's own limit on one.
const MAX_REPOSITORY_NAME_LEN: usize = 255;

/// The longest tag this registry accepts, which is the registry API's own limit
/// on one.
const MAX_TAG_LEN: usize = 128;

/// The manifest media types this registry stores.
///
/// A registry stores a manifest only in a media type it knows how to serve, and
/// refuses one it does not: these are the image-manifest and image-index media
/// types of the registry API's two manifest schemas (the OCI image
/// specification's `image.manifest`/`image.index`, and the Docker registry's
/// `distribution.manifest` v2 and its manifest list). What is held beside a
/// stored manifest is the matching entry OF THIS LIST rather than the spelling
/// the push used, so a value a client sent never becomes a response header.
const MANIFEST_MEDIA_TYPES: &[&str] = &[
    "application/vnd.oci.image.manifest.v1+json",
    "application/vnd.oci.image.index.v1+json",
    "application/vnd.docker.distribution.manifest.v2+json",
    "application/vnd.docker.distribution.manifest.list.v2+json",
];

/// The entry of [`MANIFEST_MEDIA_TYPES`] that `declared` names, or `None` when
/// it names none.
///
/// A media type is compared without its parameters and without regard to case,
/// as RFC 9110 defines the comparison, and the answer is the canonical spelling
/// of this registry's own list.
fn manifest_media_type(declared: &str) -> Option<&'static str> {
    let name = declared
        .split(';')
        .next()
        .unwrap_or_default()
        .trim();
    MANIFEST_MEDIA_TYPES
        .iter()
        .find(|known| known.eq_ignore_ascii_case(name))
        .copied()
}

/// One refusal of the registry API: the status it is answered with, its code,
/// and the sentence that code stands for.
#[derive(Debug, Clone, Copy)]
struct RegistryRefusal {
    status: u16,
    code: &'static str,
    message: &'static str,
}

/// The digest a client claimed is not the digest of what it sent, or is not a
/// digest this registry addresses content by.
const DIGEST_INVALID: RegistryRefusal = RegistryRefusal {
    status: 400,
    code: "DIGEST_INVALID",
    message: "provided digest did not match uploaded content",
};

/// The registry holds no blob of that digest.
const BLOB_UNKNOWN: RegistryRefusal = RegistryRefusal {
    status: 404,
    code: "BLOB_UNKNOWN",
    message: "blob unknown to registry",
};

/// The upload cannot be taken as it stands — no digest on the request that
/// finishes it, or more bytes than this registry holds for one blob.
const BLOB_UPLOAD_INVALID: RegistryRefusal = RegistryRefusal {
    status: 400,
    code: "BLOB_UPLOAD_INVALID",
    message: "blob upload invalid",
};

/// The registry knows no upload session of that id.
const BLOB_UPLOAD_UNKNOWN: RegistryRefusal = RegistryRefusal {
    status: 404,
    code: "BLOB_UPLOAD_UNKNOWN",
    message: "blob upload unknown to registry",
};

/// A blob the manifest references is not in the registry.
const MANIFEST_BLOB_UNKNOWN: RegistryRefusal = RegistryRefusal {
    status: 404,
    code: "MANIFEST_BLOB_UNKNOWN",
    message: "blob unknown to manifest",
};

/// The manifest is not one this registry can store: not JSON, not an object, no
/// media type, nothing it references, or a reference that is not a tag.
const MANIFEST_INVALID: RegistryRefusal = RegistryRefusal {
    status: 400,
    code: "MANIFEST_INVALID",
    message: "manifest invalid",
};

/// The registry holds no manifest under that reference.
const MANIFEST_UNKNOWN: RegistryRefusal = RegistryRefusal {
    status: 404,
    code: "MANIFEST_UNKNOWN",
    message: "manifest unknown",
};

/// The repository name is not one the registry API's grammar admits.
const NAME_INVALID: RegistryRefusal = RegistryRefusal {
    status: 400,
    code: "NAME_INVALID",
    message: "invalid repository name",
};

/// Nothing has been pushed to that repository.
const NAME_UNKNOWN: RegistryRefusal = RegistryRefusal {
    status: 404,
    code: "NAME_UNKNOWN",
    message: "repository name not known to registry",
};

/// The path or the method is not one of this registry's API.
const UNSUPPORTED: RegistryRefusal = RegistryRefusal {
    status: 405,
    code: "UNSUPPORTED",
    message: "the operation is unsupported",
};

/// What this registry says when its backing store cannot be reached at all — the
/// engine is not the shared in-memory one.
const STORE_UNAVAILABLE: RegistryRefusal = RegistryRefusal {
    status: 503,
    code: "UNAVAILABLE",
    message: "the Lightsail container image registry store is unavailable",
};

/// What it says when the request body could not be read in memory at all. The
/// router buffers every request before dispatch, so a body it cannot hand back
/// is a failure of the server rather than a caller's malformed push.
const UNREADABLE_BODY: RegistryRefusal = RegistryRefusal {
    status: 500,
    code: "UNAVAILABLE",
    message: "the server could not read the pushed content",
};

/// Which part of the registry API a request target names.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Endpoint {
    /// `/v2/` — the version check.
    ApiVersion,
    /// `/v2/<name>/blobs/<digest>`.
    Blob { repository: String, digest: String },
    /// `/v2/<name>/blobs/uploads/`.
    BlobUploads { repository: String },
    /// `/v2/<name>/blobs/uploads/<session>`.
    BlobUpload {
        repository: String,
        session: String,
    },
    /// `/v2/<name>/manifests/<reference>`.
    Manifest {
        repository: String,
        reference: String,
    },
    /// `/v2/<name>/tags/list`.
    TagList { repository: String },
}

/// Serve one request to the container image registry of the account.
///
/// `target` is the registry the announced address names and the request target
/// the client appended beneath it (`crate::state::parse_container_image_registry_path`).
/// The caller has already authenticated the request against the log in
/// credentials this account was handed, so what is left is the registry API
/// itself.
pub fn serve<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &RtRequest,
    target: &ContainerImageRegistryTarget,
) -> RtResponse {
    let method = req.method();
    let Some(endpoint) = parse_endpoint(&target.path) else {
        return refuse(
            method,
            UNSUPPORTED,
            Some("this registry's API is served under /v2/".to_string()),
        );
    };
    if let Some(repository) = endpoint_repository(&endpoint) {
        if !is_repository_name(repository) {
            return refuse(
                method,
                NAME_INVALID,
                Some(format!("{repository:?} is not a repository name")),
            );
        }
    }
    let base = format!(
        "{}/{}",
        state::CONTAINER_IMAGE_REGISTRY_PATH_PREFIX, target.region
    );
    let region = target.region.as_str();
    match (&endpoint, method) {
        (Endpoint::ApiVersion, "GET" | "HEAD") => api_version(method),
        (Endpoint::Blob { digest, .. }, "GET" | "HEAD") => {
            read_blob(engine, method, region, digest)
        }
        (Endpoint::BlobUploads { repository }, "POST") => {
            start_or_complete_upload(engine, req, region, &base, repository)
        }
        (
            Endpoint::BlobUpload {
                repository,
                session,
            },
            "PATCH",
        ) => append_to_upload(engine, req, region, &base, repository, session),
        (
            Endpoint::BlobUpload {
                repository,
                session,
            },
            "PUT",
        ) => finish_upload(engine, req, region, &base, repository, session),
        (Endpoint::BlobUpload {
            repository,
            session,
        }, "GET") => upload_status(engine, method, region, &base, repository, session),
        (Endpoint::BlobUpload {
            repository,
            session,
        }, "DELETE") => cancel_upload(engine, method, region, repository, session),
        (
            Endpoint::Manifest {
                repository,
                reference,
            },
            "PUT",
        ) => put_manifest(engine, req, region, &base, repository, reference),
        (
            Endpoint::Manifest {
                repository,
                reference,
            },
            "GET" | "HEAD",
        ) => read_manifest(engine, method, region, repository, reference),
        (Endpoint::TagList { repository }, "GET" | "HEAD") => {
            list_tags(engine, method, region, repository)
        }
        _ => refuse(
            method,
            UNSUPPORTED,
            Some(format!("{method} is not served on this path")),
        ),
    }
}

/// Which part of the API the request target `path` names, or `None` when it
/// names none — the announced address itself, or anything that is not under the
/// API's own `/v2` space.
fn parse_endpoint(path: &str) -> Option<Endpoint> {
    let rest = match path.strip_prefix("/v2") {
        Some("") => return Some(Endpoint::ApiVersion),
        // Only a `/` may follow `/v2`: `/v2x` is a different path space.
        Some(rest) => rest.strip_prefix('/')?,
        None => return None,
    };
    if rest.is_empty() {
        return Some(Endpoint::ApiVersion);
    }
    let segments: Vec<&str> = rest.split('/').collect();
    // A repository name may itself carry `/`, so the name is everything BEFORE
    // the last `blobs`, `manifests` or `tags` segment — the segment that says
    // which part of the API the rest of the path is about.
    let kind_at = segments
        .iter()
        .rposition(|segment| matches!(*segment, "blobs" | "manifests" | "tags"))?;
    if kind_at == 0 {
        // Nothing before it: no repository is named at all.
        return None;
    }
    let repository = segments.get(..kind_at)?.join("/");
    let tail = segments.get(kind_at + 1..)?;
    match (*segments.get(kind_at)?, tail) {
        ("blobs", ["uploads"] | ["uploads", ""]) => Some(Endpoint::BlobUploads { repository }),
        ("blobs", ["uploads", session]) => Some(Endpoint::BlobUpload {
            repository,
            session: (*session).to_string(),
        }),
        ("blobs", [digest]) if !digest.is_empty() => Some(Endpoint::Blob {
            repository,
            digest: (*digest).to_string(),
        }),
        ("manifests", [reference]) if !reference.is_empty() => Some(Endpoint::Manifest {
            repository,
            reference: (*reference).to_string(),
        }),
        ("tags", ["list"]) => Some(Endpoint::TagList { repository }),
        _ => None,
    }
}

/// The repository name an endpoint is about, where it is about one.
fn endpoint_repository(endpoint: &Endpoint) -> Option<&str> {
    match endpoint {
        Endpoint::ApiVersion => None,
        Endpoint::Blob { repository, .. }
        | Endpoint::BlobUploads { repository }
        | Endpoint::BlobUpload { repository, .. }
        | Endpoint::Manifest { repository, .. }
        | Endpoint::TagList { repository } => Some(repository),
    }
}

/// Whether `name` is a repository name the registry API's grammar admits: one or
/// more `/`-separated components, each a run of lower-case alphanumerics with
/// `.`, `_` and `-` allowed between them, and no more than
/// [`MAX_REPOSITORY_NAME_LEN`] characters in all.
fn is_repository_name(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_REPOSITORY_NAME_LEN {
        return false;
    }
    name.split('/').all(|component| {
        let mut bytes = component.bytes();
        let starts = bytes.next().is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit());
        starts
            && component
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-'))
            && component
                .bytes()
                .next_back()
                .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    })
}

/// Whether `tag` is a tag the registry API's grammar admits: it starts with an
/// alphanumeric or `_`, carries only those plus `.` and `-`, and is no longer
/// than [`MAX_TAG_LEN`] characters.
fn is_tag(tag: &str) -> bool {
    if tag.is_empty() || tag.len() > MAX_TAG_LEN {
        return false;
    }
    let starts = tag
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    starts
        && tag
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

/// The version check: a `200` is what tells a client this address is a registry
/// speaking version 2 of the API.
fn api_version(method: &str) -> RtResponse {
    let mut resp = body_response(method, 200, b"{}".to_vec(), JSON_CONTENT_TYPE);
    set_header(&mut resp, API_VERSION_HEADER, API_VERSION);
    resp
}

/// Read one blob back.
///
/// The registry is the ACCOUNT's and content-addressed, so the digest names the
/// content whichever repository path addresses it — which is also what lets a
/// client mount a blob it pushed once into another repository rather than
/// sending it again.
fn read_blob<Eng: Send + Sync + 'static>(
    engine: &Eng,
    method: &str,
    region: &str,
    digest: &str,
) -> RtResponse {
    if !state::is_registry_digest(digest) {
        return refuse(
            method,
            DIGEST_INVALID,
            Some(format!("{digest:?} is not a digest this registry addresses content by")),
        );
    }
    // A `HEAD` carries the metadata of what a `GET` would answer and not the
    // content, so the content is not copied out of the store for one.
    let wants_content = method != "HEAD";
    let Some(found) = state::with_state(engine, |store| {
        store.image_registry(region).and_then(|registry| {
            registry.blob(digest).map(|blob| {
                (
                    blob.content.len(),
                    wants_content.then(|| blob.content.clone()),
                )
            })
        })
    }) else {
        return refuse(method, STORE_UNAVAILABLE, None);
    };
    let Some((length, content)) = found else {
        return refuse(method, BLOB_UNKNOWN, None);
    };
    let mut resp = RtResponse::new(
        status(200),
        content.map_or_else(SdkBody::empty, SdkBody::from),
    );
    set_header(&mut resp, "content-type", BLOB_CONTENT_TYPE);
    set_header(&mut resp, "content-length", length.to_string());
    set_header(&mut resp, CONTENT_DIGEST_HEADER, digest.to_string());
    resp
}

/// Start a blob upload, or take a whole blob in this one request when it carries
/// the digest of what it is sending.
fn start_or_complete_upload<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &RtRequest,
    region: &str,
    base: &str,
    repository: &str,
) -> RtResponse {
    let method = req.method();
    if let Some(claimed) = query_value(req.uri(), "digest") {
        // One request carrying both the digest and the bytes: the whole blob in
        // a single push.
        return match pushed_content(req, BLOB_UPLOAD_INVALID) {
            Ok(content) => store_blob(engine, method, region, base, repository, &claimed, content),
            Err(refusal) => *refusal,
        };
    }
    let Some(session) = state::with_state(engine, |store| {
        store.start_blob_upload(region, repository, state::now_ms())
    }) else {
        return refuse(method, STORE_UNAVAILABLE, None);
    };
    accepted_upload(method, base, repository, &session, 0)
}

/// Take more bytes into an upload session.
///
/// A client sending chunks says where each one goes with `Content-Range`, so one
/// that does not carry on from where the session got to is refused rather than
/// stitched into the wrong place — the digest of the whole blob is what the
/// finishing request is checked against, and bytes taken out of order would make
/// it fail for a client that sent every chunk correctly.
///
/// The number of sessions one region keeps is bounded where they are started
/// (`crate::state::StoredImageRegistry::start_upload`), and what one session
/// takes by this registry's byte bounds.
fn append_to_upload<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &RtRequest,
    region: &str,
    base: &str,
    repository: &str,
    session: &str,
) -> RtResponse {
    let method = req.method();
    let content = match pushed_content(req, BLOB_UPLOAD_INVALID) {
        Ok(content) => content,
        Err(refusal) => return *refusal,
    };
    let Some(held) = state::with_state(engine, |store| {
        store
            .image_registry(region)
            .and_then(|registry| registry.upload(session, repository))
            .map(|upload| upload.content.len())
    }) else {
        return refuse(method, STORE_UNAVAILABLE, None);
    };
    let Some(held) = held else {
        return refuse(method, BLOB_UPLOAD_UNKNOWN, None);
    };
    if let Some(start) = chunk_start(req) {
        if start != held {
            return refuse(
                method,
                out_of_order_chunk(),
                Some(format!(
                    "this upload holds {held} bytes, so the next chunk starts there"
                )),
            );
        }
    }
    let Some(outcome) = state::with_state(engine, |store| {
        store
            .image_registry_mut(region)
            .append_to_upload(session, repository, &content)
    }) else {
        return refuse(method, STORE_UNAVAILABLE, None);
    };
    match outcome {
        UploadAppend::Appended { held } => {
            accepted_upload(method, base, repository, session, held)
        }
        UploadAppend::Unknown => refuse(method, BLOB_UPLOAD_UNKNOWN, None),
        UploadAppend::OutOfSpace { limit } => {
            refuse(method, too_large(BLOB_UPLOAD_INVALID), Some(over_budget(limit)))
        }
    }
}

/// Finish an upload session: the last bytes, if any, and the digest of the whole
/// blob.
fn finish_upload<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &RtRequest,
    region: &str,
    base: &str,
    repository: &str,
    session: &str,
) -> RtResponse {
    let method = req.method();
    let Some(claimed) = query_value(req.uri(), "digest") else {
        return refuse(
            method,
            BLOB_UPLOAD_INVALID,
            Some("the request finishing an upload names the digest of the blob it sent".to_string()),
        );
    };
    let content = match pushed_content(req, BLOB_UPLOAD_INVALID) {
        Ok(content) => content,
        Err(refusal) => return *refusal,
    };
    // The session's own bytes come first, with the ones this request carries
    // after them: a client may send the last chunk on the request that finishes
    // the upload. The digest decides, and it is taken over both in one go, so a
    // claimed digest that is not the content's stores nothing and leaves the
    // session as it was.
    let Some(outcome) = state::with_state(engine, |store| {
        store.image_registry_mut(region).finish_upload(
            session,
            repository,
            &content,
            &claimed,
            state::now_ms(),
        )
    }) else {
        return refuse(method, STORE_UNAVAILABLE, None);
    };
    match outcome {
        UploadFinish::Stored(digest) => {
            // A push CHANGES what this registry holds, so it leaves a
            // structured record: what was stored, where, and how much of it.
            // The content itself is never logged.
            ::tracing::info!(
                audit = true,
                operation = "ContainerImageRegistryPush",
                outcome = "stored",
                region = %region,
                repository = %repository,
                content = "blob",
                digest = %digest,
                "container image registry push"
            );
            stored_blob(method, base, repository, &digest)
        }
        UploadFinish::DigestMismatch { computed } => refuse(
            method,
            DIGEST_INVALID,
            Some(format!(
                "the content sent has digest {computed}, not the {claimed} the request named"
            )),
        ),
        UploadFinish::Unknown => refuse(method, BLOB_UPLOAD_UNKNOWN, None),
        UploadFinish::OutOfSpace { limit } => {
            refuse(method, too_large(BLOB_UPLOAD_INVALID), Some(over_budget(limit)))
        }
    }
}

/// The answer to a blob the registry holds under `digest`: its own address and
/// that digest.
fn stored_blob(method: &str, base: &str, repository: &str, digest: &str) -> RtResponse {
    let mut resp = body_response(method, 201, Vec::new(), BLOB_CONTENT_TYPE);
    set_header(
        &mut resp,
        "location",
        format!("{base}/v2/{repository}/blobs/{digest}"),
    );
    set_header(&mut resp, CONTENT_DIGEST_HEADER, digest.to_string());
    resp
}

/// Hold `content` as a blob of the registry, having checked that `claimed` is
/// the digest of exactly those bytes.
fn store_blob<Eng: Send + Sync + 'static>(
    engine: &Eng,
    method: &str,
    region: &str,
    base: &str,
    repository: &str,
    claimed: &str,
    content: Vec<u8>,
) -> RtResponse {
    let computed = state::registry_digest(&content);
    if claimed != computed {
        // The digest of what arrived is the authority: content is stored under
        // the digest of its own bytes, never under one a client asserted.
        return refuse(
            method,
            DIGEST_INVALID,
            Some(format!(
                "the content sent has digest {computed}, not the {claimed} the request named"
            )),
        );
    }
    let stored_bytes = content.len();
    let Some(outcome) = state::with_state(engine, |store| {
        store
            .image_registry_mut(region)
            .put_blob(content, state::now_ms())
    }) else {
        return refuse(method, STORE_UNAVAILABLE, None);
    };
    match outcome {
        RegistryWrite::Stored(digest) => {
            // A push CHANGES what this registry holds, so it leaves a
            // structured record: what was stored, where, and how much of it.
            // The content itself is never logged.
            ::tracing::info!(
                audit = true,
                operation = "ContainerImageRegistryPush",
                outcome = "stored",
                region = %region,
                repository = %repository,
                content = "blob",
                digest = %digest,
                bytes = stored_bytes,
                "container image registry push"
            );
            stored_blob(method, base, repository, &digest)
        }
        RegistryWrite::OutOfSpace { limit } => {
            refuse(method, too_large(BLOB_UPLOAD_INVALID), Some(over_budget(limit)))
        }
    }
}

/// How far an upload session has got.
fn upload_status<Eng: Send + Sync + 'static>(
    engine: &Eng,
    method: &str,
    region: &str,
    base: &str,
    repository: &str,
    session: &str,
) -> RtResponse {
    let Some(held) = state::with_state(engine, |store| {
        store
            .image_registry(region)
            .and_then(|registry| registry.upload(session, repository))
            .map(|upload| upload.content.len())
    }) else {
        return refuse(method, STORE_UNAVAILABLE, None);
    };
    let Some(held) = held else {
        return refuse(method, BLOB_UPLOAD_UNKNOWN, None);
    };
    accepted_upload(method, base, repository, session, held)
}

/// Cancel an upload session, dropping whatever it had taken so far.
fn cancel_upload<Eng: Send + Sync + 'static>(
    engine: &Eng,
    method: &str,
    region: &str,
    repository: &str,
    session: &str,
) -> RtResponse {
    let Some(cancelled) = state::with_state(engine, |store| {
        store
            .image_registry_mut(region)
            .take_upload(session, repository)
            .is_some()
    }) else {
        return refuse(method, STORE_UNAVAILABLE, None);
    };
    if !cancelled {
        return refuse(method, BLOB_UPLOAD_UNKNOWN, None);
    }
    body_response(method, 204, Vec::new(), JSON_CONTENT_TYPE)
}

/// Push one image manifest, which is what completes an image: its digest is the
/// digest of these exact bytes, and it is the digest a push reports and
/// `RegisterContainerImage` takes.
fn put_manifest<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &RtRequest,
    region: &str,
    base: &str,
    repository: &str,
    reference: &str,
) -> RtResponse {
    let method = req.method();
    let content = match pushed_content(req, MANIFEST_INVALID) {
        Ok(content) => content,
        Err(refusal) => return *refusal,
    };
    if content.is_empty() {
        return refuse(
            method,
            MANIFEST_INVALID,
            Some("the request carries no manifest".to_string()),
        );
    }
    let Ok(document) = ::serde_json::from_slice::<::serde_json::Value>(&content) else {
        return refuse(
            method,
            MANIFEST_INVALID,
            Some("the manifest is not a JSON document".to_string()),
        );
    };
    let Some(manifest) = document.as_object() else {
        return refuse(
            method,
            MANIFEST_INVALID,
            Some("the manifest is not a JSON object".to_string()),
        );
    };
    // The media type a read of these bytes answers with: what the push declared
    // for them, which the manifest itself may also carry — and which has to be
    // one of the manifest media types this registry stores, so the value held
    // beside a manifest is one of the constants below and never caller text.
    let declared = req
        .headers()
        .get(::http::header::CONTENT_TYPE.as_str())
        .or_else(|| {
            manifest
                .get("mediaType")
                .and_then(::serde_json::Value::as_str)
        });
    let Some(media_type) = declared.and_then(manifest_media_type) else {
        return refuse(
            method,
            MANIFEST_INVALID,
            Some("the manifest declares no media type this registry stores".to_string()),
        );
    };

    // The digest of the bytes themselves, which is the identity of the image.
    let digest = state::registry_digest(&content);
    // A reference that IS a digest must be the digest of what arrived; anything
    // else is a tag, which has its own grammar.
    let tag = if state::is_registry_digest(reference) {
        if reference != digest {
            return refuse(
                method,
                DIGEST_INVALID,
                Some(format!(
                    "the manifest sent has digest {digest}, not the {reference} the path named"
                )),
            );
        }
        None
    } else if is_tag(reference) {
        Some(reference)
    } else {
        return refuse(
            method,
            MANIFEST_INVALID,
            Some(format!("{reference:?} is neither a digest nor a tag")),
        );
    };

    // What the manifest references has to be in the registry already: an image
    // manifest names its configuration and its layers as blobs, and an index
    // names other manifests. A manifest naming content nothing pushed describes
    // an image this registry does not hold.
    let referenced = match referenced_content(manifest) {
        Ok(referenced) => referenced,
        Err(detail) => return refuse(method, MANIFEST_INVALID, Some(detail)),
    };
    let Some(missing) = state::with_state(engine, |store| {
        let registry = store.image_registry(region);
        referenced
            .iter()
            .find(|reference| match (registry, reference) {
                // A region nothing has been pushed to holds none of it.
                (None, _) => true,
                (Some(registry), Reference::Blob(digest)) => !registry.has_blob(digest),
                (Some(registry), Reference::Manifest(digest)) => !registry.has_manifest(digest),
            })
            .cloned()
    }) else {
        return refuse(method, STORE_UNAVAILABLE, None);
    };
    if let Some(missing) = missing {
        return refuse(
            method,
            MANIFEST_BLOB_UNKNOWN,
            Some(format!("{} is not in this registry", missing.digest())),
        );
    }

    let stored_bytes = content.len();
    let Some(outcome) = state::with_state(engine, |store| {
        store.image_registry_mut(region).put_manifest(
            repository,
            tag,
            content,
            media_type,
            state::now_ms(),
        )
    }) else {
        return refuse(method, STORE_UNAVAILABLE, None);
    };
    match outcome {
        RegistryWrite::Stored(digest) => {
            // The manifest is what completes an image, so its push is recorded
            // the same way a blob's is.
            ::tracing::info!(
                audit = true,
                operation = "ContainerImageRegistryPush",
                outcome = "stored",
                region = %region,
                repository = %repository,
                content = "manifest",
                digest = %digest,
                bytes = stored_bytes,
                "container image registry push"
            );
            let mut resp = body_response(method, 201, Vec::new(), JSON_CONTENT_TYPE);
            set_header(
                &mut resp,
                "location",
                format!("{base}/v2/{repository}/manifests/{digest}"),
            );
            set_header(&mut resp, CONTENT_DIGEST_HEADER, digest);
            resp
        }
        RegistryWrite::OutOfSpace { limit } => {
            refuse(method, too_large(MANIFEST_INVALID), Some(over_budget(limit)))
        }
    }
}

/// Read one manifest back, under a tag or under its digest.
fn read_manifest<Eng: Send + Sync + 'static>(
    engine: &Eng,
    method: &str,
    region: &str,
    repository: &str,
    reference: &str,
) -> RtResponse {
    let wants_content = method != "HEAD";
    let Some(found) = state::with_state(engine, |store| {
        store.image_registry(region).and_then(|registry| {
            let digest = registry.resolve_reference(repository, reference)?;
            let manifest = registry.manifest(repository, reference)?;
            Some((
                digest,
                manifest.media_type.clone(),
                manifest.content.len(),
                wants_content.then(|| manifest.content.clone()),
            ))
        })
    }) else {
        return refuse(method, STORE_UNAVAILABLE, None);
    };
    let Some((digest, media_type, length, content)) = found else {
        return refuse(method, MANIFEST_UNKNOWN, None);
    };
    let mut resp = RtResponse::new(
        status(200),
        content.map_or_else(SdkBody::empty, SdkBody::from),
    );
    set_header(&mut resp, "content-type", media_type);
    set_header(&mut resp, "content-length", length.to_string());
    set_header(&mut resp, CONTENT_DIGEST_HEADER, digest);
    resp
}

/// The tags of one repository.
fn list_tags<Eng: Send + Sync + 'static>(
    engine: &Eng,
    method: &str,
    region: &str,
    repository: &str,
) -> RtResponse {
    let Some(found) = state::with_state(engine, |store| {
        store.image_registry(region).and_then(|registry| {
            registry
                .holds_repository(repository)
                .then(|| registry.tags_of(repository))
        })
    }) else {
        return refuse(method, STORE_UNAVAILABLE, None);
    };
    let Some(tags) = found else {
        return refuse(method, NAME_UNKNOWN, None);
    };
    let body = ::serde_json::json!({ "name": repository, "tags": tags }).to_string();
    body_response(method, 200, body.into_bytes(), JSON_CONTENT_TYPE)
}

/// One piece of content a manifest references.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Reference {
    /// A blob: an image manifest's configuration or one of its layers.
    Blob(String),
    /// Another manifest: what an index references.
    Manifest(String),
}

impl Reference {
    /// The digest this reference names.
    fn digest(&self) -> &str {
        match self {
            Reference::Blob(digest) | Reference::Manifest(digest) => digest,
        }
    }
}

/// Everything `manifest` references, or why it references nothing this registry
/// can look for.
fn referenced_content(
    manifest: &::serde_json::Map<String, ::serde_json::Value>,
) -> Result<Vec<Reference>, String> {
    let mut referenced = Vec::new();
    // An index or a manifest list references other manifests.
    if let Some(manifests) = manifest.get("manifests") {
        let entries = manifests
            .as_array()
            .ok_or_else(|| "the manifest's manifests member is not a list".to_string())?;
        for entry in entries {
            referenced.push(Reference::Manifest(digest_member(entry, "a referenced manifest")?));
        }
        return Ok(referenced);
    }
    // An image manifest references its configuration and its layers.
    if let Some(config) = manifest.get("config") {
        referenced.push(Reference::Blob(digest_member(config, "the configuration")?));
    }
    if let Some(layers) = manifest.get("layers") {
        let entries = layers
            .as_array()
            .ok_or_else(|| "the manifest's layers member is not a list".to_string())?;
        for entry in entries {
            referenced.push(Reference::Blob(digest_member(entry, "a layer")?));
        }
    }
    if referenced.is_empty() {
        return Err("the manifest references no content".to_string());
    }
    Ok(referenced)
}

/// The `digest` member of one entry of a manifest, which must be a digest this
/// registry addresses content by. `what` names the entry in the refusal.
fn digest_member(entry: &::serde_json::Value, what: &str) -> Result<String, String> {
    let digest = entry
        .get("digest")
        .and_then(::serde_json::Value::as_str)
        .ok_or_else(|| format!("{what} names no digest"))?;
    if !state::is_registry_digest(digest) {
        return Err(format!(
            "{what} names {digest:?}, which is not a digest this registry addresses content by"
        ));
    }
    Ok(digest.to_string())
}

/// The content one request pushed, or the refusal to answer instead.
///
/// The DECLARED length is judged before the body is touched, so content past
/// this registry's per-blob bound is refused without being copied. `kind` is the
/// refusal for the content the request is pushing, so a refusal says whether it
/// was about a blob or a manifest.
///
/// The refusal is BOXED: a whole response beside the content in one `Result`
/// would make every caller of this carry a value the size of a response.
fn pushed_content(req: &RtRequest, kind: RegistryRefusal) -> Result<Vec<u8>, Box<RtResponse>> {
    let method = req.method();
    if let Some(declared) = declared_length(req) {
        if declared > MAX_REGISTRY_BLOB_BYTES as u64 {
            return Err(Box::new(refuse(
                method,
                too_large(kind),
                Some(over_budget(MAX_REGISTRY_BLOB_BYTES)),
            )));
        }
    }
    let Some(body) = req.body().bytes() else {
        return Err(Box::new(refuse(method, UNREADABLE_BODY, None)));
    };
    if body.len() > MAX_REGISTRY_BLOB_BYTES {
        return Err(Box::new(refuse(
            method,
            too_large(kind),
            Some(over_budget(MAX_REGISTRY_BLOB_BYTES)),
        )));
    }
    Ok(body.to_vec())
}

/// The length the request declares for what it is sending, when it declares one
/// this registry can read as a byte count. A header is caller-controlled text,
/// so a value that is not a plain byte count is simply not a declaration and the
/// body itself is judged instead.
fn declared_length(req: &RtRequest) -> Option<u64> {
    req.headers()
        .get(::http::header::CONTENT_LENGTH.as_str())
        .and_then(|value| value.trim().parse::<u64>().ok())
}

/// The answer to an upload session that is open and has taken `held` bytes: the
/// session's own address, its id, and how far it has got.
fn accepted_upload(
    method: &str,
    base: &str,
    repository: &str,
    session: &str,
    held: usize,
) -> RtResponse {
    let mut resp = body_response(method, 202, Vec::new(), JSON_CONTENT_TYPE);
    set_header(
        &mut resp,
        "location",
        format!("{base}/v2/{repository}/blobs/uploads/{session}"),
    );
    set_header(&mut resp, UPLOAD_SESSION_HEADER, session.to_string());
    // The range of what the session holds, which is what a client resuming one
    // reads to know where to carry on from. An empty session holds the first
    // byte position and nothing before it.
    set_header(&mut resp, "range", format!("0-{}", held.saturating_sub(1)));
    resp
}

/// The refusal of a chunk that does not carry on from where the session got to.
///
/// `416` with the registry API's `BLOB_UPLOAD_INVALID`: the range the chunk
/// names is not one this upload can take, which is what that status says.
fn out_of_order_chunk() -> RegistryRefusal {
    RegistryRefusal {
        status: 416,
        code: BLOB_UPLOAD_INVALID.code,
        message: BLOB_UPLOAD_INVALID.message,
    }
}

/// The byte position a chunked upload says its content starts at, when the
/// request declares one this registry can read as `<start>-<end>`.
///
/// A header is caller-controlled text, so anything else is simply not a
/// declaration and the chunk is taken where the session stands.
fn chunk_start(req: &RtRequest) -> Option<usize> {
    let value = req.headers().get("content-range")?;
    let (start, _) = value.trim().split_once('-')?;
    start.trim().parse::<usize>().ok()
}

/// The refusal of content past one of this registry's byte bounds.
///
/// `413` rather than one of the registry API's own 4xx codes for size: the
/// request is well formed and the registry simply will not hold that much, which
/// is what that status says. The CODE is the one for the kind of content that
/// was refused — `of` — so the envelope still says what the request was about.
fn too_large(of: RegistryRefusal) -> RegistryRefusal {
    RegistryRefusal {
        status: 413,
        code: of.code,
        message: of.message,
    }
}

/// The detail of a refusal for passing the byte bound `limit`.
fn over_budget(limit: usize) -> String {
    format!("this registry holds at most {limit} bytes of that content")
}

/// One refusal of the registry API, in that API's own error envelope.
///
/// A `HEAD` carries the metadata of what a `GET` of the same address would
/// answer — its status and its length — without the body, which is what the
/// method means.
fn refuse(method: &str, refusal: RegistryRefusal, detail: Option<String>) -> RtResponse {
    let mut error = ::serde_json::json!({
        "code": refusal.code,
        "message": refusal.message,
    });
    if let (Some(detail), Some(object)) = (detail, error.as_object_mut()) {
        object.insert("detail".to_string(), ::serde_json::Value::String(detail));
    }
    let body = ::serde_json::json!({ "errors": [error] }).to_string();
    body_response(
        method,
        refusal.status,
        body.into_bytes(),
        JSON_CONTENT_TYPE,
    )
}

/// One response of this plane carrying `body`, with the length of that body
/// reported whether or not the method takes one.
fn body_response(method: &str, code: u16, body: Vec<u8>, content_type: &str) -> RtResponse {
    let length = body.len();
    let body = if method == "HEAD" {
        SdkBody::empty()
    } else {
        SdkBody::from(body)
    };
    let mut resp = RtResponse::new(status(code), body);
    set_header(&mut resp, "content-type", content_type.to_string());
    set_header(&mut resp, "content-length", length.to_string());
    resp
}

/// Put `value` on `resp` as the header `name`.
///
/// The FALLIBLE insert, never the panicking one. A response of this plane
/// carries values that came from a request or from what a request stored — a
/// repository name, a digest, an upload session, a manifest's media type — and a
/// value the HTTP layer will not take is left OFF the response, so no request
/// can turn a header into an unwound task. The name is a constant of this
/// module.
fn set_header(resp: &mut RtResponse, name: &'static str, value: impl Into<String>) {
    if resp.headers_mut().try_insert(name, value.into()).is_err() {
        ::tracing::debug!(
            header = name,
            "a container image registry response header was not a value the HTTP layer takes, \
             and was left off the response"
        );
    }
}

/// One status code of this module, which is a constant of it.
fn status(code: u16) -> StatusCode {
    StatusCode::try_from(code).expect("the status is a constant of this module")
}

/// The value of the query parameter `name` of `uri`, percent-decoded with the
/// framework's own query decoder.
fn query_value(uri: &str, name: &str) -> Option<String> {
    let (_, query) = uri.split_once('?')?;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then(|| ::smithy_server_runtime::percent_decode(value))
    })
}
