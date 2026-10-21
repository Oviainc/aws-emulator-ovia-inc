// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! `application/x-www-form-urlencoded` request deserializer for the AWS `query`
//! protocols (`aws.protocols#awsQuery`, `aws.protocols#ec2Query`).
//!
//! Unlike awsJson/restJson1 (which reuse a JSON codec) and restXml (which reuses
//! the XML codec), the query protocols encode the request as a flat set of
//! form-urlencoded key/value pairs whose keys carry the shape structure via
//! dotted paths, e.g. for `CreateUser(UserName, Tags: [{Key,Value}])`:
//!
//! ```text
//! Action=CreateUser&Version=2010-05-08
//! &UserName=Bob
//! &Tags.member.1.Key=team&Tags.member.1.Value=iam
//! &Tags.member.2.Key=env&Tags.member.2.Value=prod
//! ```
//!
//! There is no existing codec that parses this into an arbitrary Smithy shape,
//! so [`QueryShapeDeserializer`] implements the [`ShapeDeserializer`] walk
//! directly over the parsed pairs.
//!
//! ## Layout probing (wrapped vs. flattened)
//!
//! Smithy `@xmlFlattened` changes a list from the wrapped `Name.member.N` form
//! to the flattened `Name.N` form (and a map from `Name.entry.N.key` to
//! `Name.N.key`); `ec2Query` lists are always flattened. The forked
//! `aws-smithy-schema` does not expose a public `@xmlFlattened` getter, so —
//! rather than guess — the deserializer *probes both layouts* and uses
//! whichever the request actually carries. A wire message uses exactly one
//! layout per member, so this is unambiguous and also transparently covers
//! ec2Query's always-flattened encoding.
//!
//! The response side of the query protocols is XML and reuses the existing
//! [`crate::xml`] serializer via [`crate::rest_xml`]-style output; this module
//! is request-deserialization only.

use aws_smithy_schema::serde::{SerdeError, ShapeDeserializer};
use aws_smithy_schema::traits::TimestampFormat;
use aws_smithy_schema::Schema;
use aws_smithy_types::date_time::Format;
use aws_smithy_types::{base64, BigDecimal, BigInteger, Blob, DateTime, Document};
use std::rc::Rc;
use std::str::FromStr;

use crate::http_binding::HttpBindingSerializer;
use crate::protocol::{
    insert_error_response_headers, insert_response_headers, ProtocolError, ServerProtocol,
};
use crate::xml::XmlCodec;
use aws_smithy_runtime_api::http::{Request, Response, StatusCode};
use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::SerializableStruct;
use aws_smithy_schema::{ShapeId, ShapeType, StringTrait};
use aws_smithy_types::body::SdkBody;

/// Decode one `application/x-www-form-urlencoded` component: `+` becomes a
/// space and `%XX` becomes the corresponding byte; malformed escapes are left
/// verbatim. Invalid UTF-8 falls back to the original component.
fn form_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => match (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                (Some(hi), Some(lo)) => {
                    out.push((hi << 4) | lo);
                    i += 3;
                }
                _ => {
                    out.push(bytes[i]);
                    i += 1;
                }
            },
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| input.to_string())
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Parse a form-urlencoded body into decoded `(key, value)` pairs, preserving
/// order. A pair with no `=` is treated as a key with an empty value.
pub fn parse_form_urlencoded(body: &[u8]) -> Vec<(String, String)> {
    let text = std::str::from_utf8(body).unwrap_or("");
    text.split('&')
        .filter(|seg| !seg.is_empty())
        .map(|seg| match seg.split_once('=') {
            Some((k, v)) => (form_decode(k), form_decode(v)),
            None => (form_decode(seg), String::new()),
        })
        .collect()
}

/// Extract the `Action` field from a query-protocol body — the routing key for
/// awsQuery/ec2Query. Returns `None` when absent.
pub fn extract_action(body: &[u8]) -> Option<String> {
    parse_form_urlencoded(body)
        .into_iter()
        .find(|(k, _)| k == "Action")
        .map(|(_, v)| v)
}

/// Map a Smithy `@timestampFormat` to the runtime [`Format`]. The query
/// protocols default to `date-time` (ISO-8601) when unspecified, unlike JSON's
/// epoch-seconds default.
fn query_timestamp_format(schema: &Schema) -> Format {
    match schema.timestamp_format().map(|t| t.format()) {
        Some(TimestampFormat::HttpDate) => Format::HttpDate,
        Some(TimestampFormat::EpochSeconds) => Format::EpochSeconds,
        Some(TimestampFormat::DateTime) | None => Format::DateTime,
    }
}

fn capitalize_first(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// The member's `@ec2QueryName`, if present.
///
/// `@ec2QueryName` has no dedicated accessor on `aws-smithy-schema`, so codegen
/// carries it on the member's generic `TraitMap` as a `StringTrait` keyed by its
/// Smithy shape id. Read it back the same way the constraint walk reads its own
/// traits: look the id up and downcast. The id is minted here to match the one
/// codegen emits (`smithy.api#ec2QueryName`).
fn ec2_query_name(member: &Schema) -> Option<&str> {
    let id = ShapeId::from_static("smithy.api#ec2QueryName", "smithy.api", "ec2QueryName");
    let t = member.traits()?.get(&id)?;
    Some(t.as_any().downcast_ref::<StringTrait>()?.value())
}

/// The request's pairs plus the lookups the walk needs, built once.
///
/// Every question the walk asks is keyed by a dotted path — "what is the value
/// at this key", "does the request carry anything beneath this key", "which
/// indices sit directly beneath it" — and the walk asks them once per shape it
/// visits. Answering each by scanning every pair makes the whole parse quadratic
/// in the body, so the order of the keys is recorded once here instead.
#[derive(Debug)]
struct Pairs {
    /// Decoded pairs in body order. `value_at` resolves a repeated key to the
    /// first, so this order is what decides which one that is.
    entries: Vec<(String, String)>,
    /// `entries` positions ordered by key, for locating a key or a whole run of
    /// keys beneath a prefix by bisection.
    by_key: Vec<usize>,
}

impl Pairs {
    fn new(entries: Vec<(String, String)>) -> Self {
        let mut by_key: Vec<usize> = (0..entries.len()).collect();
        // Ties broken by position so the first spelling of a repeated key sorts
        // first, which is what `value_at` then returns.
        by_key.sort_by(|&a, &b| entries[a].0.cmp(&entries[b].0).then(a.cmp(&b)));
        Self { entries, by_key }
    }

    /// The first `by_key` slot whose key is at or after `key`.
    fn lower_bound(&self, key: &str) -> usize {
        self.by_key
            .partition_point(|&i| self.entries[i].0.as_str() < key)
    }

    /// The value the request carries at exactly `key`, if any.
    fn value_at(&self, key: &str) -> Option<&str> {
        let at = self.lower_bound(key);
        self.by_key
            .get(at)
            .map(|&i| &self.entries[i])
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// The keys at or nested beneath `key`, as `(key, value)`.
    ///
    /// Keys beneath `key` all share the prefix `key.`, and sorting puts them in
    /// one contiguous run, so this walks only that run.
    fn nested(&self, key: &str) -> impl Iterator<Item = (&str, &str)> {
        let dotted = format!("{key}.");
        self.by_key[self.lower_bound(&dotted)..]
            .iter()
            .map(|&i| (self.entries[i].0.as_str(), self.entries[i].1.as_str()))
            .take_while(move |(k, _)| k.starts_with(&dotted))
    }

    /// Whether the request carries `key` itself or anything beneath it.
    fn carries(&self, key: &str) -> bool {
        self.value_at(key).is_some() || self.nested(key).next().is_some()
    }
}

/// A [`ShapeDeserializer`] over form-urlencoded query-protocol pairs.
///
/// The parsed pairs are shared (`Rc`) across the child deserializers produced
/// while walking nested shapes; each child carries the dotted-path `prefix`
/// identifying the value(s) it is positioned at.
#[derive(Debug, Clone)]
pub struct QueryShapeDeserializer {
    pairs: Rc<Pairs>,
    prefix: String,
    ec2: bool,
    /// Set only on the stand-in for a hole in an indexed list, where reading a
    /// scalar yields the empty string instead of reporting the key absent.
    empty_scalar: bool,
    /// How many nested shapes deep this deserializer sits. The root is 0 and
    /// each child produced while walking a struct, list or map is one deeper;
    /// the recursive readers refuse to descend past [`MAX_DEPTH`].
    depth: usize,
}

/// The deepest nesting a request shape may reach before it is refused.
///
/// The dotted-path keys carry the shape structure, so the request chooses how
/// deep the walk recurses: a body whose keys nest a struct inside a struct
/// without bound would drive `read_struct` -> `read_struct` until the thread
/// stack is exhausted and the process aborts. Rejecting past a fixed depth
/// turns that into a clean deserialization error.
const MAX_DEPTH: usize = 127;

/// How many consecutive absent indices an indexed list may be asked to
/// materialise before the request is refused.
///
/// A bound is required, not cosmetic. An index comes from the request, so the
/// element count this walk reaches is attacker-chosen: without a bound the
/// two-key body `Tags.member.1=a&Tags.member.1000000000=z` asks for a billion
/// elements. Reading only as far as the first hole used to bound this by
/// accident, which is why the bound arrives together with reading past one.
///
/// It bounds a single run, not the total: runs can be chained (`.1`, `.11`,
/// `.21`, …), so the elements a body can reach are at most `MAX_ABSENT_RUN + 1`
/// per key it carries. A fixed factor on the body is what this buys, and it is
/// why [`Pairs`] indexes the keys — at a factor this size, answering a per-element
/// lookup by scanning every pair would make the parse quadratic in the body.
///
/// Chaining is deliberate rather than tolerated: the services measured here also
/// bound the run and not the total.
const MAX_ABSENT_RUN: usize = 9;

impl QueryShapeDeserializer {
    /// Build a deserializer over a form-urlencoded body. `ec2` selects
    /// ec2Query key conventions (first-letter-capitalised member names when no
    /// explicit `@xmlName`); pass `false` for awsQuery.
    pub fn new(body: &[u8], ec2: bool) -> Self {
        Self {
            pairs: Rc::new(Pairs::new(parse_form_urlencoded(body))),
            prefix: String::new(),
            ec2,
            empty_scalar: false,
            depth: 0,
        }
    }

    /// Build a deserializer directly from already-parsed pairs (used in tests
    /// and by callers that parse the body once for routing then re-use it).
    pub fn from_pairs(pairs: Vec<(String, String)>, ec2: bool) -> Self {
        Self {
            pairs: Rc::new(Pairs::new(pairs)),
            prefix: String::new(),
            ec2,
            empty_scalar: false,
            depth: 0,
        }
    }

    /// A child positioned at `prefix`, which the request is expected to carry.
    ///
    /// `empty_scalar` is cleared rather than inherited, though nothing can
    /// currently observe the difference: a hole has no key beneath it, so it
    /// spawns no children at all — had the request carried one, the index would
    /// have been present and this would be the child of a present element.
    fn child(&self, prefix: String) -> Self {
        Self {
            pairs: Rc::clone(&self.pairs),
            prefix,
            ec2: self.ec2,
            empty_scalar: false,
            depth: self.depth + 1,
        }
    }

    /// A child standing in for a hole in an indexed list — a position the list
    /// reaches but the request carries no key for.
    ///
    /// Reading a scalar there yields the empty string. Reading a struct there
    /// sets no member, because no key is nested beneath the prefix, so a
    /// `@required` member of the element shape is what reports the hole.
    ///
    /// What a hole then amounts to therefore depends on the element's type, and
    /// only the string and structure cases were measured. A hole in a list of
    /// strings is the empty string, which a `@length` or `@pattern` on the
    /// element refuses and an unconstrained element accepts. A hole in a list of
    /// numbers, booleans or timestamps cannot parse at all, so the whole request
    /// is refused as unreadable rather than as a value out of bounds — a worse
    /// answer than the string case gives, and one no service was measured on.
    fn gap(&self, prefix: String) -> Self {
        Self {
            pairs: Rc::clone(&self.pairs),
            prefix,
            ec2: self.ec2,
            empty_scalar: true,
            depth: self.depth + 1,
        }
    }

    /// The scalar value at the current prefix, if present.
    fn scalar(&self) -> Option<&str> {
        self.pairs
            .value_at(&self.prefix)
            .or(if self.empty_scalar { Some("") } else { None })
    }

    /// True if any key equals `p` or is nested beneath it (`p` + `.`).
    fn has_prefix(&self, p: &str) -> bool {
        self.pairs.carries(p)
    }

    /// The element indices the request carries directly beneath `base` — keys
    /// of the form `base.<N>` or `base.<N>.<rest>` — ascending and deduplicated.
    ///
    /// Each is the index and the segment exactly as the request spelled it: a
    /// leading zero counts as the number it spells (`02` is index 2, measured),
    /// so the number alone would no longer match the key the value is under.
    ///
    /// A segment that is not a number belongs to something other than an indexed
    /// element (a struct member, say) and is passed over. A segment that *is* a
    /// number the walk cannot use — `0`, or more digits than a machine index
    /// holds — is refused rather than passed over, so a key the request did carry
    /// cannot go unread. The one exception is index `0`, which ec2Query was
    /// measured to accept; an index too long to represent is refused either way.
    ///
    /// Two segments spelling the same index would name one element but carry two
    /// values, and only one of them could be read, so that is refused too.
    fn index_tokens(&self, base: &str) -> Result<Vec<(usize, String)>, SerdeError> {
        let mut out: Vec<(usize, String)> = Vec::new();
        for (key, _) in self.pairs.nested(base) {
            let seg = key[base.len() + 1..].split('.').next().unwrap_or("");
            if seg.is_empty() || !seg.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            // Keys are in key order, so every key sharing one spelling of an
            // index arrives in one run and the spelling is taken once.
            if out.last().map(|(_, s)| s.as_str()) == Some(seg) {
                continue;
            }
            let refuse = |what: &str| {
                Err(SerdeError::custom(format!(
                    "`{seg}` is not a valid element index at query key `{base}`: {what}"
                )))
            };
            let n = match seg.parse::<usize>() {
                Ok(0) if !self.ec2 => return refuse("indices start at 1"),
                Ok(n) => n,
                // No client writes an index this long, and passing over it would
                // drop a value the request carried — so neither protocol does.
                Err(_) => return refuse("too many digits to be an index"),
            };
            out.push((n, seg.to_string()));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        // Two spellings of one index are now adjacent. Only one of them could be
        // read, so refuse rather than pick one and drop the other's value.
        if let Some(pair) = out.windows(2).find(|w| w[0].0 == w[1].0) {
            return Err(SerdeError::custom(format!(
                "query key `{base}` spells element {} both `{}` and `{}`, so one of \
                 the two values could not be read",
                pair[0].0, pair[0].1, pair[1].1
            )));
        }
        Ok(out)
    }

    /// Resolve which of the two layouts the request uses for the indexed member
    /// at the current prefix, returning the base the indices hang off and those
    /// indices.
    ///
    /// `wrapper` is the segment the wrapped form interposes (`member` for a
    /// list, `entry` for a map). A wire message uses one layout per member, so
    /// normally only one of the two bases carries anything. A request carrying
    /// both is refused: whichever layout were preferred, the other's keys could
    /// only be dropped, and dropping a key the request carried is the thing this
    /// walk is not allowed to do.
    fn indexed_layout(&self, wrapper: &str) -> Result<(String, Vec<(usize, String)>), SerdeError> {
        let wrapped_base = format!("{}.{}", self.prefix, wrapper);
        let flat_base = self.prefix.clone();
        let wrapped = self.index_tokens(&wrapped_base)?;
        let flattened = self.index_tokens(&flat_base)?;
        match (wrapped.is_empty(), flattened.is_empty()) {
            (false, false) => Err(SerdeError::custom(format!(
                "query key `{flat_base}` carries elements both beneath `{wrapper}` \
                 and without it, so one of the two layouts could not be read"
            ))),
            (false, true) => Ok((wrapped_base, wrapped)),
            _ => Ok((flat_base, flattened)),
        }
    }

    /// The query key segment for a struct member.
    ///
    /// Resolved as the ec2Query spec prescribes: an `@ec2QueryName` (read via
    /// `ec2_query_name`) wins in ec2 mode, then an `@xmlName`, then the member
    /// name, with the latter two first-letter-capitalised in ec2 mode. awsQuery
    /// takes the `@xmlName` or member name verbatim, with no capitalisation.
    fn member_key(&self, member: &Schema) -> String {
        if self.ec2 {
            if let Some(name) = ec2_query_name(member) {
                return name.to_string();
            }
        }
        let name = member
            .xml_name()
            .map(|x| x.value())
            .unwrap_or_else(|| member.member_name().unwrap_or_default());
        if self.ec2 {
            capitalize_first(name)
        } else {
            name.to_string()
        }
    }

    fn join(&self, seg: &str) -> String {
        if self.prefix.is_empty() {
            seg.to_string()
        } else {
            format!("{}.{}", self.prefix, seg)
        }
    }

    fn require_scalar(&self, ty: &str) -> Result<&str, SerdeError> {
        self.scalar().ok_or_else(|| {
            SerdeError::custom(format!(
                "expected {ty} value at query key `{}`, but it was absent",
                self.prefix
            ))
        })
    }

    /// The wrapper segment for wrapped lists: the element member's `@xmlName`,
    /// else the Smithy default `member`.
    fn list_wrapper(&self, schema: &Schema) -> String {
        schema
            .member()
            .and_then(|m| m.xml_name())
            .map(|x| x.value().to_string())
            .unwrap_or_else(|| "member".to_string())
    }

    /// The `.key` / `.value` segments for a map entry, honouring `@xmlName` on
    /// the key/value members (Smithy defaults `key` / `value`).
    ///
    /// For a `ShapeType::Map` schema, `schema.key()` returns the key member and
    /// `schema.member()` returns the *value* member (see `Schema::member`,
    /// which maps `Map { value, .. } => Some(value)`) — so `@xmlName` on either
    /// member is respected, not silently dropped.
    fn map_key_value_segments(&self, schema: &Schema) -> (String, String) {
        let key_seg = schema
            .key()
            .and_then(|m| m.xml_name())
            .map(|x| x.value().to_string())
            .unwrap_or_else(|| "key".to_string());
        let value_seg = schema
            .member()
            .and_then(|m| m.xml_name())
            .map(|x| x.value().to_string())
            .unwrap_or_else(|| "value".to_string());
        (key_seg, value_seg)
    }

    /// Refuse to descend past [`MAX_DEPTH`] nested shapes.
    ///
    /// Called at the entry of each recursive reader so a request that nests
    /// structs, lists or maps without bound is rejected with a clean error
    /// instead of recursing until the thread stack is exhausted.
    fn check_depth(&self) -> Result<(), SerdeError> {
        if self.depth > MAX_DEPTH {
            return Err(SerdeError::custom(format!(
                "request nests deeper than the limit of {MAX_DEPTH}"
            )));
        }
        Ok(())
    }
}

impl ShapeDeserializer for QueryShapeDeserializer {
    fn read_struct(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(&Schema, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.check_depth()?;
        for member in schema.members() {
            let key = self.member_key(member);
            let child_prefix = self.join(&key);
            if self.has_prefix(&child_prefix) {
                let mut child = self.child(child_prefix);
                consumer(member, &mut child)?;
            }
        }
        Ok(())
    }

    fn read_list(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(&mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.check_depth()?;
        let wrapper = self.list_wrapper(schema);
        let (base, indices) = self.indexed_layout(&wrapper)?;
        let Some(&(highest, _)) = indices.last() else {
            return Ok(());
        };

        // An index is a position, so a hole in the indexing is not the end of
        // the list: reading only up to the first hole would hand the handler a
        // shorter list than the request carried, and report nothing.
        if self.ec2 {
            // ec2Query was measured to accept a holed request, with the holes
            // contributing no element, so the present indices are the list.
            for (_, seg) in indices {
                let mut child = self.child(format!("{base}.{seg}"));
                consumer(&mut child)?;
            }
            return Ok(());
        }

        // awsQuery materialises every index from 1 up to the highest present
        // one, so a hole becomes an element none of whose keys are present —
        // which is then the element shape's own constraints to accept or refuse.
        let mut expected = 1usize;
        for (n, _) in &indices {
            // Indices are sorted and distinct, so this cannot wrap; saturating
            // anyway, because a wrap here would read as an enormous skip count.
            let absent = n.saturating_sub(expected);
            if absent > MAX_ABSENT_RUN {
                return Err(SerdeError::custom(format!(
                    "request at query key `{base}` would skip {absent} list elements"
                )));
            }
            expected = n + 1;
        }

        let mut present = indices.into_iter().peekable();
        for n in 1..=highest {
            let carried = present.next_if(|(i, _)| *i == n);
            let mut child = match carried {
                Some((_, seg)) => self.child(format!("{base}.{seg}")),
                None => self.gap(format!("{base}.{n}")),
            };
            consumer(&mut child)?;
        }
        Ok(())
    }

    fn read_map(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(String, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.check_depth()?;
        let (key_seg, value_seg) = self.map_key_value_segments(schema);
        let (base, indices) = self.indexed_layout("entry")?;

        // A map index numbers the entries rather than placing them — the entries
        // carry their own keys — so a hole has nothing to stand for and is passed
        // over; what matters is that it no longer cuts the walk short and drops
        // the entries after it. The numbering still starts at 1, so `0` is
        // refused here as it is for a list; no service was measured on a map.
        for (_, seg) in indices {
            let entry_base = format!("{base}.{seg}");

            let key_prefix = format!("{entry_base}.{key_seg}");
            let key = self
                .child(key_prefix)
                .scalar()
                .map(|s| s.to_string())
                .ok_or_else(|| {
                    SerdeError::custom(format!("map entry `{entry_base}` is missing its key"))
                })?;
            let mut value_child = self.child(format!("{entry_base}.{value_seg}"));
            consumer(key, &mut value_child)?;
        }
        Ok(())
    }

    fn read_boolean(&mut self, _schema: &Schema) -> Result<bool, SerdeError> {
        match self.require_scalar("boolean")? {
            "true" => Ok(true),
            "false" => Ok(false),
            other => Err(SerdeError::custom(format!(
                "expected `true`/`false`, got `{other}`"
            ))),
        }
    }

    fn read_byte(&mut self, schema: &Schema) -> Result<i8, SerdeError> {
        let v = self.read_long(schema)?;
        i8::try_from(v).map_err(|_| SerdeError::custom(format!("number {v} does not fit in i8")))
    }

    fn read_short(&mut self, schema: &Schema) -> Result<i16, SerdeError> {
        let v = self.read_long(schema)?;
        i16::try_from(v).map_err(|_| SerdeError::custom(format!("number {v} does not fit in i16")))
    }

    fn read_integer(&mut self, schema: &Schema) -> Result<i32, SerdeError> {
        let v = self.read_long(schema)?;
        i32::try_from(v).map_err(|_| SerdeError::custom(format!("number {v} does not fit in i32")))
    }

    fn read_long(&mut self, _schema: &Schema) -> Result<i64, SerdeError> {
        let s = self.require_scalar("long")?;
        s.trim()
            .parse::<i64>()
            .map_err(|e| SerdeError::custom(format!("invalid integer `{s}`: {e}")))
    }

    fn read_float(&mut self, schema: &Schema) -> Result<f32, SerdeError> {
        Ok(self.read_double(schema)? as f32)
    }

    fn read_double(&mut self, _schema: &Schema) -> Result<f64, SerdeError> {
        let s = self.require_scalar("double")?;
        s.trim()
            .parse::<f64>()
            .map_err(|e| SerdeError::custom(format!("invalid number `{s}`: {e}")))
    }

    fn read_big_integer(&mut self, _schema: &Schema) -> Result<BigInteger, SerdeError> {
        let s = self.require_scalar("bigInteger")?;
        BigInteger::from_str(s.trim())
            .map_err(|e| SerdeError::custom(format!("invalid bigInteger `{s}`: {e}")))
    }

    fn read_big_decimal(&mut self, _schema: &Schema) -> Result<BigDecimal, SerdeError> {
        let s = self.require_scalar("bigDecimal")?;
        BigDecimal::from_str(s.trim())
            .map_err(|e| SerdeError::custom(format!("invalid bigDecimal `{s}`: {e}")))
    }

    fn read_string(&mut self, _schema: &Schema) -> Result<String, SerdeError> {
        Ok(self.require_scalar("string")?.to_string())
    }

    fn read_blob(&mut self, _schema: &Schema) -> Result<Blob, SerdeError> {
        let s = self.require_scalar("blob")?;
        let bytes =
            base64::decode(s).map_err(|e| SerdeError::custom(format!("invalid base64: {e}")))?;
        Ok(Blob::new(bytes))
    }

    fn read_timestamp(&mut self, schema: &Schema) -> Result<DateTime, SerdeError> {
        let format = query_timestamp_format(schema);
        let s = self.require_scalar("timestamp")?;
        DateTime::from_str(s, format)
            .map_err(|e| SerdeError::custom(format!("invalid timestamp `{s}`: {e}")))
    }

    fn read_document(&mut self, _schema: &Schema) -> Result<Document, SerdeError> {
        // The query protocols have no wire representation for the open
        // `document` type; surface a clear error rather than guessing.
        Err(SerdeError::custom(
            "the awsQuery/ec2Query protocols do not support the `document` type",
        ))
    }

    fn is_null(&self) -> bool {
        // Query encodes absence by omitting the key; there is no explicit null.
        false
    }

    fn read_null(&mut self) -> Result<(), SerdeError> {
        Ok(())
    }

    fn container_size(&self) -> Option<usize> {
        // Query keys do not declare a count up front; the walk discovers the
        // element count by probing indices.
        None
    }
}

/// Build a framework-level (pre-handler) query-protocol error body as raw XML.
///
/// awsQuery and ec2Query use different error envelopes:
/// - awsQuery:  `<ErrorResponse><Error><Type>Sender</Type><Code/><Message/></Error></ErrorResponse>`
/// - ec2Query:  `<Response><Errors><Error><Code/><Message/></Error></Errors></Response>`
///
/// Used by generated dispatch code for failures (unknown `Action`, malformed
/// body) that occur before a modeled error struct exists.
pub fn query_error_body(ec2: bool, code: &str, message: &str) -> String {
    // Escape the code and message before they reach the XML error envelope.
    // The envelope is built by string formatting rather than by the XML writer,
    // so escaping here is what keeps a metacharacter in these values from
    // reading as markup. The escape is unconditional and idempotent against a
    // raw string, so the envelope stays well-formed whatever the caller passes.
    let code = crate::xml::xml_escape(code);
    let message = crate::xml::xml_escape(message);
    if ec2 {
        format!(
            "<?xml version=\"1.0\"?><Response><Errors><Error><Code>{code}</Code><Message>{message}</Message></Error></Errors></Response>"
        )
    } else {
        format!(
            "<?xml version=\"1.0\"?><ErrorResponse><Error><Type>Sender</Type><Code>{code}</Code><Message>{message}</Message></Error></ErrorResponse>"
        )
    }
}

/// Return the raw (undecoded) value of the `Action` field in a form-urlencoded
/// body as a borrowed slice — the routing key for the query protocols.
///
/// Operation names are ASCII identifiers that never require percent-encoding,
/// so the raw slice equals the decoded value; returning a borrow lets
/// [`ServerProtocol::routing_key`] hand back a `&str` tied to the request.
fn find_action_raw(body: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(body).ok()?;
    text.split('&').find_map(|seg| seg.strip_prefix("Action="))
}

/// A distinct request identifier for the `<RequestId>` / `<requestId>`
/// envelope element, generated per response.
///
/// Each response carries its own id so two responses are distinguishable, which
/// is what a client correlates on. It is an identifier, not a security token: a
/// random v4 UUID is sufficient and need not be unpredictable.
fn generate_request_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Server protocol for the AWS `query` family (`awsQuery` / `ec2Query`).
///
/// Requests are `application/x-www-form-urlencoded`; routing is by the `Action`
/// field; responses and errors are XML. The `ec2` flag selects ec2Query
/// conventions (flattened lists, first-letter-capitalised member keys, and the
/// `<Response><Errors>` error envelope).
#[derive(Debug)]
pub struct QueryServerProtocol {
    ec2: bool,
    // The service-specific XML namespace URI stamped on the response/error
    // root element, taken from the model's `@xmlNamespace`. It is wired through
    // `codec()` and `serialize_output` into response/error serialization. A
    // model with no `@xmlNamespace` leaves this `None`, and no `xmlns`
    // attribute is emitted — SDK parsers tolerate its absence.
    namespace: Option<&'static str>,
}

impl QueryServerProtocol {
    /// awsQuery (IAM, STS, SES, …).
    pub fn aws_query() -> Self {
        Self {
            ec2: false,
            namespace: None,
        }
    }

    /// ec2Query (EC2).
    pub fn ec2_query() -> Self {
        Self {
            ec2: true,
            namespace: None,
        }
    }

    /// Set the XML namespace applied to response/error envelopes.
    pub fn with_namespace(mut self, ns: &'static str) -> Self {
        self.namespace = Some(ns);
        self
    }

    fn codec(&self) -> XmlCodec {
        match self.namespace {
            Some(ns) => XmlCodec::new().with_namespace(ns),
            None => XmlCodec::new(),
        }
    }
}

static ERROR_RESPONSE_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "smithy.framework#ErrorResponse",
        "smithy.framework",
        "ErrorResponse",
    ),
    ShapeType::Structure,
    &[],
);
static ERROR_ELEMENT_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static("smithy.framework#Error", "smithy.framework", "Error"),
    ShapeType::Structure,
    &[],
);
static EC2_RESPONSE_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static("smithy.framework#Response", "smithy.framework", "Response"),
    ShapeType::Structure,
    &[],
);
static EC2_ERRORS_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static("smithy.framework#Errors", "smithy.framework", "Errors"),
    ShapeType::Structure,
    &[],
);

impl ServerProtocol for QueryServerProtocol {
    type Request = Request;
    type Response = Response;

    fn protocol_id(&self) -> &ShapeId {
        static AWS_QUERY: ShapeId =
            ShapeId::from_static("aws.protocols#awsQuery", "aws.protocols", "awsQuery");
        static EC2_QUERY: ShapeId =
            ShapeId::from_static("aws.protocols#ec2Query", "aws.protocols", "ec2Query");
        if self.ec2 {
            &EC2_QUERY
        } else {
            &AWS_QUERY
        }
    }

    fn routing_key<'a>(&self, request: &'a Request) -> Result<&'a str, ProtocolError> {
        let body = request
            .body()
            .bytes()
            .ok_or_else(|| ProtocolError::BodyRead("body is not available as bytes".into()))?;
        find_action_raw(body).ok_or_else(|| {
            ProtocolError::UnknownOperation("request is missing the `Action` field".into())
        })
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a Request,
        _input_schema: &Schema,
    ) -> Result<Box<dyn ShapeDeserializer + 'a>, ProtocolError> {
        let body = request
            .body()
            .bytes()
            .ok_or_else(|| ProtocolError::BodyRead("body is not available as bytes".into()))?;
        // The query protocols have no HTTP-bound members: every input member is
        // carried in the form-urlencoded body, so the query deserializer is used
        // directly (no HttpBindingDeserializer wrapper).
        Ok(Box::new(QueryShapeDeserializer::new(body, self.ec2)))
    }

    fn serialize_output(
        &self,
        output: &dyn SerializableStruct,
        _output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<Response, ProtocolError> {
        // Both query protocols wrap the serialized members in an XML envelope,
        // but the two envelopes differ:
        //
        //   awsQuery  <{Operation}Response xmlns="…">
        //               <{Operation}Result>…members…</{Operation}Result>
        //               <ResponseMetadata><RequestId>…</RequestId></ResponseMetadata>
        //             </{Operation}Response>
        //
        //   ec2Query  <{Operation}Response xmlns="…">
        //               …members…
        //               <requestId>…</requestId>
        //             </{Operation}Response>
        //
        // ec2Query has no `…Result` wrapper and carries a lowercase top-level
        // `<requestId>` in place of awsQuery's `<ResponseMetadata>` block.
        let codec = self.codec();
        let body = codec.create_serializer();

        // Serialize the members into the body (the inner payload)
        let mut binder = HttpBindingSerializer::new(body);
        output.serialize_members(&mut binder)?;
        let (collected, body) = binder.finish();
        let inner_bytes = body.finish();

        // The response element name is derived from the operation and the
        // namespace from the model's `@xmlNamespace` — both trusted,
        // schema-derived values, not request input. Escaping them is defensive,
        // not a reachable-injection fix: a cheap invariant that keeps the
        // envelope well-formed even if a future model or codegen path admitted
        // a metacharacter. A model with no `@xmlNamespace` gets no `xmlns`
        // attribute at all — a client parser does not require one.
        let op = crate::xml::xml_escape(operation_name);
        let xmlns_attr = match self.namespace {
            Some(ns) => format!(" xmlns=\"{}\"", crate::xml::xml_escape(ns)),
            None => String::new(),
        };
        let request_id = generate_request_id();

        let mut envelope = Vec::new();
        if self.ec2 {
            envelope.extend_from_slice(format!("<{op}Response{xmlns_attr}>").as_bytes());
            envelope.extend_from_slice(&inner_bytes);
            envelope.extend_from_slice(
                format!("<requestId>{request_id}</requestId></{op}Response>").as_bytes(),
            );
        } else {
            envelope
                .extend_from_slice(format!("<{op}Response{xmlns_attr}><{op}Result>").as_bytes());
            envelope.extend_from_slice(&inner_bytes);
            envelope.extend_from_slice(
                format!(
                    "</{op}Result><ResponseMetadata><RequestId>{request_id}</RequestId></ResponseMetadata></{op}Response>"
                )
                .as_bytes(),
            );
        }

        let status = collected.status_code.unwrap_or(success_status);
        let mut resp = Response::new(
            StatusCode::try_from(status)
                .unwrap_or_else(|_| StatusCode::try_from(200u16).expect("200")),
            SdkBody::from(envelope),
        );
        resp.headers_mut().insert("content-type", "text/xml");
        insert_response_headers(&mut resp, collected.headers)?;
        Ok(resp)
    }

    fn serialize_error(
        &self,
        error: &dyn SerializableStruct,
        _error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<Response, ProtocolError> {
        let codec = self.codec();
        let mut ser = codec.create_serializer();

        let collected = if self.ec2 {
            // <Response><Errors><Error><Code/>…members…</Error></Errors></Response>
            ser.open_element(&EC2_RESPONSE_SCHEMA)?;
            ser.open_element(&EC2_ERRORS_SCHEMA)?;
            ser.open_element(&ERROR_ELEMENT_SCHEMA)?;
            ser.write_text_element("Code", error_code)?;
            let mut binder = HttpBindingSerializer::new(ser);
            error.serialize_members(&mut binder)?;
            let (collected, mut ser) = binder.finish();
            ser.close_element(&ERROR_ELEMENT_SCHEMA)?;
            ser.close_element(&EC2_ERRORS_SCHEMA)?;
            ser.close_element(&EC2_RESPONSE_SCHEMA)?;
            let body_bytes = ser.finish();
            (collected, body_bytes)
        } else {
            // <ErrorResponse><Error><Type/><Code/>…members…</Error></ErrorResponse>
            let error_type = if http_status >= 500 {
                "Receiver"
            } else {
                "Sender"
            };
            ser.open_element(&ERROR_RESPONSE_SCHEMA)?;
            ser.open_element(&ERROR_ELEMENT_SCHEMA)?;
            ser.write_text_element("Type", error_type)?;
            ser.write_text_element("Code", error_code)?;
            let mut binder = HttpBindingSerializer::new(ser);
            error.serialize_members(&mut binder)?;
            let (collected, mut ser) = binder.finish();
            ser.close_element(&ERROR_ELEMENT_SCHEMA)?;
            ser.close_element(&ERROR_RESPONSE_SCHEMA)?;
            let body_bytes = ser.finish();
            (collected, body_bytes)
        };

        let (collected, body_bytes) = collected;
        let status = StatusCode::try_from(http_status)
            .unwrap_or_else(|_| StatusCode::try_from(400u16).expect("400"));
        let mut resp = Response::new(status, SdkBody::from(body_bytes));
        resp.headers_mut().insert("content-type", "text/xml");
        insert_error_response_headers(&mut resp, collected.headers);
        Ok(resp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_smithy_schema::TraitMap;
    use std::sync::LazyLock;

    #[test]
    fn form_decode_handles_plus_and_percent() {
        assert_eq!(form_decode("a+b"), "a b");
        assert_eq!(form_decode("a%20b"), "a b");
        assert_eq!(form_decode("100%25"), "100%");
        // Malformed escape is left verbatim.
        assert_eq!(form_decode("%zz"), "%zz");
    }

    #[test]
    fn parse_pairs_preserves_order_and_decodes() {
        let pairs = parse_form_urlencoded(b"Action=CreateUser&UserName=Bob+Smith&Empty=");
        assert_eq!(
            pairs,
            vec![
                ("Action".to_string(), "CreateUser".to_string()),
                ("UserName".to_string(), "Bob Smith".to_string()),
                ("Empty".to_string(), String::new()),
            ]
        );
    }

    #[test]
    fn extract_action_finds_operation() {
        assert_eq!(
            extract_action(b"Version=2010-05-08&Action=GetUser&UserName=x").as_deref(),
            Some("GetUser")
        );
        assert_eq!(extract_action(b"Version=2010-05-08").as_deref(), None);
    }

    #[test]
    fn find_action_raw_borrows_value() {
        assert_eq!(
            find_action_raw(b"Action=CreateUser&UserName=x"),
            Some("CreateUser")
        );
        assert_eq!(
            find_action_raw(b"Version=1&Action=Describe"),
            Some("Describe")
        );
        assert_eq!(find_action_raw(b"UserName=x"), None);
    }

    #[test]
    fn aws_query_error_body_uses_error_response_envelope() {
        let body = query_error_body(false, "InvalidAction", "no such action: <Foo>");
        assert!(body.contains("<ErrorResponse>"));
        assert!(body.contains("<Type>Sender</Type>"));
        assert!(body.contains("<Code>InvalidAction</Code>"));
        // Message content is XML-escaped.
        assert!(body.contains("no such action: &lt;Foo&gt;"));
        assert!(!body.contains("<Response>"));
    }

    #[test]
    fn ec2_query_error_body_uses_response_errors_envelope() {
        let body = query_error_body(true, "InvalidAction", "bad");
        assert!(body.contains("<Response><Errors><Error>"));
        assert!(body.contains("<Code>InvalidAction</Code>"));
        assert!(body.contains("<Message>bad</Message>"));
        // ec2Query error envelope has no <Type> element.
        assert!(!body.contains("<Type>"));
    }

    // Schema: structure Input { UserName: String, Count: Integer }
    static USER_NAME: Schema = Schema::new_member(
        ShapeId::from_static("com.example#Input$UserName", "com.example", "UserName"),
        ShapeType::String,
        "UserName",
        0,
    );
    static COUNT: Schema = Schema::new_member(
        ShapeId::from_static("com.example#Input$Count", "com.example", "Count"),
        ShapeType::Integer,
        "Count",
        1,
    );
    static INPUT: Schema = Schema::new_struct(
        ShapeId::from_static("com.example#Input", "com.example", "Input"),
        ShapeType::Structure,
        &[&USER_NAME, &COUNT],
    );

    #[test]
    fn read_struct_reads_scalar_members() {
        let mut de = QueryShapeDeserializer::new(b"Action=X&UserName=Bob+Smith&Count=42", false);

        let mut user_name = None;
        let mut count = None;
        de.read_struct(&INPUT, &mut |member, d| {
            match member.member_name() {
                Some("UserName") => user_name = Some(d.read_string(member)?),
                Some("Count") => count = Some(d.read_integer(member)?),
                _ => {}
            }
            Ok(())
        })
        .expect("struct reads");

        assert_eq!(user_name.as_deref(), Some("Bob Smith"));
        assert_eq!(count, Some(42));
    }

    // Schema: structure Ec2Input { instanceId: String } where the member carries
    // an `@ec2QueryName("InstanceIdentifier")`. codegen attaches it as a
    // `StringTrait` on the member's `TraitMap`, exactly as
    // `ec2_query_name_insert` emits it. The trait value is deliberately NOT the
    // capitalised member name (`instanceId` -> `InstanceId`), so a test that
    // reads under `InstanceIdentifier` proves the trait path was taken rather
    // than the member-name fallback.
    static EC2_INSTANCE_ID_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
        let mut m = TraitMap::new();
        m.insert(Box::new(StringTrait::new(
            ShapeId::from_static("smithy.api#ec2QueryName", "smithy.api", "ec2QueryName"),
            "InstanceIdentifier",
        )));
        m
    });
    static EC2_INSTANCE_ID: Schema = Schema::new_member(
        ShapeId::from_static(
            "com.example#Ec2Input$instanceId",
            "com.example",
            "instanceId",
        ),
        ShapeType::String,
        "instanceId",
        0,
    )
    .with_traits(&EC2_INSTANCE_ID_TRAITS);
    static EC2_INPUT: Schema = Schema::new_struct(
        ShapeId::from_static("com.example#Ec2Input", "com.example", "Ec2Input"),
        ShapeType::Structure,
        &[&EC2_INSTANCE_ID],
    );

    #[test]
    fn ec2_query_name_resolves_the_member_key_in_ec2_mode() {
        // The request uses the `@ec2QueryName` key, which differs from both the
        // member name and its capitalisation.
        let mut de = QueryShapeDeserializer::new(b"Action=X&InstanceIdentifier=i-0abc", true);
        let mut instance_id = None;
        de.read_struct(&EC2_INPUT, &mut |member, d| {
            if member.member_name() == Some("instanceId") {
                instance_id = Some(d.read_string(member)?);
            }
            Ok(())
        })
        .expect("struct reads");
        assert_eq!(instance_id.as_deref(), Some("i-0abc"));
    }

    #[test]
    fn member_key_prefers_ec2_query_name_over_capitalised_name() {
        // ec2 mode: the `@ec2QueryName` wins over the capitalised member name.
        let ec2 = QueryShapeDeserializer::new(b"", true);
        assert_eq!(ec2.member_key(&EC2_INSTANCE_ID), "InstanceIdentifier");
        // awsQuery (non-ec2) mode: `@ec2QueryName` does not apply, so the key is
        // the bare member name.
        let aws = QueryShapeDeserializer::new(b"", false);
        assert_eq!(aws.member_key(&EC2_INSTANCE_ID), "instanceId");
    }

    // A member carrying only `@xmlName("fooBar")` and no `@ec2QueryName`. The
    // ec2Query spec resolves its key as the `@xmlName` with the first letter
    // capitalised, where awsQuery takes it verbatim.
    static XML_NAMED: Schema = Schema::new_member(
        ShapeId::from_static("com.example#S$member", "com.example", "member"),
        ShapeType::String,
        "member",
        0,
    )
    .with_xml_name("fooBar");

    #[test]
    fn member_key_capitalises_xml_name_in_ec2_mode_only() {
        // ec2 mode: `@xmlName` key gets its first letter capitalised.
        let ec2 = QueryShapeDeserializer::new(b"", true);
        assert_eq!(ec2.member_key(&XML_NAMED), "FooBar");
        // awsQuery mode: `@xmlName` is taken verbatim.
        let aws = QueryShapeDeserializer::new(b"", false);
        assert_eq!(aws.member_key(&XML_NAMED), "fooBar");
    }

    #[test]
    fn absent_members_are_skipped() {
        let mut de = QueryShapeDeserializer::new(b"UserName=OnlyName", false);
        let mut seen: Vec<String> = Vec::new();
        de.read_struct(&INPUT, &mut |member, _d| {
            seen.push(member.member_name().unwrap_or("").to_string());
            Ok(())
        })
        .expect("struct reads");
        // Only the present member is visited; Count is omitted.
        assert_eq!(seen, vec!["UserName".to_string()]);
    }

    // Schema: structure with a list member Tags: [String]
    static TAG_ELEMENT: Schema = Schema::new_member(
        ShapeId::from_static("com.example#Tags$member", "com.example", "member"),
        ShapeType::String,
        "member",
        0,
    );
    static TAGS_LIST: Schema = Schema::new_list(
        ShapeId::from_static("com.example#Tags", "com.example", "Tags"),
        &TAG_ELEMENT,
    );

    fn read_string_list(de: &mut QueryShapeDeserializer, list_schema: &Schema) -> Vec<String> {
        let mut out = Vec::new();
        de.read_list(list_schema, &mut |d| {
            out.push(d.read_string(&TAG_ELEMENT)?);
            Ok(())
        })
        .expect("list reads");
        out
    }

    #[test]
    fn read_list_wrapped_member_form() {
        // awsQuery wrapped list: Tags.member.1, Tags.member.2
        let mut de = QueryShapeDeserializer::from_pairs(
            vec![
                ("Tags.member.1".to_string(), "a".to_string()),
                ("Tags.member.2".to_string(), "b".to_string()),
            ],
            false,
        );
        de.prefix = "Tags".to_string();
        assert_eq!(read_string_list(&mut de, &TAGS_LIST), vec!["a", "b"]);
    }

    #[test]
    fn read_list_flattened_form() {
        // ec2Query / @xmlFlattened list: Tags.1, Tags.2
        let mut de = QueryShapeDeserializer::from_pairs(
            vec![
                ("Tags.1".to_string(), "x".to_string()),
                ("Tags.2".to_string(), "y".to_string()),
                ("Tags.3".to_string(), "z".to_string()),
            ],
            true,
        );
        de.prefix = "Tags".to_string();
        assert_eq!(read_string_list(&mut de, &TAGS_LIST), vec!["x", "y", "z"]);
    }

    /// `Tags.member.1` and `Tags.member.3` with nothing at `2`.
    ///
    /// The hole is a position in the list, so the element after it must still be
    /// read: ending the list at the hole would hand the handler two elements
    /// where the request carried a third, and say nothing about it.
    #[test]
    fn a_hole_does_not_end_an_aws_query_list() {
        let mut de = QueryShapeDeserializer::from_pairs(
            vec![
                ("Tags.member.1".to_string(), "a".to_string()),
                ("Tags.member.3".to_string(), "c".to_string()),
            ],
            false,
        );
        de.prefix = "Tags".to_string();
        assert_eq!(read_string_list(&mut de, &TAGS_LIST), vec!["a", "", "c"]);
    }

    /// Indices need not arrive in order, and the list is still positional.
    #[test]
    fn element_order_follows_the_index_not_the_body() {
        let mut de = QueryShapeDeserializer::from_pairs(
            vec![
                ("Tags.member.3".to_string(), "c".to_string()),
                ("Tags.member.1".to_string(), "a".to_string()),
                ("Tags.member.2".to_string(), "b".to_string()),
            ],
            false,
        );
        de.prefix = "Tags".to_string();
        assert_eq!(read_string_list(&mut de, &TAGS_LIST), vec!["a", "b", "c"]);
    }

    /// A list whose indices start above 1 keeps the leading holes, so the value
    /// the request carried stays at the position it named.
    #[test]
    fn leading_holes_are_kept() {
        let mut de = QueryShapeDeserializer::from_pairs(
            vec![("Tags.member.2".to_string(), "b".to_string())],
            false,
        );
        de.prefix = "Tags".to_string();
        assert_eq!(read_string_list(&mut de, &TAGS_LIST), vec!["", "b"]);
    }

    /// The bound is what keeps a short key from naming a huge list. Without it
    /// `Tags.member.1000000000` would ask for a billion elements.
    #[test]
    fn an_over_long_run_of_holes_is_refused() {
        let mut de = QueryShapeDeserializer::from_pairs(
            vec![
                ("Tags.member.1".to_string(), "a".to_string()),
                ("Tags.member.1000000000".to_string(), "z".to_string()),
            ],
            false,
        );
        de.prefix = "Tags".to_string();
        let err = de
            .read_list(&TAGS_LIST, &mut |d| d.read_string(&TAG_ELEMENT).map(|_| ()))
            .expect_err("a run of holes this long is refused");
        assert!(
            err.to_string().contains("would skip"),
            "the refusal should say what it would have had to skip, got: {err}"
        );
    }

    /// Right at the bound the list is still read, so the refusal above is the
    /// bound talking and not a list with any hole at all being refused.
    #[test]
    fn a_run_of_holes_at_the_bound_is_still_read() {
        let mut de = QueryShapeDeserializer::from_pairs(
            vec![
                ("Tags.member.1".to_string(), "a".to_string()),
                (
                    format!("Tags.member.{}", MAX_ABSENT_RUN + 2),
                    "z".to_string(),
                ),
            ],
            false,
        );
        de.prefix = "Tags".to_string();
        let out = read_string_list(&mut de, &TAGS_LIST);
        assert_eq!(out.len(), MAX_ABSENT_RUN + 2);
        assert_eq!(out.first().map(String::as_str), Some("a"));
        assert_eq!(out.last().map(String::as_str), Some("z"));
    }

    /// Runs of holes can be chained, each one inside the bound, so what the
    /// bound buys is a fixed factor on the body rather than a cap on the list.
    /// This pins that: such a body is accepted, and stays inside the factor.
    #[test]
    fn chained_runs_of_holes_stay_within_a_fixed_factor_of_the_body() {
        let stride = MAX_ABSENT_RUN + 1;
        let keys = 3;
        let pairs: Vec<(String, String)> = (0..keys)
            .map(|i| (format!("Tags.member.{}", i * stride + 1), "v".to_string()))
            .collect();
        let mut de = QueryShapeDeserializer::from_pairs(pairs, false);
        de.prefix = "Tags".to_string();

        let out = read_string_list(&mut de, &TAGS_LIST);
        // Three keys at stride 10 reach element 21, so 21 elements from 3 keys —
        // inside the documented factor of `MAX_ABSENT_RUN + 1` per key, and the
        // values land at the positions the request named.
        assert_eq!(out.len(), (keys - 1) * stride + 1);
        assert_eq!(out.iter().filter(|s| *s == "v").count(), keys);
        assert_eq!(out.first().map(String::as_str), Some("v"));
        assert_eq!(out.last().map(String::as_str), Some("v"));
    }

    /// ec2Query was measured to accept a holed request without refusing it, so
    /// there the holes contribute no element. The list is still not cut short.
    #[test]
    fn ec2_query_passes_over_holes_instead_of_filling_them() {
        let mut de = QueryShapeDeserializer::from_pairs(
            vec![
                ("Tags.1".to_string(), "x".to_string()),
                ("Tags.3".to_string(), "z".to_string()),
            ],
            true,
        );
        de.prefix = "Tags".to_string();
        assert_eq!(read_string_list(&mut de, &TAGS_LIST), vec!["x", "z"]);
    }

    /// Indices start at 1, so `0` names no position. Passing over it would drop a
    /// value the request carried, so it is refused instead.
    #[test]
    fn index_zero_is_refused_rather_than_dropped() {
        let mut de = QueryShapeDeserializer::from_pairs(
            vec![
                ("Tags.member.1".to_string(), "a".to_string()),
                ("Tags.member.0".to_string(), "b".to_string()),
            ],
            false,
        );
        de.prefix = "Tags".to_string();
        let err = de
            .read_list(&TAGS_LIST, &mut |d| d.read_string(&TAG_ELEMENT).map(|_| ()))
            .expect_err("`0` is not a valid index");
        assert!(
            err.to_string().contains("indices start at 1"),
            "the refusal should say why `0` is not an index, got: {err}"
        );
    }

    /// A leading zero spells the number it spells: `02` is the second element,
    /// not a hole and not a dropped key. Measured against the live services.
    #[test]
    fn a_leading_zero_names_the_index_it_spells() {
        let mut de = QueryShapeDeserializer::from_pairs(
            vec![
                ("Tags.member.1".to_string(), "a".to_string()),
                ("Tags.member.02".to_string(), "b".to_string()),
            ],
            false,
        );
        de.prefix = "Tags".to_string();
        assert_eq!(read_string_list(&mut de, &TAGS_LIST), vec!["a", "b"]);
    }

    /// And on its own, so the value stays reachable rather than being read as a
    /// hole because the number no longer matches the key it sits under.
    #[test]
    fn a_leading_zero_alone_is_still_read() {
        let mut de = QueryShapeDeserializer::from_pairs(
            vec![("Tags.member.01".to_string(), "a".to_string())],
            false,
        );
        de.prefix = "Tags".to_string();
        assert_eq!(read_string_list(&mut de, &TAGS_LIST), vec!["a"]);
    }

    /// An index too long to represent is refused under either protocol, rather
    /// than passed over, since passing over it would drop the value it carries.
    #[test]
    fn an_index_too_long_to_represent_is_refused_under_either_protocol() {
        let huge = "9".repeat(40);
        for ec2 in [false, true] {
            let de = QueryShapeDeserializer::from_pairs(
                vec![(format!("Tags.member.{huge}"), "a".to_string())],
                ec2,
            );
            let err = de
                .index_tokens("Tags.member")
                .expect_err("an index of 40 digits is refused");
            assert!(
                err.to_string().contains("digits"),
                "the refusal should say why, got: {err}"
            );
        }
    }

    /// The bound's magnitude, written with literal indices so that loosening the
    /// constant cannot pass. `MAX_ABSENT_RUN` is 9, so index 11 after index 1 is
    /// 9 holes and index 12 is 10.
    #[test]
    fn the_absent_run_bound_is_nine_holes() {
        assert_eq!(MAX_ABSENT_RUN, 9, "the cases below are written for 9");
        let read = |far: &str| {
            let mut de = QueryShapeDeserializer::from_pairs(
                vec![
                    ("Tags.member.1".to_string(), "a".to_string()),
                    (format!("Tags.member.{far}"), "z".to_string()),
                ],
                false,
            );
            de.prefix = "Tags".to_string();
            de.read_list(&TAGS_LIST, &mut |d| d.read_string(&TAG_ELEMENT).map(|_| ()))
        };
        assert!(read("11").is_ok(), "9 holes is inside the bound");
        assert!(read("12").is_err(), "10 holes is outside it");
    }

    /// A body carrying BOTH layouts for one member is refused. Preferring either
    /// would drop the other's keys, which is the failure this walk exists to
    /// avoid; before the bound and the layout check, it silently dropped them.
    #[test]
    fn a_member_carrying_both_layouts_is_refused() {
        let mut de = QueryShapeDeserializer::from_pairs(
            vec![
                ("Tags.member.1".to_string(), "a".to_string()),
                ("Tags.2".to_string(), "b".to_string()),
            ],
            false,
        );
        de.prefix = "Tags".to_string();
        let err = de
            .read_list(&TAGS_LIST, &mut |d| d.read_string(&TAG_ELEMENT).map(|_| ()))
            .expect_err("two layouts for one member is refused");
        assert!(
            err.to_string().contains("layouts"),
            "the refusal should say why, got: {err}"
        );
    }

    /// Two spellings of one index carry two values where only one can be read.
    #[test]
    fn two_spellings_of_one_index_are_refused() {
        for order in [["1", "01"], ["01", "1"]] {
            let mut de = QueryShapeDeserializer::from_pairs(
                vec![
                    (format!("Tags.member.{}", order[0]), "a".to_string()),
                    (format!("Tags.member.{}", order[1]), "b".to_string()),
                ],
                false,
            );
            de.prefix = "Tags".to_string();
            let err = de
                .read_list(&TAGS_LIST, &mut |d| d.read_string(&TAG_ELEMENT).map(|_| ()))
                .expect_err("one index spelled twice is refused");
            assert!(
                err.to_string().contains("could not be read"),
                "the refusal should say a value would be lost, got: {err}"
            );
        }
    }

    /// A member absent beneath an element the request DID carry stays absent —
    /// it is not visited at all, and so never picks up the hole's empty string.
    #[test]
    fn an_absent_member_of_a_present_element_is_not_visited() {
        let mut de = QueryShapeDeserializer::from_pairs(
            vec![
                ("Items.member.1.UserName".to_string(), "Bob".to_string()),
                ("Items.member.3.UserName".to_string(), "Eve".to_string()),
            ],
            false,
        );
        de.prefix = "Items".to_string();
        static ITEMS: Schema = Schema::new_list(
            ShapeId::from_static("com.example#Items", "com.example", "Items"),
            &INPUT,
        );

        // `Count` is absent under every element, including the two present ones.
        let mut counts: Vec<Result<i32, ()>> = Vec::new();
        de.read_list(&ITEMS, &mut |d| {
            d.read_struct(&INPUT, &mut |member, md| {
                if member.member_name() == Some("Count") {
                    counts.push(md.read_integer(member).map_err(|_| ()));
                }
                Ok(())
            })
        })
        .expect("the list reads");
        assert!(
            counts.is_empty(),
            "an absent member must not be visited at all, got {counts:?}"
        );
    }

    /// A hole in a list of numbers cannot parse, so the request is refused rather
    /// than quietly becoming a zero. Recorded because it differs from the string
    /// case and no service was measured on it.
    #[test]
    fn a_hole_in_a_number_list_is_refused_rather_than_defaulted() {
        static NUM: Schema = Schema::new(
            ShapeId::from_static("com.example#Num", "com.example", "Num"),
            ShapeType::Integer,
        );
        static NUMS: Schema = Schema::new_list(
            ShapeId::from_static("com.example#Nums", "com.example", "Nums"),
            &NUM,
        );
        let mut de = QueryShapeDeserializer::from_pairs(
            vec![
                ("Nums.member.1".to_string(), "1".to_string()),
                ("Nums.member.3".to_string(), "3".to_string()),
            ],
            false,
        );
        de.prefix = "Nums".to_string();
        let mut seen: Vec<i32> = Vec::new();
        let result = de.read_list(&NUMS, &mut |d| {
            seen.push(d.read_integer(&NUM)?);
            Ok(())
        });
        assert!(result.is_err(), "an empty number is not a number: {seen:?}");
        assert_eq!(seen, vec![1], "the hole stops the read, it is not a zero");
    }

    /// ec2Query was measured to accept index 0, and it must keep the element.
    #[test]
    fn ec2_query_keeps_an_element_at_index_zero() {
        let mut de = QueryShapeDeserializer::from_pairs(
            vec![
                ("Tags.0".to_string(), "zero".to_string()),
                ("Tags.1".to_string(), "one".to_string()),
            ],
            true,
        );
        de.prefix = "Tags".to_string();
        assert_eq!(read_string_list(&mut de, &TAGS_LIST), vec!["zero", "one"]);
    }

    /// A body whose holes are chained stays correct at a size where answering
    /// each per-element lookup by scanning every pair would not finish promptly.
    #[test]
    fn a_large_chained_body_is_read_correctly() {
        let stride = MAX_ABSENT_RUN + 1;
        let keys = 4_000;
        let pairs: Vec<(String, String)> = (0..keys)
            .map(|i| (format!("Tags.member.{}", i * stride + 1), "v".to_string()))
            .collect();
        let mut de = QueryShapeDeserializer::from_pairs(pairs, false);
        de.prefix = "Tags".to_string();

        let out = read_string_list(&mut de, &TAGS_LIST);
        assert_eq!(out.len(), (keys - 1) * stride + 1);
        assert_eq!(out.iter().filter(|s| *s == "v").count(), keys);
    }

    /// A struct member whose name is not a number must not be read as an index.
    #[test]
    fn a_named_segment_is_not_mistaken_for_an_index() {
        let de = QueryShapeDeserializer::from_pairs(
            vec![("Filter.Name".to_string(), "key".to_string())],
            false,
        );
        assert_eq!(
            de.index_tokens("Filter").expect("no index is not an error"),
            Vec::<(usize, String)>::new()
        );
    }

    // Schema: map<String,String> Attributes
    static MAP_KEY: Schema = Schema::new_member(
        ShapeId::from_static("com.example#Attrs$key", "com.example", "key"),
        ShapeType::String,
        "key",
        0,
    );
    static MAP_VALUE: Schema = Schema::new_member(
        ShapeId::from_static("com.example#Attrs$value", "com.example", "value"),
        ShapeType::String,
        "value",
        1,
    );
    static ATTRS_MAP: Schema = Schema::new_map(
        ShapeId::from_static("com.example#Attrs", "com.example", "Attrs"),
        &MAP_KEY,
        &MAP_VALUE,
    );

    #[test]
    fn read_map_wrapped_entry_form() {
        let mut de = QueryShapeDeserializer::from_pairs(
            vec![
                ("Attrs.entry.1.key".to_string(), "region".to_string()),
                ("Attrs.entry.1.value".to_string(), "iad".to_string()),
                ("Attrs.entry.2.key".to_string(), "tier".to_string()),
                ("Attrs.entry.2.value".to_string(), "prod".to_string()),
            ],
            false,
        );
        de.prefix = "Attrs".to_string();

        let mut entries: Vec<(String, String)> = Vec::new();
        de.read_map(&ATTRS_MAP, &mut |k, d| {
            entries.push((k, d.read_string(&MAP_VALUE)?));
            Ok(())
        })
        .expect("map reads");
        assert_eq!(
            entries,
            vec![
                ("region".to_string(), "iad".to_string()),
                ("tier".to_string(), "prod".to_string()),
            ]
        );
    }

    #[test]
    fn read_map_flattened_form() {
        // ec2Query / @xmlFlattened map: Attrs.1.key / Attrs.1.value (no `entry`).
        let mut de = QueryShapeDeserializer::from_pairs(
            vec![
                ("Attrs.1.key".to_string(), "region".to_string()),
                ("Attrs.1.value".to_string(), "iad".to_string()),
                ("Attrs.2.key".to_string(), "tier".to_string()),
                ("Attrs.2.value".to_string(), "prod".to_string()),
            ],
            true,
        );
        de.prefix = "Attrs".to_string();

        let mut entries: Vec<(String, String)> = Vec::new();
        de.read_map(&ATTRS_MAP, &mut |k, d| {
            entries.push((k, d.read_string(&MAP_VALUE)?));
            Ok(())
        })
        .expect("flattened map reads");
        assert_eq!(
            entries,
            vec![
                ("region".to_string(), "iad".to_string()),
                ("tier".to_string(), "prod".to_string()),
            ]
        );
    }

    /// A hole in a map's entry numbering must not drop the entries after it.
    /// Unlike a list index the number is only an enumeration, so the hole itself
    /// stands for nothing and contributes no entry.
    #[test]
    fn a_hole_does_not_end_a_map() {
        let mut de = QueryShapeDeserializer::from_pairs(
            vec![
                ("Attrs.entry.1.key".to_string(), "region".to_string()),
                ("Attrs.entry.1.value".to_string(), "iad".to_string()),
                ("Attrs.entry.3.key".to_string(), "tier".to_string()),
                ("Attrs.entry.3.value".to_string(), "prod".to_string()),
            ],
            false,
        );
        de.prefix = "Attrs".to_string();

        let mut entries: Vec<(String, String)> = Vec::new();
        de.read_map(&ATTRS_MAP, &mut |k, d| {
            entries.push((k, d.read_string(&MAP_VALUE)?));
            Ok(())
        })
        .expect("map reads past the hole");
        assert_eq!(
            entries,
            vec![
                ("region".to_string(), "iad".to_string()),
                ("tier".to_string(), "prod".to_string()),
            ]
        );
    }

    #[test]
    fn read_timestamp_defaults_to_iso8601() {
        let mut de = QueryShapeDeserializer::from_pairs(
            vec![("When".to_string(), "2019-12-16T23:48:18Z".to_string())],
            false,
        );
        de.prefix = "When".to_string();
        static TS: Schema = Schema::new(sid_const(), ShapeType::Timestamp);
        const fn sid_const() -> ShapeId {
            ShapeId::from_static("com.example#When", "com.example", "When")
        }
        let ts = de.read_timestamp(&TS).expect("timestamp parses");
        assert_eq!(ts.secs(), 1_576_540_098);
    }

    #[test]
    fn read_blob_decodes_base64() {
        let mut de = QueryShapeDeserializer::from_pairs(
            vec![("Data".to_string(), "aGVsbG8=".to_string())],
            false,
        );
        de.prefix = "Data".to_string();
        static B: Schema = Schema::new(
            ShapeId::from_static("com.example#Data", "com.example", "Data"),
            ShapeType::Blob,
        );
        let blob = de.read_blob(&B).expect("blob decodes");
        assert_eq!(blob.as_ref(), b"hello");
    }

    #[test]
    fn missing_scalar_is_an_error_not_a_silent_default() {
        let mut de = QueryShapeDeserializer::from_pairs(vec![], false);
        de.prefix = "Missing".to_string();
        static S: Schema = Schema::new(
            ShapeId::from_static("com.example#Missing", "com.example", "Missing"),
            ShapeType::String,
        );
        assert!(de.read_string(&S).is_err());
    }

    // ---- map key/value element names honour a modeled `@xmlName` ----

    // A struct member targeting a map whose key/value sub-members carry a
    // modeled `@xmlName` ("TagKey"/"TagValue" rather than the Smithy defaults
    // "key"/"value"). This is the shape the macro now emits via
    // `with_map_members`, so the deserializer must read entries under the
    // modeled names and the serializer must write them.
    static NAMED_MAP_KEY: Schema = Schema::new_member(
        ShapeId::from_static("com.example#Out$Tags$key", "com.example", "key"),
        ShapeType::String,
        "key",
        0,
    )
    .with_xml_name("TagKey");
    static NAMED_MAP_VALUE: Schema = Schema::new_member(
        ShapeId::from_static("com.example#Out$Tags$value", "com.example", "value"),
        ShapeType::String,
        "value",
        1,
    )
    .with_xml_name("TagValue");
    static NAMED_MAP_MEMBER: Schema = Schema::new_member(
        ShapeId::from_static("com.example#Out$Tags", "com.example", "Tags"),
        ShapeType::Map,
        "Tags",
        0,
    )
    .with_map_members(&NAMED_MAP_KEY, &NAMED_MAP_VALUE);

    #[test]
    fn read_map_honours_modeled_key_value_element_names() {
        // The request names the key/value segments by the modeled `@xmlName`,
        // not the Smithy defaults.
        let mut de = QueryShapeDeserializer::from_pairs(
            vec![
                ("Tags.entry.1.TagKey".to_string(), "region".to_string()),
                ("Tags.entry.1.TagValue".to_string(), "iad".to_string()),
            ],
            false,
        );
        de.prefix = "Tags".to_string();

        let mut entries: Vec<(String, String)> = Vec::new();
        de.read_map(&NAMED_MAP_MEMBER, &mut |k, d| {
            entries.push((k, d.read_string(&NAMED_MAP_VALUE)?));
            Ok(())
        })
        .expect("map reads under the modeled names");
        assert_eq!(entries, vec![("region".to_string(), "iad".to_string())]);
    }

    #[test]
    fn read_map_under_default_names_is_empty_when_model_renames_them() {
        // With the key/value renamed, the Smithy-default `key`/`value` segments
        // name nothing, so the entry is absent (its key segment is missing).
        let mut de = QueryShapeDeserializer::from_pairs(
            vec![
                ("Tags.entry.1.key".to_string(), "region".to_string()),
                ("Tags.entry.1.value".to_string(), "iad".to_string()),
            ],
            false,
        );
        de.prefix = "Tags".to_string();

        let err = de
            .read_map(&NAMED_MAP_MEMBER, &mut |_k, _d| Ok(()))
            .expect_err("the modeled key segment is absent, so the entry has no key");
        assert!(
            err.to_string().contains("missing its key"),
            "a renamed key means the default `key` segment names nothing, got: {err}"
        );
    }

    // A `SerializableStruct` that writes a single-entry map member, used to
    // drive `serialize_output` end to end (envelope + map body). It writes one
    // `("region","iad")` entry through `write_map`.
    struct MapOutput;
    impl SerializableStruct for MapOutput {
        fn serialize_members(
            &self,
            s: &mut dyn aws_smithy_schema::serde::ShapeSerializer,
        ) -> Result<(), aws_smithy_schema::serde::SerdeError> {
            s.write_map(&NAMED_MAP_MEMBER, &|entry| {
                entry.write_string(&NAMED_MAP_KEY, "region")?;
                entry.write_string(&NAMED_MAP_VALUE, "iad")?;
                Ok(())
            })
        }
    }

    // A placeholder output schema; `serialize_output` does not read it.
    static OUT_SCHEMA: Schema = Schema::new_struct(
        ShapeId::from_static("com.example#Out", "com.example", "Out"),
        ShapeType::Structure,
        &[&NAMED_MAP_MEMBER],
    );

    fn serialize_output_body(proto: &QueryServerProtocol, op: &str) -> String {
        let resp = proto
            .serialize_output(&MapOutput, &OUT_SCHEMA, op, 200)
            .expect("serialize_output succeeds");
        String::from_utf8(resp.into_body().bytes().expect("in-memory body").to_vec())
            .expect("UTF-8 XML body")
    }

    #[test]
    fn aws_query_output_carries_modeled_namespace_and_a_non_zero_request_id() {
        let proto =
            QueryServerProtocol::aws_query().with_namespace("https://example.com/doc/2024-01-01/");
        let body = serialize_output_body(&proto, "GetThing");

        // the modeled namespace, not a synthesised `/doc/2010-05-08/` URI.
        assert!(
            body.contains("<GetThingResponse xmlns=\"https://example.com/doc/2024-01-01/\">"),
            "awsQuery output must carry the modeled xmlns, got: {body}"
        );
        // awsQuery wraps the payload in a `…Result` element.
        assert!(
            body.contains("<GetThingResult>"),
            "awsQuery output must wrap the payload in `…Result`, got: {body}"
        );
        // a real, non-zero request id inside `<ResponseMetadata>`.
        assert!(
            body.contains("<ResponseMetadata><RequestId>"),
            "awsQuery output must carry a ResponseMetadata/RequestId, got: {body}"
        );
        assert!(
            !body.contains("<RequestId>00000000-0000-0000-0000-000000000000</RequestId>"),
            "the request id must not be the all-zero placeholder, got: {body}"
        );
        // the map renders its entries under the modeled names.
        assert!(
            body.contains("<entry><TagKey>region</TagKey><TagValue>iad</TagValue></entry>"),
            "the map must render the modeled key/value element names, got: {body}"
        );
    }

    #[test]
    fn ec2_query_output_renders_the_ec2_envelope_with_a_request_id() {
        let proto =
            QueryServerProtocol::ec2_query().with_namespace("https://example.com/doc/2024-01-01/");
        let body = serialize_output_body(&proto, "DescribeThings");

        // ec2Query uses no `…Result` wrapper and a lowercase top-level
        // `<requestId>` directly under the response element.
        assert!(
            body.contains("<DescribeThingsResponse xmlns=\"https://example.com/doc/2024-01-01/\">"),
            "ec2 output must open with the response element carrying the modeled xmlns, got: {body}"
        );
        assert!(
            !body.contains("<DescribeThingsResult>"),
            "ec2 output must NOT wrap the payload in `…Result`, got: {body}"
        );
        assert!(
            body.contains("<requestId>") && body.contains("</requestId></DescribeThingsResponse>"),
            "ec2 output must carry a lowercase top-level <requestId>, got: {body}"
        );
        assert!(
            !body.contains("<ResponseMetadata>"),
            "ec2 output must NOT use the awsQuery <ResponseMetadata> block, got: {body}"
        );
        assert!(
            !body.contains("00000000-0000-0000-0000-000000000000"),
            "the request id must not be the all-zero placeholder, got: {body}"
        );
    }

    #[test]
    fn output_without_a_modeled_namespace_emits_no_xmlns() {
        // A model with no `@xmlNamespace` gets no `xmlns` attribute — the old
        // synthesised `https://…/doc/2010-05-08/` URI is gone.
        let proto = QueryServerProtocol::aws_query();
        let body = serialize_output_body(&proto, "GetThing");
        assert!(
            body.starts_with("<GetThingResponse><GetThingResult>"),
            "with no modeled namespace the response element carries no xmlns, got: {body}"
        );
        assert!(
            !body.contains("/doc/2010-05-08/"),
            "the fabricated legacy namespace must not appear, got: {body}"
        );
    }

    #[test]
    fn generate_request_id_is_well_formed_and_distinct() {
        let a = generate_request_id();
        let b = generate_request_id();
        assert_ne!(a, b, "two calls must produce distinct ids");
        for id in [&a, &b] {
            assert_ne!(id.as_str(), "00000000-0000-0000-0000-000000000000");
            // 8-4-4-4-12 hex layout.
            let parts: Vec<&str> = id.split('-').collect();
            assert_eq!(
                parts.len(),
                5,
                "UUID has five hyphen-separated groups: {id}"
            );
            assert_eq!(
                parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
                vec![8, 4, 4, 4, 12],
                "UUID group lengths are 8-4-4-4-12: {id}"
            );
            assert!(
                id.chars().all(|c| c == '-' || c.is_ascii_hexdigit()),
                "UUID is hex + hyphens: {id}"
            );
            // Version-4 nibble.
            assert!(parts[2].starts_with('4'), "version nibble is 4: {id}");
            // RFC 4122 variant nibble (8, 9, a or b).
            assert!(
                matches!(parts[3].chars().next(), Some('8' | '9' | 'a' | 'b')),
                "variant nibble is one of 8/9/a/b: {id}"
            );
        }
    }

    // A `SerializableStruct` with no members, enough to drive `serialize_output`
    // when only the envelope is under test.
    struct UnitValue;
    impl SerializableStruct for UnitValue {
        fn serialize_members(
            &self,
            _s: &mut dyn aws_smithy_schema::serde::ShapeSerializer,
        ) -> Result<(), SerdeError> {
            Ok(())
        }
    }

    /// Defensive check on trusted values: the operation name and the modeled
    /// namespace are schema-derived, not request input, so the generator cannot
    /// produce a metacharacter in either. The escape is a cheap invariant that
    /// keeps the envelope well-formed regardless; this proves it reaches the
    /// wire, so the invariant holds even if a future model or codegen path
    /// admitted a `<`, `>`, `&` or `"`. It is not a reachable-injection
    /// regression test.
    #[test]
    fn serialize_output_defensively_escapes_the_trusted_operation_name_and_namespace() {
        // Both carry XML metacharacters a real generator would never emit.
        let proto = QueryServerProtocol::aws_query().with_namespace("ns&<\"evil");
        let body = serialize_output_body(&proto, "Op<injected>");
        // The raw metacharacters must not appear unescaped as markup.
        assert!(
            !body.contains("<injected>"),
            "operation name was interpolated unescaped: {body}"
        );
        assert!(
            !body.contains("ns&<\"evil"),
            "namespace was interpolated unescaped: {body}"
        );
        // Their escaped forms appear instead.
        assert!(
            body.contains("Op&lt;injected&gt;"),
            "operation name was not escaped: {body}"
        );
        assert!(
            body.contains("ns&amp;&lt;&quot;evil"),
            "namespace was not escaped: {body}"
        );
    }

    /// A request whose dotted-path keys nest a struct past the depth bound is
    /// refused with a clean error instead of recursing until the stack is
    /// exhausted.
    #[test]
    fn deeply_nested_query_struct_is_rejected() {
        // `Node { Node: Node }` — a struct whose one member targets the same
        // struct, so a dotted key `Node.Node.Node...` drives `read_struct` into
        // itself once per segment.
        static NODE_MEMBER: LazyLock<Schema> = LazyLock::new(|| {
            Schema::new_member(
                ShapeId::from_static("com.test#Node$Node", "com.test", "Node"),
                ShapeType::Structure,
                "Node",
                0,
            )
        });
        static NODE: LazyLock<Schema> = LazyLock::new(|| {
            let members: &'static [&'static Schema] = Box::leak(Box::new([&*NODE_MEMBER]));
            Schema::new_struct(
                ShapeId::from_static("com.test#Node", "com.test", "Node"),
                ShapeType::Structure,
                members,
            )
        });

        fn read_node(d: &mut dyn ShapeDeserializer) -> Result<(), SerdeError> {
            d.read_struct(&NODE, &mut |_member, child| read_node(child))
        }

        let depth = MAX_DEPTH + 50;
        // One key whose value sits at the bottom of a `Node.Node...` nest, so
        // every intermediate prefix is carried and the walk descends the whole
        // way.
        let key = std::iter::repeat("Node")
            .take(depth)
            .collect::<Vec<_>>()
            .join(".");
        let body = format!("{key}=x");
        let mut de = QueryShapeDeserializer::new(body.as_bytes(), false);
        let err =
            read_node(&mut de).expect_err("a request nested past the depth bound must be refused");
        assert!(
            format!("{err}").contains("nests deeper than the limit"),
            "unexpected error: {err}"
        );
    }
}
