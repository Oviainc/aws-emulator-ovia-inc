// SPDX-License-Identifier: CC0-1.0

//! `ListResources` end to end: the resources a Region's index holds, what a filter
//! prefix and a view's own filter string select out of them, how the match pages, and
//! the refusals a query answers.
//!
//! The rule under test: a Resource Explorer query reports the resources the index of the
//! view's Region holds — read from the records of the services that OWN them, so an
//! index is a live view over those services rather than a second copy of their
//! resources — each one projected with the ARN, account, Region, service,
//! `service:resourceType` key, CloudFormation type and last-reported instant that owner
//! holds for it; the result is the intersection of the caller's filter string with the
//! view's own; the filter string carries only filter prefixes, never free-form text; and
//! a query that cannot resolve a view is refused with 401 `UnauthorizedException`.
//!
//! The tables span what the rule generalises over: WHICH service owns the resource (an
//! SQS queue, a FIFO queue, an SNS topic, this service's own index and views), the
//! filter NAME (`service`, `resourcetype`, `region`, `arn`, `id`, `accountid`, `tag`),
//! the operator (negation, a `*` suffix wildcard), the Region the resource lives in, the
//! page size, and the form of a term that is not a filter prefix. The recorded case — a
//! `resourcetype:sqs:queue` view returning the queue and never the topic — is one row.
//!
//! Every call goes over the wire through the router the emulator serves in production,
//! with the owning services mounted on the SAME engine: an SQS queue is created by
//! `AmazonSQS.CreateQueue` and an SNS topic by the `CreateTopic` query action, so
//! nothing but the owning service's own persisted state puts a resource in the index.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use bytes::Bytes;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_resourceexplorer2_resourceexplorer as explorer;
use service_com_amazonaws_sns_amazonsimplenotificationservice as owner_sns;
use service_com_amazonaws_sqs_amazonsqs as owner_sqs;

/// The Region the fixture's index, views and most resources live in.
const REGION: &str = "us-east-1";

/// A second Region with an index and a view of its own: what proves a Region's index
/// holds only "the resources that are located in the same Region as the index".
const OTHER_REGION: &str = "eu-west-1";

/// The account segment every emulated ARN carries.
const ACCOUNT_ID: &str = engine::EMULATOR_ACCOUNT_ID;

/// The one property name `IncludedProperties` documents.
const TAGS_PROPERTY: &str = "tags";

/// What one Resource Explorer call answered.
struct Answer {
    status: u16,
    error: Option<String>,
    body: Value,
}

impl Answer {
    /// The body of a call that had to succeed.
    fn ok(&self, case: &str) -> &Value {
        assert_eq!(
            self.status, 200,
            "case {case:?}: expected HTTP 200, got {} ({:?}) with body {}",
            self.status, self.error, self.body
        );
        &self.body
    }

    /// Assert this call failed with `status` and the error code `code`.
    fn failed(&self, case: &str, status: u16, code: &str) {
        assert_eq!(
            (self.status, self.error.as_deref()),
            (status, Some(code)),
            "case {case:?}: expected HTTP {status} {code}, got {} ({:?}) with body {}",
            self.status,
            self.error,
            self.body
        );
    }

    /// The error's `Message` member, however the envelope cases it.
    fn error_message(&self) -> String {
        self.body
            .get("Message")
            .or_else(|| self.body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The `Resources` list of a successful answer.
    fn resources(&self, case: &str) -> Vec<Value> {
        let resources = self.ok(case).get("Resources").cloned().unwrap_or(Value::Null);
        resources
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected a Resources list, got {resources}"))
            .clone()
    }

    /// The ARNs of a successful answer, as a set — the order a page reports them in is
    /// not what any of these rows is about.
    fn arns(&self, case: &str) -> BTreeSet<String> {
        self.resources(case)
            .iter()
            .map(|resource| {
                resource["Arn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: a Resource with no Arn: {resource}"))
                    .to_string()
            })
            .collect()
    }
}

/// One emulator: Resource Explorer and the services that own the resources it indexes,
/// mounted on ONE shared engine — the arrangement `core/router` builds.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // These suites exercise handler behaviour, not SigV4: the `Authorization`
        // header below is present because it is what names the Region a call is made
        // in, not to be verified.
        router.set_request_authentication(false);
        router.register_service(explorer::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(owner_sqs::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(owner_sns::EmulatedService::new(engine));
        Self { router }
    }

    /// The SigV4 `Authorization` header of a call made in `region` for `signing_name`.
    fn authorization(region: &str, signing_name: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{region}/{signing_name}\
             /aws4_request, SignedHeaders=host, Signature=00"
        )
    }

    /// `POST /{operation}` with a JSON body — how this service's RPC-shaped routes are
    /// addressed.
    async fn explorer(&self, region: &str, operation: &str, body: Value) -> Answer {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!(
                "https://resource-explorer-2.{region}.amazonaws.com/{operation}"
            ))
            .header("content-type", "application/json")
            .header(
                "authorization",
                Self::authorization(region, "resource-explorer-2"),
            )
            .body(Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Answer {
            status,
            error,
            body,
        }
    }

    /// One Amazon SQS operation, which must answer 200: `POST /` with an
    /// `X-Amz-Target` and an awsJson body.
    async fn sqs(&self, region: &str, operation: &str, body: Value) -> Value {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("sqs.{region}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("AmazonSQS.{operation}"))
            .header("authorization", Self::authorization(region, "sqs"))
            .body(Bytes::from(body.to_string()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        assert_eq!(
            status, 200,
            "SQS {operation} must answer 200, got {status} with body {parsed}"
        );
        parsed
    }

    /// One Amazon SNS query action, which must answer 200, with its XML body.
    async fn sns(&self, region: &str, action: &str, params: &[(&str, &str)]) -> String {
        let mut body = format!("Action={action}&Version=2010-03-31");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&utf8_percent_encode(value, NON_ALPHANUMERIC).to_string());
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("https://sns.amazonaws.com/")
            .header("host", format!("sns.{region}.amazonaws.com"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", Self::authorization(region, "sns"))
            .body(Bytes::from(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let text = String::from_utf8_lossy(response.body()).to_string();
        assert_eq!(
            status, 200,
            "SNS {action} must answer 200, got {status} with body {text}"
        );
        text
    }

    /// Create an SQS queue through the owning service and read back the ARN it minted,
    /// with the queue's URL.
    async fn create_queue(
        &self,
        region: &str,
        name: &str,
        attributes: Value,
        tags: Value,
    ) -> (String, String) {
        let mut request = json!({ "QueueName": name });
        if attributes != Value::Null {
            request["Attributes"] = attributes;
        }
        if tags != Value::Null {
            request["tags"] = tags;
        }
        let created = self.sqs(region, "CreateQueue", request).await;
        let url = created["QueueUrl"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateQueue {name}: no QueueUrl in {created}"))
            .to_string();
        let read = self
            .sqs(
                region,
                "GetQueueAttributes",
                json!({ "QueueUrl": &url, "AttributeNames": ["QueueArn"] }),
            )
            .await;
        let arn = read["Attributes"]["QueueArn"]
            .as_str()
            .unwrap_or_else(|| panic!("GetQueueAttributes {name}: no QueueArn in {read}"))
            .to_string();
        (url, arn)
    }

    /// Create an SNS topic through the owning service and read back the ARN it minted.
    async fn create_topic(&self, region: &str, name: &str) -> String {
        let body = self.sns(region, "CreateTopic", &[("Name", name)]).await;
        xml_element(&body, "TopicArn")
            .unwrap_or_else(|| panic!("CreateTopic {name}: no TopicArn in {body}"))
    }

    /// `CreateIndex` in `region`, with the index ARN it answered with.
    async fn create_index(&self, region: &str) -> String {
        let answer = self
            .explorer(region, "CreateIndex", json!({ "ClientToken": region }))
            .await;
        answer.ok(&format!("CreateIndex in {region}"))["Arn"]
            .as_str()
            .expect("CreateIndex answers an Arn")
            .to_string()
    }

    /// `CreateView` in `region`, with the view ARN it answered with.
    async fn create_view(&self, region: &str, body: Value) -> String {
        let case = format!("CreateView in {region}: {body}");
        let answer = self.explorer(region, "CreateView", body).await;
        answer.ok(&case)["View"]["ViewArn"]
            .as_str()
            .expect("CreateView answers a View.ViewArn")
            .to_string()
    }

    /// `AssociateDefaultView`, which must succeed.
    async fn set_default_view(&self, region: &str, view_arn: &str) {
        self.explorer(
            region,
            "AssociateDefaultView",
            json!({ "ViewArn": view_arn }),
        )
        .await
        .ok(&format!("AssociateDefaultView in {region}"));
    }

    /// The ARN `GetDefaultView` reports for `region`, which must have one.
    async fn reported_default_view(&self, region: &str) -> String {
        let answer = self.explorer(region, "GetDefaultView", json!({})).await;
        answer.ok(&format!("GetDefaultView in {region}"))["ViewArn"]
            .as_str()
            .expect("a Region that was turned on has a default view")
            .to_string()
    }

    /// `ListResources` with the Region's default view and one filter string.
    async fn list(&self, region: &str, filter_string: &str) -> Answer {
        self.explorer(
            region,
            "ListResources",
            json!({ "Filters": { "FilterString": filter_string } }),
        )
        .await
    }

    /// `ListResources` through one named view and one filter string.
    async fn list_through(&self, region: &str, view_arn: &str, filter_string: &str) -> Answer {
        self.explorer(
            region,
            "ListResources",
            json!({ "ViewArn": view_arn, "Filters": { "FilterString": filter_string } }),
        )
        .await
    }
}

/// The text of the first `<tag>…</tag>` element of an awsQuery response.
fn xml_element(body: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body.find(&open)? + open.len();
    let rest = &body[start..];
    let end = rest.find(&close)?;
    Some(rest[..end].to_string())
}

/// A set of ARNs, written as the rows write them.
fn set(arns: &[&String]) -> BTreeSet<String> {
    arns.iter().map(|arn| (*arn).clone()).collect()
}

/// One resource of the fixture, with everything the index must report about it.
struct Indexed {
    case: &'static str,
    arn: String,
    /// `Resource$ResourceType`: the `service:resourceType` key.
    resource_type: &'static str,
    /// `Resource$Service`.
    service: &'static str,
    /// `Resource$CfnResourceType`.
    cfn_resource_type: &'static str,
}

/// The fixture: one index and one default view per Region, plus resources of every
/// owning service the index holds.
struct Fixture {
    emulator: Emulator,
    index: String,
    /// The view turning Resource Explorer on in the Region left there, which is one of
    /// the resources that Region's index holds.
    turn_on_view: String,
    /// The view the fixture created and made the Region's default: it includes the
    /// `tags` property, so the rows that filter on tags can be served.
    default_view: String,
    /// A standard queue carrying tags.
    queue: String,
    queue_url: String,
    /// A FIFO queue, whose name carries the `.fifo` suffix: the same resource type under
    /// a different name shape.
    fifo_queue: String,
    /// An SNS topic indexed in the same index as the queues.
    topic: String,
    /// A queue of the OTHER Region, which this Region's index must never report.
    foreign_queue: String,
    /// The other Region's index and views.
    foreign_index: String,
    foreign_turn_on_view: String,
    foreign_view: String,
}

impl Fixture {
    async fn new() -> Self {
        let emulator = Emulator::new();
        let index = emulator.create_index(REGION).await;
        let turn_on_view = emulator.reported_default_view(REGION).await;
        let foreign_index = emulator.create_index(OTHER_REGION).await;
        let foreign_turn_on_view = emulator.reported_default_view(OTHER_REGION).await;
        let default_view = emulator
            .create_view(
                REGION,
                json!({
                    "ViewName": "everything",
                    "IncludedProperties": [{ "Name": TAGS_PROPERTY }],
                }),
            )
            .await;
        emulator.set_default_view(REGION, &default_view).await;
        let foreign_view = emulator
            .create_view(OTHER_REGION, json!({ "ViewName": "everything-abroad" }))
            .await;
        emulator
            .set_default_view(OTHER_REGION, &foreign_view)
            .await;

        let (queue_url, queue) = emulator
            .create_queue(
                REGION,
                "orders",
                Value::Null,
                json!({ "stage": "prod", "team": "payments" }),
            )
            .await;
        let (_, fifo_queue) = emulator
            .create_queue(
                REGION,
                "payments.fifo",
                json!({ "FifoQueue": "true" }),
                Value::Null,
            )
            .await;
        let topic = emulator.create_topic(REGION, "alerts").await;
        let (_, foreign_queue) = emulator
            .create_queue(OTHER_REGION, "orders-abroad", Value::Null, Value::Null)
            .await;

        Self {
            emulator,
            index,
            turn_on_view,
            foreign_turn_on_view,
            default_view,
            queue,
            queue_url,
            fifo_queue,
            topic,
            foreign_queue,
            foreign_index,
            foreign_view,
        }
    }

    /// Every resource the fixture's Region indexes, with the projection the index must
    /// report for it.
    ///
    /// The `service:resourceType` keys and CloudFormation type names are those of the
    /// Resource Explorer supported-resource-type list and the CloudFormation registry,
    /// and are the ones `crate::state::owners` records.
    fn indexed(&self) -> Vec<Indexed> {
        vec![
            Indexed {
                case: "a standard SQS queue",
                arn: self.queue.clone(),
                resource_type: "sqs:queue",
                service: "sqs",
                cfn_resource_type: "AWS::SQS::Queue",
            },
            Indexed {
                case: "a FIFO SQS queue",
                arn: self.fifo_queue.clone(),
                resource_type: "sqs:queue",
                service: "sqs",
                cfn_resource_type: "AWS::SQS::Queue",
            },
            Indexed {
                case: "an SNS topic",
                arn: self.topic.clone(),
                resource_type: "sns:topic",
                service: "sns",
                cfn_resource_type: "AWS::SNS::Topic",
            },
            Indexed {
                case: "the Region's own index",
                arn: self.index.clone(),
                resource_type: "resource-explorer-2:index",
                service: "resource-explorer-2",
                cfn_resource_type: "AWS::ResourceExplorer2::Index",
            },
            Indexed {
                case: "the Region's own view",
                arn: self.default_view.clone(),
                resource_type: "resource-explorer-2:view",
                service: "resource-explorer-2",
                cfn_resource_type: "AWS::ResourceExplorer2::View",
            },
            Indexed {
                case: "the view the Region was turned on with",
                arn: self.turn_on_view.clone(),
                resource_type: "resource-explorer-2:view",
                service: "resource-explorer-2",
                cfn_resource_type: "AWS::ResourceExplorer2::View",
            },
        ]
    }

    /// Every ARN the fixture's Region indexes.
    fn every_arn(&self) -> BTreeSet<String> {
        self.indexed()
            .into_iter()
            .map(|resource| resource.arn)
            .collect()
    }
}

/// The index reports every resource of the Region, whichever service owns it, projected
/// from that owner's own record: the ARN it minted, the account and Region that ARN
/// carries, the `service:resourceType` key and CloudFormation type of its kind, and a
/// `LastReportedAt` instant. A resource of ANOTHER Region is never in it.
#[tokio::test]
async fn the_index_reports_every_owning_services_resources_in_its_region() {
    let fixture = Fixture::new().await;
    let answer = fixture.emulator.list(REGION, "").await;
    let case = "the default view, no filter";
    assert_eq!(
        answer.ok(case)["ViewArn"].as_str(),
        Some(fixture.default_view.as_str()),
        "case {case:?}: the answer names the view it used, got {}",
        answer.body
    );
    assert_eq!(
        answer.arns(case),
        fixture.every_arn(),
        "case {case:?}: expected every resource of {REGION}, got {}",
        answer.body
    );

    let reported = answer.resources(case);
    for resource in fixture.indexed() {
        let found = reported
            .iter()
            .find(|candidate| candidate["Arn"] == json!(resource.arn))
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: {} is not in the answer: {}",
                    resource.case, resource.arn, answer.body
                )
            });
        for (member, want) in [
            ("ResourceType", resource.resource_type),
            ("Service", resource.service),
            ("CfnResourceType", resource.cfn_resource_type),
            ("Region", REGION),
            ("OwningAccountId", ACCOUNT_ID),
        ] {
            assert_eq!(
                found[member],
                json!(want),
                "case {:?}: expected {member} {want:?}, got {found}",
                resource.case
            );
        }
        assert!(
            found["LastReportedAt"].is_number() || found["LastReportedAt"].is_string(),
            "case {:?}: expected a LastReportedAt instant, got {found}",
            resource.case
        );
    }

    // The other Region's index holds its own resources and nothing of this one's.
    let case = "the other Region's default view";
    let abroad = fixture.emulator.list(OTHER_REGION, "").await;
    assert_eq!(
        abroad.arns(case),
        set(&[
            &fixture.foreign_queue,
            &fixture.foreign_index,
            &fixture.foreign_turn_on_view,
            &fixture.foreign_view
        ]),
        "case {case:?}: expected only {OTHER_REGION} resources, got {}",
        abroad.body
    );
}

/// A filter prefix selects out of the index by the field it names, with `-` negating a
/// term, `*` matching by prefix and several terms AND-ed. The rows differ along the
/// filter NAME, the operator and the number of resources selected.
#[tokio::test]
async fn filter_prefixes_select_resources_out_of_the_index() {
    let fixture = Fixture::new().await;
    let queue_id = "orders";

    let cases: Vec<(&str, String, BTreeSet<String>)> = vec![
        (
            "one service",
            "service:sqs".to_string(),
            set(&[&fixture.queue, &fixture.fifo_queue]),
        ),
        (
            "the other service",
            "service:sns".to_string(),
            set(&[&fixture.topic]),
        ),
        (
            "a qualified resource type",
            "resourcetype:sqs:queue".to_string(),
            set(&[&fixture.queue, &fixture.fifo_queue]),
        ),
        (
            "the other qualified resource type",
            "resourcetype:sns:topic".to_string(),
            set(&[&fixture.topic]),
        ),
        (
            "this service's own views",
            "resourcetype:resource-explorer-2:view".to_string(),
            set(&[&fixture.default_view, &fixture.turn_on_view]),
        ),
        (
            "the serving Region",
            format!("region:{REGION}"),
            fixture.every_arn(),
        ),
        (
            "a Region prefix wildcard",
            "region:us*".to_string(),
            fixture.every_arn(),
        ),
        (
            "a Region prefix nothing is in",
            "region:ap*".to_string(),
            BTreeSet::new(),
        ),
        (
            "one ARN",
            format!("arn:{}", fixture.queue),
            set(&[&fixture.queue]),
        ),
        (
            "one resource id",
            format!("id:{queue_id}"),
            set(&[&fixture.queue]),
        ),
        (
            "the owning account",
            format!("accountid:{ACCOUNT_ID}"),
            fixture.every_arn(),
        ),
        (
            "another account",
            "accountid:999999999999".to_string(),
            BTreeSet::new(),
        ),
        (
            "a negated service",
            "resourcetype:sqs:queue -id:orders".to_string(),
            set(&[&fixture.fifo_queue]),
        ),
        (
            "two filters AND-ed to nothing",
            "service:sqs service:sns".to_string(),
            BTreeSet::new(),
        ),
        (
            "a tag pair one queue carries",
            "tag:stage=prod".to_string(),
            set(&[&fixture.queue]),
        ),
        (
            "a tag key no resource carries",
            "tag.key:absent".to_string(),
            BTreeSet::new(),
        ),
        (
            "a global resource type, which no regional resource is",
            "globalresourcetype:sqs:queue".to_string(),
            BTreeSet::new(),
        ),
    ];

    for (case, filter_string, want) in cases {
        let answer = fixture.emulator.list(REGION, &filter_string).await;
        assert_eq!(
            answer.arns(case),
            want,
            "case {case:?}: expected {filter_string:?} to select {want:?}, got {}",
            answer.body
        );
    }
}

/// A view's own filter string SCOPES what the view can return, and the result is the
/// intersection of it with the caller's filter string: a view cut to one resource type
/// never reports a resource of another type indexed in the same index, and every
/// resource it does report carries that type's own `ResourceType` and `Service`.
///
/// The recorded case is the first row.
#[tokio::test]
async fn a_views_filter_string_scopes_what_it_can_return() {
    let fixture = Fixture::new().await;

    /// What a view reports with no filter string of the caller's own.
    enum Want {
        /// Exactly these ARNs.
        Exactly(BTreeSet<String>),
        /// Everything the Region indexes WHEN THE ROW RUNS, except these ARNs: each row
        /// creates a view, and a view is itself a resource of the index, so a row whose
        /// filter admits this service's own resources cannot name them up front.
        EverythingExcept(BTreeSet<String>),
    }

    struct Row {
        case: &'static str,
        view_name: &'static str,
        view_filter: &'static str,
        want: Want,
        /// The `ResourceType` and `Service` every resource it reports must carry, when
        /// the view is cut to exactly one type.
        want_type: Option<(&'static str, &'static str)>,
        /// Whether this view's own filter string admits the Region's index, which is
        /// what the caller's filter below is narrowed to.
        admits_index: bool,
    }

    let rows = vec![
        Row {
            case: "a view cut to one resource type",
            view_name: "queues-only",
            view_filter: "resourcetype:sqs:queue",
            want: Want::Exactly(set(&[&fixture.queue, &fixture.fifo_queue])),
            want_type: Some(("sqs:queue", "sqs")),
            admits_index: false,
        },
        Row {
            case: "a view cut to another service",
            view_name: "topics-only",
            view_filter: "service:sns",
            want: Want::Exactly(set(&[&fixture.topic])),
            want_type: Some(("sns:topic", "sns")),
            admits_index: false,
        },
        Row {
            case: "a view cut by a negated term",
            view_name: "nothing-of-sqs",
            view_filter: "-service:sqs",
            want: Want::EverythingExcept(set(&[&fixture.queue, &fixture.fifo_queue])),
            want_type: None,
            admits_index: true,
        },
        Row {
            case: "a view cut to one ARN",
            view_name: "one-topic",
            view_filter: "arn:PLACEHOLDER",
            want: Want::Exactly(set(&[&fixture.topic])),
            want_type: Some(("sns:topic", "sns")),
            admits_index: false,
        },
        Row {
            case: "a view cut to a Region nothing is in",
            view_name: "elsewhere",
            view_filter: "region:ap-southeast-2",
            want: Want::Exactly(BTreeSet::new()),
            want_type: None,
            admits_index: false,
        },
    ];

    for row in rows {
        let filter = row
            .view_filter
            .replace("PLACEHOLDER", fixture.topic.as_str());
        let view = fixture
            .emulator
            .create_view(
                REGION,
                json!({
                    "ViewName": row.view_name,
                    "Filters": { "FilterString": filter },
                }),
            )
            .await;

        let answer = fixture.emulator.list_through(REGION, &view, "").await;
        assert_eq!(
            answer.ok(row.case)["ViewArn"].as_str(),
            Some(view.as_str()),
            "case {:?}: the answer names the view it used, got {}",
            row.case,
            answer.body
        );
        let got = answer.arns(row.case);
        let want = match &row.want {
            Want::Exactly(arns) => arns.clone(),
            // What the Region indexes right now, which the views the earlier rows
            // created are part of, less the ARNs this row's filter excludes.
            Want::EverythingExcept(excluded) => {
                let case = format!("{}: everything the Region indexes", row.case);
                let every = fixture.emulator.list(REGION, "").await;
                every
                    .arns(&case)
                    .into_iter()
                    .filter(|arn| !excluded.contains(arn))
                    .collect()
            }
        };
        assert_eq!(
            got, want,
            "case {:?}: expected the view {filter:?} to return {want:?}, got {}",
            row.case, answer.body
        );
        if let Some((resource_type, service)) = row.want_type {
            assert!(
                !got.is_empty(),
                "case {:?}: the row is about the type of what it returns, so it must \
                 return something",
                row.case
            );
            for resource in answer.resources(row.case) {
                assert_eq!(
                    (
                        resource["ResourceType"].as_str(),
                        resource["Service"].as_str()
                    ),
                    (Some(resource_type), Some(service)),
                    "case {:?}: every resource of this view reports {resource_type}/{service}, \
                     got {resource}",
                    row.case
                );
            }
        }

        // The caller's own filter string intersects with the view's: a term the view
        // excludes leaves nothing, and one it allows narrows what it returns.
        let excluded = fixture
            .emulator
            .list_through(REGION, &view, "resourcetype:resource-explorer-2:index")
            .await;
        let case = format!("{}: a filter the view excludes", row.case);
        if row.admits_index {
            assert_eq!(
                excluded.arns(&case),
                set(&[&fixture.index]),
                "case {case:?}: expected only the index, got {}",
                excluded.body
            );
        } else {
            assert!(
                excluded.arns(&case).is_empty(),
                "case {case:?}: a view that cannot return the index must return nothing, \
                 got {}",
                excluded.body
            );
        }
    }
}

/// The filter string "supports only filter prefixes with optional operators. It doesn't
/// support free-form text", so a term that carries no documented filter prefix is
/// refused with 400 `ValidationException` naming the term — never matched as text, and
/// never a 501. The accepted rows show the refusal is about the PREFIX and not about the
/// value: every documented prefix is served, even one that selects nothing.
#[tokio::test]
async fn a_filter_string_carrying_free_form_text_is_refused() {
    let fixture = Fixture::new().await;

    // The rows differ along the FORM of the offending term: a bare word, a word beside
    // a well-formed filter, a negated bare word, a prefix that is not a filter name, and
    // one that is nearly a filter name.
    let refused = [
        ("a bare word", "orders"),
        ("a word beside a filter", "service:sqs orders"),
        ("a negated bare word", "-orders"),
        ("a prefix that names no filter", "name:orders"),
        ("a negated unknown prefix", "-queuename:orders"),
        ("a prefix nearly a filter name", "resourcetypes:sqs:queue"),
        ("a prefix nearly a tag filter name", "tag.name:stage"),
    ];
    for (case, filter_string) in refused {
        let answer = fixture.emulator.list(REGION, filter_string).await;
        answer.failed(case, 400, "ValidationException");
        let message = answer.error_message();
        assert!(
            message.contains("free-form text") && message.contains("Filters.FilterString"),
            "case {case:?}: the refusal names the member and the requirement, got {message:?}"
        );
        // `SearchFilter` is @sensitive, so the string the caller sent is not echoed.
        assert!(
            !message.contains(filter_string) || filter_string.is_empty(),
            "case {case:?}: a sensitive member's value must not be echoed, got {message:?}"
        );
    }

    // Every documented filter prefix is served. `tag` filters are included because the
    // fixture's default view includes the `tags` property.
    let accepted = [
        ("an empty filter string", ""),
        ("a service filter", "service:sqs"),
        ("a resource type filter", "resourcetype:sqs:queue"),
        ("a region filter", "region:us-east-1"),
        ("an arn filter", "arn:arn:aws:sqs:us-east-1:000000000000:ghost"),
        ("an id filter", "id:orders"),
        ("an account filter", "accountid:000000000000"),
        ("a global resource type filter", "globalresourcetype:ec2:vpc"),
        ("a tag filter", "tag:stage=prod"),
        ("a tag key filter", "tag.key:stage"),
        ("a tag value filter", "tag.value:prod"),
        ("a negated filter", "-service:sns"),
        ("a wildcard filter", "service:sq*"),
        ("filters AND-ed", "service:sqs region:us-east-1 -id:orders"),
    ];
    for (case, filter_string) in accepted {
        let answer = fixture.emulator.list(REGION, filter_string).await;
        answer.ok(case);
    }
}

/// The match pages by `MaxResults` with an OPAQUE `NextToken` that resumes where the
/// page stopped, the walk covering the whole match exactly once whatever the page size;
/// a token issued for one query cannot resume another; and a page of the operation's
/// 1,000-result limit carries no token at all, because that page already holds every
/// result there is.
#[tokio::test]
async fn the_match_pages_with_an_opaque_next_token() {
    let fixture = Fixture::new().await;
    let every = fixture.every_arn();

    for page_size in [1_usize, 2, 3, 5, 100] {
        let case = format!("pages of {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({
                "Filters": { "FilterString": "" },
                "MaxResults": page_size,
            });
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let answer = fixture
                .emulator
                .explorer(REGION, "ListResources", request)
                .await;
            let page = answer.arns(&case);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most MaxResults, got {}",
                answer.body
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= every.len() + 1,
                "case {case:?}: the walk must terminate, got {pages} pages"
            );
            match answer.ok(&case).get("NextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        let walked: BTreeSet<String> = seen.iter().cloned().collect();
        assert_eq!(
            walked, every,
            "case {case:?}: the walk covers the whole match, got {seen:?}"
        );
        assert_eq!(
            seen.len(),
            every.len(),
            "case {case:?}: no resource is reported twice, got {seen:?}"
        );
    }

    // A token is bound to the request it was issued for.
    let first = fixture
        .emulator
        .explorer(
            REGION,
            "ListResources",
            json!({ "Filters": { "FilterString": "" }, "MaxResults": 1 }),
        )
        .await;
    let token = first.ok("the first page")["NextToken"]
        .as_str()
        .expect("a first page of one carries a NextToken")
        .to_string();
    fixture
        .emulator
        .explorer(
            REGION,
            "ListResources",
            json!({
                "Filters": { "FilterString": "service:sqs" },
                "MaxResults": 1,
                "NextToken": token,
            }),
        )
        .await
        .failed("a token from another query", 400, "ValidationException");

    // "The ListResources operation does not generate a NextToken if you set MaxResults
    // to 1000": that page already carries every result the operation can return.
    let whole = fixture
        .emulator
        .explorer(
            REGION,
            "ListResources",
            json!({ "Filters": { "FilterString": "" }, "MaxResults": 1000 }),
        )
        .await;
    let case = "a page of the result limit";
    assert_eq!(
        whole.arns(case),
        every,
        "case {case:?}: expected the whole match, got {}",
        whole.body
    );
    assert_eq!(
        whole.ok(case).get("NextToken"),
        None,
        "case {case:?}: a page of 1000 issues no NextToken, got {}",
        whole.body
    );
}

/// A query must use a view, and both ways of failing to resolve one are the same
/// authorization failure: "if the Region either doesn't have a default view or if you
/// don't have permission to use the default view, then the operation fails with a 401
/// Unauthorized exception". The refusal carries nothing from the request.
#[tokio::test]
async fn a_query_that_cannot_resolve_a_view_is_unauthorized() {
    let fixture = Fixture::new().await;

    // A Region with an index of its own, cleared of the default view turning Resource
    // Explorer on there left it: a Region with no default view is a state it has to be
    // cleared into.
    let bare = "ap-southeast-2";
    fixture.emulator.create_index(bare).await;
    fixture
        .emulator
        .explorer(bare, "DisassociateDefaultView", json!({}))
        .await
        .ok("clear the Region's default view");

    let unresolvable: Vec<(&str, Value)> = vec![
        ("a Region with no default view", json!({})),
        (
            "a view ARN never minted",
            json!({
                "ViewArn": format!(
                    "arn:aws:resource-explorer-2:{bare}:{ACCOUNT_ID}:view/ghost/\
                     0f8c2a1e-5d3b-8a47-9c61-2b7e4d905a13"
                ),
            }),
        ),
        (
            "another account's view",
            json!({
                "ViewArn": format!(
                    "arn:aws:resource-explorer-2:{bare}:999999999999:view/other/\
                     5c2d8f31-7a46-8b92-ae03-1d49f6520b78"
                ),
            }),
        ),
        ("a bare name, not an ARN", json!({ "ViewArn": "not-an-arn" })),
    ];
    for (case, request) in unresolvable {
        let answer = fixture
            .emulator
            .explorer(bare, "ListResources", request.clone())
            .await;
        answer.failed(case, 401, "UnauthorizedException");
        assert_eq!(
            answer.error_message(),
            "Unauthorized",
            "case {case:?}: the refusal carries nothing from the request, got {}",
            answer.body
        );
    }
}

/// The index is a LIVE view over the owning services: a resource created there is in it
/// with no Resource Explorer call in between, a tag written there is the tag a `tag:`
/// filter matches and the one a view that includes the `tags` property reports, and a
/// resource deleted there leaves the index while everything beside it stays.
#[tokio::test]
async fn the_index_follows_the_owning_services_own_records() {
    let fixture = Fixture::new().await;
    let case = "a queue created through SQS";
    let (url, arn) = fixture
        .emulator
        .create_queue(REGION, "invoices", Value::Null, Value::Null)
        .await;
    let answer = fixture.emulator.list(REGION, "resourcetype:sqs:queue").await;
    assert!(
        answer.arns(case).contains(&arn),
        "case {case:?}: expected {arn} in the index, got {}",
        answer.body
    );

    // A tag written through the owning service is the resource's own tag.
    let case = "a tag written through SQS";
    fixture
        .emulator
        .sqs(
            REGION,
            "TagQueue",
            json!({ "QueueUrl": &url, "Tags": { "stage": "test" } }),
        )
        .await;
    let answer = fixture.emulator.list(REGION, "tag:stage=test").await;
    assert_eq!(
        answer.arns(case),
        set(&[&arn]),
        "case {case:?}: expected the tag filter to select {arn}, got {}",
        answer.body
    );
    let properties = answer.resources(case)[0]["Properties"].clone();
    assert_eq!(
        properties[0]["Name"], json!(TAGS_PROPERTY),
        "case {case:?}: a view that includes the tags property reports it, got {properties}"
    );
    assert_eq!(
        properties[0]["Data"]["stage"],
        json!("test"),
        "case {case:?}: the reported tags are the owner's own, got {properties}"
    );

    // A delete there removes it from the index; the queues beside it stay.
    let case = "a queue deleted through SQS";
    fixture
        .emulator
        .sqs(REGION, "DeleteQueue", json!({ "QueueUrl": &url }))
        .await;
    let answer = fixture.emulator.list(REGION, "resourcetype:sqs:queue").await;
    assert_eq!(
        answer.arns(case),
        set(&[&fixture.queue, &fixture.fifo_queue]),
        "case {case:?}: expected {arn} to leave the index, got {}",
        answer.body
    );

    // And the same for the topic, so the rule is per-owner rather than per-service.
    let case = "a topic deleted through SNS";
    fixture
        .emulator
        .sns(REGION, "DeleteTopic", &[("TopicArn", &fixture.topic)])
        .await;
    let answer = fixture.emulator.list(REGION, "").await;
    let remaining = answer.arns(case);
    assert!(
        !remaining.contains(&fixture.topic),
        "case {case:?}: expected the topic to leave the index, got {}",
        answer.body
    );
    assert!(
        remaining.contains(&fixture.queue) && remaining.contains(&fixture.index),
        "case {case:?}: everything beside it stays, got {}",
        answer.body
    );

    // Nothing above went through this service's store: the queue URL the fixture kept is
    // still the one SQS answers for its own queue.
    let read = fixture
        .emulator
        .sqs(
            REGION,
            "GetQueueAttributes",
            json!({ "QueueUrl": &fixture.queue_url, "AttributeNames": ["QueueArn"] }),
        )
        .await;
    assert_eq!(
        read["Attributes"]["QueueArn"],
        json!(fixture.queue),
        "the owning service still holds its own queue, got {read}"
    );
}

/// A tag filter reads a resource's TAGS, which a view carries only when it includes the
/// `tags` property: a view without it refuses a query that filters on tags, naming the
/// filter at fault, and reports no `Properties` at all.
#[tokio::test]
async fn a_tag_filter_needs_a_view_that_includes_the_tags_property() {
    let fixture = Fixture::new().await;
    let plain = fixture
        .emulator
        .create_view(REGION, json!({ "ViewName": "no-properties" }))
        .await;

    for (case, filter_string) in [
        ("the tag filter", "tag:stage=prod"),
        ("the tag key filter", "tag.key:stage"),
        ("the tag value filter", "tag.value:prod"),
    ] {
        let answer = fixture.emulator.list_through(REGION, &plain, filter_string).await;
        answer.failed(case, 400, "ValidationException");
        assert!(
            answer.error_message().contains("not included in the view"),
            "case {case:?}: the refusal names the view's missing property, got {:?}",
            answer.error_message()
        );
    }

    let case = "a view with no properties";
    let answer = fixture.emulator.list_through(REGION, &plain, "service:sqs").await;
    for resource in answer.resources(case) {
        assert_eq!(
            resource.get("Properties"),
            None,
            "case {case:?}: a view that does not include a property reports none, got {resource}"
        );
    }

    // The fixture's default view DOES include it, so the same filter is served there.
    let case = "a view that includes the tags property";
    let answer = fixture.emulator.list(REGION, "tag:stage=prod").await;
    assert_eq!(
        answer.arns(case),
        set(&[&fixture.queue]),
        "case {case:?}: expected the tagged queue, got {}",
        answer.body
    );
}

/// Every resource `ListResources` reports through one view is the resource `Search`
/// reports through the same view: there is ONE index and ONE projection of it, so the
/// two query operations cannot disagree about what the account holds.
#[tokio::test]
async fn list_resources_and_search_report_the_same_index() {
    let fixture = Fixture::new().await;
    for filter_string in ["", "service:sqs", "resourcetype:sns:topic", "region:us*"] {
        let listed = fixture.emulator.list(REGION, filter_string).await;
        let searched = fixture
            .emulator
            .explorer(
                REGION,
                "Search",
                json!({ "QueryString": filter_string, "MaxResults": 100 }),
            )
            .await;
        let case = format!("{filter_string:?} through both operations");
        assert_eq!(
            listed.arns(&case),
            searched.arns(&case),
            "case {case:?}: expected the same resources, got {} and {}",
            listed.body,
            searched.body
        );
    }
}

/// The `Resources` list is a `BTreeMap`-free projection of persisted state, so two reads
/// of an unchanged index answer identically — a query writes nothing.
#[tokio::test]
async fn a_query_changes_nothing_a_later_caller_sees() {
    let fixture = Fixture::new().await;
    let before = fixture.emulator.list(REGION, "").await;
    let case = "the first read";
    let first: BTreeMap<String, Value> = before
        .resources(case)
        .into_iter()
        .map(|resource| (resource["Arn"].as_str().unwrap_or_default().to_string(), resource))
        .collect();
    let after = fixture.emulator.list(REGION, "").await;
    let case = "the second read";
    let second: BTreeMap<String, Value> = after
        .resources(case)
        .into_iter()
        .map(|resource| (resource["Arn"].as_str().unwrap_or_default().to_string(), resource))
        .collect();
    assert_eq!(
        first, second,
        "case {case:?}: a read must leave the index exactly as it was, got {} then {}",
        before.body, after.body
    );
}
