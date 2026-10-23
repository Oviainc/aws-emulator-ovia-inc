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
//! A `Conditions` section decides what the stack HAS: a resource or an output the
//! template declares under a condition that evaluates false is not part of it ("you can
//! … create resources conditionally"), while a condition this module cannot evaluate
//! leaves both in place rather than dropping them.
//!
//! A stack that finished deploying reports a VALUE for every output its template
//! declares, because that is the only way a client learns what CloudFormation created:
//! `Ref` answers a declared resource's physical id, `Fn::GetAtt` the named attribute of
//! one — from that type's published "Return values" where [`return_values`] carries it,
//! from the resource's own declared property where the attribute IS that property, and
//! from CloudFormation's uniform `Arn`/`Id`/`Name` naming otherwise (see
//! [`conventional_attributes`]) — and the intrinsic functions an output composes them
//! with are evaluated rather than dropped.
//!
//! ## What this does NOT do
//!
//! CREATING the resource in the service that owns it is
//! [`super::provisioning`](crate::state::provisioning), which calls that service's own
//! create API for the resource types this emulator provisions. This module computes only
//! what CloudFormation's documented naming and return values give a declared resource;
//! nothing here reports a status, an error or a value for an upstream call that never ran.
//!
//! An expression this module cannot evaluate yields NO value rather than a guessed one:
//! the output is reported with its key and description and without an `OutputValue`,
//! because `Output$OutputValue` is optional and a made-up ARN is worse than an absent
//! one. What is left unevaluated is an expression whose INPUT this emulator does not
//! hold — a `Ref` to a name the template never declares, a `Fn::ImportValue` of an
//! export no stack of this account published (the account-wide export registry is
//! `ListExports`'s, and it is still scaffolded), a `Fn::Transform` macro, or a
//! `Fn::GetAtt` for an attribute that is neither published for the type, declared as a
//! property of the resource, nor one of the conventional names — never a function whose
//! value follows from the template and the stack.
//!
//! ## Bounds
//!
//! The template body is caller-supplied, so a single `CreateStack`/`UpdateStack` request
//! must not be able to spend unbounded CPU, stack or memory here. Three independent
//! limits do that, and each refuses (yields no value) rather than recursing, allocating
//! or looping further:
//!
//! * **Before any tree is built** — [`document`] refuses a body past the documented
//!   `TemplateBody` maximum, whichever spelling it is in: `serde_json`'s own limit bounds
//!   nesting DEPTH, not how many bytes or nodes a FLAT body carries, so the length bound
//!   cannot live in the YAML guard alone. [`guard_yaml_body`] then bounds the NESTING the
//!   recursive YAML loader can descend into (flow collections open at once, compact
//!   indicators per line, leading indentation), none of which grows with how big a flat
//!   template is. The loader's own recursion is NOT abortable from an `EventReceiver`
//!   (`on_event` returns `()`), and exhausting the stack aborts the process rather than
//!   unwinding, so that nesting has to be refused BEFORE `load` is called.
//! * **While the tree is built** — [`MAX_TEMPLATE_NODES`] nodes and
//!   [`MAX_TEMPLATE_DEPTH`] levels for the YAML spelling, counted as its builder builds;
//!   the node bound is checked by [`within_node_bound`] for the JSON tree `serde_json`
//!   builds in one go, whose nesting is already bounded by that reader's own recursion
//!   limit (the YAML loader needs its own because an `EventReceiver` cannot abort `load`).
//!   Plus the documented per-template quotas on
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

/// The most bytes a template's own `Description` carries.
///
/// "Description: … must be a literal string that's between 0 and 1024 bytes in length",
/// which is also the `@length(min: 1, max: 1024)` the model puts on the `Description`
/// members that report it (`ValidateTemplateOutput$Description`).
pub const MAX_DESCRIPTION_BYTES: usize = 1024;

/// The most attributes one provisioned resource records for `Fn::GetAtt` to read.
///
/// A resource's attributes are its type's published return values, the conventional
/// names and the properties the template declared for it — all three bounded by the
/// template body, which is itself bounded; this is the explicit ceiling on the
/// per-resource map so one caller's template cannot grow it further. Chosen: far past
/// the properties any resource declaration carries.
pub const MAX_ATTRIBUTES_PER_RESOURCE: usize = 128;

/// How many Availability Zones a Region has, for `Fn::GetAZs`.
///
/// The account's network is modelled Region by Region rather than stored (see
/// `services/common/src/network.rs`): a Region has three Availability Zones, named
/// `<region>a`, `<region>b`, `<region>c`, which is exactly what EC2's own
/// `availability_zones_in_region` reports — so a template reading the zones here reads
/// the zones `DescribeAvailabilityZones` lists.
pub const AVAILABILITY_ZONES_PER_REGION: usize = 3;

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
/// from it. Only the YAML spelling needs these: the JSON spelling is read by `serde_json`,
/// which enforces its own nesting limit. (The body LENGTH is bounded for both spellings in
/// [`document`]; it is re-checked here so this guard holds on its own terms.)
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
pub const PARTITION: &str = "aws";

/// What `{"Ref": "AWS::URLSuffix"}` resolves to in the `aws` partition.
const URL_SUFFIX: &str = "amazonaws.com";

/// How many hexadecimal characters an AWS Transfer Family resource identifier carries
/// after its one-letter prefix.
///
/// Transfer's own Smithy model fixes it: `ServerId` is `^s-([0-9a-f]{17})$` and
/// `WorkflowId` is `^w-([a-z0-9]{17})$`, both `length 19..19`.
const TRANSFER_ID_HEX_DIGITS: usize = 17;

/// How many characters an Amazon Bedrock AgentCore identifier carries after its label.
///
/// The AgentCore control-plane model fixes it: every one of its identifiers ends in a
/// `-` plus exactly ten characters — `MemoryId` is
/// `^[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$`, `AgentRuntimeId` is
/// `^[a-zA-Z][a-zA-Z0-9_]{0,99}-[a-zA-Z0-9]{10}$` and `GatewayId` is
/// `^([0-9a-z][-]?){1,100}-[0-9a-z]{10}$`. Those alphabets are wider than hexadecimal
/// (which is why this is not named after hex, as `TRANSFER_ID_HEX_DIGITS` is): the
/// suffix [`generated_service_id`] mixes is hexadecimal, and hexadecimal satisfies every
/// one of them.
const AGENTCORE_ID_SUFFIX_CHARS: usize = 10;

/// The longest LABEL an AgentCore identifier carries before that suffix.
///
/// 48, the tightest of the bounds the model publishes: `AgentRuntimeArn` admits
/// `[a-zA-Z][a-zA-Z0-9_]{0,47}` and `GatewayArn` `([0-9a-z][-]?){1,48}` before the
/// suffix, so a label cut to 48 characters satisfies both the identifier's pattern and
/// the ARN's.
const AGENTCORE_LABEL_CHARS: usize = 48;

/// The ARN namespace (and SigV4 signing name) AgentCore resources carry:
/// `arn:aws:bedrock-agentcore:<region>:<account>:<resource>`, as every `arnReference`
/// in the AgentCore control-plane model spells it (`MemoryArn`, `GatewayArn`,
/// `AgentRuntimeArn`).
const AGENTCORE_ARN_SERVICE: &str = "bedrock-agentcore";

/// The version an AgentCore Runtime carries when it is first created.
///
/// `CreateAgentRuntime` answers an `agentRuntimeVersion` (`^([1-9][0-9]{0,4})$`,
/// `length 1..5`) for the version it published, and a resource CloudFormation has just
/// declared has exactly one.
const FIRST_AGENTCORE_RUNTIME_VERSION: u32 = 1;

/// One resource a template declares, as the template spells it.
#[derive(Debug, Clone)]
pub struct DeclaredResource {
    pub logical_id: String,
    /// `Type`, e.g. `AWS::SNS::Topic`.
    pub resource_type: String,
    /// `Properties`, with intrinsic functions still unevaluated.
    pub properties: Value,
    /// `Condition`: the name of the condition that governs whether this resource is
    /// created at all ("the optional `Conditions` section … controls whether certain
    /// resources are created"). `None` for a resource the template declares
    /// unconditionally.
    pub condition: Option<String>,
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
    /// `Condition`: the name of the condition that governs whether this output is
    /// reported at all ("Condition: … output values that are conditionally created").
    pub condition: Option<String>,
}

/// One parameter a template declares, as the template spells it.
///
/// A declaration keeps every member a client READING a template is told about
/// (`ValidateTemplate` reports `TemplateParameter$ParameterKey`, `$DefaultValue`,
/// `$NoEcho` and `$Description`), not only the two a deployment evaluates expressions
/// with — this is the one reader of the `Parameters` section.
#[derive(Debug, Clone)]
pub struct DeclaredParameter {
    /// The name the section keys this declaration by, which is what a `Ref` names and
    /// what a stack operation passes a `Parameter$ParameterKey` for.
    pub key: String,
    /// `Type` (required): "The data type for the parameter". What decides whether a
    /// `Ref` to it resolves to one value or to a LIST of them: "CommaDelimitedList: An
    /// array of literal strings that are separated by commas", and likewise every
    /// `List<…>` type.
    pub declared_type: Option<String>,
    /// `Default`, where the declaration gives a scalar one: "A value of the appropriate
    /// type for the template to use if no value is specified when a stack is created".
    /// Only a scalar default is a value a `Ref` can resolve to without knowing the
    /// parameter's type; a `List<Number>` default is left to the request.
    pub default: Option<String>,
    /// `NoEcho`: "Whether to mask the parameter value to prevent it from being displayed
    /// in the console, command line tools, or API" — `false` where the declaration omits
    /// it, which is the documented default.
    pub no_echo: bool,
    /// `Description`: "A string of up to 4000 characters that describes the parameter".
    pub description: Option<String>,
}

/// A template body, as this service reads it.
#[derive(Debug, Clone, Default)]
pub struct Template {
    /// The template's own `Description`, which is where CloudFormation takes
    /// `Stack$Description` from.
    pub description: Option<String>,
    /// The `Parameters` section: one entry per declared parameter, in the order the
    /// reader walked the section. The SINGLE record of what a template declares as a
    /// parameter — [`Self::parameter_defaults`] and [`Self::parameter_types`] are views
    /// of it, so nothing holds a second copy.
    pub parameters: Vec<DeclaredParameter>,
    /// The `Transform` section: the names of the macros the template declares, in
    /// declaration order (`ValidateTemplateOutput$DeclaredTransforms`).
    pub transforms: Vec<String>,
    /// The `Mappings` section, which `Fn::FindInMap` reads.
    pub mappings: Value,
    /// The `Conditions` section, which `Fn::If` and a resource's or output's own
    /// `Condition` are evaluated against.
    pub conditions: Value,
    pub resources: Vec<DeclaredResource>,
    pub outputs: Vec<DeclaredOutput>,
}

impl Template {
    /// Each declared parameter's `Default` where it has a scalar one: the value a `Ref`
    /// to that parameter resolves to when the request supplied none.
    ///
    /// Builds the map on each call (the declarations are the stored form), so a caller
    /// that needs it more than once binds it to a local rather than calling in a loop.
    pub fn parameter_defaults(&self) -> BTreeMap<String, String> {
        self.parameters
            .iter()
            .filter_map(|parameter| {
                parameter
                    .default
                    .clone()
                    .map(|default| (parameter.key.clone(), default))
            })
            .collect()
    }

    /// Each declared parameter's `Type`, which is what decides whether a `Ref` to it
    /// resolves to one value or to a LIST of them.
    ///
    /// Allocates per call, the same way [`Self::parameter_defaults`] does.
    pub fn parameter_types(&self) -> BTreeMap<String, String> {
        self.parameters
            .iter()
            .filter_map(|parameter| {
                parameter
                    .declared_type
                    .clone()
                    .map(|declared_type| (parameter.key.clone(), declared_type))
            })
            .collect()
    }
}

/// The `Capability` value that acknowledges a template's IAM resources when
/// CloudFormation generates their names (the model's `Capability` enum).
pub const CAPABILITY_IAM: &str = "CAPABILITY_IAM";

/// The `Capability` value that acknowledges a template's IAM resources when the template
/// gives one of them a CUSTOM name (the model's `Capability` enum).
pub const CAPABILITY_NAMED_IAM: &str = "CAPABILITY_NAMED_IAM";

/// The IAM resource types a template has to be acknowledged for, each with the property
/// a resource of that type takes a CUSTOM name from (`None` for a type that has no such
/// property).
///
/// Source: "Acknowledging IAM resources in CloudFormation templates" in the
/// CloudFormation User Guide, which lists these resource types as the ones requiring
/// `CAPABILITY_IAM`, and "you must specify `CAPABILITY_NAMED_IAM` if you have IAM
/// resources with custom names" for the named form.
const IAM_CAPABILITY_RESOURCES: &[(&str, Option<&str>)] = &[
    ("AWS::IAM::AccessKey", None),
    ("AWS::IAM::Group", Some("GroupName")),
    ("AWS::IAM::InstanceProfile", Some("InstanceProfileName")),
    ("AWS::IAM::ManagedPolicy", Some("ManagedPolicyName")),
    ("AWS::IAM::OIDCProvider", None),
    ("AWS::IAM::Policy", None),
    ("AWS::IAM::Role", Some("RoleName")),
    ("AWS::IAM::SAMLProvider", None),
    ("AWS::IAM::ServerCertificate", None),
    ("AWS::IAM::ServiceLinkedRole", None),
    ("AWS::IAM::User", Some("UserName")),
    ("AWS::IAM::UserToGroupAddition", None),
];

/// The capabilities a template's own declarations require, and the resources that
/// required them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequiredCapabilities {
    /// The `Capability` values, without repetition
    /// (`ValidateTemplateOutput$Capabilities`). Empty for a template that declares
    /// nothing needing acknowledgement.
    pub capabilities: Vec<String>,
    /// `CapabilitiesReason`: "the list of resources that generated the values in the
    /// Capabilities response element" — `None` when there are no capabilities to
    /// explain.
    pub reason: Option<String>,
}

/// What a template has to be acknowledged for: the IAM resources it declares.
///
/// A template declaring any of [`IAM_CAPABILITY_RESOURCES`] requires
/// [`CAPABILITY_IAM`], and one that gives such a resource a custom name requires
/// [`CAPABILITY_NAMED_IAM`] instead — "if your stack template contains IAM resources
/// with custom names, you must specify CAPABILITY_NAMED_IAM". A resource the template
/// declares under a condition still counts: the acknowledgement is about what the
/// template CONTAINS, which is what the request is held to before any condition is
/// evaluated.
pub fn required_capabilities(template: &Template) -> RequiredCapabilities {
    let mut types: Vec<&str> = Vec::new();
    let mut named = false;
    for resource in &template.resources {
        let Some((resource_type, name_property)) = IAM_CAPABILITY_RESOURCES
            .iter()
            .find(|(known, _)| *known == resource.resource_type)
        else {
            continue;
        };
        if !types.contains(resource_type) {
            types.push(resource_type);
        }
        // A custom name is one the TEMPLATE sets: the property's presence is what makes
        // it custom, whatever expression spells it.
        if let Some(property) = name_property {
            if resource
                .properties
                .get(property)
                .is_some_and(|name| !name.is_null())
            {
                named = true;
            }
        }
    }
    if types.is_empty() {
        return RequiredCapabilities::default();
    }
    let capability = if named {
        CAPABILITY_NAMED_IAM
    } else {
        CAPABILITY_IAM
    };
    RequiredCapabilities {
        capabilities: vec![capability.to_string()],
        reason: Some(format!(
            "The following resource(s) require capabilities: [{}]",
            types.join(", ")
        )),
    }
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
///
/// The body's LENGTH is refused here, before either reader runs and for both spellings: a
/// tree costs several times the bytes it was read from (the sections below are cloned out
/// of it), and `serde_json`'s own limit bounds nesting depth rather than size, so a large
/// FLAT body would otherwise be read and copied unbounded.
fn document(body: &str) -> Option<Value> {
    if body.len() > super::TEMPLATE_BODY_MAX_BYTES {
        return None;
    }
    if let Ok(value) = ::serde_json::from_str::<Value>(body) {
        // `serde_json` bounds how deep it recurses but counts no nodes, so the tree it
        // built is held to the same node bound the YAML builder applies as it builds.
        return within_node_bound(&value).then_some(value);
    }
    yaml_document(body)
}

/// Whether a built tree is within [`MAX_TEMPLATE_NODES`] nodes.
///
/// Iterative, with an explicit stack: a recursive walk of a caller-supplied tree would
/// spend the process stack on nesting the caller chose. The pending stack cannot outgrow
/// the node count, which is what the bound refuses.
fn within_node_bound(document: &Value) -> bool {
    let mut nodes = 0usize;
    let mut pending: Vec<&Value> = vec![document];
    while let Some(value) = pending.pop() {
        nodes += 1;
        if nodes > MAX_TEMPLATE_NODES {
            return false;
        }
        match value {
            Value::Array(items) => pending.extend(items.iter()),
            Value::Object(entries) => pending.extend(entries.values()),
            _ => {}
        }
    }
    true
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
        // "Conditions: Conditions that control whether certain resources are created or
        // whether certain resource properties are assigned a value" — a section, like
        // `Mappings`, that the evaluator reads rather than a declaration list.
        conditions: document
            .get("Conditions")
            .filter(|conditions| conditions.is_object())
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
            template.parameters.push(DeclaredParameter {
                key: name.clone(),
                // "Type (required): The data type for the parameter" — kept because a
                // list-typed parameter's value is an ARRAY to every function that reads
                // it.
                declared_type: declaration
                    .get("Type")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                // Only a scalar `Default` is a value a `Ref` can resolve to without
                // knowing the parameter's type; a `List<Number>` default is left to the
                // request.
                default: declaration.get("Default").and_then(scalar_text),
                no_echo: declaration.get("NoEcho").is_some_and(flag),
                description: declaration
                    .get("Description")
                    .and_then(Value::as_str)
                    .filter(|description| !description.is_empty())
                    .map(ToString::to_string),
            });
        }
    }
    template.transforms = declared_transforms(document.get("Transform"));
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
                condition: declaration
                    .get("Condition")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
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
                condition: declaration
                    .get("Condition")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
            });
        }
    }
    Some(template)
}

/// The names the `Transform` section declares.
///
/// "The optional Transform section … specifies one or more macros that CloudFormation
/// uses to process your template", spelled as a single name or as a list of them, and a
/// macro entry may be the name alone or an object carrying `Name` beside the macro's
/// `Parameters` — so the NAME is what is reported either way, and an entry that names
/// nothing contributes nothing.
fn declared_transforms(section: Option<&Value>) -> Vec<String> {
    fn transform_name(entry: &Value) -> Option<String> {
        match entry {
            Value::String(name) => Some(name.clone()),
            Value::Object(_) => entry
                .get("Name")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            _ => None,
        }
    }
    match section {
        Some(Value::Array(entries)) => entries.iter().filter_map(transform_name).collect(),
        Some(entry) => transform_name(entry).into_iter().collect(),
        None => Vec::new(),
    }
}

/// A template scalar as the boolean CloudFormation reads it: a true boolean, or the text
/// a JSON template spells one as (`"NoEcho": "true"`), case-insensitively. Anything else
/// is not a boolean and reads as `false`.
fn flag(value: &Value) -> bool {
    match value {
        Value::Bool(flag) => *flag,
        Value::String(text) => text.eq_ignore_ascii_case("true"),
        _ => false,
    }
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
///
/// Every record starts out NOT materialised: this function computes the identity
/// CloudFormation gives the resource, and
/// [`super::provisioning`](crate::state::provisioning) is what then creates it in the
/// service that owns it, where this emulator provisions that type.
///
/// A resource the template declares under a `Condition` is materialised only where that
/// condition HOLDS ("you can … create resources conditionally"); a condition this module
/// cannot evaluate leaves the resource in place rather than silently dropping it.
///
/// `scope` is the deployment's scope with NO resources resolved yet — which is what this
/// function produces. The attributes each resource answers are filled in in a second
/// pass, once every resource's identity is known, so an attribute that is the resource's
/// own declared property resolves even where that property refers to another resource of
/// the same template (`VpcId: !Ref Vpc`).
pub fn provision(
    template: &Template,
    scope: &Scope<'_>,
    budget: &mut Budget,
) -> Vec<StoredResource> {
    let mut names: Vec<(&DeclaredResource, String)> = Vec::new();
    let mut provisioned: Vec<StoredResource> = Vec::new();
    for declared in &template.resources {
        if !is_created(declared, scope, budget) {
            continue;
        }
        let name = physical_name(declared, scope, budget);
        let (physical_resource_id, attributes) = return_values(declared, &name, scope, budget);
        provisioned.push(StoredResource {
            logical_id: declared.logical_id.clone(),
            resource_type: declared.resource_type.clone(),
            physical_resource_id,
            attributes,
            metadata: declared_metadata(declared),
            // Filled by [`resolve_properties`], once every resource's identity is
            // known; `materialised` by
            // [`super::provisioning`](crate::state::provisioning), once the owning
            // service has created the resource.
            properties: Value::Null,
            materialised: false,
        });
        names.push((declared, name));
    }

    let by_logical_id: BTreeMap<String, StoredResource> = provisioned
        .iter()
        .map(|resource| (resource.logical_id.clone(), resource.clone()))
        .collect();
    let resolved = Scope {
        resources: &by_logical_id,
        ..*scope
    };
    for (resource, (declared, name)) in provisioned.iter_mut().zip(names) {
        attach_attributes(resource, declared, &name, &resolved, budget);
    }
    provisioned
}

/// The content of the `Metadata` resource attribute a declaration carries, as JSON.
///
/// `Metadata` is one of the resource attributes CloudFormation defines on a template's
/// `Resources` entry (the model's `ResourceAttribute` enum lists it beside
/// `CreationPolicy`, `DeletionPolicy`, `Properties`, `Tags`, `UpdatePolicy` and
/// `UpdateReplacePolicy`), and `StackResourceDetail$Metadata` reports its content
/// verbatim. Rendered as JSON whatever the body was spelled in, because the member is a
/// string on the wire and `Metadata` is a JSON object in a template.
///
/// `None` for a declaration with no `Metadata`, and for one whose `Metadata` is `null` —
/// never an invented value.
fn declared_metadata(declared: &DeclaredResource) -> Option<String> {
    let metadata = declared.declaration.get("Metadata")?;
    if metadata.is_null() {
        return None;
    }
    // `Value`'s `Display` is its compact JSON rendering and cannot fail, so no `Metadata`
    // a template declares can be lost to a serialisation error here.
    Some(metadata.to_string())
}

/// Whether a declared resource is one this deployment creates.
///
/// Unconditional unless the declaration names a `Condition`, and then exactly when that
/// condition evaluates TRUE. A condition this module cannot evaluate is not an answer of
/// `false`: the resource is created, because dropping a resource the template declares
/// would be a worse answer than materialising one a false condition would have excluded.
fn is_created(declared: &DeclaredResource, scope: &Scope<'_>, budget: &mut Budget) -> bool {
    declared.condition.as_deref().is_none_or(|condition| {
        condition_holds(condition, scope, 0, budget).unwrap_or(true)
    })
}

/// Fill in the attributes a provisioned resource answers `Fn::GetAtt` with, beyond the
/// ones its type's published return values already gave it:
///
/// 1. the conventional `Arn`/`Id`/`Name` names every type answers
///    ([`conventional_attributes`]);
/// 2. each property the template DECLARED for the resource, under the property's own
///    name — CloudFormation publishes a great many attributes that are exactly the
///    property the template set (`AWS::EC2::Subnet`'s `AvailabilityZone`, `CidrBlock`
///    and `VpcId` among them), and the declared value is the one the resource has.
///
/// Neither ever overwrites an attribute already present: the type's published return
/// value wins over the convention, and the convention over the raw property.
fn attach_attributes(
    resource: &mut StoredResource,
    declared: &DeclaredResource,
    name: &str,
    scope: &Scope<'_>,
    budget: &mut Budget,
) {
    for (attribute, value) in
        conventional_attributes(&declared.resource_type, name, resource, scope.stack)
    {
        if resource.attributes.len() >= MAX_ATTRIBUTES_PER_RESOURCE {
            return;
        }
        resource.attributes.entry(attribute).or_insert(value);
    }
    let Some(properties) = declared.properties.as_object() else {
        return;
    };
    for (property, declaration) in properties {
        if resource.attributes.len() >= MAX_ATTRIBUTES_PER_RESOURCE {
            return;
        }
        if resource.attributes.contains_key(property) {
            continue;
        }
        // Only a property that resolves to ONE value is an attribute: a list- or
        // object-valued property is not something `Fn::GetAtt` reports as a string.
        if let Some(value) = text(declaration, scope, 0, budget) {
            resource.attributes.insert(property.clone(), value);
        }
    }
}

/// The attributes CloudFormation's UNIFORM return-value naming gives a resource of ANY
/// type.
///
/// Every resource type publishes its own "Return values" section, and [`return_values`]
/// carries the ones whose values do not follow from the resource's identity. Across those
/// sections the NAMES are uniform, though: a type's `Arn`/`<Type>Arn` attribute is the
/// resource's ARN, its `Id`/`<Type>Id` the physical id `Ref` returns, and its
/// `Name`/`<Type>Name` the resource's name. A resource of a type this service carries no
/// catalogue entry for therefore still answers those, so an output over it resolves
/// instead of being reported with a key and no value.
///
/// The ARN is the resource's OWN: the one its type's published return values already gave
/// it, where they gave it one (an `…Arn` attribute, or a physical id that is itself an
/// ARN), and otherwise the type's SERVICE segment as the ARN namespace
/// (`AWS::EC2::Subnet` -> `ec2`) with its TYPE segment, hyphenated, as the resource path
/// (`arn:aws:ec2:<region>:<account>:subnet/<name>`) — the form the majority of AWS ARNs
/// take.
///
/// That minted form is the same kind of approximation as the physical id it is built
/// from, and it diverges the same way: a type whose published ARN carries a
/// multi-segment resource path (`AWS::Scheduler::Schedule` is `schedule/<group>/<name>`,
/// `AWS::AutoScaling::AutoScalingGroup` is
/// `autoScalingGroup:<id>:autoScalingGroupName/<name>`) gets an ARN of this emulator's
/// uniform shape rather than of that type's. Narrowing it is the business of the open
/// fidelity issue on the identifiers a stack's outputs carry
/// (`20261022T154846Z-…-stack-outputs-return-synthesized-logical-names-instead`): the
/// answer is a catalogue entry in [`return_values`], which wins here, for each type whose
/// real identifiers this emulator can produce — not an absent value, which is what a
/// client cannot use at all.
///
/// The names are offered for every type, so `Fn::GetAtt` answers one real CloudFormation
/// would reject at validation time for a type that publishes no such attribute. This
/// service does not validate a template against the resource-type registry at all (it has
/// none), so refusing those names would not make it stricter — it would only make the
/// attributes a type DOES publish depend on whether the catalogue happens to carry it.
fn conventional_attributes(
    resource_type: &str,
    name: &str,
    resource: &StoredResource,
    stack: StackScope<'_>,
) -> Vec<(String, String)> {
    // A resource type is `AWS::<Service>::<Type>`: the service segment is the ARN
    // namespace and the last segment is the resource's own type. Anything else (a
    // malformed `Type`) carries no convention at all.
    let segments: Vec<&str> = resource_type.split("::").collect();
    let (Some(service), Some(segment)) = (segments.get(1), segments.last()) else {
        return Vec::new();
    };
    if segments.len() < 3 || service.is_empty() || segment.is_empty() {
        return Vec::new();
    }
    let physical_resource_id = resource.physical_resource_id.clone();
    let arn = known_arn(resource).unwrap_or_else(|| {
        ::emulator_server::emulator_arn(
            &service.to_lowercase(),
            stack.region,
            &format!("{}/{name}", hyphenated(segment)),
        )
    });
    vec![
        ("Arn".to_string(), arn.clone()),
        (format!("{segment}Arn"), arn),
        ("Id".to_string(), physical_resource_id.clone()),
        (format!("{segment}Id"), physical_resource_id),
        ("Name".to_string(), name.to_string()),
        (format!("{segment}Name"), name.to_string()),
    ]
}

/// The ARN a resource ALREADY carries: its physical resource id where that is an ARN
/// ("Ref returns the Amazon Resource Name (ARN)", for the types documented that way), or
/// the value of the one `…Arn` attribute its type's published return values gave it.
///
/// This is what keeps the conventional `Arn` of a type [`return_values`] carries from
/// disagreeing with the ARN that type really publishes (`AWS::SNS::Topic`'s `TopicArn`,
/// `AWS::SQS::Queue`'s `Arn`).
fn known_arn(resource: &StoredResource) -> Option<String> {
    if resource.physical_resource_id.starts_with("arn:") {
        return Some(resource.physical_resource_id.clone());
    }
    let mut published = resource
        .attributes
        .iter()
        .filter(|(attribute, value)| attribute.ends_with("Arn") && value.starts_with("arn:"));
    let (_, only) = published.next()?;
    // Two published ARNs and no rule for which is "the" ARN: the convention mints the
    // resource's own rather than choosing one of them.
    if published.next().is_some() {
        return None;
    }
    Some(only.clone())
}

/// A resource type's segment as an ARN resource path spells it: `AutoScalingGroup` ->
/// `auto-scaling-group`, `LaunchTemplate` -> `launch-template`, `VPC` -> `vpc`.
///
/// A hyphen goes before an upper-case character that starts a word — one following a
/// lower-case character or digit, or one followed by a lower-case character — so an
/// initialism stays one word.
fn hyphenated(segment: &str) -> String {
    let characters: Vec<char> = segment.chars().collect();
    let mut out = String::with_capacity(segment.len() + 4);
    for (index, character) in characters.iter().enumerate() {
        let starts_word = index > 0
            && character.is_ascii_uppercase()
            && (characters[index - 1].is_ascii_lowercase()
                || characters[index - 1].is_ascii_digit()
                || characters
                    .get(index + 1)
                    .is_some_and(char::is_ascii_lowercase));
        if starts_word {
            out.push('-');
        }
        out.push(character.to_ascii_lowercase());
    }
    out
}

/// The name a declared resource carries.
///
/// CloudFormation's own convention is that a resource type's name property is the type's
/// last segment plus `Name` (`TopicName` on `AWS::SNS::Topic`, `QueueName` on
/// `AWS::SQS::Queue`, `BucketName` on `AWS::S3::Bucket`), with `Name` itself used by the
/// newer types and `Identifier` by the Amazon RDS ones (`DBClusterIdentifier` on
/// `AWS::RDS::DBCluster`, `DBInstanceIdentifier` on `AWS::RDS::DBInstance`, each of which
/// is also what that type's `Ref` returns); where none is declared, "CloudFormation
/// generates a unique physical ID and uses that ID for the resource name", in the form
/// `<stack name>-<logical id>-<unique id>`.
///
/// A type whose owning API spells its name member with the resource's FULL noun rather
/// than the type's last segment is read through [`declared_name_property`] first, so the
/// name such a template asked for is still the name the resource carries.
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
    // The type's OWN name property, where it is neither convention, comes first; then the
    // two conventions, then the Amazon RDS `Identifier` spelling. The chain is consumed
    // as it is built — the first property the template declares wins — so no candidate
    // past it is even formatted.
    let declared_name = declared_name_property(&declared.resource_type)
        .map(ToString::to_string)
        .into_iter()
        .chain([
            format!("{segment}Name"),
            "Name".to_string(),
            format!("{segment}Identifier"),
        ])
        .find_map(|property| {
            declared
                .properties
                .get(&property)
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
    format!(
        "{}-{logical_id}-{}",
        scope.stack_name,
        derived_digits(scope.stack_id, logical_id, 12, Alphabet::Upper)
    )
}

/// Which alphabet a derived identifier's digits are drawn from — the ONE thing the
/// identifier forms below differ in.
#[derive(Debug, Clone, Copy)]
enum Alphabet {
    /// Lowercase hexadecimal, the alphabet of an EC2/Transfer id and a DNS label.
    Lower,
    /// Uppercase hexadecimal, the alphabet of CloudFormation's own generated name
    /// segment and of Amazon RDS's immutable resource ids.
    Upper,
}

/// `digits` characters derived from `salt` and `name`, in `alphabet`.
///
/// The ONE derivation every identifier this module has to invent is built on: a SHA-256
/// digest of `<salt>/<name>`, rendered as hexadecimal. It is a function of the resource's
/// own name — itself derived from the stack id and the logical id — so every identifier
/// built from it is stable across reads and across a later update of the same stack, and
/// two resources of one stack never collide. The `salt` keeps two identifiers OF ONE
/// resource (its endpoint label and its resource id, say) from being the same digits.
fn derived_digits(salt: &str, name: &str, digits: usize, alphabet: Alphabet) -> String {
    use ::sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(salt.as_bytes());
    hasher.update(b"/");
    hasher.update(name.as_bytes());
    let finished = hasher.finalize();
    let digest = match alphabet {
        Alphabet::Lower => ::hex::encode(finished),
        Alphabet::Upper => ::hex::encode_upper(finished),
    };
    digest[..digits.min(digest.len())].to_string()
}

/// A service-generated identifier of the form `<prefix>-<digits hexadecimal characters>`.
///
/// Some resource types do not take a name from the template at all: the owning service
/// ASSIGNS the identifier, in a documented form (an AWS Transfer Family server is
/// `s-` + 17 hexadecimal characters, which is the `ServerId` pattern
/// `^s-([0-9a-f]{17})$` in Transfer's own Smithy model). CloudFormation's
/// `<stack name>-<logical id>-<unique id>` name is not such an identifier, so a resource
/// of one of those types reports an identifier of ITS form instead — see
/// [`derived_digits`] for why it is stable.
fn generated_service_id(prefix: &str, name: &str, digits: usize) -> String {
    format!(
        "{prefix}-{}",
        derived_digits(prefix, name, digits, Alphabet::Lower)
    )
}

/// A service-assigned resource id of `digits` UPPERCASE characters after `prefix`.
///
/// Amazon RDS's immutable resource ids (`DbClusterResourceId`, `DbiResourceId`) are
/// published as `cluster-`/`db-` followed by 26 uppercase alphanumeric characters, which
/// is a different alphabet from [`generated_service_id`]'s lowercase hexadecimal; the
/// derivation is the same one.
fn generated_upper_resource_id(prefix: &str, name: &str, digits: usize) -> String {
    format!(
        "{prefix}-{}",
        derived_digits(prefix, name, digits, Alphabet::Upper)
    )
}

/// A label of `digits` characters the OWNING SERVICE assigns inside an identifier of its
/// own, under `salt`.
///
/// Two kinds of published identifier carry one: the middle label of an Amazon RDS endpoint
/// DNS name (`<identifier>.cluster-<label>.<region>.rds.amazonaws.com`) and the suffix of a
/// Secrets Manager secret ARN (`:secret:<name>-<suffix>`). Each passes its own `salt`, so
/// one resource's endpoint label and ARN suffix are different digits.
fn service_assigned_label(salt: &str, name: &str, digits: usize) -> String {
    derived_digits(salt, name, digits, Alphabet::Lower)
}

/// The port the database engine a DB cluster or DB instance declares listens on, or `None`
/// where the declaration does not say.
///
/// `AWS::RDS::DBCluster`/`AWS::RDS::DBInstance` publish `Endpoint.Port` as a return value:
/// the port a template DECLARES fixes it, and otherwise it is the declared engine's
/// documented default (Amazon RDS: "RDS for MySQL - 3306, RDS for PostgreSQL - 5432, RDS
/// for Oracle - 1521, RDS for SQL Server - 1433, RDS for Db2 - 50000"). The engine name is
/// the prefix the property's own value list uses (`aurora-postgresql`, `sqlserver-ee`), so
/// the family is matched on the prefix.
///
/// Both properties are evaluated through the template's intrinsics, exactly as
/// [`physical_name`] evaluates a declared name, so `Port: !Ref DBPort` is the port and
/// `Engine: !Ref EngineName` the engine. A declaration whose ENGINE cannot be resolved
/// yields NO port rather than one engine's default standing in for an unknown engine's.
fn database_port(
    properties: &Value,
    scope: &Scope<'_>,
    budget: &mut Budget,
) -> Option<String> {
    if let Some(port) = properties
        .get("Port")
        .and_then(|value| text(value, scope, 0, budget))
    {
        return Some(port);
    }
    let engine = properties
        .get("Engine")
        .and_then(|value| text(value, scope, 0, budget))?
        .to_ascii_lowercase();
    Some(
        if engine.contains("db2") {
            "50000"
        } else if engine.contains("sqlserver") {
            "1433"
        } else if engine.contains("oracle") {
            "1521"
        } else if engine.contains("postgres") {
            "5432"
        } else {
            // mysql, mariadb, aurora-mysql and the Aurora MySQL aliases.
            "3306"
        }
        .to_string(),
    )
}

/// The property a resource type takes its NAME from when that property is neither the
/// type's last segment plus `Name` nor plain `Name`, which is what [`physical_name`]
/// reads by convention.
///
/// A CloudFormation resource type's name property is the name member of the API resource
/// it wraps, and a few APIs spell that member with the resource's full noun: the Bedrock
/// AgentCore control plane's `CreateAgentRuntime` takes `agentRuntimeName`, so
/// `AWS::BedrockAgentCore::Runtime` takes `AgentRuntimeName` — the type's last segment is
/// `Runtime`, so neither `RuntimeName` nor `Name` finds it. The same for
/// `AWS::ECR::PublicRepository`, whose property is `RepositoryName` ("The name to use for
/// the public repository") while the type's last segment is `PublicRepository`.
///
/// This is the OTHER HALF of the per-type catalogue [`return_values`] below keys: a type
/// whose name property is unconventional has to be registered here AND there. The two are
/// separate functions only because the name is needed BEFORE the return values are
/// computed from it ([`provision`] calls [`physical_name`] first), so they cannot share one
/// `match`; keep them adjacent, and fold both into one per-type descriptor returned by a
/// single lookup when this list grows enough to make that worth it.
fn declared_name_property(resource_type: &str) -> Option<&'static str> {
    match resource_type {
        "AWS::BedrockAgentCore::Runtime" => Some("AgentRuntimeName"),
        ECR_PUBLIC_REPOSITORY => Some("RepositoryName"),
        _ => None,
    }
}

/// The LABEL an AgentCore identifier derived from `name` carries: `name` reduced to the
/// characters that identifier's documented pattern admits, cut to
/// [`AGENTCORE_LABEL_CHARS`], and made to start with a letter.
///
/// The three AgentCore identifier patterns admit three different label alphabets
/// (`[a-zA-Z0-9-_]` for a memory, `[a-zA-Z0-9_]` for a runtime, lower-case
/// `[0-9a-z]` with single `-` separators for a gateway), so `admits` is the pattern's
/// alphabet and `lowercase` says whether the pattern admits upper case at all. A
/// CloudFormation physical name (`<stack>-<logical id>-<unique id>`) is not inside any of
/// them, which is why it is reduced here rather than used as it is: an identifier this
/// emulator reports must be of the form the owning service's model publishes.
///
/// `fallback` is the single letter the label starts with when `name` begins with
/// something the pattern cannot (a digit, or a character that was dropped) — every one of
/// the patterns requires a leading letter except the gateway's, and a leading letter
/// satisfies that one too.
fn agentcore_label(name: &str, admits: fn(char) -> bool, lowercase: bool, fallback: char) -> String {
    let source = if lowercase {
        name.to_ascii_lowercase()
    } else {
        name.to_string()
    };
    let kept: String = source.chars().filter(|c| admits(*c)).collect();
    // A `-` may not repeat in a gateway label (`([0-9a-z][-]?){1,48}` allows at most one
    // after each character) and may not end it, since the suffix adds its own.
    let mut label = String::with_capacity(kept.len() + 1);
    if !kept.starts_with(|c: char| c.is_ascii_alphabetic()) {
        label.push(fallback);
    }
    for c in kept.chars() {
        if c == '-' && (label.is_empty() || label.ends_with('-')) {
            continue;
        }
        label.push(c);
    }
    label.truncate(AGENTCORE_LABEL_CHARS);
    while label.ends_with('-') {
        label.pop();
    }
    if label.is_empty() {
        label.push(fallback);
    }
    label
}

/// Whether `c` is admitted by a memory identifier's label alphabet
/// (`MemoryId`: `[a-zA-Z][a-zA-Z0-9-_]{0,99}`).
fn memory_label_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

/// Whether `c` is admitted by an agent-runtime identifier's label alphabet
/// (`AgentRuntimeId`: `[a-zA-Z][a-zA-Z0-9_]{0,99}`, which has no `-`).
fn runtime_label_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Whether `c` is admitted by a gateway identifier's label alphabet
/// (`GatewayId`: `([0-9a-z][-]?){1,100}`, which is lower case only).
fn gateway_label_char(c: char) -> bool {
    c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'
}

/// The resource types this emulator IMPLEMENTS: the ones [`return_values`] below carries
/// the documented `Ref` and `Fn::GetAtt` return values of, so a stack that declares one
/// gets the physical id and the attributes the type's "Return values" section publishes
/// rather than a name and nothing.
///
/// This is the list of extensions the registry of every account and Region holds as
/// Amazon-Web-Services-published public resource types (see
/// [`crate::state::registry::aws_published`]): the registry reports an extension for a
/// resource type this emulator provisions, and nothing for a name it knows nothing about.
/// Keep it in step with the `match` in [`return_values`] — one entry per arm.
pub const IMPLEMENTED_RESOURCE_TYPES: &[&str] = &[
    "AWS::BedrockAgentCore::Browser",
    "AWS::BedrockAgentCore::CodeInterpreter",
    "AWS::BedrockAgentCore::Gateway",
    "AWS::BedrockAgentCore::Memory",
    "AWS::BedrockAgentCore::Runtime",
    "AWS::BedrockAgentCore::WorkloadIdentity",
    "AWS::DynamoDB::Table",
    "AWS::EC2::SecurityGroup",
    ECR_PUBLIC_REPOSITORY,
    ECR_REPOSITORY,
    ECS_CLUSTER,
    ECS_TASK_DEFINITION,
    ELB_LOAD_BALANCER,
    ELB_TARGET_GROUP,
    "AWS::IAM::Role",
    FIREHOSE_DELIVERY_STREAM,
    "AWS::Lambda::Function",
    "AWS::Logs::LogGroup",
    "AWS::RDS::DBCluster",
    "AWS::RDS::DBInstance",
    "AWS::RUM::AppMonitor",
    "AWS::S3::Bucket",
    "AWS::SNS::Topic",
    SQS_QUEUE,
    "AWS::SecretsManager::Secret",
    "AWS::Transfer::Server",
    "AWS::Transfer::Workflow",
];

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
///
/// The whole declared resource is in scope, not just its type and name, because a few
/// published return values are functions of the resource's own properties — a DB
/// endpoint's port is the one the template declared, or the declared engine's default —
/// and those properties are evaluated through the template's intrinsics, which is what the
/// `scope` and the `budget` are for.
fn return_values(
    declared: &DeclaredResource,
    name: &str,
    scope: &Scope<'_>,
    budget: &mut Budget,
) -> (String, BTreeMap<String, String>) {
    let region = scope.stack.region;
    let resource_type = declared.resource_type.as_str();
    let properties = &declared.properties;
    let mut attributes = BTreeMap::new();
    match resource_type {
        // "Ref returns the topic ARN"; Fn::GetAtt: TopicArn, TopicName.
        "AWS::SNS::Topic" => {
            let arn = ::emulator_server::emulator_arn("sns", region, name);
            attributes.insert("TopicArn".to_string(), arn.clone());
            attributes.insert("TopicName".to_string(), name.to_string());
            (arn, attributes)
        }
        // "Ref returns the queue URL"; Fn::GetAtt: Arn, QueueName, QueueUrl. This is the
        // type `super::provisioning` creates in Amazon SQS itself, which then reports the
        // queue's own url back over the `QueueUrl` attribute — the same url this computes,
        // since both are built from the one account id and the queue's name.
        SQS_QUEUE => {
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
        // "Ref returns the repository name"; Fn::GetAtt: Arn. Amazon ECR Public is a
        // partition-wide service, so the ARN carries no region — the same form the
        // `ecr-public` service mints for the repository this stack provisions, which is
        // what lets the two be compared.
        ECR_PUBLIC_REPOSITORY => {
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn("ecr-public", "", &format!("repository/{name}")),
            );
            (name.to_string(), attributes)
        }
        // "Ref returns the delivery stream name"; Fn::GetAtt: Arn. This is the type
        // `super::provisioning` creates in Amazon Data Firehose itself, which then
        // reports the stream's own ARN back over this attribute.
        FIREHOSE_DELIVERY_STREAM => {
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn(
                    "firehose",
                    region,
                    &format!("deliverystream/{name}"),
                ),
            );
            (name.to_string(), attributes)
        }
        // "Ref returns the repository name"; Fn::GetAtt: Arn, RepositoryUri. The URI is
        // the registry host of the account and Region plus the repository's name
        // ("The format is `aws_account_id.dkr.ecr.region.amazonaws.com/repository-name`"),
        // which is what a container image reference built from this stack's output pulls
        // from — and exactly what Amazon ECR's own `CreateRepository` answers.
        ECR_REPOSITORY => {
            // "A repository name must start with a letter and can only contain lowercase
            // letters, numbers, hyphens, underscores, and forward slashes"
            // (`RepositoryName`'s pattern is lower case only), so the name CloudFormation
            // GENERATES for a repository the template does not name is lower-cased; a name
            // the template declared is reported exactly as it wrote it.
            let repository = match properties.get("RepositoryName") {
                Some(_) => name.to_string(),
                None => name.to_lowercase(),
            };
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn(
                    "ecr",
                    region,
                    &format!("repository/{repository}"),
                ),
            );
            attributes.insert(
                "RepositoryUri".to_string(),
                format!(
                    "{}.dkr.ecr.{region}.{URL_SUFFIX}/{repository}",
                    ::emulator_server::emulator_account_id()
                ),
            );
            (repository, attributes)
        }
        // "Ref returns the cluster name"; Fn::GetAtt: Arn. This is the type
        // `super::provisioning` creates in Amazon ECS itself, so the cluster a stack's
        // output names is one `ecs ListTasks` finds.
        ECS_CLUSTER => {
            attributes.insert(
                "Arn".to_string(),
                ::emulator_server::emulator_arn("ecs", region, &format!("cluster/{name}")),
            );
            (name.to_string(), attributes)
        }
        // "Ref returns the Amazon Resource Name (ARN) of the task definition";
        // Fn::GetAtt: TaskDefinitionArn.
        //
        // A task definition is identified by its FAMILY and a revision, not by a name
        // property ("the family and revision (family:revision)"), so the family is the
        // declared `Family` — evaluated through the template's intrinsics, as every
        // declared name is — and the revision is the FIRST one, which is the revision the
        // registration a stack declaring a family makes carries. A later registration of
        // the same family is numbered by Amazon ECS, and nothing here re-reads that
        // number.
        ECS_TASK_DEFINITION => {
            let family = properties
                .get("Family")
                .and_then(|value| text(value, scope, 0, budget))
                .unwrap_or_else(|| name.to_string());
            let arn = ::emulator_server::emulator_arn(
                "ecs",
                region,
                &format!("task-definition/{family}:{FIRST_TASK_DEFINITION_REVISION}"),
            );
            attributes.insert("TaskDefinitionArn".to_string(), arn.clone());
            (arn, attributes)
        }
        // "Ref returns the ARN of the load balancer"; Fn::GetAtt: DNSName,
        // LoadBalancerFullName, LoadBalancerName (CanonicalHostedZoneID is a value AWS
        // allocated per Region — nothing computes it — and SecurityGroups is list-valued,
        // so neither is carried).
        //
        // Both the ARN and the DNS name carry an id Elastic Load Balancing ASSIGNS
        // (`.../loadbalancer/app/<name>/<id>`, `<name>-<id>.<region>.elb.amazonaws.com`,
        // prefixed `internal-` for an internal scheme), in the same 16-character form that
        // service mints, and the ARN's type segment is the one its own ARNs carry: `app`
        // for an Application Load Balancer, `net` for a Network one, `gwy` for a Gateway
        // one.
        ELB_LOAD_BALANCER => {
            let id = service_assigned_label(ELB_LOAD_BALANCER_ID_SALT, name, ELB_ID_HEX_DIGITS);
            let type_code = load_balancer_type_code(properties, scope, budget);
            let path = format!("{type_code}/{name}/{id}");
            let internal = properties
                .get("Scheme")
                .and_then(|value| text(value, scope, 0, budget))
                .as_deref()
                == Some(ELB_SCHEME_INTERNAL);
            let prefix = if internal { "internal-" } else { "" };
            attributes.insert(
                "DNSName".to_string(),
                format!("{prefix}{name}-{id}.{region}.elb.{URL_SUFFIX}"),
            );
            attributes.insert("LoadBalancerFullName".to_string(), path.clone());
            attributes.insert("LoadBalancerName".to_string(), name.to_string());
            (
                ::emulator_server::emulator_arn(
                    "elasticloadbalancing",
                    region,
                    &format!("loadbalancer/{path}"),
                ),
                attributes,
            )
        }
        // "Ref returns the ARN of the target group"; Fn::GetAtt: TargetGroupArn,
        // TargetGroupFullName, TargetGroupName (LoadBalancerArns is list-valued, so it is
        // not carried). The full name is the ARN's own resource path,
        // `targetgroup/<name>/<id>`, with the same service-assigned id form.
        ELB_TARGET_GROUP => {
            let id = service_assigned_label(ELB_TARGET_GROUP_ID_SALT, name, ELB_ID_HEX_DIGITS);
            let path = format!("targetgroup/{name}/{id}");
            let arn =
                ::emulator_server::emulator_arn("elasticloadbalancing", region, &path);
            attributes.insert("TargetGroupArn".to_string(), arn.clone());
            attributes.insert("TargetGroupFullName".to_string(), path);
            attributes.insert("TargetGroupName".to_string(), name.to_string());
            (arn, attributes)
        }
        // "Ref returns the DBClusterIdentifier"; Fn::GetAtt: DBClusterArn,
        // DBClusterResourceId, Endpoint.Address, Endpoint.Port, ReadEndpoint.Address
        // (MasterUserSecret.SecretArn is left out: it names a secret only RDS's own
        // managed-password integration creates, which this emulator does not have). The
        // endpoints are the published Aurora forms, `<identifier>.cluster-<label>` and
        // `<identifier>.cluster-ro-<label>` under the regional RDS suffix.
        "AWS::RDS::DBCluster" => {
            let label = service_assigned_label(
                RDS_ENDPOINT_LABEL_SALT,
                name,
                RDS_ENDPOINT_LABEL_HEX_DIGITS,
            );
            attributes.insert(
                "DBClusterArn".to_string(),
                ::emulator_server::emulator_arn("rds", region, &format!("cluster:{name}")),
            );
            attributes.insert(
                "DBClusterResourceId".to_string(),
                generated_upper_resource_id("cluster", name, RDS_RESOURCE_ID_DIGITS),
            );
            attributes.insert(
                "Endpoint.Address".to_string(),
                format!("{name}.cluster-{label}.{region}.rds.{URL_SUFFIX}"),
            );
            if let Some(port) = database_port(properties, scope, budget) {
                attributes.insert("Endpoint.Port".to_string(), port);
            }
            attributes.insert(
                "ReadEndpoint.Address".to_string(),
                format!("{name}.cluster-ro-{label}.{region}.rds.{URL_SUFFIX}"),
            );
            (name.to_string(), attributes)
        }
        // "Ref returns the DBInstanceIdentifier"; Fn::GetAtt: DBInstanceArn, DbiResourceId,
        // Endpoint.Address, Endpoint.Port (Endpoint.HostedZoneId is a value AWS allocated
        // per Region — nothing computes it — so it is left out rather than invented).
        "AWS::RDS::DBInstance" => {
            let label = service_assigned_label(
                RDS_ENDPOINT_LABEL_SALT,
                name,
                RDS_ENDPOINT_LABEL_HEX_DIGITS,
            );
            attributes.insert(
                "DBInstanceArn".to_string(),
                ::emulator_server::emulator_arn("rds", region, &format!("db:{name}")),
            );
            attributes.insert(
                "DbiResourceId".to_string(),
                generated_upper_resource_id("db", name, RDS_RESOURCE_ID_DIGITS),
            );
            attributes.insert(
                "Endpoint.Address".to_string(),
                format!("{name}.{label}.{region}.rds.{URL_SUFFIX}"),
            );
            if let Some(port) = database_port(properties, scope, budget) {
                attributes.insert("Endpoint.Port".to_string(), port);
            }
            (name.to_string(), attributes)
        }
        // "Ref returns the ARN of the secret"; Fn::GetAtt: Id, which is that same ARN. A
        // Secrets Manager secret ARN ends in a service-assigned six-character suffix
        // (`arn:aws:secretsmanager:<region>:<account>:secret:<name>-<suffix>`), so the
        // resource id is of THAT form rather than the name.
        "AWS::SecretsManager::Secret" => {
            let suffix =
                service_assigned_label(SECRET_ARN_SUFFIX_SALT, name, SECRET_ARN_SUFFIX_CHARACTERS);
            let arn = ::emulator_server::emulator_arn(
                "secretsmanager",
                region,
                &format!("secret:{name}-{suffix}"),
            );
            attributes.insert("Id".to_string(), arn.clone());
            (arn, attributes)
        }
        // "Ref returns the ID of the security group" for a VPC security group; Fn::GetAtt:
        // GroupId (VpcId is left out: it is whatever the declared `VpcId` property
        // resolves to, which is another resource of the template rather than a value of
        // this one). The id is the service-assigned `sg-` + 17 hexadecimal form EC2
        // publishes.
        "AWS::EC2::SecurityGroup" => {
            let group_id = generated_service_id("sg", name, EC2_ID_HEX_DIGITS);
            attributes.insert("GroupId".to_string(), group_id.clone());
            (group_id, attributes)
        }
        // The Amazon Bedrock AgentCore types. Source for each one's identifiers: the
        // AgentCore control-plane model, whose `arnReference` traits name these very
        // resource types — `MemoryArn` references `AWS::BedrockAgentCore::Memory`,
        // `GatewayArn` `AWS::BedrockAgentCore::Gateway`, `AgentRuntimeArn`
        // `AWS::BedrockAgentCore::Runtime`, `WorkloadIdentityArnType`
        // `AWS::BedrockAgentCore::WorkloadIdentity`, and `CodeInterpreterArn` /
        // `BrowserArn` the two sandbox types — and whose create responses publish
        // the members a CloudFormation resource of that type returns. `Ref` returns the
        // resource's own identifier (the type's primary identifier) and `Fn::GetAtt` reads
        // the identifier and the ARN, each in the form the model's pattern fixes; the
        // members those responses carry that describe a resource's RUNNING state (a
        // status, a timestamp, a failure reason, a gateway's URL) are not carried, because
        // nothing here provisioned a resource to be in a state.
        //
        // "Ref returns the memory id"; Fn::GetAtt: MemoryId, MemoryArn.
        "AWS::BedrockAgentCore::Memory" => {
            let id = generated_service_id(
                &agentcore_label(name, memory_label_char, false, 'm'),
                name,
                AGENTCORE_ID_SUFFIX_CHARS,
            );
            let arn = ::emulator_server::emulator_arn(
                AGENTCORE_ARN_SERVICE,
                region,
                &format!("memory/{id}"),
            );
            attributes.insert("MemoryId".to_string(), id.clone());
            attributes.insert("MemoryArn".to_string(), arn);
            (id, attributes)
        }
        // "Ref returns the agent runtime id"; Fn::GetAtt: AgentRuntimeId,
        // AgentRuntimeArn, AgentRuntimeVersion. A create answers version `1`
        // (`AgentRuntimeVersion` is `^([1-9][0-9]{0,4})$`, and the first version of a
        // runtime is its first), which is a value of the declared resource rather than of
        // its running state.
        "AWS::BedrockAgentCore::Runtime" => {
            let id = generated_service_id(
                &agentcore_label(name, runtime_label_char, false, 'r'),
                name,
                AGENTCORE_ID_SUFFIX_CHARS,
            );
            let arn = ::emulator_server::emulator_arn(
                AGENTCORE_ARN_SERVICE,
                region,
                &format!("runtime/{id}"),
            );
            attributes.insert("AgentRuntimeId".to_string(), id.clone());
            attributes.insert("AgentRuntimeArn".to_string(), arn);
            attributes.insert(
                "AgentRuntimeVersion".to_string(),
                FIRST_AGENTCORE_RUNTIME_VERSION.to_string(),
            );
            (id, attributes)
        }
        // The two SANDBOX types, the same shape one resource along. `CodeInterpreterId`
        // and `BrowserId` are `^[a-zA-Z][a-zA-Z0-9_]{0,47}-[a-zA-Z0-9]{10}$` — the agent
        // runtime's label alphabet exactly, hence `runtime_label_char` — and their ARN
        // patterns admit `code-interpreter(-custom)?` / `browser(-custom)?`: the bare
        // segment is the service's built-in `aws.codeinterpreter.v1` /
        // `aws.browser.v1` sandbox, so a sandbox a template declares is a
        // customer-created one and carries the `-custom` segment.
        //
        // "Ref returns the code interpreter id"; Fn::GetAtt: CodeInterpreterId,
        // CodeInterpreterArn.
        "AWS::BedrockAgentCore::CodeInterpreter" => {
            let id = generated_service_id(
                &agentcore_label(name, runtime_label_char, false, 'c'),
                name,
                AGENTCORE_ID_SUFFIX_CHARS,
            );
            let arn = ::emulator_server::emulator_arn(
                AGENTCORE_ARN_SERVICE,
                region,
                &format!("code-interpreter-custom/{id}"),
            );
            attributes.insert("CodeInterpreterId".to_string(), id.clone());
            attributes.insert("CodeInterpreterArn".to_string(), arn);
            (id, attributes)
        }
        // "Ref returns the browser id"; Fn::GetAtt: BrowserId, BrowserArn.
        "AWS::BedrockAgentCore::Browser" => {
            let id = generated_service_id(
                &agentcore_label(name, runtime_label_char, false, 'b'),
                name,
                AGENTCORE_ID_SUFFIX_CHARS,
            );
            let arn = ::emulator_server::emulator_arn(
                AGENTCORE_ARN_SERVICE,
                region,
                &format!("browser-custom/{id}"),
            );
            attributes.insert("BrowserId".to_string(), id.clone());
            attributes.insert("BrowserArn".to_string(), arn);
            (id, attributes)
        }
        // "Ref returns the gateway id"; Fn::GetAtt: GatewayId, GatewayArn. A gateway
        // identifier is lower case only (`GatewayId`: `^([0-9a-z][-]?){1,100}-[0-9a-z]{10}$`).
        "AWS::BedrockAgentCore::Gateway" => {
            let id = generated_service_id(
                &agentcore_label(name, gateway_label_char, true, 'g'),
                name,
                AGENTCORE_ID_SUFFIX_CHARS,
            );
            let arn = ::emulator_server::emulator_arn(
                AGENTCORE_ARN_SERVICE,
                region,
                &format!("gateway/{id}"),
            );
            attributes.insert("GatewayId".to_string(), id.clone());
            attributes.insert("GatewayArn".to_string(), arn);
            (id, attributes)
        }
        // "Ref returns the name"; Fn::GetAtt: Name, WorkloadIdentityArn. A workload
        // identity is named, not id'd: `CreateWorkloadIdentity` takes a `name`
        // (`^[A-Za-z0-9_.-]+$`) and answers that name plus the ARN, whose shape the model
        // leaves unpatterned, so it is built in this service's own ARN form.
        "AWS::BedrockAgentCore::WorkloadIdentity" => {
            let arn = ::emulator_server::emulator_arn(
                AGENTCORE_ARN_SERVICE,
                region,
                &format!("workload-identity-directory/default/workload-identity/{name}"),
            );
            attributes.insert("Name".to_string(), name.to_string());
            attributes.insert("WorkloadIdentityArn".to_string(), arn);
            (name.to_string(), attributes)
        }
        // "Ref returns the app monitor name"; Fn::GetAtt: Id ("The unique ID of the new
        // app monitor"). CloudWatch RUM ASSIGNS that id — the template cannot name it —
        // and its form is fixed by RUM's own Smithy model: `AppMonitorId` is
        // `^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$`,
        // `length 36..36`, so the resource reports an identifier of THAT form rather than
        // CloudFormation's `<stack>-<logical id>-<unique id>` name (the same case
        // AWS::Transfer::Server is in above).
        "AWS::RUM::AppMonitor" => {
            attributes.insert("Id".to_string(), generated_uuid("rum/appmonitor", name));
            (name.to_string(), attributes)
        }
        _ => (name.to_string(), attributes),
    }
}

/// The `AWS::<cloudFormationName>::<Resource>` type of an Amazon Data Firehose stream.
/// The middle segment is the Firehose model's own `awsService.cloudFormationName`
/// (`smithy_query(query="service_info")` on that service's model).
pub const FIREHOSE_DELIVERY_STREAM: &str = "AWS::KinesisFirehose::DeliveryStream";

/// The same for an Amazon SQS queue: the SQS model carries
/// `awsService.cloudFormationName: SQS`.
pub const SQS_QUEUE: &str = "AWS::SQS::Queue";

/// The `AWS::ECR::PublicRepository` resource type: an Amazon ECR Public repository.
///
/// Spelled as AWS's own CloudFormation resource reference publishes it, under the `ECR`
/// namespace beside the private registry's `AWS::ECR::Repository`, rather than from the
/// ECR Public model's `awsService.cloudFormationName` (`ECRPUBLIC`): the type is what a
/// template spells.
pub const ECR_PUBLIC_REPOSITORY: &str = "AWS::ECR::PublicRepository";

/// An Amazon ECR repository's resource type (ECR's model: `cloudFormationName: ECR`).
pub const ECR_REPOSITORY: &str = "AWS::ECR::Repository";

/// An Amazon ECS cluster's resource type (ECS's model: `cloudFormationName: ECS`). This
/// is the type `super::provisioning` creates in Amazon ECS itself.
pub const ECS_CLUSTER: &str = "AWS::ECS::Cluster";

/// An Amazon ECS task definition's resource type.
pub const ECS_TASK_DEFINITION: &str = "AWS::ECS::TaskDefinition";

/// An Elastic Load Balancing v2 load balancer's resource type (that model's
/// `cloudFormationName: ElasticLoadBalancingV2`).
pub const ELB_LOAD_BALANCER: &str = "AWS::ElasticLoadBalancingV2::LoadBalancer";

/// An Elastic Load Balancing v2 target group's resource type.
pub const ELB_TARGET_GROUP: &str = "AWS::ElasticLoadBalancingV2::TargetGroup";

/// The revision a task definition registered for a family the account does not hold yet
/// carries: "the first task definition that you registered to a family is given a revision
/// of 1".
const FIRST_TASK_DEFINITION_REVISION: u32 = 1;

/// How many characters the id Elastic Load Balancing assigns a load balancer, a target
/// group or a listener carries: 16 lowercase hexadecimal, the form that service's own
/// `mint_id` produces (`.../app/my-load-balancer/50dc6c495c0c9188`).
const ELB_ID_HEX_DIGITS: usize = 16;

/// The salts those ids are derived under (see [`service_assigned_label`]) — one PER KIND,
/// because Elastic Load Balancing keeps the two name spaces apart (a load balancer and a
/// target group may carry the same name) while the ids it assigns them are distinct.
const ELB_LOAD_BALANCER_ID_SALT: &str = "elasticloadbalancing/loadbalancer-id";
const ELB_TARGET_GROUP_ID_SALT: &str = "elasticloadbalancing/targetgroup-id";

/// The `Scheme` value of an INTERNAL load balancer, whose DNS name carries the
/// `internal-` prefix.
const ELB_SCHEME_INTERNAL: &str = "internal";

/// The segment a load balancer's ARN carries for its `Type`: "`app` for an Application
/// Load Balancer, `net` for a Network Load Balancer, `gwy` for a Gateway Load Balancer",
/// with `application` the modelled default for the property.
fn load_balancer_type_code(
    properties: &Value,
    scope: &Scope<'_>,
    budget: &mut Budget,
) -> &'static str {
    match properties
        .get("Type")
        .and_then(|value| text(value, scope, 0, budget))
        .as_deref()
    {
        Some("network") => "net",
        Some("gateway") => "gwy",
        _ => "app",
    }
}

/// Fill in each resource's evaluated `Properties`, against the whole set of resources the
/// template declares.
///
/// This is the request the service that owns a resource is asked to create it with
/// ([`super::provisioning`](crate::state::provisioning)), so every intrinsic function in
/// the properties is evaluated here: a property naming another resource (a stream's
/// `RoleARN` from an `Fn::GetAtt`) carries that resource's value, and one that cannot be
/// evaluated is DROPPED rather than sent as a guess — which is also what CloudFormation's
/// own `AWS::NoValue` means ("removes the corresponding resource property").
///
/// `resources` is what [`provision`] produced for `template`. A declaration whose
/// `Condition` excluded it has no record there, so each record is matched to its
/// declaration by LOGICAL ID rather than by position.
pub fn resolve_properties(
    template: &Template,
    resources: &mut [StoredResource],
    scope: &Scope<'_>,
    budget: &mut Budget,
) {
    for resource in resources.iter_mut() {
        let Some(declared) = template
            .resources
            .iter()
            .find(|declared| declared.logical_id == resource.logical_id)
        else {
            continue;
        };
        resource.properties =
            resolved_value(&declared.properties, scope, 0, budget).unwrap_or(Value::Null);
    }
}

/// One template value with every intrinsic function it carries evaluated, preserving the
/// objects and arrays around them; `None` for a value that cannot be evaluated at all.
fn resolved_value(
    value: &Value,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Option<Value> {
    if depth > MAX_INTRINSIC_DEPTH {
        return None;
    }
    budget.spend(1)?;
    match value {
        Value::Object(members) => {
            // An intrinsic function is a single-member object whose name is the
            // function's; anything else is a property map.
            if let Some((name, argument)) = sole_intrinsic(members) {
                // Through the SAME guard the output path uses: the value's size is held
                // to `MAX_RESOLVED_TEXT_BYTES` and its bytes are charged against the
                // deployment's budget, so evaluating a property cannot amplify what
                // evaluating an output cannot.
                let resolved = intrinsic(name, argument, scope, depth, budget)?;
                return match bounded(resolved, budget)? {
                    Resolved::Text(text) => Some(Value::String(text)),
                    Resolved::List(items) => {
                        Some(Value::Array(items.into_iter().map(Value::String).collect()))
                    }
                };
            }
            Some(Value::Object(
                members
                    .iter()
                    .filter_map(|(key, member)| {
                        Some((
                            key.clone(),
                            resolved_value(member, scope, depth + 1, budget)?,
                        ))
                    })
                    .collect(),
            ))
        }
        Value::Array(items) => {
            if items.len() > MAX_RESOLVED_LIST_LEN {
                return None;
            }
            Some(Value::Array(
                items
                    .iter()
                    .filter_map(|item| resolved_value(item, scope, depth + 1, budget))
                    .collect(),
            ))
        }
        scalar => {
            budget.spend_text(scalar_text(scalar).map_or(1, |text| text.len()))?;
            Some(scalar.clone())
        }
    }
}

/// The intrinsic function a single-member object names, if it names one: `Ref`, or one of
/// the `Fn::`-prefixed functions.
fn sole_intrinsic(members: &Map<String, Value>) -> Option<(&str, &Value)> {
    let mut entries = members.iter();
    let (name, argument) = entries.next()?;
    if entries.next().is_some() {
        return None;
    }
    (name == "Ref" || name.starts_with("Fn::")).then_some((name.as_str(), argument))
}

/// How many hexadecimal characters the service-assigned label of an Amazon RDS endpoint
/// carries: an RDS endpoint's middle label is 12 characters
/// (`mydb.cluster-c7g8h9i0j1k2.us-east-1.rds.amazonaws.com`).
const RDS_ENDPOINT_LABEL_HEX_DIGITS: usize = 12;

/// The salt an RDS endpoint label is derived under (see [`service_assigned_label`]).
const RDS_ENDPOINT_LABEL_SALT: &str = "rds/endpoint";

/// The salt a Secrets Manager secret ARN's suffix is derived under, so a secret's ARN
/// suffix and any endpoint label of the same name are different digits.
const SECRET_ARN_SUFFIX_SALT: &str = "secretsmanager/secret";

/// How many characters an Amazon RDS immutable resource id carries after its prefix:
/// `DbClusterResourceId`/`DbiResourceId` are published as `cluster-`/`db-` plus 26
/// characters.
const RDS_RESOURCE_ID_DIGITS: usize = 26;

/// How many characters the service-assigned suffix of a Secrets Manager secret ARN
/// carries: "Secrets Manager automatically adds a hyphen and six random characters after
/// the secret name at the end of the ARN".
const SECRET_ARN_SUFFIX_CHARACTERS: usize = 6;

/// How many hexadecimal characters an Amazon EC2 resource id carries after its prefix: the
/// current (long) id form is 17 (`sg-0123456789abcdef0`).
const EC2_ID_HEX_DIGITS: usize = 17;

/// A service-assigned identifier of the canonical UUID form, derived from `name` under
/// `domain`.
///
/// Some resource types report an identifier their owning service generates as a UUID (a
/// CloudWatch RUM app monitor's `Id`). The 16 bytes come from a SHA-256 digest of the
/// pair, with byte 6's high nibble set to `4` (the version) and byte 8's two high bits to
/// `10` (the RFC 4122 variant), exactly the layout a version-4 generator writes. It is a
/// function of the resource's own name — itself derived from the stack id and the logical
/// id — so the id is stable across reads and across a later update of the same stack, and
/// two resources of one stack never collide.
fn generated_uuid(domain: &str, name: &str) -> String {
    use ::sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update(b"/");
    hasher.update(name.as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&hasher.finalize()[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
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
    /// The TYPE each parameter was declared with, which decides whether a `Ref` to it
    /// resolves to one value or to a list of them.
    pub parameter_types: &'a BTreeMap<String, String>,
    /// The stack's resources, keyed by logical id.
    pub resources: &'a BTreeMap<String, StoredResource>,
    /// The template's `Mappings` section.
    pub mappings: &'a Value,
    /// The template's `Conditions` section.
    pub conditions: &'a Value,
}

/// Evaluate every output a template declares, in `OutputKey` order (the protocol fixes no
/// order for `Stack$Outputs`).
///
/// One entry per output the stack HAS — an output the template declares under a
/// `Condition` that evaluates false is not one of them ("output values that are
/// conditionally created"), and a condition this module cannot evaluate leaves the output
/// reported rather than dropping it. Whether or not its value could be evaluated does not
/// change that: the output exists because the template declares it, and
/// [`StoredOutput::value`] is `None` where this module cannot compute the value rather
/// than carrying an invented one. The `budget` is the deployment's — shared with
/// [`provision`] — so the work of a whole request is bounded, not just each output's.
pub fn evaluate_outputs(
    template: &Template,
    scope: &Scope<'_>,
    budget: &mut Budget,
) -> Vec<StoredOutput> {
    let mut outputs = Vec::with_capacity(template.outputs.len());
    for declared in &template.outputs {
        let reported = declared.condition.as_deref().is_none_or(|condition| {
            condition_holds(condition, scope, 0, budget).unwrap_or(true)
        });
        if !reported {
            continue;
        }
        outputs.push(StoredOutput {
            key: declared.key.clone(),
            value: text(&declared.value, scope, 0, budget),
            description: declared.description.clone(),
            export_name: declared
                .export_name
                .as_ref()
                .and_then(|name| text(name, scope, 0, budget)),
        });
    }
    outputs
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
/// The functions this module does not implement yield no value, so an output that uses
/// one is reported without an `OutputValue`: `Fn::ImportValue` (the account-wide registry
/// of exported output values is `ListExports`'s, and that operation is still scaffolded)
/// and `Fn::Transform` (a macro this emulator runs nothing for). The CONDITION functions
/// (`Fn::Equals`, `Fn::And`, `Fn::Or`, `Fn::Not`, `Condition`) are not values either:
/// they are evaluated where CloudFormation allows them — the `Conditions` section and
/// `Fn::If` — by [`boolean`].
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
        "Ref" => reference(argument.as_str()?, scope),
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
            // The joined length is known before it is built, so a join whose result is
            // past the bound is never materialised even transiently.
            let joined = values
                .iter()
                .map(String::len)
                .sum::<usize>()
                .saturating_add(delimiter.len() * values.len().saturating_sub(1));
            if joined > MAX_RESOLVED_TEXT_BYTES {
                return None;
            }
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
        // "The intrinsic function Fn::GetAZs returns an array that lists Availability
        // Zones for a specified Region … you can specify "" (empty string) … to get all
        // Availability Zones for the Region the stack is created in."
        "Fn::GetAZs" => {
            let requested = if argument.is_null() {
                String::new()
            } else {
                text(argument, scope, depth + 1, budget)?
            };
            let region = if requested.is_empty() {
                scope.stack.region
            } else {
                requested.as_str()
            };
            Some(Resolved::List(availability_zones(region)))
        }
        // "The intrinsic function Fn::Base64 returns the Base64 representation of the
        // input string." The codec is `aws_smithy_types::base64`, the standard-alphabet
        // encoder the wire serializer already uses for every Smithy blob.
        "Fn::Base64" => {
            let source = text(argument, scope, depth + 1, budget)?;
            // Base64 expands its input by 4/3, so the encoded SIZE is refused before the
            // string is built and charged to the budget once it is — the same bound every
            // other text-producing arm is held to, so nesting one inside another cannot
            // multiply what one request allocates.
            if 4 * source.len().div_ceil(3) > MAX_RESOLVED_TEXT_BYTES {
                return None;
            }
            bounded(
                Resolved::Text(::aws_smithy_types::base64::encode(source)),
                budget,
            )
        }
        // "The intrinsic function Fn::Cidr returns an array of CIDR address blocks."
        "Fn::Cidr" => cidr(argument, scope, depth, budget),
        // "Fn::If returns one value if the specified condition evaluates to true and
        // another value if the specified condition evaluates to false":
        // [condition_name, value_if_true, value_if_false].
        "Fn::If" => {
            let branches = argument.as_array()?;
            if branches.len() != 3 {
                return None;
            }
            let holds = condition_holds(branches[0].as_str()?, scope, depth + 1, budget)?;
            let chosen = if holds { &branches[1] } else { &branches[2] };
            evaluate(chosen, scope, depth + 1, budget)
        }
        // "Fn::Length returns the number of elements within an array or an intrinsic
        // function that returns an array." (A `AWS::LanguageExtensions` function; this
        // service runs no transform, and the count is what it is either way.)
        "Fn::Length" => {
            let items = list(argument, scope, depth + 1, budget)?;
            Some(Resolved::Text(items.len().to_string()))
        }
        // "Fn::ToJsonString … converts an object or array to its corresponding JSON
        // string." (An `AWS::LanguageExtensions` function, as above.)
        "Fn::ToJsonString" => json_text(argument, scope, depth, budget).map(Resolved::Text),
        _ => None,
    }
}

/// The Availability Zones Region `region` has, in zone-name order, as `Fn::GetAZs` lists
/// them.
///
/// Derived from the Region name rather than stored: the zones of a Region are a fact of
/// the account's network, named `<region>` + a letter running from `a`
/// ([`AVAILABILITY_ZONES_PER_REGION`] of them) — the model of the account's network every
/// service of this emulator shares (stated in `services/common/src/network.rs`, and
/// implemented for EC2 by that crate's own `availability_zones_in_region`), so the zones a
/// template reads here are the zones `DescribeAvailabilityZones` reports.
fn availability_zones(region: &str) -> Vec<String> {
    (0..AVAILABILITY_ZONES_PER_REGION)
        .map(|index| {
            let letter = char::from(b'a' + u8::try_from(index).unwrap_or(0));
            format!("{region}{letter}")
        })
        .collect()
}

/// `{"Fn::Cidr": [ipBlock, count, cidrBits]}`: "the specified number of CIDRs"
/// (`count`) carved out of `ipBlock` in order, each with `cidrBits` subnet bits —
/// "specifying a value of 8 for this parameter will create a CIDR with a mask of /24"
/// for an IPv4 block, which is the block's address size (32 bits, or 128 for IPv6) less
/// those subnet bits.
///
/// No value for a request the arithmetic cannot satisfy: a block that is not a CIDR, a
/// subnet size larger than the block, or more subnets than the block holds.
fn cidr(
    argument: &Value,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Option<Resolved> {
    use ::std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    let arguments = argument.as_array()?;
    if arguments.len() < 3 {
        return None;
    }
    let block = text(&arguments[0], scope, depth + 1, budget)?;
    let count: usize = text(&arguments[1], scope, depth + 1, budget)?.parse().ok()?;
    let subnet_bits: u32 = text(&arguments[2], scope, depth + 1, budget)?.parse().ok()?;
    if count == 0 || count > MAX_RESOLVED_LIST_LEN {
        return None;
    }
    let (address, prefix) = block.split_once('/')?;
    let prefix: u32 = prefix.parse().ok()?;
    let (base, total_bits, is_ipv6) = match address.parse::<IpAddr>().ok()? {
        IpAddr::V4(address) => (u128::from(u32::from(address)), 32u32, false),
        IpAddr::V6(address) => (u128::from(address), 128u32, true),
    };
    if prefix > total_bits || subnet_bits > total_bits {
        return None;
    }
    let subnet_prefix = total_bits - subnet_bits;
    if subnet_prefix < prefix {
        return None;
    }
    // The block holds 2^(subnet prefix - block prefix) subnets of that size.
    let available = 1u128.checked_shl(subnet_prefix - prefix)?;
    if u128::try_from(count).ok()? > available {
        return None;
    }
    // The distance from one subnet's network to the next. A subnet that IS the whole
    // address space (`::/0` with 128 subnet bits) has no next one — the bound above has
    // already held `count` to the single subnet the block holds — so the step is never
    // added to anything there.
    let step = 1u128.checked_shl(subnet_bits).unwrap_or(0);
    let network = base & (ones(total_bits) ^ ones(total_bits - prefix));
    let mut blocks = Vec::with_capacity(count);
    for index in 0..count {
        let address = network.checked_add(u128::try_from(index).ok()?.checked_mul(step)?)?;
        let rendered = if is_ipv6 {
            Ipv6Addr::from(address).to_string()
        } else {
            Ipv4Addr::from(u32::try_from(address).ok()?).to_string()
        };
        blocks.push(format!("{rendered}/{subnet_prefix}"));
    }
    bounded(Resolved::List(blocks), budget)
}

/// The low `bits` bits set.
fn ones(bits: u32) -> u128 {
    if bits >= 128 {
        u128::MAX
    } else {
        (1u128 << bits) - 1
    }
}

/// Whether the condition `name` the template declares evaluates TRUE, or `None` when
/// this module cannot evaluate it — including when the template declares no condition by
/// that name.
fn condition_holds(
    name: &str,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Option<bool> {
    if depth > MAX_INTRINSIC_DEPTH {
        return None;
    }
    budget.spend(1)?;
    let declaration = scope.conditions.get(name)?;
    boolean(declaration, scope, depth + 1, budget)
}

/// Evaluate a CONDITION: "the condition functions … Fn::And, Fn::Equals, Fn::If,
/// Fn::Not, Fn::Or", plus a `Condition` naming another declared condition.
///
/// Separate from [`evaluate`] because a condition is not a value: CloudFormation allows
/// these functions in the `Conditions` section and in `Fn::If`, and nowhere a value is
/// expected.
fn boolean(
    value: &Value,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Option<bool> {
    if depth > MAX_INTRINSIC_DEPTH {
        return None;
    }
    budget.spend(1)?;
    if let Some(flag) = value.as_bool() {
        return Some(flag);
    }
    let members = value.as_object()?;
    let mut entries = members.iter();
    let (name, argument) = entries.next()?;
    if entries.next().is_some() {
        return None;
    }
    match name.as_str() {
        // "Fn::Equals compares if two values are equal. Returns true if the two values
        // are equal or false if they aren't."
        "Fn::Equals" => {
            let [left, right] = pair(argument)?;
            let left = evaluate(left, scope, depth + 1, budget)?;
            let right = evaluate(right, scope, depth + 1, budget)?;
            Some(left == right)
        }
        // "Fn::Not returns true for a condition that evaluates to false or returns false
        // for a condition that evaluates to true": a one-element list.
        "Fn::Not" => {
            let conditions = argument.as_array()?;
            if conditions.len() != 1 {
                return None;
            }
            Some(!boolean(&conditions[0], scope, depth + 1, budget)?)
        }
        // "Fn::And returns true if all the specified conditions evaluate to true"; and
        // "Fn::Or returns true if any one of the specified conditions evaluate to true".
        "Fn::And" | "Fn::Or" => {
            let conditions = argument.as_array()?;
            if conditions.is_empty() || conditions.len() > MAX_RESOLVED_LIST_LEN {
                return None;
            }
            let all = name == "Fn::And";
            let mut holds = all;
            for condition in conditions {
                let evaluated = boolean(condition, scope, depth + 1, budget)?;
                holds = if all {
                    holds && evaluated
                } else {
                    holds || evaluated
                };
            }
            Some(holds)
        }
        // "Condition: … the name of a condition that evaluates to true or false."
        "Condition" => condition_holds(argument.as_str()?, scope, depth + 1, budget),
        _ => None,
    }
}

/// `{"Fn::ToJsonString": <value>}`: the value with every intrinsic function inside it
/// resolved, serialized as JSON.
fn json_text(
    argument: &Value,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Option<String> {
    let resolved = json_value(argument, scope, depth + 1, budget)?;
    let rendered = ::serde_json::to_string(&resolved).ok()?;
    budget.spend_text(rendered.len())?;
    (rendered.len() <= MAX_RESOLVED_TEXT_BYTES).then_some(rendered)
}

/// One node of a `Fn::ToJsonString` argument with the intrinsic functions inside it
/// resolved: an intrinsic becomes the string (or the array of strings) it evaluates to,
/// and every other node keeps its shape.
fn json_value(
    value: &Value,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Option<Value> {
    if depth > MAX_INTRINSIC_DEPTH {
        return None;
    }
    budget.spend(1)?;
    match value {
        Value::Object(members) if is_intrinsic(members) => {
            match evaluate(value, scope, depth + 1, budget)? {
                Resolved::Text(text) => Some(Value::String(text)),
                Resolved::List(items) => {
                    Some(Value::Array(items.into_iter().map(Value::String).collect()))
                }
            }
        }
        Value::Object(members) => {
            if members.len() > MAX_RESOLVED_LIST_LEN {
                return None;
            }
            let mut resolved = Map::new();
            for (key, member) in members {
                resolved.insert(key.clone(), json_value(member, scope, depth + 1, budget)?);
            }
            Some(Value::Object(resolved))
        }
        Value::Array(items) => {
            if items.len() > MAX_RESOLVED_LIST_LEN {
                return None;
            }
            let mut resolved = Vec::with_capacity(items.len());
            for item in items {
                resolved.push(json_value(item, scope, depth + 1, budget)?);
            }
            Some(Value::Array(resolved))
        }
        scalar => Some(scalar.clone()),
    }
}

/// Whether an object is an intrinsic function: one member, named `Ref` or `Fn::<Name>`.
fn is_intrinsic(members: &Map<String, Value>) -> bool {
    members.len() == 1
        && members
            .keys()
            .next()
            .is_some_and(|name| name == "Ref" || name.starts_with("Fn::"))
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
        // A function that produced one string where a list was expected is not a list:
        // the list-valued expressions are the list-typed parameters (see
        // [`is_list_type`]) and the functions documented to return an array, and this
        // yields no value rather than splitting a string nobody asked to split.
        Resolved::Text(_) => None,
    }
}

/// `{"Ref": "<name>"}`: a pseudo parameter, a template parameter, or a resource's
/// physical id.
///
/// A parameter declared with a LIST type resolves to a list, not to one string:
/// "CommaDelimitedList: An array of literal strings that are separated by commas", and
/// likewise every `List<…>` type — which is what lets `Fn::Select`, `Fn::Join` and
/// `Fn::Length` read a `Ref` to one.
fn reference(name: &str, scope: &Scope<'_>) -> Option<Resolved> {
    // "CloudFormation provides pseudo parameters … use them the same way as you would a
    // parameter, as the argument for the Ref function."
    match name {
        "AWS::Region" => return Some(Resolved::Text(scope.stack.region.to_string())),
        "AWS::AccountId" => {
            return Some(Resolved::Text(
                ::emulator_server::emulator_account_id().to_string(),
            ))
        }
        "AWS::StackName" => return Some(Resolved::Text(scope.stack.stack_name.to_string())),
        "AWS::StackId" => return Some(Resolved::Text(scope.stack.stack_id.to_string())),
        "AWS::Partition" => return Some(Resolved::Text(PARTITION.to_string())),
        "AWS::URLSuffix" => return Some(Resolved::Text(URL_SUFFIX.to_string())),
        // "AWS::NoValue … removes the corresponding resource property" — there is no
        // value to substitute.
        "AWS::NoValue" => return None,
        _ => {}
    }
    if let Some(value) = scope.parameters.get(name) {
        if is_list_type(scope.parameter_types.get(name).map(String::as_str)) {
            let items: Vec<String> = value
                .split(',')
                .map(|item| item.trim().to_string())
                .collect();
            return (items.len() <= MAX_RESOLVED_LIST_LEN).then_some(Resolved::List(items));
        }
        return Some(Resolved::Text(value.clone()));
    }
    // "Ref returns the physical ID of the resource" for a logical id the template
    // declares.
    scope
        .resources
        .get(name)
        .map(|resource| Resolved::Text(resource.physical_resource_id.clone()))
}

/// Whether a declared parameter type carries a LIST of values: `CommaDelimitedList`, or
/// any of the `List<…>` types ("List<Number>", "List<AWS::EC2::Subnet::Id>", …).
fn is_list_type(declared_type: Option<&str>) -> bool {
    declared_type.is_some_and(|declared| {
        declared == "CommaDelimitedList" || declared.starts_with("List<")
    })
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
            match reference(name, scope)? {
                Resolved::Text(value) => out.push_str(&value),
                // A list is not a value a substitution can put into a string.
                Resolved::List(_) => return None,
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// [`IMPLEMENTED_RESOURCE_TYPES`] is the catalogue of the arms of [`return_values`],
    /// and the registry reports a published extension for exactly its entries — so an
    /// entry that `return_values` does not actually know would make the registry report an
    /// extension for a type this emulator provisions no better than any other. Every
    /// catalogued type must therefore yield the attributes its "Return values" section
    /// publishes, which a type outside the `match` has none of.
    #[test]
    fn every_implemented_resource_type_has_known_return_values() {
        let stack = StackScope {
            region: "us-east-1",
            stack_name: "a-stack",
            stack_id: "a-stack-id",
        };
        let parameters = BTreeMap::new();
        let parameter_types = BTreeMap::new();
        let resources = BTreeMap::new();
        let mappings = Value::Null;
        let conditions = Value::Null;
        let scope = Scope {
            stack,
            parameters: &parameters,
            parameter_types: &parameter_types,
            resources: &resources,
            mappings: &mappings,
            conditions: &conditions,
        };
        let mut budget = Budget::new();
        // `return_values` reads the whole declaration, because a few published return
        // values are functions of the resource's own properties: a declaration carrying
        // only the type is what a template that sets no property spells. A DB type's
        // `Endpoint.Port` is one of those — it needs a declared engine — so each DB row
        // declares the engine a template would.
        let declared_as = |resource_type: &str| DeclaredResource {
            logical_id: "ARes".to_string(),
            resource_type: resource_type.to_string(),
            properties: ::serde_json::json!({"Engine": "aurora-mysql"}),
            condition: None,
            declaration: Value::Null,
        };
        for resource_type in IMPLEMENTED_RESOURCE_TYPES {
            let (_, attributes) =
                return_values(&declared_as(resource_type), "a-name", &scope, &mut budget);
            assert!(
                !attributes.is_empty(),
                "case {resource_type:?}: expected the type's documented Fn::GetAtt \
                 attributes, got none — the catalogue and the return_values match have \
                 drifted apart"
            );
        }
        // And a type no arm carries is NOT in the catalogue: it has no attributes at all.
        let (_, unknown) = return_values(
            &declared_as("AWS::NotAService::NotAType"),
            "a-name",
            &scope,
            &mut budget,
        );
        assert!(
            unknown.is_empty(),
            "case \"a type no arm carries\": expected no attributes, got {unknown:?}"
        );
    }
}
