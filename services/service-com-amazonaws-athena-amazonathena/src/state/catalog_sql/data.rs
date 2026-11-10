// SPDX-License-Identifier: CC0-1.0

//! The DATA under a catalog table: the objects at the table's location, read as the rows
//! its SerDe says they hold.
//!
//! A Data Catalog table says where its data lives (`StorageDescriptor$Location`) and how
//! to read it (`StorageDescriptor$SerdeInfo` plus the table's own properties), and
//! Athena's answer to a query over that table is computed from the OBJECTS at that
//! location. So the rows a query reads here are the bytes the object store holds, decoded
//! by the SerDe the catalog recorded — never values synthesised from the query or from the
//! table's definition.

use ::std::collections::HashMap;

use ::service_com_amazonaws_glue_awsglue::state as glue_state;
use ::service_com_amazonaws_s3_amazons3::state as s3_state;

use super::ddl;
use super::statement::{self, DeclaredColumn, Relation};
use super::Failure;
use crate::state::query_engine::{self, LocalColumn, LocalTable};

/// The URI scheme a table's location names its data by.
const S3_SCHEME: &str = "s3://";

/// The table property that says how many lines at the head of each object are a header
/// rather than data, as Hive names it.
const SKIP_HEADER_PROPERTY: &str = "skip.header.line.count";

/// The SerDe parameters that carry the character fields are separated by, in the order
/// Hive reads them: the field delimiter, and the serialization format a table that names
/// only that one falls back to. Both names are the SIBLING's — the one the DDL writes
/// them under — so a writer and a reader of one parameter can never spell it differently.
const FIELD_DELIMITER_PARAMETERS: &[&str] = &[
    statement::FIELD_DELIMITER_PARAMETER,
    statement::SERIALIZATION_FORMAT_PARAMETER,
];

/// The SerDe parameter whose value Hive writes a separator's CHARACTER CODE in rather
/// than the character itself: the default field separator is recorded as the code `1`,
/// which is the unit separator and not the digit.
const CODED_PARAMETER: &str = statement::SERIALIZATION_FORMAT_PARAMETER;

/// The largest character code [`CODED_PARAMETER`] can name.
const MAX_CHARACTER_CODE: u32 = 255;

/// The SerDe parameter `OpenCSVSerde` carries its separator in.
const CSV_SEPARATOR_PARAMETER: &str = "separatorChar";

/// The SerDe parameter `OpenCSVSerde` carries its quote character in.
const CSV_QUOTE_PARAMETER: &str = "quoteChar";

/// The SerDe parameter that says what text stands for a NULL value.
const NULL_FORMAT_PARAMETER: &str = "serialization.null.format";

/// The text a field holds when it carries no value, as Hive writes a NULL by default.
const DEFAULT_NULL_TEXT: &str = "\\N";

/// The character fields are separated by when a table's SerDe names none: Hive's own
/// default field separator, the ASCII unit separator.
pub const DEFAULT_FIELD_DELIMITER: char = '\u{1}';

/// The serialization library of the delimited-text SerDe that reads quoted CSV.
const OPEN_CSV_SERDE: &str = "org.apache.hadoop.hive.serde2.OpenCSVSerde";

/// The serialization libraries that read one JSON document per line.
const JSON_SERDES: &[&str] = &[
    "org.apache.hive.hcatalog.data.JsonSerDe",
    "org.apache.hadoop.hive.serde2.JsonSerDe",
    "org.openx.data.jsonserde.JsonSerDe",
];

/// The table property that records which open table format a catalog entry holds its
/// data in, as the engines that write such a table record it.
const TABLE_TYPE_PROPERTY: &str = "table_type";

/// The value [`TABLE_TYPE_PROPERTY`] carries for a table of the Iceberg open table
/// format.
const ICEBERG_TABLE_TYPE: &str = "ICEBERG";

/// The table property that records WHERE an Iceberg table's current metadata file is: the
/// one object every read of such a table has to go through, because the table's schema,
/// its snapshots and the data files of each are named there rather than in the catalog.
const METADATA_LOCATION_PROPERTY: &str = "metadata_location";

/// The serialization library an Iceberg table's storage descriptor carries.
const ICEBERG_SERDE: &str = "org.apache.iceberg.mr.hive.HiveIcebergSerDe";

/// The `InputFormat` an Iceberg table's storage descriptor carries.
const ICEBERG_INPUT_FORMAT: &str = "org.apache.iceberg.mr.hive.HiveIcebergInputFormat";

/// How the objects at a table's location are decoded into rows.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Decoder {
    /// One row per line, fields separated by a character, with an optional quote
    /// character a field may be wrapped in.
    Delimited {
        /// What separates two fields.
        separator: char,
        /// What a field may be wrapped in, when the SerDe quotes its fields.
        quote: Option<char>,
    },
    /// One row per line, each line a JSON document whose members are the columns.
    Json,
}

/// Everything about one catalog table that decides what rows a read of it produces.
#[derive(Debug, Clone)]
pub struct DataSource {
    /// The relation this table was addressed by, for the messages a failed read reports.
    pub relation: Relation,
    /// The name the rows are materialised under in the engine database.
    pub local_name: String,
    /// The table's columns, in order.
    pub columns: Vec<DeclaredColumn>,
    /// Where the data lives, as the catalog recorded it.
    location: Option<String>,
    /// How the objects there are decoded.
    decoder: Decoder,
    /// How many lines at the head of each object are a header.
    skip_header_lines: usize,
    /// What text stands for a NULL value.
    null_text: String,
}

/// What one query may read, which every read of a table charges against.
///
/// ONE budget per statement, carried through the whole resolution and every read it
/// drives, so what bounds the work is the TOTAL — not each factor of it separately: a
/// statement naming many relations, each a view over many more, spends the same relation
/// count as one naming them directly, and a deadline stops the resolution whatever it has
/// reached.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    /// How many relations this statement may still resolve.
    pub relations_left: usize,
    /// How many rows one query may still materialise.
    pub rows_left: usize,
    /// How many VALUES one query may still materialise: a row of a wide table costs more
    /// than a row of a narrow one, and both the rows an object holds and the columns a
    /// table declares are caller-chosen, so what is bounded is their PRODUCT.
    pub cells_left: usize,
    /// How many bytes of object data one query may still read.
    pub bytes_left: usize,
    /// When the resolution must stop, whatever is left of the counts above.
    pub deadline: ::std::time::Instant,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            relations_left: query_engine::MAX_RESOLVED_RELATIONS,
            rows_left: query_engine::MAX_MATERIALISED_ROWS,
            cells_left: query_engine::MAX_MATERIALISED_CELLS,
            bytes_left: query_engine::MAX_MATERIALISED_BYTES,
            deadline: ::std::time::Instant::now() + query_engine::MAX_QUERY_DEADLINE,
        }
    }
}

impl Budget {
    /// Charge ONE relation against this budget, or say that the statement has reached
    /// what this emulator resolves for one query.
    pub fn charge_relation(&mut self) -> Result<(), Failure> {
        self.charge_deadline()?;
        if self.relations_left == 0 {
            return Err(Failure::emulator(format!(
                "this statement reads more than the {} relations this emulator resolves \
                 for one query",
                query_engine::MAX_RESOLVED_RELATIONS
            )));
        }
        self.relations_left -= 1;
        Ok(())
    }

    /// Charge ONE row of `width` values against this budget, BEFORE it is built: what the
    /// budget bounds is what a read allocates, not only what it keeps, so a decode stops
    /// at the first row over budget rather than after the whole object is in memory.
    pub fn charge_row(&mut self, relation: &Relation, width: usize) -> Result<(), Failure> {
        self.charge_deadline()?;
        if self.rows_left == 0 {
            return Err(Failure::emulator(format!(
                "reading {}.{} would materialise more than the {} rows this emulator holds \
                 for one query",
                relation.database,
                relation.table,
                query_engine::MAX_MATERIALISED_ROWS
            )));
        }
        if width > self.cells_left {
            return Err(Failure::emulator(format!(
                "reading {}.{} would materialise more than the {} values this emulator \
                 holds for one query",
                relation.database,
                relation.table,
                query_engine::MAX_MATERIALISED_CELLS
            )));
        }
        self.rows_left -= 1;
        self.cells_left -= width;
        Ok(())
    }

    /// Whether the time this emulator gives one statement is spent.
    pub fn charge_deadline(&self) -> Result<(), Failure> {
        if ::std::time::Instant::now() >= self.deadline {
            return Err(Failure::emulator(format!(
                "this statement spent the {:?} this emulator gives one query before it \
                 could read what it names",
                query_engine::MAX_QUERY_DEADLINE
            )));
        }
        Ok(())
    }
}

impl DataSource {
    /// How one stored catalog table is read, or the failure to report for a table this
    /// emulator's engine cannot read the storage format of.
    pub fn of(
        relation: &Relation,
        table: &glue_state::TableEntry,
    ) -> Result<Self, Failure> {
        let storage = table.storage_descriptor.as_ref();
        let serde = storage.and_then(|storage| storage.serde_info.as_ref());
        let serde_parameters: HashMap<String, String> = serde
            .and_then(|serde| serde.parameters.clone())
            .unwrap_or_default();
        let library = serde
            .and_then(|serde| serde.serialization_library.clone())
            .unwrap_or_else(|| ddl::LAZY_SIMPLE_SERDE.to_string());
        let input_format = storage.and_then(|storage| storage.input_format.clone());
        let table_parameters = table.parameters.clone().unwrap_or_default();
        // A table of an open table format is read THROUGH its own metadata, so one the
        // catalog marks as such and holds no metadata location for cannot be read at all
        // — which is a failure of the caller's own catalog entry, of Athena's published
        // class for it, and not a storage format this engine merely does not decode.
        if is_iceberg(&table_parameters, &library, input_format.as_deref())
            && property(&table_parameters, METADATA_LOCATION_PROPERTY).is_none()
        {
            return Err(Failure::iceberg_without_metadata_location());
        }
        let decoder = decoder_of(&library, input_format.as_deref(), &serde_parameters)
            .ok_or_else(|| {
                Failure::emulator(format!(
                    "the data of {}.{} is stored as {library}, which this emulator's query \
                     engine does not read",
                    relation.database, relation.table
                ))
            })?;
        Ok(Self {
            relation: relation.clone(),
            local_name: relation.local_name(),
            columns: ddl::table_columns(table),
            location: storage.and_then(|storage| storage.location.clone()),
            decoder,
            skip_header_lines: table_parameters
                .get(SKIP_HEADER_PROPERTY)
                .and_then(|lines| lines.parse().ok())
                .unwrap_or(0),
            null_text: serde_parameters
                .get(NULL_FORMAT_PARAMETER)
                .cloned()
                .unwrap_or_else(|| DEFAULT_NULL_TEXT.to_string()),
        })
    }

    /// The table as the engine holds it: its columns with the type each is read as, and
    /// the rows the objects at its location decode to.
    pub fn read(
        &self,
        objects: &s3_state::ComAmazonawsS3Amazons3State,
        budget: &mut Budget,
    ) -> Result<(LocalTable, usize), Failure> {
        if self.columns.is_empty() {
            return Err(Failure::user(format!(
                "Table '{}.{}' has no columns, so nothing can be read from it",
                self.relation.database, self.relation.table
            )));
        }
        let (rows, scanned) = self.rows(objects, budget)?;
        Ok((
            LocalTable {
                name: self.local_name.clone(),
                columns: self
                    .columns
                    .iter()
                    .map(|column| LocalColumn {
                        name: column.name.clone(),
                        declared: engine_type(&column.data_type),
                    })
                    .collect(),
                rows,
            },
            scanned,
        ))
    }

    /// The rows the objects at this table's location decode to, and the bytes read for
    /// them.
    fn rows(
        &self,
        objects: &s3_state::ComAmazonawsS3Amazons3State,
        budget: &mut Budget,
    ) -> Result<(Vec<Vec<::rusqlite::types::Value>>, usize), Failure> {
        let mut rows: Vec<Vec<::rusqlite::types::Value>> = Vec::new();
        let mut scanned = 0usize;
        // A table whose catalog entry names no location has nothing under it to read.
        let Some(location) = self.location.as_deref().filter(|at| !at.is_empty()) else {
            return Ok((rows, scanned));
        };
        let Some((bucket_name, prefix)) = parse_s3_uri(location) else {
            return Err(Failure::user(format!(
                "Unable to read the location {location} of {}.{}: it is not an Amazon S3 \
                 location",
                self.relation.database, self.relation.table
            )));
        };
        let Some(bucket) = objects.bucket(&bucket_name) else {
            return Err(Failure::user(format!(
                "Permission denied on S3 path: s3://{bucket_name}/{prefix}"
            )));
        };
        for (key, object) in bucket.objects() {
            if !under_prefix(key, &prefix) || is_hidden(key) {
                continue;
            }
            budget.charge_deadline()?;
            if object.body.len() > budget.bytes_left {
                return Err(Failure::emulator(format!(
                    "reading {}.{} would scan more than the {} bytes this emulator reads \
                     for one query",
                    self.relation.database,
                    self.relation.table,
                    query_engine::MAX_MATERIALISED_BYTES
                )));
            }
            budget.bytes_left -= object.body.len();
            scanned = scanned.saturating_add(object.body.len());
            self.decode(&object.body, &mut rows, budget)?;
        }
        Ok((rows, scanned))
    }

    /// Decode one object's bytes into the rows it holds, appending them to `rows`.
    ///
    /// STREAMING, and budget-charged BEFORE each row is built: one record is read, charged
    /// and converted at a time, so a read stops at the first row over budget instead of
    /// holding a whole object's decoded records in memory first.
    fn decode(
        &self,
        body: &[u8],
        rows: &mut Vec<Vec<::rusqlite::types::Value>>,
        budget: &mut Budget,
    ) -> Result<(), Failure> {
        let width = self.columns.len();
        match &self.decoder {
            Decoder::Delimited { separator, quote } => {
                let mut reader = delimited_reader(body, *separator, *quote).map_err(|reason| {
                    Failure::user(format!(
                        "HIVE_BAD_DATA: Error reading the data of {}.{}: {reason}",
                        self.relation.database, self.relation.table
                    ))
                })?;
                let mut record = ::csv::StringRecord::new();
                let mut skipped = 0usize;
                loop {
                    let read = reader.read_record(&mut record).map_err(|reason| {
                        Failure::user(format!(
                            "HIVE_BAD_DATA: Error reading the data of {}.{}: {reason}",
                            self.relation.database, self.relation.table
                        ))
                    })?;
                    if !read {
                        break;
                    }
                    if skipped < self.skip_header_lines {
                        skipped += 1;
                        continue;
                    }
                    budget.charge_row(&self.relation, width)?;
                    rows.push(self.row(|position| record.get(position)));
                }
            }
            Decoder::Json => {
                let text = String::from_utf8_lossy(body);
                let mut skipped = 0usize;
                for line in text
                    .split('\n')
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                {
                    if skipped < self.skip_header_lines {
                        skipped += 1;
                        continue;
                    }
                    budget.charge_row(&self.relation, width)?;
                    let document: Option<::serde_json::Value> =
                        ::serde_json::from_str(line).ok();
                    let members = document.as_ref().and_then(::serde_json::Value::as_object);
                    // The members a JSON SerDe reads are the table's columns BY NAME, so
                    // each column's value is looked up rather than taken by position, and
                    // only the values this table's columns name are ever built.
                    let fields: Vec<Option<String>> = self
                        .columns
                        .iter()
                        .map(|column| {
                            members.and_then(|members| json_member(members, &column.name))
                        })
                        .collect();
                    rows.push(self.row(|position| {
                        fields.get(position).and_then(Option::as_deref)
                    }));
                }
            }
        }
        Ok(())
    }

    /// One record as the row it holds: one value per column of the table, in the table's
    /// own column order, each read from the record by its column's position.
    fn row<'f>(
        &self,
        field: impl Fn(usize) -> Option<&'f str>,
    ) -> Vec<::rusqlite::types::Value> {
        self.columns
            .iter()
            .enumerate()
            .map(|(position, column)| self.cell(column, field(position)))
            .collect()
    }

    /// One field as the value its column holds it as.
    ///
    /// A field the object does not carry, or one carrying the SerDe's NULL text, is NULL;
    /// so is a field whose text is not a value of the column's own type, which is what
    /// Hive reads a malformed value as rather than failing the whole read.
    fn cell(&self, column: &DeclaredColumn, field: Option<&str>) -> ::rusqlite::types::Value {
        use ::rusqlite::types::Value;
        let Some(field) = field else {
            return Value::Null;
        };
        if field == self.null_text {
            return Value::Null;
        }
        match engine_type(&column.data_type) {
            TYPE_TEXT if is_timestamp(&column.data_type) => {
                Value::Text(timestamp_text(field))
            }
            TYPE_TEXT => Value::Text(field.to_string()),
            TYPE_BLOB => Value::Blob(field.as_bytes().to_vec()),
            TYPE_BOOLEAN => match field.trim() {
                text if text.eq_ignore_ascii_case("true") => Value::Integer(1),
                text if text.eq_ignore_ascii_case("false") => Value::Integer(0),
                _ => Value::Null,
            },
            TYPE_DOUBLE => field
                .trim()
                .parse::<f64>()
                .map_or(Value::Null, Value::Real),
            _ => field
                .trim()
                .parse::<i64>()
                .map_or(Value::Null, Value::Integer),
        }
    }
}

/// Whether a column of Hive type `data_type` holds a TIMESTAMP, whose values are reported
/// to the precision that type carries (see [`timestamp_text`]).
fn is_timestamp(data_type: &str) -> bool {
    named_type(data_type) == "timestamp"
}

/// How many digits of a second a timestamp carries: Athena's `timestamp` is a value to
/// the millisecond, and a read of such a column reports every one of those digits — so a
/// stored value that writes fewer is reported with the rest of them, and one that writes
/// more is reported to that precision.
const TIMESTAMP_FRACTION_DIGITS: usize = 3;

/// One timestamp field as the value its column holds it as: the instant the field names,
/// written to the precision the type carries. Text that names no instant at all is held as
/// it stands, because nothing else can honestly be said about it.
fn timestamp_text(field: &str) -> String {
    let trimmed = field.trim();
    let (instant, fraction) = match trimmed.split_once('.') {
        Some((instant, fraction)) if fraction.chars().all(|digit| digit.is_ascii_digit()) => {
            (instant, fraction)
        }
        Some(_) => return field.to_string(),
        None => (trimmed, ""),
    };
    if !names_an_instant(instant) {
        return field.to_string();
    }
    let mut digits: String = fraction.chars().take(TIMESTAMP_FRACTION_DIGITS).collect();
    while digits.len() < TIMESTAMP_FRACTION_DIGITS {
        digits.push('0');
    }
    format!("{instant}.{digits}")
}

/// Whether `text` names an instant as a timestamp writes one: a date, a separator and a
/// time of day.
fn names_an_instant(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() != "0000-00-00 00:00:00".len() {
        return false;
    }
    text.char_indices().all(|(position, character)| {
        match position {
            4 | 7 => character == '-',
            10 => character == ' ' || character == 'T',
            13 | 16 => character == ':',
            _ => character.is_ascii_digit(),
        }
    })
}

/// The declared type a column of a 32-bit integer type is held as.
const TYPE_INTEGER: &str = "integer";
/// The declared type a column of a 64-bit integer type is held as.
const TYPE_BIGINT: &str = "bigint";
/// The declared type an approximate-numeric column is held as.
const TYPE_DOUBLE: &str = "double";
/// The declared type a boolean column is held as.
const TYPE_BOOLEAN: &str = "boolean";
/// The declared type a character column is held as.
const TYPE_TEXT: &str = "varchar";
/// The declared type a binary column is held as.
const TYPE_BLOB: &str = "blob";

/// The type the engine holds a column of Hive type `data_type` as.
///
/// The catalog records a column's type in Hive's own type names, and each one maps to the
/// type this engine computes over and reports: the exact-numeric types to an integer of
/// the matching width, the approximate-numeric and fixed-point types to a double, Hive's
/// boolean to a boolean, its binary type to binary data, and everything else — the
/// character, date, time and complex types — to character data, which is the text the
/// object holds it as.
pub fn engine_type(data_type: &str) -> &'static str {
    match named_type(data_type).as_str() {
        "boolean" => TYPE_BOOLEAN,
        "tinyint" | "smallint" | "int" | "integer" => TYPE_INTEGER,
        "bigint" => TYPE_BIGINT,
        "float" | "real" | "double" | "double precision" | "decimal" | "numeric" => TYPE_DOUBLE,
        "binary" | "varbinary" => TYPE_BLOB,
        _ => TYPE_TEXT,
    }
}

/// The TYPE one Hive type name names, without the precision, length or element type that
/// qualifies it: `varchar(20)` is a character type and `array<string>` an array.
fn named_type(data_type: &str) -> String {
    let data_type = data_type.trim().to_ascii_lowercase();
    data_type
        .split(['(', '<'])
        .next()
        .unwrap_or(&data_type)
        .trim()
        .to_string()
}

/// The value the table property `name` carries, whatever case the catalog recorded the
/// property's NAME in, and `None` when the property is absent or carries no value at all.
///
/// A property a client wrote through the catalog API carries whatever name that client
/// spelled, so a reader that matched the name byte for byte would miss the same property
/// under another spelling of its case.
fn property<'held>(
    parameters: &'held HashMap<String, String>,
    name: &str,
) -> Option<&'held str> {
    parameters.iter().find_map(|(property, value)| {
        let value = value.trim();
        (property.trim().eq_ignore_ascii_case(name) && !value.is_empty()).then_some(value)
    })
}

/// Whether the catalog marks this entry as a table of the ICEBERG open table format.
///
/// Either half of the entry says so: the `table_type` property an engine that writes such
/// a table records the format in, or a storage descriptor carrying Iceberg's own
/// serialization library or input format.
fn is_iceberg(
    parameters: &HashMap<String, String>,
    library: &str,
    input_format: Option<&str>,
) -> bool {
    property(parameters, TABLE_TYPE_PROPERTY)
        .is_some_and(|table_type| table_type.eq_ignore_ascii_case(ICEBERG_TABLE_TYPE))
        || library.trim().eq_ignore_ascii_case(ICEBERG_SERDE)
        || input_format
            .is_some_and(|format| format.trim().eq_ignore_ascii_case(ICEBERG_INPUT_FORMAT))
}

/// How a table stored with the SerDe `library` under the input format `input_format` is
/// decoded, or `None` for a storage format this emulator's engine does not read.
///
/// BOTH halves of what the catalog records decide it: the serialization library says how a
/// record's fields are written, and the input format says how the objects themselves are.
/// A table whose input format is not the one delimited text is read with holds data in a
/// format this engine cannot decode, whatever its SerDe says — so it is reported rather
/// than decoded as text.
fn decoder_of(
    library: &str,
    input_format: Option<&str>,
    parameters: &HashMap<String, String>,
) -> Option<Decoder> {
    let text_objects = input_format
        .map(str::trim)
        .filter(|format| !format.is_empty())
        .is_none_or(|format| format.eq_ignore_ascii_case(ddl::TEXT_INPUT_FORMAT));
    if !text_objects {
        return None;
    }
    if JSON_SERDES
        .iter()
        .any(|serde| library.eq_ignore_ascii_case(serde))
    {
        return Some(Decoder::Json);
    }
    if library.eq_ignore_ascii_case(OPEN_CSV_SERDE) {
        return Some(Decoder::Delimited {
            separator: single_char(parameters.get(CSV_SEPARATOR_PARAMETER)).unwrap_or(','),
            quote: single_char(parameters.get(CSV_QUOTE_PARAMETER)).or(Some('"')),
        });
    }
    if library.eq_ignore_ascii_case(ddl::LAZY_SIMPLE_SERDE) {
        let separator = FIELD_DELIMITER_PARAMETERS
            .iter()
            .find_map(|parameter| separator_char(parameter, parameters.get(*parameter)))
            .unwrap_or(DEFAULT_FIELD_DELIMITER);
        return Some(Decoder::Delimited {
            separator,
            quote: None,
        });
    }
    None
}

/// The character a SerDe parameter names as the field separator.
///
/// [`CODED_PARAMETER`] carries a character CODE — Hive records the default separator as
/// the code of the unit separator — so a value that is a number names the character with
/// that code; every other value is the character it carries.
fn separator_char(parameter: &str, value: Option<&String>) -> Option<char> {
    if parameter == CODED_PARAMETER {
        if let Some(code) = value
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|code| *code <= MAX_CHARACTER_CODE)
        {
            return char::from_u32(code);
        }
    }
    single_char(value)
}

/// The one character a SerDe parameter names, or `None` when it names none.
fn single_char(parameter: Option<&String>) -> Option<char> {
    let mut characters = parameter?.chars();
    let first = characters.next()?;
    characters.next().is_none().then_some(first)
}

/// The bucket and key prefix an `s3://` location names, or `None` for a location that
/// names no object store path.
pub fn parse_s3_uri(location: &str) -> Option<(String, String)> {
    let rest = location.strip_prefix(S3_SCHEME)?;
    let (bucket, prefix) = rest.split_once('/').unwrap_or((rest, ""));
    if bucket.is_empty() {
        return None;
    }
    Some((
        bucket.to_string(),
        prefix.trim_end_matches('/').to_string(),
    ))
}

/// Whether `key` names an object AT a table's location: the prefix itself, or anything
/// under it, including the objects of a nested prefix.
fn under_prefix(key: &str, prefix: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    key == prefix || key.starts_with(&format!("{prefix}/"))
}

/// Whether `key` names an object a read of a table's location SKIPS: the prefix markers a
/// console write leaves behind, and the files Hive treats as hidden — the ones whose name
/// starts with `_` or `.`.
fn is_hidden(key: &str) -> bool {
    if key.ends_with('/') {
        return true;
    }
    let name = key.rsplit('/').next().unwrap_or(key);
    name.is_empty() || name.starts_with('_') || name.starts_with('.')
}

/// The reader one object of delimited text is read RECORD BY RECORD with, or the reason
/// its SerDe names a separator this reader cannot read.
///
/// The format is read by the maintained reader for delimited text, configured from the
/// table's own SerDe: the separator it names, whether its fields may be quoted, and
/// records of varying width — a Hive table's object may hold a line with fewer fields
/// than the table has columns, and the missing ones are simply absent.
fn delimited_reader(
    body: &[u8],
    separator: char,
    quote: Option<char>,
) -> Result<::csv::Reader<&[u8]>, String> {
    let separator = ascii_byte(separator).ok_or_else(|| {
        format!(
            "the field separator {separator:?} is not one character of the text this \
             reader reads"
        )
    })?;
    let mut builder = ::csv::ReaderBuilder::new();
    builder
        .has_headers(false)
        .flexible(true)
        .delimiter(separator)
        .quoting(quote.is_some());
    if let Some(quote) = quote.and_then(ascii_byte) {
        builder.quote(quote);
    }
    Ok(builder.from_reader(body))
}

/// The one byte `character` is written as, or `None` for a character that is not one byte
/// of text.
fn ascii_byte(character: char) -> Option<u8> {
    character.is_ascii().then_some(character as u8)
}

/// The member `name` of a JSON document, as the text its value denotes.
///
/// A JSON SerDe reads a document's members as the table's columns by name, folded the way
/// the catalog folds a column name, so a document whose member is spelled in another case
/// still fills its column.
fn json_member(
    members: &::serde_json::Map<String, ::serde_json::Value>,
    name: &str,
) -> Option<String> {
    let value = members.get(name).or_else(|| {
        members
            .iter()
            .find(|(member, _)| member.eq_ignore_ascii_case(name))
            .map(|(_, value)| value)
    })?;
    match value {
        ::serde_json::Value::Null => None,
        ::serde_json::Value::String(text) => Some(text.clone()),
        other => Some(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of the decoding table: the bytes one object holds, the SerDe that reads
    /// them, and the records they decode to.
    struct DecodeCase {
        /// What this row is about.
        case: &'static str,
        /// The bytes one object at the table's location holds.
        body: &'static str,
        /// What separates two fields.
        separator: char,
        /// What a field may be wrapped in.
        quote: Option<char>,
        /// The records the object holds, each as its fields.
        records: Vec<Vec<&'static str>>,
    }

    /// An object of delimited text decodes to the records its SerDe says it holds: the
    /// rows differ along the separator, whether fields are quoted, how the lines are
    /// terminated, how many records the object carries and whether a field is empty or a
    /// record short.
    #[test]
    fn an_object_decodes_to_the_records_its_serde_declares() {
        let cases: Vec<DecodeCase> = vec![
            DecodeCase {
                case: "the default Hive field separator, over several records",
                body: "1\u{1}desk\u{1}249.5\n2\u{1}lamp\u{1}19.0\n",
                separator: DEFAULT_FIELD_DELIMITER,
                quote: None,
                records: vec![vec!["1", "desk", "249.5"], vec!["2", "lamp", "19.0"]],
            },
            DecodeCase {
                case: "a comma separator",
                body: "1,desk,249.5\n",
                separator: ',',
                quote: None,
                records: vec![vec!["1", "desk", "249.5"]],
            },
            DecodeCase {
                case: "a tab separator and no final line terminator",
                body: "1\tdesk",
                separator: '\t',
                quote: None,
                records: vec![vec!["1", "desk"]],
            },
            DecodeCase {
                case: "an empty field between separators",
                body: "1,,3\n",
                separator: ',',
                quote: None,
                records: vec![vec!["1", "", "3"]],
            },
            DecodeCase {
                case: "records terminated the way a Windows writer terminates them",
                body: "1,desk\r\n2,lamp\r\n",
                separator: ',',
                quote: None,
                records: vec![vec!["1", "desk"], vec!["2", "lamp"]],
            },
            DecodeCase {
                case: "an empty line between records",
                body: "1,desk\n\n2,lamp\n",
                separator: ',',
                quote: None,
                records: vec![vec!["1", "desk"], vec!["2", "lamp"]],
            },
            DecodeCase {
                case: "a record shorter than the one before it",
                body: "1,desk,249.5\n2,lamp\n",
                separator: ',',
                quote: None,
                records: vec![vec!["1", "desk", "249.5"], vec!["2", "lamp"]],
            },
            DecodeCase {
                case: "a quoted field carrying the separator",
                body: "1,\"desk, oak\",3\n",
                separator: ',',
                quote: Some('"'),
                records: vec![vec!["1", "desk, oak", "3"]],
            },
            DecodeCase {
                case: "a doubled quote inside a quoted field",
                body: "\"a \"\"b\"\" c\",2\n",
                separator: ',',
                quote: Some('"'),
                records: vec![vec!["a \"b\" c", "2"]],
            },
            DecodeCase {
                case: "a quote character where fields are not quoted at all",
                body: "1,\"desk\n",
                separator: ',',
                quote: None,
                records: vec![vec!["1", "\"desk"]],
            },
            DecodeCase {
                case: "an object holding nothing",
                body: "",
                separator: ',',
                quote: None,
                records: vec![],
            },
        ];

        for case in cases {
            let mut reader =
                delimited_reader(case.body.as_bytes(), case.separator, case.quote)
                    .unwrap_or_else(|reason| {
                        panic!("case {:?}: the object must be readable, got {reason:?}", case.case)
                    });
            let actual: Vec<Vec<String>> = reader
                .records()
                .map(|record| {
                    let record = record.unwrap_or_else(|reason| {
                        panic!("case {:?}: the object must decode, got {reason:?}", case.case)
                    });
                    record.iter().map(ToString::to_string).collect()
                })
                .collect();
            assert_eq!(
                actual, case.records,
                "case {:?}: expected the records {:?}, got {actual:?}",
                case.case, case.records
            );
        }
    }

    /// A column's Hive type decides the type the engine holds it as, so a type the
    /// catalog records maps to the one Athena reports for that column.
    #[test]
    fn a_hive_column_type_maps_to_the_type_the_engine_holds_it_as() {
        // (the Hive type the catalog holds, the engine type it is read as)
        let cases: Vec<(&str, &str)> = vec![
            ("bigint", TYPE_BIGINT),
            ("int", TYPE_INTEGER),
            ("integer", TYPE_INTEGER),
            ("smallint", TYPE_INTEGER),
            ("tinyint", TYPE_INTEGER),
            ("double", TYPE_DOUBLE),
            ("float", TYPE_DOUBLE),
            ("decimal(10,2)", TYPE_DOUBLE),
            ("boolean", TYPE_BOOLEAN),
            ("string", TYPE_TEXT),
            ("varchar(20)", TYPE_TEXT),
            ("char(3)", TYPE_TEXT),
            ("date", TYPE_TEXT),
            ("timestamp", TYPE_TEXT),
            ("array<string>", TYPE_TEXT),
            ("binary", TYPE_BLOB),
            ("BIGINT", TYPE_BIGINT),
        ];

        for (declared, expected) in cases {
            let actual = engine_type(declared);
            assert_eq!(
                actual, expected,
                "case {declared:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// An `s3://` location names a bucket and a prefix, and the prefix a table's data
    /// lives under never carries the separator that closed it.
    #[test]
    fn a_location_parses_into_a_bucket_and_a_prefix() {
        // (case, the location, the bucket it names, the prefix it names)
        let cases: Vec<(&str, &str, Option<&str>, &str)> = vec![
            (
                "a prefix with a trailing separator",
                "s3://bucket/data/",
                Some("bucket"),
                "data",
            ),
            (
                "a prefix with none",
                "s3://bucket/data",
                Some("bucket"),
                "data",
            ),
            (
                "a nested prefix",
                "s3://bucket/a/b/c/",
                Some("bucket"),
                "a/b/c",
            ),
            ("the bucket root", "s3://bucket", Some("bucket"), ""),
            (
                "the bucket root with a separator",
                "s3://bucket/",
                Some("bucket"),
                "",
            ),
            ("no object store at all", "hdfs://cluster/data", None, ""),
            ("no bucket", "s3:///data", None, ""),
        ];

        for (case, location, bucket, prefix) in cases {
            let actual = parse_s3_uri(location);
            let expected =
                bucket.map(|bucket| (bucket.to_string(), prefix.to_string()));
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// A timestamp field is held as the instant it names, written to the precision the
    /// type carries: the rows differ along how many digits of a second the stored text
    /// writes, and one row is text that names no instant at all.
    #[test]
    fn a_timestamp_field_is_held_to_the_precision_its_type_carries() {
        // (the stored text, the value its column holds)
        let cases: Vec<(&str, &str)> = vec![
            ("2024-01-02 03:04:05", "2024-01-02 03:04:05.000"),
            ("2024-01-02 03:04:05.1", "2024-01-02 03:04:05.100"),
            ("2024-01-02 03:04:05.12", "2024-01-02 03:04:05.120"),
            ("2024-01-02 03:04:05.123", "2024-01-02 03:04:05.123"),
            ("2024-01-02 03:04:05.123456", "2024-01-02 03:04:05.123"),
            ("2024-01-02T03:04:05", "2024-01-02T03:04:05.000"),
            ("  2024-01-02 03:04:05  ", "2024-01-02 03:04:05.000"),
            ("not an instant", "not an instant"),
            ("2024-01-02", "2024-01-02"),
        ];

        for (stored, expected) in cases {
            let actual = timestamp_text(stored);
            assert_eq!(
                actual, expected,
                "case {stored:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// The field separator a SerDe parameter names: the serialization format carries a
    /// character CODE, and every other parameter the character itself.
    #[test]
    fn a_serde_parameter_names_the_separator_it_carries() {
        // (case, the parameter, its value, the separator it names)
        let cases: Vec<(&str, &str, &str, Option<char>)> = vec![
            (
                "the default separator, as its code",
                CODED_PARAMETER,
                "1",
                Some(DEFAULT_FIELD_DELIMITER),
            ),
            ("a comma as a code", CODED_PARAMETER, "44", Some(',')),
            ("a comma as itself", CODED_PARAMETER, ",", Some(',')),
            ("a field delimiter", "field.delim", ",", Some(',')),
            ("a field delimiter written as a digit", "field.delim", "1", Some('1')),
            ("more than one character", "field.delim", "::", None),
            ("nothing at all", "field.delim", "", None),
        ];

        for (case, parameter, value, expected) in cases {
            let actual = separator_char(parameter, Some(&value.to_string()));
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// What a read of a table's location skips: the prefix markers and the files Hive
    /// hides, and nothing else.
    #[test]
    fn a_read_skips_the_objects_hive_hides() {
        // (the key, whether a read skips it)
        let cases: Vec<(&str, bool)> = vec![
            ("data/rows.txt", false),
            ("data/nested/rows.txt", false),
            ("data/", true),
            ("data/_SUCCESS", true),
            ("data/.hidden", true),
            ("data/_$folder$", true),
        ];

        for (key, expected) in cases {
            let actual = is_hidden(key);
            assert_eq!(
                actual, expected,
                "case {key:?}: expected hidden={expected}, got {actual}"
            );
        }
    }
}
