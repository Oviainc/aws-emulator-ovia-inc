Issue-Source: trace-sweep-divergence
Issue-Trace: service-quotas-catalog-list-aws-default-service-quotas-amplifyuibuilder
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: service-quotas-catalog-list-aws-default-service-quotas-amplifyuibuilder service-quotas-catalog-list-aws-default-service-quotas-discovery service-quotas-catalog-list-aws-default-service-quotas-ivschat service-quotas-catalog-list-aws-default-service-quotas-aoss service-quotas-catalog-list-aws-default-service-quotas-panorama service-quotas-catalog-list-aws-default-service-quotas-application-signals service-quotas-catalog-list-aws-default-service-quotas-macie2 service-quotas-catalog-list-aws-default-service-quotas-neptune-graph service-quotas-catalog-list-aws-default-service-quotas-repostspace service-quotas-catalog-list-aws-default-service-quotas-appstudio service-quotas-catalog-list-aws-default-service-quotas-axis service-quotas-catalog-list-aws-default-service-quotas-bedrock-websearch service-quotas-catalog-list-aws-default-service-quotas-connect-campaigns service-quotas-catalog-list-aws-default-service-quotas-dlm service-quotas-catalog-list-aws-default-service-quotas-eks-mcp service-quotas-catalog-list-aws-default-service-quotas-evs service-quotas-catalog-list-aws-default-service-quotas-outposts service-quotas-catalog-list-aws-default-service-quotas-rbin service-quotas-catalog-list-aws-default-service-quotas-resource-explorer-2 service-quotas-catalog-list-aws-default-service-quotas-rum service-quotas-catalog-list-aws-default-service-quotas-s3-outposts service-quotas-catalog-list-aws-default-service-quotas-s3files service-quotas-catalog-list-aws-default-service-quotas-shield service-quotas-catalog-list-aws-default-service-quotas-sms service-quotas-catalog-list-aws-default-service-quotas-snowball service-quotas-catalog-list-aws-default-service-quotas-vmimportexport service-quotas-catalog-list-aws-default-service-quotas-workspaces-instances service-quotas-catalog-list-aws-default-service-quotas-auditmanager service-quotas-catalog-list-aws-default-service-quotas-cognito-sync service-quotas-catalog-list-aws-default-service-quotas-devicefarm service-quotas-catalog-list-aws-default-service-quotas-docdb-elastic service-quotas-catalog-list-aws-default-service-quotas-payment-cryptography service-quotas-catalog-list-aws-default-service-quotas-qbusiness service-quotas-catalog-list-aws-default-service-quotas-route53-recovery-control-config service-quotas-catalog-list-aws-default-service-quotas-schemas service-quotas-catalog-list-aws-default-service-quotas-ssm-guiconnect service-quotas-catalog-list-aws-default-service-quotas-autoscaling-plans service-quotas-catalog-list-aws-default-service-quotas-controltower service-quotas-catalog-list-aws-default-service-quotas-datasync service-quotas-catalog-list-aws-default-service-quotas-internetmonitor service-quotas-catalog-list-aws-default-service-quotas-kafkaconnect service-quotas-catalog-list-aws-default-service-quotas-migrationhuborchestrator service-quotas-catalog-list-aws-default-service-quotas-networkmonitor service-quotas-catalog-list-aws-default-service-quotas-securityagent service-quotas-catalog-list-aws-default-service-quotas-serverlessrepo service-quotas-catalog-list-aws-default-service-quotas-ssm-sap service-quotas-catalog-list-aws-default-service-quotas-support service-quotas-catalog-list-aws-default-service-quotas-translate service-quotas-catalog-list-aws-default-service-quotas-aws-marketplace service-quotas-catalog-list-aws-default-service-quotas-drs service-quotas-catalog-list-aws-default-service-quotas-evidently service-quotas-catalog-list-aws-default-service-quotas-voiceid service-quotas-catalog-list-aws-default-service-quotas-aws-mcp service-quotas-catalog-list-aws-default-service-quotas-datapipeline service-quotas-catalog-list-aws-default-service-quotas-entityresolution service-quotas-catalog-list-aws-default-service-quotas-rtbfabric service-quotas-catalog-list-aws-default-service-quotas-social-messaging service-quotas-catalog-list-aws-default-service-quotas-supportauthz service-quotas-catalog-list-aws-default-service-quotas-b2bi service-quotas-catalog-list-aws-default-service-quotas-chime
Issue-Shape-Id: com.amazonaws.servicequotas#ServiceQuotasV20190624
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-05F6A9EF"]
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-19B5E505"]
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-CD5D2786"]
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="L-E15C983E"]
Issue-Key: service-quotas|ListAWSDefaultServiceQuotas|2xx->2xx|$.Quotas[QuotaCode="ProfilesPerAccount"]
Issue-Related: 20261030T072417Z-service-quotas-listawsdefaultservicequotas-omits-most-default-quotas-for-access-analyzer.md
Issue-Commit: e635fc8db7925c597029a243d97ecda6419e6d1a

# ListAWSDefaultServiceQuotas omits most default quotas for amplifyuibuilder, aoss, application-signals and seven more service codes

> _Note: this issue was identified against the code base at commit `e635fc8db7925c597029a243d97ecda6419e6d1a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAWSDefaultServiceQuotas

## Description
Second bundle of the same defect family: the emulator's Service Quotas default-quota catalog is a hand-seeded stub, so ListAWSDefaultServiceQuotas answers 200 with an empty or near-empty Quotas list for most ServiceCodes, and for one ServiceCode it serves an entry AWS never returns. TEN net-new causes, one per ServiceCode.

Smithy: ListAWSDefaultServiceQuotas -> ListAWSDefaultServiceQuotasResponse { Quotas: list<ServiceQuota>, NextToken }; ServiceQuota carries ServiceCode, ServiceName, QuotaArn, QuotaCode, QuotaName, Value, Unit, Adjustable, GlobalQuota and the optional Period / UsageMetric blocks. The model does not enumerate quota codes, so catalog content is recorded behavior only. Every cause below answers 200 on both sides; the divergence is entirely in the body. AWS quota codes have the form L-XXXXXXXX and each entry's QuotaArn follows arn:aws:servicequotas:<region>::<serviceCode>/<quotaCode>.

Cause 1 of 10 - ServiceCode amplifyuibuilder (same cluster also covers discovery and ivschat).
AWS: returns the Amplify UI Builder default set (ServiceName 'Amplify UI Builder'), eight entries including L-D500C0BA (Theme size, Unit Kilobytes, Value 350), L-A25B72F2 (Form size), L-4FE3AB63 (Component size), L-E4AD9560 (Views per app, Value 1000) and L-429DD3BA (Themes per app).
Emulator: Quotas is empty for this ServiceCode - $.Quotas[0]..$.Quotas[7] all missing - and the same empty catalog is answered for ServiceCodes discovery and ivschat.
Why: a successful but empty catalog tells a client the service has no quotas, so no size or per-app limit can be discovered or requested.
Fix: the service-quotas default-quota catalog seed read by the ListAWSDefaultServiceQuotas handler.
Traces: service-quotas-catalog-list-aws-default-service-quotas-amplifyuibuilder#task#1 (cluster also covers -discovery#task#1 and -ivschat#task#1).

Cause 2 of 10 - ServiceCode aoss (also panorama in the same cluster).
AWS: returns the Amazon OpenSearch Serverless default set, 21 entries including L-2005AF7E (Encryption policy size per policy, Unit Bytes, Value 10240), L-877395DB (Network policy size per policy, Unit Bytes), L-1A85D401 (Collections per Collection Group (Classic)), L-50FA809B (Default indexing MAX OCU setting) and L-FBAA6E20 (Default indexing MAX Collection Group OCU setting).
Emulator: Quotas empty - $.Quotas[0]..$.Quotas[20] missing - likewise for panorama.
Why: OCU-capacity and policy-size limits are undiscoverable.
Fix: the service-quotas default-quota catalog seed.
Traces: service-quotas-catalog-list-aws-default-service-quotas-aoss#task#1 (cluster also covers -panorama#task#1).

Cause 3 of 10 - ServiceCode application-signals (also macie2, neptune-graph, repostspace).
AWS: returns the CloudWatch Application Signals default set (ServiceName 'Amazon CloudWatch Application Signals'), 25 entries including L-EB837E0B (Rate of StartDiscovery requests, Period {SECOND,1}, UsageMetric CallCount/AWS Usage, Value 5), L-D898488F (Rate of ListAuditFindings requests, Value 1) and L-A3B40D72 (Rate of ListServiceDependencies requests).
Emulator: Quotas empty - $.Quotas[0]..$.Quotas[24] missing - likewise for macie2, neptune-graph and repostspace.
Why: per-API throttle rates and their UsageMetric dimensions never reach the client.
Fix: the service-quotas default-quota catalog seed.
Traces: service-quotas-catalog-list-aws-default-service-quotas-application-signals#task#1 (cluster also covers -macie2#task#1, -neptune-graph#task#1, -repostspace#task#1).

Cause 4 of 10 - ServiceCode appstudio (cluster of 18 ServiceCodes).
AWS: returns the AWS App Studio default set with L-B07DC829 (Applications, Adjustable false, Value 20) and L-45B0D852 (Published applications, Value 6), ServiceName 'AWS App Studio'.
Emulator: Quotas empty - $.Quotas[0] and $.Quotas[1] missing - and the same empty catalog is answered for the 17 other ServiceCodes in this cluster (axis, bedrock-websearch, connect-campaigns, dlm, eks-mcp, evs, outposts, rbin, resource-explorer-2, rum, s3-outposts, s3files, shield, sms, snowball, vmimportexport, workspaces-instances).
Why: eighteen ServiceCodes report no default quotas at all.
Fix: the service-quotas default-quota catalog seed.
Traces: service-quotas-catalog-list-aws-default-service-quotas-appstudio#task#1 (cluster covers 18 fixtures).

Cause 5 of 10 - ServiceCode auditmanager (cluster of 9 ServiceCodes).
AWS: returns the AWS Audit Manager default set including L-0255B75F (Custom controls, Adjustable true, Value 500), L-BEA222D4 (Accounts in scope across all assessments, Value 250), L-8935A6F1 (Custom frameworks, Value 100), L-724DD74D (Controls per framework, Value 1400) and L-92B50F18 (Running assessments).
Emulator: Quotas empty - $.Quotas[0]..$.Quotas[4] missing - same for cognito-sync, devicefarm, docdb-elastic, payment-cryptography, qbusiness, route53-recovery-control-config, schemas and ssm-guiconnect.
Why: assessment and framework limits are invisible.
Fix: the service-quotas default-quota catalog seed.
Traces: service-quotas-catalog-list-aws-default-service-quotas-auditmanager#task#1 (cluster covers 9 fixtures).

Cause 6 of 10 - ServiceCode autoscaling-plans (cluster of 12 ServiceCodes).
AWS: returns L-BD401546 (Scaling plans, Adjustable true, Value 100), L-6538FA5E (Target tracking configurations per scaling instruction, Value 10) and L-7FAA513E (Scaling instructions per scaling plan, Value 500), ServiceName 'AWS Auto Scaling Plans'.
Emulator: Quotas empty - $.Quotas[0]..$.Quotas[2] missing - same for controltower, datasync, internetmonitor, kafkaconnect, migrationhuborchestrator, networkmonitor, securityagent, serverlessrepo, ssm-sap, support and translate.
Why: twelve more ServiceCodes report an empty catalog.
Fix: the service-quotas default-quota catalog seed.
Traces: service-quotas-catalog-list-aws-default-service-quotas-autoscaling-plans#task#1 (cluster covers 12 fixtures).

Cause 7 of 10 - ServiceCode aws-marketplace (also drs, evidently, voiceid).
AWS: returns the AWS Marketplace default set, nine entries including L-CD4DC1E4 ('[AWS Marketplace Discovery] GetOffer throttle quota', Value 5), L-96311249 (SearchListings throttle quota, Value 10), L-68F80CEA (GetProduct throttle quota), L-DF8B8977 (ListFulfillmentOptions throttle quota) and the GetOfferTerms throttle quota.
Emulator: Quotas empty - $.Quotas[0]..$.Quotas[8] missing - same for drs, evidently and voiceid.
Why: Marketplace Discovery throttle quotas are undiscoverable.
Fix: the service-quotas default-quota catalog seed.
Traces: service-quotas-catalog-list-aws-default-service-quotas-aws-marketplace#task#1 (cluster also covers -drs#task#1, -evidently#task#1, -voiceid#task#1).

Cause 8 of 10 - ServiceCode aws-mcp (also datapipeline, entityresolution, rtbfabric, social-messaging, supportauthz).
AWS: returns the AWS MCP default set, 13 entries including L-7A518ECF (Rate of retrieve_skill tool requests, Unit Count/Second, Value 10), L-E5DA7401 (Concurrent sessions per account, Period {MINUTE,5}, UsageMetric AccountSessionCount/ResourceCount, Value 180), L-24E93970 (Rate of call_aws tool requests) and L-A2044E93 (Rate of search_documentation tool requests).
Emulator: Quotas empty - $.Quotas[0]..$.Quotas[12] missing - same for the five other ServiceCodes in the cluster.
Why: per-tool rate limits and session caps cannot be read.
Fix: the service-quotas default-quota catalog seed.
Traces: service-quotas-catalog-list-aws-default-service-quotas-aws-mcp#task#1 (cluster covers 6 fixtures).

Cause 9 of 10 - ServiceCode b2bi (omissions AND a fabricated entry with the wrong ServiceName).
AWS: returns the B2B Data Interchange default set with ServiceName 'B2B Data Interchange' and codes L-E15C983E (Transformers, Value 500), L-CD5D2786 (Partnerships, Value 700), L-19B5E505 (Capabilities, Value 100) and L-05F6A9EF (Profiles, Value 5).
Emulator: all four L- codes are missing, and it serves an extra entry AWS never returns - QuotaCode 'ProfilesPerAccount', QuotaName 'Customer profiles per account', ServiceName 'AWS B2B Data Interchange' (the real catalog says 'B2B Data Interchange') and a QuotaArn ending /ProfilesPerAccount.
Why: a client that reads the emulator's catalog and feeds 'ProfilesPerAccount' back into GetAWSDefaultServiceQuota or RequestServiceQuotaIncrease is using a code AWS rejects, and the ServiceName it displays is wrong.
Fix: the service-quotas default-quota catalog seed - remove the invented row, add the real L- codes.
Traces: service-quotas-catalog-list-aws-default-service-quotas-b2bi#task#1.

Cause 10 of 10 - ServiceCode chime.
AWS: returns the Amazon Chime default set, 60 entries including L-F147F728 (Amazon Chime SDK meetings - All meeting management API rate in requests per second, Value 10), L-FE9F896D (CreateMeeting API burst rate, Value 20), L-398B2BD6 (DeleteAttendee API burst rate, Value 20) and L-B6F2C623 (SIP trunking and voice - calls per second (CPS), Adjustable true, Value 1).
Emulator: Quotas empty - $.Quotas[0]..$.Quotas[59] all missing.
Why: the largest catalog in this bundle is served as an empty list.
Fix: the service-quotas default-quota catalog seed.
Traces: service-quotas-catalog-list-aws-default-service-quotas-chime#task#1.

Note: the open issues 20261029T203936Z-service-quotas-listawsdefaultservicequotas-default-catalog-omits-quotas-* and 20261030T005329Z-service-quotas-defaults-for-memorydb-mgn-monitoring-mq.md cover different ServiceCodes; the replay reported owned_by empty for every cluster bundled here.

## Evidence
Every cause: expected_status 200, actual_status 200, verdict mismatch, divergence_key service=service-quotas|ListAWSDefaultServiceQuotas|status=200->200.

1) amplifyuibuilder (affected_count 3) - $.Quotas[0]..[7] missing; expected $.Quotas[0] = {Adjustable False, Description 'The maximum size (in KB) of the theme.', GlobalQuota False, QuotaArn 'arn:aws:servicequotas:us-west-2::amplifyuibuilder/L-D500C0BA', QuotaCode 'L-D500C0BA', QuotaName 'Theme size', ServiceCode 'amplifyuibuilder', ServiceName 'Amplify UI Builder', Unit 'Kilobytes', Value 350.0}.
2) aoss (affected_count 2) - $.Quotas[0]..[20] missing; expected L-2005AF7E / L-877395DB / L-1A85D401 / L-50FA809B / L-FBAA6E20 ...
3) application-signals (affected_count 4) - $.Quotas[0]..[24] missing; expected L-EB837E0B with Period {SECOND,1} and UsageMetric {MetricNamespace 'AWS/Usage', MetricName 'CallCount', MetricStatisticRecommendation 'Sum', MetricDimensions {Service 'CloudWatch Application Signals', Resource 'StartDiscovery', Type 'API', Class 'None'}}.
4) appstudio (affected_count 18) - $.Quotas[0],[1] missing; expected L-B07DC829 (Value 20.0) and L-45B0D852 (Value 6.0).
5) auditmanager (affected_count 9) - $.Quotas[0]..[4] missing; expected L-0255B75F 500.0, L-BEA222D4 250.0, L-8935A6F1 100.0, L-724DD74D 1400.0, L-92B50F18.
6) autoscaling-plans (affected_count 12) - $.Quotas[0]..[2] missing; expected L-BD401546 100.0, L-6538FA5E 10.0, L-7FAA513E 500.0.
7) aws-marketplace (affected_count 4) - $.Quotas[0]..[8] missing; expected L-CD4DC1E4 5.0, L-96311249 10.0, L-68F80CEA 5.0, L-DF8B8977 5.0.
8) aws-mcp (affected_count 6) - $.Quotas[0]..[12] missing; expected L-7A518ECF (Unit 'Count/Second', 10.0), L-E5DA7401 (Period {MINUTE,5}, 180.0), L-24E93970, L-A2044E93.
9) b2bi - missing L-E15C983E, L-CD5D2786, L-19B5E505, L-05F6A9EF; EXTRA $.Quotas[QuotaCode="ProfilesPerAccount"] = {ServiceCode 'b2bi', ServiceName 'AWS B2B Data Interchange', QuotaArn '...::b2bi/ProfilesPerAccount', QuotaCode 'ProfilesPerAccount', QuotaName 'Customer profiles per account', Value 5.0, Unit 'None', Adjustable True, GlobalQuota False} where AWS returns nothing.
10) chime - $.Quotas[0]..[59] missing; expected L-F147F728 10.0, L-FE9F896D 20.0, L-398B2BD6 20.0, L-B6F2C623 1.0.

The us-west-2 region in the recorded QuotaArns is environment-specific; the requirement is the template arn:aws:servicequotas:<region>::<serviceCode>/<quotaCode> together with each entry's code, name, description, unit, value and adjustability.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay fixture service-quotas-catalog-list-aws-default-service-quotas-amplifyuibuilder, step task#1: ListAWSDefaultServiceQuotas answers 200 with an empty Quotas list where the recording has eight entries.
3. Repeat for ServiceCode aoss, application-signals, appstudio, auditmanager, autoscaling-plans, aws-marketplace, aws-mcp, b2bi and chime (fixtures service-quotas-catalog-list-aws-default-service-quotas-<serviceCode>, step task#1) for causes 2-10.
4. For cause 9 (b2bi) note additionally the emulator-only ProfilesPerAccount entry and its 'AWS B2B Data Interchange' ServiceName.
5. A fix must make all ten fixture steps replay clean, including the extra cluster members (discovery, ivschat, panorama, macie2, neptune-graph, repostspace, the 17 ServiceCodes travelling with appstudio, the 8 with auditmanager, the 11 with autoscaling-plans, drs/evidently/voiceid, and datapipeline/entityresolution/rtbfabric/social-messaging/supportauthz).
