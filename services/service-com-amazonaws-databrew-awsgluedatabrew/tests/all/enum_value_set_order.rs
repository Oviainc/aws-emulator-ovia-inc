// SPDX-License-Identifier: CC0-1.0

//! The ORDER a refusal lists an enumerated type's admitted values in.
//!
//! The rule pinned here: a value outside a modelled enum is refused in the validation
//! envelope whose requirement clause states the admitted set in the order THIS SERVICE
//! states that set in — the order the set's own hash table is walked, each value's
//! bucket ascending — which is a property of the enumerated TYPE rather than of the
//! member, the operation or the request that carried the value. So every member
//! targeting one enum reports the same bracketed list, in the same order, whatever
//! request carried it, and whatever order the model happens to declare its values in.
//! The envelope, the echoed value, the lowerCamel member path and the requirement's
//! wording are the framework's; only the order inside the brackets is this service's.
//!
//! The rows differ along what the rule generalises over: which enumerated type the
//! value belongs to (seven of this model's), where the member carrying it sits (a
//! top-level member, a member nested in a structure, a member nested in a structure
//! inside a list), the value refused (a name no value carries, an admitted value
//! miscased, an admitted value with a typo, the empty string), and how the stated order
//! relates to the declared one (reversed, rotated, unchanged, and reordered into
//! neither the declared nor the sorted order).
//!
//! Each value set below names where its declaration order comes from — the
//! `smithy_query` that reads the type off the model — so the test pins the order rather
//! than restating whatever the service produced.

use serde_json::{json, Value};

use crate::databrew_harness::Emulator;

/// The `RoleArn` the job-creating rows carry (modelled `len 20..2048`); the role itself
/// is not what these cases are about.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/brew";

/// `com.amazonaws.databrew#InputFormat`, declared `CSV, JSON, PARQUET, EXCEL, ORC`
/// (`smithy_query(query="shape",shape="InputFormat")`).
const INPUT_FORMAT_SET: &str = "[ORC, CSV, JSON, PARQUET, EXCEL]";

/// `com.amazonaws.databrew#OutputFormat`, declared `CSV, JSON, PARQUET, GLUEPARQUET,
/// AVRO, ORC, XML, TABLEAUHYPER`.
const OUTPUT_FORMAT_SET: &str = "[ORC, CSV, JSON, TABLEAUHYPER, AVRO, PARQUET, XML, GLUEPARQUET]";

/// `com.amazonaws.databrew#SampleType`, declared `FIRST_N, LAST_N, RANDOM` — stated in
/// the reverse of that order.
const SAMPLE_TYPE_SET: &str = "[RANDOM, LAST_N, FIRST_N]";

/// `com.amazonaws.databrew#SampleMode`, declared `FULL_DATASET, CUSTOM_ROWS`.
const SAMPLE_MODE_SET: &str = "[CUSTOM_ROWS, FULL_DATASET]";

/// `com.amazonaws.databrew#EncryptionMode`, declared `SSE-KMS, SSE-S3`.
const ENCRYPTION_MODE_SET: &str = "[SSE-S3, SSE-KMS]";

/// `com.amazonaws.databrew#LogSubscription`, declared `ENABLE, DISABLE` — the one type
/// here the service states in its declared order.
const LOG_SUBSCRIPTION_SET: &str = "[ENABLE, DISABLE]";

/// `com.amazonaws.databrew#ThresholdType`, declared `GREATER_THAN_OR_EQUAL,
/// LESS_THAN_OR_EQUAL, GREATER_THAN, LESS_THAN` — stated in neither that order nor the
/// sorted one.
const THRESHOLD_TYPE_SET: &str = "[GREATER_THAN, LESS_THAN, LESS_THAN_OR_EQUAL, GREATER_THAN_OR_EQUAL]";

/// One refused request: the operation it names, the body it sends, and the three things
/// its clause must state.
struct Case {
    label: &'static str,
    method: &'static str,
    uri: &'static str,
    body: Value,
    /// The lowerCamel member path the clause names.
    member_path: &'static str,
    /// The value the request sent, which the clause echoes.
    value: &'static str,
    /// The bracketed admitted set the clause must list.
    value_set: &'static str,
}

/// The validation envelope over one enum-constraint clause: the value as submitted, the
/// member path, and the admitted set.
fn enum_clause(value: &str, member_path: &str, value_set: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member_path}' failed to satisfy \
         constraint: Member must satisfy enum value set: {value_set}"
    )
}

/// Every row of the table. Each body is otherwise valid — every other required member is
/// present and within its bounds — so the enum value is the ONE thing refused.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "the Format of CreateDataset",
            method: "POST",
            uri: "/datasets",
            body: json!({
                "Name": "orders",
                "Input": {"S3InputDefinition": {"Bucket": "brew-bucket", "Key": "orders.csv"}},
                "Format": "PARQUETX",
            }),
            member_path: "format",
            value: "PARQUETX",
            value_set: INPUT_FORMAT_SET,
        },
        Case {
            label: "an admitted input format miscased",
            method: "POST",
            uri: "/datasets",
            body: json!({
                "Name": "orders",
                "Input": {"S3InputDefinition": {"Bucket": "brew-bucket", "Key": "orders.csv"}},
                "Format": "csv",
            }),
            member_path: "format",
            value: "csv",
            value_set: INPUT_FORMAT_SET,
        },
        Case {
            label: "the empty string where an input format is admitted",
            method: "POST",
            uri: "/datasets",
            body: json!({
                "Name": "orders",
                "Input": {"S3InputDefinition": {"Bucket": "brew-bucket", "Key": "orders.csv"}},
                "Format": "",
            }),
            member_path: "format",
            value: "",
            value_set: INPUT_FORMAT_SET,
        },
        Case {
            label: "the sample type NESTED in CreateProject's Sample",
            method: "POST",
            uri: "/projects",
            body: json!({
                "Name": "cleanup",
                "DatasetName": "orders",
                "RecipeName": "tidy",
                "RoleArn": ROLE_ARN,
                "Sample": {"Type": "EVERY_N", "Size": 500},
            }),
            member_path: "sample.type",
            value: "EVERY_N",
            value_set: SAMPLE_TYPE_SET,
        },
        Case {
            label: "the sample mode NESTED in CreateProfileJob's JobSample",
            method: "POST",
            uri: "/profileJobs",
            body: json!({
                "Name": "profile",
                "DatasetName": "orders",
                "OutputLocation": {"Bucket": "brew-bucket"},
                "RoleArn": ROLE_ARN,
                "JobSample": {"Mode": "FULL", "Size": 20},
            }),
            member_path: "jobSample.mode",
            value: "FULL",
            value_set: SAMPLE_MODE_SET,
        },
        Case {
            label: "the encryption mode of CreateProfileJob",
            method: "POST",
            uri: "/profileJobs",
            body: json!({
                "Name": "profile",
                "DatasetName": "orders",
                "OutputLocation": {"Bucket": "brew-bucket"},
                "RoleArn": ROLE_ARN,
                "EncryptionMode": "SSE-AES",
            }),
            member_path: "encryptionMode",
            value: "SSE-AES",
            value_set: ENCRYPTION_MODE_SET,
        },
        Case {
            label: "the log subscription of CreateRecipeJob",
            method: "POST",
            uri: "/recipeJobs",
            body: json!({
                "Name": "nightly",
                "RoleArn": ROLE_ARN,
                "LogSubscription": "ON",
            }),
            member_path: "logSubscription",
            value: "ON",
            value_set: LOG_SUBSCRIPTION_SET,
        },
        Case {
            label: "the output format nested in a CreateRecipeJob output ELEMENT",
            method: "POST",
            uri: "/recipeJobs",
            body: json!({
                "Name": "nightly",
                "RoleArn": ROLE_ARN,
                "Outputs": [{
                    "Location": {"Bucket": "brew-bucket", "Key": "out/"},
                    "Format": "AVROX",
                }],
            }),
            member_path: "outputs.1.member.format",
            value: "AVROX",
            value_set: OUTPUT_FORMAT_SET,
        },
        Case {
            label: "the threshold type nested two levels into a CreateRuleset rule",
            method: "POST",
            uri: "/rulesets",
            body: json!({
                "Name": "quality",
                "TargetArn": "arn:aws:databrew:us-east-1:000000000000:dataset/orders",
                "Rules": [{
                    "Name": "no nulls",
                    "CheckExpression": "(:col1 != :val1)",
                    "Threshold": {"Value": 1.0, "Type": "GREATER"},
                }],
            }),
            member_path: "rules.1.member.threshold.type",
            value: "GREATER",
            value_set: THRESHOLD_TYPE_SET,
        },
    ]
}

#[tokio::test]
async fn a_refused_enum_value_lists_the_admitted_set_in_the_order_the_service_states_it() {
    let emulator = Emulator::new();

    for case in cases() {
        let refused = emulator
            .call(case.method, case.uri, Some(case.body.clone()))
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {:?}: expected 400 ValidationException, got {} {:?}",
            case.label,
            refused.status,
            refused.body
        );
        let expected = enum_clause(case.value, case.member_path, case.value_set);
        assert_eq!(
            refused.error_message(),
            expected,
            "case {:?}: expected {expected:?}, got {:?}",
            case.label,
            refused.error_message()
        );
    }
}

#[tokio::test]
async fn the_stated_order_is_the_enums_own_whatever_request_carries_it() {
    let emulator = Emulator::new();

    // ONE enumerated type, refused through every spelling of a value outside it a caller
    // can send, states ONE set in ONE order: the order belongs to the type, so nothing
    // about the request moves it. The set each answer states is read off the SERVICE's
    // own message here and then held to the order the type is stated in, rather than
    // compared against a literal.
    let spellings = [
        ("a name no value carries", "PARQUETX"),
        ("an admitted value miscased", "Excel"),
        ("an admitted value padded", " ORC"),
        ("the empty string", ""),
        ("a value carrying the set's own text", "ORC, CSV, JSON, PARQUET, EXCEL"),
    ];
    for (label, format) in spellings {
        let refused = emulator
            .call(
                "POST",
                "/datasets",
                Some(json!({
                    "Name": "orders",
                    "Input": {"S3InputDefinition": {"Bucket": "brew-bucket", "Key": "o.csv"}},
                    "Format": format,
                })),
            )
            .await;
        let stated = stated_set(&refused.error_message()).unwrap_or_else(|| {
            panic!(
                "case {label:?}: the message states no enum value set: {:?}",
                refused.error_message()
            )
        });
        assert_eq!(
            stated, INPUT_FORMAT_SET,
            "case {label:?}: the clause must state {INPUT_FORMAT_SET} however the value was \
             spelled, got {stated:?}"
        );
        // And the order the service states is neither the model's declaration order nor
        // the sorted one, so a client cannot read either off the message.
        let listed = values_of(&stated);
        let mut sorted = listed.clone();
        sorted.sort_unstable();
        assert_ne!(
            listed, sorted,
            "case {label:?}: this type is not stated sorted, got {listed:?}"
        );
        let declared = ["CSV", "JSON", "PARQUET", "EXCEL", "ORC"];
        assert_ne!(
            listed.iter().map(String::as_str).collect::<Vec<_>>(),
            declared,
            "case {label:?}: this type is not stated in the model's declaration order, got \
             {listed:?}"
        );
        assert_eq!(
            listed.len(),
            5,
            "case {label:?}: every value the type admits must be listed once, got {listed:?}"
        );
    }
}

/// The bracketed value set a clause states, or `None` for a message stating none.
fn stated_set(message: &str) -> Option<String> {
    let (_, set) = message.split_once("enum value set: ")?;
    Some(set.trim().to_string())
}

/// The values a bracketed set lists, in the order it lists them.
fn values_of(value_set: &str) -> Vec<String> {
    value_set
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(", ")
        .map(str::to_string)
        .collect()
}
