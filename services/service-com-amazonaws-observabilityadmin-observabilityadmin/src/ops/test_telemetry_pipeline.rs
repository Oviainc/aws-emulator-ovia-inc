// SPDX-License-Identifier: CC0-1.0

//! @op TestTelemetryPipeline
//!
//! Runs the processor chain the request's `Configuration` declares over each sample
//! record and answers one `PipelineOutput` per record, in request order: the record
//! as the chain left it, or that record's own `Error` when the chain could not
//! process it. Nothing is stored — this operation "[t]ests a pipeline configuration
//! with sample records to validate data processing before deployment", so it mints no
//! pipeline and leaves every stored record exactly as it found it.
//!
//! ## The configuration document
//!
//! `Configuration.Body` is the same document `CreateTelemetryPipeline` stores
//! verbatim: a YAML (or JSON, which YAML 1.2 is a superset of) pipeline definition
//! whose sections carry a `source`, a `processor` sequence and a `sink` sequence. The
//! sibling OpenSearch Ingestion crate's blueprint catalogue
//! (`services/service-com-amazonaws-osis-amazonopensearchingestionservice/src/state.rs`,
//! `BLUEPRINT_CATALOG`) holds complete examples of that format, including the `csv`,
//! `grok` and `parse_json` processors and the `pipeline` source/sink that chains one
//! section into the next.
//!
//! What this operation reads out of that document is ONLY the processor chain a record
//! passes through: the top-level `processor` sequence when the document writes its
//! sections at the top level, otherwise the entry section's sequence followed by the
//! sequence of each section a `pipeline` sink hands the records on to. Sources and
//! sinks name where data would come from and go to, which a test with
//! caller-supplied records neither reads nor writes. Every section the document
//! declares has to be ON that chain: a sink naming a section the document does not
//! declare, a cycle, a fan-out to two pipelines and a section nothing reaches are each
//! refused, because a chain cut short would report records as processed by processors
//! that never ran.
//!
//! The emulator verifies every request's signature centrally and models no IAM policy
//! evaluation, so the modelled `AccessDeniedException` is deliberately unreachable
//! here: a 200 from this operation is not evidence that a principal is authorized.
//!
//! [`PROCESSORS`] is the set of processors this emulator runs, and a document naming
//! one outside it — or an option outside the set a listed processor takes — is refused
//! with the modelled 400 `ValidationException` rather than quietly ignored: a record
//! answered as processed by a step that never ran would be a wrong answer carrying a
//! success status.
//!
//! ## What a record looks like
//!
//! A record enters the chain in the form its `Type` names: a `JSON` record is the
//! document it carries, and a `STRING` record (the form a record naming no `Type` is
//! in) is its text under the `message` field the field-reading processors of this
//! format default their `source` to. It is reported back in the form it then holds, so
//! a definition declaring no processor answers every record exactly as it arrived.
//!
//! A processor that PARSES a field — `csv`, `parse_json` — is consumed by what it
//! produced: the fields read out of the text take the text's place, so a row of
//! comma-separated values comes back as its columns and not as the columns beside the
//! line they were read from.
//!
//! ## Bounds
//!
//! Every input to this operation is caller-supplied, so each one that drives work is
//! bounded before the work starts: [`MAX_TEST_RECORDS`] records of at most
//! [`MAX_RECORD_DATA_BYTES`] each and [`MAX_TOTAL_RECORD_BYTES`] together,
//! [`MAX_PROCESSORS`] steps in the chain with at most [`MAX_PROCESSOR_ENTRIES`]
//! entries apiece, [`MAX_WORK_BYTES`] of record text read across the WHOLE call (the
//! bound on the work the records, the steps and the entries of one processor would
//! otherwise multiply into, charged before each step reads its field),
//! [`MAX_CONFIGURATION_NODES`] nodes at [`MAX_CONFIGURATION_DEPTH`] of
//! nesting in the configuration document, and [`MAX_EVENT_BYTES`] for the record a
//! chain may grow a record into — charged by `PipelineRecord::insert`, which refuses
//! the write BEFORE the value is stored, so no step materialises a record past the
//! bound. A substitution's replacement is bounded in its own right, and each match's
//! worst-case expansion — its group references, every one of which can expand to the
//! whole match — is charged against the record's remaining bytes BEFORE that match is
//! expanded. The document is read off
//! `yaml-rust2`'s event stream, which refuses an alias rather than expanding it, and a
//! substitution pattern is compiled once per request with the `regex` crate's own size
//! limit, at most [`MAX_COMPILED_PATTERNS`] of them per call so the compiled chain's
//! own memory is bounded too. The per-record loop yields, and so does the chain between
//! its steps, so neither a batch nor one record's chain can hold an async worker.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `TestTelemetryPipeline::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
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
//! # Modeled contract for `TestTelemetryPipeline`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-observabilityadmin-observabilityadmin/smithy-model/com-amazonaws-observabilityadmin-observabilityadmin.json")
//! ```
//!
//! Tests a pipeline configuration with sample records to validate data processing before
//! deployment. This operation helps ensure your pipeline configuration works as expected.
//!
//! ## Input
//!
//! * `Configuration` (TelemetryPipelineConfiguration) **required** — The pipeline
//!   configuration to test with the provided sample records.
//! * `Records` (Records) **required** — The sample records to process through the pipeline
//!   configuration for testing purposes.
//! * `SignalType` (SignalType, one of: LOG | METRIC, default "LOG") **required** — The type
//!   of telemetry signal to test. If not specified, defaults to log processing.
//!
//! ## Output
//!
//! * `Results` (PipelineOutputs) — The results of processing the test records through the
//!   pipeline configuration, including any outputs or errors.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 400) — Indicates you don't have permissions to perform the
//!   requested operation. The user or role that is making the request must have at least one
//!   IAM permissions policy attached that grants the required permissions. For more
//!   information, see Access management for Amazon Web Services resources in the IAM user
//!   guide.
//! * `InternalServerException` (HTTP 500) — Indicates the request has failed to process
//!   because of an unknown server error, exception, or failure.
//! * `TooManyRequestsException` (HTTP 429) — The request throughput limit was exceeded.
//! * `ValidationException` (HTTP 400) — Indicates input validation failed. Check your request
//!   parameters and retry the request.

#[allow(unused_imports)]
use super::types::*;

use ::serde_json::{Map, Value};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `TestTelemetryPipelineError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-observabilityadmin-observabilityadmin.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `TestTelemetryPipelineError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.observabilityadmin#TestTelemetryPipeline")]
#[derive(Debug)]
pub enum TestTelemetryPipelineErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    TooManyRequestsException(TooManyRequestsException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::test_telemetry_pipeline::TestTelemetryPipelineError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type TestTelemetryPipelineError = crate::scaffolded::Scaffolded<TestTelemetryPipelineErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<TestTelemetryPipelineErrorModeled> for TestTelemetryPipelineError {
    fn from(modeled: TestTelemetryPipelineErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

// =============================================================================
// Bounds
//
// Emulator resource guards, NOT AWS quotas: this service publishes none for a test
// call. Each one bounds a growth site a request would otherwise choose the size of.
// =============================================================================

/// The most sample records one call processes.
pub const MAX_TEST_RECORDS: usize = 100;

/// The most `Data` bytes one sample record may carry.
pub const MAX_RECORD_DATA_BYTES: usize = 256 * 1024;

/// The most `Data` bytes one call's records may carry together, which is what bounds
/// the product of the record count and the record size.
pub const MAX_TOTAL_RECORD_BYTES: usize = 1024 * 1024;

/// The most processors one chain may run per record. Processors that copy a field
/// compose, so the chain length has to be bounded alongside [`MAX_EVENT_BYTES`].
pub const MAX_PROCESSORS: usize = 25;

/// The most entries one per-field processor may declare, so the work (and the compiled
/// patterns a `substitute_string` holds) inside ONE step is bounded as well as the
/// number of steps.
pub const MAX_PROCESSOR_ENTRIES: usize = 25;

/// The most bytes of record text one call processes, across every record and every
/// step: the bound on the WORK a request drives, as [`MAX_EVENT_BYTES`] is the bound on
/// the memory one record holds. Without it the records, the chain length and the
/// entries of one processor would multiply into a scan nothing caps.
pub const MAX_WORK_BYTES: usize = 16 * 1024 * 1024;

/// The most bytes a `substitute_string` replacement may carry, so the expansion of one
/// match is bounded before any record is matched against it.
const MAX_REPLACEMENT_BYTES: usize = 256;

/// The most bytes one record may hold while the chain runs it. A record the chain
/// grows past this is answered as that record's own error.
pub const MAX_EVENT_BYTES: usize = 1024 * 1024;

/// The most nodes read out of one configuration document.
const MAX_CONFIGURATION_NODES: usize = 4096;

/// The deepest nesting read out of one configuration document.
const MAX_CONFIGURATION_DEPTH: usize = 24;

/// Columns of leading indentation a configuration document may carry. Bounds how deep
/// the parser's own recursion can go in BLOCK style, which an event receiver cannot
/// abort once `load` is running.
const MAX_CONFIGURATION_INDENT: usize = 200;

/// Flow collection openers (`[`, `{`) a configuration document may hold open at once.
const MAX_CONFIGURATION_FLOW_DEPTH: usize = 24;

/// Flow collection openers a configuration document may carry in total: a byte scan
/// cannot tell a closer inside a scalar from a real one, and whatever a body hides,
/// its real flow nesting cannot exceed its opener count.
const MAX_CONFIGURATION_FLOW_OPENERS: usize = 512;

/// Compact block indicators (`- `, `? `) one line of a configuration document may
/// carry, the other way block nesting descends.
const MAX_CONFIGURATION_COMPACT_PER_LINE: usize = 24;

/// The most bytes a substitution pattern may carry before it is compiled.
const MAX_PATTERN_BYTES: usize = 1024;

/// The compiled size ONE substitution pattern may take, handed to the `regex` crate's
/// own builder so a pattern is refused at compile time rather than at match time.
/// [`MAX_COMPILED_PATTERNS`] bounds how many of them one request holds, so the product
/// bounds the memory a compiled chain takes before any record runs.
const REGEX_SIZE_LIMIT: usize = 256 * 1024;

/// The most substitution patterns one request compiles, counted across every step of
/// the chain rather than per processor.
pub const MAX_COMPILED_PATTERNS: usize = 25;

// =============================================================================
// The pipeline definition's vocabulary
// =============================================================================

/// The field a record's raw text is read into and reported back out of: the `source`
/// every field-reading processor of this format defaults to.
const MESSAGE_FIELD: &str = "message";

/// The document key holding a section's processor sequence.
pub(crate) const PROCESSOR_KEY: &str = "processor";

/// The document key holding a section's source.
pub(crate) const SOURCE_KEY: &str = "source";

/// The document key holding a section's sink sequence.
pub(crate) const SINK_KEY: &str = "sink";

/// The source and sink plugin that chains one pipeline section into another, and the
/// key naming the section it chains to.
const PIPELINE_PLUGIN: &str = "pipeline";
const PIPELINE_NAME_KEY: &str = "name";

/// `RecordFormat`'s own two values (`RECORD_FORMAT_VALUES`), named here because the
/// two forms a record is read in and reported back as are what the output's `Type`
/// carries.
const RECORD_FORMAT_JSON: &str = "JSON";
const RECORD_FORMAT_STRING: &str = "STRING";

/// `SignalType`'s modelled default — "If not specified, defaults to log processing."
const DEFAULT_SIGNAL_TYPE: &str = "LOG";

/// Every processor this emulator runs, with the options it takes.
///
/// The names and option names are this pipeline format's own, as the sibling
/// OpenSearch Ingestion crate's blueprint catalogue spells them
/// (`service-com-amazonaws-osis-amazonopensearchingestionservice`'s
/// `BLUEPRINT_CATALOG`: `csv` with `delimiter`/`column_names`, `parse_json` with
/// `source`). A processor NOT listed here, and an option not listed beside its
/// processor, is refused: see the module docs.
const PROCESSORS: &[(&str, &[&str])] = &[
    (
        "csv",
        &[
            "source",
            "delimiter",
            "quote_character",
            "column_names",
            "column_names_source_key",
            "delete_header",
        ],
    ),
    ("parse_json", &["source", "destination", "pointer"]),
    ("add_entries", &["entries"]),
    ("delete_entries", &["with_keys"]),
    ("rename_keys", &["entries"]),
    ("copy_values", &["entries"]),
    ("lowercase_string", &["with_keys"]),
    ("uppercase_string", &["with_keys"]),
    ("trim_string", &["with_keys"]),
    ("substitute_string", &["entries"]),
    ("split_string", &["entries"]),
];

// =============================================================================
// The configuration document, as nodes
// =============================================================================

/// One node of a configuration document.
///
/// `yaml-rust2` is the YAML 1.2 implementation this workspace reads caller-supplied
/// documents with, and the tree below is built from its EVENT stream rather than from
/// its loader: an event receiver is where an alias can be refused (an alias copies the
/// node its anchor named, which is how a few hundred bytes of YAML expand into
/// gigabytes) and where the node count and the nesting depth can be bounded by
/// constants instead of by what the document asks for. The sibling API Gateway crate
/// reads an API definition the same way, for the same reasons
/// (`services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/put_rest_api.rs`).
/// `pub(crate)` because `ValidateTelemetryPipelineConfiguration` reports defects in the
/// SAME document this reads, so it reads it through the same bounded reader rather
/// than through a second one that could disagree with this one.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) enum Node {
    /// An empty value, `~` or `null` written plain.
    #[default]
    Null,
    /// A scalar, with whether it was written PLAIN — which is what decides whether
    /// `5` is the number five or the text "5".
    Scalar { text: String, plain: bool },
    Sequence(Vec<Node>),
    /// A mapping, in document order, which is the order a chain of sections is read in.
    Mapping(Vec<(String, Node)>),
}

impl Node {
    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Self::Scalar { text, .. } => Some(text.as_str()),
            _ => None,
        }
    }

    pub(crate) fn as_sequence(&self) -> Option<&[Node]> {
        match self {
            Self::Sequence(items) => Some(items.as_slice()),
            _ => None,
        }
    }

    pub(crate) fn as_mapping(&self) -> Option<&[(String, Node)]> {
        match self {
            Self::Mapping(entries) => Some(entries.as_slice()),
            _ => None,
        }
    }

    /// The value of `key`, for a mapping that carries one.
    pub(crate) fn get(&self, key: &str) -> Option<&Node> {
        self.as_mapping()?
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value)
    }

    /// The node as a JSON value, which is what an `add_entries` value is written into
    /// a record as. A plain scalar resolves by YAML's core schema, so `true` and `5`
    /// are the boolean and the number; a quoted one is always text.
    fn to_json(&self) -> Value {
        match self {
            Self::Null => Value::Null,
            Self::Scalar { text, plain } => scalar_to_json(text, *plain),
            Self::Sequence(items) => Value::Array(items.iter().map(Node::to_json).collect()),
            Self::Mapping(entries) => Value::Object(
                entries
                    .iter()
                    .map(|(key, value)| (key.clone(), value.to_json()))
                    .collect(),
            ),
        }
    }
}

/// One scalar as a JSON value. Only a PLAIN scalar resolves to a non-string type,
/// which is YAML's own rule for the core schema.
fn scalar_to_json(text: &str, plain: bool) -> Value {
    if !plain {
        return Value::String(text.to_string());
    }
    match text {
        "true" | "True" | "TRUE" => return Value::Bool(true),
        "false" | "False" | "FALSE" => return Value::Bool(false),
        _ => {}
    }
    if let Ok(integer) = text.parse::<i64>() {
        return Value::from(integer);
    }
    // `from_f64` answers `None` for a non-finite value, which JSON has no spelling
    // for, so such a scalar stays the text it was written as.
    if let Some(number) = text
        .parse::<f64>()
        .ok()
        .and_then(::serde_json::Number::from_f64)
    {
        return Value::Number(number);
    }
    Value::String(text.to_string())
}

/// One collection the reader has open: what its entries collect into until its end
/// event arrives.
#[derive(Debug)]
enum Frame {
    Sequence(Vec<Node>),
    Mapping {
        entries: Vec<(String, Node)>,
        /// The key of the entry being read; `None` while the NEXT node is a key.
        pending_key: Option<String>,
    },
}

/// Builds the first document's node tree off the parser's event stream, bounded.
#[derive(Debug, Default)]
struct ConfigurationBuilder {
    root: Option<Node>,
    open: Vec<Frame>,
    nodes: usize,
    /// Why this document was not read, once a bound or an alias refused it.
    refused: Option<&'static str>,
}

impl ConfigurationBuilder {
    /// Place a finished node where the stream says it belongs.
    fn place(&mut self, node: Node) {
        if self.refused.is_some() {
            return;
        }
        self.nodes = self.nodes.saturating_add(1);
        if self.nodes > MAX_CONFIGURATION_NODES {
            self.refused = Some("it carries more nodes than this emulator reads");
            return;
        }
        match self.open.last_mut() {
            // Only the FIRST document of a multi-document body is read, which is the
            // document a pipeline definition is.
            None => {
                if self.root.is_none() {
                    self.root = Some(node);
                }
            }
            Some(Frame::Sequence(items)) => items.push(node),
            Some(Frame::Mapping {
                entries,
                pending_key,
            }) => match pending_key.take() {
                None => match node {
                    Node::Scalar { text, .. } => *pending_key = Some(text),
                    _ => self.refused = Some("every key of a pipeline definition must be a string"),
                },
                Some(key) => entries.push((key, node)),
            },
        }
    }

    fn open_frame(&mut self, frame: Frame) {
        if self.refused.is_some() {
            return;
        }
        if self.open.len() >= MAX_CONFIGURATION_DEPTH {
            self.refused = Some("it nests more deeply than this emulator reads");
            return;
        }
        self.open.push(frame);
    }

    fn close_frame(&mut self) {
        if self.refused.is_some() {
            return;
        }
        let Some(frame) = self.open.pop() else {
            return;
        };
        let node = match frame {
            Frame::Sequence(items) => Node::Sequence(items),
            Frame::Mapping { entries, .. } => Node::Mapping(entries),
        };
        self.place(node);
    }
}

impl ::yaml_rust2::parser::EventReceiver for ConfigurationBuilder {
    fn on_event(&mut self, event: ::yaml_rust2::parser::Event) {
        use ::yaml_rust2::parser::Event;
        if self.refused.is_some() {
            return;
        }
        match event {
            Event::Scalar(text, style, _anchor, _tag) => {
                let plain = matches!(style, ::yaml_rust2::scanner::TScalarStyle::Plain);
                let node = if plain && matches!(text.as_str(), "" | "~" | "null" | "Null" | "NULL") {
                    Node::Null
                } else {
                    Node::Scalar { text, plain }
                };
                self.place(node);
            }
            Event::SequenceStart(..) => self.open_frame(Frame::Sequence(Vec::new())),
            Event::MappingStart(..) => self.open_frame(Frame::Mapping {
                entries: Vec::new(),
                pending_key: None,
            }),
            Event::SequenceEnd | Event::MappingEnd => self.close_frame(),
            // An alias copies an anchored node, which is the unbounded amplification a
            // YAML body can carry. No pipeline definition needs one.
            Event::Alias(_) => {
                self.refused =
                    Some("an alias expands the document it appears in and is not read here");
            }
            _ => {}
        }
    }
}

/// Nesting bounds applied to the document BEFORE the parser builds anything from it.
///
/// What has to be bounded here is how deep the parser can RECURSE: an event receiver
/// cannot abort `load`, and exhausting the stack aborts the process rather than
/// unwinding. Each measure bounds NESTING rather than size, so a long flat document is
/// not affected by any of them.
fn guard_configuration_nesting(body: &str) -> Result<(), String> {
    let refusal = || {
        "Configuration.Body is not a pipeline definition this emulator reads: it nests more \
         deeply than this emulator reads"
            .to_string()
    };
    let mut flow_depth = 0usize;
    let mut flow_openers = 0usize;
    let mut compact_on_line = 0usize;
    let mut indent = 0usize;
    let mut counting_indent = true;
    let mut previous = b'\n';
    let mut before_previous = b'\n';

    for byte in body.bytes() {
        match byte {
            b'\n' | b'\r' => {
                indent = 0;
                compact_on_line = 0;
                counting_indent = true;
            }
            b' ' | b'\t' if counting_indent => {
                indent = indent.saturating_add(1);
                if indent > MAX_CONFIGURATION_INDENT {
                    return Err(refusal());
                }
            }
            b'[' | b'{' => {
                counting_indent = false;
                flow_depth = flow_depth.saturating_add(1);
                flow_openers = flow_openers.saturating_add(1);
                if flow_depth > MAX_CONFIGURATION_FLOW_DEPTH
                    || flow_openers > MAX_CONFIGURATION_FLOW_OPENERS
                {
                    return Err(refusal());
                }
            }
            b']' | b'}' => {
                counting_indent = false;
                flow_depth = flow_depth.saturating_sub(1);
            }
            _ => counting_indent = false,
        }
        // A `-` or `?` is a block indicator only where a token may start — at the start
        // of a line or after a space — and only when a space follows it, which is what
        // separates `- item` from the hyphen in `log-pipeline` and from the ` - ` inside
        // a scalar.
        if matches!(previous, b'-' | b'?')
            && matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
            && matches!(before_previous, b' ' | b'\t' | b'\n' | b'\r')
        {
            compact_on_line = compact_on_line.saturating_add(1);
            if compact_on_line > MAX_CONFIGURATION_COMPACT_PER_LINE {
                return Err(refusal());
            }
        }
        before_previous = previous;
        previous = byte;
    }
    Ok(())
}

/// The configuration document as a node tree.
pub(crate) fn parse_configuration(body: &str) -> Result<Node, String> {
    guard_configuration_nesting(body)?;
    let mut builder = ConfigurationBuilder::default();
    ::yaml_rust2::parser::Parser::new_from_str(body)
        .load(&mut builder, true)
        .map_err(|why| {
            format!("Configuration.Body is not a pipeline definition this emulator reads: {why}")
        })?;
    if let Some(refusal) = builder.refused {
        return Err(format!(
            "Configuration.Body is not a pipeline definition this emulator reads: {refusal}"
        ));
    }
    Ok(builder.root.take().unwrap_or_default())
}

// =============================================================================
// The processor chain
// =============================================================================

/// The processors a record passes through, in the order it passes through them.
///
/// A document that writes its `source`/`processor`/`sink` at the top level IS one
/// pipeline section. Otherwise every mapping entry that carries one of those keys is a
/// named section, and the chain is the entry section's processors followed by those of
/// each section a `pipeline` sink hands its records on to — which is what the format's
/// `pipeline` source and sink mean.
pub(crate) fn processor_chain(root: &Node) -> Result<Vec<(&str, &Node)>, String> {
    if matches!(root, Node::Null) {
        return Err("Configuration.Body declares no pipeline".to_string());
    }
    let Some(entries) = root.as_mapping() else {
        return Err("Configuration.Body must declare a pipeline as a mapping".to_string());
    };
    let chain = if is_section(root) {
        steps_of(root)?
    } else {
        chain_of_sections(entries)?
    };
    if chain.len() > MAX_PROCESSORS {
        return Err(format!(
            "Configuration.Body declares {} processors, more than the {MAX_PROCESSORS} this \
             emulator runs over one record",
            chain.len()
        ));
    }
    Ok(chain)
}

/// The chain of a document that writes its pipelines as named sections: the entry
/// section's processors, followed by those of each section a `pipeline` sink hands its
/// records on to.
///
/// Every section the document declares has to be ON that chain. A sink naming a
/// section the document does not declare, a cycle among the sections, a section that
/// hands its records to more than one pipeline, and a section the walk never reaches
/// are each refused: a chain the walk had to cut short would otherwise report records
/// as processed by processors that never ran.
fn chain_of_sections(entries: &[(String, Node)]) -> Result<Vec<(&str, &Node)>, String> {
    let sections: Vec<&(String, Node)> = entries
        .iter()
        .filter(|(_, value)| is_section(value))
        .collect();
    if sections.is_empty() {
        return Err(
            "Configuration.Body declares no pipeline section carrying a source, a processor \
             or a sink"
                .to_string(),
        );
    }

    // The entry section is the one no other section feeds, i.e. the first whose own
    // source is not another pipeline.
    let start = sections
        .iter()
        .position(|(_, section)| !sourced_from_pipeline(section))
        .unwrap_or(0);
    let mut chain: Vec<(&str, &Node)> = Vec::new();
    let mut visited: Vec<bool> = vec![false; sections.len()];
    let mut current = Some(start);
    while let Some(index) = current {
        let (Some(visit), Some((_, section))) = (visited.get_mut(index), sections.get(index))
        else {
            break;
        };
        if *visit {
            return Err(
                "Configuration.Body's pipeline sections hand their records round a cycle"
                    .to_string(),
            );
        }
        *visit = true;
        chain.extend(steps_of(section)?);
        current = next_section(section, &sections)?;
    }
    if let Some((name, _)) = visited
        .iter()
        .zip(sections.iter())
        .find(|(reached, _)| !**reached)
        .map(|(_, section)| *section)
    {
        return Err(format!(
            "Configuration.Body declares the pipeline section '{name}', which no sink of the \
             entry section hands its records to"
        ));
    }
    Ok(chain)
}

/// The pipeline sections a document declares, in document order, each with the name it
/// declares that section under — EMPTY for a document that writes its one pipeline at
/// the top level.
///
/// A document that writes its `source`/`processor`/`sink` at the top level IS one
/// pipeline section; otherwise every mapping entry whose value carries one of those
/// keys is a section, and each of those is a pipeline in its own right. A document that
/// declares neither declares NO pipeline, so the list is empty: what such a document is
/// missing is the pipeline itself, which is what
/// [`crate::ops::validate_telemetry_pipeline_configuration`] reports and what
/// [`processor_chain`] refuses.
///
/// `pub(crate)` for the same reason the reader below is: the validator reports defects
/// in the same sections this operation runs, so both read the document's sections out
/// of one place rather than out of two that could disagree.
pub(crate) fn declared_sections(root: &Node) -> Vec<(&str, &Node)> {
    if is_section(root) {
        return vec![("", root)];
    }
    root.as_mapping()
        .unwrap_or_default()
        .iter()
        .filter(|(_, value)| is_section(value))
        .map(|(name, value)| (name.as_str(), value))
        .collect()
}

/// Whether a node is a pipeline section: a mapping carrying a source, a processor
/// sequence or a sink.
pub(crate) fn is_section(node: &Node) -> bool {
    node.as_mapping().is_some_and(|entries| {
        entries
            .iter()
            .any(|(key, _)| matches!(key.as_str(), SOURCE_KEY | PROCESSOR_KEY | SINK_KEY))
    })
}

/// Whether a section reads its records from another pipeline section.
fn sourced_from_pipeline(section: &Node) -> bool {
    section
        .get(SOURCE_KEY)
        .and_then(|source| source.get(PIPELINE_PLUGIN))
        .is_some()
}

/// The section a section's `pipeline` sink hands its records on to, if it has one.
///
/// A sink naming a section the document does not declare, and a section naming more
/// than one pipeline sink, are refused rather than walked past: the first leaves
/// records nowhere, and the second is a fan-out whose records would go through two
/// chains at once.
fn next_section(section: &Node, sections: &[&(String, Node)]) -> Result<Option<usize>, String> {
    let Some(sink) = section.get(SINK_KEY) else {
        return Ok(None);
    };
    let Some(sinks) = sink.as_sequence() else {
        return Err(format!("{SINK_KEY} must be a sequence of sinks"));
    };
    let mut targets: Vec<usize> = Vec::new();
    for entry in sinks {
        let Some(pipeline) = entry.get(PIPELINE_PLUGIN) else {
            continue;
        };
        let name = pipeline
            .get(PIPELINE_NAME_KEY)
            .and_then(Node::as_str)
            .ok_or_else(|| {
                format!("a {PIPELINE_PLUGIN} sink names the section it hands its records to")
            })?;
        let index = sections
            .iter()
            .position(|(key, _)| key == name)
            .ok_or_else(|| {
                format!(
                    "a {PIPELINE_PLUGIN} sink names the section '{name}', which \
                     Configuration.Body does not declare"
                )
            })?;
        targets.push(index);
    }
    match targets.as_slice() {
        [] => Ok(None),
        [next] => Ok(Some(*next)),
        _ => Err(format!(
            "a section handing its records to {} pipelines at once is a fan-out this emulator \
             does not run",
            targets.len()
        )),
    }
}

/// One section's `processor` sequence, as `(name, options)` pairs. Each entry of the
/// sequence names exactly one processor, which is what this format's processor list is.
fn steps_of(section: &Node) -> Result<Vec<(&str, &Node)>, String> {
    let Some(processors) = section.get(PROCESSOR_KEY) else {
        return Ok(Vec::new());
    };
    let Some(items) = processors.as_sequence() else {
        return Err(format!("{PROCESSOR_KEY} must be a sequence of processors"));
    };
    items
        .iter()
        .map(|item| {
            let Some(entries) = item.as_mapping() else {
                return Err(format!(
                    "every entry of {PROCESSOR_KEY} must name one processor"
                ));
            };
            match entries {
                [(name, options)] => Ok((name.as_str(), options)),
                _ => Err(format!(
                    "every entry of {PROCESSOR_KEY} must name exactly one processor, got {}",
                    entries.len()
                )),
            }
        })
        .collect()
}

// =============================================================================
// The compiled chain
// =============================================================================

/// One compiled processor. Compiling the chain ONCE per request is what keeps a
/// configuration defect a request-level refusal (a 400 naming the processor) rather
/// than a per-record failure, and what keeps a pattern out of the per-record loop.
#[derive(Debug)]
enum Step {
    Csv(CsvStep),
    ParseJson(ParseJsonStep),
    AddEntries(Vec<AddEntry>),
    DeleteEntries(Vec<String>),
    RenameKeys(Vec<KeyMove>),
    CopyValues(Vec<KeyMove>),
    MapStrings(StringMap, Vec<String>),
    SubstituteString(Vec<Substitution>),
    SplitString(Vec<SplitEntry>),
}

#[derive(Debug)]
struct CsvStep {
    source: String,
    delimiter: u8,
    quote: u8,
    column_names: Option<Vec<String>>,
    column_names_source_key: Option<String>,
    delete_header: bool,
}

#[derive(Debug)]
struct ParseJsonStep {
    source: String,
    destination: Option<String>,
    pointer: Option<String>,
}

#[derive(Debug)]
struct AddEntry {
    key: String,
    value: Value,
    overwrite_if_key_exists: bool,
}

#[derive(Debug)]
struct KeyMove {
    from_key: String,
    to_key: String,
    overwrite_if_to_key_exists: bool,
}

#[derive(Debug)]
struct Substitution {
    source: String,
    from: ::regex::Regex,
    to: String,
    /// How many group references the replacement may carry — the count of `$` in it,
    /// which is an upper bound on them, since `$$` is one escaped dollar and not a
    /// reference. Each reference expands to at most the whole match, so this is what
    /// bounds one match's expansion before it is made.
    expansions: usize,
}

#[derive(Debug)]
struct SplitEntry {
    source: String,
    delimiter: String,
}

/// Which string transform a `*_string` processor applies.
#[derive(Debug, Clone, Copy)]
enum StringMap {
    Lowercase,
    Uppercase,
    Trim,
}

impl StringMap {
    fn apply(self, text: &str) -> String {
        match self {
            Self::Lowercase => text.to_lowercase(),
            Self::Uppercase => text.to_uppercase(),
            Self::Trim => text.trim().to_string(),
        }
    }
}

/// Compile the chain, refusing anything this emulator would otherwise have to ignore.
///
/// The whole chain is compiled once per request, so the patterns it holds are counted
/// across every step: the per-pattern bounds alone would leave the SUM of the compiled
/// automata to the configuration's own length.
fn compile_chain(chain: &[(&str, &Node)]) -> Result<Vec<Step>, String> {
    let mut patterns = 0usize;
    chain
        .iter()
        .map(|&(name, options)| compile_step(name, options, &mut patterns))
        .collect()
}

/// Whether the processor `name` with `options` is one this emulator runs, with options
/// it takes, and the refusal text when it is not.
///
/// `ValidateTelemetryPipelineConfiguration` reports that refusal per POSITION of a
/// document's processor sequence instead of refusing the request, and asks this rather
/// than keeping a second copy of the catalogue [`compile_step`] reads.
///
/// `patterns` is the compiled-pattern count the caller carries across the whole
/// request, so the automata ONE request holds stay bounded by
/// [`MAX_COMPILED_PATTERNS`] however the document spreads its processors over the
/// positions and sections it declares.
pub(crate) fn step_defect(name: &str, options: &Node, patterns: &mut usize) -> Result<(), String> {
    compile_step(name, options, patterns).map(|_| ())
}

fn compile_step(name: &str, options: &Node, patterns: &mut usize) -> Result<Step, String> {
    let Some((_, allowed)) = PROCESSORS.iter().find(|(known, _)| known == &name) else {
        return Err(format!(
            "the pipeline configuration names the processor '{name}', which this emulator does \
             not run; it runs [{}]",
            PROCESSORS
                .iter()
                .map(|(known, _)| *known)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    };
    reject_unknown_options(name, options, allowed)?;
    match name {
        "csv" => compile_csv(name, options),
        "parse_json" => Ok(Step::ParseJson(ParseJsonStep {
            source: option_str(name, options, "source")?
                .unwrap_or(MESSAGE_FIELD)
                .to_string(),
            destination: option_str(name, options, "destination")?.map(str::to_string),
            pointer: option_str(name, options, "pointer")?.map(str::to_string),
        })),
        "add_entries" => compile_add_entries(name, options),
        "delete_entries" => Ok(Step::DeleteEntries(required_keys(
            name,
            options,
            "with_keys",
        )?)),
        "rename_keys" => Ok(Step::RenameKeys(compile_key_moves(name, options)?)),
        "copy_values" => Ok(Step::CopyValues(compile_key_moves(name, options)?)),
        "lowercase_string" => Ok(Step::MapStrings(
            StringMap::Lowercase,
            required_keys(name, options, "with_keys")?,
        )),
        "uppercase_string" => Ok(Step::MapStrings(
            StringMap::Uppercase,
            required_keys(name, options, "with_keys")?,
        )),
        "trim_string" => Ok(Step::MapStrings(
            StringMap::Trim,
            required_keys(name, options, "with_keys")?,
        )),
        "substitute_string" => compile_substitutions(name, options, patterns),
        "split_string" => compile_splits(name, options),
        // `PROCESSORS` is the one record of what this emulator runs, so a name it
        // lists and this match does not is a defect in this file rather than in the
        // request.
        _ => Err(format!(
            "the processor '{name}' is listed as supported but has no implementation"
        )),
    }
}

fn compile_csv(name: &str, options: &Node) -> Result<Step, String> {
    let delimiter = option_char(name, options, "delimiter")?.unwrap_or(b',');
    let quote = option_char(name, options, "quote_character")?.unwrap_or(b'"');
    // A delimiter that is also the quote character separates nothing: what the reader
    // would make of such a row is not defined, so the configuration is refused instead.
    if delimiter == quote {
        return Err(format!(
            "the {name} processor's 'delimiter' and 'quote_character' must differ, got both as \
             '{}'",
            char::from(delimiter)
        ));
    }
    Ok(Step::Csv(CsvStep {
        source: option_str(name, options, "source")?
            .unwrap_or(MESSAGE_FIELD)
            .to_string(),
        delimiter,
        quote,
        column_names: option_keys(name, options, "column_names")?,
        column_names_source_key: option_str(name, options, "column_names_source_key")?
            .map(str::to_string),
        delete_header: option_bool(name, options, "delete_header")?.unwrap_or(true),
    }))
}

fn compile_add_entries(name: &str, options: &Node) -> Result<Step, String> {
    entries_of(name, options)?
        .iter()
        .map(|entry| {
            reject_unknown_options(name, entry, &["key", "value", "overwrite_if_key_exists"])?;
            Ok(AddEntry {
                key: required_str(name, entry, "key")?.to_string(),
                value: entry.get("value").map(Node::to_json).unwrap_or(Value::Null),
                overwrite_if_key_exists: option_bool(name, entry, "overwrite_if_key_exists")?
                    .unwrap_or(false),
            })
        })
        .collect::<Result<Vec<_>, String>>()
        .map(Step::AddEntries)
}

fn compile_key_moves(name: &str, options: &Node) -> Result<Vec<KeyMove>, String> {
    entries_of(name, options)?
        .iter()
        .map(|entry| {
            reject_unknown_options(
                name,
                entry,
                &["from_key", "to_key", "overwrite_if_to_key_exists"],
            )?;
            Ok(KeyMove {
                from_key: required_str(name, entry, "from_key")?.to_string(),
                to_key: required_str(name, entry, "to_key")?.to_string(),
                overwrite_if_to_key_exists: option_bool(
                    name,
                    entry,
                    "overwrite_if_to_key_exists",
                )?
                .unwrap_or(false),
            })
        })
        .collect()
}

fn compile_substitutions(
    name: &str,
    options: &Node,
    patterns: &mut usize,
) -> Result<Step, String> {
    entries_of(name, options)?
        .iter()
        .map(|entry| {
            reject_unknown_options(name, entry, &["source", "from", "to"])?;
            let replacement = required_str(name, entry, "to")?;
            if replacement.len() > MAX_REPLACEMENT_BYTES {
                return Err(format!(
                    "the {name} processor's 'to' replacement is {} bytes, more than the \
                     {MAX_REPLACEMENT_BYTES} this emulator substitutes",
                    replacement.len()
                ));
            }
            let pattern = required_str(name, entry, "from")?;
            if pattern.len() > MAX_PATTERN_BYTES {
                return Err(format!(
                    "the {name} processor's 'from' pattern is {} bytes, more than the \
                     {MAX_PATTERN_BYTES} this emulator compiles",
                    pattern.len()
                ));
            }
            // The patterns of a whole request are counted here, not per processor:
            // each compiled automaton may take up to REGEX_SIZE_LIMIT, so the COUNT is
            // what bounds the memory the compiled chain holds before any record runs.
            *patterns = patterns.saturating_add(1);
            if *patterns > MAX_COMPILED_PATTERNS {
                return Err(format!(
                    "the pipeline configuration declares more than the {MAX_COMPILED_PATTERNS} \
                     substitution patterns this emulator compiles in one call"
                ));
            }
            // Compiled through the crate's builder with its size limit, so a pattern
            // whose automaton would be large is refused here rather than while a
            // record is being matched. The crate matches in time linear in the
            // subject, so a pattern cannot make one record's match unbounded.
            let from = ::regex::RegexBuilder::new(pattern)
                .size_limit(REGEX_SIZE_LIMIT)
                .dfa_size_limit(REGEX_SIZE_LIMIT)
                .build()
                .map_err(|why| {
                    format!("the {name} processor's 'from' pattern is not a pattern: {why}")
                })?;
            Ok(Substitution {
                source: required_str(name, entry, "source")?.to_string(),
                from,
                to: replacement.to_string(),
                expansions: replacement.bytes().filter(|byte| *byte == b'$').count(),
            })
        })
        .collect::<Result<Vec<_>, String>>()
        .map(Step::SubstituteString)
}

fn compile_splits(name: &str, options: &Node) -> Result<Step, String> {
    entries_of(name, options)?
        .iter()
        .map(|entry| {
            reject_unknown_options(name, entry, &["source", "delimiter"])?;
            Ok(SplitEntry {
                source: required_str(name, entry, "source")?.to_string(),
                delimiter: option_str(name, entry, "delimiter")?.unwrap_or(",").to_string(),
            })
        })
        .collect::<Result<Vec<_>, String>>()
        .map(Step::SplitString)
}

/// Refuse an option a processor does not take, so nothing a configuration asks for is
/// silently dropped.
fn reject_unknown_options(name: &str, options: &Node, allowed: &[&str]) -> Result<(), String> {
    if matches!(options, Node::Null) {
        return Ok(());
    }
    let Some(entries) = options.as_mapping() else {
        return Err(format!("the {name} processor's options must be a mapping"));
    };
    for (key, _) in entries {
        if !allowed.contains(&key.as_str()) {
            return Err(format!(
                "the {name} processor takes no option '{key}'; it takes [{}]",
                allowed.join(", ")
            ));
        }
    }
    Ok(())
}

/// The `entries` sequence a per-field processor takes.
fn entries_of<'a>(name: &str, options: &'a Node) -> Result<&'a [Node], String> {
    let Some(entries) = options.get("entries") else {
        return Err(format!("the {name} processor takes an 'entries' sequence"));
    };
    let entries = entries
        .as_sequence()
        .ok_or_else(|| format!("the {name} processor's 'entries' must be a sequence"))?;
    if entries.len() > MAX_PROCESSOR_ENTRIES {
        return Err(format!(
            "the {name} processor declares {} entries, more than the {MAX_PROCESSOR_ENTRIES} \
             this emulator runs in one processor",
            entries.len()
        ));
    }
    Ok(entries)
}

fn option_str<'a>(name: &str, options: &'a Node, key: &str) -> Result<Option<&'a str>, String> {
    match options.get(key) {
        None | Some(Node::Null) => Ok(None),
        Some(node) => node
            .as_str()
            .map(Some)
            .ok_or_else(|| format!("the {name} processor's '{key}' must be text")),
    }
}

fn required_str<'a>(name: &str, options: &'a Node, key: &str) -> Result<&'a str, String> {
    option_str(name, options, key)?
        .ok_or_else(|| format!("the {name} processor's entries each take a '{key}'"))
}

fn option_bool(name: &str, options: &Node, key: &str) -> Result<Option<bool>, String> {
    match options.get(key) {
        None | Some(Node::Null) => Ok(None),
        Some(node) => match node.as_str() {
            Some("true" | "True" | "TRUE") => Ok(Some(true)),
            Some("false" | "False" | "FALSE") => Ok(Some(false)),
            _ => Err(format!(
                "the {name} processor's '{key}' must be true or false"
            )),
        },
    }
}

/// A single-character option — a delimiter or a quote character, which this format
/// takes one character of. A record terminator and a byte outside ASCII are refused:
/// the first would end the row the option is meant to separate, and the second is half
/// of a character rather than one.
fn option_char(name: &str, options: &Node, key: &str) -> Result<Option<u8>, String> {
    let Some(text) = option_str(name, options, key)? else {
        return Ok(None);
    };
    match text.as_bytes() {
        [byte] if byte.is_ascii() && !matches!(byte, b'\n' | b'\r') => Ok(Some(*byte)),
        _ => Err(format!(
            "the {name} processor's '{key}' must be one ASCII character other than a line \
             break, got '{text}'"
        )),
    }
}

fn option_keys(name: &str, options: &Node, key: &str) -> Result<Option<Vec<String>>, String> {
    match options.get(key) {
        None | Some(Node::Null) => Ok(None),
        Some(node) => {
            let items = node
                .as_sequence()
                .ok_or_else(|| format!("the {name} processor's '{key}' must be a sequence"))?;
            items
                .iter()
                .map(|item| {
                    item.as_str().map(str::to_string).ok_or_else(|| {
                        format!("the {name} processor's '{key}' must be a sequence of names")
                    })
                })
                .collect::<Result<Vec<_>, String>>()
                .map(Some)
        }
    }
}

fn required_keys(name: &str, options: &Node, key: &str) -> Result<Vec<String>, String> {
    option_keys(name, options, key)?
        .ok_or_else(|| format!("the {name} processor takes a '{key}' sequence"))
}

// =============================================================================
// Running one record through the chain
// =============================================================================

/// What one value of a record is charged beyond its own text: the bookkeeping a value
/// and its string hold, so a field of many short strings is accounted for what it
/// really occupies rather than for the characters alone.
const VALUE_OVERHEAD: usize = 32;

/// One record as the chain holds it, with the bytes it holds ACCOUNTED as it is
/// mutated: every write goes through [`PipelineRecord::insert`], which checks the
/// record's new size against [`MAX_EVENT_BYTES`] BEFORE the value is stored, so no
/// step can grow a record past the bound and have it noticed afterwards.
///
/// It also carries whether the record came in as raw text: a record that came in as
/// text and still holds nothing but its message is reported back as text, and
/// everything else as the JSON document it is.
#[derive(Debug)]
struct PipelineRecord {
    fields: Map<String, Value>,
    from_text: bool,
    /// The accounted size of [`Self::fields`], kept in step with every mutation.
    bytes: usize,
}

impl PipelineRecord {
    /// Read one sample record into the chain. A `JSON` record is the document it
    /// carries; a `STRING` record (the form a record that names no `Type` is in) is
    /// its text in the `message` field every field-reading processor of this format
    /// defaults to.
    fn read(record: &Record) -> Result<Self, String> {
        let data = record.data.as_deref().unwrap_or_default();
        if record.r#type.as_deref() == Some(RECORD_FORMAT_JSON) {
            let value: Value = ::serde_json::from_str(data)
                .map_err(|why| format!("the record is not {RECORD_FORMAT_JSON}: {why}"))?;
            return match value {
                Value::Object(fields) => Ok(Self {
                    bytes: fields_size(&fields),
                    fields,
                    from_text: false,
                }),
                _ => Err(format!(
                    "a {RECORD_FORMAT_JSON} record must carry a JSON object"
                )),
            };
        }
        let mut fields = Map::new();
        fields.insert(MESSAGE_FIELD.to_string(), Value::String(data.to_string()));
        Ok(Self {
            bytes: fields_size(&fields),
            fields,
            from_text: true,
        })
    }

    fn get(&self, key: &str) -> Option<&Value> {
        self.fields.get(key)
    }

    /// The text of `key`, for a field that carries text — what every field-reading
    /// processor works on.
    fn text(&self, key: &str) -> Option<&str> {
        self.fields.get(key).and_then(Value::as_str)
    }

    fn contains(&self, key: &str) -> bool {
        self.fields.contains_key(key)
    }

    /// Write `value` under `key`, refusing the record BEFORE the write when it would
    /// take it past [`MAX_EVENT_BYTES`]. This is the ONLY way a step grows a record,
    /// so the bound dominates every growth site inside a step.
    fn insert(&mut self, key: String, value: Value) -> Result<(), String> {
        let charge = |key: &str, value: &Value| {
            key.len()
                .saturating_add(VALUE_OVERHEAD)
                .saturating_add(value_size(value))
        };
        let replaced = self.fields.get(&key).map_or(0, |existing| {
            charge(key.as_str(), existing)
        });
        let projected = self
            .bytes
            .saturating_sub(replaced)
            .saturating_add(charge(key.as_str(), &value));
        if projected > MAX_EVENT_BYTES {
            return Err(grown_too_large());
        }
        self.fields.insert(key, value);
        self.bytes = projected;
        Ok(())
    }

    /// Take the value of `key` out of the record, so a step that moves a field moves
    /// the value it already holds instead of copying it.
    fn take(&mut self, key: &str) -> Option<Value> {
        let value = self.fields.remove(key)?;
        self.bytes = self.bytes.saturating_sub(
            key.len()
                .saturating_add(VALUE_OVERHEAD)
                .saturating_add(value_size(&value)),
        );
        Some(value)
    }

    fn remove(&mut self, key: &str) {
        self.take(key);
    }

    /// The bytes a step may still add to this record.
    fn headroom(&self) -> usize {
        MAX_EVENT_BYTES.saturating_sub(self.bytes)
    }

    /// The record as the response reports it.
    fn written(&self) -> Result<Record, String> {
        let bare_message = (self.from_text && self.fields.len() == 1)
            .then(|| self.fields.get(MESSAGE_FIELD).and_then(Value::as_str))
            .flatten();
        if let Some(text) = bare_message {
            return Ok(Record {
                data: Some(text.to_string()),
                r#type: Some(RECORD_FORMAT_STRING.to_string()),
            });
        }
        let data = ::serde_json::to_string(&self.fields)
            .map_err(|why| format!("the processed record cannot be reported: {why}"))?;
        Ok(Record {
            data: Some(data),
            r#type: Some(RECORD_FORMAT_JSON.to_string()),
        })
    }
}

/// Why a record is not processed any further: a step would have taken it past the size
/// one record is processed into.
fn grown_too_large() -> String {
    format!(
        "the record would grow past the {MAX_EVENT_BYTES} bytes this emulator processes one \
         record into"
    )
}

/// What a record's fields are accounted as.
fn fields_size(fields: &Map<String, Value>) -> usize {
    fields.iter().fold(0usize, |total, (key, value)| {
        total
            .saturating_add(key.len())
            .saturating_add(VALUE_OVERHEAD)
            .saturating_add(value_size(value))
    })
}

/// What one value is accounted as. Walked over an explicit stack, so no caller-chosen
/// depth recurses, and stops counting once the bound is already exceeded.
fn value_size(value: &Value) -> usize {
    let mut total = 0usize;
    let mut pending: Vec<&Value> = vec![value];
    while let Some(value) = pending.pop() {
        total = total.saturating_add(match value {
            Value::Null => 4,
            Value::Bool(_) => 5,
            Value::Number(number) => number.to_string().len(),
            Value::String(text) => text.len(),
            Value::Array(items) => {
                pending.extend(items.iter());
                items.len().saturating_mul(VALUE_OVERHEAD)
            }
            Value::Object(fields) => {
                let mut keys = 0usize;
                for (key, nested) in fields {
                    keys = keys
                        .saturating_add(key.len())
                        .saturating_add(VALUE_OVERHEAD);
                    pending.push(nested);
                }
                keys
            }
        });
        if total > MAX_EVENT_BYTES {
            break;
        }
    }
    total
}

/// Run one record through the compiled chain, answering the record the chain left or
/// why it could not run it. Either way it is THIS record's outcome: the records of one
/// call are independent of each other, as a pipeline processing them one at a time
/// makes them.
///
/// `work` is the call's REMAINING work budget, shared by every record: a step charges
/// it the bytes it scans before scanning them, so the product of the records, the steps
/// and the bytes each step reads is bounded for the whole request and not only per
/// step. The chain yields between steps, so one record's steps cannot hold an async
/// worker either.
async fn run_chain(
    steps: &[Step],
    record: &Record,
    work: &mut usize,
) -> Result<Record, String> {
    let mut current = PipelineRecord::read(record)?;
    for step in steps {
        run_step(step, &mut current, work)?;
        ::tokio::task::yield_now().await;
    }
    current.written()
}

/// Charge `bytes` of scanning to the call's remaining work budget, refusing the record
/// when the budget cannot cover it. Charged BEFORE the work is done.
fn charge(work: &mut usize, bytes: usize) -> Result<(), String> {
    let cost = bytes.max(1);
    if *work < cost {
        return Err(format!(
            "this call has spent the {MAX_WORK_BYTES} bytes of record processing this emulator \
             runs in one call"
        ));
    }
    *work = work.saturating_sub(cost);
    Ok(())
}

fn run_step(
    step: &Step,
    record: &mut PipelineRecord,
    work: &mut usize,
) -> Result<(), String> {
    match step {
        Step::Csv(csv) => run_csv(csv, record, work),
        Step::ParseJson(parse) => run_parse_json(parse, record, work),
        Step::AddEntries(entries) => {
            for entry in entries {
                if entry.overwrite_if_key_exists || !record.contains(&entry.key) {
                    record.insert(entry.key.clone(), entry.value.clone())?;
                }
            }
            Ok(())
        }
        Step::DeleteEntries(keys) => {
            for key in keys {
                record.remove(key);
            }
            Ok(())
        }
        Step::RenameKeys(moves) => {
            for key_move in moves {
                // A rename onto the key the value already sits under moves nothing, so
                // it is not a way to make the record be walked again.
                if key_move.from_key == key_move.to_key {
                    continue;
                }
                if !key_move.overwrite_if_to_key_exists && record.contains(&key_move.to_key) {
                    continue;
                }
                // The value is MOVED, not copied; what the move costs is charged to
                // the call's budget before the record is written again.
                if let Some(value) = record.take(&key_move.from_key) {
                    charge(work, value_size(&value))?;
                    record.insert(key_move.to_key.clone(), value)?;
                }
            }
            Ok(())
        }
        Step::CopyValues(moves) => {
            for key_move in moves {
                if key_move.from_key == key_move.to_key {
                    continue;
                }
                if !key_move.overwrite_if_to_key_exists && record.contains(&key_move.to_key) {
                    continue;
                }
                if let Some(value) = record.get(&key_move.from_key) {
                    charge(work, value_size(value))?;
                    let value = value.clone();
                    record.insert(key_move.to_key.clone(), value)?;
                }
            }
            Ok(())
        }
        Step::MapStrings(map, keys) => {
            for key in keys {
                if let Some(text) = record.text(key) {
                    // Case mapping is at most a few bytes per character, so the
                    // mapped text is charged by the insert below before it is stored.
                    charge(work, text.len())?;
                    let mapped = map.apply(text);
                    record.insert(key.clone(), Value::String(mapped))?;
                }
            }
            Ok(())
        }
        Step::SubstituteString(substitutions) => {
            for substitution in substitutions {
                let Some(text) = record.text(&substitution.source) else {
                    continue;
                };
                // The scan is charged to the call's budget before it runs, and the
                // replacement is built INCREMENTALLY against the bytes this record may
                // still hold, so a pattern matching at every position with a long
                // replacement is refused while the string is being built rather than
                // after a large one has been allocated.
                charge(work, text.len())?;
                let budget = record
                    .headroom()
                    .saturating_add(value_size(&Value::String(text.to_string())));
                let substituted = substitute_bounded(substitution, text, budget)?;
                record.insert(substitution.source.clone(), Value::String(substituted))?;
            }
            Ok(())
        }
        Step::SplitString(splits) => {
            for split in splits {
                let Some(text) = record.text(&split.source) else {
                    continue;
                };
                charge(work, text.len())?;
                // Accounted part by part, so a field of bare delimiters cannot build a
                // huge array before the bound is consulted.
                let mut parts: Vec<Value> = Vec::new();
                let mut charged = 0usize;
                for part in text.split(split.delimiter.as_str()) {
                    charged = charged
                        .saturating_add(part.len())
                        .saturating_add(VALUE_OVERHEAD);
                    if charged > MAX_EVENT_BYTES {
                        return Err(grown_too_large());
                    }
                    parts.push(Value::String(part.to_string()));
                }
                record.insert(split.source.clone(), Value::Array(parts))?;
            }
            Ok(())
        }
    }
}

/// One `substitute_string` entry applied to `text`, built into a string that is never
/// allowed past `budget` bytes.
///
/// `Captures::expand` is what gives the replacement the same `$name` / `${name}`
/// expansion `Regex::replace_all` would, so the result is the crate's own semantics
/// with the size bound added. The bound is charged BEFORE each match is expanded: a
/// group reference expands to at most the whole match, so the worst case this match can
/// add is the replacement's own length plus one match length per reference it carries,
/// and a match whose worst case does not fit the remaining budget stops the
/// substitution and becomes this record's error rather than being allocated first.
fn substitute_bounded(
    substitution: &Substitution,
    text: &str,
    budget: usize,
) -> Result<String, String> {
    let mut out = String::new();
    let mut last = 0usize;
    for captures in substitution.from.captures_iter(text) {
        let Some(whole) = captures.get(0) else {
            continue;
        };
        let worst_case = out
            .len()
            .saturating_add(whole.start().saturating_sub(last))
            .saturating_add(substitution.to.len())
            .saturating_add(substitution.expansions.saturating_mul(whole.len()));
        if worst_case > budget {
            return Err(grown_too_large());
        }
        out.push_str(text.get(last..whole.start()).unwrap_or_default());
        captures.expand(substitution.to.as_str(), &mut out);
        last = whole.end();
        if out.len() > budget {
            return Err(grown_too_large());
        }
    }
    out.push_str(text.get(last..).unwrap_or_default());
    if out.len() > budget {
        return Err(grown_too_large());
    }
    Ok(out)
}

/// The `csv` processor: read the source field as one CSV row and put each column in
/// the record under its column name, in place of the text they were read from — the
/// field the processor parsed is consumed by the fields it produced. The column names
/// are the header the `column_names_source_key` field carries when the configuration
/// names one, else the configured `column_names`; a column past the last name is
/// `column_<N>` for its 1-based position, which is also how every column is named when
/// no name is configured at all.
fn run_csv(step: &CsvStep, record: &mut PipelineRecord, work: &mut usize) -> Result<(), String> {
    let Some(text) = record.text(&step.source) else {
        return Err(format!(
            "the csv processor's source field '{}' carries no text to parse",
            step.source
        ));
    };
    charge(work, text.len())?;
    let row = parse_csv_row(text, step.delimiter, step.quote)?;

    let mut names: Option<Vec<String>> = None;
    if let Some(header_key) = &step.column_names_source_key {
        match record.get(header_key) {
            Some(Value::String(header)) => {
                // The header is a field read like any other, so what reading it costs
                // is charged to the call's budget before it is read.
                let header = header.clone();
                charge(work, header.len())?;
                names = Some(parse_csv_row(&header, step.delimiter, step.quote)?);
            }
            Some(value @ Value::Array(_)) => {
                charge(work, value_size(value))?;
                let Some(items) = value.as_array() else {
                    return Err("the csv processor's column names are not a list".to_string());
                };
                names = Some(
                    items
                        .iter()
                        .map(|item| match item {
                            Value::String(text) => text.clone(),
                            other => other.to_string(),
                        })
                        .collect(),
                );
            }
            // The field the header would come from is not in this record, so the
            // configured names (or the generated ones) are what this row is read with.
            _ => {}
        }
    }
    // The configured names are borrowed rather than cloned per record: a record only
    // needs to READ them.
    let names = names.as_deref().or(step.column_names.as_deref());

    // The text is consumed by the columns read out of it, so a column may carry the
    // source field's own name without the removal taking it away again.
    record.remove(&step.source);
    for (index, column) in row.into_iter().enumerate() {
        // A column the configuration named nothing is named by its 1-based position —
        // `column_1`, `column_2`, … — which is the naming this service answers for a
        // row read with no configured column names.
        let name = names
            .and_then(|names| names.get(index))
            .cloned()
            .unwrap_or_else(|| format!("column_{}", index.saturating_add(1)));
        record.insert(name, Value::String(column))?;
    }
    if let Some(header_key) = step
        .column_names_source_key
        .as_ref()
        .filter(|_| step.delete_header)
    {
        record.remove(header_key);
    }
    Ok(())
}

/// One CSV row, read by the `csv` crate rather than split by hand, so a quoted field
/// carrying the delimiter, a newline or an escaped quote is ONE field as CSV defines
/// it. A row is read `flexible`, because a test row is not measured against a header.
fn parse_csv_row(text: &str, delimiter: u8, quote: u8) -> Result<Vec<String>, String> {
    let mut reader = ::csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .delimiter(delimiter)
        .quote(quote)
        .from_reader(text.as_bytes());
    match reader.records().next() {
        Some(Ok(row)) => Ok(row.iter().map(str::to_string).collect()),
        Some(Err(why)) => Err(format!("the csv processor cannot read the record: {why}")),
        // An empty source field is no row at all, which adds no column.
        None => Ok(Vec::new()),
    }
}

/// The `parse_json` processor: read the source field's text as a JSON document and
/// merge it into the record, or put it under `destination` when one is configured,
/// consuming the field it parsed the same way the `csv` processor does. A `pointer`
/// selects the part of the parsed document to take, as JSON Pointer (RFC 6901)
/// addresses it.
fn run_parse_json(
    step: &ParseJsonStep,
    record: &mut PipelineRecord,
    work: &mut usize,
) -> Result<(), String> {
    let Some(text) = record.text(&step.source) else {
        return Err(format!(
            "the parse_json processor's source field '{}' carries no text to parse",
            step.source
        ));
    };
    charge(work, text.len())?;
    let parsed: Value = ::serde_json::from_str(text)
        .map_err(|why| format!("the parse_json processor cannot read the record: {why}"))?;
    let selected = match &step.pointer {
        None => parsed,
        Some(pointer) => parsed
            .pointer(pointer)
            .cloned()
            .ok_or_else(|| format!("the parsed record carries nothing at '{pointer}'"))?,
    };
    record.remove(&step.source);
    match &step.destination {
        Some(destination) => record.insert(destination.clone(), selected)?,
        None => match selected {
            Value::Object(parsed_fields) => {
                for (key, value) in parsed_fields {
                    record.insert(key, value)?;
                }
            }
            _ => {
                return Err(
                    "the parse_json processor needs a destination for a record that is not a \
                     JSON object"
                        .to_string(),
                )
            }
        },
    }
    Ok(())
}

// =============================================================================
// The operation
// =============================================================================

/// The `TestTelemetryPipeline` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct TestTelemetryPipeline<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> TestTelemetryPipeline<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for TestTelemetryPipeline<Eng>
{
    type Input = TestTelemetryPipelineInput;
    type Output = TestTelemetryPipelineOutput;
    type Error = TestTelemetryPipelineError;

    const NAME: &'static str = "TestTelemetryPipeline";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::TEST_TELEMETRY_PIPELINE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::TEST_TELEMETRY_PIPELINE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // `Configuration.Body`'s 1..24000 characters are modelled, so the centralized
        // request walk has already refused a body outside them. Everything below is a
        // rule the model cannot express.
        //
        // The engine is NOT reached at all: this call "[t]ests a pipeline
        // configuration with sample records", creating no pipeline and reading no
        // stored one, so it answers from the request alone and leaves the store as it
        // found it. `self.engine` stays bound for the constructor seam.
        let _ = &self.engine;

        // `SignalType` is a modelled enum with a default, so an absent one is the
        // default rather than a refusal. It names which telemetry the records are, not
        // how a processor reads a field, so the chain below runs the same way for
        // either value.
        let signal_type = if input.signal_type.is_empty() {
            DEFAULT_SIGNAL_TYPE
        } else {
            input.signal_type.as_str()
        };
        if let Some(why) =
            crate::state::enum_rejection("SignalType", Some(signal_type), SIGNAL_TYPE_VALUES)
        {
            return Err(validation(why));
        }

        // Every bound is applied BEFORE any record is read, so a request past one
        // costs the size of the request and nothing more.
        if input.records.len() > MAX_TEST_RECORDS {
            return Err(validation(format!(
                "Records carries {} records, more than the {MAX_TEST_RECORDS} this emulator \
                 processes in one call",
                input.records.len()
            )));
        }
        let mut total_bytes = 0usize;
        for (index, record) in input.records.iter().enumerate() {
            if let Some(why) = crate::state::enum_rejection(
                &format!("Records[{index}].Type"),
                record.r#type.as_deref(),
                RECORD_FORMAT_VALUES,
            ) {
                return Err(validation(why));
            }
            let bytes = record.data.as_deref().unwrap_or_default().len();
            if bytes > MAX_RECORD_DATA_BYTES {
                return Err(validation(format!(
                    "Records[{index}].Data carries {bytes} bytes, more than the \
                     {MAX_RECORD_DATA_BYTES} this emulator processes in one record"
                )));
            }
            total_bytes = total_bytes.saturating_add(bytes);
        }
        if total_bytes > MAX_TOTAL_RECORD_BYTES {
            return Err(validation(format!(
                "Records carries {total_bytes} bytes of data, more than the \
                 {MAX_TOTAL_RECORD_BYTES} this emulator processes in one call"
            )));
        }

        // The configuration is read and compiled ONCE: a defect in it is a refusal of
        // the request, not an error against every record.
        let configuration = parse_configuration(&input.configuration.body).map_err(validation)?;
        let chain = processor_chain(&configuration).map_err(validation)?;
        let steps = compile_chain(&chain).map_err(validation)?;

        let mut results: Vec<PipelineOutput> = Vec::with_capacity(input.records.len());
        // The work budget is the call's, not the record's: it bounds the product of the
        // records, the chain's steps and the bytes each step reads, which no per-step
        // bound can.
        let mut work = MAX_WORK_BYTES;
        for record in &input.records {
            match run_chain(&steps, record, &mut work).await {
                Ok(processed) => results.push(PipelineOutput {
                    record: Some(processed),
                    error: None,
                }),
                Err(why) => results.push(PipelineOutput {
                    record: None,
                    error: Some(PipelineOutputError { message: Some(why) }),
                }),
            }
            // A batch is bounded, but it is still caller-sized work: yielding between
            // records leaves a cancellation and a shutdown somewhere to land.
            ::tokio::task::yield_now().await;
        }

        // The presented access-key id is an identifier, never a secret: it is what
        // attributes this call. Neither the configuration body nor a record's data is
        // in the record below — only what was called and how much of it.
        ::tracing::info!(
            target: "audit",
            operation = Self::NAME,
            outcome = "tested",
            principal = %::emulator_server::extract_akid(&ctx.parts().headers),
            region = %crate::state::request_region(ctx.parts()),
            signal_type = %signal_type,
            records = input.records.len(),
            processors = steps.len(),
            "tested a telemetry pipeline configuration"
        );

        Ok(TestTelemetryPipelineOutput {
            results: Some(results),
        })
    }
}

/// The modelled 400 this operation refuses a request with. The model declares
/// `ValidationException` for it — "Indicates input validation failed. Check your
/// request parameters and retry the request." — so the refusal carries that shape
/// rather than the scaffold's synthetic one.
fn validation(message: impl Into<String>) -> TestTelemetryPipelineError {
    TestTelemetryPipelineErrorModeled::ValidationException(ValidationException {
        message: Some(message.into()),
        ..Default::default()
    })
    .into()
}
