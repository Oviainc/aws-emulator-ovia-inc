Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: b2bi.create-capability-nonexistent-transformer-not-found b2bi.create-partnership-missing-capability-not-found b2bi.get-profile-not-found b2bi.list-capabilities-max-results-out-of-range-error b2bi.profile-lifecycle-logging-enabled-provisions-log-group b2bi.profile-logging-disabled-no-log-group b2bi.profile-quota-enforced-under-concurrent-creation-rejected
Issue-Shape-Id: com.amazonaws.b2bi#B2BI
Issue-Key: b2bi.create-capability-nonexistent-transformer-not-found|_error#5|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded and returned {result!r}")
Issue-Key: b2bi.create-partnership-missing-capability-not-found|_error#6|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded and returned {result!r}")
Issue-Key: b2bi.get-profile-not-found|run#5|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: b2bi.list-capabilities-max-results-out-of-range-error|run#5|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: b2bi.profile-lifecycle-logging-enabled-provisions-log-group|run#13|AssertionError|assert any(g["logGroupName"] == log_group_name for g in groups), (
Issue-Key: b2bi.profile-logging-disabled-no-log-group|run#10|AssertionError|assert any(g["logGroupName"] == control_log_group for g in control_groups), (
Issue-Key: service-quotas|ListServiceQuotas|NoSuchResourceException
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# b2bi control plane accepts dangling references, skips log-group provisioning, has no Service Quotas entry, and uses wrong error-message rules (6 causes)

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCapability

## Description
Six independent root causes found in the b2bi control plane by the behavioral suite (b2bi is bootstrapped; the emulator answered every call below). Each cause must be closed before this issue is done.

Cause 1 of 6: CreateCapability does not validate the referenced transformerId.
AWS: CreateCapability models ResourceNotFoundException (httpError 404) among its errors (Smithy: CreateCapability errors = AccessDenied/Conflict/InternalServer/ResourceNotFound/ServiceQuotaExceeded/Throttling/Validation; success is 201). The recorded behavior is that an edi configuration whose transformerId does not exist is refused with ResourceNotFoundException and no capability is created; the recorded message is, as an example, "Transformer with transformerId <id> not found".
Emulator: the call succeeds with HTTP 201 and mints a capability that permanently references a nonexistent transformer (replay returned capabilityId ca-… with configuration.edi.transformerId = xfmr-doesnotexist00000000).
Why: a client that relies on the create call to validate its wiring gets a silently broken capability; its own error handling for ResourceNotFoundException is never exercised and the leaked resource must be cleaned up by hand.
Fix: the CreateCapability handler's validation must resolve configuration.edi.transformerId in the b2bi state store before inserting, and return ResourceNotFoundException (404) when it is absent.
Traces: b2bi.create-capability-nonexistent-transformer-not-found.

Cause 2 of 6: CreatePartnership does not validate the referenced capability ids.
AWS: CreatePartnership models ResourceNotFoundException (httpError 404) and `capabilities` is a required list (Smithy: CreatePartnership.capabilities -> PartnershipCapabilities, required). The recording shows a partnership naming a capability id that does not exist is refused with ResourceNotFoundException and nothing is created; recorded message, as an example: "Capability ID <id> not found."
Emulator: the call succeeds with HTTP 201 (replay returned partnershipId ps-… with capabilities = ['cap-doesnotexist0000000000']).
Why: partnerships pointing at nonexistent capabilities are accepted, so a client cannot distinguish a valid trading relationship from a typo, and downstream job/parsing flows fail much later with a confusing error.
Fix: the CreatePartnership handler's validation must resolve every element of `capabilities` (and reject with ResourceNotFoundException/404 on the first unknown one) before persisting.
Traces: b2bi.create-partnership-missing-capability-not-found.

Cause 3 of 6: ResourceNotFoundException message rule for profiles diverges.
AWS: GetProfile models ResourceNotFoundException (httpError 404). The message is built from the request's profileId, and the recorded AWS form is "Profile ID <profileId> not found." The same request-derived template appears across the recorded b2bi not-found paths (capability: "Capability ID <id> not found.", partnership: "Partnership with partnershipId <id> not found.", transformer: "Transformer not found with ID <id>").
Emulator: returns the correct code ResourceNotFoundException but the message "Could not find profile with ID profile-doesnotexist000000" instead of "Profile ID profile-doesnotexist000000 not found."
Why: clients and tests that match on the documented not-found wording (and tooling that parses the identifier out of it) misclassify the failure even though the code is right.
Fix: the not-found error construction in the Profile operations' handlers/state lookup should use the service's request-derived template rather than an emulator-local phrase.
Traces: b2bi.get-profile-not-found.

Cause 4 of 6: range-constraint ValidationException echoes the rejected value.
AWS: ListCapabilities.maxResults has range min 1 / max 100 (Smithy: ListCapabilities.maxResults -> MaxResults, range {1,100}) and the operation models ValidationException (httpError 400). The validation message is built from the request and, as recorded, names only the member and the constraint: "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100".
Emulator: returns ValidationException but interpolates the offending value: "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100".
Why: the message is request-derived and therefore a stable contract; a client surfacing or asserting on AWS's constraint text sees an extra value token that AWS does not emit for this integer range constraint.
Fix: the shared constraint-validation message builder used by the b2bi operations must omit the value prefix for range violations on integer members.
Traces: b2bi.list-capabilities-max-results-out-of-range-error.

Cause 5 of 6: logging=ENABLED does not provision a CloudWatch Logs log group.
AWS: CreateProfile.logging is a required enum and both CreateProfile and GetProfile return logGroupName (Smithy: CreateProfile/GetProfile output member logGroupName -> LogGroupName, length 1..512). With logging ENABLED the service provisions a real CloudWatch Logs log group for the profile, discoverable via logs:DescribeLogGroups with that name (recorded behavior; the recorded name is of the form b2bi/<profileId>-Logs).
Emulator: returns a logGroupName in the CreateProfile/GetProfile payloads, but no log group with that name exists in the (bootstrapped) CloudWatch Logs emulator — DescribeLogGroups with that prefix returns [].
Why: the profile reports a destination that cannot be read, subscribed to or deleted, so any client that follows logGroupName into CloudWatch Logs breaks; the ENABLED vs DISABLED distinction has no observable effect.
Fix: the CreateProfile handler's state transition must actually create the log group in the CloudWatch Logs state when logging=ENABLED (and leave none when DISABLED), so logGroupName resolves.
Traces: b2bi.profile-lifecycle-logging-enabled-provisions-log-group, b2bi.profile-logging-disabled-no-log-group.

Cause 6 of 6: no Service Quotas entry for ServiceCode 'b2bi' and no profile-quota enforcement.
AWS: the account-wide customer-profiles limit is published in Service Quotas under ServiceCode "b2bi", and CreateProfile models ServiceQuotaExceededException (Smithy: CreateProfile errors include ServiceQuotaExceededException, httpError 402); racers beyond the remaining slots are refused with that code rather than creating more profiles than the quota allows (recorded behavior).
Emulator: servicequotas ListServiceQuotas(ServiceCode='b2bi') fails with NoSuchResourceException "no service quotas are available for ServiceCode 'b2bi'", so the quota is not discoverable at all, and CreateProfile never emits ServiceQuotaExceededException.
Why: a client cannot discover the limit it must plan against, and nothing enforces it, so quota-exhaustion handling (402/ServiceQuotaExceededException) can never be exercised.
Fix: register b2bi's quota catalogue in the Service Quotas emulator's quota data and have the CreateProfile handler enforce the profile count against it, returning ServiceQuotaExceededException.
Traces: b2bi.profile-quota-enforced-under-concurrent-creation-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1, 0/42 b2bi checks passed.

Cause 1 — b2bi.create-capability-nonexistent-transformer-not-found [mismatch]: expected ResourceNotFoundException; got HTTP 201 with {'capabilityId': 'ca-146e3d8fa22f8234f', ... 'transformerId': 'xfmr-doesnotexist00000000'} (AssertionError: expected the call to be refused, but it succeeded).
Cause 2 — b2bi.create-partnership-missing-capability-not-found [mismatch]: expected ResourceNotFoundException; got HTTP 201 with {'partnershipId': 'ps-4dc97e0bbaea250d0', 'capabilities': ['cap-doesnotexist0000000000'], ...}.
Cause 3 — b2bi.get-profile-not-found [mismatch]: AssertionError: expected message 'Profile ID profile-doesnotexist000000 not found.', got 'Could not find profile with ID profile-doesnotexist000000' (code ResourceNotFoundException was correct).
Cause 4 — b2bi.list-capabilities-max-results-out-of-range-error [mismatch]: AssertionError: expected message "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100", got "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100".
Cause 5 — b2bi.profile-lifecycle-logging-enabled-provisions-log-group [mismatch]: AssertionError: expected a real CloudWatch log group named 'b2bi/p-aa9d7e1462ac9347a-Logs' (logs.describe_log_groups returned no match); b2bi.profile-logging-disabled-no-log-group [mismatch]: AssertionError: positive control: expected a real CloudWatch log group named 'b2bi/p-c7844125fb8db9fd5-Logs', found [].
Cause 6 — b2bi.profile-quota-enforced-under-concurrent-creation-rejected [error]: botocore.errorfactory.NoSuchResourceException: An error occurred (NoSuchResourceException) when calling the ListServiceQuotas operation: no service quotas are available for ServiceCode 'b2bi'.

## Steps to Reproduce
1. Boot the emulator and point a botocore client at it (b2bi, logs, service-quotas).
2. Cause 1: create_capability(name=..., type='edi', configuration={'edi': {... 'transformerId': 'xfmr-doesnotexist00000000' ...}}) -> expect ResourceNotFoundException (404); emulator returns 201.
3. Cause 2: create_profile(...), then create_partnership(profileId=<real>, name=..., email='partner@example.com', capabilities=['cap-doesnotexist0000000000']) -> expect ResourceNotFoundException (404); emulator returns 201.
4. Cause 3: get_profile(profileId='profile-doesnotexist000000') and compare Error.Message to 'Profile ID profile-doesnotexist000000 not found.'.
5. Cause 4: list_capabilities(maxResults=101) and compare the ValidationException message.
6. Cause 5: create_profile(..., logging='ENABLED'), take logGroupName from the response, then logs.describe_log_groups(logGroupNamePrefix=<logGroupName>) -> expect the group to exist.
7. Cause 6: service-quotas list_service_quotas(ServiceCode='b2bi') -> expect the customer-profiles quota; then race CreateProfile past the quota and expect ServiceQuotaExceededException.
8. Re-replay the checks listed in trace_ids.
