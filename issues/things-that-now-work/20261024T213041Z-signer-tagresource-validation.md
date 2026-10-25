Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: aa34a7732176a121f53f6fa0a60cefb8de4c89bc

# TagResource accepts tag keys outside the modelled TagKey character set

_Filed: 2026-10-24T21:30:41Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `aa34a7732176a121f53f6fa0a60cefb8de4c89bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResource` writes any tag key the request carries as long as it does not start with `aws:`. The model constrains a tag key far more tightly than that: `TagKey` (the key shape of `TagMap`, the map `TagResourceRequest$tags` targets) is `@length(min: 1, max: 128)` with `@pattern("^(?!aws:)[a-zA-Z+-=._:/]+$")`. The character class that pattern admits is letters, digits and the punctuation in `+`..`=` plus `.`, `_`, `:`, `/` — it does NOT admit a space, `!`, `#`, `$`, `%`, `&`, `'`, `(`, `)`, `?`, `@`, `[`, `]`, `^`, backtick, `{`, `|`, `}` or `~`.

Expected: a `TagResource` whose `tags` carries a key such as `"my key"`, `"owner@team"` or `"cost#center"` is refused — the tagging operations' modelled bad request (`BadRequestException`, HTTP 400, "The request contains invalid parameters for the ARN or tags") — and the profile's tag map is left untouched.
Actual: the emulator answers HTTP 200 and stores the key; `ListTagsForResource` and `GetSigningProfile` then report a tag map AWS Signer could never hold, and `UntagResource` is the only way to get rid of it.

The constraint is unenforced on BOTH sides of the boundary, which is why it is a real hole rather than a redundant check:
* the framework's boundary validator treats a `@pattern` whose regex does not compile as MATCHING — `vendor/smithy-schema-server/src/constraints.rs` `pattern_matches()`: `let Ok(re) = regex::Regex::new(regex) else { return true; }` (the comment above it says so outright). `TagKey`'s pattern uses a negative lookahead `(?!aws:)`, which the `regex` crate cannot compile, so NO part of the pattern — including the character class — is ever checked;
* the handler only re-implements the `aws:` half of the pattern (`crate::state::reserved_tag_key`), as its own comment at `src/ops/tag_resource.rs:197-200` states, and nothing checks the admitted alphabet.

Related entry point for whoever fixes this (filed separately / already covered for that operation): `PutSigningProfileRequest$tags` targets the SAME `TagMap`/`TagKey` shapes and `src/ops/put_signing_profile.rs:258` stores `input.tags` verbatim with no key judgement at all, so a create can seed a profile with both an out-of-alphabet key and a reserved `aws:`-prefixed key that `TagResource` would refuse. The shared key-judgement helper in `crate::state` is the natural place for the rule so both write paths agree.

## Evidence

Smithy model: `smithy_query(query="shape", shape="TagKey")` -> `{"type": "string", "length": {"min": 1, "max": 128}, "pattern": "^(?!aws:)[a-zA-Z+-=._:/]+$"}`; `TagResourceRequest$tags` is `TagMap` (`@length(min:1,max:200)`, key `TagKey`, value `TagValue`) and `@required`. `TagResource` declares `BadRequestException` (HTTP 400) for "The request contains invalid parameters for the ARN or tags".

Handler: `services/service-com-amazonaws-signer-wallabyservice/src/ops/tag_resource.rs` lines 196-247 — the only key judgement is `crate::state::reserved_tag_key(input.tags.keys()...)` (the `aws:` prefix); after the ARN resolution, the status gate and the 200-tag count check, the write is an unconditional `profile.tags.insert(key.clone(), value.clone())`.

Framework: `vendor/smithy-schema-server/src/constraints.rs:454-462` — a `@pattern` that fails to compile is treated as matching, so `TagKey`'s lookahead pattern is skipped in full (not merely its lookahead), leaving the character class unenforced at the boundary too.

Test coverage: `services/service-com-amazonaws-signer-wallabyservice/tests/all/resource_tags.rs::a_write_outside_the_modelled_tag_bounds_is_refused_and_stores_nothing` (lines 781-879) asserts only the reserved-`aws:`-prefix cases and the 200-tag count bound; every key it uses (`env`, `key0`, `one-too-many`, `owner`) is inside the modelled alphabet, so no test asserts the character-set rule in either direction.

## Steps to Reproduce

1. Boot the emulator and create a signing profile: `PUT /signing-profiles/Tagged_Profile` with `{"platformId": "AWSLambda-SHA384-ECDSA"}`; note the returned `arn`.
2. Call `POST /tags/<url-encoded arn>` with body `{"tags": {"my key": "v"}}` (a key containing a space, which `TagKey`'s pattern does not admit).
3. Observe HTTP 200 and no error body, where AWS Signer refuses the write with HTTP 400 / `BadRequestException`.
4. Call `GET /tags/<url-encoded arn>` and observe the stored map contains `"my key"` — a key outside the modelled `TagKey` pattern is now readable through `ListTagsForResource` and `GetSigningProfile`.
5. Repeat with keys `owner@team`, `cost#center` and `a{b}` — all are accepted.
6. For contrast, `{"tags": {"aws:createdBy": "x"}}` IS refused 400, showing only the `aws:` half of the pattern is implemented.
