Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-w22n-traj-filter-combination-mistakes
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-w22n-traj-filter-combination-mistakes ssm-w22n-traj-filter-value-enum-casing ssm-w22n-traj-paging-and-instance-id-mistakes ssm-w22n-traj-platform-name-filter-key-mistakes ssm-w22n-traj-instance-properties-route
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|DescribeInstanceInformation|4xx->4xx|$.message
Issue-Key: ssm|DescribeInstanceInformation|4xx->2xx|$.__type
Issue-Key: ssm|DescribeInstanceInformation|4xx->2xx|$.message
Issue-Key: ssm|DescribeInstanceInformation|4xx->2xx|status
Issue-Key: ssm|DescribeInstanceProperties|4xx->2xx|$.__type
Issue-Key: ssm|DescribeInstanceProperties|4xx->2xx|$.message
Issue-Key: ssm|DescribeInstanceProperties|4xx->2xx|status
Issue-Key: ssm|DescribeInstanceInformation|4xx->4xx|$.Message
Issue-Key: ssm|DescribeInstanceInformation|4xx->4xx|$.__type
Issue-Commit: 133bbb645e9618d24e6206d20cee2b832cb9c7e9

# DescribeInstanceInformation and DescribeInstanceProperties filter validation is missing or misclassified

> _Note: this issue was identified against the code base at commit `133bbb645e9618d24e6206d20cee2b832cb9c7e9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceInformation

## Description
The managed-node read surface rejects malformed filter input with emulator-authored text, accepts two malformed inputs AWS refuses, and misclassifies a third. 5 CAUSES.

Cause 1 of 5 - mixing the legacy and current filter members produces authored prose. AWS: 400 whose message is the fixed sentence 'Cannot use both legacy and new filter simultaneously.' - no variable part (Smithy: DescribeInstanceInformation input has both InstanceInformationFilterList (legacy) and Filters; the refusal text is recorded behavior only). Emulator: 400 with "Filters and InstanceInformationFilterList can't be used in the same request: use Filters, which supersedes the legacy InstanceInformationFilterList". Why: the refusal is correct but no client matching or displaying AWS's sentence sees it; this single cluster covers 14 replayed steps. Fix: DescribeInstanceInformation input validation message. Traces: ssm-w22n-traj-filter-combination-mistakes#task#1, #task#4, #task#5, #task#6; ssm-w22n-traj-filter-value-enum-casing#task#1 through #task#6; ssm-w22n-traj-paging-and-instance-id-mistakes#task#1, #task#2, #task#7; ssm-w22n-traj-platform-name-filter-key-mistakes#task#5.

Cause 2 of 5 - a duplicated filter key is accepted. AWS: 400 ValidationException with message 'Cannot specify duplicate filter <Key>' - fixed text plus the duplicated key name from the request. Emulator: 200 with a normal result; the duplicate is never detected. Why: a caller that accidentally repeats InstanceIds gets silently different filtering semantics instead of an error, so a bug in its request construction is invisible. Fix: DescribeInstanceInformation filter validation - reject repeated filter keys. Traces: ssm-w22n-traj-filter-combination-mistakes#task#3.

Cause 3 of 5 - an out-of-domain filter VALUE is accepted by DescribeInstanceProperties. AWS: 400 InvalidInstancePropertyFilterValue (Smithy: DescribeInstanceProperties error InvalidInstancePropertyFilterValue, 'The specified filter value isn''t valid'), message 'PlatformTypes filter is invalid, valid values are Windows, Linux, MacOS' - fixed text naming the filter key and its legal values (Smithy: PlatformType enum [Windows, Linux, MacOS]). Emulator: 200. Why: a typo'd platform value silently yields an empty or unfiltered list rather than an error, so the caller cannot tell 'no such platform' from 'no instances'. Fix: DescribeInstanceProperties filter-value validation against the key's value domain. Traces: ssm-w22n-traj-instance-properties-route#task#2, ssm-w22n-traj-instance-properties-route#task#3.

Cause 4 of 5 - the InvalidNextToken body carries an invented Message. AWS: 400 InvalidNextToken for this operation with NO Message member in the body (Smithy: InvalidNextToken declares an optional Message; the service leaves it unset here, per the recording). Emulator: adds $.Message 'The NextToken value is not valid'. Why: a client branching on the presence of a message, or logging the error body verbatim, reproduces text AWS never sends. Fix: DescribeInstanceInformation's InvalidNextToken construction - do not populate Message. Traces: ssm-w22n-traj-paging-and-instance-id-mistakes#task#4.

Cause 5 of 5 - a malformed activation id in a filter is classified as a filter-value error with no message. AWS: 400 __type InvalidActivationId (Smithy: InvalidActivationId, 'The activation ID isn''t valid. Verify that you entered the correct ActivationId or ActivationCode and try again.', member Message) with $.Message 'Invalid Activation Ids <value>' - fixed text plus the rejected value(s) from the request. Emulator: __type InvalidInstanceInformationFilterValue with no Message. Why: a caller that catches InvalidActivationId to re-register an activation never sees it, and the empty body hides which value was rejected. Fix: DescribeInstanceInformation's ActivationIds filter validation and its error classification. Traces: ssm-w22n-traj-paging-and-instance-id-mistakes#task#6.

## Evidence
Replay of simparity-wire-aws-v1. Representative (ssm-w22n-traj-filter-combination-mistakes#task#1, DescribeInstanceInformation, 14 affected steps): expected 400 / actual 400; $.message expected 'Cannot use both legacy and new filter simultaneously.', got "Filters and InstanceInformationFilterList can't be used in the same request: use Filters, which supersedes the legacy InstanceInformationFilterList". #task#3: status expected 400, got 200; $.__type expected 'ValidationException' and $.message expected 'Cannot specify duplicate filter InstanceIds' both absent. ssm-w22n-traj-instance-properties-route#task#2 (DescribeInstanceProperties): status expected 400, got 200; $.__type expected 'InvalidInstancePropertyFilterValue', $.message expected 'PlatformTypes filter is invalid, valid values are Windows, Linux, MacOS'. ssm-w22n-traj-paging-and-instance-id-mistakes#task#4: $.Message unexpected_present (expected None, got 'The NextToken value is not valid'). #task#6: $.__type expected 'InvalidActivationId', got 'InvalidInstanceInformationFilterValue'; $.Message missing, expected 'Invalid Activation Ids not-an-activation'. Smithy model consulted: services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json (DescribeInstanceInformation and DescribeInstanceProperties inputs and errors, InvalidActivationId, InvalidInstancePropertyFilterValue, InvalidNextToken, PlatformType).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1. 2. Representative: fixture ssm-w22n-traj-filter-combination-mistakes step task#1 - DescribeInstanceInformation with both InstanceInformationFilterList and Filters; compare $.message. 3. Step task#3 of the same fixture - DescribeInstanceInformation with the InstanceIds filter supplied twice; expect 400 ValidationException. 4. Fixture ssm-w22n-traj-instance-properties-route step task#2 - DescribeInstanceProperties filtering PlatformTypes with a value outside [Windows, Linux, MacOS]; expect 400 InvalidInstancePropertyFilterValue. 5. Fixture ssm-w22n-traj-paging-and-instance-id-mistakes step task#4 - DescribeInstanceInformation with a bogus NextToken; the error body must carry no Message. 6. Step task#6 - DescribeInstanceInformation filtering ActivationIds with 'not-an-activation'; expect InvalidActivationId with Message.
