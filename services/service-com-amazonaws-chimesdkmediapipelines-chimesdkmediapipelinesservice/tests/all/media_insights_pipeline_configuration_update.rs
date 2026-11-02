// SPDX-License-Identifier: CC0-1.0

//! `UpdateMediaInsightsPipelineConfiguration`, end to end over HTTP.
//!
//! The wire harness is the pool suite's ([`crate::kinesis_video_stream_pool_lifecycle`])
//! and the "every submitted member comes back" comparison is the create suite's
//! ([`crate::media_insights_pipeline_configuration_lifecycle`]), so this suite adds no
//! second copy of either: one `MultiServiceRouter` with this service mounted on one
//! shared engine, and every assertion about the emulator's HTTP behaviour.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! * an update REPLACES the configuration's settings — the access role, the processors
//!   and sinks (`Elements`) and the alert rules — wholesale, so a setting the update
//!   omits is gone afterwards, and every later read renders the new settings;
//! * an update leaves the configuration's IDENTITY alone: its name, ARN,
//!   `MediaInsightsPipelineConfigurationId` and `CreatedTimestamp` are the create's,
//!   while `UpdatedTimestamp` is at or after the creation instant;
//! * an update resolves either form of the `Identifier` — the name or the ARN — exactly
//!   as the read and the delete resolve it;
//! * an `Identifier` that resolves to no stored configuration is this service's modelled
//!   404 `NotFoundException`, and that refusal changes nothing;
//! * `Elements`, the one caller-chosen size the model leaves unbounded, is bounded per
//!   configuration on the update as it is on the create: at or under the maximum the
//!   update is served and stored in full, past it the modelled 400
//!   `BadRequestException` with the stored settings untouched.

use serde_json::{json, Value};
use service_com_amazonaws_chimesdkmediapipelines_chimesdkmediapipelinesservice::state::{
    too_many_elements_message, MAX_INSIGHTS_CONFIGURATION_ELEMENTS,
};

use crate::kinesis_video_stream_pool_lifecycle::{
    encode_label, iso8601_seconds, Emulator, Response, ACCOUNT, REGION,
};
use crate::media_insights_pipeline_configuration_lifecycle::assert_submitted_members_are_reported;

/// The collection route the configuration operations hang off, from
/// `smithy_query(query="digest")`: `POST /media-insights-pipeline-configurations` and
/// `GET`/`PUT`/`DELETE …/{Identifier}`.
const CONFIGURATIONS: &str = "/media-insights-pipeline-configurations";

/// The `ErrorCode` values the modelled errors carry in their body `Code` member, from
/// `smithy_query(query="shape",shape="ErrorCode")`.
const ERROR_CODE_NOT_FOUND: &str = "NotFound";
const ERROR_CODE_BAD_REQUEST: &str = "BadRequest";

/// The wire error-shape names restJson1 resolves from `x-amzn-errortype`.
const NOT_FOUND_EXCEPTION: &str = "NotFoundException";
const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// `ResourceAccessRoleArn` values satisfying the modelled `Arn` shape (len 1..1024,
/// pattern `^arn[\/\:\-\_\.a-zA-Z0-9]+$`): the one a create carries, and the DIFFERENT
/// one an update moves it to.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/media-insights-access";
const UPDATED_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/media-insights-updated";

/// Every `Type` the model declares for an element, from
/// `smithy_query(query="shape",shape="MediaInsightsPipelineConfigurationElementType")`.
/// Only `Type` is required on an element, so a row may name a type and carry no
/// element configuration.
const ELEMENT_TYPES: [&str; 9] = [
    "AmazonTranscribeCallAnalyticsProcessor",
    "VoiceAnalyticsProcessor",
    "AmazonTranscribeProcessor",
    "KinesisDataStreamSink",
    "LambdaFunctionSink",
    "SqsQueueSink",
    "SnsTopicSink",
    "S3RecordingSink",
    "VoiceEnhancementSink",
];

async fn create(emulator: &Emulator, input: Value) -> Response {
    emulator.call("POST", CONFIGURATIONS, Some(input)).await
}

async fn get(emulator: &Emulator, identifier: &str) -> Response {
    emulator
        .call(
            "GET",
            &format!("{CONFIGURATIONS}/{}", encode_label(identifier)),
            None,
        )
        .await
}

async fn update(emulator: &Emulator, identifier: &str, input: Value) -> Response {
    emulator
        .call(
            "PUT",
            &format!("{CONFIGURATIONS}/{}", encode_label(identifier)),
            Some(input),
        )
        .await
}

async fn delete(emulator: &Emulator, identifier: &str) -> Response {
    emulator
        .call(
            "DELETE",
            &format!("{CONFIGURATIONS}/{}", encode_label(identifier)),
            None,
        )
        .await
}

/// One sink element, the smallest shape the model allows beyond a bare `Type`: a type
/// plus the one ARN its configuration carries. `index` only keeps the ARNs distinct.
fn sink_element(index: usize) -> Value {
    json!({
        "Type": "SqsQueueSink",
        "SqsQueueSinkConfiguration": {
            "InsightsTarget": format!("arn:aws:sqs:{REGION}:{ACCOUNT}:insights-{index}"),
        },
    })
}

/// The create every update row starts from, named `name`: one processor and one sink,
/// and a real-time alert configuration, so a row can both replace settings and drop
/// one the create had.
fn create_body(name: &str) -> Value {
    json!({
        "MediaInsightsPipelineConfigurationName": name,
        "ResourceAccessRoleArn": ROLE_ARN,
        "Elements": [
            {
                "Type": "AmazonTranscribeProcessor",
                "AmazonTranscribeProcessorConfiguration": { "LanguageCode": "en-US" },
            },
            {
                "Type": "KinesisDataStreamSink",
                "KinesisDataStreamSinkConfiguration": {
                    "InsightsTarget": format!("arn:aws:kinesis:{REGION}:{ACCOUNT}:stream/insights"),
                },
            },
        ],
        "RealTimeAlertConfiguration": {
            "Disabled": false,
            "Rules": [{
                "Type": "KeywordMatch",
                "KeywordMatchConfiguration": {
                    "RuleName": "escalate",
                    "Keywords": ["manager"],
                    "Negate": false,
                },
            }],
        },
    })
}

/// The update bodies the rules are asserted over. The rows differ along what the rules
/// generalise over: how many elements the new settings carry and of which declared
/// types, whether the access role changes, and whether the alert configuration is
/// replaced, dropped or grown to the three rules the model's `Rules` list allows.
fn update_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "one sink only, the alert configuration dropped, a new access role",
            json!({
                "ResourceAccessRoleArn": UPDATED_ROLE_ARN,
                "Elements": [sink_element(1)],
            }),
        ),
        (
            "the same access role, a different processor and sink, alerts replaced",
            json!({
                "ResourceAccessRoleArn": ROLE_ARN,
                "Elements": [
                    {
                        "Type": "AmazonTranscribeCallAnalyticsProcessor",
                        "AmazonTranscribeCallAnalyticsProcessorConfiguration": {
                            "LanguageCode": "en-GB",
                        },
                    },
                    {
                        "Type": "S3RecordingSink",
                        "S3RecordingSinkConfiguration": {
                            "Destination": "arn:aws:s3:::insights-bucket/updated",
                            "RecordingFileFormat": "Opus",
                        },
                    },
                ],
                "RealTimeAlertConfiguration": {
                    "Disabled": true,
                    "Rules": [{
                        "Type": "Sentiment",
                        "SentimentConfiguration": {
                            "RuleName": "unhappy",
                            "SentimentType": "NEGATIVE",
                            "TimePeriod": 60,
                        },
                    }],
                },
            }),
        ),
        (
            "one element of every type the model declares, no alert configuration",
            json!({
                "ResourceAccessRoleArn": UPDATED_ROLE_ARN,
                "Elements": ELEMENT_TYPES
                    .iter()
                    .map(|element_type| json!({ "Type": element_type }))
                    .collect::<Vec<Value>>(),
            }),
        ),
        (
            "the three alert rules the model's Rules list allows at most",
            json!({
                "ResourceAccessRoleArn": UPDATED_ROLE_ARN,
                "Elements": [sink_element(7)],
                "RealTimeAlertConfiguration": {
                    "Disabled": false,
                    "Rules": [
                        {
                            "Type": "KeywordMatch",
                            "KeywordMatchConfiguration": {
                                "RuleName": "escalate-2",
                                "Keywords": ["supervisor", "manager"],
                                "Negate": true,
                            },
                        },
                        {
                            "Type": "Sentiment",
                            "SentimentConfiguration": {
                                "RuleName": "unhappy-2",
                                "SentimentType": "NEGATIVE",
                                "TimePeriod": 180,
                            },
                        },
                        {
                            "Type": "IssueDetection",
                            "IssueDetectionConfiguration": { "RuleName": "issue-2" },
                        },
                    ],
                },
            }),
        ),
    ]
}

/// RULE: an update replaces the configuration's SETTINGS wholesale and leaves its
/// IDENTITY alone, whichever form of the `Identifier` named it, and every later read
/// renders the new settings.
#[tokio::test]
async fn an_update_replaces_the_settings_and_leaves_the_identity_alone() {
    for (row, body) in update_rows() {
        // Both identifier forms: a name, and the ARN the create derived.
        for by_arn in [false, true] {
            let case = format!("{row} / by {}", if by_arn { "ARN" } else { "name" });
            let emulator = Emulator::new();
            let name = "updated-configuration";

            let created = create(&emulator, create_body(name)).await;
            assert_eq!(
                created.status, 201,
                "case {case:?}: the configuration must be created, got {} with {:?}",
                created.status, created.body
            );
            let created_configuration = created
                .body
                .get("MediaInsightsPipelineConfiguration")
                .cloned()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: no MediaInsightsPipelineConfiguration in {:?}",
                        created.body
                    )
                });
            let arn = created
                .str_at("/MediaInsightsPipelineConfiguration/MediaInsightsPipelineConfigurationArn");
            let identifier = if by_arn { arn.clone() } else { name.to_string() };

            let updated = update(&emulator, &identifier, body.clone()).await;
            assert_eq!(
                updated.status, 200,
                "case {case:?}: expected 200 from the update, got {} with {:?}",
                updated.status, updated.body
            );
            let updated_configuration = updated
                .body
                .get("MediaInsightsPipelineConfiguration")
                .cloned()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: no MediaInsightsPipelineConfiguration in {:?}",
                        updated.body
                    )
                });

            // The IDENTITY the create minted is untouched.
            for member in [
                "MediaInsightsPipelineConfigurationName",
                "MediaInsightsPipelineConfigurationArn",
                "MediaInsightsPipelineConfigurationId",
                "CreatedTimestamp",
            ] {
                assert_eq!(
                    updated_configuration.get(member),
                    created_configuration.get(member),
                    "case {case:?}: an update must not change {member}, got {:?}",
                    updated_configuration.get(member)
                );
            }

            // Every SETTING the update submitted comes back as submitted.
            for member in [
                "ResourceAccessRoleArn",
                "Elements",
                "RealTimeAlertConfiguration",
            ] {
                assert_submitted_members_are_reported(
                    &format!("{case} / {member}"),
                    body.get(member),
                    updated_configuration.get(member),
                );
            }
            // Wholesale replacement: an alert configuration the update omitted is gone,
            // even though the create carried one.
            if body.get("RealTimeAlertConfiguration").is_none() {
                assert_eq!(
                    updated_configuration.get("RealTimeAlertConfiguration"),
                    None,
                    "case {case:?}: an update that names no RealTimeAlertConfiguration leaves \
                     none, got {:?}",
                    updated_configuration.get("RealTimeAlertConfiguration")
                );
            }

            // `UpdatedTimestamp` is an ISO-8601 instant at or after the creation one: the
            // record really did change, and the clock never runs backwards.
            let created_at = iso8601_seconds(
                &case,
                &created.str_at("/MediaInsightsPipelineConfiguration/CreatedTimestamp"),
            );
            let updated_at = iso8601_seconds(
                &case,
                &updated.str_at("/MediaInsightsPipelineConfiguration/UpdatedTimestamp"),
            );
            assert!(
                updated_at >= created_at,
                "case {case:?}: expected UpdatedTimestamp at or after CreatedTimestamp \
                 ({created_at}), got {updated_at}"
            );

            // PERSISTED: both identifier forms read the updated record back, and reading
            // twice answers alike because a read writes nothing.
            for read_by in [name, arn.as_str()] {
                for attempt in 1..=2 {
                    let fetched = get(&emulator, read_by).await;
                    assert_eq!(
                        fetched.status, 200,
                        "case {case:?}: read {attempt} of {read_by:?} expected 200, got {} with {:?}",
                        fetched.status, fetched.body
                    );
                    assert_eq!(
                        fetched.body.get("MediaInsightsPipelineConfiguration"),
                        Some(&updated_configuration),
                        "case {case:?}: read {attempt} of {read_by:?} must report what the update \
                         persisted"
                    );
                }
            }
        }
    }
}

/// Assert one answer is this service's modelled not-found refusal, in restJson1's error
/// framing.
fn assert_not_found(case: &str, response: &Response) {
    assert_eq!(
        response.status, 404,
        "case {case:?}: expected 404, got {} with {:?}",
        response.status, response.body
    );
    assert_eq!(
        response.error_code(),
        NOT_FOUND_EXCEPTION,
        "case {case:?}: expected the modelled {NOT_FOUND_EXCEPTION}, got {:?}",
        response.body
    );
    assert_eq!(
        response.body.get("Code").and_then(Value::as_str),
        Some(ERROR_CODE_NOT_FOUND),
        "case {case:?}: expected body Code {ERROR_CODE_NOT_FOUND:?}, got {:?}",
        response.body
    );
    for member in ["Message", "RequestId"] {
        let value = response.body.get(member).and_then(Value::as_str);
        assert!(
            value.is_some_and(|text| !text.is_empty()),
            "case {case:?}: the modelled error carries a non-empty {member}, got {:?}",
            response.body
        );
    }
}

/// RULE: an `Identifier` that resolves to no stored configuration is the modelled 404
/// `NotFoundException`, whatever form it takes and whether nothing was ever created
/// under it or it has been deleted — and a refused update leaves every other
/// configuration alone.
#[tokio::test]
async fn an_update_of_an_unknown_identifier_is_the_modelled_not_found_and_changes_nothing() {
    let emulator = Emulator::new();

    // A live neighbour, so a miss is decided per identifier and not by an empty store.
    let neighbour = "live-configuration";
    let created = create(&emulator, create_body(neighbour)).await;
    assert_eq!(
        created.status, 201,
        "the neighbour must be created, got {} with {:?}",
        created.status, created.body
    );
    let neighbour_before = created
        .body
        .get("MediaInsightsPipelineConfiguration")
        .cloned()
        .expect("a created configuration is reported");

    // A configuration created and then deleted: its name and its ARN resolved once and
    // must not resolve now.
    let gone = "deleted-configuration";
    let gone_created = create(&emulator, create_body(gone)).await;
    assert_eq!(
        gone_created.status, 201,
        "the configuration to delete must be created, got {} with {:?}",
        gone_created.status, gone_created.body
    );
    let gone_arn = gone_created
        .str_at("/MediaInsightsPipelineConfiguration/MediaInsightsPipelineConfigurationArn");
    let deleted = delete(&emulator, gone).await;
    assert_eq!(
        deleted.status, 204,
        "the configuration must delete, got {} with {:?}",
        deleted.status, deleted.body
    );

    let rows: Vec<(&str, String)> = vec![
        (
            "a well-formed name nothing was created under",
            "no-such-configuration".to_string(),
        ),
        (
            "this service's own ARN form for a name nothing holds",
            format!(
                "arn:aws:chime:{REGION}:{ACCOUNT}:media-insights-pipeline-configuration/no-such-configuration"
            ),
        ),
        (
            "an ARN of another resource type in the same namespace",
            format!("arn:aws:chime:{REGION}:{ACCOUNT}:media-pipeline/8f14e45f-ceea-467a-9ba1-3d1e2b3c4d5e"),
        ),
        (
            "a GUID, which is not an identifier form",
            "8f14e45f-ceea-467a-9ba1-3d1e2b3c4d5e".to_string(),
        ),
        ("a deleted configuration's name", gone.to_string()),
        ("a deleted configuration's ARN", gone_arn),
        (
            // The label's own modelled maximum: `NonEmptyString` is len 0..1024.
            "an identifier at the Identifier label's 1024-character maximum",
            "z".repeat(1024),
        ),
    ];

    for (case, identifier) in rows {
        let refused = update(
            &emulator,
            &identifier,
            json!({
                "ResourceAccessRoleArn": UPDATED_ROLE_ARN,
                "Elements": [sink_element(0)],
            }),
        )
        .await;
        assert_not_found(case, &refused);

        let neighbour_now = get(&emulator, neighbour).await;
        assert_eq!(
            neighbour_now.body.get("MediaInsightsPipelineConfiguration"),
            Some(&neighbour_before),
            "case {case:?}: a refused update must leave the live configuration alone, got {:?}",
            neighbour_now.body
        );
    }

    // The identifier refusal is authoritative over this service's own per-record element
    // bound: an unknown identifier is the modelled 404 however large the settings are.
    let over_bound: Vec<Value> = (0..=MAX_INSIGHTS_CONFIGURATION_ELEMENTS)
        .map(sink_element)
        .collect();
    let refused = update(
        &emulator,
        "no-such-configuration",
        json!({
            "ResourceAccessRoleArn": UPDATED_ROLE_ARN,
            "Elements": over_bound,
        }),
    )
    .await;
    assert_not_found(
        "an unknown identifier carrying more elements than the service stores",
        &refused,
    );
}

/// RULE: a request member whose size the caller chooses and the model does not bound is
/// bounded by the service before it is stored — on the UPDATE as on the create, because
/// an update is the other way the stored element list grows. At or under the
/// per-configuration maximum the update is served and persisted in full; past it the
/// answer is the modelled 400 `BadRequestException` and the stored settings are
/// untouched.
///
/// `Elements` is that member: `smithy_query(query="shape",
/// shape="MediaInsightsPipelineConfigurationElements")` reports a list with NO
/// `@length`, while every member inside an element is `@length`-bounded — so the COUNT
/// is what the service caps, and `MAX_INSIGHTS_CONFIGURATION_ELEMENTS` is that cap. The
/// rows span the class: one element, as many as the model declares types, exactly the
/// cap, one past it, and far past it.
#[tokio::test]
async fn the_element_list_is_bounded_on_an_update_and_a_refusal_changes_nothing() {
    let cap = MAX_INSIGHTS_CONFIGURATION_ELEMENTS;
    let rows: [(&str, usize, u16); 5] = [
        ("one element", 1, 200),
        ("as many elements as the model declares types", 9, 200),
        ("exactly the per-configuration maximum", cap, 200),
        ("one element past the maximum", cap + 1, 400),
        ("far past the maximum", cap * 16, 400),
    ];

    for (case, count, expected_status) in rows {
        let emulator = Emulator::new();
        let name = "bounded-configuration";
        let created = create(&emulator, create_body(name)).await;
        assert_eq!(
            created.status, 201,
            "case {case:?}: the configuration must be created, got {} with {:?}",
            created.status, created.body
        );
        let before = created
            .body
            .get("MediaInsightsPipelineConfiguration")
            .cloned()
            .expect("a created configuration is reported");

        let elements: Vec<Value> = (0..count).map(sink_element).collect();
        let response = update(
            &emulator,
            name,
            json!({
                "ResourceAccessRoleArn": UPDATED_ROLE_ARN,
                "Elements": elements,
            }),
        )
        .await;
        assert_eq!(
            response.status, expected_status,
            "case {case:?} ({count} elements): expected {expected_status}, got {} with {:?}",
            response.status, response.body
        );

        let stored = get(&emulator, name).await;
        if expected_status == 200 {
            // Served means PERSISTED: the read reports every element back.
            let reported = stored
                .body
                .pointer("/MediaInsightsPipelineConfiguration/Elements")
                .and_then(Value::as_array)
                .map(Vec::len);
            assert_eq!(
                reported,
                Some(count),
                "case {case:?}: the stored configuration must report all {count} elements, got \
                 {reported:?}"
            );
        } else {
            assert_eq!(
                response.error_code(),
                BAD_REQUEST_EXCEPTION,
                "case {case:?}: expected the modelled {BAD_REQUEST_EXCEPTION}, got {:?}",
                response.body
            );
            assert_eq!(
                response.body.get("Code").and_then(Value::as_str),
                Some(ERROR_CODE_BAD_REQUEST),
                "case {case:?}: expected body Code {ERROR_CODE_BAD_REQUEST:?}, got {:?}",
                response.body
            );
            // The refusal is worded by the one function the create's refusal uses, so the
            // count the request sent and the bound both reach the caller.
            assert_eq!(
                response.body.get("Message").and_then(Value::as_str),
                Some(too_many_elements_message(count).as_str()),
                "case {case:?}: expected the shared refusal wording, got {:?}",
                response.body
            );
            // Refused before anything was written: the stored settings are the create's.
            assert_eq!(
                stored.body.get("MediaInsightsPipelineConfiguration"),
                Some(&before),
                "case {case:?}: a refused update must leave the stored settings alone, got {:?}",
                stored.body
            );
        }
    }
}
