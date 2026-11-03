Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.associate-packages-batch-duplicate-name-collapsed opensearch.dissociate-packages-not-associated-rejected opensearch.inbound-outbound-connection-accept-reject-lifecycle opensearch.domain-upgrade-lifecycle opensearch.update-scheduled-action-conflicting-window-rejected opensearch.vpc-endpoint-access-ops-on-missing-domain opensearch.data-source-attachment-lifecycle opensearch.osdocstat-fgac-without-encryption-at-rest-rejected opensearch.osdocstat-fgac-without-enforce-https-rejected opensearch.osdocstat-fgac-without-node-to-node-encryption-rejected opensearch.osdocstat-fgac-master-user-arn-of-a-non-principal-rejected opensearch.albos-fgac-and-node-to-node-encryption-cannot-be-disabled-once-enabled
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.associate-packages-batch-duplicate-name-collapsed|run#18|AssertionError|assert message == "Operation not allowed. Duplicate packages found in the request.", (
Issue-Key: opensearch.dissociate-packages-not-associated-rejected|run#18|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: opensearch.inbound-outbound-connection-accept-reject-lifecycle|run#39|AssertionError|assert rejected_inbound_delete_message == expected, (
Issue-Key: opensearch.domain-upgrade-lifecycle|run#24|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: opensearch.update-scheduled-action-conflicting-window-rejected|run#12|AssertionError|assert control_message == expected, f"expected message {expected!r}, got {control_message!r}"
Issue-Key: opensearch.vpc-endpoint-access-ops-on-missing-domain|run#7|AssertionError|assert message == expected, f'AuthorizeVpcEndpointAccess: expected message {expected!r}, got {message!r}'
Issue-Key: opensearch.data-source-attachment-lifecycle|run#24|AssertionError|assert message == expected_message, (
Issue-Key: opensearch.osdocstat-fgac-without-encryption-at-rest-rejected|_expect_ear_rejection#5|AssertionError|assert refused is not None, (
Issue-Key: opensearch.osdocstat-fgac-without-enforce-https-rejected|_expect_https_rejection#5|AssertionError|assert refused is not None, (
Issue-Key: opensearch.osdocstat-fgac-without-node-to-node-encryption-rejected|_expect_n2n_rejection#5|AssertionError|assert refused is not None, (
Issue-Key: opensearch.osdocstat-fgac-master-user-arn-of-a-non-principal-rejected|run#8|AssertionError|assert refused is not None, (
Issue-Key: opensearch.albos-fgac-and-node-to-node-encryption-cannot-be-disabled-once-enabled|run#8|AssertionError|assert got == expected, f"disabling FGAC was answered {got!r}, expected {expected!r}"
Issue-Commit: 87f5308bc0aabe3f00012e198d39bc9d2e6e6ae4

# opensearch: ten control-plane divergences — six refusal messages are emulator-worded, FGAC prerequisites and MasterUserARN type are unvalidated at CreateDomain, and UpdateDomainConfig answers a config change with a change-in-progress error instead of the documented immutability refusals (10 causes)

> _Note: this issue was identified against the code base at commit `87f5308bc0aabe3f00012e198d39bc9d2e6e6ae4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
Ten net-new control-plane root causes in the opensearch service crate, each with its own failing check ids. Causes 1-7 are refusal-wording divergences (status and error code are right, the sentence is not); causes 8-10 are missing validation, where the emulator ACCEPTS a request AWS refuses, or refuses with the wrong rule.

Cause 1 of 10 — AssociatePackages: the duplicate-package refusal is emulator-worded.
AWS: Smithy: AssociatePackages (POST /2021-01-01/packages/associateMultiple, PackageList required) declares ValidationException (errorKind client, httpError 400). A request listing the same package twice is refused as a whole with ValidationException and the fixed sentence "Operation not allowed. Duplicate packages found in the request." — the message is entirely fixed text and names no id.
Emulator: ValidationException with "PackageList names package <packageId> twice." — different fixed text, and it interpolates the request's PackageID (the recorded run shows pkg-3615177a41f74f38ba3bf3c2e751098d; that value is per-run and is not part of the requirement).
Why: a client or operator that matches/handles the documented duplicate-batch sentence never sees it; log- and alarm-matching on that text silently stops firing.
Fix: the refusal message rendered by the AssociatePackages handler's duplicate-detection path in services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/associate_packages.rs.
Traces: opensearch.associate-packages-batch-duplicate-name-collapsed

Cause 2 of 10 — DissociatePackages: the not-associated refusal is emulator-worded.
AWS: Smithy: DissociatePackages (POST /2021-01-01/packages/dissociateMultiple) declares ValidationException (client, 400). Dissociating a package that was never associated with the domain is refused with ValidationException and the fixed sentence "Package is not associated to this domain." — all fixed text, no ids.
Emulator: ValidationException with "Package <packageId> is not associated with domain <domainName>." — different fixed text, plus two interpolated per-run values (recorded: pkg-ffca3c19eefb493cbad944a87103e32e, sim2real-dpaf593863e432) that AWS does not include.
Why: same as cause 1 — the refusal is the only signal that a dissociate was a no-op, and its text does not match AWS.
Fix: the refusal message in the DissociatePackages handler in .../src/ops/dissociate_packages.rs.
Traces: opensearch.dissociate-packages-not-associated-rejected

Cause 3 of 10 — DeleteInboundConnection on a REJECTED connection: the refusal drops the "Connection <id>" subject.
AWS: recorded behavior only (the model binds only DisabledOperationException/ResourceNotFoundException to DeleteInboundConnection; the recording shows an unmodeled ValidationException). Deleting an inbound connection whose status is REJECTED is refused with error code ValidationException and the message "Connection <ConnectionId> existing status is REJECTED, unable to make necessary change". Fixed text: "Connection ", " existing status is REJECTED, unable to make necessary change"; variable part: the ConnectionId from the request path (recorded: cc-jvvdqvyaaaaa).
Emulator: the same call answers ValidationException with "existing status is REJECTED, unable to make necessary change" — the leading "Connection <id> " subject is missing, so the message does not say WHICH connection was refused.
Why: a caller tearing down several connections cannot attribute the refusal to a connection, and exact-match handling of the documented sentence fails.
Fix: the refusal message composed on the terminal-status path of the DeleteInboundConnection handler in .../src/ops/delete_inbound_connection.rs.
Traces: opensearch.inbound-outbound-connection-accept-reject-lifecycle

Cause 4 of 10 — UpgradeDomain: the unsupported-path refusal renders the target version with its engine prefix.
AWS: Smithy: UpgradeDomain.TargetVersion is a VersionString matching ^Elasticsearch_[0-9]\.[0-9]{1,2}$|^OpenSearch_[0-9]{1,2}\.[0-9]{1,2}$; an invalid upgrade path is refused with the unmodeled BaseException (client, httpError 400) and the message "Failed to submit upgrade. Upgrade from <source engine version> to <bare target version> not supported." Fixed text: "Failed to submit upgrade. Upgrade from ", " to ", " not supported."; variable parts: the domain's current EngineVersion as declared (e.g. OpenSearch_2.11) and the requested TargetVersion with its engine prefix STRIPPED (TargetVersion=Elasticsearch_7.10 renders as 7.10).
Emulator: same code, message "Failed to submit upgrade. Upgrade from OpenSearch_2.11 to Elasticsearch_7.10 not supported." — the target is echoed verbatim instead of stripped to the bare version number.
Why: a client matching the documented refusal sentence for a downgrade/unsupported path does not match, and the two version members are rendered inconsistently (source prefixed, target bare in AWS).
Fix: the message rendering on the unsupported-path branch of the UpgradeDomain handler in .../src/ops/upgrade_domain.rs.
Traces: opensearch.domain-upgrade-lifecycle

Cause 5 of 10 — UpdateScheduledAction with an unknown ActionID: the not-found refusal is emulator-worded.
AWS: Smithy: UpdateScheduledAction (PUT /2021-01-01/opensearch/domain/{DomainName}/scheduledAction/update) declares ResourceNotFoundException (client, httpError 409). A well-formed request (ActionType SERVICE_SOFTWARE_UPDATE, ScheduleAt NOW) naming an ActionID that does not exist is refused ResourceNotFoundException with the fixed sentence "ReleaseVersion does not applies for this domain or release does not exist" (AWS's wording, grammatical quirk included) — all fixed text, no interpolation.
Emulator: ResourceNotFoundException with "Scheduled action <actionId> doesn't exist for domain <domainName>." — different fixed text plus two per-run values.
Why: the error code is right but the sentence never appears in AWS, so clients that disambiguate the two refusal classes of this operation by message read something unknown.
Fix: the not-found message in the UpdateScheduledAction handler in .../src/ops/update_scheduled_action.rs.
Traces: opensearch.update-scheduled-action-conflicting-window-rejected

Cause 6 of 10 — Authorize/RevokeVpcEndpointAccess on an unknown domain: the refusal is the generic domain-not-found sentence.
AWS: Smithy: AuthorizeVpcEndpointAccess (POST /2021-01-01/opensearch/domain/{DomainName}/authorizeVpcEndpointAccess) and RevokeVpcEndpointAccess declare ResourceNotFoundException (client, httpError 409). For a domain that does not exist, both answer ResourceNotFoundException with the deliberately non-committal fixed sentence "Either the domain doesn't exist, it doesn't support creation of VPC endpoints, or you don't have permission to create a VPC endpoint for the domain." — all fixed text; it intentionally does NOT disclose whether the domain exists.
Emulator: ResourceNotFoundException with "Domain not found: <domainName>" — the generic domain-not-found sentence, which also discloses existence the AWS message withholds.
Why: beyond the wording mismatch, the emulator turns a deliberately ambiguous refusal into an existence oracle, so a caller (and a security reviewer) sees a different information-disclosure posture than AWS.
Fix: the not-found messages on the VPC-endpoint-access handlers in .../src/ops/authorize_vpc_endpoint_access.rs and .../src/ops/revoke_vpc_endpoint_access.rs (do not reuse the shared domain-not-found helper here).
Traces: opensearch.vpc-endpoint-access-ops-on-missing-domain

Cause 7 of 10 — DescribeDataSourceAttachment after a detach: the not-found refusal is emulator-worded and transposes its members.
AWS: Smithy: DescribeDataSourceAttachment declares ResourceNotFoundException (client, httpError 409); for an id/dataSourceArn pair that names no attachment the message is "Data source attachment not found for application <id> and data source <dataSourceArn>". Fixed text: "Data source attachment not found for application ", " and data source "; variable parts: the request's id and dataSourceArn, in that order, with no trailing full stop.
Emulator: ResourceNotFoundException with "Data source <dataSourceArn> is not attached to application <id>." — different fixed text, members transposed, trailing full stop added.
Note: open issue 20261101T140814Z-opensearch-describedatasourceattachment-s-resourcenotfoundexception-is-emulator-worded-data.md describes this same wording defect but does not carry this check's replay key, so it does not cover it; fixing them together is expected.
Why: the refusal is the documented observable consequence of DetachDataSource, so a client that confirms a detach by matching the message never matches.
Fix: the not-found message in the DescribeDataSourceAttachment handler in .../src/ops/describe_data_source_attachment.rs.
Traces: opensearch.data-source-attachment-lifecycle

Cause 8 of 10 — CreateDomain accepts fine-grained access control without its documented prerequisites.
AWS: Smithy: CreateDomain.AdvancedSecurityOptions (AdvancedSecurityOptionsInput) and ValidationException (client, httpError 400). AWS documents that fine-grained access control requires encryption at rest, node-to-node encryption and HTTPS-only traffic (https://docs.aws.amazon.com/opensearch-service/latest/developerguide/fgac.html). CreateDomain with AdvancedSecurityOptions.Enabled=true and any one prerequisite missing is refused ValidationException, no domain is created, and the message names the missing prerequisite: "You must enable encryption at rest to use advanced security options." (EncryptionAtRestOptions omitted or Enabled=false), "You must enable node-to-node encryption to use advanced security options." (NodeToNodeEncryptionOptions omitted or Enabled=false), "You must enable EnforceHTTPS in the domain endpoint options to use advanced security." (DomainEndpointOptions omitted or EnforceHTTPS=false). All three are fully fixed text. After the refusal DescribeDomain answers ResourceNotFoundException "Domain not found: <DomainName>".
Emulator: all three requests are ACCEPTED — CreateDomain returns a DomainStatus and the domain exists, so no refusal is produced at all.
Why: a caller gets a domain that AWS would never create: FGAC enabled with no encryption at rest, no node-to-node encryption, or plaintext HTTP allowed. Code paths that depend on the refusal (infrastructure validation, policy-as-code tests, CFN rollbacks) pass in the emulator and fail against AWS, and the emulated domain's security posture is one AWS does not permit.
Why also: open issue 20261029T182036Z-opensearch-ten-documented-request-validation-and-authorization-rules.md carries a related FGAC-prerequisite check but not these three replay keys, so they are not covered.
Fix: the AdvancedSecurityOptions validation in the CreateDomain handler in .../src/ops/create_domain.rs (and the shared validator it should share with UpdateDomainConfig), before any domain record is written.
Traces: opensearch.osdocstat-fgac-without-encryption-at-rest-rejected, opensearch.osdocstat-fgac-without-enforce-https-rejected, opensearch.osdocstat-fgac-without-node-to-node-encryption-rejected

Cause 9 of 10 — CreateDomain does not validate that the FGAC master user ARN is an IAM principal.
AWS: Smithy: MasterUserOptions.MasterUserARN is an ARN (length 20-2048) documented as "Amazon Resource Name (ARN) for the master user". A well-formed ARN that is not an IAM user, root user or role (the recording uses an S3 bucket ARN, arn:aws:s3:::<name>) is refused with ValidationException and the fixed sentence "The master user ARN is not one of the allowed types: user, root user, or role." — all fixed text. No domain is created: DescribeDomain then answers ResourceNotFoundException "Domain not found: <DomainName>".
Emulator: the request is ACCEPTED and the domain is created, so the refusal never happens.
Why: the domain's FGAC master principal can be set to something that can never authenticate, and the emulator reports a working security configuration that AWS rejects outright.
Fix: MasterUserARN type validation in the CreateDomain handler's AdvancedSecurityOptions validator in .../src/ops/create_domain.rs (accept only iam user/root/role ARNs).
Traces: opensearch.osdocstat-fgac-master-user-arn-of-a-non-principal-rejected

Cause 10 of 10 — UpdateDomainConfig answers a follow-up config change with a change-in-progress error instead of the documented immutability refusals.
AWS: Smithy: UpdateDomainConfig (POST /2021-01-01/opensearch/domain/{DomainName}/config) declares ValidationException (client, httpError 400). On a live FGAC domain, an AccessPolicies update is accepted and the immediately following attempts to turn the two one-way switches OFF are each refused ValidationException with their own fixed sentence: AdvancedSecurityOptions.Enabled=false -> "You can't disable advanced security options once you enable it."; NodeToNodeEncryptionOptions.Enabled=false -> "Once enabled, node to node encryption cannot be disabled". Both are fully fixed text, and both settings stay enabled.
Emulator: the AdvancedSecurityOptions.Enabled=false call answers ValidationException with "A change/update is in progress. Please wait for it to complete before requesting another change." — the immutability rule is never evaluated; instead a serialisation gate the recording does not show fires because the preceding accepted AccessPolicies update left the domain processing.
Why: two different defects in one answer: the documented one-way-switch rules are unenforced (so a caller cannot learn that FGAC/node-to-node encryption are irreversible, and a request AWS always refuses may be applied once the gate clears), and config updates that AWS accepts back-to-back are refused in the emulator, so any multi-step reconfiguration sequence stalls.
Fix: the UpdateDomainConfig handler in .../src/ops/update_domain_config.rs — evaluate the immutability validation for AdvancedSecurityOptions and NodeToNodeEncryptionOptions before the in-progress gate, and relax that gate to match the recorded behaviour of accepting a further config change on a just-updated domain.
Traces: opensearch.albos-fgac-and-node-to-node-encryption-cannot-be-disabled-once-enabled

## Evidence
Cause 1: expected ValidationException 'Operation not allowed. Duplicate packages found in the request.'; got ValidationException 'PackageList names package pkg-3615177a41f74f38ba3bf3c2e751098d twice.'
Cause 2: expected 'Package is not associated to this domain.'; got 'Package pkg-ffca3c19eefb493cbad944a87103e32e is not associated with domain sim2real-dpaf593863e432.'
Cause 3: expected ValidationException 'Connection cc-jvvdqvyaaaaa existing status is REJECTED, unable to make necessary change'; got 'existing status is REJECTED, unable to make necessary change' (the connection id is a per-run value filling the template; the "Connection <id> " prefix is the requirement).
Cause 4: UpgradeDomain(TargetVersion='Elasticsearch_7.10', PerformCheckOnly=true) on an OpenSearch_2.11 domain — expected BaseException 'Failed to submit upgrade. Upgrade from OpenSearch_2.11 to 7.10 not supported.'; got '... to Elasticsearch_7.10 not supported.'
Cause 5: UpdateScheduledAction(ActionID=<nonexistent>, ActionType=SERVICE_SOFTWARE_UPDATE, ScheduleAt=NOW) — code ResourceNotFoundException matched; expected message 'ReleaseVersion does not applies for this domain or release does not exist'; got "Scheduled action sim2real-bogus-action-d10f751a9a8b doesn't exist for domain sim2real-sch-47ba2128a278."
Cause 6: AuthorizeVpcEndpointAccess(DomainName=<nonexistent>, Account=<caller>) — code ResourceNotFoundException matched; expected message "Either the domain doesn't exist, it doesn't support creation of VPC endpoints, or you don't have permission to create a VPC endpoint for the domain."; got 'Domain not found: sim2real-behav-os-missing-2f'. RevokeVpcEndpointAccess diverges the same way.
Cause 7: expected 'Data source attachment not found for application sfi9sv1baaaa and data source arn:aws:es:us-west-2:000000000000:domain/sim2real-at-7f92b92bce44'; got 'Data source arn:aws:es:us-west-2:000000000000:domain/sim2real-at-7f92b92bce44 is not attached to application sfi9sv1baaaa.'
Cause 8: three CreateDomain requests with AdvancedSecurityOptions.Enabled=true and, respectively, EncryptionAtRestOptions / DomainEndpointOptions(EnforceHTTPS) / NodeToNodeEncryptionOptions omitted — expected ValidationException with 'You must enable encryption at rest to use advanced security options.' / 'You must enable EnforceHTTPS in the domain endpoint options to use advanced security.' / 'You must enable node-to-node encryption to use advanced security options.' and no domain; each was ACCEPTED ('expected CreateDomain with advanced security (omitted EncryptionAtRestOptions) to be REJECTED, but it was accepted').
Cause 9: CreateDomain with MasterUserOptions.MasterUserARN='arn:aws:s3:::<bucket>' — expected ValidationException 'The master user ARN is not one of the allowed types: user, root user, or role.' and no domain; it was accepted ('expected CreateDomain with an S3 ARN as FGAC master user to be REJECTED, but it was accepted').
Cause 10: on a live FGAC + node-to-node-encrypted domain, after an accepted AccessPolicies update, UpdateDomainConfig(AdvancedSecurityOptions={Enabled:false}) — expected ('ValidationException', "You can't disable advanced security options once you enable it."); got ('ValidationException', 'A change/update is in progress. Please wait for it to complete before requesting another change.').

## Steps to Reproduce
1. Cause 8/9: CreateDomain (EngineVersion OpenSearch_2.11, ClusterConfig {t3.small.search, 1}, EBSOptions gp3/10) with AdvancedSecurityOptions {Enabled:true, InternalUserDatabaseEnabled:false, MasterUserOptions:{MasterUserARN:<caller principal>}} but omit EncryptionAtRestOptions — observe a 200 DomainStatus instead of ValidationException; repeat omitting NodeToNodeEncryptionOptions, then omitting DomainEndpointOptions/EnforceHTTPS; then repeat with MasterUserARN='arn:aws:s3:::<any-bucket>'. DescribeDomain each name: the domain exists.
2. Cause 10: CreateDomain with NodeToNodeEncryptionOptions{Enabled:true}, EncryptionAtRestOptions{Enabled:true}, DomainEndpointOptions{EnforceHTTPS:true} and AdvancedSecurityOptions enabled; wait until it settles; UpdateDomainConfig(AccessPolicies=<policy>) (accepted); then UpdateDomainConfig(AdvancedSecurityOptions={Enabled:false}) -> 'A change/update is in progress...' instead of "You can't disable advanced security options once you enable it."; then UpdateDomainConfig(NodeToNodeEncryptionOptions={Enabled:false}) -> expect 'Once enabled, node to node encryption cannot be disabled'.
3. Cause 1: CreatePackage from an S3 object, then AssociatePackages(DomainName=<domain>, PackageList=[{PackageID:<id>},{PackageID:<same id>}]) and read the ValidationException message.
4. Cause 2: DissociatePackages(DomainName=<domain>, PackageList=[<packageId never associated>]) and read the ValidationException message.
5. Cause 3: create a cross-cluster outbound connection, reject it from the inbound side so its status is REJECTED, then DeleteInboundConnection(ConnectionId=<id>) and read the ValidationException message.
6. Cause 4: on an OpenSearch_2.11 domain call UpgradeDomain(TargetVersion='Elasticsearch_7.10', PerformCheckOnly=true) and read the BaseException message.
7. Cause 5: UpdateScheduledAction(DomainName=<existing domain>, ActionID=<nonexistent>, ActionType='SERVICE_SOFTWARE_UPDATE', ScheduleAt='NOW') and read the ResourceNotFoundException message.
8. Cause 6: AuthorizeVpcEndpointAccess(DomainName=<nonexistent domain>, Account=<account id>) and RevokeVpcEndpointAccess likewise; read the ResourceNotFoundException messages.
9. Cause 7: CreateApplication + CreateDomain, AttachDataSource(id,dataSourceArn), wait ATTACHED, DetachDataSource, then DescribeDataSourceAttachment(id,dataSourceArn) and read the ResourceNotFoundException message.
