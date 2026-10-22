// SPDX-License-Identifier: CC0-1.0

//! The CloudFormation template a stack carries: reading it, and evaluating what it
//! declares.
//!
//! A stack's `Outputs`, and the identifiers a client reads out of them, are not request
//! input — they are COMPUTED from the template body the stack stores:
//!
//! 1. [`parse`] reads the body. CloudFormation accepts a template as JSON *or* YAML
//!    ("You can write CloudFormation templates in JSON or YAML"), and the YAML form may
//!    spell an intrinsic function with its short-form tag (`!Ref x` is documented as the
//!    short form of `{"Ref": "x"}`), so the YAML reader turns every short-form tag back
//!    into the long form before anything looks at the tree. One `serde_json::Value`
//!    therefore represents either spelling.
//! 2. [`provision`] turns each `Resources` entry into a [`StoredResource`]: its physical
//!    resource id and the attributes `Fn::GetAtt` reads off it, per the "Return values"
//!    section the resource-type reference publishes for that type.
//! 3. [`evaluate_outputs`] evaluates each `Outputs` entry's `Value` against the stack's
//!    parameters, its resources and the pseudo parameters, which is what
//!    `DescribeStacks` reports as `Stack$Outputs`.
//!
//! ## What this does NOT do
//!
//! Creating the resource in the service that owns it (an `AWS::SNS::Topic` in Amazon
//! SNS) is CloudFormation's resource-provisioning layer, which this emulator does not
//! have: a stack records the resources its template declares and the identifiers
//! CloudFormation's documented naming rules give them, and nothing here reports a status,
//! an error or a value for an upstream call that never ran.
//!
//! An expression this module cannot evaluate (an intrinsic function it does not
//! implement, a `Ref` to a name the template never declares) yields NO value rather than
//! a guessed one: the output is reported with its key and description and without an
//! `OutputValue`, because `Output$OutputValue` is optional and a made-up ARN is worse
//! than an absent one.
//!
//! ## Bounds
//!
//! The template body is caller-supplied, so a single `CreateStack`/`UpdateStack` request
//! must not be able to spend unbounded CPU, stack or memory here. Three independent
//! limits do that, and each refuses (yields no value) rather than recursing, allocating
//! or looping further:
//!
//! * **Before the YAML loader sees the body** — [`guard_yaml_body`] bounds its length and
//!   the NESTING the recursive loader can descend into (flow collections open at once,
//!   compact indicators per line, leading indentation), none of which grows with how big
//!   a flat template is. The loader's own recursion is NOT abortable from an
//!   `EventReceiver` (`on_event` returns `()`), and exhausting the stack aborts the
//!   process rather than unwinding, so that nesting has to be refused BEFORE `load` is
//!   called. The JSON spelling does not go through it at all: `serde_json` reads it under
//!   its own nesting limit.
//! * **While the tree is built** — [`MAX_TEMPLATE_NODES`] nodes and
//!   [`MAX_TEMPLATE_DEPTH`] levels, plus the documented per-template quotas on
//!   [resources](MAX_RESOURCES_PER_TEMPLATE), [outputs](MAX_OUTPUTS_PER_TEMPLATE) and
//!   [parameters](MAX_PARAMETERS_PER_TEMPLATE): a body past one of those is not a template
//!   CloudFormation would accept, and this reader answers `None` for it rather than
//!   reporting a truncated subset of it.
//! * **While expressions are evaluated** — ONE [`Budget`] per deployment bounds the TOTAL
//!   work of evaluating every resource name and every output of a template, counting both
//!   the nodes visited and the bytes they produce, so no composition of functions can
//!   multiply the cost of its parts. [`MAX_INTRINSIC_DEPTH`], [`MAX_RESOLVED_LIST_LEN`]
//!   and [`MAX_RESOLVED_TEXT_BYTES`] bound each single expression in addition, and
//!   `Fn::Sub` resolves each of its variables ONCE per substitution however many times its
//!   source names it.

use std::collections::BTreeMap;

use ::serde_json::{Map, Value};

use super::{StoredOutput, StoredResource};

/// The most nodes one parsed template body may hold.
///
/// Chosen: a 51,200-byte body (the documented `TemplateBody` maximum) cannot hold more
/// than this many YAML nodes in any realistic template, so the cap only ever stops a body
/// built to explode the parse.
pub const MAX_TEMPLATE_NODES: usize = 20_000;

/// The most levels one parsed template body may nest. Chosen: far past the nesting of a
/// template's `Resources` → properties → intrinsic trees.
pub const MAX_TEMPLATE_DEPTH: usize = 32;

/// The most resources one template declares.
///
/// The documented CloudFormation quota ("Resources — maximum number of resources that you
/// can declare in a template: 500"), used here as the bound on the per-stack resource
/// records this module materialises.
pub const MAX_RESOURCES_PER_TEMPLATE: usize = 500;

/// The most outputs one template declares.
///
/// The documented CloudFormation quota ("Outputs — maximum number of outputs that you can
/// declare in a template: 200").
pub const MAX_OUTPUTS_PER_TEMPLATE: usize = 200;

/// The most parameters one template declares. The documented CloudFormation quota
/// ("Parameters — maximum number of parameters that you can declare in a template: 200").
pub const MAX_PARAMETERS_PER_TEMPLATE: usize = 200;

/// How deep one intrinsic-function expression may nest before it is left unevaluated.
/// Chosen: deeper than any `Fn::Join`/`Fn::Sub`/`Fn::GetAtt` composition a template
/// writes, and shallow enough that the evaluator's recursion is bounded by a constant.
pub const MAX_INTRINSIC_DEPTH: usize = 16;

/// The most elements an evaluated list value may carry (`Fn::Split`'s result, a
/// `Fn::Join` list). Chosen.
pub const MAX_RESOLVED_LIST_LEN: usize = 256;

/// The most bytes one evaluated value may carry. Chosen: `Output$OutputValue` carries an
/// identifier, not a payload, and this keeps a template that joins a body into itself
/// from growing one.
pub const MAX_RESOLVED_TEXT_BYTES: usize = 8 * 1024;

/// The total work one template deployment may do: the nodes its expressions evaluate,
/// weighted by the bytes they produce (see [`Budget`]).
///
/// Chosen so that the biggest template CloudFormation accepts evaluates comfortably
/// within it (200 outputs and 500 resource names, each a composition of a handful of
/// functions) while the WORST case stays a few milliseconds of work: at most
/// `MAX_EVALUATION_WORK` nodes and at most `MAX_EVALUATION_WORK * BYTES_PER_WORK_UNIT`
/// bytes of evaluated text per request. Bounding the TOTAL — rather than only each
/// expression's depth and size — is what keeps one function's cost from being multiplied
/// by another's: a composition that would re-evaluate its parts exponentially spends the
/// budget instead, and its output is reported without a value.
pub const MAX_EVALUATION_WORK: usize = 50_000;

/// How many bytes of evaluated text count as one unit of [`Budget`].
const BYTES_PER_WORK_UNIT: usize = 256;

/// The most leading spaces a line of a YAML template body may carry, which is what block
/// nesting costs per level ([`guard_yaml_body`]). Chosen: far past the indentation of a
/// template written at any sane indent step, and small enough that the loader's recursion
/// stays shallow.
pub const MAX_TEMPLATE_INDENT: usize = 256;

/// The most flow collections (`[`, `{`) a YAML template body may have OPEN at once —
/// closers subtracted — which is the nesting a well-formed body reaches. Chosen: far past
/// any template's `!Join`/`!GetAtt`/property nesting.
pub const MAX_TEMPLATE_FLOW_DEPTH: usize = 64;

/// The most flow openers (`[`, `{`) a YAML template body may carry in total.
///
/// The backstop for [`MAX_TEMPLATE_FLOW_DEPTH`]: a byte scan cannot tell a `]` inside a
/// quoted scalar from a real closer, and such a `]` lowers the running depth while the
/// loader's own depth keeps climbing — but no body can nest deeper than its opener COUNT.
/// Chosen as the deepest the loader is MEASURED to survive with room to spare (the
/// `a_template_body_built_to_explode_the_parse_is_read_as_no_template` suite drives a body
/// of just under this many openers with every closer quoted, which is the worst case this
/// bound permits), while sitting far above the flow collections a YAML template uses in
/// total — a template written entirely in flow style is JSON, which `serde_json` reads
/// instead and which this guard never sees.
pub const MAX_TEMPLATE_FLOW_OPENERS: usize = 2_048;

/// The most compact block indicators (`- `, `? `) one line of a YAML template body may
/// carry, which is where block nesting (`- - - x`) descends without costing a column. A
/// hyphen inside a name, an ARN or a description is not one of these (no space follows
/// it). Chosen.
pub const MAX_TEMPLATE_COMPACT_PER_LINE: usize = 256;

/// The work left for one template deployment to do.
///
/// One budget is created per deployment and threaded through every evaluation it performs,
/// so what is bounded is the TOTAL work of a request rather than each expression on its
/// own: nesting depth and value size bound the factors, and this bounds their product.
#[derive(Debug)]
pub struct Budget {
    remaining: usize,
}

impl Budget {
    /// A full budget, [`MAX_EVALUATION_WORK`] units.
    pub fn new() -> Self {
        Self {
            remaining: MAX_EVALUATION_WORK,
        }
    }

    /// Charge `units` of work, or `None` once the budget is spent — which is what makes an
    /// expression answer with no value instead of with unbounded work.
    fn spend(&mut self, units: usize) -> Option<()> {
        self.remaining = self.remaining.checked_sub(units)?;
        Some(())
    }

    /// Charge for producing `bytes` of evaluated text: one unit for the node plus one per
    /// [`BYTES_PER_WORK_UNIT`] bytes, so copying bytes is paid for as well as visiting
    /// nodes.
    fn spend_text(&mut self, bytes: usize) -> Option<()> {
        self.spend(1 + bytes / BYTES_PER_WORK_UNIT)
    }
}

impl Default for Budget {
    fn default() -> Self {
        Self::new()
    }
}

/// Pre-flight bounds on an untrusted YAML body, applied BEFORE the loader builds a tree
/// from it. Only the YAML spelling needs this: the JSON spelling is read by `serde_json`,
/// which enforces its own nesting limit.
///
/// What has to be bounded is how deep the loader can RECURSE, because an `EventReceiver`
/// cannot abort `load` (`on_event` returns `()`) and because exhausting the stack aborts
/// the process rather than unwinding. Three measures do that, and each is a bound on
/// nesting rather than a count of how big the document is, so a large, flat template is
/// not affected by any of them:
///
/// * [`MAX_TEMPLATE_FLOW_DEPTH`] — the flow collections (`[`, `{`) open at once, with
///   closers subtracted: the nesting a well-formed body actually reaches.
/// * [`MAX_TEMPLATE_FLOW_OPENERS`] — the total flow openers, which is the backstop for
///   the one way the measure above can be fooled: this is a byte scan, so a `]` inside a
///   scalar lowers the running depth while the loader's own depth keeps climbing. However
///   many closers a body hides, its real flow nesting cannot exceed its opener COUNT.
/// * [`MAX_TEMPLATE_INDENT`] columns of leading indentation and
///   [`MAX_TEMPLATE_COMPACT_PER_LINE`] compact `- `/`? ` indicators on one line — the two
///   ways block nesting descends (a column per level across lines, or a compact entry
///   within one line).
///
/// Nothing here counts a byte that does not open nesting: a hyphen inside a name, an ARN
/// or a description is not a `- ` indicator, and a brace in a long flow expression costs
/// depth only while it is open. Over-counting (a `[` inside a quoted scalar) is the only
/// failure mode, and each bound sits far above the nesting a CloudFormation template
/// written in YAML reaches.
fn guard_yaml_body(body: &str) -> Option<()> {
    if body.len() > super::TEMPLATE_BODY_MAX_BYTES {
        return None;
    }
    let mut flow_depth = 0usize;
    let mut flow_openers = 0usize;
    let mut compact_on_line = 0usize;
    let mut indent = 0usize;
    let mut counting_indent = true;
    let mut previous = b'\n';

    for byte in body.bytes() {
        match byte {
            b'\n' | b'\r' => {
                indent = 0;
                compact_on_line = 0;
                counting_indent = true;
            }
            b' ' | b'\t' if counting_indent => {
                indent += 1;
                if indent > MAX_TEMPLATE_INDENT {
                    return None;
                }
            }
            b'[' | b'{' => {
                counting_indent = false;
                flow_depth += 1;
                flow_openers += 1;
                if flow_depth > MAX_TEMPLATE_FLOW_DEPTH
                    || flow_openers > MAX_TEMPLATE_FLOW_OPENERS
                {
                    return None;
                }
            }
            b']' | b'}' => {
                counting_indent = false;
                flow_depth = flow_depth.saturating_sub(1);
            }
            _ => counting_indent = false,
        }
        // A `-` or `?` is a block indicator only where a token may start — at the
        // beginning of a line or after a space — and only when a space follows it, which
        // is what separates `- item` from the hyphen in `my-topic` or `a-b-c`.
        if matches!(previous, b'-' | b'?')
            && matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
            && compact_on_line_opener(&mut compact_on_line)
        {
            return None;
        }
        previous = byte;
    }
    Some(())
}

/// Count one compact block indicator on the current line, answering `true` once the line
/// is past [`MAX_TEMPLATE_COMPACT_PER_LINE`].
fn compact_on_line_opener(compact_on_line: &mut usize) -> bool {
    *compact_on_line += 1;
    *compact_on_line > MAX_TEMPLATE_COMPACT_PER_LINE
}

/// The partition every ARN this emulator mints belongs to, which is what
/// `{"Ref": "AWS::Partition"}` resolves to (`::emulator_server::emulator_arn` builds
/// `arn:aws:…`).
const PARTITION: &str = "aws";

/// What `{"Ref": "AWS::URLSuffix"}` resolves to in the `aws` partition.
const URL_SUFFIX: &str = "amazonaws.com";

/// How many hexadecimal characters an AWS Transfer Family resource identifier carries
/// after its one-letter prefix.
///
/// Transfer's own Smithy model fixes it: `ServerId` is `^s-([0-9a-f]{17})$` and
/// `WorkflowId` is `^w-([a-z0-9]{17})$`, both `length 19..19`.
const TRANSFER_ID_HEX_DIGITS: usize = 17;

/// One resource a template declares, as the template spells it.
#[derive(Debug, Clone)]
pub struct DeclaredResource {
    pub logical_id: String,
    /// `Type`, e.g. `AWS::SNS::Topic`.
    pub resource_type: String,
    /// `Properties`, with intrinsic functions still unevaluated.
    pub properties: Value,
    /// The WHOLE resource object the template declares, `Type` and every other
    /// `ResourceAttribute` (`Metadata`, `DeletionPolicy`, `UpdatePolicy`, …) included.
    ///
    /// A deployment only needs the type and the properties; comparing two templates needs
    /// the rest, because `ResourceChange$Scope` names the resource ATTRIBUTES that differ.
    pub declaration: Value,
}

/// One output a template declares, as the template spells it.
#[derive(Debug, Clone)]
pub struct DeclaredOutput {
    pub key: String,
    /// `Value`, with intrinsic functions still unevaluated.
    pub value: Value,
    pub description: Option<String>,
    /// `Export.Name`, with intrinsic functions still unevaluated.
    pub export_name: Option<Value>,
}

/// A template body, as this service reads it.
#[derive(Debug, Clone, Default)]
pub struct Template {
    /// The template's own `Description`, which is where CloudFormation takes
    /// `Stack$Description` from.
    pub description: Option<String>,
    /// `Parameters`, with each declared parameter's `Default` where it has a string one:
    /// the value a `Ref` to that parameter resolves to when the request supplied none.
    pub parameter_defaults: BTreeMap<String, String>,
    /// The `Mappings` section, which `Fn::FindInMap` reads.
    pub mappings: Value,
    pub resources: Vec<DeclaredResource>,
    pub outputs: Vec<DeclaredOutput>,
}

/// Read a template body, whether it is spelled as JSON or as YAML.
///
/// `None` for a body that is neither, for a YAML body past the nesting bounds
/// [`guard_yaml_body`] puts on an untrusted document, and for one past a documented
/// per-template quota — a caller may put anything in `TemplateBody`, and a body this
/// reader will not read declares no resources and no outputs rather than a subset of them.
pub fn parse(body: &str) -> Option<Template> {
    let document = document(body)?;
    template_of(&document)
}

/// The `Description` a template declares, if it declares one.
pub fn description(body: &str) -> Option<String> {
    parse(body).and_then(|template| template.description)
}

/// The template body as one JSON value: `serde_json` for a JSON body, the YAML reader
/// otherwise.
///
/// JSON is tried first because YAML 1.2 is a superset of JSON and `serde_json` reads the
/// JSON spelling exactly (and far more cheaply) than a YAML event walk would, under its
/// own nesting limit.
fn document(body: &str) -> Option<Value> {
    if let Ok(value) = ::serde_json::from_str::<Value>(body) {
        return Some(value);
    }
    yaml_document(body)
}

/// The sections of a parsed template this service reads, or `None` when a section is past
/// the documented quota on it (such a template is not one CloudFormation accepts, so
/// nothing here reports part of it).
fn template_of(document: &Value) -> Option<Template> {
    let mut template = Template {
        mappings: document
            .get("Mappings")
            .filter(|mappings| mappings.is_object())
            .cloned()
            .unwrap_or(Value::Null),
        ..Template::default()
    };
    template.description = document
        .get("Description")
        .and_then(Value::as_str)
        .filter(|description| !description.is_empty())
        .map(ToString::to_string);
    if let Some(parameters) = document.get("Parameters").and_then(Value::as_object) {
        if parameters.len() > MAX_PARAMETERS_PER_TEMPLATE {
            return None;
        }
        for (name, declaration) in parameters {
            // Only a string `Default` is a value a `Ref` can resolve to without knowing
            // the parameter's type; a `List<Number>` default is left to the request.
            if let Some(default) = declaration.get("Default").and_then(scalar_text) {
                template.parameter_defaults.insert(name.clone(), default);
            }
        }
    }
    if let Some(resources) = document.get("Resources").and_then(Value::as_object) {
        if resources.len() > MAX_RESOURCES_PER_TEMPLATE {
            return None;
        }
        for (logical_id, declaration) in resources {
            // "The Resources section declares the AWS resources that you want to include
            // in the stack … Type: required": an entry with no string `Type` declares no
            // resource.
            let Some(resource_type) = declaration.get("Type").and_then(Value::as_str) else {
                continue;
            };
            template.resources.push(DeclaredResource {
                logical_id: logical_id.clone(),
                resource_type: resource_type.to_string(),
                properties: declaration
                    .get("Properties")
                    .cloned()
                    .unwrap_or(Value::Null),
                declaration: declaration.clone(),
            });
        }
    }
    if let Some(outputs) = document.get("Outputs").and_then(Value::as_object) {
        if outputs.len() > MAX_OUTPUTS_PER_TEMPLATE {
            return None;
        }
        for (key, declaration) in outputs {
            // "Value (required): The value of the property returned by the
            // DescribeStacks operation."
            let Some(value) = declaration.get("Value") else {
                continue;
            };
            template.outputs.push(DeclaredOutput {
                key: key.clone(),
                value: value.clone(),
                description: declaration
                    .get("Description")
                    .and_then(Value::as_str)
                    .filter(|description| !description.is_empty())
                    .map(ToString::to_string),
                export_name: declaration
                    .get("Export")
                    .and_then(|export| export.get("Name"))
                    .cloned(),
            });
        }
    }
    Some(template)
}

/// A scalar as the text an intrinsic function substitutes for it. A number or a boolean
/// in a template is spelled back as the text CloudFormation substitutes; a list or an
/// object is not a scalar at all.
fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The YAML spelling of a template
// ---------------------------------------------------------------------------

/// Read a YAML template body into the same tree a JSON body parses to, expanding every
/// CloudFormation short-form intrinsic tag into its long form.
///
/// Built on the parser's EVENT stream rather than on a loaded tree for two reasons: the
/// loader drops the `!Ref`/`!GetAtt` tags that carry a template's whole meaning, and an
/// event walk sees a YAML alias (`*anchor`) BEFORE the loader copies the anchored node,
/// so an alias chain cannot be expanded here at all.
fn yaml_document(body: &str) -> Option<Value> {
    #[derive(Default)]
    struct Builder {
        /// The open collections, innermost last: each holds the items/entries read so
        /// far and the tag the collection itself carried.
        open: Vec<Frame>,
        /// The finished document, once the outermost node closes.
        root: Option<Value>,
        /// Set when the document is one this reader will not build: an alias, a second
        /// document, or a body past [`MAX_TEMPLATE_NODES`]/[`MAX_TEMPLATE_DEPTH`].
        refused: bool,
        nodes: usize,
    }

    enum Frame {
        Sequence {
            tag: Option<String>,
            items: Vec<Value>,
        },
        Mapping {
            tag: Option<String>,
            entries: Map<String, Value>,
            /// The key read but not yet paired with its value.
            pending_key: Option<String>,
        },
    }

    impl Builder {
        /// Place one finished node: as the root, as the next item of the open sequence,
        /// or as the key or the value of the open mapping.
        fn place(&mut self, node: Value) {
            self.nodes += 1;
            if self.nodes > MAX_TEMPLATE_NODES {
                self.refused = true;
                return;
            }
            if self.open.is_empty() {
                // A second document in the same stream: a template is one document.
                if self.root.is_some() {
                    self.refused = true;
                }
                self.root = Some(node);
                return;
            }
            let mut refuse = false;
            match self.open.last_mut().expect("a frame is open, just checked") {
                Frame::Sequence { items, .. } => items.push(node),
                Frame::Mapping {
                    entries,
                    pending_key,
                    ..
                } => match pending_key.take() {
                    // A mapping key is a scalar; a template never keys a section by a
                    // list or a map, and this reader does not invent a spelling for one.
                    None => match scalar_text(&node) {
                        Some(key) => *pending_key = Some(key),
                        None => refuse = true,
                    },
                    Some(key) => {
                        entries.insert(key, node);
                    }
                },
            }
            if refuse {
                self.refused = true;
            }
        }

        fn open_frame(&mut self, frame: Frame) {
            if self.open.len() >= MAX_TEMPLATE_DEPTH {
                self.refused = true;
                return;
            }
            self.open.push(frame);
        }

        fn close_frame(&mut self) {
            let Some(frame) = self.open.pop() else {
                self.refused = true;
                return;
            };
            let (tag, node) = match frame {
                Frame::Sequence { tag, items } => (tag, Value::Array(items)),
                Frame::Mapping { tag, entries, .. } => (tag, Value::Object(entries)),
            };
            let node = match tag {
                Some(tag) => long_form(&tag, node),
                None => node,
            };
            self.place(node);
        }
    }

    impl ::yaml_rust2::parser::EventReceiver for Builder {
        fn on_event(&mut self, event: ::yaml_rust2::parser::Event) {
            use ::yaml_rust2::parser::Event;
            if self.refused {
                return;
            }
            match event {
                Event::Scalar(value, style, _anchor, tag) => {
                    let quoted = !matches!(style, ::yaml_rust2::scanner::TScalarStyle::Plain);
                    let node = yaml_scalar(value, quoted);
                    let node = match short_form_tag(tag.as_ref()) {
                        Some(name) => long_form(&name, node),
                        None => node,
                    };
                    self.place(node);
                }
                Event::SequenceStart(_anchor, tag) => {
                    let tag = short_form_tag(tag.as_ref());
                    self.open_frame(Frame::Sequence {
                        tag,
                        items: Vec::new(),
                    });
                }
                Event::MappingStart(_anchor, tag) => {
                    let tag = short_form_tag(tag.as_ref());
                    self.open_frame(Frame::Mapping {
                        tag,
                        entries: Map::new(),
                        pending_key: None,
                    });
                }
                Event::SequenceEnd | Event::MappingEnd => self.close_frame(),
                // An alias copies an anchored node, which is how a few hundred bytes of
                // YAML expand into gigabytes. Nothing in a CloudFormation template needs
                // one, so a body carrying one is not read here.
                Event::Alias(_) => self.refused = true,
                _ => {}
            }
        }
    }

    let mut builder = Builder::default();
    guard_yaml_body(body)?;
    ::yaml_rust2::parser::Parser::new_from_str(body)
        .load(&mut builder, true)
        .ok()?;
    if builder.refused || !builder.open.is_empty() {
        return None;
    }
    builder.root
}

/// The intrinsic function a YAML tag names, if it names one.
///
/// A CloudFormation short-form tag is a LOCAL tag (handle `!`) whose suffix is the
/// function's name: `!Ref`, `!GetAtt`, `!Sub`, … A standard YAML tag (`!!str`, handle
/// `!!`) is not one, and neither is an unknown local tag.
fn short_form_tag(tag: Option<&::yaml_rust2::parser::Tag>) -> Option<String> {
    let tag = tag?;
    if tag.handle != "!" {
        return None;
    }
    SHORT_FORM_FUNCTIONS
        .iter()
        .find(|name| **name == tag.suffix)
        .map(|name| (*name).to_string())
}

/// Every intrinsic function (and condition function) CloudFormation documents a YAML
/// short form for, plus `Ref` and `Condition`.
///
/// Source: "Intrinsic function reference" and "Condition functions" in the
/// CloudFormation User Guide, which give each function's full name and note that "you can
/// use the short form `!Fn::…`" in YAML.
const SHORT_FORM_FUNCTIONS: [&str; 20] = [
    "And",
    "Base64",
    "Cidr",
    "Condition",
    "Equals",
    "FindInMap",
    "GetAZs",
    "GetAtt",
    "If",
    "ImportValue",
    "Join",
    "Length",
    "Not",
    "Or",
    "Ref",
    "Select",
    "Split",
    "Sub",
    "ToJsonString",
    "Transform",
];

/// One short-form tag as the long form a JSON template spells: `!Ref x` is
/// `{"Ref": "x"}`, `!Condition c` is `{"Condition": "c"}`, and every other function is
/// `{"Fn::<Name>": …}`.
fn long_form(name: &str, node: Value) -> Value {
    let key = match name {
        "Ref" | "Condition" => name.to_string(),
        other => format!("Fn::{other}"),
    };
    let mut wrapper = Map::new();
    wrapper.insert(key, node);
    Value::Object(wrapper)
}

/// One YAML scalar as a JSON value.
///
/// A QUOTED scalar is always text. A plain one resolves by YAML's core schema, so
/// `Outputs` that join a number or a boolean see the same value a JSON template would
/// give; anything else is text.
///
/// A number is only taken as a number when writing it back gives the SAME text the
/// template spelled: CloudFormation substitutes what the template says, so `007` and a
/// 20-digit account id stay the text they are rather than becoming `7` or an exponent.
fn yaml_scalar(value: String, quoted: bool) -> Value {
    if quoted {
        return Value::String(value);
    }
    match value.as_str() {
        "" | "~" | "null" | "Null" | "NULL" => return Value::Null,
        "true" | "True" | "TRUE" => return Value::Bool(true),
        "false" | "False" | "FALSE" => return Value::Bool(false),
        _ => {}
    }
    if let Some(number) = value
        .parse::<i64>()
        .ok()
        .map(|integer| Value::Number(integer.into()))
        .or_else(|| {
            value
                .parse::<f64>()
                .ok()
                .and_then(::serde_json::Number::from_f64)
                .map(Value::Number)
        })
        .filter(|number| scalar_text(number).as_deref() == Some(value.as_str()))
    {
        return number;
    }
    Value::String(value)
}

// ---------------------------------------------------------------------------
// Resources
// ---------------------------------------------------------------------------

/// The stack a template is being evaluated for: what the pseudo parameters resolve to,
/// and what `Ref`/`Fn::GetAtt` resolve against.
#[derive(Debug, Clone, Copy)]
pub struct StackScope<'a> {
    pub region: &'a str,
    pub stack_name: &'a str,
    pub stack_id: &'a str,
}

/// Materialise the resources a template declares.
///
/// Each resource gets the physical resource id CloudFormation's documented naming rules
/// give it — the name the template asked for where it names one, CloudFormation's
/// generated `<stack>-<logical id>-<suffix>` name where it does not — and the attributes
/// that type's "Return values" section publishes for `Fn::GetAtt`.
///
/// A name the template builds with an intrinsic function is evaluated first (the common
/// `TopicName: !Ref TopicName` case), against the stack's parameters and pseudo
/// parameters: the resources are what this function is producing, so a name that refers
/// to ANOTHER resource is not resolvable here and that resource takes the generated name.
pub fn provision(
    template: &Template,
    parameters: &BTreeMap<String, String>,
    scope: StackScope<'_>,
    budget: &mut Budget,
) -> Vec<StoredResource> {
    let no_resources = BTreeMap::new();
    let naming = Scope {
        stack: scope,
        parameters,
        resources: &no_resources,
        mappings: &template.mappings,
    };
    template
        .resources
        .iter()
        .map(|declared| {
            let name = physical_name(declared, &naming, budget);
            let (physical_resource_id, attributes) =
                return_values(&declared.resource_type, &name, scope);
            StoredResource {
                logical_id: declared.logical_id.clone(),
                resource_type: declared.resource_type.clone(),
                physical_resource_id,
                attributes,
            }
        })
        .collect()
}

/// The name a declared resource carries.
///
/// CloudFormation's own convention is that a resource type's name property is the type's
/// last segment plus `Name` (`TopicName` on `AWS::SNS::Topic`, `QueueName` on
/// `AWS::SQS::Queue`, `BucketName` on `AWS::S3::Bucket`), with `Name` itself used by the
/// newer types; where neither is declared, "CloudFormation generates a unique physical ID
/// and uses that ID for the resource name", in the form `<stack name>-<logical
/// id>-<unique id>`.
fn physical_name(
    declared: &DeclaredResource,
    naming: &Scope<'_>,
    budget: &mut Budget,
) -> String {
    let segment = declared
        .resource_type
        .rsplit("::")
        .next()
        .unwrap_or_default();
    let candidates = [format!("{segment}Name"), "Name".to_string()];
    let declared_name = candidates.iter().find_map(|property| {
        declared
            .properties
            .get(property)
            .and_then(|value| text(value, naming, 0, budget))
    });
    declared_name.unwrap_or_else(|| generated_name(&declared.logical_id, naming.stack))
}

/// The physical name CloudFormation generates for a resource the template does not name:
/// `<stack name>-<logical id>-<unique id>`.
///
/// The unique segment is derived from the stack id and the logical id rather than drawn
/// fresh, so it is stable across reads and across a later update of the same stack — the
/// physical id of a resource that was not replaced does not change.
fn generated_name(logical_id: &str, scope: StackScope<'_>) -> String {
    use ::sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(scope.stack_id.as_bytes());
    hasher.update(b"/");
    hasher.update(logical_id.as_bytes());
    let digest = ::hex::encode_upper(hasher.finalize());
    format!("{}-{logical_id}-{}", scope.stack_name, &digest[0..12])
}

/// A service-generated identifier of the form `<prefix>-<digits hexadecimal characters>`.
///
/// Some resource types do not take a name from the template at all: the owning service
/// ASSIGNS the identifier, in a documented form (an AWS Transfer Family server is
/// `s-` + 17 hexadecimal characters, which is the `ServerId` pattern
/// `^s-([0-9a-f]{17})$` in Transfer's own Smithy model). CloudFormation's
/// `<stack name>-<logical id>-<unique id>` name is not such an identifier, so a resource
/// of one of those types reports an identifier of ITS form instead — derived from the
/// name, which is itself derived from the stack id and the logical id, so the identifier
/// is stable across reads and across a later update of the same stack, and two resources
/// of one stack never collide.
fn generated_service_id(prefix: &str, name: &str, digits: usize) -> String {
    use ::sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(prefix.as_bytes());
    hasher.update(b"/");
    hasher.update(name.as_bytes());
    let digest = ::hex::encode(hasher.finalize());
    format!("{prefix}-{}", &digest[..digits])
}

/// What `Ref` returns for a resource of this type, and what `Fn::GetAtt` reads off it.
///
/// Source: the "Return values" section the AWS CloudFormation resource and property type
/// reference publishes for each type — `Ref` returns the topic ARN for
/// `AWS::SNS::Topic`, the queue URL for `AWS::SQS::Queue`, the bucket name for
/// `AWS::S3::Bucket`, and so on, with `Fn::GetAtt` reading the listed attributes. A type
/// this catalogue does not carry gets its physical NAME as its physical resource id,
/// which is what the majority of types return, and no attributes: `Fn::GetAtt` against it
/// yields no value rather than a guessed one.
///
/// An attribute whose documented return value is a LIST (an
/// `As2ServiceManagedEgressIpAddresses`, say) is not carried either: the attribute map is
/// string-valued, and reporting one element of a list as the whole of it would be worse
/// than reporting no value.
fn return_values(
    resource_type: &str,
    name: &str,
    scope: StackScope<'_>,
) -> (String, BTreeMap<String, String>) {
    let region = scope.region;
    let mut attributes = BTreeMap::new();
    match resource_type {
        // "Ref returns the topic ARN"; Fn::GetAtt: TopicArn, TopicName.
        "AWS::SNS::Topic" => {
            let arn = ::emulator_server::emulator_arn("sns", region, name);
            attributes.insert("TopicArn".to_string(), arn.clone());
            attributes.insert("TopicName".to_string(), name.to_string());
            (arn, attributes)
        }
        // "Ref returns the queue URL"; Fn::GetAtt: Arn, QueueName, QueueUrl.
        "AWS::SQS::Queue" => {
            let url = format!(
                "https://sqs.{region}.{URL_SUFFIX}/{}/{name}",
                ::emulator_server::emulator_account_id()
            );
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn("sqs", region, name),
            );
            attributes.insert("QueueName".to_string(), name.to_string());
            attributes.insert("QueueUrl".to_string(), url.clone());
            (url, attributes)
        }
        // "Ref returns the bucket name"; Fn::GetAtt: Arn, DomainName. An Amazon S3 bucket
        // ARN carries neither a region nor an account id (`arn:aws:s3:::<bucket>`).
        "AWS::S3::Bucket" => {
            attributes.insert("Arn".to_string(), format!("arn:{PARTITION}:s3:::{name}"));
            attributes.insert("DomainName".to_string(), format!("{name}.s3.{URL_SUFFIX}"));
            (name.to_string(), attributes)
        }
        // "Ref returns the table name"; Fn::GetAtt: Arn.
        "AWS::DynamoDB::Table" => {
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn("dynamodb", region, &format!("table/{name}")),
            );
            (name.to_string(), attributes)
        }
        // "Ref returns the function name"; Fn::GetAtt: Arn.
        "AWS::Lambda::Function" => {
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn("lambda", region, &format!("function:{name}")),
            );
            (name.to_string(), attributes)
        }
        // "Ref returns the log group name"; Fn::GetAtt: Arn. A CloudWatch Logs log-group
        // ARN ends in `:*` ("such as
        // arn:aws:logs:us-east-1:123456789012:log-group:/mystack-testgroup-12ABC1AB12A1:*").
        "AWS::Logs::LogGroup" => {
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn(
                    "logs",
                    region,
                    &format!("log-group:{name}:*"),
                ),
            );
            (name.to_string(), attributes)
        }
        // "Ref returns the Amazon Resource Name (ARN) of the server"; Fn::GetAtt: Arn,
        // ServerId ("the service-assigned ID of the server that is created") and the
        // list-valued As2ServiceManagedEgressIpAddresses, which is not carried. The
        // template names no server — AWS::Transfer::Server has no name property — so the
        // id is the service-assigned `s-` + 17 hexadecimal form.
        "AWS::Transfer::Server" => {
            let server_id = generated_service_id("s", name, TRANSFER_ID_HEX_DIGITS);
            let arn =
                ::emulator_server::emulator_arn("transfer", region, &format!("server/{server_id}"));
            attributes.insert("Arn".to_string(), arn.clone());
            attributes.insert("ServerId".to_string(), server_id);
            (arn, attributes)
        }
        // The same shape one type along: "Ref returns the Amazon Resource Name (ARN)";
        // Fn::GetAtt: Arn, WorkflowId. Transfer's `WorkflowId` is `w-` + 17 characters of
        // `[a-z0-9]` (model pattern `^w-([a-z0-9]{17})$`), which hexadecimal satisfies.
        "AWS::Transfer::Workflow" => {
            let workflow_id = generated_service_id("w", name, TRANSFER_ID_HEX_DIGITS);
            let arn = ::emulator_server::emulator_arn(
                "transfer",
                region,
                &format!("workflow/{workflow_id}"),
            );
            attributes.insert("Arn".to_string(), arn.clone());
            attributes.insert("WorkflowId".to_string(), workflow_id);
            (arn, attributes)
        }
        // "Ref returns the role name"; Fn::GetAtt: Arn. IAM is a global service, so its
        // ARNs carry no region.
        "AWS::IAM::Role" => {
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn("iam", "", &format!("role/{name}")),
            );
            (name.to_string(), attributes)
        }
        _ => (name.to_string(), attributes),
    }
}

// ---------------------------------------------------------------------------
// Outputs and intrinsic functions
// ---------------------------------------------------------------------------

/// Everything an intrinsic function resolves against.
#[derive(Debug, Clone, Copy)]
pub struct Scope<'a> {
    pub stack: StackScope<'a>,
    /// The parameter values in force on the stack, keyed by `ParameterKey`.
    pub parameters: &'a BTreeMap<String, String>,
    /// The stack's resources, keyed by logical id.
    pub resources: &'a BTreeMap<String, StoredResource>,
    /// The template's `Mappings` section.
    pub mappings: &'a Value,
}

/// Evaluate every output a template declares, in `OutputKey` order (the protocol fixes no
/// order for `Stack$Outputs`).
///
/// One entry per DECLARED output, whether or not its value could be evaluated: the output
/// exists because the template declares it, and [`StoredOutput::value`] is `None` where
/// this module cannot compute the value rather than carrying an invented one. The
/// `budget` is the deployment's — shared with [`provision`] — so the work of a whole
/// request is bounded, not just each output's.
pub fn evaluate_outputs(
    template: &Template,
    scope: &Scope<'_>,
    budget: &mut Budget,
) -> Vec<StoredOutput> {
    template
        .outputs
        .iter()
        .map(|declared| StoredOutput {
            key: declared.key.clone(),
            value: text(&declared.value, scope, 0, budget),
            description: declared.description.clone(),
            export_name: declared
                .export_name
                .as_ref()
                .and_then(|name| text(name, scope, 0, budget)),
        })
        .collect()
}

/// An evaluated expression: CloudFormation's intrinsic functions produce either a string
/// or a list of strings (`Fn::Split`, `Fn::GetAZs`), and the list form is only ever
/// consumed by another function.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Resolved {
    Text(String),
    List(Vec<String>),
}

/// Evaluate an expression to the text CloudFormation substitutes for it, or `None` when
/// this module cannot evaluate it — or when the deployment's work budget is spent.
fn text(
    value: &Value,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Option<String> {
    match evaluate(value, scope, depth, budget)? {
        Resolved::Text(text) => Some(text),
        // A list is not a value an output or a name can carry.
        Resolved::List(_) => None,
    }
}

/// Evaluate one expression.
fn evaluate(
    value: &Value,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Option<Resolved> {
    if depth > MAX_INTRINSIC_DEPTH {
        return None;
    }
    // One unit for visiting this node, before anything it might cost: this is what makes
    // the TOTAL work of a deployment finite however the template composes its functions.
    budget.spend(1)?;
    match value {
        Value::Null => None,
        Value::String(_) | Value::Number(_) | Value::Bool(_) => scalar_text(value)
            .map(Resolved::Text)
            .and_then(|resolved| bounded(resolved, budget)),
        Value::Array(items) => {
            if items.len() > MAX_RESOLVED_LIST_LEN {
                return None;
            }
            let mut resolved = Vec::with_capacity(items.len());
            for item in items {
                resolved.push(text(item, scope, depth + 1, budget)?);
            }
            bounded(Resolved::List(resolved), budget)
        }
        Value::Object(members) => {
            // An intrinsic function is an object with exactly one member, whose name is
            // the function's.
            let mut entries = members.iter();
            let (name, argument) = entries.next()?;
            if entries.next().is_some() {
                return None;
            }
            intrinsic(name, argument, scope, depth, budget)
                .and_then(|resolved| bounded(resolved, budget))
        }
    }
}

/// Refuse an evaluated value past [`MAX_RESOLVED_TEXT_BYTES`], and charge the deployment's
/// budget for the bytes it carries, so no composition of functions grows one — or copies
/// one — without limit.
fn bounded(resolved: Resolved, budget: &mut Budget) -> Option<Resolved> {
    let size = match &resolved {
        Resolved::Text(text) => text.len(),
        Resolved::List(items) => items.iter().map(String::len).sum(),
    };
    budget.spend_text(size)?;
    (size <= MAX_RESOLVED_TEXT_BYTES).then_some(resolved)
}

/// Evaluate one intrinsic function.
///
/// The functions this module does not implement — `Fn::ImportValue` (a cross-stack export
/// registry this emulator does not keep), `Fn::GetAZs` (the Availability Zones of an
/// account in a Region), `Fn::Base64`, `Fn::Cidr`, `Fn::ToJsonString`, `Fn::Length` and
/// the condition functions — yield no value, so an output that uses one is reported
/// without an `OutputValue`.
fn intrinsic(
    name: &str,
    argument: &Value,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Option<Resolved> {
    match name {
        // "The intrinsic function Ref returns the value of the specified parameter or
        // resource."
        "Ref" => reference(argument.as_str()?, scope).map(Resolved::Text),
        // "The Fn::GetAtt intrinsic function returns the value of an attribute from a
        // resource in the template": [logicalNameOfResource, attributeName], or the
        // short form's dotted string.
        "Fn::GetAtt" => attribute(argument, scope, depth, budget).map(Resolved::Text),
        // "Fn::Sub substitutes variables in an input string with values that you
        // specify": a string, or [string, {variables}].
        "Fn::Sub" => substitute(argument, scope, depth, budget).map(Resolved::Text),
        // "Fn::Join appends a set of values into a single value, separated by the
        // specified delimiter": [delimiter, [values]].
        "Fn::Join" => {
            let [delimiter, values] = pair(argument)?;
            let delimiter = text(delimiter, scope, depth + 1, budget)?;
            let values = list(values, scope, depth + 1, budget)?;
            Some(Resolved::Text(values.join(&delimiter)))
        }
        // "Fn::Select returns a single object from a list of objects by index":
        // [index, [objects]].
        "Fn::Select" => {
            let [index, values] = pair(argument)?;
            let index: usize = text(index, scope, depth + 1, budget)?.parse().ok()?;
            let values = list(values, scope, depth + 1, budget)?;
            values.get(index).cloned().map(Resolved::Text)
        }
        // "Fn::Split splits a string into a list of string values": [delimiter, string].
        "Fn::Split" => {
            let [delimiter, source] = pair(argument)?;
            let delimiter = text(delimiter, scope, depth + 1, budget)?;
            let source = text(source, scope, depth + 1, budget)?;
            if delimiter.is_empty() {
                return None;
            }
            let parts: Vec<String> = source
                .split(delimiter.as_str())
                .map(ToString::to_string)
                .collect();
            (parts.len() <= MAX_RESOLVED_LIST_LEN).then_some(Resolved::List(parts))
        }
        // "The intrinsic function Fn::FindInMap returns the value corresponding to keys
        // in a two-level map declared in the Mappings section":
        // [MapName, TopLevelKey, SecondLevelKey].
        "Fn::FindInMap" => {
            let keys = argument.as_array()?;
            if keys.len() < 3 {
                return None;
            }
            let map = text(&keys[0], scope, depth + 1, budget)?;
            let top = text(&keys[1], scope, depth + 1, budget)?;
            let second = text(&keys[2], scope, depth + 1, budget)?;
            let found = scope
                .mappings
                .get(map)?
                .get(top)?
                .get(second)
                .and_then(scalar_text)?;
            Some(Resolved::Text(found))
        }
        _ => None,
    }
}

/// The two arguments a binary intrinsic function takes.
fn pair(argument: &Value) -> Option<[&Value; 2]> {
    let arguments = argument.as_array()?;
    match arguments.len() {
        2 => Some([&arguments[0], &arguments[1]]),
        _ => None,
    }
}

/// Evaluate an expression to a list of strings.
fn list(
    value: &Value,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Option<Vec<String>> {
    match evaluate(value, scope, depth, budget)? {
        Resolved::List(items) => Some(items),
        // A function that produced one string where a list was expected (a `Ref` to a
        // `CommaDelimitedList` parameter, say) is not a list: CloudFormation's own typed
        // parameters are not modelled here, so this yields no value rather than
        // splitting a string nobody asked to split.
        Resolved::Text(_) => None,
    }
}

/// `{"Ref": "<name>"}`: a pseudo parameter, a template parameter, or a resource's
/// physical id.
fn reference(name: &str, scope: &Scope<'_>) -> Option<String> {
    // "CloudFormation provides pseudo parameters … use them the same way as you would a
    // parameter, as the argument for the Ref function."
    match name {
        "AWS::Region" => return Some(scope.stack.region.to_string()),
        "AWS::AccountId" => return Some(::emulator_server::emulator_account_id().to_string()),
        "AWS::StackName" => return Some(scope.stack.stack_name.to_string()),
        "AWS::StackId" => return Some(scope.stack.stack_id.to_string()),
        "AWS::Partition" => return Some(PARTITION.to_string()),
        "AWS::URLSuffix" => return Some(URL_SUFFIX.to_string()),
        // "AWS::NoValue … removes the corresponding resource property" — there is no
        // value to substitute.
        "AWS::NoValue" => return None,
        _ => {}
    }
    if let Some(value) = scope.parameters.get(name) {
        return Some(value.clone());
    }
    // "Ref returns the physical ID of the resource" for a logical id the template
    // declares.
    scope
        .resources
        .get(name)
        .map(|resource| resource.physical_resource_id.clone())
}

/// `{"Fn::GetAtt": ["<logical id>", "<attribute>"]}`, or the short form's
/// `<logical id>.<attribute>`.
fn attribute(
    argument: &Value,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Option<String> {
    let (logical_id, attribute) = match argument {
        Value::Array(parts) if parts.len() >= 2 => {
            let logical_id = text(&parts[0], scope, depth + 1, budget)?;
            // "Fn::GetAtt … attributeName … For attributes that return a list, use
            // dotted notation": every part past the first names one attribute path.
            let mut path = Vec::with_capacity(parts.len() - 1);
            for part in &parts[1..] {
                path.push(text(part, scope, depth + 1, budget)?);
            }
            (logical_id, path.join("."))
        }
        // The YAML short form spells both halves as one dotted string.
        Value::String(dotted) => {
            let (logical_id, attribute) = dotted.split_once('.')?;
            (logical_id.to_string(), attribute.to_string())
        }
        _ => return None,
    };
    scope
        .resources
        .get(&logical_id)?
        .attributes
        .get(&attribute)
        .cloned()
}

/// `{"Fn::Sub": "<string>"}` or `{"Fn::Sub": ["<string>", {"<var>": <value>}]}`.
///
/// "To write a dollar sign and curly braces (${}) literally, add an exclamation point (!)
/// after the open curly brace", and a `${Name}` that names neither a local variable nor
/// anything resolvable leaves the whole substitution unevaluated rather than substituting
/// an empty string.
///
/// Each variable the source names is evaluated ONCE however many times it appears
/// (`resolved` below memoises it for this substitution), so repeating `${a}` costs one
/// evaluation of `a` rather than one per occurrence; the deployment's budget bounds the
/// rest.
fn substitute(
    argument: &Value,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Option<String> {
    let (source, variables) = match argument {
        Value::String(source) => (source.clone(), Map::new()),
        Value::Array(parts) if parts.len() == 2 => (
            parts[0].as_str()?.to_string(),
            parts[1].as_object()?.clone(),
        ),
        _ => return None,
    };
    let mut resolved: BTreeMap<&str, String> = BTreeMap::new();
    let mut out = String::new();
    let mut rest = source.as_str();
    while let Some(start) = rest.find("${") {
        budget.spend_text(start)?;
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after.find('}')?;
        let name = &after[..end];
        rest = &after[end + 1..];
        // `${!Literal}` is the escape for a literal `${Literal}`.
        if let Some(literal) = name.strip_prefix('!') {
            out.push_str("${");
            out.push_str(literal);
            out.push('}');
        } else if let Some((key, local)) = variables.get_key_value(name) {
            let substitution = match resolved.get(key.as_str()) {
                Some(already) => already.clone(),
                None => {
                    let evaluated = text(local, scope, depth + 1, budget)?;
                    resolved.insert(key.as_str(), evaluated.clone());
                    evaluated
                }
            };
            out.push_str(&substitution);
        } else if name.contains('.') {
            out.push_str(&attribute(
                &Value::String(name.to_string()),
                scope,
                depth + 1,
                budget,
            )?);
        } else {
            out.push_str(&reference(name, scope)?);
        }
        budget.spend(1)?;
        if out.len() > MAX_RESOLVED_TEXT_BYTES {
            return None;
        }
    }
    budget.spend_text(rest.len())?;
    out.push_str(rest);
    (out.len() <= MAX_RESOLVED_TEXT_BYTES).then_some(out)
}
