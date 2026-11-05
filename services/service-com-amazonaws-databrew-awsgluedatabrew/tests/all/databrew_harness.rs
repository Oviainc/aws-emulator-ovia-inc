// SPDX-License-Identifier: CC0-1.0

//! Shared harness for the DataBrew suites: a one-service emulator driven over this
//! service's real wire protocol (`restJson1`, the modelled method and URI), so every
//! case exercises the same deserialize -> validate -> handle -> serialize walk a
//! client does and asserts the status, error code and JSON body a client sees.

#![allow(dead_code)]

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The access-key id the harness signs with by default. It is what ROUTES the request
/// (through the credential scope's service name); the identity the service reports is
/// the emulator's own, not this id.
pub const DEFAULT_AKID: &str = "AKIDEXAMPLE";

/// A SigV4 credential scope naming `databrew`: it is what routes a request to this
/// service.
fn authorization(akid: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential={akid}/20260101/us-east-1/databrew/aws4_request, \
         SignedHeaders=host, Signature=0123456789abcdef"
    )
}

/// The ARN prefix every DataBrew resource of this account/region carries.
pub const ARN_PREFIX: &str = "arn:aws:databrew:us-east-1:000000000000:";

/// The ARN of the identity this emulator's credentials speak for, as
/// `CreatedBy`/`LastModifiedBy`/`StartedBy` name it: the IAM user STS answers
/// `GetCallerIdentity` with for a key that names no session.
pub const CALLER_PRINCIPAL: &str = "arn:aws:iam::000000000000:user/emulator";

/// One request's answer: the status, the restJson1 error code (`x-amzn-errortype`)
/// and the decoded body.
pub struct Reply {
    pub status: u16,
    pub error_type: Option<String>,
    pub body: Value,
}

impl Reply {
    /// A top-level string member, or `""` when it is absent.
    pub fn text(&self, member: &str) -> String {
        self.body
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// A top-level member as the wire carries it, or `Value::Null` when the response
    /// omits it. An OPTIONAL member the service has nothing to report for must be
    /// ABSENT from the body, which is what [`Reply::is_absent`] checks.
    pub fn member(&self, member: &str) -> Value {
        self.body.get(member).cloned().unwrap_or(Value::Null)
    }

    /// Whether the response carries no such member at all.
    pub fn is_absent(&self, member: &str) -> bool {
        self.body.get(member).is_none()
    }

    /// The `Message` of a modelled DataBrew error, whichever casing the protocol put it
    /// on the wire with, or `""` when the reply carries none.
    pub fn error_message(&self) -> String {
        self.body
            .get("Message")
            .or_else(|| self.body.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulator under test: this one service mounted on one shared engine.
pub struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The signature itself is not what these cases are about; the credential
        // scope above still routes the request and names the caller.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_databrew_awsgluedatabrew::EmulatedService::new(engine.clone()),
        );
        Self { router, engine }
    }

    /// The SINGLE shared engine this service is mounted on, so a case can read or seed
    /// the very store the handlers serve from (`engine.state::<State>()`) rather than a
    /// private copy.
    pub fn engine(&self) -> &Arc<engine::InMemoryEngine> {
        &self.engine
    }

    /// Serve one request, signed with [`DEFAULT_AKID`].
    pub async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> Reply {
        self.call_as(DEFAULT_AKID, method, uri, body).await
    }

    /// Serve one request signed with `akid`, so a case can vary the credential the
    /// caller presents.
    pub async fn call_as(
        &self,
        akid: &str,
        method: &str,
        uri: &str,
        body: Option<Value>,
    ) -> Reply {
        let payload = body
            .map(|body| serde_json::to_vec(&body).expect("the test body serializes"))
            .unwrap_or_default();
        let request = ::http::Request::builder()
            .method(method)
            .uri(format!("https://databrew.us-east-1.amazonaws.com{uri}"))
            .header("authorization", authorization(akid))
            .header("content-type", "application/json")
            .body(::bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string);
        let body = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_type,
            body,
        }
    }
}

impl Default for Emulator {
    fn default() -> Self {
        Self::new()
    }
}

/// A resource name as it appears in a URI path label. DataBrew names may contain a
/// space, which a URI cannot carry raw.
pub fn label(name: &str) -> String {
    name.replace(' ', "%20")
}

/// The `Input` of a plain S3-backed dataset.
pub fn s3_input(key: &str) -> Value {
    json!({"S3InputDefinition": {"Bucket": "brew-bucket", "Key": key}})
}

pub async fn create_dataset(emulator: &Emulator, name: &str, input: Value) -> Reply {
    emulator
        .call(
            "POST",
            "/datasets",
            Some(json!({"Name": name, "Input": input, "Format": "CSV"})),
        )
        .await
}

/// The role a project assumes, as `CreateProjectRequest$RoleArn` takes it (modelled
/// `len 20..2048`).
pub const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/brew";

/// `CreateProject`: `POST /projects` (model: `smithy_query(query="operation",
/// operation="CreateProject")`).
pub async fn create_project(emulator: &Emulator, body: Value) -> Reply {
    emulator.call("POST", "/projects", Some(body)).await
}

/// `DescribeProject`: `GET /projects/{Name}`.
pub async fn describe_project(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call("GET", &format!("/projects/{}", label(name)), None)
        .await
}

/// `DescribeRecipe` of a recipe's working draft: a created recipe holds only that
/// draft, which the `LATEST_WORKING` alias addresses.
pub async fn describe_working_recipe(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/recipes/{}?recipeVersion=LATEST_WORKING", label(name)),
            None,
        )
        .await
}

/// Seed the dataset and recipe a project is created against, and answer their names.
pub async fn seed_dataset_and_recipe(emulator: &Emulator, suffix: &str) -> (String, String) {
    let dataset = format!("ds-{suffix}");
    let recipe = format!("rc-{suffix}");
    let created_dataset = emulator
        .call(
            "POST",
            "/datasets",
            Some(json!({"Name": dataset, "Input": s3_input("orders.csv"), "Format": "CSV"})),
        )
        .await;
    assert_eq!(
        created_dataset.status, 200,
        "the dataset a project acts upon must be created, got {} {:?}",
        created_dataset.status, created_dataset.body
    );
    let created_recipe = emulator
        .call(
            "POST",
            "/recipes",
            Some(json!({
                "Name": recipe,
                "Steps": [{"Action": {"Operation": "UPPER_CASE", "Parameters": {"sourceColumn": "name"}}}],
            })),
        )
        .await;
    assert_eq!(
        created_recipe.status, 200,
        "the recipe a project develops must be created, got {} {:?}",
        created_recipe.status, created_recipe.body
    );
    (dataset, recipe)
}
