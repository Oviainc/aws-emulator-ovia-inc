Issue-Source: trace-sweep-divergence
Issue-Trace: connecthealth-get-subscription-not-found
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: connecthealth-get-subscription-not-found connecthealth-subscription-error-paths
Issue-Shape-Id: com.amazonaws.connecthealth#ConnectHealth
Issue-Key: connecthealth|GetSubscription|4xx->4xx|$.Message
Issue-Key: connecthealth|ListSubscriptions|4xx->4xx|$.Message
Issue-Key: connecthealth|ActivateSubscription|4xx->4xx|$.Message
Issue-Commit: 6136e2766a973f12a1067e5538f7a2dced441244

# ConnectHealth error responses omit the Message member entirely

> _Note: this issue was identified against the code base at commit `6136e2766a973f12a1067e5538f7a2dced441244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSubscription

## Description
The ConnectHealth error serializer produces the right HTTP status for the subscription error paths but emits no 'Message' member in the body, so the modeled exception message is unreadable to a client. This bundle carries 3 causes; all 3 must be closed.

Cause 1 of 3 - GetSubscription 404 ResourceNotFoundException body carries no Message (fixture connecthealth-get-subscription-not-found, step task#4).
AWS: Smithy GetSubscription errors include ResourceNotFoundException (errorKind client, httpError 404, member message -> String, documented 'This error is thrown when the requested resource is not found'). The recorded 404 response carries the message under the wire key 'Message' and the message is built from the request - it names the domain id and subscription id that were not found (recorded example: 'Subscription not found: dom-5c7cbb2c13d48666/sub-000000000000000000000'). That request-derived content is the requirement; the exact phrasing is not.
Emulator: answers 404 (status correct) but $.Message is absent from the body - the asserted member does not exist under that key at all.
Why: an SDK or raw HTTP client reading the error's message field gets null, so the operator-facing log line and any message-based triage lose which subscription was missing. A client distinguishing a missing domain from a missing subscription on the same 404 has nothing to read.
Fix: the ConnectHealth error serializer - emit the exception's message member on the wire under the key the service uses ('Message'), populated with the requested domain/subscription identifiers.

Cause 2 of 3 - ListSubscriptions 400 ValidationException body carries no Message (fixture connecthealth-subscription-error-paths, step task#3).
AWS: Smithy ListSubscriptionsInput.maxResults -> Integer with @range min 1 max 100, and ListSubscriptions declares ValidationException (errorKind client, httpError 400, member message -> String, documented 'This error is thrown when the client supplies invalid input to the API'). The message rule here IS known: it is AWS's standard constraint-violation message, built from the request - the offending value, the member path and the violated constraint. Recorded: "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100".
Emulator: answers 400 (status correct) but $.Message is absent.
Why: constraint violations are developer-facing; with no message the caller sees a bare 400 and cannot tell which query parameter it got wrong, which is the entire value of the modeled ValidationException. The rejection itself works - only the diagnostic is lost.
Fix: the ListSubscriptions input validation path plus the error serializer - render the @range violation into the standard 'N validation error(s) detected: Value '<v>' at '<member>' failed to satisfy constraint: <constraint>' message and serialize it as the Message member.

Cause 3 of 3 - ActivateSubscription 404 ResourceNotFoundException body carries no Message (fixture connecthealth-subscription-error-paths, step task#4).
AWS: Smithy ActivateSubscription errors include ResourceNotFoundException (httpError 404, member message -> String). The recorded 404 carries the request-derived message naming the domain and subscription that could not be activated (recorded example: 'Subscription not found: dom-8c080cffb3fb1570/sub-d4d4d4d4d4d4d4d4d4d4d').
Emulator: answers 404 (status correct) but $.Message is absent.
Why: as Cause 1, on the mutating path - a client retrying an activation cannot tell from the response whether the domain or the subscription is the missing one.
Fix: as Cause 1 - the shared ConnectHealth error serializer; listed as its own cause because it is reached through ActivateSubscription's error mapping and must be re-replayed separately.

Note: two further 404 clusters in this corpus (connecthealth-domain-lifecycle#task#4 GetDomain and #task#5 DeleteDomain) differ ONLY in the phrasing of a message that IS present and DOES name the requested domain id; those are deliberately NOT filed, since no documented rule fixes the wording. This issue is about the member being absent, not about wording.

Traces: connecthealth-get-subscription-not-found#task#4 (GetSubscription, 404), connecthealth-subscription-error-paths#task#3 (ListSubscriptions, 400), connecthealth-subscription-error-paths#task#4 (ActivateSubscription, 404).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, bootstrap_signal=false, state_reset_failures=0, 15/43 steps passed, score 34.9%).

Cause 1 - connecthealth-get-subscription-not-found#task#4, GetSubscription, expected 404 / actual 404:
  $.Message: missing (expected 'Subscription not found: dom-5c7cbb2c13d48666/sub-000000000000000000000', got None)

Cause 2 - connecthealth-subscription-error-paths#task#3, ListSubscriptions, expected 400 / actual 400:
  $.Message: missing (expected "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100", got None)

Cause 3 - connecthealth-subscription-error-paths#task#4, ActivateSubscription, expected 404 / actual 404:
  $.Message: missing (expected 'Subscription not found: dom-8c080cffb3fb1570/sub-d4d4d4d4d4d4d4d4d4d4d', got None)

In all three the HTTP status matches; the divergence is that the Message member is absent from the body. The recorded domain/subscription ids are examples; the requirement is the member's presence and its request-derived content (the identifiers for Causes 1 and 3, the offending value + member path + violated constraint for Cause 2, which follows AWS's documented validation-message rule).

Model citations: ResourceNotFoundException (errorKind client, httpError 404, member message -> String); ValidationException (errorKind client, httpError 400, member message -> String); ListSubscriptionsInput.maxResults -> Integer @range{min:1,max:100} bound as httpQuery 'maxResults'; GetSubscription / ActivateSubscription / ListSubscriptions all declare both errors.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Cause 1: fixture connecthealth-get-subscription-not-found, step task#4 - GET /domains/{domainId}/subscriptions/{subscriptionId} for a subscription id that does not exist in an existing domain. Response is 404; assert the body has a Message member naming the domain and subscription.
4. Cause 2: fixture connecthealth-subscription-error-paths, step task#3 - GET /domains/{domainId}/subscriptions?maxResults=101 (above the modeled @range max of 100). Response is 400; assert the body has a Message member in AWS's standard validation form naming 'maxResults', the value and the constraint.
5. Cause 3: same fixture, step task#4 - POST /domains/{domainId}/subscriptions/{subscriptionId}/activate for a non-existent subscription id. Response is 404; assert the body has a Message member naming the domain and subscription.
6. All three causes must pass before this issue is closed.
