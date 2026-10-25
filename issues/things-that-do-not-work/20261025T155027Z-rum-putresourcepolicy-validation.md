Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 9c887f34f86da2c6771f7b9b12ec1c8e6bf552af

# After a conditional PutResourcePolicy assigns a revision id, a put that provides no PolicyRevisionId still overwrites the policy instead of being refused

_Filed: 2026-10-25T15:50:27Z_
_Operation: PutResourcePolicy | Class: validation_

> _Note: this issue was identified against the code base at commit `9c887f34f86da2c6771f7b9b12ec1c8e6bf552af`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutResourcePolicyRequest$PolicyRevisionId` is documented with a STICKY rule: once a revision id has been assigned, later requests about that policy are rejected unless they provide the correct current revision id. The crate implements the conditional check in one shared helper, `AppMonitorRecord::policy_revision_refusal`, whose first statement is `let provided = provided?;` — a request that names NO revision id returns `None` (no refusal) and is treated as an unconditional overwrite, whatever the policy's history. So a caller that has adopted the documented read-revision-then-write protocol gets no protection at all: any other caller (or a stale retry) can clobber the policy with a put that simply omits the member, and the emulator answers HTTP 200 with a fresh revision.

The helper's doc asserts the two operations share one rule — "Shared by `PutResourcePolicy` and `DeleteResourcePolicy`, which the model gives the same member and the same error" — but the model does NOT give them the same member documentation: `DeleteResourcePolicyRequest$PolicyRevisionId` conditions the refusal on "the revision ID that you provide" (so omitting it is unconditional, which the handler gets right), while the put's member conditions it on the request "not providing the correct current revision ID" once an id has been assigned. Flattening the two into one helper is what drops the put's stronger rule.

## Evidence

Smithy, `PutResourcePolicyRequest$PolicyRevisionId` (smithy_query query="doc" shape="PutResourcePolicyRequest" member="PolicyRevisionId"): "A string value that you can use to conditionally update your policy. You can provide the revision ID of your existing policy to make mutating requests against that policy. **When you assign a policy revision ID, then later requests about that policy will be rejected with an `InvalidPolicyRevisionIdException` error if they don't provide the correct current revision ID.**"

Contrast, `DeleteResourcePolicyRequest$PolicyRevisionId`: "Specifies a specific policy revision to delete. Provide a `PolicyRevisionId` to ensure an atomic delete operation. If the revision ID **that you provide** doesn't match the latest policy revision ID, the request will be rejected …"

Handler side — `services/service-com-amazonaws-rum-rum/src/state.rs:698-705`:
```
    pub fn policy_revision_refusal(
        &self,
        provided: Option<&str>,
    ) -> Option<InvalidPolicyRevisionIdException> {
        let provided = provided?;
        (self.policy_revision_id().as_deref() != Some(provided))
            .then(|| policy_revision_mismatch(provided))
    }
```
and its doc at state.rs:689-697, which elides exactly the clause that makes the rule sticky and then concludes the opposite: "… later requests about that policy will be rejected with an InvalidPolicyRevisionIdException error if they don't provide the correct current revision ID." So a request that names a revision id is CONDITIONAL on it being the current one … **and a request that names none is unconditional.** Shared by `PutResourcePolicy` and `DeleteResourcePolicy`, which the model gives the same member and the same error."

Call site: `services/service-com-amazonaws-rum-rum/src/ops/put_resource_policy.rs:254-262` passes `input.policy_revision_id.as_deref()` straight into that helper, so an absent member skips the check and line 277 stores a new revision.

Existing coverage pins the current behaviour without any model citation — `services/service-com-amazonaws-rum-rum/tests/all/resource_policy.rs:917-922`: "// A put naming no revision is unconditional: it replaces whatever is there." (asserted immediately after line 889's conditional put assigned a revision id).

## Steps to Reproduce

1. CreateAppMonitor(Name="m", Domain="example.com")
2. PutResourcePolicy(Name="m", PolicyDocument=<valid policy>) -> PolicyRevisionId "1"
3. PutResourcePolicy(Name="m", PolicyDocument=<policy B>, PolicyRevisionId="1") -> HTTP 200, PolicyRevisionId "2" — a revision id has now been assigned by the caller
4. PutResourcePolicy(Name="m", PolicyDocument=<policy C>) with NO PolicyRevisionId. Observed: HTTP 200 and PolicyRevisionId "3"; GetResourcePolicy reports policy C. Expected per the member's documentation: InvalidPolicyRevisionIdException (HTTP 400) because the request did not provide the correct current revision ID, with policy B left in place.
