Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 8323f4d349418e3a01a6cea3a48f7209706d6844

# Front door's `@sensitive` predicate matches any violation path ending in `secretString`, turning a modelled 400 on `tags.<key>` into a 403

_Filed: 2026-10-27T11:41:44Z_
_Operation: PutDeploymentParameter | Class: error-response_

> _Note: this issue was identified against the code base at commit `8323f4d349418e3a01a6cea3a48f7209706d6844`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The front-door refusal is keyed on a violation whose path's LAST dotted segment is the literal member name `secretString`. The crate's own comment narrows that intent to the one modelled sensitive member (`DeploymentParameterInput$secretString`, reported at `deploymentParameter.secretString`), but the implemented test is a bare last-segment comparison, and this service's constraint walk reports a tag-map VALUE violation at `tags.<tag key>` — the crate's own test pins `fieldName == "tags.owner"` for exactly such a violation.

Consequence: `PutDeploymentParameter` carrying a perfectly legal tag key named `secretString` whose VALUE is past `TagValue`'s modelled maximum produces a `MaxLength` violation at path `tags.secretString`, so `names_a_sensitive_member` returns true, `is_oversized_sensitive_payload` returns true, `front_door_refuses` returns true, and the request is answered with HTTP 403 `ForbiddenException` (a shape this model does not declare, body `{"message":"Forbidden"}`) instead of the modelled HTTP 400 `ValidationException` the operation declares for an input-constraint violation. The identical request with any other tag key (e.g. `owner`) is correctly answered 400. A client branching on the error code therefore sees two different codes for one logical failure, decided by the caller's choice of tag key.

Same root cause, second symptom: `constraint_clause` suppresses the echoed value for any violation whose path ends in `secretString`, so a too-long/ill-patterned tag value stored under that key is reported without its value while every other tag value is echoed.

## Evidence

The predicate, matching on the last path segment only — `services/.../src/service.rs:146-174`:
```
/// The members this service marks `@sensitive`, as a validation path spells them.
///
/// `smithy_query(query="shape",shape="SecretString")`: `SecretString` is the only
/// shape in this model carrying `sensitive: true`, and it is reached as
/// `DeploymentParameterInput$secretString`, so a violation on it is reported at
/// `deploymentParameter.secretString` — the LAST segment is the member whose value
/// would be echoed.
const SENSITIVE_MEMBERS: &[&str] = &["secretString"];

fn names_a_sensitive_member(path: &str) -> bool {
    let member = path.split('.').next_back().unwrap_or(path);
    SENSITIVE_MEMBERS.contains(&member)
}
...
fn is_oversized_sensitive_payload(violation: &Violation) -> bool {
    matches!(violation.kind, ViolationKind::MaxLength(_))
        && names_a_sensitive_member(&violation.path)
}
```
The refusal it produces — `services/.../src/service.rs:504-510`: `if self.front_door_refuses(req) { return Some(&self.forbidden as &dyn ErasedOperation); }`, which answers `FORBIDDEN_STATUS = 403` / `FORBIDDEN_CODE = "ForbiddenException"` (`src/scaffolded.rs:380-391`), a code `src/scaffolded.rs:364-366` itself calls out as "NOT a shape this service's model declares". `front_door_refuses` runs for exactly this operation (`src/service.rs:414-423`), whose input carries `tags`.

That a tag-map value violation's path carries the TAG KEY as its last segment is pinned by this crate's own test — `tests/all/boundary_validation_field_name.rs:204-214`:
```
        // A value inside a COLLECTION, named at the position the report gives it.
        Row::new(
            "a tag value outside the character class the constrained map states",
            ...
                "tags": { "owner": "bad value" },
            }),
            "tags.owner",
        ),
```
and `fieldName` is literally `violation.path` (`src/service/modelled_refusal.rs:138-143`: `violations.iter().map(|violation| violation.path.as_str()).find(|path| !path.is_empty())`).

The model makes `secretString` a legal tag key and a 257-character value a 400: `smithy_query(query="shape",shape="TagsMap")` → `key: TagKey {length 1..128, pattern ^[a-zA-Z0-9/_+=.:@-]+$}`, `value: TagValue {length 1..256, ...}`, and `PutDeploymentParameter` declares `ValidationException (HTTP 400)` (`OPERATIONS.md:56`).

Second symptom, `src/service.rs:313-319`:
```
        // A `@sensitive` member names itself and nothing of what it held.
        (_, Some(_)) if names_a_sensitive_member(&violation.path) => "Value".to_string(),
        (_, Some(shown)) => format!("Value '{shown}'"),
```
Test coverage today: `tests/all/boundary_validation_field_name.rs:443-461` only exercises the oversized real secret (`deploymentParameter.secretString`), and `tests/all/boundary_validation_field_name.rs:204-214` only a tag key named `owner`; no test uses a tag key named `secretString`, so nothing pins the current answer.

## Steps to Reproduce

1. Mount the emulator as `tests/all/deployment_parameters.rs::emulator()` does and provision a target (`provision(&engine, "AWSMarketplace", "agmt-x", &["prod-x"])`).
2. POST /catalogs/AWSMarketplace/products/prod-x/deployment-parameters with body `{"agreementId":"agmt-x","deploymentParameter":{"name":"N","secretString":"s"},"tags":{"secretString":"<257 'v' characters>"}}`.
3. Observe HTTP 403 with `__type`/`x-amzn-errortype` `ForbiddenException` and body `{"message":"Forbidden"}`.
4. Expected: HTTP 400 `ValidationException` with `fieldName` = `tags.secretString`, the answer the model declares for a `TagValue` past its `@length` maximum.
5. Re-send the identical request with the tag key `owner` instead of `secretString` and observe the expected 400 `ValidationException` — the only difference is the tag key the caller chose.
