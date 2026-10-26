Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.security-configuration-roundtrip emr.supported-instance-types-report-accurate-hardware-facts
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr.security-configuration-roundtrip|run#13|AssertionError|assert detail == "ResourceNotFound", f"expected ErrorCode ResourceNotFound for the deleted name, got {detail!r}"
Issue-Key: emr.supported-instance-types-report-accurate-hardware-facts|run#20|AssertionError|assert workload_classes == {"GENERAL", "COMPUTE", "HI_MEM", "STORAGE", "GPU"}, (
Issue-Commit: 71cff0481dbaf9b41262283aed7350a64aff149f

# EMR control-plane fidelity gaps: security-configuration not-found refusals carry no ErrorCode, and the ListSupportedInstanceTypes catalogue omits the STORAGE and GPU workload classes (2 causes)

> _Note: this issue was identified against the code base at commit `71cff0481dbaf9b41262283aed7350a64aff149f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSecurityConfiguration

## Description
Two net-new root causes in the emr (ElasticMapReduce) service, both reached by passing behavioral checks that then diverge on response CONTENT.

Cause 1 of 2: the InvalidRequestException raised for a security configuration that does not exist omits the exception's ErrorCode discriminator, so a client cannot tell a not-found from any other bad request.
AWS: Smithy com.amazonaws.emr#DescribeSecurityConfiguration and #DeleteSecurityConfiguration each model exactly two errors, InternalServerException (HTTP 500) and InvalidRequestException (HTTP 400, errorKind client). com.amazonaws.emr#InvalidRequestException is a structure with TWO members: ErrorCode (target ErrorCode, @length min 1 max 256, documented "The error code associated with the exception.") and Message ("The message associated with the exception."). Because EMR funnels every bad request through one exception shape, ErrorCode is the only discriminator a client has; recorded behavior for a name that is absent (never created, or already deleted) is HTTP 400, Error.Code "InvalidRequestException" AND body member ErrorCode "ResourceNotFound". The message's fixed text is "Security configuration with name '<Name>' does not exist." with <Name> taken verbatim from the request's Name member — the emulator already matches that message, so the message is not the defect.
Emulator: both legs answer HTTP 400 with Error.Code "InvalidRequestException" and the correct message, but the ErrorCode member is ABSENT from the body — botocore surfaces response["ErrorCode"] as None. Grounded in the handlers: services/service-com-amazonaws-emr-elasticmapreduce/src/ops/describe_security_configuration.rs line 161-166 constructs InvalidRequestException { error_code: None, message: Some(...) }, and services/service-com-amazonaws-emr-elasticmapreduce/src/ops/delete_security_configuration.rs line 152-159 does the same for the repeated delete.
Why: every client that branches on EMR's not-found condition breaks. Idempotent teardown is the common one — the canonical pattern is `except ClientError as e: if e.response.get("ErrorCode") != "ResourceNotFound": raise`, which against the emulator re-raises on an already-deleted configuration and fails the cleanup path; retry/CRUD wrappers that distinguish "gone" from "malformed input" to decide whether to recreate or to abort cannot distinguish them at all, because every refusal from this service looks identical apart from prose.
Fix: the error construction in these two operations' handlers (and any sibling that refuses a missing security configuration) must populate InvalidRequestException.error_code with the service's discriminator for an absent resource rather than leaving it None; the natural home is a shared not-found constructor in this crate's error/state layer so all EMR refusals set a discriminator consistently.
Traces: emr.security-configuration-roundtrip — the create/describe/list/delete round trip passes, then `assert detail == "ResourceNotFound"` fails with "got None" for the describe-after-delete leg; the identical assertion for the re-delete leg follows it.

Cause 2 of 2: the instance-type catalogue ListSupportedInstanceTypes publishes only three of EMR's five workload classes — no STORAGE and no GPU family is catalogued for any release.
AWS: Smithy com.amazonaws.emr#ListSupportedInstanceTypes returns SupportedInstanceTypesList of com.amazonaws.emr#SupportedInstanceType, whose InstanceFamilyId member is documented as "The Amazon EC2 family and generation for the SupportedInstanceType" (and StorageGB/NumberOfDisks as "0 for Amazon EBS-only instance types", i.e. non-zero for instance-store types). The model enumerates no instance type, so the CONTENT is recorded behavior only: real EMR's catalogue for a current emr-6.x release spans all five workload classes crossed with generation — GENERAL_/COMPUTE_/HI_MEM_/STORAGE_/GPU_ × CURRENT_GEN|PREVIOUS_GEN — covering storage-optimised (instance-store) and GPU/accelerator families alongside the EBS-only ones, several hundred entries in total.
Emulator: the catalogue for the newest emr-6 release reports InstanceFamilyId values drawn from only {COMPUTE, GENERAL, HI_MEM} × CURRENT_GEN; no STORAGE_* and no GPU_* entry exists, and no PREVIOUS_GEN entry exists. This is by construction: services/service-com-amazonaws-emr-elasticmapreduce/src/state.rs lines 3440-3456 document that instance-store families and "the GPU/accelerator families" are deliberately outside INSTANCE_FAMILIES (lines 3456-3621 list only m5/m5a/m6i/m6a/m7i/m6g/m7g, c5/c6i/c7i/c6g/c7g, r5/r6i/r6g/r7g, every one current_generation: true), and ops/list_supported_instance_types.rs line 176 projects exactly that table.
Why: ListSupportedInstanceTypes plus ListReleaseLabels are EMR's entire reference-data surface, and correctness here IS the content. A capacity-planning, cost-estimation or instance-selection flow that asks EMR which types a release supports is told that no storage-optimised and no GPU instance type is supportable on any EMR release, so it silently excludes exactly the families chosen for HBase/Presto local-disk and ML workloads — and a validator built on the catalogue will reject a cluster request for i3en/d3/g4dn/p3 that real EMR accepts.
Fix: the service's fixed catalogue data (crate::state::INSTANCE_FAMILIES in services/service-com-amazonaws-emr-elasticmapreduce/src/state.rs, consumed by crate::state::supported_instance_types) must be extended to cover the storage-optimised and GPU/accelerator families EMR supports, with per-size StorageGB/NumberOfDisks for the instance-store ones and the previous-generation families EMR still lists, so InstanceFamilyId spans the five workload classes; the handler itself needs no change.
Traces: emr.supported-instance-types-report-accurate-hardware-facts — the per-type vCPU/memory/InstanceFamilyId assertions for m5.xlarge, m5.2xlarge and c5.xlarge pass, then `assert workload_classes == {"GENERAL", "COMPUTE", "HI_MEM", "STORAGE", "GPU"}` fails with "got ['COMPUTE', 'GENERAL', 'HI_MEM']".

Neither cause is covered by an open issue. The nearest open EMR issue, 20261025T021542Z-emr-s-service-wide-enum-refusal-override-misclassifies.md, concerns which exception an out-of-enum value is refused with (ListClusters/PutAutoScalingPolicy) and owns those two checks; it does not touch the ErrorCode member of a not-found refusal, nor the instance-type catalogue. The open stub issue 20261024T015202Z-emr-listsecurityconfigurations-stub.md does not apply here either: ListSecurityConfigurations answered correctly in this replay (the round trip's list assertions passed before the failing one).

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against this checkout: 28/101 passed; service emr 27/84.

Cause 1 — [mismatch] emr.security-configuration-roundtrip
check_emr_security_configuration_roundtrip.py line 89:
  assert detail == "ResourceNotFound", f"expected ErrorCode ResourceNotFound for the deleted name, got {detail!r}"
  AssertionError: expected ErrorCode ResourceNotFound for the deleted name, got None
where detail = exc.response.get("ErrorCode").
Expected (AWS): DescribeSecurityConfiguration(Name=<deleted name>) -> HTTP 400, Error.Code "InvalidRequestException", body ErrorCode "ResourceNotFound", Message "Security configuration with name '<Name>' does not exist." (fixed text; <Name> is the request's Name). DeleteSecurityConfiguration on the same dead name -> the same refusal.
Actual (emulator): HTTP 400 and Error.Code "InvalidRequestException" (those assertions passed), message present, but no ErrorCode member at all -> None.

Cause 2 — [mismatch] emr.supported-instance-types-report-accurate-hardware-facts
check_emr_supported_instance_types_report_accurate_hardware_facts.py line 212:
  assert workload_classes == {"GENERAL", "COMPUTE", "HI_MEM", "STORAGE", "GPU"}
  AssertionError: the emr-6.15.0 catalogue must span all five EMR workload classes, got ['COMPUTE', 'GENERAL', 'HI_MEM']
Expected (AWS): the catalogue for the newest emr-6.x release reports InstanceFamilyId values covering all five workload classes (GENERAL/COMPUTE/HI_MEM/STORAGE/GPU, each × CURRENT_GEN|PREVIOUS_GEN), including instance-store families with non-zero StorageGB/NumberOfDisks and GPU/accelerator families.
Actual (emulator): only GENERAL_CURRENT_GEN, COMPUTE_CURRENT_GEN and HI_MEM_CURRENT_GEN occur; every STORAGE_* and GPU_* family is absent, as is every *_PREVIOUS_GEN family.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-behavioral-ga-prod-v1 behavioral suite.

Cause 1 (emr.security-configuration-roundtrip), by hand against the emulator's emr endpoint:
2. emr CreateSecurityConfiguration(Name=<n>, SecurityConfiguration='{"EncryptionConfiguration":{"EnableInTransitEncryption":false,"EnableAtRestEncryption":false}}').
3. emr DeleteSecurityConfiguration(Name=<n>).
4. emr DescribeSecurityConfiguration(Name=<n>) and catch the ClientError; inspect exc.response["ErrorCode"]. Observe it is missing (None) where AWS returns "ResourceNotFound"; Error.Code and the HTTP 400 are already correct.
5. Repeat emr DeleteSecurityConfiguration(Name=<n>) and inspect exc.response["ErrorCode"]: same omission.

Cause 2 (emr.supported-instance-types-report-accurate-hardware-facts):
6. emr ListReleaseLabels(Filters={"Prefix": "emr-6."}) and take the newest label returned (newest-first).
7. emr ListSupportedInstanceTypes(ReleaseLabel=<that label>), following the Marker to the end of the catalogue.
8. Collect {entry["InstanceFamilyId"].rsplit("_", 2)[0] for entry in all entries}. Observe {"GENERAL","COMPUTE","HI_MEM"}; AWS reports all five of GENERAL/COMPUTE/HI_MEM/STORAGE/GPU. Equivalently, no entry has a non-zero StorageGB/NumberOfDisks and no g/p/i/d family appears.
