Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: agent-registry-control__cloudformation.deploy-agent-registry-record-round-trip-and-delete agent-registry-control__cloudformation.deploy-agent-registry-round-trip-and-delete agent-registry-control__cloudformation.deploy-agent-registry-record-nonexistent-registry-rejected
Issue-Key: agent-registry-control__cloudformation|CreateStack|ValidationError
Issue-Key: agent-registry-control__cloudformation|ListRegistries|NotImplemented
Issue-Commit: 86c25e3173c48b1c4d4a9173324e183693bfe876

# agent-registry-control__cloudformation: AWS::AgentRegistry::{Registry,RegistryRecord} are unregistered resource types and ListRegistries is an unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `86c25e3173c48b1c4d4a9173324e183693bfe876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two independent causes behind the 3 failing agent-registry-control__cloudformation checks.

Cause 1 of 2: AWS::AgentRegistry::Registry and AWS::AgentRegistry::RegistryRecord are missing from the CloudFormation template resource-type registry.
AWS: recorded behavior only — CreateStack accepts both types, provisions the registry and its record through the Agent Registry control plane, and reaches CREATE_COMPLETE; a record naming a registry that does not exist instead rolls the stack back with the service's own refusal as the resource failure reason.
Emulator: CreateStack answers 400 ValidationError `Template format error: Unrecognized resource types: [AWS::AgentRegistry::Registry, AWS::AgentRegistry::RegistryRecord]` (and `[AWS::AgentRegistry::Registry]` for the single-resource template), so nothing is created.
Why: no agent registry or registry record can be deployed from a template; the user is told these AWS resource types do not exist.
Fix: register both types in the CloudFormation template resource-type registry and add a resource handler per type over the agent-registry-control crate.
Traces: agent-registry-control__cloudformation.deploy-agent-registry-record-round-trip-and-delete, agent-registry-control__cloudformation.deploy-agent-registry-round-trip-and-delete

Cause 2 of 2: ListRegistries answers NotImplemented.
AWS: recorded behavior only — ListRegistries answers 200 with the account's registries (an empty list when there are none); it is a normal read operation, never a server error.
Emulator: `An error occurred (NotImplemented) when calling the ListRegistries operation: operation ListRegistries is scaffolded but not implemented in this emulator` — the handler is a scaffold stub.
Why: a client cannot enumerate registries at all, so it cannot discover existing registries, confirm a create, or establish the control a negative test needs. (This check is NOT marked ALREADY FILED in the replay report, so no stub issue covers it.)
Fix: implement the ListRegistries handler in the agent-registry-control crate's ops module over the registry state the create path writes.
Traces: agent-registry-control__cloudformation.deploy-agent-registry-record-nonexistent-registry-rejected

## Evidence
Cause 1 — botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::AgentRegistry::Registry, AWS::AgentRegistry::RegistryRecord] / [AWS::AgentRegistry::Registry]. Expected 200 + StackId and CREATE_COMPLETE.

Cause 2 — botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListRegistries operation: operation ListRegistries is scaffolded but not implemented in this emulator. Expected 200 with the Registries list.

## Steps to Reproduce
1. Cause 1: CreateStack with a template declaring AWS::AgentRegistry::Registry (optionally with ::RegistryRecord); observe the 400 ValidationError "Unrecognized resource types".
2. Cause 2: call ListRegistries; observe NotImplemented instead of 200 with a Registries list.
3. Re-replay the 3 checks in trace_ids.
