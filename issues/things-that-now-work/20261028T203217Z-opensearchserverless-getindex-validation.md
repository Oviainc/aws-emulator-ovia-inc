Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 0f071a90384ece90fe46e5865745f415e3851c94

# GetIndex skips the index-name rule CreateIndex enforces, so the same name is 400 on the write and 404 on the read

_Filed: 2026-10-28T20:32:17Z_
_Operation: GetIndex | Class: validation_

> _Note: this issue was identified against the code base at commit `0f071a90384ece90fe46e5865745f415e3851c94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate has ONE helper for the index-name rule the model documents — `crate::state::validate_index_name` — and only `CreateIndex` calls it. `GetIndex`, the sibling that addresses the same resource by the same `(id, indexName)` pair, never calls it: it goes straight to the collection lookup and then to `collection.indexes.get(&input.index_name)`, so a name the service's own validator rejects (uppercase, leading `_`, leading `-`, empty) is reported as `ResourceNotFoundException` ("The index '…' does not exist in the collection with Id '…'.") instead of the `ValidationException` the operation also models.

The result is that the two operations disagree about one and the same string: `CreateIndex(indexName="MixedCase")` answers 400 ValidationException ("Member must be lowercase"), while `GetIndex(indexName="MixedCase")` answers 404 and tells the caller the index is merely absent — for a name no index in this emulator can ever carry, because the only path that creates one refuses it. A client cannot distinguish "this name is not a legal index name" from "nothing is stored under it".

This is the read path skipping a helper the crate already has, not a feature the crate never attempts.

## Evidence

The helper and the rule it holds a name to — src/state.rs:1963-1969: "/// Hold an `IndexName` to the rule `CreateIndexRequest$indexName` documents: \"Index names\n/// must be lowercase and can't begin with underscores (_) or hyphens (-)\".\n///\n/// `IndexName` carries no `@length` and no `@pattern`, so the boundary validator narrows\n/// nothing here; an empty name is refused too, since an index has to be named to be\n/// addressable at all." followed by "pub fn validate_index_name(name: &str) -> Result<(), String>" (src/state.rs:1970).

The ONLY caller — src/ops/create_index.rs:160-170: "// `IndexName` carries no `@pattern`, so the boundary validator does not narrow it;\n        // the rule is in the member's own documentation: \"Index names must be lowercase\n        // and can't begin with underscores (_) or hyphens (-)\".\n        if let Err(reason) = crate::state::validate_index_name(&input.index_name) {\n            return Err(\n                CreateIndexErrorModeled::ValidationException(ValidationException {\n                    message: Some(reason),\n                })". A repo-wide grep for `validate_index_name` under src/ops/ matches create_index.rs:163 and nothing else.

The read path that skips it — src/ops/get_index.rs:166-176: "let Some(index) = collection.indexes.get(&input.index_name) else {\n            return Err(GetIndexErrorModeled::ResourceNotFoundException(\n                ResourceNotFoundException {\n                    message: Some(format!(\n                        \"The index '{}' does not exist in the collection with Id '{}'.\",\n                        input.index_name, input.id\n                    ))," — no name check anywhere in the handler body (src/ops/get_index.rs:145-183).

GetIndex declares the error it does not use — OPERATIONS.md:223: "- **Errors:** `InternalServerException (HTTP 500)`, `ResourceNotFoundException (HTTP 404)`, `ValidationException (HTTP 400)`".

Test coverage: the four invalid names are pinned as ValidationException for the WRITE only — tests/all/encryption_indexes_and_tagging.rs:648-671: "(\n            \"an index name beginning with an underscore\",\n            json!({ \"id\": id.clone(), \"indexName\": \"_system-index\" }),\n            400,\n            \"ValidationException\",\n        )," … "(\n            \"an index name with an uppercase letter\",\n            json!({ \"id\": id.clone(), \"indexName\": \"MixedCase\" }),\n            400,\n            \"ValidationException\",\n        )," — driven through `call(&router, "CreateIndex", request)` (:674). The GetIndex refusal table next to it uses only VALID-but-absent names — tests/all/encryption_indexes_and_tagging.rs:684-692: "(\n            \"an index name that names no index\",\n            json!({ \"id\": id.clone(), \"indexName\": \"never-created\" }),\n        )," — so no test pins, and no comment justifies, what GetIndex answers for a name its sibling refuses.

## Steps to Reproduce

1. CreateSecurityPolicy(type="encryption", name="enc", policy='{"Rules":[{"ResourceType":"collection","Resource":["collection/*"]}],"AWSOwnedKey":true}') -> 200.
2. CreateCollection(name="indexed") -> 200; wait for ACTIVE.
3. CreateIndex(id=<collection id>, indexName="MixedCase") -> 400 ValidationException "Value 'MixedCase' at 'indexName' failed to satisfy constraint: Member must be lowercase" (src/ops/create_index.rs:163).
4. GetIndex(id=<collection id>, indexName="MixedCase") -> 404 ResourceNotFoundException "The index 'MixedCase' does not exist in the collection with Id '<id>'." (src/ops/get_index.rs:166-176), although the same string was just refused as invalid input by the only operation that could create it.
5. Repeat step 4 with indexName="_system-index", "-leading-hyphen" and "" — all answer the not-found refusal instead of the validation refusal the operation models.
