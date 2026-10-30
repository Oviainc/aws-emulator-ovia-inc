Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appstream.create-duplicate-stack-name-rejected appstream.delete-nonexistent-fleet-not-found appstream.delete-fleet-associated-with-stack-rejected appstream.delete-stack-with-associated-fleet-rejected
Issue-Shape-Id: com.amazonaws.appstream#PhotonAdminProxyService
Issue-Key: appstream.create-duplicate-stack-name-rejected|run#8|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: appstream.delete-nonexistent-fleet-not-found|run#7|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: appstream.delete-fleet-associated-with-stack-rejected|run#16|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: appstream.delete-stack-with-associated-fleet-rejected|run#19|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: a2b95246df63384d1b9b9d3ee28f05a727c934df

# appstream refusal messages are emulator-authored: four refusals carry the wrong sentence (4 causes)

> _Note: this issue was identified against the code base at commit `a2b95246df63384d1b9b9d3ee28f05a727c934df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteFleet

## Description
Four implemented AppStream control-plane refusals pick the RIGHT modeled error shape and status but compose their own message text instead of AWS's sentence. The error code and HTTP status are a requirement and they are correct; the message is also a requirement and every one of these four diverges. In each cause below, the fixed text is AWS's and the italic parts are values taken from the request and from the emulator's state (resource name, account id) — never from the one recorded run.

Cause 1 of 4: CreateStack with a Name that already exists.
AWS: Smithy: CreateStack models ResourceAlreadyExistsException (httpError 400) — "The specified resource already exists." Recorded behavior: 400 ResourceAlreadyExistsException with message `The stack <Name> for account <accountId>  already exists. Use a unique name within an AWS region in your AWS account.` Fixed text: "The stack ", " for account ", "  already exists. Use a unique name within an AWS region in your AWS account." (AWS emits TWO spaces between the account id and "already exists"). Variable parts: <Name> from CreateStack.Name, <accountId> the caller's 12-digit account.
Emulator: 400 ResourceAlreadyExistsException with message `Stack already exists: <Name>` — AWS's sentence is not produced and the account id is omitted entirely.
Why: a client (or a human reading a log) that matches on the duplicate-name sentence, or that parses the account id out of it to report which account owns the colliding stack, sees text no AWS region ever returns.
Fix: the CreateStack handler's duplicate-name branch, where it builds the ResourceAlreadyExistsException message.
Traces: appstream.create-duplicate-stack-name-rejected.

Cause 2 of 4: DeleteFleet for a fleet name that was never created.
AWS: Smithy: DeleteFleet models ResourceNotFoundException (httpError 404) — "The specified resource was not found." Recorded behavior: message `The fleet <Name> for account <accountId> does not exist.` Fixed text: "The fleet ", " for account ", " does not exist." Variable parts: <Name> from DeleteFleet.Name, <accountId> the caller's account.
Emulator: `Fleet not found: <Name>` — a handler-authored sentence that drops the account id.
Why: not-found text is what callers surface to operators; the emulator's phrasing cannot be matched by code written against AWS, and omitting the account hides the cross-account case the AWS sentence is explicit about.
Fix: the DeleteFleet handler's unknown-name branch (its ResourceNotFoundException message).
Traces: appstream.delete-nonexistent-fleet-not-found.

Cause 3 of 4: DeleteFleet on a fleet still associated with a stack.
AWS: Smithy: DeleteFleet models ResourceInUseException (httpError 400) — "The specified resource is in use." Recorded behavior: message `The fleet <Name> is associated with a stack. To delete the fleet, you must dissociate it from the stack.` Fixed text is that whole sentence pair; the only variable part is <Name>. AWS does NOT enumerate the associated stacks in this message.
Emulator: `Fleet <Name> is still associated with stack(s): <stack list>` — different sentence, and it leaks a joined list of stack names AWS never includes.
Why: the remediation wording AWS gives ("you must dissociate it from the stack") is what clients show and what tests assert; a different sentence plus an extra, unmodeled stack enumeration diverges in both text and shape of information.
Fix: the DeleteFleet handler's association guard, where it formats the ResourceInUseException message (services/service-com-amazonaws-appstream-photonadminproxyservice/src/ops/delete_fleet.rs lines 170-182; the not-found message of cause 2 is at line 143 of the same file).
Traces: appstream.delete-fleet-associated-with-stack-rejected.

Cause 4 of 4: DeleteStack on a stack that still has an associated fleet.
AWS: Smithy: DeleteStack models ResourceInUseException (httpError 400). Recorded behavior: message `Cannot delete stack <Name>. This stack has fleets associated with it. Only stacks with no fleets associated with it can be deleted.` Fixed text: "Cannot delete stack ", ". This stack has fleets associated with it. Only stacks with no fleets associated with it can be deleted." Variable part: <Name> from DeleteStack.Name. AWS does not list the fleets.
Emulator: `Stack <Name> still has associated fleet(s): <fleet list>` — handler-authored sentence that again enumerates the associated resources.
Why: same as cause 3 — the client-visible explanation and remediation differ from AWS's, so message-matching logic and operator runbooks written against AWS do not fire.
Fix: the DeleteStack handler's association guard and its ResourceInUseException message.
Traces: appstream.delete-stack-with-associated-fleet-rejected.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator (appstream 1/24 passed):

1) appstream.create-duplicate-stack-name-rejected [mismatch]
   AssertionError: expected message 'The stack <stack-name> for account <accountId>  already exists. Use a unique name within an AWS region in your AWS account.', got 'Stack already exists: <stack-name>'
   (error code ResourceAlreadyExistsException matched; only the message diverged)

2) appstream.delete-nonexistent-fleet-not-found [mismatch]
   AssertionError: expected message 'The fleet <fleet-name> for account <accountId> does not exist.', got 'Fleet not found: <fleet-name>'

3) appstream.delete-fleet-associated-with-stack-rejected [mismatch]
   AssertionError: expected message 'The fleet <fleet-name> is associated with a stack. To delete the fleet, you must dissociate it from the stack.', got 'Fleet <fleet-name> is still associated with stack(s): <stack-name>'

4) appstream.delete-stack-with-associated-fleet-rejected [mismatch]
   AssertionError: expected message 'Cannot delete stack <stack-name>. This stack has fleets associated with it. Only stacks with no fleets associated with it can be deleted.', got 'Stack <stack-name> still has associated fleet(s): <fleet-name>'

Source of the emulator text for causes 2 and 3: services/service-com-amazonaws-appstream-photonadminproxyservice/src/ops/delete_fleet.rs, `format!("Fleet not found: {}", input.name)` (line 143) and `format!("Fleet {} is still associated with stack(s): {}", ...)` (lines 174-178).

Names, ids and account numbers above are placeholders for per-run values; the requirement is the fixed text plus interpolation of the request's resource name and the caller's account id.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. CreateStack(Name=S); CreateStack(Name=S) again -> inspect Error.Message of the ResourceAlreadyExistsException.
3. DeleteFleet(Name=<never created>) -> inspect Error.Message of the ResourceNotFoundException.
4. CreateFleet(Name=F); CreateStack(Name=S2); AssociateFleet(FleetName=F, StackName=S2); DeleteFleet(Name=F) -> inspect Error.Message of the ResourceInUseException.
5. With the same association in place, DeleteStack(Name=S2) -> inspect Error.Message of the ResourceInUseException.
6. Or replay the four checks in trace_ids.
