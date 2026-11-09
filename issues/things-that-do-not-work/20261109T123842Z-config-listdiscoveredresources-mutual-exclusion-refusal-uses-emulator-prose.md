Issue-Source: trace-sweep-divergence
Issue-Trace: config-list-discovered-resources-optional-members
Issue-Step: task#3
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: config-list-discovered-resources-optional-members
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config|ListDiscoveredResources|4xx->4xx|$.message
Issue-Commit: 68689e45c55a2a3c2aeeec91dff9a251e0f23b15

# config ListDiscoveredResources mutual-exclusion refusal uses emulator prose instead of AWS's ValidationException sentence

> _Note: this issue was identified against the code base at commit `68689e45c55a2a3c2aeeec91dff9a251e0f23b15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDiscoveredResources

## Description
ONE CAUSE must be closed.

Cause 1 of 1: ListDiscoveredResources refuses a request that carries both `resourceIds` and `resourceName` with the right status and error code but the wrong message text (fixture config-list-discovered-resources-optional-members, step task#3).

AWS: ListDiscoveredResources takes a required `resourceType` plus the two mutually exclusive narrowing members `resourceIds` (ResourceIdList) and `resourceName` (ResourceName), and models ValidationException as a 400 client error for an input that 'fails the validation' (Smithy: ListDiscoveredResources input members resourceIds/resourceName; error ValidationException, httpError 400; shape ValidationException member `message`). The model does not state the sentence, so for the wording this is recorded behavior only: AWS answers 400 with the fixed, request-independent sentence 'Both Resource ID and Resource Name cannot be specified in the request'. That message is entirely fixed text — it interpolates nothing from the request (no resource type, no ids, no name), so there is no variable part to substitute and the requirement is the sentence verbatim.

Emulator: same 400 and the same error code, but the message body is its own prose — 'resourceIds and resourceName cannot both be specified in the same ListDiscoveredResources request; narrow by one or the other'. It names the wire member spellings and the operation and appends remediation advice, none of which AWS emits.

Why: a client relying on AWS's behavior reads this refusal's message to decide what it hit. Error-handling that matches or surfaces the AWS sentence (log scrapers, retry/branch logic keyed on the refusal text, SDK wrappers that re-raise a typed error for the mutual-exclusion case, and test suites asserting the refusal) classifies the emulator's answer as an unrecognized validation failure and either mis-branches or surfaces text an operator has never seen in AWS. The status and code match, so nothing else flags the difference.

Fix: the input validation for ListDiscoveredResources (the mutual-exclusion pre-check that raises ValidationException before the read runs) — emit AWS's fixed sentence as the ValidationException message instead of the locally authored explanation.

Related but NOT covering this: the open issue 20261030T003924Z-config-read-paths-over-refuse-fabricate-aggregator-rows.md owns the separate ListDiscoveredResources cluster where the emulator answers 400 on a request AWS answers 200 (empty `resourceIdentifiers`); it says nothing about this refusal's message. 20261028T085841Z-config-5-net-new-behavioral-root-causes-putremediationconfigurations.md and 20261104T093105Z-config-a-running-recorder-never-discovers-an-ec2.md also touch ListDiscoveredResources but cover a custom resource type being refused and the recorder never discovering resources, not this wording.

Traces: config-list-discovered-resources-optional-members#task#3 (ListDiscoveredResources, 1 step in cluster).

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, score_pct 74.5).

Cause 1 — ListDiscoveredResources, config-list-discovered-resources-optional-members#task#3, verdict mismatch, expected_status 400 / actual_status 400 (status and error code agree; only $.message diverges):
  $.message: value_mismatch
    expected: 'Both Resource ID and Resource Name cannot be specified in the request'
    got:      'resourceIds and resourceName cannot both be specified in the same ListDiscoveredResources request; narrow by one or the other'

Message-template analysis: the expected message is 100% fixed text. It contains no value taken from the request or from emulator state — no resourceType, no resource id, no resource name, no limit, no count — so a faithful implementation emits the identical string for every occurrence of this refusal.

Model grounding (services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json): ListDiscoveredResources input { resourceType (required, enum), resourceIds -> ResourceIdList, resourceName -> ResourceName, limit (range 0..100), includeDeletedResources, nextToken }; errors InvalidLimitException, InvalidNextTokenException, NoAvailableConfigurationRecorderException, ValidationException (all errorKind client, httpError 400); ValidationException has a single `message` member (target ErrorMessage) and is documented as covering input that 'fails the validation'.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the wire corpus simparity-wire-aws-v1.
3. Cause 1: fixture config-list-discovered-resources-optional-members, step task#3 — call ListDiscoveredResources with a valid resourceType AND both `resourceIds` and `resourceName` populated in the same request.
4. Observe a 400 ValidationException whose message is the emulator's own sentence rather than AWS's 'Both Resource ID and Resource Name cannot be specified in the request'.
5. Re-replay config-list-discovered-resources-optional-members#task#3 to confirm the fix.
