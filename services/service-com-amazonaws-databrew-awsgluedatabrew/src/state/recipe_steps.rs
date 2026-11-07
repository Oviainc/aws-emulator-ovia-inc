// SPDX-License-Identifier: CC0-1.0

//! The TRANSFORMATIONS a recipe's steps name, applied to a read table.
//!
//! A recipe is "one or more actions to be performed on a DataBrew dataset" (model:
//! `Recipe`), each action "a valid DataBrew transformation" named by an `Operation` and
//! parameterised by a `Parameters` map, optionally gated by `ConditionExpression`s:
//! "conditions are used with recipe actions. The action is only performed for column
//! values where the condition evaluates to true" (model: `ConditionExpression`). That is
//! the whole rule this module implements: select the rows the step's conditions hold for,
//! then perform the step's action on them.
//!
//! A step this emulator does not perform is an ERROR rather than a step quietly skipped
//! ([`apply_steps`] answers `Err`, and the run that asked for it FAILS carrying the
//! reason): a run that reported success over output the recipe did not produce would be
//! indistinguishable from one that transformed the data. The step that names a column the
//! data does not have is diagnosed by the sentence DataBrew words such a run's
//! `ErrorMessage` with ([`recipe_step_error_message`]): the step's 1-based index, the
//! column it named, and the columns the data really carries.
//!
//! Nothing here evaluates caller-supplied TEXT: an action is a name from
//! [`PERFORMED_OPERATIONS`] with a parameter map, and a condition is a name from
//! [`EVALUATED_CONDITIONS`] with a value, so there is no expression language to parse and
//! no interpreter to bound. The one parse is the condition VALUE, which the console
//! writes as a JSON array of the values to match, and that is `serde_json`'s.

use std::collections::HashMap;

use super::tabular_data::{self, Cell, Table};
use crate::ops::types::{RecipeAction, RecipeStep};

/// The `Parameters` keys naming the column an action reads (DataBrew recipe actions
/// reference: a column-wise action takes `sourceColumn`, and a two-column action its
/// `sourceColumn1`/`sourceColumn2` pair).
const SOURCE_COLUMNS_SINGLY: [&str; 3] = ["sourceColumn", "sourceColumn1", "sourceColumn2"];

/// The `Parameters` key naming the column an action reads (DataBrew recipe actions
/// reference: every column-wise action takes `sourceColumn`).
const SOURCE_COLUMN: &str = "sourceColumn";

/// The `Parameters` key naming SEVERAL columns an action reads, as a JSON array of column
/// names (DataBrew recipe actions reference: `DELETE` takes `sourceColumns`).
const SOURCE_COLUMNS: &str = "sourceColumns";

/// The `Parameters` key naming the column an action writes (DataBrew recipe actions
/// reference: `RENAME` takes `targetColumn`).
const TARGET_COLUMN: &str = "targetColumn";

/// The `Parameters` key naming the type a coercion reads a column as (DataBrew recipe
/// actions reference: `CHANGE_DATA_TYPE` takes `columnDataType`).
const COLUMN_DATA_TYPE: &str = "columnDataType";

/// The `Parameters` key naming the separator a `MERGE` joins its source columns with
/// (DataBrew recipe actions reference: `MERGE` takes `delimiter`).
const DELIMITER: &str = "delimiter";

/// The recipe actions this emulator performs, as the DataBrew recipe actions reference
/// names them. A recipe step naming anything else fails the run that references it
/// rather than being skipped.
pub const PERFORMED_OPERATIONS: [&str; 7] = [
    "RENAME",
    "DELETE",
    "MERGE",
    "REMOVE_VALUES",
    "CHANGE_DATA_TYPE",
    "UPPER_CASE",
    "LOWER_CASE",
];

/// The condition names this emulator evaluates, as DataBrew's recipe structure names
/// them. A condition expression naming anything else fails the run that references it.
pub const EVALUATED_CONDITIONS: [&str; 14] = [
    "IS",
    "IS_NOT",
    "CONTAINS",
    "NOT_CONTAINS",
    "STARTS_WITH",
    "NOT_STARTS_WITH",
    "ENDS_WITH",
    "NOT_ENDS_WITH",
    "IS_MISSING",
    "IS_NOT_MISSING",
    "LESS_THAN",
    "LESS_THAN_EQUAL",
    "GREATER_THAN",
    "GREATER_THAN_EQUAL",
];

/// The conditions that ask whether the target column holds a value at all, which are
/// the two that need no `Value` of their own.
const MISSINGNESS_CONDITIONS: [&str; 2] = ["IS_MISSING", "IS_NOT_MISSING"];

/// The column types `CHANGE_DATA_TYPE` coerces a column to, as the DataBrew recipe
/// actions reference spells them in `columnDataType`.
const COLUMN_DATA_TYPES: [&str; 7] = [
    "string", "boolean", "int", "long", "float", "double", "decimal",
];

/// The most steps this emulator applies in one run.
///
/// A recipe's step list carries no modelled length, and each step is a pass over every
/// row, so the number of passes is a growth site the caller drives: a recipe past this
/// is not applied at all rather than applied slowly. CHOSEN for this emulator (AWS
/// publishes no such limit).
pub const MAX_STEPS: usize = 100;

/// The most columns one `MERGE` joins at once. The joined value is as long as the cells
/// it reads put together, so the number of them is capped beside the length of what they
/// produce. CHOSEN for this emulator.
pub const MAX_MERGED_COLUMNS: usize = 32;

/// The most condition expressions one step carries. `ConditionExpressionList` has no
/// modelled length, and each condition is evaluated once per row, so the number of them
/// multiplies the pass. CHOSEN for this emulator.
pub const MAX_CONDITIONS_PER_STEP: usize = 16;

/// The most values one condition compares a cell against — the entries of the JSON array
/// its `Value` carries — for the same reason: each entry is compared once per row.
/// CHOSEN for this emulator.
pub const MAX_CONDITION_VALUES: usize = 256;

/// How many rows one pass walks between yield points.
///
/// A pass over the rows is CPU work with no await of its own, so it hands the runtime a
/// yield point every so many rows: a cancelled request or a shutdown has somewhere to
/// land in the middle of a step, not only between steps. CHOSEN for this emulator.
const ROWS_PER_YIELD: usize = 4_096;

/// Hand the runtime a yield point at every [`ROWS_PER_YIELD`]th row of a pass.
async fn yield_between_rows(position: usize) {
    if position > 0 && position.is_multiple_of(ROWS_PER_YIELD) {
        ::tokio::task::yield_now().await;
    }
}

/// The `ErrorMessage` a run that could not apply one of its recipe's steps because the
/// step names a column the data does not have reports.
///
/// `step` is the 1-based index of that step, `column` the column it names, and `columns`
/// the columns the DATASET really carries, in the order the data carries them — the three
/// facts an operator needs to repair either the recipe or the dataset.
pub fn recipe_step_error_message(step: usize, column: &str, columns: &[String]) -> String {
    format!(
        "RecipeStepError: Failed at step {step}: Column '{column}' does not exist. \
         The dataset's columns are: {}",
        columns.join(", ")
    )
}

/// The `ErrorMessage` a run reports for a step it could not apply for any other reason —
/// one this emulator does not perform, a parameter it carries none of — stated the same
/// way, so a client reads every step failure off the same sentence.
fn unapplicable_step_message(step: usize, reason: &str) -> String {
    format!("RecipeStepError: Failed at step {step}: {reason}")
}

/// `table` with every step of `steps` performed on it, in order.
///
/// `Err` names the step that could not be applied and why, which is what the run reports
/// as its `ErrorMessage` instead of a success. A step is applied against the columns the
/// data has AT THAT POINT in the recipe, so a step reading a column an earlier step wrote
/// applies, while the first step naming a column that is not there ends the run.
///
/// Every pass is CPU work with no await of its own, so the runtime is handed a yield
/// point between steps AND inside each pass ([`ROWS_PER_YIELD`]): a cancelled request or
/// a shutdown has somewhere to land while a long recipe is being applied.
pub async fn apply_steps(mut table: Table, steps: &[RecipeStep]) -> Result<Table, String> {
    if steps.len() > MAX_STEPS {
        return Err(format!(
            "The recipe holds {} steps, more than the {MAX_STEPS} this emulator applies in one \
             job run.",
            steps.len()
        ));
    }
    // The columns the DATASET carries, for the diagnosis: a step is judged against the
    // columns the table has when it runs, while the sentence reports the data's own.
    let dataset_columns = table.columns.clone();
    for (position, step) in steps.iter().enumerate() {
        ::tokio::task::yield_now().await;
        let number = position + 1;
        for referenced in referenced_columns(step) {
            if table.column_index(&referenced).is_none() {
                return Err(recipe_step_error_message(
                    number,
                    &referenced,
                    &dataset_columns,
                ));
            }
        }
        apply_step(&mut table, step)
            .await
            .map_err(|reason| unapplicable_step_message(number, &reason))?;
    }
    Ok(table)
}

/// The columns one recipe step names and the data must ALREADY have, in a stable order:
/// the action's own source parameters, then the names its `sourceColumns` array carries,
/// then the column each of its conditions applies to (model:
/// `ConditionExpression$TargetColumn` — "A column to apply this condition to").
///
/// A parameter naming the column a step WRITES ([`TARGET_COLUMN`]) is not one of them: a
/// step that renames or merges into a column names the result there, and that name IS one
/// of the data's columns for the steps after it. The order is fixed so one recipe's
/// diagnosis names the same column on every run of it.
fn referenced_columns(step: &RecipeStep) -> Vec<String> {
    let mut referenced = Vec::new();
    if let Some(parameters) = step.action.parameters.as_ref() {
        for name in SOURCE_COLUMNS_SINGLY {
            if let Some(column) = parameters.get(name).filter(|value| !value.is_empty()) {
                referenced.push(column.clone());
            }
        }
        if let Some(list) = parameters.get(SOURCE_COLUMNS) {
            referenced.extend(value_list(list));
        }
    }
    for condition in step.condition_expressions.iter().flatten() {
        referenced.push(condition.target_column.clone());
    }
    referenced
}

/// One condition of a step, ready to be evaluated against a row: the column it reads,
/// the condition's name, and the values it compares against — PARSED ONCE for the step
/// rather than once per row.
struct Gate<'a> {
    column: usize,
    condition: &'a str,
    wanted: Vec<String>,
}

/// Perform one step: the rows its conditions hold for, transformed by its action.
async fn apply_step(table: &mut Table, step: &RecipeStep) -> Result<(), String> {
    let operation = step.action.operation.as_str();
    if !PERFORMED_OPERATIONS.contains(&operation) {
        return Err(format!(
            "{operation} is not a transformation this emulator performs. It performs {}.",
            PERFORMED_OPERATIONS.join(", ")
        ));
    }
    let gates = gates_of(table, step)?;

    match operation {
        // A rename is a change to the HEADER, so it is performed on the table rather
        // than on the rows a condition selects.
        "RENAME" => rename(table, &step.action),
        "DELETE" => delete(table, &step.action),
        "MERGE" => merge(table, &step.action).await,
        "REMOVE_VALUES" => remove_rows(table, &gates).await,
        "CHANGE_DATA_TYPE" => change_data_type(table, &step.action, &gates).await,
        "UPPER_CASE" => {
            map_cells(table, &step.action, &gates, |cell| cell.to_uppercase()).await
        }
        "LOWER_CASE" => {
            map_cells(table, &step.action, &gates, |cell| cell.to_lowercase()).await
        }
        // Unreachable: the operation was checked against [`PERFORMED_OPERATIONS`] above.
        other => Err(format!("{other} is not a transformation this emulator performs.")),
    }
}

/// The gates one step's conditions put on its rows, or the reason the step cannot be
/// applied.
///
/// Each condition's `Value` is read into its value set ONCE here, so a pass over the rows
/// re-parses nothing however many rows there are. The number of conditions and the number
/// of values each carries are capped, because both multiply that pass.
fn gates_of<'a>(table: &Table, step: &'a RecipeStep) -> Result<Vec<Gate<'a>>, String> {
    let conditions = step.condition_expressions.as_deref().unwrap_or_default();
    if conditions.len() > MAX_CONDITIONS_PER_STEP {
        return Err(format!(
            "the step carries {} conditions, more than the {MAX_CONDITIONS_PER_STEP} this \
             emulator evaluates for one step.",
            conditions.len()
        ));
    }
    let mut gates = Vec::with_capacity(conditions.len());
    for condition in conditions {
        let name = condition.condition.as_str();
        if !EVALUATED_CONDITIONS.contains(&name) {
            return Err(format!(
                "{name} is not a condition this emulator evaluates. It evaluates {}.",
                EVALUATED_CONDITIONS.join(", ")
            ));
        }
        let column = table
            .column_index(&condition.target_column)
            .ok_or_else(|| absent_column(&condition.target_column))?;
        let wanted = condition.value.as_deref().map(value_list).unwrap_or_default();
        // Every condition but the two that ask whether the cell holds a value at all
        // compares it AGAINST a value, so one carrying none decides nothing — and
        // neither does one carrying an EMPTY list of values, which would otherwise hold
        // for every row of a negated condition.
        if !MISSINGNESS_CONDITIONS.contains(&name) && wanted.is_empty() {
            return Err(format!(
                "the {name} condition carries no Value to evaluate against."
            ));
        }
        if wanted.len() > MAX_CONDITION_VALUES {
            return Err(format!(
                "the {name} condition carries {} values, more than the \
                 {MAX_CONDITION_VALUES} this emulator compares a cell against.",
                wanted.len()
            ));
        }
        gates.push(Gate {
            column,
            condition: name,
            wanted,
        });
    }
    Ok(gates)
}

/// `MERGE`: a new column named `targetColumn` holds every row's source cells joined by
/// the step's `delimiter`, and the source columns stay where they are.
///
/// The sources are the `sourceColumns` array, else the `sourceColumn1`/`sourceColumn2`
/// pair a two-column action names them with. A step naming no delimiter joins the cells
/// with nothing between them (CHOSEN: the reference documents the parameter, not its
/// default). A NULL source cell contributes nothing, so a merge of a missing value is the
/// rest of the values rather than a NULL row.
///
/// A merge JOINS values, so it is the one step that can produce more data than it read:
/// the source set must therefore name each column once and must not name the column the
/// step writes (a step merging a column into itself would double its cells, and a recipe
/// of such steps would multiply them), and what it produces is held to the cell and table
/// ceilings [`super::tabular_data`] states.
async fn merge(table: &mut Table, action: &RecipeAction) -> Result<(), String> {
    let target = parameter(action, TARGET_COLUMN)?.to_string();
    let sources = merged_sources(action)?;
    if sources.len() > MAX_MERGED_COLUMNS {
        return Err(format!(
            "a merge of more than the {MAX_MERGED_COLUMNS} columns this emulator joins at once \
             was asked for."
        ));
    }
    for (position, column) in sources.iter().enumerate() {
        if sources[..position].contains(column) {
            return Err(format!("the merge names the column {column} more than once."));
        }
        if column == &target {
            return Err(format!(
                "the merge names the column {column} it writes among the columns it reads."
            ));
        }
    }
    let indexes: Vec<usize> = sources
        .iter()
        .map(|column| {
            table
                .column_index(column)
                .ok_or_else(|| absent_column(column))
        })
        .collect::<Result<_, String>>()?;
    let delimiter = parameter(action, DELIMITER).unwrap_or_default().to_string();
    let base = table.byte_size();
    let mut merged_cells: Vec<Cell> = Vec::with_capacity(table.rows.len());
    let mut produced = 0usize;
    for (position, row) in table.rows.iter().enumerate() {
        yield_between_rows(position).await;
        let merged = indexes
            .iter()
            .filter_map(|index| Table::cell_at(row, *index))
            .collect::<Vec<&str>>()
            .join(&delimiter);
        // What the step produces is checked BEFORE it is held, per cell and in total, so
        // a recipe cannot grow the table past the ceiling by joining. The table's own
        // size is taken once, outside the pass.
        if merged.len() > tabular_data::MAX_CELL_BYTES {
            return Err(tabular_data::cell_too_large());
        }
        produced = produced.saturating_add(merged.len());
        if base.saturating_add(produced) > tabular_data::MAX_TABLE_BYTES {
            return Err(tabular_data::table_too_large());
        }
        merged_cells.push(Some(merged));
    }
    for (row, merged) in table.rows.iter_mut().zip(merged_cells) {
        row.push(merged);
    }
    // The merged column is appended, so an existing column of that name is replaced by
    // the merge rather than held twice.
    if let Some(held) = table.column_index(&target) {
        table.columns.remove(held);
        for row in &mut table.rows {
            if held < row.len() {
                row.remove(held);
            }
        }
    }
    table.columns.push(target);
    Ok(())
}

/// The columns a `MERGE` joins: its `sourceColumns` array, else its
/// `sourceColumn1`/`sourceColumn2` pair.
fn merged_sources(action: &RecipeAction) -> Result<Vec<String>, String> {
    if let Ok(list) = parameter(action, SOURCE_COLUMNS) {
        let named = value_list(list);
        if !named.is_empty() {
            return Ok(named);
        }
    }
    let mut pair = Vec::new();
    for name in SOURCE_COLUMNS_SINGLY {
        if let Ok(column) = parameter(action, name) {
            pair.push(column.to_string());
        }
    }
    if pair.is_empty() {
        return Err(format!(
            "the {} action carries no {SOURCE_COLUMNS} parameter.",
            action.operation
        ));
    }
    Ok(pair)
}

/// `RENAME`: the column `sourceColumn` is held under the name `targetColumn` from here
/// on, in the header and in every later step that names it. The rows are untouched — a
/// rename moves no cell.
///
/// A rename onto a name the table ALREADY holds is refused: two columns of one name make
/// every later step that names it ambiguous, so the recipe cannot be applied.
fn rename(table: &mut Table, action: &RecipeAction) -> Result<(), String> {
    let source = parameter(action, SOURCE_COLUMN)?;
    let target = parameter(action, TARGET_COLUMN)?;
    let index = table
        .column_index(source)
        .ok_or_else(|| absent_column(source))?;
    if target != source && table.column_index(target).is_some() {
        return Err(format!(
            "the rename would leave two columns named {target}."
        ));
    }
    match table.columns.get_mut(index) {
        Some(column) => {
            *column = target.to_string();
            Ok(())
        }
        None => Err(absent_column(source)),
    }
}

/// `DELETE`: the named column (or every column of `sourceColumns`) and its cells are
/// dropped from the table.
fn delete(table: &mut Table, action: &RecipeAction) -> Result<(), String> {
    let named = match parameter(action, SOURCE_COLUMNS).ok() {
        Some(columns) => value_list(columns),
        None => vec![parameter(action, SOURCE_COLUMN)?.to_string()],
    };
    for column in &named {
        let index = table
            .column_index(column)
            .ok_or_else(|| absent_column(column))?;
        table.columns.remove(index);
        for row in &mut table.rows {
            if index < row.len() {
                row.remove(index);
            }
        }
    }
    Ok(())
}

/// `REMOVE_VALUES`: the rows the step's conditions hold for are removed from the table.
/// The header is not touched, so a filtered output carries the same columns the dataset
/// did. A step carrying no condition holds for every row, exactly as an unconditioned
/// action is performed on every row.
async fn remove_rows(table: &mut Table, gates: &[Gate<'_>]) -> Result<(), String> {
    let selected = selected_rows(table, gates).await;
    let mut kept: Vec<Vec<Cell>> = Vec::new();
    for (position, row) in table.rows.drain(..).enumerate() {
        if !selected.get(position).copied().unwrap_or(false) {
            kept.push(row);
        }
    }
    table.rows = kept;
    Ok(())
}

/// `CHANGE_DATA_TYPE`: each selected cell of `sourceColumn` is read as
/// `columnDataType`. A cell that reads as that type keeps its value; a cell that does not
/// becomes NULL and its ROW is retained — a coercion reports what it could not read as a
/// missing value, it does not drop the record.
async fn change_data_type(
    table: &mut Table,
    action: &RecipeAction,
    gates: &[Gate<'_>],
) -> Result<(), String> {
    let data_type = parameter(action, COLUMN_DATA_TYPE)?.to_ascii_lowercase();
    if !COLUMN_DATA_TYPES.contains(&data_type.as_str()) {
        return Err(format!(
            "{data_type} is not a column type this emulator coerces to. It coerces to {}.",
            COLUMN_DATA_TYPES.join(", ")
        ));
    }
    let source = parameter(action, SOURCE_COLUMN)?;
    let index = table
        .column_index(source)
        .ok_or_else(|| absent_column(source))?;
    let selected = selected_rows(table, gates).await;
    for (position, row) in table.rows.iter_mut().enumerate() {
        yield_between_rows(position).await;
        if !selected.get(position).copied().unwrap_or(false) {
            continue;
        }
        if let Some(cell) = row.get_mut(index) {
            *cell = cell.as_deref().and_then(|held| coerce(held, &data_type));
        }
    }
    Ok(())
}

/// Apply `transform` to every selected cell of the column `sourceColumn` names.
async fn map_cells(
    table: &mut Table,
    action: &RecipeAction,
    gates: &[Gate<'_>],
    transform: impl Fn(&str) -> String,
) -> Result<(), String> {
    let source = parameter(action, SOURCE_COLUMN)?;
    let index = table
        .column_index(source)
        .ok_or_else(|| absent_column(source))?;
    let selected = selected_rows(table, gates).await;
    for (position, row) in table.rows.iter_mut().enumerate() {
        yield_between_rows(position).await;
        if !selected.get(position).copied().unwrap_or(false) {
            continue;
        }
        if let Some(cell) = row.get_mut(index) {
            *cell = cell.as_deref().map(&transform);
        }
    }
    Ok(())
}

/// Which rows of `table` every gate of `gates` holds for, by position.
///
/// "If a recipe requires more than one condition, then the recipe must specify multiple
/// `ConditionExpression` elements. Each condition is applied to the rows in a dataset
/// first, before the recipe action is performed" (model: `ConditionExpression`) — so the
/// conditions are applied together and a row qualifies only where every one of them
/// holds. A step with no condition qualifies every row.
///
/// The pass re-parses nothing: each gate carries the value set its condition was read
/// into once ([`gates_of`]), and the walk hands the runtime a yield point every
/// [`ROWS_PER_YIELD`] rows.
async fn selected_rows(table: &Table, gates: &[Gate<'_>]) -> Vec<bool> {
    let mut selected = Vec::with_capacity(table.rows.len());
    for (position, row) in table.rows.iter().enumerate() {
        yield_between_rows(position).await;
        selected.push(gates.iter().all(|gate| {
            condition_holds(gate.condition, &gate.wanted, Table::cell_at(row, gate.column))
        }));
    }
    selected
}

/// Whether one condition holds for `cell`, the value of the row's target column.
///
/// The condition's `Value` is "a value that the condition must evaluate to for the
/// condition to succeed" (model: `ConditionExpression$Value`). DataBrew writes it as a
/// JSON array of the values to match, so an array holds where the cell matches ANY of
/// its entries; a value that is not an array is matched as the single value it is.
/// `wanted` is that value set, already read ([`gates_of`]).
fn condition_holds(name: &str, wanted: &[String], cell: Option<&str>) -> bool {
    match name {
        "IS_MISSING" => return cell.is_none_or(str::is_empty),
        "IS_NOT_MISSING" => return cell.is_some_and(|cell| !cell.is_empty()),
        _ => {}
    }
    let Some(cell) = cell else {
        // Every remaining condition compares the cell against a value, and a NULL cell
        // equals none of them: what holds for it is `IS_MISSING`.
        return false;
    };
    let holds_for_any = wanted.iter().any(|wanted| match name {
        "IS" | "IS_NOT" => cell == wanted,
        "CONTAINS" | "NOT_CONTAINS" => cell.contains(wanted.as_str()),
        "STARTS_WITH" | "NOT_STARTS_WITH" => cell.starts_with(wanted.as_str()),
        "ENDS_WITH" | "NOT_ENDS_WITH" => cell.ends_with(wanted.as_str()),
        "LESS_THAN" => ordered_less(cell, wanted, false),
        "LESS_THAN_EQUAL" => ordered_less(cell, wanted, true),
        "GREATER_THAN" => ordered_less(wanted, cell, false),
        "GREATER_THAN_EQUAL" => ordered_less(wanted, cell, true),
        _ => false,
    });
    // The negated conditions are the complement of their positive twin over the same
    // value set, so they are stated once.
    if name.starts_with("NOT_") || name == "IS_NOT" {
        !holds_for_any
    } else {
        holds_for_any
    }
}

/// Whether `left` orders before `right` (or equals it, when `or_equal`).
///
/// An ordering condition carrying a NUMERIC value is a numeric comparison, so a cell that
/// is not itself a number satisfies neither direction of it — a column holding
/// uncoercible text is not ordered against a number. Where neither side reads as a
/// number the comparison is by text, which is how an ordering condition answers for a
/// column of labels.
fn ordered_less(left: &str, right: &str, or_equal: bool) -> bool {
    match (left.trim().parse::<f64>(), right.trim().parse::<f64>()) {
        (Ok(left), Ok(right)) => left < right || (or_equal && left == right),
        (Ok(_), Err(_)) | (Err(_), Ok(_)) => false,
        (Err(_), Err(_)) => left < right || (or_equal && left == right),
    }
}

/// A `Parameters` or condition value as the list of values it carries: the entries of a
/// JSON array, else the value itself.
fn value_list(value: &str) -> Vec<String> {
    match ::serde_json::from_str::<Vec<::serde_json::Value>>(value) {
        Ok(items) => items
            .iter()
            .map(|item| match item {
                ::serde_json::Value::String(text) => text.clone(),
                other => other.to_string(),
            })
            .collect(),
        Err(_) => vec![value.to_string()],
    }
}

/// `cell` read as the column type `data_type`, or `None` when it does not read as one.
///
/// A cell that reads as the type keeps its value: an INTEGER type renders the number it
/// read (so a whole number loses a `+` sign or leading zeros), while a fractional type
/// keeps the digits the data carried, which is the value and not a re-rendering of it. A
/// cell that does not read as the type at all answers `None`, which is the NULL the
/// coercion leaves behind.
fn coerce(cell: &str, data_type: &str) -> Option<String> {
    let trimmed = cell.trim();
    match data_type {
        "string" => Some(cell.to_string()),
        "boolean" => match trimmed.to_ascii_lowercase().as_str() {
            "true" | "1" => Some("true".to_string()),
            "false" | "0" => Some("false".to_string()),
            _ => None,
        },
        "int" | "long" => trimmed.parse::<i64>().ok().map(|read| read.to_string()),
        "float" | "double" | "decimal" => {
            let read = trimmed.parse::<f64>().ok()?;
            if read.is_finite() {
                Some(trimmed.to_string())
            } else {
                None
            }
        }
        _ => None,
    }
}

/// One `Parameters` entry of an action, or the reason the step cannot be performed
/// without it.
fn parameter<'a>(action: &'a RecipeAction, key: &str) -> Result<&'a str, String> {
    action
        .parameters
        .as_ref()
        .and_then(|parameters: &HashMap<String, String>| parameters.get(key))
        .map(String::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            format!(
                "the {} action carries no {key} parameter.",
                action.operation
            )
        })
}

/// The reason a step naming a column the table does not hold cannot be performed.
fn absent_column(column: &str) -> String {
    format!("the dataset holds no column {column}.")
}
