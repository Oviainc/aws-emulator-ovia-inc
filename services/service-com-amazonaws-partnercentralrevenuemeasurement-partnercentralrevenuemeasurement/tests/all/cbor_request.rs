// SPDX-License-Identifier: CC0-1.0

//! Building an `rpcv2Cbor` request body for a member the schema serializer
//! cannot carry.
//!
//! An `rpcv2Cbor` request body is a CBOR map of member name to value, and every
//! suite in this crate normally builds one through the generated schema
//! serializer (`CborCodec` + the operation's `*_INPUT_SCHEMA`) — including every
//! probe in `tests/all/probe.rs`. Exactly one case cannot go that way:
//!
//! * a list of SCALARS. The vendored codegen hands the COLLECTION member's
//!   schema to each element of a list of scalars (`emit_write` in
//!   `vendor/smithy-macros/src/codegen.rs`), and the CBOR codec emits that
//!   member's key again inside the array — so `ProductIds: ["prod-…"]` goes out
//!   as `ProductIds: ["ProductIds", "prod-…"]` and the service refuses its own
//!   client's request. `vendor/` is read-only here and the defect is on the
//!   SERIALIZE side only, so the fix belongs upstream: it is filed as
//!   `issues/things-that-do-not-work/20261022T141000Z-vendor-cbor-serializer-reemits-member-key-in-a-list-of-scalars.md`,
//!   and this module exists to be deleted once that lands.
//!
//! So that `ListMarketplaceRevenueShares`'s `ProductIds` / `ProductCodes`
//! filters stay covered until then, this arranges the bytes of a request body
//! itself: the three definite-length headers RFC 8949 section 3 gives the forms
//! used here (map = major type 5, array = 4, text = 3) and the UTF-8 of each
//! string. It is TEST DATA only — the emulator's own CBOR, request and response,
//! is the vendored codec's and is untouched — and what it encodes is three item
//! headers for a two-level body, bounded to the lengths a test body needs and
//! panicking rather than mis-encoding anything longer. Delete this module, and
//! route these two filter suites back through `CborCodec` like every other suite
//! here, as soon as the vendored serializer stops re-emitting the member key.

/// A CBOR value a request body carries.
pub(crate) enum Cbor {
    /// A text string.
    Text(String),
    /// An array of values.
    List(Vec<Cbor>),
}

impl Cbor {
    /// A text value from anything string-like.
    pub(crate) fn text(value: impl Into<String>) -> Self {
        Self::Text(value.into())
    }
}

/// A CBOR array of text strings.
pub(crate) fn texts(values: &[&str]) -> Cbor {
    Cbor::List(values.iter().map(|value| Cbor::text(*value)).collect())
}

/// The initial byte(s) of a definite-length item: its major type and its
/// argument (RFC 8949 section 3). Only the lengths a test body needs are
/// encodable, so a longer one panics rather than being mis-encoded silently.
fn head(major: u8, argument: usize, out: &mut Vec<u8>) {
    let major = major << 5;
    match u8::try_from(argument) {
        // An argument below 24 travels in the initial byte itself.
        Ok(small) if small < 24 => out.push(major | small),
        // 24 says "one more byte carries the argument".
        Ok(byte) => out.extend_from_slice(&[major | 24, byte]),
        // 25 says "two more bytes, big-endian".
        Err(_) => match u16::try_from(argument) {
            Ok(short) => {
                out.push(major | 25);
                out.extend_from_slice(&short.to_be_bytes());
            }
            Err(_) => panic!("a test body needs no CBOR item of length {argument}"),
        },
    }
}

fn encode(value: &Cbor, out: &mut Vec<u8>) {
    match value {
        Cbor::Text(text) => {
            head(3, text.len(), out);
            out.extend_from_slice(text.as_bytes());
        }
        Cbor::List(items) => {
            head(4, items.len(), out);
            for item in items {
                encode(item, out);
            }
        }
    }
}

/// `entries` as the definite-length CBOR map an `rpcv2Cbor` request body is.
pub(crate) fn request_body(entries: Vec<(&str, Cbor)>) -> Vec<u8> {
    let mut out = Vec::new();
    head(5, entries.len(), &mut out);
    for (key, value) in entries {
        encode(&Cbor::text(key), &mut out);
        encode(&value, &mut out);
    }
    out
}
