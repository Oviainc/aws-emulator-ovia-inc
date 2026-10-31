// SPDX-License-Identifier: CC0-1.0

//! The AWS-VENDED log record formats a log group's transformer reads: the published layout of
//! one vended log type, and the fields one record of it carries.
//!
//! A vended parser of [`super::transformer`] "parse[s] Amazon VPC vended logs, extract[s]
//! fields, and convert[s] them into a JSON format" (`ParseVPC`) — it reads the WHOLE log event
//! message as a record of its own log type ("This processor always processes the entire log
//! event message") and answers the fields that record carries, each already in the JSON type
//! the format gives it. This module holds those formats, apart from the pipeline that applies
//! them, because a format is a published property of the log type rather than of any
//! processor's configuration.
//!
//! One format is held here: the VPC flow log record, which `parseVPC` reads. The other vended
//! parsers (`parseCloudfront`, `parseRoute53`, `parsePostgres`, `parseWAF`) read one record
//! layout each as well, and gain a reading here when that layout is written down; until then
//! [`super::transformer`] leaves the event as it found it for them rather than inventing
//! fields nothing derived.
//!
//! Reading one record costs one pass over the message: the fields are the message's
//! space-delimited fields ([`super::filter_pattern::space_delimited_fields`]), paired with
//! the catalogue of the format, which bounds how many fields one record can produce whatever
//! the message carries.

use ::serde_json::Value;

/// The value a vended record carries for a field that "is not applicable or could not be
/// computed for a specific record": the record "displays a '-' symbol for that entry". Such a
/// field names no observation, so it is left OUT of the parsed fields rather than carrying
/// the placeholder.
const UNAVAILABLE_FIELD: &str = "-";

/// The JSON type a parsed field takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FieldType {
    /// A count, a port, a protocol number or an instant: a JSON number.
    Number,
    /// An identifier, an address or a status word: a JSON string.
    Text,
}

impl FieldType {
    /// One field's value in this type. A field typed as a number whose text is not one keeps
    /// the text the record carried: the record said something, and dropping it would report
    /// less than the record holds.
    fn value(self, text: &str) -> Value {
        match self {
            Self::Number => text
                .parse::<i64>()
                .map_or_else(|_| Value::String(text.to_string()), Value::from),
            Self::Text => Value::String(text.to_string()),
        }
    }
}

/// The fields of a VPC flow log record in the DEFAULT format, in the order the record carries
/// them, each with the JSON type the parsed field takes.
///
/// Source: the flow log record's default format — "version account-id interface-id srcaddr
/// dstaddr srcport dstport protocol packets bytes start end action log-status" — with each
/// field spelled in the camelCase `parseVPC` converts it to. That processor "doesn't support
/// custom log formats", so this is the whole catalogue: a record carrying more fields than it
/// names leaves them unparsed, and one carrying fewer parses as far as it goes.
const VPC_FLOW_LOG_FIELDS: [(&str, FieldType); 14] = [
    ("version", FieldType::Number),
    ("accountId", FieldType::Text),
    ("interfaceId", FieldType::Text),
    ("srcAddr", FieldType::Text),
    ("dstAddr", FieldType::Text),
    ("srcPort", FieldType::Number),
    ("dstPort", FieldType::Number),
    ("protocol", FieldType::Number),
    ("packets", FieldType::Number),
    ("bytes", FieldType::Number),
    ("start", FieldType::Number),
    ("end", FieldType::Number),
    ("action", FieldType::Text),
    ("logStatus", FieldType::Text),
];

/// The fields one VPC flow log record carries, in the order the format lists them: each field
/// the record supplies, under its own name and in its own JSON type, and nothing for a field
/// whose value is the "not available" placeholder.
pub fn vpc_flow_log_fields(record: &str) -> Vec<(&'static str, Value)> {
    let carried = super::filter_pattern::space_delimited_fields(record);
    VPC_FLOW_LOG_FIELDS
        .iter()
        .zip(&carried)
        .filter(|(_, value)| *value != UNAVAILABLE_FIELD)
        .map(|((name, kind), value)| (*name, kind.value(value)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::vpc_flow_log_fields;

    /// A record parses to the catalogue's fields with its numeric fields as JSON numbers and
    /// its identifiers as JSON strings, and a field the record could not compute is ABSENT
    /// rather than carrying the placeholder.
    #[test]
    fn a_flow_log_record_parses_to_typed_fields_without_its_unavailable_ones() {
        let cases: [(&str, &str, &[&str], &[&str]); 3] = [
            (
                "a full record",
                "2 123456789012 eni-1 10.0.0.1 10.0.0.2 443 49152 6 10 840 1700000000 \
                 1700000060 ACCEPT OK",
                &[
                    "version", "accountId", "interfaceId", "srcAddr", "dstAddr", "srcPort",
                    "dstPort", "protocol", "packets", "bytes", "start", "end", "action",
                    "logStatus",
                ],
                &[],
            ),
            (
                "a record that reports no data",
                "2 123456789012 eni-1 - - - - - - - 1700000000 1700000060 - NODATA",
                &["version", "accountId", "interfaceId", "start", "end", "logStatus"],
                &[
                    "srcAddr", "dstAddr", "srcPort", "dstPort", "protocol", "packets", "bytes",
                    "action",
                ],
            ),
            (
                "a record carrying fewer fields than the format names",
                "2 123456789012 eni-1",
                &["version", "accountId", "interfaceId"],
                &["srcAddr", "action", "logStatus"],
            ),
        ];
        // The fields the format types as numbers, from the catalogue's own source.
        let numeric = [
            "version", "srcPort", "dstPort", "protocol", "packets", "bytes", "start", "end",
        ];
        for (case, record, present, absent) in cases {
            let fields = vpc_flow_log_fields(record);
            let names: Vec<&str> = fields.iter().map(|(name, _)| *name).collect();
            assert_eq!(
                names, present,
                "case {case}: the record's own fields are parsed, in format order"
            );
            for missing in absent {
                assert!(
                    !names.contains(missing),
                    "case {case}: {missing} is absent for an unavailable value, got {names:?}"
                );
            }
            for (name, value) in &fields {
                let expected_number = numeric.contains(name);
                assert_eq!(
                    value.is_number(),
                    expected_number,
                    "case {case}: {name} is a JSON number: {expected_number}, got {value}"
                );
            }
        }
    }
}
