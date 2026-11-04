Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.put-parameter-without-overwrite-fails ssm.put-parameter-without-overwrite-on-an-existing-flag-rejected ssm.deploy-ssm-parameter-via-cloudformation-then-behaves ssm.put-parameter-rejects-pattern-mismatch ssm.allowed-pattern-rejected-overwrite-burns-no-version ssm.secure-string-allowed-pattern-validates-plaintext ssm.string-list-allowed-pattern-applies-per-element ssm.get-parameter-rejects-unknown-version ssm.get-parameter-invalid-label-fails ssm.ssm-concurrent-overwrites-keep-every-version-byte-exact-and-both-read-paths-agree ssm.delete-parameter-destroys-every-version-and-the-dead-name-refuses-mutations ssm.secure-string-reads-plaintext-only-with-decryption-and-is-gone-after-delete ssm.secure-string-roundtrip-decryption ssm.describe-parameters-label-filter-rejected ssm.get-parameters-by-path-unsupported-filter-keys-rejected ssm.get-parameters-by-path-filters-by-label-pins-version ssm.deep-secrets-and-params-parameter-version-ceiling-evicts-oldest-and-labelled-oldest-rejected ssm.label-parameter-version-rejects-over-label-quota ssm.parameter-tier-upgrade-is-one-way-and-intelligent-tiering-derives-the-tier ssm.put-parameter-policies-without-advanced-tier-rejected
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm.put-parameter-without-overwrite-fails|run#10|AssertionError|assert message == (
Issue-Key: ssm.put-parameter-without-overwrite-on-an-existing-flag-rejected|run#8|AssertionError|assert message == (
Issue-Key: ssm.deploy-ssm-parameter-via-cloudformation-then-behaves|run#21|AssertionError|assert message == (
Issue-Key: ssm.put-parameter-rejects-pattern-mismatch|run#15|AssertionError|assert code == "ParameterPatternMismatchException", (
Issue-Key: ssm.allowed-pattern-rejected-overwrite-burns-no-version|run#13|AssertionError|assert code == "ParameterPatternMismatchException", (
Issue-Key: ssm.secure-string-allowed-pattern-validates-plaintext|run#17|AssertionError|assert code == "ParameterPatternMismatchException", (
Issue-Key: ssm|PutParameter|InvalidAllowedPatternException
Issue-Key: ssm.get-parameter-rejects-unknown-version|run#17|AssertionError|assert message_missing_version == (
Issue-Key: ssm.get-parameter-invalid-label-fails|run#15|AssertionError|assert message == (
Issue-Key: ssm|GetParameter|ParameterVersionNotFound
Issue-Key: ssm.delete-parameter-destroys-every-version-and-the-dead-name-refuses-mutations|run#28|AssertionError|assert relabel == relabel_gone, (
Issue-Key: ssm.secure-string-reads-plaintext-only-with-decryption-and-is-gone-after-delete|run#19|AssertionError|assert got == ("ParameterNotFound", f"Parameter {name} not found."), (
Issue-Key: ssm.secure-string-roundtrip-decryption|run#24|AssertionError|assert message == expected_message, f"expected the message {expected_message!r}, got {message!r}"
Issue-Key: ssm.describe-parameters-label-filter-rejected|run#14|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: ssm.get-parameters-by-path-unsupported-filter-keys-rejected|run#7|AssertionError|assert message == expected_message, (
Issue-Key: ssm.get-parameters-by-path-filters-by-label-pins-version|run#24|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: ssm.deep-secrets-and-params-parameter-version-ceiling-evicts-oldest-and-labelled-oldest-rejected|run#11|AssertionError|assert message == _max_version_message(name, 1), message
Issue-Key: ssm.label-parameter-version-rejects-over-label-quota|run#15|AssertionError|assert err.get("Message") == expected_message, (
Issue-Key: ssm.parameter-tier-upgrade-is-one-way-and-intelligent-tiering-derives-the-tier|run#17|AssertionError|assert std_message == _STANDARD_CEILING_MESSAGE, std_message
Issue-Key: ssm.put-parameter-policies-without-advanced-tier-rejected|run#8|AssertionError|assert omitted_message == _STANDARD_TIER_MESSAGE, omitted_message
Issue-Commit: 69e3918bc598d03b2c669434388419738bf97909

# SSM Parameter Store refusal messages and error classes are emulator-authored (10 causes, behavioral replay)

> _Note: this issue was identified against the code base at commit `69e3918bc598d03b2c669434388419738bf97909`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutParameter

## Description
Every Parameter Store refusal below returns the right HTTP status (400) but the wrong error class or a message the service never writes — several carry no Message member at all. 10 CAUSES. Each AWS message is a TEMPLATE: the quoted text is fixed, and the variable parts (parameter name, version number, label, filter key, limit) come from the request and the emulator's own state. No value from one recording is itself the requirement.

Related open issue: 20261103T173026Z-ssm-parameter-store-error-classes-and-messages-are-emulator.md describes the same formatter from the WIRE corpus and overlaps causes 1, 2, 3, 4 and 5 here; it does not carry these behavioral check ids, so this issue attaches them. Causes 6-10 are not in that issue.

Cause 1 of 10 — PutParameter on an existing name without Overwrite.
AWS: 400 ParameterAlreadyExists (Smithy: PutParameter error ParameterAlreadyExists, httpError 400) with the wholly fixed sentence, which contains no name: 'The parameter already exists. To overwrite this value, set the overwrite option in the request to true.'
Emulator: 400 ParameterAlreadyExists with the authored sentence 'Parameter <name> already exists; set Overwrite to update it' (name taken from the request).
Why: a client that matches or surfaces AWS's sentence — the one that names the 'overwrite' option — never sees it, and the CloudFormation-managed parameter check fails on the same string.
Fix: the PutParameter handler's already-exists error construction.
Traces: ssm.put-parameter-without-overwrite-fails, ssm.put-parameter-without-overwrite-on-an-existing-flag-rejected, ssm.deploy-ssm-parameter-via-cloudformation-then-behaves.

Cause 2 of 10 — a Value that violates AllowedPattern is refused under the wrong error class.
AWS: 400 ParameterPatternMismatchException (Smithy: PutParameter models BOTH ParameterPatternMismatchException and InvalidAllowedPatternException; the latter means the supplied AllowedPattern itself is malformed).
Emulator: 400 InvalidAllowedPatternException with "Value doesn't satisfy the regular expression requirement '<pattern>'".
Why: the two classes mean opposite things. A caller catching ParameterPatternMismatchException to re-prompt for a value never sees it; one catching InvalidAllowedPatternException wrongly concludes its own pattern is bad. The same wrong class is returned for String, StringList (per element) and SecureString plaintext.
Fix: PutParameter validation — classify a value/AllowedPattern mismatch as ParameterPatternMismatchException.
Traces: ssm.put-parameter-rejects-pattern-mismatch, ssm.allowed-pattern-rejected-overwrite-burns-no-version, ssm.secure-string-allowed-pattern-validates-plaintext, ssm.string-list-allowed-pattern-applies-per-element.

Cause 3 of 10 — ParameterVersionNotFound message, both the version-selector and the label-selector form.
AWS: 400 ParameterVersionNotFound (Smithy: GetParameter error ParameterVersionNotFound). For '<name>:<version>': 'Systems Manager could not find version <version> of <name>. Verify the version and try again.' For '<name>:<label>': 'Systems Manager could not find a version of <name> that has the specified label, <label>. Verify the parameter label and try again.' In both the name is rendered WITHOUT its leading '/' for a one-level name; the version/label are the request's.
Emulator: 400 ParameterVersionNotFound with 'Parameter <name> has no version or label '<selector>'' — one authored sentence for both cases, name with its leading slash.
Fix: the GetParameter selector-resolution error construction.
Traces: ssm.get-parameter-rejects-unknown-version, ssm.get-parameter-invalid-label-fails, ssm.ssm-concurrent-overwrites-keep-every-version-byte-exact-and-both-read-paths-agree.

Cause 4 of 10 — LabelParameterVersion on a name that does not exist returns ParameterNotFound with an EMPTY message.
AWS: 400 ParameterNotFound (Smithy: LabelParameterVersion error ParameterNotFound) with 'Parameter <name> not found.' — fixed text around the requested name. (Note the asymmetry AWS really has: GetParameter's own ParameterNotFound carries no message, and the checks assert that empty message, so this is not a blanket 'always populate ParameterNotFound' change.)
Emulator: 400 ParameterNotFound with Message absent/empty.
Why: the caller gets an empty error body and cannot report which name was missing.
Fix: the LabelParameterVersion not-found path.
Traces: ssm.delete-parameter-destroys-every-version-and-the-dead-name-refuses-mutations, ssm.secure-string-reads-plaintext-only-with-decryption-and-is-gone-after-delete, ssm.secure-string-roundtrip-decryption.

Cause 5 of 10 — InvalidFilterKey carries no Message, on both DescribeParameters and GetParametersByPath.
AWS: 400 InvalidFilterKey (Smithy: both operations model InvalidFilterKey; the shape declares no members, yet the service populates Message — recorded behavior only) with 'The following filter key is not valid: <key>. Valid filter keys include: [<the operation's legal keys>].' The legal-key list differs per operation: DescribeParameters prints [Path, Name, Type, KeyId, Tier, DataType]; GetParametersByPath prints [Type, KeyId, Label].
Emulator: 400 InvalidFilterKey with Message absent.
Why: the caller is told only that some key was rejected, not which or what is legal.
Fix: the ParameterFilters validation shared by DescribeParameters and GetParametersByPath — populate InvalidFilterKey.Message with the per-operation key list.
Traces: ssm.describe-parameters-label-filter-rejected, ssm.get-parameters-by-path-unsupported-filter-keys-rejected.

Cause 6 of 10 — InvalidFilterOption message is authored prose.
AWS: 400 InvalidFilterOption (Smithy: GetParametersByPath error InvalidFilterOption) with 'The following filter option is not valid: <option>. Valid options include: [<the key's legal options>].' — e.g. for Key=Label with Option=BeginsWith the valid-options list is [Equals].
Emulator: 400 InvalidFilterOption with "Option '<option>' at filter key '<key>' isn't valid: valid options are <list>".
Fix: the ParameterFilters option validation message.
Traces: ssm.get-parameters-by-path-filters-by-label-pins-version.

Cause 7 of 10 — ParameterMaxVersionLimitExceeded message.
AWS: 400 ParameterMaxVersionLimitExceeded (Smithy: PutParameter error ParameterMaxVersionLimitExceeded) with 'You attempted to create a new version of <name> by calling the PutParameter API with the overwrite flag. Version <oldest>, the oldest version, can\u2019t be deleted because it has a label associated with it. Move the label to another version of the parameter, and try again.' — the name is rendered without its leading '/', <oldest> is the current oldest version number from state, and the apostrophe is U+2019. The sentence is re-rendered with the recomputed oldest version on each subsequent refusal.
Emulator: 400 with 'Parameter <name> retains 100 versions and its oldest version can't be deleted because it has a label associated with it' — authored text that never names the version the caller must unlabel.
Fix: the PutParameter version-ceiling refusal path.
Traces: ssm.deep-secrets-and-params-parameter-version-ceiling-evicts-oldest-and-labelled-oldest-rejected.

Cause 8 of 10 — the per-version label quota refusal message.
AWS: 400 (Smithy: LabelParameterVersion error ParameterVersionLabelLimitExceeded; Labels @length max 10) with 'A parameter version can have maximum 10 labels.Move one or more labels to another version and try again.' — fixed text, reproduced exactly including the missing space after the full stop.
Emulator: 'A parameter version can have a maximum of ten labels.'
Fix: the LabelParameterVersion label-quota refusal message.
Traces: ssm.label-parameter-version-rejects-over-label-quota.

Cause 9 of 10 — the Standard-tier value-size ceiling refusal message.
AWS: 400 with 'Standard tier parameters support a maximum parameter value of 4096 characters. To create a larger parameter value, upgrade the parameter to use the advanced-parameter tier. For more information, see https://docs.aws.amazon.com/systems-manager/latest/userguide/parameter-store-advanced-parameters.html' — entirely fixed text with no variable part (recorded behavior only; Smithy puts no @length on PSParameterValue).
Emulator: 'Value is <n> bytes: a Standard parameter has a content size limit of 4096 bytes' — authored, echoes the request's byte count and offers no upgrade instruction.
Fix: the PutParameter tier/size validation message.
Traces: ssm.parameter-tier-upgrade-is-one-way-and-intelligent-tiering-derives-the-tier.

Cause 10 of 10 — the policies-require-Advanced-tier refusal message.
AWS: 400 with "You can't assign parameter policies to a standard-parameter tier. You must upgrade the parameter to use the advanced-parameter tier. For more information, see https://docs.aws.amazon.com/systems-manager/latest/userguide/parameter-store-advanced-parameters.html" — entirely fixed text (recorded behavior only; Smithy models PutParameter error IncompatiblePolicyException with no message rule).
Emulator: 'Policies require an Advanced parameter; this request would create a Standard parameter'.
Fix: the PutParameter Policies/tier precondition message.
Traces: ssm.put-parameter-policies-without-advanced-tier-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout; all 20 checks below are listed under '### ssm' in the suite report.

Cause 1 — ssm.put-parameter-without-overwrite-fails / ...-on-an-existing-flag-rejected: AssertionError "unexpected message 'Parameter /sim2real-behav-probe-param-noverwrite-<id> already exists; set Overwrite to update it'"; ssm.deploy-ssm-parameter-via-cloudformation-then-behaves asserts message == 'The parameter already exists. To overwrite this value, set the overwrite option in the request to true.' and got the same authored sentence.
Cause 2 — ssm.put-parameter-rejects-pattern-mismatch: "expected ParameterPatternMismatchException, got 'InvalidAllowedPatternException'"; ssm.allowed-pattern-rejected-overwrite-burns-no-version: "an overwrite whose value violates AllowedPattern must be refused with ParameterPatternMismatchException, got 'InvalidAllowedPatternException'"; ssm.secure-string-allowed-pattern-validates-plaintext: same for a SecureString overwrite; ssm.string-list-allowed-pattern-applies-per-element: uncaught botocore.errorfactory.InvalidAllowedPatternException — "Value doesn't satisfy the regular expression requirement '^[0-9]+$'".
Cause 3 — ssm.get-parameter-rejects-unknown-version: expected 'Systems Manager could not find version 99 of <name>. Verify the version and try again.', got "Parameter /<name> has no version or label '99'"; ssm.get-parameter-invalid-label-fails: expected 'Systems Manager could not find a version of <name> that has the specified label, no-such-label. Verify the parameter label and try again.', got "Parameter /<name> has no version or label 'no-such-label'"; ssm.ssm-concurrent-overwrites-...: same version-form sentence expected, got "Parameter /<name> has no version or label '10'".
Cause 4 — ssm.delete-parameter-destroys-every-version-...: "labelling a version of a deleted parameter must be refused with ('ParameterNotFound', 'Parameter /<name> not found.'), got ('ParameterNotFound', '')"; ssm.secure-string-reads-plaintext-only-...: "LabelParameterVersion on the deleted parameter: got ('ParameterNotFound', '')"; ssm.secure-string-roundtrip-decryption: "expected the message 'Parameter /<name> not found.', got ''".
Cause 5 — ssm.describe-parameters-label-filter-rejected: "expected message 'The following filter key is not valid: Label. Valid filter keys include: [Path, Name, Type, KeyId, Tier, DataType].', got ''"; ssm.get-parameters-by-path-unsupported-filter-keys-rejected: "for filter key 'Name' expected message 'The following filter key is not valid: Name. Valid filter keys include: [Type, KeyId, Label].', got ''".
Cause 6 — ssm.get-parameters-by-path-filters-by-label-pins-version: "expected message 'The following filter option is not valid: BeginsWith. Valid options include: [Equals].', got \"Option 'BeginsWith' at filter key 'Label' isn't valid: valid options are Equals\"".
Cause 7 — ssm.deep-secrets-...-version-ceiling-...: got 'Parameter /sim2real-deep-ssm-<id>/ceiling retains 100 versions and its oldest version can't be deleted because it has a label associated with it'.
Cause 8 — ssm.label-parameter-version-rejects-over-label-quota: "expected message 'A parameter version can have maximum 10 labels.Move one or more labels to another version and try again.', got 'A parameter version can have a maximum of ten labels.'".
Cause 9 — ssm.parameter-tier-upgrade-is-one-way-...: got 'Value is 5000 bytes: a Standard parameter has a content size limit of 4096 bytes'.
Cause 10 — ssm.put-parameter-policies-without-advanced-tier-rejected: got 'Policies require an Advanced parameter; this request would create a Standard parameter'.

Smithy model consulted: services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json (PutParameter errors ParameterAlreadyExists / ParameterPatternMismatchException / InvalidAllowedPatternException / ParameterMaxVersionLimitExceeded / IncompatiblePolicyException; GetParameter errors ParameterNotFound, ParameterVersionNotFound; LabelParameterVersion errors ParameterNotFound, ParameterVersionLabelLimitExceeded with Labels @length 1..10; DescribeParameters and GetParametersByPath errors InvalidFilterKey / InvalidFilterOption / InvalidFilterValue).

## Steps to Reproduce
1. Build and boot the emulator from this checkout. 2. Replay the behavioral suite simparity-behavioral-aws-v1 and re-run exactly the check ids in trace_ids. 3. Per cause: PutParameter twice for the same Name with no Overwrite (cause 1); PutParameter with AllowedPattern='^[0-9]+$' and a non-matching Value, as String, StringList and SecureString (cause 2); GetParameter on '<name>:99' and on '<name>:no-such-label' for an existing parameter (cause 3); DeleteParameter then LabelParameterVersion on the dead name (cause 4); DescribeParameters with ParameterFilters key 'Label' and GetParametersByPath with key 'Name' (cause 5); GetParametersByPath with {Key:'Label', Option:'BeginsWith'} (cause 6); drive one parameter to its 100-version ceiling with the oldest version labelled and Overwrite again (cause 7); LabelParameterVersion with an 11th label on one version (cause 8); PutParameter Tier='Standard' with a 5000-byte Value (cause 9); PutParameter with Policies and no Tier='Advanced' (cause 10). 4. Compare Error.Code and Error.Message against the templates in the description.
