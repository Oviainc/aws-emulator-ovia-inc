Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rum.create-app-monitor-duplicate-name-conflict rum.create-app-monitor-duplicate-name-conflict-blocked rum.create-app-monitor-duplicate-name-conflict-rejected rum.duplicate-app-monitor-name-conflict-rejected rum.get-app-monitor-deleted-or-unknown-name-not-found rum.delete-app-monitor-twice-rejected rum.delete-app-monitor-wrong-state-after-delete-rejected rum.update-app-monitor-unknown-id-rejected rum.update-app-monitor-on-deleted-resource-blocked rum.create-app-monitor-malformed-domain-rejected rum.put-rum-events-unknown-app-monitor-rejected rum.e2e-put-rum-events-after-delete-rejected rum.delete-resource-policy-nonexistent-rejected rum.delete-resource-policy-without-existing-conflict rum.get-resource-policy-missing-policy-conflict rum.put-resource-policy-malformed-document-rejected
Issue-Shape-Id: com.amazonaws.rum#RUM
Issue-Key: rum.create-app-monitor-duplicate-name-conflict|run#6|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: rum.create-app-monitor-duplicate-name-conflict-blocked|run#9|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: rum.create-app-monitor-duplicate-name-conflict-rejected|run#11|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: rum.duplicate-app-monitor-name-conflict-rejected|run#8|AssertionError|assert message == f"Resource {name} already exists", message
Issue-Key: rum.get-app-monitor-deleted-or-unknown-name-not-found|run#16|AssertionError|assert message == f"Resource not found {victim_name}", message
Issue-Key: rum.delete-app-monitor-twice-rejected|run#10|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: rum.delete-app-monitor-wrong-state-after-delete-rejected|run#9|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: rum.update-app-monitor-unknown-id-rejected|run#6|AssertionError|assert err.get("Message") == expected_message, (
Issue-Key: rum.update-app-monitor-on-deleted-resource-blocked|run#9|AssertionError|assert err.get("Message") == expected_message, (
Issue-Key: rum.create-app-monitor-malformed-domain-rejected|_client_error#3|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded with {result!r}")
Issue-Key: rum.put-rum-events-unknown-app-monitor-rejected|run#10|AssertionError|assert code == "AccessDeniedException", repr(code)
Issue-Key: rum.e2e-put-rum-events-after-delete-rejected|run#10|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code!r}"
Issue-Key: rum.delete-resource-policy-nonexistent-rejected|run#4|AssertionError|assert code == "PolicyNotFoundException", f"expected PolicyNotFoundException, got {code}"
Issue-Key: rum.delete-resource-policy-without-existing-conflict|run#4|AssertionError|assert code == "PolicyNotFoundException", f"expected PolicyNotFoundException, got {code!r}"
Issue-Key: rum.get-resource-policy-missing-policy-conflict|run#4|AssertionError|assert code == "PolicyNotFoundException", f"expected PolicyNotFoundException, got {code}"
Issue-Key: rum.put-resource-policy-malformed-document-rejected|run#7|AssertionError|assert code == "MalformedPolicyDocumentException", (
Issue-Commit: f973f78f74c99aad4e85f2736f1fde89ed7327e3

# RUM error representation and input validation diverge: conflict/not-found message rules, unenforced Domain pattern, wrong PutRumEvents error code, and resource-policy operations returning NotImplemented

> _Note: this issue was identified against the code base at commit `f973f78f74c99aad4e85f2736f1fde89ed7327e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAppMonitor

## Description
Seven distinct root causes in the rum emulator's error representation and validation. All are in one service (rum). A fixer must close ALL SEVEN before submitting.

=== Cause 1 of 7: ConflictException message for a duplicate app-monitor name is not the request-derived AWS message ===
AWS: CreateAppMonitor models ConflictException (Smithy: CreateAppMonitor errors include ConflictException, httpError 409; input member Name -> AppMonitorName, required, @length 1..255, @pattern ^(?!\.)[\.\-_#A-Za-z0-9]+$). Recorded behavior: when a name is already taken, RUM returns ConflictException whose message is built from the request as the fixed form "Resource <Name> already exists" (no trailing period), i.e. the literal prefix "Resource ", the Name member verbatim, and the literal suffix " already exists". Four independent checks across three different name prefixes agree on that rule, so the rule — not any one recorded string — is the requirement.
Emulator: returns ConflictException with "App monitor <Name> already exists." (different prefix, different suffix, trailing period).
Why: clients and test harnesses that match on the documented message rule (and anything that parses the resource identifier out of it) misclassify a real name collision; the discrepancy is in the message the service itself constructs from the request, not in environment-specific text.
Fix: the CreateAppMonitor handler's conflict path — the message construction for ConflictException. The error code (ConflictException) and status (409) are already correct.
Traces: rum.create-app-monitor-duplicate-name-conflict, rum.create-app-monitor-duplicate-name-conflict-blocked, rum.create-app-monitor-duplicate-name-conflict-rejected, rum.duplicate-app-monitor-name-conflict-rejected.

=== Cause 2 of 7: ResourceNotFoundException message for an unknown/deleted app monitor is not the request-derived AWS message ===
AWS: GetAppMonitor, UpdateAppMonitor and DeleteAppMonitor all model ResourceNotFoundException (Smithy: httpError 404) with input member Name as an @httpLabel. Recorded behavior: for a name that does not exist (never created, or already deleted), RUM returns ResourceNotFoundException whose message is built from the request as "Resource not found <Name>" — literal prefix "Resource not found " followed by the Name member verbatim, no trailing period. Five checks across three operations agree, so that construction rule is the requirement.
Emulator: returns ResourceNotFoundException with "App monitor <Name> does not exist."
Why: the code and status are right but the service-constructed message is wrong, so clients keying off the documented message rule (and tooling extracting the missing resource name) behave differently against the emulator than against AWS. The divergence is uniform across Get/Update/Delete, indicating one shared message builder.
Fix: the shared not-found error construction used by the GetAppMonitor / UpdateAppMonitor / DeleteAppMonitor handlers (the app-monitor lookup helper that raises ResourceNotFoundException).
Traces: rum.get-app-monitor-deleted-or-unknown-name-not-found, rum.delete-app-monitor-twice-rejected, rum.delete-app-monitor-wrong-state-after-delete-rejected, rum.update-app-monitor-unknown-id-rejected, rum.update-app-monitor-on-deleted-resource-blocked.

=== Cause 3 of 7: CreateAppMonitor does not enforce the Domain pattern — a malformed Domain is accepted with HTTP 200 ===
AWS: Smithy: CreateAppMonitor.Domain targets AppMonitorDomain with @length min 1 max 253 and @pattern ^(localhost)$|^((25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\.){3}(25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)$|(?=^[a-zA-Z0-9\.\*-]{4,253}$)(?!.*\.-)(?!.*-\.)(?!.*\.\.)(?!.*[^\.]{64,})^(\*\.)?(?![-\.\*])[^\*]{1,}\.(\*|(?!.*--)(?=.*[a-zA-Z])[^\*]{1,}[^\*-])$ ; CreateAppMonitor models ValidationException (httpError 400). A value containing a protocol prefix and slashes (e.g. "http://example.com") fails that pattern, so AWS rejects the call with ValidationException / HTTP 400 and creates nothing. The recorded message follows the standard constraint-violation rule, built from the request: "1 validation error detected: Value '<value>' at 'domain' failed to satisfy constraint: Member must satisfy regular expression pattern: <the modeled pattern>".
Emulator: accepts the call, returns HTTP 200 with a generated Id, and the app monitor exists afterwards.
Why: an overly permissive data path. Code that relies on AWS rejecting a malformed domain silently succeeds against the emulator and leaves a resource behind that real AWS would never create; a later GetAppMonitor then returns a monitor instead of ResourceNotFoundException, so negative tests pass for the wrong reason.
Fix: input validation in the CreateAppMonitor handler — apply the modeled @pattern/@length constraints on Domain (and the same constraints on UpdateAppMonitor.Domain, which shares the shape) before any state mutation.
Traces: rum.create-app-monitor-malformed-domain-rejected.

=== Cause 4 of 7: PutRumEvents against an unresolvable AppMonitorId returns ResourceNotFoundException instead of AccessDeniedException ===
AWS: Smithy: PutRumEvents models both AccessDeniedException (httpError 403) and ResourceNotFoundException (httpError 404); Id is an @httpLabel AppMonitorId (@length 36, UUID @pattern). Recorded behavior: for a syntactically valid but never-issued app-monitor Id — and likewise for the Id of an app monitor that has since been deleted — RUM refuses with AccessDeniedException / HTTP 403, because it cannot authorize a write against an app monitor it cannot resolve; it does not disclose non-existence. The recorded message is "Caller principal does not have permissions for that resource." (a fixed, non-request-derived string — treat as an example, the requirement is the code and status).
Emulator: returns ResourceNotFoundException (HTTP 404).
Why: wrong status class and wrong error code. Clients that distinguish authorization failures from missing resources — RUM web-client retry/backoff logic in particular — take the wrong branch, and the emulator leaks existence information that AWS deliberately hides.
Fix: the PutRumEvents handler's app-monitor resolution path — unresolvable Id must map to AccessDeniedException/403, not ResourceNotFoundException/404.
Traces: rum.put-rum-events-unknown-app-monitor-rejected, rum.e2e-put-rum-events-after-delete-rejected.

=== Cause 5 of 7: DeleteResourcePolicy is a stub — returns NotImplemented instead of PolicyNotFoundException ===
AWS: Smithy: DeleteResourcePolicy (DELETE /appmonitor/{Name}/policy) models PolicyNotFoundException (httpError 404), InvalidPolicyRevisionIdException (400), ResourceNotFoundException (404), ConflictException (409), ValidationException (400) and returns PolicyRevisionId on success. Deleting when the app monitor exists but carries no resource policy is refused with PolicyNotFoundException / HTTP 404.
Emulator: every call fails with error code "NotImplemented" and message "operation DeleteResourcePolicy is scaffolded but not implemented in this emulator".
Why: the operation is unusable and no modeled error is reachable; callers see an unmodeled error code their SDK cannot classify, instead of the 404 PolicyNotFoundException they handle.
Fix: implement the DeleteResourcePolicy handler (services/service-com-amazonaws-rum-rum/src/ops/delete_resource_policy.rs) over the app-monitor resource-policy state, including the PolicyNotFoundException path.
Traces: rum.delete-resource-policy-nonexistent-rejected, rum.delete-resource-policy-without-existing-conflict.

=== Cause 6 of 7: GetResourcePolicy is a stub — returns NotImplemented instead of PolicyNotFoundException ===
AWS: Smithy: GetResourcePolicy (GET /appmonitor/{Name}/policy, @readonly) models PolicyNotFoundException (httpError 404) alongside ResourceNotFoundException (404) and ValidationException (400), and returns PolicyDocument + PolicyRevisionId. Reading the policy of an app monitor that has none is refused with PolicyNotFoundException / HTTP 404.
Emulator: error code "NotImplemented" ("operation GetResourcePolicy is scaffolded but not implemented in this emulator").
Why: same as cause 5 — an unmodeled code instead of the modeled 404, so policy-presence probing is impossible.
Fix: implement the GetResourcePolicy handler (services/service-com-amazonaws-rum-rum/src/ops/get_resource_policy.rs), including the PolicyNotFoundException path.
Traces: rum.get-resource-policy-missing-policy-conflict.

=== Cause 7 of 7: PutResourcePolicy does not reject a syntactically invalid PolicyDocument with MalformedPolicyDocumentException ===
AWS: Smithy: PutResourcePolicy (PUT /appmonitor/{Name}/policy) models MalformedPolicyDocumentException (httpError 400), PolicySizeLimitExceededException (400), InvalidPolicyRevisionIdException (400), ResourceNotFoundException (404), ConflictException (409), ValidationException (400). A PolicyDocument that is not parseable/valid IAM policy JSON is refused with MalformedPolicyDocumentException / HTTP 400 and no policy is stored.
Emulator: error code "NotImplemented" — the operation is scaffolded. NOTE: an issue is already filed for the bare PutResourcePolicy scaffold stub (reported in this suite as blocking 14 checks, services/service-com-amazonaws-rum-rum/src/ops/put_resource_policy.rs); this cause records the additional requirement that this check grades and that a minimal happy-path implementation would still miss — the malformed-document validation branch. Close it together with that issue.
Why: without the 400 MalformedPolicyDocumentException branch, a caller can store a policy document AWS would reject, and the emulator never produces the error code policy-management tooling handles.
Fix: validation in the PutResourcePolicy handler — parse/validate PolicyDocument and map failures to MalformedPolicyDocumentException/400 before storing.
Traces: rum.put-resource-policy-malformed-document-rejected.

## Evidence
Behavioral suite: rum 2/77 passed (3%). Per-cause expected vs actual, verbatim from replay:

Cause 1 — AssertionError: expected message 'Resource sim2real-rum-am-6f6c2e9a9951 already exists', got 'App monitor sim2real-rum-am-6f6c2e9a9951 already exists.' (same shape in ...-blocked, ...-rejected, and duplicate-app-monitor-name-conflict-rejected). Code/status correct: ConflictException/409.

Cause 2 — AssertionError: expected message 'Resource not found sim2real-rum-victim-1f9fb06084c9', got 'App monitor sim2real-rum-victim-1f9fb06084c9 does not exist.'; same for 'Resource not found sim2real-behav-appmon-ab08518e095c', 'Resource not found sim2real-rum-delstate-6cf0da8c18c5', 'Resource not found sim2real-rum-missing-487c003b120f', 'Resource not found sim2real-rum-delupd-d67e7afd1a65'. Code/status correct: ResourceNotFoundException/404.

Cause 3 — CreateAppMonitor(Name=<unique>, Domain='http://example.com'): expected ValidationException/400; actual AssertionError: expected a ClientError, but the call succeeded with {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'Id': '2bb2fd20-25f9-4e05-8f51-ba75c0341c46'}.

Cause 4 — PutRumEvents with a fresh uuid4 Id: expected AccessDeniedException/403; actual AssertionError: 'ResourceNotFoundException'. Same after DeleteAppMonitor: AssertionError: expected AccessDeniedException, got 'ResourceNotFoundException'.

Cause 5 — DeleteResourcePolicy on a monitor with no policy: expected PolicyNotFoundException/404; actual AssertionError: expected PolicyNotFoundException, got NotImplemented (and ... got 'NotImplemented').

Cause 6 — GetResourcePolicy on a monitor with no policy: expected PolicyNotFoundException/404; actual AssertionError: expected PolicyNotFoundException, got NotImplemented.

Cause 7 — PutResourcePolicy with an invalid PolicyDocument: expected MalformedPolicyDocumentException/400; actual AssertionError: a syntactically invalid PolicyDocument should be refused with MalformedPolicyDocumentException, got 'NotImplemented'.

## Steps to Reproduce
Boot the emulator and point a botocore rum client at it.
1) Cause 1: rum.create_app_monitor(Name='dup1', Domain='example.com'); call it again with the same Name -> observe ConflictException message 'App monitor dup1 already exists.' instead of 'Resource dup1 already exists'.
2) Cause 2: rum.get_app_monitor(Name='never-created') (and update_app_monitor / delete_app_monitor with the same name) -> ResourceNotFoundException message 'App monitor never-created does not exist.' instead of 'Resource not found never-created'.
3) Cause 3: rum.create_app_monitor(Name='bad', Domain='http://example.com') -> HTTP 200 with an Id; expected ValidationException/400. Then rum.get_app_monitor(Name='bad') returns the monitor instead of ResourceNotFoundException.
4) Cause 4: rum.put_rum_events(Id=str(uuid4()), BatchId=str(uuid4()), AppMonitorDetails={}, UserDetails={...}, RumEvents=[{...}]) -> ResourceNotFoundException/404; expected AccessDeniedException/403. Repeat with the Id of a monitor that was just deleted — same divergence.
5) Cause 5: create an app monitor, then rum.delete_resource_policy(Name=<name>) -> NotImplemented; expected PolicyNotFoundException/404.
6) Cause 6: rum.get_resource_policy(Name=<name>) on that same monitor -> NotImplemented; expected PolicyNotFoundException/404.
7) Cause 7: rum.put_resource_policy(Name=<name>, PolicyDocument='{not valid json') -> NotImplemented; expected MalformedPolicyDocumentException/400.
Or replay the checks listed in trace_ids.
