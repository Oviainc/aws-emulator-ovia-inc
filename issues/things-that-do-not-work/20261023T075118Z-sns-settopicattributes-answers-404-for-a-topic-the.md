Issue-Source: trace-sweep-divergence
Issue-Trace: cfnrep-drift-modified-property-differences
Issue-Step: setup#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cfnrep-drift-modified-property-differences
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: sns|SetTopicAttributes|2xx->4xx|status
Issue-Commit: 6c57fad11ffc4b94eeb9d9eccf7474a9457dce26

# SNS SetTopicAttributes answers 404 for a topic the stack just provisioned

> _Note: this issue was identified against the code base at commit `6c57fad11ffc4b94eeb9d9eccf7474a9457dce26`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetTopicAttributes

## Description
Cause 1 of 1 - SetTopicAttributes on an existing topic is refused as not found (fixture cfnrep-drift-modified-property-differences, step setup#3).
AWS: SetTopicAttributes on a topic that exists succeeds with HTTP 200 and an empty SetTopicAttributesResponse; a 404 NotFound is reserved for a TopicArn that does not exist. Recorded response for this step: 200.
Emulator: HTTP 404 for the topic the preceding setup steps provisioned, so the attribute write never lands.
Why: every out-of-band mutation of a topic - the ordinary way a client changes DisplayName or a delivery policy, and the way CloudFormation drift is induced and then detected - fails with a not-found error against a topic that demonstrably exists. A client cannot distinguish 'wrong ARN' from 'this topic is invisible to SNS', and the whole drift scenario built on top of it cannot run. Hypothesis: the topic was created through a path that did not register it in SNS's own topic state, so SNS's existence lookup misses it.
Fix: the SetTopicAttributes handler's topic lookup and the SNS topic state it consults - a topic created earlier in the same run must resolve, and the ARN produced at creation must be the key the lookup accepts.
Traces: cfnrep-drift-modified-property-differences#setup#3.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, state_reset_failures=0):

cfnrep-drift-modified-property-differences#setup#3 SetTopicAttributes: status expected 200, got 404. No other member diverged - the step produced no response body to compare because the request was refused outright, which also blocks the drift steps the fixture builds on this setup.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay fixture cfnrep-drift-modified-property-differences up to and including step setup#3, which calls SNS SetTopicAttributes against the topic the earlier setup steps provisioned.
3. Observe HTTP 404 where the recording has 200.
