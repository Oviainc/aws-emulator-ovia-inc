Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.delete-default-parameter-group-forbidden elasticache.modify-and-reset-cache-parameter-group elasticache.tag-key-reserved-prefix-and-length-rejected elasticache.tag-unicode-value-round-trip elasticache.tags-add-list-remove elasticache.user-group-modify-membership
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.delete-default-parameter-group-forbidden|_assert_error#18|AssertionError|assert got == message, f"{expected}: expected message {message!r}, got {got!r}"
Issue-Key: elasticache.modify-and-reset-cache-parameter-group|_assert_error#18|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: elasticache.tag-key-reserved-prefix-and-length-rejected|_assert_error#15|AssertionError|assert actual == message, f"{what}: expected message {message!r}, got {actual!r}"
Issue-Key: elasticache|AddTagsToResource|InvalidParameterValue
Issue-Key: elasticache.tags-add-list-remove|_assert_error#16|AssertionError|assert actual == message, f"{expected}: expected message {message!r}, got {actual!r}"
Issue-Key: elasticache.user-group-modify-membership|_assert_error#14|AssertionError|assert got == message, f"{code}: expected message {message!r}, got {got!r}"
Issue-Commit: b3076e7c0c2ebb8fac474fe10cf99e18802cbe88

# ElastiCache refusal fidelity: CacheParameterGroupName identifier validation missing and five handler refusals carry paraphrased messages instead of AWS's (6 causes)

> _Note: this issue was identified against the code base at commit `b3076e7c0c2ebb8fac474fe10cf99e18802cbe88`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteCacheParameterGroup

## Description
Six distinct root causes in the `elasticache` service, all reached over the ordinary Query surface with no supporting EC2/VPC resources (so none is blocked by another service's gap). In every one of them the request IS refused and the error CODE is the modeled one; what diverges is which rule fires (cause 1 — and there the HTTP status diverges too, for the general case) and the refusal text the handler authors (causes 2-6). Each cause is a separate rule in a separate component and must be closed separately. These are NOT covered by the open issues 20261023T155010Z-elasticache-control-plane-is-unusable-end-to-end.md (subnet groups / lifecycle operations), 20261023T155010Z-elasticache-read-validate-fidelity-unknown-engine-accepted-events.md (describe/engine-catalog fidelity) or 20261024T075915Z-elasticache-rbac-surface-deleteusergroup-and-modifyuser-answer-501.md (501 stubs): none of those names any of the six rules below.

Cause 1 of 6 — DeleteCacheParameterGroup does not validate `CacheParameterGroupName` against the identifier grammar, so a malformed name is answered by the existence/default-group rules instead.
AWS: Smithy (com-amazonaws-elasticache-amazonelasticachev9) binds `DeleteCacheParameterGroup` with input `DeleteCacheParameterGroupMessage{CacheParameterGroupName (required)}`, output `Unit` at 200, modeled errors `CacheParameterGroupNotFoundFault` (httpError 404, awsQueryError code `CacheParameterGroupNotFound`), `InvalidCacheParameterGroupStateFault` (400, `InvalidCacheParameterGroupState`), `InvalidParameterValueException` (400, `InvalidParameterValue`) and `InvalidParameterCombinationException` (400, `InvalidParameterCombination`). AWS validates the identifier FORM of `CacheParameterGroupName` before it resolves the name against anything: an identifier must begin with a letter, contain only ASCII letters, digits and hyphens, not end with a hyphen and not contain two consecutive hyphens. A name that breaks that rule — a name containing a '.', such as a `default.<family>` group name, but equally `9x`, `bad--name`, `trailing-` — is refused HTTP 400 `InvalidParameterValue` with the message built from the request's member name and that constraint: "The parameter CacheParameterGroupName is not a valid identifier. Identifiers must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens." (That rule is why AWS never reaches the "default groups cannot be deleted" rule for a `default.*` name: the name is not a legal identifier in the first place.)
Emulator: `delete_cache_parameter_group.rs` performs no identifier-form check at all. It lowercases the name, looks it up in the store, and refuses an unresolvable name with HTTP 404 `CacheParameterGroupNotFound` ("CacheParameterGroup <name> not found."); a name in the `default.` namespace is refused 400 `InvalidParameterValue` but with "Invalid value default.redis7 for CacheParameterGroupName: the default cache parameter groups in your account cannot be deleted."
Why: a client that sends a syntactically illegal group name gets the wrong error CLASS — HTTP 404 `CacheParameterGroupNotFound` instead of HTTP 400 `InvalidParameterValue` — so "you typed an illegal name" is indistinguishable from "that group does not exist", and retry/branch logic that treats not-found as benign (create-if-absent) loops on a name that can never be created either. For the `default.*` case the status and code coincide but the diagnosis text does not, so a caller cannot learn the actual naming rule from the response.
Fix: the operation's validation in the handler — the identifier rule is ALREADY implemented centrally in `crate::state` (the helper that renders "The parameter {parameter} is not a valid identifier. …" via `is_valid_identifier`) and is simply not called here; call it on `CacheParameterGroupName` BEFORE the store lookup and before the `default.` check, and let the existing default-group refusal apply only to names that are legal identifiers.
Traces: elasticache.delete-default-parameter-group-forbidden.

Cause 2 of 6 — ModifyCacheParameterGroup's non-modifiable-parameter refusal is worded as its own sentence rather than AWS's.
AWS: Smithy binds `ModifyCacheParameterGroup{CacheParameterGroupName (required), ParameterNameValues (required)}` -> `CacheParameterGroupNameMessage`, with `InvalidParameterValueException` (400, `InvalidParameterValue`) among its modeled errors. A request that asks to set a parameter the engine publishes as non-modifiable is refused HTTP 400 `InvalidParameterValue` with a message built from the request — it names the offending parameter and the constraint: "The parameter <name> cannot be modified." (observed for `activerehashing`).
Emulator: HTTP 400 `InvalidParameterValue` (code correct) with "Parameter activerehashing is not modifiable."
Why: the message is the only machine-free statement of WHICH submitted parameter was rejected and why; tooling and tests that surface or match AWS's sentence for a non-modifiable parameter see a different sentence for the identical condition.
Fix: the shared parameter-value validation in `crate::state` (`check_parameter_value`, the single place every writer of parameter values goes through) must render the member-and-constraint message in AWS's form for the non-modifiable case; `modify_cache_parameter_group.rs` keeps using it.
Traces: elasticache.modify-and-reset-cache-parameter-group.

Cause 3 of 6 — AddTagsToResource's reserved-`aws:`-prefix refusal is worded as a per-value message instead of AWS's rule sentence.
AWS: Smithy binds `AddTagsToResource{ResourceName (required), Tags (required)}` -> `TagListMessage` with `InvalidParameterValueException` (400, `InvalidParameterValue`) modeled. The `aws:` prefix is reserved for AWS-managed tags (no `@pattern` on the tag key in the model, so the rule is enforced server-side only); a tag key starting with it is refused HTTP 400 `InvalidParameterValue` stating the rule: 'Tag keys cannot start with the reserved prefix "aws:".'
Emulator: HTTP 400 `InvalidParameterValue` (code correct) with "Invalid value aws:sim2real-forged for Tags: a tag's Key may not start with the reserved prefix aws:."
Why: callers that assert on or display AWS's reserved-prefix sentence see a different one; the emulator's form also echoes the rejected key back into the message where AWS states the rule alone.
Fix: the tag-key validation in `crate::state` (the key-refusal helper that renders "Invalid value {key} for Tags: a tag's Key may not start with the reserved prefix aws:.") — it is the one statement of the rule shared by every `Tags` writer, so changing it there fixes AddTagsToResource and every create that accepts `Tags`.
Traces: elasticache.tag-key-reserved-prefix-and-length-rejected.

Cause 4 of 6 — the tag-value character-set refusal text does not match AWS's published sentence.
AWS: recorded behavior only (the model puts no `@pattern` on a tag value). A tag value containing a character outside "unicode letters, digits, whitespace" plus the symbol set is refused HTTP 400 `InvalidParameterValue`, and AWS's sentence carries the symbol list with a DOUBLED backslash and a trailing stray double-quote: `Tag values may only contain unicode letters, digits, whitespace, or one of these symbols: _ . : / = + \\ - @"`.
Emulator: the same code, with the symbol list normalized to a single backslash and no trailing quote: `Tag values may only contain unicode letters, digits, whitespace, or one of these symbols: _ . : / = + \ - @`.
Why: a client (or a golden-output test) that matches AWS's exact refusal text for an illegal tag value does not match. The surrounding happy path of this check — unicode tag values (emoji, CJK, Cyrillic, accented Latin) surviving AddTagsToResource/ListTagsForResource byte-for-byte — is reached only after this refusal is observed, so the divergence also hides that round-trip from the suite.
Fix: the tag charset refusal in `crate::state` (`tag_charset_message`, which builds the sentence for "keys"/"values") must emit the service's published text verbatim, including the doubled backslash and trailing quote, rather than a tidied-up transcription.
Traces: elasticache.tag-unicode-value-round-trip.

Cause 5 of 6 — RemoveTagsFromResource's TagNotFound refusal reports one tag per resource ARN instead of AWS's list of the absent keys.
AWS: Smithy binds `RemoveTagsFromResource{ResourceName (required), TagKeys (required)}` -> `TagListMessage`, with `TagNotFoundFault` (httpError 404, awsQueryError code `TagNotFound`) modeled. Removing a key that is not present on the resource is refused HTTP 404 `TagNotFound`, with a message built from the request that lists the absent KEYS: "Failing request. These tags are not present : never-existed".
Emulator: HTTP 404 `TagNotFound` (code correct) with "The tag never-existed was not found on resource arn:aws:elasticache:<region>:<account>:parametergroup:<name>."
Why: the emulator's sentence is per-resource rather than per-key-set, so a caller removing several absent keys cannot read the set of offending keys out of the response the way AWS's plural form allows; it also leaks the resource ARN into a message AWS keys on the tags.
Fix: `remove_tags_from_resource.rs` must collect every requested key that is absent and refuse once with the request-derived list form, instead of refusing on the first key with a resource-centric sentence.
Traces: elasticache.tags-add-list-remove.

Cause 6 of 6 — CreateUserGroup's already-exists refusal drops AWS's remediation clause.
AWS: Smithy binds `CreateUserGroup{UserGroupId (required), Engine (required), UserIds, Tags}` -> `UserGroup`, with `UserGroupAlreadyExistsFault` (httpError 400, awsQueryError code `UserGroupAlreadyExists`) modeled. Re-creating an existing user-group id is refused HTTP 400 `UserGroupAlreadyExists` with a message built from the request id plus a fixed remediation clause: "User group <id> already exists. Please retry with another name."
Emulator: HTTP 400 `UserGroupAlreadyExists` (code correct) with "User group <id> already exists." — the "Please retry with another name." sentence is missing.
Why: the response no longer tells the caller what to do about the collision; a client matching AWS's full sentence for the idempotency-collision case does not match.
Fix: `create_user_group.rs`'s already-exists refusal must render the full AWS sentence (id from the request plus the remediation clause).
Traces: elasticache.user-group-modify-membership.

## Evidence
Cause 1 — elasticache.delete-default-parameter-group-forbidden:
  call: elasticache DeleteCacheParameterGroup(CacheParameterGroupName="default.redis7")
  expected: HTTP 400, Error.Code `InvalidParameterValue`, message 'The parameter CacheParameterGroupName is not a valid identifier. Identifiers must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens.'
  actual:   HTTP 400, Error.Code `InvalidParameterValue`, message 'Invalid value default.redis7 for CacheParameterGroupName: the default cache parameter groups in your account cannot be deleted.'
  replay assertion: "AssertionError: InvalidParameterValueException: expected message 'The parameter CacheParameterGroupName is not a valid identifier. …', got 'Invalid value default.redis7 for CacheParameterGroupName: the default cache parameter groups in your account cannot be deleted.'" (check line 95)
  general case (same cause, wrong status): any non-identifier name reaches the store lookup first — services/service-com-amazonaws-elasticache-amazonelasticachev9/src/ops/delete_cache_parameter_group.rs lines 182-191 answer `CacheParameterGroupNotFoundFault` (404) and lines 197-209 the default-group `InvalidParameterValueException`; no call to the identifier validator precedes them. The validator and AWS's exact sentence already exist in services/service-com-amazonaws-elasticache-amazonelasticachev9/src/state.rs lines 2452-2459.

Cause 2 — elasticache.modify-and-reset-cache-parameter-group:
  call: elasticache ModifyCacheParameterGroup(CacheParameterGroupName=<custom group>, ParameterNameValues=[{ParameterName: "activerehashing", ParameterValue: <any>}])
  expected: HTTP 400, Error.Code `InvalidParameterValue`, message 'The parameter activerehashing cannot be modified.'
  actual:   HTTP 400, Error.Code `InvalidParameterValue`, message 'Parameter activerehashing is not modifiable.'
  replay assertion: "AssertionError: expected message 'The parameter activerehashing cannot be modified.', got 'Parameter activerehashing is not modifiable.'" (check line 114)
  source: services/service-com-amazonaws-elasticache-amazonelasticachev9/src/state.rs line 2144 (`check_parameter_value`).

Cause 3 — elasticache.tag-key-reserved-prefix-and-length-rejected:
  call: elasticache AddTagsToResource(ResourceName=<parametergroup ARN>, Tags=[{Key: "aws:<something>", Value: <any>}])
  expected: HTTP 400, Error.Code `InvalidParameterValue`, message 'Tag keys cannot start with the reserved prefix "aws:".'
  actual:   HTTP 400, Error.Code `InvalidParameterValue`, message "Invalid value aws:sim2real-forged for Tags: a tag's Key may not start with the reserved prefix aws:."
  replay assertion: "AssertionError: tag key with the reserved 'aws:' prefix: expected message 'Tag keys cannot start with the reserved prefix \"aws:\".', got \"Invalid value aws:sim2real-forged for Tags: a tag's Key may not start with the reserved prefix aws:.\"" (check line 163)
  source: services/service-com-amazonaws-elasticache-amazonelasticachev9/src/state.rs lines 528-531.

Cause 4 — elasticache.tag-unicode-value-round-trip:
  call: elasticache AddTagsToResource(ResourceName=<ARN>, Tags=[{Key: <legal>, Value: <contains a character outside the published set>}])
  expected: HTTP 400 `InvalidParameterValue`, message 'Tag values may only contain unicode letters, digits, whitespace, or one of these symbols: _ . : / = + \\ - @"'
  actual:   HTTP 400 `InvalidParameterValue`, message 'Tag values may only contain unicode letters, digits, whitespace, or one of these symbols: _ . : / = + \ - @'
  replay assertion: check line 59 ("expected message … got …")
  source: services/service-com-amazonaws-elasticache-amazonelasticachev9/src/state.rs lines 498-503 (`tag_charset_message`) and the TAG_SYMBOLS comment at lines 476-483.

Cause 5 — elasticache.tags-add-list-remove:
  setup (succeeds): CreateCacheParameterGroup + AddTagsToResource + ListTagsForResource round-trip
  call: elasticache RemoveTagsFromResource(ResourceName=<parametergroup ARN>, TagKeys=["never-existed"])
  expected: HTTP 404, Error.Code `TagNotFound` (modeled `TagNotFoundFault`), message 'Failing request. These tags are not present : never-existed'
  actual:   HTTP 404, Error.Code `TagNotFound`, message 'The tag never-existed was not found on resource arn:aws:elasticache:us-west-2:000000000000:parametergroup:sim2real-behav-ecpg-tags-<suffix>.'
  replay assertion: "AssertionError: TagNotFoundFault: expected message 'Failing request. These tags are not present : never-existed', got 'The tag never-existed was not found on resource …'" (check line 84)

Cause 6 — elasticache.user-group-modify-membership:
  setup (succeeds): CreateUserGroup(UserGroupId=<id>, Engine=redis, UserIds=["default"]) -> 200
  call: the same CreateUserGroup again
  expected: HTTP 400, Error.Code `UserGroupAlreadyExists`, message 'User group <id> already exists. Please retry with another name.'
  actual:   HTTP 400, Error.Code `UserGroupAlreadyExists`, message 'User group <id> already exists.'
  replay assertion: "AssertionError: UserGroupAlreadyExists: expected message 'User group sim2real-behav-ecug-mod-<suffix> already exists. Please retry with another name.', got 'User group sim2real-behav-ecug-mod-<suffix> already exists.'" (check line 110)

## Steps to Reproduce
Point an elasticache client at the emulator (Query surface, no VPC/EC2 resources needed for any step).

Cause 1 (DeleteCacheParameterGroup identifier validation):
1. aws elasticache delete-cache-parameter-group --cache-parameter-group-name default.redis7
   -> observe 400 InvalidParameterValue with the "default cache parameter groups … cannot be deleted" text; AWS answers 400 InvalidParameterValue with the "is not a valid identifier" text.
2. aws elasticache delete-cache-parameter-group --cache-parameter-group-name 9x--bad-
   -> observe 404 CacheParameterGroupNotFound; AWS answers 400 InvalidParameterValue ("is not a valid identifier …").
Replay: elasticache.delete-default-parameter-group-forbidden

Cause 2 (non-modifiable parameter):
1. aws elasticache create-cache-parameter-group --cache-parameter-group-name <g> --cache-parameter-group-family redis7 --description x
2. aws elasticache modify-cache-parameter-group --cache-parameter-group-name <g> --parameter-name-values ParameterName=activerehashing,ParameterValue=no
   -> observe 400 InvalidParameterValue "Parameter activerehashing is not modifiable."; AWS: "The parameter activerehashing cannot be modified."
Replay: elasticache.modify-and-reset-cache-parameter-group

Cause 3 (reserved tag prefix):
1. aws elasticache add-tags-to-resource --resource-name <parametergroup ARN> --tags Key=aws:forged,Value=1
   -> observe 400 InvalidParameterValue "Invalid value aws:forged for Tags: a tag's Key may not start with the reserved prefix aws:."; AWS: 'Tag keys cannot start with the reserved prefix "aws:".'
Replay: elasticache.tag-key-reserved-prefix-and-length-rejected

Cause 4 (tag value charset):
1. aws elasticache add-tags-to-resource --resource-name <ARN> --tags Key=ok,Value='bad%value'
   -> observe the normalized symbol-list sentence; AWS's sentence carries a doubled backslash and a trailing double-quote.
2. Then aws elasticache add-tags-to-resource with emoji/CJK/Cyrillic values and aws elasticache list-tags-for-resource to confirm the unicode round trip this check guards.
Replay: elasticache.tag-unicode-value-round-trip

Cause 5 (RemoveTagsFromResource TagNotFound):
1. aws elasticache remove-tags-from-resource --resource-name <parametergroup ARN> --tag-keys never-existed
   -> observe 404 TagNotFound "The tag never-existed was not found on resource <ARN>."; AWS: 'Failing request. These tags are not present : never-existed'
Replay: elasticache.tags-add-list-remove

Cause 6 (CreateUserGroup collision):
1. aws elasticache create-user-group --user-group-id <g> --engine redis --user-ids default
2. repeat the same command
   -> observe 400 UserGroupAlreadyExists "User group <g> already exists."; AWS appends " Please retry with another name."
Replay: elasticache.user-group-modify-membership
