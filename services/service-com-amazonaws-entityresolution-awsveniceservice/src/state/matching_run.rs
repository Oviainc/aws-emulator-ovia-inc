// SPDX-License-Identifier: CC0-1.0

//! What a matching job's RUN does: read the records of the workflow's input sources,
//! resolve them into match groups and write the matched records to its output source.
//!
//! `StartMatchingJob` "starts the MatchingJob of a workflow" (model:
//! `StartMatchingJob`), and the point of that job is the output table: once it reports
//! `SUCCEEDED`, the matched records are in the workflow's `outputS3Path`, each carrying
//! the `matchId` of the group it was resolved into, and `GetMatchingJob` reports the
//! `JobMetrics` of what was read and processed. A job is admitted FIRST and runs
//! afterwards — the start answers a `jobId` and the job reaches a terminal status on
//! its own — so a workflow whose input cannot be read is never a refusal at the door:
//! the job is admitted, runs, and ends `FAILED` carrying the diagnosis as its modelled
//! `ErrorDetails$errorMessage`. The clock decides WHEN the terminal status is reported
//! ([`super::MatchingJob::materialize_due`]); this module decides WHICH status it is,
//! by doing the work and answering the [`super::JobOutcome`] it reached. There is no
//! success to report for work that read nothing and wrote nothing.
//!
//! Two other emulated services own what a run touches, and each is reached where it is
//! owned:
//!
//! * the AWS Glue Data Catalog holds the TABLE every `InputSource$inputSourceARN`
//!   names, so a run resolves the table — where its data lives and what its columns
//!   are — out of Glue's own store on the shared engine, rather than keeping a second
//!   copy of the catalog;
//! * Amazon S3 holds the objects that table's location names and the output table the
//!   run writes, and it is reached the way the framework documents a handler reaching
//!   another emulated service ([`smithy_server_runtime::OperationContext::callback`]):
//!   an AWS SDK client over the emulator's OWN in-process callback, so nothing here
//!   opens a socket, resolves a host or reads a host credential. The object a client
//!   staged with `PutObject` is therefore the object a run reads, and the object a run
//!   writes is the one `ListObjectsV2` then lists. The one caller-supplied value that
//!   reaches the client's authority is the signing Region, narrowed to a Region label
//!   before it gets there, so no request can re-point this client at an address of its
//!   own choosing.
//!
//! Every bound on how much is listed, read, held and written is a constant below, and
//! each is applied before the allocation it caps. Nothing here parses a format by
//! hand: the comma-separated reading and writing is the `csv` crate's.

use std::collections::HashMap;

use ::service_com_amazonaws_glue_awsglue::state as glue_state;
use sha2::{Digest, Sha256};

use super::match_groups::{self, GroupAssignment, ResolvedRule, RunRecord};
use super::{
    ComAmazonawsEntityresolutionAwsveniceserviceState as State, IndexedRecord, JobOutcome,
    MatchingWorkflow,
};
use crate::ops::types::{JobMetrics, JobOutputSource, OutputAttribute};

/// The most objects one run reads under ONE input table's location.
///
/// AWS publishes no such limit; this is the emulator's own, and it is enforced against
/// the LISTING — a location holding more is the stated reason the run failed, never a
/// run that quietly matched a part of the table.
pub const MAX_OBJECTS_PER_SOURCE: usize = 64;

/// The most bytes one run reads from ONE object of an input table. CHOSEN for this
/// emulator.
pub const MAX_OBJECT_BYTES: usize = 8 * 1024 * 1024;

/// The most bytes one run reads in total, across every object of every input source:
/// the bound on the PRODUCT of the object count and the object size, which neither
/// alone caps. CHOSEN for this emulator.
pub const MAX_RUN_INPUT_BYTES: usize = 32 * 1024 * 1024;

/// The most records one run holds, across every input source. CHOSEN for this
/// emulator, and applied as each record is read.
pub const MAX_RUN_RECORDS: usize = 20_000;

/// The most columns one input table is read with.
///
/// A schema mapping maps at most 60 attributes (`SchemaInputAttributes` is
/// `length 2..60`, `smithy_query(query="shape",shape="SchemaInputAttributes")`) and an
/// input table may carry columns the schema does not map — those are still written to
/// the output table — so this is wider than the modelled schema and still bounded.
pub const MAX_COLUMNS: usize = 200;

/// The most bytes one run encodes its output table to. CHOSEN for this emulator.
pub const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;

/// The delimiter an input table and the output table are read and written with.
///
/// A run reads the comma-separated form, which is what a Glue table over
/// comma-separated objects carries; a table stored in another format is the stated
/// reason the run failed rather than a table read as something it is not.
const CSV_DELIMITER: u8 = b',';

/// The Hive SerDe parameter that says how many leading lines of each object are a
/// header rather than data, as a Glue table over comma-separated objects declares it.
const SKIP_HEADER_PARAMETER: &str = "skip.header.line.count";

/// The most leading lines of one object this run will skip as a header: a parameter
/// value past this is read as naming no header at all, so a table definition cannot
/// make a run walk an unbounded number of records.
const MAX_SKIPPED_HEADER_LINES: usize = 16;

/// The output table's column carrying `Record$inputSourceARN`: which input source a
/// matched record came from.
const INPUT_SOURCE_COLUMN: &str = "InputSourceARN";

/// The output table's column carrying the `matchId` of the group a record was resolved
/// into.
const MATCH_ID_COLUMN: &str = "MatchID";

/// The output table's column carrying the `ruleName` of the rule that joined a record
/// to its group — empty for a record no rule joined to another.
const MATCH_RULE_COLUMN: &str = "MatchRule";

/// The name of the object one run writes its output table as, under the per-run prefix
/// [`output_key`] builds.
const OUTPUT_PART_NAME: &str = "part-00000.csv";

/// What a run needs in order to be performed, copied out of the store under its lock so
/// nothing is read from the store while Glue or Amazon S3 is being called.
#[derive(Debug, Clone)]
pub enum RunPlan {
    /// Everything the run reads, matches on and writes.
    Ready(Box<ReadyRun>),
    /// The run cannot be performed and the reason is already settled, without reading
    /// anything: the workflow matches in a way this emulator does not run, names an
    /// input this emulator does not read, or has nowhere to write.
    Unperformable(String),
}

/// A run that can be attempted: which tables it reads, which rules it matches on and
/// where the matched records go.
#[derive(Debug, Clone)]
pub struct ReadyRun {
    /// The workflow being run, which is part of the output table's prefix.
    pub workflow_name: String,
    /// The `jobId` of the run, which is the rest of that prefix: two runs of one
    /// workflow therefore write two output tables rather than overwriting each other.
    pub job_id: String,
    /// The workflow's input sources, in the order it declares them.
    pub sources: Vec<SourceRead>,
    /// The workflow's rules with each `matchingKey` already resolved to the schema
    /// fields it compares.
    pub rules: Vec<ResolvedRule>,
    /// Where the matched records go: the job's own `outputSourceConfig` entry.
    pub output: JobOutputSource,
    /// The columns of the output table that come from the records themselves
    /// (`OutputSource$output`), each written as itself or hashed as that attribute
    /// asks.
    pub output_attributes: Vec<OutputAttribute>,
}

/// One input source of a run, reduced to the Glue table it names.
#[derive(Debug, Clone)]
pub struct SourceRead {
    /// `InputSource$inputSourceARN`, as the workflow carries it: reported on every
    /// record read from this source.
    pub input_source_arn: String,
    /// The `CatalogId` the table's ARN names, resolved through Glue's own default rule.
    pub catalog_id: String,
    /// The database segment of the table ARN.
    pub database: String,
    /// The table segment of the table ARN.
    pub table: String,
    /// `InputSource$applyNormalization` for this source.
    pub normalize: bool,
    /// The `fieldName` of the schema mapping's `UNIQUE_ID` attribute, which is the
    /// column carrying each record's identity inside the workflow; `None` when the
    /// schema names no such attribute.
    pub unique_id_field: Option<String>,
}

/// One input table as the catalog describes it: where its objects are and what its
/// columns are called.
#[derive(Debug, Clone)]
struct TableRead {
    input_source_arn: String,
    normalize: bool,
    unique_id_field: Option<String>,
    bucket: String,
    /// The key prefix the table's `StorageDescriptor$Location` names.
    prefix: String,
    /// The table's own column names, in catalog order; empty when the stored table
    /// declares none.
    columns: Vec<String>,
    /// How many leading records of each object are a header
    /// ([`SKIP_HEADER_PARAMETER`]).
    skip_header_lines: usize,
}

/// What `workflow` would run with, read out of `store`.
///
/// Decided entirely from the workflow and the schema mappings its input sources name,
/// so nothing outside this service is touched while its store is locked.
pub fn plan(store: &State, workflow: &MatchingWorkflow, job_id: &str) -> RunPlan {
    // "RULE_MATCHING: match records using configurable rule-based criteria"
    // (`smithy_query(query="shape",shape="ResolutionTechniques")`). The other two
    // resolution types match with a machine-learning model or a third-party provider
    // service, neither of which runs here, so a workflow configured for one is stated
    // as the reason its run could not be performed.
    if workflow.resolution_techniques.resolution_type != super::RULE_MATCHING {
        return RunPlan::Unperformable(format!(
            "The workflow resolves with {}, which this emulator does not run; it runs \
             {} workflows.",
            workflow.resolution_techniques.resolution_type,
            super::RULE_MATCHING
        ));
    }
    // A rule-based workflow carries its rules either as `ruleBasedProperties` — a list
    // of matching keys, which is what this run compares — or as
    // `ruleConditionProperties`, "the ability to use more complex syntax"
    // (`smithy_query(query="shape",shape="RuleConditionProperties")`), an expression
    // language nothing here evaluates.
    if workflow.resolution_techniques.rule_based_properties.is_none()
        && workflow
            .resolution_techniques
            .rule_condition_properties
            .is_some()
    {
        return RunPlan::Unperformable(
            "The workflow matches with rule conditions, which this emulator does not \
             evaluate; it matches with the matching keys of ruleBasedProperties."
                .to_string(),
        );
    }

    // Where the matched records go. `JobOutputSourceConfig` is `length 1..1`, and a
    // workflow whose output source names no S3 path yields none at all
    // ([`super::job_output_source_config`]), so such a run has nowhere to write.
    let Some(output) = super::job_output_source_config(workflow)
        .and_then(|sources| sources.into_iter().next())
    else {
        return RunPlan::Unperformable(
            "The workflow's output source names no S3 path, so the run has nowhere to \
             write its matched records."
                .to_string(),
        );
    };
    if s3_location(&output.output_s3_path).is_none() {
        return RunPlan::Unperformable(format!(
            "The workflow's output path {} does not name an S3 location.",
            output.output_s3_path
        ));
    }

    let mut sources: Vec<SourceRead> = Vec::new();
    for source in &workflow.input_source_config {
        // `InputSourceARN` admits an ID namespace and a matching workflow beside a Glue
        // table (`smithy_query(query="shape",shape="InputSource")`); a batch run reads
        // "an Glue table Amazon Resource Name (ARN) for the input source table", so an
        // ARN of either other kind is stated rather than read as a table.
        let Some((account, database, table)) = glue_table_arn_parts(&source.input_source_arn)
        else {
            return RunPlan::Unperformable(format!(
                "The input source {} does not name an AWS Glue table, which is what a \
                 matching job reads.",
                source.input_source_arn
            ));
        };
        sources.push(SourceRead {
            input_source_arn: source.input_source_arn.clone(),
            catalog_id: glue_state::resolve_catalog_id(Some(&account)),
            database,
            table,
            normalize: source.apply_normalization.unwrap_or(false),
            // "`UNIQUE_ID`" is the `SchemaAttributeType` of the attribute that
            // identifies a record (`smithy_query(query="shape",shape="SchemaInputAttribute")`),
            // so the column of that name carries each record's `uniqueId`.
            unique_id_field: store
                .schema_mappings
                .get(&source.schema_name)
                .and_then(|schema| {
                    schema
                        .mapped_input_fields
                        .iter()
                        .find(|attribute| attribute.r#type == UNIQUE_ID_ATTRIBUTE_TYPE)
                        .map(|attribute| attribute.field_name.clone())
                }),
        });
    }

    // Each rule's matching keys are resolved to schema fields ONCE here, under the
    // store's lock, by the same resolution the real-time pair uses — so a batch run and
    // `GenerateMatchId` compare the same fields for the same key.
    let rules: Vec<ResolvedRule> = super::workflow_rules(workflow)
        .iter()
        .map(|rule| ResolvedRule {
            rule_name: rule.rule_name.clone(),
            fields: store.rule_fields(workflow, rule),
        })
        .collect();

    RunPlan::Ready(Box::new(ReadyRun {
        workflow_name: workflow.workflow_name.clone(),
        job_id: job_id.to_string(),
        sources,
        rules,
        output,
        output_attributes: workflow
            .output_source_config
            .first()
            .map(|source| source.output.clone())
            .unwrap_or_default(),
    }))
}

/// The `SchemaAttributeType` of the attribute that identifies a record inside its
/// workflow (`smithy_query(query="shape",shape="SchemaAttributeType")` lists it among
/// the attribute types), i.e. the column a record's `uniqueId` is read from.
const UNIQUE_ID_ATTRIBUTE_TYPE: &str = "UNIQUE_ID";

/// What one run produced: the terminal state it reached, and the records a successful
/// run resolved, ready for the workflow's match index.
#[derive(Debug, Clone)]
pub struct RunResult {
    /// The terminal status, metrics and failure reason the run reached.
    pub outcome: JobOutcome,
    /// The records the run resolved, each carrying the `matchId` of its group and the
    /// `ruleName` that joined it there — empty for a run that resolved none.
    ///
    /// `GetMatchId` "returns the corresponding Match ID of a customer record if the
    /// record has been processed in a rule-based matching workflow"
    /// (`smithy_query(query="doc",shape="GetMatchId")`), and a batch run is how a record
    /// gets processed in one, so what a finished run resolved is readable through that
    /// operation as well as from the output table.
    pub indexed: Vec<IndexedRecord>,
}

/// Perform `plan`: read every input table, resolve the records into match groups, write
/// the output table — and answer the terminal state that reached, with the records it
/// resolved.
///
/// Awaited with NO store guard held: the catalog read and the object reads and writes
/// go through the shared engine and the in-process callback, and the result is
/// recorded against the job afterwards.
pub async fn perform<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    plan: &RunPlan,
) -> RunResult {
    let run = match plan {
        RunPlan::Ready(run) => run,
        RunPlan::Unperformable(reason) => return failed(reason.clone()),
    };
    let tables = match input_tables(engine, run) {
        Ok(tables) => tables,
        Err(reason) => return failed(reason),
    };

    let client = s3_client(ctx, region);
    let mut records: Vec<RunRecord> = Vec::new();
    // What the modelled `JobMetrics` counts: "the total number of input records", "the
    // total number of records processed" and "the total number of records that did not
    // get processed" (`smithy_query(query="shape",shape="JobMetrics")`). A row this run
    // could not read as a record of its table is counted as not processed, which is
    // what distinguishes the two.
    let mut input_records = 0usize;
    let mut not_processed = 0usize;
    let mut read_bytes = 0usize;
    for table in &tables {
        let keys = match object_keys(&client, table).await {
            Ok(keys) => keys,
            Err(reason) => return failed(reason),
        };
        for key in keys {
            let bytes = match read_object(&client, &table.bucket, &key).await {
                Ok(bytes) => bytes,
                Err(reason) => return failed(reason),
            };
            read_bytes = read_bytes.saturating_add(bytes.len());
            if read_bytes > MAX_RUN_INPUT_BYTES {
                return failed(run_input_too_large());
            }
            match read_table_records(&bytes, table, &mut records) {
                Ok(counted) => {
                    input_records = input_records.saturating_add(counted.rows);
                    not_processed = not_processed.saturating_add(counted.not_processed);
                }
                Err(reason) => return failed(reason),
            }
        }
    }

    // The matching itself: one `matchId` per resolved group, and the rule that joined
    // each record to its group.
    let assignments = match_groups::resolve(&records, &run.rules, super::new_match_id);
    let document = match output_document(&records, &assignments, &run.output_attributes) {
        Ok(document) => document,
        Err(reason) => return failed(reason),
    };
    let Some((bucket, prefix)) = s3_location(&run.output.output_s3_path) else {
        return failed(format!(
            "The job's output path {} does not name an S3 location.",
            run.output.output_s3_path
        ));
    };
    let key = output_key(&prefix, &run.workflow_name, &run.job_id);
    if let Err(reason) = write_object(&client, &bucket, &key, document).await {
        return failed(reason);
    }

    // Counted from what was actually read and written: the records of the input
    // tables, the ones that could not be read as records, and the groups the rest were
    // resolved into.
    let matched_groups = assignments
        .iter()
        .map(|assignment| assignment.match_id.as_str())
        .collect::<::std::collections::BTreeSet<_>>()
        .len();
    RunResult {
        outcome: JobOutcome::succeeded(JobMetrics {
            input_records: counted(input_records),
            total_records_processed: counted(records.len()),
            records_not_processed: counted(not_processed),
            // "The number of records processed that were marked for deletion (DELETE =
            // True) in the input file": this run reads no such marker, so it counts none.
            delete_records_processed: None,
            match_i_ds: counted(matched_groups),
        }),
        indexed: indexed_records(&records, &assignments),
    }
}

/// The result of a run that could not be performed: `FAILED` with `reason`, and no
/// record to index — nothing was resolved.
fn failed(reason: String) -> RunResult {
    RunResult {
        outcome: JobOutcome::failed(reason),
        indexed: Vec::new(),
    }
}

/// The records a finished run resolved, as the workflow's match index holds one.
///
/// A record the schema gives no `uniqueId` has no identity to be held under, so it is
/// left out of the index; its row of the output table still carries its group. A
/// `uniqueId` that appears twice in the input is ONE record of the workflow, so the last
/// row read for it is the one indexed — the same replacement rule `GenerateMatchId`
/// applies when it re-indexes an id.
fn indexed_records(records: &[RunRecord], assignments: &[GroupAssignment]) -> Vec<IndexedRecord> {
    records
        .iter()
        .zip(assignments)
        .filter_map(|(record, assignment)| {
            let unique_id = record
                .unique_id
                .clone()
                .filter(|unique_id| !unique_id.is_empty())?;
            Some(IndexedRecord {
                input_source_arn: record.input_source_arn.clone(),
                unique_id,
                record_attribute_map: record.attributes.clone(),
                match_id: assignment.match_id.clone(),
                match_rule: assignment.match_rule.clone(),
            })
        })
        .collect()
}

/// A count as the modelled `JobMetrics` carries it. Every count is bounded by
/// [`MAX_RUN_RECORDS`], so the conversion cannot fail; it is written without a panic so
/// a run stays total.
fn counted(value: usize) -> Option<i32> {
    i32::try_from(value).ok()
}

/// The `(account, database, table)` a Glue table ARN names, or `None` for a value that
/// is not one.
///
/// The ARN is read through the shared parse rather than split again here, so this
/// service and every other agree about what an ARN is.
pub fn glue_table_arn_parts(arn: &str) -> Option<(String, String, String)> {
    let parsed = ::service_common::arn::parse(arn)?;
    if parsed.vendor != "glue" {
        return None;
    }
    let (database, table) = parsed.resource.strip_prefix("table/")?.split_once('/')?;
    (!database.is_empty() && !table.is_empty()).then(|| {
        (
            parsed.account.to_string(),
            database.to_string(),
            table.to_string(),
        )
    })
}

/// The `(bucket, key)` an `s3://bucket/key` path names, or `None` for a value that is
/// not one. The key is empty for a path naming only a bucket.
fn s3_location(path: &str) -> Option<(String, String)> {
    let rest = path.strip_prefix("s3://")?;
    let (bucket, key) = rest.split_once('/').unwrap_or((rest, ""));
    (!bucket.is_empty()).then(|| (bucket.to_string(), key.to_string()))
}

/// The key the output table is written under: the output path's own key as a prefix,
/// then this WORKFLOW and this JOB, then the part name.
///
/// `outputS3Path` is "the S3 path to which Entity Resolution will write the output
/// table" (`smithy_query(query="shape",shape="JobOutputSource")`), i.e. a prefix the run
/// writes its table under rather than an object name of its own. The run's own two
/// segments are CHOSEN for this emulator — the model fixes no layout — and they are what
/// keeps one run's output from replacing another's: a job reports where THAT run wrote.
fn output_key(prefix: &str, workflow_name: &str, job_id: &str) -> String {
    let mut key = prefix.to_string();
    if !key.is_empty() && !key.ends_with('/') {
        key.push('/');
    }
    key.push_str(workflow_name);
    key.push('/');
    key.push_str(job_id);
    key.push('/');
    key.push_str(OUTPUT_PART_NAME);
    key
}

/// Resolve every input source of `run` against the Glue Data Catalog on the shared
/// engine, or answer the reason the run could not be performed.
///
/// The catalog's lock is taken and released here: nothing is awaited while it is held.
fn input_tables<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    run: &ReadyRun,
) -> Result<Vec<TableRead>, String> {
    let Some(catalog) = glue_state::service_state(engine) else {
        return Err(
            "The AWS Glue Data Catalog, which holds the workflow's input tables, is not \
             reachable on this emulator."
                .to_string(),
        );
    };
    let catalog = catalog.lock();
    let mut tables: Vec<TableRead> = Vec::new();
    for source in &run.sources {
        let Some(database) = catalog.database(&source.catalog_id, &source.database) else {
            return Err(table_not_found(&source.input_source_arn));
        };
        // The catalog keys a table by its folded name and folds a requested name the
        // same way, so the table is reached by the casing the workflow names.
        let Some(table) = database.tables.get(&glue_state::fold_name(&source.table)) else {
            return Err(table_not_found(&source.input_source_arn));
        };
        let Some(storage) = table.storage_descriptor.as_ref() else {
            return Err(table_has_no_location(&source.input_source_arn));
        };
        let Some(location) = storage.location.as_deref().filter(|path| !path.is_empty()) else {
            return Err(table_has_no_location(&source.input_source_arn));
        };
        let Some((bucket, prefix)) = s3_location(location) else {
            return Err(format!(
                "The input table {} stores its data at {location}, which is not an Amazon \
                 S3 location; a matching job reads its records from Amazon S3.",
                source.input_source_arn
            ));
        };
        let columns: Vec<String> = storage
            .columns
            .iter()
            .flatten()
            .map(|column| column.name.clone())
            .collect();
        if columns.len() > MAX_COLUMNS {
            return Err(too_many_columns(columns.len()));
        }
        tables.push(TableRead {
            input_source_arn: source.input_source_arn.clone(),
            normalize: source.normalize,
            unique_id_field: source.unique_id_field.clone(),
            bucket,
            prefix,
            columns,
            skip_header_lines: skipped_header_lines(storage),
        });
    }
    Ok(tables)
}

/// How many leading records of each of a table's objects are a header, as its SerDe
/// declares with [`SKIP_HEADER_PARAMETER`].
///
/// A value that is not a number, or one past [`MAX_SKIPPED_HEADER_LINES`], names no
/// header: a table definition cannot make a run skip an unbounded number of records.
fn skipped_header_lines(
    storage: &::service_com_amazonaws_glue_awsglue::ops::types::StorageDescriptor,
) -> usize {
    storage
        .serde_info
        .as_ref()
        .and_then(|serde| serde.parameters.as_ref())
        .and_then(|parameters| parameters.get(SKIP_HEADER_PARAMETER))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|lines| *lines <= MAX_SKIPPED_HEADER_LINES)
        .unwrap_or(0)
}

/// The keys of the objects one input table's location holds, as `ListObjectsV2`
/// answered.
///
/// Bounded by the REQUEST: at most [`MAX_OBJECTS_PER_SOURCE`] keys are asked for, and a
/// listing Amazon S3 reports as truncated is the stated reason the run failed rather
/// than a run that matched a part of the table. A "folder" marker — a key ending in
/// `/`, which carries no records — is left out.
async fn object_keys(
    client: &::aws_sdk_s3::Client,
    table: &TableRead,
) -> Result<Vec<String>, String> {
    let keys = i32::try_from(MAX_OBJECTS_PER_SOURCE).unwrap_or(i32::MAX);
    let listed = client
        .list_objects_v2()
        .bucket(&table.bucket)
        .prefix(&table.prefix)
        .max_keys(keys)
        .send()
        .await
        .map_err(|error| {
            // The caller's own values are rendered in their escaped `Debug` form, so a
            // bucket or key carrying a newline cannot forge a log line.
            ::tracing::debug!(
                bucket = ?table.bucket,
                prefix = ?table.prefix,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "an Entity Resolution matching job could not list its input table's objects"
            );
            format!(
                "The input table {}'s objects at s3://{}/{} could not be listed.",
                table.input_source_arn, table.bucket, table.prefix
            )
        })?;
    if listed.is_truncated() == Some(true) {
        return Err(format!(
            "The input table {} holds more than the {MAX_OBJECTS_PER_SOURCE} objects this \
             emulator reads in one job run.",
            table.input_source_arn
        ));
    }
    Ok(listed
        .contents()
        .iter()
        .filter_map(|object| object.key())
        .filter(|key| !key.ends_with('/') && selects(key, &table.prefix))
        .map(ToString::to_string)
        .collect())
}

/// Whether `key` is one of the objects `prefix` names: the object AT that prefix, or an
/// object under it as a directory.
///
/// A table's `StorageDescriptor$Location` names a directory or a single object, and
/// Amazon S3's prefix listing would also answer a SIBLING whose name merely starts with
/// the same characters, so the boundary is checked here.
fn selects(key: &str, prefix: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    match key.strip_prefix(prefix) {
        None => false,
        Some(rest) => rest.is_empty() || prefix.ends_with('/') || rest.starts_with('/'),
    }
}

/// The bytes of one object of an input table, or the reason the run could not read it.
///
/// [`MAX_OBJECT_BYTES`] is enforced by the REQUEST — the read asks for at most the
/// ceiling plus the one byte that shows the object is past it — and again against the
/// length the answer declares and the bytes that arrive, so an answer that declared no
/// length is bounded too.
async fn read_object(
    client: &::aws_sdk_s3::Client,
    bucket: &str,
    key: &str,
) -> Result<Vec<u8>, String> {
    let read = client
        .get_object()
        .bucket(bucket)
        .key(key)
        .range(format!("bytes=0-{MAX_OBJECT_BYTES}"))
        .send()
        .await
        .map_err(|error| {
            ::tracing::debug!(
                bucket = ?bucket,
                key = ?key,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "an Entity Resolution matching job could not read an input object"
            );
            format!("The input object s3://{bucket}/{key} could not be read.")
        })?;
    if let Some(declared) = read
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
    {
        if declared > MAX_OBJECT_BYTES {
            return Err(object_too_large(bucket, key, declared));
        }
    }
    let body = read
        .body
        .collect()
        .await
        .map_err(|error| {
            format!("The input object s3://{bucket}/{key} could not be read in full: {error}")
        })?
        .into_bytes();
    if body.len() > MAX_OBJECT_BYTES {
        return Err(object_too_large(bucket, key, body.len()));
    }
    Ok(body.to_vec())
}

/// Write the output table as one object, or answer the reason the run could not.
async fn write_object(
    client: &::aws_sdk_s3::Client,
    bucket: &str,
    key: &str,
    document: Vec<u8>,
) -> Result<(), String> {
    client
        .put_object()
        .bucket(bucket)
        .key(key)
        .body(::aws_sdk_s3::primitives::ByteStream::from(document))
        .send()
        .await
        .map_err(|error| {
            ::tracing::debug!(
                bucket = ?bucket,
                key = ?key,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "an Entity Resolution matching job could not write its output table"
            );
            format!("The job's output table could not be written to s3://{bucket}/{key}.")
        })?;
    Ok(())
}

/// How many rows one object contributed, and how many of them could not be read as
/// records of their table.
#[derive(Debug, Clone, Copy, Default)]
struct RowCounts {
    rows: usize,
    not_processed: usize,
}

/// Read the records one object of `table` holds into `records`, and count the rows.
///
/// The column NAMES a record's attributes are keyed by are the table's own, in catalog
/// order; a table that declares none is read with the object's first record as its
/// header, which is what a comma-separated export carries. A record whose field count
/// does not match the column list is counted as NOT PROCESSED rather than matched on
/// guessed positions.
fn read_table_records(
    bytes: &[u8],
    table: &TableRead,
    records: &mut Vec<RunRecord>,
) -> Result<RowCounts, String> {
    let mut reader = ::csv::ReaderBuilder::new()
        .delimiter(CSV_DELIMITER)
        // The header is decided below, from the table definition rather than from the
        // reader's own first-record rule.
        .has_headers(false)
        // A ragged record is the caller's object rather than a parse failure: it is
        // counted as not processed below.
        .flexible(true)
        .from_reader(bytes);

    let mut columns = table.columns.clone();
    let mut counts = RowCounts::default();
    let mut skipped = 0usize;
    let mut first = true;
    for row in reader.records() {
        let row = row.map_err(|error| {
            format!(
                "The input table {}'s records could not be read: {error}",
                table.input_source_arn
            )
        })?;
        if skipped < table.skip_header_lines {
            skipped += 1;
            continue;
        }
        if columns.is_empty() {
            if row.len() > MAX_COLUMNS {
                return Err(too_many_columns(row.len()));
            }
            columns = row.iter().map(ToString::to_string).collect();
            first = false;
            continue;
        }
        // A table that declares its columns AND carries them as the first record of the
        // object (the usual comma-separated export) is read as data from the second
        // record on, so the header is never matched as a record of its own.
        if first && table.skip_header_lines == 0 && is_header_row(&row, &columns) {
            first = false;
            continue;
        }
        first = false;
        counts.rows = counts.rows.saturating_add(1);
        if row.len() != columns.len() {
            counts.not_processed = counts.not_processed.saturating_add(1);
            continue;
        }
        if records.len() == MAX_RUN_RECORDS {
            return Err(too_many_records());
        }
        let attributes: HashMap<String, String> = columns
            .iter()
            .cloned()
            .zip(row.iter().map(ToString::to_string))
            .collect();
        records.push(RunRecord {
            input_source_arn: table.input_source_arn.clone(),
            unique_id: table
                .unique_id_field
                .as_ref()
                .and_then(|field| attributes.get(field).cloned()),
            normalize: table.normalize,
            attributes,
        });
    }
    Ok(counts)
}

/// Whether `row` is the header of a table whose columns are `columns`: the same names,
/// in the same order, compared without regard to case or surrounding whitespace — which
/// is how a Hive-style catalog relates a lowercased column name to the header a
/// comma-separated export wrote.
fn is_header_row(row: &::csv::StringRecord, columns: &[String]) -> bool {
    row.len() == columns.len()
        && row.iter().zip(columns).all(|(cell, column)| {
            cell.trim().eq_ignore_ascii_case(column.trim())
        })
}

/// The output table: the header, then one record per input record with the `matchId` of
/// the group it was resolved into.
///
/// Each `OutputAttribute` is "a column to be included in the output table, and whether
/// the values of the column should be hashed"
/// (`smithy_query(query="shape",shape="OutputSource")`), so a column the attribute
/// marks hashed carries the digest of the value rather than the value. Beside them come
/// the three columns the run itself determines: which input source the record came
/// from, the group it belongs to and the rule that joined it there.
///
/// Comma-separated, written by the `csv` crate — the model fixes no format for the
/// output table, and this is the form a matching job reads its own input tables in, so
/// one run's output table is readable as another workflow's input.
fn output_document(
    records: &[RunRecord],
    assignments: &[GroupAssignment],
    attributes: &[OutputAttribute],
) -> Result<Vec<u8>, String> {
    let mut writer = ::csv::WriterBuilder::new()
        .delimiter(CSV_DELIMITER)
        .from_writer(Vec::new());
    let mut header: Vec<&str> = attributes
        .iter()
        .map(|attribute| attribute.name.as_str())
        .collect();
    header.extend([INPUT_SOURCE_COLUMN, MATCH_ID_COLUMN, MATCH_RULE_COLUMN]);
    writer
        .write_record(&header)
        .map_err(|error| format!("The job's output table header could not be written: {error}"))?;
    for (record, assignment) in records.iter().zip(assignments) {
        let mut row: Vec<String> = attributes
            .iter()
            .map(|attribute| {
                let value = record
                    .attributes
                    .get(&attribute.name)
                    .cloned()
                    .unwrap_or_default();
                if attribute.hashed == Some(true) {
                    hashed(&value)
                } else {
                    value
                }
            })
            .collect();
        row.push(record.input_source_arn.clone());
        row.push(assignment.match_id.clone());
        row.push(assignment.match_rule.clone());
        writer.write_record(&row).map_err(|error| {
            format!("The job's output table records could not be written: {error}")
        })?;
        // The encoded size is checked as it grows rather than after the whole document
        // is built, so the bound caps what is allocated.
        if writer.get_ref().len() > MAX_OUTPUT_BYTES {
            return Err(output_too_large());
        }
    }
    let document = writer
        .into_inner()
        .map_err(|error| format!("The job's output table could not be completed: {error}"))?;
    if document.len() > MAX_OUTPUT_BYTES {
        return Err(output_too_large());
    }
    Ok(document)
}

/// One value as a column the output asks to hash carries it: the hex SHA-256 of the
/// value, from the `sha2` crate this crate already depends on — nothing here
/// hand-rolls a digest.
fn hashed(value: &str) -> String {
    ::hex::encode(Sha256::digest(value.as_bytes()))
}

/// The reason a run fails for an input table the catalog does not hold.
fn table_not_found(input_source_arn: &str) -> String {
    format!(
        "The input table {input_source_arn} is not held by the AWS Glue Data Catalog, so the \
         run has no records to read."
    )
}

/// The reason a run fails for an input table the catalog holds without a location.
fn table_has_no_location(input_source_arn: &str) -> String {
    format!(
        "The input table {input_source_arn} names no storage location, so the run has no \
         records to read."
    )
}

/// The reason a run fails for an object larger than [`MAX_OBJECT_BYTES`].
fn object_too_large(bucket: &str, key: &str, declared: usize) -> String {
    format!(
        "The input object s3://{bucket}/{key} is {declared} bytes, larger than the \
         {MAX_OBJECT_BYTES} bytes this emulator reads from one object."
    )
}

/// The reason a run fails for reading more than [`MAX_RUN_INPUT_BYTES`] in total.
fn run_input_too_large() -> String {
    format!(
        "The workflow's input sources hold more than the {MAX_RUN_INPUT_BYTES} bytes this \
         emulator reads in one job run."
    )
}

/// The reason a run fails for more records than [`MAX_RUN_RECORDS`].
fn too_many_records() -> String {
    format!(
        "The workflow's input sources hold more than the {MAX_RUN_RECORDS} records this \
         emulator matches in one job run."
    )
}

/// The reason a run fails for a table wider than [`MAX_COLUMNS`].
fn too_many_columns(held: usize) -> String {
    format!(
        "An input table holds {held} columns, more than the {MAX_COLUMNS} this emulator reads \
         in one job run."
    )
}

/// The reason a run fails for an output table larger than [`MAX_OUTPUT_BYTES`].
fn output_too_large() -> String {
    format!(
        "The job's output table is larger than the {MAX_OUTPUT_BYTES} bytes this emulator \
         writes in one job run."
    )
}

/// The Amazon S3 client a run reads its input tables and writes its output table with,
/// addressed to `region`.
///
/// The transport is the emulator's own in-process callback, so the `endpoint_url` below
/// is an ADDRESS the in-process router reads rather than a host any resolver looks up:
/// this client reaches the emulator's own Amazon S3 and nothing else, whatever bucket or
/// key a caller names. The Region is narrowed to a Region label before it reaches the
/// authority.
///
/// There is nothing to retry, time out or watch for a stalled stream, each of which
/// would need a sleep implementation this client is not given. The same goes for caching
/// the identity: the credentials the callback hands over are used directly, and they
/// only sign this operation's in-process calls.
fn s3_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let region = ::service_common::arn::region_label(region).unwrap_or(super::DEFAULT_REGION);
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The emulator serves one endpoint for every service, so the bucket travels in
        // the path rather than in a per-bucket host the router could not resolve.
        .force_path_style(true)
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}
