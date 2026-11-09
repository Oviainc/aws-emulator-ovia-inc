Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__connectcases.deploy-cfn-cases-domain-name-conflict-rolls-back-stack
Issue-Key: cloudformation__connectcases|TagResource|NotImplemented
Issue-Commit: e85d2009e41cfda8a7867f84cab4a050d23db70a

# cloudformation__connectcases: Connect Cases TagResource is an unimplemented scaffold stub, so a Cases domain name-conflict stack cannot be exercised

> _Note: this issue was identified against the code base at commit `e85d2009e41cfda8a7867f84cab4a050d23db70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Cause 1 of 1: Connect Cases TagResource answers NotImplemented.
AWS: TagResource on a Connect Cases resource returns HTTP 200 with an empty body and attaches the tags; it is a modelled, implemented operation.
Emulator: TagResource answers the error code `NotImplemented` with the message `operation TagResource is scaffolded but not implemented in this emulator` — a message AWS never sends for this operation.
Why: any client that tags a Cases domain (directly, or through the CloudFormation path that tags what it creates) fails, and the whole domain-conflict flow cannot be driven.
Fix: the Connect Cases TagResource operation handler and its tag state.
Traces: the failing check below.

## Evidence
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the TagResource operation: operation TagResource is scaffolded but not implemented in this emulator. Expected: HTTP 200 and the tags recorded on the resource.

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite.
2. Create a Connect Cases domain and call TagResource against its ARN.
3. Observe the NotImplemented error instead of a 200.
