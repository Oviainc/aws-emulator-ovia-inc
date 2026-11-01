Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: qapps.get-qapp-nonexistent-not-found qapps.delete-qapp-is-idempotent qapps.update-qapp-title-increments-version qapps.start-qapp-session-returns-pending-card-status qapps.create-qapp-invalid-title-pattern-rejected qapps.update-qapp-permissions-grant-list-over-max-rejected qapps.associate-and-disassociate-qapp-with-user-lifecycle qapps.associate-library-item-review-increments-rating qapps.batch-create-category-then-list-categories-reflects qapps.create-library-item-publishes-qapp-status qapps.predict-qapp-missing-input-validation-error qapps.update-qapp-permissions-grant-reflected-in-describe qapps.update-qapp-session-completes-text-input-card qapps.update-qapp-session-metadata-sharing-config-persists qapps.tag-resource-roundtrip-list-and-untag
Issue-Shape-Id: com.amazonaws.qapps#QAppsService
Issue-Key: qapps.get-qapp-nonexistent-not-found|run#4|AssertionError|assert code == "UnauthorizedException"
Issue-Key: qapps.delete-qapp-is-idempotent|run#5|AssertionError|assert code_first == "UnauthorizedException"
Issue-Key: qapps.update-qapp-title-increments-version|run#5|AssertionError|assert code == "UnauthorizedException", (
Issue-Key: qapps.start-qapp-session-returns-pending-card-status|run#5|AssertionError|assert code_start == "UnauthorizedException", (
Issue-Key: qapps.create-qapp-invalid-title-pattern-rejected|run#7|AssertionError|assert code_bad == "UnauthorizedException", (
Issue-Key: qapps.update-qapp-permissions-grant-list-over-max-rejected|run#6|AssertionError|assert code_over_max == "UnauthorizedException", (
Issue-Key: qapps.associate-and-disassociate-qapp-with-user-lifecycle|run#5|AssertionError|assert code_associate == "UnauthorizedException", (
Issue-Key: qapps.associate-library-item-review-increments-rating|run#5|AssertionError|assert code_write == "UnauthorizedException", f"expected UnauthorizedException, got {code_write}"
Issue-Key: qapps.batch-create-category-then-list-categories-reflects|run#6|AssertionError|assert code_write == "UnauthorizedException", (
Issue-Key: qapps.create-library-item-publishes-qapp-status|run#5|AssertionError|assert code_create == "UnauthorizedException", (
Issue-Key: qapps.predict-qapp-missing-input-validation-error|run#4|AssertionError|assert code_missing_input == "UnauthorizedException", (
Issue-Key: qapps.update-qapp-permissions-grant-reflected-in-describe|run#6|AssertionError|assert code_describe == "UnauthorizedException", code_describe
Issue-Key: qapps.update-qapp-session-completes-text-input-card|run#6|AssertionError|assert code_update == "UnauthorizedException", code_update
Issue-Key: qapps.update-qapp-session-metadata-sharing-config-persists|run#6|AssertionError|assert code_update == "UnauthorizedException", code_update
Issue-Key: qapps.tag-resource-roundtrip-list-and-untag|run#8|AssertionError|assert code_list == "AccessDeniedException", code_list
Issue-Commit: 0b023ce22a329ff2896e758c2b0769a212a04448

# Q Apps has no Identity Center front-door authorization: every operation answers 404/400/501 instead of 401 UnauthorizedException, and the tagging surface instead of 403 AccessDeniedException (4 causes)

> _Note: this issue was identified against the code base at commit `0b023ce22a329ff2896e758c2b0769a212a04448`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQApp

## Description
The qapps emulator implements no authorization front door at all. Real Q Apps evaluates an IAM Identity Center federated-user context ahead of ANY other request handling (resource existence, request-shape validation, business logic), and the AWS-standard resource-tagging surface evaluates plain IAM ARN authorization. The emulator skips both and lets requests fall through to state lookup, Smithy validation, or the scaffold stub, so every refusal carries the wrong status and error code. This is one missing gate observed on four distinct emulator paths; all four must be closed.

Cause 1 of 4 — implemented core operations run state lookup and answer ResourceNotFoundException instead of the front-door refusal.
AWS: GetQApp/DeleteQApp/UpdateQApp/StartQAppSession/GetQAppSession all model UnauthorizedException (Smithy: com.amazonaws.qapps#UnauthorizedException, errorKind client, httpError 401, required member `message`; bound as an error on each of these operations). A SigV4/IAM caller with no Identity Center federation is refused with 401 UnauthorizedException regardless of whether the referenced instanceId/appId/sessionId exist — recorded behavior, with message `Unauthorized` as the recorded example.
Emulator: these handlers reach their state and answer ResourceNotFoundException, e.g. UpdateQApp -> `ResourceNotFoundException: Q App 308522c1-... was not found`, StartQAppSession -> ResourceNotFoundException, GetQApp/DeleteQApp -> a non-UnauthorizedException code.
Why: a client cannot distinguish "you are not federated" from "the app does not exist". Retry/provisioning logic keyed on 401-vs-404 takes the wrong branch, and the emulator reports a resource namespace the caller was never authorized to observe.
Fix: an authorization gate in the qapps request pipeline, evaluated before per-operation handlers and before state lookup, in the service crate's dispatch/auth layer (services/service-com-amazonaws-qapps-qappsservice) rather than per handler.
Traces: qapps.get-qapp-nonexistent-not-found, qapps.delete-qapp-is-idempotent, qapps.update-qapp-title-increments-version, qapps.start-qapp-session-returns-pending-card-status.

Cause 2 of 4 — input validation runs ahead of the authorization gate, leaking 400 ValidationException.
AWS: the identity check precedes request-shape validation, so an unfederated caller gets 401 UnauthorizedException even when the payload would independently fail validation (CreateQApp title pattern/length; UpdateQAppPermissions grantPermissions over its 100-entry limit) — recorded behavior. ValidationException (httpError 400) is a service-common error reachable only after the caller is authorized.
Emulator: CreateQApp with a title containing `{}` answers 400 ValidationException; UpdateQAppPermissions with 101 grantPermissions entries answers 400 ValidationException.
Why: an unauthenticated caller can use the emulator as a validation oracle to probe field-level constraints before authenticating — a refusal class real AWS never exposes pre-auth — and clients that branch on ValidationException retry-vs-fail behave differently against the emulator.
Fix: order the authorization gate before Smithy/field validation in the qapps request pipeline (same gate as Cause 1), so validation never executes for an unauthorized caller.
Traces: qapps.create-qapp-invalid-title-pattern-rejected, qapps.update-qapp-permissions-grant-list-over-max-rejected.

Cause 3 of 4 — scaffolded operations answer 501 NotImplemented where the front door would refuse first.
AWS: AssociateQAppWithUser, DisassociateQAppFromUser, ListQApps, BatchCreateCategory, ListCategories, CreateLibraryItem, GetLibraryItem, PredictQApp, DescribeQAppPermissions, UpdateQAppPermissions, UpdateQAppSession, UpdateQAppSessionMetadata and GetQAppSessionMetadata every one model UnauthorizedException (Smithy digest: UnauthorizedException is in the error list of all of them; ListQApps and PredictQApp model it as their ONLY error). An unfederated caller is refused with 401 UnauthorizedException, recorded message `Unauthorized`, regardless of whether the ids are well-formed-but-nonexistent or syntactically malformed.
Emulator: each answers 501 with code NotImplemented and message of the form `operation <Name> is scaffolded but not implemented in this emulator`.
Why: 501 NotImplemented is not a modeled error for any qapps operation; SDK retry classification treats it as a server/transport fault rather than a terminal client refusal, so clients retry a call AWS refuses outright. Even once these handlers are implemented, the refusal must come from the gate before the handler.
Fix: the authorization gate of Cause 1 must run before stub dispatch, so a scaffolded operation still answers 401 UnauthorizedException for an unauthorized caller; the individual handler implementations are separate work (the AssociateLibraryItemReview stub in services/service-com-amazonaws-qapps-qappsservice/src/ops/associate_library_item_review.rs is already filed on its own issue and is not part of this one).
Traces: qapps.associate-and-disassociate-qapp-with-user-lifecycle, qapps.associate-library-item-review-increments-rating, qapps.batch-create-category-then-list-categories-reflects, qapps.create-library-item-publishes-qapp-status, qapps.predict-qapp-missing-input-validation-error, qapps.update-qapp-permissions-grant-reflected-in-describe, qapps.update-qapp-session-completes-text-input-card, qapps.update-qapp-session-metadata-sharing-config-persists.

Cause 4 of 4 — the resource-tagging surface has a DIFFERENT front door (plain IAM) and answers neither it nor anything modeled.
AWS: ListTagsForResource, TagResource and UntagResource deliberately do NOT model UnauthorizedException (Smithy: their only modeled error is ResourceNotFoundException, plus ConflictException on TagResource); they authorize purely through the caller's IAM policy against the target ARN and refuse with 403 AccessDeniedException (Smithy: com.amazonaws.qapps#AccessDeniedException, errorKind client, httpError 403, service-common error). The refusal message is built from the request's caller principal — the rule is `User: <caller-arn> is not authorized to access this resource` — so the ARN varies per caller while the template is the requirement.
Emulator: all three answer 501 NotImplemented.
Why: a client gets neither of qapps' two distinct refusal classes, and cannot tell the IAM-authorized tagging surface apart from the Identity-Center-gated data plane — the contrast that makes qapps' error semantics faithful.
Fix: plain-IAM ARN authorization for the three tagging operations (403 AccessDeniedException with the caller-ARN message template), evaluated separately from the Identity Center gate, in the qapps tagging handlers/auth layer.
Traces: qapps.tag-resource-roundtrip-list-and-untag.

## Evidence
Behavioral suite: qapps 0/16 passed (16 mismatched). One check (qapps.concurrent-library-item-reviews-single-winner) is ALREADY FILED against the AssociateLibraryItemReview stub and is excluded here.

Cause 1 (expected 401 UnauthorizedException / message example "Unauthorized"):
- update_q_app(instanceId=<uuid>, appId=<uuid>, title=...) -> actual: ResourceNotFoundException: "Q App 308522c1-b567-4f91-a596-47b9ccf9899a was not found"
- start_q_app_session(instanceId=<uuid>, appId=<uuid>, appVersion=1) -> actual: ResourceNotFoundException
- get_q_app / delete_q_app on random uuids -> actual code != UnauthorizedException (assertion `code_first == "UnauthorizedException"` failed)

Cause 2:
- create_q_app(title="sim2real-{invalid}-title", appDefinition={cards:[textInput]}) -> expected 401 UnauthorizedException, actual 400 'ValidationException'
- update_q_app_permissions(grantPermissions=[101 entries]) -> expected 401 UnauthorizedException, actual 400 'ValidationException'

Cause 3 (expected 401 UnauthorizedException, actual 501 'NotImplemented'):
- associate_q_app_with_user / list_q_apps / disassociate_q_app_from_user
- batch_create_category / list_categories
- create_library_item, get_library_item
- predict_q_app(instanceId=<uuid>) and predict_q_app(options={problemStatement:...})
- describe_q_app_permissions, update_q_app_permissions
- update_q_app_session, update_q_app_session_metadata, get_q_app_session_metadata
Example body: 'operation AssociateLibraryItemReview is scaffolded but not implemented in this emulator'.

Cause 4 (expected 403 AccessDeniedException, message "User: <sts:GetCallerIdentity Arn> is not authorized to access this resource"):
- list_tags_for_resource(resourceARN=arn:aws:qapps:us-east-1:111122223333:application/<uuid>/qapp/<uuid>) -> actual 501 'NotImplemented'
- tag_resource(...) and untag_resource(...) on the same ARN -> actual 501 'NotImplemented'

## Steps to Reproduce
With plain SigV4/IAM credentials (no IAM Identity Center federation) against the emulator's qapps endpoint, using random UUIDs for instanceId/appId/sessionId:
1. Cause 1: `aws qapps get-q-app --instance-id <uuid> --app-id <uuid>`; also delete-q-app, update-q-app --title X, start-q-app-session --app-version 1. Observe ResourceNotFoundException instead of 401 UnauthorizedException.
2. Cause 2: `aws qapps create-q-app --instance-id <uuid> --title 'sim2real-{invalid}-title' --app-definition '{"cards":[{"textInput":{"id":"<uuid>","type":"text-input","title":"Name"}}]}'` and `aws qapps update-q-app-permissions --instance-id <uuid> --app-id <uuid> --grant-permissions <101 entries>`. Observe 400 ValidationException instead of 401 UnauthorizedException.
3. Cause 3: call associate-q-app-with-user, disassociate-q-app-from-user, list-q-apps, batch-create-category, list-categories, create-library-item, get-library-item, predict-q-app, describe-q-app-permissions, update-q-app-permissions, update-q-app-session, update-q-app-session-metadata, get-q-app-session-metadata. Observe 501 NotImplemented instead of 401 UnauthorizedException.
4. Cause 4: call list-tags-for-resource / tag-resource / untag-resource with resourceARN arn:aws:qapps:us-east-1:111122223333:application/<uuid>/qapp/<uuid>. Observe 501 NotImplemented instead of 403 AccessDeniedException with message "User: <caller arn from sts get-caller-identity> is not authorized to access this resource".
5. Replay the check ids in trace_ids to confirm.
