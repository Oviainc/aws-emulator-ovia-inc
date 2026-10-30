Issue-Source: trace-sweep-divergence
Issue-Trace: backup-gateway-virtual-machine-inventory-and-sync
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-gateway-virtual-machine-inventory-and-sync backup-gateway-tag-resource-lifecycle-round-trip backup-gateway-untag-nonexistent-key-noop
Issue-Shape-Id: com.amazonaws.backupgateway#BackupOnPremises_v20210101
Issue-Key: backup-gateway|StartVirtualMachinesMetadataSync|4xx->2xx|$.Message
Issue-Key: backup-gateway|StartVirtualMachinesMetadataSync|4xx->2xx|$.__type
Issue-Key: backup-gateway|StartVirtualMachinesMetadataSync|4xx->2xx|status
Issue-Key: backup-gateway|ListTagsForResource|2xx->2xx|$.Tags[*]
Issue-Commit: 2216fbd2f00c4c2b6b2036585ad12df5e43cbdbf

# backup-gateway hypervisor sync precondition missing and system tags omitted

> _Note: this issue was identified against the code base at commit `2216fbd2f00c4c2b6b2036585ad12df5e43cbdbf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartVirtualMachinesMetadataSync

## Description
TWO net-new root causes where the emulator answers HTTP 200 but the recorded response carries more than it serves: a precondition refusal that is absent altogether, and the AWS-assigned system tags a tag read must report. Both must be closed before this issue is submitted.

Cause 1 of 2 -- StartVirtualMachinesMetadataSync against a hypervisor that has no property mappings configured is accepted instead of refused.
AWS: HTTP 400 with AccessDeniedException (recorded __type 'com.amazon.coral.service#AccessDeniedException', i.e. the AccessDeniedException class) and Message exactly 'HypervisorPropertyMappings is not configured for the Hypervisor' -- entirely fixed text, no variable part taken from the request. Smithy: StartVirtualMachinesMetadataSync errors = AccessDeniedException (@httpError 403), ResourceNotFoundException (@httpError 404), plus the common ValidationException; AccessDeniedException$ErrorCode is modeled required, but the recording asserts only __type and Message, so the status 400 for this class and the sentence are recorded behavior only. Note the status: AWS answers this refusal 400, NOT the 403 the model's @httpError declares.
Emulator: HTTP 200 with a normal StartVirtualMachinesMetadataSync output; no refusal at all, so the sync is accepted and the inventory published for a hypervisor AWS would not sync.
Why: a client that configures property mappings only when the sync is refused never learns the hypervisor is unconfigured; it sees a successful sync, then reads an inventory whose provenance AWS would never have produced, and its error-handling branch for AccessDeniedException is dead code.
History: this precondition was previously implemented as a 403 AccessDeniedException and then removed by the fixes for 20261024T030827Z-backup-gateway-startvirtualmachinesmetadatasync-error-response.md and 20261024T084427Z-backup-gateway-startvirtualmachinesmetadatasync-wrong-behavior.md (both resolved). The recorded wire shows the gate is REAL; what was wrong was its status (403) -- AWS refuses with AccessDeniedException at HTTP 400 and the sentence above. Re-adding it as a 403 would re-break those issues.
Fix: StartVirtualMachinesMetadataSync's handler must check the hypervisor's property mappings (the state the PutHypervisorPropertyMappings path writes) before starting a sync and refuse with AccessDeniedException at status 400 carrying AWS's fixed sentence; the status override belongs beside the crate's other refusal-status choices in crate::state rather than being hand-written in the handler.
Traces: backup-gateway-virtual-machine-inventory-and-sync#task#2.

Cause 2 of 2 -- ListTagsForResource omits the AWS-assigned system tags, so a tag read reports only the client's own tags.
AWS: a Backup Gateway resource created through the API carries AWS-assigned system tags, and ListTagsForResource reports them AHEAD of the client-written tags: key 'aws:createdBy' whose value is derived from the calling identity (for an IAM user, 'IAMUser:<access-key-id>:<user-name>' -- the identity parts come from the caller, they are not literal text, and the recording redacts the key id), and key 'aws:createdDate' whose value is the resource's creation timestamp (the recording does not assert the value). The requirement is the presence, the key names and the position of these two entries, not the per-account/per-run values. Recorded behavior only: the Smithy model documents no system tags (ListTagsForResource output = ResourceArn + Tags).
Emulator: returns ONLY the tags a client wrote -- the first two list positions hold the client's own pairs and no 'aws:' entry appears anywhere, on both a tagged-resource round trip and after an untag of a key that was never set.
Why: a client that reads provenance from aws:createdBy/aws:createdDate (console-equivalent displays, cost/ownership tooling, tag-policy checks) finds nothing; a client that budgets against the 50-tag limit or asserts the full tag set of a resource sees a different set from AWS, and the two system keys are also reserved, so code that filters 'aws:'-prefixed keys is never exercised.
Fix: the crate's tag state (crate::state -- the tag map the create/import paths populate and that resource_tags() resolves for every resource type) must stamp the two AWS-assigned system tags when a resource is created, and ListTagsForResource must report them ahead of the client tags. The caller identity for aws:createdBy comes from the signed request, not from a constant.
Traces: backup-gateway-tag-resource-lifecycle-round-trip#task#4 (and the same cause at #task#6 and backup-gateway-untag-nonexistent-key-noop#task#5).

## Evidence
Replay of trace set simparity-wire-aws-v1, service=backup-gateway:

1) backup-gateway-virtual-machine-inventory-and-sync#task#2 StartVirtualMachinesMetadataSync -- status: expected 400, got 200; $.__type: missing (expected 'com.amazon.coral.service#AccessDeniedException', got None); $.Message: missing (expected 'HypervisorPropertyMappings is not configured for the Hypervisor', got None). The emulator served a success body where the recording is a refusal.

2) backup-gateway-tag-resource-lifecycle-round-trip#task#4 ListTagsForResource (cluster of 3 steps: also #task#6 and backup-gateway-untag-nonexistent-key-noop#task#5) -- status 200 matched; $.Tags[0]: missing (expected {'Key': 'aws:createdBy', 'Value': 'IAMUser:[REDACTED:aws-access-key]:emulator'}, got {'Key': 'sim2real-k1', 'Value': 'v1'}); $.Tags[1]: missing (expected {'Key': 'aws:createdDate', 'Value': '<shaped-not-asserted>'}, got {'Key': 'sim2real-k2', 'Value': 'v2'}). The emulator's list is the client's tags only; the two aws:-prefixed entries are absent, and the client tags are therefore shifted to positions 0 and 1. The recorded aws:createdBy value is caller-identity-derived (access key id redacted) and aws:createdDate is not asserted -- the keys, their presence and their leading position are the requirement.

Smithy (services/service-com-amazonaws-backupgateway-backuponpremises-v20210101/smithy-model/...json): StartVirtualMachinesMetadataSync errors AccessDeniedException(403)/ResourceNotFoundException(404)/ValidationException(400), AccessDeniedException$ErrorCode required; ListTagsForResource output members ResourceArn + Tags, errors ResourceNotFoundException(404)/ValidationException(400).

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it; replay trace set simparity-wire-aws-v1.
2. Cause 1: ImportHypervisorConfiguration to create a hypervisor, then call StartVirtualMachinesMetadataSync{HypervisorArn: <that ARN>} WITHOUT calling PutHypervisorPropertyMappings first. Expect HTTP 400 AccessDeniedException with Message 'HypervisorPropertyMappings is not configured for the Hypervisor'; observe HTTP 200 and a started sync. (fixture backup-gateway-virtual-machine-inventory-and-sync step task#2)
3. Cause 2: create/import a resource, TagResource it with two client tags, then ListTagsForResource on it. Expect the AWS-assigned 'aws:createdBy' and 'aws:createdDate' entries ahead of the client tags; observe only the two client tags and no 'aws:'-prefixed key. Repeat after UntagResource with a key that was never set (a no-op) -- same omission. (fixtures backup-gateway-tag-resource-lifecycle-round-trip steps task#4 and task#6; backup-gateway-untag-nonexistent-key-noop step task#5)
