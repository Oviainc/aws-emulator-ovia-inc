Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: compute-optimizer.get-idle-recommendations-max-results-over-limit-rejected compute-optimizer.recommendation-preferences-preferred-resources-lifecycle compute-optimizer.update-enrollment-status-include-member-accounts-non-management-rejected
Issue-Shape-Id: com.amazonaws.computeoptimizer#ComputeOptimizerService
Issue-Key: compute-optimizer.get-idle-recommendations-max-results-over-limit-rejected|run#4|AssertionError|assert code == "InvalidParameterValueException", (
Issue-Key: compute-optimizer.recommendation-preferences-preferred-resources-lifecycle|run#16|AssertionError|assert "effectiveIncludeList" in entry, f"expected a resolved effectiveIncludeList, got entry={entry!r}"
Issue-Key: compute-optimizer.update-enrollment-status-include-member-accounts-non-management-rejected|_error#3|AssertionError|raise AssertionError(
Issue-Commit: 1bf7f94f5369037d1be2743fe86a59b83a69434a

# compute-optimizer: wrong rejection class for out-of-range maxResults, effectiveIncludeList never resolved, and includeMemberAccounts accepted from a non-management account

> _Note: this issue was identified against the code base at commit `1bf7f94f5369037d1be2743fe86a59b83a69434a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetIdleRecommendations

## Description
Three independent fidelity defects in implemented compute-optimizer surfaces: a constraint violation is refused with the wrong (unmodeled) error class, a read-back of a recommendation preference omits a modeled resolved member, and an organization-only input flag is accepted from an account that may not use it. THREE causes, all on compute-optimizer; all three must be closed before submitting.

Cause 1 of 3: GetIdleRecommendations refuses maxResults=101 with ValidationException instead of InvalidParameterValueException.
  AWS: Smithy: GetIdleRecommendationsRequest.maxResults targets IdleMaxResults with @range min 0, max 100, so 101 violates the modeled constraint. The only client error the operation models for a bad input value is InvalidParameterValueException (httpError 400, "The value supplied for the input parameter is out of range or not valid"); ValidationException is NOT in GetIdleRecommendations' modeled error set (AccessDeniedException 403, InternalServerException 500, InvalidParameterValueException 400, MissingAuthenticationToken 403, OptInRequiredException 403, ResourceNotFoundException 404, ServiceUnavailableException 503, ThrottlingException 429). Recorded behavior: error code `InvalidParameterValueException`; recorded message "Invalid Max Results." as an example only.
  Emulator: refuses the call with error code `ValidationException` — the centralized pre-handler constraint check's generic rejection class, not this service's modeled one.
  Why: a client catching the documented InvalidParameterValueException for a paging-size mistake sees an error code its SDK models for a different family; botocore raises a ClientError whose Code does not match the operation's error shapes, so typed error handling and retry/branch logic on InvalidParameterValueException never fires. The same mapping applies to every modeled @range/@length violation in this service, so the blast radius is service-wide, not just this one member.
  Fix: the error mapping for modeled-constraint violations in the compute-optimizer service crate — the pre-handler validation path must emit the service's modeled InvalidParameterValueException (400) rather than the generic ValidationException. (Related but distinct: GetIdleRecommendations' own handler is still a scaffold stub and has its own filed issue; this cause is about the rejection class produced BEFORE the handler runs and is not closed by implementing the handler.)
  Traces: compute-optimizer.get-idle-recommendations-max-results-over-limit-rejected.

Cause 2 of 3: GetRecommendationPreferences returns a preferredResources entry with no effectiveIncludeList.
  AWS: Smithy: RecommendationPreferencesDetail.preferredResources targets EffectivePreferredResources, whose element EffectivePreferredResource has members name, includeList, effectiveIncludeList ("The expanded version of your preferred resource's include list") and excludeList. Recorded behavior: after PutRecommendationPreferences(resourceType=Ec2Instance, scope={name:AccountId,value:<account id>}, preferredResources=[{name:Ec2InstanceTypes, excludeList:[<exact instance type>]}]), the read-back detail's preferredResources element carries a resolved effectiveIncludeList — the expanded candidate set with the excluded exact instance type absent and sibling sizes in the same family still present. Status 200 throughout.
  Emulator: answers 200 but the entry echoes only what was written: {'name': 'Ec2InstanceTypes', 'excludeList': ['m6g.large']} — `effectiveIncludeList` is absent entirely, so the exclusion's effect is unobservable.
  Why: the whole point of the EffectivePreferredResource read model is that the server resolves include/exclude lists into the effective candidate set. A client that reads effectiveIncludeList to confirm an exclusion took effect, or to render which instance types remain candidates, gets a missing member and either crashes on the absent key or reports the preference as having no effect.
  Fix: the recommendation-preference state and the GetRecommendationPreferences handler of the compute-optimizer service crate — on read, expand the stored PreferredResource include/exclude lists into EffectivePreferredResource.effectiveIncludeList rather than echoing the stored request members.
  Traces: compute-optimizer.recommendation-preferences-preferred-resources-lifecycle.

Cause 3 of 3: UpdateEnrollmentStatus accepts includeMemberAccounts=true from a non-management account.
  AWS: Smithy: UpdateEnrollmentStatusRequest has required `status` (Status enum) and `includeMemberAccounts` (boolean, default false); the operation models InvalidParameterValueException (httpError 400) as its bad-input client error. Recorded behavior: includeMemberAccounts=true is only legal for an organization's management (or delegated-admin) account; from a standalone/non-management account the call is refused with error code `InvalidParameterValueException` and the account's own enrollment status is left unchanged. Recorded message, as an example only: "Cannot include member accounts. This could be because you are not an AWS Organization user or you are not the master account in the organization."
  Emulator: the call SUCCEEDS — HTTP 200 with body {"status": "Active", "statusReason": "The account is opted in to Compute Optimizer."} — so an organization-wide enrollment request from an account that may not make one is silently honored.
  Why: this is an over-permissive acceptance: a client (or IaC/onboarding script) that wrongly asks to enroll member accounts gets a 200 and believes an organization-wide opt-in happened. Code that handles the documented 400 to fall back to an account-only opt-in never takes that branch, and the emulator cannot be used to test the management-account boundary at all.
  Fix: the UpdateEnrollmentStatus handler's input validation in the compute-optimizer service crate — reject includeMemberAccounts=true unless the caller is the organization management/delegated-admin account, with the modeled InvalidParameterValueException (400), and leave the caller's own enrollment status unmutated on rejection.
  Traces: compute-optimizer.update-enrollment-status-include-member-accounts-non-management-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 against this checkout; compute-optimizer 4/25 passed.

Cause 1 — check compute-optimizer.get-idle-recommendations-max-results-over-limit-rejected [mismatch]:
  call: GetIdleRecommendations(maxResults=101)
  expected: error code InvalidParameterValueException (HTTP 400) — the only bad-value client error the operation models
  actual:   AssertionError: expected InvalidParameterValueException for maxResults=101, got 'ValidationException'
  model: GetIdleRecommendationsRequest.maxResults -> IdleMaxResults, range {min: 0, max: 100}; errors = [AccessDeniedException 403, InternalServerException 500, InvalidParameterValueException 400, MissingAuthenticationToken 403, OptInRequiredException 403, ResourceNotFoundException 404, ServiceUnavailableException 503, ThrottlingException 429] — ValidationException is not among them.

Cause 2 — check compute-optimizer.recommendation-preferences-preferred-resources-lifecycle [mismatch]:
  sequence: PutRecommendationPreferences(resourceType=Ec2Instance, scope={name:AccountId,value:<account id>}, preferredResources=[{name:Ec2InstanceTypes, excludeList:['m6g.large']}]) then GetRecommendationPreferences(resourceType=Ec2Instance, scope=same)
  expected: the detail's preferredResources entry carries a resolved effectiveIncludeList (excluded exact type absent, sibling sizes in the family present)
  actual:   AssertionError: expected a resolved effectiveIncludeList, got entry={'name': 'Ec2InstanceTypes', 'excludeList': ['m6g.large']}
  model: RecommendationPreferencesDetail.preferredResources -> EffectivePreferredResources; EffectivePreferredResource members {name, includeList, effectiveIncludeList, excludeList}.

Cause 3 — check compute-optimizer.update-enrollment-status-include-member-accounts-non-management-rejected [mismatch]:
  call: UpdateEnrollmentStatus(status='Active', includeMemberAccounts=True) from non-management credentials
  expected: error code InvalidParameterValueException (HTTP 400), enrollment status unchanged
  actual:   AssertionError: real AWS rejects includeMemberAccounts=True from a non-management account, but the call SUCCEEDED and returned {'status': 'Active', 'statusReason': 'The account is opted in to Compute Optimizer.'} with HTTPStatusCode 200
  model: UpdateEnrollmentStatusRequest {status (required, Status enum), includeMemberAccounts (boolean, default false)}; errors include InvalidParameterValueException httpError 400.

All three were replayed against a live emulator (other compute-optimizer checks in the same run passed, so the service is serving).

Related open issue, which does NOT cover these: 20261023T034444Z-compute-optimizer-rdsdbinstance-recommendation-preferences-rejected-and-never-persisted.md (PutRecommendationPreferences resource-type allowlist for RdsDBInstance and the empty recommendationPreferencesDetails read-back). Cause 2 here is a different defect: the Ec2Instance account-scoped preference IS persisted and read back, but its EffectivePreferredResource.effectiveIncludeList is never resolved.

## Steps to Reproduce
1. Boot the emulator from this checkout and run the behavioral suite simparity-behavioral-aws-v1 (service compute-optimizer).
2. Cause 1: replay check compute-optimizer.get-idle-recommendations-max-results-over-limit-rejected — opt the account in (UpdateEnrollmentStatus status=Active), then call GetIdleRecommendations(maxResults=101). Expect a 400 whose error code is InvalidParameterValueException; observe error code ValidationException. Then call GetIdleRecommendations(maxResults=100) and expect a 200 whose idleRecommendations is a list.
3. Cause 2: replay check compute-optimizer.recommendation-preferences-preferred-resources-lifecycle — call PutRecommendationPreferences(resourceType='Ec2Instance', scope={'name':'AccountId','value':<caller account id>}, preferredResources=[{'name':'Ec2InstanceTypes','excludeList':['m6g.large']}]), then GetRecommendationPreferences(resourceType='Ec2Instance', scope=same) and inspect the detail's preferredResources[0]. Expect an effectiveIncludeList that omits the excluded exact instance type while retaining sibling sizes of the same family; observe the entry with only name+excludeList and no effectiveIncludeList. Then DeleteRecommendationPreferences(resourceType='Ec2Instance', scope=same, recommendationPreferenceNames=['PreferredResources']) and confirm removal.
4. Cause 3: replay check compute-optimizer.update-enrollment-status-include-member-accounts-non-management-rejected — with non-management-account credentials, call UpdateEnrollmentStatus(status='Active', includeMemberAccounts=True). Expect a 400 with error code InvalidParameterValueException and GetEnrollmentStatus still reporting the pre-call status; observe HTTP 200 with status=Active.
5. All three causes must pass before closing: re-replay compute-optimizer.get-idle-recommendations-max-results-over-limit-rejected, compute-optimizer.recommendation-preferences-preferred-resources-lifecycle and compute-optimizer.update-enrollment-status-include-member-accounts-non-management-rejected.
