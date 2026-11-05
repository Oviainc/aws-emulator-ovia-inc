// SPDX-License-Identifier: CC0-1.0

//! The services a service-last-accessed report covers, and the wording of a read asked
//! for a namespace none of them is published under.
//!
//! One concern, held in one place: the catalog [`REPORTED_SERVICES`], the row type
//! [`ReportedService`], the lookup over it ([`reported_service`]) and the refusal wording
//! for a namespace it does not carry ([`unsupported_service_namespace`]).

#[allow(unused_imports)]
use super::*;

/// One service a service-last-accessed report covers: the namespace a policy names that
/// service's actions under, and the service's own name.
#[derive(Debug, Clone, Copy)]
pub struct ReportedService {
    /// "The namespace of the service in which access was attempted"
    /// (`ServiceLastAccessed$ServiceNamespace`): the service prefix an action element
    /// spells before the separator.
    pub service_namespace: &'static str,
    /// "The name of the service in which access was attempted"
    /// (`ServiceLastAccessed$ServiceName`): the service's published name.
    pub service_name: &'static str,
}

impl ReportedService {
    /// One catalog row: the service's published prefix, and the name it is published
    /// under. A constructor rather than a struct literal because the catalog is long
    /// enough that the field names would hide the data.
    const fn new(service_namespace: &'static str, service_name: &'static str) -> Self {
        Self {
            service_namespace,
            service_name,
        }
    }
}

/// The services a service-last-accessed report reports on, ascending by namespace.
///
/// SOURCE: the Service Authorization Reference ("Actions, resources, and condition keys for
/// Amazon Web Services services"), which `ServiceLastAccessed$ServiceNamespace`'s own
/// documentation points at to learn a service's namespace — "choose the name of the service
/// to view details for that service. In the first paragraph, find the service prefix" — so
/// each row is one service of that reference: its prefix, and the name it is published
/// under.
///
/// The reference's own breadth, not a shortlist: a last-accessed report is what a
/// least-privilege review is built on, so the services it enumerates have to be the
/// services an account's principals can actually be granted — every one a permissions
/// policy can name an action of — and a report short of them reads as a principal that
/// reaches a handful of services. A namespace this catalog does not carry is a service no
/// entry is reported for rather than one reported without a name.
///
/// Ascending by namespace, because "by default, the list is sorted by service namespace"
/// (`GetServiceLastAccessedDetails`), so the catalog's own order IS the reported order,
/// and a report of it runs past one page of the default `MaxItems`
/// ([`DEFAULT_MAX_ITEMS`]) — which is why the reads of it page.
pub const REPORTED_SERVICES: &[ReportedService] = &[
    ReportedService::new("a4b", "Alexa for Business"),
    ReportedService::new("access-analyzer", "AWS IAM Access Analyzer"),
    ReportedService::new("account", "AWS Account Management"),
    ReportedService::new("acm", "AWS Certificate Manager"),
    ReportedService::new("acm-pca", "AWS Private Certificate Authority"),
    ReportedService::new("airflow", "Amazon Managed Workflows for Apache Airflow"),
    ReportedService::new("amplify", "AWS Amplify"),
    ReportedService::new("amplifybackend", "AWS Amplify Admin"),
    ReportedService::new("amplifyuibuilder", "AWS Amplify UI Builder"),
    ReportedService::new("apigateway", "Amazon API Gateway"),
    ReportedService::new("appconfig", "AWS AppConfig"),
    ReportedService::new("appflow", "Amazon AppFlow"),
    ReportedService::new("appintegrations", "Amazon AppIntegrations"),
    ReportedService::new("application-autoscaling", "Application Auto Scaling"),
    ReportedService::new("application-cost-profiler", "AWS Application Cost Profiler Service"),
    ReportedService::new("applicationinsights", "CloudWatch Application Insights"),
    ReportedService::new("appmesh", "AWS App Mesh"),
    ReportedService::new("apprunner", "AWS App Runner"),
    ReportedService::new("appstream", "Amazon AppStream 2.0"),
    ReportedService::new("appsync", "AWS AppSync"),
    ReportedService::new("aps", "Amazon Managed Service for Prometheus"),
    ReportedService::new("arsenal", "Application Discovery Arsenal"),
    ReportedService::new("artifact", "AWS Artifact"),
    ReportedService::new("athena", "Amazon Athena"),
    ReportedService::new("auditmanager", "AWS Audit Manager"),
    ReportedService::new("autoscaling", "Amazon EC2 Auto Scaling"),
    ReportedService::new("autoscaling-plans", "AWS Auto Scaling"),
    ReportedService::new("backup", "AWS Backup"),
    ReportedService::new("backup-gateway", "AWS Backup Gateway"),
    ReportedService::new("batch", "AWS Batch"),
    ReportedService::new("bedrock", "Amazon Bedrock"),
    ReportedService::new("billing", "AWS Billing"),
    ReportedService::new("billingconductor", "AWS Billing Conductor"),
    ReportedService::new("braket", "Amazon Braket"),
    ReportedService::new("budgets", "AWS Budget Service"),
    ReportedService::new("cases", "Amazon Connect Cases"),
    ReportedService::new("cassandra", "Amazon Keyspaces"),
    ReportedService::new("ce", "AWS Cost Explorer Service"),
    ReportedService::new("chatbot", "AWS Chatbot"),
    ReportedService::new("chime", "Amazon Chime"),
    ReportedService::new("cleanrooms", "AWS Clean Rooms"),
    ReportedService::new("cloud9", "AWS Cloud9"),
    ReportedService::new("clouddirectory", "Amazon Cloud Directory"),
    ReportedService::new("cloudformation", "AWS CloudFormation"),
    ReportedService::new("cloudfront", "Amazon CloudFront"),
    ReportedService::new("cloudhsm", "AWS CloudHSM"),
    ReportedService::new("cloudsearch", "Amazon CloudSearch"),
    ReportedService::new("cloudshell", "AWS CloudShell"),
    ReportedService::new("cloudtrail", "AWS CloudTrail"),
    ReportedService::new("cloudwatch", "Amazon CloudWatch"),
    ReportedService::new("codeartifact", "AWS CodeArtifact"),
    ReportedService::new("codebuild", "AWS CodeBuild"),
    ReportedService::new("codecommit", "AWS CodeCommit"),
    ReportedService::new("codedeploy", "AWS CodeDeploy"),
    ReportedService::new("codeguru-profiler", "Amazon CodeGuru Profiler"),
    ReportedService::new("codeguru-reviewer", "Amazon CodeGuru Reviewer"),
    ReportedService::new("codepipeline", "AWS CodePipeline"),
    ReportedService::new("codestar-connections", "AWS CodeStar Connections"),
    ReportedService::new("codestar-notifications", "AWS CodeStar Notifications"),
    ReportedService::new("cognito-identity", "Amazon Cognito Identity"),
    ReportedService::new("cognito-idp", "Amazon Cognito User Pools"),
    ReportedService::new("cognito-sync", "Amazon Cognito Sync"),
    ReportedService::new("comprehend", "Amazon Comprehend"),
    ReportedService::new("comprehendmedical", "Amazon Comprehend Medical"),
    ReportedService::new("compute-optimizer", "AWS Compute Optimizer"),
    ReportedService::new("config", "AWS Config"),
    ReportedService::new("connect", "Amazon Connect"),
    ReportedService::new("controltower", "AWS Control Tower"),
    ReportedService::new("cur", "AWS Cost and Usage Report"),
    ReportedService::new("databrew", "AWS Glue DataBrew"),
    ReportedService::new("dataexchange", "AWS Data Exchange"),
    ReportedService::new("datapipeline", "AWS Data Pipeline"),
    ReportedService::new("datasync", "AWS DataSync"),
    ReportedService::new("dax", "Amazon DynamoDB Accelerator (DAX)"),
    ReportedService::new("detective", "Amazon Detective"),
    ReportedService::new("devicefarm", "AWS Device Farm"),
    ReportedService::new("devops-guru", "Amazon DevOps Guru"),
    ReportedService::new("directconnect", "AWS Direct Connect"),
    ReportedService::new("discovery", "AWS Application Discovery Service"),
    ReportedService::new("dlm", "Amazon Data Lifecycle Manager"),
    ReportedService::new("dms", "AWS Database Migration Service"),
    ReportedService::new("drs", "AWS Elastic Disaster Recovery"),
    ReportedService::new("ds", "AWS Directory Service"),
    ReportedService::new("dynamodb", "Amazon DynamoDB"),
    ReportedService::new("ebs", "Amazon Elastic Block Store"),
    ReportedService::new("ec2", "Amazon Elastic Compute Cloud"),
    ReportedService::new("ec2-instance-connect", "Amazon EC2 Instance Connect"),
    ReportedService::new("ec2messages", "Amazon Message Delivery Service"),
    ReportedService::new("ecr", "Amazon Elastic Container Registry"),
    ReportedService::new("ecr-public", "Amazon Elastic Container Registry Public"),
    ReportedService::new("ecs", "Amazon Elastic Container Service"),
    ReportedService::new("eks", "Amazon Elastic Kubernetes Service"),
    ReportedService::new("elasticache", "Amazon ElastiCache"),
    ReportedService::new("elasticbeanstalk", "AWS Elastic Beanstalk"),
    ReportedService::new("elasticfilesystem", "Amazon Elastic File System"),
    ReportedService::new("elasticloadbalancing", "Elastic Load Balancing"),
    ReportedService::new("elasticmapreduce", "Amazon EMR"),
    ReportedService::new("elastictranscoder", "Amazon Elastic Transcoder"),
    ReportedService::new("emr-containers", "Amazon EMR on EKS"),
    ReportedService::new("emr-serverless", "Amazon EMR Serverless"),
    ReportedService::new("es", "Amazon OpenSearch Service"),
    ReportedService::new("events", "Amazon EventBridge"),
    ReportedService::new("evidently", "Amazon CloudWatch Evidently"),
    ReportedService::new("execute-api", "Amazon API Gateway"),
    ReportedService::new("firehose", "Amazon Data Firehose"),
    ReportedService::new("fis", "AWS Fault Injection Simulator"),
    ReportedService::new("fms", "AWS Firewall Manager"),
    ReportedService::new("forecast", "Amazon Forecast"),
    ReportedService::new("frauddetector", "Amazon Fraud Detector"),
    ReportedService::new("fsx", "Amazon FSx"),
    ReportedService::new("gamelift", "Amazon GameLift"),
    ReportedService::new("geo", "Amazon Location Service"),
    ReportedService::new("glacier", "Amazon S3 Glacier"),
    ReportedService::new("globalaccelerator", "AWS Global Accelerator"),
    ReportedService::new("glue", "AWS Glue"),
    ReportedService::new("grafana", "Amazon Managed Grafana"),
    ReportedService::new("greengrass", "AWS IoT Greengrass"),
    ReportedService::new("groundstation", "AWS Ground Station"),
    ReportedService::new("guardduty", "Amazon GuardDuty"),
    ReportedService::new("health", "AWS Health APIs and Notifications"),
    ReportedService::new("healthlake", "AWS HealthLake"),
    ReportedService::new("iam", "AWS Identity and Access Management"),
    ReportedService::new("identitystore", "AWS Identity Store"),
    ReportedService::new("imagebuilder", "EC2 Image Builder"),
    ReportedService::new("inspector", "Amazon Inspector"),
    ReportedService::new("inspector2", "Amazon Inspector2"),
    ReportedService::new("internetmonitor", "Amazon CloudWatch Internet Monitor"),
    ReportedService::new("iot", "AWS IoT"),
    ReportedService::new("iotanalytics", "AWS IoT Analytics"),
    ReportedService::new("iotdeviceadvisor", "AWS IoT Core Device Advisor"),
    ReportedService::new("iotevents", "AWS IoT Events"),
    ReportedService::new("iotfleethub", "AWS IoT Fleet Hub for Device Management"),
    ReportedService::new("iotfleetwise", "AWS IoT FleetWise"),
    ReportedService::new("iotsitewise", "AWS IoT SiteWise"),
    ReportedService::new("iottwinmaker", "AWS IoT TwinMaker"),
    ReportedService::new("iotwireless", "AWS IoT Core for LoRaWAN"),
    ReportedService::new("ivs", "Amazon Interactive Video Service"),
    ReportedService::new("kafka", "Amazon Managed Streaming for Apache Kafka"),
    ReportedService::new("kafkaconnect", "Amazon Managed Streaming for Kafka Connect"),
    ReportedService::new("kendra", "Amazon Kendra"),
    ReportedService::new("kinesis", "Amazon Kinesis"),
    ReportedService::new("kinesisanalytics", "Amazon Kinesis Analytics"),
    ReportedService::new("kinesisvideo", "Amazon Kinesis Video Streams"),
    ReportedService::new("kms", "AWS Key Management Service"),
    ReportedService::new("lakeformation", "AWS Lake Formation"),
    ReportedService::new("lambda", "AWS Lambda"),
    ReportedService::new("launchwizard", "AWS Launch Wizard"),
    ReportedService::new("lex", "Amazon Lex"),
    ReportedService::new("license-manager", "AWS License Manager"),
    ReportedService::new("lightsail", "Amazon Lightsail"),
    ReportedService::new("logs", "Amazon CloudWatch Logs"),
    ReportedService::new("lookoutequipment", "Amazon Lookout for Equipment"),
    ReportedService::new("lookoutmetrics", "Amazon Lookout for Metrics"),
    ReportedService::new("lookoutvision", "Amazon Lookout for Vision"),
    ReportedService::new("m2", "AWS Mainframe Modernization Service"),
    ReportedService::new("machinelearning", "Amazon Machine Learning"),
    ReportedService::new("macie2", "Amazon Macie"),
    ReportedService::new("managedblockchain", "Amazon Managed Blockchain"),
    ReportedService::new("mediaconnect", "AWS Elemental MediaConnect"),
    ReportedService::new("mediaconvert", "AWS Elemental MediaConvert"),
    ReportedService::new("medialive", "AWS Elemental MediaLive"),
    ReportedService::new("mediapackage", "AWS Elemental MediaPackage"),
    ReportedService::new("mediapackage-vod", "AWS Elemental MediaPackage VOD"),
    ReportedService::new("mediastore", "AWS Elemental MediaStore"),
    ReportedService::new("mediatailor", "AWS Elemental MediaTailor"),
    ReportedService::new("memorydb", "Amazon MemoryDB"),
    ReportedService::new("mgh", "AWS Migration Hub"),
    ReportedService::new("mgn", "AWS Application Migration Service"),
    ReportedService::new("mobiletargeting", "Amazon Pinpoint"),
    ReportedService::new("monitron", "Amazon Monitron"),
    ReportedService::new("mq", "Amazon MQ"),
    ReportedService::new("neptune-db", "Amazon Neptune"),
    ReportedService::new("network-firewall", "AWS Network Firewall"),
    ReportedService::new("networkmanager", "AWS Network Manager"),
    ReportedService::new("nimble", "Amazon Nimble Studio"),
    ReportedService::new("oam", "Amazon CloudWatch Observability Access Manager"),
    ReportedService::new("omics", "Amazon Omics"),
    ReportedService::new("opsworks", "AWS OpsWorks"),
    ReportedService::new("organizations", "AWS Organizations"),
    ReportedService::new("osis", "Amazon OpenSearch Ingestion"),
    ReportedService::new("outposts", "AWS Outposts"),
    ReportedService::new("panorama", "AWS Panorama"),
    ReportedService::new("personalize", "Amazon Personalize"),
    ReportedService::new("pi", "AWS Performance Insights"),
    ReportedService::new("pipes", "Amazon EventBridge Pipes"),
    ReportedService::new("polly", "Amazon Polly"),
    ReportedService::new("pricing", "AWS Price List"),
    ReportedService::new("profile", "Amazon Connect Customer Profiles"),
    ReportedService::new("proton", "AWS Proton"),
    ReportedService::new("qldb", "Amazon QLDB"),
    ReportedService::new("quicksight", "Amazon QuickSight"),
    ReportedService::new("ram", "AWS Resource Access Manager"),
    ReportedService::new("rbin", "Amazon Recycle Bin"),
    ReportedService::new("rds", "Amazon Relational Database Service"),
    ReportedService::new("rds-data", "Amazon RDS Data API"),
    ReportedService::new("redshift", "Amazon Redshift"),
    ReportedService::new("redshift-data", "Amazon Redshift Data API"),
    ReportedService::new("redshift-serverless", "Amazon Redshift Serverless"),
    ReportedService::new("rekognition", "Amazon Rekognition"),
    ReportedService::new("resiliencehub", "AWS Resilience Hub"),
    ReportedService::new("resource-explorer-2", "AWS Resource Explorer"),
    ReportedService::new("resource-groups", "AWS Resource Groups"),
    ReportedService::new("robomaker", "AWS RoboMaker"),
    ReportedService::new("route53", "Amazon Route 53"),
    ReportedService::new("route53-recovery-control-config", "Amazon Route 53 Recovery Controls"),
    ReportedService::new("route53domains", "Amazon Route 53 Domains"),
    ReportedService::new("route53resolver", "Amazon Route 53 Resolver"),
    ReportedService::new("rum", "Amazon CloudWatch RUM"),
    ReportedService::new("s3", "Amazon Simple Storage Service"),
    ReportedService::new("s3-object-lambda", "Amazon S3 Object Lambda"),
    ReportedService::new("s3express", "Amazon S3 Express"),
    ReportedService::new("s3outposts", "Amazon S3 on Outposts"),
    ReportedService::new("sagemaker", "Amazon SageMaker"),
    ReportedService::new("savingsplans", "AWS Savings Plans"),
    ReportedService::new("scheduler", "Amazon EventBridge Scheduler"),
    ReportedService::new("schemas", "Amazon EventBridge Schemas"),
    ReportedService::new("sdb", "Amazon SimpleDB"),
    ReportedService::new("secretsmanager", "AWS Secrets Manager"),
    ReportedService::new("securityhub", "AWS Security Hub"),
    ReportedService::new("serverlessrepo", "AWS Serverless Application Repository"),
    ReportedService::new("servicecatalog", "AWS Service Catalog"),
    ReportedService::new("servicediscovery", "AWS Cloud Map"),
    ReportedService::new("servicequotas", "Service Quotas"),
    ReportedService::new("ses", "Amazon Simple Email Service"),
    ReportedService::new("shield", "AWS Shield"),
    ReportedService::new("signer", "AWS Signer"),
    ReportedService::new("simspaceweaver", "AWS SimSpace Weaver"),
    ReportedService::new("sms", "AWS Server Migration Service"),
    ReportedService::new("sms-voice", "Amazon Pinpoint SMS and Voice Service"),
    ReportedService::new("snow-device-management", "AWS Snow Device Management"),
    ReportedService::new("snowball", "AWS Snowball"),
    ReportedService::new("sns", "Amazon Simple Notification Service"),
    ReportedService::new("sqs", "Amazon Simple Queue Service"),
    ReportedService::new("ssm", "AWS Systems Manager"),
    ReportedService::new("ssm-contacts", "AWS Systems Manager Incident Manager Contacts"),
    ReportedService::new("ssm-incidents", "AWS Systems Manager Incident Manager"),
    ReportedService::new("sso", "AWS IAM Identity Center"),
    ReportedService::new("sso-directory", "AWS IAM Identity Center Directory"),
    ReportedService::new("states", "AWS Step Functions"),
    ReportedService::new("storagegateway", "AWS Storage Gateway"),
    ReportedService::new("sts", "AWS Security Token Service"),
    ReportedService::new("support", "AWS Support"),
    ReportedService::new("swf", "Amazon Simple Workflow Service"),
    ReportedService::new("synthetics", "Amazon CloudWatch Synthetics"),
    ReportedService::new("tag", "Amazon Resource Group Tagging API"),
    ReportedService::new("textract", "Amazon Textract"),
    ReportedService::new("timestream", "Amazon Timestream"),
    ReportedService::new("transcribe", "Amazon Transcribe"),
    ReportedService::new("transfer", "AWS Transfer Family"),
    ReportedService::new("translate", "Amazon Translate"),
    ReportedService::new("trustedadvisor", "AWS Trusted Advisor"),
    ReportedService::new("voiceid", "Amazon Connect Voice ID"),
    ReportedService::new("vpc-lattice", "Amazon VPC Lattice"),
    ReportedService::new("waf", "AWS WAF"),
    ReportedService::new("waf-regional", "AWS WAF Regional"),
    ReportedService::new("wafv2", "AWS WAF V2"),
    ReportedService::new("wellarchitected", "AWS Well-Architected Tool"),
    ReportedService::new("wisdom", "Amazon Connect Wisdom"),
    ReportedService::new("workdocs", "Amazon WorkDocs"),
    ReportedService::new("worklink", "Amazon WorkLink"),
    ReportedService::new("workmail", "Amazon WorkMail"),
    ReportedService::new("workspaces", "Amazon WorkSpaces"),
    ReportedService::new("workspaces-web", "Amazon WorkSpaces Web"),
    ReportedService::new("xray", "AWS X-Ray"),
];

/// How many attempts to reach a service this account's records hold: the count
/// `ServiceLastAccessed$TotalAuthenticatedEntities` reports, "the total number of
/// authenticated principals (root user, IAM users, or IAM roles) that have attempted to
/// access the service".
///
/// Nothing in this emulator records a principal reaching a service — no operation writes
/// such a record and none reads one — so the count is the same for every service a report
/// covers, and that count is zero. It is NOT the absence of a count: recorded AWS reports
/// the number for every service of a report, including the ones nobody has reached, which
/// is how a least-privilege consumer tells "nobody used this" from "nothing is known".
pub const RECORDED_SERVICE_ACCESS_ATTEMPTS: i32 = 0;

/// The catalog entry for `service_namespace`, or `None` when [`REPORTED_SERVICES`] carries
/// no service under that namespace.
///
/// A service prefix is published in lower case and an action element may spell it in any
/// (the comparison [`grants_service_access`] makes is case-insensitive too), so the lookup
/// folds case: two spellings of one namespace name one service.
pub fn reported_service(service_namespace: &str) -> Option<&'static ReportedService> {
    REPORTED_SERVICES
        .iter()
        .find(|service| service.service_namespace.eq_ignore_ascii_case(service_namespace))
}

/// The message a read asked for ONE service's last accessed data under a namespace no
/// service of [`REPORTED_SERVICES`] is published under is refused with — the
/// `InvalidInputException` message, whose fixed text is `Service namepace ` and ` is either
/// invalid or unsupported.` (IAM's own spelling of the word) around the namespace the
/// request supplied.
///
/// The one wording for it, so every read taking a single `ServiceNamespace` to name the
/// service it reports on describes an unusable namespace the same way.
pub fn unsupported_service_namespace(service_namespace: &str) -> String {
    format!("Service namepace {service_namespace} is either invalid or unsupported.")
}
