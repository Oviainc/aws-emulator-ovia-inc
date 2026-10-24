Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: aeaddd52642475574d853bf8e089296dd7f2f86c

# GetPolicyVersion's ResourceNotFoundException invents prose containing the requested ARN instead of MPA's fixed not-found message

_Filed: 2026-10-24T00:24:17Z_
_Operation: GetPolicyVersion | Class: error-response_

> _Note: this issue was identified against the code base at commit `aeaddd52642475574d853bf8e089296dd7f2f86c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: MPA answers every ARN-addressed resource lookup that resolves to nothing with ONE service-wide message — 404 ResourceNotFoundException whose Message is exactly "The requested resource does not exist." — independent of the operation and of the ARN supplied, so the refusal names nothing the request carried. This service already encodes that rule in one place (crate::state::RESOURCE_NOT_FOUND_MESSAGE / crate::state::resource_not_found(), documented in src/state.rs as "the one message every ARN-addressed resource lookup in this service answers with"), and GetApprovalTeam, UpdateApprovalTeam, StartActiveApprovalTeamDeletion, ListPolicyVersions, ListResourcePolicies and GetResourcePolicy all raise it through that constructor.

Actual: GetPolicyVersion is the one handler that formats its own sentence and leaks the requested ARN into it: src/ops/get_policy_version.rs builds ResourceNotFoundException { message: format!("No policy version found for ARN '{}'.", input.policy_version_arn) } (lines ~190-200). Status (404) and error code are right; the Message differs per request, which is exactly the divergence class already corrected for the other operations.

## Evidence

Spec/recorded rule: ResourceNotFoundException is a client error with @httpError(404) and a single @required Message (@length max 1000); the model fixes no text, and the recorded AWS behaviour (captured in src/state.rs's RESOURCE_NOT_FOUND_MESSAGE documentation and in the fixed issue 20261023T180508Z-mpa-error-message-fidelity-across-the-not-found, Cause 1, where four ARN-addressed operations agree on "The requested resource does not exist.") makes it a service constant. Handler: services/service-com-amazonaws-mpa-awsfluffycoreservice/src/ops/get_policy_version.rs formats per-request prose instead of calling crate::state::resource_not_found(). Not a duplicate: GetPolicyVersion was still a 501 scaffold when that message-fidelity issue was filed (see 20261023T100857Z-mpa-getpolicyversion-stub), so its not-found path is new code. Test coverage: tests/all/rejection_wording.rs asserts the constant only for GetApprovalTeam, UpdateApprovalTeam, StartActiveApprovalTeamDeletion, ListPolicyVersions and GetResourcePolicy (the Lookup enum excludes GetPolicyVersion), and tests/all/policy_version_detail.rs asserts only the 404 status and the error code, never the message.

## Steps to Reproduce

1. Boot the emulator and point an MPA client at it.
2. Call GetPolicyVersion with a well-formed QualifiedPolicyArn the catalog holds no version for, e.g. GET /policy-versions/arn%3Aaws%3Ampa%3A%3A%3Aaws%3Apolicy%2FIdentityCenter%2Fdoes-not-exist%2F1.
3. Observe 404 ResourceNotFoundException with Message "No policy version found for ARN 'arn:aws:mpa:::aws:policy/IdentityCenter/does-not-exist/1'." Expected Message: "The requested resource does not exist."
4. Repeat with a different nonexistent ARN: on AWS the body is byte-identical, on the emulator the message changes with the ARN.
5. Contrast with GetApprovalTeam on a nonexistent team ARN, which already answers the constant message.
