// SPDX-License-Identifier: CC0-1.0

//! @op ImportTerminology
//!
//! Parses the uploaded custom-terminology file and stores one terminology
//! record, then reports that record. Re-importing a name with the only modelled
//! merge strategy (`OVERWRITE`) replaces the stored terms and moves
//! `LastUpdatedAt` on rather than creating a second resource.
//!
//! Shared types live in `super::types`; the stored shapes and everything the
//! five terminology operations report by live in [`crate::state`].
//!
//! ---
//!
//! # Modeled contract for `ImportTerminology`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-translate-awsshinefrontendservice-20170701/smithy-model/com-amazonaws-translate-awsshinefrontendservice-20170701.json")
//! ```
//!
//! Creates or updates a custom terminology, depending on whether one already exists for the
//! given terminology name. Importing a terminology with the same name as an existing one
//! will merge the terminologies based on the chosen merge strategy. The only supported
//! merge strategy is OVERWRITE, where the imported terminology overwrites the existing
//! terminology of the same name. If you import a terminology that overwrites an existing
//! one, the new terminology takes up to 10 minutes to fully propagate. After that,
//! translations have access to the new terminology.
//!
//! ## Input
//!
//! * `Description` (Description, len 0..256, pattern ^[\P{M}\p{M}]{0,256}$) — The description
//!   of the custom terminology being imported.
//! * `EncryptionKey` (EncryptionKey) — The encryption key for the custom terminology being
//!   imported.
//! * `MergeStrategy` (MergeStrategy, one of: OVERWRITE) **required** — The merge strategy of
//!   the custom terminology being imported. Currently, only the OVERWRITE merge strategy is
//!   supported. In this case, the imported terminology will overwrite an existing terminology
//!   of the same name.
//! * `Name` (ResourceName, len 1..256, pattern ^([A-Za-z0-9-]_?)+$) **required** — The name
//!   of the custom terminology being imported.
//! * `Tags` (TagList, len 0..200) — Tags to be associated with this resource. A tag is a
//!   key-value pair that adds metadata to a resource. Each tag key for the resource must be
//!   unique. For more information, see Tagging your resources.
//! * `TerminologyData` (TerminologyData) **required** — The terminology data for the custom
//!   terminology being imported.
//!
//! ## Output
//!
//! * `AuxiliaryDataLocation` (TerminologyDataLocation) — The Amazon S3 location of a file
//!   that provides any errors or warnings that were produced by your input file. This file
//!   was created when Amazon Translate attempted to create a terminology resource. The
//!   location is returned as a presigned URL to that has a 30 minute expiration.
//! * `TerminologyProperties` (TerminologyProperties) — The properties of the custom
//!   terminology being imported.
//!
//! ## Errors
//!
//! * `ConcurrentModificationException` (HTTP 409) — Another modification is being made. That
//!   modification must complete before you can make your change.
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `InvalidParameterValueException` (HTTP 400) — The value of the parameter is not valid.
//!   Review the value of the parameter you are using to correct it, and then retry your
//!   operation.
//! * `LimitExceededException` (HTTP 400) — The specified limit has been exceeded. Review your
//!   request and retry it with a quantity below the stated limit.
//! * `TooManyRequestsException` (HTTP 429) — You have made too many requests within a short
//!   period of time. Wait for a short time and then try your request again.
//! * `TooManyTagsException` (HTTP 400) — You have added too many tags to this resource. The
//!   maximum is 50 tags.

#[allow(unused_imports)]
use super::types::*;

use std::collections::BTreeMap;

use crate::state::{
    auxiliary_data_location, canonical_language, content_revision, counted,
    invalid_parameter_value, request_region, service_state, terminology_arn,
    terminology_properties, too_many_tags, TermEntry, Terminology, FORMAT_CSV, FORMAT_TMX,
    FORMAT_TSV, MAX_TAGS_PER_RESOURCE, MAX_TERMINOLOGY_FILE_BYTES, MAX_STORED_TERMINOLOGIES,
    MAX_TERM_TEXT_CHARS, MAX_TERMS_PER_TERMINOLOGY,
};

/// The member a refusal about the uploaded file names, as the caller addresses it.
const FILE_MEMBER: &str = "TerminologyData.File";

/// The fewest language columns a terminology file declares: one to translate
/// from and one to translate into.
const MIN_LANGUAGE_COLUMNS: usize = 2;

/// The fewest non-empty cells a row needs to state a term in one language and
/// its translation in another; a row with fewer is counted as skipped rather
/// than refusing the whole file.
const MIN_FILLED_CELLS: usize = 2;

/// What one terminology file declares: the language code of each column, in the
/// order the file declares them, one entry per usable row, and how many rows
/// the service could not use.
struct ParsedTerminology {
    languages: Vec<&'static str>,
    entries: Vec<TermEntry>,
    skipped: usize,
}

/// Parse a CSV or TSV terminology file: the first row names the languages of
/// the columns, every later row is one term.
///
/// The parsing itself is the `csv` crate's; this only interprets the rows it
/// hands back. `flexible` accepts a short row (the service skips it) rather
/// than failing the whole file on one ragged line.
fn parse_delimited(file: &[u8], delimiter: u8) -> Result<ParsedTerminology, String> {
    let mut reader = ::csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(false)
        .flexible(true)
        .from_reader(file);
    let mut records = reader.records();
    let header = match records.next() {
        Some(row) => row.map_err(|error| format!("the header row could not be read ({error})"))?,
        None => return Err("the file holds no rows".to_string()),
    };
    let languages = language_columns(header.iter())?;

    let mut entries = Vec::new();
    let mut skipped = 0usize;
    for row in records {
        let row = row.map_err(|error| format!("a row could not be read ({error})"))?;
        let mut values = BTreeMap::new();
        let mut oversized = false;
        for (language, cell) in languages.iter().zip(row.iter()) {
            let cell = cell.trim();
            // A cell longer than a term may carry is dropped with its row
            // rather than stored: no response could report it.
            if cell.chars().count() > MAX_TERM_TEXT_CHARS {
                oversized = true;
                break;
            }
            if !cell.is_empty() {
                values.insert((*language).to_string(), cell.to_string());
            }
        }
        if oversized || row.len() > languages.len() || values.len() < MIN_FILLED_CELLS {
            skipped = skipped.saturating_add(1);
            continue;
        }
        entries.push(TermEntry { values });
        if entries.len() > MAX_TERMS_PER_TERMINOLOGY {
            // Stop reading rather than growing the store further: the caller
            // is already past the per-terminology term limit the import
            // refuses below.
            break;
        }
    }
    Ok(ParsedTerminology {
        languages,
        entries,
        skipped,
    })
}

/// The language code of each column of a terminology file's header row.
fn language_columns<'a>(
    declared: impl Iterator<Item = &'a str>,
) -> Result<Vec<&'static str>, String> {
    let mut languages: Vec<&'static str> = Vec::new();
    for column in declared {
        let language = canonical_language(column).ok_or_else(|| {
            format!("{column:?} is not a language Amazon Translate supports")
        })?;
        if languages.contains(&language) {
            return Err(format!("{language} is declared by two columns"));
        }
        languages.push(language);
    }
    if languages.len() < MIN_LANGUAGE_COLUMNS {
        return Err(format!(
            "the file declares {} language column(s), and a terminology needs at least {MIN_LANGUAGE_COLUMNS}",
            languages.len()
        ));
    }
    Ok(languages)
}

/// One language code read off a TMX attribute.
fn tmx_language(
    tag: &::quick_xml::events::BytesStart<'_>,
    attribute: &[u8],
) -> Option<&'static str> {
    for found in tag.attributes() {
        let Ok(found) = found else { continue };
        if found.key.as_ref() != attribute {
            continue;
        }
        let Ok(declared) = ::std::str::from_utf8(found.value.as_ref()) else {
            continue;
        };
        return canonical_language(declared);
    }
    None
}

/// Parse a TMX terminology file: `<header srclang=…>` names the source
/// language, and each `<tu>` holds one `<tuv xml:lang=…><seg>` per language.
///
/// The XML parsing itself is the `quick-xml` crate's; this only interprets the
/// events it hands back, and stops once the file declares more terms than one
/// terminology may hold.
fn parse_tmx(file: &[u8]) -> Result<ParsedTerminology, String> {
    let mut reader = ::quick_xml::Reader::from_reader(file);
    let mut buffer = Vec::new();

    let mut languages: Vec<&'static str> = Vec::new();
    let mut entries: Vec<TermEntry> = Vec::new();
    let mut skipped = 0usize;

    let mut row: Option<BTreeMap<String, String>> = None;
    let mut row_oversized = false;
    let mut unit_language: Option<&'static str> = None;
    let mut segment: Option<String> = None;

    loop {
        let event = reader
            .read_event_into(&mut buffer)
            .map_err(|error| format!("the file is not well-formed XML ({error})"))?;
        match event {
            ::quick_xml::events::Event::Eof => break,
            ::quick_xml::events::Event::Start(tag) | ::quick_xml::events::Event::Empty(tag) => {
                match tag.local_name().as_ref() {
                    b"header" => {
                        if let Some(language) = tmx_language(&tag, b"srclang") {
                            if !languages.contains(&language) {
                                languages.push(language);
                            }
                        }
                    }
                    b"tu" => {
                        row = Some(BTreeMap::new());
                        row_oversized = false;
                    }
                    b"tuv" => {
                        unit_language = tmx_language(&tag, b"xml:lang")
                            .or_else(|| tmx_language(&tag, b"lang"));
                    }
                    b"seg" => segment = Some(String::new()),
                    _ => {}
                }
            }
            ::quick_xml::events::Event::Text(text) => {
                if let Some(segment) = segment.as_mut() {
                    // The two steps the vendored XML codec takes for a text
                    // value: decode the bytes, then resolve the entity
                    // references `quick-xml` leaves verbatim.
                    let decoded = text
                        .xml10_content()
                        .map_err(|error| format!("a segment could not be read ({error})"))?;
                    let unescaped = ::quick_xml::escape::unescape(&decoded)
                        .map_err(|error| format!("a segment could not be read ({error})"))?;
                    segment.push_str(unescaped.as_ref());
                }
            }
            ::quick_xml::events::Event::CData(data) => {
                if let Some(segment) = segment.as_mut() {
                    segment.push_str(&String::from_utf8_lossy(data.as_ref()));
                }
            }
            ::quick_xml::events::Event::End(tag) => match tag.local_name().as_ref() {
                b"seg" => {
                    if let (Some(language), Some(text), Some(row)) =
                        (unit_language, segment.take(), row.as_mut())
                    {
                        let text = text.trim();
                        // A segment longer than a term may carry takes its
                        // whole unit out of the terminology: no response could
                        // report it.
                        if text.chars().count() > MAX_TERM_TEXT_CHARS {
                            row_oversized = true;
                        } else if !text.is_empty() {
                            row.insert(language.to_string(), text.to_string());
                            if !languages.contains(&language) {
                                languages.push(language);
                            }
                        }
                    }
                }
                b"tuv" => unit_language = None,
                b"tu" => {
                    if let Some(values) = row.take() {
                        if row_oversized || values.len() < MIN_FILLED_CELLS {
                            skipped = skipped.saturating_add(1);
                        } else {
                            entries.push(TermEntry { values });
                        }
                    }
                    row_oversized = false;
                    if entries.len() > MAX_TERMS_PER_TERMINOLOGY {
                        break;
                    }
                }
                _ => {}
            },
            _ => {}
        }
        buffer.clear();
    }

    if languages.len() < MIN_LANGUAGE_COLUMNS {
        return Err(format!(
            "the file declares {} language(s), and a terminology needs at least {MIN_LANGUAGE_COLUMNS}",
            languages.len()
        ));
    }
    Ok(ParsedTerminology {
        languages,
        entries,
        skipped,
    })
}

/// Parse the uploaded file in the format the request declared.
fn parse_terminology(file: &[u8], format: &str) -> Result<ParsedTerminology, String> {
    match format {
        FORMAT_CSV => parse_delimited(file, b','),
        FORMAT_TSV => parse_delimited(file, b'\t'),
        FORMAT_TMX => parse_tmx(file),
        other => Err(format!("{other} is not a terminology data format")),
    }
}

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ImportTerminologyError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-translate-awsshinefrontendservice-20170701.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ImportTerminologyError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.translate#ImportTerminology")]
#[derive(Debug)]
pub enum ImportTerminologyErrorModeled {
    ConcurrentModificationException(ConcurrentModificationException),
    InternalServerException(InternalServerException),
    InvalidParameterValueException(InvalidParameterValueException),
    LimitExceededException(LimitExceededException),
    TooManyRequestsException(TooManyRequestsException),
    TooManyTagsException(TooManyTagsException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::import_terminology::ImportTerminologyError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ImportTerminologyError = crate::scaffolded::Scaffolded<ImportTerminologyErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ImportTerminologyErrorModeled> for ImportTerminologyError {
    fn from(modeled: ImportTerminologyErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ImportTerminology` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ImportTerminology<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ImportTerminology<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ImportTerminology<Eng>
{
    type Input = ImportTerminologyInput;
    type Output = ImportTerminologyOutput;
    type Error = ImportTerminologyError;

    const NAME: &'static str = "ImportTerminology";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::IMPORT_TERMINOLOGY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::IMPORT_TERMINOLOGY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = service_state(&*self.engine).ok_or_else(|| {
            ImportTerminologyErrorModeled::InternalServerException(InternalServerException {
                message: Some("the terminology store is unavailable".to_string()),
            })
        })?;
        let region = request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();

        // `MergeStrategy` is an enum whose only value is `OVERWRITE`, so the
        // boundary walk has already refused anything else: reaching here means
        // the request asked for the imported file to overwrite whatever this
        // name already held.
        let file = input.terminology_data.file;

        // Bound the work this request can ask for BEFORE parsing or storing any
        // of it: the file is caller-sized, and both the parse and the stored
        // record grow with it.
        if file.len() > MAX_TERMINOLOGY_FILE_BYTES {
            return Err(ImportTerminologyErrorModeled::LimitExceededException(
                LimitExceededException {
                    message: Some(format!(
                        "The terminology file is {} bytes, which exceeds the maximum of \
                         {MAX_TERMINOLOGY_FILE_BYTES} bytes. Review your request and retry \
                         it with a quantity below the stated limit.",
                        file.len()
                    )),
                },
            )
            .into());
        }

        let tags = input.tags;
        if let Some(requested) = tags.as_ref() {
            if requested.len() > MAX_TAGS_PER_RESOURCE {
                return Err(ImportTerminologyErrorModeled::TooManyTagsException(
                    too_many_tags(&terminology_arn(
                        &region,
                        &input.name,
                        &content_revision(&file),
                    )),
                )
                .into());
            }
        }

        let format = input.terminology_data.format;
        // The parse below is bounded by the file cap above, but it is still the
        // largest piece of work one request asks for: yield first, so a timeout
        // or a cancellation has a point to land on.
        ::tokio::task::yield_now().await;
        let parsed = parse_terminology(&file, &format).map_err(|reason| {
            ImportTerminologyErrorModeled::InvalidParameterValueException(
                invalid_parameter_value(FILE_MEMBER, &reason),
            )
        })?;

        if parsed.entries.len() > MAX_TERMS_PER_TERMINOLOGY {
            return Err(ImportTerminologyErrorModeled::LimitExceededException(
                LimitExceededException {
                    message: Some(format!(
                        "The terminology file declares more than {MAX_TERMS_PER_TERMINOLOGY} \
                         terms. Review your request and retry it with a quantity below the \
                         stated limit."
                    )),
                },
            )
            .into());
        }

        // The first column is the language a uni-directional resource
        // translates from; the rest are the languages it translates into. A
        // multi-directional resource reports the same split and additionally
        // serves every other pair of them.
        let Some((source_language_code, target_language_codes)) = parsed.languages.split_first()
        else {
            return Err(ImportTerminologyErrorModeled::InvalidParameterValueException(
                invalid_parameter_value(FILE_MEMBER, "the file declares no language columns"),
            )
            .into());
        };

        let revision = content_revision(&file);
        let mut store = state.lock();
        let existing = store.terminologies.get(&input.name);

        // A new name grows the store, which only a caller's delete ever
        // shrinks, so the account's terminology quota is checked before the
        // insert. An overwrite of a name the store already holds adds no entry
        // and is never refused by it.
        if existing.is_none() && store.terminologies.len() >= MAX_STORED_TERMINOLOGIES {
            return Err(ImportTerminologyErrorModeled::LimitExceededException(
                LimitExceededException {
                    message: Some(format!(
                        "This account already holds the maximum of \
                         {MAX_STORED_TERMINOLOGIES} custom terminologies. Review your \
                         request and retry it with a quantity below the stated limit."
                    )),
                },
            )
            .into());
        }

        let record = Terminology {
            name: input.name.clone(),
            description: input.description,
            arn: terminology_arn(&region, &input.name, &revision),
            revision,
            source_language_code: (*source_language_code).to_string(),
            target_language_codes: target_language_codes
                .iter()
                .map(|language| (*language).to_string())
                .collect(),
            format,
            directionality: input.terminology_data.directionality,
            encryption_key: input.encryption_key,
            size_bytes: counted(file.len()),
            term_count: counted(parsed.entries.len()),
            skipped_term_count: counted(parsed.skipped),
            // An overwrite of an existing name keeps the instant the resource
            // was first created and moves only `LastUpdatedAt` on.
            created_at: existing.map_or(now, |previous| previous.created_at),
            last_updated_at: now,
            // Tags the request does not restate are the ones the resource
            // already carries: this operation's input has no way to say "drop
            // my tags", which is what `UntagResource` is for.
            tags: tags.unwrap_or_else(|| {
                existing.map(|previous| previous.tags.clone()).unwrap_or_default()
            }),
            terms: parsed.entries,
        };
        store.terminologies.insert(input.name.clone(), record);

        let stored = store.terminologies.get(&input.name).ok_or_else(|| {
            ImportTerminologyErrorModeled::InternalServerException(InternalServerException {
                message: Some("the imported terminology could not be read back".to_string()),
            })
        })?;
        Ok(ImportTerminologyOutput {
            terminology_properties: Some(terminology_properties(stored)),
            auxiliary_data_location: auxiliary_data_location(&region, stored),
        })
    }
}
