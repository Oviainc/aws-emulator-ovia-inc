Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.parameter-version-selector-and-label-move ssm.deep-secrets-and-params-label-moves-back-to-an-older-version-and-does-not-follow-overwrites ssm.get-parameters-batch-orders-alphabetically-and-buckets-unresolvable-selectors ssm.get-parameters-by-path-tag-filter-silently-ignored ssm.describe-parameters-tag-filter-requires-exact-value-despite-beginswith-option ssm.get-parameters-by-path-filters-by-type ssm.get-parameters-by-path-pagination-token-exactness ssm.expiration-notification-requires-expiration-policy ssm.parameter-policies-survive-a-value-update-and-only-an-empty-array-clears-them ssm.secure-string-customer-managed-kms-key-overwrite-rekeys-history ssm.parameter-hierarchy-depth-and-prefix-conflict-rejected ssm.role-scoped-to-parameter-path-prefix-denies-outside-subtree
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm.parameter-version-selector-and-label-move|run#17|AssertionError|assert pinned["Selector"] == ":2", (
Issue-Key: ssm.deep-secrets-and-params-label-moves-back-to-an-older-version-and-does-not-follow-overwrites|run#14|AssertionError|assert at1["Selector"] == f":{_LABEL}", f"Selector must echo the label: {at1.get('Selector')!r}"
Issue-Key: ssm.get-parameters-batch-orders-alphabetically-and-buckets-unresolvable-selectors|run#23|AssertionError|assert sorted(by_selector) == sorted([":1", f":{_LABEL}"]), (
Issue-Key: ssm|GetParametersByPath|InvalidFilterKey
Issue-Key: ssm.describe-parameters-tag-filter-requires-exact-value-despite-beginswith-option|run#15|AssertionError|assert prefix_via_beginswith == [], (
Issue-Key: ssm.get-parameters-by-path-filters-by-type|_error#20|AssertionError|raise AssertionError(f"expected the call to be refused but it succeeded: {result!r}")
Issue-Key: ssm.get-parameters-by-path-pagination-token-exactness|_error#19|AssertionError|raise AssertionError(f"the call was expected to be refused but it succeeded: {result!r}")
Issue-Key: ssm.expiration-notification-requires-expiration-policy|_error_code#8|AssertionError|raise AssertionError(
Issue-Key: ssm.parameter-policies-survive-a-value-update-and-only-an-empty-array-clears-them|run#20|AssertionError|assert _policy_types(after_update) == ["Expiration", "NoChangeNotification"], (
Issue-Key: ssm.secure-string-customer-managed-kms-key-overwrite-rekeys-history|run#28|AssertionError|assert _current_key_id() == key2, (
Issue-Key: ssm|PutParameter|HierarchyLevelLimitExceededException
Issue-Key: ssm.role-scoped-to-parameter-path-prefix-denies-outside-subtree|_client_error#27|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: 69e3918bc598d03b2c669434388419738bf97909

# SSM Parameter Store semantics diverge: selector echo, filter matching, pagination tokens, policy and KMS retention, hierarchy depth, path-scoped authorization (10 causes)

> _Note: this issue was identified against the code base at commit `69e3918bc598d03b2c669434388419738bf97909`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetParameter

## Description
Ten distinct Parameter Store behaviors are wrong in substance, not only in wording: reads drop the selector the caller asked with, filters match on rules AWS does not use, a bogus pagination token is accepted, an Overwrite silently destroys policies and re-keys a SecureString, the hierarchy-depth limit is off by one, and an identity policy scoped to a path prefix is not enforced. 10 CAUSES.

Cause 1 of 10 — reads do not echo the selector the caller asked with.
AWS: GetParameter on '<name>:<version>' or '<name>:<label>' answers with Parameter.Selector set to ':<version>'/':<label>' (and Parameter.Name as requested); GetParameters echoes each requested selector back on its result entry, so a batch caller can join results to requests.
Emulator: Selector is the bare value ('2', 'sim2real-release') with no leading ':', and the batch results come back as '1'/'sim2real-batch-pin' instead of ':1'/':sim2real-batch-pin'.
Why: a client that keys its results by the selector string it sent cannot match them, and the ':' form is what AWS documents as the selector syntax.
Fix: the GetParameter / GetParameters response serializer (populate Selector verbatim from the requested name suffix).
Traces: ssm.parameter-version-selector-and-label-move, ssm.deep-secrets-and-params-label-moves-back-to-an-older-version-and-does-not-follow-overwrites, ssm.get-parameters-batch-orders-alphabetically-and-buckets-unresolvable-selectors.

Cause 2 of 10 — GetParametersByPath refuses a tag:<key> ParameterFilters entry that AWS accepts and ignores.
AWS: a filter {Key:'tag:<k>', Option:'Equals', Values:[...]} on GetParametersByPath is accepted and is a silent no-op — a matching and a non-matching value both return the full subtree (recorded behavior only; Smithy models InvalidFilterKey for this operation but the service does not raise it for tag: keys). The same filter DOES narrow DescribeParameters.
Emulator: 400 InvalidFilterKey (with no Message — see the companion messages issue).
Why: a caller that passes a tag filter to the path read, as AWS permits, is refused outright.
Fix: GetParametersByPath ParameterFilters validation — accept tag:<key> and ignore it.
Traces: ssm.get-parameters-by-path-tag-filter-silently-ignored.

Cause 3 of 10 — DescribeParameters treats Option=BeginsWith on a tag:<key> filter as a prefix match.
AWS: on a tag: key the value must match the tag value EXACTLY even when Option=BeginsWith; a genuine strict prefix of the tag value matches nothing.
Emulator: prefix-matches, so a filter value that is a strict prefix of the stored tag value returns the tagged parameter.
Why: over-permissive matching returns parameters the caller's filter excluded — a tag-scoped listing silently widens.
Fix: the DescribeParameters tag-filter evaluation (ignore BeginsWith for tag: keys).
Traces: ssm.describe-parameters-tag-filter-requires-exact-value-despite-beginswith-option.

Cause 4 of 10 — GetParametersByPath treats Option=BeginsWith on the Type filter as a prefix match instead of validating the enum.
AWS: Type is a closed enum, so {Key:'Type', Option:'BeginsWith', Values:['Str']} is refused 400 InvalidFilterValue (Smithy: GetParametersByPath error InvalidFilterValue) with 'The following filter value is not valid: <value>. Valid values include: [String, StringList, SecureString].'
Emulator: 200, prefix-matching 'Str' against String and StringList and returning both.
Why: an invalid filter value reads as a successful narrowing, so the caller trusts a result set AWS would never return.
Fix: GetParametersByPath Type-filter value validation.
Traces: ssm.get-parameters-by-path-filters-by-type.

Cause 5 of 10 — GetParametersByPath accepts a malformed NextToken and mints tokens that are plain parameter names.
AWS: an unrecognised NextToken is refused 400 InvalidNextToken (Smithy: GetParametersByPath error InvalidNextToken) with 'The following token is invalid: <token>.'
Emulator: 200 with a normal first page — the token is ignored, not validated; and the NextToken it mints is the next parameter's name (e.g. '/<base>/leaf02'), i.e. not an opaque token.
Why: a paginating client that corrupts or replays a stale token silently restarts the scan and can loop or double-process instead of failing fast.
Fix: GetParametersByPath pagination-token encode/decode and its InvalidNextToken path.
Traces: ssm.get-parameters-by-path-pagination-token-exactness.

Cause 6 of 10 — PutParameter accepts an ExpirationNotification policy with no Expiration policy.
AWS: 400 IncompatiblePolicyException (Smithy: PutParameter error IncompatiblePolicyException) and the parameter is NOT created — a follow-up GetParameter answers ParameterNotFound.
Emulator: 200 with Version 1 / Tier 'Advanced'; the parameter is created.
Why: an invalid policy set is persisted, so a notification that can never fire looks configured, and the refusal a caller codes against never arrives.
Fix: the PutParameter policy-compatibility validation.
Traces: ssm.expiration-notification-requires-expiration-policy.

Cause 7 of 10 — an Overwrite that sends no Policies wipes the parameter's existing policies.
AWS: existing policies are preserved until new policies or an empty policy array is sent; after a value-only Overwrite, DescribeParameters still reports the prior PolicyTypes (e.g. Expiration + NoChangeNotification). Only Policies='[]' clears them.
Emulator: after the value-only Overwrite the parameter carries no policies at all.
Why: a routine value update silently drops expiration and notification policies — data-retention behavior the caller configured disappears without any API call asking for it.
Fix: the PutParameter overwrite path's policy merge (omitted Policies must not be treated as an empty set).
Traces: ssm.parameter-policies-survive-a-value-update-and-only-an-empty-array-clears-them.

Cause 8 of 10 — a SecureString Overwrite that omits KeyId reverts to the default key.
AWS: omitting KeyId on an Overwrite keeps reusing the key the parameter already carries (a customer-managed CMK stays the CMK); DescribeParameters keeps reporting that key id.
Emulator: DescribeParameters reports 'alias/aws/ssm' after the Overwrite — the parameter is re-keyed to the AWS-managed default.
Why: a value written under a customer-managed CMK is silently re-encrypted under the service default, which breaks key-policy based access control and any audit that asserts the CMK.
Fix: the PutParameter SecureString path's KeyId retention.
Traces: ssm.secure-string-customer-managed-kms-key-overwrite-rekeys-history.

Cause 9 of 10 — the hierarchy-depth limit is off by one, and its refusal message is authored.
AWS: a name may carry 16 slash-separated segments (15 levels plus the leaf), so a 16-segment name is ACCEPTED and returns Version 1; only 17 segments is refused, 400 HierarchyLevelLimitExceededException (Smithy: PutParameter error HierarchyLevelLimitExceededException) with the fixed sentence 'Depth of the tree cannot exceed 15'.
Emulator: refuses the legal 16-segment name with HierarchyLevelLimitExceededException and the authored message 'Name '<name>' is 16 levels deep: a parameter hierarchy is limited to a maximum depth of 15 levels' — it counts the leaf as a level and prints prose AWS never writes.
Fix: the PutParameter name-depth validation and its message.
Traces: ssm.parameter-hierarchy-depth-and-prefix-conflict-rejected.

Cause 10 of 10 — an identity policy scoped to a parameter path prefix is not enforced on GetParameter.
AWS: a role whose policy allows ssm:GetParameter only on arn:aws:ssm:<region>:<account>:parameter/<prefix>/* is denied 400 AccessDeniedException when it reads a parameter outside that subtree.
Emulator: 200 with the full Parameter payload for the out-of-subtree name.
Why: resource-scoped least-privilege is unobservable — a caller cannot test that its own scoping works, and an emulated environment lets reads through that AWS refuses.
Fix: the SSM request authorization path — evaluate the caller's identity policy resource ARNs for parameter reads.
Traces: ssm.role-scoped-to-parameter-path-prefix-denies-outside-subtree.

## Evidence
Replay of simparity-behavioral-aws-v1; all 12 checks are listed under '### ssm' in the suite report.

Cause 1 — ssm.parameter-version-selector-and-label-move: "GetParameter must echo Selector ':2', got '2'"; ssm.deep-secrets-...-label-moves-back-...: "Selector must echo the label: 'sim2real-release'"; ssm.get-parameters-batch-...: "each batch result must echo the selector it was asked with; expected [':1', ':sim2real-batch-pin'], got ['1', 'sim2real-batch-pin']".
Cause 2 — ssm.get-parameters-by-path-tag-filter-silently-ignored: uncaught botocore.errorfactory.InvalidFilterKey on GetParametersByPath with ParameterFilters=[{Key:'tag:env',Option:'Equals',Values:['sim2real-tagsilent']}]; the check asserts both the matching and the non-matching tag value return the whole subtree.
Cause 3 — ssm.describe-parameters-tag-filter-requires-exact-value-despite-beginswith-option: "tag:env BeginsWith['sim2real-tagprefix'], a genuine strict prefix of the tag value 'sim2real-tagprefix-test', must NOT match ... got ['/<base>/p']".
Cause 4 — ssm.get-parameters-by-path-filters-by-type: "expected the call to be refused but it succeeded: {'Parameters': [... '/<base>/list' StringList ..., '/<base>/str' String ...], 'ResponseMetadata': {'HTTPStatusCode': 200 ...}}" for {Key:'Type',Option:'BeginsWith',Values:['Str']}; expected 400 InvalidFilterValue with 'The following filter value is not valid: Str. Valid values include: [String, StringList, SecureString].'
Cause 5 — ssm.get-parameters-by-path-pagination-token-exactness: "the call was expected to be refused but it succeeded: {... 'NextToken': '/<base>/leaf02', 'HTTPStatusCode': 200 ...}" for NextToken='sim2real-behav-not-a-real-token'; expected 400 InvalidNextToken with 'The following token is invalid: sim2real-behav-not-a-real-token.'
Cause 6 — ssm.expiration-notification-requires-expiration-policy: "expected a ClientError, but the call SUCCEEDED and returned {'Version': 1, 'Tier': 'Advanced', ... 'HTTPStatusCode': 200}"; expected 400 IncompatiblePolicyException and no parameter created.
Cause 7 — ssm.parameter-policies-survive-a-value-update-...: "an Overwrite that sends no Policies must PRESERVE the existing policies ('All existing policies are preserved until you send new policies or an empty policy'); after the update the parameter carries []" (expected ['Expiration','NoChangeNotification']).
Cause 8 — ssm.secure-string-customer-managed-kms-key-overwrite-rekeys-history: "omitting KeyId on overwrite must keep reusing the previous key '<cmk-id>', not revert to a default key; DescribeParameters reported 'alias/aws/ssm'".
Cause 9 — ssm.parameter-hierarchy-depth-and-prefix-conflict-rejected: uncaught botocore.errorfactory.HierarchyLevelLimitExceededException on PutParameter for the 16-segment name '/<base>/l2/.../l16', message "Name '<name>' is 16 levels deep: a parameter hierarchy is limited to a maximum depth of 15 levels"; the check asserts that name succeeds with Version 1 and that only the 17-segment name is refused, with message 'Depth of the tree cannot exceed 15'.
Cause 10 — ssm.role-scoped-to-parameter-path-prefix-denies-outside-subtree: "expected a rejection, but the call SUCCEEDED and returned {'Parameter': {'Name': '/<base>/other/x', ... }, 'HTTPStatusCode': 200}".

Smithy model consulted: services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json (GetParametersByPath errors InvalidFilterKey / InvalidFilterOption / InvalidFilterValue / InvalidNextToken, MaxResults range 1..10, paginated NextToken; PutParameter errors HierarchyLevelLimitExceededException / IncompatiblePolicyException; DescribeParameters ParameterFilters).

## Steps to Reproduce
1. Build and boot the emulator from this checkout. 2. Replay simparity-behavioral-aws-v1 and re-run the check ids in trace_ids. 3. Per cause: GetParameter '<name>:2' and '<name>:<label>' and GetParameters with those selectors, and inspect Parameter.Selector (cause 1); GetParametersByPath with ParameterFilters=[{Key:'tag:env',...}] (cause 2); DescribeParameters with {Key:'tag:env',Option:'BeginsWith',Values:[<strict prefix of the tag value>]} (cause 3); GetParametersByPath with {Key:'Type',Option:'BeginsWith',Values:['Str']} (cause 4); GetParametersByPath with NextToken='sim2real-behav-not-a-real-token', and inspect the NextToken it mints at MaxResults=2 (cause 5); PutParameter Tier='Advanced' with only an ExpirationNotification policy (cause 6); PutParameter with Expiration+NoChangeNotification, then Overwrite with a new Value and no Policies, then DescribeParameters (cause 7); PutParameter SecureString with a customer-managed KeyId, then Overwrite omitting KeyId, then DescribeParameters (cause 8); PutParameter for a 16-segment name, then a 17-segment name (cause 9); assume a role whose policy allows ssm:GetParameter only under /<prefix>/* and GetParameter a name outside it (cause 10).
