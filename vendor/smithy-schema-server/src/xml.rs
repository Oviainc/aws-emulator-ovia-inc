// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! [`Codec`] implementation for XML, built on `quick-xml`.

use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{
    SerdeError, SerializableStruct, ShapeDeserializer, ShapeSerializer,
};
use aws_smithy_schema::{Schema, ShapeId, Trait};
use aws_smithy_types::{base64, BigDecimal, BigInteger, Blob, DateTime, Document};
use quick_xml::escape::unescape;
use quick_xml::events::{BytesCData, BytesEnd, BytesRef, BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer};
use std::any::Any;
use std::io::Cursor;
use std::str::FromStr;

/// The whitespace XML treats as layout: space, tab, carriage return, line feed.
///
/// NOT `str::trim`'s set, which also strips NBSP, U+2028 and the ideographic space. Those are
/// characters a document states, not indentation a formatter added, so removing one changes the
/// value a client sent.
const XML_WHITESPACE: [char; 4] = [' ', '\t', '\r', '\n'];

/// A value assembled from a run of text and reference events, remembering which kind each
/// fragment came from.
///
/// Layout whitespace can only arrive in a TEXT fragment: a formatter indents with literal
/// bytes, never with `&#32;`. So the trim is applied to the EDGE fragments and only when they
/// are text-sourced. The two simpler rules are both wrong —
///
/// * trimming the ASSEMBLED value also strips a reference a client deliberately encoded at the
///   edge, so `<Status>&#32;x&#32;</Status>` loses both spaces; and
/// * trimming EVERY fragment eats the value's own interior spaces, so `a &amp; b` collapses
///   to `a&b`.
///
/// Whitespace the document states is content; whitespace a formatter added is not, and only the
/// fragment's origin distinguishes them.
#[derive(Default)]
struct AssembledText {
    /// `(came from an Event::Text, decoded content)`, in document order.
    fragments: Vec<(bool, String)>,
}

impl AssembledText {
    fn push_text(&mut self, decoded: String) {
        self.fragments.push((true, decoded));
    }

    /// Content the document STATED rather than indented with: a resolved `&name;` / `&#NN;`,
    /// or a CDATA section. Never trimmed — a formatter produces neither.
    fn push_stated(&mut self, content: String) {
        self.fragments.push((false, content));
    }

    /// The value, with layout whitespace removed from text-sourced edges only.
    fn finish(mut self) -> String {
        if let Some(first) = self.fragments.first_mut() {
            if first.0 {
                first.1 = first.1.trim_start_matches(XML_WHITESPACE).to_owned();
            }
        }
        if let Some(last) = self.fragments.last_mut() {
            if last.0 {
                last.1 = last.1.trim_end_matches(XML_WHITESPACE).to_owned();
            }
        }
        self.fragments.into_iter().map(|(_, s)| s).collect()
    }
}

/// Resolve a `&name;` / `&#NN;` reference event to the text it stands for.
///
/// The reader no longer folds entity references into the surrounding text event: it BREAKS the
/// text at each reference and emits the reference separately, so a read loop that only handles
/// `Event::Text` silently drops every entity — `a &amp; b` deserializes as `ab`. Resolution goes
/// through `escape::unescape` on the reconstructed reference so predefined entities and numeric
/// character references share one implementation, and an undefined entity is still an error
/// rather than a silent empty string.
fn resolved_reference(reference: &BytesRef<'_>) -> Result<String, SerdeError> {
    let name = reference
        .decode()
        .map_err(|e| SerdeError::custom(format!("xml decode error: {e}")))?;
    let reconstructed = format!("&{name};");
    let resolved = unescape(&reconstructed)
        .map_err(|e| SerdeError::custom(format!("xml unescape error: {e}")))?;
    Ok(resolved.into_owned())
}

/// A CDATA section's content, verbatim.
///
/// NOT unescaped: CDATA's whole purpose is to carry bytes the parser must not interpret, so
/// `<![CDATA[a &amp; b]]>` is the nine characters `a &amp; b` and not `a & b`.
fn cdata_text(event: &BytesCData<'_>) -> Result<String, SerdeError> {
    std::str::from_utf8(event.as_ref())
        .map(str::to_owned)
        .map_err(|e| SerdeError::custom(format!("xml cdata is not valid utf-8: {e}")))
}

/// Decode a text event's bytes and resolve its entity references.
///
/// One step used to do both. `BytesText::xml10_content` decodes the bytes with the reader's
/// encoding and normalizes EOLs per XML 1.0; `escape::unescape` resolves `&amp;`, `&lt;` and
/// friends. BOTH halves are required: decoding alone leaves entity references verbatim in the
/// value, which is a wire-visible difference in every XML protocol this codec serves, and
/// unescaping without decoding cannot handle a non-UTF-8 document at all.
fn decoded_text(event: &BytesText<'_>) -> Result<String, SerdeError> {
    let decoded = event
        .xml10_content()
        .map_err(|e| SerdeError::custom(format!("xml decode error: {e}")))?;
    let unescaped =
        unescape(&decoded).map_err(|e| SerdeError::custom(format!("xml unescape error: {e}")))?;
    Ok(unescaped.into_owned())
}

/// True when `c` is permitted by the XML 1.0 `Char` production:
///
/// ```text
/// Char ::= #x9 | #xA | #xD | [#x20-#xD7FF] | [#xE000-#xFFFD] | [#x10000-#x10FFFF]
/// ```
///
/// Everything else is illegal in a document and has NO escaped form — not even a
/// numeric character reference — so a serializer's only parseable option is to
/// drop it. Surrogates (`#xD800`-`#xDFFF`) cannot exist in a Rust `char`, which
/// leaves two easily-missed exclusions that a `< 0x20` test does not catch:
/// **`U+FFFE` and `U+FFFF`**. Both are valid `char`s and both make a document
/// unparseable.
fn is_xml_char(c: char) -> bool {
    matches!(c,
        '\u{09}' | '\u{0A}' | '\u{0D}'
        | '\u{20}'..='\u{D7FF}'
        | '\u{E000}'..='\u{FFFD}'
        | '\u{10000}'..='\u{10FFFF}'
    )
}

/// Whether `name` is a legal XML 1.0 element name (the `Name` production).
///
/// A tag name is interpolated into element markup verbatim, not as character
/// data, so escaping does not make an arbitrary string usable as one: `&` has
/// no legal place in a `Name`, and `<a&lt;b>` is still malformed even though it
/// round-trips through a lenient parser. The only safe handling of a name taken
/// from request input is to accept it when it is a legal `Name` and reject it
/// otherwise, so both the read and write paths that reuse a parsed element name
/// gate on this one predicate.
///
/// ```text
/// NameStartChar ::= ":" | [A-Z] | "_" | [a-z] | [#xC0-#xD6] | [#xD8-#xF6]
///                 | [#xF8-#x2FF] | [#x370-#x37D] | [#x37F-#x1FFF]
///                 | [#x200C-#x200D] | [#x2070-#x218F] | [#x2C00-#x2FEF]
///                 | [#x3001-#xD7FF] | [#xF900-#xFDCF] | [#xFDF0-#xFFFD]
///                 | [#x10000-#xEFFFF]
/// NameChar      ::= NameStartChar | "-" | "." | [0-9] | #xB7
///                 | [#x0300-#x036F] | [#x203F-#x2040]
/// Name          ::= NameStartChar (NameChar)*
/// ```
fn is_xml_name(name: &str) -> bool {
    fn is_name_start(c: char) -> bool {
        matches!(c,
            ':' | 'A'..='Z' | '_' | 'a'..='z'
            | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}'
            | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}'
            | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}'
            | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}'
            | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}'
            | '\u{10000}'..='\u{EFFFF}'
        )
    }
    fn is_name_char(c: char) -> bool {
        is_name_start(c)
            || matches!(c,
                '-' | '.' | '0'..='9' | '\u{B7}'
                | '\u{0300}'..='\u{036F}' | '\u{203F}'..='\u{2040}'
            )
    }
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if is_name_start(first) => chars.all(is_name_char),
        _ => false,
    }
}

/// Escape a string for XML character data or an attribute value.
///
/// The five predefined entities, plus one thing entity-escaping cannot express:
/// XML 1.0 forbids some code points OUTRIGHT — most C0 controls, and `U+FFFE` /
/// `U+FFFF`. They are illegal in a document even written as `&#x1;`, so there is
/// no escaped form, so they are REPLACED with U+FFFD (legal XML) rather than
/// dropped — dropping would make the rejected value in an error message look
/// legal. See
/// [`is_xml_char`]. Tab, newline and carriage return ARE legal, but are emitted as
/// numeric references so they survive attribute-value normalization and line-end
/// folding unchanged.
///
/// This matters because the text reaching this envelope is the text reaching the
/// JSON one: an error message renders a member's `@pattern` verbatim, and a
/// caller-supplied value can carry a control character. Escaping only the five
/// entities left restXml and the query family emitting XML that no SDK can
/// parse - the same failure fixed for JSON, where `escape_json` can at least
/// render `\u00XX`.
pub fn xml_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for c in input.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // LEGAL but not round-trip safe as raw bytes, so emitted as numeric
            // references. XML 1.0 normalizes a literal #x9/#xA/#xD in an ATTRIBUTE
            // value to a space (spec 3.3.3) and folds a literal #xD in element text
            // to #xA (spec 2.11), so a raw tab in `xmlns` reaches the client as a
            // space and a message containing CR decodes as LF. A numeric reference
            // survives both — and it is available precisely because these three are
            // legal; a reference to an ILLEGAL code point (`&#x7;`) is rejected by a
            // conforming parser, which is why the arm below must replace instead.
            //
            // Without this, `escape_json` and `xml_escape` disagree about the same
            // violation text: JSON is lossless for \t / \n / \r and XML was not.
            '\t' => out.push_str("&#x9;"),
            '\n' => out.push_str("&#xA;"),
            '\r' => out.push_str("&#xD;"),
            // Anything the XML 1.0 `Char` production excludes has NO escaped form,
            // so it cannot be represented — it is REPLACED with U+FFFD rather than
            // deleted.
            //
            // Deleting would be equally well-formed, but it makes the error message
            // LIE. The same violation travels both
            // envelopes, and `escape_json` preserves a control character as
            // `\u0007`; if this one deleted it, a restJson1 client would be told
            // `Value 'a\u0007b' ... failed` while a restXml client was told
            // `Value 'ab' ... failed` — a value that looks perfectly legal, so the
            // reader concludes the service is wrong rather than their input. U+FFFD
            // is the Unicode replacement character, is legal XML, and keeps the
            // rejected value visibly corrupted instead of silently plausible.
            c if !is_xml_char(c) => out.push('\u{FFFD}'),
            _ => out.push(c),
        }
    }
    out
}

/// Codegen-attached XML layout for a list member: whether the list is
/// `@xmlFlattened`, and the element name each item gets when it is not.
///
/// `aws-smithy-schema` records `@xmlFlattened` but exposes no getter for it (the
/// same gap [`crate::query`] works around by probing both wire layouts), and a
/// list member needs *two* names — its own element name and its items' element
/// name — where a `Schema` has only one `xml_name` slot. Codegen therefore
/// attaches this trait to the list member's [`TraitMap`](aws_smithy_schema::TraitMap): `xml_name` keeps its
/// usual meaning (this member's own element name, from the structure member's
/// `@xmlName`) and the item name lives here.
///
/// The distinction is not cosmetic. `restXml` renders the two layouts
/// differently and SDK parsers only understand the modeled one:
///
/// ```text
/// wrapped    <Contents><Object>…</Object><Object>…</Object></Contents>
/// flattened  <Contents>…</Contents><Contents>…</Contents>
/// ```
///
/// S3's `ListObjectsV2Output.Contents` is `@xmlFlattened`, so emitting the
/// wrapped form makes botocore find one `<Contents>` element holding no members
/// it recognises and yield `[{}]` — the keys are on the wire but unreachable.
///
/// Per the Smithy XML binding, a flattened list's repeated element name comes
/// from the *structure* member (its `@xmlName` taking precedence over the list
/// member's), which is exactly what `xml_name` already carries.
///
/// This is a framework-internal trait, not a Smithy trait; its shape id lives in
/// a private `smithy.rust.synthetic` namespace so it can never collide with a real
/// Smithy trait id in the same map.
#[derive(Debug, Clone, Copy)]
pub struct XmlListTrait {
    flattened: bool,
    item_name: Option<&'static str>,
}

impl XmlListTrait {
    /// Synthetic shape id for the wrapper (not a real Smithy trait).
    pub const TRAIT_ID: ShapeId =
        ShapeId::from_static("smithy.rust.synthetic#xmlList", "smithy.rust.synthetic", "xmlList");

    pub const fn new(flattened: bool, item_name: Option<&'static str>) -> Self {
        Self {
            flattened,
            item_name,
        }
    }

    /// Whether the list is `@xmlFlattened` (no wrapper element).
    pub const fn flattened(&self) -> bool {
        self.flattened
    }

    /// The `@xmlName` on the list's member, used as the item element name in the
    /// wrapped layout.
    pub const fn item_name(&self) -> Option<&'static str> {
        self.item_name
    }
}

impl Trait for XmlListTrait {
    fn trait_id(&self) -> &ShapeId {
        &Self::TRAIT_ID
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The XML list layout codegen attached to this member, if any.
fn xml_list_trait(schema: &Schema) -> Option<&XmlListTrait> {
    schema
        .traits()?
        .get(&XmlListTrait::TRAIT_ID)?
        .as_any()
        .downcast_ref::<XmlListTrait>()
}

/// The `@xmlFlattened` flag codegen attaches to a map member, which
/// `aws-smithy-schema` records but exposes no getter for.
///
/// `restXml` renders a map two ways and the writer must know which the model
/// chose. The key and value element names already come from `Schema::key()` and
/// `Schema::member()` (honouring their `@xmlName`), so unlike a list this trait
/// carries only the flattened flag:
///
/// ```text
/// wrapped    <Attrs><entry><key>k</key><value>v</value></entry>...</Attrs>
/// flattened  <Attrs><key>k</key><value>v</value></Attrs><Attrs>...</Attrs>
/// ```
///
/// A flattened map drops both the outer wrapper and the `<entry>` wrapper: each
/// pair is one repeated element named by the map member (its `@xmlName`, else
/// the member name), which `Schema::xml_name()` already carries. Emitting the
/// wrapped form for a flattened map gives an SDK parser one `<Attrs>` element
/// holding members it cannot reach, the same failure a wrongly-wrapped list
/// produces.
///
/// This is a framework-internal trait, not a Smithy trait; its shape id lives in
/// a private `smithy.rust.synthetic` namespace so it can never collide with a
/// real Smithy trait id in the same map.
#[derive(Debug, Clone, Copy)]
pub struct XmlMapTrait {
    flattened: bool,
}

impl XmlMapTrait {
    /// Synthetic shape id for the wrapper (not a real Smithy trait).
    pub const TRAIT_ID: ShapeId = ShapeId::from_static(
        "smithy.rust.synthetic#xmlMap",
        "smithy.rust.synthetic",
        "xmlMap",
    );

    pub const fn new(flattened: bool) -> Self {
        Self { flattened }
    }

    /// Whether the map is `@xmlFlattened` (no outer wrapper and no `<entry>`).
    pub const fn flattened(&self) -> bool {
        self.flattened
    }
}

impl Trait for XmlMapTrait {
    fn trait_id(&self) -> &ShapeId {
        &Self::TRAIT_ID
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The XML map layout codegen attached to this member, if any.
fn xml_map_trait(schema: &Schema) -> Option<&XmlMapTrait> {
    schema
        .traits()?
        .get(&XmlMapTrait::TRAIT_ID)?
        .as_any()
        .downcast_ref::<XmlMapTrait>()
}

/// Whether this member is a `@xmlFlattened` list, i.e. arrives on the wire as
/// repeated sibling elements rather than one wrapper.
/// Route one struct member's element to the consumer, or buffer it as a flattened-list
/// fragment.
///
/// Shared by the `Event::Start` and `Event::Empty` arms of `read_struct`. They differ ONLY in
/// where `inner_xml` comes from — a started element's content has to be read, a self-closing
/// one has none — and everything after that is identical. Factored so the two cannot drift:
/// the self-closing shape was originally absent, which silently dropped the member.
///
/// Returns whether the element matched a member, so the caller can skip an unknown one.
fn dispatch_struct_member(
    schema: &Schema,
    local: &str,
    inner_xml: &[u8],
    flattened: &mut Vec<(usize, String, Vec<u8>)>,
    child_depth: usize,
    consumer: &mut dyn FnMut(&Schema, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
) -> Result<bool, SerdeError> {
    // By member_name (the Smithy field name) first, then by `@xmlName`.
    let member = schema.members().iter().copied().find(|m| {
        if m.member_name() == Some(local) {
            return true;
        }
        if let Some(xn) = m.xml_name() {
            return xn.value() == local;
        }
        false
    });
    let Some(member) = member else {
        return Ok(false);
    };
    // A `@xmlFlattened` list and a `@xmlFlattened` map both arrive as repeated
    // sibling elements under the struct, so both are buffered the same way: each
    // sibling is re-wrapped into the member's element and appended to one slot,
    // then drained in `read_struct` as a single run. They differ only in how the
    // drained run is parsed (list items vs key/value pairs), which `read_struct`
    // decides from the member schema.
    if is_flattened_list(member) || is_flattened_map(member) {
        let index = member.member_index().unwrap_or(usize::MAX);
        let slot = match flattened.iter_mut().find(|(i, _, _)| *i == index) {
            Some(slot) => slot,
            None => {
                flattened.push((index, local.to_owned(), Vec::new()));
                flattened.last_mut().expect("just pushed")
            }
        };
        append_element(&mut slot.2, local, inner_xml)?;
    } else {
        let mut sub = XmlElementDeserializer::with_depth(local, inner_xml, child_depth);
        consumer(member, &mut sub)?;
    }
    Ok(true)
}

fn is_flattened_list(schema: &Schema) -> bool {
    xml_list_trait(schema).is_some_and(|layout| layout.flattened())
}

/// Whether this member is a `@xmlFlattened` map. See [`XmlMapTrait`] for the
/// wrapped and flattened layouts.
fn is_flattened_map(schema: &Schema) -> bool {
    xml_map_trait(schema).is_some_and(|layout| layout.flattened())
}

/// Append `<name>inner</name>` to `out`.
///
/// Used to re-wrap the repeated siblings of a flattened list into the wrapped
/// layout the list reader already understands. `name` comes from the parsed
/// request and is written into markup verbatim, so it is gated by
/// [`is_xml_name`] and rejected when it is not a legal XML `Name`; the read path
/// that reuses the same parsed name gates on the same predicate. `inner` is
/// bytes the reader already emitted, so it is forwarded unchanged.
fn append_element(out: &mut Vec<u8>, name: &str, inner: &[u8]) -> Result<(), SerdeError> {
    if !is_xml_name(name) {
        return Err(SerdeError::custom(format!(
            "element name `{name}` is not a legal XML name"
        )));
    }
    out.push(b'<');
    out.extend_from_slice(name.as_bytes());
    out.push(b'>');
    out.extend_from_slice(inner);
    out.extend_from_slice(b"</");
    out.extend_from_slice(name.as_bytes());
    out.push(b'>');
    Ok(())
}

/// XML codec for `restXml` protocols (e.g., Route 53).
#[derive(Debug, Clone)]
pub struct XmlCodec {
    namespace: Option<&'static str>,
}

impl XmlCodec {
    pub const fn new() -> Self {
        Self { namespace: None }
    }

    pub const fn with_namespace(mut self, ns: &'static str) -> Self {
        self.namespace = Some(ns);
        self
    }
}

impl Default for XmlCodec {
    fn default() -> Self {
        Self::new()
    }
}

impl Codec for XmlCodec {
    type Serializer = XmlShapeSerializer;
    type Deserializer<'a> = XmlShapeDeserializer<'a>;

    fn create_serializer(&self) -> Self::Serializer {
        XmlShapeSerializer::new(self.namespace)
    }

    fn create_deserializer<'a>(&self, input: &'a [u8]) -> Self::Deserializer<'a> {
        XmlShapeDeserializer::new(input)
    }
}

// Helper: resolve the XML element name for a schema.
fn xml_element_name(schema: &Schema) -> &str {
    if let Some(xn) = schema.xml_name() {
        xn.value()
    } else if let Some(mn) = schema.member_name() {
        mn
    } else {
        schema.shape_id().shape_name()
    }
}

// =================================================================================================
// Serializer
// =================================================================================================

/// XML [`ShapeSerializer`] backed by `quick-xml`.
pub struct XmlShapeSerializer {
    writer: Writer<Cursor<Vec<u8>>>,
    namespace: Option<&'static str>,
    /// Whether we are at the root level (next struct gets xmlns).
    root: bool,
}

impl XmlShapeSerializer {
    pub fn new(namespace: Option<&'static str>) -> Self {
        Self {
            writer: Writer::new(Cursor::new(Vec::new())),
            namespace,
            root: true,
        }
    }

    pub fn write_text_element(&mut self, name: &str, text: &str) -> Result<(), SerdeError> {
        self.writer
            .write_event(Event::Start(BytesStart::new(name)))
            .map_err(|e| SerdeError::custom(format!("xml write error: {e}")))?;
        self.writer
            .write_event(Event::Text(BytesText::new(text)))
            .map_err(|e| SerdeError::custom(format!("xml write error: {e}")))?;
        self.writer
            .write_event(Event::End(BytesEnd::new(name)))
            .map_err(|e| SerdeError::custom(format!("xml write error: {e}")))?;
        Ok(())
    }

    /// Write raw XML bytes directly (for protocol envelopes like awsQuery wrappers).
    pub fn write_raw(&mut self, raw: &str) -> Result<(), SerdeError> {
        self.writer
            .get_mut()
            .get_mut()
            .extend_from_slice(raw.as_bytes());
        Ok(())
    }
}

impl FinishSerializer for XmlShapeSerializer {
    fn finish(self) -> Vec<u8> {
        self.writer.into_inner().into_inner()
    }
}

impl XmlShapeSerializer {
    /// Open an XML element tag (with xmlns if at root). Call [`close_element`]
    /// when done writing children.
    pub fn open_element(&mut self, schema: &Schema) -> Result<(), SerdeError> {
        let name = xml_element_name(schema);
        let mut start = BytesStart::new(name.to_owned());
        if self.root {
            if let Some(ns) = self.namespace {
                start.push_attribute(("xmlns", ns));
            }
            self.root = false;
        }
        self.writer
            .write_event(Event::Start(start))
            .map_err(|e| SerdeError::custom(format!("xml write error: {e}")))
    }

    /// Close a previously opened XML element.
    pub fn close_element(&mut self, schema: &Schema) -> Result<(), SerdeError> {
        let name = xml_element_name(schema);
        self.writer
            .write_event(Event::End(BytesEnd::new(name.to_owned())))
            .map_err(|e| SerdeError::custom(format!("xml write error: {e}")))
    }

    /// Write a struct under an explicitly supplied element name.
    ///
    /// Needed for list items, whose element name is decided by the enclosing
    /// list member (`<Contents>`) rather than by the item's own shape
    /// (`<Object>`); see [`ListItemSerializer`].
    fn write_named_struct(
        &mut self,
        name: &str,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        let mut start = BytesStart::new(name.to_owned());
        if self.root {
            if let Some(ns) = self.namespace {
                start.push_attribute(("xmlns", ns));
            }
            self.root = false;
        }
        self.writer
            .write_event(Event::Start(start))
            .map_err(|e| SerdeError::custom(format!("xml write error: {e}")))?;
        value.serialize_members(self)?;
        self.writer
            .write_event(Event::End(BytesEnd::new(name.to_owned())))
            .map_err(|e| SerdeError::custom(format!("xml write error: {e}")))?;
        Ok(())
    }
}

/// Writes each element of a list under a fixed element name.
///
/// Every scalar becomes `<item_name>text</item_name>`. A struct element is
/// written under `item_name` too — NOT under its own shape name — because the
/// element name of a list item is decided by the list, not by the item's shape:
/// S3's `Contents` is a `list<Object>` whose items are `<Contents>`, never
/// `<Object>`.
struct ListItemSerializer<'a> {
    inner: &'a mut XmlShapeSerializer,
    item_name: String,
}

impl<'a> ShapeSerializer for ListItemSerializer<'a> {
    fn write_struct(
        &mut self,
        _schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        self.inner
            .write_named_struct(&self.item_name.clone(), value)
    }
    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner.write_list(schema, write_elements)
    }
    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.inner.write_map(schema, write_entries)
    }
    fn write_boolean(&mut self, _schema: &Schema, value: bool) -> Result<(), SerdeError> {
        self.inner
            .write_text_element(&self.item_name, &value.to_string())
    }
    fn write_byte(&mut self, _schema: &Schema, value: i8) -> Result<(), SerdeError> {
        self.inner
            .write_text_element(&self.item_name, &value.to_string())
    }
    fn write_short(&mut self, _schema: &Schema, value: i16) -> Result<(), SerdeError> {
        self.inner
            .write_text_element(&self.item_name, &value.to_string())
    }
    fn write_integer(&mut self, _schema: &Schema, value: i32) -> Result<(), SerdeError> {
        self.inner
            .write_text_element(&self.item_name, &value.to_string())
    }
    fn write_long(&mut self, _schema: &Schema, value: i64) -> Result<(), SerdeError> {
        self.inner
            .write_text_element(&self.item_name, &value.to_string())
    }
    fn write_float(&mut self, _schema: &Schema, value: f32) -> Result<(), SerdeError> {
        self.inner
            .write_text_element(&self.item_name, &value.to_string())
    }
    fn write_double(&mut self, _schema: &Schema, value: f64) -> Result<(), SerdeError> {
        self.inner
            .write_text_element(&self.item_name, &value.to_string())
    }
    fn write_big_integer(
        &mut self,
        _schema: &Schema,
        value: &BigInteger,
    ) -> Result<(), SerdeError> {
        self.inner
            .write_text_element(&self.item_name, value.as_ref())
    }
    fn write_big_decimal(
        &mut self,
        _schema: &Schema,
        value: &BigDecimal,
    ) -> Result<(), SerdeError> {
        self.inner
            .write_text_element(&self.item_name, value.as_ref())
    }
    fn write_string(&mut self, _schema: &Schema, value: &str) -> Result<(), SerdeError> {
        self.inner.write_text_element(&self.item_name, value)
    }
    fn write_blob(&mut self, _schema: &Schema, value: &[u8]) -> Result<(), SerdeError> {
        self.inner
            .write_text_element(&self.item_name, &base64::encode(value.as_ref()))
    }
    fn write_timestamp(&mut self, _schema: &Schema, value: &DateTime) -> Result<(), SerdeError> {
        let s = value
            .fmt(aws_smithy_types::date_time::Format::DateTime)
            .map_err(|e| SerdeError::custom(format!("timestamp format error: {e}")))?;
        self.inner.write_text_element(&self.item_name, &s)
    }
    fn write_document(&mut self, _schema: &Schema, _value: &Document) -> Result<(), SerdeError> {
        Err(SerdeError::custom("documents not supported in XML"))
    }
    fn write_null(&mut self, _schema: &Schema) -> Result<(), SerdeError> {
        Ok(())
    }
}

/// Adapter for serializing map entries into the `<key>` / `<value>` pair layout.
/// The element wrapping each pair is parameterized by `entry_name` (`entry` when
/// wrapped, the map member's element when flattened). See
/// [`XmlShapeSerializer::write_map`] for the full map layout rules and element
/// naming.
struct MapEntrySerializer<'a> {
    inner: &'a mut XmlShapeSerializer,
    expecting_key: bool,
    key_name: String,
    value_name: String,
    /// Element that wraps one key/value pair. `entry` in the wrapped layout; the
    /// map member's own element name in the flattened one, where the pair IS the
    /// repeated member element and there is no outer wrapper.
    entry_name: String,
}

impl<'a> ShapeSerializer for MapEntrySerializer<'a> {
    fn write_struct(
        &mut self,
        _schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        if self.expecting_key {
            return Err(SerdeError::custom("map key must be a string"));
        }
        self.inner
            .write_named_struct(&self.value_name.clone(), value)?;
        self.inner
            .writer
            .write_event(Event::End(BytesEnd::new(self.entry_name.clone())))
            .map_err(|e| SerdeError::custom(format!("xml write error: {e}")))?;
        self.expecting_key = true;
        Ok(())
    }
    fn write_list(
        &mut self,
        _schema: &Schema,
        _write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        Err(SerdeError::custom(
            "map value as list not supported in XML maps",
        ))
    }
    fn write_map(
        &mut self,
        _schema: &Schema,
        _write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        Err(SerdeError::custom("nested maps not supported in XML"))
    }
    fn write_boolean(&mut self, _schema: &Schema, value: bool) -> Result<(), SerdeError> {
        self.write_scalar_value(&value.to_string())
    }
    fn write_byte(&mut self, _schema: &Schema, value: i8) -> Result<(), SerdeError> {
        self.write_scalar_value(&value.to_string())
    }
    fn write_short(&mut self, _schema: &Schema, value: i16) -> Result<(), SerdeError> {
        self.write_scalar_value(&value.to_string())
    }
    fn write_integer(&mut self, _schema: &Schema, value: i32) -> Result<(), SerdeError> {
        self.write_scalar_value(&value.to_string())
    }
    fn write_long(&mut self, _schema: &Schema, value: i64) -> Result<(), SerdeError> {
        self.write_scalar_value(&value.to_string())
    }
    fn write_float(&mut self, _schema: &Schema, value: f32) -> Result<(), SerdeError> {
        self.write_scalar_value(&value.to_string())
    }
    fn write_double(&mut self, _schema: &Schema, value: f64) -> Result<(), SerdeError> {
        self.write_scalar_value(&value.to_string())
    }
    fn write_big_integer(
        &mut self,
        _schema: &Schema,
        value: &BigInteger,
    ) -> Result<(), SerdeError> {
        self.write_scalar_value(value.as_ref())
    }
    fn write_big_decimal(
        &mut self,
        _schema: &Schema,
        value: &BigDecimal,
    ) -> Result<(), SerdeError> {
        self.write_scalar_value(value.as_ref())
    }
    fn write_string(&mut self, _schema: &Schema, value: &str) -> Result<(), SerdeError> {
        self.write_scalar_value(value)
    }
    fn write_blob(&mut self, _schema: &Schema, value: &[u8]) -> Result<(), SerdeError> {
        self.write_scalar_value(&base64::encode(value.as_ref()))
    }
    fn write_timestamp(
        &mut self,
        _schema: &Schema,
        value: &DateTime,
    ) -> Result<(), SerdeError> {
        let s = value
            .fmt(aws_smithy_types::date_time::Format::DateTime)
            .map_err(|e| SerdeError::custom(format!("timestamp format error: {e}")))?;
        self.write_scalar_value(&s)
    }
    fn write_document(
        &mut self,
        _schema: &Schema,
        _value: &Document,
    ) -> Result<(), SerdeError> {
        Err(SerdeError::custom("documents not supported in XML"))
    }
    fn write_null(&mut self, _schema: &Schema) -> Result<(), SerdeError> {
        if self.expecting_key {
            return Err(SerdeError::custom("map key must not be null"));
        }
        // Emit empty <value/> and close the entry to keep the state machine balanced.
        // A sparse map with a null value still closes the entry element and toggles
        // expecting_key so the next pair's key writes correctly.
        self.inner.write_text_element(&self.value_name, "")?;
        self.inner
            .writer
            .write_event(Event::End(BytesEnd::new(self.entry_name.clone())))
            .map_err(|e| SerdeError::custom(format!("xml write error: {e}")))?;
        self.expecting_key = true;
        Ok(())
    }
}

impl<'a> MapEntrySerializer<'a> {
    fn write_scalar_value(&mut self, text: &str) -> Result<(), SerdeError> {
        if self.expecting_key {
            // Open the pair element and write <key>text</key>.
            self.inner
                .writer
                .write_event(Event::Start(BytesStart::new(self.entry_name.clone())))
                .map_err(|e| SerdeError::custom(format!("xml write error: {e}")))?;
            self.inner.write_text_element(&self.key_name, text)?;
            self.expecting_key = false;
        } else {
            // Write <value>text</value>, then close the pair element.
            self.inner.write_text_element(&self.value_name, text)?;
            self.inner
                .writer
                .write_event(Event::End(BytesEnd::new(self.entry_name.clone())))
                .map_err(|e| SerdeError::custom(format!("xml write error: {e}")))?;
            self.expecting_key = true;
        }
        Ok(())
    }
}

impl ShapeSerializer for XmlShapeSerializer {
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        let name = xml_element_name(schema);
        let mut start = BytesStart::new(name.to_owned());
        if self.root {
            if let Some(ns) = self.namespace {
                start.push_attribute(("xmlns", ns));
            }
            self.root = false;
        }
        self.writer
            .write_event(Event::Start(start))
            .map_err(|e| SerdeError::custom(format!("xml write error: {e}")))?;

        value.serialize_members(self)?;

        self.writer
            .write_event(Event::End(BytesEnd::new(name.to_owned())))
            .map_err(|e| SerdeError::custom(format!("xml write error: {e}")))?;
        Ok(())
    }

    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        // `restXml` has two list layouts and the model picks which one applies.
        //
        //   wrapped    <Members><item>A</item><item>B</item></Members>
        //   flattened  <Members>A</Members><Members>B</Members>
        //
        // The list member's own element name (`@xmlName` on the structure
        // member, else the member name) names the wrapper in the wrapped layout
        // and each repeated element in the flattened one. The item name comes
        // from the `@xmlName` on the *list's* member, defaulting to `member` —
        // see [`XmlListTrait`], which also carries the `@xmlFlattened` flag that
        // `aws-smithy-schema` records but does not expose.
        let layout = xml_list_trait(schema);
        let element_name = xml_element_name(schema).to_owned();
        let item_name = layout
            .and_then(|l| l.item_name())
            .or_else(|| {
                schema
                    .member()
                    .and_then(|m| m.xml_name())
                    .map(|x| x.value())
            })
            .unwrap_or("member")
            .to_owned();
        let flattened = layout.is_some_and(|l| l.flattened());

        // In the flattened layout each item IS an `element_name` element and
        // there is no wrapper at all.
        if flattened {
            let mut items = ListItemSerializer {
                inner: self,
                item_name: element_name,
            };
            return write_elements(&mut items);
        }

        self.writer
            .write_event(Event::Start(BytesStart::new(element_name.clone())))
            .map_err(|e| SerdeError::custom(format!("xml write error: {e}")))?;

        {
            let mut items = ListItemSerializer {
                inner: self,
                item_name,
            };
            write_elements(&mut items)?;
        }

        self.writer
            .write_event(Event::End(BytesEnd::new(element_name)))
            .map_err(|e| SerdeError::custom(format!("xml write error: {e}")))?;
        Ok(())
    }
    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        // See [`XmlMapTrait`] for the wrapped-vs-flattened layouts and the
        // `@xmlFlattened` flag this reads. The key and value element names honour
        // a modeled `@xmlName` on the map's key or value member; absent one, they
        // fall back to the Smithy defaults `key`/`value`.
        let flattened = xml_map_trait(schema).is_some_and(|l| l.flattened());
        let key_name = schema
            .key()
            .and_then(|m| m.xml_name())
            .map(|x| x.value())
            .unwrap_or("key")
            .to_owned();
        let value_name = schema
            .member()
            .and_then(|m| m.xml_name())
            .map(|x| x.value())
            .unwrap_or("value")
            .to_owned();

        let element_name = xml_element_name(schema).to_owned();

        // Flattened: no outer wrapper and no `<entry>`. Each pair IS one repeated
        // `element_name` element holding the key and value directly.
        if flattened {
            let mut adapter = MapEntrySerializer {
                inner: self,
                expecting_key: true,
                key_name,
                value_name,
                entry_name: element_name,
            };
            return write_entries(&mut adapter);
        }

        // Wrapped (default): the whole `<entry>` run sits inside the map member's
        // own element, so an SDK parser can locate the keys that would otherwise
        // be unreachable in a bare entry run.
        self.writer
            .write_event(Event::Start(BytesStart::new(element_name.clone())))
            .map_err(|e| SerdeError::custom(format!("xml write error: {e}")))?;

        {
            let mut adapter = MapEntrySerializer {
                inner: self,
                expecting_key: true,
                key_name,
                value_name,
                entry_name: "entry".to_owned(),
            };
            write_entries(&mut adapter)?;
        }

        self.writer
            .write_event(Event::End(BytesEnd::new(element_name)))
            .map_err(|e| SerdeError::custom(format!("xml write error: {e}")))?;
        Ok(())
    }

    fn write_boolean(&mut self, schema: &Schema, value: bool) -> Result<(), SerdeError> {
        self.write_text_element(xml_element_name(schema), &value.to_string())
    }

    fn write_byte(&mut self, schema: &Schema, value: i8) -> Result<(), SerdeError> {
        self.write_text_element(xml_element_name(schema), &value.to_string())
    }

    fn write_short(&mut self, schema: &Schema, value: i16) -> Result<(), SerdeError> {
        self.write_text_element(xml_element_name(schema), &value.to_string())
    }

    fn write_integer(&mut self, schema: &Schema, value: i32) -> Result<(), SerdeError> {
        self.write_text_element(xml_element_name(schema), &value.to_string())
    }

    fn write_long(&mut self, schema: &Schema, value: i64) -> Result<(), SerdeError> {
        self.write_text_element(xml_element_name(schema), &value.to_string())
    }

    fn write_float(&mut self, schema: &Schema, value: f32) -> Result<(), SerdeError> {
        self.write_text_element(xml_element_name(schema), &value.to_string())
    }

    fn write_double(&mut self, schema: &Schema, value: f64) -> Result<(), SerdeError> {
        self.write_text_element(xml_element_name(schema), &value.to_string())
    }

    fn write_big_integer(&mut self, schema: &Schema, value: &BigInteger) -> Result<(), SerdeError> {
        self.write_text_element(xml_element_name(schema), value.as_ref())
    }

    fn write_big_decimal(&mut self, schema: &Schema, value: &BigDecimal) -> Result<(), SerdeError> {
        self.write_text_element(xml_element_name(schema), value.as_ref())
    }

    fn write_string(&mut self, schema: &Schema, value: &str) -> Result<(), SerdeError> {
        self.write_text_element(xml_element_name(schema), value)
    }

    fn write_blob(&mut self, schema: &Schema, value: &[u8]) -> Result<(), SerdeError> {
        self.write_text_element(xml_element_name(schema), &base64::encode(value.as_ref()))
    }

    fn write_timestamp(&mut self, schema: &Schema, value: &DateTime) -> Result<(), SerdeError> {
        let s = value
            .fmt(aws_smithy_types::date_time::Format::DateTime)
            .map_err(|e| SerdeError::custom(format!("timestamp format error: {e}")))?;
        self.write_text_element(xml_element_name(schema), &s)
    }

    fn write_document(&mut self, _schema: &Schema, _value: &Document) -> Result<(), SerdeError> {
        Err(SerdeError::custom("documents not supported in XML"))
    }

    fn write_null(&mut self, _schema: &Schema) -> Result<(), SerdeError> {
        Ok(())
    }
}

// =================================================================================================
// Deserializer
// =================================================================================================

/// XML [`ShapeDeserializer`] backed by `quick-xml`.
pub struct XmlShapeDeserializer<'a> {
    /// Current position in the input for sub-deserializers.
    reader: Reader<&'a [u8]>,
    /// How many nested shapes deep this deserializer sits. The root is 0 and
    /// each sub-deserializer produced while walking a nested struct or list is
    /// one deeper; the recursive readers refuse to descend past [`MAX_DEPTH`].
    depth: usize,
}

/// The deepest nesting an XML request may reach before it is refused.
///
/// `read_struct` and `read_list` recurse into sub-deserializers for nested
/// members and list items, so request-chosen element nesting drives the
/// recursion depth directly: a document nesting elements without bound (an "XML
/// bomb") would recurse until the thread stack is exhausted and the process
/// aborts. Rejecting past a fixed depth turns that into a clean deserialization
/// error.
const MAX_DEPTH: usize = 127;

impl<'a> XmlShapeDeserializer<'a> {
    pub fn new(input: &'a [u8]) -> Self {
        Self::with_depth(input, 0)
    }

    /// Build a sub-deserializer that sits `depth` levels into the walk.
    fn with_depth(input: &'a [u8], depth: usize) -> Self {
        let mut reader = Reader::from_reader(input);
        // NOT `trim_text(true)`. The reader breaks text at every entity reference, so a value
        // arrives as several fragments and per-fragment trimming would eat the value's own
        // interior spaces: `a &amp; b` would deserialize as `a&b`. Whitespace is handled where
        // the meaning is known instead: an assembled value is trimmed once, when it is returned.
        // The paths that COPY events forward text verbatim — the captured fragment FEEDS a value
        // reader, so dropping a space there would drop it from the value.
        reader.config_mut().trim_text(false);
        Self { reader, depth }
    }

    /// Read the text content of the current element (assumes we just consumed a Start event).
    fn read_element_text(&mut self) -> Result<String, SerdeError> {
        let mut buf = Vec::new();
        let mut text = AssembledText::default();
        loop {
            match self.reader.read_event_into(&mut buf) {
                Ok(Event::Text(e)) => {
                    text.push_text(decoded_text(&e)?);
                }
                // The reference is part of the value, arriving as its own event.
                Ok(Event::GeneralRef(r)) => {
                    text.push_stated(resolved_reference(&r)?);
                }
                // VERBATIM, and stated: CDATA exists to hold bytes no entity resolution
                // should touch, so it is neither unescaped nor trimmed. Dropping it made
                // `<Id><![CDATA[foo]]></Id>` deserialize to the empty string.
                Ok(Event::CData(c)) => {
                    text.push_stated(cdata_text(&c)?);
                }
                Ok(Event::End(_)) => return Ok(text.finish()),
                Ok(Event::Start(_)) => {
                    // Nested element inside what we expected to be text — skip it
                    self.skip_element(&mut buf)?;
                }
                Ok(Event::Eof) => {
                    return Err(SerdeError::custom("unexpected EOF reading element text"))
                }
                Err(e) => return Err(SerdeError::custom(format!("xml parse error: {e}"))),
                _ => {}
            }
            buf.clear();
        }
    }

    /// Skip the current element and all its children (assumes we just consumed a Start event).
    fn skip_element(&mut self, buf: &mut Vec<u8>) -> Result<(), SerdeError> {
        let mut depth = 1u32;
        loop {
            match self.reader.read_event_into(buf) {
                Ok(Event::Start(_)) => depth += 1,
                Ok(Event::End(_)) => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                Ok(Event::Eof) => {
                    return Err(SerdeError::custom("unexpected EOF while skipping element"))
                }
                Err(e) => return Err(SerdeError::custom(format!("xml parse error: {e}"))),
                _ => {}
            }
            buf.clear();
        }
    }

    /// Read the inner XML content of the current element (we just consumed a Start event).
    /// Returns the raw bytes between the start and end tags, preserving nested structure.
    fn read_inner_xml(&mut self, buf: &mut Vec<u8>) -> Result<Vec<u8>, SerdeError> {
        let mut output = Vec::new();
        let mut writer = Writer::new(Cursor::new(&mut output));
        let mut depth = 1u32;
        loop {
            buf.clear();
            match self.reader.read_event_into(buf) {
                Ok(Event::Start(ref e)) => {
                    depth += 1;
                    writer
                        .write_event(Event::Start(e.clone()))
                        .map_err(|e| SerdeError::custom(format!("xml write error: {e}")))?;
                }
                Ok(Event::End(ref e)) => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    writer
                        .write_event(Event::End(e.clone()))
                        .map_err(|e| SerdeError::custom(format!("xml write error: {e}")))?;
                }
                Ok(Event::Text(ref e)) => {
                    writer
                        .write_event(Event::Text(e.clone()))
                        .map_err(|e| SerdeError::custom(format!("xml write error: {e}")))?;
                }
                // Forwarded verbatim: dropping it would silently strip entities from the
                // captured XML, which is copied through rather than interpreted here.
                Ok(Event::GeneralRef(ref e)) => {
                    writer
                        .write_event(Event::GeneralRef(e.clone()))
                        .map_err(|e| SerdeError::custom(format!("xml write error: {e}")))?;
                }
                // Forwarded for the same reason: dropping it loses the section's bytes from
                // the captured XML before it is re-parsed.
                Ok(Event::CData(ref e)) => {
                    writer
                        .write_event(Event::CData(e.clone()))
                        .map_err(|e| SerdeError::custom(format!("xml write error: {e}")))?;
                }
                // A self-closing child, forwarded for the same reason. `depth` is NOT touched:
                // an empty element opens and closes in one event, so counting it as a Start
                // would leave the loop waiting for an End that never comes.
                Ok(Event::Empty(ref e)) => {
                    writer
                        .write_event(Event::Empty(e.clone()))
                        .map_err(|e| SerdeError::custom(format!("xml write error: {e}")))?;
                }
                Ok(Event::Eof) => {
                    return Err(SerdeError::custom("unexpected EOF while reading inner XML"))
                }
                Err(e) => return Err(SerdeError::custom(format!("xml parse error: {e}"))),
                _ => {}
            }
        }
        drop(writer);
        Ok(output)
    }

    /// Advance past any leading Start element (the root wrapper) so we're positioned
    /// at the first child.
    ///
    /// A document with no root element at all is *not* an error: it is a struct
    /// with zero members. S3 `CreateBucket` relies on this — it sends an empty
    /// body, or a childless `<CreateBucketConfiguration/>`. Enforcement of
    /// `@required` members stays with the caller's member consumer, which is the
    /// layer that knows the schema.
    fn consume_root_start(&mut self) -> Result<(), SerdeError> {
        let mut buf = Vec::new();
        loop {
            match self.reader.read_event_into(&mut buf) {
                Ok(Event::Start(_)) => return Ok(()),
                // A self-closing root (`<CreateBucketConfiguration/>`) has no
                // children, so the root is consumed and there is nothing to
                // position onto.
                Ok(Event::Empty(_)) => return Ok(()),
                Ok(Event::Decl(_)) | Ok(Event::Comment(_)) | Ok(Event::PI(_)) => {
                    buf.clear();
                    continue;
                }
                // A document type declaration is refused outright. Only the five
                // predefined XML entities and numeric character references are
                // resolved (see `resolved_reference`); a DOCTYPE is the one place
                // a document could define its own entities, so rejecting it keeps
                // entity resolution restricted to the predefined set and leaves
                // no path to pull in external or custom-defined entities.
                Ok(Event::DocType(_)) => {
                    return Err(SerdeError::custom(
                        "XML document type declarations (DOCTYPE/DTD) are not accepted",
                    ))
                }
                // Element-less document (blank body, or a bare XML declaration).
                Ok(Event::Eof) => return Ok(()),
                Err(e) => return Err(SerdeError::custom(format!("xml parse error: {e}"))),
                _ => {
                    buf.clear();
                    continue;
                }
            }
        }
    }
}

impl<'a> ShapeDeserializer for XmlShapeDeserializer<'a> {
    fn read_struct(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(&Schema, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        if self.depth > MAX_DEPTH {
            return Err(SerdeError::custom(format!(
                "XML nests deeper than the limit of {MAX_DEPTH}"
            )));
        }
        // Consume the root/wrapper start element
        self.consume_root_start()?;

        let mut buf = Vec::new();
        // A `@xmlFlattened` list arrives as N sibling elements that all repeat the
        // member's name, not as one wrapper holding N items. Each sibling matches
        // the same member, so handing them to the consumer one at a time would
        // rebuild the list from only the last element. Instead their fragments are
        // buffered here, re-wrapped into the wrapped layout once the struct is
        // fully read, and handed over in a single consumer call — which lets the
        // existing wrapped-layout `read_list` parse them unchanged.
        let mut flattened: Vec<(usize, String, Vec<u8>)> = Vec::new();
        loop {
            match self.reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    let local = std::str::from_utf8(e.local_name().as_ref())
                        .map_err(|e| SerdeError::custom(format!("invalid utf8 in tag: {e}")))?
                        .to_owned();
                    // Captured so the sub-deserializer can handle scalar text and nested
                    // struct/list content alike. Read BEFORE the member lookup would let an
                    // unknown member consume it, so the lookup happens inside the helper and
                    // the skip stays here.
                    let inner_xml = self.read_inner_xml(&mut buf)?;
                    // The bool says whether a member matched. An unknown one needs no skip:
                    // the `read_inner_xml` above already consumed its content.
                    let _matched = dispatch_struct_member(
                        schema,
                        &local,
                        &inner_xml,
                        &mut flattened,
                        self.depth + 1,
                        consumer,
                    )?;
                }
                // A SELF-CLOSING MEMBER IS STILL A MEMBER. `<Status/>` is `<Status></Status>`
                // with the same meaning, and the reader delivers the first as one `Empty` event
                // and the second as Start-then-End — so an arm that handles only `Start` drops
                // it, and it reached the catch-all below.
                //
                // Two consequences, and the second is the worse one: a scalar member vanishes
                // from the struct, and a flattened-list item vanishes from its list, shifting
                // every later index so a positional caller reads the wrong element.
                //
                // `read_inner_xml` is deliberately NOT called: the element is already closed,
                // so reading would consume the SIBLING's events and corrupt the rest of the
                // struct. Its content is empty by definition.
                Ok(Event::Empty(ref e)) => {
                    let local = std::str::from_utf8(e.local_name().as_ref())
                        .map_err(|e| SerdeError::custom(format!("invalid utf8 in tag: {e}")))?
                        .to_owned();
                    dispatch_struct_member(schema, &local, &[], &mut flattened, self.depth + 1, consumer)?;
                }
                Ok(Event::End(_)) | Ok(Event::Eof) => break,
                Err(e) => return Err(SerdeError::custom(format!("xml parse error: {e}"))),
                _ => {}
            }
            buf.clear();
        }

        for (index, element_name, fragments) in flattened {
            let Some(member) = schema
                .members()
                .iter()
                .copied()
                .find(|m| m.member_index() == Some(index))
            else {
                continue;
            };
            // A flattened map's drained run is repeated `<element_name>` pairs,
            // not list items, so its sub-deserializer parses the run as a map;
            // a flattened list's run reads as list items. Both share the same
            // buffering above and differ only here.
            let mut sub = if is_flattened_map(member) {
                XmlElementDeserializer::flattened_map(&element_name, &fragments, self.depth + 1)
            } else {
                XmlElementDeserializer::with_depth(&element_name, &fragments, self.depth + 1)
            };
            consumer(member, &mut sub)?;
        }
        Ok(())
    }

    fn read_list(
        &mut self,
        _schema: &Schema,
        consumer: &mut dyn FnMut(&mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        if self.depth > MAX_DEPTH {
            return Err(SerdeError::custom(format!(
                "XML nests deeper than the limit of {MAX_DEPTH}"
            )));
        }
        // We're positioned inside the wrapper element; each child is a list item.
        let mut buf = Vec::new();
        loop {
            match self.reader.read_event_into(&mut buf) {
                Ok(Event::Start(_)) => {
                    let text = self.read_element_text()?;
                    let mut sub = XmlTextDeserializer::new(&text);
                    consumer(&mut sub)?;
                }
                // A SELF-CLOSING ITEM IS STILL AN ITEM. `<member/>` is a member whose content
                // is empty, so it must reach `consumer` exactly like `<member></member>`,
                // which the reader delivers as Start-then-End and this arm already accepts.
                //
                // Dropping it corrupts the list's ARITY rather than one item's value, which is
                // the worse failure: every later index shifts, so a caller reading positionally
                // gets the wrong element with nothing appearing to have gone wrong.
                Ok(Event::Empty(_)) => {
                    let mut sub = XmlTextDeserializer::new("");
                    consumer(&mut sub)?;
                }
                Ok(Event::End(_)) => return Ok(()),
                Ok(Event::Eof) => return Ok(()),
                Err(e) => return Err(SerdeError::custom(format!("xml parse error: {e}"))),
                _ => {}
            }
            buf.clear();
        }
    }

    fn read_map(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(String, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        if self.depth > MAX_DEPTH {
            return Err(SerdeError::custom(format!(
                "XML nests deeper than the limit of {MAX_DEPTH}"
            )));
        }
        // XML maps: <entry><key>k</key><value>v</value></entry>. The `key`
        // element name honours a modeled `@xmlName` on the map's key member
        // (read off `schema.key()`), symmetric with the serializer; the value is
        // whatever element is not the key, so a renamed value needs no match.
        // The `entry` wrapper has no rename trait, matching the serialize side.
        let key_name = schema
            .key()
            .and_then(|m| m.xml_name())
            .map(|x| x.value())
            .unwrap_or("key")
            .to_owned();
        let mut buf = Vec::new();
        loop {
            match self.reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    let tag = std::str::from_utf8(e.local_name().as_ref())
                        .unwrap_or("")
                        .to_owned();
                    if tag == "entry" {
                        let mut key = String::new();
                        let mut value_text = String::new();
                        let mut inner_buf = Vec::new();
                        loop {
                            match self.reader.read_event_into(&mut inner_buf) {
                                Ok(Event::Start(ref ie)) => {
                                    let local_name = ie.local_name();
                                    let itag =
                                        std::str::from_utf8(local_name.as_ref()).unwrap_or("");
                                    let text = self.read_element_text()?;
                                    if itag == key_name {
                                        key = text;
                                    } else {
                                        value_text = text;
                                    }
                                }
                                Ok(Event::End(_)) => break,
                                Ok(Event::Eof) => break,
                                Err(e) => {
                                    return Err(SerdeError::custom(format!("xml parse error: {e}")))
                                }
                                _ => {}
                            }
                            inner_buf.clear();
                        }
                        let mut sub = XmlTextDeserializer::new(&value_text);
                        consumer(key, &mut sub)?;
                    } else {
                        self.skip_element(&mut buf)?;
                    }
                }
                Ok(Event::End(_)) => return Ok(()),
                Ok(Event::Eof) => return Ok(()),
                Err(e) => return Err(SerdeError::custom(format!("xml parse error: {e}"))),
                _ => {}
            }
            buf.clear();
        }
    }

    fn read_boolean(&mut self, _schema: &Schema) -> Result<bool, SerdeError> {
        Err(SerdeError::custom(
            "read_boolean called on XML element deserializer directly",
        ))
    }

    fn read_byte(&mut self, _schema: &Schema) -> Result<i8, SerdeError> {
        Err(SerdeError::custom(
            "read_byte called on XML element deserializer directly",
        ))
    }

    fn read_short(&mut self, _schema: &Schema) -> Result<i16, SerdeError> {
        Err(SerdeError::custom(
            "read_short called on XML element deserializer directly",
        ))
    }

    fn read_integer(&mut self, _schema: &Schema) -> Result<i32, SerdeError> {
        Err(SerdeError::custom(
            "read_integer called on XML element deserializer directly",
        ))
    }

    fn read_long(&mut self, _schema: &Schema) -> Result<i64, SerdeError> {
        Err(SerdeError::custom(
            "read_long called on XML element deserializer directly",
        ))
    }

    fn read_float(&mut self, _schema: &Schema) -> Result<f32, SerdeError> {
        Err(SerdeError::custom(
            "read_float called on XML element deserializer directly",
        ))
    }

    fn read_double(&mut self, _schema: &Schema) -> Result<f64, SerdeError> {
        Err(SerdeError::custom(
            "read_double called on XML element deserializer directly",
        ))
    }

    fn read_big_integer(&mut self, _schema: &Schema) -> Result<BigInteger, SerdeError> {
        Err(SerdeError::custom(
            "read_big_integer called on XML element deserializer directly",
        ))
    }

    fn read_big_decimal(&mut self, _schema: &Schema) -> Result<BigDecimal, SerdeError> {
        Err(SerdeError::custom(
            "read_big_decimal called on XML element deserializer directly",
        ))
    }

    fn read_string(&mut self, _schema: &Schema) -> Result<String, SerdeError> {
        Err(SerdeError::custom(
            "read_string called on XML element deserializer directly",
        ))
    }

    fn read_blob(&mut self, _schema: &Schema) -> Result<Blob, SerdeError> {
        Err(SerdeError::custom(
            "read_blob called on XML element deserializer directly",
        ))
    }

    fn read_timestamp(&mut self, _schema: &Schema) -> Result<DateTime, SerdeError> {
        Err(SerdeError::custom(
            "read_timestamp called on XML element deserializer directly",
        ))
    }

    fn read_document(&mut self, _schema: &Schema) -> Result<Document, SerdeError> {
        Err(SerdeError::custom("documents not supported in XML"))
    }

    fn is_null(&self) -> bool {
        false
    }

    fn container_size(&self) -> Option<usize> {
        None
    }
}

// =================================================================================================
// XmlTextDeserializer — deserializes scalar values from element text content
// =================================================================================================
// XmlElementDeserializer — handles both scalar text and nested struct/list content
// =================================================================================================

/// A deserializer for a single XML element's content. It holds the raw inner XML
/// bytes and can handle:
/// - Scalar reads: extracts the text content (for simple elements like `<Id>foo</Id>`)
/// - Struct reads: parses nested elements as struct members
/// - List reads: parses child elements as list items
struct XmlElementDeserializer {
    /// The element name (for wrapping in struct reads)
    element_name: String,
    /// The raw inner XML bytes between the start and end tags
    inner_xml: Vec<u8>,
    /// How many nested shapes deep this item sits; carried into the
    /// sub-deserializer its reads build so the depth bound spans both types.
    depth: usize,
    /// Whether `inner_xml` is a flattened map's drained run rather than a
    /// wrapped one; see [`XmlMapTrait`] for the two layouts. `read_map` parses
    /// the flattened layout when set and the wrapped `<entry>` layout otherwise.
    flattened_map: bool,
}

impl XmlElementDeserializer {
    /// Build an item deserializer that sits `depth` levels into the walk.
    fn with_depth(element_name: &str, inner_xml: &[u8], depth: usize) -> Self {
        Self {
            element_name: element_name.to_owned(),
            inner_xml: inner_xml.to_vec(),
            depth,
            flattened_map: false,
        }
    }

    /// Build an item deserializer whose `inner_xml` is a flattened map's drained
    /// run of repeated `<element_name>` key/value pairs, so `read_map` parses the
    /// repeated-pair layout rather than the wrapped `<entry>` one.
    fn flattened_map(element_name: &str, inner_xml: &[u8], depth: usize) -> Self {
        Self {
            element_name: element_name.to_owned(),
            inner_xml: inner_xml.to_vec(),
            depth,
            flattened_map: true,
        }
    }

    /// Extract plain text from the inner XML (for scalar values).
    fn text_content(&self) -> Result<String, SerdeError> {
        // If the inner XML is just text (no child elements), return it directly
        let mut reader = Reader::from_reader(self.inner_xml.as_slice());
        // NOT `trim_text(true)`. The reader breaks text at every entity reference, so a value
        // arrives as several fragments and per-fragment trimming would eat the value's own
        // interior spaces: `a &amp; b` would deserialize as `a&b`. Whitespace is handled where
        // the meaning is known instead: an assembled value is trimmed once, when it is returned.
        // The paths that COPY events forward text verbatim — the captured fragment FEEDS a value
        // reader, so dropping a space there would drop it from the value.
        reader.config_mut().trim_text(false);
        let mut text = AssembledText::default();
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Text(e)) => {
                    text.push_text(decoded_text(&e)?);
                }
                // The reference is part of the value, arriving as its own event.
                Ok(Event::GeneralRef(r)) => {
                    text.push_stated(resolved_reference(&r)?);
                }
                // VERBATIM, and stated: CDATA exists to hold bytes no entity resolution
                // should touch, so it is neither unescaped nor trimmed. Dropping it made
                // `<Id><![CDATA[foo]]></Id>` deserialize to the empty string.
                Ok(Event::CData(c)) => {
                    text.push_stated(cdata_text(&c)?);
                }
                Ok(Event::Eof) => break,
                Ok(Event::Start(_)) => {
                    // Has nested elements — not pure text
                    return Ok(text.finish());
                }
                Err(e) => return Err(SerdeError::custom(format!("xml parse error: {e}"))),
                _ => {}
            }
            buf.clear();
        }
        // Trimmed on the text-sourced EDGES only: formatting whitespace around an element's
        // text is not part of it, but whitespace the document states — between fragments, or
        // encoded as a reference at an edge — is.
        Ok(text.finish())
    }

    /// Rewrite a flattened map's drained run into the wrapped `<entry>` layout
    /// the map reader already understands, then parse it with that one reader.
    /// See [`XmlMapTrait`] for the two layouts.
    ///
    /// Each top-level grouping element is one pair whose `<key>`/`<value>`
    /// children are exactly an `<entry>` body, so wrapping that body in `entry`
    /// yields the wrapped document `XmlShapeDeserializer::read_map` parses. Key
    /// and value resolution (the modeled key `@xmlName`, the value as the
    /// non-key child) then come from that one reader, identical to the wrapped
    /// path. This mirrors the flattened-list read, which re-wraps its siblings
    /// into the wrapped layout `read_list` already understands rather than
    /// teaching the reader a second shape.
    ///
    /// A grouping element with no child names no key, so it is dropped rather
    /// than wrapped: a map has no positional arity to preserve (unlike a list),
    /// and an empty pair would read as a spurious empty-key entry that a later
    /// empty element would overwrite.
    fn read_flattened_map(
        &self,
        schema: &Schema,
        consumer: &mut dyn FnMut(String, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        let mut reader = Reader::from_reader(self.inner_xml.as_slice());
        reader.config_mut().trim_text(false);
        let mut rewritten: Vec<u8> = Vec::new();
        let mut depth = 0u32;
        // One top-level grouping element is one pair. Its children are buffered
        // and flushed as one `<entry>...</entry>` only once the pair is known to
        // hold a child element — a pair with no child names no key, and a map
        // has no positional arity to preserve (unlike a list), so an empty pair
        // is dropped rather than read as a spurious empty-key entry that a later
        // empty element would overwrite. A pair's children and nested content
        // are forwarded verbatim so the wrapped reader resolves them unchanged.
        let mut pair_writer = Writer::new(Cursor::new(Vec::new()));
        let mut pair_has_child = false;
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    if depth == 0 {
                        pair_has_child = false;
                        pair_writer = Writer::new(Cursor::new(Vec::new()));
                    } else {
                        if depth == 1 {
                            pair_has_child = true;
                        }
                        pair_writer
                            .write_event(Event::Start(e.clone()))
                            .map_err(|e| SerdeError::custom(format!("xml write error: {e}")))?;
                    }
                    depth += 1;
                }
                Ok(Event::End(ref e)) => {
                    depth -= 1;
                    if depth == 0 {
                        if pair_has_child {
                            let body = pair_writer.into_inner().into_inner();
                            rewritten.extend_from_slice(b"<entry>");
                            rewritten.extend_from_slice(&body);
                            rewritten.extend_from_slice(b"</entry>");
                        }
                        pair_writer = Writer::new(Cursor::new(Vec::new()));
                    } else {
                        pair_writer
                            .write_event(Event::End(e.clone()))
                            .map_err(|e| SerdeError::custom(format!("xml write error: {e}")))?;
                    }
                }
                // A self-closing element: a top-level `<Member/>` holds no child
                // and names no pair, so it is dropped; one inside a pair is a
                // child, forwarded verbatim.
                Ok(Event::Empty(ref e)) => {
                    if depth != 0 {
                        if depth == 1 {
                            pair_has_child = true;
                        }
                        pair_writer
                            .write_event(Event::Empty(e.clone()))
                            .map_err(|e| SerdeError::custom(format!("xml write error: {e}")))?;
                    }
                }
                Ok(Event::Eof) => break,
                Err(e) => return Err(SerdeError::custom(format!("xml parse error: {e}"))),
                // Text, references and CDATA inside a pair are forwarded verbatim.
                Ok(other) => {
                    if depth != 0 {
                        pair_writer
                            .write_event(other)
                            .map_err(|e| SerdeError::custom(format!("xml write error: {e}")))?;
                    }
                }
            }
            buf.clear();
        }

        let mut sub = XmlShapeDeserializer::with_depth(&rewritten, self.depth);
        sub.read_map(schema, consumer)
    }
}

impl ShapeDeserializer for XmlElementDeserializer {
    fn read_struct(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(&Schema, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        if self.depth > MAX_DEPTH {
            return Err(SerdeError::custom(format!(
                "XML nests deeper than the limit of {MAX_DEPTH}"
            )));
        }
        // Wrap the inner XML in the element tag so XmlShapeDeserializer can
        // consume_root_start and then iterate over children. The element name is
        // gated by `is_xml_name`, like the write path (`append_element`).
        if !is_xml_name(&self.element_name) {
            return Err(SerdeError::custom(format!(
                "element name `{}` is not a legal XML name",
                self.element_name
            )));
        }
        let mut wrapped = Vec::new();
        wrapped.extend_from_slice(b"<");
        wrapped.extend_from_slice(self.element_name.as_bytes());
        wrapped.extend_from_slice(b">");
        wrapped.extend_from_slice(&self.inner_xml);
        wrapped.extend_from_slice(b"</");
        wrapped.extend_from_slice(self.element_name.as_bytes());
        wrapped.extend_from_slice(b">");

        // Delegate at `self.depth`: this item's own element level was already
        // counted when the item was created (`dispatch_struct_member` and the
        // list-item loop build items at the parent's depth + 1), so the wrapped
        // reader continues at that same depth rather than counting the element
        // twice. The entry check above still bounds the recursion.
        let mut sub = XmlShapeDeserializer::with_depth(&wrapped, self.depth);
        sub.read_struct(schema, consumer)
    }

    fn read_list(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(&mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        if self.depth > MAX_DEPTH {
            return Err(SerdeError::custom(format!(
                "XML nests deeper than the limit of {MAX_DEPTH}"
            )));
        }
        // The inner XML contains the list items directly
        let mut reader = Reader::from_reader(self.inner_xml.as_slice());
        // NOT `trim_text(true)`. The reader breaks text at every entity reference, so a value
        // arrives as several fragments and per-fragment trimming would eat the value's own
        // interior spaces: `a &amp; b` would deserialize as `a&b`. Whitespace is handled where
        // the meaning is known instead: an assembled value is trimmed once, when it is returned.
        // The paths that COPY events forward text verbatim — the captured fragment FEEDS a value
        // reader, so dropping a space there would drop it from the value.
        reader.config_mut().trim_text(false);
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) => {
                    // Each child element is a list item
                    let mut inner_buf = Vec::new();
                    let mut inner_writer = Writer::new(Cursor::new(&mut inner_buf));
                    let mut depth = 1u32;
                    loop {
                        let mut sub_buf = Vec::new();
                        match reader.read_event_into(&mut sub_buf) {
                            Ok(Event::Start(ref se)) => {
                                depth += 1;
                                inner_writer.write_event(Event::Start(se.clone())).map_err(
                                    |e| SerdeError::custom(format!("xml write error: {e}")),
                                )?;
                            }
                            Ok(Event::End(ref se)) => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                                inner_writer
                                    .write_event(Event::End(se.clone()))
                                    .map_err(|e| {
                                        SerdeError::custom(format!("xml write error: {e}"))
                                    })?;
                            }
                            Ok(Event::Text(ref t)) => {
                                inner_writer
                                    .write_event(Event::Text(t.clone()))
                                    .map_err(|e| {
                                        SerdeError::custom(format!("xml write error: {e}"))
                                    })?;
                            }
                            // Forwarded verbatim, as above.
                            Ok(Event::GeneralRef(ref t)) => {
                                inner_writer
                                    .write_event(Event::GeneralRef(t.clone()))
                                    .map_err(|e| {
                                        SerdeError::custom(format!("xml write error: {e}"))
                                    })?;
                            }
                            // A self-closing child and a CDATA section, forwarded for the same
                            // reason. This loop is the SIBLING of `read_inner_xml`'s: every
                            // event kind one forwards, the other must, or a list item loses
                            // content the top level keeps.
                            Ok(Event::Empty(ref t)) => {
                                inner_writer
                                    .write_event(Event::Empty(t.clone()))
                                    .map_err(|e| {
                                        SerdeError::custom(format!("xml write error: {e}"))
                                    })?;
                            }
                            Ok(Event::CData(ref t)) => {
                                inner_writer
                                    .write_event(Event::CData(t.clone()))
                                    .map_err(|e| {
                                        SerdeError::custom(format!("xml write error: {e}"))
                                    })?;
                            }
                            Ok(Event::Eof) => {
                                return Err(SerdeError::custom("unexpected EOF in list item"))
                            }
                            Err(e) => {
                                return Err(SerdeError::custom(format!("xml parse error: {e}")))
                            }
                            _ => {}
                        }
                    }
                    drop(inner_writer);

                    let tag_name = std::str::from_utf8(e.local_name().as_ref())
                        .unwrap_or("item")
                        .to_owned();
                    let mut item_deser =
                        XmlElementDeserializer::with_depth(&tag_name, &inner_buf, self.depth + 1);
                    consumer(&mut item_deser)?;
                }
                // The same item, self-closing: an item with EMPTY inner XML rather than no
                // item. See the arity note on the sibling `read_list`.
                Ok(Event::Empty(ref e)) => {
                    let tag_name = std::str::from_utf8(e.local_name().as_ref())
                        .unwrap_or("item")
                        .to_owned();
                    let mut item_deser =
                        XmlElementDeserializer::with_depth(&tag_name, &[], self.depth + 1);
                    consumer(&mut item_deser)?;
                }
                Ok(Event::End(_)) => break,
                Ok(Event::Eof) => break,
                Err(e) => return Err(SerdeError::custom(format!("xml parse error: {e}"))),
                _ => {}
            }
            buf.clear();
        }
        let _ = schema;
        Ok(())
    }

    fn read_map(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(String, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        if self.depth > MAX_DEPTH {
            return Err(SerdeError::custom(format!(
                "XML nests deeper than the limit of {MAX_DEPTH}"
            )));
        }
        if self.flattened_map {
            return self.read_flattened_map(schema, consumer);
        }
        // Wrap and delegate to XmlShapeDeserializer
        let mut wrapped = Vec::new();
        wrapped.extend_from_slice(b"<map>");
        wrapped.extend_from_slice(&self.inner_xml);
        wrapped.extend_from_slice(b"</map>");
        // Delegate at `self.depth`: this item's element level was already
        // counted when the item was created, so the wrapped reader continues at
        // that depth rather than counting it twice. The entry check above
        // bounds the recursion.
        let mut sub = XmlShapeDeserializer::with_depth(&wrapped, self.depth);
        sub.read_map(schema, consumer)
    }

    fn read_boolean(&mut self, _schema: &Schema) -> Result<bool, SerdeError> {
        let text = self.text_content()?;
        match text.as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            other => Err(SerdeError::custom(format!("invalid boolean: '{other}'"))),
        }
    }

    fn read_byte(&mut self, _schema: &Schema) -> Result<i8, SerdeError> {
        self.text_content()?
            .parse()
            .map_err(|e| SerdeError::custom(format!("invalid byte: {e}")))
    }

    fn read_short(&mut self, _schema: &Schema) -> Result<i16, SerdeError> {
        self.text_content()?
            .parse()
            .map_err(|e| SerdeError::custom(format!("invalid short: {e}")))
    }

    fn read_integer(&mut self, _schema: &Schema) -> Result<i32, SerdeError> {
        self.text_content()?
            .parse()
            .map_err(|e| SerdeError::custom(format!("invalid integer: {e}")))
    }

    fn read_long(&mut self, _schema: &Schema) -> Result<i64, SerdeError> {
        self.text_content()?
            .parse()
            .map_err(|e| SerdeError::custom(format!("invalid long: {e}")))
    }

    fn read_float(&mut self, _schema: &Schema) -> Result<f32, SerdeError> {
        self.text_content()?
            .parse()
            .map_err(|e| SerdeError::custom(format!("invalid float: {e}")))
    }

    fn read_double(&mut self, _schema: &Schema) -> Result<f64, SerdeError> {
        self.text_content()?
            .parse()
            .map_err(|e| SerdeError::custom(format!("invalid double: {e}")))
    }

    fn read_big_integer(&mut self, _schema: &Schema) -> Result<BigInteger, SerdeError> {
        BigInteger::from_str(&self.text_content()?)
            .map_err(|e| SerdeError::custom(format!("invalid BigInteger: {e}")))
    }

    fn read_big_decimal(&mut self, _schema: &Schema) -> Result<BigDecimal, SerdeError> {
        BigDecimal::from_str(&self.text_content()?)
            .map_err(|e| SerdeError::custom(format!("invalid BigDecimal: {e}")))
    }

    fn read_string(&mut self, _schema: &Schema) -> Result<String, SerdeError> {
        self.text_content()
    }

    fn read_blob(&mut self, _schema: &Schema) -> Result<Blob, SerdeError> {
        let text = self.text_content()?;
        let bytes = base64::decode(&text)
            .map_err(|e| SerdeError::custom(format!("invalid base64: {e}")))?;
        Ok(Blob::new(bytes))
    }

    fn read_timestamp(&mut self, _schema: &Schema) -> Result<DateTime, SerdeError> {
        let text = self.text_content()?;
        DateTime::from_str(&text, aws_smithy_types::date_time::Format::DateTime)
            .map_err(|e| SerdeError::custom(format!("invalid timestamp: {e}")))
    }

    fn read_document(&mut self, _schema: &Schema) -> Result<Document, SerdeError> {
        Err(SerdeError::custom("documents not supported in XML"))
    }

    fn is_null(&self) -> bool {
        false
    }

    fn container_size(&self) -> Option<usize> {
        None
    }
}

// =================================================================================================
// XmlTextDeserializer — deserializes scalar values from element text content
// =================================================================================================

struct XmlTextDeserializer {
    text: String,
}

impl XmlTextDeserializer {
    fn new(text: &str) -> Self {
        Self {
            text: text.to_owned(),
        }
    }
}

impl ShapeDeserializer for XmlTextDeserializer {
    fn read_struct(
        &mut self,
        _schema: &Schema,
        _consumer: &mut dyn FnMut(&Schema, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        Err(SerdeError::custom(
            "cannot read struct from text deserializer",
        ))
    }

    fn read_list(
        &mut self,
        _schema: &Schema,
        _consumer: &mut dyn FnMut(&mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        Err(SerdeError::custom(
            "cannot read list from text deserializer",
        ))
    }

    fn read_map(
        &mut self,
        _schema: &Schema,
        _consumer: &mut dyn FnMut(String, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        Err(SerdeError::custom("cannot read map from text deserializer"))
    }

    fn read_boolean(&mut self, _schema: &Schema) -> Result<bool, SerdeError> {
        match self.text.as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            other => Err(SerdeError::custom(format!("invalid boolean: '{other}'"))),
        }
    }

    fn read_byte(&mut self, _schema: &Schema) -> Result<i8, SerdeError> {
        self.text
            .parse()
            .map_err(|e| SerdeError::custom(format!("invalid byte: {e}")))
    }

    fn read_short(&mut self, _schema: &Schema) -> Result<i16, SerdeError> {
        self.text
            .parse()
            .map_err(|e| SerdeError::custom(format!("invalid short: {e}")))
    }

    fn read_integer(&mut self, _schema: &Schema) -> Result<i32, SerdeError> {
        self.text
            .parse()
            .map_err(|e| SerdeError::custom(format!("invalid integer: {e}")))
    }

    fn read_long(&mut self, _schema: &Schema) -> Result<i64, SerdeError> {
        self.text
            .parse()
            .map_err(|e| SerdeError::custom(format!("invalid long: {e}")))
    }

    fn read_float(&mut self, _schema: &Schema) -> Result<f32, SerdeError> {
        self.text
            .parse()
            .map_err(|e| SerdeError::custom(format!("invalid float: {e}")))
    }

    fn read_double(&mut self, _schema: &Schema) -> Result<f64, SerdeError> {
        self.text
            .parse()
            .map_err(|e| SerdeError::custom(format!("invalid double: {e}")))
    }

    fn read_big_integer(&mut self, _schema: &Schema) -> Result<BigInteger, SerdeError> {
        BigInteger::from_str(&self.text)
            .map_err(|e| SerdeError::custom(format!("invalid BigInteger: {e}")))
    }

    fn read_big_decimal(&mut self, _schema: &Schema) -> Result<BigDecimal, SerdeError> {
        BigDecimal::from_str(&self.text)
            .map_err(|e| SerdeError::custom(format!("invalid BigDecimal: {e}")))
    }

    fn read_string(&mut self, _schema: &Schema) -> Result<String, SerdeError> {
        Ok(std::mem::take(&mut self.text))
    }

    fn read_blob(&mut self, _schema: &Schema) -> Result<Blob, SerdeError> {
        let bytes = base64::decode(&self.text)
            .map_err(|e| SerdeError::custom(format!("invalid base64: {e}")))?;
        Ok(Blob::new(bytes))
    }

    fn read_timestamp(&mut self, _schema: &Schema) -> Result<DateTime, SerdeError> {
        DateTime::from_str(&self.text, aws_smithy_types::date_time::Format::DateTime)
            .map_err(|e| SerdeError::custom(format!("invalid timestamp: {e}")))
    }

    fn read_document(&mut self, _schema: &Schema) -> Result<Document, SerdeError> {
        Err(SerdeError::custom("documents not supported in XML"))
    }

    fn is_null(&self) -> bool {
        false
    }

    fn container_size(&self) -> Option<usize> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_schema::{ShapeType, TraitMap};
    use std::sync::LazyLock;

    static FLAT_LIST_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
        let mut m = TraitMap::new();
        m.insert(Box::new(XmlListTrait::new(true, None)));
        m
    });

    static WRAPPED_LIST_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
        let mut m = TraitMap::new();
        m.insert(Box::new(XmlListTrait::new(false, Some("Item"))));
        m
    });

    /// `struct Config { Status: String, Prefixes: @xmlFlattened @xmlName("Prefix") list<String> }`
    struct ConfigSchema;

    impl ConfigSchema {
        const M_STATUS: Schema = Schema::new_member(
            ShapeId::from_static("com.test#Config$Status", "com.test", "Status"),
            ShapeType::String,
            "Status",
            0,
        );
        const M_PREFIXES: Schema = Schema::new_member(
            ShapeId::from_static("com.test#Config$Prefixes", "com.test", "Prefixes"),
            ShapeType::List,
            "Prefixes",
            1,
        )
        .with_xml_name("Prefix")
        .with_traits(&FLAT_LIST_TRAITS);
        const M_WRAPPED: Schema = Schema::new_member(
            ShapeId::from_static("com.test#Config$Wrapped", "com.test", "Wrapped"),
            ShapeType::List,
            "Wrapped",
            2,
        )
        .with_traits(&WRAPPED_LIST_TRAITS);
        const SCHEMA: Schema = Schema::new_struct(
            ShapeId::from_static("com.test#Config", "com.test", "Config"),
            ShapeType::Structure,
            &[&Self::M_STATUS, &Self::M_PREFIXES, &Self::M_WRAPPED],
        );
    }

    /// Read `body` as `Config`, returning `(status, prefixes, wrapped)`.
    fn read_config(body: &[u8]) -> (Option<String>, Vec<String>, Vec<String>) {
        let mut status = None;
        let mut prefixes = Vec::new();
        let mut wrapped = Vec::new();
        let mut d = XmlShapeDeserializer::new(body);
        d.read_struct(&ConfigSchema::SCHEMA, &mut |member, d| {
            match member.member_index() {
                Some(0) => status = Some(d.read_string(member)?),
                Some(1) => d.read_list(member, &mut |item| {
                    prefixes.push(item.read_string(member)?);
                    Ok(())
                })?,
                Some(2) => d.read_list(member, &mut |item| {
                    wrapped.push(item.read_string(member)?);
                    Ok(())
                })?,
                _ => {}
            }
            Ok(())
        })
        .expect("read_struct");
        (status, prefixes, wrapped)
    }

    /// A self-closing list item keeps its PLACE in the list.
    ///
    /// Arity, not content, is what this protects. Dropping `<Prefix/>` would shorten the list,
    /// so every later index shifts and a positional caller silently reads the wrong element —
    /// a worse failure than one item deserializing to the wrong value.
    #[test]
    fn a_self_closing_list_item_is_still_an_item() {
        let (_, prefixes, _) =
            read_config(b"<Config><Prefix>a/</Prefix><Prefix/><Prefix>c/</Prefix></Config>");
        assert_eq!(
            prefixes,
            vec!["a/".to_string(), String::new(), "c/".to_string()]
        );
    }

    /// `<Prefix/>` and `<Prefix></Prefix>` are the same document, so they must deserialize the
    /// same way. The reader delivers the second as Start-then-End and the first as one Empty,
    /// which is the entire reason the two arms can diverge.
    #[test]
    fn a_self_closing_item_matches_its_expanded_form() {
        let (_, self_closing, _) = read_config(b"<Config><Prefix/></Config>");
        let (_, expanded, _) = read_config(b"<Config><Prefix></Prefix></Config>");
        assert_eq!(self_closing, expanded);
        assert_eq!(self_closing.len(), 1);
    }

    /// The read path for a flattened list: N sibling elements repeating the
    /// member's element name must rebuild ONE list with all N items, in order.
    #[test]
    fn flattened_list_siblings_are_collected_into_one_list() {
        let (status, prefixes, _) = read_config(
            b"<Config><Prefix>a/</Prefix><Status>Enabled</Status>\
              <Prefix>b/</Prefix><Prefix>c/</Prefix></Config>",
        );
        assert_eq!(prefixes, vec!["a/", "b/", "c/"]);
        assert_eq!(
            status.as_deref(),
            Some("Enabled"),
            "an interleaved scalar member still binds"
        );
    }

    /// Negative: a flattened list absent from the document yields no items (and
    /// does not fabricate an empty entry).
    #[test]
    fn absent_flattened_list_yields_no_items() {
        let (status, prefixes, _) = read_config(b"<Config><Status>Suspended</Status></Config>");
        assert_eq!(status.as_deref(), Some("Suspended"));
        assert!(prefixes.is_empty());
    }

    #[test]
    fn single_flattened_sibling_yields_one_item() {
        let (_, prefixes, _) = read_config(b"<Config><Prefix>solo/</Prefix></Config>");
        assert_eq!(prefixes, vec!["solo/"]);
    }

    /// The wrapped layout is untouched: one wrapper holding item elements.
    #[test]
    fn wrapped_list_still_reads_from_its_wrapper() {
        let (_, prefixes, wrapped) =
            read_config(b"<Config><Wrapped><Item>x</Item><Item>y</Item></Wrapped></Config>");
        assert_eq!(wrapped, vec!["x", "y"]);
        assert!(prefixes.is_empty());
    }

    #[test]
    fn xml_list_trait_exposes_its_layout() {
        let flat = XmlListTrait::new(true, None);
        assert!(flat.flattened());
        assert_eq!(flat.item_name(), None);
        let wrapped = XmlListTrait::new(false, Some("Item"));
        assert!(!wrapped.flattened());
        assert_eq!(wrapped.item_name(), Some("Item"));
        assert_eq!(flat.trait_id(), &XmlListTrait::TRAIT_ID);
    }

    #[test]
    fn xml_map_trait_exposes_its_layout() {
        let flat = XmlMapTrait::new(true);
        assert!(flat.flattened());
        let wrapped = XmlMapTrait::new(false);
        assert!(!wrapped.flattened());
        assert_eq!(flat.trait_id(), &XmlMapTrait::TRAIT_ID);
    }

    /// Entity references in element text are RESOLVED, not passed through.
    ///
    /// The reader hands back raw content and leaves entity resolution to the caller, so a
    /// decode that skipped `escape::unescape` would put `&amp;` into the deserialized value —
    /// a wire-visible difference on every XML protocol this codec serves, and one no type
    /// error would catch.
    #[test]
    fn element_text_entities_are_unescaped() {
        let (status, _, _) = read_config(
            b"<Config><Status>a &amp; b &lt;c&gt; &quot;d&quot; &apos;e&apos;</Status></Config>",
        );
        assert_eq!(status.as_deref(), Some("a & b <c> \"d\" 'e'"));
    }

    /// Whitespace AROUND an entity is part of the value and must survive.
    ///
    /// This is the regression the reader's own trimming introduces once text is split at each
    /// reference: trimming every fragment turns `a &amp; b` into `a&b`. Whitespace is therefore
    /// handled on the assembled value, not per fragment.
    #[test]
    fn whitespace_around_an_entity_is_preserved() {
        let (status, _, _) = read_config(b"<Config><Status>Tom &amp; Jerry</Status></Config>");
        assert_eq!(status.as_deref(), Some("Tom & Jerry"));
    }

    /// Leading and trailing formatting whitespace is still not part of the value.
    #[test]
    fn surrounding_formatting_whitespace_is_not_part_of_the_value() {
        let (status, _, _) =
            read_config(b"<Config>\n  <Status>\n    hello\n  </Status>\n</Config>");
        assert_eq!(status.as_deref(), Some("hello"));
    }

    /// A numeric character reference resolves too — same path, different entity form.
    #[test]
    fn element_text_numeric_references_are_unescaped() {
        let (status, _, _) = read_config(b"<Config><Status>&#65;&#x42;</Status></Config>");
        assert_eq!(status.as_deref(), Some("AB"));
    }

    /// Text that needs no unescaping survives byte-for-byte.
    #[test]
    fn plain_element_text_is_unchanged() {
        let (status, _, _) = read_config(b"<Config><Status>plain text</Status></Config>");
        assert_eq!(status.as_deref(), Some("plain text"));
    }

    /// Entities inside LIST ITEMS survive too — a different path from the scalar one.
    ///
    /// A list item's inner XML is COPIED event by event into a fragment and re-parsed, so the
    /// reference has to be forwarded by the copy loop and then resolved by the value reader.
    /// Both halves are needed, and neither is exercised by a scalar test.
    #[test]
    fn entities_inside_list_items_survive_the_copy_and_reparse() {
        let (_, prefixes, wrapped) = read_config(
            b"<Config><Prefix>a &amp; b</Prefix><Prefix>&lt;x&gt;</Prefix>\
              <Wrapped><Item>Tom &amp; Jerry</Item></Wrapped></Config>",
        );
        assert_eq!(prefixes, vec!["a & b", "<x>"]);
        assert_eq!(wrapped, vec!["Tom & Jerry"]);
    }

    /// A non-breaking space at the edge of a value is CONTENT, not layout.
    ///
    /// XML strips space/tab/CR/LF; `str::trim` would also strip NBSP and the other Unicode
    /// whitespace, silently altering a value a client sent.
    #[test]
    fn a_non_breaking_space_at_the_edge_survives() {
        let body = "<Config><Status>\u{00A0}x\u{00A0}</Status></Config>";
        let (status, _, _) = read_config(body.as_bytes());
        assert_eq!(status.as_deref(), Some("\u{00A0}x\u{00A0}"));
    }

    /// An ASCII space the client ENCODED is content, and survives the edge trim.
    ///
    /// The companion to the NBSP case above, and the one that distinguishes trimming the
    /// assembled value from trimming its text-sourced edges: `&#32;` resolves to the same
    /// character a formatter would have indented with, so only the fragment's origin says
    /// whether it is the document's or the formatter's.
    #[test]
    fn an_encoded_space_at_the_edge_survives() {
        let body = "<Config><Status>&#32;x&#32;</Status></Config>";
        let (status, _, _) = read_config(body.as_bytes());
        assert_eq!(status.as_deref(), Some(" x "));
    }

    /// The other encoded layout characters, for the same reason.
    #[test]
    fn encoded_tab_and_newline_at_the_edges_survive() {
        let body = "<Config><Status>&#x09;x&#10;</Status></Config>";
        let (status, _, _) = read_config(body.as_bytes());
        assert_eq!(status.as_deref(), Some("\tx\n"));
    }

    /// And the layout case still works: real indentation around a value is still removed.
    ///
    /// Paired with the two above so the edge trim cannot be "fixed" by deleting it: that would
    /// satisfy them and fail this.
    #[test]
    fn literal_indentation_around_a_value_is_still_trimmed() {
        let body = "<Config><Status>\n      x\n    </Status></Config>";
        let (status, _, _) = read_config(body.as_bytes());
        assert_eq!(status.as_deref(), Some("x"));
    }

    /// A CDATA section is the value, not an empty string.
    ///
    /// It was dropped by the catch-all arm, so this deserialized to `""` — silent content
    /// loss of exactly the class this change set out to remove.
    #[test]
    fn a_cdata_section_is_the_value() {
        let body = "<Config><Status><![CDATA[foo]]></Status></Config>";
        let (status, _, _) = read_config(body.as_bytes());
        assert_eq!(status.as_deref(), Some("foo"));
    }

    /// CDATA is verbatim: its whole purpose is bytes the parser must not interpret.
    #[test]
    fn cdata_content_is_not_unescaped_or_trimmed() {
        let body = "<Config><Status><![CDATA[  a &amp; b  ]]></Status></Config>";
        let (status, _, _) = read_config(body.as_bytes());
        assert_eq!(status.as_deref(), Some("  a &amp; b  "));
    }

    /// Indentation is trimmed at an edge while an encoded space beside it survives.
    ///
    /// Both rules apply to the same value, which neither simpler rule can do.
    #[test]
    fn indentation_is_trimmed_but_an_encoded_space_inside_it_is_kept() {
        let body = "<Config><Status>  &#32;x  </Status></Config>";
        let (status, _, _) = read_config(body.as_bytes());
        assert_eq!(status.as_deref(), Some(" x"));
    }

    // A map member whose key/value sub-members carry a modeled `@xmlName`
    // ("TagKey"/"TagValue"), so the XML read path must honour the key element
    // name rather than the Smithy default "key" — symmetric with the serializer.
    static TAG_MAP_KEY: Schema = Schema::new_member(
        ShapeId::from_static("com.test#Tags$key", "com.test", "key"),
        ShapeType::String,
        "key",
        0,
    )
    .with_xml_name("TagKey");
    static TAG_MAP_VALUE: Schema = Schema::new_member(
        ShapeId::from_static("com.test#Tags$value", "com.test", "value"),
        ShapeType::String,
        "value",
        1,
    )
    .with_xml_name("TagValue");
    static TAG_MAP_MEMBER: Schema = Schema::new_member(
        ShapeId::from_static("com.test#Config$Tags", "com.test", "Tags"),
        ShapeType::Map,
        "Tags",
        0,
    )
    .with_map_members(&TAG_MAP_KEY, &TAG_MAP_VALUE);

    /// The XML read path honours a modeled `@xmlName` on the map's key element.
    /// `read_map` consumes the entry sequence directly, so the body is the
    /// `<entry>` run a map member holds. With the key renamed to `<TagKey>`, each
    /// entry reads under that name; the value element (`<TagValue>`) binds as the
    /// value regardless of name. This is symmetric with the serializer, which
    /// writes the same modeled names.
    #[test]
    fn read_map_honours_the_modeled_key_element_name() {
        let body = b"<entry><TagKey>region</TagKey><TagValue>iad</TagValue></entry>\
                     <entry><TagKey>tier</TagKey><TagValue>prod</TagValue></entry>";
        let mut d = XmlShapeDeserializer::new(body);
        let mut entries: Vec<(String, String)> = Vec::new();
        d.read_map(&TAG_MAP_MEMBER, &mut |k, v| {
            entries.push((k, v.read_string(&TAG_MAP_VALUE)?));
            Ok(())
        })
        .expect("map reads under the modeled key name");
        assert_eq!(
            entries,
            vec![
                ("region".to_string(), "iad".to_string()),
                ("tier".to_string(), "prod".to_string()),
            ]
        );
    }

    /// Negative control: with the key renamed in the schema, an entry that still
    /// uses the default `<key>` element name does NOT bind as the key — proving
    /// the read path is driven by the modeled name, not the hardcoded default.
    #[test]
    fn read_map_does_not_bind_the_default_key_name_when_renamed() {
        let body = b"<entry><key>region</key><TagValue>iad</TagValue></entry>";
        let mut d = XmlShapeDeserializer::new(body);
        let mut entries: Vec<(String, String)> = Vec::new();
        d.read_map(&TAG_MAP_MEMBER, &mut |k, v| {
            entries.push((k, v.read_string(&TAG_MAP_VALUE)?));
            Ok(())
        })
        .expect("map reads");
        // The `<key>` element is not the modeled key name "TagKey", so it does
        // not bind as the key: the single entry's key stays empty.
        assert_eq!(entries.len(), 1, "one entry is read");
        assert_eq!(
            entries[0].0,
            String::new(),
            "the default <key> element must NOT bind as the key once the key is renamed"
        );
    }

    /// Drive `Config` as a struct and return the raw result, so a rejection is
    /// observable rather than unwrapped.
    fn try_read_config(body: &[u8]) -> Result<(), SerdeError> {
        let mut d = XmlShapeDeserializer::new(body);
        d.read_struct(&ConfigSchema::SCHEMA, &mut |member, d| {
            match member.member_index() {
                Some(0) => {
                    d.read_string(member)?;
                }
                Some(1) | Some(2) => d.read_list(member, &mut |item| {
                    item.read_string(member)?;
                    Ok(())
                })?,
                _ => {}
            }
            Ok(())
        })
    }

    /// A struct nested far past the depth bound is refused with a clean error
    /// instead of recursing until the stack is exhausted.
    #[test]
    fn deeply_nested_xml_is_rejected() {
        // `Node { Node: Node }` — a struct whose one member targets the same
        // struct, so a document that nests `<Node>` without bound drives
        // `read_struct` into itself once per level.
        static NODE_MEMBER: LazyLock<Schema> = LazyLock::new(|| {
            Schema::new_member(
                ShapeId::from_static("com.test#Node$Node", "com.test", "Node"),
                ShapeType::Structure,
                "Node",
                0,
            )
        });
        static NODE: LazyLock<Schema> = LazyLock::new(|| {
            let members: &'static [&'static Schema] = Box::leak(Box::new([&*NODE_MEMBER]));
            Schema::new_struct(
                ShapeId::from_static("com.test#Node", "com.test", "Node"),
                ShapeType::Structure,
                members,
            )
        });

        // Recurse the whole way down: each matched `<Node>` is read as a `Node`
        // again, so the test consumer's depth tracks the document's.
        fn read_node(d: &mut dyn ShapeDeserializer) -> Result<(), SerdeError> {
            d.read_struct(&NODE, &mut |_member, child| read_node(child))
        }

        let depth = MAX_DEPTH + 50;
        let mut body = String::new();
        for _ in 0..depth {
            body.push_str("<Node>");
        }
        for _ in 0..depth {
            body.push_str("</Node>");
        }

        let mut d = XmlShapeDeserializer::new(body.as_bytes());
        let err =
            read_node(&mut d).expect_err("a document nested past the depth bound must be refused");
        assert!(
            format!("{err}").contains("nests deeper than the limit"),
            "unexpected error: {err}"
        );
    }

    /// A document type declaration is refused, so a document cannot define its
    /// own entities for the resolver to expand.
    #[test]
    fn a_doctype_declaration_is_rejected() {
        let body = concat!(
            "<!DOCTYPE Config [ <!ENTITY x \"expanded\"> ]>",
            "<Config><Status>&x;</Status></Config>"
        );
        let err = try_read_config(body.as_bytes()).expect_err("a DOCTYPE/DTD must be refused");
        assert!(
            format!("{err}").contains("DOCTYPE"),
            "unexpected error: {err}"
        );
    }

    /// An undefined entity reference is an error, never silently expanded to
    /// nothing or to a request-defined value.
    #[test]
    fn an_undefined_entity_reference_is_rejected() {
        let body = "<Config><Status>&unknownEntity;</Status></Config>";
        try_read_config(body.as_bytes())
            .expect_err("an undefined entity reference must be refused");
    }

    /// A predefined entity resolves to exactly its one character and nothing
    /// else, so entity resolution stays confined to the predefined set.
    #[test]
    fn a_predefined_entity_resolves_to_its_single_character() {
        let (status, _, _) = read_config(b"<Config><Status>a&amp;b</Status></Config>");
        assert_eq!(status.as_deref(), Some("a&b"));
    }

    /// The write path rejects a flattened-list element name that is not a legal
    /// XML `Name` instead of writing malformed markup. `&` has no legal place in
    /// a name, so escaping it would not make the result a valid element.
    #[test]
    fn append_element_rejects_a_name_that_is_not_a_legal_xml_name() {
        let mut out = Vec::new();
        let err = append_element(&mut out, "a<b&c", b"inner")
            .expect_err("a name carrying a metacharacter must be refused");
        assert!(
            format!("{err}").contains("not a legal XML name"),
            "unexpected error: {err}"
        );
        assert!(out.is_empty(), "nothing is written on rejection: {out:?}");
    }

    /// A legal name is still written, so the gate rejects only genuinely
    /// invalid names.
    #[test]
    fn append_element_writes_a_legal_name() {
        let mut out = Vec::new();
        append_element(&mut out, "Prefix", b"inner").expect("a legal name must be accepted");
        assert_eq!(String::from_utf8(out).expect("utf8"), "<Prefix>inner</Prefix>");
    }

    /// The read path applies the same gate: an item whose element name is not a
    /// legal XML `Name` is refused when `read_struct` would wrap it, so the read
    /// and write paths stay consistent on the same parsed name.
    #[test]
    fn read_struct_rejects_an_element_name_that_is_not_a_legal_xml_name() {
        static EMPTY: LazyLock<Schema> = LazyLock::new(|| {
            Schema::new_struct(
                ShapeId::from_static("com.test#Empty", "com.test", "Empty"),
                ShapeType::Structure,
                &[],
            )
        });
        let mut item = XmlElementDeserializer::with_depth("a<b&c", b"", 1);
        let err = item
            .read_struct(&EMPTY, &mut |_m, _d| Ok(()))
            .expect_err("an item whose name is not a legal XML name must be refused");
        assert!(
            format!("{err}").contains("not a legal XML name"),
            "unexpected error: {err}"
        );
    }

    /// A legal element name is still wrapped and read, so the read-path gate
    /// rejects only genuinely invalid names.
    #[test]
    fn read_struct_accepts_a_legal_element_name() {
        static EMPTY: LazyLock<Schema> = LazyLock::new(|| {
            Schema::new_struct(
                ShapeId::from_static("com.test#Empty", "com.test", "Empty"),
                ShapeType::Structure,
                &[],
            )
        });
        let mut item = XmlElementDeserializer::with_depth("Item", b"", 1);
        item.read_struct(&EMPTY, &mut |_m, _d| Ok(()))
            .expect("a legal element name must be accepted");
    }

    /// Proves the invariant documented on `write_map`: a non-flattened map's
    /// entry run is enclosed in the map member's own element, so the output is
    /// `<Attributes><entry><key>...</key><value>...</value></entry></Attributes>`.
    #[test]
    fn write_map_emits_wrapper_element_for_non_flattened_map() {
        // A map member without @xmlFlattened, so the entries must be wrapped.
        static ATTRS_MAP: LazyLock<Schema> = LazyLock::new(|| {
            let key = Schema::new_member(
                ShapeId::from_static("com.test#Attributes$key", "com.test", "key"),
                ShapeType::String,
                "key",
                0,
            );
            let value = Schema::new_member(
                ShapeId::from_static("com.test#Attributes$value", "com.test", "value"),
                ShapeType::String,
                "value",
                1,
            );
            Schema::new_member(
                ShapeId::from_static("com.test#Topic$Attributes", "com.test", "Attributes"),
                ShapeType::Map,
                "Attributes",
                0,
            )
            .with_map_members(Box::leak(Box::new(key)), Box::leak(Box::new(value)))
        });

        let mut serializer = XmlShapeSerializer::new(None);
        serializer
            .write_map(&ATTRS_MAP, &|s| {
                s.write_string(&ATTRS_MAP, "region")?;
                s.write_string(&ATTRS_MAP, "us-west-2")?;
                s.write_string(&ATTRS_MAP, "tier")?;
                s.write_string(&ATTRS_MAP, "prod")?;
                Ok(())
            })
            .expect("map writes");
        let xml = String::from_utf8(serializer.finish()).expect("utf8");
        // Entries must be INSIDE the wrapper: check that opening <Attributes>,
        // an entry, and closing </Attributes> sit adjacent.
        assert!(
            xml.contains("<Attributes><entry><key>region</key><value>us-west-2</value></entry>"),
            "first entry must sit directly inside the wrapper opening: {xml}"
        );
        assert!(
            xml.contains("<entry><key>tier</key><value>prod</value></entry></Attributes>"),
            "last entry must sit directly before the wrapper closing: {xml}"
        );
    }

    /// A `@xmlFlattened` map (an SQS-style `QueueAttributeMap Attributes`) emits
    /// each pair as one repeated `<Attributes>` element holding `<key>` and
    /// `<value>` directly: no outer wrapper and no `<entry>`. Emitting the
    /// wrapped form instead leaves an SDK parser one `<Attributes>` element whose
    /// keys it cannot reach, the same failure a wrongly-wrapped list produces.
    #[test]
    fn write_map_emits_repeated_member_for_flattened_map() {
        static FLAT_MAP_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
            let mut m = TraitMap::new();
            m.insert(Box::new(XmlMapTrait::new(true)));
            m
        });
        // A map member WITH @xmlFlattened, so the entries must be unwrapped.
        static ATTRS_MAP: LazyLock<Schema> = LazyLock::new(|| {
            let key = Schema::new_member(
                ShapeId::from_static("com.test#Attributes$key", "com.test", "key"),
                ShapeType::String,
                "key",
                0,
            );
            let value = Schema::new_member(
                ShapeId::from_static("com.test#Attributes$value", "com.test", "value"),
                ShapeType::String,
                "value",
                1,
            );
            Schema::new_member(
                ShapeId::from_static("com.test#Topic$Attributes", "com.test", "Attributes"),
                ShapeType::Map,
                "Attributes",
                0,
            )
            .with_map_members(Box::leak(Box::new(key)), Box::leak(Box::new(value)))
            .with_traits(&FLAT_MAP_TRAITS)
        });

        let mut serializer = XmlShapeSerializer::new(None);
        serializer
            .write_map(&ATTRS_MAP, &|s| {
                s.write_string(&ATTRS_MAP, "region")?;
                s.write_string(&ATTRS_MAP, "us-west-2")?;
                s.write_string(&ATTRS_MAP, "tier")?;
                s.write_string(&ATTRS_MAP, "prod")?;
                Ok(())
            })
            .expect("map writes");
        let xml = String::from_utf8(serializer.finish()).expect("utf8");
        // Each pair IS a repeated <Attributes> element holding key/value directly.
        assert_eq!(
            xml,
            "<Attributes><key>region</key><value>us-west-2</value></Attributes>\
             <Attributes><key>tier</key><value>prod</value></Attributes>",
            "flattened map must emit repeated member elements with no wrapper and no entry: {xml}"
        );
        // The flattened layout has neither an <entry> wrapper nor an outer wrapper.
        assert!(
            !xml.contains("<entry>"),
            "flattened map must not emit an <entry> wrapper: {xml}"
        );
    }

    /// A `@xmlFlattened` map survives a serialize-then-deserialize round trip
    /// with every pair intact.
    ///
    /// This is the honest check the write side alone cannot make: `write_map`
    /// emits the flattened `<Attributes><key/><value/></Attributes>` siblings,
    /// and `read_struct` must rebuild the full map from exactly those bytes. A
    /// read path that only understands the wrapped `<entry>` layout yields an
    /// empty map here, so request parsing for a flattened-map input would drop
    /// every pair. The map member is driven through a struct because the
    /// flattened run is only recognisable in a struct's child-element stream.
    #[test]
    fn flattened_map_round_trips_through_a_struct() {
        static FLAT_MAP_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
            let mut m = TraitMap::new();
            m.insert(Box::new(XmlMapTrait::new(true)));
            m
        });
        // An SQS-style `QueueAttributeMap Attributes`, @xmlFlattened.
        static ATTRS_KEY: Schema = Schema::new_member(
            ShapeId::from_static("com.test#Attributes$key", "com.test", "key"),
            ShapeType::String,
            "key",
            0,
        );
        static ATTRS_VALUE: Schema = Schema::new_member(
            ShapeId::from_static("com.test#Attributes$value", "com.test", "value"),
            ShapeType::String,
            "value",
            1,
        );
        static ATTRS_MAP: LazyLock<Schema> = LazyLock::new(|| {
            Schema::new_member(
                ShapeId::from_static("com.test#Queue$Attributes", "com.test", "Attributes"),
                ShapeType::Map,
                "Attributes",
                0,
            )
            .with_map_members(&ATTRS_KEY, &ATTRS_VALUE)
            .with_traits(&FLAT_MAP_TRAITS)
        });
        static QUEUE: LazyLock<Schema> = LazyLock::new(|| {
            let members: &'static [&'static Schema] =
                Box::leak(Box::new([&*ATTRS_MAP as &'static Schema]));
            Schema::new_struct(
                ShapeId::from_static("com.test#Queue", "com.test", "Queue"),
                ShapeType::Structure,
                members,
            )
        });

        // Serialize the flattened map, then wrap its sibling run in the struct's
        // root element — exactly the bytes a struct holding this member produces.
        let mut serializer = XmlShapeSerializer::new(None);
        serializer
            .write_map(&ATTRS_MAP, &|s| {
                s.write_string(&ATTRS_MAP, "VisibilityTimeout")?;
                s.write_string(&ATTRS_MAP, "30")?;
                s.write_string(&ATTRS_MAP, "DelaySeconds")?;
                s.write_string(&ATTRS_MAP, "5")?;
                Ok(())
            })
            .expect("map writes");
        let flattened = serializer.finish();
        let mut body = Vec::new();
        body.extend_from_slice(b"<Queue>");
        body.extend_from_slice(&flattened);
        body.extend_from_slice(b"</Queue>");

        // Deserialize those exact bytes back into the map.
        let mut pairs: Vec<(String, String)> = Vec::new();
        let mut d = XmlShapeDeserializer::new(&body);
        d.read_struct(&QUEUE, &mut |member, d| {
            if member.member_index() == Some(0) {
                d.read_map(member, &mut |k, v| {
                    pairs.push((k, v.read_string(&ATTRS_VALUE)?));
                    Ok(())
                })?;
            }
            Ok(())
        })
        .expect("read_struct");

        // The map survives with ALL pairs intact — not empty.
        assert_eq!(
            pairs,
            vec![
                ("VisibilityTimeout".to_string(), "30".to_string()),
                ("DelaySeconds".to_string(), "5".to_string()),
            ],
            "a flattened map must round-trip every pair, not deserialize empty"
        );
    }

    /// The read path for a flattened map: repeated `<MemberName>` siblings, each
    /// holding `<key>`/`<value>` directly (no `<entry>`), rebuild the whole map.
    /// The modeled key `@xmlName` is honoured, symmetric with the wrapped path.
    #[test]
    fn flattened_map_siblings_rebuild_the_whole_map() {
        static FLAT_MAP_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
            let mut m = TraitMap::new();
            m.insert(Box::new(XmlMapTrait::new(true)));
            m
        });
        // Key renamed to <TagKey> to prove the flattened path reads the modeled
        // key name, not the default <key>.
        static TAGS_KEY: Schema = Schema::new_member(
            ShapeId::from_static("com.test#Tags$key", "com.test", "key"),
            ShapeType::String,
            "key",
            0,
        )
        .with_xml_name("TagKey");
        static TAGS_VALUE: Schema = Schema::new_member(
            ShapeId::from_static("com.test#Tags$value", "com.test", "value"),
            ShapeType::String,
            "value",
            1,
        );
        static TAGS_MAP: LazyLock<Schema> = LazyLock::new(|| {
            Schema::new_member(
                ShapeId::from_static("com.test#Res$Tags", "com.test", "Tags"),
                ShapeType::Map,
                "Tags",
                0,
            )
            .with_map_members(&TAGS_KEY, &TAGS_VALUE)
            .with_traits(&FLAT_MAP_TRAITS)
        });
        static RES: LazyLock<Schema> = LazyLock::new(|| {
            let members: &'static [&'static Schema] =
                Box::leak(Box::new([&*TAGS_MAP as &'static Schema]));
            Schema::new_struct(
                ShapeId::from_static("com.test#Res", "com.test", "Res"),
                ShapeType::Structure,
                members,
            )
        });

        let body = b"<Res>\
            <Tags><TagKey>region</TagKey><value>iad</value></Tags>\
            <Tags><TagKey>tier</TagKey><value>prod</value></Tags>\
            </Res>";
        let mut pairs: Vec<(String, String)> = Vec::new();
        let mut d = XmlShapeDeserializer::new(body);
        d.read_struct(&RES, &mut |member, d| {
            if member.member_index() == Some(0) {
                d.read_map(member, &mut |k, v| {
                    pairs.push((k, v.read_string(&TAGS_VALUE)?));
                    Ok(())
                })?;
            }
            Ok(())
        })
        .expect("read_struct");
        assert_eq!(
            pairs,
            vec![
                ("region".to_string(), "iad".to_string()),
                ("tier".to_string(), "prod".to_string()),
            ],
            "flattened-map siblings must rebuild one map honouring the modeled key name"
        );
    }

    /// A self-closing grouping element in a flattened-map run names no pair and
    /// is dropped, not read as an empty-key entry.
    ///
    /// A flattened map has no positional arity to preserve (unlike a list), and
    /// a `<Member/>` carries no `<key>`, so admitting it would inject a spurious
    /// `("", "")` pair that any later empty element would overwrite. Only the two
    /// real pairs survive.
    #[test]
    fn a_self_closing_flattened_map_element_names_no_pair() {
        static FLAT_MAP_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
            let mut m = TraitMap::new();
            m.insert(Box::new(XmlMapTrait::new(true)));
            m
        });
        static TAGS_KEY: Schema = Schema::new_member(
            ShapeId::from_static("com.test#Tags$key", "com.test", "key"),
            ShapeType::String,
            "key",
            0,
        );
        static TAGS_VALUE: Schema = Schema::new_member(
            ShapeId::from_static("com.test#Tags$value", "com.test", "value"),
            ShapeType::String,
            "value",
            1,
        );
        static TAGS_MAP: LazyLock<Schema> = LazyLock::new(|| {
            Schema::new_member(
                ShapeId::from_static("com.test#Res$Tags", "com.test", "Tags"),
                ShapeType::Map,
                "Tags",
                0,
            )
            .with_map_members(&TAGS_KEY, &TAGS_VALUE)
            .with_traits(&FLAT_MAP_TRAITS)
        });
        static RES: LazyLock<Schema> = LazyLock::new(|| {
            let members: &'static [&'static Schema] =
                Box::leak(Box::new([&*TAGS_MAP as &'static Schema]));
            Schema::new_struct(
                ShapeId::from_static("com.test#Res", "com.test", "Res"),
                ShapeType::Structure,
                members,
            )
        });

        // A self-closing <Tags/> sits between two real pairs.
        let body = b"<Res>\
            <Tags><key>region</key><value>iad</value></Tags>\
            <Tags/>\
            <Tags><key>tier</key><value>prod</value></Tags>\
            </Res>";
        let mut pairs: Vec<(String, String)> = Vec::new();
        let mut d = XmlShapeDeserializer::new(body);
        d.read_struct(&RES, &mut |member, d| {
            if member.member_index() == Some(0) {
                d.read_map(member, &mut |k, v| {
                    pairs.push((k, v.read_string(&TAGS_VALUE)?));
                    Ok(())
                })?;
            }
            Ok(())
        })
        .expect("read_struct");
        assert_eq!(
            pairs,
            vec![
                ("region".to_string(), "iad".to_string()),
                ("tier".to_string(), "prod".to_string()),
            ],
            "a self-closing grouping element must be dropped, not read as an empty-key pair"
        );
    }

    /// Map values that are structs are emitted under the value element name
    /// (`<value>` or its `@xmlName`), not under the struct's shape name.
    ///
    /// A map<string, Server> emitting `<value><Id>...</Id></value>` rather than
    /// `<Server><Id>...</Id></Server>` inside each entry is symmetric with list
    /// items, whose element name comes from the enclosing list rather than the
    /// item's shape. Emitting under the shape name breaks SDK parsers the same
    /// way as emitting `<Object>` for a `list<Object>` instead of `<Contents>`.
    #[test]
    fn map_with_struct_value_emits_value_element_not_shape_name() {
        struct ServerSchema;
        impl ServerSchema {
            const M_ID: Schema = Schema::new_member(
                ShapeId::from_static("com.test#Server$Id", "com.test", "Id"),
                ShapeType::String,
                "Id",
                0,
            );
            const SCHEMA: Schema = Schema::new_struct(
                ShapeId::from_static("com.test#Server", "com.test", "Server"),
                ShapeType::Structure,
                &[&Self::M_ID],
            );
        }

        static SERVERS_MAP: LazyLock<Schema> = LazyLock::new(|| {
            let key = Schema::new_member(
                ShapeId::from_static("com.test#Servers$key", "com.test", "key"),
                ShapeType::String,
                "key",
                0,
            );
            let value = Schema::new_member(
                ShapeId::from_static("com.test#Servers$value", "com.test", "value"),
                ShapeType::Structure,
                "value",
                1,
            );
            Schema::new_member(
                ShapeId::from_static("com.test#Config$Servers", "com.test", "Servers"),
                ShapeType::Map,
                "Servers",
                0,
            )
            .with_map_members(Box::leak(Box::new(key)), Box::leak(Box::new(value)))
        });

        struct ServerValue {
            id: String,
        }

        impl SerializableStruct for ServerValue {
            fn serialize_members(
                &self,
                serializer: &mut dyn ShapeSerializer,
            ) -> Result<(), SerdeError> {
                serializer.write_string(&ServerSchema::M_ID, &self.id)
            }
        }

        let mut serializer = XmlShapeSerializer::new(None);
        serializer
            .write_map(&SERVERS_MAP, &|s| {
                s.write_string(&SERVERS_MAP, "web")?;
                s.write_struct(&SERVERS_MAP, &ServerValue { id: "srv-1".into() })?;
                s.write_string(&SERVERS_MAP, "db")?;
                s.write_struct(&SERVERS_MAP, &ServerValue { id: "srv-2".into() })?;
                Ok(())
            })
            .expect("map with struct values");
        let xml = String::from_utf8(serializer.finish()).expect("utf8");
        // The struct is emitted under <value>, not <Server>.
        assert!(
            xml.contains("<value><Id>srv-1</Id></value>"),
            "struct value must be emitted under <value>, not shape name: {xml}"
        );
        assert!(
            xml.contains("<value><Id>srv-2</Id></value>"),
            "second struct value too: {xml}"
        );
        assert!(
            !xml.contains("<Server>"),
            "struct shape name must NOT appear: {xml}"
        );
    }
}
