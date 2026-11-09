Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: drs.concurrent-tag-resource-disjoint-keys-merge drs.replication-configuration-template-lifecycle drs.create-replication-configuration-template-nonexistent-subnet-rejected drs.create-launch-configuration-template-invalid-enum-rejected drs.tag-operations-not-found-on-nonexistent-recovery-plan
Issue-Shape-Id: com.amazonaws.drs#ElasticDisasterRecoveryService
Issue-Key: drs|CreateReplicationConfigurationTemplate|UninitializedAccountException
Issue-Key: drs.create-replication-configuration-template-nonexistent-subnet-rejected|run#4|AssertionError|assert code == "ValidationException", (
Issue-Key: drs.create-launch-configuration-template-invalid-enum-rejected|run#3|AssertionError|assert code == "BadRequestException", (
Issue-Key: drs.tag-operations-not-found-on-nonexistent-recovery-plan|run#5|AssertionError|assert code == "NotFoundException", f"TagResource on a nonexistent plan ARN got {code!r}"
Issue-Commit: 545841f07feb57b92e392d55db412e521f655a7b

# drs: 4 net-new behavioral root causes — the account-initialization gate refuses every initialized-account call, no stagingAreaSubnetId existence check, out-of-enum rejection uses the wrong error code, and the three tag operations are still 501 stubs

> _Note: this issue was identified against the code base at commit `545841f07feb57b92e392d55db412e521f655a7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateReplicationConfigurationTemplate

## Description
Four distinct root causes in the Elastic Disaster Recovery (drs) emulator, all grounded in failing behavioral checks of this suite. They are independent: fixing any one leaves the others failing. All four must be closed before submitting.

Cause 1 of 4 — the account-initialization gate refuses calls from an account the suite has initialized.
AWS: recorded behavior only (the model declares UninitializedAccountException, HTTP 400, for CreateReplicationConfigurationTemplate and CreateLaunchConfigurationTemplate, documented as 'The account performing the request has not been initialized'). The suite's checks all carry the precondition tag `requires:drs-initialized`, i.e. they run against an account for which Elastic Disaster Recovery IS initialized, and in the recording CreateReplicationConfigurationTemplate answers 201 with the created template. AWS raises UninitializedAccountException only for an account that has never been onboarded; initialization is per-account, durable and idempotent, so once it holds every subsequent request from that account is served.
Emulator: every control-plane call from the suite is refused with HTTP 400 `UninitializedAccountException` and the message `Elastic Disaster Recovery has not been initialized for this account; call InitializeService first` (fixed text; it carries no request- or state-derived values). The gate is the in-process `initialized: bool` of ComAmazonawsDrsElasticdisasterrecoveryserviceState, which ONLY InitializeService sets, and it is still false when the suite's body runs. Hypothesis: either the suite satisfies `requires:drs-initialized` without an explicit InitializeService call (a real account is already onboarded), or the flag InitializeService writes is not visible to the requests that follow it; either way a freshly booted emulator presents an account that AWS would consider initialized as uninitialized.
Why: the DRS surface is unusable. A client that legitimately expects an onboarded account gets a 400 for every create/update/describe it attempts, and a client that defensively retries sees the same 400 forever, because nothing in a normal workflow flips the flag.
Fix: the service's initialization state and the gate that reads it — crate::state (the `initialized` field and its default) together with the per-operation check in each handler that raises UninitializedAccountException; the emulated account should present as initialized (InitializeService staying idempotent) rather than defaulting to refusing.
Traces: drs.replication-configuration-template-lifecycle, drs.concurrent-tag-resource-disjoint-keys-merge, drs.create-replication-configuration-template-nonexistent-subnet-rejected.

Cause 2 of 4 — CreateReplicationConfigurationTemplate never checks that stagingAreaSubnetId exists.
AWS: recorded behavior only. For a well-formed but nonexistent subnet id (`stagingAreaSubnetId` is modelled SubnetID, len 0..255, pattern ^subnet-[0-9a-fA-F]{8,}$, so a fake id satisfies every modelled constraint), AWS refuses with HTTP 400 `ValidationException` and the message `SubnetId <subnet-id> wasn't found` — fixed text `SubnetId ` … ` wasn't found` with the subnet id taken verbatim from the request — and creates nothing; the identical request naming a real subnet succeeds.
Emulator: the handler performs no existence check at all. It mints an id, stores the request's subnet id as given and answers 201, so a nonexistent subnet is accepted as valid (the replayed check currently never reaches this point because Cause 1 refuses it first with UninitializedAccountException — the check will still fail on this cause once Cause 1 is fixed).
Why: the emulator is overly permissive on the one input that ties a replication template to real network state. A client's bad or stale subnet id is silently accepted, so code that depends on the create failing fast (and on nothing being created) proceeds with a template that could never replicate, and the refusal path is untestable against the emulator.
Fix: input validation of CreateReplicationConfigurationTemplate's handler — resolve `stagingAreaSubnetId` against the emulator's EC2 subnet state before any state write, and refuse with the modelled ValidationException carrying the message above, leaving the store unchanged.
Traces: drs.create-replication-configuration-template-nonexistent-subnet-rejected.

Cause 3 of 4 — an out-of-enum enum member is refused with the wrong error code and message.
AWS: recorded behavior only; the model gives the member's value set (CreateLaunchConfigurationTemplate.targetInstanceTypeRightSizingMethod targets TargetInstanceTypeRightSizingMethod, whose members are BASIC | IN_AWS | NONE — the model declares no BadRequestException for the operation, so this is the frontend's refusal rather than a modelled one). The recording shows a value outside that enum refused with error code `BadRequestException` and the message `Invalid request body` (entirely fixed text — it names neither the member nor the offending value), and nothing created; the same request with a valid member succeeds and echoes it back.
Emulator: the same request is refused with error code `ValidationException`. The modelled-constraint validator that runs before the handler raises ValidationException for every constraint violation, including an unknown enum member.
Why: a client branching on the error code — retry/report logic, and SDK error-shape matching in particular — takes an enum typo for a constraint failure of a different class and never matches the BadRequestException AWS actually sends, so its handling of a malformed request body diverges from production.
Fix: the request-validation layer for this REST-JSON service's enum deserialization (the pre-handler validate step that maps an unknown enum member onto an error), which must answer BadRequestException with `Invalid request body` for an out-of-enum member rather than ValidationException.
Traces: drs.create-launch-configuration-template-invalid-enum-rejected.

Cause 4 of 4 — TagResource, ListTagsForResource and UntagResource are scaffold stubs answering 501 NotImplemented.
AWS: TagResource (POST /tags/{resourceArn}), ListTagsForResource (GET /tags/{resourceArn}) and UntagResource (DELETE /tags/{resourceArn}) are modelled with required `resourceArn` (ARN, len 20..2048, pattern ^arn:.{16,2044}$) and tag payloads, and declare ResourceNotFoundException (HTTP 404). For a nonexistent recovery-plan ARN the recording shows all three refusing with error code `NotFoundException` and the message `Resource was not found` (fixed text, no request-derived part) — notably NOT the ResourceNotFoundException Get/Update/Delete raise for the same missing plan — while against a real resource TagResource stores the tags and ListTagsForResource reads them back.
Emulator: all three handlers return `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501 with error code `NotImplemented` and the message `operation <Op> is scaffolded but not implemented in this emulator`, for every input — nonexistent ARN and real ARN alike.
Why: tagging is unusable on the whole DRS surface: a client cannot tag, read or untag any resource, and its not-found handling for tag operations never runs because every call fails with a code no AWS client models. Concurrent tag merges and tag-based lookups cannot be exercised at all.
Fix: implement the three operations in their own handler files (ops/tag_resource.rs, ops/list_tags_for_resource.rs, ops/untag_resource.rs) over per-resource tag state, resolving `resourceArn` against the stored resources and refusing an unknown ARN as described; delete each file's SCAFFOLDED marker when done.
Traces: drs.tag-operations-not-found-on-nonexistent-recovery-plan (and drs.concurrent-tag-resource-disjoint-keys-merge, which reaches TagResource only after Cause 1 is fixed).

Note: the open issues listed for this project cover ec2 scaffold stubs and Directory Service; none of them covers any cause above.

## Evidence
Behavioral suite: drs 0/15 (0%).

Cause 1 — expected: CreateReplicationConfigurationTemplate answers 201 with the created template (checks tagged `requires:drs-initialized`).
Actual: botocore.errorfactory.UninitializedAccountException: An error occurred (UninitializedAccountException) when calling the CreateReplicationConfigurationTemplate operation: Elastic Disaster Recovery has not been initialized for this account; call InitializeService first
  — drs.replication-configuration-template-lifecycle (errored before any assertion ran)
  — drs.concurrent-tag-resource-disjoint-keys-merge (errored after creating the VPC/subnet/SG it needed)
  — drs.create-replication-configuration-template-nonexistent-subnet-rejected: AssertionError: CreateReplicationConfigurationTemplate with a nonexistent subnet got 'UninitializedAccountException'
Gate source: services/service-com-amazonaws-drs-elasticdisasterrecoveryservice/src/ops/create_replication_configuration_template.rs:230-244 (`if !store.initialized`), state field at src/state.rs:19, only writer at src/ops/initialize_service.rs:151.

Cause 2 — expected: error code ValidationException, message `SubnetId subnet-00000000deadbeef0 wasn't found` (that id is the one the check sends; the message template is `SubnetId <request's stagingAreaSubnetId> wasn't found`), nothing created.
Actual: no existence check exists — create_replication_configuration_template.rs:271-307 mints an id and stores `input.staging_area_subnet_id` verbatim, then answers 201. (The replayed assertion currently reports 'UninitializedAccountException' because Cause 1 fires first.)

Cause 3 — drs.create-launch-configuration-template-invalid-enum-rejected:
  expected: error code `BadRequestException`, message `Invalid request body`
  actual:   AssertionError: CreateLaunchConfigurationTemplate with an out-of-enum targetInstanceTypeRightSizingMethod got 'ValidationException'
  request:  create_launch_configuration_template(targetInstanceTypeRightSizingMethod="BOGUS_VALUE"); the modelled enum is BASIC | IN_AWS | NONE.

Cause 4 — drs.tag-operations-not-found-on-nonexistent-recovery-plan:
  expected: TagResource / ListTagsForResource / UntagResource on `arn:aws:drs:<region>:<account>:recovery-plan/<missing-plan-id>` each answer error code `NotFoundException` with message `Resource was not found`
  actual:   AssertionError: TagResource on a nonexistent plan ARN got 'NotImplemented' (HTTP 501, message `operation TagResource is scaffolded but not implemented in this emulator`)
  source:   ops/tag_resource.rs:185, ops/list_tags_for_resource.rs and ops/untag_resource.rs all still carry the `//! SCAFFOLDED, NOT IMPLEMENTED` marker and return `Scaffolded::not_implemented(..)`.

## Steps to Reproduce
Boot the emulator and point a botocore `drs` client at it (region/account as the suite configures them).

Cause 1:
  1. drs.create_replication_configuration_template(stagingAreaSubnetId=<a real subnet in the run's network>, associateDefaultSecurityGroup=True, replicationServersSecurityGroupsIDs=[], replicationServerInstanceType='t3.small', useDedicatedReplicationServer=False, defaultLargeStagingDiskType='GP3', ebsEncryption='DEFAULT', bandwidthThrottling=0, dataPlaneRouting='PRIVATE_IP', createPublicIP=False, stagingAreaTags={}, pitPolicy=[3 rules])
  2. Observe HTTP 400 UninitializedAccountException instead of a 201 template. Re-replay drs.replication-configuration-template-lifecycle and drs.concurrent-tag-resource-disjoint-keys-merge.

Cause 2:
  1. Make the account initialized (fix Cause 1, or call drs.initialize_service() first).
  2. Issue the same create with stagingAreaSubnetId='subnet-00000000deadbeef0' (well-formed, nonexistent).
  3. Observe a 201 creating a template instead of ValidationException `SubnetId subnet-00000000deadbeef0 wasn't found`. Re-replay drs.create-replication-configuration-template-nonexistent-subnet-rejected.

Cause 3:
  1. drs.create_launch_configuration_template(targetInstanceTypeRightSizingMethod='BOGUS_VALUE')
  2. Observe error code ValidationException instead of BadRequestException / `Invalid request body`. Re-replay drs.create-launch-configuration-template-invalid-enum-rejected.

Cause 4:
  1. drs.tag_resource(resourceArn='arn:aws:drs:<region>:<account>:recovery-plan/plan-00000000000000000000000000000000', tags={'k':'v'}), then drs.list_tags_for_resource(resourceArn=<same>) and drs.untag_resource(resourceArn=<same>, tagKeys=['k']).
  2. Observe HTTP 501 NotImplemented from each instead of NotFoundException / `Resource was not found`; the same three calls against a real DRS resource also answer 501. Re-replay drs.tag-operations-not-found-on-nonexistent-recovery-plan.
