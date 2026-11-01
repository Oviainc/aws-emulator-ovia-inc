// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that reports `CREATE_COMPLETE` has really created the
//! `AWS::Glue::Database`, `AWS::Glue::Table` and `AWS::Glue::Crawler` resources its
//! template declares — in AWS Glue, in the stack's own Region, under the names the
//! declarations state.
//!
//! The rule under test: **a stack reports a resource as created only where the service that
//! owns it created it, through that service's own public API, under the physical id the
//! stack publishes — and that id is the NAME the declaration states, wherever the owning
//! API takes that name, including inside an input structure.** For the three AWS Glue
//! catalog types the name sits in `DatabaseInput.Name`, in `TableInput.Name` and in a
//! crawler's own `Name`, so `{"Ref": "<logical id>"}` resolves to that name and a client
//! that reads it out of the stack's outputs calls `glue GetDatabase` / `GetTable` /
//! `GetCrawler` with a name AWS Glue answers for — rather than an `EntityNotFoundException`
//! for an entity its own stack just reported complete, or a CloudFormation-generated
//! `<stack>-<logical id>-<suffix>` physical id nothing in AWS Glue is held under.
//!
//! Two crates have to agree for that to hold (CloudFormation makes the calls, AWS Glue
//! serves them), so the suite lives here: a crate-local test can only ever see one side of
//! it. IAM is mounted beside them because the role a crawler crawls as is the other half of
//! the template an ordinary deploy writes.
//!
//! The rows differ along what the rule generalises over: how the template states each name
//! (literally, through `Fn::Sub` over the Region and account pseudo-parameters, or not at
//! all — leaving the name to CloudFormation), whether the template is written in JSON or
//! YAML, which optional members each entity carries (a description, a location, parameters,
//! columns, partition keys, a table type, a table prefix, a schedule, tags), which crawl
//! target kind the crawler names, and the Region the stack is deployed in.
//!
//! The update half and the deletion half are rows of the same rule: a changed declaration
//! reaches AWS Glue through that service's own update with the entity keeping its identity,
//! and deleting the stack releases all three entities there.
//!
//! The router, the two wire helpers over it and the XML readers are the test crate's own
//! ([`crate::cloudformation_provisions_glue_jobs`], [`crate::query_wire`]), not another
//! copy.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_provisions_glue_jobs::{
    build_router, cloudformation, error_code, glue, ACCOUNT, PAST_STACK_SPAN, TRUST_POLICY,
};
use crate::query_wire::{element, opt_element, outputs_of, physical_ids_of};

/// The logical ids every row gives the three entities, so each physical id is read from the
/// one resource it belongs to.
const DATABASE_LOGICAL_ID: &str = "CatalogDatabase";
const TABLE_LOGICAL_ID: &str = "CatalogTable";
const CRAWLER_LOGICAL_ID: &str = "CatalogCrawler";
const ROLE_LOGICAL_ID: &str = "CrawlerRole";

/// The output keys every row's template publishes: each entity's `Ref`, which is how a
/// client reaches the deployed catalog at all.
const DATABASE_OUTPUT: &str = "CatalogDatabaseName";
const TABLE_OUTPUT: &str = "CatalogTableName";
const CRAWLER_OUTPUT: &str = "CatalogCrawlerName";

/// The longest name AWS Glue accepts for any of the three (`NameString`,
/// `@length(min: 1, max: 255)` in that service's own model), which a GENERATED name is held
/// to.
const MAX_NAME: usize = 255;

/// The stack-level tag every `CreateStack` below supplies, which CloudFormation propagates
/// to the resources it creates that are taggable — of these three, the crawler.
const STACK_TAG: (&str, &str) = ("Stack", "glue-catalog-suite");

/// One row of the table: a template, and what the rule says the catalog entities it declares
/// must then be in AWS Glue.
struct Row {
    case: &'static str,
    /// The Region the row deploys in, which is the Region the three entities are created in.
    region: &'static str,
    /// The template body, in the form the row writes it (JSON or YAML), built from the
    /// Region it deploys in.
    template: fn(&str) -> String,
    /// The names the template asks for, as a function of the Region (the `Fn::Sub` row's are
    /// derived from the account and the Region), each `None` for a declaration that names
    /// nothing — whose entity carries the name CloudFormation generated.
    declared_names: fn(&str) -> [Option<String>; 3],
    /// The members `GetDatabase` must report for the database, as `(JSON pointer into
    /// Database, value)` pairs.
    expected_database: fn() -> Vec<(&'static str, Value)>,
    /// The same for the table (`GetTable`) and the crawler (`GetCrawler`).
    expected_table: fn() -> Vec<(&'static str, Value)>,
    expected_crawler: fn() -> Vec<(&'static str, Value)>,
    /// The tags the crawler's own declaration asks for, beside the ones CloudFormation gives
    /// it.
    expected_crawler_tags: &'static [(&'static str, &'static str)],
}

/// The trust policy document every row's role carries, as the inline object a template
/// declares it with.
fn trust_policy() -> Value {
    serde_json::from_str::<Value>(TRUST_POLICY).expect("the trust policy is a document")
}

/// The rows: three templates, each declaring the same three entities differently.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "literal names in a JSON template, with a scheduled crawler over Amazon S3",
            region: "us-east-2",
            template: |_| {
                json!({
                    "Description": "a Glue Data Catalog database, a table in it and a crawler",
                    "Resources": {
                        ROLE_LOGICAL_ID: {
                            "Type": "AWS::IAM::Role",
                            "Properties": { "AssumeRolePolicyDocument": trust_policy() }
                        },
                        DATABASE_LOGICAL_ID: {
                            "Type": "AWS::Glue::Database",
                            "Properties": {
                                "DatabaseInput": {
                                    "Name": "gcat_lake_raw",
                                    "Description": "the raw landing zone",
                                    "LocationUri": "s3://sim2real-lake/raw/",
                                    "Parameters": { "classification": "parquet" }
                                }
                            }
                        },
                        TABLE_LOGICAL_ID: {
                            "Type": "AWS::Glue::Table",
                            "Properties": {
                                "DatabaseName": { "Ref": DATABASE_LOGICAL_ID },
                                "TableInput": {
                                    "Name": "gcat_events",
                                    "TableType": "EXTERNAL_TABLE",
                                    "Parameters": { "classification": "parquet" },
                                    "PartitionKeys": [{ "Name": "dt", "Type": "string" }],
                                    "StorageDescriptor": {
                                        "Location": "s3://sim2real-lake/raw/events/",
                                        "Columns": [
                                            { "Name": "id", "Type": "bigint" },
                                            { "Name": "payload", "Type": "string" }
                                        ]
                                    }
                                }
                            }
                        },
                        CRAWLER_LOGICAL_ID: {
                            "Type": "AWS::Glue::Crawler",
                            "Properties": {
                                "Name": "gcat-raw-crawler",
                                "Role": { "Fn::GetAtt": [ROLE_LOGICAL_ID, "Arn"] },
                                "DatabaseName": { "Ref": DATABASE_LOGICAL_ID },
                                "Description": "crawls the landing zone",
                                "TablePrefix": "raw_",
                                "Schedule": { "ScheduleExpression": "cron(15 12 * * ? *)" },
                                "Targets": {
                                    "S3Targets": [{ "Path": "s3://sim2real-lake/raw/" }]
                                },
                                "Tags": { "Owner": "platform", "Tier": "raw" }
                            }
                        }
                    },
                    "Outputs": {
                        DATABASE_OUTPUT: { "Value": { "Ref": DATABASE_LOGICAL_ID } },
                        TABLE_OUTPUT: { "Value": { "Ref": TABLE_LOGICAL_ID } },
                        CRAWLER_OUTPUT: { "Value": { "Ref": CRAWLER_LOGICAL_ID } }
                    }
                })
                .to_string()
            },
            declared_names: |_| {
                [
                    Some("gcat_lake_raw".to_string()),
                    Some("gcat_events".to_string()),
                    Some("gcat-raw-crawler".to_string()),
                ]
            },
            expected_database: || {
                vec![
                    ("/Description", json!("the raw landing zone")),
                    ("/LocationUri", json!("s3://sim2real-lake/raw/")),
                    ("/Parameters/classification", json!("parquet")),
                ]
            },
            expected_table: || {
                vec![
                    ("/TableType", json!("EXTERNAL_TABLE")),
                    (
                        "/StorageDescriptor/Location",
                        json!("s3://sim2real-lake/raw/events/"),
                    ),
                    ("/StorageDescriptor/Columns/0/Name", json!("id")),
                    ("/StorageDescriptor/Columns/1/Type", json!("string")),
                    ("/PartitionKeys/0/Name", json!("dt")),
                ]
            },
            expected_crawler: || {
                vec![
                    ("/Description", json!("crawls the landing zone")),
                    ("/TablePrefix", json!("raw_")),
                    ("/Schedule/ScheduleExpression", json!("cron(15 12 * * ? *)")),
                    (
                        "/Targets/S3Targets/0/Path",
                        json!("s3://sim2real-lake/raw/"),
                    ),
                ]
            },
            expected_crawler_tags: &[("Owner", "platform"), ("Tier", "raw")],
        },
        Row {
            case: "names derived through Fn::Sub, an unscheduled crawler over Amazon DynamoDB",
            region: "eu-west-1",
            template: |_| {
                json!({
                    "Resources": {
                        ROLE_LOGICAL_ID: {
                            "Type": "AWS::IAM::Role",
                            "Properties": { "AssumeRolePolicyDocument": trust_policy() }
                        },
                        DATABASE_LOGICAL_ID: {
                            "Type": "AWS::Glue::Database",
                            "Properties": {
                                "CatalogId": { "Ref": "AWS::AccountId" },
                                "DatabaseInput": {
                                    "Name": { "Fn::Sub": "gcat_${AWS::Region}_curated" }
                                }
                            }
                        },
                        TABLE_LOGICAL_ID: {
                            "Type": "AWS::Glue::Table",
                            "Properties": {
                                "CatalogId": { "Ref": "AWS::AccountId" },
                                "DatabaseName": { "Ref": DATABASE_LOGICAL_ID },
                                "TableInput": {
                                    "Name": { "Fn::Sub": "gcat_${AWS::Region}_sessions" },
                                    "Owner": "data-eng",
                                    "StorageDescriptor": {
                                        "Location": "s3://sim2real-lake/curated/sessions/",
                                        "Columns": [{ "Name": "session_id", "Type": "string" }]
                                    }
                                }
                            }
                        },
                        CRAWLER_LOGICAL_ID: {
                            "Type": "AWS::Glue::Crawler",
                            "Properties": {
                                "Name": { "Fn::Sub": "gcat-${AWS::AccountId}-ddb" },
                                "Role": { "Fn::GetAtt": [ROLE_LOGICAL_ID, "Arn"] },
                                "DatabaseName": { "Ref": DATABASE_LOGICAL_ID },
                                "Targets": {
                                    "DynamoDBTargets": [{ "Path": "sim2real-sessions" }]
                                }
                            }
                        }
                    },
                    "Outputs": {
                        DATABASE_OUTPUT: { "Value": { "Ref": DATABASE_LOGICAL_ID } },
                        TABLE_OUTPUT: { "Value": { "Ref": TABLE_LOGICAL_ID } },
                        CRAWLER_OUTPUT: { "Value": { "Ref": CRAWLER_LOGICAL_ID } }
                    }
                })
                .to_string()
            },
            declared_names: |region| {
                [
                    Some(format!("gcat_{region}_curated")),
                    Some(format!("gcat_{region}_sessions")),
                    Some(format!("gcat-{ACCOUNT}-ddb")),
                ]
            },
            expected_database: Vec::new,
            expected_table: || {
                vec![
                    ("/Owner", json!("data-eng")),
                    (
                        "/StorageDescriptor/Location",
                        json!("s3://sim2real-lake/curated/sessions/"),
                    ),
                    ("/StorageDescriptor/Columns/0/Name", json!("session_id")),
                ]
            },
            expected_crawler: || {
                vec![(
                    "/Targets/DynamoDBTargets/0/Path",
                    json!("sim2real-sessions"),
                )]
            },
            expected_crawler_tags: &[],
        },
        Row {
            case: "a YAML template that names none of the three, so CloudFormation generates \
                   each name",
            region: "ap-southeast-2",
            template: |_| {
                [
                    "Resources:",
                    "  CrawlerRole:",
                    "    Type: AWS::IAM::Role",
                    "    Properties:",
                    "      AssumeRolePolicyDocument:",
                    "        Version: \"2012-10-17\"",
                    "        Statement:",
                    "          - Effect: Allow",
                    "            Principal:",
                    "              Service: glue.amazonaws.com",
                    "            Action: sts:AssumeRole",
                    "  CatalogDatabase:",
                    "    Type: AWS::Glue::Database",
                    "    Properties:",
                    "      DatabaseInput:",
                    "        Description: a database CloudFormation named",
                    "  CatalogTable:",
                    "    Type: AWS::Glue::Table",
                    "    Properties:",
                    "      DatabaseName: !Ref CatalogDatabase",
                    "      TableInput:",
                    "        TableType: EXTERNAL_TABLE",
                    "        StorageDescriptor:",
                    "          Location: s3://sim2real-lake/generated/",
                    "          Columns:",
                    "            - Name: ingested_at",
                    "              Type: timestamp",
                    "  CatalogCrawler:",
                    "    Type: AWS::Glue::Crawler",
                    "    Properties:",
                    "      Role: !GetAtt CrawlerRole.Arn",
                    "      DatabaseName: !Ref CatalogDatabase",
                    "      Targets:",
                    "        S3Targets:",
                    "          - Path: s3://sim2real-lake/generated/",
                    "Outputs:",
                    "  CatalogDatabaseName:",
                    "    Value: !Ref CatalogDatabase",
                    "  CatalogTableName:",
                    "    Value: !Ref CatalogTable",
                    "  CatalogCrawlerName:",
                    "    Value: !Ref CatalogCrawler",
                ]
                .join("\n")
            },
            declared_names: |_| [None, None, None],
            expected_database: || vec![("/Description", json!("a database CloudFormation named"))],
            expected_table: || {
                vec![
                    ("/TableType", json!("EXTERNAL_TABLE")),
                    (
                        "/StorageDescriptor/Location",
                        json!("s3://sim2real-lake/generated/"),
                    ),
                    ("/StorageDescriptor/Columns/0/Type", json!("timestamp")),
                ]
            },
            expected_crawler: || {
                vec![(
                    "/Targets/S3Targets/0/Path",
                    json!("s3://sim2real-lake/generated/"),
                )]
            },
            expected_crawler_tags: &[],
        },
    ]
}

/// The name AWS Glue holds a catalog entity under, given the name the stack published: "for
/// Hive compatibility, this is folded to lowercase when it is stored"
/// (`DatabaseInput$Name`, and `TableInput$Name` with it), so a name carrying upper case —
/// which a CloudFormation-generated `<stack>-<logical id>-<suffix>` does — is reachable by
/// either casing and REPORTED folded.
fn folded(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// Read one entity back through AWS Glue's own API, requiring the 200 the rule says the
/// name the stack published must answer with, and answer the entity document.
async fn read_entity(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    operation: &str,
    request: Value,
    member: &str,
) -> Value {
    let (status, body) = glue(router, region, operation, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {operation} on the name the stack reported, got \
         {status} with body {body}"
    );
    body.get(member)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: no {member} in the {operation} answer {body}"))
}

/// Every row: each name the stack reports for its three AWS Glue resources is an entity AWS
/// Glue really holds, configured as the template declared it — and deleting the stack
/// releases all three.
#[tokio::test]
async fn a_stack_provisions_the_glue_catalog_entities_its_template_declares() {
    let router = build_router();

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let stack_name = format!("glue-catalog-stack-{index}");
        let template = (row.template)(region);

        let (status, body) = cloudformation(
            &router,
            region,
            "CreateStack",
            &[
                ("StackName", &stack_name),
                ("TemplateBody", &template),
                // Every row's template declares the IAM role the crawler crawls as.
                ("Capabilities.member.1", "CAPABILITY_IAM"),
                ("Tags.member.1.Key", STACK_TAG.0),
                ("Tags.member.1.Value", STACK_TAG.1),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
        );

        tokio::time::sleep(PAST_STACK_SPAN).await;
        let (status, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body \
             {described}"
        );
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to reach CREATE_COMPLETE, got {described}"
        );

        let (status, resources) = cloudformation(
            &router,
            region,
            "DescribeStackResources",
            &[("StackName", &stack_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResources, got {status} with \
             body {resources}"
        );
        let physical_ids = physical_ids_of(&resources);
        let outputs = outputs_of(&described);
        let declared = (row.declared_names)(region);

        // `Ref` of each of the three resolves to the NAME the declaration states — inside
        // `DatabaseInput`/`TableInput` for the two catalog entities, the crawler's own `Name`
        // for the crawler — or to the one CloudFormation generated for a declaration that
        // named nothing, which is held to the name AWS Glue accepts.
        let mut names: Vec<String> = Vec::new();
        for (slot, (logical_id, output)) in [
            (DATABASE_LOGICAL_ID, DATABASE_OUTPUT),
            (TABLE_LOGICAL_ID, TABLE_OUTPUT),
            (CRAWLER_LOGICAL_ID, CRAWLER_OUTPUT),
        ]
        .into_iter()
        .enumerate()
        {
            let physical = physical_ids.get(logical_id).cloned().unwrap_or_else(|| {
                panic!("case {case:?}: no {logical_id} resource in {resources}")
            });
            if let Some(expected) = declared[slot].as_deref() {
                assert_eq!(
                    physical, expected,
                    "case {case:?}: expected the physical id of {logical_id} to be the declared \
                     name {expected:?}, got {physical:?}"
                );
            }
            assert!(
                !physical.is_empty() && physical.chars().count() <= MAX_NAME,
                "case {case:?}: expected the physical id of {logical_id} to be inside AWS \
                 Glue's own NameString bound of {MAX_NAME}, got {physical:?} ({} characters)",
                physical.chars().count()
            );
            assert_eq!(
                outputs.get(output),
                Some(&physical),
                "case {case:?}: expected output {output:?} to be {physical:?}, got {outputs:?}"
            );
            names.push(physical);
        }
        let [database_name, table_name, crawler_name] = <[String; 3]>::try_from(names)
            .unwrap_or_else(|held| panic!("case {case:?}: three names, got {held:?}"));

        // CloudFormation's own resource index resolves the database's name back to the stack,
        // the join a client makes to ask which stack owns a physical id.
        let (status, owned) = cloudformation(
            &router,
            region,
            "DescribeStackResources",
            &[("PhysicalResourceId", &database_name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResources on the physical id, \
             got {status} with body {owned}"
        );
        assert_eq!(
            element(&owned, "StackName"),
            stack_name,
            "case {case:?}: expected the physical id to resolve to {stack_name:?}, got {owned}"
        );

        // AWS Glue itself resolves each published name to the entity the template declared.
        let database = read_entity(
            &router,
            case,
            region,
            "GetDatabase",
            json!({ "Name": database_name }),
            "Database",
        )
        .await;
        assert_eq!(
            database.get("Name").and_then(Value::as_str),
            Some(folded(&database_name).as_str()),
            "case {case:?}: expected Database.Name to be the name the stack published, got \
             {database}"
        );
        assert_eq!(
            database.get("CatalogId").and_then(Value::as_str),
            Some(ACCOUNT),
            "case {case:?}: expected the database to be in this account's catalog, got \
             {database}"
        );
        for (pointer, expected) in (row.expected_database)() {
            let actual = database.pointer(pointer).cloned().unwrap_or(Value::Null);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected Database{pointer} to be {expected}, got {actual} in \
                 {database}"
            );
        }

        let table = read_entity(
            &router,
            case,
            region,
            "GetTable",
            json!({ "DatabaseName": database_name, "Name": table_name }),
            "Table",
        )
        .await;
        assert_eq!(
            table.get("Name").and_then(Value::as_str),
            Some(folded(&table_name).as_str()),
            "case {case:?}: expected Table.Name to be the name the stack published, got {table}"
        );
        // The table was created IN the database the `Ref` named, which is the other half of
        // the pair AWS Glue keys a table by.
        assert_eq!(
            table.get("DatabaseName").and_then(Value::as_str),
            Some(folded(&database_name).as_str()),
            "case {case:?}: expected Table.DatabaseName to be {database_name:?}, got {table}"
        );
        for (pointer, expected) in (row.expected_table)() {
            let actual = table.pointer(pointer).cloned().unwrap_or(Value::Null);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected Table{pointer} to be {expected}, got {actual} in \
                 {table}"
            );
        }

        let crawler = read_entity(
            &router,
            case,
            region,
            "GetCrawler",
            json!({ "Name": crawler_name }),
            "Crawler",
        )
        .await;
        assert_eq!(
            crawler.get("Name").and_then(Value::as_str),
            Some(crawler_name.as_str()),
            "case {case:?}: expected Crawler.Name to be the name the stack published, got \
             {crawler}"
        );
        // A crawler's name is NOT folded — only a database's and a table's are — and the
        // crawler writes its output to the database the `Ref` named, as the role the template
        // declared.
        assert_eq!(
            crawler.get("DatabaseName").and_then(Value::as_str),
            Some(database_name.as_str()),
            "case {case:?}: expected Crawler.DatabaseName to be {database_name:?}, got {crawler}"
        );
        let role_arn = physical_ids
            .get(ROLE_LOGICAL_ID)
            .map(|name| format!("arn:aws:iam::{ACCOUNT}:role/{name}"))
            .unwrap_or_else(|| panic!("case {case:?}: no {ROLE_LOGICAL_ID} resource"));
        assert_eq!(
            crawler.get("Role").and_then(Value::as_str),
            Some(role_arn.as_str()),
            "case {case:?}: expected Crawler.Role to be the role ARN the template passed \
             ({role_arn:?}), got {crawler}"
        );
        for (pointer, expected) in (row.expected_crawler)() {
            let actual = crawler.pointer(pointer).cloned().unwrap_or(Value::Null);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected Crawler{pointer} to be {expected}, got {actual} in \
                 {crawler}"
            );
        }

        // The tags the crawler's declaration asked for are on it, beside the stack's own and
        // the three CloudFormation records on every taggable resource it creates.
        let arn = format!("arn:aws:glue:{region}:{ACCOUNT}:crawler/{crawler_name}");
        let (status, tagged) =
            glue(&router, region, "GetTags", &json!({ "ResourceArn": arn })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetTags on {arn}, got {status} with body \
             {tagged}"
        );
        let reported: BTreeMap<String, String> = tagged
            .get("Tags")
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("case {case:?}: no Tags map in {tagged}"))
            .iter()
            .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_string())))
            .collect();
        for (key, value) in row.expected_crawler_tags {
            assert_eq!(
                reported.get(*key).map(String::as_str),
                Some(*value),
                "case {case:?}: expected tag {key:?} to be {value:?}, got {reported:?}"
            );
        }
        assert_eq!(
            reported.get(STACK_TAG.0).map(String::as_str),
            Some(STACK_TAG.1),
            "case {case:?}: expected the stack's own tag to be propagated, got {reported:?}"
        );
        assert_eq!(
            reported
                .get("aws:cloudformation:logical-id")
                .map(String::as_str),
            Some(CRAWLER_LOGICAL_ID),
            "case {case:?}: expected the crawler to carry its logical id, got {reported:?}"
        );

        // Deleting the stack releases all three entities AWS Glue holds for it.
        let (status, body) =
            cloudformation(&router, region, "DeleteStack", &[("StackName", &stack_name)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
        );
        for (operation, request) in [
            ("GetDatabase", json!({ "Name": database_name })),
            (
                "GetTable",
                json!({ "DatabaseName": database_name, "Name": table_name }),
            ),
            ("GetCrawler", json!({ "Name": crawler_name })),
        ] {
            let (status, gone) = glue(&router, region, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 from {operation} once the stack deleted the \
                 entity, got {status} with body {gone}"
            );
            assert_eq!(
                error_code(&gone),
                "EntityNotFoundException",
                "case {case:?}: expected EntityNotFoundException from {operation} once the \
                 entity was deleted, got {gone}"
            );
        }
    }
}

/// One deployment of the update suite's one stack: the case, the three entities' declared
/// members beyond their names, and the `(JSON pointer, value)` pairs each read must then
/// report.
struct Deployment {
    case: &'static str,
    database_input: Value,
    table_input: Value,
    crawler_properties: Value,
    expected_database: Vec<(&'static str, Value)>,
    expected_table: Vec<(&'static str, Value)>,
    expected_crawler: Vec<(&'static str, Value)>,
}

/// A CHANGED declaration of any of the three reaches AWS Glue through that service's own
/// update: the stack keeps the entities it created — their names, which are their identities,
/// are unchanged — and the definition in force is the one the template now declares.
///
/// The deployments differ along which member changes: a database's description and
/// parameters, a table's columns and location, and a crawler's prefix and schedule — so what
/// is asserted is that the declaration in force reaches the owning service, not that one
/// member does.
#[tokio::test]
async fn a_changed_glue_catalog_declaration_is_applied_through_the_owning_service() {
    let router = build_router();
    let region = "us-west-2";
    let stack_name = "glue-catalog-update-stack";
    let database_name = "gcat_update_db";
    let table_name = "gcat_update_table";
    let crawler_name = "gcat-update-crawler";

    let deployments = vec![
        Deployment {
            case: "the first deployment, which creates the three entities",
            database_input: json!({ "Description": "the first description" }),
            table_input: json!({
                "StorageDescriptor": {
                    "Location": "s3://sim2real-lake/v1/",
                    "Columns": [{ "Name": "id", "Type": "bigint" }]
                }
            }),
            crawler_properties: json!({ "TablePrefix": "v1_" }),
            expected_database: vec![("/Description", json!("the first description"))],
            expected_table: vec![
                ("/StorageDescriptor/Location", json!("s3://sim2real-lake/v1/")),
                ("/StorageDescriptor/Columns/0/Name", json!("id")),
            ],
            expected_crawler: vec![("/TablePrefix", json!("v1_"))],
        },
        Deployment {
            case: "a changed description, a column added, a changed location and prefix",
            database_input: json!({
                "Description": "the second description",
                "Parameters": { "classification": "json" }
            }),
            table_input: json!({
                "StorageDescriptor": {
                    "Location": "s3://sim2real-lake/v2/",
                    "Columns": [
                        { "Name": "id", "Type": "bigint" },
                        { "Name": "received_at", "Type": "timestamp" }
                    ]
                }
            }),
            crawler_properties: json!({
                "TablePrefix": "v2_",
                "Schedule": { "ScheduleExpression": "cron(0 1 * * ? *)" }
            }),
            expected_database: vec![
                ("/Description", json!("the second description")),
                ("/Parameters/classification", json!("json")),
            ],
            expected_table: vec![
                ("/StorageDescriptor/Location", json!("s3://sim2real-lake/v2/")),
                (
                    "/StorageDescriptor/Columns/1/Name",
                    json!("received_at"),
                ),
            ],
            expected_crawler: vec![
                ("/TablePrefix", json!("v2_")),
                ("/Schedule/ScheduleExpression", json!("cron(0 1 * * ? *)")),
            ],
        },
        Deployment {
            case: "a description removed and the crawler's description stated instead",
            database_input: json!({ "LocationUri": "s3://sim2real-lake/v3/" }),
            table_input: json!({
                "TableType": "EXTERNAL_TABLE",
                "StorageDescriptor": {
                    "Location": "s3://sim2real-lake/v3/",
                    "Columns": [{ "Name": "id", "Type": "string" }]
                }
            }),
            crawler_properties: json!({
                "TablePrefix": "v2_",
                "Schedule": { "ScheduleExpression": "cron(0 1 * * ? *)" },
                "Description": "the third deployment"
            }),
            expected_database: vec![("/LocationUri", json!("s3://sim2real-lake/v3/"))],
            expected_table: vec![
                ("/TableType", json!("EXTERNAL_TABLE")),
                ("/StorageDescriptor/Columns/0/Type", json!("string")),
            ],
            expected_crawler: vec![("/Description", json!("the third deployment"))],
        },
    ];

    for (index, deployment) in deployments.into_iter().enumerate() {
        let case = deployment.case;
        let mut database_input = deployment
            .database_input
            .as_object()
            .cloned()
            .expect("a declared DatabaseInput");
        database_input.insert("Name".to_string(), json!(database_name));
        let mut table_input = deployment
            .table_input
            .as_object()
            .cloned()
            .expect("a declared TableInput");
        table_input.insert("Name".to_string(), json!(table_name));
        let mut crawler_properties = deployment
            .crawler_properties
            .as_object()
            .cloned()
            .expect("declared crawler properties");
        crawler_properties.insert("Name".to_string(), json!(crawler_name));
        crawler_properties.insert(
            "Role".to_string(),
            json!({ "Fn::GetAtt": [ROLE_LOGICAL_ID, "Arn"] }),
        );
        crawler_properties.insert(
            "DatabaseName".to_string(),
            json!({ "Ref": DATABASE_LOGICAL_ID }),
        );
        crawler_properties.insert(
            "Targets".to_string(),
            json!({ "S3Targets": [{ "Path": "s3://sim2real-lake/" }] }),
        );

        let template = json!({
            "Resources": {
                ROLE_LOGICAL_ID: {
                    "Type": "AWS::IAM::Role",
                    "Properties": { "AssumeRolePolicyDocument": trust_policy() }
                },
                DATABASE_LOGICAL_ID: {
                    "Type": "AWS::Glue::Database",
                    "Properties": { "DatabaseInput": database_input }
                },
                TABLE_LOGICAL_ID: {
                    "Type": "AWS::Glue::Table",
                    "Properties": {
                        "DatabaseName": { "Ref": DATABASE_LOGICAL_ID },
                        "TableInput": table_input
                    }
                },
                CRAWLER_LOGICAL_ID: {
                    "Type": "AWS::Glue::Crawler",
                    "Properties": crawler_properties
                }
            }
        })
        .to_string();

        let action = if index == 0 {
            "CreateStack"
        } else {
            "UpdateStack"
        };
        let (status, body) = cloudformation(
            &router,
            region,
            action,
            &[
                ("StackName", stack_name),
                ("TemplateBody", &template),
                ("Capabilities.member.1", "CAPABILITY_IAM"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
        );
        tokio::time::sleep(PAST_STACK_SPAN).await;

        let (_, described) =
            cloudformation(&router, region, "DescribeStacks", &[("StackName", stack_name)]).await;
        let expected_status = if index == 0 {
            "CREATE_COMPLETE"
        } else {
            "UPDATE_COMPLETE"
        };
        assert_eq!(
            opt_element(&described, "StackStatus").as_deref(),
            Some(expected_status),
            "case {case:?}: expected the stack to reach {expected_status}, got {described}"
        );

        for (operation, request, member, expected) in [
            (
                "GetDatabase",
                json!({ "Name": database_name }),
                "Database",
                deployment.expected_database,
            ),
            (
                "GetTable",
                json!({ "DatabaseName": database_name, "Name": table_name }),
                "Table",
                deployment.expected_table,
            ),
            (
                "GetCrawler",
                json!({ "Name": crawler_name }),
                "Crawler",
                deployment.expected_crawler,
            ),
        ] {
            let entity = read_entity(&router, case, region, operation, request, member).await;
            assert!(
                entity.get("Name").and_then(Value::as_str).is_some(),
                "case {case:?}: expected {member}.Name to be reported, got {entity}"
            );
            for (pointer, wanted) in expected {
                let actual = entity.pointer(pointer).cloned().unwrap_or(Value::Null);
                assert_eq!(
                    actual, wanted,
                    "case {case:?}: expected {member}{pointer} to be {wanted}, got {actual} in \
                     {entity}"
                );
            }
        }
    }
}
