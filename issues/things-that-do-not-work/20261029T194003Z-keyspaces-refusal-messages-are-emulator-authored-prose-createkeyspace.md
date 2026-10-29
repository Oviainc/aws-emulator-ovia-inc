Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: keyspaces.create-keyspace-duplicate-name-rejected keyspaces.get-keyspace-not-found keyspaces.delete-keyspace-cascades-and-rejects-redelete keyspaces.delete-type-referenced-by-column-rejected
Issue-Shape-Id: com.amazonaws.keyspaces#KeyspacesService
Issue-Key: keyspaces.create-keyspace-duplicate-name-rejected|run#10|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: keyspaces.get-keyspace-not-found|run#12|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: keyspaces.delete-keyspace-cascades-and-rejects-redelete|run#22|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: keyspaces.delete-type-referenced-by-column-rejected|run#17|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: baf896d7efe27c876c690dc5952f8e38980dff3b

# keyspaces refusal messages are emulator-authored prose: CreateKeyspace conflict, GetKeyspace/DeleteKeyspace not-found and DeleteType conflict all diverge from AWS wording (4 causes)

> _Note: this issue was identified against the code base at commit `baf896d7efe27c876c690dc5952f8e38980dff3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateKeyspace

## Description
This issue bundles 4 net-new root causes in the keyspaces crate. Every one is the same KIND of defect — the emulator invents its own refusal sentence instead of the sentence Amazon Keyspaces returns — but each is a DISTINCT message template produced at a distinct site, so each must be fixed separately. The error CODE and HTTP status are correct in all four; only the `message` member of the error payload diverges. Four causes, all 4 must be closed before submitting.

Cause 1 of 4: CreateKeyspace duplicate keyspace name — ConflictException message.
AWS: Smithy (services/service-com-amazonaws-keyspaces-keyspacesservice/smithy-model/com-amazonaws-keyspaces-keyspacesservice.json) declares CreateKeyspace error ConflictException, errorKind client, httpError 409, doc 'Amazon Keyspaces couldn't complete the requested action.' The model does not state the sentence; recorded behavior only. The recorded refusal for a keyspace name already taken in the Region is 409 ConflictException with message `Cannot add existing keyspace <keyspaceName>`. Fixed text: `Cannot add existing keyspace ` — variable part: the keyspaceName exactly as supplied in the request. No quoting of the name, no Region clause, no trailing period.
Emulator: answers 409 ConflictException with `Keyspace '<keyspaceName>' already exists in Region '<region>'.` — a different sentence, the name single-quoted, an extra Region clause taken from the SigV4 credential scope, and a trailing period.
Why: a client that matches on the refusal sentence (log/alert rules, operator runbooks, or code that distinguishes 'name taken' from other conflicts because ConflictException covers several conditions on this operation) does not match. The emulator additionally leaks the request's Region into a message AWS does not scope by Region.
Fix: the CreateKeyspace handler's duplicate-name refusal — the ConflictException it constructs for the name-already-held branch.
Traces: keyspaces.create-keyspace-duplicate-name-rejected (services/service-com-amazonaws-keyspaces-keyspacesservice/src/ops/create_keyspace.rs:211-219).

Cause 2 of 4: GetKeyspace on a keyspace that does not exist — ResourceNotFoundException message.
AWS: Smithy declares GetKeyspace error ResourceNotFoundException, errorKind client, httpError 404, doc 'The operation tried to access a keyspace, table, or type that doesn't exist.' The model does not state the sentence; recorded behavior only. The recorded refusal is 404 ResourceNotFoundException with message `Keyspace <keyspaceName> does not exist`. Fixed text: `Keyspace ` + ` does not exist` — variable part: the keyspaceName from the request, unquoted. No trailing period.
Emulator: answers 404 ResourceNotFoundException with `Keyspace '<keyspaceName>' was not found.` — different fixed text, the name single-quoted, trailing period.
Why: a caller that keys an absent-keyspace branch off the message text (or asserts it in tests/golden files) misreads the emulator's reply, and the two not-found sentences the service words differently (Cause 3) are collapsed into one here, so a caller cannot tell a failed read from a failed drop.
Fix: the shared not-found payload builder for keyspaces used by the GetKeyspace handler — it must render the read-path sentence rather than one generic template for every operation.
Traces: keyspaces.get-keyspace-not-found (message built by `keyspace_not_found` at services/service-com-amazonaws-keyspaces-keyspacesservice/src/state.rs:2152-2157, called from src/ops/get_keyspace.rs:185-190).

Cause 3 of 4: DeleteKeyspace of an already-gone keyspace — ResourceNotFoundException message.
AWS: Smithy declares DeleteKeyspace error ResourceNotFoundException, errorKind client, httpError 404. The model does not state the sentence; recorded behavior only. The recorded refusal for re-deleting a keyspace that no longer exists is 404 ResourceNotFoundException with message `Cannot drop non existing keyspace '<keyspaceName>'`. Fixed text: `Cannot drop non existing keyspace ` with the name wrapped in single quotes — variable part: the keyspaceName from the request. No trailing period. Note this is a DIFFERENT sentence from the read path in Cause 2: the service words the drop path as a failed DDL drop, not as a failed lookup.
Emulator: answers 404 ResourceNotFoundException with the same generic `Keyspace '<keyspaceName>' was not found.` it uses for GetKeyspace, so the delete-path sentence does not exist at all.
Why: the cascade/re-delete boundary is exactly where clients distinguish 'already deleted, safe to proceed' from other failures; the emulator's one-size message means a client written against AWS's drop wording never matches, and operators cannot tell from a log line which operation refused.
Fix: the DeleteKeyspace handler's not-found refusal must carry the drop-path sentence instead of reusing the generic keyspace not-found payload builder.
Traces: keyspaces.delete-keyspace-cascades-and-rejects-redelete (the rest of the check — cascade removal of the keyspace's tables and the keyspace itself — passed; only the re-delete message diverged).

Cause 4 of 4: DeleteType on a type still referenced by a table column — ConflictException message.
AWS: Smithy declares DeleteType error ConflictException, errorKind client, httpError 409. The model does not state the sentence; recorded behavior only. The recorded refusal when the user-defined type is still used by a table or another UDT is 409 ConflictException with message `<typeName> is referenced by other UDTs or tables.` Fixed text: ` is referenced by other UDTs or tables.` — variable part: the formatted type name only. AWS names NEITHER the keyspace nor the specific referring table: the sentence is the same whichever and however many users the type has.
Emulator: answers 409 ConflictException with `Type '<keyspace>.<typeName>' is still used by table '<keyspace>.<table>' and can only be deleted once nothing uses it.` — different fixed text, and it enumerates the keyspace-qualified type plus every referring table and parent type, so the message's shape varies with the number of users where AWS's is constant.
Why: a client matching AWS's sentence never matches, and one parsing the emulator's enumerated user list builds a dependency on text AWS never emits. The enumeration also discloses sibling resource names AWS withholds in this refusal.
Fix: the DeleteType handler's still-referenced refusal — the ConflictException it constructs after deriving referring tables and parent types.
Traces: keyspaces.delete-type-referenced-by-column-rejected (services/service-com-amazonaws-keyspaces-keyspacesservice/src/ops/delete_type.rs:241-260).

## Evidence
Cause 1 — keyspaces.create-keyspace-duplicate-name-rejected:
  expected message 'Cannot add existing keyspace sim2real_behav_ks_85fce0ad6734'
  got      "Keyspace 'sim2real_behav_ks_85fce0ad6734' already exists in Region 'us-west-2'."
  (code ConflictException matched; only the message diverged. The keyspace name shown is this run's generated name — the requirement is the template, not that literal name.)

Cause 2 — keyspaces.get-keyspace-not-found:
  expected message 'Keyspace sim2real_behav_missing_ks_b0b9e1acae0e does not exist'
  got      "Keyspace 'sim2real_behav_missing_ks_b0b9e1acae0e' was not found."
  (code ResourceNotFoundException matched.)

Cause 3 — keyspaces.delete-keyspace-cascades-and-rejects-redelete:
  expected message "Cannot drop non existing keyspace 'sim2real_behav_ks_32bb79ac8a58'"
  got      "Keyspace 'sim2real_behav_ks_32bb79ac8a58' was not found."
  (code ResourceNotFoundException matched; the cascade assertions before it passed.)

Cause 4 — keyspaces.delete-type-referenced-by-column-rejected:
  expected message 'sim2real_behav_type_2bdd9f129875 is referenced by other UDTs or tables.'
  got      "Type 'sim2real_behav_ks_06fff665b089.sim2real_behav_type_2bdd9f129875' is still used by table 'sim2real_behav_ks_06fff665b089.sim2real_behav_tbl_f0c25a050999' and can only be deleted once nothing uses it."
  (code ConflictException matched.)

In all four the HTTP status and error code are right and the side effects asserted by the checks (original keyspace ARN unchanged, no second list entry, cascade completed, type still ACTIVE) hold — the divergence is confined to the message text.

## Steps to Reproduce
Against the emulator's keyspaces endpoint:

Cause 1: CreateKeyspace(keyspaceName=K) once; wait for K to appear in ListKeyspaces; CreateKeyspace(keyspaceName=K) again. Read Error.Code and Error.Message — expect 409 ConflictException / `Cannot add existing keyspace K`.

Cause 2: GetKeyspace(keyspaceName=K2) for a name never created. Expect 404 ResourceNotFoundException / `Keyspace K2 does not exist`.

Cause 3: CreateKeyspace(K3); CreateTable in K3; wait ACTIVE; DeleteKeyspace(K3); poll GetKeyspace/GetTable until both answer ResourceNotFoundException; DeleteKeyspace(K3) again. Expect 404 ResourceNotFoundException / `Cannot drop non existing keyspace 'K3'`.

Cause 4: CreateKeyspace(K4); CreateType(K4, T, fields=[street text]); wait T ACTIVE; CreateTable(K4, TB, allColumns=[pk text, addr frozen<T>], partitionKeys=[pk]); wait TB ACTIVE; DeleteType(K4, T). Expect 409 ConflictException / `T is referenced by other UDTs or tables.`

Each failing check in trace_ids reproduces its cause end to end.
