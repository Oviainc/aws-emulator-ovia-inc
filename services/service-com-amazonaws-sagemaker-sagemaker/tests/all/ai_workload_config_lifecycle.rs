// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for SageMaker's AI workload configuration slice:
//! `CreateAIWorkloadConfig`, the `DescribeAIWorkloadConfig` that replays it and the
//! `DeleteAIWorkloadConfig` that releases it.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, so
//! what these tests exercise is what a real SDK client gets.
//!
//! The tests assert RULES over a TABLE of inputs rather than one recorded call: the ARN
//! form for every region and name shape; the create/describe round trip for every
//! combination of the optional members (dataset channels, inline workload spec, tags);
//! the refusal of a name already in use; the refusal of a read of a name that was never
//! created; and, for the delete, that it answers the stored ARN, makes the name unreadable
//! and reusable, touches no other configuration, and is refused as not found whenever the
//! name names nothing — including a second delete of a name it just released.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// One emulator: the shared engine plus the SageMaker service mounted on the router.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These tests exercise handler behaviour, not SigV4; the request below carries a
        // credential scope only so the ARN region is the one the caller signed for.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `target` with a SigV4 credential scope naming `region`.
    async fn call_in(&self, region: &str, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SageMaker.{target}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/sagemaker/\
                     aws4_request, SignedHeaders=host;x-amz-target, Signature=0000"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        self.dispatch(request).await
    }

    /// Call `target` unsigned: no credential scope, so the ARN region falls back to the
    /// emulator default.
    async fn call_unsigned(&self, target: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SageMaker.{target}"))
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        self.dispatch(request).await
    }

    async fn dispatch(&self, request: http::Request<bytes::Bytes>) -> (u16, Value) {
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(body).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    async fn call(&self, target: &str, body: &Value) -> (u16, Value) {
        self.call_in("us-east-1", target, body).await
    }
}

/// The `__type` an error body carries, with any `namespace#` prefix stripped.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn str_field(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a string member {member} in {body}"))
        .to_string()
}

/// One input data channel naming `channel` and `uri`.
fn channel(channel: &str, uri: &str) -> Value {
    json!({ "ChannelName": channel, "DataSource": { "S3DataSource": { "S3Uri": uri } } })
}

/// `Tags` with `count` distinct key/value pairs.
fn tags(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({ "Key": format!("k{index}"), "Value": format!("v{index}") }))
            .collect(),
    )
}

/// One row of the round-trip table: a case name and the optional members a
/// `CreateAIWorkloadConfig` request carries beyond the required name.
struct Row {
    case: &'static str,
    members: Value,
}

/// The round-trip table: rows differ along the axes the round-trip rule generalises
/// over — which optional members are present, how many channels the dataset carries,
/// the form of the inline workload specification (JSON and YAML, both documented forms
/// of `WorkloadSpec$Inline`), and how many tags there are (0, 1 and the modelled
/// `TagList` maximum of 50).
fn round_trip_rows() -> Vec<Row> {
    vec![
        Row {
            case: "name only",
            members: json!({}),
        },
        Row {
            case: "one channel",
            members: json!({
                "DatasetConfig": {
                    "InputDataConfig": [channel("train", "s3://bucket/train/")],
                },
            }),
        },
        Row {
            case: "three channels, https data source",
            members: json!({
                "DatasetConfig": {
                    "InputDataConfig": [
                        channel("train", "s3://bucket/train/"),
                        channel("validation.set", "s3://bucket/validation/"),
                        channel("test_1", "https://bucket.s3.amazonaws.com/test/"),
                    ],
                },
            }),
        },
        Row {
            case: "inline JSON workload spec",
            members: json!({
                "AIWorkloadConfigs": {
                    "WorkloadSpec": { "Inline": "{\"concurrency\": 4, \"duration\": \"60s\"}" },
                },
            }),
        },
        Row {
            case: "inline YAML workload spec",
            members: json!({
                "AIWorkloadConfigs": {
                    "WorkloadSpec": { "Inline": "concurrency: 4\nduration: 60s\n" },
                },
            }),
        },
        Row {
            case: "empty tag list",
            members: json!({ "Tags": tags(0) }),
        },
        Row {
            case: "one tag",
            members: json!({ "Tags": tags(1) }),
        },
        Row {
            case: "fifty tags (TagList maximum)",
            members: json!({ "Tags": tags(50) }),
        },
        Row {
            case: "every optional member at once",
            members: json!({
                "DatasetConfig": {
                    "InputDataConfig": [
                        channel("train", "s3://bucket/train/"),
                        channel("eval", "s3://bucket/eval/"),
                    ],
                },
                "AIWorkloadConfigs": {
                    "WorkloadSpec": { "Inline": "concurrency: 1\n" },
                },
                "Tags": tags(3),
            }),
        },
    ]
}

/// A create request body: the name plus whatever optional members the row carries.
fn create_body(name: &str, members: &Value) -> Value {
    let mut body = json!({ "AIWorkloadConfigName": name });
    let object = body.as_object_mut().expect("the body is a JSON object");
    for (member, value) in members.as_object().expect("the row is a JSON object") {
        object.insert(member.clone(), value.clone());
    }
    body
}

/// RULE: a created AI workload configuration is readable back under its own name, and
/// the read replays exactly the members the create stored — nothing invented, nothing
/// dropped — plus the ARN the create answered and a creation timestamp.
#[tokio::test]
async fn describe_replays_every_member_the_create_stored() {
    for row in round_trip_rows() {
        let emulator = Emulator::new();
        let case = row.case;
        let name = "wlc-round-trip";
        let body = create_body(name, &row.members);

        let (status, created) = emulator.call("CreateAIWorkloadConfig", &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateAIWorkloadConfig to answer 200, got {status} \
             with {created:?}"
        );
        let arn = str_field(&created, "AIWorkloadConfigArn");
        assert_eq!(
            arn,
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:ai-workload-config/{name}"),
            "case {case:?}: expected the minted ARN to name the resource kind and name, \
             got {created:?}"
        );

        let (status, described) = emulator
            .call(
                "DescribeAIWorkloadConfig",
                &json!({ "AIWorkloadConfigName": name }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeAIWorkloadConfig to answer 200, got {status} \
             with {described:?}"
        );
        assert_eq!(
            str_field(&described, "AIWorkloadConfigName"),
            name,
            "case {case:?}: expected the stored name, got {described:?}"
        );
        assert_eq!(
            str_field(&described, "AIWorkloadConfigArn"),
            arn,
            "case {case:?}: expected the same ARN the create answered, got {described:?}"
        );

        // Each optional member comes back exactly as sent; one the request omitted is
        // absent, never defaulted to an empty value a caller cannot distinguish.
        for member in ["DatasetConfig", "AIWorkloadConfigs"] {
            assert_eq!(
                described.get(member),
                body.get(member),
                "case {case:?}: expected {member} to round-trip, got {:?}",
                described.get(member)
            );
        }
        let sent_tags = body
            .get("Tags")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let read_tags = described
            .get("Tags")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        assert_eq!(
            read_tags, sent_tags,
            "case {case:?}: expected the stored tags to round-trip, got {:?}",
            described.get("Tags")
        );
        // The tag set is the configuration's own collection, so the member is present
        // whatever is in it: a request that carried none — omitted or an explicitly empty
        // list — reports the EMPTY list rather than going missing (the rule the
        // `read_responses_report_what_a_resource_holds` suite pins).
        if sent_tags.is_empty() {
            assert_eq!(
                described.get("Tags"),
                Some(&json!([])),
                "case {case:?}: expected Tags to be the empty list when none were given, \
                 got {:?}",
                described.get("Tags")
            );
        }

        // The creation timestamp is present and stable: a second read reports the same
        // instant, so the read is reporting rather than re-stamping.
        let creation_time = described
            .get("CreationTime")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: expected a CreationTime in {described:?}"));
        let (status, again) = emulator
            .call(
                "DescribeAIWorkloadConfig",
                &json!({ "AIWorkloadConfigName": name }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the second read to answer 200, got {status} with {again:?}"
        );
        assert_eq!(
            again.get("CreationTime").and_then(Value::as_f64),
            Some(creation_time),
            "case {case:?}: expected CreationTime to be stable across reads, got {:?}",
            again.get("CreationTime")
        );
        assert_eq!(
            again, described,
            "case {case:?}: expected a read to change nothing a later read sees, got {again:?}"
        );
    }
}

/// RULE: the ARN a create mints is
/// `arn:aws:sagemaker:<signed region>:<account>:ai-workload-config/<name>`, whatever the
/// name's shape and whichever region the caller signed for (the emulator default when
/// the request is unsigned).
#[tokio::test]
async fn minted_arns_carry_the_signed_region_and_the_stored_name() {
    // Rows differ along the axes the rule generalises over: the name's shape and the
    // region. `AIWorkloadConfigName` targets `AIEntityName`, `len 1..63` with pattern
    // `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$` (smithy_query shape=AIEntityName), so the
    // rows sit on both published bounds and in between.
    let names = [
        "a",
        "wlc1",
        "demo-workload-config-with-hyphens",
        "a234567890123456789012345678901234567890123456789012345678901b",
    ];
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];

    for region in regions {
        for name in names {
            let emulator = Emulator::new();
            let case = format!("region={region} name={name}");
            let (status, created) = emulator
                .call_in(
                    region,
                    "CreateAIWorkloadConfig",
                    &json!({ "AIWorkloadConfigName": name }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected CreateAIWorkloadConfig to answer 200, got {status} \
                 with {created:?}"
            );
            assert_eq!(
                str_field(&created, "AIWorkloadConfigArn"),
                format!("arn:aws:sagemaker:{region}:{ACCOUNT}:ai-workload-config/{name}"),
                "case {case:?}: expected the ARN to carry the signed region and the name, \
                 got {created:?}"
            );
        }
    }

    // A name outside `AIEntityName`'s pattern or length never reaches the store: the
    // boundary validator refuses it, so no ARN is minted for it.
    // One past the published upper bound of `AIEntityName`'s `len 1..63`.
    let too_long = "a".repeat(64);
    for name in ["-leading-hyphen", "under_score", too_long.as_str()] {
        let emulator = Emulator::new();
        let (status, body) = emulator
            .call(
                "CreateAIWorkloadConfig",
                &json!({ "AIWorkloadConfigName": name }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case name={name:?}: expected a constraint refusal with HTTP 400, got {status} \
             with {body:?}"
        );
    }

    // An unsigned request names no region, so the ARN carries the emulator's default.
    let emulator = Emulator::new();
    let (status, body) = emulator
        .call_unsigned(
            "CreateAIWorkloadConfig",
            &json!({ "AIWorkloadConfigName": "unsigned-wlc" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case unsigned: expected CreateAIWorkloadConfig to answer 200, got {status} with {body:?}"
    );
    assert_eq!(
        str_field(&body, "AIWorkloadConfigArn"),
        format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:ai-workload-config/unsigned-wlc"),
        "case unsigned: expected the ARN to fall back to the default region, got {body:?}"
    );
}

/// RULE: `AIWorkloadConfigName` is unique per account and Region, so a create naming one
/// that already exists is refused with `ResourceInUse` and leaves the stored
/// configuration exactly as the first create left it.
#[tokio::test]
async fn a_name_already_in_use_is_refused_and_the_stored_config_is_untouched() {
    for row in round_trip_rows() {
        let emulator = Emulator::new();
        let case = row.case;
        let name = "wlc-duplicate";

        let (status, first) = emulator
            .call("CreateAIWorkloadConfig", &create_body(name, &row.members))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first create to answer 200, got {status} with {first:?}"
        );
        let (status, before) = emulator
            .call(
                "DescribeAIWorkloadConfig",
                &json!({ "AIWorkloadConfigName": name }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the first read to answer 200, got {status} with {before:?}"
        );

        // A second create under the same name, carrying DIFFERENT members, must not
        // overwrite the first.
        let (status, refused) = emulator
            .call(
                "CreateAIWorkloadConfig",
                &json!({
                    "AIWorkloadConfigName": name,
                    "AIWorkloadConfigs": { "WorkloadSpec": { "Inline": "concurrency: 99\n" } },
                    "Tags": tags(2),
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the duplicate create to answer 400, got {status} \
             with {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ResourceInUse",
            "case {case:?}: expected the modelled ResourceInUse code, got {refused:?}"
        );

        let (status, after) = emulator
            .call(
                "DescribeAIWorkloadConfig",
                &json!({ "AIWorkloadConfigName": name }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the read after the refusal to answer 200, got {status} \
             with {after:?}"
        );
        assert_eq!(
            after, before,
            "case {case:?}: expected the refused create to change nothing, got {after:?}"
        );
    }
}

/// RULE: a read of an AI workload configuration that does not exist is refused with
/// `ResourceNotFound`, never answered with a 200 carrying empty members — and a create
/// under one name does not make any other name readable.
#[tokio::test]
async fn describing_a_name_that_was_never_created_is_refused() {
    let emulator = Emulator::new();
    let (status, created) = emulator
        .call(
            "CreateAIWorkloadConfig",
            &json!({ "AIWorkloadConfigName": "present" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case setup: expected CreateAIWorkloadConfig to answer 200, got {status} with {created:?}"
    );

    // Rows differ along the ways a name can miss: never created, a prefix of one that
    // was, a suffix, and a case variant (names are matched exactly).
    for name in ["absent", "pres", "resent", "PRESENT", "present1"] {
        let (status, body) = emulator
            .call(
                "DescribeAIWorkloadConfig",
                &json!({ "AIWorkloadConfigName": name }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case name={name:?}: expected HTTP 400 for a name that was never created, \
             got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFound",
            "case name={name:?}: expected the modelled ResourceNotFound code, got {body:?}"
        );
    }

    // And the one that WAS created still reads back, so the refusals above are about the
    // name and not about a store the reads emptied.
    let (status, body) = emulator
        .call(
            "DescribeAIWorkloadConfig",
            &json!({ "AIWorkloadConfigName": "present" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case name=\"present\": expected HTTP 200, got {status} with {body:?}"
    );
}

/// RULE: distinct names are distinct resources — each create is stored under its own
/// name and each read answers that name's own ARN and members.
#[tokio::test]
async fn distinct_names_are_distinct_resources() {
    let emulator = Emulator::new();
    let rows = round_trip_rows();

    for (index, row) in rows.iter().enumerate() {
        let name = format!("wlc{index}");
        let (status, created) = emulator
            .call("CreateAIWorkloadConfig", &create_body(&name, &row.members))
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected create {name} to answer 200, got {status} with {created:?}",
            row.case
        );
    }

    for (index, row) in rows.iter().enumerate() {
        let name = format!("wlc{index}");
        let (status, described) = emulator
            .call(
                "DescribeAIWorkloadConfig",
                &json!({ "AIWorkloadConfigName": name }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected read {name} to answer 200, got {status} with {described:?}",
            row.case
        );
        assert_eq!(
            str_field(&described, "AIWorkloadConfigArn"),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:ai-workload-config/{name}"),
            "case {:?}: expected {name}'s own ARN, got {described:?}",
            row.case
        );
        let expected = create_body(&name, &row.members);
        for member in ["DatasetConfig", "AIWorkloadConfigs"] {
            assert_eq!(
                described.get(member),
                expected.get(member),
                "case {:?}: expected {name}'s own {member}, got {:?}",
                row.case,
                described.get(member)
            );
        }
    }
}

/// RULE: a delete releases the stored configuration and answers the ARN that
/// configuration carried, so afterwards the name reads back as not found and is free for
/// a new create — whatever members the released configuration held and whichever Region
/// the caller signed for.
#[tokio::test]
async fn delete_releases_the_config_and_answers_its_stored_arn() {
    // Rows differ along the axes the rule generalises over: the members the released
    // configuration carried (the round-trip table), the Region the ARN was minted in, and
    // the name's shape (both published bounds of `AIEntityName`'s `len 1..63`, from
    // smithy_query shape=AIEntityName).
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];
    let names = [
        "a",
        "wlc-to-delete",
        "a234567890123456789012345678901234567890123456789012345678901b",
    ];

    for row in round_trip_rows() {
        for (index, region) in regions.iter().enumerate() {
            let name = names[index % names.len()];
            let emulator = Emulator::new();
            let case = format!("{} region={region} name={name}", row.case);

            let (status, created) = emulator
                .call_in(region, "CreateAIWorkloadConfig", &create_body(name, &row.members))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the create to answer 200, got {status} with {created:?}"
            );
            let arn = str_field(&created, "AIWorkloadConfigArn");

            let (status, deleted) = emulator
                .call_in(
                    region,
                    "DeleteAIWorkloadConfig",
                    &json!({ "AIWorkloadConfigName": name }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected DeleteAIWorkloadConfig to answer 200, got {status} \
                 with {deleted:?}"
            );
            assert_eq!(
                str_field(&deleted, "AIWorkloadConfigArn"),
                arn,
                "case {case:?}: expected the delete to answer the ARN the create minted \
                 ({arn}), got {deleted:?}"
            );

            // The record is gone: the read that replayed it is now refused.
            let (status, read_back) = emulator
                .call_in(
                    region,
                    "DescribeAIWorkloadConfig",
                    &json!({ "AIWorkloadConfigName": name }),
                )
                .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected a read after the delete to answer 400, got {status} \
                 with {read_back:?}"
            );
            assert_eq!(
                error_code(&read_back),
                "ResourceNotFound",
                "case {case:?}: expected the modelled ResourceNotFound code after the delete, \
                 got {read_back:?}"
            );

            // And the name is free again: a new create under it is accepted (no
            // `ResourceInUse`) and reads back with its OWN members, not the released ones.
            let remade = json!({
                "AIWorkloadConfigName": name,
                "AIWorkloadConfigs": { "WorkloadSpec": { "Inline": "concurrency: 7\n" } },
            });
            let (status, recreated) = emulator
                .call_in(region, "CreateAIWorkloadConfig", &remade)
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the name to be reusable after the delete, got {status} \
                 with {recreated:?}"
            );
            assert_eq!(
                str_field(&recreated, "AIWorkloadConfigArn"),
                arn,
                "case {case:?}: expected the re-created configuration to carry the same ARN \
                 form, got {recreated:?}"
            );
            let (status, described) = emulator
                .call_in(
                    region,
                    "DescribeAIWorkloadConfig",
                    &json!({ "AIWorkloadConfigName": name }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the re-created configuration to read back, got {status} \
                 with {described:?}"
            );
            assert_eq!(
                described.get("AIWorkloadConfigs"),
                remade.get("AIWorkloadConfigs"),
                "case {case:?}: expected the re-created configuration's own members, got {:?}",
                described.get("AIWorkloadConfigs")
            );
            assert_eq!(
                described.get("DatasetConfig"),
                None,
                "case {case:?}: expected no member of the released configuration to survive, \
                 got {:?}",
                described.get("DatasetConfig")
            );
        }
    }
}

/// RULE: a delete naming a configuration that does not exist is refused with
/// `ResourceNotFound` — it is not idempotent, so a second delete of a name it just
/// released is refused too — and the refusal releases nothing else.
#[tokio::test]
async fn deleting_a_name_that_names_nothing_is_refused() {
    let emulator = Emulator::new();
    for name in ["present", "keeper"] {
        let (status, created) = emulator
            .call(
                "CreateAIWorkloadConfig",
                &json!({ "AIWorkloadConfigName": name }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case setup name={name:?}: expected the create to answer 200, got {status} \
             with {created:?}"
        );
    }

    // Rows differ along the ways a name can name nothing: never created, a prefix of one
    // that was, a suffix, a case variant (names are matched exactly), and an extended one.
    for name in ["absent", "pres", "resent", "PRESENT", "present1"] {
        let (status, body) = emulator
            .call(
                "DeleteAIWorkloadConfig",
                &json!({ "AIWorkloadConfigName": name }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case name={name:?}: expected HTTP 400 for a delete of a name that was never \
             created, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFound",
            "case name={name:?}: expected the modelled ResourceNotFound code, got {body:?}"
        );
    }

    // The first delete of an existing name succeeds; the SECOND is refused, because the
    // name no longer names a configuration.
    let (status, deleted) = emulator
        .call(
            "DeleteAIWorkloadConfig",
            &json!({ "AIWorkloadConfigName": "present" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case name=\"present\" first delete: expected 200, got {status} with {deleted:?}"
    );
    let (status, again) = emulator
        .call(
            "DeleteAIWorkloadConfig",
            &json!({ "AIWorkloadConfigName": "present" }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case name=\"present\" second delete: expected HTTP 400, got {status} with {again:?}"
    );
    assert_eq!(
        error_code(&again),
        "ResourceNotFound",
        "case name=\"present\" second delete: expected the modelled ResourceNotFound code, \
         got {again:?}"
    );

    // Every refusal above left the other configuration alone.
    let (status, keeper) = emulator
        .call(
            "DescribeAIWorkloadConfig",
            &json!({ "AIWorkloadConfigName": "keeper" }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case name=\"keeper\": expected the untouched configuration to still read back, \
         got {status} with {keeper:?}"
    );
}

/// RULE: a delete releases exactly the configuration it names — every other name keeps
/// its own record, with its own ARN and its own members.
#[tokio::test]
async fn delete_releases_only_the_configuration_it_names() {
    let emulator = Emulator::new();
    let rows = round_trip_rows();

    for (index, row) in rows.iter().enumerate() {
        let name = format!("wlc{index}");
        let (status, created) = emulator
            .call("CreateAIWorkloadConfig", &create_body(&name, &row.members))
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected create {name} to answer 200, got {status} with {created:?}",
            row.case
        );
    }

    // Delete each configuration in turn; after each one, every name not yet deleted still
    // reads back with its own ARN and members, and every name already deleted is refused.
    for (deleted_index, row) in rows.iter().enumerate() {
        let target = format!("wlc{deleted_index}");
        let (status, deleted) = emulator
            .call(
                "DeleteAIWorkloadConfig",
                &json!({ "AIWorkloadConfigName": target }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected delete {target} to answer 200, got {status} with {deleted:?}",
            row.case
        );
        assert_eq!(
            str_field(&deleted, "AIWorkloadConfigArn"),
            format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:ai-workload-config/{target}"),
            "case {:?}: expected delete {target} to answer {target}'s own ARN, got {deleted:?}",
            row.case
        );

        for (index, other) in rows.iter().enumerate() {
            let name = format!("wlc{index}");
            let (status, body) = emulator
                .call(
                    "DescribeAIWorkloadConfig",
                    &json!({ "AIWorkloadConfigName": name }),
                )
                .await;
            if index <= deleted_index {
                assert_eq!(
                    status, 400,
                    "case {:?}: expected the deleted {name} to be refused, got {status} \
                     with {body:?}",
                    other.case
                );
                assert_eq!(
                    error_code(&body),
                    "ResourceNotFound",
                    "case {:?}: expected ResourceNotFound for the deleted {name}, got {body:?}",
                    other.case
                );
            } else {
                assert_eq!(
                    status, 200,
                    "case {:?}: expected the surviving {name} to read back, got {status} \
                     with {body:?}",
                    other.case
                );
                assert_eq!(
                    str_field(&body, "AIWorkloadConfigArn"),
                    format!("arn:aws:sagemaker:us-east-1:{ACCOUNT}:ai-workload-config/{name}"),
                    "case {:?}: expected {name}'s own ARN, got {body:?}",
                    other.case
                );
                let expected = create_body(&name, &other.members);
                for member in ["DatasetConfig", "AIWorkloadConfigs"] {
                    assert_eq!(
                        body.get(member),
                        expected.get(member),
                        "case {:?}: expected {name}'s own {member} to survive the delete of \
                         {target}, got {:?}",
                        other.case,
                        body.get(member)
                    );
                }
            }
        }
    }
}
