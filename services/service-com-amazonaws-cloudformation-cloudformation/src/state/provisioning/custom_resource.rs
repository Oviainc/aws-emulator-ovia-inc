// SPDX-License-Identifier: CC0-1.0

//! CUSTOM RESOURCES: the provider a template names in a resource's `ServiceToken`, the
//! request CloudFormation sends that provider on every stack operation touching the
//! resource, and the answer the provider sends back.
//!
//! ## The rule this module implements
//!
//! A resource whose type is `Custom::<Name>` or `AWS::CloudFormation::CustomResource` is
//! not backed by an API of this emulator's at all: it is backed by the PROVIDER its own
//! `ServiceToken` property names — an Amazon SNS topic or an AWS Lambda function. On every
//! stack operation that acts on such a resource, CloudFormation sends that provider a
//! request and holds the resource — and the stack operation behind it — in its
//! `*_IN_PROGRESS` status until the provider answers at the request's `ResponseURL`. The
//! provider's answer is what the resource IS: the `PhysicalResourceId` it reports is what
//! `Ref` resolves to, and the `Data` it reports is what `Fn::GetAtt` resolves against.
//!
//! The request names which operation it is for and what the resource is to be:
//! `RequestType` (`Create`, `Update` or `Delete`), `RequestId`, `StackId`,
//! `LogicalResourceId`, `ResourceType`, `ResourceProperties` and the `ResponseURL`, plus
//! the `PhysicalResourceId` of the resource that already exists on an `Update` or a
//! `Delete`, and the `OldResourceProperties` it was last declared with on an `Update`.
//!
//! The answer carries `Status` (`SUCCESS` or `FAILED`), the `PhysicalResourceId` the
//! resource is to be known by, the `Reason` a failure gives, the `Data` the resource
//! publishes and `NoEcho`. A `FAILED` answer fails the resource with that reason, exactly
//! as a refusal from an owning service does.
//!
//! An `Update` the provider answers with a DIFFERENT `PhysicalResourceId` has REPLACED the
//! resource: the new physical resource is the one the stack keeps, and the OLD one is
//! cleaned up by a further `Delete` request carrying the old id and the old properties
//! ([`crate::state::provisioning::advance_stabilisations`] sends it). The same id is an
//! in-place update and owes no `Delete` at all.
//!
//! ## What is in this module, and what is not
//!
//! Here: the request document, the two transports a `ServiceToken` can name, the
//! `ResponseURL` this emulator publishes and serves, and how a provider's answer is read.
//! The WAIT the answer releases is [`crate::state::stabilising`]'s — a custom resource's
//! barrier carries the request it is waiting for ([`ProviderRequest`]) and the answer that
//! arrived ([`ProviderResponse`]) — and the plane that receives the answer is
//! [`crate::service::custom_resource_plane`].
//!
//! ## Bounds
//!
//! One request per resource per stack operation, so the set in flight is bounded by the
//! barriers of that operation ([`crate::state::stabilising`]). What a provider's answer may
//! leave on a resource is bounded by [`MAX_DATA_ENTRIES`] entries of at most
//! [`MAX_DATA_VALUE_CHARS`] characters each, and the reason it may leave on a status by
//! [`MAX_REASON_CHARS`] — a provider is a client, so nothing it sends sizes this store.

use std::collections::BTreeMap;
use std::time::SystemTime;

use serde_json::{Map, Value};

use super::{call_query, dispatch, percent_encoded, refusal_of, Refusal, REST_JSON};

/// The property of a custom resource that names its provider: "the service token, such as
/// an `AWS::Lambda::Function` ARN or an `AWS::SNS::Topic` ARN, that was given to the
/// template developer by the service provider to access the service"
/// (`AWS::CloudFormation::CustomResource`'s `ServiceToken`).
pub const SERVICE_TOKEN: &str = "ServiceToken";

/// The `RequestType` of a provider request: which stack operation it is for.
pub mod request_type {
    /// The resource is being created.
    pub const CREATE: &str = "Create";
    /// The resource already exists and its declaration changed.
    pub const UPDATE: &str = "Update";
    /// The resource is being removed — by the stack's own deletion, by a deployment that no
    /// longer declares it, or as the cleanup of a replacement.
    pub const DELETE: &str = "Delete";
}

/// The `Status` a provider answers with when it carried the request out.
pub const SUCCESS: &str = "SUCCESS";

/// The `Status` it answers with when it did not.
pub const FAILED: &str = "FAILED";

/// Amazon SNS's signing name and awsQuery API version, from its model (`sigv4Name: sns`,
/// `version: 2010-03-31`) — the transport a `ServiceToken` naming a TOPIC rides.
const SNS_SIGNING_NAME: &str = "sns";
const SNS_API_VERSION: &str = "2010-03-31";

/// AWS Lambda's signing name — the transport a `ServiceToken` naming a FUNCTION rides.
const LAMBDA_SIGNING_NAME: &str = "lambda";

/// The `InvocationType` CloudFormation invokes a function-backed provider under: the
/// provider answers at the `ResponseURL` rather than in the invocation's response, so the
/// invocation does not wait for it ("to invoke a function asynchronously, set
/// `InvocationType` to `Event`", `Invoke`).
const INVOCATION_TYPE_EVENT: &str = "Event";

/// The first label of the host this emulator publishes a request's `ResponseURL` on.
///
/// Its first label names no service of this emulator, so the framework reads no routing key
/// out of it and only the service that HANDED THE URL OUT claims a request addressed to it
/// ([`crate::service::EmulatedService::can_handle`]).
const RESPONSE_HOST_LABEL: &str = "cloudformation-custom-resource-response";

/// The suffix every host this emulator publishes carries.
const AWS_HOST_SUFFIX: &str = ".amazonaws.com";

/// The separator the published `ResponseURL` joins the request's three identifying parts
/// with inside its ONE path segment, as CloudFormation's own response URLs do.
const RESPONSE_PATH_SEPARATOR: char = '|';

/// The signing algorithm a pre-signed URL names in `X-Amz-Algorithm`.
const SIGV4_ALGORITHM: &str = "AWS4-HMAC-SHA256";

/// The headers the published URL's signature covers, as `X-Amz-SignedHeaders` names them:
/// the endpoint the write is addressed to, which is the one header a pre-signed URL signs.
const SIGNED_HEADERS: &str = "host";

/// How long the published `ResponseURL` may be written to, in seconds: "the maximum
/// expiration time for the signed URL is one week" (AWS General Reference, *Authenticating
/// requests: using query parameters*), which is the longest window a query-authenticated URL
/// can state — and longer than any wait this service holds a resource at, so the window
/// never closes before the resource it answers for has failed.
const RESPONSE_URL_EXPIRES_SECONDS: u64 = 604_800;

/// How many hexadecimal characters the published URL's `X-Amz-Signature` carries: the width
/// of the SHA-256 digest a SigV4 signature is.
const SIGNATURE_HEX_CHARS: usize = 64;

/// What the published URL's signature is derived under, which keeps it apart from every
/// other value this crate derives from the same request.
const SIGNATURE_SALT: &str = "custom-resource-response-url";

/// How many `Data` entries of a provider's answer a resource keeps.
///
/// The same ceiling every other resource's attribute set is held to
/// ([`crate::state::template::MAX_ATTRIBUTES_PER_RESOURCE`]), since `Data` is exactly what
/// `Fn::GetAtt` resolves against for this type.
pub const MAX_DATA_ENTRIES: usize = crate::state::template::MAX_ATTRIBUTES_PER_RESOURCE;

/// How many characters of one `Data` value a resource keeps. "The total size of the
/// response body can't exceed 4096 bytes" (custom resource response objects), so no single
/// value a well-formed answer carries is longer than this.
pub const MAX_DATA_VALUE_CHARS: usize = 4096;

/// How many characters of a `FAILED` answer's `Reason` the resource's status carries. The
/// same bound a response body is held to, so a reason is kept whole and nothing longer than
/// one response sizes an event.
pub const MAX_REASON_CHARS: usize = 4096;

/// What a provider answers a `FAILED` request with when it states no reason of its own.
const UNSTATED_REASON: &str = "The custom resource provider reported FAILED";

/// The provider request ONE stack operation sent about ONE custom resource: which operation
/// it was for and the id it is answered under.
///
/// This is what identifies the wait the resource is held at: the answer that releases it is
/// the one PUT to the `ResponseURL` of THIS request, so a resource that is updated twice is
/// never released by the answer to the earlier request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRequest {
    /// The `RequestType` the provider was sent.
    pub request_type: String,
    /// The `RequestId` the provider answers under, which the `ResponseURL` carries.
    pub request_id: String,
    /// The `PhysicalResourceId` the request named, for a request about a resource that
    /// already existed — the id a replacement's cleanup `Delete` addresses, and what says
    /// whether an answer REPLACED the resource.
    pub physical_resource_id: Option<String>,
    /// The properties the request carried, kept because the cleanup `Delete` of a replaced
    /// resource carries the OLD ones.
    pub properties: Value,
    /// The `OldResourceProperties` the request carried, for an `Update`: the declaration the
    /// resource that an answer may REPLACE was last deployed with, which that replacement's
    /// cleanup `Delete` carries.
    pub old_properties: Option<Value>,
}

/// What a provider PUT to the `ResponseURL` of a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderResponse {
    /// `Status`: `SUCCESS` or `FAILED`.
    pub status: String,
    /// `PhysicalResourceId`: "this value should be an identifier unique to the custom
    /// resource vendor, and can be up to 1 KB in size".
    pub physical_resource_id: String,
    /// `Reason`: "describes the reason for a failure response".
    pub reason: Option<String>,
    /// `Data`: "the custom resource provider-defined name-value pairs to send with the
    /// response. You can access the values provided here by name in the template with
    /// `Fn::GetAtt`."
    pub data: BTreeMap<String, String>,
}

impl ProviderResponse {
    /// Whether the provider carried the request out.
    pub fn succeeded(&self) -> bool {
        self.status == SUCCESS
    }

    /// Why the resource failed, for an answer that is not a success: the provider's own
    /// `Reason`, bounded, and a fixed sentence for an answer that states none.
    pub fn failure_reason(&self) -> String {
        match self.reason.as_deref().map(str::trim) {
            Some(reason) if !reason.is_empty() => bounded(reason, MAX_REASON_CHARS),
            _ => UNSTATED_REASON.to_string(),
        }
    }

    /// Read one provider answer out of the document it was PUT as.
    ///
    /// `Err` names the member a well-formed answer must carry and this one did not — which
    /// is what the plane refuses the write for, rather than recording an answer that says
    /// nothing about the resource. `Status` is required ("must be either SUCCESS or
    /// FAILED") and so is `PhysicalResourceId`.
    pub fn read(document: &Value) -> Result<Self, &'static str> {
        let status = match document.get("Status").and_then(Value::as_str) {
            Some(SUCCESS) => SUCCESS,
            Some(FAILED) => FAILED,
            _ => return Err("Status"),
        };
        let physical_resource_id = document
            .get("PhysicalResourceId")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .ok_or("PhysicalResourceId")?;
        let mut data: BTreeMap<String, String> = BTreeMap::new();
        if let Some(Value::Object(entries)) = document.get("Data") {
            for (name, value) in entries.iter().take(MAX_DATA_ENTRIES) {
                data.insert(name.clone(), bounded(&attribute_text(value), MAX_DATA_VALUE_CHARS));
            }
        }
        Ok(Self {
            status: status.to_string(),
            physical_resource_id: physical_resource_id.to_string(),
            reason: document
                .get("Reason")
                .and_then(Value::as_str)
                .map(str::to_string),
            data,
        })
    }
}

/// One `Data` value as the string `Fn::GetAtt` resolves to: the text of a string value, and
/// the JSON text of anything else — a provider's name-value pairs reach a template as
/// values, and a template resolves a value to text.
fn attribute_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// `value` cut to at most `chars` characters, on a character boundary.
fn bounded(value: &str, chars: usize) -> String {
    value.chars().take(chars).collect()
}

/// Whether `resource_type` names a custom resource — the one question that decides whether
/// a resource's provisioning is this module's.
pub fn is_custom_resource(resource_type: &str) -> bool {
    crate::state::resource_types::is_custom_resource(resource_type)
}

/// Mint the `RequestId` one provider request is answered under.
///
/// A fresh identifier per request, in the same 8-4-4-4-12 form every other identifier this
/// service hands out carries: it is what the published `ResponseURL` is addressed by, so two
/// requests about one resource are never answered by one write.
pub fn new_request_id() -> String {
    let digits = crate::state::new_identifier_digits("custom-resource-request", 32);
    format!(
        "{}-{}-{}-{}-{}",
        &digits[0..8],
        &digits[8..12],
        &digits[12..16],
        &digits[16..20],
        &digits[20..32]
    )
}

/// The host this emulator publishes a `ResponseURL` on, for `region`.
pub fn response_host(region: &str) -> String {
    format!(
        "{RESPONSE_HOST_LABEL}.{region}{AWS_HOST_SUFFIX}",
        region = region_or_default(region)
    )
}

/// True when `host` is the one [`response_host`] publishes — with any `user@` and `:port`
/// removed, as a `Host` header may carry either.
///
/// Recognising it is what routes a request to a URL this service HANDED OUT back to this
/// service.
pub fn is_response_host(host: &str) -> bool {
    response_host_region(host).is_some()
}

/// The Region a published response host names, or `None` when `host` is not one
/// [`response_host`] publishes.
///
/// A write to this plane is UNSIGNED — a pre-signed URL is followed with no credential — so
/// the host is where the Region of the stack the answer is about comes from: it is the one
/// this service put there when it published the URL.
pub fn response_host_region(host: &str) -> Option<&str> {
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    let rest = host
        .strip_prefix(RESPONSE_HOST_LABEL)
        .and_then(|rest| rest.strip_prefix('.'))?;
    rest.strip_suffix(AWS_HOST_SUFFIX)
        .and_then(::service_common::arn::region_label)
}

/// What a write to a published `ResponseURL` names: the stack the resource belongs to, the
/// resource it is about and the request it answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResponseTarget {
    /// The `StackId` the request carried.
    pub stack_id: String,
    /// The `LogicalResourceId` the request was about.
    pub logical_resource_id: String,
    /// The `RequestId` this write answers.
    pub request_id: String,
}

/// The `ResponseURL` one provider request publishes: a URL on this emulator's own endpoint
/// whose ONE path segment carries the stack, the resource and the request, joined as
/// CloudFormation's own response URLs join them, under the QUERY that makes it a pre-signed
/// URL.
///
/// A URL this emulator SERVES ([`crate::service::custom_resource_plane`]) and reads back
/// ([`parse_response_path`]), so the provider's answer the request asks for can really be
/// made against it — with no credential of its own, as a pre-signed URL is followed.
///
/// The query is the SigV4 query-authentication form a pre-signed URL carries: the algorithm,
/// the instant it was issued, the window it states ([`RESPONSE_URL_EXPIRES_SECONDS`]), the
/// headers the signature covers and the signature itself, which is over the target the URL
/// authorises and is what the plane holds a write to ([`signature_admits`]). The window is
/// PUBLISHED and not enforced: the plane does not refuse a write for the instant it arrived,
/// and the window is the longest one a query-authenticated URL may state rather than a
/// deadline this service keeps. It names NO credential: the write is addressed to the
/// endpoint that issued the URL and is served unsigned, so a credential in the query would
/// both route the write to whatever service that credential's scope named and publish a
/// credential of this emulator's in a response.
pub fn response_url(region: &str, stack_id: &str, logical_id: &str, request_id: &str) -> String {
    let target = format!(
        "{stack_id}{RESPONSE_PATH_SEPARATOR}{logical_id}{RESPONSE_PATH_SEPARATOR}{request_id}"
    );
    let signature = target_signature(&target);
    format!(
        "https://{host}/{segment}?X-Amz-Algorithm={SIGV4_ALGORITHM}&X-Amz-Date={date}\
         &X-Amz-Expires={RESPONSE_URL_EXPIRES_SECONDS}&X-Amz-SignedHeaders={SIGNED_HEADERS}\
         &{SIGNATURE_PARAMETER}={signature}",
        host = response_host(region),
        segment = percent_encoded(&target),
        date = amz_date(SystemTime::now()),
    )
}

/// `now` in the basic-format instant a SigV4 query carries in `X-Amz-Date`:
/// `YYYYMMDDTHHMMSSZ`.
fn amz_date(now: SystemTime) -> String {
    let stamp = crate::state::timestamp(now)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default();
    // The extended form the formatter writes, with its separators dropped and its fractional
    // seconds — which the basic form does not carry — cut off.
    let whole = stamp.split('.').next().unwrap_or(&stamp).to_string();
    let whole = whole.trim_end_matches('Z');
    format!("{}Z", whole.replace(['-', ':'], ""))
}

/// The Region of the stack a response target names: the Region segment of that stack's own
/// ARN.
///
/// A write to this plane carries no credential — a pre-signed URL is followed unsigned — so
/// the Region it is about comes from what the URL itself names. The stack's ARN is the
/// authority on it, read through the workspace's one reading of a caller-supplied ARN's
/// Region ([`::service_common::arn::region_of`]), which holds the field to the grammar every
/// model's ARN pattern states for it: a target whose first part is no CloudFormation ARN, or
/// whose Region field could re-point an authority, names no stack of this service at all —
/// which is what keeps this plane's claim to its own URLs.
pub fn target_region(target: &ResponseTarget) -> Option<&str> {
    let region = ::service_common::arn::region_of(&target.stack_id, crate::state::ARN_NAMESPACE)?;
    // ...and the ARN must name a STACK: this plane answers for a resource of one.
    target
        .stack_id
        .split(':')
        .nth(5)?
        .starts_with("stack/")
        .then_some(region)
}

/// The `X-Amz-Signature` the published URL for one request carries.
///
/// A signature is over what the URL AUTHORISES, so it is derived from the target and the
/// window rather than drawn fresh: the same request's URL is the same URL however often the
/// request document is built, and [`signature_admits`] is what re-computes it for a write
/// that arrives — so the field is checked rather than decorative.
fn target_signature(target: &str) -> String {
    crate::state::stable_identifier_digits(
        &format!("{SIGNATURE_SALT}|{target}|{RESPONSE_URL_EXPIRES_SECONDS}"),
        SIGNATURE_HEX_CHARS,
    )
}

/// The query parameter a pre-signed URL carries its signature in.
const SIGNATURE_PARAMETER: &str = "X-Amz-Signature";

/// Whether `query` carries the signature this service published for `target` — the one write
/// the URL authorises.
///
/// A write whose signature is not the published one is a write to an address this service
/// did not hand out: the path alone would admit a target assembled by a caller, so the
/// signature is what the plane holds the write to ([`crate::service::custom_resource_plane`]),
/// exactly as a pre-signed URL whose signature does not verify is refused rather than served.
pub fn signature_admits(target: &ResponseTarget, query: &str) -> bool {
    let presented = query.split('&').find_map(|pair| {
        pair.strip_prefix(SIGNATURE_PARAMETER)
            .and_then(|rest| rest.strip_prefix('='))
    });
    let expected = target_signature(&format!(
        "{stack}{RESPONSE_PATH_SEPARATOR}{logical}{RESPONSE_PATH_SEPARATOR}{request}",
        stack = target.stack_id,
        logical = target.logical_resource_id,
        request = target.request_id,
    ));
    presented == Some(expected.as_str())
}

/// What a request to a published `ResponseURL` names, or `None` when the path is not one
///
/// Reading the path back is a syntax check only: whether it names a request this emulator is
/// waiting for an answer to is the store's judgement
/// ([`crate::state::ComAmazonawsCloudformationCloudformationState::record_provider_response`]).
pub fn parse_response_path(path: &str) -> Option<ResponseTarget> {
    // The query is not part of what the URL names: a pre-signed URL carries its signing
    // parameters there, and the target is the path alone.
    let path = path.split('?').next().unwrap_or(path);
    let segment = path.strip_prefix('/')?;
    // ONE path segment: the parts are joined inside it, so a raw separator means this is not
    // a URL this service published.
    if segment.is_empty() || segment.contains('/') {
        return None;
    }
    let decoded = ::percent_encoding::percent_decode_str(segment)
        .decode_utf8()
        .ok()?;
    let mut parts = decoded.split(RESPONSE_PATH_SEPARATOR);
    let (Some(stack_id), Some(logical_resource_id), Some(request_id), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return None;
    };
    if stack_id.is_empty() || logical_resource_id.is_empty() || request_id.is_empty() {
        return None;
    }
    Some(ResponseTarget {
        stack_id: stack_id.to_string(),
        logical_resource_id: logical_resource_id.to_string(),
        request_id: request_id.to_string(),
    })
}

/// One provider request, as the document it is sent as is built from.
#[derive(Debug, Clone, Copy)]
pub struct ProviderCall<'a> {
    /// `RequestType`: which stack operation this request is for
    /// ([`request_type`]).
    pub request_type: &'a str,
    /// `RequestId`: what the answer is addressed under ([`new_request_id`]).
    pub request_id: &'a str,
    /// `StackId`: "the ARN that identifies the stack that contains the custom resource".
    pub stack_id: &'a str,
    /// `LogicalResourceId`: "the template developer-chosen name (logical ID) of the custom
    /// resource in the template".
    pub logical_resource_id: &'a str,
    /// `ResourceType`: "the template developer-chosen resource type of the custom resource
    /// in the template".
    pub resource_type: &'a str,
    /// `ResourceProperties`: "this field contains the contents of the custom resource's
    /// `Properties` section of the template".
    pub properties: &'a Value,
    /// `PhysicalResourceId`: "a required custom resource provider-defined physical ID that
    /// is unique for that provider" — carried by an `Update` and a `Delete`, and by nothing
    /// else, because nothing exists yet when a `Create` is sent.
    pub physical_resource_id: Option<&'a str>,
    /// `OldResourceProperties`: "the resource property values that were previously declared
    /// by the template developer" — carried by an `Update`.
    pub old_properties: Option<&'a Value>,
}

impl ProviderCall<'_> {
    /// The record of this request the wait for its answer keeps ([`ProviderRequest`]).
    pub fn remembered(&self) -> ProviderRequest {
        ProviderRequest {
            request_type: self.request_type.to_string(),
            request_id: self.request_id.to_string(),
            physical_resource_id: self.physical_resource_id.map(str::to_string),
            properties: self.properties.clone(),
            old_properties: self.old_properties.cloned(),
        }
    }
}

/// The document a provider request is sent as: the members CloudFormation sends, and only
/// the ones this request has.
fn request_document(call: &ProviderCall<'_>, service_token: &str, response_url: &str) -> Value {
    let mut document = Map::new();
    document.insert("RequestType".to_string(), Value::String(call.request_type.to_string()));
    // The provider the request was sent TO, beside the copy of it the resource's own
    // properties carry: a subscriber of a topic several providers share reads which token the
    // request was published under off the request itself.
    document.insert(
        SERVICE_TOKEN.to_string(),
        Value::String(service_token.to_string()),
    );
    document.insert("ResponseURL".to_string(), Value::String(response_url.to_string()));
    document.insert("StackId".to_string(), Value::String(call.stack_id.to_string()));
    document.insert("RequestId".to_string(), Value::String(call.request_id.to_string()));
    document.insert("ResourceType".to_string(), Value::String(call.resource_type.to_string()));
    document.insert(
        "LogicalResourceId".to_string(),
        Value::String(call.logical_resource_id.to_string()),
    );
    if let Some(physical_resource_id) = call.physical_resource_id {
        document.insert(
            "PhysicalResourceId".to_string(),
            Value::String(physical_resource_id.to_string()),
        );
    }
    document.insert("ResourceProperties".to_string(), stringified(call.properties));
    if let Some(old) = call.old_properties {
        document.insert("OldResourceProperties".to_string(), stringified(old));
    }
    Value::Object(document)
}

/// `properties` with every SCALAR rendered as text, recursively through the objects and
/// lists that carry it.
///
/// A template's property values are text: CloudFormation "passes the template developer's
/// property values" to the provider, and what it passes a number or a boolean as is the
/// string the template stated it with — so a provider reading `ResourceProperties` reads
/// `"3"` and `"true"`, never a JSON number or a JSON boolean, however the template's own
/// JSON spelled them. The structure is untouched: an object stays an object and a list stays
/// a list, with each of their own scalars rendered the same way.
///
/// The walk is as deep as the document it walks, which is bounded where that document was
/// READ: a property document reaches this store only through a parse, and the one this crate
/// parses with rejects a document nested past its own limit, so no template can drive this
/// walk deeper than one parse admits.
fn stringified(properties: &Value) -> Value {
    match properties {
        Value::Object(entries) => Value::Object(
            entries
                .iter()
                .map(|(name, value)| (name.clone(), stringified(value)))
                .collect(),
        ),
        Value::Array(values) => Value::Array(values.iter().map(stringified).collect()),
        Value::String(_) | Value::Null => properties.clone(),
        scalar => Value::String(attribute_text(scalar)),
    }
}

/// The provider a custom resource's properties name, and the transport it is reached over.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Provider {
    /// An Amazon SNS topic: the request is PUBLISHED to it, so every subscriber of the topic
    /// receives it.
    Topic(String),
    /// An AWS Lambda function: the request is the event it is INVOKED with.
    Function(String),
}

/// The error code a refusal of the DECLARATION carries — a rule about the template rather
/// than an answer from the provider's own service.
const INVALID_REQUEST: &str = "InvalidRequest";

/// The provider `properties` name, or the refusal a declaration that names none — or names
/// something no provider can be reached at — fails the resource with.
fn provider_of(properties: &Value, resource_type: &str) -> Result<Provider, Refusal> {
    let token = properties
        .get(SERVICE_TOKEN)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| Refusal {
            code: INVALID_REQUEST.to_string(),
            message: format!("{resource_type} requires the {SERVICE_TOKEN} property"),
        })?;
    // `arn:<partition>:<service>:<region>:<account>:<resource>`: the SERVICE segment is what
    // says which transport the provider is reached over.
    let service = token
        .split(':')
        .nth(2)
        .filter(|_| token.starts_with("arn:"))
        .unwrap_or_default();
    match service {
        SNS_SIGNING_NAME => Ok(Provider::Topic(token.to_string())),
        LAMBDA_SIGNING_NAME => Ok(Provider::Function(token.to_string())),
        _ => Err(Refusal {
            code: INVALID_REQUEST.to_string(),
            message: format!(
                "the {SERVICE_TOKEN} of {resource_type} must be an Amazon SNS topic ARN or an \
                 AWS Lambda function ARN"
            ),
        }),
    }
}

/// Send one provider request, and answer once the provider's own service has ACCEPTED it.
///
/// `Ok(())` says the request was delivered to the `ServiceToken` — not that the provider has
/// answered it: the answer arrives at the `ResponseURL` and releases the resource's wait
/// ([`crate::state::stabilising`]). `Err` is the refusal of the service the token names, or
/// of the declaration that names no provider at all, and fails the resource exactly as a
/// refused create does.
///
/// The request never leaves this process. The `ServiceToken` is caller-supplied text, and it
/// is never resolved, connected to or used to address anything: what it names is a topic or a
/// function OF THIS EMULATOR, reached through the same signed, in-process callback every
/// other provisioning call rides ([`super::dispatch`]), whose endpoint this module fixes
/// itself from the called service's own signing name. A token naming a service no provider is
/// reached over is refused rather than dialled.
pub async fn send(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    call: &ProviderCall<'_>,
) -> Result<(), Refusal> {
    let provider = provider_of(call.properties, call.resource_type)?;
    let service_token = match &provider {
        Provider::Topic(token) | Provider::Function(token) => token.clone(),
    };
    let document = request_document(
        call,
        &service_token,
        &response_url(
            region,
            call.stack_id,
            call.logical_resource_id,
            call.request_id,
        ),
    );
    let message = serde_json::to_string(&document).map_err(|error| Refusal {
        code: "SerializationException".to_string(),
        message: error.to_string(),
    })?;
    match provider {
        // "CloudFormation sends a request to the SNS topic": the request document IS the
        // message, so every subscriber of the topic reads it as its own notification.
        Provider::Topic(topic_arn) => {
            let parameters = vec![
                ("TopicArn".to_string(), topic_arn),
                ("Message".to_string(), message),
            ];
            call_query(
                ctx,
                SNS_SIGNING_NAME,
                region,
                SNS_API_VERSION,
                "Publish",
                &parameters,
            )
            .await?;
            Ok(())
        }
        // The function is invoked with the request as its EVENT, and asynchronously: the
        // provider answers at the `ResponseURL`, so nothing waits for the invocation's own
        // response.
        Provider::Function(function_arn) => {
            let uri = format!(
                "/2015-03-31/functions/{}/invocations",
                percent_encoded(&function_arn)
            );
            let headers = vec![
                ("content-type", REST_JSON.to_string()),
                ("x-amz-invocation-type", INVOCATION_TYPE_EVENT.to_string()),
            ];
            let (status, payload) = dispatch(
                ctx,
                LAMBDA_SIGNING_NAME,
                region,
                "POST",
                &uri,
                headers,
                message.into_bytes(),
            )
            .await
            .map_err(|message| Refusal {
                code: "InternalFailure".to_string(),
                message,
            })?;
            if (200..300).contains(&status) {
                return Ok(());
            }
            let answered: Value = serde_json::from_slice(&payload).unwrap_or(Value::Null);
            Err(refusal_of(LAMBDA_SIGNING_NAME, status, &answered))
        }
    }
}

/// The region a value this module publishes may name: `region` when it is a well-formed
/// Region LABEL, else [`crate::state::DEFAULT_REGION`].
///
/// A credential scope is request-controlled text and this value is interpolated into a
/// HOST, so only a value the workspace's one reading of a Region label admits may reach it
/// ([`::service_common::arn::region_label`]: non-empty, bounded, and `[a-z0-9-]` only) —
/// which is what keeps every URL this module publishes one [`is_response_host`] recognises.
fn region_or_default(region: &str) -> &str {
    ::service_common::arn::region_label(region).unwrap_or(crate::state::DEFAULT_REGION)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The published `ResponseURL` is one this service recognises and reads back: its host is
    /// the one claimed and its path names the three parts the request was sent under,
    /// whatever those parts carry.
    #[test]
    fn every_published_response_url_is_read_back_whole() {
        let cases: &[(&str, &str, &str, &str, &str)] = &[
            (
                "a stack ARN, a plain logical id and a minted request id",
                "us-east-1",
                "arn:aws:cloudformation:us-east-1:000000000000:stack/probe/0123",
                "Thing",
                "11111111-2222-3333-4444-555555555555",
            ),
            (
                "another Region",
                "eu-west-2",
                "arn:aws:cloudformation:eu-west-2:000000000000:stack/other/4567",
                "Provisioner",
                "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            ),
            (
                "a credential scope that is no Region at all",
                "not a region",
                "arn:aws:cloudformation:us-east-1:000000000000:stack/probe/0123",
                "Thing",
                "11111111-2222-3333-4444-555555555555",
            ),
            (
                "a logical id at the length a template may declare",
                "ap-southeast-2",
                "arn:aws:cloudformation:ap-southeast-2:000000000000:stack/s/89ab",
                "AVeryLongLogicalResourceIdDeclaredByATemplate0123456789",
                "99999999-8888-7777-6666-555555555555",
            ),
        ];
        for (case, region, stack_id, logical_id, request_id) in cases {
            let url = response_url(region, stack_id, logical_id, request_id);
            let (host, path) = url
                .strip_prefix("https://")
                .and_then(|rest| rest.split_once('/'))
                .unwrap_or_else(|| panic!("case {case:?}: expected a published https URL, got {url}"));
            assert!(
                is_response_host(host),
                "case {case:?}: expected the published host to be claimed, got {host:?}"
            );
            // The published URL is a PRE-SIGNED one: its query carries the signing
            // parameters a client reads off it, and nothing of the target.
            let (_, query) = path
                .split_once('?')
                .unwrap_or_else(|| panic!("case {case:?}: expected a pre-signing query, got {url}"));
            for parameter in [
                "X-Amz-Algorithm=",
                "X-Amz-Date=",
                "X-Amz-Expires=",
                "X-Amz-SignedHeaders=",
                "X-Amz-Signature=",
            ] {
                assert!(
                    query.contains(parameter),
                    "case {case:?}: expected the query to carry {parameter}, got {query}"
                );
            }
            assert!(
                !query.contains("X-Amz-Credential") && !query.contains("AWSAccessKeyId"),
                "case {case:?}: a published URL names no credential, got {query}"
            );
            let read = parse_response_path(&format!("/{path}"));
            assert_eq!(
                read,
                Some(ResponseTarget {
                    stack_id: (*stack_id).to_string(),
                    logical_resource_id: (*logical_id).to_string(),
                    request_id: (*request_id).to_string(),
                }),
                "case {case:?}: expected the path to read back whole, got {read:?}"
            );
        }
    }

    /// A path that is not one this module publishes names no request.
    #[test]
    fn a_path_this_module_did_not_publish_names_nothing() {
        let cases: &[(&str, &str)] = &[
            ("the bare root", "/"),
            ("no separators at all", "/just-a-request-id"),
            ("one separator short", "/stack%7CThing"),
            ("one separator too many", "/stack%7CThing%7Cid%7Cextra"),
            ("an empty part", "/stack%7C%7Cid"),
            ("more than one segment", "/stack/Thing/id"),
        ];
        for (case, path) in cases {
            let read = parse_response_path(path);
            assert!(
                read.is_none(),
                "case {case:?}: expected no request to be named, got {read:?}"
            );
        }
    }

    /// The transport follows the SERVICE segment of the `ServiceToken`, and a declaration
    /// that names no provider this emulator can reach is refused rather than reported sent.
    #[test]
    fn the_service_token_decides_the_transport() {
        let topic = serde_json::json!({ "ServiceToken": "arn:aws:sns:us-east-1:000000000000:p" });
        let function = serde_json::json!({
            "ServiceToken": "arn:aws:lambda:us-east-1:000000000000:function:p"
        });
        assert_eq!(
            provider_of(&topic, "Custom::Probe").ok(),
            Some(Provider::Topic(
                "arn:aws:sns:us-east-1:000000000000:p".to_string()
            )),
            "case \"an SNS topic ARN\": expected the topic transport"
        );
        assert_eq!(
            provider_of(&function, "Custom::Probe").ok(),
            Some(Provider::Function(
                "arn:aws:lambda:us-east-1:000000000000:function:p".to_string()
            )),
            "case \"a Lambda function ARN\": expected the function transport"
        );
        let refused: &[(&str, Value)] = &[
            ("no properties at all", Value::Null),
            ("no ServiceToken", serde_json::json!({ "Marker": "v1" })),
            ("an empty ServiceToken", serde_json::json!({ "ServiceToken": "" })),
            (
                "a token that is no ARN",
                serde_json::json!({ "ServiceToken": "a-topic" }),
            ),
            (
                "an ARN of a service no provider is reached over",
                serde_json::json!({ "ServiceToken": "arn:aws:sqs:us-east-1:000000000000:q" }),
            ),
        ];
        for (case, properties) in refused {
            let answered = provider_of(properties, "Custom::Probe");
            assert!(
                answered.is_err(),
                "case {case:?}: expected a refusal of the declaration, got {answered:?}"
            );
        }
    }

    /// The document a request is SENT as carries the members CloudFormation sends for that
    /// `RequestType` and no others, and every scalar of its property documents as TEXT —
    /// recursively, through the objects and lists that carry them.
    ///
    /// The rows differ in the request type (which decides which members travel) and in what
    /// the properties carry (a string, a number, a boolean, a list, a nested object).
    #[test]
    fn a_request_document_carries_its_members_with_every_scalar_as_text() {
        let properties = serde_json::json!({
            "ServiceToken": "arn:aws:sns:us-east-1:000000000000:p",
            "Name": "a-name",
            "Count": 3,
            "Flag": true,
            "Fraction": 1.5,
            "Nested": { "a": [1, "x", false] },
        });
        let old = serde_json::json!({ "Count": 2 });
        /// One row: the request type, the physical resource it names, the properties it was
        /// last declared with, and the members that must and must not travel with it.
        struct Row<'a> {
            case: &'a str,
            request_type: &'a str,
            physical_resource_id: Option<&'a str>,
            old_properties: Option<&'a Value>,
            present: &'a [&'a str],
            absent: &'a [&'a str],
        }
        let rows: &[Row<'_>] = &[
            Row {
                case: "a create",
                request_type: request_type::CREATE,
                physical_resource_id: None,
                old_properties: None,
                present: &["RequestType", "ServiceToken", "ResponseURL", "StackId", "RequestId",
                  "ResourceType", "LogicalResourceId", "ResourceProperties"],
                absent: &["PhysicalResourceId", "OldResourceProperties"],
            },
            Row {
                case: "an update",
                request_type: request_type::UPDATE,
                physical_resource_id: Some("provider-minted-id"),
                old_properties: Some(&old),
                present: &["RequestType", "PhysicalResourceId", "ResourceProperties",
                  "OldResourceProperties"],
                absent: &[],
            },
            Row {
                case: "a delete",
                request_type: request_type::DELETE,
                physical_resource_id: Some("provider-minted-id"),
                old_properties: None,
                present: &["RequestType", "PhysicalResourceId", "ResourceProperties"],
                absent: &["OldResourceProperties"],
            },
        ];
        for row in rows {
            let Row {
                case,
                request_type,
                physical_resource_id,
                old_properties,
                present,
                absent,
            } = row;
            let call = ProviderCall {
                request_type,
                request_id: "11111111-2222-3333-4444-555555555555",
                stack_id: "arn:aws:cloudformation:us-east-1:000000000000:stack/probe/0123",
                logical_resource_id: "Thing",
                resource_type: "Custom::Probe",
                properties: &properties,
                physical_resource_id: *physical_resource_id,
                old_properties: *old_properties,
            };
            let document = request_document(&call, "arn:aws:sns:us-east-1:000000000000:p", "https://host/x");
            for member in *present {
                assert!(
                    document.get(*member).is_some(),
                    "case {case:?}: the request must carry {member}, got {document}"
                );
            }
            for member in *absent {
                assert!(
                    document.get(*member).is_none(),
                    "case {case:?}: the request must not carry {member}, got {document}"
                );
            }
            assert_eq!(
                document.get("ResourceProperties"),
                Some(&serde_json::json!({
                    "ServiceToken": "arn:aws:sns:us-east-1:000000000000:p",
                    "Name": "a-name",
                    "Count": "3",
                    "Flag": "true",
                    "Fraction": "1.5",
                    "Nested": { "a": ["1", "x", "false"] },
                })),
                "case {case:?}: every scalar of the properties must travel as text, got \
                 {document}"
            );
            if old_properties.is_some() {
                assert_eq!(
                    document.get("OldResourceProperties"),
                    Some(&serde_json::json!({ "Count": "2" })),
                    "case {case:?}: the old properties travel the same way, got {document}"
                );
            }
        }
    }

    /// A provider's answer is read for the two members it must carry, and `Data` reaches the
    /// resource as the text `Fn::GetAtt` resolves to.
    #[test]
    fn an_answer_is_read_for_the_members_it_must_carry() {
        let answered = ProviderResponse::read(&serde_json::json!({
            "Status": "SUCCESS",
            "PhysicalResourceId": "provider-minted-id",
            "Data": { "Endpoint": "https://example.invalid", "Count": 3, "On": true },
        }))
        .expect("a well-formed success must be read");
        assert!(
            answered.succeeded(),
            "case \"a success\": expected the answer to be a success, got {answered:?}"
        );
        assert_eq!(
            answered.physical_resource_id, "provider-minted-id",
            "case \"a success\": expected the provider's own id, got {answered:?}"
        );
        assert_eq!(
            answered.data.get("Count").map(String::as_str),
            Some("3"),
            "case \"a success\": expected a non-string Data value as its JSON text, got \
             {answered:?}"
        );
        let refused: &[(&str, Value)] = &[
            ("no Status", serde_json::json!({ "PhysicalResourceId": "id" })),
            (
                "a Status outside the two the protocol admits",
                serde_json::json!({ "Status": "Succeeded", "PhysicalResourceId": "id" }),
            ),
            ("no PhysicalResourceId", serde_json::json!({ "Status": "SUCCESS" })),
            (
                "an empty PhysicalResourceId",
                serde_json::json!({ "Status": "SUCCESS", "PhysicalResourceId": "  " }),
            ),
        ];
        for (case, document) in refused {
            let read = ProviderResponse::read(document);
            assert!(
                read.is_err(),
                "case {case:?}: expected the answer to be refused, got {read:?}"
            );
        }
        let failed = ProviderResponse::read(&serde_json::json!({
            "Status": "FAILED",
            "PhysicalResourceId": "id",
        }))
        .expect("a well-formed failure must be read");
        assert_eq!(
            failed.failure_reason(),
            UNSTATED_REASON,
            "case \"a failure stating no reason\": expected the fixed sentence, got {failed:?}"
        );
    }
}
