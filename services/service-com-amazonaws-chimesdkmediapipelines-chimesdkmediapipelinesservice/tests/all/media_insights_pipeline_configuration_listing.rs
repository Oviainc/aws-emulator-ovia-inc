// SPDX-License-Identifier: CC0-1.0

//! `ListMediaInsightsPipelineConfigurations`, end to end over HTTP.
//!
//! Every assertion goes through the wire: the suite reuses the sibling lifecycle suite's
//! `Emulator` harness, which mounts this service on the real `MultiServiceRouter` and
//! dispatches `http::Request`s, so what it pins is the emulator's HTTP behaviour and not
//! an internal helper.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one row:
//!
//! * the listing reports every STORED configuration exactly once, in creation order, each
//!   summarised by the name, id and ARN the stored record carries — so a summary agrees
//!   member for member with what `GetMediaInsightsPipelineConfiguration` renders;
//! * it pages at `MaxResults` and carries `NextToken` only while records remain beyond the
//!   page, whatever the page size, and resuming from that token continues after the page;
//! * a `NextToken` this service never issued is the modelled 400 `BadRequestException`;
//! * `MaxResults` is served across its whole modelled 1..100 range and refused one past
//!   either published bound;
//! * the listing is a REPORT: two listings in a row answer alike, and a create or a delete
//!   is what changes the answer.

use serde_json::{json, Value};

use crate::media_insights_pipeline_configuration_lifecycle::{
    Emulator, Response, BAD_REQUEST_EXCEPTION, ROLE_ARN,
};

/// The listing's own path space, as the model binds it
/// (`smithy_query(query="operation", operation="ListMediaInsightsPipelineConfigurations")`
/// — `http.uri` `/media-insights-pipeline-configurations`, method `GET`).
const LIST_PATH: &str = "/media-insights-pipeline-configurations";

/// The create inputs the listing is asserted over. The rows differ along what the rule
/// generalises over — the NAME's form and length across the modelled
/// `MediaInsightsPipelineConfigurationNameString` pattern (`^[0-9a-zA-Z._-]+$`, len 2..64)
/// and the number and kind of `Elements` — because the listing must report them all alike.
fn create_rows() -> Vec<(String, Value)> {
    vec![
        (
            "c1".to_string(),
            json!({
                "Elements": [
                    {
                        "Type": "AmazonTranscribeProcessor",
                        "AmazonTranscribeProcessorConfiguration": { "LanguageCode": "en-US" },
                    },
                    {
                        "Type": "KinesisDataStreamSink",
                        "KinesisDataStreamSinkConfiguration": {
                            "InsightsTarget": "arn:aws:kinesis:us-west-2:000000000000:stream/insights",
                        },
                    },
                ],
            }),
        ),
        (
            "Listed.Conf_1-two".to_string(),
            json!({
                "Elements": [
                    {
                        "Type": "S3RecordingSink",
                        "S3RecordingSinkConfiguration": {
                            "Destination": "arn:aws:s3:::insights-bucket/recordings",
                            "RecordingFileFormat": "Wav",
                        },
                    },
                ],
                "Tags": [{ "Key": "team", "Value": "voice" }],
            }),
        ),
        (
            "20261029".to_string(),
            json!({
                "Elements": [
                    {
                        "Type": "SnsTopicSink",
                        "SnsTopicSinkConfiguration": {
                            "InsightsTarget": "arn:aws:sns:us-west-2:000000000000:insights",
                        },
                    },
                ],
            }),
        ),
        (
            "l".repeat(64),
            json!({
                "Elements": [
                    {
                        "Type": "AmazonTranscribeCallAnalyticsProcessor",
                        "AmazonTranscribeCallAnalyticsProcessorConfiguration": {
                            "LanguageCode": "en-US",
                        },
                    },
                    {
                        "Type": "LambdaFunctionSink",
                        "LambdaFunctionSinkConfiguration": {
                            "InsightsTarget": "arn:aws:lambda:us-west-2:000000000000:function:insights",
                        },
                    },
                    {
                        "Type": "SqsQueueSink",
                        "SqsQueueSinkConfiguration": {
                            "InsightsTarget": "arn:aws:sqs:us-west-2:000000000000:insights",
                        },
                    },
                ],
                "RealTimeAlertConfiguration": {
                    "Disabled": false,
                    "Rules": [
                        {
                            "Type": "KeywordMatch",
                            "KeywordMatchConfiguration": {
                                "RuleName": "escalate",
                                "Keywords": ["manager"],
                                "Negate": false,
                            },
                        },
                    ],
                },
            }),
        ),
        (
            "fifth-configuration".to_string(),
            json!({
                "Elements": [
                    {
                        "Type": "VoiceAnalyticsProcessor",
                        "VoiceAnalyticsProcessorConfiguration": {
                            "SpeakerSearchStatus": "Enabled",
                            "VoiceToneAnalysisStatus": "Enabled",
                        },
                    },
                    {
                        "Type": "KinesisDataStreamSink",
                        "KinesisDataStreamSinkConfiguration": {
                            "InsightsTarget": "arn:aws:kinesis:us-west-2:000000000000:stream/voice",
                        },
                    },
                ],
            }),
        ),
    ]
}

/// Create every row in order and hand back the names in CREATION order, which is the
/// order the listing must report them in.
async fn seed(emulator: &Emulator) -> Vec<String> {
    let mut names = Vec::new();
    for (name, extra) in create_rows() {
        let mut input = json!({
            "MediaInsightsPipelineConfigurationName": name,
            "ResourceAccessRoleArn": ROLE_ARN,
        });
        let object = input.as_object_mut().expect("the create input is an object");
        for (key, value) in extra.as_object().expect("the row's extras are an object") {
            object.insert(key.clone(), value.clone());
        }
        let created = emulator.create(input).await;
        assert_eq!(
            created.status, 201,
            "case {name:?}: expected the create to be served with 201, got {} body {:?}",
            created.status, created.body
        );
        names.push(name);
    }
    names
}

/// One page's summaries, as the output member carries them.
fn summaries(case: &str, page: &Response) -> Vec<Value> {
    assert_eq!(
        page.status, 200,
        "case {case:?}: expected 200, got {} body {:?}",
        page.status, page.body
    );
    page.body
        .pointer("/MediaInsightsPipelineConfigurations")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a MediaInsightsPipelineConfigurations list, got {:?}",
                page.body
            )
        })
}

fn member(case: &str, summary: &Value, name: &str) -> String {
    summary
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected {name}, got {summary:?}"))
        .to_string()
}

/// RULE: the listing reports every stored configuration once, in creation order, and each
/// summary's three members are the stored record's — the same values a read of that
/// configuration renders.
#[tokio::test]
async fn the_listing_reports_every_stored_configuration_from_the_store() {
    let emulator = Emulator::new();

    let empty = emulator.call("GET", LIST_PATH, None).await;
    assert_eq!(
        summaries("an empty store", &empty),
        Vec::<Value>::new(),
        "case {:?}: an empty store lists nothing, got {:?}",
        "an empty store",
        empty.body
    );
    assert!(
        empty.body.get("NextToken").is_none(),
        "case {:?}: nothing remains, so no NextToken is due, got {:?}",
        "an empty store",
        empty.body
    );

    let names = seed(&emulator).await;
    let listed = emulator.call("GET", LIST_PATH, None).await;
    let page = summaries("every row, unpaged", &listed);
    let reported: Vec<String> = page
        .iter()
        .map(|summary| {
            member(
                "every row, unpaged",
                summary,
                "MediaInsightsPipelineConfigurationName",
            )
        })
        .collect();
    assert_eq!(
        reported, names,
        "case {:?}: expected every configuration once in creation order, got {reported:?}",
        "every row, unpaged"
    );
    assert!(
        listed.body.get("NextToken").is_none(),
        "case {:?}: the whole collection fits, so no NextToken is due, got {:?}",
        "every row, unpaged",
        listed.body
    );

    // Each summary agrees with the stored record a read renders, member for member.
    for summary in &page {
        let name = member(
            "summary against the stored record",
            summary,
            "MediaInsightsPipelineConfigurationName",
        );
        let read = emulator.get(&name).await;
        assert_eq!(
            read.status, 200,
            "case {name:?}: expected the read to be served, got {} body {:?}",
            read.status, read.body
        );
        for (summary_member, read_member) in [
            (
                "MediaInsightsPipelineConfigurationId",
                "/MediaInsightsPipelineConfiguration/MediaInsightsPipelineConfigurationId",
            ),
            (
                "MediaInsightsPipelineConfigurationArn",
                "/MediaInsightsPipelineConfiguration/MediaInsightsPipelineConfigurationArn",
            ),
            (
                "MediaInsightsPipelineConfigurationName",
                "/MediaInsightsPipelineConfiguration/MediaInsightsPipelineConfigurationName",
            ),
        ] {
            let expected = read
                .body
                .pointer(read_member)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let actual = member("summary against the stored record", summary, summary_member);
            assert_eq!(
                actual, expected,
                "case {name:?}: the summary's {summary_member} must be the stored record's, got {actual:?}"
            );
        }
    }
}

/// RULE: the listing pages at `MaxResults`, carrying `NextToken` only while records remain
/// beyond the page, and resuming from that token continues after it.
#[tokio::test]
async fn the_listing_pages_at_max_results_in_creation_order() {
    let emulator = Emulator::new();
    let names = seed(&emulator).await;

    // Page sizes that divide the count, that do not, that equal it, and that exceed it,
    // plus the modelled range's upper bound.
    for page_size in [1, 2, 3, 5, 6, 100] {
        let case = format!("max-results={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut query = format!("{LIST_PATH}?max-results={page_size}");
        let mut pages = 0usize;
        loop {
            let page = emulator.call("GET", &query, None).await;
            let items = summaries(&case, &page);
            assert!(
                items.len() <= page_size,
                "case {case:?}: a page must not exceed the asked-for size, got {}",
                items.len()
            );
            seen.extend(
                items
                    .iter()
                    .map(|summary| member(&case, summary, "MediaInsightsPipelineConfigurationName")),
            );
            pages += 1;
            assert!(
                pages <= names.len() + 1,
                "case {case:?}: paging must terminate, but it has served {pages} pages"
            );
            let token = page.body.get("NextToken").and_then(Value::as_str);
            let remaining = names.len() - seen.len();
            assert_eq!(
                token.is_some(),
                remaining > 0,
                "case {case:?}: a NextToken is due exactly while records remain ({remaining} left), got {:?}",
                page.body.get("NextToken")
            );
            match token {
                Some(token) => {
                    query = format!("{LIST_PATH}?max-results={page_size}&next-token={token}")
                }
                None => break,
            }
        }
        assert_eq!(
            seen, names,
            "case {case:?}: paging must report every configuration once, in creation order"
        );
        let expected_pages = names.len().div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, got {pages}"
        );
    }
}

/// RULE: `MaxResults` is served across its whole modelled 1..100 range and refused one past
/// either published bound; a `NextToken` this service never issued is the modelled 400
/// `BadRequestException`.
#[tokio::test]
async fn the_listing_refuses_an_unissued_token_and_an_out_of_range_page_size() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // Both published bounds of the modelled `ResultMax` range (min 1, max 100), from
    // `smithy_query(query="operation", operation="ListMediaInsightsPipelineConfigurations")`.
    for good in [1, 2, 99, 100] {
        let case = format!("max-results={good}, inside the modelled range");
        let page = emulator
            .call("GET", &format!("{LIST_PATH}?max-results={good}"), None)
            .await;
        assert_eq!(
            page.status, 200,
            "case {case:?}: expected 200, got {} body {:?}",
            page.status, page.body
        );
    }

    let refusals = [
        format!("{LIST_PATH}?max-results=0"),
        format!("{LIST_PATH}?max-results=101"),
        format!("{LIST_PATH}?next-token=not-a-token-this-service-issued"),
    ];
    for query in refusals {
        let refused = emulator.call("GET", &query, None).await;
        assert_eq!(
            refused.status, 400,
            "case {query:?}: expected the modelled 400, got {} body {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            BAD_REQUEST_EXCEPTION,
            "case {query:?}: expected the modelled error shape, got {:?}",
            refused.body
        );
    }
}

/// RULE: the listing is a report — listing twice answers alike — and what changes its
/// answer is a create or a delete.
#[tokio::test]
async fn the_listing_writes_nothing_and_follows_the_store() {
    let emulator = Emulator::new();
    let names = seed(&emulator).await;

    let first = emulator.call("GET", LIST_PATH, None).await;
    let second = emulator.call("GET", LIST_PATH, None).await;
    assert_eq!(
        first.body, second.body,
        "case {:?}: two listings in a row must answer alike, got {:?} then {:?}",
        "a repeated listing", first.body, second.body
    );

    // A delete of each row in turn: every later listing reports exactly what remains, in
    // the order the survivors were created.
    let mut remaining = names.clone();
    while let Some(gone) = remaining.first().cloned() {
        let deleted = emulator.delete(&gone).await;
        assert_eq!(
            deleted.status, 204,
            "case {gone:?}: expected the delete to be served, got {} body {:?}",
            deleted.status, deleted.body
        );
        remaining.remove(0);
        let case = format!("after deleting {gone:?}");
        let listed = emulator.call("GET", LIST_PATH, None).await;
        let reported: Vec<String> = summaries(&case, &listed)
            .iter()
            .map(|summary| member(&case, summary, "MediaInsightsPipelineConfigurationName"))
            .collect();
        assert_eq!(
            reported, remaining,
            "case {case:?}: the listing must report exactly the survivors, got {reported:?}"
        );
    }
}
