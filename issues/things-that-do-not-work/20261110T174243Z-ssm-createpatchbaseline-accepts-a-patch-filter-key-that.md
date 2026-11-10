Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.patch-baseline-amazon-linux-2023-echo-and-invalid-filter-key-rejected
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm.patch-baseline-amazon-linux-2023-echo-and-invalid-filter-key-rejected|_error#16|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Commit: ef9cd1f472238b59a7ccf5edc608de1fa0253c97

# ssm: CreatePatchBaseline accepts a patch filter key that is not valid for the baseline's OperatingSystem (MSRC_SEVERITY on AMAZON_LINUX_2023) instead of refusing with ValidationException "Unknown Filter Key: <KEY>"

> _Note: this issue was identified against the code base at commit `ef9cd1f472238b59a7ccf5edc608de1fa0253c97`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePatchBaseline

## Description
Cause 1 of 1: CreatePatchBaseline does not validate PatchFilter keys against the baseline's OperatingSystem. A Windows-only key supplied on a Linux baseline is accepted and a baseline is created. Failing check id: ssm.patch-baseline-amazon-linux-2023-echo-and-invalid-filter-key-rejected.

AWS: the set of valid PatchFilter keys is per-operating-system. Smithy: PatchFilter.Key targets the PatchFilterKey enum (ARCH, ADVISORY_ID, BUGZILLA_ID, PATCH_SET, PRODUCT, PRODUCT_FAMILY, CLASSIFICATION, CVE_ID, EPOCH, MSRC_SEVERITY, NAME, PATCH_ID, SECTION, PRIORITY, REPOSITORY, RELEASE, SEVERITY, SECURITY, VERSION) and its documentation states "Run the DescribePatchProperties command to view lists of valid keys for each operating system type" — membership in the enum is not sufficient, the key must be valid for the request's OperatingSystem. MSRC_SEVERITY is a Windows key and is not valid for AMAZON_LINUX_2023. CreatePatchBaseline's modelled errors are IdempotentParameterMismatch, InternalServerError and ResourceLimitExceededException, so the refusal class is recorded behavior only: the recording shows HTTP 400 with error code ValidationException and message "Unknown Filter Key: MSRC_SEVERITY". That message is the fixed text "Unknown Filter Key: " followed by the offending key taken verbatim from the request's ApprovalRules.PatchRules[].PatchFilterGroup.PatchFilters[].Key — never a hard-coded key. A refused CreatePatchBaseline must also leave no baseline behind (DescribePatchBaselines with a NAME_PREFIX filter on the requested name returns no BaselineIdentities).

Emulator: the same CreatePatchBaseline call returns HTTP 200 with a freshly minted BaselineId (e.g. 'pb-<17 hex>'), creating a baseline whose approval rule filters on a key the OS does not support. No validation of the filter key against OperatingSystem is performed.

Why: a client relying on AWS to reject an OS-invalid patch filter gets a silently useless baseline — the rule can never match a patch, so approval/compliance results computed from it are wrong, and the mistake surfaces much later as "no patches approved" rather than at the create call. Code that handles ValidationException to correct the key never sees the refusal, and cleanup/listing paths now have a baseline AWS would never have created.

Fix: CreatePatchBaseline's input validation (the operation's handler validation for ssm), applied to GlobalFilters.PatchFilters and every ApprovalRules.PatchRules[].PatchFilterGroup.PatchFilters entry: reject keys outside the set valid for the request's OperatingSystem with ValidationException and the message above, before any baseline is minted or stored. The same per-OS key table should back DescribePatchProperties and UpdatePatchBaseline.

Traces: ssm.patch-baseline-amazon-linux-2023-echo-and-invalid-filter-key-rejected (the GetPatchBaseline echo earlier in the same check passed; the divergence is at the second, negative CreatePatchBaseline).

## Evidence
Request: ssm.create_patch_baseline(Name=<unique name>, OperatingSystem="AMAZON_LINUX_2023", ApprovalRules={"PatchRules":[{"PatchFilterGroup":{"PatchFilters":[{"Key":"MSRC_SEVERITY","Values":["Critical"]}]},"ApproveAfterDays":7,"ComplianceLevel":"CRITICAL"}]}, Tags=[...]).

Expected (real AWS): refusal with HTTP 400, Error.Code = "ValidationException", Error.Message = "Unknown Filter Key: MSRC_SEVERITY" (fixed prefix "Unknown Filter Key: " + the Key value from the request), and no baseline created — ssm.describe_patch_baselines(Filters=[{"Key":"NAME_PREFIX","Values":[<name>]}])["BaselineIdentities"] == [].

Actual (emulator): HTTP 200, {'BaselineId': 'pb-8bf98658d9d4b1ec1', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}} — the baseline id value is per-run; the divergence is the 200-with-baseline where AWS answers 400 ValidationException. Harness assertion: "expected the call to be refused, but it succeeded".

## Steps to Reproduce
1. ssm.create_patch_baseline(Name="sim2real-behav-pbal2023-<unique>", OperatingSystem="AMAZON_LINUX_2023", Description="security patches after seven days", ApprovalRules with PatchFilters [{CLASSIFICATION:[Security]},{SEVERITY:[Critical,Important]}], ApprovedPatches=["openssl"], ApprovedPatchesComplianceLevel="HIGH") — succeeds, and GetPatchBaseline echoes it with AWS's defaults (this part already matches).
2. ssm.create_patch_baseline(Name="sim2real-behav-pbbad-<unique>", OperatingSystem="AMAZON_LINUX_2023", ApprovalRules={"PatchRules":[{"PatchFilterGroup":{"PatchFilters":[{"Key":"MSRC_SEVERITY","Values":["Critical"]}]},"ApproveAfterDays":7,"ComplianceLevel":"CRITICAL"}]}).
Expected: 400 ValidationException "Unknown Filter Key: MSRC_SEVERITY". Observed: 200 with a BaselineId.
3. ssm.describe_patch_baselines(Filters=[{"Key":"NAME_PREFIX","Values":["sim2real-behav-pbbad-<unique>"]}]) must return no BaselineIdentities.
