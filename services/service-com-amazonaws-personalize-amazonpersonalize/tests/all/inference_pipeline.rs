// SPDX-License-Identifier: CC0-1.0

//! The Amazon Personalize provisioning pipeline, end to end over the wire, and the
//! inference it makes reachable.
//!
//! Every case drives the production `MultiServiceRouter` with three services mounted on
//! ONE shared engine — the Personalize control plane (awsJson1_1, `POST /` with
//! `X-Amz-Target: AmazonPersonalize.<Operation>`), Amazon S3 (where the training data
//! is uploaded) and Amazon Personalize Runtime (restJson1, `POST /recommendations`,
//! `/personalize-ranking`, `/action-recommendations`) — so what is asserted is what a
//! signing AWS client would see: routing, deserialization, constraint validation, the
//! handlers, the cross-service read and serialization included.
//!
//! The rule under test: a client provisions a pipeline through the control plane
//! (CreateSchema, CreateDatasetGroup, CreateDataset, CreateDatasetImportJob,
//! CreateSolution, CreateSolutionVersion, CreateCampaign), polling `Describe*` until
//! each resource reports a terminal status, and the inference plane then serves the
//! deployed campaign from the records that pipeline actually imported — with the
//! operation the solution's recipe type names serving it and the other two refusing it.
//!
//! The suites are TABLES: the rows vary what the rule generalises over (the recipe, the
//! data-location form, the import mode, the input list's shape, the failure), and the
//! recorded cases the issue cited — a personalized-ranking round trip, and a
//! USER_PERSONALIZATION campaign refused by the two operations needing another recipe
//! type — are rows among them.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request is signed for, and every minted ARN is scoped to.
const REGION: &str = "us-east-1";
/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";
/// The bucket the training data is uploaded to.
const BUCKET: &str = "personalize-training-data";

/// The recipes under test. Source: the catalog the control plane resolves `recipeArn`
/// against (`service_com_amazonaws_personalize_amazonpersonalize::state::pipeline::RECIPES`),
/// which carries the recipe ARNs the Amazon Personalize "Choosing a recipe"
/// documentation publishes.
const RANKING_RECIPE: &str = "arn:aws:personalize:::recipe/aws-personalized-ranking";
/// A second `PERSONALIZED_RANKING` recipe, so the rule is tested on an entry the issue
/// did not cite.
const RANKING_RECIPE_V2: &str = "arn:aws:personalize:::recipe/aws-personalized-ranking-v2";
/// The `USER_PERSONALIZATION` recipe.
const USER_RECIPE: &str = "arn:aws:personalize:::recipe/aws-user-personalization";
/// A `RELATED_ITEMS` recipe.
const RELATED_RECIPE: &str = "arn:aws:personalize:::recipe/aws-sims";
/// The `PERSONALIZED_ACTIONS` recipe.
const ACTIONS_RECIPE: &str = "arn:aws:personalize:::recipe/aws-next-best-action";
/// A `USER_SEGMENTATION` recipe, whose results are read in batch rather than deployed
/// in a campaign.
const SEGMENTATION_RECIPE: &str = "arn:aws:personalize:::recipe/aws-item-affinity";

/// `GetRecommendations`: `POST /recommendations`.
const RECOMMENDATIONS: &str = "/recommendations";
/// `GetPersonalizedRanking`: `POST /personalize-ranking`.
const PERSONALIZE_RANKING: &str = "/personalize-ranking";
/// `GetActionRecommendations`: `POST /action-recommendations`.
const ACTION_RECOMMENDATIONS: &str = "/action-recommendations";

/// The statuses a resource reports before its create completes. Source: the model's own
/// documentation — "CREATE PENDING > CREATE IN_PROGRESS > ACTIVE -or- CREATE FAILED".
const TRANSIENT_STATUSES: &[&str] = &["CREATE PENDING", "CREATE IN_PROGRESS"];
/// How long a case waits for a create to reach a terminal status before calling it
/// stuck. Far above the emulator's own create span, so a slow machine does not fail the
/// suite.
const TERMINAL_DEADLINE: Duration = Duration::from_secs(20);

/// How many items the imported interactions mention.
const CATALOG_ITEMS: usize = 10;
/// How many actions the imported action interactions mention.
const CATALOG_ACTIONS: usize = 6;

/// One emulator: the Personalize control plane, the Personalize Runtime inference plane
/// and S3, all on ONE shared engine — which is what the assembled router does.
struct Pipeline {
    router: MultiServiceRouter,
}

impl Pipeline {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are verified by the emulator, never by a service; this suite is
        // about behaviour, so it signs with a fixed scope and verifies nothing.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_personalize_amazonpersonalize::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        Self { router }
    }

    /// One control-plane call (`AmazonPersonalize.<Operation>` on `POST /`).
    async fn control(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri(format!("https://personalize.{REGION}.amazonaws.com/"))
            .header("host", format!("personalize.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("AmazonPersonalize.{operation}"))
            .header("authorization", sigv4("personalize"))
            .body(::bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        (response.status().as_u16(), parse(response.body()))
    }

    /// A control-plane call that must succeed.
    async fn control_ok(&self, operation: &str, body: &Value, case: &str) -> Value {
        let (status, parsed) = self.control(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} to answer 200, got {status} {parsed}"
        );
        parsed
    }

    /// One inference call (restJson1 `POST <route>` on the runtime endpoint).
    async fn inference(&self, route: &str, body: &Value) -> (u16, Value) {
        let host = format!("personalize-runtime.{REGION}.amazonaws.com");
        let request = ::http::Request::builder()
            .method("POST")
            .uri(format!("https://{host}{route}"))
            .header("host", host)
            .header("content-type", "application/json")
            .header("authorization", sigv4("personalize"))
            .body(::bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        (response.status().as_u16(), parse(response.body()))
    }

    /// An inference call that must succeed.
    async fn inference_ok(&self, route: &str, body: &Value, case: &str) -> Value {
        let (status, parsed) = self.inference(route, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {route} to answer 200, got {status} {parsed}"
        );
        parsed
    }

    /// One path-style request to the emulator's S3 front door: S3 is a REST service, so
    /// the bucket and key live in the URI and the body is the object's bytes.
    async fn s3(&self, method: &str, path: &str, body: &[u8]) -> u16 {
        let request = ::http::Request::builder()
            .method(method)
            .uri(format!("https://s3.{REGION}.amazonaws.com{path}"))
            .header("host", format!("s3.{REGION}.amazonaws.com"))
            .header("authorization", sigv4("s3"))
            .body(::bytes::Bytes::copy_from_slice(body))
            .expect("a well-formed request");
        self.router.dispatch(request).await.status().as_u16()
    }

    /// Create [`BUCKET`] and upload one training file to it.
    ///
    /// The create names NO `LocationConstraint`: the unspecified constraint stands for
    /// us-east-1, which is the Region this suite signs for, and naming it explicitly is
    /// what S3 refuses.
    async fn upload(&self, key: &str, body: &str) {
        let created = self.s3("PUT", &format!("/{BUCKET}"), &[]).await;
        assert!(
            created == 200 || created == 409,
            "CreateBucket {BUCKET}: expected 200 or 409, got {created}"
        );
        let put = self
            .s3("PUT", &format!("/{BUCKET}/{key}"), body.as_bytes())
            .await;
        assert_eq!(put, 200, "PutObject {key}: expected 200, got {put}");
    }

    /// Poll `operation` until the resource it reports leaves the transient statuses, and
    /// return that resource.
    ///
    /// Every status seen on the way has to be one of the documented transient ones, so a
    /// resource that is reported ready immediately — which Amazon Personalize never does
    /// for these — would fail the case.
    async fn await_terminal(
        &self,
        operation: &str,
        arn_member: &str,
        resource_member: &str,
        arn: &str,
        case: &str,
    ) -> Value {
        let deadline = Instant::now() + TERMINAL_DEADLINE;
        loop {
            let body = self
                .control_ok(operation, &json!({arn_member: arn}), case)
                .await;
            let resource = body[resource_member].clone();
            let reported = resource["status"].as_str().unwrap_or("").to_string();
            if !TRANSIENT_STATUSES.contains(&reported.as_str()) {
                assert!(
                    !reported.is_empty(),
                    "case {case:?}: expected {operation} to report a status, got {body}"
                );
                return resource;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: {operation} never left {reported:?}"
            );
            ::tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Await a terminal status and assert it is `ACTIVE`.
    async fn await_active(
        &self,
        operation: &str,
        arn_member: &str,
        resource_member: &str,
        arn: &str,
        case: &str,
    ) -> Value {
        let resource = self
            .await_terminal(operation, arn_member, resource_member, arn, case)
            .await;
        assert_eq!(
            resource["status"].as_str().unwrap_or(""),
            "ACTIVE",
            "case {case:?}: expected {operation} to report ACTIVE, got {resource}"
        );
        resource
    }

    /// Create a schema and a dataset of `dataset_type` in `group`, import `csv` into it
    /// from object storage, and return the dataset ARN once the import is `ACTIVE`.
    async fn import_dataset(
        &self,
        case: &str,
        suffix: &str,
        group: &str,
        dataset_type: &str,
        columns: &[&str],
        csv: &str,
    ) -> String {
        let schema = self
            .control_ok(
                "CreateSchema",
                &json!({
                    "name": format!("{dataset_type}-{suffix}"),
                    "schema": avro_schema(dataset_type, columns),
                }),
                case,
            )
            .await["schemaArn"]
            .as_str()
            .expect("a schemaArn")
            .to_string();
        let dataset = self
            .control_ok(
                "CreateDataset",
                &json!({
                    "name": format!("{dataset_type}-{suffix}"),
                    "datasetGroupArn": group,
                    "datasetType": dataset_type,
                    "schemaArn": schema,
                }),
                case,
            )
            .await["datasetArn"]
            .as_str()
            .expect("a datasetArn")
            .to_string();

        let key = format!("{suffix}/{dataset_type}.csv");
        self.upload(&key, csv).await;
        let job = self.start_import(case, suffix, &dataset, &format!("s3://{BUCKET}/{key}"), None).await;
        self.await_active(
            "DescribeDatasetImportJob",
            "datasetImportJobArn",
            "datasetImportJob",
            &job,
            case,
        )
        .await;
        dataset
    }

    /// Submit one dataset import job and return its ARN.
    async fn start_import(
        &self,
        case: &str,
        suffix: &str,
        dataset: &str,
        data_location: &str,
        import_mode: Option<&str>,
    ) -> String {
        let mut body = json!({
            "jobName": format!("import-{suffix}-{}", short_hash(data_location)),
            "datasetArn": dataset,
            "dataSource": {"dataLocation": data_location},
            "roleArn": format!("arn:aws:iam::{ACCOUNT}:role/PersonalizeImport"),
        });
        if let Some(mode) = import_mode {
            body["importMode"] = json!(mode);
        }
        self.control_ok("CreateDatasetImportJob", &body, case).await["datasetImportJobArn"]
            .as_str()
            .expect("a datasetImportJobArn")
            .to_string()
    }

    /// Create a dataset group and wait for it to become `ACTIVE`.
    async fn active_group(&self, case: &str, suffix: &str) -> String {
        let arn = self
            .control_ok("CreateDatasetGroup", &json!({"name": format!("group-{suffix}")}), case)
            .await["datasetGroupArn"]
            .as_str()
            .expect("a datasetGroupArn")
            .to_string();
        self.await_active(
            "DescribeDatasetGroup",
            "datasetGroupArn",
            "datasetGroup",
            &arn,
            case,
        )
        .await;
        arn
    }

    /// Train a solution version of `recipe` on `group` and wait for it to reach a
    /// terminal status; return `(solution ARN, solution version ARN, that status)`.
    async fn train(
        &self,
        case: &str,
        suffix: &str,
        group: &str,
        recipe: &str,
    ) -> (String, String, Value) {
        let solution = self
            .control_ok(
                "CreateSolution",
                &json!({
                    "name": format!("solution-{suffix}"),
                    "datasetGroupArn": group,
                    "recipeArn": recipe,
                }),
                case,
            )
            .await["solutionArn"]
            .as_str()
            .expect("a solutionArn")
            .to_string();
        let version = self
            .control_ok(
                "CreateSolutionVersion",
                &json!({"solutionArn": solution}),
                case,
            )
            .await["solutionVersionArn"]
            .as_str()
            .expect("a solutionVersionArn")
            .to_string();
        let reported = self
            .await_terminal(
                "DescribeSolutionVersion",
                "solutionVersionArn",
                "solutionVersion",
                &version,
                case,
            )
            .await;
        (solution, version, reported)
    }

    /// Deploy `version` in a campaign and wait for it to become `ACTIVE`.
    async fn deploy(&self, case: &str, suffix: &str, version: &str, metadata: bool) -> String {
        let mut body = json!({
            "name": format!("campaign-{suffix}"),
            "solutionVersionArn": version,
        });
        if metadata {
            body["campaignConfig"] = json!({"enableMetadataWithRecommendations": true});
        }
        let arn = self.control_ok("CreateCampaign", &body, case).await["campaignArn"]
            .as_str()
            .expect("a campaignArn")
            .to_string();
        self.await_active("DescribeCampaign", "campaignArn", "campaign", &arn, case)
            .await;
        arn
    }

    /// The whole pipeline for one recipe: a dataset group, the datasets `datasets`
    /// names imported into it, a trained solution version and a deployed campaign.
    async fn provision(
        &self,
        case: &str,
        suffix: &str,
        recipe: &str,
        datasets: &[DatasetFixture],
        metadata: bool,
    ) -> String {
        let group = self.active_group(case, suffix).await;
        for dataset in datasets {
            self.import_dataset(
                case,
                suffix,
                &group,
                dataset.dataset_type,
                dataset.columns,
                &dataset.csv,
            )
            .await;
        }
        let (_, version, reported) = self.train(case, suffix, &group, recipe).await;
        assert_eq!(
            reported["status"].as_str().unwrap_or(""),
            "ACTIVE",
            "case {case:?}: expected the solution version to train, got {reported}"
        );
        self.deploy(case, suffix, &version, metadata).await
    }
}

/// One dataset of a fixture pipeline: its type, the columns its schema declares and the
/// CSV uploaded for it.
struct DatasetFixture {
    dataset_type: &'static str,
    columns: &'static [&'static str],
    csv: String,
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name` — the
/// routing signal the router reads first.
fn sigv4(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=0"
    )
}

/// A response body as JSON, or `Null` for an empty one.
fn parse(body: &::bytes::Bytes) -> Value {
    if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(body).unwrap_or(Value::Null)
    }
}

/// The wire error code of a failure response, with any namespace stripped.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `message` member of a failure response.
fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

/// A short, stable suffix for a name derived from `value`, so two fixtures of one suite
/// never collide on a resource name.
fn short_hash(value: &str) -> String {
    let digest = <sha2::Sha256 as sha2::Digest>::digest(value.as_bytes());
    hex::encode(&digest[..4])
}

/// An Avro record schema for `dataset_type` declaring `columns`, in the Avro JSON format
/// `CreateSchema` documents.
fn avro_schema(dataset_type: &str, columns: &[&str]) -> String {
    let fields: Vec<Value> = columns
        .iter()
        .map(|column| json!({"name": column, "type": "string"}))
        .collect();
    json!({
        "type": "record",
        "name": dataset_type,
        "namespace": "com.amazonaws.personalize.schema",
        "fields": fields,
        "version": "1.0",
    })
    .to_string()
}

/// The id of the `index`-th fixture item.
fn item_id(index: usize) -> String {
    format!("item-{index:02}")
}

/// The id of the `index`-th fixture action.
fn action_id(index: usize) -> String {
    format!("action-{index:02}")
}

/// The interactions fixture: item `n` carries `n` interactions from cycling users, so
/// the IMPORTED data is what decides which items the trained catalog holds.
fn interactions_csv(items: usize) -> String {
    let mut csv = String::from("USER_ID,ITEM_ID,TIMESTAMP,EVENT_TYPE\n");
    for index in 1..=items {
        for occurrence in 0..index {
            csv.push_str(&format!(
                "user-{},{},{},watch\n",
                occurrence % 5 + 1,
                item_id(index),
                1_700_000_000 + occurrence
            ));
        }
    }
    csv
}

/// The Items fixture: one row per item, carrying the `GENRE` column a
/// `metadataColumns` request and a filter expression read.
fn items_csv(items: usize) -> String {
    let genres = ["Comedy", "Drama", "Action"];
    let mut csv = String::from("ITEM_ID,GENRE\n");
    for index in 1..=items {
        csv.push_str(&format!(
            "{},{}\n",
            item_id(index),
            genres[index % genres.len()]
        ));
    }
    csv
}

/// The action-interactions fixture, laid out like [`interactions_csv`].
fn action_interactions_csv(actions: usize) -> String {
    let mut csv = String::from("USER_ID,ACTION_ID,TIMESTAMP,EVENT_TYPE\n");
    for index in 1..=actions {
        for occurrence in 0..index {
            csv.push_str(&format!(
                "user-{},{},{},taken\n",
                occurrence % 3 + 1,
                action_id(index),
                1_700_000_000 + occurrence
            ));
        }
    }
    csv
}

/// The interactions dataset of a fixture pipeline.
fn interactions_fixture(items: usize) -> DatasetFixture {
    DatasetFixture {
        dataset_type: "Interactions",
        columns: &["USER_ID", "ITEM_ID", "TIMESTAMP", "EVENT_TYPE"],
        csv: interactions_csv(items),
    }
}

/// The Items dataset of a fixture pipeline.
fn items_fixture(items: usize) -> DatasetFixture {
    DatasetFixture {
        dataset_type: "Items",
        columns: &["ITEM_ID", "GENRE"],
        csv: items_csv(items),
    }
}

/// The action-interactions dataset of a fixture pipeline.
fn action_interactions_fixture(actions: usize) -> DatasetFixture {
    DatasetFixture {
        dataset_type: "Action_Interactions",
        columns: &["USER_ID", "ACTION_ID", "TIMESTAMP", "EVENT_TYPE"],
        csv: action_interactions_csv(actions),
    }
}

/// The ids of a response list member, in response order.
fn ids(response: &Value, member: &str, id_member: &str) -> Vec<String> {
    response
        .get(member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("response {response} carries no {member}"))
        .iter()
        .map(|entry| {
            entry
                .get(id_member)
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("entry {entry} carries no {id_member}"))
                .to_string()
        })
        .collect()
}

/// The scores of a response list member, in response order.
fn scores(response: &Value, member: &str) -> Vec<f64> {
    response
        .get(member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("response {response} carries no {member}"))
        .iter()
        .map(|entry| {
            entry
                .get("score")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| panic!("entry {entry} carries no score"))
        })
        .collect()
}

/// The `recommendationId` of a response, which must never be empty.
fn assert_recommendation_id(case: &str, response: &Value) {
    let id = response
        .get("recommendationId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        !id.is_empty(),
        "case {case:?}: expected a non-empty recommendationId, got {response}"
    );
}

/// Assert the scores of a reported list never increase — "sorted in descending order by
/// prediction score".
fn assert_descending(case: &str, scores: &[f64]) {
    for pair in scores.windows(2) {
        assert!(
            pair[0] >= pair[1],
            "case {case:?}: scores must not increase, got {scores:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The recipe type decides which operation serves the campaign
// ---------------------------------------------------------------------------

/// One row of the recipe table: the recipe a pipeline is provisioned with, the route
/// that serves the campaign it deploys, and the request that route needs.
struct RecipeCase {
    case: &'static str,
    recipe: &'static str,
    served: &'static str,
    /// Whether the pipeline needs an action-interactions dataset rather than an
    /// item-interactions one.
    actions: bool,
}

/// The routes an inference request can take, so a case can assert the two that must
/// refuse a campaign as well as the one that serves it.
const ROUTES: [&str; 3] = [RECOMMENDATIONS, PERSONALIZE_RANKING, ACTION_RECOMMENDATIONS];

/// The request body `route` needs against `campaign`, for a catalog of known ids.
fn request_for(route: &str, campaign: &str, known: &[String]) -> Value {
    match route {
        PERSONALIZE_RANKING => json!({
            "campaignArn": campaign,
            "userId": "user-1",
            "inputList": known,
        }),
        ACTION_RECOMMENDATIONS => json!({"campaignArn": campaign, "userId": "user-1"}),
        _ => json!({
            "campaignArn": campaign,
            "userId": "user-1",
            "itemId": known.first().cloned().unwrap_or_default(),
        }),
    }
}

/// The list member and id member `route` answers with.
fn list_of(route: &str) -> (&'static str, &'static str) {
    match route {
        PERSONALIZE_RANKING => ("personalizedRanking", "itemId"),
        ACTION_RECOMMENDATIONS => ("actionList", "actionId"),
        _ => ("itemList", "itemId"),
    }
}

#[tokio::test]
async fn every_recipe_type_is_provisioned_over_the_wire_and_served_by_its_own_operation() {
    let cases = [
        RecipeCase {
            case: "personalized ranking",
            recipe: RANKING_RECIPE,
            served: PERSONALIZE_RANKING,
            actions: false,
        },
        RecipeCase {
            case: "personalized ranking v2",
            recipe: RANKING_RECIPE_V2,
            served: PERSONALIZE_RANKING,
            actions: false,
        },
        RecipeCase {
            case: "user personalization",
            recipe: USER_RECIPE,
            served: RECOMMENDATIONS,
            actions: false,
        },
        RecipeCase {
            case: "related items",
            recipe: RELATED_RECIPE,
            served: RECOMMENDATIONS,
            actions: false,
        },
        RecipeCase {
            case: "personalized actions",
            recipe: ACTIONS_RECIPE,
            served: ACTION_RECOMMENDATIONS,
            actions: true,
        },
    ];

    for (index, row) in cases.iter().enumerate() {
        let emulator = Pipeline::new();
        let suffix = format!("recipe-{index}");
        let datasets = if row.actions {
            vec![action_interactions_fixture(CATALOG_ACTIONS)]
        } else {
            vec![interactions_fixture(CATALOG_ITEMS)]
        };
        let campaign = emulator
            .provision(row.case, &suffix, row.recipe, &datasets, false)
            .await;

        let known: Vec<String> = if row.actions {
            (1..=CATALOG_ACTIONS).map(action_id).collect()
        } else {
            (1..=CATALOG_ITEMS).map(item_id).collect()
        };

        // The operation the recipe type names serves the campaign, from the catalog the
        // imported records trained.
        let served = emulator
            .inference_ok(
                row.served,
                &request_for(row.served, &campaign, &known),
                row.case,
            )
            .await;
        let (member, id_member) = list_of(row.served);
        let reported = ids(&served, member, id_member);
        assert!(
            !reported.is_empty(),
            "case {:?}: expected {} to report entries trained from the imported records, got {served}",
            row.case,
            row.served
        );
        for id in &reported {
            assert!(
                known.contains(id),
                "case {:?}: {id:?} was never imported, so no campaign may recommend it \
                 (imported {known:?})",
                row.case
            );
        }
        assert_descending(row.case, &scores(&served, member));
        assert_recommendation_id(row.case, &served);

        // The other two refuse it: "The solution backing the campaign must have been
        // created using a recipe of type ..." is a different recipe type for each.
        for route in ROUTES.iter().filter(|route| **route != row.served) {
            let (status, body) = emulator
                .inference(route, &request_for(route, &campaign, &known))
                .await;
            assert_eq!(
                status, 400,
                "case {:?}: expected {route} to refuse the campaign with 400, got {status} {body}",
                row.case
            );
            assert_eq!(
                error_code(&body),
                "InvalidInputException",
                "case {:?}: expected {route} to refuse with InvalidInputException, got {body}",
                row.case
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The ranking is a permutation of the input list
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_personalized_ranking_is_a_scored_permutation_of_the_input_list() {
    let emulator = Pipeline::new();
    let campaign = emulator
        .provision(
            "ranking round trip",
            "ranking",
            RANKING_RECIPE,
            &[interactions_fixture(CATALOG_ITEMS)],
            false,
        )
        .await;

    let imported: Vec<String> = (1..=CATALOG_ITEMS).map(item_id).collect();
    // Rows vary what the rule generalises over: the size of the list, whether it names
    // ids the training data never held, and whether it repeats one.
    let unknown = "item-99".to_string();
    let cases: [(&str, Vec<String>); 5] = [
        ("every imported item", imported.clone()),
        ("one item", vec![imported[3].clone()]),
        (
            "a subset",
            imported.iter().take(4).cloned().collect::<Vec<_>>(),
        ),
        (
            "an item no import held",
            imported
                .iter()
                .take(3)
                .cloned()
                .chain([unknown.clone()])
                .collect::<Vec<_>>(),
        ),
        (
            "a repeated id",
            vec![imported[0].clone(), imported[0].clone(), imported[1].clone()],
        ),
    ];

    for (case, input_list) in cases {
        let response = emulator
            .inference_ok(
                PERSONALIZE_RANKING,
                &json!({
                    "campaignArn": campaign,
                    "userId": "user-2",
                    "inputList": input_list,
                }),
                case,
            )
            .await;
        let ranked = ids(&response, "personalizedRanking", "itemId");
        let distinct: BTreeSet<&String> = input_list.iter().collect();
        assert_eq!(
            ranked.iter().collect::<BTreeSet<_>>(),
            distinct,
            "case {case:?}: the ranking must be a permutation of the distinct inputList ids, \
             got {ranked:?} for {input_list:?}"
        );
        let reported = scores(&response, "personalizedRanking");
        assert_descending(case, &reported);
        assert_recommendation_id(case, &response);
        // "If an item was not included in the training dataset, the item is appended to
        // the end of the reranked list" — with no relevance for the user, so the
        // imported items all score ahead of it.
        if input_list.contains(&unknown) {
            assert_eq!(
                ranked.last(),
                Some(&unknown),
                "case {case:?}: an id the training data never held is appended last, got {ranked:?}"
            );
            assert_eq!(
                reported.last().copied(),
                Some(0.0),
                "case {case:?}: an id the training data never held carries no relevance, got \
                 {reported:?}"
            );
        }
        for (id, score) in ranked.iter().zip(&reported) {
            if id != &unknown {
                assert!(
                    *score > 0.0,
                    "case {case:?}: {id:?} was imported, so it carries a score, got {reported:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// What the import actually read is what the campaign serves
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_campaign_recommends_exactly_the_records_the_import_read() {
    // Rows vary the form the `dataLocation` takes and the import mode, which is what
    // decides the records a dataset holds.
    let cases: [(&str, bool, Option<&str>); 3] = [
        ("one file key", false, None),
        ("a folder prefix holding two files", true, None),
        ("an incremental import on top of a full one", false, Some("INCREMENTAL")),
    ];

    for (index, (case, folder, incremental)) in cases.into_iter().enumerate() {
        let emulator = Pipeline::new();
        let suffix = format!("import-{index}");
        let group = emulator.active_group(case, &suffix).await;

        let schema = emulator
            .control_ok(
                "CreateSchema",
                &json!({
                    "name": format!("interactions-{suffix}"),
                    "schema": avro_schema(
                        "Interactions",
                        &["USER_ID", "ITEM_ID", "TIMESTAMP", "EVENT_TYPE"],
                    ),
                }),
                case,
            )
            .await["schemaArn"]
            .as_str()
            .expect("a schemaArn")
            .to_string();
        let dataset = emulator
            .control_ok(
                "CreateDataset",
                &json!({
                    "name": format!("interactions-{suffix}"),
                    "datasetGroupArn": group,
                    "datasetType": "Interactions",
                    "schemaArn": schema,
                }),
                case,
            )
            .await["datasetArn"]
            .as_str()
            .expect("a datasetArn")
            .to_string();

        // The records the pipeline is expected to recommend from, built from exactly
        // what is uploaded below.
        let mut expected: BTreeSet<String> = BTreeSet::new();
        let location = if folder {
            // Two files under one prefix: "you can specify the path to the folder".
            let first = format!("{suffix}/folder/first.csv");
            let second = format!("{suffix}/folder/second.csv");
            emulator.upload(&first, &interactions_csv(3)).await;
            emulator
                .upload(
                    &second,
                    "USER_ID,ITEM_ID,TIMESTAMP,EVENT_TYPE\nuser-9,item-77,1700000000,watch\n",
                )
                .await;
            expected.extend((1..=3).map(item_id));
            expected.insert("item-77".to_string());
            format!("s3://{BUCKET}/{suffix}/folder/")
        } else {
            let key = format!("{suffix}/history.csv");
            emulator.upload(&key, &interactions_csv(4)).await;
            expected.extend((1..=4).map(item_id));
            format!("s3://{BUCKET}/{key}")
        };

        let job = emulator
            .start_import(case, &suffix, &dataset, &location, None)
            .await;
        emulator
            .await_active(
                "DescribeDatasetImportJob",
                "datasetImportJobArn",
                "datasetImportJob",
                &job,
                case,
            )
            .await;

        if let Some(mode) = incremental {
            // "Specify INCREMENTAL to append the new records to the existing data in
            // your dataset", so the catalog afterwards holds BOTH imports' items.
            let key = format!("{suffix}/more.csv");
            emulator
                .upload(
                    &key,
                    "USER_ID,ITEM_ID,TIMESTAMP,EVENT_TYPE\nuser-4,item-55,1700000500,watch\n",
                )
                .await;
            expected.insert("item-55".to_string());
            let second = emulator
                .start_import(
                    case,
                    &suffix,
                    &dataset,
                    &format!("s3://{BUCKET}/{key}"),
                    Some(mode),
                )
                .await;
            emulator
                .await_active(
                    "DescribeDatasetImportJob",
                    "datasetImportJobArn",
                    "datasetImportJob",
                    &second,
                    case,
                )
                .await;
        }

        let (_, version, reported) = emulator.train(case, &suffix, &group, USER_RECIPE).await;
        assert_eq!(
            reported["status"].as_str().unwrap_or(""),
            "ACTIVE",
            "case {case:?}: expected the solution version to train on the imported records, \
             got {reported}"
        );
        let campaign = emulator.deploy(case, &suffix, &version, false).await;

        let response = emulator
            .inference_ok(
                RECOMMENDATIONS,
                &json!({"campaignArn": campaign, "userId": "user-1", "numResults": 50}),
                case,
            )
            .await;
        let recommended: BTreeSet<String> = ids(&response, "itemList", "itemId").into_iter().collect();
        assert_eq!(
            recommended, expected,
            "case {case:?}: the campaign must recommend exactly the items the import read"
        );
        assert_descending(case, &scores(&response, "itemList"));
    }
}

// ---------------------------------------------------------------------------
// Metadata and filters created over the wire
// ---------------------------------------------------------------------------

#[tokio::test]
async fn imported_item_columns_serve_metadata_and_a_filter_created_over_the_wire() {
    let emulator = Pipeline::new();
    let case = "metadata and filters";
    let group = emulator.active_group(case, "filtered").await;
    for fixture in [
        interactions_fixture(CATALOG_ITEMS),
        items_fixture(CATALOG_ITEMS),
    ] {
        emulator
            .import_dataset(
                case,
                "filtered",
                &group,
                fixture.dataset_type,
                fixture.columns,
                &fixture.csv,
            )
            .await;
    }
    let (_, version, _) = emulator.train(case, "filtered", &group, USER_RECIPE).await;
    let campaign = emulator.deploy(case, "filtered", &version, true).await;

    // A filter created over the wire: its expression is parsed where it is created, and
    // applied by the inference plane.
    let filter = emulator
        .control_ok(
            "CreateFilter",
            &json!({
                "name": "exclude-comedy",
                "datasetGroupArn": group,
                "filterExpression": "EXCLUDE ItemID WHERE Items.GENRE IN (\"Comedy\")",
            }),
            case,
        )
        .await["filterArn"]
        .as_str()
        .expect("a filterArn")
        .to_string();
    let described = emulator
        .await_active("DescribeFilter", "filterArn", "filter", &filter, case)
        .await;
    assert_eq!(
        described["filterExpression"].as_str().unwrap_or(""),
        "EXCLUDE ItemID WHERE Items.GENRE IN (\"Comedy\")",
        "case {case:?}: DescribeFilter must report the expression as created, got {described}"
    );

    // Unfiltered: every recommended item reports the GENRE its Items row carried.
    let unfiltered = emulator
        .inference_ok(
            RECOMMENDATIONS,
            &json!({
                "campaignArn": campaign,
                "userId": "user-1",
                "numResults": 50,
                "metadataColumns": {"ITEMS": ["GENRE"]},
            }),
            case,
        )
        .await;
    let genres = ["Comedy", "Drama", "Action"];
    let mut comedies = 0_usize;
    for entry in unfiltered["itemList"].as_array().expect("an itemList") {
        let id = entry["itemId"].as_str().expect("an itemId");
        let index: usize = id
            .trim_start_matches("item-")
            .parse()
            .unwrap_or_else(|_| panic!("case {case:?}: unexpected item id {id:?}"));
        let reported = entry["metadata"]["GENRE"].as_str().unwrap_or("");
        assert_eq!(
            reported,
            genres[index % genres.len()],
            "case {case:?}: {id} must report the GENRE its imported Items row carried, got {entry}"
        );
        if reported == "Comedy" {
            comedies += 1;
        }
    }
    assert!(
        comedies > 0,
        "case {case:?}: the fixture must hold a Comedy item for the filter to exclude"
    );

    // Filtered: the excluded items are gone, the rest are untouched.
    let filtered = emulator
        .inference_ok(
            RECOMMENDATIONS,
            &json!({
                "campaignArn": campaign,
                "userId": "user-1",
                "numResults": 50,
                "filterArn": filter,
                "metadataColumns": {"ITEMS": ["GENRE"]},
            }),
            case,
        )
        .await;
    for entry in filtered["itemList"].as_array().expect("an itemList") {
        assert_ne!(
            entry["metadata"]["GENRE"].as_str().unwrap_or(""),
            "Comedy",
            "case {case:?}: the filter excludes every Comedy item, got {entry}"
        );
    }
    assert_eq!(
        ids(&filtered, "itemList", "itemId").len(),
        ids(&unfiltered, "itemList", "itemId").len() - comedies,
        "case {case:?}: the filter removes exactly the Comedy items"
    );

    // An expression the runtime could not apply is refused where the filter is created.
    let (status, body) = emulator
        .control(
            "CreateFilter",
            &json!({
                "name": "not-an-expression",
                "datasetGroupArn": group,
                "filterExpression": "KEEP EVERYTHING",
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected CreateFilter to refuse an unparsable expression, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "case {case:?}: expected InvalidInputException, got {body}"
    );
}

// ---------------------------------------------------------------------------
// Work that could not be carried out
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_import_that_cannot_be_carried_out_fails_the_job_and_the_pipeline() {
    let emulator = Pipeline::new();
    let case = "unreadable data";
    let suffix = "failed";
    let group = emulator.active_group(case, suffix).await;
    let schema = emulator
        .control_ok(
            "CreateSchema",
            &json!({
                "name": format!("interactions-{suffix}"),
                "schema": avro_schema(
                    "Interactions",
                    &["USER_ID", "ITEM_ID", "TIMESTAMP", "EVENT_TYPE"],
                ),
            }),
            case,
        )
        .await["schemaArn"]
        .as_str()
        .expect("a schemaArn")
        .to_string();
    let dataset = emulator
        .control_ok(
            "CreateDataset",
            &json!({
                "name": format!("interactions-{suffix}"),
                "datasetGroupArn": group,
                "datasetType": "Interactions",
                "schemaArn": schema,
            }),
            case,
        )
        .await["datasetArn"]
        .as_str()
        .expect("a datasetArn")
        .to_string();

    // A location this emulator cannot import from at all is refused at the submission:
    // the modelled pattern admits `http` and `https`, which name no Amazon S3 data, and
    // an `s3` URI that names no bucket.
    for location in [
        format!("https://example.invalid/{BUCKET}/history.csv"),
        "s3:///history.csv".to_string(),
    ] {
        let (status, body) = emulator
            .control(
                "CreateDatasetImportJob",
                &json!({
                    "jobName": format!("refused-{}", short_hash(&location)),
                    "datasetArn": dataset,
                    "dataSource": {"dataLocation": location},
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {location} to be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {case:?}: expected InvalidInputException for {location}, got {body}"
        );
    }

    // An INCREMENTAL import into a dataset nothing was bulk-imported into yet: "If you
    // haven't imported bulk records into the dataset previously, you can only specify
    // FULL."
    let (status, body) = emulator
        .control(
            "CreateDatasetImportJob",
            &json!({
                "jobName": "incremental-first",
                "datasetArn": dataset,
                "dataSource": {"dataLocation": format!("s3://{BUCKET}/{suffix}/history.csv")},
                "importMode": "INCREMENTAL",
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a first INCREMENTAL import to be refused, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "case {case:?}: expected InvalidInputException, got {body}"
    );

    // An S3 path that holds no object: the job is created, and its own status reports
    // that the work did not succeed.
    let location = format!("s3://{BUCKET}/{suffix}/not-there.csv");
    emulator.upload(&format!("{suffix}/other.csv"), "ITEM_ID\n").await;
    let job = emulator
        .start_import(case, suffix, &dataset, &location, None)
        .await;
    let reported = emulator
        .await_terminal(
            "DescribeDatasetImportJob",
            "datasetImportJobArn",
            "datasetImportJob",
            &job,
            case,
        )
        .await;
    assert_eq!(
        reported["status"].as_str().unwrap_or(""),
        "CREATE FAILED",
        "case {case:?}: an import that read nothing must report CREATE FAILED, got {reported}"
    );
    let reason = reported["failureReason"].as_str().unwrap_or("");
    assert!(
        reason.contains(&location),
        "case {case:?}: the failure must name the location it could not read, got {reported}"
    );

    // Nothing was imported, so there is nothing to train on: the solution version
    // reports the failure, and a campaign cannot deploy it.
    let (_, version, trained) = emulator.train(case, suffix, &group, USER_RECIPE).await;
    assert_eq!(
        trained["status"].as_str().unwrap_or(""),
        "CREATE FAILED",
        "case {case:?}: training without interactions must report CREATE FAILED, got {trained}"
    );
    assert!(
        trained["failureReason"]
            .as_str()
            .unwrap_or("")
            .contains(&group),
        "case {case:?}: the failure must name the dataset group it found no data in, got {trained}"
    );
    let (status, body) = emulator
        .control(
            "CreateCampaign",
            &json!({"name": "no-model", "solutionVersionArn": version}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected CreateCampaign to refuse an untrained version, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "case {case:?}: expected InvalidInputException, got {body}"
    );

    // And the campaign that was never created is what the inference plane refuses.
    let (status, body) = emulator
        .inference(
            RECOMMENDATIONS,
            &json!({
                "campaignArn": format!("arn:aws:personalize:{REGION}:{ACCOUNT}:campaign/no-model"),
                "userId": "user-1",
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected the inference plane to refuse a campaign nothing created, \
         got {status} {body}"
    );
    assert!(
        error_message(&body).contains("does not exist or is not active yet"),
        "case {case:?}: expected the unresolvable-campaign refusal, got {body}"
    );
}

// ---------------------------------------------------------------------------
// A failed import leaves the dataset as it was
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_import_that_cannot_place_a_record_leaves_the_dataset_as_it_was() {
    let emulator = Pipeline::new();
    let case = "a record with no entity id";
    let suffix = "untouched";
    let group = emulator.active_group(case, suffix).await;
    emulator
        .import_dataset(
            case,
            suffix,
            &group,
            "Interactions",
            &["USER_ID", "ITEM_ID", "TIMESTAMP", "EVENT_TYPE"],
            &interactions_csv(CATALOG_ITEMS),
        )
        .await;
    let items = emulator
        .import_dataset(
            case,
            suffix,
            &group,
            "Items",
            &["ITEM_ID", "GENRE"],
            &items_csv(CATALOG_ITEMS),
        )
        .await;

    // An INCREMENTAL import whose row names no entity id cannot be placed, so the whole
    // import fails — and a failed import imports nothing, leaving what the dataset
    // already held exactly as it was.
    let key = format!("{suffix}/no-id.csv");
    emulator.upload(&key, "ITEM_ID,GENRE\n,Documentary\n").await;
    let job = emulator
        .start_import(
            case,
            suffix,
            &items,
            &format!("s3://{BUCKET}/{key}"),
            Some("INCREMENTAL"),
        )
        .await;
    let reported = emulator
        .await_terminal(
            "DescribeDatasetImportJob",
            "datasetImportJobArn",
            "datasetImportJob",
            &job,
            case,
        )
        .await;
    assert_eq!(
        reported["status"].as_str().unwrap_or(""),
        "CREATE FAILED",
        "case {case:?}: a record that cannot be placed must fail the import, got {reported}"
    );
    assert!(
        reported["failureReason"]
            .as_str()
            .unwrap_or("")
            .contains("item_id"),
        "case {case:?}: the failure must name the column the record did not carry, got {reported}"
    );

    // What the earlier import stored is still what a model trained now is fitted to:
    // every item still reports the GENRE its Items row carried.
    let (_, version, trained) = emulator.train(case, suffix, &group, USER_RECIPE).await;
    assert_eq!(
        trained["status"].as_str().unwrap_or(""),
        "ACTIVE",
        "case {case:?}: the dataset still holds its records, so training succeeds, got {trained}"
    );
    let campaign = emulator.deploy(case, suffix, &version, true).await;
    let response = emulator
        .inference_ok(
            RECOMMENDATIONS,
            &json!({
                "campaignArn": campaign,
                "userId": "user-1",
                "numResults": 50,
                "metadataColumns": {"ITEMS": ["GENRE"]},
            }),
            case,
        )
        .await;
    let genres = ["Comedy", "Drama", "Action"];
    let reported_items = response["itemList"].as_array().expect("an itemList");
    assert_eq!(
        reported_items.len(),
        CATALOG_ITEMS,
        "case {case:?}: every item the earlier import stored is still served, got {response}"
    );
    for entry in reported_items {
        let id = entry["itemId"].as_str().expect("an itemId");
        let index: usize = id
            .trim_start_matches("item-")
            .parse()
            .unwrap_or_else(|_| panic!("case {case:?}: unexpected item id {id:?}"));
        assert_eq!(
            entry["metadata"]["GENRE"].as_str().unwrap_or(""),
            genres[index % genres.len()],
            "case {case:?}: {id} must still report the GENRE the earlier import stored, got {entry}"
        );
    }
}

// ---------------------------------------------------------------------------
// The recipe decides what a solution and a campaign may be
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_recipe_decides_what_a_solution_and_a_campaign_may_be() {
    let emulator = Pipeline::new();
    let case = "recipes";
    let group = emulator.active_group(case, "recipes").await;
    emulator
        .import_dataset(
            case,
            "recipes",
            &group,
            "Interactions",
            &["USER_ID", "ITEM_ID", "TIMESTAMP", "EVENT_TYPE"],
            &interactions_csv(CATALOG_ITEMS),
        )
        .await;

    // A recipe ARN the catalog does not hold names no recipe at all.
    let (status, body) = emulator
        .control(
            "CreateSolution",
            &json!({
                "name": "unknown-recipe",
                "datasetGroupArn": group,
                "recipeArn": "arn:aws:personalize:::recipe/aws-not-a-recipe",
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected an unknown recipe to be refused, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "case {case:?}: expected ResourceNotFoundException, got {body}"
    );

    // `recipeArn` and `performAutoML` are alternatives: "recipeArn must not be
    // specified" with AutoML, and it "is required when performAutoML is false".
    for (row, request) in [
        (
            "both",
            json!({
                "name": "automl-and-recipe",
                "datasetGroupArn": group,
                "performAutoML": true,
                "recipeArn": USER_RECIPE,
            }),
        ),
        (
            "neither",
            json!({"name": "no-recipe", "datasetGroupArn": group}),
        ),
    ] {
        let (status, body) = emulator.control("CreateSolution", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}/{row}: expected CreateSolution to refuse, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {case:?}/{row}: expected InvalidInputException, got {body}"
        );
    }

    // AutoML "performs a search for the best USER_PERSONALIZATION recipe", and what it
    // settled on is what the solution reports.
    let auto = emulator
        .control_ok(
            "CreateSolution",
            &json!({
                "name": "automl",
                "datasetGroupArn": group,
                "performAutoML": true,
            }),
            case,
        )
        .await["solutionArn"]
        .as_str()
        .expect("a solutionArn")
        .to_string();
    let described = emulator
        .await_active("DescribeSolution", "solutionArn", "solution", &auto, case)
        .await;
    let best = described["autoMLResult"]["bestRecipeArn"]
        .as_str()
        .unwrap_or("");
    assert_eq!(
        best, USER_RECIPE,
        "case {case:?}: AutoML reports the recipe it searched out, got {described}"
    );
    assert_eq!(
        described["recipeArn"].as_str().unwrap_or(""),
        best,
        "case {case:?}: the solution trains with the recipe AutoML chose, got {described}"
    );

    // UPDATE training is available only where the recipe supports incremental training.
    for (row, recipe, mode, expected_status) in [
        ("user personalization", USER_RECIPE, "UPDATE", 200),
        ("personalized ranking", RANKING_RECIPE, "UPDATE", 400),
        ("personalized ranking full", RANKING_RECIPE, "FULL", 200),
    ] {
        let solution = emulator
            .control_ok(
                "CreateSolution",
                &json!({
                    "name": format!("mode-{}", short_hash(&format!("{row}{mode}"))),
                    "datasetGroupArn": group,
                    "recipeArn": recipe,
                }),
                case,
            )
            .await["solutionArn"]
            .as_str()
            .expect("a solutionArn")
            .to_string();
        let (status, body) = emulator
            .control(
                "CreateSolutionVersion",
                &json!({"solutionArn": solution, "trainingMode": mode}),
            )
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}/{row}: expected trainingMode {mode} to answer {expected_status}, \
             got {status} {body}"
        );
        if expected_status == 200 {
            let version = body["solutionVersionArn"].as_str().expect("an ARN");
            let reported = emulator
                .await_terminal(
                    "DescribeSolutionVersion",
                    "solutionVersionArn",
                    "solutionVersion",
                    version,
                    case,
                )
                .await;
            assert_eq!(
                reported["trainingMode"].as_str().unwrap_or(""),
                mode,
                "case {case:?}/{row}: the version reports the mode it was trained in, got {reported}"
            );
        } else {
            assert_eq!(
                error_code(&body),
                "InvalidInputException",
                "case {case:?}/{row}: expected InvalidInputException, got {body}"
            );
        }
    }

    // A recipe whose results are read in batch is not deployed in a campaign.
    let segmentation = emulator
        .control_ok(
            "CreateSolution",
            &json!({
                "name": "segmentation",
                "datasetGroupArn": group,
                "recipeArn": SEGMENTATION_RECIPE,
            }),
            case,
        )
        .await["solutionArn"]
        .as_str()
        .expect("a solutionArn")
        .to_string();
    let version = emulator
        .control_ok(
            "CreateSolutionVersion",
            &json!({"solutionArn": segmentation}),
            case,
        )
        .await["solutionVersionArn"]
        .as_str()
        .expect("a solutionVersionArn")
        .to_string();
    emulator
        .await_active(
            "DescribeSolutionVersion",
            "solutionVersionArn",
            "solutionVersion",
            &version,
            case,
        )
        .await;
    let (status, body) = emulator
        .control(
            "CreateCampaign",
            &json!({"name": "segmentation", "solutionVersionArn": version}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a batch recipe not to be deployable, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInputException",
        "case {case:?}: expected InvalidInputException, got {body}"
    );
}

// ---------------------------------------------------------------------------
// A campaign is served only once its create is complete
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_campaign_is_served_only_once_its_create_is_complete() {
    let emulator = Pipeline::new();
    let case = "campaign readiness";
    let group = emulator.active_group(case, "ready").await;
    emulator
        .import_dataset(
            case,
            "ready",
            &group,
            "Interactions",
            &["USER_ID", "ITEM_ID", "TIMESTAMP", "EVENT_TYPE"],
            &interactions_csv(CATALOG_ITEMS),
        )
        .await;
    let (solution, version, _) = emulator.train(case, "ready", &group, USER_RECIPE).await;
    assert!(
        !solution.is_empty(),
        "case {case:?}: expected a solution ARN, got none"
    );

    let campaign = emulator
        .control_ok(
            "CreateCampaign",
            &json!({"name": "ready", "solutionVersionArn": version}),
            case,
        )
        .await["campaignArn"]
        .as_str()
        .expect("a campaignArn")
        .to_string();
    let request = json!({"campaignArn": campaign, "userId": "user-1"});

    // While the create is still in progress the inference plane refuses the campaign
    // with the same refusal it gives one that was never created.
    let described = emulator
        .control_ok("DescribeCampaign", &json!({"campaignArn": campaign}), case)
        .await["campaign"]
        .clone();
    if TRANSIENT_STATUSES.contains(&described["status"].as_str().unwrap_or("")) {
        let (status, body) = emulator.inference(RECOMMENDATIONS, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a campaign still being created to be refused, got \
             {status} {body}"
        );
        assert!(
            error_message(&body).contains("does not exist or is not active yet"),
            "case {case:?}: expected the not-active-yet refusal, got {body}"
        );
    }

    // Once ACTIVE, the same request is served.
    emulator
        .await_active("DescribeCampaign", "campaignArn", "campaign", &campaign, case)
        .await;
    let response = emulator.inference_ok(RECOMMENDATIONS, &request, case).await;
    assert!(
        !ids(&response, "itemList", "itemId").is_empty(),
        "case {case:?}: expected the ACTIVE campaign to serve recommendations, got {response}"
    );
    assert_recommendation_id(case, &response);

    // Creating the campaign again collides on the name the ARN is minted from.
    let (status, body) = emulator
        .control(
            "CreateCampaign",
            &json!({"name": "ready", "solutionVersionArn": version}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected a duplicate campaign name to be refused, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceAlreadyExistsException",
        "case {case:?}: expected ResourceAlreadyExistsException, got {body}"
    );
}
