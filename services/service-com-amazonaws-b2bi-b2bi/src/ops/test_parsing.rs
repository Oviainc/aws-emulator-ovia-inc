// SPDX-License-Identifier: CC0-1.0

//! @op TestParsing
//! Parses the EDI interchange held by the Amazon S3 object `inputFile` names and answers
//! it as a document in the requested `fileFormat`: a JSON document for `JSON`, an XML
//! document for `XML`.
//!
//! The object is READ from object storage over the in-process callback into this same
//! emulator (the framework's seam for calling another emulated service), so what is
//! parsed is the bytes a client's own `s3:PutObject` stored — nothing here is derived
//! from the request's own members, and an `inputFile` that names no stored object is the
//! modelled `ResourceNotFoundException`.
//!
//! This operation owns no resource — AWS persists nothing for it — so it reads
//! [`crate::state`] only for the authorization decision every operation of this service
//! that models `AccessDeniedException` takes, and writes none of it: a call leaves what
//! every later caller sees untouched.
//!
//! This file owns this service's X12 READER. [`parse_x12`] is the seam onto it, and
//! `CreateStarterMappingTemplate` (its sample-driven path) and `StartTransformerJob` (a
//! transformer's `inputConversion`) read a document through that seam, so no two
//! operations of this service disagree about what an EDI file holds and none of them
//! carries a second reader.
//!
//! ## The parsed document
//!
//! X12 is a nest of three envelopes, and the parsed document is that nest: an
//! interchange (ISA ... IEA), holding functional groups (GS ... GE), holding transaction
//! sets (ST ... SE), holding the transaction's own segments:
//!
//! ```text
//! interchanges[]                       ISA elements, then IEA elements
//!   functional_groups[]                GS elements, then GE elements
//!     transactions[]                   ST elements, segments[], then SE elements
//! ```
//!
//! An envelope segment's elements sit DIRECTLY in the object that holds the segment,
//! keyed `<SEGMENT>_<position>_<ElementName>` (`ST_01_TransactionSetIdentifierCode`); a
//! segment of the transaction's own body is one entry of that transaction's `segments`
//! list, carrying its `segment_id` and its elements keyed `<SEGMENT>_<position>`
//! (`BEG_03`), with a composite element's components keyed
//! `<SEGMENT>_<position>_<component>`. That is the SAME tree, with the same keys, that
//! `CreateStarterMappingTemplate` generates a mapping template for, so a template written
//! against a starter template addresses the document this operation answers with.
//!
//! The delimiters are the interchange's OWN: the element separator is the character that
//! follows `ISA`, the component separator is ISA16 and the segment terminator is the
//! character after it, exactly as the ASC X12 envelope declares them — so a document
//! delimited any other way than `*` / `>` / `~` parses the same.
//!
//! A segment that sits inside an interchange or a functional group but outside every
//! transaction set (an interchange acknowledgment, say) is one entry of THAT object's
//! `segments` list. `splitBy TRANSACTION` answers one document per transaction set, each
//! carrying the interchange and group envelope its transaction set sits in — with the
//! control counts GE01 and IEA01 reporting what that one file really holds — and not those
//! loose segments, which belong to the input file as a whole and not to any one
//! transaction set (see [`Scope`]).
//!
//! ## What is refused, and why
//!
//! Each of these is the modelled `ValidationException` (HTTP 400) — the request, not the
//! service, is what cannot be honoured:
//!
//! * content that does not begin with the interchange header of an EDI standard this
//!   emulator parses (X12's `ISA`), which is what a client is testing a document for;
//! * an envelope that does not close (an ISA without its IEA, a GS without its GE, an ST
//!   without its SE) or a trailer with no matching header: the file is not an interchange
//!   even though it starts like one;
//! * an object larger than [`MAX_INPUT_FILE_BYTES`] — "The input file has a file size
//!   limit of 250 KB" (this operation's own documentation);
//! * a split that would answer more than [`MAX_SPLIT_FILES`] files or more than
//!   [`MAX_SPLIT_OUTPUT_BYTES`] of them together, and more than
//!   [`MAX_VALIDATION_RULES`] configured validation rules: the file count, the per-file
//!   size and the rule count are factors of the work this operation does that AWS's input
//!   limit does not bound on its own, so each one carries its own bound;
//! * an object whose bytes are not UTF-8 text, which this emulator cannot read as EDI;
//! * `fileFormat` `NOT_USED`, which names no document format to answer in ("the currently
//!   supported file formats for EDI transformations are JSON and XML");
//! * an `advancedOptions.x12.validationOptions` rule keyed by a four-digit X12 DATA
//!   ELEMENT id (`codeListValidationRule`, `elementLengthValidationRule`): applying one
//!   needs the X12 data element dictionary that maps every segment position to its
//!   element id, which this emulator does not carry, and answering 200 with no message
//!   would report a validation that never ran. `elementRequirementValidationRule` is
//!   keyed by position (`"ST-01"`), so it IS applied, and its violations are the
//!   `validationMessages` the response carries.
//!
//! * `AccessDeniedException` (HTTP 403) answers an input file the read was not permitted
//!   for — the S3 read answering `AccessDenied` (and the other permission codes) is a
//!   terminal refusal of this request, not a service fault a caller should retry.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `TestParsing::new(engine)` and drives it through the centralized request walk:
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
//! # Modeled contract for `TestParsing`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-b2bi-b2bi/smithy-model/com-amazonaws-b2bi-b2bi.json")
//! ```
//!
//! Parses the input EDI (electronic data interchange) file. The input file has a file size
//! limit of 250 KB.
//!
//! ## Input
//!
//! * `advancedOptions` (AdvancedOptions) — Specifies advanced options for parsing the input
//!   EDI file. These options allow for more granular control over the parsing process,
//!   including split options for X12 files.
//! * `ediType` (EdiType) **required** — Specifies the details for the EDI standard that is
//!   being used for the transformer. Currently, only X12 is supported. X12 is a set of
//!   standards and corresponding messages that define specific business documents.
//! * `fileFormat` (FileFormat, one of: JSON | NOT_USED | XML) **required** — Specifies that
//!   the currently supported file formats for EDI transformations are JSON and XML.
//! * `inputFile` (S3Location) **required** — Specifies an S3Location object, which contains
//!   the Amazon S3 bucket and prefix for the location of the input file.
//!
//! ## Output
//!
//! * `parsedFileContent` (String) **required** — Returns the contents of the input file being
//!   tested, parsed according to the specified EDI (electronic data interchange) type.
//! * `parsedSplitFileContents` (ParsedSplitFileContentsList) — Returns an array of parsed
//!   file contents when the input file is split according to the specified split options.
//!   Each element in the array represents a separate split file's parsed content.
//! * `validationMessages` (ValidationMessages) — Returns an array of validation messages
//!   generated during EDI validation. These messages provide detailed information about
//!   validation errors, warnings, or confirmations based on the configured X12 validation
//!   rules such as element length constraints, code list validations, and element requirement
//!   checks. This field is populated when the TestParsing API validates EDI documents.
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

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `TestParsingError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-b2bi-b2bi.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `TestParsingError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.b2bi#TestParsing")]
#[derive(Debug)]
pub enum TestParsingErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::test_parsing::TestParsingError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type TestParsingError = crate::scaffolded::Scaffolded<TestParsingErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<TestParsingErrorModeled> for TestParsingError {
    fn from(modeled: TestParsingErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// AWS's documented size limit on the input file: "The input file has a file size limit
/// of 250 KB" (this operation's own documentation). A larger object is refused rather
/// than parsed, so the work one request can drive is bounded by the limit AWS publishes.
pub const MAX_INPUT_FILE_BYTES: usize = 250 * 1024;

/// How many characters of the document the delimiter scan reads while locating the end of
/// the interchange control header.
///
/// The ASC X12 envelope's ISA segment is 106 characters with fixed-width elements; twice
/// that is read before giving up, so a sender that pads an element wider is still read
/// while a file that merely starts with `ISA` cannot make the scan walk the whole 250 KB.
const ISA_SCAN_CHARS: usize = 212;

/// The first X12 release whose ISA11 is the Repetition Separator; in the releases before
/// it ISA11 is the Interchange Control Standards Identifier (ASC X12 interchange control
/// header, element ISA11), so the element's name follows the interchange's release.
const FIRST_REPETITION_SEPARATOR_RELEASE: u32 = 4020;

/// The number of elements the ASC X12 interchange control header (ISA) carries.
const ISA_ELEMENT_COUNT: usize = 16;

/// How many characters of an unparsable file a refusal quotes back: the three an
/// interchange identifier occupies (`ISA`), which is what the check for a supported EDI
/// standard reads, so the caller sees what was rejected without the message carrying the
/// document.
const REFUSAL_EXCERPT_CHARS: usize = 3;

/// The longest message this handler puts in a modelled error, the maximum of the modelled
/// `ErrorMessage` (`len 10..1024`).
const MAX_ERROR_MESSAGE_CHARS: usize = 1024;

/// The member a parsed body segment carries its X12 segment id in, so a segment that
/// carries no element at all is still identified by the document.
///
/// Shared with `CreateStarterMappingTemplate`, whose generated template describes the very
/// document this reader builds.
pub(crate) const SEGMENT_ID_KEY: &str = "segment_id";

/// The most split files one request is answered with.
///
/// The number of transaction sets and the size of each one are SEPARATE factors of the
/// split output, and AWS's input limit bounds only their sum, so a 250 KB file of minimal
/// transaction sets would otherwise be answered with a multiple of its own size: each
/// split file repeats the interchange and group envelope its transaction set sits in. This
/// bounds the count; [`MAX_SPLIT_OUTPUT_BYTES`] bounds what they hold together. A request
/// past either is refused with the modelled `ValidationException` rather than answered.
pub const MAX_SPLIT_FILES: usize = 1_000;

/// The most bytes the split files of one request hold together — sixteen times the input
/// limit, which leaves room for the key-per-element document a 250 KB interchange renders
/// to while keeping one request's resident output bounded. See [`MAX_SPLIT_FILES`].
pub const MAX_SPLIT_OUTPUT_BYTES: usize = 16 * MAX_INPUT_FILE_BYTES;

/// The most validation rules one request configures.
///
/// Each rule is evaluated over every segment of the document, so the work and the messages
/// are the product of the rule count and the segment count: the rule list is the factor
/// the request chooses, and it is bounded here. A request past it is refused with the
/// modelled `ValidationException`.
pub const MAX_VALIDATION_RULES: usize = 100;

/// The most `validationMessages` one response carries. Past it the response reports how
/// many further violations it is not listing, rather than growing without a bound.
pub const MAX_VALIDATION_MESSAGES: usize = 100;

/// The `X12SplitBy` value that splits the input file into one file per transaction set.
const SPLIT_BY_TRANSACTION: &str = "TRANSACTION";

/// The `X12SplitBy` value that splits nothing — the default when no split options are
/// given.
const SPLIT_BY_NONE: &str = "NONE";

/// The `ElementRequirement` value that makes an element required.
const REQUIREMENT_MANDATORY: &str = "MANDATORY";

/// The element names of the interchange control header (ISA), in positional order.
///
/// Source: the element table of the ASC X12 interchange control header. Position 11 is
/// release-dependent — see [`isa_element_names`]. These are the same envelope element
/// names `CreateStarterMappingTemplate` keys a starter template's elements by, so a
/// parsed document and a starter template describe the same tree with the same keys; that
/// operation's table is private to its own file, which this change does not edit.
pub(crate) const ISA_ELEMENTS: &[&str] = &[
    "AuthorizationInformationQualifier",
    "AuthorizationInformation",
    "SecurityInformationQualifier",
    "SecurityInformation",
    "InterchangeIdQualifier",
    "InterchangeSenderId",
    "InterchangeIdQualifier",
    "InterchangeReceiverId",
    "InterchangeDate",
    "InterchangeTime",
    // Position 11, replaced per release by `isa_element_names`.
    "RepetitionSeparator",
    "InterchangeControlVersionNumber",
    "InterchangeControlNumber",
    "AcknowledgmentRequested",
    "UsageIndicator",
    "ComponentElementSeparator",
];

/// Element 11 of the interchange control header from X12 release 004020 on.
const ISA11_FROM_4020: &str = "RepetitionSeparator";

/// Element 11 of the interchange control header before X12 release 004020.
const ISA11_PRE_4020: &str = "InterchangeControlStandardsIdentifier";

/// The element names of the interchange control trailer (IEA), in positional order.
pub(crate) const IEA_ELEMENTS: &[&str] =
    &["NumberOfIncludedFunctionalGroups", "InterchangeControlNumber"];

/// The element names of the functional group header (GS), in positional order.
pub(crate) const GS_ELEMENTS: &[&str] = &[
    "FunctionalIdentifierCode",
    "ApplicationSendersCode",
    "ApplicationReceiversCode",
    "Date",
    "Time",
    "GroupControlNumber",
    "ResponsibleAgencyCode",
    "VersionReleaseIndustryIdentifierCode",
];

/// The element names of the functional group trailer (GE), in positional order.
pub(crate) const GE_ELEMENTS: &[&str] = &["NumberOfTransactionSetsIncluded", "GroupControlNumber"];

/// The element names of the transaction set header (ST), in positional order.
pub(crate) const ST_ELEMENTS: &[&str] = &[
    "TransactionSetIdentifierCode",
    "TransactionSetControlNumber",
    "ImplementationConventionReference",
];

/// The element names of the transaction set trailer (SE), in positional order.
pub(crate) const SE_ELEMENTS: &[&str] = &["NumberOfIncludedSegments", "TransactionSetControlNumber"];

/// The X12 segment ids the three envelopes are opened and closed with.
const ISA: &str = "ISA";
const IEA: &str = "IEA";
const GS: &str = "GS";
const GE: &str = "GE";
const ST: &str = "ST";
const SE: &str = "SE";

/// The key the interchanges of a document sit under, and the element name each repetition
/// takes in the XML rendering.
///
/// These are the document's own member names, shared with
/// `CreateStarterMappingTemplate`: a starter template describes the very document this
/// reader builds, so the two cannot name its members differently.
pub(crate) const INTERCHANGES_KEY: &str = "interchanges";
pub(crate) const INTERCHANGE_ITEM: &str = "interchange";
/// The same pair for the functional groups inside an interchange.
pub(crate) const FUNCTIONAL_GROUPS_KEY: &str = "functional_groups";
pub(crate) const FUNCTIONAL_GROUP_ITEM: &str = "functional_group";
/// …for the transaction sets inside a functional group.
pub(crate) const TRANSACTIONS_KEY: &str = "transactions";
pub(crate) const TRANSACTION_ITEM: &str = "transaction";
/// …and for the body segments of a transaction set.
pub(crate) const SEGMENTS_KEY: &str = "segments";
pub(crate) const SEGMENT_ITEM: &str = "segment";

/// [`ISA_ELEMENTS`] with position 11 named for `release`.
pub(crate) fn isa_element_names(release: u32) -> Vec<&'static str> {
    let mut names = ISA_ELEMENTS.to_vec();
    names[10] = if release < FIRST_REPETITION_SEPARATOR_RELEASE {
        ISA11_PRE_4020
    } else {
        ISA11_FROM_4020
    };
    names
}

/// One node of a parsed document's tree, rendered to JSON for `fileFormat` `JSON` and to
/// XML for `XML`.
///
/// A map keeps its members in INSERTION order — the positional order of an X12 segment's
/// elements — which a `serde_json::Value` (a sorted map) would lose.
///
/// Visible to the crate because [`parse_x12`] hands this tree to the two other operations
/// that read an EDI document (see that function).
pub(crate) enum Node {
    /// A leaf: one element's value, as the interchange carries it.
    Text(String),
    /// Named children, in order.
    Map(Vec<(String, Node)>),
    /// A repeated child — an interchange, a functional group, a transaction set, a
    /// segment. `item` is the element name each repetition takes in the XML rendering;
    /// JSON renders the repetitions as an array.
    List { item: String, items: Vec<Node> },
}

impl ::serde::Serialize for Node {
    fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Node::Text(text) => serializer.serialize_str(text),
            Node::Map(members) => {
                serializer.collect_map(members.iter().map(|(key, child)| (key.as_str(), child)))
            }
            Node::List { items, .. } => serializer.collect_seq(items.iter()),
        }
    }
}

/// One X12 segment as the interchange carries it: its segment id and its elements, in
/// positional order, each element's value verbatim.
#[derive(Debug, Clone)]
struct Segment {
    id: String,
    elements: Vec<String>,
}

impl Segment {
    /// The value of element `position` (1-based), or `""` when the segment carries no
    /// such element. An element that is PRESENT but empty is the X12 encoding of an
    /// absent element, so both read the same way here.
    fn element(&self, position: usize) -> &str {
        position
            .checked_sub(1)
            .and_then(|index| self.elements.get(index))
            .map(String::as_str)
            .unwrap_or_default()
    }

    /// This segment with element 1 replaced by `count` — the control count a SPLIT
    /// envelope carries, since the split file holds `count` of what the original counted.
    /// A trailer carrying no element at all is left alone.
    fn with_count(&self, count: usize) -> Segment {
        let mut split = self.clone();
        if let Some(first) = split.elements.first_mut() {
            *first = count.to_string();
        }
        split
    }
}

/// One transaction set: ST, the segments of its body, and SE.
#[derive(Debug, Clone)]
struct Transaction {
    st: Segment,
    body: Vec<Segment>,
    se: Segment,
}

/// One functional group: GS, its transaction sets, and GE. `loose` holds the segments
/// that sit inside the group but outside every transaction set.
#[derive(Debug, Clone)]
struct Group {
    gs: Segment,
    loose: Vec<Segment>,
    transactions: Vec<Transaction>,
    ge: Segment,
}

/// One interchange: ISA, its functional groups, and IEA. `loose` holds the segments that
/// sit inside the interchange but outside every functional group — an interchange
/// acknowledgment (TA1), say.
#[derive(Debug, Clone)]
struct Interchange {
    isa: Segment,
    loose: Vec<Segment>,
    groups: Vec<Group>,
    iea: Segment,
}

/// The delimiters an X12 interchange declares in its own interchange control header.
#[derive(Debug, Clone, Copy)]
struct Delimiters {
    /// The character between two elements of a segment: the character that follows `ISA`.
    element: char,
    /// The character between two components of a composite element: ISA16.
    component: char,
    /// The character that ends a segment: the character after ISA16.
    segment: char,
}

/// The refusal for content that does not begin with the interchange header of an EDI
/// standard this emulator parses.
///
/// Quotes back the leading characters it read, so a caller testing a document learns
/// which file was rejected and not merely that one was.
fn unsupported_standard(content: &str) -> String {
    let excerpt: String = content.chars().take(REFUSAL_EXCERPT_CHARS).collect();
    format!(
        "The EDI file could not be parsed. No supported EDI standard interchange begins \
         with {excerpt}"
    )
}

/// The refusal for an interchange whose structure is not one: `detail` says what about it.
fn unparsable(detail: impl ::std::fmt::Display) -> String {
    format!("The EDI file could not be parsed. {detail}")
}

/// `message` held inside the modelled `ErrorMessage` length bound.
fn bounded_message(message: String) -> String {
    if message.chars().count() <= MAX_ERROR_MESSAGE_CHARS {
        return message;
    }
    message.chars().take(MAX_ERROR_MESSAGE_CHARS).collect()
}

/// The modelled `ValidationException` (HTTP 400): the REQUEST cannot be honoured.
fn validation(message: impl Into<String>) -> TestParsingError {
    TestParsingErrorModeled::ValidationException(ValidationException {
        message: bounded_message(message.into()),
    })
    .into()
}

/// The modelled `ResourceNotFoundException` (HTTP 404) for an `inputFile` that names no
/// stored object — "occurs when the requested resource does not exist, or cannot be
/// found" — worded as this service words a missing object
/// ([`crate::state::s3_object_not_found`]).
fn not_found(bucket: &str, key: &str) -> TestParsingError {
    TestParsingErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
        message: bounded_message(crate::state::s3_object_not_found(bucket, key)),
    })
    .into()
}

/// The modelled `InternalServerException` (HTTP 500) for a read or a rendering this
/// service could not complete — not a statement about the request.
fn internal(detail: impl ::std::fmt::Display) -> TestParsingError {
    TestParsingErrorModeled::InternalServerException(InternalServerException {
        message: bounded_message(format!("The input file could not be parsed: {detail}")),
        retry_after_seconds: None,
    })
    .into()
}

/// The modelled `AccessDeniedException` (HTTP 403) for an input file this caller is not
/// permitted to read — "you do not have sufficient access to perform this action".
fn access_denied(bucket: &str, key: &str, code: &str) -> TestParsingError {
    TestParsingErrorModeled::AccessDeniedException(AccessDeniedException {
        message: bounded_message(format!(
            "The input file s3://{bucket}/{key} could not be read: {code}"
        )),
    })
    .into()
}

/// The delimiters `content` declares in its interchange control header.
///
/// The X12 envelope is self-delimiting: `ISA` is followed by the element separator, the
/// header's sixteenth element is the component separator, and the character after it ends
/// the segment. None of the three may be a character an element's value is made of, which
/// is what tells a file that merely starts with the letters `ISA` from an interchange.
fn detect_delimiters(content: &str) -> Result<Delimiters, String> {
    if !content.starts_with(ISA) {
        return Err(unsupported_standard(content));
    }
    let header: Vec<char> = content.chars().take(ISA_SCAN_CHARS).collect();
    let Some(&element) = header.get(ISA.len()) else {
        return Err(unsupported_standard(content));
    };
    if element.is_ascii_alphanumeric() || element.is_whitespace() {
        return Err(unsupported_standard(content));
    }

    let mut separators = 0usize;
    let mut component_at = None;
    for (index, character) in header.iter().enumerate().skip(ISA.len()) {
        if *character == element {
            separators += 1;
            if separators == ISA_ELEMENT_COUNT {
                component_at = Some(index + 1);
                break;
            }
        }
    }
    let Some(component_at) = component_at else {
        return Err(unparsable(format!(
            "The interchange control header (ISA) does not carry the {ISA_ELEMENT_COUNT} \
             elements the X12 envelope requires."
        )));
    };
    let (Some(&component), Some(&segment)) =
        (header.get(component_at), header.get(component_at + 1))
    else {
        return Err(unparsable(
            "The interchange control header (ISA) ends before its component element \
             separator and segment terminator.",
        ));
    };
    // The segment terminator is the one delimiter whitespace is accepted for: a line
    // break is what many real senders end a segment with, and the X12 standard puts no
    // character class on it beyond its not being one an element's value is made of.
    if component.is_ascii_alphanumeric()
        || component.is_whitespace()
        || segment.is_ascii_alphanumeric()
        || component == element
        || segment == element
        || segment == component
    {
        return Err(unparsable(
            "The interchange control header (ISA) does not declare a distinct component \
             element separator and segment terminator.",
        ));
    }
    Ok(Delimiters {
        element,
        component,
        segment,
    })
}

/// Every segment of `content`, in document order, split on the delimiters the interchange
/// declared.
///
/// A segment terminator may be followed by line breaks (and a file may end with one), so
/// a piece is trimmed of leading whitespace and trailing line breaks — never of the
/// trailing SPACES an X12 fixed-width element is padded with, which belong to the value.
///
/// A segment id is alphanumeric and STARTS WITH A LETTER, as every id in the X12 segment
/// directory does. That is also what keeps the keys built from it legal XML element names,
/// so an `XML` parse cannot answer a success a conforming parser would reject.
fn split_segments(content: &str, delimiters: &Delimiters) -> Result<Vec<Segment>, String> {
    let mut segments: Vec<Segment> = Vec::new();
    for raw in content.split(delimiters.segment) {
        let piece = raw.trim_start().trim_end_matches(['\r', '\n']);
        if piece.is_empty() {
            continue;
        }
        let mut fields = piece.split(delimiters.element);
        let id = fields.next().unwrap_or_default().trim().to_string();
        let shaped = id
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic())
            && id.chars().all(|character| character.is_ascii_alphanumeric());
        if !shaped {
            return Err(unparsable(format!(
                "{id:?} is not an X12 segment identifier."
            )));
        }
        segments.push(Segment {
            id,
            elements: fields.map(str::to_string).collect(),
        });
    }
    Ok(segments)
}

/// The interchanges `segments` holds, each with its functional groups and transaction
/// sets.
///
/// Every envelope must be closed by its own trailer and every trailer must close an
/// envelope that is open: a file whose envelopes do not nest is not an interchange, which
/// is the whole of what this operation is called to find out.
fn parse_interchanges(segments: Vec<Segment>) -> Result<Vec<Interchange>, String> {
    let mut interchanges: Vec<Interchange> = Vec::new();
    // The envelopes currently open, innermost last: ISA, then GS, then ST.
    let mut isa: Option<(Segment, Vec<Segment>, Vec<Group>)> = None;
    let mut gs: Option<(Segment, Vec<Segment>, Vec<Transaction>)> = None;
    let mut st: Option<(Segment, Vec<Segment>)> = None;

    for segment in segments {
        match segment.id.as_str() {
            ISA => {
                if isa.is_some() {
                    return Err(unparsable(
                        "An interchange control header (ISA) opens inside an interchange \
                         that was never closed by its trailer (IEA).",
                    ));
                }
                isa = Some((segment, Vec::new(), Vec::new()));
            }
            IEA => {
                if st.is_some() || gs.is_some() {
                    return Err(unparsable(
                        "An interchange control trailer (IEA) closes an interchange whose \
                         functional group or transaction set is still open.",
                    ));
                }
                let Some((header, loose, groups)) = isa.take() else {
                    return Err(unparsable(
                        "An interchange control trailer (IEA) closes an interchange no \
                         header (ISA) opened.",
                    ));
                };
                interchanges.push(Interchange {
                    isa: header,
                    loose,
                    groups,
                    iea: segment,
                });
            }
            GS => {
                if isa.is_none() {
                    return Err(unparsable(
                        "A functional group header (GS) opens outside any interchange.",
                    ));
                }
                if gs.is_some() {
                    return Err(unparsable(
                        "A functional group header (GS) opens inside a functional group \
                         that was never closed by its trailer (GE).",
                    ));
                }
                gs = Some((segment, Vec::new(), Vec::new()));
            }
            GE => {
                if st.is_some() {
                    return Err(unparsable(
                        "A functional group trailer (GE) closes a functional group whose \
                         transaction set is still open.",
                    ));
                }
                let Some((header, loose, transactions)) = gs.take() else {
                    return Err(unparsable(
                        "A functional group trailer (GE) closes a functional group no \
                         header (GS) opened.",
                    ));
                };
                let Some((_, _, groups)) = isa.as_mut() else {
                    return Err(unparsable(
                        "A functional group closes outside any interchange.",
                    ));
                };
                groups.push(Group {
                    gs: header,
                    loose,
                    transactions,
                    ge: segment,
                });
            }
            ST => {
                if gs.is_none() {
                    return Err(unparsable(
                        "A transaction set header (ST) opens outside any functional group.",
                    ));
                }
                if st.is_some() {
                    return Err(unparsable(
                        "A transaction set header (ST) opens inside a transaction set that \
                         was never closed by its trailer (SE).",
                    ));
                }
                st = Some((segment, Vec::new()));
            }
            SE => {
                let Some((header, body)) = st.take() else {
                    return Err(unparsable(
                        "A transaction set trailer (SE) closes a transaction set no header \
                         (ST) opened.",
                    ));
                };
                let Some((_, _, transactions)) = gs.as_mut() else {
                    return Err(unparsable(
                        "A transaction set closes outside any functional group.",
                    ));
                };
                transactions.push(Transaction {
                    st: header,
                    body,
                    se: segment,
                });
            }
            _ => {
                if let Some((_, body)) = st.as_mut() {
                    body.push(segment);
                } else if let Some((_, loose, _)) = gs.as_mut() {
                    loose.push(segment);
                } else if let Some((_, loose, _)) = isa.as_mut() {
                    loose.push(segment);
                } else {
                    return Err(unparsable(format!(
                        "The segment {} appears outside any interchange.",
                        segment.id
                    )));
                }
            }
        }
    }

    if st.is_some() {
        return Err(unparsable(
            "A transaction set is not closed by its trailer (SE).",
        ));
    }
    if gs.is_some() {
        return Err(unparsable(
            "A functional group is not closed by its trailer (GE).",
        ));
    }
    if isa.is_some() {
        return Err(unparsable(
            "The interchange is not closed by its trailer (IEA).",
        ));
    }
    if interchanges.is_empty() {
        return Err(unparsable("The file holds no interchange."));
    }
    Ok(interchanges)
}

/// The X12 release the request's `ediType` names, as a number — `VERSION_4010` is 4010 —
/// or `None` when it names none.
fn requested_release(details: &X12Details) -> Option<u32> {
    let value = details.version.as_deref()?;
    let rest = value.strip_prefix("VERSION_")?;
    let release = rest.strip_suffix("_HIPAA").unwrap_or(rest);
    release.parse().ok()
}

/// The X12 release the interchange itself declares in ISA12, the Interchange Control
/// Version Number: a five-digit code whose release is that code with its trailing release
/// digit — `00401` is release 004010. `None` for an ISA12 that is not such a code.
fn declared_release(isa: &Segment) -> Option<u32> {
    let value = isa.element(12).trim();
    if value.len() != 5 || !value.chars().all(|digit| digit.is_ascii_digit()) {
        return None;
    }
    value.parse::<u32>().ok().map(|code| code * 10)
}

/// An envelope segment's elements as named members, in positional order.
///
/// An element the standard's table does not name — one past the table, which a later X12
/// release may add — keeps the segment and position key without a name, so the document
/// still carries its value.
fn envelope_members(segment: &Segment, names: &[&str]) -> Vec<(String, Node)> {
    segment
        .elements
        .iter()
        .enumerate()
        .filter(|(_, value)| !value.is_empty())
        .map(|(index, value)| {
            let position = index + 1;
            let key = match names.get(index) {
                Some(name) => format!("{}_{position:02}_{name}", segment.id),
                None => format!("{}_{position:02}", segment.id),
            };
            (key, Node::Text(value.clone()))
        })
        .collect()
}

/// One segment of a transaction's body: its `segment_id`, then its elements keyed by
/// segment and position, a composite element's components keyed by segment, position and
/// component.
///
/// An element that is present but EMPTY is the X12 encoding of an absent element, so it
/// is left out rather than carried as an empty value.
fn body_segment_node(segment: &Segment, delimiters: &Delimiters) -> Node {
    let mut members = vec![(
        SEGMENT_ID_KEY.to_string(),
        Node::Text(segment.id.clone()),
    )];
    for (index, value) in segment.elements.iter().enumerate() {
        if value.is_empty() {
            continue;
        }
        let position = index + 1;
        let key = format!("{}_{position:02}", segment.id);
        if value.contains(delimiters.component) {
            let components: Vec<(String, Node)> = value
                .split(delimiters.component)
                .enumerate()
                .filter(|(_, component)| !component.is_empty())
                .map(|(sub, component)| {
                    let sub_position = sub + 1;
                    (
                        format!("{key}_{sub_position:02}"),
                        Node::Text(component.to_string()),
                    )
                })
                .collect();
            members.push((key, Node::Map(components)));
        } else {
            members.push((key, Node::Text(value.clone())));
        }
    }
    Node::Map(members)
}

/// `segments` as a named list of body segments, or nothing when there are none.
fn body_segments_member(
    name: &str,
    segments: &[Segment],
    delimiters: &Delimiters,
) -> Option<(String, Node)> {
    if segments.is_empty() {
        return None;
    }
    Some((
        name.to_string(),
        Node::List {
            item: SEGMENT_ITEM.to_string(),
            items: segments
                .iter()
                .map(|segment| body_segment_node(segment, delimiters))
                .collect(),
        },
    ))
}

/// Whether a rendering is of the WHOLE input file or of ONE split file.
///
/// A split file holds ONE transaction set, so the segments that sit inside the enclosing
/// interchange or functional group but outside every transaction set belong to the input
/// file as a whole and not to any one transaction: the whole-file rendering carries them
/// and a split file does not. That also keeps the split output LINEAR in the input, rather
/// than repeating every such segment once per transaction set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    WholeFile,
    SplitFile,
}

impl Scope {
    /// The segments outside every transaction set that this scope carries.
    fn loose(self, loose: &[Segment]) -> &[Segment] {
        match self {
            Self::WholeFile => loose,
            Self::SplitFile => &[],
        }
    }
}

/// One transaction set: its ST elements, the segments of its body, then its SE elements.
fn transaction_node(transaction: &Transaction, delimiters: &Delimiters) -> Node {
    let mut members = envelope_members(&transaction.st, ST_ELEMENTS);
    members.extend(body_segments_member(
        SEGMENTS_KEY,
        &transaction.body,
        delimiters,
    ));
    members.extend(envelope_members(&transaction.se, SE_ELEMENTS));
    Node::Map(members)
}

/// One functional group holding `transactions`: its GS elements, the segments that sit
/// inside it but outside every transaction set, its transaction sets, then its GE
/// elements.
///
/// `ge` is the group's own trailer unless the rendering holds FEWER transaction sets than
/// the group does (a split file), in which case GE01 — the number of transaction sets
/// included — is the number this rendering really holds.
fn group_node(
    group: &Group,
    transactions: &[&Transaction],
    delimiters: &Delimiters,
    scope: Scope,
) -> Node {
    let mut members = envelope_members(&group.gs, GS_ELEMENTS);
    members.extend(body_segments_member(
        SEGMENTS_KEY,
        scope.loose(&group.loose),
        delimiters,
    ));
    members.push((
        TRANSACTIONS_KEY.to_string(),
        Node::List {
            item: TRANSACTION_ITEM.to_string(),
            items: transactions
                .iter()
                .map(|transaction| transaction_node(transaction, delimiters))
                .collect(),
        },
    ));
    let ge = if transactions.len() == group.transactions.len() {
        group.ge.clone()
    } else {
        group.ge.with_count(transactions.len())
    };
    members.extend(envelope_members(&ge, GE_ELEMENTS));
    Node::Map(members)
}

/// One interchange holding `groups`: its ISA elements, the segments that sit inside it but
/// outside every functional group, its functional groups, then its IEA elements.
///
/// IEA01 — the number of functional groups included — follows the same rule as GE01 in
/// [`group_node`]: the interchange's own value unless this rendering holds fewer groups.
fn interchange_node(
    interchange: &Interchange,
    groups: &[(&Group, Vec<&Transaction>)],
    delimiters: &Delimiters,
    release: u32,
    scope: Scope,
) -> Node {
    let names = isa_element_names(release);
    let mut members = envelope_members(&interchange.isa, &names);
    members.extend(body_segments_member(
        SEGMENTS_KEY,
        scope.loose(&interchange.loose),
        delimiters,
    ));
    members.push((
        FUNCTIONAL_GROUPS_KEY.to_string(),
        Node::List {
            item: FUNCTIONAL_GROUP_ITEM.to_string(),
            items: groups
                .iter()
                .map(|(group, transactions)| group_node(group, transactions, delimiters, scope))
                .collect(),
        },
    ));
    let iea = if groups.len() == interchange.groups.len() {
        interchange.iea.clone()
    } else {
        interchange.iea.with_count(groups.len())
    };
    members.extend(envelope_members(&iea, IEA_ELEMENTS));
    Node::Map(members)
}

/// The document tree for `interchanges`: every interchange, every group and every
/// transaction set the input file holds.
fn document_node(interchanges: &[Interchange], delimiters: &Delimiters, release: u32) -> Node {
    let items: Vec<Node> = interchanges
        .iter()
        .map(|interchange| {
            let groups: Vec<(&Group, Vec<&Transaction>)> = interchange
                .groups
                .iter()
                .map(|group| (group, group.transactions.iter().collect()))
                .collect();
            interchange_node(interchange, &groups, delimiters, release, Scope::WholeFile)
        })
        .collect();
    Node::Map(vec![(
        INTERCHANGES_KEY.to_string(),
        Node::List {
            item: INTERCHANGE_ITEM.to_string(),
            items,
        },
    )])
}

/// The parsed document the X12 interchange `content` holds, as this operation answers it
/// for the whole input file.
///
/// This is the SEAM onto this service's one X12 reader: `CreateStarterMappingTemplate`
/// (its sample-driven path) and `StartTransformerJob` (a transformer's `inputConversion`)
/// read a document through it too, so no two operations of this service disagree about
/// what an EDI file holds, and none of them carries a second reader. The error is the
/// reason the content is not an interchange, worded as [`unsupported_standard`] and
/// [`unparsable`] word it, so a caller reads the same sentence whichever operation it
/// reached — each operation puts it in the error ITS model declares.
///
/// The element names follow the release the interchange itself declares in ISA12; a
/// caller that names a release (this operation's `ediType`) resolves it before building
/// the tree, which is why this entry takes none.
pub(crate) fn parse_x12(content: &str) -> Result<Node, String> {
    // Leading whitespace is file formatting, not part of the interchange.
    let content = content.trim_start();
    let delimiters = detect_delimiters(content)?;
    let segments = split_segments(content, &delimiters)?;
    let interchanges = parse_interchanges(segments)?;
    let release = interchanges
        .first()
        .and_then(|interchange| declared_release(&interchange.isa))
        .unwrap_or_default();
    Ok(document_node(&interchanges, &delimiters, release))
}

/// The document tree of ONE split file: a single transaction set, inside the functional
/// group and the interchange it came from. See [`Scope`] for what a split file does NOT
/// carry.
fn split_document_node(
    interchange: &Interchange,
    group: &Group,
    transaction: &Transaction,
    delimiters: &Delimiters,
    release: u32,
) -> Node {
    let groups = vec![(group, vec![transaction])];
    Node::Map(vec![(
        INTERCHANGES_KEY.to_string(),
        Node::List {
            item: INTERCHANGE_ITEM.to_string(),
            items: vec![interchange_node(
                interchange,
                &groups,
                delimiters,
                release,
                Scope::SplitFile,
            )],
        },
    )])
}

/// Every split file's parsed content: one per transaction set the input file holds.
///
/// Bounded twice, INDEPENDENTLY of the input's own 250 KB limit — by [`MAX_SPLIT_FILES`]
/// over the number of files and by [`MAX_SPLIT_OUTPUT_BYTES`] over the bytes they hold
/// together — because the number of transaction sets and the size of each one are separate
/// factors of the output and the input limit bounds only their sum. Each rendering yields
/// to the runtime afterwards, so a cancelled or timed-out request stops here rather than
/// running the whole split out first.
async fn split_file_contents(
    interchanges: &[Interchange],
    delimiters: &Delimiters,
    release: u32,
    format: DocumentFormat,
) -> Result<Vec<String>, TestParsingError> {
    let transaction_sets: usize = interchanges
        .iter()
        .flat_map(|interchange| interchange.groups.iter())
        .map(|group| group.transactions.len())
        .sum();
    if transaction_sets > MAX_SPLIT_FILES {
        return Err(validation(format!(
            "The input file holds {transaction_sets} transaction sets, more than the \
             {MAX_SPLIT_FILES} split files this emulator answers one request with. Parse it \
             without advancedOptions.x12.splitOptions.splitBy TRANSACTION, or split the \
             input file first."
        )));
    }

    let mut contents: Vec<String> = Vec::with_capacity(transaction_sets);
    let mut rendered = 0usize;
    for interchange in interchanges {
        for group in &interchange.groups {
            for transaction in &group.transactions {
                let document = format.render(&split_document_node(
                    interchange,
                    group,
                    transaction,
                    delimiters,
                    release,
                ))?;
                rendered = rendered.saturating_add(document.len());
                if rendered > MAX_SPLIT_OUTPUT_BYTES {
                    return Err(validation(format!(
                        "The split files of this input file hold more than the \
                         {MAX_SPLIT_OUTPUT_BYTES} bytes this emulator answers one request \
                         with. Parse it without \
                         advancedOptions.x12.splitOptions.splitBy TRANSACTION, or split the \
                         input file first."
                    )));
                }
                contents.push(document);
                ::tokio::task::yield_now().await;
            }
        }
    }
    Ok(contents)
}

/// The JSON rendering of a parsed document.
fn json_document(document: &Node) -> Result<String, TestParsingError> {
    ::serde_json::to_string_pretty(document).map_err(internal)
}

/// The XML rendering of a parsed document: the same tree, each node an element named by
/// its key — the document an XSLT mapping template consumes.
fn xml_document(document: &Node) -> Result<String, TestParsingError> {
    use ::quick_xml::events::{BytesDecl, Event};

    let Node::Map(roots) = document else {
        return Err(internal("the parsed document is not a map"));
    };
    let mut writer = ::quick_xml::Writer::new_with_indent(Vec::new(), b' ', 2);
    writer
        .write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))
        .map_err(internal)?;
    for (key, child) in roots {
        write_xml_node(&mut writer, key, child)?;
    }
    String::from_utf8(writer.into_inner()).map_err(internal)
}

/// One node of the document tree, as an XML element named `name`.
fn write_xml_node(
    writer: &mut ::quick_xml::Writer<Vec<u8>>,
    name: &str,
    node: &Node,
) -> Result<(), TestParsingError> {
    use ::quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};

    match node {
        Node::Text(text) => {
            writer
                .write_event(Event::Start(BytesStart::new(name)))
                .map_err(internal)?;
            writer
                .write_event(Event::Text(BytesText::new(text)))
                .map_err(internal)?;
            writer
                .write_event(Event::End(BytesEnd::new(name)))
                .map_err(internal)?;
        }
        Node::Map(members) if members.is_empty() => {
            writer
                .write_event(Event::Empty(BytesStart::new(name)))
                .map_err(internal)?;
        }
        Node::Map(members) => {
            writer
                .write_event(Event::Start(BytesStart::new(name)))
                .map_err(internal)?;
            for (key, child) in members {
                write_xml_node(writer, key, child)?;
            }
            writer
                .write_event(Event::End(BytesEnd::new(name)))
                .map_err(internal)?;
        }
        Node::List { item, items } => {
            writer
                .write_event(Event::Start(BytesStart::new(name)))
                .map_err(internal)?;
            for child in items {
                write_xml_node(writer, item, child)?;
            }
            writer
                .write_event(Event::End(BytesEnd::new(name)))
                .map_err(internal)?;
        }
    }
    Ok(())
}

/// The document format `fileFormat` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DocumentFormat {
    Json,
    Xml,
}

impl DocumentFormat {
    /// The format `value` names, or `None` for a `FileFormat` value that names no document
    /// format a parse can be answered in (`NOT_USED`).
    fn decode(value: &str) -> Option<Self> {
        match value {
            "JSON" => Some(Self::Json),
            "XML" => Some(Self::Xml),
            _ => None,
        }
    }

    /// `document` rendered in this format.
    fn render(self, document: &Node) -> Result<String, TestParsingError> {
        match self {
            Self::Json => json_document(document),
            Self::Xml => xml_document(document),
        }
    }
}

/// An `elementPosition` of an element requirement rule, split into its segment id and its
/// element position: "the format follows the pattern of segment identifier followed by
/// element position (e.g., \"ST-01\" for the first element of the ST segment)"
/// (`X12ElementRequirementValidationRule$elementPosition`).
fn decode_element_position(value: &str) -> Option<(String, usize)> {
    let (segment, position) = value.split_once('-')?;
    // The same shape a real segment id has (see `split_segments`): alphanumeric, starting
    // with a letter.
    let shaped = segment
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && segment
            .chars()
            .all(|character| character.is_ascii_alphanumeric());
    if !shaped {
        return None;
    }
    let position: usize = position.parse().ok()?;
    (position > 0).then(|| (segment.to_string(), position))
}

/// Every segment of `interchanges` in document order, envelope segments included: what an
/// element requirement rule is evaluated over.
fn all_segments(interchanges: &[Interchange]) -> Vec<&Segment> {
    let mut segments: Vec<&Segment> = Vec::new();
    for interchange in interchanges {
        segments.push(&interchange.isa);
        segments.extend(interchange.loose.iter());
        for group in &interchange.groups {
            segments.push(&group.gs);
            segments.extend(group.loose.iter());
            for transaction in &group.transactions {
                segments.push(&transaction.st);
                segments.extend(transaction.body.iter());
                segments.push(&transaction.se);
            }
            segments.push(&group.ge);
        }
        segments.push(&interchange.iea);
    }
    segments
}

/// The validation messages the configured element requirement rules produce over the
/// parsed document: one per occurrence of a segment that does not carry an element the
/// request made MANDATORY.
///
/// A rule whose requirement is `OPTIONAL` makes an element permitted to be absent, so it
/// can produce no message; a rule naming a segment the file does not carry has nothing to
/// check.
fn requirement_messages(
    rules: &[&X12ElementRequirementValidationRule],
    interchanges: &[Interchange],
) -> Vec<String> {
    let segments = all_segments(interchanges);
    let mut messages: Vec<String> = Vec::new();
    let mut withheld = 0usize;
    for rule in rules {
        if rule.requirement != REQUIREMENT_MANDATORY {
            continue;
        }
        let Some((segment_id, position)) = decode_element_position(&rule.element_position) else {
            continue;
        };
        let mut occurrence = 0usize;
        for segment in segments.iter().filter(|segment| segment.id == segment_id) {
            occurrence += 1;
            if segment.element(position).is_empty() {
                // One slot of the bound is kept for the notice below, so a saturated
                // response carries MAX_VALIDATION_MESSAGES entries and not one more.
                if messages.len() + 1 >= MAX_VALIDATION_MESSAGES {
                    withheld += 1;
                    continue;
                }
                messages.push(format!(
                    "Element {} is configured MANDATORY but occurrence {occurrence} of \
                     segment {segment_id} in the input file carries no value for it.",
                    rule.element_position
                ));
            }
        }
    }
    if withheld > 0 {
        messages.push(format!(
            "{withheld} further validation messages are not listed: this emulator reports \
             at most {MAX_VALIDATION_MESSAGES} validation messages for one request."
        ));
    }
    messages
}

/// The S3 error codes that mean the object `inputFile` names is not there — which is the
/// modelled `ResourceNotFoundException`, not a service fault.
const ABSENT_OBJECT_CODES: &[&str] = &["NoSuchKey", "NoSuchBucket", "NotFound"];

/// The S3 error codes that mean the input file could not be read because the read was not
/// PERMITTED — which is the modelled `AccessDeniedException` (HTTP 403), "you do not have
/// sufficient access to perform this action", and not a service fault either: a caller
/// that is told 500 would retry a request that can never succeed.
const DENIED_READ_CODES: &[&str] = &[
    "AccessDenied",
    "AllAccessDisabled",
    "InvalidAccessKeyId",
    "SignatureDoesNotMatch",
];

/// The text of the EDI document `location` names, read from object storage.
async fn read_input_file(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    location: &S3Location,
) -> Result<String, TestParsingError> {
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;

    let Some(bucket) = location
        .bucket_name
        .as_deref()
        .filter(|bucket| !bucket.is_empty())
    else {
        return Err(validation(
            "inputFile.bucketName is required: TestParsing parses the EDI document held \
             by the Amazon S3 object inputFile names.",
        ));
    };
    let Some(key) = location.key.as_deref().filter(|key| !key.is_empty()) else {
        return Err(validation(
            "inputFile.key is required: TestParsing parses the EDI document held by the \
             Amazon S3 object inputFile names.",
        ));
    };

    // The ONE Amazon S3 client this service reads an object through
    // (`crate::state::s3_client`): the in-process callback into this same emulator, so
    // the object read here is the one a caller's own `s3:PutObject` stored.
    let object = match crate::state::s3_client(ctx, region)
        .get_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
    {
        Ok(object) => object,
        Err(error) => {
            let code = error.code().unwrap_or_default().to_string();
            let detail = ::aws_smithy_types::error::display::DisplayErrorContext(&error).to_string();
            if ABSENT_OBJECT_CODES.contains(&code.as_str()) {
                return Err(not_found(bucket, key));
            }
            if DENIED_READ_CODES.contains(&code.as_str()) {
                return Err(access_denied(bucket, key, &code));
            }
            return Err(internal(detail));
        }
    };
    // The object's listed length is what the size limit is decided from, before any of it
    // is buffered; the fetched length is checked again below in case the two disagree.
    if object
        .content_length()
        .is_some_and(|length| length > MAX_INPUT_FILE_BYTES as i64)
    {
        return Err(oversized(bucket, key, object.content_length().unwrap_or(0)));
    }
    let body = object.body.collect().await.map_err(internal)?.into_bytes();
    if body.len() > MAX_INPUT_FILE_BYTES {
        return Err(oversized(bucket, key, body.len() as i64));
    }
    String::from_utf8(body.to_vec()).map_err(|_| {
        validation(format!(
            "The input file s3://{bucket}/{key} is not UTF-8 text, so it holds no EDI \
             document this emulator can parse."
        ))
    })
}

/// The refusal for an input file past AWS's documented [`MAX_INPUT_FILE_BYTES`] limit.
fn oversized(bucket: &str, key: &str, bytes: i64) -> TestParsingError {
    validation(format!(
        "The input file s3://{bucket}/{key} is {bytes} bytes, more than the \
         {MAX_INPUT_FILE_BYTES} byte limit TestParsing documents for the input file."
    ))
}

/// The `splitBy` the request configures, defaulted to `NONE` when it configures none.
fn split_by(advanced: Option<&AdvancedOptions>) -> &str {
    advanced
        .and_then(|advanced| advanced.x12.as_ref())
        .and_then(|x12| x12.split_options.as_ref())
        .map(|options| options.split_by.as_str())
        .unwrap_or(SPLIT_BY_NONE)
}

/// The `TestParsing` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct TestParsing<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> TestParsing<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for TestParsing<Eng>
{
    type Input = TestParsingInput;
    type Output = TestParsingOutput;
    type Error = TestParsingError;

    const NAME: &'static str = "TestParsing";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::TEST_PARSING_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::TEST_PARSING_OUTPUT_SCHEMA;
    // awsJson1_0 is an RPC protocol, so the `@http` code the model puts on this operation
    // (201) is inert on the wire and a success is 200.
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Authorization first: nothing is read for a caller IAM refuses. A parse
        // addresses no resource ARN, so the request is matched against the policy
        // wildcard — the decision every operation of this service that models
        // `AccessDeniedException` takes (`crate::state::authorize`).
        if let Err(denied) =
            crate::state::authorize(&*self.engine, ctx, Self::NAME, crate::state::ANY_RESOURCE)
        {
            return Err(TestParsingErrorModeled::AccessDeniedException(denied).into());
        }

        // Beyond that decision no state is touched: this operation owns no resource (AWS
        // persists nothing for it — the model gives a parse no id and no Get/List), so the
        // answer is computed from the stored OBJECT the request names and nothing a later
        // caller sees changes.

        let Some(format) = DocumentFormat::decode(&input.file_format) else {
            return Err(validation(format!(
                "fileFormat value '{}' names no document format a parsed EDI file can be \
                 answered in: the currently supported file formats for EDI transformations \
                 are JSON and XML.",
                input.file_format
            )));
        };

        // `EdiType` is a union with one member: X12 is the only EDI standard this
        // operation parses ("currently, only X12 is supported").
        let EdiType::x12Details(details) = &input.edi_type;

        let split = match split_by(input.advanced_options.as_ref()) {
            SPLIT_BY_NONE => false,
            SPLIT_BY_TRANSACTION => true,
            other => {
                return Err(validation(format!(
                    "advancedOptions.x12.splitOptions.splitBy value '{other}' is not one of \
                     {SPLIT_BY_NONE}, {SPLIT_BY_TRANSACTION}."
                )))
            }
        };

        // The validation rules this emulator can apply are the positional ones; the two
        // kinds keyed by a four-digit X12 DATA ELEMENT id need the data element dictionary
        // this emulator does not carry, and answering 200 with no message for one would
        // report a validation that never ran.
        let mut requirement_rules: Vec<&X12ElementRequirementValidationRule> = Vec::new();
        let configured = input
            .advanced_options
            .as_ref()
            .and_then(|advanced| advanced.x12.as_ref())
            .and_then(|x12| x12.validation_options.as_ref())
            .and_then(|options| options.validation_rules.as_deref())
            .unwrap_or_default();
        if configured.len() > MAX_VALIDATION_RULES {
            return Err(validation(format!(
                "advancedOptions.x12.validationOptions.validationRules carries {} rules, \
                 more than the {MAX_VALIDATION_RULES} rules this emulator evaluates over \
                 one input file.",
                configured.len()
            )));
        }
        for rule in configured {
            match rule {
                X12ValidationRule::elementRequirementValidationRule(rule) => {
                    if decode_element_position(&rule.element_position).is_none() {
                        return Err(validation(format!(
                            "advancedOptions.x12.validationOptions.validationRules \
                             elementRequirementValidationRule.elementPosition value '{}' is \
                             not a segment identifier followed by an element position, as \
                             in 'ST-01'.",
                            rule.element_position
                        )));
                    }
                    requirement_rules.push(rule);
                }
                X12ValidationRule::codeListValidationRule(rule) => {
                    return Err(validation(format!(
                        "advancedOptions.x12.validationOptions.validationRules carries a \
                         codeListValidationRule for element id '{}': a rule keyed by an X12 \
                         data element id cannot be applied by this emulator, which carries \
                         no X12 data element dictionary, so no code list validation was \
                         performed. Configure elementRequirementValidationRule rules, which \
                         are keyed by element position.",
                        rule.element_id
                    )))
                }
                X12ValidationRule::elementLengthValidationRule(rule) => {
                    return Err(validation(format!(
                        "advancedOptions.x12.validationOptions.validationRules carries an \
                         elementLengthValidationRule for element id '{}': a rule keyed by an \
                         X12 data element id cannot be applied by this emulator, which \
                         carries no X12 data element dictionary, so no element length \
                         validation was performed. Configure \
                         elementRequirementValidationRule rules, which are keyed by element \
                         position.",
                        rule.element_id
                    )))
                }
            }
        }

        let region = crate::state::request_region(ctx.parts());
        let content = read_input_file(ctx, &region, &input.input_file).await?;
        // Leading whitespace is file formatting, not part of the interchange; the
        // interchange header itself is what declares the delimiters.
        let content = content.trim_start();

        let delimiters = detect_delimiters(content).map_err(validation)?;
        let segments = split_segments(content, &delimiters).map_err(validation)?;
        let interchanges = parse_interchanges(segments).map_err(validation)?;

        // ISA11 is the Repetition Separator from X12 release 004020 and the Interchange
        // Control Standards Identifier before it, so the element's name follows the
        // release: the one the request names, else the one the interchange declares in
        // ISA12.
        let release = requested_release(details)
            .or_else(|| {
                interchanges
                    .first()
                    .and_then(|interchange| declared_release(&interchange.isa))
            })
            .unwrap_or_default();

        let parsed_file_content = format.render(&document_node(&interchanges, &delimiters, release))?;

        let parsed_split_file_contents = if split {
            Some(split_file_contents(&interchanges, &delimiters, release, format).await?)
        } else {
            None
        };

        // A violated requirement rule is REPORTED, not refused: `validationMessages`
        // carries "validation errors, warnings, or confirmations based on the configured
        // X12 validation rules" (that member's own documentation), which is what it exists
        // for — a caller testing a document wants the parse AND the findings in one
        // answer. The modelled `ValidationException` is kept for a request that cannot be
        // honoured at all: content that is not an interchange, or a rule this emulator
        // cannot apply.
        let messages = requirement_messages(&requirement_rules, &interchanges);
        let validation_messages = (!messages.is_empty()).then_some(messages);

        Ok(TestParsingOutput {
            parsed_file_content,
            parsed_split_file_contents,
            validation_messages,
        })
    }
}
