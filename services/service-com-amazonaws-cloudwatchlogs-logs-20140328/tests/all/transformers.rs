// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the log-group level transformer loop:
//! `PutTransformer` / `GetTransformer` / `DeleteTransformer`.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The rule under test: `GetTransformer` reports the log-group level transformer stored
//! on the log group its `logGroupIdentifier` resolves to — the processor pipeline exactly
//! as the last `PutTransformer` configured it, in that order, with the times that put and
//! the one that created it stamped — reports an EMPTY pipeline for a group of this account
//! that carries no transformer (including one whose transformer was deleted), and refuses,
//! as a missing resource, an identifier that names no log group. A pipeline `PutTransformer`
//! will not accept as a transformer at all is refused as a bad parameter: one whose element
//! names other than exactly one processor, and one that does not BEGIN with a parse-type
//! processor or that carries an AWS-vended parser anywhere but first.
//!
//! The tables below span the classes the rule generalises over (the processor KINDS the
//! model's `Processor` structure names, the pipeline LENGTH at both published bounds and
//! one past each, the pipeline ORDER, repetition of one kind, and the identifier FORMS a
//! log group is named by); any single configuration an issue happens to mention is one
//! row.

use serde_json::{json, Value};

use crate::harness::{error_code, now_ms, Logs};

/// The code CloudWatch Logs answers for a parameter it will not accept. Both operations
/// under test declare it and neither declares `ValidationException`, so it is also the
/// code a modelled-constraint refusal carries here (see `tests/all/refusal_error_codes.rs`).
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The code answered when the resource an operation names is not there.
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The code answered when the operation is not valid on the resource it names.
const INVALID_OPERATION: &str = "InvalidOperationException";

/// The published bounds of one `transformerConfig`. Source: the model's `Processors`
/// list shape, `@length(min: 1, max: 20)`, which both `PutTransformer$transformerConfig`
/// and `GetTransformerResponse$transformerConfig` target.
const PROCESSORS_MIN: usize = 1;
const PROCESSORS_MAX: usize = 20;

/// The log group classes the model names. Source: the `LogGroupClass` enum
/// (`DELIVERY` | `INFREQUENT_ACCESS` | `STANDARD`).
const LOG_GROUP_CLASSES: [&str; 3] = ["DELIVERY", "INFREQUENT_ACCESS", "STANDARD"];

/// The one class a transformer can be created on. Source: `PutTransformer` — "You can
/// create transformers only for the log groups in the Standard log class."
const TRANSFORMER_CLASS: &str = "STANDARD";

/// Every processor kind a transformer can be built from, as `(member, configuration)`
/// pairs in the order the model declares them. Source: the members of the model's
/// `Processor` structure — `addKeys`, `copyValue`, `csv`, `dateTimeConverter`,
/// `deleteKeys`, `grok`, `listToMap`, `lowerCaseString`, `moveKeys`, `parseCloudfront`,
/// `parseJSON`, `parseKeyValue`, `parseRoute53`, `parseToOCSF`, `parsePostgres`,
/// `parseVPC`, `parseWAF`, `renameKeys`, `splitString`, `substituteString`, `trimString`,
/// `typeConverter`, `upperCaseString`.
///
/// Each configuration populates EVERY member of that processor's own structure, so a
/// round trip can be compared member for member rather than on the handful of members one
/// recorded request happened to carry. The enumerated values come from the model's own
/// enums: `Type` (`boolean` | `double` | `integer` | `string`), `EventSource` (`AWSWAF` |
/// `CloudTrail` | `EKSAudit` | `Route53Resolver` | `VPCFlow`), `OCSFVersion` (`V1.1` |
/// `V1.5`) and `FlattenedElement` (`first` | `last`).
fn processor_catalog() -> Vec<(&'static str, Value)> {
    vec![
        (
            "addKeys",
            json!({"entries": [{"key": "env", "value": "prod", "overwriteIfExists": true}]}),
        ),
        (
            "copyValue",
            json!({"entries": [{"source": "status", "target": "status_copy", "overwriteIfExists": false}]}),
        ),
        (
            "csv",
            json!({
                "quoteCharacter": "'",
                "delimiter": ";",
                "columns": ["when", "who", "what"],
                "source": "@message",
                "destination": "csv",
            }),
        ),
        (
            "dateTimeConverter",
            json!({
                "source": "when",
                "target": "when_iso",
                "targetFormat": "yyyy-MM-dd'T'HH:mm:ss.SSS'Z'",
                "matchPatterns": ["dd/MM/yyyy", "yyyy-MM-dd"],
                "sourceTimezone": "Europe/London",
                "targetTimezone": "UTC",
                "locale": "en-GB",
            }),
        ),
        ("deleteKeys", json!({"withKeys": ["debug", "trace"]})),
        (
            "grok",
            json!({"source": "@message", "match": "%{COMMONAPACHELOG}"}),
        ),
        (
            "listToMap",
            json!({
                "source": "attributes",
                "key": "name",
                "valueKey": "value",
                "target": "attribute_map",
                "flatten": true,
                "flattenedElement": "last",
            }),
        ),
        ("lowerCaseString", json!({"withKeys": ["level"]})),
        (
            "moveKeys",
            json!({"entries": [{"source": "a", "target": "b", "overwriteIfExists": true}]}),
        ),
        ("parseCloudfront", json!({"source": "@message"})),
        (
            "parseJSON",
            json!({"source": "@message", "destination": "parsed"}),
        ),
        (
            "parseKeyValue",
            json!({
                "source": "@message",
                "destination": "pairs",
                "fieldDelimiter": "&",
                "keyValueDelimiter": "=",
                "keyPrefix": "kv_",
                "nonMatchValue": "unparsed",
                "overwriteIfExists": true,
            }),
        ),
        ("parseRoute53", json!({"source": "@message"})),
        (
            "parseToOCSF",
            json!({
                "source": "@message",
                "eventSource": "CloudTrail",
                "ocsfVersion": "V1.1",
                "mappingVersion": "1.0",
            }),
        ),
        ("parsePostgres", json!({"source": "@message"})),
        ("parseVPC", json!({"source": "@message"})),
        ("parseWAF", json!({"source": "@message"})),
        (
            "renameKeys",
            json!({"entries": [{"key": "lvl", "renameTo": "level", "overwriteIfExists": false}]}),
        ),
        (
            "splitString",
            json!({"entries": [{"source": "path", "delimiter": "/"}]}),
        ),
        (
            "substituteString",
            json!({"entries": [{"source": "message", "from": "secret", "to": "redacted"}]}),
        ),
        ("trimString", json!({"withKeys": ["level", "message"]})),
        (
            "typeConverter",
            json!({"entries": [{"key": "status", "type": "integer"}]}),
        ),
        ("upperCaseString", json!({"withKeys": ["region"]})),
    ]
}

/// The processor kinds a transformer's pipeline may BEGIN with. Source: the catalogue
/// `PutTransformer` holds a pipeline to — a transformer "must have at least one parse-type
/// processor" (the model's `ParseJSON` doc), and the parse-type kinds are the configurable
/// parsers and the parsers for AWS-vended log formats.
pub(crate) const PARSE_TYPE_KINDS: [&str; 10] = [
    "csv",
    "grok",
    "parseCloudfront",
    "parseJSON",
    "parseKeyValue",
    "parsePostgres",
    "parseRoute53",
    "parseToOCSF",
    "parseVPC",
    "parseWAF",
];

/// The processor kinds that may appear only as the FIRST processor of a transformer.
/// Source: the model's own note on each of them — "If you use this processor, it must be the
/// first processor in your transformer" (`ParseCloudfront`, `ParseRoute53`, `ParsePostgres`,
/// `ParseVPC`, `ParseWAF`).
pub(crate) const LEADING_ONLY_KINDS: [&str; 5] = [
    "parseCloudfront",
    "parsePostgres",
    "parseRoute53",
    "parseVPC",
    "parseWAF",
];

/// One processor of the kind `member` names, as a `transformerConfig` element.
fn processor(member: &str) -> Value {
    let (member, configuration) = processor_catalog()
        .into_iter()
        .find(|(name, _)| *name == member)
        .unwrap_or_else(|| panic!("case {member}: the model's Processor declares no such member"));
    json!({member: configuration})
}

/// One processor of every kind in the catalogue, in model order.
fn every_kind() -> Vec<Value> {
    processor_catalog()
        .into_iter()
        .map(|(member, configuration)| json!({member: configuration}))
        .collect()
}

/// The catalogue's kinds that may follow another processor: everything but the AWS-vended
/// parsers, which must lead.
fn followable_kinds() -> Vec<Value> {
    processor_catalog()
        .into_iter()
        .filter(|(member, _)| !LEADING_ONLY_KINDS.contains(member))
        .map(|(member, configuration)| json!({member: configuration}))
        .collect()
}

/// A pipeline of `length` processors led by a parser: the parse-type kind `leading`, then as
/// many followable kinds as the length needs, repeating the catalogue where it runs out.
fn pipeline_of_length(leading: &str, length: usize) -> Vec<Value> {
    let followable = followable_kinds();
    let mut pipeline = vec![processor(leading)];
    while pipeline.len() < length {
        let at = (pipeline.len() - 1) % followable.len();
        pipeline.push(followable[at].clone());
    }
    pipeline.truncate(length.max(1));
    pipeline
}

/// One row of the round-trip table: the group to put on and the pipeline to put.
#[derive(Debug, Clone)]
struct Row {
    /// What this row varies, named in every assertion.
    case: &'static str,
    group: &'static str,
    config: Vec<Value>,
}

/// The pipelines the round trip spans: the list's minimum and maximum length, an order
/// and its reverse, a kind repeated with different parameters, and — across the rows — every
/// processor kind the model names. Every pipeline begins with a parse-type processor, which
/// is what `PutTransformer` accepts a transformer at all for, and the AWS-vended parsers
/// lead the one-processor rows because they may appear nowhere else.
fn round_trip_rows() -> Vec<Row> {
    let mutators: Vec<Value> = ["addKeys", "lowerCaseString", "deleteKeys"]
        .iter()
        .map(|member| processor(member))
        .collect();
    let pipeline: Vec<Value> = std::iter::once(processor("parseJSON"))
        .chain(mutators.iter().cloned())
        .collect();
    let reversed: Vec<Value> = std::iter::once(processor("parseJSON"))
        .chain(mutators.iter().rev().cloned())
        .collect();
    let mut rows = vec![
        Row {
            case: "one processor — the list's minimum length",
            group: "/transformers/minimum",
            config: vec![processor("parseJSON")],
        },
        Row {
            case: "a four-kind pipeline",
            group: "/transformers/pipeline",
            config: pipeline,
        },
        Row {
            case: "the same mutate kinds in the opposite order behind the parser",
            group: "/transformers/pipeline-reversed",
            config: reversed,
        },
        Row {
            case: "one kind repeated with different parameters",
            group: "/transformers/repeated-kind",
            config: vec![
                json!({"grok": {"source": "@message", "match": "%{COMMONAPACHELOG}"}}),
                json!({"grok": {"source": "parsed.detail", "match": "%{NUMBER:code}"}}),
                json!({"grok": {"match": "%{GREEDYDATA:rest}"}}),
            ],
        },
        Row {
            case: "twenty processors — the list's maximum length",
            group: "/transformers/maximum",
            config: pipeline_of_length("parseJSON", PROCESSORS_MAX),
        },
    ];
    // Every kind the rows above cannot carry: each AWS-vended parser leads a pipeline of
    // its own, so the catalogue is covered between the rows.
    for member in LEADING_ONLY_KINDS {
        rows.push(Row {
            case: "a parser that must lead, leading a pipeline of its own",
            group: match member {
                "parseCloudfront" => "/transformers/leading-cloudfront",
                "parsePostgres" => "/transformers/leading-postgres",
                "parseRoute53" => "/transformers/leading-route53",
                "parseVPC" => "/transformers/leading-vpc",
                _ => "/transformers/leading-waf",
            },
            config: vec![processor(member), processor("trimString")],
        });
    }
    rows
}

/// Create one log group, in the class given when one is given.
async fn create_group(logs: &Logs, name: &str, class: Option<&str>) {
    let mut body = json!({"logGroupName": name});
    if let Some(class) = class {
        body["logGroupClass"] = json!(class);
    }
    logs.ok("CreateLogGroup", &body).await;
}

/// The `PutTransformer` body for one group and pipeline.
fn put_body(identifier: &str, config: &[Value]) -> Value {
    json!({"logGroupIdentifier": identifier, "transformerConfig": config})
}

/// The `transformerConfig` a `GetTransformer` response reports.
fn reported_config(response: &Value) -> Vec<Value> {
    response
        .get("transformerConfig")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("GetTransformer must return transformerConfig, got {response}")
        })
        .clone()
}

/// Assert that the log group `identifier` names carries NO log-group level transformer: the
/// read succeeds and reports an empty `transformerConfig`, and no transformer time with it.
async fn assert_no_transformer(logs: &Logs, case: &str, identifier: &str) {
    let found = logs
        .ok("GetTransformer", &json!({"logGroupIdentifier": identifier}))
        .await;
    assert_eq!(
        reported_config(&found),
        Vec::<Value>::new(),
        "case {case}: a group carrying no transformer reports an empty transformerConfig, \
         got {found}"
    );
    for member in ["creationTime", "lastModifiedTime"] {
        assert!(
            found.get(member).is_none(),
            "case {case}: there is no transformer, so no {member} is reported, got {found}"
        );
    }
}

/// A timestamp member of a `GetTransformer` response.
fn reported_time(response: &Value, member: &str) -> i64 {
    response
        .get(member)
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("GetTransformer must return {member}, got {response}"))
}

/// Every accepted put is readable back through `GetTransformer`, under every form the log
/// group is named by, with the pipeline it configured — every member of every processor,
/// in the order put — the ARN of the group it was created for, and a live creation time
/// that a first put makes the last-modified time too.
#[tokio::test]
async fn put_is_readable_back_through_get_transformer() {
    let logs = Logs::new();

    // Every kind the catalogue names is carried by some row, so the round trip covers the
    // whole of `Processor` rather than the kinds one pipeline happens to fit.
    let mut covered: Vec<String> = Vec::new();
    for row in round_trip_rows() {
        for element in &row.config {
            covered.extend(
                element
                    .as_object()
                    .into_iter()
                    .flat_map(|members| members.keys().cloned()),
            );
        }
    }
    for element in every_kind() {
        let member = element
            .as_object()
            .and_then(|members| members.keys().next().cloned())
            .unwrap_or_default();
        assert!(
            covered.contains(&member),
            "case {member}: every processor kind the catalogue names is round-tripped by some \
             row, and this one is not among {covered:?}"
        );
    }

    for row in round_trip_rows() {
        assert!(
            row.config.len() >= PROCESSORS_MIN && row.config.len() <= PROCESSORS_MAX,
            "case {}: the row's pipeline must be within the modelled bounds, got {} processors",
            row.case,
            row.config.len()
        );
        create_group(&logs, row.group, None).await;
        let (_wildcard_arn, group_arn) = logs.group_arns(row.group).await;

        let before = now_ms();
        logs.ok("PutTransformer", &put_body(row.group, &row.config))
            .await;
        let after = now_ms();

        // The name and the ARN name the same log group, so both read the same
        // transformer back. The `LogGroup$arn` form that carries a trailing `:*` is not a
        // third form here: `*` is outside the modelled `LogGroupIdentifier` pattern, so
        // the boundary refuses it before any handler runs (asserted below).
        for identifier in [row.group, group_arn.as_str()] {
            let found = logs
                .ok("GetTransformer", &json!({"logGroupIdentifier": identifier}))
                .await;

            assert_eq!(
                reported_config(&found),
                row.config,
                "case {} by {identifier}: the stored pipeline must be reported verbatim and in order, got {found}",
                row.case
            );
            assert_eq!(
                found.get("logGroupIdentifier").and_then(Value::as_str),
                Some(group_arn.as_str()),
                "case {} by {identifier}: the reported identifier is the ARN of the log group, got {found}",
                row.case
            );

            let creation = reported_time(&found, "creationTime");
            let modified = reported_time(&found, "lastModifiedTime");
            assert!(
                (before..=after).contains(&creation),
                "case {} by {identifier}: creationTime must be the time of the put, expected {before}..={after}, got {creation}",
                row.case
            );
            assert_eq!(
                modified, creation,
                "case {} by {identifier}: a transformer never updated was last modified when it was created, got {found}",
                row.case
            );
        }
    }
}

/// A put over a group that already has a transformer replaces the pipeline wholesale —
/// no member of the previous one survives — and keeps the creation time while moving the
/// last-modified time to this put.
#[tokio::test]
async fn put_over_an_existing_transformer_replaces_it_and_keeps_its_creation_time() {
    let logs = Logs::new();
    let group = "/transformers/updated";
    create_group(&logs, group, None).await;

    let first = vec![processor("parseJSON"), processor("addKeys")];
    let before = now_ms();
    logs.ok("PutTransformer", &put_body(group, &first)).await;
    let after_first = now_ms();
    let created = logs
        .ok("GetTransformer", &json!({"logGroupIdentifier": group}))
        .await;
    let creation = reported_time(&created, "creationTime");

    // Each later put is a different pipeline: a shorter one, then a longer one, then one
    // of a kind neither named.
    let updates = vec![
        vec![processor("grok")],
        vec![
            processor("csv"),
            processor("trimString"),
            processor("typeConverter"),
        ],
        vec![processor("parseToOCSF")],
    ];
    for update in updates {
        let before_update = now_ms();
        logs.ok("PutTransformer", &put_body(group, &update)).await;
        let after_update = now_ms();

        let found = logs
            .ok("GetTransformer", &json!({"logGroupIdentifier": group}))
            .await;
        assert_eq!(
            reported_config(&found),
            update,
            "case {update:?}: an update replaces the whole pipeline, got {found}"
        );
        assert_eq!(
            reported_time(&found, "creationTime"),
            creation,
            "case {update:?}: an update keeps the transformer's creation time, got {found}"
        );
        let modified = reported_time(&found, "lastModifiedTime");
        assert!(
            (before_update..=after_update).contains(&modified),
            "case {update:?}: lastModifiedTime must be the time of this put, expected {before_update}..={after_update}, got {modified}"
        );
        assert!(
            modified >= creation,
            "case {update:?}: a transformer is modified no earlier than it was created, got {found}"
        );
    }

    assert!(
        (before..=after_first).contains(&creation),
        "the creation time is the time of the first put, expected {before}..={after_first}, got {creation}"
    );
}

/// `GetTransformer` reports an EMPTY configuration for every read that names no stored
/// log-group level transformer on a group the account holds — a group that never had one, and
/// a group whose transformer went with it when the group was deleted — and refuses, as a
/// missing resource, a read that names no log group at all.
#[tokio::test]
async fn get_transformer_reports_an_empty_configuration_where_no_transformer_is_stored() {
    let logs = Logs::new();
    let configured = "/transformers/configured";
    let bare = "/transformers/bare";
    create_group(&logs, configured, None).await;
    create_group(&logs, bare, None).await;
    let (wildcard_arn, group_arn) = logs.group_arns(configured).await;
    let (_bare_wildcard, bare_arn) = logs.group_arns(bare).await;
    logs.ok(
        "PutTransformer",
        &put_body(configured, &[processor("parseJSON")]),
    )
    .await;

    // A group the account holds that has no transformer of its own, named both ways: the
    // configuration it reports is empty.
    for (case, identifier) in [
        ("a group that never had a transformer, by name", bare.to_string()),
        ("a group that never had a transformer, by ARN", bare_arn),
    ] {
        assert_no_transformer(&logs, case, &identifier).await;
    }

    // A name and an ARN that name no log group at all: the missing resource is the GROUP,
    // which is refused.
    for (case, identifier) in [
        ("a name no group was created under", "/transformers/missing".to_string()),
        (
            "an ARN no group was created under",
            group_arn.replace("configured", "never-created"),
        ),
    ] {
        let code = logs
            .err("GetTransformer", &json!({"logGroupIdentifier": identifier}))
            .await;
        assert_eq!(
            code, RESOURCE_NOT_FOUND,
            "case {case}: expected {RESOURCE_NOT_FOUND}, got {code}"
        );
    }

    // The `LogGroup$arn` form carries a trailing `:*`, and `*` is outside the
    // `LogGroupIdentifier` pattern (`^[\w#+=/:,.@-]*$`), so that form is refused for the
    // constraint it violates rather than looked up at all.
    let code = logs
        .err(
            "GetTransformer",
            &json!({"logGroupIdentifier": wildcard_arn}),
        )
        .await;
    assert_eq!(
        code, INVALID_PARAMETER,
        "case the wildcard ARN form: expected {INVALID_PARAMETER}, got {code}"
    );

    // The transformer lives on its log group, so deleting the group takes it away, and a
    // group recreated under the same name starts with none.
    for identifier in [configured, group_arn.as_str()] {
        let found = logs
            .ok("GetTransformer", &json!({"logGroupIdentifier": identifier}))
            .await;
        assert_eq!(
            reported_config(&found).len(),
            1,
            "case {identifier}: the group put on reports its own transformer, got {found}"
        );
    }
    logs.ok("DeleteLogGroup", &json!({"logGroupName": configured}))
        .await;
    create_group(&logs, configured, None).await;
    assert_no_transformer(&logs, "a recreated log group", configured).await;
}

/// `PutTransformer` refuses a group that is not there, and leaves no transformer behind
/// for a group created under that name afterwards.
#[tokio::test]
async fn put_transformer_refuses_a_log_group_that_is_not_there() {
    let logs = Logs::new();
    let group = "/transformers/not-created-yet";

    for identifier in [
        group.to_string(),
        format!(
            "arn:aws:logs:eu-west-2:{}:log-group:{group}",
            engine::emulator_account_id()
        ),
    ] {
        let code = logs
            .err("PutTransformer", &put_body(&identifier, &[processor("grok")]))
            .await;
        assert_eq!(
            code, RESOURCE_NOT_FOUND,
            "case {identifier}: expected {RESOURCE_NOT_FOUND}, got {code}"
        );
    }

    create_group(&logs, group, None).await;
    assert_no_transformer(&logs, "a refused put stores nothing", group).await;
}

/// Only a log group in the Standard log class can carry a transformer: every other class
/// the model names is refused as the operation being invalid on that resource, and
/// nothing is stored for it.
#[tokio::test]
async fn put_transformer_refuses_a_log_group_outside_the_standard_class() {
    let logs = Logs::new();

    for class in LOG_GROUP_CLASSES {
        let group = format!("/transformers/class-{class}");
        create_group(&logs, &group, Some(class)).await;
        let body = put_body(&group, &[processor("parseJSON")]);

        if class == TRANSFORMER_CLASS {
            logs.ok("PutTransformer", &body).await;
            let found = logs
                .ok("GetTransformer", &json!({"logGroupIdentifier": group}))
                .await;
            assert_eq!(
                reported_config(&found).len(),
                1,
                "case class {class}: a Standard-class group carries its transformer, got {found}"
            );
            continue;
        }

        let code = logs.err("PutTransformer", &body).await;
        assert_eq!(
            code, INVALID_OPERATION,
            "case class {class}: expected {INVALID_OPERATION}, got {code}"
        );
        assert_no_transformer(&logs, &format!("class {class}, a refused put"), &group).await;
    }

    // A group created with no `logGroupClass` at all is in the Standard class, so it
    // carries a transformer too.
    let defaulted = "/transformers/class-defaulted";
    create_group(&logs, defaulted, None).await;
    logs.ok(
        "PutTransformer",
        &put_body(defaulted, &[processor("parseJSON")]),
    )
    .await;
    let found = logs
        .ok("GetTransformer", &json!({"logGroupIdentifier": defaulted}))
        .await;
    assert_eq!(
        reported_config(&found).len(),
        1,
        "case the default log class: the group carries its transformer, got {found}"
    );
}

/// A transformer's pipeline must BEGIN with a parse-type processor — a transformer "must
/// have at least one parse-type processor", and the processors "work one after another, in
/// the order that you list them", so a first processor that is a mutation step structures
/// nothing for the rest to read — and a parser for an AWS-vended log format may appear
/// nowhere but first ("If you use this processor, it must be the first processor in your
/// transformer"). Each row drives the whole catalogue: every parse-type kind leads a pipeline
/// that is accepted, every other kind leading one that is refused, and every must-lead kind
/// at a later position refused too. A refused put leaves the stored transformer as it was.
#[tokio::test]
async fn put_transformer_requires_a_pipeline_that_begins_with_a_parser() {
    let logs = Logs::new();
    let group = "/transformers/leading-parser";
    create_group(&logs, group, None).await;

    let stored = vec![processor("parseJSON")];
    logs.ok("PutTransformer", &put_body(group, &stored)).await;

    for (member, _) in processor_catalog() {
        let parses = PARSE_TYPE_KINDS.contains(&member);
        // A pipeline led by this kind, with a mutate processor behind it so the row is a
        // pipeline rather than a single processor.
        let led = vec![processor(member), processor("trimString")];

        if parses {
            logs.ok("PutTransformer", &put_body(group, &led)).await;
            let found = logs
                .ok("GetTransformer", &json!({"logGroupIdentifier": group}))
                .await;
            assert_eq!(
                reported_config(&found),
                led,
                "case {member} leading: a parse-type processor may lead a transformer, got {found}"
            );
            logs.ok("PutTransformer", &put_body(group, &stored)).await;
            continue;
        }

        let (code, message) = logs.refusal("PutTransformer", &put_body(group, &led)).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {member} leading: expected {INVALID_PARAMETER}, got {code}"
        );
        assert!(
            message.contains(member) && message.contains("parse-type"),
            "case {member} leading: the refusal states the rule and the kind the pipeline \
             begins with, got {message:?}"
        );
        let found = logs
            .ok("GetTransformer", &json!({"logGroupIdentifier": group}))
            .await;
        assert_eq!(
            reported_config(&found),
            stored,
            "case {member} leading: a refused put changes nothing, got {found}"
        );
    }

    // A must-lead parser anywhere but first, behind each of two different leading parsers,
    // so what is refused is the position rather than one pairing.
    for member in LEADING_ONLY_KINDS {
        for leading in ["parseJSON", "csv"] {
            let config = vec![processor(leading), processor(member)];
            let (code, message) = logs
                .refusal("PutTransformer", &put_body(group, &config))
                .await;
            assert_eq!(
                code, INVALID_PARAMETER,
                "case {member} behind {leading}: expected {INVALID_PARAMETER}, got {code}"
            );
            assert!(
                message.contains(member),
                "case {member} behind {leading}: the refusal names the processor that must \
                 lead, got {message:?}"
            );
            let found = logs
                .ok("GetTransformer", &json!({"logGroupIdentifier": group}))
                .await;
            assert_eq!(
                reported_config(&found),
                stored,
                "case {member} behind {leading}: a refused put changes nothing, got {found}"
            );
        }
    }
}

/// Each element of a `transformerConfig` names exactly ONE transformation — `Processor`
/// is "the information about one processor in a log transformer" — so an element naming
/// none, and one naming several, are refused, wherever in the pipeline they sit, and the
/// transformer the group already had is left as it was.
#[tokio::test]
async fn put_transformer_refuses_a_processor_that_does_not_name_one_transformation() {
    let logs = Logs::new();
    let group = "/transformers/one-per-processor";
    create_group(&logs, group, None).await;

    let stored = vec![processor("parseJSON")];
    logs.ok("PutTransformer", &put_body(group, &stored)).await;

    let refused: Vec<(&str, Vec<Value>)> = vec![
        ("an element naming no transformation", vec![json!({})]),
        (
            "an element naming no transformation after a valid one",
            vec![processor("grok"), json!({})],
        ),
        (
            "an element naming two transformations",
            vec![json!({
                "parseJSON": {"source": "@message", "destination": "parsed"},
                "grok": {"match": "%{GREEDYDATA:rest}"},
            })],
        ),
        (
            "an element naming three transformations",
            vec![json!({
                "lowerCaseString": {"withKeys": ["level"]},
                "upperCaseString": {"withKeys": ["region"]},
                "trimString": {"withKeys": ["message"]},
            })],
        ),
    ];
    for (case, config) in refused {
        let (code, message) = logs.refusal("PutTransformer", &put_body(group, &config)).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case}: expected {INVALID_PARAMETER}, got {code}"
        );
        assert!(
            message.contains("transformerConfig"),
            "case {case}: the refusal names the member it was built from, got {message:?}"
        );

        let found = logs
            .ok("GetTransformer", &json!({"logGroupIdentifier": group}))
            .await;
        assert_eq!(
            reported_config(&found),
            stored,
            "case {case}: a refused put changes nothing, got {found}"
        );
    }
}

/// A pipeline outside the published length bounds is refused, at either end, and the
/// bounds themselves are accepted.
#[tokio::test]
async fn transformer_config_length_is_held_to_its_published_bounds() {
    let logs = Logs::new();
    let group = "/transformers/bounds";
    create_group(&logs, group, None).await;

    let over_the_maximum: Vec<Value> = (0..=PROCESSORS_MAX)
        .map(|at| {
            json!({"grok": {"source": format!("field_{at}"), "match": "%{GREEDYDATA:rest}"}})
        })
        .collect();

    for (case, config) in [
        ("an empty pipeline, one under the minimum", Vec::new()),
        ("one processor past the maximum", over_the_maximum),
    ] {
        let (status, parsed) = logs
            .call("PutTransformer", &put_body(group, &config))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: a pipeline of {} processors is outside {PROCESSORS_MIN}..={PROCESSORS_MAX}, expected HTTP 400, got {status} {parsed}",
            config.len()
        );
        let code = error_code(&parsed);
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case}: expected {INVALID_PARAMETER}, got {code}"
        );
    }

    for (case, config) in [
        ("the minimum", pipeline_of_length("parseJSON", PROCESSORS_MIN)),
        ("the maximum", pipeline_of_length("grok", PROCESSORS_MAX)),
    ] {
        logs.ok("PutTransformer", &put_body(group, &config)).await;
        let found = logs
            .ok("GetTransformer", &json!({"logGroupIdentifier": group}))
            .await;
        assert_eq!(
            reported_config(&found),
            config,
            "case {case}: a pipeline at the bound round-trips, got {found}"
        );
    }
}

/// `DeleteTransformer` removes the transformer the group carries — "as soon as you do
/// this, the transformation of incoming log events according to that transformer stops" —
/// so the group stops reporting one and a later put configures a NEW transformer, created
/// at the time of that put. A delete that names no stored transformer is refused as a
/// missing resource, and leaves every other group's transformer alone.
#[tokio::test]
async fn delete_transformer_removes_the_one_the_group_carries() {
    let logs = Logs::new();
    let group = "/transformers/deleted";
    let other = "/transformers/kept";
    create_group(&logs, group, None).await;
    create_group(&logs, other, None).await;
    let (_wildcard_arn, group_arn) = logs.group_arns(group).await;
    let kept = vec![processor("grok")];
    logs.ok("PutTransformer", &put_body(other, &kept)).await;

    // The name and the ARN name the same group, so a delete through either takes the
    // transformer away; each round re-puts one so the next form has something to delete.
    for identifier in [group.to_string(), group_arn.clone()] {
        let config = vec![processor("parseJSON"), processor("trimString")];
        logs.ok("PutTransformer", &put_body(&identifier, &config))
            .await;
        let created = logs
            .ok("GetTransformer", &json!({"logGroupIdentifier": group}))
            .await;
        let first_creation = reported_time(&created, "creationTime");

        logs.ok(
            "DeleteTransformer",
            &json!({"logGroupIdentifier": identifier}),
        )
        .await;

        assert_no_transformer(&logs, &format!("{identifier}, after the delete"), group).await;

        // A delete of a transformer that is no longer there names a missing resource.
        let code = logs
            .err(
                "DeleteTransformer",
                &json!({"logGroupIdentifier": identifier}),
            )
            .await;
        assert_eq!(
            code, RESOURCE_NOT_FOUND,
            "case {identifier}: deleting twice expected {RESOURCE_NOT_FOUND}, got {code}"
        );

        // What a put after the delete creates is a new transformer, not the old one
        // restored: its creation time is the time of that put.
        let before = now_ms();
        logs.ok("PutTransformer", &put_body(&identifier, &config))
            .await;
        let after = now_ms();
        let recreated = logs
            .ok("GetTransformer", &json!({"logGroupIdentifier": group}))
            .await;
        let creation = reported_time(&recreated, "creationTime");
        assert!(
            (before..=after).contains(&creation),
            "case {identifier}: a put after a delete creates a new transformer, expected creationTime {before}..={after}, got {creation} (the deleted one was created at {first_creation})"
        );
        logs.ok(
            "DeleteTransformer",
            &json!({"logGroupIdentifier": identifier}),
        )
        .await;
    }

    // A delete of a group that is not there, and of a group that never had a transformer.
    create_group(&logs, "/transformers/never-configured", None).await;
    for (case, identifier) in [
        ("a group that is not there", "/transformers/no-such-group"),
        (
            "a group that never had a transformer",
            "/transformers/never-configured",
        ),
    ] {
        let code = logs
            .err(
                "DeleteTransformer",
                &json!({"logGroupIdentifier": identifier}),
            )
            .await;
        assert_eq!(
            code, RESOURCE_NOT_FOUND,
            "case {case}: expected {RESOURCE_NOT_FOUND}, got {code}"
        );
    }

    // Nothing above touched the other group's transformer.
    let found = logs
        .ok("GetTransformer", &json!({"logGroupIdentifier": other}))
        .await;
    assert_eq!(
        reported_config(&found),
        kept,
        "case {other}: a delete on one group leaves another group's transformer alone, got {found}"
    );
}

/// A transformer belongs to the one log group it was put on: putting one on a group
/// leaves every other group reporting its own, and nothing where it has none.
#[tokio::test]
async fn a_transformer_is_scoped_to_the_log_group_it_was_put_on() {
    let logs = Logs::new();
    let groups = [
        ("/transformers/scoped-a", vec![processor("parseJSON")]),
        (
            "/transformers/scoped-b",
            vec![processor("grok"), processor("trimString")],
        ),
        ("/transformers/scoped-c", Vec::new()),
    ];

    for (group, config) in &groups {
        create_group(&logs, group, None).await;
        if !config.is_empty() {
            logs.ok("PutTransformer", &put_body(group, config)).await;
        }
    }

    for (group, config) in &groups {
        if config.is_empty() {
            assert_no_transformer(&logs, group, group).await;
            continue;
        }
        let found = logs
            .ok("GetTransformer", &json!({"logGroupIdentifier": group}))
            .await;
        assert_eq!(
            reported_config(&found),
            *config,
            "case {group}: each group reports the pipeline put on IT, got {found}"
        );
    }
}
