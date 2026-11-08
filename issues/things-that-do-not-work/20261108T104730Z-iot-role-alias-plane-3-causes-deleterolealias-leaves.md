Issue-Source: trace-sweep-divergence
Issue-Trace: iot-role-alias-duplicate-error
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-role-alias-duplicate-error iot-role-alias-lifecycle iot-role-alias-lifecycle-and-errors
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|CreateRoleAlias|4xx->4xx|$.message
Issue-Key: iot|DescribeRoleAlias|4xx->4xx|$.message
Issue-Key: iot|DescribeRoleAlias|4xx->2xx|$.message
Issue-Key: iot|DescribeRoleAlias|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot|DescribeRoleAlias|4xx->2xx|status
Issue-Commit: 274c242a07c2045a47799c7436640bb31a4ba660

# iot role-alias plane: 3 causes — DeleteRoleAlias leaves the alias readable and both role-alias refusal messages are emulator-authored

> _Note: this issue was identified against the code base at commit `274c242a07c2045a47799c7436640bb31a4ba660`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRoleAlias

## Description
Three net-new divergences in the iot role-alias plane: the role-alias refusals are written in the emulator's own prose rather than AWS's per-resource text, and DeleteRoleAlias does not remove the alias from state so the alias remains readable afterwards. All three are iot and none is owned by an open issue (the replay reports an empty owned_by for each). The open bundles '20261107T022027Z-iot-already-exists-conflicts-9-causes-reworded-conflictexception.md' and '20261107T022027Z-iot-not-found-refusals-10-causes-emulator-authored.md' describe the same CLASS of wording defect for other iot resources, but neither carries these role-alias steps, so they do not cover them.

Cause 1 of 3: CreateRoleAlias duplicate-name refusal wording.
AWS: Smithy (com-amazonaws-iot-awsiotservice) CreateRoleAlias POST /role-aliases/{roleAlias}, errors include ResourceAlreadyExistsException @httpError 409; the recorded response is 409 with message 'Role alias <roleAlias> already exists for this account'. Fixed text: 'Role alias ' + ' already exists for this account'; the only variable part is the roleAlias value taken from the request URI label (the recording's 'sim2real-probe-iot-roledup-c70b3c85' is just that run's value, not part of the requirement).
Emulator: answers 409 but with the emulator-authored message 'RoleAlias cannot be created - name already exists (name=<roleAlias>)' — different noun ('RoleAlias' vs 'Role alias'), different sentence, and the name rendered as a 'name=' parenthetical.
Why: a client that matches or surfaces the AWS conflict sentence (log scraping, user-facing error text, tests asserting the documented message) sees text AWS never emits, so duplicate-alias handling built against AWS misbehaves even though the status and error class match.
Fix: the CreateRoleAlias handler's already-exists refusal construction (the message template for ResourceAlreadyExistsException in the role-alias handler).
Traces: iot-role-alias-duplicate-error#task#3 (cluster also covers iot-role-alias-lifecycle#task#5, iot-role-alias-lifecycle-and-errors#task#5).

Cause 2 of 3: DescribeRoleAlias not-found refusal wording.
AWS: Smithy DescribeRoleAlias GET /role-aliases/{roleAlias}, errors include ResourceNotFoundException @httpError 404; the recorded response is 404 with message 'Role alias <roleAlias> not found'. Fixed text: 'Role alias ' + ' not found'; the variable part is the requested roleAlias from the URI label.
Emulator: answers 404 with 'Role alias <roleAlias> does not exist' — the trailing clause is the emulator's own wording, not AWS's 'not found'.
Why: a client keying off the AWS not-found sentence (or a human reading it) gets a message AWS never produces for this resource, so not-found discrimination that relies on the documented text fails while the status looks correct.
Fix: the DescribeRoleAlias handler's not-found refusal construction (ResourceNotFoundException message template for role aliases).
Traces: iot-role-alias-lifecycle#task#2 (cluster also covers iot-role-alias-lifecycle-and-errors#setup#2).

Cause 3 of 3: a deleted role alias is still readable — DeleteRoleAlias does not remove it from state.
AWS: Smithy DeleteRoleAlias DELETE /role-aliases/{roleAlias} returns 200 with an empty body and the alias ceases to exist; a subsequent DescribeRoleAlias for that name must answer 404 ResourceNotFoundException (modelled on DescribeRoleAlias) with message 'Role alias <roleAlias> not found' and an x-amzn-errortype header naming the error class. The recorded teardown read is exactly that 404.
Emulator: after a successful DeleteRoleAlias, DescribeRoleAlias answers 200 and serves the alias description — no message and no x-amzn-errortype header, because the record is never removed.
Why: delete is not durable, so a client that deletes an alias and then verifies, re-creates, or garbage-collects sees the alias still present; idempotent create/delete loops and cleanup logic that depends on a 404 after delete never converge, and a stale credential-provider alias stays resolvable.
Fix: the DeleteRoleAlias handler's state mutation (removal of the alias from the role-alias store), so subsequent reads miss and fall into the not-found path of cause 2.
Traces: iot-role-alias-lifecycle#teardown#7 (cluster also covers iot-role-alias-lifecycle-and-errors#teardown#7).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, 1107 steps, 295 passed, score 26.6%).

Cause 1 — iot-role-alias-duplicate-error#task#3, CreateRoleAlias, status 409 expected and 409 received:
  expected $.message = 'Role alias sim2real-probe-iot-roledup-c70b3c85 already exists for this account'
  actual   $.message = 'RoleAlias cannot be created - name already exists (name=sim2real-probe-iot-roledup-c70b3c85)'
  divergence_key service=iot|CreateRoleAlias|status=409->409|keys=$.message; affected_count 3.

Cause 2 — iot-role-alias-lifecycle#task#2, DescribeRoleAlias, status 404 expected and 404 received:
  expected $.message = 'Role alias sim2real-probe-iot-rolealias-254bbe3f not found'
  actual   $.message = 'Role alias sim2real-probe-iot-rolealias-254bbe3f does not exist'
  divergence_key service=iot|DescribeRoleAlias|status=404->404|keys=$.message; affected_count 2.

Cause 3 — iot-role-alias-lifecycle#teardown#7, DescribeRoleAlias after DeleteRoleAlias:
  status: expected 404, got 200
  $.message: missing (expected 'Role alias sim2real-probe-iot-rolealias-254bbe3f not found', got None)
  $headers.x-amzn-errortype: missing (expected an error-type header, got None)
  divergence_key service=iot|DescribeRoleAlias|status=404->200|keys=$.message,$headers.x-amzn-errortype,status; affected_count 2.

Model evidence: Smithy CreateRoleAlias errors -> ResourceAlreadyExistsException (409); DescribeRoleAlias errors -> ResourceNotFoundException (404); DeleteRoleAlias DELETE /role-aliases/{roleAlias} -> 200 empty output. The recorded names/suffixes above are per-run values; the fixed sentence text is the requirement.

## Steps to Reproduce
1. git_clone() and boot the emulator built from this checkout.
2. replay_wire_corpus() and look at the iot role-alias fixtures.
3. Cause 1: POST /role-aliases/<name> with a roleArn to create an alias, then POST /role-aliases/<name> again with the same name. Expect 409 ResourceAlreadyExistsException with message 'Role alias <name> already exists for this account'; the emulator answers 409 with 'RoleAlias cannot be created - name already exists (name=<name>)' (fixture iot-role-alias-duplicate-error step task#3).
4. Cause 2: GET /role-aliases/<unknown-name>. Expect 404 ResourceNotFoundException with message 'Role alias <unknown-name> not found'; the emulator answers 404 with '... does not exist' (fixture iot-role-alias-lifecycle step task#2).
5. Cause 3: create an alias, DELETE /role-aliases/<name> (200), then GET /role-aliases/<name>. Expect 404 ResourceNotFoundException with the not-found message and an x-amzn-errortype header; the emulator answers 200 with the alias description (fixture iot-role-alias-lifecycle step teardown#7).
