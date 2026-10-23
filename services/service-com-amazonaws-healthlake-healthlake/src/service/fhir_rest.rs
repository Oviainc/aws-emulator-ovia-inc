// SPDX-License-Identifier: CC0-1.0

//! The data store's FHIR REST data plane: the wire
//! `DatastoreProperties$DatastoreEndpoint` advertises.
//!
//! # Not modelled operations
//!
//! `smithy_query(query="service_info")` reports this service as `awsJson1_0`, and its
//! 26 modelled operations are all control plane: they create, describe, tag and
//! delete data stores. The FHIR interactions a caller then performs on the endpoint a
//! data store reports — `POST <DatastoreEndpoint>Patient`,
//! `GET <DatastoreEndpoint>Patient/{id}` — are not modelled operations and carry no
//! `X-Amz-Target`: they are the FHIR REST API, addressed by path and HTTP method, on
//! the same signed `healthlake` wire. Nothing in the generated routing table can name
//! them, so they are reached through
//! [`crate::service::EmulatedService::override_operation`], the one seam a service
//! has for a request the model's routing key cannot name.
//!
//! # What is served
//!
//! The two interactions the FHIR RESTful API defines over a resource's lifetime
//! start:
//!
//! * `create` — `POST [base]/{type}`, which assigns the logical id (FHIR requires the
//!   server to assign one and to IGNORE any `id` the submitted resource carries), and
//!   answers `201` with the stored resource and its `Location`;
//! * `read` — `GET [base]/{type}/{id}`, which answers `200` with the stored resource,
//!   byte-for-byte what the create stored, and `404` for an id the data store holds no
//!   resource under.
//!
//! Every other interaction on this path space (`update`, `delete`, `search`,
//! `vread`, the batch/transaction endpoint) is answered `501` with an
//! `OperationOutcome` of `not-supported`, which is this emulator's "not implemented
//! yet" for a FHIR wire — never an empty `200` that would look like a successful
//! interaction nothing performed. Each is its own gap, to be implemented against its
//! own recorded behaviour.
//!
//! # The error envelope
//!
//! A FHIR endpoint states its failures as an `OperationOutcome` resource, not as the
//! control plane's awsJson error shape: a FHIR client reads the `issue[].code` to
//! decide what to do. [`outcome`] is the one builder of that body here.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};
use std::sync::Arc;
use std::time::SystemTime;

use super::non_rpc::{bounded, request_path};
use crate::state::FhirEndpointPath;

/// The media type a FHIR JSON representation travels as, which is what a FHIR client
/// sends and expects back (FHIR R4 §2.2 "Content Types").
pub const FHIR_JSON_CONTENT_TYPE: &str = "application/fhir+json";

/// The `OperationOutcome` issue code for a resource, or a resource type, the data
/// store does not hold.
const NOT_FOUND: &str = "not-found";

/// The issue code for a submitted resource whose content breaks a rule of the
/// interaction — the `resourceType` not matching the URL's type, say.
const INVALID: &str = "invalid";

/// The issue code for a body that could not be read as a FHIR JSON resource at all.
const STRUCTURE: &str = "structure";

/// The issue code for an interaction this endpoint does not implement.
const NOT_SUPPORTED: &str = "not-supported";

/// The issue code for an interaction refused because a bound on what this emulator
/// RETAINS was reached. FHIR's `too-costly` is "the operation was stopped to protect
/// server resources", which is what a retention ceiling is.
const TOO_COSTLY: &str = "too-costly";

/// The issue code for a submitted resource larger than this endpoint stores. FHIR's
/// `too-long` is "provided content is too long".
const TOO_LONG: &str = "too-long";

/// The issue code for a failure of the server itself.
const EXCEPTION: &str = "exception";

/// The longest resource type name this endpoint accepts in a path. FHIR resource type
/// names are short UpperCamelCase tokens; the bound keeps a long path segment out of
/// the store's keys and out of a message.
const MAX_RESOURCE_TYPE_LEN: usize = 64;

/// How many logical ids a create offers before it gives up.
///
/// The mint advances a counter on every call, so each candidate differs and one
/// attempt is enough in practice; the cap is what BOUNDS the walk, because it runs
/// while the per-service store lock is held and no single request may pin that lock.
const MAX_ID_MINT_ATTEMPTS: usize = 8;

/// The data store FHIR REST data plane, as the one [`ErasedOperation`] the service
/// holds for it.
///
/// Owns the shared engine exactly as a generated operation does, so the resources it
/// writes live in the SAME per-service store the control plane's handlers read: a
/// data store deleted through `DeleteFHIRDatastore` takes its FHIR resources with it,
/// because they are held on the data store's own record.
pub struct FhirRestDataPlane<Eng> {
    engine: Arc<Eng>,
}

impl<Eng> FhirRestDataPlane<Eng> {
    /// Build the data plane from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for FhirRestDataPlane<Eng> {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move { self.serve(req) })
    }
}

impl<Eng: Send + Sync + 'static> FhirRestDataPlane<Eng> {
    /// Serve one request on a data store's FHIR endpoint.
    ///
    /// The method and the shape of the interaction path together name the FHIR
    /// interaction, which is how the FHIR RESTful API is addressed — there is no
    /// operation name on this wire.
    fn serve(&self, req: &RtRequest) -> RtResponse {
        let path = request_path(req.uri());
        let Some(route) = crate::state::parse_datastore_fhir_path(path) else {
            // `override_operation` only routes a path this reader accepts, so this is
            // unreachable; answering the FHIR envelope rather than panicking keeps the
            // handler total.
            return outcome(
                404,
                NOT_FOUND,
                "The request path is not a data store FHIR endpoint path",
            );
        };
        let interaction: Vec<&str> = route.interaction.iter().map(String::as_str).collect();
        match (req.method(), interaction.as_slice()) {
            ("POST", [resource_type]) => match req.body().bytes() {
                Some(body) => self.create(&route, resource_type, body),
                // The router buffers every request before dispatch, so a body it
                // cannot hand back in memory is a server-side failure, never a
                // caller's malformed resource: answering 400 here would report a
                // document this handler never read.
                None => outcome(
                    500,
                    EXCEPTION,
                    "The server could not read the submitted resource from the request body",
                ),
            },
            ("GET", [resource_type, id]) => self.read(&route, resource_type, id),
            (method, _) => outcome(
                501,
                NOT_SUPPORTED,
                &format!(
                    "This data store endpoint implements the FHIR create (POST [base]/{{type}}) \
                     and read (GET [base]/{{type}}/{{id}}) interactions; {} {} is not \
                     implemented yet",
                    bounded(method),
                    bounded(&format!("[base]/{}", route.interaction.join("/")))
                ),
            ),
        }
    }

    /// The FHIR `create` interaction: store the submitted resource under a logical id
    /// this server assigns, and answer `201` with what was stored.
    fn create(&self, route: &FhirEndpointPath, resource_type: &str, body: &[u8]) -> RtResponse {
        if !is_resource_type_name(resource_type) {
            return unknown_resource_type(resource_type);
        }
        // The BYTES are bounded before the body is parsed, let alone stored: a count
        // ceiling alone would admit the count times whatever one body the transport
        // accepts, so the size of one resource is refused here and the retained totals
        // below.
        if body.len() > crate::state::MAX_FHIR_RESOURCE_BYTES {
            return outcome(
                413,
                TOO_LONG,
                &format!(
                    "A FHIR resource this endpoint stores is at most {} bytes; the submitted \
                     one is {} bytes",
                    crate::state::MAX_FHIR_RESOURCE_BYTES,
                    body.len()
                ),
            );
        }
        let submitted = match submitted_resource(body, resource_type) {
            Ok(submitted) => submitted,
            Err(failure) => return failure.response(),
        };
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return store_unavailable();
        };
        let mut store = state.lock();
        // Writing to a data store means first letting the registry catch up with the
        // clock: a lifecycle transition that was already due is materialised here, so
        // a store that has become ACTIVE serves, and one that has become DELETED does
        // not.
        store.settle_lifecycle();
        if let Err(failure) = serving(&store, &route.datastore_id) {
            return failure.response();
        }
        let held = store
            .datastores
            .get(&route.datastore_id)
            .map_or(0, |datastore| datastore.fhir_resources.len());
        if held >= crate::state::MAX_FHIR_RESOURCES_PER_DATASTORE {
            return outcome(
                507,
                TOO_COSTLY,
                &format!(
                    "This data store already holds the maximum number of FHIR resources this \
                     emulator retains ({})",
                    crate::state::MAX_FHIR_RESOURCES_PER_DATASTORE
                ),
            );
        }

        let now = SystemTime::now();
        let stamp = ::aws_smithy_types::DateTime::from(now);
        let Ok(last_updated) = stamp.fmt(::aws_smithy_types::date_time::Format::DateTime) else {
            return outcome(
                500,
                EXCEPTION,
                "The server could not render the resource's last-updated instant",
            );
        };
        // A minted id is checked against the data store before it is used, so a
        // create can never overwrite a stored resource. Each call advances the mint
        // counter, so a retry always offers a DIFFERENT candidate; the attempt cap
        // bounds the walk regardless, because this runs while the store lock is held
        // and no request may pin that lock.
        let mut minted = None;
        for _ in 0..MAX_ID_MINT_ATTEMPTS {
            let candidate = store.next_fhir_resource_id(now);
            let taken = store
                .datastores
                .get(&route.datastore_id)
                .is_some_and(|datastore| {
                    datastore
                        .fhir_resource(resource_type, &candidate)
                        .is_some()
                });
            if !taken {
                minted = Some(candidate);
                break;
            }
        }
        let Some(id) = minted else {
            return outcome(
                500,
                EXCEPTION,
                "The server could not assign an unused logical id for the resource",
            );
        };

        let version = 1;
        let content = stored_content(submitted, &id, version, &last_updated);
        // What this resource will COST the store, measured on the bytes actually
        // retained (the stored content, not the request), and charged against both
        // budgets with `saturating_add` before anything is written.
        let serialized = content.to_string();
        let bytes = serialized.len();
        // The pre-parse check above refused an over-large REQUEST; this one makes the
        // per-resource ceiling exact, because what is retained is the stored
        // serialisation (the submitted content with `id` and `meta` written on), which
        // is a little larger than the request.
        if bytes > crate::state::MAX_FHIR_RESOURCE_BYTES {
            return outcome(
                413,
                TOO_LONG,
                &format!(
                    "A FHIR resource this endpoint stores is at most {} bytes; the one this \
                     create would store is {bytes} bytes",
                    crate::state::MAX_FHIR_RESOURCE_BYTES
                ),
            );
        }
        let held_bytes = store
            .datastores
            .get(&route.datastore_id)
            .map_or(0, |datastore| datastore.fhir_resource_bytes);
        if held_bytes.saturating_add(bytes) > crate::state::MAX_FHIR_RESOURCE_BYTES_PER_DATASTORE {
            return outcome(
                507,
                TOO_COSTLY,
                &format!(
                    "This data store already retains as many FHIR resource bytes as this \
                     emulator holds for one data store ({})",
                    crate::state::MAX_FHIR_RESOURCE_BYTES_PER_DATASTORE
                ),
            );
        }
        if store
            .fhir_resource_bytes_total()
            .saturating_add(bytes)
            > crate::state::MAX_FHIR_RESOURCE_BYTES_TOTAL
        {
            return outcome(
                507,
                TOO_COSTLY,
                &format!(
                    "This emulator already retains as many FHIR resource bytes as it holds \
                     across every data store ({})",
                    crate::state::MAX_FHIR_RESOURCE_BYTES_TOTAL
                ),
            );
        }
        let resource = crate::state::FhirResource {
            resource_type: resource_type.to_string(),
            id: id.clone(),
            version,
            last_updated: stamp,
            content,
            bytes,
        };
        let Some(datastore) = store.datastores.get_mut(&route.datastore_id) else {
            // `serving` resolved the record under this same lock.
            return store_unavailable();
        };
        let location = format!(
            "{}{resource_type}/{id}/_history/{version}",
            datastore.endpoint
        );
        datastore.fhir_resource_bytes = datastore.fhir_resource_bytes.saturating_add(bytes);
        datastore
            .fhir_resources
            .insert((resource_type.to_string(), id), resource);
        let mut response = fhir_response(201, serialized);
        response.headers_mut().insert("location", location);
        response
            .headers_mut()
            .insert("etag", format!("W/\"{version}\""));
        response
    }

    /// The FHIR `read` interaction: answer the stored resource, exactly as the create
    /// stored it.
    fn read(&self, route: &FhirEndpointPath, resource_type: &str, id: &str) -> RtResponse {
        if !is_resource_type_name(resource_type) {
            return unknown_resource_type(resource_type);
        }
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return store_unavailable();
        };
        let mut store = state.lock();
        // Reporting a resource means reporting it from a data store whose own status
        // is up to date with the clock; the transition was owed at this instant
        // whether or not anybody read.
        store.settle_lifecycle();
        if let Err(failure) = serving(&store, &route.datastore_id) {
            return failure.response();
        }
        let found = store
            .datastores
            .get(&route.datastore_id)
            .and_then(|datastore| datastore.fhir_resource(resource_type, id));
        let Some(resource) = found else {
            return outcome(
                404,
                NOT_FOUND,
                &format!(
                    "This data store holds no {} with id '{}'",
                    bounded(resource_type),
                    bounded(id)
                ),
            );
        };
        let mut response = fhir_response(200, resource.content.to_string());
        response
            .headers_mut()
            .insert("etag", format!("W/\"{}\"", resource.version));
        response
    }
}

/// The submitted resource, read from the request body, or the refusal it earned.
///
/// Two rules of the FHIR create interaction, in the order a server can apply them: the
/// body must be a JSON object (a FHIR resource is one), and its `resourceType` must be
/// the type the URL names — a mismatch is a refusal rather than a silent re-filing,
/// because the URL and the body would otherwise disagree about what was created.
fn submitted_resource(body: &[u8], resource_type: &str) -> Result<::serde_json::Value, Failure> {
    if body.is_empty() {
        return Err(Failure::new(
            400,
            STRUCTURE,
            "The create interaction requires a FHIR resource in the request body".to_string(),
        ));
    }
    let parsed: ::serde_json::Value = ::serde_json::from_slice(body).map_err(|error| {
        Failure::new(
            400,
            STRUCTURE,
            format!(
                "The request body is not a FHIR JSON resource: {}",
                bounded(&error.to_string())
            ),
        )
    })?;
    if !parsed.is_object() {
        return Err(Failure::new(
            400,
            STRUCTURE,
            "A FHIR resource is a JSON object, and the request body is not one".to_string(),
        ));
    }
    match parsed
        .get("resourceType")
        .and_then(::serde_json::Value::as_str)
    {
        Some(submitted) if submitted == resource_type => Ok(parsed),
        Some(submitted) => Err(Failure::new(
            400,
            INVALID,
            format!(
                "The submitted resource's resourceType is '{}', but the request URL names \
                 '{}'",
                bounded(submitted),
                bounded(resource_type)
            ),
        )),
        None => Err(Failure::new(
            400,
            STRUCTURE,
            "A FHIR resource carries a resourceType, and the submitted one does not".to_string(),
        )),
    }
}

/// The submitted resource as it is STORED: its own content, with the two values the
/// server owns written onto it.
///
/// FHIR has the server assign the logical id and the version, so any `id`,
/// `meta.versionId` or `meta.lastUpdated` the request carried is replaced rather than
/// honoured. The REST of `meta` (a profile claim, a security label, a tag) is the
/// caller's and is kept.
fn stored_content(
    submitted: ::serde_json::Value,
    id: &str,
    version: u64,
    last_updated: &str,
) -> ::serde_json::Value {
    let mut content = submitted;
    let mut meta = content
        .get("meta")
        .filter(|meta| meta.is_object())
        .cloned()
        .unwrap_or_else(|| ::serde_json::json!({}));
    meta["versionId"] = ::serde_json::json!(version.to_string());
    meta["lastUpdated"] = ::serde_json::json!(last_updated);
    let object = content
        .as_object_mut()
        .expect("the caller checked the submitted resource is a JSON object");
    object.insert("id".to_string(), ::serde_json::json!(id));
    object.insert("meta".to_string(), meta);
    content
}

/// `Ok` when the data store `datastore_id` names is serving its FHIR endpoint.
///
/// A data store serves only from `ACTIVE`: while it is still `CREATING`, and once it
/// is `DELETING` or `DELETED`, its endpoint holds nothing a caller may read or write,
/// and an id no data store carries is the same answer — so a caller cannot tell a data
/// store that is not ready from one that does not exist by probing the data plane.
fn serving(
    store: &crate::state::ComAmazonawsHealthlakeHealthlakeState,
    datastore_id: &str,
) -> Result<(), Failure> {
    let active = store
        .datastores
        .get(datastore_id)
        .is_some_and(|datastore| datastore.status == crate::state::DatastoreStatus::Active);
    if active {
        return Ok(());
    }
    Err(Failure::new(
        404,
        NOT_FOUND,
        format!(
            "No ACTIVE data store serves the FHIR endpoint '{}'",
            bounded(datastore_id)
        ),
    ))
}

/// A FHIR failure a helper found, before it is rendered.
///
/// Carried instead of a built response so a helper that refuses a request answers in
/// the same words [`outcome`] writes for every other refusal, and so a `Result`'s
/// error variant stays small.
struct Failure {
    /// The HTTP status the refusal carries.
    status: u16,
    /// The FHIR `IssueType` a client branches on.
    code: &'static str,
    /// The sentence for a human.
    diagnostics: String,
}

impl Failure {
    /// One failure, from the three facts it states.
    fn new(status: u16, code: &'static str, diagnostics: String) -> Self {
        Self {
            status,
            code,
            diagnostics,
        }
    }

    /// This failure as the `OperationOutcome` response it answers with.
    fn response(self) -> RtResponse {
        outcome(self.status, self.code, &self.diagnostics)
    }
}

/// The refusal of a path segment that is not a FHIR resource type name.
fn unknown_resource_type(resource_type: &str) -> RtResponse {
    outcome(
        404,
        NOT_FOUND,
        &format!(
            "'{}' is not a FHIR resource type",
            bounded(resource_type)
        ),
    )
}

/// The answer when this service's store cannot be reached: a server fault, never a
/// fresh store.
fn store_unavailable() -> RtResponse {
    outcome(
        500,
        EXCEPTION,
        "The HealthLake data store registry is unavailable",
    )
}

/// Whether `segment` names a FHIR resource type.
///
/// FHIR resource type names are UpperCamelCase ASCII tokens (`Patient`,
/// `Observation`, `MedicationRequest`), so a segment that is not one names no type at
/// all. This emulator does not carry the R4 resource-type catalogue, so a WELL-FORMED
/// type name is served and the resource stored under it; which types a release
/// defines is its own gap, and a catalogue invented here would reject resource types
/// HealthLake accepts.
fn is_resource_type_name(segment: &str) -> bool {
    segment.len() <= MAX_RESOURCE_TYPE_LEN
        && segment.starts_with(|first: char| first.is_ascii_uppercase())
        && segment.chars().all(|c| c.is_ascii_alphanumeric())
}

/// One FHIR response: the status, the FHIR JSON media type, and the body.
fn fhir_response(status: u16, body: String) -> RtResponse {
    let mut response = RtResponse::new(
        StatusCode::try_from(status).expect("the status is a constant of this module"),
        SdkBody::from(body),
    );
    response
        .headers_mut()
        .insert("content-type", FHIR_JSON_CONTENT_TYPE);
    response
}

/// A FHIR failure, as the `OperationOutcome` resource a FHIR endpoint answers with.
///
/// `severity` follows the status: a server fault is `fatal`, everything a caller can
/// act on is `error` (FHIR R4 `IssueSeverity`). The `code` is the FHIR `IssueType` a
/// client branches on; the `diagnostics` sentence is for a human and carries only
/// bounded, control-character-free text.
fn outcome(status: u16, code: &str, diagnostics: &str) -> RtResponse {
    let severity = if status >= 500 { "fatal" } else { "error" };
    let body = ::serde_json::json!({
        "resourceType": "OperationOutcome",
        "issue": [{
            "severity": severity,
            "code": code,
            "diagnostics": diagnostics,
        }],
    });
    fhir_response(status, body.to_string())
}

