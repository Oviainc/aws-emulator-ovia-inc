Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__lambda__s3.delete-subnet-rejected-while-an-orphaned-interface-survives-instance-termination
Issue-Key: ec2__lambda__s3.delete-subnet-rejected-while-an-orphaned-interface-survives-instance-termination|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Commit: 620c90c23a321396910fd66a692ef2fcecf33f7f

# DeleteSubnet is not refused for an orphaned network interface left behind by instance termination

> _Note: this issue was identified against the code base at commit `620c90c23a321396910fd66a692ef2fcecf33f7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteSubnet

## Description
One net-new root cause.

Cause 1 of 1: an interface that outlives its instance is never reflected as a subnet dependency, so the scenario never reaches the refusal it waits for.
AWS: terminating an instance releases the interfaces it created but leaves an independently created (orphaned) ENI in the subnet; DeleteSubnet is then refused DependencyViolation — "The subnet '<subnet id>' has dependencies and cannot be deleted." — until that interface is deleted (recorded behavior).
Emulator: the scenario never converges — the check ends "Check timed out after 60.0s" waiting for the orphaned interface to survive termination and for the refusal to appear.
Why: a teardown order that AWS enforces is not enforced, so cleanup code that relies on DependencyViolation to discover a leftover ENI never sees it.
Fix: the instance-termination path's ENI ownership/detach bookkeeping and DeleteSubnet's dependency check.
Traces: ec2__lambda__s3.delete-subnet-rejected-while-an-orphaned-interface-survives-instance-termination.

## Evidence
From the behavioral replay:
- ec2__lambda__s3.delete-subnet-rejected-while-an-orphaned-interface-survives-instance-termination [mismatch]: Error: Check timed out after 60.0s.

## Steps to Reproduce
Replay the behavioral suite and re-run ec2__lambda__s3.delete-subnet-rejected-while-an-orphaned-interface-survives-instance-termination: create a VPC/subnet, create an ENI in it, attach it to an instance, terminate the instance, then confirm the ENI survives and DeleteSubnet is refused DependencyViolation until the ENI is deleted.
