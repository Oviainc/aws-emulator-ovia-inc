Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53-recovery-control-config.create-cluster-invalid-name-pattern-rejected
Issue-Shape-Id: com.amazonaws.route53recoverycontrolconfig#Route53RecoveryControlConfig
Issue-Key: route53-recovery-control-config.create-cluster-invalid-name-pattern-rejected|run#6|AssertionError|assert message == expected_message, f"expected {expected_message!r}, got {message!r}"
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# CreateCluster BadRequestException message echoes the Smithy model pattern "^\\S+$" instead of the regex Route 53 ARC actually reports ("^((?![;'\\s<>&\"])[\\u0021-\\u007E])+$")

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Cause 1 of 1: the pattern-constraint rejection message for CreateCluster's ClusterName quotes the wrong ECMA-262 regex.

AWS: a CreateCluster whose ClusterName contains whitespace is refused by the Route 53 Application Recovery Controller control API with HTTP 400 and error code BadRequestException (empirically BadRequestException, not the ValidationException the model lists for the operation — Smithy: CreateCluster.ClusterName -> __stringMin1Max64PatternS, @length 1..64, @pattern "^\\S+$", required; modeled errors AccessDeniedException/ConflictException/InternalServerException/ResourceNotFoundException/ServiceQuotaExceededException/ThrottlingException/ValidationException — BadRequestException is recorded behavior only). The message AWS returns is a fixed template:

    Invalid request body : [ECMA 262 regex "^((?![;'\\s<>&\"])[\\u0021-\\u007E])+$" does not match input string "<ClusterName>"]

Fixed text: everything outside the second pair of quotes, including the regex literal `^((?![;'\\s<>&\"])[\\u0021-\\u007E])+$` — the service reports its own internal validation regex here, which is NARROWER than and textually different from the model's published `^\\S+$`. Variable part: only the input string, echoed verbatim from the request's ClusterName member. No cluster is created for the rejected request, and a whitespace-free name still succeeds (both of those the emulator already gets right).

Emulator: the code (BadRequestException) and status are right and the effect is absent as expected, but the message substitutes the Smithy pattern into the template:

    got:      Invalid request body : [ECMA 262 regex "^\\S+$" does not match input string "<ClusterName>"]
    expected: Invalid request body : [ECMA 262 regex "^((?![;'\\s<>&\"])[\\u0021-\\u007E])+$" does not match input string "<ClusterName>"]

i.e. the generic constraint validator renders whatever @pattern the model carries for the member, rather than the regex this service reports for string-member pattern violations.

Why: a client that parses or asserts on the rejection message — a test fixture, a user-facing form that surfaces the allowed character class, or a retry/repair path that derives the legal alphabet from the reported regex — is told that any non-whitespace string is acceptable. AWS's reported class is strictly narrower (printable ASCII U+0021..U+007E excluding `;`, `'`, `<`, `>`, `&`, `"`), so a client repairing a name against the emulator's advertised regex can produce a name real AWS still refuses, and message-equality assertions against recorded AWS behavior fail.

Fix: belongs in the pattern-constraint rejection path of this service's request validation (the centralized deserialize -> validate step that formats `Invalid request body : [ECMA 262 regex ... does not match input string ...]`), not in the CreateCluster handler body, which never reaches this branch. The emitted regex literal must be the one the service reports for a string-member pattern violation, independent of the member's modeled @pattern.

Traces: route53-recovery-control-config.create-cluster-invalid-name-pattern-rejected (assertion at check line 26: message equality; the preceding assertion that the code is BadRequestException passed).

## Evidence
Check route53-recovery-control-config.create-cluster-invalid-name-pattern-rejected, verdict mismatch:

AssertionError: expected 'Invalid request body : [ECMA 262 regex "^((?![;\'\\s<>&")[\\u0021-\\u007E])+$" does not match input string "sim2real-r53arc-bad name-320f8d93e67d"]', got 'Invalid request body : [ECMA 262 regex "^\\S+$" does not match input string "sim2real-r53arc-bad name-320f8d93e67d"]'

(The name shown is the per-run unique name the check generated; it is the request's ClusterName echoed back, and only the regex literal diverges.)

Expected (AWS): HTTP 400, Error.Code = BadRequestException, Error.Message = 'Invalid request body : [ECMA 262 regex "^((?![;\'\\s<>&")[\\u0021-\\u007E])+$" does not match input string "<ClusterName>"]'
Actual (emulator): HTTP 400, Error.Code = BadRequestException, Error.Message = 'Invalid request body : [ECMA 262 regex "^\\S+$" does not match input string "<ClusterName>"]'

Smithy (services/service-com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig/smithy-model/com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig.json): CreateCluster input member ClusterName targets __stringMin1Max64PatternS with length 1..64 and pattern "^\\S+$"; the emulator is rendering that model pattern into the message.

## Steps to Reproduce
1. Build and boot the emulator, point a botocore client at route53-recovery-control-config.
2. Call CreateCluster with a ClusterName containing a space, e.g. ClusterName="my cluster-<suffix>" (1..64 chars so only the pattern constraint is violated).
3. Catch the ClientError. Error.Code is BadRequestException (correct) but Error.Message reads 'Invalid request body : [ECMA 262 regex "^\\S+$" does not match input string "my cluster-<suffix>"]'.
4. AWS returns the same code with the regex literal '^((?![;\'\\s<>&")[\\u0021-\\u007E])+$' in place of '^\\S+$'.
5. Confirm the rest of the contract still holds: ListClusters contains no cluster with that name, and a whitespace-free name creates a cluster that DescribeCluster reports under that name.
