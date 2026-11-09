Issue-Severity: HIGH
Issue-Gap-Class: framework

# `vendor/smithy-schema-server` trims the trailing slashes off a request path before it binds a greedy `@httpLabel`, so a key that ends in `/` is bound as another key

_Filed: 2026-11-10T00:00:00Z_
_Operation: vendor/smithy-schema-server (HTTP binding) | Class: framework_

## Description

`vendor/smithy-schema-server/src/http_binding.rs::match_path_labels` reduces the request
path to its segments with `let pth = path.trim_matches('/')` and binds a greedy label to
`path_segs[si..].join("/")`. `trim_matches` strips the TRAILING separators as well as the
leading one, so every empty trailing segment is gone before the label is bound: a greedy
label can never carry a value that ends in `/`. Interior empty segments survive the
`split('/')`/`join("/")` round trip, so this is the only shape affected.

That is wrong wherever a greedy label's value is a name rather than a path. An S3 object key
is "a sequence of Unicode characters whose UTF-8 encoding is at most 1,024 bytes long"
(`smithy_query(query="doc",shape="ObjectKey")`) in which `/` carries no structure, and a key
ending in `/` is exactly how the zero-byte "folder" objects a console or a CLI creates are
named. `PUT /<bucket>/sourc/` therefore bound the key `sourc`, and the write landed on a key
the client never named.

## Observable effect

Against the S3 emulator before the workaround below:

- `PUT /<bucket>/sourc/` stored the object at `sourc`, so `GET /<bucket>/sourc` answered it
  and `GET /<bucket>/sourc/` answered 404 — the reverse of AWS.
- The listing reported `<Key>sourc</Key>`, and `DELETE /<bucket>/sourc/` minted its delete
  marker for `sourc`.
- A `CopyObject` whose `x-amz-copy-source` named `<bucket>/sourc` resolved to the object
  written at `sourc/` and answered 200, where AWS answers 404 `NoSuchKey`.

## Scope

Every operation of every REST service whose `@http` pattern ends in a greedy label whose
value may end in a separator. In S3 that is the whole object plane (`PutObject`, `GetObject`,
`HeadObject`, `DeleteObject`, `CopyObject`, the multipart operations, the tagging, ACL,
retention and legal-hold sub-resources — every route shaped `/{Bucket}/{Key+}`), and the same
holds for any `/{resourcePath+}` label another service binds a name from.

## Fix

In `match_path_labels`, trim only the LEADING separator of the path (`strip_prefix('/')`)
rather than both ends, so an empty trailing segment reaches the greedy branch and the
`join("/")` puts its separator back. The literal and non-greedy label arms are unaffected: a
pattern segment is never empty, so a trailing empty path segment cannot match one.

This cannot be done from inside the emulator repo: `vendor/` is read-only here and the merge
gate refuses a change to it, so it needs a change to the vendored core library.

## Current workaround, and why it should be removed

`services/service-com-amazonaws-s3-amazons3/src/service/wire.rs` works around it in the one
seam the service owns, the protocol's `deserialize_input`: `key_trailing_slashes_escaped`
hands the binding deserializer the same path with the key's trailing separators written as
`%2F` escapes, which the label's own percent-decode turns back into the separators the
request carried. It is a per-service re-spelling of a path the framework already has, and
every other affected service has to repeat it; it should be deleted once `match_path_labels`
keeps the trailing segment.

## Evidence

`vendor/smithy-schema-server/src/http_binding.rs` — `match_path_labels`' `let pth =
path.trim_matches('/')` and the greedy arm's `path_segs[si..].join("/")`.
`services/service-com-amazonaws-s3-amazons3/tests/all/object_key_label_verbatim.rs` —
`a_key_is_stored_and_reported_exactly_as_it_was_addressed` drives the write, read, listing,
delete and copy of four trailing-slash keys and fails without the workaround.
