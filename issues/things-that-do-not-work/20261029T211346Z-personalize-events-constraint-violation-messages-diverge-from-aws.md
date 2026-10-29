Issue-Source: trace-sweep-divergence
Issue-Trace: personalize-events-put-action-interactions-invalid-input
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: personalize-events-put-action-interactions-invalid-input personalize-events-put-operations-invalid-input
Issue-Shape-Id: com.amazonaws.personalizeevents#AmazonPersonalizeEvents
Issue-Key: personalize-events|PutActionInteractions|4xx->4xx|$.message
Issue-Key: personalize-events|PutItems|4xx->4xx|$.message
Issue-Key: personalize-events|PutUsers|4xx->4xx|$.message
Issue-Key: personalize-events|PutActions|4xx->4xx|$.message
Issue-Commit: 0622c476c3133f3ae7e7fc8f4e228880242e86b0

# personalize-events constraint-violation messages diverge from AWS: value echoed, pattern anchors kept, and only the first violation reported

> _Note: this issue was identified against the code base at commit `0622c476c3133f3ae7e7fc8f4e228880242e86b0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutActionInteractions

## Description
The request-validation stage of personalize-events renders AWS's constraint-violation message in its own wording: it echoes the rejected value, prints the model's regex with its ^ and $ anchors, and stops at the first violated member instead of reporting all of them. Status (400) and error class (InvalidInputException) are correct in every cause below; the message body is not. FOUR causes, one per diverging cluster.

Cause 1 of 4 - PutActionInteractions, both required members absent (fixture personalize-events-put-action-interactions-invalid-input, step task#1).
AWS: Smithy: PutActionInteractions.trackingId @required (StringType, @length 1..256) and PutActionInteractions.actionInteractions @required (ActionInteractionsList, @length 1..10); service-common error InvalidInputException (httpError 400). AWS aggregates EVERY violated member into one message and counts them: the fixed text is "<N> validation error[s] detected: " followed by one clause per violation, joined with "; ", each clause reading "Value at '<member>' failed to satisfy constraint: Member must not be null". The variable parts are N (the number of violations) and the member names taken from the request; nothing else varies. For a request omitting both members the recorded response is "2 validation errors detected: Value at 'actionInteractions' failed to satisfy constraint: Member must not be null; Value at 'trackingId' failed to satisfy constraint: Member must not be null".
Emulator: 400 with "1 validation error detected: Value null at 'trackingId' failed to satisfy constraint: Member must not be null" - only one of the two violations is reported, the count is therefore wrong, and the clause inserts the literal word "null" between "Value" and "at", which is not in AWS's fixed text.
Why: a client that parses the violation list to tell the user which fields to fix is told about one field when two are wrong, so a corrected retry fails again on the member that was never reported; a client matching AWS's clause text ("Value at '...' failed") does not match "Value null at '...' failed" at all.
Fix: the shared constraint-violation renderer and aggregator in the server runtime's validate stage of the request walk (deserialize -> validate -> handle), not the PutActionInteractions handler - the handler is never reached for these requests.
Traces: personalize-events-put-action-interactions-invalid-input#task#1 (same cluster: task#2, task#3).

Cause 2 of 4 - PutItems, datasetArn violating its @pattern (fixture personalize-events-put-operations-invalid-input, step task#1).
AWS: Smithy: PutItems.datasetArn @required, target Arn, @length 0..256, @pattern "^arn:([a-z\\d-]+):personalize:.*:.*:.+$"; error InvalidInputException (httpError 400). Recorded behavior: the message is "1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>", where the only variable parts are the member name from the request path and the pattern from the model with its leading ^ and trailing $ removed - and the offending value is NOT echoed. Recorded: "1 validation error detected: Value at 'datasetArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:([a-z\\d-]+):personalize:.*:.*:.+".
Emulator: 400 with "1 validation error detected: Value 'not-a-valid-personalize-arn' at 'datasetArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:([a-z\\d-]+):personalize:.*:.*:.+$" - it interpolates the rejected value into the clause and emits the anchored pattern verbatim. (The quoted ARN is this recording's input value, not part of the template.)
Why: the message is echoed into logs and user-facing errors; echoing the submitted value where AWS does not changes what a caller leaks, and the anchors make the advertised pattern differ from the one AWS publishes, so a client that compiles the reported pattern to pre-validate its own input gets a different expression than AWS's.
Fix: same shared pattern-violation renderer in the validate stage: suppress the value echo and strip the model pattern's anchors before rendering.
Traces: personalize-events-put-operations-invalid-input#task#1.

Cause 3 of 4 - PutUsers, datasetArn violating its @pattern (fixture personalize-events-put-operations-invalid-input, step task#2).
AWS: Smithy: PutUsers.datasetArn @required, target Arn, @length 0..256, @pattern "^arn:([a-z\\d-]+):personalize:.*:.*:.+$"; error InvalidInputException (400). Same message template and same anchor-stripped, value-free rendering as Cause 2.
Emulator: identical defect to Cause 2 - value echoed, anchors kept.
Why: as Cause 2; it reaches a second operation, so a fix applied only to PutItems leaves PutUsers diverging.
Fix: as Cause 2 (shared renderer) - confirm it covers all three metadata puts.
Traces: personalize-events-put-operations-invalid-input#task#2 (same cluster: task#4).

Cause 4 of 4 - PutActions, datasetArn violating its @pattern (fixture personalize-events-put-operations-invalid-input, step task#3).
AWS: Smithy: PutActions.datasetArn @required, target Arn, @length 0..256, @pattern "^arn:([a-z\\d-]+):personalize:.*:.*:.+$"; error InvalidInputException (400). Same template as Cause 2.
Emulator: identical defect - value echoed, anchors kept.
Why: as Cause 2.
Fix: as Cause 2 (shared renderer).
Traces: personalize-events-put-operations-invalid-input#task#3.

No open issue covers these: the open personalize-events issues concern registry wiring ("Wire the Personalize control plane to the Personalize Events registry", "personalize-events ingestion unreachable: personalize ListSchemas is a 501 stub..."), not the rendering of constraint violations.

## Evidence
Cause 1 (personalize-events-put-action-interactions-invalid-input#task#1, status 400 expected and 400 observed):
  expected $.message = "2 validation errors detected: Value at 'actionInteractions' failed to satisfy constraint: Member must not be null; Value at 'trackingId' failed to satisfy constraint: Member must not be null"
  actual   $.message = "1 validation error detected: Value null at 'trackingId' failed to satisfy constraint: Member must not be null"
  (cluster also covers task#2, task#3)

Causes 2-4 (personalize-events-put-operations-invalid-input#task#1 PutItems, #task#2 PutUsers (+task#4), #task#3 PutActions; status 400 expected and 400 observed in each):
  expected $.message = "1 validation error detected: Value at 'datasetArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:([a-z\\d-]+):personalize:.*:.*:.+"
  actual   $.message = "1 validation error detected: Value 'not-a-valid-personalize-arn' at 'datasetArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:([a-z\\d-]+):personalize:.*:.*:.+$"

Model cited: smithy_query(operation="PutActionInteractions,PutItems,PutUsers,PutActions") on services/service-com-amazonaws-personalizeevents-amazonpersonalizeevents/smithy-model/com-amazonaws-personalizeevents-amazonpersonalizeevents.json - trackingId/actionInteractions @required; datasetArn @pattern ^arn:([a-z\\d-]+):personalize:.*:.*:.+$; InvalidInputException httpError 400.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus simparity-wire-aws-v1.
2. Cause 1: POST /action-interactions with a JSON body omitting both "trackingId" and "actionInteractions" (fixture personalize-events-put-action-interactions-invalid-input, step task#1). Observe 400 whose $.message counts 1 violation and reads "Value null at 'trackingId' ..."; AWS counts 2 and reads "Value at 'actionInteractions' ...; Value at 'trackingId' ...".
3. Causes 2-4: POST /items, /users and /actions with "datasetArn": "not-a-valid-personalize-arn" plus a one-element items/users/actions list (fixture personalize-events-put-operations-invalid-input, steps task#1, task#2, task#3). Observe each 400 message echoes the submitted ARN and prints the ^...$ anchored pattern; AWS omits the value and the anchors.
4. Re-replay personalize-events-put-action-interactions-invalid-input#task#1 and personalize-events-put-operations-invalid-input#task#1, #task#2, #task#3 to confirm all four causes.
