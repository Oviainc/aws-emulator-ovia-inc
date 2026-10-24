Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr.lifecycle-policy-invalid-json-rejected ecr.op-contract-put-lifecycle-policy ecr.put-registry-policy-invalid-json-rejected ecr.put-registry-policy-malformed-json-rejected ecr.put-registry-policy-overly-broad-principal-rejected ecr.registry-policy-invalid-document-rejected ecr.ecracc-lifecycle-policy-with-a-zero-image-count-rejected ecr.ecracc-tagged-lifecycle-rule-without-a-tag-filter-rejected
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr.lifecycle-policy-invalid-json-rejected|run#10|AssertionError|assert message == (
Issue-Key: ecr.op-contract-put-lifecycle-policy|run#20|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: ecr.put-registry-policy-invalid-json-rejected|run#11|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: ecr.put-registry-policy-malformed-json-rejected|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: ecr.put-registry-policy-overly-broad-principal-rejected|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: ecr.registry-policy-invalid-document-rejected|run#11|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: ecr.ecracc-lifecycle-policy-with-a-zero-image-count-rejected|run#8|AssertionError|assert err == want, f"countNumber 0 must be refused; got {err!r}, expected {want!r}"
Issue-Key: ecr.ecracc-tagged-lifecycle-rule-without-a-tag-filter-rejected|run#8|AssertionError|assert err == want, (
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# ECR policy/lifecycle-document validation errors carry emulator-internal messages instead of AWS's parameter-and-constraint message

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRegistryPolicy

## Description
Cause 1 of 1: document-validation rejections return the right status and error code but a message built by the emulator's own JSON parser rather than by AWS's rule.
AWS: these rejections are InvalidParameterException (client error, HTTP 400 — as modeled on ecr#PutRegistryPolicy / ecr#PutLifecyclePolicy) and their message is built from the request to a fixed template that names the API parameter and the constraint it failed: "Invalid parameter at '<Parameter>' failed to satisfy constraint: '<constraint>'". The recorded parameter labels are the API-documented ones, 'PolicyText' and 'LifecyclePolicyText' (capitalised), and the constraint text is a stable phrase such as 'Invalid registry policy provided', 'Lifecycle policy parsing failure: Could not map policyString into LifecyclePolicy.' or a 'Lifecycle policy validation failure: ...' sentence — never a parser position.
Emulator: the status (400) and code (InvalidParameterException) match, but the message is a different rule: it names the wire member in lowerCamel ('policyText', 'lifecyclePolicyText') and embeds internal serde-json diagnostics or bespoke prose, e.g. "Invalid parameter at 'policyText' failed to satisfy constraint: the policy is not valid JSON: key must be a string at line 1 column 2", "...: the policy is not valid JSON: expected ident at line 1 column 43", "...: a policy statement's Effect is 'AllowAndDeny', which is not one of Allow, Deny", "lifecyclePolicyText is not a valid lifecycle policy: rule 1 carries no 'selection.countNumber' integer of 1 or more". Two of them drop AWS's template entirely.
Why: the message is the only machine-readable detail these errors carry (the code is the same for every document problem), so tooling and operators that parse it to learn WHICH parameter failed get a member name AWS never emits, plus line/column offsets into the emulator's parse of the document. It also leaks implementation internals into an API response.
Fix: the document-validation path for PutRegistryPolicy / PutLifecyclePolicy / repository-creation-template lifecycle validation must format its message from the AWS parameter label and a stable constraint phrase, and must not surface serde parse positions.
Traces: ecr.lifecycle-policy-invalid-json-rejected, ecr.op-contract-put-lifecycle-policy, ecr.put-registry-policy-invalid-json-rejected, ecr.put-registry-policy-malformed-json-rejected, ecr.put-registry-policy-overly-broad-principal-rejected, ecr.registry-policy-invalid-document-rejected, ecr.ecracc-lifecycle-policy-with-a-zero-image-count-rejected, ecr.ecracc-tagged-lifecycle-rule-without-a-tag-filter-rejected.

Note for the fixer: purely cosmetic wording differences in ECR's not-found / already-exists messages (pull-through-cache rules, repository creation templates, pull-time update exclusions) were NOT filed, because the rule producing those strings is not stated by the model or the docs. Only the parameter-label/constraint template above is treated as a requirement here.

## Evidence
Expected (recorded from AWS) vs actual (emulator):
- PutLifecyclePolicy with malformed JSON: expected "Invalid parameter at 'LifecyclePolicyText' failed to satisfy constraint: 'Lifecycle policy parsing failure: Could not map policyString into LifecyclePolicy.'", got "lifecyclePolicyText is not a valid lifecycle policy: key must be a string at line 1 column 2".
- PutRegistryPolicy with malformed JSON: expected "Invalid parameter at 'PolicyText' failed to satisfy constraint: 'Invalid registry policy provided'", got "Invalid parameter at 'policyText' failed to satisfy constraint: the policy is not valid JSON: expected ident at line 1 column 43" (and "...key must be a string at line 1 column 2").
- PutRegistryPolicy with Effect 'AllowAndDeny': expected "Invalid parameter at 'PolicyText' failed to satisfy constraint: 'Invalid registry policy provided'", got "Invalid parameter at 'policyText' failed to satisfy constraint: a policy statement's Effect is 'AllowAndDeny', which is not one of Allow, Deny".
- Lifecycle rule with countNumber 0: expected "Invalid parameter at 'LifecyclePolicyText' failed to satisfy constraint: 'Lifecycle policy validation failure: numeric instance is lower than the required minimum (minimum: 1, found: 0)'", got "lifecyclePolicyText is not a valid lifecycle policy: rule 1 carries no 'selection.countNumber' integer of 1 or more".
- Tagged lifecycle rule with no tag filter: expected "...'Lifecycle policy validation failure: Must specify tagPrefixList or tagPatternList when tagStatus=TAGGED.'", got "lifecyclePolicyText is not a valid lifecycle policy: rule 1 selects tagged images but carries neither 'selection.tagPrefixList' nor 'selection.tagPatternList'".
Error code (InvalidParameterException) and status (400) matched in every case.

## Steps to Reproduce
1. CreateRepository R. 2. Call PutLifecyclePolicy(repositoryName=R, lifecyclePolicyText='{1:2}') and inspect the Message. 3. Call PutRegistryPolicy(policyText='{"Version": "2012-10-17", "Statement": [bad}') and inspect the Message. 4. Call PutLifecyclePolicy with a rule whose selection.countNumber is 0, and with tagStatus=TAGGED and no tagPrefixList/tagPatternList. Compare each Message against the AWS template "Invalid parameter at '<Parameter>' failed to satisfy constraint: '<constraint>'".
