Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: fms.batch-associate-resource-without-admin-rejected fms.delete-apps-list-without-admin-rejected fms.delete-protocols-list-without-admin-rejected fms.disassociate-third-party-firewall-not-admin-rejected fms.get-notification-channel-unconfigured-not-found fms.get-protocols-list-nonexistent-without-admin-rejected fms.get-third-party-firewall-association-status-unconfigured-not-found fms.list-discovered-resources-non-admin-rejected fms.put-notification-channel-deploy-round-trip fms.put-resource-set-invalid-resource-type-rejected
Issue-Shape-Id: com.amazonaws.fms#AWSFMS_20180101
Issue-Key: fms.batch-associate-resource-without-admin-rejected|run#8|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}"
Issue-Key: fms.delete-apps-list-without-admin-rejected|run#5|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}"
Issue-Key: fms.delete-protocols-list-without-admin-rejected|run#5|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}"
Issue-Key: fms.disassociate-third-party-firewall-not-admin-rejected|run#3|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}"
Issue-Key: fms.get-notification-channel-unconfigured-not-found|run#3|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}"
Issue-Key: fms.get-protocols-list-nonexistent-without-admin-rejected|run#5|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}"
Issue-Key: fms.get-third-party-firewall-association-status-unconfigured-not-found|run#3|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}"
Issue-Key: fms.list-discovered-resources-non-admin-rejected|run#5|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}"
Issue-Key: fms.put-notification-channel-deploy-round-trip|run#21|AssertionError|assert put_code == "AccessDeniedException", f"expected AccessDeniedException, got {put_code}"
Issue-Key: fms.put-resource-set-invalid-resource-type-rejected|run#6|AssertionError|assert bad_code == "AccessDeniedException", f"expected AccessDeniedException, got {bad_code}"
Issue-Commit: 360c4c1618594432a64f84ba94d0fa368ed8f619

# FMS: ten administrator-gated operations are unimplemented scaffold stubs answering 501 NotImplemented instead of the service's authorization-gate rejection

> _Note: this issue was identified against the code base at commit `360c4c1618594432a64f84ba94d0fa368ed8f619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourceSet

## Description
Ten Firewall Manager operations in services/service-com-amazonaws-fms-awsfms-20180101/src/ops/ are still scaffold stubs: their handle() bodies end in Err(crate::scaffolded::Scaffolded::not_implemented("<Op>")), which serializes as HTTP 501 with error code `NotImplemented` (confirmed by reading tag_resource.rs lines 149-167, the same template every stub uses; two sibling stubs, DisassociateAdminAccount and ListMemberAccounts, are already filed separately and are NOT part of this issue). Real Firewall Manager answers each of these with a modeled 400 client error — for an account with no Firewall Manager default administrator association it is AccessDeniedException, which the Smithy model does not list for these operations, so this is recorded behavior only (each check's docstring records the live-AWS outcome, and the no-admin precondition is independently confirmed in-suite by GetAdminAccount answering ResourceNotFoundException). This issue bundles TEN root causes — ten separate handlers to implement. Each must be closed before submitting.

Common shape for all ten causes:
AWS: 400 with a modeled client error code (AccessDeniedException for the no-admin state; recorded behavior only — the model lists only InternalErrorException/InvalidInputException/InvalidOperationException/LimitExceededException/ResourceNotFoundException for these ops, per smithy_query operation on each). The recorded message is an example: "No default admin could be found for account <account> in Region <region>".
Emulator: 501, Error.Code `NotImplemented` (message example: "operation <Op> is scaffolded but not implemented in this emulator").
Why: a client gets a non-AWS error code with a 5xx status. botocore treats 501 as a server fault and retries it, and SDK error handling that branches on AccessDeniedException/ResourceNotFoundException never fires, so callers cannot distinguish "you are not the FM administrator" from a transient server failure. Negative-path and authorization tests against the emulator cannot run at all.
Fix: implement each operation's handler in services/service-com-amazonaws-fms-awsfms-20180101/src/ops/<op>.rs — add the administrator-association gate (read state::service_state(..).admin_account, as get_admin_account.rs already does) and return the authorization refusal BEFORE any identifier lookup or payload validation, then delete the SCAFFOLDED marker. A shared gate helper in the service crate would serve all ten.

Cause 1 of 10: BatchAssociateResource. The gate must run before the ResourceSetIdentifier is looked up and before item ARNs are validated, so a nonexistent resource-set id AND a malformed item ARN both surface the authorization error rather than a per-item FailedItems result, NOT_VALID_ARN, or ResourceNotFoundException. Check: fms.batch-associate-resource-without-admin-rejected.

Cause 2 of 10: DeleteAppsList. Applications lists are administrator-scoped; a well-formed but nonexistent ListId must surface the authorization error, not ResourceNotFoundException and not success. Check: fms.delete-apps-list-without-admin-rejected.

Cause 3 of 10: DeleteProtocolsList. Same gate as DeleteAppsList for protocols lists. Check: fms.delete-protocols-list-without-admin-rejected.

Cause 4 of 10: GetProtocolsList. The gate runs before the ListId lookup, so the response is the authorization error rather than a list-specific ResourceNotFoundException. Check: fms.get-protocols-list-nonexistent-without-admin-rejected.

Cause 5 of 10: GetNotificationChannel. With no administrator configured the read is refused by the authorization gate rather than returning a channel or a channel-specific ResourceNotFoundException. Check: fms.get-notification-channel-unconfigured-not-found.

Cause 6 of 10: PutNotificationChannel (input SnsTopicArn + SnsRoleName, both required ResourceArn, len 1..1024 — smithy_query PutNotificationChannel). For an account with no administrator association the write is refused and leaves no channel state, so a following GetNotificationChannel is refused identically. Check: fms.put-notification-channel-deploy-round-trip.

Cause 7 of 10: ListDiscoveredResources (MemberAccountIds + ResourceType required). Cross-account resource discovery is administrator-only; a non-administrator caller must be refused rather than receiving Items. Check: fms.list-discovered-resources-non-admin-rejected.

Cause 8 of 10: PutResourceSet. The authorization gate must run BEFORE any ResourceTypeList semantic validation, so a bogus resource type ("AWS::Bogus::NotARealType") and a valid one ("AWS::NetworkFirewall::Firewall") are refused identically with the same error, and no resource set is created (a subsequent ListResourceSets must be refused the same way). Check: fms.put-resource-set-invalid-resource-type-rejected.

Cause 9 of 10: DisassociateThirdPartyFirewall (ThirdPartyFirewall required enum). A caller that is not the FM default administrator is refused; per the recording the refusal is the generic authorization error, not the InvalidOperationException the model documents. Check: fms.disassociate-third-party-firewall-not-admin-rejected.

Cause 10 of 10: GetThirdPartyFirewallAssociationStatus (ThirdPartyFirewall required enum). Same gate as cause 9 on the read path, rather than a status-specific ResourceNotFoundException. Check: fms.get-third-party-firewall-association-status-unconfigured-not-found.

Traces: the ten failing check ids listed above (see trace_ids).

## Evidence
Replay of suite simparity-behavioral-aws-v1, service fms: 0/24 checks passed.

Expected (real AWS, recorded): HTTP 400, Error.Code AccessDeniedException, message example "No default admin could be found for account <account> in Region <region>".
Actual (emulator): HTTP 501, Error.Code NotImplemented.

Per-check assertion output:
- fms.batch-associate-resource-without-admin-rejected: AssertionError: expected AccessDeniedException, got NotImplemented
- fms.delete-apps-list-without-admin-rejected: AssertionError: expected AccessDeniedException, got NotImplemented
- fms.delete-protocols-list-without-admin-rejected: AssertionError: expected AccessDeniedException, got NotImplemented
- fms.disassociate-third-party-firewall-not-admin-rejected: AssertionError: expected AccessDeniedException, got NotImplemented
- fms.get-notification-channel-unconfigured-not-found: AssertionError: expected AccessDeniedException, got NotImplemented
- fms.get-protocols-list-nonexistent-without-admin-rejected: AssertionError: expected AccessDeniedException, got NotImplemented
- fms.get-third-party-firewall-association-status-unconfigured-not-found: AssertionError: expected AccessDeniedException, got NotImplemented
- fms.list-discovered-resources-non-admin-rejected: AssertionError: expected AccessDeniedException, got NotImplemented
- fms.put-notification-channel-deploy-round-trip: AssertionError: expected AccessDeniedException, got NotImplemented (PutNotificationChannel leg)
- fms.put-resource-set-invalid-resource-type-rejected: AssertionError: expected AccessDeniedException, got NotImplemented (GetAdminAccount health check first returned the expected ResourceNotFoundException, so the emulator IS reachable and in the no-admin state)

Source evidence that the cause is an unimplemented stub: services/service-com-amazonaws-fms-awsfms-20180101/src/ops/tag_resource.rs:166 — Err(crate::scaffolded::Scaffolded::not_implemented("TagResource")) — the identical template used by the ten handlers above. Contrast services/service-com-amazonaws-fms-awsfms-20180101/src/ops/get_admin_account.rs:145-168, an implemented handler that reads store.admin_account and returns a modeled 400.

Model evidence: smithy_query(query="operation", operation="PutResourceSet,BatchAssociateResource,DeleteAppsList,DeleteProtocolsList,GetProtocolsList,GetNotificationChannel,PutNotificationChannel,ListDiscoveredResources,DisassociateThirdPartyFirewall,GetThirdPartyFirewallAssociationStatus") on services/service-com-amazonaws-fms-awsfms-20180101/smithy-model/com-amazonaws-fms-awsfms-20180101.json — every one models only 400-class client errors (InternalErrorException, InvalidInputException, InvalidOperationException, LimitExceededException, ResourceNotFoundException); none models a 501 or a NotImplemented code.

## Steps to Reproduce
1. Boot the emulator from this checkout and point a botocore fms client at it, in an account/region with no Firewall Manager administrator association (the emulator's default state; confirm with fms.get_admin_account(), which raises ResourceNotFoundException).
2. Call each of: fms.batch_associate_resource(ResourceSetIdentifier="0123456789AbCdEfGhIjKl", Items=["arn:aws:network-firewall:<region>:<account>:firewall/x","this-is-not-an-arn-at-all"]); fms.delete_apps_list(ListId=<uuid4>); fms.delete_protocols_list(ListId=<uuid4>); fms.get_protocols_list(ListId=<uuid4>); fms.get_notification_channel(); fms.put_notification_channel(SnsTopicArn=<topic arn>, SnsRoleName=<role arn>); fms.list_discovered_resources(MemberAccountIds=[<account>], ResourceType="AWS::EC2::Instance"); fms.put_resource_set(ResourceSet={"Name":"x","ResourceTypeList":["AWS::Bogus::NotARealType"]}); fms.disassociate_third_party_firewall(ThirdPartyFirewall="PALO_ALTO_NETWORKS_CLOUD_NGFW"); fms.get_third_party_firewall_association_status(ThirdPartyFirewall="PALO_ALTO_NETWORKS_CLOUD_NGFW").
3. Observe each raises a ClientError with HTTP status 501 and Error.Code "NotImplemented" instead of a 400 AccessDeniedException.
4. Re-replay the ten checks in trace_ids to confirm a fix.
