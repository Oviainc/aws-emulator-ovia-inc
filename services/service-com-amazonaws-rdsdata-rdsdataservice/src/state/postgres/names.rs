// SPDX-License-Identifier: CC0-1.0

//! How a name the caller wrote reaches the engine.
//!
//! A statement is parsed as PostgreSQL and then RENDERED into the grammar the
//! engine behind it parses, and the two grammars reserve different words: a word
//! PostgreSQL admits as a name (`nothing`, `index`, `transaction` — every
//! non-reserved key word, and every word that is no key word of PostgreSQL at
//! all) can be a word the engine's own grammar reads as syntax, and a statement
//! using it as a name is then refused for a syntax error PostgreSQL never has.
//!
//! So every name the parse found is rendered as a DELIMITED identifier, which
//! both grammars read as exactly that name whatever words they reserve.
//! Delimiting is the only thing it changes: the engine compares identifiers
//! without regard to case whether they are delimited or not, so a name still
//! resolves to the same table or column it did, and the name a result column
//! reports is the name inside the quotes.
//!
//! The positions a name is delimited in are every one a statement this data plane
//! runs can carry: a column reference, bare or qualified, and a `t.*` prefix,
//! wherever an expression or a projection may appear; a table, wherever the
//! statement names one; the alias a `FROM` item or a common table expression is
//! given and the names it gives its columns; a result column's alias, in each of
//! the queries a set operation combines; the columns a table is declared with,
//! including a foreign key's own lists; the columns an `ALTER TABLE` adds,
//! renames or drops; and the columns an insert names, an upsert conflicts on and
//! an update or an upsert assigns.
//!
//! Three positions are deliberately left as the caller wrote them:
//!
//! * a FUNCTION's name, which the engine resolves against its own library, and
//!   which it also spells as bare syntax for the key-word functions
//!   (`CURRENT_TIMESTAMP`);
//! * a TYPE's name, which the engine's grammar reads as a declaration;
//! * the alias of a `FROM` item of a form the engine has no grammar for at all (a
//!   pivot, a JSON or XML table): the engine refuses such a statement whatever its
//!   names are rendered as, so delimiting them would change no answer.
//!
//! Delimiting a name is only half of it: the engine also has to read a delimited
//! identifier AS an identifier. SQLite's double-quoted-string misfeature would
//! otherwise fall back to reading an unresolvable one as a string literal, so the
//! two `DQS` settings that turn that fallback off are part of opening a database
//! (see `Database::open`), which is what makes an unknown column an error here as
//! it is in PostgreSQL.

use std::ops::ControlFlow;

use sqlparser::ast::{
    AlterTableOperation, Assignment, AssignmentTarget, ConflictTarget, Expr, Ident, ObjectName,
    ObjectNamePart, OnConflict, OnConflictAction, OnInsert, Query, SelectItem,
    SelectItemQualifiedWildcardKind, SetExpr, Statement, TableAlias, TableConstraint, TableFactor,
    TableObject, VisitMut, VisitorMut,
};

/// The quote both grammars delimit an identifier with.
const IDENTIFIER_QUOTE: char = '"';

/// Render every name in the statement — table, column and alias — as a delimited
/// identifier.
///
/// DEPTH: the walk descends the statement tree, so what bounds the stack it uses
/// is what bounds that tree, and both bounds are applied by [`super::plan`] BEFORE
/// a tree exists at all: the parser's own recursion limit
/// (`with_recursion_limit`) for the nesting it descends into, and the TOKEN pass
/// (`chained_levels`) for the levels a chain of operators or of combined queries
/// could ask for, which the parse builds iteratively and no parser limit sees. A
/// statement past either is refused, so this is only ever handed a tree whose
/// depth those two settle — the same depth the render that follows it, and the
/// drop of the tree itself, already walk.
pub(super) fn quote_identifiers(statement: &mut Statement) {
    // The visitor cannot break, so the walk always completes.
    let _ = statement.visit(&mut Names);
}

/// The walk that delimits the names. It visits the WHOLE statement, so a name
/// inside a subquery, a common table expression or a constraint is rendered the
/// same way as one in the statement's own clauses.
struct Names;

impl VisitorMut for Names {
    type Break = ();

    /// A column reference, wherever an expression may appear.
    fn pre_visit_expr(&mut self, expr: &mut Expr) -> ControlFlow<Self::Break> {
        match expr {
            Expr::Identifier(name) => quote(name),
            Expr::CompoundIdentifier(parts) => parts.iter_mut().for_each(quote),
            _ => {}
        }
        ControlFlow::Continue(())
    }

    /// A table, wherever the statement names one.
    fn pre_visit_relation(&mut self, relation: &mut ObjectName) -> ControlFlow<Self::Break> {
        quote_object_name(relation);
        ControlFlow::Continue(())
    }

    /// The alias a `FROM` item is given, and the names it gives its columns.
    ///
    /// A `FROM` item of a form the engine has no grammar for keeps the alias the
    /// caller wrote, for the reason the module docs give.
    fn pre_visit_table_factor(&mut self, factor: &mut TableFactor) -> ControlFlow<Self::Break> {
        let alias = match factor {
            TableFactor::Table { alias, .. }
            | TableFactor::Derived { alias, .. }
            | TableFactor::TableFunction { alias, .. }
            | TableFactor::Function { alias, .. }
            | TableFactor::UNNEST { alias, .. }
            | TableFactor::NestedJoin { alias, .. } => alias.as_mut(),
            _ => None,
        };
        if let Some(alias) = alias {
            quote_table_alias(alias);
        }
        ControlFlow::Continue(())
    }

    /// The names a query gives: its result columns' aliases, and the common table
    /// expressions it defines.
    fn pre_visit_query(&mut self, query: &mut Query) -> ControlFlow<Self::Break> {
        if let Some(with) = query.with.as_mut() {
            for cte in &mut with.cte_tables {
                quote_table_alias(&mut cte.alias);
            }
        }
        quote_result_aliases(&mut query.body);
        ControlFlow::Continue(())
    }

    /// The names a statement itself lists: the columns a table is declared with or
    /// altered to carry, the columns an insert names and the columns an update or an
    /// upsert assigns.
    fn pre_visit_statement(&mut self, statement: &mut Statement) -> ControlFlow<Self::Break> {
        match statement {
            Statement::CreateTable(create) => {
                for column in &mut create.columns {
                    quote(&mut column.name);
                }
                for constraint in &mut create.constraints {
                    quote_constrained_columns(constraint);
                }
            }
            Statement::Insert(insert) => {
                if let TableObject::TableName(name) = &mut insert.table {
                    quote_object_name(name);
                }
                insert.columns.iter_mut().for_each(quote);
                if let Some(OnInsert::OnConflict(conflict)) = insert.on.as_mut() {
                    quote_conflict(conflict);
                }
            }
            Statement::Update { assignments, .. } => {
                quote_assigned_columns(assignments);
            }
            // The columns an `ALTER TABLE` adds, renames or drops.
            Statement::AlterTable { operations, .. } => {
                for operation in operations.iter_mut() {
                    match operation {
                        AlterTableOperation::AddColumn { column_def, .. } => {
                            quote(&mut column_def.name);
                        }
                        AlterTableOperation::DropColumn { column_names, .. } => {
                            column_names.iter_mut().for_each(quote);
                        }
                        AlterTableOperation::RenameColumn {
                            old_column_name,
                            new_column_name,
                        } => {
                            quote(old_column_name);
                            quote(new_column_name);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        ControlFlow::Continue(())
    }
}

/// The columns an assignment list assigns to, in an update and in the update half
/// of an upsert alike.
fn quote_assigned_columns(assignments: &mut [Assignment]) {
    for assignment in assignments.iter_mut() {
        match &mut assignment.target {
            AssignmentTarget::ColumnName(name) => quote_object_name(name),
            AssignmentTarget::Tuple(names) => names.iter_mut().for_each(quote_object_name),
        }
    }
}

/// The names an `ON CONFLICT` clause carries: the columns or the constraint it
/// conflicts on, and the columns its update half assigns.
fn quote_conflict(conflict: &mut OnConflict) {
    match conflict.conflict_target.as_mut() {
        Some(ConflictTarget::Columns(columns)) => columns.iter_mut().for_each(quote),
        Some(ConflictTarget::OnConstraint(name)) => quote_object_name(name),
        None => {}
    }
    if let OnConflictAction::DoUpdate(update) = &mut conflict.action {
        quote_assigned_columns(&mut update.assignments);
    }
}

/// The aliases a query's result columns carry.
///
/// Walked ITERATIVELY over the queries a set operation combines, for the reason
/// the leftmost-projection walk is: a combined query's depth grows with the
/// request text. A query nested inside one is reached by the walk itself, which
/// visits every query in the statement.
fn quote_result_aliases(body: &mut SetExpr) {
    let mut pending = vec![body];
    while let Some(body) = pending.pop() {
        match body {
            SetExpr::Select(select) => {
                for item in &mut select.projection {
                    match item {
                        SelectItem::ExprWithAlias { alias, .. } => quote(alias),
                        // `t.*` names the relation whose columns it stands for.
                        SelectItem::QualifiedWildcard(
                            SelectItemQualifiedWildcardKind::ObjectName(name),
                            _,
                        ) => quote_object_name(name),
                        _ => {}
                    }
                }
            }
            SetExpr::SetOperation { left, right, .. } => {
                pending.push(left);
                pending.push(right);
            }
            _ => {}
        }
    }
}

/// The columns a table constraint names, where it names them as bare identifiers
/// rather than as the expressions the walk already reaches.
fn quote_constrained_columns(constraint: &mut TableConstraint) {
    if let TableConstraint::ForeignKey {
        columns,
        referred_columns,
        ..
    } = constraint
    {
        columns.iter_mut().for_each(quote);
        referred_columns.iter_mut().for_each(quote);
    }
}

/// An alias and the names it gives the columns under it.
fn quote_table_alias(alias: &mut TableAlias) {
    quote(&mut alias.name);
    for column in &mut alias.columns {
        quote(&mut column.name);
    }
}

/// Every part of a possibly-qualified name (`schema.table`).
fn quote_object_name(name: &mut ObjectName) {
    for part in &mut name.0 {
        if let ObjectNamePart::Identifier(identifier) = part {
            quote(identifier);
        }
    }
}

/// Delimit one name, leaving a name the caller already delimited as it wrote it.
fn quote(name: &mut Ident) {
    if name.quote_style.is_none() {
        name.quote_style = Some(IDENTIFIER_QUOTE);
    }
}
