Issue-Source: trace-sweep-divergence
Issue-Trace: xray-encryption-config-update
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: xray-encryption-config-update xray-get-insight-summaries-window xray-group-lifecycle-with-tags xray-sampling-statistics-and-targets
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray|PutEncryptionConfig|4xx->4xx|$.Message
Issue-Key: xray|GetInsightSummaries|4xx->4xx|$.message
Issue-Key: xray|TagResource|4xx->4xx|$.Message
Issue-Key: xray|GetSamplingTargets|2xx->2xx|$.UnprocessedStatistics[*].Message
Issue-Commit: 54073c01592ce4e56a3cca58cb5387afed010108

# xray wire replay: error and status message text diverges from AWS on four operations (4 causes)

> _Note: this issue was identified against the code base at commit `54073c01592ce4e56a3cca58cb5387afed010108`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEncryptionConfig

## Description
Four net-new root causes found by wire replay against the X-Ray emulator. In every one the HTTP status and the error code/shape are already right and ONLY the human-readable message text diverges from AWS's wording. A client that branches on, logs or asserts on the refusal text sees an unknown message. The message is part of the contract for each error class, so each of these is a defect.

Cause 1 of 4: PutEncryptionConfig(Type=KMS) with no KeyId is refused with the wrong message.
AWS: Smithy PutEncryptionConfig (POST /PutEncryptionConfig) models Type as a required EncryptionType enum and KeyId as optional EncryptionKeyId @length 1..3000; the only modeled client error is InvalidRequestException (httpError 400). The conditional "KeyId required when Type=KMS" rule is server-side, and the recorded response is 400 with message `Key ID is required for type KMS` — entirely FIXED text, no part of it is taken from the request or from state.
Emulator: answers 400 (correct status, correct error class) with `KeyId is required when Type is KMS: it selects the KMS key to encrypt with`.
Why: a client (or an operator-facing console/CLI wrapper) that recognises the missing-key refusal by AWS's text to re-prompt for a key never matches, so a well-understood misconfiguration is reported as an unclassified bad request.
Fix: the PutEncryptionConfig handler's conditional-required validation in the X-Ray service crate (services/service-com-amazonaws-xray-awsxray).
Traces: xray-encryption-config-update#task#2. Note: open issue 20261022T045628Z-xray-x-ray-account-config-surfaces-are-overly-permissive.md covers PutEncryptionConfig ACCEPTING a nonexistent KMS key and enum/length misclassification, and 20261029T190848Z-xray-refusal-messages-diverge-from-aws-on-four.md records the same wording defect from the BEHAVIORAL corpus — neither carries this fixture/step replay key, so this wire step is unowned and is filed here.

Cause 2 of 4: the ValidationException message drops the offending value, so the `Value '<rendered value>'` clause is rendered as a bare `Value`.
AWS: Smithy GetInsightSummaries (POST /InsightSummaries) targets States at InsightStateList with @length min 0 / max 1. A violation is refused 400 with the AWS validation template `1 validation error detected: Value '<rendered value>' at '<memberPath>' failed to satisfy constraint: Member must have length less than or equal to <max>`. Fixed text: `1 validation error detected: Value '`, `' at '`, `' failed to satisfy constraint: Member must have length less than or equal to `. Variable parts, all derived from the request and the model: the rendered value (a list rendered `[A, B]` in request order), the lowerCamel member path (`states`), and the maximum from the @length trait (1).
Emulator: emits the template with that whole clause collapsed — `1 validation error detected: Value at 'states' failed to satisfy constraint: Member must have length less than or equal to 1`. Status, code, member path and bound are all correct; only the rejected value is never rendered.
Why: the rendered value is the only part of the message identifying WHICH input was oversized when a request carries several constrained collections, so a caller reading the refusal cannot tell what to shrink, and any matcher on AWS's text fails.
Fix: the shared validation-error message formatter used by the X-Ray crate's input validation — render the offending value into the template for list and map length violations (this is the formatter, not one operation, so it affects every constrained member).
Traces: xray-get-insight-summaries-window#task#4.

Cause 3 of 4: TagResource on an ARN that names no resource reports the right status and error type with the wrong message.
AWS: Smithy TagResource (POST /TagResource) declares ResourceNotFoundException (httpError 404, members Message and ResourceName). AWS's message template for an unknown resource is `Resource <resourceArn> not found` — fixed text `Resource ` and ` not found`, with the single variable part being the ResourceARN echoed verbatim from the request (in the replay this was a deleted group ARN; the specific ARN, account id, region and group-id suffix are per-run values and are not part of the requirement).
Emulator: 404 with `no X-Ray resource is named by ARN `+backtick-quoted ARN — an entirely different sentence, and it decorates the ARN with backticks that AWS does not emit.
Why: a client that distinguishes "the tag target is gone" from other 404s by message text, or that parses the ARN out of the message, fails on both counts.
Fix: the TagResource handler's resource-lookup miss path in the X-Ray service crate (and the shared ResourceNotFoundException message builder it uses, so UntagResource/ListTagsForResource stay consistent).
Traces: xray-group-lifecycle-with-tags#task#13.

Cause 4 of 4: the UnprocessedStatistics entry GetSamplingTargets returns for an unknown sampling rule carries the wrong Message.
AWS: Smithy GetSamplingTargets (POST /SamplingTargets) returns 200 with UnprocessedStatistics (RuleName, ErrorCode, Message). For a statistics document naming a rule the service does not know, the recorded response is 200 with ErrorCode `400` and Message `Unknown rule` — entirely fixed text; only RuleName is a value echoed from the request's statistics document. Recorded behavior only: the model does not specify the text.
Emulator: serves the entry with the right RuleName and ErrorCode but Message `Sampling rule does not exist`.
Why: an X-Ray SDK sampling poller inspects UnprocessedStatistics to decide whether to drop a stale cached rule or retry; keyed on AWS's wording it recognises neither, so it either retries the rejected document forever or surfaces an unclassified error.
Fix: the GetSamplingTargets handler's unknown-rule path in the X-Ray service crate — emit AWS's fixed `Unknown rule` text for the unknown-rule UnprocessedStatistics entry.
Traces: xray-sampling-statistics-and-targets#task#3. Note: open issue 20261029T223652Z-xray-getsamplingtargets-refuses-an-unseen-clientid-reporting-for.md argues a DIFFERENT case (a DELETED rule, where AWS emits no UnprocessedStatistics entry at all); it does not cover the wording of the entry for a rule that was never created, and carries no wire replay key, so this step is unowned.

## Evidence
All four from replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health 200).

Cause 1 — xray-encryption-config-update#task#2, PutEncryptionConfig, expected 400 / actual 400:
  $.Message: expected 'Key ID is required for type KMS'
                actual 'KeyId is required when Type is KMS: it selects the KMS key to encrypt with'

Cause 2 — xray-get-insight-summaries-window#task#4, GetInsightSummaries, expected 400 / actual 400:
  $.message: expected "1 validation error detected: Value '[ACTIVE, CLOSED]' at 'states' failed to satisfy constraint: Member must have length less than or equal to 1"
                actual "1 validation error detected: Value at 'states' failed to satisfy constraint: Member must have length less than or equal to 1"
  (the `[ACTIVE, CLOSED]` rendering is the request's States list; the fixed text is the surrounding template, the bound 1 comes from InsightStateList @length max 1)

Cause 3 — xray-group-lifecycle-with-tags#task#13, TagResource, expected 404 / actual 404:
  $.Message: expected 'Resource arn:aws:xray:us-west-2:000000000000:group/<group-name>/<group-id> not found'
                actual 'no X-Ray resource is named by ARN `arn:aws:xray:us-west-2:000000000000:group/<group-name>/<group-id>`'
  (the recorded run used group sim2real-probe-xray-grp-c0945404 / id cc61385143d1; the generated name, id, account and region are per-run values — the requirement is the 'Resource <arn> not found' template with the request's ResourceARN)

Cause 4 — xray-sampling-statistics-and-targets#task#3, GetSamplingTargets, expected 200 / actual 200:
  $.UnprocessedStatistics[0].Message: expected 'Unknown rule'
                                         actual 'Sampling rule does not exist'
  (RuleName and ErrorCode '400' matched; only the fixed message text diverges)

## Steps to Reproduce
Cause 1:
1. Replay fixture xray-encryption-config-update, step task#2: PutEncryptionConfig(Type='KMS') with no KeyId -> 400 InvalidRequestException whose Message must be exactly 'Key ID is required for type KMS'.

Cause 2:
2. Replay fixture xray-get-insight-summaries-window, step task#4: GetInsightSummaries(GroupName=<g>, StartTime, EndTime, States=['ACTIVE','CLOSED']) -> 400 whose message must render the offending value, i.e. "1 validation error detected: Value '[ACTIVE, CLOSED]' at 'states' failed to satisfy constraint: Member must have length less than or equal to 1".

Cause 3:
3. Replay fixture xray-group-lifecycle-with-tags through step task#13: create a group, delete it, then TagResource(ResourceARN=<the deleted group ARN>, Tags=[...]) -> 404 ResourceNotFoundException whose Message must be exactly 'Resource <that ARN> not found'.

Cause 4:
4. Replay fixture xray-sampling-statistics-and-targets, step task#3: GetSamplingTargets with a SamplingStatisticsDocument naming a rule that does not exist -> 200 with UnprocessedStatistics[0] = {RuleName: <the requested rule>, ErrorCode: '400', Message: 'Unknown rule'}.
