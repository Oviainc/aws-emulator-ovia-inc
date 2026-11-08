Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.type-not-found-across-apis-rejected cloudformation.publisher-and-type-testing-lifecycle cloudformation.type-version-publish-and-test
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.type-not-found-across-apis-rejected|run#20|AssertionError|assert publish_error == expected_publish, (
Issue-Key: cloudformation.publisher-and-type-testing-lifecycle|run#13|AssertionError|assert publish_message == expected, f"expected message {expected!r}, got {publish_message!r}"
Issue-Key: cloudformation.type-version-publish-and-test|run#13|AssertionError|assert publish_error == expected_publish, (
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# cloudformation PublishType: no publisher-verification pre-flight and the default-version rule is unreachable — every refusal collapses to 'Type not found in your account.' (2 causes)

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PublishType

## Description
REOPENS 20261029T040629Z-cloudformation-2-net-new-root-causes-stackset-operationnotfoundexception.md: its fix left the call refused. That issue's Cause 2 reordered PublishType so that account-scoped type existence is resolved before the ARN version-shape rule; after the fix PublishType answers `Type not found in your account.` for three distinct request classes, two of which AWS refuses with a different message. Two net-new root causes of service `cloudformation`, grounded in three failing behavioral checks. Each cause is independent and must be closed separately.

=== Cause 1 of 2: PublishType has no publisher-verification pre-flight ===
AWS: Smithy (services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json) models PublishType with input Arn (PrivateTypeArn, @length 0..1024, pattern ^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}:[0-9]{12}:type/.+$), TypeName, Type, PublicVersionNumber, and errors `CFNRegistryException` (errorKind client, httpError 400, awsQueryError CFNRegistryException/400) and `TypeNotFoundException` (client, httpError 404). The model states no publisher rule, so that rule is recorded behavior only: publishing requires the CALLING ACCOUNT to be a verified CloudFormation extension publisher, and an account that is not one is refused FIRST — before the ARN version-shape rule and before any account-scoped type lookup — with Error.Code `CFNRegistryException` and Error.Message `The account <account-id> is not verified as a publisher. Please register as publisher first`. Fixed text is `The account ` + ` is not verified as a publisher. Please register as publisher first`; the single variable part is the caller's 12-digit account id, taken from the request's credentials, not from any member.
Emulator: for that same request it answers CFNRegistryException with `Type not found in your account.` — the registry's existence refusal. The publisher pre-flight does not exist at all, so an unverified publisher is never told to call RegisterPublisher.
Why: the caller is told its extension is missing when the real blocker is its own publisher status. Registry tooling branches on this message to run RegisterPublisher; against the emulator it instead re-registers or re-names the type and never gets past the gate. Worse, for a type that DOES resolve, an unverified account's PublishType would be allowed to proceed, since nothing in the handler ever asks whether the caller is a publisher.
Fix: the PublishType handler in the cloudformation crate's type-registry ops — consult the calling account's publisher registration (the state RegisterPublisher writes) and refuse with the modelled CFNRegistryException carrying the message above BEFORE the version rule and before any registry resolution.
Traces: cloudformation.type-not-found-across-apis-rejected.

=== Cause 2 of 2: the default-version rule is never reached, so an ARN carrying a version segment is reported as a missing type ===
AWS: recorded behavior only — for a verified publisher, an `Arn` whose `type/...` suffix carries a trailing version segment is refused with Error.Code `CFNRegistryException`, Error.Message `Only default version can be published. Please remove versionId from Arn` (entirely fixed text, nothing interpolated). This refusal is about the ARN's SHAPE and is reached whether or not the named type is registered; the modelled PrivateTypeArn pattern admits any `type/.+` suffix, so the request is well formed and the rule lives in the handler.
Emulator: answers CFNRegistryException `Type not found in your account.` — the existence refusal, now unconditionally first. The version rule is dead code for any type the account has not registered.
Why: a client cannot distinguish 'this ARN names a non-default version' (strip the version and retry) from 'this type is not in my account' (register it). The emulator gives the second answer to the first question, so the retry it prescribes can never succeed.
Fix: the same PublishType handler's refusal ordering — publisher verification (Cause 1), then the ARN version-segment rule, then the account-scoped type lookup. Validation ORDER is the contract here, not just the message text.
Traces: cloudformation.publisher-and-type-testing-lifecycle, cloudformation.type-version-publish-and-test.

## Evidence
Cause 1 — cloudformation.type-not-found-across-apis-rejected [mismatch], check line 87:
  assert publish_error == expected_publish
  AssertionError: PublishType returned ('CFNRegistryException', 'Type not found in your account.'), expected ('CFNRegistryException', 'The account 000000000000 is not verified as a publisher. Please register as publisher first')
  (000000000000 is the emulator's caller account id filling the message's one variable slot; the requirement is the template, not that literal)

Cause 2 — cloudformation.publisher-and-type-testing-lifecycle [mismatch]:
  AssertionError: expected message 'Only default version can be published. Please remove versionId from Arn', got 'Type not found in your account.'
  cloudformation.type-version-publish-and-test [mismatch]:
  AssertionError: expected ('CFNRegistryException', 'Only default version can be published. Please remove versionId from Arn') from PublishType on a nonexistent type version, got ('CFNRegistryException', 'Type not found in your account.')

Smithy consulted: PublishType input {Type: ThirdPartyType enum; Arn: PrivateTypeArn @length 0..1024 pattern ^arn:aws[A-Za-z0-9-]{0,64}:cloudformation:[A-Za-z0-9-]{1,64}:[0-9]{12}:type/.+$; TypeName @length 10..204 pattern ^[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}::[A-Za-z0-9]{2,64}(::MODULE){0,1}$; PublicVersionNumber @length min 5 pattern ^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(.*)$}, output {PublicTypeArn}, errors = CFNRegistryException (client/400) and TypeNotFoundException (client/404), idempotent. All three messages are HTTP 400 / CFNRegistryException — only the message text differs, so the divergence is which precondition is evaluated and in what order.

## Steps to Reproduce
Cause 1:
1. From an account that has NOT called RegisterPublisher, call cfn.publish_type(Arn='arn:aws:cloudformation:<region>:<caller-account>:type/resource/Sim2realBehav-Probe-<token>').
2. Observe 400 CFNRegistryException 'Type not found in your account.'. AWS answers 400 CFNRegistryException 'The account <caller-account> is not verified as a publisher. Please register as publisher first'.

Cause 2:
1. Call RegisterPublisher so the account is a verified publisher.
2. Call cfn.publish_type(Arn='arn:aws:cloudformation:<region>:<caller-account>:type/resource/Sim2realBehav-Probe-<token>/00000001') — a well-formed PrivateTypeArn carrying a version segment, naming a type registered nowhere.
3. Observe 400 CFNRegistryException 'Type not found in your account.'. AWS answers 400 CFNRegistryException 'Only default version can be published. Please remove versionId from Arn'.
4. Ordering control a message-only fix would leave broken: register a private type so the ARN DOES resolve, then publish it with a version segment — it must still be refused with the version message.
5. Re-replay: cloudformation.type-not-found-across-apis-rejected, cloudformation.publisher-and-type-testing-lifecycle, cloudformation.type-version-publish-and-test.
