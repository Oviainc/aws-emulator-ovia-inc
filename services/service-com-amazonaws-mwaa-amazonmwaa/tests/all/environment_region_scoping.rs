// SPDX-License-Identifier: CC0-1.0

//! Region scoping of the Amazon MWAA environment store, end to end over HTTP through the
//! router this service mounts in production.
//!
//! The rule every case here asserts: an environment belongs to the Region the request that
//! created it was signed for, and every operation addressed by `Name` — or by an ARN, which
//! carries a Region of its own — resolves that name only among the environments of the
//! Region the CALLING request is signed for, so one Region's listing names only its own
//! environments, a name that resolves in one Region resolves to nothing in another, and the
//! same name may name an independent environment in each Region.
//!
//! Each row of the table varies the Region: the Region codes below differ in their
//! geography prefix, their direction segment and their ordinal, and one of them holds no
//! environment at all. No row is privileged: every assertion runs for every Region, and the
//! per-Region records differ in a stored member so a cross-Region read cannot pass by
//! returning a look-alike.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_mwaa_amazonmwaa::state;

/// The canonical emulator account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// Comfortably longer than [`state::TRANSITION_SPAN`], so a transition the clock owes has
/// certainly come due and an environment accepts an update.
const PAST_TRANSITION: Duration = Duration::from_millis(900);

/// The Regions the table runs over: every one of them gets the same environment names, so
/// a lookup that ignored the Region would answer with another Region's record. They differ
/// in geography prefix (`us`/`eu`/`ap`/`sa`), direction and ordinal — the shape
/// `EnvironmentArn`'s own Region segment (`[a-z0-9\-]+`) allows.
const POPULATED_REGIONS: [&str; 4] = ["us-east-1", "eu-west-1", "ap-southeast-2", "sa-east-1"];

/// A Region no create is ever signed for: its listing must be empty and its lookups must
/// resolve nothing, however many environments the other Regions hold.
const EMPTY_REGION: &str = "eu-central-1";

/// The name every populated Region stores an environment under — the case the rule is
/// about: one name, an independent environment per Region.
const SHARED_NAME: &str = "Shared";

/// One HTTP answer, reduced to what these tests assert on.
struct Answer {
    status: u16,
    /// `x-amzn-errortype`, which is where restJson1 resolves an error shape from.
    error_type_header: Option<String>,
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The body as a JSON document when it is one, and `Value::Null` when it is not — the
    /// refusal the proxied operation relays carries the sentence as plain text rather than
    /// a document.
    fn document(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    /// The modelled error code a restJson1 client resolves: the `x-amzn-errortype` header
    /// first, then the body's own `__type` discriminator.
    fn error_code(&self) -> String {
        let named = self.error_type_header.clone().unwrap_or_else(|| {
            self.document()
                .get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        });
        named
            .rsplit(['#', '/'])
            .next()
            .unwrap_or_default()
            .split(':')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// The sentence a client reads off a refusal, the way an SDK's JSON error parser reads
    /// it: the `message` member of a document body, and the body itself when the body is
    /// not a document.
    fn refusal_message(&self) -> String {
        match self.document() {
            Value::Object(object) => object
                .get("message")
                .or_else(|| object.get("Message"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            _ => self.text(),
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// The emulator, assembled exactly as the router assembles it: one shared engine behind
/// one mounted service. Every request below names the Region it is signed for, so one
/// store is reached through several regional endpoints.
struct Harness {
    router: MultiServiceRouter,
}

impl Harness {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_mwaa_amazonmwaa::EmulatedService::new(Arc::clone(&engine)),
        );
        Self { router }
    }

    async fn send(&self, region: &str, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let payload = body
            .map(|value| serde_json::to_vec(&value).expect("a serialisable body"))
            .unwrap_or_default();
        // A signed-looking request whose SigV4 credential scope names `region`, the way an
        // SDK configured for that Region signs. Signature verification is off in this
        // harness, so the signature value is never checked.
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/airflow/aws4_request, \
             SignedHeaders=host, Signature=0000"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", format!("api.airflow.{region}.amazonaws.com"))
            .header("authorization", authorization)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let error_type_header = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        Answer {
            status: response.status().as_u16(),
            error_type_header,
            body: response.into_body().to_vec(),
        }
    }

    async fn create(&self, region: &str, name: &str, max_workers: i64) -> Answer {
        self.send(
            region,
            "PUT",
            &format!("/environments/{name}"),
            Some(create_body(max_workers)),
        )
        .await
    }

    async fn get(&self, region: &str, name: &str) -> Answer {
        self.send(region, "GET", &format!("/environments/{name}"), None)
            .await
    }

    /// The `Environment` of a successful `GetEnvironment`.
    async fn environment(&self, region: &str, name: &str) -> Value {
        let answer = self.get(region, name).await;
        assert_eq!(
            answer.status,
            200,
            "case {region}/{name}: expected GetEnvironment 200, got {} {}",
            answer.status,
            answer.text()
        );
        answer
            .json()
            .get("Environment")
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {region}/{name}: GetEnvironment answered without Environment")
            })
    }

    async fn list(&self, region: &str) -> Vec<String> {
        let answer = self.send(region, "GET", "/environments", None).await;
        assert_eq!(
            answer.status,
            200,
            "case {region}: expected ListEnvironments 200, got {} {}",
            answer.status,
            answer.text()
        );
        answer
            .json()
            .get("Environments")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {region}: ListEnvironments answered without a list"))
            .iter()
            .map(|entry| entry.as_str().unwrap_or_default().to_string())
            .collect()
    }

    async fn update(&self, region: &str, name: &str, body: Value) -> Answer {
        self.send(region, "PATCH", &format!("/environments/{name}"), Some(body))
            .await
    }

    async fn delete(&self, region: &str, name: &str) -> Answer {
        self.send(region, "DELETE", &format!("/environments/{name}"), None)
            .await
    }

    async fn list_tags(&self, region: &str, arn: &str) -> Answer {
        self.send(region, "GET", &format!("/tags/{}", encode_label(arn)), None)
            .await
    }

    async fn tag(&self, region: &str, arn: &str, tags: Value) -> Answer {
        self.send(
            region,
            "POST",
            &format!("/tags/{}", encode_label(arn)),
            Some(json!({"Tags": tags})),
        )
        .await
    }

    async fn untag(&self, region: &str, arn: &str, key: &str) -> Answer {
        self.send(
            region,
            "DELETE",
            &format!("/tags/{}?tagKeys={key}", encode_label(arn)),
            None,
        )
        .await
    }
}

/// Percent-encode the characters an ARN carries that are not legal in a single
/// (non-greedy) `@httpLabel` segment — exactly what an SDK does for `ResourceArn`.
fn encode_label(value: &str) -> String {
    value.replace(':', "%3A").replace('/', "%2F")
}

/// A create body every member of which satisfies its modelled constraints. `MaxWorkers`
/// is the member that differs per Region, so a read that crossed Regions would report
/// another Region's worker ceiling.
fn create_body(max_workers: i64) -> Value {
    json!({
        "ExecutionRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/mwaa-execution"),
        "SourceBucketArn": "arn:aws:s3:::my-airflow-bucket",
        "DagS3Path": "dags",
        "MaxWorkers": max_workers,
        "NetworkConfiguration": {
            "SubnetIds": ["subnet-00000000000000001", "subnet-00000000000000002"],
            "SecurityGroupIds": ["sg-00000000000000001"],
        },
    })
}

/// The name only `region` stores an environment under, so a lookup of it from any other
/// Region must resolve nothing. Built from the Region code, which keeps it inside
/// `EnvironmentName`'s own `^[a-zA-Z][0-9a-zA-Z-_]*$`.
fn region_only_name(region: &str) -> String {
    format!("Only-{region}")
}

/// The `MaxWorkers` the environments of `region` are created with: a different ceiling per
/// Region, all inside the modelled `@range(min: 1)`.
fn max_workers_of(region: &str) -> i64 {
    let index = POPULATED_REGIONS
        .iter()
        .position(|candidate| candidate == &region)
        .unwrap_or_else(|| panic!("{region} is not one of the populated Regions"));
    2 + index as i64
}

/// Every populated Region holding two environments: the shared name and its own.
async fn populated() -> Harness {
    let harness = Harness::new();
    for region in POPULATED_REGIONS {
        for name in [SHARED_NAME.to_string(), region_only_name(region)] {
            let answer = harness.create(region, &name, max_workers_of(region)).await;
            assert_eq!(
                answer.status,
                200,
                "case {region}/{name}: the fixture create must succeed, got {} {}",
                answer.status,
                answer.text()
            );
        }
    }
    harness
}

/// The expected listing of `region`: the names it created, sorted.
fn expected_names(region: &str) -> Vec<String> {
    let mut names = vec![SHARED_NAME.to_string(), region_only_name(region)];
    names.sort();
    names
}

/// A string member of an `Environment`.
fn member(environment: &Value, name: &str) -> String {
    environment
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("the Environment must carry {name}, got {environment:?}"))
        .to_string()
}

#[tokio::test]
async fn a_listing_names_only_the_environments_of_the_region_it_is_signed_for() {
    let harness = populated().await;

    for region in POPULATED_REGIONS {
        let listed = harness.list(region).await;
        assert_eq!(
            listed,
            expected_names(region),
            "case {region}: expected only this Region's environments, got {listed:?}"
        );
    }

    let empty = harness.list(EMPTY_REGION).await;
    assert!(
        empty.is_empty(),
        "case {EMPTY_REGION}: a Region no create was signed for holds no environment, got {empty:?}"
    );
}

#[tokio::test]
async fn one_name_names_an_independent_environment_in_each_region() {
    let harness = populated().await;

    for region in POPULATED_REGIONS {
        let environment = harness.environment(region, SHARED_NAME).await;
        // The create was accepted in every Region, each minting its own ARN — the name is
        // not taken account-wide.
        assert_eq!(
            member(&environment, "Arn"),
            format!("arn:aws:airflow:{region}:{ACCOUNT}:environment/{SHARED_NAME}"),
            "case {region}: the ARN must carry the Region the environment lives in, got {:?}",
            member(&environment, "Arn")
        );
        // Every minted host and ARN this record reports names this Region, and no other.
        for reported in [
            "WebserverUrl",
            "CeleryExecutorQueue",
            "WebserverVpcEndpointService",
            "DatabaseVpcEndpointService",
        ] {
            let value = member(&environment, reported);
            assert!(
                value.contains(region),
                "case {region}/{reported}: expected a value in this Region, got {value:?}"
            );
            for other in POPULATED_REGIONS.iter().filter(|other| *other != &region) {
                assert!(
                    !value.contains(other),
                    "case {region}/{reported}: must not name {other}, got {value:?}"
                );
            }
        }
        // And the stored members are this Region's, not a look-alike from another.
        assert_eq!(
            environment.get("MaxWorkers").and_then(Value::as_i64),
            Some(max_workers_of(region)),
            "case {region}: expected this Region's stored MaxWorkers, got {:?}",
            environment.get("MaxWorkers")
        );
    }
}

#[tokio::test]
async fn a_name_addressed_operation_resolves_nothing_outside_the_environments_region() {
    let harness = populated().await;
    tokio::time::sleep(PAST_TRANSITION).await;

    for owner in POPULATED_REGIONS {
        let name = region_only_name(owner);
        for caller in POPULATED_REGIONS
            .iter()
            .copied()
            .chain([EMPTY_REGION])
            .filter(|caller| *caller != owner)
        {
            // `(label, method, uri, body, the sentence the refusal carries)`. The control
            // plane words the condition from the request; the environment data endpoint's
            // front door words it itself, naming no environment.
            let calls: Vec<(&str, &str, String, Option<Value>, String)> = vec![
                (
                    "GetEnvironment",
                    "GET",
                    format!("/environments/{name}"),
                    None,
                    state::environment_not_found_message(&name),
                ),
                (
                    "UpdateEnvironment",
                    "PATCH",
                    format!("/environments/{name}"),
                    Some(json!({"MaxWorkers": 9})),
                    state::environment_not_found_message(&name),
                ),
                (
                    "DeleteEnvironment",
                    "DELETE",
                    format!("/environments/{name}"),
                    None,
                    state::environment_not_found_message(&name),
                ),
                (
                    "CreateCliToken",
                    "POST",
                    format!("/clitoken/{name}"),
                    None,
                    state::ENVIRONMENT_ENDPOINT_NOT_FOUND_MESSAGE.to_string(),
                ),
                (
                    "CreateWebLoginToken",
                    "POST",
                    format!("/webtoken/{name}"),
                    None,
                    state::ENVIRONMENT_ENDPOINT_NOT_FOUND_MESSAGE.to_string(),
                ),
            ];
            for (operation, method, uri, body, expected_message) in calls {
                let case = format!("{operation} {name} from {caller}");
                let answer = harness.send(caller, method, &uri, body).await;
                assert_eq!(
                    answer.status,
                    404,
                    "case {case:?}: an environment of another Region resolves to nothing, got {} {}",
                    answer.status,
                    answer.text()
                );
                assert_eq!(
                    answer.error_code(),
                    "ResourceNotFoundException",
                    "case {case:?}: expected the modelled wire code, got {:?}",
                    answer.error_code()
                );
                let message = answer.refusal_message();
                assert_eq!(
                    message, expected_message,
                    "case {case:?}: expected the endpoint's own wording, got {message:?}"
                );
            }

            // The proxied operation refuses the same lookup with the same modelled
            // exception, worded by the data endpoint's front door.
            let proxied = harness
                .send(
                    caller,
                    "POST",
                    &format!("/restapi/{name}"),
                    Some(json!({"Path": "/dags", "Method": "GET"})),
                )
                .await;
            assert_eq!(
                proxied.status,
                404,
                "case InvokeRestApi {name} from {caller}: expected 404, got {} {}",
                proxied.status,
                proxied.text()
            );
        }

        // The positive control: in its OWN Region every one of those calls resolves the
        // environment, and the proxied one serves the Apache Airflow call against that
        // environment's own state.
        let own_region_calls: Vec<(&str, &str, String, Option<Value>, u16)> = vec![
            ("GetEnvironment", "GET", format!("/environments/{name}"), None, 200),
            (
                "CreateCliToken",
                "POST",
                format!("/clitoken/{name}"),
                None,
                200,
            ),
            (
                "CreateWebLoginToken",
                "POST",
                format!("/webtoken/{name}"),
                None,
                200,
            ),
            (
                "InvokeRestApi",
                "POST",
                format!("/restapi/{name}"),
                Some(json!({"Path": "/dags", "Method": "GET"})),
                200,
            ),
            (
                "UpdateEnvironment",
                "PATCH",
                format!("/environments/{name}"),
                Some(json!({"MaxWorkers": 9})),
                200,
            ),
        ];
        for (operation, method, uri, body, expected_status) in own_region_calls {
            let answer = harness.send(owner, method, &uri, body).await;
            assert_eq!(
                answer.status,
                expected_status,
                "case {operation} {name} from its own Region {owner}: expected {expected_status}, \
                 got {} {}",
                answer.status,
                answer.text()
            );
        }
    }
}

#[tokio::test]
async fn a_write_in_one_region_leaves_the_same_name_in_every_other_region_alone() {
    let harness = populated().await;
    tokio::time::sleep(PAST_TRANSITION).await;

    // An update of the shared name in ONE Region.
    let [updated_region, ..] = POPULATED_REGIONS;
    let raised = 11;
    let answer = harness
        .update(updated_region, SHARED_NAME, json!({"MaxWorkers": raised}))
        .await;
    assert_eq!(
        answer.status,
        200,
        "case {updated_region}: expected UpdateEnvironment 200, got {} {}",
        answer.status,
        answer.text()
    );
    tokio::time::sleep(PAST_TRANSITION).await;

    for region in POPULATED_REGIONS {
        let expected = if region == updated_region {
            raised
        } else {
            max_workers_of(region)
        };
        let environment = harness.environment(region, SHARED_NAME).await;
        assert_eq!(
            environment.get("MaxWorkers").and_then(Value::as_i64),
            Some(expected),
            "case {region}: only the updated Region's environment may change, got {:?}",
            environment.get("MaxWorkers")
        );
    }

    // A delete of the shared name in ONE Region removes it only there — once the teardown
    // it starts has come due.
    let answer = harness.delete(updated_region, SHARED_NAME).await;
    assert_eq!(
        answer.status,
        200,
        "case {updated_region}: expected DeleteEnvironment 200, got {} {}",
        answer.status,
        answer.text()
    );
    tokio::time::sleep(PAST_TRANSITION).await;
    for region in POPULATED_REGIONS {
        let listed = harness.list(region).await;
        let expected = if region == updated_region {
            vec![region_only_name(region)]
        } else {
            expected_names(region)
        };
        assert_eq!(
            listed, expected,
            "case {region}: a delete in {updated_region} must not remove another Region's \
             environment, got {listed:?}"
        );
    }
    // And the name is free to be created again in the Region it was deleted from, with its
    // own ARN.
    let answer = harness.create(updated_region, SHARED_NAME, 4).await;
    assert_eq!(
        answer.status,
        200,
        "case {updated_region}: expected a second create of the freed name to succeed, got {} {}",
        answer.status,
        answer.text()
    );
}

#[tokio::test]
async fn an_arn_addressed_tag_operation_resolves_only_inside_its_own_region() {
    let harness = populated().await;

    for owner in POPULATED_REGIONS {
        let arn = format!("arn:aws:airflow:{owner}:{ACCOUNT}:environment/{SHARED_NAME}");

        // In its own Region the ARN resolves, and the tags round-trip through the reader.
        let answer = harness
            .tag(owner, &arn, json!({"Team": owner}))
            .await;
        assert_eq!(
            answer.status,
            200,
            "case {owner}: expected TagResource 200, got {} {}",
            answer.status,
            answer.text()
        );
        let listed = harness.list_tags(owner, &arn).await;
        assert_eq!(
            listed.json().get("Tags").and_then(Value::as_object).and_then(|tags| tags
                .get("Team")
                .and_then(Value::as_str)),
            Some(owner),
            "case {owner}: expected the stored tag, got {}",
            listed.text()
        );

        // From any other Region the same ARN resolves to nothing: an ARN carries the
        // Region its resource lives in, and a regional endpoint holds only its own.
        for caller in POPULATED_REGIONS
            .iter()
            .copied()
            .chain([EMPTY_REGION])
            .filter(|caller| *caller != owner)
        {
            let calls: Vec<(&str, Answer)> = vec![
                ("ListTagsForResource", harness.list_tags(caller, &arn).await),
                (
                    "TagResource",
                    harness.tag(caller, &arn, json!({"Team": "elsewhere"})).await,
                ),
                ("UntagResource", harness.untag(caller, &arn, "Team").await),
            ];
            for (operation, answer) in calls {
                let case = format!("{operation} {arn} from {caller}");
                assert_eq!(
                    answer.status,
                    404,
                    "case {case:?}: an ARN of another Region resolves to nothing, got {} {}",
                    answer.status,
                    answer.text()
                );
            }
        }

        // Nothing a cross-Region call attempted reached the record.
        let listed = harness.list_tags(owner, &arn).await;
        assert_eq!(
            listed.json().get("Tags").and_then(Value::as_object).and_then(|tags| tags
                .get("Team")
                .and_then(Value::as_str)),
            Some(owner),
            "case {owner}: a cross-Region tag write or removal must not reach this record, got {}",
            listed.text()
        );
    }
}
