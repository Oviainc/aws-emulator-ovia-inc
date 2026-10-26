Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm-quicksetup.get-configuration-manager-not-found
Issue-Shape-Id: com.amazonaws.ssmquicksetup#QuickSetup
Issue-Key: ssm-quicksetup.get-configuration-manager-not-found|run#13|AssertionError|assert message == f"Quick setup manager with arn: {live['ManagerArn']} could not be found.", (
Issue-Commit: fe685894da71d873af94e76be025d1114dc569e7

# GetConfigurationManager's ResourceNotFoundException message does not use AWS's not-found template

> _Note: this issue was identified against the code base at commit `fe685894da71d873af94e76be025d1114dc569e7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetConfigurationManager

## Description
1 cause.

Cause 1 of 1: GetConfigurationManager on a well-formed ManagerArn that names no (longer existing) configuration manager returns the right status and the right error class, but the exception's message is locally invented instead of AWS's not-found sentence.

AWS: GET /configurationManager/{ManagerArn} for an ARN that resolves to no manager fails with ResourceNotFoundException (Smithy: GetConfigurationManager binds ResourceNotFoundException, errorKind client, httpError 404; ResourceNotFoundException has a single member Message of target String; GetConfigurationManagerInput.ManagerArn is @httpLabel @required, length min 1, pattern ^arn:aws:ssm-quicksetup:([^:]+):(\d{12}):configuration-manager/<uuid>$). The model does not state the message text, so recorded behavior: the message is built from the request by interpolating the ManagerArn the caller supplied into the fixed template "Quick setup manager with arn: <ManagerArn> could not be found." The fixed text is "Quick setup manager with arn: " and the trailing " could not be found."; the only variable part is the caller-supplied ManagerArn, echoed verbatim. The template plus the request's ARN is the requirement — no ARN, account id or uuid from any single run is part of it.

Emulator: status and class are correct (404 / ResourceNotFoundException), but the message is a different sentence of the emulator's own wording: "no configuration manager with ARN <arn> exists" (observed: "no configuration manager with ARN arn:aws:ssm-quicksetup:us-west-2:000000000000:configuration-manager/198e29e9-b449-41e4-8f0a-000000000001 exists" — the ARN there is just this run's value filling the emulator's own template).

Why: a client or operator that surfaces, logs, or matches on the service's not-found text sees wording AWS never emits. Callers that branch on the message to distinguish a deleted manager from another 404-class refusal, and test harnesses asserting AWS's exact sentence, fail against the emulator even though the status and code are right. The positive control in the same check — the identical GetConfigurationManager call while the manager was still live — succeeds, so this is purely the not-found message rule, not a blanket failure of the operation.

Fix: the ResourceNotFoundException construction in the GetConfigurationManager handler of the ssm-quicksetup service crate, and the shared not-found helper it builds the message through, which must render the "Quick setup manager with arn: {ManagerArn} could not be found." template. NOTE: the open issue 20261022T034519Z-ssm-quicksetup-error-paths-wrong-rejection-class-unvalidated.md records the SAME wrong sentence on ListConfigurations (its Cause 7) and attributes it to that handler plus "the not-found helper it shares"; it does not carry this check's replay key and names a different operation, so this GetConfigurationManager path is filed separately — if the shared helper is the single root cause, fixing it should close both, but this check must be re-replayed to confirm. That issue's Cause 1 additionally claims a wire recording where an unknown ManagerArn is refused 401 UnauthorizedException at the front door; this behavioral check instead exercises an ARN whose manager existed for this caller and was then deleted, and requires 404 ResourceNotFoundException with the template above, so both paths must coexist.

Traces: ssm-quicksetup.get-configuration-manager-not-found.

## Evidence
Check ssm-quicksetup.get-configuration-manager-not-found [mismatch]:
  AssertionError: expected the exact ResourceNotFoundException message AWS returns, got 'no configuration manager with ARN arn:aws:ssm-quicksetup:us-west-2:000000000000:configuration-manager/198e29e9-b449-41e4-8f0a-000000000001 exists'
  at check_ssm_quicksetup_get_configuration_manager_not_found.py:54 —
    assert message == f"Quick setup manager with arn: {live['ManagerArn']} could not be found."

Expected (AWS): HTTP 404, Error.Code = ResourceNotFoundException, Error.Message = "Quick setup manager with arn: {ManagerArn} could not be found." where {ManagerArn} is the ARN the request supplied (the same ARN the preceding successful GetConfigurationManager returned).
Actual (emulator): HTTP 404, Error.Code = ResourceNotFoundException (both correct), Error.Message = "no configuration manager with ARN {arn} exists".
Positive control in the same check: GetConfigurationManager on that identical ManagerArn while the manager was still live returned 200 with ManagerArn echoed, so only the not-found message diverges.

## Steps to Reproduce
1. Boot the emulator from this checkout and run the behavioral suite (suite simparity-behavioral-ga-prod-v1), check ssm-quicksetup.get-configuration-manager-not-found.
2. Create the two local deployment roles (an administration role trusted by cloudformation.amazonaws.com / ssm-quicksetup.amazonaws.com with AdministratorAccess, and an execution role trusting that admin role).
3. Resolve the latest version of type AWSQuickSetupType-SSMHostMgmt via ListQuickSetupTypes, then CreateConfigurationManager with one ConfigurationDefinitions entry of that type (TargetType=Tags, TargetAccounts=<account>, TargetRegions=<region>, plus the local role arn/name). Keep the returned ManagerArn.
4. Positive control: GetConfigurationManager(ManagerArn=<arn>) — succeeds and echoes the ARN.
5. DeleteConfigurationManager(ManagerArn=<arn>) and poll GetConfigurationManager(ManagerArn=<arn>) until it raises ResourceNotFoundException.
6. Inspect that error's Error.Message. Observed: "no configuration manager with ARN <arn> exists". Required: "Quick setup manager with arn: <arn> could not be found." with <arn> the ARN passed in the request.
