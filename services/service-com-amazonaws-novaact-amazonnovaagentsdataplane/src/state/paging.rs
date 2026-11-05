// SPDX-License-Identifier: CC0-1.0

//! Ordering, paging and resumption tokens for this service's listings.
//!
//! Every `List*` operation of this model pages the same way — `maxResults` bounds the
//! page, `sortOrder` orders it by the one instant its summary carries, and `nextToken`
//! resumes it strictly after the record the previous page ended on — so that rule lives
//! here once and each handler supplies only what differs: which records it is listing,
//! the instant and the identifier that place each one ([`Position`]), and which modelled
//! error channel a refusal travels on.
//!
//! A token carries a POSITION, never an offset, so a listing resumed after a record that
//! has since been removed still answers the records that sort after it, and one resumed
//! after a record created meanwhile still answers every record exactly once.

use std::cmp::Ordering;
use std::time::{Duration, SystemTime};

use crate::ops::types::{ValidationException, ValidationExceptionField, SORT_ORDER_VALUES};

/// The largest page a listing serves, and the page size a request naming no `maxResults`
/// is answered with: `MaxResults`' modelled maximum
/// (`smithy_query(query="shape",shape="MaxResults")` reports `range 1..100`).
pub const MAX_PAGE_SIZE: i32 = 100;

/// The byte that separates the fields inside a `nextToken`. It is outside the patterns of
/// every identifier this service names a record by (`^[a-zA-Z0-9_-]{1,40}$` for a workflow
/// definition, lowercase hex and hyphens for a `UuidString`) and outside the digits of an
/// instant, so it cannot occur inside a field it separates.
const TOKEN_SEPARATOR: char = '\u{1f}';

/// The longest name `WorkflowDefinitionName` admits (`^[a-zA-Z0-9_-]{1,40}$`, length
/// 1..40), so a forged token cannot carry a name no definition could ever have.
const MAX_DEFINITION_NAME_LENGTH: usize = 40;

/// The group lengths of a `UuidString`
/// (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`).
const UUID_GROUP_LENGTHS: [usize; 5] = [8, 4, 4, 4, 12];

/// The page size a request asks for: the `maxResults` it names, else the modelled maximum.
///
/// `@range(1..100)` is refused at the boundary, so the clamp only keeps a page-size
/// calculation total; it is never what enforces the published range.
pub fn page_size(max_results: Option<i32>) -> usize {
    max_results.unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize
}

/// Where one record sorts in a listing: the instant its own summary reports, and the
/// identifier that breaks a tie between two records created in the same instant.
///
/// The tie-break is an identifier the summary itself carries, so a resumption token can
/// name the position it stopped at in terms the caller's next request resolves against
/// the store, which an internal counter could not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Position {
    /// The instant the record was created, as its summary reports it.
    pub created_at: SystemTime,
    /// The record's own identifier.
    pub key: String,
}

impl Position {
    /// The position of a record created at `created_at` and identified by `key`.
    pub fn new(created_at: SystemTime, key: impl Into<String>) -> Self {
        Self {
            created_at,
            key: key.into(),
        }
    }

    /// The pair the ordering compares: instant first, identifier as the tie-break.
    fn sort_key(&self) -> (SystemTime, &str) {
        (self.created_at, self.key.as_str())
    }
}

/// `SortOrder`: the order a listing reports its records in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageOrder {
    /// Oldest first. The order a request that names no `sortOrder` is answered in, so
    /// paging without naming one is still a total order.
    Ascending,
    /// Newest first.
    Descending,
}

impl PageOrder {
    /// The order the request asked for, or a refusal naming `sortOrder`.
    ///
    /// The model constrains the member to its enum, so the boundary walk refuses an
    /// unknown value before a handler runs; reading it here means a value a listing can
    /// order by, and anything else is refused rather than silently answered in some other
    /// order.
    pub fn from_input(sort_order: Option<&str>) -> Result<Self, ValidationException> {
        match sort_order {
            None => Ok(Self::Ascending),
            Some(value) if value == Self::Ascending.as_wire() => Ok(Self::Ascending),
            Some(value) if value == Self::Descending.as_wire() => Ok(Self::Descending),
            Some(other) => Err(field_validation_failed(
                "sortOrder",
                format!(
                    "sortOrder must be one of {}, not {other}",
                    SORT_ORDER_VALUES.join(" | ")
                ),
            )),
        }
    }

    /// The value as the model spells it on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Ascending => "Ascending",
            Self::Descending => "Descending",
        }
    }

    /// Where `here` sorts relative to `there` in this order.
    fn compare(self, here: &Position, there: &Position) -> Ordering {
        match self {
            Self::Ascending => here.sort_key().cmp(&there.sort_key()),
            Self::Descending => there.sort_key().cmp(&here.sort_key()),
        }
    }

    /// Whether `candidate` sorts strictly after `cursor` in this order, i.e. whether it
    /// belongs on the page that resumes from it.
    fn is_after(self, cursor: &Position, candidate: &Position) -> bool {
        self.compare(cursor, candidate) == Ordering::Less
    }
}

/// One page of `items`, and the `nextToken` that resumes the listing after it.
///
/// `items` is every record the request selected, in any order; this orders them, skips the
/// ones at or before `cursor`, and keeps at most `page_size`. A token is issued only when
/// a record is left over, so the last page of a listing ends it.
pub fn page<T>(
    items: Vec<T>,
    order: PageOrder,
    page_size: usize,
    cursor: Option<&Position>,
    position: impl Fn(&T) -> Position,
) -> (Vec<T>, Option<String>) {
    let mut ordered: Vec<(Position, T)> = items
        .into_iter()
        .map(|item| (position(&item), item))
        .collect();
    ordered.sort_by(|(here, _), (there, _)| order.compare(here, there));

    // The list is already in the requested order, so the first record that sorts strictly
    // after the cursor is where the page resumes.
    let start = match cursor {
        None => 0,
        Some(cursor) => ordered
            .iter()
            .position(|(candidate, _)| order.is_after(cursor, candidate))
            .unwrap_or(ordered.len()),
    };
    let mut rest = ordered.split_off(start);
    let left_over = rest.len() > page_size;
    rest.truncate(page_size);
    let next_token = rest
        .last()
        .filter(|_| left_over)
        .map(|(last, _)| encode_next_token(order, last));

    (rest.into_iter().map(|(_, item)| item).collect(), next_token)
}

/// The opaque `NextToken` for the last record a page emitted.
///
/// Hex-encoded (`hex`, already a dependency of this crate) so the token is opaque to the
/// caller and satisfies `NextToken`'s `^\S*$` pattern. The order is carried with the
/// position because the position means nothing without it.
fn encode_next_token(order: PageOrder, position: &Position) -> String {
    let since_epoch = position
        .created_at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::hex::encode(format!(
        "{}{TOKEN_SEPARATOR}{}.{:09}{TOKEN_SEPARATOR}{}",
        order.as_wire(),
        since_epoch.as_secs(),
        since_epoch.subsec_nanos(),
        position.key,
    ))
}

/// The [`Position`] inside a `nextToken`, or the refusal for a token `operation` did not
/// issue for this listing.
///
/// `key_is_valid` is the identifier rule of the records being listed, so a token cannot
/// carry a key no record of that kind could ever have.
///
/// Every step is fallible on purpose: the token is caller-supplied, so nothing read out of
/// it may panic the handler. The instant in particular is rebuilt with `checked_add`, since
/// `SystemTime + Duration` panics on overflow and a forged token can name any `u64`.
pub fn decode_next_token(
    token: &str,
    order: PageOrder,
    operation: &str,
    key_is_valid: impl Fn(&str) -> bool,
) -> Result<Position, ValidationException> {
    let invalid = || {
        field_validation_failed(
            "nextToken",
            format!(
                "nextToken is not a token {operation} issued for a {} listing",
                order.as_wire()
            ),
        )
    };
    let decoded = ::hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(invalid)?;
    let mut fields = decoded.split(TOKEN_SEPARATOR);
    let (token_order, created_at, key) =
        match (fields.next(), fields.next(), fields.next(), fields.next()) {
            (Some(token_order), Some(created_at), Some(key), None) => {
                (token_order, created_at, key)
            }
            _ => return Err(invalid()),
        };
    // A token carries the order it was issued for: resuming a `Descending` listing from an
    // `Ascending` token would silently answer a page from neither order.
    if token_order != order.as_wire() || !key_is_valid(key) {
        return Err(invalid());
    }
    let (secs, nanos) = created_at.split_once('.').ok_or_else(invalid)?;
    let secs: u64 = secs.parse().map_err(|_| invalid())?;
    let nanos: u32 = nanos.parse().map_err(|_| invalid())?;
    if nanos >= 1_000_000_000 {
        return Err(invalid());
    }
    let created_at = SystemTime::UNIX_EPOCH
        .checked_add(Duration::new(secs, nanos))
        .ok_or_else(invalid)?;
    Ok(Position::new(created_at, key))
}

/// The modelled refusal for a request member a listing cannot use: HTTP 400
/// `ValidationException` with `reason` `FieldValidationFailed` — one of the three values
/// `ValidationExceptionReason` declares — and one `fieldList` entry naming the member, so
/// a caller learns WHICH member to fix.
pub fn field_validation_failed(member: &str, message: String) -> ValidationException {
    ValidationException {
        message: message.clone(),
        reason: "FieldValidationFailed".to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message,
        }]),
    }
}

/// Whether `value` is a name `WorkflowDefinitionName` admits
/// (`^[a-zA-Z0-9_-]{1,40}$`), the key of a workflow-definition listing.
pub fn is_workflow_definition_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_DEFINITION_NAME_LENGTH
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        })
}

/// Whether `value` is a `UuidString`
/// (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`), the key of every
/// listing whose records are identified by a minted id.
pub fn is_uuid_string(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == UUID_GROUP_LENGTHS.len()
        && groups.iter().zip(UUID_GROUP_LENGTHS).all(|(group, length)| {
            group.len() == length
                && group
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}
