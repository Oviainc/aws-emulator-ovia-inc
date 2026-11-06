// SPDX-License-Identifier: CC0-1.0

//! The opaque pagination tokens every listing of this service pages with: one codec,
//! one scope marker, and one encode/decode pair per listing, so a token issued by one
//! page is accepted by exactly the listing and the view that issued it.

use super::*;

/// The opaque `nextToken` a `ListStages` page ends with: the creation sequence the
/// next page resumes after, inside the view that page reported.
///
/// The codec is `aws_smithy_types::base64`, the standard-alphabet encoder the wire
/// serializer already uses for every blob, and its output stays inside the model's
/// `PaginationToken` pattern `^[a-zA-Z0-9+/=_-]*$`.
///
/// `region` is the scope this listing pages within — "all stages in your account, in the
/// AWS region where the API request is processed" — so the position means nothing in
/// another region's page and is refused there ([`encode_after`]).
pub fn encode_page_token(region: &str, after_sequence: u64) -> String {
    encode_after(PAGE_TOKEN_PREFIX, &[region], NO_FILTERS, after_sequence)
}

/// The creation sequence a `nextToken` resumes after in `region`'s page, or `None` when
/// the token was not issued by [`encode_page_token`] for that same scope, or names a
/// position no page of this store ever ended at (`issued_through`, see [`decode_after`]).
pub fn decode_page_token(region: &str, issued_through: u64, token: &str) -> Option<u64> {
    decode_after(PAGE_TOKEN_PREFIX, &[region], NO_FILTERS, issued_through, token)
}

/// The opaque `nextToken` a `ListParticipants` page ends with: the join sequence of
/// the last participant it reported, which the next page resumes after.
///
/// Encoded exactly as [`encode_page_token`] is, under its own prefix
/// ([`PARTICIPANT_PAGE_TOKEN_PREFIX`]) and over the scope this listing pages within —
/// the region, the stage and the session whose participants the page held, and the
/// filter members the request carried, which narrow those participants further — so a
/// token one listing, one session or one filter issued names no position in another.
pub fn encode_participant_page_token(
    region: &str,
    stage_arn: &str,
    session_id: &str,
    filters: &[FilterTerm<'_>],
    after_sequence: u64,
) -> String {
    encode_after(
        PARTICIPANT_PAGE_TOKEN_PREFIX,
        &[region, stage_arn, session_id],
        filters,
        after_sequence,
    )
}

/// The join sequence a `ListParticipants` `nextToken` resumes after among the
/// participants of this stage session, or `None` when the token was not issued by
/// [`encode_participant_page_token`] for that same scope — a token of another listing, of
/// another session or of another filter selection included.
pub fn decode_participant_page_token(
    region: &str,
    stage_arn: &str,
    session_id: &str,
    filters: &[FilterTerm<'_>],
    issued_through: u64,
    token: &str,
) -> Option<u64> {
    decode_after(
        PARTICIPANT_PAGE_TOKEN_PREFIX,
        &[region, stage_arn, session_id],
        filters,
        issued_through,
        token,
    )
}

/// [`encode_page_token`] for a `ListParticipantEvents` page: the occurrence position of
/// the last event it reported, which the next page resumes after, under that listing's
/// own marker and over the scope it pages within — the region, the stage, the session and
/// the participant whose events the page held.
pub fn encode_participant_event_page_token(
    region: &str,
    stage_arn: &str,
    session_id: &str,
    participant_id: &str,
    after_sequence: u64,
) -> String {
    encode_after(
        PARTICIPANT_EVENT_PAGE_TOKEN_PREFIX,
        &[region, stage_arn, session_id, participant_id],
        NO_FILTERS,
        after_sequence,
    )
}

/// The occurrence position a `ListParticipantEvents` `nextToken` resumes after among this
/// participant's events, or `None` when the token was not issued by
/// [`encode_participant_event_page_token`] for that same scope — a token of another
/// listing, another session or another participant included.
pub fn decode_participant_event_page_token(
    region: &str,
    stage_arn: &str,
    session_id: &str,
    participant_id: &str,
    issued_through: u64,
    token: &str,
) -> Option<u64> {
    decode_after(
        PARTICIPANT_EVENT_PAGE_TOKEN_PREFIX,
        &[region, stage_arn, session_id, participant_id],
        NO_FILTERS,
        issued_through,
        token,
    )
}

/// [`encode_page_token`] for a `ListStageSessions` page: the start sequence the next
/// page of stage sessions resumes after, under that listing's own marker and over the
/// scope it pages within — the region and the stage whose sessions the page held.
pub fn encode_stage_session_page_token(
    region: &str,
    stage_arn: &str,
    after_sequence: u64,
) -> String {
    encode_after(
        STAGE_SESSION_PAGE_TOKEN_PREFIX,
        &[region, stage_arn],
        NO_FILTERS,
        after_sequence,
    )
}

/// The start sequence a stage-session `nextToken` resumes after among this stage's
/// sessions, or `None` when the token was not issued by
/// [`encode_stage_session_page_token`] for that same scope — a token of another listing or
/// of another stage included.
pub fn decode_stage_session_page_token(
    region: &str,
    stage_arn: &str,
    issued_through: u64,
    token: &str,
) -> Option<u64> {
    decode_after(
        STAGE_SESSION_PAGE_TOKEN_PREFIX,
        &[region, stage_arn],
        NO_FILTERS,
        issued_through,
        token,
    )
}

/// [`encode_page_token`] for a `ListParticipantReplicas` page: the start sequence the
/// next page of replicas resumes after, under that listing's own marker and over the
/// scope it pages within — the region, the source stage and the participant whose
/// replicas the page held.
pub fn encode_replica_page_token(
    region: &str,
    source_stage_arn: &str,
    participant_id: &str,
    after_sequence: u64,
) -> String {
    encode_after(
        REPLICA_PAGE_TOKEN_PREFIX,
        &[region, source_stage_arn, participant_id],
        NO_FILTERS,
        after_sequence,
    )
}

/// The start sequence a replica `nextToken` resumes after among this participant's
/// replicas, or `None` when the token was not issued by [`encode_replica_page_token`] for
/// that same scope — a token of another listing, another source stage or another
/// participant included.
pub fn decode_replica_page_token(
    region: &str,
    source_stage_arn: &str,
    participant_id: &str,
    issued_through: u64,
    token: &str,
) -> Option<u64> {
    decode_after(
        REPLICA_PAGE_TOKEN_PREFIX,
        &[region, source_stage_arn, participant_id],
        NO_FILTERS,
        issued_through,
        token,
    )
}

/// [`encode_page_token`] for a `ListStorageConfigurations` page: the creation sequence
/// the next page of storage configurations resumes after, under that listing's own
/// marker and over the region whose view the page reported.
pub fn encode_storage_configuration_page_token(region: &str, after_sequence: u64) -> String {
    encode_after(
        STORAGE_CONFIGURATION_PAGE_TOKEN_PREFIX,
        &[region],
        NO_FILTERS,
        after_sequence,
    )
}

/// The creation sequence a storage-configuration `nextToken` resumes after, or `None`
/// when the token was not issued by [`encode_storage_configuration_page_token`] for that
/// same scope — a token of another listing or of another region included.
pub fn decode_storage_configuration_page_token(
    region: &str,
    issued_through: u64,
    token: &str,
) -> Option<u64> {
    decode_after(
        STORAGE_CONFIGURATION_PAGE_TOKEN_PREFIX,
        &[region],
        NO_FILTERS,
        issued_through,
        token,
    )
}

/// [`encode_page_token`] for a `ListPublicKeys` page: the import sequence the next page
/// of public keys resumes after, under that listing's own marker and over the region
/// whose view the page reported.
pub fn encode_public_key_page_token(region: &str, after_sequence: u64) -> String {
    encode_after(
        PUBLIC_KEY_PAGE_TOKEN_PREFIX,
        &[region],
        NO_FILTERS,
        after_sequence,
    )
}

/// The import sequence a public-key `nextToken` resumes after, or `None` when the token
/// was not issued by [`encode_public_key_page_token`] for that same scope — a token of
/// another listing or of another region included.
pub fn decode_public_key_page_token(
    region: &str,
    issued_through: u64,
    token: &str,
) -> Option<u64> {
    decode_after(
        PUBLIC_KEY_PAGE_TOKEN_PREFIX,
        &[region],
        NO_FILTERS,
        issued_through,
        token,
    )
}

/// [`encode_page_token`] for a `ListEncoderConfigurations` page: the creation sequence
/// the next page of encoder configurations resumes after, under that listing's own
/// marker and over the region whose view the page reported.
pub fn encode_encoder_configuration_page_token(region: &str, after_sequence: u64) -> String {
    encode_after(
        ENCODER_CONFIGURATION_PAGE_TOKEN_PREFIX,
        &[region],
        NO_FILTERS,
        after_sequence,
    )
}

/// The creation sequence an encoder-configuration `nextToken` resumes after, or `None`
/// when the token was not issued by [`encode_encoder_configuration_page_token`] for that
/// same scope — a token of another listing or of another region included.
pub fn decode_encoder_configuration_page_token(
    region: &str,
    issued_through: u64,
    token: &str,
) -> Option<u64> {
    decode_after(
        ENCODER_CONFIGURATION_PAGE_TOKEN_PREFIX,
        &[region],
        NO_FILTERS,
        issued_through,
        token,
    )
}

/// [`encode_page_token`] for a `ListIngestConfigurations` page: the creation sequence the
/// next page of ingest configurations resumes after, under that listing's own marker and
/// over the scope the page reported — the region whose view it is, and the filter members
/// the request carried, which select which of that region's configurations the page
/// holds.
pub fn encode_ingest_configuration_page_token(
    region: &str,
    filters: &[FilterTerm<'_>],
    after_sequence: u64,
) -> String {
    encode_after(
        INGEST_CONFIGURATION_PAGE_TOKEN_PREFIX,
        &[region],
        filters,
        after_sequence,
    )
}

/// The creation sequence an ingest-configuration `nextToken` resumes after, or `None`
/// when the token was not issued by [`encode_ingest_configuration_page_token`] for that
/// same scope — a token of another listing, of another region or of another filter
/// selection included.
pub fn decode_ingest_configuration_page_token(
    region: &str,
    filters: &[FilterTerm<'_>],
    issued_through: u64,
    token: &str,
) -> Option<u64> {
    decode_after(
        INGEST_CONFIGURATION_PAGE_TOKEN_PREFIX,
        &[region],
        filters,
        issued_through,
        token,
    )
}

/// [`encode_page_token`] for a `ListCompositions` page: the start sequence the next page
/// of compositions resumes after, under that listing's own marker and over the scope the
/// page reported — the region whose view it is, and the filter members the request
/// carried, which select which of that region's compositions the page holds.
pub fn encode_composition_page_token(
    region: &str,
    filters: &[FilterTerm<'_>],
    after_sequence: u64,
) -> String {
    encode_after(
        COMPOSITION_PAGE_TOKEN_PREFIX,
        &[region],
        filters,
        after_sequence,
    )
}

/// The start sequence a composition `nextToken` resumes after, or `None` when the token
/// was not issued by [`encode_composition_page_token`] for that same scope — a token of
/// another listing, of another region or of another filter selection included.
pub fn decode_composition_page_token(
    region: &str,
    filters: &[FilterTerm<'_>],
    issued_through: u64,
    token: &str,
) -> Option<u64> {
    decode_after(
        COMPOSITION_PAGE_TOKEN_PREFIX,
        &[region],
        filters,
        issued_through,
        token,
    )
}

/// One filter member of a listing request: the member's name as the model declares it,
/// and the value this request gave it — `None` when the request carries that filter not
/// at all.
///
/// A filter member SELECTS which records a page holds, so it is a term of that page's
/// scope beside the identity terms, and every listing that has filter members hands its
/// whole set (in the order the model declares them) to the token codec below.
pub type FilterTerm<'a> = (&'a str, Option<&'a str>);

/// The filter set of a listing whose request has no filter member at all: its scope is
/// its identity terms alone.
pub(super) const NO_FILTERS: &[FilterTerm<'static>] = &[];

/// The one page-token codec, so the listings cannot encode a position two ways.
///
/// A page position is an ordinal in ONE list of records, and which records a listing
/// pages over is decided by the terms of the request that select them: the identity terms
/// `scope` names, and the `filters` the request narrows them by. The token therefore
/// carries both beside the position, and [`decode_after`] reads a position out of it only
/// for the scope the resuming request itself names — so a token offered to any other
/// scope names no position in the records that page holds, whatever store-wide sequence it
/// carries.
pub(super) fn encode_after(
    prefix: &str,
    scope: &[&str],
    filters: &[FilterTerm<'_>],
    after_sequence: u64,
) -> String {
    ::aws_smithy_types::base64::encode(format!(
        "{}{after_sequence}",
        scope_marker(prefix, scope, filters)
    ))
}

/// The position `token` names under `prefix` within the scope `scope` and `filters` name,
/// or `None` when it names none there.
///
/// A token names a position only when it is one [`encode_after`] really could have
/// issued, which is two requirements beyond the framing:
///
/// * the position is one this store has handed out — inside `issued_through`, the
///   sequences [`ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState::issued_through`]
///   counts — since the encoder only ever writes the sequence of a record the store holds;
/// * the token is BYTE-IDENTICAL to what the encoder writes for that position, so a
///   payload spelt any other way than the encoder spells it (a sign, a leading zero, any
///   other base64 rendering of the same bytes) is a token this listing never issued.
///
/// Both are the same rule as the scope marker's: a `nextToken` resumes a listing only
/// when that listing issued it, and anything else is refused rather than read as a
/// position.
pub(super) fn decode_after(
    prefix: &str,
    scope: &[&str],
    filters: &[FilterTerm<'_>],
    issued_through: u64,
    token: &str,
) -> Option<u64> {
    let decoded = String::from_utf8(::aws_smithy_types::base64::decode(token).ok()?).ok()?;
    let after_sequence: u64 = decoded
        .strip_prefix(&scope_marker(prefix, scope, filters))?
        .parse()
        .ok()?;
    ((1..=issued_through).contains(&after_sequence)
        && encode_after(prefix, scope, filters, after_sequence) == token)
        .then_some(after_sequence)
}

/// `prefix` followed by every term of `scope` and then every filter member, each written
/// as its byte length, a `/` and the term itself, so two different scopes cannot build the
/// same marker however the terms are punctuated — an ARN carries both `:` and `/`, which a
/// plain separator would let one term imitate.
///
/// A filter the request carries is written as its member name and then its value; one it
/// carries not at all is written as its member name alone, so "no filter" and "this
/// filter, with the empty value" are two scopes — the empty string is a value a filter
/// member can carry (`ListParticipants`' `filterByUserId` is `@length 0..128`).
pub(super) fn scope_marker(prefix: &str, scope: &[&str], filters: &[FilterTerm<'_>]) -> String {
    let mut marker = String::from(prefix);
    for term in scope {
        push_scope_term(&mut marker, term);
    }
    for (member, value) in filters {
        push_scope_term(&mut marker, member);
        if let Some(value) = value {
            marker.push('=');
            push_scope_term(&mut marker, value);
        }
    }
    marker
}

/// Write one scope term onto `marker`: its byte length, a `/`, the term and a `:`.
pub(super) fn push_scope_term(marker: &mut String, term: &str) {
    marker.push_str(&term.len().to_string());
    marker.push('/');
    marker.push_str(term);
    marker.push(':');
}
