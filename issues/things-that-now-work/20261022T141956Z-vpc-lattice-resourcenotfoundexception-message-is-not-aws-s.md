Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: vpc-lattice.delete-service-network-with-active-service-association-rejected vpc-lattice.delete-service-with-active-service-network-association-rejected vpc-lattice.deleting-a-service-network-is-rejected-while-an-association-survives-it
Issue-Shape-Id: com.amazonaws.vpclattice#MercuryControlPlane
Issue-Key: vpc-lattice.delete-service-network-with-active-service-association-rejected|run#28|AssertionError|assert message == expected, (
Issue-Key: vpc-lattice.delete-service-with-active-service-network-association-rejected|run#29|AssertionError|assert message == expected, (
Issue-Key: vpc-lattice|GetServiceNetwork|ResourceNotFoundException
Issue-Commit: c73ebf67a23d923b166243bec836d38257c16c68

# VPC Lattice ResourceNotFoundException message is not AWS's "<ResourceType> <id> cannot be found." construction

> _Note: this issue was identified against the code base at commit `c73ebf67a23d923b166243bec836d38257c16c68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetServiceNetwork

## Description
The emulator's vpc-lattice referential-integrity guards and delete lifecycle behave correctly (DeleteServiceNetwork/DeleteService are refused with 409 ConflictException while an association is live, the refused delete leaves the resource and the association intact, the association count is right, and the identical delete is accepted once the association is gone — all of those assertions passed). ONE cause remains, in the not-found error body produced after a successful delete.

Cause 1 of 1: ResourceNotFoundException.message for a deleted/absent service network or service is built with internal lowercase resource naming instead of AWS's construction.
AWS: Smithy (com.amazonaws.vpclattice#ResourceNotFoundException, httpError 404) declares `message`, `resourceId` and `resourceType` as REQUIRED String members; the model does not state the text, so: recorded behavior only — every recorded vpc-lattice not-found response builds `message` from the request as "<ResourceType> <identifier> cannot be found.", with the resource type in AWS's CamelCase wire spelling and the identifier exactly as the caller passed it (e.g. "ServiceNetwork sn-... cannot be found." for GetServiceNetwork, "Service svc-... cannot be found." for GetService). The rule is therefore known — the text is derived from the resource kind plus the submitted identifier, not free prose. Status (404) and error code (ResourceNotFoundException) are correct in the emulator and are not part of this divergence.
Emulator: it emits a lowercase, space-separated internal resource name and a different trailing form: 'service network <id> not found' for a service network and 'service <id> not found' for a service. The identifier is right; the resource-type token and the "cannot be found." construction are not. This is the same shared not-found formatter for both resource kinds.
Why: a client (or a resource provider / test harness) that surfaces or matches the not-found text to tell an operator which resource kind disappeared sees neither AWS's resource-type spelling nor AWS's sentence form, so any such reporting or assertion diverges; the divergence is systematic rather than a single recorded string, and it appears on the normal post-delete read path of two different resource kinds.
Fix: the service's shared error-construction path for ResourceNotFoundException (the formatter used by the get/read handlers for service network and service) — render the resource kind to its AWS wire token and format the message as "<ResourceType> <identifier> cannot be found." from the request identifier, rather than reusing the internal lowercase resource label and a bespoke 'not found' suffix.
Related: the open wire issue "VPC Lattice error payloads: wrong resourceType token and non-AWS message construction" (20261022T044730Z-vpc-lattice-error-payloads-wrong-resourcetype-token-and.md) describes the same message construction under its Cause 3, but it carries only wire replay keys; the behavioral checks below are not covered by its keys and must be re-replayed to confirm this cause is closed.
Traces: vpc-lattice.delete-service-network-with-active-service-association-rejected, vpc-lattice.delete-service-with-active-service-network-association-rejected, vpc-lattice.deleting-a-service-network-is-rejected-while-an-association-survives-it.

## Evidence
Behavioral replay of simparity-behavioral-aws-v1 (vpc-lattice 1/5 passed; 3 mismatches, 1 error already filed against the unimplemented CreateRule stub).

All three mismatches are the same assertion, reached only after every ConflictException-guard assertion in the check had already passed:
- vpc-lattice.delete-service-network-with-active-service-association-rejected: after the association was removed and DeleteServiceNetwork succeeded, GetServiceNetwork raised 404 ResourceNotFoundException (correct code) with message 'service network sn-g8e2ue6jhsmn065tg not found'; AWS's construction gives 'ServiceNetwork sn-g8e2ue6jhsmn065tg cannot be found.'
- vpc-lattice.deleting-a-service-network-is-rejected-while-an-association-survives-it: same path, message 'service network sn-t5r6dee9mgw332vi1 not found'; AWS: 'ServiceNetwork sn-t5r6dee9mgw332vi1 cannot be found.'
- vpc-lattice.delete-service-with-active-service-network-association-rejected: after DeleteService succeeded, GetService raised 404 ResourceNotFoundException with message 'service svc-apyvzagzr32jqxnw1 not found'; AWS: 'Service svc-apyvzagzr32jqxnw1 cannot be found.'

The identifiers above are environment-generated and are examples only; the requirement is the construction "<ResourceType> <identifier> cannot be found." with the AWS resource-type spelling (ServiceNetwork, Service).

## Steps to Reproduce
1. Point a botocore vpc-lattice client at the emulator.
2. CreateServiceNetwork(name=..., authType='NONE'); CreateService(name=..., authType='NONE'); wait for the service to reach ACTIVE.
3. CreateServiceNetworkServiceAssociation for the pair and wait for ACTIVE.
4. DeleteServiceNetwork -> correctly refused with 409 ConflictException (this part passes).
5. DeleteServiceNetworkServiceAssociation, wait until it is gone, then DeleteServiceNetwork -> accepted.
6. GetServiceNetwork(serviceNetworkIdentifier=<id>) and inspect the 404 body: $.message is 'service network <id> not found' instead of 'ServiceNetwork <id> cannot be found.'
7. Repeat steps 2-6 from the service end (DeleteService after the association is gone) and inspect GetService's 404 body: $.message is 'service <id> not found' instead of 'Service <id> cannot be found.'
8. Re-replay the three check ids under trace_ids; all three must pass.
