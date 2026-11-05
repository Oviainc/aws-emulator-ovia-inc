// SPDX-License-Identifier: CC0-1.0

//! The opaque pagination tokens this service's listings mint and read, and the
//! refusal a token none of them minted earns.
//!
//! One concern of [`crate::state`], kept in its own file: every paginated
//! operation reaches these through the parent module, which re-exports them.

use super::*;

/// The opaque `nextToken` every paginated operation of this service mints.
///
/// One shape, shared by all of them so a token means the same thing wherever it
/// is read: hex (so the token is opaque to a caller and safe in a JSON body)
/// over an `envelope` naming the LISTING it belongs to, the resume `key` it
/// resumes at, the `region` the page was taken in, and the `scope` the listing
/// was taken in — the parent resource whose page it is, which makes a token
/// minted for another resource, by another listing, or by hand recognisably not
/// this listing's.
///
/// The `region` is part of what a token is bound to for the reason the scope is:
/// this service is regional ([`request_region`]), so each Region answers out of
/// its own store and the same id names an independent resource in each. A resume
/// key only means something inside the ONE sequence it was taken from, and a
/// Region's store is its own sequence, so a token carries the Region that minted
/// it and no other Region's endpoint resumes it.
///
/// The parts are separated by newlines, which the Region segment
/// ([`request_region`] admits only `[a-z0-9-]+`) and the ids, ARNs and tag keys
/// this service's listings resume at all exclude; the LAST part keeps any
/// newline it carries (see [`decode_page_token`]), so a scope part that is free
/// text — a query statement — goes last.
///
/// One cursor has exactly ONE token: the body is rendered in lower-case hex and
/// nothing else, so a differently cased rendering of the same bytes is a value
/// no caller was handed ([`decode_page_token`]).
pub fn encode_page_token(envelope: &str, region: &str, scope: &[&str], key: &str) -> String {
    let mut body = format!("{envelope}{key}\n{region}");
    for part in scope {
        body.push('\n');
        body.push_str(part);
    }
    ::hex::encode(body)
}

/// The resume key inside a `nextToken` [`encode_page_token`] minted for this
/// `envelope`, `region` and `scope`, or `None` when the token is not one of
/// them.
///
/// The key is split off first, then the Region and each scope part are compared
/// in turn, so a token whose envelope, Region, arity or scope disagrees is
/// rejected rather than resumed; the final part absorbs the remainder, so the
/// last scope part may itself contain newlines. A token carrying an EMPTY key is
/// rejected too: a listing never mints one, since a page is only resumed at a
/// row that exists.
///
/// Decoding is only the first half of reading a token: `::hex::decode` reads
/// spellings [`encode_page_token`] never writes — an upper-case or a mixed-case
/// digit beside the lower-case one `::hex::encode` emits — so a value that
/// decodes here is NOT yet a token this service handed out. The read is finished
/// by minting the token again from the cursor it understood and keeping it only
/// when that token is the one it was given, byte for byte: a token is this
/// service's own opaque value, so the only value that carries a cursor is the
/// one value a caller was handed, and every other rendering of the same cursor
/// is a token nothing minted ([`invalid_next_token_message`]).
pub fn decode_page_token(
    envelope: &str,
    region: &str,
    scope: &[&str],
    token: &str,
) -> Option<String> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    let body = decoded.strip_prefix(envelope)?;
    let mut parts = body.splitn(scope.len() + 2, '\n');
    let key = parts.next()?;
    if parts.next()? != region {
        return None;
    }
    for expected in scope {
        if parts.next()? != *expected {
            return None;
        }
    }
    if key.is_empty() {
        return None;
    }
    (encode_page_token(envelope, region, scope, key) == token).then(|| key.to_string())
}

/// The opaque `nextToken` for a page that resumes at a row's ORDINAL rather
/// than at a key of its own: [`encode_page_token`] over the ONE spelling of a
/// position this service mints.
///
/// A sequence whose rows carry no key — a query's rows, a table's rows, a
/// series' samples — is resumed at a row's position in it, and that position is
/// written here in exactly one spelling (`usize::to_string`: a plain decimal,
/// no sign and no leading zero), so [`decode_ordinal_page_token`] can tell a
/// position this service wrote from any other spelling of the same number.
pub fn encode_ordinal_page_token(
    envelope: &str,
    region: &str,
    scope: &[&str],
    ordinal: usize,
) -> String {
    encode_page_token(envelope, region, scope, &ordinal.to_string())
}

/// The row ORDINAL inside a `nextToken` [`encode_ordinal_page_token`] minted for
/// this `envelope` and `scope`, or `None` when the token is not one of them.
///
/// A token is only minted where a page stopped with rows still LEFT OVER, so a
/// minted position is at least 1 and is spelled the one way
/// [`encode_ordinal_page_token`] writes it. Every other spelling of a number is
/// a position nothing minted — a sign, a leading zero, a value no `usize`
/// holds — and so is 0 itself; each earns the refusal a key no listing minted
/// earns ([`invalid_next_token_message`]), rather than being resumed at
/// whatever position the text happens to parse to.
///
/// The other half of the rule is [`ordinal_resumes_a_selected_row`], asked
/// where the length of the sequence the position is read against is known.
pub fn decode_ordinal_page_token(
    envelope: &str,
    region: &str,
    scope: &[&str],
    token: &str,
) -> Option<usize> {
    let key = decode_page_token(envelope, region, scope, token)?;
    let ordinal: usize = key.parse().ok()?;
    (ordinal >= 1 && key == ordinal.to_string()).then_some(ordinal)
}

/// Whether an ordinal [`decode_ordinal_page_token`] read is a position a token
/// can have been minted at in a sequence of `selected` rows.
///
/// A page mints a token only where rows are still left over, so a minted
/// position is the position of a row the sequence really HOLDS. A position at
/// or past its end is one nothing minted: it earns the same refusal
/// ([`invalid_next_token_message`]) as a token of another listing, rather than
/// an HTTP 200 page of no rows and no further token — which would report the
/// end of a walk this service never started.
pub fn ordinal_resumes_a_selected_row(resume_at: usize, selected: usize) -> bool {
    resume_at < selected
}

/// What a `nextToken` binds the `filters` of a LISTING to: the conjunction
/// those filters stand for, as a value two requests share exactly when they
/// select the same rows.
///
/// A resume key — a row's own key or its ordinal — only means something inside
/// the ONE sequence it was taken from, so a listing that resumes at one binds
/// its token to every request member that decides that sequence, and a filter
/// decides it, since a row no filter selects is not in it. Resuming a filtered
/// page under other filters would otherwise walk past every row the new filters
/// select that sorts before the key, so a request whose filters are not the ones
/// the token was minted under is refused ([`invalid_next_token_message`]) rather
/// than answered a page with rows missing.
///
/// A `terms` entry is one filter as the member it names and the value that
/// member compares against. Filters are applied as a CONJUNCTION, which neither
/// the order they were listed in nor a repeated term changes, so the terms are
/// taken sorted and de-duplicated; JSON is what renders them, so a free-text
/// value delimits itself rather than depending on a separator the value could
/// itself carry, and a value's TYPE is part of the rendering, so two filters one
/// text spelling cannot tell apart are still told apart.
///
/// Shared by every paginated listing that filters, so one filter list reads as
/// one scope wherever it is bound; a listing filtering on a property's VALUE
/// binds its filters by [`property_filters_token_scope`] instead, that filter
/// shape carrying an operator of its own.
pub fn filters_token_scope<'a, I>(terms: I) -> String
where
    I: IntoIterator<Item = (&'a str, ::serde_json::Value)>,
{
    let mut rendered: Vec<String> = terms
        .into_iter()
        .map(|(member, value)| ::serde_json::json!([member, value]).to_string())
        .collect();
    rendered.sort();
    rendered.dedup();
    ::serde_json::Value::from(rendered).to_string()
}

/// What a `nextToken` that resumes at an ORDINAL binds a property FILTER list
/// to: the conjunction those filters stand for, as a value two requests share
/// exactly when they keep the same rows out of the same sequence.
///
/// An ordinal only means something inside the ONE sequence it was counted in, so
/// a listing that resumes at one binds its token to every request member that
/// decides that sequence — and a filter decides it, since a value no filter
/// selects is not in it. Filters are applied as a CONJUNCTION, which neither the
/// order they were listed in nor a repeated term changes, so they are taken
/// sorted and de-duplicated, the way a selection is; a filter naming no operator
/// is taken under the one it is applied with
/// ([`COMPARISON_OPERATOR_EQUALS`]). Shared by every paginated operation that
/// filters on a property's value, so one filter list reads as one scope wherever
/// it is bound.
pub fn property_filters_token_scope(
    filters: &[crate::ops::types::PropertyFilter],
) -> ::serde_json::Value {
    let mut rendered: Vec<String> = filters
        .iter()
        .map(|filter| {
            ::serde_json::json!([
                filter.property_name.as_deref().unwrap_or(""),
                filter
                    .operator
                    .as_deref()
                    .unwrap_or(COMPARISON_OPERATOR_EQUALS),
                filter
                    .value
                    .as_ref()
                    .map_or(::serde_json::Value::Null, |value| {
                        data_value_scope(value, 0)
                    }),
            ])
            .to_string()
        })
        .collect();
    rendered.sort();
    rendered.dedup();
    ::serde_json::Value::from(rendered)
}

/// How deeply a `DataValue`'s list and map nesting is rendered into a token
/// scope before the rendering stops. A `DataValue` nests through
/// `listValue`/`mapValue` and a filter value's depth is what the REQUEST says it
/// is, so the number of stack frames must not be something a caller gets to
/// choose. Past the bound the nested value renders as nothing.
const MAX_TOKEN_VALUE_DEPTH: usize = 32;

/// A `DataValue` as the value a token scope binds to: the members it carries
/// under the names the model gives them, so two values read the same here
/// exactly when they are the same value.
///
/// A map's entries are taken in KEY order — a map is unordered, so the order a
/// request wrote them in says nothing about the value — and a number is rendered
/// as text, so two numbers one JSON double cannot tell apart are still told
/// apart. JSON is what renders it, so a value's text delimits itself rather than
/// depending on a separator a free-text string value could itself carry.
///
/// `depth` is how far inside a nested list or map this value sits.
fn data_value_scope(value: &crate::ops::types::DataValue, depth: usize) -> ::serde_json::Value {
    use ::serde_json::Value as Json;

    if depth >= MAX_TOKEN_VALUE_DEPTH {
        return Json::Null;
    }
    let mut members = ::serde_json::Map::new();
    if let Some(found) = value.boolean_value {
        members.insert("booleanValue".to_string(), Json::Bool(found));
    }
    if let Some(found) = value.double_value {
        members.insert("doubleValue".to_string(), Json::String(format!("{found:?}")));
    }
    if let Some(found) = value.integer_value {
        members.insert("integerValue".to_string(), Json::String(found.to_string()));
    }
    if let Some(found) = value.long_value {
        members.insert("longValue".to_string(), Json::String(found.to_string()));
    }
    if let Some(found) = &value.string_value {
        members.insert("stringValue".to_string(), Json::String(found.clone()));
    }
    if let Some(found) = &value.list_value {
        members.insert(
            "listValue".to_string(),
            Json::Array(
                found
                    .iter()
                    .map(|entry| data_value_scope(entry, depth + 1))
                    .collect(),
            ),
        );
    }
    if let Some(found) = &value.map_value {
        let entries: BTreeMap<&String, &crate::ops::types::DataValue> = found.iter().collect();
        members.insert(
            "mapValue".to_string(),
            Json::Object(
                entries
                    .into_iter()
                    .map(|(key, entry)| (key.clone(), data_value_scope(entry, depth + 1)))
                    .collect(),
            ),
        );
    }
    if let Some(found) = &value.relationship_value {
        members.insert(
            "relationshipValue".to_string(),
            ::serde_json::json!([
                found.target_entity_id.as_deref().unwrap_or(""),
                found.target_component_name.as_deref().unwrap_or(""),
            ]),
        );
    }
    if let Some(found) = &value.expression {
        members.insert("expression".to_string(), Json::String(found.clone()));
    }
    Json::Object(members)
}

/// The message for a `nextToken` [`decode_page_token`] does not recognise: the
/// ONE refusal every paginated operation of this service reports for a token it
/// did not mint, so the same failure reads the same way whichever page was
/// asked for.
///
/// The token is NOT part of it. A `nextToken` is opaque to the caller and
/// modelled up to 17880 characters of any bytes, so interpolating it would copy
/// an arbitrary request string back into a response body; the refusal names the
/// member and what is wrong with it instead.
pub fn invalid_next_token_message() -> String {
    "nextToken is not a token this listing minted".to_string()
}
