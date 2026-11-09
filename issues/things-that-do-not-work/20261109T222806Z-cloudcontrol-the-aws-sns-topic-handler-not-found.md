Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol.list-resources-filters-by-type-name cloudcontrol.timestamp-property-precision-preserved
Issue-Shape-Id: com.amazonaws.cloudcontrol#CloudApiService
Issue-Key: cloudcontrol.list-resources-filters-by-type-name|run#22|AssertionError|assert topic_message == expected, f"expected message {expected!r}, got {topic_message!r}"
Issue-Key: cloudcontrol.timestamp-property-precision-preserved|run#20|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 785433b7ada0bbe790b60a1839b6b0233a99f365

# cloudcontrol: the AWS::SNS::Topic handler not-found message served by GetResource omits the "(SDK Attempt Count: <n>)" segment

> _Note: this issue was identified against the code base at commit `785433b7ada0bbe790b60a1839b6b0233a99f365`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResource

## Description
Cause 1 of 1: Cloud Control's relayed AWS::SNS::Topic NotFound handler message is missing one of its fixed segments.

AWS:      GetResource on an AWS::SNS::Topic identifier that no longer exists is refused with ResourceNotFoundException (Smithy: GetResource errors include ResourceNotFoundException, errorKind client, httpError 404). Recorded behavior only for the message text (the Smithy model carries no message template): the Error.Message is the SNS resource handler's relayed refusal and has this shape —
          `AWS::SNS::Topic Handler returned status FAILED: Topic does not exist (Service: Sns, Status Code: 404, Request ID: <request id>) (SDK Attempt Count: <n>) (HandlerErrorCode: NotFound, RequestToken: <request token>)`
          Fixed text: `AWS::SNS::Topic Handler returned status FAILED: Topic does not exist (Service: Sns, Status Code: 404, Request ID: `, `) (SDK Attempt Count: `, `) (HandlerErrorCode: NotFound, RequestToken: `, `)`. Values taken from the call/state: the downstream SNS request id, the SDK attempt count (an integer), and the Cloud Control RequestToken (a UUID). Those three vary per call and are never part of the requirement.

Emulator: serves the right error code and every other segment, but drops the `(SDK Attempt Count: <n>) ` segment entirely, so the message reads `... Request ID: <id>) (HandlerErrorCode: NotFound, RequestToken: <token>)` — the Request ID group is followed directly by the HandlerErrorCode group.

Why:      a client that parses the relayed handler message (the documented way to recover the downstream service, status code, request id and SDK attempt count of a handler failure) finds no attempt count, and any comparison or regex written against AWS's handler-message shape fails on a message that is otherwise correct. Both failing checks reach exactly this point: the resource lifecycle itself succeeded and only the not-found wording diverged.

Fix:      the handler-refusal message builder on the cloudcontrol GetResource path (where a downstream handler failure is rendered into ResourceNotFoundException.Message for AWS::SNS::Topic) must emit the `(SDK Attempt Count: <n>)` group between the Request ID group and the HandlerErrorCode group.

Traces:   cloudcontrol.list-resources-filters-by-type-name (asserts the message after DeleteResource of the topic); cloudcontrol.timestamp-property-precision-preserved (same assertion after its own topic delete).

## Evidence
Expected (both checks rebuild the per-call values from the served message and compare the rest exactly):
  'AWS::SNS::Topic Handler returned status FAILED: Topic does not exist (Service: Sns, Status Code: 404, Request ID: <id>) (SDK Attempt Count: <n>) (HandlerErrorCode: NotFound, RequestToken: <token>)'

Actual (cloudcontrol.timestamp-property-precision-preserved):
  'AWS::SNS::Topic Handler returned status FAILED: Topic does not exist (Service: Sns, Status Code: 404, Request ID: aa57dac3-e7aa-41c7-a1bd-14287226fb23) (HandlerErrorCode: NotFound, RequestToken: c5a25a2e-cb86-4fe6-aea3-e65af1bb9421)'

Actual (cloudcontrol.list-resources-filters-by-type-name):
  'AWS::SNS::Topic Handler returned status FAILED: Topic does not exist (Service: Sns, Status Code: 404, Request ID: e1c12d84-3686-4913-8c1e-b72da50c0193) (HandlerErrorCode: NotFound, RequestToken: f3063ad3-baa3-4808-8330-d1efdacdaa83)'

The error code is right in both cases (ResourceNotFoundException); only the '(SDK Attempt Count: <n>)' group of the message is absent. The ids/tokens above are per-run values, not requirements.

## Steps to Reproduce
1. cloudcontrol CreateResource TypeName=AWS::SNS::Topic DesiredState={"TopicName":"<name>"} and poll GetResourceRequestStatus to OperationStatus=SUCCESS; keep the returned Identifier (the topic ARN).
2. cloudcontrol DeleteResource TypeName=AWS::SNS::Topic Identifier=<that ARN>, poll to SUCCESS.
3. cloudcontrol GetResource TypeName=AWS::SNS::Topic Identifier=<that ARN>.
4. Observe ResourceNotFoundException whose Error.Message lacks the '(SDK Attempt Count: <n>)' group that AWS places between the 'Request ID: <id>)' group and the '(HandlerErrorCode: NotFound, RequestToken: <token>)' group.
