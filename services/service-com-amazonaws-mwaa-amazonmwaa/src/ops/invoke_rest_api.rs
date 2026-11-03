// SPDX-License-Identifier: CC0-1.0

//! @op InvokeRestApi
//!
//! Routes the call to the Apache Airflow REST API of the STORED environment the request
//! names, over that environment's own Apache Airflow metadata
//! ([`crate::state::AirflowMetadata`]), and reports what that API answered: the HTTP
//! status in `RestApiStatusCode` and the JSON document in `RestApiResponse`. A name that
//! resolves to nothing is refused with HTTP 404 and this operation's modelled
//! `ResourceNotFoundException`, worded by the environment-scoped data endpoint that fronts
//! it (`@endpoint` host prefix `env.`):
//! [`crate::state::ENVIRONMENT_ENDPOINT_NOT_FOUND_MESSAGE`] — and relayed to the client the
//! way that front door sent it, with the sentence as the whole body and the code in the
//! `x-amzn-errortype` header, because this operation reports what the endpoint it called
//! answered (`crate::service::env_endpoint`).
//!
//! `Method`, `Path`, `QueryParameters` and `Body` are all acted on: the path selects the
//! Apache Airflow resource, the method what is done to it, the query parameters page a
//! collection, and the body carries the entity a write creates or changes. Both documents
//! are the JSON objects the model documents them as, so one of any other shape names no
//! parameter and carries no entity, and is refused ([`object_document`]) rather than
//! dropped — each of them on every call, whatever method and path the call addresses,
//! since both documents belong to the request rather than to the endpoint it reaches.
//!
//! ## How an Apache Airflow answer becomes this operation's answer
//!
//! The operation is a PROXY, so the Apache Airflow status decides which of its modelled
//! answers the caller gets, by CLASS and not per endpoint:
//!
//! * `2xx` — the operation's own HTTP 200, carrying that status and that document.
//! * `4xx` — `RestApiClientException`, carrying the same pair.
//! * `5xx` — `RestApiServerException`, carrying the same pair.
//!
//! ## What this environment's Apache Airflow API serves
//!
//! Everything below is computed from the environment's stored state, and a write through
//! this operation is visible to the next read through it:
//!
//! * `GET /version` — the Apache Airflow version the stored environment runs.
//! * `/variables` and `/variables/{key}` — the variables in the environment's metadata
//!   database: list, create-or-replace, read, change and delete.
//! * `/pools` and `/pools/{name}` — its slot pools, seeded with Apache Airflow's own
//!   `default_pool`: list, create, read, change and delete.
//! * Neither collection holds a sub-resource, so the entity a path addresses is named by
//!   the WHOLE remainder of that path after the collection's own segment: a key that
//!   itself carries the path separator addresses the entity stored under it, and the
//!   writes refuse a key no such path can name — one no path splits into, and one the
//!   modelled `Path` is too short to carry under its collection — so nothing is stored
//!   that a later read, change or delete cannot reach.
//! * `GET /dags` — the environment's DAG inventory, which is EMPTY and whose
//!   `total_entries` is therefore 0, so every path under `/dags/{dag_id}` resolves to
//!   nothing: a DAG and its `dag_id` are defined by the Python DAG file the environment
//!   loads from Amazon S3, and knowing which DAGs a file defines means importing it,
//!   which no Apache Airflow DAG processor in this process does. Reporting a DAG, a DAG
//!   run or a task instance anyway would describe work nothing ran.
//! * Any other path — that API's own not-found answer; a path it serves called with a
//!   method it does not — its method-not-allowed answer.
//!
//! A write through this API is a change applied to the environment's own metadata, so it
//! needs an environment a change can be applied to: a record whose teardown has already
//! started still answers a read but takes no write
//! ([`crate::state::EnvironmentRecord::accepts_change`]), so a non-`GET` call against one
//! is refused with `ValidationException` (HTTP 400) and nothing is written, while a `GET`
//! is served for as long as the record is there.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `InvokeRestApi::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `InvokeRestApi`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-mwaa-amazonmwaa/smithy-model/com-amazonaws-mwaa-amazonmwaa.json")
//! ```
//!
//! Invokes the Apache Airflow REST API on the webserver with the specified inputs. To learn
//! more, see Using the Apache Airflow REST API
//!
//! ## Input
//!
//! * `Body` (RestApiRequestBody) — The request body for the Apache Airflow REST API call,
//!   provided as a JSON object.
//! * `Method` (RestApiMethod, one of: DELETE | GET | PATCH | POST | PUT) **required** — The
//!   HTTP method used for making Airflow REST API calls. For example, POST.
//! * `Name` (EnvironmentName, len 1..80, pattern ^[a-zA-Z][0-9a-zA-Z-_]*$) **required** — The
//!   name of the Amazon MWAA environment. For example, MyMWAAEnvironment.
//! * `Path` (RestApiPath, len 1..64) **required** — The Apache Airflow REST API endpoint path
//!   to be called. For example, /dags/123456/clearTaskInstances. For more information, see
//!   Apache Airflow API
//! * `QueryParameters` (Document) — Query parameters to be included in the Apache Airflow
//!   REST API call, provided as a JSON object.
//!
//! ## Output
//!
//! * `RestApiResponse` (RestApiResponse) — The response data from the Apache Airflow REST API
//!   call, provided as a JSON object.
//! * `RestApiStatusCode` (Integer) — The HTTP status code returned by the Apache Airflow REST
//!   API call.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — Access to the Apache Airflow Web UI or CLI has been
//!   denied due to insufficient permissions. To learn more, see Accessing an Amazon MWAA
//!   environment.
//! * `InternalServerException` (HTTP 500) — InternalServerException: An internal error has
//!   occurred.
//! * `ResourceNotFoundException` (HTTP 404) — ResourceNotFoundException: The resource is not
//!   available.
//! * `RestApiClientException` (HTTP 400) — An exception indicating that a client-side error
//!   occurred during the Apache Airflow REST API call.
//! * `RestApiServerException` (HTTP 400) — An exception indicating that a server-side error
//!   occurred during the Apache Airflow REST API call.
//! * `ValidationException` (HTTP 400) — ValidationException: The provided input is not valid.

#[allow(unused_imports)]
use super::types::*;

use ::aws_smithy_types::{Document, Number};

use crate::state::{AirflowMetadata, AirflowPool, AirflowVariable, EnvironmentRecord};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `InvokeRestApiError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-mwaa-amazonmwaa.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `InvokeRestApiError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.mwaa#InvokeRestApi")]
#[derive(Debug)]
pub enum InvokeRestApiErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    RestApiClientException(RestApiClientException),
    RestApiServerException(RestApiServerException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::invoke_rest_api::InvokeRestApiError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type InvokeRestApiError = crate::scaffolded::Scaffolded<InvokeRestApiErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<InvokeRestApiErrorModeled> for InvokeRestApiError {
    fn from(modeled: InvokeRestApiErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `InvokeRestApi` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct InvokeRestApi<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> InvokeRestApi<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for InvokeRestApi<Eng>
{
    type Input = InvokeRestApiInput;
    type Output = InvokeRestApiOutput;
    type Error = InvokeRestApiError;

    const NAME: &'static str = "InvokeRestApi";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::INVOKE_REST_API_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::INVOKE_REST_API_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let now = ::std::time::SystemTime::now();
        // An environment lives in the Region it was created in, so a name is resolved in
        // the Region this request was signed for and nowhere else.
        let region = crate::state::request_region(ctx);

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            InvokeRestApiErrorModeled::InternalServerException(InternalServerException {
                message: Some("the Amazon MWAA environment store is unavailable".to_string()),
            })
        })?;
        let mut store = state.lock();
        // The transition the clock already owes, materialised before anything is read.
        store.settle(now);

        // The Apache Airflow REST API this call reaches is the one the STORED environment's
        // web server serves, so a name that resolves to nothing — in the Region this
        // request was signed for — has no API to call.
        //
        // This operation is served through the environment data endpoint's proxy
        // (`@endpoint` host prefix `env.`), so the refusal is that endpoint's: HTTP 404 with
        // the modeled `ResourceNotFoundException` code, and the endpoint's own fixed
        // wording, which names no environment. The status, the code and the sentence are
        // decided here; the SHAPE that refusal travels in on the wire is the front door's
        // (`crate::service::env_endpoint`), which this operation relays — the sentence as
        // the whole body.
        let Some(environment) = store.environment_mut(&region, &input.name) else {
            return Err(InvokeRestApiErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(
                        crate::state::ENVIRONMENT_ENDPOINT_NOT_FOUND_MESSAGE.to_string(),
                    ),
                },
            )
            .into());
        };

        let call = Call::new(&input);
        // A call that asks Apache Airflow to CHANGE something is a change applied to this
        // environment's own metadata database, so it needs an environment a change can be
        // applied to: a record whose teardown has already started still answers a read but
        // takes no write ([`crate::state::EnvironmentRecord::accepts_change`]), and the
        // write is refused — with the sentence that one rule words — before the API serves
        // it, so nothing is applied to a record the clock is about to remove. A read is
        // served for as long as the record is there, exactly as the control plane's readers
        // are.
        let refusal = call
            .changes_state()
            .then(|| environment.change_refusal())
            .flatten();
        if let Some(message) = refusal {
            return Err(InvokeRestApiErrorModeled::ValidationException(
                ValidationException {
                    message: Some(message),
                },
            )
            .into());
        }

        let answer = serve(environment, &call);
        drop(store);
        proxied(answer)
    }
}

/// One call to an environment's Apache Airflow REST API, as this operation's input states
/// it: the method, the path split into its segments, and the query parameters and body as
/// the caller supplied them.
struct Call<'r> {
    method: &'r str,
    path: &'r str,
    /// `path` without its empty segments, so a trailing or doubled `/` addresses the same
    /// resource as the plain path does.
    segments: Vec<&'r str>,
    query: Option<&'r Document>,
    body: Option<&'r Document>,
}

impl<'r> Call<'r> {
    fn new(input: &'r InvokeRestApiInput) -> Self {
        Self {
            method: input.method.as_str(),
            path: input.path.as_str(),
            segments: input
                .path
                .split(SEPARATOR)
                .filter(|segment| !segment.is_empty())
                .collect(),
            query: input.query_parameters.as_ref(),
            body: input.body.as_ref(),
        }
    }

    /// Whether this call asks the Apache Airflow REST API to CHANGE the environment's
    /// metadata, rather than to report it.
    ///
    /// Decided by the method alone, because that is what the method means in that API:
    /// [`GET`] is its only read — every other method `RestApiMethod` admits
    /// (`smithy_query(query="shape",shape="RestApiMethod")`: DELETE | GET | PATCH | POST |
    /// PUT) creates, replaces or removes an entity — so a non-`GET` call is a write
    /// whichever path it addresses, including one this API does not serve with that method.
    fn changes_state(&self) -> bool {
        self.method != GET
    }
}

/// What one Apache Airflow REST API call answered: its HTTP status, and its JSON document
/// where that status carries one (a `204` carries none).
struct AirflowAnswer {
    status: i32,
    document: Option<Document>,
}

/// What an Apache Airflow REST API path is built from, and so also what a stored entity
/// key is addressed through: the path is split on it, and the segments after a
/// collection's own are rejoined with it to name the entity inside that collection.
const SEPARATOR: &str = "/";

/// The longest `Path` a call to this operation can carry, in characters: the upper bound of
/// the modelled path member (`smithy_query(query="shape",shape="RestApiPath")` —
/// `@length(1, 64)`), which the request boundary enforces — counting characters, as
/// `@length` on a string does — before this handler runs.
///
/// So it is also the longest path anything stored here can be ADDRESSED through again, and
/// therefore what bounds the key a write may store ([`longest_key`]).
const LONGEST_PATH: usize = 64;

/// The path segment the variables collection is addressed under, and the member its
/// collection answer carries its entries in.
const VARIABLES: &str = "variables";

/// The same for the slot pools collection.
const POOLS: &str = "pools";

/// Apache Airflow's `GET` method, the only one its read-only endpoints serve.
const GET: &str = "GET";
/// Apache Airflow's `POST` method, which creates an entity in a collection.
const POST: &str = "POST";
/// Apache Airflow's `PATCH` method, which changes an entity.
const PATCH: &str = "PATCH";
/// Apache Airflow's `DELETE` method, which removes an entity.
const DELETE: &str = "DELETE";

/// How many entries a collection endpoint returns when the call pages it with no `limit`,
/// and the most it returns for any `limit`.
///
/// Apache Airflow's REST API pages every collection with `limit` and `offset` and caps the
/// page at its maximum page limit; this is the emulator's own page size, deliberately
/// equal to the cap, so one unparameterised call cannot materialise a response that grows
/// with the stored entry count.
const PAGE_LIMIT: usize = 100;

/// The window a collection call asked for: how many entries, from which offset.
struct PageWindow {
    limit: usize,
    offset: usize,
}

/// How many slots of a pool are in use here.
///
/// Every slot counter an Apache Airflow pool reports counts TASK INSTANCES the scheduler
/// placed in that pool, and nothing in this process schedules a task instance (see this
/// module's docs on the DAG inventory), so each counter is zero and every configured slot
/// is open. Zero is computed, not assumed: it is the count of task instances this store
/// holds.
const SLOTS_IN_USE: i64 = 0;

/// Serve one Apache Airflow REST API call against the environment's own state.
///
/// Takes the environment mutably because the API's writes are writes to that
/// environment's metadata: a `GET` reaches nothing that changes what a later caller sees,
/// while a `POST`, `PATCH` or `DELETE` is the change the caller asked Apache Airflow to
/// make.
fn serve(environment: &mut EnvironmentRecord, call: &Call<'_>) -> AirflowAnswer {
    // The query parameters are what this API's request carries on its query string, and a
    // query string is built from the NAMED values of an object: a document of any other
    // shape names no parameter at all, so it is refused here — before the path is
    // addressed, since the query string belongs to the request rather than to the endpoint,
    // and for every method, since every one of them is called with a URL — rather than read
    // as if the caller had asked for no parameters.
    let query = match object_document(call.query, QUERY_PARAMETERS) {
        Ok(query) => query,
        Err(refusal) => return refusal,
    };
    // The body is the entity the request carries, and an entity is likewise built from the
    // NAMED members of an object: a document of any other shape carries no entity at all,
    // so it is refused in the same place and for the same reason as the query parameters —
    // before the path is addressed, and for every method, since the body travels with the
    // request whichever endpoint it is addressed to and whether or not that endpoint reads
    // one — rather than read as if the caller had carried none.
    let body = match object_document(call.body, REQUEST_BODY) {
        Ok(body) => body,
        Err(refusal) => return refusal,
    };
    match call.segments.as_slice() {
        ["version"] => match call.method {
            GET => airflow_version(environment),
            _ => method_not_allowed(call),
        },
        [VARIABLES] => match call.method {
            GET => list_variables(&environment.airflow, query),
            POST => set_variable(&mut environment.airflow, body),
            _ => method_not_allowed(call),
        },
        // Every segment after `variables` belongs to the KEY, because this collection
        // holds no sub-resource a further segment could address: see [`entity_key`].
        [VARIABLES, key @ ..] => {
            let key = entity_key(key);
            match call.method {
                GET => read_variable(&environment.airflow, &key),
                PATCH => change_variable(&mut environment.airflow, &key, body),
                DELETE => delete_variable(&mut environment.airflow, &key),
                _ => method_not_allowed(call),
            }
        }
        [POOLS] => match call.method {
            GET => list_pools(&environment.airflow, query),
            POST => create_pool(&mut environment.airflow, body),
            _ => method_not_allowed(call),
        },
        // A pool's name is addressed the same way, and for the same reason.
        [POOLS, name @ ..] => {
            let name = entity_key(name);
            match call.method {
                GET => read_pool(&environment.airflow, &name),
                PATCH => change_pool(&mut environment.airflow, &name, body),
                DELETE => delete_pool(&mut environment.airflow, &name),
                _ => method_not_allowed(call),
            }
        }
        ["dags"] => match call.method {
            GET => list_dags(query),
            _ => method_not_allowed(call),
        },
        // Everything under `/dags/{dag_id}` — the DAG itself, its DAG runs, their task
        // instances — is addressed THROUGH a DAG, so the DAG has to resolve before the
        // method or the rest of the path means anything, and no DAG resolves here.
        ["dags", dag_id, ..] => dag_not_found(dag_id),
        _ => path_not_found(call),
    }
}

/// The caller's answer for what the Apache Airflow REST API answered: a success carries
/// the status and the document, and a failure carries the same pair in the modelled
/// exception its CLASS selects.
fn proxied(answer: AirflowAnswer) -> Result<InvokeRestApiOutput, InvokeRestApiError> {
    match answer.status {
        200..=299 => Ok(InvokeRestApiOutput {
            rest_api_status_code: Some(answer.status),
            rest_api_response: answer.document,
        }),
        400..=499 => Err(InvokeRestApiErrorModeled::RestApiClientException(
            RestApiClientException {
                rest_api_status_code: Some(answer.status),
                rest_api_response: answer.document,
            },
        )
        .into()),
        // Any other status is a server-side failure of the call, which is the other
        // modelled exception. Nothing this emulator serves answers one: a
        // `RestApiServerException` would otherwise report the failure of a server that
        // never ran.
        _ => Err(InvokeRestApiErrorModeled::RestApiServerException(
            RestApiServerException {
                rest_api_status_code: Some(answer.status),
                rest_api_response: answer.document,
            },
        )
        .into()),
    }
}

impl AirflowAnswer {
    /// A `200` carrying `document`.
    fn ok(document: Document) -> Self {
        Self {
            status: 200,
            document: Some(document),
        }
    }

    /// A `204`, which carries no document: the answer to a delete.
    fn no_content() -> Self {
        Self {
            status: 204,
            document: None,
        }
    }

    /// A failure as that API reports one: a problem document (`type`, `title`, `status`,
    /// `detail`) naming the status, a title for its class and the detail of this one
    /// failure.
    ///
    /// `type` is the `about:blank` the problem-document format itself defines for a
    /// problem with no further type beyond its status code.
    fn problem(status: i32, title: &str, detail: String) -> Self {
        Self {
            status,
            document: Some(object([
                ("type", Document::String("about:blank".to_string())),
                ("title", Document::String(title.to_string())),
                ("status", integer(i64::from(status))),
                ("detail", Document::String(detail)),
            ])),
        }
    }

    /// The answer to a call whose request this API cannot act on.
    fn bad_request(detail: String) -> Self {
        Self::problem(400, "Bad Request", detail)
    }

    /// The answer to a call addressing something this API holds nothing for.
    fn not_found(detail: String) -> Self {
        Self::problem(404, "Not Found", detail)
    }

    /// The answer to a create of an entity whose key is taken.
    fn conflict(detail: String) -> Self {
        Self::problem(409, "Conflict", detail)
    }
}

/// `GET /version`: the Apache Airflow version this environment runs, read from the stored
/// record rather than from any default. The build the version was cut from is not
/// something this emulator holds, so it reports none.
fn airflow_version(environment: &EnvironmentRecord) -> AirflowAnswer {
    AirflowAnswer::ok(object([
        (
            "version",
            Document::String(environment.spec.airflow_version.clone()),
        ),
        ("git_version", Document::Null),
    ]))
}

/// `GET /dags`: the environment's DAG inventory, which holds nothing — see this module's
/// docs. The page the call asked for is still resolved, so a call paging the collection
/// with an unusable `limit` or `offset` is refused here exactly as it is on a collection
/// that does hold entries.
fn list_dags(query: Option<&Document>) -> AirflowAnswer {
    let window = match page_window(query) {
        Ok(window) => window,
        Err(refusal) => return refusal,
    };
    collection("dags", Vec::new(), &window)
}

/// `GET /variables`: the environment's variables, in key order.
fn list_variables(metadata: &AirflowMetadata, query: Option<&Document>) -> AirflowAnswer {
    let window = match page_window(query) {
        Ok(window) => window,
        Err(refusal) => return refusal,
    };
    let entries = metadata.variables.values().map(variable_document).collect();
    collection(VARIABLES, entries, &window)
}

/// `POST /variables`: set the variable the body names, whether or not the key is already
/// taken — a variable's key is its identity, and this endpoint SETS the value under that
/// key rather than refusing a key that exists.
fn set_variable(metadata: &mut AirflowMetadata, body: Option<&Document>) -> AirflowAnswer {
    let requested = match requested_variable(body) {
        Ok(requested) => requested,
        Err(refusal) => return refusal,
    };
    let fresh = !metadata.variables.contains_key(&requested.key);
    if fresh && metadata.variables.len() >= crate::state::MAX_AIRFLOW_VARIABLES {
        return AirflowAnswer::bad_request(format!(
            "this emulator holds at most {} variables for one environment",
            crate::state::MAX_AIRFLOW_VARIABLES
        ));
    }
    let key = requested.key.clone();
    metadata.variables.insert(key.clone(), requested);
    stored_variable(metadata, &key)
}

/// `GET /variables/{key}`.
fn read_variable(metadata: &AirflowMetadata, key: &str) -> AirflowAnswer {
    match metadata.variables.get(key) {
        Some(variable) => AirflowAnswer::ok(variable_document(variable)),
        None => AirflowAnswer::not_found(no_such_variable(key)),
    }
}

/// `PATCH /variables/{key}`: change the variable the path names. A body keyed to a
/// different variable is refused rather than silently moving the value to another key.
fn change_variable(
    metadata: &mut AirflowMetadata,
    key: &str,
    body: Option<&Document>,
) -> AirflowAnswer {
    let requested = match requested_variable(body) {
        Ok(requested) => requested,
        Err(refusal) => return refusal,
    };
    if requested.key != key {
        return AirflowAnswer::bad_request(format!(
            "the key {:?} in the request body does not name the variable {key:?} in the path",
            requested.key
        ));
    }
    if !metadata.variables.contains_key(key) {
        return AirflowAnswer::not_found(no_such_variable(key));
    }
    metadata.variables.insert(key.to_string(), requested);
    stored_variable(metadata, key)
}

/// `DELETE /variables/{key}`.
fn delete_variable(metadata: &mut AirflowMetadata, key: &str) -> AirflowAnswer {
    match metadata.variables.remove(key) {
        Some(_) => AirflowAnswer::no_content(),
        None => AirflowAnswer::not_found(no_such_variable(key)),
    }
}

/// `GET /pools`: the environment's slot pools, in name order.
fn list_pools(metadata: &AirflowMetadata, query: Option<&Document>) -> AirflowAnswer {
    let window = match page_window(query) {
        Ok(window) => window,
        Err(refusal) => return refusal,
    };
    let entries = metadata.pools.values().map(pool_document).collect();
    collection(POOLS, entries, &window)
}

/// `POST /pools`: create the pool the body names. A pool's name is its identity, so a
/// name already taken is a conflict rather than a replacement.
fn create_pool(metadata: &mut AirflowMetadata, body: Option<&Document>) -> AirflowAnswer {
    let requested = match requested_pool(body) {
        Ok(requested) => requested,
        Err(refusal) => return refusal,
    };
    if metadata.pools.contains_key(&requested.name) {
        return AirflowAnswer::conflict(format!("the pool {:?} already exists", requested.name));
    }
    if metadata.pools.len() >= crate::state::MAX_AIRFLOW_POOLS {
        return AirflowAnswer::bad_request(format!(
            "this emulator holds at most {} pools for one environment",
            crate::state::MAX_AIRFLOW_POOLS
        ));
    }
    let name = requested.name.clone();
    metadata.pools.insert(name.clone(), requested);
    stored_pool(metadata, &name)
}

/// `GET /pools/{name}`.
fn read_pool(metadata: &AirflowMetadata, name: &str) -> AirflowAnswer {
    match metadata.pools.get(name) {
        Some(pool) => AirflowAnswer::ok(pool_document(pool)),
        None => AirflowAnswer::not_found(no_such_pool(name)),
    }
}

/// `PATCH /pools/{name}`: change the pool the path names. As for a variable, a body naming
/// a different pool is refused rather than renaming the one the path addresses.
fn change_pool(
    metadata: &mut AirflowMetadata,
    name: &str,
    body: Option<&Document>,
) -> AirflowAnswer {
    let requested = match requested_pool(body) {
        Ok(requested) => requested,
        Err(refusal) => return refusal,
    };
    if requested.name != name {
        return AirflowAnswer::bad_request(format!(
            "the name {:?} in the request body does not name the pool {name:?} in the path",
            requested.name
        ));
    }
    if !metadata.pools.contains_key(name) {
        return AirflowAnswer::not_found(no_such_pool(name));
    }
    metadata.pools.insert(name.to_string(), requested);
    stored_pool(metadata, name)
}

/// `DELETE /pools/{name}`. Apache Airflow's own default pool is the pool a task with no
/// pool of its own runs in, so it is not one a caller can take away.
fn delete_pool(metadata: &mut AirflowMetadata, name: &str) -> AirflowAnswer {
    if name == crate::state::DEFAULT_POOL_NAME {
        return AirflowAnswer::bad_request(format!(
            "the {} cannot be deleted",
            crate::state::DEFAULT_POOL_NAME
        ));
    }
    match metadata.pools.remove(name) {
        Some(_) => AirflowAnswer::no_content(),
        None => AirflowAnswer::not_found(no_such_pool(name)),
    }
}

/// The key the entity a path addresses is stored under: the segments after its
/// collection's own, rejoined with the [`SEPARATOR`] the path was split on.
///
/// An Apache Airflow variable key and a pool name are free text that may itself carry
/// that separator, and neither collection holds a sub-resource a further segment could
/// address, so every segment after the collection's own is part of the one key — which is
/// what makes every key a write stores addressable by the very path grammar that stored
/// it. Empty segments were dropped when the path was split, so a doubled or trailing
/// separator addresses the same entity the plain path does, and the writes refuse a key
/// that would need one ([`addressable`]).
fn entity_key(segments: &[&str]) -> String {
    segments.join(SEPARATOR)
}

/// The answer to a call on a path this API serves with a method it does not.
fn method_not_allowed(call: &Call<'_>) -> AirflowAnswer {
    AirflowAnswer::problem(
        405,
        "Method Not Allowed",
        format!("{} is not allowed on {}", call.method, call.path),
    )
}

/// The answer to a call on a path this environment's Apache Airflow REST API does not
/// serve.
///
/// The detail says that THIS API serves no such path, and claims nothing about what the
/// Apache Airflow REST API serves in general: the paths this one answers are the ones this
/// module's docs list, and a path outside them may well be one Apache Airflow itself
/// serves.
fn path_not_found(call: &Call<'_>) -> AirflowAnswer {
    AirflowAnswer::not_found(format!(
        "{} is not one of the paths this environment's Apache Airflow REST API serves",
        call.path
    ))
}

/// The answer to a call addressed through a DAG the environment holds none of.
fn dag_not_found(dag_id: &str) -> AirflowAnswer {
    AirflowAnswer::not_found(format!("no DAG with dag_id {dag_id:?} was found"))
}

/// Why a variable the path named could not be read, changed or deleted.
fn no_such_variable(key: &str) -> String {
    format!("no variable with key {key:?} was found")
}

/// Why a pool the path named could not be read, changed or deleted.
fn no_such_pool(name: &str) -> String {
    format!("no pool with name {name:?} was found")
}

/// Answer a variable write from the STORED record, so what the caller is told cannot
/// differ from what the next read of it reports.
fn stored_variable(metadata: &AirflowMetadata, key: &str) -> AirflowAnswer {
    read_variable(metadata, key)
}

/// Answer a pool write from the STORED record, for the same reason.
fn stored_pool(metadata: &AirflowMetadata, name: &str) -> AirflowAnswer {
    read_pool(metadata, name)
}

/// One variable as that API reports it.
fn variable_document(variable: &AirflowVariable) -> Document {
    object([
        ("key", Document::String(variable.key.clone())),
        ("value", Document::String(variable.value.clone())),
        ("description", nullable(variable.description.as_deref())),
    ])
}

/// One slot pool as that API reports it: its stored configuration, and the slot counts
/// computed from the task instances in it ([`SLOTS_IN_USE`]).
fn pool_document(pool: &AirflowPool) -> Document {
    object([
        ("name", Document::String(pool.name.clone())),
        ("slots", integer(pool.slots)),
        ("description", nullable(pool.description.as_deref())),
        ("include_deferred", Document::Bool(pool.include_deferred)),
        ("occupied_slots", integer(SLOTS_IN_USE)),
        ("running_slots", integer(SLOTS_IN_USE)),
        ("queued_slots", integer(SLOTS_IN_USE)),
        ("scheduled_slots", integer(SLOTS_IN_USE)),
        ("deferred_slots", integer(SLOTS_IN_USE)),
        ("open_slots", integer(pool.slots - SLOTS_IN_USE)),
    ])
}

/// A collection answer: the page of `entries` the window selects under `member`, beside
/// the `total_entries` the whole collection holds.
fn collection(member: &str, entries: Vec<Document>, window: &PageWindow) -> AirflowAnswer {
    let total = entries.len();
    let page: Vec<Document> = entries
        .into_iter()
        .skip(window.offset)
        .take(window.limit)
        .collect();
    AirflowAnswer::ok(Document::Object(
        [
            (member.to_string(), Document::Array(page)),
            ("total_entries".to_string(), count(total)),
        ]
        .into(),
    ))
}

/// The page a collection call asked for: the `limit` and `offset` of the query parameters
/// it carries ([`object_document`] has already read those parameters as the object they
/// are, so `query` is that object or nothing at all), or [`PAGE_LIMIT`] and the start of
/// the collection when it named none.
///
/// A limit past [`PAGE_LIMIT`] is capped to it, and a limit of none at all — an absent
/// parameter, or one asking for no entry — falls back to that same page size, since a page
/// of no entries is not a page the parameter selects. A negative limit, and an offset
/// before the start of the collection, name no page at all and are refused.
fn page_window(query: Option<&Document>) -> Result<PageWindow, AirflowAnswer> {
    let limit = whole_parameter(query, "limit")?;
    let offset = whole_parameter(query, "offset")?;
    for (name, value) in [("limit", limit), ("offset", offset)] {
        if value.is_some_and(|value| value < 0) {
            return Err(AirflowAnswer::bad_request(format!(
                "{name} must not be negative"
            )));
        }
    }
    let limit = limit
        .and_then(|limit| usize::try_from(limit).ok())
        .filter(|limit| *limit > 0)
        .unwrap_or(PAGE_LIMIT)
        .min(PAGE_LIMIT);
    let offset = offset
        .and_then(|offset| usize::try_from(offset).ok())
        .unwrap_or_default();
    Ok(PageWindow { limit, offset })
}

/// The variable a write's body states, or why that body states none.
fn requested_variable(body: Option<&Document>) -> Result<AirflowVariable, AirflowAnswer> {
    let body = object_body(body)?;
    let key = required_text(body, "key")?;
    let value = required_text(body, "value")?;
    let description = optional_text(body, "description")?;
    addressable(&key, "key", VARIABLES)?;
    bounded(&value, "value", crate::state::MAX_AIRFLOW_VALUE_LENGTH)?;
    bounded_description(description.as_deref())?;
    Ok(AirflowVariable {
        key,
        value,
        description,
    })
}

/// The pool a write's body states, or why that body states none.
fn requested_pool(body: Option<&Document>) -> Result<AirflowPool, AirflowAnswer> {
    let body = object_body(body)?;
    let name = required_text(body, "name")?;
    let Some(slots) = member(body, "slots").and_then(whole) else {
        return Err(AirflowAnswer::bad_request(
            "the request body must carry the member \"slots\" as a whole number".to_string(),
        ));
    };
    // A pool's slots are how many slots it HOLDS, so a count below none is not a pool this
    // store can hold.
    if slots < 0 {
        return Err(AirflowAnswer::bad_request(format!(
            "the member \"slots\" must not be negative, got {slots}"
        )));
    }
    let description = optional_text(body, "description")?;
    addressable(&name, "name", POOLS)?;
    bounded_description(description.as_deref())?;
    let include_deferred = match member(body, "include_deferred") {
        Some(Document::Bool(include)) => *include,
        Some(Document::Null) | None => false,
        Some(_) => {
            return Err(AirflowAnswer::bad_request(
                "the member \"include_deferred\" must be a boolean".to_string(),
            ))
        }
    };
    Ok(AirflowPool {
        name,
        slots,
        description,
        include_deferred,
    })
}

/// What this operation's query parameters are called in a refusal of a document that
/// states none.
const QUERY_PARAMETERS: &str = "query parameters";
/// What this operation's request body is called in the same refusal.
const REQUEST_BODY: &str = "request body";

/// A document this API takes as a JSON object, as that object.
///
/// Each of this operation's two documents is one the model documents as a JSON object
/// (`smithy_query(query="doc",shape="InvokeRestApiRequest",member="QueryParameters"|
/// "Body")`), because an object is what names the values it carries: a document of any
/// other shape — an array, a string, a number, a boolean — names none of them, so it is
/// refused rather than read as if the caller had supplied nothing. An absent document, and
/// the `null` that spells one, state nothing and are `None`; what a caller who states
/// nothing gets is the endpoint's own business.
fn object_document<'d>(
    document: Option<&'d Document>,
    what: &str,
) -> Result<Option<&'d Document>, AirflowAnswer> {
    match document {
        None | Some(Document::Null) => Ok(None),
        Some(object @ Document::Object(_)) => Ok(Some(object)),
        Some(_) => Err(not_an_object(what)),
    }
}

/// A write's body as the JSON object every one of these endpoints takes, which — unlike
/// the query parameters — a write has to carry: a body stating nothing states no entity.
///
/// Its SHAPE was already judged for this call, whatever endpoint the call addresses
/// ([`serve`]), so the body here is that object or nothing at all; what is left for a write
/// is that it has to be there.
fn object_body(body: Option<&Document>) -> Result<&Document, AirflowAnswer> {
    body.ok_or_else(|| not_an_object(REQUEST_BODY))
}

/// Why a document this API takes as a JSON object could not be read as one.
fn not_an_object(what: &str) -> AirflowAnswer {
    AirflowAnswer::bad_request(format!("the {what} must be a JSON object"))
}

/// A member a body has to carry as a non-empty string.
fn required_text(body: &Document, name: &str) -> Result<String, AirflowAnswer> {
    match member(body, name).and_then(text) {
        Some(value) if !value.is_empty() => Ok(value.to_string()),
        _ => Err(AirflowAnswer::bad_request(format!(
            "the request body must carry the member {name:?} as a non-empty string"
        ))),
    }
}

/// A member a body may carry as a string; `null` and an absent member both state none.
fn optional_text(body: &Document, name: &str) -> Result<Option<String>, AirflowAnswer> {
    match member(body, name) {
        Some(Document::String(value)) => Ok(Some(value.clone())),
        Some(Document::Null) | None => Ok(None),
        Some(_) => Err(AirflowAnswer::bad_request(format!(
            "the member {name:?} must be a string"
        ))),
    }
}

/// The longest key an entity of the collection whose path segment is `collection` may be
/// stored under, in characters.
///
/// A key is reached again only through the path `/{collection}/{key}`, so what a legal path
/// can still carry under that collection — [`LONGEST_PATH`] less the collection's own
/// segment and the two [`SEPARATOR`]s around it — is one ceiling on the key; the store's own
/// ([`crate::state::MAX_AIRFLOW_KEY_LENGTH`]) is the other, and a key has to satisfy both to
/// be a key this API both holds and can address, so the lower of the two is the ceiling.
fn longest_key(collection: &str) -> usize {
    let addressable =
        LONGEST_PATH.saturating_sub(2 * SEPARATOR.chars().count() + collection.chars().count());
    addressable.min(crate::state::MAX_AIRFLOW_KEY_LENGTH)
}

/// Refuse a key this API could store but no path of it could name again.
///
/// An entity is addressed by the segments of its path, which carry no empty one
/// ([`Call::new`]), so a key with an empty [`SEPARATOR`]-delimited segment — one that
/// opens or closes with the separator, or carries two in a row — is a key no path splits
/// into. A key longer than [`longest_key`] is unreachable for the other reason: every path
/// that names it is longer than the modelled path member admits, so the boundary refuses
/// the call before this API is asked. Either way storing it would put an entity in the
/// collection that the API's own read, change and delete could never reach, and whose slot
/// nothing could free. Every other key, including one that carries the separator between
/// segments, is addressable and taken.
///
/// The length is judged FIRST, and its refusal states the lengths rather than echoing the
/// value: a value past that ceiling is longer than any path that could carry it, so a
/// refusal echoing it would answer with a document whose size the request decides.
fn addressable(value: &str, name: &str, collection: &str) -> Result<(), AirflowAnswer> {
    let longest = longest_key(collection);
    let length = value.chars().count();
    if length > longest {
        return Err(AirflowAnswer::bad_request(format!(
            "a {name} of {length} characters is not one this API could address again: it is \
             addressed through {SEPARATOR}{collection}{SEPARATOR}{{{name}}}, so only a \
             {name} of at most {longest} characters can be named by a path"
        )));
    }
    if value.split(SEPARATOR).any(str::is_empty) {
        return Err(AirflowAnswer::bad_request(format!(
            "the {name} {value:?} is not one this API could address again: a {name} must \
             neither begin nor end with {SEPARATOR:?} nor carry an empty segment between \
             two of them"
        )));
    }
    Ok(())
}

/// Refuse a value this store will not hold at that length.
fn bounded(value: &str, name: &str, longest: usize) -> Result<(), AirflowAnswer> {
    if value.chars().count() > longest {
        return Err(AirflowAnswer::bad_request(format!(
            "this emulator holds a {name} of at most {longest} characters"
        )));
    }
    Ok(())
}

/// Refuse a `description` this store will not hold at that length, for a body that states
/// one; a body that states none states no length to bound, so it is taken.
///
/// Both entities a write can carry — a variable and a slot pool — may carry an optional
/// `description`, and this is the one place either of them asks what one may be: the
/// branch, the member name the refusal sentence names and the ceiling it is held to are
/// decided HERE and nowhere else, so a variable write and a pool write cannot drift apart
/// about the length of a description.
fn bounded_description(description: Option<&str>) -> Result<(), AirflowAnswer> {
    match description {
        Some(description) => bounded(
            description,
            "description",
            crate::state::MAX_AIRFLOW_VALUE_LENGTH,
        ),
        None => Ok(()),
    }
}

/// A query parameter as a whole number, or why it is not one. An absent parameter is
/// `None` rather than a refusal.
fn whole_parameter(
    query: Option<&Document>,
    name: &str,
) -> Result<Option<i64>, AirflowAnswer> {
    let Some(value) = query.and_then(|query| member(query, name)) else {
        return Ok(None);
    };
    if matches!(value, Document::Null) {
        return Ok(None);
    }
    whole(value).map(Some).ok_or_else(|| {
        AirflowAnswer::bad_request(format!("{name} must be a whole number"))
    })
}

/// One member of a document that is an object.
fn member<'d>(document: &'d Document, name: &str) -> Option<&'d Document> {
    match document {
        Document::Object(members) => members.get(name),
        _ => None,
    }
}

/// A document as the string it carries, if it carries one.
fn text(document: &Document) -> Option<&str> {
    match document {
        Document::String(value) => Some(value.as_str()),
        _ => None,
    }
}

/// A document as the whole number it carries. A query parameter travels as text on an HTTP
/// query string, so a number spelled as a string counts as one.
fn whole(document: &Document) -> Option<i64> {
    match document {
        Document::Number(Number::PosInt(value)) => i64::try_from(*value).ok(),
        Document::Number(Number::NegInt(value)) => Some(*value),
        Document::String(value) => value.trim().parse().ok(),
        _ => None,
    }
}

/// A JSON object of the members given, in the order written.
fn object<const N: usize>(members: [(&str, Document); N]) -> Document {
    Document::Object(
        members
            .into_iter()
            .map(|(name, value)| (name.to_string(), value))
            .collect(),
    )
}

/// A string member that may carry nothing, as the `null` a JSON document spells that with.
fn nullable(value: Option<&str>) -> Document {
    match value {
        Some(value) => Document::String(value.to_string()),
        None => Document::Null,
    }
}

/// A whole number as a JSON number.
fn integer(value: i64) -> Document {
    match u64::try_from(value) {
        Ok(unsigned) => Document::Number(Number::PosInt(unsigned)),
        Err(_) => Document::Number(Number::NegInt(value)),
    }
}

/// A count as a JSON number.
fn count(value: usize) -> Document {
    Document::Number(Number::PosInt(u64::try_from(value).unwrap_or(u64::MAX)))
}
