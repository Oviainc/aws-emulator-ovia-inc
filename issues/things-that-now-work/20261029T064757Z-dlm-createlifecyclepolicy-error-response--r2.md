Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: c17c6e54c038586d9d6328bb68b9d1b3d6bcf360

# A request that omits a required member skips the iam:PassRole authorization the crate says runs first, so a non-passable ExecutionRoleArn is answered 400 instead of 403

_Filed: 2026-10-29T06:47:57Z_
_Operation: CreateLifecyclePolicy | Class: error-response_

> _Note: this issue was identified against the code base at commit `c17c6e54c038586d9d6328bb68b9d1b3d6bcf360`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`service.rs` states a fixed order for the two refusals that precede every handler of this service: layer 1 is the `iam:PassRole` authorization of the `ExecutionRoleArn` the request names (403 `AccessDeniedException`), layer 2 is the modelled input constraints (400 `InvalidRequestException`), and layer 2 is explicitly defined to include "a `@required` member absent".

`refused_before_handling` breaks that order on exactly the absent-member path. The `ExecutionRoleArn` is read, and `pass_role_denied` called, only in the `Ok(input)` arm of the deserialization match. When the body omits a member the generated input holds non-optionally (`CreateLifecyclePolicyInput::state`, `::description`, `::execution_role_arn` are `String`, not `Option<String>`, and nested shapes such as `CrossRegionCopyRule::encrypted` and `Script::execution_handler` are likewise non-optional), the deserializer returns `Err(SerdeError::MissingMember { .. })`, the helper synthesises a `MissingRequiredMember` violation and answers 400 — `pass_role_denied` is never called, even though the body carried an `ExecutionRoleArn` the caller may not pass.

So one and the same non-passable role gets two different error classes depending on an unrelated member: `{"ExecutionRoleArn": "arn:aws:iam::111122223333:role/Foo", "Description": "x", "State": "ENABLED"}` is 403 `AccessDeniedException` ("Cross-account pass role is not allowed."), while the same body without `State` is 400 `InvalidRequestException` naming `State`. The crate's own comment on that arm says absence and a bad value "to a caller they are one refusal, so they are one here" — but the one refusal they are folded into is layer 2, which the module doc says runs only after layer 1. `UpdateLifecyclePolicy` reaches the same hole through a nested required member (a `PolicyDetails.Schedules[n].CrossRegionCopyRules[m]` that omits `Encrypted`) while naming a cross-account `ExecutionRoleArn`, because both operations are wrapped in the same `InModelledEnvelope`. This is distinct from the already-fixed "ValidationException instead of AccessDeniedException" gap, which was about a fully-formed request: that fix introduced `pass_role_denied` and this is the one path that still bypasses it.

## Evidence

The order the crate declares — `services/service-com-amazonaws-dlm-dlm-20180112/src/service.rs:54-62`:
```
//! A request meets three layers before a handler runs, in this order:
//!
//! 1. **`iam:PassRole` for the `ExecutionRoleArn` it names.** Amazon Data
//!    Lifecycle Manager assumes that role, so the authorization precedes even the
//!    member-constraint walk: an ARN that denotes a principal the caller may not
//!    pass is `AccessDeniedException` (403), not a shape refusal.
//! 2. **The modelled input constraints.** A value outside a declared `@pattern`,
//!    `@length` or `@range`, or a `@required` member absent, is this service's own
//!    `InvalidRequestException` (400) ...
```
and again on the envelope that runs them — `src/service.rs:288-294`:
```
/// Two layers, in the order the real front door runs them:
///
/// 1. `iam:PassRole` for the `ExecutionRoleArn` the request names. That is ahead
///    of the member-constraint walk, so the denial reaches the caller even for an
///    ARN the `ExecutionRoleArn` `@pattern` would otherwise refuse as malformed —
///    and, because nothing has run yet, a denied create leaves no policy behind
///    and a denied update leaves the stored policy as it was.
```

The code that breaks it — `src/service.rs:476-494` (the helper's own comment calls absence part of the same refusal as the walk, yet only the `Ok` arm authorizes):
```
    let mut deserializer = protocol.deserialize_input(req, O::INPUT_SCHEMA).ok()?;
    let violations = match <O::Input as DeserializableStruct>::deserialize(&mut *deserializer) {
        Ok(input) => {
            if let Some(denied) = pass_role_denied(protocol, input.execution_role_arn()) {
                return Some(denied);
            }
            constraints::validate(&input, O::INPUT_SCHEMA)
        }
        // Absence is found while READING the request and a bad value while
        // walking what was read; to a caller they are one refusal, so they are
        // one here.
        Err(SerdeError::MissingMember { member_name }) => vec![Violation {
            path: member_name,
            kind: ViolationKind::MissingRequiredMember,
            value: None,
        }],
        Err(_) => Vec::new(),
    };
    invalid_request(protocol, &violations)
```

The helper that path skips — `src/service.rs:604-608`:
```
fn pass_role_denied(
    protocol: &dyn ServerHttpProtocol,
    execution_role_arn: Option<&str>,
) -> Option<RtResponse> {
    let message = crate::state::pass_role_denial(execution_role_arn?)?;
```

That the absent-member path is reachable with a body-level `ExecutionRoleArn` present — `src/ops/types/create_lifecycle_policy_types.rs:16-19`:
```
pub struct CreateLifecyclePolicyInput {
    pub execution_role_arn: String,
    pub description: String,
    pub state: String,
```
(`state` is not `Option`, so a body omitting `State` fails in the deserializer rather than in the constraint walk), and for the nested case `src/ops/types/shared_types.rs:112-117`:
```
#[smithy_macros::smithy(shape = "com.amazonaws.dlm#CrossRegionCopyRule")]
#[derive(Debug, Clone, Default)]
pub struct CrossRegionCopyRule {
    pub target_region: Option<String>,
    pub target: Option<String>,
    pub encrypted: bool,
```

Test coverage: `tests/all/execution_role_pass_role.rs:9-14` pins the rule for well-formed requests — "an `ExecutionRoleArn` that denotes an IAM principal the caller may not pass — one in another account, or one that is not a role — is refused with the error code `AccessDeniedException` (HTTP 403) ... while a value that is not a well-formed IAM ARN at all names no principal to authorize and stays the member's own shape refusal" — and every row in `cases()` (`tests/all/execution_role_pass_role.rs:61-80`) sends a complete body, so the missing-member combination is unpinned.

## Steps to Reproduce

1. POST /policies (SigV4-signed for dlm) with body {"ExecutionRoleArn": "arn:aws:iam::111122223333:role/CrossAccountDlmRole", "Description": "cross account role", "State": "ENABLED", "PolicyDetails": {"ResourceTypes": ["VOLUME"], "TargetTags": [{"Key": "k", "Value": "v"}], "Schedules": [{"CreateRule": {"Interval": 24, "IntervalUnit": "HOURS"}, "RetainRule": {"Count": 1}}]}} — observe HTTP 403 with error code AccessDeniedException and message "Cross-account pass role is not allowed." (layer 1 ran)
2. Repeat the same request with the "State" member removed from the body — observe HTTP 400 InvalidRequestException, Code InvalidParameter, RequiredParameters ["State"]: the identical non-passable ExecutionRoleArn is now answered with a shape refusal because the deserializer failed with SerdeError::MissingMember and src/service.rs:487-491 returns before pass_role_denied is ever called
3. Read src/service.rs:54-62 and src/service.rs:288-294 — the crate states the pass-role authorization "precedes even the member-constraint walk" and that layer 2 is where "a `@required` member absent" is refused, so step 2 answers the layer-2 error for a request layer 1 was supposed to deny first
4. PATCH /policies/{PolicyId} with {"ExecutionRoleArn": "arn:aws:iam::111122223333:role/CrossAccountDlmRole", "PolicyDetails": {... "Schedules": [{"CreateRule": {...}, "RetainRule": {"Count": 1}, "CrossRegionCopyRules": [{"TargetRegion": "us-west-2"}]}]}} — the copy rule omits the required Encrypted, so the same bypass answers 400 instead of the 403 the same ARN gets on a complete body
