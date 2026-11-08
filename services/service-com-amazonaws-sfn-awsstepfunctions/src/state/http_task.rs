// SPDX-License-Identifier: CC0-1.0

//! The HTTP Task (`arn:aws:states:::http:invoke`): the one `Task` state whose unit of
//! work is an HTTP REQUEST to an endpoint the state's own parameters name, rather than
//! an API call to an operation of another service ([`super::integration`]).
//!
//! A `Task` state "represents a single unit of work performed by a state machine", and
//! for this `Resource` that unit of work is one request: the state's effective input —
//! the document its `Parameters` payload template builds — SAYS the request, member by
//! member (`ApiEndpoint`, `Method`, `Headers`, `QueryParameters`, `RequestBody`,
//! `Transform`), and what the endpoint ANSWERS is the state's result (`StatusCode`,
//! `StatusText`, `Headers`, `ResponseBody`). That is the rule this module serves, and
//! it is served for real: the request is handed to the in-process transport every
//! emulated service reaches another by
//! ([`smithy_server_runtime::EmbeddedEmulator::http_connector`]), so what answers is
//! whichever service of this emulator CLAIMS that endpoint — a Lambda function URL, an
//! api stage, any data plane this emulator publishes — and the status, headers and body
//! the task reports are that endpoint's own. Nothing here synthesises a response.
//!
//! ## The EventBridge connection
//!
//! `Authentication.ConnectionArn` names an EventBridge connection, which "defines the
//! authorization type and credentials to use for authorization with an API destination
//! HTTP endpoint" (`CreateConnection`), and the request the task makes carries that
//! authorization: an `API_KEY` connection sends its key under the header name the
//! connection names and a `BASIC` one sends the HTTP basic scheme's own header, exactly
//! as an api-destination invocation through the same connection does — the header is
//! built by EventBridge's own
//! [`::service_com_amazonaws_eventbridge_awsevents::state::invocation::ConnectionSecret::authorization_header`],
//! so one statement of it serves both callers.
//!
//! The connection is read out of EventBridge's OWN store, through the shared engine
//! ([`connections`]), which is where its state and its credentials live: what decides
//! this task is therefore the record EventBridge's handlers wrote rather than a second
//! copy of it. A `ConnectionArn` that resolves to no connection, and one whose
//! `ConnectionState` is not a state in which it holds authorization
//! ([`AUTHORIZED_CONNECTION_STATES`]), each fail the task BEFORE any request is made —
//! so a deauthorized connection reaches no endpoint at all, whatever that endpoint
//! would have answered.
//!
//! Beside the authorization, what the connection contributes to a request it authorizes
//! is its `InvocationHttpParameters`, "additional parameters for the connection that
//! are passed through with every invocation to the HTTP endpoint": its header and
//! query-string parameters, with the connection's value winning over the task's own
//! where both name one key, because the connection is what authorizes the call.
//!
//! ## Where the request goes
//!
//! Nowhere outside this process. The endpoint is handed to the in-process transport,
//! which is this emulator's own router rather than a socket: no client is built, no
//! name is resolved and no address is dialled. An `ApiEndpoint` naming an endpoint
//! nothing in this emulator serves therefore reaches no endpoint at all — the transport
//! answers the front door's own refusal
//! ([`service_common::in_process_refusal`]), which is not a response from the
//! endpoint that was addressed, so the task reports that nothing was run for it rather
//! than that refusal passed off as an answer.
//!
//! What it carries is as narrow as where it goes. NOTHING of the request being served
//! travels on it: not a header the caller of `StartExecution` sent, and not the
//! emulator's own credentials, which sign the in-process calls of a service integration
//! ([`super::integration`]) and sign nothing here. The only credential on it is the
//! emulated connection's own, which is meaningful to the emulated endpoint it is sent to
//! and to nothing else.
//!
//! ## What this task is NOT judged against
//!
//! The execution role. A `Task` state's integration call is decided against the policies
//! in force on that role over the `(action, resource)` pair the call is subject to
//! ([`super::integration::access_request`], over the one table that states, per
//! integration, WHICH member of the request names the resource it acts on). This task
//! acts on no resource of an AWS service at all — its target is an HTTP endpoint its own
//! parameters name — so no such pair can be formed for it, and the gate the walk carries
//! shows nothing about it, exactly as it shows nothing about an integration no entry
//! names the target of. Nothing is refused on an access request that cannot be stated.
//!
//! ## What the task reports
//!
//! A 2xx answer is the task's RESULT, carrying the status, the registered reason phrase
//! for it, every header the answer held and its body — the body as the document it
//! holds, or as its own text when it is not one. Any other status is the task's
//! FAILURE, under the error name the endpoint's own status gives it
//! ([`HTTP_STATUS_ERROR_PREFIX`]), with that same response document as the cause: the
//! `Retry` and `Catch` of a definition written for one status match on it. A failure of
//! the CONNECTION is named for the connection resource instead
//! ([`CONNECTION_NOT_FOUND_ERROR`], [`INVALID_CONNECTION_STATE_ERROR`]), because no
//! request was made at all.

use ::serde_json::{Map, Value};

use super::integration::{Answer, Failed};

/// The member that names the endpoint the request is made to, an absolute URL.
const API_ENDPOINT_MEMBER: &str = "ApiEndpoint";

/// The member that names the request's HTTP method.
const METHOD_MEMBER: &str = "Method";

/// The member carrying the authorization the request is made under, and the member of
/// it that names the EventBridge connection.
const AUTHENTICATION_MEMBER: &str = "Authentication";
/// See [`AUTHENTICATION_MEMBER`].
const CONNECTION_ARN_MEMBER: &str = "ConnectionArn";

/// The member carrying the headers the request sends, as a map of header name to one
/// value or a list of values.
const HEADERS_MEMBER: &str = "Headers";

/// The member carrying the request's query-string parameters, in the same two forms.
const QUERY_PARAMETERS_MEMBER: &str = "QueryParameters";

/// The member carrying the request's body.
const REQUEST_BODY_MEMBER: &str = "RequestBody";

/// The member carrying how the body is ENCODED, and the two members of it: the encoding
/// itself and the options that encoding takes.
const TRANSFORM_MEMBER: &str = "Transform";
/// See [`TRANSFORM_MEMBER`].
const REQUEST_BODY_ENCODING_MEMBER: &str = "RequestBodyEncoding";
/// See [`TRANSFORM_MEMBER`].
const REQUEST_ENCODING_OPTIONS_MEMBER: &str = "RequestEncodingOptions";
/// See [`TRANSFORM_MEMBER`].
const ARRAY_FORMAT_MEMBER: &str = "ArrayFormat";

/// The members the task's RESULT carries: the status the endpoint answered, the reason
/// phrase registered for that status, the headers it answered and its body.
const STATUS_CODE_MEMBER: &str = "StatusCode";
/// See [`STATUS_CODE_MEMBER`].
const STATUS_TEXT_MEMBER: &str = "StatusText";
/// See [`STATUS_CODE_MEMBER`].
const RESPONSE_BODY_MEMBER: &str = "ResponseBody";

/// The error name a task whose endpoint answered a status outside 2xx reports, closed
/// by that status: the name the HTTP Task gives a non-2xx answer, which is what a
/// definition's `Retry` or `Catch` written for one status matches on.
const HTTP_STATUS_ERROR_PREFIX: &str = "States.Http.StatusCode.";

/// The error name a task whose `Authentication.ConnectionArn` resolves to no connection
/// reports, and the one a task whose connection holds no authorization reports: the two
/// names Step Functions gives a failure of the EventBridge CONNECTION RESOURCE an HTTP
/// Task authenticates through, rather than of the request (which was never made) or of
/// the interpreter.
const CONNECTION_NOT_FOUND_ERROR: &str = "Events.ConnectionResource.ResourceNotFound";
/// See [`CONNECTION_NOT_FOUND_ERROR`].
const INVALID_CONNECTION_STATE_ERROR: &str =
    "Events.ConnectionResource.InvalidConnectionState";

/// The media type a body sent as the JSON document the state built travels under, which
/// is the encoding a task that asks for none sends.
const JSON_MEDIA_TYPE: &str = "application/json";

/// The media type a body the `Transform` asks to URL-ENCODE travels under.
const FORM_MEDIA_TYPE: &str = "application/x-www-form-urlencoded";

/// The two `RequestBodyEncoding` values: the JSON document the state built, sent as it
/// stands, and that document's members as form fields.
const JSON_ENCODING: &str = "JSON";
/// See [`JSON_ENCODING`].
const URL_ENCODED_ENCODING: &str = "URL_ENCODED";

/// The ARN namespace of Amazon EventBridge, which owns the connections this task
/// authenticates through (`awsService.arnNamespace`).
const EVENTS_NAMESPACE: &str = "events";

/// The resource type an EventBridge connection's ARN carries before its name
/// (`arn:aws:events:<region>:<account>:connection/<name>/<id>`, the `ConnectionArn`
/// pattern of
/// `smithy_query(query="shape",shape="DescribeConnectionOutput",model_path="services/
/// service-com-amazonaws-eventbridge-awsevents/smithy-model/
/// com-amazonaws-eventbridge-awsevents.json")`).
const CONNECTION_RESOURCE_PREFIX: &str = "connection/";

/// The `ConnectionState`s in which a connection HOLDS the authorization an invocation
/// through it carries, out of that enum's nine values
/// (`smithy_query(query="shape",shape="ConnectionState",model_path="services/
/// service-com-amazonaws-eventbridge-awsevents/smithy-model/
/// com-amazonaws-eventbridge-awsevents.json")`: `CREATING`, `UPDATING`, `DELETING`,
/// `AUTHORIZED`, `DEAUTHORIZED`, `AUTHORIZING`, `DEAUTHORIZING`, `ACTIVE`,
/// `FAILED_CONNECTIVITY`).
///
/// Every other value is a connection that does not: one still being created,
/// authorized, updated or deleted has not come to rest holding one, and
/// `DEAUTHORIZED`, `DEAUTHORIZING` and `FAILED_CONNECTIVITY` say outright that it does
/// not hold one. A task authenticating through such a connection is failed before any
/// request is made.
const AUTHORIZED_CONNECTION_STATES: &[&str] = &["ACTIVE", "AUTHORIZED"];

/// Make the HTTP request `request` describes and report what the endpoint answered.
///
/// `connections` is what resolves the EventBridge connection the request is authorized
/// through ([`connections`]); `None` is an engine carrying no EventBridge store, so
/// there is no connection to read and no request is made.
pub async fn invoke(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    connections: Option<&Connections>,
    request: &Value,
) -> Answer {
    let asked = Asked::read(request)?;
    // The connection decides whether the request is made AT ALL, so it is resolved
    // first: nothing is sent for a task whose connection does not authorize one.
    let connection = resolved_connection(connections, &asked.connection_arn)?;
    let authorized = AUTHORIZED_CONNECTION_STATES.contains(&connection.state.as_str());
    if !authorized {
        return Err(Failed {
            error: INVALID_CONNECTION_STATE_ERROR.to_string(),
            cause: format!(
                "the connection '{}' this task authenticates with is {}, which is not a state \
                 in which it holds authorization, so no request was made to '{}'",
                super::asl::echoed(&connection.arn),
                super::asl::echoed(&connection.state),
                super::asl::echoed(&asked.endpoint)
            ),
        });
    }
    let built = asked.wired(&connection)?;
    let answered = dispatch(callback, built).await?;
    let reported = reported(&answered);
    if (200..300).contains(&answered.status) {
        return Ok(reported);
    }
    // The endpoint answered that it will not do the work: the task fails under the name
    // its own status gives it, carrying that whole answer as the cause.
    Err(Failed {
        error: format!("{HTTP_STATUS_ERROR_PREFIX}{}", answered.status),
        cause: reported.to_string(),
    })
}

/// What one HTTP Task asks for, as its parameters say it.
#[derive(Debug, Clone)]
struct Asked {
    /// The absolute URL the request is made to, as the parameters named it.
    endpoint: String,
    /// The authority of that URL, which is the host the request is ADDRESSED to — read
    /// once, where the endpoint is read.
    authority: Option<String>,
    /// Its HTTP method.
    method: ::http::Method,
    /// The connection the request is authorized through.
    connection_arn: String,
    /// The headers the task asks for, in the order read; one entry per value, so a
    /// header the task repeats is sent once per value.
    headers: Vec<(String, String)>,
    /// Its query-string parameters, the same way.
    query: Vec<(String, String)>,
    /// Its body and the media type that body travels under; `None` for a task that
    /// sends none.
    body: Option<Body>,
}

/// The body of one request: the bytes and the media type they travel under.
#[derive(Debug, Clone)]
struct Body {
    media_type: &'static str,
    bytes: Vec<u8>,
}

impl Asked {
    /// What `request` asks for, or why no request is made for it.
    ///
    /// Each refusal says what the parameters named and what about it cannot be sent, and
    /// never claims the request happened.
    fn read(request: &Value) -> Result<Self, Failed> {
        let endpoint = required_text(request, API_ENDPOINT_MEMBER)?;
        // The endpoint is an ADDRESS the request is made at, so it is read as one here
        // rather than assembled into one: a value that is not an absolute URL names no
        // endpoint at all.
        let parsed = endpoint.parse::<::http::Uri>().ok().filter(|uri| {
            uri.scheme().is_some() && uri.authority().is_some()
        });
        let Some(parsed) = parsed else {
            return Err(unsent(format!(
                "this task's '{API_ENDPOINT_MEMBER}' is '{}', which is not the absolute URL of \
                 an endpoint, so no request was made for it",
                super::asl::echoed(&endpoint)
            )));
        };
        let authority = parsed
            .authority()
            .map(|authority| authority.as_str().to_string());
        let named = required_text(request, METHOD_MEMBER)?;
        let method = ::http::Method::from_bytes(named.as_bytes()).map_err(|_| {
            unsent(format!(
                "this task's '{METHOD_MEMBER}' is '{}', which is not an HTTP method, so no \
                 request was made for it",
                super::asl::echoed(&named)
            ))
        })?;
        let authentication = request.get(AUTHENTICATION_MEMBER).ok_or_else(|| {
            unsent(format!(
                "this task names no '{AUTHENTICATION_MEMBER}', so there is no connection to \
                 authorize its request through and nothing was run for it"
            ))
        })?;
        let connection_arn = required_text(authentication, CONNECTION_ARN_MEMBER)?;
        Ok(Self {
            endpoint,
            authority,
            method,
            connection_arn,
            headers: pairs(request, HEADERS_MEMBER)?,
            query: pairs(request, QUERY_PARAMETERS_MEMBER)?,
            body: body_of(request)?,
        })
    }

    /// The request this task makes, with the connection's own authorization and
    /// parameters merged onto it.
    ///
    /// Three sets of headers are merged in order, each REPLACING a header of the same
    /// name the one before it wrote: the request's own framing (the host it is addressed
    /// to, the media type of its body), then the task's own `Headers`, then the
    /// connection's parameters and its AUTHORIZATION last — the connection is what
    /// authorizes the call, so a definition cannot override what it sends, and a task
    /// that names its own media type gets ONE such header rather than two. Within one
    /// set every value is kept, because a header may be repeated. A header name is
    /// matched without regard to case, as an HTTP field name is (RFC 9110), so two
    /// spellings of one name are one header.
    fn wired(&self, connection: &Connection) -> Result<::http::Request<Vec<u8>>, Failed> {
        // The host a client sets from the URL it dials, which is what the service
        // claiming that endpoint reads the address off.
        let mut headers: Vec<(String, String)> = self
            .authority
            .iter()
            .map(|authority| {
                (
                    ::http::header::HOST.as_str().to_string(),
                    authority.clone(),
                )
            })
            .collect();
        // A request that carries no body names no media type for one either.
        if let Some(body) = &self.body {
            headers.push((
                ::http::header::CONTENT_TYPE.as_str().to_string(),
                body.media_type.to_string(),
            ));
        }
        replacing(&mut headers, &self.headers);
        let mut named: Vec<(String, String)> = connection.header_parameters.clone();
        named.extend(connection.authorization.clone());
        replacing(&mut headers, &named);

        let mut query = self.query.clone();
        replacing(&mut query, &connection.query_parameters);

        let mut builder = ::http::Request::builder()
            .method(self.method.clone())
            .uri(with_query(&self.endpoint, &query));
        for (name, value) in &headers {
            builder = builder.header(name, value);
        }
        let body = self
            .body
            .as_ref()
            .map(|body| body.bytes.clone())
            .unwrap_or_default();
        builder.body(body).map_err(|error| {
            unsent(format!(
                "the request this task would have made could not be built: {error}"
            ))
        })
    }
}

/// Merge `incoming` onto `carried`: every name `incoming` mentions REPLACES what
/// `carried` holds under it, and every value `incoming` holds is kept.
///
/// One name may be carried more than once — a repeated header, a repeated query
/// parameter — so the removal is by NAME over the whole incoming set before anything is
/// appended, rather than per value; otherwise the second value of a repeated name would
/// drop the first. Names are compared without regard to case, as an HTTP field name is
/// (RFC 9110); a query parameter's name is compared the same way the merge of a
/// connection's parameters compares one.
fn replacing(carried: &mut Vec<(String, String)>, incoming: &[(String, String)]) {
    carried.retain(|(name, _)| {
        !incoming
            .iter()
            .any(|(replaced, _)| replaced.eq_ignore_ascii_case(name))
    });
    carried.extend(incoming.iter().cloned());
}

/// The text `document` names `member` with, or why the request cannot be made without
/// it: these are the members that SAY the request, so one missing or written as
/// something other than text leaves no request to make.
fn required_text(document: &Value, member: &str) -> Result<String, Failed> {
    match document.get(member) {
        Some(Value::String(text)) if !text.is_empty() => Ok(text.clone()),
        Some(Value::String(_)) | None => Err(unsent(format!(
            "this task names no '{member}', so there is no request to make and nothing was run \
             for it"
        ))),
        Some(_) => Err(unsent(format!(
            "this task's '{member}' is not a string, so there is no request to make and nothing \
             was run for it"
        ))),
    }
}

/// The name/value pairs `document` carries under `member`, one entry per value.
///
/// Both `Headers` and `QueryParameters` are maps whose value is one value or a LIST of
/// values, because a header and a query parameter may each be repeated — so a list
/// becomes one pair per element, in the order written. A value that is not text is
/// carried as its own JSON text, which is how a URI component or a field value carries
/// one. A member carrying something other than a map leaves no request to make.
fn pairs(document: &Value, member: &str) -> Result<Vec<(String, String)>, Failed> {
    let carried = match document.get(member) {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Object(members)) => members,
        Some(_) => {
            return Err(unsent(format!(
                "this task's '{member}' is not a map of names to values, so there is no request \
                 to make and nothing was run for it"
            )))
        }
    };
    let mut pairs = Vec::new();
    for (name, value) in carried {
        match value {
            Value::Null => {}
            Value::Array(values) => {
                for value in values {
                    pairs.push((name.clone(), one_value(value)));
                }
            }
            value => pairs.push((name.clone(), one_value(value))),
        }
    }
    Ok(pairs)
}

/// One value as a header or a field carries it: text as it stands, and anything else as
/// its own JSON text.
fn one_value(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        value => value.to_string(),
    }
}

/// The body the task sends, under the encoding its `Transform` asks for; `None` for a
/// task that declares no `RequestBody`.
fn body_of(request: &Value) -> Result<Option<Body>, Failed> {
    let body = match request.get(REQUEST_BODY_MEMBER) {
        None | Some(Value::Null) => return Ok(None),
        Some(body) => body,
    };
    let transform = request.get(TRANSFORM_MEMBER);
    let encoding = transform
        .and_then(|transform| transform.get(REQUEST_BODY_ENCODING_MEMBER))
        .and_then(Value::as_str)
        .unwrap_or(JSON_ENCODING);
    match encoding {
        // The body is the document the state built, sent as it stands.
        JSON_ENCODING => Ok(Some(Body {
            media_type: JSON_MEDIA_TYPE,
            bytes: body.to_string().into_bytes(),
        })),
        URL_ENCODED_ENCODING => {
            let format = ArrayFormat::read(
                transform
                    .and_then(|transform| transform.get(REQUEST_ENCODING_OPTIONS_MEMBER))
                    .and_then(|options| options.get(ARRAY_FORMAT_MEMBER))
                    .and_then(Value::as_str),
            )?;
            Ok(Some(Body {
                media_type: FORM_MEDIA_TYPE,
                bytes: form_fields(body, format)?.into_bytes(),
            }))
        }
        other => Err(unsent(format!(
            "this task's '{TRANSFORM_MEMBER}' asks to send its body under the \
             '{REQUEST_BODY_ENCODING_MEMBER}' '{}', which is neither '{JSON_ENCODING}' nor \
             '{URL_ENCODED_ENCODING}', so no request was made for it",
            super::asl::echoed(other)
        ))),
    }
}

/// How a LIST is written in a URL-encoded body, which `RequestEncodingOptions`'
/// `ArrayFormat` selects.
///
/// The four values the HTTP Task documents for that member, and what each writes for a
/// member `a` holding `[1, 2]`: `INDICES` (the default) `a[0]=1&a[1]=2`, `BRACKETS`
/// `a[]=1&a[]=2`, `REPEAT` `a=1&a=2`, and `COMMAS` `a=1,2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArrayFormat {
    Indices,
    Brackets,
    Repeat,
    Commas,
}

impl ArrayFormat {
    /// The format `named` selects, the documented default for a task that names none,
    /// and a refusal for a value that is none of them — an encoding this emulator does
    /// not guess at.
    fn read(named: Option<&str>) -> Result<Self, Failed> {
        match named {
            None | Some("INDICES") => Ok(Self::Indices),
            Some("BRACKETS") => Ok(Self::Brackets),
            Some("REPEAT") => Ok(Self::Repeat),
            Some("COMMAS") => Ok(Self::Commas),
            Some(other) => Err(unsent(format!(
                "this task's '{ARRAY_FORMAT_MEMBER}' is '{}', which is not one of the array \
                 formats a URL-encoded body is written under, so no request was made for it",
                super::asl::echoed(other)
            ))),
        }
    }
}

/// `body`'s members as the fields of a URL-encoded request body.
///
/// Each value is percent-encoded by the maintained encoder ([`::percent_encoding`]) over
/// RFC 3986's unreserved set, so a value carrying a delimiter is one field rather than
/// several. A member holding a LIST is written under the asked-for [`ArrayFormat`]; a
/// member holding a nested OBJECT is not written at all — a form field carries text, and
/// how a nested document is spread over fields is not something this emulator guesses at,
/// so the request is not made rather than made with a field the endpoint did not ask
/// for. A body that is not an object has no members to write.
fn form_fields(body: &Value, format: ArrayFormat) -> Result<String, Failed> {
    let Some(members) = body.as_object() else {
        return Err(unsent(format!(
            "this task asks to send its '{REQUEST_BODY_MEMBER}' URL-encoded and that body is \
             not an object, so it has no fields to send and no request was made for it"
        )));
    };
    let mut fields: Vec<String> = Vec::new();
    for (name, value) in members {
        match value {
            Value::Null => {}
            Value::Object(_) => {
                return Err(unsent(format!(
                    "this task's '{REQUEST_BODY_MEMBER}' carries '{}' as a nested object, which \
                     this emulator does not write as URL-encoded fields, so no request was made \
                     for it",
                    super::asl::echoed(name)
                )))
            }
            Value::Array(values) => match format {
                ArrayFormat::Commas => fields.push(format!(
                    "{}={}",
                    encoded(name),
                    values
                        .iter()
                        .map(|value| encoded(&one_value(value)))
                        .collect::<Vec<String>>()
                        .join(",")
                )),
                format => {
                    for (at, value) in values.iter().enumerate() {
                        let key = match format {
                            ArrayFormat::Indices => format!("{name}[{at}]"),
                            ArrayFormat::Brackets => format!("{name}[]"),
                            _ => name.clone(),
                        };
                        fields.push(format!("{}={}", encoded(&key), encoded(&one_value(value))));
                    }
                }
            },
            value => fields.push(format!("{}={}", encoded(name), encoded(&one_value(value)))),
        }
    }
    Ok(fields.join("&"))
}

/// `text` percent-encoded as one component of a query or one field of a form, over the
/// ONE set this crate writes such a value under
/// ([`super::integration::UNRESERVED_ONLY`]).
fn encoded(text: &str) -> String {
    ::percent_encoding::utf8_percent_encode(text, super::integration::UNRESERVED_ONLY).to_string()
}

/// `endpoint` with `parameters` on its query string, appended to whatever query the
/// endpoint itself already carried.
///
/// A FRAGMENT is not part of the request-target a query is written into, so one the
/// endpoint carries is split off before the parameters are appended and re-attached
/// after them — otherwise a parameter would be written inside the fragment.
fn with_query(endpoint: &str, parameters: &[(String, String)]) -> String {
    if parameters.is_empty() {
        return endpoint.to_string();
    }
    let (target, fragment) = match endpoint.split_once('#') {
        Some((target, fragment)) => (target, format!("#{fragment}")),
        None => (endpoint, String::new()),
    };
    let rendered: Vec<String> = parameters
        .iter()
        .map(|(name, value)| format!("{}={}", encoded(name), encoded(value)))
        .collect();
    let separator = if target.contains('?') { '&' } else { '?' };
    format!("{target}{separator}{}{fragment}", rendered.join("&"))
}

/// What one EventBridge connection authorizes an HTTP Task's request with, read out of
/// EventBridge's own record of it.
#[derive(Clone)]
pub struct Connection {
    /// The ARN the connection carries, which is the connection's own rather than the
    /// text the task named it with.
    arn: String,
    /// Its `ConnectionState`, as stored.
    state: String,
    /// The header the connection's credentials ARE, as EventBridge builds it for every
    /// invocation through the connection
    /// ([`::service_com_amazonaws_eventbridge_awsevents::state::invocation::ConnectionSecret::authorization_header`]);
    /// `None` for a connection whose credentials have to be exchanged for a token first
    /// and for one holding none at all.
    authorization: Option<(String, String)>,
    /// The header parameters every invocation through it sends.
    header_parameters: Vec<(String, String)>,
    /// Its query-string parameters.
    query_parameters: Vec<(String, String)>,
}

/// WHICH connection this is and what state it is in, and never what it holds: its
/// authorization is the credential EventBridge keeps in the connection's secret, and its
/// parameters are values the connection may equally have flagged secret, so no
/// formatting of a connection — a log line, a trace field, a panic message — can put one
/// anywhere but in the request it authorizes. The non-secret half of every parameter
/// block is what EventBridge's own read reports, and that is where to look for a value.
impl ::std::fmt::Debug for Connection {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("Connection")
            .field("arn", &self.arn)
            .field("state", &self.state)
            .field(
                "authorization",
                &self
                    .authorization
                    .as_ref()
                    .map(|(name, _)| format!("<the credential sent as {name}>")),
            )
            .field("header_parameters", &self.header_parameters.len())
            .field("query_parameters", &self.query_parameters.len())
            .finish()
    }
}

/// What resolves the EventBridge connection an HTTP Task names, by its ARN; `None` for
/// an ARN that store holds no connection under.
///
/// A closure rather than a trait, because the records it reads live in another service's
/// store and are reached through the engine the operation holds ([`connections`]): the
/// walk knows nothing of either, and asks only the question.
pub type Connections =
    ::std::sync::Arc<dyn Fn(&str) -> Option<Connection> + Send + Sync>;

/// What resolves this walk's HTTP Task connections, or `None` when the shared engine
/// carries no EventBridge store — a deployment with no EventBridge to ask, where there
/// is no connection to authorize such a request through at all.
///
/// The records are EventBridge's OWN, reached through the SAME shared engine this
/// service's store lives in, so an HTTP Task reads the very connection
/// `CreateConnection` wrote — its state and the credentials it holds — rather than a
/// second copy of it. Each read settles the connection slice first, so a transition the
/// clock has already made due (`CREATING` -> `AUTHORIZED`, `DEAUTHORIZING` ->
/// `DEAUTHORIZED`) is materialised exactly as EventBridge's own readers materialise it.
///
/// It is consulted once per request a task makes, so a connection deauthorized between
/// two states of one execution stops authorizing from the next request: nothing is read
/// in advance and nothing is cached.
pub fn connections<Eng: Send + Sync + 'static>(
    engine: &::std::sync::Arc<Eng>,
) -> Option<Connections> {
    events_state(&**engine)?;
    let engine = ::std::sync::Arc::clone(engine);
    Some(::std::sync::Arc::new(move |arn: &str| {
        let state = events_state(&*engine)?;
        let mut store = state.lock();
        store.settle_connections(super::now());
        let connection = store.connection_by_arn(arn)?;
        let parameters = connection.secret.invocation_http_parameters.as_ref();
        Some(Connection {
            arn: connection.arn.clone(),
            // The state the connection has SETTLED on, which is the state its clock
            // already owes it while a transition is pending.
            state: connection
                .settled_state()
                .unwrap_or(&connection.state)
                .to_string(),
            authorization: connection.secret.authorization_header(),
            header_parameters: parameters
                .and_then(|parameters| parameters.header_parameters.as_ref())
                .map(|list| {
                    list.iter()
                        .filter_map(|parameter| {
                            Some((parameter.key.clone()?, parameter.value.clone()?))
                        })
                        .collect()
                })
                .unwrap_or_default(),
            query_parameters: parameters
                .and_then(|parameters| parameters.query_string_parameters.as_ref())
                .map(|list| {
                    list.iter()
                        .filter_map(|parameter| {
                            Some((parameter.key.clone()?, parameter.value.clone()?))
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
    }))
}

/// The EventBridge store in the SAME shared engine, which is where the account's
/// connections live.
fn events_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<EventsState>> {
    ::service_com_amazonaws_eventbridge_awsevents::state::service_state(engine)
}

/// EventBridge's OWN state struct, reached through the shared engine so an HTTP Task
/// reads the very connections that service's handlers wrote.
type EventsState =
    ::service_com_amazonaws_eventbridge_awsevents::ComAmazonawsEventbridgeAwseventsState;

/// The connection `arn` names, or why this task is failed without a request.
fn resolved_connection(
    connections: Option<&Connections>,
    arn: &str,
) -> Result<Connection, Failed> {
    if connection_named(arn).is_none() {
        return Err(Failed {
            error: CONNECTION_NOT_FOUND_ERROR.to_string(),
            cause: format!(
                "this task's '{CONNECTION_ARN_MEMBER}' is '{}', which is not the ARN of an \
                 EventBridge connection, so there is no connection to authorize its request \
                 through and nothing was run for it",
                super::asl::echoed(arn)
            ),
        });
    }
    let Some(connections) = connections else {
        return Err(unsent(format!(
            "the connection '{}' this task authenticates with cannot be read, because this \
             emulator carries no EventBridge for it to belong to, so nothing was run for it",
            super::asl::echoed(arn)
        )));
    };
    connections(arn).ok_or_else(|| Failed {
        error: CONNECTION_NOT_FOUND_ERROR.to_string(),
        cause: format!(
            "the connection '{}' this task authenticates with does not exist, so no request \
             was made for it",
            super::asl::echoed(arn)
        ),
    })
}

/// Whether `arn` is the ARN of an EventBridge connection, and the connection NAME it
/// carries.
///
/// The form is the one EventBridge mints —
/// `arn:aws:events:<region>:<account>:connection/<name>/<id>` — whose Region field the
/// `ConnectionArn` pattern admits as empty.
fn connection_named(arn: &str) -> Option<&str> {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    let resource = match fields.as_slice() {
        ["arn", _partition, EVENTS_NAMESPACE, _region, _account, resource] => *resource,
        _ => return None,
    };
    let named = resource.strip_prefix(CONNECTION_RESOURCE_PREFIX)?;
    let named = named.split('/').next().unwrap_or_default();
    (!named.is_empty()).then_some(named)
}

/// What one request to an endpoint of this emulator came back with.
#[derive(Debug, Clone)]
struct Answered {
    /// The HTTP status the endpoint answered.
    status: u16,
    /// Every header of the answer, in the order it carried them.
    headers: Vec<(String, String)>,
    /// Its body.
    payload: Vec<u8>,
}

/// Hand `built` to the in-process transport and read what the endpoint answered.
async fn dispatch(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    built: ::http::Request<Vec<u8>>,
) -> Result<Answered, Failed> {
    use ::aws_smithy_runtime_api::client::http::HttpConnector as _;

    let endpoint = built.uri().to_string();
    let (parts, body) = built.into_parts();
    let request = ::http::Request::from_parts(
        parts,
        ::aws_smithy_types::body::SdkBody::from(::bytes::Bytes::from(body)),
    );
    let request = ::aws_smithy_runtime_api::http::Request::try_from(request).map_err(|error| {
        unsent(format!(
            "the request this task would have made could not be built: {error}"
        ))
    })?;
    let answer = callback
        .http_connector()
        .call(request)
        .await
        .map_err(|error| {
            unsent(format!(
                "the endpoint '{}' this task names did not answer the request ({}), so nothing \
                 was run for it",
                super::asl::echoed(&endpoint),
                ::aws_smithy_types::error::display::DisplayErrorContext(&error)
            ))
        })?;
    let status = answer.status().as_u16();
    let headers: Vec<(String, String)> = answer
        .headers()
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    let payload = answer
        .body()
        .bytes()
        .ok_or_else(|| {
            unsent(format!(
                "the endpoint '{}' this task names answered a streaming body, which this \
                 emulator does not report as a task's result",
                super::asl::echoed(&endpoint)
            ))
        })?
        .to_vec();
    // The front door's own refusal is not the addressed endpoint answering: nothing in
    // this emulator serves that endpoint, so the request reached none at all and there
    // is no response to report.
    if ::service_common::in_process_refusal::is_transport_refusal(&payload) {
        return Err(unsent(format!(
            "the endpoint '{}' this task names is one nothing in this emulator serves, so no \
             request reached an endpoint and nothing was run for it",
            super::asl::echoed(&endpoint)
        )));
    }
    Ok(Answered {
        status,
        headers,
        payload,
    })
}

/// The document the task reports for `answered`: the endpoint's own status, the reason
/// phrase registered for that status, the headers it answered and its body.
///
/// Every value in it is the ANSWER's own, and a fact the answer did not carry is
/// reported by nothing rather than filled in: a status no reason phrase is registered
/// for carries no `StatusText`, and an empty body no `ResponseBody`. The reason phrase is
/// the one registered for the status rather than one read off the wire, because the
/// in-process answer carries no reason line at all.
///
/// Pure, so the shape of the result is pinned by this module's own tests.
fn reported(answered: &Answered) -> Value {
    let mut members = Map::new();
    let mut headers: Map<String, Value> = Map::new();
    for (name, value) in &answered.headers {
        match headers.get_mut(name).and_then(Value::as_array_mut) {
            Some(values) => values.push(Value::String(value.clone())),
            None => {
                headers.insert(
                    name.clone(),
                    Value::Array(vec![Value::String(value.clone())]),
                );
            }
        }
    }
    if !headers.is_empty() {
        members.insert(HEADERS_MEMBER.to_string(), Value::Object(headers));
    }
    if !answered.payload.is_empty() {
        // The body is reported as the document it holds, and as its own text when it is
        // not one.
        let body = ::serde_json::from_slice::<Value>(&answered.payload).unwrap_or_else(|_| {
            Value::String(String::from_utf8_lossy(&answered.payload).to_string())
        });
        members.insert(RESPONSE_BODY_MEMBER.to_string(), body);
    }
    members.insert(
        STATUS_CODE_MEMBER.to_string(),
        Value::Number(answered.status.into()),
    );
    if let Some(reason) = ::http::StatusCode::from_u16(answered.status)
        .ok()
        .and_then(|status| status.canonical_reason())
    {
        members.insert(
            STATUS_TEXT_MEMBER.to_string(),
            Value::String(reason.to_string()),
        );
    }
    Value::Object(members)
}

/// What a task fails with when no request was made at all: this emulator could not make
/// the one the parameters describe, which is its own gap rather than an endpoint
/// refusing, so it carries the name the interpreter raises its own failures under and a
/// cause that never claims the work happened.
fn unsent(cause: String) -> Failed {
    Failed {
        error: super::asl::RUNTIME_ERROR.to_string(),
        cause,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The pairs a map of headers or query parameters reads as, rendered as the
    /// `<name>=<value>` fields they become so one row is one string. The rows differ
    /// along the form the value takes: one value, a repeated one, a value that is not
    /// text, an absent member and a member written as something other than a map.
    #[test]
    fn a_map_of_names_to_values_reads_as_one_pair_per_value() {
        let cases: &[(&str, Value, Option<&str>)] = &[
            ("no member at all", ::serde_json::json!({}), Some("")),
            (
                "one value",
                ::serde_json::json!({ "Headers": { "x-a": "1" } }),
                Some("x-a=1"),
            ),
            (
                "a repeated header",
                ::serde_json::json!({ "Headers": { "x-a": ["1", "2"] } }),
                Some("x-a=1&x-a=2"),
            ),
            (
                "values that are not text",
                ::serde_json::json!({ "Headers": { "x-a": 7, "x-b": true } }),
                Some("x-a=7&x-b=true"),
            ),
            (
                "a member that is not a map",
                ::serde_json::json!({ "Headers": "x-a: 1" }),
                None,
            ),
        ];
        for (case, document, expected) in cases {
            let read = pairs(document, HEADERS_MEMBER).ok().map(|pairs| {
                pairs
                    .iter()
                    .map(|(name, value)| format!("{name}={value}"))
                    .collect::<Vec<String>>()
                    .join("&")
            });
            assert_eq!(
                read.as_deref(),
                *expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }

    /// One set of headers a row of the merge table supplies, as `(name, value)` pairs.
    type Supplied = &'static [(&'static str, &'static str)];

    /// The parameters one row of the query table appends.
    type Appended<'r> = &'r [(String, String)];

    /// The query parameters are appended to the request-target the endpoint already is.
    /// The rows differ along what that target carries: nothing, a query of its own, a
    /// fragment, and both — plus the endpoint a task adds no parameter to.
    #[test]
    fn the_query_parameters_are_appended_to_the_target_the_endpoint_carries() {
        let parameters = vec![("q".to_string(), "a b".to_string())];
        let cases: &[(&str, &str, Appended<'_>, &str)] = &[
            ("no parameter at all", "https://api.test/x", &[], "https://api.test/x"),
            (
                "an endpoint carrying no query",
                "https://api.test/x",
                &parameters,
                "https://api.test/x?q=a%20b",
            ),
            (
                "an endpoint carrying a query of its own",
                "https://api.test/x?page=2",
                &parameters,
                "https://api.test/x?page=2&q=a%20b",
            ),
            (
                "an endpoint carrying a fragment",
                "https://api.test/x#top",
                &parameters,
                "https://api.test/x?q=a%20b#top",
            ),
            (
                "an endpoint carrying both",
                "https://api.test/x?page=2#top",
                &parameters,
                "https://api.test/x?page=2&q=a%20b#top",
            ),
        ];
        for (case, endpoint, parameters, expected) in cases {
            let written = with_query(endpoint, parameters);
            assert_eq!(
                written, *expected,
                "case {case:?}: expected {expected:?}, got {written:?}"
            );
        }
    }

    /// Three sets of headers are merged in order, each replacing a name the one before it
    /// wrote and keeping every value of its own. The rows span what the merge
    /// generalises over: a name only one set carries, a name the task takes from the
    /// request's own framing, a name the connection takes from the task, a header the
    /// task repeats, and the connection's authorization.
    ///
    /// The headers are compared as a SORTED set of `<name>=<value>` fields: which order a
    /// request writes its headers in is no part of the rule, and a repeated name keeps
    /// one field per value either way.
    #[test]
    fn each_set_of_headers_replaces_the_names_the_one_before_it_wrote() {
        let cases: &[(&str, Supplied, Supplied, &str)] = &[
            (
                "a name only the task carries",
                &[("x-task", "1")],
                &[],
                "content-type=application/json&host=api.test&x-task=1",
            ),
            (
                "the task naming the media type the body travels under",
                &[("content-type", "text/plain")],
                &[],
                "content-type=text/plain&host=api.test",
            ),
            (
                "a header the task repeats",
                &[("x-task", "1"), ("x-task", "2")],
                &[],
                "content-type=application/json&host=api.test&x-task=1&x-task=2",
            ),
            (
                "a name the connection takes from the task",
                &[("x-shared", "from-the-task")],
                &[("X-Shared", "from-the-connection")],
                "content-type=application/json&host=api.test&x-shared=from-the-connection",
            ),
        ];
        for (case, asked, parameters, expected) in cases {
            let owned = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
                pairs
                    .iter()
                    .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                    .collect()
            };
            let asked = Asked {
                endpoint: "https://api.test/".to_string(),
                authority: Some("api.test".to_string()),
                method: ::http::Method::POST,
                connection_arn: String::new(),
                headers: owned(asked),
                query: Vec::new(),
                body: Some(Body {
                    media_type: JSON_MEDIA_TYPE,
                    bytes: b"{}".to_vec(),
                }),
            };
            let connection = Connection {
                arn: String::new(),
                state: "AUTHORIZED".to_string(),
                authorization: None,
                header_parameters: owned(parameters),
                query_parameters: Vec::new(),
            };
            let built = asked.wired(&connection).expect("the request is built");
            let mut written: Vec<String> = built
                .headers()
                .iter()
                .map(|(name, value)| {
                    format!("{name}={}", value.to_str().unwrap_or_default())
                })
                .collect();
            written.sort();
            assert_eq!(
                written.join("&"),
                *expected,
                "case {case:?}: expected {expected:?}, got {written:?}"
            );
        }
    }

    /// A URL-encoded body writes one field per value, with a list written under the
    /// asked-for format. The rows span the four documented formats and the two bodies
    /// that have no fields to write.
    #[test]
    fn a_url_encoded_body_writes_its_members_as_fields() {
        let body = ::serde_json::json!({ "a": "x y", "b": [1, 2] });
        let cases: &[(&str, ArrayFormat, &str)] = &[
            ("INDICES", ArrayFormat::Indices, "a=x%20y&b%5B0%5D=1&b%5B1%5D=2"),
            ("BRACKETS", ArrayFormat::Brackets, "a=x%20y&b%5B%5D=1&b%5B%5D=2"),
            ("REPEAT", ArrayFormat::Repeat, "a=x%20y&b=1&b=2"),
            ("COMMAS", ArrayFormat::Commas, "a=x%20y&b=1,2"),
        ];
        for (case, format, expected) in cases {
            let written = form_fields(&body, *format).expect("the body has fields");
            assert_eq!(
                written, *expected,
                "case {case:?}: expected {expected:?}, got {written:?}"
            );
        }
        for (case, body) in [
            ("a body that is not an object", ::serde_json::json!("text")),
            (
                "a body carrying a nested object",
                ::serde_json::json!({ "a": { "b": 1 } }),
            ),
        ] {
            let written = form_fields(&body, ArrayFormat::Indices);
            assert!(
                written.is_err(),
                "case {case:?}: expected no fields to send, got {written:?}"
            );
        }
    }

    /// An EventBridge connection ARN names the connection to read. The rows differ along
    /// what the text is: the documented form, one carrying no Region, one naming another
    /// service, and text that is no ARN.
    #[test]
    fn a_connection_arn_names_the_connection_to_read() {
        let cases: &[(&str, Option<&str>)] = &[
            (
                "arn:aws:events:us-west-2:000000000000:connection/orders/7b3c",
                Some("orders"),
            ),
            (
                "arn:aws:events::000000000000:connection/orders/7b3c",
                Some("orders"),
            ),
            (
                "arn:aws:events:us-west-2:000000000000:connection/orders",
                Some("orders"),
            ),
            ("arn:aws:events:us-west-2:000000000000:rule/orders", None),
            (
                "arn:aws:sqs:us-west-2:000000000000:connection/orders/7b3c",
                None,
            ),
            ("orders", None),
        ];
        for (arn, expected) in cases {
            let read = connection_named(arn);
            assert_eq!(
                read, *expected,
                "case {arn:?}: expected {expected:?}, got {read:?}"
            );
        }
    }

    /// The result reports the answer's own status, reason phrase, headers and body, and
    /// fills in nothing the answer did not carry.
    #[test]
    fn the_result_reports_what_the_endpoint_answered() {
        let answered = Answered {
            status: 200,
            headers: vec![
                ("content-type".to_string(), "application/json".to_string()),
                ("x-a".to_string(), "1".to_string()),
                ("x-a".to_string(), "2".to_string()),
            ],
            payload: br#"{"echoed":true}"#.to_vec(),
        };
        let answer = reported(&answered);
        assert_eq!(
            answer,
            ::serde_json::json!({
                "Headers": {
                    "content-type": ["application/json"],
                    "x-a": ["1", "2"],
                },
                "ResponseBody": { "echoed": true },
                "StatusCode": 200,
                "StatusText": "OK",
            }),
            "case {:?}: unexpected result, got {answer:?}",
            "a JSON answer carrying a repeated header"
        );
        let bare = reported(&Answered {
            status: 599,
            headers: Vec::new(),
            payload: Vec::new(),
        });
        assert_eq!(
            bare,
            ::serde_json::json!({ "StatusCode": 599 }),
            "case {:?}: a fact the answer did not carry must be reported by nothing, got {bare:?}",
            "an empty answer under an unregistered status"
        );
    }
}
