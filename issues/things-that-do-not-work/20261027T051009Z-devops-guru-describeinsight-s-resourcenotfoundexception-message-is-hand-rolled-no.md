Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: devops-guru.describe-insight-not-found
Issue-Shape-Id: com.amazonaws.devopsguru#CapstoneControlPlaneService
Issue-Key: devops-guru.describe-insight-not-found|run#6|AssertionError|assert error.get("Message") in (f"[DescribeInsight] {not_found}", f"[DescribeFindingById] {not_found}"), (
Issue-Commit: 6faa7ce4aec2ce039d1c72fb49683f578ec3667c

# DescribeInsight's ResourceNotFoundException message is hand-rolled ("no insight with id X exists") instead of AWS's "[DescribeInsight] failed - Resource Not found for awsAccountId [...] for type [findingId] with id [...]" (1 cause)

> _Note: this issue was identified against the code base at commit `6faa7ce4aec2ce039d1c72fb49683f578ec3667c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInsight

## Description
Cause 1 of 1: DescribeInsight on an Id the insight store does not hold refuses with the right error code and status but the WRONG message text.

AWS: Smithy (services/service-com-amazonaws-devopsguru-capstonecontrolplaneservice/smithy-model/com-amazonaws-devopsguru-capstonecontrolplaneservice.json) models DescribeInsight as GET /insights/{Id} with error ResourceNotFoundException (errorKind client, httpError 404) whose members are Message (required), ResourceId (required) and ResourceType (required); Id is InsightId, @length 1..100, @pattern ^[\w-]*$. The model documents ResourceNotFoundException only as "A requested resource could not be found" and does not fix the message string, so the message template is recorded behavior only: real AWS answers 404 ResourceNotFoundException whose Message is an operation prefix followed by a fixed sentence, i.e.
  "[DescribeInsight] failed - Resource Not found for awsAccountId [<accountId>] for type [findingId] with id [<Id>]"
Fixed text: the bracketed operation prefix, then "failed - Resource Not found for awsAccountId [", "] for type [findingId] with id [", and the trailing "]". Variable parts, taken from the request and from the emulator's resolved identity: <accountId> is the account the request resolves to (the request's AccountId when supplied, otherwise the caller's own account — the same value the handler already computes via crate::state::requested_account_id) and <Id> is the InsightId from the URI label. The type token inside the message is the literal "findingId" for this lookup, not the emulator's own insight resource-type constant. The operation prefix is "[DescribeInsight]" in an account with no DevOps Guru state and "[DescribeFindingById]" once the account has a resource collection or service integration; both prefixes are accepted, the rest of the sentence is fixed. No value from any single run (a particular account number or insight id) is part of the template — they are substituted per request.

Emulator: returns ResourceNotFoundException with HTTP 404 and Code ResourceNotFoundException (both correct), but Message is the hand-rolled string "no insight with id <Id> exists" — e.g. 'no insight with id ADtFW0Yw0dPOaKNSlNmiqSkAAAAAAAAAAPlAoSfVxXzLkZvXYw4FbGZUMxj1T8yv exists'. It carries neither the operation prefix, nor the "failed - Resource Not found for awsAccountId [...]" fixed text, nor the account id, nor the "for type [findingId]" segment, and it does not bracket the id. The positive control in the same check (ListInsights with StatusFilter Ongoing/REACTIVE) succeeds, so the surface is alive and the divergence is specific to this refusal's message.

Why: clients and tooling that parse or assert on DevOps Guru's not-found message — log scrapers, test fixtures asserting the exact AWS sentence, and support/diagnostic paths that extract the awsAccountId and the resource type from the message — get no account id and no resource-type token, and any exact-match assertion on AWS's wording fails. Code-only consumers are unaffected, which is why this is MEDIUM rather than HIGH, but the refusal is not wire-faithful.

Fix: the DescribeInsight handler's not-found branch — the ResourceNotFoundException construction in the ops handler for this operation (the `message:` field of the error built when the insight store has no record for the requested Id). It must compose the message from the fixed text plus the resolved account id and the requested Id, and use the "findingId" type token in the message (ResourceId/ResourceType on the error structure itself are separate modeled members and are already populated). If other DevOps Guru reads share this message style, prefer a single helper so the account id and the operation prefix are rendered consistently.

Traces: devops-guru.describe-insight-not-found (assertion at check_devops_guru_describe_insight_not_found.py line 35). Handler evidence: services/service-com-amazonaws-devopsguru-capstonecontrolplaneservice/src/ops/describe_insight.rs lines 186-195, where the message is formatted as "no insight with id {} exists".

## Evidence
Check: devops-guru.describe-insight-not-found (mismatch).

Request: DescribeInsight with a well-formed but never-generated Id (matches ^[\w-]*$, length within 1..100).

Expected (real AWS): HTTP 404, Error.Code "ResourceNotFoundException", Error.Message one of
  "[DescribeInsight] failed - Resource Not found for awsAccountId [<accountId>] for type [findingId] with id [<Id>]"
  "[DescribeFindingById] failed - Resource Not found for awsAccountId [<accountId>] for type [findingId] with id [<Id>]"
where <accountId> is the account the request resolves to and <Id> is the requested InsightId.

Actual (emulator): HTTP 404, Error.Code "ResourceNotFoundException" (correct), Error.Message = "no insight with id <Id> exists" — reported by the suite as 'no insight with id ADtFW0Yw0dPOaKNSlNmiqSkAAAAAAAAAAPlAoSfVxXzLkZvXYw4FbGZUMxj1T8yv exists'.

AssertionError from the replay: "expected the exact ResourceNotFoundException message AWS returns, got 'no insight with id ... exists'".

Positive control in the same check: ListInsights(StatusFilter={'Ongoing': {'Type': 'REACTIVE'}}) returns a body containing ReactiveInsights — the API surface is live, so this is a message-fidelity defect in the refusal path only.

Source: services/service-com-amazonaws-devopsguru-capstonecontrolplaneservice/src/ops/describe_insight.rs:189 — `message: format!("no insight with id {} exists", input.id)`.

## Steps to Reproduce
1. Boot the emulator and point a DevOps Guru client at it.
2. Call DescribeInsight with an Id that was never minted by the emulator's analysis and that satisfies the model's constraints, e.g. Id="ADtFW0Yw0dPOaKNSlNmiqSkAAAAAAAAAAPlAoSfVxXzLkZvXYw4FbGZUMxj1T8yv" (any value matching ^[\w-]*$ with length 1..100 reproduces it).
3. Observe HTTP 404 with Error.Code ResourceNotFoundException and Error.Message "no insight with id <Id> exists".
4. Compare against AWS, which returns Message "[DescribeInsight] failed - Resource Not found for awsAccountId [<accountId>] for type [findingId] with id [<Id>]" (prefix may instead be "[DescribeFindingById]" once the account has a resource collection or service integration).
5. Sanity-check the surface is otherwise live: ListInsights(StatusFilter={"Ongoing": {"Type": "REACTIVE"}}) returns ReactiveInsights.
6. Replay devops-guru.describe-insight-not-found to confirm the fix.
