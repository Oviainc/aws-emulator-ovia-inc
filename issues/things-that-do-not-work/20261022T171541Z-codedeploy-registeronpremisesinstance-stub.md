Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.batch-get-deployment-instances-matches-individual-gets codedeploy.batch-get-deployment-targets-matches-individual-gets codedeploy.batch-get-deployment-targets-reflects-instances codedeploy.batch-get-deployments-matches-individual-gets codedeploy.batch-get-on-premises-instances-matches-individual-gets codedeploy.deployment-create-get-list-lifecycle codedeploy.deployment-create-stop-status-transition codedeploy.deployment-instance-lifecycle-transitions codedeploy.deployment-target-lifecycle-transitions codedeploy.deregister-invalid-instance-name-rejected codedeploy.deregister-onprem-instance-invalid-name-rejected codedeploy.e2e-application-deploymentgroup-deployment-stop-flow codedeploy.e2e-on-premises-instance-tag-and-deploy codedeploy.e2e-onprem-deployment-full-flow codedeploy.e2e-onprem-instance-tag-filter-flow codedeploy.get-deployment-target-not-started-rejected codedeploy.on-premises-iam-user-arn-reuse-rejected codedeploy.on-premises-instance-deregister-reflects-in-status-filters codedeploy.on-premises-instance-register-deregister-lifecycle codedeploy.on-premises-instance-tags-round-trip codedeploy.onprem-instance-duplicate-name-registration-rejected codedeploy.onprem-instance-duplicate-registration-rejected codedeploy.onprem-instance-list-reflects-registration-status codedeploy.onprem-instance-register-get-deregister-lifecycle codedeploy.onprem-instance-tags-round-trip codedeploy.onprem-instance-tags-unicode-round-trip codedeploy.register-on-premises-instance-missing-iam-arn-rejected codedeploy.register-onprem-instance-duplicate-iam-session-arn-rejected codedeploy.register-onprem-instance-duplicate-iam-user-arn-rejected codedeploy.register-onprem-instance-missing-iam-arn-rejected codedeploy.stop-deployment-already-completed-blocked codedeploy.stop-deployment-nonexistent-fails
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy|RegisterOnPremisesInstance|NotImplemented
Issue-Key: codedeploy.register-on-premises-instance-missing-iam-arn-rejected|run#4|AssertionError|assert error == (
Issue-Key: codedeploy.register-onprem-instance-missing-iam-arn-rejected|run#4|AssertionError|assert error == (
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Implement codedeploy RegisterOnPremisesInstance: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterOnPremisesInstance

## Description
`RegisterOnPremisesInstance` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/ops/register_on_premises_instance.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

32 recorded unit(s) fail on their `RegisterOnPremisesInstance` call, so each one unblocks only when `RegisterOnPremisesInstance` works: `codedeploy.batch-get-deployment-instances-matches-individual-gets`, `codedeploy.batch-get-deployment-targets-matches-individual-gets`, `codedeploy.batch-get-deployment-targets-reflects-instances`, `codedeploy.batch-get-deployments-matches-individual-gets`, `codedeploy.batch-get-on-premises-instances-matches-individual-gets`, `codedeploy.deployment-create-get-list-lifecycle`, `codedeploy.deployment-create-stop-status-transition`, `codedeploy.deployment-instance-lifecycle-transitions`, `codedeploy.deployment-target-lifecycle-transitions`, `codedeploy.deregister-invalid-instance-name-rejected`, `codedeploy.deregister-onprem-instance-invalid-name-rejected`, `codedeploy.e2e-application-deploymentgroup-deployment-stop-flow`, `codedeploy.e2e-on-premises-instance-tag-and-deploy`, `codedeploy.e2e-onprem-deployment-full-flow`, `codedeploy.e2e-onprem-instance-tag-filter-flow`, `codedeploy.get-deployment-target-not-started-rejected`, `codedeploy.on-premises-iam-user-arn-reuse-rejected`, `codedeploy.on-premises-instance-deregister-reflects-in-status-filters`, `codedeploy.on-premises-instance-register-deregister-lifecycle`, `codedeploy.on-premises-instance-tags-round-trip` (+12 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/ops/register_on_premises_instance.rs`. Replaying the corpus, the emulator refused the `RegisterOnPremisesInstance` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the RegisterOnPremisesInstance operation: operation RegisterOnPremisesInstance is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the codedeploy emulator
Call RegisterOnPremisesInstance and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
