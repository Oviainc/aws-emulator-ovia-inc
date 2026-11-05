// SPDX-License-Identifier: CC0-1.0

//! The authorization pre-check in front of this service, end to end over HTTP, with
//! request authentication ON (`set_request_authentication(true)`, the `--auth on` path the
//! production front door runs).
//!
//! The rule under test: every request to this endpoint is authorized before it is
//! dispatched, so a caller the emulator's authentication layer cannot verify — unsigned, or
//! presenting credentials this emulator does not hold — is refused by that layer with the
//! front door's own answer: HTTP 401, `x-amzn-errortype: UnauthorizedException`, and a body
//! that is exactly the fixed sentence `Unauthorized` under a LOWERCASE `message` member.
//! Never one of this service's modelled errors, whose sentence travels under `Message` and
//! whose statuses are the model's (400 `ValidationException`/`AccessDeniedException`, 404
//! `ResourceNotFoundException`, 409 `ConflictException`); and never the generic
//! `AccessDenied` code the framework reports to the service, which this model declares
//! nowhere.
//!
//! The refusal is decided from the credentials the request presents and from NOTHING it
//! says, so it is the same answer whatever operation the request names, whatever its
//! payload carries, and whether or not the resource it names exists — and it lands ahead of
//! dispatch, so a refused mutation changes nothing a later caller sees.
//!
//! verified. The rows differ along the dimension the rule generalises over — WHY the caller
//! cannot be verified — and span that space rather than one case: whether a credential
//! scope travels at all, where it travels (an `Authorization` header or a presigned query),
//! what the scope is shaped like (a key this emulator never minted, a terminator that is
//! not `aws4_request`), which algorithm it names, and whether a signing date is present.
//! Each row is walked against a read, a create, both mutations (addressed by name AND by
//! ARN, so the refusal is shown not to depend on the handle) and a list.
//!
//! Nothing here is signed: every row is a presentation the emulator's verifier rejects
//! BEFORE it compares a signature, which is what lets this suite span the refusal space
//! without a signer (the verifier's own accept path is pinned by the framework beside the
//! verifier it belongs to). What this service answers a caller it DOES verify is pinned by
//! every other suite in this crate, which drives the same routes over the same engine with
//! the authenticator off; the last case below drives one such request through that router
//! to show the 401 above is the verification layer's verdict rather than the operation's.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_observabilityadmin_observabilityadmin::state::ComAmazonawsObservabilityadminObservabilityadminState as Store;
use service_com_amazonaws_observabilityadmin_observabilityadmin::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for, and the one the seeded pipeline lives in.
const REGION: &str = "us-west-2";

/// The status the front door refuses a caller it could not verify with.
const UNAUTHORIZED_STATUS: u16 = 401;

/// The wire error code that refusal names.
const UNAUTHORIZED_CODE: &str = "UnauthorizedException";

/// The one fixed sentence it states.
const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";

/// The host this service's endpoint resolves to, so a request whose credential scope names
/// something else — or nothing at all — still reaches THIS service and is refused by
/// authentication rather than left unclaimed.
const ENDPOINT_HOST: &str = "observabilityadmin.us-west-2.amazonaws.com";

/// A fixed signing date, as SigV4 writes it, with its credential-scope date.
const AMZ_DATE: &str = "20260101T000000Z";
const SCOPE_DATE: &str = "20260101";

/// The configuration document the seeded pipeline is created with.
const CONFIGURATION: &str = "source:\n  http: {}\nsink:\n  - cloudwatch_logs: {}\n";

/// A second document, so a refused update can be seen to have replaced nothing.
const UPDATED_CONFIGURATION: &str =
    "source:\n  http: {}\nprocessor:\n  - parse_json: {}\nsink:\n  - cloudwatch_logs: {}\n";

/// One answer, read strictly: the status, the `x-amzn-errortype` header and the WHOLE
/// parsed body, with nothing folded together.
#[derive(Debug, PartialEq, Eq)]
struct Reply {
    status: u16,
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    /// Whether this is the front door's refusal, in full: the status, the wire error code,
    /// and a body that is exactly the fixed sentence under the lowercase `message` member.
    fn is_unauthorized(&self) -> bool {
        self.status == UNAUTHORIZED_STATUS
            && self.error_type.as_deref() == Some(UNAUTHORIZED_CODE)
            && self.body == json!({ "message": UNAUTHORIZED_MESSAGE })
    }
}

/// How one caller presents its credentials: the `Authorization` header when it carries
/// one, the signing date when it sends one, and the query string a presigned caller carries
/// its scope in.
struct Presentation {
    what: &'static str,
    authorization: Option<String>,
    amz_date: Option<&'static str>,
    query: String,
}

/// A header-signed presentation carrying `scope` under `algorithm`, with no valid
/// signature: the verifier reads the algorithm and the scope before it compares one.
fn header_signed(
    what: &'static str,
    algorithm: &str,
    scope: &str,
    amz_date: Option<&'static str>,
) -> Presentation {
    Presentation {
        what,
        authorization: Some(format!(
            "{algorithm} Credential={scope}, SignedHeaders=host;x-amz-date, \
             Signature=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        )),
        amz_date,
        query: String::new(),
    }
}

/// A presigned presentation: the same scope fields in the query string, where a presigned
/// URL carries them (`/` percent-encoded, as a URL carries it).
fn presigned(what: &'static str, scope: &str) -> Presentation {
    Presentation {
        what,
        authorization: None,
        amz_date: None,
        query: format!(
            "?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential={scope}&X-Amz-Date={AMZ_DATE}\
             &X-Amz-Expires=300&X-Amz-SignedHeaders=host\
             &X-Amz-Signature=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            scope = scope.replace('/', "%2F"),
        ),
    }
}

/// Every way a caller can fail verification at this endpoint.
fn presentations() -> Vec<Presentation> {
    let scope = format!("AKIDEXAMPLE/{SCOPE_DATE}/{REGION}/observabilityadmin/aws4_request");
    vec![
        // Nothing at all: the request a client that forgot its credentials sends.
        Presentation {
            what: "unsigned",
            authorization: None,
            amz_date: None,
            query: String::new(),
        },
        // A key this emulator never minted: nothing in its credential store holds the
        // secret such a signature would need.
        header_signed(
            "an access key the emulator never minted",
            "AWS4-HMAC-SHA256",
            &scope,
            Some(AMZ_DATE),
        ),
        // Header-signed with no signing date: a SigV4 signature cannot even be read
        // without the instant it was made at.
        header_signed(
            "header-signed with no x-amz-date",
            "AWS4-HMAC-SHA256",
            &scope,
            None,
        ),
        // A scope that does not end in the SigV4 terminator: claimed by the endpoint host,
        // since the scope cannot name the service.
        header_signed(
            "a credential scope whose terminator is not aws4_request",
            "AWS4-HMAC-SHA256",
            &format!("AKIDEXAMPLE/{SCOPE_DATE}/{REGION}/observabilityadmin/aws5_request"),
            Some(AMZ_DATE),
        ),
        // An algorithm the emulator's only authenticator does not read.
        header_signed(
            "an AWS4-ECDSA-P256-SHA256 Authorization the authenticator cannot read",
            "AWS4-ECDSA-P256-SHA256",
            &scope,
            Some(AMZ_DATE),
        ),
        // The presigned carrier, with the scope in the query and a key never minted.
        presigned(
            "presigned with an access key the emulator never minted",
            &scope,
        ),
    ]
}

/// One request this suite sends, named by the operation it reaches.
struct Call {
    operation: &'static str,
    uri: &'static str,
    body: Value,
}

/// A request for each shape of operation this service serves, so the rows show the refusal
/// landing ahead of each: a read, a create, both mutations — addressed by NAME and by the
/// seeded pipeline's own ARN, because the verdict does not depend on the handle — and a
/// list that names no resource at all.
fn calls(name: &str, arn: &str) -> Vec<Call> {
    let configuration = json!({ "Body": UPDATED_CONFIGURATION });
    vec![
        Call {
            operation: "GetTelemetryPipeline",
            uri: "/GetTelemetryPipeline",
            body: json!({ "PipelineIdentifier": name }),
        },
        Call {
            operation: "CreateTelemetryPipeline",
            uri: "/CreateTelemetryPipeline",
            body: json!({
                "Name": "refused-create",
                "Configuration": { "Body": CONFIGURATION },
            }),
        },
        Call {
            operation: "UpdateTelemetryPipeline (by name)",
            uri: "/UpdateTelemetryPipeline",
            body: json!({ "PipelineIdentifier": name, "Configuration": configuration }),
        },
        Call {
            operation: "UpdateTelemetryPipeline (by ARN)",
            uri: "/UpdateTelemetryPipeline",
            body: json!({ "PipelineIdentifier": arn, "Configuration": configuration }),
        },
        Call {
            operation: "DeleteTelemetryPipeline (by name)",
            uri: "/DeleteTelemetryPipeline",
            body: json!({ "PipelineIdentifier": name }),
        },
        Call {
            operation: "DeleteTelemetryPipeline (by ARN)",
            uri: "/DeleteTelemetryPipeline",
            body: json!({ "PipelineIdentifier": arn }),
        },
        Call {
            operation: "ListTelemetryPipelines",
            uri: "/ListTelemetryPipelines",
            body: json!({}),
        },
    ]
}

/// The emulator under test: ONE engine behind TWO routers — the authenticating one every
/// row drives, and the open one the rest of this crate's suites drive, which is how a case
/// seeds a pipeline and reads back what a refused request did or did not change.
struct Emulator {
    authenticating: MultiServiceRouter,
    open: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut authenticating = MultiServiceRouter::new();
        authenticating.set_request_authentication(true);
        authenticating.register_service(EmulatedService::new(Arc::clone(&engine)));
        let mut open = MultiServiceRouter::new();
        open.set_request_authentication(false);
        open.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self {
            authenticating,
            open,
            engine,
        }
    }

    /// Send `call` to the authenticating router as `presentation` says.
    async fn refused(&self, presentation: &Presentation, call: &Call) -> Reply {
        let mut builder = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://{ENDPOINT_HOST}{uri}{query}",
                uri = call.uri,
                query = presentation.query
            ))
            .header("host", ENDPOINT_HOST)
            .header("content-type", "application/json");
        if let Some(authorization) = &presentation.authorization {
            builder = builder.header("authorization", authorization.clone());
        }
        if let Some(date) = presentation.amz_date {
            builder = builder.header("x-amz-date", date);
        }
        let request = builder
            .body(bytes::Bytes::from(
                serde_json::to_vec(&call.body).expect("the request body serializes"),
            ))
            .expect("a well-formed test request");
        read(self.authenticating.dispatch(request).await)
    }

    /// Send one request to the OPEN router, which this crate's other suites drive: a
    /// request the verification layer is not asked about.
    async fn served(&self, uri: &str, body: &Value) -> Reply {
        let request = http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("host", ENDPOINT_HOST)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDOBSTEST/{SCOPE_DATE}/{REGION}/\
                     observabilityadmin/aws4_request, SignedHeaders=host, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed test request");
        read(self.open.dispatch(request).await)
    }

    /// Create one pipeline through the open router and answer its name and its ARN.
    async fn seed(&self, name: &'static str) -> (&'static str, String) {
        let created = self
            .served(
                "/CreateTelemetryPipeline",
                &json!({ "Name": name, "Configuration": { "Body": CONFIGURATION } }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "setup: CreateTelemetryPipeline({name}) must answer 200, got {created:?}"
        );
        let arn = created
            .body
            .get("Arn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("setup: the create reports the minted Arn, got {created:?}"))
            .to_string();
        (name, arn)
    }

    /// The configuration body the store holds for `identifier`, asked of the store itself.
    fn stored_configuration(&self, identifier: &str) -> Option<String> {
        let state = self.engine.state::<Store>();
        let store = state.lock();
        store
            .pipeline(REGION, identifier)
            .map(|record| record.configuration_body.clone())
    }

    /// How many pipelines the seeded region holds.
    fn stored_count(&self) -> usize {
        self.engine.state::<Store>().lock().pipeline_count(REGION)
    }
}

/// Read one response strictly.
fn read(response: http::Response<bytes::Bytes>) -> Reply {
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let body = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    Reply {
        status,
        error_type,
        body,
    }
}

/// RULE: a caller the authentication layer cannot verify is refused by the front door —
/// HTTP 401, `x-amzn-errortype: UnauthorizedException`, body exactly
/// `{"message":"Unauthorized"}` — whatever operation it names and whatever its payload
/// carries, and the refusal reaches no handler: nothing it would have written is applied.
#[tokio::test]
async fn every_caller_the_layer_cannot_verify_is_refused_by_the_front_door() {
    let emulator = Emulator::new();
    let (name, arn) = emulator.seed("front-door-pipeline").await;

    for presentation in presentations() {
        for call in &calls(name, &arn) {
            let case = format!("{} -> {}", presentation.what, call.operation);
            let reply = emulator.refused(&presentation, call).await;
            assert!(
                reply.is_unauthorized(),
                "case {case:?}: expected {UNAUTHORIZED_STATUS} \
                 {{\"message\":\"{UNAUTHORIZED_MESSAGE}\"}} with \
                 x-amzn-errortype: {UNAUTHORIZED_CODE}, got {reply:?}"
            );
            assert!(
                reply.body.get("Message").is_none() && reply.body.get("__type").is_none(),
                "case {case:?}: the refusal is none of this service's modelled errors, \
                 got {reply:?}"
            );
        }
    }

    // No refused request reached a handler: the seeded pipeline still carries the
    // configuration the create wrote, and the refused creates stored nothing.
    let stored = emulator.stored_configuration(&arn);
    assert_eq!(
        stored.as_deref(),
        Some(CONFIGURATION),
        "a refused mutation must change nothing, got {stored:?}"
    );
    assert_eq!(
        emulator.stored_count(),
        1,
        "a refused create must store nothing, so the seeded pipeline is the only record"
    );
}

/// RULE: the refusal is decided from the credentials alone, so it is byte-identical
/// whatever the request says — the same bytes for an operation that would have been served,
/// for one whose target does not exist, and for one whose payload the model would refuse.
#[tokio::test]
async fn the_refusal_is_identical_whatever_the_request_says() {
    let emulator = Emulator::new();
    let (name, arn) = emulator.seed("identical-refusal").await;
    let presentation = header_signed(
        "an access key the emulator never minted",
        "AWS4-HMAC-SHA256",
        &format!("AKIDEXAMPLE/{SCOPE_DATE}/{REGION}/observabilityadmin/aws4_request"),
        Some(AMZ_DATE),
    );

    // Rows: a mutation that WOULD have been served, the same one on a pipeline that does
    // not exist, one carrying a payload the model's own constraints refuse (no
    // `Configuration` at all, and an identifier past its 1..512 length), and a read.
    let rows: Vec<Call> = vec![
        Call {
            operation: "a delete that would have been served",
            uri: "/DeleteTelemetryPipeline",
            body: json!({ "PipelineIdentifier": arn }),
        },
        Call {
            operation: "a delete of a pipeline that does not exist",
            uri: "/DeleteTelemetryPipeline",
            body: json!({ "PipelineIdentifier": "never-created" }),
        },
        Call {
            operation: "an update missing its required Configuration",
            uri: "/UpdateTelemetryPipeline",
            body: json!({ "PipelineIdentifier": name }),
        },
        Call {
            operation: "a delete whose identifier is past the modelled maximum length",
            uri: "/DeleteTelemetryPipeline",
            body: json!({ "PipelineIdentifier": "p".repeat(513) }),
        },
        Call {
            operation: "a read",
            uri: "/GetTelemetryPipeline",
            body: json!({ "PipelineIdentifier": name }),
        },
    ];

    let mut answers = Vec::new();
    for call in &rows {
        let reply = emulator.refused(&presentation, call).await;
        assert!(
            reply.is_unauthorized(),
            "case {:?}: expected the front door's refusal, got {reply:?}",
            call.operation
        );
        answers.push((call.operation, reply));
    }
    let (first_case, first) = &answers[0];
    for (case, reply) in &answers[1..] {
        assert_eq!(
            reply, first,
            "case {case:?}: the refusal must be identical to {first_case:?}, got {reply:?} \
             against {first:?}"
        );
    }
}

/// RULE: the 401 is the verification layer's verdict, not the operation's. The very same
/// requests, over the same engine through the router this crate's other suites drive — the
/// one the layer is not asked about — reach the service and earn its OWN answers: the
/// modelled empty 200 for a mutation it serves, and the modelled 404
/// `ResourceNotFoundException` for an identifier naming no pipeline.
#[tokio::test]
async fn the_same_requests_reach_the_service_when_the_layer_is_not_asked() {
    let emulator = Emulator::new();
    let (name, arn) = emulator.seed("served-pipeline").await;

    let updated = emulator
        .served(
            "/UpdateTelemetryPipeline",
            &json!({
                "PipelineIdentifier": name,
                "Configuration": { "Body": UPDATED_CONFIGURATION },
            }),
        )
        .await;
    assert_eq!(
        updated.status, 200,
        "case \"an update of the seeded pipeline\": expected the modelled 200, got {updated:?}"
    );
    assert_eq!(
        emulator.stored_configuration(&arn).as_deref(),
        Some(UPDATED_CONFIGURATION),
        "case \"an update of the seeded pipeline\": the accepted body is what the store holds"
    );

    let missing = emulator
        .served(
            "/DeleteTelemetryPipeline",
            &json!({ "PipelineIdentifier": "never-created" }),
        )
        .await;
    assert_eq!(
        (missing.status, missing.error_type.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case \"a delete of a pipeline that does not exist\": expected the modelled 404, \
         got {missing:?}"
    );

    let deleted = emulator
        .served(
            "/DeleteTelemetryPipeline",
            &json!({ "PipelineIdentifier": arn }),
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case \"a delete of the seeded pipeline\": expected the modelled 200, got {deleted:?}"
    );
    assert_eq!(
        emulator.stored_count(),
        0,
        "case \"a delete of the seeded pipeline\": the record is gone from the store"
    );
}
