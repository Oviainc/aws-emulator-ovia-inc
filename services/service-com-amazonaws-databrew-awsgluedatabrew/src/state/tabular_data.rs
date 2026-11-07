// SPDX-License-Identifier: CC0-1.0

//! The TABLE a dataset's object holds and a job's output is written from.
//!
//! A DataBrew dataset is tabular — "a dataset is a collection of rows and columns" — so
//! every recipe step reads and writes one value: named columns, and rows of cells under
//! them. This module is that value plus the decoding and encoding of the formats this
//! emulator reads and writes, and nothing about any particular transformation
//! ([`super::recipe_steps`] owns those).
//!
//! NOTHING here parses a format by hand: the comma-separated reading and writing is the
//! `csv` crate's, and the JSON reading and writing is `serde_json`'s. Both are maintained
//! crates that already answer a malformed document with an error rather than a panic,
//! which is what a dataset's own bytes — caller-supplied, through the object store —
//! need.
//!
//! Every growth site is bounded BEFORE it allocates, because the size of all of them is
//! the caller's to choose: how many bytes are read from one object
//! ([`MAX_INPUT_BYTES`]), how many rows and columns a table holds ([`MAX_ROWS`],
//! [`MAX_COLUMNS`]) and how many bytes one output part may be encoded to
//! ([`MAX_OUTPUT_BYTES`]). Each is CHOSEN for this emulator (AWS publishes no such
//! limit) and stated as the reason a run fails rather than as a truncation, so a run
//! never reports success over a part of the data.

use std::collections::BTreeMap;

/// One cell of a table. `None` is a NULL cell — a cell holding no value, which is a
/// different thing from a cell holding the empty string: a type coercion that cannot
/// read a value leaves NULL behind ([`super::recipe_steps`]), and a CSV writer renders
/// it as an empty field while a JSON writer renders it as `null`.
pub type Cell = Option<String>;

/// The most bytes this emulator reads from one dataset object.
pub const MAX_INPUT_BYTES: usize = 8 * 1024 * 1024;

/// The most rows one table holds.
pub const MAX_ROWS: usize = 100_000;

/// The most columns one table holds (model: `Output$PartitionColumns` is capped at 200
/// column names; a table this emulator reads is allowed well past that and still
/// bounded).
pub const MAX_COLUMNS: usize = 1_000;

/// The most CELLS one table holds — the bound on the PRODUCT of the two dimensions above,
/// not on either alone.
///
/// Every row is padded to one cell per column ([`squared`]), so a document that declares
/// many columns in one record and then many short records would otherwise materialise
/// rows times columns cells out of a small object. The product is therefore capped on its
/// own, and the cap is applied before the padding allocates.
pub const MAX_CELLS: usize = 1_000_000;

/// The most bytes one cell holds. A transformation that JOINS cells can produce a value
/// longer than any it read ([`super::recipe_steps`]), so the length of what a step
/// produces is capped as well as the count.
pub const MAX_CELL_BYTES: usize = 64 * 1024;

/// The most bytes one table holds across every cell and column name: the bound on what a
/// recipe's steps may GROW the table to, whatever the object it was read from cost.
pub const MAX_TABLE_BYTES: usize = 16 * 1024 * 1024;

/// The most bytes this emulator encodes one output part file to.
pub const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;

/// The most bytes one run writes across every part of every output: the bound on the
/// PRODUCT of the part count and the part size, which neither alone caps.
pub const MAX_RUN_OUTPUT_BYTES: usize = 64 * 1024 * 1024;

/// The delimiter a comma-separated dataset is read with when its `FormatOptions` name
/// none (model: `CsvOptions$Delimiter` — "a single character that specifies the delimiter
/// being used in the CSV file"; CSV is comma-separated by name).
pub const DEFAULT_DELIMITER: u8 = b',';

/// A table: the column names, and the rows of cells under them.
///
/// Every row holds exactly one cell per column, which is what lets a step address a cell
/// by its column's position; a shorter row read from a ragged document is padded with
/// NULL cells and a longer one is an error ([`read_csv`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Cell>>,
}

impl Table {
    /// The position of the column named `name`, or `None` when the table holds no such
    /// column. Column names are compared exactly: a recipe step names the column as the
    /// dataset holds it.
    #[must_use]
    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.columns.iter().position(|held| held == name)
    }

    /// The cell of `row` under the column at `index`, as the value a condition or a
    /// transformation reads. `None` for a NULL cell and for a row that does not reach
    /// that column.
    #[must_use]
    pub fn cell_at(row: &[Cell], index: usize) -> Option<&str> {
        row.get(index).and_then(|cell| cell.as_deref())
    }

    /// How many bytes this table holds across its column names and every cell: what a
    /// step that GROWS the table is checked against ([`MAX_TABLE_BYTES`]).
    #[must_use]
    pub fn byte_size(&self) -> usize {
        let names: usize = self.columns.iter().map(String::len).sum();
        let cells: usize = self
            .rows
            .iter()
            .flat_map(|row| row.iter())
            .map(|cell| cell.as_ref().map_or(0, String::len))
            .sum();
        names.saturating_add(cells)
    }

    /// This table's rows split into `parts` contiguous groups, largest first, so each
    /// group becomes one output part file.
    ///
    /// `parts` is never zero and never more than there are rows, so no part is empty: a
    /// run that asks for more files than it has rows writes one file per row. A table
    /// with no rows at all still answers ONE group — the header is the output.
    #[must_use]
    pub fn parts(&self, parts: usize) -> Vec<&[Vec<Cell>]> {
        let groups = parts.max(1).min(self.rows.len().max(1));
        let size = self.rows.len().div_ceil(groups).max(1);
        if self.rows.is_empty() {
            return vec![&self.rows[..]];
        }
        self.rows.chunks(size).collect()
    }
}

/// The table a comma-separated document holds, read with `delimiter` and with its first
/// record taken as the header when `header_row` says so.
///
/// "A variable that specifies whether the first row in the file is parsed as the header.
/// If this value is false, column names are auto-generated" (model:
/// `CsvOptions$HeaderRow`), and the generated names are `Column_1`, `Column_2`, … in
/// column order.
pub fn read_csv(bytes: &[u8], delimiter: u8, header_row: bool) -> Result<Table, String> {
    let mut reader = ::csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(header_row)
        // A dataset is not always rectangular, and a ragged record is the caller's
        // document rather than a parse failure: the rows are squared up below.
        .flexible(true)
        .from_reader(bytes);

    let mut columns: Vec<String> = if header_row {
        reader
            .headers()
            .map_err(|error| format!("The dataset's CSV header could not be read: {error}"))?
            .iter()
            .map(ToString::to_string)
            .collect()
    } else {
        Vec::new()
    };
    if columns.len() > MAX_COLUMNS {
        return Err(too_many_columns(columns.len()));
    }

    let mut rows: Vec<Vec<Cell>> = Vec::new();
    for record in reader.records() {
        let record =
            record.map_err(|error| format!("The dataset's CSV rows could not be read: {error}"))?;
        if rows.len() == MAX_ROWS {
            return Err(too_many_rows());
        }
        if record.len() > MAX_COLUMNS {
            return Err(too_many_columns(record.len()));
        }
        // A record wider than the header widens the table, under generated names, so no
        // cell of the caller's document is dropped on the floor.
        while columns.len() < record.len() {
            columns.push(generated_column_name(columns.len()));
        }
        // Every row is padded to one cell per column, so the table costs rows times
        // columns cells however narrow the records are: the PRODUCT is checked here,
        // before the next row is held and before the padding allocates.
        if cells_exceeded(rows.len() + 1, columns.len()) {
            return Err(too_many_cells());
        }
        rows.push(record.iter().map(|cell| Some(cell.to_string())).collect());
    }

    squared(columns, rows)
}

/// The table a JSON document holds: an array of objects, or one object per line, as the
/// two shapes `JsonOptions$MultiLine` distinguishes (model: "a value that specifies
/// whether JSON input contains embedded new line characters").
///
/// The columns are the member names in the order they are first seen, so a row that
/// carries a member another row does not still has a column, holding NULL where it is
/// absent. A member whose value is not a string is rendered as the JSON it is, which is
/// what a tabular reading of a nested value is.
pub fn read_json(bytes: &[u8]) -> Result<Table, String> {
    let text = ::std::str::from_utf8(bytes)
        .map_err(|error| format!("The dataset's JSON is not valid UTF-8: {error}"))?;
    let mut records: Vec<::serde_json::Value> = Vec::new();
    match ::serde_json::from_str::<::serde_json::Value>(text) {
        // One document: an array of records, or a single record.
        Ok(::serde_json::Value::Array(items)) => records.extend(items),
        Ok(value) => records.push(value),
        // Not one document, so the records are one per line.
        Err(_) => {
            for line in text.lines().filter(|line| !line.trim().is_empty()) {
                records.push(::serde_json::from_str(line).map_err(|error| {
                    format!("The dataset's JSON records could not be read: {error}")
                })?);
            }
        }
    }
    if records.len() > MAX_ROWS {
        return Err(too_many_rows());
    }

    let mut columns: Vec<String> = Vec::new();
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    for record in &records {
        let ::serde_json::Value::Object(members) = record else {
            return Err(
                "The dataset's JSON holds a record that is not an object, which is not a row of \
                 columns."
                    .to_string(),
            );
        };
        let mut row: BTreeMap<String, Cell> = BTreeMap::new();
        for (name, value) in members {
            if !columns.iter().any(|held| held == name) {
                if columns.len() == MAX_COLUMNS {
                    return Err(too_many_columns(columns.len() + 1));
                }
                columns.push(name.clone());
            }
            row.insert(name.clone(), json_cell(value));
        }
        // The product, for the reason [`read_csv`] checks it: every record becomes a row
        // as wide as the columns seen so far.
        if cells_exceeded(rows.len() + 1, columns.len()) {
            return Err(too_many_cells());
        }
        rows.push(
            columns
                .iter()
                .map(|name| row.get(name).cloned().unwrap_or(None))
                .collect(),
        );
    }

    squared(columns, rows)
}

/// `table` as a comma-separated document written with `delimiter`: the header record,
/// then `rows`. A NULL cell is written as an empty field, which is how a comma-separated
/// document carries one.
pub fn write_csv(
    table: &Table,
    rows: &[Vec<Cell>],
    delimiter: u8,
) -> Result<Vec<u8>, String> {
    let mut writer = ::csv::WriterBuilder::new()
        .delimiter(delimiter)
        .from_writer(Vec::new());
    writer
        .write_record(&table.columns)
        .map_err(|error| format!("The output's CSV header could not be written: {error}"))?;
    for row in rows {
        writer
            .write_record(row.iter().map(|cell| cell.as_deref().unwrap_or_default()))
            .map_err(|error| format!("The output's CSV rows could not be written: {error}"))?;
        // The encoded size is checked as it grows rather than after the whole document
        // is built, so the bound caps what is allocated.
        if writer.get_ref().len() > MAX_OUTPUT_BYTES {
            return Err(output_too_large());
        }
    }
    let bytes = writer
        .into_inner()
        .map_err(|error| format!("The output's CSV could not be completed: {error}"))?;
    if bytes.len() > MAX_OUTPUT_BYTES {
        return Err(output_too_large());
    }
    Ok(bytes)
}

/// `table` as a JSON document: one object per line, each carrying every column of the
/// table IN COLUMN ORDER, with a NULL cell as `null`.
pub fn write_json(table: &Table, rows: &[Vec<Cell>]) -> Result<Vec<u8>, String> {
    let mut bytes: Vec<u8> = Vec::new();
    for row in rows {
        let line = ::serde_json::to_vec(&JsonRecord {
            columns: &table.columns,
            row,
        })
        .map_err(|error| format!("The output's JSON could not be written: {error}"))?;
        if bytes.len() + line.len() + 1 > MAX_OUTPUT_BYTES {
            return Err(output_too_large());
        }
        bytes.extend_from_slice(&line);
        bytes.push(b'\n');
    }
    Ok(bytes)
}

/// One row as the JSON object a record is written as: the columns in the table's own
/// order, which a `serde_json` map would not keep (it orders its members by name).
struct JsonRecord<'a> {
    columns: &'a [String],
    row: &'a [Cell],
}

impl ::serde::Serialize for JsonRecord<'_> {
    fn serialize<S: ::serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use ::serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(self.columns.len()))?;
        for (index, column) in self.columns.iter().enumerate() {
            map.serialize_entry(column, &Table::cell_at(self.row, index))?;
        }
        map.end()
    }
}

/// A table whose every row holds exactly one cell per column: a short row is padded with
/// NULL cells, which is what a document that leaves a trailing column off a row carries.
///
/// `Err` when the padded table would hold more than [`MAX_CELLS`] cells or more than
/// [`MAX_TABLE_BYTES`] bytes — checked before the padding allocates, so neither bound can
/// be passed by the allocation it caps.
fn squared(columns: Vec<String>, mut rows: Vec<Vec<Cell>>) -> Result<Table, String> {
    if cells_exceeded(rows.len(), columns.len()) {
        return Err(too_many_cells());
    }
    for row in &mut rows {
        row.resize(columns.len(), None);
    }
    let table = Table { columns, rows };
    if table.byte_size() > MAX_TABLE_BYTES {
        return Err(table_too_large());
    }
    Ok(table)
}

/// Whether a table of `rows` rows and `columns` columns would hold more than
/// [`MAX_CELLS`] cells. The product is computed with a checked multiply, so the check
/// cannot itself overflow.
fn cells_exceeded(rows: usize, columns: usize) -> bool {
    rows.checked_mul(columns)
        .is_none_or(|cells| cells > MAX_CELLS)
}

/// The name a column with no name in the document is read under: its 1-based position.
fn generated_column_name(index: usize) -> String {
    format!("Column_{}", index + 1)
}

/// One JSON member's value as a cell: a string as itself, `null` as a NULL cell, and
/// anything else as the JSON text of that value.
fn json_cell(value: &::serde_json::Value) -> Cell {
    match value {
        ::serde_json::Value::Null => None,
        ::serde_json::Value::String(text) => Some(text.clone()),
        other => Some(other.to_string()),
    }
}

/// The reason a run fails for a document with more rows than [`MAX_ROWS`].
fn too_many_rows() -> String {
    format!(
        "The dataset holds more than the {MAX_ROWS} rows this emulator processes in one job run."
    )
}

/// The reason a run fails for a document with more columns than [`MAX_COLUMNS`].
fn too_many_columns(held: usize) -> String {
    format!(
        "The dataset holds {held} columns, more than the {MAX_COLUMNS} this emulator processes in \
         one job run."
    )
}

/// The reason a run fails for an output part larger than [`MAX_OUTPUT_BYTES`].
fn output_too_large() -> String {
    format!(
        "The job's output is larger than the {MAX_OUTPUT_BYTES} bytes this emulator writes in one \
         output file."
    )
}

/// The reason a run fails for a table holding more than [`MAX_CELLS`] cells.
fn too_many_cells() -> String {
    format!(
        "The dataset holds more than the {MAX_CELLS} cells this emulator processes in one job run."
    )
}

/// The reason a run fails for a table holding more than [`MAX_TABLE_BYTES`] bytes.
pub fn table_too_large() -> String {
    format!(
        "The data holds more than the {MAX_TABLE_BYTES} bytes this emulator processes in one job \
         run."
    )
}

/// The reason a run fails for a cell longer than [`MAX_CELL_BYTES`].
pub fn cell_too_large() -> String {
    format!("a value longer than the {MAX_CELL_BYTES} bytes a cell holds would be produced.")
}
