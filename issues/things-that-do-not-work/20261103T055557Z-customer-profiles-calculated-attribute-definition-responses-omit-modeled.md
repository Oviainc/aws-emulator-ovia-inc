Issue-Source: trace-sweep-divergence
Issue-Trace: customer-profiles-create-calculated-attribute-definition-roundtrip
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: customer-profiles-create-calculated-attribute-definition-roundtrip
Issue-Shape-Id: com.amazonaws.customerprofiles#CustomerProfiles_20200815
Issue-Key: customer-profiles|CreateCalculatedAttributeDefinition|2xx->2xx|$.Conditions
Issue-Key: customer-profiles|CreateCalculatedAttributeDefinition|2xx->2xx|$.DisplayName
Issue-Key: customer-profiles|CreateCalculatedAttributeDefinition|2xx->2xx|$.Filter
Issue-Key: customer-profiles|CreateCalculatedAttributeDefinition|2xx->2xx|$.Readiness.Message
Issue-Key: customer-profiles|CreateCalculatedAttributeDefinition|2xx->2xx|$.Tags
Issue-Key: customer-profiles|GetCalculatedAttributeDefinition|2xx->2xx|$.Conditions
Issue-Key: customer-profiles|GetCalculatedAttributeDefinition|2xx->2xx|$.DisplayName
Issue-Key: customer-profiles|GetCalculatedAttributeDefinition|2xx->2xx|$.Filter
Issue-Key: customer-profiles|GetCalculatedAttributeDefinition|2xx->2xx|$.Readiness.Message
Issue-Key: customer-profiles|GetCalculatedAttributeDefinition|2xx->2xx|$.Tags
Issue-Commit: 39390cfa3000e408bc0d1db242197466d1e66569

# customer-profiles calculated-attribute definition responses omit modeled members and misword Readiness (2 causes)

> _Note: this issue was identified against the code base at commit `39390cfa3000e408bc0d1db242197466d1e66569`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCalculatedAttributeDefinition

## Description
Two net-new wire causes sharing ONE fault: the calculated-attribute definition projection used by both the create and the get path drops four modeled members and composes its own Readiness message, so a definition cannot be read back as it was written.

Cause 1 of 2: CreateCalculatedAttributeDefinition's 200 response omits DisplayName, Conditions, Filter and Tags, and words Readiness.Message itself.
AWS: Smithy: CreateCalculatedAttributeDefinitionResponse models CalculatedAttributeName, DisplayName (`displayName`, @length 1..255, @pattern ^[a-zA-Z_][a-zA-Z_0-9-\s]*$), Description, AttributeDetails, Conditions, Filter, Statistic, CreatedAt, LastUpdatedAt, UseHistoricalData, Status (ReadinessStatus), Readiness and Tags; Readiness is a structure {ProgressPercentage, Message} documented as 'Information indicating if the Calculated Attribute is ready for use by confirming all historical data has been processed and reflected.' Recorded response: 200 that ECHOES DisplayName as supplied in the request, carries Conditions present as a structure with its unset members null ({ObjectCount: null, Range: null, Threshold: null}) and Filter likewise ({Groups: null, Include: null}) rather than omitting them, carries Tags present and empty ({}) when the request supplied none, and whose Readiness.Message is the fixed sentence 'Calculated Attribute is ready to use.' - that sentence is entirely fixed text and interpolates nothing.
Emulator: 200 with DisplayName, Conditions, Filter and Tags all absent from the body, and Readiness.Message = 'All historical data has been processed.' - a sentence of the emulator's own composition.
Why: the create response is what a caller stores as the definition it just made; with DisplayName dropped the human label vanishes, with Conditions/Filter absent a caller cannot tell 'no conditions configured' from 'conditions not reported', an SDK caller indexing response['Tags'] raises KeyError, and any client gating on the readiness sentence never sees the ready-to-use wording.
Fix: the CreateCalculatedAttributeDefinition response projection (and the definition state it serializes) - echo DisplayName, always serialize Conditions, Filter and Tags (present, with unset members null / the map empty), and emit AWS's fixed readiness sentence for the ready state. (Open stub issue 20261030T101909Z-customer-profiles-createcalculatedattributedefinition-stub.md exists, but the replay shows the operation SERVING a 200 - this cause is the projection of that served response.)
Traces: customer-profiles-create-calculated-attribute-definition-roundtrip#task#3.

Cause 2 of 2: GetCalculatedAttributeDefinition drops the same members on the read path, so the definition does not round-trip.
AWS: Smithy: GetCalculatedAttributeDefinitionResponse (GET /domains/{DomainName}/calculated-attributes/{CalculatedAttributeName}) models the same member set - CalculatedAttributeName, DisplayName, Description, CreatedAt, LastUpdatedAt, Statistic, Filter, Conditions, AttributeDetails, UseHistoricalData, Status, Readiness, Tags. Recorded response: 200 identical in shape to the create response - DisplayName as created, Conditions and Filter present with null members, Tags present and empty, Readiness.Message 'Calculated Attribute is ready to use.'
Emulator: the same four members absent and the same emulator-authored readiness sentence.
Why: create-then-get is how a client verifies a definition; because both sides drop the same members the omission is invisible against the emulator, so a config-drift check (compare what GET returns with what was requested) passes locally and fails against AWS.
Fix: the GetCalculatedAttributeDefinition response projection, sharing the definition serializer corrected in Cause 1.
Traces: customer-profiles-create-calculated-attribute-definition-roundtrip#task#4.

No open issue covers these: 20261031T024257Z-customer-profiles-upload-job-responses-diverge-getuploadjob-omits.md owns the analogous upload-job projection gaps (GetUploadJob/CreateUploadJob) and the replay marks those steps as owned by it, but the calculated-attribute steps above are unowned.

## Evidence
Replay of simparity-wire-aws-v1 (emulator_up=true, bootstrap_signal=false). Both clusters are verdict=mismatch with expected_status == actual_status == 200, same five diverging keys.

Cause 1 - fixture customer-profiles-create-calculated-attribute-definition-roundtrip step task#3, CreateCalculatedAttributeDefinition, 200 -> 200:
  $.Conditions: missing (expected {'ObjectCount': None, 'Range': None, 'Threshold': None}, got None)
  $.DisplayName: missing (expected the DisplayName supplied in the request - 'sim2real-probe-cad-41f7eae6' in this recording - got None)
  $.Filter: missing (expected {'Groups': None, 'Include': None}, got None)
  $.Readiness.Message: value_mismatch (expected 'Calculated Attribute is ready to use.', got 'All historical data has been processed.')
  $.Tags: missing (expected {}, got None)

Cause 2 - same fixture step task#4, GetCalculatedAttributeDefinition, 200 -> 200: the identical five keys diverge with the identical expected/actual values.

The DisplayName in the recording is that run's generated name; the requirement is that it is echoed. The readiness sentence is fixed text in full.

## Steps to Reproduce
CreateDomain(D, DefaultExpirationDays=366); define the object type the attribute reads from; then:
Cause 1: CreateCalculatedAttributeDefinition(DomainName=D, CalculatedAttributeName=C, DisplayName=N, Statistic=COUNT, AttributeDetails=<items over the defined object type>, no Conditions, no Filter, no Tags) -> expect 200 whose body echoes DisplayName == N, carries Conditions {'ObjectCount': null, 'Range': null, 'Threshold': null}, Filter {'Groups': null, 'Include': null}, Tags {} and Readiness.Message 'Calculated Attribute is ready to use.'; observe DisplayName/Conditions/Filter/Tags absent and Readiness.Message 'All historical data has been processed.'
Cause 2: GetCalculatedAttributeDefinition(DomainName=D, CalculatedAttributeName=C) -> expect the same five members with the same values; observe the same four absent and the same reworded readiness sentence.

Re-replay fixture customer-profiles-create-calculated-attribute-definition-roundtrip to confirm.
