// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! Server-side HTTP binding ser/deser.
//!
//! REST protocols (`restXml`, `restJson1`) split each operation's input and
//! output between several HTTP locations: URI labels (`@httpLabel`), query
//! string parameters (`@httpQuery`), headers (`@httpHeader`,
//! `@httpPrefixHeaders`), and the body. The Smithy schema records, for each
//! member, which location it belongs to. A protocol that wants to support
//! these bindings simply consults the schema at runtime — there is no need
//! for the proc macro to bake binding logic into generated code.
//!
//! This module provides the runtime pieces:
//!
//! - [`HttpBindingDeserializer`] — a [`ShapeDeserializer`] that wraps an HTTP
//!   request and a body deserializer. When asked to read an input struct it
//!   walks the input schema's members; for each HTTP-bound member it produces
//!   a single-value deserializer over the path / query / header value, and
//!   for body members it forwards the body codec's reads.
//! - [`HttpBindingSerializer`] — a [`ShapeSerializer`] that wraps a body
//!   serializer plus a [`HeaderCollector`]. As the output struct's
//!   `serialize_members` runs, member writes for HTTP-bound members are
//!   captured separately and only body members go to the body serializer.
//!
//! The corresponding client-side primitives live upstream in
//! [`aws_smithy_schema::schema::http_protocol::binding`]; this is the same
//! design inverted for server use.

use aws_smithy_schema::serde::{
    SerdeError, SerializableStruct, ShapeDeserializer, ShapeSerializer,
};
use aws_smithy_schema::Schema;
use aws_smithy_types::{base64, BigDecimal, BigInteger, Blob, DateTime, Document};
use std::collections::HashMap;

/// True when `schema` is the Smithy `smithy.api#Unit` shape — an operation that
/// declares no output. Such a response carries NO body: the protocol must emit
/// an empty body rather than serializing a bogus `<Unit/>` / `{}` element.
pub(crate) fn is_unit_shape(schema: &Schema) -> bool {
    let id = schema.shape_id();
    id.namespace() == "smithy.api" && id.shape_name() == "Unit"
}

// =============================================================================
// Deserialization
// =============================================================================

/// Wraps a `request` (URI + headers) and a body [`ShapeDeserializer`] and
/// presents the combined view as a single deserializer.
///
/// The deserializer drives the input schema rather than the body alone:
/// `read_struct` iterates the schema's members and routes each one to the
/// HTTP location its schema declares.
pub struct HttpBindingDeserializer<B: ShapeDeserializer> {
    /// Pre-computed `name -> value` map for `@httpLabel` members extracted by
    /// matching the request URI against the input schema's `@http.uri` pattern.
    labels: HashMap<String, String>,
    /// Pre-parsed query string: `name -> value` (single-valued; for repeated
    /// keys we currently keep the first occurrence). Lists are joined as the
    /// server-side caller expects.
    query: HashMap<String, String>,
    /// All query values keyed by name, in arrival order. Used for list-typed
    /// `@httpQuery` members that may be repeated across the query string.
    query_multi: HashMap<String, Vec<String>>,
    /// Header view of the request, lower-cased for case-insensitive lookup.
    headers: HashMap<String, String>,
    /// Multi-valued header view (e.g., comma-separated).
    headers_multi: HashMap<String, Vec<String>>,
    /// Raw header pairs preserving the original case so `@httpPrefixHeaders`
    /// can reconstruct the suffix.
    raw_headers: Vec<(String, String)>,
    /// Body codec deserializer for non-HTTP-bound members.
    body: B,
    /// Raw request-body bytes, bound directly to an `@httpPayload` member when
    /// the input schema declares one. `None` means no payload member (the body
    /// is swept by the codec as a normal member object).
    payload_bytes: Option<Vec<u8>>,
}

impl<B: ShapeDeserializer> HttpBindingDeserializer<B> {
    /// Construct a binding deserializer.
    ///
    /// `uri_pattern` is the `@http.uri` pattern (e.g., `"/things/{Id}"`) used
    /// to extract `@httpLabel` values from `path`. `path` is the request URI's
    /// path component, `query` is the raw query string (without the leading
    /// `?`), and `headers` is the list of `(name, value)` header pairs in
    /// arrival order.
    pub fn new(
        uri_pattern: Option<&str>,
        path: &str,
        query_str: &str,
        headers: Vec<(String, String)>,
        body: B,
    ) -> Self {
        let labels = match uri_pattern {
            Some(pat) => match_path_labels(pat, path),
            None => HashMap::new(),
        };

        let mut query = HashMap::new();
        let mut query_multi: HashMap<String, Vec<String>> = HashMap::new();
        if !query_str.is_empty() {
            for pair in query_str.split('&') {
                let mut it = pair.splitn(2, '=');
                let k = percent_decode_query(it.next().unwrap_or(""));
                let v = percent_decode_query(it.next().unwrap_or(""));
                query.entry(k.clone()).or_insert_with(|| v.clone());
                query_multi.entry(k).or_default().push(v);
            }
        }

        let mut headers_multi: HashMap<String, Vec<String>> = HashMap::new();
        let mut headers_first: HashMap<String, String> = HashMap::new();
        for (name, value) in &headers {
            let key = name.to_ascii_lowercase();
            headers_first
                .entry(key.clone())
                .or_insert_with(|| value.clone());
            headers_multi.entry(key).or_default().push(value.clone());
        }

        Self {
            labels,
            query,
            query_multi,
            headers: headers_first,
            headers_multi,
            raw_headers: headers,
            body,
            payload_bytes: None,
        }
    }
}

impl<B: ShapeDeserializer> HttpBindingDeserializer<B> {
    /// Provide the raw request-body bytes so that an `@httpPayload`-bound member
    /// is filled directly from them (rather than from the codec body sweep).
    ///
    /// Additive: protocols with no payload member simply never call this, and
    /// the body is swept by the codec exactly as before.
    pub fn with_payload_bytes(mut self, bytes: Vec<u8>) -> Self {
        self.payload_bytes = Some(bytes);
        self
    }
}

/// Percent-decode a path segment (literal `+` is preserved per RFC 3986).
fn percent_decode_path(s: &str) -> String {
    percent_decode_inner(s, false)
}

/// Percent-decode a query component (`+` decodes to space per application/x-www-form-urlencoded).
fn percent_decode_query(s: &str) -> String {
    percent_decode_inner(s, true)
}

fn percent_decode_inner(s: &str, plus_as_space: bool) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push(hi << 4 | lo);
                i += 3;
                continue;
            }
        } else if plus_as_space && bytes[i] == b'+' {
            out.push(b' ');
            i += 1;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Match a request path against a URI pattern containing `{Label}` and
/// `{Label+}` placeholders, returning a map of label name -> decoded value.
fn match_path_labels(pattern: &str, path: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    // A route pattern may carry static query literals (e.g. `/{Bucket}?location`
    // or `/{Bucket}?list-type=2`) used only to disambiguate overloaded routes.
    // Only the path portion binds `@httpLabel` members, so drop the query first.
    let pattern = match pattern.find('?') {
        Some(i) => &pattern[..i],
        None => pattern,
    };
    let pat = pattern.trim_matches('/');
    let pth = path.trim_matches('/');
    let pat_segs: Vec<&str> = if pat.is_empty() {
        Vec::new()
    } else {
        pat.split('/').collect()
    };
    let path_segs: Vec<&str> = if pth.is_empty() {
        Vec::new()
    } else {
        pth.split('/').collect()
    };

    let mut pi = 0usize;
    let mut si = 0usize;
    while pi < pat_segs.len() {
        let seg = pat_segs[pi];
        if let Some(stripped) = seg.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            let (name_raw, greedy) = match stripped.strip_suffix('+') {
                Some(n) => (n, true),
                None => (stripped, false),
            };
            let name = name_raw.to_string();
            if greedy {
                // Consume the rest of the path segments, joined with '/'.
                let rest = path_segs[si..].join("/");
                out.insert(name, percent_decode_path(&rest));
                si = path_segs.len();
                pi += 1;
            } else {
                if si >= path_segs.len() {
                    return out; // mismatch — but caller will likely fail later
                }
                out.insert(name, percent_decode_path(path_segs[si]));
                si += 1;
                pi += 1;
            }
        } else {
            if si >= path_segs.len() || path_segs[si] != seg {
                return out;
            }
            si += 1;
            pi += 1;
        }
    }
    out
}

impl<B: ShapeDeserializer> ShapeDeserializer for HttpBindingDeserializer<B> {
    fn read_struct(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(&Schema, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        // First, drive HTTP-bound members directly off the schema. For each
        // member with an HTTP location, synthesize a single-value
        // deserializer that returns the bound value when the consumer asks
        // for it.
        let mut has_payload = false;
        for member in schema.members() {
            if let Some(label) = member.http_label() {
                let _ = label;
                let name = match member.member_name() {
                    Some(n) => n,
                    None => continue,
                };
                let value = match self.labels.get(name) {
                    Some(v) => v.clone(),
                    None => continue,
                };
                let mut sv = StringValueDeserializer::new(value);
                consumer(member, &mut sv)?;
                continue;
            }
            if let Some(q) = member.http_query() {
                let qname = q.value();
                // Collect possibly-multiple values to support list members.
                let values = self.query_multi.get(qname).cloned().unwrap_or_else(|| {
                    self.query
                        .get(qname)
                        .map(|v| vec![v.clone()])
                        .unwrap_or_default()
                });
                if values.is_empty() {
                    continue;
                }
                if matches!(member.shape_type(), aws_smithy_schema::ShapeType::List) {
                    let mut lv = ListValueDeserializer::new(values);
                    consumer(member, &mut lv)?;
                } else {
                    let v = values.into_iter().next().unwrap();
                    let mut sv = StringValueDeserializer::new(v);
                    consumer(member, &mut sv)?;
                }
                continue;
            }
            if let Some(h) = member.http_header() {
                let hname = h.value().to_ascii_lowercase();
                if matches!(member.shape_type(), aws_smithy_schema::ShapeType::List) {
                    // List headers are comma-separated per RFC 7230, but
                    // multiple header lines are also allowed. Concatenate.
                    let mut all: Vec<String> = Vec::new();
                    if let Some(vals) = self.headers_multi.get(&hname) {
                        for line in vals {
                            for piece in split_header_list(line) {
                                all.push(piece);
                            }
                        }
                    }
                    if all.is_empty() {
                        continue;
                    }
                    let mut lv = ListValueDeserializer::new(all);
                    consumer(member, &mut lv)?;
                } else if let Some(value) = self.headers.get(&hname) {
                    let mut sv = StringValueDeserializer::new(value.clone());
                    consumer(member, &mut sv)?;
                }
                continue;
            }
            if let Some(p) = member.http_prefix_headers() {
                let prefix = p.value();
                let prefix_lower = prefix.to_ascii_lowercase();
                let mut entries: Vec<(String, String)> = Vec::new();
                for (n, v) in &self.raw_headers {
                    let lower = n.to_ascii_lowercase();
                    if lower.starts_with(&prefix_lower) {
                        let suffix = &n[prefix.len().min(n.len())..];
                        entries.push((suffix.to_string(), v.clone()));
                    }
                }
                if entries.is_empty() {
                    continue;
                }
                let mut mv = MapValueDeserializer::new(entries);
                consumer(member, &mut mv)?;
                continue;
            }
            if member.http_query_params().is_some() {
                let entries: Vec<(String, String)> = self
                    .query_multi
                    .iter()
                    .flat_map(|(k, vs)| vs.iter().map(move |v| (k.clone(), v.clone())))
                    .collect();
                if entries.is_empty() {
                    continue;
                }
                let mut mv = MapValueDeserializer::new(entries);
                consumer(member, &mut mv)?;
                continue;
            }
            if member.http_payload().is_some() {
                // `@httpPayload`: the body IS this member's value, so the codec
                // must not sweep it as a normal member object. How the body maps
                // to the member depends on the member's target shape kind:
                //
                //  * a structured target (`structure`/`union`/`list`/`map`/
                //    `document`) means the whole body is that shape encoded in
                //    the protocol's body format (REST-XML: XML; REST-JSON:
                //    JSON). Drive the real body codec (`self.body`) over the
                //    bytes so it parses into the target shape.
                //  * a scalar target (`blob`/`string`) binds the raw body
                //    verbatim — no base64, no codec parse — via
                //    `PayloadValueDeserializer`.
                has_payload = true;
                match member.shape_type() {
                    aws_smithy_schema::ShapeType::Structure
                    | aws_smithy_schema::ShapeType::Union
                    | aws_smithy_schema::ShapeType::List
                    | aws_smithy_schema::ShapeType::Map
                    | aws_smithy_schema::ShapeType::Document => {
                        consumer(member, &mut self.body)?;
                    }
                    _ => {
                        let bytes = self.payload_bytes.clone().unwrap_or_default();
                        let mut pv = PayloadValueDeserializer::new(bytes);
                        consumer(member, &mut pv)?;
                    }
                }
                continue;
            }
            // Body member — handled by the body sweep below.
        }

        // Then drive the body codec for body members. The body deserializer
        // walks its own document and only sees members that are actually
        // present; HTTP-bound members are absent from the body so they fall
        // through. Each body member is forwarded to the consumer.
        //
        // Skipped entirely when an `@httpPayload` member claimed the body: the
        // raw bytes were already bound above and are not a member object.
        if !has_payload {
            self.body.read_struct(schema, &mut |m, d| consumer(m, d))?;
        }
        Ok(())
    }

    // The remaining methods only ever fire when the body deserializer needs
    // them (e.g., for list/map elements). Forward them so nested reads work.

    fn read_list(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(&mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.body.read_list(schema, consumer)
    }
    fn read_map(
        &mut self,
        schema: &Schema,
        consumer: &mut dyn FnMut(String, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.body.read_map(schema, consumer)
    }
    fn read_boolean(&mut self, schema: &Schema) -> Result<bool, SerdeError> {
        self.body.read_boolean(schema)
    }
    fn read_byte(&mut self, schema: &Schema) -> Result<i8, SerdeError> {
        self.body.read_byte(schema)
    }
    fn read_short(&mut self, schema: &Schema) -> Result<i16, SerdeError> {
        self.body.read_short(schema)
    }
    fn read_integer(&mut self, schema: &Schema) -> Result<i32, SerdeError> {
        self.body.read_integer(schema)
    }
    fn read_long(&mut self, schema: &Schema) -> Result<i64, SerdeError> {
        self.body.read_long(schema)
    }
    fn read_float(&mut self, schema: &Schema) -> Result<f32, SerdeError> {
        self.body.read_float(schema)
    }
    fn read_double(&mut self, schema: &Schema) -> Result<f64, SerdeError> {
        self.body.read_double(schema)
    }
    fn read_big_integer(&mut self, schema: &Schema) -> Result<BigInteger, SerdeError> {
        self.body.read_big_integer(schema)
    }
    fn read_big_decimal(&mut self, schema: &Schema) -> Result<BigDecimal, SerdeError> {
        self.body.read_big_decimal(schema)
    }
    fn read_string(&mut self, schema: &Schema) -> Result<String, SerdeError> {
        self.body.read_string(schema)
    }
    fn read_blob(&mut self, schema: &Schema) -> Result<Blob, SerdeError> {
        self.body.read_blob(schema)
    }
    fn read_timestamp(&mut self, schema: &Schema) -> Result<DateTime, SerdeError> {
        self.body.read_timestamp(schema)
    }
    fn read_document(&mut self, schema: &Schema) -> Result<Document, SerdeError> {
        self.body.read_document(schema)
    }
    fn is_null(&self) -> bool {
        self.body.is_null()
    }
    fn container_size(&self) -> Option<usize> {
        self.body.container_size()
    }
}

/// Split a header value such as `"a, b, c"` into its components, respecting
/// quoted strings (RFC 7230 list rule).
fn split_header_list(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if !in_quotes => in_quotes = true,
            '"' if in_quotes => in_quotes = false,
            '\\' if in_quotes => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            ',' if !in_quotes => {
                out.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    let last = cur.trim().to_string();
    if !last.is_empty() || !out.is_empty() {
        out.push(last);
    }
    out
}

// =============================================================================
// Single-value deserializer (string-formatted scalars from path/query/header)
// =============================================================================

/// Returns a single string-formatted scalar via `read_*`. The macro-generated
/// `deserialize` body calls one of `read_string` / `read_integer` / … on the
/// member's own deserializer; this struct parses the held string accordingly.
struct StringValueDeserializer {
    value: String,
    consumed: bool,
}

impl StringValueDeserializer {
    fn new(value: String) -> Self {
        Self {
            value,
            consumed: false,
        }
    }
    fn take(&mut self) -> Result<String, SerdeError> {
        if self.consumed {
            return Err(SerdeError::custom(
                "StringValueDeserializer already consumed",
            ));
        }
        self.consumed = true;
        Ok(std::mem::take(&mut self.value))
    }
    fn parse<T: std::str::FromStr>(&mut self, kind: &'static str) -> Result<T, SerdeError>
    where
        T::Err: std::fmt::Display,
    {
        let s = self.take()?;
        s.parse::<T>()
            .map_err(|e| SerdeError::custom(format!("invalid {kind} `{s}`: {e}")))
    }
}

impl ShapeDeserializer for StringValueDeserializer {
    fn read_struct(
        &mut self,
        _schema: &Schema,
        _consumer: &mut dyn FnMut(&Schema, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        Err(SerdeError::custom(
            "cannot read a struct from an HTTP-bound string value",
        ))
    }
    fn read_list(
        &mut self,
        _schema: &Schema,
        _consumer: &mut dyn FnMut(&mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        Err(SerdeError::custom(
            "cannot read a list from a single HTTP-bound string value",
        ))
    }
    fn read_map(
        &mut self,
        _schema: &Schema,
        _consumer: &mut dyn FnMut(String, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        Err(SerdeError::custom(
            "cannot read a map from a single HTTP-bound string value",
        ))
    }
    fn read_boolean(&mut self, _schema: &Schema) -> Result<bool, SerdeError> {
        let s = self.take()?;
        match s.as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(SerdeError::custom(format!("invalid bool `{s}`"))),
        }
    }
    fn read_byte(&mut self, _schema: &Schema) -> Result<i8, SerdeError> {
        self.parse("i8")
    }
    fn read_short(&mut self, _schema: &Schema) -> Result<i16, SerdeError> {
        self.parse("i16")
    }
    fn read_integer(&mut self, _schema: &Schema) -> Result<i32, SerdeError> {
        self.parse("i32")
    }
    fn read_long(&mut self, _schema: &Schema) -> Result<i64, SerdeError> {
        self.parse("i64")
    }
    fn read_float(&mut self, _schema: &Schema) -> Result<f32, SerdeError> {
        self.parse("f32")
    }
    fn read_double(&mut self, _schema: &Schema) -> Result<f64, SerdeError> {
        self.parse("f64")
    }
    fn read_big_integer(&mut self, _schema: &Schema) -> Result<BigInteger, SerdeError> {
        let s = self.take()?;
        s.parse::<BigInteger>()
            .map_err(|e| SerdeError::custom(format!("invalid big_integer `{s}`: {e}")))
    }
    fn read_big_decimal(&mut self, _schema: &Schema) -> Result<BigDecimal, SerdeError> {
        let s = self.take()?;
        s.parse::<BigDecimal>()
            .map_err(|e| SerdeError::custom(format!("invalid big_decimal `{s}`: {e}")))
    }
    fn read_string(&mut self, _schema: &Schema) -> Result<String, SerdeError> {
        self.take()
    }
    fn read_blob(&mut self, _schema: &Schema) -> Result<Blob, SerdeError> {
        let s = self.take()?;
        let bytes = base64::decode(&s)
            .map_err(|e| SerdeError::custom(format!("invalid base64 blob: {e}")))?;
        Ok(Blob::new(bytes))
    }
    fn read_timestamp(&mut self, schema: &Schema) -> Result<DateTime, SerdeError> {
        let s = self.take()?;
        let format = match schema.timestamp_format().map(|t| t.format()) {
            Some(aws_smithy_schema::traits::TimestampFormat::EpochSeconds) => {
                aws_smithy_types::date_time::Format::EpochSeconds
            }
            Some(aws_smithy_schema::traits::TimestampFormat::HttpDate) => {
                aws_smithy_types::date_time::Format::HttpDate
            }
            Some(aws_smithy_schema::traits::TimestampFormat::DateTime) | None => {
                aws_smithy_types::date_time::Format::DateTime
            }
        };
        DateTime::from_str(&s, format)
            .map_err(|e| SerdeError::custom(format!("invalid timestamp `{s}`: {e}")))
    }
    fn read_document(&mut self, _schema: &Schema) -> Result<Document, SerdeError> {
        Err(SerdeError::custom(
            "cannot decode a Document from an HTTP-bound string value",
        ))
    }
    fn is_null(&self) -> bool {
        self.consumed
    }
    fn container_size(&self) -> Option<usize> {
        None
    }
}

// =============================================================================
// Payload value deserializer (`@httpPayload`: the raw request body IS the value)
// =============================================================================

/// Yields the raw request-body bytes as an `@httpPayload`-bound member's value.
///
/// Unlike [`StringValueDeserializer`], the bytes are the *literal* body — a
/// `Blob` payload is the raw bytes (no base64), and a `String` payload is the
/// raw UTF-8 body. Scalar/aggregate reads are rejected: `@httpPayload` only
/// targets a blob or string member on this raw-body path.
struct PayloadValueDeserializer {
    bytes: Vec<u8>,
    consumed: bool,
}

impl PayloadValueDeserializer {
    fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            consumed: false,
        }
    }
    fn take(&mut self) -> Result<Vec<u8>, SerdeError> {
        if self.consumed {
            return Err(SerdeError::custom(
                "PayloadValueDeserializer already consumed",
            ));
        }
        self.consumed = true;
        Ok(std::mem::take(&mut self.bytes))
    }
}

impl ShapeDeserializer for PayloadValueDeserializer {
    fn read_struct(
        &mut self,
        _schema: &Schema,
        _consumer: &mut dyn FnMut(&Schema, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        Err(SerdeError::custom(
            "cannot read a struct from a raw @httpPayload body",
        ))
    }
    fn read_list(
        &mut self,
        _schema: &Schema,
        _consumer: &mut dyn FnMut(&mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        Err(SerdeError::custom(
            "cannot read a list from a raw @httpPayload body",
        ))
    }
    fn read_map(
        &mut self,
        _schema: &Schema,
        _consumer: &mut dyn FnMut(String, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        Err(SerdeError::custom(
            "cannot read a map from a raw @httpPayload body",
        ))
    }
    fn read_boolean(&mut self, _schema: &Schema) -> Result<bool, SerdeError> {
        Err(SerdeError::custom("@httpPayload is not a boolean"))
    }
    fn read_byte(&mut self, _schema: &Schema) -> Result<i8, SerdeError> {
        Err(SerdeError::custom("@httpPayload is not a byte"))
    }
    fn read_short(&mut self, _schema: &Schema) -> Result<i16, SerdeError> {
        Err(SerdeError::custom("@httpPayload is not a short"))
    }
    fn read_integer(&mut self, _schema: &Schema) -> Result<i32, SerdeError> {
        Err(SerdeError::custom("@httpPayload is not an integer"))
    }
    fn read_long(&mut self, _schema: &Schema) -> Result<i64, SerdeError> {
        Err(SerdeError::custom("@httpPayload is not a long"))
    }
    fn read_float(&mut self, _schema: &Schema) -> Result<f32, SerdeError> {
        Err(SerdeError::custom("@httpPayload is not a float"))
    }
    fn read_double(&mut self, _schema: &Schema) -> Result<f64, SerdeError> {
        Err(SerdeError::custom("@httpPayload is not a double"))
    }
    fn read_big_integer(&mut self, _schema: &Schema) -> Result<BigInteger, SerdeError> {
        Err(SerdeError::custom("@httpPayload is not a big_integer"))
    }
    fn read_big_decimal(&mut self, _schema: &Schema) -> Result<BigDecimal, SerdeError> {
        Err(SerdeError::custom("@httpPayload is not a big_decimal"))
    }
    fn read_string(&mut self, _schema: &Schema) -> Result<String, SerdeError> {
        let bytes = self.take()?;
        String::from_utf8(bytes)
            .map_err(|e| SerdeError::custom(format!("@httpPayload body is not valid UTF-8: {e}")))
    }
    fn read_blob(&mut self, _schema: &Schema) -> Result<Blob, SerdeError> {
        Ok(Blob::new(self.take()?))
    }
    fn read_timestamp(&mut self, _schema: &Schema) -> Result<DateTime, SerdeError> {
        Err(SerdeError::custom("@httpPayload is not a timestamp"))
    }
    fn read_document(&mut self, _schema: &Schema) -> Result<Document, SerdeError> {
        Err(SerdeError::custom(
            "cannot decode a Document from a raw @httpPayload body",
        ))
    }
    fn is_null(&self) -> bool {
        self.consumed
    }
    fn container_size(&self) -> Option<usize> {
        None
    }
}

// =============================================================================
// List value deserializer (multiple HTTP-bound string values, e.g. headers/query lists)
// =============================================================================

struct ListValueDeserializer {
    values: Vec<String>,
}

impl ListValueDeserializer {
    fn new(values: Vec<String>) -> Self {
        Self { values }
    }
}

impl ShapeDeserializer for ListValueDeserializer {
    fn read_list(
        &mut self,
        _schema: &Schema,
        consumer: &mut dyn FnMut(&mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        for v in std::mem::take(&mut self.values) {
            let mut sv = StringValueDeserializer::new(v);
            consumer(&mut sv)?;
        }
        Ok(())
    }
    fn read_struct(
        &mut self,
        _schema: &Schema,
        _consumer: &mut dyn FnMut(&Schema, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        Err(SerdeError::custom("ListValueDeserializer.read_struct"))
    }
    fn read_map(
        &mut self,
        _schema: &Schema,
        _consumer: &mut dyn FnMut(String, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        Err(SerdeError::custom("ListValueDeserializer.read_map"))
    }
    fn read_boolean(&mut self, _: &Schema) -> Result<bool, SerdeError> {
        Err(SerdeError::custom("ListValueDeserializer.read_boolean"))
    }
    fn read_byte(&mut self, _: &Schema) -> Result<i8, SerdeError> {
        Err(SerdeError::custom("ListValueDeserializer.read_byte"))
    }
    fn read_short(&mut self, _: &Schema) -> Result<i16, SerdeError> {
        Err(SerdeError::custom("ListValueDeserializer.read_short"))
    }
    fn read_integer(&mut self, _: &Schema) -> Result<i32, SerdeError> {
        Err(SerdeError::custom("ListValueDeserializer.read_integer"))
    }
    fn read_long(&mut self, _: &Schema) -> Result<i64, SerdeError> {
        Err(SerdeError::custom("ListValueDeserializer.read_long"))
    }
    fn read_float(&mut self, _: &Schema) -> Result<f32, SerdeError> {
        Err(SerdeError::custom("ListValueDeserializer.read_float"))
    }
    fn read_double(&mut self, _: &Schema) -> Result<f64, SerdeError> {
        Err(SerdeError::custom("ListValueDeserializer.read_double"))
    }
    fn read_big_integer(&mut self, _: &Schema) -> Result<BigInteger, SerdeError> {
        Err(SerdeError::custom("ListValueDeserializer.read_big_integer"))
    }
    fn read_big_decimal(&mut self, _: &Schema) -> Result<BigDecimal, SerdeError> {
        Err(SerdeError::custom("ListValueDeserializer.read_big_decimal"))
    }
    fn read_string(&mut self, _: &Schema) -> Result<String, SerdeError> {
        Err(SerdeError::custom("ListValueDeserializer.read_string"))
    }
    fn read_blob(&mut self, _: &Schema) -> Result<Blob, SerdeError> {
        Err(SerdeError::custom("ListValueDeserializer.read_blob"))
    }
    fn read_timestamp(&mut self, _: &Schema) -> Result<DateTime, SerdeError> {
        Err(SerdeError::custom("ListValueDeserializer.read_timestamp"))
    }
    fn read_document(&mut self, _: &Schema) -> Result<Document, SerdeError> {
        Err(SerdeError::custom("ListValueDeserializer.read_document"))
    }
    fn is_null(&self) -> bool {
        self.values.is_empty()
    }
    fn container_size(&self) -> Option<usize> {
        Some(self.values.len())
    }
}

// =============================================================================
// Map value deserializer (httpPrefixHeaders / httpQueryParams)
// =============================================================================

struct MapValueDeserializer {
    entries: Vec<(String, String)>,
}

impl MapValueDeserializer {
    fn new(entries: Vec<(String, String)>) -> Self {
        Self { entries }
    }
}

impl ShapeDeserializer for MapValueDeserializer {
    fn read_map(
        &mut self,
        _schema: &Schema,
        consumer: &mut dyn FnMut(String, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        for (k, v) in std::mem::take(&mut self.entries) {
            let mut sv = StringValueDeserializer::new(v);
            consumer(k, &mut sv)?;
        }
        Ok(())
    }
    fn read_struct(
        &mut self,
        _schema: &Schema,
        _consumer: &mut dyn FnMut(&Schema, &mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        Err(SerdeError::custom("MapValueDeserializer.read_struct"))
    }
    fn read_list(
        &mut self,
        _schema: &Schema,
        _consumer: &mut dyn FnMut(&mut dyn ShapeDeserializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        Err(SerdeError::custom("MapValueDeserializer.read_list"))
    }
    fn read_boolean(&mut self, _: &Schema) -> Result<bool, SerdeError> {
        Err(SerdeError::custom("MapValueDeserializer.read_boolean"))
    }
    fn read_byte(&mut self, _: &Schema) -> Result<i8, SerdeError> {
        Err(SerdeError::custom("MapValueDeserializer.read_byte"))
    }
    fn read_short(&mut self, _: &Schema) -> Result<i16, SerdeError> {
        Err(SerdeError::custom("MapValueDeserializer.read_short"))
    }
    fn read_integer(&mut self, _: &Schema) -> Result<i32, SerdeError> {
        Err(SerdeError::custom("MapValueDeserializer.read_integer"))
    }
    fn read_long(&mut self, _: &Schema) -> Result<i64, SerdeError> {
        Err(SerdeError::custom("MapValueDeserializer.read_long"))
    }
    fn read_float(&mut self, _: &Schema) -> Result<f32, SerdeError> {
        Err(SerdeError::custom("MapValueDeserializer.read_float"))
    }
    fn read_double(&mut self, _: &Schema) -> Result<f64, SerdeError> {
        Err(SerdeError::custom("MapValueDeserializer.read_double"))
    }
    fn read_big_integer(&mut self, _: &Schema) -> Result<BigInteger, SerdeError> {
        Err(SerdeError::custom("MapValueDeserializer.read_big_integer"))
    }
    fn read_big_decimal(&mut self, _: &Schema) -> Result<BigDecimal, SerdeError> {
        Err(SerdeError::custom("MapValueDeserializer.read_big_decimal"))
    }
    fn read_string(&mut self, _: &Schema) -> Result<String, SerdeError> {
        Err(SerdeError::custom("MapValueDeserializer.read_string"))
    }
    fn read_blob(&mut self, _: &Schema) -> Result<Blob, SerdeError> {
        Err(SerdeError::custom("MapValueDeserializer.read_blob"))
    }
    fn read_timestamp(&mut self, _: &Schema) -> Result<DateTime, SerdeError> {
        Err(SerdeError::custom("MapValueDeserializer.read_timestamp"))
    }
    fn read_document(&mut self, _: &Schema) -> Result<Document, SerdeError> {
        Err(SerdeError::custom("MapValueDeserializer.read_document"))
    }
    fn is_null(&self) -> bool {
        self.entries.is_empty()
    }
    fn container_size(&self) -> Option<usize> {
        Some(self.entries.len())
    }
}

// =============================================================================
// Serialization
// =============================================================================

/// Captures the side-effects of serializing an output struct: HTTP headers
/// and an optional override of the response body bytes for `@httpPayload`.
#[derive(Default, Debug)]
pub struct HttpBindingOutput {
    /// Headers to attach to the response, in arrival order.
    pub headers: Vec<(String, String)>,
    /// Optional `@httpResponseCode` override.
    pub status_code: Option<u16>,
    /// Raw bytes of the `@httpPayload`-bound member plus the content-type the
    /// member's target type maps to, if any. When present, the protocol emits
    /// these bytes as the *entire* response body (with this content-type)
    /// instead of the codec-serialized member object. Per the Smithy spec a
    /// blob payload is `application/octet-stream` and a string payload is
    /// `text/plain`.
    pub body_override: Option<(Vec<u8>, &'static str)>,
}

/// Wraps a body [`ShapeSerializer`] and intercepts member writes for HTTP-bound
/// members (`@httpHeader`, `@httpResponseCode`). Anything not HTTP-bound goes
/// to the body serializer.
///
/// Designed for the **server output** path: given an output struct and its
/// schema, run `serialize_members` against this serializer, then call
/// [`HttpBindingSerializer::finish`] to recover collected headers /
/// status code and get the body serializer back.
pub struct HttpBindingSerializer<S: ShapeSerializer> {
    body: S,
    output: HttpBindingOutput,
}

impl<S: ShapeSerializer> HttpBindingSerializer<S> {
    pub fn new(body: S) -> Self {
        Self {
            body,
            output: HttpBindingOutput::default(),
        }
    }

    /// Consume the binder and return the HTTP binding output plus the body
    /// serializer (for finishing/extracting bytes).
    pub fn finish(self) -> (HttpBindingOutput, S) {
        (self.output, self.body)
    }

    fn record_header(&mut self, schema: &Schema, value: String) {
        if let Some(h) = schema.http_header() {
            self.output.headers.push((h.value().to_string(), value));
        }
    }
}

impl<S: ShapeSerializer> ShapeSerializer for HttpBindingSerializer<S> {
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        // For nested structs (not the top-level output), just forward to body.
        self.body.write_struct(schema, value)
    }

    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        if let Some(h) = schema.http_header() {
            let mut collector = StringCollector::default();
            write_elements(&mut collector)?;
            let joined = collector
                .values
                .iter()
                .map(|s| {
                    if s.contains(',') || s.contains('"') {
                        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
                    } else {
                        s.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            self.output.headers.push((h.value().to_string(), joined));
            return Ok(());
        }
        self.body.write_list(schema, write_elements)
    }

    fn write_map(
        &mut self,
        schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        if let Some(p) = schema.http_prefix_headers() {
            let mut collector = MapStringCollector::default();
            write_entries(&mut collector)?;
            for (k, v) in collector.entries {
                self.output.headers.push((format!("{}{}", p.value(), k), v));
            }
            return Ok(());
        }
        self.body.write_map(schema, write_entries)
    }

    fn write_boolean(&mut self, schema: &Schema, value: bool) -> Result<(), SerdeError> {
        if schema.http_header().is_some() {
            self.record_header(schema, value.to_string());
            return Ok(());
        }
        self.body.write_boolean(schema, value)
    }
    fn write_byte(&mut self, schema: &Schema, value: i8) -> Result<(), SerdeError> {
        if schema.http_header().is_some() {
            self.record_header(schema, value.to_string());
            return Ok(());
        }
        self.body.write_byte(schema, value)
    }
    fn write_short(&mut self, schema: &Schema, value: i16) -> Result<(), SerdeError> {
        if schema.http_header().is_some() {
            self.record_header(schema, value.to_string());
            return Ok(());
        }
        self.body.write_short(schema, value)
    }
    fn write_integer(&mut self, schema: &Schema, value: i32) -> Result<(), SerdeError> {
        if schema.http_header().is_some() {
            self.record_header(schema, value.to_string());
            return Ok(());
        }
        if schema.http_response_code().is_some() {
            self.output.status_code = Some(value as u16);
            return Ok(());
        }
        self.body.write_integer(schema, value)
    }
    fn write_long(&mut self, schema: &Schema, value: i64) -> Result<(), SerdeError> {
        if schema.http_header().is_some() {
            self.record_header(schema, value.to_string());
            return Ok(());
        }
        self.body.write_long(schema, value)
    }
    fn write_float(&mut self, schema: &Schema, value: f32) -> Result<(), SerdeError> {
        if schema.http_header().is_some() {
            self.record_header(schema, value.to_string());
            return Ok(());
        }
        self.body.write_float(schema, value)
    }
    fn write_double(&mut self, schema: &Schema, value: f64) -> Result<(), SerdeError> {
        if schema.http_header().is_some() {
            self.record_header(schema, value.to_string());
            return Ok(());
        }
        self.body.write_double(schema, value)
    }
    fn write_big_integer(&mut self, schema: &Schema, value: &BigInteger) -> Result<(), SerdeError> {
        if schema.http_header().is_some() {
            self.record_header(schema, value.as_ref().to_string());
            return Ok(());
        }
        self.body.write_big_integer(schema, value)
    }
    fn write_big_decimal(&mut self, schema: &Schema, value: &BigDecimal) -> Result<(), SerdeError> {
        if schema.http_header().is_some() {
            self.record_header(schema, value.as_ref().to_string());
            return Ok(());
        }
        self.body.write_big_decimal(schema, value)
    }
    fn write_string(&mut self, schema: &Schema, value: &str) -> Result<(), SerdeError> {
        if schema.http_header().is_some() {
            self.record_header(schema, value.to_string());
            return Ok(());
        }
        if schema.http_payload().is_some() {
            self.output.body_override = Some((value.as_bytes().to_vec(), "text/plain"));
            return Ok(());
        }
        self.body.write_string(schema, value)
    }
    fn write_blob(&mut self, schema: &Schema, value: &[u8]) -> Result<(), SerdeError> {
        if schema.http_header().is_some() {
            self.record_header(schema, base64::encode(value.as_ref()));
            return Ok(());
        }
        if schema.http_payload().is_some() {
            self.output.body_override = Some((value.as_ref().to_vec(), "application/octet-stream"));
            return Ok(());
        }
        self.body.write_blob(schema, value)
    }
    fn write_timestamp(&mut self, schema: &Schema, value: &DateTime) -> Result<(), SerdeError> {
        if schema.http_header().is_some() {
            let format = match schema.timestamp_format().map(|t| t.format()) {
                Some(aws_smithy_schema::traits::TimestampFormat::EpochSeconds) => {
                    aws_smithy_types::date_time::Format::EpochSeconds
                }
                Some(aws_smithy_schema::traits::TimestampFormat::DateTime) => {
                    aws_smithy_types::date_time::Format::DateTime
                }
                _ => aws_smithy_types::date_time::Format::HttpDate,
            };
            let s = value
                .fmt(format)
                .map_err(|e| SerdeError::custom(format!("timestamp format error: {e}")))?;
            self.record_header(schema, s);
            return Ok(());
        }
        self.body.write_timestamp(schema, value)
    }
    fn write_document(&mut self, schema: &Schema, value: &Document) -> Result<(), SerdeError> {
        self.body.write_document(schema, value)
    }
    fn write_null(&mut self, schema: &Schema) -> Result<(), SerdeError> {
        self.body.write_null(schema)
    }
}

/// Collects scalar element values written via `ShapeSerializer` (used for
/// list-typed `@httpHeader` / `@httpQuery`).
#[derive(Default)]
struct StringCollector {
    values: Vec<String>,
}

impl ShapeSerializer for StringCollector {
    fn write_string(&mut self, _: &Schema, value: &str) -> Result<(), SerdeError> {
        self.values.push(value.to_string());
        Ok(())
    }
    fn write_boolean(&mut self, _: &Schema, value: bool) -> Result<(), SerdeError> {
        self.values.push(value.to_string());
        Ok(())
    }
    fn write_byte(&mut self, _: &Schema, value: i8) -> Result<(), SerdeError> {
        self.values.push(value.to_string());
        Ok(())
    }
    fn write_short(&mut self, _: &Schema, value: i16) -> Result<(), SerdeError> {
        self.values.push(value.to_string());
        Ok(())
    }
    fn write_integer(&mut self, _: &Schema, value: i32) -> Result<(), SerdeError> {
        self.values.push(value.to_string());
        Ok(())
    }
    fn write_long(&mut self, _: &Schema, value: i64) -> Result<(), SerdeError> {
        self.values.push(value.to_string());
        Ok(())
    }
    fn write_float(&mut self, _: &Schema, value: f32) -> Result<(), SerdeError> {
        self.values.push(value.to_string());
        Ok(())
    }
    fn write_double(&mut self, _: &Schema, value: f64) -> Result<(), SerdeError> {
        self.values.push(value.to_string());
        Ok(())
    }
    fn write_big_integer(&mut self, _: &Schema, v: &BigInteger) -> Result<(), SerdeError> {
        self.values.push(v.as_ref().to_string());
        Ok(())
    }
    fn write_big_decimal(&mut self, _: &Schema, v: &BigDecimal) -> Result<(), SerdeError> {
        self.values.push(v.as_ref().to_string());
        Ok(())
    }
    fn write_blob(&mut self, _: &Schema, v: &[u8]) -> Result<(), SerdeError> {
        self.values.push(base64::encode(v.as_ref()));
        Ok(())
    }
    fn write_timestamp(&mut self, _: &Schema, v: &DateTime) -> Result<(), SerdeError> {
        let s = v
            .fmt(aws_smithy_types::date_time::Format::DateTime)
            .map_err(|e| SerdeError::custom(format!("timestamp format error: {e}")))?;
        self.values.push(s);
        Ok(())
    }
    fn write_document(&mut self, _: &Schema, _: &Document) -> Result<(), SerdeError> {
        Ok(())
    }
    fn write_null(&mut self, _: &Schema) -> Result<(), SerdeError> {
        Ok(())
    }
    fn write_struct(&mut self, _: &Schema, _: &dyn SerializableStruct) -> Result<(), SerdeError> {
        Ok(())
    }
    fn write_list(
        &mut self,
        _: &Schema,
        _: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        Ok(())
    }
    fn write_map(
        &mut self,
        _: &Schema,
        _: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        Ok(())
    }
}

/// Collects map key/value pairs written via `ShapeSerializer` (used for
/// `@httpPrefixHeaders`).
#[derive(Default)]
struct MapStringCollector {
    entries: Vec<(String, String)>,
    pending_key: Option<String>,
}

impl ShapeSerializer for MapStringCollector {
    fn write_string(&mut self, _: &Schema, value: &str) -> Result<(), SerdeError> {
        if let Some(k) = self.pending_key.take() {
            self.entries.push((k, value.to_string()));
        } else {
            self.pending_key = Some(value.to_string());
        }
        Ok(())
    }
    fn write_boolean(&mut self, _: &Schema, _: bool) -> Result<(), SerdeError> {
        Ok(())
    }
    fn write_byte(&mut self, _: &Schema, _: i8) -> Result<(), SerdeError> {
        Ok(())
    }
    fn write_short(&mut self, _: &Schema, _: i16) -> Result<(), SerdeError> {
        Ok(())
    }
    fn write_integer(&mut self, _: &Schema, _: i32) -> Result<(), SerdeError> {
        Ok(())
    }
    fn write_long(&mut self, _: &Schema, _: i64) -> Result<(), SerdeError> {
        Ok(())
    }
    fn write_float(&mut self, _: &Schema, _: f32) -> Result<(), SerdeError> {
        Ok(())
    }
    fn write_double(&mut self, _: &Schema, _: f64) -> Result<(), SerdeError> {
        Ok(())
    }
    fn write_big_integer(&mut self, _: &Schema, _: &BigInteger) -> Result<(), SerdeError> {
        Ok(())
    }
    fn write_big_decimal(&mut self, _: &Schema, _: &BigDecimal) -> Result<(), SerdeError> {
        Ok(())
    }
    fn write_blob(&mut self, _: &Schema, _: &[u8]) -> Result<(), SerdeError> {
        Ok(())
    }
    fn write_timestamp(&mut self, _: &Schema, _: &DateTime) -> Result<(), SerdeError> {
        Ok(())
    }
    fn write_document(&mut self, _: &Schema, _: &Document) -> Result<(), SerdeError> {
        Ok(())
    }
    fn write_null(&mut self, _: &Schema) -> Result<(), SerdeError> {
        Ok(())
    }
    fn write_struct(&mut self, _: &Schema, _: &dyn SerializableStruct) -> Result<(), SerdeError> {
        Ok(())
    }
    fn write_list(
        &mut self,
        _: &Schema,
        _: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        Ok(())
    }
    fn write_map(
        &mut self,
        _: &Schema,
        _: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_simple_label() {
        let labels = match_path_labels("/things/{Id}", "/things/abc");
        assert_eq!(labels.get("Id").map(|s| s.as_str()), Some("abc"));
    }

    #[test]
    fn match_greedy_label() {
        let labels = match_path_labels("/files/{Path+}", "/files/a/b/c.txt");
        assert_eq!(labels.get("Path").map(|s| s.as_str()), Some("a/b/c.txt"));
    }

    #[test]
    fn split_header_value_simple() {
        assert_eq!(split_header_list("a,b,c"), vec!["a", "b", "c"]);
    }

    #[test]
    fn split_header_value_quoted() {
        assert_eq!(split_header_list(r#"a,"b,c",d"#), vec!["a", "b,c", "d"]);
    }
}
