Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: customer-profiles.merge-profiles-self-reference-rejected customer-profiles.put-profile-object-type-template-and-fields-conflict-rejected
Issue-Shape-Id: com.amazonaws.customerprofiles#CustomerProfiles_20200815
Issue-Key: customer-profiles.merge-profiles-self-reference-rejected|run#7|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got {code!r}"
Issue-Key: customer-profiles.put-profile-object-type-template-and-fields-conflict-rejected|run#7|AssertionError|assert code == "BadRequestException", f"expected BadRequestException for TemplateId+Fields/Keys conflict, got {code!r}"
Issue-Commit: 2f933b71caa2a46f81d276f12cbb60da2946d084

# customer-profiles: 2 net-new root causes — MergeProfiles never refuses a self-referencing merge and PutProfileObjectType never refuses TemplateId mixed with Fields/Keys; both answer NotImplemented

> _Note: this issue was identified against the code base at commit `2f933b71caa2a46f81d276f12cbb60da2946d084`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
MergeProfiles

## Description
Two net-new rejection root causes in the customer-profiles crate. Both are validation gaps on the REFUSAL path of a request real AWS rejects with BadRequestException (400); in this emulator the request instead reaches an unimplemented handler and the caller is answered with the emulator's own `NotImplemented` error. Related scaffold-stub issues for `MergeProfiles` and `PutProfileObjectType` are filed in this same batch from the checkout (they cover the operations' HAPPY paths, reported against customer-profiles.merge-profiles-deep-history-and-deletion and customer-profiles.batch-put-profile-object-partial-failure); neither carries the replay key of the two checks below, so the input-validation contracts named here must be closed in addition to standing the operations up.

Cause 1 of 2: MergeProfiles does not refuse a merge whose ProfileIdsToBeMerged contains the MainProfileId itself.
AWS: Smithy: MergeProfiles (POST /domains/{DomainName}/profiles/objects/merge) models BadRequestException with httpError 400; MainProfileId is a required `uuid` (pattern ^[a-f0-9]{32}$) and ProfileIdsToBeMerged a required list of 1..20 of the same. Recorded behavior: a request naming the main profile id inside ProfileIdsToBeMerged is refused with HTTP 400, error code `BadRequestException`, message `Request contains duplicated profile ID` — that message is entirely fixed text for this error class, it interpolates no request value. The rejected call must leave state untouched (the main profile remains readable), and the same operation must still succeed when ProfileIdsToBeMerged names a genuinely different profile.
Emulator: the call is answered with error code `NotImplemented`, message `operation MergeProfiles is scaffolded but not implemented in this emulator` — the duplicate-id check does not exist, so the refusal AWS makes is unreachable.
Why: a client that relies on AWS refusing a self-merge gets no BadRequestException to branch on; it sees an unmodelled `NotImplemented` code that is not in the operation's error set, so error handling written against the API cannot classify it, and a deduplicating caller that depends on AWS to catch the degenerate case gets no feedback at all.
Fix: belongs in the MergeProfiles handler's input validation (services/service-com-amazonaws-customerprofiles-customerprofiles-20200815/src/ops/merge_profiles.rs), ahead of any state mutation: reject with BadRequestException/400 and the fixed message above when MainProfileId appears in ProfileIdsToBeMerged (and, generally, when ProfileIdsToBeMerged repeats an id), leaving the store unchanged.
Traces: customer-profiles.merge-profiles-self-reference-rejected (mismatch: `expected BadRequestException, got 'NotImplemented'`).

Cause 2 of 2: PutProfileObjectType does not refuse a request that mixes TemplateId with Fields/Keys.
AWS: Smithy: PutProfileObjectType (PUT /domains/{DomainName}/object-types/{ObjectTypeName}) models BadRequestException with httpError 400, and the model's own documentation for the TemplateId member states that when TemplateId is present the service uses the template's defaults and "if these attributes are present in the request, the service may return a BadRequestException. These attributes include: AllowProfileCreation, SourceLastUpdatedTimestampFormat, Fields, and Keys." So a request carrying TemplateId together with Fields and Keys is refused with HTTP 400 and error code `BadRequestException` (message text: recorded behavior only — the check asserts the code, so reproduce AWS's wording for the conflict class, with the conflicting member names taken from the request rather than hard-coded). Two further parts of the contract: the refused request must NOT create the object type (a following GetProfileObjectType for that ObjectTypeName must answer ResourceNotFoundException), and TemplateId ALONE must succeed, the 200 echoing ObjectTypeName and GetProfileObjectType then reporting the same TemplateId.
Emulator: the conflicting request is answered with error code `NotImplemented`, message `operation PutProfileObjectType is scaffolded but not implemented in this emulator`; the mutually-exclusive-parameter check does not exist, and the template-only success path and its TemplateId round-trip are equally unreachable.
Why: a client that mistakenly sends both forms of configuration is told nothing about the conflict and cannot distinguish "your request is contradictory" from "this operation is unavailable"; conversely a client doing template-based onboarding cannot create the object type at all, so the whole template path silently fails in the emulator while working against AWS.
Fix: belongs in the PutProfileObjectType handler's input validation and its state write (services/service-com-amazonaws-customerprofiles-customerprofiles-20200815/src/ops/put_profile_object_type.rs): validate TemplateId's mutual exclusivity with Fields/Keys (and the other template-defaulted members the model names) before persisting anything, refuse with BadRequestException/400, and persist/serve TemplateId for the template-only request.
Traces: customer-profiles.put-profile-object-type-template-and-fields-conflict-rejected (mismatch: `expected BadRequestException for TemplateId+Fields/Keys conflict, got 'NotImplemented'`).

## Evidence
Suite: simparity-behavioral-aws-v1, 1/22 passed; both failures below are [mismatch] verdicts, i.e. the emulator answered and answered wrongly.

Cause 1 — customer-profiles.merge-profiles-self-reference-rejected:
  expected: HTTP 400, Error.Code = "BadRequestException", Error.Message = "Request contains duplicated profile ID" (fixed text), main profile still readable afterwards, and a subsequent merge of a DIFFERENT profile succeeds.
  actual:   Error.Code = "NotImplemented", Error.Message = "operation MergeProfiles is scaffolded but not implemented in this emulator".
  assertion: `AssertionError: expected BadRequestException, got 'NotImplemented'` (check_customer_profiles_merge_profiles_self_reference_rejected.py line 33).
  request shape: merge_profiles(DomainName=<domain>, MainProfileId=<id>, ProfileIdsToBeMerged=[<same id>]).

Cause 2 — customer-profiles.put-profile-object-type-template-and-fields-conflict-rejected:
  expected: HTTP 400, Error.Code = "BadRequestException" for a request carrying TemplateId plus Fields plus Keys; then GetProfileObjectType for that name answers ResourceNotFoundException (no partial create); then the same Put with TemplateId only succeeds and GetProfileObjectType reports that TemplateId.
  actual:   Error.Code = "NotImplemented", Error.Message = "operation PutProfileObjectType is scaffolded but not implemented in this emulator".
  assertion: `AssertionError: expected BadRequestException for TemplateId+Fields/Keys conflict, got 'NotImplemented'` (check_customer_profiles_put_profile_object_type_template_and_fields_conflict_rejected.py line 34).
  model citation: PutProfileObjectTypeRequest$TemplateId documentation names AllowProfileCreation, SourceLastUpdatedTimestampFormat, Fields and Keys as the conflicting attributes and BadRequestException as the result; PutProfileObjectType errors = AccessDeniedException(403), BadRequestException(400), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429) — `NotImplemented` is not among them. MergeProfiles errors = BadRequestException(400), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429).

## Steps to Reproduce
Cause 1 (customer-profiles.merge-profiles-self-reference-rejected):
1. CreateDomain(DomainName=<unique>, DefaultExpirationDays=30).
2. CreateProfile(DomainName=<domain>, FirstName="Solo") -> ProfileId P.
3. MergeProfiles(DomainName=<domain>, MainProfileId=P, ProfileIdsToBeMerged=[P]).
   Expect HTTP 400 BadRequestException with message "Request contains duplicated profile ID"; observe Error.Code "NotImplemented".
4. BatchGetProfile(DomainName=<domain>, ProfileIds=[P]) must still return P (no state change from the refused merge).
5. CreateProfile a second profile Q and MergeProfiles(MainProfileId=P, ProfileIdsToBeMerged=[Q]) must succeed and Q must stop being returned by BatchGetProfile.

Cause 2 (customer-profiles.put-profile-object-type-template-and-fields-conflict-rejected):
1. CreateDomain(DomainName=<unique>, DefaultExpirationDays=30).
2. PutProfileObjectType(DomainName=<domain>, ObjectTypeName="CTR", Description=..., TemplateId="CTR-NoInferred", Fields={"phone": {ContentType: PHONE_NUMBER, Source: "_source.phone", Target: "_profile.PhoneNumber"}}, Keys={"phoneKey": [{StandardIdentifiers: ["PROFILE"], FieldNames: ["phone"]}]}).
   Expect HTTP 400 BadRequestException; observe Error.Code "NotImplemented".
3. GetProfileObjectType(DomainName=<domain>, ObjectTypeName="CTR") must answer ResourceNotFoundException (the refused request created nothing).
4. PutProfileObjectType(DomainName=<domain>, ObjectTypeName="CTR", Description="CTR", TemplateId="CTR-NoInferred") must succeed with ObjectTypeName="CTR"; GetProfileObjectType must then report TemplateId="CTR-NoInferred".
