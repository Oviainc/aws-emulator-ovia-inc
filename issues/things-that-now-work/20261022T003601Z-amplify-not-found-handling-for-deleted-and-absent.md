Issue-Source: trace-sweep-divergence
Issue-Trace: amplify-branch-lifecycle
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: amplify-branch-lifecycle amplify-create-app-default-state
Issue-Shape-Id: com.amazonaws.amplify#Amplify
Issue-Key: amplify|GetBranch|4xx->2xx|$.message
Issue-Key: amplify|GetBranch|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: amplify|GetBranch|4xx->2xx|status
Issue-Key: amplify|GetApp|4xx->4xx|$.message
Issue-Commit: 7d2cd57c0bb94f32b894957a728316a26e29b449

# Amplify not-found handling for deleted and absent resources diverges

> _Note: this issue was identified against the code base at commit `7d2cd57c0bb94f32b894957a728316a26e29b449`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBranch

## Description
The emulator's Amplify NotFound path is wrong in two ways: a deleted branch keeps answering 200, and the one NotFoundException it does raise carries a differently-built message. This issue bundles 2 causes.

Cause 1 of 2: GetBranch on a deleted branch returns 200 instead of 404 NotFoundException (fixture amplify-branch-lifecycle, step task#8).
AWS: Smithy GetBranch (GET /apps/{appId}/branches/{branchName}) models NotFoundException, errorKind client, httpError 404. After a successful DeleteBranch the branch no longer exists, and the recording shows AWS answering 404 with header `x-amzn-errortype: NotFoundException` and a request-derived body message naming the branch (recorded example: "Branch sim2real-probe-amplify-main-8d741ddd not found.").
Emulator: answers 200 with a branch body - no 404, no `x-amzn-errortype` header, no `message` member - so DeleteBranch leaves the branch readable.
Why: this is the deletion-visibility contract. A client that deletes a branch and polls GetBranch to confirm removal never observes it; retry/cleanup loops that treat 404 as "gone" spin forever, and code that distinguishes "absent" from "present" by catching NotFoundException takes the wrong branch and may recreate or double-delete. It also means delete is not applied to state at all, so any later read of the same app disagrees with AWS.
Fix: DeleteBranch must remove the branch from the app's branch state, and the GetBranch handler must map an absent branch to NotFoundException / HTTP 404 with the `x-amzn-errortype: NotFoundException` header.
Traces: amplify-branch-lifecycle#task#8.

Cause 2 of 2: the GetApp NotFoundException message is built without its terminating period (fixture amplify-create-app-default-state, step task#4).
AWS: Smithy GetApp models NotFoundException (client, httpError 404). Status 404 and errortype match; only the message text differs. The message rule is known because it is built from the request - the requested appId interpolated into "App <appId> not found." - so the construction, including the sentence-terminating period, is the requirement, not the recorded id (recorded example: "App d2qog9nasxmw3d not found.").
Emulator: 404 with "App d2qog9nasxmw3d not found" - identical but for the missing trailing period.
Why: low impact - clients should not parse messages - but log/snapshot comparisons and message-asserting tests diverge from AWS on every absent-app lookup.
Fix: the NotFoundException message formatter used by the GetApp handler.
Traces: amplify-create-app-default-state#task#4.

## Evidence
Replay of simparity-wire-aws-v1.

Cause 1 (amplify-branch-lifecycle#task#8, GetBranch, verdict mismatch, expected 404 / actual 200):
  status: expected 404, got 200
  $.message: missing (expected 'Branch sim2real-probe-amplify-main-8d741ddd not found.', got None)
  $headers.x-amzn-errortype: missing (expected 'NotFoundException', got None)
The preceding step in the same fixture deletes that branch successfully, so the 200 shows the delete was not reflected in state.

Cause 2 (amplify-create-app-default-state#task#4, GetApp, verdict mismatch, expected 404 / actual 404):
  $.message: value_mismatch (expected 'App d2qog9nasxmw3d not found.', got 'App d2qog9nasxmw3d not found')
Status (404) and x-amzn-errortype matched; the only diff is the absent terminating period in the request-derived message.

Model references: Smithy GetBranch errors -> NotFoundException (client, httpError 404); Smithy GetApp errors -> NotFoundException (client, httpError 404); Smithy DeleteBranch -> DeleteBranchResult.branch @required.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Cause 1: replay fixture amplify-branch-lifecycle through step task#8 - CreateBranch, DeleteBranch, then GET /apps/{appId}/branches/{branchName}. Expect 404 with x-amzn-errortype: NotFoundException and a message naming the branch; the emulator returns 200 with a branch body.
3. Cause 2: replay fixture amplify-create-app-default-state step task#4 - GET /apps/{appId} for a deleted/unknown app. Expect the message built as 'App <appId> not found.' including the terminating period.
Both steps must pass before this issue is closed.
