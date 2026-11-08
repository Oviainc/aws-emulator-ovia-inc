// SPDX-License-Identifier: CC0-1.0

//! The CONSTRAINT a refused row violated, named as the engine the client is talking to
//! names it.
//!
//! The engines refuse a row that violates a unique, primary-key or foreign-key constraint by
//! NAMING that constraint: PostgreSQL answers `duplicate key value violates unique
//! constraint "<name>"` and `insert or update on table "<table>" violates foreign key
//! constraint "<name>"`, and a client reads the name to tell WHICH rule it broke — two
//! unique constraints on one table are two different conditions to a client, and the
//! embedded engine's own sentence (`UNIQUE constraint failed: <table>.<column>`) names
//! neither the constraint nor, for a foreign key, anything at all.
//!
//! So the name is resolved from the engine's OWN CATALOG: the `CREATE TABLE` statement the
//! database holds for the table is read back and parsed ([`sqlparser`]), and the constraint
//! whose columns the refusal names is the one that refused the row. A constraint the client
//! named with `CONSTRAINT <name>` is reported under that name; one it did not name is
//! reported under the name PostgreSQL itself would have given it, which is
//! `<table>_<column(s)>_<kind>` with `pkey` for a primary key, `key` for a unique constraint
//! and `fkey` for a foreign key (PostgreSQL, "Constraint Naming"). Nothing is invented: a
//! constraint the catalog does not account for leaves the engine's own sentence as it was.

use sqlparser::ast::{ColumnOption, Statement, TableConstraint};
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::Parser;

use super::sql_engine::{Cell, EngineError, EngineErrorKind, InstanceDatabase};
use super::wire_session::SessionFacts;
use super::EngineWire;

/// The refusal `error`, worded as the session's own engine words a constraint violation.
///
/// Only a refusal the embedded engine reported as a constraint violation is reworded, and
/// only for the PostgreSQL wire: the MySQL wording for the same conditions quotes the
/// duplicate VALUE, which the engine's refusal does not carry.
#[must_use]
pub fn as_the_engine_names_it(
    database: &InstanceDatabase,
    facts: &SessionFacts,
    target: &str,
    sql: &str,
    error: EngineError,
) -> EngineError {
    if facts.wire != EngineWire::Postgres {
        return error;
    }
    match error.kind {
        EngineErrorKind::UniqueViolation => unique_violation(database, target, &error),
        EngineErrorKind::ForeignKeyViolation => {
            foreign_key_violation(database, target, sql, &error)
        }
        _ => error,
    }
}

/// `duplicate key value violates unique constraint "<name>"`, for the constraint whose
/// columns the engine's own sentence names.
fn unique_violation(
    database: &InstanceDatabase,
    target: &str,
    error: &EngineError,
) -> EngineError {
    let Some((table, columns)) = violated_columns(&error.message) else {
        return error.clone();
    };
    let Some(name) = unique_constraint_named(database, target, &table, &columns) else {
        return error.clone();
    };
    EngineError::new(
        error.kind,
        format!("duplicate key value violates unique constraint \"{name}\""),
    )
}

/// The table and the columns `UNIQUE constraint failed: <table>.<column>, …` names, or
/// `None` for a sentence of another shape.
fn violated_columns(message: &str) -> Option<(String, Vec<String>)> {
    let listed = message.strip_prefix("UNIQUE constraint failed: ")?;
    let mut table = String::new();
    let mut columns = Vec::new();
    for pair in listed.split(',') {
        let (named_table, column) = pair.trim().split_once('.')?;
        if table.is_empty() {
            table = named_table.to_string();
        } else if !table.eq_ignore_ascii_case(named_table) {
            // Two tables in one refusal is not a shape this engine reports.
            return None;
        }
        columns.push(column.to_ascii_lowercase());
    }
    (!table.is_empty() && !columns.is_empty()).then_some((table, columns))
}

/// The name of the unique or primary-key constraint of `table` over `columns`.
fn unique_constraint_named(
    database: &InstanceDatabase,
    target: &str,
    table: &str,
    columns: &[String],
) -> Option<String> {
    let created = create_statement(database, target, table)?;
    let Statement::CreateTable(created) = created else {
        return None;
    };
    for constraint in &created.constraints {
        match constraint {
            TableConstraint::Unique {
                name,
                columns: named,
                ..
            } if same_columns(named, columns) => {
                return Some(constraint_name(name, table, columns, "key"));
            }
            TableConstraint::PrimaryKey {
                name,
                columns: named,
                ..
            } if same_columns(named, columns) => {
                return Some(constraint_name(name, table, columns, "pkey"));
            }
            _ => {}
        }
    }
    // A constraint declared on the COLUMN itself, which is the one-column case.
    let [only] = columns else {
        return None;
    };
    for column in &created.columns {
        if !names_match(&column.name.value, only) {
            continue;
        }
        for option in &column.options {
            if let ColumnOption::Unique { is_primary, .. } = option.option {
                let kind = if is_primary { "pkey" } else { "key" };
                return Some(constraint_name(&option.name, table, columns, kind));
            }
        }
    }
    None
}

/// `insert or update on table "<child>" violates foreign key constraint "<name>"`, or the
/// sentence for the other end of the same constraint when the statement removed a parent row.
fn foreign_key_violation(
    database: &InstanceDatabase,
    target: &str,
    sql: &str,
    error: &EngineError,
) -> EngineError {
    let Some(statement) = one_statement(sql) else {
        return error.clone();
    };
    let (table, removes_a_parent) = match &statement {
        Statement::Insert(insert) => (insert.table.to_string(), false),
        Statement::Update { table, .. } => (table.to_string(), false),
        Statement::Delete(delete) => match &delete.from {
            sqlparser::ast::FromTable::WithFromKeyword(tables)
            | sqlparser::ast::FromTable::WithoutKeyword(tables) => {
                match tables.first().map(|from| from.relation.to_string()) {
                    Some(table) => (table, true),
                    None => return error.clone(),
                }
            }
        },
        _ => return error.clone(),
    };
    let table = unquoted(&table);
    // A statement that writes a row is refused for the child's own constraint; a statement
    // that removes one is refused for a constraint pointing AT it, which is the other end of
    // the same rule and the other sentence PostgreSQL has for it.
    if removes_a_parent {
        let Some((child, name)) = referring_constraint(database, target, &table) else {
            return error.clone();
        };
        return EngineError::new(
            error.kind,
            format!(
                "update or delete on table \"{table}\" violates foreign key constraint \
                 \"{name}\" on table \"{child}\""
            ),
        );
    }
    let Some(name) = foreign_key_of(database, target, &table) else {
        return error.clone();
    };
    EngineError::new(
        error.kind,
        format!(
            "insert or update on table \"{table}\" violates foreign key constraint \"{name}\""
        ),
    )
}

/// The name of the foreign-key constraint `table` carries, for the table that carries ONE:
/// which rule a row broke is only decided for a table with a single foreign key, and a table
/// with several keeps the engine's own sentence rather than being told the wrong rule.
fn foreign_key_of(database: &InstanceDatabase, target: &str, table: &str) -> Option<String> {
    let keys = foreign_keys_in(&create_sql(database, target, table)?);
    let [(_, columns, name)] = keys.as_slice() else {
        return None;
    };
    Some(constraint_name(name, table, columns, "fkey"))
}

/// The table that refers to `table` and the name of the constraint it refers with, for the
/// one table that does.
///
/// The candidates are read in ONE statement from the engine's own catalog — the tables whose
/// definition MENTIONS this one, which is what a reference to it has to — and at most
/// [`MAX_REFERRING_CANDIDATES`] of them are read: naming the rule a row broke must not cost
/// one statement and one parse per table of the database.
fn referring_constraint(
    database: &InstanceDatabase,
    target: &str,
    table: &str,
) -> Option<(String, String)> {
    let mut found: Option<(String, String)> = None;
    for (child, ddl) in candidates_referring_to(database, target, table) {
        for (parent, columns, name) in foreign_keys_in(&ddl) {
            if !names_match(&parent, table) {
                continue;
            }
            if found.is_some() {
                // More than one table refers to this one: which rule the row broke is not
                // decided here.
                return None;
            }
            let named = constraint_name(&name, &child, &columns, "fkey");
            found = Some((child.clone(), named));
        }
    }
    found
}

/// How many of a database's tables are read when a refusal has to name the constraint that
/// refers to one of them.
///
/// The catalog itself narrows the candidates to the tables whose definition mentions the one
/// the row was removed from; this bounds the work that narrowing can still leave, so a
/// refusal costs a bounded number of parses whatever the database holds. Past it the engine's
/// own sentence is what the client is told.
const MAX_REFERRING_CANDIDATES: usize = 16;

/// The tables of `target` whose own definition mentions `table`, with that definition: the
/// candidates for a constraint referring to it, as the engine's catalog holds them.
fn candidates_referring_to(
    database: &InstanceDatabase,
    target: &str,
    table: &str,
) -> Vec<(String, String)> {
    let Ok(answer) = database.execute(
        target,
        "SELECT name, sql FROM sqlite_master WHERE type = 'table' AND sql IS NOT NULL \
         AND name NOT LIKE 'sqlite_%' AND lower(sql) LIKE '%' || lower(?1) || '%' \
         ORDER BY name LIMIT ?2",
        &[
            Some(table.to_string()),
            Some(MAX_REFERRING_CANDIDATES.to_string()),
        ],
    ) else {
        return Vec::new();
    };
    answer
        .rows
        .iter()
        .filter_map(|row| match (row.first(), row.get(1)) {
            (Some(Cell::Text(name)), Some(Cell::Text(ddl))) => Some((name.clone(), ddl.clone())),
            _ => None,
        })
        .collect()
}

/// The foreign keys the definition `ddl` declares: the table each refers to, the columns it
/// is over, and the name the client gave it, if any.
fn foreign_keys_in(ddl: &str) -> Vec<(String, Vec<String>, Option<sqlparser::ast::Ident>)> {
    let Some(Statement::CreateTable(created)) = one_statement(ddl) else {
        return Vec::new();
    };
    let mut keys = Vec::new();
    for constraint in &created.constraints {
        if let TableConstraint::ForeignKey {
            name,
            columns,
            foreign_table,
            ..
        } = constraint
        {
            keys.push((
                unquoted(&foreign_table.to_string()),
                columns
                    .iter()
                    .map(|column| column.value.to_ascii_lowercase())
                    .collect(),
                name.clone(),
            ));
        }
    }
    // A reference declared on the COLUMN itself.
    for column in &created.columns {
        for option in &column.options {
            if let ColumnOption::ForeignKey { foreign_table, .. } = &option.option {
                keys.push((
                    unquoted(&foreign_table.to_string()),
                    vec![column.name.value.to_ascii_lowercase()],
                    option.name.clone(),
                ));
            }
        }
    }
    keys
}

/// The `CREATE TABLE` statement the database `target` holds for `table`, as its text.
fn create_sql(database: &InstanceDatabase, target: &str, table: &str) -> Option<String> {
    let answer = database
        .execute(
            target,
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND lower(name) = lower(?1)",
            &[Some(table.to_string())],
        )
        .ok()?;
    match answer.rows.first().and_then(|row| row.first()) {
        Some(Cell::Text(text)) => Some(text.clone()),
        _ => None,
    }
}

/// The `CREATE TABLE` statement the database `target` holds for `table`, parsed.
fn create_statement(
    database: &InstanceDatabase,
    target: &str,
    table: &str,
) -> Option<Statement> {
    one_statement(&create_sql(database, target, table)?)
}

/// `sql` as the one statement it is, or `None` where it is not one statement the parser
/// reads.
fn one_statement(sql: &str) -> Option<Statement> {
    let mut statements = Parser::parse_sql(&PostgreSqlDialect {}, sql).ok()?;
    (statements.len() == 1).then(|| statements.remove(0))
}

/// The name a constraint is reported under: the one the client gave it, else the name
/// PostgreSQL itself would have given it (see this module's own notes).
fn constraint_name(
    named: &Option<sqlparser::ast::Ident>,
    table: &str,
    columns: &[String],
    kind: &str,
) -> String {
    if let Some(named) = named {
        return named.value.clone();
    }
    if kind == "pkey" {
        return format!("{table}_pkey");
    }
    let mut name = table.to_string();
    for column in columns {
        name.push('_');
        name.push_str(column);
    }
    format!("{name}_{kind}")
}

/// Whether a constraint's column list is the one the refusal named, in the same order.
fn same_columns<T: std::fmt::Display>(named: &[T], columns: &[String]) -> bool {
    let named: Vec<String> = named
        .iter()
        .map(|column| column_of(&column.to_string()))
        .collect();
    named.len() == columns.len()
        && named
            .iter()
            .zip(columns)
            .all(|(named, column)| names_match(named, column))
}

/// The column one entry of a constraint's column list names: its own name, without the sort
/// order or the operator class an index column may carry.
fn column_of(entry: &str) -> String {
    let entry = entry.split_whitespace().next().unwrap_or(entry);
    unquoted(entry)
}

/// One identifier without the quoting a catalog entry carries.
fn unquoted(name: &str) -> String {
    name.trim_matches(|character| character == '"' || character == '`' || character == '[')
        .trim_matches(']')
        .to_string()
}

/// Whether two identifiers name the same thing: both engines resolve a name
/// case-insensitively here.
fn names_match(left: &str, right: &str) -> bool {
    unquoted(left).eq_ignore_ascii_case(&unquoted(right))
}
