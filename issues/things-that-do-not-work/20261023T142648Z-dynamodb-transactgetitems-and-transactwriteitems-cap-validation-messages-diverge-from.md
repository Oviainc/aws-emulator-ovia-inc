Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-batch-caps-and-unprocessed
Issue-Step: task#11
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-batch-caps-and-unprocessed
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|TransactGetItems|4xx->4xx|$.message
Issue-Key: dynamodb|TransactWriteItems|4xx->4xx|$.message
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# TransactGetItems and TransactWriteItems cap-validation messages diverge from AWS's constraint report

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TransactWriteItems

## Description
Both transactional APIs reject an over-cap TransactItems list with the right status and error code, but the validation message does not match AWS's constraint report. 2 causes, all dynamodb.

Cause 1 of 2: TransactGetItems with more TransactItems than the modeled maximum reports the constraint violation with the wrong message.
AWS: Smithy: TransactGetItems input member TransactItems is required and targets TransactGetItemList with @length min 1 max 100, so a list above 100 entries is a constraint violation; the operation models no success for it. The recording shows HTTP 400 and a message in AWS's canonical constraint-report form, with the rejected value serialized into the message and the member named in lower camel: "1 validation error detected: Value '[TransactGetItem(get=Get(key={pk=AttributeValue(s=k0, n=null, b=null, ...)}, tableName=sim2real-probe-dynamodb-batchcap-2bcefecd, projectionExpression=null, expressionAttributeNames=null, workloadProfileName=null)), TransactGetItem(...), ...]' at 'transactItems' failed to satisfy constraint: ...". The replay's detail truncates the expected text inside that serialized list; the bound it reports against is the model's @length max 100.
Emulator: HTTP 400 with the matching error code but a different $.message (the replay flags only $.message as diverging). Hypothesis: it is the same constraint-violation renderer already filed in open issue 20261023T105249Z-dynamodb-request-validation-gaps-rejected-value-omitted-from-messages.md, which drops the rejected value and uses the input-member spelling instead of the lower-camel wire path - that issue does not carry this cluster's replay key, so this path is unowned and must be re-replayed on its own.
Why: the constraint report is the only machine-readable part of a DynamoDB validation failure - clients and test harnesses read the rejected value and the member path out of it to find which element of which member was refused. With a different message the caller of a batched transaction cannot tell the cap violation from any other ValidationException.
Fix: the shared constraint-violation message renderer used by TransactGetItems' input validation - emit "1 validation error detected: Value '<serialized rejected value>' at 'transactItems' failed to satisfy constraint: <the @length bound>".
Traces: dynamodb-batch-caps-and-unprocessed#task#11.

Cause 2 of 2: TransactWriteItems has the same defect on its own TransactItems list.
AWS: Smithy: TransactWriteItems input TransactItems is required and targets TransactWriteItemList with @length min 1 max 100. Recorded 400 message, same canonical form, serializing the rejected list of TransactWriteItem(conditionCheck=null, put=Put(item={...}, tableName=sim2real-probe-dynamodb-batchcap-2bcefecd, ...), delete=null, update=null) entries at member 'transactItems'.
Emulator: HTTP 400 with the matching error code and a diverging $.message, as in cause 1.
Why: same as cause 1, on the write path, where a client retrying a too-large transaction needs to know the cap was the reason.
Fix: same shared constraint-violation renderer reached from TransactWriteItems' input validation; one fix should cover both causes, but both fixture steps must be re-replayed.
Traces: dynamodb-batch-caps-and-unprocessed#task#12.

## Evidence
Divergence 14 (TransactGetItems, 400->400, fixture dynamodb-batch-caps-and-unprocessed#task#11): $.message value_mismatch - expected "1 validation error detected: Value '[TransactGetItem(get=Get(key={pk=AttributeValue(s=k0, n=null, b=null, iNT=null, dECIMAL=null, dOUBLE=null, sS=null, nS=null, bS=null, iNTSET=null, dECIMALSET=null, dOUBLESET=null, m=null, l=null, nULL=null, bOOL=null)}, tableName=sim2real-probe-dynamodb-batchcap-2bcefecd, projectionExpression=null, expressionAttributeNames=null, workloadProfileName=null)), TransactGetItem(... k1 ... k5 ...)" (the replay detail is truncated inside the serialized list); the emulator's $.message differs. Status 400 expected and 400 actual; no other key diverges.
Divergence 15 (TransactWriteItems, 400->400, fixture dynamodb-batch-caps-and-unprocessed#task#12): $.message value_mismatch - expected "1 validation error detected: Value '[TransactWriteItem(conditionCheck=null, put=Put(item={pk=AttributeValue(s=k0, ...)}, tableName=sim2real-probe-dynamodb-batchcap-2bcefecd, conditionExpression=null, expressionAttributeNames=null, expressionAttributeValues=null, returnValuesOnConditionCheckFailure=null, workloadProfileName=null), delete=null, update=null), TransactWriteItem(... k1 ... k3 ...)" (likewise truncated); the emulator's $.message differs. Status 400 expected and 400 actual.
Model: smithy_query(operation=TransactGetItems) -> TransactItems required, TransactGetItemList @length{min:1,max:100}; smithy_query(operation=TransactWriteItems) -> TransactItems required, TransactWriteItemList @length{min:1,max:100}.
Both clusters are reported with empty owned_by and empty stub_path by the replay.

## Steps to Reproduce
1. Replay dynamodb-batch-caps-and-unprocessed#task#11: TransactGetItems with a TransactItems list above the modeled maximum of 100 -> 400 whose $.message is not AWS's "1 validation error detected: Value '<serialized list>' at 'transactItems' failed to satisfy constraint: ..." report.
2. Replay dynamodb-batch-caps-and-unprocessed#task#12: TransactWriteItems with the same over-cap list -> same message divergence on the write path.
3. Compare the emulator's rendered $.message for both steps with the recorded expected text in the replay detail for divergences 14 and 15.
