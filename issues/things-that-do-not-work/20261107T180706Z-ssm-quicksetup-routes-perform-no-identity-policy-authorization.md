Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm-quicksetup.access-denied-without-permission-rejected
Issue-Shape-Id: com.amazonaws.ssmquicksetup#QuickSetup
Issue-Key: ssm-quicksetup.access-denied-without-permission-rejected|run#31|AssertionError|assert err.get("Code") == "AccessDeniedException", f"expected AccessDeniedException, got {err!r}"
Issue-Commit: 5d2b65c7bbfaadbf1aa4811c4280349eeebcd167

# ssm-quicksetup routes perform no identity-policy authorization: an explicitly denied GetConfigurationManager answers 200 instead of 403 AccessDeniedException

> _Note: this issue was identified against the code base at commit `5d2b65c7bbfaadbf1aa4811c4280349eeebcd167`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetConfigurationManager

## Description
1 cause; it must be closed before this issue is submitted. NOTE: open issue 20261102T032525Z-ssm-quicksetup-routes-enforce-no-identity-policy-authorization.md and cause 4 of open issue 20261029T184830Z-ssm-quicksetup-4-net-new-root-causes-anchored.md describe this same authorization gap, but the replay report marks this check as neither ALREADY FILED nor OWNED by either of them (their replay keys were recorded against a harness-timeout verdict; the check now fails as a [mismatch] with a concrete AssertionError, so neither key binds it any more). It is recorded here with its current replay key. Fixing the authorization stage once closes all three.

Cause 1 of 1: the ssm-quicksetup control plane never authorizes the calling principal, so an identity-based policy that explicitly denies ssm-quicksetup:GetConfigurationManager is not enforced and the denied caller's read succeeds with the full manager body.
Failing check ids for this cause: ssm-quicksetup.access-denied-without-permission-rejected.

AWS: a caller whose in-effect identity policy carries an explicit Deny for the action is refused with HTTP 403 and error code AccessDeniedException. Smithy (services/service-com-amazonaws-ssmquicksetup-quicksetup/smithy-model/com-amazonaws-ssmquicksetup-quicksetup.json): GetConfigurationManager binds AccessDeniedException with errorKind client and httpError 403 alongside ConflictException/ResourceNotFoundException/ThrottlingException/ValidationException; its http binding is GET /configurationManager/{ManagerArn} with the required httpLabel member ManagerArn; ListConfigurationManagers (POST /listConfigurationManagers) binds the same AccessDeniedException. The message template is recorded behavior only: fixed text "User: " <caller principal ARN> " is not authorized to perform: ssm-quicksetup:GetConfigurationManager on resource: " <ManagerArn> " with an explicit deny in an identity-based policy". Its variable parts are the assumed-role principal ARN derived from the signing credentials of the request (arn:aws:sts::<account-id>:assumed-role/<role-name>/<session-name>) and the ManagerArn taken from the request path; the action name is fixed per operation. The status and error shape are modeled; the sentence is from the recording. Authorization is scoped to the one denied action: the SAME principal, holding only ssm-quicksetup:ListConfigurationManagers, must keep getting 200 from ListConfigurationManagers and must still see that manager listed there, and the original fully permissioned caller must keep reading it with 200.

Emulator: there is no authorization step in front of the ssm-quicksetup routes at all. GET /configurationManager/{ManagerArn} issued with the explicitly denied credentials answers HTTP 200 with the complete manager body; no error is produced, so the check's bounded poll for the refusal observes an empty error dict for its whole window and asserts out with "AssertionError: expected AccessDeniedException, got {}" (check_ssm_quicksetup_access_denied_without_permission_rejected.py line 133). Nothing in the read path inspects the caller identity and AccessDeniedException is never constructed for this operation.

Why: a client relying on AWS's behavior cannot distinguish "not permitted" from "permitted". A principal that real AWS refuses can read — and by the same absent check mutate — configuration managers it has been explicitly denied, so every authorization-negative path passes silently against the emulator, security regressions that AWS surfaces as 403 are undetectable, and client code that branches on AccessDeniedException (retry/escalation/UI permission messaging) is never exercised.

Fix: a request-authorization stage fronting the ssm-quicksetup routes — the service crate's request walk, evaluated before each handler's resource lookup in services/service-com-amazonaws-ssmquicksetup-quicksetup/src/ops/*.rs (get_configuration_manager.rs for this operation). It must evaluate the caller's in-effect identity policy for the ssm-quicksetup action of the operation being served, refuse an explicit Deny with 403 AccessDeniedException carrying AWS's explicit-deny sentence built from the request's signing principal and the path's ManagerArn, and leave actions the policy allows (ListConfigurationManagers here) answering normally.

Traces: ssm-quicksetup.access-denied-without-permission-rejected.

## Evidence
Check ssm-quicksetup.access-denied-without-permission-rejected [mismatch], suite simparity-behavioral-aws-v1 (per-service line: ssm-quicksetup 10/21 passed, 11 failed).

Scenario: the fully permissioned caller creates an AWSQuickSetupType-SSMHostMgmt configuration manager; iam:CreateRole creates a role trusted by the account root; iam:PutRolePolicy attaches an inline policy that Allows sts:GetCallerIdentity and ssm-quicksetup:ListConfigurationManagers and Denies ssm-quicksetup:GetConfigurationManager; sts:AssumeRole is called into that role; GetConfigurationManager(ManagerArn=<created manager>) is issued with the returned temporary credentials, polled to a bounded deadline.

Expected (AWS): ClientError, HTTP 403, Error.Code "AccessDeniedException", Error.Message "User: arn:aws:sts::<account-id>:assumed-role/<role-name>/<session-name> is not authorized to perform: ssm-quicksetup:GetConfigurationManager on resource: <ManagerArn> with an explicit deny in an identity-based policy". Fixed text: "User: ", " is not authorized to perform: ssm-quicksetup:GetConfigurationManager on resource: ", " with an explicit deny in an identity-based policy". Variable parts: the assumed-role principal ARN from the signing credentials and the ManagerArn from the request path — the per-run account id, role name, session name and manager uuid are fillers for that template, not requirements. ListConfigurationManagers with the same credentials keeps answering 200 and still reports that manager across pages; the fully permissioned caller's GetConfigurationManager still answers 200.

Actual (emulator): the denied principal's GET /configurationManager/{ManagerArn} answers HTTP 200 with the manager body on every attempt for the full 60s poll window. No error code and no message are emitted — the harness records the error dict as {} — so the assertion fails: "AssertionError: expected AccessDeniedException, got {}" at check_ssm_quicksetup_access_denied_without_permission_rejected.py line 133. The divergence is in the status (200 vs 403), the error code (none vs AccessDeniedException) and the error message (none vs the explicit-deny sentence).

Model citation: GetConfigurationManager errors include {shape: AccessDeniedException, errorKind: client, httpError: 403}; http {method: GET, uri: /configurationManager/{ManagerArn}, code: 200}. ListConfigurationManagers also binds AccessDeniedException (403).

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite (simparity-behavioral-aws-v1); check ssm-quicksetup.access-denied-without-permission-rejected fails [mismatch] with "expected AccessDeniedException, got {}".
2. Manually: with fully permissioned credentials, POST /configurationManager with one AWSQuickSetupType-SSMHostMgmt definition (TargetType=Tags, unique TargetTagKey/TargetTagValue, TargetAccounts=<account-id>, TargetRegions=<region>, plus LocalDeploymentAdministrationRoleArn and LocalDeploymentExecutionRoleName naming roles created via iam:CreateRole). Keep the returned ManagerArn.
3. iam:CreateRole a role whose trust policy allows sts:AssumeRole from arn:aws:iam::<account-id>:root; iam:PutRolePolicy an inline policy with Allow sts:GetCallerIdentity, Allow ssm-quicksetup:ListConfigurationManagers, Deny ssm-quicksetup:GetConfigurationManager.
4. sts:AssumeRole into that role and build an ssm-quicksetup client from the returned temporary credentials.
5. Call GET /configurationManager/{ManagerArn} with those credentials. Observed: HTTP 200 and the manager body. Expected: HTTP 403 with Code AccessDeniedException and the explicit-deny sentence.
6. Call POST /listConfigurationManagers with the same credentials and confirm it answers 200 and lists that manager — it must keep succeeding after the fix, proving the denial is scoped to the one action.
7. Call GET /configurationManager/{ManagerArn} with the original fully permissioned credentials and confirm it still answers 200 (positive control).
