Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__pricing.ecprice-describe-max-results-below-five-rejected ec2__pricing.ecprice-max-results-with-instance-ids-rejected ec2__pricing.ecprice-unknown-instance-state-filter-name-rejected
Issue-Key: ec2__pricing.ecprice-describe-max-results-below-five-rejected|_error#12|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: ec2__pricing.ecprice-max-results-with-instance-ids-rejected|run#16|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: ec2__pricing.ecprice-unknown-instance-state-filter-name-rejected|run#11|AssertionError|assert (code, msg) == ("InvalidParameterValue", expected_msg), (
Issue-Commit: 7ff327d821bcd1f37fa31be7f3b9efbce7107294

# DescribeInstances request validation diverges: MaxResults below the floor is accepted, and the InstanceIds+MaxResults and unknown-filter refusal messages are not AWS's wording (3 causes)

> _Note: this issue was identified against the code base at commit `7ff327d821bcd1f37fa31be7f3b9efbce7107294`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstances

## Description
Three net-new root causes, all in DescribeInstances' request validation, reached through the ec2__pricing running-cost-estimate checks. Each is a separate defect; all three must be closed before this issue is submitted. Related but NOT covering these: 20261104T142958Z-ec2-ten-ec2-refusal-messages-are-emulator-authored-prose.md and 20261103T021956Z-ec2-pricing-pricing-getproducts-matches-no-sku-for-the-standard.md (the latter is a Pricing GetProducts catalog gap; these three are EC2 DescribeInstances validation).

Cause 1 of 3: DescribeInstances accepts MaxResults below the documented floor.
AWS: DescribeInstances paginates with a MaxResults floor of 5 (and a ceiling of 1000); a value below the floor is refused with code InvalidParameterValue and the message "Value ( <n> ) for parameter maxResults is invalid. Expecting a value greater than 5." — fixed text "Value ( " ... " ) for parameter maxResults is invalid. Expecting a value greater than 5.", with the rejected value from the request as the only variable part, and the parameter named by its WIRE name maxResults. Smithy: DescribeInstances.MaxResults targets Integer with wire name maxResults and carries NO range trait, so neither botocore nor the model intercepts the value — the floor is service-side behavior (recorded behavior only, see the sibling GetRouteServerRoutingDatabase.MaxResults which does model range min 5 max 1000).
Emulator: the same call with MaxResults=4 SUCCEEDS with HTTP 200 and returns the matching reservations — no refusal at all.
Why: this is an overly-permissive accept. Code written against the emulator passes a MaxResults real AWS refuses, so it works in the emulator and fails in production on the first call; and a client probing the paging floor to size its own pages infers a floor that does not exist.
Fix: enforce the MaxResults floor in DescribeInstances' input validation and refuse with InvalidParameterValue and AWS's message template.
Traces: ec2__pricing.ecprice-describe-max-results-below-five-rejected.

Cause 2 of 3: the InstanceIds+MaxResults refusal message names SDK members instead of wire parameters.
AWS: DescribeInstances refuses InstanceIds together with MaxResults with code InvalidParameterCombination and the message "The parameter instancesSet cannot be used with the parameter maxResults" — fixed text, naming the two parameters by their ec2Query WIRE names: instancesSet (the InstanceIds list) and maxResults. Smithy: DescribeInstances.InstanceIds targets InstanceIdStringList (wire member InstanceId, serialized as the instancesSet set), MaxResults wire name maxResults; the operation declares no modeled errors, so code and message are recorded behavior.
Emulator: the code is correct but the message is "The parameter InstanceIds cannot be used with the parameter MaxResults" — the SDK/model member names, not the wire names AWS puts in the message.
Why: the message is part of the contract clients read and match. A caller that greps or asserts AWS's wording (or that maps the named wire parameter back to its own request builder) cannot match it, and an operator is pointed at member names that do not appear on the wire.
Fix: use the wire parameter names in the InvalidParameterCombination message emitted by DescribeInstances' input validation.
Traces: ec2__pricing.ecprice-max-results-with-instance-ids-rejected.

Cause 3 of 3: the unknown-filter-name refusal message is emulator-authored prose that identifies itself as an emulator.
AWS: EC2 validates the filter vocabulary: a filter NAME it does not recognise (here 'instance-state', the real name being instance-state-name) is refused with code InvalidParameterValue and the message "The filter '<name>' is invalid" — fixed text "The filter '" ... "' is invalid" with the rejected filter name from the request. (A recognised name with an unmodelled VALUE is NOT an error: it is a 200 with no match.) Smithy: DescribeInstances.Filters targets FilterList; the operation declares no modeled errors, so code and message are recorded behavior.
Emulator: the code is correct but the message is "The filter 'instance-state' is not one this emulator resolves" — different fixed text that also advertises the responder as an emulator rather than reproducing EC2's wording.
Why: a client or operator matching AWS's filter-validation text cannot match it, and the message tells the caller the problem is the responder's capability rather than their own invalid filter name — the opposite diagnosis.
Fix: emit AWS's filter-invalid message template from the filter-name validation used by DescribeInstances (and the shared filter-validation helper), with no self-reference to the emulator.
Traces: ec2__pricing.ecprice-unknown-instance-state-filter-name-rejected.

## Evidence
Cause 1 — ec2__pricing.ecprice-describe-max-results-below-five-rejected (mismatch):
  call: describe_instances(Filters=[tag filter matching one running instance], MaxResults=4)
  expected: InvalidParameterValue / "Value ( 4 ) for parameter maxResults is invalid. Expecting a value greater than 5."
  got: AssertionError: the call succeeded; real AWS rejects it, so there is no error code
  (control in the same check: MaxResults=5 pages the same sweep and finds the instance)

Cause 2 — ec2__pricing.ecprice-max-results-with-instance-ids-rejected (mismatch):
  call: describe_instances(InstanceIds=[i-...], MaxResults=5); code InvalidParameterCombination matched
  AssertionError: expected message 'The parameter instancesSet cannot be used with the parameter maxResults', got 'The parameter InstanceIds cannot be used with the parameter MaxResults'

Cause 3 — ec2__pricing.ecprice-unknown-instance-state-filter-name-rejected (mismatch):
  call: describe_instances(Filters=[{'Name':'tag:sim2real-probe',...},{'Name':'instance-state','Values':['running']}])
  AssertionError: an unknown filter name 'instance-state' was answered with ('InvalidParameterValue', "The filter 'instance-state' is not one this emulator resolves"), expected ('InvalidParameterValue', "The filter 'instance-state' is invalid")

## Steps to Reproduce
Replay the behavioral suite and re-run exactly these checks (each launches one tagged t3.nano, waits for 'running', then probes DescribeInstances):
1. ec2__pricing.ecprice-describe-max-results-below-five-rejected — DescribeInstances(Filters=<tag filter>, MaxResults=4); expect InvalidParameterValue with "Value ( 4 ) for parameter maxResults is invalid. Expecting a value greater than 5." (and MaxResults=5 must still page successfully).
2. ec2__pricing.ecprice-max-results-with-instance-ids-rejected — DescribeInstances(InstanceIds=[<the instance>], MaxResults=5); expect InvalidParameterCombination with "The parameter instancesSet cannot be used with the parameter maxResults".
3. ec2__pricing.ecprice-unknown-instance-state-filter-name-rejected — DescribeInstances with a filter named 'instance-state'; expect InvalidParameterValue with "The filter 'instance-state' is invalid" (while instance-state-name with an unknown value must stay a 200 with no match).
