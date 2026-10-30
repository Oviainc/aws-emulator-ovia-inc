// SPDX-License-Identifier: CC0-1.0

//! The channel-definition distinctness rule, read wherever a start says which speaker
//! is on which channel.
//!
//! THE RULE UNDER TEST: a start whose channel definitions do not each name a different
//! audio channel and a different participant is refused with the modelled
//! `BadRequestException` (HTTP 400) and stores nothing, while a start whose definitions
//! are distinct — or that names no participant where the member is optional — is
//! accepted and reads its definitions back unchanged. A definition "makes it possible
//! to specify which speaker is on which channel", one channel and one participant per
//! definition (`smithy_query(query="shape",
//! shape="ChannelDefinition,MedicalScribeChannelDefinition")`), so a list naming one
//! channel twice, or one participant twice, says nothing about where a speaker is.
//!
//! The rows span the class rather than one request: both operations that accept channel
//! definitions, each published participant of each family's own role enum
//! (`smithy_query(query="shape",
//! shape="ParticipantRole,MedicalScribeParticipantRole")`), both published channel ids
//! (`ChannelId` is `@range(min: 0, max: 1)`), each way a pair can collide (the channel,
//! the participant, or both), the participants swapped between the channels, and — for
//! the family whose `ParticipantRole` is OPTIONAL — definitions that name one
//! participant or none at all. A start carrying no definitions is a row too: the rule
//! decides nothing about it.
//!
//! The emulator, the call and the answer assertions are the transcription-job suite's
//! harness (`crate::transcription_jobs`), reused rather than stood up a second time.

use serde_json::{json, Value};

use crate::transcription_jobs::{Reply, Transcribe};

/// A valid `DataAccessRoleArn`, whose `@pattern` pins it to an IAM role ARN
/// (`smithy_query(query="operation", operation="StartMedicalScribeJob")`).
const ROLE_ARN: &str = "arn:aws:iam::111122223333:role/ChannelDefinitionsAccess";

/// How this service refuses channel definitions that are not distinct — one sentence
/// for every family that accepts them.
const NOT_DISTINCT_MESSAGE: &str =
    "Each ChannelDefinition must name a different ChannelId and a different \
     ParticipantRole. Correct your request and try again.";

/// One pair of channel definitions a start can ask for: per definition, the channel id
/// and which of the family's two published participants it names (`None` to leave the
/// `ParticipantRole` member out).
type Definitions = [(i32, Option<usize>); 2];

/// One row of the class: the definitions a start asks for (`None` for a start that
/// names none) and whether the rule refuses them.
struct Row {
    case: &'static str,
    definitions: Option<Definitions>,
    refused: bool,
}

/// The rows whose every definition names a participant, so they apply to every family
/// that accepts channel definitions.
///
/// Both published `ChannelId` values and both published participants of whichever
/// family is under test appear, in both pairings, and every way two definitions can
/// collide is a row of its own.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a start naming no channel definitions at all",
            definitions: None,
            refused: false,
        },
        Row {
            case: "each channel with its own participant",
            definitions: Some([(0, Some(0)), (1, Some(1))]),
            refused: false,
        },
        Row {
            case: "the participants on the opposite channels",
            definitions: Some([(0, Some(1)), (1, Some(0))]),
            refused: false,
        },
        Row {
            case: "both definitions on the first channel",
            definitions: Some([(0, Some(0)), (0, Some(1))]),
            refused: true,
        },
        Row {
            case: "both definitions on the second channel",
            definitions: Some([(1, Some(0)), (1, Some(1))]),
            refused: true,
        },
        Row {
            case: "the first participant named on both channels",
            definitions: Some([(0, Some(0)), (1, Some(0))]),
            refused: true,
        },
        Row {
            case: "the second participant named on both channels",
            definitions: Some([(0, Some(1)), (1, Some(1))]),
            refused: true,
        },
        Row {
            case: "one channel and one participant, each named twice",
            definitions: Some([(1, Some(0)), (1, Some(0))]),
            refused: true,
        },
    ]
}

/// The rows that leave `ParticipantRole` out of a definition.
///
/// Only a family whose role member is OPTIONAL can ask for these: a definition that
/// names no participant has not named one twice, so distinctness is decided over the
/// channels alone. Where the member is `@required` the very same request is refused at
/// the request boundary before the rule is read at all, which the test asserts too.
fn unnamed_role_rows() -> Vec<Row> {
    vec![
        Row {
            case: "neither definition naming a participant",
            definitions: Some([(0, None), (1, None)]),
            refused: false,
        },
        Row {
            case: "only the first definition naming a participant",
            definitions: Some([(0, Some(0)), (1, None)]),
            refused: false,
        },
        Row {
            case: "only the second definition naming a participant",
            definitions: Some([(0, None), (1, Some(1))]),
            refused: false,
        },
        Row {
            case: "neither definition naming a participant, both on one channel",
            definitions: Some([(1, None), (1, None)]),
            refused: true,
        },
    ]
}

/// One job family whose start accepts channel definitions: how its request carries
/// them, which participants its own role enum publishes, and how a started job is read
/// back.
struct Family {
    case: &'static str,
    start: &'static str,
    read: &'static str,
    /// The member a read request names the job in.
    read_name_member: &'static str,
    /// The member a read answer carries the job record in.
    record: &'static str,
    /// The two values of this family's participant-role enum.
    roles: [&'static str; 2],
    /// Whether this family models `ParticipantRole` `@required`.
    role_required: bool,
    /// A start request for the job `name` asking for `definitions` (absent when the row
    /// names none).
    request: fn(name: &str, definitions: Option<Value>) -> Value,
}

/// The two operations that accept channel definitions (`smithy_query(query="shape",
/// shape="StartCallAnalyticsJobRequest,StartMedicalScribeJobRequest")`): the same rows
/// apply to each, so a rule applied to one and not the other fails here.
fn families() -> Vec<Family> {
    vec![
        Family {
            case: "call analytics job",
            start: "StartCallAnalyticsJob",
            read: "GetCallAnalyticsJob",
            read_name_member: "CallAnalyticsJobName",
            record: "CallAnalyticsJob",
            // `ParticipantRole`'s values (`smithy_query(query="shape",
            // shape="ParticipantRole")`).
            roles: ["AGENT", "CUSTOMER"],
            role_required: false,
            request: |name, definitions| {
                let mut request = json!({
                    "CallAnalyticsJobName": name,
                    "Media": {"MediaFileUri": "s3://example-media/two-channel.wav"}
                });
                if let Some(definitions) = definitions {
                    request
                        .as_object_mut()
                        .expect("a start request is a JSON object")
                        .insert("ChannelDefinitions".to_string(), definitions);
                }
                request
            },
        },
        Family {
            case: "medical scribe job",
            start: "StartMedicalScribeJob",
            read: "GetMedicalScribeJob",
            read_name_member: "MedicalScribeJobName",
            record: "MedicalScribeJob",
            // `MedicalScribeParticipantRole`'s values (`smithy_query(query="shape",
            // shape="MedicalScribeParticipantRole")`).
            roles: ["CLINICIAN", "PATIENT"],
            role_required: true,
            request: |name, definitions| {
                // "A MedicalScribeChannelDefinitions array should be set if and only if
                // the ChannelIdentification value of Settings is set to true", and
                // exactly one of the two speaker-partitioning modes must be chosen, so
                // a row that names no definitions partitions by speaker labels instead.
                let settings = match definitions {
                    Some(_) => json!({"ChannelIdentification": true}),
                    None => json!({"ShowSpeakerLabels": true, "MaxSpeakerLabels": 2}),
                };
                let mut request = json!({
                    "MedicalScribeJobName": name,
                    "Media": {"MediaFileUri": "s3://example-media/two-channel.wav"},
                    "OutputBucketName": "example-scribe-output",
                    "DataAccessRoleArn": ROLE_ARN,
                    "Settings": settings
                });
                if let Some(definitions) = definitions {
                    request
                        .as_object_mut()
                        .expect("a start request is a JSON object")
                        .insert("ChannelDefinitions".to_string(), definitions);
                }
                request
            },
        },
    ]
}

/// The `ChannelDefinitions` list a row asks of a family, with the family's own
/// participant names filled in.
fn definitions_json(family: &Family, row: &Row) -> Option<Value> {
    row.definitions.map(|definitions| {
        Value::Array(
            definitions
                .iter()
                .map(|(channel_id, role)| {
                    let mut definition = serde_json::Map::new();
                    definition.insert("ChannelId".to_string(), json!(channel_id));
                    if let Some(index) = role {
                        definition
                            .insert("ParticipantRole".to_string(), json!(family.roles[*index]));
                    }
                    Value::Object(definition)
                })
                .collect(),
        )
    })
}

/// Read the job `name` of `family` back.
async fn read(transcribe: &Transcribe, family: &Family, name: &str) -> Reply {
    let mut request = serde_json::Map::new();
    request.insert(family.read_name_member.to_string(), json!(name));
    transcribe.call(family.read, Value::Object(request)).await
}

#[tokio::test]
async fn channel_definitions_that_are_not_distinct_are_refused_by_every_start() {
    for family in families() {
        let transcribe = Transcribe::new();
        for (index, row) in rows().iter().enumerate().filter(|(_, row)| row.refused) {
            let case = format!("{} / {}", family.case, row.case);
            let name = format!("not-distinct-{index}");
            let asked = definitions_json(&family, row);
            let refused = transcribe
                .call(family.start, (family.request)(&name, asked))
                .await;
            refused.refused(&case, 400, "BadRequestException");

            // One sentence for every family that accepts channel definitions, naming
            // the members the caller sent and no value of the request.
            let message = refused.error_message(&case);
            assert_eq!(
                message, NOT_DISTINCT_MESSAGE,
                "case {case:?}: expected the refusal {NOT_DISTINCT_MESSAGE:?}, got \
                 {message:?}"
            );

            // A refused start stores nothing, so the name it asked for is unknown.
            read(&transcribe, &family, &name)
                .await
                .refused(&case, 400, "BadRequestException");
        }
    }
}

#[tokio::test]
async fn distinct_channel_definitions_are_accepted_and_read_back() {
    for family in families() {
        let transcribe = Transcribe::new();
        for (index, row) in rows().iter().enumerate().filter(|(_, row)| !row.refused) {
            let case = format!("{} / {}", family.case, row.case);
            let name = format!("distinct-{index}");
            let asked = definitions_json(&family, row);
            transcribe
                .call(family.start, (family.request)(&name, asked.clone()))
                .await
                .ok_status(&case, 200);

            // The served start stored the definitions, so the family's reader reports
            // them unchanged — and a row that asked for none has none invented for it.
            let fetched = read(&transcribe, &family, &name).await;
            let stored = fetched.ok(&case)[family.record]["ChannelDefinitions"].clone();
            let expected = asked.unwrap_or(Value::Null);
            assert_eq!(
                stored, expected,
                "case {case:?}: the stored job must report the channel definitions it \
                 was started with, expected {expected:?}, got {stored:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_definition_naming_no_participant_is_decided_by_its_channel_alone() {
    for family in families() {
        let transcribe = Transcribe::new();
        for (index, row) in unnamed_role_rows().iter().enumerate() {
            let case = format!("{} / {}", family.case, row.case);
            let name = format!("unnamed-role-{index}");
            let asked = definitions_json(&family, row);
            let answer = transcribe
                .call(family.start, (family.request)(&name, asked.clone()))
                .await;

            // Where the family models `ParticipantRole` `@required`, a definition that
            // names no participant never reaches the distinctness rule: the request
            // boundary refuses it for the missing member.
            if family.role_required {
                answer.refused(&case, 400, "ValidationException");
                read(&transcribe, &family, &name)
                    .await
                    .refused(&case, 400, "BadRequestException");
                continue;
            }

            if row.refused {
                answer.refused(&case, 400, "BadRequestException");
                let message = answer.error_message(&case);
                assert_eq!(
                    message, NOT_DISTINCT_MESSAGE,
                    "case {case:?}: expected the refusal {NOT_DISTINCT_MESSAGE:?}, got \
                     {message:?}"
                );
                read(&transcribe, &family, &name)
                    .await
                    .refused(&case, 400, "BadRequestException");
                continue;
            }

            answer.ok_status(&case, 200);
            let fetched = read(&transcribe, &family, &name).await;
            let stored = fetched.ok(&case)[family.record]["ChannelDefinitions"].clone();
            let expected = asked.unwrap_or(Value::Null);
            assert_eq!(
                stored, expected,
                "case {case:?}: the stored job must report the channel definitions it \
                 was started with, expected {expected:?}, got {stored:?}"
            );
        }
    }
}
