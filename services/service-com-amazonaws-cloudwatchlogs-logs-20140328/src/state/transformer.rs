// SPDX-License-Identifier: CC0-1.0

//! The TRANSFORMATION a log group's transformer applies to an ingested log event.
//!
//! `PutTransformer` configures "a series of processors, where each processor applies one type
//! of transformation to the log events ingested into this log group. The processors work one
//! after another, in the order that you list them, like a pipeline", and "CloudWatch Logs
//! performs the transformations at the time of log ingestion". What the transformation
//! produces is the TRANSFORMED version of the event: the version a filter configured with
//! `applyOnTransformedLogs` is run against — "if the log group uses either a log-group level
//! or account-level transformer, and you specify true, the metric filter will be applied on
//! the transformed version of the log events instead of the original ingested log events"
//! ([`super::metric_filters`], [`super::subscription_delivery`]).
//!
//! ## What a processor works on
//!
//! A processor names the field it reads and the field it writes by a dot-separated key path —
//! "Use dot notation to access child fields. For example, `store.book`" — so the event under
//! transformation is a JSON object: it starts as the members of the ingested message where
//! that message IS a JSON object, and empty where it is not, and the ingested message itself
//! stays readable under the metadata key [`MESSAGE_SOURCE`], which is the source a parser
//! reads when it names none ("If you omit this value, the whole log message is processed").
//! The transformed message is that object, serialized; an event no processor put a field on
//! has no transformed version, and the ingested message stands.
//!
//! ## Which processors run
//!
//! The processors applied here are the ones whose output is fixed by their own configuration
//! and the event's content: the configurable parsers that structure a message (`parseJSON`,
//! `parseKeyValue`, `csv`) and the mutate processors that move, rewrite and retype the
//! fields a parser produced. The remaining kinds each need a published catalogue that is not
//! in this model — a grok pattern library for `grok`, the per-source record layout of each
//! AWS-vended log type for `parseCloudfront` / `parseRoute53` / `parsePostgres` / `parseVPC`
//! / `parseWAF`, the OCSF event mapping for `parseToOCSF`, and the date-format syntax of
//! `dateTimeConverter$matchPatterns` — so they leave the event as they found it rather than
//! inventing fields nothing derived. They are still ACCEPTED by `PutTransformer`, which is
//! what a client observes of them.
//!
//! What one transformation costs is bounded by the event itself: a key path is followed at
//! most [`MAX_PATH_SEGMENTS`] segments, and an event holds at most [`MAX_EVENT_FIELDS`] names
//! and [`MAX_EVENT_BYTES`] bytes — both charged for what a write adds and refunded for what
//! it replaces, BEFORE anything is written, with every member of a structure and every
//! element of a list counted — so neither the key paths a stored pipeline names, nor how many
//! processors chain, nor how much text one of them would produce decides how much one
//! ingested event allocates. A `substituteString` that would expand a field past that
//! ceiling is abandoned as it is built ([`substituted`]) rather than materialised first, and
//! the expressions it states are compiled ONCE per batch under an explicit size limit, when
//! the pipeline is prepared ([`Pipeline::of`]), rather than once per event at the regex
//! crate's own default.

use serde_json::{Map, Value};

use crate::ops::types::Processor;

/// The metadata key the ingested message is readable under, which is the `source` a parser
/// reads when the configuration names none: `CSV$source` — "The path to the field in the log
/// event that has the comma separated values to be parsed. If you omit this value, the whole
/// log message is processed."
pub const MESSAGE_SOURCE: &str = "@message";

/// How many segments of one dot-separated key path are followed. A path is "dot notation to
/// access child fields", and this bounds how deep a stored processor's path can make a
/// transformation descend or build.
const MAX_PATH_SEGMENTS: usize = 10;

/// How many names one transformed event may hold, so a pipeline that copies a field cannot
/// grow one ingested event without bound. Every member of a nested structure and every
/// element of a list is one name ([`count_value`]).
const MAX_EVENT_FIELDS: usize = 1_000;

/// How many BYTES one transformed event may hold, charged and refunded per write beside the
/// name count, because a name says nothing about the size of what sits under it.
///
/// A transformed log event is a log event, and `PutLogEvents` states the ceiling for one:
/// "Each log event can be no larger than 1 MB" — the number this crate holds as
/// [`crate::state::MAX_LOG_EVENT_BYTES`]. So a write whose value would take the event past
/// what CloudWatch Logs holds for a log event does not happen, which is what bounds an
/// expanding `substituteString` chain, a `splitString` over a long field and a `copyValue`
/// that clones either.
const MAX_EVENT_BYTES: usize = crate::state::MAX_LOG_EVENT_BYTES as usize;

/// The largest compiled program one caller-supplied regular expression may occupy, the same
/// explicit clamp this crate compiles every other request-derived expression under (see
/// `super::filter_pattern` and `super::query_compile`), rather than the regex crate's own
/// much larger default.
const MAX_COMPILED_REGEX_BYTES: usize = 1 << 20;

/// The separator between key-value pairs `parseKeyValue` reads when the configuration names
/// none: "If you omit this, the ampersand `&` character is used."
const DEFAULT_FIELD_DELIMITER: &str = "&";

/// The separator between a key and its value `parseKeyValue` reads when the configuration
/// names none: "If you omit this, the equal `=` character is used."
const DEFAULT_KEY_VALUE_DELIMITER: &str = "=";

/// The column separator `csv` reads when the configuration names none: "If you omit this,
/// the processor looks for the comma `,` character as the delimiter."
const DEFAULT_CSV_DELIMITER: u8 = b',';

/// The text qualifier `csv` reads when the configuration names none: "If you omit this, the
/// double quotation mark `"` character is used."
const DEFAULT_CSV_QUOTE: u8 = b'"';

/// The name `csv` gives a column the configuration does not name: "If you omit this, default
/// column names (`[column_1, column_2 ...]`) are used", so the name is this prefix and the
/// column's 1-based position.
const DEFAULT_CSV_COLUMN_PREFIX: &str = "column_";

/// The `FlattenedElement` value that keeps the LAST of the items one `listToMap` key
/// collected: "use `flattenedElement` to specify which element, `first` or `last`, to keep"
/// (the model's `FlattenedElement` enum, `first` | `last`).
const FLATTENED_ELEMENT_LAST: &str = "last";

/// The `Type` values `typeConverter` converts to: "Valid values are `integer`, `double`,
/// `string` and `boolean`" (the model's `Type` enum).
const TYPE_INTEGER: &str = "integer";
const TYPE_DOUBLE: &str = "double";
const TYPE_STRING: &str = "string";
const TYPE_BOOLEAN: &str = "boolean";

/// A log group's transformer, PREPARED for the events of one batch: the processors it
/// applies, in order, with everything derivable from the configuration alone already derived
/// — the regular expressions its `substituteString` entries state, compiled ONCE here rather
/// than once per event, because a batch carries up to 10,000 events and the expressions are
/// a property of the stored transformer rather than of any event.
#[derive(Debug, Default)]
pub struct Pipeline {
    /// The processors, in the order the transformer lists them.
    processors: Vec<Processor>,
    /// One entry per processor, holding its `substituteString` entries' compiled expressions
    /// in entry order: `None` where the expression does not compile, and empty for a
    /// processor that is no `substituteString`.
    substitutions: Vec<Vec<Option<::regex::Regex>>>,
}

impl Pipeline {
    /// Prepare `processors` for a batch.
    pub fn of(processors: &[Processor]) -> Self {
        let substitutions = processors
            .iter()
            .map(|processor| {
                processor
                    .substitute_string
                    .as_ref()
                    .map(|substitute| {
                        substitute
                            .entries
                            .iter()
                            .map(|entry| {
                                ::regex::RegexBuilder::new(&entry.from)
                                    .size_limit(MAX_COMPILED_REGEX_BYTES)
                                    .dfa_size_limit(MAX_COMPILED_REGEX_BYTES)
                                    .build()
                                    .ok()
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .collect();
        Self {
            processors: processors.to_vec(),
            substitutions,
        }
    }

    /// Whether this is a pipeline at all: a log group with no transformer has none, and then
    /// no event has a transformed version.
    pub fn is_empty(&self) -> bool {
        self.processors.is_empty()
    }

    /// The TRANSFORMED version of `message` under this pipeline, or `None` when the pipeline
    /// leaves the event with no field at all — an event nothing structured has no transformed
    /// version, so the ingested message is what every reader sees.
    pub fn transform(&self, message: &str) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let mut event = Event::of(message);
        for (processor, substitutions) in self.processors.iter().zip(&self.substitutions) {
            apply(&mut event, processor, substitutions);
        }
        event.into_message()
    }
}

/// One log event under transformation: the fields it carries, and the message it was
/// ingested as.
#[derive(Debug)]
struct Event<'a> {
    /// The event's fields, which is what the transformed message is made of.
    fields: Map<String, Value>,
    /// The ingested message, readable under [`MESSAGE_SOURCE`] however the pipeline
    /// rewrites the fields.
    message: &'a str,
    /// How many names [`Event::fields`] holds, kept as writes and removals charge and refund
    /// it rather than recounted per write, so one event's transformation costs a pass over
    /// what it WRITES and not one over the whole event per write.
    names: usize,
    /// How many bytes [`Event::fields`] holds, kept the same way: a name is not a size, so
    /// the two ceilings are charged side by side ([`MAX_EVENT_BYTES`]).
    bytes: usize,
}

impl<'a> Event<'a> {
    /// The event an ingested `message` enters the pipeline as: the members of the message
    /// where it is a JSON object, and no field at all where it is not.
    fn of(message: &'a str) -> Self {
        let fields = match ::serde_json::from_str::<Value>(message) {
            Ok(Value::Object(members)) => members,
            _ => Map::new(),
        };
        let names = count_members(&fields);
        let bytes = member_bytes(&fields);
        Self {
            fields,
            message,
            names,
            bytes,
        }
    }

    /// The transformed message: the event's fields as a JSON document, and `None` for an
    /// event that carries none.
    fn into_message(self) -> Option<String> {
        (!self.fields.is_empty()).then(|| Value::Object(self.fields).to_string())
    }

    /// The value at `path`, left where it is.
    fn read(&self, path: &str) -> Option<&Value> {
        if path.split('.').count() > MAX_PATH_SEGMENTS {
            return None;
        }
        let mut segments = path.split('.');
        let mut node = self.fields.get(segments.next()?)?;
        for segment in segments {
            node = node.as_object()?.get(segment)?;
        }
        Some(node)
    }

    /// The TEXT one processor's `source` reads: the ingested message for the metadata key
    /// (and for a processor that names no source at all), and otherwise the scalar at that
    /// key path.
    fn source_text(&self, source: Option<&str>) -> Option<String> {
        match source {
            None | Some(MESSAGE_SOURCE) => Some(self.message.to_string()),
            Some(path) => self.read(path).and_then(scalar_text),
        }
    }

    /// Write `value` at `path`, answering whether the write happened: a path a field already
    /// occupies is written only when the processor says to overwrite it, and a write that
    /// could take the event past [`MAX_EVENT_FIELDS`] names or [`MAX_EVENT_BYTES`] bytes does
    /// not happen at all — what it adds is charged and what it replaces refunded, so neither
    /// a pipeline copying a subtree over a node inside that same subtree nor a chain of
    /// expanding rewrites can grow the event without bound. Missing parents along the path
    /// are created as nested objects, which is what writing a child key does, and each is
    /// charged too.
    fn write(&mut self, path: &str, value: Value, overwrite: bool) -> bool {
        let depth = path.split('.').count();
        if depth > MAX_PATH_SEGMENTS {
            return false;
        }
        let replaced = self
            .read(path)
            .map(|replaced| (count_value(replaced), value_bytes(replaced)));
        if replaced.is_some() && !overwrite {
            return false;
        }
        // Charged against both ceilings before anything is written, with the parents the path
        // may have to create counted at their maximum, so neither check depends on how far
        // down the walk below gets.
        let (refunded_names, refunded_bytes) = replaced.unwrap_or((0, 0));
        let names = count_value(&value) + depth - 1;
        let bytes = value_bytes(&value) + path.len();
        if self.names.saturating_add(names) > MAX_EVENT_FIELDS.saturating_add(refunded_names) {
            return false;
        }
        if self.bytes.saturating_add(bytes) > MAX_EVENT_BYTES.saturating_add(refunded_bytes) {
            return false;
        }
        let mut segments: Vec<&str> = path.split('.').collect();
        let Some(last) = segments.pop() else {
            return false;
        };
        let mut current = &mut self.fields;
        let mut created = 0;
        for segment in segments {
            if !current.contains_key(segment) {
                created += 1;
            }
            let parent = current
                .entry(segment.to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            if !parent.is_object() {
                *parent = Value::Object(Map::new());
            }
            let Some(members) = parent.as_object_mut() else {
                return false;
            };
            current = members;
        }
        let written_names = count_value(&value) + created;
        let written_bytes = value_bytes(&value) + last.len();
        current.insert(last.to_string(), value);
        self.names = self
            .names
            .saturating_add(written_names)
            .saturating_sub(refunded_names);
        self.bytes = self
            .bytes
            .saturating_add(written_bytes)
            .saturating_sub(refunded_bytes);
        true
    }

    /// The value at `path`, removed from the event.
    fn take(&mut self, path: &str) -> Option<Value> {
        if path.split('.').count() > MAX_PATH_SEGMENTS {
            return None;
        }
        let mut segments: Vec<&str> = path.split('.').collect();
        let last = segments.pop()?;
        let mut current = &mut self.fields;
        for segment in segments {
            current = current.get_mut(segment)?.as_object_mut()?;
        }
        let removed = current.remove(last)?;
        self.names = self.names.saturating_sub(count_value(&removed));
        self.bytes = self.bytes.saturating_sub(value_bytes(&removed));
        Some(removed)
    }

    /// Rewrite the scalar text at `path` through `rewrite`, where the event carries one and
    /// the rewrite produced one: a rewrite that answers `None` — because what it would write
    /// is larger than an event may hold — leaves the field exactly as it was.
    fn rewrite_text(&mut self, path: &str, rewrite: impl Fn(&str) -> Option<String>) {
        if let Some(text) = self.read(path).and_then(scalar_text) {
            if let Some(rewritten) = rewrite(&text) {
                self.write(path, Value::String(rewritten), true);
            }
        }
    }
}

/// How many names one value occupies: itself, plus the names nested under it — one per member
/// of a structure and one per element of a list, because a list of a million elements is a
/// million values however few keys reach it.
fn count_value(value: &Value) -> usize {
    match value {
        Value::Object(members) => 1 + count_members(members),
        Value::Array(items) => 1 + items.iter().map(count_value).sum::<usize>(),
        _ => 1,
    }
}

/// How many names one object holds, nested members included.
fn count_members(members: &Map<String, Value>) -> usize {
    members.values().map(count_value).sum()
}

/// How many bytes one value occupies as the transformed message carries it: the text of a
/// scalar, and the members or elements of a structure or a list with the punctuation that
/// separates them. An estimate of the serialized size, and never an under-estimate of the
/// text it holds, which is what [`MAX_EVENT_BYTES`] bounds.
fn value_bytes(value: &Value) -> usize {
    /// The quotes, colons and commas one member or element costs around its own bytes.
    const PUNCTUATION_BYTES: usize = 4;
    match value {
        Value::String(text) => text.len() + PUNCTUATION_BYTES,
        Value::Array(items) => {
            PUNCTUATION_BYTES + items.iter().map(|item| value_bytes(item) + 1).sum::<usize>()
        }
        Value::Object(members) => PUNCTUATION_BYTES + member_bytes(members),
        // A number and a boolean are written as a short literal; `null` is shorter still.
        _ => PUNCTUATION_BYTES + ::std::mem::size_of::<f64>(),
    }
}

/// How many bytes one object's members occupy, their key names included.
fn member_bytes(members: &Map<String, Value>) -> usize {
    members
        .iter()
        .map(|(name, value)| name.len() + value_bytes(value))
        .sum()
}

/// One scalar's text: the string itself for a string, and the JSON rendering of a number or
/// a boolean. A structure and a list name no single value, so neither has one.
fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// Apply one processor to the event, with the expressions its `substituteString` entries
/// state already compiled ([`Pipeline`]).
///
/// Destructured EXHAUSTIVELY — no `..` — so a processor kind the model gains is a COMPILE
/// error here rather than a transformation this function silently cannot see.
fn apply(event: &mut Event, processor: &Processor, substitutions: &[Option<::regex::Regex>]) {
    let Processor {
        add_keys,
        copy_value,
        csv,
        // `dateTimeConverter` states its `matchPatterns` in a date-format syntax this model
        // does not publish, so the conversion it would write is not derivable here.
        date_time_converter: _,
        delete_keys,
        // `grok` matches against a named pattern library this model does not publish.
        grok: _,
        list_to_map,
        lower_case_string,
        move_keys,
        // The AWS-vended parsers each read one published record layout — the fields that log
        // type carries, in order — which this model does not publish either.
        parse_cloudfront: _,
        parse_json,
        parse_key_value,
        parse_route53: _,
        parse_to_ocsf: _,
        parse_postgres: _,
        parse_vpc: _,
        parse_waf: _,
        rename_keys,
        split_string,
        substitute_string,
        trim_string,
        type_converter,
        upper_case_string,
    } = processor;

    // `parseJSON` "can extract JSON key-value pairs and place them under a destination that
    // you specify"; with no destination "it is placed under the root node", which is the
    // members of the parsed document.
    if let Some(parse_json) = parse_json {
        if let Some(text) = event.source_text(parse_json.source.as_deref()) {
            if let Ok(parsed) = ::serde_json::from_str::<Value>(&text) {
                match (&parse_json.destination, parsed) {
                    (Some(destination), parsed) => {
                        event.write(destination, parsed, true);
                    }
                    (None, Value::Object(members)) => {
                        for (key, value) in members {
                            event.write(&key, value, true);
                        }
                    }
                    // A parsed value that is no object names no key to place at the root.
                    (None, _) => {}
                }
            }
        }
    }

    // `parseKeyValue` "parses a specified field in the original log event into key-value
    // pairs", split on its two delimiters, each key carrying the prefix the processor names.
    if let Some(parse_key_value) = parse_key_value {
        if let Some(text) = event.source_text(parse_key_value.source.as_deref()) {
            let field_delimiter = parse_key_value
                .field_delimiter
                .as_deref()
                .unwrap_or(DEFAULT_FIELD_DELIMITER);
            let key_value_delimiter = parse_key_value
                .key_value_delimiter
                .as_deref()
                .unwrap_or(DEFAULT_KEY_VALUE_DELIMITER);
            for pair in text.split(field_delimiter) {
                // "A value to insert into the value field in the result, when a key-value
                // pair is not successfully split": a pair carrying no delimiter has a value
                // only where the processor names one.
                let (key, value) = match pair.split_once(key_value_delimiter) {
                    Some((key, value)) => (key, value.to_string()),
                    None => match &parse_key_value.non_match_value {
                        Some(non_match) => (pair, non_match.clone()),
                        None => continue,
                    },
                };
                let key = key.trim();
                if key.is_empty() {
                    continue;
                }
                let key = format!(
                    "{}{key}",
                    parse_key_value.key_prefix.as_deref().unwrap_or_default()
                );
                let target = under(parse_key_value.destination.as_deref(), &key);
                event.write(
                    &target,
                    Value::String(value),
                    parse_key_value.overwrite_if_exists,
                );
            }
        }
    }

    // `csv` "parses comma-separated values (CSV) from the log events into columns", under
    // "an array of names to use for the columns in the transformed log event" where the
    // processor names them. Read by the `csv` crate, so a quoted field carrying the
    // delimiter is one column, as CSV defines it.
    if let Some(csv) = csv {
        if let Some(text) = event.source_text(csv.source.as_deref()) {
            let mut reader = ::csv::ReaderBuilder::new()
                .delimiter(first_byte(csv.delimiter.as_deref(), DEFAULT_CSV_DELIMITER))
                .quote(first_byte(
                    csv.quote_character.as_deref(),
                    DEFAULT_CSV_QUOTE,
                ))
                .has_headers(false)
                .flexible(true)
                .from_reader(text.as_bytes());
            if let Some(Ok(record)) = reader.records().next() {
                for (index, column) in record.iter().enumerate() {
                    let name = csv
                        .columns
                        .as_ref()
                        .and_then(|columns| columns.get(index))
                        .cloned()
                        .unwrap_or_else(|| format!("{DEFAULT_CSV_COLUMN_PREFIX}{}", index + 1));
                    let target = under(csv.destination.as_deref(), &name);
                    event.write(&target, Value::String(column.to_string()), true);
                }
            }
        }
    }

    // `addKeys` adds "the key of the new entry to be added to the log event" carrying the
    // literal value the entry names.
    if let Some(add_keys) = add_keys {
        for entry in &add_keys.entries {
            event.write(
                &entry.key,
                Value::String(entry.value.clone()),
                entry.overwrite_if_exists,
            );
        }
    }

    // `copyValue` copies "the key to copy" to "the key of the field to copy the value to",
    // leaving the source where it is.
    if let Some(copy_value) = copy_value {
        for entry in &copy_value.entries {
            let value = if entry.source == MESSAGE_SOURCE {
                Some(Value::String(event.message.to_string()))
            } else {
                event.read(&entry.source).cloned()
            };
            if let Some(value) = value {
                event.write(&entry.target, value, entry.overwrite_if_exists);
            }
        }
    }

    // `deleteKeys` "deletes entries from a log event".
    if let Some(delete_keys) = delete_keys {
        for key in &delete_keys.with_keys {
            event.take(key);
        }
    }

    // `moveKeys` moves "the key to move" to "the key to move to": the source is gone once
    // the target took it, and nothing moves where the target is occupied and the entry does
    // not overwrite it.
    if let Some(move_keys) = move_keys {
        for entry in &move_keys.entries {
            if let Some(value) = event.read(&entry.source).cloned() {
                if event.write(&entry.target, value, entry.overwrite_if_exists) {
                    event.take(&entry.source);
                }
            }
        }
    }

    // `renameKeys` is the same move under "the string to use for the new key name".
    if let Some(rename_keys) = rename_keys {
        for entry in &rename_keys.entries {
            if let Some(value) = event.read(&entry.key).cloned() {
                if event.write(&entry.rename_to, value, entry.overwrite_if_exists) {
                    event.take(&entry.key);
                }
            }
        }
    }

    // `lowerCaseString`, `upperCaseString` and `trimString` ("remove leading and trailing
    // whitespace") each rewrite the text under the keys they name, and name no new field.
    if let Some(lower_case_string) = lower_case_string {
        for key in &lower_case_string.with_keys {
            event.rewrite_text(key, |text| Some(text.to_lowercase()));
        }
    }
    if let Some(upper_case_string) = upper_case_string {
        for key in &upper_case_string.with_keys {
            event.rewrite_text(key, |text| Some(text.to_uppercase()));
        }
    }
    if let Some(trim_string) = trim_string {
        for key in &trim_string.with_keys {
            event.rewrite_text(key, |text| Some(text.trim().to_string()));
        }
    }

    // `splitString` splits "the key of the field to split" on "the separator characters to
    // split the string entry on", so that field becomes a list of text. Every element is one
    // name and its own bytes, so a split of a long field past what an event may hold is
    // refused by the write rather than stored.
    if let Some(split_string) = split_string {
        for entry in &split_string.entries {
            if let Some(text) = event.read(&entry.source).and_then(scalar_text) {
                let parts: Vec<Value> = text
                    .split(entry.delimiter.as_str())
                    .map(|part| Value::String(part.to_string()))
                    .collect();
                event.write(&entry.source, Value::Array(parts), true);
            }
        }
    }

    // `substituteString` replaces, in "the key to modify", each match of "the regular
    // expression string to be replaced" with "the string to be substituted for each match".
    // An expression that does not compile matches nothing, so that entry rewrites nothing,
    // and a substitution whose result would be larger than an event may hold is abandoned as
    // it is built ([`substituted`]) rather than materialised and then refused.
    if let Some(substitute_string) = substitute_string {
        for (entry, expression) in substitute_string.entries.iter().zip(substitutions) {
            let Some(expression) = expression else {
                continue;
            };
            event.rewrite_text(&entry.source, |text| {
                substituted(expression, text, &entry.to)
            });
        }
    }

    // `typeConverter` converts the value under "the key with the value that is to be
    // converted to a different type" to that type. A value the conversion does not apply to
    // is left as it is.
    if let Some(type_converter) = type_converter {
        for entry in &type_converter.entries {
            if let Some(converted) = event
                .read(&entry.key)
                .and_then(|value| convert(value, &entry.r#type))
            {
                event.write(&entry.key, converted, true);
            }
        }
    }

    // `listToMap` "takes a list of objects that contain key fields, and converts them into a
    // map of target keys": each item's `key` field names an entry of the map, whose value is
    // the item's `valueKey` field where one is named and the item itself otherwise.
    if let Some(list_to_map) = list_to_map {
        if let Some(Value::Array(items)) = event.read(&list_to_map.source).cloned() {
            let keep_last =
                list_to_map.flattened_element.as_deref() == Some(FLATTENED_ELEMENT_LAST);
            let mut map: Map<String, Value> = Map::new();
            for item in items {
                let Some(members) = item.as_object() else {
                    continue;
                };
                let Some(key) = members.get(&list_to_map.key).and_then(scalar_text) else {
                    continue;
                };
                let value = match &list_to_map.value_key {
                    Some(value_key) => match members.get(value_key) {
                        Some(value) => value.clone(),
                        None => continue,
                    },
                    None => item.clone(),
                };
                // "A Boolean value to indicate whether the list will be flattened into
                // single items": a flattened key holds one value — the first or the last the
                // list named — and an unflattened one holds every value it named.
                if list_to_map.flatten {
                    if keep_last || !map.contains_key(&key) {
                        map.insert(key, value);
                    }
                } else if let Some(Value::Array(values)) = map.get_mut(&key) {
                    values.push(value);
                } else {
                    map.insert(key, Value::Array(vec![value]));
                }
            }
            match &list_to_map.target {
                Some(target) => {
                    event.write(target, Value::Object(map), true);
                }
                None => {
                    for (key, value) in map {
                        event.write(&key, value, true);
                    }
                }
            }
        }
    }
}

/// The key path `name` sits at under `destination`: "the path to the parent field to put
/// transformed key value pairs under. If you omit this value, the key value pairs will be
/// placed under the root node."
fn under(destination: Option<&str>, name: &str) -> String {
    match destination {
        Some(destination) => format!("{destination}.{name}"),
        None => name.to_string(),
    }
}

/// The single byte a one-character CSV setting names, or `fallback` where it names none.
fn first_byte(setting: Option<&str>, fallback: u8) -> u8 {
    setting
        .and_then(|value| value.as_bytes().first().copied())
        .unwrap_or(fallback)
}

/// `text` with `replacement` substituted for every match of `expression`, or `None` when the
/// result would be larger than one event may hold ([`MAX_EVENT_BYTES`]).
///
/// Built match by match, through the expression's own non-overlapping matches, and ABANDONED
/// as soon as what has been built passes the ceiling — so a pattern matching at nearly every
/// position of a long field, with the longest replacement the model admits, allocates no more
/// than one event's worth before the rewrite is dropped, and a chain of such processors
/// cannot compound past it. `replacement` is substituted as the literal text the model
/// describes ("the string to be substituted for each match"), so nothing in it is expanded.
fn substituted(expression: &::regex::Regex, text: &str, replacement: &str) -> Option<String> {
    let mut substituted = String::new();
    let mut after_previous = 0;
    for matched in expression.find_iter(text) {
        substituted.push_str(text.get(after_previous..matched.start())?);
        substituted.push_str(replacement);
        after_previous = matched.end();
        if substituted.len() > MAX_EVENT_BYTES {
            return None;
        }
    }
    substituted.push_str(text.get(after_previous..)?);
    (substituted.len() <= MAX_EVENT_BYTES).then_some(substituted)
}

/// `value` as the type `type_name` names, or `None` where it is not one of that type: a
/// conversion nothing supplies a value for leaves the field as it was.
fn convert(value: &Value, type_name: &str) -> Option<Value> {
    let text = scalar_text(value)?;
    let text = text.trim();
    match type_name {
        TYPE_STRING => Some(Value::String(text.to_string())),
        TYPE_INTEGER => text.parse::<i64>().ok().map(Value::from),
        TYPE_DOUBLE => text
            .parse::<f64>()
            .ok()
            .and_then(::serde_json::Number::from_f64)
            .map(Value::Number),
        TYPE_BOOLEAN => match text.to_ascii_lowercase().as_str() {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            _ => None,
        },
        _ => None,
    }
}
