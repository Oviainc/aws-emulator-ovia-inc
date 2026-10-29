Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pcs.delete-dependents-out-of-order-conflict-rejected
Issue-Shape-Id: com.amazonaws.pcs#AWSParallelComputingService
Issue-Key: pcs.delete-dependents-out-of-order-conflict-rejected|_get_pcs_recommended_ami#11|AssertionError|raise AssertionError(f"no x86_64 PCS AMI parameter found under /aws/service/pcs; saw {names}")
Issue-Commit: b4a0d642bb551c1144d98d8c876f3d4bc2002490

# The AWS-published public SSM parameter namespace /aws/service/pcs is empty, so the PCS-recommended AMI id can never be resolved and no compute-node-group workflow can be built

> _Note: this issue was identified against the code base at commit `b4a0d642bb551c1144d98d8c876f3d4bc2002490`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetParametersByPath

## Description
One net-new root cause, grounded in the failing behavioral check pcs.delete-dependents-out-of-order-conflict-rejected. The check is attributed to service `pcs` (the leading token of its check id); the defective component is the emulated SSM Parameter Store public-parameter catalog that every PCS compute-node-group workflow has to read before it can call CreateComputeNodeGroup.

Cause 1 of 1: GetParametersByPath over the AWS-managed public namespace /aws/service/pcs returns an empty Parameters list, so the PCS-recommended AMI alias (.../x86_64/latest/ami-id) does not exist in the emulator.

AWS: Smithy (services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json): GetParametersByPath input Path (PSParameterName, required, @length 1..2048), Recursive (boolean), MaxResults @range 1..10, paginated by NextToken; output Parameters (ParameterList); modeled errors are only InvalidFilterKey/InvalidFilterOption/InvalidFilterValue/InvalidKeyId/InvalidNextToken (400) and InternalServerError (500) — there is no "path does not exist" refusal, an unknown path simply yields an empty page, so the model cannot distinguish "empty" from "not seeded". Recorded behavior only for the catalog itself: AWS publishes the PCS AMI aliases as public parameters that exist in every account and Region under /aws/service/pcs, so a recursive GetParametersByPath(Path='/aws/service/pcs', Recursive=true) returns a non-empty page including at least one name ending in '/x86_64/latest/ami-id', and GetParameter on that name returns a String parameter whose Value is an AMI id usable as the amiId of CreateComputeNodeGroup / as an EC2 ImageId. The requirement is that the namespace resolves non-empty and that the value it yields is an image id the emulated EC2/PCS state also accepts; the concrete parameter names, Slurm version segments and ami-xxxxxxxx value in any single run are that run's values, not part of the requirement.

Emulator: the paginated GetParametersByPath(Path='/aws/service/pcs', Recursive=True) call answers HTTP 200 with Parameters == [] on every page — no error code, just nothing. The check therefore fails at setup with AssertionError: "no x86_64 PCS AMI parameter found under /aws/service/pcs; saw []" before any PCS call under test is made. Note this is a DIFFERENT namespace and a different operation from the already-open issue 20261022T093447Z-fis-4-fidelity-divergences-cloudformation-aws-fis-experimenttemplate.md, whose Cause 3 covers GetParameter on /aws/service/ami-amazon-linux-latest/* (ParameterNotFound); that issue carries no replay key for this check and does not name /aws/service/pcs, so it does not cover this cause — a fix should seed both namespaces through the same catalog.

Why: reading the recommended AMI from the public parameter namespace is the account-independent, documented way to pick an image for a PCS compute node group. With the namespace empty, a client has no AMI to pass and every PCS workflow that provisions compute (CreateComputeNodeGroup, the queue that references it, and the whole strict delete-ordering guardrail that only exists once a node group and queue are ACTIVE) is unreachable against the emulator: it fails during setup rather than being exercised, so a test suite built on the emulator silently loses the PCS compute-plane coverage it believes it has, and a client that hardcodes a workaround AMI diverges from the one PCS would recommend.

Fix: the emulated SSM Parameter Store's public-parameter catalog and the GetParametersByPath/GetParameter handlers that read it (the same seed path the FIS issue's /aws/service/ami-amazon-linux-latest cause names) — seed the /aws/service/pcs/** aliases, including the '<slurm-version>/<os>/x86_64/latest/ami-id' leaves, make them visible to a recursive path listing (and to its pagination), and resolve their values to image ids the emulated EC2 image state and the PCS CreateComputeNodeGroup handler both accept.

Traces: pcs.delete-dependents-out-of-order-conflict-rejected (the only failing check in this suite not already filed or owned; the other twelve pcs failures are the ALREADY FILED CreateInternetGateway stub cluster or are owned by 20261023T123102Z-pcs-fidelity-bundle-5-divergences-not-found-message.md / 20261022T093447Z-fis-4-fidelity-divergences-cloudformation-aws-fis-experimenttemplate.md).

## Evidence
Behavioral suite replay (simparity-behavioral-aws-v1), service pcs 0/13: check pcs.delete-dependents-out-of-order-conflict-rejected [mismatch].

Failing frame, verbatim from the report:
  File "check_pcs_delete_dependents_out_of_order_conflict_rejected.py", line 47, in run
    ami_id = _get_pcs_recommended_ami(ctx)
  File "check_pcs_delete_dependents_out_of_order_conflict_rejected.py", line 240, in _get_pcs_recommended_ami
    raise AssertionError(f"no x86_64 PCS AMI parameter found under /aws/service/pcs; saw {names}")
  AssertionError: no x86_64 PCS AMI parameter found under /aws/service/pcs; saw []

The helper that diverges (from the check source in the report):
  ssm = ctx.client("ssm")
  names = []
  for page in ssm.get_paginator("get_parameters_by_path").paginate(Path="/aws/service/pcs", Recursive=True):
      names.extend(p["Name"] for p in page.get("Parameters", []))
  recommended = [n for n in names if n.endswith("/x86_64/latest/ami-id")]

Expected: at least one parameter name under /aws/service/pcs ending in '/x86_64/latest/ami-id', and GetParameter on it returning Parameter.Value = a usable AMI id.
Actual: every page carries Parameters == [] (HTTP 200, no error), so `names` is the empty list and the check aborts during setup — the PCS calls it exists to exercise (CreateComputeNodeGroup to ACTIVE, CreateQueue to ACTIVE, the ConflictException refusals of DeleteComputeNodeGroup/DeleteCluster out of order, then the ordered positive-control deletes) are never reached.

## Steps to Reproduce
Boot the emulator and point botocore clients for 'ssm', 'pcs' and 'ec2' at it (us-west-2), then:
1. ssm.get_paginator('get_parameters_by_path').paginate(Path='/aws/service/pcs', Recursive=True) and collect every Parameters[].Name. Expect a non-empty listing containing at least one name ending in '/x86_64/latest/ami-id'. Observe HTTP 200 with Parameters == [] on every page.
2. ssm.get_parameter(Name=<the sorted-first such name>) — not reachable today because step 1 yields nothing; expect a String parameter whose Value is an AMI id.
3. Confirm the knock-on effect: with no ami id available, pcs.create_compute_node_group(... amiId=<it> ...) cannot be composed, so the whole delete-ordering guardrail (DeleteComputeNodeGroup while a queue references the group, and DeleteCluster while a queue/node group is ACTIVE, both expected to be refused with ConflictException) is never exercised.
Equivalently, re-replay the behavioral check pcs.delete-dependents-out-of-order-conflict-rejected.
