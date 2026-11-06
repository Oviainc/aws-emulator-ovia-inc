// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `GetIndex` and of the three operations bound to
//! `/tags/{resourceArn}`: `TagResource`, `ListTagsForResource` and `UntagResource`.
//!
//! Every assertion here goes over the wire: the suite mounts this service on a
//! [`MultiServiceRouter`] over ONE shared engine and dispatches restJson1 requests at
//! it, so nothing is proved about a handler that is not also proved about the serialized
//! request and response an SDK sends and receives. That matters twice over here:
//! `GetIndex` has a `smithy.api#Unit` input and so carries no body at all, and the three
//! tagging operations address their resource through a non-greedy `@httpLabel` rather
//! than through a body member.
//!
//! The Region of a request is the Region of its SigV4 credential scope, which is why
//! every call carries an `Authorization` header: it is the only place the wire states
//! which Region "the Region in which you call this operation" means.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// Comfortably past `crate::state::INDEX_SETTLE_DELAY`, so an index left in `CREATING`
/// or `DELETING` is certain to have settled by the next call.
const PAST_SETTLE: Duration = Duration::from_millis(900);

/// One emulator: the service mounted on a router over a single shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

/// What one call answered: the HTTP status, the restJson1 error code (the
/// `x-amzn-errortype` header), and the decoded JSON body.
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

    /// The `Tags` member of a successful answer, as a sorted map so two tag sets compare
    /// by content rather than by JSON member order.
    fn tags(&self, case: &str) -> BTreeMap<String, String> {
        let tags = self.ok(case).get("Tags").cloned().unwrap_or(Value::Null);
        assert!(
            tags.is_object(),
            "case {case:?}: expected a Tags map, got {tags}"
        );
        tags.as_object()
            .expect("Tags is an object")
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value
                        .as_str()
                        .unwrap_or_else(|| panic!("case {case:?}: tag {key:?} is not a string"))
                        .to_string(),
                )
            })
            .collect()
    }
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_resourceexplorer2_resourceexplorer::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Dispatch `method path` as a client signed for `region` would.
    async fn send(&self, region: &str, method: &str, path: &str, body: Option<Value>) -> Answer {
        let payload = match body {
            Some(value) => serde_json::to_vec(&value).expect("the request body serializes"),
            None => Vec::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!(
                "https://resource-explorer-2.{region}.amazonaws.com{path}"
            ))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{region}\
                     /resource-explorer-2/aws4_request, SignedHeaders=host, Signature=00"
                ),
            )
            .body(Bytes::from(payload))
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

    /// `POST /{operation}` with a JSON body — how this service's RPC-shaped routes
    /// (`CreateIndex`, `GetIndex`, `CreateView`, …) are addressed.
    async fn call(&self, region: &str, operation: &str, body: Value) -> Answer {
        self.send(region, "POST", &format!("/{operation}"), Some(body))
            .await
    }

    /// `POST /GetIndex`, carrying NO body: the operation's input is `smithy.api#Unit`.
    async fn get_index(&self, region: &str) -> Answer {
        self.send(region, "POST", "/GetIndex", None).await
    }

    /// `POST /tags/{resourceArn}` — `TagResource`, with the map in the body.
    async fn tag(&self, region: &str, arn: &str, tags: &BTreeMap<String, String>) -> Answer {
        self.send(
            region,
            "POST",
            &format!("/tags/{}", label(arn)),
            Some(json!({ "Tags": tags })),
        )
        .await
    }

    /// `GET /tags/{resourceArn}` — `ListTagsForResource`.
    async fn list_tags(&self, region: &str, arn: &str) -> Answer {
        self.send(region, "GET", &format!("/tags/{}", label(arn)), None)
            .await
    }

    /// `DELETE /tags/{resourceArn}?tagKeys=…` — `UntagResource`. `tagKeys` is an
    /// `@httpQuery` list, so it travels as one query entry per key.
    async fn untag(&self, region: &str, arn: &str, keys: &[&str]) -> Answer {
        assert!(
            !keys.is_empty(),
            "tagKeys is @required, so a request must name at least one key"
        );
        let query = keys
            .iter()
            .map(|key| format!("tagKeys={}", label(key)))
            .collect::<Vec<_>>()
            .join("&");
        self.send(
            region,
            "DELETE",
            &format!("/tags/{}?{query}", label(arn)),
            None,
        )
        .await
    }
}

/// One URI path segment or query value, percent-encoded the way an SDK encodes a
/// non-greedy `@httpLabel` or a query entry: every byte outside the unreserved set
/// travels as `%XX`, so an ARN's `:` and `/` stay inside the one `{resourceArn}`
/// segment.
fn label(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

/// A tag map as the assertions compare it.
fn tag_map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// `CreateIndex` in `region`, asserting success; its index ARN and the `CreatedAt` it
/// reported, so a later `GetIndex` can be held to the same instant.
async fn create_index(
    emulator: &Emulator,
    region: &str,
    tags: &BTreeMap<String, String>,
) -> (String, Value) {
    let answer = emulator
        .call(
            region,
            "CreateIndex",
            json!({ "ClientToken": format!("index-{region}"), "Tags": tags }),
        )
        .await;
    let body = answer.ok(region);
    let arn = body["Arn"]
        .as_str()
        .expect("CreateIndex answers an Arn")
        .to_string();
    let created_at = body["CreatedAt"].clone();
    assert!(
        !created_at.is_null(),
        "case {region:?}: CreateIndex answers a CreatedAt, got {body}"
    );
    (arn, created_at)
}

/// `CreateView` in `region`, asserting success, and the view ARN it answered with.
async fn create_view(emulator: &Emulator, region: &str, name: &str) -> String {
    let answer = emulator
        .call(region, "CreateView", json!({ "ViewName": name }))
        .await;
    answer.ok(name)["View"]["ViewArn"]
        .as_str()
        .expect("CreateView answers a View.ViewArn")
        .to_string()
}

/// An ARN in this service's own namespace that names nothing: the account's own Region
/// and account, with a resource segment no `CreateIndex` or `CreateView` ever minted.
fn own_arn(region: &str, resource: &str) -> String {
    engine::emulator_arn("resource-explorer-2", region, resource)
}

/// `GetIndex` reports the Region's index for as long as the store holds it, through
/// every state of the index lifecycle, and the modelled 404 when the Region has none.
///
/// The rows span Regions so the answer is shown to be keyed on the request's Region
/// rather than on one value, and each Region is polled before it has an index, in
/// `CREATING`, in `ACTIVE`, in `DELETING` and after the deletion has settled.
#[tokio::test]
async fn get_index_reports_the_region_index_in_every_state_it_can_be_in() {
    let emulator = Emulator::new();
    let regions = ["us-east-1", "eu-west-1", "ap-southeast-2"];

    // A Region with no index: the modelled ResourceNotFoundException, not a 501 and not
    // an empty success.
    for region in regions {
        let answer = emulator.get_index(region).await;
        answer.failed(&format!("{region} before CreateIndex"), 404, "ResourceNotFoundException");
        assert!(
            answer.error_message().contains(region),
            "case {region:?}: the refusal must name the Region it found no index in, got {:?}",
            answer.error_message()
        );
    }

    // One Region at a time, so the answer for a Region that HAS an index is shown not to
    // leak into a Region that does not.
    let mut arns = Vec::new();
    for (created, region) in regions.iter().enumerate() {
        let tags = tag_map(&[("owner", "platform")]);
        let (arn, created_at) = create_index(&emulator, region, &tags).await;
        arns.push((arn.clone(), created_at.clone()));

        let answer = emulator.get_index(region).await;
        let case = format!("{region} just after CreateIndex");
        let body = answer.ok(&case);
        assert_eq!(
            body["Arn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: GetIndex must report the Region's own index ARN, got {body}"
        );
        assert_eq!(
            body["State"].as_str(),
            Some("CREATING"),
            "case {case:?}: an index that has not settled is still CREATING, got {body}"
        );
        assert_eq!(
            body["Type"].as_str(),
            Some("LOCAL"),
            "case {case:?}: CreateIndex creates only a local index, got {body}"
        );
        assert_eq!(
            body["CreatedAt"], created_at,
            "case {case:?}: GetIndex reports the instant CreateIndex recorded, got {body}"
        );
        assert_eq!(
            body["LastUpdatedAt"], created_at,
            "case {case:?}: an index that has not changed since it was created was last \
             updated then, got {body}"
        );
        assert_eq!(
            answer.tags(&case),
            tags,
            "case {case:?}: GetIndex reports the tags CreateIndex stored"
        );
        // No aggregator index exists, so neither replication member has anything to
        // report and both stay absent.
        assert!(
            body.get("ReplicatingFrom").is_none() && body.get("ReplicatingTo").is_none(),
            "case {case:?}: with no aggregator index neither replication member is \
             reported, got {body}"
        );

        // Every Region that has not been given an index yet still 404s.
        for later in regions.iter().skip(created + 1) {
            emulator.get_index(later).await.failed(
                &format!("{later} while {region} has an index"),
                404,
                "ResourceNotFoundException",
            );
        }
    }

    // The clock owes each index its ACTIVE state; nothing had to poll for that to be due.
    tokio::time::sleep(PAST_SETTLE).await;
    for (region, (_, created_at)) in regions.iter().zip(&arns) {
        let case = format!("{region} after the settle span");
        let answer = emulator.get_index(region).await;
        let body = answer.ok(&case);
        assert_eq!(
            body["State"].as_str(),
            Some("ACTIVE"),
            "case {case:?}: a settled index is ACTIVE"
        );
        assert_eq!(
            body["CreatedAt"], *created_at,
            "case {case:?}: the creation instant never changes, got {body}"
        );
        assert_ne!(
            body["LastUpdatedAt"], *created_at,
            "case {case:?}: coming up ACTIVE is an update to the index, got {body}"
        );
    }

    // DeleteIndex puts the index in DELETING, which GetIndex reports, and once that has
    // settled the Region holds no index again.
    for (region, (arn, created_at)) in regions.iter().zip(&arns) {
        let case = format!("{region} DeleteIndex");
        let deleted = emulator
            .call(region, "DeleteIndex", json!({ "Arn": arn }))
            .await;
        let deleted_at = deleted.ok(&case)["LastUpdatedAt"].clone();
        let answer = emulator.get_index(region).await;
        let body = answer.ok(&case);
        assert_eq!(
            body["State"].as_str(),
            Some("DELETING"),
            "case {case:?}: an index being torn down is reported DELETING"
        );
        assert_eq!(
            body["LastUpdatedAt"], deleted_at,
            "case {case:?}: GetIndex reports the instant DeleteIndex recorded, got {body}"
        );
        assert_eq!(
            body["CreatedAt"], *created_at,
            "case {case:?}: the creation instant never changes, got {body}"
        );
    }
    tokio::time::sleep(PAST_SETTLE).await;
    for region in regions {
        emulator.get_index(region).await.failed(
            &format!("{region} after the deletion settled"),
            404,
            "ResourceNotFoundException",
        );
    }
}

/// The input is `smithy.api#Unit`, so `GetIndex` answers the same whether the client
/// sends no body at all or an empty JSON object — both are what an SDK produces for an
/// operation with no input members.
#[tokio::test]
async fn get_index_accepts_a_unit_input_in_either_wire_form() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    let (arn, _) = create_index(&emulator, region, &BTreeMap::new()).await;

    for (case, body) in [("no body", None), ("an empty JSON object", Some(json!({})))] {
        let answer = emulator.send(region, "POST", "/GetIndex", body).await;
        assert_eq!(
            answer.ok(case)["Arn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: GetIndex answers the Region's index"
        );
    }
}

/// A tagging call resolves its `resourceArn` against the resources THIS service owns.
///
/// Rows one to eight are ARNs outside this service's ARN namespace — another service's
/// resource, a service whose name merely resembles this one, and values that are not
/// ARNs at all: none of them names something this service can authorize against, so all
/// three operations refuse them with `UnauthorizedException` (HTTP 401). Rows nine to
/// twelve are well-formed ARNs INSIDE the namespace that name no stored resource: the
/// two operations that model a not-found error answer 404 there, and `TagResource`,
/// which models none, answers the same 401.
///
/// Every refusal must also leave the store exactly as it was, which the view tagged at
/// the end of the test pins.
#[tokio::test]
async fn tagging_refuses_an_arn_that_names_no_resource_this_service_owns() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    create_index(&emulator, region, &BTreeMap::new()).await;
    let view_arn = create_view(&emulator, region, "rex-tag-probe").await;
    let account = engine::emulator_account_id();

    // Rows: an ARN of another service, in and out of this Region; a global (region-less)
    // ARN; a service whose name merely starts with this one's; and four values that are
    // not ARNs at all (no scheme, too few fields, a mis-cased scheme, empty).
    let foreign = [
        ("an SNS topic", engine::emulator_arn("sns", region, "rex-tag-foreign-probe")),
        ("an SQS queue", engine::emulator_arn("sqs", "eu-west-1", "rex-foreign-queue")),
        ("an S3 bucket, with no region or account", "arn:aws:s3:::rex-foreign-bucket".to_string()),
        ("an IAM role, a global resource", format!("arn:aws:iam::{account}:role/rex-foreign-role")),
        (
            "a service whose name starts with this one's",
            engine::emulator_arn("resource-explorer-2-preview", region, "view/x/y"),
        ),
        ("a bare name, not an ARN", "rex-tag-foreign-probe".to_string()),
        (
            "an ARN missing its resource field",
            format!("arn:aws:resource-explorer-2:{region}:{account}"),
        ),
        (
            "an ARN whose scheme is mis-cased",
            format!("ARN:aws:resource-explorer-2:{region}:{account}:view/x/y"),
        ),
    ];
    for (case, arn) in &foreign {
        for (operation, answer) in [
            ("TagResource", emulator.tag(region, arn, &tag_map(&[("k", "v")])).await),
            ("ListTagsForResource", emulator.list_tags(region, arn).await),
            ("UntagResource", emulator.untag(region, arn, &["k"]).await),
        ] {
            let case = format!("{operation} on {case}");
            answer.failed(&case, 401, "UnauthorizedException");
            assert_eq!(
                answer.error_message(),
                "Unauthorized",
                "case {case:?}: the refusal carries nothing from the request"
            );
        }
    }

    // Rows: own-namespace ARNs naming nothing — a view and an index id never minted, an
    // ARN in a Region with no index at all, and another account's view.
    let unresolvable = [
        ("a view id never minted", own_arn(region, "view/absent/0f8c2a1e-5d3b-8a47-9c61-2b7e4d905a13")),
        ("an index id never minted", own_arn(region, "index/7d1f4b60-9e2c-8351-a4d8-6c3b05e71924")),
        ("a view in a Region with no index", own_arn("eu-west-1", "view/absent/3a6e9c25-1b74-8de0-95f2-8c417b6d2035")),
        (
            "another account's view",
            format!("arn:aws:resource-explorer-2:{region}:999999999999:view/other/5c2d8f31-7a46-8b92-ae03-1d49f6520b78"),
        ),
    ];
    for (case, arn) in &unresolvable {
        let tagged = emulator.tag(region, arn, &tag_map(&[("k", "v")])).await;
        let case_tag = format!("TagResource on {case}");
        // TagResource models no ResourceNotFoundException, so an ARN it cannot resolve
        // is an ARN it cannot authorize.
        tagged.failed(&case_tag, 401, "UnauthorizedException");
        assert_eq!(
            tagged.error_message(),
            "Unauthorized",
            "case {case_tag:?}: the refusal carries nothing from the request"
        );

        for (operation, answer) in [
            ("ListTagsForResource", emulator.list_tags(region, arn).await),
            ("UntagResource", emulator.untag(region, arn, &["k"]).await),
        ] {
            let case = format!("{operation} on {case}");
            answer.failed(&case, 404, "ResourceNotFoundException");
            assert!(
                answer.error_message().contains(arn),
                "case {case:?}: the refusal names the ARN it could not resolve, got {:?}",
                answer.error_message()
            );
        }
    }

    // None of those refusals attached, removed or invented anything: the one real
    // resource in the store still has no tags, and tagging it still works.
    assert_eq!(
        emulator.list_tags(region, &view_arn).await.tags("the probe view after every refusal"),
        BTreeMap::new(),
        "a refused tagging call must leave the store untouched"
    );
    emulator
        .tag(region, &view_arn, &tag_map(&[("k", "v")]))
        .await
        .ok("TagResource on the caller's own view");
    assert_eq!(
        emulator.list_tags(region, &view_arn).await.tags("the probe view"),
        tag_map(&[("k", "v")]),
        "the own-ARN positive control reads back what it wrote"
    );
}

/// One row of the tag-write table: what is attached, what is attached on top of it, and
/// what is then removed.
struct TagCase {
    case: &'static str,
    /// The pairs the first `TagResource` attaches.
    first: &'static [(&'static str, &'static str)],
    /// The pairs a second `TagResource` attaches on top of the first.
    second: &'static [(&'static str, &'static str)],
    /// The keys `UntagResource` then removes — some of which were never attached.
    remove: &'static [&'static str],
}

/// `TagResource` merges into the tag map stored on the resource, `ListTagsForResource`
/// reports exactly that map, and `UntagResource` removes exactly the keys it names —
/// for BOTH kinds of resource this service owns, the Region's index and a view.
///
/// The rows differ along what the rule generalises over: how many pairs are written, an
/// empty write, a key written twice, keys and values carrying the characters the search
/// query syntax gives meaning to, non-ASCII text, removing a key that was never
/// attached, and removing every key there is. Each expectation is COMPUTED from the row,
/// so no row can pass by agreeing with a literal in the handler.
#[tokio::test]
async fn tag_writes_round_trip_through_every_reader_of_a_resource() {
    let cases = [
        TagCase {
            case: "one pair",
            first: &[("k", "v")],
            second: &[],
            remove: &[],
        },
        TagCase {
            case: "an empty map attaches nothing and removes nothing",
            first: &[],
            second: &[("env", "prod")],
            remove: &[],
        },
        TagCase {
            case: "several pairs, one of them later overwritten",
            first: &[("env", "prod"), ("team", "platform"), ("cost-center", "1234")],
            second: &[("env", "staging")],
            remove: &["team"],
        },
        TagCase {
            case: "the same key written twice keeps one entry",
            first: &[("env", "prod")],
            second: &[("env", "prod-2")],
            remove: &[],
        },
        TagCase {
            case: "keys and values carrying the query syntax's own characters",
            first: &[("tag:env", "a=b:c"), ("-negated", "value*")],
            second: &[],
            remove: &["-negated"],
        },
        TagCase {
            case: "non-ASCII key and value",
            first: &[("équipe", "naïve ☁"), ("キー", "値")],
            second: &[],
            remove: &["キー"],
        },
        TagCase {
            case: "removing a key that was never attached",
            first: &[("keep", "yes")],
            second: &[],
            remove: &["never-attached", "Keep"],
        },
        TagCase {
            case: "removing every key leaves an empty map",
            first: &[("a", "1"), ("b", "2")],
            second: &[],
            remove: &["a", "b"],
        },
    ];

    for case in &cases {
        // A fresh emulator per row, so each row starts from a resource with no tags.
        let emulator = Emulator::new();
        let region = "us-east-1";
        let (index_arn, _) = create_index(&emulator, region, &BTreeMap::new()).await;
        let view_arn = create_view(&emulator, region, "rex-tagged-view").await;

        for (kind, arn, reader, member) in [
            ("index", &index_arn, "GetIndex", "Tags"),
            ("view", &view_arn, "GetView", "Tags"),
        ] {
            let label = format!("{} [{kind}]", case.case);

            // Expectations computed from the row, never written out by hand.
            let after_first = tag_map(case.first);
            let mut after_second = after_first.clone();
            after_second.extend(tag_map(case.second));
            let mut after_remove = after_second.clone();
            for key in case.remove {
                after_remove.remove(*key);
            }

            let tagged = emulator.tag(region, arn, &after_first).await;
            tagged.ok(&label);
            assert!(
                tagged.body.get("Tags").is_none(),
                "case {label:?}: TagResource has an empty output, got {}",
                tagged.body
            );
            assert_eq!(
                emulator.list_tags(region, arn).await.tags(&label),
                after_first,
                "case {label:?}: ListTagsForResource reports the map TagResource stored"
            );

            emulator
                .tag(region, arn, &tag_map(case.second))
                .await
                .ok(&label);
            assert_eq!(
                emulator.list_tags(region, arn).await.tags(&label),
                after_second,
                "case {label:?}: a second TagResource merges onto the stored map"
            );

            // The SAME map the resource's own reader reports: the tags live on the
            // resource, not in a second place the tagging operations keep.
            let read_back = match reader {
                "GetIndex" => emulator.get_index(region).await,
                _ => emulator.call(region, "GetView", json!({ "ViewArn": arn })).await,
            };
            assert_eq!(
                read_back.tags(&label),
                after_second,
                "case {label:?}: {reader}'s {member} is the same stored map"
            );

            // `tagKeys` is `@required`, so there is no "remove nothing" request for a
            // client to send: a row with no keys to remove ends here.
            if case.remove.is_empty() {
                continue;
            }
            emulator.untag(region, arn, case.remove).await.ok(&label);
            assert_eq!(
                emulator.list_tags(region, arn).await.tags(&label),
                after_remove,
                "case {label:?}: UntagResource removes exactly the keys it names"
            );

            // Removing the same keys again is the same end state, so a retry is safe.
            emulator.untag(region, arn, case.remove).await.ok(&label);
            assert_eq!(
                emulator.list_tags(region, arn).await.tags(&label),
                after_remove,
                "case {label:?}: repeating the removal changes nothing"
            );
        }
    }
}

/// A tag written through `TagResource` is indexed with the resource it is attached to,
/// so `Search`'s documented `tag.key`, `tag.value` and `tag` filters find it — the
/// strongest statement that the write landed in the one stored map every reader of the
/// resource derives its answer from.
#[tokio::test]
async fn tags_written_through_tag_resource_are_searchable() {
    let emulator = Emulator::new();
    let region = "us-east-1";
    create_index(&emulator, region, &BTreeMap::new()).await;
    let view_arn = create_view(&emulator, region, "rex-searchable-tags").await;
    // A tag filter reads a resource's tags, so the view has to include that property
    // before it can answer one.
    emulator
        .call(
            region,
            "UpdateView",
            json!({
                "ViewArn": view_arn,
                "IncludedProperties": [{ "Name": "tags" }],
            }),
        )
        .await
        .ok("including the tags property");
    emulator
        .tag(region, &view_arn, &tag_map(&[("env", "prod")]))
        .await
        .ok("tagging the view");

    // Each row is a documented filter form that must now match the view, and a row of
    // the same form that must not.
    for (query, expected) in [
        ("tag.key:env", true),
        ("tag.value:prod", true),
        ("tag:env=prod", true),
        ("tag.key:absent", false),
        ("tag:env=staging", false),
    ] {
        let case = format!("Search {query:?}");
        let answer = emulator
            .call(
                region,
                "Search",
                json!({ "QueryString": query, "ViewArn": view_arn }),
            )
            .await;
        let found = answer.ok(&case)["Resources"]
            .as_array()
            .expect("Search answers a Resources list")
            .iter()
            .any(|resource| resource["Arn"].as_str() == Some(view_arn.as_str()));
        assert_eq!(
            found, expected,
            "case {case:?}: expected the tagged view to be found={expected}, got body {}",
            answer.body
        );
    }

    // And once the tag is removed it is no longer found.
    emulator
        .untag(region, &view_arn, &["env"])
        .await
        .ok("untagging the view");
    let answer = emulator
        .call(
            region,
            "Search",
            json!({ "QueryString": "tag:env=prod", "ViewArn": view_arn }),
        )
        .await;
    let found = answer.ok("Search after UntagResource")["Resources"]
        .as_array()
        .expect("Search answers a Resources list")
        .iter()
        .any(|resource| resource["Arn"].as_str() == Some(view_arn.as_str()));
    assert!(
        !found,
        "case \"Search after UntagResource\": a removed tag must stop matching, got {}",
        answer.body
    );
}

/// A tag write is bounded by the per-resource tag quota: a request that would leave the
/// resource holding more distinct keys than the quota admits is the modelled
/// `ValidationException` (HTTP 400) and writes nothing, while a request that only
/// rewrites keys the resource already carries is never refused by it — even at the
/// quota.
///
/// The rows walk the bound: one key short of it, exactly at it, one past it in a single
/// request, and one past it reached by a second request.
#[tokio::test]
async fn tag_writes_stop_at_the_tags_per_resource_quota() {
    let quota = service_com_amazonaws_resourceexplorer2_resourceexplorer::state::MAX_TAGS_PER_RESOURCE;
    let emulator = Emulator::new();
    let region = "us-east-1";
    let (index_arn, _) = create_index(&emulator, region, &BTreeMap::new()).await;
    let view_arn = create_view(&emulator, region, "rex-quota-view").await;

    /// `count` distinct pairs, named so two different counts never disagree on a key.
    fn pairs(count: usize, value: &str) -> BTreeMap<String, String> {
        (0..count)
            .map(|index| (format!("key-{index}"), value.to_string()))
            .collect()
    }

    for arn in [&index_arn, &view_arn] {
        // One short of the quota, then exactly at it: both stored.
        for count in [quota - 1, quota] {
            let case = format!("{count} tags on {arn}");
            emulator.tag(region, arn, &pairs(count, "v")).await.ok(&case);
            assert_eq!(
                emulator.list_tags(region, arn).await.tags(&case).len(),
                count,
                "case {case:?}: a request up to the quota is stored"
            );
        }

        // At the quota, rewriting the SAME keys adds no key, so it is not refused.
        let case = format!("rewriting all {quota} keys on {arn}");
        emulator
            .tag(region, arn, &pairs(quota, "rewritten"))
            .await
            .ok(&case);
        let at_quota = emulator.list_tags(region, arn).await.tags(&case);
        assert_eq!(
            at_quota,
            pairs(quota, "rewritten"),
            "case {case:?}: re-tagging at the quota rewrites the values"
        );

        // One key past the quota, reached by adding a single new key — refused, and
        // nothing is written.
        let case = format!("one key past the quota on {arn}");
        let refused = emulator
            .tag(region, arn, &tag_map(&[("one-too-many", "v")]))
            .await;
        refused.failed(&case, 400, "ValidationException");
        assert!(
            refused.error_message().contains(&quota.to_string()),
            "case {case:?}: the refusal names the limit, got {:?}",
            refused.error_message()
        );
        // The structured field list blames the member the pairs arrived in and reports
        // the count the request would have left behind.
        let field = &refused.body["FieldList"][0];
        assert_eq!(
            field["Name"].as_str(),
            Some("Tags"),
            "case {case:?}: the refusal blames the Tags member, got {}",
            refused.body
        );
        assert!(
            field["ValidationIssue"]
                .as_str()
                .is_some_and(|issue| issue.contains(&quota.to_string())),
            "case {case:?}: the field list reports the limit the member is bound by, \
             got {}",
            refused.body
        );
        assert_eq!(
            emulator.list_tags(region, arn).await.tags(&case),
            at_quota,
            "case {case:?}: a refused tag write stores nothing"
        );
    }

    // And a single request that is itself past the quota is refused on a resource that
    // carries no tags at all.
    let fresh = create_view(&emulator, region, "rex-quota-fresh-view").await;
    let case = "a single request past the quota";
    let refused = emulator.tag(region, &fresh, &pairs(quota + 1, "v")).await;
    refused.failed(case, 400, "ValidationException");
    assert_eq!(
        emulator.list_tags(region, &fresh).await.tags(case),
        BTreeMap::new(),
        "case {case:?}: a refused tag write stores nothing"
    );
}

/// Each tag WRITE leaves one structured audit record naming the operation, the caller's
/// presented access key id, the resource and the outcome — and no tag key or value,
/// because `TagMap` and the `tagKeys` list are both `@sensitive`. The reads leave none.
#[tokio::test]
async fn tag_writes_leave_one_audit_record_each_and_no_tag_content() {
    use std::collections::BTreeMap as Fields;
    use std::sync::Mutex;

    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Metadata, Subscriber};

    /// One captured event: its target and its fields as strings.
    #[derive(Clone, Debug)]
    struct CapturedEvent {
        target: String,
        fields: Fields<String, String>,
    }

    impl CapturedEvent {
        fn field(&self, name: &str) -> Option<&str> {
            self.fields.get(name).map(|value| value.trim_matches('"'))
        }

        /// Everything the record would print, so a test can state what must NOT be in it.
        fn rendered(&self) -> String {
            self.fields.values().cloned().collect::<Vec<_>>().join(" ")
        }
    }

    /// Collects an event's fields into strings, whatever type they were recorded as.
    struct FieldCollector<'a>(&'a mut Fields<String, String>);

    impl Visit for FieldCollector<'_> {
        fn record_str(&mut self, field: &Field, value: &str) {
            self.0.insert(field.name().to_string(), value.to_string());
        }

        fn record_u64(&mut self, field: &Field, value: u64) {
            self.0.insert(field.name().to_string(), value.to_string());
        }

        fn record_i64(&mut self, field: &Field, value: i64) {
            self.0.insert(field.name().to_string(), value.to_string());
        }

        fn record_bool(&mut self, field: &Field, value: bool) {
            self.0.insert(field.name().to_string(), value.to_string());
        }

        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.0.insert(field.name().to_string(), format!("{value:?}"));
        }
    }

    /// A `tracing` subscriber that keeps every event it is handed, so the records are
    /// read as FIELDS rather than scraped out of formatted text.
    #[derive(Clone, Default)]
    struct Capture {
        events: Arc<Mutex<Vec<CapturedEvent>>>,
    }

    impl Capture {
        /// The audit records about one resource, oldest first. The resource is part of
        /// the filter because the capture is installed for the whole test binary, so
        /// records of the suites running beside this one land in it too.
        fn records_for(&self, resource: &str) -> Vec<CapturedEvent> {
            self.events
                .lock()
                .expect("the capture buffer")
                .iter()
                .filter(|event| {
                    event.target == "audit" && event.field("resource") == Some(resource)
                })
                .cloned()
                .collect()
        }
    }

    impl Subscriber for Capture {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _span: &Attributes<'_>) -> Id {
            // This test asserts on events, not spans; one id for every span is enough.
            Id::from_u64(1)
        }

        fn record(&self, _span: &Id, _values: &Record<'_>) {}

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, event: &Event<'_>) {
            let mut fields = Fields::new();
            event.record(&mut FieldCollector(&mut fields));
            self.events
                .lock()
                .expect("the capture buffer")
                .push(CapturedEvent {
                    target: event.metadata().target().to_string(),
                    fields,
                });
        }

        fn enter(&self, _span: &Id) {}

        fn exit(&self, _span: &Id) {}
    }

    // Installed ONCE for the whole test binary, and global rather than thread-local:
    // tracing caches a callsite's interest the first time it is reached, and a suite
    // running beside this one reaches the handlers' callsites with no subscriber of its
    // own, which would cache them as "never" and suppress the records this test is about.
    static CAPTURE: std::sync::OnceLock<Capture> = std::sync::OnceLock::new();
    let capture = CAPTURE
        .get_or_init(|| {
            let capture = Capture::default();
            assert!(
                tracing::subscriber::set_global_default(capture.clone()).is_ok(),
                "a subscriber was already installed, so audit records cannot be captured"
            );
            tracing::callsite::rebuild_interest_cache();
            capture
        })
        .clone();

    let emulator = Emulator::new();
    let region = "us-east-1";
    create_index(&emulator, region, &BTreeMap::new()).await;
    let view_arn = create_view(&emulator, region, "rex-audited-view").await;

    // Each call must SUCCEED, or the assertions below would fail as "no record emitted"
    // when the real cause was a refused request.
    emulator
        .tag(region, &view_arn, &tag_map(&[("secret-key", "secret-value")]))
        .await
        .ok("the tag write");
    // A read emits no record, so the negative assertion below cannot pass on one.
    emulator.list_tags(region, &view_arn).await.ok("the read");
    emulator
        .untag(region, &view_arn, &["secret-key"])
        .await
        .ok("the tag removal");

    for (label, operation, outcome) in [
        ("the tag write", "TagResource", "tagged"),
        ("the tag removal", "UntagResource", "untagged"),
    ] {
        let matching: Vec<_> = capture
            .records_for(&view_arn)
            .into_iter()
            .filter(|event| event.field("operation") == Some(operation))
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "case {label:?}: expected exactly one audit record for {operation}, captured \
             {matching:?}"
        );
        let record = &matching[0];
        for (field, expected) in [
            ("principal", "AKIDTESTTESTTESTTEST"),
            ("resource", view_arn.as_str()),
            ("outcome", outcome),
        ] {
            assert_eq!(
                record.field(field),
                Some(expected),
                "case {label:?}: expected {field}={expected:?}, got {record:?}"
            );
        }
        // The tag key and value are the request's own content, and both shapes are
        // `@sensitive`: a record naming them would copy them somewhere less protected
        // than the store they went into.
        let rendered = record.rendered();
        for forbidden in ["secret-key", "secret-value"] {
            assert!(
                !rendered.contains(forbidden),
                "case {label:?}: the audit record must not carry tag content {forbidden:?}, \
                 got {record:?}"
            );
        }
    }

    // The read is not a state change, so it left no record of its own.
    let reads: Vec<_> = capture
        .records_for(&view_arn)
        .into_iter()
        .filter(|event| event.field("operation") == Some("ListTagsForResource"))
        .collect();
    assert!(
        reads.is_empty(),
        "case \"the read\": a reporting operation emits no audit record, captured {reads:?}"
    );
}
