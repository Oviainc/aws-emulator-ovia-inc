// SPDX-License-Identifier: CC0-1.0

//! The lookup-miss refusals: the `ResourceNotFoundException` messages this service
//! words for a table, an index or a tagged resource that is not there.

use super::*;

/// The `ResourceNotFoundException` message the CONTROL plane answers with: the
/// table-level operations (`DescribeTable`, `UpdateTable`, the tagging surface)
/// name the resource they could not find, because the caller addressed that
/// table by name and the describing surface reports it back.
pub(super) fn not_found(name: &str) -> RequestError {
    RequestError::NotFound(format!("Requested resource not found: Table: {name} not found"))
}

/// The `ResourceNotFoundException` message for an INDEX the addressed table does not
/// carry.
///
/// "The operation tried to access a nonexistent table or index" (the
/// `ResourceNotFoundException` doc): the caller addressed this one by a table and an
/// index name, so the refusal names both, exactly as the name-addressed [`not_found`]
/// above names the table it could not find.
pub fn index_not_found(table: &str, index: &str) -> RequestError {
    RequestError::NotFound(format!(
        "Requested resource not found: Index not found for table {table}: {index}"
    ))
}

/// The `ResourceNotFoundException` message a VECTOR index the addressed table does not
/// carry is refused with.
///
/// Same rule as [`index_not_found`] — "the operation tried to access a nonexistent table
/// or index" (the `ResourceNotFoundException` doc), named by both the index and the table
/// it was looked for in — and a different SENTENCE, because the surface that changes a
/// table's vector indexes words it with the index name first and the table after it. One
/// sentence per surface rather than one shared one: a message is part of each surface's
/// contract, so the surfaces that answer the other wording keep it.
pub fn vector_index_not_found(table: &str, index: &str) -> RequestError {
    RequestError::NotFound(format!(
        "Requested resource not found: Index {index} for table {table}"
    ))
}

/// The `ResourceNotFoundException` message an ARN-ADDRESSED operation answers with.
///
/// The tagging surface and the resource-policy surface carry no `TableName`: the
/// caller's whole selector is the `ResourceArn` member, so the refusal names THAT
/// member and echoes the value the request carried — the same construction the
/// name-addressed [`not_found`] above makes for `Table`, over the identifier that was
/// actually sent. Nothing derived from the ARN (the table name inside it) appears,
/// because no member of the request holds it.
pub fn resource_arn_not_found(resource_arn: &str) -> RequestError {
    RequestError::NotFound(format!(
        "Requested resource not found: ResourceArn: {resource_arn} not found"
    ))
}

/// The `ValidationException` message an ARN-ADDRESSED operation answers a
/// `ResourceArn` it cannot address with: the fixed prefix DynamoDB's own ARN
/// validator states, followed by the value the request carried.
///
/// This is an INPUT refusal, not a lookup miss: a value that is not an ARN this
/// service can address names no resource whose existence could be checked.
pub fn invalid_resource_arn(resource_arn: &str) -> RequestError {
    RequestError::Validation(format!(
        "Invalid TableArn: Invalid ResourceArn provided as input {resource_arn}"
    ))
}

/// The `ValidationException` message for a request whose tag list repeats a key.
///
/// Built from the request: the fixed prefix, then the key found twice. A tag key
/// identifies at most one tag on a resource ("Each DynamoDB table can only have up to
/// one tag with the same key" — the `Tag$Key` doc), so a list that names one key twice
/// asks for two tags that cannot both exist and is refused rather than silently
/// resolved to one of its values.
pub fn duplicate_tag_key(key: &str) -> RequestError {
    RequestError::Validation(format!(
        "Duplicate Tag Keys provided as input: Duplicate Tag Key found {key}"
    ))
}

/// The `ResourceNotFoundException` message the DATA plane answers with.
///
/// An item operation (`GetItem`, `PutItem`, `DeleteItem`, `Query`, …) against a
/// table that is absent — or present but not yet serving items — is refused with
/// AWS's FIXED text, with no resource detail appended: the one message is all a
/// client sees for "the table you addressed is not there", so nothing about this
/// store's contents (which name was looked up, whether it exists in another
/// status) leaks through the data-plane refusal.
///
/// A data-plane read of an INDEX that the table declares but that is not serving yet
/// answers the same sentence, for the same reason: the operation's own
/// `ResourceNotFoundException` covers both ("The operation tried to access a nonexistent
/// table or index … or its status might not be ACTIVE").
pub fn item_resource_not_found() -> RequestError {
    RequestError::NotFound("Requested resource not found".to_string())
}

/// The `ValidationException` message for a request that names an index the addressed
/// table does not declare as the SOURCE IT READS.
///
/// An index named in a statement's source — or in an `IndexName` member — is part of the
/// request's own shape rather than a resource the caller addressed: the table was found,
/// and what is wrong is that it has no such index, which AWS states with this fixed
/// sentence. A statement's source is refused with the sentence alone; a member that
/// carries the name is answered the name as well ([`unknown_index_named`]). (A caller that
/// addresses an index AS THE RESOURCE, by table and index name, is answered by
/// [`index_not_found`] instead: there the index is what it asked for.)
pub const UNKNOWN_INDEX_MESSAGE: &str = "The table does not have the specified index";

/// That message as the refusal it is.
pub fn unknown_index() -> RequestError {
    RequestError::Validation(UNKNOWN_INDEX_MESSAGE.to_string())
}

/// The same refusal for a request that names the index in an `IndexName` MEMBER, which
/// AWS answers with that sentence and the name the request sent appended after a colon.
///
/// The name echoed is the caller's own input, so the refusal still reports nothing of
/// what the table does carry — a caller that misspelled a member learns which value was
/// not resolved, which is why AWS echoes it here and not for a statement's source.
pub fn unknown_index_named(index_name: &str) -> RequestError {
    RequestError::Validation(format!("{UNKNOWN_INDEX_MESSAGE}: {index_name}"))
}
