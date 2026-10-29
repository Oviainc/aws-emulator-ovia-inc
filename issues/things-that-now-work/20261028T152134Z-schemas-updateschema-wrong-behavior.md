Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7e4efc65cc687607a3c1322cd06c2fb91d881b32

# UpdateSchema's retype guard compares Type against a stale Schema::r#type that names a deleted version

_Filed: 2026-10-28T15:21:34Z_
_Operation: UpdateSchema | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7e4efc65cc687607a3c1322cd06c2fb91d881b32`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Schema::r#type` is written only by `Schema::add_version`, so it holds the type of the definition LAST WRITTEN. `Schema::remove_version` never revisits it, so after a `DeleteSchemaVersion` of the newest version the member names the type of a definition the store no longer holds, while every READ reports the type of the version it resolved (`describe_schema.rs:209`, and `update_schema.rs`'s own response member).

`UpdateSchema` then decides whether a request asks to relabel a stored document by comparing the request's `Type` against that stale member (`update_schema.rs:216-219`). Two wrong outcomes follow, both reachable with nothing but modelled input:

1. A request that asks for NO change is refused 400. With v1 stored as `OpenApi3` and v2 (since deleted) stored as `JSONSchemaDraft4`, `schema.r#type` is `JSONSchemaDraft4` while every read of the schema reports `OpenApi3`. `PUT {"Type":"OpenApi3","Description":"x"}` is then refused with "Type names the type of the schema content, so it can only be changed by a request that also sends Content" — although `OpenApi3` is exactly the type the schema's only remaining definition carries and the type the same handler would report back.
2. The inverse request is accepted and answered self-contradictorily. `PUT {"Type":"JSONSchemaDraft4","Description":"x"}` passes the guard (`retyped == false`) and answers 200 with `Type: "OpenApi3"` in the same response body — the service accepts a request declaring one type and reports another.

This is the exact failure mode the crate already fixed for version NUMBERS: `latest_stored_version` exists so that "a version a delete took out is reported by nothing afterwards" (`update_schema.rs:310-313`), yet the type of that same deleted version keeps deciding this guard. The fix is to compare against the type of the definition the schema still holds (the resolved/latest stored version's `r#type`, falling back to the stored member only when no version remains, which is what the existing versionless test pins).

## Evidence

The guard, comparing against the stored member — services/service-com-amazonaws-schemas-schemas/src/ops/update_schema.rs:213-228:
```
        // `Type` is the type OF the content: a stored document cannot be relabelled
        // without being rewritten, or every reader of the old version would be told
        // the wrong type for the bytes it gets.
        let retyped = input
            .r#type
            .as_deref()
            .is_some_and(|requested| requested != schema.r#type);
        if retyped && input.content.is_none() {
            return Err(UpdateSchemaErrorModeled::BadRequestException(
                crate::state::bad_request(
                    "Type names the type of the schema content, so it can only be changed \
                     by a request that also sends Content.",
                ),
            )
            .into());
        }
```

What that member means — the only writer of it, services/service-com-amazonaws-schemas-schemas/src/state.rs:1471-1480 (`add_version`):
```
    pub fn add_version(
        &mut self,
        content: String,
        r#type: String,
        now: SystemTime,
    ) -> &SchemaVersion {
        let version = self.latest_version.saturating_add(1);
        self.latest_version = version;
        self.modified_at(now);
        self.r#type = r#type.clone();
```
and the handler's own characterisation of it — update_schema.rs:347-352: "the type is also the SCHEMA's own stored member (`Schema::r#type`, which `add_version` keeps at the type of the definition last written)".

The delete that leaves it stale — state.rs:1422-1431:
```
    pub fn remove_version(&mut self, version: u64, now: SystemTime) -> bool {
        if self.versions.remove(&version).is_none() {
            return false;
        }
        self.code_bindings
            .retain(|(bound_version, _), _| *bound_version != version);
        self.minted_by_token.retain(|_, minted| *minted != version);
        self.modified_at(now);
        true
    }
```
(nothing touches `self.r#type`).

The rule the crate states for exactly this situation — update_schema.rs:310-313:
```
                // The newest version the schema still HOLDS: a version a delete took
                // out is reported by nothing afterwards, and a schema every version of
                // which has been deleted has no definition to report at all.
                schema.latest_stored_version()
```
and state.rs:1398-1405: "once a version is deleted the store no longer holds it, so what a read of 'the latest version' answers is the newest one that survives."

What reads report instead of the stored member — services/service-com-amazonaws-schemas-schemas/src/ops/describe_schema.rs:209: `r#type: Some(version.r#type.clone()),`.

Test coverage: nothing pins this case. tests/all/schema_last_modified.rs:239-246 pins a genuine retype ("a refused UpdateSchema (Type without Content)" -> 400) where the schema's remaining definitions are all `OpenApi3` and the request names `JSONSchemaDraft4`, which stays 400 under the fix; tests/all/schema_version_deletion.rs:630-635 pins that `Type` survives when the schema holds NO version ("the schema keeps the type its definitions were written with"), which also stays unchanged. No test mixes types across versions and then deletes the newest.

## Steps to Reproduce

1. POST /v1/registries/name/mine -> 201 (CreateRegistry)
2. POST /v1/registries/name/mine/schemas/name/s with {"Type":"OpenApi3","Content":"{\"openapi\":\"3.0.0\"}"} -> 201, minting version 1 (type OpenApi3)
3. PUT /v1/registries/name/mine/schemas/name/s with {"Type":"JSONSchemaDraft4","Content":"{\"$schema\":\"http://json-schema.org/draft-04/schema#\"}"} -> 200, minting version 2; Schema::r#type is now JSONSchemaDraft4 (state.rs:1480)
4. DELETE /v1/registries/name/mine/schemas/name/s/version/2 -> 204 (DeleteSchemaVersion); versions = {1: OpenApi3}, Schema::r#type still JSONSchemaDraft4
5. GET /v1/registries/name/mine/schemas/name/s -> 200 with Type: "OpenApi3" (describe_schema.rs:209)
6. PUT /v1/registries/name/mine/schemas/name/s with {"Type":"OpenApi3","Description":"x"} -> 400 BadRequestException "Type names the type of the schema content...", although OpenApi3 is the type of the only definition the schema holds and the type every read reports
7. PUT /v1/registries/name/mine/schemas/name/s with {"Type":"JSONSchemaDraft4","Description":"x"} -> 200, and the response body reports Type: "OpenApi3" — the accepted request declared the other type
