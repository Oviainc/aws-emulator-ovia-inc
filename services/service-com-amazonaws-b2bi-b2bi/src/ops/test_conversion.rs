// SPDX-License-Identifier: CC0-1.0

//! @op TestConversion
//! Serialises the B2Bi EDI document the request carries INLINE into an X12 interchange and
//! answers that interchange: "it takes an input JSON/XML in the B2Bi shape as input,
//! converts it to an X12 EDI string, and return that string".
//!
//! This file owns this service's X12 WRITER, the inverse of the X12 READER
//! `super::test_parsing` owns: every member of the document names the segment and the
//! position of the element it carries, so the interchange is rebuilt from the document's
//! own values and nothing in the answer is derived from anything else. A document this
//! service parsed out of an interchange converts back to that interchange.
//!
//! This operation owns no resource — AWS persists nothing for it — so it reads
//! [`crate::state`] only for the authorization decision every operation of this service
//! that models `AccessDeniedException` takes, and writes none of it: a call leaves what
//! every later caller sees untouched.
//!
//! ## The document that is written
//!
//! X12 is a nest of three envelopes, and the document is that nest (see
//! `super::test_parsing`, which keys it):
//!
//! ```text
//! interchanges[]                       ISA elements, then IEA elements
//!   functional_groups[]                GS elements, then GE elements
//!     transactions[]                   ST elements, segments[], then SE elements
//! ```
//!
//! An envelope segment's elements sit DIRECTLY in the object that holds the segment, keyed
//! `<SEGMENT>_<position>_<ElementName>` (`ST_01_TransactionSetIdentifierCode`); a segment
//! of a transaction's own body is one entry of that transaction's `segments` list, carrying
//! its `segment_id` and its elements keyed `<SEGMENT>_<position>` (`BEG_03`), with a
//! composite element's components keyed `<SEGMENT>_<position>_<component>`. A repeating
//! detail table arrives as ONE entry keyed `<SEGMENT>-8000_loop` holding one object per
//! occurrence, which is written back as the segments those occurrences hold, in order. Only
//! the POSITION in a member's name decides where its value goes, so a document whose
//! members arrived in any other order writes the same interchange, and an element the
//! document leaves out is written as the empty element X12 encodes an absent element as.
//!
//! The same document in XML is the same tree: each node is an element named by its key,
//! and a repeated child is a wrapper element holding one child element per repetition.
//!
//! ## The delimiters
//!
//! The X12 envelope declares two of its three delimiters BY POSITION — the element
//! separator is the character after `ISA`, the segment terminator the character after ISA16
//! — so the document carries neither and the writer uses the characters X12 senders
//! delimit with ([`ELEMENT_SEPARATOR`], [`SEGMENT_TERMINATOR`]). The third, the component
//! element separator, IS an element (ISA16), so a composite element is written with the
//! character the document's own ISA16 names.
//!
//! ## The control counts
//!
//! A trailer's first element counts what its envelope holds — segments for SE01,
//! transaction sets for GE01, functional groups for IEA01 — so each one is the count of
//! what was really written rather than the number the document carried, and its control
//! number defaults to the one its header carries, which X12 requires the two to match on.
//!
//! ## What is refused, and why
//!
//! Each of these is the modelled `ValidationException` (HTTP 400) — the request, not the
//! service, is what cannot be honoured:
//!
//! * a `source` whose `fileContent` is not the `fileFormat` it was declared as;
//! * content that is that format but is not the B2Bi EDI document shape: no `interchanges`,
//!   an interchange with no ISA, a functional group with no GS, a transaction set with no
//!   ST, a body segment with no `segment_id`, or a member whose value is not an element's;
//! * an ISA16 that cannot delimit the components of the interchange being written, or an
//!   element value carrying one of the three characters that delimit it — a delimiter is a
//!   character no element's value is made of, and the interchange would not be the one the
//!   document described;
//! * a source document larger than [`MAX_SOURCE_BYTES`], one nesting deeper than
//!   [`MAX_DOCUMENT_DEPTH`], or a member naming an element past
//!   [`MAX_ELEMENT_POSITION`] — the three factors of the work one document can drive that
//!   the modelled `fileContent` length does not bound on its own;
//! * an `advancedOptions.x12.validationOptions` rule keyed by a four-digit X12 DATA ELEMENT
//!   id, which no X12 data element dictionary here could apply (the same refusal
//!   `TestParsing` gives it, through the seam that words it once).
//!
//! `target.formatDetails` names the transaction set and version of the conversion, which
//! the document itself declares in ST01 and GS08 and which the writer therefore takes from
//! the document; `target.outputSampleFile` is a sample for a target whose structure has to
//! be inferred, and this document carries its own structure. Neither is read here, and
//! `target.advancedOptions.x12.splitOptions` splits an input file into several, which is
//! `TestParsing`'s side of an EDI request and not one string of converted output.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `TestConversion::new(engine)` and drives it through the centralized request walk:
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
//! # Modeled contract for `TestConversion`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-b2bi-b2bi/smithy-model/com-amazonaws-b2bi-b2bi.json")
//! ```
//!
//! This operation mimics the latter half of a typical Outbound EDI request. It takes an
//! input JSON/XML in the B2Bi shape as input, converts it to an X12 EDI string, and return
//! that string.
//!
//! ## Input
//!
//! * `source` (ConversionSource) **required** — Specify the source file for an outbound EDI
//!   request.
//! * `target` (ConversionTarget) **required** — Specify the format (X12 is the only currently
//!   supported format), and other details for the conversion target.
//!
//! ## Output
//!
//! * `convertedFileContent` (String) **required** — Returns the converted file content.
//! * `validationMessages` (ValidationMessages) — Returns an array of validation messages that
//!   Amazon Web Services B2B Data Interchange generates during the conversion process. These
//!   messages include both standard EDI validation results and custom validation messages
//!   when custom validation rules are configured. Custom validation messages provide detailed
//!   feedback on element length constraints, code list validations, and element requirement
//!   checks applied during the outbound EDI generation process.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient access to perform this
//!   action.
//! * `InternalServerException` (HTTP 500) — This exception is thrown when an error occurs in
//!   the Amazon Web Services B2B Data Interchange service.
//! * `ResourceNotFoundException` (HTTP 404) — Occurs when the requested resource does not
//!   exist, or cannot be found. In some cases, the resource exists in a region other than the
//!   region specified in the API call.
//! * `ThrottlingException` (HTTP 429) — The request was denied due to throttling: the data
//!   speed and rendering may be limited depending on various parameters and conditions.
//! * `ValidationException` (HTTP 400) — Occurs when a B2BI object cannot be validated against
//!   a request from another object. This exception can be thrown during standard EDI
//!   validation or when custom validation rules fail, such as when element length constraints
//!   are violated, invalid codes are used in code list validations, or required elements are
//!   missing based on configured element requirement rules.

#[allow(unused_imports)]
use super::types::*;

/// The segment ids of the three X12 envelopes, the document's member names, the count of
/// elements the interchange control header is defined to carry and what a segment
/// identifier looks like all belong to this service's X12 READER
/// (`super::test_parsing`), the module that owns what an X12 document is made of: this
/// writer is that reader's inverse, so the two cannot disagree about the name of a segment
/// or of a member.
use super::test_parsing::{
    element_requirement_rules, is_segment_id, requirement_messages_for, FUNCTIONAL_GROUPS_KEY,
    FUNCTIONAL_GROUP_ITEM, GE, GS, IEA, INTERCHANGES_KEY, INTERCHANGE_ITEM, ISA,
    ISA_ELEMENT_COUNT, LOOP_KEY_SUFFIX, SE, SEGMENTS_KEY, SEGMENT_ID_KEY, SEGMENT_ITEM, ST,
    TRANSACTIONS_KEY, TRANSACTION_ITEM,
};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `TestConversionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-b2bi-b2bi.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `TestConversionError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.b2bi#TestConversion")]
#[derive(Debug)]
pub enum TestConversionErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::test_conversion::TestConversionError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type TestConversionError = crate::scaffolded::Scaffolded<TestConversionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<TestConversionErrorModeled> for TestConversionError {
    fn from(modeled: TestConversionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// Refuse the request with the modelled `ValidationException` (HTTP 400).
fn invalid(message: String) -> TestConversionError {
    TestConversionErrorModeled::ValidationException(ValidationException { message }).into()
}

/// The refusal for a source document that is its declared format but does not convert:
/// `detail` says what about it.
fn not_converted(detail: impl ::std::fmt::Display) -> String {
    format!("The source document could not be converted to X12. {detail}")
}

/// The `ConversionTargetFormat` value naming the X12 standard — the only conversion target
/// the model carries ("X12 is the only currently supported format").
const TARGET_X12: &str = "X12";

/// The most bytes of source document one request converts: AWS's own bound for the file the
/// test operations take, "the input file has a file size limit of 250 KB"
/// (`TestConversion` / `TestParsing` documentation). It also bounds the interchange one
/// request makes this emulator hold.
const MAX_SOURCE_BYTES: usize = crate::state::MAX_EDI_OBJECT_BYTES;

/// The element separator the interchange is written with, the character X12 senders
/// delimit elements with.
///
/// The document carries no element separator of its own: the X12 envelope declares it BY
/// POSITION — it is the character that follows `ISA` — rather than as an element, so the
/// writer chooses it, and a document read out of an interchange delimited this way is
/// written back as that interchange.
const ELEMENT_SEPARATOR: char = '*';

/// The character every written segment ends with. Declared by position too (it is the
/// character after ISA16), so the writer chooses it for the same reason.
const SEGMENT_TERMINATOR: char = '~';

/// The component element separator a document that carries no ISA16 is written with. ISA16
/// IS the component separator, so a document that carries one is written with the character
/// it names ([`component_separator`]).
const DEFAULT_COMPONENT_SEPARATOR: char = '>';

/// The element position of the component element separator in the interchange control
/// header: its LAST element, ISA16.
const COMPONENT_SEPARATOR_POSITION: usize = ISA_ELEMENT_COUNT;

/// The element position of the interchange control number in the interchange control
/// header, ISA13 — the number IEA02 closes the interchange with.
const INTERCHANGE_CONTROL_POSITION: usize = 13;

/// The element position of the group control number in the functional group header, GS06 —
/// the number GE02 closes the group with.
const GROUP_CONTROL_POSITION: usize = 6;

/// The element position of the transaction set control number in the transaction set
/// header, ST02 — the number SE02 closes the transaction set with.
const TRANSACTION_CONTROL_POSITION: usize = 2;

/// The segments an envelope's trailer counts beside its body: its own header and itself
/// (SE01 is "the number of segments included in a transaction set including ST and SE
/// segments", ASC X12 transaction set trailer).
const ENVELOPE_SEGMENTS: usize = 2;

/// The most elements this emulator writes in one segment, and the most components in one
/// composite element.
///
/// The POSITION in a member's name is what decides how many elements a segment is written
/// with — every position the document leaves out before it is written as the empty element
/// X12 encodes an absent element as — so it is also what bounds that work, and one member
/// naming a far position would otherwise drive it arbitrarily. 100 is more elements than the
/// segments of the ASC X12 segment directory carry; a member past it is refused rather than
/// written.
const MAX_ELEMENT_POSITION: usize = 100;

/// The deepest nesting a source document may carry before this operation refuses it.
///
/// 64 levels: deeper than any X12 interchange document (the envelope nest and a detail loop
/// are a handful of levels), and it bounds the recursion reading and writing the document do
/// per request, which a source document could otherwise drive arbitrarily deep. Counted in
/// NODES on one root-to-leaf chain, so the same number holds for both formats — the same
/// bound `GenerateMapping` puts on the sample documents it walks.
const MAX_DOCUMENT_DEPTH: usize = 64;

/// One node of the source document, whichever of the two source formats it arrived in.
///
/// Members keep the order the document wrote them in, and a name may REPEAT: the XML
/// rendering of a repeated child is a wrapper element holding one child element per
/// repetition, all under the same name.
#[derive(Debug)]
enum Source {
    /// A leaf: one element's value, as the document carries it.
    Text(String),
    /// Named children, in document order.
    Map(Vec<(String, Source)>),
    /// A repeated child, as the JSON rendering writes it.
    List(Vec<Source>),
}

/// One X12 segment to write: its segment id, then its elements in positional order.
type Fields = Vec<String>;

/// The refusal for a document that nests deeper than this operation reads.
fn too_deep() -> String {
    format!(
        "The document nests deeper than the {MAX_DOCUMENT_DEPTH} levels this emulator \
         converts."
    )
}

/// The JSON document `value` as a source tree, `depth` being the levels already open.
///
/// A number or a boolean is written as the text it reads as: an X12 element carries
/// characters, and a document that wrote a control number unquoted still names that number.
fn from_json(value: ::serde_json::Value, depth: usize) -> Result<Source, String> {
    use ::serde_json::Value;

    if depth >= MAX_DOCUMENT_DEPTH {
        return Err(too_deep());
    }
    let node = match value {
        Value::Null => Source::Text(String::new()),
        Value::Bool(flag) => Source::Text(flag.to_string()),
        Value::Number(number) => Source::Text(number.to_string()),
        Value::String(text) => Source::Text(text),
        Value::Array(items) => Source::List(
            items
                .into_iter()
                .map(|item| from_json(item, depth + 1))
                .collect::<Result<Vec<Source>, String>>()?,
        ),
        Value::Object(members) => Source::Map(
            members
                .into_iter()
                .map(|(key, child)| from_json(child, depth + 1).map(|child| (key, child)))
                .collect::<Result<Vec<(String, Source)>, String>>()?,
        ),
    };
    Ok(node)
}

/// The XML document `content` as a source tree: each element is a node named by its tag,
/// an element with child elements is a map of them, and one without is the text it holds.
///
/// Whitespace is NOT trimmed from an element's text: a fixed-width X12 element is padded
/// with the spaces that belong to its value.
fn from_xml(content: &str) -> Result<Source, String> {
    use ::quick_xml::events::Event;

    let mut reader = ::quick_xml::Reader::from_str(content);
    reader.config_mut().check_end_names = true;

    // The elements still open, outermost first.
    let mut open: Vec<Open> = Vec::new();
    let mut roots: Vec<(String, Source)> = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Start(start)) => {
                if open.len() >= MAX_DOCUMENT_DEPTH {
                    return Err(too_deep());
                }
                let name = String::from_utf8_lossy(start.name().into_inner()).to_string();
                open.push(Open {
                    name,
                    children: Vec::new(),
                    text: String::new(),
                });
            }
            Ok(Event::Empty(start)) => {
                let name = String::from_utf8_lossy(start.name().into_inner()).to_string();
                close(
                    &mut open,
                    &mut roots,
                    Open {
                        name,
                        children: Vec::new(),
                        text: String::new(),
                    },
                );
            }
            // The two steps the vendored XML codec takes for a text value: decode the
            // bytes, then resolve the entity references `quick-xml` leaves verbatim.
            Ok(Event::Text(text)) => {
                let decoded = text
                    .xml10_content()
                    .map_err(|failure| failure.to_string())?;
                let unescaped = ::quick_xml::escape::unescape(&decoded)
                    .map_err(|failure| failure.to_string())?;
                if let Some(element) = open.last_mut() {
                    element.text.push_str(&unescaped);
                }
            }
            // CDATA holds bytes no entity resolution should touch, so it is taken verbatim.
            Ok(Event::CData(section)) => {
                let decoded = ::std::str::from_utf8(section.as_ref())
                    .map_err(|failure| failure.to_string())?;
                if let Some(element) = open.last_mut() {
                    element.text.push_str(decoded);
                }
            }
            // The reader breaks text at an entity reference and emits it on its own, so a
            // loop that ignored this would silently drop every entity from a value.
            Ok(Event::GeneralRef(reference)) => {
                let name = reference.decode().map_err(|failure| failure.to_string())?;
                let reconstructed = format!("&{name};");
                let resolved = ::quick_xml::escape::unescape(&reconstructed)
                    .map_err(|failure| failure.to_string())?;
                if let Some(element) = open.last_mut() {
                    element.text.push_str(&resolved);
                }
            }
            Ok(Event::End(_)) => {
                let Some(element) = open.pop() else {
                    return Err("an end tag closes an element no start tag opened".to_string());
                };
                close(&mut open, &mut roots, element);
            }
            Ok(_) => {}
            Err(failure) => return Err(failure.to_string()),
        }
    }
    if !open.is_empty() {
        return Err(format!(
            "the document ends with {} element(s) still open",
            open.len()
        ));
    }
    // The document's root element is the document's one member — the `interchanges` the
    // JSON rendering writes as the document's one key.
    Ok(Source::Map(roots))
}

/// One XML element being read: its tag, the child elements it has collected and the text it
/// holds.
#[derive(Debug)]
struct Open {
    name: String,
    children: Vec<(String, Source)>,
    text: String,
}

/// Add the element just closed to the element that holds it, or to the document when
/// nothing holds it.
///
/// An element with child elements is the map of them; one without is the text it holds.
fn close(open: &mut [Open], roots: &mut Vec<(String, Source)>, element: Open) {
    let Open {
        name,
        children,
        text,
    } = element;
    let node = if children.is_empty() {
        Source::Text(text)
    } else {
        Source::Map(children)
    };
    match open.last_mut() {
        Some(parent) => parent.children.push((name, node)),
        None => roots.push((name, node)),
    }
}

/// The member of `members` named `name`, or `None` when it carries no such member.
fn member<'a>(members: &'a [(String, Source)], name: &str) -> Option<&'a Source> {
    members
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}

/// The text `value` carries, or `None` when it is not a leaf.
fn leaf(value: &Source) -> Option<&str> {
    match value {
        Source::Text(text) => Some(text),
        _ => None,
    }
}

/// The repetitions `value` holds, `item` being the name each repetition takes in the XML
/// rendering.
///
/// The JSON rendering writes them as an array; the XML rendering as a wrapper element
/// holding one child element per repetition. An object that is neither — the one occurrence
/// of a child written without its wrapper — is that single repetition.
fn repetitions<'a>(value: &'a Source, item: &str) -> Vec<&'a Source> {
    match value {
        Source::List(items) => items.iter().collect(),
        Source::Map(members) if members.is_empty() => Vec::new(),
        Source::Map(members) if members.iter().all(|(key, _)| key == item) => {
            members.iter().map(|(_, child)| child).collect()
        }
        Source::Map(_) => vec![value],
        Source::Text(_) => Vec::new(),
    }
}

/// The element position the member key `key` names for the segment `id`:
/// `ISA_06_SenderId` is element 6 of ISA and `BEG_03` element 3 of BEG, so the position is
/// what follows the segment id, and the name that may follow the position is a label.
///
/// `None` for a member that names no element of that segment — the `segments` of a
/// transaction set's body, or the `<SEGMENT>-8000_loop` of its detail table.
fn element_position(id: &str, key: &str) -> Option<usize> {
    let rest = key.strip_prefix(id)?.strip_prefix('_')?;
    let digits: String = rest
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect();
    // A position is followed by the element's name or by nothing at all, so a key that
    // merely starts with digits names no element.
    let label = rest.strip_prefix(digits.as_str())?;
    if !(label.is_empty() || label.starts_with('_')) {
        return None;
    }
    digits.parse().ok().filter(|position| *position > 0)
}

/// `value` written at element `position` of `elements`, every position before it the
/// document left out written as the empty element X12 encodes an absent element as.
fn place(elements: &mut Vec<String>, position: usize, value: String) -> Result<(), String> {
    if position > MAX_ELEMENT_POSITION {
        return Err(format!(
            "The member naming element {position} names a position past the \
             {MAX_ELEMENT_POSITION} elements this emulator writes in one segment."
        ));
    }
    if elements.len() < position {
        elements.resize(position, String::new());
    }
    if let Some(slot) = elements.get_mut(position - 1) {
        *slot = value;
    }
    Ok(())
}

/// `value`, as the member `key` carries it, confirmed to be a value an X12 element can
/// hold: none of the three characters that delimit the interchange it is written into.
///
/// A delimiter is a character no element's value is made of — the rule
/// [`component_separator`] holds ISA16 to — so a value carrying one would be read back as a
/// further element, component or segment, and the interchange would not be the one the
/// document described. The document is refused rather than written that way.
fn delimiter_free(key: &str, value: &str, component: char) -> Result<(), String> {
    let Some(found) = value
        .chars()
        .find(|character| *character == ELEMENT_SEPARATOR || *character == SEGMENT_TERMINATOR || *character == component)
    else {
        return Ok(());
    };
    Err(format!(
        "The value of {key} carries {found:?}, which delimits the interchange it would be \
         written into and so is a character no X12 element's value is made of."
    ))
}

/// The elements of the segment `id` that `members` carries, in positional order.
///
/// Only the position in a member's name decides where its value goes, so the members may
/// arrive in any order; a position the document leaves out is written as the empty element
/// X12 encodes an absent element as, and the elements end at the last one the document
/// carries.
fn segment_elements(
    id: &str,
    members: &[(String, Source)],
    component: char,
) -> Result<Vec<String>, String> {
    let mut elements: Vec<String> = Vec::new();
    for (key, value) in members {
        let Some(position) = element_position(id, key) else {
            continue;
        };
        let written = match value {
            Source::Text(text) => {
                // ISA16 IS the component element separator, so the one element whose value
                // is a delimiter is the element that declares it.
                if !(id == ISA && position == COMPONENT_SEPARATOR_POSITION) {
                    delimiter_free(key, text, component)?;
                }
                text.clone()
            }
            Source::Map(components) => composite(key, components, component)?,
            Source::List(_) => {
                return Err(format!(
                    "The member {key} carries a list of values, and an X12 element carries \
                     one value or the components of one composite element."
                ))
            }
        };
        place(&mut elements, position, written)?;
    }
    Ok(elements)
}

/// One composite element's value: its components in positional order, separated by the
/// component element separator the interchange declares in ISA16.
fn composite(
    key: &str,
    components: &[(String, Source)],
    component: char,
) -> Result<String, String> {
    let mut values: Vec<String> = Vec::new();
    for (sub_key, value) in components {
        let Some(position) = element_position(key, sub_key) else {
            continue;
        };
        let Some(text) = leaf(value) else {
            return Err(format!(
                "The member {sub_key} nests a further document under a component of a \
                 composite element, and an X12 composite element carries components."
            ));
        };
        delimiter_free(sub_key, text, component)?;
        place(&mut values, position, text.to_string())?;
    }
    Ok(values.join(&component.to_string()))
}

/// One segment's fields: its id, then its elements.
fn fields(id: &str, elements: Vec<String>) -> Fields {
    let mut fields = Vec::with_capacity(elements.len() + 1);
    fields.push(id.to_string());
    fields.extend(elements);
    fields
}

/// The envelope segment `id` the object `members` holds the elements of, or `None` when it
/// holds none of them.
fn envelope_segment(
    id: &str,
    members: &[(String, Source)],
    component: char,
) -> Result<Option<Fields>, String> {
    let elements = segment_elements(id, members, component)?;
    if elements.is_empty() {
        return Ok(None);
    }
    Ok(Some(fields(id, elements)))
}

/// The value of element `position` of `segment`, or `""` when it carries no such element.
fn element_at(segment: &Fields, position: usize) -> String {
    segment.get(position).cloned().unwrap_or_default()
}

/// Element `position` of `segment` set to `value`, every element before it the document left
/// out written as the empty element X12 encodes an absent element as.
///
/// Every caller names a position the X12 envelope fixes (a trailer's control count and
/// control number, ISA16), so this one is not driven by the document; a position a MEMBER
/// names goes through [`place`], which bounds it.
fn set_element(segment: &mut Fields, position: usize, value: String) {
    if segment.len() <= position {
        segment.resize(position + 1, String::new());
    }
    if let Some(slot) = segment.get_mut(position) {
        *slot = value;
    }
}

/// The trailer segment `id` of an envelope that holds `count` of what its first element
/// counts, closing a header whose control number is `control`.
///
/// X12 defines a trailer's first element as the number of things the envelope really holds
/// and requires its control number to be the one its header opened with, so the count is
/// what was written and the control number defaults to the header's.
fn trailer(id: &str, found: Option<Fields>, count: usize, control: String) -> Fields {
    let mut trailer = found.unwrap_or_else(|| vec![id.to_string()]);
    set_element(&mut trailer, 1, count.to_string());
    if element_at(&trailer, 2).is_empty() {
        set_element(&mut trailer, 2, control);
    }
    trailer
}

/// Every segment of a `segments` list, in document order, with each detail LOOP entry
/// flattened back into the segments it holds.
fn body_segments(list: Option<&Source>, component: char) -> Result<Vec<Fields>, String> {
    let mut segments: Vec<Fields> = Vec::new();
    let Some(list) = list else {
        return Ok(segments);
    };
    for item in repetitions(list, SEGMENT_ITEM) {
        let Source::Map(members) = item else {
            return Err(format!(
                "An entry of a {SEGMENTS_KEY} list is not a segment: a segment carries its \
                 {SEGMENT_ID_KEY} and its elements."
            ));
        };
        // A loop entry holds one object per occurrence of the segment that opens the
        // transaction set's detail table, and every occurrence is a segment of the
        // transaction set's body.
        match members
            .iter()
            .find(|(key, _)| key.ends_with(LOOP_KEY_SUFFIX))
        {
            Some((_, occurrences)) => {
                for occurrence in repetitions(occurrences, SEGMENT_ITEM) {
                    segments.extend(body_segment(occurrence, component)?);
                }
            }
            None => segments.extend(body_segment(item, component)?),
        }
    }
    Ok(segments)
}

/// One body segment and the segments nested inside it: a detail loop occurrence carries the
/// segments that follow it inside that occurrence in its own `segments` list.
fn body_segment(item: &Source, component: char) -> Result<Vec<Fields>, String> {
    let Source::Map(members) = item else {
        return Err(format!(
            "An entry of a {SEGMENTS_KEY} list is not a segment: a segment carries its \
             {SEGMENT_ID_KEY} and its elements."
        ));
    };
    let Some(id) = member(members, SEGMENT_ID_KEY).and_then(leaf) else {
        return Err(format!(
            "A segment of the document carries no {SEGMENT_ID_KEY}, so the X12 segment it \
             is cannot be told."
        ));
    };
    if !is_segment_id(id) {
        return Err(format!("{id:?} is not an X12 segment identifier."));
    }
    let mut segments = vec![fields(id, segment_elements(id, members, component)?)];
    segments.extend(body_segments(member(members, SEGMENTS_KEY), component)?);
    Ok(segments)
}

/// One transaction set: ST, the segments of its body, then SE.
fn transaction_segments(transaction: &Source, component: char) -> Result<Vec<Fields>, String> {
    let Source::Map(members) = transaction else {
        return Err(format!(
            "An entry of a {TRANSACTIONS_KEY} list is not a transaction set: a transaction \
             set carries its {ST} elements and its {SEGMENTS_KEY}."
        ));
    };
    let Some(st) = envelope_segment(ST, members, component)? else {
        return Err(format!(
            "A transaction set of the document carries no transaction set header ({ST}) \
             element."
        ));
    };
    let control = element_at(&st, TRANSACTION_CONTROL_POSITION);
    let body = body_segments(member(members, SEGMENTS_KEY), component)?;
    let included = body.len() + ENVELOPE_SEGMENTS;
    let se = trailer(SE, envelope_segment(SE, members, component)?, included, control);

    let mut segments = vec![st];
    segments.extend(body);
    segments.push(se);
    Ok(segments)
}

/// One functional group: GS, the segments that sit inside it but outside every transaction
/// set, its transaction sets, then GE.
fn group_segments(group: &Source, component: char) -> Result<Vec<Fields>, String> {
    let Source::Map(members) = group else {
        return Err(format!(
            "An entry of a {FUNCTIONAL_GROUPS_KEY} list is not a functional group: a \
             functional group carries its {GS} elements and its {TRANSACTIONS_KEY}."
        ));
    };
    let Some(gs) = envelope_segment(GS, members, component)? else {
        return Err(format!(
            "A functional group of the document carries no functional group header ({GS}) \
             element."
        ));
    };
    let control = element_at(&gs, GROUP_CONTROL_POSITION);

    let mut segments = vec![gs];
    segments.extend(body_segments(member(members, SEGMENTS_KEY), component)?);
    let transactions = member(members, TRANSACTIONS_KEY)
        .map(|transactions| repetitions(transactions, TRANSACTION_ITEM))
        .unwrap_or_default();
    for transaction in &transactions {
        segments.extend(transaction_segments(transaction, component)?);
    }
    segments.push(trailer(
        GE,
        envelope_segment(GE, members, component)?,
        transactions.len(),
        control,
    ));
    Ok(segments)
}

/// The component element separator the interchange `members` holds is written with: the
/// character its own ISA16 names, else the one X12 senders use.
///
/// The three delimiters have to be distinct and none of them a character an element's value
/// is made of, or the interchange this writer wrote could not be read back as one, so an
/// ISA16 that is not a delimiter at all is refused rather than quietly replaced.
fn component_separator(members: &[(String, Source)]) -> Result<char, String> {
    let declared = members
        .iter()
        .find(|(key, _)| element_position(ISA, key) == Some(COMPONENT_SEPARATOR_POSITION))
        .and_then(|(_, value)| leaf(value))
        .and_then(|value| value.chars().next());
    let Some(component) = declared else {
        return Ok(DEFAULT_COMPONENT_SEPARATOR);
    };
    if component == ELEMENT_SEPARATOR
        || component == SEGMENT_TERMINATOR
        || component.is_ascii_alphanumeric()
        || component.is_whitespace()
    {
        return Err(format!(
            "ISA16 declares the component element separator of the interchange, and \
             {component:?} cannot be one: the interchange is written with \
             {ELEMENT_SEPARATOR:?} between its elements and {SEGMENT_TERMINATOR:?} at the \
             end of each of its segments."
        ));
    }
    Ok(component)
}

/// One interchange: ISA, the segments that sit inside it but outside every functional
/// group, its functional groups, then IEA.
fn interchange_segments(interchange: &Source) -> Result<Vec<Fields>, String> {
    let Source::Map(members) = interchange else {
        return Err(format!(
            "An entry of {INTERCHANGES_KEY} is not an interchange: an interchange carries \
             its {ISA} elements and its {FUNCTIONAL_GROUPS_KEY}."
        ));
    };
    let component = component_separator(members)?;
    let Some(mut isa) = envelope_segment(ISA, members, component)? else {
        return Err(format!(
            "An interchange of the document carries no interchange control header ({ISA}) \
             element."
        ));
    };
    // The X12 envelope declares the component element separator as ISA16 and the segment
    // terminator as the character after it, so the control header carries exactly the
    // sixteen elements it is defined to have however many of them the document named.
    set_element(&mut isa, COMPONENT_SEPARATOR_POSITION, component.to_string());
    let control = element_at(&isa, INTERCHANGE_CONTROL_POSITION);

    let mut segments = vec![isa];
    segments.extend(body_segments(member(members, SEGMENTS_KEY), component)?);
    let groups = member(members, FUNCTIONAL_GROUPS_KEY)
        .map(|groups| repetitions(groups, FUNCTIONAL_GROUP_ITEM))
        .unwrap_or_default();
    for group in &groups {
        segments.extend(group_segments(group, component)?);
    }
    segments.push(trailer(
        IEA,
        envelope_segment(IEA, members, component)?,
        groups.len(),
        control,
    ));
    Ok(segments)
}

/// The X12 interchange the B2Bi EDI document `document` holds: every interchange it
/// carries, each segment delimited the way the X12 envelope declares.
fn write_x12(document: &Source) -> Result<String, String> {
    let Source::Map(members) = document else {
        return Err(format!(
            "The document is not a B2Bi EDI document: an EDI document carries its \
             {INTERCHANGES_KEY}."
        ));
    };
    let Some(interchanges) = member(members, INTERCHANGES_KEY) else {
        return Err(format!(
            "The document carries no {INTERCHANGES_KEY} member, so it holds no interchange \
             to convert."
        ));
    };
    let items = repetitions(interchanges, INTERCHANGE_ITEM);
    if items.is_empty() {
        return Err(format!(
            "The document's {INTERCHANGES_KEY} holds no interchange to convert."
        ));
    }

    let separator = ELEMENT_SEPARATOR.to_string();
    let mut written = String::new();
    for item in items {
        for segment in interchange_segments(item)? {
            written.push_str(&segment.join(&separator));
            written.push(SEGMENT_TERMINATOR);
        }
    }
    Ok(written)
}

/// The `TestConversion` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct TestConversion<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> TestConversion<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for TestConversion<Eng>
{
    type Input = TestConversionInput;
    type Output = TestConversionOutput;
    type Error = TestConversionError;

    const NAME: &'static str = "TestConversion";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::TEST_CONVERSION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::TEST_CONVERSION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Authorization first: nothing is converted for a caller IAM refuses. A conversion
        // addresses no resource ARN, so the request is matched against the policy wildcard
        // — the same decision `TestParsing` and `TestMapping` take, which have the same
        // shape (`crate::state::authorize`).
        if let Err(denied) =
            crate::state::authorize(&*self.engine, ctx, Self::NAME, crate::state::ANY_RESOURCE)
        {
            return Err(TestConversionErrorModeled::AccessDeniedException(denied).into());
        }

        // Beyond that decision a conversion is a pure function of its request: this
        // operation owns no resource (AWS persists nothing for it — the model gives a
        // conversion no id and no Get/List), so nothing a later caller sees changes.
        use crate::state::documents;

        let InputFileSource::fileContent(content) = &input.source.input_file;

        if content.len() > MAX_SOURCE_BYTES {
            return Err(invalid(format!(
                "source.inputFile.fileContent is {} bytes, more than the \
                 {MAX_SOURCE_BYTES} bytes of source document this emulator converts in one \
                 request.",
                content.len()
            )));
        }

        // The source document must BE the format the request says it is: this is the
        // only thing the caller declares that the service can check against the bytes,
        // and content that does not parse cannot be converted into anything.
        let document = match input.source.file_format.as_str() {
            documents::JSON => {
                let parsed = documents::parse_json(content).map_err(|failure| {
                    invalid(format!(
                        "source.inputFile.fileContent is declared as {} but is not valid JSON: {}",
                        documents::JSON, failure
                    ))
                })?;
                from_json(parsed, 0).map_err(|failure| invalid(not_converted(failure)))?
            }
            documents::XML => {
                documents::check_xml(content).map_err(|failure| {
                    invalid(format!(
                        "source.inputFile.fileContent is declared as {} but is not well-formed \
                         XML: {}",
                        documents::XML, failure
                    ))
                })?;
                // The document is well-formed XML by the check above, so what is left for
                // the read to refuse is the document being more than this emulator converts.
                from_xml(content).map_err(|failure| invalid(not_converted(failure)))?
            }
            other => {
                // `ConversionSourceFormat` is `JSON | XML`, so the boundary validator
                // has already refused anything else; a value reaching here would be a
                // format this handler cannot read.
                return Err(invalid(format!(
                    "source.fileFormat: {other} is not a source document format this service \
                     can read"
                )));
            }
        };

        // `ConversionTargetFormat` carries one value, so the boundary validator has already
        // refused any other; a value reaching here would be a standard this service cannot
        // write.
        if input.target.file_format != TARGET_X12 {
            return Err(invalid(format!(
                "target.fileFormat: {} is not an EDI standard this service converts to: \
                 {TARGET_X12} is the only currently supported format.",
                input.target.file_format
            )));
        }

        let converted_file_content = write_x12(&document).map_err(|failure| invalid(not_converted(failure)))?;

        // The interchange is READ BACK before it is answered, with the same reader
        // `TestParsing` uses, so a document whose segments do not nest as an interchange (a
        // body segment that is an envelope header, say) is refused rather than answered with
        // a file that is not an interchange. The custom validation rules the request
        // configures are evaluated over that reading — "custom validation messages ...
        // applied during the outbound EDI generation process" (`validationMessages`) —
        // through the seam `TestParsing` evaluates the same rules through, so a rule reports
        // the same finding in the same words whichever operation configured it. A violated
        // rule is REPORTED, not refused: a caller testing a conversion wants the converted
        // file AND the findings in one answer.
        let configured = input
            .target
            .advanced_options
            .as_ref()
            .and_then(|advanced| advanced.x12.as_ref())
            .and_then(|x12| x12.validation_options.as_ref())
            .and_then(|options| options.validation_rules.as_deref())
            .unwrap_or_default();
        let rules = element_requirement_rules(
            configured,
            "target.advancedOptions.x12.validationOptions.validationRules",
        )
        .map_err(invalid)?;
        let messages = requirement_messages_for(&converted_file_content, &rules)
            .map_err(|failure| invalid(not_converted(failure)))?;

        Ok(TestConversionOutput {
            converted_file_content,
            validation_messages: (!messages.is_empty()).then_some(messages),
        })
    }
}
