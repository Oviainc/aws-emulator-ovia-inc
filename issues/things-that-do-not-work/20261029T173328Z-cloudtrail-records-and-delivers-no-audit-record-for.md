Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail.lookup-events-records-the-transport-details-of-my-own-call cloudtrail__cognito-identity.identity-pool-data-events-are-delivered-only-once-a-selector-names-them cloudtrail__cognito-identity__s3.xsvc-cognito-identity-1-guest-and-authenticated-sessions-audited-with-own-amr-and-role cloudtrail__cognito-identity__s3.xsvc-cognito-identity-1-vended-session-list-buckets-audited-as-cognito-federation cloudtrail__ecr__kms.a-kms-encrypted-repository-is-audited-as-ecr-acting-on-the-callers-behalf cloudtrail__events__sqs.read-only-management-events-reach-a-rule-only-in-the-all-management-events-state cloudtrail__iam.iam-write-is-audited-only-in-the-partition-leader-region cloudtrail__lambda.obs-lambda-3-duplicate-permission-rejected-and-audited cloudtrail__s3.write-only-read-write-type-delivers-the-put-and-withholds-the-get
Issue-Key: cloudtrail.lookup-events-records-the-transport-details-of-my-own-call|run#18|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Key: cloudtrail__cognito-identity.identity-pool-data-events-are-delivered-only-once-a-selector-names-them|run#39|Timeout|time.sleep(_DELIVERY_POLL_S)
Issue-Key: cloudtrail__cognito-identity__s3.xsvc-cognito-identity-1-guest-and-authenticated-sessions-audited-with-own-amr-and-role|run#29|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Key: cloudtrail__cognito-identity__s3.xsvc-cognito-identity-1-vended-session-list-buckets-audited-as-cognito-federation|run#35|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Key: cloudtrail__ecr__kms.a-kms-encrypted-repository-is-audited-as-ecr-acting-on-the-callers-behalf|run#17|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Key: cloudtrail__events__sqs.read-only-management-events-reach-a-rule-only-in-the-all-management-events-state|run#32|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Key: cloudtrail__iam.iam-write-is-audited-only-in-the-partition-leader-region|run#20|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Key: cloudtrail__lambda.obs-lambda-3-duplicate-permission-rejected-and-audited|run#35|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Key: cloudtrail__s3.write-only-read-write-type-delivers-the-put-and-withholds-the-get|run#23|Timeout|time.sleep(_ROUND_INTERVAL_S)
Issue-Commit: d7c92a359e65b119da339df847b0735e9ef403e3

# CloudTrail records and delivers no audit record for the caller's own transport, service-principal (invokedBy) calls, cross-service sessions, global IAM writes, Lambda/S3 data events or the EventBridge fan-out (8 causes)

> _Note: this issue was identified against the code base at commit `d7c92a359e65b119da339df847b0735e9ef403e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
LookupEvents

## Description
Eight further surfaces of CloudTrail's recording/delivery pipeline produce nothing at all in the emulator. Each check sets its subject up successfully (trail created, IsLogging true, selectors accepted, the audited call accepted), then polls LookupEvents and/or the trail's S3 prefix for the record and never observes one, exhausting its budget. All eight are distinct from the already-open event-history issues (20261024T114720Z-cloudtrail-event-history-is-empty-for-everything-but.md, 20261028T010530Z-cloudtrail-event-history-records-nothing-for-read-only.md, 20261027T003725Z-cloudtrail-delivers-no-data-event-log-files-for.md, 20261025T121312Z-cloudtrail-lookupevents-records-no-iam-management-event-so.md) in the surface they exercise, and none of these eight checks is marked OWNED in the replay report.

Cause 1 of 8: the record of the caller's own call carries no transport details.
AWS: recorded behavior only (the CloudTrail record-contents reference documents the members). A cloudtrail.amazonaws.com management record for the caller's own LookupEvents carries userAgent equal to the User-Agent header the client actually sent (so a botocore user_agent_extra string appears VERBATIM and is absent from a record of a call made without it), sourceIPAddress equal to the caller's own address (an IP literal, stable across calls from one process, never a service principal and never "AWS Internal"), and a tlsDetails object with tlsVersion ("TLSv1.<minor>"), cipherSuite and clientProvidedHostHeader (the HOST of the endpoint the client used, no scheme, no port — the host value follows the target and is not a requirement in itself).
Emulator: no record for the caller's own call is ever returned by LookupEvents, so none of userAgent, sourceIPAddress or tlsDetails exists; the check exhausts its index budget.
Why: transport fields are what incident response uses to tell one client/host/TLS posture from another; a consumer that keys on userAgent or sourceIPAddress sees nothing and concludes the call never happened.
Fix: the CloudTrail event-recording path must capture the request's User-Agent header, peer address and TLS parameters onto the record it stores, and LookupEvents must return that record.
Traces: cloudtrail.lookup-events-records-the-transport-details-of-my-own-call

Cause 2 of 8: Cognito identity-pool DATA events are never delivered once a selector names them.
AWS: recorded behavior only (Cognito's CloudTrail page and the GetOpenIdToken example). cognito-identity GetId/GetOpenIdToken are DATA events: not captured by a default management-only trail, and captured once an advanced event selector of {Field: eventCategory, Equals: ["Data"]} + {Field: resources.type, Equals: ["AWS::Cognito::IdentityPool"]} is put. The delivered record carries eventCategory "Data", managementEvent false, a resources entry of type AWS::Cognito::IdentityPool whose ARN is the pool, requestParameters.logins.<provider> redacted to the exact string "HIDDEN_DUE_TO_SECURITY_REASONS", and never appears in LookupEvents.
Emulator: GetEventSelectors accepts and stores the selector, the trail reports IsLogging=true, and zero objects are ever written under AWSLogs/<account>/CloudTrail/<region>/ — so the redaction and the Data classification can never be observed and the check exhausts its delivery budget.
Why: the only observable place a data event exists is a delivered log file; with none, a consumer cannot distinguish "selector not applied" from "pipeline dead", and the token-redaction contract is unverifiable.
Fix: the trail delivery pipeline (data-event selector evaluation plus S3 log-file writer) in the cloudtrail service crate.
Traces: cloudtrail__cognito-identity.identity-pool-data-events-are-delivered-only-once-a-selector-names-them

Cause 3 of 8: a Cognito-vended session's S3 call is never audited with its federation identity.
AWS: recorded behavior only. S3 ListBuckets called with credentials vended by GetCredentialsForIdentity is recorded as an s3.amazonaws.com management event whose userIdentity is an AssumedRole with sessionContext.sessionIssuer naming the identity-pool role, a session name of "CognitoIdentityCredentials", and sessionContext.webIdFederationData.attributes carrying the "cognito-identity.amazonaws.com:amr" claim (a JSON-array string) distinguishing a guest (unauthenticated) session from an authenticated one; LookupEvents with LookupAttributes AccessKeyId=<the session's AKID> returns it.
Emulator: LookupEvents returns no record for either the guest or the authenticated session's ListBuckets, under AccessKeyId lookup, for the whole budget — so neither the per-session role attribution nor the amr claim exists.
Why: distinguishing guest from authenticated federated activity is the entire point of auditing a Cognito pool; with no record a consumer sees unattributed access.
Fix: management-event recording for s3.amazonaws.com calls made with web-identity/Cognito-vended temporary credentials, including the webIdFederationData population in the userIdentity serializer.
Traces: cloudtrail__cognito-identity__s3.xsvc-cognito-identity-1-guest-and-authenticated-sessions-audited-with-own-amr-and-role, cloudtrail__cognito-identity__s3.xsvc-cognito-identity-1-vended-session-list-buckets-audited-as-cognito-federation

Cause 4 of 8: calls a service makes on the caller's behalf (userIdentity.invokedBy) are never audited, and ECR leaves no KMS grant.
AWS: recorded behavior only (ECR's CloudTrail page states CloudTrail captures "All actions taken due to the encryption settings on your repositories"; the CloudTrail userIdentity reference documents invokedBy). Creating an ECR repository with encryptionConfiguration {encryptionType: KMS, kmsKey: <arn>} makes ECR call KMS on the caller's behalf; those kms.amazonaws.com records carry the CALLER's identity type and ARN with userIdentity.invokedBy == "ecr.amazonaws.com", alongside the caller's own ecr.amazonaws.com CreateRepository record (eventSource from the model's cloudTrailEventSource), and ECR holds a KMS grant on the key (kms:ListGrants shows an ECR service principal grantee with Decrypt and GenerateDataKey-family operations) that is retired when the repository is deleted.
Emulator: neither the caller's own CreateRepository record nor any invokedBy KMS record appears in LookupEvents, so the caller-vs-service attribution contrast cannot be formed at all; the check exhausts its budget.
Why: invokedBy is how an auditor tells "a service did this for me" from "I did this"; without it every service-initiated action is invisible.
Fix: event recording for service-principal-initiated calls (populate userIdentity.invokedBy) plus the ECR-side KMS grant lifecycle.
Traces: cloudtrail__ecr__kms.a-kms-encrypted-repository-is-audited-as-ecr-acting-on-the-callers-behalf

Cause 5 of 8: the CloudTrail-to-EventBridge fan-out never delivers "AWS API Call via CloudTrail".
AWS: recorded behavior only (CloudTrail's EventBridge integration page: to receive detail-type "AWS API Call via CloudTrail" you must have a trail currently logging management or data events). With such a trail, an SQS management write (e.g. TagQueue) is delivered to a matching EventBridge rule as {"source": "aws.sqs", "detail-type": "AWS API Call via CloudTrail", "detail": <the CloudTrail record>}, and a READ-ONLY management event (e.g. ListMessageMoveTasks) reaches a rule only when its State is ENABLED_WITH_ALL_CLOUDTRAIL_MANAGEMENT_EVENTS, not plain ENABLED; the delivered record's own readOnly member states the classification.
Emulator: with the trail logging, neither the write nor the read-only management event is ever delivered to either rule, so the rule-state partition is unobservable and the check exhausts its budget.
Why: a consumer whose automation is triggered by CloudTrail-sourced EventBridge events never fires; the ENABLED_WITH_ALL_CLOUDTRAIL_MANAGEMENT_EVENTS state is silently inert.
Fix: the CloudTrail -> EventBridge fan-out, and EventBridge rule-state evaluation for read-only management events.
Traces: cloudtrail__events__sqs.read-only-management-events-reach-a-rule-only-in-the-all-management-events-state

Cause 6 of 8: a global-service (IAM) write is not recorded in the partition leader region.
AWS: recorded behavior only (CloudTrail concepts, Global service events). An IAM write made from ANY region is recorded with awsRegion "us-east-1" and is returned by LookupEvents in us-east-1 ONLY — not in the caller's region — while an ordinary regional call is returned in its own region and not in us-east-1. For iam:CreateRole the record carries a populated requestParameters (the role name) and a populated responseElements (the created role's RoleId).
Emulator: LookupEvents in us-east-1 never returns the IAM write, so neither the leader-region routing nor the requestParameters/responseElements population can be observed; the check exhausts its budget. The open issue 20261025T121312Z-cloudtrail-lookupevents-records-no-iam-management-event-so.md covers the secret-omission contract for IAM records; the leader-region ROUTING property reported here is a separate surface and that issue does not carry this check.
Why: anyone auditing IAM in a multi-region account looks in us-east-1 by instruction; an empty answer there reads as "no IAM activity".
Fix: region attribution for global-service events in the event-recording path, and the LookupEvents region filter.
Traces: cloudtrail__iam.iam-write-is-audited-only-in-the-partition-leader-region

Cause 7 of 8: a refused Lambda AddPermission is never audited.
AWS: recorded behavior only; measured against real AWS. A successful and a DUPLICATE (refused) lambda:AddPermission are both recorded as lambda.amazonaws.com management events under eventName "AddPermission20150331v2", the refused one carrying the rejection's errorCode and the SDK's own request id, so the rejection is auditable.
Emulator: the function and the first AddPermission are accepted, but LookupEvents returns no lambda.amazonaws.com record for either the success or the refused duplicate; the check exhausts its 420s index budget.
Why: the versioned eventName and the audited rejection are what a policy-drift detector keys on; with no record, a duplicate-permission attempt leaves no trace.
Fix: management-event recording for lambda.amazonaws.com, including the operation's versioned eventName and the errorCode/errorMessage of a refused call.
Traces: cloudtrail__lambda.obs-lambda-3-duplicate-permission-rejected-and-audited

Cause 8 of 8: ReadWriteType does not partition S3 data events, because none are delivered.
AWS: recorded behavior only (PutEventSelectors reference, EventSelector$ReadWriteType with values ReadOnly/WriteOnly/All; AWS::S3::Object with a Values entry of arn:aws:s3:::<bucket>/ selects every object in the bucket). With ReadWriteType "WriteOnly", a PutObject data event is delivered to the trail's S3 prefix and a GetObject is withheld; widening the selector to "All" and repeating the identical GetObject delivers it. Delivered records carry eventCategory "Data", managementEvent false, eventSource "s3.amazonaws.com" and readOnly tracking the operation.
Emulator: the selector is accepted and readable via GetEventSelectors, but no data-event log file is delivered in either phase, so "the GetObject was withheld" cannot be told from "no data events are delivered at all"; the check exhausts its delivery windows.
Why: ReadWriteType is the primary cost/noise control on data events; an inert setting means a consumer's trail configuration has no observable effect.
Fix: data-event selector evaluation (ReadWriteType partitioning) and the S3 log-file delivery writer.
Traces: cloudtrail__s3.write-only-read-write-type-delivers-the-put-and-withholds-the-get

## Evidence
Replay report (suite simparity-behavioral-aws-v1). Eight causes over nine failing checks; none marked ALREADY FILED or OWNED.

Expected (AWS): for each cause, the record/log file described above becomes observable within the check's budget — via LookupEvents (Causes 1, 3, 6, 7), via a gzipped log file under AWSLogs/<account>/CloudTrail/<region>/ (Causes 2, 8), or via an EventBridge target (Cause 5) — and carries the named members.

Actual (emulator): every one of the nine checks reaches its audited call successfully and then observes NOTHING for the whole budget, surfacing as:
  [mismatch] cloudtrail.lookup-events-records-the-transport-details-of-my-own-call — Error: Check timed out after 60.0s
  [mismatch] cloudtrail__cognito-identity.identity-pool-data-events-are-delivered-only-once-a-selector-names-them — Error: Check timed out after 60.0s (prior real-AWS audit notes in the check record 0 objects under the trail prefix and LatestDeliveryTime=None against this target class)
  [mismatch] cloudtrail__cognito-identity__s3.xsvc-cognito-identity-1-guest-and-authenticated-sessions-audited-with-own-amr-and-role — Error: Check timed out after 60.0s
  [mismatch] cloudtrail__cognito-identity__s3.xsvc-cognito-identity-1-vended-session-list-buckets-audited-as-cognito-federation — Error: Check timed out after 60.0s
  [mismatch] cloudtrail__ecr__kms.a-kms-encrypted-repository-is-audited-as-ecr-acting-on-the-callers-behalf — Error: Check timed out after 60.0s
  [mismatch] cloudtrail__events__sqs.read-only-management-events-reach-a-rule-only-in-the-all-management-events-state — Error: Check timed out after 60.0s
  [mismatch] cloudtrail__iam.iam-write-is-audited-only-in-the-partition-leader-region — Error: Check timed out after 60.0s
  [mismatch] cloudtrail__lambda.obs-lambda-3-duplicate-permission-rejected-and-audited — Error: Check timed out after 60.0s
  [mismatch] cloudtrail__s3.write-only-read-write-type-delivers-the-put-and-withholds-the-get — Error: Check timed out after 60.0s

The setup calls in each check (CreateTrail, StartLogging, PutEventSelectors/PutAdvancedEventSelectors, CreateRepository, CreateFunction/AddPermission, PutObject/GetObject, CreateRole, GetId/GetOpenIdToken/GetCredentialsForIdentity) all SUCCEED against the emulator, so the divergence is isolated to recording and delivery, not to the audited operations themselves.

## Steps to Reproduce
Per cause, against the booted emulator:
1 (transport): make three LookupEvents calls from one process, one of them with botocore user_agent_extra set to a run-unique string; poll LookupEvents for the record of those calls and inspect userAgent / sourceIPAddress / tlsDetails. No record appears.
2 (cognito data events): create a trail with a logging S3 bucket, PutAdvancedEventSelectors with eventCategory=Data + resources.type=AWS::Cognito::IdentityPool, call cognito-identity GetId/GetOpenIdToken with a logins map, then list and gunzip objects under AWSLogs/<account>/CloudTrail/<region>/. Zero objects.
3 (cognito federation): create an identity pool with roles, vend guest and developer-authenticated sessions via GetCredentialsForIdentity, call s3:ListBuckets with each, then LookupEvents with LookupAttributes AccessKeyId=<session AKID>. Empty.
4 (invokedBy): create a KMS CMK, create an ECR repository with encryptionConfiguration {encryptionType: KMS, kmsKey: <arn>}, then LookupEvents for ecr.amazonaws.com and kms.amazonaws.com records and kms:ListGrants on the key. No records.
5 (EventBridge): create a logging trail, create two EventBridge rules (State ENABLED and State ENABLED_WITH_ALL_CLOUDTRAIL_MANAGEMENT_EVENTS) on source aws.sqs / detail-type "AWS API Call via CloudTrail" with SQS targets, call sqs:ListMessageMoveTasks then sqs:TagQueue, and drain the target queues. Nothing delivered.
6 (global IAM): iam:CreateRole from a non-us-east-1 client, then LookupEvents in us-east-1 for the role name, and in the caller's region for a regional control call. The us-east-1 lookup is empty.
7 (lambda): create a function, AddPermission once (succeeds), AddPermission again with the same StatementId (expect ResourceConflictException), then LookupEvents for eventSource lambda.amazonaws.com / eventName AddPermission20150331v2. Empty.
8 (ReadWriteType): create a logging trail with a basic event selector ReadWriteType=WriteOnly and DataResource AWS::S3::Object value arn:aws:s3:::<bucket>/, PutObject then GetObject then PutObject again, and read the delivered log files; then widen to ReadWriteType=All and repeat the GetObject. No log files in either phase.
