// SPDX-License-Identifier: CC0-1.0

//! The data-transformation endpoint's REST tagging surface: `GET /tags/{ResourceArn}`,
//! `POST /tags/{ResourceArn}` and `POST /untag/{ResourceArn}`.
//!
//! # Why these are not generated operations
//!
//! The shipped model DOES declare all three — `ListTagsForResource`, `TagResource`
//! and `UntagResource`, each with an `@http` binding whose URI carries the
//! `ResourceArn` as an `@httpLabel` — but under the
//! `com.amazon.awshealthlakedatatransformationfrontendservice` namespace, OUTSIDE the
//! closure of the service shape this crate serves (`com.amazonaws.healthlake#HealthLake`,
//! whose bound operations `smithy_query(query="digest")` lists and which includes none
//! of them). So no module is generated for any of them, nothing is in the routing
//! table, and this crate's awsJson1_0 routing key — the `X-Amz-Target` header a REST
//! call like these does not carry — cannot name them. They are therefore reached
//! through [`crate::service::EmulatedService::override_operation`], the one seam a
//! service has for a request the model's routing key cannot name, exactly as the
//! sibling out-of-closure REST operation [`super::validate_source`] is.
//!
//! # One surface over one tag map
//!
//! These three address the tags of a resource this service already holds, so nothing
//! about the tag state is new here: the ARN is resolved, and the tags read and
//! written, through the SAME state helpers the bound awsJson tagging operations use
//! ([`crate::state::ComAmazonawsHealthlakeHealthlakeState::tags_for_report`] for the
//! report, [`crate::state::ComAmazonawsHealthlakeHealthlakeState::tags_for_mutation`]
//! plus [`crate::state::MutableTags`] for the two mutations). A tag written over
//! either wire is therefore reported by both, and a resource no record holds is
//! refused with the same fixed sentence whichever wire asked.
//!
//! # The contract
//!
//! Taken from the model shapes rather than invented:
//!
//! * `GET /tags/{ResourceArn}` reports the resource's tags as the `Tags` member
//!   `ListTagsForResourceResponse` requires — the modelled `TagMap`, a JSON object of
//!   tag key to tag value — at HTTP 200;
//! * `POST /tags/{ResourceArn}` merges the request's required `Tags` map onto the
//!   resource, by key, and answers the modelled `Unit` output: HTTP 200 with no body;
//! * `POST /untag/{ResourceArn}` removes the keys the required `TagKeys` list names
//!   and answers the same empty 200.
//!
//! The refusals are the shapes the three operations declare, each at the status its
//! own `@httpError` names: `ValidationException` at 400 for a request that breaks a
//! rule the model states, `ResourceNotFoundException` at 404 for an ARN naming a
//! resource this service does not hold, and `InternalServerException` at 500 for a
//! failure of the service itself. The `@httpError` status is what a PATH-addressed
//! surface answers with — unlike the awsJson1_0 control plane, where
//! [`crate::scaffolded::wire_http_status`] folds every client fault onto 400 because
//! that protocol binds nothing to the transport.

use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, OperationContext, Service, Violation, ViolationKind,
};

use super::non_rpc::{bounded, modelled_error, request_path};

/// The first path segment of the reporting and tagging operations, from their
/// modelled `@http(uri: "/tags/{ResourceArn}")`.
pub const TAGS_PATH_SEGMENT: &str = "tags";

/// The first path segment of the untagging operation, from its modelled
/// `@http(uri: "/untag/{ResourceArn}")`.
pub const UNTAG_PATH_SEGMENT: &str = "untag";

/// The member name the request's tag map is sent under, as
/// `TagResourceRequest$Tags` spells it.
const TAGS_MEMBER: &str = "Tags";

/// The member name the request's tag keys are sent under, as
/// `UntagResourceRequest$TagKeys` spells it.
const TAG_KEYS_MEMBER: &str = "TagKeys";

/// The largest tag map a request may carry, from the modelled `TagMap`
/// `@length(min: 0, max: 50)`.
const MAX_TAGS: usize = 50;

/// The fewest tag keys an untag request may name, from the modelled `TagKeyList`
/// `@length(min: 1, max: 50)`.
const MIN_TAG_KEYS: usize = 1;

/// The most tag keys an untag request may name, from that same `@length`.
const MAX_TAG_KEYS: usize = 50;

/// The shortest tag key either request admits, from the modelled
/// `DataTransformationTagKey` `@length(min: 1, max: 128)`.
const MIN_TAG_KEY_LEN: usize = 1;

/// The longest tag key either request admits, from that same `@length`.
const MAX_TAG_KEY_LEN: usize = 128;

/// The longest tag value a tag request admits, from the modelled
/// `DataTransformationTagValue` `@length(min: 0, max: 256)`.
const MAX_TAG_VALUE_LEN: usize = 256;

/// Which of the three REST tagging operations a request addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaggingOperation {
    /// `GET /tags/{ResourceArn}` — report the resource's tags.
    Report,
    /// `POST /tags/{ResourceArn}` — merge tags onto the resource.
    Merge,
    /// `POST /untag/{ResourceArn}` — remove tags from the resource.
    Remove,
}

/// A request on this surface, as its method and path name it.
#[derive(Debug)]
pub(crate) struct TaggingRoute {
    /// The operation the method and the first path segment together name.
    pub(crate) operation: TaggingOperation,
    /// The `ResourceArn` the `@httpLabel` carries, percent-decoded.
    pub(crate) resource_arn: String,
}

/// The tagging request `method` and `uri` address, or `None` when they address no
/// operation of this surface.
///
/// The ONE reader of this path space: [`crate::service::EmulatedService::override_operation`]
/// routes with it and [`RestTagging`] serves with it, so the routing decision and the
/// handler cannot read a request differently.
///
/// Only the three method/path pairs the model declares are claimed. Anything else on
/// these segments — another method, or a path carrying no label at all, which an
/// `@httpLabel` cannot be empty for — names no modelled operation and is left to the
/// framework's unroutable-operation path rather than answered here with a status
/// nothing modelled it with.
///
/// The label is the remainder of the path: a HealthLake ARN carries `:` and `/`, so a
/// client percent-encodes the separator (`%2F`) and the decoding is the framework's own
/// ([`smithy_server_runtime::percent_decode`]) — while a client that sent the `/`
/// unencoded spread the one label over several segments, which rejoining recovers.
pub(crate) fn addressed_tagging(method: &str, uri: &str) -> Option<TaggingRoute> {
    let path = request_path(uri);
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    let operation = match (method, segments.next()?) {
        ("GET", TAGS_PATH_SEGMENT) => TaggingOperation::Report,
        ("POST", TAGS_PATH_SEGMENT) => TaggingOperation::Merge,
        ("POST", UNTAG_PATH_SEGMENT) => TaggingOperation::Remove,
        _ => return None,
    };
    let label: Vec<&str> = segments.collect();
    if label.is_empty() {
        return None;
    }
    Some(TaggingRoute {
        operation,
        resource_arn: smithy_server_runtime::percent_decode(&label.join("/")),
    })
}

/// The data-transformation endpoint's REST tagging surface, as the one
/// [`ErasedOperation`] the service holds for it.
///
/// Owns the shared engine exactly as a generated operation does, so the tags it writes
/// live in the SAME per-service store the control plane's handlers read.
pub struct RestTagging<Eng> {
    engine: Arc<Eng>,
}

impl<Eng> RestTagging<Eng> {
    /// Build the surface from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for RestTagging<Eng> {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move { self.serve(req) })
    }
}

impl<Eng: Send + Sync + 'static> RestTagging<Eng> {
    /// Serve one request on this surface.
    fn serve(&self, req: &RtRequest) -> RtResponse {
        let Some(route) = addressed_tagging(req.method(), req.uri()) else {
            // `override_operation` routes only what `addressed_tagging` accepts, so
            // this is unreachable; answering the modelled server fault rather than
            // panicking keeps the handler total.
            return modelled_error(
                500,
                INTERNAL_SERVER_EXCEPTION,
                "The server did not read the request path as a tagging path",
            );
        };
        match route.operation {
            TaggingOperation::Report => self.report(&route.resource_arn),
            TaggingOperation::Merge => match req.body().bytes() {
                Some(body) => self.merge(&route.resource_arn, body),
                // The router buffers every request before dispatch, so a body it
                // cannot hand back in memory is a server-side failure, never a
                // caller's malformed request.
                None => unreadable_body(),
            },
            TaggingOperation::Remove => match req.body().bytes() {
                Some(body) => self.remove(&route.resource_arn, body),
                None => unreadable_body(),
            },
        }
    }

    /// `GET /tags/{ResourceArn}`: report the stored tags of the resource the ARN
    /// names.
    ///
    /// A report, so it writes nothing of its own; the lifecycle settle is the clock
    /// catching up, which was owed at this instant whether or not anybody called.
    fn report(&self, resource_arn: &str) -> RtResponse {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return store_unavailable();
        };
        let mut store = state.lock();
        store.settle_lifecycle();
        match store.tags_for_report(resource_arn) {
            Ok(tags) => {
                let reported: ::serde_json::Map<String, ::serde_json::Value> = tags
                    .into_iter()
                    .map(|tag| (tag.key, ::serde_json::Value::String(tag.value)))
                    .collect();
                let mut body = ::serde_json::Map::new();
                body.insert(
                    TAGS_MEMBER.to_string(),
                    ::serde_json::Value::Object(reported),
                );
                json_response(SUCCESS_STATUS, &::serde_json::Value::Object(body))
            }
            Err(not_found) => resource_not_found(&not_found),
        }
    }

    /// `POST /tags/{ResourceArn}`: merge the request's tags, by key, onto the resource
    /// the ARN names.
    fn merge(&self, resource_arn: &str, body: &[u8]) -> RtResponse {
        let requested = match requested_tags(body) {
            Ok(requested) => requested,
            Err(refused) => return validation_exception(&refused),
        };
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return store_unavailable();
        };
        let mut store = state.lock();
        // Settle first, so the status this write is held against is the one the clock
        // already owes — a data store whose deletion has fallen due is DELETED here,
        // not still DELETING.
        store.settle_lifecycle();
        let mut target = match store.tags_for_mutation(resource_arn) {
            Ok(target) => target,
            Err(not_found) => return resource_not_found(&not_found),
        };
        match target.merge(requested) {
            Ok(()) => empty_response(),
            Err(refused) => validation_exception(&refused),
        }
    }

    /// `POST /untag/{ResourceArn}`: remove the keys the request names from the
    /// resource the ARN names.
    fn remove(&self, resource_arn: &str, body: &[u8]) -> RtResponse {
        let requested = match requested_tag_keys(body) {
            Ok(requested) => requested,
            Err(refused) => return validation_exception(&refused),
        };
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return store_unavailable();
        };
        let mut store = state.lock();
        store.settle_lifecycle();
        let mut target = match store.tags_for_mutation(resource_arn) {
            Ok(target) => target,
            Err(not_found) => return resource_not_found(&not_found),
        };
        target.remove_keys(requested);
        empty_response()
    }
}

/// The `Tags` the request carries, or the sentence its refusal states.
///
/// Every rule applied is one the model declares on the request shape: `Tags` is
/// `@required`, its `TagMap` target is `@length(min: 0, max: 50)`, and the key and
/// value shapes carry their own `@length`. They are stated here because this surface
/// is routed by path and so never passes through the generated operations'
/// schema-driven gate ([`crate::service`]'s input-validation note); the wording is
/// that gate's own ([`crate::service::validation_envelope`]), so a refusal here reads
/// exactly like a refusal of any other HealthLake member.
fn requested_tags(body: &[u8]) -> Result<Vec<(String, String)>, String> {
    let members = request_members(body)?;
    let Some(sent) = members.get(TAGS_MEMBER) else {
        return Err(missing_member(TAGS_MEMBER));
    };
    let Some(sent) = sent.as_object() else {
        return Err(not_of_modelled_type(
            TAGS_MEMBER,
            "a JSON object of tag key to tag value",
        ));
    };

    // The collection's own bound is decided BEFORE its entries are walked, and
    // refuses on its own: a collection past the maximum is already outside the model,
    // so nothing after this point is sized by a count the caller chose.
    if sent.len() > MAX_TAGS {
        return Err(super::validation_envelope(&[collection_too_long(
            TAGS_MEMBER, MAX_TAGS,
        )]));
    }

    let mut violations = Vec::new();
    let mut requested = Vec::with_capacity(sent.len());
    for (key, value) in sent {
        let Some(value) = value.as_str() else {
            return Err(not_of_modelled_type(
                &entry_path(TAGS_MEMBER, key),
                "a tag value, as a JSON string",
            ));
        };
        violations.extend(tag_key_violations(TAGS_MEMBER, key));
        if value.chars().count() > MAX_TAG_VALUE_LEN {
            violations.push(Violation {
                path: entry_path(TAGS_MEMBER, key),
                kind: ViolationKind::MaxLength(bound(MAX_TAG_VALUE_LEN)),
                value: Some(bounded(value)),
                sensitive: false,
            });
        }
        requested.push((key.clone(), value.to_string()));
    }
    if violations.is_empty() {
        Ok(requested)
    } else {
        Err(super::validation_envelope(&violations))
    }
}

/// The `TagKeys` the request names, or the sentence its refusal states.
///
/// The rules are the model's, exactly as [`requested_tags`]'s are: `TagKeys` is
/// `@required`, its `TagKeyList` target is `@length(min: 1, max: 50)`, and each element
/// is a `DataTransformationTagKey`.
fn requested_tag_keys(body: &[u8]) -> Result<Vec<String>, String> {
    let members = request_members(body)?;
    let Some(sent) = members.get(TAG_KEYS_MEMBER) else {
        return Err(missing_member(TAG_KEYS_MEMBER));
    };
    let Some(sent) = sent.as_array() else {
        return Err(not_of_modelled_type(
            TAG_KEYS_MEMBER,
            "a JSON array of tag keys",
        ));
    };

    // Both of the collection's own bounds are decided before its elements are walked,
    // for the reason [`requested_tags`]'s is.
    if sent.len() < MIN_TAG_KEYS {
        return Err(super::validation_envelope(&[Violation {
            path: member_path(TAG_KEYS_MEMBER),
            kind: ViolationKind::MinLength(bound(MIN_TAG_KEYS)),
            // Deliberately absent: echoing a whole collection back would let a
            // request amplify itself into the response.
            value: None,
            sensitive: false,
        }]));
    }
    if sent.len() > MAX_TAG_KEYS {
        return Err(super::validation_envelope(&[collection_too_long(
            TAG_KEYS_MEMBER,
            MAX_TAG_KEYS,
        )]));
    }

    let mut violations = Vec::new();
    let mut requested = Vec::with_capacity(sent.len());
    for (index, element) in sent.iter().enumerate() {
        let Some(key) = element.as_str() else {
            return Err(not_of_modelled_type(
                &element_path(TAG_KEYS_MEMBER, index),
                "a tag key, as a JSON string",
            ));
        };
        violations.extend(key_length_violations(
            &element_path(TAG_KEYS_MEMBER, index),
            key,
        ));
        requested.push(key.to_string());
    }
    if violations.is_empty() {
        Ok(requested)
    } else {
        Err(super::validation_envelope(&violations))
    }
}

/// The members a request body carries.
///
/// An empty body carries no member at all, which is what the required-member rule
/// above then states. A body that is not a JSON object carries no members either, but
/// it is not a statement about one member, so it is refused as a whole.
fn request_members(body: &[u8]) -> Result<::serde_json::Map<String, ::serde_json::Value>, String> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(::serde_json::Map::new());
    }
    match ::serde_json::from_slice::<::serde_json::Value>(body) {
        Ok(::serde_json::Value::Object(members)) => Ok(members),
        Ok(_) | Err(_) => Err("The request body must be a JSON object".to_string()),
    }
}

/// How a required member left out of the request is stated: the envelope the
/// framework's own constraint walk words an absent `@required` member with.
fn missing_member(member: &str) -> String {
    super::validation_envelope(&[Violation {
        path: member_path(member),
        kind: ViolationKind::MissingRequiredMember,
        // An absent member has no value to echo.
        value: None,
        sensitive: false,
    }])
}

/// How a member whose JSON type is not the modelled shape's is stated.
///
/// This is not a constraint violation — the value never became a value of the
/// modelled type at all — so it states what the member must be instead of claiming a
/// constraint the model does not declare. It is still the modelled
/// `ValidationException` that carries it: that shape is what this service declares
/// for an invalid user input parameter, and a member of the wrong type is one.
fn not_of_modelled_type(path: &str, expected: &str) -> String {
    format!("Value at '{path}' must be {expected}")
}

/// A collection longer than its modelled `@length` maximum.
fn collection_too_long(member: &str, max: usize) -> Violation {
    Violation {
        path: member_path(member),
        kind: ViolationKind::MaxLength(bound(max)),
        // Deliberately absent, as for the minimum above: a collection is as long as
        // the caller chose, so echoing it would amplify the request.
        value: None,
        sensitive: false,
    }
}

/// Everything a tag key of the map member `member` breaks.
///
/// A map KEY has no path of its own — an entry is identified BY its key — so a key
/// that breaks its shape's bound is reported at the map member, quoting the key as the
/// offending value.
fn tag_key_violations(member: &str, key: &str) -> Vec<Violation> {
    key_length_violations(&member_path(member), key)
}

/// Everything the tag key `key` breaks, reported at `path`.
fn key_length_violations(path: &str, key: &str) -> Vec<Violation> {
    let length = key.chars().count();
    let kind = if length < MIN_TAG_KEY_LEN {
        ViolationKind::MinLength(bound(MIN_TAG_KEY_LEN))
    } else if length > MAX_TAG_KEY_LEN {
        ViolationKind::MaxLength(bound(MAX_TAG_KEY_LEN))
    } else {
        return Vec::new();
    };
    vec![Violation {
        path: path.to_string(),
        kind,
        value: Some(bounded(key)),
        sensitive: false,
    }]
}

/// A `@length` bound as a violation carries it.
fn bound(bound: usize) -> i64 {
    i64::try_from(bound).unwrap_or(i64::MAX)
}

/// The violation path of the member `member`: its model name in the lowerCamel
/// spelling a path takes.
fn member_path(member: &str) -> String {
    ::aws_smithy_schema_server::constraints::path::member(member)
}

/// The violation path of the map entry `key` of the member `member`.
///
/// The key is bounded in the path for the reason every echoed value is: the key's own
/// bound is checked first, so a key past it is stated by that violation rather than
/// spread through this one.
fn entry_path(member: &str, key: &str) -> String {
    format!("{}.{}", member_path(member), bounded(key))
}

/// The violation path of the element at the zero-based `index` of the list member
/// `member`, which a path names by its ONE-based position.
fn element_path(member: &str, index: usize) -> String {
    ::aws_smithy_schema_server::constraints::path::join(&[
        member_path(member),
        ::aws_smithy_schema_server::constraints::path::element(index + 1),
    ])
}

/// The shape name the modelled server fault is reported under.
const INTERNAL_SERVER_EXCEPTION: &str = "InternalServerException";

/// The modelled `ValidationException`, at the HTTP 400 its `@httpError` names.
fn validation_exception(message: &str) -> RtResponse {
    modelled_error(400, "ValidationException", message)
}

/// The modelled `ResourceNotFoundException`, at the HTTP 404 its `@httpError` names,
/// stating the sentence the state resolver reported — the same sentence the awsJson
/// tagging operations answer with.
fn resource_not_found(message: &str) -> RtResponse {
    modelled_error(404, "ResourceNotFoundException", message)
}

/// The answer when this service's store cannot be reached: a server fault, never a
/// fresh store and never an empty tag set.
fn store_unavailable() -> RtResponse {
    modelled_error(
        500,
        INTERNAL_SERVER_EXCEPTION,
        "The HealthLake data store registry is unavailable",
    )
}

/// The answer when the request body cannot be read in memory at all: the modelled
/// server fault, for the reason [`super::validate_source`] answers one.
fn unreadable_body() -> RtResponse {
    modelled_error(
        500,
        INTERNAL_SERVER_EXCEPTION,
        "The server could not read the submitted tags from the request body",
    )
}

/// One JSON response of this surface.
fn json_response(status: u16, body: &::serde_json::Value) -> RtResponse {
    let mut response = RtResponse::new(
        StatusCode::try_from(status).expect("the status is a constant of this module"),
        SdkBody::from(body.to_string()),
    );
    response
        .headers_mut()
        .insert("content-type", JSON_CONTENT_TYPE);
    response
}

/// The media type this surface's reports travel as.
const JSON_CONTENT_TYPE: &str = "application/json";

/// The answer of an operation whose modelled output is `Unit`: the success status its
/// `@http` binding names, with no body — there is no member to carry, and an invented
/// one would look like an answer the model does not declare.
fn empty_response() -> RtResponse {
    RtResponse::new(
        StatusCode::try_from(SUCCESS_STATUS).expect("200 is a valid status"),
        SdkBody::empty(),
    )
}

/// The success status every operation of this surface declares
/// (`@http(code: 200)`).
const SUCCESS_STATUS: u16 = 200;

#[cfg(test)]
mod tests {
    //! The path rule this surface is routed on, over the shapes a client sends it in.

    use super::*;

    /// A tagging path names its operation by method and first segment, and carries the
    /// whole `ResourceArn` as its label however the client encoded the separator.
    #[test]
    fn a_tagging_path_names_its_operation_and_its_resource_arn() {
        let arn = "arn:aws:healthlake:us-east-1:123456789012:dataTransformationProfile/\
                   0123456789abcdef0123456789abcdef";
        let encoded = arn.replace(':', "%3A").replace('/', "%2F");
        /// One row: the case, the request method and URI, and the operation and ARN
        /// the pair names (`None` when it addresses no operation of this surface).
        type Row = (&'static str, &'static str, String, Option<(TaggingOperation, &'static str)>);
        let rows: Vec<Row> = vec![
            (
                "a report with the ARN percent-encoded",
                "GET",
                format!("/tags/{encoded}"),
                Some((TaggingOperation::Report, arn)),
            ),
            (
                "a report with the separator sent raw",
                "GET",
                format!("/tags/{arn}"),
                Some((TaggingOperation::Report, arn)),
            ),
            (
                "a merge",
                "POST",
                format!("/tags/{encoded}"),
                Some((TaggingOperation::Merge, arn)),
            ),
            (
                "a removal",
                "POST",
                format!("/untag/{encoded}"),
                Some((TaggingOperation::Remove, arn)),
            ),
            (
                "a removal with a query string the surface declares no member for",
                "POST",
                format!("/untag/{encoded}?x=1"),
                Some((TaggingOperation::Remove, arn)),
            ),
            (
                "a data store ARN, the other resource form the tagging state holds",
                "GET",
                "/tags/arn%3Aaws%3Ahealthlake%3Aus-east-1%3A123456789012%3Adatastore%2Ffhir%2F\
                 00000000000000000000000000000000"
                    .to_string(),
                Some((
                    TaggingOperation::Report,
                    "arn:aws:healthlake:us-east-1:123456789012:datastore/fhir/\
                     00000000000000000000000000000000",
                )),
            ),
            (
                "a method no tagging operation declares",
                "DELETE",
                format!("/tags/{encoded}"),
                None,
            ),
            (
                "an untag by GET, which the model does not declare",
                "GET",
                format!("/untag/{encoded}"),
                None,
            ),
            ("no label at all", "GET", "/tags".to_string(), None),
            (
                "no label at all, with the trailing separator",
                "POST",
                "/tags/".to_string(),
                None,
            ),
            ("another path space", "GET", "/validate-source".to_string(), None),
            ("the control plane's root path", "POST", "/".to_string(), None),
        ];

        for (case, method, uri, expected) in rows {
            let actual = addressed_tagging(method, &uri);
            let named = actual
                .as_ref()
                .map(|route| (route.operation, route.resource_arn.as_str()));
            assert_eq!(
                named, expected,
                "case {case:?} ({method} {uri}): expected {expected:?}, got {named:?}"
            );
        }
    }
}
