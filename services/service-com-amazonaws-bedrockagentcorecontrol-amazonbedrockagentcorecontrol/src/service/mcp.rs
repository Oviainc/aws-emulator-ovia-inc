// SPDX-License-Identifier: CC0-1.0

//! The MCP wire surface a gateway publishes: the endpoint `CreateGateway` hands back in
//! `gatewayUrl`, and what answers on it.
//!
//! # Not a modelled operation
//!
//! Every operation in this service's model is control plane: it creates, reads, updates
//! and deletes gateways and the targets configured on them. What an MCP client then does
//! with the URL `CreateGateway` / `GetGateway` hand back is the Model Context Protocol
//! itself — a JSON-RPC 2.0 `POST` to that URL, whose path is the fixed `/mcp` transport
//! path of the gateway's host ([`crate::state::GATEWAY_MCP_PATH`], which
//! [`crate::state::gateway_url`] publishes as part of the endpoint), carrying
//! `initialize`, `tools/list` and
//! `tools/call`. None of those is a modelled operation and none carries an `X-Amz-Target`:
//! they are addressed by host, path and method, so nothing in the generated routing table
//! can name them and they are reached through the seams
//! [`crate::service::EmulatedService`] has for a request the model's routing key cannot
//! name.
//!
//! # What is served
//!
//! * `initialize` — the protocol version this surface speaks and the one capability it
//!   has, `tools`.
//! * `tools/list` — the tools of the gateway's `READY` targets, aggregated under the
//!   qualified name a gateway reports them as, `<targetName>___<toolName>`
//!   ([`crate::state::qualified_tool_name`]). Only tools a target DECLARES are listed:
//!   the inline tool schema of a Lambda MCP target. A target whose schema lives in S3,
//!   and every other target shape, declares none here, so nothing is listed for it.
//! * `tools/call` — translated into an invocation of the target's own `lambdaArn`, through
//!   the in-process callback into this same emulator
//!   ([`crate::state::invoke_lambda_tool`]). The tool's result is what the function
//!   returned; an invocation Lambda refused is reported as a tool error carrying that
//!   answer, never as a result nothing produced.
//! * `ping` — the JSON-RPC liveness call, an empty result.
//!
//! A JSON-RPC notification is answered `202` with no body, as a request that expects no
//! response must be.
//!
//! # The two error envelopes
//!
//! A failure of the JSON-RPC exchange itself — a method this surface does not serve,
//! parameters it cannot read — is a JSON-RPC `error` object, which is what an MCP client
//! branches on. A failure of the ENDPOINT, before any JSON-RPC is read — a host no gateway
//! publishes, a path it does not serve, a request its inbound authorizer refuses — is the
//! AWS error envelope of this service's `restJson1` protocol, because that is the layer it
//! happens at.

use std::sync::Arc;
use std::time::SystemTime;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use serde_json::{json, Map, Value};
use smithy_server_runtime::{http, BoxFuture, ErasedOperation, OperationContext, Service};

use crate::ops::types::{SchemaDefinition, ToolDefinition};
use crate::state;

/// The media type JSON-RPC travels as on this wire, and the one every answer here
/// carries.
const JSON_CONTENT_TYPE: &str = "application/json";

/// The JSON-RPC version every request and response on this wire declares (JSON-RPC 2.0
/// §4: "A String specifying the version of the JSON-RPC protocol. MUST be exactly
/// "2.0"").
const JSONRPC_VERSION: &str = "2.0";

/// The MCP protocol revision this surface speaks, which `initialize` answers with and the
/// recorded client declares in its `MCP-Protocol-Version` header.
const MCP_PROTOCOL_VERSION: &str = "2025-03-26";

/// The version reported in `initialize`'s `serverInfo`, which the MCP `Implementation`
/// object requires beside the name. A gateway publishes no version of its own anywhere in
/// this service's model, so this is the version of the surface serving it; the NAME is the
/// gateway's own.
const SERVER_VERSION: &str = "1.0.0";

/// JSON-RPC 2.0 §5.1 error codes: the reserved ones this surface answers with.
const PARSE_ERROR: i64 = -32700;
/// See [`PARSE_ERROR`].
const INVALID_REQUEST: i64 = -32600;
/// See [`PARSE_ERROR`].
const METHOD_NOT_FOUND: i64 = -32601;
/// See [`PARSE_ERROR`].
const INVALID_PARAMS: i64 = -32602;

/// The MCP methods this surface serves.
const METHOD_INITIALIZE: &str = "initialize";
/// See [`METHOD_INITIALIZE`].
const METHOD_PING: &str = "ping";
/// See [`METHOD_INITIALIZE`].
const METHOD_TOOLS_LIST: &str = "tools/list";
/// See [`METHOD_INITIALIZE`].
const METHOD_TOOLS_CALL: &str = "tools/call";

/// The one method this surface is addressed with. MCP's HTTP transport reaches a server's
/// JSON-RPC endpoint with `POST`; the `GET` of its streaming transport opens a
/// server-to-client event stream, which this surface does not serve, and the transport
/// states `405` for a `GET` a server does not support.
const ALLOWED_METHOD: &str = "POST";

/// This service's `restJson1` error codes for a failure of the endpoint itself. Both are
/// modelled errors of the gateway's control plane
/// (`smithy_query operation=GetGateway,CreateGateway`), reported here at the same statuses
/// the model gives them.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";
/// See [`RESOURCE_NOT_FOUND`].
const ACCESS_DENIED: &str = "AccessDeniedException";
/// See [`RESOURCE_NOT_FOUND`].
const INTERNAL_SERVER: &str = "InternalServerException";

/// The wire error code this emulator answers with for a surface nobody has implemented
/// yet ([`crate::scaffolded`]).
const NOT_IMPLEMENTED: &str = "NotImplemented";

/// What a request that addresses no gateway endpoint of this emulator is told. It carries
/// nothing from the request: a caller branches on the status and the code.
const NO_GATEWAY_HERE: &str = "No READY gateway of this account serves this endpoint";

/// What a request to a path on a gateway host that publishes nothing is told.
const NO_SUCH_PATH: &str = "This gateway serves no resource at this path";

/// What a request the gateway's inbound authorizer refuses is told.
const NOT_AUTHORIZED: &str =
    "This gateway authorizes inbound requests with AWS_IAM; the request carries no \
     Amazon Web Services credential";

/// The auth schemes a request to a published gateway endpoint is verified under.
///
/// SigV4 is the only scheme this service's model declares. This is NOT a modelled
/// operation, so there is no modelled operation for the router to read schemes off:
/// without this the router would find no scheme for such a request and could not verify
/// the credential it carries. Whether a presented signature is CORRECT is the emulator's
/// own check, never this service's.
const DATA_PLANE_AUTH_SCHEMES: &[&str] = &[::emulator_server::aws_credentials::SIGV4_SCHEME];

/// The longest run of a caller's own text any message here quotes back.
const MAX_QUOTED_LEN: usize = 128;

/// One gateway's MCP surface, as the one [`ErasedOperation`] the service holds for it.
///
/// Owns the shared engine exactly as a generated operation does, so every answer is
/// computed from the SAME per-service store the control plane's handlers write: the tools
/// a gateway lists are the tools of the targets `CreateGatewayTarget` persisted on it, and
/// a gateway `DeleteGateway` removed serves nothing.
pub struct GatewayMcpSurface<Eng> {
    engine: Arc<Eng>,
}

impl<Eng> GatewayMcpSurface<Eng> {
    /// Build the surface from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for GatewayMcpSurface<Eng> {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move { self.serve(req, ctx).await })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        Some(DATA_PLANE_AUTH_SCHEMES)
    }
}

impl<Eng: Send + Sync + 'static> GatewayMcpSurface<Eng> {
    /// Serve one request to a published gateway endpoint.
    ///
    /// The host names the gateway and its Region, the path names the MCP surface, and the
    /// JSON-RPC body names the interaction — which is how an MCP server is addressed;
    /// there is no operation name on this wire.
    pub async fn serve(&self, req: &RtRequest, ctx: &OperationContext<'_>) -> RtResponse {
        let Some((gateway_id, region)) = host_of(req).and_then(state::gateway_endpoint_identity)
        else {
            // Unreachable: the service claims a request for this surface only when its
            // host IS a gateway endpoint host. Answering the envelope rather than
            // panicking keeps the handler total.
            return aws_error(404, RESOURCE_NOT_FOUND, NO_GATEWAY_HERE);
        };
        if !addresses_mcp_path(req) {
            return aws_error(404, RESOURCE_NOT_FOUND, NO_SUCH_PATH);
        }
        if req.method() != ALLOWED_METHOD {
            let mut answer = aws_error(
                405,
                RESOURCE_NOT_FOUND,
                &format!("This MCP endpoint is addressed with {ALLOWED_METHOD}"),
            );
            answer.headers_mut().insert("allow", ALLOWED_METHOD);
            return answer;
        }

        let Some(store) = state::service_state(&*self.engine) else {
            return aws_error(500, INTERNAL_SERVER, "the gateway store is unavailable");
        };
        // The gateway is read at THIS instant, with the transitions the clock already
        // made due materialised first: the `CREATING` -> `READY` the create owed it, and
        // the removal an accepted deletion owed it. That is what makes a client which
        // polled until `READY` able to drive the endpoint, and a deleted gateway stop
        // serving. The snapshot is owned, so the store's lock is released before the
        // tool call below reaches Lambda.
        let now = SystemTime::now();
        let serving = {
            let mut store = store.lock();
            store.expire_deleted_gateways_and_targets(now);
            store
                .gateway_on_endpoint_host(region, gateway_id)
                .map(|gateway| ServingGateway::of(gateway, now))
        };
        let Some(gateway) = serving.filter(ServingGateway::serves) else {
            return aws_error(404, RESOURCE_NOT_FOUND, NO_GATEWAY_HERE);
        };
        if let Some(refusal) = gateway.refuse_unauthorized(req) {
            return refusal;
        }

        let body = req.body().bytes().unwrap_or_default();
        let message: Value = match ::serde_json::from_slice(body) {
            Ok(message) => message,
            Err(error) => {
                return jsonrpc_error(
                    400,
                    Value::Null,
                    PARSE_ERROR,
                    &format!("Parse error: {}", bounded(&error.to_string())),
                )
            }
        };
        let Some(request) = JsonRpcRequest::of(&message) else {
            return jsonrpc_error(
                400,
                message.get("id").cloned().unwrap_or(Value::Null),
                INVALID_REQUEST,
                "Invalid Request: expected a JSON-RPC 2.0 request object carrying a method",
            );
        };
        // "A Notification is a Request object without an "id" member. […] The Server MUST
        // NOT reply to a Notification" (JSON-RPC 2.0 §4.1), so there is no response to
        // write and the exchange is simply accepted.
        let Some(id) = request.id else {
            return accepted();
        };

        match request.method {
            METHOD_INITIALIZE => jsonrpc_result(id, gateway.initialize_result()),
            METHOD_PING => jsonrpc_result(id, json!({})),
            METHOD_TOOLS_LIST => jsonrpc_result(id, gateway.tools_list_result()),
            METHOD_TOOLS_CALL => match gateway.tool_call(request.params) {
                Ok(call) => {
                    let outcome = state::invoke_lambda_tool(
                        ctx,
                        &call.lambda_arn,
                        call.payload,
                        Some(call.client_context),
                    )
                    .await;
                    jsonrpc_result(id, tool_call_result(outcome))
                }
                Err(message) => jsonrpc_error(200, id, INVALID_PARAMS, &message),
            },
            // Everything else, including a notification method that arrived WITH an id:
            // a request for something that has no response is no method to call.
            other => jsonrpc_error(
                200,
                id,
                METHOD_NOT_FOUND,
                &format!("Method not found: {}", bounded(other)),
            ),
        }
    }
}

/// Whether a request's path names the gateway's MCP endpoint — the one resource a
/// gateway endpoint host publishes.
///
/// A trailing slash is the same resource: the `gatewayUrl` a client holds already ends in
/// this sub-path, and a client that appended its own separator sends `/mcp/`.
fn addresses_mcp_path(req: &RtRequest) -> bool {
    let path = ::service_common::request_path(req.uri());
    path.strip_suffix('/').unwrap_or(path) == state::GATEWAY_MCP_PATH
}

/// One gateway as this surface reads it: everything an answer is computed from, owned, so
/// the store's lock is not held across the Lambda invocation a tool call makes.
#[derive(Debug)]
struct ServingGateway {
    /// The gateway's own name, which `initialize` reports as the server's name.
    name: String,
    /// The status the clock owes the gateway at the instant it was read.
    status: String,
    /// The protocol the gateway speaks, which decides whether it has an MCP surface at
    /// all.
    protocol_type: Option<String>,
    /// How the gateway authorizes an inbound request (`AuthorizerType`).
    authorizer_type: String,
    /// Every tool the gateway's `READY` targets declare, in the deterministic order the
    /// target store holds them in.
    tools: Vec<GatewayTool>,
}

/// One tool of one target, as the gateway aggregates it.
#[derive(Debug)]
struct GatewayTool {
    /// `<targetName>___<toolName>`, the name the gateway reports and a caller calls.
    qualified_name: String,
    /// The tool's own name on its target, which the target's handler is told.
    tool_name: String,
    /// The tool as its target declares it.
    definition: ToolDefinition,
    /// The function the tool's call is an invocation of.
    lambda_arn: String,
}

/// A `tools/call` resolved against the gateway's targets: the function to invoke and what
/// to invoke it with.
#[derive(Debug)]
struct ToolCall {
    lambda_arn: String,
    /// The event the function is invoked with: the call's own `arguments` object.
    payload: Vec<u8>,
    /// Lambda's `ClientContext`, carrying the name of the tool being called, so a handler
    /// serving several tools can tell which one it was asked for.
    client_context: String,
}

impl ServingGateway {
    /// Snapshot `gateway` as it stands at `now`.
    fn of(gateway: &state::GatewayRecord, now: SystemTime) -> Self {
        let mut tools = Vec::new();
        for target in gateway.targets.values() {
            // A target that is not READY is not serving: its tools are not aggregated
            // into the gateway's listing, and a call naming one of them resolves to no
            // tool.
            if target.status_at(now) != state::STATUS_READY {
                continue;
            }
            let configuration = &target.target_configuration;
            let Some(lambda_arn) = state::mcp_lambda_arn(configuration) else {
                continue;
            };
            let Some(declared) = state::inline_tool_definitions(configuration) else {
                continue;
            };
            for definition in declared {
                tools.push(GatewayTool {
                    qualified_name: state::qualified_tool_name(&target.name, &definition.name),
                    tool_name: definition.name.clone(),
                    definition: definition.clone(),
                    lambda_arn: lambda_arn.to_string(),
                });
            }
        }
        Self {
            name: gateway.name.clone(),
            status: gateway.status_at(now).to_string(),
            protocol_type: gateway.protocol_type.clone(),
            authorizer_type: gateway.authorizer_type.clone(),
            tools,
        }
    }

    /// Whether this gateway serves an MCP surface at all: a gateway reaches its endpoint
    /// when it is `READY`, and the surface it then serves is the protocol it was created
    /// with.
    fn serves(&self) -> bool {
        self.status == state::STATUS_READY
            && self.protocol_type.as_deref() == Some(state::PROTOCOL_TYPE_MCP)
    }

    /// The refusal this gateway's inbound authorizer answers `req` with, or `None` when
    /// it admits the request.
    fn refuse_unauthorized(&self, req: &RtRequest) -> Option<RtResponse> {
        match self.authorizer_type.as_str() {
            // "AWS_IAM - Authorize with your Amazon Web Services IAM credentials"
            // (`AuthorizerType`'s own documentation), so a request that presents none is
            // refused before anything is served. Only PRESENCE is read here: the
            // signature over a presented credential is verified by the emulator itself.
            state::AUTHORIZER_TYPE_AWS_IAM => {
                (!carries_sigv4_credential(req)).then(|| aws_error(403, ACCESS_DENIED, NOT_AUTHORIZED))
            }
            // "NONE - No authorization".
            state::AUTHORIZER_TYPE_NONE => None,
            // `CUSTOM_JWT` ("Authorize with a bearer token") and `AUTHENTICATE_ONLY` admit
            // a request on a bearer token this emulator verifies no signature of, so
            // there is nothing here that can tell an authorized caller from any other
            // one, and admitting every caller would report an authorization nothing
            // performed.
            other => Some(aws_error(
                501,
                NOT_IMPLEMENTED,
                &format!(
                    "inbound {} authorization of a gateway's MCP endpoint is not implemented in \
                     this emulator",
                    bounded(other)
                ),
            )),
        }
    }

    /// `initialize`'s result: the protocol revision this surface speaks, the one
    /// capability it has, and which server answered.
    fn initialize_result(&self) -> Value {
        json!({
            "protocolVersion": MCP_PROTOCOL_VERSION,
            // The tools of a gateway change when a target is created, deleted or
            // synchronized, and nothing here notifies a connected client of that, so the
            // capability is declared without `listChanged`.
            "capabilities": {"tools": {}},
            "serverInfo": {"name": self.name, "version": SERVER_VERSION},
        })
    }

    /// `tools/list`'s result: every tool of the gateway's `READY` targets, under the
    /// qualified name the gateway reports it as.
    fn tools_list_result(&self) -> Value {
        let tools: Vec<Value> = self
            .tools
            .iter()
            .map(|tool| {
                let mut entry = Map::new();
                entry.insert("name".to_string(), json!(tool.qualified_name));
                entry.insert(
                    "description".to_string(),
                    json!(tool.definition.description),
                );
                entry.insert(
                    "inputSchema".to_string(),
                    json_schema(&tool.definition.input_schema),
                );
                // Reported only for a tool whose target declares one: `outputSchema` is
                // optional on `ToolDefinition`.
                if let Some(output) = &tool.definition.output_schema {
                    entry.insert("outputSchema".to_string(), json_schema(output));
                }
                Value::Object(entry)
            })
            .collect();
        json!({"tools": tools})
    }

    /// Resolve a `tools/call` against this gateway's tools, or say why it names none.
    fn tool_call(&self, params: Option<&Value>) -> Result<ToolCall, String> {
        let name = params
            .and_then(|params| params.get("name"))
            .and_then(Value::as_str)
            .ok_or_else(|| "Invalid params: tools/call requires a tool name".to_string())?;
        let tool = self
            .tools
            .iter()
            .find(|tool| tool.qualified_name == name)
            .ok_or_else(|| {
                format!(
                    "Invalid params: this gateway serves no tool named {}",
                    bounded(name)
                )
            })?;
        // "arguments" is the tool's input object, which is what the target's handler is
        // invoked with; a call that carries none invokes it with an empty one.
        let arguments = params
            .and_then(|params| params.get("arguments"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        if !arguments.is_object() {
            return Err("Invalid params: tools/call arguments must be an object".to_string());
        }
        Ok(ToolCall {
            lambda_arn: tool.lambda_arn.clone(),
            payload: arguments.to_string().into_bytes(),
            client_context: tool_client_context(&tool.tool_name),
        })
    }
}

/// The `ClientContext` a tool invocation carries, so the function knows which of its
/// tools was called: a base64 JSON object whose `custom` member names the tool, which is
/// where a Lambda handler reads client context from.
fn tool_client_context(tool_name: &str) -> String {
    let context = json!({"custom": {TOOL_NAME_CONTEXT_KEY: tool_name}});
    ::aws_smithy_types::base64::encode(context.to_string())
}

/// The `clientContext.custom` key the called tool's name travels in.
const TOOL_NAME_CONTEXT_KEY: &str = "bedrockAgentCoreToolName";

/// A `tools/call` result built from what Lambda answered.
///
/// MCP distinguishes a protocol failure (a JSON-RPC `error`) from a TOOL failure, which
/// "should be reported inside the result object" with `isError` set, so the model driving
/// the client can see it: a function that failed, and an invocation Lambda refused, are
/// both the latter.
fn tool_call_result(outcome: Result<state::LambdaInvocation, String>) -> Value {
    let (text, is_error) = match outcome {
        Ok(invocation) => {
            let text = payload_text(&invocation.payload);
            match invocation.function_error {
                Some(error) => (format!("{error}: {text}"), true),
                None => (text, false),
            }
        }
        Err(refusal) => (refusal, true),
    };
    json!({"content": [{"type": "text", "text": text}], "isError": is_error})
}

/// The function's response payload as the text content of a tool result: the string the
/// function returned when it returned one, and the JSON document it returned otherwise.
fn payload_text(payload: &[u8]) -> String {
    match ::serde_json::from_slice::<Value>(payload) {
        Ok(Value::String(text)) => text,
        Ok(document) => document.to_string(),
        // Not JSON at all: the bytes as they arrived, which is what the function wrote.
        Err(_) => String::from_utf8_lossy(payload).to_string(),
    }
}

/// One tool's `inputSchema` / `outputSchema` as the JSON Schema object MCP carries, built
/// from the [`SchemaDefinition`] the target declared.
///
/// Every member of the modelled shape is a JSON Schema keyword of the same name
/// (`smithy_query shape=SchemaDefinition`: `type`, `properties`, `required`, `items`,
/// `description`), so the projection is member for member and adds nothing. Properties
/// are written in sorted key order, so one stored target always renders one document.
fn json_schema(schema: &SchemaDefinition) -> Value {
    json_schema_to_depth(schema, 0)
}

/// How deep a declared schema is projected. A schema arrives as the JSON body of
/// `CreateGatewayTarget`, whose own parser bounds nesting well below this, so no stored
/// schema reaches it; it is here so this walk stays total — and bounded on the request
/// thread — whatever a target holds.
const MAX_SCHEMA_DEPTH: usize = 64;

/// [`json_schema`] at nesting level `depth`: past [`MAX_SCHEMA_DEPTH`] only the type is
/// projected, so the walk cannot recurse without bound.
fn json_schema_to_depth(schema: &SchemaDefinition, depth: usize) -> Value {
    let mut document = Map::new();
    document.insert("type".to_string(), json!(schema.r#type));
    if depth >= MAX_SCHEMA_DEPTH {
        return Value::Object(document);
    }
    if let Some(description) = &schema.description {
        document.insert("description".to_string(), json!(description));
    }
    if let Some(properties) = &schema.properties {
        let mut rendered = Map::new();
        let mut names: Vec<&String> = properties.keys().collect();
        names.sort();
        for name in names {
            if let Some(property) = properties.get(name) {
                rendered.insert(name.clone(), json_schema_to_depth(property, depth + 1));
            }
        }
        document.insert("properties".to_string(), Value::Object(rendered));
    }
    if let Some(required) = &schema.required {
        document.insert("required".to_string(), json!(required));
    }
    if let Some(items) = &schema.items {
        document.insert("items".to_string(), json_schema_to_depth(items, depth + 1));
    }
    Value::Object(document)
}

/// One JSON-RPC request message, as this surface reads it.
#[derive(Debug)]
struct JsonRpcRequest<'m> {
    /// The id to answer under, or `None` for a notification, which is not answered.
    id: Option<Value>,
    method: &'m str,
    params: Option<&'m Value>,
}

impl<'m> JsonRpcRequest<'m> {
    /// `message` as a JSON-RPC 2.0 request, or `None` when it is not one: a batch, a
    /// scalar, a missing or non-string `method`, or a `jsonrpc` member that is not
    /// exactly `"2.0"`.
    fn of(message: &'m Value) -> Option<Self> {
        let object = message.as_object()?;
        if object.get("jsonrpc").and_then(Value::as_str) != Some(JSONRPC_VERSION) {
            return None;
        }
        let method = object.get("method").and_then(Value::as_str)?;
        // "An identifier established by the Client that MUST contain a String, Number, or
        // NULL value if included" — and MCP forbids the null id, so a message carrying
        // one is read as a notification rather than answered under it.
        let id = object
            .get("id")
            .filter(|id| id.is_string() || id.is_number())
            .cloned();
        Some(Self {
            id,
            method,
            params: object.get("params"),
        })
    }
}

/// A JSON-RPC response carrying a result, at HTTP 200: the exchange succeeded, whatever
/// the tool behind it reported.
fn jsonrpc_result(id: Value, result: Value) -> RtResponse {
    json_response(
        200,
        &json!({"jsonrpc": JSONRPC_VERSION, "id": id, "result": result}),
    )
}

/// A JSON-RPC response carrying an error object (JSON-RPC 2.0 §5.1).
fn jsonrpc_error(status: u16, id: Value, code: i64, message: &str) -> RtResponse {
    json_response(
        status,
        &json!({
            "jsonrpc": JSONRPC_VERSION,
            "id": id,
            "error": {"code": code, "message": message},
        }),
    )
}

/// The answer to a notification: accepted, with no body, because a notification has no
/// response.
fn accepted() -> RtResponse {
    RtResponse::new(
        StatusCode::try_from(202).expect("202 is a valid status"),
        SdkBody::empty(),
    )
}

/// A failure of the endpoint itself, in this service's `restJson1` error envelope: the
/// `__type` a client branches on, the header the protocol carries it in, and a sentence
/// that quotes nothing from the request.
fn aws_error(status: u16, code: &str, message: &str) -> RtResponse {
    let mut answer = json_response(status, &json!({"__type": code, "message": message}));
    answer
        .headers_mut()
        .insert("x-amzn-errortype", code.to_string());
    answer
}

/// One JSON answer on this wire.
fn json_response(status: u16, body: &Value) -> RtResponse {
    let mut answer = RtResponse::new(
        StatusCode::try_from(status).expect("the status is a constant of this module"),
        SdkBody::from(body.to_string()),
    );
    answer
        .headers_mut()
        .insert("content-type", JSON_CONTENT_TYPE);
    answer
}

/// `value` as a message quotes it: ASCII graphic characters only and at most
/// [`MAX_QUOTED_LEN`] of them, so a message cannot carry control characters or an
/// unbounded run of a caller's own text.
fn bounded(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_graphic() || *c == ' ')
        .take(MAX_QUOTED_LEN)
        .collect()
}

/// Whether `req` addresses a gateway endpoint host — one of the hosts this service
/// PUBLISHES in `gatewayUrl`.
pub fn addresses_gateway_endpoint(req: &RtRequest) -> bool {
    host_of(req)
        .and_then(state::gateway_endpoint_identity)
        .is_some()
}

/// Whether a buffered request addresses a gateway endpoint host.
///
/// Reads the `Host` header an HTTP/1 request carries, else the authority an HTTP/2
/// request carries on its URI (`http` parses `:authority` onto the URI rather than
/// exposing it as a header).
pub fn addresses_gateway_endpoint_parts(parts: &http::request::Parts) -> bool {
    let host = match parts.headers.get("host").and_then(|host| host.to_str().ok()) {
        Some(host) => host,
        None => match parts.uri.authority() {
            Some(authority) => authority.as_str(),
            None => return false,
        },
    };
    state::gateway_endpoint_identity(host).is_some()
}

/// The host a runtime-API request addresses: its `Host` header, else the authority of its
/// absolute-form URI.
fn host_of(req: &RtRequest) -> Option<&str> {
    match req.headers().get("host") {
        Some(host) => Some(host),
        None => authority_of(req.uri()),
    }
}

/// The authority of an absolute request URI, or `None` when the URI is origin-form.
fn authority_of(uri: &str) -> Option<&str> {
    let (_scheme, rest) = uri.split_once("://")?;
    Some(match rest.find('/') {
        Some(path_start) => &rest[..path_start],
        None => rest,
    })
}

/// Whether the request presents a SigV4 credential at all — in the `Authorization` header
/// a signed SDK request carries, or in the `X-Amz-Credential` query parameter a presigned
/// URL carries.
///
/// Only PRESENCE is read. Whether the signature over that credential is correct is the
/// emulator's own check, never a service's.
pub fn carries_sigv4_credential(req: &RtRequest) -> bool {
    if req
        .headers()
        .get("authorization")
        .is_some_and(|value| value.contains("Credential="))
    {
        return true;
    }
    request_query(req.uri())
        .split('&')
        .any(|pair| pair.starts_with("X-Amz-Credential="))
}

/// The query component of a request URI: everything after the first `?`.
fn request_query(uri: &str) -> &str {
    match uri.find('?') {
        Some(mark) => &uri[mark + 1..],
        None => "",
    }
}
