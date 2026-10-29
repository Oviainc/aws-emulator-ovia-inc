Issue-Source: trace-sweep-divergence
Issue-Trace: email-tenant-creation-and-resource-association
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: email-tenant-creation-and-resource-association sesv2-reputation-entity-error-paths sesv2-tenant-lifecycle sesv2-reputation-entity-customer-managed-status sesv2-reputation-entity-policy-management
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2|CreateTenant|2xx->2xx|$.SuppressionAttributes
Issue-Key: sesv2|CreateTenant|2xx->2xx|$.TenantId
Issue-Key: sesv2|GetTenant|2xx->2xx|$.Tenant.SuppressionAttributes
Issue-Key: sesv2|ListTenantResources|2xx->2xx|$.TenantResources[*]
Issue-Key: sesv2|GetTenant|4xx->4xx|$.message
Issue-Key: sesv2|DeleteTenant|4xx->4xx|$.message
Issue-Key: sesv2|GetReputationEntity|2xx->2xx|$.ReputationEntity.AwsSesManagedStatus
Issue-Key: sesv2|GetReputationEntity|2xx->2xx|$.ReputationEntity.CustomerManagedStatus.Cause
Issue-Key: sesv2|GetReputationEntity|2xx->2xx|$.ReputationEntity.ReputationImpact
Issue-Key: sesv2|GetReputationEntity|2xx->2xx|$headers.content-type
Issue-Key: sesv2|GetReputationEntity|2xx->2xx|$.ReputationEntity.CustomerManagedStatus
Issue-Key: sesv2|GetReputationEntity|4xx->4xx|$headers.content-type
Issue-Key: sesv2|UpdateReputationEntityPolicy|4xx->4xx|$.message
Issue-Commit: a9006feb831fdd789c02a15722f93d2f2e52852a

# Tenant and reputation-entity reads expose unset status objects and wrong identifiers

> _Note: this issue was identified against the code base at commit `a9006feb831fdd789c02a15722f93d2f2e52852a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTenant

## Description
Ten net-new causes in the SESv2 tenant and reputation-entity planes. Smithy: Tenant {TenantName, TenantId, TenantArn, CreatedTimestamp, Tags, SendingStatus, SuppressionAttributes}; TenantResource.ResourceType -> ResourceType enum [EMAIL_IDENTITY, CONFIGURATION_SET, EMAIL_TEMPLATE]; ReputationEntity {ReputationEntityReference, ReputationEntityType, ReputationManagementPolicy, CustomerManagedStatus, AwsSesManagedStatus, SendingStatusAggregate, ReputationImpact}.

Cause 1 of 10: CreateTenant returns an unset SuppressionAttributes and a TenantId of the wrong form. AWS: when the create request sets no suppression preferences the response OMITS SuppressionAttributes (the ACCOUNT default is implicit, per TenantSuppressionAttributes.SuppressionScope's documented default), and TenantId is a SES tenant identifier of the form 'tn-' followed by a 30-character lowercase hex string. Emulator: returns SuppressionAttributes {SuppressionScope: ACCOUNT} that was never requested, and a TenantId that is a bare 32-hex-digit counter-like string with no 'tn-' prefix. Why: a client cannot distinguish an explicitly scoped tenant from a defaulted one, and any client that validates or parses the documented tenant-id format rejects the emulator's id. (The literal id in the recording is a per-run value; the requirement is the 'tn-<hex>' shape.) Fix: the CreateTenant handler's response construction and the tenant-id generator in tenant state. Traces: email-tenant-creation-and-resource-association#task#2 (same cluster also at sesv2-reputation-entity-error-paths#setup#1, sesv2-tenant-lifecycle#task#1).

Cause 2 of 10: the same fabricated SuppressionAttributes on the CreateTenant path used by the reputation fixtures. Traces: sesv2-reputation-entity-customer-managed-status#task#1 (same cluster also at sesv2-reputation-entity-policy-management#task#1).

Cause 3 of 10: GetTenant echoes the fabricated SuppressionAttributes. AWS: Tenant.SuppressionAttributes is absent for a tenant created without suppression preferences. Emulator: returns {SuppressionScope: ACCOUNT}. Why: as Cause 1, on the read path. Fix: the GetTenant serializer / tenant state must only hold explicitly set suppression attributes. Traces: email-tenant-creation-and-resource-association#task#4.

Cause 4 of 10: ListTenantResources reports the wrong ResourceType token. AWS: the recorded response gives TenantResources[i].ResourceType as 'identity' for an associated email identity. Emulator: returns 'EMAIL_IDENTITY'. (Smithy models ResourceType as [EMAIL_IDENTITY, CONFIGURATION_SET, EMAIL_TEMPLATE]; the recording shows AWS serving the lowercase short token on this path, so this is recorded behavior only and the recording is the requirement.) Why: a client filtering associated resources by the value AWS returns never matches the emulator's token. Fix: the ListTenantResources serializer's resource-type mapping. Traces: email-tenant-creation-and-resource-association#task#7.

Cause 5 of 10: GetTenant unknown-tenant refusal message. AWS: 404 NotFoundException with 'The requested tenant <TENANT-NAME> does not exist.' — the fixed text is 'The requested tenant ' + '<' + name + '>' + ' does not exist.', the variable part being the TenantName from the request. Emulator: 'Tenant <name> does not exist.' without the brackets or the 'The requested' lead-in. Why: a client matching AWS's sentence does not recognise the error. Fix: the GetTenant handler's not-found refusal message. Traces: email-tenant-creation-and-resource-association#task#15 (same cluster also at sesv2-tenant-lifecycle#task#4).

Cause 6 of 10: DeleteTenant unknown-tenant refusal message — same AWS sentence as Cause 5 ('The requested tenant <NAME> does not exist.'), same emulator shortening, on the delete path. Fix: the DeleteTenant handler's not-found refusal message. Traces: sesv2-tenant-lifecycle#task#3. The open issue 20261028T060336Z-sesv2-deletetenant-cascades-the-resource-keyed-tenant-index.md concerns index cascading, not this message.

Cause 7 of 10: GetReputationEntity fabricates AwsSesManagedStatus, drops CustomerManagedStatus.Cause and never reports ReputationImpact. AWS: for a resource entity whose customer-managed status was set, the response carries CustomerManagedStatus with a Cause sentence ('Status manually updated.' when the status was set through UpdateReputationEntityCustomerManagedStatus), reports ReputationImpact (NONE when no finding is active) and OMITS AwsSesManagedStatus when SES has not set one. Emulator: returns AwsSesManagedStatus {Status: ENABLED, LastUpdatedTimestamp}, omits CustomerManagedStatus.Cause and omits ReputationImpact; the response also carries content-type application/json instead of application/x-amz-json-1.1 (recorded behavior only). Why: a client that derives sending eligibility or displays why a status changed reads an SES-managed status that does not exist and gets no impact level at all. Fix: the GetReputationEntity serializer and the reputation-entity state (cause text, impact, SES-managed record). Traces: sesv2-reputation-entity-customer-managed-status#task#3 (same cluster also at task#5, task#7).

Cause 8 of 10: the same GetReputationEntity defect on the policy-management path, where BOTH status records are fabricated. AWS: CustomerManagedStatus and AwsSesManagedStatus are both absent for an entity whose statuses were never set, and ReputationImpact is NONE. Emulator: both come back as {Status: ENABLED, LastUpdatedTimestamp} and ReputationImpact is absent. Why: a client cannot tell an unset status from an explicitly enabled one. Fix: as Cause 7. Traces: sesv2-reputation-entity-policy-management#task#2 (same cluster also at task#4, task#6).

Cause 9 of 10: content-type on the GetReputationEntity 400 (BadRequestException) response — application/json instead of application/x-amz-json-1.1; status and error code match. Fix: the service's response content-type default. Traces: sesv2-reputation-entity-error-paths#task#2.

Cause 10 of 10: UpdateReputationEntityPolicy invalid-policy refusal message. AWS: 400 BadRequestException with the fixed sentence 'Provided resource identifier is invalid' — no variable part. Emulator: an emulator-authored message that echoes the supplied ARN and enumerates the managed policies ('Invalid reputation management policy: <arn>. The policy must be an Amazon SES-managed policy: none, standard, strict.'). Why: a client matching AWS's refusal text does not recognise the error. Fix: the UpdateReputationEntityPolicy handler's validation refusal message. Traces: sesv2-reputation-entity-error-paths#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1.
Cause 1 (email-tenant-creation-and-resource-association#task#2, 200/200): $.SuppressionAttributes expected absent, got {'SuppressionScope': 'ACCOUNT'}; $.TenantId expected 'tn-a208a7f1d0100b91f666be58a6074' (tn-<30 hex>), got '000001a188a431e60000000000000001'.
Cause 2 (sesv2-reputation-entity-customer-managed-status#task#1, 200/200): $.SuppressionAttributes expected absent, got {'SuppressionScope': 'ACCOUNT'}.
Cause 3 (task#4, 200/200): $.Tenant.SuppressionAttributes expected absent, got {'SuppressionScope': 'ACCOUNT'}.
Cause 4 (task#7, 200/200): $.TenantResources[0].ResourceType expected 'identity', got 'EMAIL_IDENTITY' (ResourceArn matches).
Cause 5 (task#15, 404/404): $.message expected 'The requested tenant <sim2real-probe-sesv2-tenant-76b331f1> does not exist.', got 'Tenant sim2real-probe-sesv2-tenant-76b331f1 does not exist.'
Cause 6 (sesv2-tenant-lifecycle#task#3, 404/404): $.message expected 'The requested tenant <sim2real-probe-sesv2-tenant-387de600> does not exist.', got 'Tenant sim2real-probe-sesv2-tenant-387de600 does not exist.'
Cause 7 (sesv2-reputation-entity-customer-managed-status#task#3, 200/200): $.ReputationEntity.AwsSesManagedStatus expected absent, got {'Status': 'ENABLED', 'LastUpdatedTimestamp': ...}; $.ReputationEntity.CustomerManagedStatus.Cause expected 'Status manually updated.', got absent; $.ReputationEntity.ReputationImpact expected 'NONE', got absent; content-type application/json vs application/x-amz-json-1.1.
Cause 8 (sesv2-reputation-entity-policy-management#task#2, 200/200): $.ReputationEntity.AwsSesManagedStatus and $.ReputationEntity.CustomerManagedStatus both expected absent, got {'Status': 'ENABLED', 'LastUpdatedTimestamp': ...}; $.ReputationEntity.ReputationImpact expected 'NONE', got absent.
Cause 9 (sesv2-reputation-entity-error-paths#task#2, 400/400): $headers.content-type expected 'application/x-amz-json-1.1', got 'application/json'.
Cause 10 (sesv2-reputation-entity-error-paths#task#3, 400/400): $.message expected 'Provided resource identifier is invalid', got 'Invalid reputation management policy: arn:aws:ses:us-west-2:aws:reputation-policy/not-a-real-policy. The policy must be an Amazon SES-managed policy: none, standard, strict.'

## Steps to Reproduce
1. Replay fixtures email-tenant-creation-and-resource-association (task#2, task#4, task#7, task#15), sesv2-tenant-lifecycle (task#3), sesv2-reputation-entity-customer-managed-status (task#1, task#3), sesv2-reputation-entity-policy-management (task#2) and sesv2-reputation-entity-error-paths (task#2, task#3).
2. CreateTenant without SuppressionAttributes and inspect the response's SuppressionAttributes and the form of TenantId; then GetTenant for the same tenant.
3. Associate an email identity with the tenant and call ListTenantResources; compare ResourceType.
4. GetTenant and DeleteTenant for a tenant name that does not exist; compare the 404 messages.
5. UpdateReputationEntityCustomerManagedStatus, then GetReputationEntity; compare CustomerManagedStatus.Cause, AwsSesManagedStatus and ReputationImpact. Repeat with only UpdateReputationEntityPolicy applied.
6. UpdateReputationEntityPolicy with a policy ARN that is not SES-managed; compare the 400 message. Inspect the content-type header on the GetReputationEntity 400.
