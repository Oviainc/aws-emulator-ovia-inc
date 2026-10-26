Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.update-domain-name-invalid-security-policy-bad-request
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.update-domain-name-invalid-security-policy-bad-request|run#10|AssertionError|assert err.get("Message") == "Invalid security policy specified", (
Issue-Commit: cb77b4aa1b924c22c31c4299978c59e199331760

# UpdateDomainName refuses an out-of-enum /securityPolicy patch with a generic enum-list message instead of AWS's "Invalid security policy specified"

> _Note: this issue was identified against the code base at commit `cb77b4aa1b924c22c31c4299978c59e199331760`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDomainName

## Description
One net-new root cause in the apigateway (REST v1) service. A fixer must close this one cause before submitting.

Context on what is NOT this issue: open issue 20261025T174338Z-apigateway-api-gateway-custom-domain-surface-cloudformation-provisioned-domainname.md lists this check id, but under its Cause 3, whose replay key is the assertion `err.get("Code") == "BadRequestException"` (UpdateDomainName answering 501 NotImplemented). That part now behaves: the emulator refuses the patch with HTTP 400 / BadRequestException. The check now fails one assertion later, on the refusal MESSAGE, and no open issue carries that key. Open issue 20261025T161227Z-apigateway-updateapikey-s-invalid-patch-path-refusal-carries-an.md is the same family (a refusal message built wrongly by patch validation) but for UpdateApiKey's invalid patch PATH, not for an out-of-enum /securityPolicy VALUE on UpdateDomainName, and does not carry this key.

Cause 1 of 1: UpdateDomainName rejects a /securityPolicy patch whose value is not a member of the SecurityPolicy enum with the right status and the right error code, but with the emulator's generic "member takes one of <enum values>" message rather than the condition-specific message AWS emits for an invalid security policy.
AWS: Smithy (com.amazonaws.apigateway#UpdateDomainName): PATCH /domainnames/{domainName}, input `domainName` (@httpLabel, required) + `patchOperations` (ListOfPatchOperation), output DomainName whose `securityPolicy` member targets the enum com.amazonaws.apigateway#SecurityPolicy (values TLS_1_0, TLS_1_2, SecurityPolicy_TLS12_2018_EDGE, SecurityPolicy_TLS12_PFS_2025_EDGE, SecurityPolicy_TLS13_1_2_2021_06, SecurityPolicy_TLS13_1_2_Ext2_FIPS_PQ_2025_09, SecurityPolicy_TLS13_1_2_Ext2_PQ_2025_09, SecurityPolicy_TLS13_1_2_FIPS_PFS_PQ_2025_09, SecurityPolicy_TLS13_1_2_FIPS_PQ_2025_09, SecurityPolicy_TLS13_1_2_PFS_PQ_2025_09, SecurityPolicy_TLS13_1_2_PQ_2025_09, SecurityPolicy_TLS13_1_3_2025_09, SecurityPolicy_TLS13_1_3_FIPS_2025_09, SecurityPolicy_TLS13_2025_EDGE); modeled errors BadRequestException(400, "The submitted request is not valid..."), ConflictException(409), NotFoundException(404), LimitExceededException(429), TooManyRequestsException(429), UnauthorizedException(401). Recorded behavior for this error class: a patch {op: replace, path: /securityPolicy, value: <not a SecurityPolicy member>} is refused with HTTP 400, Error.Code=BadRequestException and Error.Message="Invalid security policy specified" — the message is ENTIRELY fixed text: it has no variable parts, it does not echo the rejected value, the patch path, the op, the domain name, nor the list of legal policies. The domain's stored securityPolicy is left unchanged, so a following GetDomainName still reports the value the domain was created with.
Emulator: HTTP 400 and Error.Code=BadRequestException are correct, but Error.Message is the generic enum-rejection template the shared patch helper builds: "/securityPolicy takes one of SecurityPolicy_TLS12_2018_EDGE, SecurityPolicy_TLS12_PFS_2025_EDGE, SecurityPolicy_TLS13_1_2_2021_06, SecurityPolicy_TLS13_1_2_Ext2_FIPS_PQ_2025_09, SecurityPolicy_TLS13_1_2_Ext2_PQ_2025_09, SecurityPolicy_TLS13_1_2_FIPS_PFS_PQ_2025_09, SecurityPolicy_TLS13_1_2_FIPS_PQ_2025_09, SecurityPolicy_TLS13_1_2_PFS_PQ_2025_09, SecurityPolicy_TLS13_1_2_PQ_2025_09, SecurityPolicy_TLS13_1_3_2025_09, SecurityPolicy_TLS13_1_3_FIPS_2025_09, SecurityPolicy_TLS13_2025_EDGE, TLS_1_0, TLS_1_2, got \"<value>\"" — i.e. it interpolates the patch path, the whole sorted enum value set and the rejected value, none of which AWS's message contains.
Why: the message is the one part of this refusal a client, CLI user or IaC tool reads or asserts on. A caller matching AWS's documented/observed text to classify "unsupported TLS policy" against other BadRequestException causes on the same operation (invalid patch path, immutable member, bad certificate) cannot classify it here, because the emulator's text belongs to a different, emulator-internal template; log scrapers and test suites that pin AWS's wording diverge, and the divergence is silent since the status and code match.
Fix: the UpdateDomainName handler's patch validation for the `/securityPolicy` member — the securityPolicy branch must raise the condition-specific BadRequestException message rather than delegating to the generic enum helper shared by other members (it currently calls the shared `patch_enum_string` helper that formats "<path> takes one of <values>, got <value>"). Scope the change to this member so other members' messages are untouched, and keep the existing (correct) 400/BadRequestException classification and the no-mutation-on-refusal behavior. File and line citations are under Traces/Evidence.
Traces: apigateway.update-domain-name-invalid-security-policy-bad-request.

## Evidence
Cause 1, check apigateway.update-domain-name-invalid-security-policy-bad-request (mismatch), from the behavioral replay of suite simparity-behavioral-ga-prod-v1:

  File "check_apigateway_update_domain_name_invalid_security_policy_bad_request.py", line 116, in run
    assert err.get("Message") == "Invalid security policy specified", (
  AssertionError: expected the exact invalid-security-policy message, got '/securityPolicy takes one of SecurityPolicy_TLS12_2018_EDGE, SecurityPolicy_TLS12_PFS_2025_EDGE, SecurityPolicy_TLS13_1_2_2021_06, SecurityPolicy_TLS13_1_2_Ext2_FIPS_PQ_2025_09, SecurityPolicy_TLS13_1_2_Ext2_PQ_2025_09, SecurityPolicy_TLS13_1_2_FIPS_PFS_PQ_2025_09, SecurityPolicy_TLS13_1_2_FIPS_PQ_2025_09, SecurityPolicy_TLS13_1_2_PFS_PQ_2025_09, SecurityPolicy_TLS13_1_2_PQ_2025_09, SecurityPolicy_TLS13_1_3_2025_09, SecurityPolicy_TLS13_1_3_FIPS_2025_09, SecurityPolicy_TLS13_2025_EDGE, TLS_1_0, TLS_1_2, got "NOT_A_REAL_POLICY"'

The preceding assertion in the same check, err.get("Code") == "BadRequestException", PASSED — so the HTTP status (400) and the error code are correct and only the message text diverges. The later assertion that GetDomainName still reports securityPolicy == "TLS_1_2" was never reached, so no claim is made here about state preservation.

Diverging requirement, stated as a template: status 400; code BadRequestException; message "Invalid security policy specified" — all fixed text, zero variable parts. Emulator message template: "<patch path> takes one of <comma-joined SecurityPolicy values>, got \"<rejected value>\"", where the patch path and rejected value come from the request and the value list is the emulator's enum table. (The value NOT_A_REAL_POLICY above is this run's request value filling that template, not part of the requirement.)

Code pointers in the checkout: services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/update_domain_name.rs lines 219-223 route the "securityPolicy" patch path to the shared helper `patch_enum_string(&mut entry.security_policy, patch, SECURITY_POLICY_VALUES)`; that helper is imported from crate::state (services/service-com-amazonaws-apigateway-backplanecontrolservice/src/state.rs, line 148 of the import list) and is the source of the "<path> takes one of ..., got ..." wording, which the same file also builds inline for endpointConfiguration types at lines 337-341.

## Steps to Reproduce
1. Boot the emulator and point botocore at it (region us-west-2).
2. Generate a self-signed certificate whose CN is a unique test domain <domain> and acm.ImportCertificate it -> note <cert-arn>.
3. apigw.create_domain_name(domainName=<domain>, regionalCertificateArn=<cert-arn>, endpointConfiguration={'types': ['REGIONAL']}, securityPolicy='TLS_1_2') -> succeeds and echoes securityPolicy='TLS_1_2'.
4. Poll apigw.get_domain_name(domainName=<domain>) until domainNameStatus == 'AVAILABLE'.
5. apigw.update_domain_name(domainName=<domain>, patchOperations=[{'op': 'replace', 'path': '/securityPolicy', 'value': 'NOT_A_REAL_POLICY'}]) -> observe HTTP 400 Error.Code='BadRequestException' (correct) with Error.Message='/securityPolicy takes one of SecurityPolicy_TLS12_2018_EDGE, ... TLS_1_2, got "NOT_A_REAL_POLICY"' instead of the required exact message 'Invalid security policy specified'. Any non-enum value reproduces it; the rejected value is interpolated into the emulator's message but must not appear in AWS's.
6. apigw.get_domain_name(domainName=<domain>) -> securityPolicy must still be 'TLS_1_2' (the refused patch must mutate nothing).
7. Re-replay apigateway.update-domain-name-invalid-security-policy-bad-request to confirm the fix.
