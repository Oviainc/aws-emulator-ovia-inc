Issue-Source: trace-sweep-divergence
Issue-Trace: emr-containers-job-template-lifecycle
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: emr-containers-job-template-lifecycle emr-containers-security-configuration-lifecycle
Issue-Shape-Id: com.amazonaws.emrcontainers#AwsChicagoWebService
Issue-Key: emr-containers|DescribeJobTemplate|2xx->2xx|$.jobTemplate.createdAt
Issue-Key: emr-containers|DescribeJobTemplate|2xx->2xx|$.jobTemplate.createdBy
Issue-Key: emr-containers|ListJobTemplates|2xx->2xx|$.templates[arn="arn
Issue-Key: emr-containers|DescribeSecurityConfiguration|2xx->2xx|$.securityConfiguration.containerProvider
Issue-Key: emr-containers|DescribeSecurityConfiguration|2xx->2xx|$.securityConfiguration.createdAt
Issue-Key: emr-containers|DescribeSecurityConfiguration|2xx->2xx|$.securityConfiguration.createdBy
Issue-Commit: 0dfa9e823e78890cf2fe273b1e49d27d43c189eb

# emr-containers describe/list responses omit createdBy and serialize createdAt as epoch not date-time

> _Note: this issue was identified against the code base at commit `0dfa9e823e78890cf2fe273b1e49d27d43c189eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeJobTemplate

## Description
Three causes, one underlying fault: the emr-containers emulator does not populate the request-identity/creation metadata of the resources it stores, and it serializes the `createdAt` timestamp in the wrong wire form. Every read path that returns a JobTemplate or a SecurityConfiguration is affected. This issue is NOT closed until all three causes below are fixed and all three named fixture steps replay clean.

Cause 1 of 3 - DescribeJobTemplate returns a JobTemplate with no `createdBy` and a numeric `createdAt` (fixture emr-containers-job-template-lifecycle, step task#4).
AWS: Smithy (services/service-com-amazonaws-emrcontainers-awschicagowebservice/smithy-model/com-amazonaws-emrcontainers-awschicagowebservice.json): DescribeJobTemplate (GET /jobtemplates/{id}) returns `jobTemplate` of shape JobTemplate, whose member `createdAt` targets `Date` with @timestampFormat date-time ("The date and time when the job template was created") and whose member `createdBy` targets RequestIdentityUserArn, @length 20..2048, @pattern ^arn:(aws[a-zA-Z0-9-]*):(iam|sts)::(\d{12})?:[\w/+=,.@-]+$ ("The user who created the job template"). So AWS emits createdAt as an ISO-8601 date-time STRING, and emits createdBy as the IAM/STS principal ARN of the caller that created the template. The recording confirms both: createdAt present in the asserted date-time shape, and createdBy an IAM user ARN (recorded example: arn:aws:iam::000000000000:user/emulator - the account id and principal name are environment-specific and not themselves a requirement; the requirement is that the member is present and is a syntactically valid iam/sts principal ARN).
Emulator: returns HTTP 200 with `$.jobTemplate.createdAt` = 1792743173.931, an epoch-seconds NUMBER rather than a date-time string, and `$.jobTemplate.createdBy` absent entirely (None).
Why: `createdAt` is a @timestampFormat date-time member, so an AWS SDK deserializer expects a string and either fails to parse the number or silently yields a wrong/zero instant; a client that renders or sorts job templates by creation time gets a parse error or a bogus date. `createdBy` is how a caller attributes a template to a principal - audit/ownership logic, "who created this" columns, and any policy check keyed on the creator see a null and cannot proceed.
Fix: the JobTemplate serializer plus the CreateJobTemplate state write in the emr-containers service crate - record the caller's principal ARN on the stored template, and emit `createdAt` through the date-time timestamp serializer used for @timestampFormat date-time members instead of a raw epoch number.
Traces: emr-containers-job-template-lifecycle#task#4.

Cause 2 of 3 - ListJobTemplates has the same two defects in each element of `templates` (fixture emr-containers-job-template-lifecycle, step task#5).
AWS: Smithy: ListJobTemplates (GET /jobtemplates) returns `templates` as a list of the SAME JobTemplate shape, so every element carries the date-time `createdAt` and the RequestIdentityUserArn `createdBy` described in Cause 1. The recording shows the listed template carrying both.
Emulator: the listed template (arn .../jobtemplates/00000000083xm71d1b97ouzp8) comes back with `createdAt` = 1792743173.931 (epoch number, not a date-time string) and no `createdBy` member.
Why: list is the discovery path - a client that enumerates templates to display or filter them by age/owner is broken for every template, not just the one it describes individually. It also means the two read paths cannot be made consistent by fixing Describe alone.
Fix: same JobTemplate serializer in the emr-containers service crate, shared by the Describe and List paths, so one serialization change covers both; confirm the list path is not building its elements through a separate projection.
Traces: emr-containers-job-template-lifecycle#task#5.

Cause 3 of 3 - DescribeSecurityConfiguration omits createdBy and containerProvider and emits a numeric createdAt (fixture emr-containers-security-configuration-lifecycle, step task#4).
AWS: Smithy: DescribeSecurityConfiguration (GET /securityconfigurations/{id}) returns `securityConfiguration` of shape SecurityConfiguration, whose `createdAt` targets `Date` with @timestampFormat date-time and whose `createdBy` targets RequestIdentityUserArn with the same ARN pattern as above. For `containerProvider` the on-disk model says nothing - SecurityConfiguration declares only id/name/arn/createdAt/createdBy/securityConfigurationData/tags - so this part is RECORDED BEHAVIOR ONLY: CreateSecurityConfiguration does accept a `containerProvider` (ContainerProvider: required type EKS enum, required cluster id, optional info.eksInfo.namespace), and the recording shows the describe response echoing back the container provider supplied at create time, as the structure {type: EKS, id: <eks cluster id>, info: {eksInfo: {namespace: <namespace>}}}. Hypothesis: the on-disk model predates the member. The requirement taken from the recording is that the create-time containerProvider is echoed on read; its concrete id/namespace are per-environment values.
Emulator: returns HTTP 200 whose `securityConfiguration` has `createdAt` = 1792743174.676 (epoch number), and no `createdBy` and no `containerProvider` members at all. securityConfigurationData, id, name, arn and tags are present.
Why: the container provider is the only thing that tells a caller WHICH EKS cluster and namespace a security configuration is scoped to, so a client that reads a configuration back to decide where to submit work loses that binding and cannot round-trip a create. createdBy/createdAt break ownership attribution and time rendering as in Cause 1.
Fix: the SecurityConfiguration serializer and the CreateSecurityConfiguration state write in the emr-containers service crate - persist the create-time containerProvider and caller principal alongside the configuration and emit all three members on read, with createdAt going through the date-time timestamp serializer.
Traces: emr-containers-security-configuration-lifecycle#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up true; /__admin/health answered 200 and the data listener accepts; state_reset_failures 0; 22 steps, 9 passed, 9 divergence clusters).

Cause 1 - emr-containers-job-template-lifecycle#task#4, DescribeJobTemplate, status 200 -> 200:
  $.jobTemplate.createdAt: value_mismatch (expected '<shaped-not-asserted>' i.e. the recorded date-time shape, got 1792743173.931)
  $.jobTemplate.createdBy: missing (expected an iam principal ARN, recorded example 'arn:aws:iam::000000000000:user/emulator', got None)
  divergence_key: service=emr-containers|DescribeJobTemplate|status=200->200|keys=$.jobTemplate.createdAt,$.jobTemplate.createdBy

Cause 2 - emr-containers-job-template-lifecycle#task#5, ListJobTemplates, status 200 -> 200:
  $.templates[arn="arn:aws:emr-containers:us-west-2:000000000000:/jobtemplates/00000000083xm71d1b97ouzp8"].createdAt: value_mismatch (expected '<shaped-not-asserted>', got 1792743173.931)
  $.templates[arn="..."].createdBy: missing (expected 'arn:aws:iam::000000000000:user/emulator', got None)
  divergence_key: service=emr-containers|ListJobTemplates|status=200->200|keys=$.templates[arn="arn

Cause 3 - emr-containers-security-configuration-lifecycle#task#4, DescribeSecurityConfiguration, status 200 -> 200:
  $.securityConfiguration.createdAt: value_mismatch (expected '<shaped-not-asserted>', got 1792743174.676)
  $.securityConfiguration.createdBy: missing (expected 'arn:aws:iam::000000000000:user/emulator', got None)
  $.securityConfiguration.containerProvider: missing (expected {'type': 'EKS', 'id': 'sim2real-probe-emr-containers-secconfig-6fd42819-eks', 'info': {'eksInfo': {'namespace': 'default'}}}, got None)
  divergence_key: service=emr-containers|DescribeSecurityConfiguration|status=200->200|keys=$.securityConfiguration.containerProvider,$.securityConfiguration.createdAt,$.securityConfiguration.createdBy

Model evidence (services/service-com-amazonaws-emrcontainers-awschicagowebservice/smithy-model/com-amazonaws-emrcontainers-awschicagowebservice.json): JobTemplate.createdAt -> Date @timestampFormat date-time; JobTemplate.createdBy -> RequestIdentityUserArn @length 20..2048 @pattern ^arn:(aws[a-zA-Z0-9-]*):(iam|sts)::(\d{12})?:[\w/+=,.@-]+$; SecurityConfiguration.createdAt/createdBy identical; SecurityConfiguration declares no containerProvider member (recorded behavior only), while CreateSecurityConfiguration input accepts containerProvider (ContainerProvider{type,id,info}).

Note: this is distinct from the open issue 20261022T001417Z-emr-containers-error-paths-missing-input-validation-on.md, which covers input validation on nonexistent/malformed identifiers (CreateVirtualCluster task#2, DescribeJobRun task#4) and carries no replay key for any of the three success-path steps above.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from that checkout.
2. Replay trace set simparity-wire-aws-v1 against it.
3. Cause 1 - fixture emr-containers-job-template-lifecycle, step task#4: create a job template, then GET /jobtemplates/{id} (DescribeJobTemplate). Expect $.jobTemplate.createdAt to be an ISO-8601 date-time string and $.jobTemplate.createdBy to be an iam/sts principal ARN; observe createdAt = 1792743173.931 (epoch number) and createdBy absent.
4. Cause 2 - same fixture, step task#5: GET /jobtemplates (ListJobTemplates). Expect each element of $.templates to carry a date-time createdAt and a createdBy ARN; observe the epoch number and a missing createdBy on the listed template.
5. Cause 3 - fixture emr-containers-security-configuration-lifecycle, step task#4: CreateSecurityConfiguration with a containerProvider (type EKS, an eks cluster id, info.eksInfo.namespace), then GET /securityconfigurations/{id} (DescribeSecurityConfiguration). Expect createdAt as a date-time string, createdBy as a principal ARN, and containerProvider echoed back; observe a numeric createdAt and both other members absent.
6. Re-replay emr-containers-job-template-lifecycle#task#4, emr-containers-job-template-lifecycle#task#5 and emr-containers-security-configuration-lifecycle#task#4 to confirm the fix; all three must pass.
