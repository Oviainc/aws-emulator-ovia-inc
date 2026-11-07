Issue-Source: trace-sweep-divergence
Issue-Trace: keyspaces-keyspace-lifecycle-tags-and-rejections
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: keyspaces-keyspace-lifecycle-tags-and-rejections keyspaces-table-create-invalid-schema-rejected
Issue-Shape-Id: com.amazonaws.keyspaces#KeyspacesService
Issue-Key: keyspaces|CreateTable|4xx->4xx|$.message
Issue-Related: 20261029T202315Z-keyspaces-serves-modeled-client-exceptions-with-409-404.md
Issue-Commit: 221dd3602e5d0e66d71e70da1fb8509b07c948c5

# keyspaces ValidationException messages render the name pattern with regex anchors and order the per-member errors wrongly

> _Note: this issue was identified against the code base at commit `221dd3602e5d0e66d71e70da1fb8509b07c948c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateKeyspace

## Description
This issue bundles 2 net-new root causes in the keyspaces crate's input-validation message builder. Both steps returned the right HTTP 400 and the right ValidationException code; only the aggregated message text diverged. Two causes, both must be closed before submitting.

Cause 1 of 2: CreateKeyspace with an over-long keyspaceName — the two constraint violations are listed in the wrong order and the pattern is printed with its anchors.
AWS: Smithy declares CreateKeyspaceRequest.keyspaceName as @required KeyspaceName with @length min 1 max 48 and @pattern ^[a-zA-Z0-9][a-zA-Z0-9_]{0,47}$; ValidationException is the operation's client error (httpError 400, defaulted). The recorded refusal for a value that breaks both constraints is the standard AWS aggregated validation sentence: `<n> validation errors detected: ` followed by one clause per violation joined with `; `, each clause reading `Value '<value>' at '<memberName>' failed to satisfy constraint: ` plus the constraint phrase — `Member must satisfy regular expression pattern: <pattern>` or `Member must have length less than or equal to <max>`. Two details are requirements and both diverge: (a) the pattern is rendered WITHOUT the model's ^ and $ anchors, i.e. `[a-zA-Z0-9][a-zA-Z0-9_]{0,47}`; (b) for this member AWS emits the pattern clause BEFORE the length clause. Fixed text is the template above; the variable parts are the violation count, the offending value and member name from the request, the pattern, and the limit from the model.
Emulator: returns the same count and the same two clauses but in the reverse order (length first, pattern second) and prints the pattern as `^[a-zA-Z0-9][a-zA-Z0-9_]{0,47}$`, i.e. it echoes the Smithy @pattern string verbatim including the anchors AWS strips.
Why: the validation message is the only place the API states WHICH member and WHICH rule failed; clients, test goldens and operator runbooks match on this text, and a caller that extracts the pattern from the message and compiles or displays it gets a different regex source than AWS publishes. The clause order change also means the first-reported violation — which is what most parsers surface to the user — is the wrong one.
Fix: the shared validation-message builder for the keyspaces crate: strip the leading ^ and trailing $ when rendering a @pattern constraint, and emit the per-member clauses in AWS's order for this member.
Traces: keyspaces-keyspace-lifecycle-tags-and-rejections#task#5.

Cause 2 of 2: CreateTable with an over-long tableName — the pattern is printed with its anchors.
AWS: Smithy declares CreateTableRequest.tableName as @required TableName with @length min 1 max 48 and @pattern ^[a-zA-Z0-9][a-zA-Z0-9_]{0,47}$. The recorded refusal uses the same aggregated template; here the length clause comes first and the pattern clause second (so the per-member clause ORDER differs from Cause 1 and must be taken from the recorded behavior per member, not normalised), and the pattern is again rendered unanchored: `Member must satisfy regular expression pattern: [a-zA-Z0-9][a-zA-Z0-9_]{0,47}`.
Emulator: gets the count, the clauses and their order right here, but still prints `^[a-zA-Z0-9][a-zA-Z0-9_]{0,47}$`.
Why: as Cause 1 — the published pattern in the refusal does not match AWS's, so text matches and any client that reuses the pattern diverge. This is a separate cause from Cause 1 because it is a different operation's validation path and is the one that proves the anchor-stripping fix without the ordering change.
Fix: the same shared pattern-rendering in the keyspaces validation-message builder, exercised from the CreateTable input validation.
Traces: keyspaces-table-create-invalid-schema-rejected#task#2.

## Evidence
Replay of simparity-wire-aws-v1 — both steps returned the expected HTTP 400 and ValidationException; only $.message diverged:

Cause 1 — keyspaces-keyspace-lifecycle-tags-and-rejections#task#5, CreateKeyspace:
  expected "2 validation errors detected: Value '<name>' at 'keyspaceName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9][a-zA-Z0-9_]{0,47}; Value '<name>' at 'keyspaceName' failed to satisfy constraint: Member must have length less than or equal to 48"
  got      "2 validation errors detected: Value '<name>' at 'keyspaceName' failed to satisfy constraint: Member must have length less than or equal to 48; Value '<name>' at 'keyspaceName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9][a-zA-Z0-9_]{0,47}$"
  (the recorded run's <name> was the 67-character value sim2real_wire_ks1_toolong_aaaa... — a per-run value filling the template)

Cause 2 — keyspaces-table-create-invalid-schema-rejected#task#2, CreateTable:
  expected "2 validation errors detected: Value '<name>' at 'tableName' failed to satisfy constraint: Member must have length less than or equal to 48; Value '<name>' at 'tableName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9][a-zA-Z0-9_]{0,47}"
  got      the same sentence but with the pattern printed as ^[a-zA-Z0-9][a-zA-Z0-9_]{0,47}$

The two differences are therefore: the anchors (both causes) and the clause order for keyspaceName (Cause 1 only).

## Steps to Reproduce
Against the emulator's keyspaces endpoint:

Cause 1: CreateKeyspace(keyspaceName=<a 67-character value starting with a letter, e.g. 'a' repeated 67 times>). Expect HTTP 400 ValidationException with message `2 validation errors detected: Value '<value>' at 'keyspaceName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9][a-zA-Z0-9_]{0,47}; Value '<value>' at 'keyspaceName' failed to satisfy constraint: Member must have length less than or equal to 48` — pattern clause first, pattern unanchored.

Cause 2: CreateKeyspace(K) for a valid K, then CreateTable(keyspaceName=K, tableName=<a 67-character value>, schemaDefinition valid). Expect HTTP 400 ValidationException with the length clause first and the pattern clause second, the pattern again unanchored.

Replaying the fixture steps in Issue-Trace/Issue-Step and Issue-Trace-Ids reproduces each cause.
