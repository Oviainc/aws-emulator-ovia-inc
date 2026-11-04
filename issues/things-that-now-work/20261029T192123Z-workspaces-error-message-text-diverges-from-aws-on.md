Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: workspaces.connection-alias-create-delete-lifecycle workspaces.ip-group-duplicate-name-and-missing-group-rejected
Issue-Shape-Id: com.amazonaws.workspaces#WorkspacesService
Issue-Key: workspaces.connection-alias-create-delete-lifecycle|run#15|AssertionError|assert err.get("Message") == expected, f"unexpected message: {err.get('Message')!r}"
Issue-Key: workspaces.ip-group-duplicate-name-and-missing-group-rejected|run#9|AssertionError|assert err.get("Message") == expected, f"unexpected message: {err.get('Message')!r}"
Issue-Commit: 8d5a4f803a514b21e009b483e221bcf8b3c8087d

# WorkSpaces error-message text diverges from AWS on two refusals: DeleteConnectionAlias ResourceNotFoundException and CreateIpGroup ResourceAlreadyExistsException

> _Note: this issue was identified against the code base at commit `8d5a4f803a514b21e009b483e221bcf8b3c8087d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteConnectionAlias

## Description
Two net-new root causes in the WorkSpaces emulator. Both are refusals where the HTTP status (400) and the error CODE are correct, but the error MESSAGE the emulator serves is not the message AWS words for that error class. The message is part of the contract clients see, so each is a fidelity defect in its own handler. Neither is covered by any open issue (the open workspaces issues cover CreateWorkspaces FailedRequests entries and the separate workspaces-instances service; the two CreateTags / ec2 CreateInternetGateway stub blockages in this suite are ALREADY FILED and are not part of this issue).

Cause 1 of 2: DeleteConnectionAlias on an alias that does not exist (including one already deleted) returns ResourceNotFoundException with the shape-doc sentence instead of AWS's connection-alias message.
  AWS: Smithy (services/service-com-amazonaws-workspaces-workspacesservice/smithy-model/com-amazonaws-workspaces-workspacesservice.json) models DeleteConnectionAlias with input AliasId (ConnectionAliasId, @required, @length 13..68, @pattern ^wsca-[0-9a-z]{8,63}$) and error ResourceNotFoundException (client, HTTP 400). The model's doc string for that SHAPE is the generic "The resource could not be found." — it is the shape's documentation, not the per-operation message. Recorded behavior: AWS refuses an unknown/deleted AliasId with code ResourceNotFoundException, HTTP 400, and the connection-alias-specific message "The connection alias couldn\u2019t be found. Make sure that you have the correct connection alias, and then try again." That whole string is FIXED text for this error class — it interpolates no request or state value (note the typographic apostrophe U+2019 in "couldn\u2019t", not an ASCII "'"). The alias the caller named is carried in the shape's separate ResourceId member, not in the message.
  Emulator: serves code ResourceNotFoundException with message "The resource could not be found." — the shape's doc sentence, re-used verbatim as the operation's message.
  Why: a client (or an operator reading a log/console) matching or displaying the documented WorkSpaces connection-alias message sees a generic resource-not-found sentence instead, so message-based branching and user-facing diagnostics that work against AWS do not work here; the message gives no hint that the missing thing was a connection alias.
  Fix: the DeleteConnectionAlias handler's not-found refusal path — the message it constructs for ResourceNotFoundException. ResourceId should keep carrying the requested AliasId.
  Traces: workspaces.connection-alias-create-delete-lifecycle. The check creates an alias, polls it to CREATED, deletes it, confirms it is gone from DescribeConnectionAliases, then asserts the second DeleteConnectionAlias refuses with ResourceNotFoundException and that message; the code matched, only the message diverged. Handler and line: services/service-com-amazonaws-workspaces-workspacesservice/src/ops/delete_connection_alias.rs:185-191.

Cause 2 of 2: CreateIpGroup with a GroupName already held by another group returns ResourceAlreadyExistsException with an invented message in the wrong form.
  AWS: Smithy models CreateIpGroup with GroupName (IpGroupName, @required), output GroupId (IpGroupId, @pattern ^wsipg-[0-9a-z]{8,63}$) and error ResourceAlreadyExistsException (client, HTTP 400, shape doc "The specified resource already exists."). Recorded behavior: AWS refuses a duplicate group name with HTTP 400, code ResourceAlreadyExistsException, and the message "Encountered an exception while creating IP Group - group already exists for given group name - <GroupName>". Fixed text is everything up to and including the final " - "; the single variable part is <GroupName>, taken verbatim from the request. Notably the message names the requested NAME and does NOT name the conflicting group's id.
  Emulator: serves message "The IP access control group <existing GroupId> is already named <GroupName>." — different fixed wording, a trailing period AWS does not emit, and it leaks the id of the pre-existing group (a per-run generated value) where AWS names only the requested name.
  Why: a client that recognizes the duplicate-name condition by message, or surfaces it to a user, gets wholly different text; and because the emulator's text embeds a generated group id, any consumer keying on the message also sees a value that changes per run and that AWS never puts there.
  Fix: the CreateIpGroup handler's name-conflict branch — the message built for the ResourceAlreadyExistsException it returns from the state layer's NameTaken refusal.
  Traces: workspaces.ip-group-duplicate-name-and-missing-group-rejected. Handler and line: services/service-com-amazonaws-workspaces-workspacesservice/src/ops/create_ip_group.rs:201-211 (state refusal crate::state::IpGroupCreateError::NameTaken).
  Note for the fixer: this check aborts at the first message assertion, so the asserts after it were never exercised. They are still part of the check and must hold when it is re-replayed: DeleteIpGroup on a well-formed but nonexistent GroupId (and on an already-deleted one) must refuse with code ResourceNotFoundException and the message "Ip Group Id is not found." (entirely fixed text, no interpolation), must leave the real group untouched, and the name must become reusable after the holding group is deleted.

## Evidence
Suite: workspaces 6/10 (60%), 4 failed; 2 of the 4 are ALREADY FILED stub blockages (workspaces CreateTags, ec2 CreateInternetGateway) and are excluded here.

Cause 1 — [mismatch] workspaces.connection-alias-create-delete-lifecycle
  Expected (AWS): code=ResourceNotFoundException, HTTP 400, Message="The connection alias couldn\u2019t be found. Make sure that you have the correct connection alias, and then try again."
  Actual (emulator): code=ResourceNotFoundException (correct), Message='The resource could not be found.'
  Assertion: check_workspaces_connection_alias_create_delete_lifecycle.py:55 — AssertionError: unexpected message: 'The resource could not be found.'
  Source of the emulator string: services/service-com-amazonaws-workspaces-workspacesservice/src/ops/delete_connection_alias.rs:187 sets message to the ResourceNotFoundException shape's doc sentence.

Cause 2 — [mismatch] workspaces.ip-group-duplicate-name-and-missing-group-rejected
  Expected (AWS): code=ResourceAlreadyExistsException, HTTP 400, Message="Encountered an exception while creating IP Group - group already exists for given group name - <GroupName>" (fixed prefix + requested GroupName)
  Actual (emulator): code=ResourceAlreadyExistsException (correct), Message='The IP access control group wsipg-00000001 is already named sim2real-wsipg-dup-d8e06c6786e9.' — the wsipg- id and the sim2real-... name are that run's generated values filling the emulator's own template; the template itself is what diverges.
  Assertion: check_workspaces_ip_group_duplicate_name_and_missing_group_rejected.py:31 — AssertionError: unexpected message.
  Source of the emulator string: services/service-com-amazonaws-workspaces-workspacesservice/src/ops/create_ip_group.rs:204-207.

## Steps to Reproduce
Cause 1:
1. workspaces CreateConnectionAlias(ConnectionString="<unique>.example.com") -> AliasId.
2. Poll DescribeConnectionAliases(AliasIds=[AliasId]) until State == CREATED.
3. DeleteConnectionAlias(AliasId=AliasId); poll DescribeConnectionAliases(AliasIds=[AliasId]) until it returns 0 aliases.
4. Call DeleteConnectionAlias(AliasId=AliasId) again. Observe HTTP 400 / ResourceNotFoundException with Message 'The resource could not be found.'; AWS returns "The connection alias couldn\u2019t be found. Make sure that you have the correct connection alias, and then try again."

Cause 2:
1. workspaces CreateIpGroup(GroupName="<unique-name>", GroupDesc="...") -> GroupId.
2. Call CreateIpGroup(GroupName="<same unique-name>", GroupDesc="duplicate"). Observe HTTP 400 / ResourceAlreadyExistsException with Message 'The IP access control group <GroupId> is already named <name>.'; AWS returns "Encountered an exception while creating IP Group - group already exists for given group name - <name>".
3. (Not yet reached by the check) DeleteIpGroup(GroupId="wsipg-00000000") must refuse with ResourceNotFoundException / "Ip Group Id is not found.", leave the real group present, and the name must be reusable after DeleteIpGroup(GroupId=<GroupId>).

Replay: run_behavioral_suite and re-run the two checks in trace_ids.
