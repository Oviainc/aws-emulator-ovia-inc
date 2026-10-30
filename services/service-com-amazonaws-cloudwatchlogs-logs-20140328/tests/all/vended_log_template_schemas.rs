// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the RECORD SCHEMA side of the vended-log delivery configuration
//! templates `DescribeConfigurationTemplates` publishes.
//!
//! The rule under test: a template says what a delivery of ONE log type of ONE kind of
//! resource to ONE kind of destination may carry, and the two halves of that come from
//! different places. The SENDING service decides the record — `allowedFields` is the schema
//! of the records that log type carries, in the order `recordFields` restates them, with the
//! fields a delivery may not drop marked mandatory, and the S3 Tables datasource is published
//! for the log types that have one — so a template never advertises the
//! CloudWatch-Logs-generated `@`-prefixed fields of an INGESTED event for a log type some
//! other service sends. The DESTINATION decides the packaging — the output formats, the field
//! delimiters and the S3 suffix path and its variables — so those follow from the destination
//! type alone and from nothing about the request.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so what is
//! asserted here is what a client reads off the wire.
//!
//! The table spans the rule rather than one request: one row per published log source, across
//! senders whose records are a generic vended-log envelope, an all-mandatory event document,
//! an OpenTelemetry log record, a fixed-shape load-balancer line and a service-specific
//! family; and the packaging invariants are checked over the WHOLE catalogue, at every
//! destination type the model names.

use serde_json::{json, Value};

use crate::harness::Logs;

/// The operation under test.
const DESCRIBE: &str = "DescribeConfigurationTemplates";

/// The largest page the model admits (`DescribeLimit` is `range 1..50`), so one filtered
/// enumeration below is read in a single call.
const MAX_LIMIT: usize = 50;

/// The `DeliveryDestinationType`s the model's enum names
/// (`smithy_query(query="shape",shape="DeliveryDestinationType")`).
const DESTINATION_TYPES: [&str; 4] = ["CWL", "FH", "S3", "XRAY"];

/// The `OutputFormat`s the operation publishes for each destination type: a destination that
/// stores one structured record for each delivered item takes the structured format alone, a
/// destination that receives a stream of records takes each per-record encoding, and a
/// delivery that writes objects takes those plus the columnar file layout.
const RECORD_DESTINATION_FORMATS: [&str; 1] = ["json"];
const STREAM_DESTINATION_FORMATS: [&str; 3] = ["plain", "json", "w3c"];
const OBJECT_DESTINATION_FORMATS: [&str; 4] = ["plain", "json", "w3c", "parquet"];

/// The field delimiters the operation publishes for a delivery that delivers in a delimited
/// format, and the one a template defaults to (the first).
const FIELD_DELIMITERS: [&str; 3] = ["\t", " ", ","];

/// One published log source, and the record schema of its log type: the field names in the
/// order `recordFields` restates them, each paired with whether a delivery MUST carry it.
///
/// Stated here rather than imported from the catalogue, so an edit to a published schema
/// fails this suite instead of agreeing with itself.
type Schema = &'static [(&'static str, bool)];
struct Row {
    service: &'static str,
    resource_type: &'static str,
    log_type: &'static str,
    schema: Schema,
}

/// One row per log source the catalogue publishes a distinct record schema for.
const ROWS: &[Row] = &[
    // A generic vended-log envelope: the record's own timestamp, the ARN and id of the
    // resource that sent it and the instant of the event, then that sender's dimensions and
    // the logged content.
    Row {
        service: "aidevops",
        resource_type: "agent-space",
        log_type: "APPLICATION_LOGS",
        schema: &[
            ("timestamp", false),
            ("resource_arn", true),
            ("resource_id", false),
            ("event_timestamp", true),
            ("log_level", false),
            ("operation", false),
            ("request_id", false),
            ("message", true),
        ],
    },
    Row {
        service: "apigateway",
        resource_type: "stages",
        log_type: "EXECUTION_LOGS",
        schema: &[
            ("timestamp", false),
            ("resource_arn", true),
            ("resource_id", false),
            ("event_timestamp", true),
            ("api_id", false),
            ("stage", false),
            ("resource_path", false),
            ("http_method", false),
            ("payload", true),
        ],
    },
    Row {
        service: "autoloop",
        resource_type: "subscription",
        log_type: "ERROR_LOGS",
        schema: &[
            ("timestamp", false),
            ("resource_arn", true),
            ("resource_id", false),
            ("event_timestamp", true),
            ("transformer_arn", false),
            ("object_identifier", false),
            ("event_identifier", false),
            ("trace_identifier", false),
            ("log_message", true),
        ],
    },
    Row {
        service: "eks",
        resource_type: "cluster",
        log_type: "AUTO_MODE_LOAD_BALANCING_LOGS",
        schema: &[
            ("timestamp", false),
            ("resource_arn", true),
            ("resource_id", false),
            ("event_timestamp", true),
            ("level", false),
            ("logger", false),
            ("controller", false),
            ("controllerKind", false),
            ("reconcileID", false),
            ("message", true),
        ],
    },
    Row {
        service: "events",
        resource_type: "subscriber",
        log_type: "ERROR_LOGS",
        schema: &[
            ("timestamp", false),
            ("resource_arn", true),
            ("resource_id", false),
            ("event_timestamp", true),
            ("subscriber_name", false),
            ("event_id", false),
            ("invocation_id", false),
            ("message_type", false),
            ("log_level", false),
            ("payload", true),
        ],
    },
    // One event document, every field mandatory: a delivery that drops any part of it is not
    // that document.
    Row {
        service: "bedrock",
        resource_type: "agent-alias",
        log_type: "EVENT_LOGS",
        schema: &[
            ("timestamp", true),
            ("resourceId", true),
            ("traceId", true),
            ("spanId", true),
            ("sessionId", true),
            ("requestId", true),
            ("operation", true),
            ("attributes", true),
            ("body", true),
            ("eventType", true),
            ("eventVersion", true),
            ("eventName", true),
            ("level", true),
        ],
    },
    // An OpenTelemetry log record.
    Row {
        service: "bedrock-agentcore",
        resource_type: "workload-identity",
        log_type: "APPLICATION_LOGS",
        schema: &[
            ("timeUnixNano", true),
            ("severityNumber", false),
            ("severityText", false),
            ("body", true),
            ("traceId", false),
            ("spanId", false),
        ],
    },
    // A fixed-shape load-balancer line.
    Row {
        service: "elasticloadbalancing",
        resource_type: "loadbalancer",
        log_type: "ALB_CONNECTION_LOGS",
        schema: &[
            ("time", true),
            ("client_ip", true),
            ("client_port", true),
            ("listener_port", true),
            ("tls_protocol", true),
            ("tls_cipher", true),
            ("tls_handshake_latency", true),
            ("leaf_client_cert_subject", true),
            ("leaf_client_cert_validity", true),
            ("leaf_client_cert_serial_number", true),
            ("tls_verify_status", true),
            ("conn_trace_id", true),
        ],
    },
    // Service-specific families.
    Row {
        service: "mediatailor",
        resource_type: "playbackConfiguration",
        log_type: "AD_DECISION_SERVER_LOGS",
        schema: &[
            ("eventTimestamp", true),
            ("awsAccountId", true),
            ("eventType", false),
            ("sessionId", false),
            ("originId", false),
            ("playbackConfigurationName", false),
            ("vastResponse", false),
            ("vastAds", false),
            ("adBreaks", false),
            ("beaconInfo", false),
            ("trackingEvents", false),
        ],
    },
    Row {
        service: "workmail",
        resource_type: "organization",
        log_type: "ACCESS_CONTROL_LOGS",
        schema: &[
            ("timestamp", false),
            ("organization_arn", true),
            ("event_timestamp", true),
            ("user_id", false),
            ("protocol", false),
            ("source_ip", false),
            ("scope", false),
            ("access_granted", true),
        ],
    },
];

/// Every template of one (service, log type, resource type) slice, keyed by its delivery
/// destination type.
async fn slice(logs: &Logs, service: &str, log_type: &str, resource_type: &str) -> Vec<Value> {
    let body = json!({
        "service": service,
        "logTypes": [log_type],
        "resourceTypes": [resource_type],
        "limit": MAX_LIMIT,
    });
    let response = logs.ok(DESCRIBE, &body).await;
    response
        .get("configurationTemplates")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {body}: the response carries configurationTemplates, got {response}"))
        .clone()
}

/// A list-of-strings member of a reported template.
fn strings(template: &Value, member: &str, case: &str) -> Vec<String> {
    template
        .get(member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: a template carries {member}, got {template}"))
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: {member} holds strings, got {value}"))
                .to_string()
        })
        .collect()
}

/// The `allowedFields` of a reported template, as (name, mandatory) pairs.
fn allowed_fields(template: &Value, case: &str) -> Vec<(String, bool)> {
    template
        .get("allowedFields")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: a template carries allowedFields, got {template}"))
        .iter()
        .map(|field| {
            let name = field
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: an allowed field carries a name, got {field}"))
                .to_string();
            let mandatory = field
                .get("mandatory")
                .and_then(Value::as_bool)
                .unwrap_or_else(|| {
                    panic!("case {case}: an allowed field says whether it is mandatory, got {field}")
                });
            (name, mandatory)
        })
        .collect()
}

/// Each published log source's templates carry THAT SENDER'S record schema, at every delivery
/// destination type the log type can be sent to, and the default record field list restates it
/// in order.
#[tokio::test]
async fn every_log_source_publishes_its_own_record_schema() {
    let logs = Logs::new();

    for row in ROWS {
        let case = format!("{}|{}|{}", row.service, row.log_type, row.resource_type);
        let templates = slice(&logs, row.service, row.log_type, row.resource_type).await;
        assert!(
            !templates.is_empty(),
            "case {case}: the catalogue publishes templates for this log source, got none"
        );
        let expected: Vec<(String, bool)> = row
            .schema
            .iter()
            .map(|(name, mandatory)| ((*name).to_string(), *mandatory))
            .collect();
        for template in &templates {
            let destination_type = template
                .get("deliveryDestinationType")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let case = format!("{case}|{destination_type}");
            let reported = allowed_fields(template, &case);
            assert_eq!(
                reported, expected,
                "case {case}: expected the allowed fields {expected:?}, got {reported:?}"
            );
            let defaults = template
                .get("defaultDeliveryConfigValues")
                .unwrap_or_else(|| panic!("case {case}: a template carries defaultDeliveryConfigValues, got {template}"));
            let default_fields = strings(defaults, "recordFields", &case);
            let names: Vec<String> = expected.iter().map(|(name, _)| name.clone()).collect();
            assert_eq!(
                default_fields, names,
                "case {case}: the default record fields are the allowed fields in order, got {default_fields:?}"
            );
        }
    }
}

/// No template advertises a field that CloudWatch Logs generates for an INGESTED event: those
/// `@`-prefixed fields are the schema of an event in a log group, not of a record a vending
/// service sends, so a delivery built from a template never names one.
#[tokio::test]
async fn no_template_advertises_an_ingested_events_generated_fields() {
    let logs = Logs::new();
    let mut token: Option<String> = None;
    let mut seen = 0usize;
    loop {
        let mut body = json!({"limit": MAX_LIMIT});
        if let Some(token) = &token {
            body["nextToken"] = json!(token);
        }
        let response = logs.ok(DESCRIBE, &body).await;
        let page = response
            .get("configurationTemplates")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("the response carries configurationTemplates, got {response}"))
            .clone();
        for template in &page {
            let case = [
                template.get("service").and_then(Value::as_str).unwrap_or_default(),
                template.get("logType").and_then(Value::as_str).unwrap_or_default(),
                template.get("resourceType").and_then(Value::as_str).unwrap_or_default(),
                template
                    .get("deliveryDestinationType")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ]
            .join("|");
            seen += 1;
            for (name, _mandatory) in allowed_fields(template, &case) {
                assert!(
                    !name.starts_with('@'),
                    "case {case}: a published field is a record field of the sending service, got {name:?}"
                );
            }
        }
        match response.get("nextToken").and_then(Value::as_str) {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    assert!(
        seen > MAX_LIMIT,
        "the whole catalogue is under test, got {seen} templates"
    );
}

/// What a delivery may be PACKAGED as follows from the destination type alone: the published
/// output formats, the delimiters that apply exactly where a delimited format does and the
/// default taken from them.
#[tokio::test]
async fn the_packaging_a_template_publishes_follows_from_its_destination_type() {
    let logs = Logs::new();

    for destination_type in DESTINATION_TYPES {
        let body = json!({"deliveryDestinationTypes": [destination_type], "limit": MAX_LIMIT});
        let response = logs.ok(DESCRIBE, &body).await;
        let page = response
            .get("configurationTemplates")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("the response carries configurationTemplates, got {response}"))
            .clone();
        assert!(
            !page.is_empty(),
            "case {destination_type}: every destination type the model names has templates, got none"
        );
        let expected_formats: Vec<String> = match destination_type {
            "CWL" | "XRAY" => RECORD_DESTINATION_FORMATS.iter(),
            "S3" => OBJECT_DESTINATION_FORMATS.iter(),
            _ => STREAM_DESTINATION_FORMATS.iter(),
        }
        .map(|format| (*format).to_string())
        .collect();
        let expected_delimiters: Vec<String> = FIELD_DELIMITERS
            .iter()
            .map(|delimiter| (*delimiter).to_string())
            .collect();
        let delimited = destination_type == "S3" || destination_type == "FH";

        for template in &page {
            let case = format!("{destination_type} template {template}");
            let formats = strings(template, "allowedOutputFormats", &case);
            assert_eq!(
                formats, expected_formats,
                "case {case}: expected the output formats {expected_formats:?}, got {formats:?}"
            );
            let delimiters = template
                .get("allowedFieldDelimiters")
                .map(|_| strings(template, "allowedFieldDelimiters", &case));
            let default_delimiter = template
                .get("defaultDeliveryConfigValues")
                .and_then(|defaults| defaults.get("fieldDelimiter"))
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if delimited {
                assert_eq!(
                    delimiters.as_deref(),
                    Some(expected_delimiters.as_slice()),
                    "case {case}: expected the delimiters {expected_delimiters:?}, got {delimiters:?}"
                );
                assert_eq!(
                    default_delimiter.as_deref(),
                    expected_delimiters.first().map(String::as_str),
                    "case {case}: the default delimiter is the first published one, got {default_delimiter:?}"
                );
            } else {
                assert!(
                    delimiters.is_none() && default_delimiter.is_none(),
                    "case {case}: a destination that takes no delimited format publishes no delimiter, got {delimiters:?} and {default_delimiter:?}"
                );
            }
        }
    }
}

/// An S3 delivery's object layout: the suffix path variables a template publishes name the
/// account and Region the delivery is made in, the SENDING RESOURCE, and the time partition —
/// and the default suffix path is written in those variables alone.
#[tokio::test]
async fn an_s3_templates_suffix_path_variables_name_the_sending_resource() {
    let logs = Logs::new();

    // (service, log type, resource type, the variables naming the resource, the default path)
    let cases: [(&str, &str, &str, &[&str], &str); 5] = [
        (
            "apigateway",
            "EXECUTION_LOGS",
            "stages",
            &["api_id", "stage"],
            "{region}/{api_id}/{stage}/",
        ),
        (
            "eks",
            "AUTO_MODE_LOAD_BALANCING_LOGS",
            "cluster",
            &["cluster_id"],
            "{region}/{cluster_id}/{yyyy}/{MM}/{dd}/{HH}/",
        ),
        (
            "events",
            "ERROR_LOGS",
            "subscriber",
            &["subscriber_name"],
            "{region}/{subscriber_name}/{yyyy}/{MM}/{dd}/{HH}/",
        ),
        (
            "bedrock-agentcore",
            "APPLICATION_LOGS",
            "workload-identity",
            &["workload_identity_id"],
            "{region}/{workload_identity_id}/{yyyy}/{MM}/{dd}/{HH}/",
        ),
        (
            "workmail",
            "ACCESS_CONTROL_LOGS",
            "organization",
            &["organization_id"],
            "{region}/{yyyy}/{MM}/{dd}/{HH}/",
        ),
    ];

    for (service, log_type, resource_type, resource_fields, suffix_path) in cases {
        let case = format!("{service}|{log_type}|{resource_type}");
        let templates = slice(&logs, service, log_type, resource_type).await;
        let object_template = templates
            .iter()
            .find(|template| {
                template.get("deliveryDestinationType").and_then(Value::as_str) == Some("S3")
            })
            .unwrap_or_else(|| panic!("case {case}: the catalogue publishes an S3 template, got {templates:?}"));

        let mut expected: Vec<String> = vec!["accountid".to_string(), "region".to_string()];
        expected.extend(resource_fields.iter().map(|field| (*field).to_string()));
        expected.extend(
            ["yyyy", "MM", "dd", "HH"]
                .iter()
                .map(|field| (*field).to_string()),
        );
        let reported = strings(object_template, "allowedSuffixPathFields", &case);
        assert_eq!(
            reported, expected,
            "case {case}: expected the suffix path fields {expected:?}, got {reported:?}"
        );

        let reported_path = object_template
            .get("defaultDeliveryConfigValues")
            .and_then(|defaults| defaults.get("s3DeliveryConfiguration"))
            .and_then(|s3| s3.get("suffixPath"))
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: an S3 template carries a default suffix path, got {object_template}"));
        assert_eq!(
            reported_path, suffix_path,
            "case {case}: expected the default suffix path {suffix_path:?}, got {reported_path:?}"
        );
        for variable in reported_path
            .split('{')
            .skip(1)
            .filter_map(|rest| rest.split_once('}'))
            .map(|(name, _)| name.to_string())
        {
            assert!(
                reported.contains(&variable),
                "case {case}: the default suffix path uses {variable:?}, which is not among {reported:?}"
            );
        }
    }
}

/// An S3 Tables integration is published for the log types that have one, and for no others:
/// it is a property of the LOG TYPE's own schema, so it travels with the log type to every
/// destination type that log type can be delivered to.
#[tokio::test]
async fn the_s3_tables_datasource_is_published_for_the_log_types_that_have_one() {
    let logs = Logs::new();

    /// The S3 Tables datasource a log type publishes: its name and its type.
    type Datasource = Option<(&'static str, &'static str)>;
    // (service, log type, resource type, the datasource name and type, if any)
    let cases: [(&str, &str, &str, Datasource); 4] = [
        (
            "elasticloadbalancing",
            "ALB_ACCESS_LOGS",
            "loadbalancer",
            Some(("aws_alb", "access")),
        ),
        (
            "elasticloadbalancing",
            "ALB_CONNECTION_LOGS",
            "loadbalancer",
            Some(("aws_alb", "connection")),
        ),
        (
            "elasticloadbalancing",
            "ALB_HEALTH_CHECK_LOGS",
            "loadbalancer",
            None,
        ),
        ("workmail", "ACCESS_CONTROL_LOGS", "organization", None),
    ];

    for (service, log_type, resource_type, integration) in cases {
        let case = format!("{service}|{log_type}|{resource_type}");
        let templates = slice(&logs, service, log_type, resource_type).await;
        assert!(
            !templates.is_empty(),
            "case {case}: the catalogue publishes templates for this log source, got none"
        );
        for template in &templates {
            let reported = template.get("s3TablesIntegration");
            match integration {
                Some((name, kind)) => {
                    let reported = reported.unwrap_or_else(|| {
                        panic!("case {case}: this log type publishes an S3 Tables datasource, got {template}")
                    });
                    assert_eq!(
                        reported.get("datasourceName").and_then(Value::as_str),
                        Some(name),
                        "case {case}: expected the datasource name {name:?}, got {reported}"
                    );
                    assert_eq!(
                        reported.get("datasourceType").and_then(Value::as_str),
                        Some(kind),
                        "case {case}: expected the datasource type {kind:?}, got {reported}"
                    );
                }
                None => assert!(
                    reported.is_none(),
                    "case {case}: this log type publishes no S3 Tables datasource, got {reported:?}"
                ),
            }
        }
    }
}

/// An Application Load Balancer's access-log template publishes the access-log record itself:
/// every field mandatory, opening with the request's type, instant and load balancer, and
/// closing with the connection it was served on.
#[tokio::test]
async fn a_load_balancer_access_log_template_publishes_the_access_log_record() {
    let logs = Logs::new();
    let case = "elasticloadbalancing|ALB_ACCESS_LOGS|loadbalancer";
    let templates = slice(&logs, "elasticloadbalancing", "ALB_ACCESS_LOGS", "loadbalancer").await;
    let template = templates
        .first()
        .unwrap_or_else(|| panic!("case {case}: the catalogue publishes this template, got none"));
    let fields = allowed_fields(template, case);
    let names: Vec<String> = fields.iter().map(|(name, _)| name.clone()).collect();

    for opening in ["type", "time", "elb", "client_ip", "client_port"] {
        assert!(
            names.contains(&opening.to_string()),
            "case {case}: the access-log record carries {opening:?}, got {names:?}"
        );
    }
    assert_eq!(
        names.last().map(String::as_str),
        Some("conn_trace_id"),
        "case {case}: the access-log record closes with the connection it was served on, got {names:?}"
    );
    assert!(
        fields.iter().all(|(_name, mandatory)| *mandatory),
        "case {case}: every field of a fixed-shape access-log line is mandatory, got {fields:?}"
    );
}
