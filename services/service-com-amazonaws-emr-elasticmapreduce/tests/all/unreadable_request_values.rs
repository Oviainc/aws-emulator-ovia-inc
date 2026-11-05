// SPDX-License-Identifier: CC0-1.0

//! WHICH error class a request value this service could not READ is refused with, end to
//! end over the wire.
//!
//! THE RULE: there are two refusals before a handler runs, and they are not the same error
//! class.
//!
//! 1. A value whose JSON type the modelled type of the member it is bound to cannot be
//!    read from — a number where the model declares a string, a string where it declares
//!    an integer, an object where it declares a list — never becomes a value at all. That
//!    is the PROTOCOL's own failure, which AWS answers with HTTP 400
//!    `SerializationException` whose `Message` states the CLASS of value the request sent
//!    and the modelled type it could not become: `<class> can not be converted to a/an
//!    <type>`. The refusal names the CLASS, never the value, so a long value cannot be
//!    amplified into a long response — and the boolean LITERALS are two classes rather
//!    than one.
//! 2. A value that WAS read and then broke a rule the model declares — an absent
//!    `@required` member, a value outside an enum, a `@length` or `@range` violation — is
//!    a constraint refusal, which keeps this service's own request-validation envelope
//!    (`ValidationException`, or EMR's `InvalidRequestException` where the operation
//!    declares it).
//!
//! That separation is what lets a client tell a bug in its own request ENCODER from a
//! semantic rejection of a well-formed request.
//!
//! The rows span what the rule generalises over: which JSON class arrived, which modelled
//! type it was bound to, which operation carries the member, and how deep in the request
//! the value sits (a member of the input shape, a member of a nested structure, an element
//! of a list) — plus, for each class, the neighbouring request that READS and is refused
//! by the other layer instead. Every case drives the real HTTP path through the router's
//! `dispatch`.

use serde_json::{json, Value};

use crate::job_flow_lifecycle::{at, Emr, INSTANCE_PROFILE, SERVICE_ROLE};

/// The error code the protocol answers a body it could not read with.
const SERIALIZATION_EXCEPTION: &str = "SerializationException";

/// The error code the request validator answers a value it READ and then refused with.
const REQUEST_VALIDATION: &str = "ValidationException";

/// EMR's own client-input error, which the operations that declare it answer a refused
/// request with.
const SERVICE_ERROR: &str = "InvalidRequestException";

/// The awsJson1_1 error code, without the namespace the envelope may prefix.
fn error_code(payload: &Value) -> String {
    at(payload, &["__type"])
        .as_str()
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The error message, whichever casing the envelope carries it under.
fn error_message(payload: &Value) -> String {
    at(payload, &["Message"])
        .as_str()
        .or_else(|| at(payload, &["message"]).as_str())
        .unwrap_or_default()
        .to_string()
}

/// One row: a request carrying a value of the wrong JSON class, and the sentence the
/// protocol refuses it with.
struct Unreadable {
    case: &'static str,
    operation: &'static str,
    body: Value,
    message: &'static str,
}

fn unreadable_rows() -> Vec<Unreadable> {
    vec![
        Unreadable {
            // The recorded mistake: a JSON number where a string member is modelled.
            // `RunJobFlow$Name` targets `XmlStringMaxLen256`.
            case: "a whole number where RunJobFlow models a string",
            operation: "RunJobFlow",
            body: json!({
                "Name": 17,
                "JobFlowRole": INSTANCE_PROFILE, "ServiceRole": SERVICE_ROLE,
                "Instances": { "InstanceCount": 1, "MasterInstanceType": "m5.xlarge" },
            }),
            message: "NUMBER_VALUE can not be converted to a String",
        },
        Unreadable {
            case: "a fractional number where RunJobFlow models a string",
            operation: "RunJobFlow",
            body: json!({
                "Name": 17.5,
                "JobFlowRole": INSTANCE_PROFILE, "ServiceRole": SERVICE_ROLE,
                "Instances": { "InstanceCount": 1, "MasterInstanceType": "m5.xlarge" },
            }),
            message: "NUMBER_VALUE can not be converted to a String",
        },
        Unreadable {
            // The boolean LITERALS are two classes rather than one: the refusal names the
            // literal the request sent.
            case: "the true literal where RunJobFlow models a string",
            operation: "RunJobFlow",
            body: json!({
                "Name": true,
                "JobFlowRole": INSTANCE_PROFILE, "ServiceRole": SERVICE_ROLE,
                "Instances": { "InstanceCount": 1, "MasterInstanceType": "m5.xlarge" },
            }),
            message: "TRUE_VALUE can not be converted to a String",
        },
        Unreadable {
            case: "the false literal where RunJobFlow models a string",
            operation: "RunJobFlow",
            body: json!({
                "Name": false,
                "JobFlowRole": INSTANCE_PROFILE, "ServiceRole": SERVICE_ROLE,
                "Instances": { "InstanceCount": 1, "MasterInstanceType": "m5.xlarge" },
            }),
            message: "FALSE_VALUE can not be converted to a String",
        },
        Unreadable {
            case: "an object where RunJobFlow models a string",
            operation: "RunJobFlow",
            body: json!({
                "Name": { "Value": "demo" },
                "JobFlowRole": INSTANCE_PROFILE, "ServiceRole": SERVICE_ROLE,
                "Instances": { "InstanceCount": 1, "MasterInstanceType": "m5.xlarge" },
            }),
            message: "OBJECT_VALUE can not be converted to a String",
        },
        Unreadable {
            // A NESTED structure's string member: `JobFlowInstancesConfig$Ec2KeyName`.
            case: "a number where a nested structure models a string",
            operation: "RunJobFlow",
            body: json!({
                "Name": "nested",
                "JobFlowRole": INSTANCE_PROFILE, "ServiceRole": SERVICE_ROLE,
                "Instances": {
                    "InstanceCount": 1,
                    "MasterInstanceType": "m5.xlarge",
                    "Ec2KeyName": 42,
                },
            }),
            message: "NUMBER_VALUE can not be converted to a String",
        },
        Unreadable {
            // An ELEMENT of a list of strings: `TerminateJobFlows$JobFlowIds`. The
            // target the sentence names is the ELEMENT's own type, which is the type the
            // failing read is for — the JSON codec hands a collection's elements the
            // collection member's own schema, so the schema alone would name the
            // collection instead.
            case: "a number as an element of a list of strings",
            operation: "TerminateJobFlows",
            body: json!({ "JobFlowIds": [7] }),
            message: "NUMBER_VALUE can not be converted to a String",
        },
        Unreadable {
            // A VALUE of a map of string to string: `Configuration$Properties`, the
            // recorded mistake at that depth.
            case: "a number as a value of a map of strings",
            operation: "RunJobFlow",
            body: json!({
                "Name": "numeric property",
                "JobFlowRole": INSTANCE_PROFILE, "ServiceRole": SERVICE_ROLE,
                "Instances": { "InstanceCount": 1, "MasterInstanceType": "m5.xlarge" },
                "Configurations": [{
                    "Classification": "core-site",
                    "Properties": { "fs.s3.maxConnections": 200 },
                }],
            }),
            message: "NUMBER_VALUE can not be converted to a String",
        },
        Unreadable {
            // The other direction: a string where an integer is modelled.
            // `JobFlowInstancesConfig$InstanceCount` targets `Integer`.
            case: "a string where a nested structure models an integer",
            operation: "RunJobFlow",
            body: json!({
                "Name": "stringly typed count",
                "JobFlowRole": INSTANCE_PROFILE, "ServiceRole": SERVICE_ROLE,
                "Instances": { "InstanceCount": "1", "MasterInstanceType": "m5.xlarge" },
            }),
            message: "STRING_VALUE can not be converted to an Integer",
        },
        Unreadable {
            // A whole number the modelled integer's width does not hold is no more
            // readable than a string of digits.
            case: "a number past the modelled integer width",
            operation: "RunJobFlow",
            body: json!({
                "Name": "too wide",
                "JobFlowRole": INSTANCE_PROFILE, "ServiceRole": SERVICE_ROLE,
                "Instances": { "InstanceCount": 2_147_483_648i64, "MasterInstanceType": "m5.xlarge" },
            }),
            message: "NUMBER_VALUE can not be converted to an Integer",
        },
        Unreadable {
            case: "a string where a boolean is modelled",
            operation: "SetTerminationProtection",
            body: json!({ "JobFlowIds": ["j-ABCDEFGHIJKLM"], "TerminationProtected": "yes" }),
            message: "STRING_VALUE can not be converted to a Boolean",
        },
        Unreadable {
            // `ListClusters$ClusterStates` targets a list.
            case: "a string where a list is modelled",
            operation: "ListClusters",
            body: json!({ "ClusterStates": "RUNNING" }),
            message: "STRING_VALUE can not be converted to a List",
        },
        Unreadable {
            case: "an object where a list is modelled",
            operation: "ListClusters",
            body: json!({ "ClusterStates": { "0": "RUNNING" } }),
            message: "OBJECT_VALUE can not be converted to a List",
        },
        Unreadable {
            // `ListClusters$CreatedAfter` targets the timestamp shape `Date`.
            case: "a boolean where a timestamp is modelled",
            operation: "ListClusters",
            body: json!({ "CreatedAfter": true }),
            message: "TRUE_VALUE can not be converted to a Timestamp",
        },
        Unreadable {
            // `Configuration$Properties` targets a map of string to string.
            case: "a string where a map is modelled",
            operation: "RunJobFlow",
            body: json!({
                "Name": "stringly typed properties",
                "JobFlowRole": INSTANCE_PROFILE, "ServiceRole": SERVICE_ROLE,
                "Instances": { "InstanceCount": 1, "MasterInstanceType": "m5.xlarge" },
                "Configurations": [{ "Classification": "core-site", "Properties": "a=b" }],
            }),
            message: "STRING_VALUE can not be converted to a Map",
        },
    ]
}

#[tokio::test]
async fn a_value_the_codec_cannot_read_is_refused_as_a_serialization_failure() {
    let emr = Emr::new();
    for case in unreadable_rows() {
        let label = case.case;
        let (status, payload) = emr.call(case.operation, case.body).await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (400, SERIALIZATION_EXCEPTION),
            "case {label:?}: expected 400 {SERIALIZATION_EXCEPTION}, got {status} with \
             {payload}"
        );
        assert_eq!(
            error_message(&payload),
            case.message,
            "case {label:?}: expected the coercion sentence, got {payload}"
        );
    }

    // A refused body wrote nothing: no cluster was launched by any row above.
    let listed = emr.ok("ListClusters", json!({})).await;
    assert_eq!(
        at(&listed, &["Clusters"]).as_array().map(Vec::len),
        Some(0),
        "a body that could not be read must write nothing, got {listed}"
    );
}

/// One row: a request that READS and is then refused by the other layer, so the two
/// classes are told apart on the wire rather than collapsed into one.
struct Readable {
    case: &'static str,
    operation: &'static str,
    body: Value,
    /// The error code the constraint layer answers, which is per-operation: EMR's own
    /// client error where the operation declares it, the generic one otherwise.
    code: &'static str,
}

#[tokio::test]
async fn a_value_that_reads_and_then_breaks_a_rule_keeps_the_constraint_envelope() {
    let emr = Emr::new();
    let rows = vec![
        Readable {
            // An absent `@required` member is found while READING, and is a constraint
            // refusal rather than a codec one.
            case: "RunJobFlow with no Name at all",
            operation: "RunJobFlow",
            body: json!({
                "JobFlowRole": INSTANCE_PROFILE, "ServiceRole": SERVICE_ROLE,
                "Instances": { "InstanceCount": 1, "MasterInstanceType": "m5.xlarge" },
            }),
            code: REQUEST_VALIDATION,
        },
        Readable {
            // An explicit null is absence, not a type error.
            case: "RunJobFlow with a null Name",
            operation: "RunJobFlow",
            body: json!({
                "Name": null,
                "JobFlowRole": INSTANCE_PROFILE, "ServiceRole": SERVICE_ROLE,
                "Instances": { "InstanceCount": 1, "MasterInstanceType": "m5.xlarge" },
            }),
            code: REQUEST_VALIDATION,
        },
        Readable {
            // A string that READS and then falls outside a modelled enum.
            case: "ListClusters with an undeclared ClusterStates value",
            operation: "ListClusters",
            body: json!({ "ClusterStates": ["NOPE"] }),
            code: SERVICE_ERROR,
        },
        Readable {
            // A string that READS and then names no cluster.
            case: "DescribeCluster with a cluster id of the right class",
            operation: "DescribeCluster",
            body: json!({ "ClusterId": "j-ZZZZZZZZZZZZZ" }),
            code: SERVICE_ERROR,
        },
    ];

    for case in rows {
        let label = case.case;
        let (status, payload) = emr.call(case.operation, case.body).await;
        assert_eq!(
            (status, error_code(&payload).as_str()),
            (400, case.code),
            "case {label:?}: expected 400 {}, got {status} with {payload}",
            case.code
        );
        assert_ne!(
            error_code(&payload),
            SERIALIZATION_EXCEPTION,
            "case {label:?}: a value that READ must not be refused as a serialization \
             failure, got {payload}"
        );
    }
}
