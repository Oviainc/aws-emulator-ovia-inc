Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connecthealth.domain-tags-visible-via-resourcegroupstaggingapi connecthealth.get-patient-insights-job-not-found connecthealth.start-patient-insights-job-conflicting-client-token-rejected connecthealth.start-patient-insights-job-malformed-s3-uri-rejected
Issue-Shape-Id: com.amazonaws.connecthealth#ConnectHealth
Issue-Key: connecthealth.domain-tags-visible-via-resourcegroupstaggingapi|run#17|AssertionError|assert found is not None, "tagged Domain never appeared via ResourceGroupsTaggingAPI within 60s"
Issue-Key: connecthealth.get-patient-insights-job-not-found|_seed_patient#10|AssertionError|raise AssertionError(f"seeded Patient/{patient_id} never became searchable: {sstatus} {sresp}")
Issue-Key: connecthealth.start-patient-insights-job-conflicting-client-token-rejected|_seed_patient#11|AssertionError|raise AssertionError(f"seeded Patient/{patient_id} never became searchable: {sstatus} {sresp}")
Issue-Key: connecthealth.start-patient-insights-job-malformed-s3-uri-rejected|_seed_patient#10|AssertionError|raise AssertionError(f"seeded Patient/{patient_id} never became searchable: {sstatus} {sresp}")
Issue-Commit: 9fabd942fcccf035b09fd7fdc090ac5f93c12e3a

# connecthealth behavioral surface: 2 divergences — tagged Domains are invisible to Resource Groups Tagging API GetResources, and the HealthLake datastore FHIR endpoint answers type-level search 501

> _Note: this issue was identified against the code base at commit `9fabd942fcccf035b09fd7fdc090ac5f93c12e3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResources (resourcegroupstaggingapi) and the HealthLake datastore FHIR type-level search GET [base]/Patient that StartPatientInsightsJob/GetPatientInsightsJob checks depend on

## Description
Two distinct root causes found by replaying the simparity-behavioral-aws-v1 behavioral suite against the emulator built from this checkout. Both are reported by the suite under service heading '### connecthealth' (4 failing checks total). They live in different components and BOTH must be closed before this issue is submitted.

Cause 1 of 2: a Domain tagged through connecthealth TagResource is never discoverable through Resource Groups Tagging API GetResources (failing check: connecthealth.domain-tags-visible-via-resourcegroupstaggingapi).
AWS: tagging is a cross-service index. A connecthealth Domain carries an ARN (CreateDomain output member `arn`; Smithy connecthealth restJson1, TagResource POST /tags/{resourceArn} -> 204, ListTagsForResource GET /tags/{resourceArn}), and once a tag is attached to that ARN through the owning service's TagResource, resourcegroupstaggingapi GetResources returns a ResourceTagMapping for that ARN on every query route. The resourcegroupstaggingapi model states this explicitly on GetResourcesInput.ResourceTypeFilters: "All tagged resources, whether the tagging used the Resource Groups Tagging API or not, are returned by the Get* operation". Propagation is eventually consistent but bounded; the recorded behavior is that the Domain's ARN appears in ResourceTagMappingList, with Tags containing the key/value just written, when queried with TagFilters [{Key, Values:[value]}] and ResourceTypeFilters ["health-agent"], and disappears from that same index after UntagResource.
Emulator: connecthealth's own tag plane is correct — TagResource then ListTagsForResource returns exactly the written tag — but GetResources answers 200 with an empty ResourceTagMappingList for that tag filter + the "health-agent" resource-type filter, repeatedly, for the full 60s the check polls. The ARN never enters the tagging index at all, so the connecthealth tag store and the Resource Groups Tagging index are disconnected.
Why: a client doing cross-service tag discovery (inventory, cost allocation, tag-compliance, bulk cleanup) concludes the account holds no tagged health-agent resources even though connecthealth reports the tags, so governance tooling silently acts on an empty set. It also makes the negative half of the contract (absence after UntagResource) vacuously true, hiding removal bugs.
Fix: the write path that must publish a connecthealth Domain's tags into the shared tagging index — connecthealth's TagResource/UntagResource handlers and the tag state they write, plus the GetResources read path/resource-type table in the resourcegroupstaggingapi service so "health-agent" resolves to connecthealth Domains. Open issue 20261102T083644Z-resourcegroupstaggingapi-getresources-never-returns-resources-tagged-by-their-native.md describes the same store gap for Lambda-tagged resources from the wire corpus; it is a separate work item (different service token, Lambda-only evidence) and does not cover the connecthealth/health-agent route proven here — if that store gap is fixed generically, re-replay this check to confirm.
Traces: connecthealth.domain-tags-visible-via-resourcegroupstaggingapi.

Cause 2 of 2: the HealthLake data store FHIR endpoint does not implement type-level search, so no connecthealth check that needs a resolvable FHIR Patient can run (failing checks: connecthealth.get-patient-insights-job-not-found, connecthealth.start-patient-insights-job-conflicting-client-token-rejected, connecthealth.start-patient-insights-job-malformed-s3-uri-rejected).
AWS: recorded behavior only — the FHIR REST data plane is not in the Smithy model; healthlake models only the control plane and hands the caller the data-plane base URL (DatastoreProperties.DatastoreEndpoint, required). Against that endpoint, a SigV4-signed `GET [base]/<ResourceType>?<search-param>=<value>` (here `GET [base]/Patient?identifier=<id>`) is a FHIR R4 type-level search and answers HTTP 200 with a Bundle of `type` "searchset" whose `entry` array holds one element per matching resource (an empty `entry`/`total` 0 when nothing matches) — never a 501. The recording shows the seeded Patient becoming searchable by `identifier` within the poll window after the PUT create, which is what connecthealth StartPatientInsightsJob resolves patientContext.patientId against.
Emulator: the datastore endpoint serves only the create/read/update interactions and refuses search with HTTP 501 and a FHIR OperationOutcome: issue[0].severity "fatal", issue[0].code "not-supported", issue[0].diagnostics "This data store endpoint implements the FHIR create (POST [base]/{type}), read (GET [base]/{type}/{id}) and update (PUT [base]/{type}/{id}) interactions; GET [base]/Patient is not implemented yet". In that message the whole sentence is fixed emulator text (it is itself the defect — AWS has no such response); only the request-derived part would be the resource type. The direct by-id read (GET [base]/Patient/{id}) returns 200, so the resource exists; only the search route is unserved.
Why: a client cannot query a datastore by search parameter — the primary FHIR read pattern — and gets a status (501) that is not part of the FHIR/HealthLake contract, so SDK/FHIR clients surface it as a server defect rather than an empty result. Downstream, three connecthealth checks cannot even reach their assertions on GetPatientInsightsJob not-found, StartPatientInsightsJob idempotency-token conflict, and StartPatientInsightsJob s3OutputPath validation, so that whole slice of connecthealth fidelity is unobservable.
Fix: the HealthLake datastore FHIR data-plane router/handler — add the type-level search interaction (`GET [base]/{type}` with search parameters, at minimum `identifier` for Patient) returning a 200 searchset Bundle over the resources the create/update path already stores.
Traces: connecthealth.get-patient-insights-job-not-found, connecthealth.start-patient-insights-job-conflicting-client-token-rejected, connecthealth.start-patient-insights-job-malformed-s3-uri-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout: 15/22 passed (68.2%), 7 mismatched, 0 errored, 0 skipped, bootstrap_signal=false. All 7 failures are under '### connecthealth'; 3 are marked OWNED by existing issues and excluded here. The 4 remaining cluster into the 2 causes above.

Cause 1 — check connecthealth.domain-tags-visible-via-resourcegroupstaggingapi [mismatch]:
  AssertionError: tagged Domain never appeared via ResourceGroupsTaggingAPI within 60s
  (check_connecthealth_domain_tags_visible_via_resourcegroupstaggingapi.py line 70)
  Sequence: connecthealth CreateDomain -> TagResource(resourceArn=<domain arn>, tags={"sim2real-probe": <value>}) -> ListTagsForResource returns exactly that tag (this assertion PASSED) -> resourcegroupstaggingapi GetResources(TagFilters=[{Key:"sim2real-probe", Values:[<value>]}], ResourceTypeFilters=["health-agent"]) polled every 3s for 60s, ResourceTagMappingList never contains the Domain ARN.
  Model evidence: services/service-com-amazonaws-resourcegroupstaggingapi-resourcegroupstaggingapi-20170126/smithy-model/... GetResourcesInput.ResourceTypeFilters doc: "All tagged resources, whether the tagging used the Resource Groups Tagging API or not, are returned by the Get* operation." connecthealth model (services/service-com-amazonaws-connecthealth-connecthealth/smithy-model/com-amazonaws-connecthealth-connecthealth.json): TagResource POST /tags/{resourceArn} (204), UntagResource DELETE /tags/{resourceArn} (204), ListTagsForResource GET /tags/{resourceArn}.

Cause 2 — checks connecthealth.get-patient-insights-job-not-found, connecthealth.start-patient-insights-job-conflicting-client-token-rejected, connecthealth.start-patient-insights-job-malformed-s3-uri-rejected [all mismatch], all failing in the shared _seed_patient helper:
  AssertionError: seeded Patient/<patient-id> never became searchable: 501 {'resourceType': 'OperationOutcome', 'issue': [{'severity': 'fatal', 'code': 'not-supported', 'diagnostics': 'This data store endpoint implements the FHIR create (POST [base]/{type}), read (GET [base]/{type}/{id}) and update (PUT [base]/{type}/{id}) interactions; GET [base]/Patient is not implemented yet'}]}
  (the patient ids in the three runs — sim2real-patient-307343eb8a43 / -3756dd39191e / -c28c76301283 — are per-run values, not part of the requirement)
  Preceding steps in the same helper SUCCEEDED: healthlake CreateFHIRDatastore reached ACTIVE, SigV4-signed PUT <DatastoreEndpoint>/Patient/<id> returned 200/201, and GET <DatastoreEndpoint>/Patient/<id> returned 200. Only GET <DatastoreEndpoint>/Patient?identifier=<id> is refused, with HTTP 501, for the full 90s poll window.
  Model evidence: healthlake Smithy (services/service-com-amazonaws-healthlake-healthlake/smithy-model/com-amazonaws-healthlake-healthlake.json) DatastoreProperties.DatastoreEndpoint is required ("The Amazon Web Services endpoint for the data store"); the FHIR REST interactions served at that endpoint are not modeled, hence "recorded behavior only" for the expected 200 searchset Bundle.

## Steps to Reproduce
Run the simparity-behavioral-aws-v1 behavioral suite against the emulator built from this checkout and re-replay the four checks in trace_ids.

Cause 1 (connecthealth.domain-tags-visible-via-resourcegroupstaggingapi):
1. connecthealth CreateDomain(name=<unique>) -> note domainId and arn.
2. connecthealth TagResource(resourceArn=<arn>, tags={"sim2real-probe": "<value>"}).
3. connecthealth ListTagsForResource(resourceArn=<arn>) -> {"sim2real-probe": "<value>"} (passes today).
4. resourcegroupstaggingapi GetResources(TagFilters=[{"Key":"sim2real-probe","Values":["<value>"]}], ResourceTypeFilters=["health-agent"]), polled for 60s.
   Expect: a ResourceTagMappingList entry whose ResourceARN == <arn> and whose Tags contain sim2real-probe=<value>.
   Observe: ResourceTagMappingList stays empty for the whole window.
5. connecthealth UntagResource(resourceArn=<arn>, tagKeys=["sim2real-probe"]) and repeat step 4: the entry must then be absent (unverifiable today because it never appeared).

Cause 2 (the three StartPatientInsightsJob/GetPatientInsightsJob checks):
1. healthlake CreateFHIRDatastore(DatastoreTypeVersion="R4") and wait for DatastoreStatus ACTIVE; take DatastoreProperties.DatastoreEndpoint.
2. SigV4-sign (service "healthlake") and send PUT <DatastoreEndpoint>/Patient/<patient-id> with Content-Type application/fhir+json and a minimal Patient body carrying identifier[0].value = <patient-id>. Returns 200/201 today.
3. GET <DatastoreEndpoint>/Patient/<patient-id> -> 200 today.
4. GET <DatastoreEndpoint>/Patient?identifier=<patient-id>.
   Expect: HTTP 200 with {"resourceType":"Bundle","type":"searchset", ...} whose entry array holds the seeded Patient (empty entry/total 0 when no resource matches).
   Observe: HTTP 501 with an OperationOutcome whose issue[0] is severity "fatal", code "not-supported", diagnostics "This data store endpoint implements the FHIR create (POST [base]/{type}), read (GET [base]/{type}/{id}) and update (PUT [base]/{type}/{id}) interactions; GET [base]/Patient is not implemented yet".
5. With search served, the three checks proceed to their real assertions: connecthealth StartPatientInsightsJob with patientContext.patientId = <patient-id> (positive control), then GetPatientInsightsJob on an unknown jobId, a reused-but-conflicting clientToken, and a malformed s3OutputPath.
