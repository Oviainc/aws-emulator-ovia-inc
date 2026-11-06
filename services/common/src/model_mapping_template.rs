// SPDX-License-Identifier: CC0-1.0

//! The ONE reading of the JSON Schema document a gateway model carries, and the two
//! documents a gateway control plane generates from it.
//!
//! A model describes the data structure of a method request or response, and a gateway can
//! generate, from that structure, the Velocity (VTL) template content a caller starts a
//! mapping template from. Both control planes read the same schema language (JSON Schema
//! draft-04, "the schema for the model"), so the reading — what JSON type a node declares,
//! what a `$ref` resolves to, and the bounds a generation runs within — is stated ONCE
//! here instead of once per service.
//!
//! What each generates from that reading is its own, because the wire each one answers is:
//!
//! * [`generate_sample_payload`] — the preamble followed by a SAMPLE PAYLOAD in the
//!   structure the schema declares: every declared property carries an example value of
//!   the type declared for it, so the generated document is itself JSON. This is what the
//!   REST control plane's recorded calls answer: a template whose body parses as JSON and
//!   whose example for an `integer` property is an integer
//!   (`apigateway.get-model-template-generates-example-json`,
//!   `apigateway.get-model-template-generates-example-payload`).
//! * [`generate_template`] — the preamble followed by a MAPPING SKELETON: every declared
//!   property mapped to its path under `$inputRoot`, an `array` to a `#foreach` over its
//!   elements and a nested `object` nested. This is what the other control plane's
//!   recorded call answers (`apigatewayv2.get-model-template-returns-schema`).
//!
//! The two renderings are each the one their own service's recording shows; neither was
//! chosen here. Which of them the OTHER service would answer is not knowable from either
//! recording, so nothing in this module assumes they agree.
//!
//! Both are PURE functions of the document — the same schema generates the same text, and
//! nothing is read or written beside it — and both are bounded in descent ([`MAX_DEPTH`]),
//! in STEPS ([`MAX_STEPS`]) and in output ([`MAX_TEMPLATE_BYTES`]). A step is every unit of
//! work a generation does on the document: each value rendered, each property name
//! considered and each segment of a `$ref` walked, so what one call costs is bounded by the
//! budget rather than by the size of the stored schema. A document that does not fit the
//! step or output bound is reported as [`Exhausted`] rather than expanded, so one call
//! costs a fixed amount of work and returns a fixed amount of output however the schema
//! branches or refers back into itself.

use ::std::collections::BTreeMap;

/// The line every generated template opens with.
///
/// It binds the Velocity reference `$inputRoot` to the root of the incoming payload
/// (`$input.path('$')`), so a mapped member is addressed relative to it. This is the
/// preamble API Gateway's own generated templates carry (see the generated templates in the
/// API Gateway "Models and mapping templates" documentation).
const INPUT_ROOT: &str = "$inputRoot";

/// One indentation step of a generated document's nested lines.
const INDENT: &str = "  ";

/// The example value a declared `string` carries in a generated sample payload: the
/// placeholder string the gateway fills a string leaf with, which a caller replaces with a
/// value of their own. A recorded generation shows it as `foo` — the sample is an EXAMPLE
/// payload, so a string leaf carries an example string rather than the name of its type.
const SAMPLE_STRING: &str = "foo";

/// The example value a declared `integer` carries — an integer literal, so the generated
/// document carries the JSON type the schema declares rather than a near one.
const SAMPLE_INTEGER: i64 = 0;

/// The example value a declared `number` carries: a fractional literal, for the reason
/// [`SAMPLE_INTEGER`] is an integral one.
const SAMPLE_NUMBER: f64 = 0.0;

/// The example value a declared `boolean` carries.
const SAMPLE_BOOLEAN: bool = true;

/// How deep into a schema a generation descends before it stops and renders the value as a
/// whole.
///
/// A JSON Schema may point at itself through `$ref` (a tree node whose children are the
/// same schema), so the descent needs a bound to terminate at all; past the bound the value
/// is rendered as one of unknown shape. Every `$ref` hop costs a level too, so a cycle
/// cannot be re-entered free.
pub const MAX_DEPTH: usize = 12;

/// How many STEPS one generation may take.
///
/// A step is every unit of work a generation does on the document: a value rendered, a
/// property name considered while a member list is built, a segment of a `$ref` walked. The
/// budget is therefore total rather than per value — the alternative, charging only the
/// values rendered, leaves the cost of each one bounded by the size of the stored schema,
/// which is the caller's to choose, and a schema that re-renders one wide object pays that
/// cost once per render.
///
/// It is the bound the depth bound alone does not give: a schema branches (an object renders
/// one value per declared property), so the work grows as the product of the branching
/// factors, not with the depth. An EMULATOR bound, not a figure AWS publishes, and what it
/// buys is that one request costs a fixed amount of work whatever the stored schema does. It
/// is far above any schema a gateway model may carry: the published schema maxima are tens
/// to hundreds of kilobytes of text, which cannot declare more than a few thousand
/// properties, so only a schema that re-expands itself through `$ref` reaches this bound,
/// and such a schema is refused rather than expanded.
pub const MAX_STEPS: usize = 20_000;

/// The most bytes a generated document may carry.
///
/// A bound on the generator's OUTPUT, so a small request cannot be answered with an
/// arbitrarily large response. Also an emulator bound, and generous against the schema
/// maxima the gateway models publish: the generated document describes what the schema
/// declares, so this size covers every schema that is not re-expanding itself.
pub const MAX_TEMPLATE_BYTES: usize = 64 * 1024;

/// Which bound a generation hit, for the refusal that names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exhausted {
    /// More steps than [`MAX_STEPS`].
    Steps,
    /// More output than [`MAX_TEMPLATE_BYTES`].
    Size,
}

impl Exhausted {
    /// The bound, as the refusal states it to the caller.
    pub fn bound(self) -> String {
        match self {
            Self::Steps => format!("{MAX_STEPS} steps"),
            Self::Size => format!("{MAX_TEMPLATE_BYTES} bytes of template"),
        }
    }
}

/// The document under construction, and the budget it is generated within.
///
/// Every byte goes through [`Writer::push`] and every unit of work through
/// [`Writer::step`], so the two bounds hold by construction: a generation cannot take more
/// than [`MAX_STEPS`] steps or build more than [`MAX_TEMPLATE_BYTES`] bytes, and it stops at
/// the first of them it reaches.
#[derive(Debug)]
struct Writer {
    template: String,
    /// How many more steps may be taken.
    steps: usize,
}

impl Writer {
    fn new() -> Self {
        Self {
            template: String::new(),
            steps: MAX_STEPS,
        }
    }

    /// Charge one unit of work — a rendered value, a property name considered, a reference
    /// segment walked — against the step budget.
    fn step(&mut self) -> Result<(), Exhausted> {
        self.steps = self.steps.checked_sub(1).ok_or(Exhausted::Steps)?;
        Ok(())
    }

    /// Append `text`, or report the output bound if it would not fit.
    fn push(&mut self, text: &str) -> Result<(), Exhausted> {
        if self.template.len() + text.len() > MAX_TEMPLATE_BYTES {
            return Err(Exhausted::Size);
        }
        self.template.push_str(text);
        Ok(())
    }

    /// Open a generated document with the preamble that binds `$inputRoot`.
    fn preamble(&mut self) -> Result<(), Exhausted> {
        self.push(&format!("#set({INPUT_ROOT} = $input.path('$'))\n"))
    }
}

/// Generate the SAMPLE PAYLOAD of a model whose schema is `document`: the preamble, then an
/// example of the payload the schema declares.
///
/// Every declared property carries a value of the JSON type the schema declares for it — a
/// `string` an example string, an `integer` an integer, a `number` a fractional number, a
/// `boolean` a boolean, an `object` an object of its own declared properties and an `array`
/// one example element — so the generated document is an example of the structure a caller
/// is mapping a payload onto. A schema COMPOSED of subschemas (`allOf`, `anyOf`, `oneOf`)
/// is read through them, so the example describes what the instance must satisfy and not
/// only the keywords the outer schema carries itself. Where the schema itself states a
/// value (an `enum`'s first entry, a `default`), THAT value is the example, because the
/// schema has already named the one it means. A value the schema declares nothing about is
/// rendered `null`.
///
/// Bounded in depth ([`MAX_DEPTH`]), in steps ([`MAX_STEPS`]) and in output
/// ([`MAX_TEMPLATE_BYTES`]): a schema that does not fit the step or output bound is
/// reported as [`Exhausted`] rather than expanded, so this is a fixed amount of work and a
/// fixed amount of output per call.
pub fn generate_sample_payload(document: &::serde_json::Value) -> Result<String, Exhausted> {
    let mut writer = Writer::new();
    writer.preamble()?;
    render_sample(document, document, 0, 0, &mut writer)?;
    Ok(writer.template)
}

/// Generate the MAPPING SKELETON of a model whose schema is `document`.
///
/// The text is the preamble followed by a skeleton of the payload the schema declares: each
/// declared property is mapped to its path under `$inputRoot`, a string-typed value is
/// quoted and any other scalar is not, an `array` becomes a `#foreach` over its items, and
/// a nested `object` nests. A schema that declares no type for a value maps that value by
/// reference, which preserves it whatever it is — including one that declares only a
/// composition of subschemas, which [`generate_sample_payload`] reads through and this
/// rendering deliberately does not: a value of unknown shape flows through a mapping
/// template unchanged, so there is nothing for the skeleton to gain from reading it.
///
/// Bounded exactly as [`generate_sample_payload`] is.
pub fn generate_template(document: &::serde_json::Value) -> Result<String, Exhausted> {
    let mut writer = Writer::new();
    writer.preamble()?;
    render(document, document, INPUT_ROOT, 0, 0, 0, &mut writer)?;
    Ok(writer.template)
}

/// The JSON type a schema node declares for its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Object,
    Array,
    Str,
    /// An `integer`: a whole number.
    Integer,
    /// A `number`, which a fractional value satisfies and an `integer` does not.
    Number,
    Boolean,
    Null,
    /// The schema declares nothing this generator can key on.
    Unknown,
}

/// Render an example of the value `schema` declares into `writer`.
///
/// `root` is the whole schema document, so a `$ref` can be resolved against it; `indent` is
/// how deep the rendered lines sit and `depth` the descent bounded by [`MAX_DEPTH`].
/// Following a `$ref` costs a level of that descent, so a schema that refers back into
/// itself is not re-expanded for free.
fn render_sample(
    root: &::serde_json::Value,
    schema: &::serde_json::Value,
    indent: usize,
    depth: usize,
    writer: &mut Writer,
) -> Result<(), Exhausted> {
    writer.step()?;
    if depth >= MAX_DEPTH {
        // Past the descent bound nothing more is known about the value, which is what
        // `null` renders.
        return writer.push("null");
    }
    if let Some(target) = referenced(root, schema, writer)? {
        return render_sample(root, target, indent, depth + 1, writer);
    }
    // What the instance must satisfy is the schema together with the subschemas it is
    // COMPOSED of, so the example describes all of them rather than only the keywords the
    // outer schema happens to carry itself.
    let declared = composed(root, schema, writer)?;
    // A value the schema itself names — the entry an `enum` restricts it to, or the
    // `default` it supplies (JSON Schema draft-04 sections 5.5.1 and 6.2) — is the example,
    // because the schema has already stated the value it means.
    if let Some(stated) = declared.iter().find_map(|source| stated_value(source)) {
        return writer.push(&compact(stated));
    }
    match declared
        .iter()
        .map(|source| kind(source))
        .find(|kind| *kind != Kind::Unknown)
        .unwrap_or(Kind::Unknown)
    {
        Kind::Object => render_sample_object(root, &declared, indent, depth, writer),
        Kind::Array => render_sample_array(root, &declared, indent, depth, writer),
        Kind::Str => writer.push(&quoted(SAMPLE_STRING)),
        Kind::Integer => writer.push(&SAMPLE_INTEGER.to_string()),
        Kind::Number => writer.push(&compact(&::serde_json::json!(SAMPLE_NUMBER))),
        Kind::Boolean => writer.push(&SAMPLE_BOOLEAN.to_string()),
        Kind::Null | Kind::Unknown => writer.push("null"),
    }
}

/// `schema` and the subschemas an instance of it must ALSO satisfy, as the sources the
/// declaration is read from — the schema itself first, so what it declares itself wins.
///
/// draft-04 section 5.5.3: an instance satisfying `allOf` satisfies EVERY subschema, so all
/// of them describe it and their declarations are read together. An instance satisfying
/// `anyOf` or `oneOf` satisfies at least one branch (sections 5.5.4, 5.5.5), so the first
/// branch is the one the example describes. A subschema that is itself a reference is
/// resolved one hop, which is how a composition of named definitions reads.
///
/// Borrows throughout: the sources live in the document, so reading a composed schema
/// copies nothing. Every source charges one step, which is what bounds a composition —
/// however many subschemas it names, reading them spends the one budget, so a composition
/// too large to read is REFUSED on that bound rather than read in part and answered as if
/// it had been read whole.
fn composed<'doc>(
    root: &'doc ::serde_json::Value,
    schema: &'doc ::serde_json::Value,
    writer: &mut Writer,
) -> Result<Vec<&'doc ::serde_json::Value>, Exhausted> {
    let mut sources = vec![schema];
    let Some(object) = schema.as_object() else {
        return Ok(sources);
    };
    let every = object
        .get("allOf")
        .and_then(::serde_json::Value::as_array)
        .map(::std::vec::Vec::as_slice)
        .unwrap_or_default()
        .iter();
    let first_branch = ["anyOf", "oneOf"].into_iter().filter_map(|keyword| {
        object
            .get(keyword)
            .and_then(::serde_json::Value::as_array)
            .and_then(|branches| branches.first())
    });
    for subschema in every.chain(first_branch) {
        writer.step()?;
        sources.push(referenced(root, subschema, writer)?.unwrap_or(subschema));
    }
    Ok(sources)
}

/// Render an example object: a JSON object whose members are the properties `declared`
/// states — the schema's own and those of the subschemas it is composed of — each carrying
/// an example of its own declared value.
///
/// The properties are emitted in lexicographic order, so one model has ONE generated
/// document however the schema was written and however a JSON map iterates. A schema that
/// declares no properties constrains no member, so there is nothing to describe and the
/// object is rendered empty.
fn render_sample_object(
    root: &::serde_json::Value,
    declared: &[&::serde_json::Value],
    indent: usize,
    depth: usize,
    writer: &mut Writer,
) -> Result<(), Exhausted> {
    let properties = declared_properties_of(declared, writer)?;
    if properties.is_empty() {
        return writer.push("{ }");
    }

    let inner = INDENT.repeat(indent + 1);
    writer.push("{\n")?;
    for (position, (name, property)) in properties.into_iter().enumerate() {
        if position > 0 {
            writer.push(",\n")?;
        }
        writer.push(&inner)?;
        writer.push(&quoted(name))?;
        writer.push(" : ")?;
        render_sample(root, property, indent + 1, depth + 1, writer)?;
    }
    writer.push("\n")?;
    writer.push(&INDENT.repeat(indent))?;
    writer.push("}")
}

/// Render an example array: ONE example element, which is what the `items` schema declares
/// every element to look like.
///
/// `items` may be a single schema (every element) or, in draft-04, a tuple of schemas
/// (section 5.3.1), of which the first describes the element rendered here. An array
/// declaring no `items` constrains no element, so the example is empty.
fn render_sample_array(
    root: &::serde_json::Value,
    declared: &[&::serde_json::Value],
    indent: usize,
    depth: usize,
    writer: &mut Writer,
) -> Result<(), Exhausted> {
    let Some(items) = declared.iter().find_map(|source| declared_items(source)) else {
        return writer.push("[ ]");
    };
    writer.push("[ ")?;
    render_sample(root, items, indent, depth + 1, writer)?;
    writer.push(" ]")
}

/// Render the value `schema` describes, reachable in the template at `path`, into `writer`.
///
/// `root` is the whole schema document, so a `$ref` can be resolved against it; `indent`
/// is how deep the rendered lines sit, `loops` how many `#foreach`s enclose this value
/// (which names its loop variable), and `depth` the descent bounded by [`MAX_DEPTH`].
/// Following a `$ref` costs a level of that descent, so a schema that refers back into
/// itself is not re-expanded for free.
fn render(
    root: &::serde_json::Value,
    schema: &::serde_json::Value,
    path: &str,
    indent: usize,
    loops: usize,
    depth: usize,
    writer: &mut Writer,
) -> Result<(), Exhausted> {
    writer.step()?;
    if depth >= MAX_DEPTH {
        // Past the descent bound the value is mapped by reference, which is the mapping
        // for a value of unknown shape — whatever the payload carries there flows through.
        return writer.push(path);
    }
    if let Some(target) = referenced(root, schema, writer)? {
        return render(root, target, path, indent, loops, depth + 1, writer);
    }
    match kind(schema) {
        Kind::Object => render_object(root, schema, path, indent, loops, depth, writer),
        Kind::Array => render_array(root, schema, path, indent, loops, depth, writer),
        // A string value is quoted, so the template renders valid JSON; every other
        // scalar is substituted bare.
        Kind::Str => {
            writer.push("\"")?;
            writer.push(path)?;
            writer.push("\"")
        }
        Kind::Integer | Kind::Number | Kind::Boolean | Kind::Unknown => writer.push(path),
        Kind::Null => writer.push("null"),
    }
}

/// The schema a `$ref` names, when it resolves inside this document.
///
/// JSON Schema draft-04 section 7 addresses a referenced schema by URI fragment, and the
/// fragment of a reference into the same document is a JSON Pointer (`#/definitions/Pet`),
/// which is what a model's schema carries: API Gateway resolves a `$ref` within the model.
/// A reference to another document, or one this document does not hold, resolves to
/// nothing — its value is then of unknown shape.
///
/// Each segment of the pointer charges one step: walking a reference costs the document as
/// many lookups as the reference names, and the reference is the caller's text, so the cost
/// of resolving one is covered by the budget rather than by the reference's own length.
fn referenced<'doc>(
    root: &'doc ::serde_json::Value,
    schema: &::serde_json::Value,
    writer: &mut Writer,
) -> Result<Option<&'doc ::serde_json::Value>, Exhausted> {
    let Some(pointer) = schema
        .get("$ref")
        .and_then(::serde_json::Value::as_str)
        .and_then(|reference| reference.strip_prefix('#'))
    else {
        return Ok(None);
    };
    for _ in pointer.split('/').skip(1) {
        writer.step()?;
    }
    Ok(root.pointer(pointer))
}

/// The value `schema` itself states for its instance: the first entry of an `enum`, which
/// restricts the instance to that set (draft-04 section 5.5.1), or the `default` the schema
/// supplies for it (section 6.2).
fn stated_value(schema: &::serde_json::Value) -> Option<&::serde_json::Value> {
    let object = schema.as_object()?;
    object
        .get("enum")
        .and_then(::serde_json::Value::as_array)
        .and_then(|values| values.first())
        .or_else(|| object.get("default"))
}

/// The properties `schema` declares, in the order the document's member map holds them.
///
/// Every name considered charges one step, so building a member list is covered by the same
/// budget the rendering is: a schema declaring a wide object cannot be re-read for free.
/// The ORDER is the caller's to settle — the sample path merges several sources through an
/// ordered map, and the skeleton path sorts what it reads.
fn declared_properties<'doc>(
    schema: &'doc ::serde_json::Value,
    writer: &mut Writer,
) -> Result<Vec<(&'doc str, &'doc ::serde_json::Value)>, Exhausted> {
    let declared = schema
        .get("properties")
        .and_then(::serde_json::Value::as_object);
    let mut properties: Vec<(&'doc str, &'doc ::serde_json::Value)> = Vec::new();
    for (name, property) in declared.into_iter().flatten() {
        writer.step()?;
        properties.push((name.as_str(), property));
    }
    Ok(properties)
}

/// The properties every source in `declared` declares together, in lexicographic order.
///
/// Where two sources declare one name the FIRST wins, so what a schema declares itself is
/// what its example carries, whatever the subschemas it is composed of also say about that
/// member. The names are merged through an ordered MAP, which both settles that order and
/// keeps the merge's cost proportional to the names considered rather than to their square.
fn declared_properties_of<'doc>(
    declared: &[&'doc ::serde_json::Value],
    writer: &mut Writer,
) -> Result<Vec<(&'doc str, &'doc ::serde_json::Value)>, Exhausted> {
    let mut merged: BTreeMap<&'doc str, &'doc ::serde_json::Value> = BTreeMap::new();
    for source in declared {
        for (name, property) in declared_properties(source, writer)? {
            merged.entry(name).or_insert(property);
        }
    }
    Ok(merged.into_iter().collect())
}

/// The schema an array declares its elements by, for one that declares them.
fn declared_items(schema: &::serde_json::Value) -> Option<&::serde_json::Value> {
    match schema.get("items") {
        Some(::serde_json::Value::Array(schemas)) => schemas.first(),
        other => other,
    }
}

/// The type `schema` declares for its value.
///
/// `type` is the keyword that states it, and draft-04 allows a union of types there
/// (section 5.5.2), of which the first that is not `null` is taken. A schema that states no
/// `type` still declares one implicitly: `properties` only applies to an object and `items`
/// only to an array (sections 5.4.4, 5.3.1), and every value of an `enum` has the JSON type
/// its first entry does.
fn kind(schema: &::serde_json::Value) -> Kind {
    let Some(object) = schema.as_object() else {
        return Kind::Unknown;
    };
    let declared = match object.get("type") {
        Some(::serde_json::Value::String(name)) => Some(name.as_str()),
        Some(::serde_json::Value::Array(names)) => names
            .iter()
            .find_map(::serde_json::Value::as_str)
            .filter(|name| *name != "null"),
        _ => None,
    };
    if let Some(declared) = declared {
        return match declared {
            "object" => Kind::Object,
            "array" => Kind::Array,
            "string" => Kind::Str,
            "integer" => Kind::Integer,
            "number" => Kind::Number,
            "boolean" => Kind::Boolean,
            "null" => Kind::Null,
            _ => Kind::Unknown,
        };
    }
    if object.contains_key("properties") {
        return Kind::Object;
    }
    if object.contains_key("items") {
        return Kind::Array;
    }
    match object.get("enum").and_then(::serde_json::Value::as_array) {
        Some(values) => match values.first() {
            Some(::serde_json::Value::String(_)) => Kind::Str,
            Some(::serde_json::Value::Object(_)) => Kind::Object,
            Some(::serde_json::Value::Array(_)) => Kind::Array,
            Some(::serde_json::Value::Bool(_)) => Kind::Boolean,
            Some(::serde_json::Value::Number(number)) => {
                if number.is_f64() {
                    Kind::Number
                } else {
                    Kind::Integer
                }
            }
            Some(::serde_json::Value::Null) => Kind::Null,
            None => Kind::Unknown,
        },
        None => Kind::Unknown,
    }
}

/// Render an object: a JSON literal whose members are the schema's declared properties,
/// each mapped to its own path under `path`.
///
/// The properties are emitted in lexicographic order, so one model has ONE template
/// however the schema document happened to be written and however a JSON map iterates. A
/// schema that declares no properties constrains no member, so there is nothing to map and
/// the object is rendered empty.
fn render_object(
    root: &::serde_json::Value,
    schema: &::serde_json::Value,
    path: &str,
    indent: usize,
    loops: usize,
    depth: usize,
    writer: &mut Writer,
) -> Result<(), Exhausted> {
    let mut properties = declared_properties(schema, writer)?;
    // Lexicographic, so one model has ONE template however the document was written and
    // however a JSON member map iterates.
    properties.sort_by_key(|(name, _)| *name);
    if properties.is_empty() {
        return writer.push("{ }");
    }

    let inner = INDENT.repeat(indent + 1);
    writer.push("{\n")?;
    for (position, (name, property)) in properties.into_iter().enumerate() {
        if position > 0 {
            writer.push(",\n")?;
        }
        writer.push(&inner)?;
        writer.push(&quoted(name))?;
        writer.push(" : ")?;
        render(
            root,
            property,
            &member_path(path, name),
            indent + 1,
            loops,
            depth + 1,
            writer,
        )?;
    }
    writer.push("\n")?;
    writer.push(&INDENT.repeat(indent))?;
    writer.push("}")
}

/// Render an array: a `#foreach` over the value at `path`, whose body is the element its
/// `items` schema describes.
///
/// `items` may be a single schema (every element) or, in draft-04, a tuple of schemas
/// (section 5.3.1), of which the first describes the element rendered here. The separating
/// comma is emitted for every element but the last (`$foreach.hasNext`), so the rendered
/// payload is valid JSON.
fn render_array(
    root: &::serde_json::Value,
    schema: &::serde_json::Value,
    path: &str,
    indent: usize,
    loops: usize,
    depth: usize,
    writer: &mut Writer,
) -> Result<(), Exhausted> {
    let items = declared_items(schema);
    let element = loop_variable(loops);
    writer.push(&format!("[#foreach({element} in {path})\n"))?;
    writer.push(&INDENT.repeat(indent + 1))?;
    match items {
        Some(items) => render(root, items, &element, indent + 1, loops + 1, depth + 1, writer)?,
        // Elements of unspecified shape are mapped by reference, one per element.
        None => writer.push(&element)?,
    }
    writer.push("#if($foreach.hasNext),#end\n")?;
    writer.push(&INDENT.repeat(indent))?;
    writer.push("#end]")
}

/// The Velocity variable the `#foreach` at nesting level `loops` binds its element to.
///
/// The outermost loop binds `$elem`, the loop inside it `$elem2`, and so on: a nested loop
/// needs its own name or it would shadow the element its parent is iterating.
fn loop_variable(loops: usize) -> String {
    if loops == 0 {
        "$elem".to_string()
    } else {
        format!("$elem{}", loops + 1)
    }
}

/// How the property `name` of the value at `path` is addressed in a template.
///
/// `$inputRoot.id` is the dotted reference Velocity resolves on a map, but it is only a
/// reference when the name is an identifier (a letter or `_` followed by letters, digits,
/// `_` or `-`): `$inputRoot.order id` would not parse. Every other name is addressed
/// through the map's own `get`, which takes the name as a single-quoted Velocity string
/// literal, with the name ESCAPED for that literal ([`escaped`]) — so the structure of the
/// emitted template is this generator's, never a property name's.
fn member_path(path: &str, name: &str) -> String {
    let mut characters = name.chars();
    let identifier = characters
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        });
    if identifier {
        format!("{path}.{name}")
    } else {
        format!("{path}.get('{}')", escaped(name))
    }
}

/// `name`, escaped for the single-quoted Velocity literal it is embedded in.
///
/// A quote of either kind, a backslash and every control character is emitted in backslash
/// form: a name carrying an apostrophe would otherwise close that literal, and one
/// carrying a double quote or a newline would close the double-quoted JSON literal a
/// string-typed member is rendered inside (or its line). Escaping them is what keeps the
/// emitted template well formed whatever the stored schema names its properties.
fn escaped(name: &str) -> String {
    let mut escaped = String::with_capacity(name.len());
    for character in name.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '\'' => escaped.push_str("\\'"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            control if control.is_control() => {
                escaped.push_str(&format!("\\u{:04x}", control as u32));
            }
            plain => escaped.push(plain),
        }
    }
    escaped
}

/// `value` as a JSON string literal, with whatever it carries escaped.
fn quoted(value: &str) -> String {
    ::serde_json::Value::String(value.to_string()).to_string()
}

/// `value` as its own compact JSON text — how a value the schema states is carried into the
/// generated document.
fn compact(value: &::serde_json::Value) -> String {
    value.to_string()
}
