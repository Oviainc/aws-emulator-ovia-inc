Issue-Source: trace-sweep-divergence
Issue-Trace: rds-db-proxy-lifecycle
Issue-Step: teardown#21
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-db-proxy-lifecycle
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|ModifyDBProxy|4xx->2xx|status
Issue-Key: rds|DeleteDBProxy|4xx->4xx|status
Issue-Key: rds|ModifyDBProxy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 1f4cf209e4b6def02b8dc25f0332c16acb014f54

# rds DB proxy lifecycle refusals diverge: a deleted proxy is still found, and the invalid-state message is emulator-worded

> _Note: this issue was identified against the code base at commit `1f4cf209e4b6def02b8dc25f0332c16acb014f54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyDBProxy

## Description
This bundle carries 3 net-new causes, all in the rds service's DB-proxy handlers and all sharing ONE fault: the proxy's lifecycle state is not the state AWS exposes, so the refusals that depend on it come out with the wrong class, the wrong status and the wrong sentence. A DeleteDBProxy leaves the proxy record in the emulator's store, so later calls for that name are answered as if the proxy still existed, and the one state-gate refusal that is reached is worded by the emulator rather than by AWS.

Cause 1 of 3: ModifyDBProxy on a proxy that has already been deleted answers 200 instead of 404 DBProxyNotFoundFault.
AWS: Smithy ModifyDBProxy declares errors DBProxyAlreadyExistsFault (400), DBProxyNotFoundFault (client, httpError 404, awsQueryError {code DBProxyNotFoundFault, httpResponseCode 404}) and InvalidDBProxyStateFault (400). DBProxyNotFoundFault doc: "The specified proxy name doesn't correspond to a proxy owned by your Amazon Web Services account in the specified Amazon Web Services Region." Once DeleteDBProxy has completed, the name corresponds to no proxy in the account, so AWS refuses any later ModifyDBProxy for it with HTTP 404 / error code DBProxyNotFoundFault. The recording asserts the status (404); the message is the not-found sentence naming the requested DBProxyName from the request (the proxy name is the variable part, the not-found sentence is the fixed part).
Emulator: HTTP 200 with a DBProxy envelope — the deleted proxy is still resolvable and the modification is accepted.
Why: the delete is unobservable to a client. Cleanup and idempotency logic that waits for DBProxyNotFoundFault to confirm a proxy is gone never terminates, a re-create after delete can collide with a record AWS no longer has, and mutations are silently applied to a resource that does not exist.
Fix: the DB-proxy state in the rds crate (the DeleteDBProxy transition must remove the proxy once deletion completes) plus ModifyDBProxy's existence check, which must raise DBProxyNotFoundFault.
Traces: rds-db-proxy-lifecycle#teardown#21.

Cause 2 of 3: DeleteDBProxy on a proxy that has already been deleted answers 400 instead of 404 DBProxyNotFoundFault.
AWS: Smithy DeleteDBProxy declares exactly two errors — DBProxyNotFoundFault (404) and InvalidDBProxyStateFault (400, doc "The requested operation can't be performed while the proxy is in this state."). A name that no longer names a proxy is the not-found class: HTTP 404, error code DBProxyNotFoundFault. The invalid-state class applies only to a proxy that exists and is in a state the operation cannot act on.
Emulator: HTTP 400 — the proxy record survives the delete in a non-terminal 'deleting' state, so the second delete is refused by the state gate instead of the existence check.
Why: a client cannot distinguish "already gone" (an idempotent, ignorable outcome) from "wrong state" (retry later). SDK waiters and teardown scripts keyed on the error class retry a resource that will never come back, and 4xx class-based branching takes the wrong path.
Fix: the same DeleteDBProxy state transition plus DeleteDBProxy's refusal classification in the rds proxy handler.
Traces: rds-db-proxy-lifecycle#teardown#22.

Cause 3 of 3: ModifyDBProxy's invalid-state refusal message is emulator prose, not AWS's sentence.
AWS: for a proxy that exists but is not yet usable, ModifyDBProxy is refused with HTTP 400 / InvalidDBProxyStateFault and the message "DB Proxy <proxy-name> is in an unsupported state - <CURRENT-STATE>, needs to be in [ACTIVE]". Fixed text: "DB Proxy ", " is in an unsupported state - ", ", needs to be in [ACTIVE]". Variable parts: <proxy-name> is the DBProxyName from the request, and <CURRENT-STATE> is the proxy's current status in upper case (CREATING in the recording, where the proxy had just been created). The required-state list [ACTIVE] is fixed text, not a recorded value.
Emulator: same status 400 but the message is "DB proxy <proxy-name> is in state creating and cannot be modified" — a different sentence, a lower-cased state, and no statement of the state the proxy must reach.
Why: the message is the only part of this refusal that tells a caller what to wait for; a client (or a test, or an operator reading a log) that matches AWS's wording or parses the required-state list sees neither. A message that differs from AWS's wording is itself the defect.
Fix: the ModifyDBProxy state-gate refusal in the rds proxy handler — emit AWS's message template with the request's proxy name and the proxy's upper-cased current state.
Traces: rds-db-proxy-lifecycle#task#8.

No open issue covers these: the replay reports all three clusters unowned. Open issue 20261029T214632Z-rds-omits-vpc-subnet-and-availability-zone-attributes.md touches the same fixture (DescribeDBProxies envelope members and the proxy target-group status) but carries neither these steps nor these refusal paths.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (96 fixtures, 977 steps, 493 mismatched, 104 divergence clusters). All three clusters: verdict=mismatch, owned_by=[], stub_path="", affected_count=1.

Cause 1 — divergence_key 'service=rds|ModifyDBProxy|status=404->200|keys=status'; fixture rds-db-proxy-lifecycle, step teardown#21; expected_status 404, actual_status 200; detail 'status: expected 404, got 200'.

Cause 2 — divergence_key 'service=rds|DeleteDBProxy|status=404->400|keys=status'; fixture rds-db-proxy-lifecycle, step teardown#22; expected_status 404, actual_status 400; detail 'status: expected 404, got 400'.

Cause 3 — divergence_key 'service=rds|ModifyDBProxy|status=400->400|keys=$.ErrorResponse.Error.Message'; fixture rds-db-proxy-lifecycle, step task#8; expected_status 400, actual_status 400; detail '$.ErrorResponse.Error.Message: value_mismatch (expected \'DB Proxy sim2real-probe-rds-proxy-3b421e2a is in an unsupported state - CREATING, needs to be in [ACTIVE]\', got \'DB proxy sim2real-probe-rds-proxy-3b421e2a is in state creating and cannot be modified\')'. The proxy name in both strings is the fixture's generated name and is not itself a requirement; the sentence structure, the upper-cased state and the '[ACTIVE]' required-state list are.

Model evidence from the on-disk Smithy model services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/com-amazonaws-rds-amazonrdsv19.json: ModifyDBProxy errors [DBProxyAlreadyExistsFault 400, DBProxyNotFoundFault 404, InvalidDBProxyStateFault 400]; DeleteDBProxy errors [DBProxyNotFoundFault 404, InvalidDBProxyStateFault 400]; DBProxyNotFoundFault httpError 404 with awsQueryError code DBProxyNotFoundFault; InvalidDBProxyStateFault httpError 400 with doc "The requested operation can't be performed while the proxy is in this state."

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay the wire corpus (replay_wire_corpus, trace set simparity-wire-aws-v1).
3. Cause 3: replay fixture rds-db-proxy-lifecycle step task#8 — CreateDBProxy, then ModifyDBProxy while the proxy is still CREATING. The 400 InvalidDBProxyStateFault body carries the emulator's sentence instead of 'DB Proxy <name> is in an unsupported state - CREATING, needs to be in [ACTIVE]'.
4. Cause 1: replay the same fixture through teardown#21 — after the proxy has been deleted, ModifyDBProxy for that name answers 200 with a DBProxy envelope instead of 404 DBProxyNotFoundFault.
5. Cause 2: replay teardown#22 — DeleteDBProxy for the same already-deleted name answers 400 (invalid state) instead of 404 DBProxyNotFoundFault.
6. After fixing, re-replay all three bindings: rds-db-proxy-lifecycle#teardown#21, rds-db-proxy-lifecycle#teardown#22 and rds-db-proxy-lifecycle#task#8.
