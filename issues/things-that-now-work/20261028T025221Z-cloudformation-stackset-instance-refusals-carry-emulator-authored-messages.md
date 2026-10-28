Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.concurrent-stack-instance-update-rejected cloudformation.stack-set-instance-lifecycle cloudformation.update-stack-set-invalid-region-rejected
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.concurrent-stack-instance-update-rejected|run#45|AssertionError|assert msg_nf == "StackInstance not found", msg_nf
Issue-Key: cloudformation|UpdateStackInstances|StackInstanceNotFoundException
Issue-Key: cloudformation.update-stack-set-invalid-region-rejected|run#12|AssertionError|assert error == expected, f"expected {expected!r}, got {error!r}"
Issue-Commit: ccdeb765042b765c77fa3564f54cdee577f4e50e

# CloudFormation StackSet-instance refusals carry emulator-authored messages: StackInstanceNotFoundException (Describe/UpdateStackInstances) and the invalid-Region ValidationError

> _Note: this issue was identified against the code base at commit `ccdeb765042b765c77fa3564f54cdee577f4e50e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackInstance

## Description
Three net-new root causes in the CloudFormation StackSets refusal path. Each one is a refusal whose HTTP status and error code already match AWS but whose MESSAGE is text this emulator invented, so a client that reads the message (or a test/tool that asserts on it) sees something AWS never says. The messages come from two shared helpers in this crate's state module, which deliberately answer ONE sentence per exception type regardless of which operation refused — AWS words these per operation. The three causes must all be closed before this issue is done.

Cause 1 of 3: DescribeStackInstance's StackInstanceNotFoundException message.
AWS: DescribeStackInstance models StackInstanceNotFoundException (HTTP 404, awsQueryError code StackInstanceNotFoundException; Smithy doc: "The specified stack instance doesn't exist"); the model does not state the message text, so this is recorded behavior only. For a StackSet whose instance in the requested (account, Region) pair has been deleted, AWS answers the fixed message `StackInstance not found` — the whole message is fixed text, with NO variable part (the account and Region the request named do not appear).
Emulator: returns code StackInstanceNotFoundException but the message `Stack instance in account <account> and Region <region> not found` (observed: "Stack instance in account 000000000000 and Region us-east-1 not found"; the account id and Region there are values from the request/state filling the emulator's own template, not part of the requirement).
Why: a client or deployment tool that distinguishes CloudFormation refusals by message text — and any assertion/log scrape of DescribeStackInstance's not-found answer — sees a sentence AWS never emits, so not-found handling that is exercised against real AWS does not match here.
Fix: the StackInstanceNotFoundException message built in the DescribeStackInstance handler, i.e. the shared message helper it calls rather than the handler's control flow.
Traces: cloudformation.concurrent-stack-instance-update-rejected (fails at the final DescribeStackInstance assertion after the instance was deleted; see services/service-com-amazonaws-cloudformation-cloudformation/src/ops/describe_stack_instance.rs lines 180-189 calling crate::state::stack_instance_does_not_exist).

Cause 2 of 3: UpdateStackInstances's StackInstanceNotFoundException message.
AWS: UpdateStackInstances models StackInstanceNotFoundException (HTTP 404, awsQueryError code StackInstanceNotFoundException). The model does not state the message text, so recorded behavior only: when a selected (account, Region) pair holds no instance (e.g. it was just deleted), AWS refuses with the message `No Stack Instance found in region <region>` — fixed text `No Stack Instance found in region ` plus ONE variable part, the Region name taken from the request's Regions list (observed with region us-west-2). Note that AWS's wording here is operation-specific and differs from DescribeStackInstance's (Cause 1).
Emulator: returns the same generic sentence as Cause 1, `Stack instance in account <account> and Region <region> not found`, for UpdateStackInstances too — the code comment at update_stack_instances.rs lines 419-423 states this is intentional ("the exception reads the same whichever operation addressed the instance"), which is the defect: AWS does not share one sentence across these operations.
Why: a caller that mutates a deleted instance gets a message AWS never returns; tooling that parses the Region out of UpdateStackInstances' not-found message cannot find it, and error-text-based retry/branching diverges.
Fix: the StackInstanceNotFoundException message built in the UpdateStackInstances handler — it must stop reusing the shared describe-shaped sentence and word the refusal as the operation does.
Traces: cloudformation.stack-set-instance-lifecycle (asserts the UpdateStackInstances refusal message on a deleted instance; see services/service-com-amazonaws-cloudformation-cloudformation/src/ops/update_stack_instances.rs lines 417-434 calling crate::state::stack_instance_does_not_exist).

Cause 3 of 3: the invalid-Region ValidationError message on CreateStackInstances.
AWS: CreateStackInstances takes Regions (RegionList of Region, pattern ^[a-zA-Z0-9-]{1,128}$). A Region that is syntactically valid but is not a real Region is refused wholesale (no instance is created in the valid Regions named alongside it) with the service-common ValidationError (HTTP 400). The model does not state the text, so recorded behavior only: the message is `Region <region> is not supported` — fixed text `Region ` + the offending Region value from the request + ` is not supported`.
Emulator: same code ValidationError, but the message `Region <region> is not a supported Amazon Web Services Region` (observed: "Region not-a-real-region-1 is not a supported Amazon Web Services Region"; the Region token is the request value, the rest is emulator-authored fixed text).
Why: the request is correctly rejected, but a client surfacing or matching the validation message shows wording AWS never produces; the same shared helper is used by the sibling Region validations (UpdateStackInstances/DeleteStackInstances call it too), so the divergence is systematic across the StackSets Region check.
Fix: the Region validation message in the CreateStackInstances handler's validation step — the shared helper crate::state::stack_sets::region_does_not_exist that builds it.
Traces: cloudformation.update-stack-set-invalid-region-rejected (CreateStackInstances with one valid and one bogus Region; the emulator's refusal code and its no-partial-effect behavior are correct, only the message diverges).

Related but NOT covering these: open issue 20261025T015306Z-cloudformation-is-not-region-scoped-stack-names-and.md concerns region scoping of stacks and stack-set deployments, not the wording of these three refusals.

## Evidence
Cause 1 — cloudformation.concurrent-stack-instance-update-rejected, after DeleteStackInstances settled:
  expected (AWS): code StackInstanceNotFoundException, message "StackInstance not found"
  actual (emulator): code StackInstanceNotFoundException, message "Stack instance in account 000000000000 and Region us-east-1 not found"
  (AssertionError: assert msg_nf == "StackInstance not found")

Cause 2 — cloudformation.stack-set-instance-lifecycle, UpdateStackInstances on the deleted instance:
  expected (AWS): code StackInstanceNotFoundException, message "No Stack Instance found in region us-west-2" (fixed text + Region from the request)
  actual (emulator): message "Stack instance in account 000000000000 and Region us-west-2 not found"
  (AssertionError: assert mutate_msg == f"No Stack Instance found in region {region}")

Cause 3 — cloudformation.update-stack-set-invalid-region-rejected, CreateStackInstances(Regions=[<ctx.region>, "not-a-real-region-1"]):
  expected (AWS): ('ValidationError', 'Region not-a-real-region-1 is not supported')
  actual (emulator): ('ValidationError', 'Region not-a-real-region-1 is not a supported Amazon Web Services Region')

In all three the HTTP status and error code match; only the message text diverges. Emulator sources confirming the single shared sentences: ops/describe_stack_instance.rs:180-189 and ops/update_stack_instances.rs:417-434 both call crate::state::stack_instance_does_not_exist(account, region); ops/update_stack_instances.rs:296-300 and the CreateStackInstances sibling call crate::state::stack_sets::region_does_not_exist(unknown).

## Steps to Reproduce
Cause 1:
1. CreateStackSet (PermissionModel SELF_MANAGED, with admin/execution roles) with a one-resource template.
2. CreateStackInstances for your account in one Region; wait for the operation to SUCCEED.
3. DeleteStackInstances for that account/Region (RetainStacks=false); wait for the operation to SUCCEED.
4. Call DescribeStackInstance(StackSetName, StackInstanceAccount, StackInstanceRegion) for that pair and read Error.Message. Expect exactly "StackInstance not found".

Cause 2:
5. With the same StackSet and the instance deleted, call UpdateStackInstances(StackSetName, Accounts=[account], Regions=[region], ParameterOverrides=[]) and read Error.Message. Expect exactly "No Stack Instance found in region <region>".

Cause 3:
6. CreateStackSet as above, then call CreateStackInstances(StackSetName, Accounts=[account], Regions=[<a valid region>, "not-a-real-region-1"]). Expect ValidationError with message "Region not-a-real-region-1 is not supported", and ListStackInstances to remain empty (the no-partial-effect part already passes).
