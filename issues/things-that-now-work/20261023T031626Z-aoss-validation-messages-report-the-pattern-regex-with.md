Issue-Source: trace-sweep-divergence
Issue-Trace: opensearchserverless-access-policy-errors
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: opensearchserverless-access-policy-errors opensearchserverless-collection-error-paths opensearchserverless-collection-group-error-paths opensearchserverless-lifecycle-policy-errors
Issue-Shape-Id: com.amazonaws.opensearchserverless#OpenSearchServerless
Issue-Key: aoss|UpdateAccessPolicy|4xx->4xx|$.message
Issue-Key: aoss|UpdateLifecyclePolicy|4xx->4xx|$.message
Issue-Related: 20261023T031626Z-aoss-client-errors-answer-the-modeled-httperror-status.md
Issue-Commit: 7f205ee96657d4151f96c4aa7296372f34590a63

# aoss validation messages report the @pattern regex with its ^/$ anchors

> _Note: this issue was identified against the code base at commit `7f205ee96657d4151f96c4aa7296372f34590a63`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAccessPolicy

## Description
This issue bundles FOUR distinct causes in the opensearchserverless (`aoss`) emulator, all the same fault: the constraint-violation message renders the modeled @pattern verbatim, including the ^ and $ anchors, where AWS renders the pattern unanchored. Each cause must be closed before submitting.

Shared background. AWS: the ValidationException message is built from the request, so its rule is known and is a requirement, not an example — "1 validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" — and <pattern> is the constraint WITHOUT the surrounding anchors. Status (HTTP 400) and __type (com.amazon.aoss#ValidationException) are correct in all four causes; only the rendered pattern diverges. Smithy declares PolicyVersion pattern ^([0-9a-zA-Z+/]{4})*(([0-9a-zA-Z+/]{2}==)|([0-9a-zA-Z+/]{3}=))?$ (length 20..36), PolicyName/CollectionGroupName pattern ^[a-z][a-z0-9-]+$ and CollectionName pattern ^[a-z][a-z0-9-]+$.

NOTE: the open behavioral issue "opensearchserverless: 10 divergences — encryption requirement unenforced on CreateCollection..." (file 20261022T233713Z-opensearchserverless-10-divergences-...) describes this same formatter as its Cause 5, but it carries no wire replay keys for these fixture steps, so these four wire causes are filed here with the fixture steps a fixer must re-replay.

Cause 1 of 4: UpdateAccessPolicy with a malformed policyVersion.
AWS: HTTP 400 ValidationException whose message reports the pattern as ([0-9a-zA-Z+/]{4})*(([0-9a-zA-Z+/]{2}==)|([0-9a-zA-Z+/]{3}=))? at member 'policyVersion'.
Emulator: reports ^([0-9a-zA-Z+/]{4})*(([0-9a-zA-Z+/]{2}==)|([0-9a-zA-Z+/]{3}=))?$.
Why: a client that parses the reported constraint out of the message (or asserts on it in a conformance test) receives a regex AWS never emits; the anchors change the literal text of a message whose construction rule is documented behavior.
Fix: the shared constraint-violation message formatter in the validation layer — strip the leading ^ and trailing $ when rendering a @pattern trait into the message — not the individual operation handlers.
Traces: opensearchserverless-access-policy-errors#task#3.

Cause 2 of 4: CreateCollection with a malformed name.
AWS: message reports the pattern as [a-z][a-z0-9_-]+ at member 'name' — note AWS's own validator for CollectionName reports a character class that includes the underscore, which the public model's ^[a-z][a-z0-9-]+$ does not (recorded behavior only for the underscore).
Emulator: reports ^[a-z][a-z0-9-]+$ — anchored AND a different character class.
Why: as Cause 1, with the extra effect that the reported rule for collection names does not match the rule the service actually applies.
Fix: the same formatter, plus the per-member pattern text used for CollectionName validation.
Traces: opensearchserverless-collection-error-paths#task#4.

Cause 3 of 4: CreateCollectionGroup with a malformed name.
AWS: message reports the pattern as [a-z][a-z0-9-]+ at member 'name'.
Emulator: reports ^[a-z][a-z0-9-]+$.
Fix: the same formatter.
Traces: opensearchserverless-collection-group-error-paths#task#3.

Cause 4 of 4: UpdateLifecyclePolicy with a malformed policyVersion.
AWS: message reports the pattern as ([0-9a-zA-Z+/]{4})*(([0-9a-zA-Z+/]{2}==)|([0-9a-zA-Z+/]{3}=))? at member 'policyVersion'.
Emulator: reports the anchored form.
Fix: the same formatter.
Traces: opensearchserverless-lifecycle-policy-errors#task#5.

## Evidence
Cause 1 — opensearchserverless-access-policy-errors#task#3 UpdateAccessPolicy (status 400 matched): $.message expected "1 validation error detected: Value 'AAAAAAAAAAAAAAAAAAAA==' at 'policyVersion' failed to satisfy constraint: Member must satisfy regular expression pattern: ([0-9a-zA-Z+/]{4})*(([0-9a-zA-Z+/]{2}==)|([0-9a-zA-Z+/]{3}=))?", got the same text with the pattern wrapped as ^...$.
Cause 2 — opensearchserverless-collection-error-paths#task#4 CreateCollection: expected "...Value '1invalidname' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-z][a-z0-9_-]+", got "...pattern: ^[a-z][a-z0-9-]+$".
Cause 3 — opensearchserverless-collection-group-error-paths#task#3 CreateCollectionGroup: expected "...Value '1cd' at 'name' ... pattern: [a-z][a-z0-9-]+", got "...pattern: ^[a-z][a-z0-9-]+$".
Cause 4 — opensearchserverless-lifecycle-policy-errors#task#5 UpdateLifecyclePolicy: expected "...Value 'AAAAAAAAAAAAAAAAAAAA==' at 'policyVersion' ... pattern: ([0-9a-zA-Z+/]{4})*(([0-9a-zA-Z+/]{2}==)|([0-9a-zA-Z+/]{3}=))?", got the anchored form.
In all four the HTTP status (400) and the error __type (com.amazon.aoss#ValidationException) matched.

## Steps to Reproduce
Boot the emulator and replay the aoss fixtures.
1 (Cause 1): UpdateAccessPolicy(type='data', name=<existing>, policyVersion='AAAAAAAAAAAAAAAAAAAA==') -> expect the reported pattern unanchored; observe ^...$. Fixture opensearchserverless-access-policy-errors#task#3.
2 (Cause 2): CreateCollection(name='1invalidname') -> expect "...pattern: [a-z][a-z0-9_-]+"; observe "...pattern: ^[a-z][a-z0-9-]+$". Fixture opensearchserverless-collection-error-paths#task#4.
3 (Cause 3): CreateCollectionGroup(name='1cd', standbyReplicas=...) -> expect "...pattern: [a-z][a-z0-9-]+"; observe the anchored form. Fixture opensearchserverless-collection-group-error-paths#task#3.
4 (Cause 4): UpdateLifecyclePolicy(type='retention', name=<existing>, policyVersion='AAAAAAAAAAAAAAAAAAAA==') -> expect the unanchored pattern; observe the anchored form. Fixture opensearchserverless-lifecycle-policy-errors#task#5.
