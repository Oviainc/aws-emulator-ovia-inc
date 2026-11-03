// SPDX-License-Identifier: CC0-1.0

//! How this service's `restXml` responses serialize an UNWRAPPED output document.
//!
//! One S3 operation answers a document with no envelope at all: `GetBucketLocation`
//! sends `<LocationConstraint>us-west-2</LocationConstraint>` as the whole body, the
//! single output member promoted to the response root. The model says so twice — the
//! output shape carries `@xmlName("LocationConstraint")` and the operation carries
//! `aws.customizations#s3UnwrappedXmlOutput`
//! (`smithy_query(query="operation",operation="GetBucketLocation",
//! model_path="services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json")`
//! reports `s3UnwrappedXmlOutput: true`) — and every SDK reads the Region off that
//! root's own text: botocore's `parse_get_bucket_location` takes `root.text` and reports
//! `None` for anything nested.
//!
//! The framework's generic restXml serializer opens the output shape's element and then
//! writes each member inside it, which for this one operation produces the member nested
//! under an identically-named root — a document whose root carries no text, so the Region
//! is lost. [`UnwrappedXmlRestXml`] is the protocol `service.rs` hands the dispatch
//! sequence through `Service::override_protocol`: every operation but the unwrapped ones
//! is the framework's own restXml protocol, delegated to unchanged, and an operation named
//! by [`UNWRAPPED_XML_OUTPUT_OPERATIONS`] has its single member written AS the root.
//!
//! This is the one place that rule lives, so no handler and no shape declaration mentions
//! the envelope at all.
//!
//! ## `@xmlAttribute`: a member that is an ATTRIBUTE, not an element
//!
//! The same protocol carries the second thing the framework's generic XML codec cannot
//! express. An ACL grantee's `Type` is an XML ATTRIBUTE of its own element:
//! `smithy_query(query="shape",shape="Grantee")` reports
//! `Type: {xmlName: "xsi:type", xmlAttribute: true, required: true}`, and the `xsi` prefix
//! stands for the namespace `Grant$Grantee` declares
//! (`smithy_query(query="shape",shape="Grant")` reports
//! `xmlNamespace {uri: "http://www.w3.org/2001/XMLSchema-instance", prefix: "xsi"}`), so
//! every S3 client sends and reads
//! `<Grantee xmlns:xsi="…" xsi:type="Group"><URI>…</URI></Grantee>`.
//!
//! The framework's codec reads a structure's members from its CHILD ELEMENTS and writes
//! them the same way, with no notion of an attribute, so unaided it refuses every real
//! `AccessControlPolicy` for the missing required member and answers an `<xsi:type>`
//! element whose prefix nothing declares — a document a namespace-aware SDK parser cannot
//! read at all. [`XML_ATTRIBUTE_MEMBERS`] is the one place that mapping lives:
//! [`attributes_as_elements`] normalizes an incoming attribute into the element the codec
//! binds, and [`elements_as_attributes`] puts the outgoing element back where the API
//! publishes it. Both are applied ONLY to a document that carries one, so every other
//! request and response goes through the framework's own path untouched.
//!
//! A `Grantee` is not the ACL's alone: `LoggingEnabled$TargetGrants` reaches one too
//! (`smithy_query(query="shape",shape="TargetGrant")`), so both normalizations are keyed on
//! the DOCUMENT — a request whose body is an XML document, a response whose body spells the
//! attribute — and never on which operation is being served.
//!
//! ## `@xmlName` on a UNION member: an element the codec does not match
//!
//! The same protocol carries one more thing the framework's generic XML codec cannot
//! express. A union's set member is read from the child element whose local name is the
//! member's own SMITHY name, so a union member carrying an `@xmlName` arrives under a name
//! that matches nothing and the union deserializes as "no member set" — a request every S3
//! client sends correctly, refused for a member it did set.
//! `UpdateObjectEncryption`'s payload is the one such document in this service:
//! `smithy_query(query="shape",shape="ObjectEncryption")` reports its single member
//! `SSEKMS` with `xmlName: "SSE-KMS"`. [`XML_NAMED_UNION_MEMBERS`] is the one place that
//! mapping lives, and [`published_names_as_members`] normalizes an incoming document onto
//! the member name the codec binds. Nothing is normalized on the way OUT: no operation in
//! this service answers a union.
//!
//! ## A MODE header, whose value S3 reads case-insensitively
//!
//! The last thing this protocol normalizes is in a HEADER rather than in a document. S3 reads
//! the value of `x-amz-checksum-mode` as the NAME OF A MODE, not as a case-sensitive
//! identifier: the AWS CLI's own `s3 cp` download path sends it lowercase and AWS serves the
//! read, while the member's target enum lists the one value `ENABLED`, so matching the value
//! verbatim refuses `InvalidArgument` for a request every such client sends.
//! [`MODE_HEADER_VALUES`] is the one place that rule lives, and [`mode_headers_folded`] folds
//! an incoming value onto the model's spelling — only its CASE, so a mode the model does not
//! list is still refused, and only for an operation whose own input binds that header.
//!
//! ## An output that binds NOTHING to the body: a response with no document
//!
//! The framework's generic serializer always opens the output shape's element, so an
//! operation whose every output member is bound to a response HEADER still answers an empty
//! envelope — `<PutObjectOutput></PutObjectOutput>` for a write whose whole result is its
//! `ETag` and `x-amz-version-id`. S3 sends no body there at all, and a client that reads the
//! response DOCUMENT for the write's result finds a document carrying none of it.
//! [`without_unbound_body`] is the one place that rule lives, keyed on the output SCHEMA so
//! every such operation answers the same way; an output with a payload member or a member
//! written into the envelope keeps the document that IS its response.
//!
//! ## A BODY that is not XML: the media type it is described by
//!
//! One response of this service carries a document in another language. `GetBucketPolicy`'s
//! payload is "the bucket policy as a JSON document" (`GetBucketPolicyOutput$Policy`), and a
//! payload targeting a plain string gets Smithy's default `text/plain`, so a client or a
//! proxy that dispatches on `content-type` will not parse the body it was handed.
//! [`JSON_PAYLOAD_OUTPUT_OPERATIONS`] is the one place that fact lives, and
//! [`with_json_payload_content_type`] is what applies it; nothing about the body changes.
//!
//! ## A HEADER the API reports and the model declares no member for
//!
//! The last thing this module carries is a response header an output shape has nowhere to
//! put. S3 reports the object version every object sub-resource call acted on in
//! `x-amz-version-id`, and this model binds a member for it on the three tagging operations
//! alone — the ACL and Object Lock sub-resources model only `RequestCharged`, so their
//! generated output shapes cannot carry the version. [`ActedOnVersion`] is the one place
//! that binding lives: it wraps a modelled output, writes it unchanged, and reports the
//! version beside it through [`ACTED_ON_VERSION_ID_MEMBER`].

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request, Response, StatusCode};
use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeDeserializer, ShapeSerializer};
use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use aws_smithy_schema_server::constraints::{path, Violation, ViolationKind};
use aws_smithy_schema_server::protocol::{ProtocolError, ServerProtocol};
use aws_smithy_schema_server::xml::XmlShapeDeserializer;
use aws_smithy_schema_server::{xml_escape, HttpBindingDeserializer, RestXmlServerProtocol};
use aws_smithy_types::body::SdkBody;
use aws_smithy_types::{BigDecimal, BigInteger, Blob, DateTime, Document};
use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer};

/// The `xmlns` every S3 response document carries, from the model's own
/// `@xmlNamespace` (`smithy_query(query="service_info")` reports
/// `xmlNamespace.uri`). The framework stamps it on an enveloped response's root; an
/// unwrapped one gets it here, on the root this module writes.
pub const S3_XML_NAMESPACE: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// Whether this service's model declares `restXml`'s `noErrorWrapping`
/// (`smithy_query(query="service_info")` reports `protocol_config.restXml.noErrorWrapping:
/// true`), which is what makes an S3 error document a bare `<Error>` root. The generated
/// protocol is built with the same flag; this protocol replaces it, so it must carry it
/// too or every error would go out in an envelope no S3 client reads.
const NO_ERROR_WRAPPING: bool = true;

/// The operations whose output document carries NO envelope: the single output member's
/// element is the response root.
///
/// Source: the operations this service's model marks
/// `aws.customizations#s3UnwrappedXmlOutput`, re-derived from the model by
/// `tests/all/bucket_location.rs` so a model update that marks another operation cannot
/// quietly keep the envelope.
pub const UNWRAPPED_XML_OUTPUT_OPERATIONS: &[&str] = &["GetBucketLocation"];

/// The operations whose response body is a JSON DOCUMENT, and so is described as one.
///
/// An `@httpPayload` member IS the body, and Smithy's default media type for a payload
/// targeting a plain string is `text/plain` — correct for a payload that is text, wrong for
/// one that is a document in another language, which a client or a proxy dispatching on
/// `content-type` will then not parse. S3 has exactly one such response: the bucket policy,
/// "the bucket policy as a JSON document" (`GetBucketPolicyOutput$Policy`, the operation's
/// only output member and its payload), which AWS serves as
/// [`JSON_PAYLOAD_CONTENT_TYPE`]. The model carries no `@mediaType` for it
/// (`smithy_query(query="shape",shape="Policy")` reports a bare string), so the fact lives
/// here, keyed on the operation, beside the other thing the generic serializer cannot know.
pub const JSON_PAYLOAD_OUTPUT_OPERATIONS: &[&str] = &["GetBucketPolicy"];

/// The `content-type` a response whose body is a JSON document carries — see
/// [`JSON_PAYLOAD_OUTPUT_OPERATIONS`].
pub const JSON_PAYLOAD_CONTENT_TYPE: &str = "application/json";

/// Whether a name is a bucket this emulator holds — the one question an ADDRESS cannot
/// answer from its own shape (see [`super::addressed_in_host`]).
///
/// A closure rather than an engine, so this file names no engine type and holds no store of
/// its own: `super`'s constructor captures the one shared engine it was handed.
pub type BucketLookup = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// This service's wire protocol: `restXml` exactly as the model declares it, with an
/// unwrapped output document for the operations the model marks as carrying one.
///
/// Everything but those outputs is the framework's own restXml protocol, delegated to
/// unchanged — request deserialization and the bare `<Error>` envelope included.
pub struct UnwrappedXmlRestXml {
    inner: RestXmlServerProtocol,
    /// Whether a name is a bucket this emulator holds, for the one question an ADDRESS
    /// cannot answer from its own shape: whether a request's first HOST label is the bucket
    /// (virtual-hosted style) or part of the endpoint's name (path style). See
    /// [`super::addressed_in_host`] for the rule and [`Self::with_buckets`] for where the
    /// answer comes from.
    holds_bucket: Option<BucketLookup>,
}

/// Hand-written because the bucket lookup is a closure, which carries no `Debug` of its own;
/// the protocol is otherwise its inner one plus that one function.
impl ::std::fmt::Debug for UnwrappedXmlRestXml {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("UnwrappedXmlRestXml")
            .field("inner", &self.inner)
            .field("holds_bucket", &self.holds_bucket.is_some())
            .finish()
    }
}

impl UnwrappedXmlRestXml {
    /// Build the protocol once, at service construction: `override_protocol` borrows it
    /// from `&self` and must never build one per request.
    ///
    /// The route table is EMPTY by design. A protocol's routes are read by
    /// [`ServerProtocol::routing_key`] alone, and the router resolves a request's
    /// operation through the generated `ServiceDefinition`'s own protocol (whose table is
    /// the model's); an author-overridden protocol drives only the modeled sequence —
    /// `deserialize_input`, `serialize_output`, `serialize_error` — none of which consults
    /// routes.
    pub fn new() -> Self {
        Self {
            inner: RestXmlServerProtocol::new(&[])
                .with_namespace(S3_XML_NAMESPACE)
                .with_no_error_wrapping(NO_ERROR_WRAPPING),
            holds_bucket: None,
        }
    }

    /// The same protocol, able to ask whether a name is a bucket this emulator holds.
    ///
    /// Taken as a [`BucketLookup`] rather than as an engine, so this file names no engine
    /// type and holds no store of its own. A protocol built WITHOUT it reads every request
    /// as path-style, which is what a protocol constructed outside a service (a unit test of
    /// the codec) has.
    pub fn with_buckets(mut self, holds_bucket: BucketLookup) -> Self {
        self.holds_bucket = Some(holds_bucket);
        self
    }

    /// The bucket a virtual-hosted-style `request` addresses in its host, when it is one —
    /// see [`super::addressed_in_host`].
    fn addressed_in_host(&self, request: &Request) -> Option<String> {
        let holds_bucket = self.holds_bucket.as_ref()?;
        let host = super::request_host_of(request)?;
        super::addressed_in_host(host, request_path(request.uri()), &**holds_bucket)
            .map(str::to_string)
    }
}

impl Default for UnwrappedXmlRestXml {
    fn default() -> Self {
        Self::new()
    }
}

/// The XML element an output shape occupies: its `@xmlName` when the model renames it
/// (`GetBucketLocationOutput` is `LocationConstraint`), else the shape's own name — the
/// same resolution the framework's XML codec performs.
fn root_element_name(schema: &Schema) -> &str {
    match schema.xml_name() {
        Some(name) => name.value(),
        None => schema.shape_id().shape_name(),
    }
}

/// `x-amz-version-id`, as an object sub-resource response reports the version it acted on.
///
/// S3 answers an object sub-resource call — the tag set, the ACL, the Object Lock legal hold
/// or retention of one object version — with the id of the version it read or wrote in this
/// header, and this model binds an output member for it on only three of them
/// (`smithy_query(query="operation",operation="PutObjectAcl",
/// model_path="services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json")`
/// reports an output of `RequestCharged` alone, and `GetObjectAcl`, `PutObjectLegalHold` and
/// `PutObjectRetention` the same). The operations whose output shape has nowhere to carry it
/// bind it HERE instead, through [`ActedOnVersion`]: one member declaration, carrying the
/// member name and the header every one of them reports the version under.
///
/// A member schema is all the HTTP binder needs — it routes a member carrying `@httpHeader`
/// into the response headers and never into the body — so each of those operations keeps its
/// modelled output schema, and therefore the body and the status it already answered with.
/// The member targets `ObjectVersionId`, the shape every modelled `VersionId` member of this
/// service targets.
pub static ACTED_ON_VERSION_ID_MEMBER: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.s3#ObjectVersionId",
        "com.amazonaws.s3",
        "ObjectVersionId",
    ),
    ShapeType::String,
    "VersionId",
    0,
)
.with_http_header("x-amz-version-id");

/// A modelled output, answered with the id of the object version the call acted on.
///
/// The wrapped `output` writes its own members exactly as it does unwrapped — body document,
/// bound headers and status alike — and the version id goes out in
/// `x-amz-version-id` beside them ([`ACTED_ON_VERSION_ID_MEMBER`]).
///
/// `version_id` is the id the store holds for the version the request addressed, and `None`
/// for a version that has none: a bucket whose versioning was never enabled, and the null
/// version of a suspended one. S3 reports no version id for those, so the header is then
/// absent rather than carrying an id nothing minted.
#[derive(Debug, Clone)]
pub struct ActedOnVersion<O> {
    /// The operation's modelled output, serialized first and unchanged.
    pub output: O,
    /// The id of the object version this call read or wrote, when that version has one.
    pub version_id: Option<String>,
}

impl<O: SerializableStruct> SerializableStruct for ActedOnVersion<O> {
    fn serialize_members(&self, serializer: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        self.output.serialize_members(serializer)?;
        if let Some(version_id) = self.version_id.as_deref() {
            serializer.write_string(&ACTED_ON_VERSION_ID_MEMBER, version_id)?;
        }
        Ok(())
    }
}

/// The ONE string an unwrapped output's single member carries, or `None` for an output
/// that wrote no string at all (an absent member — `us-east-1`'s null location
/// constraint) or wrote anything else.
///
/// An unwrapped output document is a root element holding text, so the only member shape
/// it can express is one string. Every other write is recorded as "not a shape this
/// document can carry" and makes the caller fall back to the enveloped form rather than
/// emit a document that drops a value.
#[derive(Default)]
struct SingleString {
    value: Option<String>,
    unsupported: bool,
}

impl SingleString {
    /// Note a write this document cannot carry.
    fn refuse(&mut self) -> Result<(), SerdeError> {
        self.unsupported = true;
        Ok(())
    }
}

impl ShapeSerializer for SingleString {
    fn write_string(&mut self, _schema: &Schema, value: &str) -> Result<(), SerdeError> {
        if self.value.is_some() {
            // A second member: more than a root's text, so not an unwrapped document.
            return self.refuse();
        }
        self.value = Some(value.to_string());
        Ok(())
    }

    fn write_struct(
        &mut self,
        _schema: &Schema,
        _value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_list(
        &mut self,
        _schema: &Schema,
        _write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_map(
        &mut self,
        _schema: &Schema,
        _write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_boolean(&mut self, _schema: &Schema, _value: bool) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_byte(&mut self, _schema: &Schema, _value: i8) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_short(&mut self, _schema: &Schema, _value: i16) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_integer(&mut self, _schema: &Schema, _value: i32) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_long(&mut self, _schema: &Schema, _value: i64) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_float(&mut self, _schema: &Schema, _value: f32) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_double(&mut self, _schema: &Schema, _value: f64) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_big_integer(
        &mut self,
        _schema: &Schema,
        _value: &BigInteger,
    ) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_big_decimal(
        &mut self,
        _schema: &Schema,
        _value: &BigDecimal,
    ) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_blob(&mut self, _schema: &Schema, _value: &[u8]) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_timestamp(&mut self, _schema: &Schema, _value: &DateTime) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_document(&mut self, _schema: &Schema, _value: &Document) -> Result<(), SerdeError> {
        self.refuse()
    }

    fn write_null(&mut self, _schema: &Schema) -> Result<(), SerdeError> {
        // An absent member writes nothing on the wire either way.
        Ok(())
    }
}

/// The unwrapped document for `output`, or `None` when its members are not one string —
/// in which case the caller serializes it in the framework's enveloped form rather than
/// inventing a document for it.
///
/// An absent member (`us-east-1`'s null location constraint) is the empty root, which is
/// exactly what AWS sends and what every SDK reads back as "no constraint".
fn unwrapped_document(
    output: &dyn SerializableStruct,
    output_schema: &Schema,
) -> Result<Option<String>, SerdeError> {
    let mut members = SingleString::default();
    output.serialize_members(&mut members)?;
    if members.unsupported {
        return Ok(None);
    }
    let root = root_element_name(output_schema);
    Ok(Some(match members.value {
        Some(value) => format!(
            "<{root} xmlns=\"{S3_XML_NAMESPACE}\">{}</{root}>",
            xml_escape(&value)
        ),
        None => format!("<{root} xmlns=\"{S3_XML_NAMESPACE}\"/>"),
    }))
}

/// One member this service's model marks `@xmlAttribute`: how the API spells it, which
/// element the framework's codec binds it from, and what its prefix stands for.
///
/// `Grantee$Type` is the member this exists for; see the module docs for the
/// `smithy_query` that reports its three traits. The `element` is the member's own Smithy
/// name, which is what the codec's struct reader matches a child element's LOCAL name
/// against — the `@xmlName` (`xsi:type`) would reach it as the local name `type`, which
/// matches nothing.
#[derive(Debug, Clone, Copy)]
pub struct XmlAttributeMember {
    /// The attribute the API publishes, prefix included (`xsi:type`).
    pub attribute: &'static str,
    /// The element the framework's XML codec binds the member from (`Type`).
    pub element: &'static str,
    /// The namespace the attribute's prefix stands for, declared on the element the
    /// attribute is written back onto so the document stays namespace-well-formed.
    pub namespace: &'static str,
}

/// Every member of this service's model that travels as an XML ATTRIBUTE rather than as a
/// child element.
///
/// One row today — the ACL grantee's `Type` — and the two normalizations below are written
/// over the TABLE rather than over that one row, so a model update that marks another
/// member needs a row here and nothing else.
pub const XML_ATTRIBUTE_MEMBERS: &[XmlAttributeMember] = &[XmlAttributeMember {
    attribute: "xsi:type",
    element: "Type",
    namespace: "http://www.w3.org/2001/XMLSchema-instance",
}];

/// The member spelled by the attribute named `attribute`, if any.
fn member_by_attribute(attribute: &[u8]) -> Option<&'static XmlAttributeMember> {
    XML_ATTRIBUTE_MEMBERS
        .iter()
        .find(|member| member.attribute.as_bytes() == attribute)
}

/// The member the codec writes as the element named `element`, if any.
///
/// The WRITE path names the element by the member's `@xmlName` — the attribute spelling —
/// so that is what an outgoing document carries and what is matched here.
fn member_by_element(element: &[u8]) -> Option<&'static XmlAttributeMember> {
    XML_ATTRIBUTE_MEMBERS
        .iter()
        .find(|member| member.attribute.as_bytes() == element)
}

/// Whether `haystack` holds `needle` anywhere.
///
/// A byte scan rather than a decode: the documents this is asked about are ACL payloads,
/// but the question is asked before anything parses them.
fn holds(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.len() >= needle.len() && haystack.windows(needle.len()).any(|slice| slice == needle)
}

/// Whether the input shape binds a member as the request's XML DOCUMENT: an `@httpPayload`
/// member whose target is a structured shape (a structure, union, list or map).
///
/// What tells a request whose BODY IS A DOCUMENT from one whose body is arbitrary bytes (an
/// object's payload) or has no body at all. Only such a document can carry a member that
/// travels as an XML attribute, and only such a document may be scanned for one: an
/// object's bytes are not XML and must never be rewritten.
fn is_xml_document_payload(member: &Schema) -> bool {
    member.http_payload().is_some()
        && matches!(
            member.shape_type(),
            ShapeType::Structure
                | ShapeType::Union
                | ShapeType::List
                | ShapeType::Map
                | ShapeType::Document
        )
}

/// Whether the input shape's body is an XML document, i.e. whether this request can carry a
/// member that travels as an XML attribute.
///
/// The gate on the request side. A `Grantee` is reached by MORE than the two ACL writes —
/// `BucketLoggingStatus` reaches one through `LoggingEnabled$TargetGrants`
/// (`smithy_query(query="shape",shape="TargetGrant")`) — so the gate is the SHAPE OF THE
/// BODY rather than the name of one payload member: every operation whose body is a
/// document is normalized when that document actually spells an attribute, and every
/// operation whose body is not is left to the framework's own path without even being
/// scanned.
fn carries_xml_document_payload(input_schema: &Schema) -> bool {
    input_schema
        .members()
        .iter()
        .any(|member| is_xml_document_payload(member))
}

/// The violation to refuse `request` with when its body is an XML DOCUMENT carrying a
/// character the XML character set does not allow, or `None` when it is not or does not.
///
/// XML 1.0 defines which characters a document may contain (see
/// [`crate::state::first_forbidden_xml_char`]); a body carrying any other one is not
/// well-formed XML, whatever it otherwise says, so no codec can read it and no handler can
/// act on it. Refusing it as a violation of the member the document is BOUND to is what
/// makes it carry S3's own `MalformedXML` (`crate::service::s3_refusal` chooses the code
/// from the member), rather than reaching the body codec and surfacing as whatever that
/// codec, or the response serializer echoing the character back, happens to produce.
///
/// Scoped to a STRUCTURED `@httpPayload` member, so the bytes of an object's body — which
/// are not a document and may legitimately hold any byte at all — are never scanned.
fn xml_charset_violation(request: &Request, input_schema: &Schema) -> Option<Violation> {
    let payload = input_schema
        .members()
        .iter()
        .find(|member| is_xml_document_payload(member))?;
    let body = std::str::from_utf8(request.body().bytes()?).ok()?;
    let forbidden = crate::state::first_forbidden_xml_char(body)?;
    Some(Violation {
        path: path::member(payload.member_name()?),
        kind: ViolationKind::Pattern(XML_CHARACTER_SET.to_string()),
        value: Some(format!("{forbidden:?}")),
        sensitive: false,
    })
}

/// The XML 1.0 `Char` production, as the refusal above names the rule a body broke (W3C
/// XML 1.0, §2.2 "Characters").
///
/// `pub(crate)`: it is also what tells `crate::service::violation_is_in_request_document`
/// that a violation naming the payload member ALONE is about the DOCUMENT rather than
/// about a scalar binding of a member of the same name.
pub(crate) const XML_CHARACTER_SET: &str =
    "#x9 | #xA | #xD | [#x20-#xD7FF] | [#xE000-#xFFFD] | [#x10000-#x10FFFF]";

/// `document` with every `@xmlAttribute` member moved out of its element's start tag and
/// into a child element of that element, or `None` when it carries none (in which case
/// nothing is rewritten and the original bytes are used).
///
/// The namespace declarations are left exactly where they were: an unused `xmlns:xsi` is
/// legal, and removing it would change a document the client chose.
fn attributes_as_elements(document: &[u8]) -> Option<Vec<u8>> {
    let mut reader = Reader::from_reader(document);
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new(Vec::new());
    let mut moved = false;
    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Start(start)) => {
                let (clean, children) = split_attribute_members(&start);
                moved |= !children.is_empty();
                writer.write_event(Event::Start(clean)).ok()?;
                for (element, value) in children {
                    write_text_element(&mut writer, element, &value)?;
                }
            }
            Ok(Event::Empty(empty)) => {
                let (clean, children) = split_attribute_members(&empty);
                if children.is_empty() {
                    writer.write_event(Event::Empty(clean)).ok()?;
                    continue;
                }
                // An element that held its members in attributes alone is self-closing;
                // it has to open and close now that they are children of it.
                moved = true;
                let name = String::from_utf8_lossy(clean.name().as_ref()).into_owned();
                writer.write_event(Event::Start(clean)).ok()?;
                for (element, value) in children {
                    write_text_element(&mut writer, element, &value)?;
                }
                writer.write_event(Event::End(BytesEnd::new(name))).ok()?;
            }
            Ok(other) => {
                writer.write_event(other).ok()?;
            }
            // A document the XML reader cannot walk is left for the framework's own
            // parser to refuse, so the refusal a client sees is the framework's.
            Err(_) => return None,
        }
    }
    moved.then(|| writer.into_inner())
}

/// `start` without the attributes that are really members, beside the elements those
/// members become.
///
/// Both halves carry the attribute's bytes VERBATIM — the kept attributes because
/// `push_attribute` copies an `Attribute`'s raw value unchanged, and a moved one because
/// it is written as pre-escaped character data. XML spells an entity reference the same
/// way in an attribute value and in character data, so the value the codec finally reads
/// is the one the client sent, decoded exactly once.
fn split_attribute_members(
    start: &BytesStart<'_>,
) -> (BytesStart<'static>, Vec<(&'static str, Vec<u8>)>) {
    let name = String::from_utf8_lossy(start.name().as_ref()).into_owned();
    let mut clean = BytesStart::new(name).into_owned();
    let mut members: Vec<(&'static str, Vec<u8>)> = Vec::new();
    for attribute in start.attributes().flatten() {
        match member_by_attribute(attribute.key.as_ref()) {
            Some(member) => members.push((member.element, attribute.value.to_vec())),
            None => clean.push_attribute(attribute),
        }
    }
    (clean, members)
}

/// Write `<name>text</name>`, with `text` taken as already-escaped character data.
fn write_text_element(writer: &mut Writer<Vec<u8>>, name: &str, text: &[u8]) -> Option<()> {
    let escaped = String::from_utf8_lossy(text).into_owned();
    writer.write_event(Event::Start(BytesStart::new(name))).ok()?;
    writer
        .write_event(Event::Text(BytesText::from_escaped(escaped)))
        .ok()?;
    writer.write_event(Event::End(BytesEnd::new(name))).ok()?;
    Some(())
}

/// One UNION member whose element the API publishes under a name the framework's XML codec
/// does not match it by.
///
/// The codec reads a union's set member from the child element whose local name is the
/// member's own SMITHY name; a union member carrying an `@xmlName` therefore arrives under
/// a name that matches nothing, and the union deserializes as "no member set". The
/// published name is what every S3 client sends, so an incoming document is normalized onto
/// the name the codec binds — the same normalization [`XML_ATTRIBUTE_MEMBERS`] does for a
/// member that travels as an attribute, and for the same reason.
#[derive(Debug, Clone, Copy)]
pub struct XmlNamedUnionMember {
    /// The element the API publishes (`ObjectEncryption$SSEKMS`'s `@xmlName`, `SSE-KMS`).
    pub published: &'static str,
    /// The member's own Smithy name, which is the element the codec binds it from
    /// (`SSEKMS`).
    pub member: &'static str,
}

/// Every union member of this service's model whose `@xmlName` differs from its member
/// name.
///
/// One row today — `ObjectEncryption$SSEKMS`
/// (`smithy_query(query="shape",shape="ObjectEncryption")` reports
/// `SSEKMS: {xmlName: "SSE-KMS"}`) — and the normalization below is written over the TABLE
/// rather than over that row. `tests/all/object_encryption_update.rs` re-derives the set
/// from the model, so a model update that names another one cannot quietly go unmatched.
pub const XML_NAMED_UNION_MEMBERS: &[XmlNamedUnionMember] = &[XmlNamedUnionMember {
    published: "SSE-KMS",
    member: "SSEKMS",
}];

/// The union member the API publishes under the element named `element`, if any.
fn union_member_by_published_name(element: &[u8]) -> Option<&'static XmlNamedUnionMember> {
    XML_NAMED_UNION_MEMBERS
        .iter()
        .find(|member| member.published.as_bytes() == element)
}

/// `document` with every published union-member element renamed to the member name the
/// codec binds, or `None` when it carries none.
///
/// Only the element NAME changes: its attributes, its children and their text are written
/// back exactly as they arrived.
fn published_names_as_members(document: &[u8]) -> Option<Vec<u8>> {
    let mut reader = Reader::from_reader(document);
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new(Vec::new());
    let mut renamed = false;
    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Start(start)) => match union_member_by_published_name(start.name().as_ref()) {
                Some(member) => {
                    renamed = true;
                    writer
                        .write_event(Event::Start(renamed_start(&start, member.member)))
                        .ok()?;
                }
                None => {
                    writer.write_event(Event::Start(start)).ok()?;
                }
            },
            Ok(Event::Empty(empty)) => match union_member_by_published_name(empty.name().as_ref()) {
                Some(member) => {
                    renamed = true;
                    writer
                        .write_event(Event::Empty(renamed_start(&empty, member.member)))
                        .ok()?;
                }
                None => {
                    writer.write_event(Event::Empty(empty)).ok()?;
                }
            },
            Ok(Event::End(end)) => match union_member_by_published_name(end.name().as_ref()) {
                Some(member) => {
                    renamed = true;
                    writer
                        .write_event(Event::End(BytesEnd::new(member.member)))
                        .ok()?;
                }
                None => {
                    writer.write_event(Event::End(end)).ok()?;
                }
            },
            Ok(other) => {
                writer.write_event(other).ok()?;
            }
            // A document the XML reader cannot walk is left for the framework's own
            // parser to refuse, so the refusal a client sees is the framework's.
            Err(_) => return None,
        }
    }
    renamed.then(|| writer.into_inner())
}

/// `start` under the element name `name`, carrying its attributes verbatim —
/// `push_attribute` copies an `Attribute`'s raw value unchanged, so nothing is re-escaped.
fn renamed_start(start: &BytesStart<'_>, name: &'static str) -> BytesStart<'static> {
    let mut renamed = BytesStart::new(name).into_owned();
    for attribute in start.attributes().flatten() {
        renamed.push_attribute(attribute);
    }
    renamed
}

/// `document` with every `@xmlAttribute` member moved out of its own element and onto its
/// parent's start tag, with the namespace its prefix needs declared there, or `None` when
/// it carries none.
fn elements_as_attributes(document: &[u8]) -> Option<Vec<u8>> {
    let mut reader = Reader::from_reader(document);
    reader.config_mut().trim_text(false);

    // The document's events, with the attribute members' own elements left out, and for
    // each event that opened an element the members that belong on its start tag.
    //
    // Keyed BY THAT EVENT's index rather than held as one flat list, so the write pass
    // below is one lookup per start tag: a document carrying many promoted members (an
    // ACL's grants, a logging status's target grants) costs time linear in its events
    // rather than in events times members.
    let mut events: Vec<Event<'static>> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    let mut promoted: BTreeMap<usize, Vec<(&'static XmlAttributeMember, String)>> =
        BTreeMap::new();
    let mut reading: Option<(usize, &'static XmlAttributeMember, String)> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Start(start)) => {
                match (
                    open.last().copied(),
                    member_by_element(start.name().as_ref()),
                ) {
                    (Some(parent), Some(member)) => {
                        reading = Some((parent, member, String::new()));
                    }
                    _ => {
                        open.push(events.len());
                        events.push(Event::Start(start.into_owned()));
                    }
                }
            }
            Ok(Event::Empty(empty)) => {
                match (
                    open.last().copied(),
                    member_by_element(empty.name().as_ref()),
                ) {
                    // An empty element is the empty value of its member.
                    (Some(parent), Some(member)) => promoted
                        .entry(parent)
                        .or_default()
                        .push((member, String::new())),
                    _ => events.push(Event::Empty(empty.into_owned())),
                }
            }
            Ok(Event::Text(text)) => match reading.as_mut() {
                Some((_, _, value)) => {
                    value.push_str(&text.xml10_content().unwrap_or_default());
                }
                None => events.push(Event::Text(text.into_owned())),
            },
            Ok(Event::End(end)) => match reading.take() {
                Some((parent, member, value)) => {
                    promoted.entry(parent).or_default().push((member, value));
                }
                None => {
                    open.pop();
                    events.push(Event::End(end.into_owned()));
                }
            },
            Ok(other) => events.push(other.into_owned()),
            // A document this reader cannot walk is answered as the framework wrote it,
            // rather than being dropped.
            Err(_) => return None,
        }
    }
    if promoted.is_empty() {
        return None;
    }

    let mut writer = Writer::new(Vec::new());
    for (index, event) in events.into_iter().enumerate() {
        match event {
            Event::Start(mut start) => {
                for (member, value) in promoted.get(&index).into_iter().flatten() {
                    if let Some((prefix, _)) = member.attribute.split_once(':') {
                        let declaration = format!("xmlns:{prefix}");
                        start.push_attribute((declaration.as_str(), member.namespace));
                    }
                    start.push_attribute((member.attribute, value.as_str()));
                }
                writer.write_event(Event::Start(start)).ok()?;
            }
            other => writer.write_event(other).ok()?,
        }
    }
    Some(writer.into_inner())
}

/// The path component of a request URI, without the scheme, the authority or the query.
///
/// `pub(super)`, with [`request_query`] below it: [`super`]'s routing seam reads the same
/// two components off a request URI, and one reader for both keeps the request this
/// service ROUTES and the request it DESERIALIZES from disagreeing about which path and
/// which query string arrived.
pub(super) fn request_path(uri: &str) -> &str {
    let after_authority = match uri.find("://") {
        None => uri,
        Some(scheme_end) => match uri[scheme_end + 3..].find('/') {
            None => "/",
            Some(path_start) => &uri[scheme_end + 3 + path_start..],
        },
    };
    after_authority
        .split('?')
        .next()
        .unwrap_or(after_authority)
}

/// The query component of a request URI (everything after the first `?`), or the empty
/// string when it has none.
pub(super) fn request_query(uri: &str) -> &str {
    match uri.find('?') {
        Some(mark) => &uri[mark + 1..],
        None => "",
    }
}

/// The request headers whose VALUE S3 reads as a MODE NAME rather than as a case-sensitive
/// identifier, each with the spelling the model gives that mode.
///
/// `x-amz-checksum-mode` is the one such header this service binds: it asks for the checksum
/// the object was stored with ("to retrieve the checksum, this parameter must be enabled",
/// `ChecksumMode`'s own documentation) and its target enum has exactly ONE permitted value, so
/// a case-insensitive match cannot be ambiguous. S3 serves the header however a client spells
/// it — the AWS CLI's own `s3 cp` download path sends `enabled` and AWS answers 200 with the
/// object's checksum, where refusing it answers `InvalidArgument` for a request AWS accepts.
/// No other value is accepted: a mode the model does not list is still refused, because only
/// the CASE is folded here.
///
/// Source: the permitted spellings are the GENERATED value set the model's own enum produced
/// ([`crate::ops::types::CHECKSUM_MODE_VALUES`], `["ENABLED"]`), so a value the enum gains
/// cannot drift from this fold; the header name is the `@httpHeader` the three operations that
/// bind the member carry (`GetObject`, `HeadObject`, `GetObjectAnnotation`), and the fold below
/// is applied only where the operation's own input binds it.
const MODE_HEADER_VALUES: &[(&str, &[&str])] = &[(
    "x-amz-checksum-mode",
    crate::ops::types::CHECKSUM_MODE_VALUES,
)];

/// `request`'s headers with every [`MODE_HEADER_VALUES`] value the request spells in another
/// case folded to the model's spelling — or `None` when there is nothing to fold, which is
/// every request that spells them as the model does and every operation whose input binds no
/// such header.
///
/// Driven off `input_schema`: the fold applies to a header the operation being served really
/// binds, so a header of that name on an operation that binds no such member is left exactly
/// as it arrived.
fn mode_headers_folded(request: &Request, input_schema: &Schema) -> Option<Vec<(String, String)>> {
    let bound = |name: &str| {
        input_schema.members().iter().any(|member| {
            member
                .http_header()
                .is_some_and(|header| header.value().eq_ignore_ascii_case(name))
        })
    };
    let folded = |name: &str, value: &str| {
        MODE_HEADER_VALUES
            .iter()
            .filter(|(header, _)| header.eq_ignore_ascii_case(name))
            .flat_map(|(_, modelled)| modelled.iter())
            .find(|modelled| **modelled != value && modelled.eq_ignore_ascii_case(value))
            .copied()
    };
    let mut headers = request_headers(request);
    let mut changed = false;
    for (name, value) in &mut headers {
        if !bound(name) {
            continue;
        }
        if let Some(modelled) = folded(name, value) {
            *value = modelled.to_string();
            changed = true;
        }
    }
    changed.then_some(headers)
}

/// A request's headers as `(name, value)` pairs, original case preserved and every value
/// of a repeated header kept, which is what the binding deserializer needs to bind a
/// list-typed or prefixed header.
fn request_headers(request: &Request) -> Vec<(String, String)> {
    let headers = request.headers();
    let names: BTreeSet<&str> = headers.iter().map(|(name, _)| name).collect();
    let mut pairs: Vec<(String, String)> = Vec::new();
    for name in names {
        for value in headers.get_all(name) {
            pairs.push((name.to_string(), value.to_string()));
        }
    }
    pairs
}

/// A body deserializer that OWNS its XML document, so a NORMALIZED body can be read
/// without borrowing from the request the original arrived in.
///
/// The framework's own body deserializer borrows the request's bytes, which a rewritten
/// document is not; this holds the rewritten document and builds the framework's reader
/// over it per read. Each read starts at the document's root, which is exactly how the
/// binding deserializer uses a body: once, for the payload member.
struct OwnedXmlBody {
    document: Vec<u8>,
}

/// Read one scalar from the owned document through the framework's own XML reader.
macro_rules! owned_xml_scalar {
    ($name:ident -> $value:ty) => {
        fn $name(&mut self, schema: &Schema) -> Result<$value, SerdeError> {
            XmlShapeDeserializer::new(&self.document).$name(schema)
        }
    };
}

impl ShapeDeserializer for OwnedXmlBody {
    fn read_struct(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(&Schema, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        XmlShapeDeserializer::new(&self.document).read_struct(schema, consumer)
    }

    fn read_list(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(&mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        XmlShapeDeserializer::new(&self.document).read_list(schema, consumer)
    }

    fn read_map(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(String, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        XmlShapeDeserializer::new(&self.document).read_map(schema, consumer)
    }

    owned_xml_scalar!(read_boolean -> bool);
    owned_xml_scalar!(read_byte -> i8);
    owned_xml_scalar!(read_short -> i16);
    owned_xml_scalar!(read_integer -> i32);
    owned_xml_scalar!(read_long -> i64);
    owned_xml_scalar!(read_float -> f32);
    owned_xml_scalar!(read_double -> f64);
    owned_xml_scalar!(read_big_integer -> BigInteger);
    owned_xml_scalar!(read_big_decimal -> BigDecimal);
    owned_xml_scalar!(read_string -> String);
    owned_xml_scalar!(read_blob -> Blob);
    owned_xml_scalar!(read_timestamp -> DateTime);
    owned_xml_scalar!(read_document -> Document);

    fn is_null(&self) -> bool {
        XmlShapeDeserializer::new(&self.document).is_null()
    }

    fn container_size(&self) -> Option<usize> {
        XmlShapeDeserializer::new(&self.document).container_size()
    }
}

/// `response` with the status HTTP gives a PARTIAL answer, when it is one.
///
/// "The 206 (Partial Content) status code indicates that the server is successfully
/// fulfilling a range request for the target resource by transferring one or more parts of
/// the selected representation" and such a response "generates a `Content-Range` header
/// field describing what range of the selected representation is enclosed" (RFC 9110, HTTP
/// Semantics, §15.3.7). So the `Content-Range` a handler bound IS the signal: a successful
/// response that reports one carries part of the object rather than the whole of it, and
/// 200 would tell a client it had received everything.
///
/// The rule lives here, once, because an operation's success status is a single modelled
/// constant (`Operation::SUCCESS_STATUS`) and this service's model binds the member rather
/// than the status: two operations answer a `Content-Range` (`GetObject` and `HeadObject`),
/// and neither handler can state a second status of its own. Only a 200 is promoted — an
/// operation whose modelled success status is anything else is answering something this rule
/// is not about.
fn with_partial_content_status(response: Response) -> Response {
    if response.status().as_u16() != 200 || response.headers().get("content-range").is_none() {
        return response;
    }
    let Ok(partial) = StatusCode::try_from(PARTIAL_CONTENT_STATUS) else {
        return response;
    };
    let headers: Vec<(String, String)> = response
        .headers()
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    let mut promoted = Response::new(partial, response.into_body());
    for (name, value) in headers {
        // `append`, not `insert`: a header name the serializer wrote twice keeps both of its
        // values. The headers came off a response the serializer built, so every one of them
        // is already representable; a value the HTTP layer refuses is dropped rather than
        // turning a served range into a transport failure.
        let _ = promoted.headers_mut().try_append(name, value);
    }
    promoted
}

/// The status a response carrying part of the selected representation is sent with — see
/// [`with_partial_content_status`].
const PARTIAL_CONTENT_STATUS: u16 = 206;

/// The response headers this model binds to a TIMESTAMP whose `timestampFormat` is
/// `date-time`, which is the format each of them has to go out in.
///
/// `smithy_query(query="shape",shape="ObjectLockRetainUntilDate",
/// model_path="services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json")`
/// reports `timestampFormat: date-time` on the shape `GetObject`'s and `HeadObject`'s
/// `x-amz-object-lock-retain-until-date` member targets, and S3 answers that header as an
/// ISO8601 date-time accordingly. Every other timestamp header of this service's responses
/// (`Last-Modified`, `x-amz-abort-date`) targets a shape carrying no such trait, and so goes
/// out in HTTP's own date format.
const DATE_TIME_TIMESTAMP_HEADERS: [&str; 1] = ["x-amz-object-lock-retain-until-date"];

/// `response` with every header of [`DATE_TIME_TIMESTAMP_HEADERS`] re-rendered in the
/// `date-time` format the model binds it with.
///
/// The rule lives here, once, for the same reason [`with_partial_content_status`] does: the
/// framework's header serializer renders a bound timestamp in HTTP's date format unless the
/// MEMBER itself carries the format trait, and in this model the trait is on the member's
/// TARGET shape — so no handler and no shape declaration can state it, and a header left in
/// the other format is one no client reads as the date the API publishes. Only a header the
/// response actually carries is touched, and a value that does not read back as a date is
/// left exactly as it is rather than dropped.
fn with_date_time_timestamp_headers(mut response: Response) -> Response {
    for name in DATE_TIME_TIMESTAMP_HEADERS {
        let Some(rendered) = response.headers().get(name).map(str::to_string) else {
            continue;
        };
        let Ok(instant) =
            DateTime::from_str(&rendered, aws_smithy_types::date_time::Format::HttpDate)
        else {
            continue;
        };
        let Ok(published) = instant.fmt(aws_smithy_types::date_time::Format::DateTime) else {
            continue;
        };
        let _ = response.headers_mut().try_insert(name, published);
    }
    response
}

/// The header naming the request a response answers: "`x-amz-request-id`: A value created
/// by Amazon S3 that uniquely identifies the request" (Amazon S3 API Reference, *Common
/// Response Headers*), which every S3 response carries — it is what a client's
/// `ResponseMetadata.RequestId` reports and what an event notification names the call it
/// came of by.
const REQUEST_ID_HEADER: &str = "x-amz-request-id";

/// The second correlation header of that same list: "`x-amz-id-2`: A special token that
/// helps Amazon Web Services troubleshoot problems", reported as
/// `ResponseMetadata.HostId`.
const EXTENDED_REQUEST_ID_HEADER: &str = "x-amz-id-2";
/// The pair a SUCCESS response carries: the one the operation that served the request
/// recorded ([`crate::state::request_ids`]), so an event notification and the response of
/// the call that produced it name the SAME request, or a freshly minted one when nothing
/// recorded any (every operation that publishes no event).
///
/// TAKEN before anything that can fail, so a serializer that errors on its way to a
/// response cannot leave a pair behind for a LATER response on this thread to pick up. An
/// ERROR response's pair is the envelope's own — see `serialize_error`.
fn answering_request() -> crate::state::request_ids::RequestCorrelation {
    crate::state::request_ids::answering()
        .unwrap_or_else(crate::state::request_ids::RequestCorrelation::mint)
}

/// `response` with the two request-correlation headers every S3 response carries.
fn with_request_correlation(
    mut response: Response,
    correlation: crate::state::request_ids::RequestCorrelation,
) -> Response {
    // A value a header map refuses is dropped rather than turning a served response into a
    // transport failure; both are hex, so neither ever is.
    let _ = response
        .headers_mut()
        .try_insert(REQUEST_ID_HEADER, correlation.request_id);
    let _ = response
        .headers_mut()
        .try_insert(EXTENDED_REQUEST_ID_HEADER, correlation.extended_request_id);
    response
}

/// `response` with every `@xmlAttribute` member written where the API publishes it, or
/// `response` unchanged when its body carries none.
///
/// A `content-length` the serializer set is corrected rather than left stale: the
/// normalized document is a different length (an element becomes an attribute and a
/// namespace declaration appears), and a header that disagrees with the body is worse
/// than none.
fn with_attribute_members(mut response: Response) -> Response {
    let Some(body) = response.body().bytes() else {
        return response;
    };
    if !XML_ATTRIBUTE_MEMBERS
        .iter()
        .any(|member| holds(body, format!("<{}", member.attribute).as_bytes()))
    {
        return response;
    }
    let Some(document) = elements_as_attributes(body) else {
        return response;
    };
    let length = document.len();
    *response.body_mut() = SdkBody::from(document);
    if response.headers().get("content-length").is_some() {
        response
            .headers_mut()
            .insert("content-length", length.to_string());
    }
    response
}

::std::thread_local! {
    /// The XML DOCUMENT the request this thread is serving carried, for the length of that
    /// one request — see [`spelled_in_request`].
    static REQUEST_DOCUMENT: ::std::cell::RefCell<Option<Vec<u8>>> =
        const { ::std::cell::RefCell::new(None) };
}

/// Record `document` as the XML document the request this thread is serving carried, `None`
/// for a request whose body is not one.
///
/// Called for EVERY request this service reads, under the same discipline as
/// [`crate::state::addressed_bucket`]: a `None` overwrites whatever the previous request on
/// this thread recorded, so no handler can read another request's document.
fn remember_request_document(document: Option<&[u8]>) {
    let recorded = document.map(<[u8]>::to_vec);
    REQUEST_DOCUMENT.with(|note| {
        *note.borrow_mut() = recorded;
    });
}

/// The TEXT the request's own XML document spelled `element` in — the first element of that
/// local name — or `None` for a request that carried no document or no such element.
///
/// What an `<ArgumentValue>` echoes for a value refused inside a request document: S3's
/// `InvalidArgument` names the field it rejected and the value that field CARRIED, and the
/// parsed value has lost its spelling (a timestamp parses to an instant, which can be
/// rendered several ways and is no longer the text the caller sent). A client matching the
/// envelope against what it sent reads the text, so the text is what goes out.
///
/// Read rather than taken: the handler that refuses the value runs in the same stretch of
/// the one dispatch that deserialized the request (`vendor/smithy-server-runtime`'s request
/// walk), and the note is replaced by the next request this thread reads.
pub fn spelled_in_request(element: &str) -> Option<String> {
    REQUEST_DOCUMENT.with(|note| {
        let document = note.borrow();
        element_text(document.as_deref()?, element)
    })
}

/// The text of the first element of `document` whose name is `element`, read through the
/// same XML reader this module normalizes documents with.
fn element_text(document: &[u8], element: &str) -> Option<String> {
    let mut reader = Reader::from_reader(document);
    let mut inside = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) => {
                inside = start.name().as_ref() == element.as_bytes();
            }
            Ok(Event::Text(text)) if inside => {
                return text.xml10_content().ok().map(|text| text.into_owned())
            }
            Ok(Event::End(_)) => inside = false,
            Ok(Event::Eof) | Err(_) => return None,
            _ => {}
        }
    }
}

/// The OBJECT a request addresses, read off its PATH-STYLE path and its query string: the/// `{Key+}` label below the bucket segment, decoded the way the codec decodes a path label,
/// and the `versionId` parameter when the request carries one. `None` for a path that
/// addresses a bucket or the service root, which names no object at all.
///
/// What the `<Key>` / `<VersionId>` of an object-scoped error document is written from (see
/// [`crate::scaffolded`]), so one request's address is read once, here, rather than per
/// handler. The decode and the query read are the ones [`super::sigv4`] already owns, so a
/// request's address is read one way wherever it is read.
fn addressed_object(
    path: &str,
    query: &str,
) -> Option<crate::state::addressed_bucket::AddressedObject> {
    let (_, key) = path.trim_start_matches('/').split_once('/')?;
    if key.is_empty() {
        return None;
    }
    Some(crate::state::addressed_bucket::AddressedObject {
        key: super::sigv4::percent_decoded(key),
        version_id: super::sigv4::query_parameter(query, VERSION_ID_PARAMETER),
    })
}

/// The query parameter a request names one stored version of an object by — the
/// `@httpQuery` every operation that takes a version binds it from
/// (`smithy_query(query="operation",operation="GetObject")` reports
/// `VersionId: {httpQuery: "versionId"}`).
const VERSION_ID_PARAMETER: &str = "versionId";

/// The query parameter a request names the multipart upload it addresses by — the
/// `@httpQuery` every operation that takes one binds it from
/// (`smithy_query(query="operation",operation="CompleteMultipartUpload")` reports
/// `UploadId: {httpQuery: "uploadId"}`).
const UPLOAD_ID_PARAMETER: &str = "uploadId";

/// `response` describing its body as the JSON document it is — see
/// [`JSON_PAYLOAD_OUTPUT_OPERATIONS`]. Only the header changes; the body the handler
/// answered with is untouched.
fn with_json_payload_content_type(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert("content-type", JSON_PAYLOAD_CONTENT_TYPE);
    response
}

/// `response` with no body at all when the operation's output binds NOTHING to one.///
/// An output every member of which goes out in a response HEADER (or as the response
/// status) leaves no document to send, and S3 sends none: `PutObject` answers 200 with an
/// empty payload and its result — the stored object's `ETag`, the `x-amz-version-id` of
/// the version it created — in those headers (Amazon S3 API Reference, *PutObject* sample
/// response, whose body is `Content-Length: 0`). The framework's generic restXml
/// serializer always opens the output shape's element, so unaided such an operation
/// answers an empty envelope (`<PutObjectOutput></PutObjectOutput>`): a document the API
/// does not send, and one in which a client that reads the response DOCUMENT for the
/// write's result finds none of it.
///
/// The response's own HEADERS are left exactly as the serializer wrote them, this
/// service's `content-type` included: a response that carries no body still describes what
/// a `GET` of the same resource would have answered with, which is what makes a HEAD's
/// headers worth reading at all.
///
/// Keyed on the output SCHEMA rather than on the operation, so every operation whose
/// result is entirely header-bound answers the same way and no new one has to be added
/// here. An output with a payload member or any member written into the envelope is
/// untouched — its body IS the response.
fn without_unbound_body(mut response: Response, output_schema: &Schema) -> Response {
    let binds_nothing = output_schema.members().iter().all(|member| {
        member.http_header().is_some()
            || member.http_prefix_headers().is_some()
            || member.http_response_code().is_some()
    });
    if !binds_nothing {
        return response;
    }
    *response.body_mut() = SdkBody::empty();
    response
}

impl ServerProtocol for UnwrappedXmlRestXml {
    type Request = Request;
    type Response = Response;

    fn protocol_id(&self) -> &ShapeId {
        self.inner.protocol_id()
    }

    fn routing_key<'a>(&self, request: &'a Self::Request) -> Result<&'a str, ProtocolError> {
        // Routing is the generated entry's protocol's job (see `new`); this answers
        // through the same code path, which with no routes reports the request as naming
        // no operation rather than guessing one.
        self.inner.routing_key(request)
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a Self::Request,
        input_schema: &Schema,
    ) -> Result<Box<dyn ShapeDeserializer + 'a>, ProtocolError> {
        // Whatever this worker thread may still hold from an earlier response is dropped
        // before this request's handler can record its own pair, so no response can ever
        // be named by another request's id (see [`crate::state::request_ids`]).
        crate::state::request_ids::forget_any_unanswered();
        // The BUCKET this request addresses, recorded for the one thing that needs it once
        // the handler has returned: the `BucketName` a bucket-scoped error document names
        // the bucket by when the refusal did not name one itself
        // ([`crate::state::addressed_bucket`]). Recorded for EVERY request this service
        // reads, before anything here can refuse one, so the slot a response reads is
        // always its own request's — a request that addresses no bucket (`GET /`) records
        // nothing rather than leaving the previous one's note behind.
        //
        // It is the bucket of the request's address in either form: the first label of the
        // host for a virtual-hosted-style request ([`super::addressed_in_host`]), else the
        // first segment of the path, which is the `{Bucket}` label every bucket-scoped
        // route binds. A name LONGER than a bucket name may be
        // ([`super::BUCKET_NAME_CHARS`]) names no bucket at all and is not recorded: the
        // element is a bucket's name, not an echo of arbitrary request text.
        let addressed_bucket = self
            .addressed_in_host(request)
            .or_else(|| {
                super::first_path_segment(request_path(request.uri())).map(str::to_string)
            })
            .filter(|name| name.chars().count() <= *super::BUCKET_NAME_CHARS.end());
        crate::state::addressed_bucket::remember(addressed_bucket.as_deref());
        // And the OBJECT it addresses, for the `<Key>` and `<VersionId>` an object-scoped
        // error document names it by — recorded under the same discipline, from the request's
        // address in PATH-STYLE form, so a virtual-hosted-style request (whose path is the
        // key alone) records the same key a path-style one does. A request that addresses a
        // bucket or the service root records nothing.
        let path_style = match self.addressed_in_host(request) {
            Some(bucket) => super::path_with_bucket(&bucket, request_path(request.uri())),
            None => request_path(request.uri()).to_string(),
        };
        crate::state::addressed_bucket::remember_object(addressed_object(
            &path_style,
            request_query(request.uri()),
        ));
        // And the multipart UPLOAD it addresses, for the `<UploadId>` an upload-scoped error
        // document names the upload that was not there by — the `uploadId` query parameter
        // every operation that addresses an upload binds that member from
        // (`smithy_query(query="operation",operation="CompleteMultipartUpload")` reports
        // `UploadId: {httpQuery: "uploadId"}`, as do `UploadPart`, `UploadPartCopy`,
        // `ListParts` and `AbortMultipartUpload`). Recorded under the same discipline: a
        // request that addresses no upload records nothing.
        crate::state::addressed_bucket::remember_upload(
            super::sigv4::query_parameter(request_query(request.uri()), UPLOAD_ID_PARAMETER)
                .as_deref(),
        );
        // And the DOCUMENT it carried, for a refusal that has to echo a value AS THE REQUEST
        // SPELLED IT once the codec has parsed it ([`spelled_in_request`]). Only a request
        // whose body IS an XML document carries one; every other request records nothing,
        // which is what clears whatever the previous request on this thread recorded.
        let document = match carries_xml_document_payload(input_schema) {
            true => request.body().bytes(),
            false => None,
        };
        remember_request_document(document);
        // A request whose body IS an XML document is refused here when that document
        // carries a character XML cannot hold, before any codec reads it. See
        // [`xml_charset_violation`].
        if let Some(violation) = xml_charset_violation(request, input_schema) {
            return Err(ProtocolError::Invalid(vec![violation]));
        }
        // The PATH the `@httpLabel` members are bound from. A VIRTUAL-HOSTED-STYLE request
        // names its bucket in the host and leaves its path as the key alone, so the bucket
        // is put back in front of it — the same normalization the routing seam resolved the
        // operation from (`super::Service::override_operation`), so the operation that was
        // matched binds the bucket that was addressed. `None` for a path-style request,
        // whose path already IS the path the labels bind from.
        let host_style_path = self
            .addressed_in_host(request)
            .map(|bucket| super::path_with_bucket(&bucket, request_path(request.uri())));
        // The HEADERS the bound members are read from, with a MODE header the request spells
        // in another case folded to the model's spelling ([`mode_headers_folded`]). `None`
        // leaves the headers exactly as they arrived, which is every request that needs no
        // fold.
        let folded_headers = mode_headers_folded(request, input_schema);
        // Only a request whose BODY IS AN XML DOCUMENT can carry a member the framework's
        // codec cannot bind as the document spells it, and only a body that actually
        // spells one needs normalizing; everything else — every request whose body is not
        // a document, and every document whose members are already spelled the way the
        // codec binds them — is the framework's own path, byte for byte.
        if carries_xml_document_payload(input_schema) {
            let body = request
                .body()
                .bytes()
                .ok_or_else(|| ProtocolError::BodyRead("body is not available as bytes".into()))?;
            // Two normalizations, each applied only to a document that calls for it, and
            // the second over the result of the first so a document needing both is served.
            let mut normalized: Option<Vec<u8>> = None;
            if XML_ATTRIBUTE_MEMBERS
                .iter()
                .any(|member| holds(body, format!("{}=", member.attribute).as_bytes()))
            {
                normalized = attributes_as_elements(body);
            }
            let current: &[u8] = normalized.as_deref().unwrap_or(body);
            if XML_NAMED_UNION_MEMBERS
                .iter()
                .any(|member| holds(current, format!("<{}", member.published).as_bytes()))
            {
                if let Some(document) = published_names_as_members(current) {
                    normalized = Some(document);
                }
            }
            if let Some(document) = normalized {
                let uri = request.uri();
                let binding = HttpBindingDeserializer::new(
                    input_schema.http().map(|http| http.uri()),
                    host_style_path.as_deref().unwrap_or(request_path(uri)),
                    request_query(uri),
                    folded_headers.unwrap_or_else(|| request_headers(request)),
                    OwnedXmlBody {
                        document: document.clone(),
                    },
                )
                .with_payload_bytes(document);
                return Ok(Box::new(binding));
            }
        }
        if host_style_path.is_some() || folded_headers.is_some() {
            // The framework's own deserializer reads the path and the headers off the request,
            // which for a virtual-hosted-style request carries no bucket in its path and for a
            // request spelling a mode in another case carries a value the model's enum does not
            // list — so this one is built over the normalized path and headers instead.
            // Everything else is the framework's: the same binding deserializer, over the same
            // query and body.
            let path = match host_style_path {
                Some(path) => path,
                None => request_path(request.uri()).to_string(),
            };
            let body = request
                .body()
                .bytes()
                .ok_or_else(|| ProtocolError::BodyRead("body is not available as bytes".into()))?;
            // The body has to be IN MEMORY to be read here, which is the same condition the
            // framework's own restXml deserializer puts on EVERY request it reads, so this
            // branch asks no more of a request than the path it stands in for: a body not
            // available as bytes is refused there too, whatever made this branch the one taken.
            let uri = request.uri();
            let binding = HttpBindingDeserializer::new(
                input_schema.http().map(|http| http.uri()),
                &path,
                request_query(uri),
                folded_headers.unwrap_or_else(|| request_headers(request)),
                OwnedXmlBody {
                    document: body.to_vec(),
                },
            );
            // An `@httpPayload` member IS the body, so the raw bytes are wired for it (the
            // binding deserializer picks them for a scalar payload and the codec above for
            // a structured one). An input with no payload member never reads them.
            let binding = match input_schema
                .members()
                .iter()
                .any(|member| member.http_payload().is_some())
            {
                true => binding.with_payload_bytes(body.to_vec()),
                false => binding,
            };
            return Ok(Box::new(binding));
        }
        self.inner.deserialize_input(request, input_schema)
    }

    fn serialize_output(
        &self,
        output: &dyn SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<Self::Response, ProtocolError> {
        // Taken FIRST, before anything that can fail: see [`answering_request`].
        let correlation = answering_request();
        if UNWRAPPED_XML_OUTPUT_OPERATIONS.contains(&operation_name) {
            if let Some(body) =
                unwrapped_document(output, output_schema).map_err(ProtocolError::Serde)?
            {
                let status = StatusCode::try_from(success_status).map_err(|_| {
                    ProtocolError::Malformed(format!(
                        "{operation_name} declares the unusable success status {success_status}"
                    ))
                })?;
                let mut response = Response::new(status, SdkBody::from(body));
                response
                    .headers_mut()
                    .insert("content-type", "application/xml");
                return Ok(with_request_correlation(response, correlation));
            }
        }
        let response =
            self.inner
                .serialize_output(output, output_schema, operation_name, success_status)?;
        // A response whose body is a JSON document says so, whatever media type the generic
        // serializer derived for its payload member.
        let response = match JSON_PAYLOAD_OUTPUT_OPERATIONS.contains(&operation_name) {
            true => with_json_payload_content_type(response),
            false => response,
        };
        // A member the model marks `@xmlAttribute` has been written as an element; it
        // belongs on its parent's start tag, where the API publishes it and where a
        // namespace-aware parser can read it. And a response that reports a
        // `Content-Range` carries PART of the object, which HTTP answers with 206. A
        // timestamp header the model binds in the `date-time` format is re-rendered in it.
        // An output that binds nothing to the body answers with no body at all.
        Ok(with_request_correlation(
            without_unbound_body(
                with_date_time_timestamp_headers(with_partial_content_status(
                    with_attribute_members(response),
                )),
                output_schema,
            ),
            correlation,
        ))
    }

    fn serialize_error(
        &self,
        error: &dyn SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<Self::Response, ProtocolError> {
        // The error envelope's correlation pair is NOT this service's to choose. This
        // model sets `noErrorWrapping` ([`NO_ERROR_WRAPPING`]), so the inner serializer
        // writes ONE pair into both the `<RequestId>` / `<HostId>` elements of the body and
        // the matching response headers — and the two have to agree, because a client reads
        // the id from the body and an operator reads it from the header. A refusal
        // publishes no event notification and so records no pair of its own, which is why
        // there is nothing here to stamp over it with: the ONE value an error response
        // names its request by is the one the envelope itself carries.
        self.inner
            .serialize_error(error, error_schema, error_code, http_status)
    }

    fn serialize_event_stream_message(
        &self,
        event_type: &str,
        is_exception: bool,
        event: &dyn SerializableStruct,
        event_schema: &Schema,
    ) -> Result<Self::Response, ProtocolError> {
        self.inner
            .serialize_event_stream_message(event_type, is_exception, event, event_schema)
    }
}
