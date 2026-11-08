// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch Logs data-processing pipeline a destination can enable, run over one
//! delivered record.
//!
//! Two of the `ProcessorType` values a destination's `ProcessingConfiguration` can enable
//! are about the payload CloudWatch Logs delivers and nothing else
//! ([`crate::state::CLOUDWATCH_LOGS_PROCESSOR_TYPES`]), which is why a stream enabling one
//! of them takes data from that producer only:
//!
//! * `Decompression` inflates the record. The format is the one its `CompressionFormat`
//!   parameter names, and the one CloudWatch Logs frames a subscription payload in: "When
//!   log events are sent to the receiving service, they are Base64 encoded and compressed
//!   with the GZIP format" (`PutSubscriptionFilter`).
//! * `CloudWatchLogProcessing` extracts the log events out of the CloudWatch Logs envelope
//!   the inflated record is — its `DataMessageExtraction` parameter being the switch — so
//!   what is delivered is the `message` of each log event the envelope carries, one per
//!   line. An envelope carrying no log event of the account (the `CONTROL_MESSAGE` the
//!   subscribing service writes when a filter is configured) has nothing to extract and is
//!   therefore dropped rather than delivered empty.
//!
//! A record the enabled pipeline cannot process — one that is not framed in the format
//! `Decompression` inflates, or whose inflated bytes are not the envelope document
//! `CloudWatchLogProcessing` reads — is a record whose PROCESSING failed: the delivery writes
//! it under the destination's error output instead of into the delivered object
//! ([`Processed::Failed`]), so nothing of it is lost and a consumer of that prefix learns
//! why.

use std::io::Read;

use crate::ops::types::ProcessingConfiguration;

/// The `ProcessorType` that inflates a compressed record.
const DECOMPRESSION_PROCESSOR: &str = "Decompression";

/// The `ProcessorType` that reads the CloudWatch Logs envelope of an inflated record.
const CLOUDWATCH_LOG_PROCESSING_PROCESSOR: &str = "CloudWatchLogProcessing";

/// The `ProcessorParameterName` naming the format `Decompression` inflates.
const COMPRESSION_FORMAT_PARAMETER: &str = "CompressionFormat";

/// The `ProcessorParameterName` switching `CloudWatchLogProcessing`'s extraction on.
const DATA_MESSAGE_EXTRACTION_PARAMETER: &str = "DataMessageExtraction";

/// The one `CompressionFormat` a CloudWatch Logs payload arrives in, and so the one this
/// pipeline inflates: "they are Base64 encoded and compressed with the GZIP format"
/// (`PutSubscriptionFilter`).
const GZIP_FORMAT: &str = "GZIP";

/// The `messageType` of an envelope carrying ingested log events — the one with events to
/// extract.
const DATA_MESSAGE: &str = "DATA_MESSAGE";

/// The `messageType` of the envelope the subscribing service writes to validate a
/// destination. It carries no log event of the account, so message extraction has nothing to
/// take out of it.
const CONTROL_MESSAGE: &str = "CONTROL_MESSAGE";

/// The separator between the extracted messages of one record: each message is delivered as
/// its own line, so an object holding several is read a line at a time.
const MESSAGE_SEPARATOR: &[u8] = b"\n";

/// The most the records of ONE delivery may inflate to, over all of them.
///
/// An EMULATOR ceiling, and the aggregate half of [`MAX_INFLATED_BYTES`]: a per-record bound
/// alone leaves the PRODUCT of the record count and the inflated size unbounded, and the
/// buffer a delivery comes from is counted in the bytes the producer PUT, which say nothing
/// about what they inflate to. It is the budget this service already allows itself for
/// buffered record data across every stream (`crate::state::MAX_BUFFERED_BYTES`), so the
/// processed form of one flush — held beside that buffer while the objects are built — cannot
/// cost more than the buffer itself may. A record the budget leaves no room for is reported
/// as one the processing could not read, which writes it to the error output with its own
/// bytes rather than dropping it.
pub const MAX_INFLATED_BYTES_PER_DELIVERY: u64 = crate::state::MAX_BUFFERED_BYTES as u64;

/// The most one record may inflate to.
///
/// An EMULATOR ceiling, not an AWS quota: inflating is the one step here whose output a
/// record chooses the size of, and a compressed frame can name a far larger one than it
/// occupies, so the inflated bytes are bounded before they are held. It sits well above what
/// the producer this pipeline exists for can send — a CloudWatch Logs subscription payload is
/// framed so that one record stays inside the one-MiB data quota of the stream it is put on —
/// so no envelope of that producer reaches it, and a record that does is reported as one the
/// processing could not read rather than held.
const MAX_INFLATED_BYTES: u64 = 8 * 1024 * 1024;

/// Which parts of the CloudWatch Logs pipeline one destination has enabled.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CloudWatchLogsPipeline {
    /// Whether an enabled `Decompression` processor inflates each record.
    pub decompress: bool,
    /// Whether an enabled `CloudWatchLogProcessing` processor extracts each envelope's log
    /// event messages.
    pub extract_messages: bool,
}

impl CloudWatchLogsPipeline {
    /// Whether this pipeline transforms a record at all.
    #[must_use]
    pub fn runs(&self) -> bool {
        self.decompress || self.extract_messages
    }

    /// What `processing` enables of the pipeline.
    ///
    /// A configuration whose `Enabled` is not `true` processes nothing — "Enables or disables
    /// data processing" (`ProcessingConfiguration$Enabled`) — so the processors it lists leave
    /// every record exactly as it arrived.
    #[must_use]
    pub fn of(processing: Option<&ProcessingConfiguration>) -> Self {
        let Some(processing) = processing.filter(|processing| processing.enabled == Some(true))
        else {
            return Self::default();
        };
        let mut pipeline = Self::default();
        for processor in processing.processors.iter().flatten() {
            if processor.r#type == DECOMPRESSION_PROCESSOR {
                // The parameter names the format to inflate; a processor naming none inflates
                // the one format this pipeline exists for.
                pipeline.decompress |= parameter(processor, COMPRESSION_FORMAT_PARAMETER)
                    .is_none_or(|format| format.eq_ignore_ascii_case(GZIP_FORMAT));
            }
            if processor.r#type == CLOUDWATCH_LOG_PROCESSING_PROCESSOR {
                // Likewise the switch: this processor exists to extract the envelope's
                // messages, so only a parameter that says not to leaves them in place.
                pipeline.extract_messages |=
                    parameter(processor, DATA_MESSAGE_EXTRACTION_PARAMETER)
                        .is_none_or(|value| !value.eq_ignore_ascii_case("false"));
            }
        }
        pipeline
    }
}

/// The value of the parameter `name` of `processor`, or `None` when it carries none.
fn parameter<'p>(
    processor: &'p crate::ops::types::Processor,
    name: &str,
) -> Option<&'p str> {
    processor
        .parameters
        .iter()
        .flatten()
        .find(|parameter| parameter.parameter_name == name)
        .map(|parameter| parameter.parameter_value.as_str())
}

/// What the pipeline leaves of one record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Processed {
    /// The bytes the delivered object carries for this record.
    Record(Vec<u8>),
    /// The record held nothing to deliver, so the object carries nothing for it.
    Dropped,
    /// The pipeline could not process the record: it goes to the destination's error output.
    Failed,
}

/// Run `pipeline` over one record's bytes, charging what it inflates against `budget` — the
/// inflated bytes the rest of this delivery may still hold ([`MAX_INFLATED_BYTES_PER_DELIVERY`]).
///
/// A record that would inflate past what is left of the budget is [`Processed::Failed`] like
/// any other the processing cannot deliver, so the records of one flush cannot together hold
/// more than that budget and none of them is dropped for it.
#[must_use]
pub fn process(data: &[u8], pipeline: CloudWatchLogsPipeline, budget: &mut u64) -> Processed {
    if !pipeline.runs() {
        // Nothing is inflated, so this record costs no more than the bytes the buffer was
        // already holding and charges nothing against the budget.
        return Processed::Record(data.to_vec());
    }
    let inflated = if pipeline.decompress {
        // One record may hold at most its own ceiling, and no more than the delivery has
        // left: the narrower of the two bounds this inflation.
        match inflated(data, MAX_INFLATED_BYTES.min(*budget)) {
            Some(inflated) => {
                *budget = budget.saturating_sub(u64::try_from(inflated.len()).unwrap_or(u64::MAX));
                inflated
            }
            None => return Processed::Failed,
        }
    } else {
        data.to_vec()
    };
    if !pipeline.extract_messages {
        return Processed::Record(inflated);
    }
    extracted(&inflated)
}

/// `data` inflated out of its gzip frame, or `None` when it is not one — or when it inflates
/// past `ceiling`, which is read the same way: a record this destination's processing cannot
/// deliver.
///
/// A record CloudWatch Logs delivered is one frame; `MultiGzDecoder` reads the concatenated
/// members a producer that frames its payload in several leaves behind, which is the same
/// reading of a single-member frame. The reader is bounded to one byte past the ceiling, so
/// the overrun is seen without ever holding more than that.
fn inflated(data: &[u8], ceiling: u64) -> Option<Vec<u8>> {
    let mut inflated = Vec::new();
    ::flate2::read::MultiGzDecoder::new(data)
        .take(ceiling.saturating_add(1))
        .read_to_end(&mut inflated)
        .ok()?;
    (u64::try_from(inflated.len()).is_ok_and(|held| held <= ceiling)).then_some(inflated)
}

/// The messages of the CloudWatch Logs envelope `inflated` is, one per line.
///
/// `Dropped` for an envelope that carries no log event of the account ([`CONTROL_MESSAGE`]),
/// and `Failed` for bytes that are not an envelope document at all — which is what a record
/// extraction was asked to run on and could not read is.
fn extracted(inflated: &[u8]) -> Processed {
    let Ok(envelope) = ::serde_json::from_slice::<::serde_json::Value>(inflated) else {
        return Processed::Failed;
    };
    match envelope.get("messageType").and_then(|kind| kind.as_str()) {
        Some(DATA_MESSAGE) => {}
        Some(CONTROL_MESSAGE) => return Processed::Dropped,
        // Neither kind of envelope: there is no documented message to take out of it.
        _ => return Processed::Failed,
    }
    let Some(events) = envelope.get("logEvents").and_then(|events| events.as_array()) else {
        return Processed::Failed;
    };
    let mut extracted = Vec::new();
    for event in events {
        let Some(message) = event.get("message").and_then(|message| message.as_str()) else {
            return Processed::Failed;
        };
        extracted.extend_from_slice(message.as_bytes());
        extracted.extend_from_slice(MESSAGE_SEPARATOR);
    }
    // An envelope whose event list is empty carries no message either, so it is dropped for
    // the same reason a control message is.
    if extracted.is_empty() {
        return Processed::Dropped;
    }
    Processed::Record(extracted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// `body` in one gzip frame, the framing a CloudWatch Logs payload arrives in.
    fn gzip(body: &str) -> Vec<u8> {
        let mut encoder =
            ::flate2::write::GzEncoder::new(Vec::new(), ::flate2::Compression::default());
        encoder.write_all(body.as_bytes()).expect("a Vec takes writes");
        encoder.finish().expect("the frame finishes over a Vec")
    }

    /// One envelope document of `message_type` carrying `messages`.
    fn envelope(message_type: &str, messages: &[&str]) -> String {
        ::serde_json::json!({
            "messageType": message_type,
            "owner": "000000000000",
            "logGroup": "/aws/emulator/group",
            "logStream": "stream",
            "subscriptionFilters": ["filter"],
            "logEvents": messages
                .iter()
                .map(|message| ::serde_json::json!({
                    "id": "0",
                    "timestamp": 1_700_000_000_000_i64,
                    "message": message,
                }))
                .collect::<Vec<::serde_json::Value>>(),
        })
        .to_string()
    }

    /// The pipeline leaves of a record what the processors it enables ask for, and reports a
    /// record it cannot process as one whose processing failed.
    #[test]
    fn each_enabled_processor_leaves_what_it_asks_for() {
        let data = envelope(DATA_MESSAGE, &["first", "second"]);
        let control = envelope(CONTROL_MESSAGE, &["CWL CONTROL MESSAGE: Checking health."]);
        let inflate = CloudWatchLogsPipeline {
            decompress: true,
            extract_messages: false,
        };
        let whole = CloudWatchLogsPipeline {
            decompress: true,
            extract_messages: true,
        };
        for (case, bytes, pipeline, expected) in [
            (
                "no processor at all leaves the frame",
                gzip(&data),
                CloudWatchLogsPipeline::default(),
                Processed::Record(gzip(&data)),
            ),
            (
                "decompression alone inflates the frame",
                gzip(&data),
                inflate,
                Processed::Record(data.clone().into_bytes()),
            ),
            (
                "the whole pipeline extracts each message",
                gzip(&data),
                whole,
                Processed::Record(b"first\nsecond\n".to_vec()),
            ),
            (
                "a control message carries nothing to extract",
                gzip(&control),
                whole,
                Processed::Dropped,
            ),
            (
                "a data message carrying no event",
                gzip(&envelope(DATA_MESSAGE, &[])),
                whole,
                Processed::Dropped,
            ),
            (
                "a record that is not framed at all",
                data.clone().into_bytes(),
                whole,
                Processed::Failed,
            ),
            (
                "a frame that is not an envelope document",
                gzip("not a document"),
                whole,
                Processed::Failed,
            ),
            (
                "a frame whose document names no message type",
                gzip("{\"logEvents\":[]}"),
                whole,
                Processed::Failed,
            ),
        ] {
            let mut budget = MAX_INFLATED_BYTES_PER_DELIVERY;
            let processed = process(&bytes, pipeline, &mut budget);
            assert_eq!(
                processed, expected,
                "case {case:?}: expected {expected:?}, got {processed:?}"
            );
        }
    }

    /// The budget bounds the records of one delivery TOGETHER: once what is left of it leaves
    /// no room for the next record, that record is reported as one the processing could not
    /// read rather than inflated anyway.
    #[test]
    fn the_delivery_budget_bounds_the_records_together() {
        let body = "a".repeat(4_096);
        let frame = gzip(&body);
        let inflate = CloudWatchLogsPipeline {
            decompress: true,
            extract_messages: false,
        };
        // Room for three records of this size and not a byte more.
        let mut budget = 3 * 4_096;
        for (case, expected) in [
            ("the first record of the delivery", Processed::Record(body.clone().into_bytes())),
            ("the second", Processed::Record(body.clone().into_bytes())),
            ("the third, which exhausts the budget", Processed::Record(body.clone().into_bytes())),
            ("the fourth, which the budget leaves no room for", Processed::Failed),
            ("and every record after it", Processed::Failed),
        ] {
            let processed = process(&frame, inflate, &mut budget);
            assert_eq!(
                processed, expected,
                "case {case:?}: expected {expected:?}, got {processed:?} with {budget} bytes left"
            );
        }
    }

    /// A record that inflates past the ceiling is reported as one the processing could not
    /// read, and nothing past the ceiling is held while deciding that.
    #[test]
    fn a_frame_inflating_past_the_ceiling_is_a_processing_failure() {
        let ceiling = usize::try_from(MAX_INFLATED_BYTES).expect("the ceiling fits in a usize");
        let whole = CloudWatchLogsPipeline {
            decompress: true,
            extract_messages: false,
        };
        for (case, body) in [
            ("a frame inflating to just under the ceiling", ceiling - 1),
            ("a frame inflating to the ceiling itself", ceiling),
            ("a frame inflating past the ceiling", ceiling + 1),
        ] {
            let mut encoder =
                ::flate2::write::GzEncoder::new(Vec::new(), ::flate2::Compression::default());
            encoder
                .write_all(&vec![b'a'; body])
                .expect("a Vec takes writes");
            let frame = encoder.finish().expect("the frame finishes over a Vec");
            let mut budget = MAX_INFLATED_BYTES_PER_DELIVERY;
            let processed = process(&frame, whole, &mut budget);
            let expected = if body > ceiling {
                Processed::Failed
            } else {
                Processed::Record(vec![b'a'; body])
            };
            assert!(
                processed == expected,
                "case {case:?}: expected {} bytes to inflate to {}, got {}",
                frame.len(),
                match &expected {
                    Processed::Record(record) => format!("{} bytes", record.len()),
                    other => format!("{other:?}"),
                },
                match &processed {
                    Processed::Record(record) => format!("{} bytes", record.len()),
                    other => format!("{other:?}"),
                }
            );
        }
    }

    /// What a `ProcessingConfiguration` enables of the pipeline, over the forms one is
    /// written in.
    #[test]
    fn the_configuration_decides_which_part_of_the_pipeline_runs() {
        let processor = |kind: &str, parameters: &[(&str, &str)]| crate::ops::types::Processor {
            r#type: kind.to_string(),
            parameters: Some(
                parameters
                    .iter()
                    .map(|(name, value)| crate::ops::types::ProcessorParameter {
                        parameter_name: (*name).to_string(),
                        parameter_value: (*value).to_string(),
                    })
                    .collect(),
            ),
        };
        let configuration = |enabled: Option<bool>, processors: Vec<crate::ops::types::Processor>| {
            ProcessingConfiguration {
                enabled,
                processors: Some(processors),
            }
        };
        let decompression = processor(
            DECOMPRESSION_PROCESSOR,
            &[(COMPRESSION_FORMAT_PARAMETER, GZIP_FORMAT)],
        );
        let extraction = processor(
            CLOUDWATCH_LOG_PROCESSING_PROCESSOR,
            &[(DATA_MESSAGE_EXTRACTION_PARAMETER, "true")],
        );
        let unrelated = processor("AppendDelimiterToRecord", &[("Delimiter", "\\n")]);
        for (case, processing, expected) in [
            ("no configuration at all", None, CloudWatchLogsPipeline::default()),
            (
                "a configuration that is not enabled",
                Some(configuration(
                    Some(false),
                    vec![decompression.clone(), extraction.clone()],
                )),
                CloudWatchLogsPipeline::default(),
            ),
            (
                "a configuration whose Enabled is absent",
                Some(configuration(None, vec![decompression.clone()])),
                CloudWatchLogsPipeline::default(),
            ),
            (
                "only a processor of another kind",
                Some(configuration(Some(true), vec![unrelated.clone()])),
                CloudWatchLogsPipeline::default(),
            ),
            (
                "decompression alone",
                Some(configuration(Some(true), vec![decompression.clone()])),
                CloudWatchLogsPipeline {
                    decompress: true,
                    extract_messages: false,
                },
            ),
            (
                "the whole pipeline, beside an unrelated processor",
                Some(configuration(
                    Some(true),
                    vec![unrelated, decompression.clone(), extraction],
                )),
                CloudWatchLogsPipeline {
                    decompress: true,
                    extract_messages: true,
                },
            ),
            (
                "extraction with its switch turned off",
                Some(configuration(
                    Some(true),
                    vec![
                        decompression,
                        processor(
                            CLOUDWATCH_LOG_PROCESSING_PROCESSOR,
                            &[(DATA_MESSAGE_EXTRACTION_PARAMETER, "false")],
                        ),
                    ],
                )),
                CloudWatchLogsPipeline {
                    decompress: true,
                    extract_messages: false,
                },
            ),
            (
                "decompression naming a format this pipeline does not inflate",
                Some(configuration(
                    Some(true),
                    vec![processor(
                        DECOMPRESSION_PROCESSOR,
                        &[(COMPRESSION_FORMAT_PARAMETER, "Snappy")],
                    )],
                )),
                CloudWatchLogsPipeline::default(),
            ),
        ] {
            let read = CloudWatchLogsPipeline::of(processing.as_ref());
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }
}
