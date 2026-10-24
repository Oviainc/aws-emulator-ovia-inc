Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.put-resource-policy-concurrent-writes-settle-on-one-policy bedrock.resource-policy-cross-account-access-enforced-denied
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock.put-resource-policy-concurrent-writes-settle-on-one-policy|run#18|AssertionError|assert results["a"] == "ok", f"PutResourcePolicy A unexpectedly failed: {results['a']}"
Issue-Key: bedrock|PutResourcePolicy|NotImplemented
Issue-Commit: ceffded050eaaeab66f25b25154c54549736906e

# Implement bedrock PutResourcePolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ceffded050eaaeab66f25b25154c54549736906e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourcePolicy

## Description
`PutResourcePolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/put_resource_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `PutResourcePolicy` call, so each one unblocks only when `PutResourcePolicy` works: `bedrock.put-resource-policy-concurrent-writes-settle-on-one-policy`, `bedrock.resource-policy-cross-account-access-enforced-denied`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/put_resource_policy.rs`. Replaying the corpus, the emulator refused the `PutResourcePolicy` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_bedrock_put_resource_policy_concurrent_writes_settle_on_one_policy.py", line 87, in run
    assert results["a"] == "ok", f"PutResourcePolicy A unexpectedly failed: {results['a']}"
           ^^^^^^^^^^^^^^^^^^^^
AssertionError: PutResourcePolicy A unexpectedly failed: (ClientError('An error occurred (NotImplemented) when calling the PutResourcePolicy operation: operation PutResourcePolicy is scaffolded but not implemented in this emulator'), '{"Version": "2012-10-17", "Statement": [{"Sid": "sim2real-both_actions", "Effect": "Allow", "Principal": {"AWS": "arn:aws:iam::000000000000:root"}, "Action": ["bedrock:ApplyGuardrail", "bedrock:GetGuardrail"], "Resource": "arn:aws:bedrock:us-west-2:000000000000:guardrail/000001g7ry0t"}]}')

## Steps to Reproduce
Boot the bedrock emulator
Call PutResourcePolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
