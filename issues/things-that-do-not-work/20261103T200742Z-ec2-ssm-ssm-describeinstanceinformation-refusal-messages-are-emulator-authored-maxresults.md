Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ssm.w22n-instance-information-max-results-above-fifty-rejected ec2__ssm.w22n-platform-name-filter-key-rejected-so-the-2022-conjunct-is-client-side ec2__ssm.w22n-legacy-and-current-filter-dialects-together-rejected ec2__ssm.w22n-platform-types-filter-value-outside-the-enum-rejected
Issue-Key: ec2__ssm.w22n-instance-information-max-results-above-fifty-rejected|run#4|AssertionError|assert err == (
Issue-Key: ec2__ssm.w22n-platform-name-filter-key-rejected-so-the-2022-conjunct-is-client-side|run#11|AssertionError|assert msg == maxresults_msg, (
Issue-Key: ec2__ssm.w22n-legacy-and-current-filter-dialects-together-rejected|run#12|AssertionError|assert msg == expected, f"expected {expected!r}, got {msg!r}"
Issue-Key: ec2__ssm.w22n-platform-types-filter-value-outside-the-enum-rejected|run#8|AssertionError|assert got == want, (
Issue-Commit: 74acd3b5911f8cc01604f18ec0c28de145582648

# SSM DescribeInstanceInformation refusal messages are emulator-authored: maxResults echoes the rejected value, and the dialect-conflict and PlatformTypes messages are rewritten (3 causes)

> _Note: this issue was identified against the code base at commit `74acd3b5911f8cc01604f18ec0c28de145582648`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceInformation

## Description
DescribeInstanceInformation refuses the three malformed request classes these checks exercise with the right status and (in two of three) the right error code, but every message is the emulator's own prose rather than AWS's. An HTTP status, an error code and the error message are all requirements, so each of these is a defect in its own right. 3 CAUSES.

Cause 1 of 3 - the maxResults range refusal echoes the rejected value.
AWS: 400 ValidationException whose message is the fixed sentence "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 50". The only variable parts AWS fills are the member path (maxResults) and the bound from the model; the rejected value is NOT echoed for this member. Smithy: DescribeInstanceInformation.MaxResults targets MaxResultsEC2Compatible with @range min 5 max 50 (the operation's modeled errors are InternalServerError, InvalidFilterKey, InvalidInstanceId, InvalidInstanceInformationFilterValue, InvalidNextToken; ValidationException is the service-common request-validation error - recorded behavior only).
Emulator: 400 ValidationException with "1 validation error detected: Value '51' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 50" - an extra "Value '<rejected value>'" inserted from the request.
Why: a client that matches, parses or asserts on AWS's validation sentence never sees it, and anything that logs the message now leaks the rejected value AWS deliberately omits. The same wrong template also defeats the precedence contract: a call that is wrong both ways (unknown filter key plus MaxResults=51) must name only maxResults in AWS's words, so a caller cannot fix the page size first and then discover the filter-key problem.
Fix: the DescribeInstanceInformation input-validation message template for range constraints (the shared SSM request-validation formatter), not the handler's control flow - the status and code are already right.
Traces: ec2__ssm.w22n-instance-information-max-results-above-fifty-rejected, ec2__ssm.w22n-platform-name-filter-key-rejected-so-the-2022-conjunct-is-client-side (the latter's PlatformName filter-key refusal matched; it fails only on this maxResults message).

Cause 2 of 3 - mixing the legacy and current filter dialects produces authored prose.
AWS: 400 whose message is the fixed sentence "Cannot use both legacy and new filter simultaneously." - no variable part at all. Smithy: the input declares both InstanceInformationFilterList (legacy) and Filters; the refusal text itself is recorded behavior only.
Emulator: 400 with "Filters and InstanceInformationFilterList can't be used in the same request: use Filters, which supersedes the legacy InstanceInformationFilterList".
Why: the refusal is correctly raised, but no client matching or displaying AWS's sentence sees it; SDK-level error-message assertions and user-facing surfaces diverge.
Fix: DescribeInstanceInformation input validation - emit AWS's fixed sentence for the dialect conflict.
Note: open issue 20261103T173026Z-ssm-describeinstanceinformation-and-describeinstanceproperties-filter-validation-is-missing-or.md describes this same wording divergence from the wire corpus under service `ssm`; it does not carry this behavioral check's replay key, so this behavioral failure is filed here and should be closed together with it.
Traces: ec2__ssm.w22n-legacy-and-current-filter-dialects-together-rejected.

Cause 3 of 3 - the out-of-enum PlatformTypes filter value is refused with a rewritten message.
AWS: 400 InvalidInstanceInformationFilterValue with the fixed sentence "PlatformTypes filter is invalid, valid values are Windows, Linux, MacOS" - fixed text naming the filter key and its legal values, comma-separated in that order, with no echo of the rejected value. The same message is returned in both the Filters and the legacy InstanceInformationFilterList dialect, and the enum is case-sensitive ('windows' and 'Windows2022' are both refused). Smithy: InvalidInstanceInformationFilterValue is a modeled 400 client error of DescribeInstanceInformation; the platform value domain is the PlatformType enum [Windows, Linux, MacOS].
Emulator: right code (InvalidInstanceInformationFilterValue), wrong message - "Value 'windows' for filter key 'PlatformTypes' is not valid: valid values are Linux | MacOS | Windows": it echoes the rejected value from the request, pipe-separates the values and re-sorts them alphabetically instead of using AWS's fixed Windows, Linux, MacOS sentence.
Why: a caller branching on or surfacing the valid-values sentence gets different text, and the echoed request value is content AWS never sends.
Fix: the DescribeInstanceInformation filter-value validation message for PlatformTypes (both dialects share it).
Traces: ec2__ssm.w22n-platform-types-filter-value-outside-the-enum-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout; all four checks are listed under the suite's `### ec2__ssm` heading.

Cause 1 - ec2__ssm.w22n-instance-information-max-results-above-fifty-rejected [mismatch]: MaxResults=5 and MaxResults=50 are accepted (controls pass); DescribeInstanceInformation(MaxResults=51) expected ('ValidationException', "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 50"), got ('ValidationException', "1 validation error detected: Value '51' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 50"). ec2__ssm.w22n-platform-name-filter-key-rejected-so-the-2022-conjunct-is-client-side [mismatch]: the PlatformName filter-key refusal matched; the combined call (Filters Key=PlatformName plus MaxResults=51) failed on the same message - "MaxResults must be validated ahead of the filter key, masking it, got \"1 validation error detected: Value '51' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 50\"".

Cause 2 - ec2__ssm.w22n-legacy-and-current-filter-dialects-together-rejected [mismatch]: "expected 'Cannot use both legacy and new filter simultaneously.', got \"Filters and InstanceInformationFilterList can't be used in the same request: use Filters, which supersedes the legacy InstanceInformationFilterList\"".

Cause 3 - ec2__ssm.w22n-platform-types-filter-value-outside-the-enum-rejected [mismatch]: "Filters PlatformTypes='windows' must be refused with ('InvalidInstanceInformationFilterValue', 'PlatformTypes filter is invalid, valid values are Windows, Linux, MacOS'), got ('InvalidInstanceInformationFilterValue', \"Value 'windows' for filter key 'PlatformTypes' is not valid: valid values are Linux | MacOS | Windows\")". The Filters=PlatformTypes=Windows and legacy InstanceInformationFilterList=Windows positive controls both succeeded.

Smithy model consulted: services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json (DescribeInstanceInformation input members Filters / InstanceInformationFilterList / MaxResults with @range 5..50, and its modeled errors including InvalidInstanceInformationFilterValue).

## Steps to Reproduce
1. Clone this checkout and replay the behavioral suite (simparity-behavioral-aws-v1) against the booted emulator.
2. Cause 1: run check ec2__ssm.w22n-instance-information-max-results-above-fifty-rejected - call DescribeInstanceInformation(MaxResults=51) and compare the ValidationException message to "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 50" (no echoed value). Then run ec2__ssm.w22n-platform-name-filter-key-rejected-so-the-2022-conjunct-is-client-side, whose combined Filters=PlatformName + MaxResults=51 call must return exactly that same sentence.
3. Cause 2: run ec2__ssm.w22n-legacy-and-current-filter-dialects-together-rejected - call DescribeInstanceInformation with both Filters and InstanceInformationFilterList set; the message must be exactly 'Cannot use both legacy and new filter simultaneously.'
4. Cause 3: run ec2__ssm.w22n-platform-types-filter-value-outside-the-enum-rejected - call DescribeInstanceInformation with Filters=[{Key: PlatformTypes, Values: ['windows']}] and again with InstanceInformationFilterList=[{key: PlatformTypes, valueSet: ['windows']}] (and with 'Windows2022'); each must return InvalidInstanceInformationFilterValue with 'PlatformTypes filter is invalid, valid values are Windows, Linux, MacOS'.
