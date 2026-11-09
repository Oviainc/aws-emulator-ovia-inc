Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-launch-template-versioning
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-launch-template-versioning
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:ModifyLaunchTemplate
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# Implement ec2 ModifyLaunchTemplate: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyLaunchTemplate

## Description
`ModifyLaunchTemplate` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/modify_launch_template.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ModifyLaunchTemplate` call, so each one unblocks only when `ModifyLaunchTemplate` works: `ec2-launch-template-versioning#task#3`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/modify_launch_template.rs`. Replaying the corpus, the emulator refused the `ModifyLaunchTemplate` call: status: expected 200, got 501
$.ModifyLaunchTemplateResponse: missing (expected {'requestId': '3c96888a-9d52-4c13-bf6d-76ad793aaa6a', 'launchTemplate': {'createTime': '1970-01-01T00:00:00.000Z', 'createdBy': 'arn:aws:iam::000000000000:root', 'defaultVersionNumber': '2', 'latestVersionNumber': '2', 'launchTemplateId': 'lt-0000000000000001c', 'launchTemplateName': 'sim2real-probe-ec2-lt-089649aa', 'operator': {'managed': 'false'}}}, got None)
$headers.content-type: value_mismatch (expected 'text/xml;charset=UTF-8', got 'text/xml')

## Steps to Reproduce
Boot the ec2 emulator
Call ModifyLaunchTemplate and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
