// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-schemas-schemas service.
//!
//! This is the service's single store: the registry → schema → schema-version tree
//! every handler reads and writes. It also holds the few helpers more than one
//! handler needs (ARN synthesis, the request's region, tag conversion, listing
//! pagination and the error payloads), because `src/lib.rs` and `src/ops/mod.rs` are
//! generated and cannot declare another module.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    BadRequestException, ConflictException, ForbiddenException, InternalServerErrorException,
    NotFoundException, PreconditionFailedException,
};

/// Per-service state for com-amazonaws-schemas-schemas. A handler reaches it through [`service_state`].
///
/// `BTreeMap` rather than `HashMap`: every listing operation this service models is
/// `@paginated` with an opaque `NextToken`, and a token can only resume a page
/// deterministically if the underlying order is stable. `BTreeMap`'s key order is.
#[derive(Debug)]
pub struct ComAmazonawsSchemasSchemasState {
    /// Registries by `RegistryName`, of both scopes: the ones callers create and the
    /// service-owned catalogue [`aws_managed_schemas`] seeds.
    pub registries: BTreeMap<String, Registry>,
    /// Source of resource-policy revision ids, so every revision this process mints
    /// is distinct. See [`ComAmazonawsSchemasSchemasState::mint_policy_revision_id`].
    pub next_policy_revision_seq: u64,
}

impl ComAmazonawsSchemasSchemasState {
    /// Mint the next resource-policy revision id.
    ///
    /// A revision id identifies ONE revision of a policy: it must differ from the
    /// revision it replaces (otherwise a stale-revision precondition could not be
    /// detected) and must stay put until the next write. It is drawn from a
    /// store-wide counter rather than at random, so a process's ids are
    /// deterministic and no two policies can ever share one, and rendered in the
    /// 8-4-4-4-12 shape AWS opaque ids use (version nibble 4, variant nibble 8, as
    /// RFC 4122 lays them out) so a client sees the form it expects.
    pub fn mint_policy_revision_id(&mut self) -> String {
        self.next_policy_revision_seq = self.next_policy_revision_seq.saturating_add(1);
        let seq = self.next_policy_revision_seq;
        format!("{:08x}-0000-4000-8000-{seq:012x}", seq & 0xffff_ffff)
    }
}

/// A fresh store already holds the service-owned registries: they exist in every
/// account before any call is made, so they are part of what "empty" means here.
/// Only [`RegistryScope::Local`] registries are a caller's.
impl Default for ComAmazonawsSchemasSchemasState {
    fn default() -> Self {
        Self {
            registries: aws_managed_registries(),
            next_policy_revision_seq: 0,
        }
    }
}

/// Who owns a registry — what `ListRegistries`' `Scope` selects between.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RegistryScope {
    /// A caller's own registry, created through `CreateRegistry`.
    #[default]
    Local,
    /// A service-owned registry, present before any call: AWS owns its contents, so a
    /// caller may read it but not write to it.
    Aws,
}

impl RegistryScope {
    /// Whether AWS, rather than the caller, owns what is in this registry.
    pub fn is_managed(self) -> bool {
        matches!(self, Self::Aws)
    }
}

/// A schema registry, as `CreateRegistry` persisted it or as the service-owned
/// catalogue seeded it.
#[derive(Debug, Clone)]
pub struct Registry {
    pub name: String,
    /// `arn:aws:schemas:<region>:<account>:registry/<name>` as minted at create time,
    /// for a registry a caller created. Read it through [`Registry::arn_in`], which is
    /// what a service-owned registry — created in no single request, so signed for no
    /// single region — needs.
    pub arn: String,
    pub description: Option<String>,
    pub tags: BTreeMap<String, String>,
    /// Whether AWS or the caller owns this registry.
    pub scope: RegistryScope,
    /// Schemas by `SchemaName`.
    pub schemas: BTreeMap<String, Schema>,
    /// The resource-based policy attached to this registry, as `PutResourcePolicy`
    /// last wrote it; `None` for a registry no policy is attached to. It lives on
    /// the registry because the policy is a property OF the registry: deleting the
    /// registry takes its policy with it.
    pub policy: Option<ResourcePolicy>,
}

/// A registry's resource-based policy and the revision it is at.
#[derive(Debug, Clone)]
pub struct ResourcePolicy {
    /// The policy document, as [`canonical_json`] encodes what the caller sent.
    pub document: String,
    /// The revision this policy is at, minted by
    /// [`ComAmazonawsSchemasSchemasState::mint_policy_revision_id`] on every write.
    pub revision_id: String,
}

/// The bytes a policy document is stored and reported as: the document re-encoded as
/// JSON, with its insignificant whitespace gone and its member order kept.
///
/// A resource policy travels as an opaque JSON DOCUMENT (`Policy` is modelled
/// `@mediaType("application/json")`), and the service that takes one parses it and
/// writes it back out: what a later read reports is the document's JSON content, not
/// the byte-for-byte text the caller's renderer happened to produce. So a document
/// indented or spaced by one caller reads back the same as the same document sent
/// compact by another, and a caller that hashes or diffs a policy round-trip — the
/// standard way to detect drift — sees no change.
///
/// Encoded by streaming the reader straight into the writer, so the members keep the
/// order the caller wrote them in; an intermediate [`serde_json::Value`] would sort
/// them, which would reorder a document rather than re-encode it.
///
/// What a re-encode does NOT keep is the LITERAL a number was written as: a number
/// reaches the writer as the value the reader read, so `1e2` comes back `100.0` and a
/// literal with more digits than a double holds comes back rounded to one. The VALUE a
/// policy denotes is kept; its spelling is not. Keeping the spelling too would mean
/// turning on `serde_json`'s arbitrary-precision mode, which is a feature of the crate
/// as a whole and would change how every other service in this workspace reads and
/// writes a JSON number — a far larger change than a policy's digits are worth, and one
/// no recording asks for (a resource-based policy is statements of strings; the fixed
/// point `tests/all/resource_policy.rs` pins holds for every form a policy actually
/// carries).
///
/// `Err` carries what made the value unreadable, for a value that is not JSON at
/// all — the service's own diagnostic for an unreadable body, as
/// [`json_parse_detail`] builds it.
pub fn canonical_json(document: &str) -> Result<String, String> {
    let mut encoded: Vec<u8> = Vec::with_capacity(document.len());
    let mut reader = ::serde_json::Deserializer::from_str(document);
    {
        let mut writer = ::serde_json::Serializer::new(&mut encoded);
        ::serde_transcode::transcode(&mut reader, &mut writer)
            .map_err(|error| json_parse_detail(document, &error))?;
    }
    // One document, not a stream of them: anything after the first value makes the
    // value unreadable rather than being silently dropped.
    reader
        .end()
        .map_err(|error| json_parse_detail(document, &error))?;
    String::from_utf8(encoded).map_err(|error| error.to_string())
}

// ---------------------------------------------------------------------------
// Where a JSON body stops being JSON
//
// Every refusal this service gives for a body it could not read — `CreateSchema`'s
// and `UpdateSchema`'s `Content`, `PutResourcePolicy`'s `Policy` — says WHERE the
// body stopped being JSON, and says it the way the service does: by naming the
// OFFENDING CHARACTER, what was being read when the parser reached it, and that
// character's BYTE OFFSET into the body the caller sent
// (`invalid character 'o' in literal null (expecting 'u') at offset 2`).
//
// The OFFSET is the count of bytes the parser had consumed when it reached the
// offending character — so the 1-based byte position of that character, which is what
// the recorded refusals report: the instance above names the second byte of its body
// as `offset 2`.
//
// The offset is the only part of such a refusal that tells a client WHERE in its own
// document the defect is, and a byte position in the submitted bytes stays meaningful
// whatever shape the document has — one line or a thousand. `serde_json`, the parser
// this emulator reads JSON with, reports a LINE and COLUMN in its own vocabulary
// instead (`expected ident at line 1 column 2`): a different coordinate system,
// naming a different place in any multi-line document, in words no client of this API
// is written against. So `serde_json` stays what decides whether a body is VALID
// JSON, and the position and wording a refusal carries are produced here.
//
// The scan below walks the bytes with an explicit stack rather than by recursion: a
// body is bounded (`Content` is modelled at most 100000 bytes) but its NESTING is
// not, and a hundred thousand open brackets must not be able to exhaust the stack.
// ---------------------------------------------------------------------------

/// Where a JSON document stops being JSON, as a refusal reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonFault {
    /// The offset the refusal reports: the number of bytes read when the offending
    /// character was reached, which is that character's 1-based byte position in the
    /// document. `None` where the document simply ran out: then there is no character
    /// to point a client at.
    pub offset: Option<usize>,
    /// What was wrong there: the offending character and what was being read.
    pub detail: String,
}

impl ::std::fmt::Display for JsonFault {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        match self.offset {
            Some(offset) => write!(f, "{} at offset {offset}", self.detail),
            None => write!(f, "{}", self.detail),
        }
    }
}

/// What this service says about a body `serde_json` refused: the offending character
/// and its byte offset, as [`json_fault`] finds them.
///
/// `serde_json` decides VALIDITY; this decides the wording. Where the scan finds no
/// fault at all the document is one well-formed JSON value that `serde_json` still
/// refused — it holds itself to a nesting limit this scan does not — so the position
/// `serde_json` names is converted to a byte offset in the same coordinates and
/// reported without its line/column vocabulary.
pub fn json_parse_detail(document: &str, error: &::serde_json::Error) -> String {
    match json_fault(document) {
        Some(fault) => fault.to_string(),
        None => format!(
            "invalid JSON at offset {}",
            byte_offset_of(document, error.line(), error.column()).saturating_add(1)
        ),
    }
}

/// The 0-based byte index a `serde_json` position names: its `line` is counted from 1
/// and its `column` is the 1-based position within that line.
fn byte_offset_of(document: &str, line: usize, column: usize) -> usize {
    let mut offset = 0usize;
    for (number, text) in document.split_inclusive('\n').enumerate() {
        if number + 1 == line {
            return (offset + column.saturating_sub(1)).min(document.len());
        }
        offset += text.len();
    }
    document.len()
}

/// Scan `document` as ONE JSON value and report where it stops being JSON, or `None`
/// where it is a well-formed JSON document with nothing after it.
///
/// The grammar is RFC 8259's: one value, with insignificant whitespace around it and
/// nothing following it. What this reports about a document it refuses is the
/// offending character, what was being read, and the character's byte offset.
pub fn json_fault(document: &str) -> Option<JsonFault> {
    JsonScan {
        bytes: document.as_bytes(),
        at: 0,
    }
    .fault()
}

/// Which container the scan is inside, so the character that may follow a value is
/// known: a `,` and the container's own closing bracket, and after a `,` in an object
/// another member.
#[derive(Debug, Clone, Copy)]
enum JsonFrame {
    Array,
    Object,
}

/// The scan: a cursor over the document's bytes.
///
/// Bytes, not characters: a refusal names the offending BYTE's offset, and a
/// multi-byte character that cannot start a JSON value is reported by its leading
/// byte the same way a parser reading octets reports it.
struct JsonScan<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl JsonScan<'_> {
    /// The byte at the cursor, without consuming it.
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    /// Consume the byte at the cursor.
    fn bump(&mut self) -> Option<u8> {
        let byte = self.peek();
        if byte.is_some() {
            self.at += 1;
        }
        byte
    }

    /// Skip the whitespace JSON allows between tokens: space, tab, newline, carriage
    /// return (RFC 8259, section 2).
    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    /// A document that ran out before its value was complete. There is no offending
    /// character, so no offset is reported.
    fn truncated() -> JsonFault {
        JsonFault {
            offset: None,
            detail: "unexpected end of JSON input".to_string(),
        }
    }

    /// The character at byte index `index` cannot appear where the scan is: `reading`
    /// says what was being read there.
    ///
    /// The offset the refusal reports is the number of bytes read to reach that
    /// character — `index + 1` — which is the coordinate the service's refusals are
    /// recorded in.
    fn invalid(index: usize, byte: u8, reading: &str) -> JsonFault {
        JsonFault {
            offset: Some(index.saturating_add(1)),
            detail: format!("invalid character {} {reading}", quoted_byte(byte)),
        }
    }

    /// Walk the whole document; `None` when it is one well-formed JSON value.
    fn fault(&mut self) -> Option<JsonFault> {
        let mut stack: Vec<JsonFrame> = Vec::new();
        'value: loop {
            // A value is expected here: the start of the document, an element after a
            // `,` in an array, or a member's value after a `:` in an object.
            self.skip_whitespace();
            let offset = self.at;
            // Nothing where a value must be: the document ran out (or holds only
            // whitespace), which is not a JSON document.
            let Some(byte) = self.peek() else {
                return Some(Self::truncated());
            };
            match byte {
                b'{' => {
                    self.at += 1;
                    self.skip_whitespace();
                    if self.peek() == Some(b'}') {
                        // The empty object: complete, with no member to read.
                        self.at += 1;
                    } else {
                        stack.push(JsonFrame::Object);
                        if let Some(fault) = self.member_name() {
                            return Some(fault);
                        }
                        continue 'value;
                    }
                }
                b'[' => {
                    self.at += 1;
                    self.skip_whitespace();
                    if self.peek() == Some(b']') {
                        self.at += 1;
                    } else {
                        stack.push(JsonFrame::Array);
                        continue 'value;
                    }
                }
                b'"' => {
                    if let Some(fault) = self.string() {
                        return Some(fault);
                    }
                }
                b't' => {
                    if let Some(fault) = self.literal("true") {
                        return Some(fault);
                    }
                }
                b'f' => {
                    if let Some(fault) = self.literal("false") {
                        return Some(fault);
                    }
                }
                b'n' => {
                    if let Some(fault) = self.literal("null") {
                        return Some(fault);
                    }
                }
                b'-' | b'0'..=b'9' => {
                    if let Some(fault) = self.number() {
                        return Some(fault);
                    }
                }
                other => {
                    return Some(Self::invalid(
                        offset,
                        other,
                        "looking for beginning of value",
                    ))
                }
            }

            // One value is complete. What may follow it depends on what encloses it,
            // and a closing bracket completes the enclosing value in turn.
            loop {
                match stack.last().copied() {
                    None => {
                        // The document's one value is complete: only whitespace may
                        // follow it.
                        self.skip_whitespace();
                        let offset = self.at;
                        return self
                            .peek()
                            .map(|byte| Self::invalid(offset, byte, "after top-level value"));
                    }
                    Some(JsonFrame::Array) => {
                        self.skip_whitespace();
                        let offset = self.at;
                        match self.bump() {
                            None => return Some(Self::truncated()),
                            Some(b',') => continue 'value,
                            Some(b']') => {
                                stack.pop();
                            }
                            Some(byte) => {
                                return Some(Self::invalid(offset, byte, "after array element"))
                            }
                        }
                    }
                    Some(JsonFrame::Object) => {
                        self.skip_whitespace();
                        let offset = self.at;
                        match self.bump() {
                            None => return Some(Self::truncated()),
                            Some(b',') => {
                                if let Some(fault) = self.member_name() {
                                    return Some(fault);
                                }
                                continue 'value;
                            }
                            Some(b'}') => {
                                stack.pop();
                            }
                            Some(byte) => {
                                return Some(Self::invalid(
                                    offset,
                                    byte,
                                    "after object key:value pair",
                                ))
                            }
                        }
                    }
                }
            }
        }
    }

    /// An object member's name and the `:` that separates it from its value.
    fn member_name(&mut self) -> Option<JsonFault> {
        self.skip_whitespace();
        let offset = self.at;
        match self.peek() {
            None => return Some(Self::truncated()),
            Some(b'"') => {
                if let Some(fault) = self.string() {
                    return Some(fault);
                }
            }
            Some(byte) => {
                return Some(Self::invalid(
                    offset,
                    byte,
                    "looking for beginning of object key string",
                ))
            }
        }
        self.skip_whitespace();
        let offset = self.at;
        match self.bump() {
            None => Some(Self::truncated()),
            Some(b':') => None,
            Some(byte) => Some(Self::invalid(offset, byte, "after object key")),
        }
    }

    /// A string, from the opening quote at the cursor to its closing quote, including
    /// the escapes RFC 8259 section 7 allows.
    fn string(&mut self) -> Option<JsonFault> {
        // The opening quote is what selected this scan.
        self.at += 1;
        loop {
            let offset = self.at;
            match self.bump() {
                None => return Some(Self::truncated()),
                Some(b'"') => return None,
                Some(b'\\') => {
                    let offset = self.at;
                    match self.bump() {
                        None => return Some(Self::truncated()),
                        Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => {}
                        Some(b'u') => {
                            // `\uXXXX`: exactly four hexadecimal digits.
                            for _ in 0..4 {
                                let offset = self.at;
                                match self.bump() {
                                    None => return Some(Self::truncated()),
                                    Some(byte) if byte.is_ascii_hexdigit() => {}
                                    Some(byte) => {
                                        return Some(Self::invalid(
                                            offset,
                                            byte,
                                            "in \\u hexadecimal character escape",
                                        ))
                                    }
                                }
                            }
                        }
                        Some(byte) => {
                            return Some(Self::invalid(offset, byte, "in string escape code"))
                        }
                    }
                }
                // A control character has to be escaped to appear in a string.
                Some(byte) if byte < 0x20 => {
                    return Some(Self::invalid(offset, byte, "in string literal"))
                }
                Some(_) => {}
            }
        }
    }

    /// One of the three literals, whose first byte the cursor is on: the rest of it
    /// must follow exactly.
    fn literal(&mut self, literal: &str) -> Option<JsonFault> {
        self.at += 1;
        for expected in &literal.as_bytes()[1..] {
            let offset = self.at;
            match self.bump() {
                None => return Some(Self::truncated()),
                Some(byte) if byte == *expected => {}
                Some(byte) => {
                    return Some(Self::invalid(
                        offset,
                        byte,
                        &format!("in literal {literal} (expecting '{}')", *expected as char),
                    ))
                }
            }
        }
        None
    }

    /// A number: an optional `-`, an integer part, an optional fraction and an
    /// optional exponent (RFC 8259, section 6).
    ///
    /// A number ENDS at the first byte that cannot continue it, which is not an error
    /// here: whether that byte may follow a value is decided by what encloses the
    /// number, so `01` at the top level is reported as a character after a complete
    /// value rather than as a bad number.
    fn number(&mut self) -> Option<JsonFault> {
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        let offset = self.at;
        match self.bump() {
            None => return Some(Self::truncated()),
            // A leading zero is the whole integer part: `01` is not a JSON number.
            Some(b'0') => {}
            Some(byte) if byte.is_ascii_digit() => self.skip_digits(),
            Some(byte) => return Some(Self::invalid(offset, byte, "in numeric literal")),
        }
        if self.peek() == Some(b'.') {
            self.at += 1;
            let offset = self.at;
            match self.bump() {
                None => return Some(Self::truncated()),
                Some(byte) if byte.is_ascii_digit() => self.skip_digits(),
                Some(byte) => {
                    return Some(Self::invalid(
                        offset,
                        byte,
                        "after decimal point in numeric literal",
                    ))
                }
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.at += 1;
            }
            let offset = self.at;
            match self.bump() {
                None => return Some(Self::truncated()),
                Some(byte) if byte.is_ascii_digit() => self.skip_digits(),
                Some(byte) => {
                    return Some(Self::invalid(
                        offset,
                        byte,
                        "in exponent of numeric literal",
                    ))
                }
            }
        }
        None
    }

    /// Consume the digits that continue a number.
    fn skip_digits(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
            self.at += 1;
        }
    }
}

/// One byte as a refusal names it: printable ASCII as itself, and anything else by
/// the escape that stands for it, so a refusal stays one line of text whatever byte
/// it is about.
fn quoted_byte(byte: u8) -> String {
    let escaped = match byte {
        b'\'' => "\\'",
        b'\\' => "\\\\",
        0x07 => "\\a",
        0x08 => "\\b",
        0x09 => "\\t",
        0x0a => "\\n",
        0x0b => "\\v",
        0x0c => "\\f",
        0x0d => "\\r",
        0x20..=0x7e => return format!("'{}'", byte as char),
        _ => return format!("'\\x{byte:02x}'"),
    };
    format!("'{escaped}'")
}

impl Registry {
    /// The ARN this registry reports to a request signed for `region`.
    ///
    /// A caller's registry reports the ARN its `CreateRegistry` call minted. A
    /// service-owned one was created by no request, so no request's region is stored
    /// for it: it is present in every region, and reports the region of the request
    /// asking.
    pub fn arn_in(&self, region: &str) -> String {
        match self.scope {
            RegistryScope::Local => self.arn.clone(),
            RegistryScope::Aws => registry_arn(region, &self.name),
        }
    }

    /// The ARN one of this registry's schemas reports, or `None` for a schema that has
    /// none.
    ///
    /// A schema a caller created IS a customer resource: it was minted by a request, it
    /// can be tagged and named in a policy, and it reports the ARN that create stored.
    /// A schema of a service-owned registry is not — AWS publishes it, no account owns
    /// it, and no `TagResource` or policy call accepts an identifier for it — so it
    /// carries no ARN at all. Reporting one would hand a client a resource identifier
    /// the service never issued.
    ///
    /// The `region` argument is kept so a future service-owned resource that DOES carry
    /// a regional ARN can be rendered here beside a caller's; nothing needs it today.
    pub fn schema_arn_in(&self, _region: &str, schema: &Schema) -> Option<String> {
        match self.scope {
            RegistryScope::Local => Some(schema.arn.clone()),
            RegistryScope::Aws => None,
        }
    }

    /// The `SchemaArn` a single-schema response reports: the schema's ARN, or the empty
    /// string for a schema that has none ([`Registry::schema_arn_in`]).
    ///
    /// `DescribeSchema` and `ExportSchema` answer ABOUT one schema and always carry the
    /// member; a schema with no ARN reports it empty. A listing's SUMMARY leaves the
    /// member out instead, because a summary carries only the members a schema has.
    pub fn reported_schema_arn_in(&self, region: &str, schema: &Schema) -> String {
        self.schema_arn_in(region, schema).unwrap_or_default()
    }
}

/// A schema and all of its versions.
#[derive(Debug, Clone)]
pub struct Schema {
    pub name: String,
    /// `arn:aws:schemas:<region>:<account>:schema/<registry>/<name>`.
    pub arn: String,
    pub description: Option<String>,
    /// One of [`crate::ops::types::TYPE_VALUES`].
    pub r#type: String,
    pub tags: BTreeMap<String, String>,
    /// The highest version number stored; rendered as the decimal `SchemaVersion`.
    pub latest_version: u64,
    /// When the schema itself was last written (created, or a new version added).
    pub last_modified: SystemTime,
    /// Versions by number. Numeric keys, so the map's order is the version order —
    /// a string key would sort `"10"` before `"2"`.
    pub versions: BTreeMap<u64, SchemaVersion>,
    /// The version each `ClientTokenId` already minted, so a RETRIED `UpdateSchema`
    /// answers the version its first attempt wrote instead of writing a second copy
    /// of the same definition — what the member's `@idempotencyToken` trait is for.
    ///
    /// Bounded by the version map: only a call that mints a version records a token,
    /// so there is at most one entry per version ever minted. An entry whose version
    /// has since been deleted names nothing, and `UpdateSchema` ignores it rather
    /// than answering a version the store no longer holds.
    pub minted_by_token: BTreeMap<String, u64>,
    /// The code bindings `PutCodeBinding` has started for this schema, keyed by the
    /// schema version they were generated from and the language they were generated
    /// for: a binding is identified by that pair, so one schema version can carry a
    /// binding per language and one language a binding per version.
    pub code_bindings: BTreeMap<(u64, String), CodeBinding>,
}

/// One version of a schema: the content a caller sent for it.
///
/// The `content`, the `type` and the `created_date` are written once and never
/// rewritten — a reader of version N gets the bytes version N was minted with.
///
/// A version holds NO description: `Description` is metadata of the SCHEMA, not of a
/// definition ([`Schema::description`]), so an `UpdateSchema` that changes it changes
/// what every read of the schema reports, whichever version the read selects. Keeping
/// a second copy per version is what let a read of one version report a description a
/// later update had already replaced.
#[derive(Debug, Clone)]
pub struct SchemaVersion {
    pub version: u64,
    /// Returned verbatim by `DescribeSchema`.
    pub content: String,
    pub r#type: String,
    pub created_date: SystemTime,
}

/// One code binding: the code generation `PutCodeBinding` started for one schema
/// version in one language.
///
/// It holds no generated source — this slice models the binding's LIFECYCLE only, as
/// `PutCodeBinding` and `DescribeCodeBinding` report it. `GetCodeBindingSource`, which
/// is what serves the generated source itself, is still a scaffold stub and answers
/// 501; it owns the decision of what that source is.
#[derive(Debug, Clone)]
pub struct CodeBinding {
    /// The language generation was asked for: one of [`CODE_BINDING_LANGUAGES`],
    /// which is what [`code_binding_language`] admits.
    ///
    /// The model types `Language` as an unconstrained `__string`, but the service
    /// does constrain it — it refuses anything outside the pattern quoted on
    /// [`CODE_BINDING_LANGUAGES`] — so only a language this service generates for
    /// ever reaches the store.
    pub language: String,
    /// The schema version the binding was generated from, resolved at put time: an
    /// absent request `SchemaVersion` means the then-latest version, and the binding
    /// stays attached to that number afterwards.
    pub schema_version: u64,
    /// When `PutCodeBinding` started this generation.
    pub created_date: SystemTime,
    /// The instant generation is due to finish; [`CodeBinding::status_at`] reports
    /// `CREATE_IN_PROGRESS` before it and `CREATE_COMPLETE` from it on.
    ///
    /// Stored as an instant rather than waited for: the transition is the clock's, so
    /// every later read derives it and no request sleeps.
    pub generation_due: SystemTime,
}

/// The `CodeGenerationStatus` of a generation that has not finished yet. One of
/// [`crate::ops::types::CODE_GENERATION_STATUS_VALUES`].
pub const CODE_GENERATION_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";

/// The `CodeGenerationStatus` of a generation that has finished. One of
/// [`crate::ops::types::CODE_GENERATION_STATUS_VALUES`].
pub const CODE_GENERATION_COMPLETE: &str = "CREATE_COMPLETE";

/// How long after `PutCodeBinding` a code binding's generation is due to complete.
///
/// AWS publishes no duration for code generation, so this span is the emulator's own
/// choice. It is deliberately NOT zero: a caller that describes the binding straight
/// after starting it must still observe the transient `CREATE_IN_PROGRESS` status that
/// the real service reports, and the 202 `PutCodeBinding` answers with says the work
/// is accepted, not done. It is short enough that a caller polling the binding reaches
/// `CREATE_COMPLETE` in well under a second.
pub const CODE_GENERATION_DURATION: Duration = Duration::from_millis(400);

/// What both `PutCodeBinding` and `DescribeCodeBinding` report about one binding.
///
/// The two operations answer the same four members from the same stored binding, so
/// they render it through one function ([`CodeBinding::report_at`]) and cannot drift.
#[derive(Debug, Clone)]
pub struct CodeBindingReport {
    pub creation_date: SystemTime,
    pub last_modified: SystemTime,
    /// The version number in decimal, as `SchemaVersion` is rendered everywhere else.
    pub schema_version: String,
    pub status: &'static str,
}

impl CodeBinding {
    /// The status of this generation as of `now`: in progress until the instant it is
    /// due, complete from then on. Derived, so a read never has to write.
    pub fn status_at(&self, now: SystemTime) -> &'static str {
        if now < self.generation_due {
            CODE_GENERATION_IN_PROGRESS
        } else {
            CODE_GENERATION_COMPLETE
        }
    }

    /// When this binding last changed, as of `now`: it was created, and the only later
    /// change is the completion the clock makes due.
    pub fn last_modified_at(&self, now: SystemTime) -> SystemTime {
        if now < self.generation_due {
            self.created_date
        } else {
            self.generation_due
        }
    }

    /// This binding as the two code-binding operations report it.
    pub fn report_at(&self, now: SystemTime) -> CodeBindingReport {
        CodeBindingReport {
            creation_date: self.created_date,
            last_modified: self.last_modified_at(now),
            schema_version: self.schema_version.to_string(),
            status: self.status_at(now),
        }
    }
}

impl Schema {
    /// The version a request asked for: `SchemaVersion` when given, else the latest.
    ///
    /// `None` means the request named a version this schema does not hold — including
    /// a value that is not a decimal version number at all, which no stored version
    /// can match. The raw label is parsed as given: a version number is the whole
    /// value, so surrounding space is not a version number.
    pub fn version_for(&self, requested: Option<&str>) -> Option<&SchemaVersion> {
        match requested {
            None => self.versions.get(&self.latest_version),
            Some(raw) => self.versions.get(&raw.parse::<u64>().ok()?),
        }
    }

    /// Store `content` as this schema's NEXT version and answer the version stored.
    ///
    /// The number is taken from `latest_version`, which only ever rises — so it is
    /// unique over the life of the schema even when a `DeleteSchemaVersion` freed an
    /// earlier number, and two writers cannot be handed the same one, because the
    /// whole mint-and-insert happens under the one state lock the caller already
    /// holds. An existing version is never rewritten.
    /// The schema's `description` is NOT touched here: it is metadata of the schema
    /// rather than of a definition, so only a request that sends `Description`
    /// changes it.
    pub fn add_version(
        &mut self,
        content: String,
        r#type: String,
        now: SystemTime,
    ) -> &SchemaVersion {
        let version = self.latest_version.saturating_add(1);
        self.latest_version = version;
        self.last_modified = now;
        self.r#type = r#type.clone();
        self.versions.insert(
            version,
            SchemaVersion {
                version,
                content,
                r#type,
                created_date: now,
            },
        );
        self.versions
            .get(&version)
            .expect("the version inserted above is present")
    }
}

/// The SERVICE-SPECIFIC event types the service-owned catalogue holds a schema for:
/// the events an AWS service publishes beyond the CloudTrail-delivered family every
/// source publishes (which [`CLOUD_TRAIL_EVENT_FAMILY`] carries instead, one entry per
/// source per kind of CloudTrail record).
///
/// Source: the `detail` of each event as AWS documents it (the EventBridge event
/// reference for the publishing service); the `detail_type` is the `detail-type`
/// EventBridge delivers it under and the `event_name` is the name the catalogue
/// publishes it as, which AWS derives from that detail-type.
///
/// This is seeded content, not a response: `ListRegistries`, `ListSchemas` and
/// `DescribeSchema` report it from the store like any other registry, and
/// `ExportSchema` converts it. A caller cannot write to it — see
/// [`RegistryScope::Aws`]. [`aws_managed_schemas`] is the whole catalogue, and is
/// what seeds the store.
pub const AWS_SERVICE_EVENT_TYPES: &[ManagedEventType] = &[
    ManagedEventType {
        source: "aws.autoscaling",
        detail_type: "EC2 Instance Launch Successful",
        event_name: "EC2InstanceLaunchSuccessful",
        components: r##"{"EC2InstanceLaunchSuccessful":{"type":"object","required":["AutoScalingGroupName","EC2InstanceId","RequestId"],"properties":{"AutoScalingGroupName":{"type":"string"},"EC2InstanceId":{"type":"string"},"RequestId":{"type":"string"}}}}"##,
    },
    ManagedEventType {
        source: "aws.ec2",
        detail_type: "EC2 Instance State-change Notification",
        event_name: "EC2InstanceStateChangeNotification",
        components: r##"{"EC2InstanceStateChangeNotification":{"type":"object","required":["instance-id","state"],"properties":{"instance-id":{"type":"string"},"state":{"type":"string"}}}}"##,
    },
    ManagedEventType {
        source: "aws.s3",
        detail_type: "Object Created",
        event_name: "ObjectCreated",
        components: r##"{"Bucket":{"type":"object","required":["name"],"properties":{"name":{"type":"string"}}},"Object":{"type":"object","required":["key","size"],"properties":{"key":{"type":"string"},"size":{"type":"integer","format":"int64"},"etag":{"type":"string"}}},"ObjectCreated":{"type":"object","required":["bucket","object","reason","version"],"properties":{"bucket":{"$ref":"#/components/schemas/Bucket"},"object":{"$ref":"#/components/schemas/Object"},"reason":{"type":"string"},"version":{"type":"string"}}}}"##,
    },
    ManagedEventType {
        source: "aws.batch",
        detail_type: "Batch Job State Change",
        event_name: "BatchJobStateChange",
        components: r##"{"BatchJobStateChange":{"type":"object","required":["jobArn","jobDefinition","jobId","jobName","jobQueue","status"],"properties":{"createdAt":{"type":"integer","format":"int64"},"jobArn":{"type":"string"},"jobDefinition":{"type":"string"},"jobId":{"type":"string"},"jobName":{"type":"string"},"jobQueue":{"type":"string"},"startedAt":{"type":"integer","format":"int64"},"status":{"type":"string"},"statusReason":{"type":"string"},"stoppedAt":{"type":"integer","format":"int64"}}}}"##,
    },
    ManagedEventType {
        source: "aws.codebuild",
        detail_type: "CodeBuild Build State Change",
        event_name: "CodeBuildBuildStateChange",
        components: r##"{"CodeBuildBuildStateChange":{"type":"object","required":["build-id","build-status","project-name","version"],"properties":{"build-id":{"type":"string"},"build-status":{"type":"string"},"current-phase":{"type":"string"},"current-phase-context":{"type":"string"},"project-name":{"type":"string"},"version":{"type":"string"}}}}"##,
    },
    ManagedEventType {
        source: "aws.codepipeline",
        detail_type: "CodePipeline Pipeline Execution State Change",
        event_name: "CodePipelinePipelineExecutionStateChange",
        components: r##"{"CodePipelinePipelineExecutionStateChange":{"type":"object","required":["execution-id","pipeline","state","version"],"properties":{"execution-id":{"type":"string"},"pipeline":{"type":"string"},"state":{"type":"string"},"version":{"type":"number"}}}}"##,
    },
    ManagedEventType {
        source: "aws.ecs",
        detail_type: "ECS Task State Change",
        event_name: "ECSTaskStateChange",
        components: r##"{"ECSContainer":{"type":"object","required":["containerArn","lastStatus","name","taskArn"],"properties":{"containerArn":{"type":"string"},"exitCode":{"type":"integer"},"lastStatus":{"type":"string"},"name":{"type":"string"},"reason":{"type":"string"},"taskArn":{"type":"string"}}},"ECSTaskStateChange":{"type":"object","required":["clusterArn","desiredStatus","lastStatus","taskArn","taskDefinitionArn"],"properties":{"clusterArn":{"type":"string"},"containers":{"type":"array","items":{"$ref":"#/components/schemas/ECSContainer"}},"createdAt":{"type":"string","format":"date-time"},"desiredStatus":{"type":"string"},"group":{"type":"string"},"lastStatus":{"type":"string"},"launchType":{"type":"string"},"startedBy":{"type":"string"},"stoppedReason":{"type":"string"},"taskArn":{"type":"string"},"taskDefinitionArn":{"type":"string"},"updatedAt":{"type":"string","format":"date-time"}}}}"##,
    },
    ManagedEventType {
        source: "aws.events",
        detail_type: "Scheduled Event",
        event_name: "ScheduledEvent",
        components: r##"{"ScheduledEvent":{"type":"object","properties":{}}}"##,
    },
    ManagedEventType {
        source: "aws.health",
        detail_type: "AWS Health Event",
        event_name: "AWSHealthEvent",
        components: r##"{"AWSHealthEvent":{"type":"object","required":["eventArn","eventTypeCategory","eventTypeCode","service","startTime"],"properties":{"affectedEntities":{"type":"array","items":{"$ref":"#/components/schemas/AWSHealthAffectedEntity"}},"endTime":{"type":"string"},"eventArn":{"type":"string"},"eventDescription":{"type":"array","items":{"$ref":"#/components/schemas/AWSHealthEventDescription"}},"eventTypeCategory":{"type":"string"},"eventTypeCode":{"type":"string"},"service":{"type":"string"},"startTime":{"type":"string"}}},"AWSHealthAffectedEntity":{"type":"object","properties":{"entityValue":{"type":"string"},"status":{"type":"string"}}},"AWSHealthEventDescription":{"type":"object","properties":{"language":{"type":"string"},"latestDescription":{"type":"string"}}}}"##,
    },
    ManagedEventType {
        source: "aws.tag",
        detail_type: "Tag Change on Resource",
        event_name: "TagChangeonResource",
        components: r##"{"TagChangeonResource":{"type":"object","required":["changed-tag-keys","resource-type","service","version"],"properties":{"changed-tag-keys":{"type":"array","items":{"type":"string"}},"resource-type":{"type":"string"},"service":{"type":"string"},"tags":{"type":"object"},"version":{"type":"integer"}}}}"##,
    },
    ManagedEventType {
        source: "aws.acm",
        detail_type: "ACM Certificate Available",
        event_name: "ACMCertificateAvailable",
        components: r##"{"ACMCertificateAvailable":{"type":"object","required":["Action","CertificateType","CommonName"],"properties":{"Action":{"type":"string"},"CertificateType":{"type":"string"},"CommonName":{"type":"string"},"DomainValidationMethod":{"type":"string"}}}}"##,
    },
    // The event types below are published by the catalogue with their identity (the
    // source and the `detail-type` EventBridge delivers them under) but with an
    // unconstrained `detail`: AWS documents that these events exist, not the members
    // their `detail` carries. [`UNDOCUMENTED_DETAIL`] says what that means for a
    // reader.
    ManagedEventType {
        source: "aws.a4b",
        detail_type: "Room State Change",
        event_name: "RoomStateChange",
        components: UNDOCUMENTED_DETAIL,
    },
    ManagedEventType {
        source: "aws.a4b",
        detail_type: "Room Device Calendar Error Status",
        event_name: "RoomDeviceCalendarErrorStatus",
        components: UNDOCUMENTED_DETAIL,
    },
    ManagedEventType {
        source: "aws.act",
        detail_type: "Trust Bundle Association Version Updated",
        event_name: "TrustBundleAssociationVersionUpdated",
        components: UNDOCUMENTED_DETAIL,
    },
    ManagedEventType {
        source: "aws.events",
        detail_type: "API Destination Activated",
        event_name: "APIDestinationActivated",
        components: UNDOCUMENTED_DETAIL,
    },
    ManagedEventType {
        source: "aws.events",
        detail_type: "API Destination Deactivated",
        event_name: "APIDestinationDeactivated",
        components: UNDOCUMENTED_DETAIL,
    },
    ManagedEventType {
        source: "aws.events",
        detail_type: "Connection Activated",
        event_name: "ConnectionActivated",
        components: UNDOCUMENTED_DETAIL,
    },
    ManagedEventType {
        source: "aws.events",
        detail_type: "Connection Authorization Started",
        event_name: "ConnectionAuthorizationStarted",
        components: UNDOCUMENTED_DETAIL,
    },
    ManagedEventType {
        source: "aws.events",
        detail_type: "Connection Deactivated",
        event_name: "ConnectionDeactivated",
        components: UNDOCUMENTED_DETAIL,
    },
    ManagedEventType {
        source: "aws.events",
        detail_type: "Connection Deauthorization Started",
        event_name: "ConnectionDeauthorizationStarted",
        components: UNDOCUMENTED_DETAIL,
    },
];

/// The `components.schemas` of an event type whose `detail` AWS publishes no member
/// list for: one component, named after the event type, constrained only to be an
/// object.
///
/// The catalogue's membership and an event's identity are what a discovery flow reads
/// (which event types exist, under which names, published by which source and
/// `detail-type`); the members of a `detail` AWS does not document cannot be invented
/// here, and a document that named members the real event does not carry would make a
/// generated model reject real events. So the component says exactly what is known:
/// the `detail` is a JSON object.
const UNDOCUMENTED_DETAIL: &str = r##"{"{EventName}":{"type":"object"}}"##;

/// The token a components template carries where the event type's own component name
/// goes, substituted by [`ManagedEventType::components_json`].
///
/// A template rather than a literal name, because one template serves a whole FAMILY
/// of event types: every CloudTrail-delivered event carries a CloudTrail record as its
/// `detail`, and each of them is published under its own component name.
const EVENT_COMPONENT_PLACEHOLDER: &str = "{EventName}";

/// How many versions the AWS-managed catalogue publishes for a schema, where it
/// publishes more than one.
///
/// `VersionCount` is "the number of versions available for the schema" — a property of
/// the PUBLISHED schema, not of this process: AWS has revised some of its event schemas
/// and still serves the earlier revisions, so a client that pins a version has one to
/// pin. The count is therefore catalogue data, seeded like a schema's name and its
/// document, and [`aws_managed_registries`] seeds exactly this many versions so the
/// count a listing reports stays what the store actually holds.
///
/// Source: the counts AWS reports for these entries. Anything not named here is
/// published at one version, which is the catalogue's norm.
const MANAGED_VERSION_COUNTS: &[(&str, u64)] = &[
    ("aws.a4b@RoomStateChange", 3),
    ("aws.acm@ACMCertificateAvailable", 3),
];

/// The number of versions the catalogue publishes for the schema named — see
/// [`MANAGED_VERSION_COUNTS`]. Never zero: a schema with no version could not be
/// described at all.
pub fn managed_version_count(schema_name: &str) -> u64 {
    MANAGED_VERSION_COUNTS
        .iter()
        .find(|(name, _)| *name == schema_name)
        .map_or(1, |(_, count)| *count)
}

/// The `Type` every schema of the service-owned catalogue is stored as: AWS stores its
/// event schemas as OpenAPI 3 documents, and `ExportSchema` is what converts one to
/// JSON Schema draft 4.
pub const AWS_MANAGED_SCHEMA_TYPE: &str = "OpenApi3";

/// The registry AWS publishes its discovered event schemas in — the read-only
/// catalogue of every AWS service event type, which every account sees without
/// creating anything.
pub const AWS_MANAGED_REGISTRY: &str = "aws.events";

/// Where an AWS event schema's OpenAPI document keeps its reusable components, as a
/// `$ref` names one. Shared with `ExportSchema`, which rewrites these refs when it
/// converts a document to JSON Schema draft 4.
pub const OPENAPI_COMPONENT_REF: &str = "#/components/schemas/";

/// The component an AWS event schema's OpenAPI document roots at: the event envelope,
/// whose `detail` references the event-specific component. Shared with
/// `ExportSchema`, which promotes this component to the root of its draft-4 export.
pub const EVENT_ENVELOPE_COMPONENT: &str = "AWSEvent";

/// The event name the CloudTrail-delivered API-call schema carries in the catalogue,
/// i.e. the `<EventName>` half of `<source>@<EventName>`.
///
/// Source: the catalogue names a schema after its event's `detail-type` with the
/// spaces taken out, so the `AWS API Call via CloudTrail` detail-type
/// ([`CLOUD_TRAIL_DETAIL_TYPE`]) is published as `AWSAPICallviaCloudTrail` — the
/// lower-case `via` is AWS's own, because the detail-type spells it that way.
pub const CLOUD_TRAIL_EVENT_NAME: &str = "AWSAPICallviaCloudTrail";

/// The `detail-type` EventBridge delivers a CloudTrail-recorded API call under.
pub const CLOUD_TRAIL_DETAIL_TYPE: &str = "AWS API Call via CloudTrail";

/// The CloudTrail-delivered event family every AWS event source publishes, as
/// `(detail-type, EventName)` pairs.
///
/// CloudTrail records more than API calls, and EventBridge delivers each KIND of record
/// under its own `detail-type`: a console action, a console sign-in, an Insights event,
/// a network-activity event and a service event beside the API call itself. The
/// catalogue holds one schema per kind per source, so a source's entries are this whole
/// family, not the API call alone.
///
/// The `detail` of every one of them IS a CloudTrail record, whose contents are the
/// same whichever kind it records (CloudTrail User Guide, "CloudTrail record
/// contents"), so one components template ([`CLOUD_TRAIL_COMPONENTS`]) serves the whole
/// family — published under each event's own component name.
///
/// The `EventName` beside each `detail-type` is that detail-type with its spaces taken
/// out, which is how the catalogue derives a schema's name; the pairs are written out
/// rather than computed so the published spelling (AWS's own lower-case `via`) is the
/// one in the table, and `the_cloud_trail_family_is_named_after_its_detail_types` below
/// holds the two to that rule.
pub const CLOUD_TRAIL_EVENT_FAMILY: &[(&str, &str)] = &[
    (CLOUD_TRAIL_DETAIL_TYPE, CLOUD_TRAIL_EVENT_NAME),
    (
        "AWS Console Action via CloudTrail",
        "AWSConsoleActionviaCloudTrail",
    ),
    (
        "AWS Console Sign In via CloudTrail",
        "AWSConsoleSignInviaCloudTrail",
    ),
    ("AWS Insight via CloudTrail", "AWSInsightviaCloudTrail"),
    (
        "AWS Network Activity Event via CloudTrail",
        "AWSNetworkActivityEventviaCloudTrail",
    ),
    (
        "AWS Service Event via CloudTrail",
        "AWSServiceEventviaCloudTrail",
    ),
];

/// The `components.schemas` of a CloudTrail-delivered event's document, excluding the
/// envelope [`event_schema_document`] renders around them. The record component is
/// named by [`EVENT_COMPONENT_PLACEHOLDER`], because the same record is published under
/// each member of [`CLOUD_TRAIL_EVENT_FAMILY`].
///
/// Source: the CloudTrail record contents AWS documents (CloudTrail User Guide,
/// "CloudTrail record contents") — the `detail` of a CloudTrail-delivered event IS a
/// CloudTrail record, and the record's contents are the same for every event source and
/// every record kind, which is why one template serves all of [`AWS_EVENT_SOURCES`].
/// The nested `userIdentity`/`sessionContext`/`resources` structures are components of
/// their own, as the document's `$ref`s name them.
///
/// The members CloudTrail fills only for some records (the free-form
/// `requestParameters`/`responseElements`/`additionalEventData` objects, the `addendum`
/// a late-delivered record carries, the TLS, Insights and edge-device details) are
/// marked `nullable`, which is OpenAPI 3.0's way of saying the member may be present
/// and null; `ExportSchema` lowers that to draft-04's type union.
const CLOUD_TRAIL_COMPONENTS: &str = r##"{
  "{EventName}": {
    "type": "object",
    "required": [
      "eventID", "eventName", "eventSource", "eventTime", "eventVersion",
      "awsRegion", "recipientAccountId", "requestID", "userIdentity"
    ],
    "properties": {
      "addendum": {
        "type": "object",
        "nullable": true,
        "properties": {
          "originalEventID": {"type": "string"},
          "originalRequestID": {"type": "string"},
          "reason": {"type": "string"},
          "updatedFields": {"type": "array", "items": {"type": "string"}}
        }
      },
      "additionalEventData": {"type": "object", "nullable": true},
      "annotation": {"type": "string"},
      "apiVersion": {"type": "string"},
      "awsRegion": {"type": "string"},
      "edgeDeviceDetails": {"type": "object", "nullable": true},
      "errorCode": {"type": "string"},
      "errorMessage": {"type": "string"},
      "eventCategory": {"type": "string"},
      "eventID": {"type": "string"},
      "eventName": {"type": "string"},
      "eventSource": {"type": "string"},
      "eventTime": {"type": "string", "format": "date-time"},
      "eventType": {"type": "string"},
      "eventVersion": {"type": "string"},
      "insightDetails": {"type": "object", "nullable": true},
      "managementEvent": {"type": "boolean"},
      "readOnly": {"type": "boolean"},
      "recipientAccountId": {"type": "string"},
      "requestID": {"type": "string"},
      "requestParameters": {"type": "object", "nullable": true},
      "resources": {
        "type": "array",
        "items": {"$ref": "#/components/schemas/CloudTrailResource"}
      },
      "responseElements": {"type": "object", "nullable": true},
      "serviceEventDetails": {"type": "object", "nullable": true},
      "sessionCredentialFromConsole": {"type": "string"},
      "sharedEventID": {"type": "string"},
      "sourceIPAddress": {"type": "string"},
      "tlsDetails": {
        "type": "object",
        "nullable": true,
        "properties": {
          "cipherSuite": {"type": "string"},
          "clientProvidedHostHeader": {"type": "string"},
          "tlsVersion": {"type": "string"}
        }
      },
      "userAgent": {"type": "string"},
      "userIdentity": {"$ref": "#/components/schemas/CloudTrailUserIdentity"},
      "vpcEndpointAccountId": {"type": "string"},
      "vpcEndpointId": {"type": "string"}
    }
  },
  "CloudTrailResource": {
    "type": "object",
    "properties": {
      "ARN": {"type": "string"},
      "accountId": {"type": "string"},
      "type": {"type": "string"}
    }
  },
  "CloudTrailUserIdentity": {
    "type": "object",
    "properties": {
      "accessKeyId": {"type": "string"},
      "accountId": {"type": "string"},
      "arn": {"type": "string"},
      "credentialId": {"type": "string"},
      "invokedBy": {"type": "string"},
      "onBehalfOf": {
        "type": "object",
        "nullable": true,
        "properties": {
          "backTraceId": {"type": "string"},
          "userId": {"type": "string"}
        }
      },
      "principalId": {"type": "string"},
      "sessionContext": {"$ref": "#/components/schemas/CloudTrailSessionContext"},
      "type": {"type": "string"},
      "userName": {"type": "string"}
    }
  },
  "CloudTrailSessionContext": {
    "type": "object",
    "properties": {
      "attributes": {"$ref": "#/components/schemas/CloudTrailSessionAttributes"},
      "ec2RoleDelivery": {"type": "string"},
      "sessionIssuer": {"$ref": "#/components/schemas/CloudTrailSessionIssuer"},
      "sourceIdentity": {"type": "string"},
      "webIdFederationData": {"type": "object", "nullable": true}
    }
  },
  "CloudTrailSessionAttributes": {
    "type": "object",
    "properties": {
      "creationDate": {"type": "string", "format": "date-time"},
      "mfaAuthenticated": {"type": "string"}
    }
  },
  "CloudTrailSessionIssuer": {
    "type": "object",
    "properties": {
      "accountId": {"type": "string"},
      "arn": {"type": "string"},
      "principalId": {"type": "string"},
      "type": {"type": "string"},
      "userName": {"type": "string"}
    }
  }
}"##;

/// The AWS event sources the managed catalogue holds a schema for — the `aws.<service>`
/// namespaces EventBridge delivers AWS service events under.
///
/// Source: the event-source namespaces AWS publishes events under, one per service
/// that records API calls in CloudTrail (EventBridge delivers those records under the
/// detail-types of [`CLOUD_TRAIL_EVENT_FAMILY`], which is why every source below
/// carries that whole family of schemas in the catalogue). Kept in alphabetical order
/// for reading; the store sorts by name on its own, and a name repeated here seeds one
/// schema, not two.
pub const AWS_EVENT_SOURCES: &[&str] = &[
    "aws.a4b",
    "aws.abuse",
    "aws.access-analyzer",
    "aws.account",
    "aws.acm",
    "aws.acm-pca",
    "aws.act",
    "aws.aidevops",
    "aws.airflow",
    "aws.amazon.connect",
    "aws.amazonmq",
    "aws.amazonq",
    "aws.amplify",
    "aws.amplifybackend",
    "aws.amplifyuibuilder",
    "aws.apigateway",
    "aws.appconfig",
    "aws.appflow",
    "aws.appintegrations",
    "aws.application-autoscaling",
    "aws.application-insights",
    "aws.applicationcostprofiler",
    "aws.appmesh",
    "aws.apprunner",
    "aws.appstream",
    "aws.appsync",
    "aws.aps",
    "aws.artifact",
    "aws.athena",
    "aws.auditmanager",
    "aws.autoscaling",
    "aws.autoscaling-plans",
    "aws.backup",
    "aws.backup-gateway",
    "aws.batch",
    "aws.billingconductor",
    "aws.braket",
    "aws.budgets",
    "aws.cassandra",
    "aws.ce",
    "aws.chatbot",
    "aws.chime",
    "aws.cloud9",
    "aws.clouddirectory",
    "aws.cloudformation",
    "aws.cloudfront",
    "aws.cloudhsm",
    "aws.cloudsearch",
    "aws.cloudshell",
    "aws.cloudtrail",
    "aws.cloudwatch",
    "aws.codeartifact",
    "aws.codebuild",
    "aws.codecommit",
    "aws.codedeploy",
    "aws.codeguru-profiler",
    "aws.codeguru-reviewer",
    "aws.codepipeline",
    "aws.codestar",
    "aws.codestar-connections",
    "aws.codestar-notifications",
    "aws.cognito-identity",
    "aws.cognito-idp",
    "aws.cognito-sync",
    "aws.comprehend",
    "aws.comprehendmedical",
    "aws.compute-optimizer",
    "aws.config",
    "aws.connect",
    "aws.connect-contact-lens",
    "aws.controltower",
    "aws.databrew",
    "aws.dataexchange",
    "aws.datapipeline",
    "aws.datasync",
    "aws.dax",
    "aws.deepracer",
    "aws.detective",
    "aws.devicefarm",
    "aws.devops-guru",
    "aws.directconnect",
    "aws.discovery",
    "aws.dlm",
    "aws.dms",
    "aws.drs",
    "aws.ds",
    "aws.dynamodb",
    "aws.ebs",
    "aws.ec2",
    "aws.ec2-instance-connect",
    "aws.ecr",
    "aws.ecr-public",
    "aws.ecs",
    "aws.eks",
    "aws.elasticache",
    "aws.elasticbeanstalk",
    "aws.elasticfilesystem",
    "aws.elasticloadbalancing",
    "aws.elasticmapreduce",
    "aws.elastictranscoder",
    "aws.email",
    "aws.emr-containers",
    "aws.emr-serverless",
    "aws.es",
    "aws.events",
    "aws.evidently",
    "aws.finspace",
    "aws.firehose",
    "aws.fis",
    "aws.fms",
    "aws.forecast",
    "aws.frauddetector",
    "aws.fsx",
    "aws.gamelift",
    "aws.glacier",
    "aws.globalaccelerator",
    "aws.glue",
    "aws.grafana",
    "aws.greengrass",
    "aws.groundstation",
    "aws.guardduty",
    "aws.health",
    "aws.healthlake",
    "aws.iam",
    "aws.identitystore",
    "aws.imagebuilder",
    "aws.inspector",
    "aws.inspector2",
    "aws.iot",
    "aws.iot1click",
    "aws.iotanalytics",
    "aws.iotdeviceadvisor",
    "aws.iotevents",
    "aws.iotfleethub",
    "aws.iotsitewise",
    "aws.iottwinmaker",
    "aws.iotwireless",
    "aws.ivs",
    "aws.kafka",
    "aws.kafkaconnect",
    "aws.kendra",
    "aws.kinesis",
    "aws.kinesisanalytics",
    "aws.kinesisvideo",
    "aws.kms",
    "aws.lakeformation",
    "aws.lambda",
    "aws.lex",
    "aws.license-manager",
    "aws.lightsail",
    "aws.location",
    "aws.logs",
    "aws.lookoutequipment",
    "aws.lookoutmetrics",
    "aws.lookoutvision",
    "aws.machinelearning",
    "aws.macie",
    "aws.managedblockchain",
    "aws.mediaconnect",
    "aws.mediaconvert",
    "aws.medialive",
    "aws.mediapackage",
    "aws.mediapackage-vod",
    "aws.mediastore",
    "aws.mediatailor",
    "aws.memorydb",
    "aws.mgh",
    "aws.mgn",
    "aws.mq",
    "aws.mturk",
    "aws.neptune",
    "aws.network-firewall",
    "aws.networkmanager",
    "aws.nimble",
    "aws.opensearch",
    "aws.opsworks",
    "aws.organizations",
    "aws.outposts",
    "aws.panorama",
    "aws.personalize",
    "aws.pi",
    "aws.pinpoint",
    "aws.polly",
    "aws.pricing",
    "aws.proton",
    "aws.qldb",
    "aws.quicksight",
    "aws.ram",
    "aws.rbin",
    "aws.rds",
    "aws.redshift",
    "aws.redshift-data",
    "aws.redshift-serverless",
    "aws.rekognition",
    "aws.resiliencehub",
    "aws.resource-groups",
    "aws.robomaker",
    "aws.route53",
    "aws.route53domains",
    "aws.route53resolver",
    "aws.rum",
    "aws.s3",
    "aws.s3-outposts",
    "aws.sagemaker",
    "aws.savingsplans",
    "aws.scheduler",
    "aws.schemas",
    "aws.sdb",
    "aws.secretsmanager",
    "aws.securityhub",
    "aws.serverlessrepo",
    "aws.servicecatalog",
    "aws.servicediscovery",
    "aws.servicequotas",
    "aws.ses",
    "aws.shield",
    "aws.signer",
    "aws.signin",
    "aws.sms",
    "aws.snowball",
    "aws.sns",
    "aws.sqs",
    "aws.ssm",
    "aws.ssm-contacts",
    "aws.ssm-incidents",
    "aws.sso",
    "aws.states",
    "aws.storagegateway",
    "aws.sts",
    "aws.support",
    "aws.swf",
    "aws.synthetics",
    "aws.tag",
    "aws.textract",
    "aws.timestream",
    "aws.transcribe",
    "aws.transfer",
    "aws.translate",
    "aws.trustedadvisor",
    "aws.voiceid",
    "aws.waf",
    "aws.waf-regional",
    "aws.wafv2",
    "aws.wellarchitected",
    "aws.wisdom",
    "aws.workdocs",
    "aws.worklink",
    "aws.workmail",
    "aws.workspaces",
    "aws.xray",
];

/// The service-owned registries a fresh store holds. Every entry of the catalogue
/// lives in [`AWS_MANAGED_REGISTRY`], the one registry AWS publishes event schemas in.
pub const AWS_MANAGED_REGISTRIES: &[&str] = &[AWS_MANAGED_REGISTRY];

/// One AWS event type the service-owned catalogue holds a schema for.
///
/// It is the event's IDENTITY plus the shape of its `detail`; the OpenAPI document
/// itself is rendered from them by [`event_schema_document`], because the envelope
/// around `detail` is the same for every AWS event.
#[derive(Debug, Clone, Copy)]
pub struct ManagedEventType {
    /// The event source that publishes it: `aws.<service>`.
    pub source: &'static str,
    /// The `detail-type` EventBridge delivers the event under.
    pub detail_type: &'static str,
    /// The name the catalogue publishes the event type under: the `<EventName>` half
    /// of `<source>@<EventName>`, which AWS derives from the `detail_type`.
    pub event_name: &'static str,
    /// The document's `components.schemas` minus the envelope: the component the
    /// envelope's `detail` references (the one named [`Self::event_name`]) together
    /// with everything that component references in turn.
    pub components: &'static str,
}

impl ManagedEventType {
    /// One member of the CloudTrail-delivered event family of `source` — the family
    /// every AWS event source publishes, so it is rendered per source rather than
    /// written out per source. See [`CLOUD_TRAIL_EVENT_FAMILY`].
    pub const fn cloud_trail(
        source: &'static str,
        detail_type: &'static str,
        event_name: &'static str,
    ) -> Self {
        Self {
            source,
            detail_type,
            event_name,
            components: CLOUD_TRAIL_COMPONENTS,
        }
    }

    /// `<source>@<EventName>` — the name the catalogue publishes this event type
    /// under, which is what `ListSchemas` reports and `DescribeSchema` takes.
    pub fn schema_name(&self) -> String {
        format!("{}@{}", self.source, self.event_name)
    }

    /// This event type's `components.schemas`, with the event's own component name
    /// substituted for [`EVENT_COMPONENT_PLACEHOLDER`]: one template serves a whole
    /// family of event types, each published under its own component name.
    pub fn components_json(&self) -> String {
        self.components
            .replace(EVENT_COMPONENT_PLACEHOLDER, self.event_name)
    }

    /// This event type as the catalogue holds it. `None` when its document cannot be
    /// rendered — see [`event_schema_document`].
    pub fn schema(&self) -> Option<ManagedSchema> {
        let name = self.schema_name();
        Some(ManagedSchema {
            description: managed_schema_description(&name),
            content: event_schema_document(
                self.source,
                self.detail_type,
                self.event_name,
                &self.components_json(),
            )?,
            last_modified: managed_last_modified(&name),
            version_count: managed_version_count(&name),
            name,
        })
    }
}

/// One schema of the service-owned catalogue, as [`aws_managed_schemas`] renders it
/// and the store seeds it.
#[derive(Debug, Clone)]
pub struct ManagedSchema {
    /// `<source>@<EventName>`, the naming the catalogue publishes under.
    pub name: String,
    /// The sentence AWS describes a catalogue entry with — see
    /// [`managed_schema_description`].
    pub description: Option<String>,
    /// The `OpenApi3` document of the event type, as `DescribeSchema` serves it and
    /// `ExportSchema` converts it.
    pub content: String,
    /// When the published schema was last modified — see [`managed_last_modified`].
    pub last_modified: SystemTime,
    /// How many versions the catalogue publishes for it — see
    /// [`managed_version_count`].
    pub version_count: u64,
}

/// When the AWS-managed catalogue's entry named `schema_name` was last modified.
///
/// `LastModified` is "the date and time that schema was modified": a property of the
/// PUBLISHED schema, which is what a client compares against its cached copy to decide
/// whether that copy is stale. So what the member has to satisfy is that it is the
/// SCHEMA's own instant — fixed, at whole-second precision, the same in every process
/// and across every reset of this store, and distinct between schemas that were
/// published at different times. A reading of the clock satisfies none of that: it
/// makes every entry report the same instant, moves that instant on every boot, and so
/// invalidates every client's cache on every run.
///
/// AWS publishes no rule for the instant itself, so it is derived from the one thing
/// that identifies the schema and never changes — its name — by a stable digest, and
/// placed in the window the catalogue has been maintained over ([`CATALOGUE_EPOCH`],
/// [`CATALOGUE_SPAN`]). Derived, not stored: the catalogue is built from constants, so
/// every process derives the same instant for the same name.
pub fn managed_last_modified(schema_name: &str) -> SystemTime {
    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(schema_name.as_bytes());
    let mut offset = [0u8; 8];
    offset.copy_from_slice(&digest[..8]);
    let seconds = u64::from_be_bytes(offset) % CATALOGUE_SPAN_SECONDS;
    SystemTime::UNIX_EPOCH + Duration::from_secs(CATALOGUE_EPOCH_SECONDS + seconds)
}

/// The start of the window the AWS-managed catalogue's publication instants fall in,
/// in seconds since the Unix epoch: 2019-12-01T00:00:00Z, the month the EventBridge
/// schema registry became available, so no entry claims to have been published before
/// the registry existed.
const CATALOGUE_EPOCH_SECONDS: u64 = 1_575_158_400;

/// How wide that window is: six years of whole seconds, which is long enough that no
/// two entries of a catalogue this size are forced onto the same instant and short
/// enough that every instant in it is in the past.
const CATALOGUE_SPAN_SECONDS: u64 = 6 * 365 * 24 * 60 * 60;

/// How far apart the catalogue's versions of one schema are published.
///
/// AWS publishes no interval, so this span is the emulator's own: what the store owes
/// is that version N of a schema was created before version N+1 and that the newest
/// version's creation is the schema's `LastModified`, so the versions are laid out
/// backwards from that instant, one month apart.
const MANAGED_VERSION_INTERVAL: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// The description a catalogue entry carries, derived from its name.
///
/// AWS describes every schema in the managed catalogue with one sentence built from
/// the schema's own name: a `<source>@<EventName>` schema is "Schema for event type
/// `<EventName>`, published by AWS service `<source>`". `None` for a name that is not
/// of that form, which describes no event type and so has no such sentence.
pub fn managed_schema_description(schema_name: &str) -> Option<String> {
    let (source, event_name) = schema_name.split_once('@')?;
    Some(format!(
        "Schema for event type {event_name}, published by AWS service {source}"
    ))
}

/// Render the `OpenApi3` document of one AWS event type: the envelope every AWS event
/// shares, wrapped around the `components` of its `detail`.
///
/// The envelope is the same for every AWS event — the EventBridge event fields
/// (`detail-type`, `source`, `time`, `region`, `account`, `id`, `resources`,
/// `version`) with `detail` referencing the event-specific component — so it is
/// rendered here rather than repeated per entry.
///
/// The document's `info.title` is the sentence the catalogue titles a published event
/// schema with: the event's `detail-type`, then which source publishes it
/// (`AWS API Call via CloudTrail published by aws.events`). It names the EVENT, not the
/// schema's component, so a client that renders the title shows the event the document
/// describes; `ExportSchema` carries the same title onto the draft-04 translation.
///
/// The `x-amazon-events-detail-type` and `x-amazon-events-source` extensions AWS's
/// documents carry are stamped on BOTH the envelope and the event's own component: they
/// record which event the document is for, and that is what the registry indexes a
/// schema under (see `SearchSchemas`), so they travel with the component a converted
/// document keeps.
///
/// `None` when `components` is not a JSON object, or does not define the component the
/// envelope's `detail` must reference (the one named after the event) as an object:
/// that is a document with nothing to describe, and the catalogue leaves it out rather
/// than seeding a schema whose `detail` dangles.
fn event_schema_document(
    source: &str,
    detail_type: &str,
    event_name: &str,
    components: &str,
) -> Option<String> {
    let mut schemas = match ::serde_json::from_str(components).ok()? {
        ::serde_json::Value::Object(schemas) => schemas,
        _ => return None,
    };
    let event = schemas.get_mut(event_name)?.as_object_mut()?;
    event.insert(
        "x-amazon-events-detail-type".to_string(),
        ::serde_json::Value::String(detail_type.to_string()),
    );
    event.insert(
        "x-amazon-events-source".to_string(),
        ::serde_json::Value::String(source.to_string()),
    );
    schemas.insert(
        EVENT_ENVELOPE_COMPONENT.to_string(),
        ::serde_json::json!({
            "type": "object",
            "required": [
                "detail-type", "resources", "detail", "id", "source", "time",
                "region", "version", "account"
            ],
            "x-amazon-events-detail-type": detail_type,
            "x-amazon-events-source": source,
            "properties": {
                "account": {"type": "string"},
                "detail": {"$ref": format!("{OPENAPI_COMPONENT_REF}{event_name}")},
                "detail-type": {"type": "string"},
                "id": {"type": "string"},
                "region": {"type": "string"},
                "resources": {"type": "array", "items": {"type": "string"}},
                "source": {"type": "string"},
                "time": {"type": "string", "format": "date-time"},
                "version": {"type": "string"}
            }
        }),
    );
    ::serde_json::to_string(&::serde_json::json!({
        "openapi": "3.0.0",
        "info": {"version": "1.0.0", "title": managed_schema_title(source, detail_type)},
        "paths": {},
        "components": {"schemas": schemas}
    }))
    .ok()
}

/// The title the catalogue gives the document of one published event type:
/// `<detail-type> published by <source>`.
///
/// Shared with `ExportSchema`, which reports the source document's title on the
/// draft-04 translation it renders, so the two cannot drift.
pub fn managed_schema_title(source: &str, detail_type: &str) -> String {
    format!("{detail_type} published by {source}")
}

/// Every event type the service-owned catalogue holds a schema for: the whole
/// CloudTrail-delivered event family ([`CLOUD_TRAIL_EVENT_FAMILY`]) of every source in
/// [`AWS_EVENT_SOURCES`], followed by the service-specific event types in
/// [`AWS_SERVICE_EVENT_TYPES`].
///
/// The service-specific entries come last, so one of them wins over a generated entry
/// of the same name: the store seeds entries in this order and keys them by name.
pub fn aws_managed_event_types() -> Vec<ManagedEventType> {
    AWS_EVENT_SOURCES
        .iter()
        .flat_map(|source| {
            CLOUD_TRAIL_EVENT_FAMILY
                .iter()
                .map(move |(detail_type, event_name)| {
                    ManagedEventType::cloud_trail(source, detail_type, event_name)
                })
        })
        .chain(AWS_SERVICE_EVENT_TYPES.iter().copied())
        .collect()
}

/// The whole service-owned catalogue, one entry per schema: every event type
/// [`aws_managed_event_types`] names, as the name, description and document the store
/// seeds and the read operations report.
///
/// Rendered ONCE per process and handed out by reference afterwards: the catalogue is
/// built from constants, so it is the same in every store, and rendering it renders
/// every member's document.
pub fn aws_managed_schemas() -> &'static [ManagedSchema] {
    static CATALOGUE: ::std::sync::OnceLock<Vec<ManagedSchema>> = ::std::sync::OnceLock::new();
    CATALOGUE.get_or_init(|| {
        aws_managed_event_types()
            .iter()
            .map(|event| {
                // A row whose `components` do not parse, or do not define the
                // component named after the event, describes no event type. That is a
                // typo in the seed data above rather than a runtime condition — the
                // rows are constants — so it fails here, in every build, rather than
                // quietly shrinking the catalogue the service serves.
                event.schema().unwrap_or_else(|| {
                    panic!(
                        "the seeded event type {}@{} must render a document",
                        event.source, event.event_name
                    )
                })
            })
            .collect()
    })
}

/// Build the service-owned registries and seed them with [`aws_managed_schemas`].
///
/// Their ARNs are left empty: a service-owned registry is minted by no request, so no
/// request's region is stored for it, and [`Registry::arn_in`] derives the ARN from
/// the region of the request asking. A service-owned SCHEMA carries no ARN at all —
/// see [`Registry::schema_arn_in`].
///
/// Nothing here reads the clock: every instant a catalogue entry reports is the
/// published schema's own, derived from its name ([`managed_last_modified`]), so two
/// stores seeded at two different moments report the same catalogue.
fn aws_managed_registries() -> BTreeMap<String, Registry> {
    let mut registries: BTreeMap<String, Registry> = AWS_MANAGED_REGISTRIES
        .iter()
        .map(|name| {
            (
                (*name).to_string(),
                Registry {
                    name: (*name).to_string(),
                    arn: String::new(),
                    description: None,
                    tags: BTreeMap::new(),
                    scope: RegistryScope::Aws,
                    // A service-owned registry carries no caller-written policy.
                    policy: None,
                    schemas: BTreeMap::new(),
                },
            )
        })
        .collect();

    let Some(registry) = registries.get_mut(AWS_MANAGED_REGISTRY) else {
        return registries;
    };
    for entry in aws_managed_schemas() {
        // Every version the catalogue publishes is stored, so the `VersionCount` a
        // listing reports is the count of versions this store actually holds and a
        // client that pins one of them can read it. They are laid out backwards from
        // the schema's last-modified instant: the newest version was created then, and
        // each earlier one a [`MANAGED_VERSION_INTERVAL`] before its successor. The
        // document is the one AWS publishes for the schema, which is the document of
        // its newest version — AWS publishes no earlier revision of it, so an earlier
        // version carries the same document rather than one invented here.
        let versions: BTreeMap<u64, SchemaVersion> = (1..=entry.version_count.max(1))
            .map(|version| {
                let age = MANAGED_VERSION_INTERVAL * u32::try_from(entry.version_count - version)
                    .unwrap_or(u32::MAX);
                (
                    version,
                    SchemaVersion {
                        version,
                        content: entry.content.clone(),
                        r#type: AWS_MANAGED_SCHEMA_TYPE.to_string(),
                        created_date: entry
                            .last_modified
                            .checked_sub(age)
                            .unwrap_or(entry.last_modified),
                    },
                )
            })
            .collect();
        registry.schemas.insert(
            entry.name.clone(),
            Schema {
                name: entry.name.clone(),
                arn: String::new(),
                description: entry.description.clone(),
                r#type: AWS_MANAGED_SCHEMA_TYPE.to_string(),
                tags: BTreeMap::new(),
                latest_version: entry.version_count.max(1),
                last_modified: entry.last_modified,
                versions,
                minted_by_token: BTreeMap::new(),
                code_bindings: BTreeMap::new(),
            },
        );
    }
    registries
}

/// The languages this service generates code bindings for, in the identifiers the
/// API takes in the `Language` URI label.
///
/// Source: the `Language` member is modelled as a plain `__string` with no enum and
/// no pattern (`smithy_query(query="operation",operation="PutCodeBinding")`), so the
/// accepted set is the SERVICE's, not the model's. The service states it in the
/// refusal it answers a bad `Language` with — "Parameter CodeBindingLanguage doesn't
/// match pattern `^(Java8|TypeScript3|Python36|Go1)$`" — i.e. the code-binding
/// languages EventBridge Schemas generates for: Java 8+, TypeScript 3+, Python 3.6+
/// and Go 1.x. Anything else is a language this service cannot generate, and is
/// refused rather than recorded.
///
/// The order is the pattern's: the alternation in the refusal below is built from
/// this list, so the two cannot drift apart.
pub const CODE_BINDING_LANGUAGES: &[&str] = &["Java8", "TypeScript3", "Python36", "Go1"];

/// The name the service's own validation gives the `Language` URI label it checks
/// against [`code_binding_language_pattern`].
pub const CODE_BINDING_LANGUAGE_PARAMETER: &str = "CodeBindingLanguage";

/// The pattern an acceptable `Language` matches: the alternation of
/// [`CODE_BINDING_LANGUAGES`], anchored at both ends, which is why a language family
/// without its version suffix and a differently-cased spelling are both refused.
pub fn code_binding_language_pattern() -> String {
    format!("^({})$", CODE_BINDING_LANGUAGES.join("|"))
}

/// The canonical language identifier a request named, or `None` when it named a
/// language this service does not generate bindings for.
///
/// The comparison is exact: these identifiers are wire values, and a service that
/// quietly accepted another spelling would persist a binding under a language a
/// later read cannot ask for.
pub fn code_binding_language(requested: &str) -> Option<&'static str> {
    CODE_BINDING_LANGUAGES
        .iter()
        .copied()
        .find(|language| *language == requested)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSchemasSchemasState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSchemasSchemasState>)
}

/// The region a request's ARNs name when the request carries no SigV4 scope to read
/// it from. It is the region the emulator writes into the shared AWS config it hands
/// clients (`vendor/server/src/lib.rs`, `[default] region = us-east-1`).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The region this request was signed for.
///
/// An ARN is regional, and the region a client's ARNs must carry is the one it
/// signed with, so it is read from the SigV4 credential scope rather than fixed.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// `arn:aws:schemas:<region>:<account>:registry/<registry>`.
pub fn registry_arn(region: &str, registry: &str) -> String {
    ::engine::emulator_arn("schemas", region, &format!("registry/{registry}"))
}

/// `arn:aws:schemas:<region>:<account>:schema/<registry>/<schema>`.
pub fn schema_arn(region: &str, registry: &str, schema: &str) -> String {
    ::engine::emulator_arn("schemas", region, &format!("schema/{registry}/{schema}"))
}

/// Store the `Tags` member of a request. An absent map and an empty one are the same
/// thing: a resource with no tags.
pub fn tags_from_input(tags: Option<&HashMap<String, String>>) -> BTreeMap<String, String> {
    tags.map(|tags| {
        tags.iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<BTreeMap<_, _>>()
    })
    .unwrap_or_default()
}

/// Render a stored tag set for a response. Always present — an untagged resource
/// reports an empty map, not an absent member.
pub fn tags_to_output(tags: &BTreeMap<String, String>) -> Option<HashMap<String, String>> {
    Some(
        tags.iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<HashMap<_, _>>(),
    )
}

/// One page of a key-ordered listing.
#[derive(Debug)]
pub struct Page<T> {
    /// The items this page returns, in key order.
    pub items: Vec<T>,
    /// The token that resumes after the last item, or `None` on the last page.
    pub next_token: Option<String>,
}

/// Mint a `NextToken` that resumes strictly after `last_key`, for a listing selected
/// by `selection`.
///
/// Both halves are hex, so the token is a single opaque word: it is safe in a query
/// string whatever the key or the selection contains, a client cannot mistake it for
/// a value it may construct, and the two halves cannot run into each other.
///
/// The selection is carried IN the token because a token names a position within one
/// listing: resuming a differently-filtered listing from it would skip or repeat
/// items with no error. A token presented with another selection is therefore not a
/// token for that listing, and is refused.
fn encode_next_token(selection: &str, last_key: &str) -> String {
    format!("{}.{}", ::hex::encode(selection), ::hex::encode(last_key))
}

/// Read back the key a `NextToken` [`encode_next_token`] minted for `selection`
/// resumes after. `None` for anything else, including a token minted for a different
/// selection.
fn decode_next_token(selection: &str, token: &str) -> Option<String> {
    let (encoded_selection, encoded_key) = token.split_once('.')?;
    if ::hex::decode(encoded_selection).ok()? != selection.as_bytes() {
        return None;
    }
    String::from_utf8(::hex::decode(encoded_key).ok()?).ok()
}

/// Take one page out of a key-ordered sequence.
///
/// `entries` must be ordered by key (a `BTreeMap`'s iteration order is). The page
/// starts strictly after the key `token` encodes and holds at most `page_size` items;
/// `next_token` is minted only when a further entry exists, so the last page of a
/// listing carries none.
///
/// `page_size` is a count, never "unbounded": a listing that materialised its whole
/// selection into one response would be a response-amplification path out of a store
/// the caller fills. Callers resolve it through [`requested_page_size`].
///
/// `selection` identifies the listing being walked — every request member that
/// decides WHICH items it contains (the parent resource, a name prefix, a scope).
/// It is bound into the token and checked when one comes back, so a token cannot
/// resume a listing it was not issued for.
///
/// Returns `Err` for a token this service did not mint for this selection — the
/// caller answers that with its own modelled client error.
pub fn page_by_key<K: AsRef<str>, V>(
    entries: impl Iterator<Item = (K, V)>,
    selection: &str,
    token: Option<&str>,
    page_size: usize,
) -> Result<Page<V>, InvalidToken> {
    let after = match token {
        None => None,
        Some(token) => Some(decode_next_token(selection, token).ok_or(InvalidToken)?),
    };
    let mut remaining =
        entries.filter(|(key, _)| after.as_deref().is_none_or(|after| key.as_ref() > after));

    let mut items = Vec::new();
    let mut last_key = String::new();
    while items.len() < page_size {
        match remaining.next() {
            Some((key, value)) => {
                last_key = key.as_ref().to_string();
                items.push(value);
            }
            // The sequence ran out before the page filled: this is the last page.
            None => {
                return Ok(Page {
                    items,
                    next_token: None,
                })
            }
        }
    }

    // The page filled. Only a sequence that still has an entry gets a token, so a
    // listing whose total is an exact multiple of `limit` does not hand back a token
    // that would resolve to an empty page.
    let next_token = remaining
        .next()
        .map(|_| encode_next_token(selection, &last_key));
    Ok(Page { items, next_token })
}

/// A `NextToken` this service did not mint for the listing it was presented to.
#[derive(Debug)]
pub struct InvalidToken;

/// The ceiling on how many registries this store holds, on how many schemas one
/// registry holds, and on the total schema content it retains: the caps that bound
/// caller-driven growth.
///
/// A generated emulator keeps its resources in memory for the life of the process and
/// a create is the only thing that adds one, so the store needs explicit maxima —
/// neither a TTL nor an eviction policy is available to it, because `DescribeSchema`
/// must still answer what `CreateSchema` created. Without them, one `CreateRegistry`
/// per caller-chosen `RegistryName` and one `CreateSchema` per caller-chosen
/// `SchemaName` grow the store for as long as a client keeps calling.
///
/// The entry counts bound the PRODUCT at
/// `MAX_REGISTRIES * MAX_SCHEMAS_PER_REGISTRY` schemas, and
/// [`MAX_STORED_CONTENT_BYTES`] bounds what those entries can retain: the model caps
/// one `Content` at 100 000 characters, so the entry counts alone would still admit
/// that many hundred-kilobyte payloads. Both creates refuse past their cap with the
/// service's own `BadRequestException` (HTTP 400) — this model declares no quota
/// error, and a client error is the honest class: the store cannot accept the
/// request, and no retry of it will succeed.
///
/// The numbers are the emulator's own bound, not a published AWS default: AWS
/// publishes these quotas as adjustable, so no single value is the contract, and they
/// are set high enough that no ordinary workflow meets one.
pub const MAX_REGISTRIES: usize = 500;

/// See [`MAX_REGISTRIES`].
pub const MAX_SCHEMAS_PER_REGISTRY: usize = 500;

/// See [`MAX_REGISTRIES`]. Counted over every version of every schema in every
/// registry a CALLER owns, which is everything a caller's `Content` can make this
/// service retain; the service-owned catalogue is fixed at compile time, so it is not
/// part of what this budget bounds.
///
/// `CreateSchema` writes a schema's first version and `UpdateSchema` writes every
/// later one, so a single schema's versions are bounded on their own by
/// [`MAX_VERSIONS_PER_SCHEMA`] beside this byte budget.
pub const MAX_STORED_CONTENT_BYTES: usize = 64 * 1024 * 1024;

/// The most versions one schema may hold. See [`MAX_REGISTRIES`] for why the store
/// needs explicit maxima at all: `UpdateSchema` mints a version per call, so without
/// this one schema grows for as long as a client keeps updating it, under the byte
/// budget alone.
///
/// Like the other entry caps this is the emulator's own bound, not a published AWS
/// default, and is set high enough that no ordinary workflow meets it.
pub const MAX_VERSIONS_PER_SCHEMA: usize = 500;

/// The largest `Policy` document `PutResourcePolicy` will store for one registry.
///
/// `Policy` is modelled as a plain `__string`, so the model bounds it nowhere, and a
/// policy is retained for the life of the process like every other resource here —
/// the same reason [`MAX_REGISTRIES`] exists. One policy per registry, each at most
/// this many bytes, bounds what policies can make this store retain. The number is
/// the emulator's own bound rather than a published AWS quota, and is far larger than
/// any real policy document.
pub const MAX_POLICY_BYTES: usize = 64 * 1024;

/// See [`MAX_REGISTRIES`]. The ceiling on how many code bindings one schema holds.
///
/// A binding is keyed by schema version and language. The language is one of the
/// finitely many [`CODE_BINDING_LANGUAGES`], so what a caller can grow is the number
/// of VERSIONS it starts bindings for; this cap is the explicit ceiling on the
/// product, kept so the binding map is bounded however many versions a schema comes
/// to hold. A put past the cap is refused with the service's own
/// `BadRequestException`, as the other caps are.
pub const MAX_CODE_BINDINGS_PER_SCHEMA: usize = 500;

/// The most items one page of a listing may hold, whatever `Limit` asks for.
///
/// Without it, an absent or very large `Limit` would materialise the whole selected
/// store into a single response — the amplifying half of an unbounded store. A
/// listing that has more to give says so with a `NextToken`, which is how a caller
/// reads past one page, so clamping loses nothing.
pub const MAX_PAGE_SIZE: usize = 100;

/// The most keywords one `SearchSchemas` request may name.
///
/// A keyword search costs the PRODUCT of two factors: how many keywords the request
/// names, and how much stored text each one is matched against. The stored side is
/// already bounded ([`MAX_STORED_CONTENT_BYTES`]), but the request side is not —
/// `Keywords` is one query-string value that yields one more keyword for every
/// separator a caller puts in it, so without this cap a single small GET could ask
/// for an arbitrary multiple of a scan of the whole store. With it, one request costs
/// at most `MAX_KEYWORDS_PER_SEARCH` passes over the text of the registry it names.
///
/// Refused past the cap with the service's own `BadRequestException` (HTTP 400): this
/// model declares no quota error, and the store cannot serve the request as asked, so
/// no retry of it will succeed. The number is the emulator's own bound, not a
/// published AWS limit, and is set high enough that no ordinary search meets it.
pub const MAX_KEYWORDS_PER_SEARCH: usize = 10;

/// The most named schemas one inferred document may carry — one per object position
/// the sample shows, plus the root.
///
/// The model bounds the sample itself: `Events` is `@length(min: 1, max: 10)` and an
/// entry is `@length(min: 1, max: 100000)`
/// (`smithy_query(query="shape", shape="__listOfGetDiscoveredSchemaVersionItemInput")`),
/// so the framework refuses more than a megabyte of sampled events before any handler
/// runs. What that does NOT bound is how many distinct OBJECTS a sample of that size
/// describes — a bare `{"m":{}}` is a dozen bytes — and each one becomes a named schema
/// the document holds, names and writes. A thousand distinct shapes in one sample is
/// already past anything a schema a client generates code from describes.
///
/// Refused past the cap with `BadRequestException` (HTTP 400), as the other bounds of
/// this service are: the request cannot be served as asked, so no retry of it will
/// succeed. The emulator's own bound, not a published AWS limit.
pub const MAX_INFERRED_SCHEMAS: usize = 1_000;

/// Refuse a sample that would describe more schemas than one document may carry. Names
/// the member, the bound and what the sample showed — never the sample itself, which is
/// the caller's data.
pub fn too_many_inferred_schemas(named: usize) -> BadRequestException {
    bad_request(format!(
        "Events describe {named} distinct objects, but an inferred schema document \
         carries at most {MAX_INFERRED_SCHEMAS}."
    ))
}

/// The page size to serve a request whose `Limit` member is `limit`.
///
/// `Err` when the request asked for a page of fewer than one item: the model gives
/// `Limit` no range, so that bound is the handler's, and a page of zero items is not
/// a page. Anything larger than [`MAX_PAGE_SIZE`] is served as `MAX_PAGE_SIZE`, and
/// an absent `Limit` means "as much as one page may hold".
pub fn requested_page_size(limit: Option<i32>) -> Result<usize, BadRequestException> {
    match limit {
        None => Ok(MAX_PAGE_SIZE),
        Some(limit) if limit >= 1 => Ok(usize::try_from(limit)
            .unwrap_or(MAX_PAGE_SIZE)
            .min(MAX_PAGE_SIZE)),
        Some(limit) => Err(limit_out_of_range(limit)),
    }
}

/// How many bytes of schema `Content` this store currently retains on a caller's
/// behalf, across every version of every schema in every registry the caller owns —
/// what [`MAX_STORED_CONTENT_BYTES`] bounds.
///
/// Walked on demand rather than tracked in a counter: a counter has to be decremented
/// on every delete path too, and a counter that drifts reports a budget the store is
/// not actually using.
pub fn stored_content_bytes(state: &ComAmazonawsSchemasSchemasState) -> usize {
    state
        .registries
        .values()
        .filter(|registry| !registry.scope.is_managed())
        .flat_map(|registry| registry.schemas.values())
        .flat_map(|schema| schema.versions.values())
        .map(|version| version.content.len())
        .sum()
}

/// How many registries the caller owns — what [`MAX_REGISTRIES`] bounds. The
/// service-owned catalogue is not a caller's and does not consume the caller's
/// allowance.
pub fn local_registry_count(state: &ComAmazonawsSchemasSchemasState) -> usize {
    state
        .registries
        .values()
        .filter(|registry| !registry.scope.is_managed())
        .count()
}

/// The `$schema` a JSON Schema draft-4 document carries — what makes a document
/// draft 4 rather than some other draft.
///
/// Shared: `GetDiscoveredSchema` stamps it on the document it infers and
/// `ExportSchema` on the document it converts, so one constant keeps the two from
/// naming two different drafts.
pub const JSON_SCHEMA_DRAFT4_URI: &str = "http://json-schema.org/draft-04/schema#";

// ---------------------------------------------------------------------------
// Error codes
//
// Every error shape this service models carries two REQUIRED members, `Code`
// ("The error code.") and `Message`, and the two say different things. The
// exception SHAPE name is what the protocol already carries, in the
// `x-amzn-errortype` header (`OperationError::shape_name`, see
// `crate::scaffolded`); `Code` carries the service's own code for the CONDITION
// that was raised, which is what a client branching on the SDK's error-code
// accessor reads. So the two are never the same string: a condition-level code
// goes in `Code` and the shape name is left to the header.
//
// The codes below are the ones this service answers with; each is named once
// here so no handler can spell one differently.
// ---------------------------------------------------------------------------

/// `Code` for a resource the store does not hold — a registry, a schema, a schema
/// version, a code binding or a resource-based policy. One code for every
/// not-found condition: what is missing is said in the `Message`, not the code.
pub const CODE_RESOURCE_NOT_FOUND: &str = "ResourceNotFound";

/// `Code` for a create whose resource already exists, the conflict an idempotent
/// create-or-reuse caller keys on.
pub const CODE_RESOURCE_ALREADY_EXIST: &str = "ResourceAlreadyExist";

/// `Code` for a request the service will not act on as sent: a member outside the
/// constraint the service enforces, or content that is not the document it must be.
pub const CODE_MALFORMED_REQUEST: &str = "MalformedRequest";

/// `Code` for a request that conflicts with the state of the resource it names —
/// both the 409 conflicts that are not "already exists" and the 412 a stale
/// `RevisionId` precondition fails with, which an optimistic-concurrency retry loop
/// keys on.
pub const CODE_CONFLICT: &str = "Conflict";

/// `Code` for a caller that may not perform the action on this resource.
pub const CODE_FORBIDDEN: &str = "Forbidden";

/// `Code` for a fault on the service's side rather than in the request.
pub const CODE_INTERNAL_SERVER_ERROR: &str = "InternalServerError";

/// A `NotFoundException` for a resource this store does not hold.
///
/// `Code` is [`CODE_RESOURCE_NOT_FOUND`], never the shape name: see the note above.
pub fn not_found(message: impl Into<String>) -> NotFoundException {
    NotFoundException {
        code: CODE_RESOURCE_NOT_FOUND.to_string(),
        message: message.into(),
    }
}

/// A `ConflictException` for a create whose resource already exists.
/// `Code` is [`CODE_RESOURCE_ALREADY_EXIST`].
pub fn already_exists(message: impl Into<String>) -> ConflictException {
    ConflictException {
        code: CODE_RESOURCE_ALREADY_EXIST.to_string(),
        message: message.into(),
    }
}

/// A `ConflictException` for a request that conflicts with the state of the
/// resource it names, other than a resource that already exists.
/// `Code` is [`CODE_CONFLICT`].
pub fn conflict(message: impl Into<String>) -> ConflictException {
    ConflictException {
        code: CODE_CONFLICT.to_string(),
        message: message.into(),
    }
}

/// A `BadRequestException`. `Code` is [`CODE_MALFORMED_REQUEST`].
pub fn bad_request(message: impl Into<String>) -> BadRequestException {
    BadRequestException {
        code: CODE_MALFORMED_REQUEST.to_string(),
        message: message.into(),
    }
}

/// A `ForbiddenException`. `Code` is [`CODE_FORBIDDEN`].
pub fn forbidden(message: impl Into<String>) -> ForbiddenException {
    ForbiddenException {
        code: CODE_FORBIDDEN.to_string(),
        message: message.into(),
    }
}

/// A `PreconditionFailedException`: the request's precondition is not the state the
/// resource is in. `Code` is [`CODE_CONFLICT`] — the condition is a conflict with
/// the stored resource, and the 412 is what says the request stated a precondition.
pub fn precondition_failed(message: impl Into<String>) -> PreconditionFailedException {
    PreconditionFailedException {
        code: CODE_CONFLICT.to_string(),
        message: message.into(),
    }
}

/// An `InternalServerErrorException`. `Code` is [`CODE_INTERNAL_SERVER_ERROR`].
///
/// Raised only where a handler cannot reach its own store, which is a server fault.
pub fn internal_error() -> InternalServerErrorException {
    InternalServerErrorException {
        code: CODE_INTERNAL_SERVER_ERROR.to_string(),
        message: "The schemas emulator could not reach its own state store.".to_string(),
    }
}

/// Message for a registry no caller has created. The template is AWS's own, built
/// from the request's `RegistryName`.
pub fn registry_not_found(registry: &str) -> NotFoundException {
    not_found(format!("Registry with name {registry} does not exist."))
}

/// Message for a registry that holds no resource-based policy. The template is
/// AWS's own, built from the request's `RegistryName`.
pub fn policy_not_found(registry: &str) -> NotFoundException {
    not_found(format!(
        "Policy for registry with name '{registry}' does not exist."
    ))
}

/// Refuse a request that carries no `RegistryName`.
///
/// A resource policy is a property of ONE registry, so a request that does not name
/// a registry names no resource to read or write. Names the member, as a validation
/// error does.
pub fn registry_name_required() -> BadRequestException {
    bad_request(
        "RegistryName is required: a resource-based policy belongs to one registry, \
         so the request must name the registry it applies to.",
    )
}

/// Refuse a write whose `RevisionId` precondition is not the revision the stored
/// policy is at.
///
/// The service's sentence for this refusal is a CONSTANT: it names neither the
/// revision the caller sent nor the one the store holds. Nothing request-derived or
/// environment-derived belongs in it — the caller already holds what it sent, a
/// refusal is not a read of the resource (which is what tells the caller the current
/// revision), and a message carrying a minted id would differ from run to run for a
/// client or test that compares one.
pub fn stale_policy_revision() -> PreconditionFailedException {
    precondition_failed(
        "The RevisionId provided does not match the latest RevisionId for the registry policy.",
    )
}

/// Refuse a `Policy` larger than the store will retain for one registry. Names the
/// cap, so the caller knows a retry cannot help.
pub fn policy_too_large(sent: usize) -> BadRequestException {
    bad_request(format!(
        "Policy must be at most {MAX_POLICY_BYTES} bytes; the document sent is {sent} bytes."
    ))
}

/// Refuse a `Policy` that is not a JSON object.
///
/// `Policy` is modelled `@mediaType("application/json")` and a resource-based policy
/// document is a JSON object, so a value that is not one cannot be stored as a
/// policy. Names the member and what it must carry.
pub fn policy_not_json(detail: &str) -> BadRequestException {
    bad_request(format!(
        "Policy must be a JSON object holding a resource-based policy document: {detail}."
    ))
}

/// Message for a schema the named registry does not hold.
///
/// The sentence is the service's own template for a missing resource — "«Resource»
/// with name «name» does not exist.", as [`registry_not_found`] carries it — built
/// from the request's `SchemaName` and from that member ALONE, the way
/// [`schema_already_exists`] keeps the registry out of its sentence.
pub fn schema_not_found(schema: &str) -> NotFoundException {
    not_found(format!("Schema with name {schema} does not exist."))
}

/// Refuse a `CreateRegistry` for a name the store already holds. The template is
/// AWS's own, built from the request's `RegistryName`.
pub fn registry_already_exists(registry: &str) -> ConflictException {
    already_exists(format!("Registry with name {registry} already exists."))
}

/// Refuse a `CreateSchema` for a name its registry already holds. The template is
/// AWS's own, built from the request's `SchemaName` — and from that member ALONE:
/// AWS keeps the registry out of this sentence.
pub fn schema_already_exists(schema: &str) -> ConflictException {
    already_exists(format!("Schema with name {schema} already exists."))
}

/// Refuse a `DeleteRegistry` that would take a registry's schemas with it. Names no
/// request member: AWS's sentence for this refusal names none.
pub fn registry_not_empty() -> ConflictException {
    conflict("The registry you tried to delete is not empty.")
}

/// Refuse a caller's write to a service-owned registry or one of its schemas: AWS
/// owns that content, so a caller may read it but never change it.
pub fn managed_registry_is_read_only(registry: &str) -> ForbiddenException {
    forbidden(format!(
        "Registry {registry} is an AWS-managed registry; its contents are owned by AWS \
         and cannot be created, updated or deleted."
    ))
}

/// Message for a version the named schema does not hold — including a `SchemaVersion`
/// that is not a version number at all.
pub fn schema_version_not_found(registry: &str, schema: &str, requested: Option<&str>) -> NotFoundException {
    match requested {
        Some(version) => not_found(format!(
            "Schema {schema} in registry {registry} has no version {version}."
        )),
        None => not_found(format!(
            "Schema {schema} in registry {registry} has no versions."
        )),
    }
}

/// Message for a code binding nobody has started for this schema version and
/// language.
///
/// The template is AWS's own, built from the request's `SchemaName`, the resolved
/// version and the request's `Language`. It names no registry — AWS's sentence for
/// this refusal names none — so this helper takes none.
pub fn code_binding_not_found(schema: &str, version: u64, language: &str) -> NotFoundException {
    not_found(format!(
        "Code bindings for schema with name {schema} and version {version} language \
         {language} not found."
    ))
}

/// Refuse a `Language` this service generates no code bindings for.
///
/// The model gives `Language` no enum, so this is the service's own constraint, and
/// the refusal states it the way the service does: it names the parameter it checked
/// and the pattern that parameter must match ([`code_binding_language_pattern`]), so
/// a caller can fix the request from the message alone. The rejected value is not
/// echoed back — the pattern is what the caller needs.
pub fn unsupported_code_binding_language() -> BadRequestException {
    bad_request(format!(
        "Parameter {CODE_BINDING_LANGUAGE_PARAMETER} doesn't match pattern {}",
        code_binding_language_pattern()
    ))
}

/// Refuse a `Limit` the model cannot express a bound for: a page of fewer than one
/// item is not a page. Names the member and the bound, as a validation error does.
pub fn limit_out_of_range(limit: i32) -> BadRequestException {
    bad_request(format!("Limit must be at least 1, but was {limit}."))
}

/// Refuse a create that would take the store past a cap it keeps so caller-driven
/// growth stays bounded. Names the cap, so the caller knows a retry cannot help.
pub fn store_cap_reached(what: &str, cap: usize) -> BadRequestException {
    bad_request(format!(
        "This emulator holds at most {cap} {what}; delete one before creating another."
    ))
}

/// The JSON type of a value, for a refusal that names what the caller sent without
/// echoing it back. One wording for the whole service, so the refusals of
/// `UpdateSchema`'s `Content`, `PutResourcePolicy`'s `Policy` and
/// `GetDiscoveredSchema`'s `Events` cannot drift apart.
pub fn json_type_name(value: &::serde_json::Value) -> &'static str {
    match value {
        ::serde_json::Value::Null => "null",
        ::serde_json::Value::Bool(_) => "a boolean",
        ::serde_json::Value::Number(_) => "a number",
        ::serde_json::Value::String(_) => "a string",
        ::serde_json::Value::Array(_) => "an array",
        ::serde_json::Value::Object(_) => "an object",
    }
}

/// Refuse `Content` that is not a schema document at all.
///
/// A schema registry holds schema SOURCE, so content an `UpdateSchema` call would
/// store is parsed before it enters the store rather than retained blind. Both types
/// this service models are JSON documents whose top level is an object: "A JSON Schema
/// is a JSON document, and that document MUST be an object" (JSON Schema draft-04,
/// section 3.2), and an OpenAPI document "is itself a JSON object" (OpenAPI
/// Specification 3.0, "Document Structure"/"Format"). Content that does not parse as
/// JSON, or parses as a value that is not an object, is therefore a document of
/// neither modelled `Type`, and no reader of it could treat it as a schema.
///
/// What this does NOT do is check a document against the whole of its specification —
/// that every member OpenAPI requires is present, or that every draft-04 keyword is
/// well-formed. Content is stored verbatim and handed back verbatim by
/// `DescribeSchema` and `ExportSchema`, and partial documents are legitimate input to
/// those, so the refusal is scoped to content that is not a schema document at all.
///
/// `CreateSchema` and `UpdateSchema` both route through here: the requirement is a
/// property of the request, so it cannot differ between the two.
///
/// `schema_type` is named in the message only — the requirement is the same for both
/// of [`crate::ops::types::TYPE_VALUES`], and a third type cannot reach a handler
/// because the modelled enum is refused at the boundary before one runs.
///
/// The message names the member and what was wrong with it, the way a validation
/// error does; it echoes no content back. The refusal carries
/// [`CODE_MALFORMED_REQUEST`] (see [`bad_request`]): the body is not the document it
/// was declared to be.
pub fn validate_content(content: &str, schema_type: &str) -> Result<(), BadRequestException> {
    let document: ::serde_json::Value = ::serde_json::from_str(content).map_err(|error| {
        // AWS's own template for a body it cannot read as JSON, built from the
        // request: the member, then the character the body stops being JSON at, what
        // was being read there and that character's byte offset into `Content` — see
        // [`json_parse_detail`], which is where every refusal of an unreadable body in
        // this service gets its wording, so no two of them can point a client at two
        // different coordinate systems.
        bad_request(format!(
            "Content is not valid JSON: {}",
            json_parse_detail(content, &error)
        ))
    })?;
    if document.is_object() {
        return Ok(());
    }
    // The JSON type the content turned out to be, so the caller can see what it sent
    // without the content being echoed back.
    let kind = json_type_name(&document);
    Err(bad_request(format!(
        "Content is not a valid {schema_type} document: a schema document is a JSON object, \
         but this Content is {kind}."
    )))
}

/// Refuse one of `GetDiscoveredSchema`'s `Events` that discovery cannot read.
///
/// The message is built from the request: it names the INDEX of the event at
/// fault — `Events` is a list, so the index is what tells a caller sending a batch
/// WHICH event to fix — and then the reason that event could not be read. The
/// event itself is never echoed back: it is the caller's data.
pub fn event_not_parsable(index: usize, reason: impl std::fmt::Display) -> BadRequestException {
    bad_request(format!(
        "Fail to parse event index: [{index}]. Reason: {reason}"
    ))
}

/// Refuse a search that names more keywords than one request may carry. Names the
/// member, the bound and what the request asked for, as a validation error does.
pub fn too_many_keywords(named: usize) -> BadRequestException {
    bad_request(format!(
        "Keywords may name at most {MAX_KEYWORDS_PER_SEARCH} keywords, but this request named {named}."
    ))
}

/// Refuse a `NextToken` this service did not mint for this listing, without echoing
/// it back.
pub fn invalid_next_token() -> BadRequestException {
    bad_request(
        "The NextToken provided is not a token this service issued for this listing. \
         A NextToken resumes the listing it was returned by, so every request that \
         follows it must repeat that listing's selection unchanged.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeSet;

    /// The selection the helper's own tests page under; its shape does not matter
    /// here, only that it is the same one on both sides of a token.
    const SELECTION: &str = "prefix=;scope=Local";

    /// A listing shorter than the page size is one page and carries no token.
    #[test]
    fn a_single_short_page_has_no_token() {
        let entries = vec![("a", 1), ("b", 2)];
        let page = page_by_key(entries.into_iter(), SELECTION, None, 10).expect("fresh token");
        assert_eq!(page.items, vec![1, 2], "every entry is on the page");
        assert!(
            page.next_token.is_none(),
            "a short page must not carry a token, got {:?}",
            page.next_token
        );
    }

    /// A page filled exactly by the last entries carries no token either, so a
    /// client never follows a token to an empty page.
    #[test]
    fn an_exactly_filled_final_page_has_no_token() {
        let entries = vec![("a", 1), ("b", 2)];
        let page = page_by_key(entries.into_iter(), SELECTION, None, 2).expect("fresh token");
        assert!(
            page.next_token.is_none(),
            "an exactly-filled final page must not carry a token, got {:?}",
            page.next_token
        );
    }

    /// Following the token resumes strictly after the last item of the page before.
    #[test]
    fn the_token_resumes_after_the_last_item() {
        let entries = vec![("a", 1), ("b", 2), ("c", 3)];
        let first =
            page_by_key(entries.clone().into_iter(), SELECTION, None, 2).expect("fresh token");
        assert_eq!(first.items, vec![1, 2]);
        let token = first.next_token.expect("a further entry remains");
        let second =
            page_by_key(entries.into_iter(), SELECTION, Some(&token), 2).expect("own token");
        assert_eq!(second.items, vec![3], "the next page resumes after 'b'");
        assert!(second.next_token.is_none(), "and terminates");
    }

    /// A token this service did not mint is refused rather than silently restarting
    /// the listing from the top.
    #[test]
    fn a_foreign_token_is_refused() {
        let entries = vec![("a", 1)];
        for foreign in ["not-a-token", "zz", "6162", "", "."] {
            assert!(
                page_by_key(entries.clone().into_iter(), SELECTION, Some(foreign), 10).is_err(),
                "case {foreign:?}: a token this service did not mint must be refused"
            );
        }
    }

    /// A token names a position within ONE listing: presenting it to a listing with
    /// a different selection is refused, rather than silently skipping or repeating
    /// items.
    #[test]
    fn a_token_does_not_resume_a_differently_selected_listing() {
        let entries = vec![("a", 1), ("b", 2), ("c", 3)];
        let first =
            page_by_key(entries.clone().into_iter(), SELECTION, None, 1).expect("fresh token");
        let token = first.next_token.expect("further entries remain");
        assert!(
            page_by_key(
                entries.into_iter(),
                "prefix=b;scope=Local",
                Some(&token),
                1
            )
            .is_err(),
            "a token issued for one selection must not resume another"
        );
    }

    /// `Limit` resolves to a page size that is never zero and never larger than the
    /// server-side ceiling, so no single response can materialise the whole store.
    #[test]
    fn a_requested_page_size_is_clamped_to_the_server_ceiling() {
        let ceiling = i32::try_from(MAX_PAGE_SIZE).expect("the ceiling fits an i32");
        // Rows: absent, the published bounds of the clamp, one past each, and the
        // extremes of the modelled `__integer`.
        for (limit, expected) in [
            (None, Some(MAX_PAGE_SIZE)),
            (Some(1), Some(1)),
            (Some(ceiling - 1), Some(MAX_PAGE_SIZE - 1)),
            (Some(ceiling), Some(MAX_PAGE_SIZE)),
            (Some(ceiling + 1), Some(MAX_PAGE_SIZE)),
            (Some(i32::MAX), Some(MAX_PAGE_SIZE)),
            (Some(0), None),
            (Some(-1), None),
            (Some(i32::MIN), None),
        ] {
            let resolved = requested_page_size(limit).ok();
            assert_eq!(
                resolved, expected,
                "case limit={limit:?}: expected {expected:?}, got {resolved:?}"
            );
        }
    }

    /// The status a binding reports is one the model accepts, and it is the clock —
    /// not a read — that moves it: in progress until the instant generation is due,
    /// complete from then on, whoever asks and however often.
    #[test]
    fn a_code_binding_status_follows_the_clock() {
        let created = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let binding = CodeBinding {
            language: "Java8".to_string(),
            schema_version: 3,
            created_date: created,
            generation_due: created + CODE_GENERATION_DURATION,
        };

        // Rows: the put instant, just before the due instant, the due instant itself,
        // and long after it.
        for (offset, expected_status, expected_modified) in [
            (Duration::ZERO, CODE_GENERATION_IN_PROGRESS, created),
            (
                CODE_GENERATION_DURATION - Duration::from_millis(1),
                CODE_GENERATION_IN_PROGRESS,
                created,
            ),
            (
                CODE_GENERATION_DURATION,
                CODE_GENERATION_COMPLETE,
                created + CODE_GENERATION_DURATION,
            ),
            (
                Duration::from_secs(3600),
                CODE_GENERATION_COMPLETE,
                created + CODE_GENERATION_DURATION,
            ),
        ] {
            let report = binding.report_at(created + offset);
            assert!(
                crate::ops::types::CODE_GENERATION_STATUS_VALUES.contains(&report.status),
                "case +{offset:?}: {:?} is not a modelled CodeGenerationStatus",
                report.status
            );
            assert_eq!(
                report.status, expected_status,
                "case +{offset:?}: expected status {expected_status}, got {}",
                report.status
            );
            assert_eq!(
                report.last_modified, expected_modified,
                "case +{offset:?}: expected last-modified {expected_modified:?}, got {:?}",
                report.last_modified
            );
            assert_eq!(
                report.creation_date, created,
                "case +{offset:?}: the creation date never moves"
            );
            assert_eq!(
                report.schema_version, "3",
                "case +{offset:?}: expected the stored version in decimal, got {}",
                report.schema_version
            );
        }
    }

    /// The content budget counts every version of every schema in every registry —
    /// everything a caller's `Content` makes this service retain.
    #[test]
    fn the_content_budget_counts_every_stored_version() {
        let mut state = ComAmazonawsSchemasSchemasState::default();
        assert_eq!(
            stored_content_bytes(&state),
            0,
            "a store holding nothing of the caller's retains nothing on their behalf"
        );

        let now = std::time::SystemTime::now();
        for (registry_name, contents) in [("r1", ["ab", "cde"]), ("r2", ["f", "ghij"])] {
            let mut registry = Registry {
                name: registry_name.to_string(),
                arn: registry_arn(DEFAULT_REGION, registry_name),
                description: None,
                tags: BTreeMap::new(),
                scope: RegistryScope::Local,
                schemas: BTreeMap::new(),
                policy: None,
            };
            for (index, content) in contents.iter().enumerate() {
                let name = format!("s{index}");
                registry.schemas.insert(
                    name.clone(),
                    Schema {
                        arn: schema_arn(DEFAULT_REGION, registry_name, &name),
                        name,
                        description: None,
                        r#type: "OpenApi3".to_string(),
                        tags: BTreeMap::new(),
                        latest_version: 1,
                        last_modified: now,
                        code_bindings: BTreeMap::new(),
                        versions: std::iter::once((
                            1,
                            SchemaVersion {
                                version: 1,
                                content: (*content).to_string(),
                                r#type: "OpenApi3".to_string(),
                                created_date: now,
                            },
                        ))
                        .collect(),
                        minted_by_token: BTreeMap::new(),
                    },
                );
            }
            state.registries.insert(registry_name.to_string(), registry);
        }

        // 2 + 3 + 1 + 4, summed across both registries.
        assert_eq!(
            stored_content_bytes(&state),
            10,
            "every stored version's content counts towards the budget"
        );
    }

    /// Content that enters the store is a JSON schema document. The rows vary along
    /// the FORM the content takes — well-formed JSON objects of both modelled types,
    /// and every other shape a JSON body can have — not along one example of it.
    #[test]
    fn content_is_accepted_only_as_a_json_schema_document() {
        // An OpenAPI 3 document as the specification describes one: a JSON object
        // carrying `openapi` and `info`.
        const OPENAPI: &str = r#"{"openapi":"3.0.0","info":{"title":"t","version":"1.0.0"},"paths":{}}"#;
        for (case, content, schema_type, accepted) in [
            ("openapi-document", OPENAPI, "OpenApi3", true),
            (
                "openapi-with-components-and-extensions",
                r#"{"openapi":"3.0.3","info":{"title":"t","version":"1"},"paths":{},"components":{"schemas":{"E":{"type":"object","properties":{"id":{"type":"string"}}}}},"x-vendor":1}"#,
                "OpenApi3",
                true,
            ),
            // A partial document is still a document: content is stored and handed
            // back verbatim, so what is refused is content that is not one at all.
            ("openapi-version-only", r#"{"openapi":"3.0.0"}"#, "OpenApi3", true),
            ("openapi-empty-object", "{}", "OpenApi3", true),
            ("openapi-not-json", "not a schema at all", "OpenApi3", false),
            ("openapi-truncated-json", r#"{"openapi":"#, "OpenApi3", false),
            ("openapi-array", "[]", "OpenApi3", false),
            (
                "draft4-empty-object",
                "{}",
                // Draft 4 makes every keyword optional, so the empty schema is valid.
                "JSONSchemaDraft4",
                true,
            ),
            (
                "draft4-typical",
                r#"{"$schema":"http://json-schema.org/draft-04/schema#","type":"object","required":["id"]}"#,
                "JSONSchemaDraft4",
                true,
            ),
            ("draft4-array", "[]", "JSONSchemaDraft4", false),
            ("draft4-number", "42", "JSONSchemaDraft4", false),
            ("draft4-string", "\"schema\"", "JSONSchemaDraft4", false),
            ("draft4-boolean", "true", "JSONSchemaDraft4", false),
            ("draft4-null", "null", "JSONSchemaDraft4", false),
            ("draft4-empty-body", "", "JSONSchemaDraft4", false),
            ("draft4-truncated-json", "{\"type\":", "JSONSchemaDraft4", false),
            ("draft4-trailing-garbage", "{} }", "JSONSchemaDraft4", false),
        ] {
            let outcome = validate_content(content, schema_type);
            assert_eq!(
                outcome.is_ok(),
                accepted,
                "case {case:?}: expected accepted={accepted}, got {outcome:?}"
            );
            if let Err(refusal) = outcome {
                assert_eq!(
                    refusal.code, CODE_MALFORMED_REQUEST,
                    "case {case:?}: a refusal carries the service's error code for the \
                     condition, got {:?}",
                    refusal.code
                );
                assert!(
                    refusal.message.contains("Content"),
                    "case {case:?}: the message must name the member it is about, got {:?}",
                    refusal.message
                );
            }
        }
    }

    /// A fresh store already holds the service-owned catalogue: every entry of
    /// [`aws_managed_schemas`] is seeded into the AWS-scoped registry, with one
    /// stored version of the type AWS stores it as, the document rendered for it and
    /// the description derived from its name.
    #[test]
    fn a_fresh_store_holds_the_aws_managed_catalogue() {
        let state = ComAmazonawsSchemasSchemasState::default();
        let catalogue = aws_managed_schemas();
        assert_eq!(
            catalogue.len(),
            aws_managed_event_types().len(),
            "every event type the catalogue names must render a document"
        );
        for registry_name in AWS_MANAGED_REGISTRIES {
            let registry = state
                .registries
                .get(*registry_name)
                .unwrap_or_else(|| panic!("case {registry_name:?}: must be seeded"));
            assert_eq!(
                registry.scope,
                RegistryScope::Aws,
                "case {registry_name:?}: a seeded registry is AWS-scoped, got {:?}",
                registry.scope
            );
            assert!(
                !registry.schemas.is_empty(),
                "case {registry_name:?}: a seeded registry must hold schemas"
            );
        }
        let registry = &state.registries[AWS_MANAGED_REGISTRY];
        assert_eq!(
            registry.schemas.len(),
            catalogue.len(),
            "the registry holds exactly the catalogue, got {} of {}",
            registry.schemas.len(),
            catalogue.len()
        );
        for entry in catalogue {
            let schema_name = &entry.name;
            let schema = registry
                .schemas
                .get(schema_name)
                .unwrap_or_else(|| panic!("case {schema_name:?}: must be seeded"));
            assert_eq!(
                schema.r#type, AWS_MANAGED_SCHEMA_TYPE,
                "case {schema_name:?}: expected {AWS_MANAGED_SCHEMA_TYPE}, got {:?}",
                schema.r#type
            );
            assert_eq!(
                schema.description, entry.description,
                "case {schema_name:?}: expected {:?}, got {:?}",
                entry.description, schema.description
            );
            let version = schema
                .version_for(None)
                .unwrap_or_else(|| panic!("case {schema_name:?}: has a latest version"));
            assert_eq!(
                version.content, entry.content,
                "case {schema_name:?}: the seeded content is stored verbatim"
            );
            // The catalogue's documents are what ExportSchema converts, so each
            // one has to BE an OpenAPI 3 document whose envelope's `detail`
            // references a component the document defines.
            let parsed: ::serde_json::Value = ::serde_json::from_str(&entry.content)
                .unwrap_or_else(|e| panic!("case {schema_name:?}: must be JSON: {e}"));
            let components = &parsed["components"]["schemas"];
            assert!(
                components.is_object(),
                "case {schema_name:?}: must carry components.schemas, got {parsed:?}"
            );
            let detail = components[EVENT_ENVELOPE_COMPONENT]["properties"]["detail"]["$ref"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let referenced = detail
                .strip_prefix(OPENAPI_COMPONENT_REF)
                .unwrap_or_default()
                .to_string();
            assert!(
                !referenced.is_empty() && components[&referenced].is_object(),
                "case {schema_name:?}: the envelope's detail must reference a defined \
                 component, got {detail:?}"
            );
        }
    }

    /// Every event type the catalogue names is published under `<source>@<EventName>`
    /// and described by the sentence that name yields, and no name is seeded twice.
    #[test]
    fn the_catalogue_names_and_describes_every_event_type() {
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for event in aws_managed_event_types() {
            let name = event.schema_name();
            assert_eq!(
                name,
                format!("{}@{}", event.source, event.event_name),
                "case {name:?}: a catalogue name is <source>@<EventName>"
            );
            assert_eq!(
                managed_schema_description(&name),
                Some(format!(
                    "Schema for event type {}, published by AWS service {}",
                    event.event_name, event.source
                )),
                "case {name:?}: wrong description"
            );
            assert!(
                event.source.starts_with("aws."),
                "case {name:?}: an AWS event source is `aws.<service>`"
            );
            assert!(
                seen.insert(name.clone()),
                "case {name:?}: the catalogue names it twice"
            );
        }
        assert_eq!(
            managed_schema_description("no-event-type-here"),
            None,
            "a name that is not <source>@<EventName> describes no event type"
        );
    }

    /// A service-owned registry reports the region of the request asking, because no
    /// request created it; a caller's reports the ARN its create minted.
    #[test]
    fn a_managed_registry_reports_the_asking_region() {
        let state = ComAmazonawsSchemasSchemasState::default();
        let managed_name = AWS_MANAGED_REGISTRY;
        let managed = &state.registries[managed_name];
        for region in ["us-east-1", "eu-west-2", "ap-southeast-1"] {
            assert_eq!(
                managed.arn_in(region),
                registry_arn(region, managed_name),
                "case {region:?}: a managed registry's ARN names the asking region"
            );
        }

        let local = Registry {
            name: "mine".to_string(),
            arn: registry_arn("us-west-2", "mine"),
            description: None,
            tags: BTreeMap::new(),
            scope: RegistryScope::Local,
            policy: None,
            schemas: BTreeMap::new(),
        };
        assert_eq!(
            local.arn_in("eu-west-2"),
            registry_arn("us-west-2", "mine"),
            "a caller's registry keeps the ARN its create minted"
        );
    }

    /// Minting versions hands out strictly increasing, never-repeated numbers, even
    /// across a delete of the newest one.
    #[test]
    fn minting_a_version_never_repeats_a_number() {
        let now = SystemTime::now();
        let mut schema = Schema {
            name: "s".to_string(),
            arn: schema_arn(DEFAULT_REGION, "r", "s"),
            description: None,
            r#type: "OpenApi3".to_string(),
            tags: BTreeMap::new(),
            latest_version: 1,
            last_modified: now,
            versions: std::iter::once((
                1,
                SchemaVersion {
                    version: 1,
                    content: "one".to_string(),
                    r#type: "OpenApi3".to_string(),
                    created_date: now,
                },
            ))
            .collect(),
            minted_by_token: BTreeMap::new(),
            code_bindings: BTreeMap::new(),
        };

        let mut minted = vec![1];
        for round in 0..4 {
            let version = schema
                .add_version(format!("content-{round}"), "OpenApi3".to_string(), now)
                .version;
            assert!(
                !minted.contains(&version),
                "case round={round}: version {version} was minted twice ({minted:?})"
            );
            minted.push(version);
            // Deleting the newest version must not let the next mint reuse it.
            if round == 1 {
                schema.versions.remove(&version);
            }
        }
        assert_eq!(
            minted,
            vec![1, 2, 3, 4, 5],
            "versions are minted in sequence, got {minted:?}"
        );
    }
}
