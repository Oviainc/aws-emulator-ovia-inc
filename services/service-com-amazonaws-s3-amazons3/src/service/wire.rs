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

use std::collections::{BTreeMap, BTreeSet};

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

/// This service's wire protocol: `restXml` exactly as the model declares it, with an
/// unwrapped output document for the operations the model marks as carrying one.
///
/// Everything but those outputs is the framework's own restXml protocol, delegated to
/// unchanged — request deserialization and the bare `<Error>` envelope included.
#[derive(Debug)]
pub struct UnwrappedXmlRestXml {
    inner: RestXmlServerProtocol,
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
        }
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
        // A request whose body IS an XML document is refused here when that document
        // carries a character XML cannot hold, before any codec reads it. See
        // [`xml_charset_violation`].
        if let Some(violation) = xml_charset_violation(request, input_schema) {
            return Err(ProtocolError::Invalid(vec![violation]));
        }
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
                    request_path(uri),
                    request_query(uri),
                    request_headers(request),
                    OwnedXmlBody {
                        document: document.clone(),
                    },
                )
                .with_payload_bytes(document);
                return Ok(Box::new(binding));
            }
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
                return Ok(response);
            }
        }
        let response =
            self.inner
                .serialize_output(output, output_schema, operation_name, success_status)?;
        // A member the model marks `@xmlAttribute` has been written as an element; it
        // belongs on its parent's start tag, where the API publishes it and where a
        // namespace-aware parser can read it.
        Ok(with_attribute_members(response))
    }

    fn serialize_error(
        &self,
        error: &dyn SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<Self::Response, ProtocolError> {
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
