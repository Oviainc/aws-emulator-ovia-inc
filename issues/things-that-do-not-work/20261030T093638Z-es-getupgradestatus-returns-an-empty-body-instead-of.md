Issue-Source: trace-sweep-divergence
Issue-Trace: opensearch-domain-version-capacity-and-software-update-management
Issue-Step: task#7
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: opensearch-domain-version-capacity-and-software-update-management
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|GetUpgradeStatus|2xx->2xx|$
Issue-Commit: e0e11b7672600d6645a012dfa38238750280479b

# es GetUpgradeStatus returns an empty body instead of the GetUpgradeStatusResponse envelope

> _Note: this issue was identified against the code base at commit `e0e11b7672600d6645a012dfa38238750280479b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetUpgradeStatus

## Description
1 net-new root cause on the es service token: the GetUpgradeStatus reader answers 200 with no response members at all, instead of serializing the modeled GetUpgradeStatusResponse envelope.

Cause 1 of 1: GetUpgradeStatus omits every member of GetUpgradeStatusResponse.
AWS: Smithy GetUpgradeStatus = GET /2021-01-01/opensearch/upgradeDomain/{DomainName}/status, DomainName an @httpLabel required path member (@length 3..28, @pattern ^[a-z][a-z0-9\-]+$); output shape GetUpgradeStatusResponse with members UpgradeStep (enum UpgradeStep: PRE_UPGRADE_CHECK | SNAPSHOT | UPGRADE), StepStatus (enum UpgradeStatus: IN_PROGRESS | SUCCEEDED | SUCCEEDED_WITH_ISSUES | FAILED) and UpgradeName (string); modeled errors BaseException(400), DisabledOperationException(409), InternalException(500), ResourceNotFoundException(409), ValidationException(400). The recording shows that for an existing domain that has no upgrade or upgrade-eligibility check on record AWS still emits the full envelope, with all three members present and carrying JSON null (UpgradeStep, StepStatus and UpgradeName all null); when an upgrade or pre-upgrade check has run, those same three members carry the step, its status and the upgrade's name. Recorded behavior only for the explicit-null form - the model constrains the members, not whether they are emitted when unset.
Emulator: the 200 response body carries no members whatsoever - it serializes as {} (reported by the replay as "$: empty_body"), so UpgradeStep, StepStatus and UpgradeName are all absent rather than present-and-null.
Why: a client polling an in-place engine upgrade reads these three members to decide where the upgrade is (which step, whether that step is IN_PROGRESS, SUCCEEDED, SUCCEEDED_WITH_ISSUES or FAILED, and which named upgrade it is reporting on). Against the emulator the envelope is structurally empty, so a client that distinguishes "member reported as null / no upgrade on record" from "member not reported at all" - and any client that validates the response against the modeled output shape before reading it - sees a response AWS never sends, and the step/status/name channel the operation exists to expose is never populated at all.
Fix: the GetUpgradeStatus handler and its output serializer in the es/opensearch service crate - it must build a GetUpgradeStatusResponse from the domain's upgrade history (or the explicit-null envelope when there is none) rather than returning an empty body.
Traces: opensearch-domain-version-capacity-and-software-update-management#task#7.

Prior art: open issue 20261030T090206Z-es-reference-catalogs-diverge-from-aws-upgrade-matrix.md covers the es upgrade-compatibility/instance-limit/reserved-instance reference CATALOGS and carries replay keys for es|GetCompatibleVersions, es|DescribeInstanceTypeLimits and es|DescribeReservedInstanceOfferings only; open issue 20261030T073207Z-opensearch-3-net-new-root-causes-a-verbose.md and 20261030T090206Z-es-describedryrunprogress-unknown-dryrunid-refusal-diverges-http-409.md cover the dry-run surface. None of them carries the es|GetUpgradeStatus replay key, so this cluster is neither exercised nor closed by any of them.

## Evidence
Cause 1 (fixture opensearch-domain-version-capacity-and-software-update-management, step task#7, divergence_key service=es|GetUpgradeStatus|status=200->200|keys=$, verdict mismatch, affected_count 1):
- Expected status 200, actual status 200 - the status matches; the body does not.
- Replay detail: "$: empty_body (expected {'StepStatus': None, 'UpgradeName': None, 'UpgradeStep': None}, got {})".
- Expected body: the GetUpgradeStatusResponse envelope with all three modeled members present - StepStatus, UpgradeName and UpgradeStep - each carrying JSON null because the domain under test has no upgrade on record. The member NAMES and their presence are the requirement; the null values are what this particular domain state yields, and on a domain with upgrade history the same three members carry the UpgradeStep/UpgradeStatus enum values and the upgrade name.
- Actual body: {} - no members emitted.
- No error code or message is involved: both sides answer HTTP 200 with no error envelope.

## Steps to Reproduce
1. Replay fixture opensearch-domain-version-capacity-and-software-update-management against the emulator.
2. Let it run through to step task#7, the GET /2021-01-01/opensearch/upgradeDomain/{DomainName}/status call for the domain the fixture created earlier in the same run.
3. Diff the 200 response body against the recording: the recording carries the GetUpgradeStatusResponse envelope with UpgradeStep, StepStatus and UpgradeName all present (null for this domain, which has no upgrade on record), while the emulator returns {} with none of the three members emitted.
