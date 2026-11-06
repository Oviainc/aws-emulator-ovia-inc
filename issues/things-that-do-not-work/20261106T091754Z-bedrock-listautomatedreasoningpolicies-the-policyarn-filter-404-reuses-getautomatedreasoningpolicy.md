Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.delete-automated-reasoning-policy-in-use-rejected
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock.delete-automated-reasoning-policy-in-use-rejected|run#23|AssertionError|assert list_gone == list_gone_expected, (
Issue-Commit: 1c8a1289845eaeeaa26c5e9db61e4b06e58a51c0

# bedrock ListAutomatedReasoningPolicies: the policyArn-filter 404 reuses GetAutomatedReasoningPolicy's message text, dropping the trailing period AWS sends

> _Note: this issue was identified against the code base at commit `1c8a1289845eaeeaa26c5e9db61e4b06e58a51c0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAutomatedReasoningPolicies

## Description
Cause 1 of 1: ListAutomatedReasoningPolicies' ResourceNotFoundException message text is wrong — the emulator serves the sibling Get operation's wording for it.

AWS: ListAutomatedReasoningPolicies takes an optional `policyArn` filter (Smithy: ListAutomatedReasoningPolicies.policyArn, AutomatedReasoningPolicyArn, httpQuery `policyArn`, len 1..2048, pattern ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$; modeled error ResourceNotFoundException, HTTP 404). When the filter names an ARN the account holds no revision of — e.g. a policy deleted by DeleteAutomatedReasoningPolicy — AWS refuses the listing with 404 ResourceNotFoundException rather than returning an empty page. The model documents only the generic shape doc ("The specified resource Amazon Resource Name (ARN) was not found..."), so the concrete message is recorded behavior only: the recording shows ListAutomatedReasoningPolicies answering code `ResourceNotFoundException` with message `Policy does not exist.` — the whole message is FIXED text, terminated by a period, with no variable part (the ARN is not interpolated). The recording also shows that this wording is NOT shared with GetAutomatedReasoningPolicy: the same dead ARN on Get yields `ResourceNotFoundException` / `Policy does not exist` with NO trailing period, and the behavioral check asserts both spellings separately in the same run.

Emulator: for the dead-ARN filtered listing the emulator answers 404 `ResourceNotFoundException` with message `Policy does not exist` — no trailing period. The status and the error code are correct; only the message text diverges. The Get leg of the same check passes, so the emulator is serving Get's wording for List: the List handler builds the message from the store helper `automated_reasoning_policy_revision_not_found_message(policy_arn)` (services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/list_automated_reasoning_policies.rs lines 227-238), a single helper shared with the Get path, so the one operation that needs the period-terminated spelling cannot produce it.

Why: a client that matches on the error message — error-message assertions in tests, log/alert rules, SDK retry-and-classify wrappers, or anything that distinguishes which Automated Reasoning API refused — sees a message that AWS never sends for this operation. Because the two sibling operations differ in exactly this one character on real AWS, code written against real AWS and keyed on List's message silently stops matching against the emulator (and vice versa), while a generic code-only handler still works, so the break is invisible until the message path is exercised.

Fix: the ListAutomatedReasoningPolicies handler's refusal message for the unresolvable `policyArn` filter — services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/list_automated_reasoning_policies.rs, the ResourceNotFoundException it constructs when `store.automated_reasoning_policy_revision(policy_arn)` yields nothing. The shared state helper it calls serves two operations whose AWS wording differs by a trailing period, so List needs its own message text rather than the Get spelling (or the helper needs to be parameterised per operation). Nothing else about this refusal changes: it stays 404 with code ResourceNotFoundException, and it must stay a refusal rather than an empty page.

Traces: bedrock.delete-automated-reasoning-policy-in-use-rejected — the check's earlier legs (the ValidationException "There are build workflow that have not been deleted." refusal while a build workflow exists, the policy surviving the rejected delete, and the Get 404 message) all pass; it fails only on the filtered-list message.

## Evidence
Failing check bedrock.delete-automated-reasoning-policy-in-use-rejected (verdict: mismatch).

Assertion from the replay:
  AssertionError: expected a filtered list on a dead policy to 404 with ('ResourceNotFoundException', 'Policy does not exist.'), got ('ResourceNotFoundException', 'Policy does not exist')

Expected (AWS, as recorded):
  ListAutomatedReasoningPolicies(policyArn=<deleted policy ARN>)
    -> HTTP 404, Error.Code = "ResourceNotFoundException",
       Error.Message = "Policy does not exist."   (fixed text, trailing period, no interpolated values)

Actual (emulator):
  ListAutomatedReasoningPolicies(policyArn=<deleted policy ARN>)
    -> HTTP 404, Error.Code = "ResourceNotFoundException",
       Error.Message = "Policy does not exist"    (no trailing period)

Control, passing in the same run — confirms the two messages are genuinely distinct on AWS and that only List diverges:
  GetAutomatedReasoningPolicy(policyArn=<same deleted ARN>)
    -> HTTP 404, "ResourceNotFoundException", "Policy does not exist"  (no period) — emulator matches.

Source of the shared wording:
  services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/list_automated_reasoning_policies.rs:227-238 builds the message via
  store.automated_reasoning_policy_revision_not_found_message(policy_arn), the same helper the Get path uses.

Smithy (services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json):
  ListAutomatedReasoningPolicies: GET /automated-reasoning-policies, success 200; errors include ResourceNotFoundException (client, httpError 404); input member policyArn is an optional httpQuery filter. The model carries no message template, hence "recorded behavior only" for the text.

## Steps to Reproduce
1. bedrock.create_automated_reasoning_policy(name=<unique>, policyDefinition={}) -> capture policyArn.
2. bedrock.start_automated_reasoning_policy_build_workflow(policyArn=..., buildWorkflowType="INGEST_CONTENT", sourceContent={...}) and wait for the workflow to reach COMPLETED.
3. bedrock.delete_automated_reasoning_policy_build_workflow(policyArn=..., buildWorkflowId=..., lastUpdatedAt=<workflow updatedAt>).
4. bedrock.delete_automated_reasoning_policy(policyArn=...)  -> succeeds; the policy is now gone.
5. bedrock.list_automated_reasoning_policies(policyArn=<the deleted ARN>) and read the ClientError's Error.Code and Error.Message.
   Observed: ("ResourceNotFoundException", "Policy does not exist").
   Expected: ("ResourceNotFoundException", "Policy does not exist.").
6. For the contrast, bedrock.get_automated_reasoning_policy(policyArn=<same ARN>) -> ("ResourceNotFoundException", "Policy does not exist") with no period, which is correct and must stay unchanged.

Replay the whole check with: bedrock.delete-automated-reasoning-policy-in-use-rejected
