Issue-Source: trace-sweep-divergence
Issue-Trace: email-dedicated-ip-pool-lifecycle-and-scaling
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: email-dedicated-ip-pool-lifecycle-and-scaling
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2|CreateDedicatedIpPool|4xx->4xx|$.message
Issue-Key: sesv2|GetDedicatedIpPool|2xx->2xx|$headers.content-type
Issue-Key: sesv2|ListDedicatedIpPools|2xx->2xx|$headers.content-type
Issue-Key: sesv2|GetDedicatedIpPool|2xx->2xx|$.DedicatedIpPool.ScalingMode
Issue-Key: sesv2|GetDedicatedIps|2xx->2xx|$headers.content-type
Issue-Key: sesv2|GetDedicatedIp|4xx->4xx|$headers.content-type
Issue-Key: sesv2|DeleteDedicatedIpPool|2xx->2xx|$headers.content-type
Issue-Key: sesv2|GetDedicatedIpPool|4xx->4xx|$headers.content-type
Issue-Key: sesv2|DeleteDedicatedIpPool|4xx->4xx|$headers.content-type
Issue-Commit: a9006feb831fdd789c02a15722f93d2f2e52852a

# Dedicated IP pool scaling mode is never persisted and pool responses use application/json

> _Note: this issue was identified against the code base at commit `a9006feb831fdd789c02a15722f93d2f2e52852a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDedicatedIpPool

## Description
Nine net-new causes in the SESv2 dedicated-IP plane. Smithy: ScalingMode enum [STANDARD, MANAGED]; GetDedicatedIpPool -> DedicatedIpPool {PoolName, ScalingMode}.

Cause 1 of 9: CreateDedicatedIpPool duplicate refusal message. AWS: 400 AlreadyExistsException with 'The pool <POOL-NAME> already exists.' — the fixed text is 'The pool ' + '<' + name + '>' + ' already exists.', the only variable part being the PoolName from the request. Emulator: same 400 and bracketing but an emulator-authored lead-in, 'Dedicated IP pool <POOL-NAME> already exists.'. Why: a client matching AWS's sentence does not recognise the duplicate-pool refusal. Fix: the CreateDedicatedIpPool handler's duplicate refusal message. Traces: email-dedicated-ip-pool-lifecycle-and-scaling#task#2.

Cause 2 of 9: ScalingMode is not persisted by PutDedicatedIpPoolScalingAttributes. AWS: after the pool's scaling attributes are changed to MANAGED, GetDedicatedIpPool reports DedicatedIpPool.ScalingMode = MANAGED. Emulator: the read still reports STANDARD, so the transition is accepted (no error at the write step) but discarded. Why: a client that promotes a pool to managed scaling and verifies the change sees it silently reverted, and any logic keyed on ScalingMode takes the wrong branch. Fix: the PutDedicatedIpPoolScalingAttributes handler must write ScalingMode into the dedicated-IP-pool state that GetDedicatedIpPool reads. The open issues 20261027T223115Z-sesv2-putdedicatedippoolscalingattributes-is-an-unimplemented-scaffold-stub-every.md and 20261028T103411Z-sesv2-putdedicatedippoolscalingattributes-stub.md describe the operation refusing 501; here the call no longer refuses but does not persist, so neither covers this. Traces: email-dedicated-ip-pool-lifecycle-and-scaling#task#6.

Cause 3 of 9: content-type on the successful GetDedicatedIpPool response (200). AWS: recorded behavior only — SESv2 responses carry content-type application/x-amz-json-1.1 (the model declares restJson1, whose default is application/json). Emulator: application/json. Why: a client or proxy keyed on the AWS content type mis-dispatches the body. Fix: the service's response content-type default. Traces: email-dedicated-ip-pool-lifecycle-and-scaling#task#3.

Cause 4 of 9: same content-type defect on ListDedicatedIpPools (200). Traces: email-dedicated-ip-pool-lifecycle-and-scaling#task#4.

Cause 5 of 9: same content-type defect on GetDedicatedIps (200). Traces: email-dedicated-ip-pool-lifecycle-and-scaling#task#8.

Cause 6 of 9: same content-type defect on the GetDedicatedIp 404 (NotFoundException) response. Traces: email-dedicated-ip-pool-lifecycle-and-scaling#task#9.

Cause 7 of 9: same content-type defect on the successful DeleteDedicatedIpPool response (200). Traces: email-dedicated-ip-pool-lifecycle-and-scaling#task#11.

Cause 8 of 9: same content-type defect on the GetDedicatedIpPool 404 (NotFoundException) response. Traces: email-dedicated-ip-pool-lifecycle-and-scaling#task#12.

Cause 9 of 9: same content-type defect on the DeleteDedicatedIpPool 404 (NotFoundException) response. Traces: email-dedicated-ip-pool-lifecycle-and-scaling#teardown#13.

## Evidence
Replay of trace set simparity-wire-aws-v1.
Cause 1 (task#2, 400/400): $.message expected 'The pool <sim2real-probe-sesv2-ippool-1a0c2b62> already exists.', got 'Dedicated IP pool <sim2real-probe-sesv2-ippool-1a0c2b62> already exists.'
Cause 2 (task#6, 200/200): $.DedicatedIpPool.ScalingMode expected 'MANAGED', got 'STANDARD'.
Causes 3-9 (task#3, task#4, task#8, task#9, task#11, task#12, teardown#13): status matches on every step (200/200/200/404/200/404/404); $headers.content-type expected 'application/x-amz-json-1.1', got 'application/json'.

## Steps to Reproduce
1. Replay fixture email-dedicated-ip-pool-lifecycle-and-scaling (steps task#2, task#3, task#4, task#6, task#8, task#9, task#11, task#12, teardown#13).
2. Cause 1: CreateDedicatedIpPool twice with the same PoolName and compare the 400 message.
3. Cause 2: CreateDedicatedIpPool with ScalingMode STANDARD, call PutDedicatedIpPoolScalingAttributes with MANAGED, then GetDedicatedIpPool and read DedicatedIpPool.ScalingMode.
4. Causes 3-9: inspect the content-type response header on the pool read/list/delete and the dedicated-IP read paths, including their 404s.
