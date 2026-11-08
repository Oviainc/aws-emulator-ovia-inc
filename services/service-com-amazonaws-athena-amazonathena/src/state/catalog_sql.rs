// SPDX-License-Identifier: CC0-1.0

//! Athena's STATEMENT layer: what a submitted statement means to the DATA CATALOG, and
//! how it is run.
//!
//! Athena is a query service over a data catalog, not a database of its own: a statement
//! either DEFINES or REPORTS a catalog object, or it READS data through relations the
//! catalog resolves. So a statement is first read as what it is
//! ([`statement::read`]), and then:
//!
//! * a catalog statement — `CREATE EXTERNAL TABLE`, `CREATE VIEW`, `DROP VIEW`,
//!   `SHOW CREATE VIEW`, `SHOW TABLES`, `SHOW VIEWS` — is performed against the AWS Glue
//!   Data Catalog ([`ddl`]), which is the one catalog `AwsDataCatalog` names, so what this
//!   DDL writes is what Glue's own reads report and what Glue writes is what these
//!   statements see;
//! * a statement that reads relations has each one resolved through that same catalog and
//!   the OBJECTS at its location decoded into rows ([`data`]), which the SQL engine then
//!   computes over ([`super::query_engine`]);
//! * anything else — a projection of constants, a statement over a table an earlier
//!   statement defined in the engine itself — is the engine's own business and runs as
//!   submitted.
//!
//! A name the catalog cannot resolve is a FAILED execution carrying Athena's own error
//! classification for it ([`Failure`]): the category, the error type and the message of
//! that failure class, with the identifiers and the line and column taken from the
//! submitted statement.

/// The objects under a catalog table's location, as the rows they hold.
pub mod data;
/// The statements the Data Catalog performs.
pub mod ddl;
/// What one submitted statement is, and the plan for running it.
pub mod statement;

use ::std::collections::{HashMap, HashSet};
use ::std::time::{Instant, SystemTime};

use ::service_com_amazonaws_glue_awsglue::state as glue_state;
use ::service_com_amazonaws_s3_amazons3::state as s3_state;

use super::query_engine::{
    self, Completion, DatabaseHandle, FailureCause, LocalColumn, LocalObject, LocalTable, Outcome,
    StoredResultSet,
};
use data::{Budget, DataSource};
use statement::{
    CatalogStatement, Plan, Position, QueryContext, Reading, Relation, RelationalStatement,
};

/// The `AthenaError$ErrorCategory` of a failure the SERVICE is the cause of: the
/// documented category list is "1 - System, 2 - User, 3 - Other".
pub const ERROR_CATEGORY_SYSTEM: i32 = 1;

/// The `AthenaError$ErrorCategory` of a failure the CALLER's own input is the cause of;
/// see [`ERROR_CATEGORY_SYSTEM`].
pub const ERROR_CATEGORY_USER: i32 = 2;

/// The `AthenaError$ErrorType` Athena publishes for the ALREADY-EXISTS class: a
/// statement that would define a catalog object the catalog already holds.
pub const ERROR_TYPE_ALREADY_EXISTS: i32 = 1006;

/// The `AthenaError$ErrorType` Athena publishes for the NOT-FOUND class: a statement
/// naming a table or a database the data catalog does not hold.
pub const ERROR_TYPE_ENTITY_NOT_FOUND: i32 = 1301;

/// The `AthenaError$ErrorType` Athena publishes for a VIEW a statement addresses that is
/// not the view it needs: one that already exists where a plain `CREATE VIEW` would
/// define it, and one that is not there — or is not a view at all — where a `DROP VIEW` or
/// a `SHOW CREATE VIEW` addresses it.
pub const ERROR_TYPE_VIEW: i32 = 1502;

/// How many levels of view a statement may read through: a view over a view over a table
/// is resolved, and a chain longer than this is reported rather than followed.
const MAX_VIEW_DEPTH: usize = 4;

/// Why one statement did not run: the message Athena reports as both the execution's
/// `StateChangeReason` and its `AthenaError$ErrorMessage`, and the classification that
/// message belongs to.
#[derive(Debug, Clone)]
pub struct Failure {
    /// The message, as Athena words it for this failure class.
    pub message: String,
    /// Whose doing the failure was, and the error type Athena publishes for it.
    pub cause: FailureCause,
}

impl Failure {
    /// A failure this EMULATOR is the cause of: a bound it holds a read to, or a
    /// capability it does not have. Neither the caller's statement being wrong nor the
    /// service failing, which is what the third category names.
    pub fn emulator(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            cause: FailureCause::Emulator,
        }
    }

    /// A failure the submitted statement is the cause of, of no error class Athena
    /// publishes a type for.
    pub fn user(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            cause: FailureCause::Athena {
                category: ERROR_CATEGORY_USER,
                error_type: None,
            },
        }
    }

    /// A failure of one of Athena's published error classes.
    fn classified(message: String, category: i32, error_type: i32) -> Self {
        Self {
            message,
            cause: FailureCause::Athena {
                category,
                error_type: Some(error_type),
            },
        }
    }

    /// The statement names a table the data catalog does not hold.
    pub fn table_not_found(relation: &Relation) -> Self {
        Self::classified(
            format!(
                "TABLE_NOT_FOUND: line {}:{}: Table '{}.{}.{}' does not exist",
                relation.at.line,
                relation.at.column,
                relation.catalog.to_ascii_lowercase(),
                relation.database,
                relation.table
            ),
            ERROR_CATEGORY_USER,
            ERROR_TYPE_ENTITY_NOT_FOUND,
        )
    }

    /// The statement names a database the data catalog does not hold.
    pub fn schema_not_found(relation: &Relation) -> Self {
        Self::schema_not_found_at(relation.at, &relation.database)
    }

    /// The statement names a database the data catalog does not hold, where the database
    /// is named on its own rather than as part of a relation.
    pub fn schema_not_found_at(at: Position, database: &str) -> Self {
        Self::classified(
            format!(
                "SCHEMA_NOT_FOUND: line {}:{}: Schema '{database}' does not exist",
                at.line, at.column
            ),
            ERROR_CATEGORY_USER,
            ERROR_TYPE_ENTITY_NOT_FOUND,
        )
    }

    /// The statement would define a table the data catalog already holds. The catalog's
    /// own exception for that is what Athena's DDL engine surfaces, so it is that
    /// exception's name and sentence.
    pub fn table_already_exists() -> Self {
        Self::classified(
            format!(
                "AlreadyExistsException: {}",
                glue_state::messages::TABLE_ALREADY_EXISTS
            ),
            ERROR_CATEGORY_USER,
            ERROR_TYPE_ALREADY_EXISTS,
        )
    }

    /// A plain `CREATE VIEW` names a view that already exists.
    pub fn view_already_exists(target: &Relation) -> Self {
        Self::classified(
            format!(
                "line {}:{}: View already exists: '{}.{}'",
                target.at.line, target.at.column, target.database, target.table
            ),
            ERROR_CATEGORY_SYSTEM,
            ERROR_TYPE_VIEW,
        )
    }

    /// A statement addresses a view the catalog does not hold.
    pub fn view_not_found(target: &Relation) -> Self {
        Self::classified(
            format!(
                "line {}:{}: View '{}.{}' does not exist",
                target.at.line, target.at.column, target.database, target.table
            ),
            ERROR_CATEGORY_SYSTEM,
            ERROR_TYPE_VIEW,
        )
    }

    /// A statement that addresses a view names an entry that is a TABLE.
    pub fn not_a_view(target: &Relation) -> Self {
        Self::classified(
            format!(
                "line {}:{}: Relation '{}.{}' is a table, not a view",
                target.at.line, target.at.column, target.database, target.table
            ),
            ERROR_CATEGORY_SYSTEM,
            ERROR_TYPE_VIEW,
        )
    }

    /// The statement names a data catalog this account has not registered.
    pub fn catalog_not_found(at: Position, catalog: &str) -> Self {
        Self::user(format!(
            "CATALOG_NOT_FOUND: line {}:{}: Catalog '{}' does not exist",
            at.line,
            at.column,
            catalog.to_ascii_lowercase()
        ))
    }

    /// This failure as the outcome of a run that produced nothing.
    fn outcome(self) -> Outcome {
        Outcome::Failed {
            message: self.message,
            cause: self.cause,
        }
    }
}

/// Run the statement `reading` planned, and report what it produced.
///
/// The catalog and the object store are reached here rather than in the handler because
/// this is the one place that knows what the statement needs from each: a catalog
/// statement takes the catalog's own guard and writes or reads it; a statement over
/// relations resolves them in the catalog, releases that guard, reads the objects their
/// locations name, and only then hands the SQL to the engine.
///
/// BLOCKING, deliberately, exactly as [`query_engine::run`] is: the engine is
/// synchronous, so a caller runs this on a blocking pool.
pub fn execute<Eng: Send + Sync + 'static>(
    engine: &Eng,
    handle: &DatabaseHandle,
    sql: &str,
    reading: Reading,
    context: &QueryContext,
    parameters: &[String],
    now: SystemTime,
) -> Completion {
    match reading.plan {
        Plan::Engine => query_engine::run(handle, sql, reading.counts_rows, parameters),
        Plan::Catalog(statement) => {
            let started = Instant::now();
            let outcome = match perform(engine, &statement, now) {
                Ok(result) => Outcome::Succeeded {
                    result,
                    update_count: None,
                    data_scanned_bytes: 0,
                },
                Err(failure) => failure.outcome(),
            };
            Completion {
                outcome,
                engine_time: started.elapsed(),
            }
        }
        Plan::Relations(relational) => {
            let started = Instant::now();
            match resolved_sql(engine, handle, *relational, context) {
                Ok(prepared) => query_engine::run_over(
                    handle,
                    &prepared.objects,
                    &prepared.sql,
                    reading.counts_rows,
                    parameters,
                    prepared.scanned_bytes,
                ),
                Err(failure) => Completion {
                    outcome: failure.outcome(),
                    engine_time: started.elapsed(),
                },
            }
        }
    }
}

/// Perform one catalog statement against the Data Catalog.
fn perform<Eng: Send + Sync + 'static>(
    engine: &Eng,
    statement: &CatalogStatement,
    now: SystemTime,
) -> Result<StoredResultSet, Failure> {
    let (catalog, at) = match statement {
        CatalogStatement::CreateExternalTable(table) => {
            (table.target.catalog.clone(), table.target.at)
        }
        CatalogStatement::CreateView(view) => (view.target.catalog.clone(), view.target.at),
        CatalogStatement::DropView { target, .. }
        | CatalogStatement::ShowCreateView { target } => (target.catalog.clone(), target.at),
        CatalogStatement::ShowRelations { schema, .. } => (schema.catalog.clone(), schema.at),
    };
    let catalog_id = catalog_id_of(engine, &catalog, at)?;
    let Some(state) = glue_state::service_state(engine) else {
        return Err(Failure::emulator(CATALOG_UNREACHABLE));
    };
    let mut store = state.lock();
    match statement {
        CatalogStatement::CreateExternalTable(table) => {
            ddl::create_external_table(&mut store, &catalog_id, table, now)
        }
        CatalogStatement::CreateView(view) => {
            ddl::create_view(&mut store, &catalog_id, view, now)
        }
        CatalogStatement::DropView { target, if_exists } => {
            ddl::drop_view(&mut store, &catalog_id, target, *if_exists)
        }
        CatalogStatement::ShowCreateView { target } => {
            ddl::show_create_view(&store, &catalog_id, target)
        }
        CatalogStatement::ShowRelations {
            schema,
            views_only,
            filter,
        } => ddl::show_relations(&store, &catalog_id, schema, *views_only, filter.as_ref()),
    }
}

/// The message for a store this engine does not carry.
const CATALOG_UNREACHABLE: &str =
    "the Data Catalog is not reachable on this engine, so no statement can resolve a table";

/// The message for an object store this engine does not carry.
const OBJECT_STORE_UNREACHABLE: &str =
    "the object store is not reachable on this engine, so no table's data can be read";

/// The `CatalogId` the data catalog `catalog` names, or why its metadata cannot be read.
///
/// The SAME resolution Athena's metadata reads use: `AwsDataCatalog` is the account's own
/// Glue Data Catalog, a registered `GLUE` catalog naming that catalog id is that catalog
/// under a second name, and one whose metadata an external metastore or another account's
/// catalog serves is not one this emulator holds.
fn catalog_id_of<Eng: Send + Sync + 'static>(
    engine: &Eng,
    catalog: &str,
    at: Position,
) -> Result<String, Failure> {
    let Some(state) = crate::state::service_state(engine) else {
        return Err(Failure::emulator(
            "Athena state is not reachable on this engine",
        ));
    };
    // The registry's guard is taken and released here, before the catalog's own store is
    // reached, so no two stores are ever held at once.
    let source = {
        let store = state.lock();
        crate::state::metadata_source(&store.data_catalogs, catalog)
    };
    match source {
        crate::state::MetadataSource::GlueDataCatalog(catalog_id) => Ok(catalog_id),
        crate::state::MetadataSource::ExternalMetastore(catalog_type) => Err(Failure::emulator(
            crate::state::metastore_unreachable(catalog, &catalog_type),
        )),
        crate::state::MetadataSource::ForeignGlueCatalog(catalog_id) => Err(Failure::emulator(
            crate::state::foreign_glue_catalog(catalog, &catalog_id),
        )),
        crate::state::MetadataSource::Unregistered => Err(Failure::catalog_not_found(at, catalog)),
    }
}

/// A statement whose relations are resolved: the objects to materialise in the engine
/// database, the SQL that reads them, and the bytes the resolution read.
struct Prepared {
    /// The tables and views the statement reads, base tables first.
    objects: Vec<LocalObject>,
    /// The statement, with every resolved relation naming its materialised rows.
    sql: String,
    /// The bytes of object data this statement scanned.
    scanned_bytes: i64,
}

/// Resolve every relation one statement reads, and render the SQL the engine runs.
fn resolved_sql<Eng: Send + Sync + 'static>(
    engine: &Eng,
    handle: &DatabaseHandle,
    relational: RelationalStatement,
    context: &QueryContext,
) -> Result<Prepared, Failure> {
    let RelationalStatement {
        mut statement,
        relations,
    } = relational;
    // ONE budget for the whole statement: how many relations may be resolved at all
    // (which is what bounds the PRODUCT of the relations a statement names and the levels
    // of view they are defined over), how many rows and bytes may be materialised, and
    // when the resolution must stop whatever it has reached.
    let mut budget = Budget::default();
    let mut resolution = Resolution::default();
    let resolved = resolve(engine, &relations, 0, &mut resolution, &mut budget)?;

    // The catalog's guard is released before the object store's is taken, so no two
    // stores are ever held at once.
    let mut objects: Vec<LocalObject> = resolution
        .schema_tables
        .into_iter()
        .map(LocalObject::Table)
        .collect();
    let mut scanned_bytes = 0usize;
    if !resolution.sources.is_empty() {
        let Some(state) = s3_state::service_state(engine) else {
            return Err(Failure::emulator(OBJECT_STORE_UNREACHABLE));
        };
        let store = state.lock();
        for source in &resolution.sources {
            let (table, scanned) = source.read(&store, &mut budget)?;
            scanned_bytes = scanned_bytes.saturating_add(scanned);
            objects.push(LocalObject::Table(table));
        }
    }
    objects.extend(
        resolution
            .views
            .into_iter()
            .map(|(name, sql)| LocalObject::View { name, sql }),
    );

    // A relation the catalog holds nothing for may still be a table an earlier statement
    // defined in THIS engine database, which is where a query that defined its own table
    // reads it from; one that is neither is the catalog's own not-found refusal.
    let mut rewrites = resolved.rewrites;
    for (relation, miss) in resolved.unresolved {
        let local = relation.table.clone();
        if relation.database == glue_state::fold_name(&context.database)
            && query_engine::holds_relation(handle, &local)
        {
            rewrites.insert(relation.written.clone(), local);
            continue;
        }
        return Err(miss);
    }

    Ok(Prepared {
        sql: statement::rewritten(&mut statement, &rewrites),
        objects,
        scanned_bytes: i64::try_from(scanned_bytes).unwrap_or(i64::MAX),
    })
}

/// What resolving a statement's relations accumulated, across every level of view it
/// followed.
#[derive(Debug, Default)]
struct Resolution {
    /// The catalog tables to read, in the order they were resolved.
    sources: Vec<DataSource>,
    /// The catalog's own reporting tables the statement reads, already built.
    schema_tables: Vec<LocalTable>,
    /// The views to define over them, base-first.
    views: Vec<(String, String)>,
    /// The local names already resolved, ACROSS the whole statement, so a relation two
    /// branches of it share is resolved and read exactly once.
    resolved: HashSet<String>,
}

/// What resolving ONE list of relations established for the statement that names them.
#[derive(Debug, Default)]
struct Resolved {
    /// Which local name each written relation is rewritten to.
    rewrites: HashMap<String, String>,
    /// The relations the catalog holds nothing for, each with the refusal to report when
    /// nothing else resolves them either.
    unresolved: Vec<(Relation, Failure)>,
}

/// Resolve `relations` against the Data Catalog, following a view to the relations it
/// reads, and charging every relation against `budget`.
fn resolve<Eng: Send + Sync + 'static>(
    engine: &Eng,
    relations: &[Relation],
    depth: usize,
    resolution: &mut Resolution,
    budget: &mut Budget,
) -> Result<Resolved, Failure> {
    if depth > MAX_VIEW_DEPTH {
        return Err(Failure::emulator(format!(
            "a view this statement reads is defined over more than {MAX_VIEW_DEPTH} levels \
             of view, which this emulator does not expand"
        )));
    }
    let mut resolved = Resolved::default();
    for relation in relations {
        let local = relation.local_name();
        if resolution.resolved.contains(&local) {
            resolved
                .rewrites
                .insert(relation.written.clone(), local.clone());
            continue;
        }
        budget.charge_relation()?;
        let catalog_id = catalog_id_of(engine, &relation.catalog, relation.at)?;
        // What the catalog holds for this relation is read under its guard, and the guard
        // is released before anything else is reached.
        let held = {
            let Some(state) = glue_state::service_state(engine) else {
                return Err(Failure::emulator(CATALOG_UNREACHABLE));
            };
            let store = state.lock();
            if relation.is_information_schema() {
                Held::ReportingTable(reporting_table(&store, &catalog_id, relation, budget)?)
            } else {
                match store.database(&catalog_id, &relation.database) {
                    None => Held::NoDatabase,
                    Some(database) => match database.tables.get(&relation.table) {
                        None => Held::NoTable,
                        Some(table) if ddl::is_view(table) => Held::View(
                            table
                                .view_original_text
                                .as_deref()
                                .map(ddl::view_definition_sql)
                                .unwrap_or_default(),
                        ),
                        Some(table) => Held::Table(DataSource::of(relation, table)?),
                    },
                }
            }
        };
        match held {
            Held::ReportingTable(table) => {
                resolution.schema_tables.push(table);
                resolution.resolved.insert(local.clone());
                resolved.rewrites.insert(relation.written.clone(), local);
            }
            Held::Table(source) => {
                resolution.sources.push(source);
                resolution.resolved.insert(local.clone());
                resolved.rewrites.insert(relation.written.clone(), local);
            }
            Held::View(definition) => {
                resolution.resolved.insert(local.clone());
                resolved
                    .rewrites
                    .insert(relation.written.clone(), local.clone());
                let view_context = QueryContext {
                    catalog: relation.catalog.clone(),
                    database: relation.database.clone(),
                };
                let sql =
                    view_sql(engine, &definition, &view_context, depth, resolution, budget)?;
                resolution.views.push((local, sql));
            }
            Held::NoDatabase => resolved
                .unresolved
                .push((relation.clone(), Failure::schema_not_found(relation))),
            Held::NoTable => resolved
                .unresolved
                .push((relation.clone(), Failure::table_not_found(relation))),
        }
    }
    Ok(resolved)
}

/// What the catalog holds for one relation.
enum Held {
    /// A table whose data lives at a location.
    Table(DataSource),
    /// One of the catalog's own reporting tables, already built.
    ReportingTable(LocalTable),
    /// A view, carrying the query it is defined by.
    View(String),
    /// No database of that name.
    NoDatabase,
    /// No relation of that name in a database that is there.
    NoTable,
}

/// The SQL one view's rows come from: its own definition, with every relation IT reads
/// resolved and rewritten the same way the submitted statement's are.
fn view_sql<Eng: Send + Sync + 'static>(
    engine: &Eng,
    definition: &str,
    context: &QueryContext,
    depth: usize,
    resolution: &mut Resolution,
    budget: &mut Budget,
) -> Result<String, Failure> {
    let parsed = statement::parse_statements(definition)
        .and_then(|mut statements| (statements.len() == 1).then(|| statements.remove(0)));
    let Some(mut parsed) = parsed else {
        return Err(Failure::emulator(
            "a view this statement reads carries a definition that is not one SQL \
             statement, so its rows cannot be read",
        ));
    };
    let relations = statement::relations_of(&parsed, context);
    if relations.is_empty() {
        return Ok(statement::rewritten(&mut parsed, &HashMap::new()));
    }
    let resolved = resolve(engine, &relations, depth + 1, resolution, budget)?;
    // A view whose own source is missing cannot be read at all: the view is there, and
    // the relation under it is not.
    if let Some((_, miss)) = resolved.unresolved.into_iter().next() {
        return Err(miss);
    }
    Ok(statement::rewritten(&mut parsed, &resolved.rewrites))
}

/// One of the catalog's own reporting tables, as the rows it holds.
///
/// `information_schema` is how a SQL client asks the engine what the catalog holds, and
/// every row below is read off the catalog itself: the databases it holds, the relations
/// in each, what kind each relation is, and the columns each one declares.
fn reporting_table(
    store: &glue_state::ComAmazonawsGlueAwsglueState,
    catalog_id: &str,
    relation: &Relation,
    budget: &mut Budget,
) -> Result<LocalTable, Failure> {
    let catalog = relation.catalog.to_ascii_lowercase();
    let held = store.catalogs.get(catalog_id);
    let databases = held.map(glue_state::Catalog::databases_ordered).unwrap_or_default();
    let mut rows: Vec<Vec<::rusqlite::types::Value>> = Vec::new();
    let columns: Vec<&str> = match relation.table.as_str() {
        "schemata" => {
            for database in &databases {
                budget.charge_row(relation, SCHEMATA_COLUMNS)?;
                rows.push(vec![text(&catalog), text(&database.name)]);
            }
            vec!["catalog_name", "schema_name"]
        }
        "tables" => {
            for database in &databases {
                for table in database.tables_ordered() {
                    budget.charge_row(relation, TABLES_COLUMNS)?;
                    rows.push(vec![
                        text(&catalog),
                        text(&database.name),
                        text(&table.name),
                        text(if ddl::is_view(table) {
                            RELATION_TYPE_VIEW
                        } else {
                            RELATION_TYPE_BASE_TABLE
                        }),
                    ]);
                }
            }
            vec!["table_catalog", "table_schema", "table_name", "table_type"]
        }
        "views" => {
            for database in &databases {
                for table in database.tables_ordered() {
                    if !ddl::is_view(table) {
                        continue;
                    }
                    budget.charge_row(relation, VIEWS_COLUMNS)?;
                    rows.push(vec![
                        text(&catalog),
                        text(&database.name),
                        text(&table.name),
                        text(
                            &table
                                .view_original_text
                                .as_deref()
                                .map(ddl::view_definition_sql)
                                .unwrap_or_default(),
                        ),
                    ]);
                }
            }
            vec![
                "table_catalog",
                "table_schema",
                "table_name",
                "view_definition",
            ]
        }
        "columns" => {
            for database in &databases {
                for table in database.tables_ordered() {
                    for (position, column) in ddl::table_columns(table).iter().enumerate() {
                        budget.charge_row(relation, COLUMNS_COLUMNS)?;
                        rows.push(vec![
                            text(&catalog),
                            text(&database.name),
                            text(&table.name),
                            text(&column.name),
                            ::rusqlite::types::Value::Integer(
                                i64::try_from(position.saturating_add(1)).unwrap_or(i64::MAX),
                            ),
                            text(&column.data_type),
                            text(COLUMN_IS_NULLABLE),
                            column
                                .comment
                                .as_deref()
                                .map_or(::rusqlite::types::Value::Null, text),
                        ]);
                    }
                }
            }
            vec![
                "table_catalog",
                "table_schema",
                "table_name",
                "column_name",
                "ordinal_position",
                "data_type",
                "is_nullable",
                "comment",
            ]
        }
        _ => return Err(Failure::table_not_found(relation)),
    };
    Ok(LocalTable {
        name: relation.local_name(),
        columns: columns
            .into_iter()
            .map(|name| LocalColumn {
                name: name.to_string(),
                declared: data::engine_type(if name == "ordinal_position" {
                    "int"
                } else {
                    "string"
                }),
            })
            .collect(),
        rows,
    })
}

/// How many columns `information_schema.schemata` reports.
const SCHEMATA_COLUMNS: usize = 2;

/// How many columns `information_schema.tables` reports.
const TABLES_COLUMNS: usize = 4;

/// How many columns `information_schema.views` reports.
const VIEWS_COLUMNS: usize = 4;

/// How many columns `information_schema.columns` reports.
const COLUMNS_COLUMNS: usize = 8;

/// How `information_schema.tables` classifies a view.
const RELATION_TYPE_VIEW: &str = "VIEW";

/// How `information_schema.tables` classifies a table that holds data.
const RELATION_TYPE_BASE_TABLE: &str = "BASE TABLE";

/// What `information_schema.columns` reports for a column's nullability: the catalog
/// records none for a Hive column, and every such column admits a missing value.
const COLUMN_IS_NULLABLE: &str = "YES";

/// One text value, as the engine holds it.
fn text(value: &str) -> ::rusqlite::types::Value {
    ::rusqlite::types::Value::Text(value.to_string())
}
