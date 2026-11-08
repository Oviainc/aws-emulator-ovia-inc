Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: resource-explorer-2.delete-default-view-clears-default-pointer
Issue-Shape-Id: com.amazonaws.resourceexplorer2#ResourceExplorer
Issue-Key: resource-explorer-2.delete-default-view-clears-default-pointer|run#22|AssertionError|assert (reassoc_code, reassoc_message) == expected_reassoc, (
Issue-Commit: 6822a77211f8c6b3cb79ac25b6a931609e4ade77

# AssociateDefaultView refuses a deleted view with the wrong ResourceNotFoundException message

> _Note: this issue was identified against the code base at commit `6822a77211f8c6b3cb79ac25b6a931609e4ade77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateDefaultView

## Description
Cause 1 of 1: AssociateDefaultView on a view ARN that does not exist (here, a view that was just deleted) is correctly refused with ResourceNotFoundException / HTTP 404, but the emulator words the message differently from AWS, so the response body does not match real AWS for this error class. Failing check: resource-explorer-2.delete-default-view-clears-default-pointer.

AWS: Smithy (com.amazonaws.resourceexplorer2): AssociateDefaultView declares ResourceNotFoundException (errorKind=client, httpError=404, member Message: String), documented as "You specified a resource that doesn't exist. Check the ID or ARN that you used to identity the resource, and try again." The model does not pin the message text, so the message is recorded behavior only: AWS refuses AssociateDefaultView on a nonexistent/deleted view ARN with code ResourceNotFoundException, HTTP 404, and message of the form "Views Not Found for provided arn: <ViewArn>". The fixed text is "Views Not Found for provided arn: "; the only variable part is the view ARN, taken verbatim from the request's ViewArn member. The rest of the same recording matches already: after DeleteView of the Region's default view, GetDefaultView returns 200 with no ViewArn key, and GetView on the deleted ARN is refused UnauthorizedException / "Unauthorized".

Emulator: returns code ResourceNotFoundException (correct) with message "View not found: <ViewArn>" — a different fixed prefix and different capitalization/phrasing than AWS. The handler formats the message as `format!("View not found: {}", input.view_arn)` on the views-lookup miss, so every unknown or deleted view ARN gets this non-AWS wording.

Why: a client that branches on or surfaces the error message — error-message assertions in tests, log scrapers, operator-facing UIs, and SDK retry/classification wrappers that pattern-match the message of a 404 — sees text real AWS never emits. Since the code and status are right, the failure is silent until something reads the message, and then it either mis-classifies the condition or reports an error string that does not exist in AWS, making emulator-recorded diagnostics non-portable to real AWS.

Fix: the error-construction path in the AssociateDefaultView handler (its validation of the ViewArn against stored views) must word the ResourceNotFoundException message as AWS does for an unknown view ARN, with the request's ViewArn interpolated. No state or serializer change is required — the lookup, status and error code are already correct.

Traces: resource-explorer-2.delete-default-view-clears-default-pointer (asserts the (code, message) pair for AssociateDefaultView on the deleted view). Handler citation: services/service-com-amazonaws-resourceexplorer2-resourceexplorer/src/ops/associate_default_view.rs lines 165-172.

## Evidence
Check: resource-explorer-2.delete-default-view-clears-default-pointer

Expected (AWS, recorded): ('ResourceNotFoundException', 'Views Not Found for provided arn: <ViewArn>') with HTTP 404 — fixed text 'Views Not Found for provided arn: ', variable part = the ViewArn from the request.

Actual (emulator): ('ResourceNotFoundException', 'View not found: arn:aws:resource-explorer-2:us-west-2:000000000000:view/sim2real-rexdefdel-358bc737b89e/b4ffc683-0115-8e13-b1cc-95c1f485fca1')

AssertionError from the replay:
  assert (reassoc_code, reassoc_message) == expected_reassoc
  AssertionError: expected AssociateDefaultView on the deleted view to fail, got ('ResourceNotFoundException', 'View not found: arn:aws:resource-explorer-2:...')

(The account id, view name and view id above are per-run values that fill the template; only the prefix text is the requirement.)

Source of the divergence — services/service-com-amazonaws-resourceexplorer2-resourceexplorer/src/ops/associate_default_view.rs:165-172:
  let Some(view) = guard.views.get(&input.view_arn) else {
      return Err(AssociateDefaultViewErrorModeled::ResourceNotFoundException(
          ResourceNotFoundException { message: Some(format!("View not found: {}", input.view_arn)) },
      ).into());
  };

## Steps to Reproduce
1. CreateIndex and poll GetIndex until State == ACTIVE.
2. CreateView with any ViewName; capture View.ViewArn.
3. AssociateDefaultView(ViewArn=<that arn>); GetDefaultView returns that ViewArn.
4. DeleteView(ViewArn=<that arn>).
5. GetDefaultView — returns 200 with no ViewArn key (this already matches).
6. GetView(ViewArn=<deleted arn>) — refused UnauthorizedException/'Unauthorized' (this already matches).
7. AssociateDefaultView(ViewArn=<deleted arn>) — observe the error: code is ResourceNotFoundException (correct) but the message is 'View not found: <arn>' instead of AWS's 'Views Not Found for provided arn: <arn>'.
