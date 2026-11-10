// SPDX-License-Identifier: CC0-1.0

//! A `Map` state's `ItemReader`: the S3 dataset its items come from.
//!
//! "ItemReader — specifies a dataset and its location": a `Map` state that declares
//! one does not iterate its own input, it iterates what that dataset holds. The
//! dataset is REALLY READ — the `Resource` names an S3 operation, and that operation
//! is called on S3's own handler in this same emulator over the seam every other
//! in-process call goes through ([`super::integration::dataset_call`]), so the items
//! a state iterates are the objects and the bytes S3 itself answers with. Nothing is
//! synthesised: a dataset nobody stored reads as nothing, and a read S3 refused fails
//! the state.
//!
//! ## Which datasets are read
//!
//! * `arn:aws:states:::s3:listObjectsV2` — "a JSON array of Amazon S3 object metadata
//!   items": one item per object of the bucket (or of the `Prefix` within it), each
//!   carrying that object's `Etag`, `Key`, `LastModified`, `Size` and `StorageClass`,
//!   every value the listing's own. The listing is PAGED, so the reader follows
//!   `NextContinuationToken` until the listing is complete or [`MAX_ITEMS`] items have
//!   been read.
//! * `arn:aws:states:::s3:getObject` — the dataset is the ONE object the `Key` names,
//!   and `ReaderConfig.InputType` says how its bytes are items:
//!   * `CSV` — the first row is the header unless `CSVHeaderLocation` is `GIVEN` and
//!     `CSVHeaders` carries them, and each later row is one item whose members are
//!     those headers against that row's columns, read as the STRINGS the file holds;
//!   * `JSON` — the file is one JSON ARRAY, and each element is one item;
//!   * `JSONL` — each line is one JSON value, and each is one item.
//!
//! `ReaderConfig.MaxItems` bounds how many items any of those delivers — "limits the
//! number of data items passed" — and `MaxItemsPath` reads that figure from the state's
//! own input instead; the caller resolves whichever the reader declares in the state's
//! query language and hands the figure to [`read`].
//!
//! An `InputType` outside those, and a `Resource` naming neither call, is not faked:
//! the state fails with ASL's own [`super::asl::RUNTIME_ERROR`] and a cause saying what
//! was not read.
//!
//! ## What a failed read does
//!
//! "States.ItemReaderFailed — occurs when a state fails because it can't read from the
//! `ItemReader` field" ([`ITEM_READER_ERROR`]): a bucket or an object that is not
//! there, a CSV or JSON document the dataset's own `InputType` does not describe, a
//! dataset larger than this emulator reads. It is the MAP STATE's failure, raised
//! before a single item is iterated — so a `Map` whose dataset cannot be read starts
//! no child execution at all — and it carries an error name, so the state's own
//! `Retry` and `Catch` answer for it exactly as they answer for a failed task.
//!
//! ## What bounds one read
//!
//! A dataset is as large as whoever wrote it chose, and it is read into this process,
//! so each dimension carries its own ceiling and a dataset past one fails the read
//! rather than the process: [`MAX_ITEMS`] items (which also bounds how many are BUILT,
//! not only how many are delivered), [`MAX_DATASET_BYTES`] of one object's bytes,
//! [`MAX_PAGES`] pages of a listing, and — for a CSV dataset, which chooses both
//! dimensions of the members its items carry — [`MAX_COLUMNS`] columns and
//! [`MAX_ITEM_MEMBERS`] members over all its rows together. The walk's own work
//! allowance ([`super::asl::MAX_INTERPRET_WORK`]) bounds the whole read on top of
//! those.
//!
//! ## What the read is subject to
//!
//! The execution role is asked BEFORE the request goes anywhere, exactly as it is for a
//! `Task` state's integration call ([`Tasks::authorize`]). S3 carries no entry in the
//! catalogue of which request member names a call's TARGET
//! ([`super::integration::TARGET_RESOURCES`]): a bucket and a key are two members where
//! that catalogue reads one, so no access request is formed for a dataset read yet and
//! the read is made as before. The gate is asked all the same, so an entry added there
//! governs this read without a second decision being written here.

use serde_json::{Map, Value};

use super::integration::{Call, Failed, Tasks};

/// The ASL predefined error name for a state that could not read its `ItemReader`
/// ("States.ItemReaderFailed").
pub const ITEM_READER_ERROR: &str = "States.ItemReaderFailed";

/// The most items ONE `ItemReader` delivers, whatever the dataset holds: the same
/// ceiling the language's own array intrinsics are held to
/// ([`super::asl::MAX_ARRAY_ITEMS`]), because the array a reader produces is the array
/// a `Map` state iterates.
pub const MAX_ITEMS: usize = super::asl::MAX_ARRAY_ITEMS;

/// The most bytes of ONE object this reads. A dataset is a caller's own file, and its
/// whole text is held in this process while it is PARSED — and parsing multiplies what
/// it costs — so a larger one fails the read before it is parsed rather than being read
/// in part. The answer itself comes back from the in-process S3 handler, whose own
/// object-plane budget is what bounds the body before this sees it.
pub const MAX_DATASET_BYTES: usize = 4 * 1_024 * 1_024;

/// The most listing pages ONE read follows. S3 answers at most a thousand objects per
/// page, so this covers more objects than [`MAX_ITEMS`] admits and is what stops a
/// listing whose pages never run out.
pub const MAX_PAGES: usize = 64;

/// The most COLUMNS one CSV dataset's header row may name. A header is as wide as
/// whoever wrote the file chose, and every row of the dataset carries one member per
/// column, so the width is bounded on its own before any row is read.
pub const MAX_COLUMNS: usize = 1_024;

/// The most members ALL the items of one dataset may carry TOGETHER — the PRODUCT of
/// how many columns a CSV dataset names and how many rows follow them, both of which
/// that one file chooses. Bounding each dimension alone bounds no product, so the
/// members are counted as the rows are built and a dataset past this fails the read.
pub const MAX_ITEM_MEMBERS: usize = 1_000_000;

/// The `Resource` values an `ItemReader` names its dataset's location with.
const LIST_OBJECTS_RESOURCE: &str = "arn:aws:states:::s3:listObjectsV2";
/// See [`LIST_OBJECTS_RESOURCE`].
const GET_OBJECT_RESOURCE: &str = "arn:aws:states:::s3:getObject";

/// The members an `ItemReader` object carries, which is what one written with any
/// other is refused for.
const READER_MEMBERS: &[&str] = &["Resource", "ReaderConfig", "Parameters", "Arguments"];

/// The members its `ReaderConfig` carries.
const CONFIG_MEMBERS: &[&str] = &[
    "InputType",
    "CSVHeaderLocation",
    "CSVHeaders",
    "CSVDelimiter",
    "MaxItems",
    "MaxItemsPath",
];

/// Where a CSV dataset's header row comes from.
const HEADER_IN_FIRST_ROW: &str = "FIRST_ROW";
/// See [`HEADER_IN_FIRST_ROW`].
const HEADER_GIVEN: &str = "GIVEN";

/// The delimiters `CSVDelimiter` names, each with the byte it stands for.
const DELIMITERS: &[(&str, u8)] = &[
    ("COMMA", b','),
    ("PIPE", b'|'),
    ("SEMICOLON", b';'),
    ("SPACE", b' '),
    ("TAB", b'\t'),
];

/// How the bytes of one object are read as items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputType {
    /// Comma-separated values, with a header row naming the members.
    Csv,
    /// One JSON array, each element an item.
    Json,
    /// JSON Lines: one JSON value per line, each an item.
    JsonLines,
}

/// The `InputType` values this reads, each as the member spells it.
const INPUT_TYPES: &[(&str, InputType)] = &[
    ("CSV", InputType::Csv),
    ("JSON", InputType::Json),
    ("JSONL", InputType::JsonLines),
];

/// Which dataset an `ItemReader` reads, and everything its members resolved to before
/// any item ran.
#[derive(Debug, Clone)]
pub struct Reader {
    /// The bucket the dataset lives in.
    bucket: String,
    /// What is read out of it.
    source: Source,
    /// "MaxItems — limits the number of data items passed", as the member or its
    /// `MaxItemsPath` form resolved; `0` is the member's own default, every item the
    /// dataset holds.
    max_items: usize,
}

/// What an `ItemReader`'s `Resource` names.
#[derive(Debug, Clone)]
enum Source {
    /// Every object of the bucket whose key begins with this prefix — empty for the
    /// whole bucket.
    Objects { prefix: String },
    /// The one object this key names, read as `input` says.
    Object { key: String, input: InputType, csv: Csv },
}

/// How a CSV dataset's rows are read: where the member names come from, and the
/// delimiter between the columns.
#[derive(Debug, Clone)]
struct Csv {
    /// The headers the definition GAVE, for `"CSVHeaderLocation": "GIVEN"`; empty when
    /// the dataset's own first row carries them.
    given: Vec<String>,
    /// The byte between two columns, which `CSVDelimiter` names.
    delimiter: u8,
}

/// The `ItemReader` of one `Map` state, as its members resolve.
///
/// `parameters` is the `Parameters` (or JSONata `Arguments`) document the reader
/// declares, already rendered by the state's own query language, and `max_items` is
/// what `MaxItems` (or `MaxItemsPath`) resolved to in that same language — the reader's
/// own members are a definition's text, so nothing here evaluates anything.
///
/// The `Err` is the failure the `Map` state reports: a reader this emulator does not
/// read carries ASL's [`super::asl::RUNTIME_ERROR`] and says what was not read, while
/// one whose own members do not describe a dataset carries [`ITEM_READER_ERROR`].
pub fn read(
    state: &str,
    reader: &Value,
    parameters: &Value,
    max_items: usize,
) -> Result<Reader, Failed> {
    let Some(members) = reader.as_object() else {
        return Err(malformed(
            state,
            format!("'ItemReader' must be an object, not {}", super::asl::echoed(&reader.to_string())),
        ));
    };
    if let Some(member) = unknown(members, READER_MEMBERS) {
        return Err(not_read(
            state,
            format!("carries the 'ItemReader' member '{member}'"),
        ));
    }
    let resource = members
        .get("Resource")
        .and_then(Value::as_str)
        .ok_or_else(|| malformed(state, "'ItemReader' must carry a 'Resource' string".to_string()))?;
    let empty = Map::new();
    let config = match members.get("ReaderConfig") {
        None => &empty,
        Some(Value::Object(config)) => config,
        Some(_) => {
            return Err(malformed(
                state,
                "'ReaderConfig' must be an object".to_string(),
            ))
        }
    };
    if let Some(member) = unknown(config, CONFIG_MEMBERS) {
        return Err(not_read(
            state,
            format!("carries the 'ReaderConfig' member '{member}'"),
        ));
    }
    let bucket = text(parameters, "Bucket").ok_or_else(|| {
        malformed(
            state,
            "the 'ItemReader' parameters must name the 'Bucket' the dataset lives in".to_string(),
        )
    })?;
    let source = match resource {
        LIST_OBJECTS_RESOURCE => Source::Objects {
            prefix: text(parameters, "Prefix").unwrap_or_default(),
        },
        GET_OBJECT_RESOURCE => {
            let key = text(parameters, "Key").ok_or_else(|| {
                malformed(
                    state,
                    "the 'ItemReader' parameters must name the 'Key' of the object the dataset is"
                        .to_string(),
                )
            })?;
            Source::Object {
                key,
                input: input_type(state, config)?,
                csv: csv_reading(state, config)?,
            }
        }
        other => {
            return Err(not_read(
                state,
                format!(
                    "names the dataset location '{}', which is not one this emulator reads",
                    super::asl::echoed(other)
                ),
            ))
        }
    };
    Ok(Reader {
        bucket,
        source,
        max_items,
    })
}

/// The `InputType` `config` declares. One object's bytes are items only under an input
/// type that says how, so the member's default is the one the language gives it: "if
/// you don't specify ... CSV".
fn input_type(state: &str, config: &Map<String, Value>) -> Result<InputType, Failed> {
    let Some(declared) = config.get("InputType") else {
        return Ok(InputType::Csv);
    };
    let declared = declared.as_str().ok_or_else(|| {
        malformed(state, "'InputType' must be a string".to_string())
    })?;
    INPUT_TYPES
        .iter()
        .find(|(name, _)| *name == declared)
        .map(|(_, input)| *input)
        .ok_or_else(|| {
            not_read(
                state,
                format!(
                    "declares the dataset's 'InputType' as '{}', which this emulator does not read",
                    super::asl::echoed(declared)
                ),
            )
        })
}

/// How `config` says a CSV dataset's rows are read.
fn csv_reading(state: &str, config: &Map<String, Value>) -> Result<Csv, Failed> {
    let location = config
        .get("CSVHeaderLocation")
        .map_or(Ok(HEADER_IN_FIRST_ROW), |declared| {
            declared.as_str().ok_or_else(|| {
                malformed(state, "'CSVHeaderLocation' must be a string".to_string())
            })
        })?;
    let given: Vec<String> = match config.get("CSVHeaders") {
        None => Vec::new(),
        Some(Value::Array(headers)) => headers
            .iter()
            .map(|header| {
                header.as_str().map(str::to_string).ok_or_else(|| {
                    malformed(state, "every 'CSVHeaders' entry must be a string".to_string())
                })
            })
            .collect::<Result<_, _>>()?,
        Some(_) => {
            return Err(malformed(
                state,
                "'CSVHeaders' must be an array of strings".to_string(),
            ))
        }
    };
    let given = match location {
        HEADER_IN_FIRST_ROW => Vec::new(),
        HEADER_GIVEN if given.is_empty() => {
            return Err(malformed(
                state,
                "'CSVHeaderLocation' is 'GIVEN', so 'CSVHeaders' must name the headers"
                    .to_string(),
            ))
        }
        HEADER_GIVEN => given,
        other => {
            return Err(malformed(
                state,
                format!(
                    "'CSVHeaderLocation' is '{}', which is neither '{HEADER_IN_FIRST_ROW}' nor \
                     '{HEADER_GIVEN}'",
                    super::asl::echoed(other)
                ),
            ))
        }
    };
    let delimiter = match config.get("CSVDelimiter") {
        None => b',',
        Some(Value::String(named)) => DELIMITERS
            .iter()
            .find(|(name, _)| name == named)
            .map(|(_, byte)| *byte)
            .ok_or_else(|| {
                malformed(
                    state,
                    format!(
                        "'CSVDelimiter' is '{}', which is none of {}",
                        super::asl::echoed(named),
                        DELIMITERS
                            .iter()
                            .map(|(name, _)| *name)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )
            })?,
        Some(_) => {
            return Err(malformed(
                state,
                "'CSVDelimiter' must be a string".to_string(),
            ))
        }
    };
    Ok(Csv { given, delimiter })
}

impl Reader {
    /// The items this dataset holds, read through `tasks` — the same seam a `Task`
    /// state's integration call is made over.
    ///
    /// The `Err` is the `Map` state's own failure, raised before any item is iterated.
    pub fn items(&self, state: &str, tasks: &dyn Tasks) -> Result<Vec<Value>, Failed> {
        match &self.source {
            Source::Objects { prefix } => self.objects(state, prefix, tasks),
            Source::Object { key, input, csv } => {
                let body = self.object(state, key, tasks)?;
                let limit = self.limit();
                match input {
                    InputType::Csv => rows(state, &body, csv, limit),
                    InputType::Json => elements(state, &body, limit),
                    InputType::JsonLines => lines(state, &body, limit),
                }
            }
        }
    }

    /// Every object of the bucket whose key begins with `prefix`, as the metadata items
    /// the listing answers.
    fn objects(&self, state: &str, prefix: &str, tasks: &dyn Tasks) -> Result<Vec<Value>, Failed> {
        let call = self.call(state, super::integration::LIST_OBJECTS_OPERATION)?;
        let mut items: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        for _page in 0..MAX_PAGES {
            let mut request = Map::new();
            request.insert("Bucket".to_string(), Value::String(self.bucket.clone()));
            if !prefix.is_empty() {
                request.insert("Prefix".to_string(), Value::String(prefix.to_string()));
            }
            if let Some(token) = &token {
                request.insert(
                    "ContinuationToken".to_string(),
                    Value::String(token.clone()),
                );
            }
            let body = self.answered(state, tasks, &call, &Value::Object(request))?;
            let listing = super::xml_answer::read(body.as_bytes())
                .map_err(|why| failed(state, format!("the listing of the dataset {why}")))?;
            let limit = self.limit();
            for entry in listing.each("Contents") {
                if items.len() >= limit {
                    return Ok(items);
                }
                items.push(object_item(entry));
            }
            if items.len() >= limit {
                return Ok(items);
            }
            let truncated = listing.text_of("IsTruncated") == Some("true");
            let next = listing.text_of("NextContinuationToken").unwrap_or_default();
            if !truncated || next.is_empty() {
                return Ok(items);
            }
            token = Some(next.to_string());
        }
        Err(failed(
            state,
            format!("the dataset's listing is still not complete after {MAX_PAGES} pages"),
        ))
    }

    /// The bytes of the one object `key` names.
    fn object(&self, state: &str, key: &str, tasks: &dyn Tasks) -> Result<String, Failed> {
        let call = self.call(state, super::integration::GET_OBJECT_OPERATION)?;
        let mut request = Map::new();
        request.insert("Bucket".to_string(), Value::String(self.bucket.clone()));
        request.insert("Key".to_string(), Value::String(key.to_string()));
        self.answered(state, tasks, &call, &Value::Object(request))
    }

    /// The S3 call `operation` names, for the dataset this reader reads.
    fn call(&self, state: &str, operation: &str) -> Result<Call, Failed> {
        super::integration::dataset_call(operation).ok_or_else(|| {
            failed(
                state,
                format!("this emulator binds no '{operation}' request to read a dataset with"),
            )
        })
    }

    /// What one dataset call answered, as the TEXT its body carried.
    ///
    /// The answer's body is the object's own bytes for a read of one object, and the
    /// listing's XML document for a listing; a body that parsed as a JSON document is
    /// reported back as that document's own text, so what this hands on is always the
    /// bytes the dataset holds rather than a reading of them.
    fn answered(
        &self,
        state: &str,
        tasks: &dyn Tasks,
        call: &Call,
        request: &Value,
    ) -> Result<String, Failed> {
        if let Some(denied) = tasks.authorize(call, request) {
            return Err(denied);
        }
        let answer = tasks
            .perform(call, request, None)
            .map_err(|refused| failed(state, format!("{} ({})", refused.cause, refused.error)))?;
        let body = match answer.get(super::integration::DATASET_BODY) {
            None | Some(Value::Null) => String::new(),
            Some(Value::String(text)) => text.clone(),
            Some(value) => value.to_string(),
        };
        if body.len() > MAX_DATASET_BYTES {
            return Err(failed(
                state,
                format!(
                    "the dataset holds {} bytes, more than the {MAX_DATASET_BYTES} this emulator \
                     reads",
                    body.len()
                ),
            ));
        }
        Ok(body)
    }

    /// How many items this reader may deliver at most: "MaxItems — limits the number of
    /// data items passed", held under this emulator's own ceiling, and that ceiling
    /// alone where the member declares no bound.
    ///
    /// Asked BEFORE the items are built, so a dataset holding more than the reader
    /// delivers is never materialised past the bound.
    fn limit(&self) -> usize {
        match self.max_items {
            0 => MAX_ITEMS,
            declared => declared.min(MAX_ITEMS),
        }
    }
}

/// One listing entry as the metadata item a `Map` state iterates: the object's `Etag`,
/// `Key`, `LastModified`, `Size` and `StorageClass`, every value the listing's own and
/// a member the listing did not carry absent rather than filled in.
///
/// `LastModified` is reported as whole SECONDS since the epoch, which is the number an
/// SDK's date renders as in the JSON a state reads; `Size` is the figure the listing
/// answered.
fn object_item(entry: &super::xml_answer::Element) -> Value {
    let mut item = Map::new();
    if let Some(etag) = entry.text_of("ETag") {
        item.insert("Etag".to_string(), Value::String(etag.to_string()));
    }
    if let Some(key) = entry.text_of("Key") {
        item.insert("Key".to_string(), Value::String(key.to_string()));
    }
    if let Some(modified) = entry.text_of("LastModified").and_then(epoch_seconds) {
        item.insert("LastModified".to_string(), Value::Number(modified.into()));
    }
    if let Some(size) = entry.text_of("Size").and_then(|size| size.parse::<i64>().ok()) {
        item.insert("Size".to_string(), Value::Number(size.into()));
    }
    if let Some(class) = entry.text_of("StorageClass") {
        item.insert("StorageClass".to_string(), Value::String(class.to_string()));
    }
    Value::Object(item)
}

/// `text`, an ISO 8601 instant, in whole seconds since the Unix epoch; `None` for text
/// that is not one.
fn epoch_seconds(text: &str) -> Option<i64> {
    let parsed = ::aws_smithy_types::DateTime::from_str(
        text,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    Some(parsed.secs())
}

/// The items a CSV dataset holds: one per row after the header, each member a header
/// against that row's column at the same position, and at most `limit` of them.
///
/// Every value is the STRING the file holds — a CSV file carries text, and nothing here
/// guesses a number or a boolean out of it. A row shorter than the header carries the
/// empty string for the columns it does not reach, and a column past the last header
/// names no member.
///
/// A CSV dataset chooses BOTH of the dimensions the items it yields are the product of
/// — how many columns its header declares, and how many rows follow it — so each is
/// bounded here and the product is bounded as the rows are built: the header against
/// [`MAX_COLUMNS`], the rows against `limit`, and the members they make together
/// against [`MAX_ITEM_MEMBERS`]. A dataset past one of those fails the read rather than
/// being read in part.
fn rows(state: &str, body: &str, csv: &Csv, limit: usize) -> Result<Vec<Value>, Failed> {
    let mut reader = ::csv::ReaderBuilder::new()
        .delimiter(csv.delimiter)
        // The header row is read as a RECORD either way: when the definition gave the
        // headers, the file's first row is an item like every other.
        .has_headers(false)
        .flexible(true)
        .from_reader(body.as_bytes());
    let mut records = reader.records();
    let headers: Vec<String> = if csv.given.is_empty() {
        let Some(first) = records.next() else {
            // A CSV dataset whose header row is its own first row and that holds no
            // row at all names no members, so it holds no items either.
            return Ok(Vec::new());
        };
        first
            .map_err(|error| failed(state, format!("the dataset's header row {error}")))?
            .iter()
            .map(str::to_string)
            .collect()
    } else {
        csv.given.clone()
    };
    if headers.len() > MAX_COLUMNS {
        return Err(failed(
            state,
            format!(
                "the dataset names {} columns, more than the {MAX_COLUMNS} this emulator reads",
                headers.len()
            ),
        ));
    }
    let mut items = Vec::new();
    let mut members = 0usize;
    for record in records {
        if items.len() >= limit {
            break;
        }
        let record =
            record.map_err(|error| failed(state, format!("a row of the dataset {error}")))?;
        members = members.saturating_add(headers.len());
        if members > MAX_ITEM_MEMBERS {
            return Err(failed(
                state,
                format!(
                    "the dataset's columns and rows make more than the {MAX_ITEM_MEMBERS} item \
                     members this emulator reads"
                ),
            ));
        }
        let mut item = Map::new();
        for (position, header) in headers.iter().enumerate() {
            item.insert(
                header.clone(),
                Value::String(record.get(position).unwrap_or_default().to_string()),
            );
        }
        items.push(Value::Object(item));
    }
    Ok(items)
}

/// The items a `JSON` dataset holds: the elements of the one array it is, at most
/// `limit` of them.
fn elements(state: &str, body: &str, limit: usize) -> Result<Vec<Value>, Failed> {
    let document: Value = serde_json::from_str(body.trim())
        .map_err(|error| failed(state, format!("the dataset is not JSON: {error}")))?;
    match document {
        Value::Array(mut items) => {
            items.truncate(limit);
            Ok(items)
        }
        other => Err(failed(
            state,
            format!(
                "a 'JSON' dataset must hold one array of items, and this one holds {}",
                kind_of(&other)
            ),
        )),
    }
}

/// The items a `JSONL` dataset holds: one per line, each line one JSON value, at most
/// `limit` of them. A blank line holds no value, so it names no item.
fn lines(state: &str, body: &str, limit: usize) -> Result<Vec<Value>, Failed> {
    let mut items = Vec::new();
    for (number, line) in body.lines().enumerate() {
        if items.len() >= limit {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let item: Value = serde_json::from_str(line).map_err(|error| {
            failed(
                state,
                format!("line {} of the dataset is not JSON: {error}", number + 1),
            )
        })?;
        items.push(item);
    }
    Ok(items)
}

/// What kind of document `value` is, for a cause that says what a dataset held instead
/// of what it owed.
fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// The first member of `members` that is not one of `allowed`
/// ([`super::unknown_member`]).
fn unknown(members: &Map<String, Value>, allowed: &[&str]) -> Option<String> {
    super::unknown_member(members, allowed)
}

/// The text `document` names `member` with ([`super::declared_text`]).
fn text(document: &Value, member: &str) -> Option<String> {
    super::declared_text(document, member)
}

/// The failure a state whose `ItemReader` could not be READ reports: the ASL error name
/// for exactly that, and a cause saying what about the dataset stopped it.
fn failed(state: &str, why: String) -> Failed {
    Failed {
        error: ITEM_READER_ERROR.to_string(),
        cause: format!("the 'ItemReader' of state '{state}' could not be read: {why}"),
    }
}

/// The failure a state whose `ItemReader` does not DESCRIBE a dataset reports: the same
/// error name, because the reader is still what could not be read.
fn malformed(state: &str, why: String) -> Failed {
    failed(state, why)
}

/// The failure a state asking for a dataset this emulator does not read reports: a gap
/// in this emulator rather than the service refusing anything, which is ASL's own
/// runtime error with a cause saying what was not read.
fn not_read(state: &str, what: String) -> Failed {
    Failed {
        error: super::asl::RUNTIME_ERROR.to_string(),
        cause: format!(
            "the 'ItemReader' of state '{state}' {what}, which this emulator does not read, so \
             nothing was run for it"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A reader of one object, as `read` resolves it.
    fn object_reader(config: Value, key: &str) -> Reader {
        read(
            "Fan",
            &json!({
                "Resource": GET_OBJECT_RESOURCE,
                "ReaderConfig": config,
                "Parameters": { "Bucket": "b", "Key": key },
            }),
            &json!({ "Bucket": "b", "Key": key }),
            0,
        )
        .expect("the reader resolves")
    }

    #[test]
    fn a_csv_dataset_delivers_header_keyed_string_columns() {
        let rows: &[(&str, Value, &str, Value)] = &[
            (
                "the first row is the header",
                json!({ "InputType": "CSV" }),
                "name,rating\nalpha,5\nbeta,3\n",
                json!([
                    { "name": "alpha", "rating": "5" },
                    { "name": "beta", "rating": "3" },
                ]),
            ),
            (
                "given headers make the first row an item",
                json!({
                    "InputType": "CSV",
                    "CSVHeaderLocation": "GIVEN",
                    "CSVHeaders": ["name", "rating"],
                }),
                "alpha,5\nbeta,3\n",
                json!([
                    { "name": "alpha", "rating": "5" },
                    { "name": "beta", "rating": "3" },
                ]),
            ),
            (
                "a short row carries the empty string",
                json!({ "InputType": "CSV" }),
                "name,rating\nalpha\n",
                json!([{ "name": "alpha", "rating": "" }]),
            ),
            (
                "a quoted column keeps its delimiter",
                json!({ "InputType": "CSV" }),
                "name,note\nalpha,\"one, two\"\n",
                json!([{ "name": "alpha", "note": "one, two" }]),
            ),
            (
                "another delimiter is the one named",
                json!({ "InputType": "CSV", "CSVDelimiter": "TAB" }),
                "name\trating\nalpha\t5\n",
                json!([{ "name": "alpha", "rating": "5" }]),
            ),
            (
                "a header row with no rows under it holds no items",
                json!({ "InputType": "CSV" }),
                "name,rating\n",
                json!([]),
            ),
        ];
        for (case, config, body, expected) in rows {
            let reader = object_reader(config.clone(), "dataset.csv");
            let Source::Object { csv, .. } = &reader.source else {
                panic!("case {case:?}: the reader reads one object");
            };
            let items = rows_of(body, csv);
            assert_eq!(
                Value::Array(items.clone()),
                *expected,
                "case {case:?}: expected {expected:?}, got {items:?}"
            );
        }
    }

    /// [`rows`] over one body, for the table above.
    fn rows_of(body: &str, csv: &Csv) -> Vec<Value> {
        rows("Fan", body, csv, MAX_ITEMS).expect("the dataset reads")
    }

    #[test]
    fn a_json_dataset_must_hold_one_array() {
        let items = elements("Fan", " [ {\"id\": 1}, 2 ] ", MAX_ITEMS).expect("the array reads");
        assert_eq!(
            Value::Array(items.clone()),
            json!([{ "id": 1 }, 2]),
            "every element of the array is one item, got {items:?}"
        );
        for (case, body) in [
            ("an object", "{\"id\": 1}"),
            ("a number", "7"),
            ("not JSON at all", "name,rating"),
        ] {
            let refused = elements("Fan", body, MAX_ITEMS)
                .expect_err("a dataset that is no array is refused");
            assert_eq!(
                refused.error, ITEM_READER_ERROR,
                "case {case:?}: a dataset that is not an array fails the reader, got {refused:?}"
            );
        }
    }

    #[test]
    fn a_jsonl_dataset_delivers_one_item_per_line() {
        let items =
            lines("Fan", "{\"id\": 1}\n\n{\"id\": 2}\n", MAX_ITEMS).expect("the lines read");
        assert_eq!(
            Value::Array(items.clone()),
            json!([{ "id": 1 }, { "id": 2 }]),
            "each non-blank line is one item, got {items:?}"
        );
        let refused = lines("Fan", "{\"id\": 1}\nnot json\n", MAX_ITEMS)
            .expect_err("a line that is not JSON is refused");
        assert_eq!(
            refused.error, ITEM_READER_ERROR,
            "a line that is not JSON fails the reader, got {refused:?}"
        );
    }

    #[test]
    fn a_reader_this_emulator_does_not_read_says_so() {
        let rows: &[(&str, Value)] = &[
            (
                "another dataset location",
                json!({ "Resource": "arn:aws:states:::s3:getObject.waitForTaskToken" }),
            ),
            (
                "an input type this emulator does not read",
                json!({
                    "Resource": GET_OBJECT_RESOURCE,
                    "ReaderConfig": { "InputType": "PARQUET" },
                }),
            ),
            (
                "a reader member this emulator does not interpret",
                json!({ "Resource": GET_OBJECT_RESOURCE, "Mode": "DISTRIBUTED" }),
            ),
        ];
        for (case, reader) in rows {
            let refused = read("Fan", reader, &json!({ "Bucket": "b", "Key": "k" }), 0)
                .expect_err("the reader is refused");
            assert_eq!(
                refused.error,
                super::super::asl::RUNTIME_ERROR,
                "case {case:?}: a reader nothing was run for carries ASL's runtime error, got \
                 {refused:?}"
            );
            assert!(
                refused.cause.contains("nothing was run for it"),
                "case {case:?}: the cause says nothing was run, got {:?}",
                refused.cause
            );
        }
    }

    #[test]
    fn a_reader_naming_no_dataset_fails_the_item_reader() {
        let rows: &[(&str, Value, Value)] = &[
            (
                "no bucket",
                json!({ "Resource": LIST_OBJECTS_RESOURCE }),
                json!({}),
            ),
            (
                "no key for a single object",
                json!({ "Resource": GET_OBJECT_RESOURCE }),
                json!({ "Bucket": "b" }),
            ),
            (
                "given headers without the headers",
                json!({
                    "Resource": GET_OBJECT_RESOURCE,
                    "ReaderConfig": { "InputType": "CSV", "CSVHeaderLocation": "GIVEN" },
                }),
                json!({ "Bucket": "b", "Key": "k" }),
            ),
            (
                "a delimiter no name stands for",
                json!({
                    "Resource": GET_OBJECT_RESOURCE,
                    "ReaderConfig": { "InputType": "CSV", "CSVDelimiter": "COLON" },
                }),
                json!({ "Bucket": "b", "Key": "k" }),
            ),
        ];
        for (case, reader, parameters) in rows {
            let refused = read("Fan", reader, parameters, 0).expect_err("the reader is refused");
            assert_eq!(
                refused.error, ITEM_READER_ERROR,
                "case {case:?}: a reader that names no dataset fails the item reader, got \
                 {refused:?}"
            );
        }
    }

    #[test]
    fn a_listing_entry_is_the_objects_own_metadata() {
        let listing = super::super::xml_answer::read(
            b"<ListBucketResult><Contents><Key>data/one.json</Key>\
              <LastModified>2026-01-02T03:04:05Z</LastModified><ETag>\"abc\"</ETag>\
              <Size>42</Size><StorageClass>STANDARD</StorageClass></Contents>\
              </ListBucketResult>",
        )
        .expect("the listing reads");
        let entry = listing.child("Contents").expect("the listing has an entry");
        let item = object_item(entry);
        assert_eq!(
            item,
            json!({
                "Etag": "\"abc\"",
                "Key": "data/one.json",
                "LastModified": 1_767_323_045_i64,
                "Size": 42,
                "StorageClass": "STANDARD",
            }),
            "the item is the listing's own metadata, got {item:?}"
        );
    }

    #[test]
    fn max_items_bounds_what_the_reader_delivers() {
        let rows: &[(&str, usize, usize)] = &[
            ("no limit", 0, MAX_ITEMS),
            ("two", 2, 2),
            ("one", 1, 1),
            ("past this emulator's own ceiling", MAX_ITEMS + 5, MAX_ITEMS),
        ];
        for (case, declared, expected) in rows {
            let reader = Reader {
                bucket: "b".to_string(),
                source: Source::Objects {
                    prefix: String::new(),
                },
                max_items: *declared,
            };
            assert_eq!(
                reader.limit(),
                *expected,
                "case {case:?}: expected a limit of {expected}, got {}",
                reader.limit()
            );
        }
    }

    #[test]
    fn a_dataset_past_a_ceiling_fails_the_read() {
        let wide: String = (0..=MAX_COLUMNS).map(|column| format!("c{column},")).collect();
        let csv = Csv {
            given: Vec::new(),
            delimiter: b',',
        };
        let refused = rows("Fan", &format!("{wide}\n1\n"), &csv, MAX_ITEMS)
            .expect_err("a header wider than this emulator reads is refused");
        assert_eq!(
            refused.error, ITEM_READER_ERROR,
            "a header past MAX_COLUMNS fails the reader, got {refused:?}"
        );

        // A header inside the column bound, with enough rows that their PRODUCT passes
        // the member budget: neither dimension alone is past its own bound.
        let columns = 1_000;
        let header: String = (0..columns).map(|column| format!("c{column},")).collect();
        let row = ",".repeat(columns);
        let body = format!("{header}\n{}", format!("{row}\n").repeat(1_100));
        let refused = rows("Fan", &body, &csv, MAX_ITEMS)
            .expect_err("a product past the member budget is refused");
        assert_eq!(
            refused.error, ITEM_READER_ERROR,
            "a column count times a row count past MAX_ITEM_MEMBERS fails the reader, got \
             {refused:?}"
        );
    }

    #[test]
    fn a_limit_bounds_every_dataset_form_as_the_items_are_built() {
        let csv = Csv {
            given: Vec::new(),
            delimiter: b',',
        };
        let built = rows("Fan", "a\n1\n2\n3\n", &csv, 2).expect("the rows read");
        assert_eq!(
            built.len(),
            2,
            "a CSV dataset builds no more rows than the limit, got {built:?}"
        );
        let built = lines("Fan", "1\n2\n3\n", 2).expect("the lines read");
        assert_eq!(
            built.len(),
            2,
            "a JSON Lines dataset builds no more items than the limit, got {built:?}"
        );
        let built = elements("Fan", "[1,2,3]", 2).expect("the array reads");
        assert_eq!(
            built.len(),
            2,
            "a JSON dataset delivers no more items than the limit, got {built:?}"
        );
    }
}
