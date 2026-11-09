Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol__ssm.native-ssm-parameter-delete-leaves-cloudcontrol-not-found
Issue-Key: cloudcontrol__ssm.native-ssm-parameter-delete-leaves-cloudcontrol-not-found|_poll_until#7|AssertionError|raise AssertionError(
Issue-Commit: 5bc50a64f180637d452285d750c7aafc588926d7

# Cloud Control GetResource never resolves an AWS::SSM::Parameter created by SSM's own PutParameter — it only sees resources Cloud Control created

> _Note: this issue was identified against the code base at commit `5bc50a64f180637d452285d750c7aafc588926d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResource

## Description
Cause 1 of 1: Cloud Control's read path for AWS::SSM::Parameter is backed by a Cloud-Control-local registry instead of SSM's parameter state, so a parameter created natively with ssm:PutParameter is invisible to GetResource.

AWS: Cloud Control API is a uniform front door over the resource types' own handlers — GetResource invokes the type's READ handler, which reads the live service. A parameter created by SSM's own PutParameter is therefore returned by GetResource(TypeName='AWS::SSM::Parameter', Identifier=<name>) with Identifier echoed and Properties carrying SSM's own values (Name, Type, Value, Tier...). Only after the parameter is deleted does GetResource answer ResourceNotFoundException with the handler's not-found message `Resource of type 'AWS::SSM::Parameter' with identifier '<name>' was not found.` Smithy (cloudcontrol): GetResource output ResourceDescription{Identifier, Properties}; errors include ResourceNotFoundException. Reference: https://docs.aws.amazon.com/cloudcontrolapi/latest/userguide/resource-operations.html

Emulator: GetResource never resolves the natively-created parameter at all — it stays unresolvable for the full 30 s poll with no observation recorded (`waited 30.0s for Cloud Control GetResource to resolve '<name>', a parameter it never created and it never happened; last observation None`). The emulator's own SSM API serves the same parameter, so the two planes disagree.

Why: a client that uses Cloud Control as its single read/inventory API sees only the subset of resources Cloud Control itself created. Resources created by the native API, by CloudFormation or out of band read as absent, so inventory, drift and "create-if-missing" logic all take the wrong branch — and the subsequent negative assertion (that a deleted parameter is NotFound) becomes meaningless because everything is already NotFound.

Fix: the Cloud Control GetResource/ListResources read path for AWS::SSM::Parameter — the READ handler must project the parameter out of SSM's parameter state (name, type, value, tier, tags) rather than out of a registry of Cloud-Control-originated resources.

Traces: check cloudcontrol__ssm.native-ssm-parameter-delete-leaves-cloudcontrol-not-found.

Related but not covering: open issue 20261104T172234Z-cloudcontrol-desiredstate-tags-are-never-written-to-the.md is about tags on Cloud-Control-created resources, not about reading natively-created ones.

## Evidence
Expected: after ssm PutParameter(Name=<name>, Value=<v>, Type='String'), cloudcontrol GetResource(TypeName='AWS::SSM::Parameter', Identifier=<name>) returns a ResourceDescription whose Identifier == <name> and whose Properties JSON has Name == <name> and SSM's value; after ssm DeleteParameter, the same call answers ResourceNotFoundException with message `Resource of type 'AWS::SSM::Parameter' with identifier '<name>' was not found.` (fixed text; <name> is the request's identifier).

Actual: `AssertionError: waited 30.0s for Cloud Control GetResource to resolve '<name>', a parameter it never created and it never happened; last observation None` — GetResource never returned a description for the live parameter, while ssm GetParameter(Name=<name>) succeeded in the same run.

## Steps to Reproduce
1. ssm PutParameter(Name=<name>, Value='native-original', Type='String', Tags=[...]).
2. ssm GetParameter(Name=<name>) — succeeds.
3. cloudcontrol GetResource(TypeName='AWS::SSM::Parameter', Identifier=<name>) — never resolves (poll for 30 s); it should return the parameter with SSM's values.
