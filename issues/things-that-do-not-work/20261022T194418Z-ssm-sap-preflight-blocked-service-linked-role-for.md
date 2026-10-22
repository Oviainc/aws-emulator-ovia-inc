Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm-sap.concurrent-application-refresh-contention ssm-sap.deregister-application-removes-resource ssm-sap.get-application-missing-identifier-rejected ssm-sap.get-application-nonexistent-id-validation-error ssm-sap.get-operation-invalid-uuid-format-rejected ssm-sap.get-operation-valid-uuid-not-found-rejected ssm-sap.list-components-nonexistent-application-resource-not-found ssm-sap.list-operations-tracks-register-operation ssm-sap.put-resource-permission-invalid-action-type-rejected ssm-sap.register-application-discovery-status-transition ssm-sap.register-application-duplicate-id-conflict-rejected ssm-sap.register-application-invalid-instance-pattern-rejected ssm-sap.register-application-quota-limit-rejected ssm-sap.register-application-sap-abap-credentials-rejected ssm-sap.register-application-synchronous-response-state ssm-sap.resource-permission-lifecycle-get-after-delete ssm-sap.start-application-nonexistent-rejected ssm-sap.start-configuration-checks-error-paths ssm-sap.stop-application-on-registering-app-is-accepted ssm-sap.tag-resource-lifecycle-and-nonexistent-arn ssm-sap.list-applications-malformed-nexttoken-rejected
Issue-Shape-Id: com.amazonaws.ssmsap#SsmSap
Issue-Key: ssm-sap.concurrent-application-refresh-contention|_ensure_service_linked_role#1|Timeout|time.sleep(5.0)
Issue-Key: ssm-sap.deregister-application-removes-resource|_ensure_service_linked_role#1|Timeout|time.sleep(5.0)
Issue-Key: ssm-sap.get-application-missing-identifier-rejected|_ensure_service_linked_role#1|Timeout|time.sleep(5.0)
Issue-Key: ssm-sap.get-application-nonexistent-id-validation-error|_ensure_service_linked_role#1|Timeout|time.sleep(5.0)
Issue-Key: ssm-sap.get-operation-invalid-uuid-format-rejected|_ensure_service_linked_role#1|Timeout|time.sleep(5.0)
Issue-Key: ssm-sap.get-operation-valid-uuid-not-found-rejected|_ensure_service_linked_role#1|Timeout|time.sleep(5.0)
Issue-Key: ssm-sap.list-components-nonexistent-application-resource-not-found|_ensure_service_linked_role#1|Timeout|time.sleep(5.0)
Issue-Key: ssm-sap.list-operations-tracks-register-operation|_ensure_service_linked_role#1|Timeout|time.sleep(5.0)
Issue-Key: ssm-sap.put-resource-permission-invalid-action-type-rejected|_ensure_service_linked_role#1|Timeout|time.sleep(5.0)
Issue-Key: ssm-sap.register-application-discovery-status-transition|_ensure_service_linked_role#2|Timeout|time.sleep(5.0)
Issue-Key: ssm-sap.register-application-duplicate-id-conflict-rejected|_ensure_service_linked_role#1|Timeout|time.sleep(5.0)
Issue-Key: ssm-sap.register-application-invalid-instance-pattern-rejected|_ensure_service_linked_role#1|Timeout|time.sleep(5.0)
Issue-Key: ssm-sap.register-application-quota-limit-rejected|_ensure_service_linked_role#1|Timeout|time.sleep(5.0)
Issue-Key: ssm-sap.register-application-sap-abap-credentials-rejected|_ensure_service_linked_role#1|Timeout|time.sleep(5.0)
Issue-Key: ssm-sap.register-application-synchronous-response-state|_ensure_service_linked_role#1|Timeout|time.sleep(5.0)
Issue-Key: ssm-sap.resource-permission-lifecycle-get-after-delete|_ensure_service_linked_role#1|Timeout|time.sleep(5.0)
Issue-Key: ssm-sap.start-application-nonexistent-rejected|_ensure_service_linked_role#1|Timeout|time.sleep(5.0)
Issue-Key: ssm-sap.start-configuration-checks-error-paths|_ensure_service_linked_role#1|Timeout|time.sleep(5.0)
Issue-Key: ssm-sap.stop-application-on-registering-app-is-accepted|_ensure_service_linked_role#1|Timeout|time.sleep(5.0)
Issue-Key: ssm-sap.tag-resource-lifecycle-and-nonexistent-arn|_ensure_service_linked_role#1|Timeout|time.sleep(5.0)
Issue-Key: ssm-sap.list-applications-malformed-nexttoken-rejected|run#5|AssertionError|assert err.get("Message") == expected, f"expected message {expected!r}, got {err.get('Message')!r}"
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# ssm-sap preflight blocked: service-linked role for ssm-sap.amazonaws.com is created under a derived name (AWSServiceRoleForSsm-sap) instead of AWSServiceRoleForAWSSSMForSAP, and @pattern ValidationException messages leak the regex anchors

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateServiceLinkedRole

## Description
Two distinct root causes, both grounded in the ssm-sap behavioral checks. Cause 1 blocks 20 of the 21 checks before they reach any ssm-sap assertion; Cause 2 is the only check that got far enough to compare a response.

=== Cause 1 of 2: the service-linked role AWS SSM for SAP depends on is created under the wrong role name, so it is never retrievable by the name AWS gives it ===
Failing checks: every ssm-sap check except ssm-sap.list-applications-malformed-nexttoken-rejected (20 checks, all listed in trace_ids).

AWS: Smithy (IAM model, CreateServiceLinkedRole): input AWSServiceName (groupNameType, len 1..128, pattern ^[\w+=,.@-]+$) **required**, CustomSuffix, Description; output Role; errors InvalidInputException (400), LimitExceededException (409), NoSuchEntityException (404), ServiceFailureException (500). The IAM model does NOT carry the per-service role-name spelling; the member doc only says CustomSuffix "is combined with the service-provided prefix to form the complete role name". Recorded behavior only for the spelling itself: AWS creates the SLR for the principal ssm-sap.amazonaws.com as the role named AWSServiceRoleForAWSSSMForSAP (the name the checks read back with iam GetRole, and the name AWS SSM for SAP requires before RegisterApplication will work). The service-provided prefix is AWSServiceRoleFor + the service's own AWS-assigned label, which is NOT the first label of the principal.

Emulator: services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/create_service_linked_role.rs derives the name mechanically in fn service_linked_role_name(): it takes the first dot-separated label of the principal and capitalises its first character. For AWSServiceName="ssm-sap.amazonaws.com" that yields the role name "AWSServiceRoleForSsm-sap". The create SUCCEEDS (200, Role returned) under that name, and GetRole (src/ops/get_role.rs, which looks the role up by case-folded name only) then answers NoSuchEntity / HTTP 404 for RoleName="AWSServiceRoleForAWSSSMForSAP" forever. The checks' preflight helper _ensure_service_linked_role() therefore loops on its 60s GetRole-visibility deadline and never returns, and every check that calls it dies at the harness's 60.0s limit ("Check timed out after 60.0s") without ever issuing an ssm-sap request. The one check that does not call the helper did not time out, which isolates the hang to this path.

Why: a client follows the documented bootstrap for AWS SSM for SAP — create the service-linked role, confirm it with iam GetRole on the documented name AWSServiceRoleForAWSSSMForSAP, then RegisterApplication. Against the emulator the confirm step never succeeds, so the client either spins until its own timeout or concludes the role was not created, even though a role WAS created. The account is also left holding a role under a name no AWS tool or policy will ever reference, and a second create for the same principal will be refused as a duplicate of that wrong name.

Fix: IAM's CreateServiceLinkedRole handler — the role-name derivation in service_linked_role_name() needs AWS's per-service SLR name for the principal rather than a capitalisation of the principal's first label, so that at minimum ssm-sap.amazonaws.com maps to AWSServiceRoleForAWSSSMForSAP; GetRole must then resolve that name. Note the handler's own comment already records that this spelling is "a per-service string no part of the API model carries" — the derived fallback is what diverges. Whatever ssm-sap's own RegisterApplication preconditions are must agree with the same name.

Traces: all 20 timing-out ssm-sap check ids in trace_ids; code read at services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/create_service_linked_role.rs:143-159 (name derivation) and src/ops/get_role.rs:161-170 (lookup by name only).

=== Cause 2 of 2: the ValidationException message for a @pattern violation reports the modeled regex WITH its ^...$ anchors ===
Failing check: ssm-sap.list-applications-malformed-nexttoken-rejected.

AWS: Smithy (ssm-sap): ListApplications input NextToken targets NextToken with pattern ^.{16,2048}$; errors ValidationException (400), ResourceNotFoundException (404), InternalServerException (500). The rejection message is built from the request and the violated member constraint, so its rule is known: AWS renders "N validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" with the pattern's outer anchors stripped — recorded as "Member must satisfy regular expression pattern: .{16,2048}".

Emulator: returns the right code and status (ValidationException, HTTP 400) but renders the pattern verbatim from the model including the anchors: "1 validation error detected: Value 'short' at 'nextToken' failed to satisfy constraint: Member must satisfy regular expression pattern: ^.{16,2048}$". The rest of the message (count, value, member path nextToken, constraint prose) matches.

Why: a client that surfaces or parses the constraint out of the message — or a test fixture asserting the documented message — sees a different regex than AWS states, and a client that re-applies the reported regex gets anchored matching semantics AWS did not report.

Fix: the shared request-validation layer that renders @pattern constraint violations into ValidationException text (the generated request walk's constraint-message builder), not ssm-sap's ListApplications handler — the same renderer governs every @pattern member in every service, so the anchor stripping belongs there once.

Traces: ssm-sap.list-applications-malformed-nexttoken-rejected; modeled pattern confirmed via smithy_query(query="operation", operation="ListApplications") on services/service-com-amazonaws-ssmsap-ssmsap/smithy-model/com-amazonaws-ssmsap-ssmsap.json.

## Evidence
Cause 1 — expected (AWS): iam CreateServiceLinkedRole(AWSServiceName="ssm-sap.amazonaws.com") makes the role AWSServiceRoleForAWSSSMForSAP, and a subsequent iam GetRole(RoleName="AWSServiceRoleForAWSSSMForSAP") returns 200 with that Role. Actual (emulator): CreateServiceLinkedRole returns 200 with Role.RoleName="AWSServiceRoleForSsm-sap" (first principal label capitalised); GetRole(RoleName="AWSServiceRoleForAWSSSMForSAP") returns NoSuchEntity / HTTP 404 indefinitely. Observed effect in the suite: 20/21 ssm-sap checks report "Error: Check timed out after 60.0s" — the helper _ensure_service_linked_role() polls GetRole on a 60s deadline and never returns, so no ssm-sap request is ever made. The single check that does not call that helper (ssm-sap.list-applications-malformed-nexttoken-rejected) did NOT time out, isolating the hang to the SLR preflight.

Cause 2 — expected message (AWS, rule-derived): "1 validation error detected: Value 'short' at 'nextToken' failed to satisfy constraint: Member must satisfy regular expression pattern: .{16,2048}". Actual: "1 validation error detected: Value 'short' at 'nextToken' failed to satisfy constraint: Member must satisfy regular expression pattern: ^.{16,2048}$". Error code ValidationException and HTTP 400 both match; the divergence is the anchored regex in a message whose construction rule is the request plus the member's modeled @pattern.

## Steps to Reproduce
Cause 1:
1. Boot the emulator.
2. iam GetRole(RoleName="AWSServiceRoleForAWSSSMForSAP") -> NoSuchEntity (expected on a fresh account).
3. iam CreateServiceLinkedRole(AWSServiceName="ssm-sap.amazonaws.com") -> 200; observe Role.RoleName is "AWSServiceRoleForSsm-sap", not "AWSServiceRoleForAWSSSMForSAP".
4. iam GetRole(RoleName="AWSServiceRoleForAWSSSMForSAP") -> still NoSuchEntity / 404, forever.
5. Replay any ssm-sap check other than ssm-sap.list-applications-malformed-nexttoken-rejected: it hangs in _ensure_service_linked_role() and is killed at 60.0s.

Cause 2:
1. ssm-sap ListApplications(NextToken="short") -> ValidationException / HTTP 400.
2. Compare Error.Message against AWS's: the emulator appends/prepends the modeled anchors, emitting "pattern: ^.{16,2048}$" where AWS emits "pattern: .{16,2048}".
3. ssm-sap ListApplications(MaxResults=5) still succeeds (positive control passes).
