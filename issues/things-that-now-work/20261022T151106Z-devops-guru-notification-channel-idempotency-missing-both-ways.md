Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: devops-guru.add-notification-channel-duplicate-topic-is-idempotent devops-guru.notification-channel-duplicate-topic-race-exactly-one-wins devops-guru.notification-channel-add-list-remove-lifecycle devops-guru.notification-channel-removal-enables-topic-reuse devops-guru.resource-collection-multiple-tag-filters-coexist devops-guru.service-integration-kms-missing-keyid-rejected devops-guru.describe-insight-cross-tenant-accountid-rejected
Issue-Shape-Id: com.amazonaws.devopsguru#CapstoneControlPlaneService
Issue-Key: devops-guru|AddNotificationChannel|ConflictException
Issue-Key: devops-guru.notification-channel-duplicate-topic-race-exactly-one-wins|run#10|AssertionError|assert len(ok_ids) == racers, f"expected all {racers} racers to succeed, got: {results}"
Issue-Key: devops-guru|RemoveNotificationChannel|ResourceNotFoundException
Issue-Key: devops-guru.resource-collection-multiple-tag-filters-coexist|_client_error#11|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: devops-guru.service-integration-kms-missing-keyid-rejected|_client_error#13|AssertionError|raise AssertionError(f"real AWS refuses CUSTOMER_MANAGED_KEY without a KMSKeyId, but it SUCCEEDED and returned {result!r}")
Issue-Key: devops-guru.describe-insight-cross-tenant-accountid-rejected|run#5|AssertionError|assert code == "AccessDeniedException", f"got {code!r}"
Issue-Commit: 659e25b97046a77449047daf7fe310a5a83846e1

# DevOps Guru: notification-channel idempotency missing both ways, resource-collection and service-integration refusals absent, cross-account DescribeInsight not authorized (5 causes)

> _Note: this issue was identified against the code base at commit `659e25b97046a77449047daf7fe310a5a83846e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddNotificationChannel, RemoveNotificationChannel, UpdateResourceCollection, UpdateServiceIntegration, DescribeInsight

## Description
Five distinct root causes in the DevOps Guru control plane, all reached by behavioral checks that DID get answered by the emulator. Two are missing idempotency on notification-channel mutation, two are missing server-side validation (the emulator accepts input real AWS refuses), and one is an authorization gap on a cross-account read.

Cause 1 of 5: AddNotificationChannel is not idempotent by target SNS topic.
AWS: AddNotificationChannel (Smithy: AddNotificationChannel, PUT /channels, input Config -> NotificationChannelConfig required, output Id -> NotificationChannelId required, 36-char UUID; modeled errors include ConflictException httpError 409) resolves a repeat registration of a topic that is ALREADY registered to the SAME existing channel: recorded behavior only — the recorded/empirical behavior is that the second call returns HTTP 200 with the SAME Id as the first call, and exactly one channel for that topic exists afterward. N concurrent AddNotificationChannel calls for one topic therefore all succeed and all return that one shared Id.
Emulator: the second (and every concurrent) registration of an already-registered topic is refused with HTTP 409 ConflictException (message example: "a notification channel for Amazon SNS topic arn:aws:sns:...:<topic> already exists"). In the 3-way race exactly one racer got an Id and the other two got 409.
Why: a client that re-runs its onboarding/bootstrap step (or runs it from several workers) sees a hard 409 where AWS hands back the existing channel. Idempotent provisioning code (Terraform/CDK-style converge loops, retried calls after a timeout) fails instead of converging, and the caller has no Id to use.
Fix: the AddNotificationChannel handler/state — look up an existing channel by Config.Sns.TopicArn before minting a new one and return that channel's Id rather than raising ConflictException.
Traces: devops-guru.add-notification-channel-duplicate-topic-is-idempotent, devops-guru.notification-channel-duplicate-topic-race-exactly-one-wins.

Cause 2 of 5: RemoveNotificationChannel is not idempotent on an already-removed channel.
AWS: RemoveNotificationChannel (Smithy: DELETE /channels/{Id}, input Id -> NotificationChannelId required httpLabel, empty output; ResourceNotFoundException IS modeled at httpError 404 but is not raised for this case) succeeds on a repeat removal of a channel that is already gone: recorded behavior only — the recording shows a second DELETE of the same Id returning the empty 200 output rather than a 404.
Emulator: the repeat removal is refused with HTTP 404 ResourceNotFoundException (message example: "no notification channel with id <uuid> exists").
Why: cleanup/teardown paths and retries after an ambiguous network result break. A client that retries a DELETE it already performed gets an error where AWS reports success, so teardown code must special-case a 404 that AWS never sends.
Fix: the RemoveNotificationChannel handler — treat an absent channel Id as a successful no-op and return the empty output instead of ResourceNotFoundException.
Traces: devops-guru.notification-channel-add-list-remove-lifecycle, devops-guru.notification-channel-removal-enables-topic-reuse.

Cause 3 of 5: UpdateResourceCollection accepts a SECOND, different tag AppBoundaryKey while one is already configured.
AWS: an account has exactly ONE tag AppBoundaryKey (Smithy: UpdateResourceCollection, PUT /resource-collections, input Action + ResourceCollection required, modeled ValidationException httpError 400; TagCollection.AppBoundaryKey doc: "All Amazon Web Services resources in your account and Region tagged with this key make up your DevOps Guru application and analysis boundary"). Adding TagValues under the already-configured key is additive, but an ADD that introduces a DIFFERENT AppBoundaryKey while one is configured is rejected with HTTP 400 ValidationException and persists nothing (recorded message example: "Different app boundary key provided in request (existing={<keyA>}, provided={<keyB>}), cant add these resource collection filters").
Emulator: that ADD is ACCEPTED — HTTP 200 with an empty body — so two distinct AppBoundaryKeys coexist (or silently replace one another) in the emulator's resource-collection state.
Why: the emulator is over-permissive on an account-singleton invariant. Client code that relies on AWS refusing a conflicting boundary key (and on the account's single analysis boundary) gets no error, and the account's coverage state diverges from anything AWS could hold; a subsequent GetResourceCollection returns a shape AWS never produces.
Fix: UpdateResourceCollection's validation/state — on Action=ADD with Tags, compare the request's AppBoundaryKey against the account's configured key and raise ValidationException (400) when they differ, without mutating state.
Traces: devops-guru.resource-collection-multiple-tag-filters-coexist.

Cause 4 of 5: UpdateServiceIntegration accepts KMSServerSideEncryption Type=CUSTOMER_MANAGED_KEY with no KMSKeyId.
AWS: UpdateServiceIntegration (Smithy: PUT /service-integrations, input ServiceIntegration -> UpdateServiceIntegrationConfig required, modeled ValidationException httpError 400; KMSServerSideEncryptionIntegrationConfig.KMSKeyId doc describes the key ID/ARN/alias that must be supplied to designate a customer-managed key) rejects an ENABLED CUSTOMER_MANAGED_KEY configuration that carries no KMSKeyId with HTTP 400 ValidationException and persists nothing (recorded message example: "Must specify KMSKeyId for CUSTOMER_MANAGED_KEY").
Emulator: the call is ACCEPTED — HTTP 200 with an empty body.
Why: the account is driven into an encryption state AWS cannot represent (customer-managed-key encryption enabled with no key). A client validating its KMS opt-in against the emulator passes, then fails against AWS; worse, code that reads back the integration sees encryption "enabled" with no key, which no real account can report.
Fix: UpdateServiceIntegration's validation — require KMSKeyId when KMSServerSideEncryption.OptInStatus=ENABLED and Type=CUSTOMER_MANAGED_KEY, raise ValidationException (400), and leave the stored ServiceIntegration unchanged.
Traces: devops-guru.service-integration-kms-missing-keyid-rejected.

Cause 5 of 5: DescribeInsight scoped to an inaccessible AccountId is not authorized — it answers the scaffold's 501 NotImplemented.
AWS: DescribeInsight (Smithy: GET /insights/{Id}, input Id required httpLabel + AccountId httpQuery AwsAccountId 12-digit; modeled AccessDeniedException httpError 403) refuses a request whose AccountId is not accessible to the caller (no organization/delegated-admin relationship) with HTTP 403 AccessDeniedException and never returns another account's insight (recorded message example: "Access denied when accessing insight <id> in account <accountId>").
Emulator: the request answers the unmodeled NotImplemented error code ("operation DescribeInsight is scaffolded but not implemented in this emulator"), so the cross-account authorization decision does not exist at all. Note: a separate already-filed issue covers the DescribeInsight scaffold stub itself (grounded in devops-guru.describe-insight-not-found); it names the unimplemented operation but not this cross-account rule, which must be implemented alongside the not-found path.
Why: a client cannot distinguish a tenancy refusal from a service outage, and any emulator fidelity test for cross-account isolation on DescribeInsight is untestable. Once the operation is implemented without this rule, the risk inverts into an isolation leak.
Fix: the DescribeInsight handler — when implementing it, authorize the AccountId query parameter against the caller's account/organization relationship and raise AccessDeniedException (403) for a non-accessible account before any insight lookup.
Traces: devops-guru.describe-insight-cross-tenant-accountid-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1, devops-guru 5/21 passed (24%), 16 failed.

Cause 1 — add-notification-channel-duplicate-topic-is-idempotent:
  expected: second AddNotificationChannel for the same TopicArn -> 200 with Id == first Id, exactly one channel for the topic
  actual:   botocore.errorfactory.ConflictException: An error occurred (ConflictException) when calling the AddNotificationChannel operation: a notification channel for Amazon SNS topic arn:aws:sns:us-west-2:000000000000:sim2real-devopsguru-topic-388962c1a961 already exists
Cause 1 — notification-channel-duplicate-topic-race-exactly-one-wins:
  expected: all 3 concurrent racers succeed with one shared Id
  actual:   [('ok', 'fc037dfa-...'), ('err', "ConflictException(... already exists)"), ('err', "ConflictException(... already exists)")]

Cause 2 — notification-channel-add-list-remove-lifecycle:
  expected: repeat RemoveNotificationChannel(Id) after the channel is gone -> success (empty 200)
  actual:   ResourceNotFoundException('An error occurred (ResourceNotFoundException) when calling the RemoveNotificationChannel operation: no notification channel with id c968d650-fda9-4fa8-8daa-82df615ea98f exists')
Cause 2 — notification-channel-removal-enables-topic-reuse:
  expected: idempotent no-op on already-removed channel
  actual:   AssertionError: removing an already-removed channel should be an idempotent no-op ..., got ResourceNotFoundException

Cause 3 — resource-collection-multiple-tag-filters-coexist:
  expected: ADD of a second, different AppBoundaryKey -> 400 ValidationException ("Different app boundary key provided in request (existing={...}, provided={...}), cant add these resource collection filters"), nothing persisted
  actual:   call SUCCEEDED: {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '2', ...}}}

Cause 4 — service-integration-kms-missing-keyid-rejected:
  expected: UpdateServiceIntegration(KMSServerSideEncryption={OptInStatus: ENABLED, Type: CUSTOMER_MANAGED_KEY}) -> 400 ValidationException ("Must specify KMSKeyId for CUSTOMER_MANAGED_KEY")
  actual:   call SUCCEEDED: {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '2', ...}}}

Cause 5 — describe-insight-cross-tenant-accountid-rejected:
  expected: DescribeInsight(Id=<id>, AccountId='111122223333') -> 403 AccessDeniedException
  actual:   AssertionError: got 'NotImplemented' ("operation DescribeInsight is scaffolded but not implemented in this emulator")

## Steps to Reproduce
Boot the emulator and point a botocore devops-guru client (plus an sns client for topics) at it.

Cause 1: sns.create_topic(Name=...); id1 = devopsguru.add_notification_channel(Config={'Sns': {'TopicArn': arn}})['Id']; call add_notification_channel again with the SAME TopicArn -> emulator raises 409 ConflictException; AWS returns 200 with Id == id1. Repeat with 3 concurrent threads: AWS gives 3 successes sharing one Id, emulator gives 1 success + 2 ConflictException.

Cause 2: devopsguru.remove_notification_channel(Id=id1); poll list_notification_channels until it is gone; call remove_notification_channel(Id=id1) again -> emulator raises 404 ResourceNotFoundException; AWS returns the empty 200 output.

Cause 3: update_resource_collection(Action='ADD', ResourceCollection={'Tags': [{'AppBoundaryKey': 'Devops-guru-a', 'TagValues': ['v1']}]}); then update_resource_collection(Action='ADD', ResourceCollection={'Tags': [{'AppBoundaryKey': 'Devops-guru-b', 'TagValues': ['v1']}]}) -> emulator returns 200; AWS returns 400 ValidationException and leaves key 'Devops-guru-a' intact.

Cause 4: update_service_integration(ServiceIntegration={'KMSServerSideEncryption': {'OptInStatus': 'ENABLED', 'Type': 'CUSTOMER_MANAGED_KEY'}}) with no KMSKeyId -> emulator returns 200; AWS returns 400 ValidationException.

Cause 5: describe_insight(Id='any-insight-id', AccountId='111122223333') -> emulator answers NotImplemented (501 scaffold); AWS answers 403 AccessDeniedException.
