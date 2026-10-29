Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 6946203d9f2a769569cfa156ac7c42bcf6c300bc

# An empty ClientTokenId is treated as a real idempotency token, so a later UpdateSchema silently drops its Content and Description

_Filed: 2026-10-29T08:31:20Z_
_Operation: UpdateSchema | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `6946203d9f2a769569cfa156ac7c42bcf6c300bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateSchema`'s `ClientTokenId` is modelled `__stringMin0Max36` with `@length(min: 0, max: 36)`, so the boundary validator admits the EMPTY string: a request may legitimately carry `"ClientTokenId": ""`. The handler uses the raw member value as the key of the schema's `minted_by_token` map with no screen for a value that names nothing, so `""` becomes a token like any other.

Consequence: the FIRST `UpdateSchema` that carries `ClientTokenId: ""` and `Content` mints a version and records `"" -> N`. Every LATER `UpdateSchema` on that schema that also carries `ClientTokenId: ""` is then resolved as a RETRY of that first call, whatever `Content` it sends: the handler takes the `Some(version)` arm, which (by its own comment) "writes NOTHING at all - not even a `Description` it carries". The caller gets 200 with the earlier call's `SchemaVersion`, and the `Content` and `Description` it sent are silently dropped and never stored. Two independent, unrelated updates collapse into one, and the second write is lost with a success status.

This contradicts the rule this same crate states and implements for every other value that names or specifies nothing: an empty value names nothing rather than naming a particular thing. `Schema::version_number` screens `requested.is_empty()` explicitly before reading a version label; `keywords_of` drops empty pieces because "A value that carries no keyword at all (empty, or separators only) therefore limits nothing"; `tags_from_input` says "An absent map and an empty one are the same thing"; `NameConstraint::validate` refuses the empty name outright. Only `ClientTokenId` is read as if the empty string were a caller-chosen identity.

## Evidence

Model admits the empty value - smithy_query(query="operation", operation="UpdateSchema"):
    "ClientTokenId": {"target": "__stringMin0Max36", "idempotencyToken": true, "length": {"min": 0, "max": 36}}

Handler keys the map on the raw value, with no empty screen - services/service-com-amazonaws-schemas-schemas/src/ops/update_schema.rs:248-262:
                // Unless this is a RETRY of a call that already wrote one: the token
                // the first attempt carried names the version it minted, and a retry
                // must answer that rather than write the same definition twice. A
                // token whose version has since been deleted names nothing. A retry
                // that resolves to an already-minted version writes NOTHING at all —
                // not even a `Description` it carries: the first attempt's call is
                // the one that happened.
                let already_minted = input
                    .client_token_id
                    .as_deref()
                    .and_then(|token| schema.minted_by_token.get(token))
                    .copied()
                    .filter(|version| schema.versions.contains_key(version));
                match already_minted {
                    Some(version) => Some(version),
and the recording side, update_schema.rs:292-294:
                        if let Some(token) = input.client_token_id.clone() {
                            schema.minted_by_token.insert(token, minted);
                        }

Same crate, opposite rule for a value that names nothing - services/service-com-amazonaws-schemas-schemas/src/state.rs:1415-1420:
    fn version_number(requested: &str) -> Option<u64> {
        // Screen the spelling before handing the digits to `u64: FromStr`, which on its
        // own accepts a leading `+` and so would let a signed label name a version.
        if requested.is_empty() || !requested.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
services/service-com-amazonaws-schemas-schemas/src/ops/search_schemas.rs:305-310:
/// `Keywords` is a single `__string`, but the member is plural ... empty pieces are dropped. A value
/// that carries no keyword at all (empty, or separators only) therefore limits
/// nothing, which is all "specifying this limits the results" can mean for a value
/// that specifies none.
services/service-com-amazonaws-schemas-schemas/src/state.rs:3181-3182:
/// Store the `Tags` member of a request. An absent map and an empty one are the same
/// thing: a resource with no tags.
services/service-com-amazonaws-schemas-schemas/src/state.rs:2886-2887:
        // The class is `+`-quantified, so the empty name satisfies no pattern either.
        if name.is_empty() || !name.chars().all(|character| self.permits(character)) {

Test coverage: the idempotency-token behaviour is pinned only for NON-empty tokens - tests/all/aws_managed_and_versions.rs:928-931 ("Rows: token shapes a client sends — a plain word and the UUID an SDK generates", rows `"retry-token"` and `"11111111-2222-3333-4444-555555555555"`), tests/all/update_schema_versions.rs:209 (`"ClientTokenId": "tok"`) and tests/all/schema_version_deletion.rs:525-568. No test sends an empty `ClientTokenId`, so nothing pins the current collapse.

## Steps to Reproduce

1. Start the emulator. POST /v1/registries/name/mine -> 201 (CreateRegistry).
2. POST /v1/registries/name/mine/schemas/name/order.placed with {"Type":"OpenApi3","Content":"{\"openapi\":\"3.0.0\",\"x-v\":1}"} -> 201, minting version 1.
3. PUT /v1/registries/name/mine/schemas/name/order.placed with {"Content":"{\"openapi\":\"3.0.0\",\"x-v\":2}","ClientTokenId":""} -> 200 with SchemaVersion "2". The store now records the token "" -> version 2.
4. PUT /v1/registries/name/mine/schemas/name/order.placed with a DIFFERENT definition and the same empty token: {"Content":"{\"openapi\":\"3.0.0\",\"x-v\":3}","Description":"third write","ClientTokenId":""}.
5. Observed: 200 with SchemaVersion "2" and no Description change. GET /v1/registries/name/mine/schemas/name/order.placed/versions still reports two versions, and GET /v1/registries/name/mine/schemas/name/order.placed returns the x-v:2 document - the third call's Content and Description were dropped and never stored.
6. Expected: an empty ClientTokenId names no prior call (the crate's own rule for a value that specifies nothing - state.rs:1418, search_schemas.rs:305-310), so the second update mints version 3 and stores its Description, exactly as the same request with the member omitted does.
