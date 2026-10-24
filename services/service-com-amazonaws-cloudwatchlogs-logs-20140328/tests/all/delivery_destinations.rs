// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `PutDeliveryDestination` and `GetDeliveryDestination`: the logical
//! delivery destinations an account holds.
//!
//! The rule under test: `PutDeliveryDestination` creates — or, under a name the account
//! already holds, REPLACES ("all the current delivery destination parameters are overwritten
//! with the new parameter values that you specify") — one delivery destination in state from
//! the parameters the request names, deriving its `arn` from the request's Region and its
//! `deliveryDestinationType` from the vendor of the `destinationResourceArn` it configures
//! (from the request only for the trace destination, the one kind that configures no
//! resource), and reports the stored record; `GetDeliveryDestination` then reports that same
//! record and refuses a name the account does not hold.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so what is
//! asserted here is what a client reads off the wire, and a response that is not computed
//! from the shared store cannot pass.
//!
//! The table spans what the rule generalises over rather than one request: a destination
//! resource of every vendor the model's `DeliveryDestinationType` enumerates (and resources
//! outside it), every value of the model's `OutputFormat`, a configuration present and
//! absent, a role present and absent, tag maps of zero, one and several entries, names at
//! both published length bounds and one past each, and a put that creates followed by puts
//! that replace.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::ops::types::{
    DELIVERY_DESTINATION_TYPE_VALUES, OUTPUT_FORMAT_VALUES,
};
use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::delivery_destination_arn;

use crate::harness::{Logs, REGION};

/// The two operations under test.
const PUT: &str = "PutDeliveryDestination";
const GET: &str = "GetDeliveryDestination";

/// The refusals both operations model for the cases below.
const VALIDATION: &str = "ValidationException";
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The `DeliveryDestinationType` of a trace destination, the one kind that needs no
/// `deliveryDestinationConfiguration` and the one kind `roleArn` is supported for. Source:
/// the model's `DeliveryDestinationType` enum (`DELIVERY_DESTINATION_TYPE_VALUES`), asserted
/// to be one of its values in [`every_published_destination_type_is_reachable`].
const TRACE: &str = "XRAY";

/// One destination resource of each vendor a delivery destination can represent — "a log
/// group in CloudWatch Logs, an Amazon S3 bucket, or a delivery stream in Firehose", plus the
/// X-Ray trace destination the model's enum adds — and resources outside that set.
const LOG_GROUP_ARN: &str = "arn:aws:logs:eu-west-2:123456789012:log-group:/aws/vendedlogs/one:*";
const LOG_GROUP_ARN_NO_WILDCARD: &str =
    "arn:aws:logs:eu-west-2:123456789012:log-group:/aws/vendedlogs/two";
const BUCKET_ARN: &str = "arn:aws:s3:::vended-logs-bucket";
const FIREHOSE_ARN: &str = "arn:aws:firehose:eu-west-2:123456789012:deliverystream/vended-logs";
const XRAY_ARN: &str = "arn:aws:xray:eu-west-2:123456789012:segment-destination/vended-traces";
const LOG_STREAM_ARN: &str =
    "arn:aws:logs:eu-west-2:123456789012:log-group:/aws/vendedlogs/one:log-stream:first";
const LOGICAL_DESTINATION_ARN: &str = "arn:aws:logs:eu-west-2:123456789012:destination:vended";
const KINESIS_ARN: &str = "arn:aws:kinesis:eu-west-2:123456789012:stream/vended-logs";

/// The IAM role a trace delivery destination may be created with.
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/CWLogsTraceDelivery";

/// One put, and what the service must then report for the destination it created.
#[derive(Debug)]
struct Case {
    /// What this row varies, for the assertion messages.
    label: &'static str,
    /// `name` (`DeliveryDestinationName`, 1..60, `^[\w-]*$`).
    name: String,
    /// `deliveryDestinationConfiguration.destinationResourceArn`, or `None` for a put that
    /// configures no resource.
    destination_resource_arn: Option<&'static str>,
    /// `deliveryDestinationType` as the REQUEST names it — which the service uses only when
    /// the put configures no resource.
    requested_type: Option<&'static str>,
    /// `outputFormat`, absent or one value of the model's `OutputFormat`.
    output_format: Option<&'static str>,
    /// `roleArn`, absent or the role above.
    role_arn: Option<&'static str>,
    /// `tags`, zero / one / several entries.
    tags: Vec<(&'static str, &'static str)>,
    /// The `deliveryDestinationType` the service must report.
    expected_type: &'static str,
}

impl Case {
    /// A put naming a destination resource, whose type is therefore the resource's.
    fn of(
        label: &'static str,
        name: &str,
        destination_resource_arn: &'static str,
        expected_type: &'static str,
    ) -> Self {
        Self {
            label,
            name: name.to_string(),
            destination_resource_arn: Some(destination_resource_arn),
            requested_type: None,
            output_format: None,
            role_arn: None,
            tags: Vec::new(),
            expected_type,
        }
    }

    fn with_output_format(mut self, output_format: &'static str) -> Self {
        self.output_format = Some(output_format);
        self
    }

    fn with_requested_type(mut self, requested_type: &'static str) -> Self {
        self.requested_type = Some(requested_type);
        self
    }

    fn with_tags(mut self, tags: &[(&'static str, &'static str)]) -> Self {
        self.tags = tags.to_vec();
        self
    }

    /// The request body this case puts.
    fn body(&self) -> Value {
        let mut body = json!({"name": self.name});
        if let Some(arn) = self.destination_resource_arn {
            body["deliveryDestinationConfiguration"] = json!({"destinationResourceArn": arn});
        }
        if let Some(requested_type) = self.requested_type {
            body["deliveryDestinationType"] = json!(requested_type);
        }
        if let Some(output_format) = self.output_format {
            body["outputFormat"] = json!(output_format);
        }
        if let Some(role_arn) = self.role_arn {
            body["roleArn"] = json!(role_arn);
        }
        if !self.tags.is_empty() {
            body["tags"] = tags_value(&self.tags);
        }
        body
    }

    /// The `DeliveryDestination` the service must report for this case, through the put and
    /// through every later read.
    fn expected(&self) -> Value {
        let mut expected = json!({
            "name": self.name,
            "arn": delivery_destination_arn(REGION, &self.name),
            "deliveryDestinationType": self.expected_type,
        });
        if let Some(arn) = self.destination_resource_arn {
            expected["deliveryDestinationConfiguration"] = json!({"destinationResourceArn": arn});
        }
        if let Some(output_format) = self.output_format {
            expected["outputFormat"] = json!(output_format);
        }
        // "This field is present only for X-Ray trace delivery destinations that were
        // created with a role."
        if self.expected_type == TRACE {
            if let Some(role_arn) = self.role_arn {
                expected["roleArn"] = json!(role_arn);
            }
        }
        if !self.tags.is_empty() {
            expected["tags"] = tags_value(&self.tags);
        }
        expected
    }
}

/// A trace delivery destination: the one kind created from a `deliveryDestinationType` alone,
/// with or without a role.
fn trace_case(label: &'static str, name: &str, role_arn: Option<&'static str>) -> Case {
    Case {
        label,
        name: name.to_string(),
        destination_resource_arn: None,
        requested_type: Some(TRACE),
        output_format: None,
        role_arn,
        tags: Vec::new(),
        expected_type: TRACE,
    }
}

/// A tag map as the wire carries one.
fn tags_value(tags: &[(&str, &str)]) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect(),
    )
}

/// The `deliveryDestination` a response carries.
fn reported(operation: &str, body: &Value, response: &Value) -> Value {
    response
        .get("deliveryDestination")
        .cloned()
        .unwrap_or_else(|| {
            panic!("{operation} {body}: the response must carry deliveryDestination, got {response}")
        })
}

/// The destination the service reports under `name`.
async fn get(logs: &Logs, name: &str) -> Value {
    let body = json!({"name": name});
    let response = logs.ok(GET, &body).await;
    reported(GET, &body, &response)
}

/// The corpus the reporting cases below read: one destination per vendor, per output format
/// shape, per tag-set size, and at both published name bounds.
fn cases() -> Vec<Case> {
    vec![
        Case::of("a CloudWatch Logs log group", "cwl-dest", LOG_GROUP_ARN, "CWL"),
        // The same vendor named by the other log-group ARN form (no trailing `:*`), with a
        // type the request also states — which must agree with the resource's own.
        Case::of(
            "a log group ARN without the trailing wildcard, type also stated",
            "cwl_dest_two",
            LOG_GROUP_ARN_NO_WILDCARD,
            "CWL",
        )
        .with_requested_type("CWL")
        .with_output_format("plain")
        .with_tags(&[("team", "platform")]),
        Case::of("an Amazon S3 bucket", "s3-dest", BUCKET_ARN, "S3")
            .with_output_format("parquet")
            .with_tags(&[("env", "prod"), ("owner", "obs"), ("tier", "gold")]),
        Case::of("a Firehose delivery stream", "fh-dest", FIREHOSE_ARN, "FH")
            .with_output_format("raw"),
        Case::of("an X-Ray trace destination resource", "xray-arn-dest", XRAY_ARN, TRACE),
        trace_case("a trace destination with a role", "xray-dest-role", Some(ROLE_ARN)),
        trace_case("a trace destination without a role", "xray-dest-no-role", None),
        // Both published bounds of `DeliveryDestinationName` (`@length` 1..60).
        Case::of("the shortest name the model admits", "a", BUCKET_ARN, "S3")
            .with_output_format("json"),
        Case::of(
            "the longest name the model admits",
            &"z".repeat(60),
            FIREHOSE_ARN,
            "FH",
        )
        .with_output_format("w3c"),
    ]
}

/// A put stores the destination it describes, reports it, and the read reports the SAME
/// record afterwards — for a destination of every vendor, every optional member present and
/// absent, and tag sets of every size.
#[tokio::test]
async fn a_put_destination_is_reported_and_read_back() {
    let logs = Logs::new();
    let cases = cases();

    for case in &cases {
        let body = case.body();
        let response = logs.ok(PUT, &body).await;
        let put_reported = reported(PUT, &body, &response);
        let expected = case.expected();
        assert_eq!(
            put_reported, expected,
            "case {:?}: expected the put to report {expected}, got {put_reported}",
            case.label
        );

        let read = get(&logs, &case.name).await;
        assert_eq!(
            read, expected,
            "case {:?}: expected the read-back to report {expected}, got {read}",
            case.label
        );

        // Reporting is a READ: asking twice reports the same record.
        let again = get(&logs, &case.name).await;
        assert_eq!(
            again, read,
            "case {:?}: a second read must report the same record, got {again} then {read}",
            case.label
        );
    }

    // Every destination put above is still reported as it was put: one put does not disturb
    // another name's record.
    for case in &cases {
        let read = get(&logs, &case.name).await;
        let expected = case.expected();
        assert_eq!(
            read, expected,
            "case {:?}: after every other put, expected {expected}, got {read}",
            case.label
        );
    }
}

/// Every `DeliveryDestinationType` the model publishes is reachable, and each one is the type
/// of the RESOURCE the put configures rather than the value the request states. Source: the
/// model's `DeliveryDestinationType` enum (`DELIVERY_DESTINATION_TYPE_VALUES`).
#[tokio::test]
async fn every_published_destination_type_is_reachable() {
    let logs = Logs::new();

    // (the destination resource, the type its vendor implies)
    let resources: [(&str, &str); 4] = [
        (LOG_GROUP_ARN, "CWL"),
        (BUCKET_ARN, "S3"),
        (FIREHOSE_ARN, "FH"),
        (XRAY_ARN, TRACE),
    ];
    let mut covered: BTreeMap<&str, bool> = DELIVERY_DESTINATION_TYPE_VALUES
        .iter()
        .map(|value| (*value, false))
        .collect();

    for (index, (resource, expected_type)) in resources.into_iter().enumerate() {
        assert!(
            covered.contains_key(expected_type),
            "case {resource:?}: {expected_type} must be a published DeliveryDestinationType, \
             the model publishes {DELIVERY_DESTINATION_TYPE_VALUES:?}"
        );
        covered.insert(expected_type, true);

        // Each put states a type that is NOT the resource's, so what is reported can only
        // come from the resource the destination represents.
        let stated = DELIVERY_DESTINATION_TYPE_VALUES
            .iter()
            .find(|value| **value != expected_type)
            .expect("the model publishes more than one destination type");
        let name = format!("derived-{index}");
        let body = json!({
            "name": name,
            "deliveryDestinationConfiguration": {"destinationResourceArn": resource},
            "deliveryDestinationType": stated,
        });
        let response = logs.ok(PUT, &body).await;
        let destination = reported(PUT, &body, &response);
        let actual = destination.get("deliveryDestinationType");
        assert_eq!(
            actual,
            Some(&json!(expected_type)),
            "case {resource:?} (request stated {stated}): expected deliveryDestinationType \
             {expected_type}, got {destination}"
        );

        let read = get(&logs, &name).await;
        assert_eq!(
            read, destination,
            "case {resource:?}: the read must report what the put reported, got {read}"
        );
    }

    let uncovered: Vec<&&str> = covered
        .iter()
        .filter(|(_, reached)| !**reached)
        .map(|(value, _)| value)
        .collect();
    assert!(
        uncovered.is_empty(),
        "case \"the published catalogue\": every DeliveryDestinationType must be reachable, \
         got {uncovered:?} unreached"
    );
}

/// Every `OutputFormat` the model publishes is accepted and reported as given; a format
/// outside the catalogue is refused by the modelled enum. Source: the model's `OutputFormat`
/// enum (`OUTPUT_FORMAT_VALUES`).
#[tokio::test]
async fn every_published_output_format_is_accepted_and_one_outside_is_refused() {
    let logs = Logs::new();

    for output_format in OUTPUT_FORMAT_VALUES {
        let name = format!("format-{output_format}");
        let body = json!({
            "name": name,
            "deliveryDestinationConfiguration": {"destinationResourceArn": BUCKET_ARN},
            "outputFormat": output_format,
        });
        logs.ok(PUT, &body).await;
        let read = get(&logs, &name).await;
        assert_eq!(
            read.get("outputFormat"),
            Some(&json!(output_format)),
            "case {output_format:?}: expected outputFormat {output_format}, got {read}"
        );
    }

    // One value outside the published catalogue.
    let body = json!({
        "name": "format-outside",
        "deliveryDestinationConfiguration": {"destinationResourceArn": BUCKET_ARN},
        "outputFormat": "orc",
    });
    let code = logs.err(PUT, &body).await;
    assert_eq!(
        code, VALIDATION,
        "case \"an outputFormat outside the model's enum\": expected {VALIDATION}, got {code}"
    );
    let code = logs.err(GET, &json!({"name": "format-outside"})).await;
    assert_eq!(
        code, NOT_FOUND,
        "case \"an outputFormat outside the model's enum\": the refused put must store \
         nothing, got {code}"
    );
}

/// A put under a name the account already holds REPLACES that destination's parameters: "all
/// the current delivery destination parameters are overwritten with the new parameter values
/// that you specify."
#[tokio::test]
async fn a_put_under_a_stored_name_replaces_its_parameters() {
    let logs = Logs::new();
    let name = "replaced-dest";

    // (what this put names, what the read must then report)
    let steps: Vec<(&str, Value, Value)> = vec![
        (
            "created as an S3 destination with a format and tags",
            json!({
                "name": name,
                "deliveryDestinationConfiguration": {"destinationResourceArn": BUCKET_ARN},
                "outputFormat": "json",
                "tags": {"env": "prod", "team": "obs"},
            }),
            json!({
                "name": name,
                "arn": delivery_destination_arn(REGION, name),
                "deliveryDestinationType": "S3",
                "deliveryDestinationConfiguration": {"destinationResourceArn": BUCKET_ARN},
                "outputFormat": "json",
                "tags": {"env": "prod", "team": "obs"},
            }),
        ),
        (
            "replaced by a Firehose destination that names no format",
            json!({
                "name": name,
                "deliveryDestinationConfiguration": {"destinationResourceArn": FIREHOSE_ARN},
            }),
            json!({
                "name": name,
                "arn": delivery_destination_arn(REGION, name),
                "deliveryDestinationType": "FH",
                "deliveryDestinationConfiguration": {"destinationResourceArn": FIREHOSE_ARN},
                // The tag set is the resource's, not one of the delivery parameters: a put
                // that names no tags leaves it as it was.
                "tags": {"env": "prod", "team": "obs"},
            }),
        ),
        (
            "replaced by a log-group destination with a new format and new tags",
            json!({
                "name": name,
                "deliveryDestinationConfiguration": {"destinationResourceArn": LOG_GROUP_ARN},
                "outputFormat": "w3c",
                "tags": {"env": "staging"},
            }),
            json!({
                "name": name,
                "arn": delivery_destination_arn(REGION, name),
                "deliveryDestinationType": "CWL",
                "deliveryDestinationConfiguration": {"destinationResourceArn": LOG_GROUP_ARN},
                "outputFormat": "w3c",
                "tags": {"env": "staging"},
            }),
        ),
        (
            "replaced by a trace destination, which configures no resource",
            json!({"name": name, "deliveryDestinationType": TRACE, "roleArn": ROLE_ARN}),
            json!({
                "name": name,
                "arn": delivery_destination_arn(REGION, name),
                "deliveryDestinationType": TRACE,
                "roleArn": ROLE_ARN,
                "tags": {"env": "staging"},
            }),
        ),
    ];

    for (label, body, expected) in steps {
        let response = logs.ok(PUT, &body).await;
        let put_reported = reported(PUT, &body, &response);
        assert_eq!(
            put_reported, expected,
            "case {label:?}: expected the put to report {expected}, got {put_reported}"
        );
        let read = get(&logs, name).await;
        assert_eq!(
            read, expected,
            "case {label:?}: expected the read-back to report {expected}, got {read}"
        );
    }
}

/// A put that configures no destination resource is accepted only for the trace destination:
/// "deliveryDestinationConfiguration is required for CloudWatch Logs, Amazon S3, Firehose log
/// delivery destinations and not required for X-Ray trace delivery destinations."
#[tokio::test]
async fn a_put_without_a_configuration_is_refused_unless_it_is_a_trace_destination() {
    let logs = Logs::new();

    for destination_type in DELIVERY_DESTINATION_TYPE_VALUES {
        let name = format!("unconfigured-{destination_type}");
        let body = json!({"name": name, "deliveryDestinationType": destination_type});
        if *destination_type == TRACE {
            let response = logs.ok(PUT, &body).await;
            let destination = reported(PUT, &body, &response);
            assert_eq!(
                destination.get("deliveryDestinationConfiguration"),
                None,
                "case {destination_type:?}: a trace destination configures no resource, \
                 got {destination}"
            );
            continue;
        }
        let code = logs.err(PUT, &body).await;
        assert_eq!(
            code, VALIDATION,
            "case {destination_type:?}: expected {VALIDATION}, got {code}"
        );
        let code = logs.err(GET, &json!({"name": name})).await;
        assert_eq!(
            code, NOT_FOUND,
            "case {destination_type:?}: the refused put must store nothing, got {code}"
        );
    }

    // A put naming neither a configuration nor a type says nothing about what it delivers
    // to, so it is refused the same way.
    let code = logs.err(PUT, &json!({"name": "unconfigured-untyped"})).await;
    assert_eq!(
        code, VALIDATION,
        "case \"neither a configuration nor a type\": expected {VALIDATION}, got {code}"
    );
}

/// A `destinationResourceArn` is accepted only when it names one of the resources the member
/// documents — "a log group in CloudWatch Logs, an Amazon S3 bucket, or a delivery stream in
/// Firehose" (plus the X-Ray trace destination the model's enum adds) — and the refusal names
/// the member.
#[tokio::test]
async fn a_destination_resource_outside_the_published_vendors_is_refused() {
    let logs = Logs::new();

    // (case, the destinationResourceArn it names)
    let rows: [(&str, &str); 6] = [
        ("a value that is not an ARN at all", "vended-logs-bucket"),
        ("the empty value", ""),
        ("a vendor no delivery destination can be", KINESIS_ARN),
        ("a CloudWatch Logs log STREAM", LOG_STREAM_ARN),
        (
            "a CloudWatch Logs logical destination, not a log group",
            LOGICAL_DESTINATION_ARN,
        ),
        (
            "a CloudWatch Logs query-result ARN, not a log group",
            "arn:aws:logs:eu-west-2:123456789012:query-result:*",
        ),
    ];

    for (index, (label, arn)) in rows.into_iter().enumerate() {
        let name = format!("rejected-{index}");
        let body = json!({
            "name": name,
            "deliveryDestinationConfiguration": {"destinationResourceArn": arn},
        });
        let (code, message) = logs.refusal(PUT, &body).await;
        assert_eq!(
            code, VALIDATION,
            "case {label:?}: expected {VALIDATION}, got {code} {message:?}"
        );
        assert!(
            message.contains("destinationResourceArn"),
            "case {label:?}: the refusal must name the member, got {message:?}"
        );
        let code = logs.err(GET, &json!({"name": name})).await;
        assert_eq!(
            code, NOT_FOUND,
            "case {label:?}: the refused put must store nothing, got {code}"
        );
    }
}

/// `roleArn` is "supported only for X-Ray trace delivery destinations", and must be an ARN.
#[tokio::test]
async fn a_role_is_accepted_only_for_a_trace_destination() {
    let logs = Logs::new();

    // The trace destination takes the role and reports it.
    let body = json!({"name": "role-ok", "deliveryDestinationType": TRACE, "roleArn": ROLE_ARN});
    let response = logs.ok(PUT, &body).await;
    let destination = reported(PUT, &body, &response);
    assert_eq!(
        destination.get("roleArn"),
        Some(&json!(ROLE_ARN)),
        "case \"a trace destination with a role\": expected roleArn {ROLE_ARN}, got {destination}"
    );

    // (case, the body it puts)
    let rows: Vec<(&str, Value)> = vec![
        (
            "a role on a log-group destination",
            json!({
                "name": "role-cwl",
                "deliveryDestinationConfiguration": {"destinationResourceArn": LOG_GROUP_ARN},
                "roleArn": ROLE_ARN,
            }),
        ),
        (
            "a role on an S3 destination",
            json!({
                "name": "role-s3",
                "deliveryDestinationConfiguration": {"destinationResourceArn": BUCKET_ARN},
                "roleArn": ROLE_ARN,
            }),
        ),
        (
            "a role on a Firehose destination",
            json!({
                "name": "role-fh",
                "deliveryDestinationConfiguration": {"destinationResourceArn": FIREHOSE_ARN},
                "roleArn": ROLE_ARN,
            }),
        ),
        (
            "a role that is not an ARN, on a trace destination",
            json!({
                "name": "role-unparsable",
                "deliveryDestinationType": TRACE,
                "roleArn": "CWLogsTraceDelivery",
            }),
        ),
    ];

    for (label, body) in rows {
        let (code, message) = logs.refusal(PUT, &body).await;
        assert_eq!(
            code, VALIDATION,
            "case {label:?}: expected {VALIDATION}, got {code} {message:?}"
        );
        assert!(
            message.contains("roleArn"),
            "case {label:?}: the refusal must name the member, got {message:?}"
        );
        let name = body
            .get("name")
            .and_then(Value::as_str)
            .expect("every row names a destination");
        let code = logs.err(GET, &json!({"name": name})).await;
        assert_eq!(
            code, NOT_FOUND,
            "case {label:?}: the refused put must store nothing, got {code}"
        );
    }
}

/// Both published bounds of `DeliveryDestinationName` reach the operation, and a name past
/// either bound — or outside the published pattern — is refused by the modelled constraint.
/// Source: the model's `DeliveryDestinationName` shape (`@length` 1..60, `@pattern
/// ^[\w-]*$`), which both operations' `name` targets.
#[tokio::test]
async fn the_published_name_bounds_are_accepted_and_one_past_each_is_refused() {
    let logs = Logs::new();

    for name in ["n", &"n".repeat(60)] {
        let body = json!({
            "name": name,
            "deliveryDestinationConfiguration": {"destinationResourceArn": BUCKET_ARN},
        });
        let response = logs.ok(PUT, &body).await;
        let destination = reported(PUT, &body, &response);
        assert_eq!(
            destination.get("name"),
            Some(&json!(name)),
            "case \"a name of {} characters\": expected it reported back, got {destination}",
            name.chars().count()
        );
    }

    // (case, the name it uses)
    let too_long = "n".repeat(61);
    let rows: [(&str, &str); 4] = [
        ("one character past the maximum", &too_long),
        ("the empty name, one past the minimum", ""),
        ("a name outside the published pattern", "vended logs"),
        ("another name outside the published pattern", "vended/logs"),
    ];
    for (label, name) in rows {
        for operation in [PUT, GET] {
            let mut body = json!({"name": name});
            if operation == PUT {
                body["deliveryDestinationConfiguration"] =
                    json!({"destinationResourceArn": BUCKET_ARN});
            }
            let code = logs.err(operation, &body).await;
            assert_eq!(
                code, VALIDATION,
                "case {label:?} on {operation}: expected {VALIDATION}, got {code}"
            );
        }
    }
}

/// Reading a delivery destination the account does not hold is refused with the error the
/// operation models, whatever else the account holds.
#[tokio::test]
async fn reading_a_destination_the_account_does_not_hold_is_refused() {
    let logs = Logs::new();

    let code = logs.err(GET, &json!({"name": "absent"})).await;
    assert_eq!(
        code, NOT_FOUND,
        "case \"an empty account\": expected {NOT_FOUND}, got {code}"
    );

    logs.ok(
        PUT,
        &json!({
            "name": "present",
            "deliveryDestinationConfiguration": {"destinationResourceArn": BUCKET_ARN},
        }),
    )
    .await;

    // (case, the name it reads)
    let rows: [(&str, &str); 3] = [
        ("a name the account never held", "absent"),
        ("a prefix of a stored name", "pres"),
        ("a stored name in another case", "PRESENT"),
    ];
    for (label, name) in rows {
        let code = logs.err(GET, &json!({"name": name})).await;
        assert_eq!(
            code, NOT_FOUND,
            "case {label:?}: expected {NOT_FOUND}, got {code}"
        );
    }

    // The destination the account does hold is still reported.
    let read = get(&logs, "present").await;
    assert_eq!(
        read.get("name"),
        Some(&json!("present")),
        "case \"the stored destination\": expected it still reported, got {read}"
    );
}
