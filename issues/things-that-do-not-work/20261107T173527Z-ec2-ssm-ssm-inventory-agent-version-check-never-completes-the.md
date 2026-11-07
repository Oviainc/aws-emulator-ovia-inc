Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ssm.vscan-inventory-agent-version-matches-the-installed-rpm-on-a-deprecated-ami
Issue-Key: ec2__ssm.vscan-inventory-agent-version-matches-the-installed-rpm-on-a-deprecated-ami|_poll#10|Timeout|last = probe()
Issue-Commit: edd8d27229a8bc99e308a4b32d393f13d6f32cb7

# SSM inventory agent-version check never completes: the deprecated-AMI inventory flow exhausts the replay budget with no response

> _Note: this issue was identified against the code base at commit `edd8d27229a8bc99e308a4b32d393f13d6f32cb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceInformation

## Description
Cause 1 of 1: the SSM inventory flow that compares a managed node's reported agent version with the RPM installed in the guest never converges.
AWS: recorded behavior only — the recording drives this flow (launch an instance from a deprecated AMI with an SSM instance profile, let it register as a managed node, read its inventory agent version and compare it with the version the guest's RPM database reports) to completion inside the suite's budget, with DescribeInstanceInformation/inventory reads answering on each poll.
Emulator: the check produces no assertion at all — it ends as `Check timed out after 180.0s`. The only evidence is budget exhaustion, so no single operation is named; the sibling ec2__ssm checks that do report are owned by 20261107T075601Z-ec2-ssm-windows-amis-report-platform-windows-instead.md, which does not carry this replay key. Hypothesis: the managed-node registration or the inventory read for the deprecated-AMI instance never settles, so every poll in the flow waits out its window.
Why: a client that reconciles SSM-reported agent versions against what is installed in the guest gets a flow that hangs instead of an answer, and the agent-version divergence this check exists to detect is unobservable.
Fix: the SSM managed-node registration and inventory read path for an EC2 instance launched from a deprecated AMI must converge; re-replay the check and start from whichever poll does not settle (SSM instance-information/inventory state and the guest's reported agent version).
Traces: ec2__ssm.vscan-inventory-agent-version-matches-the-installed-rpm-on-a-deprecated-ami.

## Evidence
Expected: the check completes and compares the inventory-reported SSM agent version with the guest's installed RPM version.
Actual: `Error: Check timed out after 180.0s` — no response or assertion was produced (budget exhaustion is the whole of the recorded evidence).

## Steps to Reproduce
1. Replay the behavioral suite and look under `### ec2__ssm` for vscan-inventory-agent-version-matches-the-installed-rpm-on-a-deprecated-ami.
2. Launch an instance from a deprecated Amazon Linux AMI with an instance profile carrying AmazonSSMManagedInstanceCore, then poll ssm DescribeInstanceInformation and the instance's inventory for its agent version while running a command in the guest to read the installed agent RPM version.
3. Observe that the flow does not complete within the replay budget; record which poll never settles.
