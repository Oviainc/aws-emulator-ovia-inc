Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: aiops.concurrent-create-investigation-group-race
Issue-Shape-Id: com.amazonaws.aiops#AIOps
Issue-Key: aiops.concurrent-create-investigation-group-race|run#7|AssertionError|assert (err.get("Code"), err.get("Message")) == _QUOTA_ERROR, f"{name}: {err}"
Issue-Commit: fec48c0e4a4ef614c94c9aeb73915e609cba5726

# Concurrent CreateInvestigationGroup losers are refused with an emulator-authored singleton-quota message instead of the service's sentence

> _Note: this issue was identified against the code base at commit `fec48c0e4a4ef614c94c9aeb73915e609cba5726`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateInvestigationGroup

## Description
One net-new root cause in the aiops (CloudWatch investigations) emulator, grounded in the failing behavioral check aiops.concurrent-create-investigation-group-race of this suite.

Context, and why this is filed rather than left to an existing issue: the open issue 20261029T192408Z-aiops-refusal-messages-are-emulator-authored-policy-document.md carries, as its cause 2, the SAME wording site for the singleton-quota refusal, but its Issue-Trace-Ids / Issue-Key list only aiops.put-investigation-group-policy-malformed-json-rejected, aiops.singleton-second-create-conflict-rejected and aiops.update-cross-account-configurations-persist. The replay key for aiops.concurrent-create-investigation-group-race is not carried by that issue or by any other open issue, so a fixer closing that issue will never replay this check. Fixing cause 2 of that issue is very likely to fix this check too; this issue exists so the concurrency check is actually re-replayed and the refusal is confirmed on the CONCURRENT path as well as the sequential one.

Cause 1 of 1: the per-account/Region singleton quota refusal of CreateInvestigationGroup states the quota RULE (interpolating the emulator's configured cap) instead of the fixed sentence the service words for that error class, and it does so on every path that hits the quota — including the concurrent one, where several racers are refused at once.

AWS: Smithy (services/service-com-amazonaws-aiops-aiops/smithy-model/com-amazonaws-aiops-aiops.json) — CreateInvestigationGroup, POST /investigationGroups, success 201, input name (@required, @length 1..512) and roleArn (@required, RoleArn @length 20..2048, pattern '^arn:.*$'); modelled errors include ServiceQuotaExceededException (errorKind client, httpError 402). CreateInvestigationGroup is documented 'Currently, you can have one investigation group in each Region in your account.' The model states no message text. Recorded behavior only for the wording: a create issued while the Region already holds the account's one group is refused with ServiceQuotaExceededException and the message 'Cannot create another investigation group.' That sentence is entirely FIXED text — it interpolates nothing: no count, no cap, no Region, no account id, no group name and no ARN from the request or from state. The fixed text IS therefore the whole requirement for this error class, and it is the same sentence whether the losing create raced other creates or was issued sequentially.

Emulator: the refusal fires with the right status and the right code (the check's own assertion chain reaches the message comparison only because the code matched), but Error.Message is 'An account can have only 1 investigation group in each Region.' — a sentence the emulator composes, and composes by formatting the emulator's cap constant into it.

Why: a client relying on AWS's behavior cannot recognise the condition from the message. Anything that keys off the quota sentence — retry/backoff selection on a losing racer, surfacing 'a group already exists in this Region' to a user, support or log tooling that distinguishes this refusal from other 402s — fails to match. Because the cap is interpolated, the message is also a function of emulator configuration: it changes if the cap constant changes, while AWS's sentence never does. In the concurrent case the damage is multiplied: every loser of a race carries the wrong sentence, so a client fanning out creates and inspecting the losers' messages to decide which one won gets no usable signal.

Fix: the wording of the CreateInvestigationGroup singleton-quota refusal, in the state helper that single-homes it — serve the service's fixed sentence and keep the cap constant out of the message entirely; the generic quota-message composition must not be the source of this text. The helper is shared by the sequential and concurrent paths, so correcting it there covers both. File and line citations under Traces.

Traces: aiops.concurrent-create-investigation-group-race. Wording site read from the checkout: services/service-com-amazonaws-aiops-aiops/src/state.rs:1406-1411 (region_quota_exceeded, a format! over MAX_INVESTIGATION_GROUPS_PER_REGION), built through quota_exceeded at src/state.rs:1392-1399.

## Evidence
Suite: simparity-behavioral-aws-v1 — 17/21 passed (81.0%); aiops 17/21 (81%), 4 failed. Three of the four failures are reported OWNED by 20261029T192408Z-aiops-refusal-messages-are-emulator-authored-policy-document.md and are skipped here. This issue is grounded in the fourth, which no open issue's replay keys cover.

aiops.concurrent-create-investigation-group-race [mismatch]: the check claims the singleton slot, frees it, then fires 5 concurrent create_investigation_group calls with distinct names and one shared roleArn through a ThreadPoolExecutor. At least one racer wins (the assertion 'winners' passed, and each winner is gettable and listed — those assertions were not reached as failures). Every LOSER must carry the quota refusal verbatim; the check asserts the (Code, Message) pair against ('ServiceQuotaExceededException', 'Cannot create another investigation group.'):

  File "check_aiops_concurrent_create_investigation_group_race.py", line 55, in run
    assert (err.get("Code"), err.get("Message")) == _QUOTA_ERROR, f"{name}: {err}"
  AssertionError: sim2real-race1-c2bd67479c88: {'Message': 'An account can have only 1 investigation group in each Region.', 'Code': 'ServiceQuotaExceededException'}

The Code element of the served pair is correct ('ServiceQuotaExceededException'); the Message element is the divergence. The racer name in the failure text ('sim2real-race1-c2bd67479c88') is a per-run generated name identifying which loser was reported first and is not part of any requirement — the served message contains no request-derived value at all, which is itself consistent with the requirement that the sentence is purely fixed text; it is simply the WRONG fixed text. The emulator's sentence also embeds '1', the formatted value of the cap constant.

Emulator wording site read from the checkout: services/service-com-amazonaws-aiops-aiops/src/state.rs:1406-1411 formats MAX_INVESTIGATION_GROUPS_PER_REGION into "An account can have only {…} investigation group in each Region." and passes it to quota_exceeded (src/state.rs:1392-1399), which sets resourceType/serviceCode and leaves resourceId/quotaCode unset.

## Steps to Reproduce
Boot the emulator and point a botocore aiops client at it (region us-west-2). Create an IAM role whose trust policy names aiops.amazonaws.com and take its ARN as <role>.

1. Claim and free the singleton slot so the race starts from an empty Region: arn0 = aiops.create_investigation_group(name=<n0>, roleArn=<role>)['arn']; aiops.delete_investigation_group(identifier=arn0).
2. Fire 5 concurrent create_investigation_group(name=<distinct name i>, roleArn=<role>) calls from a 5-worker thread pool.
3. Exactly one (or at least one) call returns 201 with an 'arn'; the others raise ClientError. Inspect each loser's exc.response['Error'].

Observed: Code == 'ServiceQuotaExceededException' (correct, HTTP 402) but Message == 'An account can have only 1 investigation group in each Region.'
Expected: Message == 'Cannot create another investigation group.' — fixed text, interpolating nothing from the request, the emulator's cap constant, the Region or the account.

Sequential control on the same wording site: with one group present in the Region, a single aiops.create_investigation_group(name=<another>, roleArn=<role>) is refused with the same wrong sentence (this is the path already covered as cause 2 of 20261029T192408Z-aiops-refusal-messages-are-emulator-authored-policy-document.md). After fixing the helper, re-replay aiops.concurrent-create-investigation-group-race specifically — the concurrent path must serve the corrected sentence to EVERY loser of the race, not just to a lone sequential caller.
