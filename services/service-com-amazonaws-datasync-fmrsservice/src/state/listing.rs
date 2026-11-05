// SPDX-License-Identifier: CC0-1.0

//! Paging and filtering shared by this service's `List*` operations.
//!
//! Every one of them pages over a store this service holds in creation order and
//! narrows it with the modelled `Filter`/`Operator` pair, so the cursor codec, the
//! page-size resolution, the comparison operators and the wording of a refusal live
//! here once rather than once per operation. Each operation keeps only the filter
//! NAMES its own model declares and the attribute each one compares.

use crate::ops::types::InvalidRequestException;

/// Stamped into every cursor so a token from an older encoding is rejected rather
/// than misread.
const CURSOR_VERSION: &str = "v1";

/// The opaque `NextToken` for a page that ended after creation sequence `seq`.
///
/// Hex-encoded, so the token matches the member's modelled `^[a-zA-Z0-9=_-]+$`
/// pattern and carries no meaning a client could construct.
pub fn encode_cursor(seq: u64) -> String {
    hex::encode(format!("{CURSOR_VERSION}:{seq}"))
}

/// The creation sequence a `NextToken` resumes after, or `None` when the token is not
/// one [`encode_cursor`] produced.
pub fn decode_cursor(token: &str) -> Option<u64> {
    let decoded = hex::decode(token).ok()?;
    let decoded = String::from_utf8(decoded).ok()?;
    decoded
        .strip_prefix(CURSOR_VERSION)?
        .strip_prefix(':')?
        .parse()
        .ok()
}

/// The creation sequence a request's `NextToken` resumes after: 0 for a first page,
/// and a refusal for a token this service did not issue.
pub fn resume_after(next_token: Option<&str>) -> Result<u64, FilterRefusal> {
    match next_token.map(decode_cursor) {
        None => Ok(0),
        Some(Some(seq)) => Ok(seq),
        Some(None) => Err(FilterRefusal::unissued_token()),
    }
}

/// How many records one page carries.
///
/// `MaxResults` is modelled `0..100`; a request that declares no page size, or declares
/// one that carries nothing, gets the largest page the member admits.
pub fn page_size(max_results: Option<i32>) -> usize {
    max_results
        .and_then(|max| usize::try_from(max).ok())
        .filter(|max| *max > 0)
        .unwrap_or(super::LIST_DEFAULT_PAGE_SIZE)
}

/// A refusal a filtered or paged list earns, carrying the message the calling
/// operation reports through its own modelled client error.
#[derive(Debug, Clone)]
pub struct FilterRefusal {
    pub message: String,
}

impl FilterRefusal {
    /// The refusal a `NextToken` this service never issued earns.
    pub fn unissued_token() -> Self {
        Self {
            message: "NextToken is not a token this service issued.".to_string(),
        }
    }

    /// The refusal a filter name the asking operation does not declare earns.
    pub fn unsupported_filter_name(name: &str, operation: &str) -> Self {
        Self {
            message: format!("Filter name {name} is not supported by {operation}."),
        }
    }

    /// The refusal an operator the filtered attribute cannot be compared with earns.
    pub fn unsupported_operator(operator: &str, name: &str) -> Self {
        Self {
            message: format!("Operator {operator} cannot be used with filter name {name}."),
        }
    }

    /// The refusal a filter value that had to be an instant but is not earns.
    pub fn not_a_timestamp(name: &str, value: &str) -> Self {
        Self {
            message: format!("Filter {name} value {value} is not a timestamp."),
        }
    }

    /// This refusal as the modelled client error every list operation reports it with.
    pub fn into_invalid_request(self) -> InvalidRequestException {
        InvalidRequestException {
            message: Some(self.message),
            error_code: None,
            datasync_error_code: None,
        }
    }
}

/// `attribute` against `values` under a textual operator, or `None` when the modelled
/// `Operator` is one that only orders numbers.
///
/// The operators are the positive members of the model's `Operator` enum: `Equals`,
/// `In`, `Contains` and `BeginsWith`. A multi-valued filter matches when any value
/// matches. Callers apply `NotEquals`/`NotContains` by negating the positive form,
/// which [`negated`] resolves for them.
pub fn text_matches(operator: &str, attribute: &str, values: &[String]) -> Option<bool> {
    match operator {
        "Equals" | "In" => Some(values.iter().any(|value| value == attribute)),
        "Contains" => Some(values.iter().any(|value| attribute.contains(value.as_str()))),
        "BeginsWith" => Some(
            values
                .iter()
                .any(|value| attribute.starts_with(value.as_str())),
        ),
        _ => None,
    }
}

/// The positive form of a textual operator and whether the result is negated, so a
/// negated operator is evaluated as the negation of its positive form.
pub fn negated(operator: &str) -> (&str, bool) {
    match operator {
        "NotEquals" => ("Equals", true),
        "NotContains" => ("Contains", true),
        other => (other, false),
    }
}

/// `attribute` against `values` under a textual operator, negated operators included.
pub fn text_matches_signed(operator: &str, attribute: &str, values: &[String]) -> Option<bool> {
    let (positive, negate) = negated(operator);
    text_matches(positive, attribute, values).map(|matched| matched != negate)
}

/// `attribute` against `values` under an operator that orders numbers, or `None` when
/// the modelled `Operator` is one that only compares text.
pub fn number_matches(operator: &str, attribute: f64, values: &[f64]) -> Option<bool> {
    match operator {
        "Equals" | "In" => Some(values.contains(&attribute)),
        "NotEquals" => Some(values.iter().all(|value| *value != attribute)),
        "GreaterThan" => Some(values.iter().any(|value| attribute > *value)),
        "GreaterThanOrEqual" => Some(values.iter().any(|value| attribute >= *value)),
        "LessThan" => Some(values.iter().any(|value| attribute < *value)),
        "LessThanOrEqual" => Some(values.iter().any(|value| attribute <= *value)),
        _ => None,
    }
}

/// A filter value read as an instant: epoch seconds, or an RFC 3339 date-time.
pub fn parse_instant(value: &str) -> Option<f64> {
    if let Ok(seconds) = value.parse::<f64>() {
        return Some(seconds);
    }
    ::aws_smithy_types::DateTime::from_str(value, ::aws_smithy_types::date_time::Format::DateTime)
        .ok()
        .map(|parsed| parsed.as_secs_f64())
}

/// Every filter value read as an instant, or the refusal the first one that is not a
/// timestamp earns.
pub fn instants(name: &str, values: &[String]) -> Result<Vec<f64>, FilterRefusal> {
    let mut parsed = Vec::with_capacity(values.len());
    for value in values {
        parsed.push(
            parse_instant(value).ok_or_else(|| FilterRefusal::not_a_timestamp(name, value))?,
        );
    }
    Ok(parsed)
}

/// A creation instant against a `CreationTime` filter's values, compared as instants
/// rather than as text.
pub fn creation_time_matches(
    name: &str,
    operator: &str,
    created: ::std::time::SystemTime,
    values: &[String],
) -> Result<bool, FilterRefusal> {
    let bounds = instants(name, values)?;
    let created = super::as_timestamp(created).as_secs_f64();
    number_matches(operator, created, &bounds)
        .ok_or_else(|| FilterRefusal::unsupported_operator(operator, name))
}
