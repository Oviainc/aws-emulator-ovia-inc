// SPDX-License-Identifier: CC0-1.0

//! `CreateMLTransform`, `GetMLTransforms` and `ListMLTransforms`: the machine learning
//! transforms this account holds.
//!
//! RULE: `GetMLTransforms` "gets a sortable, filterable list of existing Glue machine
//! learning transforms" — it reports every transform the store holds, narrowed to those
//! satisfying EVERY criterion the `Filter` names, ordered by the property and direction
//! the `Sort` names, one `MaxResults`-sized page at a time with a `NextToken` that resumes
//! that same order. Each element is the whole stored record: what `CreateMLTransform`
//! wrote, plus the members the server fills in (the minted id, the two instants, the
//! status of a transform nothing has taught, the label count and the schema of the catalog
//! tables the transform reads). `ListMLTransforms` selects and orders identically and
//! reports each selected transform's id, narrowed further to the transforms carrying every
//! pair its `Tags` filter names.
//!
//! A create's `Name` has to be FREE: a create naming a transform this account already
//! holds is refused with the operation's modelled `AlreadyExistsException` — "a resource
//! to be created or added already exists" — whatever definition it carries.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rows differ along what the rule generalises over: how many transforms are stored,
//! the page size asked for (including the published bounds 1 and 1000 and one past each),
//! which criterion the filter names and with which value, which column and direction the
//! sort names (every modelled value of both), and which members each create carried.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, DEFAULT_ML_TRANSFORM_DPUS,
    DEFAULT_ML_TRANSFORM_GLUE_VERSION, DEFAULT_ML_TRANSFORM_TIMEOUT_MINUTES,
    TRANSFORM_STATUS_NOT_READY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{assert_echoed, call, emulator, emulator_on, error_code, error_message, ok};

/// The IAM role every create below names: "The name or Amazon Resource Name (ARN) of the
/// IAM role with the required permissions" (`CreateMLTransform$Role`).
pub(crate) const ROLE: &str = "arn:aws:iam::000000000000:role/glue-ml-transform-role";

/// The only `TransformType` the model publishes (`smithy_query shape=TransformType`,
/// values `FIND_MATCHES`), so every create's `Parameters` names it.
pub(crate) const TRANSFORM_TYPE: &str = "FIND_MATCHES";

/// Every `TransformSortColumnType` the model publishes. Source: the generated
/// `TRANSFORM_SORT_COLUMN_TYPE_VALUES` (`smithy_query shape=TransformSortColumnType`).
const SORT_COLUMNS: [&str; 5] = [
    "CREATED",
    "LAST_MODIFIED",
    "NAME",
    "STATUS",
    "TRANSFORM_TYPE",
];

/// Every `SortDirectionType` the model publishes. Source: the generated
/// `SORT_DIRECTION_TYPE_VALUES` (`smithy_query shape=SortDirectionType`).
const SORT_DIRECTIONS: [&str; 2] = ["ASCENDING", "DESCENDING"];

/// One row of the paging table: the case, how many transforms are stored and the page
/// size asked for. Named so the tables below read as data rather than as types.
type PageRow<'a> = (&'a str, usize, Option<i32>);

/// One row of the create table: the case, the transform's name, the members the create
/// sends beside the required ones, and the members a read must then report.
type CreateRow<'a> = (&'a str, &'a str, Value, Value);

/// One row of a filter table: the case, the filter sent and the ids it must report.
type FilterRow<'a> = (&'a str, Value, Vec<String>);

/// One row of the instant-window table: the case, the filter sent and whether the one
/// stored transform is inside the window it describes.
type WindowRow<'a> = (&'a str, Value, bool);

/// One row of the schema table: the case, the `InputRecordTables` the create sends and the
/// columns (name and type) the transform must then report.
type SchemaRow<'a> = (&'a str, Value, &'a [(&'a str, &'a str)]);

/// One transform a create row stored: the case, the name it was created under, the id the
/// create minted and the members a read must report.
type CreatedRow<'a> = (&'a str, &'a str, String, &'a Value);

/// A create body naming the four `@required` members, plus whatever `extra` adds.
pub(crate) fn create_body(name: &str, extra: Value) -> Value {
    let mut body = json!({
        "Name": name,
        "InputRecordTables": [],
        "Parameters": {
            "TransformType": TRANSFORM_TYPE,
            "FindMatchesParameters": { "PrimaryKeyColumnName": "id" },
        },
        "Role": ROLE,
    });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    body
}

/// Register one transform and answer the `TransformId` the create reports.
pub(crate) async fn given_transform(
    router: &MultiServiceRouter,
    name: &str,
    extra: Value,
) -> String {
    let created = ok(router, "CreateMLTransform", &create_body(name, extra)).await;
    created
        .get("TransformId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("CreateMLTransform must answer a TransformId, got {created}"))
        .to_string()
}

/// Store `database` holding `table` with the columns `columns` names (name and type), so a
/// transform over that table has a catalog schema to report. A database several rows share
/// is created once.
pub(crate) async fn store_table(
    router: &MultiServiceRouter,
    database: &str,
    table: &str,
    columns: &[(&str, &str)],
) {
    let (status, parsed) = call(
        router,
        "CreateDatabase",
        &json!({ "DatabaseInput": { "Name": database } }),
    )
    .await;
    assert!(
        status == 200 || error_code(&parsed).ends_with("AlreadyExistsException"),
        "CreateDatabase must answer 200 or report the database is already stored, got {status} \
         with {parsed}"
    );
    ok(
        router,
        "CreateTable",
        &json!({
            "DatabaseName": database,
            "TableInput": {
                "Name": table,
                "StorageDescriptor": {
                    "Location": format!("s3://bucket/{table}/"),
                    "Columns": columns
                        .iter()
                        .map(|(name, data_type)| json!({ "Name": name, "Type": data_type }))
                        .collect::<Vec<Value>>(),
                },
            },
        }),
    )
    .await;
}

/// The `Transforms` of one answer, in the order the service returned them.
pub(crate) fn listed(page: &Value) -> Vec<Value> {
    page["Transforms"]
        .as_array()
        .unwrap_or_else(|| panic!("GetMLTransforms must report a Transforms list, got {page}"))
        .clone()
}

/// One member of every listed transform, in the order the service returned them.
fn listed_member(page: &Value, member: &str) -> Vec<String> {
    listed(page)
        .iter()
        .map(|transform| {
            transform[member]
                .as_str()
                .unwrap_or_else(|| panic!("MLTransform.{member} is a string, got {transform}"))
                .to_string()
        })
        .collect()
}

/// Walk every page the service hands out for `request`, following its own `NextToken`, and
/// answer the transforms in the order they were reported.
async fn walk(
    router: &MultiServiceRouter,
    case: &str,
    base: &Value,
    page_size: Option<i32>,
    limit: usize,
) -> Vec<Value> {
    let mut seen: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = base.clone();
        if let Some(size) = page_size {
            request["MaxResults"] = json!(size);
        }
        if let Some(cursor) = &token {
            request["NextToken"] = json!(cursor);
        }
        let page = ok(router, "GetMLTransforms", &request).await;
        let transforms = listed(&page);
        if let Some(size) = page_size {
            assert!(
                transforms.len() <= usize::try_from(size).unwrap_or(usize::MAX),
                "case {case:?}: a page must not exceed MaxResults {size}, got {page}"
            );
        }
        seen.extend(transforms);
        token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            seen.len() <= limit,
            "case {case:?}: the walk must terminate, got {} entries for {limit} transforms",
            seen.len()
        );
    }
    seen
}

/// The `TransformId` of each entry, in the order they were reported.
fn ids(transforms: &[Value]) -> Vec<String> {
    transforms
        .iter()
        .map(|transform| {
            transform["TransformId"]
                .as_str()
                .unwrap_or_else(|| panic!("MLTransform.TransformId is a string, got {transform}"))
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn every_stored_transform_is_listed_and_paged() {
    // (case, how many transforms are stored, the page size asked for)
    let rows: &[PageRow<'_>] = &[
        ("no transforms at all", 0, None),
        ("one transform, no page size", 1, None),
        // The published lower bound of `MaxResults` (`PageSize` range 1..1000).
        ("the smallest page the model accepts", 5, Some(1)),
        ("a page smaller than the stored count", 7, Some(3)),
        ("a page larger than the stored count", 3, Some(50)),
        // The published upper bound.
        ("the largest page the model accepts", 4, Some(1_000)),
    ];

    for (case, stored, page_size) in rows {
        let router = emulator();
        // Created out of name order, so the listing's own order is what is asserted.
        let expected: Vec<String> = (0..*stored)
            .map(|index| format!("transform-{index:02}"))
            .collect();
        for name in expected.iter().rev() {
            given_transform(&router, name, Value::Null).await;
        }

        let seen = walk(&router, case, &json!({}), *page_size, *stored).await;
        let names: Vec<String> = seen
            .iter()
            .map(|transform| transform["Name"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            names, expected,
            "case {case:?}: expected every stored transform in the listing's own order, got \
             {names:?}"
        );
        // An id addresses ONE transform: no page repeats one and none is dropped.
        let mut unique = ids(&seen);
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            *stored,
            "case {case:?}: expected {stored} distinct TransformIds, got {:?}",
            ids(&seen)
        );

        // An empty store is an empty listing, not a refusal and not a missing member, and
        // a request that names no page size reports the whole store in one page.
        let page = ok(&router, "GetMLTransforms", &json!({})).await;
        assert_eq!(
            listed(&page).len(),
            *stored,
            "case {case:?}: one unbounded page must report all {stored} stored transforms, got \
             {page}"
        );
        // Listing is reporting only: two listings in a row agree, so a read changed
        // nothing a later caller sees.
        let again = ok(&router, "GetMLTransforms", &json!({})).await;
        assert_eq!(
            listed(&again),
            listed(&page),
            "case {case:?}: listed twice, a listing must change nothing"
        );
    }
}

#[tokio::test]
async fn a_listed_transform_carries_the_create_and_the_documented_defaults() {
    let router = emulator();

    // The rows differ along which members the create carried: the required ones alone
    // (every documented default applies), a worker configuration, a DPU figure, an
    // explicit Glue version and timeout, and the optional description, retries, tags and
    // encryption settings.
    let rows: &[CreateRow<'_>] = &[
        (
            "required members only",
            "aaa-bare",
            Value::Null,
            // "If the value is not set, the Glue compatibility defaults to Glue 0.9"
            // (`CreateMLTransform$GlueVersion`); "the default is 10" DPUs
            // (`$MaxCapacity`); "The default is 2,880 minutes (48 hours)" (`$Timeout`).
            json!({
                "GlueVersion": DEFAULT_ML_TRANSFORM_GLUE_VERSION,
                "MaxCapacity": DEFAULT_ML_TRANSFORM_DPUS,
                "Timeout": DEFAULT_ML_TRANSFORM_TIMEOUT_MINUTES,
                "WorkerType": Value::Null,
                "NumberOfWorkers": Value::Null,
                "MaxRetries": Value::Null,
            }),
        ),
        (
            "a worker configuration sets the capacity automatically",
            "bbb-workers",
            json!({ "WorkerType": "G.2X", "NumberOfWorkers": 4 }),
            // "When the WorkerType field is set to a value other than Standard, the
            // MaxCapacity field is set automatically": each `G.2X` worker "maps to 2 DPU"
            // (`CreateMLTransform$WorkerType`), so four of them are 8.
            json!({ "WorkerType": "G.2X", "NumberOfWorkers": 4, "MaxCapacity": 8.0 }),
        ),
        (
            "a worker type the model publishes no DPU figure for",
            "ccc-standard",
            json!({ "WorkerType": "Standard", "NumberOfWorkers": 3 }),
            json!({
                "WorkerType": "Standard",
                "NumberOfWorkers": 3,
                "MaxCapacity": Value::Null,
            }),
        ),
        (
            // The sentence quoted above excepts `Standard`, so nothing overrides the
            // figure the request named and nothing the client sent is discarded.
            "a worker type with no DPU figure, and a capacity of its own",
            "ccc-standard-sized",
            json!({ "WorkerType": "Standard", "NumberOfWorkers": 2, "MaxCapacity": 12.0 }),
            json!({
                "WorkerType": "Standard",
                "NumberOfWorkers": 2,
                "MaxCapacity": 12.0,
            }),
        ),
        (
            "a DPU figure of its own",
            "ddd-capacity",
            json!({ "MaxCapacity": 32.5 }),
            json!({ "MaxCapacity": 32.5, "WorkerType": Value::Null }),
        ),
        (
            "an explicit Glue version and timeout",
            "eee-versioned",
            json!({ "GlueVersion": "3.0", "Timeout": 45, "MaxRetries": 2 }),
            json!({ "GlueVersion": "3.0", "Timeout": 45, "MaxRetries": 2 }),
        ),
        (
            "every optional member the shape reports",
            "fff-described",
            json!({
                "Description": "dedupes the customer records",
                "Tags": { "owner": "data-platform" },
                "TransformEncryption": {
                    "MlUserDataEncryption": {
                        "MlUserDataEncryptionMode": "SSE-KMS",
                        "KmsKeyId": "alias/glue-ml",
                    },
                    "TaskRunSecurityConfigurationName": "ml-security",
                },
            }),
            json!({
                "Description": "dedupes the customer records",
                "TransformEncryption": {
                    "MlUserDataEncryption": {
                        "MlUserDataEncryptionMode": "SSE-KMS",
                        "KmsKeyId": "alias/glue-ml",
                    },
                    "TaskRunSecurityConfigurationName": "ml-security",
                },
            }),
        ),
    ];

    let mut created: Vec<CreatedRow<'_>> = Vec::new();
    for (case, name, sent, expected) in rows {
        let id = given_transform(&router, name, sent.clone()).await;
        created.push((case, name, id, expected));
    }

    let seen = walk(&router, "every member", &json!({}), None, rows.len()).await;
    assert_eq!(
        seen.len(),
        rows.len(),
        "case every member: expected {} transforms, got {seen:?}",
        rows.len()
    );

    for (case, name, id, expected) in &created {
        let entry = seen
            .iter()
            .find(|transform| transform["TransformId"].as_str() == Some(id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: {id} must be listed, got {seen:?}"));
        assert_eq!(
            entry["Name"].as_str(),
            Some(*name),
            "case {case:?}: expected the stored name {name}, got {entry}"
        );
        assert_echoed(case, expected, entry);
        // Every member the create could not set is the server's: the role it named is
        // reported back, the status is the one a transform nothing has taught is in, no
        // labelling file has been generated and nothing has evaluated the transform.
        assert_eq!(
            entry["Role"].as_str(),
            Some(ROLE),
            "case {case:?}: expected the stored role, got {entry}"
        );
        assert_eq!(
            entry["Status"].as_str(),
            Some(TRANSFORM_STATUS_NOT_READY),
            "case {case:?}: expected a transform nothing has taught to be \
             {TRANSFORM_STATUS_NOT_READY}, got {entry}"
        );
        assert_eq!(
            entry["LabelCount"].as_i64(),
            Some(0),
            "case {case:?}: expected no labelling files, got {entry}"
        );
        assert!(
            entry.get("EvaluationMetrics").is_none(),
            "case {case:?}: nothing has evaluated this transform, so it must report no \
             EvaluationMetrics, got {entry}"
        );
        assert_eq!(
            entry["Parameters"]["TransformType"].as_str(),
            Some(TRANSFORM_TYPE),
            "case {case:?}: expected the stored parameters, got {entry}"
        );
        // The two instants are reported, and a creation instant never moves.
        let created_on = entry["CreatedOn"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: CreatedOn must be a timestamp, got {entry}")
        });
        assert!(
            created_on > 0.0,
            "case {case:?}: expected a creation instant, got {created_on}"
        );
        assert_eq!(
            entry["LastModifiedOn"].as_f64(),
            Some(created_on),
            "case {case:?}: nothing has modified this transform, so LastModifiedOn is its \
             creation instant, got {entry}"
        );
        // `Tags` are not part of the `MLTransform` shape, so a listed transform carries
        // none even where the create sent them.
        assert!(
            entry.get("Tags").is_none(),
            "case {case:?}: a listed transform must report no Tags, got {entry}"
        );
    }
}

/// One row of the duplicate-name table: the case, the members the FIRST create sends
/// beside the required ones, and the members the second create under that same name sends.
type RepeatRow<'a> = (&'a str, Value, Value);

/// `base` with `member` set to `value`, so a row states only how it differs from the
/// definition the first create stored.
fn with(base: &Value, member: &str, value: Value) -> Value {
    let mut extra = base.clone();
    extra[member] = value;
    extra
}

/// `base` with `member` left out altogether.
fn without(base: &Value, member: &str) -> Value {
    let mut extra = base.clone();
    extra
        .as_object_mut()
        .unwrap_or_else(|| panic!("the definition is an object, got {base}"))
        .remove(member);
    extra
}

/// Every member a create can set beside the four `@required` ones, so a row below can
/// differ from it in any one of them.
fn full_definition() -> Value {
    json!({
        "Description": "dedupes the customer records",
        "GlueVersion": "3.0",
        "Timeout": 30,
        "MaxRetries": 1,
        "MaxCapacity": 12.0,
        "Tags": { "team": "data" },
        "InputRecordTables": [{ "DatabaseName": "marketing", "TableName": "contacts" }],
        "TransformEncryption": {
            "MlUserDataEncryption": {
                "MlUserDataEncryptionMode": "SSE-KMS",
                "KmsKeyId": "alias/glue-ml",
            },
            "TaskRunSecurityConfigurationName": "ml-security",
        },
    })
}

#[tokio::test]
async fn a_create_naming_a_stored_transform_is_refused_because_that_resource_exists() {
    let definition = full_definition();
    // The second create is refused whatever it describes, so the rows span the definition
    // it carries: the stored one repeated exactly, the bare required members, and one row
    // per member a create can set — the refusal is keyed to the NAME and to no member of
    // the definition.
    let rows: &[RepeatRow<'_>] = &[
        (
            "the stored definition repeated exactly",
            definition.clone(),
            definition.clone(),
        ),
        (
            "the required members alone, twice",
            Value::Null,
            Value::Null,
        ),
        (
            "the documented defaults spelled out",
            Value::Null,
            json!({
                "GlueVersion": DEFAULT_ML_TRANSFORM_GLUE_VERSION,
                "MaxCapacity": DEFAULT_ML_TRANSFORM_DPUS,
                "Timeout": DEFAULT_ML_TRANSFORM_TIMEOUT_MINUTES,
            }),
        ),
        (
            "a different description",
            definition.clone(),
            with(&definition, "Description", json!("something else")),
        ),
        (
            "the description dropped",
            definition.clone(),
            without(&definition, "Description"),
        ),
        (
            "a different Glue version",
            definition.clone(),
            with(&definition, "GlueVersion", json!("4.0")),
        ),
        (
            "a different timeout",
            definition.clone(),
            with(&definition, "Timeout", json!(45)),
        ),
        (
            "a different retry count",
            definition.clone(),
            with(&definition, "MaxRetries", json!(2)),
        ),
        (
            "a different capacity",
            definition.clone(),
            with(&definition, "MaxCapacity", json!(24.0)),
        ),
        (
            "a worker configuration instead of a capacity",
            definition.clone(),
            with(
                &without(&definition, "MaxCapacity"),
                "WorkerType",
                json!("G.1X"),
            ),
        ),
        (
            "a different tag set",
            definition.clone(),
            with(&definition, "Tags", json!({ "team": "ops" })),
        ),
        (
            "a different input table",
            definition.clone(),
            with(
                &definition,
                "InputRecordTables",
                json!([{ "DatabaseName": "marketing", "TableName": "leads" }]),
            ),
        ),
        (
            "different algorithm parameters",
            definition.clone(),
            with(
                &definition,
                "Parameters",
                json!({
                    "TransformType": TRANSFORM_TYPE,
                    "FindMatchesParameters": { "PrimaryKeyColumnName": "email" },
                }),
            ),
        ),
        (
            "different encryption settings",
            definition.clone(),
            with(
                &definition,
                "TransformEncryption",
                json!({ "TaskRunSecurityConfigurationName": "other-security" }),
            ),
        ),
        (
            "a different role",
            definition.clone(),
            with(
                &definition,
                "Role",
                json!("arn:aws:iam::000000000000:role/other-role"),
            ),
        ),
    ];

    for (case, first_extra, second_extra) in rows {
        let router = emulator();
        store_table(
            &router,
            "marketing",
            "contacts",
            &[("email", "string"), ("zip", "int")],
        )
        .await;
        // The name differs per row, so the refusal's wording is read off the REQUEST
        // rather than being one sentence that happens to fit one name.
        let name = format!("taken-{}", case.replace(' ', "-"));
        let first = given_transform(&router, &name, first_extra.clone()).await;

        // "A resource to be created or added already exists" (`AlreadyExistsException`):
        // the `Name` a create chooses has to be free.
        let (status, body) = call(
            &router,
            "CreateMLTransform",
            &create_body(&name, second_extra.clone()),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled AlreadyExistsException's 400, got {status} \
             with {body}"
        );
        assert!(
            error_code(&body).ends_with("AlreadyExistsException"),
            "case {case:?}: expected AlreadyExistsException, got {}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            format!("Transform with name {name} already exists"),
            "case {case:?}: expected the refusal to name the name the create asked for, \
             got {body}"
        );

        // A refused create stored nothing and minted nothing: the transform the first
        // create stored is still the only one this account holds.
        let seen = walk(&router, case, &json!({}), None, 2).await;
        assert_eq!(
            ids(&seen),
            vec![first.clone()],
            "case {case:?}: a refused create must store no transform, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn one_name_per_transform_leaves_every_other_name_free() {
    let router = emulator();
    // Only the name a stored transform carries is taken, so a create under a free name is
    // a new transform even where every other member repeats a stored one.
    let first = given_transform(&router, "first-name", Value::Null).await;
    let second = given_transform(&router, "second-name", Value::Null).await;
    assert_ne!(
        first, second,
        "case two names: each create must mint its own TransformId, got {first} twice"
    );

    let seen = walk(&router, "two names", &json!({}), None, 2).await;
    let mut listed_ids = ids(&seen);
    listed_ids.sort();
    let mut expected = vec![first, second];
    expected.sort();
    assert_eq!(
        listed_ids, expected,
        "case two names: both transforms must be listed, got {seen:?}"
    );
}

#[tokio::test]
async fn a_filter_narrows_the_listing_to_the_transforms_satisfying_every_criterion() {
    let router = emulator();
    store_table(
        &router,
        "marketing",
        "contacts",
        &[("email", "string"), ("zip", "int")],
    )
    .await;

    let plain = given_transform(&router, "plain", Value::Null).await;
    let versioned =
        given_transform(&router, "versioned", json!({ "GlueVersion": "3.0" })).await;
    let schemad = given_transform(
        &router,
        "schemad",
        json!({ "InputRecordTables": [{ "DatabaseName": "marketing", "TableName": "contacts" }] }),
    )
    .await;

    let all = {
        let mut ids = vec![plain.clone(), versioned.clone(), schemad.clone()];
        ids.sort();
        ids
    };

    // (case, the Filter, the ids it must report)
    let rows: &[FilterRow<'_>] = &[
        ("no filter at all", Value::Null, all.clone()),
        ("an empty filter narrows nothing", json!({}), all.clone()),
        (
            "a name that one transform carries",
            json!({ "Name": "versioned" }),
            vec![versioned.clone()],
        ),
        (
            "a name nothing carries",
            json!({ "Name": "absent" }),
            Vec::new(),
        ),
        (
            "the only published transform type",
            json!({ "TransformType": TRANSFORM_TYPE }),
            all.clone(),
        ),
        (
            "the status every untaught transform is in",
            json!({ "Status": TRANSFORM_STATUS_NOT_READY }),
            all.clone(),
        ),
        (
            "a status no stored transform is in",
            json!({ "Status": "READY" }),
            Vec::new(),
        ),
        (
            "the Glue version the creates defaulted to",
            json!({ "GlueVersion": DEFAULT_ML_TRANSFORM_GLUE_VERSION }),
            {
                let mut ids = vec![plain.clone(), schemad.clone()];
                ids.sort();
                ids
            },
        ),
        (
            "a Glue version one create named",
            json!({ "GlueVersion": "3.0" }),
            vec![versioned.clone()],
        ),
        (
            "a column of the catalog table one transform reads",
            json!({ "Schema": [{ "Name": "email", "DataType": "string" }] }),
            vec![schemad.clone()],
        ),
        (
            "a column named without its type",
            json!({ "Schema": [{ "Name": "zip" }] }),
            vec![schemad.clone()],
        ),
        (
            "a column with the wrong type",
            json!({ "Schema": [{ "Name": "email", "DataType": "int" }] }),
            Vec::new(),
        ),
        (
            "every column of that table, which it must all carry",
            json!({
                "Schema": [
                    { "Name": "email", "DataType": "string" },
                    { "Name": "zip", "DataType": "int" },
                ],
            }),
            vec![schemad.clone()],
        ),
        (
            "a column nothing carries",
            json!({ "Schema": [{ "Name": "phone", "DataType": "string" }] }),
            Vec::new(),
        ),
        (
            "two criteria that must BOTH hold",
            json!({ "Name": "versioned", "GlueVersion": "3.0" }),
            vec![versioned.clone()],
        ),
        (
            "two criteria no transform satisfies together",
            json!({ "Name": "versioned", "GlueVersion": "4.0" }),
            Vec::new(),
        ),
    ];

    for (case, filter, expected) in rows {
        let mut request = json!({});
        if !filter.is_null() {
            request["Filter"] = filter.clone();
        }
        let page = ok(&router, "GetMLTransforms", &request).await;
        let mut got = ids(&listed(&page));
        got.sort();
        assert_eq!(
            &got, expected,
            "case {case:?}: expected the transforms satisfying {filter}, got {page}"
        );
        // A filter narrows a PAGED walk too, so the token resumes within the filtered set
        // rather than over the whole store.
        let paged = walk(&router, case, &request, Some(1), 3).await;
        let mut paged_ids = ids(&paged);
        paged_ids.sort();
        assert_eq!(
            &paged_ids, expected,
            "case {case:?}: a one-per-page walk must report the same transforms, got {paged:?}"
        );
    }
}

#[tokio::test]
async fn an_instant_criterion_bounds_the_window_the_listing_reports() {
    let router = emulator();
    let only = given_transform(&router, "dated", Value::Null).await;
    let page = ok(&router, "GetMLTransforms", &json!({})).await;
    let entry = listed(&page)
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("the stored transform must be listed, got {page}"));
    let created_on = entry["CreatedOn"]
        .as_f64()
        .unwrap_or_else(|| panic!("CreatedOn must be a timestamp, got {entry}"));
    // One second each side of the transform's own instant, so each row sits firmly inside
    // or outside the window the criterion describes rather than on its edge, where the
    // reported instant is only a rendering of the stored one.
    let (before, after) = (created_on - 1.0, created_on + 1.0);

    // (case, the Filter, whether the transform is inside the window it describes)
    let rows: &[WindowRow<'_>] = &[
        ("created before a later instant", json!({ "CreatedBefore": after }), true),
        ("created before an earlier instant", json!({ "CreatedBefore": before }), false),
        ("created after an earlier instant", json!({ "CreatedAfter": before }), true),
        ("created after a later instant", json!({ "CreatedAfter": after }), false),
        (
            "last modified before a later instant",
            json!({ "LastModifiedBefore": after }),
            true,
        ),
        (
            "last modified before an earlier instant",
            json!({ "LastModifiedBefore": before }),
            false,
        ),
        (
            "last modified after an earlier instant",
            json!({ "LastModifiedAfter": before }),
            true,
        ),
        (
            "last modified after a later instant",
            json!({ "LastModifiedAfter": after }),
            false,
        ),
        (
            "a window around the instant",
            json!({ "CreatedAfter": before, "CreatedBefore": after }),
            true,
        ),
        (
            "a window that closes before it",
            json!({ "CreatedAfter": before - 1.0, "CreatedBefore": before }),
            false,
        ),
    ];

    for (case, filter, inside) in rows {
        let page = ok(&router, "GetMLTransforms", &json!({ "Filter": filter })).await;
        let got = ids(&listed(&page));
        let expected: Vec<String> = if *inside {
            vec![only.clone()]
        } else {
            Vec::new()
        };
        assert_eq!(
            got, expected,
            "case {case:?}: expected the transforms inside {filter}, got {page}"
        );
    }
}

#[tokio::test]
async fn a_sort_criteria_orders_the_listing_and_its_pages() {
    let router = emulator();
    // Created in an order that is neither the ascending nor the descending name order, so
    // neither can be satisfied by accident, and one at a time, so their creation instants
    // differ.
    let mut by_creation: Vec<(String, String)> = Vec::new();
    for name in ["mid", "zzz", "aaa"] {
        let id = given_transform(&router, name, Value::Null).await;
        by_creation.push((name.to_string(), id));
    }
    let names_by_creation: Vec<String> = by_creation
        .iter()
        .map(|(name, _)| name.clone())
        .collect();
    let mut names_by_name = names_by_creation.clone();
    names_by_name.sort();

    for column in SORT_COLUMNS {
        for direction in SORT_DIRECTIONS {
            let case = format!("{column} {direction}");
            let sort = json!({ "Sort": { "Column": column, "SortDirection": direction } });
            let page = ok(&router, "GetMLTransforms", &sort).await;
            let ordered = listed_member(&page, "Name");
            assert_eq!(
                ordered.len(),
                by_creation.len(),
                "case {case:?}: every stored transform must be reported, got {page}"
            );

            // The columns whose value differs per transform have ONE answer; the two whose
            // value every stored transform shares (`STATUS`, `TRANSFORM_TYPE`) order by the
            // tie-break, so what is asserted of them is the complete, duplicate-free walk
            // below.
            let expected: Option<Vec<String>> = match column {
                "NAME" => Some(names_by_name.clone()),
                "CREATED" | "LAST_MODIFIED" => Some(names_by_creation.clone()),
                _ => None,
            };
            if let Some(mut expected) = expected {
                if direction == "DESCENDING" {
                    expected.reverse();
                }
                assert_eq!(
                    ordered, expected,
                    "case {case:?}: expected {expected:?}, got {ordered:?}"
                );
            }

            // A token resumes the SAME order: walking one transform per page reports
            // exactly the sequence the unpaged listing does.
            let paged = walk(&router, &case, &sort, Some(1), by_creation.len()).await;
            let paged_names: Vec<String> = paged
                .iter()
                .map(|transform| transform["Name"].as_str().unwrap_or_default().to_string())
                .collect();
            assert_eq!(
                paged_names, ordered,
                "case {case:?}: a one-per-page walk must report the listing's own order, got \
                 {paged_names:?} against {ordered:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_schema_of_a_transform_is_the_catalog_schema_of_the_tables_it_reads() {
    let router = emulator();
    store_table(
        &router,
        "sales",
        "orders",
        &[("order_id", "bigint"), ("total", "double")],
    )
    .await;
    store_table(
        &router,
        "sales",
        "returns",
        &[("total", "double"), ("reason", "string")],
    )
    .await;

    // (case, the InputRecordTables, the columns the transform must report)
    let rows: &[SchemaRow<'_>] = &[
        ("no input tables at all", json!([]), &[]),
        (
            "one stored table",
            json!([{ "DatabaseName": "sales", "TableName": "orders" }]),
            &[("order_id", "bigint"), ("total", "double")],
        ),
        (
            "two stored tables, each column once",
            json!([
                { "DatabaseName": "sales", "TableName": "orders" },
                { "DatabaseName": "sales", "TableName": "returns" },
            ]),
            &[
                ("order_id", "bigint"),
                ("total", "double"),
                ("reason", "string"),
            ],
        ),
        (
            "a table addressed through the account's own catalog",
            json!([{
                "DatabaseName": "sales",
                "TableName": "returns",
                "CatalogId": crate::wire::ACCOUNT_ID,
            }]),
            &[("total", "double"), ("reason", "string")],
        ),
        (
            "a table the catalog does not hold",
            json!([{ "DatabaseName": "sales", "TableName": "nowhere" }]),
            &[],
        ),
    ];

    for (case, tables, expected) in rows {
        let id = given_transform(
            &router,
            &format!("over-{}", case.replace(' ', "-")),
            json!({ "InputRecordTables": tables }),
        )
        .await;
        let page = ok(
            &router,
            "GetMLTransforms",
            &json!({ "Filter": { "Name": format!("over-{}", case.replace(' ', "-")) } }),
        )
        .await;
        let entry = listed(&page)
            .into_iter()
            .find(|transform| transform["TransformId"].as_str() == Some(id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: {id} must be listed, got {page}"));
        let schema: Vec<(String, String)> = entry["Schema"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: Schema is a list, got {entry}"))
            .iter()
            .map(|column| {
                (
                    column["Name"].as_str().unwrap_or_default().to_string(),
                    column["DataType"].as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let expected: Vec<(String, String)> = expected
            .iter()
            .map(|(name, data_type)| ((*name).to_string(), (*data_type).to_string()))
            .collect();
        assert_eq!(
            schema, expected,
            "case {case:?}: expected the catalog columns of {tables}, got {entry}"
        );
        // The input tables themselves are reported as the create addressed them.
        assert_echoed(case, tables, &entry["InputRecordTables"]);
    }
}

#[tokio::test]
async fn the_listing_reports_what_the_store_holds_now() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    let kept = given_transform(&router, "kept", Value::Null).await;
    let removed = given_transform(&router, "removed", Value::Null).await;

    let before = ok(&router, "GetMLTransforms", &json!({})).await;
    let mut before_ids = ids(&listed(&before));
    before_ids.sort();
    let mut both = vec![kept.clone(), removed.clone()];
    both.sort();
    assert_eq!(
        before_ids, both,
        "case before the record is dropped: expected every stored transform, got {before}"
    );

    // `DeleteMLTransform` is still a scaffold, so the record is dropped from the store the
    // listing reads rather than through that operation.
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        state.lock().ml_transforms.remove(&removed);
    }

    let after = ok(&router, "GetMLTransforms", &json!({})).await;
    assert_eq!(
        ids(&listed(&after)),
        vec![kept],
        "case after the record is dropped: a transform the store no longer holds must not be \
         listed, got {after}"
    );
}

#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let router = emulator();
    given_transform(&router, "bounded", Value::Null).await;

    // `MaxResults` is `PageSize`, range 1..1000: one past each published bound is refused
    // by the modelled constraint rather than served as some other page size.
    for operation in ["GetMLTransforms", "ListMLTransforms"] {
        for (case, max_results) in [("below the minimum", 0), ("above the maximum", 1_001)] {
            let (status, body) = call(&router, operation, &json!({ "MaxResults": max_results })).await;
            assert_eq!(
                status, 400,
                "case {case:?}: {operation} with MaxResults={max_results} must be refused, got \
                 {status} {body}"
            );
        }
    }
}

/// The `TransformIds` of one `ListMLTransforms` answer, in the order they were reported.
fn listed_ids(page: &Value) -> Vec<String> {
    page["TransformIds"]
        .as_array()
        .unwrap_or_else(|| panic!("ListMLTransforms must report a TransformIds list, got {page}"))
        .iter()
        .map(|id| {
            id.as_str()
                .unwrap_or_else(|| panic!("a TransformId is a string, got {page}"))
                .to_string()
        })
        .collect()
}

/// Walk every page of ids the service hands out for `request`, following its own
/// `NextToken`.
async fn walk_ids(
    router: &MultiServiceRouter,
    case: &str,
    base: &Value,
    page_size: Option<i32>,
    limit: usize,
) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = base.clone();
        if let Some(size) = page_size {
            request["MaxResults"] = json!(size);
        }
        if let Some(cursor) = &token {
            request["NextToken"] = json!(cursor);
        }
        let page = ok(router, "ListMLTransforms", &request).await;
        let ids = listed_ids(&page);
        if let Some(size) = page_size {
            assert!(
                ids.len() <= usize::try_from(size).unwrap_or(usize::MAX),
                "case {case:?}: a page must not exceed MaxResults {size}, got {page}"
            );
        }
        seen.extend(ids);
        token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            seen.len() <= limit,
            "case {case:?}: the walk must terminate, got {} ids for {limit} transforms",
            seen.len()
        );
    }
    seen
}

#[tokio::test]
async fn the_id_listing_selects_and_orders_exactly_as_the_transform_listing() {
    let router = emulator();
    for (name, extra) in [
        ("mid", json!({ "GlueVersion": "3.0" })),
        ("zzz", Value::Null),
        ("aaa", Value::Null),
    ] {
        given_transform(&router, name, extra).await;
    }

    // The rows differ along what the two listings share: no narrowing at all, a filter on
    // each kind of criterion, and every published sort column in both directions.
    let mut requests: Vec<(String, Value)> = vec![
        ("no filter and no sort".to_string(), json!({})),
        (
            "a name filter".to_string(),
            json!({ "Filter": { "Name": "mid" } }),
        ),
        (
            "a Glue version filter".to_string(),
            json!({ "Filter": { "GlueVersion": "3.0" } }),
        ),
        (
            "a filter nothing satisfies".to_string(),
            json!({ "Filter": { "Name": "absent" } }),
        ),
    ];
    for column in SORT_COLUMNS {
        for direction in SORT_DIRECTIONS {
            requests.push((
                format!("sorted by {column} {direction}"),
                json!({ "Sort": { "Column": column, "SortDirection": direction } }),
            ));
        }
    }

    for (case, request) in &requests {
        let transforms = ok(&router, "GetMLTransforms", request).await;
        let expected = ids(&listed(&transforms));
        let listing = ok(&router, "ListMLTransforms", request).await;
        assert_eq!(
            listed_ids(&listing),
            expected,
            "case {case:?}: the id listing must report the transform listing's own selection \
             and order, got {listing} against {transforms}"
        );
        // A token resumes that same order: one id per page reports the same sequence.
        let paged = walk_ids(&router, case, request, Some(1), 3).await;
        assert_eq!(
            paged, expected,
            "case {case:?}: a one-per-page walk must report the same ids, got {paged:?}"
        );
    }
}

#[tokio::test]
async fn a_tag_filter_narrows_the_id_listing_to_the_transforms_carrying_every_pair() {
    let router = emulator();
    let both = given_transform(
        &router,
        "tagged-both",
        json!({ "Tags": { "team": "data", "env": "prod" } }),
    )
    .await;
    let team = given_transform(
        &router,
        "tagged-team",
        json!({ "Tags": { "team": "data" } }),
    )
    .await;
    let other = given_transform(
        &router,
        "tagged-other",
        json!({ "Tags": { "team": "ops" } }),
    )
    .await;
    let untagged = given_transform(&router, "untagged", Value::Null).await;

    let all = {
        let mut ids = vec![
            both.clone(),
            team.clone(),
            other.clone(),
            untagged.clone(),
        ];
        ids.sort();
        ids
    };

    // (case, the Tags filter, the ids it must report)
    let rows: &[FilterRow<'_>] = &[
        ("no filter: every transform", Value::Null, all.clone()),
        ("an empty filter narrows nothing", json!({}), all.clone()),
        ("one pair", json!({ "team": "data" }), {
            let mut ids = vec![both.clone(), team.clone()];
            ids.sort();
            ids
        }),
        (
            "a pair that must BOTH match",
            json!({ "team": "data", "env": "prod" }),
            vec![both.clone()],
        ),
        ("another value of the same key", json!({ "team": "ops" }), vec![other.clone()]),
        ("a value nothing carries", json!({ "team": "finance" }), Vec::new()),
        ("a key nothing carries", json!({ "owner": "data" }), Vec::new()),
    ];

    for (case, filter, expected) in rows {
        let mut request = json!({});
        if !filter.is_null() {
            request["Tags"] = filter.clone();
        }
        let page = ok(&router, "ListMLTransforms", &request).await;
        let mut got = listed_ids(&page);
        got.sort();
        assert_eq!(
            &got, expected,
            "case {case:?}: expected the transforms carrying every pair of {filter}, got {page}"
        );
        // The tag filter narrows a PAGED walk too.
        let mut paged = walk_ids(&router, case, &request, Some(1), 4).await;
        paged.sort();
        assert_eq!(
            &paged, expected,
            "case {case:?}: a one-per-page walk must report the same ids, got {paged:?}"
        );
    }
}
