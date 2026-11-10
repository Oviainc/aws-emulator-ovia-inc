// SPDX-License-Identifier: CC0-1.0

//! An object written into a deployed bucket really reaches the search domain the deployed
//! function indexes it into, and the document is then readable at the address the domain
//! PUBLISHES.
//!
//! The rule under test: **an object write a bucket's notification configuration names a
//! Lambda function for runs that function on the event, and every call the handler makes of
//! its own — an AWS SDK call and a request it SIGNS and sends to another service's data
//! plane alike — really happens against this emulator's own services, so the document the
//! handler indexes into an Amazon OpenSearch Service domain is readable through the address
//! that domain published, by any caller IAM allows.**
//!
//! What the deployment owes for that to be possible is the second rule here: **a resource
//! whose declaration reads an attribute of a resource that is still COMING UP is created with
//! the value its owning service publishes for that attribute once it is up** — a domain
//! publishes no endpoint until the cluster behind it is running, so a function declared with
//! that endpoint is created once the domain is, never with the value missing.
//!
//! Five crates have to agree for that to hold (CloudFormation deploys the estate, S3
//! evaluates the configuration and makes the invocation, Lambda runs the handler, IAM and STS
//! answer for the execution role the handler's own calls are signed as, and Amazon OpenSearch
//! Service serves the signed index and read), so the suite lives here.
//!
//! The rows differ along what each rule generalises over: for the pipeline, the key written
//! (and so the document id the handler derives) and the document body — including one whose
//! key the configuration's own suffix filter excludes, which must reach no function and index
//! nothing; for the deployment, which reader of the dependency's attribute carries it (a
//! property through `Fn::GetAtt`, the same attribute embedded by `Fn::Sub`, and a stack
//! output).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::stack_status;
use crate::query_wire::{form_encode, outputs_of};

/// The Region every request here is signed for — the one the shared stack reader
/// ([`stack_status`]) signs for, so the stack this suite creates is the stack it reports.
const REGION: &str = "us-east-1";

/// The `StackStatus` a deployed stack must reach.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The engine release the deployed domain runs.
const ENGINE_VERSION: &str = "OpenSearch_2.11";

/// How long this suite waits for work the clock owes — a stack's resources coming up, and the
/// asynchronous delivery of an object event — and how often it looks while waiting.
const WAIT: Duration = Duration::from_secs(90);
const LOOK_INTERVAL: Duration = Duration::from_millis(200);

/// How long a row that must see NOTHING keeps looking before concluding nothing came.
const QUIET_WINDOW: Duration = Duration::from_secs(10);

/// The handler the stack deploys: for each record it is handed it reads the object the event
/// named through the AWS SDK the runtime supplies, then SIGNS an index request of its own and
/// sends it to the domain endpoint its configuration carries.
const INDEXING_HANDLER: &str = r#"
import json
import os
import urllib.request

import boto3
import botocore.session
from botocore.auth import SigV4Auth
from botocore.awsrequest import AWSRequest


def _indexed(document_id, body):
    endpoint = os.environ["SEARCH_ENDPOINT"]
    index = os.environ["SEARCH_INDEX"]
    url = "https://" + endpoint + "/" + index + "/_doc/" + document_id
    signed = AWSRequest(
        method="PUT", url=url, data=body, headers={"Content-Type": "application/json"}
    )
    credentials = botocore.session.get_session().get_credentials()
    SigV4Auth(credentials, "es", os.environ["AWS_REGION"]).add_auth(signed)
    sending = urllib.request.Request(signed.url, data=body, method=signed.method)
    for name, value in signed.headers.items():
        sending.add_header(name, value)
    with urllib.request.urlopen(sending) as answer:
        return answer.status


def handler(event, context):
    s3 = boto3.client("s3")
    indexed = []
    for record in event["Records"]:
        bucket = record["s3"]["bucket"]["name"]
        key = record["s3"]["object"]["key"]
        body = s3.get_object(Bucket=bucket, Key=key)["Body"].read()
        document_id = key.replace("/", "_")
        indexed.append({"id": document_id, "status": _indexed(document_id, body)})
    return {"indexed": indexed}
"#;

/// The emulator as it is ASSEMBLED, on one engine: the handler's own calls are dispatched back
/// into this same router, so the domain it indexes into is the domain this suite reads.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what routes
/// the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One awsQuery call of `service`, answered with the HTTP status and the XML body.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Dispatch one request against S3's REST surface, path-style.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str, body: &str) -> (u16, String) {
    let host = format!("s3.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("authorization", authorization("s3"))
        .header("content-type", "application/octet-stream")
        .body(Bytes::from(body.to_owned()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).into_owned(),
    )
}

/// `PUT /<bucket>/<key>` — `PutObject`.
async fn put_object(router: &MultiServiceRouter, case: &str, bucket: &str, key: &str, body: &str) {
    let (status, answered) = s3(router, "PUT", &format!("/{bucket}/{key}"), body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutObject({bucket:?}, {key:?}) must succeed, got {status} with {answered}"
    );
}

/// One request to the address the domain PUBLISHES, dialled the way a client dials it: the
/// status and the parsed body.
async fn at_address(
    router: &MultiServiceRouter,
    address: &str,
    method: &str,
    path: &str,
) -> (u16, Value) {
    let (host, prefix) = match address.split_once('/') {
        Some((host, under)) => (host.to_string(), format!("/{under}")),
        None => (address.to_string(), String::new()),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{prefix}{path}"))
        .header("host", host)
        .header("authorization", authorization("es"))
        .header("content-type", "application/json")
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, body)
}

/// The trust policy the execution role carries, and the grants the handler's own calls need:
/// the object read, the signed index request, and the log it writes.
fn role_resource() -> Value {
    json!({
        "Type": "AWS::IAM::Role",
        "Properties": {
            "AssumeRolePolicyDocument": {
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": "lambda.amazonaws.com" },
                    "Action": "sts:AssumeRole",
                }],
            },
            "Policies": [{
                "PolicyName": "index-the-object",
                "PolicyDocument": {
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Action": ["s3:GetObject", "es:ESHttp*", "logs:*"],
                        "Resource": "*",
                    }],
                },
            }],
        },
    })
}

/// The whole pipeline as ONE stack: the search domain whose access policy names the indexer's
/// role, the indexing function configured with the address that domain publishes, the grant S3
/// needs to invoke it, and the landing bucket whose notification configuration names it.
fn pipeline_stack(bucket: &str, index: &str, suffix: Option<&str>) -> Value {
    let filter = suffix.map(|suffix| {
        json!({ "S3Key": { "Rules": [{ "Name": "suffix", "Value": suffix }] } })
    });
    let mut lambda_configuration = json!({
        "Event": "s3:ObjectCreated:*",
        "Function": { "Fn::GetAtt": ["Indexer", "Arn"] },
    });
    if let Some(filter) = filter {
        lambda_configuration["Filter"] = filter;
    }
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "IndexerRole": role_resource(),
            "SearchDomain": {
                "Type": "AWS::OpenSearchService::Domain",
                "Properties": {
                    "EngineVersion": ENGINE_VERSION,
                    "ClusterConfig": { "InstanceType": "t3.small.search", "InstanceCount": 1 },
                    "EBSOptions": { "EBSEnabled": true, "VolumeType": "gp3", "VolumeSize": 10 },
                    "AccessPolicies": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": { "AWS": { "Fn::GetAtt": ["IndexerRole", "Arn"] } },
                            "Action": "es:ESHttp*",
                            "Resource": "*",
                        }],
                    },
                },
            },
            "Indexer": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Role": { "Fn::GetAtt": ["IndexerRole", "Arn"] },
                    "Timeout": 30,
                    "Environment": {
                        "Variables": {
                            "SEARCH_ENDPOINT": { "Fn::GetAtt": ["SearchDomain", "DomainEndpoint"] },
                            // The SAME attribute read in the other form a declaration reads
                            // one in: embedded in a longer value by `Fn::Sub`.
                            "SEARCH_URL": {
                                "Fn::Sub": format!(
                                    "https://${{SearchDomain.DomainEndpoint}}/{index}"
                                ),
                            },
                            "SEARCH_INDEX": index,
                        },
                    },
                    "Code": { "ZipFile": INDEXING_HANDLER },
                },
            },
            "InvokePermission": {
                "Type": "AWS::Lambda::Permission",
                "Properties": {
                    "FunctionName": { "Fn::GetAtt": ["Indexer", "Arn"] },
                    "Action": "lambda:InvokeFunction",
                    "Principal": "s3.amazonaws.com",
                    "SourceArn": format!("arn:aws:s3:::{bucket}"),
                },
            },
            "LandingBucket": {
                "Type": "AWS::S3::Bucket",
                "DependsOn": "InvokePermission",
                "Properties": {
                    "BucketName": bucket,
                    "NotificationConfiguration": {
                        "LambdaConfigurations": [lambda_configuration],
                    },
                },
            },
        },
        "Outputs": {
            "DomainEndpoint": { "Value": { "Fn::GetAtt": ["SearchDomain", "DomainEndpoint"] } },
            "DomainName": { "Value": { "Ref": "SearchDomain" } },
            "FunctionName": { "Value": { "Ref": "Indexer" } },
        },
    })
}

/// Create `stack` from `template`, wait for `CREATE_COMPLETE` and answer its outputs.
async fn deployed(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    template: &str,
) -> BTreeMap<String, String> {
    let (status, body) = query(
        router,
        "cloudformation",
        "2010-05-15",
        "CreateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", template),
            ("Capabilities.member.1", "CAPABILITY_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateStack({stack:?}) must be accepted, got {status} with {body}"
    );
    let looks = WAIT.as_millis() / LOOK_INTERVAL.as_millis();
    let mut reported = stack_status(router, stack).await;
    for _ in 0..looks {
        if reported.0 == CREATE_COMPLETE {
            break;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
        reported = stack_status(router, stack).await;
    }
    assert_eq!(
        reported.0, CREATE_COMPLETE,
        "case {case:?}: the stack must reach {CREATE_COMPLETE}, got {:?} with {}",
        reported.0, reported.1
    );
    outputs_of(&reported.1)
}

/// Poll the signed read of one document at `address` until it answers 200, and answer the last
/// answer it gave.
async fn awaited_document(
    router: &MultiServiceRouter,
    address: &str,
    index: &str,
    document_id: &str,
    expected: bool,
) -> (u16, Value) {
    let path = format!("/{index}/_doc/{document_id}");
    let wait = if expected { WAIT } else { QUIET_WINDOW };
    let looks = wait.as_millis() / LOOK_INTERVAL.as_millis();
    let mut answer = at_address(router, address, "GET", &path).await;
    for _ in 0..looks {
        if expected == (answer.0 == 200) {
            return answer;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
        answer = at_address(router, address, "GET", &path).await;
    }
    answer
}

/// One row: the key written, the document it carries, and whether the configuration's filter
/// admits that key.
struct Row {
    case: &'static str,
    key: &'static str,
    document: &'static str,
    indexed: bool,
}

/// The table, over the index the stack writes into and the suffix filter its configuration
/// carries.
const ROWS: &[Row] = &[
    Row {
        case: "an object under a prefix, indexed under the key's own id",
        key: "items/widget-1.json",
        document: r#"{"id":"widget-1","title":"a widget","qty":7}"#,
        indexed: true,
    },
    Row {
        case: "a second object of the same prefix",
        key: "items/widget-2.json",
        document: r#"{"id":"widget-2","title":"another widget","qty":0}"#,
        indexed: true,
    },
    Row {
        case: "an object at the root of the bucket",
        key: "feed.json",
        document: r#"{"id":"feed","title":"the feed"}"#,
        indexed: true,
    },
    Row {
        case: "a key the configuration's suffix filter excludes",
        key: "items/widget-3.csv",
        document: "id,title\nwidget-3,a widget\n",
        indexed: false,
    },
];

/// Every row: the write runs the deployed function, whose signed index request reaches the
/// deployed domain, so the document is readable at the address that domain published — and a
/// key the configuration excludes indexes nothing.
#[tokio::test]
async fn a_deployed_pipeline_indexes_the_written_object_into_its_domain() {
    let router = build_router();
    let stack = "search-pipeline-stack";
    let bucket = "search-pipeline-landing";
    let index = "documents";
    let outputs = deployed(
        &router,
        "the deployed pipeline",
        stack,
        &pipeline_stack(bucket, index, Some(".json")).to_string(),
    )
    .await;
    let address = outputs
        .get("DomainEndpoint")
        .cloned()
        .unwrap_or_else(|| panic!("the stack must output the address its domain publishes"));
    assert!(
        !address.is_empty(),
        "the stack must output a non-empty address for its domain, got {address:?}"
    );

    for row in ROWS {
        let case = row.case;
        put_object(&router, case, bucket, row.key, row.document).await;
        let document_id = row.key.replace('/', "_");
        let (status, answered) =
            awaited_document(&router, &address, index, &document_id, row.indexed).await;
        if !row.indexed {
            assert_ne!(
                status, 200,
                "case {case:?}: the filter excludes {:?}, so nothing may be indexed for it, got \
                 {status} with {answered}",
                row.key
            );
            continue;
        }
        assert_eq!(
            status, 200,
            "case {case:?}: the document the handler indexed must be readable at {address:?}, \
             got {status} with {answered}"
        );
        let expected: Value = serde_json::from_str(row.document)
            .unwrap_or_else(|error| panic!("case {case:?}: the row's document must be JSON: {error}"));
        assert_eq!(
            answered["_source"], expected,
            "case {case:?}: expected the uploaded document as the indexed _source, got {answered}"
        );
    }
}

/// The `Endpoint` Amazon OpenSearch Service itself reports for `domain` — `DescribeDomain`,
/// `GET /2021-01-01/opensearch/domain/{DomainName}` on that service's own wire.
async fn reported_endpoint(router: &MultiServiceRouter, domain: &str) -> String {
    let host = format!("es.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("GET")
        .uri(format!(
            "https://{host}/2021-01-01/opensearch/domain/{domain}"
        ))
        .header("host", host)
        .header("authorization", authorization("es"))
        .header("content-type", "application/json")
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let body: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    body["DomainStatus"]["Endpoint"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("DescribeDomain({domain:?}) must report the address it publishes, got {body}")
        })
        .to_string()
}

/// The environment variables AWS Lambda holds for `function` — `GetFunctionConfiguration`,
/// `GET /2015-03-31/functions/{FunctionName}/configuration`, which is where a client reads
/// what the deployed function was configured with.
async fn function_environment(
    router: &MultiServiceRouter,
    function: &str,
) -> BTreeMap<String, String> {
    let host = format!("lambda.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("GET")
        .uri(format!(
            "https://{host}/2015-03-31/functions/{function}/configuration"
        ))
        .header("host", host)
        .header("authorization", authorization("lambda"))
        .header("content-type", "application/json")
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let body: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    let variables = &body["Environment"]["Variables"];
    let Some(map) = variables.as_object() else {
        panic!("GetFunctionConfiguration({function:?}) must report its environment, got {body}");
    };
    map.iter()
        .map(|(name, value)| {
            (
                name.clone(),
                value.as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// RULE (the half the deployment owes): a resource whose declaration reads an attribute of a
/// resource that is still COMING UP is created with the value the owning service publishes
/// for it once it is up — whichever form the declaration reads that attribute in — never with
/// the attribute missing because the dependency had not finished.
///
/// The rows are the readers of one such attribute: a property read through `Fn::GetAtt`, the
/// same attribute embedded in a longer value by `Fn::Sub`, and a stack `Output`. Each expected
/// value is computed from what Amazon OpenSearch Service itself reports for the domain, so no
/// row carries a literal address.
#[tokio::test]
async fn a_dependent_is_created_with_what_its_still_coming_up_dependency_publishes() {
    let case = "a function declared with the endpoint of a domain of its own stack";
    let router = build_router();
    let index = "documents";
    let outputs = deployed(
        &router,
        case,
        "search-attribute-stack",
        &pipeline_stack("search-attribute-landing", index, None).to_string(),
    )
    .await;
    let domain = outputs
        .get("DomainName")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the stack must output its domain's name"));
    let function = outputs
        .get("FunctionName")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the stack must output its function's name"));
    let published = reported_endpoint(&router, &domain).await;
    assert!(
        !published.is_empty(),
        "case {case:?}: the domain must publish an address once it is up, got {published:?}"
    );

    let variables = function_environment(&router, &function).await;
    // (which reader, the value it must carry)
    let readers: [(&str, String, String); 3] = [
        (
            "the property that reads the attribute through Fn::GetAtt",
            variables.get("SEARCH_ENDPOINT").cloned().unwrap_or_default(),
            published.clone(),
        ),
        (
            "the property that embeds the same attribute with Fn::Sub",
            variables.get("SEARCH_URL").cloned().unwrap_or_default(),
            format!("https://{published}/{index}"),
        ),
        (
            "the stack output over the same attribute",
            outputs.get("DomainEndpoint").cloned().unwrap_or_default(),
            published.clone(),
        ),
    ];
    for (reader, carried, expected) in readers {
        assert_eq!(
            carried, expected,
            "case {case:?}: {reader} must carry what the owning service publishes, got \
             {carried:?} with the environment {variables:?}"
        );
    }
}
