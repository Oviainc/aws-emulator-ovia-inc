// SPDX-License-Identifier: CC0-1.0

//! One exported table as the Apache Parquet file Amazon RDS writes it as.
//!
//! "The exported data is stored in Apache Parquet format" (Exporting DB snapshot data to
//! Amazon S3, Amazon RDS User Guide), compressed per column, which is why the objects an
//! export writes are named `*.gz.parquet`. So the bytes this module produces are a real
//! Parquet file carrying the rows the source database holds: written by the Apache
//! implementation of the format ([`parquet`]) through its row-group writer, not assembled
//! by hand here, and read back by any Parquet reader.
//!
//! The column types are the SOURCE's. SQLite — the engine each DB instance's databases run
//! on ([`super::data_plane::sql_engine`]) — fixes a column's type affinity from its declared
//! type (`INT` in the name means INTEGER affinity, `CHAR`/`CLOB`/`TEXT` TEXT, `BLOB` or no
//! declared type BLOB, `REAL`/`FLOA`/`DOUB` REAL, everything else NUMERIC: "Determination
//! of column affinity", SQLite datatypes), and that affinity is what a column holding no
//! value at all is exported as. A column that holds values is exported as the narrowest type
//! that carries every one of them exactly, so no exported value is lost: an integer column
//! stays a 64-bit integer whatever its affinity, and a column mixing kinds is exported as
//! strings rather than nulling the rows one type could not hold.

use ::parquet::basic::{Compression, ConvertedType, GzipLevel, Repetition, Type as PhysicalType};
use ::parquet::data_type::{ByteArray, ByteArrayType, DoubleType, Int64Type};
use ::parquet::file::properties::WriterProperties;
use ::parquet::file::writer::SerializedFileWriter;
use ::parquet::schema::types::Type as SchemaType;

use super::data_plane::sql_engine::{Cell, ColumnInfo, ResultSet};

/// The Parquet physical type one exported column is written as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColumnType {
    /// A 64-bit integer column: SQLite's INTEGER affinity.
    Integer,
    /// A double column: SQLite's REAL affinity.
    Double,
    /// A UTF-8 string column: SQLite's TEXT affinity, and what a column carrying values of
    /// more than one type is widened to.
    Text,
    /// An opaque byte column: SQLite's BLOB affinity.
    Blob,
}

impl ColumnType {
    /// The affinity SQLite gives a column whose declared type is `declared` — "Determination
    /// of column affinity": a declared type containing `INT` has INTEGER affinity; one
    /// containing `CHAR`, `CLOB` or `TEXT` has TEXT; one containing `BLOB`, or no declared
    /// type at all, has BLOB; one containing `REAL`, `FLOA` or `DOUB` has REAL; anything
    /// else has NUMERIC affinity, whose values this export writes as doubles.
    ///
    /// The affinity decides the exported type of a column holding NO value at all; a column
    /// that holds values is exported as [`Self::of_values`] reads them.
    fn of_declared(declared: Option<&str>) -> Self {
        let Some(declared) = declared else {
            return Self::Blob;
        };
        let declared = declared.to_ascii_uppercase();
        if declared.contains("INT") {
            return Self::Integer;
        }
        if declared.contains("CHAR") || declared.contains("CLOB") || declared.contains("TEXT") {
            return Self::Text;
        }
        if declared.contains("BLOB") || declared.is_empty() {
            return Self::Blob;
        }
        if declared.contains("REAL") || declared.contains("FLOA") || declared.contains("DOUB") {
            return Self::Double;
        }
        Self::Double
    }

    /// The type a column is exported as once the values it HOLDS are known: the narrowest
    /// one that carries every value exactly, so no exported value is lost.
    ///
    /// Integers are exported as `INT64` whatever the column's affinity — a 64-bit value
    /// beyond a double's exact range would otherwise lose precision — a column mixing
    /// integers and reals as doubles, one holding only byte strings as bytes, and a column
    /// mixing kinds as strings, which carry every value. A column holding nothing but nulls
    /// has no value to read, so it keeps the affinity its declared type gives it
    /// (`declared`).
    fn of_values(declared: Self, values: &[Cell]) -> Self {
        let present: Vec<&Cell> = values
            .iter()
            .filter(|cell| !matches!(cell, Cell::Null))
            .collect();
        if present.is_empty() {
            return declared;
        }
        if present
            .iter()
            .all(|cell| matches!(cell, Cell::Integer(_)))
        {
            return Self::Integer;
        }
        if present
            .iter()
            .all(|cell| matches!(cell, Cell::Integer(_) | Cell::Real(_)))
        {
            return Self::Double;
        }
        if present.iter().all(|cell| matches!(cell, Cell::Blob(_))) {
            return Self::Blob;
        }
        Self::Text
    }

    /// This column's Parquet physical type and the logical (converted) type that says how
    /// its bytes are read.
    fn physical(self) -> (PhysicalType, ConvertedType) {
        match self {
            Self::Integer => (PhysicalType::INT64, ConvertedType::NONE),
            Self::Double => (PhysicalType::DOUBLE, ConvertedType::NONE),
            Self::Text => (PhysicalType::BYTE_ARRAY, ConvertedType::UTF8),
            Self::Blob => (PhysicalType::BYTE_ARRAY, ConvertedType::NONE),
        }
    }
}

/// The bytes of the Parquet file holding `rows`, one row group, every column OPTIONAL
/// because every value of the source engine may be null.
///
/// `Err` carries what the Parquet writer refused, which the export reports as the cause it
/// failed with rather than writing a truncated object.
pub fn table_parquet_bytes(table: &str, rows: &ResultSet) -> Result<Vec<u8>, String> {
    let types: Vec<ColumnType> = rows
        .columns
        .iter()
        .enumerate()
        .map(|(index, column)| {
            let values: Vec<Cell> = rows
                .rows
                .iter()
                .filter_map(|row| row.get(index).cloned())
                .collect();
            ColumnType::of_values(
                ColumnType::of_declared(column.declared_type.as_deref()),
                &values,
            )
        })
        .collect();
    let schema = message_type(table, &rows.columns, &types)?;
    // GZIP, which is the compression the `.gz.parquet` objects of an export carry.
    let properties = WriterProperties::builder()
        .set_compression(Compression::GZIP(GzipLevel::default()))
        .build();
    let mut writer = SerializedFileWriter::new(
        Vec::new(),
        ::std::sync::Arc::new(schema),
        ::std::sync::Arc::new(properties),
    )
    .map_err(|error| error.to_string())?;
    {
        let mut row_group = writer.next_row_group().map_err(|error| error.to_string())?;
        for (index, column_type) in types.iter().enumerate() {
            let mut column = row_group
                .next_column()
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("the Parquet schema of {table} lost a column"))?;
            let values: Vec<Cell> = rows
                .rows
                .iter()
                .map(|row| row.get(index).cloned().unwrap_or(Cell::Null))
                .collect();
            write_column(&mut column, *column_type, &values).map_err(|error| error.to_string())?;
            column.close().map_err(|error| error.to_string())?;
        }
        row_group.close().map_err(|error| error.to_string())?;
    }
    writer
        .into_inner()
        .map_err(|error| error.to_string())
        .map(|buffer| buffer.to_vec())
}

/// The Parquet message type of one exported table: one OPTIONAL leaf per source column.
fn message_type(
    table: &str,
    columns: &[ColumnInfo],
    types: &[ColumnType],
) -> Result<SchemaType, String> {
    let mut fields = Vec::with_capacity(columns.len());
    for (column, column_type) in columns.iter().zip(types) {
        let (physical, converted) = column_type.physical();
        let field = SchemaType::primitive_type_builder(&column.name, physical)
            // Every column is optional: a source value may be null, and a null is carried
            // by the definition levels the write below passes.
            .with_repetition(Repetition::OPTIONAL)
            .with_converted_type(converted)
            .build()
            .map_err(|error| error.to_string())?;
        fields.push(::std::sync::Arc::new(field));
    }
    SchemaType::group_type_builder(table)
        .with_fields(fields)
        .build()
        .map_err(|error| error.to_string())
}

/// Write one column's values, with the definition level of each row: 1 for a value that is
/// present and 0 for a null, which is how an OPTIONAL Parquet leaf carries one.
fn write_column(
    column: &mut ::parquet::file::writer::SerializedColumnWriter<'_>,
    column_type: ColumnType,
    values: &[Cell],
) -> Result<(), ::parquet::errors::ParquetError> {
    let definitions: Vec<i16> = values
        .iter()
        .map(|cell| i16::from(!matches!(cell, Cell::Null)))
        .collect();
    match column_type {
        ColumnType::Integer => {
            let present: Vec<i64> = values
                .iter()
                .filter_map(|cell| match cell {
                    Cell::Integer(value) => Some(*value),
                    _ => None,
                })
                .collect();
            column
                .typed::<Int64Type>()
                .write_batch(&present, Some(&definitions), None)?;
        }
        ColumnType::Double => {
            let present: Vec<f64> = values
                .iter()
                .filter_map(|cell| match cell {
                    Cell::Integer(value) => Some(*value as f64),
                    Cell::Real(value) => Some(*value),
                    _ => None,
                })
                .collect();
            column
                .typed::<DoubleType>()
                .write_batch(&present, Some(&definitions), None)?;
        }
        ColumnType::Text => {
            let present: Vec<ByteArray> = values
                .iter()
                .filter_map(text_of)
                .map(|text| ByteArray::from(text.into_bytes()))
                .collect();
            column
                .typed::<ByteArrayType>()
                .write_batch(&present, Some(&definitions), None)?;
        }
        ColumnType::Blob => {
            let present: Vec<ByteArray> = values
                .iter()
                .filter_map(|cell| match cell {
                    Cell::Blob(bytes) => Some(ByteArray::from(bytes.clone())),
                    Cell::Null => None,
                    other => text_of(other).map(|text| ByteArray::from(text.into_bytes())),
                })
                .collect();
            column
                .typed::<ByteArrayType>()
                .write_batch(&present, Some(&definitions), None)?;
        }
    }
    Ok(())
}

/// One value as the text a string column carries it as: the engine's own rendering of a
/// number, the text itself, and the lossless UTF-8 reading of a byte string (a blob that is
/// not UTF-8 is carried as the bytes it holds, which is what a `BYTE_ARRAY` leaf stores).
fn text_of(cell: &Cell) -> Option<String> {
    match cell {
        Cell::Null => None,
        Cell::Integer(value) => Some(value.to_string()),
        Cell::Real(value) => Some(value.to_string()),
        Cell::Text(text) => Some(text.clone()),
        Cell::Blob(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
    }
}
