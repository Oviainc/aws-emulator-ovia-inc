Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__cognito-identity.identity-pool-data-events-are-delivered-only-once-a-selector-names-them
Issue-Key: cloudtrail__cognito-identity.identity-pool-data-events-are-delivered-only-once-a-selector-names-them|run#40|Timeout|time.sleep(_DELIVERY_POLL_S)
Issue-Commit: 699db40da250628f4cc12f3dc175d789bc27ad1e

# cloudtrail__cognito-identity: an accepted AWS::Cognito::IdentityPool data-event selector never produces a delivered log file, so identity-pool data events are undetectable

> _Note: this issue was identified against the code base at commit `699db40da250628f4cc12f3dc175d789bc27ad1e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEventSelectors

## Description
One cause.

Cause 1 of 1 — a trail whose advanced event selector names eventCategory=Data with resources.type=AWS::Cognito::IdentityPool is accepted and reported as configured, but no data-event log file is ever delivered to the trail's S3 bucket for the identity-pool calls that match it (GetId, GetOpenIdToken), so neither the 'delivered once a selector names them' leg nor the 'withheld while no selector names them' leg of the contract can be observed.
AWS: Smithy (services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/smithy-model/com-amazonaws-cloudtrail-cloudtrail-20131101.json) models PutEventSelectors with AdvancedEventSelectors and the AdvancedFieldSelector fields (eventCategory, resources.type, resources.ARN) the check installs; the per-record content the check then grades is recorded behavior only (measured against real AWS): once the selector is in place, each matching Cognito identity data-plane call is delivered as a gzipped log record under the trail's bucket/prefix with eventCategory 'Data', eventSource cognito-identity.amazonaws.com, the resource typed AWS::Cognito::IdentityPool (NOT AWS::CognitoIdentity::...), and secret request/response material replaced by the literal HIDDEN_DUE_TO_SECURITY_REASONS; calls on a pool that no selector names are NOT delivered.
Emulator: nothing is delivered. The check warms the trail, repeats its identity-pool calls every round and polls the log bucket for the matching records; no record (and no log object) ever appears, so it is still polling when the harness's 60s cap ends it ('Check timed out after 60.0s'). The check declares DATAPLANE_MARKER = 'DATAPLANE_UNIMPLEMENTED' for exactly this absence, so this is a data-plane fidelity failure, not a connection error.
Why: data events are the only audit evidence that a specific identity pool was used to vend credentials. With delivery absent, the selector is write-only decoration: an operator cannot prove a pool's GetId/GetOpenIdToken activity happened, cannot prove the redaction of token material ever occurred, and — worse — cannot distinguish 'withheld because no selector names this pool' from 'nothing is ever delivered', so a scoping mistake that over-collects in AWS silently passes here.
Fix: data-event capture and trail log-file delivery — the Cognito identity data-plane operations must feed the shared event-recording seam with eventCategory Data and the AWS::Cognito::IdentityPool resource, and the trail's logging state must evaluate the stored advanced event selectors against those records and write the matching ones as gzipped objects into the trail's configured S3 bucket/prefix (withholding the non-matching ones).
Traces: cloudtrail__cognito-identity.identity-pool-data-events-are-delivered-only-once-a-selector-names-them. NOTE: the open issue 20261027T003725Z-cloudtrail-delivers-no-data-event-log-files-for.md covers the same delivery gap for the sibling check obs-cognito-identity-2-...; the replay report did NOT mark this check as owned by it, so this check's replay key is unclaimed and it is filed here — close it together with that issue if the fix is shared, but re-replay the trace above.

## Evidence
cloudtrail__cognito-identity.identity-pool-data-events-are-delivered-only-once-a-selector-names-them — verdict 'mismatch', detail 'Check timed out after 60.0s'. The check's own budgets are _WARM_BUDGET_S = 540.0, _DELIVERY_BUDGET_S = 660.0, _DELIVERY_POLL_S = 2.0, _ABSENCE_CEILING_S = 150.0, all far above the harness's 60s cap, and it carries DATAPLANE_MARKER = 'DATAPLANE_UNIMPLEMENTED' for the 'no log file is ever delivered' case. Its expectations name _POOL_RESOURCE_TYPE = 'AWS::Cognito::IdentityPool', _DATA_EVENT_NAMES = ('GetId','GetOpenIdToken') and _REDACTED = 'HIDDEN_DUE_TO_SECURITY_REASONS'; none of those assertions is reached because no record arrives. PutEventSelectors itself accepted the selector (the check proceeds past it).

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Create an S3 bucket with the CloudTrail delivery policy and CreateTrail + StartLogging on it; create two Cognito identity pools A and B.
3. Call cognito-identity GetId/GetOpenIdToken against pool A while NO selector names it, then PutEventSelectors(TrailName=t, AdvancedEventSelectors=[{Name:..., FieldSelectors:[{Field:'eventCategory',Equals:['Data']},{Field:'resources.type',Equals:['AWS::Cognito::IdentityPool']},{Field:'resources.ARN',Equals:[<pool B ARN>]}]}]) and repeat the calls against pool B.
4. Poll ListObjectsV2 on the log bucket and gunzip the delivered objects. Expect records for pool B's GetId/GetOpenIdToken (eventCategory 'Data', resource type AWS::Cognito::IdentityPool, secret material rendered as HIDDEN_DUE_TO_SECURITY_REASONS) and no record for pool A; observe that the bucket never receives any data-event log object at all.
