Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: shield.create-protection-group-conflicting-pattern-fields-rejected shield.create-protection-group-unprotected-member-not-found shield.describe-emergency-contacts-absent-without-subscription shield.disassociate-drt-role-requires-subscription shield.list-protection-groups-invalid-pagination-token-rejected shield.list-resources-in-protection-group-missing-group-not-found shield.tag-resource-nonexistent-resource-rejected shield.update-subscription-requires-existing-subscription
Issue-Shape-Id: com.amazonaws.shield#AWSShield_20160616
Issue-Key: shield.create-protection-group-conflicting-pattern-fields-rejected|run#7|AssertionError|assert bad_code == "InvalidParameterException", f"expected InvalidParameterException, got {bad_code!r}"
Issue-Key: shield.create-protection-group-unprotected-member-not-found|run#7|AssertionError|assert code == "InvalidParameterException", f"expected InvalidParameterException, got {code!r}"
Issue-Key: shield.describe-emergency-contacts-absent-without-subscription|run#5|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Key: shield.disassociate-drt-role-requires-subscription|run#5|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Key: shield.list-protection-groups-invalid-pagination-token-rejected|run#3|AssertionError|assert baseline_code == "ResourceNotFoundException", (
Issue-Key: shield.list-resources-in-protection-group-missing-group-not-found|run#4|AssertionError|assert err["Code"] == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {err['Code']!r}"
Issue-Key: shield.tag-resource-nonexistent-resource-rejected|run#5|AssertionError|assert not_found_code == "InvalidResourceException", (
Issue-Key: shield.update-subscription-requires-existing-subscription|run#3|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Commit: 78295efc92da5c3ab6dc723b39af1e4944870efd

# shield: seven operations answer HTTP 501 NotImplemented instead of their modeled rejections

> _Note: this issue was identified against the code base at commit `78295efc92da5c3ab6dc723b39af1e4944870efd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProtectionGroup

## Description
The shield emulator is stood up and serving (GetSubscriptionState, DescribeProtection and other operations answer with real modeled responses), but seven further operations exercised by this suite still return the scaffold's HTTP 501 `NotImplemented` (`operation <Name> is scaffolded but not implemented in this emulator`) for requests real Shield answers with a specific modeled 400. Each of the seven is a separate cause with its own handler file; none of them is covered by the issues already filed for DescribeAttackStatistics / ListAttacks / DescribeDRTAccess. 7 causes.

Cause 1 of 7: CreateProtectionGroup never validates its Pattern/Members/ResourceType field combination.
AWS: Smithy CreateProtectionGroup models InvalidParameterException (HTTP 400) and ResourceNotFoundException (HTTP 400); Members doc: "You must set this when you set Pattern to ARBITRARY and you must not set it for any other Pattern setting", ResourceType doc: "You must set this when you set Pattern to BY_RESOURCE_TYPE and you must not set it for any other Pattern setting". Recorded behavior (unsubscribed account): Pattern=BY_RESOURCE_TYPE sent together with Members, and Pattern=ARBITRARY whose Members ARN resolves to no protected resource, are both refused with HTTP 400 `InvalidParameterException` and the fixed message "One or more protection group input fields are incorrect." (no variable parts), and the group is NOT created — a following DescribeProtectionGroup for that id must answer `ResourceNotFoundException` "The referenced protection-group does not exist." (fixed text). A structurally valid request (Pattern=ALL) on an unsubscribed account is instead refused with `ResourceNotFoundException` "The subscription does not exist." (fixed text), which is what distinguishes field validation from the subscription gate.
Emulator: HTTP 501, error code `NotImplemented`, message "operation CreateProtectionGroup is scaffolded but not implemented in this emulator".
Why: a client cannot distinguish a malformed protection-group request from a missing subscription, and code that asserts on InvalidParameterException (or retries on a 5xx-class 501) takes the wrong branch; nothing in the group lifecycle is testable.
Fix: the operation's handler plus its field-combination validation and protection-group state.
Traces: shield.create-protection-group-conflicting-pattern-fields-rejected, shield.create-protection-group-unprotected-member-not-found.

Cause 2 of 7: DescribeEmergencyContactSettings does not gate on the subscription.
AWS: Smithy DescribeEmergencyContactSettings models only InternalErrorException (500) and ResourceNotFoundException (400); with no Shield Advanced subscription (GetSubscriptionState == INACTIVE) the recording shows HTTP 400 `ResourceNotFoundException` with the fixed message "The subscription does not exist."
Emulator: HTTP 501 `NotImplemented`, "operation DescribeEmergencyContactSettings is scaffolded but not implemented in this emulator".
Why: a client probing whether emergency contacts are configured gets an unmodeled 501 it has no handler for instead of the modeled absence signal.
Fix: the operation's handler, reading the subscription state the service already keeps.
Traces: shield.describe-emergency-contacts-absent-without-subscription.

Cause 3 of 7: DisassociateDRTRole does not gate on the subscription.
AWS: Smithy DisassociateDRTRole models InvalidOperationException, OptimisticLockException and ResourceNotFoundException (all HTTP 400). On an unsubscribed account the recording shows `ResourceNotFoundException` "The subscription does not exist." (fixed text) — specifically NOT InvalidOperationException, which is the "no DRT role currently associated" case.
Emulator: HTTP 501 `NotImplemented`, "operation DisassociateDRTRole is scaffolded but not implemented in this emulator".
Why: a client cannot tell "no subscription" from "nothing to disassociate", and teardown code that treats the modeled not-found as idempotent success instead sees a 501 failure.
Fix: the operation's handler plus the shared subscription gate.
Traces: shield.disassociate-drt-role-requires-subscription.

Cause 4 of 7: ListProtectionGroups answers neither the subscription gate nor pagination-token validation.
AWS: Smithy ListProtectionGroups models InvalidPaginationTokenException (400) and ResourceNotFoundException (400); NextToken is Token, @length 1..4096. Recorded behavior: an un-tokened call on an unsubscribed account is refused with `ResourceNotFoundException` "The subscription does not exist."; the same call with only NextToken changed to a malformed value is refused with `InvalidPaginationTokenException` "Invalid pagination token." — both messages are fixed text with no variable parts, and token shape is inspected ahead of the subscription gate.
Emulator: HTTP 501 `NotImplemented`, "operation ListProtectionGroups is scaffolded but not implemented in this emulator".
Why: pagination loops cannot distinguish a stale/garbled token from an unsubscribed account, so a client retries forever or aborts for the wrong reason.
Fix: the operation's handler, its NextToken validation (ordered before the subscription check) and the protection-group listing state.
Traces: shield.list-protection-groups-invalid-pagination-token-rejected.

Cause 5 of 7: ListResourcesInProtectionGroup never reports a missing protection group.
AWS: Smithy ListResourcesInProtectionGroup requires ProtectionGroupId (@length 1..36, pattern ^[a-zA-Z0-9\-]*$) and models ResourceNotFoundException (400) and InvalidPaginationTokenException (400). Recorded behavior: for a ProtectionGroupId that was never created, HTTP 400 `ResourceNotFoundException` with the group-specific fixed message "The referenced protection-group does not exist." — distinct from the account-level "The subscription does not exist." that shares the same error code.
Emulator: HTTP 501 `NotImplemented`, "operation ListResourcesInProtectionGroup is scaffolded but not implemented in this emulator".
Why: the error code alone cannot separate "no such group" from "no subscription"; a client branching on the message gets neither, and gets an unmodeled status instead.
Fix: the operation's handler and protection-group state lookup, with the two not-found messages kept distinct.
Traces: shield.list-resources-in-protection-group-missing-group-not-found.

Cause 6 of 7: TagResource (and the ListTagsForResource consistency leg) does not resolve the ResourceARN.
AWS: Smithy TagResource requires ResourceARN (ResourceArn, @length 1..2048, pattern ^arn:aws) and Tags, and models InvalidParameterException, InvalidResourceException and ResourceNotFoundException (all HTTP 400). Recorded behavior: both a well-formed-but-nonexistent Shield protection ARN and a syntactically valid ARN for a resource type Shield does not manage tags on (e.g. an S3 bucket) are refused with `InvalidResourceException` and the fixed message "The referenced resource is invalid." — not ResourceNotFoundException. ListTagsForResource on the same nonexistent ARN is refused identically.
Emulator: HTTP 501 `NotImplemented`, "operation TagResource is scaffolded but not implemented in this emulator".
Why: tagging code cannot detect a bad ARN at all; the emulator neither refuses it as AWS does nor lets the client classify the failure, and an eventual naive implementation that answers ResourceNotFoundException would still diverge.
Fix: the operation's handler plus the shared Shield resource-ARN resolution used by both tagging operations.
Traces: shield.tag-resource-nonexistent-resource-rejected.

Cause 7 of 7: UpdateSubscription does not gate on an existing subscription.
AWS: Smithy UpdateSubscription takes AutoRenew and models InvalidParameterException, LockedSubscriptionException, OptimisticLockException and ResourceNotFoundException (all HTTP 400). Recorded behavior with no active subscription: HTTP 400 `ResourceNotFoundException` with the fixed message "The subscription does not exist."
Emulator: HTTP 501 `NotImplemented`, "operation UpdateSubscription is scaffolded but not implemented in this emulator".
Why: a client that must refuse to flip AutoRenew without a subscription sees an unmodeled 501, and worse, cannot rely on the emulator refusing the mutation at all.
Fix: the operation's handler plus the subscription state and its gate.
Traces: shield.update-subscription-requires-existing-subscription.

## Evidence
Suite report (simparity-behavioral-aws-v1), service shield 3/19 (16%), 16 failed. All eight checks below fail on the same shape of response — HTTP 501 with error code 'NotImplemented' and message 'operation <Name> is scaffolded but not implemented in this emulator' — where AWS returns a modeled HTTP 400:

- shield.create-protection-group-conflicting-pattern-fields-rejected: AssertionError: expected InvalidParameterException, got 'NotImplemented' (expected message "One or more protection group input fields are incorrect.")
- shield.create-protection-group-unprotected-member-not-found: AssertionError: expected InvalidParameterException, got 'NotImplemented' (expected message "One or more protection group input fields are incorrect.")
- shield.describe-emergency-contacts-absent-without-subscription: AssertionError: expected ResourceNotFoundException, got 'NotImplemented' (expected message "The subscription does not exist."); the check's positive control GetSubscriptionState succeeded and returned INACTIVE, so the emulator IS serving shield
- shield.disassociate-drt-role-requires-subscription: AssertionError: expected ResourceNotFoundException, got 'NotImplemented' (expected "The subscription does not exist.")
- shield.list-protection-groups-invalid-pagination-token-rejected: AssertionError: expected ResourceNotFoundException for the untokened baseline, got 'NotImplemented' (expected "The subscription does not exist."; and InvalidPaginationTokenException / "Invalid pagination token." for a malformed NextToken)
- shield.list-resources-in-protection-group-missing-group-not-found: AssertionError: expected ResourceNotFoundException, got 'NotImplemented' (expected "The referenced protection-group does not exist.")
- shield.tag-resource-nonexistent-resource-rejected: AssertionError: expected InvalidResourceException, got 'NotImplemented' (expected "The referenced resource is invalid.")
- shield.update-subscription-requires-existing-subscription: AssertionError: expected ResourceNotFoundException, got 'NotImplemented' (expected "The subscription does not exist.")

Handler files still carrying the '//! SCAFFOLDED, NOT IMPLEMENTED:' marker and returning Scaffolded::not_implemented(..), under services/service-com-amazonaws-shield-awsshield-20160616/src/ops/: create_protection_group.rs (line 193), describe_emergency_contact_settings.rs, disassociate_drt_role.rs, list_protection_groups.rs, list_resources_in_protection_group.rs, tag_resource.rs, update_subscription.rs (and list_tags_for_resource.rs for the consistency leg of the tagging cause). None of these operations is named by the already-filed scaffold issues (DescribeAttackStatistics, ListAttacks, DescribeDRTAccess).

## Steps to Reproduce
Boot the emulator and point a botocore shield client at it in any region, with no Shield Advanced subscription provisioned, then:
1. create_protection_group(ProtectionGroupId=<new id>, Aggregation='SUM', Pattern='BY_RESOURCE_TYPE', ResourceType='ELASTIC_IP_ALLOCATION', Members=[<any eip-allocation ARN>]) -> expect InvalidParameterException "One or more protection group input fields are incorrect."; also create_protection_group(..., Pattern='ARBITRARY', Members=[<unprotected ARN>]) -> same rejection, and describe_protection_group(<that id>) -> ResourceNotFoundException "The referenced protection-group does not exist."
2. describe_emergency_contact_settings() -> expect ResourceNotFoundException "The subscription does not exist."
3. disassociate_drt_role() -> expect ResourceNotFoundException "The subscription does not exist."
4. list_protection_groups() -> expect ResourceNotFoundException "The subscription does not exist."; list_protection_groups(NextToken='not-a-real-continuation-token-xxxx...') -> expect InvalidPaginationTokenException "Invalid pagination token."
5. list_resources_in_protection_group(ProtectionGroupId=<never-created id>) -> expect ResourceNotFoundException "The referenced protection-group does not exist."
6. tag_resource(ResourceARN='arn:aws:shield::<account>:protection/00000000-0000-0000-0000-000000000000', Tags=[{'Key':'k','Value':'v'}]) -> expect InvalidResourceException "The referenced resource is invalid."; repeat with an S3 bucket ARN and with list_tags_for_resource on the same ARN
7. update_subscription(AutoRenew='DISABLED') -> expect ResourceNotFoundException "The subscription does not exist."
Each call currently returns HTTP 501 'NotImplemented'. Re-replay the eight check ids in trace_ids to confirm a fix.
