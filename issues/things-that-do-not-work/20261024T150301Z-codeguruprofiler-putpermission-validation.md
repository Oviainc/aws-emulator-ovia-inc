Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f5120b87affb905e7d968cf57a1e02e568fab681

# PutPermission accepts principals that are not ARNs and writes them into the resource policy

_Filed: 2026-10-24T15:03:01Z_
_Operation: PutPermission | Class: validation_

> _Note: this issue was identified against the code base at commit `f5120b87affb905e7d968cf57a1e02e568fab681`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: `principals` must be "A list ARNs for the roles and users you want to grant access to the profiling group" (PutPermissionRequest$principals doc). A resource-based policy principal has to be a valid IAM principal ARN (account root `arn:aws:iam::<account>:root`, a user, a role, or an assumed-role session). A request carrying a principal that is not an ARN at all (e.g. `principals=["not-an-arn"]`, `principals=["123456789012"]`, `principals=["arn:aws:s3:::bucket"]`) is refused by the service with 400 ValidationException and the group's policy is left untouched (so a following GetPolicy still reports no policy / the previous revision).

Actual: the handler only refuses a principal that is blank or contains a `*` wildcard. Anything else is accepted, stored verbatim as the action group's grant, a new revisionId is minted, and the 200 response (and every later GetPolicy) renders the bogus string into the policy document's `Principal.AWS` field — a policy document that could never exist in AWS. The emulator therefore turns a 400 class into a 200 plus persisted state.

The same hole widens the blast radius of a read-modify-write client: it can successfully "grant" access to a principal that does not denote anything, and the emulator reports the group as shared.

## Evidence

Spec: `smithy_query(query="doc", shape="com.amazonaws.codeguruprofiler#PutPermissionRequest", member="principals")` → "A list ARNs for the roles and users you want to grant access to the profiling group. Wildcards are not are supported in the ARNs." The `Principals` list carries only `@length(1..50)` and its `Principal` member carries no `@pattern`, so the framework's boundary walk accepts any string — the ARN shape is the handler's to enforce (the handler's own comment says exactly this). PutPermission models ValidationException (HTTP 400).

Handler: services/service-com-amazonaws-codeguruprofiler-codeguruprofiler/src/ops/put_permission.rs:197-214 — the only principal check is `principal.trim().is_empty() || principal.contains('*')`; everything else falls through to `state::put_permission`, which stores the strings as the statement's grant (src/state.rs, `ResourcePolicy::grant` / `ComAmazonawsCodeguruprofilerCodeguruprofilerState::put_permission`) and `ResourcePolicy::document` renders them into `Principal.AWS`.

Tests: tests/all/policy_surface.rs (`principal()`/`principals()`) and tests/all/resource_policy.rs (`policy_cases()`) only ever build well-formed IAM ARNs — no test asserts a refusal for a non-ARN principal, so this behavior is unpinned. No existing open issue mentions principal validation (the open PutPermission-related trace is `put-permission-invalid-action-group-rejected`, about the actionGroup enum).

## Steps to Reproduce

1. Boot the emulator and point a codeguruprofiler client at it.
2. CreateProfilingGroup(profilingGroupName="pg-principals", clientToken=<uuid>).
3. PutPermission(profilingGroupName="pg-principals", actionGroup="agentPermissions", principals=["not-an-arn"]) — the emulator answers 200 with a policy document whose Statement[0].Principal.AWS is "not-an-arn" and a fresh revisionId, where AWS answers 400 ValidationException and writes nothing.
4. GetPolicy(profilingGroupName="pg-principals") — the bogus principal is persisted and reported, instead of the group still having no policy.
5. Repeat with principals=["123456789012"] and principals=["arn:aws:s3:::some-bucket"] (a non-IAM ARN) — both are accepted as grants.
