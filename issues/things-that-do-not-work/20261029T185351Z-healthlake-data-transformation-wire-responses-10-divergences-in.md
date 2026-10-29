Issue-Source: trace-sweep-divergence
Issue-Trace: healthlake-data-transformation-job-errors-and-list
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: healthlake-data-transformation-job-errors-and-list healthlake-data-transformation-profile-errors healthlake-data-transformation-profile-lifecycle healthlake-resource-tagging-errors
Issue-Shape-Id: com.amazonaws.healthlake#HealthLake
Issue-Key: healthlake|DescribeDataTransformationJob|4xx->4xx|$.Message
Issue-Key: healthlake|DescribeDataTransformationJob|4xx->4xx|$.message
Issue-Key: healthlake|CreateDataTransformationProfile|4xx->4xx|$.message
Issue-Key: healthlake|GetDataTransformationProfile|4xx->4xx|$.Message
Issue-Key: healthlake|UpdateDataTransformationProfile|4xx->4xx|$.Message
Issue-Key: healthlake|ListDataTransformationProfileVersions|4xx->4xx|$.Message
Issue-Key: healthlake|ListDataTransformationProfiles|4xx->4xx|$.message
Issue-Key: healthlake|CreateDataTransformationProfile|2xx->2xx|$.ProfileStatus
Issue-Key: healthlake|CreateDataTransformationProfile|2xx->2xx|status
Issue-Key: healthlake|GetDataTransformationProfile|2xx->4xx|$.LastUpdatedAt
Issue-Key: healthlake|GetDataTransformationProfile|2xx->4xx|$.ProfileId
Issue-Key: healthlake|GetDataTransformationProfile|2xx->4xx|$.ProfileMapping
Issue-Key: healthlake|GetDataTransformationProfile|2xx->4xx|$.ProfileName
Issue-Key: healthlake|GetDataTransformationProfile|2xx->4xx|$.SourceFormat
Issue-Key: healthlake|GetDataTransformationProfile|2xx->4xx|$.TargetFormat
Issue-Key: healthlake|GetDataTransformationProfile|2xx->4xx|$.Version
Issue-Key: healthlake|GetDataTransformationProfile|2xx->4xx|status
Issue-Key: healthlake|ListTagsForResource|4xx->4xx|$.Message
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# HealthLake data-transformation wire responses: 10 divergences in refusal wording, ValidationException message member, and the create/read profile replies

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDataTransformationJob

## Description
TEN distinct net-new root causes in the HealthLake data-transformation wire surface, all found by replaying the wire fixture corpus simparity-wire-aws-v1 (6 fixtures, 31 steps, 12 mismatched, score 61.3%). They all share the service token `healthlake` and are bundled here; each cause is numbered below and carries its own fixture and step. A fixer must close ALL TEN before submitting.

HealthLake is awsJson1_0 (service_info: protocols [awsJson1_0]), so every @http trait on these operations is INERT on the wire: dispatch is by X-Amz-Target and the success status is 200 regardless of the modelled @http code. Three groups of fault recur: (a) refusal MESSAGES are fixed emulator constants that never quote the request, (b) boundary-validation refusals are not rendered into the lowercase `message` member AWS uses for them, and (c) the create/read profile replies diverge in status and in membership.

No open healthlake issue covers these wire steps (the replay reports `owned_by: []` for all ten). The nearest open issue is 20261029T182942Z-healthlake-data-transformation-profile-surface-7-divergences-fixed.md, which was filed from the BEHAVIORAL corpus; its Cause 1 (omitted-version DRAFT fallback) and Cause 2 (GetDataTransformationProfile not-found wording) touch the same handlers as Causes 4 and 9 here but carry none of these fixture/step replay keys, so these wire steps are not re-run by it. Other open healthlake issues checked and not covering these: 20261022T185152Z-healthlake-startfhirimportjob-stub.md, 20261024T000307Z-healthlake-createdatatransformationprofile-wrong-behavior.md, 20261024T000307Z-healthlake-transformdata-missing.md, 20261024T000307Z-healthlake-updateprofilewithagent-missing.md, 20261028T035414Z-healthlake-startdatatransformationjob-wrong-behavior.md, 20261029T115812Z-healthlake-tagresource-missing.md, 20261029T160047Z-healthlake-createdatatransformationprofile-validation.md, 20261029T165933Z-healthlake-deletefhirdatastore-maintainability.md, 20261029T172219Z-healthlake-createfhirdatastore-wrong-behavior--r2.md.

=== Cause 1 of 10: DescribeDataTransformationJob's job-not-found message is a fixed constant that does not name the job ===
Fixture/step: healthlake-data-transformation-job-errors-and-list#task#1
AWS: Smithy: DescribeDataTransformationJob.JobId is required, @length 1..32, @pattern ^[a-zA-Z0-9-]+$; the operation models ResourceNotFoundException. The model documents no message text, so the message is recorded behavior only: for a JobId no record holds, the recorded response is HTTP 400 with the error's `Message` member reading "Transformation job not found: <JobId>" — the fixed text is "Transformation job not found: " and the only variable part is the JobId taken verbatim from the request.
Emulator: answers HTTP 400 (status is right) but `$.Message` is the fixed constant "Could not find specified data transformation job." — crate::state::TRANSFORMATION_JOB_NOT_FOUND_MESSAGE, which is documented as deliberately quoting nothing from the request. Every job not-found path in the crate states it through that one constant.
Why: a client that logs or parses the refusal cannot tell WHICH job was missing, and any assertion against AWS's wording fails. In a fan-out that describes several jobs, every failure reads identically.
Fix: the not-found refusal in DescribeDataTransformationJob's handler and the shared message constant it reads — build the sentence from the requested JobId instead of serving a constant.
Traces: healthlake-data-transformation-job-errors-and-list#task#1.

=== Cause 2 of 10: DescribeDataTransformationJob's JobId pattern violation carries no `message` member ===
Fixture/step: healthlake-data-transformation-job-errors-and-list#task#2
AWS: Smithy: DescribeDataTransformationJob.JobId @pattern ^[a-zA-Z0-9-]+$, @length 1..32; ValidationException (httpError 400) is the service's common error and its modelled member is `Message`. The RECORDED boundary-validation refusal is HTTP 400 whose body carries the text in the LOWERCASE `message` member: "1 validation error detected: Value at 'jobId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9-]+". Fixed text: "1 validation error detected: Value at '", "' failed to satisfy constraint: Member must satisfy regular expression pattern: ". Variable parts: the member path, rendered lower-camel as it appears on the wire ('jobId'), and the pattern, rendered WITHOUT the ^...$ anchors the model carries. The rejected value is NOT echoed for this member.
Emulator: the 400 body has no `message` member at all ($.message missing, got None), so a client reading the framework-shaped validation body finds nothing.
Why: boundary-validation failures are unreadable to any client or SDK path that reads the lowercase `message` member AWS populates for them; the caller learns neither which member nor which constraint failed.
Fix: this crate's boundary-validation serializer for ValidationException — render the constraint sentence into the `message` member that AWS's validation refusals use, with the lower-camel member path and the unanchored pattern.
Traces: healthlake-data-transformation-job-errors-and-list#task#2.

=== Cause 3 of 10: CreateDataTransformationProfile's SourceFormat enum violation carries no `message` member, and the enum set is wrong ===
Fixture/step: healthlake-data-transformation-profile-errors#task#1
AWS: Smithy: CreateDataTransformationProfile.SourceFormat is required and targets the SourceFormat enum; ValidationException (400). The model's enum `values` are [CCDA, CSV], but the RECORDED refusal shows the served value set is larger: HTTP 400 with the lowercase `message` member reading "1 validation error detected: Value at 'sourceFormat' failed to satisfy constraint: Member must satisfy enum value set: [CSV, HL7V2, CCDA]". Fixed text: "1 validation error detected: Value at '", "' failed to satisfy constraint: Member must satisfy enum value set: [", "]". Variable parts: the wire member path ('sourceFormat') and the enum value list, which the recording renders in the order CSV, HL7V2, CCDA. The recorded set is the requirement here; the on-disk model lags it.
Emulator: the 400 body carries no `message` member ($.message missing, got None), so neither the constraint nor the accepted value set reaches the caller.
Why: a client that submits an unsupported SourceFormat cannot discover the accepted values from the refusal, and cannot distinguish an enum violation from any other 400.
Fix: CreateDataTransformationProfile's input validation plus the crate's ValidationException serializer — emit the enum-constraint sentence into the `message` member, listing the served value set (CSV, HL7V2, CCDA).
Traces: healthlake-data-transformation-profile-errors#task#1.

=== Cause 4 of 10: GetDataTransformationProfile's profile-not-found message is a fixed constant that does not name the profile ===
Fixture/step: healthlake-data-transformation-profile-errors#task#2 (cluster also covers healthlake-data-transformation-profile-lifecycle#task#8)
AWS: Smithy: GetDataTransformationProfile.ProfileId required, @length 32, @pattern ^[a-f0-9]{32}$; ResourceNotFoundException is modelled, with no documented text, so the message is recorded behavior only. For a ProfileId no record holds the recording answers HTTP 400 with `Message` = "Profile '<ProfileId>' not found" — fixed text "Profile '" and "' not found", the single variable part being the ProfileId from the request.
Emulator: HTTP 400 with `$.Message` = "Could not find specified profile." — crate::state::PROFILE_NOT_FOUND_MESSAGE, a constant shared by every profile not-found path.
Why: the refusal identifies no resource, so read-after-delete and misconfigured-id diagnostics are indistinguishable, and message assertions against AWS wording fail.
Fix: the not-found refusal in GetDataTransformationProfile's handler and the shared constant in state.rs that it reads — interpolate the requested ProfileId.
Traces: healthlake-data-transformation-profile-errors#task#2; healthlake-data-transformation-profile-lifecycle#task#8.

=== Cause 5 of 10: UpdateDataTransformationProfile's refusal for an unknown profile omits the profile and the version it resolved ===
Fixture/step: healthlake-data-transformation-profile-errors#task#3
AWS: Smithy: UpdateDataTransformationProfile.ProfileId required (@pattern ^[a-f0-9]{32}$) and ProfileMapping required (@sensitive, @length 1..500); ResourceNotFoundException modelled, text recorded behavior only. The recording answers HTTP 400 with `Message` = "Profile '<ProfileId>' version 0 not found" — fixed text "Profile '", "' version ", " not found". Variable parts: the request's ProfileId and the version the write resolved to (the DRAFT, 0, for an update).
Emulator: HTTP 400 with `$.Message` = "Could not find specified profile." — it reuses PROFILE_NOT_FOUND_MESSAGE, so the answer names neither the profile nor the version, and does not even distinguish this from the plain profile-not-found case.
Why: a caller cannot tell whether the profile or the targeted version was missing, nor which version the write would have edited.
Fix: the profile/version resolution refusal in UpdateDataTransformationProfile's handler — use the version-not-found wording with the requested ProfileId and the resolved version.
Traces: healthlake-data-transformation-profile-errors#task#3.

=== Cause 6 of 10: ListDataTransformationProfileVersions' profile-not-found message is the same fixed constant ===
Fixture/step: healthlake-data-transformation-profile-errors#task#4
AWS: Smithy: ListDataTransformationProfileVersions.ProfileId required (@pattern ^[a-f0-9]{32}$); ResourceNotFoundException modelled, text recorded behavior only. The recording answers HTTP 400 with `Message` = "Profile '<ProfileId>' not found" — same template as Cause 4, the ProfileId being the only variable part.
Emulator: HTTP 400 with `$.Message` = "Could not find specified profile."
Why: the paginated reader's refusal is indistinguishable from every other profile refusal, so a client paging several profiles cannot attribute the failure.
Fix: the not-found refusal in ListDataTransformationProfileVersions' handler (the shared state.rs constant it calls) — interpolate the requested ProfileId.
Traces: healthlake-data-transformation-profile-errors#task#4.

=== Cause 7 of 10: ListDataTransformationProfiles' SourceFormat enum violation carries no `message` member ===
Fixture/step: healthlake-data-transformation-profile-errors#task#5
AWS: Smithy: ListDataTransformationProfiles.SourceFormat is REQUIRED and targets the SourceFormat enum; ValidationException (400). The recorded refusal for an unsupported value is HTTP 400 with the lowercase `message` member reading "1 validation error detected: Value at 'sourceFormat' failed to satisfy constraint: Member must satisfy enum value set: [CSV, HL7V2, CCDA]" — same template and same served value set as Cause 3; the variable parts are the wire member path and the value list.
Emulator: the 400 body carries no `message` member ($.message missing, got None).
Why: the mandatory filter on this list operation can be refused with no readable reason, so a client cannot correct the call from the response.
Fix: ListDataTransformationProfiles' input validation plus the shared ValidationException serializer — render the enum-constraint sentence into `message`.
Traces: healthlake-data-transformation-profile-errors#task#5.

=== Cause 8 of 10: CreateDataTransformationProfile answers HTTP 201 and omits ProfileStatus ===
Fixture/step: healthlake-data-transformation-profile-lifecycle#task#1
AWS: Smithy: the operation's @http carries code 201, but HealthLake is awsJson1_0, so smithy_query reports the binding httpBindingsInert ("RPC protocol: @http/httpLabel/httpQuery/httpHeader are ignored on the wire"). The recorded success status is therefore HTTP 200. The recorded body additionally carries a `ProfileStatus` member whose value for a fresh profile is "ACTIVE"; CreateDataTransformationProfileResponse models only ProfileId, Version, SourceFormat, TargetFormat, ProfileName and LastUpdatedAt, so ProfileStatus is recorded behavior only — the service reports the new profile's status in its create reply.
Emulator: answers HTTP 201 (it applied the inert @http code as if this were a REST protocol) and the body has no ProfileStatus member at all.
Why: two separate breakages. A client that branches on the status code (==200, or any "is this exactly 200" check) treats a successful create as unexpected, and SDK/middleware layers that key success handling on the protocol's 200 can mis-route the reply. A client that reads the new profile's status from the create reply gets nothing and must issue an extra read, or sees an absent field where AWS always sends one.
Fix: CreateDataTransformationProfile's serializer/handler — return the awsJson1_0 success status (200, not the inert modelled 201) and include ProfileStatus in the create reply, sourced from the profile record the handler just inserted.
Traces: healthlake-data-transformation-profile-lifecycle#task#1.

=== Cause 9 of 10: a created-but-never-published profile is unreadable — GetDataTransformationProfile refuses instead of serving the DRAFT ===
Fixture/step: healthlake-data-transformation-profile-lifecycle#task#2 (cluster also covers healthlake-data-transformation-profile-lifecycle#task#4)
AWS: Smithy: GetDataTransformationProfile.ProfileVersion is OPTIONAL (@range 0..99); the output requires ProfileId, Version, SourceFormat, TargetFormat, ProfileMapping and LastUpdatedAt. Recorded behavior: for a profile that has been created (and optionally updated) but NEVER published, a Get with no ProfileVersion answers HTTP 200 describing the DRAFT — Version 0, the mapping the create/update wrote, and the profile's name, source format and target format (TargetFormat is always FHIR_R4 per the model). The DRAFT is a real, readable version; it is what the service returns when there is no published version.
Emulator: refuses with HTTP 400 and no success body at all (every required output member missing), because the omitted-version resolution only ever looks for a latest PUBLISHED version. The same step fails again after an update (task#4), so the whole read-after-write loop is dead.
Why: the normal authoring loop create -> get, and create -> update -> get, cannot be exercised at all: the content the client just wrote is unreadable until it publishes, and client code that reads back a draft before publishing fails against the emulator while working against AWS.
Fix: the omitted-ProfileVersion resolution in GetDataTransformationProfile's handler — fall back to the DRAFT (version 0) when the profile holds no published version, and keep serving the latest published version when it does.
Traces: healthlake-data-transformation-profile-lifecycle#task#2; healthlake-data-transformation-profile-lifecycle#task#4.

=== Cause 10 of 10: a syntactically invalid ResourceARN is refused with the framework's pattern-constraint text instead of the ARN parser's ===
Fixture/step: healthlake-resource-tagging-errors#task#1
AWS: Smithy: ListTagsForResource.ResourceARN is required, @length 1..1011, @pattern ^arn:aws((-us-gov)|(-iso)|(-iso-b)|(-cn))?:healthlake:[a-z0-9-]+:\d{12}:(datastore\/fhir\/.{32}|dataTransformationProfile\/[a-f0-9]{32})$; errors ResourceNotFoundException and ValidationException. Recorded behavior: a value that is not an ARN at all is refused with HTTP 400 whose `Message` member reports the ARN PARSER's failure, not the member pattern: "com.amazon.arn.ARNSyntaxException: ARNs must start with 'arn:': <value>" — fixed text "com.amazon.arn.ARNSyntaxException: ARNs must start with 'arn:': ", the single variable part being the rejected ResourceARN echoed from the request. The service parses the ARN and surfaces the parser's own message for a value lacking the 'arn:' prefix.
Emulator: HTTP 400 whose `$.Message` is the generic boundary-constraint sentence with the whole modelled regex pasted in: "1 validation error detected: Value '<value>' at 'resourceARN' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws((-us-gov)|(-iso)|(-iso-b)|(-cn))?:healthlake:[a-z0-9-]+:\d{12}:(datastore\/fhir\/.{32}|dataTransformationProfile\/[a-f0-9]{32})$".
Why: the refusal a client sees for a malformed ARN is text AWS never emits for this error class, and it leaks the service's internal resource-ARN regex to callers. Clients and tests that match on the ARNSyntaxException sentence do not fire.
Fix: ResourceARN handling in the tagging handlers (ListTagsForResource and the ARN-keyed tagging paths that share its parser) — parse the ARN and emit the ARN-syntax refusal for a value with no 'arn:' prefix, instead of letting the member's @pattern check produce a generic constraint sentence.
Traces: healthlake-resource-tagging-errors#task#1.

## Evidence
Replay of trace_set simparity-wire-aws-v1 against the emulator built from this checkout. Summary: emulator_up true (/__admin/health 200), bootstrap_signal false, fixtures_replayed 6, steps_total 31, steps_passed 19, steps_mismatched 12, steps_errored 0, steps_blocked 0, score_pct 61.3, divergence_count 10. All 10 clusters carry service "healthlake", empty stub_path and empty owned_by, so all 10 are net-new.

Cause 1 — healthlake-data-transformation-job-errors-and-list#task#1, DescribeDataTransformationJob, verdict mismatch, status 400 -> 400:
  $.Message: value_mismatch (expected 'Transformation job not found: sim2real-job-e00d6de8', got 'Could not find specified data transformation job.')
  The expected string's fixed part is "Transformation job not found: "; "sim2real-job-e00d6de8" is the request's JobId in that run and is NOT part of the requirement.

Cause 2 — healthlake-data-transformation-job-errors-and-list#task#2, DescribeDataTransformationJob, mismatch, 400 -> 400:
  $.message: missing (expected "1 validation error detected: Value at 'jobId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9-]+", got None)

Cause 3 — healthlake-data-transformation-profile-errors#task#1, CreateDataTransformationProfile, mismatch, 400 -> 400:
  $.message: missing (expected "1 validation error detected: Value at 'sourceFormat' failed to satisfy constraint: Member must satisfy enum value set: [CSV, HL7V2, CCDA]", got None)

Cause 4 — healthlake-data-transformation-profile-errors#task#2, GetDataTransformationProfile, mismatch, 400 -> 400 (affected_count 2; also healthlake-data-transformation-profile-lifecycle#task#8):
  $.Message: value_mismatch (expected "Profile 'aa044748ca8c7aee3796cbf0ae307227' not found", got 'Could not find specified profile.')
  Fixed parts "Profile '" / "' not found"; the 32-hex id is that run's request value.

Cause 5 — healthlake-data-transformation-profile-errors#task#3, UpdateDataTransformationProfile, mismatch, 400 -> 400:
  $.Message: value_mismatch (expected "Profile 'aa044748ca8c7aee3796cbf0ae307227' version 0 not found", got 'Could not find specified profile.')

Cause 6 — healthlake-data-transformation-profile-errors#task#4, ListDataTransformationProfileVersions, mismatch, 400 -> 400:
  $.Message: value_mismatch (expected "Profile 'aa044748ca8c7aee3796cbf0ae307227' not found", got 'Could not find specified profile.')

Cause 7 — healthlake-data-transformation-profile-errors#task#5, ListDataTransformationProfiles, mismatch, 400 -> 400:
  $.message: missing (expected "1 validation error detected: Value at 'sourceFormat' failed to satisfy constraint: Member must satisfy enum value set: [CSV, HL7V2, CCDA]", got None)

Cause 8 — healthlake-data-transformation-profile-lifecycle#task#1, CreateDataTransformationProfile, mismatch, expected 200 -> got 201:
  status: expected 200, got 201
  $.ProfileStatus: missing (expected 'ACTIVE', got None)

Cause 9 — healthlake-data-transformation-profile-lifecycle#task#2, GetDataTransformationProfile, mismatch, expected 200 -> got 400 (affected_count 2; also lifecycle#task#4):
  status: expected 200, got 400
  $.ProfileId / $.ProfileMapping / $.ProfileName / $.SourceFormat / $.TargetFormat / $.Version / $.LastUpdatedAt: all missing
  (the recorded run's values — ProfileId d6b3a52209ac7ef814d384ad8e9e9869, ProfileName sim2real-probe-healthlake-1e419ae2, SourceFormat CCDA, TargetFormat FHIR_R4, Version 0, ProfileMapping {'Resources/Patient.vm': 'initial-content'}, LastUpdatedAt shaped-not-asserted — are that run's inputs, not the requirement; the requirement is a 200 describing the DRAFT the preceding create wrote)

Cause 10 — healthlake-resource-tagging-errors#task#1, ListTagsForResource, mismatch, 400 -> 400:
  $.Message: value_mismatch (expected "com.amazon.arn.ARNSyntaxException: ARNs must start with 'arn:': not-a-valid-arn", got "1 validation error detected: Value 'not-a-valid-arn' at 'resourceARN' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws((-us-gov)|(-iso)|(-iso-b)|(-cn))?:healthlake:[a-z0-9-]+:\\d{12}:(datastore\\/fhir\\/.{32}|dataTransformationProfile\\/[a-f0-9]{32})$")

Model facts used (smithy_query on services/service-com-amazonaws-healthlake-healthlake/smithy-model/com-amazonaws-healthlake-healthlake.json):
  service_info: HealthLake 2017-07-01, protocols [awsJson1_0], common_errors [ValidationException] — so @http codes are inert and the success status is 200 (relevant to Cause 8)
  DescribeDataTransformationJob.JobId required, length 1..32, pattern ^[a-zA-Z0-9-]+$ (Causes 1, 2)
  SourceFormat enum values [CCDA, CSV] — narrower than the recorded served set [CSV, HL7V2, CCDA] (Causes 3, 7)
  CreateDataTransformationProfile @http code 201 (inert); CreateDataTransformationProfileResponse models no ProfileStatus (Cause 8)
  GetDataTransformationProfile.ProfileVersion optional, @range 0..99; output requires ProfileId/Version/SourceFormat/TargetFormat/ProfileMapping/LastUpdatedAt (Cause 9)
  ListTagsForResource.ResourceARN required, length 1..1011, with the healthlake resource-ARN pattern (Cause 10)
  ValidationException and ResourceNotFoundException both model a single `Message` member (HealthLakeString, length 0..10000)

Code locations consulted (citations only):
  services/service-com-amazonaws-healthlake-healthlake/src/state.rs:272 PROFILE_NOT_FOUND_MESSAGE = "Could not find specified profile." (Causes 4, 5, 6)
  services/service-com-amazonaws-healthlake-healthlake/src/state.rs:278 PROFILE_VERSION_NOT_FOUND_MESSAGE = "Could not find specified profile version." (Causes 5, 9)
  services/service-com-amazonaws-healthlake-healthlake/src/state.rs:288 TRANSFORMATION_JOB_NOT_FOUND_MESSAGE = "Could not find specified data transformation job." (Cause 1) — all three are documented in-crate as deliberately fixed sentences that never quote the request

## Steps to Reproduce
Boot the emulator from this checkout and replay the corpus: replay_wire_corpus() on trace set simparity-wire-aws-v1 reproduces all ten (6 fixtures, 31 steps, 12 mismatched). Per cause, against a healthlake client pointed at the emulator's data listener:

Cause 1 — healthlake-data-transformation-job-errors-and-list#task#1: DescribeDataTransformationJob with a syntactically valid JobId that no record holds (e.g. matching ^[a-zA-Z0-9-]+$, length <= 32). Read Error.Message: emulator serves the fixed constant; AWS serves "Transformation job not found: <JobId>".

Cause 2 — healthlake-data-transformation-job-errors-and-list#task#2: DescribeDataTransformationJob with a JobId violating ^[a-zA-Z0-9-]+$ (e.g. containing '_' or '/'). Inspect the raw 400 JSON body: there is no lowercase `message` member; AWS puts the "1 validation error detected: Value at 'jobId' ..." sentence there.

Cause 3 — healthlake-data-transformation-profile-errors#task#1: CreateDataTransformationProfile with SourceFormat set to a value outside the served set (not CSV/HL7V2/CCDA), with a valid ProfileName and Source. Inspect the raw 400 body for the lowercase `message` member.

Cause 4 — healthlake-data-transformation-profile-errors#task#2: GetDataTransformationProfile with a well-formed 32-hex ProfileId that no record holds (or one just deleted). Read Error.Message.

Cause 5 — healthlake-data-transformation-profile-errors#task#3: UpdateDataTransformationProfile with a well-formed 32-hex ProfileId that no record holds and a valid 1..500-entry ProfileMapping. Read Error.Message: expected "Profile '<ProfileId>' version 0 not found".

Cause 6 — healthlake-data-transformation-profile-errors#task#4: ListDataTransformationProfileVersions with a well-formed 32-hex ProfileId that no record holds. Read Error.Message.

Cause 7 — healthlake-data-transformation-profile-errors#task#5: ListDataTransformationProfiles with SourceFormat set to a value outside the served set. Inspect the raw 400 body for the lowercase `message` member.

Cause 8 — healthlake-data-transformation-profile-lifecycle#task#1: CreateDataTransformationProfile(SourceFormat=CCDA, ProfileName=<name>, Source={ProfileMapping:{ProfileMapping:{"Resources/Patient.vm":"initial-content"}}}). Observe the HTTP status (emulator 201, AWS 200) and that the body has no ProfileStatus member (AWS sends ProfileStatus=ACTIVE).

Cause 9 — healthlake-data-transformation-profile-lifecycle#task#2 and #task#4: after that create (and again after an UpdateDataTransformationProfile), call GetDataTransformationProfile(ProfileId) with NO ProfileVersion. Emulator answers 400 with no body members; AWS answers 200 with Version=0 and the DRAFT's ProfileMapping, ProfileName, SourceFormat, TargetFormat and LastUpdatedAt.

Cause 10 — healthlake-resource-tagging-errors#task#1: ListTagsForResource with a ResourceARN that is not an ARN at all (no 'arn:' prefix). Read Error.Message: emulator emits the generic constraint sentence containing the full resource-ARN regex; AWS emits "com.amazon.arn.ARNSyntaxException: ARNs must start with 'arn:': <value>".
